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
pub const LETRA_K: [&str; 7] = ["#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"];

/// Colores de las aplicaciones: bien distintos entre sí.
pub const AZUL: [u8; 3] = [0x2f, 0x7c, 0xf6];
pub const VERDE: [u8; 3] = [0x1f, 0xb3, 0x55];
pub const NARANJA: [u8; 3] = [0xf5, 0x7a, 0x14];
pub const MORADO: [u8; 3] = [0x8e, 0x4b, 0xe6];

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

/// Postura de la letra (grados): acostada hacia atrás, girada y vista desde arriba.
const TILT: f32 = 20.0;
const YAW: f32 = 30.0;
const ELEV: f32 = 30.0;
/// Hacia dónde está la luz (mundo: x derecha, y arriba, z hacia atrás).
const LIGHT: V3 = [-0.5, 1.3, -0.15];
/// Desenfoque (en cubos) e intensidad de la sombra.
const BLUR: f32 = 0.14;
const SHADOW: f32 = 0.24;
/// Corrección del encuadre (en cubos): la sombra pesa a un lado.
const SHIFT_X: f32 = 0.15;
const SHIFT_Y: f32 = -0.35;

type V3 = [f32; 3];
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: V3) -> V3 {
    mul(a, 1.0 / dot(a, a).sqrt())
}

