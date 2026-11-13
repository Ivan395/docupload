//! Pruebas de integración de obras (registros de obra) contra la BD configurada.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. Cada prueba corre dentro de UNA sola
//! transacción que se revierte al final (`tx.rollback()`; si la prueba hace panic, Drop revierte).
//! Nunca se hace COMMIT. Solo se ejecutan explícitamente con `cargo test -- --ignored`.

use chrono::NaiveDate;
use postgres::{Client, Transaction};
use uuid::Uuid;

use crate::db::bitacora_reportes::{deshacer_lote_tx, ACCION_DESACTIVAR_OBRA};
use crate::db::client::create_client;
use crate::db::obras_ops::{desactivar_obra_tx, resincronizar_universo_tx, CambioUniverso};
use crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx;
use crate::db::revisiones::{create_obra_tx, update_obra_tx, RegistroObraItem};
use crate::db::schema::{ensure_schema, format_pg_error};

fn d(anio: i32, mes: u32, dia: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(anio, mes, dia).expect("fecha de prueba inválida")
}

fn conectar() -> Client {
    let mut client = create_client()
        .unwrap_or_else(|e| panic!("No se pudo conectar a PostgreSQL para pruebas de obras: {e}"));
    ensure_schema(&mut client)
        .unwrap_or_else(|e| panic!("ensure_schema falló: {}", format_pg_error(&e)));
    client
}

/// Crea cliente, registro patronal, revisión y un empleado temporales dentro de la transacción.
fn crear_contexto(tx: &mut Transaction<'_>, sufijo: &str) -> (Uuid, Uuid, Uuid, Uuid) {
    let uuid = Uuid::now_v7();
    let nombre_cliente = format!("ZZ PRUEBA OBRAS {} {}", sufijo, uuid.simple());
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
            &[&id_rp, &format!("ZZ REVISION OBRAS {}", uuid.simple())],
        )
        .expect("crear revisión de prueba")
        .get(0);

    let id_empleado = crear_empleado(tx, id_cliente, "PRUEBA OBRAS 1", "12345678901");

    (id_cliente, id_rp, id_revision, id_empleado)
}

fn crear_empleado(tx: &mut Transaction<'_>, id_cliente: Uuid, nombre: &str, nss: &str) -> Uuid {
    tx.query_one(
        "INSERT INTO public.empleados (id_cliente, nombre, nss) VALUES ($1, $2, $3) RETURNING id",
        &[&id_cliente, &nombre, &nss],
    )
    .expect("crear empleado de prueba")
    .get(0)
}

/// Registro patronal adicional del mismo cliente (para probar el RP incorrecto).
fn crear_otro_rp(tx: &mut Transaction<'_>, id_cliente: Uuid) -> Uuid {
    let uuid = Uuid::now_v7();
    let registro = format!("X{:010}", uuid.as_u128() % 10_000_000_000);
    tx.query_one(
        "INSERT INTO public.registros_patronales (id_cliente, registro) VALUES ($1, $2) RETURNING id",
        &[&id_cliente, &registro],
    )
    .expect("crear segundo registro patronal de prueba")
    .get(0)
}

/// Liquidación de un empleado para un mes/año, con un detalle (cédula) de `dias` y `subtotal`.
fn crear_liquidacion(
    tx: &mut Transaction<'_>,
    id_rp: Uuid,
    id_empleado: Uuid,
    anio: i16,
    mes: i16,
    mes_nombre: &str,
    fecha_detalle: NaiveDate,
    dias: i16,
    subtotal: f64,
) -> Uuid {
    let id_liquidacion: Uuid = tx
        .query_one(
            "INSERT INTO public.liquidaciones
                (id_registro_patronal, id_empleado, mes_periodo, mes_periodo_int, anio_periodo, nombre_archivo)
             VALUES ($1, $2, $3, $4, $5, 'PRUEBA_OBRAS.xlsx')
             RETURNING id",
            &[&id_rp, &id_empleado, &mes_nombre, &mes, &anio],
        )
        .expect("crear liquidación de prueba")
        .get(0);

    tx.execute(
        "INSERT INTO public.detalles_liquidaciones (id_liquidacion, clave, fecha, dias, sdi, cf, patronal, obrera, subtotal)
         VALUES ($1, 'PRUEBA', $2, $3, 300.0, 10.0, $4, 5.0, $4)",
        &[&id_liquidacion, &fecha_detalle, &dias, &subtotal],
    )
    .expect("crear detalle de liquidación de prueba");

    id_liquidacion
}

