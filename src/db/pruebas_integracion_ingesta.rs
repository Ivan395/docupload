//! Prueba de integración de la ingesta de cédulas contra la BD configurada.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. La prueba crea su propio cliente
//! temporal (nombre único, RFC NULL) y SIEMPRE lo borra al final, incluso si falla.
//! Solo se ejecuta explícitamente con `cargo test -- --ignored`.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use postgres::Client;
use uuid::Uuid;

use crate::cedulas::business::Business;
use crate::cedulas::employees::{DataEmployee, Employee, RegistroObra};
use crate::cedulas::view_logic::guardar_cedulas_lote;
use crate::db::client::{create_client, create_pool, DbPool};
use crate::db::ingesta::{ArchivoLote, ArchivoParaRegistrar};

const ARCHIVO_A: &str = "ZZ__RP__MARZO__2026__aaaaaaaaaaaa.pdf";
const ARCHIVO_B: &str = "ZZ__RP__MARZO__2026__bbbbbbbbbbbb.pdf";
const NSS_1: &str = "12345678901";
const NSS_2: &str = "12345678902";
const NOMBRE_1: &str = "PRUEBA INGESTA UNO";
const NOMBRE_2: &str = "PRUEBA INGESTA DOS";
/// Folio SIROC válido ('C' + 7 dígitos).
const OBRA: &str = "C1234567";

/// Identificadores de los datos temporales creados por la prueba.
struct DatosTemporales {
    id_cliente: Uuid,
    id_rp: Uuid,
    id_revision: Uuid,
    registro: String,
}

/// Conteos filtrados por el cliente/RP/revisión temporales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Foto {
    empleados: i64,
    liquidaciones: i64,
    detalles: i64,
    vinculaciones: i64,
    reportes_liquidaciones: i64,
    archivos: i64,
    archivos_activos: i64,
}

/// Crea cliente, registro patronal y revisión en una sola transacción.
fn crear_datos_temporales(client: &mut Client) -> Result<DatosTemporales, String> {
    let uuid = Uuid::now_v7();
    // Registro patronal char(11): 'Z' + 10 dígitos derivados del UUID
    let registro = format!("Z{:010}", uuid.as_u128() % 10_000_000_000);
    let nombre_cliente = format!("ZZ PRUEBA INGESTA {}", uuid.simple());
    let nombre_revision = format!("ZZ REVISION PRUEBA {}", uuid.simple());

    let mut tx = client
        .transaction()
        .map_err(|e| format!("Error iniciando transacción de preparación: {e}"))?;

    let id_cliente: Uuid = tx
        .query_one(
            "INSERT INTO public.clientes (nombre) VALUES ($1) RETURNING id",
            &[&nombre_cliente],
        )
        .map_err(|e| format!("Error creando cliente temporal: {e}"))?
        .get(0);

    let id_rp: Uuid = tx
        .query_one(
            "INSERT INTO public.registros_patronales (id_cliente, registro) VALUES ($1, $2) RETURNING id",
            &[&id_cliente, &registro],
        )
        .map_err(|e| format!("Error creando registro patronal temporal: {e}"))?
        .get(0);

    let id_revision: Uuid = tx
        .query_one(
            "INSERT INTO public.revisiones (id_registro_patronal, nombre) VALUES ($1, $2) RETURNING id",
            &[&id_rp, &nombre_revision],
        )
        .map_err(|e| format!("Error creando revisión temporal: {e}"))?
        .get(0);

    tx.commit()
        .map_err(|e| format!("Error confirmando datos temporales: {e}"))?;

    Ok(DatosTemporales {
        id_cliente,
        id_rp,
        id_revision,
        registro,
    })
}

/// Borra el cliente temporal; el borrado se propaga en cascada a RP, revisiones, empleados,
/// liquidaciones, reportes y archivos_cedulas. Devuelve cuántos clientes se borraron.
fn borrar_datos_temporales(client: &mut Client, id_cliente: Uuid) -> Result<u64, String> {
    client
        .execute("DELETE FROM public.clientes WHERE id = $1", &[&id_cliente])
        .map_err(|e| format!("Error borrando cliente temporal {id_cliente}: {e}"))
}

fn contar(client: &mut Client, sql: &str, id: &Uuid) -> Result<i64, String> {
    client
        .query_one(sql, &[id])
        .map(|fila| fila.get::<_, i64>(0))
        .map_err(|e| format!("Error en conteo ({sql}): {e}"))
}

