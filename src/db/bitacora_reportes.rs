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
pub const ACCION_DESHACER: &str = "DESHACER";
pub const ACCION_DESACTIVAR_OBRA: &str = "DESACTIVAR_OBRA";

/// Reactiva las obras que el lote desactivó (`registros_obras.activa`). Deshacer una desactivación
/// debe devolver la obra al catálogo activo.
const SQL_REACTIVAR_OBRAS_DEL_LOTE: &str = "UPDATE public.registros_obras SET activa = true, desactivada_at = NULL \
    WHERE id IN (SELECT id_registro_obra FROM public.reportes_cambios \
    WHERE lote_id = $1 AND id_registro_obra IS NOT NULL)";

/// Longitud máxima de la columna `reportes_cambios.accion` (varchar(30)).
const LONGITUD_MAX_ACCION: usize = 30;

/// Regeneración de la tabla legada `reportes_vinculaciones_obras_liquidaciones` (rvol).
/// rdl (`reportes_detalles_liquidaciones`) es la fuente de verdad; rvol es un reflejo derivado
/// que se reconstruye agrupando los tramos por obra al cerrar cada edición.
const SQL_RVOL_DELETE: &str = "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones \
    WHERE id_reporte_liquidacion = ANY($1)";

const SQL_RVOL_INSERT: &str = "INSERT INTO public.reportes_vinculaciones_obras_liquidaciones \
    (id_registro_obra, id_reporte_liquidacion, dias_registro_obra) \
    SELECT d.id_registro_obra, d.id_reporte_liquidacion, \
           LEAST(SUM(COALESCE(d.dias_registro_obra, 0)), 31)::smallint \
    FROM public.reportes_detalles_liquidaciones d \
    WHERE d.id_reporte_liquidacion = ANY($1) AND d.id_registro_obra IS NOT NULL \
    GROUP BY d.id_registro_obra, d.id_reporte_liquidacion";

/// Restaura los tramos de un reporte (`$1` = id del reporte, `$2` = foto "antes" en texto JSON).
/// Los importes base se toman de `detalles_liquidaciones` (cédula) por clave y fecha; el prorrateo
/// lo recalcula después `cerrar_edicion`.
const SQL_RESTAURAR_TRAMOS: &str = "INSERT INTO public.reportes_detalles_liquidaciones (\
    id, id_reporte_liquidacion, clave, fecha, dias, sdi, lic, inc, aus, cf, exc_pat, exc_obr, \
    pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr, gps, patronal, obrera, subtotal, \
    id_registro_obra, dias_registro_obra) \
    SELECT (t->>'id')::uuid, $1, NULLIF(t->>'clave', ''), (t->>'fecha')::date, \
           (t->>'dias')::smallint, dl.sdi, dl.lic, dl.inc, dl.aus, dl.cf, dl.exc_pat, dl.exc_obr, \
           dl.pd_pat, dl.pd_obr, dl.gmp_pat, dl.gmp_obr, dl.rt, dl.iv_pat, dl.iv_obr, dl.gps, \
           dl.patronal, dl.obrera, dl.subtotal, \
           (t->>'id_registro_obra')::uuid, (t->>'dias_registro_obra')::smallint \
    FROM jsonb_array_elements($2::text::jsonb -> 'tramos') t \
    JOIN public.reportes_liquidaciones rl ON rl.id = $1 \
    LEFT JOIN LATERAL (\
        SELECT * FROM public.detalles_liquidaciones x \
        WHERE x.id_liquidacion = rl.id_liquidacion \
          AND COALESCE(BTRIM(LOWER(x.clave)), '') = COALESCE(BTRIM(LOWER(t->>'clave')), '') \
          AND x.fecha IS NOT DISTINCT FROM (t->>'fecha')::date \
        ORDER BY x.id LIMIT 1\
    ) dl ON true";

/// Restaura `editado_usuario` y `cedula_actualizada` desde la foto "antes" del lote `$1`.
const SQL_RESTAURAR_MARCAS: &str = "UPDATE public.reportes_liquidaciones rl \
    SET editado_usuario = COALESCE((rc.antes -> 'reporte' ->> 'editado_usuario')::boolean, rl.editado_usuario), \
        cedula_actualizada = COALESCE((rc.antes -> 'reporte' ->> 'cedula_actualizada')::boolean, rl.cedula_actualizada) \
    FROM public.reportes_cambios rc \
    WHERE rc.lote_id = $1 AND rc.id_reporte_liquidacion = rl.id";

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

    crate::db::topes::validar_topes_lote(c, lote_id)?;
    recalcular_prorrateo_tx(c, &ids).map_err(|e| format!("cerrar_edicion (prorrateo): {}", e))?;

    c.execute(
        "UPDATE public.reportes_liquidaciones \
         SET editado_usuario = $2, \
             cedula_actualizada = CASE WHEN $2 THEN cedula_actualizada ELSE false END \
         WHERE id = ANY($1)",
        &[&ids, &marcar_editado],
    )
    .map_err(|e| format!("cerrar_edicion (marcas de edición): {}", format_pg_error(&e)))?;

    // rdl es la fuente de verdad; rvol es un reflejo derivado que se regenera desde los tramos.
    c.execute(SQL_RVOL_DELETE, &[&ids])
        .map_err(|e| format!("cerrar_edicion (rvol borrar): {}", format_pg_error(&e)))?;
    c.execute(SQL_RVOL_INSERT, &[&ids])
        .map_err(|e| format!("cerrar_edicion (rvol regenerar): {}", format_pg_error(&e)))?;

    registrar_foto_despues(c, lote_id)?;

    Ok(ids.len() as u64)
}

