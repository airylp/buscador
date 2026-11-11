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
- Búsqueda avanzada (Ctrl+Shift+F): bloques «desde… hasta…» que pueden ocupar varias líneas,
  patrones multilínea opcionales, minimapa del archivo con los bloques en verde (más intenso =
  más bloques), y copia de uno o todos los bloques: incluidos los delimitadores, solo lo de en
  medio o líneas completas. Puedes abrir varias a la vez, cada una sobre un archivo, y sacarlas
  a ventanas propias (otro monitor). Escape cierra la ventana emergente de delante
- Clic derecho en un archivo: mostrar en Finder/Explorador, copiar ruta o nombre, duplicar
  (incluye los cambios sin guardar), cerrar los demás
- Ícono propio (una «B» pixelada) en Cmd+Tab, el Dock y la barra de tareas
- En macOS, menú nativo en la barra superior (Archivo, Edición, Buscar, Ver, Ventana, Ayuda)
- Botón «?» (o F1) con todos los atajos de teclado
- Ajustes (⚙): tema Sistema / Claro / Oscuro, tipo de letra del texto (monoespaciadas como Hack, Menlo,
  SF Mono, Monaco, Consolas… o proporcionales) y tamaño; se recuerdan
- Resaltado de sintaxis según la extensión (Java, Kotlin, JS/TS, Rust, Go, C/C++, C#, Swift, Python,
  shell, SQL, JSON, XML/HTML, YAML, TOML/INI, CSS) y en los logs: niveles (ERROR, WARN, INFO, DEBUG),
  fechas, excepciones y trazas. Solo se colorea lo visible; el lenguaje se cambia en la barra de estado
- Archivo nuevo (📝, Ctrl+N) para pegar y guardar lo copiado; también «Nuevo archivo con la selección»
  (clic derecho) y «A archivo nuevo» en la búsqueda avanzada. Guardar pide dónde
- Selección en bloque (botón ▦ Bloque, o Opción/Alt+arrastrar), con letra monoespaciada: marca un
  rectángulo como una matriz, incluidos los huecos tras el final de las líneas; al copiar solo va el
  texto real de cada fila, separado por saltos de línea
- Colores propios de fondo y letra para el texto (prioridad sobre el tema) y «Restablecer ajustes»
- En macOS también funcionan Ctrl+C / Ctrl+X / Ctrl+V además de Cmd
- Barra de estado: clic en «Lín X, Col Y» (o Ctrl+G) para ir a una línea; botones Resultados y Detalle
- Panel «Detalle» para ver y copiar parte de una línea larga
- Ir a línea (Ctrl+G), tamaño de letra (Ctrl+ / Ctrl−), arrastrar y soltar archivos

## Instalar

Descarga, compila y deja `./buscador` en la carpeta actual (instala Rust con rustup si hace falta):

```sh
curl -fsSL https://raw.githubusercontent.com/airylp/buscador/main/install.sh | sh
```

Requisitos: `curl`, `tar` y un compilador C (`build-essential` en Debian/Ubuntu).

En macOS, para tener `buscador.app` (con ícono en el Dock, Launchpad y Cmd+Tab):

```sh
./empaquetar-mac.sh             # crea target/release/buscador.app
./empaquetar-mac.sh --instalar  # además la copia a ~/Applications
```

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
