//! Logotipo de la aplicación: su letra (B, C, P o K) hecha de cubos en 3D, con la
//! textura de bloque «pixelada» de los juegos de cubos, sobre un cuadrado
//! redondeado totalmente blanco. Se genera en código (sin archivos de imagen) a
//! cualquier tamaño: ventana, Dock, barra de tareas, `.icns` de macOS y `.ico`
//! de Windows.
//!
//! ARCHIVO COMÚN a buscador, conector y peticiones (`utilerias/comun`): no se
//! edita aquí, sino en `comun/` y se copia con `comun/sincronizar.sh`. Lo propio
//! de cada aplicación está en `crate::APP` (main.rs).

#![allow(dead_code)]

use eframe::egui::IconData;

/// Identidad de cada aplicación (la usan el logotipo, «Acerca de» y el actualizador).
pub struct Marca {
    /// Nombre en minúsculas (= nombre del binario y del repositorio).
    pub nombre: &'static str,
    /// Descripción corta, ya traducida (un `fn` que llama a la macro de traducción).
    pub descripcion: fn() -> &'static str,
    /// La letra en una cuadrícula de 5×7 (`#` = cubo).
    pub letra: [&'static str; 7],
    /// Color de la cara frontal de los cubos.
    pub color: [u8; 3],
    /// Tecnologías («Hecho con»).
    pub hecho_con: &'static str,
}

pub const LETRA_B: [&str; 7] = ["####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."];
pub const LETRA_C: [&str; 7] = [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."];
pub const LETRA_P: [&str; 7] = ["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."];
pub const LETRA_K: [&str; 7] = ["#...#", "#..##", "#.##.", "###..", "#.##.", "#..##", "#...#"];
pub const LETRA_D: [&str; 7] = ["###..", "#..#.", "#...#", "#...#", "#...#", "#..#.", "###.."];

/// Colores de las aplicaciones: bien distintos entre sí.
pub const AZUL: [u8; 3] = [0x2f, 0x7c, 0xf6];
pub const VERDE: [u8; 3] = [0x1f, 0xb3, 0x55];
pub const NARANJA: [u8; 3] = [0xf5, 0x7a, 0x14];
pub const MORADO: [u8; 3] = [0x8e, 0x4b, 0xe6];
pub const ROJO: [u8; 3] = [0xe1, 0x3b, 0x4f];

/// Grosor extra de la letra: cada cubo crece esta fracción de celda por lado.
const BOLD: f32 = 0.18;

/// Tamaño del ícono de la ventana: 512 px (nítido en Retina).
const SIZE: usize = 512;

fn shade(c: [u8; 3], f: f32) -> [f32; 3] {
    // f > 1 aclara hacia el blanco; f < 1 oscurece.
    let mut o = [0.0; 3];
    for k in 0..3 {
        let v = c[k] as f32;
        o[k] = if f >= 1.0 { v + (255.0 - v) * (f - 1.0) } else { v * f };
    }
    o
}

/// Ruido fijo por «texel» (cuadradito de la textura de bloque), en −1..1.
fn noise(a: u32, b: u32, c: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77) ^ c.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 32_767.5 - 1.0
}

/// Lienzo con sobremuestreo (`ss` muestras por lado de cada píxel).
struct Canvas {
    w: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    /// Rellena el polígono convexo `pts` (en píxeles del lienzo) con `color(x, y)`.
    fn fill(&mut self, pts: &[(f32, f32)], color: &dyn Fn(f32, f32) -> [f32; 3]) {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let n = pts.len();
        let w = self.w as isize;
        for y in (y0.floor() as isize).max(0)..=(y1.ceil() as isize).min(w - 1) {
            for x in (x0.floor() as isize).max(0)..=(x1.ceil() as isize).min(w - 1) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                // Dentro si está al mismo lado de todas las aristas.
                let mut pos = true;
                let mut neg = true;
                for i in 0..n {
                    let (ax, ay) = pts[i];
                    let (bx, by) = pts[(i + 1) % n];
                    let cr = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
                    pos &= cr >= 0.0;
                    neg &= cr <= 0.0;
                }
                if pos || neg {
                    let c = color(px, py);
                    self.px[y as usize * self.w + x as usize] = [c[0], c[1], c[2], 1.0];
                }
            }
        }
    }
}

