//! Pruebas de integración de la Fase 6 (matriz de casos) contra la BD configurada:
//! fusión de cargas con ediciones del analista, mismo NSS en dos meses, periodo fuera y regreso,
//! fallo de RustFS tras el COMMIT con reintento, y versionado de entregables.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. Cada prueba crea su propio cliente temporal
//! ("ZZ PRUEBA FASE6 ...") y SIEMPRE lo borra al final, incluso si falla. La cascada elimina RP,
//! revisiones, obras, empleados, liquidaciones, reportes y archivos.
//! Solo se ejecuta explícitamente con `cargo test -- --ignored`.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use postgres::Client;
use uuid::Uuid;

use crate::cedulas::almacen::{sha256_hex, AlmacenArchivos, AlmacenMemoria};
use crate::cedulas::business::Business;
use crate::cedulas::employees::{DataEmployee, Employee};
use crate::cedulas::view_logic::guardar_cedulas_lote;
use crate::db::client::{create_client, create_pool, DbPool};
use crate::db::entregables::{listar_exportaciones, registrar_exportacion, siguiente_version};
use crate::db::ingesta::{reintentar_promociones_pendientes, ArchivoLote, ArchivoParaRegistrar};
use crate::db::reporte_liquidaciones::{
    batch_assign_multiple_obras_to_employees, sync_reportes_liquidaciones_for_revision,
};
use crate::db::schema::{ensure_schema, format_pg_error};

const NSS: &str = "12345678904";
const NOMBRE: &str = "PRUEBA FASE6 TRABAJADOR";
/// Tolerancia para comparar importes (pesos).
const TOLERANCIA: f64 = 0.05;

/// Identificadores de los datos temporales creados por la prueba.
struct DatosTemporales {
    id_cliente: Uuid,
    id_rp: Uuid,
    id_revision: Uuid,
    registro: String,
}

/// Reporte de liquidaciones de un trabajador en un mes de la revisión temporal.
struct ReporteInfo {
    id: Uuid,
    estado: String,
    editado: bool,
}

