//! Pruebas de integración de los arreglos de nóminas contra la BD configurada.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. Cada prueba corre dentro de UNA sola
//! transacción que se revierte al final (`tx.rollback()`; si la prueba hace panic, Drop revierte).
//! Nunca se hace COMMIT. Solo se ejecutan explícitamente con `cargo test -- --ignored`.

use chrono::NaiveDate;
use postgres::{Client, Transaction};
use uuid::Uuid;

use crate::db::client::create_client;
use crate::db::delete_nominas_de_revision_tx;
use crate::db::nominas_reporte::{sync_reportes_nominas_revision_tx, SyncNominasResumen};
use crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx;
use crate::db::schema::{ensure_schema, format_pg_error};

fn d(anio: i32, mes: u32, dia: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(anio, mes, dia).expect("fecha de prueba inválida")
}

fn conectar() -> Client {
    let mut client = create_client()
        .unwrap_or_else(|e| panic!("No se pudo conectar a PostgreSQL para pruebas de nóminas: {e}"));
    ensure_schema(&mut client)
        .unwrap_or_else(|e| panic!("ensure_schema falló: {}", format_pg_error(&e)));
    client
}

/// Crea cliente, registro patronal, revisión y un empleado temporales dentro de la transacción.
fn crear_contexto(tx: &mut Transaction<'_>, sufijo: &str) -> (Uuid, Uuid, Uuid, Uuid) {
    let uuid = Uuid::now_v7();
    let nombre_cliente = format!("ZZ PRUEBA NOMINAS {} {}", sufijo, uuid.simple());
    let registro = format!("Y{:010}", uuid.as_u128() % 10_000_000_000);

    let id_cliente: Uuid = tx
        .query_one(
            "INSERT INTO public.clientes (nombre) VALUES ($1) RETURNING id",
            &[&nombre_cliente],
        )
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
            &[&id_rp, &format!("ZZ REVISION NOMINAS {}", uuid.simple())],
        )
        .expect("crear revisión de prueba")
        .get(0);

    let id_empleado: Uuid = tx
        .query_one(
            "INSERT INTO public.empleados (id_cliente, nombre, nss) VALUES ($1, 'PRUEBA NOMINAS', '12345678901') RETURNING id",
            &[&id_cliente],
        )
        .expect("crear empleado de prueba")
        .get(0);

    (id_cliente, id_rp, id_revision, id_empleado)
}

/// Inserta una nómina (con su CFDI mínimo) y, si existe algún concepto, un detalle.
fn crear_nomina(
    tx: &mut Transaction<'_>,
    id_rp: Uuid,
    id_revision: Uuid,
    id_empleado: Uuid,
    inicio: NaiveDate,
    fin: NaiveDate,
) -> Uuid {
    // nominas.id_cfdi es NOT NULL; cfdis solo requiere fecha_emision (con default).
    let id_cfdi: Uuid = tx
        .query_one("INSERT INTO public.cfdis DEFAULT VALUES RETURNING id", &[])
        .expect("crear cfdi de prueba")
        .get(0);

    let dias: f64 = ((fin - inicio).num_days() + 1) as f64;
    let id_nomina: Uuid = tx
        .query_one(
            "INSERT INTO public.nominas
                (id_cfdi, id_registro_patronal, id_revision, id_empleado,
                 fecha_pago, fecha_inicial_pago, fecha_final_pago, num_dias_pagados, total_sueldos)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1000.0)
             RETURNING id",
            &[&id_cfdi, &id_rp, &id_revision, &id_empleado, &fin, &inicio, &fin, &dias],
        )
        .expect("crear nómina de prueba")
        .get(0);

    let concepto: Option<Uuid> = tx
        .query("SELECT id FROM public.conceptos LIMIT 1", &[])
        .expect("consultar conceptos")
        .first()
        .map(|r| r.get(0));
    if let Some(id_concepto) = concepto {
        tx.execute(
            "INSERT INTO public.detalles_nominas (id_nomina, id_concepto, clave, concepto, importe)
             VALUES ($1, $2, '001', 'SUELDO PRUEBA', 1000.0)",
            &[&id_nomina, &id_concepto],
        )
        .expect("crear detalle de nómina de prueba");
    }

    id_nomina
}

