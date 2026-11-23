//! Vista de lectura de HTML: convierte el documento en bloques (títulos,
//! párrafos, listas, citas, código, tablas, imágenes) y los muestra formateados,
//! con algo de CSS en línea (color, fondo, negrita, alineación) pero sin hojas
//! de estilo ni JavaScript. El análisis es propio (tolerante, sin árbol) y se
//! hace en un hilo cuando cambia el texto, junto con la carga de las imágenes
//! locales o `data:`. Cada bloque recuerda su línea en el código: clic en el
//! bloque → el editor va a esa línea; con «Seguir cursor» se resalta el bloque
//! donde está el cursor. Se puede ver en una ventana aparte.

use crate::icons::{Icon, IconButton, Tone};
use eframe::egui::{
    self, Align, Color32, ColorImage, Context, FontId, Frame, Label, Margin, RichText, ScrollArea, Sense, Shape, Stroke,
    TextureHandle, TextureOptions, Ui, vec2,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// Máximo que se analiza (más grande → se sugiere el navegador).
pub const MAX_BYTES: usize = 16 << 20;
/// Máximo de bloques que se dibujan.
const MAX_BLOCKS: usize = 4000;
/// Límites de las imágenes: cantidad, tamaño del archivo y lado mayor.
const MAX_IMAGES: usize = 200;
const MAX_IMAGE_FILE: u64 = 30 << 20;
const MAX_IMAGE_SIDE: u32 = 2048;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub underline: bool,
    pub mark: bool,
    pub small: bool,
    pub sup: bool,
    pub sub: bool,
    pub color: Option<Color32>,
    pub bg: Option<Color32>,
    pub link: Option<String>,
    pub title: Option<String>,
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    /// Índice en `Doc::images` si es una imagen (`text` = su descripción).
    pub image: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Heading(u8),
    Para,
    Item { depth: u8, mark: String },
    Quote,
    Pre,
    Rule,
    Caption,
    Summary,
    Table { rows: Vec<(bool, Vec<Vec<Span>>)>, caption: Vec<Span> },
}

#[derive(Clone, Debug)]
pub struct Block {
    pub kind: Kind,
    pub spans: Vec<Span>,
    /// Línea (desde 0) del código donde empieza.
    pub line: u64,
    pub align: Option<Align>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageRef {
    pub src: String,
    pub w: Option<f32>,
    pub h: Option<f32>,
}

#[derive(Default, Debug)]
pub struct Doc {
    pub title: String,
    pub blocks: Vec<Block>,
    pub images: Vec<ImageRef>,
    /// `id` / `name` → bloque (para los enlaces `#ancla`).
    pub anchors: HashMap<String, usize>,
}

impl Doc {
    /// Bloque que contiene la línea `line` del código.
    fn block_at(&self, line: u64) -> Option<usize> {
        let i = self.blocks.partition_point(|b| b.line <= line);
        i.checked_sub(1)
    }
}

type Loaded = Vec<(String, Option<Arc<ColorImage>>)>;

/// Estado de la vista en una pestaña.
pub struct HtmlView {
    pub on: bool,
    pub detached: bool,
    pub follow: bool,
    pub zoom: f32,
    doc: Option<Arc<Doc>>,
    version: Option<u64>,
    job: Option<(u64, Receiver<(Doc, Loaded)>)>,
    changed_at: Option<Instant>,
    pub too_big: bool,
    /// Imágenes ya cargadas (o que fallaron: `None`), por `src`.
    textures: HashMap<String, Option<TextureHandle>>,
    last_cursor: Option<u64>,
    scroll_to: Option<usize>,
}

impl Default for HtmlView {
    fn default() -> Self {
        HtmlView {
            on: false,
            detached: false,
            follow: true,
            zoom: 1.0,
            doc: None,
            version: None,
            job: None,
            changed_at: None,
            too_big: false,
            textures: HashMap::new(),
            last_cursor: None,
            scroll_to: None,
        }
    }
}

impl HtmlView {
    pub fn toggle(&mut self) {
        self.on = !self.on;
        if !self.on {
            // Libera la memoria; se rehace al volver a abrirla.
            self.doc = None;
            self.version = None;
            self.job = None;
            self.textures.clear();
            self.detached = false;
        }
    }

    /// ¿Hace falta el texto para la versión `v`? (espera 300 ms tras editar).
    pub fn wants(&mut self, v: u64) -> bool {
        if !self.on || self.version == Some(v) || self.job.as_ref().is_some_and(|j| j.0 == v) {
            return false;
        }
        if self.version.is_none() {
            return true;
        }
        let at = *self.changed_at.get_or_insert_with(Instant::now);
        at.elapsed() >= Duration::from_millis(300)
    }

