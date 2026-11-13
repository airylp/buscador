//! Embellecedor: valida y vuelve a sangrar JSON y XML/HTML.
//!
//! No construye un árbol: recorre el texto una vez, comprueba que esté bien
//! formado y escribe el resultado con sangría. Cadenas, números, atributos y
//! comentarios se copian tal cual; solo cambian espacios y saltos de línea.

/// Error con la posición (byte) donde se detectó.
#[derive(Debug, PartialEq)]
pub struct FormatError {
    pub pos: usize,
    pub msg: String,
}

fn err<T>(pos: usize, msg: impl Into<String>) -> Result<T, FormatError> {
    Err(FormatError { pos, msg: msg.into() })
}

/// (línea, columna) 1-based de un byte del texto.
pub fn line_col(s: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(s.len());
    let before = &s[..pos];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

// ------------------------------------------------------------------ JSON

struct Json<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    out: String,
    indent: &'a str,
}

impl Json<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn newline(&mut self, depth: usize) {
        self.out.push('\n');
        for _ in 0..depth {
            self.out.push_str(self.indent);
        }
    }

    fn value(&mut self, depth: usize) -> Result<(), FormatError> {
        self.ws();
        let Some(&c) = self.b.get(self.i) else { return err(self.i, t!("falta un valor")) };
        match c {
            b'{' => self.container(depth, b'}', true),
            b'[' => self.container(depth, b']', false),
            b'"' => self.string(),
            b'-' | b'0'..=b'9' => self.number(),
            b't' => self.literal("true"),
            b'f' => self.literal("false"),
            b'n' => self.literal("null"),
            _ => err(self.i, tf!("valor inesperado «{i}»", i = char_at(self.s, self.i))),
        }
    }

    fn container(&mut self, depth: usize, close: u8, object: bool) -> Result<(), FormatError> {
        self.out.push(self.b[self.i] as char);
        self.i += 1;
        self.ws();
        if self.b.get(self.i) == Some(&close) {
            self.out.push(close as char);
            self.i += 1;
            return Ok(());
        }
        loop {
            self.newline(depth + 1);
            self.ws();
            if object {
                if self.b.get(self.i) != Some(&b'"') {
                    return err(self.i, t!("se esperaba una clave entre comillas"));
                }
                self.string()?;
                self.ws();
                if self.b.get(self.i) != Some(&b':') {
                    return err(self.i, t!("se esperaba «:» tras la clave"));
                }
                self.i += 1;
                self.out.push_str(": ");
            }
            self.value(depth + 1)?;
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => {
                    self.i += 1;
                    self.out.push(',');
                }
                Some(&c) if c == close => {
                    self.i += 1;
                    self.newline(depth);
                    self.out.push(close as char);
                    return Ok(());
                }
                Some(_) => return err(self.i, tf!("se esperaba «,» o «{close_as_char}»", close_as_char = close as char)),
                None => return err(self.i, tf!("falta cerrar con «{close_as_char}»", close_as_char = close as char)),
            }
        }
    }

    fn string(&mut self) -> Result<(), FormatError> {
        let start = self.i;
        let mut j = self.i + 1;
        while j < self.b.len() {
            match self.b[j] {
                b'"' => {
                    self.out.push_str(&self.s[start..=j]);
                    self.i = j + 1;
                    return Ok(());
                }
                b'\\' => {
                    match self.b.get(j + 1) {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => j += 2,
                        Some(b'u') if self.b.get(j + 2..j + 6).is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit)) => j += 6,
                        _ => return err(j, t!("secuencia de escape no válida")),
                    }
                    continue;
                }
                c if c < 0x20 => return err(j, t!("salto de línea o carácter de control dentro de una cadena")),
                _ => j += 1,
            }
        }
        err(start, t!("cadena sin cerrar"))
    }

    fn number(&mut self) -> Result<(), FormatError> {
        let b = self.b;
        let start = self.i;
        let mut j = start;
        if b.get(j) == Some(&b'-') {
            j += 1;
        }
        let digits = |j: &mut usize| {
            let s = *j;
            while *j < b.len() && b[*j].is_ascii_digit() {
                *j += 1;
            }
            *j > s
        };
        if b.get(j) == Some(&b'0') {
            j += 1;
        } else if !digits(&mut j) {
            return err(start, t!("número no válido"));
        }
        if b.get(j) == Some(&b'.') {
            j += 1;
            if !digits(&mut j) {
                return err(j, t!("faltan decimales"));
            }
        }
        if matches!(b.get(j), Some(b'e' | b'E')) {
            j += 1;
            if matches!(b.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if !digits(&mut j) {
                return err(j, t!("exponente no válido"));
            }
        }
        if b.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'.') {
            return err(j, t!("número no válido"));
        }
        self.out.push_str(&self.s[start..j]);
        self.i = j;
        Ok(())
    }

    fn literal(&mut self, w: &str) -> Result<(), FormatError> {
        if self.s[self.i..].starts_with(w) && !self.b.get(self.i + w.len()).is_some_and(|c| c.is_ascii_alphanumeric()) {
            self.out.push_str(w);
            self.i += w.len();
            Ok(())
        } else {
            err(self.i, tf!("valor inesperado «{i}»", i = char_at(self.s, self.i)))
        }
    }
}

