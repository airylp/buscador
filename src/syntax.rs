//! Resaltado de sintaxis ligero, según la extensión del archivo, y de los
//! niveles (ERROR, WARN…), fechas y excepciones en los logs.
//!
//! Solo se colorean las líneas visibles. Lo que ocupa varias líneas
//! (comentarios `/* */`, `<!-- -->`, cadenas `"""` de Python) se arrastra con
//! un [`State`] que se calcula desde unas pocas líneas antes de lo visible: en
//! archivos enormes no se recorre desde el principio.

use eframe::egui::Color32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Keyword,
    String,
    Comment,
    Number,
    Tag,
    Attr,
    Error,
    Warn,
    Info,
    Debug,
    Date,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// Lo que queda abierto al final de una línea.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum State {
    #[default]
    Normal,
    /// Dentro de un comentario de bloque (`/* … */`).
    Block,
    /// Dentro de una cadena triple de Python (`"""` o `'''`).
    Triple(u8),
    /// Dentro de un comentario XML (`<!-- … -->`).
    XmlComment,
    /// Dentro de un bloque de código de Markdown (``` … ```).
    Fence,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Plain,
    Log,
    Java,
    Kotlin,
    JavaScript,
    TypeScript,
    Rust,
    Go,
    C,
    CSharp,
    Swift,
    Python,
    Shell,
    Sql,
    Json,
    Xml,
    Yaml,
    Toml,
    Css,
    Markdown,
}

impl Lang {
    pub const ALL: [Lang; 20] = [
        Lang::Log,
        Lang::Plain,
        Lang::Java,
        Lang::Kotlin,
        Lang::JavaScript,
        Lang::TypeScript,
        Lang::Rust,
        Lang::Go,
        Lang::C,
        Lang::CSharp,
        Lang::Swift,
        Lang::Python,
        Lang::Shell,
        Lang::Sql,
        Lang::Json,
        Lang::Xml,
        Lang::Yaml,
        Lang::Toml,
        Lang::Css,
        Lang::Markdown,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Lang::Plain => t!("Texto sin formato"),
            Lang::Log => "Log",
            Lang::Java => "Java",
            Lang::Kotlin => "Kotlin",
            Lang::JavaScript => "JavaScript",
            Lang::TypeScript => "TypeScript",
            Lang::Rust => "Rust",
            Lang::Go => "Go",
            Lang::C => "C / C++",
            Lang::CSharp => "C#",
            Lang::Swift => "Swift",
            Lang::Python => "Python",
            Lang::Shell => "Shell",
            Lang::Sql => "SQL",
            Lang::Json => "JSON",
            Lang::Xml => "XML / HTML",
            Lang::Yaml => "YAML",
            Lang::Toml => "TOML / INI",
            Lang::Css => "CSS",
            Lang::Markdown => "Markdown",
        }
    }

    /// ¿Puede quedar algo abierto de una línea a la siguiente?
    pub fn multiline(self) -> bool {
        !matches!(self, Lang::Plain | Lang::Log | Lang::Json | Lang::Yaml | Lang::Toml | Lang::Shell)
    }
}

/// Lenguaje según el nombre del archivo (sin extensión conocida: log).
pub fn detect(name: &str) -> Lang {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "java" | "jav" => Lang::Java,
        "kt" | "kts" => Lang::Kotlin,
        "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
        "ts" | "tsx" | "mts" | "cts" => Lang::TypeScript,
        "rs" => Lang::Rust,
        "go" => Lang::Go,
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "m" | "mm" => Lang::C,
        "cs" => Lang::CSharp,
        "swift" => Lang::Swift,
        "py" | "pyw" | "pyi" => Lang::Python,
        "sh" | "bash" | "zsh" | "ksh" | "fish" | "command" => Lang::Shell,
        "sql" | "ddl" | "dml" => Lang::Sql,
        "json" | "jsonl" | "ndjson" | "geojson" | "har" => Lang::Json,
        "xml" | "html" | "htm" | "xhtml" | "svg" | "xsd" | "xsl" | "xslt" | "plist" | "pom" | "wsdl" | "csproj" | "vue" => {
            Lang::Xml
        }
        "yml" | "yaml" => Lang::Yaml,
        "toml" | "ini" | "cfg" | "conf" | "properties" | "env" => Lang::Toml,
        "css" | "scss" | "less" => Lang::Css,
        "md" | "markdown" | "mdx" | "mkd" => Lang::Markdown,
        "txt" | "text" | "csv" | "tsv" => Lang::Plain,
        _ => Lang::Log,
    }
}

