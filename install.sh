#!/usr/bin/env sh
# buscador: instalar, actualizar o desinstalar con un solo comando.
#
#     curl -fsSL https://raw.githubusercontent.com/airylp/buscador/main/install.sh | sh
#
#   Pregunta qué hacer: si ya está instalado ofrece actualizar o desinstalar; si
#   no, instalarlo. También se puede indicar directamente (sin preguntas):
#     … | sh -s -- instalar        (instalar o actualizar)
#     … | sh -s -- desinstalar     (pregunta si borrar también la configuración)
#
#   macOS: crea buscador.app (con ícono) en /Applications (o ~/Applications sin permiso).
#   Linux: instala ~/.local/bin/buscador con ícono y entrada en el menú de aplicaciones.
#
# Variables opcionales: BUSCADOR_REPO (usuario/repo), BUSCADOR_REF (rama o tag),
#   BUSCADOR_DESTINO (carpeta de instalación), BUSCADOR_SOLO_BINARIO=1 (solo el
#   ejecutable en la carpeta actual, sin instalar nada).
set -eu

REPO="${BUSCADOR_REPO:-airylp/buscador}"
REF="${BUSCADOR_REF:-main}"
ACCION="${1:-}"
OS="$(uname)"

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
# Mensajes en el idioma elegido: m "español" "english" "français".
m() { case "${IDIOMA:-es}" in en) printf '%s' "$2" ;; fr) printf '%s' "$3" ;; *) printf '%s' "$1" ;; esac; }

# Preguntas: se leen de la terminal (el script llega por la tubería de curl).
pregunta() {
    # ¿Hay terminal de verdad? (abrir /dev/tty falla si se ejecuta sin ella).
    if (: < /dev/tty) 2>/dev/null; then
        printf '%s %s ' "$1" "$(m '[s/N]' '[y/N]' '[o/N]')" > /dev/tty
        read -r r < /dev/tty || r=""
        case "$r" in s|S|si|sí|Sí|y|Y|yes|o|O|oui) return 0 ;; esac
    fi
    return 1
}

if [ "$OS" = "Darwin" ]; then
    # Con BUSCADOR_DESTINO solo se toca esa carpeta.
    APP_DIRS="${BUSCADOR_DESTINO:-/Applications $HOME/Applications}"
    CONFIG="$HOME/Library/Application Support/buscador"
    CACHE="$HOME/Library/Caches/buscador-compilacion"
else
    BIN_DIR="${BUSCADOR_DESTINO:-$HOME/.local/bin}"
    DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
    CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/buscador"
    CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/buscador-compilacion"
fi

hay_terminal() { (: < /dev/tty) 2>/dev/null; }
leer() { # leer "pregunta" → respuesta en $r
    printf '%s' "$1" > /dev/tty
    read -r r < /dev/tty || r=""
}

# ------------------------------------------------------------ idioma
# BUSCADOR_IDIOMA=es|en|fr, o el ya guardado en los ajustes de la app; si no
# hay ninguno y hay terminal, se pregunta (y se guarda para la app).
IDIOMA="${BUSCADOR_IDIOMA:-}"
GUARDAR_IDIOMA="$IDIOMA"
[ -n "$IDIOMA" ] || IDIOMA="$(sed -n 's/^idioma=//p' "$CONFIG/ajustes.txt" 2>/dev/null | head -n 1)"
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
    f="$CONFIG/ajustes.txt"
    if [ -f "$f" ]; then grep -v '^idioma=' "$f" > "$f.tmp" || true; mv "$f.tmp" "$f"; fi
    echo "idioma=$IDIOMA" >> "$f"
}

# ------------------------------------------------------------ ¿qué hacer?
# Sin acción indicada y con terminal, se pregunta. (La actualización desde la
# app indica «instalar» y no pregunta nada.)
if [ -z "$ACCION" ]; then
    ACCION=instalar
    if hay_terminal; then
        instalada=""
        if [ "$OS" = "Darwin" ]; then
            for d in $APP_DIRS; do
                [ -d "$d/buscador.app" ] && { instalada="$d/buscador.app"; break; }
            done
            [ -n "$instalada" ] && version=$(defaults read "$instalada/Contents/Info" CFBundleVersion 2>/dev/null || echo "?")
        elif [ -x "$BIN_DIR/buscador" ]; then
            instalada="$BIN_DIR/buscador"
            version="?"
        fi
        if [ -n "$instalada" ]; then
            {
                echo "$(m "buscador ya está instalado: $instalada (versión $version)" "buscador is already installed: $instalada (version $version)" "buscador est déjà installé : $instalada (version $version)")"
                echo "  1) $(m 'Actualizar a la última versión' 'Update to the latest version' 'Mettre à jour vers la dernière version')"
                echo "  2) $(m 'Desinstalar' 'Uninstall' 'Désinstaller')"
                echo "  3) $(m 'Cancelar' 'Cancel' 'Annuler')"
            } > /dev/tty
            leer "$(m 'Elige [1-3] (Enter = 1): ' 'Choose [1-3] (Enter = 1): ' 'Choisissez [1-3] (Entrée = 1) : ')"
            case "${r:-1}" in
                1) ACCION=instalar ;;
                2) ACCION=desinstalar ;;
                *) m "Cancelado." "Cancelled." "Annulé."; echo; exit 0 ;;
            esac
        else
            destino="/Applications"
            [ "$OS" = "Darwin" ] || destino="$BIN_DIR"
            leer "$(m "Se va a instalar buscador en ${BUSCADOR_DESTINO:-$destino}. ¿Continuar? [S/n] " "buscador will be installed in ${BUSCADOR_DESTINO:-$destino}. Continue? [Y/n] " "buscador va être installé dans ${BUSCADOR_DESTINO:-$destino}. Continuer ? [O/n] ")"
            case "$r" in n|N|no|No|non|Non) m "Cancelado." "Cancelled." "Annulé."; echo; exit 0 ;; esac
        fi
    fi
