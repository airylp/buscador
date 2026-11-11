# buscador

Visor y editor de archivos de texto/logs de varios GB con interfaz gráfica,
escrito en Rust (egui). El archivo se abre con `mmap` y un índice disperso de
líneas, así que no se carga en RAM: un log de 2.3 GB / 22 M de líneas usa ~3 MB
de memoria propia. Al editar, sólo las líneas modificadas ocupan memoria.

- Varios archivos, con un panel lateral ocultable (Ctrl+B)
- Búsqueda en segundo plano: texto o regex, con/sin mayúsculas (Ctrl+F, Enter/F3, Shift+F3)
- Encodings: UTF-8, UTF-16 LE/BE (con o sin BOM), Latin-1/Windows-1252, Shift_JIS, GBK, Big5… detección automática y cambio manual
- Edición (Ctrl+E): cursor y selección por carácter, deshacer/rehacer (Ctrl+Z, Ctrl+Shift+Z),
  cortar/copiar/pegar, palabra a palabra, IME (acentos, CJK). Los archivos de más de 64 MB
  se abren en solo lectura; la edición se activa con un clic
- Guardar (Ctrl+S) y Guardar como (Ctrl+Shift+S) en streaming a un temporal + renombrado:
  el original nunca queda a medio escribir. Conserva encoding, BOM y CRLF/LF
- Reemplazar (Ctrl+H; Cmd+Alt+F en macOS) uno a uno o en todas las líneas encontradas, con `$1` en modo regex
- Selección de líneas completas haciendo clic en los números de línea
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
- Guardar necesita espacio libre para una copia completa del archivo. Al guardar se
  reinicia el historial de deshacer (así se libera la RAM de las ediciones).
- «Reemplazar todo» admite hasta 2 M de líneas modificadas de una vez (quedan en RAM hasta guardar).
- No sigue el archivo en vivo (`tail -f`).
- Cada línea se pinta hasta 16 KB; el panel Detalle muestra hasta 1 MB.
