//! Depuración de duplicados previa a las restricciones UNIQUE (Fase 1, tarea 1.1).
//!
//! Orden fijo dentro de UNA sola transacción:
//! - R1 empleados: duplicados por (id_cliente, nss). Sobrevive el más antiguo; el sobreviviente
//!   completa sus campos vacíos y todas las FKs que apuntan a empleados se reapuntan.
//! - R2 liquidaciones: duplicados por (id_registro_patronal, id_empleado, anio, mes).
//!   Sobrevive la más reciente; hereda vinculaciones de obras si no tiene y se reapuntan reportes.
//! - R3 reportes: duplicados por (id_revision, id_empleado, anio, mes). Sobrevive el que tiene
//!   más vinculaciones de obra en su detalle.
//!
//! Antes de borrar cualquier fila se escribe su respaldo JSON en `public.respaldo_depuracion`.
//! `dry_run = true` revierte la transacción al final. Si quedan duplicados, se aborta sin commit.

use postgres::Client;

use crate::db::schema::format_pg_error;

/// Resumen de la depuración: cuántas filas se afectaron en cada paso.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReporteDepuracion {
    pub dry_run: bool,
    pub empleados_fusionados: u64,
    pub filas_fk_reapuntadas: u64,
    pub liquidaciones_eliminadas: u64,
    pub vinculaciones_heredadas: u64,
    pub reportes_reapuntados: u64,
    pub reportes_eliminados: u64,
    pub respaldos_creados: u64,
    pub duplicados_restantes_empleados: i64,
    pub duplicados_restantes_liquidaciones: i64,
    pub duplicados_restantes_reportes: i64,
}

/// Verifica que existan la tabla de respaldo y la columna `cedula_actualizada`.
const SQL_PRECHECK: &str = "SELECT \
    EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'respaldo_depuracion'), \
    EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' \
            AND table_name = 'reportes_liquidaciones' AND column_name = 'cedula_actualizada')";

/// Bloqueo exclusivo (solo lectura permitida) mientras dura la depuración.
const SQL_LOCK: &str = "LOCK TABLE public.empleados, public.liquidaciones, \
    public.vinculaciones_obras_liquidaciones, public.reportes_liquidaciones, \
    public.respaldo_depuracion IN EXCLUSIVE MODE";

/// Restricciones únicas que se quitan durante la depuración y se recrean antes del commit.
const SQL_SOLTAR_RESTRICCIONES: &str = "\
    ALTER TABLE public.reportes_liquidaciones DROP CONSTRAINT IF EXISTS uq_rl_rev_emp_periodo; \
    ALTER TABLE public.liquidaciones DROP CONSTRAINT IF EXISTS uq_liquidaciones_rp_emp_periodo; \
    DROP INDEX IF EXISTS public.uq_empleados_cliente_nss;";

/// Descubre todas las columnas con FK que referencian `public.empleados(id)`.
const SQL_FK_EMPLEADOS: &str = "SELECT c.conrelid::regclass::text, quote_ident(a.attname) \
    FROM pg_constraint c \
    JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY(c.conkey) \
    WHERE c.contype = 'f' AND c.confrelid = 'public.empleados'::regclass \
    ORDER BY 1, 2";

/// R1: mapa eliminado -> sobreviviente de empleados (solo nss no vacío).
const SQL_R1_MAPA: &str = "CREATE TEMP TABLE tmp_map_emp ON COMMIT DROP AS \
    WITH ord AS ( \
        SELECT id, \
               ROW_NUMBER() OVER w AS rn, \
               FIRST_VALUE(id) OVER w AS sobreviviente \
        FROM public.empleados \
        WHERE nss IS NOT NULL AND BTRIM(nss) <> '' \
        WINDOW w AS (PARTITION BY id_cliente, BTRIM(nss) ORDER BY created_at ASC, id ASC) \
    ) \
    SELECT id AS eliminado, sobreviviente FROM ord WHERE rn > 1";

/// R1 respaldo: fila completa de cada empleado que será eliminado.
const SQL_RESPALDO_EMPLEADOS: &str = "INSERT INTO public.respaldo_depuracion \
    (proceso, tabla, id_registro, id_sobreviviente, datos) \
    SELECT 'fase1_depuracion', 'empleados', e.id, m.sobreviviente, to_jsonb(e.*) \
    FROM tmp_map_emp m JOIN public.empleados e ON e.id = m.eliminado";

