//! Analistas (D-6 a): selector de analista de la sesión, autoría de cambios y registro de
//! versiones de reportes para el bloqueo optimista.
//!
//! - El analista actual es un estado global de proceso; se recuerda el último nombre elegido
//!   en un archivo local junto a `db_config.json`.
//! - El registro de versiones guarda, por reporte, la `version` que vio ESTA instancia al cargar
//!   el reporte. Al editar, `bitacora_reportes::iniciar_edicion` compara esas versiones con las
//!   actuales en BD y rechaza la edición si otro analista ya modificó el reporte.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use postgres::GenericClient;
use uuid::Uuid;

use crate::db::config::DbConfig;
use crate::db::schema::format_pg_error;

/// Nombre del archivo local que recuerda el último analista elegido.
pub const ARCHIVO_PREFERENCIA: &str = "analista_actual.txt";

#[derive(Debug, Clone, PartialEq)]
pub struct Analista {
    pub id: Uuid,
    pub nombre: String,
}

static ACTUAL: RwLock<Option<Analista>> = RwLock::new(None);

/// Versiones de `reportes_liquidaciones` vistas por esta instancia (id_reporte → version).
static VERSIONES: Mutex<Option<HashMap<Uuid, i32>>> = Mutex::new(None);

/// Analista seleccionado en la sesión, si hay uno.
pub fn analista_actual() -> Option<Analista> {
    ACTUAL.read().ok().and_then(|g| g.clone())
}

/// Fija (o limpia con `None`) el analista de la sesión.
pub fn fijar_analista_actual(a: Option<Analista>) {
    if let Ok(mut g) = ACTUAL.write() {
        *g = a;
    }
}

pub fn id_analista_actual() -> Option<Uuid> {
    analista_actual().map(|a| a.id)
}

/// Nombre del analista de la sesión; cadena vacía si no hay.
pub fn nombre_analista_actual() -> String {
    analista_actual().map(|a| a.nombre).unwrap_or_default()
}

/// Analistas activos, ordenados por nombre.
pub fn listar_analistas<C: GenericClient>(client: &mut C) -> Result<Vec<Analista>, String> {
    let filas = client
        .query(
            "SELECT id, nombre FROM public.analistas WHERE activo ORDER BY nombre",
            &[],
        )
        .map_err(|e| format!("listar_analistas: {}", format_pg_error(&e)))?;
    Ok(filas
        .iter()
        .map(|r| Analista {
            id: r.get(0),
            nombre: r.get(1),
        })
        .collect())
}

/// Crea el analista o reactiva el existente con ese nombre (sin distinguir sobrante de espacios).
/// Genérica sobre `GenericClient` para poder usarla dentro de una transacción.
pub fn crear_o_obtener_analista<C: GenericClient>(client: &mut C, nombre: &str) -> Result<Analista, String> {
    let nombre = nombre.trim();
    if nombre.is_empty() {
        return Err("Captura el nombre del analista.".to_string());
    }
    let fila = client
        .query_one(
            "INSERT INTO public.analistas (nombre) VALUES ($1) \
             ON CONFLICT (nombre) DO UPDATE SET activo = true \
             RETURNING id, nombre",
            &[&nombre],
        )
        .map_err(|e| format!("crear_o_obtener_analista: {}", format_pg_error(&e)))?;
    Ok(Analista {
        id: fila.get(0),
        nombre: fila.get(1),
    })
}

/// Ruta del archivo de preferencia: mismo directorio que `db_config.json`.
pub fn ruta_preferencia() -> PathBuf {
    let config = DbConfig::resolve_config_path();
    config
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default()
        .join(ARCHIVO_PREFERENCIA)
}

/// Nombre recordado del último analista. Cualquier error se ignora (devuelve `None`).
pub fn leer_preferencia() -> Option<String> {
    let texto = fs::read_to_string(ruta_preferencia()).ok()?;
    let nombre = texto.trim();
    if nombre.is_empty() {
        None
    } else {
        Some(nombre.to_string())
    }
}

