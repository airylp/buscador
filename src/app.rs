//! Interfaz gráfica (egui).

use crate::buffer::{self, Buffer, Changed, EditKind, Pos};
use crate::document::Document;
use crate::encoding;
use crate::blocks::{Block, BlockParams, BlockSearch, MAX_BLOCKS};
use crate::search::{MAX_RESULTS, Params, Search};
use crate::settings::Settings;
use eframe::egui::{
    text::{CCursor, LayoutJob},
    *,
};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Bytes máximos de una línea que se pintan (el resto se indica como truncado).
const MAX_DISPLAY_BYTES: usize = 16 * 1024;
/// Bytes máximos de una línea en el panel de detalle.
const MAX_DETAIL_BYTES: usize = 1 << 20;
/// Límite de lo que se copia al portapapeles.
const MAX_COPY_BYTES: usize = 256 << 20;
/// Los archivos mayores se abren en solo lectura (la edición se activa con Ctrl+E).
const EDIT_BY_DEFAULT_MAX: usize = 64 << 20;
/// «Reemplazar todo» guarda en RAM cada línea modificada; por encima de esto se rechaza.
const MAX_REPLACE_LINES: usize = 2_000_000;
/// Tras una edición, se recomprueban como mucho estas líneas para la búsqueda.
const MAX_RECHECK_LINES: u64 = 10_000;
const TAB_WIDTH: usize = 4;
const SCROLLBAR: f32 = 12.0;
const MAC: bool = cfg!(target_os = "macos");

struct SaveJob {
    target: PathBuf,
    progress: Arc<AtomicU64>,
    total: u64,
    result: Arc<Mutex<Option<Result<usize, String>>>>,
}

pub struct Tab {
    doc: Document,
    /// Ediciones sobre el archivo; `None` mientras no se haya modificado nada.
    buf: Option<Buffer>,
    editable: bool,
    top: u64,
    scroll_frac: f32,
    hscroll: f32,
    content_width: f32,
    anchor: Option<Pos>,
    cursor: Option<Pos>,
    /// Columna visual a mantener al subir/bajar.
    goal: Option<usize>,
    /// Hay que desplazar la vista para que se vea el cursor.
    reveal: bool,
    /// Texto en composición del IME (aún no confirmado).
    preedit: String,
    search: Option<Search>,
    search_error: Option<String>,
    /// La búsqueda no refleja las últimas ediciones.
    search_stale: bool,
    /// Línea de la coincidencia actual.
    current: Option<u64>,
    /// Ir a la primera coincidencia en cuanto aparezca.
    pending_first: bool,
    center_on: Option<u64>,
    results_top: u64,
    visible_rows: u64,
    detail_cache: Option<(u64, u64, String)>,
    save: Option<SaveJob>,
    /// Cuenta todas las modificaciones (no se reinicia al guardar).
    edit_gen: u64,
}

impl Tab {
    fn new(doc: Document) -> Self {
        Tab {
            editable: doc.len() <= EDIT_BY_DEFAULT_MAX,
            doc,
            buf: None,
            top: 0,
            scroll_frac: 0.0,
            hscroll: 0.0,
            content_width: 0.0,
            anchor: None,
            cursor: None,
            goal: None,
            reveal: false,
            preedit: String::new(),
            search: None,
            search_error: None,
            search_stale: false,
            current: None,
            pending_first: false,
            center_on: None,
            results_top: 0,
            visible_rows: 1,
            detail_cache: None,
            save: None,
            edit_gen: 0,
        }
    }

    // ---- Acceso al texto (editado o, si no hay cambios, el original) ----

    fn total(&self) -> u64 {
        match &self.buf {
            Some(b) => b.line_count(),
            None => self.doc.line_count(),
        }
    }

    fn version(&self) -> u64 {
        self.buf.as_ref().map_or(0, |b| b.version)
    }

    fn dirty(&self) -> bool {
        self.buf.as_ref().is_some_and(|b| b.is_dirty())
    }

    fn line(&self, n: u64) -> String {
        match &self.buf {
            Some(b) => b.line(&self.doc, n),
            None => self.doc.line_text(n),
        }
    }

    fn line_len(&self, n: u64) -> usize {
        self.line(n).len()
    }

    /// Líneas visibles: (nº, texto recortado a `cap` bytes, recortada).
    fn rows(&self, first: u64, count: usize, cap: usize) -> Vec<(u64, String, bool)> {
        match &self.buf {
            Some(b) => b.rows(&self.doc, first, count, cap),
            None => self
                .doc
                .lines(first, count)
                .into_iter()
                .map(|(n, s, e)| {
                    let (t, tr) = self.doc.src.decode_capped(s, e, cap);
                    (n, t, tr)
                })
                .collect(),
        }
    }

    fn end_pos(&self) -> Pos {
        let last = self.total().saturating_sub(1);
        Pos::new(last, self.line_len(last))
    }