/// R1: completa en el sobreviviente los campos vacíos con el primer valor no vacío de los eliminados.
const SQL_R1_FUSIONAR_CAMPOS: &str = "UPDATE public.empleados s SET \
    nombre = COALESCE(NULLIF(BTRIM(s.nombre), ''), c.nombre), \
    rfc = COALESCE(NULLIF(BTRIM(s.rfc), ''), c.rfc), \
    curp = COALESCE(NULLIF(BTRIM(s.curp), ''), c.curp), \
    clave_ubicacion = COALESCE(NULLIF(BTRIM(s.clave_ubicacion), ''), c.clave_ubicacion), \
    pensionado = COALESCE(NULLIF(BTRIM(s.pensionado), ''), c.pensionado) \
    FROM ( \
        SELECT m.sobreviviente AS id, \
            (ARRAY_AGG(NULLIF(BTRIM(e.nombre), '') ORDER BY e.created_at ASC, e.id ASC) \
                FILTER (WHERE NULLIF(BTRIM(e.nombre), '') IS NOT NULL))[1] AS nombre, \
            (ARRAY_AGG(NULLIF(BTRIM(e.rfc), '') ORDER BY e.created_at ASC, e.id ASC) \
                FILTER (WHERE NULLIF(BTRIM(e.rfc), '') IS NOT NULL))[1] AS rfc, \
            (ARRAY_AGG(NULLIF(BTRIM(e.curp), '') ORDER BY e.created_at ASC, e.id ASC) \
                FILTER (WHERE NULLIF(BTRIM(e.curp), '') IS NOT NULL))[1] AS curp, \
            (ARRAY_AGG(NULLIF(BTRIM(e.clave_ubicacion), '') ORDER BY e.created_at ASC, e.id ASC) \
                FILTER (WHERE NULLIF(BTRIM(e.clave_ubicacion), '') IS NOT NULL))[1] AS clave_ubicacion, \
            (ARRAY_AGG(NULLIF(BTRIM(e.pensionado), '') ORDER BY e.created_at ASC, e.id ASC) \
                FILTER (WHERE NULLIF(BTRIM(e.pensionado), '') IS NOT NULL))[1] AS pensionado \
        FROM tmp_map_emp m JOIN public.empleados e ON e.id = m.eliminado \
        GROUP BY m.sobreviviente \
    ) c \
    WHERE s.id = c.id";

/// R1: elimina los empleados duplicados (tras reapuntar todas sus FKs).
const SQL_R1_ELIMINAR: &str = "DELETE FROM public.empleados e USING tmp_map_emp m WHERE e.id = m.eliminado";

/// R2: mapa eliminado -> sobreviviente de liquidaciones (claves no nulas; la más reciente sobrevive).
const SQL_R2_MAPA: &str = "CREATE TEMP TABLE tmp_map_liq ON COMMIT DROP AS \
    WITH ord AS ( \
        SELECT id, \
               ROW_NUMBER() OVER w AS rn, \
               FIRST_VALUE(id) OVER w AS sobreviviente \
        FROM public.liquidaciones \
        WHERE id_registro_patronal IS NOT NULL AND id_empleado IS NOT NULL \
          AND anio_periodo IS NOT NULL AND mes_periodo_int IS NOT NULL \
        WINDOW w AS (PARTITION BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int \
                     ORDER BY created_at DESC NULLS LAST, id DESC) \
    ) \
    SELECT id AS eliminado, sobreviviente FROM ord WHERE rn > 1";

/// R2 respaldo: liquidación con sus detalles y vinculaciones de obra.
const SQL_RESPALDO_LIQUIDACIONES: &str = "INSERT INTO public.respaldo_depuracion \
    (proceso, tabla, id_registro, id_sobreviviente, datos) \
    SELECT 'fase1_depuracion', 'liquidaciones', l.id, m.sobreviviente, \
        jsonb_build_object( \
            'liquidacion', to_jsonb(l.*), \
            'detalles', COALESCE((SELECT jsonb_agg(to_jsonb(d.*)) FROM public.detalles_liquidaciones d \
                                  WHERE d.id_liquidacion = l.id), '[]'::jsonb), \
            'vinculaciones', COALESCE((SELECT jsonb_agg(to_jsonb(v.*)) \
                                       FROM public.vinculaciones_obras_liquidaciones v \
                                       WHERE v.id_liquidacion = l.id), '[]'::jsonb)) \
    FROM tmp_map_liq m JOIN public.liquidaciones l ON l.id = m.eliminado";