/// Crea cliente, registro patronal y revisión en una sola transacción.
fn crear_datos_temporales(client: &mut Client) -> Result<DatosTemporales, String> {
    let uuid = Uuid::now_v7();
    // Registro patronal char(11): 'Z' + 10 dígitos derivados del UUID
    let registro = format!("Z{:010}", uuid.as_u128() % 10_000_000_000);
    let nombre_cliente = format!("ZZ PRUEBA FASE6 {}", uuid.simple());
    let nombre_revision = format!("ZZ REVISION PRUEBA FASE6 {}", uuid.simple());

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

/// Borra el cliente temporal; el borrado se propaga en cascada. Devuelve cuántos clientes se borraron.
fn borrar_datos_temporales(client: &mut Client, id_cliente: Uuid) -> Result<u64, String> {
    client
        .execute("DELETE FROM public.clientes WHERE id = $1", &[&id_cliente])
        .map_err(|e| format!("Error borrando cliente temporal {id_cliente}: {e}"))
}

/// Ejecuta un escenario con datos temporales y SIEMPRE limpia, incluso si el escenario falla o hace panic.
fn correr_prueba(etiqueta: &str, escenario: fn(&DbPool, &mut Client, &DatosTemporales) -> Result<(), String>) {
    println!("== {etiqueta} ==");
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

    // Esquema al día (idéntico a lo que hace la app al arrancar; no es destructivo)
    if let Err(e) = ensure_schema(&mut client) {
        panic!("ensure_schema falló: {}", format_pg_error(&e));
    }

    let datos = match crear_datos_temporales(&mut client) {
        Ok(d) => d,
        Err(e) => panic!("No se pudieron crear los datos temporales de la prueba: {e}"),
    };
    println!("Datos temporales creados: cliente {} (RP {})", datos.id_cliente, datos.registro);

    // Se captura un posible panic para garantizar la limpieza antes de propagarlo
    let resultado = catch_unwind(AssertUnwindSafe(|| escenario(&pool, &mut client, &datos)));

    let limpieza = borrar_datos_temporales(&mut client, datos.id_cliente);
    match &limpieza {
        Ok(1) => println!("Datos temporales borrados"),
        Ok(n) => eprintln!("ATENCIÓN: limpieza borró {n} clientes (se esperaba 1)"),
        Err(e) => eprintln!("ATENCIÓN: limpieza de datos temporales fallida: {e}"),
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

/// Cuenta filas de una consulta con un único parámetro UUID.
fn contar(client: &mut Client, sql: &str, id: &Uuid) -> Result<i64, String> {
    client
        .query_one(sql, &[id])
        .map(|fila| fila.get::<_, i64>(0))
        .map_err(|e| format!("Error en conteo ({sql}): {e}"))
}

/// Compara dos valores y devuelve un error descriptivo si difieren.
fn exigir_igual<T: PartialEq + std::fmt::Debug>(obtenido: T, esperado: T, contexto: &str) -> Result<(), String> {
    if obtenido == esperado {
        Ok(())
    } else {
        Err(format!("{contexto}: obtenido {obtenido:?}, esperado {esperado:?}"))
    }
}

/// Compara importes con la tolerancia de centavos definida.
fn exigir_cerca(obtenido: f64, esperado: f64, contexto: &str) -> Result<(), String> {
    if (obtenido - esperado).abs() <= TOLERANCIA {
        Ok(())
    } else {
        Err(format!("{contexto}: obtenido {obtenido:.2}, esperado {esperado:.2}"))
    }
}

/// Clave de SHA-256 única por llamada (evita choques con uq_archivos_cedulas_rp_sha).
fn sha_unico() -> String {
    sha256_hex(Uuid::now_v7().as_bytes())
}

/// Número de obra con formato SIROC válido ('C' + 7 dígitos), único por llamada.
fn numero_obra_unico() -> String {
    format!("C{:07}", Uuid::now_v7().as_u128() % 10_000_000)
}

/// Crea una obra del usuario en la revisión temporal, vigente desde marzo 2026 hasta `fecha_fin`.
fn crear_obra(client: &mut Client, d: &DatosTemporales, numero: &str, fecha_fin: &str) -> Result<Uuid, String> {
    let sql = format!(
        "INSERT INTO public.registros_obras \
         (id_revision, id_registro_patronal, numero, clase, tipo_obra, fecha_inicio, fecha_fin, monto, creado_usuario) \
         VALUES ($1, $2, $3, 'PRUEBA', 'OBRA DE PRUEBA', '2026-03-01'::date, '{fecha_fin}'::date, 1000.0, true) \
         RETURNING id"
    );
    client
        .query_one(sql.as_str(), &[&d.id_revision, &d.id_rp, &numero])
        .map(|fila| fila.get::<_, Uuid>(0))
        .map_err(|e| format!("Error creando obra temporal {numero}: {e}"))
}

/// Días naturales del mes de 2026 (marzo y abril).
fn dias_del_mes(mes_int: i16) -> f64 {
    if mes_int == 4 {
        30.0
    } else {
        31.0
    }
}

/// Renglón de cuotas con importes fijos; `subtotal` es el dato que distingue cargas.
fn detalle(mes: &str, mes_int: i16, subtotal: f64) -> DataEmployee {
    DataEmployee {
        mes_periodo: Some(mes.to_string()),
        mes_periodo_int: mes_int,
        anio_periodo: Some("2026".to_string()),
        clave: Some("CUOTA".to_string()),
        dias: Some(dias_del_mes(mes_int)),
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

/// Trabajador de la cédula de un mes, sin obras, con el subtotal indicado.
fn empleado(mes: &str, mes_int: i16, subtotal: f64, nombre_archivo: &str) -> Employee {
    Employee {
        no_seguro_social: Some(NSS.to_string()),
        nombre: Some(NOMBRE.to_string()),
        mes_periodo: Some(mes.to_string()),
        mes_periodo_int: mes_int,
        anio_periodo: Some("2026".to_string()),
        file_name: Some(nombre_archivo.to_string()),
        data_employee: vec![detalle(mes, mes_int, subtotal)],
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

/// Archivo de lote de prueba con SHA-256 único y claves de almacenamiento de prueba.
fn archivo(nombre: &str, mes: i16) -> ArchivoParaRegistrar {
    ArchivoParaRegistrar {
        nombre_archivo: nombre.to_string(),
        lote: ArchivoLote {
            nombre_original: nombre.to_string(),
            sha256: sha_unico(),
            tipo: "SUA".to_string(),
            anio: 2026,
            mes,
            trabajadores: 1,
            paginas: Some(1),
        },
        clave_final: format!("prueba/final/{nombre}"),
        clave_staging: format!("prueba/staging/{nombre}"),
    }
}

/// Guarda un lote sin reemplazo por periodo y devuelve los errores reportados por el guardado.
fn guardar_con_promocion(
    pool: &DbPool,
    d: &DatosTemporales,
    negocios: &[Business],
    archivos: &[ArchivoParaRegistrar],
    promover: Option<&(dyn Fn() -> Result<(), String> + Send + Sync)>,
) -> Result<Vec<String>, String> {
    let id_revision = d.id_revision.to_string();
    let stats = guardar_cedulas_lote(pool, &id_revision, negocios, archivos, &[], promover, None)?;
    Ok(stats.errores)
}

/// Guarda un lote con promoción exitosa y exige que no haya errores.
fn guardar_ok(
    pool: &DbPool,
    d: &DatosTemporales,
    negocios: &[Business],
    archivos: &[ArchivoParaRegistrar],
) -> Result<(), String> {
    let promover_ok: &(dyn Fn() -> Result<(), String> + Send + Sync) = &|| Ok(());
    let errores = guardar_con_promocion(pool, d, negocios, archivos, Some(promover_ok))?;
    if errores.is_empty() {
        Ok(())
    } else {
        Err(format!("guardar_cedulas_lote reportó errores: {errores:?}"))
    }
}

/// Devuelve (id de liquidación, id_archivo) del trabajador de la prueba.
fn liquidacion_de(client: &mut Client, d: &DatosTemporales) -> Result<(Uuid, Option<Uuid>), String> {
    let fila = client
        .query_one(
            "SELECT l.id, l.id_archivo
               FROM public.liquidaciones l
               JOIN public.empleados e ON e.id = l.id_empleado
              WHERE l.id_registro_patronal = $1 AND BTRIM(e.nss) = $2
              ORDER BY l.id
              LIMIT 1",
            &[&d.id_rp, &NSS],
        )
        .map_err(|e| format!("Error buscando liquidación del NSS {NSS}: {e}"))?;
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

/// Reporte del trabajador de la prueba para un mes de la revisión temporal.
fn reporte_de(client: &mut Client, d: &DatosTemporales, mes_int: i16) -> Result<ReporteInfo, String> {
    let fila = client
        .query_one(
            "SELECT rl.id, rl.estado::text, rl.editado_usuario
               FROM public.reportes_liquidaciones rl
               JOIN public.empleados e ON e.id = rl.id_empleado
              WHERE rl.id_revision = $1 AND BTRIM(e.nss) = $2 AND rl.mes_periodo_int = $3::int2",
            &[&d.id_revision, &NSS, &mes_int],
        )
        .map_err(|e| format!("Error buscando reporte del mes {mes_int}: {e}"))?;
    Ok(ReporteInfo {
        id: fila.get(0),
        estado: fila.get(1),
        editado: fila.get(2),
    })
}

/// Días de obra que tiene asignados el reporte para la obra indicada, si existe el vínculo.
fn dias_de_obra_en_reporte(client: &mut Client, reporte: Uuid, obra: Uuid) -> Result<Option<i16>, String> {
    client
        .query(
            "SELECT dias_registro_obra::int2 FROM public.reportes_vinculaciones_obras_liquidaciones
              WHERE id_reporte_liquidacion = $1 AND id_registro_obra = $2",
            &[&reporte, &obra],
        )
        .map(|filas| filas.first().map(|f| f.get::<_, i16>(0)))
        .map_err(|e| format!("Error leyendo vinculación de obra del reporte: {e}"))
}

/// Suma de subtotales de los tramos (renglones de detalle) de un reporte.
fn subtotal_reporte(client: &mut Client, reporte: Uuid) -> Result<f64, String> {
    client
        .query_one(
            "SELECT COALESCE(SUM(subtotal), 0)::float8 FROM public.reportes_detalles_liquidaciones
              WHERE id_reporte_liquidacion = $1",
            &[&reporte],
        )
        .map(|fila| fila.get::<_, f64>(0))
        .map_err(|e| format!("Error sumando tramos del reporte: {e}"))
}

/// Estado del archivo de cédula con el nombre original indicado.
fn estado_archivo(client: &mut Client, d: &DatosTemporales, nombre: &str) -> Result<String, String> {
    client
        .query_one(
            "SELECT estado::text FROM public.archivos_cedulas
              WHERE id_registro_patronal = $1 AND nombre_original = $2",
            &[&d.id_rp, &nombre],
        )
        .map(|fila| fila.get::<_, String>(0))
        .map_err(|e| format!("Error leyendo estado del archivo '{nombre}': {e}"))
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

/// Fija el periodo de la revisión temporal. Las fechas son constantes de prueba, no datos de usuario.
fn fijar_periodo(client: &mut Client, d: &DatosTemporales, inicio: &str, fin: &str) -> Result<(), String> {
    let sql = format!(
        "UPDATE public.revisiones SET periodo_inicio = '{inicio}'::date, periodo_fin = '{fin}'::date WHERE id = $1"
    );
    client
        .execute(sql.as_str(), &[&d.id_revision])
        .map(|_| ())
        .map_err(|e| format!("Error fijando periodo de la revisión: {e}"))
}

/// Lote de dos cédulas (MARZO y ABRIL) del MISMO NSS en dos negocios del mismo RP.
fn lote_dos_meses(d: &DatosTemporales, nombre_marzo: &str, nombre_abril: &str) -> (Vec<Business>, Vec<ArchivoParaRegistrar>) {
    let negocios = vec![
        negocio(&d.registro, vec![empleado("MARZO", 3, 3100.0, nombre_marzo)]),
        negocio(&d.registro, vec![empleado("ABRIL", 4, 3100.0, nombre_abril)]),
    ];
    let archivos = vec![archivo(nombre_marzo, 3), archivo(nombre_abril, 4)];
    (negocios, archivos)
}

/// Fusión: la asignación del analista sobrevive a una carga nueva del mismo mes.
fn escenario_fusion(pool: &DbPool, client: &mut Client, d: &DatosTemporales) -> Result<(), String> {
    const ARCH_A: &str = "ZZ_F6_FUSION_A.pdf";
    const ARCH_B: &str = "ZZ_F6_FUSION_B.pdf";

    // 1. Lote A: subtotal 3100, sin obras
    let negocios_a = vec![negocio(&d.registro, vec![empleado("MARZO", 3, 3100.0, ARCH_A)])];
    guardar_ok(pool, d, &negocios_a, &[archivo(ARCH_A, 3)])?;
    let (liq_1, _) = liquidacion_de(client, d)?;
    exigir_igual(subtotales_de(client, liq_1)?, vec![Some(3100.0)], "detalle tras lote A")?;
    println!("[1] Lote A cargado: 1 trabajador, subtotal 3100, sin obras");

    // 2. Obra vigente todo marzo asignada por el analista (editado_usuario = true)
    let obra = crear_obra(client, d, &numero_obra_unico(), "2026-03-31")?;
    sync_reportes_liquidaciones_for_revision(client, d.id_revision)?;
    let rep = reporte_de(client, d, 3)?;
    batch_assign_multiple_obras_to_employees(client, d.id_revision, &[rep.id], &[obra], None, None, None)?;
    let rep = reporte_de(client, d, 3)?;
    exigir_igual(rep.editado, true, "editado_usuario tras la asignación")?;
    exigir_igual(dias_de_obra_en_reporte(client, rep.id, obra)?, Some(31), "días de obra tras la asignación")?;
    println!("[2] Obra vigente asignada por el analista: 31 días, editado_usuario = true");

    // 3. Fusión: lote B del mismo mes, otro archivo, subtotal 4000, sin reemplazo por periodo
    let negocios_b = vec![negocio(&d.registro, vec![empleado("MARZO", 3, 4000.0, ARCH_B)])];
    guardar_ok(pool, d, &negocios_b, &[archivo(ARCH_B, 3)])?;
    sync_reportes_liquidaciones_for_revision(client, d.id_revision)?;

    let (liq_2, _) = liquidacion_de(client, d)?;
    exigir_igual(liq_2, liq_1, "la liquidación debe ser la misma tras la fusión")?;
    exigir_igual(subtotales_de(client, liq_2)?, vec![Some(4000.0)], "la liquidación debe tener el detalle nuevo")?;
    exigir_igual(
        contar(client, "SELECT COUNT(*) FROM public.reportes_liquidaciones WHERE id_revision = $1", &d.id_revision)?,
        1,
        "un solo reporte tras la fusión",
    )?;

    let rep_fusion = reporte_de(client, d, 3)?;
    exigir_igual(rep_fusion.id, rep.id, "el reporte del analista debe conservarse")?;
    exigir_igual(rep_fusion.editado, true, "editado_usuario debe seguir en true")?;
    exigir_igual(
        dias_de_obra_en_reporte(client, rep_fusion.id, obra)?,
        Some(31),
        "la obra asignada por el analista debe seguir en el reporte",
    )?;
    // La obra cubre todos los días del mes, así que el tramo prorrateado iguala el importe nuevo.
    exigir_cerca(subtotal_reporte(client, rep_fusion.id)?, 4000.0, "tramo prorrateado tras la fusión")?;

    // Al reemplazar los detalles de una liquidación cuyo reporte ya editó el analista, la carga marca
    // cedula_actualizada (save_single_cedula_employee_tx → SQL_MARCAR_CEDULA_ACTUALIZADA) para que lo revise.
    let marcado: bool = client
        .query_one("SELECT cedula_actualizada FROM public.reportes_liquidaciones WHERE id = $1", &[&rep_fusion.id])
        .map_err(|e| e.to_string())?
        .get(0);
    exigir_igual(marcado, true, "cedula_actualizada tras la fusión sobre un reporte editado")?;
    println!("[3] Fusión: obra del analista conservada (31 días), detalle 4000, tramo prorrateado 4000, cédula actualizada = true");
    Ok(())
}

/// El mismo NSS en MARZO y ABRIL produce un solo empleado y dos liquidaciones.
fn escenario_mismo_nss(pool: &DbPool, client: &mut Client, d: &DatosTemporales) -> Result<(), String> {
    let (negocios, archivos) = lote_dos_meses(d, "ZZ_F6_NSS_MARZO.pdf", "ZZ_F6_NSS_ABRIL.pdf");
    guardar_ok(pool, d, &negocios, &archivos)?;

    exigir_igual(
        contar(client, "SELECT COUNT(*) FROM public.empleados WHERE id_cliente = $1", &d.id_cliente)?,
        1,
        "empleados del cliente",
    )?;
    exigir_igual(
        contar(client, "SELECT COUNT(*) FROM public.liquidaciones WHERE id_registro_patronal = $1", &d.id_rp)?,
        2,
        "liquidaciones del registro patronal",
    )?;
    exigir_igual(
        contar(client, "SELECT COUNT(*) FROM public.reportes_liquidaciones WHERE id_revision = $1", &d.id_revision)?,
        2,
        "reportes de la revisión",
    )?;
    println!("[1] Un solo empleado para el NSS, dos liquidaciones (MARZO y ABRIL) y dos reportes");
    Ok(())
}

/// Periodo fuera y regreso: ABRIL pasa a FUERA_PERIODO sin perder la asignación y vuelve a ACTIVO.
fn escenario_periodo_fuera_y_regreso(pool: &DbPool, client: &mut Client, d: &DatosTemporales) -> Result<(), String> {
    let (negocios, archivos) = lote_dos_meses(d, "ZZ_F6_PER_MARZO.pdf", "ZZ_F6_PER_ABRIL.pdf");
    guardar_ok(pool, d, &negocios, &archivos)?;

    // 1. Obra vigente marzo-abril asignada por el analista a ambos meses
    let obra = crear_obra(client, d, &numero_obra_unico(), "2026-04-30")?;
    sync_reportes_liquidaciones_for_revision(client, d.id_revision)?;
    let marzo = reporte_de(client, d, 3)?;
    let abril = reporte_de(client, d, 4)?;
    batch_assign_multiple_obras_to_employees(
        client,
        d.id_revision,
        &[marzo.id, abril.id],
        &[obra],
        None,
        None,
        None,
    )?;
    let abril = reporte_de(client, d, 4)?;
    exigir_igual(abril.editado, true, "editado_usuario de ABRIL tras la asignación")?;
    let dias_abril = dias_de_obra_en_reporte(client, abril.id, obra)?;
    exigir_igual(dias_abril.is_some(), true, "ABRIL debe tener la obra asignada")?;
    let subtotal_abril = subtotal_reporte(client, abril.id)?;
    println!("[1] Obra asignada a MARZO y ABRIL ({dias_abril:?} días de obra en ABRIL)");

    // 2. Periodo de la revisión solo MARZO: ABRIL queda FUERA_PERIODO sin borrarse
    fijar_periodo(client, d, "2026-03-01", "2026-03-31")?;
    sync_reportes_liquidaciones_for_revision(client, d.id_revision)?;
    let abril_fuera = reporte_de(client, d, 4)?;
    exigir_igual(abril_fuera.id, abril.id, "ABRIL debe conservar su reporte")?;
    exigir_igual(abril_fuera.estado.as_str(), "FUERA_PERIODO", "estado de ABRIL con periodo solo marzo")?;
    exigir_igual(abril_fuera.editado, true, "ABRIL debe conservar editado_usuario")?;
    exigir_igual(dias_de_obra_en_reporte(client, abril.id, obra)?, dias_abril, "días de obra de ABRIL fuera de periodo")?;
    exigir_cerca(subtotal_reporte(client, abril.id)?, subtotal_abril, "subtotal de ABRIL fuera de periodo")?;
    exigir_igual(
        contar(client, "SELECT COUNT(*) FROM public.reportes_liquidaciones WHERE id_revision = $1", &d.id_revision)?,
        2,
        "ningún reporte se borra al salir de periodo",
    )?;
    println!("[2] ABRIL FUERA_PERIODO: reporte, obra y días conservados");

    // 3. Periodo ampliado a MARZO-ABRIL: ABRIL vuelve a ACTIVO con la misma obra y días
    fijar_periodo(client, d, "2026-03-01", "2026-04-30")?;
    sync_reportes_liquidaciones_for_revision(client, d.id_revision)?;
    let abril_regreso = reporte_de(client, d, 4)?;
    exigir_igual(abril_regreso.id, abril.id, "ABRIL debe ser el mismo reporte al regresar")?;
    exigir_igual(abril_regreso.estado.as_str(), "ACTIVO", "estado de ABRIL con periodo ampliado")?;
    exigir_igual(abril_regreso.editado, true, "ABRIL debe seguir editado al regresar")?;
    exigir_igual(dias_de_obra_en_reporte(client, abril.id, obra)?, dias_abril, "días de obra de ABRIL al regresar")?;
    exigir_cerca(subtotal_reporte(client, abril.id)?, subtotal_abril, "subtotal de ABRIL al regresar")?;
    println!("[3] ABRIL vuelve a ACTIVO con la misma obra y días");
    Ok(())
}

/// Fallo de RustFS tras el COMMIT: PostgreSQL conserva los datos, el archivo queda STAGED y el reintento lo promueve.
fn escenario_rustfs(pool: &DbPool, client: &mut Client, d: &DatosTemporales) -> Result<(), String> {
    const ARCH: &str = "ZZ_F6_RUSTFS_MARZO.pdf";
    let clave_final = format!("prueba/final/{ARCH}");

    // 1. Subir el "PDF" a staging en el almacén en memoria
    let almacen = AlmacenMemoria::new();
    let clave_staging = almacen.subir_staging(&clave_final, "B1", b"%PDF-1.4 prueba fase 6")?;
    let mut archivo_lote = archivo(ARCH, 3);
    archivo_lote.clave_staging = clave_staging;

    // 2. Guardar con promoción fallida: debe devolver Ok con la advertencia en errores
    let negocios = vec![negocio(&d.registro, vec![empleado("MARZO", 3, 3100.0, ARCH)])];
    let promover_caido: &(dyn Fn() -> Result<(), String> + Send + Sync) = &|| Err("RustFS caído (simulado)".to_string());
    let errores = guardar_con_promocion(pool, d, &negocios, &[archivo_lote], Some(promover_caido))?;
    exigir_igual(
        errores.iter().any(|e| e.contains("pendientes")),
        true,
        "advertencia de promoción pendiente en stats.errores",
    )?;
    exigir_igual(estado_archivo(client, d, ARCH)?, "STAGED".to_string(), "estado del archivo tras el fallo")?;

    let (liq, id_archivo) = liquidacion_de(client, d)?;
    exigir_igual(id_archivo, Some(id_archivo_por_nombre(client, d, ARCH)?), "la liquidación apunta al archivo staged")?;
    exigir_igual(subtotales_de(client, liq)?, vec![Some(3100.0)], "los datos quedan guardados en liquidaciones")?;
    exigir_igual(almacen.existe(&clave_final)?, false, "la clave final no existe antes del reintento")?;
    println!("[1] Fallo de RustFS: Ok con advertencia, archivo STAGED, datos guardados");

    // 3. Reintento tras la antigüedad mínima: el archivo pasa a ACTIVO y existe en el almacén
    client
        .execute(
            "UPDATE public.archivos_cedulas SET created_at = now() - interval '10 minutes' WHERE id_registro_patronal = $1",
            &[&d.id_rp],
        )
        .map_err(|e| format!("Error envejeciendo el archivo de prueba: {e}"))?;
    // El reintento recorre todos los STAGED de la BD; solo promueve los que existen en este almacén.
    let (promovidos, _fallidos) = reintentar_promociones_pendientes(client, &almacen, 5)?;
    exigir_igual(promovidos >= 1, true, "el reintento debe promover al menos el archivo de prueba")?;
    exigir_igual(estado_archivo(client, d, ARCH)?, "ACTIVO".to_string(), "estado del archivo tras el reintento")?;
    exigir_igual(almacen.existe(&clave_final)?, true, "la clave final existe tras el reintento")?;
    println!("[2] Reintento: archivo ACTIVO y presente en el almacén");
    Ok(())
}

/// Versionado de entregables: versiones consecutivas, duplicado rechazado y listado con el sha registrado.
fn escenario_entregables(_pool: &DbPool, client: &mut Client, d: &DatosTemporales) -> Result<(), String> {
    let version = siguiente_version(client, d.id_revision, "XLSX")?;
    exigir_igual(version, 1, "primera versión XLSX")?;

    let sha = sha256_hex(b"entregable fase 6 prueba");
    let resumen = serde_json::json!({ "origen": "prueba fase 6" });
    registrar_exportacion(client, d.id_revision, "XLSX", 1, &sha, "prueba/exportaciones/v1.xlsx", &resumen)?;
    exigir_igual(siguiente_version(client, d.id_revision, "XLSX")?, 2, "siguiente versión tras registrar la v1")?;
    println!("[1] Versión 1 registrada; siguiente versión = 2");

    match registrar_exportacion(client, d.id_revision, "XLSX", 1, &sha, "prueba/exportaciones/v1-bis.xlsx", &resumen) {
        Ok(_) => return Err("registrar la versión 1 por segunda vez debió fallar".to_string()),
        Err(e) => exigir_igual(e.contains("ya existe"), true, "mensaje de versión duplicada")?,
    }
    println!("[2] Versión 1 repetida rechazada con error claro");

    let lista = listar_exportaciones(client, d.id_revision)?;
    exigir_igual(lista.len(), 1, "exportaciones listadas")?;
    exigir_igual(lista[0].version, 1, "versión listada")?;
    exigir_igual(lista[0].sha256.as_str(), sha.as_str(), "sha256 listado")?;
    println!("[3] Listado: 1 exportación con el sha256 registrado");
    Ok(())
}

#[test]
#[ignore]
fn fusion_conserva_ediciones_y_marca_cedula_actualizada_bd() {
    correr_prueba("fusion_conserva_ediciones_y_marca_cedula_actualizada_bd", escenario_fusion);
}

#[test]
#[ignore]
fn mismo_nss_en_dos_meses_un_solo_empleado_bd() {
    correr_prueba("mismo_nss_en_dos_meses_un_solo_empleado_bd", escenario_mismo_nss);
}

#[test]
#[ignore]
fn periodo_fuera_y_regreso_conserva_ediciones_bd() {
    correr_prueba("periodo_fuera_y_regreso_conserva_ediciones_bd", escenario_periodo_fuera_y_regreso);
}

#[test]
#[ignore]
fn fallo_rustfs_tras_commit_queda_staged_y_reintento_promueve_bd() {
    correr_prueba("fallo_rustfs_tras_commit_queda_staged_y_reintento_promueve_bd", escenario_rustfs);
}

#[test]
#[ignore]
fn registro_y_versionado_de_entregables_bd() {
    correr_prueba("registro_y_versionado_de_entregables_bd", escenario_entregables);
}
