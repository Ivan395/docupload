#![allow(dead_code)] // se cablea en la UI de Fase 5

//! Lista de verificación previa al entregable IMSS (lógica pura, sin BD ni UI).

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use uuid::Uuid;

use crate::db::prorrateo::decimal_desde_f64;
use crate::db::reporte_liquidaciones::{
    compute_guarderias_kpis, missing_header_fields, ObraOptionDb, ReporteFinalHeaderMeta,
    ReporteLiquidacionRowDb,
};

/// Severidad de un punto de la lista de verificación.
#[derive(Debug, Clone, PartialEq)]
pub enum Severidad {
    /// Impide generar el entregable.
    Bloqueante,
    /// Se puede entregar, pero el analista debe aceptarla explícitamente.
    Advertencia,
    /// Dato informativo.
    Info,
}

/// Punto individual de la lista de verificación.
#[derive(Debug, Clone, PartialEq)]
pub struct PuntoVerificacion {
    pub severidad: Severidad,
    pub titulo: String,
    pub detalle: String,
}

/// Resultado completo de la verificación previa al entregable.
#[derive(Debug, Clone, PartialEq)]
pub struct ListaVerificacion {
    pub puntos: Vec<PuntoVerificacion>,
    /// `true` cuando no existe ningún punto bloqueante.
    pub puede_entregar: bool,
    pub resumen: ResumenEntregable,
}

/// Totales del entregable calculados sobre las filas activas.
#[derive(Debug, Clone, PartialEq)]
pub struct ResumenEntregable {
    pub filas: usize,
    pub empleados: usize,
    pub empleado_mes: usize,
    pub total_subtotal: f64,
    pub total_patronal: f64,
    pub total_obrera: f64,
    pub total_gps: f64,
}

/// Tolerancia permitida al comparar la suma de tramos contra la cédula original.
fn tolerancia_cuota() -> Decimal {
    Decimal::new(5, 2)
}

/// Redondea a 2 decimales (mitad lejos de cero) y convierte a f64 para presentación.
fn a_f64_2dp(d: Decimal) -> f64 {
    d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
        .to_f64()
        .unwrap_or(0.0)
}

/// Indica si la clave del tramo corresponde a una baja (sin distinguir mayúsculas).
fn es_baja(clave: &str) -> bool {
    clave.trim().eq_ignore_ascii_case("baja")
}

/// Indica dónde capturar en la aplicación un campo obligatorio del encabezado.
fn donde_capturar(campo: &str) -> &'static str {
    match campo {
        "Representante legal" | "Razón social del cliente" | "RFC del cliente" => {
            "Clientes (Vista 1) → Editar"
        }
        "Delegación IMSS" | "Subdelegación IMSS" | "Registro patronal" => {
            "Registros Patronales (Vista 2)"
        }
        "Folio de la revisión" | "Periodo de obra/revisión" => {
            "Revisiones (Vista 3) → Editar (Periodo revisado)"
        }
        _ => "Revisa el encabezado del reporte",
    }
}

