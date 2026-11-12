//! Bitácora de ediciones del Reporte de Liquidaciones.
//!
//! Cada acción del analista se registra como un lote: al abrirlo se guarda una foto JSON
//! "antes" por reporte afectado y al cerrarlo se recalcula el prorrateo canónico, se marca
//! `editado_usuario` y se guarda la foto "despues". Todas las operaciones usan el cliente
//! recibido, por lo que se ejecutan dentro de la transacción del llamador.

use postgres::GenericClient;
use uuid::Uuid;

use crate::db::prorrateo::recalcular_prorrateo_tx;
use crate::db::schema::format_pg_error;

/// Acciones registradas en la bitácora.
pub const ACCION_DIAS: &str = "DIAS";
pub const ACCION_VINCULAR: &str = "VINCULAR";
pub const ACCION_DESVINCULAR: &str = "DESVINCULAR";
pub const ACCION_ASIGNAR_LOTE: &str = "ASIGNAR_LOTE";
pub const ACCION_REVERTIR: &str = "REVERTIR";

/// Longitud máxima de la columna `reportes_cambios.accion` (varchar(30)).
const LONGITUD_MAX_ACCION: usize = 30;

/// Subconsulta JSON que describe un reporte (`rl`) con sus tramos, incluyendo obra y días
/// de registro de obra. Se usa como expresión correlacionada sobre `public.reportes_liquidaciones rl`.
const SQL_FOTO_REPORTE: &str = "jsonb_build_object(\
    'reporte', jsonb_build_object(\
        'id', rl.id, \
        'estado', rl.estado, \
        'editado_usuario', rl.editado_usuario, \
        'cedula_actualizada', rl.cedula_actualizada\
    ), \
    'tramos', COALESCE((\
        SELECT jsonb_agg(jsonb_build_object(\
            'id', d.id, \
            'clave', d.clave, \
            'fecha', d.fecha, \
            'dias', d.dias, \
            'id_registro_obra', d.id_registro_obra, \
            'dias_registro_obra', d.dias_registro_obra, \
            'subtotal', d.subtotal\
        ) ORDER BY d.fecha NULLS FIRST, d.id) \
        FROM public.reportes_detalles_liquidaciones d \
        WHERE d.id_reporte_liquidacion = rl.id\
    ), '[]'::jsonb)\
)";

/// Valida que la acción quepa en la columna `accion`.
fn validar_accion(accion: &str) -> Result<(), String> {
    if accion.chars().count() > LONGITUD_MAX_ACCION {
        return Err(format!(
            "Acción de bitácora '{}' excede {} caracteres",
            accion, LONGITUD_MAX_ACCION
        ));
    }
    Ok(())
}

/// Ids de reportes (padres) de un conjunto de tramos de reportes_detalles_liquidaciones.
pub fn reportes_de_tramos<C: GenericClient>(
    c: &mut C,
    ids_tramos: &[Uuid],
) -> Result<Vec<Uuid>, String> {
    let filas = c
        .query(
            "SELECT DISTINCT id_reporte_liquidacion FROM public.reportes_detalles_liquidaciones WHERE id = ANY($1)",
            &[&ids_tramos],
        )
        .map_err(|e| format!("reportes_de_tramos: {}", format_pg_error(&e)))?;
    Ok(filas.iter().map(|r| r.get(0)).collect())
}

/// Abre un lote: inserta una fila por reporte con `antes` = foto actual. Devuelve el lote_id.
pub fn iniciar_edicion<C: GenericClient>(
    c: &mut C,
    accion: &str,
    ids_reportes: &[Uuid],
) -> Result<Uuid, String> {
    validar_accion(accion)?;
    let lote = Uuid::now_v7();
    if ids_reportes.is_empty() {
        return Ok(lote);
    }
    // El WHERE sobre reportes_liquidaciones produce una sola fila por reporte aunque el id se repita.
    let sql = format!(
        "INSERT INTO public.reportes_cambios (id_revision, id_reporte_liquidacion, lote_id, accion, antes, usuario) \
         SELECT rl.id_revision, rl.id, $1, $2, {}, current_user \
         FROM public.reportes_liquidaciones rl \
         WHERE rl.id = ANY($3)",
        SQL_FOTO_REPORTE
    );
    c.execute(sql.as_str(), &[&lote, &accion, &ids_reportes])
        .map_err(|e| format!("iniciar_edicion ({}): {}", accion, format_pg_error(&e)))?;
    Ok(lote)
}

