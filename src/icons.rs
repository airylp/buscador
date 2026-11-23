//! Íconos de línea dibujados en código (nítidos a cualquier tamaño y con los dos
//! temas) y el botón de las barras que los usa. Cada ícono se diseña en una
//! cuadrícula de 16×16 y se escala al rectángulo donde se pinta.

use eframe::egui::{
    Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, TextStyle, Ui, Vec2,
    Widget, WidgetInfo, WidgetText, pos2, vec2,
};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Sidebar,
    New,
    Open,
    Save,
    Undo,
    Redo,
    Edit,
    Block,
    Sparkle,
    Prev,
    Next,
    Replace,
    Advanced,
    Map,
    FolderSearch,
    Convert,
    Download,
    Gear,
    Info,
    Help,
    Wrap,
    Minimap,
    Reload,
    Live,
    Pause,
    Filter,
    Hex,
    Detail,
    Results,
    Chevron,
    Plus,
    Collapse,
    Search,
}

/// Familia de colores (discretos) de cada grupo de botones.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    File,
    Edit,
    Search,
    Tool,
    Live,
    Neutral,
}

impl Tone {
    pub fn color(self, ui: &Ui) -> Color32 {
        let dark = ui.visuals().dark_mode;
        let (d, l) = match self {
            Tone::File => ((126, 172, 226), (44, 104, 168)),
            Tone::Edit => ((222, 182, 112), (160, 106, 26)),
            Tone::Search => ((108, 196, 180), (26, 126, 110)),
            Tone::Tool => ((172, 158, 228), (104, 84, 176)),
            Tone::Live => ((112, 202, 122), (38, 140, 60)),
            Tone::Neutral => return ui.visuals().text_color(),
        };
        let (r, g, b) = if dark { d } else { l };
        Color32::from_rgb(r, g, b)
    }
}

