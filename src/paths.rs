//! Rutas por usuario para configuración, datos y actualizaciones.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Variable de entorno no vacía, si existe.
fn env_no_vacia(nombre: &str) -> Option<PathBuf> {
    env::var_os(nombre)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Directorio de configuración del usuario actual.
#[cfg(target_os = "windows")]
pub fn config_dir() -> PathBuf {
    env_no_vacia("APPDATA")
        .map(|p| p.join("DocUpload"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Directorio de configuración del usuario actual.
#[cfg(target_os = "macos")]
pub fn config_dir() -> PathBuf {
    env_no_vacia("HOME")
        .map(|h| h.join("Library").join("Application Support").join("DocUpload"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Directorio de configuración del usuario actual.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn config_dir() -> PathBuf {
    if let Some(xdg) = env_no_vacia("XDG_CONFIG_HOME") {
        return xdg.join("docu_ffacd");
    }
    env_no_vacia("HOME")
        .map(|h| h.join(".config").join("docu_ffacd"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Directorio de datos del usuario actual (no se sincroniza con la configuración).
#[cfg(target_os = "windows")]
pub fn data_dir() -> PathBuf {
    env_no_vacia("LOCALAPPDATA")
        .map(|p| p.join("DocUpload"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Directorio de datos del usuario actual (no se sincroniza con la configuración).
#[cfg(target_os = "macos")]
pub fn data_dir() -> PathBuf {
    config_dir()
}

/// Directorio de datos del usuario actual (no se sincroniza con la configuración).
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn data_dir() -> PathBuf {
    if let Some(xdg) = env_no_vacia("XDG_DATA_HOME") {
        return xdg.join("docu_ffacd");
    }
    env_no_vacia("HOME")
        .map(|h| h.join(".local").join("share").join("docu_ffacd"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Directorio donde se descargan las actualizaciones.
pub fn updates_dir() -> PathBuf {
    data_dir().join("updates")
}

/// Crea el directorio (y sus padres) si no existe.
pub fn ensure_dir(p: &Path) -> std::io::Result<()> {
    fs::create_dir_all(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn rutas_linux_terminan_en_docu_ffacd() {
        let config = config_dir();
        let data = data_dir();
        assert!(config.ends_with("docu_ffacd") || config == Path::new("."));
        assert!(data.ends_with("docu_ffacd") || data == Path::new("."));
    }

    #[test]
    fn updates_dir_es_subcarpeta_de_datos() {
        assert!(updates_dir().ends_with("updates"));
        assert_eq!(updates_dir(), data_dir().join("updates"));
    }
}
