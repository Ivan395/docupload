use postgres::{Client, NoTls};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use std::env;

pub type DbPool = Pool<PostgresConnectionManager<NoTls>>;
pub type PooledClient = r2d2::PooledConnection<PostgresConnectionManager<NoTls>>;

pub fn get_connection_string() -> String {
    if let Ok(url) = env::var("DATABASE_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let host = env::var("PGHOST").unwrap_or_else(|_| "localhost".to_string());
    let port = env::var("PGPORT").unwrap_or_else(|_| "5432".to_string());
    let user = env::var("PGUSER").unwrap_or_else(|_| "ivan395".to_string());
    let password = env::var("PGPASSWORD").unwrap_or_else(|_| "ABCabc123".to_string());
    let dbname = env::var("PGDATABASE").unwrap_or_else(|_| "firma_fiscal_acd".to_string());

    format!("postgres://{}:{}@{}:{}/{}", user, password, host, port, dbname)
}

pub fn create_client() -> Result<Client, postgres::Error> {
    let conn_str = get_connection_string();
    Client::connect(&conn_str, NoTls)
}

pub fn create_pool(max_size: u32) -> Result<DbPool, String> {
    let conn_str = get_connection_string();
    let config: postgres::Config = conn_str
        .parse()
        .map_err(|e| format!("Error parseando configuración de PostgreSQL: {}", e))?;
    let manager = PostgresConnectionManager::new(config, NoTls);
    Pool::builder()
        .max_size(max_size)
        .min_idle(Some(1.min(max_size)))
        .build(manager)
        .map_err(|e| format!("Error creando connection pool: {}", e))
}

#[derive(Debug, Clone)]
pub struct PostgresDiagnosis {
    pub is_connected: bool,
    pub is_installed: bool,
    pub status_text: String,
    pub detail_text: String,
}

pub fn diagnose_postgres() -> PostgresDiagnosis {
    let host = env::var("PGHOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("PGPORT").unwrap_or_else(|_| "5432".to_string());
    let dbname = env::var("PGDATABASE").unwrap_or_else(|_| "firma_fiscal_acd".to_string());
    let user = env::var("PGUSER").unwrap_or_else(|_| "ivan395".to_string());

    let norm_host = if host == "localhost" { "127.0.0.1" } else { &host };
    let addr_str = format!("{}:{}", norm_host, port);
    let port_open = if let Ok(parsed_addr) = addr_str.parse() {
        std::net::TcpStream::connect_timeout(&parsed_addr, std::time::Duration::from_millis(600)).is_ok()
    } else {
        false
    };

    match create_client() {
        Ok(mut client) => {
            let ver: Result<String, _> = client.query_one("SELECT version()", &[]).map(|r| r.get(0));
            let ver_str = ver.unwrap_or_else(|_| "PostgreSQL".to_string());
            let short_ver = ver_str.split(',').next().unwrap_or("PostgreSQL").trim();
            PostgresDiagnosis {
                is_connected: true,
                is_installed: true,
                status_text: "Sincronización DB: En línea".to_string(),
                detail_text: format!("Conectado a {} ({}:{})", short_ver, host, port),
            }
        }
        Err(err) => {
            let is_installed = check_if_postgres_installed_on_system();
            let (status_text, detail_text) = if port_open {
                (
                    "Error de autenticación / BD PostgreSQL".to_string(),
                    format!(
                        "El puerto {} está abierto, pero falló la conexión con la base '{}' y usuario '{}': {}. Asegúrese de haber creado la base de datos y usuario requeridos.",
                        port, dbname, user, err
                    ),
                )
            } else if is_installed {
                (
                    "PostgreSQL detenido (Servicio apagado)".to_string(),
                    format!(
                        "PostgreSQL está instalado en este equipo, pero el servicio no se está ejecutando en el puerto {}. Por favor inicie el servicio de PostgreSQL.",
                        port
                    ),
                )
            } else {
                (
                    "PostgreSQL no instalado localmente".to_string(),
                    format!(
                        "No se detectó PostgreSQL instalado en esta computadora ni respondiendo en {}:{}. Para guardar clientes, nóminas y cédulas, instale PostgreSQL (versión 14+) o configure la variable de entorno DATABASE_URL.",
                        host, port
                    ),
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