fn char_at(s: &str, i: usize) -> char {
    s.get(i..).and_then(|r| r.chars().next()).unwrap_or(' ')
}

/// Embellece JSON (o varios valores seguidos, como JSON Lines: uno por bloque).
pub fn format_json(s: &str, indent: &str) -> Result<String, FormatError> {
    let mut p = Json { s, b: s.as_bytes(), i: 0, out: String::with_capacity(s.len() + s.len() / 4), indent };
    p.ws();
    if p.i >= p.b.len() {
        return err(0, t!("el texto está vacío"));
    }
    loop {
        p.value(0)?;
        p.ws();
        // Admite comas entre valores de primer nivel? No: JSON estricto, salvo
        // varios valores separados por espacios/saltos (JSON Lines).
        if p.i >= p.b.len() {
            break;
        }
        p.out.push('\n');
    }
    Ok(p.out)
}

// ------------------------------------------------------------------ XML / HTML

#[derive(Debug)]
enum Tok<'a> {
    /// `<?…?>`, `<!-- -->`, `<![CDATA[…]]>`, `<!DOCTYPE …>`: se copian tal cual.
    Raw(&'a str),
    Start { name: &'a str, raw: &'a str, empty: bool },
    End { name: &'a str, pos: usize },
    Text(&'a str),
}

/// Elementos vacíos de HTML (no llevan cierre).
const HTML_VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr",
];
/// Elementos de HTML cuyo contenido es texto crudo (puede tener «<»).
const HTML_RAW: &[&str] = &["script", "style"];

fn is_name(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'-' | b'.') || c >= 0x80
}

