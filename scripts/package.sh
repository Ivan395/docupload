#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

print_header() {
    echo -e "${BLUE}======================================================${NC}"
    echo -e "${BLUE}   DocuUpload v0.5.0 - Sistema de Empaquetado Multiplataforma${NC}"
    echo -e "${BLUE}======================================================${NC}"
}

usage() {
    print_header
    echo "Uso: $0 [OPCIONES]"
    echo ""
    echo "Opciones disponibles:"
    echo "  --deb        Empaquetar para Debian y Ubuntu (.deb)"
    echo "  --rpm        Empaquetar para Fedora y RHEL (.rpm)"
    echo "  --arch       Empaquetar para Arch Linux (PKGBUILD / .pkg.tar.zst)"
    echo "  --appimage   Empaquetar como Linux AppImage (.AppImage)"
    echo "  --macos      Empaquetar para macOS (.app / .dmg)"
    echo "  --windows    Empaquetar para Windows (.zip portable e instalador Inno Setup)"
    echo "  --all        Empaquetar todos los formatos compatibles con este sistema"
    echo "  --help       Mostrar esta ayuda"
    echo ""
    exit 0
}

if [ $# -eq 0 ]; then
    usage
fi

print_header

mkdir -p "$PROJECT_ROOT/target/packages"

build_deb() {
    echo -e "${GREEN}==> Construyendo paquete para Debian / Ubuntu (.deb)...${NC}"
    "$PROJECT_ROOT/packaging/debian/build_deb.sh"
}

build_rpm() {
    echo -e "${GREEN}==> Construyendo paquete para Fedora / RHEL (.rpm)...${NC}"
    if command -v rpmbuild >/dev/null 2>&1; then
        "$PROJECT_ROOT/packaging/fedora/build_rpm.sh"
    else
        echo -e "${YELLOW}Aviso: rpmbuild no encontrado. Instala 'rpm-build' para compilar RPM nativamente.${NC}"
    fi
}

build_arch() {
    echo -e "${GREEN}==> Construyendo paquete para Arch Linux (.pkg.tar.zst)...${NC}"
    "$PROJECT_ROOT/packaging/arch/build_arch.sh"
}

build_appimage() {
    echo -e "${GREEN}==> Construyendo paquete universal Linux AppImage...${NC}"
    "$PROJECT_ROOT/packaging/appimage/build_appimage.sh"
}

build_macos() {
    echo -e "${GREEN}==> Construyendo paquete para macOS (.app / .dmg)...${NC}"
    "$PROJECT_ROOT/packaging/macos/build_dmg.sh"
}

build_windows() {
    echo -e "${GREEN}==> Preparando empaquetado para Windows...${NC}"
    if [ -f "$PROJECT_ROOT/target/release/docu_ffacd.exe" ]; then
        echo "Binario .exe detectado, creando zip portable..."
        (
            cd "$PROJECT_ROOT"
            zip -j "$PROJECT_ROOT/target/packages/DocuUpload-v0.5.0-windows-x64.zip" \
                target/release/docu_ffacd.exe \
                ui/assets/app_icon.ico
        )
    else
        echo -e "${YELLOW}Para compilar el binario .exe de Windows ejecuta desde Windows o con cross:${NC}"
        echo "  cargo build --release --target x86_64-pc-windows-msvc"
        echo "  (o ejecuta packaging/windows/build_windows.bat en Windows)"
    fi
}

for arg in "$@"; do
    case "$arg" in
        --deb)
            build_deb
            ;;
        --rpm)
            build_rpm
            ;;
        --arch)
            build_arch
            ;;
        --appimage)
            build_appimage
            ;;
        --macos)
            build_macos
            ;;
        --windows)
            build_windows
            ;;
        --all)
            build_deb
            build_rpm
            build_arch
            build_appimage
            build_macos
            build_windows
            ;;
        --help|-h)
            usage
            ;;
        *)
            echo -e "${RED}Opción desconocida: $arg${NC}"
            usage
            ;;
    esac
done

echo ""
echo -e "${GREEN}======================================================${NC}"
echo -e "${GREEN}   Paquetes listos en target/packages:${NC}"
echo -e "${GREEN}======================================================${NC}"
ls -lh "$PROJECT_ROOT/target/packages"
