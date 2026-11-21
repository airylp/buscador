//! Ventana «Convertir»: Base64 y hexadecimal, en los dos sentidos.
//!
//! - Decodificar: Base64 (normal o URL, con o sin `=`) o hexadecimal (`48656c`,
//!   `48 65 6c`, `0x48,0x65`, `\x48`) → bytes, que se ven como texto (en el
//!   encoding elegido) o como volcado hexadecimal, con el tipo detectado
//!   (PNG, PDF, ZIP…). Se pueden copiar, abrir como archivo nuevo o guardar.
//! - Codificar: texto (en el encoding elegido) → Base64 o hexadecimal.
//!
//! Las entradas grandes (> 1 MB) se convierten en un hilo.

use eframe::egui::{self, Align, Context, Layout, ScrollArea, TextEdit, TextStyle, Ui};
use std::sync::{Arc, Mutex};

/// A partir de aquí se convierte en otro hilo y el campo no se edita a mano.
const BIG: usize = 1 << 20;
/// Máximo que se muestra en el campo de resultado (el resto se copia o guarda).
const SHOW_MAX: usize = 256 << 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Decode,
    Encode,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// Al decodificar: decide según el contenido.
    Auto,
    Base64,
    Base64Url,
    Hex,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HexSep {
    None,
    Space,
    /// `0x48, 0x65`
    C,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextEnc {
    Utf8,
    Utf16Le,
    Utf16Be,
    Latin1,
}

impl TextEnc {
    const ALL: [TextEnc; 4] = [TextEnc::Utf8, TextEnc::Utf16Le, TextEnc::Utf16Be, TextEnc::Latin1];
    fn label(self) -> &'static str {
        match self {
            TextEnc::Utf8 => "UTF-8",
            TextEnc::Utf16Le => "UTF-16 LE",
            TextEnc::Utf16Be => "UTF-16 BE",
            TextEnc::Latin1 => "Latin-1 / Windows-1252",
        }
    }

    pub fn encode(self, s: &str) -> Vec<u8> {
        match self {
            TextEnc::Utf8 => s.as_bytes().to_vec(),
            TextEnc::Utf16Le => s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect(),
            TextEnc::Utf16Be => s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect(),
            TextEnc::Latin1 => encoding_rs::WINDOWS_1252.encode(s).0.into_owned(),
        }
    }

    /// Texto y si hubo bytes no válidos.
    pub fn decode(self, b: &[u8]) -> (String, bool) {
        let enc = match self {
            TextEnc::Utf8 => encoding_rs::UTF_8,
            TextEnc::Utf16Le => encoding_rs::UTF_16LE,
            TextEnc::Utf16Be => encoding_rs::UTF_16BE,
            TextEnc::Latin1 => encoding_rs::WINDOWS_1252,
        };
        let (s, bad) = enc.decode_without_bom_handling(b);
        (s.into_owned(), bad)
    }
}

// ---- Base64 y hexadecimal (sin dependencias) ----

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn base64_encode(data: &[u8], url: bool, pad: bool, wrap: usize) -> String {
    let abc = if url { B64URL } else { B64 };
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4 + data.len() / 57 + 4);
    let mut col = 0;
    let mut push = |out: &mut String, c: u8| {
        if wrap > 0 && col == wrap {
            out.push('\n');
            col = 0;
        }
        out.push(c as char);
        col += 1;
    };
    for ch in data.chunks(3) {
        let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        let k = ch.len() + 1;
        for i in 0..4 {
            if i < k {
                push(&mut out, abc[(n >> (18 - 6 * i) & 63) as usize]);
            } else if pad {
                push(&mut out, b'=');
            }
        }
    }
    out
}

/// Acepta los dos alfabetos, espacios y saltos de línea, y relleno opcional.
pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut pad = 0;
    for (i, c) in s.bytes().enumerate() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                pad += 1;
                continue;
            }
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return Err(tf!("Carácter no válido en Base64 en la posición {pos}: «{c}»", pos = i + 1, c = s[i..].chars().next().unwrap_or('?'))),
        };
        if pad > 0 {
            return Err(t!("Hay texto después del relleno «=»").into());
        }
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if bits >= 6 {
        return Err(t!("Base64 incompleto: le sobra un carácter").into());
    }
    Ok(out)
}

