//! Tope de días de obra por empleado y mes (FIX-05).
//!
//! Regla: ningún camino de edición puede guardar días de obra por encima del tope
//! (días efectivos, vigencia de la obra en el mes y días ya asignados a otras obras).
//! Los datos históricos que ya violan el tope no bloquean ediciones que no lo empeoran:
//! la validación compara contra la foto "antes" del lote.

use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;
use postgres::GenericClient;
use uuid::Uuid;

use crate::db::bitacora_reportes::{
    ACCION_DESACTIVAR_OBRA, ACCION_DESHACER, ACCION_DESVINCULAR, ACCION_REVERTIR,
};
use crate::db::prorrateo::dias_efectivos;
use crate::db::reporte_liquidaciones::{calculate_employee_active_intervals, EmployeeMovement, MovementKind};
use crate::db::schema::format_pg_error;

/// Tramo de un reporte de liquidaciones tal como queda en la BD (una fila de `reportes_detalles_liquidaciones`).
#[derive(Debug, Clone, PartialEq)]
pub struct TramoTope {
    pub id_reporte: Uuid,
    pub empleado: String,
    pub anio: i16,
    pub mes: i16,
    pub clave: String,
    pub fecha: Option<NaiveDate>,
    pub dias_cedula: i16,
    pub lic: f64,
    pub inc: f64,
    pub aus: f64,
    pub id_obra: Option<Uuid>,
    pub obra_numero: String,
    pub dias_obra: i16,
    pub obra_inicio: Option<NaiveDate>,
    pub obra_fin: Option<NaiveDate>,
}

/// Desglose del tope de días de obra de un empleado en un mes.
#[derive(Debug, Clone, PartialEq)]
pub struct TopeDetalle {
    pub efectivos: i32,
    pub vigencia: Option<i32>,
    pub otras_obras: i32,
    pub tope: i32,
}

/// Violación de tope detectada al cerrar un lote. `obra_numero` None = total del reporte.
#[derive(Debug, Clone, PartialEq)]
pub struct ViolacionTope {
    pub id_reporte: Uuid,
    pub empleado: String,
    pub anio: i16,
    pub mes: i16,
    pub obra_numero: Option<String>,
    pub asignados: i32,
    pub tope: i32,
}

/// SQL de los tramos "después" de todos los reportes del lote.
/// `clave`, `numero` y `nombre` se castean a text para quitar el relleno de los char(n).
const SQL_TRAMOS_LOTE: &str = "SELECT rl.id, COALESCE(e.nombre, '')::text, rl.anio_periodo, rl.mes_periodo_int, \
     COALESCE(d.clave, '')::text, d.fecha, COALESCE(d.dias, 0)::smallint, \
     COALESCE(d.lic, 0)::float8, COALESCE(d.inc, 0)::float8, COALESCE(d.aus, 0)::float8, \
     d.id_registro_obra, COALESCE(ro.numero, '')::text, COALESCE(d.dias_registro_obra, 0)::smallint, \
     ro.fecha_inicio, ro.fecha_fin \
     FROM public.reportes_liquidaciones rl \
     JOIN public.reportes_detalles_liquidaciones d ON d.id_reporte_liquidacion = rl.id \
     LEFT JOIN public.empleados e ON e.id = rl.id_empleado \
     LEFT JOIN public.registros_obras ro ON ro.id = d.id_registro_obra \
     WHERE rl.id IN (SELECT id_reporte_liquidacion FROM public.reportes_cambios \
                     WHERE lote_id = $1 AND id_reporte_liquidacion IS NOT NULL) \
     ORDER BY rl.id, d.fecha NULLS FIRST, d.id";

/// Días de obra "antes" del lote, sumados por (reporte, obra) desde la foto JSON.
const SQL_ASIGNADOS_ANTES: &str = "SELECT rc.id_reporte_liquidacion, (t->>'id_registro_obra')::uuid, \
     SUM(COALESCE((t->>'dias_registro_obra')::int, 0))::int \
     FROM public.reportes_cambios rc \
     CROSS JOIN LATERAL jsonb_array_elements(COALESCE(rc.antes -> 'tramos', '[]'::jsonb)) t \
     WHERE rc.lote_id = $1 AND rc.id_reporte_liquidacion IS NOT NULL AND t->>'id_registro_obra' IS NOT NULL \
     GROUP BY 1, 2";