/// Colorea una línea. Devuelve los tramos y el estado al final de la línea.
pub fn highlight(lang: Lang, s: &str, st: State, out: &mut Vec<Span>) -> State {
    match lang {
        Lang::Plain => State::Normal,
        Lang::Log => {
            scan_log(s, out);
            State::Normal
        }
        Lang::Json => {
            scan_json(s, out);
            State::Normal
        }
        Lang::Yaml => {
            scan_yaml(s, out);
            State::Normal
        }
        Lang::Toml => {
            scan_toml(s, out);
            State::Normal
        }
        Lang::Xml => scan_xml(s, st, out),
        Lang::Markdown => scan_md(s, st, out),
        _ => scan_code(def(lang), s, st, out),
    }
}

/// Estado tras recorrer `lines` desde un estado normal (para lo que viene antes de lo visible).
pub fn state_after<'a>(lang: Lang, lines: impl Iterator<Item = &'a str>) -> State {
    if !lang.multiline() {
        return State::Normal;
    }
    let mut st = State::Normal;
    let mut tmp = Vec::new();
    for l in lines {
        tmp.clear();
        st = highlight(lang, l, st, &mut tmp);
    }
    st
}

/// Colores para tema oscuro o claro.
pub fn color(kind: Kind, dark: bool) -> Color32 {
    let (d, l) = match kind {
        Kind::Keyword => ((0xc5, 0x86, 0xc0), (0xaf, 0x00, 0xdb)),
        Kind::String => ((0xce, 0x91, 0x78), (0xa3, 0x15, 0x15)),
        Kind::Comment => ((0x6a, 0x99, 0x55), (0x00, 0x80, 0x00)),
        Kind::Number => ((0xb5, 0xce, 0xa8), (0x09, 0x86, 0x58)),
        Kind::Tag => ((0x56, 0x9c, 0xd6), (0x80, 0x00, 0x00)),
        Kind::Attr => ((0x9c, 0xdc, 0xfe), (0x04, 0x51, 0xa5)),
        Kind::Error => ((0xf4, 0x47, 0x47), (0xd3, 0x2f, 0x2f)),
        Kind::Warn => ((0xe5, 0xc0, 0x7b), (0xb2, 0x6b, 0x00)),
        Kind::Info => ((0x4f, 0xc1, 0xff), (0x00, 0x70, 0xc1)),
        Kind::Debug => ((0x8a, 0x8a, 0x8a), (0x7a, 0x7a, 0x7a)),
        Kind::Date => ((0x7f, 0x9f, 0xbf), (0x5f, 0x6b, 0x7a)),
    };
    let (r, g, b) = if dark { d } else { l };
    Color32::from_rgb(r, g, b)
}

// ------------------------------------------------------------------ código

struct Def {
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [u8],
    triple: bool,
    hash_comment: bool,
    /// `@Anotación` / `@decorador` como atributo.
    at_attr: bool,
    /// `$VAR` / `${VAR}` como atributo.
    dollar_var: bool,
    /// Palabras clave sin distinguir mayúsculas (SQL).
    ci: bool,
    kw: &'static [&'static str],
}

