@echo off
setlocal enabledelayedexpansion

echo ==================================================
echo   DocUpload v0.5.0 - Compilacion y Empaquetado Windows
echo ==================================================

cd /d "%~dp0\..\.."

:: 1. Comprobar entorno Rust/Cargo
where cargo >nul 2>nul
if %ERRORLEVEL% NEQ 0 (
    echo [ERROR] Cargo/Rust no esta instalado o no se encuentra en el PATH.
    echo Descargando e iniciando instalador rustup-init.exe...
    powershell -NoProfile -ExecutionPolicy Bypass -Command "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile '%TEMP%\rustup-init.exe'"
    if exist "%TEMP%\rustup-init.exe" (
        echo Ejecutando rustup-init... Sigue las instrucciones en pantalla.
        start /wait "" "%TEMP%\rustup-init.exe" -y
        set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
    ) else (
        echo No se pudo descargar rustup-init.exe. Por favor instala Rust desde https://rustup.rs/
        exit /b 1
    )
)

:: 2. Descargar librerias de soporte necesarias de Windows (Visual C++ Redistributable 2015-2022 x64)
echo [1/5] Verificando librerias de soporte necesarias (Microsoft Visual C++ Runtime)...
if not exist "packaging\windows\vc_redist.x64.exe" (
    echo Descargando Microsoft Visual C++ 2015-2022 Redistributable (x64)...
    powershell -NoProfile -ExecutionPolicy Bypass -Command "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vc_redist.x64.exe' -OutFile 'packaging\windows\vc_redist.x64.exe'"
    if exist "packaging\windows\vc_redist.x64.exe" (
        echo [OK] Librerias VC++ Redistributable descargadas correctamente.
    ) else (
        echo [AVISO] No se pudo descargar vc_redist.x64.exe. Verifica tu conexion a Internet.
    )
) else (
    echo [OK] Librerias VC++ Redistributable ya presentes en packaging\windows\vc_redist.x64.exe
)

:: 3. Compilacion de lanzamiento
echo [2/5] Compilando binario de lanzamiento para Windows...
cargo build --release
if %ERRORLEVEL% NEQ 0 (
    echo [ERROR] Error durante la compilacion.
    exit /b %ERRORLEVEL%
)

if not exist "target\packages" mkdir "target\packages"

:: 4. Generacion de archivo ZIP portable con librerias incluidas
echo [3/5] Generando archivo ZIP portable con librerias incluidas...
:: Crear script auxiliar para instalacion rapida de librerias en caso portable
echo @echo off > "target\release\instalar_librerias_vc.bat"
echo echo Instalando Microsoft Visual C++ Runtime necesario para DocUpload... >> "target\release\instalar_librerias_vc.bat"
echo if exist "vc_redist.x64.exe" ( start /wait "" "vc_redist.x64.exe" /install /quiet /norestart ) else ( echo No se encontro vc_redist.x64.exe ) >> "target\release\instalar_librerias_vc.bat"

if exist "packaging\windows\vc_redist.x64.exe" (
    copy /y "packaging\windows\vc_redist.x64.exe" "target\release\vc_redist.x64.exe" >nul
    powershell -Command "Compress-Archive -Path 'target\release\docu_ffacd.exe', 'ui\assets\app_icon.ico', 'target\release\vc_redist.x64.exe', 'target\release\instalar_librerias_vc.bat' -DestinationPath 'target\packages\DocuUpload-v0.5.0-windows-x64.zip' -Force"
) else (
    powershell -Command "Compress-Archive -Path 'target\release\docu_ffacd.exe', 'ui\assets\app_icon.ico' -DestinationPath 'target\packages\DocuUpload-v0.5.0-windows-x64.zip' -Force"
)

:: 5. Comprobar e instalar Inno Setup Compiler si falta
echo [4/5] Comprobando Inno Setup Compiler para generar instalador .exe...
set "ISCC_PATH="
where iscc >nul 2>nul
if %ERRORLEVEL% EQU 0 (
    set "ISCC_PATH=iscc"
) else if exist "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" (
    set "ISCC_PATH=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
) else if exist "%ProgramFiles%\Inno Setup 6\ISCC.exe" (
    set "ISCC_PATH=%ProgramFiles%\Inno Setup 6\ISCC.exe"
) else if exist "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" (
    set "ISCC_PATH=%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"
) else (
    echo Inno Setup no esta instalado. Descargando e instalando automaticamente...
    where winget >nul 2>nul
    if %ERRORLEVEL% EQU 0 (
        echo Instalando Inno Setup mediante winget...
        winget install JRSoftware.InnoSetup -e --silent --accept-source-agreements --accept-package-agreements
    ) else (
        echo Descargando instalador oficial de Inno Setup...
        powershell -NoProfile -ExecutionPolicy Bypass -Command "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri 'https://files.jrsoftware.org/is/6/innosetup-6.3.3.exe' -OutFile '%TEMP%\innosetup-setup.exe'"
        if exist "%TEMP%\innosetup-setup.exe" (
            echo Instalando Inno Setup silenciosamente...
            start /wait "" "%TEMP%\innosetup-setup.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
        )
    )
    if exist "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" (
        set "ISCC_PATH=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
    ) else if exist "%ProgramFiles%\Inno Setup 6\ISCC.exe" (
        set "ISCC_PATH=%ProgramFiles%\Inno Setup 6\ISCC.exe"
    )
)

:: 6. Compilacion de instalador con librerias integradas
echo [5/5] Compilando instalador Inno Setup...
if defined ISCC_PATH (
    echo Compilando instalador usando "!ISCC_PATH!"...
    "!ISCC_PATH!" packaging\windows\installer.iss
) else (
    echo [AVISO] Inno Setup no pudo ser instalado automaticamente. Se genero el archivo portable .zip en target\packages.
)

echo ==================================================
echo   Proceso de empaquetado finalizado.
echo   Archivos generados en target\packages:
dir target\packages
echo ==================================================