/// Guarda el nombre del analista para la próxima sesión.
pub fn guardar_preferencia(nombre: &str) -> Result<(), String> {
    let ruta = ruta_preferencia();
    if let Some(dir) = ruta.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)
                .map_err(|e| format!("No se pudo crear el directorio de preferencias: {}", e))?;
        }
    }
    fs::write(&ruta, nombre.trim())
        .map_err(|e| format!("No se pudo guardar la preferencia de analista: {}", e))
}

/// Registra las versiones que vio esta instancia (sobrescribe las previas de esos reportes).
pub fn registrar_versiones(pares: &[(Uuid, i32)]) {
    if let Ok(mut g) = VERSIONES.lock() {
        let mapa = g.get_or_insert_with(HashMap::new);
        for (id, version) in pares {
            mapa.insert(*id, *version);
        }
    }
}

/// Versión vista por esta instancia para un reporte, si la registró.
pub fn version_esperada(id: Uuid) -> Option<i32> {
    VERSIONES
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|m| m.get(&id).copied()))
}

/// Limpia el registro de versiones.
pub fn olvidar_versiones() {
    if let Ok(mut g) = VERSIONES.lock() {
        *g = None;
    }
}

/// Pura: ids cuya versión actual difiere de la esperada. Ids sin versión actual no son conflicto.
pub fn conflictos(esperadas: &HashMap<Uuid, i32>, actuales: &[(Uuid, i32)]) -> Vec<Uuid> {
    actuales
        .iter()
        .filter(|(id, version)| esperadas.get(id).is_some_and(|e| e != version))
        .map(|(id, _)| *id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as TestMutex;

    /// Serializa las pruebas que tocan el estado global del analista.
    static LOCK_GLOBAL: TestMutex<()> = TestMutex::new(());

    #[test]
    fn conflictos_detecta_versiones_distintas() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        let mut esperadas = HashMap::new();
        esperadas.insert(a, 0);
        esperadas.insert(b, 4);
        let actuales = vec![(a, 1), (b, 4), (c, 9)];
        assert_eq!(conflictos(&esperadas, &actuales), vec![a]);
    }

    #[test]
    fn conflictos_vacio_si_todo_coincide_o_no_hay_registro() {
        let a = Uuid::from_u128(10);
        let esperadas: HashMap<Uuid, i32> = HashMap::new();
        assert!(conflictos(&esperadas, &[(a, 7)]).is_empty());
        let mut e2 = HashMap::new();
        e2.insert(a, 7);
        assert!(conflictos(&e2, &[(a, 7)]).is_empty());
    }

    #[test]
    fn registro_de_versiones_guarda_lee_y_olvida() {
        let _g = LOCK_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        olvidar_versiones();
        let id = Uuid::from_u128(77);
        assert_eq!(version_esperada(id), None);
        registrar_versiones(&[(id, 3)]);
        assert_eq!(version_esperada(id), Some(3));
        registrar_versiones(&[(id, 5)]);
        assert_eq!(version_esperada(id), Some(5));
        olvidar_versiones();
        assert_eq!(version_esperada(id), None);
    }

    #[test]
    fn fijar_y_leer_analista_actual() {
        let _g = LOCK_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        fijar_analista_actual(None);
        assert_eq!(analista_actual(), None);
        assert_eq!(id_analista_actual(), None);
        assert_eq!(nombre_analista_actual(), "");

        let a = Analista {
            id: Uuid::from_u128(5),
            nombre: "ANA PRUEBA".to_string(),
        };
        fijar_analista_actual(Some(a.clone()));
        assert_eq!(analista_actual(), Some(a.clone()));
        assert_eq!(id_analista_actual(), Some(a.id));
        assert_eq!(nombre_analista_actual(), "ANA PRUEBA");
        fijar_analista_actual(None);
        assert_eq!(nombre_analista_actual(), "");
    }
}