    /// Analiza `text` (versión `v`) en un hilo; las imágenes relativas se
    /// buscan en `dir`.
    pub fn start(&mut self, v: u64, text: Option<String>, dir: Option<PathBuf>) {
        self.changed_at = None;
        let Some(text) = text else {
            self.too_big = true;
            self.version = Some(v);
            self.doc = None;
            return;
        };
        self.too_big = false;
        let skip: HashSet<String> = self.textures.keys().cloned().collect();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let doc = parse(&text);
            let loaded = load_images(&doc, dir.as_deref(), &skip);
            let _ = tx.send((doc, loaded));
        });
        self.job = Some((v, rx));
    }

    /// Recoge el resultado; `true` si sigue trabajando.
    pub fn poll(&mut self, ctx: &Context) -> bool {
        let Some((v, rx)) = &self.job else { return self.changed_at.is_some() };
        match rx.try_recv() {
            Ok((d, loaded)) => {
                for (src, img) in loaded {
                    let tex = img.map(|i| ctx.load_texture(format!("html:{src}"), i, TextureOptions::LINEAR));
                    self.textures.insert(src, tex);
                }
                // Las que ya no están en el documento se sueltan.
                let used: HashSet<&str> = d.images.iter().map(|i| i.src.as_str()).collect();
                self.textures.retain(|k, _| used.contains(k.as_str()));
                self.version = Some(*v);
                self.doc = Some(Arc::new(d));
                self.job = None;
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => {
                self.job = None;
                false
            }
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.doc.as_ref().map(|d| d.title.as_str()).filter(|t| !t.is_empty())
    }
}

// ---------------------------------------------------------------- imágenes

fn load_images(doc: &Doc, dir: Option<&Path>, skip: &HashSet<String>) -> Loaded {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for img in doc.images.iter().take(MAX_IMAGES) {
        if skip.contains(&img.src) || !seen.insert(img.src.clone()) {
            continue;
        }
        let decoded = image_bytes(&img.src, dir).and_then(|b| decode_image(&b));
        out.push((img.src.clone(), decoded.map(Arc::new)));
    }
    out
}

/// Bytes de una imagen `data:` (Base64) o local (ruta relativa a `dir` o `file://`).
fn image_bytes(src: &str, dir: Option<&Path>) -> Option<Vec<u8>> {
    let s = src.trim();
    if let Some(rest) = s.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        return if meta.ends_with(";base64") { crate::convert::base64_decode(data).ok() } else { Some(percent_decode(data).into_bytes()) };
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("//") {
        return None; // sin red
    }
    let path = match s.strip_prefix("file://") {
        Some(p) => PathBuf::from(percent_decode(p)),
        None => dir?.join(percent_decode(s.split(['?', '#']).next().unwrap_or(s))),
    };
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_IMAGE_FILE {
        return None;
    }
    std::fs::read(path).ok()
}

fn decode_image(bytes: &[u8]) -> Option<ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width().max(img.height()) > MAX_IMAGE_SIDE {
        img.thumbnail(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------- análisis

fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside" | "h1" | "h2" | "h3" | "h4" | "h5"
            | "h6" | "li" | "ul" | "ol" | "dl" | "dt" | "dd" | "pre" | "blockquote" | "hr" | "table" | "thead" | "tbody"
            | "tfoot" | "tr" | "figure" | "figcaption" | "form" | "fieldset" | "address" | "details" | "summary" | "body"
            | "html" | "center" | "menu" | "caption" | "legend" | "hgroup" | "search" | "dialog"
    )
}

fn is_void(tag: &str) -> bool {
    matches!(tag, "br" | "img" | "hr" | "input" | "meta" | "link" | "wbr" | "area" | "base" | "col" | "embed" | "source" | "track" | "param")
}

/// Decodifica las entidades más comunes (`&amp;`, `&#233;`, `&#xE9;`…).
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find(|c: char| c == ';' || c == '&' || c.is_whitespace() || c == '<').map(|e| e + 1);
        let decoded = end.filter(|&e| rest.as_bytes().get(e) == Some(&b';') && e <= 12).and_then(|e| {
            let name = &rest[1..e];
            let c = if let Some(n) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                u32::from_str_radix(n, 16).ok().and_then(char::from_u32)
            } else if let Some(n) = name.strip_prefix('#') {
                n.parse::<u32>().ok().and_then(char::from_u32)
            } else {
                named_entity(name)
            };
            c.map(|c| (c, e + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn named_entity(n: &str) -> Option<char> {
    Some(match n {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '•',
        "middot" => '·',
        "deg" => '°',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "sect" => '§',
        "para" => '¶',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "frac12" => '½',
        "frac14" => '¼',
        "iexcl" => '¡',
        "iquest" => '¿',
        "aacute" => 'á',
        "eacute" => 'é',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "Aacute" => 'Á',
        "Eacute" => 'É',
        "Iacute" => 'Í',
        "Oacute" => 'Ó',
        "Uacute" => 'Ú',
        "ntilde" => 'ñ',
        "Ntilde" => 'Ñ',
        "uuml" => 'ü',
        "Uuml" => 'Ü',
        "ouml" => 'ö',
        "auml" => 'ä',
        "ccedil" => 'ç',
        "agrave" => 'à',
        "egrave" => 'è',
        "ecirc" => 'ê',
        "szlig" => 'ß',
        "rarr" => '→',
        "larr" => '←',
        "uarr" => '↑',
        "darr" => '↓',
        "check" => '✓',
        "hearts" => '♥',
        "star" => '☆',
        _ => return None,
    })
}

/// Valor de un atributo dentro del texto de una etiqueta (`a href="…"`).
fn attr(tag_body: &str, name: &str) -> Option<String> {
    let lower = tag_body.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let i = from + i;
        from = i + name.len();
        let before_ok = i == 0 || lower.as_bytes()[i - 1].is_ascii_whitespace();
        let rest = tag_body[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let rest = rest[1..].trim_start();
        let v = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
        };
        return Some(decode_entities(v));
    }
    None
}

/// Color CSS: `#rgb`, `#rrggbb`, `rgb(r, g, b)` o un nombre común.
pub fn parse_color(v: &str) -> Option<Color32> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(h) = v.strip_prefix('#') {
        let n = |s: &str| u8::from_str_radix(s, 16).ok();
        return match h.len() {
            3 | 4 => {
                let c: Vec<u8> = h.chars().take(3).filter_map(|c| n(&c.to_string())).map(|x| x * 17).collect();
                (c.len() == 3).then(|| Color32::from_rgb(c[0], c[1], c[2]))
            }
            6 | 8 => Some(Color32::from_rgb(n(&h[0..2])?, n(&h[2..4])?, n(&h[4..6])?)),
            _ => None,
        };
    }
    if let Some(inner) = v.strip_prefix("rgb(").or_else(|| v.strip_prefix("rgba(")) {
        let p: Vec<u8> = inner.trim_end_matches(')').split([',', ' ', '/']).filter(|s| !s.is_empty()).take(3).filter_map(|s| s.trim().parse::<f32>().ok()).map(|x| x.clamp(0.0, 255.0) as u8).collect();
        return (p.len() == 3).then(|| Color32::from_rgb(p[0], p[1], p[2]));
    }
    let (r, g, b) = match v.as_str() {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (220, 40, 40),
        "darkred" | "maroon" => (139, 0, 0),
        "green" => (0, 128, 0),
        "darkgreen" => (0, 100, 0),
        "lime" => (50, 205, 50),
        "blue" => (30, 80, 230),
        "darkblue" | "navy" => (0, 0, 128),
        "teal" => (0, 128, 128),
        "olive" => (128, 128, 0),
        "orange" => (255, 140, 0),
        "purple" => (128, 0, 128),
        "yellow" => (255, 215, 0),
        "gray" | "grey" => (128, 128, 128),
        "silver" | "lightgray" | "lightgrey" => (192, 192, 192),
        "brown" => (139, 69, 19),
        "pink" => (255, 150, 180),
        "gold" => (255, 200, 0),
        _ => return None,
    };
    Some(Color32::from_rgb(r, g, b))
}

/// Cambios de estilo que aporta una etiqueta abierta.
#[derive(Default, Clone)]
struct Mods {
    bold: Option<bool>,
    italic: Option<bool>,
    code: bool,
    strike: bool,
    underline: bool,
    mark: bool,
    small: bool,
    sup: bool,
    sub: bool,
    color: Option<Color32>,
    bg: Option<Color32>,
    link: Option<String>,
    title: Option<String>,
    align: Option<Align>,
}

fn mods_for(name: &str, body: &str) -> Mods {
    let mut m = Mods::default();
    match name {
        "b" | "strong" | "th" | "dt" => m.bold = Some(true),
        "i" | "em" | "cite" | "var" | "dfn" | "address" => m.italic = Some(true),
        "code" | "kbd" | "samp" | "tt" => m.code = true,
        "s" | "del" | "strike" => m.strike = true,
        "u" | "ins" => m.underline = true,
        "mark" => m.mark = true,
        "small" => m.small = true,
        "sup" => m.sup = true,
        "sub" => m.sub = true,
        "a" => m.link = attr(body, "href"),
        "abbr" | "acronym" => {
            m.title = attr(body, "title");
            m.underline = m.title.is_some();
        }
        "center" => m.align = Some(Align::Center),
        "font" => m.color = attr(body, "color").and_then(|c| parse_color(&c)),
        _ => {}
    }
    if let Some(a) = attr(body, "align") {
        m.align = align_of(&a).or(m.align);
    }
    if let Some(t) = attr(body, "title").filter(|_| m.title.is_none() && name != "a") {
        m.title = Some(t);
    }
    if let Some(st) = attr(body, "style") {
        for decl in st.split(';') {
            let Some((k, v)) = decl.split_once(':') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase());
            let v = v.trim_end_matches("!important").trim();
            match k.as_str() {
                "color" => m.color = parse_color(v).or(m.color),
                "background-color" | "background" => m.bg = parse_color(v.split_whitespace().next().unwrap_or("")).or(m.bg),
                "font-weight" => m.bold = Some(v == "bold" || v == "bolder" || v.parse::<u32>().is_ok_and(|w| w >= 600)),
                "font-style" => m.italic = Some(v == "italic" || v == "oblique"),
                "text-decoration" | "text-decoration-line" => {
                    m.strike |= v.contains("line-through");
                    m.underline |= v.contains("underline");
                }
                "text-align" => m.align = align_of(v).or(m.align),
                _ => {}
            }
        }
    }
    m
}

fn align_of(v: &str) -> Option<Align> {
    match v.trim().to_ascii_lowercase().as_str() {
        "center" | "middle" => Some(Align::Center),
        "right" | "end" => Some(Align::Max),
        "left" | "start" | "justify" => Some(Align::Min),
        _ => None,
    }
}

#[derive(Default)]
struct TableState {
    rows: Vec<(bool, Vec<Vec<Span>>)>,
    row: Option<(bool, Vec<Vec<Span>>)>,
    cell: Option<Vec<Span>>,
    caption: Vec<Span>,
    in_caption: bool,
    line: u64,
}

impl TableState {
    fn end_cell(&mut self) {
        if let Some(c) = self.cell.take() {
            self.row.get_or_insert((false, Vec::new())).1.push(c);
        }
    }

    fn end_row(&mut self) {
        self.end_cell();
        if let Some(r) = self.row.take() {
            self.rows.push(r);
        }
    }
}

struct Parser {
    doc: Doc,
    cur: Vec<Span>,
    cur_kind: Kind,
    cur_line: u64,
    cur_align: Option<Align>,
    line: u64,
    stack: Vec<(String, Mods)>,
    pre: u32,
    quote: u32,
    lists: Vec<(bool, u32)>,
    tables: Vec<TableState>,
    title: Option<String>,
}

impl Parser {
    fn style(&self) -> Style {
        let mut s = Style::default();
        for (_, m) in &self.stack {
            if let Some(b) = m.bold {
                s.bold = b;
            }
            if let Some(i) = m.italic {
                s.italic = i;
            }
            s.code |= m.code && self.pre == 0;
            s.strike |= m.strike;
            s.underline |= m.underline;
            s.mark |= m.mark;
            s.small |= m.small;
            s.sup |= m.sup;
            s.sub |= m.sub;
            if m.color.is_some() {
                s.color = m.color;
            }
            if m.bg.is_some() {
                s.bg = m.bg;
            }
            if m.link.is_some() {
                s.link.clone_from(&m.link);
            }
            if m.title.is_some() {
                s.title.clone_from(&m.title);
            }
        }
        s
    }

    fn align(&self) -> Option<Align> {
        self.stack.iter().rev().find_map(|(_, m)| m.align)
    }

    /// Dónde va el texto: celda/pie de tabla o el bloque actual.
    fn target(&mut self) -> &mut Vec<Span> {
        if let Some(t) = self.tables.last_mut() {
            if t.in_caption {
                return &mut t.caption;
            }
            if let Some(c) = t.cell.as_mut() {
                return c;
            }
        }
        if self.cur.is_empty() {
            self.cur_line = self.line;
            self.cur_align = self.stack.iter().rev().find_map(|(_, m)| m.align);
        }
        &mut self.cur
    }

    fn push_span(&mut self, s: Span) {
        let target = self.target();
        if let Some(last) = target.last_mut() {
            if last.style == s.style && last.image.is_none() && s.image.is_none() {
                last.text.push_str(&s.text);
                return;
            }
        }
        target.push(s);
    }

    fn span(&mut self, text: &str) {
        if !text.is_empty() {
            let style = self.style();
            self.push_span(Span { text: text.to_string(), style, image: None });
        }
    }

    fn text(&mut self, raw: &str) {
        let t = decode_entities(raw);
        if self.pre > 0 {
            self.span(&t);
            return;
        }
        // Espacios seguidos → uno; al principio de un bloque, ninguno.
        let prev_space = {
            let target = self.target();
            target.last().map_or(true, |s| s.text.ends_with([' ', '\n']))
        };
        let mut out = String::with_capacity(t.len());
        let mut space = prev_space;
        for c in t.chars() {
            if c.is_whitespace() && c != '\u{a0}' {
                if !space {
                    out.push(' ');
                    space = true;
                }
            } else {
                out.push(c);
                space = false;
            }
        }
        self.span(&out);
    }

    fn flush(&mut self) {
        let mut spans = std::mem::take(&mut self.cur);
        if self.cur_kind != Kind::Pre {
            while let Some(last) = spans.last_mut() {
                if last.image.is_some() {
                    break;
                }
                let t = last.text.trim_end_matches([' ', '\n']).len();
                last.text.truncate(t);
                if last.text.is_empty() {
                    spans.pop();
                } else {
                    break;
                }
            }
        } else {
            if let Some(last) = spans.last_mut() {
                let t = last.text.trim_end_matches('\n').len();
                last.text.truncate(t);
            }
            if let Some(first) = spans.first_mut() {
                if first.text.starts_with('\n') {
                    first.text.remove(0);
                }
            }
        }
        if !spans.is_empty() || matches!(self.cur_kind, Kind::Rule) {
            let kind = std::mem::replace(&mut self.cur_kind, Kind::Para);
            self.doc.blocks.push(Block { kind, spans, line: self.cur_line, align: self.cur_align });
        }
        self.cur_kind = if self.quote > 0 { Kind::Quote } else { Kind::Para };
        self.cur_line = self.line;
        self.cur_align = self.align();
    }

    fn start_block(&mut self, kind: Kind) {
        self.flush();
        self.cur_kind = kind;
        self.cur_line = self.line;
        self.cur_align = self.align();
    }

    fn in_cell(&self) -> bool {
        self.tables.last().is_some_and(|t| t.cell.is_some() || t.in_caption)
    }

    fn open(&mut self, name: &str, body: &str) {
        // Anclas: el siguiente bloque que se cree.
        for key in ["id", "name"] {
            if let Some(id) = attr(body, key).filter(|s| !s.is_empty()) {
                let next = self.doc.blocks.len() + usize::from(!self.cur.is_empty() && is_block(name));
                self.doc.anchors.entry(id).or_insert(next);
            }
        }
        if !is_void(name) {
            self.stack.push((name.to_string(), mods_for(name, body)));
            if self.stack.len() > 512 {
                self.stack.remove(0);
            }
        }
        let in_table = !self.tables.is_empty();
        match name {
            "table" => {
                self.flush();
                self.tables.push(TableState { line: self.line, ..Default::default() });
            }
            "caption" if in_table => self.tables.last_mut().unwrap().in_caption = true,
            "tr" if in_table => {
                let t = self.tables.last_mut().unwrap();
                t.end_row();
                t.row = Some((false, Vec::new()));
            }
            "td" | "th" if in_table => {
                let t = self.tables.last_mut().unwrap();
                t.end_cell();
                let row = t.row.get_or_insert((false, Vec::new()));
                if name == "th" {
                    row.0 = true;
                }
                t.cell = Some(Vec::new());
            }
            _ if self.in_cell() && is_block(name) => self.text(" "),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.start_block(Kind::Heading(name.as_bytes()[1] - b'0')),
            "ul" | "menu" => {
                self.flush();
                self.lists.push((false, 0));
            }
            "ol" => {
                self.flush();
                let start = attr(body, "start").and_then(|s| s.trim().parse().ok()).unwrap_or(1u32);
                self.lists.push((true, start.saturating_sub(1)));
            }
            "li" => {
                let depth = self.lists.len().max(1) as u8 - 1;
                let mark = match self.lists.last_mut() {
                    Some((true, n)) => {
                        *n += 1;
                        format!("{n}.")
                    }
                    _ => ["•", "◦", "▪"][depth as usize % 3].to_string(),
                };
                self.start_block(Kind::Item { depth, mark });
            }
            "dt" => self.start_block(Kind::Para),
            "dd" => self.start_block(Kind::Item { depth: 0, mark: String::new() }),
            "figcaption" => self.start_block(Kind::Caption),
            "summary" => self.start_block(Kind::Summary),
            "pre" => {
                self.start_block(Kind::Pre);
                self.pre += 1;
            }
            "blockquote" => {
                self.flush();
                self.quote += 1;
                self.cur_kind = Kind::Quote;
            }
            "hr" => {
                self.start_block(Kind::Rule);
                self.flush();
            }
            "br" => self.span("\n"),
            "q" => self.span("“"),
            "img" => {
                let alt = attr(body, "alt").filter(|a| !a.trim().is_empty());
                let src = attr(body, "src").unwrap_or_default();
                let px = |k: &str| attr(body, k).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|v| *v > 0.0);
                let idx = (!src.is_empty()).then(|| {
                    self.doc.images.push(ImageRef { src: src.clone(), w: px("width"), h: px("height") });
                    self.doc.images.len() - 1
                });
                let name = alt.unwrap_or_else(|| src.rsplit('/').next().unwrap_or("").chars().take(60).collect());
                let style = self.style();
                self.push_span(Span { text: name, style, image: idx });
            }
            _ if is_block(name) => {
                let k = if self.quote > 0 { Kind::Quote } else { Kind::Para };
                self.start_block(k);
            }
            _ => {}
        }
    }

    fn close(&mut self, name: &str) {
        let in_table = !self.tables.is_empty();
        match name {
            "table" if in_table => {
                let mut t = self.tables.pop().unwrap();
                t.end_row();
                for (_, cells) in &mut t.rows {
                    for c in cells.iter_mut() {
                        if let Some(l) = c.last_mut().filter(|l| l.image.is_none()) {
                            let n = l.text.trim_end().len();
                            l.text.truncate(n);
                        }
                    }
                }
                t.rows.retain(|r| !r.1.is_empty());
                if !t.rows.is_empty() {
                    if let Some(outer) = self.tables.last_mut().and_then(|o| o.cell.as_mut()) {
                        // Tabla dentro de una celda: se aplana como texto.
                        for (_, cells) in t.rows {
                            for c in cells {
                                outer.extend(c);
                                outer.push(Span { text: " ".into(), ..Default::default() });
                            }
                        }
                    } else {
                        let kind = Kind::Table { rows: t.rows, caption: t.caption };
                        self.doc.blocks.push(Block { kind, spans: Vec::new(), line: t.line, align: None });
                    }
                }
            }
            "caption" if in_table => self.tables.last_mut().unwrap().in_caption = false,
            "td" | "th" if in_table => self.tables.last_mut().unwrap().end_cell(),
            "tr" if in_table => self.tables.last_mut().unwrap().end_row(),
            _ if self.in_cell() && is_block(name) => self.text(" "),
            "ul" | "ol" | "menu" => {
                self.flush();
                self.lists.pop();
            }
            "pre" => {
                self.flush();
                self.pre = self.pre.saturating_sub(1);
            }
            "blockquote" => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
                self.cur_kind = if self.quote > 0 { Kind::Quote } else { Kind::Para };
            }
            "q" => self.span("”"),
            _ if is_block(name) => self.flush(),
            _ => {}
        }
        // Cierra la etiqueta en la pila (y lo que quedara abierto dentro).
        if let Some(i) = self.stack.iter().rposition(|(n, _)| n == name) {
            self.stack.truncate(i);
        }
    }
}