/// R2: el sobreviviente hereda las vinculaciones de obra solo si no tiene ninguna.
const SQL_R2_HEREDAR_VINCULACIONES: &str = "INSERT INTO public.vinculaciones_obras_liquidaciones \
    (id_registro_obra, id_liquidacion, dias_registro_obra) \
    SELECT v.id_registro_obra, m.sobreviviente, v.dias_registro_obra \
    FROM tmp_map_liq m \
    JOIN public.vinculaciones_obras_liquidaciones v ON v.id_liquidacion = m.eliminado \
    WHERE NOT EXISTS (SELECT 1 FROM public.vinculaciones_obras_liquidaciones s \
                      WHERE s.id_liquidacion = m.sobreviviente) \
    ON CONFLICT (id_liquidacion, id_registro_obra) DO NOTHING";

/// R2: reapunta los reportes de liquidaciones eliminadas al sobreviviente y marca cédula actualizada.
const SQL_R2_REAPUNTAR_REPORTES: &str = "UPDATE public.reportes_liquidaciones r \
    SET id_liquidacion = m.sobreviviente, cedula_actualizada = true \
    FROM tmp_map_liq m WHERE r.id_liquidacion = m.eliminado";

/// R2: elimina liquidaciones duplicadas (CASCADE borra sus detalles y vinculaciones).
const SQL_R2_ELIMINAR: &str = "DELETE FROM public.liquidaciones l USING tmp_map_liq m WHERE l.id = m.eliminado";

/// R3: mapa eliminado -> sobreviviente de reportes. Gana el que tiene más detalles con obra asignada.
const SQL_R3_MAPA: &str = "CREATE TEMP TABLE tmp_map_rep ON COMMIT DROP AS \
    WITH cand AS ( \
        SELECT r.id, r.id_revision, r.id_empleado, r.anio_periodo, r.mes_periodo_int, r.created_at, \
               (SELECT COUNT(*) FROM public.reportes_detalles_liquidaciones d \
                WHERE d.id_reporte_liquidacion = r.id AND d.id_registro_obra IS NOT NULL) AS n_obras \
        FROM public.reportes_liquidaciones r \
        WHERE r.id_revision IS NOT NULL AND r.id_empleado IS NOT NULL \
          AND r.anio_periodo IS NOT NULL AND r.mes_periodo_int IS NOT NULL \
    ), \
    ord AS ( \
        SELECT id, \
               ROW_NUMBER() OVER w AS rn, \
               FIRST_VALUE(id) OVER w AS sobreviviente \
        FROM cand \
        WINDOW w AS (PARTITION BY id_revision, id_empleado, anio_periodo, mes_periodo_int \
                     ORDER BY n_obras DESC, created_at ASC NULLS LAST, id ASC) \
    ) \
    SELECT id AS eliminado, sobreviviente FROM ord WHERE rn > 1";

/// R3 respaldo: reporte con sus detalles y vinculaciones de obra.
const SQL_RESPALDO_REPORTES: &str = "INSERT INTO public.respaldo_depuracion \
    (proceso, tabla, id_registro, id_sobreviviente, datos) \
    SELECT 'fase1_depuracion', 'reportes_liquidaciones', r.id, m.sobreviviente, \
        jsonb_build_object( \
            'reporte', to_jsonb(r.*), \
            'reportes_detalles', COALESCE((SELECT jsonb_agg(to_jsonb(d.*)) \
                                           FROM public.reportes_detalles_liquidaciones d \
                                           WHERE d.id_reporte_liquidacion = r.id), '[]'::jsonb), \
            'reportes_vinculaciones', COALESCE((SELECT jsonb_agg(to_jsonb(v.*)) \
                                                FROM public.reportes_vinculaciones_obras_liquidaciones v \
                                                WHERE v.id_reporte_liquidacion = r.id), '[]'::jsonb)) \
    FROM tmp_map_rep m JOIN public.reportes_liquidaciones r ON r.id = m.eliminado";

