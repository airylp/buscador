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
}

impl Default for Settings {
    fn default() -> Self {
        Settings { theme: ThemePreference::System, font_size: 13.0, font: crate::fonts::DEFAULT.into() }
    }
}

/// `~/Library/Application Support/buscador` (macOS), `%APPDATA%\buscador`
/// (Windows) o `$XDG_CONFIG_HOME/buscador` / `~/.config/buscador`.
fn path() -> Option<PathBuf> {
    let env = |k| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        env("APPDATA")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    }?;
    Some(base.join("buscador").join("ajustes.txt"))
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Settings::default();
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else { return s };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match (k.trim(), v.trim()) {
                ("tema", "claro") => s.theme = ThemePreference::Light,
                ("tema", "oscuro") => s.theme = ThemePreference::Dark,
                ("tema", _) => s.theme = ThemePreference::System,
                ("fuente", v) if !v.is_empty() => s.font = v.to_string(),
                ("letra", v) => {
                    if let Ok(f) = v.parse::<f32>() {
                        s.font_size = f.clamp(7.0, 40.0);
                    }
                }
                _ => {}
            }
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
        std::fs::write(&p, format!("tema={theme}\nletra={}\nfuente={}\n", self.font_size, self.font)).map_err(|e| e.to_string())
    }
}
