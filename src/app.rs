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
/// Columnas entre paradas de tabulador (Ajustes → Tabulador); 4 por defecto.
static TAB_WIDTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(4);

/// Ver → «Mostrar espacios y tabuladores»: se pintan como · y →.
static SHOW_WS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn show_whitespace() -> bool {
    SHOW_WS.load(Ordering::Relaxed)
}

fn tab_width() -> usize {
    TAB_WIDTH.load(Ordering::Relaxed).clamp(1, 16)
}
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
    /// Posición a recuperar de la sesión anterior: (arriba, cursor). Se aplica
    /// cuando el índice ya llega hasta ahí.
    restore: Option<(u64, Pos)>,
    /// Llevar el cursor al final en cuanto el índice esté listo («Recargar»).
    goto_end: bool,
    /// Espacio (grupo del panel de archivos) al que pertenece: `Space::id`.
    space: Option<u32>,
    /// Petición del menú contextual: crear un archivo nuevo con este texto.
    export_request: Option<String>,
    /// Lenguaje para el resaltado (por la extensión; se puede cambiar).
    lang: syntax::Lang,
    /// Estado del resaltado al inicio de lo visible: ((top, edit_gen, lang), estado).
    syntax_cache: Option<((u64, u64, syntax::Lang), syntax::State)>,
    /// Selección en bloque (columnas), en columnas visuales: puede pasar del
    /// final de las líneas.
    colsel: Option<ColSel>,
    /// Con ajuste de línea: dónde quedó cada línea visible en el último
    /// fotograma (línea, y relativa, alto) y cuántas cabían enteras.
    layout_map: Vec<(u64, f32, f32)>,
    lines_shown: u64,
    /// Modo bloque activo (botón «Bloque» con letra monoespaciada).
    col_mode: bool,
    /// Ajuste de línea de este archivo (cada pestaña tiene el suyo).
    wrap: Wrap,
    /// El arrastre en curso selecciona un bloque.
    col_dragging: bool,
    /// Cuenta todas las modificaciones (no se reinicia al guardar).
    edit_gen: u64,
    /// «En vivo»: mirar el archivo cada poco y mostrar lo que se le añade.
    follow: bool,
    follow_at: Instant,
    /// Vista filtrada (reglas de incluir/excluir).
    filter: crate::filter::Filter,
    /// Minimapa ya dibujado y para qué estado (se rehace solo si cambia).
    minimap_cache: Option<(MiniKey, Arc<epaint::Mesh>)>,
}

/// Cada cuánto se mira si el archivo creció en modo «En vivo».
const FOLLOW_EVERY: Duration = Duration::from_millis(400);

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
            restore: None,
            goto_end: false,
            space: None,
            export_request: None,
            colsel: None,
            layout_map: Vec::new(),
            lines_shown: 1,
            col_mode: false,
            wrap: Wrap::Off,
            col_dragging: false,
            edit_gen: 0,
            follow: false,
            follow_at: Instant::now(),
            filter: Default::default(),
            minimap_cache: None,
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
            return Err(t!("Solo lectura: pulsa «Editar» (Ctrl+E) para modificar").into());
        }
        if self.save.is_some() {
            return Err(t!("Guardando… espera a que termine").into());
        }
        if self.buf.is_none() {
            if !self.doc.indexing_done() {
                return Err(t!("Espera a que termine de indexar para editar").into());
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

    /// Sangra (un tabulador más) o quita sangría (un tabulador o hasta 4
    /// espacios) a las líneas de la selección, o a la del cursor. Se deshace en
    /// un solo paso y la selección sigue abarcando las mismas líneas.
    fn indent(&mut self, outdent: bool) -> Result<(), String> {
        let (a, b) = self.selection().unwrap_or_else(|| {
            let c = self.cursor_or_top();
            (c, c)
        });
        // Si la selección acaba al principio de una línea, esa no cuenta.
        let last = if b.line > a.line && b.col == 0 { b.line - 1 } else { b.line };
        if (last - a.line) as usize >= MAX_REPLACE_LINES {
            return Err(tf!("Demasiadas líneas para sangrar de una vez (máx. {fmt_num})", fmt_num = fmt_num(MAX_REPLACE_LINES as u64)));
        }
        self.can_edit()?;
        let lines: Vec<u64> = (a.line..=last).collect();
        let cur = (self.anchor.unwrap_or_default(), self.cursor.unwrap_or_default());
        let buf = self.buf.as_mut().unwrap();
        let n = buf.replace_lines(&self.doc, &lines, cur, |t| {
            if outdent {
                let cut = if t.starts_with('\t') { 1 } else { t.bytes().take(tab_width()).take_while(|&c| c == b' ').count() };
                (cut > 0).then(|| t[cut..].to_string())
            } else if t.is_empty() {
                None
            } else {
                Some(format!("\t{t}"))
            }
        })?;
        if n == 0 {
            return Ok(());
        }
        self.detail_cache = None;
        self.edit_gen += 1;
        let start = Pos::new(a.line, 0);
        let end = Pos::new(last, self.line_len(last));
        if self.cursor.is_some_and(|c| c < self.anchor.unwrap_or(c)) {
            self.select(end, start);
        } else {
            self.select(start, end);
        }
        Ok(())
    }

    /// Quita los espacios y tabuladores del final de cada línea seleccionada
    /// (o del bloque, o de la línea del cursor). Un solo paso de deshacer.
    fn trim_trailing(&mut self) -> Result<String, String> {
        let (l0, l1) = match (self.block(), self.selection()) {
            (Some(cs), _) => cs.lines(),
            (None, Some((a, b))) => (a.line, if b.line > a.line && b.col == 0 { b.line - 1 } else { b.line }),
            (None, None) => {
                let c = self.cursor_or_top();
                (c.line, c.line)
            }
        };
        if (l1 - l0) as usize >= MAX_REPLACE_LINES {
            return Err(tf!("Demasiadas líneas para cambiar de una vez (máx. {n})", n = fmt_num(MAX_REPLACE_LINES as u64)));
        }
        self.can_edit()?;
        let lines: Vec<u64> = (l0..=l1).collect();
        let cur = (self.anchor.unwrap_or_default(), self.cursor.unwrap_or_default());
        let buf = self.buf.as_mut().unwrap();
        let n = buf.replace_lines(&self.doc, &lines, cur, |t| {
            let trimmed = t.trim_end_matches([' ', '\t']);
            (trimmed.len() != t.len()).then(|| trimmed.to_string())
        })?;
        if n > 0 {
            self.detail_cache = None;
            self.edit_gen += 1;
            self.search_stale |= self.search.is_some();
            // Los cursores pueden haber quedado más allá del final.
            let fix = |t: &Tab, p: Pos| Pos::new(p.line, p.col.min(t.line_len(p.line)));
            if let (Some(a), Some(c)) = (self.anchor, self.cursor) {
                let (a, c) = (fix(self, a), fix(self, c));
                self.select(a, c);
            }
        }
        Ok(if n == 0 {
            t!("No había espacios ni tabs al final").to_string()
        } else {
            tf!("Quitados espacios y tabs al final en {n} líneas (Ctrl+Z para deshacer)", n = fmt_num(n as u64))
        })
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
            return Err(if redo { t!("Nada que rehacer") } else { t!("Nada que deshacer") }.into());
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
                let Some(c) = self.cursor else { return t!("Nada seleccionado").into() };
                (Pos::new(c.line, 0), Pos::new(c.line, usize::MAX))
            }
        };
        match self.text(a, b, MAX_COPY_BYTES) {
            Some(t) => {
                let msg = tf!("Copiado: {len}", len = describe_span(a, b, t.len()));
                ctx.copy_text(t);
                msg
            }
            None => tf!("Selección demasiado grande para copiar (> {human_size})", human_size = human_size(MAX_COPY_BYTES as u64)),
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
        let t = self.text(a, b, MAX_COPY_BYTES).ok_or(t!("Selección demasiado grande para cortar"))?;
        let msg = tf!("Cortado: {len}", len = describe_span(a, b, t.len()));
        ctx.copy_text(t);
        self.replace_range(a, b, "", EditKind::Other)?;
        Ok(msg)
    }

    // ---- Reemplazar ----

    fn replace_current(&mut self, repl: &str, regex_mode: bool) -> Result<(), String> {
        let Some(s) = &self.search else { return Err(t!("Busca algo primero").into()) };
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
        let Some(s) = &self.search else { return Err(t!("Busca algo primero").into()) };
        if !s.is_done() {
            return Err(t!("Espera a que termine la búsqueda").into());
        }
        if self.search_stale {
            return Err(t!("La búsqueda está desactualizada: pulsa Enter en «Buscar» para repetirla").into());
        }
        if s.truncated.load(Ordering::Relaxed) || s.count() > MAX_REPLACE_LINES {
            return Err(tf!("Demasiadas líneas para reemplazar de una vez (máx. {fmt_num})", fmt_num = fmt_num(MAX_REPLACE_LINES as u64)));
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
        Ok(tf!("Reemplazado en {fmt_num} líneas (Ctrl+Z para deshacer)", fmt_num = fmt_num(n as u64)))
    }

    // ---- Guardar ----

    fn start_save(&mut self, target: PathBuf) -> Result<(), String> {
        if self.save.is_some() {
            return Err(t!("Ya se está guardando").into());
        }
        let same = target == self.doc.path;
        if self.buf.is_none() {
            if same {
                return Err(t!("No hay cambios que guardar").into());
            }
            if !self.doc.indexing_done() {
                return Err(t!("Espera a que termine de indexar").into());
            }
            self.buf = Some(Buffer::new(&self.doc));
        }
        let buf = self.buf.as_ref().unwrap();
        if same && !buf.is_dirty() {
            return Err(t!("No hay cambios que guardar").into());
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

    /// Si terminó un guardado: normalmente se conserva lo editado (y con ello
    /// el historial de deshacer) y solo se marca como guardado. Se reabre el
    /// archivo, liberando RAM y perdiendo el deshacer, si hay muchas líneas
    /// editadas en memoria (> 64 MB) o el archivo está «En vivo».
    fn poll_save(&mut self) -> Option<String> {
        let job = self.save.as_ref()?;
        let r = job.result.lock().unwrap().take()?;
        let target = job.target.clone();
        self.save = None;
        let lossy = match r {
            Ok(l) => l,
            Err(e) => return Some(tf!("Error al guardar: {e}", e = e)),
        };
        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        let ram = self.buf.as_ref().map_or(0, |b| b.memory().1);
        let keep = !self.follow && ram < (64 << 20) && self.buf.is_some();
        let old_name = self.doc.name.clone();
        let mut m = if keep {
            let old_path = self.doc.path.clone();
            self.doc.rebind(&target);
            if std::mem::take(&mut self.untitled) {
                let _ = std::fs::remove_file(&old_path);
                let _ = std::fs::remove_dir(untitled_dir()); // solo si quedó vacía
            }
            self.buf.as_mut().unwrap().mark_saved();
            tf!("Guardado {name} ({len})", name = self.doc.name, len = human_size(size))
        } else {
            let enc = self.doc.src.encoding;
            match Document::open(&target) {
                Ok(mut doc) => {
                    if doc.src.encoding != enc {
                        doc.set_encoding(enc);
                    }
                    let old = std::mem::replace(&mut self.doc, doc);
                    if std::mem::take(&mut self.untitled) {
                        let _ = std::fs::remove_file(&old.path);
                        let _ = std::fs::remove_dir(untitled_dir());
                    }
                    self.buf = None;
                    self.minimap_cache = None;
                    tf!("Guardado {name} ({len}) · el deshacer se reinició", name = self.doc.name, len = human_size(size))
                }
                Err(e) => return Some(tf!("Guardado, pero no se pudo reabrir: {e}", e = e)),
            }
        };
        if old_name != self.doc.name {
            self.lang = syntax::detect(&self.doc.name);
        }
        self.detail_cache = None;
        if lossy > 0 {
            m += &tf!(" · {lossy} caracteres no representables se guardaron como «?»", lossy = lossy);
        }
        Some(m)
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
                    d += char_disp_width(ch, d);
                }
                if out.len() > max {
                    return None;
                }
                n = ln + 1;
            }
        }
        Some(out)
    }

    /// Cortar en bloque: copia y borra (si se puede editar; si no, solo copia).
    fn cut_block(&mut self, ctx: &Context, cs: ColSel) -> Result<String, String> {
        let m = self.copy_block(ctx, cs);
        if !self.editable {
            return Ok(tf!("{m} (solo lectura: no se borró)", m = m));
        }
        self.delete_block(cs).map(|_| m)
    }

    fn copy_block(&self, ctx: &Context, cs: ColSel) -> String {
        let (l0, l1) = cs.lines();
        match self.column_text(cs, MAX_COPY_BYTES) {
            Some(t) => {
                let m = tf!("Copiado bloque de {fmt_num} líneas ({len})", fmt_num = fmt_num(l1 - l0 + 1), len = human_size(t.len() as u64));
                ctx.copy_text(t);
                m
            }
            None => tf!("Bloque demasiado grande para copiar (> {human_size})", human_size = human_size(MAX_COPY_BYTES as u64)),
        }
    }

    /// Borra el texto del bloque en cada una de sus líneas (un solo paso de
    /// deshacer); el cursor queda en la esquina de arriba a la izquierda.
    fn delete_block(&mut self, cs: ColSel) -> Result<(), String> {
        let (l0, l1) = cs.lines();
        let (c0, c1) = cs.cols();
        if (l1 - l0) as usize >= MAX_REPLACE_LINES {
            return Err(tf!("Demasiadas líneas para borrar de una vez (máx. {n})", n = fmt_num(MAX_REPLACE_LINES as u64)));
        }
        self.can_edit()?;
        let lines: Vec<u64> = (l0..=l1).collect();
        let cur = (self.anchor.unwrap_or_default(), self.cursor.unwrap_or_default());
        let buf = self.buf.as_mut().unwrap();
        buf.replace_lines(&self.doc, &lines, cur, |t| {
            let mut out = String::with_capacity(t.len());
            let mut d = 0;
            let mut changed = false;
            for ch in t.chars() {
                if d >= c0 && d < c1 {
                    changed = true;
                } else {
                    out.push(ch);
                }
                d += char_disp_width(ch, d);
            }
            changed.then_some(out)
        })?;
        self.detail_cache = None;
        self.edit_gen += 1;
        self.search_stale |= self.search.is_some();
        self.colsel = None;
        let t = self.line(l0);
        let p = Pos::new(l0, col_from_disp(&t, c0));
        self.select(p, p);
        Ok(())
    }

    /// Tab / Mayús+Tab con un bloque: mueve todo su texto a la siguiente (o
    /// anterior) parada de tabulador en cada línea, y el bloque lo acompaña
    /// (se puede repetir). A la derecha se inserta un tab delante del bloque; a
    /// la izquierda se quitan los espacios/tabs que haya justo delante.
    fn block_indent(&mut self, cs: ColSel, outdent: bool) -> Result<(), String> {
        let (l0, l1) = cs.lines();
        let (c0, c1) = cs.cols();
        let tw = tab_width();
        // Nueva columna de inicio del bloque.
        let to = if outdent { c0.saturating_sub(1) / tw * tw } else { c0 + tab_stop(c0) };
        if outdent && c0 == 0 {
            return Ok(());
        }
        if (l1 - l0) as usize >= MAX_REPLACE_LINES {
            return Err(tf!("Demasiadas líneas para cambiar de una vez (máx. {n})", n = fmt_num(MAX_REPLACE_LINES as u64)));
        }
        self.can_edit()?;
        let lines: Vec<u64> = (l0..=l1).collect();
        let cur = (self.anchor.unwrap_or_default(), self.cursor.unwrap_or_default());
        let buf = self.buf.as_mut().unwrap();
        // Columna donde queda el bloque (la de la primera línea que cambia).
        let mut new_c0: Option<usize> = None;
        buf.replace_lines(&self.doc, &lines, cur, |t| {
            // Byte donde empieza la columna c0 (None si la línea es más corta).
            let mut d = 0;
            let mut at = None;
            let mut starts = Vec::new(); // (byte, columna) de cada carácter
            for (i, ch) in t.char_indices() {
                if d >= c0 && at.is_none() {
                    at = Some(i);
                }
                starts.push((i, d));
                d += char_disp_width(ch, d);
            }
            if at.is_none() && d >= c0 {
                at = Some(t.len());
            }
            let at = at?;
            if !outdent {
                // Si la línea no llega al bloque, no se toca.
                if at == t.len() && d < c0 {
                    return None;
                }
                let mut out = String::with_capacity(t.len() + 1);
                out.push_str(&t[..at]);
                out.push('\t');
                out.push_str(&t[at..]);
                new_c0.get_or_insert(to);
                return Some(out);
            }
            // Quitar espacios/tabs justo antes de c0, como mucho hasta la columna `to`.
            let mut cut = (at, c0);
            for &(i, col) in starts.iter().rev().filter(|&&(i, _)| i < at) {
                let ch = t[i..].chars().next().unwrap();
                if col < to || !(ch == ' ' || ch == '\t') {
                    break;
                }
                cut = (i, col);
            }
            if cut.0 == at {
                return None;
            }
            new_c0.get_or_insert(cut.1);
            Some(format!("{}{}", &t[..cut.0], &t[at..]))
        })?;
        self.detail_cache = None;
        self.edit_gen += 1;
        self.search_stale |= self.search.is_some();
        let Some(nc0) = new_c0 else { return Ok(()) };
        let shift = |c: usize| c + nc0 - c0;
        let new = ColSel { a: (cs.a.0, shift(cs.a.1)), b: (cs.b.0, shift(cs.b.1)) };
        self.colsel = Some(new);
        let _ = c1;
        let t = self.line(new.b.0);
        let p = Pos::new(new.b.0, col_from_disp(&t, new.b.1));
        self.select(p, p);
        Ok(())
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
            return Err(t!("El embellecedor está disponible para JSON y XML/HTML (el tipo se cambia en la barra de estado)").into());
        };
        let (a, b) = self.selection().unwrap_or((Pos::default(), self.end_pos()));
        let text = self
            .text(a, b, MAX_COPY_BYTES)
            .ok_or_else(|| tf!("Demasiado grande para embellecer (máx. {human_size})", human_size = human_size(MAX_COPY_BYTES as u64)))?;
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
                return Err(tf!("{kind} mal formado en la línea {line_1}, columna {c}: {msg}", kind = kind, line_1 = fmt_num(p.line + 1), c = c, msg = e.msg));
            }
        };
        if text.ends_with('\n') && !out.ends_with('\n') {
            out.push('\n');
        }
        if out == text {
            return Ok(tf!("{kind} bien formado; ya estaba embellecido", kind = kind));
        }
        if !self.editable {
            return Ok(tf!("{kind} bien formado. Activa «Editar» (Ctrl+E) para embellecerlo", kind = kind));
        }
        self.replace_range(a, b, &out, EditKind::Other)?;
        self.select(a, a);
        self.center_on = Some(a.line);
        Ok(tf!("{kind} embellecido: {count} líneas (Ctrl+Z para deshacer)", kind = kind, count = fmt_num(out.lines().count() as u64)))
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
            return Err(t!("No hay bloque seleccionado").into());
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
                .ok_or_else(|| tf!("Demasiado texto para el portapapeles (> {human_size})", human_size = human_size(MAX_COPY_BYTES as u64)))?;
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
            // Con un bloque, Tab mueve su texto (ver `block_indent`).
            Key::Tab if !m.command && !m.ctrl && self.colsel.is_some_and(|c| !c.is_empty() || c.lines().0 != c.lines().1) => {
                let cs = self.colsel.unwrap();
                return self.block_indent(cs, shift);
            }
            // Con texto seleccionado, Tab desplaza todas sus líneas; Shift+Tab las devuelve.
            Key::Tab if !m.command && !m.ctrl && (shift || self.selection().is_some()) => return self.indent(shift),
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
                // Pulsar solo una tecla modificadora (Cmd, Mayús, Opción, Ctrl…) no
                // hace nada: egui también las envía como teclas, y antes deshacían
                // la selección (p. ej. al ir a pulsar Cmd+C).
                Event::Key { key, .. } if is_modifier_key(key) => Ok(()),
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
                    Some(cs) => self.cut_block(ctx, cs).map(|m| msg = Some(m)),
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
                            Some(cs) => self.cut_block(ctx, cs).map(|m| msg = Some(m)),
                            None => self.cut(ctx).map(|m| msg = Some(m)),
                        },
                        _ => {
                            ctx.send_viewport_cmd(ViewportCommand::RequestPaste);
                            Ok(())
                        }
                    }
                }
                // Tab / Mayús+Tab con un bloque (o un cursor de columna en varias
                // líneas): mueve el texto del bloque.
                Event::Key { key: Key::Tab, pressed: true, modifiers, .. }
                    if !modifiers.ctrl && !modifiers.command && self.colsel.is_some_and(|c| !c.is_empty() || c.lines().0 != c.lines().1) =>
                {
                    let cs = self.colsel.unwrap();
                    self.block_indent(cs, modifiers.shift)
                }
                // Suprimir / Retroceso con un bloque: lo borra.
                Event::Key { key: Key::Delete | Key::Backspace, pressed: true, .. } if self.block().is_some() => {
                    let cs = self.block().unwrap();
                    self.delete_block(cs)
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

/// Teclas que solo modifican a otras.
fn is_modifier_key(k: Key) -> bool {
    matches!(
        k,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
            | Key::CapsLock
            | Key::NumLock
            | Key::ScrollLock
    )
}

/// Todos los atajos, con los nombres de tecla de la plataforma.
fn shortcut_list() -> Vec<(&'static str, Vec<(String, &'static str)>)> {
    let c = if MAC { "Cmd" } else { "Ctrl" };
    let alt = if MAC { t!("Opción") } else { "Alt" };
    let word = if MAC { t!("Opción") } else { "Ctrl" };
    let k = |s: &str| s.replace("C+", &format!("{c}+")).replace("A+", &format!("{alt}+")).replace("W+", &format!("{word}+"));
    let mut edit = vec![
        (k("C+Z"), t!("Deshacer")),
        (k("C+Shift+Z"), t!("Rehacer")),
        (k("C+C"), t!("Copiar (sin selección: la línea entera)")),
        (k("C+X"), t!("Cortar (sin selección: la línea entera)")),
        (k("C+V"), t!("Pegar")),
        (k("C+A"), t!("Seleccionar todo")),
        (k("C+Shift+B"), t!("Embellecer JSON / XML (valida y sangra)")),
        (k("C+A+Z"), t!("Ajuste de línea (activar / desactivar)")),
        (k("C+R"), t!("Recargar el archivo del disco (cursor al final)")),
        (k("C+Shift+L"), t!("Vista filtrada (solo las líneas que cumplen las reglas)")),
        (k("C+Shift+O"), t!("Buscar en carpeta (todos sus archivos)")),
        (k("C+A+I"), t!("Mostrar espacios y tabuladores (· y →)")),
        (k(t!("Retroceso / Supr")), t!("Borrar carácter")),
        (k(t!("W+Retroceso / W+Supr")), t!("Borrar palabra")),
        (k("Enter"), t!("Nueva línea")),
        (k(t!("Tab")), t!("Insertar tabulador · con texto seleccionado: desplazar sus líneas")),
        (k("Shift+Tab"), t!("Quitar sangría a las líneas seleccionadas (o a la del cursor)")),
    ];
    if !MAC {
        edit.insert(2, (k("C+Y"), t!("Rehacer")));
    }
    let (line_start, line_end, doc_start, doc_end) = if MAC {
        (t!("Cmd+← / Inicio"), t!("Cmd+→ / Fin"), t!("Cmd+↑ / Cmd+Inicio"), t!("Cmd+↓ / Cmd+Fin"))
    } else {
        (t!("Inicio"), t!("Fin"), "Ctrl+Inicio", "Ctrl+Fin")
    };
    vec![
        (
            t!("Archivos"),
            vec![
                (k("C+N"), t!("Archivo nuevo (para pegar y guardar lo copiado)")),
                (k("C+O"), t!("Abrir (también arrastrando archivos)")),
                (k("C+S"), t!("Guardar")),
                (k("C+Shift+S"), t!("Guardar como…")),
                (k("C+W"), t!("Cerrar archivo")),
                ("Ctrl+Tab".into(), t!("Siguiente archivo")),
                ("Ctrl+Shift+Tab".into(), t!("Archivo anterior")),
                (k("C+A+→ / C+A+←"), t!("Archivo siguiente / anterior")),
                (k("C+B"), t!("Mostrar/ocultar panel de archivos")),
                (k("C+E"), t!("Activar/desactivar edición")),
                (t!("Clic derecho en un archivo").into(), t!("Abrir en ventana nueva, mostrar en carpeta, copiar ruta, duplicar…")),
            ],
        ),
        (
            t!("Buscar"),
            vec![
                (k("C+F"), t!("Ir a la caja de búsqueda")),
                (t!("Enter / F3").into(), t!("Buscar / siguiente coincidencia")),
                (t!("Shift+Enter / Shift+F3").into(), t!("Coincidencia anterior")),
                (if MAC { "Cmd+Opción+F".into() } else { "Ctrl+H".to_string() }, t!("Reemplazar")),
                (k("C+Shift+F"), t!("Nueva ventana de búsqueda avanzada")),
                (k("C+Enter"), t!("Buscar (búsqueda avanzada multilínea)")),
                (k("C+G"), t!("Ir a línea")),
            ],
        ),
        (t!("Edición"), edit),
        (
            t!("Moverse y seleccionar"),
            vec![
                ("← → ↑ ↓".into(), t!("Carácter / línea")),
                (k("W+← / W+→"), t!("Palabra")),
                (line_start.into(), t!("Inicio de línea (tras la sangría; otra vez: columna 1)")),
                (line_end.into(), t!("Fin de línea")),
                (doc_start.into(), t!("Inicio del archivo")),
                (doc_end.into(), t!("Fin del archivo")),
                (t!("RePág / AvPág").into(), t!("Página arriba / abajo")),
                (t!("Shift + movimiento").into(), t!("Seleccionar")),
                ("Esc".into(), t!("Quitar la selección")),
            ],
        ),
        (
            t!("Ratón"),
            vec![
                (t!("Clic / arrastrar").into(), t!("Colocar el cursor / seleccionar")),
                ("Shift+clic".into(), t!("Extender la selección")),
                (t!("Doble clic").into(), t!("Seleccionar palabra")),
                (k("A+arrastrar"), t!("Seleccionar un bloque (rectángulo); siempre con el botón ▦ Bloque")),
                (t!("Mayús+flechas (▦ Bloque)").into(), t!("Ampliar el bloque")),
                (t!("Clic en nº de línea").into(), t!("Seleccionar la línea entera")),
                (t!("Clic derecho").into(), t!("Copiar, cortar, pegar, ver línea completa…")),
                (t!("Clic en el minimapa").into(), t!("Ir al bloque más cercano (búsqueda avanzada)")),
            ],
        ),
        (
            t!("Vista"),
            vec![
                (k("C++ / C+-"), t!("Letra más grande / más pequeña")),
                ("F1".into(), t!("Esta lista de atajos")),
            ],
        ),
    ]
}