/// R3: elimina reportes duplicados (CASCADE borra sus detalles y vinculaciones).
const SQL_R3_ELIMINAR: &str = "DELETE FROM public.reportes_liquidaciones r USING tmp_map_rep m WHERE r.id = m.eliminado";

/// Conteo de filas sobrantes (suma de cada grupo duplicado menos uno) para empleados.
const SQL_DUP_EMPLEADOS: &str = "SELECT COALESCE(SUM(c - 1), 0)::bigint FROM ( \
    SELECT COUNT(*) AS c FROM public.empleados \
    WHERE nss IS NOT NULL AND BTRIM(nss) <> '' \
    GROUP BY id_cliente, BTRIM(nss) HAVING COUNT(*) > 1) g";

/// Conteo de filas sobrantes para liquidaciones.
const SQL_DUP_LIQUIDACIONES: &str = "SELECT COALESCE(SUM(c - 1), 0)::bigint FROM ( \
    SELECT COUNT(*) AS c FROM public.liquidaciones \
    WHERE id_registro_patronal IS NOT NULL AND id_empleado IS NOT NULL \
      AND anio_periodo IS NOT NULL AND mes_periodo_int IS NOT NULL \
    GROUP BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int HAVING COUNT(*) > 1) g";

/// Conteo de filas sobrantes para reportes de liquidaciones.
const SQL_DUP_REPORTES: &str = "SELECT COALESCE(SUM(c - 1), 0)::bigint FROM ( \
    SELECT COUNT(*) AS c FROM public.reportes_liquidaciones \
    WHERE id_revision IS NOT NULL AND id_empleado IS NOT NULL \
      AND anio_periodo IS NOT NULL AND mes_periodo_int IS NOT NULL \
    GROUP BY id_revision, id_empleado, anio_periodo, mes_periodo_int HAVING COUNT(*) > 1) g";

/// Convierte un error de PostgreSQL en texto con el paso donde ocurrió.
fn err_pg(paso: &str, e: postgres::Error) -> String {
    format!("{paso}: {}", format_pg_error(&e))
}

