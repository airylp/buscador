#!/usr/bin/env sh
# buscador: instalar, actualizar o desinstalar con un solo comando.
#
#   Instalar o actualizar:
#     curl -fsSL https://raw.githubusercontent.com/airylp/buscador/main/install.sh | sh
#   Desinstalar (pregunta si borrar la configuración):
#     curl -fsSL https://raw.githubusercontent.com/airylp/buscador/main/install.sh | sh -s -- desinstalar
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
ACCION="${1:-instalar}"
OS="$(uname)"

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# Preguntas: se leen de la terminal (el script llega por la tubería de curl).
pregunta() {
    # ¿Hay terminal de verdad? (abrir /dev/tty falla si se ejecuta sin ella).
    if (: < /dev/tty) 2>/dev/null; then
        printf '%s [s/N] ' "$1" > /dev/tty
        read -r r < /dev/tty || r=""
        case "$r" in s|S|si|sí|Sí|y|Y) return 0 ;; esac
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

# ------------------------------------------------------------ desinstalar
if [ "$ACCION" = "desinstalar" ] || [ "$ACCION" = "--desinstalar" ]; then
    if [ "$OS" = "Darwin" ]; then
        for d in $APP_DIRS; do
            if [ -d "$d/buscador.app" ]; then
                rm -rf "${d:?}/buscador.app" && info "Quitada $d/buscador.app"
            fi
        done
    else
        for f in "$BIN_DIR/buscador" "$DATA/applications/buscador.desktop" "$DATA/icons/hicolor/256x256/apps/buscador.png"; do
            if [ -e "$f" ]; then rm -f "$f" && info "Quitado $f"; fi
        done
        command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA/applications" 2>/dev/null || true
    fi
    rm -rf "$CACHE" "${TMPDIR:-/tmp}/buscador-nuevos" "${TMPDIR:-/tmp}/buscador-actualizar"
    if [ -d "$CONFIG" ]; then
        echo "Configuración: $CONFIG"
        ls -1 "$CONFIG" | sed 's/^/    /'
        if pregunta "¿Borrar también la configuración (tema, letra, colores…)?"; then
            rm -rf "$CONFIG" && info "Configuración borrada"
        else
            echo "Configuración conservada."
        fi
    fi
    info "buscador desinstalado"
    exit 0
fi
[ "$ACCION" = "instalar" ] || [ "$ACCION" = "actualizar" ] || fail "acción desconocida: $ACCION (usa instalar o desinstalar)"

# ------------------------------------------------------------ requisitos
command -v curl >/dev/null 2>&1 || fail "se necesita curl"
command -v tar  >/dev/null 2>&1 || fail "se necesita tar"
command -v cc   >/dev/null 2>&1 || fail "se necesita un compilador/enlazador C (cc).
  Debian/Ubuntu: sudo apt install build-essential
  Fedora:        sudo dnf install gcc
  Arch:          sudo pacman -S base-devel
  macOS:         xcode-select --install"

# Rust: usa el instalado o lo instala con rustup en ~/.cargo (sin sudo).
PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null 2>&1; then
    info "Rust no encontrado; instalando con rustup en ~/.cargo"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
fi
export PATH
info "Usando $(cargo --version)"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

info "Descargando $REPO@$REF"
curl -fsSL "https://github.com/$REPO/archive/$REF.tar.gz" | tar -xz -C "$TMP" --strip-components=1
# Versión exacta (commit) para «Buscar actualizaciones» de la app.
BUSCADOR_COMMIT="$(curl -fsSL -H 'Accept: application/vnd.github.sha' "https://api.github.com/repos/$REPO/commits/$REF" 2>/dev/null | cut -c1-7 || true)"
export BUSCADOR_COMMIT
# Caché de compilación: las actualizaciones siguientes tardan mucho menos.
CARGO_TARGET_DIR="$CACHE"
export CARGO_TARGET_DIR

if [ "${BUSCADOR_SOLO_BINARIO:-0}" = 1 ]; then
    info "Compilando (la primera vez tarda unos minutos)"
    (cd "$TMP" && cargo build --release --locked)
    cp "$CACHE/release/buscador" "./buscador"
    chmod +x ./buscador
    info "Listo: $(pwd)/buscador"
    exit 0
fi

if [ "$OS" = "Darwin" ]; then
    info "Compilando y creando buscador.app (la primera vez tarda unos minutos)"
    sh "$TMP/empaquetar-mac.sh" instalar --si --no-cerrar ${BUSCADOR_DESTINO:+--destino "$BUSCADOR_DESTINO"}
    info "Listo. Abre buscador desde Launchpad o Spotlight (Cmd+Espacio, «buscador»)."
else
    info "Compilando (la primera vez tarda unos minutos)"
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
Exec=$BIN_DIR/buscador %F
Icon=buscador
Terminal=false
Categories=Utility;TextEditor;Development;
MimeType=text/plain;text/x-log;application/json;application/xml;
StartupWMClass=buscador
DESKTOP
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA/applications" 2>/dev/null || true
    info "Instalado: $BIN_DIR/buscador (y en el menú de aplicaciones)"
    case ":$PATH:" in *":$BIN_DIR:"*) ;; *) echo "   Nota: $BIN_DIR no está en tu PATH; añádelo para escribir «buscador» en la terminal." ;; esac
fi
# La caché de compilación acelera las actualizaciones, pero cargo no borra lo
# viejo: si pasa de 1 GB se vacía (la próxima compilación empieza de cero).
if [ -d "$CACHE" ] && [ "$(du -sk "$CACHE" | cut -f1)" -gt 1048576 ]; then
    rm -rf "$CACHE" && info "Caché de compilación vaciada (pasaba de 1 GB)"
fi
echo "   Para actualizar más adelante: ⚙ Ajustes → «Buscar actualizaciones…», o vuelve a ejecutar este comando."
