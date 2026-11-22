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
- Ajuste de línea (↩ en la barra de estado o Ctrl+Alt+Z): al ancho de la ventana o a N columnas
- Al abrir se recuperan los archivos de la sesión anterior, cada uno en su posición
- Ajustes (⚙): tema Sistema / Claro / Oscuro, tipo de letra del texto (monoespaciadas como Hack, Menlo,
  SF Mono, Monaco, Consolas… o proporcionales) y tamaño; se recuerdan
- Resaltado de sintaxis según la extensión (Java, Kotlin, JS/TS, Rust, Go, C/C++, C#, Swift, Python, Markdown,
  shell, SQL, JSON, XML/HTML, YAML, TOML/INI, CSS) y en los logs: niveles (ERROR, WARN, INFO, DEBUG),
  fechas, excepciones y trazas. Solo se colorea lo visible; el lenguaje se cambia en la barra de estado
- Embellecer (✨, Ctrl+Shift+B) JSON y XML/HTML: valida que estén bien formados (si no, indica
  línea y columna) y los ordena con sangría; a la selección o a todo el archivo; Ctrl+Z lo deshace
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
- Ir a fecha/hora (Ctrl+G y escribir «2026-11-21 10:30», «21/11/2026», «10:30:15», «Nov 21 10:30»):
  va por bisección a la primera línea de log que llega a ese momento
- Logs: fondo rojo en las líneas ERROR (con sus trazas de pila) y ámbar en WARN (se quita en Ajustes)
- Panel «Detalle»: si la línea lleva JSON, se ve embellecido y con colores
- Convertir el archivo (clic en el encoding o en LF/CRLF de la barra de estado): otro encoding,
  saltos LF ↔ CRLF, poner o quitar el BOM; en streaming, sirve para varios GB, y avisa en qué línea
  hay un carácter que no se puede representar
- «Acerca de» (ℹ en la barra de estado): versión, licencia y enlaces al proyecto en GitHub

## Instalar, actualizar y desinstalar

Todo con un solo comando (macOS; instala Rust con rustup si hace falta):

```sh
curl -fsSL https://raw.githubusercontent.com/airylp/buscador/main/install.sh | sh
```

Si buscador no está instalado, pregunta si instalarlo en /Applications; si ya lo está, ofrece
**actualizar** o **desinstalar** (y al desinstalar pregunta si borrar también la configuración).
Sin preguntas: `… | sh -s -- instalar` o `… | sh -s -- desinstalar`.

Una vez instalada, se actualiza desde la propia app: ⚙ Ajustes → «Buscar actualizaciones…»
(o el menú buscador). Descarga la última versión, la compila y reemplaza la app; luego ofrece
reiniciar.

Requisitos: `curl` y las herramientas de línea de comandos de Xcode (`xcode-select --install`).

### Windows

Descarga `buscador-…-setup.exe` de la última versión en
[Releases](https://github.com/airylp/buscador/releases/latest) y ábrelo, o en PowerShell:

```powershell
[Net.ServicePointManager]::SecurityProtocol = 'Tls12'; irm https://raw.githubusercontent.com/airylp/buscador/main/install.ps1 | iex
```

Se instala para tu usuario (no pide permisos de administrador), con acceso en el menú Inicio,
«Abrir con» y, opcionalmente, «Abrir con buscador» en el clic derecho de los archivos. Se
desinstala desde Configuración → Aplicaciones. Las actualizaciones se instalan desde la app
(⚙ Ajustes → «Buscar actualizaciones…»).

El instalador no está firmado: si Windows muestra «Windows protegió su PC», pulsa
«Más información» → «Ejecutar de todas formas».

### Desde el código

Desde una copia del repositorio también se puede usar `./empaquetar-mac.sh` (menú: crear,
instalar, actualizar, desinstalar) o compilar a mano:

```sh
git clone https://github.com/airylp/buscador && cd buscador
cargo build --release
./target/release/buscador archivo.log
```

## Limitaciones conocidas

- Guardar necesita espacio libre para una copia completa del archivo, salvo cuando solo se
  añadió texto al final de un archivo grande (entonces se escribe solo lo nuevo).
- «Reemplazar todo» en memoria (con deshacer) admite hasta 2 M de líneas; con más, se reemplaza
  mientras se escribe el archivo (sin RAM, pero sin deshacer).
- Las líneas de más de 16 KB se pintan a trozos (el que se ve); fuera de UTF-8 solo se ve su principio.
  Sin ajuste de línea se recorren enteras; con ajuste, solo los primeros 16 KB.

## Licencia

MIT. Ver [LICENSE](LICENSE).
