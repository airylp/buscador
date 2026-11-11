//! Interfaz gráfica (egui).

use crate::buffer::{self, Buffer, Changed, EditKind, Pos};
use crate::document::Document;
use crate::encoding;
use crate::blocks::{Block, BlockParams, BlockSearch, MAX_BLOCKS};
use crate::search::{MAX_RESULTS, Params, Search};
use crate::settings::Settings;
use crate::syntax;
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
/// Líneas anteriores a lo visible que se miran para saber si se está dentro
/// de un comentario o cadena de varias líneas.
const SYNTAX_LOOKBACK: u64 = 300;
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
    /// Archivo nuevo sin guardar: su `doc` es un archivo vacío temporal que se
    /// borra al guardar o al cerrar; Guardar pide dónde.
    untitled: bool,
    /// Texto a meter en cuanto el documento esté listo (archivo nuevo con lo copiado).
    pending_text: Option<String>,
    /// Petición del menú contextual: crear un archivo nuevo con este texto.
    export_request: Option<String>,
    /// Lenguaje para el resaltado (por la extensión; se puede cambiar).
    lang: syntax::Lang,
    /// Estado del resaltado al inicio de lo visible: ((top, edit_gen, lang), estado).
    syntax_cache: Option<((u64, u64, syntax::Lang), syntax::State)>,
    /// Selección en bloque (columnas), en columnas visuales: puede pasar del
    /// final de las líneas.
    colsel: Option<ColSel>,
    /// Modo bloque activo (botón «Bloque» con letra monoespaciada).
    col_mode: bool,
    /// El arrastre en curso selecciona un bloque.
    col_dragging: bool,
    /// Cuenta todas las modificaciones (no se reinicia al guardar).
    edit_gen: u64,
}

