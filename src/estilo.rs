//! Aspecto común de buscador, conector y peticiones: tipografía, esquinas,
//! espaciado y color de acento; ventana de «Ajustes» (mismo marco, mismas filas
//! de idioma y tema, cambios que se aplican y se guardan solos) y ventana
//! «Acerca de».
//!
//! ARCHIVO COMÚN (`utilerias/comun`): no se edita aquí, sino en `comun/` y se
//! copia con `comun/sincronizar.sh`. Lo propio de cada aplicación está en
//! `crate::APP` (main.rs).
#![allow(dead_code)]

use eframe::egui::{
    Align, Align2, Color32, ColorImage, Context, CornerRadius, FontFamily, FontId, Frame, Grid, Id, Image, Label, Layout,
    Modal, RichText, ScrollArea, Stroke, TextStyle, TextureHandle, TextureOptions, Theme, ThemePreference, Ui, Window, vec2,
};

pub const GITHUB_USER: &str = "airylp";
pub const GITHUB_NAME: &str = "Airy";
pub const GITHUB_BLOG: &str = "https://decodigo.com";

/// `https://github.com/airylp/<app><rest>`.
pub fn repo_url(rest: &str) -> String {
    format!("https://github.com/{GITHUB_USER}/{}{rest}", crate::APP.nombre)
}

/// Abre una URL o una carpeta con el programa del sistema.
pub fn abrir(url: &str) {
    use std::process::{Command, Stdio};
    let mut c = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        Command::new("explorer")
    } else {
        Command::new("xdg-open")
    };
    let _ = c.arg(url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// Color de acento de la aplicación (el de su logotipo).
pub fn acento() -> Color32 {
    let [r, g, b] = crate::APP.color;
    Color32::from_rgb(r, g, b)
}

/// Tipografía y retoques comunes a los dos temas. Llamar una vez al crear la app.
pub fn aplicar(ctx: &Context) {
    let [r, g, b] = crate::APP.color;
    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |s| {
            // Misma letra (la Ubuntu que trae egui) y mismos tamaños en las tres.
            s.text_styles = [
                (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
                (TextStyle::Heading, FontId::new(19.0, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
            ]
            .into();
            let w = &mut s.visuals.widgets;
            for st in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                st.corner_radius = CornerRadius::same(5);
            }
            s.visuals.window_corner_radius = CornerRadius::same(10);
            s.visuals.menu_corner_radius = CornerRadius::same(8);
            s.spacing.button_padding = vec2(6.0, 2.0);
            s.spacing.item_spacing = vec2(8.0, 5.0);
            // Acento: selección, casillas marcadas y enlaces con el color de la app.
            let dark = theme == Theme::Dark;
            let k = |v: u8, f: f32| (v as f32 * f).round().min(255.0) as u8;
            let mix_white = |v: u8, f: f32| (v as f32 + (255.0 - v as f32) * f).round() as u8;
            let sel = if dark { Color32::from_rgb(k(r, 0.55), k(g, 0.55), k(b, 0.55)) } else { Color32::from_rgb(mix_white(r, 0.62), mix_white(g, 0.62), mix_white(b, 0.62)) };
            s.visuals.selection.bg_fill = sel;
            s.visuals.selection.stroke = Stroke::new(1.0, if dark { Color32::from_rgb(mix_white(r, 0.55), mix_white(g, 0.55), mix_white(b, 0.55)) } else { Color32::from_rgb(k(r, 0.55), k(g, 0.55), k(b, 0.55)) });
            s.visuals.hyperlink_color = if dark { Color32::from_rgb(mix_white(r, 0.35), mix_white(g, 0.35), mix_white(b, 0.35)) } else { Color32::from_rgb(k(r, 0.8), k(g, 0.8), k(b, 0.8)) };
        });
    }
}

/// Textura del logotipo (se crea una vez y se guarda en `cache`).
pub fn logo(ctx: &Context, cache: &mut Option<TextureHandle>) -> TextureHandle {
    cache
        .get_or_insert_with(|| {
            let n = 128;
            let img = ColorImage::from_rgba_unmultiplied([n, n], &crate::icon::rgba(n));
            ctx.load_texture("logo-app", img, TextureOptions::LINEAR)
        })
        .clone()
}

/// Lo que se pidió en «Ajustes» o «Acerca de».
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accion {
    Nada,
    BuscarActualizaciones,
    Restablecer,
}

/// Datos que muestran «Ajustes» y «Acerca de» al pie.
pub struct Version<'a> {
    /// «0.1.2 (abc1234)», «compilación local»…
    pub etiqueta: &'a str,
    /// La app sabe actualizarse sola (si no, no se ofrece el botón).
    pub actualizable: bool,
}

