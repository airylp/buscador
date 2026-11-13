//! Vista filtrada: muestra solo las líneas que cumplen unas reglas (incluir /
//! excluir texto o regex). Se calcula en segundo plano con la misma búsqueda
//! que «Buscar» (una para las reglas de incluir y otra para las de excluir) y,
//! si el archivo crece («seguir en vivo»), solo se revisa lo nuevo.

use crate::buffer::Piece;
use crate::document::{Index, Source};
use crate::search::{Params, Search};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, PartialEq, Default)]
pub struct Rule {
    pub text: String,
    /// Excluir las líneas que coinciden (si no, incluirlas).
    pub exclude: bool,
    pub regex: bool,
    pub case: bool,
}

impl Rule {
    fn pattern(&self) -> String {
        let p = if self.regex { self.text.clone() } else { regex::escape(&self.text) };
        if self.case { format!("(?:{p})") } else { format!("(?i:{p})") }
    }
}

/// Líneas visibles: una lista, o «todas salvo» una lista (solo reglas de excluir).
pub enum View {
    Lines(Vec<u64>),
    Except(Vec<u64>),
}

impl View {
    pub fn count(&self, total: u64) -> u64 {
        match self {
            View::Lines(v) => v.len() as u64,
            View::Except(v) => total.saturating_sub(v.len() as u64),
        }
    }

    /// Línea del archivo que ocupa la fila `k` de la vista.
    pub fn line_at(&self, k: u64) -> Option<u64> {
        match self {
            View::Lines(v) => v.get(k as usize).copied(),
            // La fila k es la línea k + (nº de excluidas antes): la menor j con ex[j] - j > k.
            View::Except(ex) => {
                let (mut lo, mut hi) = (0usize, ex.len());
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if ex[mid] - mid as u64 <= k {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                Some(k + lo as u64)
            }
        }
    }

    /// Fila de la vista donde está `line` (o la siguiente visible).
    pub fn row_of(&self, line: u64) -> u64 {
        match self {
            View::Lines(v) => v.partition_point(|&l| l < line) as u64,
            View::Except(ex) => line - ex.partition_point(|&l| l < line) as u64,
        }
    }

    fn truncate_from(&mut self, line: u64) {
        let v = match self {
            View::Lines(v) | View::Except(v) => v,
        };
        v.truncate(v.partition_point(|&l| l < line));
    }
}

struct Pending {
    inc: Option<Search>,
    exc: Option<Search>,
    /// Desde qué línea se revisa (0 = todo).
    from_line: u64,
}

pub struct Filter {
    pub rules: Vec<Rule>,
    /// La vista filtrada está a la vista (si no, se ve el archivo completo).
    pub on: bool,
    applied: Vec<Rule>,
    changed_at: Instant,
    pending: Option<Pending>,
    pub view: View,
    /// Ya hay resultado completo para las reglas aplicadas.
    pub ready: bool,
    /// Versión del texto con que se calculó.
    version: u64,
    pub error: Option<String>,
    /// Primera fila visible y línea elegida.
    pub top: u64,
    pub sel: Option<u64>,
    /// La vista estaba abajo del todo (para seguir en vivo).
    pub at_bottom: bool,
    /// Resaltado de las reglas de incluir.
    pub highlight: Option<regex::Regex>,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            rules: vec![Rule::default()],
            on: false,
            applied: Vec::new(),
            changed_at: Instant::now(),
            pending: None,
            view: View::Except(Vec::new()),
            ready: false,
            version: u64::MAX,
            error: None,
            top: 0,
            sel: None,
            at_bottom: false,
            highlight: None,
        }
    }
}

/// Une las reglas de un tipo en una sola regex.
fn params(rules: &[Rule], exclude: bool) -> Option<Params> {
    let parts: Vec<String> = rules.iter().filter(|r| r.exclude == exclude).map(|r| r.pattern()).collect();
    (!parts.is_empty()).then(|| Params { query: parts.join("|"), regex: true, case_sensitive: true })
}

impl Filter {
    /// Reglas con texto (las vacías no cuentan).
    fn effective(&self) -> Vec<Rule> {
        self.rules.iter().filter(|r| !r.text.is_empty()).cloned().collect()
    }

    /// Avisa de que las reglas cambiaron (se aplican tras una pausa al teclear).
    pub fn touch(&mut self) {
        self.changed_at = Instant::now();
    }

    pub fn has_rules(&self) -> bool {
        !self.effective().is_empty()
    }

    /// Progreso (0–1) de lo que se está calculando, si hay algo en curso.
    pub fn progress(&self) -> Option<f32> {
        let p = self.pending.as_ref()?;
        let one = |s: &Option<Search>| {
            s.as_ref().map_or(1.0, |s| {
                if s.is_done() {
                    1.0
                } else {
                    s.progress.load(std::sync::atomic::Ordering::Relaxed) as f32 / s.total.max(1) as f32
                }
            })
        };
        Some(one(&p.inc).min(one(&p.exc)))
    }

