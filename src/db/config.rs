use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub const DEFAULT_CONFIG_FILENAME: &str = "db_config.json";

pub const DEFAULT_AIVEN_URI: &str = "postgres://avnadmin@pg-20d4e7ff-angelivan395-55bb.a.aivencloud.com:19522/firma_fiscal_acd?sslmode=require";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DbConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
    #[serde(default = "default_ssl_mode")]
    pub ssl_mode: String,
    #[serde(default)]
    pub connection_url: String,
}

fn default_ssl_mode() -> String {
    "disable".to_string()
}

impl Default for DbConfig {
    fn default() -> Self {
        let host = env::var("PGHOST").unwrap_or_else(|_| "localhost".to_string());
        let port = env::var("PGPORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(5432);
        let database = env::var("PGDATABASE").unwrap_or_else(|_| "firma_fiscal_acd".to_string());
        let user = env::var("PGUSER").unwrap_or_else(|_| "ivan395".to_string());
        let password = env::var("PGPASSWORD").unwrap_or_else(|_| "ABCabc123".to_string());

        Self {
            host,
            port,
            database,
            user,
            password,
            ssl_mode: "disable".to_string(),
            connection_url: String::new(),
        }
    }
}

/// Crea un conector TLS compatible con PostgreSQL, aceptando certificados válidos o autofirmados de nube.
pub fn create_tls_connector() -> MakeTlsConnector {
    let mut builder = TlsConnector::builder();
    builder.danger_accept_invalid_certs(true);
    let tls = builder.build().unwrap_or_else(|_| TlsConnector::new().unwrap());
    MakeTlsConnector::new(tls)
}

/// Genera la configuración predeterminada para la base de datos local de contingencia.
pub fn get_local_fallback_config() -> DbConfig {
    let host = env::var("PGHOST").unwrap_or_else(|_| "localhost".to_string());
    let port = env::var("PGPORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(5432);
    let database = "firma_fiscal_acd".to_string();
    let user = env::var("PGUSER").unwrap_or_else(|_| "ivan395".to_string());
    let password = env::var("PGPASSWORD").unwrap_or_else(|_| "ABCabc123".to_string());

    DbConfig {
        host,
        port,
        database,
        user,
        password,
        ssl_mode: "disable".to_string(),
        connection_url: String::new(),
    }
}

impl DbConfig {
    /// Devuelve la cadena de conexión completa para PostgreSQL.
    pub fn to_connection_string(&self) -> String {
        let trimmed_url = self.connection_url.trim();
        if !trimmed_url.is_empty() {
            return trimmed_url.to_string();
        }

        let ssl_param = match self.ssl_mode.to_lowercase().as_str() {
            "require" => "?sslmode=require",
            "prefer" => "?sslmode=prefer",
            _ => "?sslmode=disable",
        };

        format!(
            "postgres://{}:{}@{}:{}/{}{}",
            self.user.trim(),
            self.password,
            self.host.trim(),
            self.port,
            self.database.trim(),
            ssl_param
        )
    }

    /// Genera una cadena de conexión para conectarse a una base de datos administrativa/mantenimiento.
    pub fn maintenance_connection_string(&self, maint_db: &str) -> String {
        let ssl_param = match self.ssl_mode.to_lowercase().as_str() {
            "require" => "?sslmode=require",
            "prefer" => "?sslmode=prefer",
            _ => "?sslmode=disable",
        };

        format!(
            "postgres://{}:{}@{}:{}/{}{}",
            self.user.trim(),
            self.password,
            self.host.trim(),
            self.port,
            maint_db.trim(),
            ssl_param
        )
    }

    /// Resuelve la ruta del archivo de configuración `db_config.json`.
    pub fn resolve_config_path() -> PathBuf {
        if let Ok(custom_path) = env::var("DB_CONFIG_PATH") {
            let p = PathBuf::from(custom_path.trim());
            if !p.as_os_str().is_empty() {
                return p;
            }
        }

        let current_dir_file = PathBuf::from(DEFAULT_CONFIG_FILENAME);
        if current_dir_file.exists() {
            return current_dir_file;
        }

        if let Ok(exe_path) = env::current_exe() {
            if let Some(parent) = exe_path.parent() {
                let exe_sibling = parent.join(DEFAULT_CONFIG_FILENAME);
                if exe_sibling.exists() {
                    return exe_sibling;
                }
            }
        }

        current_dir_file
    }

    /// Carga la configuración desde el disco o crea la inicial.
    pub fn load_or_create() -> (Self, PathBuf) {
        let path = Self::resolve_config_path();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<DbConfig>(&content) {
                    Ok(cfg) => {
                        log::info!("Configuración PostgreSQL cargada desde '{}'", path.display());
                        return (cfg, path);
                    }
                    Err(e) => {
                        log::warn!(
                            "Error deserializando '{}': {}. Usando valores por defecto.",
                            path.display(),
                            e
                        );
                    }
                },
                Err(e) => {
                    log::warn!(
                        "No se pudo leer '{}': {}. Usando valores por defecto.",
                        path.display(),
                        e
                    );
                }
            }
        }

        let default_cfg = DbConfig::default();
        if let Err(e) = default_cfg.save_to(&path) {
            log::warn!(
                "No se pudo guardar la configuración inicial en '{}': {}",
                path.display(),
                e
            );
        } else {
            log::info!(
                "Archivo de configuración inicial de PostgreSQL creado en '{}'",
                path.display()
            );
        }

        (default_cfg, path)
    }

