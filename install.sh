#!/usr/bin/env sh
# buscador: instalar, actualizar o desinstalar con un solo comando.
#
#     curl -fsSL https://raw.githubusercontent.com/airylp/buscador/main/install.sh | sh
#
#   Pregunta qué hacer: si ya está instalado ofrece actualizar o desinstalar; si
#   no, instalarlo. También se puede indicar directamente (sin preguntas):
#     … | sh -s -- instalar        (instalar o actualizar)
#     … | sh -s -- compilar        (compilar en este equipo lo último de main)
#     … | sh -s -- desinstalar     (pregunta si borrar también la configuración)
#
#   Por defecto descarga la última versión publicada en GitHub Releases (segundos,
#   sin Rust). Si no hay versión para este equipo (p. ej. Linux ARM) o se pide
#   «compilar», descarga el código y lo compila (instala Rust con rustup si falta).
#   macOS: buscador.app en /Applications (o ~/Applications sin permiso).
#   Linux: ~/.local/bin/buscador con ícono y entrada en el menú de aplicaciones.
#
# Variables opcionales: BUSCADOR_REPO (usuario/repo), BUSCADOR_VERSION (tag, p. ej.
#   v0.2.0; por defecto la más reciente), BUSCADOR_REF (rama o tag a compilar; main),
#   BUSCADOR_DESTINO (carpeta de instalación, para pruebas), BUSCADOR_IDIOMA (es|en|fr),
#   BUSCADOR_SOLO_BINARIO=1 (con «compilar»: solo el ejecutable en la carpeta actual).
set -eu

REPO="${BUSCADOR_REPO:-airylp/buscador}"
REF="${BUSCADOR_REF:-main}"
ACCION="${1:-}"
OS="$(uname)"

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
# Mensajes en el idioma elegido: m "español" "english" "français".
m() { case "${IDIOMA:-es}" in en) printf '%s' "$2" ;; fr) printf '%s' "$3" ;; *) printf '%s' "$1" ;; esac; }

hay_terminal() { (: < /dev/tty) 2>/dev/null; }
leer() { # leer "pregunta" → respuesta en $r (de la terminal: el script llega por la tubería de curl)
    printf '%s' "$1" > /dev/tty
    read -r r < /dev/tty || r=""
}
pregunta() {
    if hay_terminal; then
        leer "$1 $(m '[s/N]' '[y/N]' '[o/N]') "
        case "$r" in s|S|si|sí|Sí|y|Y|yes|o|O|oui) return 0 ;; esac
    fi
    return 1
}

if [ "$OS" = "Darwin" ]; then
    # Con BUSCADOR_DESTINO solo se toca esa carpeta.
    APP_DIRS="${BUSCADOR_DESTINO:-/Applications $HOME/Applications}"
    CONFIG="$HOME/Library/Application Support/buscador"
    CACHE="$HOME/Library/Caches/buscador-compilacion"
    PLATAFORMA="macos-universal"
    EXT="zip"
elif [ "$OS" = "Linux" ]; then
    BIN_DIR="${BUSCADOR_DESTINO:-$HOME/.local/bin}"
    DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
    CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/buscador"
    CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/buscador-compilacion"
    # Solo se publica para x86_64; en otras arquitecturas se compila.
    case "$(uname -m)" in
        x86_64 | amd64) PLATAFORMA="linux-x86_64" ;;
        *) PLATAFORMA="" ;;
    esac
    EXT="tar.gz"
