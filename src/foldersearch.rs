//! «Buscar en carpeta»: busca un texto (o regex) en todos los archivos de
//! texto de una carpeta y sus subcarpetas, y lista las líneas encontradas
//! agrupadas por archivo. Clic en una línea la abre en el editor.
//!
//! Recorre la carpeta en un hilo y reparte los archivos entre varios hilos
//! (mmap + `regex::bytes`, sin cargarlos en RAM). Se saltan carpetas ocultas
//! (.git…) y `node_modules`, enlaces simbólicos y archivos binarios.

use eframe::egui::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

/// Líneas que se guardan por archivo y en total.
const MAX_PER_FILE: usize = 500;
const MAX_TOTAL: u64 = 100_000;
/// Bytes de cada línea que se muestran.
const SHOW_BYTES: usize = 300;

pub struct Hit {
    pub line: u64,
    pub text: String,
    /// Dónde está la coincidencia dentro de `text` (bytes).
    pub at: (usize, usize),
}

pub struct FileHits {
    pub path: PathBuf,
    pub hits: Vec<Hit>,
    /// Había más coincidencias que no se guardaron.
    pub more: bool,
}

pub struct Job {
    pub root: PathBuf,
    pub query: String,
    pub regex: bool,
    pub case: bool,
    pub files_seen: AtomicU64,
    pub files_done: AtomicU64,
    pub skipped: AtomicU64,
    pub matches: AtomicU64,
    pub results: Mutex<Vec<FileHits>>,
    pub truncated: AtomicBool,
    cancel: AtomicBool,
    finished: AtomicBool,
    started: Instant,
    elapsed: Mutex<f32>,
}