const JAVA_KW: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const", "continue", "default", "do", "double",
    "else", "enum", "extends", "final", "finally", "float", "for", "goto", "if", "implements", "import", "instanceof", "int", "interface",
    "long", "native", "new", "package", "private", "protected", "public", "return", "short", "static", "strictfp", "super", "switch",
    "synchronized", "this", "throw", "throws", "transient", "try", "void", "volatile", "while", "var", "record", "yield", "sealed",
    "permits", "true", "false", "null",
];
const KOTLIN_KW: &[&str] = &[
    "as", "break", "class", "continue", "do", "else", "false", "for", "fun", "if", "in", "interface", "is", "null", "object", "package",
    "return", "super", "this", "throw", "true", "try", "typealias", "val", "var", "when", "while", "by", "catch", "constructor", "finally",
    "get", "import", "init", "set", "where", "override", "open", "data", "private", "public", "protected", "internal", "abstract",
    "final", "companion", "lateinit", "suspend", "enum", "sealed",
];
const JS_KW: &[&str] = &[
    "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends", "finally",
    "for", "function", "if", "import", "in", "instanceof", "let", "new", "return", "super", "switch", "this", "throw", "try", "typeof",
    "var", "void", "while", "with", "yield", "async", "await", "of", "static", "get", "set", "true", "false", "null", "undefined", "from",
];
const TS_KW: &[&str] = &[
    "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends", "finally",
    "for", "function", "if", "import", "in", "instanceof", "let", "new", "return", "super", "switch", "this", "throw", "try", "typeof",
    "var", "void", "while", "with", "yield", "async", "await", "of", "static", "get", "set", "true", "false", "null", "undefined", "from",
    "interface", "type", "enum", "implements", "namespace", "declare", "readonly", "private", "public", "protected", "abstract", "as",
    "any", "number", "string", "boolean", "never", "unknown", "keyof",
];
const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl",
    "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
    "true", "type", "unsafe", "use", "where", "while", "Some", "None", "Ok", "Err",
];
const GO_KW: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import",
    "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var", "true", "false", "nil",
];
const C_KW: &[&str] = &[
    "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else", "enum", "extern", "float", "for", "goto",
    "if", "inline", "int", "long", "register", "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef", "union",
    "unsigned", "void", "volatile", "while", "class", "namespace", "public", "private", "protected", "template", "typename", "virtual",
    "new", "delete", "this", "true", "false", "nullptr", "bool", "include", "define", "ifdef", "ifndef", "endif", "pragma", "using",
    "try", "catch", "throw", "auto", "constexpr", "override", "NULL",
];
const CS_KW: &[&str] = &[
    "abstract", "as", "base", "bool", "break", "byte", "case", "catch", "char", "class", "const", "continue", "decimal", "default",
    "delegate", "do", "double", "else", "enum", "event", "explicit", "extern", "false", "finally", "fixed", "float", "for", "foreach",
    "goto", "if", "implicit", "in", "int", "interface", "internal", "is", "lock", "long", "namespace", "new", "null", "object", "operator",
    "out", "override", "params", "private", "protected", "public", "readonly", "ref", "return", "sbyte", "sealed", "short", "sizeof",
    "static", "string", "struct", "switch", "this", "throw", "true", "try", "typeof", "uint", "ulong", "unsafe", "ushort", "using", "var",
    "virtual", "void", "volatile", "while", "async", "await", "get", "set", "record",
];
const SWIFT_KW: &[&str] = &[
    "class", "deinit", "enum", "extension", "func", "import", "init", "let", "protocol", "struct", "subscript", "typealias", "var",
    "break", "case", "continue", "default", "do", "else", "fallthrough", "for", "if", "in", "return", "switch", "where", "while", "as",
    "catch", "false", "is", "nil", "rethrows", "super", "self", "Self", "throw", "throws", "true", "try", "guard", "defer", "repeat",
    "private", "public", "internal", "fileprivate", "open", "static", "override", "mutating", "async", "await",
];
const PY_KW: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
    "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
    "return", "try", "while", "with", "yield", "self", "match", "case",
];
const SH_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done", "in", "function", "return", "exit",
    "export", "local", "readonly", "echo", "set", "unset", "source", "alias", "cd", "true", "false",
];
const SQL_KW: &[&str] = &[
    "select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set", "delete", "create", "table", "drop",
    "alter", "index", "view", "join", "inner", "left", "right", "outer", "full", "on", "as", "group", "by", "order", "having", "limit",
    "offset", "union", "all", "distinct", "null", "is", "in", "exists", "between", "like", "case", "when", "then", "else", "end",
    "primary", "key", "foreign", "references", "default", "begin", "commit", "rollback", "transaction", "with", "returning", "true",
    "false", "asc", "desc", "count", "sum", "avg", "min", "max", "varchar", "integer", "int", "text", "date", "timestamp", "boolean",
];

