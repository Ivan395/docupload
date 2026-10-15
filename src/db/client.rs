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
