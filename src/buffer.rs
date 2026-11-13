//! Texto editable sobre el archivo original sin cargarlo en memoria.
//!
//! Es una tabla de piezas por líneas: el documento es una secuencia de
//! piezas que son, o bien un rango de líneas del archivo original (que sigue
//! en el `mmap`), o bien una línea nueva/modificada guardada en RAM. Borrar
//! millones de líneas, o deshacerlo, sólo mueve rangos: no copia texto.
//! Sólo las líneas que el usuario toca ocupan memoria.
//!
//! Convención: el texto son las líneas unidas por saltos de línea. Un archivo
//! que termina en `\n` tiene una última línea vacía (como en cualquier editor).

use crate::document::{self, Document, Index, Source};
use crate::encoding;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// Líneas `[line, line + count)` del archivo original, intactas.
    Orig { line: u64, count: u64 },
    /// Una línea nueva o modificada (texto decodificado, sin salto de línea).
    New(Arc<str>),
}

impl Piece {
    pub fn lines(&self) -> u64 {
        match self {
            Piece::Orig { count, .. } => *count,
            Piece::New(_) => 1,
        }
    }
}

/// Posición en el texto: línea y offset en bytes dentro de la línea
/// decodificada (UTF-8), siempre en un límite de carácter.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Pos {
    pub line: u64,
    pub col: usize,
}

impl Pos {
    pub fn new(line: u64, col: usize) -> Self {
        Pos { line, col }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Typing,
    Deleting,
    Other,
}

/// Cambio reversible: poner `pieces` en lugar de las `n_lines` líneas que
/// empiezan en `at`. Aplicarlo devuelve el cambio inverso.
struct Change {
    at: u64,
    n_lines: u64,
    pieces: Vec<Piece>,
    /// Cursor (ancla, cursor) a restaurar tras aplicar el cambio.
    cursor: (Pos, Pos),
    kind: EditKind,
    /// Cursor tras la edición original (para agrupar tecleo consecutivo).
    after: Pos,
}

/// Qué líneas cambiaron, para ajustar los resultados de búsqueda.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Changed {
    pub at: u64,
    pub removed: u64,
    pub inserted: u64,
}

pub struct Buffer {
    pieces: Vec<Piece>,
    /// `starts[i]` = primera línea del documento que cubre `pieces[i]`.
    starts: Vec<u64>,
    total: u64,
    undo: Vec<Change>,
    redo: Vec<Change>,
    /// Profundidad de `undo` que corresponde al archivo en disco.
    clean: Option<usize>,
    /// Aumenta con cada modificación (sirve para invalidar cachés).
    pub version: u64,
}

const MAX_UNDO: usize = 10_000;

impl Buffer {
    /// Buffer sin cambios sobre un documento ya indexado.
    pub fn new(doc: &Document) -> Self {
        let n = doc.line_count();
        let mut pieces = Vec::new();
        if n > 0 {
            pieces.push(Piece::Orig { line: 0, count: n });
        }
        if doc.ends_with_newline() {
            pieces.push(Piece::New("".into()));
        }
        let mut b =
            Buffer { pieces, starts: Vec::new(), total: 0, undo: Vec::new(), redo: Vec::new(), clean: Some(0), version: 0 };
        b.recompute();
        b
    }

    pub fn line_count(&self) -> u64 {
        self.total
    }