    /// Lanza el cálculo si las reglas o el texto cambiaron y recoge el
    /// resultado cuando termina. Devuelve `true` si sigue trabajando.
    pub fn poll(&mut self, src: &Source, edits: impl FnOnce() -> Option<(Arc<Vec<Piece>>, Arc<RwLock<Index>>)>, version: u64) -> bool {
        let eff = self.effective();
        let typing = self.changed_at.elapsed() < Duration::from_millis(350);
        if (eff != self.applied && !typing) || version != self.version {
            self.applied = eff.clone();
            self.version = version;
            self.error = None;
            self.ready = false;
            self.top = 0;
            self.highlight = params(&eff, false).and_then(|p| p.highlighter().ok());
            let edits = edits();
            let start = |p: Option<Params>| -> Result<Option<Search>, String> {
                p.map(|p| Search::start(src.clone(), edits.clone(), p)).transpose()
            };
            match (start(params(&eff, false)), start(params(&eff, true))) {
                (Ok(inc), Ok(exc)) => self.pending = Some(Pending { inc, exc, from_line: 0 }),
                (Err(e), _) | (_, Err(e)) => {
                    self.pending = None;
                    self.view = View::Lines(Vec::new());
                    self.error = Some(e);
                    self.ready = true;
                }
            }
        }
        let Some(p) = &self.pending else { return false };
        let done = |s: &Option<Search>| s.as_ref().is_none_or(|s| s.is_done());
        if !done(&p.inc) || !done(&p.exc) {
            return true;
        }
        let p = self.pending.take().unwrap();
        let take = |s: &Option<Search>| s.as_ref().map(|s| std::mem::take(&mut *s.results.lock().unwrap()));
        let (inc, exc) = (take(&p.inc), take(&p.exc));
        let tail = match (inc, exc) {
            (Some(i), None) => View::Lines(i),
            (Some(i), Some(e)) => View::Lines(i.into_iter().filter(|l| e.binary_search(l).is_err()).collect()),
            (None, Some(e)) => View::Except(e),
            (None, None) => View::Except(Vec::new()),
        };
        if p.from_line == 0 {
            self.view = tail;
        } else {
            // Se añade lo nuevo a lo que ya había.
            self.view.truncate_from(p.from_line);
            match (&mut self.view, tail) {
                (View::Lines(a), View::Lines(b)) | (View::Except(a), View::Except(b)) => a.extend(b),
                (v, t) => *v = t,
            }
        }
        self.ready = true;
        false
    }

    /// El archivo creció: revisar solo desde `from_line` (empieza en `from_byte`).
    pub fn extend(&mut self, src: &Source, from_line: u64, from_byte: usize, version: u64) {
        if self.applied.is_empty() || self.pending.as_ref().is_some_and(|p| p.from_line == 0) {
            // Sin reglas no hay nada que buscar; si aún calculaba todo, se repite.
            self.version = if self.applied.is_empty() { version } else { u64::MAX };
            return;
        }
        let start = |p: Option<Params>| p.and_then(|p| Search::start_from(src.clone(), p, from_line, from_byte).ok());
        let from_line = self.pending.as_ref().map_or(from_line, |p| p.from_line.min(from_line));
        self.pending = Some(Pending { inc: start(params(&self.applied, false)), exc: start(params(&self.applied, true)), from_line });
        self.version = version;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn except_view_maps_rows_to_lines() {
        let v = View::Except(vec![1, 2, 5]);
        // Visibles: 0, 3, 4, 6, 7…
        let lines: Vec<u64> = (0..5).map(|k| v.line_at(k).unwrap()).collect();
        assert_eq!(lines, [0, 3, 4, 6, 7]);
        assert_eq!(v.count(10), 7);
        assert_eq!(v.row_of(6), 3);
    }

    #[test]
    fn include_and_exclude_rules() {
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("filtro.log");
        std::fs::write(&p, "INFO a\nERROR b\nWARN c\nerror d timeout\nINFO e\n").unwrap();
        let doc = crate::document::Document::open(&p).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut f = Filter::default();
        let run = |f: &mut Filter| {
            f.changed_at = Instant::now() - Duration::from_secs(1);
            while f.poll(&doc.src, || None, 0) {
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        f.rules = vec![
            Rule { text: "error".into(), ..Default::default() },
            Rule { text: "warn".into(), ..Default::default() },
            Rule { text: "timeout".into(), exclude: true, ..Default::default() },
        ];
        run(&mut f);
        let rows: Vec<u64> = (0..f.view.count(5)).map(|k| f.view.line_at(k).unwrap()).collect();
        assert_eq!(rows, [1, 2]);
        // Solo excluir.
        f.rules = vec![Rule { text: "INFO".into(), exclude: true, case: true, ..Default::default() }];
        run(&mut f);
        let rows: Vec<u64> = (0..f.view.count(5)).map(|k| f.view.line_at(k).unwrap()).collect();
        assert_eq!(rows, [1, 2, 3]);
    }
}