/// Convierte el HTML en bloques.
pub fn parse(src: &str) -> Doc {
    let mut p = Parser {
        doc: Doc::default(),
        cur: Vec::new(),
        cur_kind: Kind::Para,
        cur_line: 0,
        cur_align: None,
        line: 0,
        stack: Vec::new(),
        pre: 0,
        quote: 0,
        lists: Vec::new(),
        tables: Vec::new(),
        title: None,
    };
    let lines = |s: &str| s.bytes().filter(|&b| b == b'\n').count() as u64;
    let mut rest = src;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            p.text(rest);
            break;
        };
        if lt > 0 {
            p.text(&rest[..lt]);
            p.line += lines(&rest[..lt]);
            rest = &rest[lt..];
        }
        // Comentario, DOCTYPE, CDATA.
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map_or(rest.len(), |e| e + 3);
            p.line += lines(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            let end = rest.find('>').map_or(rest.len(), |e| e + 1);
            p.line += lines(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        let after = &rest[1..];
        let closing = after.starts_with('/');
        let name_part = if closing { &after[1..] } else { after };
        let name_len = name_part.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == ':')).unwrap_or(name_part.len());
        if name_len == 0 {
            // «<» suelto: es texto.
            p.text("<");
            rest = &rest[1..];
            continue;
        }
        let name = name_part[..name_len].to_ascii_lowercase();
        // Fin de la etiqueta (respetando comillas).
        let mut q = None;
        let mut end = rest.len();
        for (i, c) in rest.char_indices().skip(1) {
            match (q, c) {
                (None, '"' | '\'') => q = Some(c),
                (Some(x), c) if c == x => q = None,
                (None, '>') => {
                    end = i + 1;
                    break;
                }
                _ => {}
            }
        }
        let body_start = 1 + usize::from(closing) + name_len;
        let body = &rest[body_start..end.saturating_sub(1).max(body_start)];
        p.line += lines(&rest[..end]);
        rest = &rest[end..];
        if closing {
            p.close(&name);
            continue;
        }
        // Contenido que no es HTML: se salta (o se guarda, el título).
        if matches!(name.as_str(), "script" | "style" | "title" | "textarea" | "noscript" | "template" | "svg" | "head")
            && !body.trim_end().ends_with('/')
        {
            if name == "head" {
                // Del <head> solo interesa el título.
                let close = find_ci(rest, "</head>").unwrap_or(rest.len());
                let head = &rest[..close];
                if let Some(ts) = find_ci(head, "<title") {
                    let s = &head[ts..];
                    if let Some(gt) = s.find('>') {
                        let e = find_ci(&s[gt..], "</title>").map_or(s.len(), |e| gt + e);
                        p.title = Some(decode_entities(s[gt + 1..e].trim()));
                    }
                }
                let consumed = (close + 7).min(rest.len());
                p.line += lines(&rest[..consumed]);
                rest = &rest[consumed..];
                continue;
            }
            let pat = format!("</{name}");
            let close = find_ci(rest, &pat).unwrap_or(rest.len());
            let inner = &rest[..close];
            if name == "title" {
                p.title = Some(decode_entities(inner.trim()));
            } else if name == "textarea" {
                p.text(inner);
            }
            let after_close = rest[close..].find('>').map_or(rest.len(), |e| close + e + 1);
            p.line += lines(&rest[..after_close]);
            rest = &rest[after_close..];
            continue;
        }
        p.open(&name, body);
    }
    while !p.tables.is_empty() {
        p.close("table");
    }
    p.flush();
    p.doc.title = p.title.unwrap_or_default();
    let n = p.doc.blocks.len();
    p.doc.anchors.retain(|_, b| *b < n);
    p.doc
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let n = needle.len();
    hay.as_bytes().windows(n).position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

