//! Búsqueda avanzada: bloques de texto delimitados por dos patrones
//! («desde» … «hasta»), que pueden abarcar varias líneas. Sin «hasta», cada
//! coincidencia de «desde» es un bloque.
//!
//! Recorre el texto (original o editado) por bloques de 16 MB sin cargarlo
//! entero, igual que la búsqueda normal.

use crate::buffer::{Piece, Pos};
use crate::document::{self, Index, Source};
use regex::bytes::{Regex, RegexBuilder};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const CHUNK: usize = 16 << 20;
/// En modo multilínea, cuánto texto se vuelve a mirar entre trozos: una
/// coincidencia puede ocupar como mucho esto.
pub const MULTILINE_SPAN: usize = 1 << 20;
/// Máximo de bloques que se guardan (64 bytes cada uno).
pub const MAX_BLOCKS: usize = 1_000_000;

/// Un bloque: `s0..s1` es la coincidencia de inicio y `e0..e1` la de fin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub s0: Pos,
    pub s1: Pos,
    pub e0: Pos,
    pub e1: Pos,
}

#[derive(Clone)]
pub struct BlockParams {
    pub from: String,
    pub to: String,
    pub regex: bool,
    pub case_sensitive: bool,
    /// Los patrones pueden abarcar varias líneas.
    pub multiline: bool,
}

impl BlockParams {
    fn build(&self, pat: &str) -> Result<Regex, String> {
        let p = if self.regex {
            pat.to_string()
        } else if self.multiline {
            // Texto con saltos: vale tanto para LF como para CRLF.
            pat.replace("\r\n", "\n").split('\n').map(regex::escape).collect::<Vec<_>>().join(r"\r?\n")
        } else {
            regex::escape(pat)
        };
        RegexBuilder::new(&p)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()
            .map_err(|e| e.to_string())
    }
}

pub struct BlockSearch {
    pub blocks: Arc<Mutex<Vec<Block>>>,
    pub progress: Arc<AtomicU64>,
    pub total: u64,
    pub done: Arc<AtomicBool>,
    pub truncated: Arc<AtomicBool>,
    /// Inicios encontrados sin su fin (al llegar al final del texto).
    pub unclosed: Arc<AtomicUsize>,
    pub elapsed: Arc<Mutex<Duration>>,
    cancel: Arc<AtomicBool>,
}

impl BlockSearch {
    pub fn start(
        src: Source,
        edits: Option<(Arc<Vec<Piece>>, Arc<RwLock<Index>>)>,
        params: BlockParams,
    ) -> Result<Self, String> {
        if params.from.is_empty() {
            return Err(t!("Escribe qué buscar en «Desde»").into());
        }
        let re_from = params.build(&params.from)?;
        let re_to = if params.to.is_empty() { None } else { Some(params.build(&params.to)?) };
        let multiline = params.multiline;
        let s = BlockSearch {
            blocks: Default::default(),
            progress: Default::default(),
            total: src.data.len() as u64,
            done: Default::default(),
            truncated: Default::default(),
            unclosed: Default::default(),
            elapsed: Default::default(),
            cancel: Default::default(),
        };
        let (blocks, progress, done, truncated, unclosed, elapsed, cancel) = (
            s.blocks.clone(),
            s.progress.clone(),
            s.done.clone(),
            s.truncated.clone(),
            s.unclosed.clone(),
            s.elapsed.clone(),
            s.cancel.clone(),
        );
        std::thread::Builder::new()
            .name("bloques".into())
            .spawn(move || {
                let t0 = Instant::now();
                let mut st = State { re_from, re_to, open: None, found: Vec::new(), blocks: &blocks, truncated: &truncated };
                let edits = edits.as_ref().map(|(p, i)| (&p[..], &**i));
                if multiline {
                    scan_multiline(&src, edits, &cancel, &progress, &mut st, CHUNK, MULTILINE_SPAN);
                } else {
                    for_each_hay(&src, edits, &cancel, &progress, |hay, line| st.feed(hay, line, 0, None).is_some());
                }
                if st.open.is_some() {
                    unclosed.store(1, Ordering::Relaxed);
                }
                st.flush();
                *elapsed.lock().unwrap() = t0.elapsed();
                done.store(true, Ordering::Release);
            })
            .map_err(|e| e.to_string())?;
        Ok(s)
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub fn count(&self) -> usize {
        self.blocks.lock().unwrap().len()
    }

    pub fn get(&self, i: usize) -> Option<Block> {
        self.blocks.lock().unwrap().get(i).copied()
    }

    /// Índice del primer bloque que empieza en `p` o después.
    pub fn first_at_or_after(&self, p: Pos) -> usize {
        self.blocks.lock().unwrap().partition_point(|b| b.s0 < p)
    }
}

impl Drop for BlockSearch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct State<'a> {
    re_from: Regex,
    re_to: Option<Regex>,
    /// Inicio encontrado cuyo fin aún no aparece.
    open: Option<(Pos, Pos)>,
    found: Vec<Block>,
    blocks: &'a Mutex<Vec<Block>>,
    truncated: &'a AtomicBool,
}

