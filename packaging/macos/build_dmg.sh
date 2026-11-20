#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# La versión sale de Cargo.toml (única fuente de verdad); se puede sobreescribir con VERSION=...
VERSION="${VERSION:-$(grep -m1 '^version' "$PROJECT_ROOT/Cargo.toml" | cut -d'"' -f2)}"
APP_NAME="DocUpload.app"
OUT_DIR="$PROJECT_ROOT/target/packages"
BUNDLE_DIR="$PROJECT_ROOT/target/macos_bundle"
BIN_SRC="${BIN_PATH:-$PROJECT_ROOT/target/release/docu_ffacd}"

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

cd "$PROJECT_ROOT"
if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
    echo "==> [macOS] Compilando binario de lanzamiento (macOS mínimo ${MACOSX_DEPLOYMENT_TARGET:-11.0})..."
    export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
    cargo build --release
else
    echo "==> [macOS] SKIP_BUILD=1: se omite cargo build."
fi

if [[ ! -f "$BIN_SRC" ]]; then
    echo "==> [macOS] ERROR: no existe el binario a empaquetar: $BIN_SRC"
    exit 1
fi

rm -rf "$BUNDLE_DIR"
mkdir -p "$BUNDLE_DIR/$APP_NAME/Contents/MacOS"
mkdir -p "$BUNDLE_DIR/$APP_NAME/Contents/Frameworks"
mkdir -p "$BUNDLE_DIR/$APP_NAME/Contents/Resources"
mkdir -p "$OUT_DIR"

echo "==> [macOS] Creando estructura del bundle .app..."
BIN_TARGET="$BUNDLE_DIR/$APP_NAME/Contents/MacOS/docu_ffacd"
FRAMEWORKS_DIR="$BUNDLE_DIR/$APP_NAME/Contents/Frameworks"
install -m 755 "$BIN_SRC" "$BIN_TARGET"
sed "s/__VERSION__/$VERSION/g" "$PROJECT_ROOT/packaging/macos/Info.plist" > "$BUNDLE_DIR/$APP_NAME/Contents/Info.plist"
chmod 644 "$BUNDLE_DIR/$APP_NAME/Contents/Info.plist"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon.icns" "$BUNDLE_DIR/$APP_NAME/Contents/Resources/app_icon.icns"

# En macOS, empaquetar las .dylib de Homebrew dentro de Frameworks, incluyendo sus dependencias transitivas
if [[ "$OSTYPE" == "darwin"* ]] && command -v otool >/dev/null 2>&1; then
    echo "==> [macOS] Empaquetando dependencias dinámicas (.dylib) en Frameworks para portabilidad total..."
    # Cola de binarios/librerias a revisar: empieza por el ejecutable y crece con cada lib copiada
    PENDING=("$BIN_TARGET")
    while [[ ${#PENDING[@]} -gt 0 ]]; do
        CURRENT="${PENDING[0]}"
        PENDING=("${PENDING[@]:1}")
        while read -r LIB_PATH; do
            [[ "$LIB_PATH" =~ ^(/opt/homebrew|/usr/local)/ ]] || continue
            LIB_NAME=$(basename "$LIB_PATH")
            DEST="$FRAMEWORKS_DIR/$LIB_NAME"
            if [[ ! -f "$DEST" ]]; then
                echo "    -> Empaquetando $LIB_NAME en Contents/Frameworks..."
                cp -L "$LIB_PATH" "$DEST"
                chmod u+w "$DEST"
                install_name_tool -id "@executable_path/../Frameworks/$LIB_NAME" "$DEST"
                PENDING+=("$DEST")
            fi
            install_name_tool -change "$LIB_PATH" "@executable_path/../Frameworks/$LIB_NAME" "$CURRENT" || true
        done < <(otool -L "$CURRENT" | tail -n +2 | awk '{print $1}')
    done
fi

if [[ "$OSTYPE" == "darwin"* ]]; then
    # El binario final no debe seguir apuntando a rutas de Homebrew
    if otool -L "$BIN_TARGET" | tail -n +2 | grep -Eq '(/opt/homebrew|/usr/local)/'; then
        echo "==> [macOS] ERROR: el binario todavía referencia /opt/homebrew o /usr/local:"
        otool -L "$BIN_TARGET"
        exit 1
    fi

    # Firma del bundle: se hace después de ensamblarlo y antes de crear el dmg
    if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
        echo "==> [macOS] Firmando la app con identidad: $MACOS_SIGN_IDENTITY"
        codesign --force --deep --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$BUNDLE_DIR/$APP_NAME"
    else
        echo "==> [macOS] MACOS_SIGN_IDENTITY no definido: firma ad-hoc."
        codesign --force --deep --sign - "$BUNDLE_DIR/$APP_NAME"
    fi
    codesign --verify --deep --strict "$BUNDLE_DIR/$APP_NAME"
fi

# Etiqueta de arquitectura para el nombre del artefacto
if command -v lipo >/dev/null 2>&1; then
    BIN_ARCHS=$(lipo -archs "$BIN_TARGET")
    if [[ "$BIN_ARCHS" == *arm64* && "$BIN_ARCHS" == *x86_64* ]]; then
        ARCH_LABEL="universal"
    else
        ARCH_LABEL="${BIN_ARCHS// /-}"
    fi
else
    ARCH_LABEL="$(uname -m)"
fi

OUTPUT_DMG="$OUT_DIR/DocuUpload-${VERSION}-macos-${ARCH_LABEL}.dmg"
OUTPUT_TAR="$OUT_DIR/DocuUpload-${VERSION}-macos-${ARCH_LABEL}.tar.gz"

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

# Notarización: solo tiene sentido sobre el .dmg y con credenciales completas
if [[ -f "$OUTPUT_DMG" ]]; then
    if [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" && -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
        echo "==> [macOS] Enviando el .dmg a notarización de Apple (puede tardar varios minutos)..."
        xcrun notarytool submit "$OUTPUT_DMG" \
            --apple-id "$APPLE_ID" \
            --team-id "$APPLE_TEAM_ID" \
            --password "$APPLE_APP_PASSWORD" \
            --wait
        xcrun stapler staple "$OUTPUT_DMG"
    else
        echo "==> [macOS] AVISO: el .dmg no está notarizado (faltan APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD o MACOS_SIGN_IDENTITY)."
    fi
fi