fi

# ------------------------------------------------------------ desinstalar
if [ "$ACCION" = "desinstalar" ] || [ "$ACCION" = "--desinstalar" ]; then
    if [ "$OS" = "Darwin" ]; then
        for d in $APP_DIRS; do
            if [ -d "$d/buscador.app" ]; then
                [ -n "${BUSCADOR_DESTINO:-}" ] || osascript -e 'tell application id "io.github.airylp.buscador" to quit' >/dev/null 2>&1 || true
                /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$d/buscador.app" >/dev/null 2>&1 || true
                rm -rf "${d:?}/buscador.app" && info "$(m "Quitada $d/buscador.app" "Removed $d/buscador.app" "Supprimée : $d/buscador.app")"
            fi
        done
    else
        for f in "$BIN_DIR/buscador" "$DATA/applications/buscador.desktop" "$DATA/icons/hicolor/256x256/apps/buscador.png"; do
            if [ -e "$f" ]; then rm -f "$f" && info "$(m "Quitado $f" "Removed $f" "Supprimé : $f")"; fi
        done
        command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA/applications" 2>/dev/null || true
    fi
    rm -rf "$CACHE" "${TMPDIR:-/tmp}/buscador-nuevos" "${TMPDIR:-/tmp}/buscador-actualizar"
    if [ -d "$CONFIG" ]; then
        echo "$(m 'Configuración' 'Settings' 'Réglages'): $CONFIG"
        ls -1 "$CONFIG" | sed 's/^/    /'
        if pregunta "$(m '¿Borrar también la configuración (tema, letra, colores…)?' 'Also delete the settings (theme, font, colours…)?' 'Supprimer aussi les réglages (thème, police, couleurs…) ?')"; then
            rm -rf "$CONFIG" && info "$(m 'Configuración borrada' 'Settings deleted' 'Réglages supprimés')"
        else
            m "Configuración conservada." "Settings kept." "Réglages conservés."; echo
        fi
    fi
    info "$(m 'buscador desinstalado' 'buscador uninstalled' 'buscador désinstallé')"
    exit 0
fi
[ "$ACCION" = "instalar" ] || [ "$ACCION" = "actualizar" ] || fail "$(m "acción desconocida: $ACCION (usa instalar o desinstalar)" "unknown action: $ACCION (use instalar or desinstalar)" "action inconnue : $ACCION (utilisez instalar ou desinstalar)")"

# ------------------------------------------------------------ requisitos
command -v curl >/dev/null 2>&1 || fail "$(m 'se necesita curl' 'curl is required' 'curl est nécessaire')"
command -v tar  >/dev/null 2>&1 || fail "$(m 'se necesita tar' 'tar is required' 'tar est nécessaire')"
command -v cc   >/dev/null 2>&1 || fail "$(m 'se necesita un compilador/enlazador C (cc).' 'a C compiler/linker (cc) is required.' 'un compilateur/éditeur de liens C (cc) est nécessaire.')
  Debian/Ubuntu: sudo apt install build-essential
  Fedora:        sudo dnf install gcc
  Arch:          sudo pacman -S base-devel
  macOS:         xcode-select --install"

# Rust: usa el instalado o lo instala con rustup en ~/.cargo (sin sudo).
PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null 2>&1; then
    info "$(m 'Rust no encontrado; instalando con rustup en ~/.cargo' 'Rust not found; installing it with rustup in ~/.cargo' 'Rust introuvable ; installation avec rustup dans ~/.cargo')"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
fi
export PATH
info "$(m 'Usando' 'Using' 'Utilisation de') $(cargo --version)"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

info "$(m 'Descargando' 'Downloading' 'Téléchargement de') $REPO@$REF"
curl -fsSL "https://github.com/$REPO/archive/$REF.tar.gz" | tar -xz -C "$TMP" --strip-components=1
# Versión exacta (commit) para «Buscar actualizaciones» de la app.
BUSCADOR_COMMIT="$(curl -fsSL -H 'Accept: application/vnd.github.sha' "https://api.github.com/repos/$REPO/commits/$REF" 2>/dev/null | cut -c1-7 || true)"
export BUSCADOR_COMMIT
# Caché de compilación: las actualizaciones siguientes tardan mucho menos.
CARGO_TARGET_DIR="$CACHE"
export CARGO_TARGET_DIR

