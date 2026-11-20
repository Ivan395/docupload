#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="${VERSION:-$(grep -m1 '^version' "$PROJECT_ROOT/Cargo.toml" | cut -d'"' -f2)}"
PKG_NAME="docu_ffacd-${VERSION}-1-x86_64"
# Binario a empaquetar. CI puede compilarlo en otro job y pasar su ruta aquí.
BIN_PATH="${BIN_PATH:-$PROJECT_ROOT/target/release/docu_ffacd}"

if [ "${SKIP_BUILD:-0}" = "1" ]; then
    echo "==> [Arch Linux] SKIP_BUILD=1: se omiten dependencias de compilación y cargo build"
else
    echo "==> [Arch Linux] Verificando dependencias del sistema para la compilación..."
    SUDO_CMD=""
    if [ "$EUID" -ne 0 ]; then
        if sudo -n true 2>/dev/null; then
            SUDO_CMD="sudo -n"
        elif [ -t 0 ] && command -v sudo >/dev/null 2>&1; then
            SUDO_CMD="sudo"
        fi
    fi

    if command -v pacman >/dev/null 2>&1; then
        ARCH_BUILD_DEPS="base-devel fontconfig freetype2 zlib libx11 zstd libarchive clang"
        MISSING_DEPS=""
        for dep in $ARCH_BUILD_DEPS; do
            if ! pacman -Qi "$dep" >/dev/null 2>&1; then
                MISSING_DEPS="$MISSING_DEPS $dep"
            fi
        done
        if [ -n "$MISSING_DEPS" ]; then
            echo "==> [Arch Linux] Descargando e instalando librerías necesarias para compilar:$MISSING_DEPS"
            if [ -n "$SUDO_CMD" ]; then
                $SUDO_CMD pacman -Sy --noconfirm --needed $MISSING_DEPS || echo "Aviso: No se pudo completar la instalación con pacman."
            else
                echo "Aviso: Ejecuta con sudo o instala manualmente:$MISSING_DEPS"
            fi
        fi
    fi

    echo "==> [Arch Linux] Compilando binario de lanzamiento..."
    cd "$PROJECT_ROOT"
    cargo build --release
fi

if [ ! -f "$BIN_PATH" ]; then
    echo "Error: no se encontró el binario a empaquetar en: $BIN_PATH" >&2
    exit 1
fi

OUT_DIR="$PROJECT_ROOT/target/packages"
BUILD_DIR="$PROJECT_ROOT/target/arch_build/$PKG_NAME"
BUILD_DATE="$(date +%s)"

rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR/usr/bin"
mkdir -p "$BUILD_DIR/usr/share/applications"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/256x256/apps"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/64x64/apps"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/32x32/apps"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/scalable/apps"
mkdir -p "$OUT_DIR"

install -m 755 "$BIN_PATH" "$BUILD_DIR/usr/bin/docu_ffacd"
install -m 644 "$PROJECT_ROOT/packaging/linux/docu_ffacd.desktop" "$BUILD_DIR/usr/share/applications/docu_ffacd.desktop"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_256.png" "$BUILD_DIR/usr/share/icons/hicolor/256x256/apps/docu_ffacd.png"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_64.png" "$BUILD_DIR/usr/share/icons/hicolor/64x64/apps/docu_ffacd.png"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_32.png" "$BUILD_DIR/usr/share/icons/hicolor/32x32/apps/docu_ffacd.png"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon.svg" "$BUILD_DIR/usr/share/icons/hicolor/scalable/apps/docu_ffacd.svg"

cat <<EOF > "$BUILD_DIR/.PKGINFO"
pkgname = docu_ffacd
pkgver = $VERSION-1
pkgdesc = DocuUpload - Procesamiento concurrente de nominas SAT y cedulas SUA IMSS
url = https://github.com/Ivan395/docupload
builddate = $BUILD_DATE
packager = Firma Fiscal ACD <soporte@firmafiscalacd.com>
size = $(du -sb "$BUILD_DIR" | cut -f1)
arch = x86_64
license = MIT
license = Apache-2.0
depend = glibc
depend = gcc-libs
depend = fontconfig
depend = ttf-font
depend = libx11
depend = libxcb
depend = libxcursor
depend = libxi
depend = libxrender
depend = libxkbcommon
depend = libxkbcommon-x11
depend = wayland
depend = libglvnd
depend = xdg-utils
optdepend = xdg-desktop-portal: dialogos de archivos nativos
EOF

cat <<EOF > "$BUILD_DIR/.BUILDINFO"
format = 2
pkgname = docu_ffacd
pkgver = $VERSION-1
pkgarch = x86_64
builddate = $BUILD_DATE
EOF

# Archivos que van dentro del paquete (orden igual al de makepkg)
PKG_FILES=".PKGINFO .BUILDINFO usr"
if command -v bsdtar >/dev/null 2>&1; then
    (
        cd "$BUILD_DIR"
        LANG=C bsdtar -czf .MTREE --format=mtree --options='!all,use-set,type,uid,gid,mode,time,size,md5,sha256,link' .PKGINFO .BUILDINFO usr
    )
    PKG_FILES=".MTREE $PKG_FILES"
else
    echo "Aviso: bsdtar no encontrado; se omite .MTREE (instala libarchive / libarchive-tools para generarlo)."
fi

OUTPUT_PKG="$OUT_DIR/${PKG_NAME}.pkg.tar.zst"
rm -f "$OUTPUT_PKG"

echo "==> [Arch Linux] Creando paquete .pkg.tar.zst..."
if command -v zstd >/dev/null 2>&1; then
    (
        cd "$BUILD_DIR"
        tar --owner=0 --group=0 -cf - $PKG_FILES | zstd -f -T0 -19 -o "$OUTPUT_PKG"
    )
    echo "==> [Arch Linux] ¡Paquete Arch generado con éxito en:"
    ls -lh "$OUTPUT_PKG"
else
    echo "==> [Arch Linux] zstd no encontrado, generando tarball..."
    (
        cd "$BUILD_DIR"
        tar --owner=0 --group=0 -czf "$OUT_DIR/${PKG_NAME}.tar.gz" $PKG_FILES
    )
    echo "==> [Arch Linux] Paquete generado en: $OUT_DIR/${PKG_NAME}.tar.gz"
fi