/// Item de obra con todos los campos obligatorios; `id` vacío para crear, UUID para editar.
fn item_obra(
    id: &str,
    id_revision: Uuid,
    id_rp: Uuid,
    numero: &str,
    inicio: Option<NaiveDate>,
    fin: Option<NaiveDate>,
) -> RegistroObraItem {
    RegistroObraItem {
        id: id.to_string(),
        id_revision: id_revision.to_string(),
        id_registro_patronal: id_rp.to_string(),
        numero: numero.to_string(),
        clase: "A".to_string(),
        tipo_obra: "OBRA DE PRUEBA".to_string(),
        fecha_inicio: inicio,
        fecha_fin: fin,
        ubicacion: "PRUEBA".to_string(),
        superficie_construido: 0.0,
        monto: None,
        delegacion: "PRUEBA".to_string(),
        subdelegacion: "PRUEBA".to_string(),
        created_at: String::new(),
        creado_usuario: true,
    }
}

fn estado_reporte_de_liquidacion(tx: &mut Transaction<'_>, id_liquidacion: Uuid) -> Option<String> {
    tx.query(
        "SELECT estado FROM public.reportes_liquidaciones WHERE id_liquidacion = $1",
        &[&id_liquidacion],
    )
    .expect("consultar estado del reporte de liquidación")
    .first()
    .and_then(|r| r.get::<_, Option<String>>(0))
}

fn contar(tx: &mut Transaction<'_>, sql: &str, id: Uuid) -> i64 {
    tx.query_one(sql, &[&id]).expect("contar filas de prueba").get(0)
}

#[test]
#[ignore]
fn i4_obra_invalida_rechazada() {
    let mut client = conectar();
    let mut tx = client.transaction().unwrap();

    let (id_cliente, rp, rev, _emp) = crear_contexto(&mut tx, "I4");
    let inicio = d(2025, 1, 1);
    let fin = d(2025, 1, 31);
    let numero = "C7000003";

    // Sin fecha final.
    let sin_fin = item_obra("", rev, rp, numero, Some(inicio), None);
    let err = create_obra_tx(&mut tx, &sin_fin)
        .expect_err("una obra sin fecha final debe ser rechazada");
    assert!(
        err.to_lowercase().contains("fecha final"),
        "el error debe mencionar la fecha final: {err}"
    );

    // Fin anterior al inicio.
    let fin_antes = item_obra("", rev, rp, numero, Some(inicio), Some(d(2024, 12, 31)));
    let err = create_obra_tx(&mut tx, &fin_antes)
        .expect_err("una obra con fin anterior al inicio debe ser rechazada");
    assert!(
        err.to_lowercase().contains("fecha final"),
        "el error debe mencionar la fecha final: {err}"
    );

    // Registro patronal distinto al de la revisión (segundo RP del mismo cliente).
    let otro_rp = crear_otro_rp(&mut tx, id_cliente);
    let rp_ajeno = item_obra("", rev, otro_rp, numero, Some(inicio), Some(fin));
    let err = create_obra_tx(&mut tx, &rp_ajeno)
        .expect_err("una obra con otro registro patronal debe ser rechazada");
    assert!(
        err.to_lowercase().contains("registro patronal"),
        "el error debe mencionar el registro patronal: {err}"
    );

    // Una obra válida sí se crea (control positivo).
    let valida = create_obra_tx(&mut tx, &item_obra("", rev, rp, numero, Some(inicio), Some(fin)))
        .expect("una obra válida debe crearse");
    assert!(!valida.id.is_empty(), "la obra creada debe devolver id");

    // El CHECK de fechas aplica a filas nuevas aunque esté NOT VALID.
    // Un error de SQL aborta la transacción, por eso se usa un savepoint.
    let mut sp = tx.savepoint("i4_check_fechas").expect("savepoint");
    let insert = sp.execute(
        "INSERT INTO public.registros_obras
            (id_revision, id_registro_patronal, numero, fecha_inicio, fecha_fin, creado_usuario)
         VALUES ($1, $2, 'C7000004', $3, $4, true)",
        &[&rev, &rp, &fin, &inicio],
    );
    assert!(
        insert.is_err(),
        "el INSERT directo con fecha_fin < fecha_inicio debe fallar por chk_registros_obras_fechas"
    );
    sp.rollback().expect("revertir savepoint");

    tx.rollback().unwrap();
}