else
    fail "$(m 'En Windows usa install.ps1 (PowerShell: irm https://raw.githubusercontent.com/airylp/buscador/main/install.ps1 | iex) o el instalador de https://github.com/airylp/buscador/releases' 'On Windows use install.ps1 (PowerShell: irm https://raw.githubusercontent.com/airylp/buscador/main/install.ps1 | iex) or the installer from https://github.com/airylp/buscador/releases' "Sous Windows, utilisez install.ps1 (PowerShell : irm https://raw.githubusercontent.com/airylp/buscador/main/install.ps1 | iex) ou l'installateur de https://github.com/airylp/buscador/releases")"
fi
MARCA="$CONFIG/version-instalada"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
BUNDLE_ID="io.github.airylp.buscador"
# Tamaños del ícono en el tema hicolor (Linux).
ICON_SIZES="16 32 64 128 256 512"

# ------------------------------------------------------------ idioma
# BUSCADOR_IDIOMA, el ya guardado en los ajustes de la app o, si hay terminal y no
# se indicó acción, se pregunta (y se guarda para la app).
AJUSTES="$CONFIG/ajustes.txt"
IDIOMA="${BUSCADOR_IDIOMA:-}"
GUARDAR_IDIOMA="$IDIOMA"
[ -n "$IDIOMA" ] || IDIOMA="$(sed -n 's/^idioma=//p' "$AJUSTES" 2>/dev/null | head -n 1)"
if [ -z "$IDIOMA" ]; then
    case "${LC_ALL:-${LANG:-}}" in en*) IDIOMA=en ;; fr*) IDIOMA=fr ;; *) IDIOMA=es ;; esac
    if [ -z "$ACCION" ] && hay_terminal; then
        printf 'Idioma / Language / Langue\n  1) Español\n  2) English\n  3) Français\n' > /dev/tty
        case "$IDIOMA" in en) def=2 ;; fr) def=3 ;; *) def=1 ;; esac
        leer "[1-3] (Enter = $def): "
        case "${r:-$def}" in 2) IDIOMA=en ;; 3) IDIOMA=fr ;; *) IDIOMA=es ;; esac
        GUARDAR_IDIOMA="$IDIOMA"
    fi
fi
case "$IDIOMA" in es|en|fr) ;; *) IDIOMA=es ;; esac
export IDIOMA

# Deja el idioma en los ajustes de la app (sin tocar lo demás).
guardar_idioma() {
    [ -n "$GUARDAR_IDIOMA" ] || return 0
    mkdir -p "$CONFIG"
    if [ -f "$AJUSTES" ]; then grep -v '^idioma=' "$AJUSTES" > "$AJUSTES.tmp" || true; mv "$AJUSTES.tmp" "$AJUSTES"; fi
    echo "idioma=$GUARDAR_IDIOMA" >> "$AJUSTES"
}

# ------------------------------------------------------------ estado
instalado() {
    if [ "$OS" = "Darwin" ]; then
        for d in $APP_DIRS; do [ -d "$d/buscador.app" ] && { echo "$d/buscador.app"; return 0; }; done
        return 1
    fi
    [ -x "$BIN_DIR/buscador" ] && echo "$BIN_DIR/buscador"
}

# Tag de la versión a instalar: la pedida o la más reciente.
version_remota() {
    if [ -n "${BUSCADOR_VERSION:-}" ]; then echo "$BUSCADOR_VERSION"; return; fi
    curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1
}

