//! Prueba de integración de las operaciones de asignación (asignar lote, agregar obra, cambiar días,
//! quitar obra, deshacer y revertir) contra la BD configurada.
//!
//! AVISO: la BD configurada puede ser de PRODUCCIÓN. La prueba crea su propio cliente temporal
//! (nombre único, RFC NULL) y SIEMPRE lo borra al final, incluso si falla. Se propaga la cascada
//! a RP, revisiones, obras, empleados, liquidaciones y reportes.
//! Solo se ejecuta explícitamente con `cargo test -- --ignored`.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use postgres::Client;
use uuid::Uuid;

use crate::asignacion_view_logic::{planear_cambio_dias_seguro, planear_quitar};
use crate::cedulas::business::Business;
use crate::cedulas::employees::{DataEmployee, Employee};
use crate::cedulas::view_logic::guardar_cedulas_lote;
use crate::db::asignacion_ops::{aplicar_plan_tramos, revertir_reportes_empleado};
use crate::db::bitacora_reportes::{
    deshacer_lote, ultimo_lote_revision, ACCION_DESVINCULAR, ACCION_DIAS,
};
use crate::db::client::{create_client, create_pool, DbPool};
use crate::db::ingesta::{ArchivoLote, ArchivoParaRegistrar};
use crate::db::reporte_liquidaciones::{
    add_obra_vinculacion_for_employee, batch_assign_multiple_obras_to_employees,
    fetch_reporte_liquidaciones, sync_reportes_liquidaciones_for_revision, ReporteLiquidacionRowDb,
};
use crate::db::schema::ensure_schema;

const ARCHIVO: &str = "ZZ__RP__MARZO__2026__cccccccccccc.pdf";
const NSS: &str = "12345678903";
const NOMBRE: &str = "PRUEBA ASIGNACION UNO";
/// Folios SIROC válidos ('C' + 7 dígitos), vigentes en marzo 2026.
const NUMERO_OBRA_1: &str = "C7000001";
const NUMERO_OBRA_2: &str = "C7000002";
/// Tolerancia para comparar importes (pesos).
const TOLERANCIA: f64 = 0.05;

/// Identificadores de los datos temporales creados por la prueba.
struct DatosTemporales {
    id_cliente: Uuid,
    id_rp: Uuid,
    id_revision: Uuid,
    registro: String,
}

