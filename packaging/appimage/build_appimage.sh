#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="0.5.0"
APPDIR="$PROJECT_ROOT/target/AppDir"
OUT_DIR="$PROJECT_ROOT/target/packages"

echo "==> [AppImage] Compilando binario de lanzamiento..."
cd "$PROJECT_ROOT"
cargo build --release

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/lib"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"
mkdir -p "$OUT_DIR"

echo "==> [AppImage] Configurando estructura AppDir..."
cp "$SCRIPT_DIR/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

install -m 755 "$PROJECT_ROOT/target/release/docu_ffacd" "$APPDIR/usr/bin/docu_ffacd"
install -m 644 "$PROJECT_ROOT/packaging/linux/docu_ffacd.desktop" "$APPDIR/docu_ffacd.desktop"
install -m 644 "$PROJECT_ROOT/packaging/linux/docu_ffacd.desktop" "$APPDIR/usr/share/applications/docu_ffacd.desktop"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_256.png" "$APPDIR/docu_ffacd.png"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_256.png" "$APPDIR/.DirIcon"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_256.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/docu_ffacd.png"

OUTPUT_APPIMAGE="$OUT_DIR/DocuUpload-${VERSION}-x86_64.AppImage"

echo "==> [AppImage] Generando archivo AppImage..."
APPIMAGETOOL=""

if command -v appimagetool >/dev/null 2>&1; then
    APPIMAGETOOL="appimagetool"
elif [ -f "$PROJECT_ROOT/target/appimagetool" ]; then
    APPIMAGETOOL="$PROJECT_ROOT/target/appimagetool"
else
    echo "==> [AppImage] Descargando appimagetool para generación autónoma..."
    TOOL_URL="https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"
    if curl -sL "$TOOL_URL" -o "$PROJECT_ROOT/target/appimagetool" 2>/dev/null; then
        chmod +x "$PROJECT_ROOT/target/appimagetool"
        APPIMAGETOOL="$PROJECT_ROOT/target/appimagetool"
    fi
fi

if [ -n "$APPIMAGETOOL" ]; then
    ARCH=x86_64 "$APPIMAGETOOL" --appimage-extract-and-run "$APPDIR" "$OUTPUT_APPIMAGE" || ARCH=x86_64 "$APPIMAGETOOL" "$APPDIR" "$OUTPUT_APPIMAGE"
    chmod +x "$OUTPUT_APPIMAGE"
    echo "==> [AppImage] ¡AppImage generado con éxito en:"
    ls -lh "$OUTPUT_APPIMAGE"
else
    echo "==> [AppImage] AppDir preparado correctamente en: $APPDIR"
    echo "==> Para empaquetar en AppImage, instala appimagetool o ejecuta en un entorno con conexión."
fi