/// Ejecuta la depuración de duplicados de Fase 1 en una sola transacción.
///
/// Con `dry_run = true` la transacción se revierte al final. Si al terminar quedan duplicados,
/// devuelve `Err` y la transacción no se confirma.
pub fn depurar_duplicados_fase1(client: &mut Client, dry_run: bool) -> Result<ReporteDepuracion, String> {
    let mut tx = client
        .transaction()
        .map_err(|e| err_pg("iniciar transacción", e))?;

    let precheck = tx
        .query_one(SQL_PRECHECK, &[])
        .map_err(|e| err_pg("precondiciones", e))?;
    let tabla_respaldo: bool = precheck.get(0);
    let columna_cedula: bool = precheck.get(1);
    if !tabla_respaldo || !columna_cedula {
        return Err("Ejecute ensure_schema antes de la depuración".to_string());
    }

    tx.execute(SQL_LOCK, &[])
        .map_err(|e| err_pg("bloqueo de tablas", e))?;

    // Las restricciones únicas pueden existir ya (p. ej. uq_rl_rev_emp_periodo se crea si no había
    // duplicados de reportes). R1 reapunta id_empleado y generaría colisiones transitorias que R3
    // resuelve después, por eso se quitan aquí y se recrean al final DENTRO de esta transacción
    // (DDL transaccional: si algo falla, el rollback las restaura).
    tx.batch_execute(SQL_SOLTAR_RESTRICCIONES)
        .map_err(|e| err_pg("soltar restricciones únicas temporalmente", e))?;

    let mut r = ReporteDepuracion {
        dry_run,
        ..Default::default()
    };

    // R1: empleados. Respaldo y fusión de campos antes de reapuntar FKs y borrar.
    tx.execute(SQL_R1_MAPA, &[])
        .map_err(|e| err_pg("R1 mapa empleados", e))?;
    r.respaldos_creados += tx
        .execute(SQL_RESPALDO_EMPLEADOS, &[])
        .map_err(|e| err_pg("R1 respaldo empleados", e))?;
    tx.execute(SQL_R1_FUSIONAR_CAMPOS, &[])
        .map_err(|e| err_pg("R1 fusión de campos", e))?;

    let fks = tx
        .query(SQL_FK_EMPLEADOS, &[])
        .map_err(|e| err_pg("R1 descubrir FKs de empleados", e))?;
    for fila in &fks {
        let tabla: String = fila.get(0);
        let columna: String = fila.get(1);
        // `tabla` viene de regclass::text (ya calificada y entrecomillada si hace falta);
        // `columna` viene de quote_ident.
        let sql = format!(
            "UPDATE {tabla} AS t SET {columna} = m.sobreviviente \
             FROM tmp_map_emp m WHERE t.{columna} = m.eliminado"
        );
        r.filas_fk_reapuntadas += tx
            .execute(sql.as_str(), &[])
            .map_err(|e| err_pg(&format!("R1 reapuntar {tabla}.{columna}"), e))?;
    }

    r.empleados_fusionados = tx
        .execute(SQL_R1_ELIMINAR, &[])
        .map_err(|e| err_pg("R1 eliminar empleados", e))?;

    // R2: liquidaciones (sobre el estado ya fusionado de empleados).
    tx.execute(SQL_R2_MAPA, &[])
        .map_err(|e| err_pg("R2 mapa liquidaciones", e))?;
    r.respaldos_creados += tx
        .execute(SQL_RESPALDO_LIQUIDACIONES, &[])
        .map_err(|e| err_pg("R2 respaldo liquidaciones", e))?;
    r.vinculaciones_heredadas = tx
        .execute(SQL_R2_HEREDAR_VINCULACIONES, &[])
        .map_err(|e| err_pg("R2 heredar vinculaciones", e))?;
    r.reportes_reapuntados = tx
        .execute(SQL_R2_REAPUNTAR_REPORTES, &[])
        .map_err(|e| err_pg("R2 reapuntar reportes", e))?;
    r.liquidaciones_eliminadas = tx
        .execute(SQL_R2_ELIMINAR, &[])
        .map_err(|e| err_pg("R2 eliminar liquidaciones", e))?;

    // R3: reportes (pueden surgir colisiones tras la fusión de empleados).
    tx.execute(SQL_R3_MAPA, &[])
        .map_err(|e| err_pg("R3 mapa reportes", e))?;
    r.respaldos_creados += tx
        .execute(SQL_RESPALDO_REPORTES, &[])
        .map_err(|e| err_pg("R3 respaldo reportes", e))?;
    r.reportes_eliminados = tx
        .execute(SQL_R3_ELIMINAR, &[])
        .map_err(|e| err_pg("R3 eliminar reportes", e))?;

    // Verificación final dentro de la misma transacción.
    r.duplicados_restantes_empleados = tx
        .query_one(SQL_DUP_EMPLEADOS, &[])
        .map_err(|e| err_pg("verificar duplicados empleados", e))?
        .get(0);
    r.duplicados_restantes_liquidaciones = tx
        .query_one(SQL_DUP_LIQUIDACIONES, &[])
        .map_err(|e| err_pg("verificar duplicados liquidaciones", e))?
        .get(0);
    r.duplicados_restantes_reportes = tx
        .query_one(SQL_DUP_REPORTES, &[])
        .map_err(|e| err_pg("verificar duplicados reportes", e))?
        .get(0);

    if r.duplicados_restantes_empleados != 0
        || r.duplicados_restantes_liquidaciones != 0
        || r.duplicados_restantes_reportes != 0
    {
        return Err(format!(
            "Quedan duplicados tras la depuración (empleados={}, liquidaciones={}, reportes={}); transacción revertida",
            r.duplicados_restantes_empleados,
            r.duplicados_restantes_liquidaciones,
            r.duplicados_restantes_reportes
        ));
    }

    // Recrear las restricciones con el SQL con guarda de ensure_schema y exigir que existan todas.
    tx.batch_execute(crate::db::schema::SQL_FASE1_RESTRICCIONES)
        .map_err(|e| err_pg("recrear restricciones de integridad", e))?;
    let pendientes = crate::db::schema::restricciones_fase1_pendientes(&mut tx)
        .map_err(|e| err_pg("verificar restricciones de integridad", e))?;
    if !pendientes.is_empty() {
        return Err(format!(
            "No se pudieron crear las restricciones: {}; transacción revertida",
            pendientes.join(", ")
        ));
    }

    if dry_run {
        tx.rollback().map_err(|e| err_pg("rollback (dry_run)", e))?;
    } else {
        tx.commit().map_err(|e| err_pg("commit", e))?;
    }
    Ok(r)
}