/// Toma la "foto" de conteos del escenario temporal.
fn foto(client: &mut Client, d: &DatosTemporales) -> Result<Foto, String> {
    Ok(Foto {
        empleados: contar(
            client,
            "SELECT COUNT(*) FROM public.empleados WHERE id_cliente = $1",
            &d.id_cliente,
        )?,
        liquidaciones: contar(
            client,
            "SELECT COUNT(*) FROM public.liquidaciones WHERE id_registro_patronal = $1",
            &d.id_rp,
        )?,
        detalles: contar(
            client,
            "SELECT COUNT(*) FROM public.detalles_liquidaciones dl
               JOIN public.liquidaciones l ON l.id = dl.id_liquidacion
              WHERE l.id_registro_patronal = $1",
            &d.id_rp,
        )?,
        vinculaciones: contar(
            client,
            "SELECT COUNT(*) FROM public.vinculaciones_obras_liquidaciones v
               JOIN public.liquidaciones l ON l.id = v.id_liquidacion
              WHERE l.id_registro_patronal = $1",
            &d.id_rp,
        )?,
        reportes_liquidaciones: contar(
            client,
            "SELECT COUNT(*) FROM public.reportes_liquidaciones WHERE id_revision = $1",
            &d.id_revision,
        )?,
        archivos: contar(
            client,
            "SELECT COUNT(*) FROM public.archivos_cedulas WHERE id_registro_patronal = $1",
            &d.id_rp,
        )?,
        archivos_activos: contar(
            client,
            "SELECT COUNT(*) FROM public.archivos_cedulas
              WHERE id_registro_patronal = $1 AND estado = 'ACTIVO'",
            &d.id_rp,
        )?,
    })
}

/// Devuelve (id de liquidación, id_archivo) del trabajador con el NSS indicado.
fn liquidacion_de(client: &mut Client, d: &DatosTemporales, nss: &str) -> Result<(Uuid, Option<Uuid>), String> {
    let fila = client
        .query_one(
            "SELECT l.id, l.id_archivo
               FROM public.liquidaciones l
               JOIN public.empleados e ON e.id = l.id_empleado
              WHERE l.id_registro_patronal = $1 AND BTRIM(e.nss) = $2",
            &[&d.id_rp, &nss],
        )
        .map_err(|e| format!("Error buscando liquidación del NSS {nss}: {e}"))?;
    Ok((fila.get(0), fila.get(1)))
}

/// Subtotales de los renglones de detalle de una liquidación.
fn subtotales_de(client: &mut Client, id_liquidacion: Uuid) -> Result<Vec<Option<f64>>, String> {
    client
        .query(
            "SELECT subtotal FROM public.detalles_liquidaciones
              WHERE id_liquidacion = $1 ORDER BY subtotal",
            &[&id_liquidacion],
        )
        .map(|filas| filas.iter().map(|f| f.get::<_, Option<f64>>(0)).collect())
        .map_err(|e| format!("Error leyendo detalles de liquidación: {e}"))
}

/// Números de obra vinculados a una liquidación.
fn obras_vinculadas(client: &mut Client, id_liquidacion: Uuid) -> Result<Vec<String>, String> {
    client
        .query(
            "SELECT BTRIM(ro.numero) FROM public.vinculaciones_obras_liquidaciones v
               JOIN public.registros_obras ro ON ro.id = v.id_registro_obra
              WHERE v.id_liquidacion = $1 ORDER BY 1",
            &[&id_liquidacion],
        )
        .map(|filas| filas.iter().map(|f| f.get::<_, String>(0)).collect())
        .map_err(|e| format!("Error leyendo vinculaciones de obra: {e}"))
}

fn id_archivo_por_nombre(client: &mut Client, d: &DatosTemporales, nombre: &str) -> Result<Uuid, String> {
    client
        .query_one(
            "SELECT id FROM public.archivos_cedulas
              WHERE id_registro_patronal = $1 AND nombre_original = $2",
            &[&d.id_rp, &nombre],
        )
        .map(|fila| fila.get::<_, Uuid>(0))
        .map_err(|e| format!("Error buscando archivo '{nombre}': {e}"))
}

/// Compara dos valores y devuelve un error descriptivo si difieren.
fn exigir_igual<T: PartialEq + std::fmt::Debug>(obtenido: T, esperado: T, contexto: &str) -> Result<(), String> {
    if obtenido == esperado {
        Ok(())
    } else {
        Err(format!("{contexto}: obtenido {obtenido:?}, esperado {esperado:?}"))
    }
}

