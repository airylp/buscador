//! Menú nativo de macOS en la barra superior del sistema.
//!
//! En macOS el menú se queda con las combinaciones de teclas que muestra
//! (Cmd+C, Cmd+Z…), así que cada opción se convierte de vuelta en el mismo
//! evento de teclado que recibiría egui: el editor, las cajas de texto y los
//! atajos funcionan exactamente igual que sin menú.

use eframe::egui::{Context, Event, Key, Modifiers, ViewportCommand};
use muda::accelerator::Accelerator;
use muda::{AboutMetadata, CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use std::sync::mpsc::{Receiver, channel};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cmd {
    /// Repetir la combinación de teclas en egui.
    Key(Modifiers, Key),
    Copy,
    Cut,
    Paste,
    ToggleResults,
    ToggleDetail,
    ToggleHeader,
    Settings,
    Update,
    OpenRecent(usize),
    ClearRecent,
}

pub struct MacMenu {
    _menu: Menu,
    rx: Receiver<MenuEvent>,
    items: std::cell::RefCell<Vec<(MenuId, Cmd)>>,
    /// «Abrir recientes» (se rehace cuando cambia la lista).
    recent: Submenu,
    recent_ids: std::cell::RefCell<Vec<MenuId>>,
    pub save: MenuItem,
    pub edit: CheckMenuItem,
    pub sidebar: CheckMenuItem,
    pub results: CheckMenuItem,
    pub detail: CheckMenuItem,
    pub ws: CheckMenuItem,
    pub header: CheckMenuItem,
    /// Último estado aplicado (guardar, edición, panel, resultados, detalle).
    state: std::cell::Cell<Option<[bool; 7]>>,
}

impl MacMenu {
    pub fn new(ctx: &Context) -> Self {
        let (tx, rx) = channel();
        let c = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e| {
            let _ = tx.send(e);
            c.request_repaint();
        }));

        let items = std::cell::RefCell::new(Vec::new());
        let accel = |a: &str| a.parse::<Accelerator>().ok();
        let item = |text: &str, a: &str, cmd: Cmd| {
            let it = MenuItem::new(text, true, accel(a));
            items.borrow_mut().push((it.id().clone(), cmd));
            it
        };
        let cmd = |k| Cmd::Key(Modifiers::COMMAND, k);
        let cmd_shift = |k| Cmd::Key(Modifiers::COMMAND | Modifiers::SHIFT, k);
        let sep = PredefinedMenuItem::separator;

        let settings = item(t!("Ajustes…"), "Cmd+,", Cmd::Settings);
        let update = item(t!("Buscar actualizaciones…"), "", Cmd::Update);
        let quit = item(t!("Salir de buscador"), "Cmd+Q", Cmd::Key(Modifiers::COMMAND, Key::Q));
        let about = PredefinedMenuItem::about(
            Some(t!("Acerca de buscador")),
            Some(AboutMetadata {
                name: Some("buscador".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                comments: Some(t!("Visor y editor de archivos de varios GB").into()),
                ..Default::default()
            }),
        );
        let app = Submenu::with_items(
            "buscador",
            true,
            &[
                &about,
                &update,
                &sep(),
                &settings,
                &sep(),
                &PredefinedMenuItem::services(Some(t!("Servicios"))),
                &sep(),
                &PredefinedMenuItem::hide(Some(t!("Ocultar buscador"))),
                &PredefinedMenuItem::hide_others(Some(t!("Ocultar otros"))),
                &PredefinedMenuItem::show_all(Some(t!("Mostrar todo"))),
                &sep(),
                &quit,
            ],
        )
        .unwrap();

        let new = item(t!("Nuevo"), "Cmd+N", cmd(Key::N));
        let open = item(t!("Abrir…"), "Cmd+O", cmd(Key::O));
        let save = item(t!("Guardar"), "Cmd+S", cmd(Key::S));
        let save_as = item(t!("Guardar como…"), "Cmd+Shift+S", cmd_shift(Key::S));
        let reload = item(t!("Recargar del disco"), "Cmd+R", cmd(Key::R));
        let in_folder = item(t!("Buscar en carpeta…"), "Cmd+Shift+O", cmd_shift(Key::O));
        let recent = Submenu::new(t!("Abrir recientes"), true);
        let close = item(t!("Cerrar archivo"), "Cmd+W", cmd(Key::W));
        let next = item(t!("Siguiente archivo"), "Ctrl+Tab", Cmd::Key(Modifiers::CTRL, Key::Tab));
        let prev = item(t!("Archivo anterior"), "Ctrl+Shift+Tab", Cmd::Key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab));
        let file = Submenu::with_items(t!("Archivo"), true, &[&new, &open, &recent, &in_folder, &sep(), &save, &save_as, &sep(), &reload, &sep(), &next, &prev, &close]).unwrap();

        let undo = item(t!("Deshacer"), "Cmd+Z", cmd(Key::Z));
        let redo = item(t!("Rehacer"), "Cmd+Shift+Z", cmd_shift(Key::Z));
        let cut = item(t!("Cortar"), "Cmd+X", Cmd::Cut);
        let copy = item(t!("Copiar"), "Cmd+C", Cmd::Copy);
        let paste = item(t!("Pegar"), "Cmd+V", Cmd::Paste);
        let all = item(t!("Seleccionar todo"), "Cmd+A", cmd(Key::A));
        let beautify = item(t!("Embellecer JSON / XML"), "Cmd+Shift+B", cmd_shift(Key::B));
        let edit = CheckMenuItem::new(t!("Permitir edición"), true, false, accel("Cmd+E"));
        items.borrow_mut().push((edit.id().clone(), cmd(Key::E)));
        let edit_menu = Submenu::with_items(
            t!("Edición"),
            true,
            &[&undo, &redo, &sep(), &cut, &copy, &paste, &all, &sep(), &beautify, &edit],
        )
        .unwrap();

        let find = item(t!("Buscar…"), "Cmd+F", cmd(Key::F));
        let find_next = item(t!("Siguiente coincidencia"), "F3", Cmd::Key(Modifiers::NONE, Key::F3));
        let find_prev = item(t!("Coincidencia anterior"), "Shift+F3", Cmd::Key(Modifiers::SHIFT, Key::F3));
        let replace = item(t!("Reemplazar…"), "Cmd+Alt+F", Cmd::Key(Modifiers::COMMAND | Modifiers::ALT, Key::F));
        let adv = item(t!("Nueva búsqueda avanzada…"), "Cmd+Shift+F", cmd_shift(Key::F));
        let goto = item(t!("Ir a línea…"), "Cmd+G", cmd(Key::G));
        let in_folder2 = item(t!("Buscar en carpeta…"), "", cmd_shift(Key::O));
        let search = Submenu::with_items(
            t!("Buscar"),
            true,
            &[&find, &find_next, &find_prev, &sep(), &replace, &adv, &in_folder2, &sep(), &goto],
        )
        .unwrap();

        let sidebar = CheckMenuItem::new(t!("Panel de archivos"), true, true, accel("Cmd+B"));
        items.borrow_mut().push((sidebar.id().clone(), cmd(Key::B)));
        let results = CheckMenuItem::new(t!("Resultados"), true, true, None);
        items.borrow_mut().push((results.id().clone(), Cmd::ToggleResults));
        let detail = CheckMenuItem::new(t!("Detalle de la línea"), true, false, None);
        items.borrow_mut().push((detail.id().clone(), Cmd::ToggleDetail));
        let header = CheckMenuItem::new(t!("Nombre y ruta del archivo"), true, true, None);
        items.borrow_mut().push((header.id().clone(), Cmd::ToggleHeader));
        let wrap = item(t!("Ajuste de línea"), "Cmd+Alt+Z", Cmd::Key(Modifiers::COMMAND | Modifiers::ALT, Key::Z));
        let ws = CheckMenuItem::new(t!("Mostrar espacios y tabuladores"), true, false, accel("Cmd+Alt+I"));
        items.borrow_mut().push((ws.id().clone(), Cmd::Key(Modifiers::COMMAND | Modifiers::ALT, Key::I)));
        let bigger = item(t!("Letra más grande"), "Cmd+=", cmd(Key::Equals));
        let smaller = item(t!("Letra más pequeña"), "Cmd+-", cmd(Key::Minus));
        let view = Submenu::with_items(
            t!("Ver"),
            true,
            &[
                &sidebar,
                &results,
                &detail,
                &header,
                &wrap,
                &ws,
                &sep(),
                &bigger,
                &smaller,
                &sep(),
                &PredefinedMenuItem::fullscreen(Some(t!("Pantalla completa"))),
            ],
        )
        .unwrap();

        let window = Submenu::with_items(
            t!("Ventana"),
            true,
            &[
                &PredefinedMenuItem::minimize(Some(t!("Minimizar"))),
                &PredefinedMenuItem::maximize(Some(t!("Zoom"))),
                &sep(),
                &PredefinedMenuItem::bring_all_to_front(Some(t!("Traer todo al frente"))),
            ],
        )
        .unwrap();
        let keys = item(t!("Atajos de teclado"), "F1", Cmd::Key(Modifiers::NONE, Key::F1));
        let help = Submenu::with_items(t!("Ayuda"), true, &[&keys]).unwrap();

        let menu = Menu::new();
        let subs: [&dyn IsMenuItem; 7] = [&app, &file, &edit_menu, &search, &view, &window, &help];
        menu.append_items(&subs).unwrap();
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();

        MacMenu { _menu: menu, rx, items, recent, recent_ids: Default::default(), save, edit, sidebar, results, detail, ws, header, state: Default::default() }
    }

    /// Refleja en el menú el estado de la aplicación (sólo si cambió).
    pub fn sync(&self, st: [bool; 7]) {
        if self.state.get() == Some(st) {
            return;
        }
        self.state.set(Some(st));
        self.save.set_enabled(st[0]);
        self.edit.set_checked(st[1]);
        self.sidebar.set_checked(st[2]);
        self.results.set_checked(st[3]);
        self.detail.set_checked(st[4]);
        self.ws.set_checked(st[5]);
        self.header.set_checked(st[6]);
    }

    /// Rehace «Abrir recientes» con esta lista (nombre — carpeta).
    pub fn set_recent(&self, list: &[std::path::PathBuf]) {
        while self.recent.remove_at(0).is_some() {}
        let old = std::mem::take(&mut *self.recent_ids.borrow_mut());
        self.items.borrow_mut().retain(|(id, _)| !old.contains(id));
        let add = |text: String, cmd: Cmd| {
            let it = MenuItem::new(text, true, None);
            self.items.borrow_mut().push((it.id().clone(), cmd));
            self.recent_ids.borrow_mut().push(it.id().clone());
            let _ = self.recent.append(&it);
        };
        if list.is_empty() {
            let it = MenuItem::new(t!("(ningún archivo reciente)"), false, None);
            let _ = self.recent.append(&it);
            return;
        }
        for (i, p) in list.iter().enumerate() {
            let name = p.file_name().map_or_else(|| p.to_string_lossy(), |n| n.to_string_lossy());
            let dir = p.parent().map(|d| d.to_string_lossy()).unwrap_or_default();
            add(format!("{name} — {dir}"), Cmd::OpenRecent(i));
        }
        let _ = self.recent.append(&PredefinedMenuItem::separator());
        add(t!("Borrar la lista").to_string(), Cmd::ClearRecent);
    }

    /// Opciones elegidas desde el último fotograma.
    pub fn poll(&self) -> Vec<Cmd> {
        self.rx
            .try_iter()
            .filter_map(|e| self.items.borrow().iter().find(|(id, _)| *id == e.id).map(|(_, c)| *c))
            .collect()
    }
}

/// Mete el comando en la entrada de egui de este fotograma.
pub fn inject(ctx: &Context, c: Cmd) {
    match c {
        Cmd::Key(modifiers, key) => ctx.input_mut(|i| {
            i.events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
        }),
        Cmd::Copy => ctx.input_mut(|i| i.events.push(Event::Copy)),
        Cmd::Cut => ctx.input_mut(|i| i.events.push(Event::Cut)),
        Cmd::Paste => ctx.send_viewport_cmd(ViewportCommand::RequestPaste),
        Cmd::ToggleResults | Cmd::ToggleDetail | Cmd::ToggleHeader | Cmd::Settings | Cmd::Update | Cmd::OpenRecent(_) | Cmd::ClearRecent => {}
    }
}