/// Convierte offsets crecientes de un trozo en posiciones (línea, columna).
struct Tracker {
    off: usize,
    line: u64,
    line_start: usize,
}

impl Tracker {
    fn pos(&mut self, hay: &[u8], target: usize) -> Pos {
        for i in memchr::memchr_iter(b'\n', &hay[self.off..target]) {
            self.line += 1;
            self.line_start = self.off + i + 1;
        }
        self.off = target;
        Pos::new(self.line, target - self.line_start)
    }
}

impl State<'_> {
    /// Procesa un trozo de texto que empieza en la línea `first_line`, desde
    /// el offset `start`. Con `commit`, sólo acepta coincidencias que empiecen
    /// antes de ese offset (lo demás se vuelve a mirar en el trozo siguiente) y
    /// devuelve desde dónde seguir, relativo a `commit`. `None` = parar.
    fn feed(&mut self, hay: &[u8], first_line: u64, start: usize, commit: Option<usize>) -> Option<usize> {
        let mut tr = Tracker { off: 0, line: first_line, line_start: 0 };
        let limit = commit.unwrap_or(usize::MAX);
        let mut p = start;
        while p <= hay.len() {
            let re = match self.open {
                None => &self.re_from,
                Some(_) => self.re_to.as_ref().unwrap(),
            };
            let Some(m) = re.find_at(hay, p) else { break };
            if m.start() >= limit {
                break;
            }
            let (a, b) = (tr.pos(hay, m.start()), tr.pos(hay, m.end()));
            p = if m.end() > m.start() { m.end() } else { m.end() + 1 };
            match self.open {
                None if self.re_to.is_none() => {
                    if m.end() > m.start() {
                        self.found.push(Block { s0: a, s1: b, e0: b, e1: b });
                    }
                }
                None => self.open = Some((a, b)),
                Some((s0, s1)) => {
                    self.found.push(Block { s0, s1, e0: a, e1: b });
                    self.open = None;
                }
            }
        }
        if !self.flush() {
            return None;
        }
        Some(commit.map_or(0, |c| p.saturating_sub(c)))
    }

    fn flush(&mut self) -> bool {
        let mut r = self.blocks.lock().unwrap();
        r.append(&mut self.found);
        if r.len() >= MAX_BLOCKS {
            r.truncate(MAX_BLOCKS);
            self.truncated.store(true, Ordering::Relaxed);
            return false;
        }
        true
    }
}