/// Renglón de cuotas con importes fijos; `subtotal` es el dato que distingue cargas.
fn detalle(subtotal: f64) -> DataEmployee {
    DataEmployee {
        mes_periodo: Some("MARZO".to_string()),
        mes_periodo_int: 3,
        anio_periodo: Some("2026".to_string()),
        clave: Some("CUOTA".to_string()),
        dias: Some(31.0),
        sdi: Some(500.0),
        cf: Some(100.0),
        lic: Some(0.0),
        inc: Some(0.0),
        aus: Some(0.0),
        exc_pat: Some(0.0),
        exc_obr: Some(0.0),
        pd_pat: Some(0.0),
        pd_obr: Some(0.0),
        gmp_pat: Some(0.0),
        gmp_obr: Some(0.0),
        rt: Some(0.0),
        iv_pat: Some(0.0),
        iv_obr: Some(0.0),
        gps: Some(0.0),
        patronal: Some(0.0),
        obrera: Some(0.0),
        subtotal: Some(subtotal),
        ..Default::default()
    }
}

fn empleado(
    nss: &str,
    nombre: &str,
    subtotal: f64,
    registro_obras: Vec<RegistroObra>,
    file_name: &str,
) -> Employee {
    Employee {
        no_seguro_social: Some(nss.to_string()),
        nombre: Some(nombre.to_string()),
        mes_periodo: Some("MARZO".to_string()),
        mes_periodo_int: 3,
        anio_periodo: Some("2026".to_string()),
        file_name: Some(file_name.to_string()),
        data_employee: vec![detalle(subtotal)],
        registro_obras,
        ..Default::default()
    }
}

fn negocio(registro: &str, employees: Vec<Employee>) -> Business {
    Business {
        registro_patronal: Some(registro.to_string()),
        employees,
        ..Default::default()
    }
}

/// Archivo de lote de prueba con SHA-256 uniforme (`letra` repetida 64 veces).
fn archivo(nombre: &str, letra: char, tipo: &str, trabajadores: i32) -> ArchivoParaRegistrar {
    ArchivoParaRegistrar {
        nombre_archivo: nombre.to_string(),
        lote: ArchivoLote {
            nombre_original: nombre.to_string(),
            sha256: letra.to_string().repeat(64),
            tipo: tipo.to_string(),
            anio: 2026,
            mes: 3,
            trabajadores,
            paginas: Some(1),
        },
        clave_final: format!("prueba/final/{nombre}"),
        clave_staging: format!("prueba/staging/{nombre}"),
    }
}

/// Guarda un lote en la revisión temporal sin promoción real en almacén.
fn guardar_lote(
    pool: &DbPool,
    d: &DatosTemporales,
    negocios: &[Business],
    archivos: &[ArchivoParaRegistrar],
) -> Result<(), String> {
    let promover_ok: &(dyn Fn() -> Result<(), String> + Send + Sync) = &|| Ok(());
    let id_revision = d.id_revision.to_string();
    let stats = guardar_cedulas_lote(pool, &id_revision, negocios, archivos, &[], Some(promover_ok), None)?;
    if !stats.errores.is_empty() {
        return Err(format!("guardar_cedulas_lote reportó errores: {:?}", stats.errores));
    }
    Ok(())
}

