//! Idioma de la interfaz: español (el del código), inglés y francés.
//!
//! Los textos se escriben en español en el código con `t!("…")` o, si llevan
//! valores, `tf!("… {n} …", n = valor)`. Las traducciones están en
//! `i18n.tsv` (columnas: español, inglés, francés; `\n` = salto de línea).
//! Si falta una traducción se muestra el español.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Es,
    En,
    Fr,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::Es, Lang::En, Lang::Fr];

    pub fn code(self) -> &'static str {
        match self {
            Lang::Es => "es",
            Lang::En => "en",
            Lang::Fr => "fr",
        }
    }

    /// Nombre en su propio idioma (así se reconoce aunque no se entienda el actual).
    pub fn name(self) -> &'static str {
        match self {
            Lang::Es => "Español",
            Lang::En => "English",
            Lang::Fr => "Français",
        }
    }

    pub fn from_code(c: &str) -> Option<Lang> {
        let c = c.trim().to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| c.starts_with(l.code()))
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(l: Lang) {
    CURRENT.store(l as u8, Ordering::Relaxed);
}

pub fn get() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::En,
        2 => Lang::Fr,
        _ => Lang::Es,
    }
}

/// Idioma del sistema (si es uno de los disponibles).
pub fn system() -> Option<Lang> {
    #[cfg(target_os = "macos")]
    {
        // Las apps abiertas desde Finder no tienen LANG: se pregunta a macOS.
        if let Ok(o) = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output() {
            let s = String::from_utf8_lossy(&o.stdout);
            if let Some(first) = s.split(['"', '(', ',', '\n', ' ']).find(|w| w.len() >= 2 && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())) {
                if let Some(l) = Lang::from_code(first) {
                    return Some(l);
                }
            }
        }
    }
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|k| std::env::var(k).ok()).find(|v| !v.is_empty() && v != "C" && v != "POSIX").and_then(|v| Lang::from_code(&v))
}

type Table = HashMap<&'static str, (&'static str, &'static str)>;

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| parse(include_str!("i18n.tsv")))
}

fn parse(src: &'static str) -> Table {
    let mut m = HashMap::new();
    for line in src.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split('\t');
        let (Some(es), Some(en), Some(fr)) = (it.next(), it.next(), it.next()) else { continue };
        m.insert(unescape(es), (unescape(en), unescape(fr)));
    }
    m
}

/// `\n` y `\t` escritos en el TSV → caracteres reales. Las cadenas viven toda
/// la ejecución (la tabla se crea una vez), así que se «filtran» a `'static`.
fn unescape(s: &'static str) -> &'static str {
    if !s.contains('\\') {
        return s;
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    Box::leak(out.into_boxed_str())
}

/// Traducción de un texto escrito en español.
pub fn tr(es: &'static str) -> &'static str {
    let l = get();
    if l == Lang::Es {
        return es;
    }
    match table().get(es) {
        Some((en, fr)) => {
            let s = if l == Lang::En { *en } else { *fr };
            if s.is_empty() { es } else { s }
        }
        None => es,
    }
}

/// Sustituye `{nombre}` por su valor (para `tf!`).
pub fn fill(template: &str, args: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) if args.iter().any(|(k, _)| *k == &after[..j]) => {
                let key = &after[..j];
                out.push_str(&args.iter().find(|(k, _)| *k == key).unwrap().1);
                rest = &after[j + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Texto traducido: `t!("Abrir")`.
#[macro_export]
macro_rules! t {
    ($s:literal) => {
        $crate::i18n::tr($s)
    };
}

/// Texto traducido con valores: `tf!("{n} líneas", n = fmt_num(x))`.
#[macro_export]
macro_rules! tf {
    ($s:literal $(, $k:ident = $v:expr)* $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($s), &[$((stringify!($k), ($v).to_string())),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_and_fallback() {
        assert_eq!(fill("{n} de {m} {x}", &[("n", "1".into()), ("m", "2".into())]), "1 de 2 {x}");
        set(Lang::Es);
        assert_eq!(tr("texto sin traducir"), "texto sin traducir");
    }

    /// Todo texto marcado con t!/tf! en el código tiene traducción completa
    /// (inglés y francés) y con los mismos `{marcadores}`.
    #[test]
    fn every_text_is_translated() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let t = table();
        let mut missing = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "rs") || p.ends_with("i18n.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            for lit in literals(&src) {
                let key = unescape(Box::leak(lit.clone().into_boxed_str()));
                match t.get(key) {
                    Some((en, fr)) if !en.is_empty() && !fr.is_empty() => {
                        let marks = |s: &str| {
                            let mut v: Vec<String> = s.match_indices('{').filter_map(|(i, _)| s[i..].find('}').map(|j| s[i..i + j + 1].to_string())).collect();
                            v.sort();
                            v
                        };
                        let es = key;
                        if marks(es) != marks(en) || marks(es) != marks(fr) {
                            missing.push(format!("marcadores distintos: {lit}"));
                        }
                    }
                    _ => missing.push(lit),
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "{} textos sin traducir:\n{}", missing.len(), missing.join("\n"));
    }

    /// Literales dentro de `t!("…")` y `tf!("…"` tal como están en el código
    /// (con `\n` escrito, igual que en el TSV).
    fn literals(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        for m in ["t!(\"", "tf!(\""] {
            let mut rest = src;
            while let Some(i) = rest.find(m) {
                // Evitar coincidir dentro de otras palabras (p. ej. «assert!(»).
                let prev = rest[..i].chars().last();
                let s = &rest[i + m.len()..];
                let mut end = 0;
                let b = s.as_bytes();
                while end < b.len() && !(b[end] == b'"' && (end == 0 || b[end - 1] != b'\\')) {
                    end += 1;
                }
                if !prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    out.push(s[..end].to_string());
                }
                rest = &s[end..];
            }
        }
        out
    }
}