/// Pinta `icon` dentro de `rect` (cuadrado) con el color dado.
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let k = rect.width().min(rect.height()) / 16.0;
    let o = rect.center() - vec2(8.0, 8.0) * k;
    let at = |x: f32, y: f32| -> Pos2 { o + vec2(x, y) * k };
    let w = (1.3 * k).max(1.0);
    let st = Stroke::new(w, color);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), st));
    };
    let closed = |pts: &[(f32, f32)]| {
        p.add(Shape::closed_line(pts.iter().map(|&(x, y)| at(x, y)).collect(), st));
    };
    let rect_s = |x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        p.rect_stroke(Rect::from_min_max(at(x0, y0), at(x1, y1)), r * k, st, StrokeKind::Middle);
    };
    let circle = |x: f32, y: f32, r: f32| {
        p.circle_stroke(at(x, y), r * k, st);
    };
    let dot = |x: f32, y: f32, r: f32| {
        p.circle_filled(at(x, y), r * k, color);
    };
    // Arco (ángulos en radianes, y hacia abajo) con punta de flecha al final.
    let arc = |cx: f32, cy: f32, r: f32, a0: f32, a1: f32, head: bool| {
        let n = 18;
        let pts: Vec<Pos2> =
            (0..=n).map(|i| a0 + (a1 - a0) * i as f32 / n as f32).map(|a| at(cx + r * a.cos(), cy + r * a.sin())).collect();
        p.add(Shape::line(pts, st));
        if head {
            let e = at(cx + r * a1.cos(), cy + r * a1.sin());
            let s = if a1 > a0 { 1.0 } else { -1.0 };
            let d = vec2(-a1.sin(), a1.cos()) * s; // tangente en el sentido del arco
            let n = vec2(-d.y, d.x);
            let h = 2.6 * k;
            p.add(Shape::convex_polygon(vec![e + d * h, e + n * h * 0.8, e - n * h * 0.8], color, Stroke::NONE));
        }
    };
    match icon {
        Icon::Sidebar => {
            rect_s(2.0, 3.0, 14.0, 13.0, 1.5);
            line(&[(6.5, 3.0), (6.5, 13.0)]);
            line(&[(3.6, 5.5), (5.0, 5.5)]);
            line(&[(3.6, 7.5), (5.0, 7.5)]);
        }
        Icon::New => {
            closed(&[(3.5, 1.8), (9.5, 1.8), (12.8, 5.1), (12.8, 14.2), (3.5, 14.2)]);
            line(&[(9.5, 1.8), (9.5, 5.1), (12.8, 5.1)]);
            line(&[(8.1, 7.4), (8.1, 11.8)]);
            line(&[(5.9, 9.6), (10.3, 9.6)]);
        }
        Icon::Open => {
            closed(&[(1.8, 3.2), (5.8, 3.2), (7.3, 4.8), (13.2, 4.8), (13.2, 6.6), (4.4, 6.6), (2.2, 12.8), (1.8, 12.8)]);
            closed(&[(4.4, 6.6), (14.6, 6.6), (12.4, 12.8), (2.2, 12.8)]);
        }
        Icon::Save => {
            closed(&[(2.5, 2.5), (11.0, 2.5), (13.5, 5.0), (13.5, 13.5), (2.5, 13.5)]);
            line(&[(5.0, 2.5), (5.0, 5.8), (10.2, 5.8), (10.2, 2.5)]);
            rect_s(4.8, 8.8, 11.2, 13.5, 0.6);
        }
        Icon::Undo | Icon::Redo => {
            let m = |x: f32| if icon == Icon::Redo { 16.0 - x } else { x };
            line(&[(m(4.0), 12.5), (m(9.2), 12.5)]);
            if icon == Icon::Undo {
                arc(9.2, 8.75, 3.75, FRAC_PI_2, -FRAC_PI_2, false);
            } else {
                arc(16.0 - 9.2, 8.75, 3.75, FRAC_PI_2, PI + FRAC_PI_2, false);
            }
            line(&[(m(9.2), 5.0), (m(4.2), 5.0)]);
            line(&[(m(6.4), 2.6), (m(3.9), 5.0), (m(6.4), 7.4)]);
        }
        Icon::Edit => {
            closed(&[(2.6, 13.4), (3.2, 10.4), (10.6, 3.0), (13.0, 5.4), (5.6, 12.8)]);
            line(&[(9.0, 4.6), (11.4, 7.0)]);
        }
        Icon::Block => {
            rect_s(2.5, 2.5, 13.5, 13.5, 1.2);
            for v in [6.17, 9.83] {
                line(&[(v, 2.5), (v, 13.5)]);
                line(&[(2.5, v), (13.5, v)]);
            }
        }
        Icon::Sparkle => {
            closed(&[(6.5, 2.5), (7.7, 6.8), (12.0, 8.0), (7.7, 9.2), (6.5, 13.5), (5.3, 9.2), (1.0, 8.0), (5.3, 6.8)]);
            line(&[(12.5, 1.6), (12.5, 5.0)]);
            line(&[(10.8, 3.3), (14.2, 3.3)]);
        }
        Icon::Prev => line(&[(10.0, 3.5), (5.5, 8.0), (10.0, 12.5)]),
        Icon::Next => line(&[(6.0, 3.5), (10.5, 8.0), (6.0, 12.5)]),
        Icon::Chevron => line(&[(4.5, 6.5), (8.0, 10.0), (11.5, 6.5)]),
        Icon::Plus => {
            line(&[(8.0, 3.0), (8.0, 13.0)]);
            line(&[(3.0, 8.0), (13.0, 8.0)]);
        }
        Icon::Collapse => {
            line(&[(8.0, 3.5), (3.5, 8.0), (8.0, 12.5)]);
            line(&[(12.5, 3.5), (8.0, 8.0), (12.5, 12.5)]);
        }
        Icon::Replace => {
            line(&[(2.2, 5.2), (13.2, 5.2)]);
            line(&[(10.6, 2.6), (13.2, 5.2), (10.6, 7.8)]);
            line(&[(13.8, 10.8), (2.8, 10.8)]);
            line(&[(5.4, 8.2), (2.8, 10.8), (5.4, 13.4)]);
        }
        Icon::Search => {
            circle(6.8, 6.8, 4.3);
            p.line_segment([at(10.0, 10.0), at(13.8, 13.8)], Stroke::new(w * 1.4, color));
        }
        Icon::Advanced => {
            line(&[(1.8, 3.2), (13.5, 3.2)]);
            line(&[(1.8, 6.8), (5.6, 6.8)]);
            line(&[(1.8, 10.4), (4.6, 10.4)]);
            circle(9.6, 9.2, 3.1);
            line(&[(11.9, 11.5), (14.2, 13.8)]);
        }
        Icon::Map => {
            rect_s(2.0, 2.5, 14.0, 13.5, 1.2);
            line(&[(8.6, 2.5), (8.6, 13.5)]);
            line(&[(2.0, 9.2), (8.6, 9.2)]);
            line(&[(8.6, 7.0), (14.0, 7.0)]);
            line(&[(11.4, 7.0), (11.4, 13.5)]);
        }
        Icon::FolderSearch => {
            line(&[(8.0, 12.6), (1.8, 12.6), (1.8, 3.2), (5.8, 3.2), (7.3, 4.8), (13.6, 4.8), (13.6, 7.6)]);
            circle(10.6, 10.4, 2.6);
            line(&[(12.5, 12.3), (14.4, 14.2)]);
        }
        Icon::Convert => {
            line(&[(5.0, 13.0), (5.0, 3.2)]);
            line(&[(2.6, 5.6), (5.0, 3.2), (7.4, 5.6)]);
            line(&[(11.0, 3.0), (11.0, 12.8)]);
            line(&[(8.6, 10.4), (11.0, 12.8), (13.4, 10.4)]);
        }
        Icon::Download => {
            line(&[(8.0, 2.0), (8.0, 10.0)]);
            line(&[(4.6, 6.6), (8.0, 10.0), (11.4, 6.6)]);
            line(&[(2.5, 10.8), (2.5, 13.5), (13.5, 13.5), (13.5, 10.8)]);
        }
        Icon::Gear => {
            circle(8.0, 8.0, 2.1);
            circle(8.0, 8.0, 4.4);
            for i in 0..8 {
                let a = i as f32 * TAU / 8.0;
                let (c, s) = (a.cos(), a.sin());
                p.line_segment([at(8.0 + 4.4 * c, 8.0 + 4.4 * s), at(8.0 + 6.4 * c, 8.0 + 6.4 * s)], Stroke::new(w * 1.6, color));
            }
        }
        Icon::Info => {
            circle(8.0, 8.0, 6.2);
            dot(8.0, 5.0, 0.95);
            line(&[(8.0, 7.2), (8.0, 11.6)]);
        }
        Icon::Help => {
            circle(8.0, 8.0, 6.2);
            arc(8.0, 6.4, 2.1, PI + 0.3, TAU + 0.9, false);
            line(&[(9.3, 8.0), (8.0, 9.0), (8.0, 9.8)]);
            dot(8.0, 11.6, 0.9);
        }
        Icon::Wrap => {
            line(&[(2.0, 3.8), (14.0, 3.8)]);
            line(&[(2.0, 7.8), (11.2, 7.8)]);
            arc(11.2, 10.0, 2.2, -FRAC_PI_2, FRAC_PI_2, false);
            line(&[(11.2, 12.2), (7.4, 12.2)]);
            line(&[(9.0, 10.6), (7.4, 12.2), (9.0, 13.8)]);
            line(&[(2.0, 12.2), (4.6, 12.2)]);
        }
        Icon::Minimap => {
            rect_s(3.5, 1.8, 12.5, 14.2, 1.2);
            for (y, x1) in [(4.4, 10.4), (6.6, 8.6), (8.8, 10.0), (11.0, 7.6)] {
                line(&[(5.6, y), (x1, y)]);
            }
        }
        Icon::Reload => arc(8.0, 8.4, 5.0, -1.0, 4.4, true),
        Icon::Live => {
            p.add(Shape::convex_polygon(vec![at(5.0, 3.4), at(12.6, 8.0), at(5.0, 12.6)], color, Stroke::NONE));
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(at(4.6, 3.4), at(7.0, 12.6)), 0.6 * k, color);
            p.rect_filled(Rect::from_min_max(at(9.0, 3.4), at(11.4, 12.6)), 0.6 * k, color);
        }
        Icon::Filter => closed(&[(2.0, 3.0), (14.0, 3.0), (9.4, 8.2), (9.4, 13.2), (6.6, 11.8), (6.6, 8.2)]),
        Icon::Hex => {
            let pts: Vec<(f32, f32)> = (0..6).map(|i| i as f32 * TAU / 6.0 + PI / 6.0).map(|a| (8.0 + 6.0 * a.cos(), 8.0 + 6.0 * a.sin())).collect();
            closed(&pts);
        }
        Icon::Detail => {
            rect_s(3.0, 1.8, 13.0, 14.2, 1.2);
            for (y, x1) in [(5.2, 10.6), (8.0, 10.6), (10.8, 8.4)] {
                line(&[(5.4, y), (x1, y)]);
            }
        }
        Icon::Results => {
            for y in [3.8, 8.0, 12.2] {
                dot(3.2, y, 1.05);
                line(&[(6.0, y), (14.0, y)]);
            }
        }
    }
}