fn def(lang: Lang) -> Def {
    let c = |kw| Def {
        line: &["//"],
        block: Some(("/*", "*/")),
        quotes: b"\"'",
        triple: false,
        hash_comment: false,
        at_attr: false,
        dollar_var: false,
        ci: false,
        kw,
    };
    match lang {
        Lang::Java => Def { at_attr: true, ..c(JAVA_KW) },
        Lang::Kotlin => Def { at_attr: true, ..c(KOTLIN_KW) },
        Lang::JavaScript => Def { quotes: b"\"'`", ..c(JS_KW) },
        Lang::TypeScript => Def { quotes: b"\"'`", at_attr: true, ..c(TS_KW) },
        Lang::Rust => Def { quotes: b"\"", ..c(RUST_KW) },
        Lang::Go => Def { quotes: b"\"'`", ..c(GO_KW) },
        Lang::C => c(C_KW),
        Lang::CSharp => c(CS_KW),
        Lang::Swift => Def { at_attr: true, quotes: b"\"", ..c(SWIFT_KW) },
        Lang::Css => Def { line: &[], at_attr: true, ..c(&[]) },
        Lang::Python => Def { line: &[], block: None, triple: true, hash_comment: true, at_attr: true, ..c(PY_KW) },
        Lang::Shell => Def { line: &[], block: None, hash_comment: true, dollar_var: true, ..c(SH_KW) },
        Lang::Sql => Def { line: &["--"], quotes: b"'\"", ci: true, ..c(SQL_KW) },
        _ => c(&[]),
    }
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn push(out: &mut Vec<Span>, start: usize, end: usize, kind: Kind) {
    if end > start {
        out.push(Span { start, end, kind });
    }
}

/// Avanza un carácter (UTF-8) desde `i`.
fn next_char(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(i + 1, |c| i + c.len_utf8())
}

/// Fin de una cadena que empieza en `i` (comilla incluida), con escapes `\`.
fn string_end(b: &[u8], i: usize) -> usize {
    let q = b[i];
    let mut j = i + 1;
    while j < b.len() {
        if b[j] == b'\\' {
            j += 2;
            continue;
        }
        if b[j] == q {
            return j + 1;
        }
        j += 1;
    }
    b.len()
}

fn number_end(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'_') {
        j += 1;
    }
    j
}

fn scan_code(d: Def, s: &str, mut st: State, out: &mut Vec<Span>) -> State {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    // Lo que venía abierto de la línea anterior.
    match st {
        State::Block => {
            let close = d.block.map_or("*/", |b| b.1);
            match s.find(close) {
                Some(e) => {
                    push(out, 0, e + close.len(), Kind::Comment);
                    i = e + close.len();
                    st = State::Normal;
                }
                None => {
                    push(out, 0, n, Kind::Comment);
                    return State::Block;
                }
            }
        }
        State::Triple(q) => {
            let close = [q, q, q];
            let close = std::str::from_utf8(&close).unwrap_or("\"\"\"");
            match s.find(close) {
                Some(e) => {
                    push(out, 0, e + 3, Kind::String);
                    i = e + 3;
                    st = State::Normal;
                }
                None => {
                    push(out, 0, n, Kind::String);
                    return st;
                }
            }
        }
        _ => {}
    }
    while i < n {
        let c = b[i];
        let rest = &s[i..];
        if d.line.iter().any(|p| rest.starts_with(p)) || (d.hash_comment && c == b'#' && (i == 0 || b[i - 1].is_ascii_whitespace())) {
            push(out, i, n, Kind::Comment);
            break;
        }
        if let Some((open, close)) = d.block {
            if rest.starts_with(open) {
                match s[i + open.len()..].find(close) {
                    Some(e) => {
                        let end = i + open.len() + e + close.len();
                        push(out, i, end, Kind::Comment);
                        i = end;
                        continue;
                    }
                    None => {
                        push(out, i, n, Kind::Comment);
                        return State::Block;
                    }
                }
            }
        }
        if d.triple && (c == b'"' || c == b'\'') && b.get(i + 1) == Some(&c) && b.get(i + 2) == Some(&c) {
            let close = std::str::from_utf8(&b[i..i + 3]).unwrap_or("\"\"\"");
            match s[i + 3..].find(close) {
                Some(e) => {
                    let end = i + 3 + e + 3;
                    push(out, i, end, Kind::String);
                    i = end;
                    continue;
                }
                None => {
                    push(out, i, n, Kind::String);
                    return State::Triple(c);
                }
            }
        }
        if d.quotes.contains(&c) {
            let end = string_end(b, i);
            push(out, i, end, Kind::String);
            i = end;
            continue;
        }
        if c.is_ascii_digit() && (i == 0 || !is_ident(b[i - 1])) {
            let end = number_end(b, i);
            push(out, i, end, Kind::Number);
            i = end;
            continue;
        }
        if d.at_attr && c == b'@' {
            let mut j = i + 1;
            while j < n && (is_ident(b[j]) || b[j] == b'.' || b[j] == b'-') {
                j += 1;
            }
            push(out, i, j, Kind::Attr);
            i = j.max(i + 1);
            continue;
        }
        if d.dollar_var && c == b'$' {
            let j = if b.get(i + 1) == Some(&b'{') {
                s[i..].find('}').map_or(n, |e| i + e + 1)
            } else {
                let mut j = i + 1;
                while j < n && (is_ident(b[j]) || matches!(b[j], b'?' | b'@' | b'#')) {
                    j += 1;
                }
                j
            };
            push(out, i, j, Kind::Attr);
            i = j.max(i + 1);
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i;
            while j < n && is_ident(b[j]) {
                j += 1;
            }
            let w = &s[i..j];
            let kw = if d.ci { d.kw.iter().any(|k| k.eq_ignore_ascii_case(w)) } else { d.kw.contains(&w) };
            if kw {
                push(out, i, j, Kind::Keyword);
            }
            i = j;
            continue;
        }
        i = next_char(s, i);
    }
    st
}