/// Crea cliente, registro patronal y revisión en una sola transacción.
fn crear_datos_temporales(client: &mut Client) -> Result<DatosTemporales, String> {
    let uuid = Uuid::now_v7();
    // Registro patronal char(11): 'Z' + 10 dígitos derivados del UUID
    let registro = format!("Z{:010}", uuid.as_u128() % 10_000_000_000);
    let nombre_cliente = format!("ZZ PRUEBA ASIGNACION {}", uuid.simple());
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

/// Borra el cliente temporal; la cascada elimina RP, revisiones, obras, reportes y archivos.
fn borrar_datos_temporales(client: &mut Client, id_cliente: Uuid) -> Result<u64, String> {
    client
        .execute("DELETE FROM public.clientes WHERE id = $1", &[&id_cliente])
        .map_err(|e| format!("Error borrando cliente temporal {id_cliente}: {e}"))
}

/// Crea una obra del usuario vigente todo marzo 2026 en la revisión temporal.
fn crear_obra(client: &mut Client, d: &DatosTemporales, numero: &str) -> Result<Uuid, String> {
    client
        .query_one(
            "INSERT INTO public.registros_obras \
             (id_revision, id_registro_patronal, numero, clase, tipo_obra, fecha_inicio, fecha_fin, monto, creado_usuario) \
             VALUES ($1, $2, $3, 'PRUEBA', 'OBRA DE PRUEBA', '2026-03-01'::date, '2026-03-31'::date, 1000.0, true) \
             RETURNING id",
            &[&d.id_revision, &d.id_rp, &numero],
        )
        .map(|fila| fila.get::<_, Uuid>(0))
        .map_err(|e| format!("Error creando obra temporal {numero}: {e}"))
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

/// Renglón de cuotas con importes fijos (cf y subtotal configurables).
fn detalle(cf: f64, subtotal: f64) -> DataEmployee {
    DataEmployee {
        mes_periodo: Some("MARZO".to_string()),
        mes_periodo_int: 3,
        anio_periodo: Some("2026".to_string()),
        clave: Some("CUOTA".to_string()),
        dias: Some(31.0),
        sdi: Some(500.0),
        cf: Some(cf),
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

/// Archivo de lote de prueba con SHA-256 uniforme.
fn archivo() -> ArchivoParaRegistrar {
    ArchivoParaRegistrar {
        nombre_archivo: ARCHIVO.to_string(),
        lote: ArchivoLote {
            nombre_original: ARCHIVO.to_string(),
            sha256: 'c'.to_string().repeat(64),
            tipo: "SUA".to_string(),
            anio: 2026,
            mes: 3,
            trabajadores: 1,
            paginas: Some(1),
        },
        clave_final: format!("prueba/final/{ARCHIVO}"),
        clave_staging: format!("prueba/staging/{ARCHIVO}"),
    }
}

/// Guarda la cédula de un trabajador de 31 días, cf 310 y subtotal 3100, sin obras.
fn guardar_cedula(pool: &DbPool, d: &DatosTemporales) -> Result<(), String> {
    let empleado = Employee {
        no_seguro_social: Some(NSS.to_string()),
        nombre: Some(NOMBRE.to_string()),
        mes_periodo: Some("MARZO".to_string()),
        mes_periodo_int: 3,
        anio_periodo: Some("2026".to_string()),
        file_name: Some(ARCHIVO.to_string()),
        data_employee: vec![detalle(310.0, 3100.0)],
        registro_obras: Vec::new(),
        ..Default::default()
    };
    let negocios = vec![Business {
        registro_patronal: Some(d.registro.clone()),
        employees: vec![empleado],
        ..Default::default()
    }];
    let archivos = vec![archivo()];
    let promover_ok: &(dyn Fn() -> Result<(), String> + Send + Sync) = &|| Ok(());
    let id_revision = d.id_revision.to_string();
    let stats = guardar_cedulas_lote(pool, &id_revision, &negocios, &archivos, &[], Some(promover_ok), None)?;
    if !stats.errores.is_empty() {
        return Err(format!("guardar_cedulas_lote reportó errores: {:?}", stats.errores));
    }
    Ok(())
}

/// Reportes de la revisión temporal como (id de reporte, id de empleado).
fn reportes_de(client: &mut Client, d: &DatosTemporales) -> Result<Vec<(Uuid, Uuid)>, String> {
    client
        .query(
            "SELECT id, id_empleado FROM public.reportes_liquidaciones WHERE id_revision = $1 ORDER BY id",
            &[&d.id_revision],
        )
        .map(|filas| filas.iter().map(|f| (f.get(0), f.get(1))).collect())
        .map_err(|e| format!("Error leyendo reportes de liquidaciones: {e}"))
}

/// Tramos (renglones de detalle) de un reporte, leídos con la misma función que usa la pantalla.
fn tramos_de(client: &mut Client, d: &DatosTemporales, reporte: Uuid) -> Result<Vec<ReporteLiquidacionRowDb>, String> {
    let todos = fetch_reporte_liquidaciones(client, d.id_revision)?;
    Ok(todos
        .into_iter()
        .filter(|t| t.id_parent_reporte_liq == reporte)
        .collect())
}

/// Vista prestada de los tramos para los planificadores puros.
fn como_refs(tramos: &[ReporteLiquidacionRowDb]) -> Vec<&ReporteLiquidacionRowDb> {
    tramos.iter().collect()
}

/// Suma de subtotales de todos los tramos de un movimiento.
fn suma(tramos: &[ReporteLiquidacionRowDb]) -> f64 {
    tramos.iter().map(|t| t.subtotal).sum()
}

/// Suma de subtotales de los tramos de una obra (None = sin obra).
fn subtotal_obra(tramos: &[ReporteLiquidacionRowDb], obra: Option<Uuid>) -> f64 {
    tramos
        .iter()
        .filter(|t| t.id_registro_obra == obra)
        .map(|t| t.subtotal)
        .sum()
}

/// Días de obra del tramo que tiene asignada la obra indicada.
fn dias_obra(tramos: &[ReporteLiquidacionRowDb], obra: Uuid) -> Option<i16> {
    tramos
        .iter()
        .find(|t| t.id_registro_obra == Some(obra))
        .map(|t| t.dias_obra)
}

/// Obras distintas presentes en los tramos (None = sin obra), ordenadas.
fn obras_en(tramos: &[ReporteLiquidacionRowDb]) -> Vec<Option<Uuid>> {
    let mut obras: Vec<Option<Uuid>> = tramos.iter().map(|t| t.id_registro_obra).collect();
    obras.sort();
    obras.dedup();
    obras
}

/// Indicador de editado_usuario del reporte.
fn editado_usuario(client: &mut Client, reporte: Uuid) -> Result<bool, String> {
    client
        .query_one(
            "SELECT editado_usuario FROM public.reportes_liquidaciones WHERE id = $1",
            &[&reporte],
        )
        .map(|fila| fila.get::<_, bool>(0))
        .map_err(|e| format!("Error leyendo editado_usuario: {e}"))
}

/// Cuenta movimientos (reporte, clave normalizada, fecha) de reportes ACTIVO de la revisión cuya
/// suma de subtotales de tramos excede el renglón de la liquidación en más de la tolerancia.
/// Usa el máximo del renglón de detalle de origen para no multiplicar la suma por duplicados.
fn movimientos_duplicados(client: &mut Client, id_revision: Uuid) -> Result<i64, String> {
    client
        .query_one(
            "SELECT COUNT(*)::bigint FROM (
                 SELECT rl.id, UPPER(TRIM(rdl.clave)), rdl.fecha
                   FROM public.reportes_detalles_liquidaciones rdl
                   JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
                   JOIN (
                        SELECT id_liquidacion, UPPER(TRIM(clave)) AS clave_norm, fecha, MAX(subtotal) AS subtotal
                          FROM public.detalles_liquidaciones
                         GROUP BY id_liquidacion, UPPER(TRIM(clave)), fecha
                   ) dl ON dl.id_liquidacion = rl.id_liquidacion
                       AND dl.clave_norm = UPPER(TRIM(rdl.clave))
                       AND dl.fecha IS NOT DISTINCT FROM rdl.fecha
                  WHERE rl.id_revision = $1 AND rl.estado = 'ACTIVO'
                  GROUP BY rl.id, UPPER(TRIM(rdl.clave)), rdl.fecha
                 HAVING SUM(rdl.subtotal) > MAX(dl.subtotal) + $2
             ) excedidos",
            &[&id_revision, &TOLERANCIA],
        )
        .map(|fila| fila.get::<_, i64>(0))
        .map_err(|e| format!("Error calculando invariante de dinero: {e}"))
}

/// Exige que no existan movimientos duplicados en la revisión temporal.
fn exigir_sin_duplicados(client: &mut Client, d: &DatosTemporales, contexto: &str) -> Result<(), String> {
    let n = movimientos_duplicados(client, d.id_revision)?;
    exigir_igual(n, 0, &format!("{contexto}: movimientos con cuota duplicada"))
}

/// Escenario completo: asignar, dividir días, quitar, deshacer y revertir sin duplicar cuotas.
fn escenario(pool: &DbPool, client: &mut Client, d: &DatosTemporales) -> Result<(), String> {
    // 1. Cédula sin obras: un reporte con un tramo sin obra de 3100
    guardar_cedula(pool, d)?;
    sync_reportes_liquidaciones_for_revision(client, d.id_revision)?;
    let reportes = reportes_de(client, d)?;
    exigir_igual(reportes.len(), 1, "reportes tras guardar la cédula")?;
    let (reporte, empleado) = reportes[0];
    let t = tramos_de(client, d, reporte)?;
    exigir_igual(t.len(), 1, "tramos tras guardar la cédula")?;
    exigir_igual(obras_en(&t), vec![None], "el tramo debe estar sin obra")?;
    exigir_cerca(suma(&t), 3100.0, "subtotal de la cédula")?;
    exigir_sin_duplicados(client, d, "paso 1")?;
    println!("[1] Cédula guardada: 1 reporte, 1 tramo sin obra, subtotal 3100. Duplicados: 0");

    // 2. Obras del usuario vigentes en marzo 2026
    let obra1 = crear_obra(client, d, NUMERO_OBRA_1)?;
    let obra2 = crear_obra(client, d, NUMERO_OBRA_2)?;
    sync_reportes_liquidaciones_for_revision(client, d.id_revision)?;
    println!("[2] Obras creadas: {NUMERO_OBRA_1} y {NUMERO_OBRA_2}");

    // 3. Asignación en lote de obra1 con los 31 días
    batch_assign_multiple_obras_to_employees(client, d.id_revision, &[reporte], &[obra1], None, None, None)?;
    let t = tramos_de(client, d, reporte)?;
    exigir_igual(dias_obra(&t, obra1), Some(31), "días de obra1 tras la asignación en lote")?;
    exigir_cerca(suma(&t), 3100.0, "subtotal tras la asignación en lote")?;
    exigir_sin_duplicados(client, d, "paso 3")?;
    println!("[3] Asignación en lote de {NUMERO_OBRA_1}: 31 días, subtotal 3100. Duplicados: 0");

    // 4. Primero liberar días (obra1 a 21) y luego agregar obra2 con 10: la BD rechaza (correctamente)
    //    asignar más días de obra que los de cédula, así que el orden importa.
    let t = tramos_de(client, d, reporte)?;
    let (borrar, guardar) = planear_cambio_dias_seguro(&como_refs(&t), obra1, 21)?;
    aplicar_plan_tramos(client, ACCION_DIAS, &borrar, &guardar)?;
    add_obra_vinculacion_for_employee(client, reporte, obra2, 10)?;
    let t = tramos_de(client, d, reporte)?;
    println!("[4a] Tras liberar días y agregar {NUMERO_OBRA_2}: {} tramos", t.len());
    exigir_igual(dias_obra(&t, obra1), Some(21), "días de obra1 tras el cambio")?;
    exigir_igual(dias_obra(&t, obra2), Some(10), "días de obra2 tras el cambio")?;
    exigir_cerca(subtotal_obra(&t, Some(obra1)), 2100.0, "subtotal de obra1")?;
    exigir_cerca(subtotal_obra(&t, Some(obra2)), 1000.0, "subtotal de obra2")?;
    exigir_cerca(suma(&t), 3100.0, "suma de subtotales del movimiento")?;
    exigir_sin_duplicados(client, d, "paso 4")?;
    println!("[4] {NUMERO_OBRA_1} 21 días (2100) + {NUMERO_OBRA_2} 10 días (1000) = 3100. Duplicados: 0");

    // 5. Quitar obra2: solo queda obra1
    let t = tramos_de(client, d, reporte)?;
    let (borrar, guardar) = planear_quitar(&como_refs(&t), obra2);
    aplicar_plan_tramos(client, ACCION_DESVINCULAR, &borrar, &guardar)?;
    let t = tramos_de(client, d, reporte)?;
    exigir_igual(obras_en(&t), vec![Some(obra1)], "solo debe quedar obra1 tras quitar obra2")?;
    exigir_sin_duplicados(client, d, "paso 5")?;
    println!("[5] Quitada {NUMERO_OBRA_2}: solo queda {NUMERO_OBRA_1}. Duplicados: 0");

    // 6. Deshacer el último lote (DESVINCULAR) y verificar la pila de deshacer
    let (lote, accion) = ultimo_lote_revision(client, d.id_revision)?
        .ok_or_else(|| "no existe lote para deshacer".to_string())?;
    exigir_igual(accion.as_str(), ACCION_DESVINCULAR, "acción del último lote")?;
    deshacer_lote(client, lote)?;
    let t = tramos_de(client, d, reporte)?;
    exigir_igual(dias_obra(&t, obra1), Some(21), "obra1 tras deshacer")?;
    exigir_igual(dias_obra(&t, obra2), Some(10), "obra2 restaurada tras deshacer")?;
    exigir_cerca(suma(&t), 3100.0, "suma tras deshacer")?;
    exigir_sin_duplicados(client, d, "paso 6")?;
    let siguiente = ultimo_lote_revision(client, d.id_revision)?;
    exigir_igual(
        siguiente.map(|(id, _)| id) == Some(lote),
        false,
        "el lote ya deshecho no debe volver a aparecer como último",
    )?;
    println!("[6] Deshecho lote {accion}: vuelven {NUMERO_OBRA_1} 21 d y {NUMERO_OBRA_2} 10 d. Pila de deshacer avanza. Duplicados: 0");

    // 7. Revertir el empleado a la cédula
    let afectados = revertir_reportes_empleado(client, d.id_revision, empleado)?;
    exigir_igual(afectados >= 1, true, "reportes afectados por revertir")?;
    exigir_igual(editado_usuario(client, reporte)?, false, "editado_usuario tras revertir")?;
    let t = tramos_de(client, d, reporte)?;
    exigir_igual(t.len(), 1, "tramos tras revertir")?;
    exigir_igual(obras_en(&t), vec![None], "tras revertir el tramo debe estar sin obra")?;
    exigir_cerca(suma(&t), 3100.0, "subtotal tras revertir")?;
    exigir_sin_duplicados(client, d, "paso 7")?;
    println!("[7] Revertido a cédula: 1 tramo sin obra, subtotal 3100, editado_usuario = false. Duplicados: 0");

    Ok(())
}

#[test]
#[ignore]
fn asignacion_operaciones_y_deshacer_bd() {
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
