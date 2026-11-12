//! Verificación de la sincronización incremental (Fase 3) contra la BD configurada, SIN efectos:
//! todo ocurre en una transacción que termina en ROLLBACK.
//!
//! Para cada revisión con reportes: ejecuta `sync_reportes_liquidaciones_for_revision_tx` y verifica
//! (1) que no desaparece ningún reporte, y (2) que el importe de cada tramo guardado por el SQL coincide
//! exactamente con el calculado en memoria por `crate::db::prorrateo` (misma regla en BD y en Rust).
//!
//! Ejecutar: `cargo test sync_incremental_no_destructiva_bd -- --ignored --nocapture`

use crate::db::prorrateo::{factor_prorrateo, prorratear, ImportesTramo};
use uuid::Uuid;

/// Columnas monetarias comparadas (nombre, índice en la consulta de originales/guardados).
const RAMAS: [&str; 14] = [
    "cf", "exc_pat", "exc_obr", "pd_pat", "pd_obr", "gmp_pat", "gmp_obr", "rt", "iv_pat", "iv_obr", "gps",
    "patronal", "obrera", "subtotal",
];

fn importes_desde(fila: &postgres::Row, inicio: usize) -> ImportesTramo {
    let g = |i: usize| -> f64 { fila.get::<_, Option<f64>>(inicio + i).unwrap_or(0.0) };
    ImportesTramo {
        cf: g(0),
        exc_pat: g(1),
        exc_obr: g(2),
        pd_pat: g(3),
        pd_obr: g(4),
        gmp_pat: g(5),
        gmp_obr: g(6),
        rt: g(7),
        iv_pat: g(8),
        iv_obr: g(9),
        gps: g(10),
        patronal: g(11),
        obrera: g(12),
        subtotal: g(13),
    }
}

fn como_arreglo(i: &ImportesTramo) -> [f64; 14] {
    [
        i.cf, i.exc_pat, i.exc_obr, i.pd_pat, i.pd_obr, i.gmp_pat, i.gmp_obr, i.rt, i.iv_pat, i.iv_obr, i.gps,
        i.patronal, i.obrera, i.subtotal,
    ]
}

