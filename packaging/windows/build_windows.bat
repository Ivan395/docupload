@echo off
setlocal enabledelayedexpansion

echo ==================================================
echo   DocUpload v0.5.0 - Compilacion y Empaquetado Windows
echo ==================================================

cd /d "%~dp0\..\.."

:: 1. Comprobar entorno Rust/Cargo
where cargo >nul 2>nul
if %ERRORLEVEL% NEQ 0 (
    echo [ERROR] Cargo o Rust no esta instalado o no se encuentra en el PATH.
    exit /b 1
)

:: 2. Descargar librerias de soporte de Windows Visual C++ Redistributable
echo [1/5] Verificando librerias de soporte Visual C++ Runtime...
if not exist "packaging\windows\vc_redist.x64.exe" (
    echo Descargando Microsoft Visual C++ Redistributable x64...
    powershell -NoProfile -ExecutionPolicy Bypass -Command "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vc_redist.x64.exe' -OutFile 'packaging\windows\vc_redist.x64.exe'"
)

if exist "packaging\windows\vc_redist.x64.exe" (
    echo [OK] Librerias VC++ Redistributable listas.
) else (
    echo [AVISO] No se encontro vc_redist.x64.exe, continuando sin incluir runtime offline.
)

:: 3. Compilacion de lanzamiento si no existe el binario
echo [2/5] Verificando binario de lanzamiento para Windows...
if not exist "target\release\docu_ffacd.exe" (
    echo Compilando docu_ffacd.exe en modo release...
    cargo build --release
    if %ERRORLEVEL% NEQ 0 (
        echo [ERROR] Error durante la compilacion de Rust.
        exit /b %ERRORLEVEL%
    )
) else (
    echo [OK] Binario target\release\docu_ffacd.exe ya compilado.
)

if not exist "target\packages" mkdir "target\packages"

:: 4. Generacion de archivo ZIP portable con librerias incluidas
echo [3/5] Generando archivo ZIP portable con librerias incluidas...
powershell -NoProfile -Command "Set-Content -Path 'target\release\instalar_librerias_vc.bat' -Value '@echo off', 'echo Instalando Microsoft Visual C++ Runtime...', 'if exist vc_redist.x64.exe start /wait \"\" vc_redist.x64.exe /install /quiet /norestart'"

if exist "packaging\windows\vc_redist.x64.exe" (
    copy /y "packaging\windows\vc_redist.x64.exe" "target\release\vc_redist.x64.exe" >nul
    powershell -NoProfile -Command "Compress-Archive -Path 'target\release\docu_ffacd.exe', 'ui\assets\app_icon.ico', 'target\release\vc_redist.x64.exe', 'target\release\instalar_librerias_vc.bat' -DestinationPath 'target\packages\DocuUpload-v0.5.0-windows-x64.zip' -Force"
) else (
    powershell -NoProfile -Command "Compress-Archive -Path 'target\release\docu_ffacd.exe', 'ui\assets\app_icon.ico' -DestinationPath 'target\packages\DocuUpload-v0.5.0-windows-x64.zip' -Force"
)

:: 5. Comprobar Inno Setup Compiler para generar instalador .exe
echo [4/5] Comprobando Inno Setup Compiler para generar instalador .exe...
set "ISCC_PATH="
where iscc >nul 2>nul
if %ERRORLEVEL% EQU 0 set "ISCC_PATH=iscc"

if not defined ISCC_PATH (
    for %%D in (
        "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
        "%ProgramFiles%\Inno Setup 6\ISCC.exe"
        "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
        "C:\Program Files\Inno Setup 6\ISCC.exe"
    ) do (
        if not defined ISCC_PATH if exist %%D set "ISCC_PATH=%%~D"
    )
)

if not defined ISCC_PATH (
    echo Inno Setup no detectado en rutas estandar. Intentando instalar via winget o instalador oficial...
    where winget >nul 2>nul
    if %ERRORLEVEL% EQU 0 (
        winget install JRSoftware.InnoSetup -e --silent --accept-source-agreements --accept-package-agreements
    ) else (
        powershell -NoProfile -ExecutionPolicy Bypass -Command "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri 'https://files.jrsoftware.org/is/6/innosetup-6.3.3.exe' -OutFile '%TEMP%\innosetup-setup.exe'"
        if exist "%TEMP%\innosetup-setup.exe" (
            start /wait "" "%TEMP%\innosetup-setup.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
        )
    )
    for %%D in (
        "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
        "%ProgramFiles%\Inno Setup 6\ISCC.exe"
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
        "C:\Program Files\Inno Setup 6\ISCC.exe"
    ) do (
        if not defined ISCC_PATH if exist %%D set "ISCC_PATH=%%~D"
    )
)

:: 6. Compilacion de instalador Inno Setup
echo [5/5] Compilando instalador Inno Setup...
if defined ISCC_PATH (
    echo Compilando instalador usando: "!ISCC_PATH!"
    "!ISCC_PATH!" packaging\windows\installer.iss
    if %ERRORLEVEL% NEQ 0 (
        echo [ERROR] Error compilando el instalador con Inno Setup.
        exit /b %ERRORLEVEL%
    )
    echo [OK] Instalador generado exitosamente en target\packages.
) else (
    echo [AVISO] Inno Setup no encontrado. El paquete ZIP portable fue generado en target\packages.
)

echo ==================================================
echo   Proceso de empaquetado finalizado con exito.
echo ==================================================
