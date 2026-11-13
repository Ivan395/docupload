//! Verificación de la sincronización incremental (Fase 3) contra la BD configurada, SIN efectos:
//! todo ocurre en una transacción que termina en ROLLBACK.
//!
//! Para cada revisión con reportes: ejecuta `sync_reportes_liquidaciones_for_revision_tx` y verifica
//! (1) que no desaparece ningún reporte, y (2) que el importe de cada tramo guardado por el SQL coincide
//! exactamente con el calculado en memoria por `crate::db::prorrateo` (misma regla en BD y en Rust).
//!
//! Ejecutar: `cargo test sync_incremental_no_destructiva_bd -- --ignored --nocapture`

use crate::db::prorrateo::{factor_prorrateo, prorratear, ImportesTramo};
use crate::db::revisiones::{create_obra_tx, RegistroObraItem};
use chrono::NaiveDate;
use postgres::{Client, Transaction};
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

// ---------------------------------------------------------------------------------------------
// Universo del Reporte de Liquidaciones (solo meses de los Registros de Obra del usuario).
// Patrón de fixture copiado de `pruebas_integracion_topes.rs` (sus helpers son privados). Todo
// ocurre en una transacción que termina en ROLLBACK; nunca hay COMMIT.
// ---------------------------------------------------------------------------------------------

const MESES_NOMBRE: [&str; 12] = [
    "ENERO", "FEBRERO", "MARZO", "ABRIL", "MAYO", "JUNIO",
    "JULIO", "AGOSTO", "SEPTIEMBRE", "OCTUBRE", "NOVIEMBRE", "DICIEMBRE",
];

fn fecha_prueba(anio: i32, mes: u32, dia: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(anio, mes, dia).expect("fecha de prueba inválida")
}

/// Cliente, registro patronal, revisión (sin periodo_inicio/fin) y empleado temporales.
fn crear_contexto_universo(tx: &mut Transaction<'_>) -> (Uuid, Uuid, Uuid) {
    let uuid = Uuid::now_v7();
    let nombre_cliente = format!("ZZ PRUEBA UNIVERSO {}", uuid.simple());
    let registro = format!("U{:010}", uuid.as_u128() % 10_000_000_000);

    let id_cliente: Uuid = tx
        .query_one("INSERT INTO public.clientes (nombre) VALUES ($1) RETURNING id", &[&nombre_cliente])
        .expect("crear cliente de prueba")
        .get(0);
    let id_rp: Uuid = tx
        .query_one(
            "INSERT INTO public.registros_patronales (id_cliente, registro) VALUES ($1, $2) RETURNING id",
            &[&id_cliente, &registro],
        )
        .expect("crear registro patronal de prueba")
        .get(0);
    let id_revision: Uuid = tx
        .query_one(
            "INSERT INTO public.revisiones (id_registro_patronal, nombre) VALUES ($1, $2) RETURNING id",
            &[&id_rp, &format!("ZZ REVISION UNIVERSO {}", uuid.simple())],
        )
        .expect("crear revisión de prueba")
        .get(0);
    let id_empleado: Uuid = tx
        .query_one(
            "INSERT INTO public.empleados (id_cliente, nombre, nss) VALUES ($1, $2, $3) RETURNING id",
            &[&id_cliente, &"PRUEBA UNIVERSO 1", &"12345678903"],
        )
        .expect("crear empleado de prueba")
        .get(0);
    (id_rp, id_revision, id_empleado)
}

/// Liquidación del mes/año indicados con un detalle de 10 días.
fn crear_liquidacion_mes(tx: &mut Transaction<'_>, id_rp: Uuid, id_empleado: Uuid, anio: i32, mes: u32) -> Uuid {
    let id_liquidacion: Uuid = tx
        .query_one(
            "INSERT INTO public.liquidaciones
                (id_registro_patronal, id_empleado, mes_periodo, mes_periodo_int, anio_periodo, nombre_archivo)
             VALUES ($1, $2, $3, $4, $5, 'PRUEBA_UNIVERSO.xlsx')
             RETURNING id",
            &[&id_rp, &id_empleado, &MESES_NOMBRE[(mes - 1) as usize], &(mes as i16), &(anio as i16)],
        )
        .expect("crear liquidación de prueba")
        .get(0);
    tx.execute(
        "INSERT INTO public.detalles_liquidaciones (id_liquidacion, clave, fecha, dias, sdi, cf, patronal, obrera, subtotal)
         VALUES ($1, 'PRUEBA', $2, 10, 300.0, 10.0, 5.0, 5.0, 10.0)",
        &[&id_liquidacion, &fecha_prueba(anio, mes, 15)],
    )
    .expect("crear detalle de liquidación de prueba");
    id_liquidacion
}

