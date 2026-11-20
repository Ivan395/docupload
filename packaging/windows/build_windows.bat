@echo off
setlocal enabledelayedexpansion

cd /d "%~dp0\..\.."

:: 0. Leer la version desde Cargo.toml (fuente unica de verdad)
set "VERSION="
for /f "usebackq delims=" %%V in (`powershell -NoProfile -Command "(Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*\"(.+)\"' | Select-Object -First 1).Matches.Groups[1].Value"`) do set "VERSION=%%V"

if not defined VERSION (
    echo [ERROR] No se pudo leer la version desde Cargo.toml.
    exit /b 1
)

echo ==================================================
echo   DocUpload v%VERSION% - Compilacion y Empaquetado Windows
echo ==================================================

:: 1. Comprobar entorno Rust/Cargo
where cargo >nul 2>nul
if %ERRORLEVEL% NEQ 0 (
    echo [ERROR] Cargo o Rust no esta instalado o no se encuentra en el PATH.
    exit /b 1
)

:: 2. Compilacion de lanzamiento (siempre, salvo SKIP_BUILD=1)
echo [1/5] Compilacion de lanzamiento para Windows...
if not "%SKIP_BUILD%"=="1" (
    set "RUSTFLAGS=-C target-feature=+crt-static"
    REM Fuerza CRT estatico tambien en MuPDF compilado con MSBuild
    set "_CL_=/MT"
    echo Compilando docu_ffacd.exe en modo release...
    cargo build --release
    if !ERRORLEVEL! NEQ 0 (
        echo [ERROR] Error durante la compilacion de Rust.
        exit /b !ERRORLEVEL!
    )
) else (
    echo [OMITIDO] SKIP_BUILD=1: no se ejecuta cargo build.
)

:: Si se indica BIN_PATH, se usa ese binario como entrada del empaquetado
if defined BIN_PATH (
    echo Usando binario externo: "%BIN_PATH%"
    if not exist "target\release" mkdir "target\release"
    copy /y "%BIN_PATH%" "target\release\docu_ffacd.exe" >nul
    if !ERRORLEVEL! NEQ 0 (
        echo [ERROR] No se pudo copiar BIN_PATH a target\release\docu_ffacd.exe.
        exit /b 1
    )
)

if not exist "target\release\docu_ffacd.exe" (
    echo [ERROR] No existe target\release\docu_ffacd.exe.
    exit /b 1
)

:: 3. Verificar que el ejecutable no dependa del VC runtime dinamico
echo [2/5] Verificando dependencias del ejecutable...
where dumpbin >nul 2>nul
if !ERRORLEVEL! EQU 0 (
    dumpbin /dependents "target\release\docu_ffacd.exe" | findstr /I "VCRUNTIME MSVCP" >nul
    if !ERRORLEVEL! EQU 0 (
        echo [ERROR] El ejecutable depende de VCRUNTIME; el CRT estatico no se aplico.
        exit /b 1
    )
    echo [OK] El ejecutable no depende del VC runtime dinamico.
) else (
    echo [AVISO] dumpbin no esta en PATH; no se pudo verificar la dependencia del VC runtime.
)

if not exist "target\packages" mkdir "target\packages"

:: 4. Generacion de archivo ZIP portable
echo [3/5] Generando archivo ZIP portable...
powershell -NoProfile -Command "Compress-Archive -Path 'target\release\docu_ffacd.exe', 'ui\assets\app_icon.ico' -DestinationPath 'target\packages\DocuUpload-%VERSION%-windows-x86_64-portable.zip' -Force"
if !ERRORLEVEL! NEQ 0 (
    echo [ERROR] No se pudo generar el ZIP portable.
    exit /b 1
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
    if !ERRORLEVEL! EQU 0 (
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
    "!ISCC_PATH!" /DMyAppVersion=%VERSION% packaging\windows\installer.iss
    if !ERRORLEVEL! NEQ 0 (
        echo [ERROR] Error compilando el instalador con Inno Setup.
        exit /b !ERRORLEVEL!
    )
    echo [OK] Instalador generado exitosamente en target\packages.
) else (
    echo [AVISO] Inno Setup no encontrado. El paquete ZIP portable fue generado en target\packages.
)

echo ==================================================
echo   Proceso de empaquetado finalizado con exito.
echo ==================================================