fn tokenize(s: &str, html: bool) -> Result<Vec<(usize, Tok<'_>)>, FormatError> {
    let b = s.as_bytes();
    let n = b.len();
    let mut toks = Vec::new();
    let mut i = 0;
    let find = |from: usize, pat: &str, what: &str| -> Result<usize, FormatError> {
        s[from..].find(pat).map(|e| from + e + pat.len()).ok_or(FormatError { pos: from, msg: tf!("{what} sin cerrar", what = what) })
    };
    while i < n {
        if b[i] != b'<' {
            let end = s[i..].find('<').map_or(n, |e| i + e);
            toks.push((i, Tok::Text(&s[i..end])));
            i = end;
            continue;
        }
        let rest = &s[i..];
        if rest.starts_with("<!--") {
            let end = find(i + 4, "-->", "comentario")?;
            toks.push((i, Tok::Raw(&s[i..end])));
            i = end;
        } else if rest.starts_with("<![CDATA[") {
            let end = find(i + 9, "]]>", "CDATA")?;
            toks.push((i, Tok::Raw(&s[i..end])));
            i = end;
        } else if rest.starts_with("<?") {
            let end = find(i + 2, "?>", t!("instrucción «<?»"))?;
            toks.push((i, Tok::Raw(&s[i..end])));
            i = end;
        } else if rest.starts_with("<!") {
            // DOCTYPE (puede tener un subconjunto interno entre corchetes).
            let mut j = i + 2;
            let mut depth = 0;
            while j < n {
                match b[j] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    b'>' if depth <= 0 => break,
                    _ => {}
                }
                j += 1;
            }
            if j >= n {
                return err(i, t!("declaración «<!» sin cerrar"));
            }
            toks.push((i, Tok::Raw(&s[i..=j])));
            i = j + 1;
        } else if rest.starts_with("</") {
            let mut j = i + 2;
            while j < n && is_name(b[j]) {
                j += 1;
            }
            let name = &s[i + 2..j];
            while j < n && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if name.is_empty() || b.get(j) != Some(&b'>') {
                return err(i, t!("etiqueta de cierre mal formada"));
            }
            toks.push((i, Tok::End { name, pos: i }));
            i = j + 1;
        } else {
            let mut j = i + 1;
            while j < n && is_name(b[j]) {
                j += 1;
            }
            let name = &s[i + 1..j];
            if name.is_empty() {
                return err(i, t!("«<» suelto (en texto se escribe &lt;)"));
            }
            // Atributos: nombre = "valor" | 'valor' (en HTML también sin valor o sin comillas).
            loop {
                while j < n && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                match b.get(j) {
                    None => return err(i, tf!("etiqueta <{name}> sin cerrar", name = name)),
                    Some(b'>') => {
                        j += 1;
                        break;
                    }
                    Some(b'/') if b.get(j + 1) == Some(&b'>') => {
                        j += 2;
                        break;
                    }
                    Some(&c) if is_name(c) => {
                        let a0 = j;
                        while j < n && is_name(b[j]) {
                            j += 1;
                        }
                        while j < n && b[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        if b.get(j) == Some(&b'=') {
                            j += 1;
                            while j < n && b[j].is_ascii_whitespace() {
                                j += 1;
                            }
                            match b.get(j) {
                                Some(&q @ (b'"' | b'\'')) => match s[j + 1..].find(q as char) {
                                    Some(e) => j = j + 1 + e + 1,
                                    None => return err(j, tf!("valor del atributo «{j}» sin cerrar", j = &s[a0..j])),
                                },
                                Some(_) if html => {
                                    while j < n && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                                        j += 1;
                                    }
                                }
                                _ => return err(j, tf!("el atributo «{trim_end_matches}» necesita el valor entre comillas", trim_end_matches = s[a0..j].trim_end_matches(['=', ' ']))),
                            }
                        } else if !html {
                            return err(j, tf!("el atributo «{trim_end}» necesita un valor", trim_end = &s[a0..j].trim_end()));
                        }
                    }
                    Some(_) => return err(j, tf!("carácter inesperado en la etiqueta <{name}>", name = name)),
                }
            }
            let raw = &s[i..j];
            let lower = name.to_ascii_lowercase();
            let empty = raw.ends_with("/>") || (html && HTML_VOID.contains(&lower.as_str()));
            toks.push((i, Tok::Start { name, raw, empty }));
            i = j;
            // <script>/<style>: el contenido va tal cual hasta su cierre.
            if html && !empty && HTML_RAW.contains(&lower.as_str()) {
                let close = format!("</{lower}");
                let end = s[i..].to_ascii_lowercase().find(&close).map(|e| i + e).ok_or(FormatError {
                    pos: i,
                    msg: tf!("<{name}> sin cerrar", name = name),
                })?;
                if end > i {
                    toks.push((i, Tok::Raw(&s[i..end])));
                }
                i = end;
            }
        }
    }
    Ok(toks)
}