fn crear_obra(
    tx: &mut Transaction<'_>,
    id_rp: Uuid,
    id_revision: Uuid,
    numero: &str,
    inicio: NaiveDate,
    fin: NaiveDate,
) -> Uuid {
    tx.query_one(
        "INSERT INTO public.registros_obras
            (id_revision, id_registro_patronal, numero, fecha_inicio, fecha_fin, creado_usuario)
         VALUES ($1, $2, $3, $4, $5, true)
         RETURNING id",
        &[&id_revision, &id_rp, &numero, &inicio, &fin],
    )
    .expect("crear obra de prueba")
    .get(0)
}

fn mover_obra(tx: &mut Transaction<'_>, id_obra: Uuid, inicio: NaiveDate, fin: NaiveDate) {
    tx.execute(
        "UPDATE public.registros_obras SET fecha_inicio = $2, fecha_fin = $3 WHERE id = $1",
        &[&id_obra, &inicio, &fin],
    )
    .expect("mover obra de prueba");
}

fn estado(tx: &mut Transaction<'_>, id_nomina: Uuid) -> Option<String> {
    tx.query(
        "SELECT estado FROM public.reportes_nominas WHERE id_nomina = $1",
        &[&id_nomina],
    )
    .expect("consultar estado del reporte")
    .first()
    .map(|r| r.get(0))
}

fn contar(tx: &mut Transaction<'_>, sql: &str, id: Uuid) -> i64 {
    tx.query_one(sql, &[&id]).expect("contar filas de prueba").get(0)
}

fn sync(tx: &mut Transaction<'_>, id_revision: Uuid) -> SyncNominasResumen {
    sync_reportes_nominas_revision_tx(tx, id_revision).expect("sync_reportes_nominas_revision_tx")
}

/// Cédula de liquidaciones (regla D-2): liquidación del empleado en `mes/anio` con un tramo de
/// `dias` asignado a `obra`. Sin este tramo la nómina no entra al alcance de la revisión.
fn asignar_cedula(
    tx: &mut Transaction<'_>,
    rp: Uuid,
    rev: Uuid,
    emp: Uuid,
    obra: Uuid,
    anio: i16,
    mes: i16,
    mes_nombre: &str,
    dias: i16,
) {
    let id_liquidacion: Uuid = tx
        .query_one(
            "INSERT INTO public.liquidaciones
                (id_registro_patronal, id_empleado, mes_periodo, mes_periodo_int, anio_periodo, nombre_archivo)
             VALUES ($1, $2, $3, $4, $5, 'PRUEBA_NOMINAS.xlsx')
             RETURNING id",
            &[&rp, &emp, &mes_nombre, &mes, &anio],
        )
        .expect("crear liquidación de prueba")
        .get(0);

    tx.execute(
        "INSERT INTO public.detalles_liquidaciones (id_liquidacion, clave, fecha, dias, sdi, cf, patronal, obrera, subtotal)
         VALUES ($1, 'PRUEBA', $2, $3, 300.0, 10.0, 15.0, 5.0, 15.0)",
        &[&id_liquidacion, &d(anio as i32, mes as u32, 15), &dias],
    )
    .expect("crear detalle de liquidación de prueba");

    sync_reportes_liquidaciones_for_revision_tx(tx, rev).expect("sync de reportes de liquidaciones");

    tx.execute(
        "UPDATE public.reportes_detalles_liquidaciones SET id_registro_obra = $1, dias_registro_obra = $2
         WHERE id_reporte_liquidacion IN (
             SELECT id FROM public.reportes_liquidaciones WHERE id_revision = $3 AND id_empleado = $4
         )",
        &[&obra, &dias, &rev, &emp],
    )
    .expect("asignar tramo de cédula a la obra");
}