enum DialogKind {
    Open,
    /// Carpeta para el mapa de disco.
    Folder,
    /// Carpeta para «Buscar en carpeta».
    SearchFolder,
    /// Guardar como: ruta actual de la pestaña (la identifica aunque cambie de sitio).
    SaveAs(PathBuf),
}

/// Diálogo de archivo asíncrono esperando en un hilo; `result` = Some(respuesta)
/// al terminar (respuesta None si se canceló).
struct PendingDialog {
    kind: DialogKind,
    result: Arc<Mutex<Option<Option<Vec<PathBuf>>>>>,
}

impl PendingDialog {
    /// `fut` debe crearse en el hilo principal (rfd abre ahí la hoja); aquí solo
    /// se espera su respuesta en otro hilo.
    fn spawn(ctx: &Context, kind: DialogKind, fut: impl std::future::Future<Output = Option<Vec<PathBuf>>> + Send + 'static) -> Self {
        let result = Arc::new(Mutex::new(None));
        let (r, ctx) = (result.clone(), ctx.clone());
        std::thread::spawn(move || {
            *r.lock().unwrap() = Some(block_on(fut));
            ctx.request_repaint();
        });
        PendingDialog { kind, result }
    }
}

/// Ejecutor mínimo: espera un futuro aparcando el hilo.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

/// Ajuste de línea (soft wrap) de la vista principal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wrap {
    Off,
    /// Al ancho de la ventana.
    Window,
    /// A un número fijo de columnas.
    Columns(u16),
}

impl Wrap {
    pub fn parse(v: &str) -> Wrap {
        match v {
            "ventana" => Wrap::Window,
            v => v.strip_prefix("columnas:").and_then(|n| n.parse().ok()).map_or(Wrap::Off, |n: u16| Wrap::Columns(n.clamp(20, 1000))),
        }
    }
    pub fn as_str(self) -> String {
        match self {
            Wrap::Off => "no".into(),
            Wrap::Window => "ventana".into(),
            Wrap::Columns(n) => format!("columnas:{n}"),
        }
    }
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
    /// Clic en una fila: normal selecciona; Cmd/Ctrl añade/quita de la
    /// selección múltiple; Shift selecciona el rango.
    Click(usize, Modifiers),
    /// Cerrar los archivos seleccionados (o solo este si no está entre ellos).
    CloseSel(usize),
    /// Quitar la selección múltiple.
    Unpick,
    Close(usize),
    CloseOthers(usize),
    /// Mover el archivo a un espacio (`None` = fuera de los espacios).
    ToSpace(usize, Option<u32>),
    /// Arrastrar: el archivo pasa al espacio y a la posición del archivo destino.
    MoveBefore(usize, usize),
    /// Crear un espacio (con este archivo dentro, si hay).
    NewSpace(Option<usize>),
    ToggleSpace(u32),
    /// Arrastrar un espacio: pasa al lugar del otro (antes o después según la dirección).
    MoveSpace(u32, u32),
    RenameSpace(u32),
    /// Quitar el espacio; sus archivos siguen abiertos, fuera de espacios.
    DeleteSpace(u32),
    /// Cerrar los archivos del espacio (los que tienen cambios se quedan).
    CloseSpace(u32),
}

/// Grupo de archivos del panel lateral.
#[derive(Clone)]
struct Space {
    id: u32,
    name: String,
    open: bool,
}

pub(crate) fn reveal_label() -> &'static str {
    if MAC {
        t!("Mostrar en Finder")
    } else if cfg!(windows) {
        t!("Mostrar en el Explorador")
    } else {
        t!("Abrir carpeta contenedora")
    }
}

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
        .map(|n| dir.join(if n == 1 { tf!("{stem} copia{ext}", stem = stem, ext = ext) } else { tf!("{stem} copia {n}{ext}", stem = stem, n = n, ext = ext) }))
        .find(|p| !p.exists())
        .unwrap()
}

impl Drop for Tab {
    fn drop(&mut self) {
        if self.untitled {
            let _ = std::fs::remove_file(&self.doc.path);
            let _ = std::fs::remove_dir(untitled_dir()); // solo si quedó vacía
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
    // Si quedó vacía, se quita la carpeta (falla sola si aún tiene archivos).
    let _ = std::fs::remove_dir(untitled_dir());
}

#[derive(Clone, Copy)]
enum Confirm {
    CloseTab(usize),
    Quit,
    /// Recargar del disco un archivo con cambios sin guardar.
    Reload(usize),
}

pub struct App {
    /// Para pedir repintado desde hilos (diálogos de archivo).
    ctx: Context,
    /// Diálogo de abrir/guardar en curso (solo uno a la vez).
    dialog: Option<PendingDialog>,
    /// Ventana «Mapa de disco».
    map: crate::diskmap::DiskMap,
    /// Ventana «Buscar en carpeta».
    folder: crate::foldersearch::FolderSearch,
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
    /// Ajuste de línea y el último modo usado (para el botón de activar/desactivar).
    wrap: Wrap,
    wrap_last: Wrap,
    /// Bordes de los paneles (0 normales, 1 marcados, 2 muy marcados).
    borders: u8,
    /// Idioma elegido en Ajustes (`None`: el del sistema).
    lang_chosen: Option<crate::i18n::Lang>,
    /// Minimapa del texto a la derecha.
    show_minimap: bool,
    /// Siguiente número para «Sin título N».
    next_untitled: u32,
    /// Espacios del panel de archivos (grupos que se abren y cierran como carpetas).
    spaces: Vec<Space>,
    next_space: u32,
    /// Espacio cuyo nombre se está editando y el texto.
    renaming: Option<(u32, String)>,
    /// Selección múltiple en el panel de archivos (rutas) y desde dónde se
    /// extiende con Shift; orden de las filas visibles del último fotograma.
    picked: Vec<PathBuf>,
    /// Zonas del panel que pertenecen a cada espacio (cabecera y sus filas),
    /// del último fotograma, y el espacio bajo el puntero al arrastrar desde fuera.
    drop_zones: Vec<(Rect, u32)>,
    os_drop_space: Option<u32>,
    pick_anchor: Option<PathBuf>,
    row_order: Vec<usize>,
    /// Guardar/recuperar la sesión (no en ventanas nuevas ni en pruebas).
    use_session: bool,
    /// Última sesión guardada y cuándo se miró por última vez.
    session_saved: crate::settings::Session,
    session_checked: Instant,
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
    /// «Reiniciar ahora»: versión nueva que se abre cuando esta termine (`on_exit`).
    relaunch: Option<PathBuf>,
    debug_frames: u64,
    debug_typed: bool,
    #[cfg(target_os = "macos")]
    menu: crate::macmenu::MacMenu,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        let settings = Settings::load();
        TAB_WIDTH.store(settings.tab_width, Ordering::Relaxed);
        buffer::set_max_undo(settings.undo_steps);
        SHOW_WS.store(settings.show_ws || std::env::var("BUSCADOR_ESPACIOS_VER").is_ok(), Ordering::Relaxed);
        // Idioma: el elegido (ajustes o instalador), si no el del sistema, si no español.
        let chosen = settings.lang.as_deref().and_then(crate::i18n::Lang::from_code);
        crate::i18n::set(chosen.or_else(crate::i18n::system).unwrap_or(crate::i18n::Lang::Es));
        // Pruebas: BUSCADOR_IDIOMA=en|fr|es fuerza el idioma (sin guardarlo).
        if let Some(l) = std::env::var("BUSCADOR_IDIOMA").ok().and_then(|v| crate::i18n::Lang::from_code(&v)) {
            crate::i18n::set(l);
        }
        clean_old_untitled();
        let fallbacks = crate::fonts::load_fallbacks();
        cc.egui_ctx.set_fonts(crate::fonts::build(&settings.font, &fallbacks).0);
        cc.egui_ctx.set_theme(settings.theme);
        apply_borders(&cc.egui_ctx, settings.borders);
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
            wrap: Wrap::parse(&settings.wrap),
            borders: settings.borders,
            lang_chosen: chosen,
            show_minimap: settings.minimap,
            wrap_last: match Wrap::parse(&settings.wrap) {
                Wrap::Off => Wrap::Window,
                w => w,
            },
            next_untitled: 1,
            spaces: Vec::new(),
            next_space: 1,
            renaming: None,
            picked: Vec::new(),
            drop_zones: Vec::new(),
            os_drop_space: None,
            pick_anchor: None,
            row_order: Vec::new(),
            use_session: false,
            session_saved: Default::default(),
            session_checked: Instant::now(),
            fallback_fonts: fallbacks,
            show_settings: false,
            show_keys: false,
            settings_was_open: false,
            updater: None,
            ctx: {
                #[cfg(target_os = "macos")]
                crate::macopen::set_context(&cc.egui_ctx);
                cc.egui_ctx.clone()
            },
            dialog: None,
            map: Default::default(),
            folder: Default::default(),
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
            relaunch: None,
            debug_frames: 0,
            debug_typed: false,
            #[cfg(target_os = "macos")]
            menu: crate::macmenu::MacMenu::new(&cc.egui_ctx),
        };
        // Sesión anterior: se recupera salvo en ventanas nuevas («--sin-sesion»)
        // o en el modo de pruebas.
        let testing = std::env::var_os("BUSCADOR_SCREENSHOT").is_some() && std::env::var_os("BUSCADOR_SESION").is_none();
        let no_session = files.iter().any(|f| f.as_os_str() == "--sin-sesion") || testing;
        app.use_session = !no_session;
        if app.use_session {
            app.restore_session();
        }
        for f in files.into_iter().filter(|f| f.as_os_str() != "--sin-sesion") {
            app.open(f);
        }
        app
    }

    /// Reabre los archivos de la sesión anterior, con su posición y encoding.
    fn restore_session(&mut self) {
        let s = crate::settings::Session::load();
        let mut missing = Vec::new();
        for f in &s.files {
            if !f.path.exists() {
                missing.push(f.path.display().to_string());
                continue;
            }
            let before = self.tabs.len();
            self.open(f.path.clone());
            if self.tabs.len() > before {
                let t = self.tabs.last_mut().unwrap();
                if let Some(enc) = encoding_rs::Encoding::for_label(f.encoding.as_bytes()) {
                    if enc != t.doc.src.encoding {
                        t.doc.set_encoding(enc);
                    }
                }
                t.restore = Some((f.top, Pos::new(f.line, f.col)));
                t.space = f.space.map(|n| n as u32 + 1);
                if let Some(w) = &f.wrap {
                    t.wrap = Wrap::parse(w);
                }
            }
        }
        // Ids de espacio = índice + 1 (los archivos ya se asignaron así).
        self.spaces = s.spaces.iter().enumerate().map(|(n, (name, open))| Space { id: n as u32 + 1, name: name.clone(), open: *open }).collect();
        self.next_space = self.spaces.len() as u32 + 1;
        if !self.tabs.is_empty() {
            self.active = s.active.min(self.tabs.len() - 1);
        }
        self.session_saved = self.current_session();
        if !missing.is_empty() {
            self.msg(tf!("No se encontró (no se reabrió): {join}", join = missing.join(", ")));
        }
    }

    fn current_session(&self) -> crate::settings::Session {
        let files: Vec<_> = self
            .tabs
            .iter()
            .filter(|t| !t.untitled)
            .map(|t| {
                // Si aún no se aplicó la posición recuperada, se conserva.
                let (top, c) = t.restore.unwrap_or((t.top, t.cursor.unwrap_or_default()));
                crate::settings::SessionFile {
                    path: t.doc.path.clone(),
                    top,
                    line: c.line,
                    col: c.col,
                    encoding: t.doc.src.encoding.name().to_string(),
                    space: t.space.and_then(|id| self.spaces.iter().position(|s| s.id == id)),
                    wrap: Some(t.wrap.as_str()),
                }
            })
            .collect();
        let active = self.tabs.iter().take(self.active).filter(|t| !t.untitled).count();
        let spaces = self.spaces.iter().map(|s| (s.name.clone(), s.open)).collect();
        crate::settings::Session { active: active.min(files.len().saturating_sub(1)), files, spaces }
    }

    /// Guarda la sesión si cambió (como mucho cada 2 s, o ya si `now`).
    fn save_session(&mut self, now: bool) {
        if !self.use_session || (!now && self.session_checked.elapsed() < Duration::from_secs(2)) {
            return;
        }
        self.session_checked = Instant::now();
        let cur = self.current_session();
        if cur != self.session_saved {
            if let Err(e) = cur.save() {
                self.msg(tf!("No se pudo guardar la sesión: {e}", e = e));
            }
            self.session_saved = cur;
        }
    }

