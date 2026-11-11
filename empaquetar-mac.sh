#!/bin/sh
# Crea buscador.app (macOS) en target/release/.
#   ./empaquetar-mac.sh             # solo crea la app
#   ./empaquetar-mac.sh --instalar  # además la copia a ~/Applications
set -eu
cd "$(dirname "$0")"

[ "$(uname)" = "Darwin" ] || { echo "Esto solo funciona en macOS" >&2; exit 1; }

cargo build --release
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT=target/release
APP="$OUT/buscador.app"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Ícono: los PNG salen del propio programa (misma «B» pixelada) y iconutil arma el .icns.
"$OUT/buscador" --iconset "$TMP/buscador.iconset"
iconutil -c icns "$TMP/buscador.iconset" -o "$TMP/buscador.icns"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$OUT/buscador" "$APP/Contents/MacOS/buscador"
cp "$TMP/buscador.icns" "$APP/Contents/Resources/buscador.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>buscador</string>
    <key>CFBundleDisplayName</key><string>buscador</string>
    <key>CFBundleIdentifier</key><string>io.github.airylp.buscador</string>
    <key>CFBundleExecutable</key><string>buscador</string>
    <key>CFBundleIconFile</key><string>buscador</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleDevelopmentRegion</key><string>es</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Firma local (ad-hoc): suficiente para ejecutarla en este Mac.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || echo "Aviso: no se pudo firmar (la app funciona igual en este Mac)"
echo "Creada: $APP"

if [ "${1:-}" = "--instalar" ]; then
    mkdir -p "$HOME/Applications"
    rm -rf "$HOME/Applications/buscador.app"
    cp -R "$APP" "$HOME/Applications/"
    echo "Instalada en ~/Applications/buscador.app"
fi