// ---------------------------------------------------------------- dibujo

pub enum Action {
    Goto(u64),
    Open(String),
    /// Ventana aparte (`true`) o de vuelta junto al texto.
    Detach(bool),
    Close,
}

/// Ajusta un color del documento para que se lea sobre el fondo del tema.
fn readable(c: Color32, dark: bool) -> Option<Color32> {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if (dark && l < 70.0) || (!dark && l > 215.0) {
        return None; // negro sobre oscuro o blanco sobre claro: el del tema
    }
    Some(c)
}

fn rich(s: &Span, size: f32, ui: &Ui, heading: bool) -> RichText {
    let st = &s.style;
    let v = ui.visuals();
    let size = if st.small || st.sup || st.sub { size * 0.8 } else { size };
    let mut r = RichText::new(&s.text).size(size);
    if st.code {
        r = r.font(FontId::monospace(size * 0.92)).background_color(v.faint_bg_color.gamma_multiply(2.5));
    }
    if st.bold || heading {
        r = r.strong();
    }
    if st.italic || s.image.is_some() {
        r = r.italics();
    }
    if st.strike {
        r = r.strikethrough();
    }
    if st.underline {
        r = r.underline();
    }
    if st.sup {
        r = r.raised();
    }
    if let Some(c) = st.color.and_then(|c| readable(c, v.dark_mode)) {
        r = r.color(c);
    }
    if st.mark {
        r = r.background_color(Color32::from_rgba_unmultiplied(250, 210, 40, if v.dark_mode { 70 } else { 120 }));
    } else if let Some(bg) = st.bg {
        r = r.background_color(Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), if v.dark_mode { 70 } else { 110 }));
    }
    if s.image.is_some() {
        r = r.weak();
    }
    if st.link.is_some() {
        r = r.color(v.hyperlink_color).underline();
    }
    r
}

