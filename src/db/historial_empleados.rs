//! Histórico empleado × obra por periodos (D-10 b).
//!
//! Cada tramo es (reporte mensual ACTIVO del empleado, obra activa). Las fechas efectivas de un
//! tramo son: vigencia de la obra ∩ mes ∩ periodos activos del empleado (según sus bajas/altas
//! de cédula). Los tramos contiguos de una misma obra se compactan en un periodo.

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate};
use postgres::GenericClient;
use uuid::Uuid;

use crate::db::reporte_liquidaciones::calculate_employee_active_intervals;
use crate::db::topes::movimientos_de;

/// Un mes de un empleado en una obra (una fila de reportes_liquidaciones × obra).
#[derive(Debug, Clone, PartialEq)]
pub struct TramoHistorial {
    pub id_revision: Uuid,
    pub revision: String,
    pub id_obra: Uuid,
    pub numero: String,
    pub obra_inicio: Option<NaiveDate>,
    pub obra_fin: Option<NaiveDate>,
    pub anio: i16,
    pub mes: i16,
    pub dias_obra: i32,
    pub estado: String,
    /// Movimientos (clave, fecha) de cédula del reporte completo al que pertenece el tramo.
    pub movimientos: Vec<(String, Option<NaiveDate>)>,
}

/// Periodo continuo de un empleado en una obra.
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodoObra {
    pub id_revision: Uuid,
    pub revision: String,
    pub numero: String,
    pub inicio: NaiveDate,
    pub fin: NaiveDate,
    pub dias_calendario: i32,
    pub dias_obra: i32,
}

/// Mes de la obra con su rango efectivo (None si el empleado no estuvo activo en la obra ese mes).
#[derive(Debug, Clone, PartialEq)]
pub struct MesObra {
    pub revision: String,
    pub anio: i16,
    pub mes: i16,
    pub numero: String,
    pub dias_obra: i32,
    pub rango: Option<(NaiveDate, NaiveDate)>,
    pub estado: String,
}

/// Tramos empleado × obra (solo reportes ACTIVO y obras activas), en orden cronológico.
/// `id_revision` = None devuelve todas las revisiones del empleado.
pub fn fetch_tramos_historial<C: GenericClient>(
    c: &mut C,
    id_empleado: Uuid,
    id_revision: Option<Uuid>,
) -> Result<Vec<TramoHistorial>, String> {
    let filas = c
        .query(
            "SELECT rl.id_revision,
                    COALESCE(rv.nombre, '')::text,
                    ro.id,
                    TRIM(ro.numero)::text,
                    ro.fecha_inicio,
                    ro.fecha_fin,
                    rl.anio_periodo,
                    rl.mes_periodo_int,
                    SUM(COALESCE(d.dias_registro_obra, 0))::int,
                    rl.estado::text,
                    rl.id
             FROM public.reportes_liquidaciones rl
             JOIN public.revisiones rv ON rv.id = rl.id_revision
             JOIN public.reportes_detalles_liquidaciones d ON d.id_reporte_liquidacion = rl.id
             JOIN public.registros_obras ro ON ro.id = d.id_registro_obra AND ro.activa
             WHERE rl.id_empleado = $1
               AND rl.estado = 'ACTIVO'
               AND rl.anio_periodo IS NOT NULL AND rl.mes_periodo_int IS NOT NULL
               AND ($2::uuid IS NULL OR rl.id_revision = $2)
             GROUP BY rl.id_revision, rv.nombre, ro.id, ro.numero, ro.fecha_inicio, ro.fecha_fin,
                      rl.anio_periodo, rl.mes_periodo_int, rl.estado, rl.id
             ORDER BY rl.anio_periodo, rl.mes_periodo_int, ro.numero",
            &[&id_empleado, &id_revision],
        )
        .map_err(|e| format!("Error consultando histórico de obras del empleado: {}", e))?;

    let mut ids_reporte: Vec<Uuid> = filas.iter().map(|r| r.get::<_, Uuid>(10)).collect();
    ids_reporte.sort();
    ids_reporte.dedup();

    let mut movs_por_reporte: HashMap<Uuid, Vec<(String, Option<NaiveDate>)>> = HashMap::new();
    if !ids_reporte.is_empty() {
        let movs = c
            .query(
                "SELECT id_reporte_liquidacion, COALESCE(TRIM(clave), '')::text, fecha
                 FROM public.reportes_detalles_liquidaciones
                 WHERE id_reporte_liquidacion = ANY($1)
                 ORDER BY fecha",
                &[&ids_reporte],
            )
            .map_err(|e| format!("Error consultando movimientos de cédula: {}", e))?;
        for m in movs {
            let id_rep: Uuid = m.get(0);
            let clave: String = m.get(1);
            let fecha: Option<NaiveDate> = m.get(2);
            movs_por_reporte.entry(id_rep).or_default().push((clave, fecha));
        }
    }

    let tramos = filas
        .iter()
        .map(|r| {
            let id_rep: Uuid = r.get(10);
            TramoHistorial {
                id_revision: r.get(0),
                revision: r.get(1),
                id_obra: r.get(2),
                numero: r.get(3),
                obra_inicio: r.get(4),
                obra_fin: r.get(5),
                anio: r.get(6),
                mes: r.get(7),
                dias_obra: r.get(8),
                estado: r.get(9),
                movimientos: movs_por_reporte.get(&id_rep).cloned().unwrap_or_default(),
            }
        })
        .collect();
    Ok(tramos)
}

