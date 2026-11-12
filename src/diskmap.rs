//! Mapa de disco: lo que ocupa cada carpeta y archivo como «treemap»
//! (rectángulos proporcionales al tamaño en disco) y búsqueda de texto en los
//! archivos de ciertas extensiones; los que contienen el texto se pintan en verde.
//!
//! Todo es de solo lectura: se lee la carpeta en un hilo y se busca con varios
//! hilos sobre `mmap`, sin cargar los archivos en RAM.

use eframe::egui::{
    self, Align2, Button, Color32, Context, CornerRadius, FontId, Id, Key, Mesh, Pos2, Rect, RichText, ScrollArea, Sense, Shape, Stroke,
    StrokeKind, TextEdit, Ui, UiBuilder, pos2, vec2,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const NONE: u32 = u32::MAX;
/// Celdas como máximo en el mapa (más no se distinguen y cuestan al pintar).
const MAX_CELLS: usize = 150_000;
/// Lado mínimo (px) de una celda para dibujarla.
const MIN_SIDE: f32 = 2.0;
/// Alto de la cabecera con el nombre de las carpetas grandes.
const HEADER: f32 = 13.0;
pub const DEFAULT_EXTS: &str = ".log, .txt, .xml, .json, .csv, .md, .yaml, .yml, .ini, .conf, .html";

pub struct Node {
    pub name: Box<str>,
    /// Tamaño en disco (bloques ocupados); en carpetas, la suma de su contenido.
    pub size: u64,
    /// Tamaño del contenido (bytes) de un archivo.
    pub len: u64,
    pub parent: u32,
    /// Hijos ordenados de mayor a menor tamaño.
    pub children: Vec<u32>,
    pub dir: bool,
    /// Archivos que contiene (1 para un archivo).
    pub files: u64,
}

pub struct Tree {
    pub root: PathBuf,
    /// El nodo 0 es la carpeta raíz; los hijos siempre van después que su padre.
    pub nodes: Vec<Node>,
    /// La lectura se canceló: el árbol está incompleto.
    pub partial: bool,
}

impl Tree {
    pub fn path(&self, mut i: u32) -> PathBuf {
        let mut parts = Vec::new();
        while i != 0 && i != NONE {
            parts.push(&*self.nodes[i as usize].name);
            i = self.nodes[i as usize].parent;
        }
        let mut p = self.root.clone();
        p.extend(parts.iter().rev());
        p
    }

    /// Ruta relativa a la raíz (para mostrar).
    pub fn rel(&self, i: u32) -> String {
        let p = self.path(i);
        p.strip_prefix(&self.root).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string())
    }

    pub fn ext(&self, i: u32) -> Option<String> {
        let n = &self.nodes[i as usize];
        if n.dir {
            return None;
        }
        n.name.rsplit_once('.').filter(|(a, _)| !a.is_empty()).map(|(_, e)| e.to_ascii_lowercase())
    }

    pub fn is_ancestor(&self, a: u32, mut i: u32) -> bool {
        while i != NONE {
            if i == a {
                return true;
            }
            i = self.nodes[i as usize].parent;
        }
        false
    }
}

#[cfg(unix)]
fn disk_size(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.blocks() * 512
}
#[cfg(not(unix))]
fn disk_size(m: &std::fs::Metadata) -> u64 {
    m.len()
}

#[cfg(unix)]
fn device(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.dev()
}
#[cfg(not(unix))]
fn device(_: &std::fs::Metadata) -> u64 {
    0
}

/// Lectura de una carpeta en segundo plano.
pub struct Scan {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub cancel: AtomicBool,
    pub result: Mutex<Option<Tree>>,
}