pub fn hex_encode(data: &[u8], sep: HexSep, upper: bool) -> String {
    let digits: &[u8; 16] = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let per = match sep {
        HexSep::None => 2,
        HexSep::Space => 3,
        HexSep::C => 6,
    };
    let mut out = String::with_capacity(data.len() * per);
    for (i, &b) in data.iter().enumerate() {
        match sep {
            HexSep::None => {}
            HexSep::Space if i > 0 => out.push(' '),
            HexSep::C if i > 0 => out.push_str(", "),
            HexSep::C => {}
            HexSep::Space => {}
        }
        if sep == HexSep::C {
            out.push_str("0x");
        }
        out.push(digits[(b >> 4) as usize] as char);
        out.push(digits[(b & 15) as usize] as char);
    }
    out
}

/// Acepta `48656c`, `48 65 6c`, `0x48, 0x65`, `\x48\x65`, `48:65`.
pub fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut half: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // Prefijos 0x / \x: se saltan (solo al empezar un byte).
        if half.is_none() && (c == b'0' || c == b'\\') && matches!(b.get(i + 1), Some(b'x' | b'X')) {
            i += 2;
            continue;
        }
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            b' ' | b'\t' | b'\r' | b'\n' | b',' | b':' | b';' | b'-' => {
                if half.is_some() {
                    return Err(tf!("Hexadecimal: falta una cifra antes de la posición {pos}", pos = i + 1));
                }
                i += 1;
                continue;
            }
            _ => return Err(tf!("Carácter no válido en hexadecimal en la posición {pos}: «{c}»", pos = i + 1, c = s[i..].chars().next().unwrap_or('?'))),
        };
        match half.take() {
            Some(h) => out.push(h << 4 | v),
            None => half = Some(v),
        }
        i += 1;
    }
    if half.is_some() {
        return Err(t!("Hexadecimal: número impar de cifras").into());
    }
    Ok(out)
}

/// ¿Parece hexadecimal? (solo cifras hex y separadores, y alguna cifra).
fn looks_hex(s: &str) -> bool {
    let t = s.trim();
    let digits = t.bytes().filter(|c| c.is_ascii_hexdigit()).count();
    digits >= 2
        && (t.contains("0x") || t.contains("\\x") || t.bytes().all(|c| c.is_ascii_hexdigit() || b" \t\r\n,:;-".contains(&c)))
        && hex_decode(t).is_ok()
}

/// Formato probable de un texto a decodificar.
pub fn detect_format(s: &str) -> Format {
    if looks_hex(s) {
        Format::Hex
    } else if s.contains(['-', '_']) && !s.contains(['+', '/']) {
        Format::Base64Url
    } else {
        Format::Base64
    }
}

/// Tipo de contenido por sus primeros bytes.
pub fn kind(b: &[u8]) -> &'static str {
    let starts = |p: &[u8]| b.starts_with(p);
    if starts(b"\x89PNG\r\n\x1a\n") {
        "PNG"
    } else if starts(&[0xff, 0xd8, 0xff]) {
        "JPEG"
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        "GIF"
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "WebP"
    } else if starts(b"%PDF-") {
        "PDF"
    } else if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        t!("ZIP (o docx/xlsx/jar)")
    } else if starts(&[0x1f, 0x8b]) {
        "gzip"
    } else if starts(&[0x28, 0xb5, 0x2f, 0xfd]) {
        "zstd"
    } else if starts(b"7z\xbc\xaf\x27\x1c") {
        "7z"
    } else if starts(b"Rar!") {
        "RAR"
    } else if starts(b"\x7fELF") {
        t!("ELF (ejecutable)")
    } else if starts(b"MZ") {
        t!("EXE/DLL de Windows")
    } else if starts(&[0xca, 0xfe, 0xba, 0xbe]) {
        t!("class de Java / Mach-O")
    } else if starts(b"SQLite format 3\0") {
        "SQLite"
    } else if b.is_empty() {
        t!("vacío")
    } else if let Ok(s) = std::str::from_utf8(b) {
        let t = s.trim_start();
        if (t.starts_with('{') || t.starts_with('[')) && t.len() < (8 << 20) && crate::beautify::format_json(t, "").is_ok() {
            "JSON"
        } else if t.starts_with("<?xml") || t.starts_with('<') && t.trim_end().ends_with('>') {
            "XML/HTML"
        } else {
            t!("texto UTF-8")
        }
    } else if b.len() >= 2 && (starts(&[0xff, 0xfe]) || starts(&[0xfe, 0xff])) {
        t!("texto UTF-16")
    } else {
        t!("binario")
    }
}