fn item_ro_usuario(id_revision: Uuid, id_rp: Uuid, numero: &str, inicio: NaiveDate, fin: NaiveDate) -> RegistroObraItem {
    RegistroObraItem {
        id: String::new(),
        id_revision: id_revision.to_string(),
        id_registro_patronal: id_rp.to_string(),
        numero: numero.to_string(),
        clase: "A".to_string(),
        tipo_obra: "OBRA DE PRUEBA".to_string(),
        fecha_inicio: Some(inicio),
        fecha_fin: Some(fin),
        ubicacion: "PRUEBA".to_string(),
        superficie_construido: 0.0,
        monto: None,
        delegacion: "PRUEBA".to_string(),
        subdelegacion: "PRUEBA".to_string(),
        created_at: String::new(),
        creado_usuario: true,
    }
}

/// Meses (anio, mes) ACTIVO de una revisión tras sincronizar, con la misma condición de estado que `fetch`.
fn meses_activos(tx: &mut Transaction<'_>, id_revision: Uuid) -> Vec<(i32, i32)> {
    tx.query(
        "SELECT rl.anio_periodo::int, rl.mes_periodo_int::int FROM public.reportes_liquidaciones rl
         WHERE rl.id_revision = $1 AND rl.estado::text = 'ACTIVO'
         ORDER BY 1, 2",
        &[&id_revision],
    )
    .expect("consultar meses activos")
    .iter()
    .map(|r| (r.get::<_, i32>(0), r.get::<_, i32>(1)))
    .collect()
}

#[test]
#[ignore]
fn universo_solo_meses_de_ro() {
    let mut client: Client = match crate::db::client::create_client() {
        Ok(c) => c,
        Err(e) => {
            println!("PostgreSQL no disponible (prueba omitida): {e}");
            return;
        }
    };
    crate::db::schema::ensure_schema(&mut client).expect("ensure_schema");
    let mut tx = client.transaction().expect("transacción");

    // Caso dorado: dos RO del usuario, sin periodo de revisión. Liquidaciones de 2014-01 a 2015-12.
    let (rp, rev, emp) = crear_contexto_universo(&mut tx);
    for anio in [2014, 2015] {
        for mes in 1..=12u32 {
            crear_liquidacion_mes(&mut tx, rp, emp, anio, mes);
        }
    }
    create_obra_tx(&mut tx, &item_ro_usuario(rev, rp, "U0000001", fecha_prueba(2014, 1, 1), fecha_prueba(2014, 5, 30)))
        .expect("crear RO 1 de prueba");
    create_obra_tx(&mut tx, &item_ro_usuario(rev, rp, "U0000002", fecha_prueba(2015, 7, 15), fecha_prueba(2015, 8, 30)))
        .expect("crear RO 2 de prueba");

    crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx(&mut tx, rev)
        .expect("sync del universo con dos RO");
    let esperado: Vec<(i32, i32)> = vec![
        (2014, 1), (2014, 2), (2014, 3), (2014, 4), (2014, 5), (2015, 7), (2015, 8),
    ];
    assert_eq!(meses_activos(&mut tx, rev), esperado, "el universo debe ser solo los meses de las RO");

    // Caso sin RO del usuario: el universo debe quedar vacío (decisión D1).
    let (rp2, rev2, emp2) = crear_contexto_universo(&mut tx);
    for mes in 1..=12u32 {
        crear_liquidacion_mes(&mut tx, rp2, emp2, 2014, mes);
    }
    crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx(&mut tx, rev2)
        .expect("sync sin RO");
    assert!(meses_activos(&mut tx, rev2).is_empty(), "sin RO del usuario no debe haber meses en el universo");

    tx.rollback().expect("rollback");
}