    /// Texto entre dos posiciones; `None` si supera `max` bytes.
    fn text(&self, a: Pos, b: Pos, max: usize) -> Option<String> {
        let mut out = String::new();
        let mut n = a.line;
        while n <= b.line {
            let chunk = self.rows(n, ((b.line - n + 1) as usize).min(4096), usize::MAX);
            if chunk.is_empty() {
                break;
            }
            for (ln, t, _) in chunk {
                let lo = if ln == a.line { buffer::floor_char(&t, a.col) } else { 0 };
                let hi = if ln == b.line { buffer::floor_char(&t, b.col).max(lo) } else { t.len() };
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

    // ---- Cursor y selección ----

    fn selection(&self) -> Option<(Pos, Pos)> {
        let (a, c) = (self.anchor?, self.cursor?);
        if a == c { None } else { Some((a.min(c), a.max(c))) }
    }

    fn cursor_or_top(&self) -> Pos {
        self.cursor.unwrap_or(Pos::new(self.top, 0))
    }

    fn set_cursor(&mut self, p: Pos, extend: bool) {
        if !extend || self.anchor.is_none() {
            self.anchor = Some(p);
        }
        self.cursor = Some(p);
        self.reveal = true;
    }

    fn select(&mut self, a: Pos, b: Pos) {
        self.anchor = Some(a);
        self.cursor = Some(b);
        self.goal = None;
        self.reveal = true;
    }

    fn goto_line(&mut self, line: u64) {
        let p = Pos::new(line, 0);
        self.select(p, p);
        self.center_on = Some(line);
    }

    /// Selecciona la coincidencia `m` (rango de bytes) de la línea `line`.
    fn goto_match_at(&mut self, line: u64, m: (usize, usize)) {
        self.current = Some(line);
        self.select(Pos::new(line, m.0), Pos::new(line, m.1));
        self.center_on = Some(line);
    }

    fn goto_match(&mut self, line: u64) {
        let t = self.line(line);
        let m = self.search.as_ref().and_then(|s| s.highlight.find(&t)).map(|m| (m.start(), m.end()));
        self.goto_match_at(line, m.unwrap_or((0, 0)));
    }

    fn next_match(&mut self) {
        let Some(s) = &self.search else { return };
        // Siguiente coincidencia en la misma línea.
        let mut same_line = None;
        if let Some(c) = self.cursor {
            if s.index_of(c.line).is_some() {
                let t = self.line(c.line);
                let sel_is_match = |m: &regex::Match| m.start() == c.col && self.anchor.is_some_and(|a| a.col == m.end());
                same_line = s
                    .highlight
                    .find_iter(&t)
                    .find(|m| m.start() >= c.col && m.end() > m.start() && !sel_is_match(m))
                    .map(|m| (c.line, (m.start(), m.end())));
            }
        }
        if let Some((l, m)) = same_line {
            self.goto_match_at(l, m);
            return;
        }
        let from = self.current.or(self.cursor.map(|c| c.line)).unwrap_or(self.top);
        let inclusive = self.current.is_none() && self.cursor.is_none();
        let hit = s.next_after(from, inclusive).or_else(|| s.get(0).map(|l| (0, l)));
        if let Some((_, l)) = hit {
            self.goto_match(l);
        }
    }

    /// Última coincidencia de la línea `l` que termina antes de `before`.
    fn last_match_in(&self, l: u64, before: usize) -> Option<(usize, usize)> {
        let s = self.search.as_ref()?;
        let t = self.line(l);
        s.highlight.find_iter(&t).filter(|m| m.end() <= before && m.end() > m.start()).last().map(|m| (m.start(), m.end()))
    }

    fn prev_match(&mut self) {
        let Some(s) = &self.search else { return };
        let start = self.selection().map(|s| s.0).or(self.cursor);
        if let Some(c) = start.filter(|c| s.index_of(c.line).is_some()) {
            if let Some(m) = self.last_match_in(c.line, c.col) {
                self.goto_match_at(c.line, m);
                return;
            }
        }
        let from = self.current.or(self.cursor.map(|c| c.line)).unwrap_or(self.top);
        let n = s.count();
        let hit = s.prev_before(from).or_else(|| n.checked_sub(1).and_then(|i| s.get(i).map(|l| (i, l))));
        if let Some((_, l)) = hit {
            let m = self.last_match_in(l, usize::MAX);
            self.goto_match_at(l, m.unwrap_or((0, 0)));
        }
    }

    // ---- Movimiento ----

    fn char_left(&self, p: Pos) -> Pos {
        if p.col > 0 {
            Pos::new(p.line, prev_boundary(&self.line(p.line), p.col))
        } else if p.line > 0 {
            Pos::new(p.line - 1, self.line_len(p.line - 1))
        } else {
            p
        }
    }

    fn char_right(&self, p: Pos) -> Pos {
        let t = self.line(p.line);
        if p.col < t.len() {
            Pos::new(p.line, next_boundary(&t, p.col))
        } else if p.line + 1 < self.total() {
            Pos::new(p.line + 1, 0)
        } else {
            p
        }
    }

    fn word_left(&self, p: Pos) -> Pos {
        if p.col == 0 { self.char_left(p) } else { Pos::new(p.line, word_left_in(&self.line(p.line), p.col)) }
    }

    fn word_right(&self, p: Pos) -> Pos {
        let t = self.line(p.line);
        if p.col >= t.len() { self.char_right(p) } else { Pos::new(p.line, word_right_in(&t, p.col)) }
    }

    /// Mueve `delta` líneas manteniendo la columna visual.
    fn vertical(&self, p: Pos, delta: i64) -> (Pos, Option<usize>) {
        let last = self.total().saturating_sub(1);
        let goal = self.goal.unwrap_or_else(|| disp_col(&self.line(p.line), p.col));
        let line = add_signed(p.line, delta).min(last);
        if line == p.line {
            // En el borde del documento: ir al inicio/fin.
            return (if delta < 0 { Pos::new(line, 0) } else { Pos::new(line, self.line_len(line)) }, None);
        }
        (Pos::new(line, col_from_disp(&self.line(line), goal)), Some(goal))
    }

    // ---- Edición ----

    fn can_edit(&mut self) -> Result<(), String> {
        if !self.editable {
            return Err("Solo lectura: pulsa «Editar» (Ctrl+E) para modificar".into());
        }
        if self.save.is_some() {
            return Err("Guardando… espera a que termine".into());
        }
        if self.buf.is_none() {
            if !self.doc.indexing_done() {
                return Err("Espera a que termine de indexar para editar".into());
            }
            self.buf = Some(Buffer::new(&self.doc));
        }
        Ok(())
    }

    fn replace_range(&mut self, a: Pos, b: Pos, text: &str, kind: EditKind) -> Result<(), String> {
        self.can_edit()?;
        let before = (self.anchor.unwrap_or(a), self.cursor.unwrap_or(b));
        let buf = self.buf.as_mut().unwrap();
        let (end, ch) = buf.replace(&self.doc, a, b, text, kind, before)?;
        self.select(end, end);
        self.after_change(ch);
        Ok(())
    }

    fn insert(&mut self, text: &str, kind: EditKind) -> Result<(), String> {
        let (a, b) = self.selection().unwrap_or_else(|| {
            let c = self.cursor_or_top();
            (c, c)
        });
        let kind = if a != b || text.contains('\n') { EditKind::Other } else { kind };
        self.replace_range(a, b, text, kind)
    }

    fn delete(&mut self, forward: bool, word: bool) -> Result<(), String> {
        self.can_edit()?;
        if let Some((a, b)) = self.selection() {
            return self.replace_range(a, b, "", EditKind::Other);
        }
        let c = self.cursor_or_top();
        let o = match (forward, word) {
            (false, false) => self.char_left(c),
            (false, true) => self.word_left(c),
            (true, false) => self.char_right(c),
            (true, true) => self.word_right(c),
        };
        if o == c {
            return Ok(());
        }
        self.replace_range(o.min(c), o.max(c), "", EditKind::Deleting)
    }

    fn undo_redo(&mut self, redo: bool) -> Result<(), String> {
        self.can_edit()?;
        let now = (self.anchor.unwrap_or_default(), self.cursor.unwrap_or_default());
        let buf = self.buf.as_mut().unwrap();
        let r = if redo { buf.redo(now) } else { buf.undo(now) };
        let Some(((a, c), ch)) = r else {
            return Err(if redo { "Nada que rehacer" } else { "Nada que deshacer" }.into());
        };
        self.select(a, c);
        self.center_on = Some(c.line);
        self.after_change(ch);
        Ok(())
    }

    /// Mantiene la búsqueda al día tras un cambio sin volver a recorrer el archivo.
    fn after_change(&mut self, ch: Changed) {
        self.detail_cache = None;
        self.edit_gen += 1;
        let Some(s) = &self.search else { return };
        if !s.is_done() || ch.inserted > MAX_RECHECK_LINES {
            self.search_stale = true;
            return;
        }
        let (buf, doc) = (self.buf.as_ref().unwrap(), &self.doc);
        s.adjust(ch.at, ch.removed, ch.inserted, |n| s.highlight.is_match(&buf.line(doc, n)));
        if let Some(c) = self.current {
            if c >= ch.at {
                self.current = if c < ch.at + ch.removed { None } else { Some(c - ch.removed + ch.inserted) };
            }
        }
    }

    fn copy(&self, ctx: &Context) -> String {
        let (a, b) = match self.selection() {
            Some(s) => s,
            None => {
                let Some(c) = self.cursor else { return "Nada seleccionado".into() };
                (Pos::new(c.line, 0), Pos::new(c.line, usize::MAX))
            }
        };
        match self.text(a, b, MAX_COPY_BYTES) {
            Some(t) => {
                let msg = format!("Copiado: {}", describe_span(a, b, t.len()));
                ctx.copy_text(t);
                msg
            }
            None => format!("Selección demasiado grande para copiar (> {})", human_size(MAX_COPY_BYTES as u64)),
        }
    }

    fn cut(&mut self, ctx: &Context) -> Result<String, String> {
        self.can_edit()?;
        let (a, b) = match self.selection() {
            Some(s) => s,
            None => {
                // Sin selección: cortar la línea entera.
                let c = self.cursor_or_top();
                if c.line + 1 < self.total() {
                    (Pos::new(c.line, 0), Pos::new(c.line + 1, 0))
                } else if c.line > 0 {
                    (Pos::new(c.line - 1, self.line_len(c.line - 1)), Pos::new(c.line, self.line_len(c.line)))
                } else {
                    (Pos::new(0, 0), Pos::new(0, self.line_len(0)))
                }
            }
        };
        let t = self.text(a, b, MAX_COPY_BYTES).ok_or("Selección demasiado grande para cortar")?;
        let msg = format!("Cortado: {}", describe_span(a, b, t.len()));
        ctx.copy_text(t);
        self.replace_range(a, b, "", EditKind::Other)?;
        Ok(msg)
    }

    // ---- Reemplazar ----

    fn replace_current(&mut self, repl: &str, regex_mode: bool) -> Result<(), String> {
        let Some(s) = &self.search else { return Err("Busca algo primero".into()) };
        if let Some((a, b)) = self.selection().filter(|(a, b)| a.line == b.line) {
            let t = self.line(a.line);
            let sel = &t[buffer::floor_char(&t, a.col)..buffer::floor_char(&t, b.col)];
            let new = s.highlight.captures(sel).filter(|c| c.get(0).unwrap().as_str().len() == sel.len()).map(|caps| {
                if regex_mode {
                    let mut out = String::new();
                    caps.expand(repl, &mut out);
                    out
                } else {
                    repl.to_string()
                }
            });
            if let Some(new) = new {
                self.replace_range(a, b, &new, EditKind::Other)?;
            }
        }
        self.next_match();
        Ok(())
    }

    fn replace_all(&mut self, repl: &str, regex_mode: bool) -> Result<String, String> {
        let Some(s) = &self.search else { return Err("Busca algo primero".into()) };
        if !s.is_done() {
            return Err("Espera a que termine la búsqueda".into());
        }
        if self.search_stale {
            return Err("La búsqueda está desactualizada: pulsa Enter en «Buscar» para repetirla".into());
        }
        if s.truncated.load(Ordering::Relaxed) || s.count() > MAX_REPLACE_LINES {
            return Err(format!("Demasiadas líneas para reemplazar de una vez (máx. {})", fmt_num(MAX_REPLACE_LINES as u64)));
        }
        let lines = s.results.lock().unwrap().clone();
        let re = s.highlight.clone();
        self.can_edit()?;
        let cur = (self.anchor.unwrap_or_default(), self.cursor.unwrap_or_default());
        let buf = self.buf.as_mut().unwrap();
        let n = buf.replace_lines(&self.doc, &lines, cur, |t| {
            let r = if regex_mode { re.replace_all(t, repl) } else { re.replace_all(t, regex::NoExpand(repl)) };
            match r {
                Cow::Owned(o) if o != t => Some(o),
                _ => None,
            }
        })?;
        self.detail_cache = None;
        self.edit_gen += 1;
        if n > 0 {
            self.search_stale = true;
            self.current = None;
        }
        Ok(format!("Reemplazado en {} líneas (Ctrl+Z para deshacer)", fmt_num(n as u64)))
    }

    // ---- Guardar ----

    fn start_save(&mut self, target: PathBuf) -> Result<(), String> {
        if self.save.is_some() {
            return Err("Ya se está guardando".into());
        }
        let same = target == self.doc.path;
        if self.buf.is_none() {
            if same {
                return Err("No hay cambios que guardar".into());
            }
            if !self.doc.indexing_done() {
                return Err("Espera a que termine de indexar".into());
            }
            self.buf = Some(Buffer::new(&self.doc));
        }
        let buf = self.buf.as_ref().unwrap();
        if same && !buf.is_dirty() {
            return Err("No hay cambios que guardar".into());
        }
        let (pieces, src, index, nl) = (buf.snapshot(), self.doc.src.clone(), self.doc.index.clone(), self.doc.newline_bytes());
        let job = SaveJob {
            target: target.clone(),
            progress: Default::default(),
            total: self.doc.len() as u64,
            result: Default::default(),
        };
        let (progress, result) = (job.progress.clone(), job.result.clone());
        std::thread::Builder::new()
            .name("save".into())
            .spawn(move || {
                let r = buffer::save_file(&pieces, &src, &index, &nl, &target, &progress, &AtomicBool::new(false));
                *result.lock().unwrap() = Some(r);
            })
            .map_err(|e| e.to_string())?;
        self.save = Some(job);
        Ok(())
    }

    /// Si terminó un guardado, reabre el archivo nuevo (libera la RAM de las
    /// ediciones; el historial de deshacer se reinicia).
    fn poll_save(&mut self) -> Option<String> {
        let job = self.save.as_ref()?;
        let r = job.result.lock().unwrap().take()?;
        let target = job.target.clone();
        self.save = None;
        Some(match r {
            Ok(lossy) => {
                let enc = self.doc.src.encoding;
                match Document::open(&target) {
                    Ok(mut doc) => {
                        if doc.src.encoding != enc {
                            doc.set_encoding(enc);
                        }
                        self.doc = doc;
                        self.buf = None;
                        self.detail_cache = None;
                        let mut m = format!("Guardado {} ({})", self.doc.name, human_size(self.doc.len() as u64));
                        if lossy > 0 {
                            m += &format!(" · {lossy} caracteres no representables se guardaron como «?»");
                        }
                        m
                    }
                    Err(e) => format!("Guardado, pero no se pudo reabrir: {e}"),
                }
            }
            Err(e) => format!("Error al guardar: {e}"),
        })
    }

    // ---- Búsqueda avanzada (bloques) ----

    fn goto_block(&mut self, b: Block) {
        self.current = None;
        self.select(b.s0, b.e1);
        self.center_on = Some(b.s0.line);
    }

    /// Texto de los bloques según el modo de copia: (texto, nº de bloques).
    fn blocks_text(&self, list: &[Block], mode: CopyMode, sep: Sep) -> Result<(String, usize), String> {
        if list.is_empty() {
            return Err("No hay bloque seleccionado".into());
        }
        let sep = match sep {
            Sep::Newline => "\n",
            Sep::Blank => "\n\n",
            Sep::Nothing => "",
        };
        let mut out = String::new();
        let mut n = 0;
        // En «líneas completas», no repetir líneas ya copiadas.
        let mut last_line: Option<u64> = None;
        for b in list {
            let (mut a, z) = block_range(b, mode);
            if mode == CopyMode::Lines {
                if let Some(l) = last_line {
                    if z.line <= l {
                        continue;
                    }
                    if a.line <= l {
                        a = Pos::new(l + 1, 0);
                    }
                }
                last_line = Some(z.line);
            }
            if n > 0 {
                out.push_str(sep);
            }
            let left = MAX_COPY_BYTES.saturating_sub(out.len());
            let t = self
                .text(a, z, left)
                .ok_or_else(|| format!("Demasiado texto para el portapapeles (> {})", human_size(MAX_COPY_BYTES as u64)))?;
            out.push_str(&t);
            n += 1;
        }
        Ok((out, n))
    }

    // ---- Teclado ----

    fn handle_key(&mut self, key: Key, m: Modifiers, page: i64) -> Result<(), String> {
        let word = if MAC { m.alt } else { m.ctrl };
        let shift = m.shift;
        if self.total() == 0 {
            return Ok(());
        }
        let c = self.cursor_or_top();
        let sel = self.selection();
        let mut goal = None;
        let target = match key {
            Key::ArrowLeft if MAC && m.command => Some(Pos::new(c.line, 0)),
            Key::ArrowRight if MAC && m.command => Some(Pos::new(c.line, self.line_len(c.line))),
            Key::ArrowUp if MAC && m.command => Some(Pos::default()),
            Key::ArrowDown if MAC && m.command => Some(self.end_pos()),
            Key::ArrowLeft => Some(match sel {
                Some((a, _)) if !shift => a,
                _ if word => self.word_left(c),
                _ => self.char_left(c),
            }),
            Key::ArrowRight => Some(match sel {
                Some((_, b)) if !shift => b,
                _ if word => self.word_right(c),
                _ => self.char_right(c),
            }),
            Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown => {
                let d = match key {
                    Key::ArrowUp => -1,
                    Key::ArrowDown => 1,
                    Key::PageUp => -page,
                    _ => page,
                };
                if matches!(key, Key::PageUp | Key::PageDown) {
                    self.top = add_signed(self.top, d);
                }
                let (p, g) = self.vertical(c, d);
                goal = g;
                Some(p)
            }
            Key::Home if m.command => Some(Pos::default()),
            Key::End if m.command => Some(self.end_pos()),
            Key::Home => {
                // Inicio «inteligente»: primero tras la sangría, luego columna 0.
                let t = self.line(c.line);
                let indent = t.len() - t.trim_start().len();
                Some(Pos::new(c.line, if c.col == indent { 0 } else { indent }))
            }
            Key::End => Some(Pos::new(c.line, self.line_len(c.line))),
            Key::A if m.command => {
                self.select(Pos::default(), self.end_pos());
                self.reveal = false;
                return Ok(());
            }
            Key::Z if m.command => return self.undo_redo(shift),
            Key::Y if m.command && !MAC => return self.undo_redo(true),
            Key::Enter if !m.command => return self.insert("\n", EditKind::Other),
            Key::Tab if !m.command && !m.ctrl => return self.insert("\t", EditKind::Typing),
            Key::Backspace => return self.delete(false, word),
            Key::Delete => return self.delete(true, word),
            Key::Escape => {
                self.anchor = self.cursor;
                return Ok(());
            }
            _ => None,
        };
        if let Some(t) = target {
            self.set_cursor(t, shift);
            self.goal = goal;
        }
        Ok(())
    }

    /// Procesa los eventos de teclado/IME/portapapeles con el foco en la vista.
    fn handle_events(&mut self, ctx: &Context, page: i64) -> Option<String> {
        let events = ctx.input(|i| i.events.clone());
        let mut msg = None;
        for ev in events {
            let r: Result<(), String> = match ev {
                Event::Text(t) => {
                    self.preedit.clear();
                    self.insert(&t, EditKind::Typing)
                }
                Event::Ime(ImeEvent::Preedit { text, .. }) => {
                    self.preedit = text;
                    Ok(())
                }
                Event::Ime(ImeEvent::Commit(t)) => {
                    self.preedit.clear();
                    if t.is_empty() || t == "\n" || t == "\r" { Ok(()) } else { self.insert(&t, EditKind::Typing) }
                }
                Event::Paste(t) => self.insert(&t, EditKind::Other),
                Event::Copy => {
                    msg = Some(self.copy(ctx));
                    Ok(())
                }
                Event::Cut => self.cut(ctx).map(|m| msg = Some(m)),
                Event::Key { key, pressed: true, modifiers, .. } => self.handle_key(key, modifiers, page),
                _ => Ok(()),
            };
            if let Err(e) = r {
                msg = Some(e);
            }
        }
        msg
    }
}

/// Todos los atajos, con los nombres de tecla de la plataforma.
fn shortcut_list() -> Vec<(&'static str, Vec<(String, &'static str)>)> {
    let c = if MAC { "Cmd" } else { "Ctrl" };
    let alt = if MAC { "Opción" } else { "Alt" };
    let word = if MAC { "Opción" } else { "Ctrl" };
    let k = |s: &str| s.replace("C+", &format!("{c}+")).replace("A+", &format!("{alt}+")).replace("W+", &format!("{word}+"));
    let mut edit = vec![
        (k("C+Z"), "Deshacer"),
        (k("C+Shift+Z"), "Rehacer"),
        (k("C+C"), "Copiar (sin selección: la línea entera)"),
        (k("C+X"), "Cortar (sin selección: la línea entera)"),
        (k("C+V"), "Pegar"),
        (k("C+A"), "Seleccionar todo"),
        (k("Retroceso / Supr"), "Borrar carácter"),
        (k("W+Retroceso / W+Supr"), "Borrar palabra"),
        (k("Enter"), "Nueva línea"),
        (k("Tab"), "Insertar tabulador"),
    ];
    if !MAC {
        edit.insert(2, (k("C+Y"), "Rehacer"));
    }
    let (line_start, line_end, doc_start, doc_end) = if MAC {
        ("Cmd+← / Inicio", "Cmd+→ / Fin", "Cmd+↑ / Cmd+Inicio", "Cmd+↓ / Cmd+Fin")
    } else {
        ("Inicio", "Fin", "Ctrl+Inicio", "Ctrl+Fin")
    };
    vec![
        (
            "Archivos",
            vec![
                (k("C+O"), "Abrir (también arrastrando archivos)"),
                (k("C+S"), "Guardar"),
                (k("C+Shift+S"), "Guardar como…"),
                (k("C+W"), "Cerrar archivo"),
                ("Ctrl+Tab".into(), "Siguiente archivo"),
                (k("C+B"), "Mostrar/ocultar panel de archivos"),
                (k("C+E"), "Activar/desactivar edición"),
                ("Clic derecho en un archivo".into(), "Abrir en ventana nueva, mostrar en carpeta, copiar ruta, duplicar…"),
            ],
        ),
        (
            "Buscar",
            vec![
                (k("C+F"), "Ir a la caja de búsqueda"),
                ("Enter / F3".into(), "Buscar / siguiente coincidencia"),
                ("Shift+Enter / Shift+F3".into(), "Coincidencia anterior"),
                (if MAC { "Cmd+Opción+F".into() } else { "Ctrl+H".to_string() }, "Reemplazar"),
                (k("C+Shift+F"), "Nueva ventana de búsqueda avanzada"),
                (k("C+Enter"), "Buscar (búsqueda avanzada multilínea)"),
                (k("C+G"), "Ir a línea"),
            ],
        ),
        ("Edición", edit),
        (
            "Moverse y seleccionar",
            vec![
                ("← → ↑ ↓".into(), "Carácter / línea"),
                (k("W+← / W+→"), "Palabra"),
                (line_start.into(), "Inicio de línea (tras la sangría; otra vez: columna 1)"),
                (line_end.into(), "Fin de línea"),
                (doc_start.into(), "Inicio del archivo"),
                (doc_end.into(), "Fin del archivo"),
                ("RePág / AvPág".into(), "Página arriba / abajo"),
                ("Shift + movimiento".into(), "Seleccionar"),
                ("Esc".into(), "Quitar la selección"),
            ],
        ),
        (
            "Ratón",
            vec![
                ("Clic / arrastrar".into(), "Colocar el cursor / seleccionar"),
                ("Shift+clic".into(), "Extender la selección"),
                ("Doble clic".into(), "Seleccionar palabra"),
                ("Clic en nº de línea".into(), "Seleccionar la línea entera"),
                ("Clic derecho".into(), "Copiar, cortar, pegar, ver línea completa…"),
                ("Clic en el minimapa".into(), "Ir al bloque más cercano (búsqueda avanzada)"),
            ],
        ),
        (
            "Vista",
            vec![
                (k("C++ / C+-"), "Letra más grande / más pequeña"),
                ("F1".into(), "Esta lista de atajos"),
            ],
        ),
    ]
}

#[derive(Clone, Copy)]
enum FileAction {
    NewWindow(usize),
    Reveal(usize),
    CopyPath(usize),
    CopyName(usize),
    Duplicate(usize),
    Close(usize),
    CloseOthers(usize),
}

const REVEAL_LABEL: &str = if MAC {
    "Mostrar en Finder"
} else if cfg!(windows) {
    "Mostrar en el Explorador"
} else {
    "Abrir carpeta contenedora"
};

/// Abre el gestor de archivos con el archivo seleccionado (o su carpeta en Linux).
fn reveal_in_file_manager(path: &std::path::Path) -> std::io::Result<()> {
    use std::process::Command;
    let mut c = if MAC {
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(std::path::Path::new(".")));
        c
    };
    c.spawn().map(|_| ())
}

/// «informe.log» → «informe copia.log», «informe copia 2.log»… (el primero libre).
fn copy_name(path: &std::path::Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{stem} copia{ext}") } else { format!("{stem} copia {n}{ext}") }))
        .find(|p| !p.exists())
        .unwrap()
}

#[derive(Clone, Copy)]
enum Confirm {
    CloseTab(usize),
    Quit,
}

pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    show_sidebar: bool,
    show_results: bool,
    show_detail: bool,
    show_replace: bool,
    query: String,
    replacement: String,
    use_regex: bool,
    case_sensitive: bool,
    goto: String,
    font_size: f32,
    theme: ThemePreference,
    show_settings: bool,
    show_keys: bool,
    /// El editor tenía el foco en el último fotograma.
    view_focused: bool,
    /// Ventanas de búsqueda avanzada abiertas.
    advs: Vec<AdvWin>,
    next_adv: u64,
    message: Option<(String, Instant)>,
    focus_search: bool,
    focus_goto: bool,
    focus_view: bool,
    confirm: Option<Confirm>,
    /// Duplicados en curso: (destino, resultado).
    duplicating: Vec<(PathBuf, Arc<Mutex<Option<Result<(), String>>>>)>,
    allow_quit: bool,
    debug_frames: u64,
    debug_typed: bool,
    #[cfg(target_os = "macos")]
    menu: crate::macmenu::MacMenu,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        install_fallback_fonts(&cc.egui_ctx);
        let settings = Settings::load();
        cc.egui_ctx.set_theme(settings.theme);
        let mut app = App {
            tabs: Vec::new(),
            active: 0,
            show_sidebar: true,
            show_results: true,
            show_detail: false,
            show_replace: false,
            query: String::new(),
            replacement: String::new(),
            use_regex: false,
            case_sensitive: false,
            goto: String::new(),
            font_size: settings.font_size,
            theme: settings.theme,
            show_settings: false,
            show_keys: false,
            view_focused: false,
            advs: Vec::new(),
            next_adv: 1,
            message: None,
            focus_search: false,
            focus_goto: false,
            focus_view: false,
            confirm: None,
            duplicating: Vec::new(),
            allow_quit: false,
            debug_frames: 0,
            debug_typed: false,
            #[cfg(target_os = "macos")]
            menu: crate::macmenu::MacMenu::new(&cc.egui_ctx),
        };
        for f in files {
            app.open(f);
        }
        app
    }

    fn open(&mut self, path: PathBuf) {
        if let Some(i) = self.tabs.iter().position(|t| t.doc.path == path) {
            self.active = i;
            return;
        }
        match Document::open(&path) {
            Ok(doc) => {
                self.tabs.push(Tab::new(doc));
                self.active = self.tabs.len() - 1;
                self.focus_view = true;
            }
            Err(e) => self.msg(format!("No se pudo abrir {}: {e}", path.display())),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(files) = rfd::FileDialog::new().set_title("Abrir archivos").pick_files() {
            for f in files {
                self.open(f);
            }
        }
    }

    fn set_font(&mut self, f: f32) {
        self.font_size = f.clamp(7.0, 40.0);
        self.persist();
    }

    fn persist(&mut self) {
        if let Err(e) = (Settings { theme: self.theme, font_size: self.font_size }).save() {
            self.msg(format!("No se pudieron guardar los ajustes: {e}"));
        }
    }

    fn msg(&mut self, m: impl Into<String>) {
        self.message = Some((m.into(), Instant::now()));
    }

    fn tab(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    fn params(&self) -> Params {
        Params { query: self.query.clone(), regex: self.use_regex, case_sensitive: self.case_sensitive }
    }

    /// Enter en la caja de búsqueda: busca, o salta a la siguiente si la búsqueda no cambió.
    fn search_or_next(&mut self) {
        let params = self.params();
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if params.query.is_empty() {
            tab.search = None;
            tab.current = None;
            return;
        }
        if !tab.search_stale && tab.search.as_ref().is_some_and(|s| s.params == params) {
            tab.next_match();
            return;
        }
        let edits = tab.buf.as_ref().map(|b| (b.snapshot(), tab.doc.index.clone()));
        match Search::start(tab.doc.src.clone(), edits, params) {
            Ok(s) => {
                tab.search = Some(s);
                tab.search_error = None;
                tab.search_stale = false;
                tab.current = None;
                tab.results_top = 0;
                tab.pending_first = true;
                self.show_results = true;
            }
            Err(e) => {
                tab.search = None;
                tab.search_error = Some(e);
            }
        }
    }

    fn close_tab(&mut self, i: usize, force: bool) {
        if i >= self.tabs.len() {
            return;
        }
        if !force && (self.tabs[i].dirty() || self.tabs[i].save.is_some()) {
            self.active = i;
            self.confirm = Some(Confirm::CloseTab(i));
            return;
        }
        self.tabs.remove(i);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }

    fn toggle_edit(&mut self) {
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        t.editable = !t.editable;
        let m = if t.editable {
            if t.doc.len() > EDIT_BY_DEFAULT_MAX {
                format!(
                    "Edición activada. Las líneas que cambies se guardan en RAM; al guardar se escribe una copia completa ({}) y se renombra",
                    human_size(t.doc.len() as u64)
                )
            } else {
                "Edición activada".to_string()
            }
        } else {
            "Solo lectura".to_string()
        };
        self.msg(m);
        self.focus_view = true;
    }

    fn save(&mut self, save_as: bool) {
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        let target = if save_as {
            let mut d = rfd::FileDialog::new().set_title("Guardar como").set_file_name(&t.doc.name);
            if let Some(dir) = t.doc.path.parent() {
                d = d.set_directory(dir);
            }
            match d.save_file() {
                Some(p) => p,
                None => return,
            }
        } else {
            t.doc.path.clone()
        };
        if let Err(e) = t.start_save(target) {
            self.msg(e);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        let sc = |m, k| KeyboardShortcut::new(m, k);
        let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
        // Las combinaciones con más modificadores van primero (consume_shortcut
        // ignora modificadores de más).
        if pressed(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::S)) {
            self.save(true);
        } else if pressed(sc(Modifiers::COMMAND, Key::S)) {
            self.save(false);
        }
        let replace_key = if MAC { sc(Modifiers::COMMAND | Modifiers::ALT, Key::F) } else { sc(Modifiers::COMMAND, Key::H) };
        if pressed(replace_key) {
            self.show_replace = true;
            self.focus_search = true;
        }
        if pressed(sc(Modifiers::COMMAND, Key::Q)) {
            // Pasa por la confirmación de cambios sin guardar.
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if pressed(sc(Modifiers::NONE, Key::F1)) {
            self.show_keys = !self.show_keys;
        }
        if pressed(sc(Modifiers::COMMAND, Key::O)) {
            self.open_dialog();
        }
        if pressed(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::F)) {
            self.new_adv_window();
        }
        if pressed(sc(Modifiers::COMMAND, Key::F)) {
            self.focus_search = true;
        }
        if pressed(sc(Modifiers::COMMAND, Key::G)) {
            self.focus_goto = true;
        }
        if pressed(sc(Modifiers::COMMAND, Key::B)) {
            self.show_sidebar = !self.show_sidebar;
        }
        if pressed(sc(Modifiers::COMMAND, Key::E)) {
            self.toggle_edit();
        }
        if pressed(sc(Modifiers::COMMAND, Key::W)) {
            self.close_tab(self.active, false);
        }
        if pressed(sc(Modifiers::COMMAND, Key::Plus)) || pressed(sc(Modifiers::COMMAND, Key::Equals)) {
            self.set_font(self.font_size + 1.0);
        }
        if pressed(sc(Modifiers::COMMAND, Key::Minus)) {
            self.set_font(self.font_size - 1.0);
        }
        if pressed(sc(Modifiers::CTRL, Key::Tab)) && !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
        if pressed(sc(Modifiers::SHIFT, Key::F3)) {
            if let Some(t) = self.tab() {
                t.prev_match();
            }
        } else if pressed(sc(Modifiers::NONE, Key::F3)) {
            if let Some(t) = self.tab() {
                t.next_match();
            }
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(self.show_sidebar, "☰ Archivos")
                .on_hover_text("Mostrar/ocultar panel (Ctrl+B)")
                .clicked()
            {
                self.show_sidebar = !self.show_sidebar;
            }
            if ui.button("Abrir…").on_hover_text("Ctrl+O · también puedes arrastrar archivos").clicked() {
                self.open_dialog();
            }
            let has_tab = !self.tabs.is_empty();
            let (editable, dirty, can_undo, can_redo) = self.tabs.get(self.active).map_or((false, false, false, false), |t| {
                (
                    t.editable,
                    t.dirty(),
                    t.buf.as_ref().is_some_and(|b| b.can_undo()),
                    t.buf.as_ref().is_some_and(|b| b.can_redo()),
                )
            });
            if ui
                .add_enabled(has_tab, Button::selectable(editable, "Editar"))
                .on_hover_text("Activar/desactivar edición (Ctrl+E)")
                .clicked()
            {
                self.toggle_edit();
            }
            if ui.add_enabled(dirty, Button::new("Guardar")).on_hover_text("Ctrl+S · Guardar como: Ctrl+Shift+S").clicked() {
                self.save(false);
            }
            if ui.add_enabled(can_undo && editable, Button::new("↶")).on_hover_text("Deshacer (Ctrl+Z)").clicked() {
                if let Some(Err(e)) = self.tab().map(|t| t.undo_redo(false)) {
                    self.msg(e);
                }
            }
            if ui.add_enabled(can_redo && editable, Button::new("↷")).on_hover_text("Rehacer (Ctrl+Shift+Z)").clicked() {
                if let Some(Err(e)) = self.tab().map(|t| t.undo_redo(true)) {
                    self.msg(e);
                }
            }
            ui.separator();

            ui.label("Buscar:");
            let r = ui.add(
                TextEdit::singleline(&mut self.query)
                    .desired_width(260.0)
                    .hint_text("texto o regex  (Ctrl+F, Enter, F3)")
                    .font(TextStyle::Monospace),
            );
            if self.focus_search {
                r.request_focus();
                self.focus_search = false;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                if ui.input(|i| i.modifiers.shift) {
                    if let Some(t) = self.tab() {
                        t.prev_match();
                    }
                } else {
                    self.search_or_next();
                }
                r.request_focus();
            }
            ui.toggle_value(&mut self.case_sensitive, "Aa").on_hover_text("Distinguir mayúsculas");
            ui.toggle_value(&mut self.use_regex, ".*").on_hover_text("Expresión regular");
            if ui.button("Buscar").clicked() {
                self.search_or_next();
            }
            if ui.add_enabled(has_tab, Button::new("◀")).on_hover_text("Anterior (Shift+F3)").clicked() {
                if let Some(t) = self.tab() {
                    t.prev_match();
                }
            }
            if ui.add_enabled(has_tab, Button::new("▶")).on_hover_text("Siguiente (F3)").clicked() {
                if let Some(t) = self.tab() {
                    t.next_match();
                }
            }
            let rk = if MAC { "Cmd+Alt+F" } else { "Ctrl+H" };
            ui.toggle_value(&mut self.show_replace, "Reemplazar…").on_hover_text(rk);
            let n_adv = self.advs.len();
            let adv_label = if n_adv > 0 { format!("Avanzada… ({n_adv})") } else { "Avanzada…".to_string() };
            if ui
                .selectable_label(n_adv > 0, adv_label)
                .on_hover_text("Nueva ventana de búsqueda por bloques (desde… hasta…, multilínea), con minimapa y copia en varias líneas (Ctrl+Shift+F). Puedes abrir varias.")
                .clicked()
            {
                self.new_adv_window();
            }
            if let Some(t) = self.tabs.get(self.active) {
                if let Some(e) = &t.search_error {
                    ui.colored_label(ui.visuals().error_fg_color, format!("Regex inválida: {e}"));
                } else if let Some(s) = &t.search {
                    let n = s.count();
                    let pos = t.current.and_then(|l| s.index_of(l)).map(|i| format!("{}/", i + 1)).unwrap_or_default();
                    let mut txt = format!("{pos}{} líneas", fmt_num(n as u64));
                    if s.truncated.load(Ordering::Relaxed) {
                        txt += &format!(" (límite {})", fmt_num(MAX_RESULTS as u64));
                    }
                    if !s.is_done() {
                        let p = s.progress.load(Ordering::Relaxed) as f64 / s.total.max(1) as f64;
                        txt += &format!(" · buscando {:.0}%", p * 100.0);
                    } else {
                        txt += &format!(" · {:.2}s", s.elapsed.lock().unwrap().as_secs_f32());
                    }
                    ui.label(txt);
                    if t.search_stale {
                        ui.colored_label(ui.visuals().warn_fg_color, "desactualizada (Enter para repetir)");
                    }
                }
            }
            ui.separator();

            ui.label("Ir a línea:");
            let r = ui.add(TextEdit::singleline(&mut self.goto).desired_width(90.0).hint_text("Ctrl+G"));
            if self.focus_goto {
                r.request_focus();
                self.focus_goto = false;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let n: Option<u64> = self.goto.trim().replace(['.', ',', '_'], "").parse().ok();
                if let (Some(n), Some(t)) = (n, self.tabs.get_mut(self.active)) {
                    let last = t.total().saturating_sub(1);
                    t.goto_line(n.saturating_sub(1).min(last));
                    self.focus_view = true;
                }
            }
            ui.separator();

            let mut msg = None;
            if let Some(t) = self.tabs.get_mut(self.active) {
                let mut enc = t.doc.src.encoding;
                ComboBox::from_id_salt("encoding")
                    .selected_text(encoding::label(enc))
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for e in encoding::available() {
                            ui.selectable_value(&mut enc, e, encoding::label(e));
                        }
                    })
                    .response
                    .on_hover_text("Encoding con el que se lee el archivo");
                if enc != t.doc.src.encoding {
                    if t.dirty() || t.save.is_some() {
                        msg = Some("Guarda o deshaz los cambios antes de reabrir con otro encoding");
                    } else {
                        t.buf = None;
                        t.doc.set_encoding(enc);
                        t.search = None;
                        t.current = None;
                        t.detail_cache = None;
                        t.content_width = 0.0;
                    }
                }
            }
            if let Some(m) = msg {
                self.msg(m);
            }
            ui.separator();
            ui.toggle_value(&mut self.show_results, "Resultados");
            ui.toggle_value(&mut self.show_detail, "Detalle").on_hover_text("Línea completa, seleccionable");
            if ui.selectable_label(self.show_settings, "Ajustes").on_hover_text("Tema y tamaño de letra").clicked() {
                self.show_settings = !self.show_settings;
            }
            if ui.small_button("A-").clicked() {
                self.set_font(self.font_size - 1.0);
            }
            if ui.small_button("A+").clicked() {
                self.set_font(self.font_size + 1.0);
            }
        });
        if self.show_replace {
            ui.horizontal(|ui| {
                ui.label("Reemplazar por:");
                let r = ui.add(
                    TextEdit::singleline(&mut self.replacement)
                        .desired_width(260.0)
                        .hint_text(if self.use_regex { "texto ($1, $2… para grupos)" } else { "texto" })
                        .font(TextStyle::Monospace),
                );
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let (repl, re) = (self.replacement.clone(), self.use_regex);
                if ui.button("Reemplazar").on_hover_text("Reemplaza la coincidencia seleccionada y va a la siguiente").clicked() || enter
                {
                    if let Some(Err(e)) = self.tab().map(|t| t.replace_current(&repl, re)) {
                        self.msg(e);
                    }
                    if enter {
                        r.request_focus();
                    }
                }
                if ui.button("Reemplazar todo").on_hover_text("En todas las líneas encontradas · se deshace con Ctrl+Z").clicked() {
                    if let Some(r) = self.tab().map(|t| t.replace_all(&repl, re)) {
                        self.msg(r.unwrap_or_else(|e| e));
                    }
                }
                if ui.small_button("×").clicked() {
                    self.show_replace = false;
                }
            });
        }
    }

    /// Opciones elegidas en el menú de macOS y estado de sus marcas.
    #[cfg(target_os = "macos")]
    fn mac_menu(&mut self, ctx: &Context) {
        use crate::macmenu::{Cmd, inject};
        for c in self.menu.poll() {
            match c {
                Cmd::Settings => self.show_settings = true,
                Cmd::ToggleResults => self.show_results = !self.show_results,
                Cmd::ToggleDetail => self.show_detail = !self.show_detail,
                c => inject(ctx, c),
            }
        }
        let t = self.tabs.get(self.active);
        self.menu.sync([
            t.is_some_and(|t| t.dirty()),
            t.is_some_and(|t| t.editable),
            self.show_sidebar,
            self.show_results,
            self.show_detail,
        ]);
    }

    fn file_action(&mut self, ctx: &Context, a: FileAction) {
        match a {
            FileAction::Reveal(i) => {
                if let Err(e) = reveal_in_file_manager(&self.tabs[i].doc.path) {
                    self.msg(format!("No se pudo abrir la carpeta: {e}"));
                }
            }
            FileAction::CopyPath(i) => {
                let p = self.tabs[i].doc.path.display().to_string();
                ctx.copy_text(p.clone());
                self.msg(format!("Ruta copiada: {p}"));
            }
            FileAction::CopyName(i) => {
                let n = self.tabs[i].doc.name.clone();
                ctx.copy_text(n.clone());
                self.msg(format!("Nombre copiado: {n}"));
            }
            FileAction::Duplicate(i) => self.duplicate(i),
            FileAction::NewWindow(i) => {
                let t = &self.tabs[i];
                let r = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg(&t.doc.path).spawn());
                let m = match r {
                    Ok(_) if t.dirty() => format!("{} abierto en otra ventana (versión guardada en disco)", t.doc.name),
                    Ok(_) => format!("{} abierto en otra ventana", t.doc.name),
                    Err(e) => format!("No se pudo abrir otra ventana: {e}"),
                };
                self.msg(m);
            }
            FileAction::Close(i) => self.close_tab(i, false),
            FileAction::CloseOthers(i) => {
                // Del final al principio para no desplazar índices; las que
                // tienen cambios piden confirmación y se quedan.
                for j in (0..self.tabs.len()).rev() {
                    if j != i && !self.tabs[j].dirty() && self.tabs[j].save.is_none() {
                        self.tabs.remove(j);
                    }
                }
                self.active = self.tabs.iter().position(|t| t.doc.path == self.tabs[i.min(self.tabs.len() - 1)].doc.path).unwrap_or(0);
                if self.tabs.len() > 1 {
                    self.msg("Las pestañas con cambios sin guardar no se cerraron");
                }
            }
        }
    }

    /// Copia el archivo junto al original («nombre copia.ext») en segundo
    /// plano y lo abre al terminar. Con cambios sin guardar, la copia los incluye.
    fn duplicate(&mut self, i: usize) {
        let t = &self.tabs[i];
        let target = copy_name(&t.doc.path);
        let result: Arc<Mutex<Option<Result<(), String>>>> = Default::default();
        let r = result.clone();
        let spawned = if let Some(buf) = t.buf.as_ref().filter(|b| b.is_dirty()) {
            let (pieces, src, index, nl) = (buf.snapshot(), t.doc.src.clone(), t.doc.index.clone(), t.doc.newline_bytes());
            let target = target.clone();
            std::thread::Builder::new().name("duplicar".into()).spawn(move || {
                let res = buffer::save_file(&pieces, &src, &index, &nl, &target, &AtomicU64::new(0), &AtomicBool::new(false));
                *r.lock().unwrap() = Some(res.map(|_| ()));
            })
        } else {
            let (from, target) = (t.doc.path.clone(), target.clone());
            // En APFS/btrfs `fs::copy` clona el archivo: instantáneo aunque pese GB.
            std::thread::Builder::new().name("duplicar".into()).spawn(move || {
                *r.lock().unwrap() = Some(std::fs::copy(&from, &target).map(|_| ()).map_err(|e| e.to_string()));
            })
        };
        match spawned {
            Ok(_) => {
                self.msg(format!("Duplicando como {}…", target.file_name().unwrap_or_default().to_string_lossy()));
                self.duplicating.push((target, result));
            }
            Err(e) => self.msg(format!("No se pudo duplicar: {e}")),
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong("Archivos abiertos");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("«").on_hover_text("Ocultar (Ctrl+B)").clicked() {
                    self.show_sidebar = false;
                }
            });
        });
        ui.separator();
        let mut action = None;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, t) in self.tabs.iter().enumerate() {
                let row = vec2(ui.available_width(), ui.spacing().interact_size.y);
                ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                    // El botón de cerrar, pegado a la derecha; el nombre ocupa el resto.
                    if ui.small_button("×").on_hover_text("Cerrar (Ctrl+W)").clicked() {
                        action = Some(FileAction::Close(i));
                    }
                    let name = if t.dirty() { format!("● {}", t.doc.name) } else { t.doc.name.clone() };
                    let r = ui
                        .with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.style_mut().wrap_mode = Some(TextWrapMode::Truncate);
                            ui.selectable_label(i == self.active, RichText::new(name).strong())
                        })
                        .inner
                        .on_hover_text(format!("{}\nClic derecho: más opciones", t.doc.path.display()));
                    if r.clicked() {
                        self.active = i;
                    }
                    r.context_menu(|ui| {
                        let mut chosen = None;
                        let nw = ui.button("Abrir en una ventana nueva");
                        let nw = if t.dirty() { nw.on_hover_text("La ventana nueva muestra la versión guardada en disco") } else { nw };
                        if nw.clicked() {
                            chosen = Some(FileAction::NewWindow(i));
                        }
                        ui.separator();
                        if ui.button(REVEAL_LABEL).clicked() {
                            chosen = Some(FileAction::Reveal(i));
                        }
                        if ui.button("Copiar ruta").clicked() {
                            chosen = Some(FileAction::CopyPath(i));
                        }
                        if ui.button("Copiar nombre").clicked() {
                            chosen = Some(FileAction::CopyName(i));
                        }
                        ui.separator();
                        let dup = ui.button("Duplicar");
                        let dup = if t.dirty() { dup.on_hover_text("La copia incluye los cambios sin guardar; el original no se modifica") } else { dup };
                        if dup.clicked() {
                            chosen = Some(FileAction::Duplicate(i));
                        }
                        ui.separator();
                        if ui.button("Cerrar").clicked() {
                            chosen = Some(FileAction::Close(i));
                        }
                        if ui.add_enabled(self.tabs.len() > 1, Button::new("Cerrar los demás")).clicked() {
                            chosen = Some(FileAction::CloseOthers(i));
                        }
                        if chosen.is_some() {
                            action = chosen;
                            ui.close();
                        }
                    });
                });
                let idx = if t.doc.indexing_done() {
                    format!("{} líneas", fmt_num(t.total()))
                } else {
                    let p = t.doc.progress.load(Ordering::Relaxed) as f64 / t.doc.len().max(1) as f64;
                    format!("indexando {:.0}%", p * 100.0)
                };
                ui.label(
                    RichText::new(format!(
                        "   {} · {} · {}{}",
                        human_size(t.doc.len() as u64),
                        t.doc.src.encoding.name(),
                        idx,
                        if t.editable { "" } else { " · solo lectura" }
                    ))
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);
            }
            if self.tabs.is_empty() {
                ui.weak("Sin archivos.\nUsa «Abrir…» o arrastra archivos aquí.");
            }
        });
        if let Some(a) = action {
            self.file_action(ui.ctx(), a);
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let full = ui.max_rect();
            if let Some(t) = self.tabs.get(self.active) {
                let d = &t.doc;
                ui.label(RichText::new(if t.editable { "EDICIÓN" } else { "SOLO LECTURA" }).small().strong());
                ui.separator();
                ui.label(RichText::new(d.path.display().to_string()).small());
                ui.separator();
                ui.label(RichText::new(human_size(d.len() as u64)).small());
                ui.separator();
                ui.label(RichText::new(format!("{} ({})", encoding::label(d.src.encoding), d.detect_reason)).small());
                ui.separator();
                let lines = fmt_num(t.total());
                if d.indexing_done() {
                    ui.label(RichText::new(format!("{lines} líneas")).small());
                } else {
                    let p = d.progress.load(Ordering::Relaxed) as f32 / d.len().max(1) as f32;
                    ui.add(ProgressBar::new(p).desired_width(120.0).text(format!("indexando… {lines} líneas")));
                }
                if let Some(c) = t.cursor {
                    ui.separator();
                    let line = t.rows(c.line, 1, c.col.max(1)).into_iter().next().map(|r| r.1).unwrap_or_default();
                    let col = line[..buffer::floor_char(&line, c.col)].chars().count() + 1;
                    let mut s = format!("Lín {}, Col {}", fmt_num(c.line + 1), fmt_num(col as u64));
                    if let Some((a, b)) = t.selection() {
                        s += &if a.line == b.line {
                            format!(" · {} seleccionados", fmt_num((b.col - a.col) as u64))
                        } else {
                            format!(" · {} líneas seleccionadas", fmt_num(b.line - a.line + 1))
                        };
                    }
                    ui.label(RichText::new(s).small());
                }
                if let Some(b) = &t.buf {
                    if b.is_dirty() {
                        ui.separator();
                        let (pieces, bytes) = b.memory();
                        ui.label(RichText::new("● sin guardar").small().color(ui.visuals().warn_fg_color)).on_hover_text(format!(
                            "{} piezas · {} de texto editado en RAM",
                            fmt_num(pieces as u64),
                            human_size(bytes as u64)
                        ));
                    }
                }
                if let Some(job) = &t.save {
                    ui.separator();
                    let p = job.progress.load(Ordering::Relaxed) as f32 / job.total.max(1) as f32;
                    ui.add(ProgressBar::new(p.min(1.0)).desired_width(140.0).text("guardando…"));
                }
            }
            if let Some((m, t)) = &self.message {
                if t.elapsed() < Duration::from_secs(6) {
                    ui.separator();
                    ui.label(RichText::new(m).small().color(ui.visuals().warn_fg_color));
                }
            }
            // Botón de atajos, pegado a la esquina inferior derecha (al final,
            // para no desplazar lo demás).
            let r = Rect::from_min_size(pos2(full.right() - 22.0, full.center().y - 9.0), vec2(20.0, 18.0));
            if ui
                .put(r, Button::new(RichText::new("?").small().strong()).small().selected(self.show_keys))
                .on_hover_text("Atajos de teclado (F1)")
                .clicked()
            {
                self.show_keys = !self.show_keys;
            }
        });
    }

    fn confirm_dialog(&mut self, ctx: &Context) {
        let Some(c) = self.confirm else { return };
        let names: Vec<String> = match c {
            Confirm::CloseTab(i) => self.tabs.get(i).map(|t| t.doc.name.clone()).into_iter().collect(),
            Confirm::Quit => self.tabs.iter().filter(|t| t.dirty() || t.save.is_some()).map(|t| t.doc.name.clone()).collect(),
        };
        let mut choice = None;
        Modal::new(Id::NULL.with("confirm")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading("Cambios sin guardar");
            ui.add_space(6.0);
            ui.label(format!("Hay cambios sin guardar en: {}", names.join(", ")));
            ui.label("Si continúas se perderán. Para conservarlos, cancela y pulsa Ctrl+S.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Descartar cambios").clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancelar").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                self.confirm = None;
                match c {
                    Confirm::CloseTab(i) => self.close_tab(i, true),
                    Confirm::Quit => {
                        self.allow_quit = true;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }
}

impl App {
    /// Depuración: BUSCADOR_SCREENSHOT=ruta.rgba [BUSCADOR_QUERY=texto]
    /// [BUSCADOR_TYPE=texto a teclear en la línea 2] guarda una captura cruda
    /// (ancho, alto u32 LE + RGBA) cuando todo está listo y sale.
    fn debug_screenshot(&mut self, ctx: &Context, busy: bool) {
        let Ok(path) = std::env::var("BUSCADOR_SCREENSHOT") else { return };
        self.debug_frames += 1;
        if self.debug_frames == 2 {
            if let Ok(t) = std::env::var("BUSCADOR_THEME") {
                ctx.set_theme(if t == "oscuro" { ThemePreference::Dark } else { ThemePreference::Light });
            }
            self.show_settings = std::env::var("BUSCADOR_SETTINGS").is_ok();
            self.show_keys = std::env::var("BUSCADOR_KEYS").is_ok();
            if let Ok(a) = std::env::var("BUSCADOR_ADV") {
                // «desde|hasta»; «\n» en el texto = salto de línea (activa multilínea).
                let a = a.replace("\\n", "\n");
                let (from, to) = a.split_once('|').unwrap_or((&a, ""));
                self.new_adv_window();
                let w = self.advs.last_mut().unwrap();
                w.from = from.into();
                w.to = to.into();
                w.multiline = a.contains('\n');
                w.sep = Sep::Blank;
                w.detached = std::env::var("BUSCADOR_ADV_APARTE").is_ok();
                if let Some(t) = self.tabs.get(self.active) {
                    w.start(t);
                }
            }
            if let Ok(q) = std::env::var("BUSCADOR_QUERY") {
                self.query = q;
                self.show_detail = true;
                self.search_or_next();
            }
        }
        let indexed = self.tabs.get(self.active).is_some_and(|t| t.doc.indexing_done());
        if self.debug_frames >= 30 && self.debug_frames < 1000 && !self.debug_typed && indexed {
            self.debug_typed = true;
            if let (Ok(txt), Some(t)) = (std::env::var("BUSCADOR_TYPE"), self.tabs.get_mut(self.active)) {
                t.editable = true;
                let p = Pos::new(1, 0);
                t.select(p, p);
                if let Err(e) = t.insert(&txt, EditKind::Typing) {
                    self.msg(e);
                }
                // Selección de ejemplo en la línea siguiente.
                if let Some(t) = self.tabs.get_mut(self.active) {
                    let l = t.cursor.unwrap().line + 1;
                    t.select(Pos::new(l, 2), Pos::new(l, 9));
                }
            }
        }
        if self.debug_frames == 60 {
            if let (Some(w), Some(t)) = (self.advs.last_mut(), self.tabs.get_mut(self.active)) {
                if let Some(b) = w.get(3) {
                    w.cur = Some(3);
                    w.scroll_to = Some(3);
                    t.goto_block(b);
                }
            }
        }
        if self.debug_frames < 70 || busy || (std::env::var("BUSCADOR_TYPE").is_ok() && !self.debug_typed) {
            ctx.request_repaint();
            return;
        }
        if self.debug_frames < 1000 {
            self.debug_frames = 1000;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(ScreenshotCallback::new(move |img| {
                let mut out = Vec::with_capacity(8 + img.pixels.len() * 4);
                out.extend_from_slice(&(img.size[0] as u32).to_le_bytes());
                out.extend_from_slice(&(img.size[1] as u32).to_le_bytes());
                out.extend_from_slice(img.as_raw());
                std::fs::write(&path, out).unwrap();
                std::process::exit(0);
            })));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Cerrar la ventana con cambios sin guardar pide confirmación.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_quit {
            if self.tabs.iter().any(|t| t.dirty() || t.save.is_some()) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.confirm = Some(Confirm::Quit);
            }
        }

        #[cfg(target_os = "macos")]
        self.mac_menu(&ctx);

        // Guardados terminados.
        let done: Vec<String> = self.tabs.iter_mut().filter_map(|t| t.poll_save()).collect();
        for m in done {
            self.msg(m);
        }

        // Duplicados terminados: abrirlos.
        let finished: Vec<(PathBuf, Result<(), String>)> = {
            let mut out = Vec::new();
            self.duplicating.retain(|(p, r)| match r.lock().unwrap().take() {
                Some(res) => {
                    out.push((p.clone(), res));
                    false
                }
                None => true,
            });
            out
        };
        for (p, res) in finished {
            match res {
                Ok(()) => {
                    self.open(p.clone());
                    self.msg(format!("Duplicado: {}", p.display()));
                }
                Err(e) => self.msg(format!("No se pudo duplicar: {e}")),
            }
        }

        // Archivos arrastrados a la ventana.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for p in dropped {
            self.open(p);
        }
        if self.confirm.is_none() {
            self.handle_shortcuts(&ctx);
            self.escape_closes_popup(&ctx);
        }

        Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui);
            ui.add_space(1.0);
        });
        if self.show_sidebar {
            Panel::left("sidebar").resizable(true).default_size(240.0).min_size(140.0).show(ui, |ui| self.sidebar(ui));
        }
        Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        let font = FontId::monospace(self.font_size);
        let has_search = self.tabs.get(self.active).is_some_and(|t| t.search.is_some());
        if self.show_results && has_search {
            Panel::bottom("results").resizable(true).default_size(180.0).min_size(60.0).show(ui, |ui| {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    results_view(ui, t, &font);
                }
            });
        }
        if self.show_detail && !self.tabs.is_empty() {
            Panel::bottom("detail").resizable(true).default_size(90.0).min_size(40.0).show(ui, |ui| {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    detail_view(ui, t, &font);
                }
            });
        }

        let mut message = None;
        let mut view_focused = false;
        let show_detail = &mut self.show_detail;
        let focus_view = std::mem::take(&mut self.focus_view);
        CentralPanel::default().frame(Frame::NONE).show(ui, |ui| {
            if let Some(t) = self.tabs.get_mut(self.active) {
                (message, view_focused) = log_view(ui, t, &font, show_detail, focus_view);
            } else {
                ui.centered_and_justified(|ui| {
                    ui.heading("Arrastra archivos aquí, o pulsa Ctrl+O");
                });
            }
        });
        self.view_focused = view_focused;
        if let Some(m) = message {
            self.msg(m);
        }
        self.settings_window(&ctx);
        self.keys_window(&ctx);
        self.adv_windows(&ctx);
        self.confirm_dialog(&ctx);

        // Saltar a la primera coincidencia en cuanto aparezca.
        if let Some(t) = self.tabs.get_mut(self.active) {
            if t.pending_first {
                if let Some(s) = &t.search {
                    let from = t.cursor.map(|c| c.line).unwrap_or(t.top);
                    if let Some((_, l)) = s.next_after(from, true) {
                        t.pending_first = false;
                        t.goto_match(l);
                    } else if s.is_done() {
                        t.pending_first = false;
                        t.next_match();
                    }
                }
            }
        }

        // Mientras haya trabajo en segundo plano, refrescar periódicamente.
        let busy = self
            .tabs
            .iter()
            .any(|t| {
                !self.duplicating.is_empty()
                    || !t.doc.indexing_done()
                    || t.save.is_some()
                    || t.search.as_ref().is_some_and(|s| !s.is_done())
            })
            || self.advs.iter().any(|w| w.is_busy());
        if busy {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        self.debug_screenshot(&ctx, busy);
    }
}