/// Completa la columna `despues` de todas las filas del lote con la foto actual de cada reporte.
fn registrar_foto_despues<C: GenericClient>(c: &mut C, lote_id: Uuid) -> Result<(), String> {
    let sql_despues = format!(
        "UPDATE public.reportes_cambios rc SET despues = {} \
         FROM public.reportes_liquidaciones rl \
         WHERE rc.lote_id = $1 AND rl.id = rc.id_reporte_liquidacion",
        SQL_FOTO_REPORTE
    );
    c.execute(sql_despues.as_str(), &[&lote_id])
        .map_err(|e| format!("registrar foto despues: {}", format_pg_error(&e)))?;
    Ok(())
}

/// Deshace un lote restaurando la foto "antes" de sus reportes, dentro del cliente recibido
/// (normalmente una transacción). Abre un lote `DESHACER` propio, reinserta los tramos de la foto,
/// recalcula el prorrateo y restaura las marcas. Devuelve el número de reportes restaurados.
///
/// Rechaza el lote si algún reporte fue reconstruido (`id_reporte_liquidacion` NULL por borrado).
pub fn deshacer_lote_tx<C: GenericClient>(c: &mut C, lote_id: Uuid) -> Result<u64, String> {
    const MSG_NO_DESHACIBLE: &str =
        "Este cambio no se puede deshacer automáticamente: el reporte fue reconstruido";

    let filas = c
        .query(
            "SELECT id_reporte_liquidacion, antes::text FROM public.reportes_cambios \
             WHERE lote_id = $1 ORDER BY created_at, id",
            &[&lote_id],
        )
        .map_err(|e| format!("deshacer_lote (consulta del lote): {}", format_pg_error(&e)))?;
    if filas.is_empty() {
        return Ok(0);
    }

    let mut fotos: Vec<(Uuid, String)> = Vec::with_capacity(filas.len());
    for fila in &filas {
        let id_reporte: Option<Uuid> = fila.get(0);
        let Some(id_reporte) = id_reporte else {
            return Err(MSG_NO_DESHACIBLE.to_string());
        };
        let antes: Option<String> = fila.get(1);
        let antes = antes.ok_or_else(|| "deshacer_lote: el lote no tiene foto 'antes'".to_string())?;
        fotos.push((id_reporte, antes));
    }
    let ids: Vec<Uuid> = fotos.iter().map(|(id, _)| *id).collect();

    c.execute(SQL_REACTIVAR_OBRAS_DEL_LOTE, &[&lote_id])
        .map_err(|e| format!("deshacer_lote (reactivar obra): {}", format_pg_error(&e)))?;

    let lote_nuevo = iniciar_edicion(c, ACCION_DESHACER, &ids)?;

    for (id_reporte, antes) in &fotos {
        c.execute(
            "DELETE FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion = $1",
            &[id_reporte],
        )
        .map_err(|e| format!("deshacer_lote (borrar tramos): {}", format_pg_error(&e)))?;
        c.execute(SQL_RESTAURAR_TRAMOS, &[id_reporte, antes])
            .map_err(|e| format!("deshacer_lote (restaurar tramos): {}", format_pg_error(&e)))?;
    }

    // cerrar_edicion fuerza editado_usuario = true; por eso las marcas originales se reaplican después.
    cerrar_edicion(c, lote_nuevo, true)?;
    c.execute(SQL_RESTAURAR_MARCAS, &[&lote_id])
        .map_err(|e| format!("deshacer_lote (restaurar marcas): {}", format_pg_error(&e)))?;
    registrar_foto_despues(c, lote_nuevo)?;
    // Pila de deshacer: el lote original queda marcado para que el siguiente "Deshacer" retroceda un paso más
    c.execute(
        "UPDATE public.reportes_cambios SET deshecho_por = $2 WHERE lote_id = $1",
        &[&lote_id, &lote_nuevo],
    )
    .map_err(|e| format!("deshacer_lote (marcar lote deshecho): {}", format_pg_error(&e)))?;

    Ok(ids.len() as u64)
}

/// Igual que `deshacer_lote_tx`, pero abre y confirma su propia transacción.
pub fn deshacer_lote(client: &mut postgres::Client, lote_id: Uuid) -> Result<u64, String> {
    let mut tx = client
        .transaction()
        .map_err(|e| format!("deshacer_lote (inicio): {}", format_pg_error(&e)))?;
    let restaurados = deshacer_lote_tx(&mut tx, lote_id)?;
    tx.commit()
        .map_err(|e| format!("deshacer_lote (commit): {}", format_pg_error(&e)))?;
    Ok(restaurados)
}

