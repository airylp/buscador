#!/bin/sh
# buscador para macOS: crear, instalar, actualizar o desinstalar buscador.app
#
#   ./empaquetar-mac.sh                 menú interactivo
#   ./empaquetar-mac.sh crear           compila y crea target/release/buscador.app
#   ./empaquetar-mac.sh instalar        crea la app (si compila bien) y la copia a /Applications
#   ./empaquetar-mac.sh actualizar      trae los últimos cambios (git), recompila y reemplaza la instalada
#   ./empaquetar-mac.sh desinstalar     quita la app y pregunta por la configuración
#
# Opciones: --destino CARPETA   dónde instalar (por defecto /Applications, o ~/Applications sin permisos)
#           --si                no preguntar (responde «sí»; la configuración nunca se borra sin preguntar)
#           --no-cerrar         no cerrar buscador si está abierto (lo usa «Actualizar» desde la propia app)
set -eu
cd "$(dirname "$0")"

# Idioma de los mensajes: el que pasa install.sh (IDIOMA), BUSCADOR_IDIOMA, el
# guardado en los ajustes de la app o el de la terminal.
IDIOMA="${IDIOMA:-${BUSCADOR_IDIOMA:-$(sed -n 's/^idioma=//p' "$HOME/Library/Application Support/buscador/ajustes.txt" 2>/dev/null | head -n 1)}}"
if [ -z "$IDIOMA" ]; then
    case "${LC_ALL:-${LANG:-}}" in en*) IDIOMA=en ;; fr*) IDIOMA=fr ;; *) IDIOMA=es ;; esac
fi
# m "español" "english" "français"
m() { case "$IDIOMA" in en) printf '%s' "$2" ;; fr) printf '%s' "$3" ;; *) printf '%s' "$1" ;; esac; }

[ "$(uname)" = "Darwin" ] || { echo "$(m 'Esto solo funciona en macOS' 'This only works on macOS' 'Ceci ne fonctionne que sous macOS')" >&2; exit 1; }
# Al lanzarse desde la propia app (Finder) el PATH es mínimo: añadir cargo y Homebrew.
PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export PATH

APP_NAME="buscador.app"
OUT="${CARGO_TARGET_DIR:-target}/release"
APP="$OUT/$APP_NAME"
CONFIG="$HOME/Library/Application Support/buscador"
TEMPS="${TMPDIR:-/tmp}/buscador-nuevos"
CACHE="$HOME/Library/Caches/buscador-compilacion"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
BUNDLE_ID="io.github.airylp.buscador"
REABRIR=0

ACCION=""
DESTINO=""
SI=0
NO_CERRAR=0
while [ $# -gt 0 ]; do
    case "$1" in
        crear|instalar|actualizar|desinstalar) ACCION="$1" ;;
        --instalar) ACCION="instalar" ;;  # compatibilidad con la versión anterior
        --destino) shift; DESTINO="${1:-}" ;;
        --si|-y) SI=1 ;;
        --no-cerrar) NO_CERRAR=1 ;;
        -h|--help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "$(m "Opción desconocida: $1 (usa --help)" "Unknown option: $1 (use --help)" "Option inconnue : $1 (utilisez --help)")" >&2; exit 2 ;;
    esac
    shift
done

pregunta() { # pregunta "texto" -> 0 si responde sí
    if [ "$SI" = 1 ]; then return 0; fi
    printf "%s %s " "$1" "$(m '[s/N]' '[y/N]' '[o/N]')"
    read -r r || r=""
    case "$r" in s|S|si|SI|sí|Sí|y|Y|yes|o|O|oui) return 0 ;; *) return 1 ;; esac
}

# Dónde está instalada (si lo está).
instalada() {
    for d in "${DESTINO:-}" /Applications "$HOME/Applications"; do
        [ -n "$d" ] && [ -d "$d/$APP_NAME" ] && { echo "$d/$APP_NAME"; return 0; }
    done
    return 1
}

abierta() { pgrep -f "$APP_NAME/Contents/MacOS/buscador" >/dev/null 2>&1; }

# Pide a buscador que se cierre como si lo hiciera el usuario: si hay cambios
# sin guardar, la propia app pregunta. Nunca se mata a la fuerza.
cerrar_si_abierta() {
    [ "$NO_CERRAR" = 1 ] && return 0
    abierta || return 0
    resp=n
    if (: < /dev/tty) 2>/dev/null; then
        m "buscador está abierto. ¿Cerrarlo ahora y volver a abrirlo con la versión nueva? [S/n] " \
          "buscador is open. Close it now and reopen it with the new version? [Y/n] " \
          "buscador est ouvert. Le fermer maintenant et le rouvrir avec la nouvelle version ? [O/n] " > /dev/tty
        read -r resp < /dev/tty || resp=""
        [ -z "$resp" ] && resp=s
    fi
    case "$resp" in
        s|S|si|sí|y|Y|yes|o|O|oui)
            osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
            i=0
            while abierta && [ $i -lt 60 ]; do sleep 1; i=$((i + 1)); done
            if abierta; then
                echo "$(m 'buscador sigue abierto (¿cambios sin guardar?): se instala igual; reinícialo después.' 'buscador is still open (unsaved changes?): installing anyway; restart it afterwards.' 'buscador est toujours ouvert (modifications non enregistrées ?) : installation quand même ; relancez-le ensuite.')"
            else
                REABRIR=1
            fi
            ;;
        *) echo "$(m 'Se instala igual; reinicia buscador para usar la versión nueva.' 'Installing anyway; restart buscador to use the new version.' 'Installation quand même ; relancez buscador pour utiliser la nouvelle version.')" ;;
    esac
}