#[test]
#[ignore]
fn i2_sync_es_idempotente_y_respeta_ediciones() {
    let mut client = conectar();
    let mut tx = client.transaction().unwrap();

    let (_c, rp, rev, emp) = crear_contexto(&mut tx, "I2");
    let obra = crear_obra(&mut tx, rp, rev, "OBRA-I2", d(2026, 3, 1), d(2026, 3, 31));
    asignar_cedula(&mut tx, rp, rev, emp, obra, 2026, 3, "MARZO", 15);
    let a = crear_nomina(&mut tx, rp, rev, emp, d(2026, 3, 1), d(2026, 3, 15));
    let b = crear_nomina(&mut tx, rp, rev, emp, d(2026, 6, 1), d(2026, 6, 15));

    let primera = sync(&mut tx, rev);
    assert!(primera.insertadas >= 1, "la primera sincronización debe insertar reportes: {primera:?}");
    assert_eq!(estado(&mut tx, a).as_deref(), Some("ACTIVO"), "nómina dentro del periodo debe quedar ACTIVO");
    assert_ne!(estado(&mut tx, b).as_deref(), Some("ACTIVO"), "nómina fuera de la obra no debe quedar ACTIVO");
    let filas_primera: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM public.reportes_nominas WHERE id_revision = $1",
            &[&rev],
        )
        .unwrap()
        .get(0);

    tx.execute(
        "UPDATE public.reportes_nominas SET num_dias_pagados = 7, editado_usuario = true WHERE id_nomina = $1",
        &[&a],
    )
    .unwrap();

    let segunda = sync(&mut tx, rev);
    assert_eq!(segunda, SyncNominasResumen::default(), "la segunda sincronización no debe cambiar nada");

    let dias: f64 = tx
        .query_one(
            "SELECT num_dias_pagados FROM public.reportes_nominas WHERE id_nomina = $1",
            &[&a],
        )
        .unwrap()
        .get(0);
    assert_eq!(dias, 7.0, "la edición del analista debe conservarse");
    assert_eq!(estado(&mut tx, a).as_deref(), Some("ACTIVO"));

    let filas_segunda: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM public.reportes_nominas WHERE id_revision = $1",
            &[&rev],
        )
        .unwrap()
        .get(0);
    assert_eq!(filas_primera, filas_segunda, "el número de reportes no debe cambiar entre sincronizaciones");

    tx.rollback().unwrap();
}

#[test]
#[ignore]
fn alcance_se_marca_y_se_revierte_sin_borrar() {
    let mut client = conectar();
    let mut tx = client.transaction().unwrap();

    let (_c, rp, rev, emp) = crear_contexto(&mut tx, "ALC");
    let obra = crear_obra(&mut tx, rp, rev, "OBRA-ALC", d(2026, 3, 1), d(2026, 3, 31));
    asignar_cedula(&mut tx, rp, rev, emp, obra, 2026, 3, "MARZO", 15);
    let a = crear_nomina(&mut tx, rp, rev, emp, d(2026, 3, 1), d(2026, 3, 15));

    sync(&mut tx, rev);
    assert_eq!(estado(&mut tx, a).as_deref(), Some("ACTIVO"));

    // Sintética editada por el usuario, con fechas en junio (sin nómina asociada).
    let sintetica: Uuid = tx
        .query_one(
            "INSERT INTO public.reportes_nominas
                (id_nomina, id_registro_patronal, id_revision, id_empleado,
                 fecha_inicial_pago, fecha_final_pago, fecha_pago, sintetica, editado_usuario)
             VALUES (NULL, $1, $2, $3, $4, $5, $5, true, true)
             RETURNING id",
            &[&rp, &rev, &emp, &d(2026, 6, 1), &d(2026, 6, 15)],
        )
        .unwrap()
        .get(0);

    // Mover la obra fuera de marzo: la nómina debe marcarse FUERA_ALCANCE sin borrarse.
    mover_obra(&mut tx, obra, d(2026, 5, 1), d(2026, 5, 31));
    let fuera = sync(&mut tx, rev);
    assert!(fuera.a_fuera_alcance >= 1, "debe marcar al menos una nómina fuera de alcance: {fuera:?}");
    assert_eq!(estado(&mut tx, a).as_deref(), Some("FUERA_ALCANCE"));
    assert_eq!(
        contar(&mut tx, "SELECT COUNT(*) FROM public.reportes_nominas WHERE id_nomina = $1", a),
        1,
        "la fila fuera de alcance no debe borrarse"
    );

    let estado_sintetica: String = tx
        .query_one(
            "SELECT estado FROM public.reportes_nominas WHERE id = $1",
            &[&sintetica],
        )
        .unwrap()
        .get(0);
    assert_eq!(estado_sintetica, "ACTIVO", "una sintética editada no debe marcarse fuera de alcance");

    // Restaurar la obra: la nómina debe volver a ACTIVO.
    mover_obra(&mut tx, obra, d(2026, 3, 1), d(2026, 3, 31));
    let activo = sync(&mut tx, rev);
    assert!(activo.a_activo >= 1, "debe reactivar al menos una nómina: {activo:?}");
    assert_eq!(estado(&mut tx, a).as_deref(), Some("ACTIVO"));

    tx.rollback().unwrap();
}

