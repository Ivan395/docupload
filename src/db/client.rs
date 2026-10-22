use postgres::Client;
use postgres_native_tls::MakeTlsConnector;
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
#[cfg(target_os = "windows")]
use std::env;
use std::sync::{Arc, RwLock};

pub type DbPool = Pool<PostgresConnectionManager<MakeTlsConnector>>;
pub type PooledClient = r2d2::PooledConnection<PostgresConnectionManager<MakeTlsConnector>>;

static GLOBAL_POOL: RwLock<Option<Arc<DbPool>>> = RwLock::new(None);
static IS_FALLBACK_ACTIVE: RwLock<bool> = RwLock::new(false);

pub fn is_fallback_active() -> bool {
    IS_FALLBACK_ACTIVE.read().ok().map(|b| *b).unwrap_or(false)
}

pub fn set_fallback_active(active: bool) {
    if let Ok(mut lock) = IS_FALLBACK_ACTIVE.write() {
        *lock = active;
    }
}

pub fn set_global_pool(pool: Arc<DbPool>) {
    if let Ok(mut lock) = GLOBAL_POOL.write() {
        *lock = Some(pool);
    }
}

pub fn get_global_pool() -> Option<Arc<DbPool>> {
    GLOBAL_POOL.read().ok().and_then(|guard| guard.clone())
}

pub fn reload_global_pool(max_size: u32) -> Result<Arc<DbPool>, String> {
    let pool = create_pool(max_size)?;
    let arc_pool = Arc::new(pool);
    set_global_pool(arc_pool.clone());
    Ok(arc_pool)
}

pub fn get_connection_string() -> String {
    if is_fallback_active() {
        crate::db::config::get_local_fallback_config().to_connection_string()
    } else {
        crate::db::config::get_active_config().to_connection_string()
    }
}

pub fn create_client() -> Result<Client, postgres::Error> {
    let cfg = crate::db::config::get_active_config();
    let connector = crate::db::config::create_tls_connector();
    let conn_str = cfg.to_connection_string();
    Client::connect(&conn_str, connector)
}

/// Calcula el tamaño óptimo del pool de conexiones según los núcleos de CPU disponibles
pub fn optimal_pool_size() -> u32 {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(8);
    // Doble de hilos disponibles para soportar Rayon y lecturas concurrentes sin bloqueo
    (threads * 2).clamp(16, 32)
}

pub fn create_pool_for_config(cfg: &crate::db::config::DbConfig, max_size: u32) -> Result<DbPool, String> {
    let conn_str = cfg.to_connection_string();
    let config: postgres::Config = conn_str
        .parse()
        .map_err(|e| format!("Error parseando configuración PostgreSQL ({}): {}", cfg.host, e))?;
    let connector = crate::db::config::create_tls_connector();
    let manager = PostgresConnectionManager::new(config, connector);
    let target_size = if max_size == 0 {
        optimal_pool_size()
    } else {
        max_size.max(optimal_pool_size())
    };
    Pool::builder()
        .max_size(target_size)
        .min_idle(Some(2.min(target_size)))
        .connection_timeout(std::time::Duration::from_secs(30))
        .build(manager)
        .map_err(|e| format!("Error creando connection pool ({}): {}", cfg.host, e))
}

pub fn create_pool(max_size: u32) -> Result<DbPool, String> {
    let cfg = crate::db::config::get_active_config();
    let _ = cfg.ensure_database_exists();
    let pool = create_pool_for_config(&cfg, max_size)?;
    if let Ok(mut client) = pool.get() {
        if client.query_one("SELECT 1", &[]).is_ok() {
            log::info!(
                "Conectado exitosamente a la base de datos local: {} ({}:{})",
                cfg.database,
                cfg.host,
                cfg.port
            );
            return Ok(pool);
        }
    }

    Err(format!(
        "No se pudo conectar a la base de datos local '{}' en {}:{}",
        cfg.database, cfg.host, cfg.port
    ))
}

#[derive(Debug, Clone)]
pub struct PostgresDiagnosis {
    pub is_connected: bool,
    pub is_installed: bool,
    pub status_text: String,
    pub detail_text: String,
}

pub fn diagnose_postgres() -> PostgresDiagnosis {
    let cfg = crate::db::config::get_active_config();
    let host = cfg.host.clone();
    let port = cfg.port.to_string();

    let connector = crate::db::config::create_tls_connector();
    let conn_str = cfg.to_connection_string();

    match Client::connect(&conn_str, connector) {
        Ok(mut client) => {
            let ver: Result<String, _> = client.query_one("SELECT version()", &[]).map(|r| r.get(0));
            let ver_str = ver.unwrap_or_else(|_| "PostgreSQL".to_string());
            let short_ver = ver_str.split(',').next().unwrap_or("PostgreSQL").trim();

            PostgresDiagnosis {
                is_connected: true,
                is_installed: true,
                status_text: "Base de Datos: Modo Local".to_string(),
                detail_text: format!("Conectado a {} local ({}:{})", short_ver, host, port),
            }
        }
        Err(err) => {
            let is_installed = check_if_postgres_installed_on_system();
            PostgresDiagnosis {
                is_connected: false,
                is_installed,
                status_text: "Base de datos local no disponible".to_string(),
                detail_text: format!("No se pudo conectar a PostgreSQL local en {}:{}: {}.", host, port, err),
            }
        }
    }
}

fn check_if_postgres_installed_on_system() -> bool {
    // 1. Probar si el puerto local 5432 responde (cuando corre en Docker o servicio)
    if std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], 5432)),
        std::time::Duration::from_millis(300),
    ).is_ok() {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        use std::path::Path;
        if Path::new(r"C:\Program Files\PostgreSQL").exists()
            || Path::new(r"C:\Program Files (x86)\PostgreSQL").exists()
            || Path::new(r"C:\PostgreSQL").exists()
        {
            return true;
        }
        if let Ok(path) = env::var("PATH") {
            for p in path.split(';') {
                if Path::new(p).join("psql.exe").exists() || Path::new(p).join("pg_ctl.exe").exists() {
                    return true;
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::path::Path;
        if Path::new("/usr/lib/postgresql").exists()
            || Path::new("/var/lib/pgsql").exists()
            || Path::new("/etc/postgresql").exists()
            || Path::new("/usr/bin/psql").exists()
            || Path::new("/usr/bin/postgres").exists()
        {
            return true;
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::path::Path;
        if Path::new("/Library/PostgreSQL").exists()
            || Path::new("/opt/homebrew/opt/postgresql@16").exists()
            || Path::new("/opt/homebrew/opt/postgresql@15").exists()
            || Path::new("/opt/homebrew/opt/postgresql@14").exists()
            || Path::new("/usr/local/var/postgres").exists()
        {
            return true;
        }
    }

    false
}
