// En Windows, sin ventana de consola junto a la app.
#![cfg_attr(windows, windows_subsystem = "windows")]
// Primero: sus macros t!/tf! se usan en todos los demás módulos.
#[macro_use]
mod i18n;
mod app;
mod beautify;
mod blocks;
mod buffer;
mod convert;
mod diskmap;
mod document;
mod filter;
mod foldersearch;
mod encoding;
mod fonts;
mod hexview;
mod icon;
mod icons;
#[cfg(target_os = "macos")]
mod macmenu;
#[cfg(target_os = "macos")]
mod macopen;
mod search;
mod recovery;
mod settings;
mod sigbus;
mod syntax;
mod timestamp;
mod update;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let mut files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    // Uso interno de empaquetar-mac.sh: `buscador --iconset carpeta`.
    if files.first().is_some_and(|a| a.as_os_str() == "--iconset") {
        let dir = files.get(1).cloned().unwrap_or_else(|| PathBuf::from("buscador.iconset"));
        icon::write_iconset(&dir).expect("no se pudo escribir el iconset");
        return Ok(());
    }
    // Para el instalador de Windows: `buscador --ico archivo.ico`.
    if files.first().is_some_and(|a| a.as_os_str() == "--ico") {
        let path = files.get(1).cloned().unwrap_or_else(|| PathBuf::from("buscador.ico"));
        std::fs::write(&path, icon::ico()).expect("no se pudo escribir el .ico");
        return Ok(());
    }
    // macOS añade «-psn_…» al abrir una .app en algunas versiones.
    files.retain(|f| !f.to_string_lossy().starts_with("-psn_"));
    // En modo captura (pruebas) la ventana no se activa: no roba el teclado.
    let testing = std::env::var_os("BUSCADOR_SCREENSHOT").is_some();
    // Un archivo abierto que otro programa recorta no debe tumbar la app.
    sigbus::install();
    #[cfg(target_os = "macos")]
    macopen::install();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_active(!testing)
            .with_title("Buscador")
            .with_app_id("buscador")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([960.0, 480.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(icon::app_icon())),
        // Posición, tamaño y monitor de la ventana: se guardan al cerrar y se
        // recuperan al abrir (también tras actualizar o reiniciar). Si ese monitor
        // ya no está conectado, eframe la pone en uno que exista. No en pruebas ni
        // en «Abrir en ventana nueva» (se pondría encima de la principal).
        persist_window: !testing && !files.iter().any(|f| f.as_os_str() == "--sin-sesion"),
        ..Default::default()
    };
    eframe::run_native(
        "buscador",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, files)))),
    )
}