/// Indica si una clave de tramo es una baja (mismo criterio que `es_baja_tramo` de la vista de asignación).
pub fn es_baja(clave: &str) -> bool {
    clave.trim().eq_ignore_ascii_case("baja")
}

/// Días efectivos canónicos: ignora bajas y toma el tramo no-baja con mayor `dias_cedula`
/// (en empate, el primero). Sin tramos no-baja devuelve 0.
pub fn efectivos_de(tramos: &[(i16, f64, f64, f64, bool)]) -> i32 {
    let mut mejor: Option<(i16, f64, f64, f64)> = None;
    for &(dias, lic, inc, aus, baja) in tramos {
        if baja {
            continue;
        }
        match mejor {
            Some((actual, ..)) if actual >= dias => {}
            _ => mejor = Some((dias, lic, inc, aus)),
        }
    }
    mejor.map_or(0, |(dias, lic, inc, aus)| dias_efectivos(i32::from(dias), lic, inc, aus))
}

/// Convierte (clave, fecha) de tramos en movimientos de baja / alta-reingreso, ordenados y sin duplicados.
pub fn movimientos_de(claves_fechas: &[(&str, Option<NaiveDate>)]) -> Vec<EmployeeMovement> {
    let mut movs = Vec::new();
    for (clave, fecha) in claves_fechas {
        let Some(fecha) = *fecha else {
            continue;
        };
        let s = clave.trim().to_lowercase();
        if s.contains("baja") {
            movs.push(EmployeeMovement { kind: MovementKind::Baja, fecha });
        } else if s.contains("alta") || s.contains("reing") {
            movs.push(EmployeeMovement { kind: MovementKind::Alta, fecha });
        }
    }
    movs.sort_by(|a, b| (a.fecha, a.kind).cmp(&(b.fecha, b.kind)));
    movs.dedup();
    movs
}

/// Días del mes en los que la obra puede contar para el empleado (vigencia).
/// None = sin restricción de obra ni movimientos (no aplica vigencia).
pub fn vigencia_en_mes(
    anio: i16,
    mes: i16,
    obra_inicio: Option<NaiveDate>,
    obra_fin: Option<NaiveDate>,
    movs: &[EmployeeMovement],
) -> Option<i32> {
    if !(1..=12).contains(&mes) {
        return None;
    }
    let anio = i32::from(anio);
    let mes = u32::try_from(mes).ok()?;
    let month_start = NaiveDate::from_ymd_opt(anio, mes, 1)?;
    let (ny, nm) = if mes == 12 { (anio + 1, 1) } else { (anio, mes + 1) };
    let month_end = NaiveDate::from_ymd_opt(ny, nm, 1)?.pred_opt()?;

    let intervalos = calculate_employee_active_intervals(month_start, month_end, movs);
    if intervalos.is_empty() {
        return Some(0);
    }

    let obra_start = obra_inicio.unwrap_or(month_start).max(month_start);
    let obra_end = obra_fin.unwrap_or(month_end).min(month_end);
    if obra_start > month_end || obra_end < month_start || obra_start > obra_end {
        return Some(0);
    }

    if obra_inicio.is_none() && obra_fin.is_none() && movs.is_empty() {
        return None;
    }

    let mut traslape = 0i32;
    for (act_ini, act_fin) in &intervalos {
        let s = (*act_ini).max(obra_start);
        let e = (*act_fin).min(obra_end);
        if s <= e {
            traslape += (e - s).num_days() as i32 + 1;
        }
    }
    Some(traslape)
}

/// Tope = min(efectivos, vigencia (si existe), efectivos - otras obras), nunca negativo.
pub fn tope_dias_obra(efectivos: i32, vigencia: Option<i32>, otras_obras: i32) -> TopeDetalle {
    let tope = efectivos
        .min(vigencia.unwrap_or(efectivos))
        .min(efectivos - otras_obras)
        .max(0);
    TopeDetalle { efectivos, vigencia, otras_obras, tope }
}

