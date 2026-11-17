//! Operaciones de asignación sobre el Reporte de Liquidaciones con registro en la bitácora.
//!
//! `revertir_reportes_empleado` devuelve a la cédula los reportes de un empleado dentro de una
//! revisión: borra sus tramos y su reflejo rvol, marca el reporte como no editado y lo reconstruye
//! desde la cédula con su obra. Todo ocurre en una sola transacción.

use postgres::{Client, Transaction};
use uuid::Uuid;

use crate::db::bitacora_reportes::{cerrar_edicion, iniciar_edicion, ACCION_REVERTIR};
use crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx;
use crate::db::schema::format_pg_error;

const SQL_IDS_EMPLEADO: &str =
    "SELECT id FROM public.reportes_liquidaciones WHERE id_revision = $1 AND id_empleado = $2";

const SQL_BORRAR_TRAMOS: &str =
    "DELETE FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion = ANY($1)";

const SQL_BORRAR_RVOL: &str = "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones \
    WHERE id_reporte_liquidacion = ANY($1)";

const SQL_MARCAR_NO_EDITADO: &str = "UPDATE public.reportes_liquidaciones \
    SET editado_usuario = false, cedula_actualizada = false \
    WHERE id = ANY($1)";

/// Revierte a la cédula los reportes de un empleado en una revisión, con registro en la bitácora.
/// Confirma su propia transacción. Devuelve el número de reportes afectados.
pub fn revertir_reportes_empleado(
    client: &mut Client,
    id_revision: Uuid,
    id_empleado: Uuid,
) -> Result<u64, String> {
    let mut tx = client
        .transaction()
        .map_err(|e| format!("revertir_reportes_empleado (inicio): {}", format_pg_error(&e)))?;
    let afectados = revertir_reportes_empleado_tx(&mut tx, id_revision, id_empleado)?;
    tx.commit()
        .map_err(|e| format!("revertir_reportes_empleado (commit): {}", format_pg_error(&e)))?;
    Ok(afectados)
}

/// Versión sobre una transacción existente de `revertir_reportes_empleado`.
pub fn revertir_reportes_empleado_tx(
    tx: &mut Transaction,
    id_revision: Uuid,
    id_empleado: Uuid,
) -> Result<u64, String> {
    let filas = tx
        .query(SQL_IDS_EMPLEADO, &[&id_revision, &id_empleado])
        .map_err(|e| format!("revertir (consulta de reportes): {}", format_pg_error(&e)))?;
    let ids: Vec<Uuid> = filas.iter().map(|r| r.get(0)).collect();
    if ids.is_empty() {
        return Ok(0);
    }

    let lote = iniciar_edicion(tx, ACCION_REVERTIR, &ids)?;

    tx.execute(SQL_BORRAR_TRAMOS, &[&ids])
        .map_err(|e| format!("revertir (borrar tramos): {}", format_pg_error(&e)))?;
    tx.execute(SQL_BORRAR_RVOL, &[&ids])
        .map_err(|e| format!("revertir (borrar rvol): {}", format_pg_error(&e)))?;
    tx.execute(SQL_MARCAR_NO_EDITADO, &[&ids])
        .map_err(|e| format!("revertir (marcar no editado): {}", format_pg_error(&e)))?;

    // Al estar no editados, la sincronización reconstruye sus tramos desde la cédula con su obra.
    sync_reportes_liquidaciones_for_revision_tx(tx, id_revision)?;

    // cerrar_edicion regenera rvol desde los tramos reconstruidos y recalcula el prorrateo.
    cerrar_edicion(tx, lote, false)
}