if [ "${BUSCADOR_SOLO_BINARIO:-0}" = 1 ]; then
    info "$(m 'Compilando (la primera vez tarda unos minutos)' 'Building (the first time takes a few minutes)' 'Compilation (la première fois prend quelques minutes)')"
    (cd "$TMP" && cargo build --release --locked)
    cp "$CACHE/release/buscador" "./buscador"
    chmod +x ./buscador
    info "$(m 'Listo' 'Done' 'Terminé'): $(pwd)/buscador"
    exit 0
fi

if [ "$OS" = "Darwin" ]; then
    info "$(m 'Compilando y creando buscador.app (la primera vez tarda unos minutos)' 'Building buscador.app (the first time takes a few minutes)' 'Compilation de buscador.app (la première fois prend quelques minutes)')"
    # Si buscador está abierto, pregunta (en la terminal) si cerrarlo y reabrirlo;
    # desde «Buscar actualizaciones» de la propia app no se cierra (BUSCADOR_NO_CERRAR=1).
    sh "$TMP/empaquetar-mac.sh" instalar --si ${BUSCADOR_NO_CERRAR:+--no-cerrar} ${BUSCADOR_DESTINO:+--destino "$BUSCADOR_DESTINO"}
    guardar_idioma
    info "$(m 'Listo. Abre buscador desde Launchpad o Spotlight (Cmd+Espacio, «buscador»).' 'Done. Open buscador from Launchpad or Spotlight (Cmd+Space, “buscador”).' 'Terminé. Ouvrez buscador depuis Launchpad ou Spotlight (Cmd+Espace, « buscador »).')"
else
    info "$(m 'Compilando (la primera vez tarda unos minutos)' 'Building (the first time takes a few minutes)' 'Compilation (la première fois prend quelques minutes)')"
    (cd "$TMP" && cargo build --release --locked)
    mkdir -p "$BIN_DIR" "$DATA/applications" "$DATA/icons/hicolor/256x256/apps"
    # Copia y renombra: el ejecutable en uso sigue funcionando hasta reiniciarlo.
    cp "$CACHE/release/buscador" "$BIN_DIR/.buscador.nuevo"
    chmod +x "$BIN_DIR/.buscador.nuevo"
    mv -f "$BIN_DIR/.buscador.nuevo" "$BIN_DIR/buscador"
    "$BIN_DIR/buscador" --iconset "$TMP/iconos" && cp "$TMP/iconos/icon_256x256.png" "$DATA/icons/hicolor/256x256/apps/buscador.png"
    cat > "$DATA/applications/buscador.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=buscador
Comment=Visor y editor de archivos de varios GB
Comment[en]=Viewer and editor for multi-GB files
Comment[fr]=Visionneuse et éditeur de fichiers de plusieurs Go
Exec=$BIN_DIR/buscador %F
Icon=buscador
Terminal=false
Categories=Utility;TextEditor;Development;
MimeType=text/plain;text/x-log;application/json;application/xml;
StartupWMClass=buscador
DESKTOP
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA/applications" 2>/dev/null || true
    guardar_idioma
    info "$(m "Instalado: $BIN_DIR/buscador (y en el menú de aplicaciones)" "Installed: $BIN_DIR/buscador (and in the applications menu)" "Installé : $BIN_DIR/buscador (et dans le menu des applications)")"
    case ":$PATH:" in *":$BIN_DIR:"*) ;; *) echo "   $(m "Nota: $BIN_DIR no está en tu PATH; añádelo para escribir «buscador» en la terminal." "Note: $BIN_DIR is not in your PATH; add it to run “buscador” from the terminal." "Remarque : $BIN_DIR n’est pas dans votre PATH ; ajoutez-le pour lancer « buscador » depuis le terminal.")" ;; esac
fi
# La caché de compilación acelera las actualizaciones, pero cargo no borra lo
# viejo: si pasa de 1 GB se vacía (la próxima compilación empieza de cero).
if [ -d "$CACHE" ] && [ "$(du -sk "$CACHE" | cut -f1)" -gt 1048576 ]; then
    rm -rf "$CACHE" && info "$(m 'Caché de compilación vaciada (pasaba de 1 GB)' 'Build cache emptied (it was over 1 GB)' 'Cache de compilation vidé (il dépassait 1 Go)')"
fi
echo "   $(m 'Para actualizar más adelante: ⚙ Ajustes → «Buscar actualizaciones…», o vuelve a ejecutar este comando.' 'To update later: ⚙ Settings → “Check for updates…”, or run this command again.' 'Pour mettre à jour plus tard : ⚙ Réglages → « Rechercher des mises à jour… », ou relancez cette commande.')"