/// RGBA del logotipo de `m` a `size` px (con su sombra: no añadir otra al dibujarlo).
fn render_marca(m: &Marca, size: usize) -> Vec<u8> {
    // Sobremuestreo: bordes suaves sin depender de nada más.
    let ss = if size <= 64 { 4 } else if size <= 256 { 3 } else { 2 };
    let w = size * ss;
    let s = w as f32;
    // Márgenes y radio como los íconos de macOS (contenido ≈ 82 % del lienzo).
    let margin = s * 0.09;
    let half = (s - 2.0 * margin) / 2.0;
    let radius = (s - 2.0 * margin) * 0.225;
    let center = s / 2.0;
    let mut cv = Canvas { w, px: vec![[0.0; 4]; w * w] };
    // Cuadrado redondeado blanco (alfa 1 dentro; el borde se suaviza al reducir).
    let inside = |px: f32, py: f32| {
        let qx = (px - center).abs() - (half - radius);
        let qy = (py - center).abs() - (half - radius);
        (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius
    };
    // Sombra suave bajo el cuadrado (parece que flota): el mismo cuadrado algo más
    // abajo, difuminado, en el margen transparente. En 16 px solo ensuciaría.
    let (sombra_y, difuso, opacidad) = (s * 0.022, s * 0.028, if size >= 24 { 0.42 } else { 0.0 });
    for y in 0..w {
        for x in 0..w {
            let d = inside(x as f32 + 0.5, y as f32 + 0.5 - sombra_y);
            let a = opacidad / (1.0 + (d / (difuso * 0.45)).exp());
            if a > 0.002 {
                cv.px[y * w + x] = [12.0, 14.0, 22.0, a];
            }
        }
    }
    for y in 0..w {
        for x in 0..w {
            let d = inside(x as f32 + 0.5, y as f32 + 0.5);
            if d <= 0.0 {
                // Filo gris muy suave: se distingue sobre fondos blancos.
                let edge = (-d / (s * 0.010)).clamp(0.0, 1.0);
                let g = 255.0 - (1.0 - edge) * 38.0;
                cv.px[y * w + x] = [g, g, g, 1.0];
            }
        }
    }

    // La letra: 5×7 cubos, extruidos hacia arriba a la izquierda (proyección oblicua):
    // se ve girada hacia la derecha, con las caras de arriba y de la izquierda.
    let (cols, rows) = (5.0, 7.0);
    let depth = 0.55; // desplazamiento de la cara trasera, en celdas
    // Letra gruesa: cada cubo mide 1 + 2·BOLD celdas y se solapa con sus vecinos
    // (trazos más anchos, huecos más estrechos).
    let cell = (s - 2.0 * margin) * 0.68 / (rows + depth + 2.0 * BOLD);
    let cs = cell * (1.0 + 2.0 * BOLD);
    let lw = (cols + depth + 2.0 * BOLD) * cell;
    let lh = (rows + depth + 2.0 * BOLD) * cell;
    // Centrado óptico (la sombra queda abajo).
    let ox = center - lw / 2.0 + (depth + BOLD) * cell;
    let oy = center - lh / 2.0 + (depth + BOLD) * cell - cell * 0.15;
    let (dx, dy) = (-depth * cell, -depth * cell);
    let on = |r: isize, c: isize| {
        (0..7).contains(&r) && (0..5).contains(&c) && m.letra[r as usize].as_bytes()[c as usize] == b'#'
    };
    let base = m.color;
    // Textura: cada cara de 8×8 texels con brillo variable (se omite en tamaños
    // pequeños, donde solo sería ruido).
    let texels = if size >= 64 { 8.0 } else { 0.0 };
    let tex = move |col: [f32; 3], u: f32, v: f32, face: u32, r: isize, c: isize| -> [f32; 3] {
        if texels == 0.0 {
            return col;
        }
        let (tu, tv) = ((u * texels).floor().clamp(0.0, texels - 1.0) as u32, (v * texels).floor().clamp(0.0, texels - 1.0) as u32);
        let n = noise(tu + 16 * face, tv + 16 * r as u32, c as u32 * 31 + 7);
        let mut k = 1.0 + n * 0.07;
        // Biselado: borde de 1 texel más claro arriba/izquierda y más oscuro abajo/derecha.
        if tu == 0 || tv == 0 {
            k *= 1.10;
        }
        if tu as f32 == texels - 1.0 || tv as f32 == texels - 1.0 {
            k *= 0.86;
        }
        [(col[0] * k).min(255.0), (col[1] * k).min(255.0), (col[2] * k).min(255.0)]
    };

    // Sombra en el suelo blanco (abajo, algo desplazada a la izquierda).
    let sh = [206.0, 209.0, 218.0];
    for r in 0..7isize {
        for c in 0..5isize {
            if on(r, c) {
                let x = ox + (c as f32 - BOLD) * cell - cell * 0.30;
                let y = oy + (r as f32 - BOLD) * cell + cell * 0.30;
                cv.fill(&[(x, y), (x + cs, y), (x + cs, y + cs), (x, y + cs)], &|_, _| sh);
            }
        }
    }
    let top = shade(base, 1.38);
    let side = shade(base, 0.68);
    let front = shade(base, 1.0);
    // 1) Caras de arriba y de la izquierda (solo las que dan al exterior), de
    //    derecha a izquierda para que las más cercanas queden encima.
    for r in 0..7isize {
        for c in (0..5isize).rev() {
            if !on(r, c) {
                continue;
            }
            let x = ox + (c as f32 - BOLD) * cell;
            let y = oy + (r as f32 - BOLD) * cell;
            if !on(r - 1, c) {
                let p = [(x, y), (x + cs, y), (x + cs + dx, y + dy), (x + dx, y + dy)];
                cv.fill(&p, &|px, py| {
                    let v = (y - py) / -dy; // 0 delante … 1 detrás
                    let u = (px - x - v * dx) / cs;
                    tex(top, u, 1.0 - v, 1, r, c)
                });
            }
            if !on(r, c - 1) {
                let p = [(x, y), (x + dx, y + dy), (x + dx, y + cs + dy), (x, y + cs)];
                cv.fill(&p, &|px, py| {
                    let u = (px - x) / dx;
                    let v = (py - y - u * dy) / cs;
                    tex(side, u, v, 2, r, c)
                });
            }
        }
    }
    // 2) Caras frontales encima de todo.
    for r in 0..7isize {
        for c in 0..5isize {
            if !on(r, c) {
                continue;
            }
            let x = ox + (c as f32 - BOLD) * cell;
            let y = oy + (r as f32 - BOLD) * cell;
            cv.fill(&[(x, y), (x + cs, y), (x + cs, y + cs), (x, y + cs)], &|px, py| {
                tex(front, (px - x) / cs, (py - y) / cs, 0, r, c)
            });
        }
    }

    // Reducir: media de las muestras (premultiplicada por alfa).
    let mut out = vec![0u8; size * size * 4];
    let n = (ss * ss) as f32;
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let p = cv.px[(y * ss + sy) * w + x * ss + sx];
                    for k in 0..3 {
                        acc[k] += p[k] * p[3];
                    }
                    acc[3] += p[3];
                }
            }
            let a = acc[3] / n;
            let i = (y * size + x) * 4;
            if acc[3] > 0.0 {
                for k in 0..3 {
                    out[i + k] = (acc[k] / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
            }
            out[i + 3] = (a * 255.0).round() as u8;
        }
    }
    out
}