struct Ctx<'a> {
    doc: &'a Doc,
    textures: &'a HashMap<String, Option<TextureHandle>>,
    act: Option<Action>,
    scroll_to_anchor: Option<usize>,
}

/// Tamaño con el que se dibuja una imagen (atributos width/height, sin pasar de `max`).
fn image_size(img: &ImageRef, tex: &TextureHandle, max: f32) -> egui::Vec2 {
    let [tw, th] = tex.size().map(|v| v as f32);
    let (mut w, mut h) = match (img.w, img.h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w * th / tw),
        (None, Some(h)) => (h * tw / th, h),
        (None, None) => (tw, th),
    };
    let max = max.max(40.0);
    if w > max {
        h *= max / w;
        w = max;
    }
    vec2(w, h)
}

/// Ancho de un span en una sola línea (para centrar o alinear a la derecha).
fn span_width(ui: &Ui, s: &Span, size: f32, heading: bool, cx: &Ctx) -> f32 {
    if let Some(i) = s.image {
        let img = &cx.doc.images[i];
        if let Some(Some(tex)) = cx.textures.get(&img.src) {
            return image_size(img, tex, ui.available_width()).x;
        }
        let label = format!("[{}: {}]", t!("imagen"), s.text);
        let sp = Span { text: label, ..s.clone() };
        return egui::WidgetText::from(rich(&sp, size, ui, heading)).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body).size().x;
    }
    if s.text.contains('\n') {
        return f32::INFINITY;
    }
    egui::WidgetText::from(rich(s, size, ui, heading)).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body).size().x
}

