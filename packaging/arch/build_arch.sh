#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="0.5.0"
PKG_NAME="docu_ffacd-${VERSION}-1-x86_64"

echo "==> [Arch Linux] Compilando binario de lanzamiento..."
cd "$PROJECT_ROOT"
cargo build --release

OUT_DIR="$PROJECT_ROOT/target/packages"
BUILD_DIR="$PROJECT_ROOT/target/arch_build/$PKG_NAME"

rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR/usr/bin"
mkdir -p "$BUILD_DIR/usr/share/applications"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/256x256/apps"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/64x64/apps"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/32x32/apps"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/scalable/apps"
mkdir -p "$OUT_DIR"

install -m 755 "$PROJECT_ROOT/target/release/docu_ffacd" "$BUILD_DIR/usr/bin/docu_ffacd"
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
builddate = $(date +%s)
packager = Firma Fiscal ACD <soporte@firmafiscalacd.com>
size = $(du -sb "$BUILD_DIR" | cut -f1)
arch = x86_64
license = MIT
depend = fontconfig
depend = freetype2
EOF

OUTPUT_PKG="$OUT_DIR/${PKG_NAME}.pkg.tar.zst"

echo "==> [Arch Linux] Creando paquete .pkg.tar.zst..."
if command -v zstd >/dev/null 2>&1; then
    (
        cd "$BUILD_DIR"
        tar --owner=0 --group=0 -cf - .PKGINFO usr | zstd -T0 -19 -o "$OUTPUT_PKG"
    )
    echo "==> [Arch Linux] ¡Paquete Arch generado con éxito en:"
    ls -lh "$OUTPUT_PKG"
else
    echo "==> [Arch Linux] zstd no encontrado, generando tarball..."
    (
        cd "$BUILD_DIR"
        tar --owner=0 --group=0 -czf "$OUT_DIR/${PKG_NAME}.tar.gz" .PKGINFO usr
    )
    echo "==> [Arch Linux] Paquete generado en: $OUT_DIR/${PKG_NAME}.tar.gz"
fi
