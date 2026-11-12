//! Encabezado oficial del Excel IMSS y registro de versiones de entregables.
//!
//! Todos los datos del encabezado salen exclusivamente de la BD; ningún campo
//! se rellena con valores por defecto.

use chrono::NaiveDate;
use postgres::error::SqlState;
use uuid::Uuid;

use crate::db::reporte_liquidaciones::ReporteFinalHeaderMeta;
use crate::db::schema::format_pg_error;

/// Tipos de entregable válidos en `exportaciones_reportes.tipo`.
const TIPOS_VALIDOS: [&str; 2] = ["XLSX", "SUA"];

/// Registro de una exportación previamente generada.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportacionInfo {
    pub id: Uuid,
    pub tipo: String,
    pub version: i32,
    pub sha256: String,
    pub rustfs_key: String,
    pub resumen: Option<serde_json::Value>,
    /// Fecha de creación formateada "dd/mm/aaaa HH:MM" en hora local de la BD.
    pub creado: String,
}

/// Formatea el periodo de la obra para el encabezado.
///
/// Devuelve cadena vacía si no hay ninguna fecha; nunca inventa valores.
pub fn formatear_periodo(inicio: Option<NaiveDate>, fin: Option<NaiveDate>) -> String {
    match (inicio, fin) {
        (Some(i), Some(f)) => format!("{} AL {}", i.format("%d/%m/%Y"), f.format("%d/%m/%Y")),
        (Some(i), None) => format!("DESDE {}", i.format("%d/%m/%Y")),
        (None, Some(f)) => format!("HASTA {}", f.format("%d/%m/%Y")),
        (None, None) => String::new(),
    }
}

/// Convierte un valor opcional de BD en cadena recortada (NULL → "").
fn limpia(valor: Option<String>) -> String {
    valor.map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Obtiene de la BD los datos del encabezado oficial de una revisión.
///
/// Prioridad del periodo: periodo de la revisión → vigencias (MIN/MAX) de las
/// obras creadas por usuario → vacío.
pub fn fetch_reporte_header_meta(
    client: &mut postgres::Client,
    id_revision: Uuid,
) -> Result<ReporteFinalHeaderMeta, String> {
    let fila = client
        .query_opt(
            "SELECT
                c.razon_social,
                c.nombre,
                c.rfc,
                c.representante_legal,
                rp.registro,
                rp.delegacion,
                rp.subdelegacion,
                r.folio::text,
                r.periodo_inicio,
                r.periodo_fin,
                (SELECT MIN(o.fecha_inicio) FROM public.registros_obras o
                  WHERE o.id_revision = r.id AND o.creado_usuario) AS obras_inicio,
                (SELECT MAX(o.fecha_fin) FROM public.registros_obras o
                  WHERE o.id_revision = r.id AND o.creado_usuario) AS obras_fin
             FROM public.revisiones r
             LEFT JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
             LEFT JOIN public.clientes c ON c.id = rp.id_cliente
             WHERE r.id = $1",
            &[&id_revision],
        )
        .map_err(|e| format_pg_error(&e))?
        .ok_or_else(|| format!("La revisión {id_revision} no existe"))?;

    let razon_social = limpia(fila.get(0));
    let nombre = limpia(fila.get(1));
    let cliente_nombre = if razon_social.is_empty() { nombre } else { razon_social };

    let folio = limpia(fila.get(7));
    let revision_folio = if folio.eq_ignore_ascii_case("S/F") { String::new() } else { folio };

    let periodo_inicio: Option<NaiveDate> = fila.get(8);
    let periodo_fin: Option<NaiveDate> = fila.get(9);
    let obras_inicio: Option<NaiveDate> = fila.get(10);
    let obras_fin: Option<NaiveDate> = fila.get(11);
    let periodo_obra = formatear_periodo(
        periodo_inicio.or(obras_inicio),
        periodo_fin.or(obras_fin),
    );

    Ok(ReporteFinalHeaderMeta {
        delegacion: limpia(fila.get(5)),
        subdelegacion: limpia(fila.get(6)),
        cliente_nombre,
        rp_registro: limpia(fila.get(4)),
        rfc: limpia(fila.get(2)),
        periodo_obra,
        representante_legal: limpia(fila.get(3)),
        revision_folio,
    })
}

/// Valida tipo de entregable y formato del hash SHA-256 antes de registrar.
fn validar_registro(tipo: &str, sha256: &str) -> Result<(), String> {
    if !TIPOS_VALIDOS.contains(&tipo) {
        return Err(format!("Tipo de entregable inválido: '{tipo}'"));
    }
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("El sha256 debe tener 64 caracteres hexadecimales".to_string());
    }
    Ok(())
}

/// Devuelve la siguiente versión disponible para (revisión, tipo).
pub fn siguiente_version(
    client: &mut postgres::Client,
    id_revision: Uuid,
    tipo: &str,
) -> Result<i32, String> {
    if !TIPOS_VALIDOS.contains(&tipo) {
        return Err(format!("Tipo de entregable inválido: '{tipo}'"));
    }
    let fila = client
        .query_one(
            "SELECT COALESCE(MAX(version), 0) + 1
             FROM public.exportaciones_reportes
             WHERE id_revision = $1 AND tipo = $2",
            &[&id_revision, &tipo],
        )
        .map_err(|e| format_pg_error(&e))?;
    Ok(fila.get::<_, i32>(0))
}

