#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="${VERSION:-$(grep -m1 '^version' "$PROJECT_ROOT/Cargo.toml" | cut -d'"' -f2)}"
APPDIR="$PROJECT_ROOT/target/AppDir"
OUT_DIR="$PROJECT_ROOT/target/packages"
# Binario a empaquetar. CI puede compilarlo en otro job y pasar su ruta aquí.
BIN_PATH="${BIN_PATH:-$PROJECT_ROOT/target/release/docu_ffacd}"

if [ "${SKIP_BUILD:-0}" = "1" ]; then
    echo "==> [AppImage] SKIP_BUILD=1: se omiten dependencias de compilación y cargo build"
else
    echo "==> [AppImage] Verificando dependencias del sistema para la compilación..."
    SUDO_CMD=""
    if [ "$EUID" -ne 0 ]; then
        if sudo -n true 2>/dev/null; then
            SUDO_CMD="sudo -n"
        fi
    fi

    # Detección e instalación opcional de dependencias de compilación
    if command -v apt-get >/dev/null 2>&1; then
        MISSING_PKGS=""
        for pkg in libfontconfig1-dev libfreetype6-dev zlib1g-dev libx11-dev clang libclang-dev g++; do
            if ! dpkg -s "$pkg" >/dev/null 2>&1; then
                MISSING_PKGS="$MISSING_PKGS $pkg"
            fi
        done
        if [ -n "$MISSING_PKGS" ]; then
            echo "==> [AppImage] Librerías de desarrollo faltantes detectadas:$MISSING_PKGS"
            if [ "$EUID" -eq 0 ] || [ -n "$SUDO_CMD" ]; then
                $SUDO_CMD apt-get update && $SUDO_CMD apt-get install -y $MISSING_PKGS || echo "Aviso: No se pudo instalar automáticamente con apt-get."
            else
                echo "Aviso: Para instalarlas automáticamente ejecuta: sudo apt-get install -y$MISSING_PKGS"
            fi
        fi
    elif command -v dnf >/dev/null 2>&1; then
        MISSING_PKGS=""
        for pkg in fontconfig-devel freetype-devel zlib-devel libX11-devel clang-devel gcc-c++; do
            if ! rpm -q "$pkg" >/dev/null 2>&1; then
                MISSING_PKGS="$MISSING_PKGS $pkg"
            fi
        done
        if [ -n "$MISSING_PKGS" ]; then
            echo "==> [AppImage] Librerías de desarrollo faltantes detectadas:$MISSING_PKGS"
            if [ "$EUID" -eq 0 ] || [ -n "$SUDO_CMD" ]; then
                $SUDO_CMD dnf install -y $MISSING_PKGS || echo "Aviso: No se pudo instalar automáticamente con dnf."
            else
                echo "Aviso: Para instalarlas automáticamente ejecuta: sudo dnf install -y$MISSING_PKGS"
            fi
        fi
    elif command -v pacman >/dev/null 2>&1; then
        MISSING_PKGS=""
        for pkg in fontconfig freetype2 zlib libx11 clang; do
            if ! pacman -Qi "$pkg" >/dev/null 2>&1; then
                MISSING_PKGS="$MISSING_PKGS $pkg"
            fi
        done
        if [ -n "$MISSING_PKGS" ]; then
            echo "==> [AppImage] Librerías de desarrollo faltantes detectadas:$MISSING_PKGS"
            if [ "$EUID" -eq 0 ] || [ -n "$SUDO_CMD" ]; then
                $SUDO_CMD pacman -Sy --noconfirm --needed $MISSING_PKGS || echo "Aviso: No se pudo instalar automáticamente con pacman."
            else
                echo "Aviso: Para instalarlas automáticamente ejecuta: sudo pacman -S --needed$MISSING_PKGS"
            fi
        fi
    fi

    echo "==> [AppImage] Compilando binario de lanzamiento..."
    cd "$PROJECT_ROOT"
    cargo build --release
fi

if [ ! -f "$BIN_PATH" ]; then
    echo "Error: no se encontró el binario a empaquetar en: $BIN_PATH" >&2
    exit 1
fi

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/lib"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"
mkdir -p "$OUT_DIR"

echo "==> [AppImage] Configurando estructura AppDir..."
cp "$SCRIPT_DIR/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

