//! Archivo mapeado en memoria con un índice de líneas disperso.
//!
//! El contenido nunca se copia a RAM: se usa `mmap` y el sistema operativo
//! carga/descarta páginas bajo demanda. El índice guarda sólo el offset de
//! una de cada `STRIDE` líneas, así que un archivo de 10 GB con 100 M de
//! líneas necesita ~3 MB de índice.

use crate::encoding::{self, Newline};
use encoding_rs::Encoding;
use memmap2::Mmap;
use std::fs::File;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;

/// Cada cuántas líneas se guarda un punto de control en el índice.
pub const STRIDE: u64 = 256;
/// Cada cuántos bytes el indexador publica su progreso.
const FLUSH_BYTES: usize = 32 << 20;

pub enum Bytes {
    Map(Mmap),
    Empty,
}

impl Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Map(m) => m,
            Bytes::Empty => &[],
        }
    }
}

#[derive(Default)]
pub struct Index {
    /// `checkpoints[k]` = offset del inicio de la línea `k * STRIDE`.
    pub checkpoints: Vec<u64>,
    /// Número de líneas cuyo inicio ya se conoce.
    pub lines: u64,
    pub done: bool,
}

/// Parámetros necesarios para leer líneas; barato de clonar y enviar a hilos.
#[derive(Clone)]
pub struct Source {
    pub data: Arc<Bytes>,
    pub encoding: &'static Encoding,
    pub newline: Newline,
    /// Offset donde empieza el texto (después del BOM, si lo hay).
    pub start: usize,
}

impl Source {
    /// Posición justo después del siguiente salto de línea en o tras `from`.
    pub fn next_line_start(&self, from: usize) -> Option<usize> {
        next_line_start(&self.data, from, self.newline, self.start)
    }

    /// Fin del contenido de una línea (sin `\n` ni `\r`).
    pub fn content_end(&self, line_start: usize, next: Option<usize>) -> usize {
        let data = &self.data[..];
        let u = self.newline.unit();
        let mut end = next.unwrap_or(data.len());
        if next.is_some() {
            end -= u;
        }
        if end >= line_start + u {
            let cr = match self.newline {
                Newline::Byte => data[end - 1] == b'\r',
                Newline::Utf16Le => data[end - 2] == b'\r' && data[end - 1] == 0,
                Newline::Utf16Be => data[end - 2] == 0 && data[end - 1] == b'\r',
            };
            if cr {
                end -= u;
            }
        }
        end
    }

    pub fn decode(&self, start: usize, end: usize) -> String {
        let (s, _) = self.encoding.decode_without_bom_handling(&self.data[start..end]);
        s.into_owned()
    }

    /// Decodifica como mucho `max_bytes` (para no pintar líneas de megas).
    pub fn decode_capped(&self, start: usize, end: usize, max_bytes: usize) -> (String, bool) {
        if end - start <= max_bytes {
            return (self.decode(start, end), false);
        }
        let mut cut = start + max_bytes;
        if self.newline != Newline::Byte && (cut - start) % 2 == 1 {
            cut -= 1;
        }
        (self.decode(start, cut), true)
    }
}

pub fn next_line_start(data: &[u8], from: usize, nl: Newline, base: usize) -> Option<usize> {
    let mut p = from;
    while p < data.len() {
        let i = p + memchr::memchr(b'\n', &data[p..])?;
        match nl {
            Newline::Byte => return Some(i + 1),
            Newline::Utf16Le => {
                if (i - base) % 2 == 0 && data.get(i + 1) == Some(&0) {
                    return Some(i + 2);
                }
            }
            Newline::Utf16Be => {
                if (i - base) % 2 == 1 && data[i - 1] == 0 {
                    return Some(i + 1);
                }
            }
        }
        p = i + 1;
    }
    None
}

