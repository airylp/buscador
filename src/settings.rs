//! Ajustes persistentes (tema, tamaño de letra) en un archivo de texto
//! `clave=valor` en la carpeta de configuración del sistema.

use eframe::egui::ThemePreference;
use std::path::PathBuf;

#[derive(Clone, PartialEq)]
pub struct Settings {
    pub theme: ThemePreference,
    pub font_size: f32,
    /// Tipo de letra del texto (nombre de `fonts::OPTIONS`).
    pub font: String,
    /// Colores propios del texto (fondo, letra); `None` = los del tema.
    pub colors: Option<([u8; 3], [u8; 3])>,
    /// Resaltado de sintaxis y niveles de log.
    pub syntax: bool,
    /// Ajuste de línea: «no», «ventana» o «columnas:N».
    pub wrap: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { theme: ThemePreference::System, font_size: 13.0, font: crate::fonts::DEFAULT.into(), colors: None, syntax: true, wrap: "no".into() }
    }
}

/// `~/Library/Application Support/buscador` (macOS), `%APPDATA%\buscador`
/// (Windows) o `$XDG_CONFIG_HOME/buscador` / `~/.config/buscador`.
fn path() -> Option<PathBuf> {
    Some(config_dir()?.join("ajustes.txt"))
}

/// Carpeta de trabajo de la aplicación: `~/Library/Application Support/buscador`
/// (macOS), `%APPDATA%\buscador` (Windows) o `~/.config/buscador`.
pub fn config_dir() -> Option<PathBuf> {
    let env = |k| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        env("APPDATA")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    }?;
    Some(base.join("buscador"))
}

/// Un archivo de la sesión: dónde estaba la vista y el cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionFile {
    pub path: PathBuf,
    pub top: u64,
    pub line: u64,
    pub col: usize,
    /// Encoding elegido (nombre de encoding_rs).
    pub encoding: String,
}

/// Archivos abiertos al cerrar y cuál estaba activo (`sesion.txt`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Session {
    pub files: Vec<SessionFile>,
    pub active: usize,
}

impl Session {
    fn file() -> Option<PathBuf> {
        Some(config_dir()?.join("sesion.txt"))
    }

    /// Formato: `activo=N` y una línea por archivo
    /// `archivo=arriba|línea|columna|encoding|ruta` (la ruta va al final: puede tener «|»).
    pub fn to_text(&self) -> String {
        let mut t = format!("activo={}\n", self.active);
        for f in &self.files {
            t += &format!("archivo={}|{}|{}|{}|{}\n", f.top, f.line, f.col, f.encoding, f.path.display());
        }
        t
    }

    pub fn from_text(text: &str) -> Self {
        let mut s = Session::default();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("activo=") {
                s.active = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("archivo=") {
                let mut it = v.splitn(5, '|');
                let (Some(top), Some(l), Some(c), Some(enc), Some(path)) = (it.next(), it.next(), it.next(), it.next(), it.next()) else {
                    continue;
                };
                s.files.push(SessionFile {
                    path: PathBuf::from(path),
                    top: top.parse().unwrap_or(0),
                    line: l.parse().unwrap_or(0),
                    col: c.parse().unwrap_or(0),
                    encoding: enc.to_string(),
                });
            }
        }
        s
    }

    pub fn load() -> Self {
        Self::file().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::from_text(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::file().ok_or("no se encontró la carpeta de configuración")?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // Escribir a un temporal y renombrar: nunca queda a medias.
        let tmp = p.with_extension("txt.tmp");
        std::fs::write(&tmp, self.to_text()).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| e.to_string())
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Settings::default();
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else { return s };
        let (mut bg, mut fg) = (None, None);
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match (k.trim(), v.trim()) {
                ("tema", "claro") => s.theme = ThemePreference::Light,
                ("tema", "oscuro") => s.theme = ThemePreference::Dark,
                ("tema", _) => s.theme = ThemePreference::System,
                ("fuente", v) if !v.is_empty() => s.font = v.to_string(),
                ("resaltar", v) => s.syntax = v != "no",
                ("ajuste", v) => s.wrap = v.to_string(),
                ("fondo", v) => bg = parse_hex(v),
                ("color_letra", v) => fg = parse_hex(v),
                ("letra", v) => {
                    if let Ok(f) = v.parse::<f32>() {
                        s.font_size = f.clamp(7.0, 40.0);
                    }
                }
                _ => {}
            }
        }
        if let (Some(b), Some(f)) = (bg, fg) {
            s.colors = Some((b, f));
        }
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let p = path().ok_or("no se encontró la carpeta de configuración")?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let theme = match self.theme {
            ThemePreference::Light => "claro",
            ThemePreference::Dark => "oscuro",
            ThemePreference::System => "sistema",
        };
        let mut text = format!(
            "tema={theme}\nletra={}\nfuente={}\nresaltar={}\najuste={}\n",
            self.font_size,
            self.font,
            if self.syntax { "si" } else { "no" },
            self.wrap
        );
        if let Some((b, f)) = self.colors {
            text += &format!("fondo={}\ncolor_letra={}\n", hex(b), hex(f));
        }
        std::fs::write(&p, text).map_err(|e| e.to_string())
    }
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn parse_hex(v: &str) -> Option<[u8; 3]> {
    let v = v.trim().trim_start_matches('#');
    if v.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(v, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

#[cfg(test)]
mod tests {
    #[test]
    fn session_roundtrip() {
        use super::{Session, SessionFile};
        let s = Session {
            active: 1,
            files: vec![
                SessionFile { path: "/a/b.log".into(), top: 10, line: 12, col: 3, encoding: "UTF-8".into() },
                SessionFile { path: "/c/con|barra.txt".into(), top: 0, line: 0, col: 0, encoding: "windows-1252".into() },
            ],
        };
        assert_eq!(Session::from_text(&s.to_text()), s);
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(super::parse_hex("#1e2a38"), Some([0x1e, 0x2a, 0x38]));
        assert_eq!(super::hex([0x1e, 0x2a, 0x38]), "#1e2a38");
        assert_eq!(super::parse_hex("zz"), None);
    }
}