/// Vista virtual del texto: sólo se leen y pintan las líneas visibles.
/// Devuelve un mensaje para la barra de estado y si el editor tiene el foco.
fn log_view(ui: &mut Ui, tab: &mut Tab, font: &FontId, show_detail: &mut bool, focus: bool) -> (Option<String>, bool) {
    let mut message = None;
    let ctx = ui.ctx().clone();
    let (row_h, char_w) = ctx.fonts_mut(|f| (f.row_height(font).ceil(), f.glyph_width(font, '0')));
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    let text_area = Rect::from_min_max(full.min, pos2(full.max.x - SCROLLBAR, full.max.y - SCROLLBAR));
    let vsb = Rect::from_min_max(pos2(text_area.max.x, full.min.y), pos2(full.max.x, text_area.max.y));
    let hsb = Rect::from_min_max(pos2(full.min.x, text_area.max.y), pos2(text_area.max.x, full.max.y));

    let id = ui.scope_id().with("logview");
    let resp = ui.interact(text_area, id, Sense::click_and_drag());
    if focus {
        resp.request_focus();
    }
    let visible = ((text_area.height() / row_h).floor() as u64).max(1);
    tab.visible_rows = visible;

    // Teclado (antes de medir: puede cambiar el número de líneas).
    if resp.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
        });
        message = tab.handle_events(&ctx, visible.saturating_sub(1).max(1) as i64);
    }

    let total = tab.total();
    let max_top = total.saturating_sub(visible);
    let digits = fmt_num(total.max(1)).chars().count().max(4);
    let gutter_w = char_w * digits as f32 + 14.0;
    let text_x = text_area.left() + gutter_w + 6.0;
    let view_w = (text_area.right() - text_x).max(char_w * 4.0);

    if let Some(l) = tab.center_on.take() {
        if l < tab.top || l >= tab.top + visible {
            tab.top = l.saturating_sub(visible / 2);
        }
    }
    if tab.reveal {
        if let Some(c) = tab.cursor {
            if c.line < tab.top {
                tab.top = c.line;
            } else if c.line >= tab.top + visible {
                tab.top = c.line + 1 - visible;
            }
        }
    }

    // Rueda del ratón.
    if resp.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta);
        tab.scroll_frac -= d.y / row_h;
        let whole = tab.scroll_frac.trunc();
        tab.scroll_frac -= whole;
        tab.top = add_signed(tab.top, whole as i64);
        tab.hscroll = (tab.hscroll - d.x).max(0.0);
    }

    // Ratón: cursor por carácter; en el margen de números, líneas completas.
    let row_at = |top: u64, y: f32| -> u64 {
        let r = ((y - text_area.top()) / row_h).floor().max(0.0) as u64;
        (top + r).min(total.saturating_sub(1))
    };
    let pos_at = |tab: &Tab, top: u64, p: Pos2| -> Pos {
        let line = row_at(top, p.y);
        let (t, _) = cap_line(tab.line(line));
        let g = ui.painter().layout_job(plain_job(&t, font));
        let idx = g.cursor_from_pos(vec2(p.x - text_x + tab.hscroll, row_h / 2.0)).index.0;
        Pos::new(line, col_from_disp(&t, idx))
    };
    if total > 0 {
        let shift = ui.input(|i| i.modifiers.shift);
        let press = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()) } else { None };
        if let Some(p) = press.or_else(|| (resp.clicked() || resp.secondary_clicked()).then(|| resp.interact_pointer_pos()).flatten())
        {
            resp.request_focus();
            tab.goal = None;
            tab.preedit.clear();
            if p.x < text_area.left() + gutter_w {
                // Clic en el número de línea: seleccionar la línea entera.
                let l = row_at(tab.top, p.y);
                let end = if l + 1 < total { Pos::new(l + 1, 0) } else { Pos::new(l, tab.line_len(l)) };
                if shift && tab.anchor.is_some() {
                    let a = tab.anchor.unwrap();
                    tab.cursor = Some(if Pos::new(l, 0) < a { Pos::new(l, 0) } else { end });
                } else {
                    tab.select(Pos::new(l, 0), end);
                }
            } else {
                let pos = pos_at(tab, tab.top, p);
                let in_sel = tab.selection().is_some_and(|(a, b)| a <= pos && pos <= b);
                if shift && tab.anchor.is_some() {
                    tab.cursor = Some(pos);
                } else if !(resp.secondary_clicked() && in_sel) {
                    tab.select(pos, pos);
                }
            }
        }
        if resp.double_clicked() {
            if let Some(p) = resp.interact_pointer_pos().filter(|p| p.x >= text_area.left() + gutter_w) {
                let pos = pos_at(tab, tab.top, p);
                let t = tab.line(pos.line);
                let (a, b) = word_at(&t, pos.col);
                tab.select(Pos::new(pos.line, a), Pos::new(pos.line, b));
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                // Auto-desplazamiento al arrastrar fuera del área.
                if p.y < text_area.top() {
                    tab.top = tab.top.saturating_sub(1 + ((text_area.top() - p.y) / row_h) as u64);
                } else if p.y > text_area.bottom() {
                    tab.top += 1 + ((p.y - text_area.bottom()) / row_h) as u64;
                }
                if p.x > text_area.right() {
                    tab.hscroll += char_w * 2.0;
                } else if p.x < text_x && tab.hscroll > 0.0 && p.x > text_area.left() + gutter_w {
                    tab.hscroll = (tab.hscroll - char_w * 2.0).max(0.0);
                }
                tab.top = tab.top.min(max_top);
                let clamped = pos2(p.x.max(text_x), p.y.clamp(text_area.top(), text_area.bottom() - 1.0));
                if press.is_none() {
                    tab.cursor = Some(pos_at(tab, tab.top, clamped));
                }
                ctx.request_repaint();
            }
        }
    }
    resp.context_menu(|ui| {
        let has_sel = tab.selection().is_some();
        if ui.button("Copiar (Ctrl+C)").clicked() {
            message = Some(tab.copy(&ctx));
            ui.close();
        }
        if ui.add_enabled(tab.editable, Button::new("Cortar (Ctrl+X)")).clicked() {
            message = Some(tab.cut(&ctx).unwrap_or_else(|e| e));
            ui.close();
        }
        if ui.add_enabled(tab.editable, Button::new("Pegar (Ctrl+V)")).clicked() {
            resp.request_focus();
            ctx.send_viewport_cmd(ViewportCommand::RequestPaste);
            ui.close();
        }
        if ui.add_enabled(tab.editable && has_sel, Button::new("Borrar selección")).clicked() {
            if let Err(e) = tab.delete(false, false) {
                message = Some(e);
            }
            ui.close();
        }
        ui.separator();
        if ui.button("Seleccionar todo (Ctrl+A)").clicked() {
            tab.select(Pos::default(), tab.end_pos());
            ui.close();
        }
        if ui.button("Ver línea completa").clicked() {
            *show_detail = true;
            ui.close();
        }
    });

    tab.top = tab.top.min(max_top);

    // Maquetar las líneas visibles.
    let visuals = ui.visuals().clone();
    let highlight = tab.search.as_ref().map(|s| &s.highlight);
    let rows: Vec<(u64, String, Arc<Galley>)> = tab
        .rows(tab.top, visible as usize + 1, MAX_DISPLAY_BYTES)
        .into_iter()
        .map(|(n, t, tr)| {
            let g = ui.painter().layout_job(line_job(&t, tr, font, visuals.text_color(), highlight));
            (n, t, g)
        })
        .collect();
    let x_of = |t: &str, g: &Galley, col: usize| g.pos_from_cursor(CCursor::new(disp_col(t, col))).min.x;

    // Desplazamiento horizontal para que se vea el cursor.
    if std::mem::take(&mut tab.reveal) {
        if let Some(c) = tab.cursor {
            if let Some((_, t, g)) = rows.iter().find(|r| r.0 == c.line) {
                let x = x_of(t, g, c.col);
                if x < tab.hscroll {
                    tab.hscroll = (x - char_w * 8.0).max(0.0);
                } else if x > tab.hscroll + view_w - char_w * 2.0 {
                    tab.hscroll = x - view_w + char_w * 8.0;
                }
            }
        }
    }

    // Pintado.
    let painter = ui.painter_at(text_area);
    painter.rect_filled(text_area, 0.0, visuals.extreme_bg_color);
    painter.rect_filled(
        Rect::from_min_max(text_area.min, pos2(text_area.left() + gutter_w, text_area.bottom())),
        0.0,
        visuals.faint_bg_color,
    );
    let text_painter = painter.with_clip_rect(Rect::from_min_max(pos2(text_x - 2.0, text_area.top()), text_area.max));
    let sel = tab.selection();
    let sel_color = visuals.selection.bg_fill.gamma_multiply(0.6);
    let cur_line_color = visuals.faint_bg_color.gamma_multiply(1.5);
    let match_line_color = Color32::from_rgba_unmultiplied(255, 180, 0, 40);
    let mark_color = Color32::from_rgb(230, 160, 0);
    let ox = text_x - tab.hscroll;
    let mut max_w: f32 = 0.0;
    let mut caret_rect = None;

    for (n, t, galley) in &rows {
        let n = *n;
        let y = text_area.top() + (n - tab.top) as f32 * row_h;
        let row = Rect::from_min_size(pos2(text_area.left(), y), vec2(text_area.width(), row_h));
        if tab.current == Some(n) {
            painter.rect_filled(row, 0.0, match_line_color);
        } else if tab.cursor.is_some_and(|c| c.line == n) && sel.is_none() {
            painter.rect_filled(row, 0.0, cur_line_color);
        }
        if tab.search.as_ref().is_some_and(|s| s.index_of(n).is_some()) {
            painter.rect_filled(Rect::from_min_size(row.min, vec2(3.0, row_h)), 0.0, mark_color);
        }
        // Selección.
        if let Some((a, b)) = sel.filter(|(a, b)| a.line <= n && n <= b.line) {
            let x0 = if n == a.line { x_of(t, galley, a.col) } else { 0.0 };
            let mut x1 = if n == b.line { x_of(t, galley, b.col) } else { galley.size().x };
            if n < b.line {
                x1 += char_w * 0.6; // el salto de línea también está seleccionado
            }
            text_painter.rect_filled(
                Rect::from_min_max(pos2(ox + x0, y), pos2(ox + x1.max(x0 + 1.0), y + row_h)),
                0.0,
                sel_color,
            );
        }
        painter.text(
            pos2(text_area.left() + gutter_w - 6.0, y),
            Align2::RIGHT_TOP,
            fmt_num(n + 1),
            font.clone(),
            visuals.weak_text_color(),
        );
        max_w = max_w.max(galley.size().x);
        text_painter.galley(pos2(ox, y), galley.clone(), visuals.text_color());

        // Cursor (y texto en composición del IME).
        if let Some(c) = tab.cursor.filter(|c| c.line == n) {
            let x = ox + x_of(t, galley, c.col);
            let mut caret_x = x;
            if !tab.preedit.is_empty() {
                let g = ui.painter().layout_job(plain_job(&tab.preedit, font));
                let r = Rect::from_min_size(pos2(x, y), vec2(g.size().x, row_h));
                text_painter.rect_filled(r, 0.0, visuals.extreme_bg_color);
                text_painter.galley(pos2(x, y), g, visuals.strong_text_color());
                text_painter.hline(r.x_range(), y + row_h - 1.0, Stroke::new(1.0, visuals.strong_text_color()));
                caret_x = r.right();
            }
            let focused = resp.has_focus();
            let color = if tab.editable { visuals.strong_text_color() } else { visuals.weak_text_color() };
            if focused || !tab.editable {
                let w = if tab.editable && focused { 2.0 } else { 1.0 };
                text_painter.rect_filled(Rect::from_min_size(pos2(caret_x - w / 2.0, y), vec2(w, row_h)), 0.0, color);
            }
            caret_rect = Some(Rect::from_min_size(pos2(caret_x, y), vec2(1.0, row_h)));
        }
    }
    tab.content_width = tab.content_width.max(max_w);

    // Permitir el IME (acentos, CJK…) cuando se puede escribir.
    if resp.has_focus() && tab.editable {
        if let Some(cr) = caret_rect {
            let to_global = ctx.layer_transform_to_global(ui.layer_id()).unwrap_or_default();
            ctx.output_mut(|o| {
                o.ime = Some(output::IMEOutput {
                    purpose: IMEPurpose::Normal,
                    rect: to_global * text_area,
                    cursor_rect: to_global * cr,
                    should_interrupt_composition: false,
                })
            });
        }
    }

    // Barras de desplazamiento.
    let mut top = tab.top;
    vscrollbar(ui, vsb, id.with("vsb"), &mut top, total, visible);
    tab.top = top;
    hscrollbar(ui, hsb, id.with("hsb"), &mut tab.hscroll, tab.content_width + 40.0, view_w);
    let note = if !tab.doc.indexing_done() {
        Some("indexando…")
    } else if !tab.editable {
        Some("solo lectura · Ctrl+E para editar")
    } else {
        None
    };
    if let Some(note) = note {
        painter.text(
            text_area.right_bottom() - vec2(6.0, 4.0),
            Align2::RIGHT_BOTTOM,
            note,
            FontId::proportional(11.0),
            visuals.weak_text_color(),
        );
    }
    (message, resp.has_focus())
}

