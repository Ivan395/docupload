//! Pruebas de integración del alta masiva de revisiones contra la BD configurada.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. La prueba corre dentro de UNA sola
//! transacción que se revierte al final (`tx.rollback()`; si la prueba hace panic, Drop revierte).
//! Nunca se hace COMMIT. Solo se ejecutan explícitamente con `cargo test -- --ignored`.

use std::collections::HashSet;

use chrono::NaiveDate;
use postgres::{Client, Transaction};
use uuid::Uuid;

use crate::db::client::create_client;
use crate::db::revisiones::{crear_revisiones_lote_tx, fetch_revisiones_existentes};
use crate::db::schema::{ensure_schema, format_pg_error};
use crate::revisiones_lote_logic::{clasificar_revisiones, generar_por_prefijo, EstadoRevisionLote};

fn conectar() -> Client {
    let mut client = create_client()
        .unwrap_or_else(|e| panic!("No se pudo conectar a PostgreSQL para pruebas de revisiones: {e}"));
    ensure_schema(&mut client)
        .unwrap_or_else(|e| panic!("ensure_schema falló: {}", format_pg_error(&e)));
    client
}

/// Cliente y registro patronal temporales dentro de la transacción.
fn crear_registro_patronal(tx: &mut Transaction<'_>) -> Uuid {
    let uuid = Uuid::now_v7();
    let nombre_cliente = format!("ZZ PRUEBA REVISIONES LOTE {}", uuid.simple());
    let registro = format!("Y{:010}", uuid.as_u128() % 10_000_000_000);

    let id_cliente: Uuid = tx
        .query_one(
            "INSERT INTO public.clientes (nombre) VALUES ($1) RETURNING id",
            &[&nombre_cliente],
        )
        .expect("crear cliente de prueba")
        .get(0);

    tx.query_one(
        "INSERT INTO public.registros_patronales (id_cliente, registro) VALUES ($1, $2) RETURNING id",
        &[&id_cliente, &registro],
    )
    .expect("crear registro patronal de prueba")
    .get(0)
}

#[test]
#[ignore]
fn lote_generado_omite_nombre_existente_y_crea_el_resto() {
    let mut client = conectar();
    let mut tx = client.transaction().expect("iniciar transacción de prueba");
    let id_rp = crear_registro_patronal(&mut tx);

    // Revisión existente REV01 con folio F01.
    tx.execute(
        "INSERT INTO public.revisiones (id_registro_patronal, nombre, folio) VALUES ($1, 'REV01', 'F01')",
        &[&id_rp],
    )
    .expect("insertar revisión existente");

    let filas = generar_por_prefijo("REV", "F", 1, 3, 2).expect("generar lote");
    let existentes = fetch_revisiones_existentes(&mut tx, id_rp).expect("leer revisiones existentes");
    let previa = clasificar_revisiones(filas, &existentes);

    assert_eq!(previa[0].fila.nombre, "REV01");
    assert_eq!(previa[0].estado, EstadoRevisionLote::NombreExiste);
    assert!(!previa[0].incluir);
    assert_eq!(previa[1].estado, EstadoRevisionLote::Valida);
    assert!(previa[1].incluir);
    assert_eq!(previa[2].estado, EstadoRevisionLote::Valida);
    assert!(previa[2].incluir);

    let inhabiles: HashSet<NaiveDate> = HashSet::new();
    let resumen = crear_revisiones_lote_tx(&mut tx, id_rp, &previa, &inhabiles).expect("crear lote de revisiones");
    assert_eq!(resumen.creadas, vec!["REV02".to_string(), "REV03".to_string()]);
    assert_eq!(resumen.omitidas, 1);

    let total: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM public.revisiones WHERE id_registro_patronal = $1",
            &[&id_rp],
        )
        .expect("contar revisiones del registro patronal")
        .get(0);
    assert_eq!(total, 3, "el registro patronal debe tener 3 revisiones");

    let folio_rev02: Option<String> = tx
        .query_one(
            "SELECT TRIM(folio) FROM public.revisiones WHERE id_registro_patronal = $1 AND nombre = 'REV02'",
            &[&id_rp],
        )
        .expect("leer folio de REV02")
        .get(0);
    assert_eq!(folio_rev02.as_deref(), Some("F02"), "REV02 debe tener folio F02");

    tx.rollback().expect("revertir datos de prueba");
}