impl Tab {
    fn new(doc: Document) -> Self {
        let doc_name = doc.name.clone();
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
            untitled: false,
            lang: syntax::detect(&doc_name),
            syntax_cache: None,
            pending_text: None,
            export_request: None,
            colsel: None,
            col_mode: false,
            col_dragging: false,
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
                        let old = std::mem::replace(&mut self.doc, doc);
                        if std::mem::take(&mut self.untitled) {
                            let _ = std::fs::remove_file(&old.path);
                        }
                        if old.name != self.doc.name {
                            self.lang = syntax::detect(&self.doc.name);
                        }
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

    // ---- Selección en bloque (columnas) ----

    /// Bloque seleccionado, si no está vacío.
    fn block(&self) -> Option<ColSel> {
        self.colsel.filter(|c| !c.is_empty())
    }

    /// Texto real dentro del bloque: por cada línea, los caracteres cuyas
    /// columnas caen dentro; los huecos más allá del final de una línea no se
    /// copian. Las filas se separan con saltos de línea.
    fn column_text(&self, cs: ColSel, max: usize) -> Option<String> {
        let (l0, l1) = cs.lines();
        let (c0, c1) = cs.cols();
        let mut out = String::new();
        let mut n = l0;
        while n <= l1 {
            let chunk = self.rows(n, ((l1 - n + 1) as usize).min(4096), usize::MAX);
            if chunk.is_empty() {
                break;
            }
            for (ln, t, _) in chunk {
                if ln > l0 {
                    out.push('\n');
                }
                let mut d = 0;
                for ch in t.chars() {
                    if d >= c1 {
                        break;
                    }
                    if d >= c0 {
                        out.push(ch);
                    }
                    d += char_disp_width(ch);
                }
                if out.len() > max {
                    return None;
                }
                n = ln + 1;
            }
        }
        Some(out)
    }

    fn copy_block(&self, ctx: &Context, cs: ColSel) -> String {
        let (l0, l1) = cs.lines();
        match self.column_text(cs, MAX_COPY_BYTES) {
            Some(t) => {
                let m = format!("Copiado bloque de {} líneas ({})", fmt_num(l1 - l0 + 1), human_size(t.len() as u64));
                ctx.copy_text(t);
                m
            }
            None => format!("Bloque demasiado grande para copiar (> {})", human_size(MAX_COPY_BYTES as u64)),
        }
    }

    /// Mayús+flechas en modo bloque: amplía el bloque desde el cursor.
    fn extend_block(&mut self, key: Key) -> bool {
        let (dl, dc): (i64, i64) = match key {
            Key::ArrowUp => (-1, 0),
            Key::ArrowDown => (1, 0),
            Key::ArrowLeft => (0, -1),
            Key::ArrowRight => (0, 1),
            _ => return false,
        };
        let cs = self.colsel.unwrap_or_else(|| {
            let c = self.cursor_or_top();
            let v = (c.line, disp_col(&self.line(c.line), c.col));
            ColSel { a: v, b: v }
        });
        let last = self.total().saturating_sub(1);
        let b = (add_signed(cs.b.0, dl).min(last), add_signed(cs.b.1 as u64, dc) as usize);
        self.colsel = Some(ColSel { a: cs.a, b });
        let t = self.line(b.0);
        let p = Pos::new(b.0, col_from_disp(&t, b.1));
        self.select(p, p);
        true
    }

    // ---- Embellecer ----

    /// Tipo de embellecedor para este archivo: (nombre, ¿HTML?).
    fn beautifier(&self) -> Option<(&'static str, bool)> {
        let ext = self.doc.name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        match self.lang {
            syntax::Lang::Json => Some(("JSON", false)),
            syntax::Lang::Xml if matches!(ext.as_str(), "html" | "htm" | "vue") => Some(("HTML", true)),
            syntax::Lang::Xml => Some(("XML", false)),
            _ => None,
        }
    }

    /// Valida y embellece la selección (o el archivo entero). Si está mal
    /// formado, no cambia nada y lleva el cursor al error.
    fn beautify(&mut self) -> Result<String, String> {
        let Some((kind, html)) = self.beautifier() else {
            return Err("El embellecedor está disponible para JSON y XML/HTML (el tipo se cambia en la barra de estado)".into());
        };
        let (a, b) = self.selection().unwrap_or((Pos::default(), self.end_pos()));
        let text = self
            .text(a, b, MAX_COPY_BYTES)
            .ok_or_else(|| format!("Demasiado grande para embellecer (máx. {})", human_size(MAX_COPY_BYTES as u64)))?;
        let res = if kind == "JSON" {
            crate::beautify::format_json(&text, "  ")
        } else {
            crate::beautify::format_xml(&text, "  ", html)
        };
        let mut out = match res {
            Ok(o) => o,
            Err(e) => {
                // Posición del error en el documento.
                let pos = e.pos.min(text.len());
                let (l, c) = crate::beautify::line_col(&text, pos);
                let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
                let col = pos - line_start + if l == 1 { a.col } else { 0 };
                let p = Pos::new(a.line + l as u64 - 1, col);
                self.select(p, p);
                self.center_on = Some(p.line);
                let c = if l == 1 { c + a.col } else { c };
                return Err(format!("{kind} mal formado en la línea {}, columna {c}: {}", fmt_num(p.line + 1), e.msg));
            }
        };
        if text.ends_with('\n') && !out.ends_with('\n') {
            out.push('\n');
        }
        if out == text {
            return Ok(format!("{kind} bien formado; ya estaba embellecido"));
        }
        if !self.editable {
            return Ok(format!("{kind} bien formado. Activa «Editar» (Ctrl+E) para embellecerlo"));
        }
        self.replace_range(a, b, &out, EditKind::Other)?;
        self.select(a, a);
        self.center_on = Some(a.line);
        Ok(format!("{kind} embellecido: {} líneas (Ctrl+Z para deshacer)", fmt_num(out.lines().count() as u64)))
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
                    self.colsel = None;
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
                    msg = Some(match self.block() {
                        Some(cs) => self.copy_block(ctx, cs),
                        None => self.copy(ctx),
                    });
                    Ok(())
                }
                Event::Cut => match self.block() {
                    Some(cs) => {
                        msg = Some(format!("{} (en un bloque, Cortar solo copia)", self.copy_block(ctx, cs)));
                        Ok(())
                    }
                    None => self.cut(ctx).map(|m| msg = Some(m)),
                },
                Event::Key { key, pressed: true, modifiers, .. }
                    if self.col_mode && modifiers.shift && !modifiers.command && self.extend_block(key) =>
                {
                    Ok(())
                }
                // En macOS también Ctrl+C / Ctrl+X / Ctrl+V (costumbre de Windows y Linux).
                Event::Key { key: key @ (Key::C | Key::X | Key::V), pressed: true, modifiers, .. }
                    if MAC && modifiers.ctrl && !modifiers.command =>
                {
                    match key {
                        Key::C => {
                            msg = Some(match self.block() {
                                Some(cs) => self.copy_block(ctx, cs),
                                None => self.copy(ctx),
                            });
                            Ok(())
                        }
                        Key::X => match self.block() {
                            Some(cs) => {
                                msg = Some(format!("{} (en un bloque, Cortar solo copia)", self.copy_block(ctx, cs)));
                                Ok(())
                            }
                            None => self.cut(ctx).map(|m| msg = Some(m)),
                        },
                        _ => {
                            ctx.send_viewport_cmd(ViewportCommand::RequestPaste);
                            Ok(())
                        }
                    }
                }
                Event::Key { key, pressed: true, modifiers, .. } => {
                    // Cualquier otra tecla (salvo atajos) deja la selección en bloque.
                    if !modifiers.command && !modifiers.ctrl {
                        self.colsel = None;
                    }
                    self.handle_key(key, modifiers, page)
                }
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
        (k("C+Shift+B"), "Embellecer JSON / XML (valida y sangra)"),
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
                (k("C+N"), "Archivo nuevo (para pegar y guardar lo copiado)"),
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
                (k("A+arrastrar"), "Seleccionar un bloque (rectángulo); siempre con el botón ▦ Bloque"),
                ("Mayús+flechas (▦ Bloque)".into(), "Ampliar el bloque"),
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

/// Selección rectangular: (línea, columna visual) de la esquina donde empezó
/// y de la esquina actual.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ColSel {
    a: (u64, usize),
    b: (u64, usize),
}

impl ColSel {
    fn lines(&self) -> (u64, u64) {
        (self.a.0.min(self.b.0), self.a.0.max(self.b.0))
    }
    fn cols(&self) -> (usize, usize) {
        (self.a.1.min(self.b.1), self.a.1.max(self.b.1))
    }
    fn is_empty(&self) -> bool {
        self.a.1 == self.b.1
    }
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

impl Drop for Tab {
    fn drop(&mut self) {
        if self.untitled {
            let _ = std::fs::remove_file(&self.doc.path);
        }
    }
}

/// Carpeta de los archivos temporales de «Nuevo».
fn untitled_dir() -> PathBuf {
    std::env::temp_dir().join("buscador-nuevos")
}

/// Borra temporales de «Nuevo» olvidados (de hace más de una semana).
fn clean_old_untitled() {
    let Ok(rd) = std::fs::read_dir(untitled_dir()) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d > Duration::from_secs(7 * 86_400));
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
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
    /// Tipo de letra del texto y fuentes de respaldo ya leídas.
    text_font: String,
    /// Colores propios del texto (tienen prioridad sobre el tema).
    custom_colors: bool,
    text_bg: Color32,
    text_fg: Color32,
    /// Botón «Bloque»: seleccionar rectángulos (solo con letra monoespaciada).
    column_mode: bool,
    /// Colorear según el lenguaje / niveles de log.
    syntax_on: bool,
    /// Siguiente número para «Sin título N».
    next_untitled: u32,
    fallback_fonts: Vec<(String, Arc<FontData>)>,
    show_settings: bool,
    show_keys: bool,
    /// La ventana de ajustes estaba abierta en el fotograma anterior.
    settings_was_open: bool,
    /// Actualización desde la interfaz (ventana abierta si hay `Some`).
    updater: Option<crate::update::Updater>,
    /// El editor tenía el foco en el último fotograma.
    view_focused: bool,
    /// Ventanas de búsqueda avanzada abiertas.
    advs: Vec<AdvWin>,
    next_adv: u64,
    message: Option<(String, Instant)>,
    focus_search: bool,
    focus_goto: bool,
    /// El campo «Ir a línea» de la barra de estado está abierto.
    goto_open: bool,
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
        let settings = Settings::load();
        clean_old_untitled();
        let fallbacks = crate::fonts::load_fallbacks();
        cc.egui_ctx.set_fonts(crate::fonts::build(&settings.font, &fallbacks).0);
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
            text_font: settings.font.clone(),
            custom_colors: settings.colors.is_some(),
            text_bg: settings.colors.map_or(DEFAULT_BG, |c| Color32::from_rgb(c.0[0], c.0[1], c.0[2])),
            text_fg: settings.colors.map_or(DEFAULT_FG, |c| Color32::from_rgb(c.1[0], c.1[1], c.1[2])),
            column_mode: false,
            syntax_on: settings.syntax,
            next_untitled: 1,
            fallback_fonts: fallbacks,
            show_settings: false,
            show_keys: false,
            settings_was_open: false,
            updater: None,
            view_focused: false,
            advs: Vec::new(),
            next_adv: 1,
            message: None,
            focus_search: false,
            focus_goto: false,
            goto_open: false,
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

    /// Archivo nuevo vacío (o con `text`), editable; Guardar pedirá dónde.
    fn new_untitled(&mut self, text: Option<String>) {
        let n = self.next_untitled;
        self.next_untitled += 1;
        let dir = untitled_dir();
        let path = dir.join(format!("sin-titulo-{}-{n}.txt", std::process::id()));
        let r = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, b"")).and_then(|_| Document::open(&path));
        match r {
            Ok(mut doc) => {
                doc.name = format!("Sin título {n}");
                let mut t = Tab::new(doc);
                t.untitled = true;
                t.editable = true;
                t.pending_text = text.filter(|s| !s.is_empty());
                self.tabs.push(t);
                self.active = self.tabs.len() - 1;
                self.focus_view = true;
            }
            Err(e) => self.msg(format!("No se pudo crear el archivo nuevo: {e}")),
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
        let rgb = |c: Color32| [c.r(), c.g(), c.b()];
        let st = Settings {
            theme: self.theme,
            font_size: self.font_size,
            font: self.text_font.clone(),
            colors: self.custom_colors.then(|| (rgb(self.text_bg), rgb(self.text_fg))),
            syntax: self.syntax_on,
        };
        if let Err(e) = st.save() {
            self.msg(format!("No se pudieron guardar los ajustes: {e}"));
        }
    }

    fn text_colors(&self) -> Option<(Color32, Color32)> {
        self.custom_colors.then_some((self.text_bg, self.text_fg))
    }

