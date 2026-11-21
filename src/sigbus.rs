//! Archivos mapeados (`mmap`) que otro programa recorta mientras están abiertos.
//!
//! En macOS y Linux, leer una página de un archivo mapeado que ya no existe
//! (porque otro proceso lo truncó) no da un error: el sistema manda la señal
//! SIGBUS y el programa muere. Aquí se instala un manejador que, si la
//! dirección que falla está dentro de uno de nuestros mapas, pone encima una
//! página de ceros y deja seguir: lo recortado se lee como vacío. La app se
//! entera con [`take`] y vuelve a abrir el archivo (o avisa si tenía cambios).
//!
//! En Windows no hace falta: no deja truncar un archivo mapeado.

use std::fs::File;
use std::ops::Deref;

/// Un archivo mapeado en memoria (solo lectura) protegido frente a SIGBUS.
pub struct Map {
    map: memmap2::Mmap,
    #[cfg(unix)]
    slot: Option<usize>,
}

impl Deref for Map {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.map
    }
}

impl Map {
    /// Lectura en orden aleatorio (no leer por adelantado).
    pub fn advise_random(&self) {
        #[cfg(unix)]
        let _ = self.map.advise(memmap2::Advice::Random);
    }
}

/// Mapea `file` entero. Igual que `Mmap::map`, pero un recorte posterior del
/// archivo no tumba el programa (ver el módulo).
pub fn map(file: &File) -> std::io::Result<Map> {
    // SAFETY: el contenido puede cambiar por debajo (otro proceso escribe); se
    // trata como bytes cualesquiera. El recorte lo cubre el manejador de SIGBUS.
    let map = unsafe { memmap2::Mmap::map(file)? };
    #[cfg(unix)]
    {
        let slot = imp::register(map.as_ptr() as usize, map.len());
        Ok(Map { map, slot })
    }
    #[cfg(not(unix))]
    Ok(Map { map })
}

#[cfg(unix)]
impl Drop for Map {
    fn drop(&mut self) {
        if let Some(s) = self.slot {
            imp::unregister(s);
        }
    }
}

/// Instala el manejador (una vez; se puede llamar varias veces).
pub fn install() {
    #[cfg(unix)]
    imp::install();
}

/// ¿Se leyó algo recortado desde la última vez? (Y lo olvida.)
pub fn take() -> bool {
    #[cfg(unix)]
    {
        imp::HIT.swap(false, std::sync::atomic::Ordering::AcqRel)
    }
    #[cfg(not(unix))]
    false
}

#[cfg(unix)]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const SLOTS: usize = 1024;
    /// Mapas vigentes: (inicio, largo). Largo 0 = hueco libre. Sin cerrojos:
    /// el manejador de la señal solo puede leer atómicos.
    static STARTS: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
    static LENS: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
    static TAKEN: [AtomicBool; SLOTS] = [const { AtomicBool::new(false) }; SLOTS];
    pub static HIT: AtomicBool = AtomicBool::new(false);
    static PAGE: AtomicUsize = AtomicUsize::new(4096);
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    /// Manejador que había antes (el de Rust para desbordes de pila).
    static mut OLD: std::mem::MaybeUninit<libc::sigaction> = std::mem::MaybeUninit::uninit();

    pub fn register(start: usize, len: usize) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let s = (0..SLOTS).find(|&i| TAKEN[i].compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_ok())?;
        STARTS[s].store(start, Ordering::Release);
        LENS[s].store(len, Ordering::Release);
        Some(s)
    }

    pub fn unregister(s: usize) {
        LENS[s].store(0, Ordering::Release);
        TAKEN[s].store(false, Ordering::Release);
    }

    fn ours(addr: usize) -> bool {
        (0..SLOTS).any(|i| {
            let len = LENS[i].load(Ordering::Acquire);
            let start = STARTS[i].load(Ordering::Acquire);
            len > 0 && addr >= start && addr - start < len
        })
    }

    extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
        // SAFETY: lo llama el sistema con un `siginfo_t` válido (SA_SIGINFO).
        let addr = unsafe { (*info).si_addr() } as usize;
        if ours(addr) {
            let page = PAGE.load(Ordering::Relaxed);
            let at = addr & !(page - 1);
            // Una página de ceros encima de la que falta; la lectura se repite y sigue.
            // SAFETY: `at` es una página dentro de un mapa nuestro (de solo lectura).
            let r = unsafe {
                libc::mmap(at as *mut libc::c_void, page, libc::PROT_READ, libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0)
            };
            if r != libc::MAP_FAILED {
                HIT.store(true, Ordering::Release);
                return;
            }
        }
        // No es nuestro: lo que hubiera hecho el manejador anterior.
        // SAFETY: OLD se escribió antes de instalar este manejador.
        unsafe {
            let old = (*std::ptr::addr_of!(OLD)).assume_init_ref();
            let h = old.sa_sigaction;
            if h == libc::SIG_DFL || h == libc::SIG_IGN {
                let mut dfl: libc::sigaction = std::mem::zeroed();
                dfl.sa_sigaction = libc::SIG_DFL;
                libc::sigaction(sig, &dfl, std::ptr::null_mut());
                // Al volver se repite el acceso y el sistema termina el programa.
            } else if old.sa_flags & libc::SA_SIGINFO != 0 {
                let f: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) = std::mem::transmute(h);
                f(sig, info, ctx);
            } else {
                let f: extern "C" fn(libc::c_int) = std::mem::transmute(h);
                f(sig);
            }
        }
    }

    pub fn install() {
        INSTALLED.call_once(|| unsafe {
            let p = libc::sysconf(libc::_SC_PAGESIZE);
            if p > 0 {
                PAGE.store(p as usize, Ordering::Relaxed);
            }
            // Primero se guarda el anterior (el manejador lo necesita desde el primer momento).
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(libc::SIGBUS, std::ptr::null(), &mut old) != 0 {
                return;
            }
            (*std::ptr::addr_of_mut!(OLD)).write(old);
            std::sync::atomic::fence(Ordering::SeqCst);
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handler as *const () as usize;
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(libc::SIGBUS, &sa, std::ptr::null_mut());
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn truncated_file_reads_as_zeros() {
        install();
        let p = std::env::temp_dir().join(format!("buscador-sigbus-{}.bin", std::process::id()));
        std::fs::write(&p, vec![b'x'; 256 * 1024]).unwrap();
        let m = map(&File::open(&p).unwrap()).unwrap();
        assert_eq!(m[200_000], b'x');
        std::fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(1000).unwrap();
        // Sin el manejador, esto mataría el proceso con SIGBUS.
        let v = unsafe { std::ptr::read_volatile(&m[200_000]) };
        assert_eq!(v, 0);
        assert_eq!(m[10], b'x');
        assert!(take());
        drop(m);
        let _ = std::fs::remove_file(&p);
    }
}
