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

/// Abre la ventana principal. Primero con wgpu (DirectX 12 / Vulkan / Metal); si no
/// arranca (máquinas virtuales, Escritorio remoto, controladores viejos) la app se
/// relanza con OpenGL (winit no deja crear un segundo bucle de eventos en el mismo
/// proceso). Sin consola (Windows) un fallo pasaría en silencio: se muestra en un
/// cuadro de diálogo y se guarda en `<config>/error.log`.
pub fn arrancar(nombre: &str, mut options: eframe::NativeOptions, app: eframe::AppCreator<'static>) -> eframe::Result {
    let var = format!("{}_GLOW", crate::APP.nombre.to_uppercase());
    let previo = std::env::var(&var).ok().filter(|v| !v.is_empty());
    let glow = previo.is_some();
    options.renderer = if glow { eframe::Renderer::Glow } else { eframe::Renderer::Wgpu };
    std::panic::set_hook(Box::new(|info| error_fatal(&info.to_string())));
    // Windows: si junto al .exe va Mesa (opengl32.dll), que dibuje con el procesador
    // (llvmpipe) y no intente su controlador de DirectX 12, que ahí no habría.
    #[cfg(windows)]
    if std::env::var_os("GALLIUM_DRIVER").is_none() {
        // SAFETY: al arrancar, antes de crear hilos.
        unsafe { std::env::set_var("GALLIUM_DRIVER", "llvmpipe") };
    }
    let r = eframe::run_native(nombre, options, app);
    if let Err(e) = &r {
        if !glow {
            let otro = std::env::current_exe().and_then(|exe| {
                std::process::Command::new(exe).args(std::env::args_os().skip(1)).env(&var, e.to_string()).spawn()
            });
            if otro.is_ok() {
                return Ok(());
            }
        }
        error_fatal(&match previo.filter(|p| p != "1") {
            Some(w) => format!("wgpu: {w}\nOpenGL: {e}"),
            None => e.to_string(),
        });
    }
    r
}

/// Informa de un error que impide seguir (consola, `<config>/error.log` y, en
/// Windows, un cuadro de diálogo).
pub fn error_fatal(msg: &str) {
    let app = crate::APP.nombre;
    eprintln!("{app}: {msg}");
    let env = |k| std::env::var_os(k).filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        env("APPDATA")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    };
    if let Some(dir) = base.map(|b| b.join(app)) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("error.log"), format!("{app} {}\n{msg}\n", env!("CARGO_PKG_VERSION")));
    }
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(hwnd: *mut std::ffi::c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
        }
        let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (text, caption) = (w(msg), w(app));
        // SAFETY: cadenas UTF-16 terminadas en 0 que viven durante la llamada.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10) };
    }
}

/// Color de acento de la aplicación (el de su logotipo).
pub fn acento() -> Color32 {
    let [r, g, b] = crate::APP.color;
    Color32::from_rgb(r, g, b)
}

/// Colores de base al estilo de Tailwind (grises «zinc»): todo es una
/// constante de color; no cuesta memoria porque egui dibuja cada fotograma.
#[derive(Clone, Copy)]
pub struct Paleta {
    /// Fondo de los paneles.
    pub fondo: Color32,
    /// Fondo de tarjetas y ventanas.
    pub tarjeta: Color32,
    /// Fondo de campos de texto y bloques de código.
    pub campo: Color32,
    pub codigo: Color32,
    pub borde: Color32,
    pub borde_fuerte: Color32,
    pub texto: Color32,
    pub apagado: Color32,
    /// Fondo de la zona de trabajo (panel central); igual a `fondo` salvo en «Grafito claro».
    pub central: Color32,
}

/// «Grafito claro»: variante del tema claro con paneles grises y bordes marcados.
static GRAFITO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Activa o quita «Grafito claro» (cambia los colores del tema claro).
pub fn grafito(ctx: &Context, activo: bool) {
    if GRAFITO.swap(activo, std::sync::atomic::Ordering::Relaxed) != activo {
        ctx.style_mut_of(Theme::Light, |s| paleta_tailwind(&mut s.visuals, false));
    }
}