    /// ¿La letra del texto es monoespaciada? (el modo bloque la necesita)
    fn mono_font(&self) -> bool {
        crate::fonts::find(&self.text_font).is_none_or(|o| o.mono)
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

    fn beautify(&mut self) {
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        let m = t.beautify().unwrap_or_else(|e| e);
        self.msg(m);
        self.focus_view = true;
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
        let target = if save_as || t.untitled {
            let name = if t.untitled { format!("{}.txt", t.doc.name) } else { t.doc.name.clone() };
            let mut d = rfd::FileDialog::new().set_title("Guardar como").set_file_name(name);
            if let Some(dir) = t.doc.path.parent().filter(|_| !t.untitled) {
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
        if pressed(sc(Modifiers::COMMAND, Key::N)) {
            self.new_untitled(None);
        }
        if pressed(sc(Modifiers::COMMAND, Key::O)) {
            self.open_dialog();
        }
        if pressed(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::B)) {
            self.beautify();
        }
        if pressed(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::F)) {
            self.new_adv_window();
        }
        if pressed(sc(Modifiers::COMMAND, Key::F)) {
            self.focus_search = true;
        }
        if pressed(sc(Modifiers::COMMAND, Key::G)) {
            self.focus_goto = true;
            self.goto_open = true;
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

    /// Barra superior en una sola fila: archivo y edición | búsqueda |
    /// reemplazar y avanzada | (a la derecha) encoding y ajustes.
    fn toolbar(&mut self, ui: &mut Ui) {
        let h = ui.spacing().interact_size.y + 2.0;
        let full = ui.available_rect_before_wrap();
        let full = Rect::from_min_size(full.min, vec2(full.width(), h));
        ui.allocate_rect(full, Sense::hover());
        let right_w = if full.width() < 1150.0 { 140.0 } else { 200.0 };
        let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);
        let left = Rect::from_min_max(full.min, pos2(right.left() - 8.0, full.bottom()));
        // --- A la derecha (se reserva primero): encoding y ajustes ---
        ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
            if ui
                .selectable_label(self.show_settings, "⚙")
                .on_hover_text(if MAC { "Ajustes: tema y tamaño de letra (Cmd+,)" } else { "Ajustes: tema y tamaño de letra" })
                .clicked()
            {
                self.show_settings = !self.show_settings;
            }
            let mut msg = None;
            if let Some(t) = self.tabs.get_mut(self.active) {
                let mut enc = t.doc.src.encoding;
                ComboBox::from_id_salt("encoding")
                    .selected_text(encoding::label(enc))
                    .width(right_w - 50.0)
                    .truncate()
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
        });
        // Con poco espacio, los botones muestran solo el ícono (el texto queda en el tooltip).
        let compact = full.width() < 1480.0;
        let lb = |icon: &str, text: &str| if compact { icon.to_string() } else { format!("{icon} {text}") };
        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.set_clip_rect(left.intersect(ui.clip_rect()));
            // --- Archivo y edición ---
            if ui.selectable_label(self.show_sidebar, "☰").on_hover_text("Panel de archivos (Ctrl+B)").clicked() {
                self.show_sidebar = !self.show_sidebar;
            }
            if ui.button(lb("📝", "Nuevo")).on_hover_text("Archivo nuevo, por ejemplo para pegar lo copiado (Ctrl+N)").clicked() {
                self.new_untitled(None);
            }
            if ui.button(lb("📂", "Abrir")).on_hover_text("Abrir archivos (Ctrl+O) · también puedes arrastrarlos").clicked() {
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
            let untitled = self.tabs.get(self.active).is_some_and(|t| t.untitled);
            if ui.add_enabled(dirty || untitled, Button::new(lb("💾", "Guardar"))).on_hover_text("Guardar (Ctrl+S) · Guardar como: Ctrl+Shift+S").clicked() {
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
            if ui
                .add_enabled(has_tab, Button::selectable(editable, lb("✏", "Editar")))
                .on_hover_text("Activar/desactivar edición (Ctrl+E)")
                .clicked()
            {
                self.toggle_edit();
            }
            let mono = self.mono_font();
            let block_hint = if mono {
                "Selección en bloque: arrastra para marcar un rectángulo de texto (como una matriz). Al copiar se copia solo el texto real, línea por línea. También: Opción/Alt+arrastrar."
            } else {
                "La selección en bloque necesita una letra monoespaciada (⚙ Ajustes)"
            };
            if ui
                .add_enabled(mono && has_tab, Button::selectable(self.column_mode && mono, lb("▦", "Bloque")))
                .on_hover_text(block_hint)
                .on_disabled_hover_text(block_hint)
                .clicked()
            {
                self.column_mode = !self.column_mode;
                if !self.column_mode {
                    for t in &mut self.tabs {
                        t.colsel = None;
                    }
                }
                self.focus_view = true;
            }
            let beaut = self.tabs.get(self.active).and_then(|t| t.beautifier()).map(|b| b.0);
            let bhint = match beaut {
                Some(k) => format!("Embellecer {k}: comprueba que esté bien formado y lo ordena con sangría (Ctrl+Shift+B). Se aplica a la selección o a todo el archivo; Ctrl+Z lo deshace."),
                None => "Embellecer: disponible para JSON y XML/HTML".to_string(),
            };
            if ui
                .add_enabled(beaut.is_some(), Button::new(lb("✨", "Embellecer")))
                .on_hover_text(&bhint)
                .on_disabled_hover_text(&bhint)
                .clicked()
            {
                self.beautify();
            }
            ui.separator();

            // --- Búsqueda ---
            // Lo que va detrás del buscador ocupa ~400 px; el buscador se adapta.
            let search_w = (ui.available_width() - if compact { 330.0 } else { 440.0 }).clamp(100.0, 320.0);
            let r = ui.add(
                TextEdit::singleline(&mut self.query)
                    .desired_width(search_w)
                    .hint_text("🔍 Buscar…  (Ctrl+F, Enter)")
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
            if ui.add_enabled(has_tab, Button::new("◀")).on_hover_text("Anterior (Shift+Enter, Shift+F3)").clicked() {
                if let Some(t) = self.tab() {
                    t.prev_match();
                }
            }
            if ui.add_enabled(has_tab, Button::new("▶")).on_hover_text("Siguiente (Enter, F3)").clicked() {
                if self.tabs.get(self.active).is_some_and(|t| t.search.is_some()) {
                    if let Some(t) = self.tab() {
                        t.next_match();
                    }
                } else {
                    self.search_or_next();
                }
            }
            if let Some(t) = self.tabs.get(self.active) {
                if let Some(e) = &t.search_error {
                    ui.colored_label(ui.visuals().error_fg_color, "regex inválida").on_hover_text(e);
                } else if let Some(s) = &t.search {
                    // Compacto en la barra; el detalle, al pasar el ratón.
                    let n = s.count();
                    let pos = t.current.and_then(|l| s.index_of(l)).map(|i| format!("{}/", fmt_num(i as u64 + 1))).unwrap_or_default();
                    let mut txt = format!("{pos}{}", fmt_num(n as u64));
                    let mut hint = format!("{} líneas con coincidencias", fmt_num(n as u64));
                    if s.truncated.load(Ordering::Relaxed) {
                        txt += "+";
                        hint += &format!(" (límite {})", fmt_num(MAX_RESULTS as u64));
                    }
                    if !s.is_done() {
                        let p = s.progress.load(Ordering::Relaxed) as f64 / s.total.max(1) as f64;
                        txt += &format!(" · {:.0}%", p * 100.0);
                    } else {
                        hint += &format!(" · {:.2}s", s.elapsed.lock().unwrap().as_secs_f32());
                    }
                    let color = if t.search_stale { ui.visuals().warn_fg_color } else { ui.visuals().text_color() };
                    if t.search_stale {
                        hint += "\nDesactualizada por las ediciones: Enter para repetir";
                    }
                    ui.label(RichText::new(txt).color(color)).on_hover_text(hint);
                }
            }
            ui.separator();

            // --- Reemplazar y avanzada ---
            let rk = if MAC { "Cmd+Opción+F" } else { "Ctrl+H" };
            ui.toggle_value(&mut self.show_replace, lb("🔁", "Reemplazar")).on_hover_text(format!("Reemplazar ({rk})"));
            let n_adv = self.advs.len();
            let adv_label = if n_adv > 0 { format!("{} ({n_adv})", lb("🔎", "Avanzada")) } else { lb("🔎", "Avanzada") };
            if ui
                .selectable_label(n_adv > 0, adv_label)
                .on_hover_text("Nueva ventana de búsqueda por bloques (desde… hasta…, multilínea), con minimapa y copia en varias líneas (Ctrl+Shift+F). Puedes abrir varias.")
                .clicked()
            {
                self.new_adv_window();
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
                Cmd::Update => self.open_updater(ctx),
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
                        .on_hover_text(if t.untitled {
                            "Archivo nuevo sin guardar\nClic derecho: más opciones".to_string()
                        } else {
                            format!("{}\nClic derecho: más opciones", t.doc.path.display())
                        });
                    if r.clicked() {
                        self.active = i;
                    }
                    r.context_menu(|ui| {
                        let mut chosen = None;
                        let on_disk = !t.untitled;
                        let nw = ui.add_enabled(on_disk, Button::new("Abrir en una ventana nueva"));
                        let nw = if t.dirty() { nw.on_hover_text("La ventana nueva muestra la versión guardada en disco") } else { nw };
                        if nw.clicked() {
                            chosen = Some(FileAction::NewWindow(i));
                        }
                        ui.separator();
                        if ui.add_enabled(on_disk, Button::new(REVEAL_LABEL)).clicked() {
                            chosen = Some(FileAction::Reveal(i));
                        }
                        if ui.add_enabled(on_disk, Button::new("Copiar ruta")).clicked() {
                            chosen = Some(FileAction::CopyPath(i));
                        }
                        if ui.button("Copiar nombre").clicked() {
                            chosen = Some(FileAction::CopyName(i));
                        }
                        ui.separator();
                        let dup = ui.add_enabled(on_disk, Button::new("Duplicar"));
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

    /// Barra de estado: información del archivo y del cursor a la izquierda
    /// («Lín X» sirve para ir a una línea); Resultados, Detalle y «?» a la derecha.
    fn status_bar(&mut self, ui: &mut Ui) {
        let h = ui.spacing().interact_size.y;
        let full = ui.available_rect_before_wrap();
        let full = Rect::from_min_size(full.min, vec2(full.width(), h));
        ui.allocate_rect(full, Sense::hover());
        let right_w = 230.0_f32.min(full.width() / 2.0);
        let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);
        let left = Rect::from_min_max(full.min, pos2(right.left() - 6.0, full.bottom()));

        ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
            if ui
                .add(Button::new(RichText::new("?").small().strong()).small().selected(self.show_keys))
                .on_hover_text("Atajos de teclado (F1)")
                .clicked()
            {
                self.show_keys = !self.show_keys;
            }
            ui.separator();
            ui.toggle_value(&mut self.show_detail, RichText::new("📄 Detalle").small())
                .on_hover_text("Panel con la línea actual completa, seleccionable");
            ui.toggle_value(&mut self.show_results, RichText::new("📋 Resultados").small())
                .on_hover_text("Panel con la lista de líneas encontradas");
        });

        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.set_clip_rect(left.intersect(ui.clip_rect()));
            let mut goto_line = None;
            let mut set_lang = None;
            if let Some(t) = self.tabs.get(self.active) {
                let d = &t.doc;
                ui.label(RichText::new(if t.editable { "EDICIÓN" } else { "SOLO LECTURA" }).small().strong());
                ui.separator();
                // Cursor / ir a línea.
                if self.goto_open {
                    ui.label(RichText::new("Ir a línea:").small());
                    let r = ui.add(TextEdit::singleline(&mut self.goto).desired_width(90.0).hint_text("nº de línea"));
                    if std::mem::take(&mut self.focus_goto) {
                        r.request_focus();
                    }
                    if r.lost_focus() {
                        if ui.input(|i| i.key_pressed(Key::Enter)) {
                            goto_line = self.goto.trim().replace(['.', ',', '_'], "").parse::<u64>().ok();
                        }
                        self.goto_open = false;
                    }
                } else {
                    let mut s = match t.cursor {
                        Some(c) => {
                            let line = t.rows(c.line, 1, c.col.max(1)).into_iter().next().map(|r| r.1).unwrap_or_default();
                            let col = line[..buffer::floor_char(&line, c.col)].chars().count() + 1;
                            format!("Lín {}, Col {}", fmt_num(c.line + 1), fmt_num(col as u64))
                        }
                        None => "Ir a línea…".to_string(),
                    };
                    if let Some(cs) = t.block() {
                        let ((l0, l1), (c0, c1)) = (cs.lines(), cs.cols());
                        s += &format!(" · bloque {} × {}", fmt_num(l1 - l0 + 1), fmt_num((c1 - c0) as u64));
                    } else if let Some((a, b)) = t.selection() {
                        s += &if a.line == b.line {
                            format!(" · {} seleccionados", fmt_num((b.col - a.col) as u64))
                        } else {
                            format!(" · {} líneas seleccionadas", fmt_num(b.line - a.line + 1))
                        };
                    }
                    if ui.add(Button::new(RichText::new(s).small()).frame(false)).on_hover_text("Ir a línea (Ctrl+G)").clicked() {
                        self.goto_open = true;
                        self.focus_goto = true;
                    }
                }
                ui.separator();
                let lines = fmt_num(t.total());
                if d.indexing_done() {
                    ui.label(RichText::new(format!("{lines} líneas")).small());
                } else {
                    let p = d.progress.load(Ordering::Relaxed) as f32 / d.len().max(1) as f32;
                    ui.add(ProgressBar::new(p).desired_width(120.0).text(format!("indexando… {lines} líneas")));
                }
                ui.separator();
                ui.label(RichText::new(human_size(d.len() as u64)).small());
                ui.separator();
                ui.label(RichText::new(encoding::label(d.src.encoding)).small()).on_hover_text(format!("Detectado por: {}", d.detect_reason));
                if self.syntax_on {
                    ui.separator();
                    let mut lang = t.lang;
                    ui.menu_button(RichText::new(lang.name()).small(), |ui| {
                        for l in syntax::Lang::ALL {
                            if ui.selectable_label(l == lang, l.name()).clicked() {
                                lang = l;
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Lenguaje para colorear (se deduce de la extensión)");
                    if lang != t.lang {
                        set_lang = Some(lang);
                    }
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
                // Los mensajes van antes de la ruta para que no se corten.
                if let Some((m, when)) = &self.message {
                    if when.elapsed() < Duration::from_secs(8) {
                        ui.separator();
                        ui.label(RichText::new(m).small().color(ui.visuals().warn_fg_color));
                    }
                }
                ui.separator();
                if t.untitled {
                    ui.label(RichText::new("sin guardar en disco · Ctrl+S para elegir dónde").small().weak());
                } else {
                    ui.label(RichText::new(d.path.display().to_string()).small().weak()).on_hover_text("Ruta del archivo");
                }
            }
            if self.tabs.is_empty() {
                if let Some((m, when)) = &self.message {
                    if when.elapsed() < Duration::from_secs(8) {
                        ui.label(RichText::new(m).small().color(ui.visuals().warn_fg_color));
                    }
                }
            }
            if let (Some(l), Some(t)) = (set_lang, self.tabs.get_mut(self.active)) {
                t.lang = l;
                t.syntax_cache = None;
            }
            if let (Some(n), Some(t)) = (goto_line, self.tabs.get_mut(self.active)) {
                let last = t.total().saturating_sub(1);
                t.goto_line(n.saturating_sub(1).min(last));
                self.focus_view = true;
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
            if std::env::var("BUSCADOR_ACTUALIZAR").is_ok() {
                self.open_updater(ctx);
            }
            self.show_replace = std::env::var("BUSCADOR_REPLACE").is_ok();
            if let Ok(c) = std::env::var("BUSCADOR_COLORES") {
                // «#fondo,#letra»
                let hexc = |h: &str| u32::from_str_radix(h.trim().trim_start_matches('#'), 16).ok().map(|n| Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
                if let Some((b, f)) = c.split_once(',').and_then(|(b, f)| Some((hexc(b)?, hexc(f)?))) {
                    self.custom_colors = true;
                    self.text_bg = b;
                    self.text_fg = f;
                }
            }
            if let Ok(b) = std::env::var("BUSCADOR_BLOQUE") {
                // «l0,c0,l1,c1»
                let v: Vec<usize> = b.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if let (4, Some(t)) = (v.len(), self.tabs.get_mut(self.active)) {
                    self.column_mode = true;
                    t.colsel = Some(ColSel { a: (v[0] as u64, v[1]), b: (v[2] as u64, v[3]) });
                }
            }
            if let Ok(txt) = std::env::var("BUSCADOR_NUEVO") {
                self.new_untitled(Some(txt.replace("\\n", "\n")));
            }
            if let Ok(f) = std::env::var("BUSCADOR_FUENTE") {
                ctx.set_fonts(crate::fonts::build(&f, &self.fallback_fonts).0);
                self.text_font = f;
            }
            if let Ok(w) = std::env::var("BUSCADOR_ANCHO").map(|w| w.parse::<f32>().unwrap_or(900.0)) {
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(w, 700.0)));
            }
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
        if self.debug_frames == 45 && std::env::var("BUSCADOR_EMBELLECER").is_ok() {
            if let Some(t) = self.tabs.get_mut(self.active) {
                t.editable = true;
            }
            self.beautify();
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
        if self.debug_frames < 90 || busy || (std::env::var("BUSCADOR_TYPE").is_ok() && !self.debug_typed) {
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

        let font = crate::fonts::text_font(self.font_size);
        let colors = self.text_colors();
        let has_search = self.tabs.get(self.active).is_some_and(|t| t.search.is_some());
        if self.show_results && has_search {
            Panel::bottom("results").resizable(true).default_size(180.0).min_size(60.0).show(ui, |ui| {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    results_view(ui, t, &font, colors, self.syntax_on);
                }
            });
        }
        if self.show_detail && !self.tabs.is_empty() {
            Panel::bottom("detail").resizable(true).default_size(90.0).min_size(40.0).show(ui, |ui| {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    detail_view(ui, t, &font, colors);
                }
            });
        }

        let mut message = None;
        let mut view_focused = false;
        let colors = self.text_colors();
        let syntax_on = self.syntax_on;
        let col_mode = self.column_mode && self.mono_font();
        for t in &mut self.tabs {
            t.col_mode = col_mode;
        }
        let show_detail = &mut self.show_detail;
        let focus_view = std::mem::take(&mut self.focus_view);
        CentralPanel::default().frame(Frame::NONE).show(ui, |ui| {
            if let Some(t) = self.tabs.get_mut(self.active) {
                (message, view_focused) = log_view(ui, t, &font, show_detail, focus_view, colors, syntax_on);
            } else {
                ui.centered_and_justified(|ui| {
                    ui.heading("Arrastra archivos aquí, o pulsa Ctrl+O");
                });
            }
        });
        self.view_focused = view_focused;
        // «Nuevo archivo con la selección» y texto pendiente de archivos nuevos.
        let exports: Vec<String> = self.tabs.iter_mut().filter_map(|t| t.export_request.take()).collect();
        for text in exports {
            self.new_untitled(Some(text));
        }
        for t in &mut self.tabs {
            if t.pending_text.is_some() && t.doc.indexing_done() {
                let text = t.pending_text.take().unwrap();
                t.select(Pos::default(), Pos::default());
                if let Err(e) = t.insert(&text, EditKind::Other) {
                    self.message = Some((e, Instant::now()));
                }
                t.select(Pos::default(), Pos::default());
                t.center_on = Some(0);
            }
        }
        if self.tabs.iter().any(|t| t.pending_text.is_some()) {
            ctx.request_repaint();
        }
        if let Some(m) = message {
            self.msg(m);
        }
        self.settings_window(&ctx);
        self.update_window(&ctx);
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
fn log_view(
    ui: &mut Ui,
    tab: &mut Tab,
    font: &FontId,
    show_detail: &mut bool,
    focus: bool,
    colors: Option<(Color32, Color32)>,
    syntax_on: bool,
) -> (Option<String>, bool) {
    apply_text_colors(ui, colors);
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
    // Columna visual bajo el puntero (puede pasar del final de la línea).
    let vcol_at = |tab: &Tab, x: f32| -> usize { ((x - text_x + tab.hscroll) / char_w).round().max(0.0) as usize };
    if total > 0 {
        let shift = ui.input(|i| i.modifiers.shift);
        // Modo bloque, o Opción/Alt+arrastrar para un bloque puntual.
        let block_drag = tab.col_mode || ui.input(|i| i.modifiers.alt);
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
            } else if block_drag && !resp.secondary_clicked() {
                let pos = pos_at(tab, tab.top, p);
                let v = (pos.line, vcol_at(tab, p.x));
                tab.colsel = Some(match tab.colsel {
                    Some(cs) if shift => ColSel { a: cs.a, b: v },
                    _ => ColSel { a: v, b: v },
                });
                tab.col_dragging = true;
                tab.select(pos, pos);
            } else {
                tab.col_dragging = false;
                if !resp.secondary_clicked() {
                    tab.colsel = None;
                }
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
                    let pos = pos_at(tab, tab.top, clamped);
                    if tab.col_dragging {
                        let v = vcol_at(tab, clamped.x);
                        if let Some(cs) = &mut tab.colsel {
                            cs.b = (pos.line, v);
                        }
                        tab.select(pos, pos);
                    } else {
                        tab.cursor = Some(pos);
                    }
                }
                ctx.request_repaint();
            }
        }
    }
    resp.context_menu(|ui| {
        let has_sel = tab.selection().is_some();
        if ui.button("Copiar (Ctrl+C)").clicked() {
            message = Some(match tab.block() {
                Some(cs) => tab.copy_block(&ctx, cs),
                None => tab.copy(&ctx),
            });
            ui.close();
        }
        let has_any = tab.block().is_some() || tab.selection().is_some();
        if ui
            .add_enabled(has_any, Button::new("📝 Nuevo archivo con la selección"))
            .on_hover_text("Abre un archivo nuevo con el texto seleccionado (o el bloque), listo para guardar")
            .clicked()
        {
            let text = match (tab.block(), tab.selection()) {
                (Some(cs), _) => tab.column_text(cs, MAX_COPY_BYTES),
                (None, Some((a, b))) => tab.text(a, b, MAX_COPY_BYTES),
                _ => None,
            };
            match text {
                Some(t) => tab.export_request = Some(t),
                None => message = Some(format!("Selección demasiado grande (> {})", human_size(MAX_COPY_BYTES as u64))),
            }
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
    // Resaltado: estado arrastrado desde unas líneas antes de lo visible (cacheado).
    let lang = if syntax_on { tab.lang } else { syntax::Lang::Plain };
    let dark = is_dark(visuals.extreme_bg_color);
    let mut st = syntax::State::Normal;
    if lang.multiline() && tab.top > 0 {
        let key = (tab.top, tab.edit_gen, lang);
        st = match tab.syntax_cache {
            Some((k, s)) if k == key => s,
            _ => {
                let from = tab.top.saturating_sub(SYNTAX_LOOKBACK);
                let before = tab.rows(from, (tab.top - from) as usize, MAX_DISPLAY_BYTES);
                let s = syntax::state_after(lang, before.iter().map(|r| r.1.as_str()));
                tab.syntax_cache = Some((key, s));
                s
            }
        };
    }
    let mut spans = Vec::new();
    let rows: Vec<(u64, String, Arc<Galley>)> = tab
        .rows(tab.top, visible as usize + 1, MAX_DISPLAY_BYTES)
        .into_iter()
        .map(|(n, t, tr)| {
            spans.clear();
            st = syntax::highlight(lang, &t, st, &mut spans);
            let g = ui.painter().layout_job(line_job(&t, tr, font, visuals.text_color(), highlight, &spans, dark));
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
        // Selección en bloque: el rectángulo entero, aunque pase del final de la línea.
        if let Some(cs) = tab.colsel.filter(|c| (c.lines().0..=c.lines().1).contains(&n)) {
            let (c0, c1) = cs.cols();
            let (x0, x1) = (c0 as f32 * char_w, (c1 as f32 * char_w).max(c0 as f32 * char_w + 1.5));
            text_painter.rect_filled(Rect::from_min_max(pos2(ox + x0, y), pos2(ox + x1, y + row_h)), 0.0, sel_color);
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

const DEFAULT_BG: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
const DEFAULT_FG: Color32 = Color32::from_rgb(0xd4, 0xd4, 0xd4);

/// Colores propios del texto: sustituyen a los del tema en esta zona.
fn apply_text_colors(ui: &mut Ui, colors: Option<(Color32, Color32)>) {
    let Some((bg, fg)) = colors else { return };
    let v = ui.visuals_mut();
    v.extreme_bg_color = bg;
    v.override_text_color = Some(fg);
    v.weak_text_color = Some(bg.lerp_to_gamma(fg, 0.45));
    v.faint_bg_color = bg.lerp_to_gamma(fg, 0.07);
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
/// Texto de una línea con colores de sintaxis (`spans`) y las coincidencias
/// de búsqueda resaltadas encima.
fn line_job(
    txt: &str,
    truncated: bool,
    font: &FontId,
    color: Color32,
    hl: Option<&regex::Regex>,
    spans: &[syntax::Span],
    dark: bool,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let normal = TextFormat { font_id: font.clone(), color, ..Default::default() };
    let n = txt.len();
    // Por byte: tipo de sintaxis (0 = ninguno) y si es parte de una coincidencia.
    let mut kind: Vec<Option<syntax::Kind>> = vec![None; n];
    for sp in spans {
        for k in &mut kind[sp.start.min(n)..sp.end.min(n)] {
            *k = Some(sp.kind);
        }
    }
    let mut mark = vec![false; n];
    if let Some(re) = hl {
        for m in re.find_iter(txt).take(500).filter(|m| m.end() > m.start()) {
            mark[m.start()..m.end()].fill(true);
        }
    }
    let mut i = 0;
    while i < n {
        let (k, m) = (kind[i], mark[i]);
        let mut j = i + 1;
        while j < n && (kind[j], mark[j]) == (k, m) {
            j += 1;
        }
        while !txt.is_char_boundary(j) {
            j += 1;
        }
        let fmt = if m {
            TextFormat { background: Color32::from_rgb(255, 200, 0), color: Color32::BLACK, ..normal.clone() }
        } else if let Some(k) = k {
            TextFormat { color: syntax::color(k, dark), ..normal.clone() }
        } else {
            normal.clone()
        };
        job.append(&sanitize(&txt[i..j]), 0.0, fmt);
        i = j;
    }
    if truncated {
        job.append("  …[línea truncada; ver Detalle]", 0.0, TextFormat { italics: true, ..normal });
    }
    job
}

/// ¿Fondo oscuro? (para elegir los colores de sintaxis)
fn is_dark(bg: Color32) -> bool {
    let l = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    l < 128.0
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
    /// Texto para un archivo nuevo («A archivo nuevo»).
    new_doc: Option<String>,
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
        // Al abrirse, aparece centrada (luego se puede mover).
        let just_opened = self.show_settings && !std::mem::replace(&mut self.settings_was_open, self.show_settings);
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let (mut theme, mut font) = (self.theme, self.font_size);
        let mut family = self.text_font.clone();
        let (mut custom, mut bg, mut fg) = (self.custom_colors, self.text_bg, self.text_fg);
        let mut syn = self.syntax_on;
        let mut open_update = false;
        let mut reset = false;
        let mut win = Window::new("Ajustes").id(settings_window_id()).pivot(Align2::CENTER_CENTER);
        if just_opened {
            win = win.current_pos(ctx.content_rect().center());
        }
        win.open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
            ui.strong("Tema");
            ui.horizontal(|ui| {
                ui.radio_value(&mut theme, ThemePreference::System, "Sistema");
                ui.radio_value(&mut theme, ThemePreference::Light, "Claro");
                ui.radio_value(&mut theme, ThemePreference::Dark, "Oscuro");
            });
            ui.add_space(8.0);
            ui.strong("Tipo de letra del texto");
            ComboBox::from_id_salt("tipo_letra").selected_text(&family).width(220.0).show_ui(ui, |ui| {
                let av = crate::fonts::available();
                ui.weak("Monoespaciadas (alinean columnas)");
                for o in av.iter().filter(|o| o.mono) {
                    ui.selectable_value(&mut family, o.name.to_string(), o.name);
                }
                ui.separator();
                ui.weak("Proporcionales");
                for o in av.iter().filter(|o| !o.mono) {
                    ui.selectable_value(&mut family, o.name.to_string(), o.name);
                }
            });
            if crate::fonts::find(&family).is_some_and(|o| !o.mono) {
                ui.weak("Con letra proporcional las columnas de los logs no quedan alineadas.");
            }
            ui.add_space(8.0);
            ui.strong("Tamaño de letra del texto");
            ui.add(Slider::new(&mut font, 7.0..=40.0).step_by(1.0));
            ui.add_space(8.0);
            ui.strong("Colores del texto");
            ui.checkbox(&mut custom, "Usar colores propios (tienen prioridad sobre el tema)");
            ui.add_enabled_ui(custom, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Fondo");
                    ui.color_edit_button_srgba(&mut bg);
                    ui.add_space(12.0);
                    ui.label("Letra");
                    ui.color_edit_button_srgba(&mut fg);
                });
            });
            ui.add_space(8.0);
            ui.strong("Resaltado");
            ui.checkbox(&mut syn, "Colorear según el tipo de archivo (.java, .xml, .json, .py…) y los niveles de los logs");
            ui.add_space(6.0);
            ui.weak("Vista previa:");
            let (pbg, pfg) = if custom { (bg, fg) } else { (ui.visuals().extreme_bg_color, ui.visuals().text_color()) };
            Frame::NONE.fill(pbg).inner_margin(6.0).corner_radius(4.0).show(ui, |ui| {
                let sample = ["2026-11-10 12:00:01 [ERROR] año, acción: java.io.IOException", "0O 1lI {}[]() -> => != 日本"];
                let lang = if syn { syntax::Lang::Log } else { syntax::Lang::Plain };
                for line in sample {
                    let mut spans = Vec::new();
                    syntax::highlight(lang, line, syntax::State::Normal, &mut spans);
                    let job = line_job(line, false, &crate::fonts::text_font(font), pfg, None, &spans, is_dark(pbg));
                    ui.label(job);
                }
            });
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(format!("Versión {}", crate::update::version_label()));
                if crate::update::SUPPORTED {
                    if ui.button("⟳ Buscar actualizaciones…").on_hover_text("Descarga la última versión de GitHub, la compila y la instala").clicked() {
                        open_update = true;
                    }
                }
            });
            if !crate::update::SUPPORTED {
                ui.weak("En Windows, actualiza volviendo a ejecutar el instalador.");
            }
            ui.separator();
            if ui.button("↺ Restablecer ajustes predeterminados").on_hover_text("Tema del sistema, Hack 13, sin colores propios").clicked() {
                reset = true;
            }
            ui.add_space(8.0);
            ui.weak("Los ajustes se guardan automáticamente.");
        });
        self.show_settings = open;
        if open_update {
            self.open_updater(ctx);
        }
        if reset {
            let d = Settings::default();
            self.theme = d.theme;
            ctx.set_theme(d.theme);
            self.font_size = d.font_size;
            self.text_font = d.font;
            ctx.set_fonts(crate::fonts::build(&self.text_font, &self.fallback_fonts).0);
            self.custom_colors = false;
            self.syntax_on = d.syntax;
            self.text_bg = DEFAULT_BG;
            self.text_fg = DEFAULT_FG;
            self.persist();
            self.msg("Ajustes restablecidos");
            return;
        }
        if (custom, bg, fg, syn) != (self.custom_colors, self.text_bg, self.text_fg, self.syntax_on) {
            self.syntax_on = syn;
            self.custom_colors = custom;
            self.text_bg = bg;
            self.text_fg = fg;
            self.persist();
        }
        if theme != self.theme {
            self.theme = theme;
            ctx.set_theme(theme);
            self.persist();
        }
        if font != self.font_size {
            self.set_font(font);
        }
        if family != self.text_font {
            let (defs, ok) = crate::fonts::build(&family, &self.fallback_fonts);
            ctx.set_fonts(defs);
            if !ok {
                self.msg(format!("No se pudo cargar «{family}»; se usa {}", crate::fonts::DEFAULT));
            }
            self.text_font = family;
            for t in &mut self.tabs {
                t.content_width = 0.0;
            }
            self.persist();
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

    fn open_updater(&mut self, ctx: &Context) {
        if self.updater.as_ref().is_some_and(|u| u.stage() == crate::update::Stage::Running) {
            return; // ya está en marcha
        }
        self.updater = Some(crate::update::Updater::check(ctx.clone()));
    }

    fn update_window(&mut self, ctx: &Context) {
        use crate::update::Stage;
        let Some(u) = &self.updater else { return };
        let stage = u.stage();
        let mut open = true;
        let (mut run, mut restart) = (false, false);
        let dirty = self.tabs.iter().any(|t| t.dirty() || t.save.is_some());
        Window::new("Actualizar buscador")
            .id(Id::NULL.with("ventana_actualizar"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([560.0, 360.0])
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!("Versión instalada: {}", crate::update::version_label()));
                ui.add_space(6.0);
                match &stage {
                    Stage::Checking => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Comprobando la última versión en GitHub…");
                        });
                    }
                    Stage::UpToDate(h) => {
                        ui.label(format!("✔ Ya tienes la última versión ({h})."));
                        run = ui.button("Reinstalar de todos modos").clicked();
                    }
                    Stage::Available(h) => {
                        ui.strong(format!("Hay una versión nueva: {h}"));
                        ui.label("Se descarga el código a una carpeta temporal, se compila (la primera vez tarda unos minutos) y se reemplaza la app instalada. Puedes seguir trabajando mientras tanto.");
                        run = ui.button("⬇ Descargar, compilar e instalar").clicked();
                    }
                    Stage::Running => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Descargando, compilando e instalando…");
                        });
                    }
                    Stage::Done(p) => {
                        ui.strong(format!("✔ Instalada en {}", p.display()));
                        let b = ui.add_enabled(!dirty, Button::new("⟳ Reiniciar ahora"));
                        let b = if dirty { b.on_disabled_hover_text("Guarda o descarta los cambios abiertos antes de reiniciar") } else { b };
                        restart = b.clicked();
                        if dirty {
                            ui.weak("Hay cambios sin guardar: guárdalos antes de reiniciar.");
                        }
                    }
                    Stage::Failed(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, format!("No se pudo actualizar: {e}"));
                        run = ui.button("Reintentar").clicked();
                    }
                }
                let log = u.log.lock().unwrap();
                if !log.is_empty() {
                    ui.add_space(6.0);
                    ui.weak("Registro:");
                    ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).max_height(220.0).show(ui, |ui| {
                        for l in log.iter() {
                            ui.label(RichText::new(l).monospace().small());
                        }
                    });
                }
            });
        if run {
            u.run(ctx.clone());
        }
        if restart {
            if let Stage::Done(p) = &stage {
                match crate::update::relaunch(p) {
                    Ok(_) => {
                        self.allow_quit = true;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    Err(e) => self.msg(format!("No se pudo abrir la versión nueva: {e}")),
                }
            }
        }
        let running = stage == Stage::Running;
        if !open && !running {
            self.updater = None;
        } else if !open && running {
            self.msg("La actualización sigue en segundo plano; vuelve a abrirla desde Ajustes");
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
            if let Some(t) = o.new_doc {
                self.new_untitled(Some(t));
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
        if ui
            .add_enabled(count > 0, Button::new("📝 A archivo nuevo"))
            .on_hover_text("Abre un archivo nuevo con todos los bloques (según «Copiar» y el separador), listo para guardar")
            .clicked()
        {
            if let Some((s, _, _)) = &w.search {
                if !s.is_done() {
                    out.msg = Some("Espera a que termine la búsqueda".into());
                } else {
                    let all = s.blocks.lock().unwrap();
                    match tab.blocks_text(&all, w.mode, w.sep) {
                        Ok((t, _)) => out.new_doc = Some(t),
                        Err(e) => out.msg = Some(e),
                    }
                }
            }
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
fn results_view(ui: &mut Ui, tab: &mut Tab, font: &FontId, colors: Option<(Color32, Color32)>, syntax_on: bool) {
    let Some(search) = &tab.search else { return };
    apply_text_colors(ui, colors);
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
            let mut spans = Vec::new();
            if syntax_on {
                syntax::highlight(tab.lang, &txt, syntax::State::Normal, &mut spans);
            }
            let dark = is_dark(visuals.extreme_bg_color);
            let job = line_job(&txt, tr, font, visuals.text_color(), Some(&search.highlight), &spans, dark);
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
fn detail_view(ui: &mut Ui, tab: &mut Tab, font: &FontId, colors: Option<(Color32, Color32)>) {
    apply_text_colors(ui, colors);
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
    fn column_block_copies_only_real_text() {
        let (mut t, _) = tab_with("col.txt", "abc def\nx\n\tuno dos\n  hola mundo\n");
        // Columnas 2..6 de las líneas 0..3. El tab ocupa las columnas 0..4: empieza fuera del
        // bloque, así que no se copia.
        let cs = ColSel { a: (0, 2), b: (3, 6) };
        assert_eq!(t.column_text(cs, usize::MAX).unwrap(), "c de\n\nun\nhola");
        // El orden de las esquinas da igual.
        let cs2 = ColSel { a: (3, 6), b: (0, 2) };
        assert_eq!(t.column_text(cs2, usize::MAX), t.column_text(cs, usize::MAX));
        // Bloque totalmente fuera del texto: líneas vacías, sin espacios inventados.
        assert_eq!(t.column_text(ColSel { a: (0, 40), b: (1, 50) }, usize::MAX).unwrap(), "\n");
        // Mayús+flechas en modo bloque amplían desde el cursor.
        t.col_mode = true;
        t.select(Pos::new(0, 1), Pos::new(0, 1));
        t.extend_block(Key::ArrowDown);
        t.extend_block(Key::ArrowRight);
        assert_eq!(t.colsel, Some(ColSel { a: (0, 1), b: (1, 2) }));
    }

    #[test]
    fn untitled_save_becomes_real_file() {
        let dir = untitled_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("sin-titulo-test.txt");
        std::fs::write(&tmp, b"").unwrap();
        let doc = Document::open(&tmp).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut t = Tab::new(doc);
        t.untitled = true;
        t.select(Pos::default(), Pos::default());
        t.insert("copiado\nen bloque", EditKind::Other).unwrap();
        let target = std::env::temp_dir().join("buscador-tests").join("guardado-nuevo.txt");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&target);
        t.start_save(target.clone()).unwrap();
        while t.poll_save().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "copiado\nen bloque");
        assert!(!t.untitled && t.doc.path == std::fs::canonicalize(&target).unwrap_or(target.clone()) || t.doc.path == target);
        assert!(!tmp.exists(), "el temporal se borra al guardar");
        // Cerrar un «Sin título» también borra su temporal.
        let tmp2 = dir.join("sin-titulo-test2.txt");
        std::fs::write(&tmp2, b"").unwrap();
        let mut t2 = Tab::new(Document::open(&tmp2).unwrap());
        t2.untitled = true;
        drop(t2);
        assert!(!tmp2.exists());
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

#[cfg(test)]
mod copy_keeps_selection {
    use super::*;

    fn tab() -> Tab {
        use std::sync::atomic::AtomicUsize;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        // Un archivo por llamada: los tests corren en paralelo.
        let p = dir.join(format!("copia-sel-{}.txt", N.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&p, "hola mundo\nsegunda línea\n").unwrap();
        let doc = Document::open(&p).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        Tab::new(doc)
    }

    fn key(k: Key, m: Modifiers, pressed: bool) -> Event {
        Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers: m }
    }

    /// Devuelve el texto copiado al portapapeles, si lo hubo.
    fn run(t: &mut Tab, events: Vec<Event>) -> Option<String> {
        let ctx = Context::default();
        let out = ctx.run_ui(RawInput { events, ..Default::default() }, |ui| {
            t.handle_events(ui.ctx(), 10);
        });
        out.platform_output.commands.iter().find_map(|c| match c {
            OutputCommand::CopyText(s) => Some(s.clone()),
            _ => None,
        })
    }

    /// Arrastra con el ratón en la vista real y luego pulsa la combinación.
    fn drag_then(evs_after: Vec<Event>, alt: bool) -> Tab {
        let mut t = tab();
        t.col_mode = alt;
        let ctx = Context::default();
        let font = FontId::monospace(13.0);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let mods = if alt { Modifiers::ALT } else { Modifiers::NONE };
        let frames: Vec<Vec<Event>> = vec![
            vec![Event::ModifiersChanged(mods), Event::PointerMoved(pos2(120.0, 8.0))],
            vec![Event::PointerButton { pos: pos2(120.0, 8.0), button: PointerButton::Primary, pressed: true, modifiers: mods }],
            vec![Event::PointerMoved(pos2(160.0, 9.0))],
            vec![Event::PointerMoved(pos2(220.0, 26.0))],
            vec![Event::PointerButton { pos: pos2(220.0, 26.0), button: PointerButton::Primary, pressed: false, modifiers: mods }],
            vec![Event::ModifiersChanged(Modifiers::NONE)],
            evs_after,
            vec![],
        ];
        let mut show = false;
        for (k, events) in frames.into_iter().enumerate() {
            let input = RawInput { screen_rect: Some(screen), events, time: Some(k as f64 * 0.05), ..Default::default() };
            let _ = ctx.run_ui(input, |ui| {
                log_view(ui, &mut t, &font, &mut show, false, None, true);
            });
        }
        t
    }

    #[test]
    fn drag_select_then_ctrl_c() {
        let ctrl_c = || vec![key(Key::C, Modifiers::CTRL, true), key(Key::C, Modifiers::CTRL, false)];
        let cmd_c = || vec![key(Key::C, Modifiers::MAC_CMD | Modifiers::COMMAND, true), Event::Copy];
        for (name, evs) in [("Ctrl+C", ctrl_c()), ("Cmd+C", cmd_c())] {
            let t = drag_then(evs, false);
            assert!(t.selection().is_some(), "{name}: se perdió la selección");
            let t = drag_then(if name == "Ctrl+C" { ctrl_c() } else { cmd_c() }, true);
            assert!(t.block().is_some(), "{name}: se perdió el bloque");
        }
    }

    #[test]
    fn ctrl_c_cmd_c_and_copy_event() {
        for (name, evs) in [
            ("Ctrl+C", vec![key(Key::C, Modifiers::CTRL, true), key(Key::C, Modifiers::CTRL, false)]),
            ("Cmd+C", vec![key(Key::C, Modifiers::MAC_CMD | Modifiers::COMMAND, true), Event::Copy]),
            ("Copy", vec![Event::Copy]),
        ] {
            let mut t = tab();
            t.select(Pos::new(0, 0), Pos::new(0, 4));
            let copied = run(&mut t, evs.clone());
            if MAC || name != "Ctrl+C" {
                assert_eq!(copied.as_deref(), Some("hola"), "{name}: copia");
            }
            assert_eq!(t.selection(), Some((Pos::new(0, 0), Pos::new(0, 4))), "{name}: selección");
            t.col_mode = true;
            t.colsel = Some(ColSel { a: (0, 0), b: (1, 4) });
            run(&mut t, evs);
            assert!(t.block().is_some(), "{name}: bloque");
        }
    }
}
