//! Diagnóstico de integridad (solo lectura) de las tablas de cédulas y reportes de liquidaciones.
//!
//! Se usa antes de la migración de restricciones UNIQUE (Fase 1) para medir el impacto, y después
//! como criterio de aceptación: todos los conteos de duplicados deben quedar en 0.
//! Toda la lectura ocurre dentro de una transacción `READ ONLY`: PostgreSQL rechaza cualquier escritura.

use postgres::Client;

/// Un indicador del diagnóstico: nombre legible y valor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indicador {
    pub nombre: &'static str,
    pub valor: i64,
}

/// Consultas escalares del diagnóstico. Cada una devuelve un único `bigint`.
const CONSULTAS: &[(&str, &str)] = &[
    ("empleados_total", "SELECT COUNT(*)::bigint FROM public.empleados"),
    (
        "empleados_dup_nss_grupos",
        "SELECT COUNT(*)::bigint FROM (SELECT 1 FROM public.empleados \
         WHERE nss IS NOT NULL AND BTRIM(nss) <> '' \
         GROUP BY id_cliente, BTRIM(nss) HAVING COUNT(*) > 1) g",
    ),
    (
        "empleados_dup_nss_filas_sobrantes",
        "SELECT COALESCE(SUM(c - 1), 0)::bigint FROM (SELECT COUNT(*) c FROM public.empleados \
         WHERE nss IS NOT NULL AND BTRIM(nss) <> '' \
         GROUP BY id_cliente, BTRIM(nss) HAVING COUNT(*) > 1) g",
    ),
    (
        "empleados_dup_curp_grupos",
        "SELECT COUNT(*)::bigint FROM (SELECT 1 FROM public.empleados \
         WHERE curp IS NOT NULL AND BTRIM(curp) <> '' AND BTRIM(curp) <> 'XEXX010101HNEXXXA4' \
         GROUP BY id_cliente, BTRIM(curp) HAVING COUNT(*) > 1) g",
    ),
    (
        "empleados_mismo_nss_distinto_nombre",
        "SELECT COUNT(*)::bigint FROM (SELECT 1 FROM public.empleados \
         WHERE nss IS NOT NULL AND BTRIM(nss) <> '' \
         GROUP BY id_cliente, BTRIM(nss) HAVING COUNT(DISTINCT UPPER(BTRIM(nombre))) > 1) g",
    ),
    ("liquidaciones_total", "SELECT COUNT(*)::bigint FROM public.liquidaciones"),
    (
        "liquidaciones_mes_invalido",
        "SELECT COUNT(*)::bigint FROM public.liquidaciones \
         WHERE mes_periodo_int IS NULL OR mes_periodo_int NOT BETWEEN 1 AND 12",
    ),
    (
        "liquidaciones_dup_grupos",
        "SELECT COUNT(*)::bigint FROM (SELECT 1 FROM public.liquidaciones \
         GROUP BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int HAVING COUNT(*) > 1) g",
    ),
    (
        "liquidaciones_dup_filas_sobrantes",
        "SELECT COALESCE(SUM(c - 1), 0)::bigint FROM (SELECT COUNT(*) c FROM public.liquidaciones \
         GROUP BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int HAVING COUNT(*) > 1) g",
    ),
    (
        // Duplicados que aparecerán al fusionar empleados por NSS (colisiones post-fusión)
        "liquidaciones_dup_tras_fusion_nss",
        "WITH e AS (SELECT id, FIRST_VALUE(id) OVER (PARTITION BY id_cliente, BTRIM(nss) ORDER BY created_at, id) keep_id \
                    FROM public.empleados WHERE nss IS NOT NULL AND BTRIM(nss) <> '') \
         SELECT COUNT(*)::bigint FROM (SELECT 1 FROM public.liquidaciones l LEFT JOIN e ON e.id = l.id_empleado \
         GROUP BY l.id_registro_patronal, COALESCE(e.keep_id, l.id_empleado), l.anio_periodo, l.mes_periodo_int \
         HAVING COUNT(*) > 1) g",
    ),
    ("reportes_liquidaciones_total", "SELECT COUNT(*)::bigint FROM public.reportes_liquidaciones"),
    (
        "reportes_liquidaciones_dup_grupos",
        "SELECT COUNT(*)::bigint FROM (SELECT 1 FROM public.reportes_liquidaciones \
         GROUP BY id_revision, id_empleado, anio_periodo, mes_periodo_int HAVING COUNT(*) > 1) g",
    ),
    (
        "reportes_liquidaciones_mes_invalido",
        "SELECT COUNT(*)::bigint FROM public.reportes_liquidaciones \
         WHERE mes_periodo_int IS NULL OR mes_periodo_int NOT BETWEEN 1 AND 12",
    ),
    ("reportes_detalles_total", "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones"),
    (
        "reportes_detalles_con_obra",
        "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones WHERE id_registro_obra IS NOT NULL",
    ),
    (
        "rvol_total",
        "SELECT COUNT(*)::bigint FROM public.reportes_vinculaciones_obras_liquidaciones",
    ),
    (
        // Vínculos en rvol sin tramo equivalente en rdl: se perderían al convertir rvol en vista
        "rvol_sin_tramo_en_rdl",
        "SELECT COUNT(*)::bigint FROM public.reportes_vinculaciones_obras_liquidaciones rvol \
         WHERE NOT EXISTS (SELECT 1 FROM public.reportes_detalles_liquidaciones rdl \
                           WHERE rdl.id_reporte_liquidacion = rvol.id_reporte_liquidacion \
                             AND rdl.id_registro_obra = rvol.id_registro_obra)",
    ),
    (
        // Grupos duplicados cuyas liquidaciones tienen exactamente el mismo contenido de detalle
        "liquidaciones_dup_grupos_detalle_identico",
        "WITH firma AS (SELECT l.id, l.id_registro_patronal, l.id_empleado, l.anio_periodo, l.mes_periodo_int, \
                COALESCE((SELECT md5(string_agg(concat_ws('|', BTRIM(d.clave), d.fecha, d.dias, round(d.sdi::numeric,2), \
                    round(d.subtotal::numeric,2), round(d.cf::numeric,2), round(d.gps::numeric,2)), ';' \
                    ORDER BY d.fecha NULLS FIRST, BTRIM(d.clave), d.dias)) FROM public.detalles_liquidaciones d \
                    WHERE d.id_liquidacion = l.id), 'SIN_DETALLE') f FROM public.liquidaciones l) \
         SELECT COUNT(*)::bigint FROM (SELECT 1 FROM firma GROUP BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int \
         HAVING COUNT(*) > 1 AND COUNT(DISTINCT f) = 1) g",
    ),
    (
        // Liquidaciones duplicadas (no la más antigua del grupo) referenciadas por reportes_liquidaciones
        "liquidaciones_dup_no_primera_con_reporte",
        "WITH r AS (SELECT id, ROW_NUMBER() OVER (PARTITION BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int \
                    ORDER BY created_at, id) rn FROM public.liquidaciones) \
         SELECT COUNT(*)::bigint FROM r WHERE rn > 1 AND EXISTS (SELECT 1 FROM public.reportes_liquidaciones rl WHERE rl.id_liquidacion = r.id)",
    ),
    (
        // Liquidaciones duplicadas (no la más antigua) con vinculaciones de obra propias
        "liquidaciones_dup_no_primera_con_vinculacion",
        "WITH r AS (SELECT id, ROW_NUMBER() OVER (PARTITION BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int \
                    ORDER BY created_at, id) rn FROM public.liquidaciones) \
         SELECT COUNT(*)::bigint FROM r WHERE rn > 1 AND EXISTS (SELECT 1 FROM public.vinculaciones_obras_liquidaciones v WHERE v.id_liquidacion = r.id)",
    ),
    (
        "liquidaciones_dup_distinto_nombre_archivo",
        "SELECT COUNT(*)::bigint FROM (SELECT 1 FROM public.liquidaciones \
         GROUP BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int \
         HAVING COUNT(*) > 1 AND COUNT(DISTINCT nombre_archivo) > 1) g",
    ),
    (
        // Regla "gana la más reciente": grupos donde la sobreviviente NO tiene obras pero alguna eliminada sí
        "regla_reciente_grupos_que_pierden_obras",
        "WITH r AS (SELECT l.id, ROW_NUMBER() OVER (PARTITION BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int \
                    ORDER BY created_at DESC, id DESC) rn, \
                    COUNT(*) OVER (PARTITION BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int) n, \
                    id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int, \
                    EXISTS (SELECT 1 FROM public.vinculaciones_obras_liquidaciones v WHERE v.id_liquidacion = l.id) tiene_obra \
                    FROM public.liquidaciones l) \
         SELECT COUNT(*)::bigint FROM r k WHERE k.rn = 1 AND k.n > 1 AND NOT k.tiene_obra AND EXISTS ( \
             SELECT 1 FROM r o WHERE o.rn > 1 AND o.tiene_obra AND o.id_registro_patronal = k.id_registro_patronal \
               AND o.id_empleado = k.id_empleado AND o.anio_periodo = k.anio_periodo AND o.mes_periodo_int = k.mes_periodo_int)",
    ),
    (
        // Reportes cuyo id_liquidacion apunta a una liquidación que la regla "más reciente" eliminaría
        "regla_reciente_reportes_a_reapuntar",
        "WITH r AS (SELECT id, ROW_NUMBER() OVER (PARTITION BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int \
                    ORDER BY created_at DESC, id DESC) rn FROM public.liquidaciones) \
         SELECT COUNT(*)::bigint FROM public.reportes_liquidaciones rl JOIN r ON r.id = rl.id_liquidacion WHERE r.rn > 1",
    ),
    (
        "empleados_dup_curp_con_nss_distinto",
        "SELECT COUNT(*)::bigint FROM (SELECT 1 FROM public.empleados \
         WHERE curp IS NOT NULL AND BTRIM(curp) <> '' AND BTRIM(curp) <> 'XEXX010101HNEXXXA4' \
         GROUP BY id_cliente, BTRIM(curp) HAVING COUNT(DISTINCT NULLIF(BTRIM(nss), '')) > 1) g",
    ),
    (
        "fks_hacia_empleados",
        "SELECT COUNT(*)::bigint FROM pg_constraint WHERE contype = 'f' AND confrelid = 'public.empleados'::regclass",
    ),
    (
        // Datos temporales de pruebas de integración que no debieron quedar (deben ser 0)
        "clientes_prueba_residuales",
        "SELECT COUNT(*)::bigint FROM public.clientes WHERE nombre LIKE 'ZZ PRUEBA%'",
    ),
    (
        "archivos_cedulas_por_estado_staged",
        "SELECT COUNT(*)::bigint FROM public.archivos_cedulas WHERE estado = 'STAGED'",
    ),
    (
        // Tramos con obra y días de obra < días de cédula (deberían estar prorrateados)
        "rdl_tramos_que_requieren_prorrateo",
        "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones rdl \
         WHERE rdl.id_registro_obra IS NOT NULL AND COALESCE(rdl.dias, 0) > 0 \
           AND COALESCE(rdl.dias_registro_obra, 0) < rdl.dias AND COALESCE(rdl.subtotal, 0) > 0",
    ),
    (
        // De esos, cuántos tienen el importe COMPLETO de la cédula (prorrateo perdido)
        "rdl_prorrateo_perdido_importe_completo",
        "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones rdl \
         JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion \
         JOIN public.detalles_liquidaciones dl ON dl.id_liquidacion = rl.id_liquidacion \
          AND COALESCE(BTRIM(LOWER(dl.clave)),'') = COALESCE(BTRIM(LOWER(rdl.clave)),'') \
          AND dl.fecha IS NOT DISTINCT FROM rdl.fecha \
         WHERE rdl.id_registro_obra IS NOT NULL AND COALESCE(rdl.dias, 0) > 0 \
           AND COALESCE(rdl.dias_registro_obra, 0) < rdl.dias AND COALESCE(rdl.subtotal, 0) > 0 \
           AND ABS(rdl.subtotal - dl.subtotal) < 0.01",
    ),
    (
        // Invariante de dinero: movimientos (reporte, clave, fecha) cuya suma de tramos supera la cédula
        "movimientos_con_cuota_duplicada",
        "SELECT COUNT(*)::bigint FROM ( \
            SELECT r.id_reporte_liquidacion, COALESCE(BTRIM(LOWER(r.clave)),'') AS c, r.fecha, SUM(COALESCE(r.subtotal,0)) AS suma, \
                   MAX(dl.subtotal) AS original \
            FROM public.reportes_detalles_liquidaciones r \
            JOIN public.reportes_liquidaciones rl ON rl.id = r.id_reporte_liquidacion AND rl.estado = 'ACTIVO' \
            JOIN LATERAL (SELECT d.subtotal FROM public.detalles_liquidaciones d WHERE d.id_liquidacion = rl.id_liquidacion \
                AND COALESCE(BTRIM(LOWER(d.clave)),'') = COALESCE(BTRIM(LOWER(r.clave)),'') AND d.fecha IS NOT DISTINCT FROM r.fecha \
                ORDER BY d.id LIMIT 1) dl ON true \
            GROUP BY 1, 2, 3) g WHERE g.suma > COALESCE(g.original,0) + 0.05",
    ),
    (
        // Movimientos con un tramo SIN obra (factor 1) y otro CON obra a la vez (origen típico del doble conteo)
        "movimientos_sin_obra_y_con_obra_mezclados",
        "SELECT COUNT(*)::bigint FROM ( \
            SELECT r.id_reporte_liquidacion, COALESCE(BTRIM(LOWER(r.clave)),''), r.fecha \
            FROM public.reportes_detalles_liquidaciones r \
            JOIN public.reportes_liquidaciones rl ON rl.id = r.id_reporte_liquidacion AND rl.estado = 'ACTIVO' \
            GROUP BY 1, 2, 3 HAVING bool_or(r.id_registro_obra IS NULL) AND bool_or(r.id_registro_obra IS NOT NULL)) g",
    ),
    (
        // Registros patronales sin código postal válido (el SUA 2_AFIL lo deja en blanco)
        "registros_patronales_sin_cp_valido",
        "SELECT COUNT(*)::bigint FROM public.registros_patronales WHERE codigo_postal IS NULL OR BTRIM(codigo_postal) !~ '^[0-9]{5}$'",
    ),
    ("registros_patronales_total", "SELECT COUNT(*)::bigint FROM public.registros_patronales"),
    ("postgres_version_num", "SELECT current_setting('server_version_num')::bigint"),
];