pub fn es_grafito() -> bool {
    GRAFITO.load(std::sync::atomic::Ordering::Relaxed)
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl Paleta {
    pub const CLARA: Paleta = Paleta {
        fondo: hex(0xfafafa),        // zinc-50
        tarjeta: hex(0xffffff),      // white
        campo: hex(0xffffff),        // white
        codigo: hex(0xf4f4f5),       // zinc-100
        borde: hex(0xe4e4e7),        // zinc-200
        borde_fuerte: hex(0xd4d4d8), // zinc-300
        texto: hex(0x18181b),        // zinc-900
        apagado: hex(0x71717a),      // zinc-500
        central: hex(0xfafafa),
    };
    pub const GRAFITO: Paleta = Paleta {
        fondo: hex(0xd9d9de),        // paneles laterales y barras: gris grafito claro
        tarjeta: hex(0xf7f7f8),
        campo: hex(0xffffff),
        codigo: hex(0xe9e9ec),
        borde: hex(0xa8a8b0),        // separaciones bien visibles
        borde_fuerte: hex(0x8a8a93),
        texto: hex(0x18181b),
        apagado: hex(0x55555d),
        central: hex(0xf1f1f3),      // zona de trabajo más clara que los paneles
    };
    pub const OSCURA: Paleta = Paleta {
        fondo: hex(0x18181b),        // zinc-900
        tarjeta: hex(0x1f1f23),      // entre zinc-900 y zinc-800
        campo: hex(0x0f0f11),        // casi zinc-950
        codigo: hex(0x111113),
        borde: hex(0x2e2e33),        // zinc-800 algo más claro
        borde_fuerte: hex(0x3f3f46), // zinc-700
        texto: hex(0xf4f4f5),        // zinc-100
        apagado: hex(0xa1a1aa),      // zinc-400
        central: hex(0x18181b),
    };

    pub fn de(ui: &Ui) -> Paleta {
        Paleta::del_tema(ui.visuals().dark_mode)
    }

    pub fn del_tema(dark: bool) -> Paleta {
        if dark {
            Paleta::OSCURA
        } else if es_grafito() {
            Paleta::GRAFITO
        } else {
            Paleta::CLARA
        }
    }
}

/// Fondos, bordes, sombras y estados de los controles con la paleta.
fn paleta_tailwind(v: &mut eframe::egui::Visuals, dark: bool) {
    use eframe::egui::Shadow;
    let p = Paleta::del_tema(dark);
    let g = !dark && es_grafito();
    v.panel_fill = p.fondo;
    v.window_fill = p.tarjeta;
    v.extreme_bg_color = p.campo;
    v.text_edit_bg_color = Some(p.campo);
    v.code_bg_color = p.codigo;
    v.faint_bg_color = if dark { hex(0x1c1c20) } else if g { hex(0xe6e6ea) } else { hex(0xf4f4f5) };
    v.window_stroke = Stroke::new(1.0, p.borde);
    // Sombras cortas y suaves (las grandes cuestan más triángulos por fotograma).
    v.window_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if dark { 90 } else { 28 }) };
    v.popup_shadow = Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(if dark { 80 } else { 22 }) };
    v.override_text_color = None;
    let w = &mut v.widgets;
    // Sin interacción (marcos, separadores).
    w.noninteractive.bg_fill = p.fondo;
    w.noninteractive.weak_bg_fill = p.fondo;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.borde);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.texto);
    // Botones y casillas en reposo: como un botón «secundario» (fondo de tarjeta y borde).
    let (inactivo, encima, pulsado) = if dark {
        (hex(0x27272a), hex(0x323238), hex(0x3f3f46))
    } else if g {
        (hex(0xf7f7f8), hex(0xe9e9ec), hex(0xd4d4d8))
    } else {
        (hex(0xffffff), hex(0xf4f4f5), hex(0xe4e4e7))
    };
    w.inactive.weak_bg_fill = inactivo;
    w.inactive.bg_fill = inactivo;
    w.inactive.bg_stroke = Stroke::new(1.0, p.borde);
    w.inactive.fg_stroke = Stroke::new(1.0, p.texto);
    w.hovered.weak_bg_fill = encima;
    w.hovered.bg_fill = encima;
    w.hovered.bg_stroke = Stroke::new(1.0, p.borde_fuerte);
    w.hovered.fg_stroke = Stroke::new(1.5, p.texto);
    w.hovered.expansion = 0.0;
    w.active.weak_bg_fill = pulsado;
    w.active.bg_fill = pulsado;
    w.active.bg_stroke = Stroke::new(1.0, p.borde_fuerte);
    w.active.fg_stroke = Stroke::new(1.5, p.texto);
    w.active.expansion = 0.0;
    w.open.weak_bg_fill = encima;
    w.open.bg_fill = encima;
    w.open.bg_stroke = Stroke::new(1.0, p.borde_fuerte);
    w.open.fg_stroke = Stroke::new(1.0, p.texto);
}