/// Modo multilínea: junta el texto (también a través de líneas editadas) en
/// trozos de ~`chunk` bytes que se solapan `span` bytes, para no perder
/// coincidencias que crucen el borde entre dos trozos.
fn scan_multiline(
    src: &Source,
    edits: Option<(&[Piece], &RwLock<Index>)>,
    cancel: &AtomicBool,
    progress: &AtomicU64,
    st: &mut State,
    chunk: usize,
    span: usize,
) {
    let mut acc: Vec<u8> = Vec::new();
    let mut acc_line = 0u64;
    let mut resume = 0usize;
    let mut artificial_nl = false;
    let mut stopped = false;
    for_each_hay(src, edits, cancel, progress, |hay, line| {
        if acc.is_empty() {
            acc_line = line;
        }
        acc.extend_from_slice(hay);
        // Las piezas son líneas: entre una y otra hay un salto.
        artificial_nl = !hay.ends_with(b"\n");
        if artificial_nl {
            acc.push(b'\n');
        }
        while acc.len() >= chunk + span {
            let target = acc.len() - span;
            let commit = memchr::memrchr(b'\n', &acc[..target]).map_or(0, |i| i + 1);
            if commit <= resume {
                break;
            }
            match st.feed(&acc, acc_line, resume, Some(commit)) {
                None => {
                    stopped = true;
                    return false;
                }
                Some(r) => resume = r,
            }
            acc_line += memchr::memchr_iter(b'\n', &acc[..commit]).count() as u64;
            acc.drain(..commit);
        }
        true
    });
    if stopped || cancel.load(Ordering::Relaxed) {
        return;
    }
    if artificial_nl {
        acc.pop();
    }
    st.feed(&acc, acc_line, resume.min(acc.len()), None);
}