/// Recorta una línea muy larga para maquetarla (igual que al pintarla).
fn cap_line(t: String) -> (String, bool) {
    if t.len() <= MAX_DISPLAY_BYTES {
        return (t, false);
    }
    let cut = buffer::floor_char(&t, MAX_DISPLAY_BYTES);
    (t[..cut].to_string(), true)
}

fn plain_job(txt: &str, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    job.append(&sanitize(txt), 0.0, TextFormat { font_id: font.clone(), ..Default::default() });
    job
}

/// Construye el texto de una línea con las coincidencias resaltadas.
fn line_job(txt: &str, truncated: bool, font: &FontId, color: Color32, hl: Option<&regex::Regex>) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let normal = TextFormat { font_id: font.clone(), color, ..Default::default() };
    let marked = TextFormat {
        background: Color32::from_rgb(255, 200, 0),
        color: Color32::BLACK,
        ..normal.clone()
    };
    let mut last = 0;
    if let Some(re) = hl {
        for m in re.find_iter(txt).take(500) {
            if m.start() == m.end() {
                continue;
            }
            job.append(&sanitize(&txt[last..m.start()]), 0.0, normal.clone());
            job.append(&sanitize(m.as_str()), 0.0, marked.clone());
            last = m.end();
        }
    }
    job.append(&sanitize(&txt[last..]), 0.0, normal.clone());
    if truncated {
        job.append("  …[línea truncada; ver Detalle]", 0.0, TextFormat { italics: true, ..normal });
    }
    job
}