/// Registra una exportación generada y devuelve su id.
///
/// Si otra exportación ya tomó la misma versión (restricción única), devuelve
/// un error indicando que hay que volver a generar.
pub fn registrar_exportacion(
    client: &mut postgres::Client,
    id_revision: Uuid,
    tipo: &str,
    version: i32,
    sha256: &str,
    rustfs_key: &str,
    resumen: &serde_json::Value,
) -> Result<Uuid, String> {
    validar_registro(tipo, sha256)?;
    // Se envía como texto y se castea en SQL: el crate postgres no tiene
    // habilitado el soporte serde_json para jsonb.
    let resumen_txt = resumen.to_string();
    let fila = client
        .query_one(
            "INSERT INTO public.exportaciones_reportes
                (id_revision, tipo, version, sha256, rustfs_key, resumen)
             VALUES ($1, $2, $3, $4, $5, $6::text::jsonb)
             RETURNING id",
            &[&id_revision, &tipo, &version, &sha256, &rustfs_key, &resumen_txt],
        )
        .map_err(|e| {
            if e.code() == Some(&SqlState::UNIQUE_VIOLATION) {
                format!("La versión {version} ya existe; vuelva a generar")
            } else {
                format_pg_error(&e)
            }
        })?;
    Ok(fila.get::<_, Uuid>(0))
}

/// Lista las exportaciones de una revisión, de la más reciente a la más antigua.
pub fn listar_exportaciones(
    client: &mut postgres::Client,
    id_revision: Uuid,
) -> Result<Vec<ExportacionInfo>, String> {
    let filas = client
        .query(
            "SELECT id, tipo, version, sha256, rustfs_key, resumen::text,
                    to_char(created_at, 'DD/MM/YYYY HH24:MI')
             FROM public.exportaciones_reportes
             WHERE id_revision = $1
             ORDER BY created_at DESC",
            &[&id_revision],
        )
        .map_err(|e| format_pg_error(&e))?;

    filas
        .iter()
        .map(|f| {
            let resumen_txt: Option<String> = f.get(5);
            let resumen = match resumen_txt {
                Some(txt) => Some(
                    serde_json::from_str::<serde_json::Value>(&txt)
                        .map_err(|e| format!("Resumen de exportación inválido: {e}"))?,
                ),
                None => None,
            };
            Ok(ExportacionInfo {
                id: f.get(0),
                tipo: f.get(1),
                version: f.get(2),
                sha256: limpia(f.get(3)),
                rustfs_key: f.get(4),
                resumen,
                creado: f.get::<_, Option<String>>(6).unwrap_or_default(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::reporte_liquidaciones::missing_header_fields;

    fn d(a: i32, m: u32, g: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(a, m, g).unwrap()
    }

    #[test]
    fn formatear_periodo_ambas_fechas() {
        assert_eq!(
            formatear_periodo(Some(d(2024, 1, 5)), Some(d(2024, 3, 31))),
            "05/01/2024 AL 31/03/2024"
        );
    }

    #[test]
    fn formatear_periodo_solo_inicio() {
        assert_eq!(formatear_periodo(Some(d(2024, 1, 5)), None), "DESDE 05/01/2024");
    }

    #[test]
    fn formatear_periodo_solo_fin() {
        assert_eq!(formatear_periodo(None, Some(d(2024, 3, 31))), "HASTA 31/03/2024");
    }

    #[test]
    fn formatear_periodo_ninguna() {
        assert_eq!(formatear_periodo(None, None), "");
    }

    #[test]
    fn validar_registro_tipo_y_hash() {
        let hash_ok = "a".repeat(64);
        assert!(validar_registro("XLSX", &hash_ok).is_ok());
        assert!(validar_registro("SUA", &hash_ok).is_ok());
        assert!(validar_registro("PDF", &hash_ok).is_err());
        assert!(validar_registro("XLSX", &"a".repeat(63)).is_err());
        assert!(validar_registro("XLSX", &format!("{}g", "a".repeat(63))).is_err());
    }

    /// Solo lectura: imprime el encabezado de cada revisión existente.
    /// Requiere BD configurada; NO ejecutar salvo explícitamente con --ignored.
    #[test]
    #[ignore]
    fn lectura_header_meta_revisiones_existentes() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("PostgreSQL no disponible en test (skipping): {}", e);
                return;
            }
        };
        let ids = client
            .query("SELECT id FROM public.revisiones", &[])
            .expect("No se pudo listar revisiones");
        for fila in ids {
            let id: Uuid = fila.get(0);
            match fetch_reporte_header_meta(&mut client, id) {
                Ok(meta) => {
                    println!("REVISION {id}: {meta:?}");
                    println!("  faltantes: {:?}", missing_header_fields(&meta));
                }
                Err(e) => println!("REVISION {id}: ERROR {e}"),
            }
        }
    }
}