// ------------------------------------------------------------------ datos

fn scan_json(s: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        let c = b[i];
        if c == b'"' {
            let end = string_end(b, i);
            // Una cadena seguida de «:» es una clave.
            let mut k = end;
            while k < n && b[k].is_ascii_whitespace() {
                k += 1;
            }
            push(out, i, end, if b.get(k) == Some(&b':') { Kind::Attr } else { Kind::String });
            i = end;
        } else if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let end = number_end(b, i + 1);
            push(out, i, end, Kind::Number);
            i = end;
        } else if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < n && b[j].is_ascii_alphabetic() {
                j += 1;
            }
            if matches!(&s[i..j], "true" | "false" | "null") {
                push(out, i, j, Kind::Keyword);
            }
            i = j;
        } else {
            i = next_char(s, i);
        }
    }
}

/// Valor de YAML/TOML: cadenas, números, booleanos y comentario final.
fn scan_value(s: &str, from: usize, comment: &[u8], out: &mut Vec<Span>) {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = from;
    while i < n {
        let c = b[i];
        if comment.contains(&c) && (i == 0 || b[i - 1].is_ascii_whitespace()) {
            push(out, i, n, Kind::Comment);
            return;
        }
        if c == b'"' || c == b'\'' {
            let end = string_end(b, i);
            push(out, i, end, Kind::String);
            i = end;
        } else if (c.is_ascii_digit() || c == b'-') && (i == 0 || !is_ident(b[i - 1])) && b.get(i + usize::from(c == b'-')).is_some_and(|d| d.is_ascii_digit()) {
            let end = number_end(b, i + 1);
            push(out, i, end, Kind::Number);
            i = end;
        } else if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < n && is_ident(b[j]) {
                j += 1;
            }
            if matches!(s[i..j].to_ascii_lowercase().as_str(), "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "none") {
                push(out, i, j, Kind::Keyword);
            }
            i = j;
        } else {
            i = next_char(s, i);
        }
    }
}

fn scan_yaml(s: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    let ind = s.len() - s.trim_start().len();
    let t = &s[ind..];
    if t.starts_with('#') {
        push(out, ind, s.len(), Kind::Comment);
        return;
    }
    if t.starts_with("---") || t.starts_with("...") {
        push(out, ind, ind + 3, Kind::Keyword);
        return;
    }
    let mut k = ind;
    if t.starts_with("- ") {
        k += 2;
    }
    // «clave:» al principio de la línea.
    if let Some(colon) = s[k..].find(':').map(|c| k + c) {
        let key = &s[k..colon];
        let after_ok = b.get(colon + 1).is_none_or(|c| c.is_ascii_whitespace());
        if after_ok && !key.is_empty() && !key.contains(['#', '"', '\'', '{', '[']) {
            push(out, k, colon, Kind::Attr);
            scan_value(s, colon + 1, b"#", out);
            return;
        }
    }
    scan_value(s, k, b"#", out);
}

fn scan_toml(s: &str, out: &mut Vec<Span>) {
    let ind = s.len() - s.trim_start().len();
    let t = &s[ind..];
    if t.starts_with('#') || t.starts_with(';') || t.starts_with('!') {
        push(out, ind, s.len(), Kind::Comment);
        return;
    }
    if t.starts_with('[') {
        let end = s.find(']').map_or(s.len(), |e| e + 1);
        push(out, ind, end, Kind::Tag);
        scan_value(s, end, b"#;", out);
        return;
    }
    if let Some(eq) = s.find(['=', ':']) {
        push(out, ind, s[..eq].trim_end().len(), Kind::Attr);
        scan_value(s, eq + 1, b"#;", out);
    }
}