fn fin_de_mes(inicio_mes: NaiveDate) -> Option<NaiveDate> {
    let (anio, mes) = if inicio_mes.month() == 12 {
        (inicio_mes.year() + 1, 1)
    } else {
        (inicio_mes.year(), inicio_mes.month() + 1)
    };
    NaiveDate::from_ymd_opt(anio, mes, 1)?.pred_opt()
}

/// Rango efectivo (primer día, último día) del tramo dentro de su mes.
///
/// Simplificación: si el empleado tiene varios intervalos activos en el mes, se toma desde el
/// inicio del primero hasta el fin del último (los huecos intermedios no se descuentan).
pub fn rango_efectivo(t: &TramoHistorial) -> Option<(NaiveDate, NaiveDate)> {
    let inicio_mes = NaiveDate::from_ymd_opt(
        i32::from(t.anio),
        u32::try_from(t.mes).ok()?,
        1,
    )?;
    let fin_mes = fin_de_mes(inicio_mes)?;

    let pares: Vec<(&str, Option<NaiveDate>)> =
        t.movimientos.iter().map(|(c, f)| (c.as_str(), *f)).collect();
    let movs = movimientos_de(&pares);
    let intervalos = calculate_employee_active_intervals(inicio_mes, fin_mes, &movs);

    let vig_ini = t.obra_inicio.unwrap_or(inicio_mes).max(inicio_mes);
    let vig_fin = t.obra_fin.unwrap_or(fin_mes).min(fin_mes);

    let efectivos: Vec<(NaiveDate, NaiveDate)> = intervalos
        .iter()
        .filter_map(|(a, b)| {
            let s = (*a).max(vig_ini);
            let e = (*b).min(vig_fin);
            (s <= e).then_some((s, e))
        })
        .collect();

    let primero = efectivos.first()?.0;
    let ultimo = efectivos.last()?.1;
    Some((primero, ultimo))
}

/// Un registro por tramo, con su rango efectivo.
pub fn meses_obra(tramos: &[TramoHistorial]) -> Vec<MesObra> {
    tramos
        .iter()
        .map(|t| MesObra {
            revision: t.revision.clone(),
            anio: t.anio,
            mes: t.mes,
            numero: t.numero.clone(),
            dias_obra: t.dias_obra,
            rango: rango_efectivo(t),
            estado: t.estado.clone(),
        })
        .collect()
}

/// Agrupa por (revisión, obra) y une meses contiguos (inicio == fin anterior + 1 día).
/// Un mes sin días en la obra o sin rango efectivo rompe la continuidad.
pub fn compactar_periodos(tramos: &[TramoHistorial]) -> Vec<PeriodoObra> {
    let mut orden: Vec<&TramoHistorial> = tramos.iter().collect();
    orden.sort_by(|a, b| {
        (a.anio, a.mes)
            .cmp(&(b.anio, b.mes))
            .then_with(|| a.numero.cmp(&b.numero))
    });

    let mut periodos: Vec<PeriodoObra> = Vec::new();
    let mut abiertos: HashMap<(Uuid, String), usize> = HashMap::new();

    for t in orden {
        let clave = (t.id_revision, t.numero.clone());
        let rango = rango_efectivo(t).filter(|_| t.dias_obra > 0);
        let Some((ini, fin)) = rango else {
            abiertos.remove(&clave);
            continue;
        };

        if let Some(&idx) = abiertos.get(&clave) {
            let p = &mut periodos[idx];
            if p.fin.succ_opt() == Some(ini) {
                p.fin = fin;
                p.dias_obra += t.dias_obra;
                p.dias_calendario = dias_calendario(p.inicio, p.fin);
                continue;
            }
        }

        periodos.push(PeriodoObra {
            id_revision: t.id_revision,
            revision: t.revision.clone(),
            numero: t.numero.clone(),
            inicio: ini,
            fin,
            dias_calendario: dias_calendario(ini, fin),
            dias_obra: t.dias_obra,
        });
        abiertos.insert(clave, periodos.len() - 1);
    }

    periodos
}

