//! Actualizar buscador desde la propia interfaz (macOS y Linux).
//!
//! Comprueba en GitHub si hay una versión nueva y, si se pide, ejecuta el
//! mismo `install.sh` que se usa para instalar: descarga el código a una
//! carpeta temporal, lo compila (con caché, para que la siguiente vez tarde
//! menos) y reemplaza la app instalada. La app abierta sigue funcionando hasta
//! que se reinicia.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// Repositorio en GitHub (usuario/repo). `BUSCADOR_REPO` lo cambia.
fn repo() -> String {
    std::env::var("BUSCADOR_REPO").unwrap_or_else(|_| "airylp/buscador".to_string())
}

/// ¿Se puede actualizar desde la app en esta plataforma?
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// Commit con el que se compiló (lo graban `install.sh` / `empaquetar-mac.sh`).
pub fn current_commit() -> Option<&'static str> {
    option_env!("BUSCADOR_COMMIT").filter(|s| !s.is_empty())
}

pub fn version_label() -> String {
    match current_commit() {
        Some(c) => format!("{} ({c})", env!("CARGO_PKG_VERSION")),
        None => format!("{} (compilación local)", env!("CARGO_PKG_VERSION")),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    Checking,
    /// Hay una versión más nueva (commit remoto).
    Available(String),
    UpToDate(String),
    Running,
    /// Instalada: ruta de la app (macOS) o del ejecutable (Linux).
    Done(PathBuf),
    Failed(String),
}

pub struct Updater {
    pub stage: Arc<Mutex<Stage>>,
    pub log: Arc<Mutex<Vec<String>>>,
}

/// PATH con cargo y Homebrew: al abrir la app desde el escritorio el PATH es mínimo.
fn path_env() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_default())
}

/// Dónde instalar: la carpeta de la .app que corre (macOS) o del ejecutable
/// (Linux, si no está dentro de un `target/` de compilación).
fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if cfg!(target_os = "macos") {
        let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"))?;
        return bundle.parent().map(PathBuf::from);
    }
    let dir = exe.parent()?;
    if dir.components().any(|c| c.as_os_str() == "target") {
        return None;
    }
    Some(dir.to_path_buf())
}

/// Quita los códigos de color ANSI de una línea.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' && it.peek() == Some(&'[') {
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Updater {
    /// Empieza comprobando la versión publicada.
    pub fn check(ctx: eframe::egui::Context) -> Self {
        let u = Updater { stage: Arc::new(Mutex::new(Stage::Checking)), log: Default::default() };
        let stage = u.stage.clone();
        std::thread::spawn(move || {
            let url = format!("https://api.github.com/repos/{}/commits/main", repo());
            let out = Command::new("curl")
                .args(["-fsSL", "-H", "Accept: application/vnd.github.sha", &url])
                .env("PATH", path_env())
                .output();
            let new = match out {
                Ok(o) if o.status.success() => {
                    let h = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if h.len() >= 7 && h.chars().all(|c| c.is_ascii_hexdigit()) {
                        let short = h[..7].to_string();
                        if current_commit().is_some_and(|c| short.starts_with(c) || c.starts_with(&short)) {
                            Stage::UpToDate(short)
                        } else {
                            Stage::Available(short)
                        }
                    } else {
                        Stage::Failed("respuesta inesperada de GitHub".into())
                    }
                }
                Ok(o) => Stage::Failed(format!("no se pudo consultar GitHub: {}", String::from_utf8_lossy(&o.stderr).trim())),
                Err(e) => Stage::Failed(format!("no se pudo ejecutar curl: {e}")),
            };
            *stage.lock().unwrap() = new;
            ctx.request_repaint();
        });
        u
    }

    /// Descarga, compila e instala con `install.sh` (en segundo plano, con registro).
    pub fn run(&self, ctx: eframe::egui::Context) {
        *self.stage.lock().unwrap() = Stage::Running;
        self.log.lock().unwrap().clear();
        let (stage, log) = (self.stage.clone(), self.log.clone());
        // `BUSCADOR_DESTINO` (pruebas) o donde está instalada la que corre.
        let dest = std::env::var_os("BUSCADOR_DESTINO").map(PathBuf::from).or_else(install_dir);
        std::thread::spawn(move || {
            // `BUSCADOR_INSTALL_SH` (pruebas) usa un install.sh local en vez del de GitHub.
            let script = match std::env::var("BUSCADOR_INSTALL_SH") {
                Ok(p) => format!("exec 2>&1\nsh '{p}' instalar"),
                Err(_) => format!(
                    "exec 2>&1\nset -e\nF=\"${{TMPDIR:-/tmp}}/buscador-install.sh\"\n# Se borra siempre, también si algo falla.\ntrap 'rm -f \"$F\"' EXIT INT TERM\ncurl -fsSL https://raw.githubusercontent.com/{}/main/install.sh -o \"$F\"\nsh \"$F\" instalar",
                    repo()
                ),
            };
            let mut cmd = Command::new("sh");
            cmd.args(["-c", &script]).env("PATH", path_env()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
            if let Some(d) = &dest {
                cmd.env("BUSCADOR_DESTINO", d);
            }
            // La app se reinicia sola al terminar («Reiniciar ahora»): el script no la cierra.
            cmd.env("BUSCADOR_NO_CERRAR", "1");
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    *stage.lock().unwrap() = Stage::Failed(format!("no se pudo iniciar: {e}"));
                    ctx.request_repaint();
                    return;
                }
            };
            let mut installed = None;
            if let Some(out) = child.stdout.take() {
                // Lectura tolerante: bytes no UTF-8 no cortan el registro (si se dejara
                // de leer, el proceso moriría por SIGPIPE). El progreso usa \r.
                let mut r = BufReader::new(out);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match r.read_until(b'\n', &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let text = strip_ansi(&String::from_utf8_lossy(&buf));
                    let line = text.trim_end_matches(['\n', '\r']).rsplit('\r').next().unwrap_or("").to_string();
                    // macOS (empaquetar-mac.sh) y Linux (install.sh) anuncian dónde quedó.
                    if let Some(p) = line.strip_prefix("==> Instalada en ") {
                        installed = Some(PathBuf::from(p.trim()));
                    } else if let Some(p) = line.strip_prefix("==> Instalado: ") {
                        installed = Some(PathBuf::from(p.split(" (").next().unwrap_or(p).trim()));
                    }
                    let mut l = log.lock().unwrap();
                    l.push(line);
                    if l.len() > 5000 {
                        l.drain(..1000);
                    }
                    drop(l);
                    ctx.request_repaint();
                }
            }
            let ok = child.wait().map(|s| s.success()).unwrap_or(false);
            *stage.lock().unwrap() = match (ok, installed) {
                (true, Some(p)) => Stage::Done(p),
                (true, None) => Stage::Failed("terminó sin instalar la app".into()),
                (false, _) => Stage::Failed("falló (mira el registro)".into()),
            };
            ctx.request_repaint();
        });
    }

    pub fn stage(&self) -> Stage {
        self.stage.lock().unwrap().clone()
    }
}

