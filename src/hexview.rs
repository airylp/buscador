//! Vista hexadecimal del archivo (solo lectura).
//!
//! Lee directamente del `mmap`, así que sirve para archivos de cualquier
//! tamaño: solo se dibujan las filas visibles (16 bytes cada una). Muestra el
//! archivo tal como está en disco (sin los cambios sin guardar). Permite ir a
//! un desplazamiento, buscar bytes o texto (en un hilo), seleccionar con el
//! ratón o el teclado, copiar la selección como hex/Base64/texto y ver el valor
//! de los bytes bajo el cursor como enteros, flotantes o carácter.

use crate::convert;
use crate::document::Bytes;
use eframe::egui::{self, pos2, vec2, Align, Align2, CursorIcon, Event, EventFilter, FontId, Key, Layout, Rect, Sense, Stroke, StrokeKind, TextEdit, Ui};
use std::sync::{Arc, Mutex};

/// Máximo que se copia de una selección.
const COPY_MAX: u64 = 16 << 20;

#[derive(Default)]
pub struct Hex {
    pub on: bool,
    /// Primera fila visible.
    top: u64,
    /// Byte del cursor y el otro extremo de la selección.
    cursor: u64,
    anchor: Option<u64>,
    goto: String,
    find: String,
    /// Buscar texto (UTF-8) en vez de bytes en hexadecimal.
    find_text: bool,
    big_endian: bool,
    job: Option<Arc<Mutex<Option<Option<u64>>>>>,
    /// Largo de lo que se busca (para seleccionarlo al encontrarlo).
    job_len: u64,
    rows: u64,
    wheel: f32,
    focus_next: bool,
}

/// Lo que la vista le pide a la app.
#[derive(Default)]
pub struct Out {
    pub message: Option<String>,
    pub focused: bool,
    /// «Abrir en Convertir» con este texto (los bytes en hexadecimal).
    pub to_convert: Option<String>,
}

impl Hex {
    pub fn toggle(&mut self) {
        self.on = !self.on;
        self.focus_next = self.on;
    }

    /// Selección [desde, hasta] inclusive.
    fn selection(&self) -> (u64, u64) {
        let a = self.anchor.unwrap_or(self.cursor);
        (a.min(self.cursor), a.max(self.cursor))
    }

    fn reveal(&mut self) {
        let row = self.cursor / 16;
        let rows = self.rows.max(1);
        if row < self.top {
            self.top = row;
        } else if row >= self.top + rows {
            self.top = row + 1 - rows;
        }
    }

    /// Pone la fila del cursor en el centro de la vista.
    fn center(&mut self) {
        self.top = (self.cursor / 16).saturating_sub(self.rows / 2);
    }

    fn move_to(&mut self, b: u64, len: u64, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = b.min(len.saturating_sub(1));
        self.reveal();
    }

    fn start_find(&mut self, data: &Arc<Bytes>, ctx: &egui::Context) -> Option<String> {
        let pat = if self.find_text {
            self.find.as_bytes().to_vec()
        } else {
            match convert::hex_decode(&self.find) {
                Ok(p) => p,
                Err(e) => return Some(e),
            }
        };
        if pat.is_empty() {
            return None;
        }
        let from = (self.cursor + 1).min(data.len() as u64) as usize;
        let slot = Arc::new(Mutex::new(None));
        let (s2, d2, ctx2) = (slot.clone(), data.clone(), ctx.clone());
        self.job_len = pat.len() as u64;
        std::thread::spawn(move || {
            let d: &[u8] = &d2;
            let f = memchr::memmem::Finder::new(&pat);
            // Desde el cursor hasta el final y, si no, desde el principio.
            let hit = f.find(&d[from..]).map(|p| p + from).or_else(|| f.find(&d[..(from + pat.len() - 1).min(d.len())]));
            *s2.lock().unwrap() = Some(hit.map(|p| p as u64));
            ctx2.request_repaint();
        });
        self.job = Some(slot);
        None
    }
}