/// Detecta violaciones en la foto "después". Solo cuentan las que empeoran respecto de "antes".
/// Orden de salida: empleado, año, mes, número de obra.
pub fn violaciones(
    despues: &[TramoTope],
    antes: &HashMap<(Uuid, Option<Uuid>), i32>,
) -> Vec<ViolacionTope> {
    let mut por_reporte: BTreeMap<Uuid, Vec<&TramoTope>> = BTreeMap::new();
    for t in despues {
        por_reporte.entry(t.id_reporte).or_default().push(t);
    }

    let mut salida = Vec::new();
    for (id_reporte, tramos) in por_reporte {
        let primero = tramos[0];
        let (empleado, anio, mes) = (primero.empleado.clone(), primero.anio, primero.mes);

        let tuplas: Vec<(i16, f64, f64, f64, bool)> = tramos
            .iter()
            .map(|t| (t.dias_cedula, t.lic, t.inc, t.aus, es_baja(&t.clave)))
            .collect();
        let ef = efectivos_de(&tuplas);
        let claves: Vec<(&str, Option<NaiveDate>)> =
            tramos.iter().map(|t| (t.clave.as_str(), t.fecha)).collect();
        let movs = movimientos_de(&claves);

        // Por obra: (días asignados, número, inicio, fin).
        let mut por_obra: BTreeMap<Uuid, (i32, String, Option<NaiveDate>, Option<NaiveDate>)> =
            BTreeMap::new();
        for t in &tramos {
            let Some(id_obra) = t.id_obra else {
                continue;
            };
            let entrada = por_obra
                .entry(id_obra)
                .or_insert_with(|| (0, t.obra_numero.clone(), t.obra_inicio, t.obra_fin));
            entrada.0 += i32::from(t.dias_obra);
        }

        for (id_obra, (asignados, numero, inicio, fin)) in &por_obra {
            let vig = vigencia_en_mes(anio, mes, *inicio, *fin, &movs);
            let tope = tope_dias_obra(ef, vig, 0).tope;
            let previo = antes.get(&(id_reporte, Some(*id_obra))).copied().unwrap_or(0);
            if *asignados > tope && *asignados > previo {
                salida.push(ViolacionTope {
                    id_reporte,
                    empleado: empleado.clone(),
                    anio,
                    mes,
                    obra_numero: Some(numero.clone()),
                    asignados: *asignados,
                    tope,
                });
            }
        }

        let total: i32 = por_obra.values().map(|(d, ..)| *d).sum();
        let previo_total = antes.get(&(id_reporte, None)).copied().unwrap_or(0);
        if total > ef && total > previo_total {
            salida.push(ViolacionTope {
                id_reporte,
                empleado: empleado.clone(),
                anio,
                mes,
                obra_numero: None,
                asignados: total,
                tope: ef,
            });
        }
    }

    salida.sort_by(|a, b| {
        (&a.empleado, a.anio, a.mes, &a.obra_numero).cmp(&(&b.empleado, b.anio, b.mes, &b.obra_numero))
    });
    salida
}

/// Mensaje para el usuario: hasta 10 violaciones y " y N más" si hay más.
pub fn mensaje_violaciones(v: &[ViolacionTope]) -> String {
    let mut msg = String::from("No se guardó: días de obra por encima del tope. ");
    let partes: Vec<String> = v
        .iter()
        .take(10)
        .map(|x| match &x.obra_numero {
            Some(num) => format!(
                "{} {:02}/{}: obra {} {} d > tope {}",
                x.empleado, x.mes, x.anio, num, x.asignados, x.tope
            ),
            None => format!(
                "{} {:02}/{}: total {} d > {} días efectivos",
                x.empleado, x.mes, x.anio, x.asignados, x.tope
            ),
        })
        .collect();
    msg.push_str(&partes.join("; "));
    if v.len() > 10 {
        msg.push_str(&format!(" y {} más", v.len() - 10));
    }
    msg
}

