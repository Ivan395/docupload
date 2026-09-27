#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="0.5.0"
APP_NAME="DocuUpload.app"
OUT_DIR="$PROJECT_ROOT/target/packages"
BUNDLE_DIR="$PROJECT_ROOT/target/macos_bundle"

echo "==> [macOS] Compilando binario de lanzamiento..."
cd "$PROJECT_ROOT"
cargo build --release

rm -rf "$BUNDLE_DIR"
mkdir -p "$BUNDLE_DIR/$APP_NAME/Contents/MacOS"
mkdir -p "$BUNDLE_DIR/$APP_NAME/Contents/Resources"
mkdir -p "$OUT_DIR"

echo "==> [macOS] Creando estructura del bundle .app..."
install -m 755 "$PROJECT_ROOT/target/release/docu_ffacd" "$BUNDLE_DIR/$APP_NAME/Contents/MacOS/docu_ffacd"
install -m 644 "$PROJECT_ROOT/packaging/macos/Info.plist" "$BUNDLE_DIR/$APP_NAME/Contents/Info.plist"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon.icns" "$BUNDLE_DIR/$APP_NAME/Contents/Resources/app_icon.icns"

OUTPUT_DMG="$OUT_DIR/DocuUpload-${VERSION}-macOS.dmg"
OUTPUT_TAR="$OUT_DIR/DocuUpload-${VERSION}-macOS.tar.gz"

if command -v hdiutil >/dev/null 2>&1; then
    echo "==> [macOS] Creando imagen de disco .dmg con hdiutil..."
    DMG_TMP="$PROJECT_ROOT/target/dmg_tmp"
    rm -rf "$DMG_TMP"
    mkdir -p "$DMG_TMP"
    cp -r "$BUNDLE_DIR/$APP_NAME" "$DMG_TMP/"
    ln -s /Applications "$DMG_TMP/Applications"

    hdiutil create -volname "DocuUpload" -srcfolder "$DMG_TMP" -ov -format UDZO "$OUTPUT_DMG"
    rm -rf "$DMG_TMP"
    echo "==> [macOS] ¡Archivo .dmg generado con éxito en:"
    ls -lh "$OUTPUT_DMG"
else
    echo "==> [macOS] Generando paquete comprimido tar.gz de la app (para entornos no macOS)..."
    (
        cd "$BUNDLE_DIR"
        tar -czf "$OUTPUT_TAR" "$APP_NAME"
    )
    echo "==> [macOS] Bundle generado en: $OUTPUT_TAR"
fi