/// Tabs a espacios y caracteres de control visibles (una sola fila por línea).
/// Cada carácter produce exactamente uno, salvo el tab (`TAB_WIDTH`): ver `disp_col`.
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\t' => out.push_str(&" ".repeat(TAB_WIDTH)),
            c if c.is_control() => out.push('·'),
            c => out.push(c),
        }
    }
    out
}

fn char_disp_width(c: char) -> usize {
    if c == '\t' { TAB_WIDTH } else { 1 }
}

/// Índice de carácter mostrado correspondiente al byte `col` de `s`.
fn disp_col(s: &str, col: usize) -> usize {
    s[..buffer::floor_char(s, col)].chars().map(char_disp_width).sum()
}

/// Byte de `s` correspondiente al carácter mostrado `d` (redondeando dentro de un tab).
fn col_from_disp(s: &str, d: usize) -> usize {
    let mut acc = 0;
    for (i, c) in s.char_indices() {
        if acc >= d {
            return i;
        }
        let w = char_disp_width(c);
        if acc + w > d {
            return if d - acc <= w / 2 { i } else { i + c.len_utf8() };
        }
        acc += w;
    }
    s.len()
}

fn prev_boundary(s: &str, i: usize) -> usize {
    s[..buffer::floor_char(s, i)].char_indices().next_back().map_or(0, |(j, _)| j)
}