fn span_ui(ui: &mut Ui, s: &Span, size: f32, heading: bool, cx: &mut Ctx, line: u64) {
    if let Some(i) = s.image {
        let img = &cx.doc.images[i];
        if let Some(Some(tex)) = cx.textures.get(&img.src) {
            let sz = image_size(img, tex, ui.available_width());
            let r = ui.add(egui::Image::new((tex.id(), sz)).sense(Sense::click()));
            let r = if s.text.is_empty() { r } else { r.on_hover_text(&s.text) };
            if r.clicked() {
                cx.act = Some(Action::Goto(line));
            }
            return;
        }
        let label = format!("[{}: {}]", t!("imagen"), s.text);
        let r = ui.add(Label::new(rich(&Span { text: label, ..s.clone() }, size, ui, heading)).sense(Sense::click()));
        if r.on_hover_text(&img.src).clicked() {
            cx.act = Some(Action::Goto(line));
        }
        return;
    }
    let r = ui.add(Label::new(rich(s, size, ui, heading)).sense(Sense::click()));
    let r = match &s.style.title {
        Some(t) => r.on_hover_text(t.as_str()),
        None => r,
    };
    if let Some(href) = &s.style.link {
        let r = r.on_hover_text(href.as_str()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() {
            if let Some(b) = href.strip_prefix('#').and_then(|id| cx.doc.anchors.get(id)) {
                cx.scroll_to_anchor = Some(*b);
            } else {
                cx.act = Some(Action::Open(href.clone()));
            }
        }
    } else if r.clicked() {
        cx.act = Some(Action::Goto(line));
    }
}

/// Dibuja los spans de un bloque con ajuste de línea.
fn spans_ui(ui: &mut Ui, spans: &[Span], size: f32, heading: bool, cx: &mut Ctx, line: u64, align: Option<Align>) {
    let avail = ui.available_size_before_wrap().x;
    // Centrado / derecha: si cabe en una línea, se desplaza; si no, a la izquierda.
    let shift = match align {
        Some(a @ (Align::Center | Align::Max)) => {
            let w: f32 = spans.iter().map(|s| span_width(ui, s, size, heading, cx)).sum();
            if w < avail { if a == Align::Center { (avail - w) / 2.0 } else { avail - w } } else { 0.0 }
        }
        _ => 0.0,
    };
    let layout = egui::Layout::left_to_right(Align::Center).with_main_wrap(true);
    ui.allocate_ui_with_layout(vec2(avail, 0.0), layout, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        if shift > 0.0 {
            ui.add_space(shift);
        }
        for s in spans {
            span_ui(ui, s, size, heading, cx, line);
        }
    });
}

/// Barra de arriba de la vista: índice, tamaño, seguir cursor, ventana aparte.
fn header(ui: &mut Ui, v: &mut HtmlView, cx_act: &mut Option<Action>, scroll_to: &mut Option<usize>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let heads: Vec<(usize, u8, String)> = v
            .doc
            .as_ref()
            .map(|d| {
                d.blocks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, b)| match b.kind {
                        Kind::Heading(n) => Some((i, n, b.spans.iter().map(|s| s.text.as_str()).collect::<String>())),
                        _ => None,
                    })
                    .take(500)
                    .collect()
            })
            .unwrap_or_default();
        ui.add_enabled_ui(!heads.is_empty(), |ui| {
            egui::containers::menu::MenuButton::from_button(egui::Button::new(RichText::new(format!("☰ {}", t!("Índice"))).small()).frame(false))
                .ui(ui, |ui| {
                    ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        for (i, n, text) in &heads {
                            let indent = "    ".repeat((*n as usize).saturating_sub(1));
                            let t: String = text.chars().take(80).collect();
                            if ui.button(format!("{indent}{t}")).clicked() {
                                *scroll_to = Some(*i);
                                ui.close();
                            }
                        }
                    });
                })
                .0
                .on_hover_text(t!("Ir a un encabezado del documento"));
        });
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if ui.add(IconButton::new(Icon::Collapse, Tone::Neutral).small()).on_hover_text(t!("Cerrar la vista de lectura")).clicked() {
                *cx_act = Some(Action::Close);
            }
            let (icon, hint) = if v.detached {
                (Icon::Attach, t!("Volver a ponerla junto al texto"))
            } else {
                (Icon::Detach, t!("Ver en una ventana aparte (se puede llevar a otra pantalla)"))
            };
            if ui.add(IconButton::new(icon, Tone::File).small()).on_hover_text(hint).clicked() {
                *cx_act = Some(Action::Detach(!v.detached));
            }
            if ui
                .add(IconButton::new(Icon::Target, Tone::Search).label(t!("Seguir cursor")).small().selected(v.follow))
                .on_hover_text(t!("Resaltar y mantener a la vista el bloque donde está el cursor del editor"))
                .clicked()
            {
                v.follow = !v.follow;
            }
            if ui.add(IconButton::text_only("A+", Tone::Neutral).small()).on_hover_text(t!("Letra más grande")).clicked() {
                v.zoom = (v.zoom + 0.1).min(2.5);
            }
            if ui.add(IconButton::text_only("A−", Tone::Neutral).small()).on_hover_text(t!("Letra más pequeña")).clicked() {
                v.zoom = (v.zoom - 0.1).max(0.6);
            }
        });
    });
    ui.separator();
}

