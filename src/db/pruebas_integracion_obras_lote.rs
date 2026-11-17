//! Pruebas de integración del alta masiva de obras contra la BD configurada.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. La prueba corre dentro de UNA sola
//! transacción que se revierte al final (`tx.rollback()`; si la prueba hace panic, Drop revierte).
//! Nunca se hace COMMIT. Solo se ejecutan explícitamente con `cargo test -- --ignored`.

use chrono::NaiveDate;
use postgres::{Client, Transaction};
use uuid::Uuid;

use crate::carga_masiva::tabla_desde_texto_pegado;
use crate::db::client::create_client;
use crate::db::obras_ops::{aplicar_lote_obras_tx, fetch_obras_existentes_revision};
use crate::db::revisiones::{create_obra_tx, RegistroObraItem};
use crate::db::schema::{ensure_schema, format_pg_error};
use crate::obras_lote_logic::{clasificar, interpretar_tabla_obras, AccionFila, EstadoFila};

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

/// Cliente, registro patronal y revisión temporales dentro de la transacción.
fn crear_contexto(tx: &mut Transaction<'_>) -> (Uuid, Uuid) {
    let uuid = Uuid::now_v7();
    let nombre_cliente = format!("ZZ PRUEBA OBRAS LOTE {}", uuid.simple());
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
            &[&id_rp, &format!("ZZ REVISION OBRAS LOTE {}", uuid.simple())],
        )
        .expect("crear revisión de prueba")
        .get(0);

    (id_rp, id_revision)
}

fn item_usuario(id_revision: Uuid, id_rp: Uuid, numero: &str, fin: NaiveDate) -> RegistroObraItem {
    RegistroObraItem {
        id: String::new(),
        id_revision: id_revision.to_string(),
        id_registro_patronal: id_rp.to_string(),
        numero: numero.to_string(),
        clase: "PUBLICA".to_string(),
        tipo_obra: "OBRA DE PRUEBA".to_string(),
        fecha_inicio: Some(d(2025, 1, 1)),
        fecha_fin: Some(fin),
        ubicacion: "CDMX".to_string(),
        superficie_construido: 0.0,
        monto: Some(1000.0),
        delegacion: "CDMX".to_string(),
        subdelegacion: "PRUEBA".to_string(),
        created_at: String::new(),
        creado_usuario: true,
    }
}

/// I-8: lote mixto contra obras existentes de usuario (A) y de cédula (B).
#[test]
#[ignore]
fn i8_lote_mixto_crear_actualizar_adoptar_omitir() {
    let mut client = conectar();
    let mut tx = client.transaction().expect("iniciar transacción de prueba");
    let (id_rp, id_rev) = crear_contexto(&mut tx);

    // A: obra creada por el usuario.
    create_obra_tx(&mut tx, &item_usuario(id_rev, id_rp, "C2000001", d(2025, 6, 30)))
        .expect("crear obra de usuario A");

    // B: obra que vino de cédula (creado_usuario = false).
    tx.execute(
        "INSERT INTO public.registros_obras
            (id_revision, id_registro_patronal, numero, fecha_inicio, fecha_fin, creado_usuario)
         VALUES ($1, $2, 'C2000002', $3, $4, false)",
        &[&id_rev, &id_rp, &d(2025, 1, 1), &d(2025, 6, 30)],
    )
    .expect("insertar obra de cédula B");

    let tabla = tabla_desde_texto_pegado(
        "NUMERO\tINICIO\tFIN\tMONTO\tCLASE\tTIPO\tUBICACION\n\
         C2000003\t01/01/2025\t31/12/2025\t5,000.00\tPUBLICA\tOBRA\tCDMX\n\
         C2000004\t01/01/2025\t31/12/2025\t\tPUBLICA\tOBRA\tCDMX\n\
         C12\t01/01/2025\t31/12/2025\t100\tPUBLICA\tOBRA\tCDMX\n\
         C2000001\t01/07/2025\t31/12/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
         C2000002\t01/01/2025\t31/12/2025\t2000\tPUBLICA\tOBRA\tCDMX\n",
    );
    let filas = interpretar_tabla_obras(&tabla).expect("interpretar lote");
    let existentes = fetch_obras_existentes_revision(&mut tx, id_rev).expect("leer obras existentes");
    let mut previa = clasificar(filas, &existentes);

    let estado_de = |numero: &str| -> EstadoFila {
        previa
            .iter()
            .find(|p| p.fila.numero == numero)
            .unwrap_or_else(|| panic!("falta la fila {numero}"))
            .estado
    };
    assert_eq!(estado_de("C2000003"), EstadoFila::Valida);
    assert_eq!(estado_de("C2000004"), EstadoFila::SinMonto);
    assert_eq!(estado_de("C12"), EstadoFila::Error);
    assert_eq!(estado_de("C2000001"), EstadoFila::ExisteUsuario);
    assert_eq!(estado_de("C2000002"), EstadoFila::ExisteCedula);

    for p in previa.iter_mut() {
        match p.fila.numero.as_str() {
            "C2000001" => p.accion = AccionFila::Actualizar,
            "C2000002" => p.accion = AccionFila::Adoptar,
            _ => {}
        }
    }

    let resumen = aplicar_lote_obras_tx(&mut tx, id_rev, id_rp, &previa).expect("aplicar lote de obras");
    assert_eq!(resumen.creadas, 2, "creadas");
    assert_eq!(resumen.actualizadas, 1, "actualizadas");
    assert_eq!(resumen.adoptadas, 1, "adoptadas");
    assert_eq!(resumen.omitidas, 1, "omitidas");

    // A: fecha fin actualizada.
    let fin_a: NaiveDate = tx
        .query_one(
            "SELECT fecha_fin FROM public.registros_obras WHERE id_revision = $1 AND BTRIM(numero) = 'C2000001'",
            &[&id_rev],
        )
        .expect("leer obra A")
        .get(0);
    assert_eq!(fin_a, d(2025, 12, 31), "la obra A debe tener la nueva fecha fin");

    // B: ahora es obra de usuario.
    let creado_b: bool = tx
        .query_one(
            "SELECT creado_usuario FROM public.registros_obras WHERE id_revision = $1 AND BTRIM(numero) = 'C2000002'",
            &[&id_rev],
        )
        .expect("leer obra B")
        .get(0);
    assert!(creado_b, "la obra B debe quedar con creado_usuario = true");

    // C y D se crearon; E (inválida) no existe.
    for numero in ["C2000003", "C2000004"] {
        let n: i64 = tx
            .query_one(
                "SELECT COUNT(*) FROM public.registros_obras WHERE id_revision = $1 AND BTRIM(numero) = $2",
                &[&id_rev, &numero],
            )
            .expect("contar obra creada")
            .get(0);
        assert_eq!(n, 1, "la obra {numero} debe existir una vez");
    }
    let n_e: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM public.registros_obras WHERE id_revision = $1 AND BTRIM(numero) = 'C12'",
            &[&id_rev],
        )
        .expect("contar obra inválida")
        .get(0);
    assert_eq!(n_e, 0, "la fila inválida no debe existir en registros_obras");

    tx.rollback().expect("revertir datos de prueba");
}
