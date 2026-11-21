//! Autoguardado para recuperar cambios sin guardar si la app se cierra de golpe.
//!
//! Cada archivo con cambios tiene un «diario» en `<config>/recuperacion/PID-N.txt`
//! que se reescribe poco después de cada edición (en un hilo) y se borra al
//! guardar, al descartar o al cerrar la app normalmente. Si al arrancar quedan
//! diarios de un proceso que ya no existe, se ofrece recuperarlos.
//!
//! Para no copiar archivos de GB, un diario guarda la tabla de piezas (rangos
//! de líneas del original + líneas editadas) junto con la huella del original
//! ([`Stamp`]): solo se puede aplicar si el archivo en disco sigue igual. Si lo
//! editado no se apoya en el disco (archivo nuevo, o se siguió editando tras
//! guardar) se guarda el texto completo, hasta [`MAX_TEXT`]; por encima, solo
//! las líneas editadas.

use crate::buffer::{self, Piece};
use crate::document::{Index, Source, Stamp};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// Tamaño máximo de un diario con el texto completo.
pub const MAX_TEXT: u64 = 64 << 20;
const MAGIC: &str = "buscador-recuperacion 1";

pub fn dir() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("recuperacion"))
}

/// Nombre de diario nuevo para un archivo abierto en este proceso.
pub fn new_file() -> Option<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(1);
    Some(dir()?.join(format!("{}-{}.txt", std::process::id(), N.fetch_add(1, Ordering::Relaxed))))
}

/// Datos de la pestaña que acompañan al contenido.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub path: PathBuf,
    pub untitled: bool,
    pub name: String,
    pub encoding: String,
    pub top: u64,
    pub cursor: (u64, usize),
}

/// Lo que se escribe (lo prepara la pestaña; se escribe en otro hilo).
pub enum Content {
    /// Piezas sobre el original intacto (con su huella).
    Pieces(Stamp, Arc<Vec<Piece>>),
    /// Texto completo, en el encoding del documento.
    Text { pieces: Arc<Vec<Piece>>, src: Source, index: Arc<RwLock<Index>>, newline: Vec<u8> },
    /// Solo las líneas editadas (no cabe el texto completo y el original cambió).
    Partial(Arc<Vec<Piece>>),
}

/// Lo leído de un diario.
#[derive(Debug, PartialEq)]
pub enum Body {
    Pieces(Stamp, Vec<Piece>),
    /// Bytes en el encoding de `Head::encoding`.
    Text(Vec<u8>),
    Partial(Vec<String>),
}

#[derive(Debug)]
pub struct Journal {
    pub file: PathBuf,
    pub head: Head,
    pub body: Body,
    /// Cuándo se escribió (segundos desde 1970).
    pub when: u64,
}

impl Journal {
    /// Texto de las líneas editadas (para cuando el original ya no sirve).
    pub fn edited_lines(&self) -> String {
        match &self.body {
            Body::Pieces(_, p) => p.iter().filter_map(|p| if let Piece::New(s) = p { Some(&**s) } else { None }).collect::<Vec<_>>().join("\n"),
            Body::Partial(l) => l.join("\n"),
            Body::Text(b) => decode(b, &self.head.encoding),
        }
    }

    /// ¿Se puede recuperar entero? (Las piezas exigen el original intacto.)
    pub fn complete(&self) -> bool {
        match &self.body {
            Body::Pieces(st, _) => Stamp::read(&self.head.path).is_ok_and(|now| now == *st),
            Body::Text(_) => true,
            Body::Partial(_) => false,
        }
    }
}

