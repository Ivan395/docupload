//! Pruebas de integración del histórico empleado × obra (D-10 b) y de la baja de cédula (D-8)
//! contra la BD configurada.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. Cada prueba corre dentro de UNA sola
//! transacción que se revierte al final (`tx.rollback()`; si la prueba hace panic, Drop revierte).
//! Nunca se hace COMMIT. Solo se ejecutan explícitamente con `cargo test -- --ignored`.

use chrono::NaiveDate;
use postgres::Transaction;
use uuid::Uuid;

use crate::db::catalogos::{estado_empleado_efectivo, fetch_empleados_by_cliente_con};
use crate::db::client::create_client;
use crate::db::historial_empleados::{compactar_periodos, fetch_tramos_historial};
use crate::db::schema::{ensure_schema, format_pg_error};

fn d(anio: i32, mes: u32, dia: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(anio, mes, dia).expect("fecha de prueba inválida")
}

/// Cliente, registro patronal, revisión y empleado temporales dentro de la transacción.
fn crear_contexto(tx: &mut Transaction<'_>) -> (Uuid, Uuid, Uuid, Uuid, String) {
    let uuid = Uuid::now_v7();
    let nombre_cliente = format!("ZZ PRUEBA HISTORIAL {}", uuid.simple());
    let registro = format!("H{:010}", uuid.as_u128() % 10_000_000_000);

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
            &[&id_rp, &format!("ZZ REVISION HISTORIAL {}", uuid.simple())],
        )
        .expect("crear revisión de prueba")
        .get(0);

    let id_empleado: Uuid = tx
        .query_one(
            "INSERT INTO public.empleados (id_cliente, nombre, nss) VALUES ($1, 'PRUEBA HISTORIAL 1', '12345678901') RETURNING id",
            &[&id_cliente],
        )
        .expect("crear empleado de prueba")
        .get(0);

    (id_cliente, id_rp, id_revision, id_empleado, registro)
}

/// Liquidación (cédula) del mes con un detalle de días; `extra` agrega un movimiento (p. ej. BAJA).
fn crear_liquidacion(
    tx: &mut Transaction<'_>,
    id_rp: Uuid,
    id_empleado: Uuid,
    mes: i16,
    anio: i16,
    nombre_mes: &str,
    extra: Option<(&str, NaiveDate)>,
) {
    let id_liq: Uuid = tx
        .query_one(
            "INSERT INTO public.liquidaciones
                (id_registro_patronal, id_empleado, mes_periodo, mes_periodo_int, anio_periodo, nombre_archivo)
             VALUES ($1, $2, $3, $4, $5, 'PRUEBA_HISTORIAL.xlsx') RETURNING id",
            &[&id_rp, &id_empleado, &nombre_mes, &mes, &anio],
        )
        .expect("crear liquidación de prueba")
        .get(0);
    let fecha_prueba = d(i32::from(anio), mes as u32, 1);
    tx.execute(
        "INSERT INTO public.detalles_liquidaciones (id_liquidacion, clave, fecha, dias) VALUES ($1, 'PRUEBA', $2, 30)",
        &[&id_liq, &fecha_prueba],
    )
    .expect("crear detalle de prueba");
    if let Some((clave, fecha)) = extra {
        tx.execute(
            "INSERT INTO public.detalles_liquidaciones (id_liquidacion, clave, fecha, dias) VALUES ($1, $2, $3, 0)",
            &[&id_liq, &clave, &fecha],
        )
        .expect("crear movimiento de cédula de prueba");
    }
}

/// Reporte ACTIVO del empleado para el mes, ligado a la liquidación del mismo mes.
fn crear_reporte(
    tx: &mut Transaction<'_>,
    id_rp: Uuid,
    id_revision: Uuid,
    id_empleado: Uuid,
    mes: i16,
    anio: i16,
    nombre_mes: &str,
) -> Uuid {
    tx.query_one(
        "INSERT INTO public.reportes_liquidaciones
            (id_registro_patronal, id_revision, id_empleado, mes_periodo, mes_periodo_int, anio_periodo, nombre_archivo)
         VALUES ($1, $2, $3, $4, $5, $6, 'PRUEBA_HISTORIAL.xlsx') RETURNING id",
        &[&id_rp, &id_revision, &id_empleado, &nombre_mes, &mes, &anio],
    )
    .expect("crear reporte de liquidación de prueba")
    .get(0)
}

/// Detalle del reporte que asigna `dias` a la obra.
fn asignar_obra(tx: &mut Transaction<'_>, id_reporte: Uuid, id_obra: Uuid, dias: i16, fecha: NaiveDate) {
    tx.execute(
        "INSERT INTO public.reportes_detalles_liquidaciones
            (id_reporte_liquidacion, clave, fecha, dias, id_registro_obra, dias_registro_obra)
         VALUES ($1, 'PRUEBA', $2, $3, $4, $3)",
        &[&id_reporte, &fecha, &dias, &id_obra],
    )
    .expect("asignar obra al reporte de prueba");
}