/// Líneas `[first, first+count)` usando el índice: (nº línea, inicio, fin de contenido).
pub fn line_spans(src: &Source, idx: &Index, first: u64, count: usize) -> Vec<(u64, usize, usize)> {
    if first >= idx.lines {
        return Vec::new();
    }
    let total = idx.lines;
    let len = src.data.len();
    let mut s = idx.checkpoints[(first / STRIDE) as usize] as usize;
    let mut n = first / STRIDE * STRIDE;
    while n < first {
        s = src.next_line_start(s).unwrap_or(len);
        n += 1;
    }
    let mut out = Vec::with_capacity(count.min(4096));
    while out.len() < count && n < total {
        let next = src.next_line_start(s);
        out.push((n, s, src.content_end(s, next)));
        match next {
            Some(x) if x < len => {
                s = x;
                n += 1;
            }
            _ => break,
        }
    }
    out
}

/// Offset de inicio de la línea `line` (o el final del archivo si no existe).
pub fn line_start(src: &Source, idx: &Index, line: u64) -> usize {
    line_spans(src, idx, line, 1).first().map(|l| l.1).unwrap_or(src.data.len())
}

pub struct Document {
    pub path: PathBuf,
    pub name: String,
    pub src: Source,
    pub detect_reason: &'static str,
    pub index: Arc<RwLock<Index>>,
    /// Bytes ya indexados.
    pub progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    /// Identidad del archivo abierto (dispositivo, inodo) para detectar rotaciones.
    id: (u64, u64),
}

/// Qué pasó con el archivo en disco desde que se abrió (para «seguir en vivo»).
#[derive(Debug, PartialEq)]
pub enum Change {
    Same,
    /// Creció por el final: hay que revisar desde la línea `from_line`
    /// (la última que había, que pudo quedar a medias), que empieza en `from_byte`.
    Grew { from_line: u64, from_byte: usize },
    /// Se truncó, se reemplazó o rotó: hay que volver a abrirlo.
    Replaced,
}

fn file_id(m: &std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        (0, 0)
    }
}

impl Document {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let meta = file.metadata()?;
        let len = meta.len();
        let data = if len == 0 {
            Bytes::Empty
        } else {
            // SAFETY: si otro proceso trunca el archivo mientras está mapeado,
            // el acceso puede fallar (SIGBUS). Aceptable para un visor.
            let m = unsafe { Mmap::map(&file)? };
            #[cfg(unix)]
            let _ = m.advise(memmap2::Advice::Random);
            Bytes::Map(m)
        };
        let data = Arc::new(data);
        let (enc, reason) = encoding::detect(&data);
        let mut doc = Document {
            path: path.to_path_buf(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            src: Source { data, encoding: enc, newline: Newline::Byte, start: 0 },
            detect_reason: reason,
            index: Default::default(),
            progress: Default::default(),
            cancel: Default::default(),
            id: file_id(&meta),
        };
        doc.set_encoding(enc);
        Ok(doc)
    }

    pub fn len(&self) -> usize {
        self.src.data.len()
    }

    /// Cambia el encoding y reconstruye el índice en segundo plano.
    pub fn set_encoding(&mut self, enc: &'static Encoding) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.index = Default::default();
        self.progress = Default::default();

        let start = match Encoding::for_bom(&self.src.data) {
            Some((bom_enc, n)) if bom_enc == enc => n,
            _ => 0,
        };
        self.src.encoding = enc;
        self.src.newline = Newline::for_encoding(enc);
        self.src.start = start;

        let (src, index, progress, cancel) =
            (self.src.clone(), self.index.clone(), self.progress.clone(), self.cancel.clone());
        thread::Builder::new()
            .name("indexer".into())
            .spawn(move || build_index(src, index, progress, cancel, None))
            .expect("no se pudo crear el hilo indexador");
    }

