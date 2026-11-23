//! Vista de lectura de HTML: convierte el documento en bloques (títulos,
//! párrafos, listas, citas, código, tablas) sin CSS ni JavaScript y los muestra
//! formateados. El análisis es propio (tolerante, sin árbol) y se hace en un
//! hilo cuando cambia el texto. Cada bloque recuerda su línea en el código:
//! clic en el bloque → el editor va a esa línea.

use eframe::egui::{self, FontId, Frame, Label, Margin, RichText, ScrollArea, Sense, Stroke, Ui, vec2};
use std::sync::mpsc::{Receiver, channel};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Máximo que se analiza (más grande → se sugiere el navegador).
pub const MAX_BYTES: usize = 16 << 20;
/// Máximo de bloques que se dibujan.
const MAX_BLOCKS: usize = 4000;

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
    pub image: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Heading(u8),
    Para,
    Item { depth: u8, mark: String },
    Quote,
    Pre,
    Rule,
    /// Filas → celdas → spans; `bool` = fila de cabecera.
    Table(Vec<(bool, Vec<Vec<Span>>)>),
}

#[derive(Clone, Debug)]
pub struct Block {
    pub kind: Kind,
    pub spans: Vec<Span>,
    /// Línea (desde 0) del código donde empieza.
    pub line: u64,
}

#[derive(Default, Debug)]
pub struct Doc {
    pub title: String,
    pub blocks: Vec<Block>,
}

/// Estado de la vista en una pestaña.
#[derive(Default)]
pub struct HtmlView {
    pub on: bool,
    doc: Option<Arc<Doc>>,
    version: Option<u64>,
    job: Option<(u64, Receiver<Doc>)>,
    changed_at: Option<Instant>,
    pub too_big: bool,
}

impl HtmlView {
    pub fn toggle(&mut self) {
        self.on = !self.on;
        if !self.on {
            // Libera la memoria; se rehace al volver a abrirla.
            self.doc = None;
            self.version = None;
            self.job = None;
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

    /// Analiza `text` (versión `v`) en un hilo.
    pub fn start(&mut self, v: u64, text: Option<String>) {
        self.changed_at = None;
        let Some(text) = text else {
            self.too_big = true;
            self.version = Some(v);
            self.doc = None;
            return;
        };
        self.too_big = false;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(parse(&text));
        });
        self.job = Some((v, rx));
    }