/// «0x1F», «1f» (si tiene letras hex) o «31» → desplazamiento.
fn parse_offset(s: &str) -> Option<u64> {
    let s = s.trim().replace(['_', ' '], "");
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else if s.chars().any(|c| c.is_ascii_alphabetic()) {
        u64::from_str_radix(s.trim_end_matches(['h', 'H']), 16).ok()
    } else {
        s.parse().ok()
    }
}

pub fn show(ui: &mut Ui, h: &mut Hex, data: &Arc<Bytes>, font_size: f32, dirty: bool) -> Out {
    let mut out = Out::default();
    let len = data.len() as u64;
    let d: &[u8] = data;
    let ctx = ui.ctx().clone();

    // Búsqueda terminada.
    if let Some(slot) = &h.job {
        let r = slot.lock().unwrap().take();
        if let Some(hit) = r {
            h.job = None;
            match hit {
                Some(p) => {
                    h.anchor = Some(p);
                    h.cursor = (p + h.job_len - 1).min(len.saturating_sub(1));
                    h.center();
                    h.focus_next = true;
                }
                None => out.message = Some(t!("No encontrado").into()),
            }
        }
    }

    // Barra: ir a, buscar.
    ui.add_space(3.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(t!("Ir a:"));
        let r = ui.add(TextEdit::singleline(&mut h.goto).desired_width(110.0).hint_text("0x1A2B / 6699"));
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            match parse_offset(&h.goto) {
                Some(o) if o < len.max(1) => {
                    h.move_to(o, len, false);
                    h.center();
                    h.focus_next = true;
                }
                _ => out.message = Some(tf!("Desplazamiento fuera del archivo: {v}", v = h.goto.trim())),
            }
        }
        ui.separator();
        ui.label(t!("Buscar:"));
        let r = ui.add(TextEdit::singleline(&mut h.find).desired_width(180.0).hint_text(if h.find_text { t!("texto") } else { "ff d8 ff" }));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        ui.selectable_value(&mut h.find_text, false, "Hex").on_hover_text(t!("Buscar bytes escritos en hexadecimal"));
        ui.selectable_value(&mut h.find_text, true, t!("Texto")).on_hover_text(t!("Buscar texto (UTF-8)"));
        let busy = h.job.is_some();
        if (ui.add_enabled(!busy && !h.find.is_empty(), egui::Button::new(t!("Siguiente ▶"))).clicked() || enter) && !busy {
            out.message = h.start_find(data, &ctx);
        }
        if busy {
            ui.spinner();
        }
        if dirty {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(6.0);
                ui.weak(t!("Muestra el archivo en disco, sin los cambios sin guardar"));
            });
        }
    });
    ui.add_space(3.0);

    let font = FontId::monospace(font_size);
    let (cw, row_h) = ui.fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font) + 3.0));
    let full = ui.available_rect_before_wrap();
    let info_h = (ui.spacing().interact_size.y + ui.spacing().item_spacing.y) * 2.0 + 12.0;
    let area = Rect::from_min_max(full.min, pos2(full.right() - 12.0, full.bottom() - info_h));
    let bar = Rect::from_min_max(pos2(area.right(), area.top()), pos2(full.right(), area.bottom()));
    let total_rows = len.div_ceil(16).max(1);
    h.rows = ((area.height() / row_h).floor() as u64).max(1);
    let id = egui::Id::NULL.with("vista_hex");
    let resp = ui.interact(area, id, Sense::click_and_drag());
    if std::mem::take(&mut h.focus_next) || resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    // (No `resp.has_focus()`: exige además que la ventana esté activa.)
    let focused = ui.memory(|m| m.has_focus(id));
    if focused {
        ui.memory_mut(|m| m.set_focus_lock_filter(id, EventFilter { tab: false, horizontal_arrows: true, vertical_arrows: true, escape: false }));
    }
    out.focused = focused;

    // Columnas.
    let wide = len > u32::MAX as u64;
    let od = if wide { 12.0 } else { 8.0 };
    let x0 = area.left() + 8.0;
    let hx = x0 + (od + 2.0) * cw;
    let ax = hx + 50.0 * cw;
    let byte_x = |i: u64| hx + (i as f32 * 3.0 + if i >= 8 { 1.0 } else { 0.0 }) * cw;

    // Ratón: byte bajo el puntero.
    let hit = |p: egui::Pos2, top: u64| -> Option<u64> {
        if len == 0 {
            return None;
        }
        let row = top + ((p.y - area.top()) / row_h).max(0.0) as u64;
        let col = if p.x >= ax - cw {
            ((p.x - ax) / cw).floor().clamp(0.0, 15.0) as u64
        } else {
            let k = ((p.x - hx) / cw).floor().max(0.0) as u64;
            (if k >= 24 { (k - 1) / 3 } else { k / 3 }).min(15)
        };
        Some((row * 16 + col).min(len - 1))
    };
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.drag_started() || resp.clicked() {
            if let Some(b) = hit(p, h.top) {
                let shift = ui.input(|i| i.modifiers.shift);
                h.move_to(b, len, shift);
            }
        } else if resp.dragged() {
            if let Some(b) = hit(p, h.top) {
                h.anchor.get_or_insert(h.cursor);
                h.cursor = b;
            }
            // Arrastrar fuera desplaza.
            if p.y < area.top() {
                h.top = h.top.saturating_sub(1);
            } else if p.y > area.bottom() {
                h.top = (h.top + 1).min(total_rows.saturating_sub(h.rows));
            }
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Text);
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            h.wheel -= dy / row_h;
            let n = h.wheel.trunc();
            h.wheel -= n;
            h.top = if n < 0.0 { h.top.saturating_sub(-n as u64) } else { h.top + n as u64 };
        }
    }

    // Teclado.
    if focused {
        let page = h.rows.max(2) - 1;
        let events = ui.input(|i| i.events.clone());
        for e in events {
            match e {
                Event::Key { key, pressed: true, modifiers: m, .. } => {
                    let c = h.cursor;
                    let target = match key {
                        Key::ArrowLeft => Some(c.saturating_sub(1)),
                        Key::ArrowRight => Some(c + 1),
                        Key::ArrowUp => Some(c.saturating_sub(16)),
                        Key::ArrowDown => Some(if c + 16 < len { c + 16 } else { c }),
                        Key::PageUp => Some(c.saturating_sub(16 * page)),
                        Key::PageDown => Some((c + 16 * page).min(len.saturating_sub(1))),
                        Key::Home if m.command => Some(0),
                        Key::End if m.command => Some(len.saturating_sub(1)),
                        Key::Home => Some(c / 16 * 16),
                        Key::End => Some(c / 16 * 16 + 15),
                        Key::Escape => {
                            h.anchor = None;
                            None
                        }
                        _ => None,
                    };
                    if let Some(t) = target {
                        h.move_to(t, len, m.shift);
                    }
                    if key == Key::A && m.command && len > 0 {
                        h.anchor = Some(0);
                        h.cursor = len - 1;
                    }
                }
                Event::Copy | Event::Cut => {
                    let (a, b) = h.selection();
                    out.message = Some(copy(&ctx, d, a, b, Fmt::Hex));
                }
                _ => {}
            }
        }
    }
    h.top = h.top.min(total_rows.saturating_sub(h.rows));

    // Pintar.
    let v = ui.visuals().clone();
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, v.extreme_bg_color);
    let (sa, sb) = h.selection();
    let has_sel = h.anchor.is_some_and(|a| a != h.cursor);
    let sel_fill = v.selection.bg_fill;
    let weak = v.weak_text_color();
    let strong = v.text_color();
    for r in 0..h.rows {
        let row = h.top + r;
        let start = row * 16;
        if start >= len && !(len == 0 && row == 0) {
            break;
        }
        let y = area.top() + r as f32 * row_h + 1.0;
        let end = (start + 16).min(len);
        let bytes = &d[start as usize..end as usize];
        // Selección y cursor debajo del texto.
        for (i, _) in bytes.iter().enumerate() {
            let b = start + i as u64;
            let in_sel = has_sel && b >= sa && b <= sb;
            let hr = Rect::from_min_size(pos2(byte_x(i as u64) - cw * 0.25, y), vec2(cw * 2.5, row_h - 1.0));
            let ar = Rect::from_min_size(pos2(ax + i as f32 * cw, y), vec2(cw, row_h - 1.0));
            if in_sel {
                painter.rect_filled(hr, 0.0, sel_fill);
                painter.rect_filled(ar, 0.0, sel_fill);
            }
            if b == h.cursor {
                let st = Stroke::new(1.0, if focused { v.selection.stroke.color } else { weak });
                painter.rect_stroke(hr, 2.0, st, StrokeKind::Inside);
                painter.rect_stroke(ar, 2.0, st, StrokeKind::Inside);
            }
        }
        let off = if wide { format!("{start:012x}") } else { format!("{start:08x}") };
        painter.text(pos2(x0, y), Align2::LEFT_TOP, off, font.clone(), weak);
        // Bytes en dos colores: los 00 tenues.
        let mut hex_job = egui::text::LayoutJob::default();
        for (i, b) in bytes.iter().enumerate() {
            let sep = if i == 0 { "" } else if i == 8 { "  " } else { " " };
            let color = if *b == 0 { weak } else { strong };
            hex_job.append(&format!("{sep}{b:02x}"), 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
        }
        let g = ui.fonts_mut(|f| f.layout_job(hex_job));
        painter.galley(pos2(hx, y), g, strong);
        let ascii: String = bytes.iter().map(|&b| convert::printable(b)).collect();
        painter.text(pos2(ax, y), Align2::LEFT_TOP, ascii, font.clone(), strong);
    }
    // Línea entre bytes y texto.
    let sep_x = ax - cw;
    painter.vline(sep_x, area.y_range(), Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color));
    let mut top = h.top;
    crate::app::vscrollbar(ui, bar, id.with("barra"), &mut top, total_rows, h.rows);
    h.top = top;

    // Menú contextual.
    resp.context_menu(|ui| {
        let (a, b) = h.selection();
        let n = b - a + 1;
        ui.weak(tf!("{n} bytes seleccionados", n = n));
        ui.separator();
        for (label, f) in [(t!("Copiar como hexadecimal"), Fmt::Hex), (t!("Copiar como Base64"), Fmt::Base64), (t!("Copiar como texto"), Fmt::Text)] {
            if ui.button(label).clicked() {
                out.message = Some(copy(&ctx, d, a, b, f));
                ui.close();
            }
        }
        ui.separator();
        if ui.button(t!("Abrir en Convertir…")).clicked() {
            let end = (b + 1).min(a + COPY_MAX);
            out.to_convert = Some(convert::hex_encode(&d[a as usize..end as usize], convert::HexSep::Space, false));
            ui.close();
        }
    });

    // Información del cursor.
    ui.allocate_rect(area.union(bar), Sense::hover());
    let info = Rect::from_min_max(pos2(full.left() + 8.0, area.bottom() + 4.0), full.right_bottom());
    ui.scope_builder(egui::UiBuilder::new().max_rect(info), |ui| {
        if len == 0 {
            ui.weak(t!("Archivo vacío"));
            return;
        }
        let c = h.cursor.min(len - 1);
        let byte = d[c as usize];
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tf!("Desplazamiento 0x{hex} ({dec})", hex = format!("{c:X}"), dec = c)).strong());
            ui.label(format!("· 0x{byte:02X} = {byte} = {}", byte as i8));
            if has_sel {
                ui.label(tf!("· selección: {n} bytes (0x{a}–0x{b})", n = sb - sa + 1, a = format!("{sa:X}"), b = format!("{sb:X}")));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.checkbox(&mut h.big_endian, "Big-endian").on_hover_text(t!("Orden de los bytes al leer números"));
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let rest = &d[c as usize..(c as usize + 8).min(d.len())];
            let mut buf = [0u8; 8];
            let get = |n: usize, buf: &mut [u8; 8]| -> Option<()> {
                (rest.len() >= n).then(|| buf[..n].copy_from_slice(&rest[..n]))
            };
            let be = h.big_endian;
            macro_rules! num {
                ($n:expr, $t:ty, $name:expr) => {
                    if get($n, &mut buf).is_some() {
                        let a: [u8; $n] = buf[..$n].try_into().unwrap();
                        let val = if be { <$t>::from_be_bytes(a) } else { <$t>::from_le_bytes(a) };
                        ui.label(egui::RichText::new(format!("{} {}", $name, Num::show(val))).font(FontId::monospace(font_size * 0.92)));
                    }
                };
            }
            num!(2, u16, "u16");
            num!(2, i16, "i16");
            num!(4, u32, "u32");
            num!(4, i32, "i32");
            num!(8, u64, "u64");
            num!(4, f32, "f32");
            num!(8, f64, "f64");
            // Carácter UTF-8 que empieza aquí.
            let s = &d[c as usize..(c as usize + 4).min(d.len())];
            let ch = (1..=s.len()).find_map(|n| std::str::from_utf8(&s[..n]).ok().and_then(|t| t.chars().next()));
            if let Some(ch) = ch.filter(|c| !c.is_control()) {
                ui.label(egui::RichText::new(format!("UTF-8 «{ch}» U+{:04X}", ch as u32)).font(FontId::monospace(font_size * 0.92)));
            }
        });
    });
    if h.job.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    out
}