impl Scan {
    pub fn start(root: PathBuf, ctx: Option<Context>) -> Arc<Scan> {
        let s = Arc::new(Scan {
            files: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            result: Mutex::new(None),
        });
        let s2 = s.clone();
        std::thread::spawn(move || {
            let t = build_tree(root, &s2);
            *s2.result.lock().unwrap() = Some(t);
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
        s
    }
}

/// Recorre la carpeta sin seguir enlaces simbólicos ni pasar a otros discos
/// (así «/» no entra en /proc ni en volúmenes montados).
fn build_tree(root: PathBuf, s: &Scan) -> Tree {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
    let dev = std::fs::metadata(&root).map(|m| device(&m)).unwrap_or(0);
    let mut nodes = vec![Node { name: name.into(), size: 0, len: 0, parent: NONE, children: Vec::new(), dir: true, files: 0 }];
    let mut stack = vec![(0u32, root.clone())];
    let mut partial = false;
    while let Some((idx, path)) = stack.pop() {
        if s.cancel.load(Ordering::Relaxed) {
            partial = true;
            break;
        }
        let Ok(rd) = std::fs::read_dir(&path) else {
            s.errors.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else {
                s.errors.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            if m.file_type().is_symlink() {
                continue;
            }
            let ni = nodes.len() as u32;
            let name: Box<str> = e.file_name().to_string_lossy().into();
            if m.is_dir() {
                if device(&m) != dev {
                    continue;
                }
                nodes.push(Node { name, size: 0, len: 0, parent: idx, children: Vec::new(), dir: true, files: 0 });
                stack.push((ni, e.path()));
            } else {
                let size = disk_size(&m);
                s.files.fetch_add(1, Ordering::Relaxed);
                s.bytes.fetch_add(size, Ordering::Relaxed);
                nodes.push(Node { name, size, len: m.len(), parent: idx, children: Vec::new(), dir: false, files: 1 });
            }
            nodes[idx as usize].children.push(ni);
        }
    }
    // Los hijos van después del padre: sumando de atrás adelante, cada carpeta
    // está completa antes de sumarse a la suya.
    for i in (1..nodes.len()).rev() {
        let (p, size, files) = (nodes[i].parent as usize, nodes[i].size, nodes[i].files);
        nodes[p].size += size;
        nodes[p].files += files;
    }
    for i in 0..nodes.len() {
        let mut ch = std::mem::take(&mut nodes[i].children);
        ch.sort_by(|a, b| nodes[*b as usize].size.cmp(&nodes[*a as usize].size).then_with(|| nodes[*a as usize].name.cmp(&nodes[*b as usize].name)));
        nodes[i].children = ch;
    }
    Tree { root, nodes, partial }
}

/// Extensiones de «.log, txt ,XML» → ["log", "txt", "xml"]. Vacío = todas.
pub fn parse_exts(s: &str) -> Vec<String> {
    s.split([',', ' ', ';'])
        .map(|e| e.trim().trim_start_matches("*.").trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

/// Búsqueda de un texto en los archivos del árbol.
pub struct Find {
    pub needle: String,
    pub case_sensitive: bool,
    pub total_files: u64,
    pub total_bytes: u64,
    pub done_files: AtomicU64,
    pub done_bytes: AtomicU64,
    pub errors: AtomicU64,
    pub found: Mutex<Vec<u32>>,
    pub cancel: AtomicBool,
    pub finished: AtomicBool,
    pub started: Instant,
    pub elapsed: Mutex<f32>,
}

impl Find {
    pub fn start(tree: Arc<Tree>, needle: &str, case_sensitive: bool, exts: &[String], ctx: Option<Context>) -> Result<Arc<Find>, String> {
        let re = regex::bytes::RegexBuilder::new(&regex::escape(needle))
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|e| e.to_string())?;
        let cands: Vec<u32> = (0..tree.nodes.len() as u32)
            .filter(|&i| {
                let n = &tree.nodes[i as usize];
                !n.dir && n.len > 0 && (exts.is_empty() || tree.ext(i).is_some_and(|e| exts.contains(&e)))
            })
            .collect();
        let f = Arc::new(Find {
            needle: needle.to_string(),
            case_sensitive,
            total_files: cands.len() as u64,
            total_bytes: cands.iter().map(|&i| tree.nodes[i as usize].len).sum(),
            done_files: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            found: Mutex::new(Vec::new()),
            cancel: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            started: Instant::now(),
            elapsed: Mutex::new(0.0),
        });
        let f2 = f.clone();
        std::thread::spawn(move || {
            let next = AtomicUsize::new(0);
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
            std::thread::scope(|sc| {
                for _ in 0..threads {
                    sc.spawn(|| {
                        let mut last_repaint = Instant::now();
                        loop {
                            if f2.cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            let k = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&i) = cands.get(k) else { return };
                            match file_contains(&tree.path(i), &re) {
                                Ok(true) => f2.found.lock().unwrap().push(i),
                                Ok(false) => {}
                                Err(_) => {
                                    f2.errors.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            f2.done_files.fetch_add(1, Ordering::Relaxed);
                            f2.done_bytes.fetch_add(tree.nodes[i as usize].len, Ordering::Relaxed);
                            if let Some(c) = &ctx {
                                if last_repaint.elapsed().as_millis() > 150 {
                                    c.request_repaint();
                                    last_repaint = Instant::now();
                                }
                            }
                        }
                    });
                }
            });
            f2.found.lock().unwrap().sort_by(|a, b| tree.nodes[*b as usize].size.cmp(&tree.nodes[*a as usize].size));
            *f2.elapsed.lock().unwrap() = f2.started.elapsed().as_secs_f32();
            f2.finished.store(true, Ordering::Release);
            if let Some(c) = &ctx {
                c.request_repaint();
            }
        });
        Ok(f)
    }

    pub fn is_done(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

fn file_contains(path: &Path, re: &regex::bytes::Regex) -> std::io::Result<bool> {
    let f = std::fs::File::open(path)?;
    if f.metadata()?.len() == 0 {
        return Ok(false);
    }
    // SAFETY: solo lectura; si otro proceso trunca el archivo podría haber
    // SIGBUS, igual que en el editor (conocido).
    let m = unsafe { memmap2::Mmap::map(&f)? };
    Ok(re.is_match(&m))
}

// ---------------------------------------------------------------------------
// Distribución «squarified» (Bruls, Huizing y van Wijk): rectángulos lo más
// cuadrados posible, en filas a lo largo del lado corto.

#[derive(Clone, Copy)]
pub struct Cell {
    pub rect: Rect,
    pub node: u32,
    pub depth: u16,
    /// La carpeta tiene cabecera con su nombre.
    pub header: bool,
}

pub fn layout(tree: &Tree, view: u32, rect: Rect) -> Vec<Cell> {
    let mut out = Vec::new();
    push_node(tree, view, rect, 0, &mut out);
    out
}

fn push_node(tree: &Tree, i: u32, rect: Rect, depth: u16, out: &mut Vec<Cell>) {
    let n = &tree.nodes[i as usize];
    let mut inner = if depth == 0 { rect } else { rect.shrink(1.5) };
    let header = n.dir && depth > 0 && inner.height() > HEADER * 2.0 && inner.width() > 50.0;
    if header {
        inner.min.y += HEADER;
    }
    out.push(Cell { rect, node: i, depth, header });
    if !n.dir || n.size == 0 || inner.width() < MIN_SIDE || inner.height() < MIN_SIDE {
        return;
    }
    let items: Vec<(u32, f64)> =
        n.children.iter().map(|&c| (c, tree.nodes[c as usize].size as f64)).filter(|(_, s)| *s > 0.0).collect();
    let total: f64 = items.iter().map(|(_, s)| s).sum();
    squarify(&items, inner, total, &mut |c, r| {
        if out.len() < MAX_CELLS && r.width() >= MIN_SIDE && r.height() >= MIN_SIDE {
            push_node(tree, c, r, depth + 1, out);
        }
    });
}

/// `items` ordenados de mayor a menor; reparte el área de `rect` en proporción.
pub fn squarify(items: &[(u32, f64)], rect: Rect, total: f64, f: &mut impl FnMut(u32, Rect)) {
    if total <= 0.0 {
        return;
    }
    let mut r = rect;
    let scale = (rect.width() as f64 * rect.height() as f64) / total;
    let mut i = 0;
    while i < items.len() {
        let short = r.width().min(r.height()) as f64;
        if short <= 0.0 {
            break;
        }
        // Se añaden elementos a la fila mientras no empeore la proporción.
        let (mut j, mut sum, mut prev) = (i, 0.0, f64::INFINITY);
        while j < items.len() {
            let a = items[j].1 * scale;
            let s = sum + a;
            let w = worst(s, items[i].1 * scale, a, short);
            if j > i && w > prev {
                break;
            }
            prev = w;
            sum = s;
            j += 1;
        }
        let thick = (sum / short) as f32;
        if r.width() >= r.height() {
            // Columna a la izquierda.
            let mut y = r.min.y;
            for &(c, s) in &items[i..j] {
                let h = (s * scale / thick as f64) as f32;
                f(c, Rect::from_min_size(pos2(r.min.x, y), vec2(thick, h)));
                y += h;
            }
            r.min.x += thick;
        } else {
            // Fila arriba.
            let mut x = r.min.x;
            for &(c, s) in &items[i..j] {
                let w = (s * scale / thick as f64) as f32;
                f(c, Rect::from_min_size(pos2(x, r.min.y), vec2(w, thick)));
                x += w;
            }
            r.min.y += thick;
        }
        i = j;
    }
}

/// Peor proporción (≥ 1) de una fila de área `s` sobre un lado `w`, con el
/// elemento mayor `mx` y el menor `mn`.
fn worst(s: f64, mx: f64, mn: f64, w: f64) -> f64 {
    let (s2, w2) = (s * s, w * w);
    (w2 * mx / s2).max(s2 / (w2 * mn))
}

// ---------------------------------------------------------------------------
// Interfaz.

/// Lo que la ventana pide a la app.
#[derive(Default)]
pub struct Out {
    pub pick_folder: bool,
    /// Abrir en el editor; con el texto buscado (y si distingue mayúsculas)
    /// para buscarlo en ellos.
    pub open: Vec<PathBuf>,
    pub needle: Option<(String, bool)>,
    pub reveal: Option<PathBuf>,
    pub copy: Option<String>,
    pub msg: Option<String>,
}

pub struct DiskMap {
    pub open: bool,
    /// Se muestra en una ventana propia del sistema (otro monitor, etc.).
    pub detached: bool,
    pub scan: Option<Arc<Scan>>,
    pub tree: Option<Arc<Tree>>,
    /// Carpeta que se ve (zoom).
    view: u32,
    pub exts: String,
    pub needle: String,
    pub case_sensitive: bool,
    pub find: Option<Arc<Find>>,
    /// 1 = archivo con el texto, 2 = carpeta que contiene alguno.
    marks: Vec<u8>,
    marks_seen: usize,
    pub selected: BTreeSet<u32>,
    cache: Option<(Rect, u32, Vec<Cell>)>,
    /// Resaltado desde la lista de encontrados.
    focus: Option<u32>,
    /// Celda del último clic derecho (el menú sigue abierto aunque el ratón salga del mapa).
    menu_node: Option<u32>,
    /// Buscar cuando termine de leer (BUSCADOR_MAPA en pruebas).
    pub search_when_ready: bool,
}

impl Default for DiskMap {
    fn default() -> Self {
        DiskMap {
            open: false,
            detached: false,
            scan: None,
            tree: None,
            view: 0,
            exts: DEFAULT_EXTS.into(),
            needle: String::new(),
            case_sensitive: false,
            find: None,
            marks: Vec::new(),
            marks_seen: 0,
            selected: BTreeSet::new(),
            cache: None,
            focus: None,
            menu_node: None,
            search_when_ready: false,
        }
    }
}

pub fn window_id() -> Id {
    Id::NULL.with("ventana_mapa")
}

impl DiskMap {
    pub fn is_busy(&self) -> bool {
        self.scan.is_some() || self.find.as_ref().is_some_and(|f| !f.is_done())
    }

    pub fn start_scan(&mut self, root: PathBuf, ctx: &Context) {
        self.stop();
        self.tree = None;
        self.find = None;
        self.marks.clear();
        self.marks_seen = 0;
        self.selected.clear();
        self.cache = None;
        self.focus = None;
        self.view = 0;
        self.scan = Some(Scan::start(root, Some(ctx.clone())));
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.scan.take() {
            s.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(f) = &self.find {
            f.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn start_find(&mut self, ctx: &Context) -> Option<String> {
        let tree = self.tree.clone()?;
        if self.needle.is_empty() {
            return Some("Escribe el texto a buscar".into());
        }
        if let Some(f) = &self.find {
            f.cancel.store(true, Ordering::Relaxed);
        }
        self.marks = vec![0; tree.nodes.len()];
        self.marks_seen = 0;
        self.selected.clear();
        match Find::start(tree, &self.needle, self.case_sensitive, &parse_exts(&self.exts), Some(ctx.clone())) {
            Ok(f) => {
                self.find = Some(f);
                None
            }
            Err(e) => Some(e),
        }
    }

    /// Pasa a `marks` las coincidencias nuevas (y marca sus carpetas).
    fn update_marks(&mut self) {
        let (Some(tree), Some(f)) = (&self.tree, &self.find) else { return };
        let found = f.found.lock().unwrap();
        if self.marks.len() != tree.nodes.len() {
            self.marks = vec![0; tree.nodes.len()];
            self.marks_seen = 0;
        }
        for &i in &found[self.marks_seen.min(found.len())..] {
            self.marks[i as usize] = 1;
            let mut p = tree.nodes[i as usize].parent;
            while p != NONE && self.marks[p as usize] == 0 {
                self.marks[p as usize] = 2;
                p = tree.nodes[p as usize].parent;
            }
        }
        self.marks_seen = found.len();
    }

    /// Estado de la carpeta: leyendo (con «Detener»), o ruta y totales (con «⟳»).
    fn status_ui(&mut self, ui: &mut Ui, ctx: &Context) {
        if let Some(s) = &self.scan {
            ui.spinner();
            ui.label(format!(
                "Leyendo… {} archivos · {}",
                crate::app::fmt_num(s.files.load(Ordering::Relaxed)),
                crate::app::human_size(s.bytes.load(Ordering::Relaxed))
            ));
            if ui.button("Detener").clicked() {
                s.cancel.store(true, Ordering::Relaxed);
            }
        } else if let Some(t) = &self.tree {
            let root = &t.nodes[0];
            ui.label(RichText::new(t.root.display().to_string()).strong());
            ui.weak(format!(
                "{} · {} archivos{}",
                crate::app::human_size(root.size),
                crate::app::fmt_num(root.files),
                if t.partial { " · lectura detenida (incompleto)" } else { "" }
            ));
            if ui.small_button("⟳").on_hover_text("Volver a leer la carpeta").clicked() {
                let r = t.root.clone();
                self.start_scan(r, ctx);
            }
        } else {
            ui.weak("Elige una carpeta para ver qué ocupa cada carpeta y archivo.");
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Out {
        let mut out = Out::default();
        let ctx = ui.ctx().clone();
        let ready = self.scan.as_ref().and_then(|s| s.result.lock().unwrap().take().map(|t| (t, s.errors.load(Ordering::Relaxed))));
        {
            if let Some((t, errors)) = ready {
                if errors > 0 {
                    out.msg = Some(format!("Mapa: {errors} carpetas o archivos sin permiso de lectura"));
                }
                self.tree = Some(Arc::new(t));
                self.scan = None;
                if std::mem::take(&mut self.search_when_ready) {
                    out.msg = self.start_find(&ctx).or(out.msg);
                }
            }
        }
        self.update_marks();

        // --- Carpeta ---
        ui.horizontal(|ui| {
            if ui.button("📁 Elegir carpeta…").clicked() {
                out.pick_folder = true;
            }
            let (label, hint) = if self.detached {
                ("Integrar", "Volver a mostrarlo dentro de la ventana principal")
            } else {
                ("Ventana aparte", "Mostrarlo en una ventana propia (puedes llevarla a otro monitor)")
            };
            // A la derecha de la fila, antes que la ruta (que puede ser larga).
            let w = ui.available_width();
            ui.allocate_ui_with_layout(vec2(w, ui.spacing().interact_size.y), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(label).on_hover_text(hint).clicked() {
                    self.detached = !self.detached;
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    self.status_ui(ui, &ctx);
                });
            });
        });
        // --- Búsqueda ---
        ui.horizontal(|ui| {
            ui.label("Extensiones:");
            ui.add(TextEdit::singleline(&mut self.exts).desired_width(230.0).hint_text("vacío = todas"))
                .on_hover_text("Solo se busca en archivos con estas extensiones (separadas por comas). Vacío = en todos.\nEl mapa siempre muestra todos los archivos.");
            ui.label("Texto:");
            let r = ui.add(TextEdit::singleline(&mut self.needle).desired_width(200.0).hint_text("texto a buscar").font(egui::TextStyle::Monospace));
            ui.toggle_value(&mut self.case_sensitive, "Aa").on_hover_text("Distinguir mayúsculas");
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let running = self.find.as_ref().is_some_and(|f| !f.is_done());
            if running {
                if ui.button("Cancelar").clicked() {
                    if let Some(f) = &self.find {
                        f.cancel.store(true, Ordering::Relaxed);
                    }
                }
            } else if self.scan.is_some() {
                // Aún se está leyendo la carpeta: la búsqueda queda en espera y
                // empieza sola al terminar el mapa.
                let b = ui
                    .add(Button::new(if self.search_when_ready { "⏳ En espera" } else { "🔍 Buscar" }).selected(self.search_when_ready))
                    .on_hover_text("La búsqueda empieza sola cuando termine de leerse la carpeta (clic otra vez para no buscar)");
                if b.clicked() || enter {
                    if self.needle.is_empty() {
                        out.msg = Some("Escribe el texto a buscar".into());
                    } else {
                        self.search_when_ready = if enter { true } else { !self.search_when_ready };
                    }
                }
            } else if ui.add_enabled(self.tree.is_some(), Button::new("🔍 Buscar")).clicked() || (enter && self.tree.is_some()) {
                out.msg = self.start_find(&ctx).or(out.msg.take());
            }
            if self.find.is_some() && !running && ui.small_button("✖").on_hover_text("Quitar el resaltado").clicked() {
                self.find = None;
                self.marks.clear();
                self.marks_seen = 0;
                self.selected.clear();
            }
        });
        if self.search_when_ready && self.scan.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Se buscará «{}» en cuanto termine de generarse el mapa", self.needle));
            });
        } else if let Some(f) = &self.find {
            let n = f.found.lock().unwrap().len();
            let done = f.done_files.load(Ordering::Relaxed);
            let mut s = if f.is_done() {
                let cancelled = f.cancel.load(Ordering::Relaxed) && done < f.total_files;
                format!(
                    "{} de {} archivos contienen «{}»{} · {:.1}s",
                    crate::app::fmt_num(n as u64),
                    crate::app::fmt_num(f.total_files),
                    f.needle,
                    if cancelled { " (cancelada)" } else { "" },
                    *f.elapsed.lock().unwrap()
                )
            } else {
                let p = f.done_bytes.load(Ordering::Relaxed) as f64 / f.total_bytes.max(1) as f64;
                format!(
                    "Buscando… {} / {} archivos ({:.0}%) · {} encontrados",
                    crate::app::fmt_num(done),
                    crate::app::fmt_num(f.total_files),
                    p * 100.0,
                    crate::app::fmt_num(n as u64)
                )
            };
            let e = f.errors.load(Ordering::Relaxed);
            if e > 0 {
                s += &format!(" · {e} sin poder leer");
            }
            ui.horizontal(|ui| {
                if !f.is_done() {
                    ui.spinner();
                }
                ui.label(s);
            });
        }

        let Some(tree) = self.tree.clone() else { return out };

        // --- Ruta del zoom ---
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(self.view != 0, Button::new("⬆").small()).on_hover_text("Subir un nivel").clicked() {
                self.view = tree.nodes[self.view as usize].parent;
            }
            let mut chain = vec![self.view];
            while let Some(&c) = chain.last().filter(|&&c| c != 0) {
                chain.push(tree.nodes[c as usize].parent);
            }
            for (k, &c) in chain.iter().rev().enumerate() {
                if k > 0 {
                    ui.weak("›");
                }
                if ui.selectable_label(c == self.view, &*tree.nodes[c as usize].name).clicked() {
                    self.view = c;
                }
            }
            ui.weak(format!("· {}", crate::app::human_size(tree.nodes[self.view as usize].size)));
        });

        // --- Mapa y, si hay coincidencias, lista a la derecha ---
        let found: Vec<u32> = self.find.as_ref().map(|f| f.found.lock().unwrap().clone()).unwrap_or_default();
        let full = ui.available_rect_before_wrap();
        let list_w = if self.find.is_some() { 300.0_f32.min(full.width() * 0.4) } else { 0.0 };
        let map_rect = Rect::from_min_max(full.min, pos2(full.right() - list_w - if list_w > 0.0 { 6.0 } else { 0.0 }, full.bottom()));
        ui.allocate_rect(full, Sense::hover());
        self.map_ui(ui, &tree, map_rect, &mut out);
        if list_w > 0.0 {
            let lr = Rect::from_min_max(pos2(full.right() - list_w, full.top()), full.max);
            ui.scope_builder(UiBuilder::new().max_rect(lr), |ui| self.list_ui(ui, &tree, &found, &mut out));
        }
        if !out.open.is_empty() {
            out.needle = self.find.as_ref().map(|f| (f.needle.clone(), f.case_sensitive));
        }
        out
    }

    fn list_ui(&mut self, ui: &mut Ui, tree: &Tree, found: &[u32], out: &mut Out) {
        ui.horizontal(|ui| {
            ui.strong(format!("Con el texto ({})", crate::app::fmt_num(found.len() as u64)));
        });
        ui.horizontal(|ui| {
            if ui.small_button("Todos").clicked() {
                self.selected = found.iter().copied().collect();
            }
            if ui.small_button("Ninguno").clicked() {
                self.selected.clear();
            }
            let n = self.selected.len();
            let b = ui.add_enabled(n > 0, Button::new(format!("📂 Abrir ({n})")));
            if b.on_hover_text("Abrir en el editor los archivos marcados y buscar en ellos el texto").clicked() {
                out.open = self.selected.iter().map(|&i| tree.path(i)).collect();
            }
        });
        ui.separator();
        let row_h = ui.spacing().interact_size.y;
        ScrollArea::vertical().id_salt("mapa_lista").auto_shrink([false, false]).show_rows(ui, row_h, found.len(), |ui, range| {
            for &i in &found[range] {
                ui.horizontal(|ui| {
                    let mut on = self.selected.contains(&i);
                    if ui.checkbox(&mut on, "").changed() {
                        if on {
                            self.selected.insert(i);
                        } else {
                            self.selected.remove(&i);
                        }
                    }
                    let n = &tree.nodes[i as usize];
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    let r = ui
                        .selectable_label(self.focus == Some(i), format!("{}  {}", n.name, crate::app::human_size(n.len)))
                        .on_hover_text(format!("{}\nClic: señalar en el mapa · Doble clic: abrir", tree.rel(i)));
                    if r.clicked() {
                        self.focus = Some(i);
                        // Que se vea: si está fuera del zoom, volver a la raíz.
                        if !tree.is_ancestor(self.view, i) {
                            self.view = 0;
                        }
                    }
                    if r.double_clicked() {
                        out.open = vec![tree.path(i)];
                    }
                });
            }
        });
    }

    fn map_ui(&mut self, ui: &mut Ui, tree: &Tree, rect: Rect, out: &mut Out) {
        if rect.width() < 20.0 || rect.height() < 20.0 {
            return;
        }
        let key_ok = self.cache.as_ref().is_some_and(|(r, v, _)| *r == rect && *v == self.view);
        if !key_ok {
            self.cache = Some((rect, self.view, layout(tree, self.view, rect)));
        }
        let cells = &self.cache.as_ref().unwrap().2;
        let resp = ui.interact(rect, Id::NULL.with("mapa_disco"), Sense::click());
        let painter = ui.painter_at(rect);

        let marks = &self.marks;
        let mark = |i: u32| marks.get(i as usize).copied().unwrap_or(0);
        let mut mesh = Mesh::default();
        for c in cells {
            let n = &tree.nodes[c.node as usize];
            let color = if n.dir {
                if mark(c.node) != 0 {
                    Color32::from_rgb(92, 158, 100)
                } else {
                    // Carpetas más oscuras que los archivos: los bordes se distinguen.
                    let g = 150u8.saturating_sub((c.depth.min(7) * 10) as u8);
                    Color32::from_gray(g)
                }
            } else if mark(c.node) == 1 {
                Color32::from_rgb(16, 112, 40)
            } else {
                file_color(tree.ext(c.node).as_deref())
            };
            // Los archivos se encogen un píxel: queda una raya del color de la carpeta.
            let r = if n.dir { c.rect } else { Rect::from_min_max(c.rect.min, c.rect.max - vec2(1.0, 1.0)) };
            mesh.add_colored_rect(r, color);
        }
        painter.add(Shape::mesh(mesh));

        // Nombres: cabecera de carpetas y archivos con sitio.
        let font = FontId::proportional(9.5);
        let ink = Color32::from_gray(15);
        for c in cells {
            let n = &tree.nodes[c.node as usize];
            let (show, pos) = if c.header {
                (true, c.rect.min + vec2(4.0, 1.0))
            } else if !n.dir && c.rect.width() > 50.0 && c.rect.height() > 14.0 {
                (true, c.rect.min + vec2(3.0, 2.0))
            } else {
                (false, Pos2::ZERO)
            };
            if show {
                let clip = painter.with_clip_rect(c.rect.shrink(1.0).intersect(rect));
                let label = if c.header || c.rect.height() < 26.0 {
                    format!("{}  {}", n.name, crate::app::human_size(n.size))
                } else {
                    format!("{}\n{}", n.name, crate::app::human_size(n.size))
                };
                let color = if !n.dir && mark(c.node) == 1 { Color32::WHITE } else { ink };
                clip.text(pos, Align2::LEFT_TOP, label, font.clone(), color);
            }
        }

        // Seleccionados y señalado desde la lista.
        for c in cells {
            if self.selected.contains(&c.node) {
                painter.rect_stroke(c.rect, CornerRadius::ZERO, Stroke::new(2.0, Color32::from_rgb(20, 90, 30)), StrokeKind::Inside);
            }
            if self.focus == Some(c.node) {
                painter.rect_stroke(c.rect.expand(1.0), CornerRadius::ZERO, Stroke::new(3.0, Color32::from_rgb(220, 60, 40)), StrokeKind::Outside);
            }
        }
        // Si el señalado es demasiado pequeño para tener celda, se marca su carpeta visible más profunda.
        if let Some(f) = self.focus.filter(|f| !cells.iter().any(|c| c.node == *f)) {
            if let Some(c) = cells.iter().rev().find(|c| tree.is_ancestor(c.node, f)) {
                painter.rect_stroke(c.rect, CornerRadius::ZERO, Stroke::new(3.0, Color32::from_rgb(220, 60, 40)), StrokeKind::Inside);
            }
        }

        // Celda bajo el ratón (la más profunda: van después de sus padres).
        let hover = resp.hover_pos().and_then(|p| cells.iter().rev().find(|c| c.rect.contains(p)).copied());
        if let Some(h) = hover {
            painter.rect_stroke(h.rect, CornerRadius::ZERO, Stroke::new(1.5, Color32::BLACK), StrokeKind::Inside);
            let n = &tree.nodes[h.node as usize];
            let mut tip = format!("{}\n{} en disco", tree.rel(h.node), crate::app::human_size(n.size));
            if n.dir {
                tip += &format!(" · {} archivos\nDoble clic: entrar", crate::app::fmt_num(n.files));
            } else {
                tip += &format!(" · {} bytes", crate::app::fmt_num(n.len));
                if mark(h.node) == 1 {
                    tip += "\n✔ Contiene el texto";
                }
                tip += "\nClic: marcar (Cmd/Ctrl+clic: varios) · Doble clic: abrir";
            }
            if mark(h.node) == 2 {
                tip += "\n✔ Tiene archivos con el texto";
            }
            resp.clone().on_hover_text_at_pointer(tip);
        }
        let n_of = |c: &Cell| &tree.nodes[c.node as usize];
        if let Some(h) = hover {
            if resp.double_clicked() {
                if n_of(&h).dir {
                    if h.node != self.view {
                        self.view = h.node;
                    }
                } else {
                    out.open = vec![tree.path(h.node)];
                }
            } else if resp.clicked() && !n_of(&h).dir {
                let multi = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                if multi {
                    if !self.selected.remove(&h.node) {
                        self.selected.insert(h.node);
                    }
                } else if self.selected.len() == 1 && self.selected.contains(&h.node) {
                    self.selected.clear();
                } else {
                    self.selected = [h.node].into();
                }
                self.focus = None;
            }
            if resp.secondary_clicked() {
                self.menu_node = Some(h.node);
            }
        }
        if let Some(m) = self.menu_node {
            if let Some(c) = cells.iter().find(|c| c.node == m) {
                painter.rect_stroke(c.rect, CornerRadius::ZERO, Stroke::new(1.5, Color32::BLACK), StrokeKind::Inside);
            }
        }
        let menu = self.menu_node;
        let shown = resp.context_menu(|ui| {
            let Some(node) = menu else {
                ui.close();
                return;
            };
            {
                let h = Cell { rect: Rect::NOTHING, node, depth: 0, header: false };
                let n = n_of(&h);
                if !n.dir && ui.button("Abrir en el editor").clicked() {
                    out.open = vec![tree.path(h.node)];
                    ui.close();
                }
                if !self.selected.is_empty() && ui.button(format!("Abrir los marcados ({})", self.selected.len())).clicked() {
                    out.open = self.selected.iter().map(|&i| tree.path(i)).collect();
                    ui.close();
                }
                if n.dir && h.node != self.view && ui.button("Entrar en la carpeta").clicked() {
                    self.view = h.node;
                    ui.close();
                }
                ui.separator();
                if ui.button(crate::app::REVEAL_LABEL).clicked() {
                    out.reveal = Some(tree.path(h.node));
                    ui.close();
                }
                if ui.button("Copiar ruta").clicked() {
                    out.copy = Some(tree.path(h.node).display().to_string());
                    ui.close();
                }
            }
        });
        if shown.is_none() {
            self.menu_node = None;
        }
    }
}

/// Color claro por extensión (el mismo siempre para la misma).
fn file_color(ext: Option<&str>) -> Color32 {
    let Some(e) = ext else { return Color32::from_rgb(214, 214, 198) };
    let mut h: u32 = 2166136261;
    for b in e.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.55, 0.90, 1.0).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squarify_cubre_el_area_sin_solaparse() {
        let items: Vec<(u32, f64)> = [50.0, 25.0, 12.0, 6.0, 4.0, 2.0, 1.0].iter().enumerate().map(|(i, s)| (i as u32, *s)).collect();
        let total: f64 = items.iter().map(|i| i.1).sum();
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(300.0, 200.0));
        let mut got = Vec::new();
        squarify(&items, rect, total, &mut |c, r| got.push((c, r)));
        assert_eq!(got.len(), items.len());
        let area: f32 = got.iter().map(|(_, r)| r.area()).sum();
        assert!((area - rect.area()).abs() < 1.0, "{area}");
        for (c, r) in &got {
            let want = items[*c as usize].1 / total * rect.area() as f64;
            assert!((r.area() as f64 - want).abs() < 1.0);
            assert!(rect.expand(0.01).contains_rect(*r));
        }
        for (a, (_, ra)) in got.iter().enumerate() {
            for (_, rb) in &got[a + 1..] {
                assert!(ra.intersect(*rb).area() < 0.01 || ra.intersect(*rb).width() < 0.0 || ra.intersect(*rb).height() < 0.0);
            }
        }
    }

    #[test]
    fn extensiones() {
        assert_eq!(parse_exts(".log, txt ,*.XML;json"), vec!["log", "txt", "xml", "json"]);
        assert!(parse_exts("  ").is_empty());
    }

    #[test]
    fn leer_carpeta_y_buscar() {
        let dir = std::env::temp_dir().join(format!("buscador-mapa-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("uno.log"), "hola\nERROR aquí\n").unwrap();
        std::fs::write(dir.join("a/dos.txt"), "nada que ver").unwrap();
        std::fs::write(dir.join("a/b/tres.log"), "x".repeat(10_000) + "error final").unwrap();
        std::fs::write(dir.join("a/b/cuatro.bin"), "ERROR pero no es de las extensiones").unwrap();
        let s = Scan::start(dir.clone(), None);
        let t = loop {
            if let Some(t) = s.result.lock().unwrap().take() {
                break t;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(t.nodes[0].files, 4);
        let a = t.nodes[0].children.iter().copied().find(|&c| &*t.nodes[c as usize].name == "a").unwrap();
        assert!(t.nodes[a as usize].dir);
        assert_eq!(t.nodes[a as usize].files, 3);
        // Los hijos van de mayor a menor.
        let sizes: Vec<u64> = t.nodes[0].children.iter().map(|&c| t.nodes[c as usize].size).collect();
        assert!(sizes.windows(2).all(|w| w[0] >= w[1]));
        let t = Arc::new(t);
        let names = |f: &Find| {
            let mut v: Vec<String> = f.found.lock().unwrap().iter().map(|&i| t.nodes[i as usize].name.to_string()).collect();
            v.sort();
            v
        };
        let wait = |f: &Arc<Find>| {
            while !f.is_done() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let f = Find::start(t.clone(), "error", false, &parse_exts(".log,.txt"), None).unwrap();
        wait(&f);
        assert_eq!(f.total_files, 3);
        assert_eq!(names(&f), vec!["tres.log", "uno.log"]);
        let f = Find::start(t.clone(), "ERROR", true, &parse_exts(".log,.txt"), None).unwrap();
        wait(&f);
        assert_eq!(names(&f), vec!["uno.log"]);
        let f = Find::start(t.clone(), "ERROR", true, &[], None).unwrap();
        wait(&f);
        assert_eq!(names(&f), vec!["cuatro.bin", "uno.log"]);
        assert!(t.path(t.nodes[a as usize].children[0]).starts_with(dir.join("a")));
        let cells = layout(&t, 0, Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0)));
        assert!(cells.len() >= 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
