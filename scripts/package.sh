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
    echo "  --install-deps Instalar automáticamente todas las librerías necesarias del sistema"
    echo "  --deb          Empaquetar para Debian y Ubuntu (.deb)"
    echo "  --rpm          Empaquetar para Fedora y RHEL (.rpm)"
    echo "  --arch         Empaquetar para Arch Linux (PKGBUILD / .pkg.tar.zst)"
    echo "  --appimage     Empaquetar como Linux AppImage autónomo (.AppImage)"
    echo "  --macos        Empaquetar para macOS (.app / .dmg)"
    echo "  --windows      Empaquetar para Windows (.zip portable con VC++ e instalador)"
    echo "  --all          Empaquetar todos los formatos compatibles con este sistema"
    echo "  --help         Mostrar esta ayuda"
    echo ""
    exit 0
}

if [ $# -eq 0 ]; then
    usage
fi

print_header

mkdir -p "$PROJECT_ROOT/target/packages"

install_system_deps() {
    echo -e "${GREEN}==> Verificando e instalando librerías y dependencias del sistema...${NC}"
    SUDO_CMD=""
    if [ "$EUID" -ne 0 ]; then
        if sudo -n true 2>/dev/null; then
            SUDO_CMD="sudo -n"
        elif [ -t 0 ] && command -v sudo >/dev/null 2>&1; then
            SUDO_CMD="sudo"
        fi
    fi

    if command -v apt-get >/dev/null 2>&1; then
        echo "Detectado gestor de paquetes APT (Debian / Ubuntu / Mint)..."
        DEPS="build-essential pkg-config libfontconfig1-dev libfreetype6-dev zlib1g-dev libx11-dev libxext-dev libxrender-dev zstd rpm curl zip"
        if [ -n "$SUDO_CMD" ]; then
            $SUDO_CMD apt-get update && $SUDO_CMD apt-get install -y $DEPS || echo -e "${YELLOW}Aviso: Falló la instalación automática de algunos paquetes con apt-get.${NC}"
        else
            echo -e "${YELLOW}Ejecuta con sudo para instalar automáticamente:${NC} sudo apt-get update && sudo apt-get install -y $DEPS"
        fi
    elif command -v dnf >/dev/null 2>&1; then
        echo "Detectado gestor de paquetes DNF (Fedora / RHEL / CentOS)..."
        DEPS="gcc gcc-c++ make pkgconf-pkg-config fontconfig-devel freetype-devel zlib-devel libX11-devel libXext-devel libXrender-devel rpm-build zstd curl zip"
        if [ -n "$SUDO_CMD" ]; then
            $SUDO_CMD dnf install -y $DEPS || echo -e "${YELLOW}Aviso: Falló la instalación automática de algunos paquetes con dnf.${NC}"
        else
            echo -e "${YELLOW}Ejecuta con sudo para instalar automáticamente:${NC} sudo dnf install -y $DEPS"
        fi
    elif command -v pacman >/dev/null 2>&1; then
        echo "Detectado gestor de paquetes Pacman (Arch Linux / Manjaro)..."
        DEPS="base-devel pkgconf fontconfig freetype2 zlib libx11 libxext libxrender zstd curl zip"
        if [ -n "$SUDO_CMD" ]; then
            $SUDO_CMD pacman -Sy --noconfirm --needed $DEPS || echo -e "${YELLOW}Aviso: Falló la instalación automática de algunos paquetes con pacman.${NC}"
        else
            echo -e "${YELLOW}Ejecuta con sudo para instalar automáticamente:${NC} sudo pacman -Sy --noconfirm --needed $DEPS"
        fi
    elif command -v zypper >/dev/null 2>&1; then
        echo "Detectado gestor de paquetes Zypper (openSUSE)..."
        DEPS="gcc gcc-c++ make pkg-config fontconfig-devel freetype2-devel zlib-devel libX11-devel rpm-build zstd curl zip"
        if [ -n "$SUDO_CMD" ]; then
            $SUDO_CMD zypper install -y $DEPS || echo -e "${YELLOW}Aviso: Falló la instalación automática de algunos paquetes con zypper.${NC}"
        else
            echo -e "${YELLOW}Ejecuta con sudo para instalar automáticamente:${NC} sudo zypper install -y $DEPS"
        fi
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        echo "Detectado sistema macOS..."
        if ! xcode-select -p >/dev/null 2>&1; then
            xcode-select --install || true
        fi
        if command -v brew >/dev/null 2>&1; then
            brew install create-dmg || true
        fi
    else
        echo -e "${YELLOW}Sistema o gestor de paquetes no reconocido para instalación automática.${NC}"
    fi
}

build_deb() {
    echo -e "${GREEN}==> Construyendo paquete para Debian / Ubuntu (.deb)...${NC}"
    "$PROJECT_ROOT/packaging/debian/build_deb.sh"
}

build_rpm() {
    echo -e "${GREEN}==> Construyendo paquete para Fedora / RHEL (.rpm)...${NC}"
    if command -v rpmbuild >/dev/null 2>&1; then
        "$PROJECT_ROOT/packaging/fedora/build_rpm.sh"
    else
        echo -e "${YELLOW}Aviso: rpmbuild no encontrado. Intentando instalar o compilar...${NC}"
        "$PROJECT_ROOT/packaging/fedora/build_rpm.sh"
    fi
}

build_arch() {
    echo -e "${GREEN}==> Construyendo paquete para Arch Linux (.pkg.tar.zst)...${NC}"
    "$PROJECT_ROOT/packaging/arch/build_arch.sh"
}

build_appimage() {
    echo -e "${GREEN}==> Construyendo paquete universal Linux AppImage (autónomo con librerías)...${NC}"
    "$PROJECT_ROOT/packaging/appimage/build_appimage.sh"
}

build_macos() {
    echo -e "${GREEN}==> Construyendo paquete para macOS (.app / .dmg)...${NC}"
    "$PROJECT_ROOT/packaging/macos/build_dmg.sh"
}

build_windows() {
    echo -e "${GREEN}==> Preparando empaquetado para Windows...${NC}"
    mkdir -p "$PROJECT_ROOT/packaging/windows"
    VC_REDIST="$PROJECT_ROOT/packaging/windows/vc_redist.x64.exe"
    if [ ! -f "$VC_REDIST" ]; then
        echo "Descargando librerías Microsoft Visual C++ 2015-2022 Redistributable (x64)..."
        curl -sL "https://aka.ms/vs/17/release/vc_redist.x64.exe" -o "$VC_REDIST" || true
    fi

    if [ -f "$PROJECT_ROOT/target/release/docu_ffacd.exe" ]; then
        echo "Binario .exe detectado, creando zip portable con librerías de soporte..."
        TEMP_WIN_ZIP="$(mktemp -d)"
        cp "$PROJECT_ROOT/target/release/docu_ffacd.exe" "$TEMP_WIN_ZIP/"
        cp "$PROJECT_ROOT/ui/assets/app_icon.ico" "$TEMP_WIN_ZIP/"
        if [ -f "$VC_REDIST" ]; then
            cp "$VC_REDIST" "$TEMP_WIN_ZIP/"
        fi
        cat <<'EOF' > "$TEMP_WIN_ZIP/instalar_librerias_vc.bat"
@echo off
echo Instalando dependencias de Microsoft Visual C++ Runtime para DocUpload...
if exist "vc_redist.x64.exe" (
    start /wait "" "vc_redist.x64.exe" /install /quiet /norestart
) else (
    echo No se encontro el archivo vc_redist.x64.exe
)
EOF
        (
            cd "$TEMP_WIN_ZIP"
            zip -j "$PROJECT_ROOT/target/packages/DocuUpload-v0.5.0-windows-x64.zip" *
        )
        rm -rf "$TEMP_WIN_ZIP"
        echo -e "${GREEN}¡Archivo ZIP portable para Windows generado en target/packages!${NC}"
    else
        echo -e "${YELLOW}Para compilar el binario .exe de Windows ejecuta desde Windows o con cross:${NC}"
        echo "  cargo build --release --target x86_64-pc-windows-msvc"
        echo "  (o ejecuta packaging/windows/build_windows.bat en Windows)"
    fi
}

for arg in "$@"; do
    case "$arg" in
        --install-deps)
            install_system_deps
            ;;
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