/// Una fila de volcado: `00000010  48 65 6c 6c 6f …  |Hello…|`.
pub fn dump_row(offset: u64, row: &[u8], wide: bool) -> String {
    let mut s = if wide { format!("{offset:012x}  ") } else { format!("{offset:08x}  ") };
    for i in 0..16 {
        match row.get(i) {
            Some(b) => s.push_str(&format!("{b:02x} ")),
            None => s.push_str("   "),
        }
        if i == 7 {
            s.push(' ');
        }
    }
    s.push(' ');
    s.push('|');
    s.extend(row.iter().map(|&b| printable(b)));
    s.push('|');
    s
}

pub fn printable(b: u8) -> char {
    if (0x20..0x7f).contains(&b) { b as char } else { '.' }
}

pub fn dump(data: &[u8]) -> String {
    let wide = data.len() as u64 > u32::MAX as u64;
    data.chunks(16).enumerate().map(|(i, r)| dump_row(i as u64 * 16, r, wide)).collect::<Vec<_>>().join("\n")
}

// ---- Ventana ----

/// Resultado de una conversión.
#[derive(Default)]
struct Output {
    /// Decodificar: los bytes. Codificar: el texto (en UTF-8).
    bytes: Vec<u8>,
    error: Option<String>,
    /// Formato usado al decodificar (el detectado si era «Auto»).
    used: Option<Format>,
    /// Tipo de lo decodificado (se calcula una vez, en `convert`).
    kind: &'static str,
    /// Es texto (UTF-8 válido o con BOM de UTF-16): se ve como texto, si no en hex.
    text_like: bool,
}

#[derive(Clone, PartialEq)]
struct Key {
    version: u64,
    dir: Dir,
    format: Format,
    pad: bool,
    wrap: bool,
    sep: HexSep,
    upper: bool,
    enc: TextEnc,
}

pub struct Convert {
    pub open: bool,
    pub input: String,
    /// Cambia con cada modificación de `input` (para no reconvertir).
    version: u64,
    pub dir: Dir,
    pub format: Format,
    pad: bool,
    wrap: bool,
    sep: HexSep,
    upper: bool,
    /// Encoding del texto: el de entrada al codificar, el del resultado al decodificar.
    enc: TextEnc,
    /// Ver el resultado decodificado como texto (si no, volcado hexadecimal).
    as_text: bool,
    done: Option<(Key, Output)>,
    pending: Option<(Key, Arc<Mutex<Option<Output>>>)>,
    /// Texto de lo mostrado (cacheado por clave).
    shown: Option<(Key, bool, String, bool)>,
}

impl Default for Convert {
    fn default() -> Self {
        Convert {
            open: false,
            input: String::new(),
            version: 0,
            dir: Dir::Decode,
            format: Format::Auto,
            pad: true,
            wrap: false,
            sep: HexSep::None,
            upper: false,
            enc: TextEnc::Utf8,
            as_text: true,
            done: None,
            pending: None,
            shown: None,
        }
    }
}

/// Lo que la ventana le pide a la app.
#[derive(Default)]
pub struct Out {
    pub use_selection: bool,
    pub copy: Option<String>,
    pub new_file: Option<String>,
    pub replace_selection: Option<String>,
    pub save_bytes: bool,
}

pub fn window_id() -> egui::Id {
    egui::Id::NULL.with("ventana_convertir")
}

impl Convert {
    /// Abre la ventana con este texto (la selección) y decide el sentido.
    pub fn show_with(&mut self, text: Option<String>) {
        self.open = true;
        if let Some(t) = text.filter(|t| !t.is_empty()) {
            let t = t.trim().to_string();
            // Si parece Base64/hex, decodificar; si no, codificar.
            let decodable = looks_hex(&t) || (t.len() >= 4 && base64_decode(&t).is_ok() && !t.contains(' '));
            self.dir = if decodable { Dir::Decode } else { Dir::Encode };
            self.set_input(t);
        }
    }