/// Aplica de forma ATÓMICA un plan de tramos sobre reportes de liquidación: borra los tramos hermanos
/// indicados y fija obra/días de los tramos a guardar, en UNA transacción y UN lote de bitácora
/// (un solo "Deshacer" revierte la acción completa). Los importes se recalculan con la regla canónica
/// al cerrar el lote. `guardar`: (id_tramo, dias_cedula (no se modifica), dias_obra, obra o None).
pub fn aplicar_plan_tramos(
    client: &mut postgres::Client,
    accion: &str,
    borrar: &[Uuid],
    guardar: &[(Uuid, i16, i16, Option<Uuid>)],
) -> Result<u64, String> {
    if borrar.is_empty() && guardar.is_empty() {
        return Ok(0);
    }
    let mut tx = client
        .transaction()
        .map_err(|e| format!("aplicar_plan_tramos (iniciar): {}", crate::db::schema::format_pg_error(&e)))?;
    let mut ids_tramos: Vec<Uuid> = borrar.to_vec();
    ids_tramos.extend(guardar.iter().map(|g| g.0));
    // Los padres se calculan ANTES de borrar (después ya no existirían los tramos).
    let ids_reportes = crate::db::bitacora_reportes::reportes_de_tramos(&mut tx, &ids_tramos)?;
    let lote = crate::db::bitacora_reportes::iniciar_edicion(&mut tx, accion, &ids_reportes)?;
    if !borrar.is_empty() {
        tx.execute(
            "DELETE FROM public.reportes_detalles_liquidaciones WHERE id = ANY($1)",
            &[&borrar],
        )
        .map_err(|e| format!("aplicar_plan_tramos (borrar): {}", crate::db::schema::format_pg_error(&e)))?;
    }
    for (id_tramo, _dias_cedula, dias_obra, obra) in guardar {
        let dias_obra = (*dias_obra).clamp(0, 31);
        tx.execute(
            "UPDATE public.reportes_detalles_liquidaciones SET id_registro_obra = $2, dias_registro_obra = $3 WHERE id = $1",
            &[id_tramo, obra, &dias_obra],
        )
        .map_err(|e| format!("aplicar_plan_tramos (guardar): {}", crate::db::schema::format_pg_error(&e)))?;
    }
    let n = crate::db::bitacora_reportes::cerrar_edicion(&mut tx, lote, true)?;
    tx.commit()
        .map_err(|e| format!("aplicar_plan_tramos (commit): {}", crate::db::schema::format_pg_error(&e)))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revertir_borra_rdl_y_rvol_solo_de_los_reportes_recibidos() {
        assert!(SQL_BORRAR_TRAMOS.contains("reportes_detalles_liquidaciones"));
        assert!(SQL_BORRAR_TRAMOS.contains("id_reporte_liquidacion = ANY($1)"));
        assert!(SQL_BORRAR_RVOL.contains("reportes_vinculaciones_obras_liquidaciones"));
        assert!(SQL_BORRAR_RVOL.contains("id_reporte_liquidacion = ANY($1)"));
    }

    #[test]
    fn revertir_marca_reportes_como_no_editados() {
        assert!(SQL_MARCAR_NO_EDITADO.contains("editado_usuario = false"));
        assert!(SQL_MARCAR_NO_EDITADO.contains("cedula_actualizada = false"));
        assert!(SQL_IDS_EMPLEADO.contains("id_empleado = $2"));
    }

    /// Prueba de integración con BD (no se ejecuta por defecto). Usa ROLLBACK: no deja cambios.
    #[test]
    #[ignore]
    fn asignacion_revertir_reportes_empleado_tx_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(_) => return,
        };
        let par: Option<(Uuid, Uuid)> = client
            .query(
                "SELECT id_revision, id_empleado FROM public.reportes_liquidaciones \
                 WHERE id_empleado IS NOT NULL LIMIT 1",
                &[],
            )
            .expect("Consulta de reportes")
            .first()
            .map(|r| (r.get(0), r.get(1)));
        let Some((revision, empleado)) = par else {
            return;
        };
        let mut tx = client.transaction().expect("No se pudo iniciar la transacción");
        let afectados =
            revertir_reportes_empleado_tx(&mut tx, revision, empleado).expect("revertir_tx");
        assert!(afectados >= 1);
        tx.rollback().expect("rollback de la prueba");
    }

    /// ADVERTENCIA: esta prueba abre una transacción y la CONFIRMA (commit) en la BD configurada
    /// (producción). Usa una revisión y empleado inexistentes, por lo que no debería escribir filas;
    /// ejecutar solo explícitamente con --ignored.
    #[test]
    #[ignore]
    fn asignacion_revertir_reportes_empleado_publico_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(_) => return,
        };
        let afectados = revertir_reportes_empleado(&mut client, Uuid::nil(), Uuid::nil())
            .expect("revertir_reportes_empleado");
        assert_eq!(afectados, 0);
    }
}
