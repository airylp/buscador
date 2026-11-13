//! Tipos de letra para el texto (vista del archivo, resultados, detalle).
//!
//! Se ofrecen las fuentes que existan en el sistema, más las dos que trae
//! egui. El texto usa la familia propia `texto`; los menús y botones no
//! cambian. Detrás de la fuente elegida siempre van las de respaldo (CJK,
//! símbolos…) para los caracteres que no tenga.

use eframe::egui::{FontData, FontDefinitions, FontFamily, FontId};
use std::sync::Arc;

pub struct FontOption {
    pub name: &'static str,
    /// Todas las letras ocupan lo mismo (las columnas de los logs se alinean).
    pub mono: bool,
    /// Rutas posibles (la primera que exista). Vacío = incluida en el programa.
    paths: &'static [&'static str],
}

pub const DEFAULT: &str = "Hack (incluida)";

macro_rules! paths {
    ($($p:expr),* $(,)?) => { &[$($p),*] };
}

pub const OPTIONS: &[FontOption] = &[
    // Monoespaciadas.
    FontOption { name: DEFAULT, mono: true, paths: &[] },
    FontOption { name: "Menlo", mono: true, paths: paths!["/System/Library/Fonts/Menlo.ttc"] },
    FontOption { name: "SF Mono", mono: true, paths: paths!["/System/Library/Fonts/SFNSMono.ttf"] },
    FontOption { name: "Monaco", mono: true, paths: paths!["/System/Library/Fonts/Monaco.ttf"] },
    FontOption { name: "Andale Mono", mono: true, paths: paths!["/System/Library/Fonts/Supplemental/Andale Mono.ttf"] },
    FontOption {
        name: "Courier New",
        mono: true,
        paths: paths![
            "/System/Library/Fonts/Supplemental/Courier New.ttf",
            "C:\\Windows\\Fonts\\cour.ttf",
            "/usr/share/fonts/truetype/msttcorefonts/Courier_New.ttf",
        ],
    },
    FontOption { name: "Consolas", mono: true, paths: paths!["C:\\Windows\\Fonts\\consola.ttf"] },
    FontOption { name: "Cascadia Mono", mono: true, paths: paths!["C:\\Windows\\Fonts\\CascadiaMono.ttf"] },
    FontOption { name: "DejaVu Sans Mono", mono: true, paths: paths!["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"] },
    FontOption { name: "Ubuntu Mono", mono: true, paths: paths!["/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf"] },
    FontOption {
        name: "Liberation Mono",
        mono: true,
        paths: paths!["/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf"],
    },
    // Proporcionales.
    FontOption { name: "Ubuntu (incluida)", mono: false, paths: &[] },
    FontOption { name: "SF Pro", mono: false, paths: paths!["/System/Library/Fonts/SFNS.ttf"] },
    FontOption { name: "Helvetica", mono: false, paths: paths!["/System/Library/Fonts/Helvetica.ttc"] },
    FontOption { name: "Avenir", mono: false, paths: paths!["/System/Library/Fonts/Avenir.ttc"] },
    FontOption {
        name: "Arial",
        mono: false,
        paths: paths!["/System/Library/Fonts/Supplemental/Arial.ttf", "C:\\Windows\\Fonts\\arial.ttf"],
    },
    FontOption {
        name: "Verdana",
        mono: false,
        paths: paths!["/System/Library/Fonts/Supplemental/Verdana.ttf", "C:\\Windows\\Fonts\\verdana.ttf"],
    },
    FontOption {
        name: "Georgia",
        mono: false,
        paths: paths!["/System/Library/Fonts/Supplemental/Georgia.ttf", "C:\\Windows\\Fonts\\georgia.ttf"],
    },
    FontOption { name: "Segoe UI", mono: false, paths: paths!["C:\\Windows\\Fonts\\segoeui.ttf"] },
    FontOption { name: "DejaVu Sans", mono: false, paths: paths!["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"] },
];

impl FontOption {
    fn file(&self) -> Option<&'static str> {
        self.paths.iter().copied().find(|p| std::path::Path::new(p).exists())
    }

    fn builtin(&self) -> bool {
        self.paths.is_empty()
    }
}

/// Fuentes que se pueden usar en este equipo.
pub fn available() -> Vec<&'static FontOption> {
    OPTIONS.iter().filter(|o| o.builtin() || o.file().is_some()).collect()
}

pub fn find(name: &str) -> Option<&'static FontOption> {
    OPTIONS.iter().find(|o| o.name == name)
}

pub const TEXT_FAMILY: &str = "texto";

pub fn text_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(TEXT_FAMILY.into()))
}

/// Fuentes de respaldo (CJK, etc.) que haya en el sistema; se leen una vez.
pub fn load_fallbacks() -> Vec<(String, Arc<FontData>)> {
    const CANDIDATES: &[&str] = &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "C:\\Windows\\Fonts\\msgothic.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
    ];
    CANDIDATES
        .iter()
        .enumerate()
        .filter_map(|(i, p)| std::fs::read(p).ok().map(|b| (format!("respaldo-{i}"), Arc::new(FontData::from_owned(b)))))
        .collect()
}

/// Definiciones de fuentes con `choice` como fuente del texto. Si no se puede
/// cargar, se usa la predeterminada (y se devuelve `false`).
pub fn build(choice: &str, fallbacks: &[(String, Arc<FontData>)]) -> (FontDefinitions, bool) {
    let mut defs = FontDefinitions::default();
    for (name, data) in fallbacks {
        defs.font_data.insert(name.clone(), data.clone());
        for fam in [FontFamily::Monospace, FontFamily::Proportional] {
            defs.families.entry(fam).or_default().push(name.clone());
        }
    }
    let mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let opt = find(choice).unwrap_or(&OPTIONS[0]);
    let loaded = opt.file().and_then(|p| std::fs::read(p).ok());
    let mut ok = true;
    let family = match loaded {
        Some(bytes) => {
            defs.font_data.insert("elegida".into(), Arc::new(FontData::from_owned(bytes)));
            // Detrás, las de su mismo tipo (para caracteres que no tenga).
            std::iter::once("elegida".to_string()).chain(if opt.mono { mono } else { prop }).collect()
        }
        None if opt.builtin() => {
            if opt.mono {
                mono
            } else {
                prop
            }
        }
        None => {
            ok = false;
            mono
        }
    };
    defs.families.insert(FontFamily::Name(TEXT_FAMILY.into()), family);
    (defs, ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_always_available() {
        let av = available();
        assert!(av.iter().any(|o| o.name == DEFAULT));
        assert!(av.iter().any(|o| !o.mono));
        let (defs, ok) = build("no existe", &[]);
        assert!(ok, "un nombre desconocido usa la predeterminada");
        assert!(defs.families.contains_key(&FontFamily::Name(TEXT_FAMILY.into())));
    }
}

/// Nombre para mostrar: «(incluida)» traducido (el nombre guardado no cambia).
pub fn label(name: &str) -> String {
    name.replace("(incluida)", &format!("({})", t!("incluida")))
}