#[test]
#[ignore]
fn sync_incremental_no_destructiva_bd() {
    let mut client = match crate::db::client::create_client() {
        Ok(c) => c,
        Err(e) => {
            println!("PostgreSQL no disponible (prueba omitida): {e}");
            return;
        }
    };
    let revisiones: Vec<Uuid> = client
        .query(
            "SELECT DISTINCT id_revision FROM public.reportes_liquidaciones WHERE id_revision IS NOT NULL",
            &[],
        )
        .expect("revisiones con reportes")
        .iter()
        .map(|r| r.get(0))
        .collect();

    let cols_d = RAMAS.iter().map(|c| format!("d.{c}")).collect::<Vec<_>>().join(", ");
    let cols_r = RAMAS.iter().map(|c| format!("r.{c}")).collect::<Vec<_>>().join(", ");
    let sql_tramos = format!(
        "SELECT r.id, (r.id_registro_obra IS NOT NULL), COALESCE(r.dias,0)::int, COALESCE(r.dias_registro_obra,0)::int, \
                COALESCE(r.lic,0), COALESCE(r.inc,0), COALESCE(r.aus,0), {cols_d}, {cols_r} \
         FROM public.reportes_detalles_liquidaciones r \
         JOIN public.reportes_liquidaciones rl ON rl.id = r.id_reporte_liquidacion \
         JOIN LATERAL (SELECT d.* FROM public.detalles_liquidaciones d WHERE d.id_liquidacion = rl.id_liquidacion \
             AND COALESCE(BTRIM(LOWER(d.clave)),'') = COALESCE(BTRIM(LOWER(r.clave)),'') \
             AND d.fecha IS NOT DISTINCT FROM r.fecha ORDER BY d.id LIMIT 1) d ON true \
         WHERE rl.id_revision = $1"
    );

    let mut total_tramos = 0usize;
    let mut diferencias: Vec<String> = Vec::new();
    // Esquema y migraciones únicas al día (igual que la app al arrancar)
    crate::db::schema::ensure_schema(&mut client).expect("ensure_schema");
    let mut tx = client.transaction().expect("transacción");
    for rev in &revisiones {
        let conteo = |tx: &mut postgres::Transaction, sql: &str| -> i64 {
            tx.query_one(sql, &[rev]).expect("conteo").get(0)
        };
        let rl_antes = conteo(&mut tx, "SELECT COUNT(*)::bigint FROM public.reportes_liquidaciones WHERE id_revision = $1");
        // Solo los reportes EDITADOS por el analista están protegidos; los no editados reflejan la cédula.
        let obra_antes = conteo(
            &mut tx,
            "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones r JOIN public.reportes_liquidaciones rl \
             ON rl.id = r.id_reporte_liquidacion WHERE rl.id_revision = $1 AND r.id_registro_obra IS NOT NULL AND rl.editado_usuario",
        );
        let no_editados_obra_antes = conteo(
            &mut tx,
            "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones r JOIN public.reportes_liquidaciones rl \
             ON rl.id = r.id_reporte_liquidacion WHERE rl.id_revision = $1 AND r.id_registro_obra IS NOT NULL AND NOT rl.editado_usuario",
        );
        let huerfanos_no_editados = conteo(
            &mut tx,
            "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones r JOIN public.reportes_liquidaciones rl \
             ON rl.id = r.id_reporte_liquidacion WHERE rl.id_revision = $1 AND NOT rl.editado_usuario AND r.id_registro_obra IS NOT NULL \
             AND NOT EXISTS (SELECT 1 FROM public.detalles_liquidaciones d WHERE d.id_liquidacion = rl.id_liquidacion \
                 AND COALESCE(BTRIM(LOWER(d.clave)),'') = COALESCE(BTRIM(LOWER(r.clave)),'') AND d.fecha IS NOT DISTINCT FROM r.fecha)",
        );

        crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx(&mut tx, *rev)
            .unwrap_or_else(|e| panic!("sync de la revisión {rev} falló: {e}"));

        let rl_despues = conteo(&mut tx, "SELECT COUNT(*)::bigint FROM public.reportes_liquidaciones WHERE id_revision = $1");
        let fuera = conteo(
            &mut tx,
            "SELECT COUNT(*)::bigint FROM public.reportes_liquidaciones WHERE id_revision = $1 AND estado = 'FUERA_PERIODO'",
        );
        let obra_despues = conteo(
            &mut tx,
            "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones r JOIN public.reportes_liquidaciones rl \
             ON rl.id = r.id_reporte_liquidacion WHERE rl.id_revision = $1 AND r.id_registro_obra IS NOT NULL AND rl.editado_usuario",
        );
        let no_editados_obra_despues = conteo(
            &mut tx,
            "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones r JOIN public.reportes_liquidaciones rl \
             ON rl.id = r.id_reporte_liquidacion WHERE rl.id_revision = $1 AND r.id_registro_obra IS NOT NULL AND NOT rl.editado_usuario",
        );
        println!(
            "SYNC rev={rev} no_editados: tramos_con_obra {no_editados_obra_antes}->{no_editados_obra_despues} (huérfanos de cédula antes: {huerfanos_no_editados})"
        );
        println!(
            "SYNC rev={rev} reportes {rl_antes}->{rl_despues} (fuera_periodo={fuera}) tramos_con_obra {obra_antes}->{obra_despues}"
        );
        assert!(rl_despues >= rl_antes, "La sincronización borró reportes en la revisión {rev}");
        assert!(
            obra_despues >= obra_antes,
            "La sincronización quitó asignaciones de obra EDITADAS por el analista en la revisión {rev} ({obra_antes} -> {obra_despues})"
        );

        for fila in tx.query(sql_tramos.as_str(), &[rev]).expect("tramos") {
            total_tramos += 1;
            let id: Uuid = fila.get(0);
            let factor = factor_prorrateo(fila.get(1), fila.get(2), fila.get(3), fila.get(4), fila.get(5), fila.get(6));
            let esperado = como_arreglo(&prorratear(&importes_desde(&fila, 7), factor));
            let guardado = como_arreglo(&importes_desde(&fila, 7 + RAMAS.len()));
            for (k, nombre) in RAMAS.iter().enumerate() {
                if esperado[k] != guardado[k] {
                    diferencias.push(format!(
                        "tramo {id} {nombre}: memoria={} bd={} (factor {factor})",
                        esperado[k], guardado[k]
                    ));
                }
            }
        }
    }
    // Invariante de dinero tras sincronizar: suma de tramos hermanos <= importe de la cédula (+ 5 centavos)
    let sql_dup = "SELECT g.id_reporte_liquidacion::text, g.c, COALESCE(g.fecha::text,''), g.suma::float8, COALESCE(g.original,0)::float8, g.dias_obra::int, g.dias_ced::int \
        FROM (SELECT r.id_reporte_liquidacion, COALESCE(BTRIM(LOWER(r.clave)),'') AS c, r.fecha, SUM(COALESCE(r.subtotal,0)) AS suma, \
                     MAX(dl.subtotal) AS original, SUM(COALESCE(r.dias_registro_obra,0)) AS dias_obra, MAX(COALESCE(r.dias,0)) AS dias_ced \
              FROM public.reportes_detalles_liquidaciones r \
              JOIN public.reportes_liquidaciones rl ON rl.id = r.id_reporte_liquidacion AND rl.estado = 'ACTIVO' \
              JOIN LATERAL (SELECT d.subtotal FROM public.detalles_liquidaciones d WHERE d.id_liquidacion = rl.id_liquidacion \
                  AND COALESCE(BTRIM(LOWER(d.clave)),'') = COALESCE(BTRIM(LOWER(r.clave)),'') AND d.fecha IS NOT DISTINCT FROM r.fecha \
                  ORDER BY d.id LIMIT 1) dl ON true \
              GROUP BY 1, 2, 3) g WHERE g.suma > COALESCE(g.original,0) + 0.05";
    let duplicados = tx.query(sql_dup, &[]).expect("invariante");
    println!("SYNC invariante: movimientos con cuota duplicada tras sincronizar = {}", duplicados.len());
    for d in duplicados.iter().take(10) {
        println!(
            "DUP reporte={} clave='{}' fecha={} suma={} original={} dias_obra_sumados={} dias_cedula={}",
            d.get::<_, String>(0), d.get::<_, String>(1), d.get::<_, String>(2),
            d.get::<_, f64>(3), d.get::<_, f64>(4), d.get::<_, i32>(5), d.get::<_, i32>(6)
        );
    }
    tx.rollback().expect("rollback");

    println!("SYNC tramos comparados: {total_tramos}, diferencias: {}", diferencias.len());
    for d in diferencias.iter().take(20) {
        println!("DIF {d}");
    }
    assert!(diferencias.is_empty(), "La BD y la memoria no aplican el mismo prorrateo");
}