fn next_boundary(s: &str, i: usize) -> usize {
    let i = buffer::floor_char(s, i);
    s[i..].chars().next().map_or(i, |c| i + c.len_utf8())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_left_in(s: &str, i: usize) -> usize {
    let mut it = s[..buffer::floor_char(s, i)].char_indices().rev().peekable();
    let mut j = i;
    while let Some(&(k, c)) = it.peek() {
        if !c.is_whitespace() {
            break;
        }
        j = k;
        it.next();
    }
    if let Some(&(_, c0)) = it.peek() {
        let w = is_word(c0);
        while let Some(&(k, c)) = it.peek() {
            if c.is_whitespace() || is_word(c) != w {
                break;
            }
            j = k;
            it.next();
        }
    }
    j
}

fn word_right_in(s: &str, i: usize) -> usize {
    let i = buffer::floor_char(s, i);
    let mut it = s[i..].char_indices().peekable();
    let mut j = s.len();
    if let Some(&(_, c0)) = it.peek() {
        if !c0.is_whitespace() {
            let w = is_word(c0);
            while let Some(&(_, c)) = it.peek() {
                if c.is_whitespace() || is_word(c) != w {
                    break;
                }
                it.next();
            }
        }
    }
    while let Some(&(_, c)) = it.peek() {
        if !c.is_whitespace() {
            break;
        }
        it.next();
    }
    if let Some(&(k, _)) = it.peek() {
        j = i + k;
    }
    j
}

/// Palabra (o bloque de espacios/símbolos) bajo `col`.
fn word_at(s: &str, col: usize) -> (usize, usize) {
    let col = buffer::floor_char(s, col);
    let Some(c) = s[col..].chars().next().or_else(|| s[..col].chars().next_back()) else { return (col, col) };
    let class = |x: char| if is_word(x) { 0 } else if x.is_whitespace() { 1 } else { 2 };
    let k = class(c);
    let a = s[..col].char_indices().rev().take_while(|&(_, x)| class(x) == k).last().map_or(col, |(i, _)| i);
    let b = s[col..].char_indices().find(|&(_, x)| class(x) != k).map_or(s.len(), |(i, _)| col + i);
    (a, b)
}

fn describe_span(a: Pos, b: Pos, bytes: usize) -> String {
    if a.line == b.line {
        format!("{}", human_size(bytes as u64))
    } else {
        format!("{} líneas ({})", fmt_num(b.line - a.line + 1), human_size(bytes as u64))
    }
}

/// Qué parte de cada bloque se copia.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CopyMode {
    /// Desde el inicio de «desde» hasta el final de «hasta».
    Full,
    /// Sólo el texto entre los delimitadores.
    Inner,
    /// Las líneas completas que ocupa el bloque.
    Lines,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sep {
    Newline,
    Blank,
    Nothing,
}

/// Una ventana de búsqueda avanzada (puede haber varias, cada una con su
/// archivo y sus resultados, dentro de la ventana principal o aparte).
struct AdvWin {
    id: u64,
    open: bool,
    /// En una ventana propia del sistema (se puede llevar a otro monitor).
    detached: bool,
    from: String,
    to: String,
    regex: bool,
    case_sensitive: bool,
    multiline: bool,
    mode: CopyMode,
    sep: Sep,
    focus: bool,
    /// Desplazar la lista hasta este bloque.
    scroll_to: Option<usize>,
    /// Archivo elegido (por defecto, el activo al abrir la ventana).
    file: Option<PathBuf>,
    /// Búsqueda en curso o terminada: (búsqueda, `edit_gen` del archivo, archivo).
    search: Option<(BlockSearch, u64, PathBuf)>,
    error: Option<String>,
    cur: Option<usize>,
    /// Densidad del minimapa: (nº bloques, alto, líneas, bloques por fila).
    minimap_cache: Option<(usize, usize, u64, Vec<u32>)>,
}

impl AdvWin {
    fn new(id: u64, file: Option<PathBuf>) -> Self {
        AdvWin {
            id,
            open: true,
            detached: false,
            from: String::new(),
            to: String::new(),
            regex: false,
            case_sensitive: false,
            multiline: false,
            mode: CopyMode::Full,
            sep: Sep::Newline,
            focus: true,
            scroll_to: None,
            file,
            search: None,
            error: None,
            cur: None,
            minimap_cache: None,
        }
    }

    fn title(&self) -> String {
        let name = self.file.as_ref().and_then(|p| p.file_name()).map(|n| format!(" — {}", n.to_string_lossy()));
        format!("Búsqueda avanzada {}{}", self.id, name.unwrap_or_default())
    }

    fn is_busy(&self) -> bool {
        self.search.as_ref().is_some_and(|(s, _, _)| !s.is_done())
    }

    /// Pestaña sobre la que trabaja esta ventana.
    fn tab_index(&self, tabs: &[Tab], active: usize) -> Option<usize> {
        let want = self.search.as_ref().map(|s| &s.2).or(self.file.as_ref());
        match want {
            Some(p) => tabs.iter().position(|t| &t.doc.path == p),
            None => (active < tabs.len()).then_some(active),
        }
    }

    fn start(&mut self, tab: &Tab) {
        let params = BlockParams {
            from: self.from.clone(),
            to: self.to.clone(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            multiline: self.multiline,
        };
        let edits = tab.buf.as_ref().map(|b| (b.snapshot(), tab.doc.index.clone()));
        self.cur = None;
        self.minimap_cache = None;
        self.scroll_to = Some(0);
        match BlockSearch::start(tab.doc.src.clone(), edits, params) {
            Ok(b) => {
                self.search = Some((b, tab.edit_gen, tab.doc.path.clone()));
                self.error = None;
            }
            Err(e) => {
                self.search = None;
                self.error = Some(e);
            }
        }
    }

    fn get(&self, i: usize) -> Option<Block> {
        self.search.as_ref().and_then(|(s, _, _)| s.get(i))
    }

    fn count(&self) -> usize {
        self.search.as_ref().map_or(0, |(s, _, _)| s.count())
    }
}

/// Lo que una ventana avanzada pide a la aplicación.
#[derive(Default)]
struct AdvOut {
    msg: Option<String>,
    activate: Option<usize>,
    focus_view: bool,
    new_window: bool,
}

fn block_range(b: &Block, mode: CopyMode) -> (Pos, Pos) {
    match mode {
        CopyMode::Full => (b.s0, b.e1),
        CopyMode::Inner => (b.s1, b.e0.max(b.s1)),
        CopyMode::Lines => (Pos::new(b.s0.line, 0), Pos::new(b.e1.line, usize::MAX)),
    }
}

fn settings_window_id() -> Id {
    Id::NULL.with("ventana_ajustes")
}

fn keys_window_id() -> Id {
    Id::NULL.with("ventana_atajos")
}

fn adv_window_id(n: u64) -> Id {
    Id::NULL.with(("ventana_avanzada", n))
}

const MINIMAP_GREEN: Color32 = Color32::from_rgb(40, 190, 80);

impl App {
    fn settings_window(&mut self, ctx: &Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let (mut theme, mut font) = (self.theme, self.font_size);
        Window::new("Ajustes").id(settings_window_id()).open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            ui.strong("Tema");
            ui.horizontal(|ui| {
                ui.radio_value(&mut theme, ThemePreference::System, "Sistema");
                ui.radio_value(&mut theme, ThemePreference::Light, "Claro");
                ui.radio_value(&mut theme, ThemePreference::Dark, "Oscuro");
            });
            ui.add_space(8.0);
            ui.strong("Tamaño de letra del texto");
            ui.add(Slider::new(&mut font, 7.0..=40.0).step_by(1.0));
            ui.add_space(8.0);
            ui.weak("Los ajustes se guardan automáticamente.");
        });
        self.show_settings = open;
        if theme != self.theme {
            self.theme = theme;
            ctx.set_theme(theme);
            self.persist();
        }
        if font != self.font_size {
            self.set_font(font);
        }
    }

    /// Escape cierra la ventana emergente que está delante (si el foco no
    /// está en el editor, donde Escape quita la selección).
    fn escape_closes_popup(&mut self, ctx: &Context) {
        if self.view_focused || !ctx.input(|i| i.key_pressed(Key::Escape)) {
            return;
        }
        let inline: Vec<Id> = self.advs.iter().filter(|w| !w.detached).map(|w| adv_window_id(w.id)).collect();
        let open: Vec<Id> = [self.show_keys.then(keys_window_id), self.show_settings.then(settings_window_id)]
            .into_iter()
            .flatten()
            .chain(inline)
            .collect();
        if open.is_empty() {
            return;
        }
        // La de delante; si no se sabe, la última abierta.
        let top = ctx.memory(|m| m.areas().top_layer_id(Order::Middle)).map(|l| l.id).filter(|id| open.contains(id));
        let target = top.unwrap_or(*open.last().unwrap());
        if target == keys_window_id() {
            self.show_keys = false;
        } else if target == settings_window_id() {
            self.show_settings = false;
        } else if let Some(w) = self.advs.iter_mut().find(|w| adv_window_id(w.id) == target) {
            w.open = false;
        }
    }

    fn keys_window(&mut self, ctx: &Context) {
        if !self.show_keys {
            return;
        }
        let mut open = true;
        Window::new("Atajos de teclado")
            .id(keys_window_id())
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([520.0, 560.0])
            .anchor(Align2::RIGHT_BOTTOM, vec2(-8.0, -30.0))
            .show(ctx, |ui| {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (section, keys) in shortcut_list() {
                        ui.add_space(4.0);
                        ui.strong(section);
                        Grid::new(section).num_columns(2).striped(true).spacing([16.0, 3.0]).min_col_width(230.0).show(ui, |ui| {
                            for (k, what) in keys {
                                ui.label(RichText::new(k).monospace());
                                ui.label(what);
                                ui.end_row();
                            }
                        });
                    }
                });
            });
        self.show_keys = open;
    }

    fn new_adv_window(&mut self) {
        let file = self.tabs.get(self.active).map(|t| t.doc.path.clone());
        self.advs.push(AdvWin::new(self.next_adv, file));
        self.next_adv += 1;
    }

    /// Dibuja todas las ventanas de búsqueda avanzada.
    fn adv_windows(&mut self, ctx: &Context) {
        let mut wins = std::mem::take(&mut self.advs);
        let mut outs = Vec::new();
        for w in wins.iter_mut() {
            let title = w.title();
            let mut out = AdvOut::default();
            let (tabs, active) = (&mut self.tabs, self.active);
            if w.detached {
                let vid = ViewportId::from_hash_of(("adv", w.id));
                let builder = ViewportBuilder::default().with_title(&title).with_inner_size([680.0, 600.0]).with_min_inner_size([420.0, 300.0]);
                ctx.show_viewport_immediate(vid, builder, |ui, _| {
                    CentralPanel::default().show(ui, |ui| out = adv_ui(ui, w, tabs, active));
                    // Escape cierra también la ventana aparte.
                    if ui.ctx().input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
                        w.open = false;
                    }
                });
            } else {
                let mut open = true;
                Window::new(&title)
                    .id(adv_window_id(w.id))
                    .open(&mut open)
                    .default_size([640.0, 560.0])
                    .max_width(ctx.content_rect().width() * 0.9)
                    .min_size([420.0, 300.0])
                    .resizable(true)
                    .collapsible(false)
                    .show(ctx, |ui| out = adv_ui(ui, w, tabs, active));
                w.open &= open;
            }
            outs.push(out);
        }
        wins.retain(|w| w.open);
        self.advs = wins;
        for o in outs {
            if let Some(m) = o.msg {
                self.msg(m);
            }
            if let Some(i) = o.activate {
                self.active = i;
            }
            self.focus_view |= o.focus_view;
            if o.new_window {
                self.new_adv_window();
            }
        }
    }
}