/// Escenario completo: idempotencia de la carga A y regla "gana la más reciente" con la carga B.
fn escenario(pool: &DbPool, client: &mut Client, d: &DatosTemporales) -> Result<(), String> {
    // Lote A: dos trabajadores; el primero con una obra vinculada
    let negocios_a = vec![negocio(
        &d.registro,
        vec![
            empleado(
                NSS_1,
                NOMBRE_1,
                1000.0,
                vec![RegistroObra {
                    registro_obra: OBRA.to_string(),
                    dias_registro_obra: 31,
                }],
                ARCHIVO_A,
            ),
            empleado(NSS_2, NOMBRE_2, 1000.0, Vec::new(), ARCHIVO_A),
        ],
    )];
    let archivos_a = vec![archivo(ARCHIVO_A, 'a', "SUA", 2)];

    guardar_lote(pool, d, &negocios_a, &archivos_a)?;
    let foto_1 = foto(client, d)?;
    exigir_igual(foto_1.empleados, 2, "empleados tras la primera carga")?;
    exigir_igual(foto_1.liquidaciones, 2, "liquidaciones tras la primera carga")?;
    exigir_igual(foto_1.archivos_activos, 1, "archivos ACTIVO tras la primera carga")?;
    let (liq_1, _) = liquidacion_de(client, d, NSS_1)?;
    exigir_igual(subtotales_de(client, liq_1)?, vec![Some(1000.0)], "subtotal tras la primera carga")?;
    exigir_igual(obras_vinculadas(client, liq_1)?, vec![OBRA.to_string()], "obra vinculada tras la primera carga")?;

    // Idempotencia: el mismo lote otra vez no cambia ningún conteo
    guardar_lote(pool, d, &negocios_a, &archivos_a)?;
    let foto_2 = foto(client, d)?;
    exigir_igual(foto_2, foto_1, "conteos tras repetir el mismo lote")?;
    exigir_igual(foto_2.empleados, 2, "empleados tras repetir el lote")?;
    exigir_igual(foto_2.liquidaciones, 2, "liquidaciones tras repetir el lote")?;
    exigir_igual(foto_2.archivos_activos, 1, "archivos ACTIVO tras repetir el lote")?;

    // Lote B: mismo mes, archivo distinto; trabajador 1 sin obras y con subtotal 2000
    let negocios_b = vec![negocio(
        &d.registro,
        vec![empleado(NSS_1, NOMBRE_1, 2000.0, Vec::new(), ARCHIVO_B)],
    )];
    let archivos_b = vec![archivo(ARCHIVO_B, 'b', "EMA", 1)];

    guardar_lote(pool, d, &negocios_b, &archivos_b)?;
    let foto_3 = foto(client, d)?;
    exigir_igual(foto_3.empleados, 2, "no debe duplicar empleados")?;
    exigir_igual(foto_3.liquidaciones, 2, "no debe duplicar liquidaciones")?;
    exigir_igual(foto_3.detalles, foto_1.detalles, "los detalles se reemplazan, no se suman")?;
    exigir_igual(foto_3.reportes_liquidaciones, foto_1.reportes_liquidaciones, "reportes sin duplicar")?;
    exigir_igual(foto_3.vinculaciones, foto_1.vinculaciones, "las vinculaciones no cambian sin obras nuevas")?;
    exigir_igual(foto_3.archivos, 2, "archivos registrados tras la carga B")?;
    exigir_igual(foto_3.archivos_activos, 2, "ambos archivos ACTIVO tras la carga B")?;

    let (liq_3, id_archivo_3) = liquidacion_de(client, d, NSS_1)?;
    exigir_igual(liq_3, liq_1, "la liquidación del trabajador debe ser la misma")?;
    exigir_igual(subtotales_de(client, liq_3)?, vec![Some(2000.0)], "gana la carga más reciente")?;
    exigir_igual(obras_vinculadas(client, liq_3)?, vec![OBRA.to_string()], "se conserva la obra vinculada")?;
    let id_archivo_b = id_archivo_por_nombre(client, d, ARCHIVO_B)?;
    exigir_igual(id_archivo_3, Some(id_archivo_b), "id_archivo apunta al archivo de la carga B")?;

    Ok(())
}

#[test]
#[ignore]
fn ingesta_idempotente_y_mas_reciente_bd() {
    let pool = match create_pool(4) {
        Ok(p) => p,
        Err(e) => {
            println!("PostgreSQL no disponible (prueba omitida): {e}");
            return;
        }
    };
    let mut client = match create_client() {
        Ok(c) => c,
        Err(e) => {
            println!("PostgreSQL no disponible (prueba omitida): {e}");
            return;
        }
    };

    // Esquema al día (idéntico a lo que hace la app al arrancar; ya no es destructivo)
    if let Err(e) = crate::db::schema::ensure_schema(&mut client) {
        panic!("ensure_schema falló: {}", crate::db::schema::format_pg_error(&e));
    }

    let datos = match crear_datos_temporales(&mut client) {
        Ok(d) => d,
        Err(e) => panic!("No se pudieron crear los datos temporales de la prueba: {e}"),
    };

    // Se captura un posible panic para garantizar la limpieza antes de propagarlo
    let resultado = catch_unwind(AssertUnwindSafe(|| escenario(&pool, &mut client, &datos)));

    let limpieza = borrar_datos_temporales(&mut client, datos.id_cliente);
    if let Err(e) = &limpieza {
        eprintln!("ATENCIÓN: limpieza de datos temporales fallida: {e}");
    }

    let resultado = match resultado {
        Ok(r) => r,
        Err(payload) => resume_unwind(payload),
    };

    match (resultado, limpieza) {
        (Ok(()), Ok(1)) => {}
        (Ok(()), Ok(n)) => panic!("Limpieza: se esperaba borrar 1 cliente temporal, se borraron {n}"),
        (Ok(()), Err(e)) => panic!("La prueba pasó, pero falló la limpieza: {e}"),
        (Err(e), limpieza) => panic!("Prueba fallida: {e}. Resultado de limpieza: {limpieza:?}"),
    }
}