impl Job {
    pub fn start(root: PathBuf, query: &str, regex: bool, case: bool, exts: Vec<String>, ctx: Context) -> Result<Arc<Job>, String> {
        let pat = if regex { query.to_string() } else { regex::escape(query) };
        let re = regex::bytes::RegexBuilder::new(&pat).case_insensitive(!case).multi_line(true).build().map_err(|e| e.to_string())?;
        let job = Arc::new(Job {
            root: root.clone(),
            query: query.to_string(),
            regex,
            case,
            files_seen: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            matches: AtomicU64::new(0),
            results: Mutex::new(Vec::new()),
            truncated: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            started: Instant::now(),
            elapsed: Mutex::new(0.0),
        });
        let j = job.clone();
        std::thread::spawn(move || {
            let (tx, rx) = mpsc::sync_channel::<PathBuf>(1024);
            let rx = Mutex::new(rx);
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
            std::thread::scope(|sc| {
                for _ in 0..threads {
                    sc.spawn(|| {
                        let mut last = Instant::now();
                        loop {
                            let Ok(p) = rx.lock().unwrap().recv() else { return };
                            if j.cancel.load(Ordering::Relaxed) {
                                continue;
                            }
                            match search_file(&p, &re) {
                                Some((hits, more)) if !hits.is_empty() => {
                                    let n = hits.len() as u64;
                                    let total = j.matches.fetch_add(n, Ordering::Relaxed) + n;
                                    j.results.lock().unwrap().push(FileHits { path: p, hits, more });
                                    if total >= MAX_TOTAL {
                                        j.truncated.store(true, Ordering::Relaxed);
                                        j.cancel.store(true, Ordering::Relaxed);
                                    }
                                }
                                Some(_) => {}
                                None => {
                                    j.skipped.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            j.files_done.fetch_add(1, Ordering::Relaxed);
                            if last.elapsed().as_millis() > 150 {
                                ctx.request_repaint();
                                last = Instant::now();
                            }
                        }
                    });
                }
                walk(&root, &exts, &j, &tx);
                drop(tx);
            });
            // Orden estable: por ruta.
            j.results.lock().unwrap().sort_by(|a, b| a.path.cmp(&b.path));
            *j.elapsed.lock().unwrap() = j.started.elapsed().as_secs_f32();
            j.finished.store(true, Ordering::Release);
            ctx.request_repaint();
        });
        Ok(job)
    }

    pub fn is_done(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Recorre la carpeta (sin seguir enlaces) y manda los archivos a los hilos.
fn walk(root: &Path, exts: &[String], job: &Job, tx: &mpsc::SyncSender<PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if job.cancel.load(Ordering::Relaxed) {
                return;
            }
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name();
            let name = name.to_string_lossy();
            if ft.is_dir() {
                if !name.starts_with('.') && name != "node_modules" {
                    stack.push(e.path());
                }
            } else if ft.is_file() {
                let ok = exts.is_empty()
                    || Path::new(&*name).extension().is_some_and(|x| exts.iter().any(|w| x.eq_ignore_ascii_case(w.as_str())));
                if ok {
                    job.files_seen.fetch_add(1, Ordering::Relaxed);
                    if tx.send(e.path()).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// Líneas de `path` con coincidencia; `None` si no se pudo leer o es binario.
fn search_file(path: &Path, re: &regex::bytes::Regex) -> Option<(Vec<Hit>, bool)> {
    let f = std::fs::File::open(path).ok()?;
    if f.metadata().ok()?.len() == 0 {
        return Some((Vec::new(), false));
    }
    // SAFETY: solo lectura; si otro proceso lo trunca podría haber SIGBUS (conocido).
    let m = unsafe { memmap2::Mmap::map(&f).ok()? };
    let data = &m[..];
    // Binario: hay un byte 0 al principio.
    if memchr::memchr(0, &data[..data.len().min(8192)]).is_some() {
        return None;
    }
    let mut hits = Vec::new();
    let (mut line, mut counted, mut p) = (0u64, 0usize, 0usize);
    while p <= data.len() {
        let Some(mt) = re.find_at(data, p) else { break };
        line += memchr::memchr_iter(b'\n', &data[counted..mt.start()]).count() as u64;
        let start = memchr::memrchr(b'\n', &data[..mt.start()]).map_or(0, |i| i + 1);
        let end = memchr::memchr(b'\n', &data[mt.start()..]).map_or(data.len(), |i| mt.start() + i);
        if hits.len() == MAX_PER_FILE {
            return Some((hits, true));
        }
        let cut = end.min(start + SHOW_BYTES.max(mt.end() - start + 40));
        let lossy = |b: &[u8]| String::from_utf8_lossy(b).replace('\t', "    ");
        let (pre, hit) = (lossy(&data[start..mt.start()]), lossy(&data[mt.start()..mt.end().min(cut)]));
        let text = lossy(&data[start..cut]).trim_end_matches('\r').to_string();
        let a = pre.len().min(text.len());
        let at = (a, (a + hit.len()).min(text.len()));
        hits.push(Hit { line, text, at });
        // Una por línea: seguir en la siguiente.
        counted = mt.start();
        p = end + 1;
        if end < data.len() {
            line += 1;
            counted = end + 1;
        }
    }
    Some((hits, false))
}

/// Lo que la ventana pide a la app.
#[derive(Default)]
pub struct Out {
    pub pick_folder: bool,
    /// Abrir este archivo en esta línea (y resaltar la búsqueda).
    pub open: Option<(PathBuf, u64)>,
}

pub fn window_id() -> Id {
    Id::NULL.with("buscar_en_carpeta")
}

pub struct FolderSearch {
    pub open: bool,
    pub dir: String,
    pub query: String,
    pub regex: bool,
    pub case: bool,
    pub exts: String,
    pub job: Option<Arc<Job>>,
    /// Archivos plegados en la lista.
    collapsed: HashSet<PathBuf>,
    pub focus_dir: bool,
    error: Option<String>,
}

impl Default for FolderSearch {
    fn default() -> Self {
        FolderSearch {
            open: false,
            dir: String::new(),
            query: String::new(),
            regex: false,
            case: false,
            exts: String::new(),
            job: None,
            collapsed: HashSet::new(),
            focus_dir: false,
            error: None,
        }
    }
}

/// Expande `~` y quita comillas (al pegar rutas).
fn expand(dir: &str) -> PathBuf {
    let d = dir.trim().trim_matches(['"', '\'']);
    if let Some(rest) = d.strip_prefix('~') {
        if let Some(h) = std::env::var_os("HOME") {
            return PathBuf::from(h).join(rest.trim_start_matches('/'));
        }
    }
    PathBuf::from(d)
}

impl FolderSearch {
    pub fn is_busy(&self) -> bool {
        self.job.as_ref().is_some_and(|j| !j.is_done())
    }

    pub fn stop(&mut self) {
        if let Some(j) = &self.job {
            j.stop();
        }
    }

    /// Abre la ventana con el foco en «Ir a carpeta».
    pub fn show(&mut self, dir: Option<&Path>) {
        self.open = true;
        if self.dir.is_empty() {
            if let Some(d) = dir {
                self.dir = d.display().to_string();
            }
        }
        self.focus_dir = true;
    }

    fn start(&mut self, ctx: &Context) {
        let root = expand(&self.dir);
        self.error = None;
        if !root.is_dir() {
            self.error = Some(format!("No existe la carpeta «{}»", root.display()));
            return;
        }
        if self.query.is_empty() {
            self.error = Some("Escribe el texto a buscar".into());
            return;
        }
        self.stop();
        let exts: Vec<String> = self.exts.split([',', ' ', ';']).map(|e| e.trim().trim_start_matches('.').to_string()).filter(|e| !e.is_empty()).collect();
        match Job::start(root, &self.query, self.regex, self.case, exts, ctx.clone()) {
            Ok(j) => {
                self.collapsed.clear();
                self.job = Some(j);
            }
            Err(e) => self.error = Some(format!("Expresión no válida: {e}")),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Out {
        let mut out = Out::default();
        let ctx = ui.ctx().clone();
        let mut go = false;
        Grid::new("buscar_carpeta_campos").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label("Ir a carpeta:");
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 130.0).max(200.0);
                let r = ui.add(TextEdit::singleline(&mut self.dir).desired_width(w).hint_text("/ruta/a/la/carpeta  (también ~/…)").font(TextStyle::Monospace));
                if std::mem::take(&mut self.focus_dir) {
                    r.request_focus();
                }
                go |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !self.query.is_empty();
                let exists = !self.dir.trim().is_empty() && expand(&self.dir).is_dir();
                if !self.dir.trim().is_empty() {
                    let (t, c) = if exists { ("✔", Color32::from_rgb(70, 170, 90)) } else { ("✖", ui.visuals().error_fg_color) };
                    ui.colored_label(c, t).on_hover_text(if exists { "La carpeta existe" } else { "No se encuentra la carpeta" });
                }
                if ui.button("📁 Elegir…").clicked() {
                    out.pick_folder = true;
                }
            });
            ui.end_row();

            ui.label("Buscar:");
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 200.0).max(200.0);
                let r = ui.add(TextEdit::singleline(&mut self.query).desired_width(w).hint_text(if self.regex { "expresión regular" } else { "texto" }).font(TextStyle::Monospace));
                go |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                ui.toggle_value(&mut self.case, "Aa").on_hover_text("Distinguir mayúsculas");
                ui.toggle_value(&mut self.regex, ".*").on_hover_text("Expresión regular");
                if self.is_busy() {
                    if ui.button("⏹ Detener").clicked() {
                        self.stop();
                    }
                } else if ui.add_enabled(!self.query.is_empty(), Button::new("🔍 Buscar")).clicked() {
                    go = true;
                }
            });
            ui.end_row();

            ui.label("Extensiones:");
            ui.add(TextEdit::singleline(&mut self.exts).desired_width(260.0).hint_text("todas (texto); ej.: log, txt, xml"))
                .on_hover_text("Solo los archivos con estas extensiones (separadas por comas). Vacío: todos los que no sean binarios.");
            ui.end_row();
        });
        if go {
            self.start(&ctx);
        }
        ui.add_space(4.0);
        // Estado.
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        } else if let Some(j) = &self.job {
            let (seen, done, m) = (j.files_seen.load(Ordering::Relaxed), j.files_done.load(Ordering::Relaxed), j.matches.load(Ordering::Relaxed));
            let nfiles = j.results.lock().unwrap().len();
            ui.horizontal(|ui| {
                if !j.is_done() {
                    ui.spinner();
                }
                let mut s = format!(
                    "{} coincidencias en {} {} · {} de {} archivos revisados",
                    crate::app::fmt_num(m),
                    crate::app::fmt_num(nfiles as u64),
                    if nfiles == 1 { "archivo" } else { "archivos" },
                    crate::app::fmt_num(done),
                    crate::app::fmt_num(seen)
                );
                if j.is_done() {
                    s += &format!(" · {:.1} s", *j.elapsed.lock().unwrap());
                }
                let sk = j.skipped.load(Ordering::Relaxed);
                if sk > 0 {
                    s += &format!(" · {} binarios o ilegibles omitidos", crate::app::fmt_num(sk));
                }
                ui.label(s);
                if j.truncated.load(Ordering::Relaxed) {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("(se paró en {} coincidencias)", crate::app::fmt_num(MAX_TOTAL)));
                }
                if nfiles > 0 {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Expandir todo").clicked() {
                            self.collapsed.clear();
                        }
                        if ui.small_button("Plegar todo").clicked() {
                            self.collapsed = j.results.lock().unwrap().iter().map(|f| f.path.clone()).collect();
                        }
                    });
                }
            });
        } else {
            ui.weak("Escribe o elige una carpeta y el texto, y pulsa Enter.");
        }
        ui.separator();
        // Resultados: filas planas (archivo + sus líneas) para pintar solo las visibles.
        let Some(job) = self.job.clone() else { return out };
        let res = job.results.lock().unwrap();
        let mut rows: Vec<(usize, Option<usize>)> = Vec::new();
        for (fi, f) in res.iter().enumerate() {
            rows.push((fi, None));
            if !self.collapsed.contains(&f.path) {
                rows.extend((0..f.hits.len()).map(|h| (fi, Some(h))));
            }
        }
        let font = TextStyle::Monospace.resolve(ui.style());
        let row_h = ui.text_style_height(&TextStyle::Monospace).max(ui.spacing().interact_size.y);
        let mut toggle = None;
        ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
            for &(fi, h) in &rows[range] {
                let f = &res[fi];
                let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                let v = ui.visuals();
                if r.hovered() {
                    ui.painter().rect_filled(rect, 2.0, v.widgets.hovered.weak_bg_fill);
                }
                match h {
                    None => {
                        let open = !self.collapsed.contains(&f.path);
                        let rel = f.path.strip_prefix(&job.root).unwrap_or(&f.path);
                        let name = rel.display().to_string();
                        let more = if f.more { "+" } else { "" };
                        let txt = format!("{} {name}  ({}{more})", if open { "▾" } else { "▸" }, f.hits.len());
                        ui.painter().text(rect.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER, txt, FontId::proportional(13.0), v.strong_text_color());
                        let r = r.on_hover_text(format!("{}\nClic: plegar/desplegar · Doble clic: abrir", f.path.display()));
                        if r.double_clicked() {
                            out.open = Some((f.path.clone(), f.hits.first().map_or(0, |h| h.line)));
                        } else if r.clicked() {
                            toggle = Some(f.path.clone());
                        }
                    }
                    Some(hi) => {
                        let hit = &f.hits[hi];
                        let num = format!("{:>8}", crate::app::fmt_num(hit.line + 1));
                        let p = ui.painter();
                        p.text(rect.left_center() + vec2(18.0, 0.0), Align2::LEFT_CENTER, num, font.clone(), v.weak_text_color());
                        let x = rect.left() + 18.0 + font.size * 0.62 * 9.0 + 8.0;
                        let mut job = text::LayoutJob::default();
                        let (a, b) = hit.at;
                        let plain = TextFormat::simple(font.clone(), v.text_color());
                        let mark = TextFormat { background: Color32::from_rgb(250, 200, 40), color: Color32::BLACK, ..plain.clone() };
                        let t = &hit.text;
                        if t.is_char_boundary(a) && t.is_char_boundary(b) {
                            job.append(&t[..a], 0.0, plain.clone());
                            job.append(&t[a..b], 0.0, mark);
                            job.append(&t[b..], 0.0, plain);
                        } else {
                            job.append(t, 0.0, plain);
                        }
                        let galley = p.layout_job(job);
                        let clip = Rect::from_min_max(pos2(x, rect.top()), rect.max);
                        ui.painter_at(clip).galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, v.text_color());
                        if r.on_hover_text("Clic: abrir el archivo en esta línea").clicked() {
                            out.open = Some((f.path.clone(), hit.line));
                        }
                    }
                }
            }
        });
        drop(res);
        if let Some(p) = toggle {
            if !self.collapsed.remove(&p) {
                self.collapsed.insert(p);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busca_en_subcarpetas_y_omite_binarios() {
        let dir = std::env::temp_dir().join("buscador-tests").join("carpeta-buscar");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("a.log"), "uno\nERROR dos\ntres error\n").unwrap();
        std::fs::write(dir.join("sub/b.txt"), "nada\r\nError final").unwrap();
        std::fs::write(dir.join("c.bin"), b"ERROR\0\0binario").unwrap();
        std::fs::write(dir.join(".git/d.log"), "ERROR oculto").unwrap();
        let ctx = Context::default();
        let j = Job::start(dir.clone(), "error", false, false, Vec::new(), ctx.clone()).unwrap();
        while !j.is_done() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let r = j.results.lock().unwrap();
        let got: Vec<(String, Vec<(u64, String)>)> = r
            .iter()
            .map(|f| (f.path.strip_prefix(&dir).unwrap().display().to_string(), f.hits.iter().map(|h| (h.line, h.text.clone())).collect()))
            .collect();
        assert_eq!(
            got,
            [
                ("a.log".to_string(), vec![(1, "ERROR dos".to_string()), (2, "tres error".to_string())]),
                ("sub/b.txt".to_string(), vec![(1, "Error final".to_string())]),
            ]
        );
        assert_eq!(j.skipped.load(Ordering::Relaxed), 1);
        // Solo .txt
        let j = Job::start(dir.clone(), "error", false, true, vec!["txt".into()], ctx).unwrap();
        while !j.is_done() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(j.matches.load(Ordering::Relaxed), 0, "Aa: «Error» no es «error»");
    }
}