    fn open(&mut self, path: PathBuf) {
        if let Some(i) = self.tabs.iter().position(|t| t.doc.path == path) {
            self.active = i;
            return;
        }
        match Document::open(&path) {
            Ok(doc) => {
                let mut t = Tab::new(doc);
                t.wrap = self.wrap;
                self.tabs.push(t);
                self.active = self.tabs.len() - 1;
                self.focus_view = true;
            }
            Err(e) => self.msg(tf!("No se pudo abrir {display}: {e}", display = path.display(), e = e)),
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
                doc.name = tf!("Sin título {n}", n = n);
                let mut t = Tab::new(doc);
                t.untitled = true;
                t.editable = true;
                t.pending_text = text.filter(|s| !s.is_empty());
                t.wrap = self.wrap;
                self.tabs.push(t);
                self.active = self.tabs.len() - 1;
                self.focus_view = true;
            }
            Err(e) => self.msg(tf!("No se pudo crear el archivo nuevo: {e}", e = e)),
        }
    }

    /// Diálogo de abrir. Es asíncrono: en macOS el modal bloqueante de rfd
    /// (`runModal`) dentro del fotograma hace que winit reentre al llegar
    /// eventos (p. ej. gestos del trackpad) y la app aborta.
    fn open_dialog(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let fut = rfd::AsyncFileDialog::new().set_title("Abrir archivos").pick_files();
        self.dialog = Some(PendingDialog::spawn(&self.ctx, DialogKind::Open, async move {
            fut.await.map(|v| v.into_iter().map(|h| h.path().to_path_buf()).collect())
        }));
    }

    /// Recoge el resultado del diálogo abierto, si ya terminó.
    fn poll_dialog(&mut self) {
        let Some(done) = self.dialog.as_ref().and_then(|d| d.result.lock().unwrap().take()) else { return };
        let kind = self.dialog.take().unwrap().kind;
        let Some(paths) = done else { return };
        match kind {
            DialogKind::Open => {
                for f in paths {
                    self.open(f);
                }
            }
            DialogKind::SearchFolder => {
                if let Some(dir) = paths.into_iter().next() {
                    self.folder.dir = dir.display().to_string();
                    self.folder.open = true;
                }
            }
            DialogKind::Folder => {
                if let Some(dir) = paths.into_iter().next() {
                    let ctx = self.ctx.clone();
                    self.map.start_scan(dir, &ctx);
                }
            }
            DialogKind::SaveAs(src) => {
                let (Some(target), Some(t)) = (paths.into_iter().next(), self.tabs.iter_mut().find(|t| t.doc.path == src)) else {
                    return;
                };
                if let Err(e) = t.start_save(target) {
                    self.msg(e);
                }
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
            wrap: self.wrap.as_str(),
            borders: self.borders,
            lang: self.lang_chosen.map(|l| l.code().to_string()),
            minimap: self.show_minimap,
            tab_width: tab_width(),
            show_ws: show_whitespace(),
            undo_steps: buffer::max_undo(),
        };
        if let Err(e) = st.save() {
            self.msg(tf!("No se pudieron guardar los ajustes: {e}", e = e));
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

    /// Archivos a los que se aplica una acción sobre la fila `i`: todos los
    /// seleccionados si `i` es uno de ellos, si no solo `i`.
    fn targets(&self, i: usize) -> Vec<usize> {
        let Some(t) = self.tabs.get(i) else { return Vec::new() };
        if self.picked.len() > 1 && self.picked.contains(&t.doc.path) {
            (0..self.tabs.len()).filter(|&j| self.picked.contains(&self.tabs[j].doc.path)).collect()
        } else {
            vec![i]
        }
    }

    /// Clic en una fila del panel de archivos.
    fn click_row(&mut self, i: usize, m: Modifiers) {
        let Some(path) = self.tabs.get(i).map(|t| t.doc.path.clone()) else { return };
        if m.shift {
            // Rango (en el orden en que se ven) desde el ancla o el activo.
            let anchor = self.pick_anchor.as_ref().and_then(|p| self.tabs.iter().position(|t| &t.doc.path == p)).unwrap_or(self.active);
            let (a, b) = (self.row_order.iter().position(|&j| j == anchor), self.row_order.iter().position(|&j| j == i));
            if let (Some(a), Some(b)) = (a, b) {
                if !m.command {
                    self.picked.clear();
                }
                for &j in &self.row_order[a.min(b)..=a.max(b)] {
                    let p = self.tabs[j].doc.path.clone();
                    if !self.picked.contains(&p) {
                        self.picked.push(p);
                    }
                }
            }
            self.active = i;
        } else if m.command {
            // Añadir/quitar (al empezar, el activo también cuenta).
            if self.picked.is_empty() {
                if let Some(t) = self.tabs.get(self.active) {
                    self.picked.push(t.doc.path.clone());
                }
            }
            if let Some(k) = self.picked.iter().position(|p| *p == path) {
                self.picked.remove(k);
            } else {
                self.picked.push(path.clone());
                self.active = i;
            }
            self.pick_anchor = Some(path);
        } else {
            self.picked.clear();
            self.pick_anchor = Some(path);
            self.active = i;
        }
    }

    /// «En vivo»: mira los archivos que se siguen; si crecieron, muestra lo
    /// nuevo (y busca/filtra solo eso); si se truncaron o rotaron, los reabre.
    fn follow_files(&mut self, ctx: &Context) {
        let mut reopen = Vec::new();
        let mut quiet = Vec::new();
        let mut any = false;
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if !t.follow || t.untitled {
                continue;
            }
            any = true;
            // Con cambios sin guardar se pausa: el texto editado manda. Si ya se
            // guardaron, se reabre el archivo para seguir leyendo de él.
            if t.follow_at.elapsed() < FOLLOW_EVERY || t.dirty() || t.save.is_some() {
                continue;
            }
            if t.buf.is_some() {
                quiet.push(i);
                continue;
            }
            t.follow_at = Instant::now();
            let old_total = t.total();
            let stick = t.cursor.is_none_or(|c| c.line + 1 >= old_total);
            match t.doc.check_growth() {
                Ok(crate::document::Change::Grew { from_line, from_byte }) => {
                    if let Some(s) = &t.search {
                        let next = if s.is_done() {
                            s.extend(t.doc.src.clone(), from_line, from_byte)
                        } else {
                            Search::start(t.doc.src.clone(), None, s.params.clone())
                        };
                        if let Ok(n) = next {
                            t.search = Some(n);
                        }
                    }
                    t.edit_gen += 1;
                    t.filter.extend(&t.doc.src, from_line, from_byte, t.edit_gen);
                    t.detail_cache = None;
                    t.syntax_cache = None;
                    if stick {
                        t.goto_end = true;
                    }
                }
                Ok(crate::document::Change::Replaced) => reopen.push(i),
                // Puede no existir un momento mientras rota: se reintenta.
                Ok(crate::document::Change::Same) | Err(_) => {}
            }
        }
        for i in quiet {
            self.reload(i, true);
        }
        for i in reopen {
            let name = self.tabs[i].doc.name.clone();
            self.reload(i, true);
            self.msg(tf!("{name} se truncó o se reemplazó: se volvió a abrir", name = name));
        }
        if any {
            ctx.request_repaint_after(FOLLOW_EVERY);
        }
    }

    /// Pasa al archivo anterior (-1) o siguiente (+1) en el orden en que se ven
    /// en el panel (los de espacios cerrados se saltan); da la vuelta.
    fn cycle_tab(&mut self, dir: i64) {
        let n = self.tabs.len();
        if n == 0 {
            return;
        }
        let order: Vec<usize> = if self.row_order.len() > 1 && self.row_order.iter().all(|&i| i < n) {
            self.row_order.clone()
        } else {
            (0..n).collect()
        };
        let pos = order.iter().position(|&i| i == self.active).unwrap_or(0) as i64;
        let next = (pos + dir).rem_euclid(order.len() as i64) as usize;
        self.active = order[next];
        self.focus_view = true;
    }

    /// Cierra los archivos que cumplen `pred` salvo los que tienen cambios sin
    /// guardar (o se están guardando); conserva el activo si sigue abierto.
    fn close_many(&mut self, pred: impl Fn(&Tab) -> bool) {
        let active = self.tabs.get(self.active).map(|t| t.doc.path.clone());
        let mut kept = false;
        self.tabs.retain(|t| {
            let close = pred(t);
            let busy = t.dirty() || t.save.is_some();
            kept |= close && busy;
            !close || busy
        });
        self.active = self.tabs.iter().position(|t| Some(&t.doc.path) == active.as_ref()).unwrap_or(0);
        if kept {
            self.msg(t!("Los archivos con cambios sin guardar no se cerraron"));
        }
    }

    /// Vuelve a leer del disco el archivo `i` (con el mismo encoding, ajuste
    /// y modo de edición) y deja el cursor al final. Con cambios sin guardar
    /// pregunta antes, salvo `force`.
    fn reload(&mut self, i: usize, force: bool) {
        let Some(t) = self.tabs.get(i) else { return };
        if t.untitled {
            return;
        }
        if t.save.is_some() {
            self.msg(t!("Espera a que termine de guardarse"));
            return;
        }
        if !force && t.dirty() {
            self.confirm = Some(Confirm::Reload(i));
            return;
        }
        let (path, enc) = (t.doc.path.clone(), t.doc.src.encoding);
        let mut doc = match Document::open(&path) {
            Ok(d) => d,
            Err(e) => {
                self.msg(tf!("No se pudo recargar {display}: {e}", display = path.display(), e = e));
                return;
            }
        };
        if doc.src.encoding != enc {
            doc.set_encoding(enc);
        }
        let old = &self.tabs[i];
        let mut t = Tab::new(doc);
        t.wrap = old.wrap;
        t.editable = old.editable;
        t.lang = old.lang;
        t.space = old.space;
        t.follow = old.follow;
        t.filter.rules = old.filter.rules.clone();
        t.filter.on = old.filter.on;
        t.goto_end = true;
        // La búsqueda que hubiera se repite sobre el contenido nuevo (sin saltar a la primera).
        if let Some(params) = old.search.as_ref().map(|s| s.params.clone()) {
            if let Ok(s) = Search::start(t.doc.src.clone(), None, params) {
                t.search = Some(s);
            }
        }
        let size = human_size(t.doc.len() as u64);
        self.tabs[i] = t;
        self.focus_view = true;
        self.msg(tf!("Recargado del disco ({size})", size = size));
    }

    /// Ajuste de línea del archivo activo.
    fn cur_wrap(&self) -> Wrap {
        self.tabs.get(self.active).map_or(Wrap::Off, |t| t.wrap)
    }

    /// Activa/desactiva el ajuste de línea solo en el archivo seleccionado.
    fn toggle_wrap(&mut self) {
        let last = self.wrap_last;
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        t.wrap = if t.wrap == Wrap::Off { last } else { Wrap::Off };
        t.content_width = 0.0;
        t.colsel = None;
        let w = t.wrap;
        self.msg(match w {
            Wrap::Off => t!("Ajuste de línea desactivado").to_string(),
            Wrap::Window => t!("Ajuste de línea al ancho de la ventana").to_string(),
            Wrap::Columns(n) => tf!("Ajuste de línea a {n} columnas", n = n),
        });
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
                tf!("Edición activada. Las líneas que cambies se guardan en RAM; al guardar se escribe una copia completa ({len}) y se renombra", len = human_size(t.doc.len() as u64))
            } else {
                t!("Edición activada").to_string()
            }
        } else {
            t!("Solo lectura").to_string()
        };
        self.msg(m);
        self.focus_view = true;
    }

    fn save(&mut self, save_as: bool) {
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        if save_as || t.untitled {
            if self.dialog.is_some() {
                return;
            }
            let name = if t.untitled { format!("{}.txt", t.doc.name) } else { t.doc.name.clone() };
            let mut d = rfd::AsyncFileDialog::new().set_title("Guardar como").set_file_name(name);
            if let Some(dir) = t.doc.path.parent().filter(|_| !t.untitled) {
                d = d.set_directory(dir);
            }
            let fut = d.save_file();
            let kind = DialogKind::SaveAs(t.doc.path.clone());
            self.dialog = Some(PendingDialog::spawn(&self.ctx, kind, async move { fut.await.map(|h| vec![h.path().to_path_buf()]) }));
            return;
        }
        let target = t.doc.path.clone();
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
        if pressed(sc(Modifiers::COMMAND | Modifiers::ALT, Key::Z)) {
            self.toggle_wrap();
        }
        if pressed(sc(Modifiers::COMMAND, Key::R)) {
            self.reload(self.active, false);
        }
        if pressed(sc(Modifiers::COMMAND | Modifiers::ALT, Key::I)) {
            SHOW_WS.store(!show_whitespace(), Ordering::Relaxed);
            for t in &mut self.tabs {
                t.syntax_cache = None;
            }
            self.persist();
        }
        if pressed(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::O)) {
            self.show_folder_search();
        }
        if pressed(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::L)) {
            let (q, re, cs) = (self.query.clone(), self.use_regex, self.case_sensitive);
            if let Some(t) = self.tabs.get_mut(self.active) {
                toggle_filter(t, &q, re, cs);
            }
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
        // Archivo anterior / siguiente (Cmd+Shift+Tab lo reserva macOS para las apps).
        if pressed(sc(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab)) || pressed(sc(Modifiers::COMMAND | Modifiers::ALT, Key::ArrowLeft)) {
            self.cycle_tab(-1);
        } else if pressed(sc(Modifiers::CTRL, Key::Tab)) || pressed(sc(Modifiers::COMMAND | Modifiers::ALT, Key::ArrowRight)) {
            self.cycle_tab(1);
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
        let narrow = full.width() < 1150.0;
        let right_w = if narrow { 235.0 } else { 400.0 };
        let enc_w = if narrow { 60.0 } else { 150.0 };
        let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);
        let left = Rect::from_min_max(full.min, pos2(right.left() - 8.0, full.bottom()));
        // --- A la derecha (se reserva primero): encoding y ajustes ---
        ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
            if ui
                .selectable_label(self.show_settings, "⚙")
                .on_hover_text(if MAC { t!("Ajustes: tema y tamaño de letra (Cmd+,)") } else { t!("Ajustes: tema y tamaño de letra") })
                .clicked()
            {
                self.show_settings = !self.show_settings;
            }
            let mut msg = None;
            if let Some(t) = self.tabs.get_mut(self.active) {
                let mut enc = t.doc.src.encoding;
                ComboBox::from_id_salt("encoding")
                    .selected_text(encoding::label(enc))
                    .width(enc_w)
                    .truncate()
                    .show_ui(ui, |ui| {
                        for e in encoding::available() {
                            ui.selectable_value(&mut enc, e, encoding::label(e));
                        }
                    })
                    .response
                    .on_hover_text(t!("Encoding con el que se lee el archivo"));
                if enc != t.doc.src.encoding {
                    if t.dirty() || t.save.is_some() {
                        msg = Some(t!("Guarda o deshaz los cambios antes de reabrir con otro encoding"));
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
            // Tamaño y tipo de letra del texto.
            ui.separator();
            let mut size = self.font_size;
            if !narrow && ui.small_button("A+").on_hover_text(t!("Letra más grande (Ctrl++)")).clicked() {
                size += 1.0;
            }
            ui.add(DragValue::new(&mut size).range(7.0..=40.0).speed(0.2).max_decimals(0))
                .on_hover_text(t!("Tamaño de letra del texto (arrastra o escribe)"));
            if !narrow && ui.small_button("A−").on_hover_text(t!("Letra más pequeña (Ctrl+-)")).clicked() {
                size -= 1.0;
            }
            let size = size.round().clamp(7.0, 40.0);
            if size != self.font_size {
                self.set_font(size);
            }
            let mut family = self.text_font.clone();
            ComboBox::from_id_salt("tipo_letra_barra")
                .selected_text(crate::fonts::label(&family))
                .width(if narrow { 56.0 } else { 90.0 })
                .truncate()
                .show_ui(ui, |ui| {
                    let av = crate::fonts::available();
                    ui.weak(t!("Monoespaciadas (alinean columnas)"));
                    for o in av.iter().filter(|o| o.mono) {
                        ui.selectable_value(&mut family, o.name.to_string(), crate::fonts::label(o.name));
                    }
                    ui.separator();
                    ui.weak(t!("Proporcionales"));
                    for o in av.iter().filter(|o| !o.mono) {
                        ui.selectable_value(&mut family, o.name.to_string(), crate::fonts::label(o.name));
                    }
                })
                .response
                .on_hover_text(tf!("Tipo de letra del texto: {family}", family = family));
            if family != self.text_font {
                self.set_text_font(ui.ctx(), family);
            }
        });
        // Con poco espacio, los botones muestran solo el ícono (el texto queda en el tooltip).
        let compact = full.width() < 1480.0;
        let lb = |icon: &str, text: &str| if compact { icon.to_string() } else { format!("{icon} {text}") };
        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.set_clip_rect(left.intersect(ui.clip_rect()));
            // --- Archivo y edición ---
            if ui.selectable_label(self.show_sidebar, "☰").on_hover_text(t!("Panel de archivos (Ctrl+B)")).clicked() {
                self.show_sidebar = !self.show_sidebar;
            }
            if ui.button(lb("📝", t!("Nuevo"))).on_hover_text(t!("Archivo nuevo, por ejemplo para pegar lo copiado (Ctrl+N)")).clicked() {
                self.new_untitled(None);
            }
            if ui.button(lb("📂", t!("Abrir"))).on_hover_text(t!("Abrir archivos (Ctrl+O) · también puedes arrastrarlos")).clicked() {
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
            if ui.add_enabled(dirty || untitled, Button::new(lb("💾", t!("Guardar")))).on_hover_text(t!("Guardar (Ctrl+S) · Guardar como: Ctrl+Shift+S")).clicked() {
                self.save(false);
            }
            if ui.add_enabled(can_undo && editable, Button::new("↶")).on_hover_text(t!("Deshacer (Ctrl+Z)")).clicked() {
                if let Some(Err(e)) = self.tab().map(|t| t.undo_redo(false)) {
                    self.msg(e);
                }
            }
            if ui.add_enabled(can_redo && editable, Button::new("↷")).on_hover_text(t!("Rehacer (Ctrl+Shift+Z)")).clicked() {
                if let Some(Err(e)) = self.tab().map(|t| t.undo_redo(true)) {
                    self.msg(e);
                }
            }
            if ui
                .add_enabled(has_tab, Button::selectable(editable, lb("✏", t!("Editar"))))
                .on_hover_text(t!("Activar/desactivar edición (Ctrl+E)"))
                .clicked()
            {
                self.toggle_edit();
            }
            let mono = self.mono_font() && self.cur_wrap() == Wrap::Off;
            let block_hint = if self.cur_wrap() != Wrap::Off {
                t!("La selección en bloque no está disponible con el ajuste de línea activado")
            } else if mono {
                t!("Selección en bloque: arrastra para marcar un rectángulo de texto (como una matriz). Al copiar se copia solo el texto real, línea por línea. También: Opción/Alt+arrastrar.")
            } else {
                t!("La selección en bloque necesita una letra monoespaciada (⚙ Ajustes)")
            };
            if ui
                .add_enabled(mono && has_tab, Button::selectable(self.column_mode && mono, "▦".to_string()))
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
                Some(k) => tf!("Embellecer {k}: comprueba que esté bien formado y lo ordena con sangría (Ctrl+Shift+B). Se aplica a la selección o a todo el archivo; Ctrl+Z lo deshace.", k = k),
                None => t!("Embellecer: disponible para JSON y XML/HTML").to_string(),
            };
            if ui
                .add_enabled(beaut.is_some(), Button::new("✨"))
                .on_hover_text(&bhint)
                .on_disabled_hover_text(&bhint)
                .clicked()
            {
                self.beautify();
            }
            ui.separator();

            // --- Búsqueda ---
            // Lo que va detrás del buscador ocupa ~400 px; el buscador se adapta.
            let search_w = (ui.available_width() - 372.0).clamp(70.0, 320.0);
            let r = ui.add(
                TextEdit::singleline(&mut self.query)
                    .desired_width(search_w)
                    .hint_text(t!("🔍 Buscar…  (Ctrl+F, Enter)"))
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
            ui.toggle_value(&mut self.case_sensitive, "Aa").on_hover_text(t!("Distinguir mayúsculas"));
            ui.toggle_value(&mut self.use_regex, ".*").on_hover_text(t!("Expresión regular"));
            if ui.add_enabled(has_tab, Button::new("◀")).on_hover_text(t!("Anterior (Shift+Enter, Shift+F3)")).clicked() {
                if let Some(t) = self.tab() {
                    t.prev_match();
                }
            }
            if ui.add_enabled(has_tab, Button::new("▶")).on_hover_text(t!("Siguiente (Enter, F3)")).clicked() {
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
                    ui.colored_label(ui.visuals().error_fg_color, t!("regex inválida")).on_hover_text(e);
                } else if let Some(s) = &t.search {
                    // Compacto en la barra; el detalle, al pasar el ratón.
                    let n = s.count();
                    let pos = t.current.and_then(|l| s.index_of(l)).map(|i| format!("{}/", fmt_num(i as u64 + 1))).unwrap_or_default();
                    let mut txt = format!("{pos}{}", fmt_num(n as u64));
                    let mut hint = tf!("{fmt_num} líneas con coincidencias", fmt_num = fmt_num(n as u64));
                    if s.truncated.load(Ordering::Relaxed) {
                        txt += "+";
                        hint += &tf!(" (límite {fmt_num})", fmt_num = fmt_num(MAX_RESULTS as u64));
                    }
                    if !s.is_done() {
                        let p = s.progress.load(Ordering::Relaxed) as f64 / s.total.max(1) as f64;
                        txt += &format!(" · {:.0}%", p * 100.0);
                    } else {
                        hint += &format!(" · {:.2}s", s.elapsed.lock().unwrap().as_secs_f32());
                    }
                    let color = if t.search_stale { ui.visuals().warn_fg_color } else { ui.visuals().text_color() };
                    if t.search_stale {
                        hint += t!("\nDesactualizada por las ediciones: Enter para repetir");
                    }
                    ui.label(RichText::new(txt).color(color)).on_hover_text(hint);
                }
            }
            ui.separator();

            // --- Reemplazar y avanzada ---
            let rk = if MAC { "Cmd+Opción+F" } else { "Ctrl+H" };
            ui.toggle_value(&mut self.show_replace, "🔁").on_hover_text(tf!("Reemplazar ({rk})", rk = rk));
            let n_adv = self.advs.len();
            let adv_label = if n_adv > 0 { format!("🔎 {n_adv}") } else { "🔎".to_string() };
            if ui
                .selectable_label(n_adv > 0, adv_label)
                .on_hover_text(t!("Búsqueda avanzada: nueva ventana de búsqueda por bloques (desde… hasta…, multilínea), con minimapa y copia en varias líneas (Ctrl+Shift+F). Puedes abrir varias."))
                .clicked()
            {
                self.new_adv_window();
            }
            ui.toggle_value(&mut self.map.open, "🗺").on_hover_text(
                t!("Mapa de disco: qué ocupa cada carpeta y archivo de una carpeta, y búsqueda de texto en sus archivos (se resaltan en verde)"),
            );
            let fk = if MAC { "Cmd+Shift+O" } else { "Ctrl+Shift+O" };
            if ui.selectable_label(self.folder.open, "📁").on_hover_text(tf!("Buscar en carpeta: el texto en todos los archivos de una carpeta ({fk})", fk = fk)).clicked() {
                if self.folder.open {
                    self.folder.open = false;
                } else {
                    self.show_folder_search();
                }
            }

        });
        if self.show_replace {
            ui.horizontal(|ui| {
                ui.label(t!("Reemplazar por:"));
                let r = ui.add(
                    TextEdit::singleline(&mut self.replacement)
                        .desired_width(260.0)
                        .hint_text(if self.use_regex { t!("texto ($1, $2… para grupos)") } else { "texto" })
                        .font(TextStyle::Monospace),
                );
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let (repl, re) = (self.replacement.clone(), self.use_regex);
                if ui.button(t!("Reemplazar")).on_hover_text(t!("Reemplaza la coincidencia seleccionada y va a la siguiente")).clicked() || enter
                {
                    if let Some(Err(e)) = self.tab().map(|t| t.replace_current(&repl, re)) {
                        self.msg(e);
                    }
                    if enter {
                        r.request_focus();
                    }
                }
                if ui.button(t!("Reemplazar todo")).on_hover_text(t!("En todas las líneas encontradas · se deshace con Ctrl+Z")).clicked() {
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
            // Un archivo nuevo sin cambios también se puede guardar (Guardar como).
            t.is_some_and(|t| t.dirty() || t.untitled),
            t.is_some_and(|t| t.editable),
            self.show_sidebar,
            self.show_results,
            self.show_detail,
            show_whitespace(),
        ]);
    }

    fn file_action(&mut self, ctx: &Context, a: FileAction) {
        match a {
            FileAction::Reveal(i) => {
                if let Err(e) = reveal_in_file_manager(&self.tabs[i].doc.path) {
                    self.msg(tf!("No se pudo abrir la carpeta: {e}", e = e));
                }
            }
            FileAction::CopyPath(i) => {
                let p = self.tabs[i].doc.path.display().to_string();
                ctx.copy_text(p.clone());
                self.msg(tf!("Ruta copiada: {p}", p = p));
            }
            FileAction::CopyName(i) => {
                let n = self.tabs[i].doc.name.clone();
                ctx.copy_text(n.clone());
                self.msg(tf!("Nombre copiado: {n}", n = n));
            }
            FileAction::Duplicate(i) => self.duplicate(i),
            FileAction::NewWindow(i) => {
                let t = &self.tabs[i];
                let r = std::env::current_exe()
                    .and_then(|exe| std::process::Command::new(exe).arg("--sin-sesion").arg(&t.doc.path).spawn());
                let m = match r {
                    Ok(_) if t.dirty() => tf!("{name} abierto en otra ventana (versión guardada en disco)", name = t.doc.name),
                    Ok(_) => tf!("{name} abierto en otra ventana", name = t.doc.name),
                    Err(e) => tf!("No se pudo abrir otra ventana: {e}", e = e),
                };
                self.msg(m);
            }
            FileAction::Close(i) => self.close_tab(i, false),
            FileAction::Click(i, m) => self.click_row(i, m),
            FileAction::CloseSel(i) => {
                let paths: Vec<PathBuf> = self.targets(i).iter().map(|&j| self.tabs[j].doc.path.clone()).collect();
                self.close_many(|t| paths.contains(&t.doc.path));
                self.picked.clear();
            }
            FileAction::Unpick => self.picked.clear(),
            FileAction::CloseOthers(i) => {
                let keep = self.tabs[i].doc.path.clone();
                self.active = i;
                self.close_many(|t| t.doc.path != keep);
            }
            FileAction::CloseSpace(id) => self.close_many(|t| t.space == Some(id)),
            FileAction::ToSpace(i, sp) => {
                for j in self.targets(i) {
                    self.tabs[j].space = sp;
                }
                if let Some(s) = self.spaces.iter_mut().find(|s| Some(s.id) == sp) {
                    s.open = true;
                }
            }
            FileAction::MoveBefore(from, to) => {
                let moving = self.targets(from);
                if to < self.tabs.len() && !moving.contains(&to) {
                    let sp = self.tabs[to].space;
                    let target = self.tabs[to].doc.path.clone();
                    let active = self.tabs[self.active].doc.path.clone();
                    let mut taken: Vec<Tab> = moving.iter().rev().map(|&j| self.tabs.remove(j)).collect();
                    taken.reverse();
                    for t in &mut taken {
                        t.space = sp;
                    }
                    // Hacia abajo quedan después del destino; hacia arriba, antes.
                    let p = self.tabs.iter().position(|t| t.doc.path == target).unwrap_or(self.tabs.len());
                    let at = if from < to { p + 1 } else { p };
                    self.tabs.splice(at..at, taken);
                    self.active = self.tabs.iter().position(|t| t.doc.path == active).unwrap_or(0);
                }
            }
            FileAction::NewSpace(file) => {
                let id = self.next_space;
                self.next_space += 1;
                let n = self.spaces.len() + 1;
                let name = tf!("Espacio {n}", n = n);
                self.spaces.push(Space { id, name: name.clone(), open: true });
                for j in file.map(|i| self.targets(i)).unwrap_or_default() {
                    self.tabs[j].space = Some(id);
                }
                self.renaming = Some((id, name));
            }
            FileAction::MoveSpace(from, to) => {
                // Insertar en la posición del destino deja el espacio después de él
                // si se arrastró hacia abajo y antes si fue hacia arriba.
                if let (Some(a), Some(b)) = (self.spaces.iter().position(|s| s.id == from), self.spaces.iter().position(|s| s.id == to)) {
                    let sp = self.spaces.remove(a);
                    self.spaces.insert(b, sp);
                }
            }
            FileAction::ToggleSpace(id) => {
                if let Some(s) = self.spaces.iter_mut().find(|s| s.id == id) {
                    s.open = !s.open;
                }
            }
            FileAction::RenameSpace(id) => {
                if let Some(s) = self.spaces.iter().find(|s| s.id == id) {
                    self.renaming = Some((id, s.name.clone()));
                }
            }
            FileAction::DeleteSpace(id) => {
                self.spaces.retain(|s| s.id != id);
                for t in self.tabs.iter_mut().filter(|t| t.space == Some(id)) {
                    t.space = None;
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
                self.msg(tf!("Duplicando como {to_string_lossy}…", to_string_lossy = target.file_name().unwrap_or_default().to_string_lossy()));
                self.duplicating.push((target, result));
            }
            Err(e) => self.msg(tf!("No se pudo duplicar: {e}", e = e)),
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong(t!("Archivos")).on_hover_text(t!("Archivos abiertos"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("«").on_hover_text(t!("Ocultar (Ctrl+B)")).clicked() {
                    self.show_sidebar = false;
                }
                if ui.add(Button::new(RichText::new(t!("+ Espacio")).small()).small()).on_hover_text(t!("Crear un espacio: un grupo de archivos que se abre y cierra como una carpeta.\nArrastra archivos encima o usa su menú (clic derecho).")).clicked() {
                    self.file_action(ui.ctx(), FileAction::NewSpace(None));
                }
                let on = self.cur_wrap() != Wrap::Off;
                let r = ui.add_enabled(!self.tabs.is_empty(), Button::new(RichText::new(t!("↩ Ajuste")).small()).small().selected(on));
                if r.on_hover_text(t!("Ajuste de línea del archivo seleccionado (Ctrl+Alt+Z)")).clicked() {
                    self.toggle_wrap();
                }
            });
        });
        ui.separator();
        let mut action = None;
        let spaces = self.spaces.clone();
        let tabs = &self.tabs;
        self.picked.retain(|p| tabs.iter().any(|t| &t.doc.path == p));
        let mut order = Vec::new();
        let mut zones = Vec::new();
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // Primero los espacios (como carpetas) y después los archivos sueltos.
            for sp in &spaces {
                let files: Vec<usize> = (0..self.tabs.len()).filter(|&i| self.tabs[i].space == Some(sp.id)).collect();
                let has_active = files.contains(&self.active);
                let y0 = ui.cursor().top();
                let dirty = files.iter().any(|&i| self.tabs[i].dirty());
                let live = files.iter().any(|&i| self.tabs[i].follow);
                self.space_header(ui, sp, files.len(), has_active, (dirty, live), &mut action);
                if sp.open {
                    for &i in &files {
                        order.push(i);
                        file_row(ui, i, &self.tabs[i], i == self.active, self.multi(i), &spaces, 14.0, self.tabs.len(), &mut action);
                    }
                }
                let r = ui.max_rect();
                zones.push((Rect::from_x_y_ranges(r.x_range(), y0..=ui.cursor().top()).intersect(ui.clip_rect()), sp.id));
            }
            if !spaces.is_empty() && self.tabs.iter().any(|t| t.space.is_none()) {
                ui.add_space(4.0);
            }
            for i in (0..self.tabs.len()).filter(|&i| self.tabs[i].space.is_none()) {
                order.push(i);
                file_row(ui, i, &self.tabs[i], i == self.active, self.multi(i), &spaces, 0.0, self.tabs.len(), &mut action);
            }
            if self.tabs.is_empty() {
                ui.weak(t!("Sin archivos.\nUsa «Abrir…» o arrastra archivos aquí."));
            }
            // El resto del panel: soltar aquí saca el archivo de su espacio.
            let rest = ui.available_rect_before_wrap();
            let rest = Rect::from_min_size(rest.min, vec2(rest.width(), rest.height().max(24.0)));
            let r = ui.allocate_rect(rest, Sense::click());
            if r.dnd_hover_payload::<DragFile>().is_some() && !spaces.is_empty() {
                ui.painter().rect_stroke(rest.shrink(2.0), 3.0, ui.visuals().selection.stroke, StrokeKind::Inside);
                ui.painter().text(rest.center_top() + vec2(0.0, 12.0), Align2::CENTER_CENTER, t!("Soltar aquí: fuera de los espacios"), FontId::proportional(11.0), ui.visuals().weak_text_color());
            }
            if let Some(d) = r.dnd_release_payload::<DragFile>() {
                action = Some(FileAction::ToSpace(d.0, None));
            } else if r.clicked() && !self.picked.is_empty() {
                action = Some(FileAction::Unpick);
            }
        });
        self.row_order = order;
        self.drop_zones = zones;
        if let Some(a) = action {
            self.file_action(ui.ctx(), a);
        }
    }

    /// Para una fila: cuántos archivos hay en la selección múltiple si ella
    /// está incluida (0 si no o si solo hay uno).
    fn multi(&self, i: usize) -> usize {
        let n = self.picked.len();
        if n > 1 && self.picked.contains(&self.tabs[i].doc.path) { n } else { 0 }
    }

    /// Cabecera de un espacio: clic abre/cierra, doble clic o menú para renombrar.
    fn space_header(&mut self, ui: &mut Ui, sp: &Space, count: usize, has_active: bool, (dirty, live): (bool, bool), action: &mut Option<FileAction>) {
        let row = vec2(ui.available_width(), ui.spacing().interact_size.y);
        let (rect, r) = ui.allocate_exact_size(row, Sense::click_and_drag());
        r.dnd_set_drag_payload(DragSpace(sp.id));
        if r.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        let v = ui.visuals().clone();
        let dropping = r.dnd_hover_payload::<DragFile>().is_some() || self.os_drop_space == Some(sp.id);
        // Otro espacio arrastrado encima: línea donde quedará.
        if let Some(d) = r.dnd_hover_payload::<DragSpace>().filter(|d| d.0 != sp.id) {
            let pos = |id: u32| self.spaces.iter().position(|s| s.id == id).unwrap_or(0);
            let y = if pos(d.0) < pos(sp.id) { rect.bottom() } else { rect.top() };
            ui.painter().hline(rect.x_range(), y, Stroke::new(2.0, v.selection.stroke.color));
        }
        if let Some(d) = r.dnd_release_payload::<DragSpace>() {
            if d.0 != sp.id {
                *action = Some(FileAction::MoveSpace(d.0, sp.id));
            }
        }
        // Cabecera siempre con un fondo más claro que el panel (se distingue de
        // los archivos); más claro aún al pasar por encima.
        let lighter = |c: Color32, k: f32| {
            let l = |x: u8| (x as f32 + (255.0 - x as f32) * k) as u8;
            Color32::from_rgb(l(c.r()), l(c.g()), l(c.b()))
        };
        let base = lighter(v.panel_fill, if v.dark_mode { 0.12 } else { 0.0 });
        let base = if v.dark_mode { base } else { v.widgets.inactive.weak_bg_fill };
        let fill = if dropping || r.hovered() {
            Some(if v.dark_mode { lighter(v.panel_fill, 0.2) } else { v.widgets.hovered.weak_bg_fill })
        } else if has_active && !sp.open {
            Some(v.selection.bg_fill.gamma_multiply(0.5))
        } else {
            Some(base)
        };
        if let Some(f) = fill {
            ui.painter().rect_filled(rect, v.widgets.inactive.corner_radius, f);
        }
        if dropping {
            ui.painter().rect_stroke(rect, v.widgets.inactive.corner_radius, v.selection.stroke, StrokeKind::Inside);
        }
        // Triángulo pintado (no depende de la fuente).
        let c = pos2(rect.left() + 10.0, rect.center().y);
        let tri = if sp.open {
            vec![c + vec2(-4.0, -2.5), c + vec2(4.0, -2.5), c + vec2(0.0, 3.0)]
        } else {
            vec![c + vec2(-2.5, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.5, 4.0)]
        };
        ui.painter().add(Shape::convex_polygon(tri, v.text_color(), Stroke::NONE));
        let mut text_left = rect.left() + 20.0;
        if live && !self.renaming.as_ref().is_some_and(|(id, _)| *id == sp.id) {
            text_left += live_icon(ui, pos2(text_left, rect.center().y));
        }
        if self.renaming.as_ref().is_some_and(|(id, _)| *id == sp.id) {
            let edit_rect = Rect::from_min_max(pos2(text_left, rect.top() + 1.0), pos2(rect.right() - 4.0, rect.bottom() - 1.0));
            let (_, text) = self.renaming.as_mut().unwrap();
            let te = ui.put(edit_rect, TextEdit::singleline(text).desired_width(edit_rect.width()));
            if !te.has_focus() && !te.lost_focus() {
                te.request_focus();
            }
            let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if te.lost_focus() || enter || esc {
                let (id, text) = self.renaming.take().unwrap();
                let text = text.trim().to_string();
                if !esc && !text.is_empty() {
                    if let Some(s) = self.spaces.iter_mut().find(|s| s.id == id) {
                        s.name = text;
                    }
                }
            }
            return;
        }
        let galley = WidgetText::from(RichText::new(format!("{}{}  ({count})", if dirty { "● " } else { "" }, sp.name)).strong()).into_galley(
            ui,
            Some(TextWrapMode::Truncate),
            (rect.right() - text_left - 4.0).max(10.0),
            TextStyle::Button,
        );
        ui.painter().galley(pos2(text_left, rect.center().y - galley.size().y / 2.0), galley, v.strong_text_color());
        let hint = t!("Clic: abrir/cerrar el espacio (los archivos siguen abiertos)\nDoble clic: renombrar · Clic derecho: más opciones\nArrastra archivos aquí para meterlos · arrastra la cabecera para cambiar el orden");
        let mut extra = String::new();
        if dirty {
            extra += t!("● Hay archivos sin guardar en este espacio");
            extra.push('\n');
        }
        if live {
            extra += t!("⏵ Hay archivos en vivo en este espacio");
            extra.push('\n');
        }
        let r = if extra.is_empty() { r.on_hover_text(hint) } else { r.on_hover_text(format!("{extra}\n{hint}")) };
        if r.double_clicked() {
            *action = Some(FileAction::RenameSpace(sp.id));
        } else if r.clicked() {
            *action = Some(FileAction::ToggleSpace(sp.id));
        }
        if let Some(d) = r.dnd_release_payload::<DragFile>() {
            *action = Some(FileAction::ToSpace(d.0, Some(sp.id)));
        }
        r.context_menu(|ui| {
            let mut chosen = None;
            if ui.button(if sp.open { t!("Cerrar el espacio") } else { t!("Abrir el espacio") }).clicked() {
                chosen = Some(FileAction::ToggleSpace(sp.id));
            }
            if ui.button(t!("Renombrar")).clicked() {
                chosen = Some(FileAction::RenameSpace(sp.id));
            }
            ui.separator();
            if ui.add_enabled(count > 0, Button::new(t!("Cerrar sus archivos"))).clicked() {
                chosen = Some(FileAction::CloseSpace(sp.id));
            }
            let del = ui.button(t!("Quitar el espacio")).on_hover_text(t!("Los archivos siguen abiertos, fuera de los espacios"));
            if del.clicked() {
                chosen = Some(FileAction::DeleteSpace(sp.id));
            }
            if chosen.is_some() {
                *action = chosen;
                ui.close();
            }
        });
    }

    /// Barra de estado: información del archivo y del cursor a la izquierda
    /// («Lín X» sirve para ir a una línea); Resultados, Detalle y «?» a la derecha.
    fn status_bar(&mut self, ui: &mut Ui) {
        let h = ui.spacing().interact_size.y;
        let full = ui.available_rect_before_wrap();
        let full = Rect::from_min_size(full.min, vec2(full.width(), h));
        ui.allocate_rect(full, Sense::hover());
        let right_w = 470.0_f32.min(full.width() * 0.55);
        let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);
        let left = Rect::from_min_max(full.min, pos2(right.left() - 6.0, full.bottom()));

        ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
            if ui
                .add(Button::new(RichText::new("?").small().strong()).small().selected(self.show_keys))
                .on_hover_text(t!("Atajos de teclado (F1)"))
                .clicked()
            {
                self.show_keys = !self.show_keys;
            }
            ui.separator();
            let wrap_hint = tf!("Ajuste de línea (Ctrl+Alt+Z): {into} · el modo se elige en ⚙ Ajustes", into = match self.wrap_last {
                    Wrap::Columns(n) => tf!("a {n} columnas", n = n),
                    _ => t!("al ancho de la ventana").into(),
                });
            if ui.selectable_label(self.cur_wrap() != Wrap::Off, RichText::new(t!("↩ Ajuste")).small()).on_hover_text(wrap_hint).clicked() {
                self.toggle_wrap();
            }
            if ui
                .selectable_label(self.show_minimap, RichText::new(t!("▤ Minimapa")).small())
                .on_hover_text(t!("Minimapa del texto a la derecha: vista en miniatura con las coincidencias; clic o arrastrar para moverse"))
                .clicked()
            {
                self.show_minimap = !self.show_minimap;
                self.persist();
            }
            let can_reload = self.tabs.get(self.active).is_some_and(|t| !t.untitled);
            let rk = if MAC { "Cmd+R" } else { "Ctrl+R" };
            if ui
                .add_enabled(can_reload, Button::new(RichText::new(t!("⟳ Recargar")).small()).frame(false))
                .on_hover_text(tf!("Volver a leer el archivo del disco y poner el cursor al final ({rk})", rk = rk))
                .clicked()
            {
                self.reload(self.active, false);
            }
            if let Some(t) = self.tabs.get_mut(self.active) {
                let hint = if t.untitled {
                    t!("Disponible para archivos guardados en disco").to_string()
                } else if t.dirty() && t.follow {
                    t!("En vivo en pausa: hay cambios sin guardar (guárdalos o deshazlos)").to_string()
                } else {
                    t!("En vivo: muestra lo que se va añadiendo al archivo (como tail -f) y baja hasta el final.\nSi el archivo se trunca o rota, se vuelve a abrir.").to_string()
                };
                let paused = t.dirty();
                let label = if t.follow && paused { t!("⏸ En vivo") } else { t!("⏵ En vivo") };
                let r = ui.add_enabled(!t.untitled, Button::selectable(t.follow, RichText::new(label).small()));
                if r.on_hover_text(&hint).on_disabled_hover_text(&hint).clicked() {
                    t.follow = !t.follow;
                    if t.follow {
                        t.follow_at = Instant::now() - FOLLOW_EVERY;
                        t.goto_end = true;
                        t.filter.at_bottom = true;
                    }
                }
                let n = t.filter.rules.iter().filter(|r| !r.text.is_empty()).count();
                let flabel = if n > 0 { tf!("⚟ Filtro ({n})", n = n) } else { t!("⚟ Filtro").to_string() };
                let r = ui.add(Button::selectable(t.filter.on, RichText::new(flabel).small()));
                if r.on_hover_text(t!("Vista filtrada: solo las líneas que cumplen las reglas (incluir / excluir), sin tocar el archivo (Ctrl+Shift+L)")).clicked() {
                    toggle_filter(t, &self.query, self.use_regex, self.case_sensitive);
                }
            }
            ui.toggle_value(&mut self.show_detail, RichText::new(t!("📄 Detalle")).small())
                .on_hover_text(t!("Panel con la línea actual completa, seleccionable"));
            ui.toggle_value(&mut self.show_results, RichText::new(t!("📋 Resultados")).small())
                .on_hover_text(t!("Panel con la lista de líneas encontradas"));
        });

        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.set_clip_rect(left.intersect(ui.clip_rect()));
            let mut goto_line = None;
            let mut set_lang = None;
            let mut copied = false;
            if let Some(t) = self.tabs.get(self.active) {
                let d = &t.doc;
                ui.label(RichText::new(if t.editable { t!("EDICIÓN") } else { t!("SOLO LECTURA") }).small().strong());
                ui.separator();
                // Cursor / ir a línea.
                if self.goto_open {
                    ui.label(RichText::new(t!("Ir a línea:")).small());
                    let r = ui.add(TextEdit::singleline(&mut self.goto).desired_width(90.0).hint_text(t!("nº de línea")));
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
                            tf!("Lín {line_1}, Col {fmt_num}", line_1 = fmt_num(c.line + 1), fmt_num = fmt_num(col as u64))
                        }
                        None => t!("Ir a línea…").to_string(),
                    };
                    if let Some(cs) = t.block() {
                        let ((l0, l1), (c0, c1)) = (cs.lines(), cs.cols());
                        s += &tf!(" · bloque {fmt_num} × {fmt_num2}", fmt_num = fmt_num(l1 - l0 + 1), fmt_num2 = fmt_num((c1 - c0) as u64));
                    } else if let Some((a, b)) = t.selection() {
                        s += &if a.line == b.line {
                            tf!(" · {col_as_u64} seleccionados", col_as_u64 = fmt_num((b.col - a.col) as u64))
                        } else {
                            tf!(" · {line_1} líneas seleccionadas", line_1 = fmt_num(b.line - a.line + 1))
                        };
                    }
                    if ui.add(Button::new(RichText::new(s).small()).frame(false)).on_hover_text(t!("Ir a línea (Ctrl+G)")).clicked() {
                        self.goto_open = true;
                        self.focus_goto = true;
                    }
                }
                ui.separator();
                let lines = fmt_num(t.total());
                if d.indexing_done() {
                    ui.label(RichText::new(tf!("{lines} líneas", lines = lines)).small());
                } else {
                    let p = d.progress.load(Ordering::Relaxed) as f32 / d.len().max(1) as f32;
                    ui.add(ProgressBar::new(p).desired_width(120.0).text(tf!("indexando… {lines} líneas", lines = lines)));
                }
                ui.separator();
                ui.label(RichText::new(human_size(d.len() as u64)).small());
                ui.separator();
                ui.label(RichText::new(encoding::label(d.src.encoding)).small()).on_hover_text(tf!("Detectado por: {detect_reason}", detect_reason = d.detect_reason));
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
                    .on_hover_text(t!("Lenguaje para colorear (se deduce de la extensión)"));
                    if lang != t.lang {
                        set_lang = Some(lang);
                    }
                }
                if let Some(b) = &t.buf {
                    if b.is_dirty() {
                        ui.separator();
                        let (pieces, bytes) = b.memory();
                        ui.label(RichText::new(t!("● sin guardar")).small().color(ui.visuals().warn_fg_color)).on_hover_text(tf!("{fmt_num} piezas · {human_size} de texto editado en RAM", fmt_num = fmt_num(pieces as u64), human_size = human_size(bytes as u64)));
                    }
                }
                if let Some(job) = &t.save {
                    ui.separator();
                    let p = job.progress.load(Ordering::Relaxed) as f32 / job.total.max(1) as f32;
                    ui.add(ProgressBar::new(p.min(1.0)).desired_width(140.0).text(t!("guardando…")));
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
                    ui.label(RichText::new(t!("sin guardar en disco · Ctrl+S para elegir dónde")).small().weak());
                } else {
                    let path = d.path.display().to_string();
                    let r = ui.add(Label::new(RichText::new(&path).small().weak()).sense(Sense::click()).truncate());
                    if r.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                    if r.on_hover_text(t!("Ruta del archivo · clic: copiarla")).clicked() {
                        ui.ctx().copy_text(path);
                        copied = true;
                    }
                }
            }
            if self.tabs.is_empty() {
                if let Some((m, when)) = &self.message {
                    if when.elapsed() < Duration::from_secs(8) {
                        ui.label(RichText::new(m).small().color(ui.visuals().warn_fg_color));
                    }
                }
            }
            if copied {
                self.msg(t!("Ruta copiada"));
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
            Confirm::CloseTab(i) | Confirm::Reload(i) => self.tabs.get(i).map(|t| t.doc.name.clone()).into_iter().collect(),
            Confirm::Quit => self.tabs.iter().filter(|t| t.dirty() || t.save.is_some()).map(|t| t.doc.name.clone()).collect(),
        };
        let mut choice = None;
        Modal::new(Id::NULL.with("confirm")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(t!("Cambios sin guardar"));
            ui.add_space(6.0);
            ui.label(tf!("Hay cambios sin guardar en: {join}", join = names.join(", ")));
            ui.label(t!("Si continúas se perderán. Para conservarlos, cancela y pulsa Ctrl+S."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(t!("Descartar cambios")).clicked() {
                    choice = Some(true);
                }
                if ui.button(t!("Cancelar")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                self.confirm = None;
                match c {
                    Confirm::CloseTab(i) => self.close_tab(i, true),
                    Confirm::Reload(i) => self.reload(i, true),
                    Confirm::Quit => {
                        self.allow_quit = true;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
            }
            Some(false) => {
                if matches!(c, Confirm::Quit) {
                    self.relaunch = None; // se canceló el cierre: no reiniciar
                }
                self.confirm = None;
            }
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
        // Se detiene en 999: 1000 marca «captura pedida».
        if self.debug_frames < 999 {
            self.debug_frames += 1;
        }
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
            if let Ok(w) = std::env::var("BUSCADOR_AJUSTE") {
                self.wrap = Wrap::parse(&w);
                for t in &mut self.tabs {
                    t.wrap = self.wrap;
                }
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
            if let Ok(m) = std::env::var("BUSCADOR_MAPA") {
                // «carpeta|texto»: abre el mapa, lee la carpeta y busca el texto.
                let (dir, q) = m.split_once('|').unwrap_or((&m, ""));
                self.map.open = true;
                self.map.detached = std::env::var("BUSCADOR_MAPA_APARTE").is_ok();
                self.map.needle = q.into();
                self.map.search_when_ready = !q.is_empty();
                self.map.start_scan(dir.into(), ctx);
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
            // Pruebas: BUSCADOR_ESPACIOS='Logs=0,1;Config-=2' crea espacios con esos
            // archivos (índices); «-» al final del nombre = espacio cerrado.
            // BUSCADOR_FILTRO='+error;-timeout' abre la vista filtrada con esas reglas.
            if let (Ok(spec), Some(t)) = (std::env::var("BUSCADOR_FILTRO"), self.tabs.get_mut(self.active)) {
                t.filter.rules = spec
                    .split(';')
                    .map(|r| crate::filter::Rule { exclude: r.starts_with('-'), text: r.trim_start_matches(['+', '-']).into(), ..Default::default() })
                    .collect();
                t.filter.on = true;
                t.filter.top = u64::MAX;
            }
            // BUSCADOR_VIVO=1 activa «En vivo» en el archivo activo.
            if let (Ok(_), Some(t)) = (std::env::var("BUSCADOR_VIVO"), self.tabs.get_mut(self.active)) {
                t.follow = true;
                t.goto_end = true;
            }
            // BUSCADOR_CARPETA='dir|texto' abre «Buscar en carpeta» y busca.
            if let Ok(spec) = std::env::var("BUSCADOR_CARPETA") {
                let (d, q) = spec.split_once('|').unwrap_or((&spec, ""));
                self.folder.dir = d.to_string();
                self.folder.query = q.to_string();
                self.folder.open = true;
                if !q.is_empty() {
                    if let Ok(j) = crate::foldersearch::Job::start(d.into(), q, false, false, Vec::new(), ctx.clone()) {
                        self.folder.job = Some(j);
                    }
                }
            }
            // BUSCADOR_ELEGIR='0,2': selección múltiple en el panel (índices).
            if let Ok(spec) = std::env::var("BUSCADOR_ELEGIR") {
                self.picked = spec.split(',').filter_map(|n| self.tabs.get(n.trim().parse::<usize>().ok()?)).map(|t| t.doc.path.clone()).collect();
            }
            if let Ok(spec) = std::env::var("BUSCADOR_ESPACIOS") {
                for part in spec.split(';') {
                    let (name, files) = part.split_once('=').unwrap_or((part, ""));
                    let id = self.next_space;
                    self.next_space += 1;
                    let open = !name.ends_with('-');
                    self.spaces.push(Space { id, name: name.trim_end_matches('-').to_string(), open });
                    for n in files.split(',').filter_map(|n| n.trim().parse::<usize>().ok()) {
                        if let Some(t) = self.tabs.get_mut(n) {
                            t.space = Some(id);
                        }
                    }
                }
            }
            // Pruebas: BUSCADOR_RECARGAR=texto añade esa línea al archivo y lo recarga.
            if let (Ok(extra), Some(t)) = (std::env::var("BUSCADOR_RECARGAR"), self.tabs.get(self.active)) {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&t.doc.path) {
                    let _ = writeln!(f, "{extra}");
                }
                self.reload(self.active, false);
            }
            // BUSCADOR_GUARDAR=línea: edita, se coloca en esa línea y guarda (Cmd+S).
            if let (Ok(l), Some(t)) = (std::env::var("BUSCADOR_GUARDAR"), self.tabs.get_mut(self.active)) {
                let l: u64 = l.parse().unwrap_or(0);
                t.editable = true;
                let p = Pos::new(1, 0);
                t.select(p, p);
                let _ = t.insert("editado ", EditKind::Typing);
                let p = Pos::new(l, 0);
                t.select(p, p);
                t.top = l.saturating_sub(5);
                self.save(false);
            }
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
        // BUSCADOR_ESPERA=ms: retrasa la captura (p. ej. para ver «En vivo»).
        let wait = std::env::var("BUSCADOR_ESPERA").ok().and_then(|v| v.parse::<f64>().ok()).is_some_and(|ms| ctx.input(|i| i.time) * 1000.0 < ms);
        if self.debug_frames < 90 || busy || wait || (std::env::var("BUSCADOR_TYPE").is_ok() && !self.debug_typed) {
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
    fn on_exit(&mut self) {
        self.save_session(true);
        if let Some(p) = self.relaunch.take() {
            if let Err(e) = crate::update::relaunch_after_exit(&p) {
                eprintln!("No se pudo abrir la versión nueva: {e}");
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Cerrar la ventana con cambios sin guardar pide confirmación.
        if ctx.input(|i| i.viewport().close_requested()) {
            if !self.allow_quit && self.tabs.iter().any(|t| t.dirty() || t.save.is_some()) {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.confirm = Some(Confirm::Quit);
            } else if let Some(p) = self.relaunch.take() {
                // Se cierra de verdad: el relanzador espera a que este proceso termine.
                self.save_session(true);
                if let Err(e) = crate::update::relaunch_after_exit(&p) {
                    self.msg(tf!("No se pudo abrir la versión nueva: {e}", e = e));
                }
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
                    self.msg(tf!("Duplicado: {display}", display = p.display()));
                }
                Err(e) => self.msg(tf!("No se pudo duplicar: {e}", e = e)),
            }
        }

        self.poll_dialog();
        // Archivos abiertos desde Finder («Abrir con…»).
        #[cfg(target_os = "macos")]
        for p in crate::macopen::take() {
            self.open(p);
        }
        // Archivos arrastrados a la ventana; si se sueltan sobre un espacio del
        // panel (su cabecera o sus archivos), quedan dentro de él.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        let os_drag = ctx.input(|i| !i.raw.hovered_files.is_empty());
        self.os_drop_space = if os_drag || !dropped.is_empty() {
            let pos = os_pointer(&ctx);
            pos.and_then(|p| self.drop_zones.iter().find(|z| z.0.contains(p)).map(|z| z.1))
        } else {
            None
        };
        if os_drag {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let into = self.os_drop_space.take();
        for p in dropped {
            self.open(p.clone());
            if let (Some(id), Some(t)) = (into, self.tabs.iter_mut().find(|t| t.doc.path == p)) {
                t.space = Some(id);
                if let Some(s) = self.spaces.iter_mut().find(|s| s.id == id) {
                    s.open = true;
                }
            }
        }
        if os_drag {
            self.os_drop_space = into;
        }
        // Con el foco en el texto, Tab y las flechas son para el texto: que egui
        // no mueva el foco a otro control aunque se haya perdido el bloqueo.
        if self.view_focused
            && ctx.input(|i| {
                i.events.iter().any(|e| {
                    matches!(e, Event::Key { key: Key::Tab | Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight, pressed: true, modifiers, .. }
                        if !modifiers.ctrl && !modifiers.command)
                })
            })
        {
            ctx.memory_mut(|m| m.move_focus(FocusDirection::None));
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

        if let Some(t) = self.tabs.get_mut(self.active).filter(|t| t.filter.on) {
            let (buf, doc) = (&t.buf, &t.doc);
            if t.filter.poll(&doc.src, || buf.as_ref().map(|b| (b.snapshot(), doc.index.clone())), t.edit_gen) {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        let mut message = None;
        let mut view_focused = false;
        let colors = self.text_colors();
        let syntax_on = self.syntax_on;
        let col_mode = self.column_mode && self.mono_font();
        for t in &mut self.tabs {
            t.col_mode = col_mode && t.wrap == Wrap::Off;
        }
        let show_minimap = self.show_minimap;
        let show_detail = &mut self.show_detail;
        let focus_view = std::mem::take(&mut self.focus_view);
        CentralPanel::default().frame(Frame::NONE).show(ui, |ui| {
            if let Some(t) = self.tabs.get_mut(self.active).filter(|t| t.filter.on) {
                message = filter_view(ui, t, &font, colors, syntax_on);
            } else if let Some(t) = self.tabs.get_mut(self.active) {
                let wrap = t.wrap;
                if show_minimap {
                    // Minimapa en una franja a la derecha; el texto en el resto.
                    let full = ui.available_rect_before_wrap();
                    let (text_r, map_r) = full.split_left_right_at_x(full.right() - MINIMAP_W);
                    ui.scope_builder(UiBuilder::new().max_rect(text_r), |ui| {
                        (message, view_focused) = log_view(ui, t, &font, show_detail, focus_view, colors, syntax_on, wrap);
                    });
                    ui.scope_builder(UiBuilder::new().max_rect(map_r), |ui| text_minimap(ui, t, syntax_on));
                } else {
                    (message, view_focused) = log_view(ui, t, &font, show_detail, focus_view, colors, syntax_on, wrap);
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.heading(t!("Arrastra archivos aquí, o pulsa Ctrl+O"));
                });
            }
        });
        self.view_focused = view_focused;
        // «Nuevo archivo con la selección» y texto pendiente de archivos nuevos.
        let exports: Vec<String> = self.tabs.iter_mut().filter_map(|t| t.export_request.take()).collect();
        for text in exports {
            self.new_untitled(Some(text));
        }
        self.follow_files(&ctx);
        for t in &mut self.tabs {
            // Posición de la sesión anterior, en cuanto el índice llega hasta ella.
            if let Some((top, c)) = t.restore {
                if t.doc.indexing_done() || t.total() > c.line.max(top) + 200 {
                    t.restore = None;
                    t.select(c, c);
                    t.top = top;
                    t.reveal = false;
                }
            }
            if t.goto_end && t.doc.indexing_done() {
                t.goto_end = false;
                let e = t.end_pos();
                t.select(e, e);
            }
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
        if self.tabs.iter().any(|t| t.pending_text.is_some() || t.restore.is_some() || t.goto_end) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.save_session(ctx.input(|i| i.viewport().close_requested()));
        if let Some(m) = message {
            self.msg(m);
        }
        self.settings_window(&ctx);
        self.update_window(&ctx);
        self.keys_window(&ctx);
        self.adv_windows(&ctx);
        self.map_window(&ctx);
        self.folder_window(&ctx);
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
            || self.advs.iter().any(|w| w.is_busy())
            || self.map.is_busy()
            || self.folder.is_busy();
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
    wrap: Wrap,
) -> (Option<String>, bool) {
    apply_text_colors(ui, colors);
    let wrapping = wrap != Wrap::Off;
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
    // Líneas por pantalla: con ajuste, las que cupieron enteras la última vez.
    let page = if wrapping { tab.lines_shown.clamp(1, visible) } else { visible };

    // Teclado (antes de medir: puede cambiar el número de líneas).
    if resp.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
        });
        message = tab.handle_events(&ctx, page.saturating_sub(1).max(1) as i64);
    }

    let total = tab.total();
    // Con ajuste no se sabe cuántas filas ocupan las últimas líneas: se deja
    // llegar hasta la última línea arriba del todo.
    let max_top = if wrapping { total.saturating_sub(1) } else { total.saturating_sub(visible) };
    let digits = fmt_num(total.max(1)).chars().count().max(4);
    let gutter_w = char_w * digits as f32 + 14.0;
    let text_x = text_area.left() + gutter_w + 6.0;
    let view_w = (text_area.right() - text_x).max(char_w * 4.0);
    // Ancho al que se parte cada línea.
    let wrap_w = match wrap {
        Wrap::Off => f32::INFINITY,
        Wrap::Window => (view_w - char_w).max(char_w * 8.0),
        Wrap::Columns(n) => n as f32 * char_w + 0.5,
    };
    if wrap == Wrap::Window {
        tab.hscroll = 0.0;
    }

    if let Some(l) = tab.center_on.take() {
        if l < tab.top || l >= tab.top + page {
            tab.top = l.saturating_sub(page / 2);
        }
    }
    if tab.reveal {
        if let Some(c) = tab.cursor {
            if c.line < tab.top {
                tab.top = c.line;
            } else if c.line >= tab.top + page {
                tab.top = c.line + 1 - page;
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
    // Con ajuste, la línea bajo el puntero sale del mapa del último fotograma.
    let map = std::mem::take(&mut tab.layout_map);
    let map_at = |y: f32| -> Option<(u64, f32)> {
        let ry = y - text_area.top();
        let e = map.iter().find(|e| ry < e.1 + e.2).or(map.last())?;
        Some((e.0, (ry - e.1).max(0.0)))
    };
    let frame_top = tab.top;
    let row_at = |top: u64, y: f32| -> u64 {
        if wrapping && top == frame_top {
            if let Some((l, _)) = map_at(y) {
                return l.min(total.saturating_sub(1));
            }
        }
        let r = ((y - text_area.top()) / row_h).floor().max(0.0) as u64;
        (top + r).min(total.saturating_sub(1))
    };
    let pos_at = |tab: &Tab, top: u64, p: Pos2| -> Pos {
        let line = row_at(top, p.y);
        let (t, _) = cap_line(tab.line(line));
        let g = ui.painter().layout_job(plain_job(&t, font, wrap_w));
        let in_y = if wrapping { map_at(p.y).filter(|m| m.0 == line).map_or(row_h / 2.0, |m| m.1) } else { row_h / 2.0 };
        let idx = g.cursor_from_pos(vec2(p.x - text_x + tab.hscroll, in_y)).index.0;
        Pos::new(line, col_from_disp(&t, idx))
    };
    // Columna visual bajo el puntero (puede pasar del final de la línea).
    let vcol_at = |tab: &Tab, x: f32| -> usize { ((x - text_x + tab.hscroll) / char_w).round().max(0.0) as usize };
    if total > 0 {
        let shift = ui.input(|i| i.modifiers.shift);
        // Modo bloque, o Opción/Alt+arrastrar para un bloque puntual.
        let block_drag = !wrapping && (tab.col_mode || ui.input(|i| i.modifiers.alt));
        let press = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()) } else { None };
        if let Some(p) = press.or_else(|| (resp.clicked() || resp.secondary_clicked()).then(|| resp.interact_pointer_pos()).flatten())
        {
            // Pedir el foco de nuevo lo «reinicia» y borra el bloqueo de Tab/flechas:
            // como egui solo repinta con eventos, el Tab siguiente movía el foco a
            // otro control en vez de sangrar. Solo se pide si no se tiene.
            if !resp.has_focus() {
                resp.request_focus();
            }
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
        if ui.button(t!("Copiar (Ctrl+C)")).clicked() {
            message = Some(match tab.block() {
                Some(cs) => tab.copy_block(&ctx, cs),
                None => tab.copy(&ctx),
            });
            ui.close();
        }
        let has_any = tab.block().is_some() || tab.selection().is_some();
        if ui
            .add_enabled(has_any, Button::new(t!("📝 Nuevo archivo con la selección")))
            .on_hover_text(t!("Abre un archivo nuevo con el texto seleccionado (o el bloque), listo para guardar"))
            .clicked()
        {
            let text = match (tab.block(), tab.selection()) {
                (Some(cs), _) => tab.column_text(cs, MAX_COPY_BYTES),
                (None, Some((a, b))) => tab.text(a, b, MAX_COPY_BYTES),
                _ => None,
            };
            match text {
                Some(t) => tab.export_request = Some(t),
                None => message = Some(tf!("Selección demasiado grande (> {human_size})", human_size = human_size(MAX_COPY_BYTES as u64))),
            }
            ui.close();
        }
        if ui.add_enabled(tab.editable, Button::new(t!("Cortar (Ctrl+X)"))).clicked() {
            message = Some(match tab.block() {
                Some(cs) => tab.cut_block(&ctx, cs).unwrap_or_else(|e| e),
                None => tab.cut(&ctx).unwrap_or_else(|e| e),
            });
            ui.close();
        }
        if ui.add_enabled(tab.editable, Button::new(t!("Pegar (Ctrl+V)"))).clicked() {
            resp.request_focus();
            ctx.send_viewport_cmd(ViewportCommand::RequestPaste);
            ui.close();
        }
        if ui.add_enabled(tab.editable && (has_sel || tab.block().is_some()), Button::new(t!("Borrar selección"))).clicked() {
            let r = match tab.block() {
                Some(cs) => tab.delete_block(cs),
                None => tab.delete(false, false),
            };
            if let Err(e) = r {
                message = Some(e);
            }
            ui.close();
        }
        let has_lines = has_sel || tab.block().is_some();
        let trim = ui.add_enabled(tab.editable && has_lines, Button::new(t!("Quitar espacios y tabs al final")));
        if trim.on_hover_text(t!("En cada línea seleccionada, borra los espacios y tabuladores que haya al final")).clicked() {
            message = Some(tab.trim_trailing().unwrap_or_else(|e| e));
            ui.close();
        }
        ui.separator();
        if ui.button(t!("Seleccionar todo (Ctrl+A)")).clicked() {
            tab.select(Pos::default(), tab.end_pos());
            ui.close();
        }
        if ui.button(t!("Ver línea completa")).clicked() {
            *show_detail = true;
            ui.close();
        }
    });

    // Mientras se indexa (p. ej. justo después de guardar, que reabre el
    // archivo) aún no se sabe cuántas líneas hay: no recortar la posición, o
    // la vista saltaría al principio.
    if tab.buf.is_some() || tab.doc.indexing_done() {
        tab.top = tab.top.min(max_top);
    }

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
    // (línea, texto, maquetación, y relativa al área, alto). Con ajuste, cada
    // línea puede ocupar varias filas.
    let mut y_acc = 0.0f32;
    let mut lines_shown = 0u64;
    let mut rows: Vec<(u64, String, Arc<Galley>, f32, f32)> = Vec::new();
    for (n, t, tr) in tab.rows(tab.top, visible as usize + 1, MAX_DISPLAY_BYTES) {
        if y_acc >= text_area.height() {
            break;
        }
        spans.clear();
        st = syntax::highlight(lang, &t, st, &mut spans);
        let mut job = line_job(&t, tr, font, visuals.text_color(), highlight, &spans, dark);
        job.wrap.max_width = wrap_w;
        let g = ui.painter().layout_job(job);
        let h = if wrapping { (g.rows.len().max(1) as f32 * row_h).max(row_h) } else { row_h };
        if y_acc + h <= text_area.height() + 0.5 {
            lines_shown += 1;
        }
        rows.push((n, t, g, y_acc, h));
        y_acc += h;
    }
    tab.lines_shown = lines_shown.max(1);
    tab.layout_map = rows.iter().map(|r| (r.0, r.3, r.4)).collect();
    // Posición (en la maquetación) de una columna: x e y de su fila.
    let pos_of = |t: &str, g: &Galley, col: usize| g.pos_from_cursor(CCursor::new(disp_col(t, col)));
    let x_of = |t: &str, g: &Galley, col: usize| pos_of(t, g, col).min.x;

    // Desplazamiento horizontal para que se vea el cursor.
    if std::mem::take(&mut tab.reveal) {
        if let Some(c) = tab.cursor {
            if let Some((_, t, g, _, _)) = rows.iter().find(|r| r.0 == c.line).filter(|_| wrap != Wrap::Window) {
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

    for (n, t, galley, y_rel, h) in &rows {
        let n = *n;
        let y = text_area.top() + y_rel;
        let row = Rect::from_min_size(pos2(text_area.left(), y), vec2(text_area.width(), *h));
        if tab.current == Some(n) {
            painter.rect_filled(row, 0.0, match_line_color);
        } else if tab.cursor.is_some_and(|c| c.line == n) && sel.is_none() {
            painter.rect_filled(row, 0.0, cur_line_color);
        }
        if tab.search.as_ref().is_some_and(|s| s.index_of(n).is_some()) {
            painter.rect_filled(Rect::from_min_size(row.min, vec2(3.0, *h)), 0.0, mark_color);
        }
        // Selección: un rectángulo por cada fila (con ajuste, varias por línea).
        if let Some((a, b)) = sel.filter(|(a, b)| a.line <= n && n <= b.line) {
            let d0 = if n == a.line { disp_col(t, a.col) } else { 0 };
            let d1 = if n == b.line { disp_col(t, b.col) } else { usize::MAX };
            for r in selection_rects(galley, d0, d1, n < b.line, char_w * 0.6, row_h) {
                text_painter.rect_filled(r.translate(vec2(ox, y)), 0.0, sel_color);
            }
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
            let cr = pos_of(t, galley, c.col);
            let x = ox + cr.min.x;
            // Con ajuste, el cursor va en su fila dentro de la línea.
            let y = if wrapping { y + (cr.min.y / row_h).round().min((galley.rows.len().max(1) - 1) as f32) * row_h } else { y };
            let mut caret_x = x;
            if !tab.preedit.is_empty() {
                let g = ui.painter().layout_job(plain_job(&tab.preedit, font, f32::INFINITY));
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
    tab.content_width = if wrap == Wrap::Window { view_w } else { tab.content_width.max(max_w) };

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

    // También aquí: si el foco llegó en este fotograma (clic), que el próximo
    // Tab o flecha ya sea para el texto.
    if resp.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
        });
    }

    // Barras de desplazamiento.
    let mut top = tab.top;
    vscrollbar(ui, vsb, id.with("vsb"), &mut top, total, if wrapping { 1 } else { visible });
    tab.top = top;
    // Con ajuste a la ventana no hay nada que desplazar en horizontal.
    let content = if wrap == Wrap::Window { view_w } else { tab.content_width + 40.0 };
    hscrollbar(ui, hsb, id.with("hsb"), &mut tab.hscroll, content, view_w);
    let note = if !tab.doc.indexing_done() {
        Some(t!("indexando…"))
    } else if !tab.editable {
        Some(t!("solo lectura · Ctrl+E para editar"))
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

fn plain_job(txt: &str, font: &FontId, wrap_w: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_w;
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
    let mut col = 0;
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
        if show_whitespace() && !m && txt[i..j].contains([' ', '\t']) {
            // Los · y → en tenue, el resto con su color.
            let ws_fmt = TextFormat { color: color.gamma_multiply(0.35), ..fmt.clone() };
            let seg = &txt[i..j];
            let mut a = 0;
            for (k, ch) in seg.char_indices() {
                if ch == ' ' || ch == '\t' {
                    if k > a {
                        job.append(&sanitize_at(&seg[a..k], &mut col), 0.0, fmt.clone());
                    }
                    job.append(&sanitize_at(&seg[k..k + 1], &mut col), 0.0, ws_fmt.clone());
                    a = k + 1;
                }
            }
            if a < seg.len() {
                job.append(&sanitize_at(&seg[a..], &mut col), 0.0, fmt);
            }
        } else {
            job.append(&sanitize_at(&txt[i..j], &mut col), 0.0, fmt);
        }
        i = j;
    }
    if truncated {
        job.append(t!("  …[línea truncada; ver Detalle]"), 0.0, TextFormat { italics: true, ..normal });
    }
    job
}

/// Rectángulos (en coordenadas de la maquetación) que cubren los caracteres
/// mostrados `d0..d1` de una línea, uno por fila. Si `newline`, el último se
/// alarga un poco para indicar que el salto de línea también está incluido.
fn selection_rects(g: &Galley, d0: usize, d1: usize, newline: bool, nl_w: f32, row_h: f32) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let last = g.rows.len().saturating_sub(1);
    for (k, row) in g.rows.iter().enumerate() {
        let n_excl = row.char_count_excluding_newline().0;
        let end = start + n_excl;
        let (a, b) = (d0.max(start), d1.min(end));
        let y0 = k as f32 * row_h;
        if a < b || (a == b && a >= start && a <= end && newline && k == last) {
            let x0 = row.pos.x + row.x_offset(epaint::text::CharIndex(a - start));
            let mut x1 = row.pos.x + row.x_offset(epaint::text::CharIndex(b - start));
            if newline && k == last {
                x1 += nl_w;
            }
            out.push(Rect::from_min_max(pos2(x0, y0), pos2(x1.max(x0 + 1.0), y0 + row_h)));
        }
        start += row.char_count_including_newline().0;
    }
    if out.is_empty() && g.rows.is_empty() && newline {
        out.push(Rect::from_min_size(Pos2::ZERO, vec2(nl_w, row_h)));
    }
    out
}

/// ¿Fondo oscuro? (para elegir los colores de sintaxis)
fn is_dark(bg: Color32) -> bool {
    let l = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    l < 128.0
}

/// Tabs a espacios y caracteres de control visibles (una sola fila por línea).
/// Cada carácter produce exactamente uno, salvo el tab (`TAB_WIDTH`): ver `disp_col`.
fn sanitize(s: &str) -> String {
    sanitize_at(s, &mut 0)
}

/// Como `sanitize`, sabiendo en qué columna empieza `s` (para que el tab
/// llegue a la siguiente parada fija, múltiplo de `TAB_WIDTH`); avanza `col`.
fn sanitize_at(s: &str, col: &mut usize) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\t' => {
                let w = tab_stop(*col);
                if show_whitespace() {
                    out.push('→');
                    out.extend(std::iter::repeat_n(' ', w - 1));
                } else {
                    out.extend(std::iter::repeat_n(' ', w));
                }
                *col += w;
                continue;
            }
            ' ' if show_whitespace() => out.push('·'),
            c if c.is_control() => out.push('·'),
            c => out.push(c),
        }
        *col += 1;
    }
    out
}

/// Ancho de un tab que empieza en la columna `col` (hasta la siguiente parada).
fn tab_stop(col: usize) -> usize {
    let w = tab_width();
    w - col % w
}

/// Ancho en columnas de `c` si empieza en la columna `col`.
fn char_disp_width(c: char, col: usize) -> usize {
    if c == '\t' { tab_stop(col) } else { 1 }
}

/// Índice de carácter mostrado correspondiente al byte `col` de `s`.
fn disp_col(s: &str, col: usize) -> usize {
    s[..buffer::floor_char(s, col)].chars().fold(0, |acc, c| acc + char_disp_width(c, acc))
}

/// Byte de `s` correspondiente al carácter mostrado `d` (redondeando dentro de un tab).
fn col_from_disp(s: &str, d: usize) -> usize {
    let mut acc = 0;
    for (i, c) in s.char_indices() {
        if acc >= d {
            return i;
        }
        let w = char_disp_width(c, acc);
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
        tf!("{line_1} líneas ({human_size})", line_1 = fmt_num(b.line - a.line + 1), human_size = human_size(bytes as u64))
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
        tf!("Búsqueda avanzada {id}{unwrap_or_default}", id = self.id, unwrap_or_default = name.unwrap_or_default())
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

fn update_window_id() -> Id {
    Id::NULL.with("ventana_actualizar")
}

/// Hace más visibles los bordes y separaciones de paneles y ventanas, en los
/// dos temas (claro: más oscuros; oscuro: más claros).
fn apply_borders(ctx: &Context, level: u8) {
    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |s| {
            let base = if theme == Theme::Dark { Visuals::dark() } else { Visuals::light() };
            let (gray, width) = match (theme, level) {
                (_, 0) => (None, 1.0),
                (Theme::Dark, 1) => (Some(105), 1.0),
                (Theme::Dark, _) => (Some(150), 1.5),
                (Theme::Light, 1) => (Some(140), 1.0),
                (Theme::Light, _) => (Some(95), 1.5),
            };
            let stroke = |def: Stroke| gray.map_or(def, |g| Stroke::new(width, Color32::from_gray(g)));
            s.visuals.widgets.noninteractive.bg_stroke = stroke(base.widgets.noninteractive.bg_stroke);
            s.visuals.window_stroke = stroke(base.window_stroke);
        });
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
        let mut borders = self.borders;
        let mut tabw = tab_width();
        let mut show_ws = show_whitespace();
        let mut undo_steps = buffer::max_undo();
        let mut lang_change = None;
        let mut wrap = self.wrap;
        let mut wrap_cols = match self.wrap_last {
            Wrap::Columns(n) => n,
            _ => 100,
        };
        let mut open_update = false;
        let mut reset = false;
        let mut win = Window::new(t!("Ajustes")).id(settings_window_id()).pivot(Align2::CENTER_CENTER);
        if just_opened {
            win = win.current_pos(ctx.content_rect().center());
        }
        win.open(&mut open)
            .resizable(false)
            .collapsible(false)
            .frame(Frame::window(&ctx.global_style()).inner_margin(18.0))
            .show(ctx, |ui| {
                ui.set_width(560.0);
                ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
                // Etiqueta a la izquierda, controles a la derecha.
                Grid::new("ajustes_grid").num_columns(2).spacing([28.0, 16.0]).min_col_width(150.0).show(ui, |ui| {
                    ui.strong(t!("Idioma")).on_hover_text("Idioma · Language · Langue");
                    ui.horizontal(|ui| {
                        let cur = crate::i18n::get();
                        for l in crate::i18n::Lang::ALL {
                            if ui.radio(cur == l, l.name()).clicked() && cur != l {
                                lang_change = Some(l);
                            }
                        }
                    });
                    ui.end_row();

                    ui.strong(t!("Tema"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut theme, ThemePreference::System, t!("Sistema"));
                        ui.radio_value(&mut theme, ThemePreference::Light, t!("Claro"));
                        ui.radio_value(&mut theme, ThemePreference::Dark, t!("Oscuro"));
                    });
                    ui.end_row();

                    ui.strong(t!("Bordes de paneles")).on_hover_text(t!("Separaciones entre paneles y bordes de las ventanas, en cualquier tema"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut borders, 0, t!("Normales"));
                        ui.radio_value(&mut borders, 1, t!("Marcados"));
                        ui.radio_value(&mut borders, 2, t!("Muy marcados"));
                    });
                    ui.end_row();

                    ui.strong(t!("Tipo de letra"));
                    ui.vertical(|ui| {
                        ComboBox::from_id_salt("tipo_letra").selected_text(crate::fonts::label(&family)).width(240.0).show_ui(ui, |ui| {
                            let av = crate::fonts::available();
                            ui.weak(t!("Monoespaciadas (alinean columnas)"));
                            for o in av.iter().filter(|o| o.mono) {
                                ui.selectable_value(&mut family, o.name.to_string(), crate::fonts::label(o.name));
                            }
                            ui.separator();
                            ui.weak(t!("Proporcionales"));
                            for o in av.iter().filter(|o| !o.mono) {
                                ui.selectable_value(&mut family, o.name.to_string(), crate::fonts::label(o.name));
                            }
                        });
                        if crate::fonts::find(&family).is_some_and(|o| !o.mono) {
                            ui.weak(t!("Con letra proporcional las columnas de los logs no quedan alineadas."));
                        }
                    });
                    ui.end_row();

                    ui.strong(t!("Tamaño de letra"));
                    ui.add(Slider::new(&mut font, 7.0..=40.0).step_by(1.0));
                    ui.end_row();

                    ui.strong(t!("Tabulador"));
                    ui.horizontal(|ui| {
                        for w in [2, 4, 8] {
                            ui.radio_value(&mut tabw, w, tf!("{w} espacios", w = w));
                        }
                        ui.add(DragValue::new(&mut tabw).range(1..=16).suffix(t!(" col."))).on_hover_text(t!("Columnas entre paradas de tabulador (1–16)"));
                    });
                    ui.end_row();

                    ui.strong(t!("Deshacer"));
                    ui.horizontal(|ui| {
                        ui.add(DragValue::new(&mut undo_steps).range(1..=10_000).speed(1.0));
                        ui.label(t!("pasos (se conservan también al guardar)"));
                    });
                    ui.end_row();

                    ui.strong(t!("Espacios"));
                    ui.checkbox(&mut show_ws, t!("Mostrar espacios y tabuladores (· y →)"));
                    ui.end_row();

                    ui.strong(t!("Colores del texto"));
                    ui.vertical(|ui| {
                        ui.checkbox(&mut custom, t!("Usar colores propios (tienen prioridad sobre el tema)"));
                        ui.add_enabled_ui(custom, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(t!("Fondo"));
                                ui.color_edit_button_srgba(&mut bg);
                                ui.add_space(16.0);
                                ui.label(t!("Letra"));
                                ui.color_edit_button_srgba(&mut fg);
                            });
                        });
                    });
                    ui.end_row();

                    ui.strong(t!("Ajuste de línea"));
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut wrap, Wrap::Off, t!("No"));
                            ui.radio_value(&mut wrap, Wrap::Window, t!("Al ancho de la ventana"));
                        });
                        ui.horizontal(|ui| {
                            let cols = match wrap {
                                Wrap::Columns(n) => n,
                                _ => wrap_cols,
                            };
                            if ui.radio(matches!(wrap, Wrap::Columns(_)), t!("A")).clicked() {
                                wrap = Wrap::Columns(cols);
                            }
                            let mut c = cols;
                            if ui.add(DragValue::new(&mut c).range(20..=1000).speed(1)).changed() {
                                wrap_cols = c;
                                wrap = Wrap::Columns(c);
                            }
                            ui.label(t!("columnas"));
                        });
                        ui.weak(t!("Valor por defecto; cada archivo lo cambia con ↩ Ajuste."));
                    });
                    ui.end_row();

                    ui.strong(t!("Resaltado"));
                    ui.add(Checkbox::new(&mut syn, t!("Colorear según el tipo de archivo (.java, .xml, .json, .py…) y los niveles de los logs")));
                    ui.end_row();
                });

                ui.add_space(14.0);
                ui.weak(t!("Vista previa:"));
                let (pbg, pfg) = if custom { (bg, fg) } else { (ui.visuals().extreme_bg_color, ui.visuals().text_color()) };
                Frame::NONE.fill(pbg).inner_margin(10.0).corner_radius(6.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let sample = [t!("2026-11-10 12:00:01 [ERROR] año, acción: java.io.IOException"), "0O 1lI {}[]() -> => != 日本"];
                    let lang = if syn { syntax::Lang::Log } else { syntax::Lang::Plain };
                    for line in sample {
                        let mut spans = Vec::new();
                        syntax::highlight(lang, line, syntax::State::Normal, &mut spans);
                        let job = line_job(line, false, &crate::fonts::text_font(font), pfg, None, &spans, is_dark(pbg));
                        ui.label(job);
                    }
                });
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(tf!("Versión {crate_update_version_label}", crate_update_version_label = crate::update::version_label()));
                    if crate::update::SUPPORTED {
                        if ui.button(t!("⟳ Buscar actualizaciones…")).on_hover_text(t!("Descarga la última versión de GitHub, la compila y la instala")).clicked() {
                            open_update = true;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(t!("↺ Restablecer")).on_hover_text(t!("Vuelve a los ajustes predeterminados: tema del sistema, Hack 13, sin colores propios, bordes normales")).clicked() {
                            reset = true;
                        }
                    });
                });
                if !crate::update::SUPPORTED {
                    ui.weak(t!("En Windows, actualiza volviendo a ejecutar el instalador."));
                }
                ui.add_space(6.0);
                ui.weak(t!("Los ajustes se guardan automáticamente."));
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
            self.wrap = Wrap::parse(&d.wrap);
            self.text_bg = DEFAULT_BG;
            self.text_fg = DEFAULT_FG;
            self.borders = d.borders;
            apply_borders(ctx, d.borders);
            TAB_WIDTH.store(d.tab_width, Ordering::Relaxed);
            buffer::set_max_undo(d.undo_steps);
            for t in &mut self.tabs {
                t.content_width = 0.0;
                t.minimap_cache = None;
            }
            self.persist();
            self.msg(t!("Ajustes restablecidos"));
            return;
        }
        if wrap != self.wrap {
            // Ajustes: modo por defecto de los archivos que se abran y del activo.
            self.wrap = wrap;
            if wrap != Wrap::Off {
                self.wrap_last = wrap;
            }
            if let Some(t) = self.tabs.get_mut(self.active) {
                t.wrap = wrap;
                t.content_width = 0.0;
                t.colsel = None;
            }
            self.persist();
        }
        let _ = wrap_cols;
        if (custom, bg, fg, syn) != (self.custom_colors, self.text_bg, self.text_fg, self.syntax_on) {
            self.syntax_on = syn;
            self.custom_colors = custom;
            self.text_bg = bg;
            self.text_fg = fg;
            self.persist();
        }
        if borders != self.borders {
            self.borders = borders;
            apply_borders(ctx, borders);
            self.persist();
        }
        if undo_steps != buffer::max_undo() {
            buffer::set_max_undo(undo_steps);
            self.persist();
        }
        if show_ws != show_whitespace() {
            SHOW_WS.store(show_ws, Ordering::Relaxed);
            self.persist();
        }
        if tabw != tab_width() {
            TAB_WIDTH.store(tabw.clamp(1, 16), Ordering::Relaxed);
            for t in &mut self.tabs {
                t.content_width = 0.0;
                t.minimap_cache = None;
            }
            self.persist();
        }
        if let Some(l) = lang_change {
            crate::i18n::set(l);
            self.lang_chosen = Some(l);
            self.persist();
            // El menú nativo se crea con los textos: se vuelve a crear.
            #[cfg(target_os = "macos")]
            {
                self.menu = crate::macmenu::MacMenu::new(ctx);
            }
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
            self.set_text_font(ctx, family);
        }
    }

    /// Cambia el tipo de letra del texto (y lo guarda en los ajustes).
    fn set_text_font(&mut self, ctx: &Context, family: String) {
        let (defs, ok) = crate::fonts::build(&family, &self.fallback_fonts);
        ctx.set_fonts(defs);
        if !ok {
            self.msg(tf!("No se pudo cargar «{family}»; se usa {crate_fonts_DEFAULT}", family = family, crate_fonts_DEFAULT = crate::fonts::DEFAULT));
        }
        self.text_font = family;
        for t in &mut self.tabs {
            t.content_width = 0.0;
        }
        self.persist();
    }

    /// Escape cierra la ventana emergente que está delante (si el foco no
    /// está en el editor, donde Escape quita la selección).
    fn escape_closes_popup(&mut self, ctx: &Context) {
        if !ctx.input(|i| i.key_pressed(Key::Escape)) {
            return;
        }
        // La de actualizar se abre desde el menú con el foco aún en el texto:
        // Escape la cierra igual (si no está instalando).
        if self.updater.as_ref().is_some_and(|u| u.stage() != crate::update::Stage::Running) {
            self.updater = None;
            return;
        }
        if self.view_focused {
            return;
        }
        let inline: Vec<Id> = self.advs.iter().filter(|w| !w.detached).map(|w| adv_window_id(w.id)).collect();
        let open: Vec<Id> = [
            self.show_keys.then(keys_window_id),
            self.show_settings.then(settings_window_id),
            (self.map.open && !self.map.detached).then(crate::diskmap::window_id),
            self.folder.open.then(crate::foldersearch::window_id),
        ]
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
        } else if target == crate::diskmap::window_id() {
            self.map.open = false;

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
        Window::new(t!("Actualizar buscador"))
            .id(update_window_id())
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([560.0, 360.0])
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(tf!("Versión instalada: {crate_update_version_label}", crate_update_version_label = crate::update::version_label()));
                ui.add_space(6.0);
                match &stage {
                    Stage::Checking => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t!("Comprobando la última versión en GitHub…"));
                        });
                    }
                    Stage::UpToDate(h) => {
                        ui.label(tf!("✔ Ya tienes la última versión ({h}).", h = h));
                        run = ui.button(t!("Reinstalar de todos modos")).clicked();
                    }
                    Stage::Available(h) => {
                        ui.strong(tf!("Hay una versión nueva: {h}", h = h));
                        ui.label(t!("Se descarga el código a una carpeta temporal, se compila (la primera vez tarda unos minutos) y se reemplaza la app instalada. Puedes seguir trabajando mientras tanto."));
                        run = ui.button(t!("⬇ Descargar, compilar e instalar")).clicked();
                    }
                    Stage::Running => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t!("Descargando, compilando e instalando…"));
                        });
                    }
                    Stage::Done(p) => {
                        ui.strong(tf!("✔ Instalada en {display}", display = p.display()));
                        restart = ui
                            .button(t!("⟳ Reiniciar ahora"))
                            .on_hover_text(t!("Cierra buscador y lo abre con la versión nueva (recupera los archivos abiertos)"))
                            .clicked();
                        if dirty {
                            ui.weak(t!("Hay cambios sin guardar: al reiniciar se preguntará si descartarlos (o cancela y guárdalos)."));
                        }
                    }
                    Stage::Failed(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, tf!("No se pudo actualizar: {e}", e = e));
                        run = ui.button(t!("Reintentar")).clicked();
                    }
                }
                let log = u.log.lock().unwrap();
                if !log.is_empty() {
                    ui.add_space(6.0);
                    ui.weak(t!("Registro:"));
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
                // Se cierra como siempre (con la confirmación si hay cambios) y la
                // versión nueva se abre en `on_exit`, cuando esta ya terminó.
                self.relaunch = Some(p.clone());
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
        let running = stage == Stage::Running;
        if !open && !running {
            self.updater = None;
        } else if !open && running {
            self.msg(t!("La actualización sigue en segundo plano; vuelve a abrirla desde Ajustes"));
        }
    }

    fn keys_window(&mut self, ctx: &Context) {
        if !self.show_keys {
            return;
        }
        let mut open = true;
        Window::new(t!("Atajos de teclado"))
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

    /// Abre «Buscar en carpeta»: propone la carpeta del archivo activo y el
    /// texto de «Buscar».
    fn show_folder_search(&mut self) {
        let dir = self.tabs.get(self.active).filter(|t| !t.untitled).and_then(|t| t.doc.path.parent().map(|p| p.to_path_buf()));
        if self.folder.query.is_empty() {
            self.folder.query = self.query.clone();
            self.folder.regex = self.use_regex;
            self.folder.case = self.case_sensitive;
        }
        self.folder.show(dir.as_deref());
    }

    /// Ventana «Buscar en carpeta» (ver `foldersearch`).
    fn folder_window(&mut self, ctx: &Context) {
        if !self.folder.open {
            if self.folder.is_busy() {
                self.folder.stop();
            }
            return;
        }
        let mut open = true;
        let mut out = crate::foldersearch::Out::default();
        Window::new(t!("Buscar en carpeta"))
            .id(crate::foldersearch::window_id())
            .open(&mut open)
            .default_size([860.0, 560.0])
            .min_size([560.0, 300.0])
            .max_width(ctx.content_rect().width() * 0.95)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| out = self.folder.ui(ui));
        self.folder.open &= open;
        if out.pick_folder && self.dialog.is_none() {
            let mut d = rfd::AsyncFileDialog::new().set_title("Carpeta donde buscar");
            let cur = std::path::PathBuf::from(self.folder.dir.trim());
            if cur.is_dir() {
                d = d.set_directory(cur);
            }
            let fut = d.pick_folder();
            self.dialog = Some(PendingDialog::spawn(&self.ctx, DialogKind::SearchFolder, async move { fut.await.map(|h| vec![h.path().to_path_buf()]) }));
        }
        if let Some((path, line)) = out.open {
            let (q, re, cs) = self.folder.job.as_ref().map(|j| (j.query.clone(), j.regex, j.case)).unwrap_or_default();
            self.open(path.clone());
            if let Some(i) = self.tabs.iter().position(|t| t.doc.path == path) {
                self.active = i;
                let t = &mut self.tabs[i];
                let p = Pos::new(line, 0);
                if t.doc.indexing_done() || t.total() > line + 200 {
                    t.select(p, p);
                    t.center_on = Some(line);
                } else {
                    t.restore = Some((line.saturating_sub(10), p));
                }
                // Resaltar lo buscado en el archivo, sin saltar a la primera.
                if !q.is_empty() {
                    (self.query, self.use_regex, self.case_sensitive) = (q, re, cs);
                    self.search_or_next();
                    if let Some(t) = self.tabs.get_mut(self.active) {
                        t.pending_first = false;
                    }
                }
                self.focus_view = true;
            }
        }
    }

    /// Ventana «Mapa de disco» (ver `diskmap`).
    fn map_window(&mut self, ctx: &Context) {
        if !self.map.open {
            if self.map.is_busy() {
                self.map.stop(); // al cerrarla se cancela lo que esté en marcha
            }
            return;
        }
        let mut open = true;
        let mut out = crate::diskmap::Out::default();
        if self.map.detached {
            let map = &mut self.map;
            let builder = ViewportBuilder::default()
                .with_title("Mapa de disco")
                .with_inner_size([1100.0, 720.0])
                .with_min_inner_size([640.0, 420.0]);
            ctx.show_viewport_immediate(ViewportId::from_hash_of("mapa_disco"), builder, |ui, _| {
                CentralPanel::default().show(ui, |ui| out = map.ui(ui));
                // Cerrar la ventana del sistema o Escape la cierran.
                if ui.ctx().input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
                    open = false;
                }
            });
        } else {
            Window::new(t!("Mapa de disco"))
                .id(crate::diskmap::window_id())
                .open(&mut open)
                .default_size([1000.0, 680.0])
                .min_size([640.0, 420.0])
                .max_width(ctx.content_rect().width() * 0.95)
                .resizable(true)
                .collapsible(false)
                .show(ctx, |ui| out = self.map.ui(ui));
        }
        self.map.open &= open;
        if out.pick_folder && self.dialog.is_none() {
            let fut = rfd::AsyncFileDialog::new().set_title("Carpeta para el mapa de disco").pick_folder();
            self.dialog = Some(PendingDialog::spawn(&self.ctx, DialogKind::Folder, async move { fut.await.map(|h| vec![h.path().to_path_buf()]) }));
        }
        if let Some(m) = out.msg {
            self.msg(m);
        }
        if let Some(p) = out.reveal {
            if let Err(e) = reveal_in_file_manager(&p) {
                self.msg(tf!("No se pudo abrir la carpeta: {e}", e = e));
            }
        }
        if let Some(t) = out.copy {
            ctx.copy_text(t);
        }
        if !out.open.is_empty() {
            let n = out.open.len();
            if let Some((q, case)) = &out.needle {
                self.query = q.clone();
                self.use_regex = false;
                self.case_sensitive = *case;
            }
            // Se abren todos y se busca el texto en cada uno; queda activo el primero.
            let mut first = None;
            for p in out.open {
                self.open(p.clone());
                if let Some(i) = self.tabs.iter().position(|t| t.doc.path == p) {
                    first.get_or_insert(i);
                    if out.needle.is_some() {
                        self.active = i;
                        self.search_or_next();
                    }
                }
            }
            if let Some(i) = first {
                self.active = i;
            }
            self.map.open = false;
            self.focus_view = true;
            // Desde la ventana aparte: traer la principal al frente.
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
            self.msg(if n == 1 { t!("Archivo abierto desde el mapa").to_string() } else { tf!("{n} archivos abiertos desde el mapa", n = n) });
        }
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
        ui.label(t!("Archivo:"));
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
            if ui.button(t!("+ Nueva")).on_hover_text(t!("Abrir otra ventana de búsqueda avanzada")).clicked() {
                out.new_window = true;
            }
            let label = if w.detached { t!("Integrar") } else { t!("Ventana aparte") };
            let hint = if w.detached {
                t!("Volver a mostrarla dentro de la ventana principal")
            } else {
                t!("Mostrarla en una ventana propia (puedes llevarla a otro monitor)")
            };
            if ui.button(label).on_hover_text(hint).clicked() {
                w.detached = !w.detached;
            }
        });
    });
    let Some(ti) = w.tab_index(tabs, active) else {
        ui.weak(t!("Abre un archivo primero (o el archivo elegido se cerró)."));
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
        ui.label(t!("Desde:"));
        ui.horizontal(|ui| {
            let r = field(ui, &mut w.from, t!("inicio del bloque (texto o regex)"), w.multiline);
            if std::mem::take(&mut w.focus) {
                r.request_focus();
            }
            run |= enter(ui, &r, w.multiline);
            ui.toggle_value(&mut w.case_sensitive, "Aa").on_hover_text(t!("Distinguir mayúsculas"));
            ui.toggle_value(&mut w.regex, ".*").on_hover_text(t!("Expresión regular"));
            ui.toggle_value(&mut w.multiline, t!("Multilínea")).on_hover_text(
                t!("Cada patrón puede ocupar varias líneas (hasta 1 MB).\nTexto: escribe los saltos tal cual (sirve para LF y CRLF); busca con Ctrl+Enter.\nRegex: usa \\n o \\r?\\n; (?s) para que «.» cruce líneas."),
            );
        });
        ui.end_row();
        ui.label(t!("Hasta:"));
        let r = field(ui, &mut w.to, t!("fin del bloque, puede estar en otra línea (opcional)"), w.multiline);
        run |= enter(ui, &r, w.multiline);
        ui.end_row();
    });
    ui.horizontal(|ui| {
        run |= ui.button(t!("Buscar")).clicked();
        if let Some(e) = &w.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        } else if let Some((s, edit_gen, _)) = &w.search {
            let n = s.count();
            let pos = w.cur.map(|i| format!("{}/", fmt_num(i as u64 + 1))).unwrap_or_default();
            let mut t = tf!("{pos}{fmt_num} bloques", pos = pos, fmt_num = fmt_num(n as u64));
            if s.truncated.load(Ordering::Relaxed) {
                t += &tf!(" (límite {fmt_num})", fmt_num = fmt_num(MAX_BLOCKS as u64));
            }
            if !s.is_done() {
                let p = s.progress.load(Ordering::Relaxed) as f64 / s.total.max(1) as f64;
                t += &tf!(" · buscando {v}%", v = format!("{:.0}", p * 100.0));
            } else {
                t += &format!(" · {:.2}s", s.elapsed.lock().unwrap().as_secs_f32());
                if s.unclosed.load(Ordering::Relaxed) > 0 {
                    t += t!(" · el último inicio no tiene fin");
                }
            }
            ui.label(t);
            if *edit_gen != tabs[ti].edit_gen {
                ui.colored_label(ui.visuals().warn_fg_color, t!("desactualizada (Buscar para repetir)"));
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
        ui.label(t!("Copiar:"));
        ui.radio_value(&mut w.mode, CopyMode::Full, if has_end { t!("desde… hasta (incluidos)") } else { "coincidencia" });
        ui.add_enabled_ui(has_end, |ui| {
            ui.radio_value(&mut w.mode, CopyMode::Inner, t!("solo lo de en medio")).on_hover_text(t!("Sin los textos de «Desde» y «Hasta»"));
        });
        ui.radio_value(&mut w.mode, CopyMode::Lines, t!("líneas completas"));
    });
    if !has_end && w.mode == CopyMode::Inner {
        w.mode = CopyMode::Full;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(t!("Separar bloques con:"));
        ui.radio_value(&mut w.sep, Sep::Newline, t!("salto de línea"));
        ui.radio_value(&mut w.sep, Sep::Blank, t!("línea en blanco"));
        ui.radio_value(&mut w.sep, Sep::Nothing, t!("nada"));
    });
    let count = w.count();
    let tab = &mut tabs[ti];
    let mut go = None;
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(count > 0, Button::new("◀")).on_hover_text(t!("Bloque anterior")).clicked() {
            go = Some(w.cur.map_or(count - 1, |i| if i == 0 { count - 1 } else { i - 1 }));
        }
        if ui.add_enabled(count > 0, Button::new("▶")).on_hover_text(t!("Bloque siguiente")).clicked() {
            go = Some(w.cur.map_or(0, |i| (i + 1) % count));
        }
        let cur = w.cur.filter(|&i| i < count);
        if ui
            .add_enabled(cur.is_some(), Button::new(t!("Seleccionar en el editor")))
            .on_hover_text(t!("Selecciona el bloque actual (según «Copiar») para copiarlo o editarlo"))
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
            let Some((s, _, _)) = &w.search else { return t!("Busca algo primero").into() };
            if only.is_none() && !s.is_done() {
                return t!("Espera a que termine la búsqueda").into();
            }
            let all = s.blocks.lock().unwrap();
            let list = match only {
                Some(i) => all.get(i..=i).unwrap_or(&[]),
                None => &all[..],
            };
            match tab.blocks_text(list, w.mode, w.sep) {
                Ok((t, n)) => {
                    let m = tf!("Copiados {fmt_num} bloques ({len})", fmt_num = fmt_num(n as u64), len = human_size(t.len() as u64));
                    ctx.copy_text(t);
                    m
                }
                Err(e) => e,
            }
        };
        if ui.add_enabled(cur.is_some(), Button::new(t!("Copiar bloque"))).clicked() {
            out.msg = Some(copy(cur));
        }
        if ui.add_enabled(count > 0, Button::new(tf!("Copiar todos ({fmt_num})", fmt_num = fmt_num(count as u64)))).clicked() {
            out.msg = Some(copy(None));
        }
        if ui
            .add_enabled(count > 0, Button::new(t!("📝 A archivo nuevo")))
            .on_hover_text(t!("Abre un archivo nuevo con todos los bloques (según «Copiar» y el separador), listo para guardar"))
            .clicked()
        {
            if let Some((s, _, _)) = &w.search {
                if !s.is_done() {
                    out.msg = Some(t!("Espera a que termine la búsqueda").into());
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

    let r = resp.clone().on_hover_text(t!("Verde: bloques encontrados (más intenso = más bloques) · recuadro: lo que se ve · clic: ir allí"));
    if r.clicked() || r.dragged() {
        let p = r.interact_pointer_pos()?;
        let frac = ((p.y - inner.top()) / inner.height()).clamp(0.0, 1.0) as f64;
        return Some(((frac * total as f64) as u64).min(total - 1));
    }
    None
}

/// Lista virtual de líneas con coincidencias.
/// Ancho de la franja del minimapa (1 px por columna) y alto de cada línea.
const MINIMAP_W: f32 = 110.0;
const MINIMAP_ROW: f32 = 2.0;

/// Lo que determina el dibujo del minimapa (si no cambia, se reutiliza).
#[derive(Clone, PartialEq)]
struct MiniKey {
    first: u64,
    rows: usize,
    version: u64,
    edit_gen: u64,
    lang: syntax::Lang,
    syntax_on: bool,
    dark: bool,
    rect: [i32; 4],
    len: usize,
}

/// Minimapa: cada línea es una tira de 2 px con un píxel por carácter
/// (coloreado como el texto), las líneas con coincidencias marcadas y un
/// recuadro con lo que se ve. Con más líneas de las que caben, muestra una
/// ventana que se desplaza en proporción. Solo lee las líneas que dibuja.
fn text_minimap(ui: &mut Ui, tab: &mut Tab, syntax_on: bool) {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let v = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, v.extreme_bg_color);
    painter.vline(rect.left(), rect.y_range(), v.widgets.noninteractive.bg_stroke);
    let total = tab.total().max(1);
    let n = ((rect.height() / MINIMAP_ROW).floor() as u64).max(1);
    let visible = tab.visible_rows.max(1);
    // Primera línea del minimapa: proporcional a la posición en el archivo.
    let first = if total <= n {
        0
    } else {
        let span = total.saturating_sub(visible).max(1) as f64;
        ((tab.top.min(total) as f64 / span) * (total - n) as f64).round() as u64
    };
    let first = first.min(total.saturating_sub(n));
    let key = MiniKey {
        first,
        rows: n as usize,
        version: tab.version(),
        edit_gen: tab.edit_gen,
        lang: tab.lang,
        syntax_on,
        dark: v.dark_mode,
        rect: [rect.min.x as i32, rect.min.y as i32, rect.max.x as i32, rect.max.y as i32],
        len: tab.doc.len(),
    };
    let mesh = match &tab.minimap_cache {
        Some((k, m)) if *k == key => m.clone(),
        _ => {
            let m = Arc::new(minimap_mesh(tab, rect, first, n as usize, syntax_on, &v));
            tab.minimap_cache = Some((key, m.clone()));
            m
        }
    };
    // Coincidencias de la búsqueda: marca amarilla a la izquierda.
    if let Some(s) = &tab.search {
        let r = s.results.lock().unwrap();
        let a = r.partition_point(|&l| l < first);
        let b = r.partition_point(|&l| l < first + n);
        let mark = Color32::from_rgba_unmultiplied(250, 200, 40, 90);
        for &l in &r[a..b] {
            let y = rect.top() + (l - first) as f32 * MINIMAP_ROW;
            painter.rect_filled(Rect::from_min_size(pos2(rect.left() + 1.0, y), vec2(rect.width() - 1.0, MINIMAP_ROW)), 0.0, mark);
        }
    }
    painter.add(Shape::mesh(mesh));
    // Lo que se ve en el editor.
    let y0 = rect.top() + tab.top.saturating_sub(first) as f32 * MINIMAP_ROW;
    let y1 = (y0 + visible as f32 * MINIMAP_ROW).min(rect.bottom());
    if tab.top + visible > first && tab.top < first + n {
        let view = Rect::from_min_max(pos2(rect.left() + 1.0, y0.max(rect.top())), pos2(rect.right(), y1));
        painter.rect_filled(view, 0.0, v.selection.bg_fill.gamma_multiply(0.25));
        painter.rect_stroke(view, 0.0, Stroke::new(1.0, v.selection.bg_fill), StrokeKind::Inside);
    }
    // Clic o arrastrar: centrar esa línea.
    if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.is_pointer_button_down_on() || resp.clicked()) {
        let line = first + ((p.y - rect.top()) / MINIMAP_ROW).max(0.0) as u64;
        tab.top = line.saturating_sub(visible / 2).min(total.saturating_sub(visible));
        ui.ctx().request_repaint();
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
}

/// Las tiras de texto del minimapa (un rectángulo por tramo del mismo color).
fn minimap_mesh(tab: &Tab, rect: Rect, first: u64, n: usize, syntax_on: bool, v: &Visuals) -> epaint::Mesh {
    let cols = rect.width() as usize - 2;
    let base = v.text_color().gamma_multiply(0.55);
    let mut mesh = epaint::Mesh::default();
    let mut spans = Vec::new();
    for (k, (_, txt, _)) in tab.rows(first, n, cols * 4).into_iter().enumerate() {
        spans.clear();
        if syntax_on {
            syntax::highlight(tab.lang, &txt, syntax::State::Normal, &mut spans);
        }
        let y = rect.top() + k as f32 * MINIMAP_ROW;
        let color_at = |i: usize| {
            spans.iter().find(|s| s.start <= i && i < s.end).map_or(base, |s| syntax::color(s.kind, v.dark_mode).gamma_multiply(0.8))
        };
        let (mut col, mut run): (usize, Option<(usize, Color32)>) = (0, None);
        let mut flush = |run: &mut Option<(usize, Color32)>, end: usize| {
            if let Some((c0, color)) = run.take() {
                let x0 = rect.left() + 2.0 + c0 as f32;
                let r = Rect::from_min_max(pos2(x0, y), pos2(rect.left() + 2.0 + end as f32, y + MINIMAP_ROW - 0.5));
                mesh.add_colored_rect(r, color);
            }
        };
        for (i, ch) in txt.char_indices() {
            if col >= cols {
                break;
            }
            let w = char_disp_width(ch, col);
            if ch.is_whitespace() {
                flush(&mut run, col);
            } else {
                let c = color_at(i);
                if run.is_some_and(|(_, rc)| rc != c) {
                    flush(&mut run, col);
                }
                if run.is_none() {
                    run = Some((col, c));
                }
            }
            col += w;
        }
        flush(&mut run, col.min(cols));
    }
    mesh
}

/// Activa/desactiva la vista filtrada; al activarla sin reglas, usa lo que
/// haya en «Buscar» como primera regla.
fn toggle_filter(t: &mut Tab, query: &str, regex: bool, case: bool) {
    let f = &mut t.filter;
    f.on = !f.on;
    if f.on {
        if !f.has_rules() && !query.is_empty() {
            f.rules = vec![crate::filter::Rule { text: query.to_string(), exclude: false, regex, case }];
        }
        // Empezar donde está el cursor.
        f.sel = t.cursor.map(|c| c.line);
        f.at_bottom = t.follow;
        f.top = u64::MAX; // se coloca al tener el resultado
    }
}

/// Vista filtrada: reglas arriba y, debajo, solo las líneas que las cumplen
/// (con su número original). Doble clic o Enter: ir a esa línea en el archivo.
fn filter_view(ui: &mut Ui, tab: &mut Tab, font: &FontId, colors: Option<(Color32, Color32)>, syntax_on: bool) -> Option<String> {
    use crate::filter::Rule;
    let mut message = None;
    // --- Reglas ---
    Frame::NONE.inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
        let mut remove = None;
        let mut changed = false;
        let n = tab.filter.rules.len();
        for (k, r) in tab.filter.rules.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let (txt, hint) = if r.exclude {
                    (t!("− Excluir"), t!("Se ocultan las líneas que contienen esto (clic: cambiar a Incluir)"))
                } else {
                    (t!("+ Incluir"), t!("Se muestran las líneas que contienen esto (clic: cambiar a Excluir)"))
                };
                let color = if r.exclude { Color32::from_rgb(220, 90, 80) } else { Color32::from_rgb(70, 170, 90) };
                if ui.add(Button::new(RichText::new(txt).color(color).strong()).min_size(vec2(84.0, 0.0))).on_hover_text(hint).clicked() {
                    r.exclude = !r.exclude;
                    changed = true;
                }
                let te = ui.add(
                    TextEdit::singleline(&mut r.text)
                        .desired_width(320.0)
                        .hint_text(if r.regex { t!("expresión regular") } else { "texto" })
                        .font(TextStyle::Monospace),
                );
                changed |= te.changed();
                changed |= ui.toggle_value(&mut r.case, "Aa").on_hover_text(t!("Distinguir mayúsculas")).changed();
                changed |= ui.toggle_value(&mut r.regex, ".*").on_hover_text(t!("Expresión regular")).changed();
                if n > 1 && ui.small_button("×").on_hover_text(t!("Quitar esta regla")).clicked() {
                    remove = Some(k);
                }
            });
        }
        if let Some(k) = remove {
            tab.filter.rules.remove(k);
            changed = true;
        }
        if changed {
            tab.filter.touch();
        }
        ui.horizontal(|ui| {
            if ui.button(t!("+ Regla")).on_hover_text(t!("Varias de Incluir: basta con que coincida una. Las de Excluir quitan líneas.")).clicked() {
                tab.filter.rules.push(Rule::default());
            }
            ui.separator();
            let total = tab.total();
            let f = &tab.filter;
            if let Some(e) = &f.error {
                ui.colored_label(ui.visuals().error_fg_color, tf!("Regla no válida: {e}", e = e));
            } else if !f.ready {
                ui.spinner();
                ui.label(tf!("Filtrando… {v}%", v = format!("{:.0}", f.progress().unwrap_or(0.0) * 100.0)));
            } else {
                let c = f.view.count(total);
                ui.label(tf!("{fmt_num} de {fmt_num2} líneas", fmt_num = fmt_num(c), fmt_num2 = fmt_num(total)))
                    .on_hover_text(t!("Doble clic en una línea (o Enter): ir a ella en el archivo completo"));
                if f.progress().is_some() {
                    ui.spinner();
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(t!("Ver todo")).on_hover_text(t!("Volver al archivo completo (las reglas se guardan)")).clicked() {
                    tab.filter.on = false;
                }
                let ready = tab.filter.ready && tab.filter.error.is_none();
                if ui.add_enabled(ready, Button::new(t!("Nuevo archivo con estas líneas"))).clicked() {
                    match filtered_text(tab) {
                        Ok(t) => tab.export_request = Some(t),
                        Err(e) => message = Some(e),
                    }
                }
                if ui.add_enabled(ready, Button::new(t!("Copiar"))).on_hover_text(t!("Copiar todas las líneas filtradas")).clicked() {
                    message = Some(match filtered_text(tab) {
                        Ok(t) => {
                            let n = t.lines().count();
                            ui.ctx().copy_text(t);
                            tf!("Copiadas {fmt_num} líneas", fmt_num = fmt_num(n as u64))
                        }
                        Err(e) => e,
                    });
                }
            });
        });
    });
    ui.separator();

    // --- Lista ---
    apply_text_colors(ui, colors);
    let ctx = ui.ctx().clone();
    let row_h = ctx.fonts_mut(|f| f.row_height(font)).ceil();
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    let list = Rect::from_min_max(full.min, pos2(full.max.x - SCROLLBAR, full.max.y));
    let vsb = Rect::from_min_max(pos2(list.max.x, full.min.y), full.max);
    let id = ui.scope_id().with("filtrada");
    let resp = ui.interact(list, id, Sense::click());
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(list);
    painter.rect_filled(list, 0.0, visuals.extreme_bg_color);
    let total = tab.total();
    let f = &tab.filter;
    let count = if f.error.is_some() { 0 } else { f.view.count(total) };
    let visible = ((list.height() / row_h).floor() as u64).max(1);
    let max_top = count.saturating_sub(visible);
    let mut top = tab.filter.top;
    let mut sel = tab.filter.sel;
    if !tab.filter.ready && tab.filter.progress().is_some() && count == 0 {
        // Aún sin resultado: no hay nada que pintar.
    } else if top == u64::MAX {
        // Recién activada: llevar a la vista la línea del cursor.
        top = match sel {
            Some(l) => tab.filter.view.row_of(l).saturating_sub(visible / 2),
            None if tab.filter.at_bottom => max_top,
            None => 0,
        };
    }
    if tab.follow && tab.filter.at_bottom {
        top = max_top;
    }
    // Teclado: ↑/↓/Re Pág/Av Pág mueven la línea elegida; Enter va a ella.
    if resp.clicked() || resp.hovered() {
        resp.request_focus();
    }
    let mut goto = None;
    if resp.has_focus() && count > 0 {
        let cur = sel.map(|l| tab.filter.view.row_of(l)).unwrap_or(top);
        let step = ui.input(|i| {
            let mut d = 0i64;
            if i.key_pressed(Key::ArrowDown) { d += 1; }
            if i.key_pressed(Key::ArrowUp) { d -= 1; }
            if i.key_pressed(Key::PageDown) { d += visible as i64; }
            if i.key_pressed(Key::PageUp) { d -= visible as i64; }
            d
        });
        if step != 0 {
            let row = add_signed(cur, step).min(count - 1);
            sel = tab.filter.view.line_at(row);
            if row < top {
                top = row;
            } else if row >= top + visible {
                top = row + 1 - visible;
            }
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            goto = sel;
        }
    }
    if resp.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta.y);
        if d != 0.0 {
            top = add_signed(top.min(max_top), (-d / row_h * 3.0).round() as i64);
        }
    }
    top = top.min(max_top);
    let digits = fmt_num(total.max(1)).chars().count().max(4) as f32;
    let num_w = ctx.fonts_mut(|f| f.glyph_width(font, '0')) * digits + 8.0;
    let dark = is_dark(visuals.extreme_bg_color);
    for k in 0..(visible + 1).min(count.saturating_sub(top)) {
        let Some(line) = tab.filter.view.line_at(top + k) else { break };
        let y = list.top() + k as f32 * row_h;
        let row = Rect::from_min_size(pos2(list.left(), y), vec2(list.width(), row_h));
        let r = ui.interact(row.intersect(list), id.with(("fila", k)), Sense::click());
        if sel == Some(line) {
            painter.rect_filled(row, 0.0, visuals.selection.bg_fill.gamma_multiply(0.6));
        } else if r.hovered() {
            painter.rect_filled(row, 0.0, visuals.widgets.hovered.weak_bg_fill);
        }
        if r.clicked() {
            sel = Some(line);
            resp.request_focus();
        }
        if r.double_clicked() {
            goto = Some(line);
        }
        painter.text(pos2(list.left() + num_w, y), Align2::RIGHT_TOP, fmt_num(line + 1), font.clone(), visuals.weak_text_color());
        if let Some((_, txt, tr)) = tab.rows(line, 1, 2048).into_iter().next() {
            let mut spans = Vec::new();
            if syntax_on {
                syntax::highlight(tab.lang, &txt, syntax::State::Normal, &mut spans);
            }
            let job = line_job(&txt, tr, font, visuals.text_color(), tab.filter.highlight.as_ref(), &spans, dark);
            painter.galley(pos2(list.left() + num_w + 12.0, y), painter.layout_job(job), visuals.text_color());
        }
    }
    if count == 0 && tab.filter.ready && tab.filter.error.is_none() {
        painter.text(list.center(), Align2::CENTER_CENTER, t!("Ninguna línea cumple las reglas"), FontId::proportional(14.0), visuals.weak_text_color());
    }
    vscrollbar(ui, vsb, id.with("vsb"), &mut top, count, visible);
    let f = &mut tab.filter;
    f.top = top;
    f.sel = sel;
    if f.ready {
        f.at_bottom = top >= max_top;
    }
    if let Some(l) = goto {
        f.on = false;
        tab.goto_line(l);
    }
    message
}