    /// Recoge el resultado; `true` si sigue trabajando.
    pub fn poll(&mut self) -> bool {
        let Some((v, rx)) = &self.job else { return self.changed_at.is_some() };
        match rx.try_recv() {
            Ok(d) => {
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
        "times" => '×',
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
        "ccedil" => 'ç',
        "agrave" => 'à',
        "egrave" => 'è',
        "rarr" => '→',
        "larr" => '←',
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

struct Parser {
    doc: Doc,
    cur: Vec<Span>,
    cur_kind: Kind,
    cur_line: u64,
    line: u64,
    bold: u32,
    italic: u32,
    code: u32,
    pre: u32,
    quote: u32,
    links: Vec<Option<String>>,
    lists: Vec<(bool, u32)>,
    /// Tabla en curso: filas, fila actual, celda actual.
    table: Vec<(Vec<(bool, Vec<Vec<Span>>)>, Option<(bool, Vec<Vec<Span>>)>, Option<Vec<Span>>, u64)>,
    title: Option<String>,
}

impl Parser {
    fn span(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let target = match self.table.last_mut().and_then(|t| t.2.as_mut()) {
            Some(cell) => cell,
            None => {
                if self.cur.is_empty() {
                    self.cur_line = self.line;
                }
                &mut self.cur
            }
        };
        let s = Span {
            text: text.to_string(),
            bold: self.bold > 0,
            italic: self.italic > 0,
            code: self.code > 0 && self.pre == 0,
            link: self.links.iter().rev().find_map(|l| l.clone()),
            image: false,
        };
        // Une con el anterior si tiene el mismo estilo.
        if let Some(last) = target.last_mut() {
            if last.bold == s.bold && last.italic == s.italic && last.code == s.code && last.link == s.link && !last.image {
                last.text.push_str(&s.text);
                return;
            }
        }
        target.push(s);
    }

    fn text(&mut self, raw: &str) {
        let t = decode_entities(raw);
        if self.pre > 0 {
            self.span(&t);
            return;
        }
        // Espacios seguidos → uno; al principio de un bloque, ninguno.
        let mut out = String::with_capacity(t.len());
        let in_cell = self.table.last().is_some_and(|t| t.2.is_some());
        let prev_space = if in_cell {
            self.table.last().and_then(|t| t.2.as_ref()).and_then(|c| c.last()).map_or(true, |s| s.text.ends_with([' ', '\n']))
        } else {
            self.cur.last().map_or(true, |s| s.text.ends_with([' ', '\n']))
        };
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
            // Quita espacios del final.
            while let Some(last) = spans.last_mut() {
                let t = last.text.trim_end_matches([' ', '\n']).len();
                last.text.truncate(t);
                if last.text.is_empty() && !last.image {
                    spans.pop();
                } else {
                    break;
                }
            }
        } else if let Some(last) = spans.last_mut() {
            let t = last.text.trim_end_matches('\n').len();
            last.text.truncate(t);
            if let Some(first) = spans.first_mut() {
                if first.text.starts_with('\n') {
                    first.text.remove(0);
                }
            }
        }
        if !spans.is_empty() || matches!(self.cur_kind, Kind::Rule) {
            let kind = std::mem::replace(&mut self.cur_kind, Kind::Para);
            self.doc.blocks.push(Block { kind, spans, line: self.cur_line });
        }
        self.cur_kind = if self.quote > 0 { Kind::Quote } else { Kind::Para };
        self.cur_line = self.line;
    }

    fn start_block(&mut self, kind: Kind) {
        self.flush();
        self.cur_kind = kind;
        self.cur_line = self.line;
    }

    fn open(&mut self, name: &str, body: &str) {
        let in_table = !self.table.is_empty();
        match name {
            "table" => {
                self.flush();
                self.table.push((Vec::new(), None, None, self.line));
            }
            "tr" if in_table => {
                let t = self.table.last_mut().unwrap();
                if let Some(cell) = t.2.take() {
                    t.1.get_or_insert((false, Vec::new())).1.push(cell);
                }
                if let Some(row) = t.1.take() {
                    t.0.push(row);
                }
                t.1 = Some((false, Vec::new()));
            }
            "td" | "th" if in_table => {
                let t = self.table.last_mut().unwrap();
                let row = t.1.get_or_insert((false, Vec::new()));
                if let Some(cell) = t.2.take() {
                    row.1.push(cell);
                }
                if name == "th" {
                    row.0 = true;
                }
                t.2 = Some(Vec::new());
            }
            _ if in_table && self.table.last().is_some_and(|t| t.2.is_some()) && is_block(name) && name != "br" => {
                // Bloques dentro de una celda: solo un espacio.
                self.text(" ");
            }
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
            "b" | "strong" => self.bold += 1,
            "i" | "em" | "cite" | "var" | "dfn" => self.italic += 1,
            "code" | "kbd" | "samp" | "tt" => self.code += 1,
            "a" => self.links.push(attr(body, "href")),
            "img" => {
                let alt = attr(body, "alt").filter(|a| !a.trim().is_empty());
                let src = attr(body, "src").unwrap_or_default();
                let name = alt.unwrap_or_else(|| src.rsplit('/').next().unwrap_or("").to_string());
                let target = match self.table.last_mut().and_then(|t| t.2.as_mut()) {
                    Some(c) => c,
                    None => {
                        if self.cur.is_empty() {
                            self.cur_line = self.line;
                        }
                        &mut self.cur
                    }
                };
                target.push(Span { text: format!("[{}: {name}]", t!("imagen")), image: true, ..Default::default() });
            }
            "input" | "button" | "select" | "textarea" => {}
            _ if is_block(name) => {
                let k = if self.quote > 0 { Kind::Quote } else { Kind::Para };
                self.start_block(k);
            }
            _ => {}
        }
    }

    fn close(&mut self, name: &str) {
        let in_table = !self.table.is_empty();
        match name {
            "table" if in_table => {
                let (mut rows, row, cell, line) = self.table.pop().unwrap();
                let mut row = row;
                if let Some(c) = cell {
                    row.get_or_insert((false, Vec::new())).1.push(c);
                }
                if let Some(r) = row {
                    rows.push(r);
                }
                for (_, cells) in &mut rows {
                    for c in cells.iter_mut() {
                        if let Some(l) = c.last_mut() {
                            let n = l.text.trim_end().len();
                            l.text.truncate(n);
                        }
                    }
                }
                rows.retain(|r| !r.1.is_empty());
                if !rows.is_empty() {
                    if let Some(outer) = self.table.last_mut().and_then(|t| t.2.as_mut()) {
                        // Tabla dentro de una celda: se aplana como texto.
                        for (_, cells) in rows {
                            for c in cells {
                                outer.extend(c);
                                outer.push(Span { text: " ".into(), ..Default::default() });
                            }
                        }
                    } else {
                        self.doc.blocks.push(Block { kind: Kind::Table(rows), spans: Vec::new(), line });
                    }
                }
            }
            "td" | "th" if in_table => {
                let t = self.table.last_mut().unwrap();
                if let Some(cell) = t.2.take() {
                    t.1.get_or_insert((false, Vec::new())).1.push(cell);
                }
            }
            "tr" if in_table => {
                let t = self.table.last_mut().unwrap();
                if let Some(cell) = t.2.take() {
                    t.1.get_or_insert((false, Vec::new())).1.push(cell);
                }
                if let Some(row) = t.1.take() {
                    t.0.push(row);
                }
            }
            _ if in_table && self.table.last().is_some_and(|t| t.2.is_some()) && is_block(name) => self.text(" "),
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
            "b" | "strong" => self.bold = self.bold.saturating_sub(1),
            "i" | "em" | "cite" | "var" | "dfn" => self.italic = self.italic.saturating_sub(1),
            "code" | "kbd" | "samp" | "tt" => self.code = self.code.saturating_sub(1),
            "a" => {
                self.links.pop();
            }
            _ if is_block(name) => self.flush(),
            _ => {}
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
        line: 0,
        bold: 0,
        italic: 0,
        code: 0,
        pre: 0,
        quote: 0,
        links: Vec::new(),
        lists: Vec::new(),
        table: Vec::new(),
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
        let body = &rest[1 + usize::from(closing) + name_len..end.saturating_sub(1).max(1 + usize::from(closing) + name_len)];
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
    while !p.table.is_empty() {
        p.close("table");
    }
    p.flush();
    p.doc.title = p.title.unwrap_or_default();
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
}

fn rich(s: &Span, size: f32, ui: &Ui, heading: bool) -> RichText {
    let mut r = RichText::new(&s.text).size(size);
    if s.code {
        r = r.font(FontId::monospace(size * 0.92)).background_color(ui.visuals().faint_bg_color.gamma_multiply(2.0));
    }
    if s.bold || heading {
        r = r.strong();
    }
    if s.italic {
        r = r.italics();
    }
    if s.image {
        r = r.weak().italics();
    }
    if s.link.is_some() {
        r = r.color(ui.visuals().hyperlink_color).underline();
    }
    r
}

/// Dibuja los spans de un bloque; devuelve si se hizo clic en el texto o un enlace.
fn spans_ui(ui: &mut Ui, spans: &[Span], size: f32, heading: bool, act: &mut Option<Action>, line: u64) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        for s in spans {
            let r = ui.add(Label::new(rich(s, size, ui, heading)).sense(Sense::click()));
            if let Some(href) = &s.link {
                let r = r.on_hover_text(href.as_str()).on_hover_cursor(egui::CursorIcon::PointingHand);
                if r.clicked() {
                    *act = Some(Action::Open(href.clone()));
                }
            } else if r.clicked() {
                *act = Some(Action::Goto(line));
            }
        }
    });
}

/// Panel de la vista; devuelve la acción pedida (ir a una línea, abrir un enlace).
pub fn show(ui: &mut Ui, v: &HtmlView, base: f32) -> Option<Action> {
    let mut act = None;
    let Some(doc) = v.doc.clone() else {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            if v.too_big {
                ui.label(t!("Demasiado grande para la vista de lectura: usa «Navegador»."));
            } else {
                ui.spinner();
            }
        });
        return None;
    };
    let fg = ui.visuals().text_color();
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        Frame::NONE.inner_margin(Margin { left: 14, right: 14, top: 10, bottom: 14 }).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            if !doc.title.is_empty() {
                ui.label(RichText::new(&doc.title).size(base * 0.95).weak());
                ui.add_space(6.0);
            }
            if doc.blocks.is_empty() {
                ui.weak(t!("(sin contenido visible)"));
            }
            for b in doc.blocks.iter().take(MAX_BLOCKS) {
                match &b.kind {
                    Kind::Heading(n) => {
                        let size = base * [2.0, 1.6, 1.35, 1.15, 1.05, 1.0][(*n as usize).clamp(1, 6) - 1];
                        ui.add_space(base * 0.5);
                        spans_ui(ui, &b.spans, size, true, &mut act, b.line);
                        if *n <= 2 {
                            let r = ui.available_rect_before_wrap();
                            ui.painter().hline(r.x_range(), r.top() + 2.0, Stroke::new(1.0, fg.gamma_multiply(0.15)));
                            ui.add_space(5.0);
                        }
                        ui.add_space(base * 0.2);
                    }
                    Kind::Para => {
                        spans_ui(ui, &b.spans, base, false, &mut act, b.line);
                        ui.add_space(base * 0.55);
                    }
                    Kind::Item { depth, mark } => {
                        ui.horizontal_top(|ui| {
                            ui.add_space(6.0 + *depth as f32 * base * 1.4);
                            let w = base * 1.4;
                            ui.allocate_ui(vec2(w, base * 1.2), |ui| {
                                ui.label(RichText::new(mark).size(base).color(fg.gamma_multiply(0.7)));
                            });
                            ui.vertical(|ui| spans_ui(ui, &b.spans, base, false, &mut act, b.line));
                        });
                        ui.add_space(base * 0.2);
                    }
                    Kind::Quote => {
                        let r = ui
                            .horizontal_top(|ui| {
                                ui.add_space(12.0);
                                ui.vertical(|ui| spans_ui(ui, &b.spans, base, false, &mut act, b.line));
                            })
                            .response
                            .rect;
                        ui.painter().vline(r.left() + 3.0, r.y_range(), Stroke::new(3.0, fg.gamma_multiply(0.25)));
                        ui.add_space(base * 0.55);
                    }
                    Kind::Pre => {
                        let r = Frame::NONE
                            .fill(ui.visuals().extreme_bg_color)
                            .corner_radius(6)
                            .inner_margin(8)
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                let text: String = b.spans.iter().map(|s| s.text.as_str()).collect();
                                ui.add(Label::new(RichText::new(text).font(FontId::monospace(base * 0.9))).sense(Sense::click()))
                            });
                        if r.inner.clicked() {
                            act = Some(Action::Goto(b.line));
                        }
                        ui.add_space(base * 0.55);
                    }
                    Kind::Rule => {
                        ui.separator();
                    }
                    Kind::Table(rows) => {
                        ScrollArea::horizontal().id_salt(("tabla_html_h", b.line)).show(ui, |ui| {
                        egui::Grid::new(("tabla_html", b.line)).striped(true).spacing(vec2(14.0, 4.0)).show(ui, |ui| {
                            for (head, cells) in rows {
                                for c in cells {
                                    // Las celdas no se parten: la tabla toma el ancho que necesite.
                                    ui.horizontal(|ui| {
                                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        for s in c {
                                            let r = ui.add(Label::new(rich(s, base * 0.95, ui, *head)).sense(Sense::click()));
                                            match &s.link {
                                                Some(h) if r.clicked() => act = Some(Action::Open(h.clone())),
                                                None if r.clicked() => act = Some(Action::Goto(b.line)),
                                                _ => {}
                                            }
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                        });
                        });
                        ui.add_space(base * 0.55);
                    }
                }
            }
            if doc.blocks.len() > MAX_BLOCKS {
                ui.weak(tf!("… y {n} bloques más (vista recortada)", n = doc.blocks.len() - MAX_BLOCKS));
            }
        });
    });
    act
}