fn dias_calendario(inicio: NaiveDate, fin: NaiveDate) -> i32 {
    (fin - inicio).num_days() as i32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(anio: i32, mes: u32, dia: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(anio, mes, dia).expect("fecha de prueba")
    }

    fn revision() -> Uuid {
        Uuid::from_u128(1)
    }

    fn tramo(
        mes: i16,
        numero: &str,
        obra: (NaiveDate, NaiveDate),
        dias: i32,
        movs: Vec<(&str, Option<NaiveDate>)>,
    ) -> TramoHistorial {
        TramoHistorial {
            id_revision: revision(),
            revision: "REV".to_string(),
            id_obra: Uuid::from_u128(100 + mes as u128),
            numero: numero.to_string(),
            obra_inicio: Some(obra.0),
            obra_fin: Some(obra.1),
            anio: 2025,
            mes,
            dias_obra: dias,
            estado: "ACTIVO".to_string(),
            movimientos: movs
                .into_iter()
                .map(|(c, f)| (c.to_string(), f))
                .collect(),
        }
    }

    fn todo_el_anio() -> (NaiveDate, NaiveDate) {
        (d(2025, 1, 1), d(2025, 12, 31))
    }

    #[test]
    fn obra_todo_el_anio_es_un_periodo() {
        let tramos: Vec<TramoHistorial> = (1..=12)
            .map(|m| tramo(m, "OBRA-A", todo_el_anio(), 30, vec![]))
            .collect();
        let periodos = compactar_periodos(&tramos);
        assert_eq!(periodos.len(), 1);
        assert_eq!(periodos[0].inicio, d(2025, 1, 1));
        assert_eq!(periodos[0].fin, d(2025, 12, 31));
        assert_eq!(periodos[0].dias_calendario, 365);
        assert_eq!(periodos[0].dias_obra, 360);
    }

    #[test]
    fn obra_que_inicia_16_de_enero_sigue_en_febrero() {
        let obra = (d(2025, 1, 16), d(2025, 12, 31));
        let tramos = vec![
            tramo(1, "OBRA-A", obra, 15, vec![]),
            tramo(2, "OBRA-A", obra, 15, vec![]),
        ];
        let meses = meses_obra(&tramos);
        assert_eq!(meses[0].rango, Some((d(2025, 1, 16), d(2025, 1, 31))));
        let periodos = compactar_periodos(&tramos);
        assert_eq!(periodos.len(), 1);
        assert_eq!(periodos[0].inicio, d(2025, 1, 16));
        assert_eq!(periodos[0].fin, d(2025, 2, 28));
    }

    #[test]
    fn baja_el_20_de_marzo_termina_el_periodo() {
        let baja = Some(d(2025, 3, 20));
        let movs = vec![("BAJA", baja)];
        let tramos = vec![
            tramo(1, "OBRA-A", todo_el_anio(), 30, movs.clone()),
            tramo(2, "OBRA-A", todo_el_anio(), 28, movs.clone()),
            tramo(3, "OBRA-A", todo_el_anio(), 20, movs.clone()),
        ];
        let periodos = compactar_periodos(&tramos);
        assert_eq!(periodos.len(), 1);
        assert_eq!(periodos[0].inicio, d(2025, 1, 1));
        assert_eq!(periodos[0].fin, d(2025, 3, 20));
    }

    #[test]
    fn hueco_de_un_mes_produce_dos_periodos() {
        let tramos = vec![
            tramo(1, "OBRA-A", todo_el_anio(), 30, vec![]),
            tramo(3, "OBRA-A", todo_el_anio(), 30, vec![]),
        ];
        let periodos = compactar_periodos(&tramos);
        assert_eq!(periodos.len(), 2);
        assert_eq!(periodos[0].fin, d(2025, 1, 31));
        assert_eq!(periodos[1].inicio, d(2025, 3, 1));
    }

    #[test]
    fn dos_obras_en_el_mismo_mes_son_dos_periodos() {
        let tramos = vec![
            tramo(1, "OBRA-A", todo_el_anio(), 15, vec![]),
            tramo(1, "OBRA-B", todo_el_anio(), 15, vec![]),
        ];
        let periodos = compactar_periodos(&tramos);
        assert_eq!(periodos.len(), 2);
        assert_ne!(periodos[0].numero, periodos[1].numero);
    }
}
