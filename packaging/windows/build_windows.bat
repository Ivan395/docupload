@echo off
setlocal enabledelayedexpansion

echo ==================================================
echo   DocuUpload v0.5.0 - Compilacion y Empaquetado Windows
echo ==================================================

cd /d "%~dp0\..\.."

echo [1/3] Compilando binario de lanzamiento para Windows...
cargo build --release
if %ERRORLEVEL% NEQ 0 (
    echo Error durante la compilacion.
    exit /b %ERRORLEVEL%
)

if not exist "target\packages" mkdir "target\packages"

echo [2/3] Generando archivo ZIP portable...
powershell -Command "Compress-Archive -Path 'target\release\docu_ffacd.exe', 'ui\assets\app_icon.ico' -DestinationPath 'target\packages\DocuUpload-v0.5.0-windows-x64.zip' -Force"

echo [3/3] Comprobando Inno Setup Compiler para generar instalador .exe...
where iscc >nul 2>nul
if %ERRORLEVEL% EQU 0 (
    echo Compilando instalador con Inno Setup...
    iscc packaging\windows\installer.iss
) else (
    if exist "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" (
        "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" packaging\windows\installer.iss
    ) else (
        echo Inno Setup no esta instalado en la ruta estandar. Puedes instalarlo para generar el instalador DocuUpload-Setup-0.5.0.exe.
    )
)

echo ==================================================
echo   Proceso de empaquetado finalizado.
echo   Archivos generados en target\packages:
dir target\packages
echo ==================================================
