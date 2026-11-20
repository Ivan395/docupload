//! Comprobación, descarga y aplicación de actualizaciones desde GitHub Releases.
//!
//! La forma de instalar la aplicación (instalador de Windows, ejecutable portable,
//! AppImage, paquete .deb/.rpm/pacman, bundle de macOS o desarrollo) determina qué
//! archivo se descarga y cómo se aplica.

use crate::paths::{data_dir, ensure_dir, updates_dir};
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Repositorio de GitHub de donde se obtienen las versiones.
pub const REPO: &str = "Ivan395/docupload";

const VERSION_ACTUAL: &str = env!("CARGO_PKG_VERSION");
const URL_ULTIMA_RELEASE: &str = "https://api.github.com/repos/Ivan395/docupload/releases/latest";
const INTERVALO_CHECK_SEGUNDOS: u64 = 6 * 60 * 60;
const NOTAS_POR_DEFECTO: &str = "Actualización con mejoras de rendimiento y correcciones de estabilidad.";
const INSTALACION_NO_RECONOCIDA: &str =
    "Instalación no reconocida; descarga la versión desde la página de releases.";

/// Forma en la que está instalada la aplicación en este equipo.
#[allow(dead_code)] // cada SO construye solo algunas variantes
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// Instalador de Windows (existe unins000.exe junto al ejecutable).
    WindowsInstaller,
    /// Ejecutable portable de Windows.
    WindowsPortable { exe: PathBuf },
    /// AppImage de Linux (ruta tomada de la variable APPIMAGE).
    AppImage { path: PathBuf },
    /// Paquete .deb instalado en /usr.
    Deb,
    /// Paquete .rpm instalado en /usr.
    Rpm,
    /// Paquete de pacman instalado en /usr.
    Pacman,
    /// Aplicación dentro de un bundle `.app` de macOS.
    MacApp { bundle: PathBuf },
    /// Ejecución en desarrollo (cargo run) u origen desconocido.
    Unknown,
}

/// Resultado de una comprobación de actualización disponible.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub tag_name: String,
    pub version: String,
    pub html_url: String,
    pub notes: String,
    /// `None` cuando no hay un asset compatible: la UI abre `html_url` en el navegador.
    pub asset: Option<ReleaseAsset>,
    pub install_kind: InstallKind,
}

/// Archivo adjunto a una release de GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    /// Digest publicado por GitHub, p. ej. `sha256:<hex>`.
    #[serde(default)]
    pub digest: Option<String>,
}

/// Origen de la comprobación: al arrancar usa caché; la manual siempre consulta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckMode {
    Startup,
    Manual,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CacheActualizacion {
    etag: Option<String>,
    body: Option<String>,
    checked_at: u64,
}