fn crear_obra(
    tx: &mut Transaction<'_>,
    id_revision: Uuid,
    id_rp: Uuid,
    numero: &str,
    inicio: NaiveDate,
    fin: NaiveDate,
) -> Uuid {
    tx.query_one(
        "INSERT INTO public.registros_obras (id_revision, id_registro_patronal, numero, fecha_inicio, fecha_fin)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
        &[&id_revision, &id_rp, &numero, &inicio, &fin],
    )
    .expect("crear obra de prueba")
    .get(0)
}

fn conectar() -> postgres::Client {
    let mut client = create_client()
        .unwrap_or_else(|e| panic!("No se pudo conectar a PostgreSQL para pruebas de historial: {e}"));
    ensure_schema(&mut client)
        .unwrap_or_else(|e| panic!("ensure_schema falló: {}", format_pg_error(&e)));
    client
}

/// I-9: empleado con enero y febrero 2025, baja el 15/02; dos obras (A todo el año, B desde el 16/01).
#[test]
#[ignore]
fn i9_historial_empleado_obra_con_baja() {
    let mut client = conectar();
    let mut tx = client.transaction().expect("iniciar transacción de prueba");

    let (id_cliente, id_rp, id_revision, id_empleado, registro) = crear_contexto(&mut tx);

    let obra_a = crear_obra(&mut tx, id_revision, id_rp, "ZZ-HIST-A", d(2025, 1, 1), d(2025, 12, 31));
    let obra_b = crear_obra(&mut tx, id_revision, id_rp, "ZZ-HIST-B", d(2025, 1, 16), d(2025, 12, 31));

    // Cédulas: enero sin movimientos; febrero con BAJA el 15/02/2025.
    crear_liquidacion(&mut tx, id_rp, id_empleado, 1, 2025, "ENERO", None);
    crear_liquidacion(
        &mut tx,
        id_rp,
        id_empleado,
        2,
        2025,
        "FEBRERO",
        Some(("BAJA", d(2025, 2, 15))),
    );

    // Reportes ACTIVO: enero A=15 y B=15; febrero A=15 (la baja queda en el detalle de febrero).
    let rep_ene = crear_reporte(&mut tx, id_rp, id_revision, id_empleado, 1, 2025, "ENERO");
    asignar_obra(&mut tx, rep_ene, obra_a, 15, d(2025, 1, 31));
    asignar_obra(&mut tx, rep_ene, obra_b, 15, d(2025, 1, 31));

    let rep_feb = crear_reporte(&mut tx, id_rp, id_revision, id_empleado, 2, 2025, "FEBRERO");
    asignar_obra(&mut tx, rep_feb, obra_a, 15, d(2025, 2, 28));
    tx.execute(
        "INSERT INTO public.reportes_detalles_liquidaciones (id_reporte_liquidacion, clave, fecha, dias)
         VALUES ($1, 'BAJA', $2, 0)",
        &[&rep_feb, &d(2025, 2, 15)],
    )
    .expect("movimiento BAJA en reporte de febrero");

    // Histórico: tramos y periodos compactados.
    let tramos = fetch_tramos_historial(&mut tx, id_empleado, Some(id_revision))
        .expect("fetch_tramos_historial");
    assert_eq!(tramos.len(), 3, "tramos: {:?}", tramos);

    let periodos = compactar_periodos(&tramos);
    assert_eq!(periodos.len(), 2, "periodos: {:?}", periodos);

    let per_a = periodos
        .iter()
        .find(|p| p.numero == "ZZ-HIST-A")
        .expect("periodo de obra A");
    assert_eq!(per_a.inicio, d(2025, 1, 1));
    assert_eq!(per_a.fin, d(2025, 2, 15));
    assert_eq!(per_a.dias_obra, 30);

    let per_b = periodos
        .iter()
        .find(|p| p.numero == "ZZ-HIST-B")
        .expect("periodo de obra B");
    assert_eq!(per_b.inicio, d(2025, 1, 16));
    assert_eq!(per_b.fin, d(2025, 1, 31));
    assert_eq!(per_b.dias_obra, 15);

    // Catálogo de empleados: baja 15/02 desde la cédula; RP único; obras del último mes (febrero).
    let lista = fetch_empleados_by_cliente_con(&mut tx, &id_cliente.to_string())
        .expect("fetch_empleados_by_cliente_con");
    let emp = lista
        .iter()
        .find(|e| e.id == id_empleado.to_string())
        .expect("empleado de prueba en el catálogo");
    assert_eq!(emp.activo_segun_cedula, Some(false));
    assert_eq!(emp.baja_fecha, Some(d(2025, 2, 15)));
    assert_eq!(
        estado_empleado_efectivo(emp),
        (false, "Baja 15/02/2025".to_string())
    );
    assert_eq!(emp.registro_patronal, registro);
    assert_eq!(emp.obras_actuales, 1);
    assert_eq!(emp.ultima_obra, "ZZ-HIST-A");

    tx.rollback().expect("revertir transacción de prueba");
}
