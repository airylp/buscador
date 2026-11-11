# buscador

Visor de logs de varios GB con interfaz gráfica, escrito en Rust (egui).
El archivo se abre con `mmap` y un índice disperso de líneas, así que no se
carga en RAM: un log de 2.3 GB / 22 M de líneas usa ~3 MB de memoria propia.

- Varios archivos, con un panel lateral ocultable (Ctrl+B)
- Búsqueda en segundo plano: texto o regex, con/sin mayúsculas (Ctrl+F, Enter/F3, Shift+F3)
- Encodings: UTF-8, UTF-16 LE/BE (con o sin BOM), Latin-1/Windows-1252, Shift_JIS, GBK, Big5… detección automática y cambio manual
- Selección de líneas (clic, Shift+clic, arrastrar, Shift+flechas) y Ctrl+C
- Panel «Detalle» para ver y copiar parte de una línea larga
- Ir a línea (Ctrl+G), tamaño de letra (Ctrl+ / Ctrl−), arrastrar y soltar archivos

## Instalar

Descarga, compila y deja `./buscador` en la carpeta actual (instala Rust con rustup si hace falta):

```sh
curl -fsSL https://raw.githubusercontent.com/airylp/buscador/main/install.sh | sh
```

Requisitos: `curl`, `tar` y un compilador C (`build-essential` en Debian/Ubuntu).

O manualmente:

```sh
git clone https://github.com/airylp/buscador && cd buscador
cargo build --release
./target/release/buscador archivo.log
```

## Limitaciones conocidas

- Si otro proceso trunca/rota el archivo mientras está abierto, el programa puede cerrarse.
- No sigue el archivo en vivo (`tail -f`).
- Cada línea se pinta hasta 16 KB; el panel Detalle muestra hasta 1 MB.
