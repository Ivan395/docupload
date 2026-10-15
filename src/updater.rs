use serde::Deserialize;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub tag_name: String,
    pub version: String,
    pub html_url: String,
    pub notes: String,
    pub asset_name: String,
    pub download_url: String,
    pub filesize_bytes: u64,
}

#[derive(Deserialize, Debug)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize, Debug)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Consulta la API pública de GitHub para ver si hay una versión más reciente
pub fn check_for_updates() -> Result<Option<UpdateInfo>, String> {
    let current_version_str = env!("CARGO_PKG_VERSION");
    let current_ver = semver::Version::parse(current_version_str)
        .map_err(|e| format!("Error analizando versión actual: {}", e))?;

    let url = "https://api.github.com/repos/Ivan395/docupload/releases/latest";

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Error creando cliente HTTP: {}", e))?;

    let response = client
        .get(url)
        .header("User-Agent", format!("DocuUpload-App/{}", current_version_str))
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .map_err(|e| format!("Error conectando con el servidor de actualizaciones: {}", e))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // No hay releases públicos aún
        return Ok(None);
    }

    if !response.status().is_success() {
        return Err(format!("El servidor respondió con código {}", response.status()));
    }

    let text = response
        .text()
        .map_err(|e| format!("Error leyendo respuesta del servidor: {}", e))?;

    let release: GitHubRelease = serde_json::from_str(&text)
        .map_err(|e| format!("Error interpretando respuesta de actualización: {}", e))?;

    let clean_remote_tag = release.tag_name.trim_start_matches('v').trim();
    let remote_ver = match semver::Version::parse(clean_remote_tag) {
        Ok(v) => v,
        Err(_) => {
            // Intento de fallback si el tag tiene formato como 0.5.0-1
            let fallback_tag = clean_remote_tag.split('-').next().unwrap_or(clean_remote_tag);
            semver::Version::parse(fallback_tag)
                .map_err(|e| format!("Error analizando versión remota '{}': {}", release.tag_name, e))?
        }
    };

    if remote_ver > current_ver {
        // Encontrar el asset adecuado según la plataforma actual
        let (selected_asset_name, selected_download_url, selected_size) = select_best_asset(&release.assets);

        Ok(Some(UpdateInfo {
            tag_name: release.tag_name,
            version: remote_ver.to_string(),
            html_url: release.html_url,
            notes: release.body.unwrap_or_else(|| "Actualización con mejoras de rendimiento y correcciones de estabilidad.".to_string()),
            asset_name: selected_asset_name,
            download_url: selected_download_url,
            filesize_bytes: selected_size,
        }))
    } else {
        Ok(None)
    }
}

fn select_best_asset(assets: &[GitHubAsset]) -> (String, String, u64) {
    if assets.is_empty() {
        return (
            "Actualizador".to_string(),
            "https://github.com/Ivan395/docupload/releases/latest".to_string(),
            0,
        );
    }

    #[cfg(target_os = "windows")]
    {
        // 1. Priorizar el instalador Setup .exe
        if let Some(a) = assets.iter().find(|a| a.name.ends_with(".exe")) {
            return (a.name.clone(), a.browser_download_url.clone(), a.size);
        }
        // 2. Si no hay exe, buscar zip
        if let Some(a) = assets.iter().find(|a| a.name.ends_with(".zip")) {
            return (a.name.clone(), a.browser_download_url.clone(), a.size);
        }
    }

    #[cfg(target_os = "macos")]
    {
        // 1. Priorizar archivo .dmg
        if let Some(a) = assets.iter().find(|a| a.name.ends_with(".dmg")) {
            return (a.name.clone(), a.browser_download_url.clone(), a.size);
        }
        // 2. Si no hay dmg, buscar tar.gz
        if let Some(a) = assets.iter().find(|a| a.name.ends_with(".tar.gz")) {
            return (a.name.clone(), a.browser_download_url.clone(), a.size);
        }
    }

    #[cfg(target_os = "linux")]
    {
        // 1. Priorizar AppImage universal autónomo
        if let Some(a) = assets.iter().find(|a| a.name.ends_with(".AppImage")) {
            return (a.name.clone(), a.browser_download_url.clone(), a.size);
        }
        // 2. Si no, deb o rpm
        if let Some(a) = assets.iter().find(|a| a.name.ends_with(".deb") || a.name.ends_with(".rpm") || a.name.ends_with(".pkg.tar.zst")) {
            return (a.name.clone(), a.browser_download_url.clone(), a.size);
        }
    }

    // Fallback genérico: primer asset disponible
    let first = &assets[0];
    (first.name.clone(), first.browser_download_url.clone(), first.size)
}