#[test]
#[ignore]
fn i5_desactivar_obra_con_dos_empleados_y_deshacer() {
    let mut client = conectar();
    let mut tx = client.transaction().unwrap();

    let (id_cliente, rp, rev, emp1) = crear_contexto(&mut tx, "I5");
    let emp2 = crear_empleado(&mut tx, id_cliente, "PRUEBA OBRAS 2", "12345678902");

    let liq1 = crear_liquidacion(&mut tx, rp, emp1, 2025, 1, "ENERO", d(2025, 1, 15), 30, 1000.0);
    let liq2 = crear_liquidacion(&mut tx, rp, emp2, 2025, 1, "ENERO", d(2025, 1, 15), 30, 800.0);

    // Obra de usuario que cubre enero: la revisión no tiene periodo, así que el universo son las obras.
    let obra = create_obra_tx(
        &mut tx,
        &item_obra("", rev, rp, "C7000005", Some(d(2025, 1, 1)), Some(d(2025, 1, 31))),
    )
    .expect("crear obra de prueba");
    let id_obra: Uuid = obra.id.parse().expect("id de obra");

    sync_reportes_liquidaciones_for_revision_tx(&mut tx, rev).expect("sync de reportes");

    let id_reportes: Vec<Uuid> = tx
        .query(
            "SELECT id FROM public.reportes_liquidaciones WHERE id_revision = $1 ORDER BY id",
            &[&rev],
        )
        .expect("consultar reportes")
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(id_reportes.len(), 2, "deben existir dos reportes (uno por empleado)");
    assert_eq!(estado_reporte_de_liquidacion(&mut tx, liq1).as_deref(), Some("ACTIVO"));
    assert_eq!(estado_reporte_de_liquidacion(&mut tx, liq2).as_deref(), Some("ACTIVO"));

    // Asignar la obra a todos los tramos de ambos reportes.
    tx.execute(
        "UPDATE public.reportes_detalles_liquidaciones SET id_registro_obra = $1, dias_registro_obra = 30
         WHERE id_reporte_liquidacion = ANY($2)",
        &[&id_obra, &id_reportes],
    )
    .expect("asignar obra a tramos");

    let asignados = contar(
        &mut tx,
        "SELECT COUNT(*) FROM public.reportes_detalles_liquidaciones WHERE id_registro_obra = $1",
        id_obra,
    );
    assert_eq!(asignados, 2, "cada empleado debe tener un tramo asignado a la obra");

    // Desactivar la obra.
    let resultado = desactivar_obra_tx(&mut tx, id_obra).expect("desactivar obra");
    assert_eq!(resultado.reportes_afectados, 2, "deben afectarse los dos reportes: {resultado:?}");
    assert!(resultado.hay_bitacora, "debe quedar registro en la bitácora: {resultado:?}");

    let activa: bool = tx
        .query_one("SELECT activa FROM public.registros_obras WHERE id = $1", &[&id_obra])
        .expect("consultar obra")
        .get(0);
    assert!(!activa, "la obra debe quedar inactiva (borrado lógico)");

    let con_obra = contar(
        &mut tx,
        "SELECT COUNT(*) FROM public.reportes_detalles_liquidaciones WHERE id_registro_obra = $1",
        id_obra,
    );
    assert_eq!(con_obra, 0, "ningún tramo debe seguir apuntando a la obra desactivada");

    for id_reporte in &id_reportes {
        let (tramos, sin_obra): (i64, i64) = {
            let fila = tx
                .query_one(
                    "SELECT COUNT(*), COUNT(*) FILTER (WHERE id_registro_obra IS NULL)
                     FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion = $1",
                    &[id_reporte],
                )
                .expect("contar tramos del reporte");
            (fila.get(0), fila.get(1))
        };
        assert_eq!(tramos, 1, "el tramo del movimiento debe seguir existiendo (uno por movimiento)");
        assert_eq!(sin_obra, 1, "el tramo debe quedar con id_registro_obra IS NULL");

        // Invariante de dinero: los tramos no pueden sumar más que la cédula de su liquidación.
        let (suma_tramos, suma_cedula): (f64, f64) = {
            let fila = tx
                .query_one(
                    "SELECT
                        (SELECT COALESCE(SUM(rdl.subtotal), 0)::float8 FROM public.reportes_detalles_liquidaciones rdl
                           WHERE rdl.id_reporte_liquidacion = $1),
                        (SELECT COALESCE(SUM(dl.subtotal), 0)::float8 FROM public.detalles_liquidaciones dl
                           JOIN public.reportes_liquidaciones rl ON rl.id_liquidacion = dl.id_liquidacion
                           WHERE rl.id = $1)",
                    &[id_reporte],
                )
                .expect("sumar importes");
            (fila.get(0), fila.get(1))
        };
        assert!(
            suma_tramos <= suma_cedula + 0.01,
            "los tramos ({suma_tramos}) no deben superar la cédula ({suma_cedula}) del reporte {id_reporte}"
        );
    }

    let lote: Uuid = tx
        .query_one(
            "SELECT lote_id FROM public.reportes_cambios
             WHERE accion = $1 AND id_registro_obra = $2 LIMIT 1",
            &[&ACCION_DESACTIVAR_OBRA, &id_obra],
        )
        .expect("debe existir un lote DESACTIVAR_OBRA ligado a la obra")
        .get(0);

    // Desactivar de nuevo una obra ya inactiva debe rechazarse (error de Rust, la tx sigue viva).
    let err = desactivar_obra_tx(&mut tx, id_obra).expect_err("desactivar dos veces debe fallar");
    assert!(
        err.contains("ya fue eliminada"),
        "el error debe indicar que la obra ya fue eliminada: {err}"
    );

    // Deshacer el lote: la obra vuelve a estar activa y los tramos recuperan la obra.
    let restaurados = deshacer_lote_tx(&mut tx, lote).expect("deshacer lote");
    assert_eq!(restaurados, 2, "deben restaurarse los dos reportes");

    let activa: bool = tx
        .query_one("SELECT activa FROM public.registros_obras WHERE id = $1", &[&id_obra])
        .expect("consultar obra tras deshacer")
        .get(0);
    assert!(activa, "al deshacer, la obra debe volver a activa");

    let con_obra = contar(
        &mut tx,
        "SELECT COUNT(*) FROM public.reportes_detalles_liquidaciones WHERE id_registro_obra = $1",
        id_obra,
    );
    assert_eq!(con_obra, 2, "al deshacer, los tramos deben volver a tener la obra");

    tx.rollback().unwrap();
}