    /// Guarda la configuración actual en el disco en formato JSON formateado.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Error serializando configuración JSON: {}", e))?;

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                let _ = fs::create_dir_all(parent);
            }
        }

        fs::write(path, json)
            .map_err(|e| format!("Error escribiendo archivo '{}': {}", path.display(), e))?;

        Ok(())
    }

    /// Verifica si la base de datos requerida existe en el servidor; si no existe, intenta crearla.
    pub fn ensure_database_exists(&self) -> Result<(), String> {
        let target_db = self.database.trim();
        if target_db.is_empty() {
            return Ok(());
        }

        // 1. Probar si la base de datos destino ya existe y se conecta directamente
        let target_conn = self.to_connection_string();
        let tls1 = create_tls_connector();
        if let Ok(mut client) = postgres::Client::connect(&target_conn, tls1) {
            let _ = client.query_one("SELECT 1", &[]);
            return Ok(());
        }

        // 2. Si falló la conexión directa al target_db, verificar si existe conectándose a la base administrativa
        let maint_dbs = if self.host.contains("aivencloud.com") {
            vec!["defaultdb", "postgres"]
        } else {
            vec!["postgres", "defaultdb", "template1"]
        };

        for maint_db in maint_dbs {
            let maint_url = self.maintenance_connection_string(maint_db);
            let tls = create_tls_connector();
            if let Ok(mut client) = postgres::Client::connect(&maint_url, tls) {
                let check_sql = "SELECT 1 FROM pg_database WHERE datname = $1";
                let exists = client.query(check_sql, &[&target_db]).map(|rows| !rows.is_empty()).unwrap_or(false);
                if !exists {
                    log::info!("Creando base de datos '{}' a través de '{}'...", target_db, maint_db);
                    let create_sql = format!("CREATE DATABASE \"{}\"", target_db);
                    if let Err(e) = client.execute(&create_sql, &[]) {
                        log::warn!("No se pudo ejecutar CREATE DATABASE en '{}': {}", maint_db, e);
                    } else {
                        log::info!("Base de datos '{}' creada exitosamente.", target_db);
                        return Ok(());
                    }
                } else {
                    return Ok(());
                }
            }
        }

        Ok(())
    }

    /// Prueba la conexión contra el servidor PostgreSQL con estos parámetros sin alterar el pool.
    pub fn test_connection(&self) -> Result<String, String> {
        let conn_str = self.to_connection_string();
        let connector = create_tls_connector();

        match postgres::Client::connect(&conn_str, connector) {
            Ok(mut client) => {
                let version_row = client.query_one("SELECT version()", &[]);
                let version_str = version_row
                    .ok()
                    .and_then(|r| r.get::<_, String>(0).into())
                    .unwrap_or_else(|| "PostgreSQL".to_string());
                let short_ver = version_str.split(',').next().unwrap_or("PostgreSQL").trim();
                Ok(format!(
                    "¡Conexión exitosa! Servidor: {} ({}:{})",
                    short_ver, self.host, self.port
                ))
            }
            Err(e) => Err(format!("Fallo al conectar/autenticar: {}", e)),
        }
    }
}

// Singleton global en memoria para acceder a la configuración activa
static ACTIVE_CONFIG: RwLock<Option<(DbConfig, PathBuf)>> = RwLock::new(None);

pub fn get_active_config() -> DbConfig {
    if let Ok(guard) = ACTIVE_CONFIG.read() {
        if let Some((ref cfg, _)) = *guard {
            return cfg.clone();
        }
    }

    let (cfg, path) = DbConfig::load_or_create();
    if let Ok(mut guard) = ACTIVE_CONFIG.write() {
        *guard = Some((cfg.clone(), path));
    }
    cfg
}

pub fn get_active_config_path() -> PathBuf {
    if let Ok(guard) = ACTIVE_CONFIG.read() {
        if let Some((_, ref path)) = *guard {
            return path.clone();
        }
    }

    let (cfg, path) = DbConfig::load_or_create();
    let res = path.clone();
    if let Ok(mut guard) = ACTIVE_CONFIG.write() {
        *guard = Some((cfg, path));
    }
    res
}

pub fn update_active_config(new_cfg: DbConfig) -> Result<PathBuf, String> {
    let path = get_active_config_path();
    new_cfg.save_to(&path)?;

    if let Ok(mut guard) = ACTIVE_CONFIG.write() {
        *guard = Some((new_cfg, path.clone()));
    }

    Ok(path)
}