    /// Tras guardar (escribir otro archivo y renombrarlo encima, o «Guardar
    /// como»): el documento pasa a llamarse como `path` y a vigilar ese
    /// archivo, pero sigue leyendo el contenido mapeado de antes (el sistema lo
    /// mantiene mientras esté mapeado), al que se refieren las ediciones.
    pub fn rebind(&mut self, path: &Path) {
        self.path = path.to_path_buf();
        self.name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        if let Ok(m) = std::fs::metadata(path) {
            self.id = file_id(&m);
        }
    }

    /// Mira el archivo en disco. Si solo creció, lo vuelve a mapear y sigue
    /// indexando desde la última línea conocida (sin recorrer lo anterior).
    pub fn check_growth(&mut self) -> std::io::Result<Change> {
        let meta = std::fs::metadata(&self.path)?;
        let len = meta.len() as usize;
        let old = self.len();
        if file_id(&meta) != self.id || len < old {
            return Ok(Change::Replaced);
        }
        if len == old || !self.indexing_done() {
            return Ok(Change::Same);
        }
        let file = File::open(&self.path)?;
        if file_id(&file.metadata()?) != self.id {
            return Ok(Change::Replaced);
        }
        // SAFETY: igual que en `open`.
        let m = unsafe { Mmap::map(&file)? };
        #[cfg(unix)]
        let _ = m.advise(memmap2::Advice::Random);
        if m.len() <= old {
            return Ok(Change::Same);
        }
        // Se reanuda en la última línea (pudo no tener salto todavía).
        let (from_line, from_byte, keep) = {
            let idx = self.index.read().unwrap();
            if idx.lines == 0 {
                (0, self.src.start, Vec::new())
            } else {
                let last = idx.lines - 1;
                let s = line_start(&self.src, &idx, last);
                (last, s, idx.checkpoints[..=(last / STRIDE) as usize].to_vec())
            }
        };
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.src.data = Arc::new(Bytes::Map(m));
        let fresh = old == 0;
        if fresh {
            // Estaba vacío: se detecta el encoding con el contenido nuevo.
            let (enc, reason) = encoding::detect(&self.src.data);
            self.detect_reason = reason;
            self.set_encoding(enc);
            return Ok(Change::Grew { from_line: 0, from_byte: self.src.start });
        }
        let lines = from_line + 1;
        self.index = Arc::new(RwLock::new(Index { checkpoints: keep, lines, done: false }));
        self.progress = Arc::new(AtomicU64::new(from_byte as u64));
        let (src, index, progress, cancel) =
            (self.src.clone(), self.index.clone(), self.progress.clone(), self.cancel.clone());
        thread::Builder::new()
            .name("indexer".into())
            .spawn(move || build_index(src, index, progress, cancel, Some((from_byte, lines))))
            .expect("no se pudo crear el hilo indexador");
        Ok(Change::Grew { from_line, from_byte })
    }

    pub fn line_count(&self) -> u64 {
        self.index.read().unwrap().lines
    }

    pub fn indexing_done(&self) -> bool {
        self.index.read().unwrap().done
    }

    /// Devuelve hasta `count` líneas a partir de `first`: (nº línea, inicio, fin de contenido).
    pub fn lines(&self, first: u64, count: usize) -> Vec<(u64, usize, usize)> {
        line_spans(&self.src, &self.index.read().unwrap(), first, count)
    }

    /// Texto decodificado de una línea completa del archivo original.
    pub fn line_text(&self, n: u64) -> String {
        self.lines(n, 1).first().map(|&(_, s, e)| self.src.decode(s, e)).unwrap_or_default()
    }

    /// ¿El archivo termina en salto de línea (o está vacío)? En ese caso, al
    /// editar, existe una última línea vacía después del último salto.
    pub fn ends_with_newline(&self) -> bool {
        let n = self.line_count();
        if n == 0 {
            return true;
        }
        match self.lines(n - 1, 1).first() {
            Some(&(_, s, _)) => self.src.next_line_start(s).is_some(),
            None => true,
        }
    }