    pub fn set_input(&mut self, s: String) {
        self.input = s;
        self.version += 1;
    }

    /// Bytes decodificados (para «Guardar bytes…»).
    pub fn decoded(&self) -> Option<&[u8]> {
        self.done.as_ref().filter(|(k, o)| k.dir == Dir::Decode && o.error.is_none()).map(|(_, o)| &o.bytes[..])
    }

    fn key(&self) -> Key {
        Key { version: self.version, dir: self.dir, format: self.format, pad: self.pad, wrap: self.wrap, sep: self.sep, upper: self.upper, enc: self.enc }
    }

    /// Guarda un resultado; con una entrada nueva elige ver texto o hex.
    fn store(&mut self, k: Key, o: Output) {
        let new_input = self.done.as_ref().is_none_or(|(d, _)| d.version != k.version || d.dir != k.dir);
        if new_input && k.dir == Dir::Decode && o.error.is_none() {
            self.as_text = o.text_like;
        }
        self.done = Some((k, o));
        self.shown = None;
    }

    /// Lanza o recoge la conversión de lo que hay ahora.
    fn update(&mut self, ctx: &Context) {
        if let Some((k, slot)) = &self.pending {
            let r = slot.lock().unwrap().take();
            if let Some(o) = r {
                let k = k.clone();
                self.pending = None;
                self.store(k, o);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
        }
        let key = self.key();
        if self.done.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let input = self.input.clone();
        let k2 = key.clone();
        if input.len() > BIG {
            let slot = Arc::new(Mutex::new(None));
            let (s2, ctx2) = (slot.clone(), ctx.clone());
            std::thread::spawn(move || {
                *s2.lock().unwrap() = Some(convert(&input, &k2));
                ctx2.request_repaint();
            });
            self.pending = Some((key, slot));
        } else {
            let o = convert(&input, &k2);
            self.store(key, o);
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, has_selection: bool, can_replace: bool) -> Out {
        let mut out = Out::default();
        self.update(ui.ctx());
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.dir, Dir::Decode, t!("Decodificar"));
            ui.selectable_value(&mut self.dir, Dir::Encode, t!("Codificar"));
            ui.separator();
            let fmts: &[(Format, &str)] = if self.dir == Dir::Decode {
                &[(Format::Auto, t!("Detectar")), (Format::Base64, "Base64"), (Format::Base64Url, "Base64 URL"), (Format::Hex, t!("Hexadecimal"))]
            } else {
                &[(Format::Base64, "Base64"), (Format::Base64Url, "Base64 URL"), (Format::Hex, t!("Hexadecimal"))]
            };
            if self.dir == Dir::Encode && self.format == Format::Auto {
                self.format = Format::Base64;
            }
            for (f, l) in fmts {
                ui.selectable_value(&mut self.format, *f, *l);
            }
        });
        ui.horizontal(|ui| {
            if self.dir == Dir::Encode {
                match self.format {
                    Format::Hex => {
                        ui.label(t!("Separador:"));
                        ui.selectable_value(&mut self.sep, HexSep::None, t!("ninguno"));
                        ui.selectable_value(&mut self.sep, HexSep::Space, t!("espacio"));
                        ui.selectable_value(&mut self.sep, HexSep::C, "0x…, ");
                        ui.checkbox(&mut self.upper, t!("Mayúsculas"));
                    }
                    _ => {
                        ui.checkbox(&mut self.pad, t!("Relleno «=»"));
                        ui.checkbox(&mut self.wrap, t!("Líneas de 76"));
                    }
                }
                ui.separator();
            }
            ui.label(if self.dir == Dir::Encode { t!("Texto en:") } else { t!("Ver el texto en:") });
            egui::ComboBox::from_id_salt("conv_enc").selected_text(self.enc.label()).show_ui(ui, |ui| {
                for e in TextEnc::ALL {
                    ui.selectable_value(&mut self.enc, e, e.label());
                }
            });
        });
        ui.add_space(4.0);