fn render(size: usize) -> Vec<u8> {
    render_marca(&crate::APP, size)
}

/// Píxeles RGBA del ícono a `size`×`size` (para la ventana «Acerca de»).
pub fn rgba(size: usize) -> Vec<u8> {
    render(size)
}

pub fn app_icon() -> IconData {
    let size = SIZE;
    IconData { rgba: render(size), width: size as u32, height: size as u32 }
}

/// PNG de RGBA (sin dependencias: deflate sin comprimir).
pub fn png_of(rgba: &[u8], size: usize) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    let mut raw = Vec::with_capacity(size * (size * 4 + 1));
    for row in rgba.chunks(size * 4) {
        raw.push(0); // filtro «ninguno»
        raw.extend_from_slice(row);
    }
    // zlib con bloques «stored».
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits, RGBA
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// El ícono como PNG.
pub fn png(size: usize) -> Vec<u8> {
    png_of(&render(size), size)
}

/// Escribe en `dir` los PNG que pide `iconutil` para crear el `.icns`.
pub fn write_iconset(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for s in [16, 32, 128, 256, 512] {
        std::fs::write(dir.join(format!("icon_{s}x{s}.png")), png(s))?;
        std::fs::write(dir.join(format!("icon_{s}x{s}@2x.png")), png(s * 2))?;
    }
    Ok(())
}