# ------------------------------------------------------------ desinstalar
desinstalar() {
    ruta="$(instalado || true)"
    if [ -z "$ruta" ]; then
        info "$(m 'buscador no está instalado.' 'buscador is not installed.' "buscador n'est pas installé.")"
    else
        if [ "$OS" = "Darwin" ]; then
            [ -n "${BUSCADOR_DESTINO:-}" ] || osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
            "$LSREGISTER" -u "$ruta" >/dev/null 2>&1 || true
            rm -rf "$ruta"
        else
            rm -f "$BIN_DIR/buscador" "$DATA/applications/buscador.desktop"
            for t in $ICON_SIZES; do rm -f "$DATA/icons/hicolor/${t}x${t}/apps/buscador.png"; done
            [ -d "$DATA/icons/hicolor" ] && touch "$DATA/icons/hicolor"
            command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA/applications" 2>/dev/null || true
        fi
        rm -f "$MARCA"
        info "$(m "Desinstalado: $ruta" "Uninstalled: $ruta" "Désinstallé : $ruta")"
    fi
    rm -rf "$CACHE" "$DATA/buscador/app.ron" "${TMPDIR:-/tmp}/buscador-nuevos" "${TMPDIR:-/tmp}/buscador-actualizar"
    [ "$OS" = "Darwin" ] || rmdir "$DATA/buscador" 2>/dev/null || true # posición de la ventana (eframe)
    if [ -d "$CONFIG" ]; then
        echo "$(m 'Configuración' 'Settings' 'Réglages'): $CONFIG"
        ls -1 "$CONFIG" | sed 's/^/    /'
        if pregunta "$(m '¿Borrar también la configuración (tema, letra, colores…)?' 'Also delete the settings (theme, font, colours…)?' 'Supprimer aussi les réglages (thème, police, couleurs…) ?')"; then
            rm -rf "$CONFIG"
            info "$(m 'Configuración borrada.' 'Settings deleted.' 'Configuration supprimée.')"
        else
            info "$(m 'Configuración conservada.' 'Settings kept.' 'Réglages conservés.')"
        fi
    fi
}

# ------------------------------------------------------------ colocar
# Pone en su sitio (macOS) una buscador.app ya armada.
colocar_app() {
    src="$1"
    destino=""
    for d in $APP_DIRS; do
        mkdir -p "$d" 2>/dev/null || true
        if [ -w "$d" ]; then destino="$d"; break; fi
    done
    [ -n "$destino" ] || fail "$(m "No hay permiso para escribir en: $APP_DIRS" "No permission to write to: $APP_DIRS" "Pas de permission d'écriture dans : $APP_DIRS")"
    reabrir=0
    # BUSCADOR_NO_CERRAR (lo pone el actualizador de la app): no se cierra la app
    # abierta; la versión nueva se abre al pulsar «Reiniciar ahora». Si no, se le
    # pide que se cierre como si lo hiciera el usuario (pregunta si hay cambios).
    if [ -z "${BUSCADOR_NO_CERRAR:-}" ] && [ -z "${BUSCADOR_DESTINO:-}" ] && pgrep -f "buscador.app/Contents/MacOS/buscador" >/dev/null 2>&1; then
        osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
        i=0
        while pgrep -f "buscador.app/Contents/MacOS/buscador" >/dev/null 2>&1 && [ $i -lt 60 ]; do sleep 1; i=$((i + 1)); done
        if pgrep -f "buscador.app/Contents/MacOS/buscador" >/dev/null 2>&1; then
            info "$(m 'buscador sigue abierto: se instala igual; reinícialo después.' 'buscador is still open: installing anyway; restart it afterwards.' 'buscador est toujours ouvert : installation quand même ; relancez-le ensuite.')"
        else
            reabrir=1
        fi
    fi
    ruta="$destino/buscador.app"
    if [ -d "$ruta" ] && command -v rsync >/dev/null 2>&1; then
        # Reemplazar el contenido sin borrar la carpeta: para macOS (Dock,
        # Launchpad) sigue siendo la misma app.
        rsync -a --delete "$src/" "$ruta/"
    else
        rm -rf "$ruta"
        mv "$src" "$ruta"
    fi
    "$LSREGISTER" -f "$ruta" >/dev/null 2>&1 || true
    if [ "$reabrir" = 1 ]; then open "$ruta" || true; fi
}

