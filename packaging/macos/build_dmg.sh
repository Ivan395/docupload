#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="0.5.0"
APP_NAME="DocUpload.app"
OUT_DIR="$PROJECT_ROOT/target/packages"
BUNDLE_DIR="$PROJECT_ROOT/target/macos_bundle"

echo "==> [macOS] Verificando entorno de compilación y herramientas de desarrollo..."
if [[ "$OSTYPE" == "darwin"* ]]; then
    if ! xcode-select -p >/dev/null 2>&1; then
        echo "==> [macOS] Xcode Command Line Tools no detectado. Solicitando instalación..."
        xcode-select --install || true
    fi
    if command -v brew >/dev/null 2>&1; then
        # Verificar utilidades opcionales como create-dmg
        if ! command -v create-dmg >/dev/null 2>&1; then
            echo "==> [macOS] Instalando create-dmg para empaquetado visual dmg optimizado..."
            brew install create-dmg || true
        fi
    fi
fi

echo "==> [macOS] Compilando binario de lanzamiento..."
cd "$PROJECT_ROOT"
cargo build --release

rm -rf "$BUNDLE_DIR"
mkdir -p "$BUNDLE_DIR/$APP_NAME/Contents/MacOS"
mkdir -p "$BUNDLE_DIR/$APP_NAME/Contents/Frameworks"
mkdir -p "$BUNDLE_DIR/$APP_NAME/Contents/Resources"
mkdir -p "$OUT_DIR"

echo "==> [macOS] Creando estructura del bundle .app..."
BIN_TARGET="$BUNDLE_DIR/$APP_NAME/Contents/MacOS/docu_ffacd"
install -m 755 "$PROJECT_ROOT/target/release/docu_ffacd" "$BIN_TARGET"
install -m 644 "$PROJECT_ROOT/packaging/macos/Info.plist" "$BUNDLE_DIR/$APP_NAME/Contents/Info.plist"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon.icns" "$BUNDLE_DIR/$APP_NAME/Contents/Resources/app_icon.icns"

# En macOS, resolver y empaquetar librerías dinámicas no estándar (.dylib) dentro de Frameworks
if [[ "$OSTYPE" == "darwin"* ]] && command -v otool >/dev/null 2>&1; then
    echo "==> [macOS] Empaquetando dependencias dinámicas (.dylib) en Frameworks para portabilidad total..."
    OTOOL_OUT=$(otool -L "$BIN_TARGET" 2>/dev/null | tail -n +2 || true)
    while read -r lib_line; do
        LIB_PATH=$(echo "$lib_line" | awk '{print $1}')
        if [[ "$LIB_PATH" =~ ^(/opt/homebrew|/usr/local)/ ]]; then
            LIB_NAME=$(basename "$LIB_PATH")
            echo "    -> Empaquetando $LIB_NAME en Contents/Frameworks..."
            cp -L "$LIB_PATH" "$BUNDLE_DIR/$APP_NAME/Contents/Frameworks/"
            install_name_tool -change "$LIB_PATH" "@executable_path/../Frameworks/$LIB_NAME" "$BIN_TARGET" || true
        fi
    done <<< "$OTOOL_OUT"
fi

OUTPUT_DMG="$OUT_DIR/DocuUpload-${VERSION}-macOS.dmg"
OUTPUT_TAR="$OUT_DIR/DocuUpload-${VERSION}-macOS.tar.gz"

if command -v create-dmg >/dev/null 2>&1 && [[ "$OSTYPE" == "darwin"* ]]; then
    echo "==> [macOS] Generando DMG con create-dmg..."
    rm -f "$OUTPUT_DMG"
    create-dmg \
        --volname "DocuUpload" \
        --window-pos 200 120 \
        --window-size 600 400 \
        --icon-size 100 \
        --icon "$APP_NAME" 175 190 \
        --hide-extension "$APP_NAME" \
        --app-drop-link 425 190 \
        "$OUTPUT_DMG" \
        "$BUNDLE_DIR/$APP_NAME" || true
fi

if [ ! -f "$OUTPUT_DMG" ]; then
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
else
    echo "==> [macOS] ¡Archivo .dmg generado con éxito en:"
    ls -lh "$OUTPUT_DMG"
fi