/// Copia para abrir en el navegador: añade `<base href>` para que las rutas
/// relativas (imágenes, CSS) apunten a la carpeta del original.
pub fn with_base(html: &str, dir: &std::path::Path) -> String {
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
            "<!DOCTYPE html>\n<html><head><title>Hola &amp; adiós</title><style>p{}</style></head>\n<body>\n<h1>Título</h1>\n<p>Un <b>texto</b> con\n   <a href=\"https://x.org\">enlace</a> y &eacute;&#241;.</p>\n<ul><li>uno</li><li>dos<ol><li>a</li></ol></li></ul>\n<pre>  x\n  y</pre><script>alert(1)</script>\n<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>\n</body></html>",
        );
        assert_eq!(d.title, "Hola & adiós");
        let k: Vec<&Kind> = d.blocks.iter().map(|b| &b.kind).collect();
        assert_eq!(k[0], &Kind::Heading(1));
        assert_eq!(texts(&d.blocks[0]), "Título");
        assert_eq!(d.blocks[0].line, 3);
        assert_eq!(texts(&d.blocks[1]), "Un texto con enlace y éñ.");
        assert!(d.blocks[1].spans.iter().any(|s| s.bold && s.text == "texto"));
        assert!(d.blocks[1].spans.iter().any(|s| s.link.as_deref() == Some("https://x.org")));
        assert_eq!(d.blocks[1].line, 4);
        assert_eq!(texts(&d.blocks[2]), "uno");
        assert!(matches!(&d.blocks[4].kind, Kind::Item { depth: 1, mark } if mark == "1."));
        assert_eq!(texts(&d.blocks[5]), "  x\n  y");
        assert!(!d.blocks.iter().any(|b| texts(b).contains("alert")));
        let Kind::Table(rows) = &d.blocks[6].kind else { panic!("{:?}", d.blocks[6]) };
        assert_eq!(rows.len(), 2);
        assert!(rows[0].0);
        assert_eq!(rows[1].1[1][0].text, "2");
    }

    #[test]
    fn base_href() {
        let h = with_base("<html><HEAD><meta></head>", std::path::Path::new("/a b/c"));
        assert_eq!(h, "<html><HEAD><base href=\"file:///a%20b/c/\"><meta></head>");
        assert_eq!(decode_entities("a &lt;b&gt; &unknown; &#x41;"), "a <b> &unknown; A");
    }
}
