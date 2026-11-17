//! Pruebas de integración del tope de días de obra (FIX-05) contra la BD configurada.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. Cada prueba corre dentro de UNA sola
//! transacción que se revierte al final (`tx.rollback()`; si la prueba hace panic, Drop revierte).
//! Nunca se hace COMMIT. Solo se ejecutan explícitamente con `cargo test -- --ignored`.

use chrono::NaiveDate;
use postgres::{Client, Transaction};
use uuid::Uuid;

use crate::db::bitacora_reportes::{cerrar_edicion, iniciar_edicion, ACCION_DIAS};
use crate::db::client::create_client;
use crate::db::revisiones::{create_obra_tx, RegistroObraItem};
use crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx;
use crate::db::schema::{ensure_schema, format_pg_error};

fn d(anio: i32, mes: u32, dia: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(anio, mes, dia).expect("fecha de prueba inválida")
}

fn conectar() -> Client {
    let mut client = create_client()
        .unwrap_or_else(|e| panic!("No se pudo conectar a PostgreSQL para pruebas de topes: {e}"));
    ensure_schema(&mut client)
        .unwrap_or_else(|e| panic!("ensure_schema falló: {}", format_pg_error(&e)));
    client
}

/// Cliente, registro patronal, revisión y empleado temporales dentro de la transacción.
fn crear_contexto(tx: &mut Transaction<'_>, sufijo: &str) -> (Uuid, Uuid, Uuid) {
    let uuid = Uuid::now_v7();
    let nombre_cliente = format!("ZZ PRUEBA TOPES {} {}", sufijo, uuid.simple());
    let registro = format!("T{:010}", uuid.as_u128() % 10_000_000_000);

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
            &[&id_rp, &format!("ZZ REVISION TOPES {}", uuid.simple())],
        )
        .expect("crear revisión de prueba")
        .get(0);

    let id_empleado: Uuid = tx
        .query_one(
            "INSERT INTO public.empleados (id_cliente, nombre, nss) VALUES ($1, $2, $3) RETURNING id",
            &[&id_cliente, &"PRUEBA TOPES 1", &"12345678903"],
        )
        .expect("crear empleado de prueba")
        .get(0);

    (id_rp, id_revision, id_empleado)
}

/// Liquidación de enero de 2025 con un único detalle (cédula) de `dias`.
fn crear_liquidacion(tx: &mut Transaction<'_>, id_rp: Uuid, id_empleado: Uuid, dias: i16) -> Uuid {
    let id_liquidacion: Uuid = tx
        .query_one(
            "INSERT INTO public.liquidaciones
                (id_registro_patronal, id_empleado, mes_periodo, mes_periodo_int, anio_periodo, nombre_archivo)
             VALUES ($1, $2, 'ENERO', 1, 2025, 'PRUEBA_TOPES.xlsx')
             RETURNING id",
            &[&id_rp, &id_empleado],
        )
        .expect("crear liquidación de prueba")
        .get(0);

    tx.execute(
        "INSERT INTO public.detalles_liquidaciones (id_liquidacion, clave, fecha, dias, sdi, cf, patronal, obrera, subtotal)
         VALUES ($1, 'PRUEBA', $2, $3, 300.0, 10.0, 5.0, 5.0, 10.0)",
        &[&id_liquidacion, &d(2025, 1, 15), &dias],
    )
    .expect("crear detalle de liquidación de prueba");

    id_liquidacion
}

fn item_obra(id_revision: Uuid, id_rp: Uuid, numero: &str, inicio: NaiveDate, fin: NaiveDate) -> RegistroObraItem {
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

/// Asigna `dias` de obra al único tramo del reporte dentro de un lote DIAS y cierra el lote.
fn editar_dias_obra(
    tx: &mut Transaction<'_>,
    id_reporte: Uuid,
    id_obra: Uuid,
    dias: i16,
) -> Result<u64, String> {
    let lote = iniciar_edicion(tx, ACCION_DIAS, &[id_reporte])?;
    tx.execute(
        "UPDATE public.reportes_detalles_liquidaciones SET id_registro_obra = $1, dias_registro_obra = $2
         WHERE id_reporte_liquidacion = $3",
        &[&id_obra, &dias, &id_reporte],
    )
    .expect("asignar días de obra al tramo");
    cerrar_edicion(tx, lote, true)
}

#[test]
#[ignore]
fn i7_tope_de_dias_de_obra_al_editar() {
    let mut client = conectar();
    let mut tx = client.transaction().unwrap();

    // Empleado con 30 días en enero 2025 y obra del 16 al 31 de enero (vigencia = 16 días).
    let (rp, rev, emp) = crear_contexto(&mut tx, "I7");
    crear_liquidacion(&mut tx, rp, emp, 30);
    let obra = create_obra_tx(&mut tx, &item_obra(rev, rp, "C7000007", d(2025, 1, 16), d(2025, 1, 31)))
        .expect("crear obra de prueba");
    let id_obra: Uuid = obra.id.parse().expect("id de obra");

    sync_reportes_liquidaciones_for_revision_tx(&mut tx, rev).expect("sync de reportes");
    let id_reporte: Uuid = tx
        .query_one(
            "SELECT id FROM public.reportes_liquidaciones WHERE id_revision = $1",
            &[&rev],
        )
        .expect("consultar reporte de prueba")
        .get(0);

    // 20 días superan el tope de 16: el cierre debe fallar y la violación debe revertirse.
    {
        let mut sp = tx.savepoint("i7_exceso").expect("savepoint");
        let err = editar_dias_obra(&mut sp, id_reporte, id_obra, 20)
            .expect_err("20 días de obra con vigencia de 16 deben rechazarse");
        assert!(err.to_lowercase().contains("tope"), "el error debe mencionar el tope: {err}");
        sp.rollback().expect("revertir savepoint del intento fallido");
    }

    // 16 días sí caben y el cierre es Ok.
    let afectados = editar_dias_obra(&mut tx, id_reporte, id_obra, 16)
        .expect("16 días de obra deben aceptarse");
    assert_eq!(afectados, 1);

    tx.rollback().unwrap();
}