pub fn decode(bytes: &[u8], encoding: &str) -> String {
    let enc = encoding_rs::Encoding::for_label(encoding.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    enc.decode(bytes).0.into_owned()
}

/// Escribe el diario (a un temporal y luego lo renombra encima). Si
/// `alive` se apaga mientras tanto (se cerró el archivo), no deja nada.
pub fn write(file: &Path, head: &Head, content: Content, alive: &AtomicBool) -> io::Result<()> {
    std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
    let tmp = file.with_extension("tmp");
    let r = (|| {
        let mut w = io::BufWriter::new(std::fs::File::create(&tmp)?);
        let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        writeln!(w, "{MAGIC}")?;
        writeln!(w, "ruta\t{}", head.path.display())?;
        writeln!(w, "sin_titulo\t{}", head.untitled as u8)?;
        writeln!(w, "nombre\t{}", head.name)?;
        writeln!(w, "encoding\t{}", head.encoding)?;
        writeln!(w, "arriba\t{}", head.top)?;
        writeln!(w, "cursor\t{} {}", head.cursor.0, head.cursor.1)?;
        writeln!(w, "cuando\t{when}")?;
        match content {
            Content::Pieces(st, pieces) => {
                writeln!(w, "piezas\t{} {} {} {} {}", st.id.0, st.id.1, st.len, st.mtime, pieces.len())?;
                write_pieces(&mut w, &pieces)?;
            }
            Content::Partial(pieces) => {
                let new: Vec<Piece> = pieces.iter().filter(|p| matches!(p, Piece::New(_))).cloned().collect();
                writeln!(w, "parcial\t{}", new.len())?;
                write_pieces(&mut w, &new)?;
            }
            Content::Text { pieces, src, index, newline } => {
                writeln!(w, "texto")?;
                let stop = AtomicBool::new(false);
                buffer::write_document(&pieces, &src, &index, &newline, &mut w, &AtomicU64::new(0), &stop)?;
            }
        }
        w.into_inner().map_err(|e| e.into_error())?.sync_data()
    })();
    if r.is_err() || !alive.load(Ordering::Acquire) {
        let _ = std::fs::remove_file(&tmp);
        return r;
    }
    std::fs::rename(&tmp, file)
}

fn write_pieces(w: &mut impl Write, pieces: &[Piece]) -> io::Result<()> {
    for p in pieces {
        match p {
            Piece::Orig { line, count } => writeln!(w, "O {line} {count}")?,
            Piece::New(s) => {
                writeln!(w, "N {}", s.len())?;
                w.write_all(s.as_bytes())?;
                w.write_all(b"\n")?;
            }
        }
    }
    Ok(())
}

pub fn read(file: &Path) -> io::Result<Journal> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, format!("diario dañado: {what}"));
    let mut r = BufReader::new(std::fs::File::open(file)?);
    let mut line = String::new();
    let mut next = |r: &mut BufReader<std::fs::File>| -> io::Result<String> {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return Err(bad("se corta"));
        }
        Ok(line.trim_end_matches(['\n', '\r']).to_string())
    };
    if next(&mut r)? != MAGIC {
        return Err(bad("cabecera"));
    }
    let mut head = Head { path: PathBuf::new(), untitled: false, name: String::new(), encoding: "UTF-8".into(), top: 0, cursor: (0, 0) };
    let mut when = 0;
    loop {
        let l = next(&mut r)?;
        let (k, v) = l.split_once('\t').unwrap_or((&l, ""));
        let nums = || v.split(' ').map(|n| n.parse::<u128>().unwrap_or(0)).collect::<Vec<_>>();
        match k {
            "ruta" => head.path = PathBuf::from(v),
            "sin_titulo" => head.untitled = v == "1",
            "nombre" => head.name = v.to_string(),
            "encoding" => head.encoding = v.to_string(),
            "arriba" => head.top = v.parse().unwrap_or(0),
            "cursor" => {
                let n = nums();
                head.cursor = (*n.first().unwrap_or(&0) as u64, *n.get(1).unwrap_or(&0) as usize);
            }
            "cuando" => when = v.parse().unwrap_or(0),
            "piezas" | "parcial" => {
                let n = nums();
                let count = *n.last().ok_or_else(|| bad("piezas"))? as usize;
                let mut pieces = Vec::with_capacity(count.min(1 << 20));
                for _ in 0..count {
                    let l = next(&mut r)?;
                    let mut it = l.split(' ');
                    let kind = it.next().unwrap_or("");
                    let a: u64 = it.next().and_then(|x| x.parse().ok()).ok_or_else(|| bad("pieza"))?;
                    if kind == "O" {
                        let c: u64 = it.next().and_then(|x| x.parse().ok()).ok_or_else(|| bad("pieza"))?;
                        pieces.push(Piece::Orig { line: a, count: c });
                    } else if kind == "N" {
                        let mut b = vec![0; a as usize + 1];
                        r.read_exact(&mut b)?;
                        b.pop();
                        pieces.push(Piece::New(String::from_utf8(b).map_err(|_| bad("texto"))?.into()));
                    } else {
                        return Err(bad("pieza"));
                    }
                }
                let body = if k == "piezas" {
                    if n.len() != 5 {
                        return Err(bad("huella"));
                    }
                    Body::Pieces(Stamp { id: (n[0] as u64, n[1] as u64), len: n[2] as u64, mtime: n[3] }, pieces)
                } else {
                    Body::Partial(pieces.into_iter().filter_map(|p| if let Piece::New(s) = p { Some(s.to_string()) } else { None }).collect())
                };
                return Ok(Journal { file: file.to_path_buf(), head, body, when });
            }
            "texto" => {
                let mut b = Vec::new();
                r.read_to_end(&mut b)?;
                return Ok(Journal { file: file.to_path_buf(), head, body: Body::Text(b), when });
            }
            _ => {}
        }
    }
}