/// Devuelve el lote más reciente de la revisión que no sea de acción `DESHACER`, con su acción.
pub fn ultimo_lote_revision(
    client: &mut postgres::Client,
    id_revision: Uuid,
) -> Result<Option<(Uuid, String)>, String> {
    let fila = client
        .query_opt(
            "SELECT lote_id, MIN(accion) FROM public.reportes_cambios \
             WHERE id_revision = $1 AND accion <> $2 AND deshecho_por IS NULL \
             GROUP BY lote_id ORDER BY MAX(created_at) DESC LIMIT 1",
            &[&id_revision, &ACCION_DESHACER],
        )
        .map_err(|e| format!("ultimo_lote_revision: {}", format_pg_error(&e)))?;
    Ok(fila.map(|r| (r.get(0), r.get(1))))
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
            ACCION_DESHACER,
            ACCION_DESACTIVAR_OBRA,
        ] {
            assert!(validar_accion(accion).is_ok(), "acción fuera de límite: {}", accion);
        }
        assert!(validar_accion(&"X".repeat(31)).is_err());
    }

    #[test]
    fn rvol_se_regenera_agrupando_por_obra() {
        assert!(SQL_RVOL_DELETE.contains("reportes_vinculaciones_obras_liquidaciones"));
        assert!(SQL_RVOL_DELETE.contains("id_reporte_liquidacion = ANY($1)"));
        assert!(SQL_RVOL_INSERT.contains("GROUP BY d.id_registro_obra, d.id_reporte_liquidacion"));
        assert!(SQL_RVOL_INSERT.contains("LEAST(SUM(COALESCE(d.dias_registro_obra, 0)), 31)"));
        assert!(SQL_RVOL_INSERT.contains("d.id_registro_obra IS NOT NULL"));
    }

    #[test]
    fn deshacer_reactiva_obras_desactivadas_por_el_lote() {
        assert!(SQL_REACTIVAR_OBRAS_DEL_LOTE.contains("activa = true"));
        assert!(SQL_REACTIVAR_OBRAS_DEL_LOTE.contains("lote_id = $1"));
    }

    #[test]
    fn deshacer_restaura_desde_tramos_de_la_foto() {
        assert!(SQL_RESTAURAR_TRAMOS.contains("jsonb_array_elements($2::text::jsonb -> 'tramos')"));
        assert!(SQL_RESTAURAR_TRAMOS.contains("dl.subtotal"));
        assert!(SQL_RESTAURAR_TRAMOS.contains("(t->>'id_registro_obra')::uuid"));
        assert!(SQL_RESTAURAR_MARCAS.contains("'reporte' ->> 'editado_usuario'"));
        assert!(SQL_RESTAURAR_MARCAS.contains("'reporte' ->> 'cedula_actualizada'"));
    }

    /// Prueba de integración con BD (no se ejecuta por defecto). Usa ROLLBACK: no deja cambios.
    #[test]
    #[ignore]
    fn bitacora_deshacer_lote_tx_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(_) => return,
        };
        let lote: Option<Uuid> = client
            .query(
                "SELECT lote_id FROM public.reportes_cambios \
                 WHERE id_reporte_liquidacion IS NOT NULL LIMIT 1",
                &[],
            )
            .expect("Consulta de lotes")
            .first()
            .map(|r| r.get(0));
        let Some(lote) = lote else {
            return;
        };
        let mut tx = client.transaction().expect("No se pudo iniciar la transacción");
        let restaurados = deshacer_lote_tx(&mut tx, lote).expect("deshacer_lote_tx");
        assert!(restaurados >= 1);
        tx.rollback().expect("rollback de la prueba");
    }

    /// Prueba de solo lectura con BD (no se ejecuta por defecto).
    #[test]
    #[ignore]
    fn bitacora_ultimo_lote_revision_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(_) => return,
        };
        let revision: Option<Uuid> = client
            .query("SELECT id_revision FROM public.reportes_cambios LIMIT 1", &[])
            .expect("Consulta de revisiones")
            .first()
            .map(|r| r.get(0));
        let Some(revision) = revision else {
            return;
        };
        let ultimo = ultimo_lote_revision(&mut client, revision).expect("ultimo_lote_revision");
        if let Some((_, accion)) = ultimo {
            assert_ne!(accion, ACCION_DESHACER);
        }
    }

    /// ADVERTENCIA: esta prueba abre una transacción y la CONFIRMA (commit) en la BD configurada
    /// (producción). Usa un lote inexistente, por lo que no debería escribir filas; ejecutar solo
    /// explícitamente con --ignored.
    #[test]
    #[ignore]
    fn bitacora_deshacer_lote_publico_lote_inexistente_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(_) => return,
        };
        let restaurados = deshacer_lote(&mut client, Uuid::nil()).expect("deshacer_lote");
        assert_eq!(restaurados, 0);
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