// ---------------------------------------------------------------- componentes

/// Tarjeta con título pequeño en mayúsculas (como `rounded-lg border p-3`).
pub fn tarjeta(ui: &mut Ui, titulo: &str, contenido: impl FnOnce(&mut Ui)) {
    let p = Paleta::de(ui);
    Frame::new().fill(p.tarjeta).stroke(Stroke::new(1.0, p.borde)).corner_radius(10.0).inner_margin(eframe::egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if !titulo.is_empty() {
            ui.label(RichText::new(titulo.to_uppercase()).size(10.5).strong().color(p.apagado).extra_letter_spacing(0.8));
            ui.add_space(8.0);
        }
        contenido(ui);
    });
    ui.add_space(10.0);
}

/// Campo de solo lectura: etiqueta pequeña y apagada encima del valor (seleccionable).
pub fn campo(ui: &mut Ui, etiqueta: &str, valor: &str) {
    let p = Paleta::de(ui);
    ui.label(RichText::new(etiqueta).small().color(p.apagado));
    ui.add(Label::new(RichText::new(valor).color(p.texto)).wrap().selectable(true));
    ui.add_space(6.0);
}

/// Texto en monoespaciada sobre fondo gris (como `<code>`).
pub fn codigo(ui: &mut Ui, texto: &str) {
    let p = Paleta::de(ui);
    Frame::new().fill(p.codigo).corner_radius(6.0).inner_margin(eframe::egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(Label::new(RichText::new(texto).monospace().size(11.5).color(p.texto)).wrap().selectable(true));
    });
}

/// Insignia redondeada con fondo muy suave de su color (`rounded-full bg-x-100 text-x-700`).
pub fn insignia(ui: &mut Ui, texto: &str, color: Color32) -> eframe::egui::Response {
    Frame::new()
        .fill(color.gamma_multiply(if ui.visuals().dark_mode { 0.22 } else { 0.14 }))
        .corner_radius(9.0)
        .inner_margin(eframe::egui::Margin::symmetric(8, 2))
        // Recortada si no cabe (p. ej. en una barra que se queda sin sitio).
        .show(ui, |ui| ui.add(Label::new(RichText::new(texto).small().strong().color(color)).truncate()))
        .inner
}

/// Etiqueta con borde (nombres, valores cortos).
pub fn chip(ui: &mut Ui, texto: &str) {
    let p = Paleta::de(ui);
    Frame::new().fill(p.codigo).stroke(Stroke::new(1.0, p.borde)).corner_radius(6.0).inner_margin(eframe::egui::Margin::symmetric(6, 2)).show(ui, |ui| {
        ui.add(Label::new(RichText::new(texto).monospace().size(11.5).color(p.texto)).selectable(true));
    });
}