/// Embellece XML (o HTML, más permisivo). Comprueba que las etiquetas cierren
/// en orden y que haya un único elemento raíz (en XML).
pub fn format_xml(s: &str, indent: &str, html: bool) -> Result<String, FormatError> {
    let toks = tokenize(s, html)?;
    let mut out = String::with_capacity(s.len() + s.len() / 4);
    let mut stack: Vec<(&str, usize)> = Vec::new();
    let mut roots = 0;
    let line = |out: &mut String, depth: usize| {
        if !out.is_empty() {
            out.push('\n');
        }
        for _ in 0..depth {
            out.push_str(indent);
        }
    };
    let same = |a: &str, b: &str| if html { a.eq_ignore_ascii_case(b) } else { a == b };
    let mut k = 0;
    while k < toks.len() {
        let (pos, ref t) = toks[k];
        match t {
            Tok::Raw(r) => {
                line(&mut out, stack.len());
                out.push_str(r.trim());
            }
            Tok::Text(txt) => {
                let tt = txt.trim();
                if !tt.is_empty() {
                    if stack.is_empty() && !html {
                        return err(pos, t!("texto fuera del elemento raíz"));
                    }
                    line(&mut out, stack.len());
                    out.push_str(tt);
                }
            }
            Tok::Start { name, raw, empty } => {
                if stack.is_empty() {
                    roots += 1;
                    if roots > 1 && !html {
                        return err(pos, tf!("hay más de un elemento raíz (<{name}>)", name = name));
                    }
                }
                line(&mut out, stack.len());
                out.push_str(raw.trim());
                if !*empty {
                    // <a>texto corto</a> en una sola línea.
                    if let (Some((_, Tok::Text(txt))), Some((_, Tok::End { name: en, .. }))) = (toks.get(k + 1), toks.get(k + 2)) {
                        if same(en, name) && !txt.trim().contains('\n') {
                            out.push_str(txt.trim());
                            out.push_str(&format!("</{en}>"));
                            k += 3;
                            continue;
                        }
                    }
                    if let Some((_, Tok::End { name: en, .. })) = toks.get(k + 1) {
                        if same(en, name) {
                            out.push_str(&format!("</{en}>"));
                            k += 2;
                            continue;
                        }
                    }
                    stack.push((name, pos));
                }
            }
            Tok::End { name, pos } => match stack.pop() {
                Some((open, _)) if same(open, name) => {
                    line(&mut out, stack.len());
                    out.push_str(&format!("</{name}>"));
                }
                Some((open, opos)) => {
                    let (l, _) = line_col(s, opos);
                    return err(*pos, tf!("se esperaba </{open}> (abierto en la línea {l}) y llegó </{name}>", open = open, l = l, name = name));
                }
                None => return err(*pos, tf!("</{name}> sin etiqueta de apertura", name = name)),
            },
        }
        k += 1;
    }
    if let Some((open, opos)) = stack.pop() {
        return err(opos, tf!("<{open}> no se cierra", open = open));
    }
    if roots == 0 && !html {
        return err(0, t!("no hay ningún elemento"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_pretty_keeps_values() {
        let s = r#"{"a":1,"b":[true,null,{"c":"x\"y","d":-1.5e3}],"e":{},"f":[]}"#;
        let out = format_json(s, "  ").unwrap();
        assert_eq!(
            out,
            "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null,\n    {\n      \"c\": \"x\\\"y\",\n      \"d\": -1.5e3\n    }\n  ],\n  \"e\": {},\n  \"f\": []\n}"
        );
        // Embellecer dos veces da lo mismo.
        assert_eq!(format_json(&out, "  ").unwrap(), out);
        // JSON Lines: un valor por bloque.
        assert_eq!(format_json("{\"a\":1}\n{\"a\":2}", "  ").unwrap(), "{\n  \"a\": 1\n}\n{\n  \"a\": 2\n}");
    }

    #[test]
    fn json_errors_with_position() {
        let e = format_json("{\n  \"a\": 1,\n  \"b\" 2\n}", "  ").unwrap_err();
        assert_eq!(line_col("{\n  \"a\": 1,\n  \"b\" 2\n}", e.pos), (3, 7));
        assert!(e.msg.contains("«:»"));
        assert!(format_json("[1, 2,]", "  ").is_err());
        assert!(format_json("{'a': 1}", "  ").is_err());
        assert!(format_json("[01]", "  ").is_err());
        assert!(format_json("\"abc", "  ").is_err());
    }

    #[test]
    fn xml_pretty_and_checks() {
        let s = r#"<?xml version="1.0"?><!-- c --><raiz a="1"><b>texto</b><c/><d><e x='2'>uno</e></d><f></f></raiz>"#;
        let out = format_xml(s, "  ", false).unwrap();
        assert_eq!(
            out,
            "<?xml version=\"1.0\"?>\n<!-- c -->\n<raiz a=\"1\">\n  <b>texto</b>\n  <c/>\n  <d>\n    <e x='2'>uno</e>\n  </d>\n  <f></f>\n</raiz>"
        );
        assert_eq!(format_xml(&out, "  ", false).unwrap(), out);
        let e = format_xml("<a>\n  <b>\n</a>", "  ", false).unwrap_err();
        assert!(e.msg.contains("</b>"), "{}", e.msg);
        assert!(format_xml("<a x=1/>", "  ", false).is_err());
        assert!(format_xml("<a/><b/>", "  ", false).is_err());
        assert!(format_xml("<a>", "  ", false).is_err());
    }

    #[test]
    fn html_is_more_lenient() {
        let s = "<html><head><meta charset=utf-8><script>if (a < b) {}</script></head><body><p>hola<br>mundo</p></body></html>";
        let out = format_xml(s, "  ", true).unwrap();
        assert!(out.contains("<meta charset=utf-8>"));
        assert!(out.contains("if (a < b) {}"));
        assert!(out.contains("    <br>"));
    }
}