#[test]
#[ignore]
fn i6_acortar_vigencia_mueve_fuera_de_periodo_y_ampliar_regresa() {
    let mut client = conectar();
    let mut tx = client.transaction().unwrap();

    let (_c, rp, rev, emp) = crear_contexto(&mut tx, "I6");
    let liq_ene = crear_liquidacion(&mut tx, rp, emp, 2025, 1, "ENERO", d(2025, 1, 15), 30, 1000.0);
    let liq_mar = crear_liquidacion(&mut tx, rp, emp, 2025, 3, "MARZO", d(2025, 3, 15), 30, 1000.0);

    // Revisión sin periodo: el universo lo marcan las obras de usuario (enero a marzo).
    let obra = create_obra_tx(
        &mut tx,
        &item_obra("", rev, rp, "C7000006", Some(d(2025, 1, 1)), Some(d(2025, 3, 31))),
    )
    .expect("crear obra de prueba");
    let id_obra: Uuid = obra.id.parse().expect("id de obra");

    sync_reportes_liquidaciones_for_revision_tx(&mut tx, rev).expect("sync inicial");
    assert_eq!(
        estado_reporte_de_liquidacion(&mut tx, liq_ene).as_deref(),
        Some("ACTIVO"),
        "enero debe quedar ACTIVO con la obra vigente"
    );
    assert_eq!(
        estado_reporte_de_liquidacion(&mut tx, liq_mar).as_deref(),
        Some("ACTIVO"),
        "marzo debe quedar ACTIVO con la obra vigente"
    );

    // Acortar la vigencia a enero: marzo debe salir del universo sin borrarse.
    let acortada = item_obra(
        &id_obra.to_string(),
        rev,
        rp,
        "C7000006",
        Some(d(2025, 1, 1)),
        Some(d(2025, 1, 31)),
    );
    update_obra_tx(&mut tx, &acortada).expect("actualizar obra a enero");
    let cambio: CambioUniverso = resincronizar_universo_tx(&mut tx, rev).expect("resincronizar tras acortar");
    assert_eq!(cambio.a_fuera, 1, "debe salir del universo un reporte: {cambio:?}");
    assert_eq!(cambio.a_activo, 0, "ningún reporte debe reactivarse al acortar: {cambio:?}");
    assert_eq!(
        estado_reporte_de_liquidacion(&mut tx, liq_mar).as_deref(),
        Some("FUERA_PERIODO"),
        "marzo debe quedar FUERA_PERIODO"
    );
    assert_eq!(
        estado_reporte_de_liquidacion(&mut tx, liq_ene).as_deref(),
        Some("ACTIVO"),
        "enero debe seguir ACTIVO"
    );
    let filas_marzo = contar(
        &mut tx,
        "SELECT COUNT(*) FROM public.reportes_liquidaciones WHERE id_liquidacion = $1",
        liq_mar,
    );
    assert_eq!(filas_marzo, 1, "el reporte fuera de periodo no debe borrarse");

    // Ampliar de nuevo a marzo: marzo debe regresar a ACTIVO.
    let ampliada = item_obra(
        &id_obra.to_string(),
        rev,
        rp,
        "C7000006",
        Some(d(2025, 1, 1)),
        Some(d(2025, 3, 31)),
    );
    update_obra_tx(&mut tx, &ampliada).expect("actualizar obra a marzo");
    let cambio = resincronizar_universo_tx(&mut tx, rev).expect("resincronizar tras ampliar");
    assert_eq!(cambio.a_activo, 1, "marzo debe regresar a ACTIVO: {cambio:?}");
    assert_eq!(cambio.a_fuera, 0, "ningún reporte debe salir al ampliar: {cambio:?}");
    assert_eq!(
        estado_reporte_de_liquidacion(&mut tx, liq_mar).as_deref(),
        Some("ACTIVO"),
        "marzo debe volver a ACTIVO"
    );

    tx.rollback().unwrap();
}

