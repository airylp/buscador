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

// ------------------------------------------------------------ actualizar

/// Lo que se pidió en la ventana «Actualizar».
pub enum AccionActualizar {
    Nada,
    /// Empezar a descargar/instalar (`Updater::run`).
    Ejecutar,
    /// Cerrar y abrir la versión instalada en esa ruta (`update::relaunch_after_exit`).
    Reiniciar(std::path::PathBuf),
}

pub fn id_actualizar() -> Id {
    Id::NULL.with("ventana_actualizar_comun")
}

/// Ventana «Actualizar <app>», común a las tres: versión instalada, estado de la
/// comprobación, botón para instalar, registro y «Reiniciar ahora». `nota`
/// se muestra tras instalar (p. ej. «hay cambios sin guardar»).
pub fn ventana_actualizar(ctx: &Context, u: &crate::update::Updater, abierta: &mut bool, nota: Option<&str>) -> AccionActualizar {
    use crate::icons::{Icon, IconButton, Tone};
    use crate::update::Stage;
    let stage = u.stage();
    let mut accion = AccionActualizar::Nada;
    let app = crate::APP.nombre;
    Window::new(tf!("Actualizar {app}", app = app))
        .id(id_actualizar())
        .open(abierta)
        .collapsible(false)
        .resizable(true)
        .default_size([560.0, 300.0])
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .frame(Frame::window(&ctx.global_style()).inner_margin(16.0))
        .show(ctx, |ui| {
            ui.label(tf!("Versión instalada: {v}", v = crate::update::version_label()));
            ui.add_space(6.0);
            let ok = Color32::from_rgb(40, 170, 80);
            match &stage {
                Stage::Checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t!("Buscando la última versión en GitHub…"));
                    });
                }
                Stage::UpToDate(v) => {
                    ui.colored_label(ok, tf!("✔ Ya tienes la última versión ({v}).", v = v));
                    if ui.add(IconButton::new(Icon::Reload, Tone::Neutral).label(t!("Reinstalar de todos modos"))).clicked() {
                        accion = AccionActualizar::Ejecutar;
                    }
                }
                Stage::Available(v) => {
                    ui.strong(tf!("Hay una versión nueva: {v}", v = v));
                    ui.label(if crate::update::WINDOWS {
                        tf!("Se descarga el instalador; al reiniciar, {app} se cierra, se instala la versión nueva y vuelve a abrirse.", app = app)
                    } else if crate::update::COMPILES {
                        tf!("Se descarga el código, se compila (la primera vez tarda unos minutos) y se reemplaza la app instalada. Puedes seguir usando {app} mientras tanto.", app = app)
                    } else {
                        tf!("Se descarga la versión publicada y se reemplaza la app instalada. Puedes seguir usando {app} mientras tanto.", app = app)
                    });
                    ui.add_space(4.0);
                    if ui.add(IconButton::new(Icon::Download, Tone::Live).label(t!("Actualizar ahora"))).clicked() {
                        accion = AccionActualizar::Ejecutar;
                    }
                }
                Stage::Running => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t!("Actualizando… (puedes seguir trabajando)"));
                    });
                }
                Stage::Done(p) => {
                    ui.colored_label(ok, t!("✔ Versión nueva instalada."));
                    ui.horizontal(|ui| {
                        if ui.add(IconButton::new(Icon::Reload, Tone::Live).label(t!("Reiniciar ahora"))).clicked() {
                            accion = AccionActualizar::Reiniciar(p.clone());
                        }
                        ui.weak(tf!("o úsala la próxima vez que abras {app}", app = app));
                    });
                    if let Some(n) = nota {
                        ui.weak(n);
                    }
                }
                Stage::Failed(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, tf!("No se pudo actualizar: {e}", e = e));
                    if ui.add(IconButton::new(Icon::Reload, Tone::Neutral).label(t!("Reintentar"))).clicked() {
                        accion = AccionActualizar::Ejecutar;
                    }
                }
            }
            let log = u.log.lock().unwrap();
            if !log.is_empty() {
                ui.add_space(6.0);
                ui.weak(t!("Registro:"));
                ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, true]).max_height(220.0).show(ui, |ui| {
                    for l in log.iter() {
                        ui.label(RichText::new(l).monospace().small());
                    }
                });
            }
        });
    accion
}