fn adv_ui(ui: &mut Ui, w: &mut AdvWin, tabs: &mut [Tab], active: usize) -> AdvOut {
    let mut out = AdvOut::default();
    // Archivo, ventana aparte, nueva ventana.
    ui.horizontal(|ui| {
        ui.label("Archivo:");
        let cur_name = w
            .tab_index(tabs, active)
            .map(|i| tabs[i].doc.name.clone())
            .unwrap_or_else(|| "(ninguno)".into());
        let mut chosen = None;
        ComboBox::from_id_salt(("adv_file", w.id)).selected_text(cur_name).width(220.0).show_ui(ui, |ui| {
            for t in tabs.iter() {
                if ui.selectable_label(w.file.as_ref() == Some(&t.doc.path), &t.doc.name).clicked() {
                    chosen = Some(t.doc.path.clone());
                }
            }
        });
        if let Some(p) = chosen {
            if w.file.as_ref() != Some(&p) {
                w.file = Some(p);
                w.search = None;
                w.cur = None;
                w.minimap_cache = None;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("+ Nueva").on_hover_text("Abrir otra ventana de búsqueda avanzada").clicked() {
                out.new_window = true;
            }
            let label = if w.detached { "Integrar" } else { "Ventana aparte" };
            let hint = if w.detached {
                "Volver a mostrarla dentro de la ventana principal"
            } else {
                "Mostrarla en una ventana propia (puedes llevarla a otro monitor)"
            };
            if ui.button(label).on_hover_text(hint).clicked() {
                w.detached = !w.detached;
            }
        });
    });
    let Some(ti) = w.tab_index(tabs, active) else {
        ui.weak("Abre un archivo primero (o el archivo elegido se cerró).");
        return out;
    };

    let mut run = false;
    let field = |ui: &mut Ui, text: &mut String, hint: &str, multiline: bool| -> Response {
        if multiline {
            ui.add(TextEdit::multiline(text).desired_width(360.0).desired_rows(3).hint_text(hint).font(TextStyle::Monospace))
        } else {
            ui.add(TextEdit::singleline(text).desired_width(360.0).hint_text(hint).font(TextStyle::Monospace))
        }
    };
    let enter = |ui: &Ui, r: &Response, multiline: bool| {
        if multiline {
            r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.command)
        } else {
            r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
        }
    };
    Grid::new(("adv_grid", w.id)).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Desde:");
        ui.horizontal(|ui| {
            let r = field(ui, &mut w.from, "inicio del bloque (texto o regex)", w.multiline);
            if std::mem::take(&mut w.focus) {
                r.request_focus();
            }
            run |= enter(ui, &r, w.multiline);
            ui.toggle_value(&mut w.case_sensitive, "Aa").on_hover_text("Distinguir mayúsculas");
            ui.toggle_value(&mut w.regex, ".*").on_hover_text("Expresión regular");
            ui.toggle_value(&mut w.multiline, "Multilínea").on_hover_text(
                "Cada patrón puede ocupar varias líneas (hasta 1 MB).\nTexto: escribe los saltos tal cual (sirve para LF y CRLF); busca con Ctrl+Enter.\nRegex: usa \\n o \\r?\\n; (?s) para que «.» cruce líneas.",
            );
        });
        ui.end_row();
        ui.label("Hasta:");
        let r = field(ui, &mut w.to, "fin del bloque, puede estar en otra línea (opcional)", w.multiline);
        run |= enter(ui, &r, w.multiline);
        ui.end_row();
    });
    ui.horizontal(|ui| {
        run |= ui.button("Buscar").clicked();
        if let Some(e) = &w.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        } else if let Some((s, edit_gen, _)) = &w.search {
            let n = s.count();
            let pos = w.cur.map(|i| format!("{}/", fmt_num(i as u64 + 1))).unwrap_or_default();
            let mut t = format!("{pos}{} bloques", fmt_num(n as u64));
            if s.truncated.load(Ordering::Relaxed) {
                t += &format!(" (límite {})", fmt_num(MAX_BLOCKS as u64));
            }
            if !s.is_done() {
                let p = s.progress.load(Ordering::Relaxed) as f64 / s.total.max(1) as f64;
                t += &format!(" · buscando {:.0}%", p * 100.0);
            } else {
                t += &format!(" · {:.2}s", s.elapsed.lock().unwrap().as_secs_f32());
                if s.unclosed.load(Ordering::Relaxed) > 0 {
                    t += " · el último inicio no tiene fin";
                }
            }
            ui.label(t);
            if *edit_gen != tabs[ti].edit_gen {
                ui.colored_label(ui.visuals().warn_fg_color, "desactualizada (Buscar para repetir)");
            }
        }
    });
    if run {
        w.start(&tabs[ti]);
    }
    ui.separator();

    // Opciones de copia.
    let has_end = !w.to.is_empty();
    ui.horizontal_wrapped(|ui| {
        ui.label("Copiar:");
        ui.radio_value(&mut w.mode, CopyMode::Full, if has_end { "desde… hasta (incluidos)" } else { "coincidencia" });
        ui.add_enabled_ui(has_end, |ui| {
            ui.radio_value(&mut w.mode, CopyMode::Inner, "solo lo de en medio").on_hover_text("Sin los textos de «Desde» y «Hasta»");
        });
        ui.radio_value(&mut w.mode, CopyMode::Lines, "líneas completas");
    });
    if !has_end && w.mode == CopyMode::Inner {
        w.mode = CopyMode::Full;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Separar bloques con:");
        ui.radio_value(&mut w.sep, Sep::Newline, "salto de línea");
        ui.radio_value(&mut w.sep, Sep::Blank, "línea en blanco");
        ui.radio_value(&mut w.sep, Sep::Nothing, "nada");
    });
    let count = w.count();
    let tab = &mut tabs[ti];
    let mut go = None;
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(count > 0, Button::new("◀")).on_hover_text("Bloque anterior").clicked() {
            go = Some(w.cur.map_or(count - 1, |i| if i == 0 { count - 1 } else { i - 1 }));
        }
        if ui.add_enabled(count > 0, Button::new("▶")).on_hover_text("Bloque siguiente").clicked() {
            go = Some(w.cur.map_or(0, |i| (i + 1) % count));
        }
        let cur = w.cur.filter(|&i| i < count);
        if ui
            .add_enabled(cur.is_some(), Button::new("Seleccionar en el editor"))
            .on_hover_text("Selecciona el bloque actual (según «Copiar») para copiarlo o editarlo")
            .clicked()
        {
            if let Some(b) = cur.and_then(|i| w.get(i)) {
                let (a, z) = block_range(&b, w.mode);
                let z = Pos::new(z.line, z.col.min(tab.line_len(z.line)));
                tab.select(a, z);
                tab.center_on = Some(a.line);
                out.focus_view = true;
                out.activate = Some(ti);
            }
        }
        let ctx = ui.ctx().clone();
        let copy = |only: Option<usize>| -> String {
            let Some((s, _, _)) = &w.search else { return "Busca algo primero".into() };
            if only.is_none() && !s.is_done() {
                return "Espera a que termine la búsqueda".into();
            }
            let all = s.blocks.lock().unwrap();
            let list = match only {
                Some(i) => all.get(i..=i).unwrap_or(&[]),
                None => &all[..],
            };
            match tab.blocks_text(list, w.mode, w.sep) {
                Ok((t, n)) => {
                    let m = format!("Copiados {} bloques ({})", fmt_num(n as u64), human_size(t.len() as u64));
                    ctx.copy_text(t);
                    m
                }
                Err(e) => e,
            }
        };
        if ui.add_enabled(cur.is_some(), Button::new("Copiar bloque")).clicked() {
            out.msg = Some(copy(cur));
        }
        if ui.add_enabled(count > 0, Button::new(format!("Copiar todos ({})", fmt_num(count as u64)))).clicked() {
            out.msg = Some(copy(None));
        }
    });
    ui.separator();

    // Lista de bloques + minimapa, en rectángulos fijos.
    let rest = ui.available_rect_before_wrap();
    let rest = Rect::from_min_max(rest.min, pos2(rest.max.x, rest.max.y.max(rest.min.y + 80.0)));
    ui.allocate_rect(rest, Sense::hover());
    let map_w = 56.0;
    let list_rect = Rect::from_min_max(rest.min, pos2(rest.max.x - map_w - 8.0, rest.max.y));
    let map_rect = Rect::from_min_max(pos2(rest.max.x - map_w, rest.min.y), rest.max);
    ui.scope_builder(UiBuilder::new().max_rect(list_rect).layout(Layout::top_down_justified(Align::Min)), |ui| {
        ui.style_mut().wrap_mode = Some(TextWrapMode::Truncate);
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().button_padding.y = 2.0;
        let row_h = ui.text_style_height(&TextStyle::Monospace) + 4.0;
        let mut area = ScrollArea::vertical().id_salt(("adv_list", w.id)).auto_shrink([false, false]);
        if let Some(i) = w.scroll_to.take() {
            area = area.vertical_scroll_offset((i as f32 * row_h - list_rect.height() / 3.0).max(0.0));
        }
        area.show_rows(ui, row_h, count, |ui, range| {
            let list: Vec<Block> = w
                .search
                .as_ref()
                .map(|(s, _, _)| {
                    let v = s.blocks.lock().unwrap();
                    v[range.start.min(v.len())..range.end.min(v.len())].to_vec()
                })
                .unwrap_or_default();
            for (k, b) in list.iter().enumerate() {
                let i = range.start + k;
                let lines = if b.s0.line == b.e1.line {
                    fmt_num(b.s0.line + 1)
                } else {
                    format!("{}–{}", fmt_num(b.s0.line + 1), fmt_num(b.e1.line + 1))
                };
                let t = tab.rows(b.s0.line, 1, 400).into_iter().next().map(|r| r.1).unwrap_or_default();
                let preview = sanitize(&t[buffer::floor_char(&t, b.s0.col)..]);
                let text = RichText::new(format!("{lines:>15}  {preview}")).monospace();
                if ui.selectable_label(w.cur == Some(i), text).clicked() {
                    go = Some(i);
                }
            }
        });
    });
    let resp = ui.interact(map_rect, ui.scope_id().with(("minimap", w.id)), Sense::click_and_drag());
    if let Some(line) = minimap(ui, map_rect, &resp, tab, w) {
        // Ir al bloque más cercano a ese punto.
        let i = w.search.as_ref().map(|(s, _, _)| s.first_at_or_after(Pos::new(line, 0))).unwrap_or(0);
        if i < count {
            go = Some(i);
            w.scroll_to = Some(i);
        } else {
            tab.center_on = Some(line);
            out.activate = Some(ti);
        }
    }
    if let Some(i) = go {
        if let Some(b) = w.get(i) {
            w.cur = Some(i);
            if w.scroll_to.is_none() && !ui.rect_contains_pointer(list_rect) {
                w.scroll_to = Some(i);
            }
            tab.goto_block(b);
            out.activate = Some(ti);
        }
    }
    out
}