    pub fn is_dirty(&self) -> bool {
        self.clean != Some(self.undo.len())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Nº de piezas y bytes de texto en RAM (para mostrar el consumo).
    pub fn memory(&self) -> (usize, usize) {
        let text: usize = self.pieces.iter().map(|p| if let Piece::New(s) = p { s.len() } else { 0 }).sum();
        (self.pieces.len(), text)
    }

    /// Copia barata (las líneas nuevas se comparten) para otros hilos.
    pub fn snapshot(&self) -> Arc<Vec<Piece>> {
        Arc::new(self.pieces.clone())
    }

    fn recompute(&mut self) {
        self.starts.clear();
        self.starts.reserve(self.pieces.len());
        let mut acc = 0;
        for p in &self.pieces {
            self.starts.push(acc);
            acc += p.lines();
        }
        self.total = acc;
    }

    /// Índice de la pieza que contiene `line` (que debe ser < total).
    fn locate(&self, line: u64) -> usize {
        self.starts.partition_point(|&s| s <= line) - 1
    }

    /// Hace que `line` sea el inicio de una pieza; devuelve su índice.
    fn split_at(&mut self, line: u64) -> usize {
        if line >= self.total {
            return self.pieces.len();
        }
        let i = self.locate(line);
        let k = line - self.starts[i];
        if k == 0 {
            return i;
        }
        let Piece::Orig { line: l, count } = self.pieces[i] else { unreachable!("pieza New con varias líneas") };
        self.pieces[i] = Piece::Orig { line: l, count: k };
        self.pieces.insert(i + 1, Piece::Orig { line: l + k, count: count - k });
        self.starts.insert(i + 1, line);
        i + 1
    }

    /// Reemplaza `n` líneas desde `at` por `ins`; devuelve las piezas quitadas.
    fn splice(&mut self, at: u64, n: u64, ins: Vec<Piece>) -> Vec<Piece> {
        // Ruta rápida: reescribir una línea ya modificada (tecleo normal).
        if n == 1 && ins.len() == 1 && matches!(ins[0], Piece::New(_)) && at < self.total {
            let i = self.locate(at);
            if matches!(self.pieces[i], Piece::New(_)) {
                let old = std::mem::replace(&mut self.pieces[i], ins.into_iter().next().unwrap());
                return vec![old];
            }
        }
        let i = self.split_at(at);
        let j = self.split_at(at + n);
        let ins_len = ins.len();
        let removed: Vec<Piece> = self.pieces.splice(i..j, ins).collect();
        // Une rangos originales contiguos (p. ej. al deshacer) para que no
        // crezca el número de piezas.
        let lo = i.saturating_sub(1);
        let mut k = (i + ins_len).min(self.pieces.len().saturating_sub(1));
        while k > lo {
            if let (Piece::Orig { line: a, count: c }, Piece::Orig { line: b, count: d }) =
                (&self.pieces[k - 1], &self.pieces[k])
            {
                if a + c == *b {
                    self.pieces[k - 1] = Piece::Orig { line: *a, count: c + d };
                    self.pieces.remove(k);
                }
            }
            k -= 1;
        }
        self.recompute();
        removed
    }

    fn apply(&mut self, c: Change, cursor_now: (Pos, Pos)) -> (Change, Changed) {
        let inserted: u64 = c.pieces.iter().map(Piece::lines).sum();
        let removed = self.splice(c.at, c.n_lines, c.pieces);
        self.version += 1;
        let info = Changed { at: c.at, removed: c.n_lines, inserted };
        (
            Change { at: c.at, n_lines: inserted, pieces: removed, cursor: cursor_now, kind: c.kind, after: c.after },
            info,
        )
    }

    /// Texto completo de una línea.
    pub fn line(&self, doc: &Document, n: u64) -> String {
        if n >= self.total {
            return String::new();
        }
        let i = self.locate(n);
        match &self.pieces[i] {
            Piece::New(s) => s.to_string(),
            Piece::Orig { line, .. } => doc.line_text(line + (n - self.starts[i])),
        }
    }

    /// Hasta `count` líneas desde `first`, decodificadas y recortadas a
    /// `cap` bytes: (nº línea, texto, recortada).
    pub fn rows(&self, doc: &Document, first: u64, count: usize, cap: usize) -> Vec<(u64, String, bool)> {
        let mut out = Vec::with_capacity(count.min(4096));
        if first >= self.total {
            return out;
        }
        let mut i = self.locate(first);
        let mut n = first;
        while out.len() < count && i < self.pieces.len() {
            match &self.pieces[i] {
                Piece::New(s) => {
                    let (t, tr) = cap_str(s, cap);
                    out.push((n, t.to_string(), tr));
                    n += 1;
                }
                Piece::Orig { line, count: c } => {
                    let off = n - self.starts[i];
                    let want = ((c - off) as usize).min(count - out.len());
                    for (k, (_, s, e)) in doc.lines(line + off, want).into_iter().enumerate() {
                        let (t, tr) = doc.src.decode_capped(s, e, cap);
                        out.push((n + k as u64, t, tr));
                    }
                    n += want as u64;
                }
            }
            i += 1;
        }
        out
    }

    /// Texto entre dos posiciones (líneas unidas con `\n`); `None` si supera `max`.
    #[cfg(test)]
    pub fn text(&self, doc: &Document, a: Pos, b: Pos, max: usize) -> Option<String> {
        let mut out = String::new();
        let mut n = a.line;
        while n <= b.line {
            let chunk = self.rows(doc, n, ((b.line - n + 1) as usize).min(4096), usize::MAX);
            if chunk.is_empty() {
                break;
            }
            for (ln, t, _) in chunk {
                let lo = if ln == a.line { floor_char(&t, a.col) } else { 0 };
                let hi = if ln == b.line { floor_char(&t, b.col).max(lo) } else { t.len() };
                if ln != a.line {
                    out.push('\n');
                }
                out.push_str(&t[lo..hi]);
                if out.len() > max {
                    return None;
                }
                n = ln + 1;
            }
        }
        Some(out)
    }

    /// Sustituye el texto entre `a` y `b` (a <= b) por `text`. Devuelve la
    /// posición tras el texto insertado.
    pub fn replace(
        &mut self,
        doc: &Document,
        a: Pos,
        b: Pos,
        text: &str,
        kind: EditKind,
        cursor_before: (Pos, Pos),
    ) -> Result<(Pos, Changed), String> {
        let text = normalize_newlines(text);
        let mut probe = Vec::new();
        if let Err(c) = encoding::encode_into(doc.src.encoding, &text, &mut probe) {
            return Err(tf!("«{c}» no se puede representar en {name}", c = c, name = doc.src.encoding.name()));
        }
        drop(probe);
        let first = self.line(doc, a.line);
        let last = if b.line == a.line { first.clone() } else { self.line(doc, b.line) };
        let (ac, bc) = (floor_char(&first, a.col), floor_char(&last, b.col));
        let suffix = &last[bc..];
        let mut joined = String::with_capacity(ac + text.len() + suffix.len());
        joined.push_str(&first[..ac]);
        joined.push_str(&text);
        joined.push_str(suffix);
        let pieces: Vec<Piece> = joined.split('\n').map(|l| Piece::New(l.into())).collect();
        let end = Pos::new(
            a.line + pieces.len() as u64 - 1,
            match pieces.last() {
                Some(Piece::New(s)) => s.len() - suffix.len(),
                _ => 0,
            },
        );

        let n_lines = b.line - a.line + 1;
        let change = Change { at: a.line, n_lines, pieces, cursor: (end, end), kind, after: end };
        let (inverse, info) = self.apply(change, cursor_before);
        self.push_undo(inverse, cursor_before.1);
        Ok((end, info))
    }

    fn push_undo(&mut self, inv: Change, cursor_before: Pos) {
        self.redo.clear();
        if self.clean.is_some_and(|c| c > self.undo.len()) {
            self.clean = None; // el estado guardado ya no es alcanzable
        }
        // Agrupar tecleo/borrado consecutivo en la misma línea.
        let at_clean = self.clean == Some(self.undo.len());
        if let Some(top) = self.undo.last_mut() {
            let mergeable = inv.kind != EditKind::Other
                && top.kind == inv.kind
                && top.at == inv.at
                && top.n_lines == 1
                && inv.n_lines == 1
                && top.after == cursor_before
                && !at_clean;
            if mergeable {
                top.after = inv.after;
                return;
            }
        }
        self.undo.push(inv);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
            self.clean = self.clean.and_then(|c| c.checked_sub(1));
        }
    }