/// Comprobación de versión nueva en segundo plano: 5 s después de abrir y luego
/// cada 6 horas, solo si la app sabe qué versión es (no en compilaciones locales).
pub struct Vigia {
    inicio: std::time::Instant,
    ultima: Option<std::time::Instant>,
    pub comprobacion: Option<crate::update::Updater>,
}

impl Default for Vigia {
    fn default() -> Self {
        Vigia { inicio: std::time::Instant::now(), ultima: None, comprobacion: None }
    }
}

impl Vigia {
    pub fn tick(&mut self, ctx: &Context, activa: bool) {
        use std::time::Duration;
        const PRIMERA: Duration = Duration::from_secs(5);
        const CADA: Duration = Duration::from_secs(6 * 3600);
        let conocida = crate::update::current_version().is_some() || crate::update::current_commit().is_some();
        if !crate::update::SUPPORTED || !activa || !conocida {
            return;
        }
        let toca = match self.ultima {
            None => self.inicio.elapsed() >= PRIMERA,
            Some(t) => t.elapsed() >= CADA,
        };
        if toca {
            self.ultima = Some(std::time::Instant::now());
            self.comprobacion = Some(crate::update::Updater::check(ctx.clone()));
        } else if self.ultima.is_none() {
            ctx.request_repaint_after(PRIMERA.saturating_sub(self.inicio.elapsed()) + Duration::from_millis(50));
        }
    }

    /// Versión nueva encontrada (en segundo plano o en la ventana `otra`).
    pub fn nueva(&self, otra: Option<&crate::update::Updater>) -> Option<String> {
        [otra, self.comprobacion.as_ref()].into_iter().flatten().find_map(|u| match u.stage() {
            crate::update::Stage::Available(v) => Some(v),
            _ => None,
        })
    }
}

/// Botón ⬇ de la barra superior, con un punto verde si hay versión nueva.
pub fn boton_actualizar(ui: &mut Ui, abierta: bool, nueva: Option<&str>) -> bool {
    use crate::icons::{Icon, IconButton, Tone};
    let hint = match nueva {
        Some(v) => tf!("Hay una versión nueva ({v}): clic para actualizar", v = v),
        None => tf!("Buscar actualizaciones · versión {v}", v = crate::update::version_label()),
    };
    let r = ui.add(IconButton::new(Icon::Download, Tone::Live).selected(abierta)).on_hover_text(hint);
    if nueva.is_some() {
        crate::icons::badge(ui, &r, Color32::from_rgb(40, 190, 80));
    }
    r.clicked()
}

// ------------------------------------------------------------ atajos

/// «Ctrl» → «Cmd» en macOS.
pub fn tecla(s: &str) -> String {
    if cfg!(target_os = "macos") { s.replace("Ctrl+", "Cmd+") } else { s.to_string() }
}

/// Atajos comunes a las tres: Ctrl+, (Ajustes) y F1 (atajos de teclado).
/// Devuelve (ajustes, atajos) si se pulsaron.
pub fn atajos_comunes(ctx: &Context) -> (bool, bool) {
    use eframe::egui::{Key, Modifiers};
    ctx.input_mut(|i| (i.consume_key(Modifiers::COMMAND, Key::Comma), i.consume_key(Modifiers::NONE, Key::F1)))
}

pub fn id_atajos() -> Id {
    Id::NULL.with("ventana_atajos_comun")
}

/// Ventana «Atajos de teclado» común: primero los atajos de las tres y luego
/// las secciones propias de la app (`(sección, [(tecla, qué hace)])`).
pub fn ventana_atajos(ctx: &Context, abierta: &mut bool, propias: &[(&str, Vec<(String, &str)>)]) {
    if !*abierta {
        return;
    }
    let comunes: Vec<(String, &str)> = vec![
        (tecla("Ctrl+,"), t!("Ajustes")),
        ("F1".into(), t!("Atajos de teclado")),
        ("Escape".into(), t!("Cerrar la ventana de delante")),
    ];
    Window::new(t!("Atajos de teclado"))
        .id(id_atajos())
        .open(abierta)
        .collapsible(false)
        .resizable(true)
        .default_size([520.0, 520.0])
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                let sec = |ui: &mut Ui, titulo: &str, filas: &[(String, &str)]| {
                    seccion(ui, titulo);
                    Grid::new(("atajos", titulo)).num_columns(2).striped(true).spacing([16.0, 4.0]).min_col_width(200.0).show(ui, |ui| {
                        for (k, que) in filas {
                            ui.label(RichText::new(k).monospace());
                            ui.label(*que);
                            ui.end_row();
                        }
                    });
                };
                sec(ui, t!("General"), &comunes);
                for (titulo, filas) in propias {
                    sec(ui, titulo, filas);
                }
            });
        });
}