/// Abre la versión recién instalada en cuanto termine este proceso: un `sh`
/// aparte espera a que muera el PID actual (así no conviven las dos y la sesión
/// ya está guardada) y entonces la abre. En macOS, con `open` y sin `-n`: abrir
/// otra instancia del mismo .app mientras la vieja seguía viva no funcionaba.
pub fn relaunch_after_exit(installed: &std::path::Path) -> std::io::Result<()> {
    let open = if cfg!(target_os = "macos") { "exec open \"$2\"" } else { "exec \"$2\"" };
    let script = format!("i=0; while kill -0 \"$1\" 2>/dev/null && [ $i -lt 150 ]; do sleep 0.2; i=$((i+1)); done; {open}");
    let mut c = Command::new("/bin/sh");
    c.arg("-c").arg(script).arg("sh").arg(std::process::id().to_string()).arg(installed);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0); // que no le lleguen las señales del grupo de la app
    }
    c.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El relanzador espera a que termine el proceso y entonces abre el programa.
    #[cfg(target_os = "linux")]
    #[test]
    fn relanzar_al_salir() {
        let dir = std::env::temp_dir().join(format!("buscador-relanzar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (prog, marca) = (dir.join("nuevo.sh"), dir.join("abierto"));
        std::fs::write(&prog, format!("#!/bin/sh\necho si > '{}'\n", marca.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Un hijo hace de «app vieja»: relanza y sale a los 0.5 s.
        let exe = std::env::current_exe().unwrap();
        let mut viejo = Command::new(exe)
            .args(["--exact", "update::tests::hijo_relanza", "--ignored", "--nocapture"])
            .env("BUSCADOR_RELANZAR", &prog)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!marca.exists(), "se abrió antes de que terminara la vieja");
        viejo.wait().unwrap();
        let t = std::time::Instant::now();
        while !marca.exists() && t.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(marca.exists(), "no se abrió la versión nueva");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[ignore]
    fn hijo_relanza() {
        let Ok(p) = std::env::var("BUSCADOR_RELANZAR") else { return };
        relaunch_after_exit(std::path::Path::new(&p)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
    }

    #[test]
    fn ansi_codes_removed() {
        assert_eq!(strip_ansi("\u{1b}[1;34m==>\u{1b}[0m Instalado: /x/buscador"), "==> Instalado: /x/buscador");
    }

    /// Prueba completa (tarda: compila todo) con un install.sh local, instalando
    /// en una carpeta temporal:
    /// BUSCADOR_INSTALL_SH=$PWD/install.sh BUSCADOR_DESTINO=/tmp/x \
    ///   cargo test --release actualizar -- --ignored --nocapture
    #[test]
    #[ignore]
    fn actualizar_de_punta_a_punta() {
        let ctx = eframe::egui::Context::default();
        let u = Updater::check(ctx.clone());
        while u.stage() == Stage::Checking {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        println!("comprobación: {:?}", u.stage());
        u.run(ctx);
        let mut shown = 0;
        loop {
            {
                let log = u.log.lock().unwrap();
                for l in &log[shown..] {
                    println!("  | {l}");
                }
                shown = log.len();
            }
            match u.stage() {
                Stage::Running => std::thread::sleep(std::time::Duration::from_millis(200)),
                s => {
                    println!("final: {s:?}");
                    assert!(matches!(s, Stage::Done(_)));
                    break;
                }
            }
        }
    }
}
