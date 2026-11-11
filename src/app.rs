//! Interfaz gráfica (egui).

use crate::document::Document;
use crate::encoding;
use crate::search::{MAX_RESULTS, Params, Search};
use eframe::egui::{text::LayoutJob, *};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Bytes máximos de una línea que se pintan (el resto se indica como truncado).
const MAX_DISPLAY_BYTES: usize = 16 * 1024;
/// Bytes máximos de una línea en el panel de detalle.
const MAX_DETAIL_BYTES: usize = 1 << 20;
/// Límite de lo que se copia al portapapeles.
const MAX_COPY_BYTES: usize = 256 << 20;
const SCROLLBAR: f32 = 12.0;

pub struct Tab {
    doc: Document,
    top: u64,
    scroll_frac: f32,
    hscroll: f32,
    content_width: f32,
    anchor: Option<u64>,
    cursor: Option<u64>,
    search: Option<Search>,
    search_error: Option<String>,
    /// Línea de la coincidencia actual.
    current: Option<u64>,
    /// Ir a la primera coincidencia en cuanto aparezca.
    pending_first: bool,
    center_on: Option<u64>,
    results_top: u64,
    visible_rows: u64,
    detail_cache: Option<(u64, String)>,
}

impl Tab {
    fn new(doc: Document) -> Self {
        Tab {
            doc,
            top: 0,
            scroll_frac: 0.0,
            hscroll: 0.0,
            content_width: 0.0,
            anchor: None,
            cursor: None,
            search: None,
            search_error: None,
            current: None,
            pending_first: false,
            center_on: None,
            results_top: 0,
            visible_rows: 1,
            detail_cache: None,
        }
    }

    fn selection(&self) -> Option<(u64, u64)> {
        let (a, c) = (self.anchor?, self.cursor?);
        Some((a.min(c), a.max(c)))
    }

    fn goto_line(&mut self, line: u64) {
        self.anchor = Some(line);
        self.cursor = Some(line);
        self.center_on = Some(line);
    }

    fn goto_match(&mut self, line: u64) {
        self.current = Some(line);
        self.goto_line(line);
    }

    fn next_match(&mut self) {
        let Some(s) = &self.search else { return };
        let from = self.current.or(self.cursor).unwrap_or(self.top);
        let inclusive = self.current.is_none() && self.cursor.is_none();
        let hit = s.next_after(from, inclusive).or_else(|| s.get(0).map(|l| (0, l)));
        if let Some((_, l)) = hit {
            self.goto_match(l);
        }
    }

    fn prev_match(&mut self) {
        let Some(s) = &self.search else { return };
        let from = self.current.or(self.cursor).unwrap_or(self.top);
        let n = s.count();
        let hit = s.prev_before(from).or_else(|| n.checked_sub(1).and_then(|i| s.get(i).map(|l| (i, l))));
        if let Some((_, l)) = hit {
            self.goto_match(l);
        }
    }

    fn copy_selection(&self, ctx: &Context) -> String {
        let Some((lo, hi)) = self.selection() else {
            return "Nada seleccionado".into();
        };
        match self.doc.text_of_lines(lo, hi, MAX_COPY_BYTES) {
            Some(t) => {
                let n = hi - lo + 1;
                let bytes = t.len();
                ctx.copy_text(t);
                format!("Copiadas {n} líneas ({})", human_size(bytes as u64))
            }
            None => format!("Selección demasiado grande (> {})", human_size(MAX_COPY_BYTES as u64)),
        }
    }
}

pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    show_sidebar: bool,
    show_results: bool,
    show_detail: bool,
    query: String,
    use_regex: bool,
    case_sensitive: bool,
    goto: String,
    font_size: f32,
    message: Option<(String, Instant)>,
    focus_search: bool,
    focus_goto: bool,
    debug_frames: u64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> Self {
        install_fallback_fonts(&cc.egui_ctx);
        let mut app = App {
            tabs: Vec::new(),
            active: 0,
            show_sidebar: true,
            show_results: true,
            show_detail: false,
            query: String::new(),
            use_regex: false,
            case_sensitive: false,
            goto: String::new(),
            font_size: 13.0,
            message: None,
            focus_search: false,
            focus_goto: false,
            debug_frames: 0,
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
            }
            Err(e) => self.msg(format!("No se pudo abrir {}: {e}", path.display())),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(files) = rfd::FileDialog::new().set_title("Abrir logs").pick_files() {
            for f in files {
                self.open(f);
            }
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
        if tab.search.as_ref().is_some_and(|s| s.params == params) {
            tab.next_match();
            return;
        }
        match Search::start(tab.doc.src.clone(), params) {
            Ok(s) => {
                tab.search = Some(s);
                tab.search_error = None;
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

    fn close_tab(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.tabs.remove(i);
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        let sc = |m, k| KeyboardShortcut::new(m, k);
        let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
        if pressed(sc(Modifiers::COMMAND, Key::O)) {
            self.open_dialog();
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
        if pressed(sc(Modifiers::COMMAND, Key::W)) {
            self.close_tab(self.active);
        }
        if pressed(sc(Modifiers::COMMAND, Key::Plus)) || pressed(sc(Modifiers::COMMAND, Key::Equals)) {
            self.font_size = (self.font_size + 1.0).min(40.0);
        }
        if pressed(sc(Modifiers::COMMAND, Key::Minus)) {
            self.font_size = (self.font_size - 1.0).max(7.0);
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
            ui.separator();

            ui.label("Buscar:");
            let r = ui.add(
                TextEdit::singleline(&mut self.query)
                    .desired_width(280.0)
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
            let has_tab = !self.tabs.is_empty();
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
            if let Some(t) = self.tabs.get(self.active) {
                if let Some(e) = &t.search_error {
                    ui.colored_label(ui.visuals().error_fg_color, format!("Regex inválida: {e}"));
                } else if let Some(s) = &t.search {
                    let n = s.count();
                    let pos = t.current.and_then(|l| s.index_of(l)).map(|i| format!("{}/", i + 1)).unwrap_or_default();
                    let mut txt = format!("{pos}{} coincidencias", fmt_num(n as u64));
                    if s.truncated.load(std::sync::atomic::Ordering::Relaxed) {
                        txt += &format!(" (límite {})", fmt_num(MAX_RESULTS as u64));
                    }
                    if !s.is_done() {
                        let p = s.progress.load(std::sync::atomic::Ordering::Relaxed) as f64 / s.total.max(1) as f64;
                        txt += &format!(" · buscando {:.0}%", p * 100.0);
                    } else {
                        txt += &format!(" · {:.2}s", s.elapsed.lock().unwrap().as_secs_f32());
                    }
                    ui.label(txt);
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
                    let last = t.doc.line_count().saturating_sub(1);
                    t.goto_line(n.saturating_sub(1).min(last));
                }
            }
            ui.separator();

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
                    .on_hover_text("Encoding del archivo");
                if enc != t.doc.src.encoding {
                    t.doc.set_encoding(enc);
                    t.search = None;
                    t.current = None;
                    t.detail_cache = None;
                    t.content_width = 0.0;
                }
            }
            ui.separator();
            ui.toggle_value(&mut self.show_results, "Resultados");
            ui.toggle_value(&mut self.show_detail, "Detalle").on_hover_text("Línea completa, seleccionable");
            if ui.small_button("A-").clicked() {
                self.font_size = (self.font_size - 1.0).max(7.0);
            }
            if ui.small_button("A+").clicked() {
                self.font_size = (self.font_size + 1.0).min(40.0);
            }
        });
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
        let mut close = None;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, t) in self.tabs.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("✕").on_hover_text("Cerrar (Ctrl+W)").clicked() {
                        close = Some(i);
                    }
                    let r = ui.selectable_label(i == self.active, RichText::new(&t.doc.name).strong());
                    if r.on_hover_text(t.doc.path.display().to_string()).clicked() {
                        self.active = i;
                    }
                });
                let idx = if t.doc.indexing_done() {
                    format!("{} líneas", fmt_num(t.doc.line_count()))
                } else {
                    let p = t.doc.progress.load(std::sync::atomic::Ordering::Relaxed) as f64 / t.doc.len().max(1) as f64;
                    format!("indexando {:.0}%", p * 100.0)
                };
                ui.label(
                    RichText::new(format!(
                        "   {} · {} · {}",
                        human_size(t.doc.len() as u64),
                        t.doc.src.encoding.name(),
                        idx
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
        if let Some(i) = close {
            self.close_tab(i);
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(t) = self.tabs.get(self.active) {
                let d = &t.doc;
                ui.label(RichText::new(d.path.display().to_string()).small());
                ui.separator();
                ui.label(RichText::new(human_size(d.len() as u64)).small());
                ui.separator();
                ui.label(RichText::new(format!("{} ({})", encoding::label(d.src.encoding), d.detect_reason)).small());
                ui.separator();
                let lines = fmt_num(d.line_count());
                if d.indexing_done() {
                    ui.label(RichText::new(format!("{lines} líneas")).small());
                } else {
                    let p = d.progress.load(std::sync::atomic::Ordering::Relaxed) as f32 / d.len().max(1) as f32;
                    ui.add(ProgressBar::new(p).desired_width(120.0).text(format!("indexando… {lines} líneas")));
                }
                if let Some((lo, hi)) = t.selection() {
                    ui.separator();
                    ui.label(
                        RichText::new(format!("Línea {} · {} seleccionadas", fmt_num(t.cursor.unwrap_or(lo) + 1), fmt_num(hi - lo + 1)))
                            .small(),
                    );
                }
            }
            if let Some((m, t)) = &self.message {
                if t.elapsed() < Duration::from_secs(5) {
                    ui.separator();
                    ui.label(RichText::new(m).small().color(ui.visuals().warn_fg_color));
                }
            }
        });
    }
}

impl App {
    /// Depuración: BUSCADOR_SCREENSHOT=ruta.rgba [BUSCADOR_QUERY=texto] guarda una
    /// captura cruda (ancho, alto u32 LE + RGBA) cuando todo está listo y sale.
    fn debug_screenshot(&mut self, ctx: &Context, busy: bool) {
        let Ok(path) = std::env::var("BUSCADOR_SCREENSHOT") else { return };
        self.debug_frames += 1;
        if self.debug_frames == 2 {
            if let Ok(q) = std::env::var("BUSCADOR_QUERY") {
                self.query = q;
                self.show_detail = true;
                self.search_or_next();
            }
        }
        if self.debug_frames < 40 || busy {
            ctx.request_repaint();
            return;
        }
        if self.debug_frames == 40 || self.debug_frames < 1000 {
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

        // Archivos arrastrados a la ventana.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for p in dropped {
            self.open(p);
        }
        self.handle_shortcuts(&ctx);

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
        let show_detail = &mut self.show_detail;
        CentralPanel::default().frame(Frame::NONE).show(ui, |ui| {
            if let Some(t) = self.tabs.get_mut(self.active) {
                message = log_view(ui, t, &font, show_detail);
            } else {
                ui.centered_and_justified(|ui| {
                    ui.heading("Arrastra archivos de log aquí, o pulsa Ctrl+O");
                });
            }
        });
        if let Some(m) = message {
            self.msg(m);
        }

        // Saltar a la primera coincidencia en cuanto aparezca.
        if let Some(t) = self.tabs.get_mut(self.active) {
            if t.pending_first {
                if let Some(s) = &t.search {
                    let from = t.cursor.unwrap_or(t.top);
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
        let busy = self.tabs.iter().any(|t| !t.doc.indexing_done() || t.search.as_ref().is_some_and(|s| !s.is_done()));
        if busy {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        self.debug_screenshot(&ctx, busy);
    }
}

/// Vista virtual del log: sólo se leen y pintan las líneas visibles.
fn log_view(ui: &mut Ui, tab: &mut Tab, font: &FontId, show_detail: &mut bool) -> Option<String> {
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

    let total = tab.doc.line_count();
    let visible = ((text_area.height() / row_h).floor() as u64).max(1);
    tab.visible_rows = visible;
    let max_top = |total: u64| total.saturating_sub(visible);

    if let Some(l) = tab.center_on.take() {
        if l < tab.top || l >= tab.top + visible {
            tab.top = l.saturating_sub(visible / 2);
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

    // Teclado.
    if resp.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter { tab: false, horizontal_arrows: true, vertical_arrows: true, escape: false },
            )
        });
        let (mods, delta, home, end, left, right, esc) = ui.input(|i| {
            let page = visible.saturating_sub(1).max(1) as i64;
            let mut d = 0i64;
            if i.key_pressed(Key::ArrowUp) {
                d -= 1;
            }
            if i.key_pressed(Key::ArrowDown) {
                d += 1;
            }
            if i.key_pressed(Key::PageUp) {
                d -= page;
            }
            if i.key_pressed(Key::PageDown) {
                d += page;
            }
            (
                i.modifiers,
                d,
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Escape),
            )
        });
        let last = total.saturating_sub(1);
        let target = if home && mods.command {
            Some(0)
        } else if end && mods.command {
            Some(last)
        } else if home {
            tab.hscroll = 0.0;
            None
        } else if delta != 0 {
            Some(add_signed(tab.cursor.unwrap_or(tab.top), delta).min(last))
        } else {
            None
        };
        if let Some(t) = target {
            if total > 0 {
                if !mods.shift || tab.anchor.is_none() {
                    tab.anchor = Some(t);
                }
                tab.cursor = Some(t);
                if t < tab.top {
                    tab.top = t;
                } else if t >= tab.top + visible {
                    tab.top = t + 1 - visible;
                }
            }
        }
        if left {
            tab.hscroll = (tab.hscroll - char_w * 8.0).max(0.0);
        }
        if right {
            tab.hscroll += char_w * 8.0;
        }
        if esc {
            tab.anchor = None;
            tab.cursor = None;
        }
        if ui.input(|i| i.events.iter().any(|e| matches!(e, Event::Copy))) {
            message = Some(tab.copy_selection(&ctx));
        }
    }

    // Ratón: selección por líneas (clic, Shift+clic, arrastrar).
    let row_at = |top: u64, y: f32| -> u64 {
        let r = ((y - text_area.top()) / row_h).floor().max(0.0) as u64;
        (top + r).min(total.saturating_sub(1))
    };
    if total > 0 {
        let shift = ui.input(|i| i.modifiers.shift);
        if resp.drag_started() || resp.clicked() || resp.secondary_clicked() {
            resp.request_focus();
            let p = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()) } else { resp.interact_pointer_pos() };
            if let Some(p) = p {
                let r = row_at(tab.top, p.y);
                let in_sel = tab.selection().is_some_and(|(lo, hi)| (lo..=hi).contains(&r));
                if shift && tab.anchor.is_some() {
                    tab.cursor = Some(r);
                } else if !(resp.secondary_clicked() && in_sel) {
                    tab.anchor = Some(r);
                    tab.cursor = Some(r);
                }
            }
        }
        if resp.double_clicked() {
            *show_detail = true;
        }
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                // Auto-desplazamiento al arrastrar fuera del área.
                if p.y < text_area.top() {
                    tab.top = tab.top.saturating_sub(1 + ((text_area.top() - p.y) / row_h) as u64);
                } else if p.y > text_area.bottom() {
                    tab.top += 1 + ((p.y - text_area.bottom()) / row_h) as u64;
                }
                tab.top = tab.top.min(max_top(total));
                tab.cursor = Some(row_at(tab.top, p.y.clamp(text_area.top(), text_area.bottom() - 1.0)));
                ctx.request_repaint();
            }
        }
    }
    resp.context_menu(|ui| {
        if ui.button("Copiar selección (Ctrl+C)").clicked() {
            message = Some(tab.copy_selection(&ctx));
            ui.close();
        }
        if ui.button("Ver línea completa").clicked() {
            *show_detail = true;
            ui.close();
        }
    });

    tab.top = tab.top.min(max_top(total));

    // Pintado.
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(text_area);
    painter.rect_filled(text_area, 0.0, visuals.extreme_bg_color);
    let digits = fmt_num(total.max(1)).chars().count().max(4);
    let gutter_w = char_w * digits as f32 + 14.0;
    let text_x = text_area.left() + gutter_w + 6.0;
    painter.rect_filled(
        Rect::from_min_max(text_area.min, pos2(text_area.left() + gutter_w, text_area.bottom())),
        0.0,
        visuals.faint_bg_color,
    );
    let text_painter = painter.with_clip_rect(Rect::from_min_max(pos2(text_x - 2.0, text_area.top()), text_area.max));
    let sel = tab.selection();
    let sel_color = visuals.selection.bg_fill.gamma_multiply(0.45);
    let cur_color = Color32::from_rgba_unmultiplied(255, 180, 0, 50);
    let mark_color = Color32::from_rgb(230, 160, 0);
    let highlight = tab.search.as_ref().map(|s| &s.highlight);
    let mut max_w: f32 = 0.0;

    for (n, s, e) in tab.doc.lines(tab.top, visible as usize + 1) {
        let y = text_area.top() + (n - tab.top) as f32 * row_h;
        let row = Rect::from_min_size(pos2(text_area.left(), y), vec2(text_area.width(), row_h));
        if sel.is_some_and(|(lo, hi)| n >= lo && n <= hi) {
            painter.rect_filled(row, 0.0, sel_color);
        } else if tab.current == Some(n) {
            painter.rect_filled(row, 0.0, cur_color);
        }
        if tab.search.as_ref().is_some_and(|s| s.index_of(n).is_some()) {
            painter.rect_filled(Rect::from_min_size(row.min, vec2(3.0, row_h)), 0.0, mark_color);
        }
        painter.text(
            pos2(text_area.left() + gutter_w - 6.0, y),
            Align2::RIGHT_TOP,
            fmt_num(n + 1),
            font.clone(),
            visuals.weak_text_color(),
        );
        let (txt, truncated) = tab.doc.src.decode_capped(s, e, MAX_DISPLAY_BYTES);
        let job = line_job(&txt, truncated, font, visuals.text_color(), highlight);
        let galley = painter.layout_job(job);
        max_w = max_w.max(galley.size().x);
        text_painter.galley(pos2(text_x - tab.hscroll, y), galley, visuals.text_color());
    }
    tab.content_width = tab.content_width.max(max_w);

    // Barras de desplazamiento.
    let mut top = tab.top;
    vscrollbar(ui, vsb, id.with("vsb"), &mut top, total, visible);
    tab.top = top;
    hscrollbar(ui, hsb, id.with("hsb"), &mut tab.hscroll, tab.content_width + 40.0, text_area.width() - gutter_w);
    if !tab.doc.indexing_done() {
        painter.text(
            text_area.right_bottom() - vec2(6.0, 4.0),
            Align2::RIGHT_BOTTOM,
            "indexando…",
            FontId::proportional(11.0),
            visuals.weak_text_color(),
        );
    }
    message
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
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\t' => out.push_str("    "),
            c if c.is_control() => out.push('·'),
            c => out.push(c),
        }
    }
    out
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
        if let Some(&(_, s, e)) = tab.doc.lines(line, 1).first() {
            let (txt, tr) = tab.doc.src.decode_capped(s, e, 2048);
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
    let Some(line) = tab.cursor else {
        ui.weak("Selecciona una línea para verla completa aquí.");
        return;
    };
    if tab.detail_cache.as_ref().map(|c| c.0) != Some(line) {
        let text = match tab.doc.lines(line, 1).first() {
            Some(&(_, s, e)) => {
                let (mut t, tr) = tab.doc.src.decode_capped(s, e, MAX_DETAIL_BYTES);
                if tr {
                    t += &format!("\n…[truncado: la línea mide {}]", human_size((e - s) as u64));
                }
                t
            }
            None => String::new(),
        };
        tab.detail_cache = Some((line, text));
    }
    let text = &tab.detail_cache.as_ref().unwrap().1;
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