/// Minimapa del documento entero: en verde, dónde están los bloques
/// encontrados (más intenso = más bloques); el recuadro es la parte visible.
/// Devuelve la línea pulsada.
fn minimap(ui: &mut Ui, rect: Rect, resp: &Response, tab: &Tab, w: &mut AdvWin) -> Option<u64> {
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    painter.rect_stroke(rect, 2.0, visuals.widgets.noninteractive.bg_stroke, StrokeKind::Inside);
    let total = tab.total().max(1);
    let inner = rect.shrink2(vec2(4.0, 2.0));
    let h = inner.height().max(1.0) as usize;
    let y_of = |line: u64| inner.top() + (line as f64 / total as f64 * h as f64) as f32;

    if let Some((s, _, _)) = &w.search {
        let count = s.count();
        let fresh = w.minimap_cache.as_ref().is_some_and(|c| c.0 == count && c.1 == h && c.2 == total);
        if !fresh {
            // Nº de bloques que caen en cada fila de píxeles.
            let mut marks = vec![0u32; h];
            for b in s.blocks.lock().unwrap().iter() {
                let a = (b.s0.line as f64 / total as f64 * h as f64) as usize;
                let z = (b.e1.line as f64 / total as f64 * h as f64) as usize;
                for m in &mut marks[a.min(h - 1)..=z.min(h - 1)] {
                    *m += 1;
                }
            }
            w.minimap_cache = Some((count, h, total, marks));
        }
        let marks = &w.minimap_cache.as_ref().unwrap().3;
        let max = marks.iter().copied().max().unwrap_or(0).max(1) as f32;
        for (y, &c) in marks.iter().enumerate() {
            if c == 0 {
                continue;
            }
            let k = 0.3 + 0.7 * (c as f32 / max).sqrt();
            let r = Rect::from_min_max(pos2(inner.left(), inner.top() + y as f32), pos2(inner.right(), inner.top() + y as f32 + 1.0));
            painter.rect_filled(r, 0.0, MINIMAP_GREEN.gamma_multiply(k));
        }
        // Bloque actual.
        if let Some(b) = w.cur.and_then(|i| s.get(i)) {
            let r = Rect::from_min_max(pos2(rect.left(), y_of(b.s0.line) - 1.5), pos2(rect.right(), y_of(b.e1.line) + 1.5));
            painter.rect_stroke(r, 1.0, Stroke::new(2.0, visuals.strong_text_color()), StrokeKind::Outside);
        }
    }
    // Parte visible en el editor.
    let view = Rect::from_min_max(
        pos2(rect.left() + 1.0, y_of(tab.top)),
        pos2(rect.right() - 1.0, y_of(tab.top + tab.visible_rows).max(y_of(tab.top) + 3.0)),
    );
    painter.rect_filled(view, 1.0, visuals.selection.bg_fill.gamma_multiply(0.25));
    painter.rect_stroke(view, 1.0, Stroke::new(1.0, visuals.selection.stroke.color), StrokeKind::Inside);

    let r = resp.clone().on_hover_text("Verde: bloques encontrados (más intenso = más bloques) · recuadro: lo que se ve · clic: ir allí");
    if r.clicked() || r.dragged() {
        let p = r.interact_pointer_pos()?;
        let frac = ((p.y - inner.top()) / inner.height()).clamp(0.0, 1.0) as f64;
        return Some(((frac * total as f64) as u64).min(total - 1));
    }
    None
}

/// Lista virtual de líneas con coincidencias.
fn results_view(ui: &mut Ui, tab: &mut Tab, font: &FontId) {
    let Some(search) = &tab.search else { return };
    let ctx = ui.ctx().clone();
    let row_h = ctx.fonts_mut(|f| f.row_height(font)).ceil();
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    let list = Rect::from_min_max(full.min, pos2(full.max.x - SCROLLBAR, full.max.y));
    let vsb = Rect::from_min_max(pos2(list.max.x, full.min.y), full.max);
    let id = ui.scope_id().with("results");
    let resp = ui.interact(list, id, Sense::click());

    let count = search.count() as u64;
    let visible = ((list.height() / row_h).floor() as u64).max(1);

    // Mantener visible la coincidencia actual.
    let cur_idx = tab.current.and_then(|l| search.index_of(l)).map(|i| i as u64);
    if let Some(i) = cur_idx {
        if i < tab.results_top {
            tab.results_top = i;
        } else if i >= tab.results_top + visible {
            tab.results_top = i + 1 - visible;
        }
    }
    if resp.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta.y);
        if d != 0.0 {
            tab.results_top = add_signed(tab.results_top, (-d / row_h * 3.0).round() as i64);
        }
    }
    tab.results_top = tab.results_top.min(count.saturating_sub(visible));

    let rows: Vec<u64> = {
        let r = search.results.lock().unwrap();
        let a = (tab.results_top as usize).min(r.len());
        let b = (a + visible as usize + 1).min(r.len());
        r[a..b].to_vec()
    };
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(list);
    painter.rect_filled(list, 0.0, visuals.extreme_bg_color);
    let mut clicked = None;
    let num_w = ctx.fonts_mut(|f| f.glyph_width(font, '0')) * 12.0;
    for (k, &line) in rows.iter().enumerate() {
        let y = list.top() + k as f32 * row_h;
        let row = Rect::from_min_size(pos2(list.left(), y), vec2(list.width(), row_h));
        let r = ui.interact(row.intersect(list), id.with(k), Sense::click());
        if tab.current == Some(line) {
            painter.rect_filled(row, 0.0, visuals.selection.bg_fill.gamma_multiply(0.5));
        } else if r.hovered() {
            painter.rect_filled(row, 0.0, visuals.widgets.hovered.weak_bg_fill);
        }
        if r.clicked() {
            clicked = Some(line);
        }
        painter.text(pos2(list.left() + num_w, y), Align2::RIGHT_TOP, fmt_num(line + 1), font.clone(), visuals.weak_text_color());
        if let Some((_, txt, tr)) = tab.rows(line, 1, 2048).into_iter().next() {
            let job = line_job(&txt, tr, font, visuals.text_color(), Some(&search.highlight));
            painter.galley(pos2(list.left() + num_w + 12.0, y), painter.layout_job(job), visuals.text_color());
        }
    }
    let mut top = tab.results_top;
    vscrollbar(ui, vsb, id.with("vsb"), &mut top, count, visible);
    tab.results_top = top;
    if let Some(l) = clicked {
        tab.goto_match(l);
    }
}

/// Línea actual completa en un cuadro de texto seleccionable (copia parcial).
fn detail_view(ui: &mut Ui, tab: &mut Tab, font: &FontId) {
    let Some(line) = tab.cursor.map(|c| c.line) else {
        ui.weak("Selecciona una línea para verla completa aquí.");
        return;
    };
    let version = tab.version();
    if tab.detail_cache.as_ref().map(|c| (c.0, c.1)) != Some((line, version)) {
        let text = match tab.rows(line, 1, MAX_DETAIL_BYTES).into_iter().next() {
            Some((_, mut t, tr)) => {
                if tr {
                    t += "\n…[truncado: la línea mide más de 1 MB]";
                }
                t
            }
            None => String::new(),
        };
        tab.detail_cache = Some((line, version, text));
    }
    let text = &tab.detail_cache.as_ref().unwrap().2;
    ui.label(RichText::new(format!("Línea {}", fmt_num(line + 1))).small().weak());
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.add(TextEdit::multiline(&mut text.as_str()).font(font.clone()).desired_width(f32::INFINITY));
    });
}

/// Barra vertical con precisión de u64 (las de egui usan f32 y no sirven
/// para cientos de millones de líneas).
fn vscrollbar(ui: &mut Ui, rect: Rect, id: Id, top: &mut u64, total: u64, visible: u64) {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let vis = ui.visuals();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, vis.extreme_bg_color);
    let max_top = total.saturating_sub(visible);
    if max_top == 0 {
        return;
    }
    let track = rect.shrink(2.0);
    let thumb_h = (track.height() * (visible as f64 / total as f64) as f32).clamp(24.0_f32.min(track.height()), track.height());
    let free = (track.height() - thumb_h).max(1.0);
    if resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            let frac = ((p.y - track.top() - thumb_h / 2.0) / free).clamp(0.0, 1.0) as f64;
            *top = (frac * max_top as f64).round() as u64;
        }
    }
    let y = track.top() + free * (*top as f64 / max_top as f64) as f32;
    let thumb = Rect::from_min_size(pos2(track.left(), y), vec2(track.width(), thumb_h));
    let c = if resp.hovered() || resp.dragged() { vis.widgets.active.bg_fill } else { vis.widgets.inactive.bg_fill };
    painter.rect_filled(thumb, 3.0, c);
}

fn hscrollbar(ui: &mut Ui, rect: Rect, id: Id, offset: &mut f32, content: f32, view: f32) {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let vis = ui.visuals();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, vis.extreme_bg_color);
    let max_off = (content - view).max(0.0);
    *offset = offset.min(max_off);
    if max_off <= 0.0 {
        return;
    }
    let track = rect.shrink(2.0);
    let thumb_w = (track.width() * view / content).clamp(24.0_f32.min(track.width()), track.width());
    let free = (track.width() - thumb_w).max(1.0);
    if resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            *offset = ((p.x - track.left() - thumb_w / 2.0) / free).clamp(0.0, 1.0) * max_off;
        }
    }
    let x = track.left() + free * (*offset / max_off);
    let thumb = Rect::from_min_size(pos2(x, track.top()), vec2(thumb_w, track.height()));
    let c = if resp.hovered() || resp.dragged() { vis.widgets.active.bg_fill } else { vis.widgets.inactive.bg_fill };
    painter.rect_filled(thumb, 3.0, c);
}

/// Añade fuentes del sistema como respaldo para CJK, cirílico, etc.
fn install_fallback_fonts(ctx: &Context) {
    const CANDIDATES: &[&str] = &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "C:\\Windows\\Fonts\\msgothic.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "/System/Library/Fonts/PingFang.ttc",
    ];
    for (i, path) in CANDIDATES.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(path) {
            ctx.add_font(epaint::text::FontInsert::new(
                &format!("fallback-{i}"),
                FontData::from_owned(bytes),
                vec![
                    epaint::text::InsertFontFamily { family: FontFamily::Monospace, priority: epaint::text::FontPriority::Lowest },
                    epaint::text::InsertFontFamily { family: FontFamily::Proportional, priority: epaint::text::FontPriority::Lowest },
                ],
            ));
        }
    }
}

fn add_signed(v: u64, d: i64) -> u64 {
    if d < 0 { v.saturating_sub(d.unsigned_abs()) } else { v.saturating_add(d as u64) }
}

fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out
}

fn human_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_with(name: &str, text: &str) -> (Tab, PathBuf) {
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        let doc = Document::open(&p).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        (Tab::new(doc), p)
    }

    fn key(t: &mut Tab, k: Key, m: Modifiers) {
        t.handle_key(k, m, 10).unwrap();
    }

    #[test]
    fn keyboard_editing_and_save() {
        let (mut t, p) = tab_with("ui.txt", "hola mundo\r\n\tsegunda\r\n");
        let word = if MAC { Modifiers::ALT } else { Modifiers::CTRL };
        t.select(Pos::new(0, 0), Pos::new(0, 0));
        key(&mut t, Key::End, Modifiers::NONE);
        assert_eq!(t.cursor, Some(Pos::new(0, 10)));
        key(&mut t, Key::ArrowLeft, word);
        assert_eq!(t.cursor, Some(Pos::new(0, 5)));
        key(&mut t, Key::ArrowLeft, word | Modifiers::SHIFT);
        assert_eq!(t.selection(), Some((Pos::new(0, 0), Pos::new(0, 5))));
        t.insert("¡Adiós ", EditKind::Typing).unwrap();
        assert_eq!(t.line(0), "¡Adiós mundo");
        key(&mut t, Key::Backspace, Modifiers::NONE);
        assert_eq!(t.line(0), "¡Adiósmundo");
        key(&mut t, Key::ArrowDown, Modifiers::NONE);
        key(&mut t, Key::Home, Modifiers::NONE); // tras la sangría
        assert_eq!(t.cursor, Some(Pos::new(1, 1)));
        key(&mut t, Key::Enter, Modifiers::NONE);
        assert_eq!((t.line(1).as_str(), t.line(2).as_str()), ("\t", "segunda"));
        key(&mut t, Key::Z, Modifiers::COMMAND);
        assert_eq!(t.line(1), "\tsegunda");
        assert_eq!(t.cursor, Some(Pos::new(1, 1)), "deshacer restaura el cursor");
        key(&mut t, Key::Home, Modifiers::NONE); // ya en la sangría: a la columna 0
        key(&mut t, Key::Delete, word);
        assert_eq!(t.line(1), "segunda");
        assert!(t.dirty());

        t.start_save(p.clone()).unwrap();
        let msg = loop {
            if let Some(m) = t.poll_save() {
                break m;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(msg.starts_with("Guardado"), "{msg}");
        assert!(t.buf.is_none() && !t.dirty());
        assert_eq!(std::fs::read(&p).unwrap(), "¡Adiósmundo\r\nsegunda\r\n".as_bytes());
        assert!(t.start_save(p).is_err(), "sin cambios no se guarda");
    }

    #[test]
    fn copy_blocks_modes() {
        let (t, _) = tab_with("cb.txt", "a <x>uno\ndos</x> b <x>tres</x>\nfin\n");
        let run = |to: &str| {
            let mut w = AdvWin::new(1, None);
            w.from = "<x>".into();
            w.to = to.into();
            w.case_sensitive = true;
            w.start(&t);
            while w.is_busy() {
                std::thread::sleep(Duration::from_millis(2));
            }
            let v = w.search.as_ref().unwrap().0.blocks.lock().unwrap().clone();
            v
        };
        let v = run("</x>");
        let txt = |m, s| t.blocks_text(&v, m, s).unwrap();
        assert_eq!(txt(CopyMode::Full, Sep::Blank), ("<x>uno\ndos</x>\n\n<x>tres</x>".to_string(), 2));
        assert_eq!(txt(CopyMode::Inner, Sep::Newline).0, "uno\ndos\ntres");
        // Líneas completas: la línea 2 no se repite aunque tenga dos bloques.
        assert_eq!(txt(CopyMode::Lines, Sep::Newline), ("a <x>uno\ndos</x> b <x>tres</x>".to_string(), 1));
        assert_eq!(t.blocks_text(&v[1..], CopyMode::Inner, Sep::Nothing).unwrap().0, "tres");
        // Sin «hasta»: cada coincidencia es un bloque.
        let v = run("");
        assert_eq!(t.blocks_text(&v, CopyMode::Full, Sep::Nothing).unwrap(), ("<x><x>".to_string(), 2));
    }

    #[test]
    fn duplicate_names() {
        let dir = std::env::temp_dir().join("buscador-tests").join("dup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("informe.log");
        std::fs::write(&p, "x").unwrap();
        assert_eq!(copy_name(&p), dir.join("informe copia.log"));
        std::fs::write(dir.join("informe copia.log"), "x").unwrap();
        assert_eq!(copy_name(&p), dir.join("informe copia 2.log"));
        assert_eq!(copy_name(&dir.join("LEEME")), dir.join("LEEME copia"));
    }

    #[test]
    fn read_only_blocks_edits() {
        let (mut t, _) = tab_with("ro.txt", "abc");
        t.editable = false;
        assert!(t.insert("x", EditKind::Typing).is_err());
        assert_eq!(t.line(0), "abc");
    }

    #[test]
    fn display_columns() {
        assert_eq!(disp_col("\tab", 1), 4);
        assert_eq!(col_from_disp("\tab", 4), 1);
        assert_eq!(col_from_disp("\tab", 1), 0);
        assert_eq!(col_from_disp("ñx", 1), 2);
        assert_eq!(word_at("foo.bar baz", 5), (4, 7));
        assert_eq!(word_right_in("foo  bar", 0), 5);
        assert_eq!(word_left_in("foo  bar", 5), 0);
    }
}
