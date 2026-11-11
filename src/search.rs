//! Búsqueda en segundo plano sobre el archivo mapeado.

use crate::buffer::Piece;
use crate::document::{self, Index, Source};
use regex::bytes::RegexBuilder;
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

/// Tamaño de los bloques que se procesan de una vez.
const CHUNK: usize = 16 << 20;
/// Máximo de líneas coincidentes que se guardan (8 bytes cada una).
pub const MAX_RESULTS: usize = 10_000_000;

#[derive(Clone, PartialEq, Eq)]
pub struct Params {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
}

impl Params {
    fn pattern(&self) -> String {
        if self.regex { self.query.clone() } else { regex::escape(&self.query) }
    }

    /// Regex para resaltar coincidencias en el texto ya decodificado.
    pub fn highlighter(&self) -> Result<regex::Regex, regex::Error> {
        regex::RegexBuilder::new(&self.pattern())
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()
    }
}

pub struct Search {
    pub params: Params,
    pub highlight: regex::Regex,
    /// Números de línea (0-based) con coincidencia, ordenados.
    pub results: Arc<Mutex<Vec<u64>>>,
    pub progress: Arc<AtomicU64>,
    pub total: u64,
    pub done: Arc<AtomicBool>,
    pub truncated: Arc<AtomicBool>,
    pub elapsed: Arc<Mutex<Duration>>,
    cancel: Arc<AtomicBool>,
}