/// Restaura desde `revision_history` tramos de `reportes_detalles_liquidaciones` borrados (evento DELETE),
/// copiando solo las columnas comunes a ambas tablas. Omite tramos cuyo reporte padre ya no existe o que
/// ya están presentes. Reconstruye las vinculaciones legadas (rvol) de los reportes afectados desde rdl.
/// Devuelve (restaurados, omitidos).
pub fn restaurar_tramos_rdl_desde_historial(client: &mut Client, ids: &[uuid::Uuid]) -> Result<(u64, u64), String> {
    let mut tx = client.transaction().map_err(|e| err_pg("iniciar transacción", e))?;
    // Columnas comunes (el historial agrega modified_at/event_name y puede carecer de columnas nuevas)
    let cols: Vec<String> = tx
        .query(
            "SELECT quote_ident(p.column_name) FROM information_schema.columns p \
             JOIN information_schema.columns h ON h.table_schema = 'revision_history' \
              AND h.table_name = p.table_name AND h.column_name = p.column_name \
             WHERE p.table_schema = 'public' AND p.table_name = 'reportes_detalles_liquidaciones' \
             ORDER BY p.ordinal_position",
            &[],
        )
        .map_err(|e| err_pg("columnas comunes rdl", e))?
        .iter()
        .map(|r| r.get::<_, String>(0))
        .collect();
    if cols.is_empty() {
        return Err("No hay columnas comunes entre public y revision_history para reportes_detalles_liquidaciones".into());
    }
    let lista = cols.join(", ");
    let sql = format!(
        "INSERT INTO public.reportes_detalles_liquidaciones ({lista}) \
         SELECT {lista} FROM ( \
             SELECT DISTINCT ON (h.id) h.* FROM revision_history.reportes_detalles_liquidaciones h \
             WHERE h.id = ANY($1) AND h.event_name = 'DELETE' ORDER BY h.id, h.modified_at DESC \
         ) h \
         WHERE EXISTS (SELECT 1 FROM public.reportes_liquidaciones rl WHERE rl.id = h.id_reporte_liquidacion) \
           AND NOT EXISTS (SELECT 1 FROM public.reportes_detalles_liquidaciones x WHERE x.id = h.id)"
    );
    let restaurados = tx.execute(sql.as_str(), &[&ids]).map_err(|e| err_pg("restaurar tramos", e))?;
    // Mantener coherente la tabla legada de vinculaciones de reportes para los padres afectados
    tx.execute(
        "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones v \
         WHERE v.id_reporte_liquidacion IN (SELECT id_reporte_liquidacion FROM public.reportes_detalles_liquidaciones WHERE id = ANY($1))",
        &[&ids],
    )
    .map_err(|e| err_pg("limpiar rvol de padres afectados", e))?;
    tx.execute(
        "INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (id_registro_obra, id_reporte_liquidacion, dias_registro_obra) \
         SELECT d.id_registro_obra, d.id_reporte_liquidacion, LEAST(SUM(COALESCE(d.dias_registro_obra, 0)), 31)::smallint \
         FROM public.reportes_detalles_liquidaciones d \
         WHERE d.id_registro_obra IS NOT NULL AND d.id_reporte_liquidacion IN \
               (SELECT id_reporte_liquidacion FROM public.reportes_detalles_liquidaciones WHERE id = ANY($1)) \
         GROUP BY d.id_registro_obra, d.id_reporte_liquidacion",
        &[&ids],
    )
    .map_err(|e| err_pg("reconstruir rvol de padres afectados", e))?;
    tx.commit().map_err(|e| err_pg("commit restauración", e))?;
    Ok((restaurados, ids.len() as u64 - restaurados))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Restauración puntual (2026-11-12) de 2 tramos borrados por la limpieza automática de arranque.
    /// Solo corre con RESTAURAR_TRAMOS_ARRANQUE=SI: `cargo test restaurar_tramos_limpieza_arranque_bd -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn restaurar_tramos_limpieza_arranque_bd() {
        if std::env::var("RESTAURAR_TRAMOS_ARRANQUE").as_deref() != Ok("SI") {
            println!("Restauración omitida: defina RESTAURAR_TRAMOS_ARRANQUE=SI");
            return;
        }
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("Sin conexión a BD: {}", e);
                return;
            }
        };
        let ids = [
            uuid::Uuid::parse_str("01a1ce0a-65d8-701d-81d9-dfe079dbf37e").expect("uuid"),
            uuid::Uuid::parse_str("01a1ce0a-6252-700d-b75b-9a9c3abf0796").expect("uuid"),
        ];
        let (rest, omit) = restaurar_tramos_rdl_desde_historial(&mut client, &ids).expect("restauración");
        println!("RESTAURACION restaurados={} omitidos={}", rest, omit);
    }

    #[test]
    fn respaldo_precede_a_los_borrados() {
        // Cada constante de respaldo debe ser un INSERT al respaldo y contener sus hijas.
        assert!(SQL_RESPALDO_EMPLEADOS.starts_with("INSERT INTO public.respaldo_depuracion"));
        assert!(SQL_RESPALDO_EMPLEADOS.contains("to_jsonb(e.*)"));

        assert!(SQL_RESPALDO_LIQUIDACIONES.starts_with("INSERT INTO public.respaldo_depuracion"));
        assert!(SQL_RESPALDO_LIQUIDACIONES.contains("'detalles'"));
        assert!(SQL_RESPALDO_LIQUIDACIONES.contains("'vinculaciones'"));

        assert!(SQL_RESPALDO_REPORTES.starts_with("INSERT INTO public.respaldo_depuracion"));
        assert!(SQL_RESPALDO_REPORTES.contains("'reportes_detalles'"));
        assert!(SQL_RESPALDO_REPORTES.contains("'reportes_vinculaciones'"));
    }

    #[test]
    fn sql_de_borrado_solo_tras_respaldo_del_mismo_mapa() {
        // Los borrados usan los mismos mapas temporales que alimentan sus respaldos.
        assert!(SQL_RESPALDO_EMPLEADOS.contains("tmp_map_emp"));
        assert!(SQL_R1_ELIMINAR.contains("tmp_map_emp"));
        assert!(SQL_RESPALDO_LIQUIDACIONES.contains("tmp_map_liq"));
        assert!(SQL_R2_ELIMINAR.contains("tmp_map_liq"));
        assert!(SQL_RESPALDO_REPORTES.contains("tmp_map_rep"));
        assert!(SQL_R3_ELIMINAR.contains("tmp_map_rep"));
    }

    #[test]
    fn descubrimiento_fk_apunta_a_empleados() {
        assert!(SQL_FK_EMPLEADOS.contains("'public.empleados'::regclass"));
        assert!(SQL_FK_EMPLEADOS.contains("quote_ident"));
    }

    #[test]
    #[ignore]
    fn depuracion_fase1_dry_run_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("BD no disponible, prueba omitida: {e}");
                return;
            }
        };
        crate::db::schema::ensure_schema(&mut client).expect("ensure_schema");
        let r = depurar_duplicados_fase1(&mut client, true).expect("depuración dry_run");
        println!("DEPURACION {:#?}", r);
        assert_eq!(r.duplicados_restantes_empleados, 0);
        assert_eq!(r.duplicados_restantes_liquidaciones, 0);
        assert_eq!(r.duplicados_restantes_reportes, 0);
    }

    #[test]
    #[ignore]
    fn depuracion_fase1_ejecutar_bd() {
        if std::env::var("CONFIRMAR_DEPURACION_FASE1").as_deref() != Ok("SI") {
            println!("Omitido: defina CONFIRMAR_DEPURACION_FASE1=SI para ejecutar la depuración real");
            return;
        }
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("BD no disponible, prueba omitida: {e}");
                return;
            }
        };
        crate::db::schema::ensure_schema(&mut client).expect("ensure_schema previo");
        let r = depurar_duplicados_fase1(&mut client, false).expect("depuración real");
        println!("DEPURACION {:#?}", r);
        crate::db::schema::ensure_schema(&mut client).expect("ensure_schema posterior");
        match crate::db::schema::restricciones_fase1_pendientes(&mut client) {
            Ok(p) => println!("RESTRICCIONES PENDIENTES {:?}", p),
            Err(e) => println!("No se pudo consultar restricciones pendientes: {e}"),
        }
    }
}