/// Botón de barra: ícono (opcional) + texto (opcional), fondo solo al pasar el
/// ratón y un tinte suave del color del grupo cuando está activo.
pub struct IconButton {
    icon: Option<Icon>,
    text: Option<String>,
    selected: bool,
    tone: Tone,
    small: bool,
}

impl IconButton {
    pub fn new(icon: Icon, tone: Tone) -> Self {
        IconButton { icon: Some(icon), text: None, selected: false, tone, small: false }
    }

    /// Solo texto (p. ej. «Aa», «.*»), con el mismo aspecto que los demás.
    pub fn text_only(text: impl Into<String>, tone: Tone) -> Self {
        IconButton { icon: None, text: Some(text.into()), selected: false, tone, small: false }
    }

    pub fn label(mut self, text: impl Into<String>) -> Self {
        let t = text.into();
        self.text = (!t.is_empty()).then_some(t);
        self
    }

    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    /// Versión para la barra de estado: más baja y con letra pequeña.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }
}

impl Widget for IconButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let enabled = ui.is_enabled();
        let tint = self.tone.color(ui);
        let (h, s, style) = if self.small {
            (ui.spacing().interact_size.y - 2.0, 13.0, TextStyle::Small)
        } else {
            (ui.spacing().interact_size.y + 2.0, 16.0, TextStyle::Body)
        };
        // El margen sale de `button_padding` (la barra lo reduce si la ventana es estrecha).
        let pad = ui.spacing().button_padding.x - if self.small { 1.0 } else { 0.0 };
        let galley = self.text.as_ref().map(|t| WidgetText::from(t.as_str()).into_galley(ui, Some(eframe::egui::TextWrapMode::Extend), f32::INFINITY, style));
        let mut w = pad * 2.0;
        if self.icon.is_some() {
            w += s;
        }
        if let Some(g) = &galley {
            w += g.size().x + if self.icon.is_some() { 5.0 } else { 0.0 };
        }
        let w = w.max(h);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        let label = self.text.clone().unwrap_or_else(|| format!("{:?}", self.icon.unwrap_or(Icon::Chevron)));
        resp.widget_info(|| WidgetInfo::selected(eframe::egui::Role::Button, enabled, self.selected, &label));
        if ui.is_rect_visible(rect) {
            let v = ui.visuals();
            let radius = CornerRadius::same(if self.small { 4 } else { 6 });
            if self.selected {
                ui.painter().rect(rect, radius, tint.gamma_multiply(if v.dark_mode { 0.16 } else { 0.13 }), Stroke::new(1.0, tint.gamma_multiply(0.55)), StrokeKind::Inside);
            } else if enabled && (resp.hovered() || resp.has_focus()) {
                let f = if resp.is_pointer_button_down_on() { v.widgets.active.weak_bg_fill } else { v.widgets.hovered.weak_bg_fill };
                ui.painter().rect_filled(rect, radius, f);
            }
            let fg = if !enabled {
                v.weak_text_color().gamma_multiply(0.55)
            } else if self.tone == Tone::Neutral {
                v.text_color().gamma_multiply(0.9)
            } else {
                tint
            };
            let mut x = rect.left() + pad;
            if self.icon.is_none() {
                x = rect.center().x - galley.as_ref().map_or(0.0, |g| g.size().x) / 2.0;
            }
            if let Some(icon) = self.icon {
                let r = Rect::from_center_size(pos2(x + s / 2.0, rect.center().y), Vec2::splat(s));
                paint(ui.painter(), r, icon, fg);
                x += s + 5.0;
            }
            if let Some(g) = galley {
                let tc = if !enabled {
                    v.weak_text_color().gamma_multiply(0.7)
                } else if self.selected {
                    v.strong_text_color()
                } else if self.icon.is_none() {
                    fg
                } else {
                    v.text_color()
                };
                ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, tc);
            }
        }
        resp
    }
}

/// Punto de aviso (p. ej. versión nueva) en la esquina de un botón.
pub fn badge(ui: &Ui, r: &Response, color: Color32) {
    let c = r.rect.right_top() + vec2(-4.0, 4.0);
    ui.painter().circle_filled(c, 3.5, color);
    ui.painter().circle_stroke(c, 3.5, Stroke::new(1.0, ui.visuals().panel_fill));
}