/// Colores de los avisos: (texto e ícono, fondo suave).
pub fn color_tipo(ui: &Ui, tipo: Tipo) -> Color32 {
    let dark = ui.visuals().dark_mode;
    match (tipo, dark) {
        (Tipo::Info, false) => hex(0x2563eb),  // blue-600
        (Tipo::Info, true) => hex(0x60a5fa),   // blue-400
        (Tipo::Exito, false) => hex(0x16a34a), // green-600
        (Tipo::Exito, true) => hex(0x4ade80),  // green-400
        (Tipo::Aviso, false) => hex(0xd97706), // amber-600
        (Tipo::Aviso, true) => hex(0xfbbf24),  // amber-400
        (Tipo::Error, false) => hex(0xdc2626), // red-600
        (Tipo::Error, true) => hex(0xf87171),  // red-400
    }
}

/// Recuadro de alerta: fondo suave, raya a la izquierda e ícono (`border-l-4 bg-x-50`).
pub fn alerta(ui: &mut Ui, tipo: Tipo, texto: &str) {
    alerta_con(ui, tipo, |ui| {
        ui.add(Label::new(RichText::new(texto).color(Paleta::de(ui).texto)).wrap().selectable(true));
    });
}

/// Alerta con contenido propio (p. ej. texto y un campo para escribir).
pub fn alerta_con(ui: &mut Ui, tipo: Tipo, contenido: impl FnOnce(&mut Ui)) {
    let c = color_tipo(ui, tipo);
    let icono = match tipo {
        Tipo::Info => crate::icons::Icon::Info,
        Tipo::Exito => crate::icons::Icon::Check,
        Tipo::Aviso => crate::icons::Icon::Warning,
        Tipo::Error => crate::icons::Icon::Error,
    };
    let r = Frame::new().fill(c.gamma_multiply(if ui.visuals().dark_mode { 0.14 } else { 0.08 })).corner_radius(6.0).inner_margin(eframe::egui::Margin { left: 12, right: 10, top: 8, bottom: 8 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), eframe::egui::Sense::hover());
            crate::icons::paint(ui.painter(), rect, icono);
            ui.vertical(contenido);
        });
    });
    let rect = r.response.rect;
    ui.painter().rect_filled(eframe::egui::Rect::from_min_max(rect.min, eframe::egui::pos2(rect.min.x + 3.0, rect.max.y)), CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, c);
}

/// Botón de una acción destructiva (borrar): rojo con letra blanca.
pub fn boton_peligro(ui: &Ui, texto: impl Into<String>) -> eframe::egui::Button<'static> {
    let c = color_tipo(ui, Tipo::Error);
    let c = if ui.visuals().dark_mode { c.lerp_to_gamma(Color32::BLACK, 0.25) } else { c };
    eframe::egui::Button::new(RichText::new(texto.into()).color(Color32::WHITE).strong()).fill(c).stroke(Stroke::new(1.0, c.lerp_to_gamma(Color32::BLACK, 0.2))).corner_radius(6.0)
}

/// Fila de una lista a todo lo ancho: fondo al pasar el ratón; la elegida con
/// tono del acento y una raya a la izquierda. Se puede pulsar en cualquier parte.
pub fn fila_lista(ui: &mut Ui, elegida: bool, texto: impl Into<eframe::egui::WidgetText>) -> eframe::egui::Response {
    use eframe::egui::{Sense, TextWrapMode, pos2};
    let g = texto.into().into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Body);
    let w = ui.available_width().max(g.size().x + 20.0);
    let (rect, r) = ui.allocate_exact_size(vec2(w, g.size().y + 8.0), Sense::click());
    let v = ui.visuals();
    if elegida {
        ui.painter().rect_filled(rect, 6.0, v.selection.bg_fill.gamma_multiply(if v.dark_mode { 0.7 } else { 0.55 }));
        ui.painter().rect_filled(eframe::egui::Rect::from_min_max(rect.min, pos2(rect.min.x + 3.0, rect.max.y)), CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, acento());
    } else if r.hovered() {
        ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
    }
    ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0), g, v.text_color());
    r
}

