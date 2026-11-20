#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="${VERSION:-$(grep -m1 '^version' "$PROJECT_ROOT/Cargo.toml" | cut -d'"' -f2)}"
ARCH="amd64"
PKG_NAME="docu-ffacd_${VERSION}_${ARCH}"
# Binario a empaquetar. CI puede compilarlo en otro job y pasar su ruta aquí.
BIN_PATH="${BIN_PATH:-$PROJECT_ROOT/target/release/docu_ffacd}"

if [ "${SKIP_BUILD:-0}" = "1" ]; then
    echo "==> [Debian/Ubuntu] SKIP_BUILD=1: se omiten dependencias de compilación y cargo build"
else
    echo "==> [Debian/Ubuntu] Verificando dependencias del sistema para la compilación..."
    SUDO_CMD=""
    if [ "$EUID" -ne 0 ]; then
        if sudo -n true 2>/dev/null; then
            SUDO_CMD="sudo -n"
        elif [ -t 0 ] && command -v sudo >/dev/null 2>&1; then
            SUDO_CMD="sudo"
        fi
    fi

    if command -v apt-get >/dev/null 2>&1; then
        DEB_BUILD_DEPS="libfontconfig1-dev libfreetype6-dev zlib1g-dev libx11-dev pkg-config build-essential clang libclang-dev g++"
        MISSING_DEPS=""
        for dep in $DEB_BUILD_DEPS; do
            if ! dpkg -s "$dep" >/dev/null 2>&1; then
                MISSING_DEPS="$MISSING_DEPS $dep"
            fi
        done
        if [ -n "$MISSING_DEPS" ]; then
            echo "==> [Debian/Ubuntu] Descargando e instalando librerías necesarias para compilar:$MISSING_DEPS"
            if [ -n "$SUDO_CMD" ]; then
                $SUDO_CMD apt-get update && $SUDO_CMD apt-get install -y $MISSING_DEPS || echo "Aviso: No se pudo completar la instalación con apt-get."
            else
                echo "Aviso: Ejecuta con sudo o instala manualmente:$MISSING_DEPS"
            fi
        fi
    fi

    echo "==> [Debian/Ubuntu] Compilando binario de lanzamiento..."
    cd "$PROJECT_ROOT"
    cargo build --release
fi

if [ ! -f "$BIN_PATH" ]; then
    echo "Error: no se encontró el binario a empaquetar en: $BIN_PATH" >&2
    exit 1
fi

BUILD_DIR="$PROJECT_ROOT/target/deb_build/$PKG_NAME"
OUT_DIR="$PROJECT_ROOT/target/packages"

rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR/DEBIAN"
mkdir -p "$BUILD_DIR/usr/bin"
mkdir -p "$BUILD_DIR/usr/share/applications"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/256x256/apps"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/64x64/apps"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/32x32/apps"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/scalable/apps"
mkdir -p "$OUT_DIR"

echo "==> [Debian/Ubuntu] Copiando archivos a la estructura del paquete..."
install -m 755 "$BIN_PATH" "$BUILD_DIR/usr/bin/docu_ffacd"
install -m 644 "$PROJECT_ROOT/packaging/linux/docu_ffacd.desktop" "$BUILD_DIR/usr/share/applications/docu_ffacd.desktop"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_256.png" "$BUILD_DIR/usr/share/icons/hicolor/256x256/apps/docu_ffacd.png"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_64.png" "$BUILD_DIR/usr/share/icons/hicolor/64x64/apps/docu_ffacd.png"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon_32.png" "$BUILD_DIR/usr/share/icons/hicolor/32x32/apps/docu_ffacd.png"
install -m 644 "$PROJECT_ROOT/ui/assets/app_icon.svg" "$BUILD_DIR/usr/share/icons/hicolor/scalable/apps/docu_ffacd.svg"

# Compute installed size in KB
INSTALLED_SIZE=$(du -sk "$BUILD_DIR" | cut -f1)

cat <<EOF > "$BUILD_DIR/DEBIAN/control"
Package: docu-ffacd
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Depends: libc6 (>= 2.28), libgcc-s1, libstdc++6, libfontconfig1, libx11-6, libx11-xcb1, libxcb1, libxcursor1, libxi6, libxrender1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-egl1, libegl1, libgl1, xdg-utils
Recommends: xdg-desktop-portal, xdg-desktop-portal-gtk | xdg-desktop-portal-kde | xdg-desktop-portal-gnome
Maintainer: Firma Fiscal ACD <soporte@firmafiscalacd.com>
Installed-Size: $INSTALLED_SIZE
Description: DocuUpload Fiscal y Nominas
 Procesamiento concurrente y en paralelo de Nominas CFDI con validacion
 ante el SAT (SOAP) y Cedulas SUA IMSS con reportes Excel.
EOF

cat <<'EOF' > "$BUILD_DIR/DEBIAN/postinst"
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
EOF
chmod 755 "$BUILD_DIR/DEBIAN/postinst"

cat <<'EOF' > "$BUILD_DIR/DEBIAN/postrm"
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
EOF
chmod 755 "$BUILD_DIR/DEBIAN/postrm"

# Generate md5sums
(
    cd "$BUILD_DIR"
    find usr -type f -exec md5sum {} + > DEBIAN/md5sums
)

echo "==> [Debian/Ubuntu] Empaquetando archivo .deb..."
OUTPUT_DEB="$OUT_DIR/${PKG_NAME}.deb"

if command -v dpkg-deb >/dev/null 2>&1; then
    dpkg-deb --build --root-owner-group "$BUILD_DIR" "$OUTPUT_DEB"
else
    # Construcción nativa multiplataforma con ar y tar
    TEMP_PACK="$(mktemp -d)"
    echo "2.0" > "$TEMP_PACK/debian-binary"
    (
        cd "$BUILD_DIR/DEBIAN"
        tar --owner=0 --group=0 -czf "$TEMP_PACK/control.tar.gz" .
    )
    (
        cd "$BUILD_DIR"
        tar --owner=0 --group=0 -czf "$TEMP_PACK/data.tar.gz" usr
    )
    (
        cd "$TEMP_PACK"
        ar rcs "$OUTPUT_DEB" debian-binary control.tar.gz data.tar.gz
    )
    rm -rf "$TEMP_PACK"
fi

echo "==> [Debian/Ubuntu] ¡Paquete .deb generado con éxito en:"
ls -lh "$OUTPUT_DEB"