    /// Deshace; devuelve el cursor a restaurar y qué cambió.
    pub fn undo(&mut self, cursor_now: (Pos, Pos)) -> Option<((Pos, Pos), Changed)> {
        let c = self.undo.pop()?;
        let restore = c.cursor;
        let (mut inv, info) = self.apply(c, cursor_now);
        inv.kind = EditKind::Other;
        self.redo.push(inv);
        Some((restore, info))
    }

    pub fn redo(&mut self, cursor_now: (Pos, Pos)) -> Option<((Pos, Pos), Changed)> {
        let c = self.redo.pop()?;
        let restore = c.cursor;
        let (mut inv, info) = self.apply(c, cursor_now);
        inv.kind = EditKind::Other;
        self.undo.push(inv);
        Some((restore, info))
    }

    /// Reemplaza en bloque las líneas indicadas (ordenadas) con `f`, que
    /// devuelve el nuevo texto o `None` si la línea no cambia. Se hace en una
    /// sola pasada y se deshace en un solo paso.
    pub fn replace_lines(
        &mut self,
        doc: &Document,
        lines: &[u64],
        cursor: (Pos, Pos),
        mut f: impl FnMut(&str) -> Option<String>,
    ) -> Result<usize, String> {
        let mut out = Vec::with_capacity(self.pieces.len() + lines.len() * 2);
        let mut li = 0;
        let mut changed = 0;
        let enc = doc.src.encoding;
        let check = |t: String| -> Result<Piece, String> {
            if t.contains('\n') {
                return Err(t!("El reemplazo no puede contener saltos de línea").into());
            }
            let mut probe = Vec::new();
            encoding::encode_into(enc, &t, &mut probe)
                .map_err(|c| tf!("«{c}» no se puede representar en {name}", c = c, name = enc.name()))?;
            Ok(Piece::New(t.into()))
        };
        for (pi, piece) in self.pieces.iter().enumerate() {
            let base = self.starts[pi];
            let end = base + piece.lines();
            while li < lines.len() && lines[li] < base {
                li += 1;
            }
            match piece {
                Piece::New(s) => {
                    if li < lines.len() && lines[li] == base {
                        li += 1;
                        if let Some(t) = f(s) {
                            out.push(check(t)?);
                            changed += 1;
                            continue;
                        }
                    }
                    out.push(piece.clone());
                }
                Piece::Orig { line, count } => {
                    let mut cur = *line;
                    while li < lines.len() && lines[li] < end {
                        let ol = line + (lines[li] - base);
                        li += 1;
                        if let Some(t) = f(&doc.line_text(ol)) {
                            if ol > cur {
                                out.push(Piece::Orig { line: cur, count: ol - cur });
                            }
                            out.push(check(t)?);
                            cur = ol + 1;
                            changed += 1;
                        }
                    }
                    if cur < line + count {
                        out.push(Piece::Orig { line: cur, count: line + count - cur });
                    }
                }
            }
        }
        if changed == 0 {
            return Ok(0);
        }
        let total = self.total;
        let old = std::mem::replace(&mut self.pieces, out);
        self.recompute();
        self.version += 1;
        self.push_undo(
            Change { at: 0, n_lines: total, pieces: old, cursor, kind: EditKind::Other, after: cursor.1 },
            cursor.1,
        );
        Ok(changed)
    }
}