/// Valida el lote recién editado contra el tope. Las acciones que solo restauran o quitan días
/// (deshacer, revertir, desactivar obra, desvincular) no pueden exceder el tope y se omiten.
pub fn validar_topes_lote<C: GenericClient>(c: &mut C, lote_id: Uuid) -> Result<(), String> {
    let accion: Option<String> = c
        .query_one(
            "SELECT MIN(accion) FROM public.reportes_cambios WHERE lote_id = $1",
            &[&lote_id],
        )
        .map_err(|e| format!("validar_topes (acción del lote): {}", format_pg_error(&e)))?
        .get(0);
    match accion.as_deref() {
        None => return Ok(()),
        Some(a) if [ACCION_DESHACER, ACCION_REVERTIR, ACCION_DESACTIVAR_OBRA, ACCION_DESVINCULAR].contains(&a) => {
            return Ok(());
        }
        Some(_) => {}
    }

    let filas = c
        .query(SQL_TRAMOS_LOTE, &[&lote_id])
        .map_err(|e| format!("validar_topes (tramos): {}", format_pg_error(&e)))?;
    let despues: Vec<TramoTope> = filas
        .iter()
        .map(|r| TramoTope {
            id_reporte: r.get(0),
            empleado: r.get(1),
            anio: r.get(2),
            mes: r.get(3),
            clave: r.get(4),
            fecha: r.get(5),
            dias_cedula: r.get(6),
            lic: r.get(7),
            inc: r.get(8),
            aus: r.get(9),
            id_obra: r.get(10),
            obra_numero: r.get(11),
            dias_obra: r.get(12),
            obra_inicio: r.get(13),
            obra_fin: r.get(14),
        })
        .collect();

    let filas_antes = c
        .query(SQL_ASIGNADOS_ANTES, &[&lote_id])
        .map_err(|e| format!("validar_topes (asignados antes): {}", format_pg_error(&e)))?;
    let mut antes: HashMap<(Uuid, Option<Uuid>), i32> = HashMap::new();
    for r in &filas_antes {
        let id_reporte: Uuid = r.get(0);
        let id_obra: Uuid = r.get(1);
        let dias: i32 = r.get(2);
        *antes.entry((id_reporte, Some(id_obra))).or_insert(0) += dias;
        *antes.entry((id_reporte, None)).or_insert(0) += dias;
    }

    let v = violaciones(&despues, &antes);
    if v.is_empty() {
        Ok(())
    } else {
        Err(mensaje_violaciones(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(anio: i32, mes: u32, dia: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(anio, mes, dia).expect("fecha de prueba")
    }

    fn tramo(id_reporte: Uuid, clave: &str, dias_cedula: i16, dias_obra: i16, id_obra: Option<Uuid>) -> TramoTope {
        TramoTope {
            id_reporte,
            empleado: "EMPLEADO PRUEBA".to_string(),
            anio: 2025,
            mes: 1,
            clave: clave.to_string(),
            fecha: Some(d(2025, 1, 15)),
            dias_cedula,
            lic: 0.0,
            inc: 0.0,
            aus: 0.0,
            id_obra,
            obra_numero: "C7000001".to_string(),
            dias_obra,
            obra_inicio: Some(d(2025, 1, 16)),
            obra_fin: Some(d(2025, 1, 31)),
        }
    }

    #[test]
    fn es_baja_ignora_mayusculas_y_espacios() {
        assert!(es_baja("  BAJA "));
        assert!(es_baja("baja"));
        assert!(!es_baja("bajas"));
        assert!(!es_baja("PRUEBA"));
    }

    #[test]
    fn efectivos_ignora_bajas_y_toma_el_maximo() {
        let tramos = [(10, 0.0, 0.0, 0.0, true), (30, 0.0, 0.0, 0.0, false), (15, 0.0, 0.0, 0.0, false)];
        assert_eq!(efectivos_de(&tramos), 30);
        assert_eq!(efectivos_de(&[(30, 0.0, 0.0, 0.0, true)]), 0);
        assert_eq!(efectivos_de(&[]), 0);
    }

    #[test]
    fn efectivos_redondea_incapacidad_fraccionaria() {
        // 30 - 2.4 = 27.6 -> 28
        assert_eq!(efectivos_de(&[(30, 0.0, 2.4, 0.0, false)]), 28);
    }

    #[test]
    fn efectivos_en_empate_toma_el_primero() {
        let tramos = [(30, 1.0, 0.0, 0.0, false), (30, 0.0, 0.0, 0.0, false)];
        assert_eq!(efectivos_de(&tramos), 29);
    }

    #[test]
    fn movimientos_ordenan_deduplican_y_omiten_sin_fecha() {
        let claves = [
            ("BAJA", Some(d(2025, 1, 10))),
            ("ALTA", Some(d(2025, 1, 2))),
            ("BAJA", Some(d(2025, 1, 10))),
            ("REINGRESO", Some(d(2025, 1, 20))),
            ("BAJA", None),
            ("PRUEBA", Some(d(2025, 1, 5))),
        ];
        let movs = movimientos_de(&claves);
        assert_eq!(
            movs,
            vec![
                EmployeeMovement { kind: MovementKind::Alta, fecha: d(2025, 1, 2) },
                EmployeeMovement { kind: MovementKind::Baja, fecha: d(2025, 1, 10) },
                EmployeeMovement { kind: MovementKind::Alta, fecha: d(2025, 1, 20) },
            ]
        );
    }

    #[test]
    fn vigencia_obra_que_inicia_dia_16_de_mes_de_31() {
        assert_eq!(vigencia_en_mes(2025, 1, Some(d(2025, 1, 16)), None, &[]), Some(16));
    }

    #[test]
    fn vigencia_baja_el_dia_10() {
        let movs = [EmployeeMovement { kind: MovementKind::Baja, fecha: d(2025, 1, 10) }];
        assert_eq!(vigencia_en_mes(2025, 1, None, None, &movs), Some(10));
    }

    #[test]
    fn vigencia_sin_obra_ni_movimientos_es_none() {
        assert_eq!(vigencia_en_mes(2025, 1, None, None, &[]), None);
    }

    #[test]
    fn vigencia_obra_fuera_del_mes_es_cero() {
        assert_eq!(vigencia_en_mes(2025, 1, Some(d(2025, 2, 1)), Some(d(2025, 2, 28)), &[]), Some(0));
    }

    #[test]
    fn vigencia_mes_invalido_es_none() {
        assert_eq!(vigencia_en_mes(2025, 13, None, None, &[]), None);
    }

    #[test]
    fn vigencia_diciembre_calcula_fin_de_mes() {
        assert_eq!(vigencia_en_mes(2025, 12, Some(d(2025, 12, 25)), None, &[]), Some(7));
    }

    #[test]
    fn tope_otras_obras_reduce_el_tope() {
        assert_eq!(tope_dias_obra(30, Some(16), 0).tope, 16);
        assert_eq!(tope_dias_obra(30, Some(16), 20).tope, 10);
        assert_eq!(tope_dias_obra(30, None, 0).tope, 30);
        assert_eq!(tope_dias_obra(10, None, 40).tope, 0);
    }

    #[test]
    fn violacion_cuando_excede_y_antes_es_menor() {
        let rep = Uuid::now_v7();
        let obra = Uuid::now_v7();
        let despues = [tramo(rep, "PRUEBA", 30, 20, Some(obra))];
        let antes = HashMap::from([((rep, Some(obra)), 0), ((rep, None), 0)]);
        let v = violaciones(&despues, &antes);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].obra_numero.as_deref(), Some("C7000001"));
        assert_eq!(v[0].asignados, 20);
        assert_eq!(v[0].tope, 16);
    }

    #[test]
    fn excede_pero_antes_igual_o_mayor_no_es_violacion() {
        let rep = Uuid::now_v7();
        let obra = Uuid::now_v7();
        let despues = [tramo(rep, "PRUEBA", 30, 20, Some(obra))];
        let igual = HashMap::from([((rep, Some(obra)), 20), ((rep, None), 20)]);
        assert!(violaciones(&despues, &igual).is_empty());
        let mayor = HashMap::from([((rep, Some(obra)), 25), ((rep, None), 25)]);
        assert!(violaciones(&despues, &mayor).is_empty());
    }

    #[test]
    fn total_del_mes_mayor_a_efectivos_es_violacion_sin_obra() {
        let rep = Uuid::now_v7();
        let obra_a = Uuid::now_v7();
        let obra_b = Uuid::now_v7();
        let mut a = tramo(rep, "PRUEBA", 30, 20, Some(obra_a));
        a.obra_inicio = None;
        a.obra_fin = None;
        let mut b = tramo(rep, "PRUEBA", 30, 15, Some(obra_b));
        b.obra_numero = "C7000002".to_string();
        b.obra_inicio = None;
        b.obra_fin = None;
        let antes = HashMap::new();
        let v = violaciones(&[a, b], &antes);
        let total: Vec<&ViolacionTope> = v.iter().filter(|x| x.obra_numero.is_none()).collect();
        assert_eq!(total.len(), 1);
        assert_eq!(total[0].asignados, 35);
        assert_eq!(total[0].tope, 30);
    }

    #[test]
    fn mensaje_lista_hasta_diez_y_luego_cuenta_restantes() {
        let v: Vec<ViolacionTope> = (0..12)
            .map(|i| ViolacionTope {
                id_reporte: Uuid::nil(),
                empleado: format!("EMP {i}"),
                anio: 2025,
                mes: 1,
                obra_numero: Some("C1".to_string()),
                asignados: 20,
                tope: 16,
            })
            .collect();
        let msg = mensaje_violaciones(&v);
        assert!(msg.starts_with("No se guardó: días de obra por encima del tope. "));
        assert!(msg.contains("EMP 9 01/2025: obra C1 20 d > tope 16"));
        assert!(!msg.contains("EMP 10"));
        assert!(msg.ends_with(" y 2 más"));
    }
}