/// Texto de todas las líneas de la vista filtrada (con límite de tamaño).
fn filtered_text(tab: &Tab) -> Result<String, String> {
    let total = tab.total();
    let count = tab.filter.view.count(total);
    let mut out = String::new();
    for k in 0..count {
        let Some(l) = tab.filter.view.line_at(k) else { break };
        out += &tab.line(l);
        out.push('\n');
        if out.len() > MAX_COPY_BYTES {
            return Err(tf!("Demasiado grande (máx. {human_size})", human_size = human_size(MAX_COPY_BYTES as u64)));
        }
    }
    Ok(out)
}

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
        ui.weak(t!("Selecciona una línea para verla completa aquí."));
        return;
    };
    let version = tab.version();
    if tab.detail_cache.as_ref().map(|c| (c.0, c.1)) != Some((line, version)) {
        let text = match tab.rows(line, 1, MAX_DETAIL_BYTES).into_iter().next() {
            Some((_, mut t, tr)) => {
                if tr {
                    t += t!("\n…[truncado: la línea mide más de 1 MB]");
                }
                t
            }
            None => String::new(),
        };
        tab.detail_cache = Some((line, version, text));
    }
    let text = &tab.detail_cache.as_ref().unwrap().2;
    ui.label(RichText::new(tf!("Línea {fmt_num}", fmt_num = fmt_num(line + 1))).small().weak());
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