/// Escribe el documento completo en `w` en streaming: los rangos originales
/// se copian tal cual desde el `mmap` (conservando sus saltos de línea) y las
/// líneas nuevas se codifican. Devuelve cuántos caracteres no representables
/// se sustituyeron por `?`.
pub fn write_document(
    pieces: &[Piece],
    src: &Source,
    index: &RwLock<Index>,
    newline: &[u8],
    w: &mut impl Write,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    const CHUNK: usize = 8 << 20;
    let data = &src.data[..];
    let idx = index.read().unwrap();
    let mut done: u64 = 0;
    let mut lossy = 0;
    let mut buf = Vec::new();
    w.write_all(&data[..src.start])?;
    for (k, p) in pieces.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelado"));
        }
        let last_piece = k + 1 == pieces.len();
        match p {
            Piece::New(s) => {
                buf.clear();
                lossy += encoding::encode_lossy_into(src.encoding, s, &mut buf);
                if !last_piece {
                    buf.extend_from_slice(newline);
                }
                w.write_all(&buf)?;
                done += buf.len() as u64;
            }
            Piece::Orig { line, count } => {
                let s = document::line_start(src, &idx, *line);
                let last_line = line + count - 1;
                let ls = document::line_start(src, &idx, last_line);
                let next = src.next_line_start(ls);
                // Hasta el final de la última línea, con o sin su salto.
                let (end, add_nl) = if last_piece {
                    (src.content_end(ls, next), false)
                } else {
                    match next {
                        Some(nx) => (nx, false),
                        None => (data.len(), true),
                    }
                };
                let mut p = s;
                while p < end {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelado"));
                    }
                    let e = (p + CHUNK).min(end);
                    w.write_all(&data[p..e])?;
                    done += (e - p) as u64;
                    progress.store(done, Ordering::Relaxed);
                    p = e;
                }
                if add_nl {
                    w.write_all(newline)?;
                }
            }
        }
        progress.store(done, Ordering::Relaxed);
    }
    w.flush()?;
    Ok(lossy)
}