# Pone el ejecutable en su sitio (Linux) y crea la entrada del menú.
colocar_bin() {
    mkdir -p "$BIN_DIR"
    # Copia + rename: el ejecutable en uso sigue funcionando hasta reiniciarlo.
    cp "$1" "$BIN_DIR/.buscador.nuevo"
    chmod +x "$BIN_DIR/.buscador.nuevo"
    mv -f "$BIN_DIR/.buscador.nuevo" "$BIN_DIR/buscador"
    ruta="$BIN_DIR/buscador"
    if [ -z "${BUSCADOR_DESTINO:-}" ]; then
        # Ícono generado por la propia app; cada PNG se escribe aparte y se renombra
        # para que GNOME nunca lea (y guarde en caché) uno a medio escribir.
        ic="$(mktemp -d)"
        if "$ruta" --iconset "$ic" 2>/dev/null; then
            for t in $ICON_SIZES; do
                case $t in 64) f=icon_32x32@2x.png ;; *) f="icon_${t}x${t}.png" ;; esac
                d="$DATA/icons/hicolor/${t}x${t}/apps"
                mkdir -p "$d" && cp "$ic/$f" "$d/.buscador.png.nuevo" && mv -f "$d/.buscador.png.nuevo" "$d/buscador.png"
            done
            # GNOME/GTK solo vuelve a leer los íconos si cambia la fecha de la carpeta del tema.
            touch "$DATA/icons/hicolor" 2>/dev/null || true
        fi
        rm -rf "$ic"
        mkdir -p "$DATA/applications"
        cat > "$DATA/applications/buscador.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=buscador
Comment=Visor y editor de archivos de varios GB
Comment[en]=Viewer and editor for multi-GB files
Comment[fr]=Visionneuse et éditeur de fichiers de plusieurs Go
Exec=$ruta %F
Icon=buscador
Terminal=false
Categories=Utility;TextEditor;Development;
MimeType=text/plain;text/x-log;application/json;application/xml;
StartupWMClass=buscador
DESKTOP
        command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA/applications" 2>/dev/null || true
    fi
    case ":$PATH:" in *":$BIN_DIR:"*) ;; *) info "$(m "Añade $BIN_DIR a tu PATH para abrirlo con «buscador»." "Add $BIN_DIR to your PATH to run it as “buscador”." "Ajoutez $BIN_DIR à votre PATH pour le lancer avec «buscador».")" ;; esac
}

terminar() { # terminar versión
    mkdir -p "$CONFIG"
    echo "$1" > "$MARCA"
    guardar_idioma
    info "$(m "Instalado $1 en $ruta" "Installed $1 in $ruta" "$1 installé dans $ruta")"
    info "$(m 'Para actualizar más adelante: ⚙ Ajustes → «Buscar actualizaciones…», o vuelve a ejecutar este comando.' 'To update later: ⚙ Settings → “Check for updates…”, or run this command again.' 'Pour mettre à jour plus tard : ⚙ Réglages → « Rechercher des mises à jour… », ou relancez cette commande.')"
    # Para el actualizador de la app (no depende del idioma). Las dos últimas
    # líneas son las que buscan las versiones de la app anteriores a Releases.
    echo "RUTA_INSTALADA=$ruta"
    if [ "$OS" = "Darwin" ]; then echo "==> Instalada en $ruta"; else echo "==> Instalado: $ruta"; fi
}