/// Botón principal: relleno con el acento de la app y letra blanca (`bg-x-600 text-white`).
pub fn boton_primario(texto: impl Into<String>) -> eframe::egui::Button<'static> {
    let c = acento();
    eframe::egui::Button::new(RichText::new(texto.into()).color(Color32::WHITE).strong()).fill(c).stroke(Stroke::new(1.0, c.lerp_to_gamma(Color32::BLACK, 0.15))).corner_radius(6.0)
}

/// Eventos de portapapeles que faltan en macOS: egui solo convierte Cmd+C/X/V en
/// copiar/cortar/pegar; Ctrl+C/X/V (costumbre de Windows y Linux) no hacían nada en
/// las cajas de texto. Devuelve los eventos a añadir y si hay que pedir el pegado.
pub fn ctrl_portapapeles(events: &[eframe::egui::Event], mac: bool) -> (Vec<eframe::egui::Event>, bool) {
    use eframe::egui::{Event, Key};
    let (mut extra, mut pegar) = (Vec::new(), false);
    if !mac {
        return (extra, pegar);
    }
    for e in events {
        if let Event::Key { key, pressed: true, modifiers, .. } = e {
            if modifiers.ctrl && !modifiers.mac_cmd && !modifiers.alt {
                match key {
                    Key::C => extra.push(Event::Copy),
                    Key::X => extra.push(Event::Cut),
                    Key::V => pegar = true,
                    _ => {}
                }
            }
        }
    }
    (extra, pegar)
}

/// Llamar desde `raw_input_hook`: en macOS, Ctrl+C/X/V también copian, cortan y pegan.
pub fn portapapeles_mac(ctx: &Context, raw: &mut eframe::egui::RawInput) {
    let (extra, pegar) = ctrl_portapapeles(&raw.events, cfg!(target_os = "macos"));
    raw.events.extend(extra);
    if pegar {
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::RequestPaste);
    }
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
            paleta_tailwind(&mut s.visuals, dark);
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

/// Título de un grupo de ajustes: pequeño, en mayúsculas y apagado, encima de su tarjeta.
pub fn seccion(ui: &mut Ui, titulo: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(titulo.to_uppercase()).size(11.0).strong().color(Paleta::de(ui).apagado).extra_letter_spacing(0.6));
    ui.add_space(2.0);
}

pub fn acento_texto(ui: &Ui) -> Color32 {
    let c = acento();
    if ui.visuals().dark_mode { c.lerp_to_gamma(Color32::WHITE, 0.25) } else { c.lerp_to_gamma(Color32::BLACK, 0.15) }
}

/// Rejilla de dos columnas para las filas de un grupo, dentro de una tarjeta.
pub fn rejilla(ui: &mut Ui, id: &str, filas: impl FnOnce(&mut Ui)) {
    let p = Paleta::de(ui);
    Frame::new().fill(p.central).stroke(Stroke::new(1.0, p.borde)).corner_radius(8.0).inner_margin(eframe::egui::Margin::symmetric(14, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        Grid::new(id).num_columns(2).spacing([24.0, 12.0]).min_col_width(150.0).show(ui, filas);
    });
}

/// Una fila: etiqueta a la izquierda (algo más suave que el texto), controles a la derecha.
pub fn fila(ui: &mut Ui, etiqueta: &str, ayuda: Option<&str>, controles: impl FnOnce(&mut Ui)) {
    let p = Paleta::de(ui);
    let r = ui.label(RichText::new(etiqueta).color(p.texto.lerp_to_gamma(p.apagado, 0.3)));
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
