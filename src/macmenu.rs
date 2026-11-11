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
    Settings,
}

pub struct MacMenu {
    _menu: Menu,
    rx: Receiver<MenuEvent>,
    items: Vec<(MenuId, Cmd)>,
    pub save: MenuItem,
    pub edit: CheckMenuItem,
    pub sidebar: CheckMenuItem,
    pub results: CheckMenuItem,
    pub detail: CheckMenuItem,
    /// Último estado aplicado (guardar, edición, panel, resultados, detalle).
    state: std::cell::Cell<Option<[bool; 5]>>,
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

        let settings = item("Ajustes…", "Cmd+,", Cmd::Settings);
        let quit = item("Salir de buscador", "Cmd+Q", Cmd::Key(Modifiers::COMMAND, Key::Q));
        let about = PredefinedMenuItem::about(
            Some("Acerca de buscador"),
            Some(AboutMetadata {
                name: Some("buscador".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                comments: Some("Visor y editor de archivos de varios GB".into()),
                ..Default::default()
            }),
        );
        let app = Submenu::with_items(
            "buscador",
            true,
            &[
                &about,
                &sep(),
                &settings,
                &sep(),
                &PredefinedMenuItem::services(Some("Servicios")),
                &sep(),
                &PredefinedMenuItem::hide(Some("Ocultar buscador")),
                &PredefinedMenuItem::hide_others(Some("Ocultar otros")),
                &PredefinedMenuItem::show_all(Some("Mostrar todo")),
                &sep(),
                &quit,
            ],
        )
        .unwrap();

        let new = item("Nuevo", "Cmd+N", cmd(Key::N));
        let open = item("Abrir…", "Cmd+O", cmd(Key::O));
        let save = item("Guardar", "Cmd+S", cmd(Key::S));
        let save_as = item("Guardar como…", "Cmd+Shift+S", cmd_shift(Key::S));
        let close = item("Cerrar archivo", "Cmd+W", cmd(Key::W));
        let next = item("Siguiente archivo", "Ctrl+Tab", Cmd::Key(Modifiers::CTRL, Key::Tab));
        let file = Submenu::with_items("Archivo", true, &[&new, &open, &sep(), &save, &save_as, &sep(), &next, &close]).unwrap();

        let undo = item("Deshacer", "Cmd+Z", cmd(Key::Z));
        let redo = item("Rehacer", "Cmd+Shift+Z", cmd_shift(Key::Z));
        let cut = item("Cortar", "Cmd+X", Cmd::Cut);
        let copy = item("Copiar", "Cmd+C", Cmd::Copy);
        let paste = item("Pegar", "Cmd+V", Cmd::Paste);
        let all = item("Seleccionar todo", "Cmd+A", cmd(Key::A));
        let edit = CheckMenuItem::new("Permitir edición", true, false, accel("Cmd+E"));
        items.borrow_mut().push((edit.id().clone(), cmd(Key::E)));
        let edit_menu = Submenu::with_items(
            "Edición",
            true,
            &[&undo, &redo, &sep(), &cut, &copy, &paste, &all, &sep(), &edit],
        )
        .unwrap();

        let find = item("Buscar…", "Cmd+F", cmd(Key::F));
        let find_next = item("Siguiente coincidencia", "F3", Cmd::Key(Modifiers::NONE, Key::F3));
        let find_prev = item("Coincidencia anterior", "Shift+F3", Cmd::Key(Modifiers::SHIFT, Key::F3));
        let replace = item("Reemplazar…", "Cmd+Alt+F", Cmd::Key(Modifiers::COMMAND | Modifiers::ALT, Key::F));
        let adv = item("Nueva búsqueda avanzada…", "Cmd+Shift+F", cmd_shift(Key::F));
        let goto = item("Ir a línea…", "Cmd+G", cmd(Key::G));
        let search = Submenu::with_items(
            "Buscar",
            true,
            &[&find, &find_next, &find_prev, &sep(), &replace, &adv, &sep(), &goto],
        )
        .unwrap();

        let sidebar = CheckMenuItem::new("Panel de archivos", true, true, accel("Cmd+B"));
        items.borrow_mut().push((sidebar.id().clone(), cmd(Key::B)));
        let results = CheckMenuItem::new("Resultados", true, true, None);
        items.borrow_mut().push((results.id().clone(), Cmd::ToggleResults));
        let detail = CheckMenuItem::new("Detalle de la línea", true, false, None);
        items.borrow_mut().push((detail.id().clone(), Cmd::ToggleDetail));
        let bigger = item("Letra más grande", "Cmd+=", cmd(Key::Equals));
        let smaller = item("Letra más pequeña", "Cmd+-", cmd(Key::Minus));
        let view = Submenu::with_items(
            "Ver",
            true,
            &[
                &sidebar,
                &results,
                &detail,
                &sep(),
                &bigger,
                &smaller,
                &sep(),
                &PredefinedMenuItem::fullscreen(Some("Pantalla completa")),
            ],
        )
        .unwrap();

        let window = Submenu::with_items(
            "Ventana",
            true,
            &[
                &PredefinedMenuItem::minimize(Some("Minimizar")),
                &PredefinedMenuItem::maximize(Some("Zoom")),
                &sep(),
                &PredefinedMenuItem::bring_all_to_front(Some("Traer todo al frente")),
            ],
        )
        .unwrap();
        let keys = item("Atajos de teclado", "F1", Cmd::Key(Modifiers::NONE, Key::F1));
        let help = Submenu::with_items("Ayuda", true, &[&keys]).unwrap();

        let menu = Menu::new();
        let subs: [&dyn IsMenuItem; 7] = [&app, &file, &edit_menu, &search, &view, &window, &help];
        menu.append_items(&subs).unwrap();
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();

        MacMenu { _menu: menu, rx, items: items.into_inner(), save, edit, sidebar, results, detail, state: Default::default() }
    }

    /// Refleja en el menú el estado de la aplicación (sólo si cambió).
    pub fn sync(&self, st: [bool; 5]) {
        if self.state.get() == Some(st) {
            return;
        }
        self.state.set(Some(st));
        self.save.set_enabled(st[0]);
        self.edit.set_checked(st[1]);
        self.sidebar.set_checked(st[2]);
        self.results.set_checked(st[3]);
        self.detail.set_checked(st[4]);
    }

    /// Opciones elegidas desde el último fotograma.
    pub fn poll(&self) -> Vec<Cmd> {
        self.rx
            .try_iter()
            .filter_map(|e| self.items.iter().find(|(id, _)| *id == e.id).map(|(_, c)| *c))
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
        Cmd::ToggleResults | Cmd::ToggleDetail | Cmd::Settings => {}
    }
}
