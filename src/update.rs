//! Actualizar buscador desde la propia interfaz (macOS): comprueba si hay
//! una versión nueva en GitHub, descarga el código a una carpeta temporal, lo
//! compila (con caché, para que la siguiente vez tarde menos) y reemplaza la
//! app instalada usando `empaquetar-mac.sh`. La app abierta sigue funcionando
//! hasta que se reinicia.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

pub const REPO: &str = "https://github.com/airylp/buscador";

/// Repositorio de donde actualizar (`BUSCADOR_REPO` lo cambia, para pruebas).
fn repo() -> String {
    std::env::var("BUSCADOR_REPO").unwrap_or_else(|_| REPO.to_string())
}

/// Commit con el que se compiló (lo graba `empaquetar-mac.sh`).
pub fn current_commit() -> Option<&'static str> {
    option_env!("BUSCADOR_COMMIT").filter(|s| !s.is_empty())
}

pub fn version_label() -> String {
    match current_commit() {
        Some(c) => format!("{} ({c})", env!("CARGO_PKG_VERSION")),
        None => format!("{} (compilación local)", env!("CARGO_PKG_VERSION")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prueba completa (tarda: compila todo). Clona este repositorio local e
    /// instala en una carpeta temporal:
    /// BUSCADOR_REPO=$PWD BUSCADOR_DESTINO=/tmp/x cargo test --release actualizar -- --ignored --nocapture
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

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    Checking,
    /// Hay una versión más nueva (commit remoto).
    Available(String),
    UpToDate(String),
    Running,
    /// Instalada: ruta de la app nueva.
    Done(PathBuf),
    Failed(String),
}

pub struct Updater {
    pub stage: Arc<Mutex<Stage>>,
    pub log: Arc<Mutex<Vec<String>>>,
}

/// PATH con cargo y Homebrew: al abrir la app desde Finder el PATH es mínimo.
fn path_env() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_default())
}

/// Carpeta donde instalar: la de la app que está corriendo, si es una .app.
fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"))?;
    bundle.parent().map(PathBuf::from)
}

impl Updater {
    /// Empieza comprobando la versión publicada.
    pub fn check(ctx: eframe::egui::Context) -> Self {
        let u = Updater { stage: Arc::new(Mutex::new(Stage::Checking)), log: Default::default() };
        let stage = u.stage.clone();
        std::thread::spawn(move || {
            let out = Command::new("git").args(["ls-remote", &repo(), "HEAD"]).env("PATH", path_env()).output();
            let new = match out {
                Ok(o) if o.status.success() => {
                    let s = String::from_utf8_lossy(&o.stdout);
                    match s.split_whitespace().next().filter(|h| h.len() >= 7) {
                        Some(h) => {
                            let short = &h[..7];
                            if current_commit().is_some_and(|c| short.starts_with(c) || c.starts_with(short)) {
                                Stage::UpToDate(short.to_string())
                            } else {
                                Stage::Available(short.to_string())
                            }
                        }
                        None => Stage::Failed("respuesta inesperada de GitHub".into()),
                    }
                }
                Ok(o) => Stage::Failed(format!("no se pudo consultar GitHub: {}", String::from_utf8_lossy(&o.stderr).trim())),
                Err(e) => Stage::Failed(format!("no se pudo ejecutar git: {e}")),
            };
            *stage.lock().unwrap() = new;
            ctx.request_repaint();
        });
        u
    }

    /// Descarga, compila e instala (en segundo plano, con registro).
    pub fn run(&self, ctx: eframe::egui::Context) {
        *self.stage.lock().unwrap() = Stage::Running;
        self.log.lock().unwrap().clear();
        let (stage, log) = (self.stage.clone(), self.log.clone());
        // `BUSCADOR_DESTINO` (pruebas) o la carpeta de la app que está corriendo.
        let dest = std::env::var_os("BUSCADOR_DESTINO").map(PathBuf::from).or_else(install_dir);
        std::thread::spawn(move || {
            let home = std::env::var("HOME").unwrap_or_default();
            let repo = repo();
            let dest_arg = dest.as_ref().map(|d| format!(" --destino '{}'", d.display())).unwrap_or_default();
            let script = format!(
                r#"set -e
command -v cargo >/dev/null 2>&1 || {{ echo "Falta Rust (cargo). Instálalo con: curl https://sh.rustup.rs -sSf | sh"; exit 1; }}
T="${{TMPDIR:-/tmp}}/buscador-actualizar"
rm -rf "$T"; mkdir -p "$T"
echo "==> Descargando el código en $T…"
if command -v git >/dev/null 2>&1; then
    git clone --depth 1 --progress "{repo}" "$T/src" 2>&1
else
    curl -fsSL "{repo}/archive/refs/heads/main.tar.gz" | tar xz -C "$T"
    mv "$T"/buscador-main "$T/src"
fi
cd "$T/src"
export CARGO_TARGET_DIR="{home}/Library/Caches/buscador-compilacion"
sh ./empaquetar-mac.sh instalar --si --no-cerrar{dest_arg}
cd /
rm -rf "$T"
echo "==> Temporales borrados"
"#
            );
            let child = Command::new("sh")
                .args(["-c", &format!("( {script} ) 2>&1")])
                .env("PATH", path_env())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    *stage.lock().unwrap() = Stage::Failed(format!("no se pudo iniciar: {e}"));
                    ctx.request_repaint();
                    return;
                }
            };
            let mut installed = None;
            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    // git escribe el progreso con \r: quedarse con lo último.
                    let line = line.rsplit('\r').next().unwrap_or("").to_string();
                    if let Some(p) = line.strip_prefix("==> Instalada en ") {
                        installed = Some(PathBuf::from(p.trim()));
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