pub fn id_ajustes() -> Id {
    Id::NULL.with("ventana_ajustes_comun")
}

/// Ventana «Ajustes» común: centrada al abrirse, mismo margen y ancho, filas
/// «etiqueta | controles» (usar `fila`) y pie con la versión, «Buscar
/// actualizaciones…» y «Restablecer». Los cambios se aplican al momento: no
/// hay botón «Aplicar».
pub fn ventana_ajustes(ctx: &Context, abierta: &mut bool, version: &Version, contenido: impl FnOnce(&mut Ui)) -> Accion {
    let recien = ctx.data_mut(|d| {
        let antes = d.get_temp::<bool>(id_ajustes().with("abierta")).unwrap_or(false);
        d.insert_temp(id_ajustes().with("abierta"), *abierta);
        *abierta && !antes
    });
    if !*abierta {
        return Accion::Nada;
    }
    let mut accion = Accion::Nada;
    let mut win = Window::new(t!("Ajustes")).id(id_ajustes()).pivot(Align2::CENTER_CENTER);
    if recien {
        win = win.current_pos(ctx.content_rect().center());
    }
    let max_h = (ctx.content_rect().height() - 230.0).max(220.0);
    win.open(abierta)
        .resizable(false)
        .collapsible(false)
        .frame(Frame::window(&ctx.global_style()).inner_margin(18.0))
        .show(ctx, |ui| {
            ui.set_width(580.0);
            ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
            ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                contenido(ui);
            });
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(tf!("Versión {v}", v = version.etiqueta));
                if version.actualizable && ui.button(t!("⟳ Buscar actualizaciones…")).clicked() {
                    accion = Accion::BuscarActualizaciones;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(t!("↺ Restablecer")).on_hover_text(t!("Vuelve a los ajustes predeterminados")).clicked() {
                        accion = Accion::Restablecer;
                    }
                });
            });
            ui.add_space(2.0);
            ui.weak(t!("Los cambios se aplican y se guardan solos."));
        });
    accion
}

/// Título de un grupo de ajustes.
pub fn seccion(ui: &mut Ui, titulo: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(titulo).heading().size(15.0).color(acento_texto(ui)));
    ui.add_space(2.0);
}

fn acento_texto(ui: &Ui) -> Color32 {
    let c = acento();
    if ui.visuals().dark_mode { c.lerp_to_gamma(Color32::WHITE, 0.25) } else { c.lerp_to_gamma(Color32::BLACK, 0.15) }
}

/// Rejilla de dos columnas para las filas de un grupo.
pub fn rejilla(ui: &mut Ui, id: &str, filas: impl FnOnce(&mut Ui)) {
    Grid::new(id).num_columns(2).spacing([24.0, 12.0]).min_col_width(150.0).show(ui, filas);
}

/// Una fila: etiqueta en negrita a la izquierda, controles a la derecha.
pub fn fila(ui: &mut Ui, etiqueta: &str, ayuda: Option<&str>, controles: impl FnOnce(&mut Ui)) {
    let r = ui.strong(etiqueta);
    if let Some(a) = ayuda {
        r.on_hover_text(a);
    }
    ui.horizontal_wrapped(controles);
    ui.end_row();
}

/// Fila «Idioma»: devuelve el elegido si cambió (cada app lo guarda a su modo).
pub fn fila_idioma(ui: &mut Ui) -> Option<crate::i18n::Lang> {
    let mut cambio = None;
    fila(ui, t!("Idioma"), Some("Idioma · Language · Langue"), |ui| {
        let cur = crate::i18n::get();
        for l in crate::i18n::Lang::ALL {
            if ui.radio(cur == l, l.name()).clicked() && cur != l {
                cambio = Some(l);
            }
        }
    });
    cambio
}