#[test]
#[ignore]
fn i6b_universo_interseccion_periodo_y_obras() {
    let mut client = conectar();
    let mut tx = client.transaction().unwrap();

    let (_c, rp, rev, emp) = crear_contexto(&mut tx, "I6B");
    let liq_ene = crear_liquidacion(&mut tx, rp, emp, 2025, 1, "ENERO", d(2025, 1, 15), 30, 1000.0);
    let liq_mar = crear_liquidacion(&mut tx, rp, emp, 2025, 3, "MARZO", d(2025, 3, 15), 30, 1000.0);

    // Revisión con periodo de todo 2025 y sin obras de usuario: enero y marzo están en el universo.
    tx.execute(
        "UPDATE public.revisiones SET periodo_inicio = $2, periodo_fin = $3 WHERE id = $1",
        &[&rev, &d(2025, 1, 1), &d(2025, 12, 31)],
    )
    .expect("asignar periodo a la revisión de prueba");

    sync_reportes_liquidaciones_for_revision_tx(&mut tx, rev).expect("sync inicial");
    assert_eq!(
        estado_reporte_de_liquidacion(&mut tx, liq_mar).as_deref(),
        Some("ACTIVO"),
        "solo con periodo, marzo debe quedar ACTIVO"
    );

    // Obra de usuario solo en enero: el universo es periodo ∩ obras, así que marzo sale aunque esté en el periodo.
    let obra = create_obra_tx(
        &mut tx,
        &item_obra("", rev, rp, "C7000007", Some(d(2025, 1, 1)), Some(d(2025, 1, 31))),
    )
    .expect("crear obra de prueba");
    let id_obra: Uuid = obra.id.parse().expect("id de obra");

    resincronizar_universo_tx(&mut tx, rev).expect("resincronizar universo");
    assert_eq!(
        estado_reporte_de_liquidacion(&mut tx, liq_ene).as_deref(),
        Some("ACTIVO"),
        "enero debe quedar ACTIVO (dentro del periodo y de la obra)"
    );
    assert_eq!(
        estado_reporte_de_liquidacion(&mut tx, liq_mar).as_deref(),
        Some("FUERA_PERIODO"),
        "marzo está en el periodo pero fuera de la obra: debe quedar FUERA_PERIODO"
    );

    // Sin obras de usuario activas: el universo es solo el periodo, así que marzo regresa a ACTIVO.
    // `desactivar_obra_tx` ya resincroniza el universo internamente.
    desactivar_obra_tx(&mut tx, id_obra).expect("desactivar obra de prueba");
    assert_eq!(
        estado_reporte_de_liquidacion(&mut tx, liq_mar).as_deref(),
        Some("ACTIVO"),
        "sin obras, marzo debe volver a ACTIVO por el periodo"
    );

    tx.rollback().unwrap();
}
