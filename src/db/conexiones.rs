use postgres::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use crate::db::config::{create_tls_connector, DbConfig};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConexionItem {
    pub id: String,
    pub nombre: String,
    pub host: String,
    pub port: i32,
    pub db_nombre: String,
    pub usuario: String,
    pub contrasenia: String,
    pub ssl_mode: String,
    pub connection_url: String,
    pub protocolo: String,
    pub es_activo: bool,
    pub estado_online: String, // "online", "offline", "checking", "unknown"
    pub estado_msg: String,
}

impl Default for ConexionItem {
    fn default() -> Self {
        Self {
            id: String::new(),
            nombre: String::new(),
            host: "localhost".to_string(),
            port: 5432,
            db_nombre: "firma_fiscal_acd".to_string(),
            usuario: "postgres".to_string(),
            contrasenia: String::new(),
            ssl_mode: "disable".to_string(),
            connection_url: String::new(),
            protocolo: "postgres".to_string(),
            es_activo: false,
            estado_online: "unknown".to_string(),
            estado_msg: String::new(),
        }
    }
}

impl ConexionItem {
    pub fn to_db_config(&self) -> DbConfig {
        DbConfig {
            host: if self.host.trim().is_empty() { "localhost".to_string() } else { self.host.trim().to_string() },
            port: if self.port > 0 && self.port <= 65535 { self.port as u16 } else { 5432 },
            database: if self.db_nombre.trim().is_empty() { "firma_fiscal_acd".to_string() } else { self.db_nombre.trim().to_string() },
            user: if self.usuario.trim().is_empty() { "postgres".to_string() } else { self.usuario.trim().to_string() },
            password: self.contrasenia.clone(),
            ssl_mode: if self.ssl_mode.trim().is_empty() { "disable".to_string() } else { self.ssl_mode.trim().to_string() },
            connection_url: self.connection_url.trim().to_string(),
        }
    }

    pub fn from_db_config(cfg: &DbConfig, nombre: &str, es_activo: bool) -> Self {
        Self {
            id: String::new(),
            nombre: nombre.to_string(),
            host: cfg.host.clone(),
            port: cfg.port as i32,
            db_nombre: cfg.database.clone(),
            usuario: cfg.user.clone(),
            contrasenia: cfg.password.clone(),
            ssl_mode: cfg.ssl_mode.clone(),
            connection_url: cfg.connection_url.clone(),
            protocolo: "postgres".to_string(),
            es_activo,
            estado_online: "unknown".to_string(),
            estado_msg: String::new(),
        }
    }
}

/// Verifica si una credencial/conexión está disponible en línea (timeout breve de 4 segundos).
pub fn check_conexion_status(item: &ConexionItem) -> (bool, String) {
    let cfg = item.to_db_config();
    let conn_str = cfg.to_connection_string();

    match conn_str.parse::<postgres::Config>() {
        Ok(mut pg_cfg) => {
            pg_cfg.connect_timeout(Duration::from_secs(4));
            let connector = create_tls_connector();
            match pg_cfg.connect(connector) {
                Ok(mut client) => {
                    let version_row = client.query_one("SELECT version()", &[]);
                    let short_ver = version_row
                        .ok()
                        .and_then(|r| r.get::<_, String>(0).into())
                        .map(|v| v.split(',').next().unwrap_or("PostgreSQL").trim().to_string())
                        .unwrap_or_else(|| "PostgreSQL".to_string());
                    (true, format!("En línea: {}", short_ver))
                }
                Err(e) => {
                    let err_str = e.to_string();
                    let friendly_err = if err_str.contains("timed out") || err_str.contains("timeout") {
                        "Tiempo de espera agotado (Servidor no responde)".to_string()
                    } else if err_str.contains("Connection refused") || err_str.contains("conexión rechazada") {
                        "Conexión rechazada (Servidor no está en ejecución)".to_string()
                    } else if err_str.contains("password authentication failed") || err_str.contains("autenticación") {
                        "Fallo de autenticación: usuario o contraseña incorrectos".to_string()
                    } else if err_str.contains("database") && err_str.contains("does not exist") {
                        "La base de datos especificada no existe".to_string()
                    } else {
                        err_str
                    };
                    (false, format!("No disponible: {}", friendly_err))
                }
            }
        }
        Err(e) => (false, format!("Error en configuración: {}", e)),
    }
}