/// «14.6 KB · UTF-8 · 60 líneas» (tooltip del panel de archivos).
/// Posición del puntero en la ventana, también mientras se arrastran archivos
/// desde fuera (en macOS winit no la informa durante el arrastre).
fn os_pointer(ctx: &Context) -> Option<Pos2> {
    #[cfg(target_os = "macos")]
    if let Some(p) = crate::macopen::mouse_in_window(ctx) {
        return Some(p);
    }
    ctx.input(|i| i.pointer.latest_pos())
}

/// Pinta el ⏵ verde de «En vivo» con el borde izquierdo en `at`; devuelve
/// el ancho que ocupa (con su separación).
fn live_icon(ui: &Ui, at: Pos2) -> f32 {
    let g = ui.painter().layout_no_wrap("⏵".into(), FontId::proportional(12.0), Color32::from_rgb(60, 185, 95));
    let w = g.size().x;
    ui.painter().galley(pos2(at.x, at.y - g.size().y / 2.0), g, Color32::from_rgb(60, 185, 95));
    w + 4.0
}

/// Un espacio que se arrastra para cambiarlo de orden.
struct DragSpace(u32);

/// Lo que se arrastra en el panel de archivos: el índice del archivo.
struct DragFile(usize);

/// Una fila del panel de archivos: toda la fila selecciona (salvo la ×), se
/// puede arrastrar a un espacio o a otra posición, y tiene menú contextual.
#[allow(clippy::too_many_arguments)]
fn file_row(ui: &mut Ui, i: usize, t: &Tab, active: bool, multi: usize, spaces: &[Space], indent: f32, total: usize, action: &mut Option<FileAction>) {
    let row = vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
        // Se registra antes que la ×, que queda encima y conserva su propio clic.
        let row_rect = ui.max_rect();
        let r = ui.interact(row_rect, Id::NULL.with(("fila_archivo", i)), Sense::click_and_drag());
        r.dnd_set_drag_payload(DragFile(i));
        let v = ui.visuals();
        let dragged = r.dnd_hover_payload::<DragFile>().map(|d| d.0).filter(|&d| d != i);
        let fill = if active {
            Some(v.selection.bg_fill)
        } else if multi > 0 {
            Some(v.selection.bg_fill.gamma_multiply(0.5))
        } else if r.contains_pointer() {
            Some(v.widgets.hovered.weak_bg_fill)
        } else {
            None
        };
        if let Some(f) = fill {
            ui.painter().rect_filled(row_rect, v.widgets.inactive.corner_radius, f);
        }
        // Marca a la izquierda de los seleccionados (varios).
        if multi > 0 {
            let bar = Rect::from_min_size(row_rect.min + vec2(1.0, 3.0), vec2(3.0, row_rect.height() - 6.0));
            ui.painter().rect_filled(bar, 1.5, v.selection.stroke.color);
        }
        // Línea donde quedará: debajo si se arrastra hacia abajo, encima si hacia arriba.
        if let Some(d) = dragged {
            let y = if d < i { row_rect.bottom() } else { row_rect.top() };
            ui.painter().hline(row_rect.x_range(), y, Stroke::new(2.0, v.selection.stroke.color));
        }
        let ink = if active || multi > 0 { v.selection.stroke.color } else { v.strong_text_color() };
        if ui.small_button("×").on_hover_text(t!("Cerrar (Ctrl+W)")).clicked() {
            *action = Some(FileAction::Close(i));
        }
        let name = if t.dirty() { format!("● {}", t.doc.name) } else { t.doc.name.clone() };
        let free = ui.available_rect_before_wrap();
        let mut left = row_rect.left() + 6.0 + indent + if multi > 0 { 4.0 } else { 0.0 };
        // «En vivo»: ⏵ verde antes del nombre.
        if t.follow {
            left += live_icon(ui, pos2(left, row_rect.center().y));
        }
        let galley = WidgetText::from(RichText::new(name).strong()).into_galley(
            ui,
            Some(TextWrapMode::Truncate),
            (free.right() - left - 2.0).max(10.0),
            TextStyle::Button,
        );
        let pos = pos2(left, row_rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, ink);
        if r.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        if let Some(d) = r.dnd_release_payload::<DragFile>() {
            if d.0 != i {
                *action = Some(FileAction::MoveBefore(d.0, i));
            }
        }
        let r = r.on_hover_text(tf!("{to_string}\nClic derecho: más opciones · Arrastrar: mover a un espacio\n{if_MAC_Cmd_else_Ctrl} + clic / Shift + clic: seleccionar varios\n{tab_details}", to_string = if t.untitled { t!("Archivo nuevo sin guardar").to_string() } else { t.doc.path.display().to_string() }, if_MAC_Cmd_else_Ctrl = if MAC { "Cmd" } else { "Ctrl" }, tab_details = tab_details(t)));
        if r.clicked() {
            *action = Some(FileAction::Click(i, ui.input(|i| i.modifiers)));
        }
        r.context_menu(|ui| {
            let mut chosen = None;
            if multi > 0 {
                // Menú para todos los seleccionados.
                ui.weak(tf!("{multi} archivos seleccionados", multi = multi));
                ui.separator();
                ui.menu_button(tf!("Mover los {multi} a un espacio", multi = multi), |ui| {
                    for sp in spaces {
                        if ui.button(&sp.name).clicked() {
                            chosen = Some(FileAction::ToSpace(i, Some(sp.id)));
                        }
                    }
                    if !spaces.is_empty() {
                        ui.separator();
                    }
                    if ui.button(t!("Nuevo espacio…")).clicked() {
                        chosen = Some(FileAction::NewSpace(Some(i)));
                    }
                    if ui.button(t!("Sacar de los espacios")).clicked() {
                        chosen = Some(FileAction::ToSpace(i, None));
                    }
                });
                ui.separator();
                if ui.button(tf!("Cerrar los {multi}", multi = multi)).clicked() {
                    chosen = Some(FileAction::CloseSel(i));
                }
                if ui.button(t!("Quitar la selección")).clicked() {
                    chosen = Some(FileAction::Unpick);
                }
                if chosen.is_some() {
                    *action = chosen;
                    ui.close();
                }
                return;
            }
            let on_disk = !t.untitled;
            let nw = ui.add_enabled(on_disk, Button::new(t!("Abrir en una ventana nueva")));
            let nw = if t.dirty() { nw.on_hover_text(t!("La ventana nueva muestra la versión guardada en disco")) } else { nw };
            if nw.clicked() {
                chosen = Some(FileAction::NewWindow(i));
            }
            ui.separator();
            if ui.add_enabled(on_disk, Button::new(reveal_label())).clicked() {
                chosen = Some(FileAction::Reveal(i));
            }
            if ui.add_enabled(on_disk, Button::new(t!("Copiar ruta"))).clicked() {
                chosen = Some(FileAction::CopyPath(i));
            }
            if ui.button(t!("Copiar nombre")).clicked() {
                chosen = Some(FileAction::CopyName(i));
            }
            ui.separator();
            ui.menu_button(t!("Mover a un espacio"), |ui| {
                for sp in spaces {
                    if ui.add_enabled(t.space != Some(sp.id), Button::new(&sp.name)).clicked() {
                        chosen = Some(FileAction::ToSpace(i, Some(sp.id)));
                    }
                }
                if !spaces.is_empty() {
                    ui.separator();
                }
                if ui.button(t!("Nuevo espacio…")).clicked() {
                    chosen = Some(FileAction::NewSpace(Some(i)));
                }
                if t.space.is_some() && ui.button(t!("Sacar del espacio")).clicked() {
                    chosen = Some(FileAction::ToSpace(i, None));
                }
            });
            ui.separator();
            let dup = ui.add_enabled(on_disk, Button::new(t!("Duplicar")));
            let dup = if t.dirty() { dup.on_hover_text(t!("La copia incluye los cambios sin guardar; el original no se modifica")) } else { dup };
            if dup.clicked() {
                chosen = Some(FileAction::Duplicate(i));
            }
            ui.separator();
            if ui.button(t!("Cerrar")).clicked() {
                chosen = Some(FileAction::Close(i));
            }
            if ui.add_enabled(total > 1, Button::new(t!("Cerrar los demás"))).clicked() {
                chosen = Some(FileAction::CloseOthers(i));
            }
            if chosen.is_some() {
                *action = chosen;
                ui.close();
            }
        });
    });
    ui.add_space(2.0);
}