/// Cierra el lote: recalcula el prorrateo canónico de esos reportes, completa `despues` con la foto
/// final y aplica `marcar_editado` a editado_usuario (true para ediciones; false para REVERTIR, que
/// además pone cedula_actualizada = false). Devuelve el número de reportes afectados.
pub fn cerrar_edicion<C: GenericClient>(
    c: &mut C,
    lote_id: Uuid,
    marcar_editado: bool,
) -> Result<u64, String> {
    let filas = c
        .query(
            "SELECT DISTINCT id_reporte_liquidacion FROM public.reportes_cambios \
             WHERE lote_id = $1 AND id_reporte_liquidacion IS NOT NULL",
            &[&lote_id],
        )
        .map_err(|e| format!("cerrar_edicion (consulta de reportes): {}", format_pg_error(&e)))?;
    if filas.is_empty() {
        return Ok(0);
    }
    let ids: Vec<Uuid> = filas.iter().map(|r| r.get(0)).collect();

    recalcular_prorrateo_tx(c, &ids).map_err(|e| format!("cerrar_edicion (prorrateo): {}", e))?;

    c.execute(
        "UPDATE public.reportes_liquidaciones \
         SET editado_usuario = $2, \
             cedula_actualizada = CASE WHEN $2 THEN cedula_actualizada ELSE false END \
         WHERE id = ANY($1)",
        &[&ids, &marcar_editado],
    )
    .map_err(|e| format!("cerrar_edicion (marcas de edición): {}", format_pg_error(&e)))?;

    let sql_despues = format!(
        "UPDATE public.reportes_cambios rc SET despues = {} \
         FROM public.reportes_liquidaciones rl \
         WHERE rc.lote_id = $1 AND rl.id = rc.id_reporte_liquidacion",
        SQL_FOTO_REPORTE
    );
    c.execute(sql_despues.as_str(), &[&lote_id])
        .map_err(|e| format!("cerrar_edicion (foto despues): {}", format_pg_error(&e)))?;

    Ok(ids.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitacora_foto_incluye_obra_y_dias_por_tramo() {
        assert!(SQL_FOTO_REPORTE.contains("tramos"));
        assert!(SQL_FOTO_REPORTE.contains("id_registro_obra"));
        assert!(SQL_FOTO_REPORTE.contains("dias_registro_obra"));
    }

    #[test]
    fn bitacora_acciones_caben_en_varchar_30() {
        for accion in [
            ACCION_DIAS,
            ACCION_VINCULAR,
            ACCION_DESVINCULAR,
            ACCION_ASIGNAR_LOTE,
            ACCION_REVERTIR,
        ] {
            assert!(validar_accion(accion).is_ok(), "acción fuera de límite: {}", accion);
        }
        assert!(validar_accion(&"X".repeat(31)).is_err());
    }

    /// Prueba de integración con BD (no se ejecuta por defecto).
    #[test]
    #[ignore]
    fn bitacora_ciclo_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut tx = client.transaction().expect("No se pudo iniciar la transacción");

        let id_reporte: Option<Uuid> = tx
            .query("SELECT id FROM public.reportes_liquidaciones LIMIT 1", &[])
            .expect("Consulta de reportes")
            .first()
            .map(|r| r.get(0));
        let Some(id_reporte) = id_reporte else {
            return;
        };

        let lote = iniciar_edicion(&mut tx, ACCION_DIAS, &[id_reporte]).expect("iniciar_edicion");
        let afectados = cerrar_edicion(&mut tx, lote, true).expect("cerrar_edicion");
        assert_eq!(afectados, 1);

        let fila = tx
            .query_one(
                "SELECT rc.antes IS NOT NULL, rc.despues IS NOT NULL, rl.editado_usuario \
                 FROM public.reportes_cambios rc \
                 JOIN public.reportes_liquidaciones rl ON rl.id = rc.id_reporte_liquidacion \
                 WHERE rc.lote_id = $1",
                &[&lote],
            )
            .expect("Consulta de bitácora");
        let tiene_antes: bool = fila.get(0);
        let tiene_despues: bool = fila.get(1);
        let editado: bool = fila.get(2);
        assert!(tiene_antes, "antes debe existir");
        assert!(tiene_despues, "despues debe existir");
        assert!(editado, "editado_usuario debe ser true");

        tx.rollback().expect("rollback de la prueba");
    }
}