    /// Salto de línea que se usará en las líneas nuevas: el de la primera línea
    /// del archivo (CRLF o LF), ya codificado.
    pub fn newline_bytes(&self) -> Vec<u8> {
        let crlf = self.lines(0, 1).first().is_some_and(|&(_, s, e)| {
            self.src.next_line_start(s).is_some_and(|nx| nx - e > self.src.newline.unit())
        });
        let mut v = Vec::new();
        let _ = crate::encoding::encode_into(self.src.encoding, if crlf { "\r\n" } else { "\n" }, &mut v);
        v
    }

    /// Texto completo de un rango de líneas, limitado a `max_bytes` de origen.
    /// Devuelve `None` si el rango supera el límite.
    #[cfg(test)]
    pub fn text_of_lines(&self, lo: u64, hi: u64, max_bytes: usize) -> Option<String> {
        let first = self.lines(lo, 1);
        let last = self.lines(hi, 1);
        let (&(_, s, _), &(_, _, e)) = (first.first()?, last.first()?);
        if e.saturating_sub(s) > max_bytes {
            return None;
        }
        // Normaliza CRLF -> LF para el portapapeles.
        let mut out = String::with_capacity(e - s);
        let mut pos = s;
        for _ in lo..=hi {
            let next = self.src.next_line_start(pos);
            let end = self.src.content_end(pos, next);
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&self.src.decode(pos, end));
            match next {
                Some(x) if x < self.len() => pos = x,
                _ => break,
            }
        }
        Some(out)
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// `resume` = (offset de la última línea conocida, líneas conocidas): sigue
/// un índice que ya tiene sus puntos de control hasta esa línea.
fn build_index(src: Source, index: Arc<RwLock<Index>>, progress: Arc<AtomicU64>, cancel: Arc<AtomicBool>, resume: Option<(usize, u64)>) {
    let data = &src.data[..];
    let len = data.len();
    let start = src.start;
    if start >= len {
        index.write().unwrap().done = true;
        progress.store(len as u64, Ordering::Relaxed);
        return;
    }

    let (mut pending, mut lines, start): (Vec<u64>, u64, usize) = match resume {
        Some((at, lines)) => (Vec::new(), lines, at),
        None => (vec![start as u64], 1, start),
    };

    let flush = |pending: &mut Vec<u64>, lines: u64, pos: usize, done: bool| {
        let mut idx = index.write().unwrap();
        idx.checkpoints.append(pending);
        idx.lines = lines;
        idx.done = done;
        progress.store(pos as u64, Ordering::Relaxed);
    };

    match src.newline {
        Newline::Byte => {
            // Ruta rápida: memchr vectorizado sobre bloques grandes.
            let mut block = start;
            while block < len {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let end = (block + FLUSH_BYTES).min(len);
                for i in memchr::memchr_iter(b'\n', &data[block..end]) {
                    let ns = block + i + 1;
                    if ns >= len {
                        break;
                    }
                    if lines % STRIDE == 0 {
                        pending.push(ns as u64);
                    }
                    lines += 1;
                }
                block = end;
                flush(&mut pending, lines, block, false);
            }
        }
        nl => {
            let mut p = start;
            let mut last_flush = start;
            while let Some(ns) = next_line_start(data, p, nl, start) {
                if ns >= len {
                    break;
                }
                if lines % STRIDE == 0 {
                    pending.push(ns as u64);
                }
                lines += 1;
                p = ns;
                if p - last_flush > FLUSH_BYTES {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    flush(&mut pending, lines, p, false);
                    last_flush = p;
                }
            }
        }
    }
    flush(&mut pending, lines, len, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn wait(doc: &Document) {
        while !doc.indexing_done() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn write_tmp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::File::create(&p).unwrap().write_all(bytes).unwrap();
        p
    }

    fn all_lines(doc: &Document) -> Vec<String> {
        doc.lines(0, usize::MAX >> 1).into_iter().map(|(_, s, e)| doc.src.decode(s, e)).collect()
    }

    #[test]
    fn utf8_crlf_and_no_trailing_newline() {
        let p = write_tmp("a.log", "uno\r\ndós\r\n\r\ntres".as_bytes());
        let doc = Document::open(&p).unwrap();
        wait(&doc);
        assert_eq!(doc.src.encoding, encoding_rs::UTF_8);
        assert_eq!(all_lines(&doc), vec!["uno", "dós", "", "tres"]);
    }

    #[test]
    fn utf16le_with_bom() {
        let mut b = vec![0xFF, 0xFE];
        for u in "hola\r\n日本\nfin\n".encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        let p = write_tmp("b.log", &b);
        let doc = Document::open(&p).unwrap();
        wait(&doc);
        assert_eq!(doc.src.encoding, encoding_rs::UTF_16LE);
        assert_eq!(all_lines(&doc), vec!["hola", "日本", "fin"]);
    }

    #[test]
    fn utf16be_no_bom_heuristic() {
        let mut b = vec![];
        for u in "linea a\nlinea b\nlinea c".encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        let p = write_tmp("c.log", &b);
        let doc = Document::open(&p).unwrap();
        wait(&doc);
        assert_eq!(doc.src.encoding, encoding_rs::UTF_16BE);
        assert_eq!(all_lines(&doc), vec!["linea a", "linea b", "linea c"]);
    }

    #[test]
    fn latin1_detected() {
        let (b, _, _) = encoding_rs::WINDOWS_1252.encode("Año añejo, acción y canción en España\nñandú\n");
        let p = write_tmp("d.log", &b);
        let doc = Document::open(&p).unwrap();
        wait(&doc);
        assert_ne!(doc.src.encoding, encoding_rs::UTF_8);
        assert_eq!(all_lines(&doc)[1], "ñandú");
    }

    #[test]
    fn grows_and_reindexes_only_the_tail() {
        let mut s = String::new();
        for i in 0..1000 {
            s.push_str(&format!("line {i}\n"));
        }
        s.push_str("medio");
        let p = write_tmp("crece.log", s.as_bytes());
        let mut doc = Document::open(&p).unwrap();
        wait(&doc);
        assert_eq!(doc.check_growth().unwrap(), Change::Same);
        assert_eq!(doc.line_count(), 1001);
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        write!(f, " completo\n").unwrap();
        for i in 1001..1600 {
            write!(f, "line {i}\n").unwrap();
        }
        drop(f);
        match doc.check_growth().unwrap() {
            Change::Grew { from_line, .. } => assert_eq!(from_line, 1000),
            c => panic!("{c:?}"),
        }
        wait(&doc);
        assert_eq!(doc.line_count(), 1600);
        for n in [0u64, 255, 999, 1001, 1024, 1599] {
            assert_eq!(doc.line_text(n), format!("line {n}"));
        }
        assert_eq!(doc.line_text(1000), "medio completo");
        // Truncado: hay que reabrir.
        std::fs::write(&p, "nuevo\n").unwrap();
        assert_eq!(doc.check_growth().unwrap(), Change::Replaced);
    }

    #[test]
    fn many_lines_sparse_index() {
        let mut s = String::new();
        for i in 0..100_000 {
            s.push_str(&format!("line {i}\n"));
        }
        let p = write_tmp("e.log", s.as_bytes());
        let doc = Document::open(&p).unwrap();
        wait(&doc);
        assert_eq!(doc.line_count(), 100_000);
        for n in [0u64, 255, 256, 257, 54_321, 99_999] {
            let v = doc.lines(n, 2);
            assert_eq!(doc.src.decode(v[0].1, v[0].2), format!("line {n}"));
        }
        assert_eq!(doc.text_of_lines(10, 12, 1 << 20).unwrap(), "line 10\nline 11\nline 12");
    }
}