fn tab_details(t: &Tab) -> String {
    let idx = if t.doc.indexing_done() {
        tf!("{total} líneas", total = fmt_num(t.total()))
    } else {
        let p = t.doc.progress.load(Ordering::Relaxed) as f64 / t.doc.len().max(1) as f64;
        tf!("indexando {v}%", v = format!("{:.0}", p * 100.0))
    };
    format!(
        "{} · {} · {}{}{}",
        human_size(t.doc.len() as u64),
        t.doc.src.encoding.name(),
        idx,
        if t.editable { "" } else { t!(" · solo lectura") },
        if t.follow { t!("\n⏵ En vivo: se muestra lo que se añade al archivo") } else { "" }
    )
}

pub(crate) fn fmt_num(n: u64) -> String {
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

pub(crate) fn human_size(b: u64) -> String {
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

    pub(super) fn tab_with(name: &str, text: &str) -> (Tab, PathBuf) {
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
        assert!(!t.dirty());
        assert_eq!(std::fs::read(&p).unwrap(), "¡Adiósmundo\r\nsegunda\r\n".as_bytes());
        assert!(t.start_save(p.clone()).is_err(), "sin cambios no se guarda");
        // Deshacer después de guardar: vuelve el texto anterior y queda modificado;
        // guardar otra vez escribe ese texto (leído del contenido de antes).
        key(&mut t, Key::Z, Modifiers::COMMAND);
        assert_eq!(t.line(1), "\tsegunda");
        assert!(t.dirty());
        t.start_save(p.clone()).unwrap();
        while t.poll_save().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(std::fs::read(&p).unwrap(), "¡Adiósmundo\r\n\tsegunda\r\n".as_bytes());
        key(&mut t, Key::Z, Modifiers::SHIFT | Modifiers::COMMAND);
        assert_eq!(t.line(1), "segunda");
        assert!(t.dirty());
    }

    #[test]
    fn tab_indents_selected_lines() {
        let (mut t, _p) = tab_with("sangria.txt", "uno\ndos\n\n    tres\ncuatro\n");
        t.editable = true;
        // Selección parcial de «no» a «tr»: Tab desplaza las líneas 0–3 (la vacía no).
        t.select(Pos::new(0, 1), Pos::new(3, 6));
        key(&mut t, Key::Tab, Modifiers::NONE);
        assert_eq!([t.line(0), t.line(1), t.line(2), t.line(3), t.line(4)], ["\tuno", "\tdos", "", "\t    tres", "cuatro"]);
        assert_eq!(t.selection(), Some((Pos::new(0, 0), Pos::new(3, 9))));
        // Shift+Tab dos veces: quita el tabulador y luego 4 espacios.
        key(&mut t, Key::Tab, Modifiers::SHIFT);
        key(&mut t, Key::Tab, Modifiers::SHIFT);
        assert_eq!([t.line(0), t.line(1), t.line(3)], ["uno", "dos", "tres"]);
        // Un solo deshacer por paso.
        key(&mut t, Key::Z, Modifiers::COMMAND);
        assert_eq!(t.line(3), "    tres");
        // Selección que acaba al inicio de una línea: esa línea no se toca.
        t.select(Pos::new(3, 0), Pos::new(4, 0));
        key(&mut t, Key::Tab, Modifiers::NONE);
        assert_eq!([t.line(3), t.line(4)], ["\t    tres", "cuatro"]);
        // Sin selección, Tab sigue insertando un tabulador en el cursor.
        t.select(Pos::new(4, 3), Pos::new(4, 3));
        key(&mut t, Key::Tab, Modifiers::NONE);
        assert_eq!(t.line(4), "cua\ttro");
    }

    #[test]
    fn trim_trailing_spaces() {
        let (mut t, _p) = tab_with("finales.txt", "uno  \ndos\t \t\ntres\ncuatro \n");
        t.editable = true;
        t.select(Pos::new(0, 2), Pos::new(2, 1));
        let m = t.trim_trailing().unwrap();
        assert!(m.contains('2'), "{m}");
        assert_eq!([t.line(0), t.line(1), t.line(2), t.line(3)], ["uno", "dos", "tres", "cuatro "]);
        t.undo_redo(false).unwrap();
        assert_eq!(t.line(1), "dos\t \t");
    }

    #[test]
    fn tab_moves_block() {
        let (mut t, _p) = tab_with("bloque_tab.txt", "ab cd\nef gh\nij\n");
        t.editable = true;
        t.col_mode = true;
        // Bloque sobre «cd»/«gh» (columnas 3–5) en las líneas 0–1.
        t.colsel = Some(ColSel { a: (0, 3), b: (1, 5) });
        key(&mut t, Key::Tab, Modifiers::NONE);
        assert_eq!([t.line(0), t.line(1), t.line(2)], ["ab \tcd", "ef \tgh", "ij"]);
        assert_eq!(t.colsel, Some(ColSel { a: (0, 4), b: (1, 6) }));
        assert_eq!(t.column_text(t.colsel.unwrap(), usize::MAX).unwrap(), "cd\ngh");
        // Otra vez: a la columna 8.
        key(&mut t, Key::Tab, Modifiers::NONE);
        assert_eq!(t.colsel, Some(ColSel { a: (0, 8), b: (1, 10) }));
        assert_eq!(t.column_text(t.colsel.unwrap(), usize::MAX).unwrap(), "cd\ngh");
        // Mayús+Tab: vuelve a la 4 y luego a la 3… no: a la 0 quitando el espacio.
        key(&mut t, Key::Tab, Modifiers::SHIFT);
        assert_eq!(t.colsel, Some(ColSel { a: (0, 4), b: (1, 6) }));
        assert_eq!(t.line(0), "ab \tcd");
        key(&mut t, Key::Tab, Modifiers::SHIFT);
        assert_eq!(t.line(0), "abcd");
        assert_eq!(t.column_text(t.colsel.unwrap(), usize::MAX).unwrap(), "cd\ngh");
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
        std::fs::create_dir_all(&dir).unwrap(); // al guardar se quitó la carpeta vacía
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
        // Paradas fijas: el tab llega a la siguiente columna múltiplo del ancho.
        assert_eq!(disp_col("ab\tc", 3), 4);
        assert_eq!(disp_col("abcd\tx", 5), 8);
        assert_eq!(col_from_disp("ab\tc", 4), 3);
        assert_eq!(sanitize("ab\tc"), "ab  c");
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
                log_view(ui, &mut t, &font, &mut show, false, None, true, Wrap::Off);
            });
        }
        t
    }

    /// Arrastre del ratón de `from` a `to` (en puntos), como fotogramas.
    fn drag_frames(from: Pos2, to: Pos2) -> Vec<Vec<Event>> {
        let mid = from + (to - from) * 0.5;
        vec![
            vec![Event::PointerMoved(from)],
            vec![Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
            vec![Event::PointerMoved(from + vec2(8.0, 1.0))],
            vec![Event::PointerMoved(mid)],
            vec![Event::PointerMoved(to)],
            vec![Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }],
            vec![],
        ]
    }

    fn run_frames(t: &mut Tab, ctx: &Context, frames: Vec<Vec<Event>>, t0: &mut f64) {
        let font = FontId::monospace(13.0);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let mut show = false;
        for events in frames {
            *t0 += 0.05;
            let input = RawInput { screen_rect: Some(screen), events, time: Some(*t0), ..Default::default() };
            let _ = ctx.run_ui(input, |ui| {
                log_view(ui, t, &font, &mut show, false, None, true, Wrap::Off);
            });
        }
    }

    #[test]
    fn block_twice_copy_and_delete() {
        let mut text = String::new();
        for i in 0..20 {
            text += &format!("{i:02}-abcdefghijklmnopqrstuvwxyz\n");
        }
        let (mut t, _p) = super::tests::tab_with("bloque2.txt", &text);
        t.editable = true;
        t.col_mode = true;
        let ctx = Context::default();
        let mut time = 0.0;
        // Primer bloque.
        run_frames(&mut t, &ctx, drag_frames(pos2(120.0, 10.0), pos2(200.0, 60.0)), &mut time);
        let first = t.block().expect("primer bloque");
        // Segundo bloque, en otro sitio.
        run_frames(&mut t, &ctx, drag_frames(pos2(150.0, 120.0), pos2(260.0, 200.0)), &mut time);
        let second = t.block().expect("segundo bloque");
        assert_ne!(first, second);
        let (l0, l1) = second.lines();
        assert!(l0 > first.lines().1, "el segundo bloque empieza donde se pulsó: {second:?}");
        let copied = t.column_text(second, usize::MAX).unwrap();
        assert_eq!(copied.lines().count() as u64, l1 - l0 + 1);
        let width = second.cols().1 - second.cols().0;
        assert!(copied.lines().all(|l| l.chars().count() == width), "{copied:?}");
        // Suprimir borra el bloque (en un paso que se deshace).
        let before: Vec<String> = (l0..=l1).map(|n| t.line(n)).collect();
        run_frames(&mut t, &ctx, vec![vec![key(Key::Delete, Modifiers::NONE, true)], vec![]], &mut time);
        for (k, n) in (l0..=l1).enumerate() {
            let line = t.line(n);
            assert_eq!(line.chars().count(), before[k].chars().count() - width, "línea {n}: {line}");
        }
        assert!(t.block().is_none());
        run_frames(&mut t, &ctx, vec![vec![key(Key::Z, Modifiers::COMMAND, true)], vec![]], &mut time);
        assert_eq!(t.line(l0), before[0]);
    }

    #[test]
    fn tab_after_several_selections() {
        let mut text = String::new();
        for i in 0..20 {
            text += &format!("linea {i:02} texto\n");
        }
        let (mut t, _p) = super::tests::tab_with("tab_varias.txt", &text);
        t.editable = true;
        let ctx = Context::default();
        let mut time = 0.0;
        let tab_key = || vec![vec![key(Key::Tab, Modifiers::NONE, true), key(Key::Tab, Modifiers::NONE, false)], vec![]];
        for (k, (y0, y1)) in [(20.0, 70.0), (120.0, 170.0), (40.0, 150.0)].into_iter().enumerate() {
            run_frames(&mut t, &ctx, drag_frames(pos2(140.0, y0), pos2(200.0, y1)), &mut time);
            let (a, b) = t.selection().unwrap_or_else(|| panic!("selección {k}: {:?} {:?}", t.anchor, t.cursor));
            let before: Vec<String> = (0..20).map(|n| t.line(n)).collect();
            run_frames(&mut t, &ctx, tab_key(), &mut time);
            let last = if b.col == 0 && b.line > a.line { b.line - 1 } else { b.line };
            for n in 0..20u64 {
                let want = if (a.line..=last).contains(&n) { format!("\t{}", before[n as usize]) } else { before[n as usize].clone() };
                assert_eq!(t.line(n), want, "selección {k} ({a:?}–{b:?}), línea {n}");
            }
        }
    }

    /// Selección con clic + Mayús+clic y Tab en el fotograma siguiente (sin
    /// fotogramas vacíos entre medias, como pasa de verdad: egui solo repinta
    /// cuando hay eventos).
    #[test]
    fn tab_right_after_shift_click() {
        let mut text = String::new();
        for i in 0..20 {
            text += &format!("linea {i:02} texto\n");
        }
        let (mut t, _p) = super::tests::tab_with("tab_shift_clic.txt", &text);
        t.editable = true;
        let ctx = Context::default();
        let mut time = 0.0;
        let click = |p: Pos2, m: Modifiers| {
            vec![
                vec![Event::ModifiersChanged(m), Event::PointerMoved(p)],
                vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: m }],
                vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: m }],
            ]
        };
        for round in 0..3 {
            let y = 20.0 + round as f32 * 60.0;
            let mut frames = click(pos2(140.0, y), Modifiers::NONE);
            frames.extend(click(pos2(200.0, y + 40.0), Modifiers::SHIFT));
            frames.push(vec![Event::ModifiersChanged(Modifiers::NONE), key(Key::Tab, Modifiers::NONE, true)]);
            frames.push(vec![key(Key::Tab, Modifiers::NONE, false)]);
            let before = t.line(t.selection().map_or(0, |s| s.0.line));
            run_frames(&mut t, &ctx, frames, &mut time);
            let (a, _) = t.selection().unwrap_or_else(|| panic!("vuelta {round}: sin selección"));
            assert!(t.line(a.line).starts_with('\t'), "vuelta {round}: no se sangró ({before:?} → {:?})", t.line(a.line));
            let focused = ctx.memory(|m| m.focused());
            assert!(focused.is_some(), "vuelta {round}: el Tab se llevó el foco");
        }
    }

    #[test]
    fn drag_select_then_ctrl_c() {
        let ctrl_c = || vec![key(Key::C, Modifiers::CTRL, true), key(Key::C, Modifiers::CTRL, false)];
        let cmd_c = || {
            vec![
                key(Key::SuperLeft, Modifiers::NONE, true),
                key(Key::C, Modifiers::MAC_CMD | Modifiers::COMMAND, true),
                Event::Copy,
                key(Key::SuperLeft, Modifiers::NONE, false),
            ]
        };
        for (name, evs) in [("Ctrl+C", ctrl_c()), ("Cmd+C", cmd_c())] {
            let t = drag_then(evs, false);
            assert!(t.selection().is_some(), "{name}: se perdió la selección");
            let t = drag_then(if name == "Ctrl+C" { ctrl_c() } else { cmd_c() }, true);
            assert!(t.block().is_some(), "{name}: se perdió el bloque");
        }
    }

    #[test]
    fn modifier_keys_alone_keep_selection() {
        // Como llegan de macOS: la tecla Cmd sola, todavía sin el modificador activo.
        for k in [Key::SuperLeft, Key::SuperRight, Key::ShiftLeft, Key::AltLeft, Key::ControlLeft] {
            let mut t = tab();
            t.select(Pos::new(0, 0), Pos::new(0, 4));
            t.col_mode = true;
            t.colsel = Some(ColSel { a: (0, 0), b: (1, 4) });
            run(&mut t, vec![key(k, Modifiers::NONE, true), key(k, Modifiers::NONE, false)]);
            assert_eq!(t.selection(), Some((Pos::new(0, 0), Pos::new(0, 4))), "{k:?}: selección");
            assert!(t.block().is_some(), "{k:?}: bloque");
        }
        // Cmd (sola) y luego Cmd+C copia el bloque, no la línea.
        let mut t = tab();
        t.col_mode = true;
        t.colsel = Some(ColSel { a: (0, 0), b: (1, 4) });
        let copied = run(
            &mut t,
            vec![key(Key::SuperLeft, Modifiers::NONE, true), key(Key::C, Modifiers::MAC_CMD | Modifiers::COMMAND, true), Event::Copy],
        );
        assert_eq!(copied.as_deref(), Some("hola
segu"));
    }

    #[test]
    fn wrapped_click_lands_on_the_right_row() {
        let dir = std::env::temp_dir().join("buscador-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("ajuste-clic.txt");
        std::fs::write(&p, format!("{}\nsegunda\n", "abcdefghij".repeat(30))).unwrap();
        let doc = Document::open(&p).unwrap();
        while !doc.indexing_done() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut t = Tab::new(doc);
        let ctx = Context::default();
        let font = FontId::monospace(13.0);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0));
        let _ = ctx.run_ui(RawInput { screen_rect: Some(screen), ..Default::default() }, |_| {});
        let row_h = ctx.fonts_mut(|f| f.row_height(&font).ceil());
        let y2 = row_h * 1.5; // segunda fila visual de la primera línea
        let frames = vec![
            vec![],
            vec![Event::PointerMoved(pos2(120.0, y2))],
            vec![Event::PointerButton { pos: pos2(120.0, y2), button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
            vec![Event::PointerButton { pos: pos2(120.0, y2), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }],
            vec![],
        ];
        let mut show = false;
        for (k, events) in frames.into_iter().enumerate() {
            let input = RawInput { screen_rect: Some(screen), events, time: Some(k as f64 * 0.05), ..Default::default() };
            let _ = ctx.run_ui(input, |ui| {
                log_view(ui, &mut t, &font, &mut show, false, None, true, Wrap::Window);
            });
        }
        let c = t.cursor.expect("cursor");
        assert_eq!(c.line, 0, "el clic en la 2.ª fila sigue siendo la línea 1");
        assert!(c.col > 20, "y cae en mitad de la línea (col {})", c.col);
        assert!(t.lines_shown >= 1 && t.layout_map[0].2 > row_h, "la línea larga ocupa varias filas");
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