install -m 755 "$BIN_PATH" "$APPDIR/usr/bin/docu_ffacd"
install -m 644 "$PROJECT_ROOT/packaging/linux/docu_ffacd.desktop" "$APPDIR/docu_ffacd.desktop"
install -m 644 "$PROJECT_ROOT/packaging/linux/docu_ffacd.desktop" "$APPDIR/usr/share/applications/docu_ffacd.desktop"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_256.png" "$APPDIR/docu_ffacd.png"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_256.png" "$APPDIR/.DirIcon"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_256.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/docu_ffacd.png"

echo "==> [AppImage] Empaquetando librerías compartidas (.so) necesarias..."
# Se omiten glibc/ld, la pila gráfica (GL/EGL/DRM/GBM/Wayland/X11/xcb/xkbcommon)
# y libstdc++/libgcc_s: deben venir del host porque los drivers gráficos y el
# ABI de glibc/libstdc++ dependen del sistema instalado. Empaquetarlos rompería
# la aceleración gráfica o la carga de drivers en el equipo del usuario.
# fontconfig/freetype (y sus dependencias expat, zlib, uuid, bz2, png, brotli,
# harfbuzz) también vienen del host, como indica la excludelist de AppImage:
# una fontconfig antigua no entiende la configuración ni la caché de
# distribuciones nuevas y reescanea las fuentes en cada arranque.
LDD_OUTPUT=$(ldd "$BIN_PATH" 2>/dev/null || true)
while read -r line; do
    LIB_PATH=$(echo "$line" | grep -o '/[^ ]*\.so[^ ]*' | head -n1)
    if [ -n "$LIB_PATH" ] && [ -f "$LIB_PATH" ]; then
        LIB_NAME=$(basename "$LIB_PATH")
        case "$LIB_NAME" in
            libc.so*|libm.so*|libpthread.so*|libdl.so*|ld-linux*|librt.so*|linux-vdso*|libGL.so*|libGLX*|libGLdispatch*|libEGL*|libdrm*|libgbm*|libwayland-*|libxkbcommon*|libX11*|libxcb*|libstdc++*|libgcc_s*|libfontconfig*|libfreetype*|libexpat*|libz.so*|libuuid*|libbz2*|libpng*|libbrotli*|libharfbuzz*)
                # Omitir: glibc y stack gráfico/ABI del host (ver comentario anterior)
                ;;
            *)
                cp -L "$LIB_PATH" "$APPDIR/usr/lib/" 2>/dev/null || true
                ;;
        esac
    fi
done <<< "$LDD_OUTPUT"

# No se copia /etc/fonts del equipo de compilación: se usa la configuración de
# fuentes del sistema donde corre el AppImage.

OUTPUT_APPIMAGE="$OUT_DIR/DocuUpload-${VERSION}-linux-x86_64.AppImage"

echo "==> [AppImage] Verificando herramienta de empaquetado appimagetool..."
APPIMAGETOOL=""

if command -v appimagetool >/dev/null 2>&1; then
    APPIMAGETOOL="appimagetool"
elif [ -f "$PROJECT_ROOT/target/appimagetool" ]; then
    APPIMAGETOOL="$PROJECT_ROOT/target/appimagetool"
else
    echo "==> [AppImage] Descargando appimagetool para generación autónoma..."
    mkdir -p "$PROJECT_ROOT/target"
    URLS=(
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
        "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage"
    )
    for url in "${URLS[@]}"; do
        if curl -fsSL "$url" -o "$PROJECT_ROOT/target/appimagetool" 2>/dev/null && [ -s "$PROJECT_ROOT/target/appimagetool" ]; then
            chmod +x "$PROJECT_ROOT/target/appimagetool"
            APPIMAGETOOL="$PROJECT_ROOT/target/appimagetool"
            break
        fi
    done
fi

if [ -n "$APPIMAGETOOL" ]; then
    ARCH=x86_64 "$APPIMAGETOOL" --appimage-extract-and-run "$APPDIR" "$OUTPUT_APPIMAGE" || ARCH=x86_64 "$APPIMAGETOOL" "$APPDIR" "$OUTPUT_APPIMAGE"
    chmod +x "$OUTPUT_APPIMAGE"
    echo "==> [AppImage] ¡AppImage generado con éxito en:"
    ls -lh "$OUTPUT_APPIMAGE"
else
    echo "==> [ERROR] No se pudo encontrar ni descargar appimagetool" >&2
    exit 1
fi