fn segundos_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn user_agent() -> String {
    format!(
        "DocuUpload/{} ({}; {})",
        VERSION_ACTUAL,
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Determina cómo está instalada la aplicación en este equipo.
#[cfg(target_os = "windows")]
pub fn detect_install_kind() -> InstallKind {
    let Ok(exe) = std::env::current_exe() else {
        return InstallKind::Unknown;
    };
    let es_instalador = exe
        .parent()
        .is_some_and(|dir| dir.join("unins000.exe").exists());
    if es_instalador {
        InstallKind::WindowsInstaller
    } else {
        InstallKind::WindowsPortable { exe }
    }
}

/// Determina cómo está instalada la aplicación en este equipo.
#[cfg(target_os = "linux")]
pub fn detect_install_kind() -> InstallKind {
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|v| !v.is_empty()) {
        return InstallKind::AppImage {
            path: PathBuf::from(appimage),
        };
    }
    let Ok(exe) = std::env::current_exe() else {
        return InstallKind::Unknown;
    };
    if exe.starts_with("/usr/") {
        if perteneciente_a_paquete("dpkg-query", &["-S"], &exe) {
            return InstallKind::Deb;
        }
        if perteneciente_a_paquete("rpm", &["-qf"], &exe) {
            return InstallKind::Rpm;
        }
        if perteneciente_a_paquete("pacman", &["-Qo"], &exe) {
            return InstallKind::Pacman;
        }
    }
    InstallKind::Unknown
}

/// Indica si el gestor de paquetes reconoce el ejecutable como parte de un paquete.
#[cfg(target_os = "linux")]
fn perteneciente_a_paquete(programa: &str, args: &[&str], exe: &Path) -> bool {
    Command::new(programa)
        .args(args)
        .arg(exe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Determina cómo está instalada la aplicación en este equipo.
#[cfg(target_os = "macos")]
pub fn detect_install_kind() -> InstallKind {
    let Ok(exe) = std::env::current_exe() else {
        return InstallKind::Unknown;
    };
    for ancestro in exe.ancestors() {
        if ancestro.extension().is_some_and(|e| e == "app") {
            if exe.starts_with(ancestro.join("Contents").join("MacOS")) {
                return InstallKind::MacApp {
                    bundle: ancestro.to_path_buf(),
                };
            }
            break;
        }
    }
    InstallKind::Unknown
}

/// Determina cómo está instalada la aplicación en este equipo.
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub fn detect_install_kind() -> InstallKind {
    InstallKind::Unknown
}

/// Elige el asset de la release que corresponde a la forma de instalación.
pub fn select_asset(kind: &InstallKind, assets: &[ReleaseAsset]) -> Option<ReleaseAsset> {
    let sufijo = match kind {
        InstallKind::WindowsInstaller => "-windows-x86_64-setup.exe",
        InstallKind::WindowsPortable { .. } => "-windows-x86_64-portable.zip",
        InstallKind::AppImage { .. } => "-linux-x86_64.AppImage",
        InstallKind::Deb => "_amd64.deb",
        InstallKind::Rpm => ".x86_64.rpm",
        InstallKind::Pacman => "-x86_64.pkg.tar.zst",
        InstallKind::MacApp { .. } => "-macos-universal.dmg",
        InstallKind::Unknown => return None,
    };
    assets.iter().find(|a| a.name.ends_with(sufijo)).cloned()
}

/// Interpreta un tag de versión ("v0.6.0", "0.6.0", "V1.2.3", ...).
pub fn parse_version(tag: &str) -> Option<semver::Version> {
    let limpio = tag.trim();
    let limpio = limpio
        .strip_prefix('v')
        .or_else(|| limpio.strip_prefix('V'))
        .unwrap_or(limpio);
    if let Ok(v) = semver::Version::parse(limpio) {
        return Some(v);
    }
    // Tags heredados con formato no semver: se usa solo la parte antes del primer '-'.
    let base = limpio.split('-').next()?;
    semver::Version::parse(base).ok()
}

/// Evalúa el JSON de una release y devuelve la actualización si es más reciente que `current`.
pub fn evaluar_release(
    json: &str,
    current: &str,
    kind: &InstallKind,
) -> Result<Option<UpdateInfo>, String> {
    let release: GitHubRelease = serde_json::from_str(json)
        .map_err(|e| format!("Error interpretando respuesta de actualización: {e}"))?;

    if release.draft || release.prerelease {
        return Ok(None);
    }

    let version_actual = parse_version(current)
        .ok_or_else(|| format!("Versión actual inválida: '{current}'"))?;
    let version_remota = parse_version(&release.tag_name)
        .ok_or_else(|| format!("Versión remota inválida: '{}'", release.tag_name))?;

    if version_remota <= version_actual {
        return Ok(None);
    }

    log::info!("Actualización disponible: {}", version_remota);

    let notes = release
        .body
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| NOTAS_POR_DEFECTO.to_string());

    Ok(Some(UpdateInfo {
        tag_name: release.tag_name,
        version: version_remota.to_string(),
        html_url: release.html_url,
        notes,
        asset: select_asset(kind, &release.assets),
        install_kind: kind.clone(),
    }))
}

fn ruta_cache() -> PathBuf {
    data_dir().join("update_cache.json")
}

fn leer_cache() -> Option<CacheActualizacion> {
    let texto = fs::read_to_string(ruta_cache()).ok()?;
    match serde_json::from_str(&texto) {
        Ok(c) => Some(c),
        Err(e) => {
            log::warn!("Caché de actualizaciones inválida: {}", e);
            None
        }
    }
}

fn guardar_cache(cache: &CacheActualizacion) {
    let ruta = ruta_cache();
    if let Some(dir) = ruta.parent()
        && let Err(e) = ensure_dir(dir)
    {
        log::warn!("No se pudo crear la carpeta de caché de actualizaciones: {}", e);
        return;
    }
    match serde_json::to_string(cache) {
        Ok(texto) => {
            if let Err(e) = fs::write(&ruta, texto) {
                log::warn!("No se pudo guardar la caché de actualizaciones: {}", e);
            }
        }
        Err(e) => log::warn!("No se pudo serializar la caché de actualizaciones: {}", e),
    }
}

/// Mensaje para el usuario cuando GitHub limita las consultas.
fn mensaje_limite(headers: &HeaderMap) -> String {
    let reinicio = headers
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<i64>().ok())
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|d| d.with_timezone(&chrono::Local));
    match reinicio {
        Some(hora) => format!(
            "Límite de consultas a GitHub alcanzado; intenta de nuevo después de las {}",
            hora.format("%H:%M")
        ),
        None => "Límite de consultas a GitHub alcanzado; intenta de nuevo más tarde".to_string(),
    }
}