fn scan_xml(s: &str, mut st: State, out: &mut Vec<Span>) -> State {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    if st == State::XmlComment {
        match s.find("-->") {
            Some(e) => {
                push(out, 0, e + 3, Kind::Comment);
                i = e + 3;
                st = State::Normal;
            }
            None => {
                push(out, 0, n, Kind::Comment);
                return State::XmlComment;
            }
        }
    }
    while i < n {
        if s[i..].starts_with("<!--") {
            match s[i + 4..].find("-->") {
                Some(e) => {
                    let end = i + 4 + e + 3;
                    push(out, i, end, Kind::Comment);
                    i = end;
                    continue;
                }
                None => {
                    push(out, i, n, Kind::Comment);
                    return State::XmlComment;
                }
            }
        }
        if b[i] == b'<' {
            // Nombre de la etiqueta.
            let mut j = i + 1;
            while j < n && matches!(b[j], b'/' | b'?' | b'!') {
                j += 1;
            }
            while j < n && (is_ident(b[j]) || matches!(b[j], b':' | b'-' | b'.')) {
                j += 1;
            }
            push(out, i, j, Kind::Tag);
            // Atributos hasta «>».
            while j < n && b[j] != b'>' {
                if b[j] == b'"' || b[j] == b'\'' {
                    let end = string_end(b, j);
                    push(out, j, end, Kind::String);
                    j = end;
                } else if is_ident(b[j]) {
                    let mut k = j;
                    while k < n && (is_ident(b[k]) || matches!(b[k], b':' | b'-' | b'.')) {
                        k += 1;
                    }
                    push(out, j, k, Kind::Attr);
                    j = k;
                } else if b[j] == b'/' || b[j] == b'?' {
                    push(out, j, j + 1, Kind::Tag);
                    j += 1;
                } else {
                    j = next_char(s, j);
                }
            }
            if j < n {
                push(out, j, j + 1, Kind::Tag);
                j += 1;
            }
            i = j;
            continue;
        }
        i = next_char(s, i);
    }
    st
}

// ------------------------------------------------------------------ Markdown

fn scan_md(s: &str, st: State, out: &mut Vec<Span>) -> State {
    let n = s.len();
    let ind = n - s.trim_start().len();
    let t = &s[ind..];
    let fence = t.starts_with("```") || t.starts_with("~~~");
    // Bloques de código: la línea de la valla y su contenido.
    if st == State::Fence {
        push(out, 0, n, if fence { Kind::Keyword } else { Kind::String });
        return if fence { State::Normal } else { State::Fence };
    }
    if fence {
        push(out, 0, n, Kind::Keyword);
        return State::Fence;
    }
    // Títulos, citas y separadores ocupan la línea entera.
    let hashes = t.bytes().take_while(|&c| c == b'#').count();
    if (1..=6).contains(&hashes) && (t.len() == hashes || t.as_bytes()[hashes] == b' ') {
        push(out, ind, n, Kind::Tag);
        return State::Normal;
    }
    if t.starts_with('>') {
        push(out, ind, n, Kind::Comment);
        return State::Normal;
    }
    let rule = t.trim_end();
    if rule.len() >= 3 && (rule.chars().all(|c| c == '-') || rule.chars().all(|c| c == '*') || rule.chars().all(|c| c == '_')) {
        push(out, ind, n, Kind::Keyword);
        return State::Normal;
    }
    // Viñetas y listas numeradas.
    let b = s.as_bytes();
    let mut i = ind;
    if matches!(t.as_bytes().first(), Some(b'-' | b'*' | b'+')) && t.as_bytes().get(1) == Some(&b' ') {
        push(out, ind, ind + 1, Kind::Keyword);
        i = ind + 2;
    } else {
        let d = t.bytes().take_while(u8::is_ascii_digit).count();
        if d > 0 && matches!(t.as_bytes().get(d), Some(b'.' | b')')) && t.as_bytes().get(d + 1) == Some(&b' ') {
            push(out, ind, ind + d + 1, Kind::Keyword);
            i = ind + d + 2;
        }
    }
    // En la línea: `código`, **negrita**, *cursiva*, [enlaces](url), tablas.
    while i < n {
        let c = b[i];
        if c == b'`' {
            let ticks = s[i..].bytes().take_while(|&x| x == b'`').count();
            let close = "`".repeat(ticks);
            match s[i + ticks..].find(&close) {
                Some(e) => {
                    let end = i + ticks + e + ticks;
                    push(out, i, end, Kind::String);
                    i = end;
                }
                None => i += ticks,
            }
            continue;
        }
        if (c == b'*' || c == b'_') && b.get(i + 1) == Some(&c) {
            let close = if c == b'*' { "**" } else { "__" };
            if let Some(e) = s[i + 2..].find(close).filter(|&e| e > 0) {
                let end = i + 2 + e + 2;
                push(out, i, end, Kind::Attr);
                i = end;
                continue;
            }
        }
        if (c == b'*' || c == b'_') && b.get(i + 1).is_some_and(|x| !x.is_ascii_whitespace()) && (i == 0 || !is_ident(b[i - 1])) {
            if let Some(e) = s[i + 1..].find(c as char).filter(|&e| e > 0) {
                let end = i + 1 + e + 1;
                if b[end - 2] != b' ' {
                    push(out, i, end, Kind::Number);
                    i = end;
                    continue;
                }
            }
        }
        if c == b'[' || (c == b'!' && b.get(i + 1) == Some(&b'[')) {
            let open = if c == b'!' { i + 1 } else { i };
            if let Some(cb) = s[open..].find("](").map(|e| open + e) {
                if let Some(cp) = s[cb + 2..].find(')').map(|e| cb + 2 + e) {
                    push(out, i, cb + 1, Kind::Info);
                    push(out, cb + 1, cp + 1, Kind::Attr);
                    i = cp + 1;
                    continue;
                }
            }
        }
        if c == b'|' {
            push(out, i, i + 1, Kind::Debug);
        }
        i = next_char(s, i);
    }
    State::Normal
}