        // Entrada.
        ui.horizontal(|ui| {
            ui.strong(if self.dir == Dir::Decode { t!("Base64 o hexadecimal") } else { t!("Texto") });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button(t!("Vaciar")).clicked() {
                    self.set_input(String::new());
                }
                if ui.add_enabled(has_selection, egui::Button::new(t!("Usar la selección")).small()).on_hover_text(t!("El texto seleccionado en el archivo activo")).clicked() {
                    out.use_selection = true;
                }
            });
        });
        let half = (ui.available_height() - 90.0).max(120.0) * 0.4;
        if self.input.len() > BIG {
            ui.weak(tf!("{n} de texto (demasiado para mostrarlo; «Vaciar» para escribir otro)", n = human(self.input.len())));
        } else {
            let before = self.input.len();
            let r = ScrollArea::vertical().id_salt("conv_in").max_height(half).show(ui, |ui| {
                ui.add(
                    TextEdit::multiline(&mut self.input)
                        .font(TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(4)
                        .hint_text(if self.dir == Dir::Decode { "SGVsbG8= · 48 65 6c 6c 6f" } else { t!("Escribe o pega el texto") }),
                )
            });
            if r.inner.changed() || self.input.len() != before {
                self.version += 1;
            }
        }
        ui.add_space(6.0);

        // Resultado.
        let Some((key, o)) = &self.done else {
            ui.spinner();
            return out;
        };
        if self.pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(t!("Convirtiendo…"));
            });
        }
        if let Some(e) = &o.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return out;
        }
        let decode = key.dir == Dir::Decode;
        ui.horizontal(|ui| {
            ui.strong(t!("Resultado"));
            if decode {
                let used = match o.used {
                    Some(Format::Hex) => t!("hexadecimal"),
                    Some(Format::Base64Url) => "Base64 URL",
                    _ => "Base64",
                };
                ui.weak(tf!("{n} · desde {used} · tipo: {kind}", n = human(o.bytes.len()), used = used, kind = o.kind));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.selectable_value(&mut self.as_text, false, t!("Hex"));
                    ui.selectable_value(&mut self.as_text, true, t!("Texto"));
                    ui.label(t!("Ver como:"));
                });
            } else {
                ui.weak(tf!("{n} caracteres", n = o.bytes.len()));
            }
        });
        // Texto mostrado (cacheado: el volcado de MB no se rehace cada fotograma).
        let as_text = !decode || self.as_text;
        if !self.shown.as_ref().is_some_and(|(k, t, _, _)| k == key && *t == as_text) {
            let (s, bad) = if !decode {
                (String::from_utf8_lossy(&o.bytes).into_owned(), false)
            } else if as_text {
                key.enc.decode(&o.bytes)
            } else {
                (dump(&o.bytes[..o.bytes.len().min(SHOW_MAX / 4)]), false)
            };
            self.shown = Some((key.clone(), as_text, s, bad));
        }
        let (_, _, shown, bad) = self.shown.as_ref().unwrap();
        if *bad {
            ui.colored_label(ui.visuals().warn_fg_color, tf!("No es texto {enc} válido: los bytes que no encajan se ven como �. Prueba «Ver como: Hex» u otro encoding.", enc = key.enc.label()));
        }
        let cut = shown.len() > SHOW_MAX || (decode && !as_text && o.bytes.len() > SHOW_MAX / 4);
        let mut view: &str = &shown[..crate::buffer::floor_char(shown, SHOW_MAX)];
        ScrollArea::both().id_salt("conv_out").max_height((ui.available_height() - 40.0).max(80.0)).show(ui, |ui| {
            ui.add(TextEdit::multiline(&mut view).font(TextStyle::Monospace).desired_width(f32::INFINITY).code_editor());
        });
        if cut {
            ui.weak(t!("Se muestra solo el principio; «Copiar», «Archivo nuevo» y «Guardar» usan todo."));
        }
        ui.horizontal(|ui| {
            let full_text = || -> String {
                if decode && !as_text { dump(&o.bytes) } else { shown.clone() }
            };
            if ui.button(t!("Copiar")).clicked() {
                out.copy = Some(full_text());
            }
            if ui.button(t!("Abrir como archivo nuevo")).clicked() {
                out.new_file = Some(full_text());
            }
            if ui.add_enabled(can_replace, egui::Button::new(t!("Reemplazar la selección"))).on_hover_text(t!("Pone el resultado en lugar del texto seleccionado del archivo activo")).clicked() {
                out.replace_selection = Some(full_text());
            }
            if decode && ui.button(t!("Guardar los bytes…")).on_hover_text(t!("Guarda los bytes decodificados tal cual en un archivo (p. ej. una imagen o un PDF)")).clicked() {
                out.save_bytes = true;
            }
        });
        out
    }
}

