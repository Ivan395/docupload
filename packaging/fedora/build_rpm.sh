#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "==> [Fedora RPM] Compilando binario de lanzamiento..."
cd "$PROJECT_ROOT"
cargo build --release

BUILD_DIR="$PROJECT_ROOT/target/rpmbuild"
OUT_DIR="$PROJECT_ROOT/target/packages"

rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
mkdir -p "$OUT_DIR"

echo "==> [Fedora RPM] Preparando fuentes para RPM..."
cp "$PROJECT_ROOT/target/release/docu_ffacd" "$BUILD_DIR/SOURCES/"
cp "$PROJECT_ROOT/packaging/linux/docu_ffacd.desktop" "$BUILD_DIR/SOURCES/"
cp "$PROJECT_ROOT/ui/assets/app_icon_256.png" "$BUILD_DIR/SOURCES/"
cp "$PROJECT_ROOT/ui/assets/app_icon_64.png" "$BUILD_DIR/SOURCES/"
cp "$PROJECT_ROOT/ui/assets/app_icon_32.png" "$BUILD_DIR/SOURCES/"
cp "$PROJECT_ROOT/ui/assets/app_icon.svg" "$BUILD_DIR/SOURCES/"
cp "$PROJECT_ROOT/packaging/fedora/docu_ffacd.spec" "$BUILD_DIR/SPECS/"

echo "==> [Fedora RPM] Ejecutando rpmbuild..."
rpmbuild --define "_topdir $BUILD_DIR" -ba "$BUILD_DIR/SPECS/docu_ffacd.spec"

find "$BUILD_DIR/RPMS" -name "*.rpm" -exec cp {} "$OUT_DIR/" \;

echo "==> [Fedora RPM] ¡Paquete RPM generado con éxito en:"
ls -lh "$OUT_DIR"/*.rpm 2>/dev/null || true