/// Vista completa (barra + contenido); devuelve la acción pedida.
pub fn show(ui: &mut Ui, v: &mut HtmlView, base: f32, cursor_line: Option<u64>) -> Option<Action> {
    let mut act = None;
    let mut scroll_to = v.scroll_to.take();
    header(ui, v, &mut act, &mut scroll_to);
    let Some(doc) = v.doc.clone() else {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            if v.too_big {
                ui.label(t!("Demasiado grande para la vista de lectura: usa «Navegador»."));
            } else {
                ui.spinner();
            }
        });
        return act;
    };
    let base = base * v.zoom;
    // Bloque del cursor (resaltado y, si cambió, a la vista).
    let cur_block = cursor_line.filter(|_| v.follow).and_then(|l| doc.block_at(l));
    let cursor_moved = v.last_cursor != cursor_line;
    v.last_cursor = cursor_line;
    let reveal = if cursor_moved { cur_block } else { None };
    let fg = ui.visuals().text_color();
    let hl = ui.visuals().selection.bg_fill.gamma_multiply(0.22);
    let mut cx = Ctx { doc: &doc, textures: &v.textures, act: None, scroll_to_anchor: None };
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        Frame::NONE.inner_margin(Margin { left: 16, right: 16, top: 10, bottom: 16 }).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            if !doc.title.is_empty() {
                ui.label(RichText::new(&doc.title).size(base * 0.95).weak());
                ui.add_space(6.0);
            }
            if doc.blocks.is_empty() {
                ui.weak(t!("(sin contenido visible)"));
            }
            for (bi, b) in doc.blocks.iter().enumerate().take(MAX_BLOCKS) {
                let bg = ui.painter().add(Shape::Noop);
                let top = ui.cursor().top();
                match &b.kind {
                    Kind::Heading(n) => {
                        let size = base * [2.0, 1.6, 1.35, 1.15, 1.05, 1.0][(*n as usize).clamp(1, 6) - 1];
                        ui.add_space(base * 0.5);
                        spans_ui(ui, &b.spans, size, true, &mut cx, b.line, b.align);
                        if *n <= 2 {
                            let r = ui.available_rect_before_wrap();
                            ui.painter().hline(r.x_range(), r.top() + 2.0, Stroke::new(1.0, fg.gamma_multiply(0.15)));
                            ui.add_space(5.0);
                        }
                    }
                    Kind::Para => spans_ui(ui, &b.spans, base, false, &mut cx, b.line, b.align),
                    Kind::Caption => {
                        let spans: Vec<Span> = b.spans.iter().cloned().map(|mut s| {
                            s.style.italic = true;
                            s.style.small = true;
                            s
                        }).collect();
                        spans_ui(ui, &spans, base, false, &mut cx, b.line, b.align.or(Some(Align::Center)));
                    }
                    Kind::Summary => {
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new("▸ ").size(base).color(fg.gamma_multiply(0.7)));
                            ui.vertical(|ui| spans_ui(ui, &b.spans, base, true, &mut cx, b.line, None));
                        });
                    }
                    Kind::Item { depth, mark } => {
                        ui.horizontal_top(|ui| {
                            ui.add_space(6.0 + *depth as f32 * base * 1.4);
                            let w = base * 1.4;
                            ui.allocate_ui(vec2(w, base * 1.2), |ui| {
                                ui.label(RichText::new(mark).size(base).color(fg.gamma_multiply(0.7)));
                            });
                            ui.vertical(|ui| spans_ui(ui, &b.spans, base, false, &mut cx, b.line, b.align));
                        });
                    }
                    Kind::Quote => {
                        let r = ui
                            .horizontal_top(|ui| {
                                ui.add_space(12.0);
                                ui.vertical(|ui| spans_ui(ui, &b.spans, base, false, &mut cx, b.line, b.align));
                            })
                            .response
                            .rect;
                        ui.painter().vline(r.left() + 3.0, r.y_range(), Stroke::new(3.0, fg.gamma_multiply(0.25)));
                    }
                    Kind::Pre => {
                        let r = Frame::NONE.fill(ui.visuals().extreme_bg_color).corner_radius(6).inner_margin(8).show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            let text: String = b.spans.iter().map(|s| s.text.as_str()).collect();
                            ui.add(Label::new(RichText::new(text).font(FontId::monospace(base * 0.9))).sense(Sense::click()))
                        });
                        if r.inner.clicked() {
                            cx.act = Some(Action::Goto(b.line));
                        }
                    }
                    Kind::Rule => {
                        ui.separator();
                    }
                    Kind::Table { rows, caption } => {
                        if !caption.is_empty() {
                            spans_ui(ui, caption, base * 0.95, true, &mut cx, b.line, None);
                        }
                        ScrollArea::horizontal().id_salt(("tabla_html_h", b.line)).show(ui, |ui| {
                            egui::Grid::new(("tabla_html", b.line)).striped(true).spacing(vec2(14.0, 4.0)).show(ui, |ui| {
                                for (head, cells) in rows {
                                    for c in cells {
                                        // Las celdas no se parten: la tabla toma el ancho que necesite.
                                        ui.horizontal(|ui| {
                                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                            ui.spacing_mut().item_spacing.x = 0.0;
                                            for s in c {
                                                span_ui(ui, s, base * 0.95, *head, &mut cx, b.line);
                                            }
                                        });
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                    }
                }
                let rect = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=ui.cursor().top());
                if Some(bi) == cur_block {
                    ui.painter().set(bg, Shape::rect_filled(rect.expand2(vec2(8.0, 2.0)), 4.0, hl));
                }
                if Some(bi) == reveal {
                    ui.scroll_to_rect(rect, Some(Align::Center));
                }
                if Some(bi) == scroll_to {
                    ui.scroll_to_rect(rect, Some(Align::Min));
                }
                ui.add_space(match b.kind {
                    Kind::Heading(_) => base * 0.2,
                    Kind::Item { .. } | Kind::Summary => base * 0.2,
                    Kind::Rule => 0.0,
                    _ => base * 0.55,
                });
            }
            if doc.blocks.len() > MAX_BLOCKS {
                ui.weak(tf!("… y {n} bloques más (vista recortada)", n = doc.blocks.len() - MAX_BLOCKS));
            }
        });
    });
    // Un enlace #ancla se ve en el siguiente fotograma.
    if let Some(b) = cx.scroll_to_anchor {
        v.scroll_to = Some(b);
        ui.ctx().request_repaint();
    }
    act.or(cx.act)
}