/// Desenfoque de caja de radio `r` sobre filas (`h`) o columnas.
fn blur(v: &mut [f32], w: usize, r: usize, h: bool) {
    let mut line = vec![0.0f32; w];
    let mut acc = vec![0.0f32; w + 1];
    for a in 0..w {
        for b in 0..w {
            line[b] = if h { v[a * w + b] } else { v[b * w + a] };
        }
        for b in 0..w {
            acc[b + 1] = acc[b] + line[b];
        }
        for b in 0..w {
            let (lo, hi) = (b.saturating_sub(r), (b + r + 1).min(w));
            let m = (acc[hi] - acc[lo]) / (2 * r + 1) as f32;
            if h { v[a * w + b] = m } else { v[b * w + a] = m }
        }
    }
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

/// RGBA del logotipo de `m` a `size` px.
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

    // La letra: 5×7 cubos en 3D, algo acostada hacia atrás y girada para verse de
    // lado, sobre el suelo blanco, donde proyecta una sombra corta. Se traza un rayo
    // por muestra (proyección ortogonal); los cubos son cajas en el espacio de la
    // letra (x 0..5, y 0..7 de abajo arriba, z 0..D hacia atrás).
    let depth = 1.0f32;
    let (tilt, yaw, elev) = (TILT.to_radians(), YAW.to_radians(), ELEV.to_radians());
    let (st, ct, sy, cy) = (tilt.sin(), tilt.cos(), yaw.sin(), yaw.cos());
    // Ejes de la letra en el mundo (inclinación sobre x y luego giro sobre y).
    let ex: V3 = [cy, 0.0, -sy];
    let ey: V3 = [st * sy, ct, st * cy];
    let ez: V3 = [ct * sy, -st, ct * cy];
    let to_world = |p: V3| add(add(mul(ex, p[0]), mul(ey, p[1])), mul(ez, p[2]));
    let c = to_world([2.5, 3.5, depth / 2.0]);
    let tr: V3 = [-c[0], st * depth, -c[2]]; // apoyada en el suelo (y = 0)
    let to_local = |w: V3| {
        let q = sub(w, tr);
        [dot(ex, q), dot(ey, q), dot(ez, q)]
    };
    let dir_local = |d: V3| [dot(ex, d), dot(ey, d), dot(ez, d)];
    // Cámara: mira hacia +z, algo desde arriba.
    let fwd: V3 = [0.0, -elev.sin(), elev.cos()];
    let up: V3 = [0.0, elev.cos(), elev.sin()];
    let on = |r: isize, c: isize| {
        (0..7).contains(&r) && (0..5).contains(&c) && m.letra[r as usize].as_bytes()[c as usize] == b'#'
    };
    let mut cubes: Vec<(isize, isize, V3, V3)> = Vec::new();
    for r in 0..7isize {
        for c in 0..5isize {
            if on(r, c) {
                cubes.push((r, c, [c as f32, (6 - r) as f32, 0.0], [c as f32 + 1.0, (7 - r) as f32, depth]));
            }
        }
    }
    // Encuadre: la letra (y un poco de su sombra) ocupa ≈ 66 % del lienzo.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(_, _, lo, hi) in &cubes {
        for k in 0..8 {
            let p = to_world(add([if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }], tr));
            let (sx, sy) = (p[0], dot(p, up));
            x0 = x0.min(sx);
            x1 = x1.max(sx);
            y0 = y0.min(sy);
            y1 = y1.max(sy);
        }
    }
    let scale = (s - 2.0 * margin) * 0.66 / (x1 - x0).max(y1 - y0);
    let (mx, my) = ((x0 + x1) / 2.0 + SHIFT_X, (y0 + y1) / 2.0 + SHIFT_Y);
    let light = norm(LIGHT);
    let light_l = dir_local(light);
    let fwd_l = dir_local(fwd);
    // Primer cubo que toca el rayo (origen y dirección en el espacio de la letra).
    let hit = |o: V3, d: V3| -> Option<(f32, usize, usize, bool)> {
        let mut best: Option<(f32, usize, usize, bool)> = None;
        for (i, &(_, _, lo, hi)) in cubes.iter().enumerate() {
            let (mut tn, mut tf, mut axis, mut neg) = (f32::MIN, f32::MAX, 0, false);
            let mut miss = false;
            for k in 0..3 {
                if d[k].abs() < 1e-9 {
                    if o[k] < lo[k] || o[k] > hi[k] {
                        miss = true;
                        break;
                    }
                    continue;
                }
                let (a, b) = ((lo[k] - o[k]) / d[k], (hi[k] - o[k]) / d[k]);
                let (a, b, n) = if a < b { (a, b, true) } else { (b, a, false) };
                if a > tn {
                    tn = a;
                    axis = k;
                    neg = n; // entra por la cara «lo» de este eje
                }
                tf = tf.min(b);
            }
            if miss || tn > tf || tf < 1e-4 {
                continue;
            }
            if best.is_none_or(|b| tn < b.0) {
                best = Some((tn, i, axis, neg));
            }
        }
        best
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
    // Tono de cada cara (luz de arriba a la izquierda): frente, arriba, derecha,
    // izquierda, abajo, atrás.
    let faces = [shade(base, 1.0), shade(base, 1.38), shade(base, 0.66), shade(base, 0.82), shade(base, 0.55), shade(base, 0.6)];
    let mut cube_px: Vec<Option<[f32; 3]>> = vec![None; w * w];
    let mut shadow = vec![0.0f32; w * w];
    for yy in 0..w {
        for xx in 0..w {
            let i = yy * w + xx;
            if cv.px[i][3] == 0.0 {
                continue;
            }
            let sx = (xx as f32 + 0.5 - center) / scale + mx;
            let syy = -(yy as f32 + 0.5 - center) / scale + my;
            let o = add(add([sx, 0.0, 0.0], mul(up, syy)), mul(fwd, -100.0));
            let ol = to_local(o);
            if let Some((t, ci, axis, neg)) = hit(ol, fwd_l) {
                let (r, c, lo, _) = cubes[ci];
                let h = add(ol, mul(fwd_l, t));
                let (fx, fy, fz) = (h[0] - lo[0], h[1] - lo[1], h[2] / depth);
                let (face, u, v) = match (axis, neg) {
                    (2, true) => (0, fx, 1.0 - fy),
                    (1, false) => (1, fx, fz),
                    (0, false) => (2, fz, 1.0 - fy),
                    (0, true) => (3, 1.0 - fz, 1.0 - fy),
                    (1, true) => (4, fx, 1.0 - fz),
                    _ => (5, 1.0 - fx, 1.0 - fy),
                };
                cube_px[i] = Some(tex(faces[face], u, v, face as u32, r, c));
            }
            // Sombra en el suelo (y = 0) aunque un cubo lo tape: así el difuminado
            // no deja un hueco junto a la base.
            if fwd[1] < 0.0 {
                let g = add(o, mul(fwd, -o[1] / fwd[1]));
                if let Some((t, ..)) = hit(to_local(g), light_l) {
                    shadow[i] = (-t * 0.22).exp();
                }
            }
        }
    }
    // Sombra suave: dos pasadas de caja horizontales y verticales.
    let rad = ((scale * BLUR) as usize).max(1);
    for _ in 0..2 {
        blur(&mut shadow, w, rad, true);
        blur(&mut shadow, w, rad, false);
    }
    for i in 0..w * w {
        if cv.px[i][3] == 0.0 {
            continue;
        }
        if let Some(col) = cube_px[i] {
            cv.px[i] = [col[0], col[1], col[2], 1.0];
        } else {
            let k = shadow[i] * SHADOW;
            let p = &mut cv.px[i];
            p[0] *= 1.0 - k;
            p[1] *= 1.0 - k * 0.94;
            p[2] *= 1.0 - k * 0.78;
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
            for (n, l, c) in [("b", LETRA_B, AZUL), ("c", LETRA_C, VERDE), ("p", LETRA_P, NARANJA), ("k", LETRA_K, MORADO)] {
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