fn convert(input: &str, k: &Key) -> Output {
    match k.dir {
        Dir::Decode => {
            let f = if k.format == Format::Auto { detect_format(input) } else { k.format };
            let r = match f {
                Format::Hex => hex_decode(input),
                _ => base64_decode(input),
            };
            match r {
                Ok(bytes) => Output {
                    kind: kind(&bytes),
                    text_like: std::str::from_utf8(&bytes).is_ok() || bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]),
                    bytes,
                    error: None,
                    used: Some(f),
                },
                Err(e) => Output { error: Some(e), ..Default::default() },
            }
        }
        Dir::Encode => {
            let data = k.enc.encode(input);
            let s = match k.format {
                Format::Hex => hex_encode(&data, k.sep, k.upper),
                f => base64_encode(&data, f == Format::Base64Url, k.pad, if k.wrap { 76 } else { 0 }),
            };
            Output { bytes: s.into_bytes(), ..Default::default() }
        }
    }
}

pub fn human(n: usize) -> String {
    match n {
        n if n < 1024 => tf!("{n} bytes", n = n),
        n if n < 1 << 20 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip_and_variants() {
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar", "ñandú €"] {
            let e = base64_encode(s.as_bytes(), false, true, 0);
            assert_eq!(base64_decode(&e).unwrap(), s.as_bytes());
            let u = base64_encode(s.as_bytes(), true, false, 0);
            assert_eq!(base64_decode(&u).unwrap(), s.as_bytes());
        }
        assert_eq!(base64_encode(b"foobar", false, true, 0), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo", false, true, 0), "Zm8=");
        assert_eq!(base64_encode(&[0xfb, 0xff], true, false, 0), "-_8");
        assert_eq!(base64_decode("Zm9v\nYmFy\n").unwrap(), b"foobar");
        assert!(base64_decode("Zm9v!").is_err());
        assert!(base64_decode("Z").is_err());
        let long = base64_encode(&[7u8; 100], false, true, 76);
        assert!(long.lines().all(|l| l.len() <= 76) && long.contains('\n'));
    }

    #[test]
    fn hex_forms() {
        for s in ["48656c6c6f", "48 65 6C 6c 6f", "0x48, 0x65, 0x6c, 0x6c, 0x6f", "\\x48\\x65\\x6c\\x6c\\x6f", "48:65:6c:6c:6f"] {
            assert_eq!(hex_decode(s).unwrap(), b"Hello", "{s}");
        }
        assert!(hex_decode("486").is_err());
        assert!(hex_decode("4 8").is_err());
        assert!(hex_decode("zz").is_err());
        assert_eq!(hex_encode(b"Hi", HexSep::C, true), "0x48, 0x69");
        assert_eq!(hex_encode(b"Hi", HexSep::Space, false), "48 69");
    }

    #[test]
    fn detects_format_and_kind() {
        assert_eq!(detect_format("48 65 6c"), Format::Hex);
        assert_eq!(detect_format("SGVsbG8="), Format::Base64);
        assert_eq!(detect_format("-_8"), Format::Base64Url);
        assert_eq!(kind(b"%PDF-1.7 ..."), "PDF");
        assert_eq!(kind(&[0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0]), "PNG");
        assert_eq!(kind(b"{\"a\": 1}"), "JSON");
        assert_eq!(kind(b"hola"), "texto UTF-8");
        assert_eq!(kind(&[0, 159, 146, 150]), "binario");
        assert_eq!(dump_row(16, b"Hello", false), "00000010  48 65 6c 6c 6f                                    |Hello|");
    }
}