// ------------------------------------------------------------------ logs

fn level(w: &str) -> Option<Kind> {
    Some(match w {
        "ERROR" | "ERR" | "FATAL" | "SEVERE" | "CRITICAL" | "CRIT" | "EMERG" | "ALERT" | "FAIL" | "FAILED" | "PANIC" => Kind::Error,
        "WARN" | "WARNING" => Kind::Warn,
        "INFO" | "NOTICE" => Kind::Info,
        "DEBUG" | "TRACE" | "FINE" | "FINER" | "FINEST" | "VERBOSE" => Kind::Debug,
        _ => return None,
    })
}

fn digits(b: &[u8], i: usize, k: usize) -> bool {
    i + k <= b.len() && b[i..i + k].iter().all(u8::is_ascii_digit)
}

/// Longitud de una fecha `AAAA-MM-DD` (o con `/`) o una hora `HH:MM:SS[.fff]` en `i`.
fn date_len(b: &[u8], i: usize) -> usize {
    if digits(b, i, 4) && matches!(b.get(i + 4), Some(b'-' | b'/')) && digits(b, i + 5, 2) && b.get(i + 7) == b.get(i + 4) && digits(b, i + 8, 2) {
        return 10;
    }
    if digits(b, i, 2) && b.get(i + 2) == Some(&b':') && digits(b, i + 3, 2) && b.get(i + 5) == Some(&b':') && digits(b, i + 6, 2) {
        let mut j = i + 8;
        if matches!(b.get(j), Some(b'.' | b',')) {
            j += 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
        }
        if b.get(j) == Some(&b'Z') {
            j += 1;
        }
        return j - i;
    }
    0
}