/// Diagnóstico (ROLLBACK): qué tramos con obra de reportes NO editados desaparecen al sincronizar y por qué.
#[test]
#[ignore]
fn diagnostico_tramos_obra_perdidos_bd() {
    let mut client = match crate::db::client::create_client() {
        Ok(c) => c,
        Err(e) => {
            println!("Sin conexión: {e}");
            return;
        }
    };
    let rev: Uuid = client
        .query_one(
            "SELECT rl.id_revision FROM public.reportes_liquidaciones rl GROUP BY 1 ORDER BY COUNT(*) DESC LIMIT 1",
            &[],
        )
        .expect("rev")
        .get(0);
    let sql = "SELECT r.id, rl.id, COALESCE(BTRIM(LOWER(r.clave)),''), COALESCE(r.fecha::text,''), BTRIM(ro.numero), \
                      COALESCE(ro.creado_usuario,false), rl.estado::text, ro.id_revision = rl.id_revision \
               FROM public.reportes_detalles_liquidaciones r \
               JOIN public.reportes_liquidaciones rl ON rl.id = r.id_reporte_liquidacion \
               JOIN public.registros_obras ro ON ro.id = r.id_registro_obra \
               WHERE rl.id_revision = $1 AND NOT rl.editado_usuario";
    let mut tx = client.transaction().expect("tx");
    let antes: Vec<postgres::Row> = tx.query(sql, &[&rev]).expect("antes");
    crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx(&mut tx, rev).expect("sync");
    let despues: std::collections::HashSet<Uuid> =
        tx.query(sql, &[&rev]).expect("despues").iter().map(|r| r.get::<_, Uuid>(0)).collect();
    let mut por_motivo: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for r in &antes {
        let id: Uuid = r.get(0);
        if despues.contains(&id) {
            continue;
        }
        let rep: Uuid = r.get(1);
        let sigue_tramo: i64 = tx
            .query_one(
                "SELECT COUNT(*)::bigint FROM public.reportes_detalles_liquidaciones x WHERE x.id = $1",
                &[&id],
            )
            .expect("x")
            .get(0);
        let motivo = format!(
            "creado_usuario={} estado={} obra_de_esta_revision={} tramo_sigue_existiendo={}",
            r.get::<_, bool>(5), r.get::<_, String>(6), r.get::<_, bool>(7), sigue_tramo > 0
        );
        *por_motivo.entry(motivo).or_default() += 1;
        let _ = rep;
    }
    tx.rollback().expect("rollback");
    println!("DIAG rev={rev} tramos_con_obra_antes={} perdidos={}", antes.len(), antes.len() - despues.len());
    for (m, n) in por_motivo {
        println!("DIAG {n} × {m}");
    }
}