/// Número para la línea de valores (los flotantes, cortos).
trait Num {
    fn show(self) -> String;
}

macro_rules! int_num {
    ($($t:ty),*) => {$(impl Num for $t { fn show(self) -> String { self.to_string() } })*};
}
int_num!(u16, i16, u32, i32, u64);

impl Num for f32 {
    fn show(self) -> String {
        (self as f64).show()
    }
}

impl Num for f64 {
    fn show(self) -> String {
        let a = self.abs();
        if self == 0.0 || !self.is_finite() || (1e-4..1e15).contains(&a) {
            let s = format!("{self:.6}");
            if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
        } else {
            format!("{self:.4e}")
        }
    }
}

#[derive(Clone, Copy)]
enum Fmt {
    Hex,
    Base64,
    Text,
}

fn copy(ctx: &egui::Context, data: &[u8], a: u64, b: u64, f: Fmt) -> String {
    if data.is_empty() {
        return t!("Archivo vacío").into();
    }
    let end = (b + 1).min(a + COPY_MAX).min(data.len() as u64);
    let s = &data[a as usize..end as usize];
    let text = match f {
        Fmt::Hex => convert::hex_encode(s, convert::HexSep::Space, false),
        Fmt::Base64 => convert::base64_encode(s, false, true, 0),
        Fmt::Text => String::from_utf8_lossy(s).into_owned(),
    };
    ctx.copy_text(text);
    let mut m = tf!("Copiados {n} bytes", n = s.len());
    if end <= b {
        m += &tf!(" (solo los primeros {max})", max = convert::human(COPY_MAX as usize));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets() {
        assert_eq!(parse_offset("0x1A2B"), Some(0x1a2b));
        assert_eq!(parse_offset("6699"), Some(6699));
        assert_eq!(parse_offset("ff"), Some(255));
        assert_eq!(parse_offset("1_000"), Some(1000));
        assert_eq!(parse_offset("zz"), None);
        assert_eq!(1.5f64.show(), "1.5");
        assert_eq!(0f32.show(), "0");
        assert_eq!(9.18e-41f64.show(), "9.1800e-41");
        assert_eq!(f32::NAN.show(), "NaN");
    }
}
