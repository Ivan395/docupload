//! Pruebas de integración de analistas (D-6 a): bloqueo optimista y deshacer solo lo propio.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. Cada prueba corre dentro de UNA sola
//! transacción que se revierte al final (`tx.rollback()`; si la prueba hace panic, Drop revierte).
//! Nunca se hace COMMIT. Solo se ejecutan explícitamente con `cargo test -- --ignored`.
//! El registro de versiones y el analista actual son globales de proceso: usar `--test-threads=1`.

use chrono::NaiveDate;
use postgres::Transaction;
use uuid::Uuid;

use crate::db::analistas::{
    crear_o_obtener_analista, fijar_analista_actual, olvidar_versiones, registrar_versiones,
    Analista,
};
use crate::db::bitacora_reportes::{
    cerrar_edicion, deshacer_lote_tx, iniciar_edicion, lote_tiene_cambios_posteriores_de_otro,
    ACCION_DIAS,
};
use crate::db::client::create_client;
use crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx;
use crate::db::revisiones::{create_obra_tx, RegistroObraItem};
use crate::db::schema::{ensure_schema, format_pg_error};

fn d(anio: i32, mes: u32, dia: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(anio, mes, dia).expect("fecha de prueba inválida")
}

fn conectar() -> postgres::Client {
    let mut client = create_client()
        .unwrap_or_else(|e| panic!("No se pudo conectar a PostgreSQL para pruebas de analistas: {e}"));
    ensure_schema(&mut client)
        .unwrap_or_else(|e| panic!("ensure_schema falló: {}", format_pg_error(&e)));
    client
}

/// Cliente, registro patronal, revisión, empleado, liquidación de enero, obra que cubre enero y
/// reporte resultante. Todo dentro de la transacción.
fn crear_contexto_con_reporte(tx: &mut Transaction<'_>, sufijo: &str) -> (Uuid, Uuid) {
    let uuid = Uuid::now_v7();
    let nombre_cliente = format!("ZZ PRUEBA ANALISTAS {} {}", sufijo, uuid.simple());
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
            &[&id_rp, &format!("ZZ REVISION ANALISTAS {}", uuid.simple())],
        )
        .expect("crear revisión de prueba")
        .get(0);
    let id_empleado: Uuid = tx
        .query_one(
            "INSERT INTO public.empleados (id_cliente, nombre, nss) VALUES ($1, $2, $3) RETURNING id",
            &[&id_cliente, &"PRUEBA ANALISTAS 1", &"12345678903"],
        )
        .expect("crear empleado de prueba")
        .get(0);

    let id_liquidacion: Uuid = tx
        .query_one(
            "INSERT INTO public.liquidaciones
                (id_registro_patronal, id_empleado, mes_periodo, mes_periodo_int, anio_periodo, nombre_archivo)
             VALUES ($1, $2, 'ENERO', 1, 2025, 'PRUEBA_ANALISTAS.xlsx')
             RETURNING id",
            &[&id_rp, &id_empleado],
        )
        .expect("crear liquidación de prueba")
        .get(0);
    tx.execute(
        "INSERT INTO public.detalles_liquidaciones (id_liquidacion, clave, fecha, dias, sdi, cf, patronal, obrera, subtotal)
         VALUES ($1, 'PRUEBA', $2, 30, 300.0, 10.0, 1000.0, 5.0, 1000.0)",
        &[&id_liquidacion, &d(2025, 1, 15)],
    )
    .expect("crear detalle de liquidación de prueba");

    // Obra que cubre enero: la revisión no tiene periodo, así que el universo son las obras.
    create_obra_tx(
        tx,
        &RegistroObraItem {
            id: String::new(),
            id_revision: id_revision.to_string(),
            id_registro_patronal: id_rp.to_string(),
            numero: "C7000010".to_string(),
            clase: "A".to_string(),
            tipo_obra: "OBRA DE PRUEBA".to_string(),
            fecha_inicio: Some(d(2025, 1, 1)),
            fecha_fin: Some(d(2025, 1, 31)),
            ubicacion: "PRUEBA".to_string(),
            superficie_construido: 0.0,
            monto: None,
            delegacion: "PRUEBA".to_string(),
            subdelegacion: "PRUEBA".to_string(),
            created_at: String::new(),
            creado_usuario: true,
        },
    )
    .expect("crear obra de prueba");

    sync_reportes_liquidaciones_for_revision_tx(tx, id_revision).expect("sync de reportes");

    let id_reporte: Uuid = tx
        .query_one(
            "SELECT id FROM public.reportes_liquidaciones WHERE id_liquidacion = $1",
            &[&id_liquidacion],
        )
        .expect("consultar reporte de prueba")
        .get(0);
    (id_revision, id_reporte)
}

