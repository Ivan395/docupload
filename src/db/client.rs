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
    let primary_cfg = crate::db::config::get_active_config();
    let connector = crate::db::config::create_tls_connector();
    let conn_str = primary_cfg.to_connection_string();

    match Client::connect(&conn_str, connector) {
        Ok(c) => {
            set_fallback_active(false);
            Ok(c)
        }
        Err(err) => {
            let is_primary_local = primary_cfg.host == "localhost" || primary_cfg.host == "127.0.0.1";
            if !is_primary_local {
                log::warn!("Conexión remota fallida en create_client ({}). Probando fallback local...", err);
                let local_cfg = crate::db::config::get_local_fallback_config();
                let local_connector = crate::db::config::create_tls_connector();
                let local_conn_str = local_cfg.to_connection_string();
                if let Ok(local_client) = Client::connect(&local_conn_str, local_connector) {
                    set_fallback_active(true);
                    return Ok(local_client);
                }
            }
            Err(err)
        }
    }
}

pub fn create_pool_for_config(cfg: &crate::db::config::DbConfig, max_size: u32) -> Result<DbPool, String> {
    let conn_str = cfg.to_connection_string();
    let config: postgres::Config = conn_str
        .parse()
        .map_err(|e| format!("Error parseando configuración PostgreSQL ({}): {}", cfg.host, e))?;
    let connector = crate::db::config::create_tls_connector();
    let manager = PostgresConnectionManager::new(config, connector);
    Pool::builder()
        .max_size(max_size)
        .min_idle(Some(1.min(max_size)))
        .connection_timeout(std::time::Duration::from_secs(7))
        .build(manager)
        .map_err(|e| format!("Error creando connection pool ({}): {}", cfg.host, e))
}

pub fn create_pool(max_size: u32) -> Result<DbPool, String> {
    let primary_cfg = crate::db::config::get_active_config();

    // Asegurar que la base de datos existe en el destino primario
    let _ = primary_cfg.ensure_database_exists();

    // 1. Intentar conectar a la base de datos primaria (ej. Aiven Cloud)
    match create_pool_for_config(&primary_cfg, max_size) {
        Ok(pool) => {
            if let Ok(mut client) = pool.get() {
                if client.query_one("SELECT 1", &[]).is_ok() {
                    set_fallback_active(false);
                    log::info!(
                        "Conectado exitosamente a la base de datos primaria: {} ({}:{})",
                        primary_cfg.database,
                        primary_cfg.host,
                        primary_cfg.port
                    );
                    return Ok(pool);
                }
            }
            log::warn!("Connection pool remoto creado pero falló el handshake de prueba.");
        }
        Err(e) => {
            log::warn!("No se pudo conectar a la base de datos primaria ({}): {}", primary_cfg.host, e);
        }
    }

    // 2. Si la base primaria no es local, aplicar fallback automático a PostgreSQL local
    let is_primary_local = primary_cfg.host == "localhost" || primary_cfg.host == "127.0.0.1";
    if !is_primary_local {
        log::warn!("Activando fallback automático: Conectando a PostgreSQL local (localhost:5432)...");
        let local_cfg = crate::db::config::get_local_fallback_config();
        let _ = local_cfg.ensure_database_exists();

        match create_pool_for_config(&local_cfg, max_size) {
            Ok(local_pool) => {
                if let Ok(mut client) = local_pool.get() {
                    if client.query_one("SELECT 1", &[]).is_ok() {
                        set_fallback_active(true);
                        log::info!("Fallback exitoso: Operando con PostgreSQL local (localhost:5432)");
                        return Ok(local_pool);
                    }
                }
            }
            Err(e) => {
                log::error!("Fallo también en la conexión local de contingencia: {}", e);
            }
        }
    }

    Err(format!(
        "No se pudo establecer conexión con PostgreSQL en '{}' ({}:{}) ni con la base de datos local de contingencia.",
        primary_cfg.database, primary_cfg.host, primary_cfg.port
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
    let fallback = is_fallback_active();
    let cfg = if fallback {
        crate::db::config::get_local_fallback_config()
    } else {
        crate::db::config::get_active_config()
    };
    let host = cfg.host.clone();
    let port = cfg.port.to_string();

    let connector = crate::db::config::create_tls_connector();
    let conn_str = cfg.to_connection_string();

    match Client::connect(&conn_str, connector) {
        Ok(mut client) => {
            let ver: Result<String, _> = client.query_one("SELECT version()", &[]).map(|r| r.get(0));
            let ver_str = ver.unwrap_or_else(|_| "PostgreSQL".to_string());
            let short_ver = ver_str.split(',').next().unwrap_or("PostgreSQL").trim();

            if fallback {
                PostgresDiagnosis {
                    is_connected: true,
                    is_installed: true,
                    status_text: "Sincronización DB: Modo Local (Contingencia)".to_string(),
                    detail_text: format!("Conectado a {} local ({}:{}). La conexión remota no estuvo disponible.", short_ver, host, port),
                }
            } else {
                let is_aiven = host.contains("aivencloud.com");
                let label = if is_aiven { "En línea (Aiven Cloud)" } else { "En línea" };
                PostgresDiagnosis {
                    is_connected: true,
                    is_installed: true,
                    status_text: format!("Sincronización DB: {}", label),
                    detail_text: format!("Conectado a {} ({}:{})", short_ver, host, port),
                }
            }
        }
        Err(err) => {
            let is_installed = check_if_postgres_installed_on_system();
            let is_local = host == "localhost" || host == "127.0.0.1" || host == "::1";
            let (status_text, detail_text) = if is_local {
                (
                    "PostgreSQL local no alcanzable".to_string(),
                    format!("No se pudo conectar a PostgreSQL local en {}:{}: {}.", host, port, err),
                )
            } else {
                (
                    "Servidor PostgreSQL remoto no disponible".to_string(),
                    format!("No se pudo conectar a PostgreSQL en {}:{}: {}.", host, port, err),
                )
            };

            PostgresDiagnosis {
                is_connected: false,
                is_installed,
                status_text,
                detail_text,
            }
        }
    }
}

fn check_if_postgres_installed_on_system() -> bool {
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