/// Fila «Tema»: cambia `tema` y lo aplica; devuelve true si cambió.
pub fn fila_tema(ui: &mut Ui, tema: &mut ThemePreference) -> bool {
    let antes = *tema;
    fila(ui, t!("Tema"), None, |ui| {
        ui.radio_value(tema, ThemePreference::System, t!("Sistema"));
        ui.radio_value(tema, ThemePreference::Light, t!("Claro"));
        ui.radio_value(tema, ThemePreference::Dark, t!("Oscuro"));
    });
    if *tema != antes {
        ui.ctx().set_theme(*tema);
        true
    } else {
        false
    }
}

/// Ventana «Acerca de» común. `extra` añade filas propias a la rejilla (usar
/// `fila_dato`). Devuelve `BuscarActualizaciones` si se pulsó ese botón.
pub fn acerca(ctx: &Context, abierta: &mut bool, logo: &TextureHandle, version: &Version, extra: impl FnOnce(&mut Ui)) -> Accion {
    if !*abierta {
        return Accion::Nada;
    }
    let mut accion = Accion::Nada;
    let mut cerrar = false;
    let link = |ui: &mut Ui, text: &str, url: &str| {
        if ui.link(text).on_hover_text(url).clicked() {
            abrir(url);
        }
    };
    let app = &crate::APP;
    let modal = Modal::new(Id::NULL.with("acerca_comun")).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.horizontal(|ui| {
            ui.add(Image::new(logo).fit_to_exact_size(vec2(72.0, 72.0)));
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(app.nombre).heading().strong().color(acento_texto(ui)));
                ui.label(tf!("Versión {v}", v = version.etiqueta));
                ui.label(RichText::new((app.descripcion)()).weak());
            });
        });
        ui.add_space(8.0);
        ui.separator();
        Grid::new("acerca-rejilla").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new(t!("Autor")).strong());
            ui.horizontal(|ui| {
                ui.label(GITHUB_NAME);
                link(ui, &format!("@{GITHUB_USER}"), &format!("https://github.com/{GITHUB_USER}"));
            });
            ui.end_row();
            ui.label(RichText::new("GitHub").strong());
            link(ui, &format!("github.com/{GITHUB_USER}/{}", app.nombre), &repo_url(""));
            ui.end_row();
            ui.label(RichText::new(t!("Sitio web")).strong());
            link(ui, GITHUB_BLOG.trim_start_matches("https://"), GITHUB_BLOG);
            ui.end_row();
            ui.label(RichText::new(t!("Licencia")).strong());
            ui.horizontal(|ui| {
                ui.label(tf!("MIT · © 2026 {user}", user = GITHUB_USER));
                link(ui, t!("ver"), &repo_url("/blob/main/LICENSE"));
            });
            ui.end_row();
            ui.label(RichText::new(t!("Hecho con")).strong());
            ui.label(app.hecho_con);
            ui.end_row();
            extra(ui);
        });
        ui.separator();
        ui.horizontal(|ui| {
            link(ui, t!("Versiones"), &repo_url("/releases"));
            ui.label("·");
            link(ui, t!("Reportar un problema"), &repo_url("/issues/new"));
            ui.label("·");
            link(ui, t!("Código fuente"), &repo_url(""));
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if version.actualizable && ui.button(t!("Buscar actualizaciones…")).clicked() {
                accion = Accion::BuscarActualizaciones;
                cerrar = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(t!("Cerrar")).clicked() {
                    cerrar = true;
                }
            });
        });
    });
    if cerrar || modal.should_close() {
        *abierta = false;
    }
    accion
}

/// Fila de «Acerca de» con una ruta y un botón para abrirla.
pub fn fila_carpeta(ui: &mut Ui, etiqueta: &str, ruta: &str) {
    ui.label(RichText::new(etiqueta).strong());
    ui.horizontal(|ui| {
        ui.add(Label::new(RichText::new(ruta).weak()).truncate());
        if !ruta.is_empty() && ui.small_button("🗁").on_hover_text(t!("Abrir carpeta")).clicked() {
            abrir(ruta);
        }
    });
    ui.end_row();
}

/// Fila de «Acerca de» con un dato.
pub fn fila_dato(ui: &mut Ui, etiqueta: &str, valor: &str) {
    ui.label(RichText::new(etiqueta).strong());
    ui.add(Label::new(valor).truncate());
    ui.end_row();
}