/// Descarga el archivo de actualización reportando progreso
pub fn download_update_file<F>(
    download_url: &str,
    asset_name: &str,
    progress_callback: F,
) -> Result<PathBuf, String>
where
    F: Fn(f32, &str) + Send + 'static,
{
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("Error creando cliente de descarga: {}", e))?;

    let mut response = client
        .get(download_url)
        .header("User-Agent", "DocuUpload-Updater")
        .send()
        .map_err(|e| format!("Error iniciando descarga: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("El servidor rechazó la descarga con código {}", response.status()));
    }

    let total_size = response
        .content_length()
        .unwrap_or(0);

    let temp_dir = std::env::temp_dir();
    let dest_path = temp_dir.join(asset_name);

    let mut file = File::create(&dest_path)
        .map_err(|e| format!("Error creando archivo destino temporal: {}", e))?;

    let mut downloaded: u64 = 0;
    let mut buffer = [0u8; 65536]; // 64 KB buffer

    loop {
        let bytes_read = response
            .read(&mut buffer)
            .map_err(|e| format!("Error leyendo datos de la descarga: {}", e))?;

        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read])
            .map_err(|e| format!("Error escribiendo en archivo temporal: {}", e))?;

        downloaded += bytes_read as u64;

        if total_size > 0 {
            let frac = (downloaded as f32 / total_size as f32).clamp(0.0, 1.0);
            let downloaded_mb = downloaded as f64 / 1_048_576.0;
            let total_mb = total_size as f64 / 1_048_576.0;
            let status = format!("{:.1} MB de {:.1} MB ({:.0}%)", downloaded_mb, total_mb, frac * 100.0);
            progress_callback(frac, &status);
        } else {
            let downloaded_mb = downloaded as f64 / 1_048_576.0;
            let status = format!("{:.1} MB descargados", downloaded_mb);
            progress_callback(0.5, &status);
        }
    }

    progress_callback(1.0, "Descarga completada");
    Ok(dest_path)
}

/// Aplica la actualización instalándola o ejecutándola según la plataforma
pub fn apply_update_now(file_path: &Path) -> Result<(), String> {
    if !file_path.exists() {
        return Err("El archivo de actualización no existe en el sistema".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        // En Windows, ejecutar el instalador .exe
        let path_str = file_path.to_string_lossy().to_string();
        if path_str.ends_with(".exe") {
            std::process::Command::new(&path_str)
                .spawn()
                .map_err(|e| format!("Error ejecutando instalador de Windows: {}", e))?;
            std::process::exit(0);
        } else {
            // Abrir la carpeta contenedora si no es un exe directamente ejecutable
            let _ = std::process::Command::new("explorer")
                .arg(format!("/select,{}", path_str))
                .spawn();
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(file_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(file_path, perms);
        }

        // Si se ejecuta desde un AppImage, reemplazar o relanzar
        if let Ok(current_appimage) = std::env::var("APPIMAGE") {
            let current_path = PathBuf::from(&current_appimage);
            let backup_path = current_path.with_extension("appimage.old");
            
            // Renombrar el actual a .old y mover el nuevo
            let _ = std::fs::rename(&current_path, &backup_path);
            if std::fs::copy(file_path, &current_path).is_ok() {
                let _ = std::fs::remove_file(&backup_path);
                let _ = std::process::Command::new(&current_path).spawn();
                std::process::exit(0);
            }
        }

        // Si no es AppImage o falló el swap, lanzar el binario descargado
        std::process::Command::new(file_path)
            .spawn()
            .map_err(|e| format!("Error ejecutando nueva versión: {}", e))?;
        std::process::exit(0);
    }

    #[cfg(target_os = "macos")]
    {
        // En macOS, abrir el archivo .dmg con la utilidad del sistema
        std::process::Command::new("open")
            .arg(file_path)
            .spawn()
            .map_err(|e| format!("Error abriendo paquete .dmg en macOS: {}", e))?;
        Ok(())
    }
}