fn scan_log(s: &str, out: &mut Vec<Span>) {
    let b = s.as_bytes();
    let n = b.len();
    let t = s.trim_start();
    // Trazas de pila: «at com.x.Y(Y.java:12)» atenuadas; «Caused by:» como error.
    if t.len() < s.len() && (t.starts_with("at ") || t.starts_with("... ")) {
        push(out, 0, n, Kind::Comment);
        return;
    }
    if t.starts_with("Caused by:") {
        let st = n - t.len();
        push(out, st, st + "Caused by:".len(), Kind::Error);
    }
    let mut i = 0;
    while i < n {
        let c = b[i];
        if c.is_ascii_digit() && (i == 0 || !is_ident(b[i - 1])) {
            let l = date_len(b, i);
            if l > 0 {
                push(out, i, i + l, Kind::Date);
                i += l;
                continue;
            }
            let end = number_end(b, i);
            i = end;
            continue;
        }
        if c == b'"' {
            let end = string_end(b, i);
            push(out, i, end, Kind::String);
            i = end;
            continue;
        }
        if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < n && (is_ident(b[j]) || b[j] == b'.' || b[j] == b'$') {
                j += 1;
            }
            let w = &s[i..j];
            if let Some(k) = level(w) {
                push(out, i, j, k);
            } else {
                // NullPointerException, java.io.IOException, OutOfMemoryError…
                let last = w.rsplit('.').next().unwrap_or(w);
                if (last.ends_with("Exception") || last.ends_with("Error")) && last.as_bytes()[0].is_ascii_uppercase() && last.len() > 5 {
                    push(out, i, j, Kind::Error);
                }
            }
            i = j;
            continue;
        }
        i = next_char(s, i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(lang: Lang, s: &str) -> Vec<(String, Kind)> {
        let mut out = Vec::new();
        highlight(lang, s, State::Normal, &mut out);
        out.into_iter().map(|sp| (s[sp.start..sp.end].to_string(), sp.kind)).collect()
    }

    #[test]
    fn detect_by_extension() {
        assert_eq!(detect("App.java"), Lang::Java);
        assert_eq!(detect("pom.XML"), Lang::Xml);
        assert_eq!(detect("datos.json"), Lang::Json);
        assert_eq!(detect("script.py"), Lang::Python);
        assert_eq!(detect("servidor.log"), Lang::Log);
        assert_eq!(detect("Sin título 1"), Lang::Log);
    }

    #[test]
    fn log_levels_dates_exceptions() {
        let v = spans(Lang::Log, "2026-11-10 12:00:01.123 [ERROR] fallo: java.lang.NullPointerException en \"x\"");
        assert!(v.contains(&("2026-11-10".into(), Kind::Date)));
        assert!(v.contains(&("12:00:01.123".into(), Kind::Date)));
        assert!(v.contains(&("ERROR".into(), Kind::Error)));
        assert!(v.contains(&("java.lang.NullPointerException".into(), Kind::Error)));
        assert!(v.contains(&("\"x\"".into(), Kind::String)));
        assert_eq!(spans(Lang::Log, "    at com.a.B.c(B.java:12)")[0].1, Kind::Comment);
        assert!(spans(Lang::Log, "un error menor").is_empty(), "solo niveles en mayúsculas");
    }

    #[test]
    fn code_and_multiline_state() {
        let v = spans(Lang::Java, "public static int x = 42; // nota");
        assert!(v.contains(&("public".into(), Kind::Keyword)));
        assert!(v.contains(&("42".into(), Kind::Number)));
        assert!(v.contains(&("// nota".into(), Kind::Comment)));
        let mut out = Vec::new();
        let st = highlight(Lang::Java, "int a; /* empieza", State::Normal, &mut out);
        assert_eq!(st, State::Block);
        out.clear();
        let st = highlight(Lang::Java, "sigue */ return", st, &mut out);
        assert_eq!((st, out[0].end), (State::Normal, "sigue */".len()));
        assert_eq!(state_after(Lang::Python, ["x = \"\"\"doc", "más"].into_iter()), State::Triple(b'"'));
        let v = spans(Lang::Python, "@app.route('/') # ruta");
        assert_eq!(v[0], ("@app.route".into(), Kind::Attr));
    }

    #[test]
    fn markdown() {
        let v = spans(Lang::Markdown, "## Instalar");
        assert_eq!(v[0], ("## Instalar".into(), Kind::Tag));
        let v = spans(Lang::Markdown, "- usa `cargo` y **ojo** con *esto*: [docs](https://x.y)");
        assert!(v.contains(&("-".into(), Kind::Keyword)));
        assert!(v.contains(&("`cargo`".into(), Kind::String)));
        assert!(v.contains(&("**ojo**".into(), Kind::Attr)));
        assert!(v.contains(&("*esto*".into(), Kind::Number)));
        assert!(v.contains(&("[docs]".into(), Kind::Info)));
        assert!(v.contains(&("(https://x.y)".into(), Kind::Attr)));
        assert_eq!(state_after(Lang::Markdown, ["texto", "```sh", "ls -la"].into_iter()), State::Fence);
        assert_eq!(state_after(Lang::Markdown, ["```", "x", "```"].into_iter()), State::Normal);
        assert!(spans(Lang::Markdown, "2 * 3 * 4").is_empty() || !spans(Lang::Markdown, "2 * 3 * 4").iter().any(|x| x.1 == Kind::Number));
    }

    #[test]
    fn data_formats() {
        let v = spans(Lang::Json, r#"{"nombre": "Ana", "edad": 30, "ok": true}"#);
        assert!(v.contains(&("\"nombre\"".into(), Kind::Attr)));
        assert!(v.contains(&("\"Ana\"".into(), Kind::String)));
        assert!(v.contains(&("30".into(), Kind::Number)));
        assert!(v.contains(&("true".into(), Kind::Keyword)));
        let v = spans(Lang::Xml, r#"<dep id="a"><!-- c --></dep>"#);
        assert!(v.contains(&("<dep".into(), Kind::Tag)));
        assert!(v.contains(&("id".into(), Kind::Attr)));
        assert!(v.contains(&("\"a\"".into(), Kind::String)));
        assert!(v.contains(&("<!-- c -->".into(), Kind::Comment)));
        let v = spans(Lang::Yaml, "  puerto: 8080 # http");
        assert!(v.contains(&("puerto".into(), Kind::Attr)));
        assert!(v.contains(&("8080".into(), Kind::Number)));
        assert!(v.contains(&("# http".into(), Kind::Comment)));
    }
}