/// Recorre el texto en trozos alineados a inicio de línea y ya en UTF-8:
/// llama a `f(trozo, primera_línea)`; si devuelve `false`, para.
pub fn for_each_hay(
    src: &Source,
    edits: Option<(&[Piece], &RwLock<Index>)>,
    cancel: &AtomicBool,
    progress: &AtomicU64,
    mut f: impl FnMut(&[u8], u64) -> bool,
) {
    let mut done_bytes = 0u64;
    let mut range = |from: usize, to: usize, mut line: u64, f: &mut dyn FnMut(&[u8], u64) -> bool| -> bool {
        let data = &src.data[..];
        let mut pos = from;
        while pos < to {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            let mut end = (pos + CHUNK).min(to);
            if end < to {
                end = src.next_line_start(end).unwrap_or(to).min(to);
            }
            let chunk = &data[pos..end];
            let hay: Cow<[u8]> = if src.encoding == encoding_rs::UTF_8 {
                Cow::Borrowed(chunk)
            } else {
                Cow::Owned(src.encoding.decode_without_bom_handling(chunk).0.into_owned().into_bytes())
            };
            if !f(&hay, line) {
                return false;
            }
            line += memchr::memchr_iter(b'\n', &hay).count() as u64;
            done_bytes += (end - pos) as u64;
            progress.store(done_bytes, Ordering::Relaxed);
            pos = end;
        }
        true
    };
    match edits {
        None => {
            range(src.start, src.data.len(), 0, &mut f);
        }
        Some((pieces, index)) => {
            let mut base = 0u64;
            for p in pieces {
                let ok = match p {
                    Piece::New(t) => !cancel.load(Ordering::Relaxed) && f(t.as_bytes(), base),
                    Piece::Orig { line, count } => {
                        let (a, b) = {
                            let idx = index.read().unwrap();
                            (document::line_start(src, &idx, *line), document::line_start(src, &idx, line + count))
                        };
                        range(a, b, base, &mut f)
                    }
                };
                if !ok {
                    return;
                }
                base += p.lines();
            }
        }
    }
    progress.store(src.data.len() as u64, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, EditKind};
    use crate::document::Document;

    fn run(text: &str, from: &str, to: &str, edit: bool) -> (Vec<Block>, usize) {
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("blk-{}.log", text.len()));
        std::fs::write(&p, text).unwrap();
        let doc = Document::open(&p).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let edits = edit.then(|| {
            let mut b = Buffer::new(&doc);
            let c = (Pos::default(), Pos::default());
            b.replace(&doc, Pos::new(0, 0), Pos::new(0, 0), "x\n", EditKind::Other, c).unwrap();
            (b.snapshot(), doc.index.clone())
        });
        let params = BlockParams { from: from.into(), to: to.into(), regex: false, case_sensitive: true, multiline: false };
        let s = BlockSearch::start(doc.src.clone(), edits, params).unwrap();
        while !s.is_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let v = s.blocks.lock().unwrap().clone();
        (v, s.unclosed.load(Ordering::Relaxed))
    }

    #[test]
    fn multiline_blocks() {
        let t = "a <ini> uno\r\ndos\r\nfin> b <ini>tres</ini>\r\n<ini> sin cierre\r\n";
        let (v, unclosed) = run(t, "<ini>", "</ini>", false);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0], Block { s0: Pos::new(0, 2), s1: Pos::new(0, 7), e0: Pos::new(2, 16), e1: Pos::new(2, 22) });
        assert_eq!(unclosed, 1);
        let (v, _) = run(t, "<ini>", "fin>", false);
        assert_eq!((v[0].s0, v[0].e1), (Pos::new(0, 2), Pos::new(2, 4)));
        // Con ediciones (una línea añadida al principio) todo baja una línea.
        let (v, _) = run(t, "<ini>", "fin>", true);
        assert_eq!((v[0].s0, v[0].e1), (Pos::new(1, 2), Pos::new(3, 4)));
    }

    #[test]
    fn multiline_pattern_crlf_and_edits() {
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("blk-ml.log");
        std::fs::write(&p, "a fin\r\nde linea\r\nfin\r\nde todo\r\n").unwrap();
        let doc = Document::open(&p).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let run = |edits| {
            let params = BlockParams { from: "fin\nde".into(), to: "".into(), regex: false, case_sensitive: true, multiline: true };
            let s = BlockSearch::start(doc.src.clone(), edits, params).unwrap();
            while !s.is_done() {
                std::thread::sleep(Duration::from_millis(2));
            }
            s.blocks.lock().unwrap().iter().map(|b| (b.s0, b.e1)).collect::<Vec<_>>()
        };
        assert_eq!(run(None), vec![(Pos::new(0, 2), Pos::new(1, 2)), (Pos::new(2, 0), Pos::new(3, 2))]);
        // La línea 2 editada (pieza en RAM) sigue encontrándose junto a la 3 (original).
        let mut b = Buffer::new(&doc);
        let c = (Pos::default(), Pos::default());
        b.replace(&doc, Pos::new(2, 0), Pos::new(2, 0), "el ", EditKind::Typing, c).unwrap();
        assert_eq!(run(Some((b.snapshot(), doc.index.clone()))), vec![(Pos::new(0, 2), Pos::new(1, 2)), (Pos::new(2, 3), Pos::new(3, 2))]);
    }

    #[test]
    fn multiline_chunk_overlap() {
        // Trozos diminutos para forzar muchos bordes: no se pierde ni se repite nada.
        let mut t = String::new();
        for i in 0..400 {
            t.push_str(&format!("l{i}\n"));
            if i % 7 == 0 {
                t.push_str("AAA\nBBB\n");
            }
        }
        let dir = std::env::temp_dir().join("buscador-tests");
        let p = dir.join("blk-ov.log");
        std::fs::write(&p, &t).unwrap();
        let doc = Document::open(&p).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let blocks = Mutex::new(Vec::new());
        let tr = AtomicBool::new(false);
        let mk = |pat: &str| RegexBuilder::new(pat).build().unwrap();
        let mut st = State { re_from: mk(r"AAA\nBBB"), re_to: None, open: None, found: Vec::new(), blocks: &blocks, truncated: &tr };
        // chunk/span mínimos: un borde casi en cada línea.
        scan_multiline(&doc.src, None, &AtomicBool::new(false), &AtomicU64::new(0), &mut st, 0, 8);
        let v = blocks.into_inner().unwrap();
        assert_eq!(v.len(), 58);
        let mut expected_line = 1;
        for (k, b) in v.iter().enumerate() {
            assert_eq!(b.s0, Pos::new(expected_line, 0), "bloque {k}");
            assert_eq!(b.e1, Pos::new(expected_line + 1, 3));
            expected_line += 9;
        }
    }

    #[test]
    fn without_end_every_match_is_a_block() {
        let (v, _) = run("ip 10.0.0.1 y 10.0.0.2\nnada\nip 10.0.0.3\n", "10.0.0.", "", false);
        let starts: Vec<Pos> = v.iter().map(|b| b.s0).collect();
        assert_eq!(starts, vec![Pos::new(0, 3), Pos::new(0, 14), Pos::new(2, 3)]);
    }
}