#[test]
#[ignore]
fn i3_borrar_nominas_de_una_revision_no_toca_otra() {
    let mut client = conectar();
    let mut tx = client.transaction().unwrap();

    let (_ca, rp_a, rev_a, emp_a) = crear_contexto(&mut tx, "I3A");
    let obra_a = crear_obra(&mut tx, rp_a, rev_a, "OBRA-I3A", d(2026, 3, 1), d(2026, 3, 31));
    asignar_cedula(&mut tx, rp_a, rev_a, emp_a, obra_a, 2026, 3, "MARZO", 15);
    crear_nomina(&mut tx, rp_a, rev_a, emp_a, d(2026, 3, 1), d(2026, 3, 15));

    let (_cb, rp_b, rev_b, emp_b) = crear_contexto(&mut tx, "I3B");
    let obra_b = crear_obra(&mut tx, rp_b, rev_b, "OBRA-I3B", d(2026, 3, 1), d(2026, 3, 31));
    asignar_cedula(&mut tx, rp_b, rev_b, emp_b, obra_b, 2026, 3, "MARZO", 15);
    crear_nomina(&mut tx, rp_b, rev_b, emp_b, d(2026, 3, 1), d(2026, 3, 15));

    sync(&mut tx, rev_a);
    sync(&mut tx, rev_b);

    let borradas = delete_nominas_de_revision_tx(&mut tx, rev_a).expect("borrado transaccional");
    assert!(borradas > 0, "el borrado debe eliminar filas de la revisión A");

    let nominas_a = contar(&mut tx, "SELECT COUNT(*) FROM public.nominas WHERE id_revision = $1", rev_a);
    let nominas_b = contar(&mut tx, "SELECT COUNT(*) FROM public.nominas WHERE id_revision = $1", rev_b);
    let reportes_b = contar(&mut tx, "SELECT COUNT(*) FROM public.reportes_nominas WHERE id_revision = $1", rev_b);
    assert_eq!(nominas_a, 0, "no deben quedar nóminas de la revisión A");
    assert_eq!(nominas_b, 1, "las nóminas de la revisión B no deben tocarse");
    assert_eq!(reportes_b, 1, "los reportes de la revisión B no deben tocarse");

    tx.rollback().unwrap();
}

#[test]
#[ignore]
fn revision_sin_obras_de_usuario_no_inserta_nominas() {
    let mut client = conectar();
    let mut tx = client.transaction().unwrap();

    // Sin obras de usuario no hay comodín: la nómina del RP queda fuera del alcance de la revisión.
    let (_c, rp, rev, emp) = crear_contexto(&mut tx, "SINOBRA");
    crear_nomina(&mut tx, rp, rev, emp, d(2026, 3, 1), d(2026, 3, 15));

    let resumen = sync(&mut tx, rev);
    assert_eq!(resumen.insertadas, 0, "sin obras de usuario no debe insertarse ninguna nómina: {resumen:?}");
    assert_eq!(
        contar(&mut tx, "SELECT COUNT(*) FROM public.reportes_nominas WHERE id_revision = $1", rev),
        0,
        "el reporte de la revisión debe quedar vacío"
    );

    tx.rollback().unwrap();
}

#[test]
fn i1_liquidaciones_no_tocan_nominas() {
    let liquidaciones = [
        ("reporte_liquidaciones.rs", include_str!("reporte_liquidaciones.rs")),
        ("asignacion_ops.rs", include_str!("asignacion_ops.rs")),
        ("bitacora_reportes.rs", include_str!("bitacora_reportes.rs")),
    ];
    for (nombre, src) in liquidaciones {
        assert!(!src.contains("nominas_reporte::"), "{nombre} no debe usar nominas_reporte::");
        assert!(
            !src.contains("DELETE FROM public.reportes_nominas"),
            "{nombre} no debe borrar public.reportes_nominas"
        );
    }

    let nominas = include_str!("nominas.rs");
    assert!(!nominas.contains("DELETE FROM public.nominas\""), "nominas.rs no debe borrar todas las nóminas");
    assert!(
        !nominas.contains(".execute(\"DELETE FROM public.nominas\", &[])"),
        "nominas.rs no debe ejecutar un DELETE sin filtro sobre public.nominas"
    );
}