/// Crea la tabla public.conexiones, revision_history.conexiones y el trigger con public.log_revision_history
pub fn ensure_conexiones_table(client: &mut Client) -> Result<(), postgres::Error> {
    client.batch_execute(
        r#"
        CREATE TABLE IF NOT EXISTS public.conexiones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            nombre varchar(50) NOT NULL UNIQUE,
            host varchar(150),
            port integer DEFAULT 5432,
            db_nombre varchar(100) NOT NULL,
            usuario varchar(100) NOT NULL,
            contrasenia varchar(100),
            ssl_mode varchar(30) DEFAULT 'prefer',
            connection_url text,
            protocolo varchar(30) DEFAULT 'postgres',
            es_activo boolean DEFAULT false,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            updated_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS revision_history.conexiones (
            id_history uuid PRIMARY KEY DEFAULT uuidv7(),
            id uuid NOT NULL,
            nombre varchar(50) NOT NULL,
            host varchar(150),
            port integer DEFAULT 5432,
            db_nombre varchar(100) NOT NULL,
            usuario varchar(100) NOT NULL,
            contrasenia varchar(100),
            ssl_mode varchar(30) DEFAULT 'prefer',
            connection_url text,
            protocolo varchar(30) DEFAULT 'postgres',
            es_activo boolean DEFAULT false,
            created_at timestamptz,
            updated_at timestamptz,
            modified_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            event_name varchar(10)
        );

        DROP TRIGGER IF EXISTS trg_conexiones_revision_history ON public.conexiones;
        CREATE TRIGGER trg_conexiones_revision_history
        AFTER INSERT OR UPDATE OR DELETE ON public.conexiones
        FOR EACH ROW EXECUTE FUNCTION public.log_revision_history();
        "#,
    )?;

    // Sembrar conexiones predeterminadas si la tabla está vacía
    let count_row = client.query_one("SELECT COUNT(*) FROM public.conexiones", &[])?;
    let count: i64 = count_row.get(0);
    if count == 0 {
        log::info!("Sembrando conexiones iniciales en public.conexiones...");
        let active_cfg = crate::db::get_active_config();
        let is_aiven = active_cfg.host.contains("aivencloud.com");

        // 1. Conexión local
        let local_cfg = crate::db::config::get_local_fallback_config();
        let not_aiven = !is_aiven;
        let _ = client.execute(
            r#"
            INSERT INTO public.conexiones (nombre, host, port, db_nombre, usuario, contrasenia, ssl_mode, protocolo, es_activo)
            VALUES ('local', $1, $2, $3, $4, $5, 'disable', 'postgres', $6)
            ON CONFLICT (nombre) DO NOTHING
            "#,
            &[
                &local_cfg.host,
                &(local_cfg.port as i32),
                &local_cfg.database,
                &local_cfg.user,
                &local_cfg.password,
                &not_aiven,
            ],
        );

        // 2. Conexión avni (Aiven Cloud)
        let aiven_pwd = if is_aiven {
            active_cfg.password.clone()
        } else {
            std::env::var("AIVEN_PASSWORD")
                .or_else(|_| std::env::var("DB_PASSWORD"))
                .unwrap_or_default()
        };
        let aiven_url = if !aiven_pwd.is_empty() {
            format!(
                "postgres://avnadmin:{}@pg-20d4e7ff-angelivan395-55bb.a.aivencloud.com:19522/firma_fiscal_acd?sslmode=require",
                aiven_pwd
            )
        } else {
            "postgres://avnadmin@pg-20d4e7ff-angelivan395-55bb.a.aivencloud.com:19522/firma_fiscal_acd?sslmode=require".to_string()
        };

        let _ = client.execute(
            r#"
            INSERT INTO public.conexiones (nombre, host, port, db_nombre, usuario, contrasenia, ssl_mode, connection_url, protocolo, es_activo)
            VALUES (
                'avni',
                'pg-20d4e7ff-angelivan395-55bb.a.aivencloud.com',
                19522,
                'firma_fiscal_acd',
                'avnadmin',
                $1,
                'require',
                $2,
                'postgres',
                $3
            )
            ON CONFLICT (nombre) DO NOTHING
            "#,
            &[&aiven_pwd, &aiven_url, &is_aiven],
        );

        // 3. Conexión house remoto (ejemplo no disponible)
        let _ = client.execute(
            r#"
            INSERT INTO public.conexiones (nombre, host, port, db_nombre, usuario, contrasenia, ssl_mode, protocolo, es_activo)
            VALUES ('house remoto', '192.168.1.200', 5432, 'firma_fiscal_acd', 'remoto_user', '', 'disable', 'postgres', false)
            ON CONFLICT (nombre) DO NOTHING
            "#,
            &[],
        );
    }

    Ok(())
}