/// Consulta la última release en GitHub (con caché y ETag) y devuelve la actualización si existe.
pub fn check_for_updates(mode: CheckMode) -> Result<Option<UpdateInfo>, String> {
    let kind = detect_install_kind();
    let ahora = segundos_unix();
    let cache = leer_cache().filter(|c| c.body.is_some());

    if mode == CheckMode::Startup
        && let Some(c) = &cache
        && ahora.saturating_sub(c.checked_at) < INTERVALO_CHECK_SEGUNDOS
    {
        log::info!("Usando resultado de actualización en caché");
        return evaluar_release(c.body.as_deref().unwrap_or_default(), VERSION_ACTUAL, &kind);
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Error creando cliente HTTP: {e}"))?;

    let mut peticion = client
        .get(URL_ULTIMA_RELEASE)
        .header("User-Agent", user_agent())
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(etag) = cache.as_ref().and_then(|c| c.etag.as_deref()) {
        peticion = peticion.header("If-None-Match", etag);
    }

    let respuesta = peticion
        .send()
        .map_err(|e| format!("Error conectando con el servidor de actualizaciones: {e}"))?;
    let estado = respuesta.status();

    if estado == StatusCode::NOT_MODIFIED {
        let mut c = cache.ok_or_else(|| "Respuesta 304 sin caché local".to_string())?;
        c.checked_at = ahora;
        guardar_cache(&c);
        return evaluar_release(c.body.as_deref().unwrap_or_default(), VERSION_ACTUAL, &kind);
    }

    if estado == StatusCode::FORBIDDEN || estado == StatusCode::TOO_MANY_REQUESTS {
        return Err(mensaje_limite(respuesta.headers()));
    }

    if estado == StatusCode::NOT_FOUND {
        // No hay releases públicos aún
        return Ok(None);
    }

    if !estado.is_success() {
        return Err(format!("El servidor de actualizaciones respondió con código {estado}"));
    }

    let etag = respuesta
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = respuesta
        .text()
        .map_err(|e| format!("Error leyendo respuesta del servidor: {e}"))?;

    guardar_cache(&CacheActualizacion {
        etag,
        body: Some(body.clone()),
        checked_at: ahora,
    });

    evaluar_release(&body, VERSION_ACTUAL, &kind)
}

/// Digest SHA-256 esperado en hexadecimal minúscula, si GitHub lo publicó.
pub fn digest_esperado(digest: &Option<String>) -> Option<String> {
    let valor = digest.as_deref()?;
    let hex = valor.strip_prefix("sha256:")?;
    Some(hex.to_ascii_lowercase())
}

/// Descarga el asset de la actualización a `part`, verificando tamaño y SHA-256.
fn descargar_a_parcial<F: Fn(f32, &str)>(
    asset: &ReleaseAsset,
    part: &Path,
    progress: &F,
) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(None::<Duration>)
        .build()
        .map_err(|e| format!("Error creando cliente de descarga: {e}"))?;

    let mut respuesta = client
        .get(&asset.browser_download_url)
        .header("User-Agent", user_agent())
        .send()
        .map_err(|e| format!("Error iniciando descarga: {e}"))?;

    if !respuesta.status().is_success() {
        return Err(format!(
            "El servidor rechazó la descarga con código {}",
            respuesta.status()
        ));
    }

    let mut archivo = File::create(part)
        .map_err(|e| format!("Error creando archivo destino temporal: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut descargados: u64 = 0;
    let total = asset.size;
    // Se avisa a la UI solo cuando cambia el porcentaje (o cada MB sin tamaño conocido)
    let mut ultimo_aviso: Option<u64> = None;

    loop {
        let leidos = respuesta
            .read(&mut buffer)
            .map_err(|e| format!("Error leyendo datos de la descarga: {e}"))?;
        if leidos == 0 {
            break;
        }

        archivo
            .write_all(&buffer[..leidos])
            .map_err(|e| format!("Error escribiendo en archivo temporal: {e}"))?;
        hasher.update(&buffer[..leidos]);
        descargados += leidos as u64;

        let marca = descargados
            .checked_mul(100)
            .and_then(|d| d.checked_div(total))
            .unwrap_or(descargados / 1_048_576);
        if ultimo_aviso == Some(marca) {
            continue;
        }
        ultimo_aviso = Some(marca);

        let mb_descargados = descargados as f64 / 1_048_576.0;
        if total > 0 {
            let frac = (descargados as f32 / total as f32).clamp(0.0, 1.0);
            let mb_total = total as f64 / 1_048_576.0;
            let status = format!(
                "{:.1} MB de {:.1} MB ({:.0}%)",
                mb_descargados,
                mb_total,
                frac * 100.0
            );
            progress(frac, &status);
        } else {
            progress(0.5, &format!("{:.1} MB descargados", mb_descargados));
        }
    }

    archivo
        .flush()
        .map_err(|e| format!("Error escribiendo en archivo temporal: {e}"))?;
    drop(archivo);

    if asset.size > 0 && descargados != asset.size {
        return Err(format!(
            "Tamaño descargado incorrecto ({} de {} bytes)",
            descargados, asset.size
        ));
    }

    if let Some(esperado) = digest_esperado(&asset.digest) {
        let obtenido: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        if obtenido != esperado {
            return Err("La verificación SHA-256 falló; el archivo se descartó".to_string());
        }
    }

    Ok(())
}

/// Descarga el asset de la actualización reportando progreso.
pub fn download_update_file<F: Fn(f32, &str) + Send + 'static>(
    info: &UpdateInfo,
    progress: F,
) -> Result<PathBuf, String> {
    let asset = info
        .asset
        .as_ref()
        .ok_or_else(|| "No hay un archivo descargable para esta instalación".to_string())?;

    let dir = updates_dir();
    ensure_dir(&dir).map_err(|e| format!("Error creando carpeta de actualizaciones: {e}"))?;

    let final_path = dir.join(&asset.name);
    let part_path = dir.join(format!("{}.part", asset.name));
    let _ = fs::remove_file(&part_path);

    if let Err(e) = descargar_a_parcial(asset, &part_path, &progress) {
        let _ = fs::remove_file(&part_path);
        return Err(e);
    }

    let _ = fs::remove_file(&final_path);
    if let Err(e) = fs::rename(&part_path, &final_path) {
        let _ = fs::remove_file(&part_path);
        return Err(format!("Error finalizando la descarga: {e}"));
    }

    progress(1.0, "Descarga completada");
    Ok(final_path)
}

/// Aplica la actualización descargada según la forma de instalación.
///
/// En caso de éxito normalmente termina el proceso con `std::process::exit(0)`.
pub fn apply_update_now(file: &Path, info: &UpdateInfo) -> Result<(), String> {
    aplicar_segun_so(file, &info.install_kind)
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn aplicar_segun_so(_file: &Path, _kind: &InstallKind) -> Result<(), String> {
    Err("La actualización automática no está disponible en este sistema operativo.".to_string())
}

#[cfg(target_os = "windows")]
fn aplicar_segun_so(file: &Path, kind: &InstallKind) -> Result<(), String> {
    match kind {
        InstallKind::WindowsInstaller => {
            Command::new(file)
                .args([
                    "/SILENT",
                    "/SUPPRESSMSGBOXES",
                    "/NORESTART",
                    "/CLOSEAPPLICATIONS",
                    "/RELAUNCH=1",
                ])
                .spawn()
                .map_err(|e| format!("Error ejecutando el instalador de Windows: {e}"))?;
            std::process::exit(0);
        }
        InstallKind::WindowsPortable { exe } => {
            reemplazar_exe_portable(file, exe)?;
            Command::new(exe)
                .spawn()
                .map_err(|e| format!("Error ejecutando la nueva versión: {e}"))?;
            std::process::exit(0);
        }
        _ => Err(INSTALACION_NO_RECONOCIDA.to_string()),
    }
}

/// Extrae `docu_ffacd.exe` del zip y lo intercambia con el ejecutable en uso.
#[cfg(target_os = "windows")]
fn reemplazar_exe_portable(zip_path: &Path, exe: &Path) -> Result<(), String> {
    let nuevo = exe.with_extension("exe.new");
    let viejo = exe.with_extension("exe.old");

    let _ = fs::remove_file(&nuevo);
    if let Err(e) = extraer_ejecutable_zip(zip_path, &nuevo) {
        let _ = fs::remove_file(&nuevo);
        return Err(e);
    }

    let _ = fs::remove_file(&viejo);
    if let Err(e) = fs::rename(exe, &viejo) {
        let _ = fs::remove_file(&nuevo);
        return Err(format!("No se pudo mover el ejecutable actual: {e}"));
    }
    if let Err(e) = fs::rename(&nuevo, exe) {
        let _ = fs::rename(&viejo, exe);
        let _ = fs::remove_file(&nuevo);
        return Err(format!("No se pudo colocar la nueva versión: {e}"));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn extraer_ejecutable_zip(zip_path: &Path, destino: &Path) -> Result<(), String> {
    const NOMBRE: &str = "docu_ffacd.exe";
    let archivo = File::open(zip_path).map_err(|e| format!("Error abriendo el paquete: {e}"))?;
    let mut zip = zip::ZipArchive::new(archivo)
        .map_err(|e| format!("Error leyendo el paquete comprimido: {e}"))?;

    for i in 0..zip.len() {
        let mut entrada = zip
            .by_index(i)
            .map_err(|e| format!("Error leyendo entrada del paquete: {e}"))?;
        let es_objetivo = Path::new(entrada.name())
            .file_name()
            .is_some_and(|n| n == NOMBRE);
        if !es_objetivo || entrada.is_dir() {
            continue;
        }
        let mut salida =
            File::create(destino).map_err(|e| format!("Error creando la nueva versión: {e}"))?;
        std::io::copy(&mut entrada, &mut salida)
            .map_err(|e| format!("Error extrayendo la nueva versión: {e}"))?;
        return Ok(());
    }
    Err(format!("El paquete no contiene {NOMBRE}"))
}

#[cfg(target_os = "linux")]
fn aplicar_segun_so(file: &Path, kind: &InstallKind) -> Result<(), String> {
    match kind {
        InstallKind::AppImage { path } => {
            reemplazar_appimage(file, path)?;
            Command::new(path)
                .spawn()
                .map_err(|e| format!("Error ejecutando la nueva versión: {e}"))?;
            std::process::exit(0);
        }
        InstallKind::Deb => instalar_con_pkexec(&["apt-get", "install", "-y"], file),
        InstallKind::Rpm => {
            if en_path("dnf") {
                instalar_con_pkexec(&["dnf", "install", "-y"], file)
            } else if en_path("zypper") {
                // El .rpm no va firmado; su integridad ya se verificó con SHA-256.
                instalar_con_pkexec(
                    &["zypper", "--non-interactive", "--no-gpg-checks", "install"],
                    file,
                )
            } else {
                Err(format!(
                    "No se encontró dnf ni zypper. Instala la actualización manualmente con: sudo rpm -U {}",
                    file.display()
                ))
            }
        }
        InstallKind::Pacman => instalar_con_pkexec(&["pacman", "-U", "--noconfirm"], file),
        _ => Err(INSTALACION_NO_RECONOCIDA.to_string()),
    }
}

/// Sustituye el AppImage en uso de forma atómica; el original no se toca si algo falla.
#[cfg(target_os = "linux")]
fn reemplazar_appimage(nuevo: &Path, actual: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let nombre = actual
        .file_name()
        .ok_or_else(|| "Ruta de AppImage inválida".to_string())?
        .to_string_lossy()
        .to_string();
    let padre = actual
        .parent()
        .ok_or_else(|| "Ruta de AppImage inválida".to_string())?;
    let temporal = padre.join(format!(".{nombre}.new"));

    let resultado = (|| -> Result<(), String> {
        fs::copy(nuevo, &temporal).map_err(|e| format!("Error copiando la nueva versión: {e}"))?;
        fs::set_permissions(&temporal, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Error ajustando permisos: {e}"))?;
        fs::rename(&temporal, actual)
            .map_err(|e| format!("Error reemplazando el AppImage: {e}"))?;
        Ok(())
    })();

    if resultado.is_err() {
        let _ = fs::remove_file(&temporal);
    }
    resultado
}

/// Indica si un programa existe en el PATH.
#[cfg(target_os = "linux")]
fn en_path(programa: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|d| d.join(programa).is_file()))
        .unwrap_or(false)
}

/// Ruta del ejecutable en uso, sin el sufijo " (deleted)" que añade Linux
/// cuando el archivo original fue reemplazado.
#[cfg(target_os = "linux")]
fn ejecutable_actual() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Error localizando la aplicación: {e}"))?;
    Ok(sin_sufijo_deleted(exe))
}

#[cfg(any(target_os = "linux", test))]
fn sin_sufijo_deleted(exe: PathBuf) -> PathBuf {
    match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(limpio) => PathBuf::from(limpio),
        None => exe,
    }
}

/// Instala el paquete con privilegios elevados (pkexec) y relanza la aplicación.
#[cfg(target_os = "linux")]
fn instalar_con_pkexec(comando: &[&str], file: &Path) -> Result<(), String> {
    if !en_path("pkexec") {
        return Err(format!(
            "No se encontró pkexec. Instala la actualización manualmente con: sudo {} {}",
            comando.join(" "),
            file.display()
        ));
    }

    // Tras reemplazar el paquete, /proc/self/exe apunta a "<ruta> (deleted)":
    // la ruta se toma antes de instalar.
    let actual = ejecutable_actual()?;

    let estado = Command::new("pkexec")
        .args(comando)
        .arg(file)
        .status()
        .map_err(|e| format!("Error ejecutando pkexec: {e}"))?;
    if !estado.success() {
        let codigo = estado
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "desconocido".to_string());
        return Err(format!("La instalación terminó con código de salida {codigo}"));
    }

    Command::new(actual)
        .spawn()
        .map_err(|e| format!("Error reiniciando la aplicación: {e}"))?;
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
fn aplicar_segun_so(file: &Path, kind: &InstallKind) -> Result<(), String> {
    match kind {
        InstallKind::MacApp { bundle } => reemplazar_app_macos(file, bundle),
        _ => Err(INSTALACION_NO_RECONOCIDA.to_string()),
    }
}

/// Abre la imagen de disco para que el usuario copie la app a mano.
#[cfg(target_os = "macos")]
fn abrir_dmg_para_copia_manual(dmg: &Path) {
    let _ = Command::new("open").arg(dmg).spawn();
}

#[cfg(target_os = "macos")]
fn reemplazar_app_macos(dmg: &Path, bundle: &Path) -> Result<(), String> {
    let Some(padre) = bundle.parent() else {
        return Err("Ruta de la aplicación inválida".to_string());
    };
    let montaje = updates_dir().join(format!("mnt-{}", std::process::id()));
    let staging = padre.join(".DocUpload.app.new");
    let viejo = padre.join(".DocUpload.app.old");

    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_dir_all(&viejo);

    if let Err(e) = preparar_app_desde_dmg(dmg, &montaje, &staging) {
        let _ = fs::remove_dir_all(&staging);
        abrir_dmg_para_copia_manual(dmg);
        return Err(format!(
            "No se pudo instalar automáticamente ({e}). Copia DocUpload.app manualmente desde la imagen que se acaba de abrir."
        ));
    }

    if let Err(e) = fs::rename(bundle, &viejo) {
        let _ = fs::remove_dir_all(&staging);
        abrir_dmg_para_copia_manual(dmg);
        return Err(format!(
            "No se pudo reemplazar la aplicación ({e}). Copia DocUpload.app manualmente desde la imagen que se acaba de abrir."
        ));
    }
    if let Err(e) = fs::rename(&staging, bundle) {
        let _ = fs::rename(&viejo, bundle);
        let _ = fs::remove_dir_all(&staging);
        abrir_dmg_para_copia_manual(dmg);
        return Err(format!(
            "No se pudo colocar la nueva versión ({e}). Copia DocUpload.app manualmente desde la imagen que se acaba de abrir."
        ));
    }
    let _ = fs::remove_dir_all(&viejo);

    Command::new("open")
        .arg("-n")
        .arg(bundle)
        .spawn()
        .map_err(|e| format!("Error reiniciando la aplicación: {e}"))?;
    std::process::exit(0);
}

/// Monta el dmg, copia DocUpload.app a `staging` y desmonta siempre.
#[cfg(target_os = "macos")]
fn preparar_app_desde_dmg(dmg: &Path, montaje: &Path, staging: &Path) -> Result<(), String> {
    crate::paths::ensure_dir(montaje).map_err(|e| format!("crear punto de montaje: {e}"))?;

    let montado = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(montaje)
        .arg(dmg)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !montado {
        let _ = fs::remove_dir(montaje);
        return Err("no se pudo montar la imagen de disco".to_string());
    }

    let resultado = copiar_app_montada(montaje, staging);

    let _ = Command::new("hdiutil")
        .arg("detach")
        .arg(montaje)
        .arg("-force")
        .status();
    let _ = fs::remove_dir(montaje);
    resultado
}

#[cfg(target_os = "macos")]
fn copiar_app_montada(montaje: &Path, staging: &Path) -> Result<(), String> {
    let lector = fs::read_dir(montaje).map_err(|e| format!("leer imagen montada: {e}"))?;
    let origen = lector
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".app"))
        })
        .ok_or_else(|| "la imagen no contiene una aplicación .app".to_string())?;

    let copiado = Command::new("ditto")
        .arg(&origen)
        .arg(staging)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !copiado {
        return Err("no se pudo copiar la aplicación".to_string());
    }
    Ok(())
}