/// Copia para abrir en el navegador: añade `<base href>` para que las rutas
/// relativas (imágenes, CSS) apunten a la carpeta del original.
pub fn with_base(html: &str, dir: &Path) -> String {
    let mut url = String::from("file://");
    let d = dir.to_string_lossy().replace('\\', "/");
    if !d.starts_with('/') {
        url.push('/');
    }
    for c in d.chars() {
        match c {
            ' ' => url.push_str("%20"),
            '#' => url.push_str("%23"),
            '%' => url.push_str("%25"),
            '?' => url.push_str("%3F"),
            _ => url.push(c),
        }
    }
    if !url.ends_with('/') {
        url.push('/');
    }
    let tag = format!("<base href=\"{url}\">");
    let at = find_ci(html, "<head").and_then(|i| html[i..].find('>').map(|e| i + e + 1)).unwrap_or(0);
    format!("{}{}{}", &html[..at], tag, &html[at..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(b: &Block) -> String {
        b.spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn blocks_and_inline() {
        let d = parse(
            "<!DOCTYPE html>\n<html><head><title>Hola &amp; adiós</title><style>p{}</style></head>\n<body>\n<h1>Título</h1>\n<p>Un <b>texto</b> con\n   <a href=\"https://x.org\">enlace</a> y &eacute;&#241;.</p>\n<ul><li>uno</li><li>dos<ol><li>a</li></ol></li></ul>\n<pre>  x\n  y</pre><script>alert(1)</script>\n<table><caption>Pie</caption><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>\n</body></html>",
        );
        assert_eq!(d.title, "Hola & adiós");
        let k: Vec<&Kind> = d.blocks.iter().map(|b| &b.kind).collect();
        assert_eq!(k[0], &Kind::Heading(1));
        assert_eq!(texts(&d.blocks[0]), "Título");
        assert_eq!(d.blocks[0].line, 3);
        assert_eq!(texts(&d.blocks[1]), "Un texto con enlace y éñ.");
        assert!(d.blocks[1].spans.iter().any(|s| s.style.bold && s.text == "texto"));
        assert!(d.blocks[1].spans.iter().any(|s| s.style.link.as_deref() == Some("https://x.org")));
        assert_eq!(d.blocks[1].line, 4);
        assert_eq!(texts(&d.blocks[2]), "uno");
        assert!(matches!(&d.blocks[4].kind, Kind::Item { depth: 1, mark } if mark == "1."));
        assert_eq!(texts(&d.blocks[5]), "  x\n  y");
        assert!(!d.blocks.iter().any(|b| texts(b).contains("alert")));
        let Kind::Table { rows, caption } = &d.blocks[6].kind else { panic!("{:?}", d.blocks[6]) };
        assert_eq!(rows.len(), 2);
        assert!(rows[0].0);
        assert_eq!(rows[1].1[1][0].text, "2");
        assert_eq!(caption[0].text, "Pie");
    }

    #[test]
    fn styles_anchors_images() {
        let d = parse(
            "<h2 id=\"uno\">A</h2><p style=\"text-align:center; color: #ff0000\">x <del>y</del> <mark>z</mark> H<sub>2</sub>O <abbr title=\"Hola\">HL</abbr></p>\n<p><font color=\"blue\">azul</font><span style=\"font-weight:700\">n</span> <img src=\"a.png\" width=\"20\" alt=\"logo\"></p><a name=\"dos\"></a><p>fin</p>",
        );
        assert_eq!(d.anchors.get("uno"), Some(&0));
        assert_eq!(d.anchors.get("dos"), Some(&3));
        let p = &d.blocks[1];
        assert_eq!(p.align, Some(Align::Center));
        assert_eq!(p.spans[0].style.color, Some(Color32::from_rgb(255, 0, 0)));
        assert!(p.spans.iter().any(|s| s.text == "y" && s.style.strike));
        assert!(p.spans.iter().any(|s| s.text == "z" && s.style.mark));
        assert!(p.spans.iter().any(|s| s.text == "2" && s.style.sub));
        assert!(p.spans.iter().any(|s| s.text == "HL" && s.style.title.as_deref() == Some("Hola")));
        let q = &d.blocks[2];
        assert_eq!(q.spans[0].style.color, Some(Color32::from_rgb(30, 80, 230)));
        assert!(q.spans.iter().any(|s| s.text == "n" && s.style.bold));
        let img = q.spans.iter().find(|s| s.image.is_some()).unwrap();
        assert_eq!(img.text, "logo");
        assert_eq!(d.images[0], ImageRef { src: "a.png".into(), w: Some(20.0), h: None });
        assert_eq!(parse_color("#0a0"), Some(Color32::from_rgb(0, 170, 0)));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some(Color32::from_rgb(1, 2, 3)));
    }

    #[test]
    fn data_image() {
        // PNG de 1×1 en Base64.
        let src = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";
        let img = image_bytes(src, None).and_then(|b| decode_image(&b)).expect("decodifica");
        assert_eq!(img.size, [1, 1]);
        assert_eq!(percent_decode("a%20b%zz"), "a b%zz");
    }

    #[test]
    fn base_href() {
        let h = with_base("<html><HEAD><meta></head>", Path::new("/a b/c"));
        assert_eq!(h, "<html><HEAD><base href=\"file:///a%20b/c/\"><meta></head>");
        assert_eq!(decode_entities("a &lt;b&gt; &unknown; &#x41;"), "a <b> &unknown; A");
    }
}