impl Search {
    /// Busca en el archivo original o, si hay `edits` (piezas + índice del
    /// original), en el texto editado.
    pub fn start(src: Source, edits: Option<(Arc<Vec<Piece>>, Arc<RwLock<Index>>)>, params: Params) -> Result<Self, String> {
        let re = RegexBuilder::new(&params.pattern())
            .case_insensitive(!params.case_sensitive)
            .multi_line(true)
            .build()
            .map_err(|e| e.to_string())?;
        let highlight = params.highlighter().map_err(|e| e.to_string())?;

        let s = Search {
            params,
            highlight,
            results: Default::default(),
            progress: Default::default(),
            total: src.data.len() as u64,
            done: Default::default(),
            truncated: Default::default(),
            elapsed: Default::default(),
            cancel: Default::default(),
        };
        let (results, progress, done, truncated, elapsed, cancel) = (
            s.results.clone(),
            s.progress.clone(),
            s.done.clone(),
            s.truncated.clone(),
            s.elapsed.clone(),
            s.cancel.clone(),
        );
        thread::Builder::new()
            .name("search".into())
            .spawn(move || {
                let t0 = Instant::now();
                let mut sc = Scan { src: &src, re: &re, results: &results, progress: &progress, truncated: &truncated, cancel: &cancel, found: Vec::new(), done_bytes: 0 };
                match edits {
                    None => {
                        sc.range(src.start, src.data.len(), 0, u64::MAX);
                    }
                    Some((pieces, index)) => {
                        let mut base = 0u64;
                        for p in pieces.iter() {
                            let ok = match p {
                                Piece::New(t) => {
                                    if re.is_match(t.as_bytes()) {
                                        sc.found.push(base);
                                    }
                                    true
                                }
                                Piece::Orig { line, count } => {
                                    let (a, b) = {
                                        let idx = index.read().unwrap();
                                        (document::line_start(&src, &idx, *line), document::line_start(&src, &idx, line + count))
                                    };
                                    sc.range(a, b, base, base + count)
                                }
                            };
                            if !ok || !sc.flush() {
                                break;
                            }
                            base += p.lines();
                        }
                    }
                }
                sc.flush();
                progress.store(src.data.len() as u64, Ordering::Relaxed);
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
        self.results.lock().unwrap().len()
    }

    pub fn get(&self, i: usize) -> Option<u64> {
        self.results.lock().unwrap().get(i).copied()
    }

    /// Índice de la primera coincidencia con línea > `line` (o >= si `inclusive`).
    pub fn next_after(&self, line: u64, inclusive: bool) -> Option<(usize, u64)> {
        let r = self.results.lock().unwrap();
        let i = r.partition_point(|&l| if inclusive { l < line } else { l <= line });
        r.get(i).map(|&l| (i, l))
    }

    pub fn prev_before(&self, line: u64) -> Option<(usize, u64)> {
        let r = self.results.lock().unwrap();
        let i = r.partition_point(|&l| l < line);
        if i == 0 { None } else { Some((i - 1, r[i - 1])) }
    }

    /// Actualiza los resultados tras una edición que sustituyó `removed`
    /// líneas en `at` por `inserted`; `matches(n)` dice si la línea nueva `n`
    /// coincide.
    pub fn adjust(&self, at: u64, removed: u64, inserted: u64, matches: impl Fn(u64) -> bool) {
        let mut r = self.results.lock().unwrap();
        let i = r.partition_point(|&l| l < at);
        let j = r.partition_point(|&l| l < at + removed);
        let new: Vec<u64> = (at..at + inserted).filter(|&n| matches(n)).collect();
        let k = i + new.len();
        r.splice(i..j, new);
        if inserted != removed {
            for l in &mut r[k..] {
                *l = *l + inserted - removed;
            }
        }
    }

    pub fn index_of(&self, line: u64) -> Option<usize> {
        self.results.lock().unwrap().binary_search(&line).ok()
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Scan<'a> {
    src: &'a Source,
    re: &'a regex::bytes::Regex,
    results: &'a Mutex<Vec<u64>>,
    progress: &'a AtomicU64,
    truncated: &'a AtomicBool,
    cancel: &'a AtomicBool,
    found: Vec<u64>,
    done_bytes: u64,
}

impl Scan<'_> {
    /// Publica lo encontrado; `false` si hay que parar.
    fn flush(&mut self) -> bool {
        let mut r = self.results.lock().unwrap();
        r.append(&mut self.found);
        if r.len() >= MAX_RESULTS {
            r.truncate(MAX_RESULTS);
            self.truncated.store(true, Ordering::Relaxed);
            return false;
        }
        !self.cancel.load(Ordering::Relaxed)
    }

    /// Busca en `data[from..to]` (alineado a inicio de línea); la primera
    /// línea es la `line` del documento y no se aceptan líneas >= `limit`.
    fn range(&mut self, from: usize, to: usize, mut line: u64, limit: u64) -> bool {
        let src = self.src;
        let data = &src.data[..];
        let is_utf8 = src.encoding == encoding_rs::UTF_8;
        let mut pos = from;
        while pos < to {
            if self.cancel.load(Ordering::Relaxed) {
                return false;
            }
            // Bloque alineado a fin de línea.
            let mut end = (pos + CHUNK).min(to);
            if end < to {
                end = src.next_line_start(end).unwrap_or(to).min(to);
            }
            let chunk = &data[pos..end];
            // UTF-8 se busca directamente sobre el mmap; el resto se decodifica
            // sólo el bloque actual (la RAM usada queda acotada por CHUNK).
            let hay: Cow<[u8]> = if is_utf8 {
                Cow::Borrowed(chunk)
            } else {
                let (s, _) = src.encoding.decode_without_bom_handling(chunk);
                Cow::Owned(s.into_owned().into_bytes())
            };
            let hay = &hay[..];

            let mut p = 0;
            let mut counted = 0;
            while p <= hay.len() {
                let Some(m) = self.re.find_at(hay, p) else { break };
                line += memchr::memchr_iter(b'\n', &hay[counted..m.start()]).count() as u64;
                if line < limit {
                    self.found.push(line);
                }
                // Una coincidencia por línea: saltar al inicio de la siguiente.
                match memchr::memchr(b'\n', &hay[m.start()..]) {
                    Some(i) => {
                        p = m.start() + i + 1;
                        line += 1;
                        counted = p;
                    }
                    None => {
                        counted = m.start();
                        p = hay.len() + 1;
                    }
                }
            }
            if counted < hay.len() {
                line += memchr::memchr_iter(b'\n', &hay[counted..]).count() as u64;
            }
            self.done_bytes += (end - pos) as u64;
            self.progress.store(self.done_bytes.min(src.data.len() as u64), Ordering::Relaxed);
            if !self.flush() {
                return false;
            }
            pos = end;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Bytes, Source};
    use crate::encoding::Newline;

    fn run_on(text: &str, enc: &'static encoding_rs::Encoding, q: &str, re: bool, cs: bool) -> Vec<u64> {
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("s-{}-{q}.log", enc.name()).replace(['\\', '*', '.', '[', ']'], "_"));
        let bytes: Vec<u8> = if enc == encoding_rs::UTF_16LE {
            text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
        } else {
            enc.encode(text).0.into_owned()
        };
        std::fs::write(&p, &bytes).unwrap();
        let f = std::fs::File::open(&p).unwrap();
        let m = unsafe { memmap2::Mmap::map(&f).unwrap() };
        let src = Source { data: Arc::new(Bytes::Map(m)), encoding: enc, newline: Newline::for_encoding(enc), start: 0 };
        let s = Search::start(src, None, Params { query: q.into(), regex: re, case_sensitive: cs }).unwrap();
        while !s.is_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        s.results.lock().unwrap().clone()
    }

    #[test]
    fn search_over_edits() {
        use crate::buffer::{Buffer, EditKind, Pos};
        use crate::document::Document;
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s-edits.log");
        let mut t = String::new();
        for i in 0..5000 {
            t.push_str(if i % 1000 == 7 { "ERROR x\n" } else { "ok\n" });
        }
        std::fs::write(&p, &t).unwrap();
        let doc = Document::open(&p).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut b = Buffer::new(&doc);
        let c = (Pos::default(), Pos::default());
        // Inserta 2 líneas al principio (una con ERROR) y borra la línea 1007.
        b.replace(&doc, Pos::new(0, 0), Pos::new(0, 0), "ERROR nuevo\nok\n", EditKind::Other, c).unwrap();
        b.replace(&doc, Pos::new(1008, 0), Pos::new(1010, 0), "", EditKind::Other, c).unwrap();
        let params = Params { query: "error".into(), regex: false, case_sensitive: false };
        let s = Search::start(doc.src.clone(), Some((b.snapshot(), doc.index.clone())), params).unwrap();
        while !s.is_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(*s.results.lock().unwrap(), vec![0, 9, 2007, 3007, 4007]);
        // Ajuste incremental tras otra edición: equivale a buscar de nuevo.
        s.adjust(5, 0, 1, |_| true);
        assert_eq!(*s.results.lock().unwrap(), vec![0, 5, 10, 2008, 3008, 4008]);
    }

    const T: &str = "INFO a\nERROR b error\nwarn c\nerror d\n\nfin ERROR";

    #[test]
    fn plain_case_insensitive_one_per_line() {
        assert_eq!(run_on(T, encoding_rs::UTF_8, "error", false, false), vec![1, 3, 5]);
    }
    #[test]
    fn case_sensitive() {
        assert_eq!(run_on(T, encoding_rs::UTF_8, "error", false, true), vec![1, 3]);
    }
    #[test]
    fn regex_mode() {
        assert_eq!(run_on(T, encoding_rs::UTF_8, r"^(warn|INFO)", true, true), vec![0, 2]);
    }
    #[test]
    fn utf16_and_latin1() {
        let t = "año\nniño\nmañana\nnada";
        assert_eq!(run_on(t, encoding_rs::UTF_16LE, "ñ", false, true), vec![0, 1, 2]);
        assert_eq!(run_on(t, encoding_rs::WINDOWS_1252, "Ñ", false, false), vec![0, 1, 2]);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::document::Document;

    /// BIG_LOG=/ruta/archivo cargo test --release bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn big_file() {
        let path = std::env::var("BIG_LOG").expect("BIG_LOG");
        let t = Instant::now();
        let doc = Document::open(std::path::Path::new(&path)).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let idx_mem = doc.index.read().unwrap().checkpoints.len() * 8;
        println!("indexado: {} líneas en {:.2?} (índice: {} KB)", doc.line_count(), t.elapsed(), idx_mem / 1024);
        let t = Instant::now();
        let v = doc.lines(doc.line_count() - 3, 3);
        println!("últimas líneas en {:.2?}: {:?}", t.elapsed(), doc.src.decode(v[2].1, v[2].2));
        for (q, re, cs) in [("aguja_unica", false, true), ("FATAL|took 99[0-9]ms", true, false), ("ÑANDÚ", false, false)] {
            let s = Search::start(doc.src.clone(), None, Params { query: q.into(), regex: re, case_sensitive: cs }).unwrap();
            while !s.is_done() {
                std::thread::sleep(Duration::from_millis(10));
            }
            println!("buscar {q:?}: {} coincidencias en {:.2?}", s.count(), s.elapsed.lock().unwrap());
            let st = std::fs::read_to_string("/proc/self/status").unwrap();
            println!("   {}", st.lines().filter(|l| l.starts_with("RssAnon") || l.starts_with("RssFile") || l.starts_with("RssShmem")).collect::<Vec<_>>().join(" | "));
        }
    }
}