fn version_de(tx: &mut Transaction<'_>, id_reporte: Uuid) -> i32 {
    tx.query_one(
        "SELECT version FROM public.reportes_liquidaciones WHERE id = $1",
        &[&id_reporte],
    )
    .expect("consultar versión del reporte")
    .get(0)
}

/// I-10: si otro analista ya avanzó la versión del reporte, la edición se rechaza; con la versión
/// registrada al día, se permite.
#[test]
#[ignore]
fn i10_bloqueo_optimista_rechaza_version_cambiada() {
    olvidar_versiones();
    let mut client = conectar();
    let mut tx = client.transaction().expect("iniciar transacción");

    let (_rev, rep) = crear_contexto_con_reporte(&mut tx, "I10");

    // Esta instancia vio la versión 0 al cargar el reporte.
    registrar_versiones(&[(rep, 0)]);
    // Otro analista modificó el reporte en BD.
    tx.execute(
        "UPDATE public.reportes_liquidaciones SET version = 1 WHERE id = $1",
        &[&rep],
    )
    .expect("simular edición de otro analista");

    let err = iniciar_edicion(&mut tx, ACCION_DIAS, &[rep])
        .expect_err("debe rechazar: la versión cambió");
    assert!(err.contains("Otro analista"), "mensaje inesperado: {err}");

    // Al recargar, la instancia registra la versión actual y la edición procede.
    registrar_versiones(&[(rep, version_de(&mut tx, rep))]);
    let lote = iniciar_edicion(&mut tx, ACCION_DIAS, &[rep]).expect("con versión al día debe permitir");
    assert_ne!(lote, Uuid::nil());

    olvidar_versiones();
    tx.rollback().expect("rollback de la prueba");
}

/// FIX-08: con el analista A en sesión, no se puede deshacer un lote de A si después otro analista
/// B modificó el mismo empleado-mes. Además, la autoría (id_analista) queda registrada en el lote.
#[test]
#[ignore]
fn i11_deshacer_bloqueado_por_cambio_posterior_de_otro_analista() {
    olvidar_versiones();
    let mut client = conectar();
    let mut tx = client.transaction().expect("iniciar transacción");

    let (_rev, rep) = crear_contexto_con_reporte(&mut tx, "I11");
    let sufijo = Uuid::now_v7().simple().to_string();
    let ana: Analista = crear_o_obtener_analista(&mut tx, &format!("ZZ PRUEBA ANA {sufijo}"))
        .expect("crear analista A");
    let beto: Analista = crear_o_obtener_analista(&mut tx, &format!("ZZ PRUEBA BETO {sufijo}"))
        .expect("crear analista B");
    // Re-obtener el mismo nombre no duplica al analista.
    let ana_de_nuevo = crear_o_obtener_analista(&mut tx, &format!("  ZZ PRUEBA ANA {sufijo}  "))
        .expect("obtener analista A existente");
    assert_eq!(ana_de_nuevo.id, ana.id);

    // Lote del analista A (versión inicial registrada por la carga del reporte).
    registrar_versiones(&[(rep, version_de(&mut tx, rep))]);
    fijar_analista_actual(Some(ana.clone()));
    let lote_a = iniciar_edicion(&mut tx, ACCION_DIAS, &[rep]).expect("lote de A");
    cerrar_edicion(&mut tx, lote_a, true).expect("cerrar lote de A");

    // Hace que el lote de A sea anterior al de B (ambos se abren en la misma transacción).
    tx.execute(
        "UPDATE public.reportes_cambios SET created_at = created_at - interval '1 minute' WHERE lote_id = $1",
        &[&lote_a],
    )
    .expect("retrasar lote de A");

    // Lote posterior del analista B sobre el mismo reporte.
    fijar_analista_actual(Some(beto.clone()));
    let lote_b = iniciar_edicion(&mut tx, ACCION_DIAS, &[rep]).expect("lote de B");
    cerrar_edicion(&mut tx, lote_b, true).expect("cerrar lote de B");

    // Autoría registrada en el lote de A.
    let autor_a: Option<Uuid> = tx
        .query_one(
            "SELECT DISTINCT id_analista FROM public.reportes_cambios WHERE lote_id = $1",
            &[&lote_a],
        )
        .expect("consultar autoría del lote de A")
        .get(0);
    assert_eq!(autor_a, Some(ana.id), "el lote de A debe llevar el id del analista A");

    assert!(
        lote_tiene_cambios_posteriores_de_otro(&mut tx, lote_a).expect("consulta FIX-08"),
        "el lote de A tiene cambios posteriores de B"
    );

    fijar_analista_actual(Some(ana.clone()));
    let err = deshacer_lote_tx(&mut tx, lote_a).expect_err("A no debe poder deshacer sobre el trabajo de B");
    assert!(err.contains("otro analista"), "mensaje inesperado: {err}");

    fijar_analista_actual(None);
    olvidar_versiones();
    tx.rollback().expect("rollback de la prueba");
}