/// Recupera todas las conexiones ordenadas por activa primero y luego alfabéticamente
pub fn fetch_conexiones(client: &mut Client) -> Result<Vec<ConexionItem>, postgres::Error> {
    let rows = client.query(
        r#"
        SELECT id::text, nombre, COALESCE(host, ''), COALESCE(port, 5432), db_nombre, usuario,
               COALESCE(contrasenia, ''), COALESCE(ssl_mode, 'prefer'), COALESCE(connection_url, ''),
               COALESCE(protocolo, 'postgres'), COALESCE(es_activo, false)
        FROM public.conexiones
        ORDER BY es_activo DESC, nombre ASC;
        "#,
        &[],
    )?;

    let mut list = Vec::new();
    for r in rows {
        list.push(ConexionItem {
            id: r.get(0),
            nombre: r.get(1),
            host: r.get(2),
            port: r.get(3),
            db_nombre: r.get(4),
            usuario: r.get(5),
            contrasenia: r.get(6),
            ssl_mode: r.get(7),
            connection_url: r.get(8),
            protocolo: r.get(9),
            es_activo: r.get(10),
            estado_online: "unknown".to_string(),
            estado_msg: String::new(),
        });
    }

    Ok(list)
}

/// Inserta una nueva conexión en public.conexiones (disparando el trigger hacia revision_history)
pub fn insert_conexion(client: &mut Client, item: &ConexionItem) -> Result<String, postgres::Error> {
    if item.es_activo {
        let _ = client.execute("UPDATE public.conexiones SET es_activo = false", &[]);
    }

    let row = client.query_one(
        r#"
        INSERT INTO public.conexiones (
            nombre, host, port, db_nombre, usuario, contrasenia, ssl_mode, connection_url, protocolo, es_activo
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id::text;
        "#,
        &[
            &item.nombre.trim(),
            &item.host.trim(),
            &item.port,
            &item.db_nombre.trim(),
            &item.usuario.trim(),
            &item.contrasenia,
            &item.ssl_mode.trim(),
            &item.connection_url.trim(),
            &item.protocolo.trim(),
            &item.es_activo,
        ],
    )?;

    let new_id: String = row.get(0);
    Ok(new_id)
}

/// Actualiza una conexión existente en public.conexiones (disparando el trigger hacia revision_history)
pub fn update_conexion(client: &mut Client, item: &ConexionItem) -> Result<(), postgres::Error> {
    let uuid_val = uuid::Uuid::parse_str(&item.id).unwrap_or_default();
    if item.es_activo {
        let _ = client.execute(
            "UPDATE public.conexiones SET es_activo = false WHERE id != $1",
            &[&uuid_val],
        );
    }

    client.execute(
        r#"
        UPDATE public.conexiones
        SET nombre = $2,
            host = $3,
            port = $4,
            db_nombre = $5,
            usuario = $6,
            contrasenia = $7,
            ssl_mode = $8,
            connection_url = $9,
            protocolo = $10,
            es_activo = $11,
            updated_at = NOW()
        WHERE id = $1;
        "#,
        &[
            &uuid_val,
            &item.nombre.trim(),
            &item.host.trim(),
            &item.port,
            &item.db_nombre.trim(),
            &item.usuario.trim(),
            &item.contrasenia,
            &item.ssl_mode.trim(),
            &item.connection_url.trim(),
            &item.protocolo.trim(),
            &item.es_activo,
        ],
    )?;

    Ok(())
}

/// Elimina una conexión de public.conexiones (disparando el trigger hacia revision_history)
pub fn delete_conexion(client: &mut Client, id: &str) -> Result<(), postgres::Error> {
    let uuid_val = uuid::Uuid::parse_str(id).unwrap_or_default();
    client.execute("DELETE FROM public.conexiones WHERE id = $1", &[&uuid_val])?;
    Ok(())
}

/// Marca una conexión como activa en la base de datos
pub fn set_active_conexion(client: &mut Client, id: &str) -> Result<(), postgres::Error> {
    let uuid_val = uuid::Uuid::parse_str(id).unwrap_or_default();
    client.execute("UPDATE public.conexiones SET es_activo = (id = $1)", &[&uuid_val])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conexiones_crud_and_trigger() {
        let mut client = match crate::db::create_client() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("PostgreSQL no disponible en test (skipping): {}", e);
                return;
            }
        };

        // 1. Asegurar esquema y tabla de conexiones
        assert!(ensure_conexiones_table(&mut client).is_ok());

        // 2. Verificar que el trigger public.log_revision_history existe
        let trg_check = client.query(
            "SELECT tgname FROM pg_trigger WHERE tgname = 'trg_conexiones_revision_history'",
            &[],
        ).unwrap();
        assert!(!trg_check.is_empty(), "El trigger trg_conexiones_revision_history debe existir");

        // 3. Probar inserción CRUD
        let test_name = format!("test_conn_{}", uuid::Uuid::now_v7());
        let item = ConexionItem {
            id: String::new(),
            nombre: test_name.clone(),
            host: "localhost".to_string(),
            port: 5432,
            db_nombre: "test_db".to_string(),
            usuario: "test_user".to_string(),
            contrasenia: "secret".to_string(),
            ssl_mode: "disable".to_string(),
            connection_url: String::new(),
            protocolo: "postgres".to_string(),
            es_activo: false,
            estado_online: "unknown".to_string(),
            estado_msg: String::new(),
        };

        let new_id = insert_conexion(&mut client, &item).expect("Fallo al insertar conexion");
        assert!(!new_id.is_empty());
        let uuid_val = uuid::Uuid::parse_str(&new_id).unwrap();

        // 4. Verificar que se registró en revision_history.conexiones con event_name = 'INSERT'
        let hist_rows = client.query(
            "SELECT event_name FROM revision_history.conexiones WHERE id = $1 AND event_name = 'INSERT'",
            &[&uuid_val],
        ).unwrap();
        assert!(!hist_rows.is_empty(), "Debe existir registro en revision_history.conexiones para el INSERT");

        // 5. Probar actualización CRUD
        let mut updated_item = item.clone();
        updated_item.id = new_id.clone();
        updated_item.port = 5433;
        assert!(update_conexion(&mut client, &updated_item).is_ok());

        // 6. Verificar que se registró en revision_history.conexiones con event_name = 'UPDATE'
        let hist_update = client.query(
            "SELECT event_name FROM revision_history.conexiones WHERE id = $1 AND event_name = 'UPDATE'",
            &[&uuid_val],
        ).unwrap();
        assert!(!hist_update.is_empty(), "Debe existir registro en revision_history.conexiones para el UPDATE");

        // 7. Probar eliminación CRUD
        assert!(delete_conexion(&mut client, &new_id).is_ok());

        // 8. Verificar que se registró en revision_history.conexiones con event_name = 'DELETE'
        let hist_del = client.query(
            "SELECT event_name FROM revision_history.conexiones WHERE id = $1 AND event_name = 'DELETE'",
            &[&uuid_val],
        ).unwrap();
        assert!(!hist_del.is_empty(), "Debe existir registro en revision_history.conexiones para el DELETE");

        // 9. Probar validación de estado en línea
        let (online_false, msg_false) = check_conexion_status(&ConexionItem {
            id: "fake".to_string(),
            nombre: "house remoto test".to_string(),
            host: "192.168.254.254".to_string(),
            port: 5432,
            db_nombre: "test".to_string(),
            usuario: "fake".to_string(),
            contrasenia: "fake".to_string(),
            ssl_mode: "disable".to_string(),
            connection_url: String::new(),
            protocolo: "postgres".to_string(),
            es_activo: false,
            estado_online: "unknown".to_string(),
            estado_msg: String::new(),
        });
        assert!(!online_false, "Servidor inalcanzable debe reportar offline");
        assert!(msg_false.contains("No disponible"));
    }
}