crear() {
    echo "==> $(m 'Compilando…' 'Building…' 'Compilation…')"
    # La versión (commit) queda grabada en el programa: la usa «Actualizar» de la interfaz.
    # (Si ya viene dada —install.sh desde un .tar.gz sin git— se respeta.)
    BUSCADOR_COMMIT="${BUSCADOR_COMMIT:-$(git rev-parse --short HEAD 2>/dev/null || echo "")}"
    export BUSCADOR_COMMIT
    if ! cargo build --release; then
        echo "$(m 'La compilación falló: no se crea ni se instala nada.' 'The build failed: nothing was created or installed.' 'La compilation a échoué : rien n’a été créé ni installé.')" >&2
        exit 1
    fi
    VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT

    # Ícono: los PNG salen del propio programa (la «B» pixelada) e iconutil arma el .icns.
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
    <key>CFBundleVersion</key><string>$VERSION${BUSCADOR_COMMIT:+ ($BUSCADOR_COMMIT)}</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleDevelopmentRegion</key><string>es</string>
    <key>CFBundleLocalizations</key><array><string>es</string><string>en</string><string>fr</string></array>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
    <!-- Aparece en «Abrir con…» para cualquier archivo, sin volverse la app predeterminada. -->
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>Archivo</string>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.data</string>
                <string>public.content</string>
            </array>
        </dict>
    </array>
</dict>
</plist>
PLIST
    # Firma local (ad-hoc): suficiente para ejecutarla en este Mac.
    codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || echo "$(m 'Aviso: no se pudo firmar (la app funciona igual en este Mac)' 'Warning: could not sign it (the app still works on this Mac)' 'Attention : signature impossible (l’app fonctionne quand même sur ce Mac)')"
    echo "==> $(m 'Creada' 'Created' 'Créée'): $APP${BUSCADOR_COMMIT:+ ($(m versión version version) $BUSCADOR_COMMIT)}"
}

copiar_a() { # copiar_a CARPETA
    dir="$1"
    mkdir -p "$dir" 2>/dev/null || true
    if [ -w "$dir" ]; then
        if [ -d "$dir/$APP_NAME" ]; then
            # Reemplazar el contenido sin borrar la carpeta: para macOS (Dock,
            # Launchpad) sigue siendo la misma app y no aparece un ícono nuevo.
            rsync -a --delete "$APP/" "$dir/$APP_NAME/"
        else
            cp -R "$APP" "$dir/"
        fi
        "$LSREGISTER" -f "$dir/$APP_NAME" >/dev/null 2>&1 || true
        echo "==> $(m 'Instalada en' 'Installed in' 'Installée dans') $dir/$APP_NAME"
        INSTALADA="$dir/$APP_NAME"
        return 0
    fi
    return 1
}

instalar() {
    crear
    cerrar_si_abierta
    if [ -n "$DESTINO" ]; then
        copiar_a "$DESTINO" || { echo "$(m 'Sin permiso para escribir en' 'No permission to write to' 'Pas la permission d’écrire dans') $DESTINO" >&2; exit 1; }
    else
        copiar_a /Applications || {
            echo "$(m 'Sin permiso en /Applications; se instala en ~/Applications' 'No permission for /Applications; installing in ~/Applications' 'Pas de permission pour /Applications ; installation dans ~/Applications')"
            copiar_a "$HOME/Applications"
        }
    fi
    # El paquete intermedio (en target/ o en la caché) ya no hace falta: así
    # macOS no lo registra como otra app más.
    if [ "$APP" != "${INSTALADA:-}" ]; then
        "$LSREGISTER" -u "$APP" >/dev/null 2>&1 || true
        rm -rf "$APP"
    fi
    if [ "$REABRIR" = 1 ]; then
        open "$INSTALADA" && echo "==> $(m 'buscador vuelto a abrir con la versión nueva' 'buscador reopened with the new version' 'buscador rouvert avec la nouvelle version')"
    else
        echo "$(m 'Ábrela desde Launchpad o Spotlight (Cmd+Espacio, «buscador»).' 'Open it from Launchpad or Spotlight (Cmd+Space, “buscador”).' 'Ouvrez-la depuis Launchpad ou Spotlight (Cmd+Espace, « buscador »).')"
    fi
}