/// Ícono de Windows (`.ico` con PNG de 16 a 256 px). Se guarda en
/// `windows/<nombre>.ico` (`<nombre> --ico ruta`); un test vigila que esté al día.
pub fn ico() -> Vec<u8> {
    const SIZES: [usize; 7] = [16, 24, 32, 48, 64, 128, 256];
    let pngs: Vec<Vec<u8>> = SIZES.iter().map(|&s| png(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len();
    for (&s, p) in SIZES.iter().zip(&pngs) {
        let b = if s >= 256 { 0 } else { s as u8 }; // 0 = 256
        out.extend_from_slice(&[b, b, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planos
        out.extend_from_slice(&32u16.to_le_bytes()); // bits por píxel
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += p.len();
    }
    for p in &pngs {
        out.extend_from_slice(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_size_and_shape() {
        let ic = app_icon();
        assert_eq!((ic.width, ic.height, ic.rgba.len()), (512, 512, 512 * 512 * 4));
        assert_eq!(ic.rgba[3], 0, "esquina transparente");
        // Fondo blanco y opaco junto al borde interior.
        let i = (60 * 512 + 256) * 4;
        assert_eq!(&ic.rgba[i..i + 4], &[255, 255, 255, 255]);
        let p = png(32);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        // LOGO_PNG=carpeta guarda los logotipos de las tres aplicaciones para verlos.
        if let Ok(dir) = std::env::var("LOGO_PNG") {
            for (n, l, c) in [("b", LETRA_B, AZUL), ("c", LETRA_C, VERDE), ("p", LETRA_P, NARANJA), ("k", LETRA_K, MORADO), ("d", LETRA_D, ROJO)] {
                let m = Marca { nombre: n, descripcion: || "", letra: l, color: c, hecho_con: "" };
                for s in [512, 64, 32, 16] {
                    std::fs::write(format!("{dir}/{n}{s}.png"), png_of(&render_marca(&m, s), s)).unwrap();
                }
            }
        }
    }

    /// El `.ico` del instalador de Windows corresponde al ícono actual.
    /// Si cambia el ícono: `cargo run --release -- --ico windows/<nombre>.ico`.
    #[test]
    fn windows_ico_up_to_date() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows");
        if !dir.is_dir() {
            return; // esta aplicación aún no tiene instalador de Windows
        }
        let path = dir.join(format!("{}.ico", crate::APP.nombre));
        let ico = ico();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 7, 0]);
        assert!(
            std::fs::read(&path).is_ok_and(|f| f == ico),
            "{} está desactualizado: cargo run --release -- --ico {}",
            path.display(),
            path.display()
        );
    }
}