# ------------------------------------------------------------ instalar (descarga)
instalar() {
    command -v curl >/dev/null 2>&1 || fail "$(m 'Hace falta curl.' 'curl is required.' 'curl est nécessaire.')"
    if [ -z "$PLATAFORMA" ]; then
        info "$(m "No hay versión publicada para $(uname -m): se compila en este equipo." "No published build for $(uname -m): building on this machine." "Pas de version publiée pour $(uname -m) : compilation sur cette machine.")"
        compilar
        return
    fi
    tag="$(version_remota || true)"
    if [ -z "$tag" ]; then
        if pregunta "$(m "No se encontró ninguna versión publicada. ¿Compilar en este equipo?" "No published version found. Build on this machine?" "Aucune version publiée. Compiler sur cette machine ?")"; then compilar; return; fi
        fail "$(m "No se encontró ninguna versión publicada en https://github.com/$REPO/releases" "No published version found at https://github.com/$REPO/releases" "Aucune version publiée sur https://github.com/$REPO/releases")"
    fi
    if [ "$(cat "$MARCA" 2>/dev/null || true)" = "$tag" ] && instalado >/dev/null && [ "$ACCION" != "instalar" ]; then
        info "$(m "Ya tienes la versión más reciente ($tag)." "You already have the latest version ($tag)." "Vous avez déjà la dernière version ($tag).")"
        return 0
    fi
    archivo="buscador-${tag#v}-$PLATAFORMA.$EXT"
    url="https://github.com/$REPO/releases/download/$tag/$archivo"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    info "$(m "Descargando $archivo…" "Downloading $archivo…" "Téléchargement de $archivo…")"
    if ! curl -fL --progress-bar -o "$tmp/$archivo" "$url"; then
        if pregunta "$(m "No se pudo descargar $url. ¿Compilar en este equipo?" "Could not download $url. Build on this machine?" "Impossible de télécharger $url. Compiler sur cette machine ?")"; then compilar; return; fi
        fail "$(m "No se pudo descargar $url" "Could not download $url" "Impossible de télécharger $url")"
    fi
    if [ "$OS" = "Darwin" ]; then
        ditto -x -k "$tmp/$archivo" "$tmp/x"
        [ -d "$tmp/x/buscador.app" ] || fail "$(m 'El archivo descargado no contiene buscador.app' 'The download does not contain buscador.app' "Le téléchargement ne contient pas buscador.app")"
        # Sin firma de Apple: se quita la cuarentena para que abra sin avisos.
        xattr -dr com.apple.quarantine "$tmp/x/buscador.app" 2>/dev/null || true
        colocar_app "$tmp/x/buscador.app"
    else
        tar -xzf "$tmp/$archivo" -C "$tmp"
        colocar_bin "$tmp/buscador-${tag#v}-$PLATAFORMA/buscador"
    fi
    terminar "$tag"
}