/// Guarda en `target` de forma segura: escribe un temporal en la misma
/// carpeta, lo sincroniza a disco y lo renombra encima del destino. El
/// original nunca queda a medio escribir (y el `mmap` abierto sigue siendo
/// válido, porque apunta al archivo viejo).
pub fn save_file(
    pieces: &[Piece],
    src: &Source,
    index: &RwLock<Index>,
    newline: &[u8],
    target: &Path,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<usize, String> {
    let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().ok_or(t!("la ruta no tiene nombre de archivo"))?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.buscador-tmp"));
    let res = (|| -> io::Result<usize> {
        let f = std::fs::File::create(&tmp)?;
        let mut w = io::BufWriter::with_capacity(1 << 20, f);
        let lossy = write_document(pieces, src, index, newline, &mut w, progress, cancel)?;
        let f = w.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        drop(f);
        if let Ok(m) = std::fs::metadata(&target) {
            std::fs::set_permissions(&tmp, m.permissions())?;
        }
        std::fs::rename(&tmp, &target)?;
        Ok(lossy)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res.map_err(|e| e.to_string())
}

/// `\r\n` y `\r` sueltos pasan a `\n`.
fn normalize_newlines(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n").into() } else { s.into() }
}

/// Mayor límite de carácter <= `i`.
pub fn floor_char(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn cap_str(s: &str, cap: usize) -> (&str, bool) {
    if s.len() <= cap { (s, false) } else { (&s[..floor_char(s, cap)], true) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn open(name: &str, bytes: &[u8]) -> Document {
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p: PathBuf = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        let doc = Document::open(&p).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        doc
    }

    fn save(b: &Buffer, doc: &Document) -> Vec<u8> {
        let mut out = Vec::new();
        write_document(
            &b.pieces,
            &doc.src,
            &doc.index,
            &doc.newline_bytes(),
            &mut out,
            &AtomicU64::new(0),
            &AtomicBool::new(false),
        )
        .unwrap();
        out
    }

    fn all(b: &Buffer, doc: &Document) -> String {
        b.text(doc, Pos::new(0, 0), Pos::new(b.line_count() - 1, usize::MAX), usize::MAX).unwrap()
    }

    const C: (Pos, Pos) = (Pos { line: 0, col: 0 }, Pos { line: 0, col: 0 });

    #[test]
    fn untouched_roundtrip() {
        for (i, t) in ["", "a", "a\n", "a\r\nb\r\n", "\n\n", "x\ny"].iter().enumerate() {
            let doc = open(&format!("rt{i}.txt"), t.as_bytes());
            let b = Buffer::new(&doc);
            assert_eq!(save(&b, &doc), t.as_bytes(), "caso {t:?}");
            assert_eq!(all(&b, &doc), t.replace('\r', ""));
        }
    }

    #[test]
    fn edits_undo_redo() {
        let mut s = String::new();
        for i in 0..2000 {
            s.push_str(&format!("línea {i}\r\n"));
        }
        let doc = open("ed.txt", s.as_bytes());
        let mut b = Buffer::new(&doc);
        assert_eq!(b.line_count(), 2001);
        // Escribir en medio de una línea.
        let (p, _) = b.replace(&doc, Pos::new(1000, 0), Pos::new(1000, 0), "¡", EditKind::Typing, C).unwrap();
        let (p, _) = b.replace(&doc, p, p, "hola ", EditKind::Typing, (p, p)).unwrap();
        assert_eq!(b.line(&doc, 1000), "¡hola línea 1000");
        assert_eq!(b.undo.len(), 1, "el tecleo consecutivo se agrupa");
        // Partir una línea y borrar un rango grande entre líneas.
        let (_, ch) = b.replace(&doc, p, p, "\n", EditKind::Other, (p, p)).unwrap();
        assert_eq!(ch, Changed { at: 1000, removed: 1, inserted: 2 });
        assert_eq!(b.line(&doc, 1001), "línea 1000");
        b.replace(&doc, Pos::new(10, 3), Pos::new(1500, 2), "", EditKind::Deleting, C).unwrap();
        assert_eq!(b.line(&doc, 10), "líínea 1499");
        assert_eq!(b.line_count(), 2001 + 1 - 1490);
        let saved = save(&b, &doc);
        let text = String::from_utf8(saved).unwrap();
        assert!(text.contains("línea 9\r\nlíínea 1499\r\nlínea 1500\r\n"));
        assert!(text.ends_with("línea 1999\r\n"));
        // Deshacer todo vuelve al original y une las piezas.
        while b.undo(C).is_some() {}
        assert!(!b.is_dirty());
        assert_eq!(save(&b, &doc), s.as_bytes());
        assert_eq!(b.pieces.len(), 2);
        // Rehacer todo.
        while b.redo(C).is_some() {}
        assert_eq!(String::from_utf8(save(&b, &doc)).unwrap(), text);
    }

    #[test]
    fn last_line_without_newline() {
        let doc = open("nl.txt", b"uno\ndos");
        let mut b = Buffer::new(&doc);
        assert_eq!(b.line_count(), 2);
        b.replace(&doc, Pos::new(0, 3), Pos::new(0, 3), "!", EditKind::Typing, C).unwrap();
        assert_eq!(save(&b, &doc), b"uno!\ndos");
        b.replace(&doc, Pos::new(1, 3), Pos::new(1, 3), "\ntres", EditKind::Other, C).unwrap();
        assert_eq!(save(&b, &doc), b"uno!\ndos\ntres");
        // Borrar la última línea original deja la anterior sin salto.
        let doc = open("nl2.txt", b"a\nb\nc");
        let mut b = Buffer::new(&doc);
        b.replace(&doc, Pos::new(1, 1), Pos::new(2, 1), "", EditKind::Deleting, C).unwrap();
        assert_eq!(save(&b, &doc), b"a\nb");
        // Insertar tras un rango original cuya última línea no tenía salto.
        let mut b = Buffer::new(&doc);
        b.replace(&doc, Pos::new(0, 0), Pos::new(0, 0), "x", EditKind::Typing, C).unwrap();
        assert_eq!(save(&b, &doc), b"xa\nb\nc");
    }

    #[test]
    fn utf16_and_unrepresentable() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in "uno\r\ndos\r\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let doc = open("u16.txt", &bytes);
        let mut b = Buffer::new(&doc);
        b.replace(&doc, Pos::new(1, 0), Pos::new(1, 3), "日本\nñ", EditKind::Other, C).unwrap();
        let out = save(&b, &doc);
        assert_eq!(&out[..2], &[0xFF, 0xFE]);
        let u: Vec<u16> = out[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&u).unwrap(), "uno\r\n日本\r\nñ\r\n");

        let (l1, _, _) = encoding_rs::WINDOWS_1252.encode("año\n");
        let doc = open("l1.txt", &l1);
        let mut b = Buffer::new(&doc);
        assert!(b.replace(&doc, Pos::new(0, 0), Pos::new(0, 0), "日", EditKind::Typing, C).is_err());
        assert!(!b.is_dirty());
        b.replace(&doc, Pos::new(0, 0), Pos::new(0, 0), "€", EditKind::Typing, C).unwrap();
        assert_eq!(save(&b, &doc), b"\x80a\xF1o\n");
    }

    #[test]
    fn bulk_replace() {
        let doc = open("bulk.txt", b"a1\nb2\na3\nc4\na5\n");
        let mut b = Buffer::new(&doc);
        let n = b.replace_lines(&doc, &[0, 2, 4], C, |l| Some(l.replace('a', "Z"))).unwrap();
        assert_eq!(n, 3);
        assert_eq!(save(&b, &doc), b"Z1\nb2\nZ3\nc4\nZ5\n");
        b.undo(C).unwrap();
        assert_eq!(save(&b, &doc), b"a1\nb2\na3\nc4\na5\n");
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    /// BIG_LOG=/ruta cargo test --release bench_edit -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_edit_save() {
        let path = std::path::PathBuf::from(std::env::var("BIG_LOG").expect("BIG_LOG"));
        let doc = Document::open(&path).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let n = doc.line_count();
        let c = (Pos::default(), Pos::default());
        let t = Instant::now();
        let mut b = Buffer::new(&doc);
        b.replace(&doc, Pos::new(n / 2, 0), Pos::new(n / 2, 0), "EDITADO ", EditKind::Typing, c).unwrap();
        b.replace(&doc, Pos::new(1_000_000, 0), Pos::new(2_000_000, 0), "", EditKind::Other, c).unwrap();
        b.replace(&doc, Pos::new(0, 0), Pos::new(0, 0), "cabecera\n", EditKind::Other, c).unwrap();
        println!("3 ediciones (incl. borrar 1 M de líneas) en {:.2?}; piezas/RAM: {:?}", t.elapsed(), b.memory());
        let out = path.with_extension("guardado.log");
        let t = Instant::now();
        let r = save_file(&b.snapshot(), &doc.src, &doc.index, &doc.newline_bytes(), &out, &AtomicU64::new(0), &AtomicBool::new(false));
        println!("guardado {:?} en {:.2?}: {} bytes", r, t.elapsed(), std::fs::metadata(&out).unwrap().len());
        let saved = Document::open(&out).unwrap();
        while !saved.indexing_done() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(saved.line_count(), n + 1 - 1_000_000);
        assert_eq!(saved.line_text(0), "cabecera");
        assert!(saved.line_text(n / 2 + 1 - 1_000_000).starts_with("EDITADO "));
        while b.undo(c).is_some() {}
        assert_eq!(b.pieces.len(), 2, "deshacer todo vuelve a 2 piezas");
        let _ = std::fs::remove_file(&out);
    }
}