/// Limpieza best-effort de restos de actualizaciones anteriores. Nunca falla.
pub fn limpiar_restos_actualizacion() {
    let _ = fs::remove_dir_all(updates_dir());

    if cfg!(target_os = "windows")
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = fs::remove_file(exe.with_extension("exe.old"));
    }

    // Solo se revisan AppImage y macOS: detect_install_kind() en Linux consultaría
    // dpkg/rpm/pacman, algo lento para hacerlo en cada arranque.
    #[cfg(target_os = "linux")]
    if let Some(path) = std::env::var_os("APPIMAGE").filter(|v| !v.is_empty()).map(PathBuf::from)
        && let (Some(padre), Some(nombre)) = (path.parent(), path.file_name())
    {
        let _ = fs::remove_file(padre.join(format!(".{}.new", nombre.to_string_lossy())));
    }

    #[cfg(target_os = "macos")]
    if let InstallKind::MacApp { bundle } = detect_install_kind()
        && let Some(padre) = bundle.parent()
    {
        let _ = fs::remove_dir_all(padre.join(".DocUpload.app.old"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(nombre: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: nombre.to_string(),
            browser_download_url: format!("https://example.invalid/{nombre}"),
            size: 1,
            digest: None,
        }
    }

    fn assets_release_0_6_0() -> Vec<ReleaseAsset> {
        [
            "DocuUpload-0.6.0-windows-x86_64-setup.exe",
            "DocuUpload-0.6.0-windows-x86_64-portable.zip",
            "DocuUpload-0.6.0-linux-x86_64.AppImage",
            "docu-ffacd_0.6.0_amd64.deb",
            "docu_ffacd-0.6.0-1.x86_64.rpm",
            "docu_ffacd-0.6.0-1-x86_64.pkg.tar.zst",
            "DocuUpload-0.6.0-macos-universal.dmg",
            "SHA256SUMS",
        ]
        .iter()
        .map(|n| asset(n))
        .collect()
    }

    fn release_json(tag: &str, draft: bool, prerelease: bool, assets: &[&str]) -> String {
        let assets: Vec<serde_json::Value> = assets
            .iter()
            .map(|n| {
                serde_json::json!({
                    "name": n,
                    "browser_download_url": format!("https://example.invalid/{n}"),
                    "size": 10,
                })
            })
            .collect();
        serde_json::json!({
            "tag_name": tag,
            "html_url": "https://example.invalid/release",
            "body": null,
            "draft": draft,
            "prerelease": prerelease,
            "assets": assets,
        })
        .to_string()
    }

    #[test]
    fn parse_version_acepta_formatos_comunes() {
        assert_eq!(parse_version("v0.6.0"), semver::Version::parse("0.6.0").ok());
        assert_eq!(parse_version("0.6.0"), semver::Version::parse("0.6.0").ok());
        assert_eq!(parse_version("V1.2.3"), semver::Version::parse("1.2.3").ok());
        assert_eq!(parse_version("  v0.6.0 "), semver::Version::parse("0.6.0").ok());
        assert!(parse_version("0.5.0-1").is_some());
        assert!(parse_version("basura").is_none());
    }

    #[test]
    fn parse_version_prerelease_es_menor_que_final() {
        let beta = parse_version("v0.6.0-beta.1").unwrap();
        let final_ = parse_version("v0.6.0").unwrap();
        assert!(beta < final_);
    }

    #[test]
    fn select_asset_elige_el_paquete_de_cada_instalacion() {
        let assets = assets_release_0_6_0();
        let caso = |kind: InstallKind, esperado: &str| {
            let elegido = select_asset(&kind, &assets).expect("debe haber asset");
            assert_eq!(elegido.name, esperado, "kind={kind:?}");
        };
        caso(InstallKind::WindowsInstaller, "DocuUpload-0.6.0-windows-x86_64-setup.exe");
        caso(
            InstallKind::WindowsPortable { exe: PathBuf::from("C:/x/docu_ffacd.exe") },
            "DocuUpload-0.6.0-windows-x86_64-portable.zip",
        );
        caso(
            InstallKind::AppImage { path: PathBuf::from("/home/u/DocuUpload.AppImage") },
            "DocuUpload-0.6.0-linux-x86_64.AppImage",
        );
        caso(InstallKind::Deb, "docu-ffacd_0.6.0_amd64.deb");
        caso(InstallKind::Rpm, "docu_ffacd-0.6.0-1.x86_64.rpm");
        caso(InstallKind::Pacman, "docu_ffacd-0.6.0-1-x86_64.pkg.tar.zst");
        caso(
            InstallKind::MacApp { bundle: PathBuf::from("/Applications/DocUpload.app") },
            "DocuUpload-0.6.0-macos-universal.dmg",
        );
        assert_eq!(select_asset(&InstallKind::Unknown, &assets), None);
    }

    #[test]
    fn evaluar_release_version_nueva_devuelve_asset() {
        let json = release_json(
            "v0.6.0",
            false,
            false,
            &["DocuUpload-0.6.0-windows-x86_64-setup.exe", "SHA256SUMS"],
        );
        let info = evaluar_release(&json, "0.5.0", &InstallKind::WindowsInstaller)
            .unwrap()
            .expect("debe haber actualización");
        assert_eq!(info.tag_name, "v0.6.0");
        assert_eq!(info.version, "0.6.0");
        assert_eq!(
            info.asset.map(|a| a.name),
            Some("DocuUpload-0.6.0-windows-x86_64-setup.exe".to_string())
        );
        assert_eq!(info.notes, NOTAS_POR_DEFECTO);
    }

    #[test]
    fn evaluar_release_misma_version_no_hay_actualizacion() {
        let json = release_json("v0.6.0", false, false, &[]);
        assert!(evaluar_release(&json, "0.6.0", &InstallKind::Unknown).unwrap().is_none());
    }

    #[test]
    fn evaluar_release_ignora_drafts_y_prereleases() {
        let draft = release_json("v0.7.0", true, false, &[]);
        let pre = release_json("v0.7.0", false, true, &[]);
        assert!(evaluar_release(&draft, "0.6.0", &InstallKind::Unknown).unwrap().is_none());
        assert!(evaluar_release(&pre, "0.6.0", &InstallKind::Unknown).unwrap().is_none());
    }

    #[test]
    fn evaluar_release_nombres_heredados_no_dan_asset() {
        let json = release_json("v0.6.0", false, false, &["DocUpload-Setup-0.5.0.exe"]);
        let info = evaluar_release(&json, "0.5.0", &InstallKind::WindowsInstaller)
            .unwrap()
            .expect("debe haber actualización");
        assert!(info.asset.is_none());
    }

    #[test]
    fn sin_sufijo_deleted_limpia_ruta_reemplazada() {
        assert_eq!(
            sin_sufijo_deleted(PathBuf::from("/usr/bin/docu_ffacd (deleted)")),
            PathBuf::from("/usr/bin/docu_ffacd")
        );
        assert_eq!(
            sin_sufijo_deleted(PathBuf::from("/usr/bin/docu_ffacd")),
            PathBuf::from("/usr/bin/docu_ffacd")
        );
    }

    #[test]
    fn digest_esperado_solo_acepta_sha256() {
        assert_eq!(digest_esperado(&Some("sha256:ABC".to_string())), Some("abc".to_string()));
        assert_eq!(digest_esperado(&Some("sha512:abc".to_string())), None);
        assert_eq!(digest_esperado(&None), None);
    }
}