# ------------------------------------------------------------ compilar
compilar() {
    command -v curl >/dev/null 2>&1 || fail "$(m 'Hace falta curl.' 'curl is required.' 'curl est nécessaire.')"
    command -v cc >/dev/null 2>&1 || fail "$(m 'Hace falta un compilador de C (cc).' 'A C compiler (cc) is required.' 'Un compilateur C (cc) est nécessaire.')
  Debian/Ubuntu: sudo apt install build-essential
  Fedora:        sudo dnf install gcc
  Arch:          sudo pacman -S base-devel
  macOS:         xcode-select --install"
    # Rust: usa el instalado o lo instala con rustup en ~/.cargo (sin sudo).
    PATH="$HOME/.cargo/bin:$PATH"
    export PATH
    if ! command -v cargo >/dev/null 2>&1; then
        info "$(m 'Rust no encontrado; instalando con rustup en ~/.cargo' 'Rust not found; installing it with rustup in ~/.cargo' 'Rust introuvable ; installation avec rustup dans ~/.cargo')"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
    fi
    # rustup instalado pero sin toolchain por defecto: se elige la estable.
    if ! cargo --version >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then
        rustup default stable || true
    fi
    cargo --version >/dev/null 2>&1 || fail "$(m 'Rust no funciona en este equipo (cargo --version falló).' 'Rust does not work on this machine (cargo --version failed).' 'Rust ne fonctionne pas sur cette machine (cargo --version a échoué).')"
    info "$(m 'Usando' 'Using' 'Utilisation de') $(cargo --version)"
    # Commit que se compila (queda grabado en el programa).
    BUSCADOR_COMMIT="$(curl -fsSL -H 'Accept: application/vnd.github.sha' "https://api.github.com/repos/$REPO/commits/$REF" 2>/dev/null | cut -c1-7 || true)"
    export BUSCADOR_COMMIT
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    info "$(m "Descargando el código ($REF)…" "Downloading the source ($REF)…" "Téléchargement du code ($REF)…")"
    curl -fsSL "https://github.com/$REPO/archive/$REF.tar.gz" | tar -xz -C "$tmp" --strip-components=1 || fail "$(m "No se pudo descargar el código de $REPO ($REF)" "Could not download the source of $REPO ($REF)" "Impossible de télécharger le code de $REPO ($REF)")"
    # La caché acelera las compilaciones siguientes; cargo no borra lo viejo: si
    # pasa de 3 GB se vacía (la próxima empieza de cero).
    if [ -d "$CACHE" ] && [ "$(du -sk "$CACHE" | cut -f1)" -gt 3145728 ]; then
        rm -rf "$CACHE" && info "$(m 'Caché de compilación vaciada (pasaba de 3 GB)' 'Build cache emptied (it was over 3 GB)' 'Cache de compilation vidé (il dépassait 3 Go)')"
    fi
    CARGO_TARGET_DIR="$CACHE"
    export CARGO_TARGET_DIR
    info "$(m 'Compilando (la primera vez tarda unos minutos)…' 'Building (the first time takes a few minutes)…' 'Compilation (la première fois prend quelques minutes)…')"
    (cd "$tmp" && cargo build --release --locked) || fail "$(m 'La compilación falló.' 'The build failed.' 'La compilation a échoué.')"
    if [ "${BUSCADOR_SOLO_BINARIO:-0}" = 1 ]; then
        cp "$CACHE/release/buscador" "./buscador"
        chmod +x ./buscador
        info "$(m 'Listo' 'Done' 'Terminé'): $(pwd)/buscador"
        exit 0
    fi
    if [ "$OS" = "Darwin" ]; then
        # El paquete (ícono, Info.plist, firma local) lo arma empaquetar-mac.sh.
        BINARIO="$CACHE/release/buscador" CARGO_TARGET_DIR="$tmp/target" sh "$tmp/empaquetar-mac.sh" crear
        colocar_app "$tmp/target/release/buscador.app"
    else
        colocar_bin "$CACHE/release/buscador"
    fi
    terminar "$REF${BUSCADOR_COMMIT:+@$BUSCADOR_COMMIT}"
}

# ------------------------------------------------------------ menú
case "$ACCION" in
    instalar | install | actualizar | update) instalar ;;
    compilar | build) compilar ;;
    desinstalar | --desinstalar | uninstall) desinstalar ;;
    "")
        if ruta="$(instalado)"; then
            actual="$(cat "$MARCA" 2>/dev/null || echo '?')"
            if hay_terminal; then
                printf '%s\n  1) %s\n  2) %s\n  3) %s\n' "$(m "buscador $actual ya está instalado ($ruta)." "buscador $actual is already installed ($ruta)." "buscador $actual est déjà installé ($ruta).")" \
                    "$(m 'Actualizar' 'Update' 'Mettre à jour')" "$(m 'Desinstalar' 'Uninstall' 'Désinstaller')" "$(m 'Cancelar' 'Cancel' 'Annuler')" > /dev/tty
                leer "[1-3] (Enter = 1): "
                case "${r:-1}" in
                    1) instalar ;;
                    2) desinstalar ;;
                    *) info "$(m 'Nada que hacer.' 'Nothing to do.' 'Rien à faire.')" ;;
                esac
            else
                instalar
            fi
        elif hay_terminal; then
            if pregunta "$(m '¿Instalar buscador?' 'Install buscador?' 'Installer buscador ?')"; then instalar; else info "$(m 'Cancelado.' 'Cancelled.' 'Annulé.')"; fi
        else
            instalar
        fi
        ;;
    *) fail "$(m "Acción desconocida: $ACCION (usa instalar, compilar o desinstalar)" "Unknown action: $ACCION (use install, build or uninstall)" "Action inconnue : $ACCION (utilisez install, build ou uninstall)")" ;;
esac
