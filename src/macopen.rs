//! Abrir archivos desde Finder («Abrir con…», arrastrar al ícono del Dock).
//!
//! macOS entrega esos archivos llamando a `application:openURLs:` del delegado
//! de la app, pero el delegado de winit no lo implementa y entonces macOS dice
//! que «buscador no puede abrir archivos de este tipo». Aquí se añade ese método
//! en tiempo de ejecución a `NSObject` (de quien hereda el delegado de winit,
//! que aún no existe al arrancar) antes de que empiece el bucle de la app, así
//! llegan también los archivos con los que se lanza. Las rutas se dejan en una
//! cola que la app vacía en cada fotograma (`take`).
//!
//! Además, `empaquetar-mac.sh` declara en Info.plist que la app abre cualquier
//! archivo (`public.data`, rango «Alternate»: no se vuelve la predeterminada).

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;
use objc2_foundation::{NSArray, NSURL};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

/// Instala el método. Llamar en `main`, antes de `eframe::run_native`.
pub fn install() {
    let Some(cls) = AnyClass::get(c"NSObject") else { return };
    let imp: unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>) = open_urls;
    // SAFETY: la firma coincide con la codificación «v@:@@» (void, self, _cmd, id, id).
    unsafe {
        objc2::ffi::class_addMethod(
            cls as *const AnyClass as *mut AnyClass,
            sel!(application:openURLs:),
            std::mem::transmute::<_, Imp>(imp),
            c"v@:@@".as_ptr(),
        );
    }
}

/// Para despertar la app cuando llegan archivos con la ventana en reposo.
pub fn set_context(ctx: &eframe::egui::Context) {
    let _ = CTX.set(ctx.clone());
}

/// Archivos recibidos desde la última llamada.
pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

unsafe extern "C-unwind" fn open_urls(_this: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *mut NSArray<NSURL>) {
    // Un pánico aquí abortaría la app (viene de Objective-C).
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: AppKit pasa un NSArray<NSURL> válido durante la llamada.
        let Some(urls) = (unsafe { urls.as_ref() }) else { return };
        let mut got = Vec::new();
        for u in urls.iter() {
            if let Some(p) = u.to_file_path() {
                got.push(p);
            }
        }
        if got.is_empty() {
            return;
        }
        PENDING.lock().unwrap_or_else(|e| e.into_inner()).extend(got);
        if let Some(c) = CTX.get() {
            c.request_repaint();
        }
    }));
}