/// Ejecuta el diagnóstico completo dentro de una transacción de solo lectura.
pub fn diagnosticar(client: &mut Client) -> Result<Vec<Indicador>, String> {
    let mut tx = client
        .build_transaction()
        .read_only(true)
        .start()
        .map_err(|e| format!("No se pudo abrir transacción READ ONLY: {}", e))?;
    let mut out = Vec::with_capacity(CONSULTAS.len());
    for (nombre, sql) in CONSULTAS {
        let valor: i64 = tx
            .query_one(*sql, &[])
            .map_err(|e| format!("Diagnóstico '{}' falló: {}", nombre, crate::db::schema::format_pg_error(&e)))?
            .get(0);
        out.push(Indicador { nombre, valor });
    }
    tx.rollback().map_err(|e| format!("Error cerrando transacción READ ONLY: {}", e))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Diagnóstico contra la BD configurada. Ignorado por defecto: `cargo test diagnostico_bd -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn diagnostico_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("Sin conexión a BD, diagnóstico omitido: {}", e);
                return;
            }
        };
        let indicadores = diagnosticar(&mut client).expect("diagnóstico");
        for i in &indicadores {
            println!("DIAG {:<40} {}", i.nombre, i.valor);
        }
    }

    /// Muestra de pares duplicados (solo lectura): `cargo test muestra_duplicados_bd -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn muestra_duplicados_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("Sin conexión a BD, muestra omitida: {}", e);
                return;
            }
        };
        let mut tx = client.build_transaction().read_only(true).start().expect("tx read only");
        let rows = tx
            .query(
                "WITH g AS (SELECT id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int \
                            FROM public.liquidaciones GROUP BY 1,2,3,4 HAVING COUNT(*) > 1) \
                 SELECT l.anio_periodo::text || '-' || l.mes_periodo_int::text, \
                        LEFT(l.id_empleado::text, 8), \
                        COALESCE(l.nombre_archivo, ''), \
                        (SELECT COUNT(*) FROM public.detalles_liquidaciones d WHERE d.id_liquidacion = l.id)::text, \
                        (SELECT COALESCE(SUM(d.dias),0) FROM public.detalles_liquidaciones d WHERE d.id_liquidacion = l.id)::text, \
                        (SELECT ROUND(COALESCE(SUM(d.subtotal),0)::numeric,2) FROM public.detalles_liquidaciones d WHERE d.id_liquidacion = l.id)::text, \
                        (SELECT COUNT(*) FROM public.vinculaciones_obras_liquidaciones v WHERE v.id_liquidacion = l.id)::text, \
                        (SELECT COUNT(*) FROM public.reportes_liquidaciones r WHERE r.id_liquidacion = l.id)::text, \
                        to_char(l.created_at, 'YYYY-MM-DD HH24:MI') \
                 FROM public.liquidaciones l JOIN g USING (id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int) \
                 ORDER BY l.anio_periodo, l.mes_periodo_int, l.id_empleado, l.created_at LIMIT 24",
                &[],
            )
            .expect("muestra");
        println!("MUESTRA periodo | emp | archivo | n_det | dias | subtotal | n_vinc | n_rep | creado");
        for r in rows {
            let c: Vec<String> = (0..9).map(|i| r.get::<_, String>(i)).collect();
            println!("MUESTRA {}", c.join(" | "));
        }
        let pares = tx
            .query(
                "SELECT a.nombre_archivo || '  <->  ' || b.nombre_archivo, COUNT(*)::text \
                 FROM public.liquidaciones a JOIN public.liquidaciones b \
                   ON a.id_registro_patronal = b.id_registro_patronal AND a.id_empleado = b.id_empleado \
                  AND a.anio_periodo = b.anio_periodo AND a.mes_periodo_int = b.mes_periodo_int AND a.id < b.id \
                 GROUP BY 1 ORDER BY COUNT(*) DESC LIMIT 15",
                &[],
            )
            .expect("pares");
        for r in pares {
            println!("PAR {} x{}", r.get::<_, String>(0), r.get::<_, String>(1));
        }
        // Por archivo: trabajadores, días y subtotal; y cuántos de sus trabajadores aparecen en OTRO archivo del mismo mes
        let archivos = tx
            .query(
                "WITH t AS (SELECT l.id, l.id_registro_patronal, l.id_empleado, l.anio_periodo, l.mes_periodo_int, l.nombre_archivo, \
                        (SELECT COALESCE(SUM(d.dias),0) FROM public.detalles_liquidaciones d WHERE d.id_liquidacion = l.id) dias, \
                        (SELECT COALESCE(SUM(d.subtotal),0) FROM public.detalles_liquidaciones d WHERE d.id_liquidacion = l.id) sub, \
                        (SELECT string_agg(BTRIM(ro.numero), ',' ORDER BY ro.numero) FROM public.vinculaciones_obras_liquidaciones v \
                           JOIN public.registros_obras ro ON ro.id = v.id_registro_obra WHERE v.id_liquidacion = l.id) obras \
                        FROM public.liquidaciones l) \
                 SELECT a.anio_periodo::text || '-' || lpad(a.mes_periodo_int::text,2,'0'), a.nombre_archivo, COUNT(*)::text, \
                        SUM(a.dias)::text, ROUND(SUM(a.sub)::numeric,2)::text, \
                        COUNT(*) FILTER (WHERE EXISTS (SELECT 1 FROM t b WHERE b.id_registro_patronal = a.id_registro_patronal \
                            AND b.id_empleado = a.id_empleado AND b.anio_periodo = a.anio_periodo AND b.mes_periodo_int = a.mes_periodo_int \
                            AND b.nombre_archivo <> a.nombre_archivo))::text, \
                        COUNT(*) FILTER (WHERE EXISTS (SELECT 1 FROM t b WHERE b.id_registro_patronal = a.id_registro_patronal \
                            AND b.id_empleado = a.id_empleado AND b.anio_periodo = a.anio_periodo AND b.mes_periodo_int = a.mes_periodo_int \
                            AND b.nombre_archivo <> a.nombre_archivo AND b.dias = a.dias AND ABS(b.sub - a.sub) < 0.01))::text, \
                        COALESCE(string_agg(DISTINCT a.obras, ' ; '), '-') \
                 FROM t a WHERE (a.anio_periodo, a.mes_periodo_int) IN ((2023,9),(2023,8),(2022,1),(2023,12),(2021,11)) \
                 GROUP BY a.anio_periodo, a.mes_periodo_int, a.nombre_archivo ORDER BY 1, 2",
                &[],
            )
            .expect("archivos");
        println!("ARCH periodo | archivo | trab | dias | subtotal | trab_en_otro_archivo | trab_identicos_en_otro | obras");
        for r in archivos {
            let c: Vec<String> = (0..8).map(|i| r.get::<_, String>(i)).collect();
            println!("ARCH {}", c.join(" | "));
        }
        tx.rollback().expect("rollback");
    }

    /// Historial reciente (solo lectura) de cambios en reportes_detalles_liquidaciones:
    /// `cargo test historial_reciente_rdl_bd -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn historial_reciente_rdl_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("Sin conexión a BD, historial omitido: {}", e);
                return;
            }
        };
        let mut tx = client.build_transaction().read_only(true).start().expect("tx read only");
        let rows = tx
            .query(
                "SELECT event_name::text, COUNT(*)::text, COUNT(DISTINCT id)::text, \
                        to_char(MIN(modified_at), 'HH24:MI:SS'), to_char(MAX(modified_at), 'HH24:MI:SS') \
                 FROM revision_history.reportes_detalles_liquidaciones \
                 WHERE modified_at > now() - interval '45 minutes' GROUP BY 1 ORDER BY 1",
                &[],
            )
            .expect("historial");
        for r in rows {
            let c: Vec<String> = (0..5).map(|i| r.get::<_, String>(i)).collect();
            println!("HIST {}", c.join(" | "));
        }
        let borrados = tx
            .query(
                "SELECT id::text, COALESCE(id_registro_obra::text,'NULL'), COALESCE(dias_registro_obra::text,'NULL'), \
                        COALESCE(BTRIM(clave),''), to_char(modified_at,'HH24:MI:SS') \
                 FROM revision_history.reportes_detalles_liquidaciones \
                 WHERE modified_at > now() - interval '45 minutes' AND event_name = 'DELETE' ORDER BY modified_at",
                &[],
            )
            .expect("borrados");
        for r in borrados {
            let c: Vec<String> = (0..5).map(|i| r.get::<_, String>(i)).collect();
            println!("BORRADO {}", c.join(" | "));
        }
        tx.rollback().expect("rollback");
    }

    /// Borrados recientes (solo lectura) en tablas maestras según revision_history:
    /// `cargo test borrados_recientes_bd -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn borrados_recientes_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("Sin conexión a BD: {}", e);
                return;
            }
        };
        let mut tx = client.build_transaction().read_only(true).start().expect("tx read only");
        for tabla in ["empleados", "liquidaciones", "reportes_liquidaciones", "clientes", "registros_patronales", "revisiones"] {
            let existe: bool = tx
                .query_one(
                    "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'revision_history' AND table_name = $1)",
                    &[&tabla],
                )
                .expect("existe")
                .get(0);
            if !existe {
                println!("BORR {tabla}: sin tabla de historial");
                continue;
            }
            let sql = format!(
                "SELECT to_char(date_trunc('minute', modified_at), 'YYYY-MM-DD HH24:MI'), COUNT(*)::text \
                 FROM revision_history.{tabla} WHERE event_name = 'DELETE' AND modified_at > now() - interval '24 hours' \
                 GROUP BY 1 ORDER BY 1"
            );
            for r in tx.query(sql.as_str(), &[]).expect("historial") {
                println!("BORR {tabla} {} x{}", r.get::<_, String>(0), r.get::<_, String>(1));
            }
        }
        let filas = tx
            .query(
                "SELECT to_char(date_trunc('minute', modified_at), 'YYYY-MM-DD HH24:MI'), COALESCE(nombre_archivo,''), \
                        anio_periodo::text || '-' || mes_periodo_int::text, COUNT(*)::text \
                 FROM revision_history.liquidaciones WHERE event_name = 'DELETE' AND modified_at > now() - interval '24 hours' \
                 GROUP BY 1, 2, 3 ORDER BY 1 LIMIT 20",
                &[],
            )
            .expect("detalle liquidaciones");
        for r in filas {
            println!("LIQ {} | {} | {} | x{}", r.get::<_, String>(0), r.get::<_, String>(1), r.get::<_, String>(2), r.get::<_, String>(3));
        }
        for r in tx
            .query(
                "SELECT to_char(modified_at, 'YYYY-MM-DD HH24:MI:SS'), COALESCE(nombre,''), COALESCE(rfc,'') \
                 FROM revision_history.clientes WHERE event_name = 'DELETE' AND modified_at > now() - interval '24 hours' ORDER BY 1",
                &[],
            )
            .expect("clientes borrados")
        {
            println!("CLI {} | {} | {}", r.get::<_, String>(0), r.get::<_, String>(1), r.get::<_, String>(2));
        }
        let ahora: String = tx.query_one("SELECT to_char(now(), 'YYYY-MM-DD HH24:MI')", &[]).expect("now").get(0);
        println!("AHORA {ahora}");
        tx.rollback().expect("rollback");
    }

    #[test]
    fn todas_las_consultas_son_solo_lectura() {
        for (nombre, sql) in CONSULTAS {
            let up = sql.to_uppercase();
            for prohibido in ["INSERT ", "UPDATE ", "DELETE ", "ALTER ", "DROP ", "TRUNCATE ", "CREATE "] {
                assert!(!up.contains(prohibido), "{} contiene {}", nombre, prohibido);
            }
        }
    }
}