actualizar() {
    if [ -z "$DESTINO" ]; then
        if actual=$(instalada); then DESTINO=$(dirname "$actual"); fi
    fi
    if git rev-parse --is-inside-work-tree >/dev/null 2>&1 && git remote get-url origin >/dev/null 2>&1; then
        if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
            echo "$(m 'Hay cambios locales sin commit: se compila lo que hay, sin traer cambios.' 'There are uncommitted local changes: building what is there, without pulling.' 'Il y a des modifications locales non validées : compilation de l’existant, sans récupérer.')"
        elif pregunta "$(m '¿Traer los últimos cambios de GitHub (git pull)?' 'Pull the latest changes from GitHub (git pull)?' 'Récupérer les dernières modifications depuis GitHub (git pull) ?')"; then
            git pull --ff-only
        fi
    fi
    instalar
}

desinstalar() {
    quitado=0
    for d in "${DESTINO:-}" /Applications "$HOME/Applications"; do
        if [ -n "$d" ] && [ -d "$d/$APP_NAME" ]; then
            cerrar_si_abierta
            if pregunta "$(m "¿Quitar $d/$APP_NAME?" "Remove $d/$APP_NAME?" "Supprimer $d/$APP_NAME ?")"; then
                "$LSREGISTER" -u "$d/$APP_NAME" >/dev/null 2>&1 || true
                rm -rf "${d:?}/$APP_NAME" && echo "==> $(m 'Quitada' 'Removed' 'Supprimée'): $d/$APP_NAME" && quitado=1
            fi
        fi
    done
    [ "$quitado" = 1 ] || echo "$(m 'No había ninguna buscador.app instalada (o no se quitó).' 'No buscador.app was installed (or it was not removed).' 'Aucune buscador.app n’était installée (ou elle n’a pas été supprimée).')"
    if [ -d "$CONFIG" ]; then
        echo "$(m 'Configuración' 'Settings' 'Réglages'): $CONFIG"
        ls -1 "$CONFIG" | sed 's/^/    /'
        # La configuración se pregunta siempre, incluso con --si.
        m "¿Borrar también la configuración (tema, letra, colores…)? [s/N] " "Also delete the settings (theme, font, colours…)? [y/N] " "Supprimer aussi les réglages (thème, police, couleurs…) ? [o/N] "
        read -r r || r=""
        case "$r" in s|S|si|sí|y|Y|yes|o|O|oui) rm -rf "$CONFIG" && echo "==> $(m 'Configuración borrada' 'Settings deleted' 'Réglages supprimés')" ;; *) echo "$(m 'Configuración conservada.' 'Settings kept.' 'Réglages conservés.')" ;; esac
    fi
    [ -d "$TEMPS" ] && rm -rf "$TEMPS" && echo "==> $(m 'Temporales de «Nuevo» borrados' 'Temporary «New» files deleted' 'Fichiers temporaires de « Nouveau » supprimés')"
    [ -d "${TMPDIR:-/tmp}/buscador-actualizar" ] && rm -rf "${TMPDIR:-/tmp}/buscador-actualizar"
    [ -d "$CACHE" ] && rm -rf "$CACHE" && echo "==> $(m 'Caché de compilación borrada' 'Build cache deleted' 'Cache de compilation supprimé')"
    echo "$(m 'Listo.' 'Done.' 'Terminé.')"
}

if [ -z "$ACCION" ]; then
    echo "$(m 'buscador para macOS' 'buscador for macOS' 'buscador pour macOS')"
    if actual=$(instalada); then echo "$(m 'Instalada en' 'Installed in' 'Installée dans'): $actual"; else echo "$(m 'No está instalada.' 'Not installed.' 'Non installée.')"; fi
    echo
    echo "  1) $(m 'Crear la app' 'Build the app' 'Créer l’app') (target/release/buscador.app)"
    echo "  2) $(m 'Instalar en Aplicaciones' 'Install in Applications' 'Installer dans Applications')"
    echo "  3) $(m 'Actualizar (traer cambios, recompilar y reemplazar)' 'Update (pull, rebuild and replace)' 'Mettre à jour (récupérer, recompiler et remplacer)')"
    echo "  4) $(m 'Desinstalar' 'Uninstall' 'Désinstaller')"
    echo "  5) $(m 'Salir' 'Quit' 'Quitter')"
    printf "%s" "$(m 'Elige [1-5]: ' 'Choose [1-5]: ' 'Choisissez [1-5] : ')"
    read -r op || op=5
    case "$op" in 1) ACCION=crear ;; 2) ACCION=instalar ;; 3) ACCION=actualizar ;; 4) ACCION=desinstalar ;; *) exit 0 ;; esac
fi

case "$ACCION" in
    crear) crear ;;
    instalar) instalar ;;
    actualizar) actualizar ;;
    desinstalar) desinstalar ;;
esac
