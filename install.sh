#!/usr/bin/env sh
# Descarga, compila y deja el ejecutable `buscador` en la carpeta actual.
#
#   curl -fsSL https://raw.githubusercontent.com/airylp/buscador/main/install.sh | sh
#
# Variables opcionales: BUSCADOR_REPO (usuario/repo), BUSCADOR_REF (rama o tag).
set -eu

REPO="${BUSCADOR_REPO:-airylp/buscador}"
REF="${BUSCADOR_REF:-main}"
DEST="$(pwd)"

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || fail "se necesita curl"
command -v tar  >/dev/null 2>&1 || fail "se necesita tar"
command -v cc   >/dev/null 2>&1 || fail "se necesita un compilador/enlazador C (cc).
  Debian/Ubuntu: sudo apt install build-essential
  Fedora:        sudo dnf install gcc
  Arch:          sudo pacman -S base-devel
  macOS:         xcode-select --install"

# Rust: usa el instalado o lo instala con rustup en ~/.cargo (sin sudo).
if ! command -v cargo >/dev/null 2>&1; then
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
        PATH="$HOME/.cargo/bin:$PATH"
    else
        info "Rust no encontrado; instalando con rustup en ~/.cargo"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
        PATH="$HOME/.cargo/bin:$PATH"
    fi
fi
export PATH
info "Usando $(cargo --version)"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

info "Descargando $REPO@$REF"
curl -fsSL "https://github.com/$REPO/archive/$REF.tar.gz" | tar -xz -C "$TMP" --strip-components=1

info "Compilando (la primera vez tarda unos minutos)"
(cd "$TMP" && cargo build --release --locked)

cp "$TMP/target/release/buscador" "$DEST/buscador"
chmod +x "$DEST/buscador"
info "Listo: $DEST/buscador"
echo "   Uso: ./buscador archivo1.log archivo2.log"