/// Evalúa si el Reporte de Liquidaciones está listo para entregarse al IMSS.
///
/// `filas` son TODAS las filas activas que irán al entregable, no solo las visibles en pantalla.
///
/// Política D-1 (entregable vN):
/// - Bloqueantes (impiden entregar): campos obligatorios del encabezado vacíos
///   (`missing_header_fields`) y cuota duplicada.
/// - Todo lo demás es advertencia o información: sin filas activas, empleado-mes sin obra,
///   tramos inconsistentes, obras sin monto, obras que superan el 18% y cédulas actualizadas.
///
/// `puede_entregar` es `bloqueantes.is_empty()`.
pub fn evaluar_lista_verificacion(
    meta: &ReporteFinalHeaderMeta,
    filas: &[ReporteLiquidacionRowDb],
    obras: &[ObraOptionDb],
    hay_filtros_activos: bool,
) -> ListaVerificacion {
    let mut bloqueantes: Vec<PuntoVerificacion> = Vec::new();
    let mut advertencias: Vec<PuntoVerificacion> = Vec::new();
    let mut info: Vec<PuntoVerificacion> = Vec::new();

    // Encabezado oficial: campos obligatorios vacíos.
    for campo in missing_header_fields(meta) {
        bloqueantes.push(PuntoVerificacion {
            severidad: Severidad::Bloqueante,
            titulo: format!("Falta: {campo}"),
            detalle: format!("Captura el dato en {}.", donde_capturar(campo)),
        });
    }

    if filas.is_empty() {
        advertencias.push(PuntoVerificacion {
            severidad: Severidad::Advertencia,
            titulo: "No hay filas activas para entregar".to_string(),
            detalle: "Activa al menos un registro antes de generar el entregable.".to_string(),
        });
    }

    // Invariante de dinero: por movimiento (reporte + clave normalizada + fecha) la suma de
    // subtotales no debe superar la cédula original (más la tolerancia).
    // Se toma el mayor subtotal_orig del grupo como importe de la cédula.
    let mut movimientos: HashMap<(Uuid, String, Option<NaiveDate>), (Decimal, Decimal)> =
        HashMap::new();
    for f in filas {
        let clave = f.clave.trim().to_lowercase();
        let entrada = movimientos
            .entry((f.id_parent_reporte_liq, clave, f.fecha))
            .or_insert((Decimal::ZERO, Decimal::ZERO));
        entrada.0 += decimal_desde_f64(f.subtotal);
        let orig = decimal_desde_f64(f.subtotal_orig);
        if orig > entrada.1 {
            entrada.1 = orig;
        }
    }
    let duplicados = movimientos
        .values()
        .filter(|(suma, orig)| *suma > *orig + tolerancia_cuota())
        .count();
    if duplicados > 0 {
        bloqueantes.push(PuntoVerificacion {
            severidad: Severidad::Bloqueante,
            titulo: format!("Cuota duplicada en {duplicados} movimiento(s)"),
            detalle: "La suma de los tramos supera el importe de la cédula; corrige los días en Asignación por empleado (filtro Con inconsistencia).".to_string(),
        });
    }

    // Empleado-mes (reporte) no-baja sin ningún tramo asignado a obra.
    let mut reportes_con_no_baja: HashSet<Uuid> = HashSet::new();
    let mut reportes_con_obra: HashSet<Uuid> = HashSet::new();
    for f in filas {
        if !es_baja(&f.clave) {
            reportes_con_no_baja.insert(f.id_parent_reporte_liq);
        }
        if f.id_registro_obra.is_some() && !es_baja(&f.clave) {
            reportes_con_obra.insert(f.id_parent_reporte_liq);
        }
    }
    let sin_obra = reportes_con_no_baja.difference(&reportes_con_obra).count();
    if sin_obra > 0 {
        advertencias.push(PuntoVerificacion {
            severidad: Severidad::Advertencia,
            titulo: format!("{sin_obra} empleado-mes sin obra asignada"),
            detalle: "Asigna obra en Asignación por empleado; sin obra se usa el importe completo.".to_string(),
        });
    }

    let tramos_inconsistentes = filas.iter().filter(|f| f.tiene_inconsistencia).count();
    if tramos_inconsistentes > 0 {
        advertencias.push(PuntoVerificacion {
            severidad: Severidad::Advertencia,
            titulo: format!("{tramos_inconsistentes} tramo(s) con inconsistencia"),
            detalle: "Revísalos en Asignación por empleado (filtro Con inconsistencia).".to_string(),
        });
    }

    // Obras creadas por el usuario sin monto contratado.
    for o in obras.iter().filter(|o| o.creado_usuario && o.monto <= 0.0) {
        advertencias.push(PuntoVerificacion {
            severidad: Severidad::Advertencia,
            titulo: format!(
                "Obra {} sin monto contratado: no se puede calcular el límite del 18%",
                o.numero
            ),
            detalle: "Captura el monto contratado de la obra para validar el tope de guarderías."
                .to_string(),
        });
    }

    // Obras que superan el 18% de guarderías.
    for k in compute_guarderias_kpis(filas, obras, None)
        .into_iter()
        .filter(|k| k.supera_18)
    {
        advertencias.push(PuntoVerificacion {
            severidad: Severidad::Advertencia,
            titulo: format!("Obra {} supera el 18% de guarderías", k.obra_numero),
            detalle: format!(
                "Valor de guarderías {:.2} frente a límite {:.2} (monto {:.2}).",
                k.valor_guarderias, k.limite_18_obra, k.monto_obra
            ),
        });
    }

    let reportes_cedula_actualizada: HashSet<Uuid> = filas
        .iter()
        .filter(|f| f.cedula_actualizada)
        .map(|f| f.id_parent_reporte_liq)
        .collect();
    if !reportes_cedula_actualizada.is_empty() {
        advertencias.push(PuntoVerificacion {
            severidad: Severidad::Advertencia,
            titulo: format!(
                "{} reporte(s) con cédula actualizada pendiente de revisar",
                reportes_cedula_actualizada.len()
            ),
            detalle: "Revisa las cédulas actualizadas antes de entregar.".to_string(),
        });
    }

    if hay_filtros_activos {
        info.push(PuntoVerificacion {
            severidad: Severidad::Info,
            titulo: "Hay filtros activos en pantalla".to_string(),
            detalle: "El entregable incluye TODAS las filas activas, no solo las visibles."
                .to_string(),
        });
    }

    let resumen = calcular_resumen(filas);
    let puede_entregar = bloqueantes.is_empty();

    let mut puntos = bloqueantes;
    puntos.extend(advertencias);
    puntos.extend(info);

    ListaVerificacion {
        puntos,
        puede_entregar,
        resumen,
    }
}