// ------------------------------------------------------------ barra de estado

/// Parte derecha de la barra de estado, igual en las tres: ? (atajos) e ℹ (acerca de).
pub fn barra_estado_derecha(ui: &mut Ui, atajos: &mut bool, acerca: &mut bool) {
    use crate::icons::{Icon, IconButton, Tone};
    if ui.add(IconButton::new(Icon::Info, Tone::Neutral).small().selected(*acerca)).on_hover_text(tf!("Acerca de {app}", app = crate::APP.nombre)).clicked() {
        *acerca = !*acerca;
    }
    if ui.add(IconButton::new(Icon::Help, Tone::Neutral).small().selected(*atajos)).on_hover_text(t!("Atajos de teclado (F1)")).clicked() {
        *atajos = !*atajos;
    }
}

/// Barra de estado común: `izquierda` (lo propio de la app) y, a la derecha,
/// atajos y «Acerca de».
pub fn barra_estado(ui: &mut Ui, atajos: &mut bool, acerca: &mut bool, izquierda: impl FnOnce(&mut Ui)) {
    use eframe::egui::Panel;
    Panel::bottom("barra_estado_comun").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                barra_estado_derecha(ui, atajos, acerca);
                ui.separator();
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.set_clip_rect(ui.max_rect());
                    izquierda(ui);
                });
            });
        });
    });
}

// ------------------------------------------------------------ avisos

/// Tipo de aviso: color e ícono.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tipo {
    Info,
    Exito,
    Aviso,
    Error,
}

/// Avisos flotantes (abajo a la derecha) que se van solos: el mismo estilo de
/// mensajes en las tres aplicaciones. Clic para cerrarlos antes.
#[derive(Default)]
pub struct Avisos {
    lista: Vec<(Tipo, String, std::time::Instant)>,
}

impl Avisos {
    pub fn push(&mut self, tipo: Tipo, texto: impl Into<String>) {
        let texto = texto.into();
        if texto.trim().is_empty() {
            return;
        }
        // El mismo texto otra vez: solo se renueva.
        self.lista.retain(|(_, t, _)| *t != texto);
        self.lista.push((tipo, texto, std::time::Instant::now()));
        if self.lista.len() > 4 {
            self.lista.remove(0);
        }
    }

    /// Elige el tipo por el principio del texto: ✔ éxito, ⚠ aviso, ✖ o «error» error.
    pub fn auto(&mut self, texto: impl Into<String>) {
        let texto: String = texto.into();
        let t = texto.trim_start();
        let low = t.to_lowercase();
        let tipo = if t.starts_with('✔') {
            Tipo::Exito
        } else if t.starts_with('⚠') {
            Tipo::Aviso
        } else if t.starts_with('✖') || low.starts_with("error") || low.contains(t!("no se pudo").to_lowercase().as_str()) {
            Tipo::Error
        } else {
            Tipo::Info
        };
        let limpio = t.trim_start_matches(['✔', '⚠', '✖']).trim_start().to_string();
        self.push(tipo, limpio);
    }

    pub fn info(&mut self, t: impl Into<String>) {
        self.push(Tipo::Info, t);
    }
    pub fn exito(&mut self, t: impl Into<String>) {
        self.push(Tipo::Exito, t);
    }
    pub fn error(&mut self, t: impl Into<String>) {
        self.push(Tipo::Error, t);
    }

    pub fn ultimo(&self) -> Option<&str> {
        self.lista.last().map(|(_, t, _)| t.as_str())
    }