/// Diarios que dejaron procesos que ya no existen (cierres inesperados).
pub fn pending() -> Vec<Journal> {
    let Some(dir) = dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let me = std::process::id();
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(pid) = name.strip_suffix(".txt").and_then(|n| n.split('-').next()).and_then(|n| n.parse::<u32>().ok()) else {
            // Temporales a medio escribir de procesos muertos.
            if let Some(pid) = name.strip_suffix(".tmp").and_then(|n| n.split('-').next()).and_then(|n| n.parse::<u32>().ok()) {
                if pid != me && !alive(pid) {
                    let _ = std::fs::remove_file(&p);
                }
            }
            continue;
        };
        if pid == me || alive(pid) {
            continue;
        }
        match read(&p) {
            Ok(j) => out.push(j),
            Err(_) => {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    out.sort_by_key(|j| std::cmp::Reverse(j.when));
    out
}

/// ¿Sigue vivo el proceso `pid`?
fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: la señal 0 no se envía; solo comprueba si el proceso existe.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            fn GetExitCodeProcess(h: *mut std::ffi::c_void, code: *mut u32) -> i32;
            fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
        }
        const QUERY_LIMITED: u32 = 0x1000;
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: llamadas simples a kernel32 con un identificador que se cierra.
        unsafe {
            let h = OpenProcess(QUERY_LIMITED, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0;
            let ok = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE;
            CloseHandle(h);
            ok
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_round_trip() {
        let dir = std::env::temp_dir().join(format!("buscador-diario-{}", std::process::id()));
        let file = dir.join("1-1.txt");
        let head = Head {
            path: PathBuf::from("/tmp/x y.log"),
            untitled: false,
            name: "x y.log".into(),
            encoding: "UTF-8".into(),
            top: 7,
            cursor: (9, 3),
        };
        let st = Stamp { id: (1, 2), len: 300, mtime: 1_700_000_000_123_456_789 };
        let pieces = vec![Piece::Orig { line: 0, count: 5 }, Piece::New("hola\nN 3 raro".into()), Piece::New("".into()), Piece::Orig { line: 6, count: 2 }];
        write(&file, &head, Content::Pieces(st, Arc::new(pieces.clone())), &AtomicBool::new(true)).unwrap();
        let j = read(&file).unwrap();
        assert_eq!(j.head, head);
        assert_eq!(j.body, Body::Pieces(st, pieces.clone()));
        assert!(!j.complete()); // /tmp/x y.log no tiene esa huella

        write(&file, &head, Content::Partial(Arc::new(pieces)), &AtomicBool::new(true)).unwrap();
        assert_eq!(read(&file).unwrap().body, Body::Partial(vec!["hola\nN 3 raro".into(), "".into()]));

        // Si se cerró el archivo mientras se escribía, no queda nada nuevo.
        let other = dir.join("1-2.txt");
        write(&other, &head, Content::Partial(Arc::new(Vec::new())), &AtomicBool::new(false)).unwrap();
        assert!(!other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dead_process_is_not_alive() {
        assert!(alive(std::process::id()));
        let mut c = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" });
        if cfg!(windows) {
            c.args(["/C", "exit"]);
        }
        let mut child = c.spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!alive(pid));
    }
}