/// Calcula los totales del entregable. Las sumas se hacen en `Decimal` y se redondean a 2 decimales.
fn calcular_resumen(filas: &[ReporteLiquidacionRowDb]) -> ResumenEntregable {
    let mut subtotal = Decimal::ZERO;
    let mut patronal = Decimal::ZERO;
    let mut obrera = Decimal::ZERO;
    let mut gps = Decimal::ZERO;
    let mut empleados: HashSet<Uuid> = HashSet::new();
    let mut empleado_mes: HashSet<Uuid> = HashSet::new();

    for f in filas {
        subtotal += decimal_desde_f64(f.subtotal);
        patronal += decimal_desde_f64(f.patronal);
        obrera += decimal_desde_f64(f.obrera);
        gps += decimal_desde_f64(f.gps);
        empleados.insert(f.id_empleado);
        empleado_mes.insert(f.id_parent_reporte_liq);
    }

    ResumenEntregable {
        filas: filas.len(),
        empleados: empleados.len(),
        empleado_mes: empleado_mes.len(),
        total_subtotal: a_f64_2dp(subtotal),
        total_patronal: a_f64_2dp(patronal),
        total_obrera: a_f64_2dp(obrera),
        total_gps: a_f64_2dp(gps),
    }
}

/// Serializa el resumen y los títulos de advertencias aceptadas para guardarlos con la versión del entregable.
pub fn resumen_json(l: &ListaVerificacion) -> serde_json::Value {
    let advertencias_aceptadas: Vec<&str> = l
        .puntos
        .iter()
        .filter(|p| p.severidad == Severidad::Advertencia)
        .map(|p| p.titulo.as_str())
        .collect();

    serde_json::json!({
        "puede_entregar": l.puede_entregar,
        "filas": l.resumen.filas,
        "empleados": l.resumen.empleados,
        "empleado_mes": l.resumen.empleado_mes,
        "total_subtotal": l.resumen.total_subtotal,
        "total_patronal": l.resumen.total_patronal,
        "total_obrera": l.resumen.total_obrera,
        "total_gps": l.resumen.total_gps,
        "advertencias_aceptadas": advertencias_aceptadas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_completo() -> ReporteFinalHeaderMeta {
        ReporteFinalHeaderMeta {
            delegacion: "Delegación Norte".to_string(),
            subdelegacion: "Subdelegación 01".to_string(),
            cliente_nombre: "Cliente Demo SA".to_string(),
            rp_registro: "A1234567890".to_string(),
            rfc: "AAA010101AAA".to_string(),
            periodo_obra: "2026-01".to_string(),
            representante_legal: "Representante Demo".to_string(),
            revision_folio: "R-001".to_string(),
        }
    }

    fn obra(n: u128, numero: &str, monto: f64) -> ObraOptionDb {
        ObraOptionDb {
            id: Uuid::from_u128(n),
            numero: numero.to_string(),
            tipo_obra: "Obra".to_string(),
            fecha_inicio: None,
            fecha_fin: None,
            label: format!("Obra {numero}"),
            creado_usuario: true,
            monto,
        }
    }

    /// Construye un tramo con valores por defecto; solo varían reporte, empleado, clave, obra e importes.
    fn tramo(
        reporte: u128,
        empleado: u128,
        clave: &str,
        obra_id: Option<u128>,
        subtotal: f64,
        subtotal_orig: f64,
    ) -> ReporteLiquidacionRowDb {
        ReporteLiquidacionRowDb {
            id_reporte_liq: Uuid::from_u128(reporte),
            id_parent_reporte_liq: Uuid::from_u128(reporte),
            id_liquidacion: Uuid::nil(),
            id_revision: Uuid::nil(),
            id_empleado: Uuid::from_u128(empleado),
            nombre_trabajador: "Trabajador Demo".to_string(),
            rfc: String::new(),
            curp: String::new(),
            nss: String::new(),
            pensionado: String::new(),
            registro_patronal: "A1234567890".to_string(),
            mes_periodo: "Enero".to_string(),
            mes_periodo_int: 1,
            anio_periodo: 2026,
            periodo_str: "2026-01".to_string(),
            dias_cedula: 30,
            id_registro_obra: obra_id.map(Uuid::from_u128),
            obra_numero: obra_id.map(|o| o.to_string()).unwrap_or_default(),
            obra_label: String::new(),
            dias_obra: 0,
            obra_creado_usuario: obra_id.is_some(),
            clave: clave.to_string(),
            fecha: NaiveDate::from_ymd_opt(2026, 1, 15),
            fecha_str: "15/01/2026".to_string(),
            sdi: 0.0,
            lic: 0.0,
            inc: 0.0,
            aus: 0.0,
            cf: 0.0,
            exc_pat: 0.0,
            exc_obr: 0.0,
            pd_pat: 0.0,
            pd_obr: 0.0,
            gmp_pat: 0.0,
            gmp_obr: 0.0,
            rt: 0.0,
            iv_pat: 0.0,
            iv_obr: 0.0,
            gps: 0.0,
            patronal: 0.0,
            obrera: 0.0,
            subtotal,
            cf_orig: 0.0,
            exc_pat_orig: 0.0,
            exc_obr_orig: 0.0,
            pd_pat_orig: 0.0,
            pd_obr_orig: 0.0,
            gmp_pat_orig: 0.0,
            gmp_obr_orig: 0.0,
            rt_orig: 0.0,
            iv_pat_orig: 0.0,
            iv_obr_orig: 0.0,
            gps_orig: 0.0,
            patronal_orig: 0.0,
            obrera_orig: 0.0,
            subtotal_orig,
            tiene_inconsistencia: false,
            observacion_validacion: String::new(),
            estado: "activo".to_string(),
            cedula_actualizada: false,
            editado_usuario: false,
        }
    }

    fn tiene_bloqueante(l: &ListaVerificacion, fragmento: &str) -> bool {
        l.puntos
            .iter()
            .any(|p| p.severidad == Severidad::Bloqueante && p.titulo.contains(fragmento))
    }

    fn tiene_advertencia(l: &ListaVerificacion, fragmento: &str) -> bool {
        l.puntos
            .iter()
            .any(|p| p.severidad == Severidad::Advertencia && p.titulo.contains(fragmento))
    }

    #[test]
    fn entregable_logic_completo_sin_bloqueantes() {
        let filas = vec![tramo(1, 1, "ORD", Some(10), 1000.0, 1000.0)];
        let obras = vec![obra(10, "OB-1", 100_000.0)];
        let l = evaluar_lista_verificacion(&meta_completo(), &filas, &obras, false);
        assert!(l.puede_entregar);
        assert!(l.puntos.iter().all(|p| p.severidad != Severidad::Bloqueante));
        assert!(l.puntos.is_empty());
    }

    #[test]
    fn entregable_logic_sin_representante_bloquea() {
        let mut meta = meta_completo();
        meta.representante_legal = "   ".to_string();
        let filas = vec![tramo(1, 1, "ORD", Some(10), 1000.0, 1000.0)];
        let obras = vec![obra(10, "OB-1", 100_000.0)];
        let l = evaluar_lista_verificacion(&meta, &filas, &obras, false);
        assert!(!l.puede_entregar);
        assert!(tiene_bloqueante(&l, "Falta: Representante legal"));
        let punto = l
            .puntos
            .iter()
            .find(|p| p.titulo == "Falta: Representante legal")
            .unwrap();
        assert!(punto.detalle.contains("Clientes (Vista 1)"));
    }

    #[test]
    fn entregable_logic_sin_filas_advierte_no_bloquea() {
        let l = evaluar_lista_verificacion(&meta_completo(), &[], &[], false);
        assert!(l.puede_entregar);
        assert!(!tiene_bloqueante(&l, "No hay filas activas"));
        assert!(tiene_advertencia(&l, "No hay filas activas"));
    }

    #[test]
    fn entregable_logic_cuota_duplicada_bloquea() {
        // Dos tramos del mismo movimiento (mismo reporte, clave y fecha) que suman 6200 sobre cédula 3100.
        let filas = vec![
            tramo(1, 1, "ORD", Some(10), 3100.0, 3100.0),
            tramo(1, 1, "ord", Some(10), 3100.0, 3100.0),
        ];
        let obras = vec![obra(10, "OB-1", 100_000.0)];
        let l = evaluar_lista_verificacion(&meta_completo(), &filas, &obras, false);
        assert!(!l.puede_entregar);
        assert!(tiene_bloqueante(&l, "Cuota duplicada en 1 movimiento(s)"));
    }

    #[test]
    fn entregable_logic_tolerancia_cinco_centavos() {
        // Diferencia de 0.05 exacta no es duplicado.
        let filas = vec![
            tramo(1, 1, "ORD", Some(10), 1500.0, 3000.0),
            tramo(1, 1, "ORD", Some(10), 1500.05, 3000.0),
        ];
        let obras = vec![obra(10, "OB-1", 100_000.0)];
        let l = evaluar_lista_verificacion(&meta_completo(), &filas, &obras, false);
        assert!(l.puede_entregar);
    }

    #[test]
    fn entregable_logic_baja_no_cuenta_como_movimiento_con_obra() {
        // Una baja sin obra no genera advertencia de falta de obra.
        let filas = vec![tramo(1, 1, "Baja", None, 0.0, 0.0)];
        let l = evaluar_lista_verificacion(&meta_completo(), &filas, &[], false);
        assert!(l.puede_entregar);
        assert!(!tiene_advertencia(&l, "sin obra asignada"));
    }

    #[test]
    fn entregable_logic_sin_obra_advierte() {
        let filas = vec![tramo(1, 1, "ORD", None, 1000.0, 1000.0)];
        let l = evaluar_lista_verificacion(&meta_completo(), &filas, &[], false);
        assert!(l.puede_entregar);
        assert!(tiene_advertencia(&l, "1 empleado-mes sin obra asignada"));
    }

    #[test]
    fn entregable_logic_obra_sin_monto_advierte() {
        let filas = vec![tramo(1, 1, "ORD", Some(10), 1000.0, 1000.0)];
        let obras = vec![obra(10, "OB-7", 0.0)];
        let l = evaluar_lista_verificacion(&meta_completo(), &filas, &obras, false);
        assert!(l.puede_entregar);
        assert!(tiene_advertencia(
            &l,
            "Obra OB-7 sin monto contratado: no se puede calcular el límite del 18%"
        ));
    }

    #[test]
    fn entregable_logic_filtros_activos_info() {
        let filas = vec![tramo(1, 1, "ORD", Some(10), 1000.0, 1000.0)];
        let obras = vec![obra(10, "OB-1", 100_000.0)];
        let l = evaluar_lista_verificacion(&meta_completo(), &filas, &obras, true);
        assert!(l.puede_entregar);
        assert!(l.puntos.iter().any(|p| p.severidad == Severidad::Info
            && p.detalle.contains("TODAS las filas activas")));
    }

    #[test]
    fn entregable_logic_resumen_suma_con_decimal() {
        let mut a = tramo(1, 1, "ORD", Some(10), 0.1, 0.1);
        a.patronal = 0.1;
        a.obrera = 0.2;
        a.gps = 0.3;
        let mut b = tramo(2, 2, "ORD", Some(10), 0.2, 0.2);
        b.patronal = 0.2;
        b.obrera = 0.1;
        b.gps = 0.3;
        let filas = vec![a, b];
        let obras = vec![obra(10, "OB-1", 100_000.0)];
        let l = evaluar_lista_verificacion(&meta_completo(), &filas, &obras, false);
        let r = &l.resumen;
        assert_eq!(r.filas, 2);
        assert_eq!(r.empleados, 2);
        assert_eq!(r.empleado_mes, 2);
        assert_eq!(r.total_subtotal, 0.3);
        assert_eq!(r.total_patronal, 0.3);
        assert_eq!(r.total_obrera, 0.3);
        assert_eq!(r.total_gps, 0.6);
    }

    #[test]
    fn entregable_logic_resumen_json_lista_advertencias() {
        let filas = vec![tramo(1, 1, "ORD", None, 1000.0, 1000.0)];
        let l = evaluar_lista_verificacion(&meta_completo(), &filas, &[], false);
        let j = resumen_json(&l);
        assert_eq!(j["filas"], 1);
        assert_eq!(j["puede_entregar"], true);
        let aceptadas = j["advertencias_aceptadas"].as_array().unwrap();
        assert_eq!(aceptadas.len(), 1);
        assert_eq!(aceptadas[0], "1 empleado-mes sin obra asignada");
    }
}