    /// Pinta los avisos vigentes (5 s; los errores, 10 s).
    pub fn mostrar(&mut self, ctx: &Context) {
        use crate::icons::{Icon, paint};
        use eframe::egui::{Area, Order, Sense};
        use std::time::Duration;
        let dura = |t: Tipo| if t == Tipo::Error { Duration::from_secs(10) } else { Duration::from_secs(5) };
        self.lista.retain(|(t, _, at)| at.elapsed() < dura(*t));
        if self.lista.is_empty() {
            return;
        }
        let mut cerrar = None;
        Area::new(Id::NULL.with("avisos_comunes")).order(Order::Foreground).anchor(Align2::RIGHT_BOTTOM, vec2(-14.0, -34.0)).interactable(true).show(ctx, |ui| {
            ui.set_max_width(420.0);
            // De arriba abajo (el más reciente, abajo): el área se ancla por su esquina inferior derecha.
            ui.with_layout(Layout::top_down(Align::Max), |ui| {
                for (i, (tipo, texto, _)) in self.lista.iter().enumerate() {
                    let (color, icono) = match tipo {
                        Tipo::Info => (Color32::from_rgb(0x2f, 0x7c, 0xf6), Icon::Info),
                        Tipo::Exito => (Color32::from_rgb(0x1f, 0xb3, 0x55), Icon::Check),
                        Tipo::Aviso => (Color32::from_rgb(0xe8, 0xa8, 0x1e), Icon::Warning),
                        Tipo::Error => (Color32::from_rgb(0xe5, 0x48, 0x4d), Icon::Error),
                    };
                    let v = ui.visuals().clone();
                    let r = Frame::new()
                        .fill(v.window_fill)
                        .stroke(Stroke::new(1.0, color.gamma_multiply(0.8)))
                        .corner_radius(8)
                        .inner_margin(vec2(10.0, 8.0))
                        .shadow(v.popup_shadow)
                        .show(ui, |ui| {
                            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                                let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                                paint(ui.painter(), rect, icono);
                                ui.add(Label::new(texto.as_str()).wrap());
                            });
                        })
                        .response
                        .interact(Sense::click());
                    if r.on_hover_text(t!("Clic para cerrar")).clicked() {
                        cerrar = Some(i);
                    }
                    if i + 1 < self.lista.len() {
                        ui.add_space(6.0);
                    }
                }
            });
        });
        if let Some(i) = cerrar {
            self.lista.remove(i);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

// ------------------------------------------------------------ bienvenida

/// Pantalla de bienvenida común (sin nada abierto): logotipo grande, nombre,
/// descripción y las acciones principales. Devuelve el índice de la pulsada.
pub fn bienvenida(ui: &mut Ui, logo: &TextureHandle, acciones: &[(crate::icons::Icon, &str, &str)], pie: Option<&str>) -> Option<usize> {
    use crate::icons::{IconButton, Tone};
    let mut pulsada = None;
    let alto = 150.0 + 40.0 * acciones.len() as f32;
    let arriba = ((ui.available_height() - alto) / 2.0 - 20.0).max(10.0);
    ui.vertical_centered(|ui| {
        ui.add_space(arriba);
        ui.add(Image::new(logo).fit_to_exact_size(vec2(96.0, 96.0)));
        ui.add_space(6.0);
        ui.label(RichText::new(crate::APP.nombre).size(26.0).strong().color(acento_texto(ui)));
        ui.label(RichText::new((crate::APP.descripcion)()).weak());
        ui.add_space(16.0);
        for (i, (icono, texto, ayuda)) in acciones.iter().enumerate() {
            let r = ui.add(IconButton::new(*icono, Tone::File).label(*texto).framed());
            let r = if ayuda.is_empty() { r } else { r.on_hover_text(*ayuda) };
            if r.clicked() {
                pulsada = Some(i);
            }
            ui.add_space(4.0);
        }
        if let Some(p) = pie {
            ui.add_space(10.0);
            ui.weak(p);
        }
    });
    pulsada
}

/// Etiqueta con un ícono delante (en lugar de un emoji), para barras de estado.
pub fn etiqueta(ui: &mut Ui, icono: crate::icons::Icon, texto: impl Into<eframe::egui::WidgetText>) -> eframe::egui::Response {
    use eframe::egui::Sense;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        crate::icons::paint(ui.painter(), rect, icono);
        ui.add(Label::new(texto).sense(Sense::click()))
    })
    .inner
}
