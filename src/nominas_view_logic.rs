use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{Datelike, NaiveDate};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use uuid::Uuid;

use crate::db::nominas_reporte::{
    check_empleado_tiene_baja_periodo, create_bulk_missing_nominas,
    create_nomina_from_template, fetch_conceptos_nomina_detalle,
    fetch_nominas_faltantes, fetch_nominas_reporte, fetch_percepcion_conceptos,
    fetch_percepciones_nomina, fetch_percepciones_por_nomina,
    fetch_totales_dias_cedula_por_registro_patronal, summarize_nominas,
    update_dias_y_conceptos_nomina, NominaConceptoEditableDb, NominaFaltanteCreateRequest,
    NominaFaltanteDb, NominaReporteRowDb, RegistroPatronalTotalDb,
};
use crate::db::DbPool;
use crate::{
    AppData, AppLogic, MainWindow, NominaEditableConceptoItem, NominaEmpleadoDiaItem,
    NominaFaltanteItem, NominaMesTotalItem, NominaPercepcionItem,
    NominaRegistroPatronalTotalItem, NominaRow, NominaTemplateOptionItem,
};

/// Estado de la sesión del reporte de nóminas.
pub struct NominasSessionState {
    pub all_rows: Vec<NominaReporteRowDb>,
    pub filtered_rows: Vec<NominaReporteRowDb>,
    /// Días por mes de cada trabajador, indexados por `id_empleado`.
    pub dias_por_empleado: HashMap<Uuid, Vec<(String, i32, f64, f64)>>,
    pub periodo_options: Vec<String>,
    pub anio_options: Vec<String>,
    pub current_page: usize,
    pub page_size: usize,
    pub search_query: String,
    pub filter_periodo_idx: usize,
    pub filter_anio_idx: usize,
    /// 0 = todos, 1 = coinciden, 2 = difieren, 3 = sin cédula
    pub filter_dias_idx: usize,
    pub only_sin_cedula: bool,
    pub mostrar_detalle: bool,
    /// Encabezados de las columnas dinámicas de percepciones.
    pub percepcion_headers: Vec<String>,
    /// Montos por percepción de cada nómina, alineados con
    /// `percepcion_headers` e indexados por `id_nomina`.
    pub percepciones_por_nomina: HashMap<Uuid, Vec<f64>>,

    /// Totales de días cédula por registro patronal y obra del usuario
    pub totales_por_rp: Vec<RegistroPatronalTotalDb>,

    /// Estado del modal de edición
    pub edit_id_nomina: Option<Uuid>,
    pub edit_empleado: String,
    pub edit_periodo: String,
    pub edit_dias_original: f64,
    pub edit_dias_input: String,
    pub edit_conceptos: Vec<NominaConceptoEditableDb>,
    pub edit_is_baja: bool,
    pub edit_baja_warning: String,

    /// Estado del modal de copia/plantilla
    pub copy_id_empleado: Option<Uuid>,
    pub copy_empleado: String,
    pub copy_templates: Vec<NominaReporteRowDb>,
    pub copy_selected_template_idx: usize,
    pub copy_target_mes_idx: usize,
    pub copy_target_anio: String,
    pub copy_dias_original: f64,
    pub copy_dias_input: String,
    pub copy_conceptos: Vec<NominaConceptoEditableDb>,

    /// Estado del detector de nóminas faltantes
    pub faltantes: Vec<(NominaFaltanteDb, bool)>,
    pub faltantes_search: String,
    pub faltantes_status_msg: String,
}

impl Default for NominasSessionState {
    fn default() -> Self {
        Self {
            all_rows: Vec::new(),
            filtered_rows: Vec::new(),
            dias_por_empleado: HashMap::new(),
            periodo_options: Vec::new(),
            anio_options: Vec::new(),
            current_page: 0,
            page_size: 100,
            search_query: String::new(),
            filter_periodo_idx: 0,
            filter_anio_idx: 0,
            filter_dias_idx: 0,
            only_sin_cedula: false,
            mostrar_detalle: true,
            percepcion_headers: Vec::new(),
            percepciones_por_nomina: HashMap::new(),
            totales_por_rp: Vec::new(),
            edit_id_nomina: None,
            edit_empleado: String::new(),
            edit_periodo: String::new(),
            edit_dias_original: 0.0,
            edit_dias_input: String::new(),
            edit_conceptos: Vec::new(),
            edit_is_baja: false,
            edit_baja_warning: String::new(),
            copy_id_empleado: None,
            copy_empleado: String::new(),
            copy_templates: Vec::new(),
            copy_selected_template_idx: 0,
            copy_target_mes_idx: 0,
            copy_target_anio: "2026".to_string(),
            copy_dias_original: 0.0,
            copy_dias_input: String::new(),
            copy_conceptos: Vec::new(),
            faltantes: Vec::new(),
            faltantes_search: String::new(),
            faltantes_status_msg: String::new(),
        }
    }
}

/// Formatea días: entero cuando no hay decimales, con un decimal si los hay.
///
/// `num_dias_pagados` es `double precision` y admite 15.5, así que redondear a
/// entero ocultaría diferencias reales contra la cédula.
pub fn fmt_dias(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 {
        format!("{}", v.round() as i64)
    } else {
        format!("{:.1}", v)
    }
}

/// Formatea un importe monetario reutilizando el formateador del reporte de
/// liquidaciones, para que ambas vistas muestren los importes igual.
pub fn fmt_money(v: f64) -> String {
    crate::reporte_view_logic::format_curr(v)
}

/// Formatea una fecha como `DD/MM/AAAA`, o `-` si falta.
pub fn fmt_fecha(d: Option<NaiveDate>) -> String {
    match d {
        Some(f) => format!("{:02}/{:02}/{}", f.day(), f.month(), f.year()),
        None => "-".to_string(),
    }
}

/// Etiqueta de semana fiscal (`S18`), o `-` si no hay fecha de periodo.
pub fn fmt_semana(semana: i32) -> String {
    if semana > 0 {
        format!("S{}", semana)
    } else {
        "-".to_string()
    }
}

/// Etiqueta RFC/CURP uniendo solo lo que existe.
pub fn build_rfc_curp(rfc: &str, curp: &str) -> String {
    let rfc = rfc.trim();
    let curp = curp.trim();
    let mut parts: Vec<&str> = Vec::new();
    if !rfc.is_empty() {
        parts.push(rfc);
    }
    if !curp.is_empty() && !curp.eq_ignore_ascii_case(rfc) {
        parts.push(curp);
    }
    if parts.is_empty() {
        "SIN RFC/CURP".to_string()
    } else {
        parts.join("\n")
    }
}

/// Acumula los días por mes de cada trabajador a partir de todas las nóminas.
///
/// Es el "total de días por empleado" que pide la vista: se calcula sobre el
/// conjunto completo, no sobre las filas filtradas, para que el número no cambie
/// al aplicar un filtro y sirva como referencia estable de comparación.
fn build_dias_por_empleado(rows: &[NominaReporteRowDb]) -> HashMap<Uuid, Vec<(String, i32, f64, f64)>> {
    let mut map: HashMap<Uuid, Vec<(String, i32, f64, f64)>> = HashMap::new();
    for r in rows {
        let periodo = format!("{} / {}", r.mes_periodo, r.anio_periodo);
        let entry = map.entry(r.id_empleado).or_default();
        match entry.iter_mut().find(|(p, _, _, _)| *p == periodo) {
            Some(slot) => {
                slot.2 += r.num_dias_pagados;
                slot.3 += r.dias_cedula;
            }
            None => entry.push((periodo, r.anio_periodo, r.num_dias_pagados, r.dias_cedula)),
        }
    }
    // De más antiguo a más reciente, para que el desglose se lea como línea de
    // tiempo. El año desempata; dentro del mismo año el nombre del mes ordena.
    for v in map.values_mut() {
        v.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    }
    map
}

/// Totales de días por mes para la fila de resumen del pie de la tabla.
fn build_mes_totals(rows: &[NominaReporteRowDb]) -> Vec<NominaMesTotalItem> {
    let mut acc: HashMap<String, (i32, f64, f64, f64)> = HashMap::new();
    for r in rows {
        let key = format!("{:04}|{}", r.anio_periodo, r.mes_periodo);
        let e = acc.entry(key).or_insert((0, 0.0, 0.0, 0.0));
        e.0 += 1;
        e.1 += r.num_dias_pagados;
        e.2 += r.dias_cedula;
        e.3 += r.total_percepciones;
    }
    let mut v: Vec<(String, (i32, f64, f64, f64))> = acc.into_iter().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.into_iter()
        .map(|(key, e)| {
            let mut parts = key.split('|');
            let anio: i32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
            let mes = parts.next().unwrap_or("").to_string();
            NominaMesTotalItem {
                periodo: format!("{} / {}", mes, anio).into(),
                nominas: e.0,
                dias_pagados_str: fmt_dias(e.1).into(),
                dias_cedula_str: fmt_dias(e.2).into(),
                percepciones_str: fmt_money(e.3).into(),
            }
        })
        .collect()
}

/// Resumen corto de las percepciones de una nómina para la celda de la fila.
fn percepcion_resumen(row: &NominaReporteRowDb) -> String {
    if row.num_percepciones <= 0 {
        return "-".to_string();
    }
    format!("{} concepto(s)", row.num_percepciones)
}

/// Aplica búsqueda, filtros y ordenamiento; deja `filtered_rows` listo.
pub fn apply_nominas_filtering(state: &mut NominasSessionState) {
    let q = state.search_query.trim().to_lowercase();

// Índice 0 = "Todos" en ambos desplegables; el índice 0 no filtra.
let periodo_sel = if state.filter_periodo_idx > 0 {
    state.periodo_options.get(state.filter_periodo_idx).cloned()
} else {
    None
};
let anio_sel = if state.filter_anio_idx > 0 {
    state.anio_options.get(state.filter_anio_idx).cloned()
} else {
    None
};

    state.filtered_rows = state
        .all_rows
        .iter()
        .filter(|r| {
            if !q.is_empty() {
                let hay = r.nombre_trabajador.to_lowercase().contains(&q)
                    || r.nss.to_lowercase().contains(&q)
                    || r.rfc.to_lowercase().contains(&q)
                    || r.curp.to_lowercase().contains(&q)
                    || r.registro_patronal.to_lowercase().contains(&q)
                    || r.periodo_str.to_lowercase().contains(&q);
                if !hay {
                    return false;
                }
            }
            if let Some(p) = &periodo_sel {
                if &r.periodo_str != p {
                    return false;
                }
            }
            if let Some(a) = &anio_sel {
                if r.anio_periodo.to_string() != *a {
                    return false;
                }
            }
            match state.filter_dias_idx {
                1 => {
                    // Coinciden: hay cédula y los días cuadran.
                    if r.dias_cedula <= 0.0 || (r.num_dias_pagados - r.dias_cedula).abs() > 0.01 {
                        return false;
                    }
                }
                2 => {
                    // Difieren: hay cédula pero los días NO cuadran. Las filas sin
                    // cédula no son comparables, así que quedan fuera; tienen su
                    // propio filtro ("Sin cédula") para no mezclarse con una
                    // diferencia real.
                    if r.dias_cedula <= 0.0 || (r.num_dias_pagados - r.dias_cedula).abs() <= 0.01 {
                        return false;
                    }
                }
                3 => {
                    if r.dias_cedula > 0.0 {
                        return false;
                    }
                }
                _ => {}
            }
            if state.only_sin_cedula && r.dias_cedula > 0.0 {
                return false;
            }
            true
        })
        .cloned()
        .collect();

    let total_pages = state
        .filtered_rows
        .len()
        .div_ceil(state.page_size.max(1))
        .max(1);
    if state.current_page >= total_pages {
        state.current_page = total_pages.saturating_sub(1);
    }
}

/// Traduce el estado de sesión a los modelos de Slint y los publica en AppData.
pub fn render_nominas_ui(ui: &MainWindow, state: &NominasSessionState) {
    let d = ui.global::<AppData>();

    let total_pages = state
        .filtered_rows
        .len()
        .div_ceil(state.page_size.max(1))
        .max(1);
    let start_idx = state.current_page * state.page_size;
    let end_idx = (start_idx + state.page_size).min(state.filtered_rows.len());
    let slice = if start_idx < end_idx {
        &state.filtered_rows[start_idx..end_idx]
    } else {
        &[][..]
    };

    let mut rows = Vec::with_capacity(slice.len());
    for (i, r) in slice.iter().enumerate() {
        let idx = start_idx + i + 1;
        let alt = idx % 2 == 0;

        let delta = r.num_dias_pagados - r.dias_cedula;
        let coinciden = r.dias_cedula > 0.0 && delta.abs() <= 0.01;
        let sin_cedula = r.dias_cedula <= 0.0;

        // Desglose de meses del trabajador (el "total de días por mes").
        let mut meses: Vec<NominaEmpleadoDiaItem> = Vec::new();
        let mut tot_nom = 0.0;
        let mut tot_ced = 0.0;
        if state.mostrar_detalle {
            if let Some(v) = state.dias_por_empleado.get(&r.id_empleado) {
                for (periodo, _anio, dn, dc) in v {
                    let dlt = dn - dc;
                    meses.push(NominaEmpleadoDiaItem {
                        periodo: periodo.clone().into(),
                        anio: *_anio,
                        dias_nomina: fmt_dias(*dn).into(),
                        dias_cedula: fmt_dias(*dc).into(),
                        delta_str: if *dc > 0.0 && dlt.abs() <= 0.01 {
                            "=".to_string().into()
                        } else {
                            fmt_dias(dlt).into()
                        },
                        coincide: *dc > 0.0 && dlt.abs() <= 0.01,
                    });
                    tot_nom += dn;
                    tot_ced += dc;
                }
            }
        }

        // Montos por concepto de percepción, alineados con los
        // encabezados dinámicos. "-" cuando no hay monto.
        let percepciones_cols: Vec<SharedString> = state
            .percepciones_por_nomina
            .get(&r.id_nomina)
            .map(|v| {
                v.iter()
                    .map(|m| {
                        if m.abs() < 0.005 {
                            "-".to_string().into()
                        } else {
                            fmt_money(*m).into()
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec!["-".into(); state.percepcion_headers.len()]);

        rows.push(NominaRow {
            idx: idx as i32,
            id_nomina: r.id_nomina.to_string().into(),
            id_empleado: r.id_empleado.to_string().into(),
            nombre_trabajador: r.nombre_trabajador.to_uppercase().into(),
            nss: r.nss.clone().into(),
            rfc_curp: build_rfc_curp(&r.rfc, &r.curp).into(),
            registro_patronal: r.registro_patronal.clone().into(),
            periodo_str: r.periodo_str.clone().into(),
            semana_fiscal: r.semana_fiscal,
            semana_fiscal_str: fmt_semana(r.semana_fiscal).into(),
            fecha_inicial_pago_str: fmt_fecha(r.fecha_inicial_pago).into(),
            fecha_final_pago_str: fmt_fecha(r.fecha_final_pago).into(),
            fecha_pago_str: fmt_fecha(r.fecha_pago).into(),
            fecha_inicio_rel_str: fmt_fecha(r.fecha_inicio_relacion_laboral).into(),
            fecha_emision_str: fmt_fecha(r.fecha_emision_cfdi).into(),
            fecha_timbrado_str: fmt_fecha(r.fecha_timbrado).into(),
            num_dias_pagados_str: fmt_dias(r.num_dias_pagados).into(),
            dias_cedula_str: if sin_cedula {
                "-".into()
            } else {
                fmt_dias(r.dias_cedula).into()
            },
            dias_delta_str: if sin_cedula {
                "-".into()
            } else if coinciden {
                "OK".to_string().into()
            } else {
                fmt_dias(delta).into()
            },
            dias_coinciden: coinciden,
            sin_cedula,
            total_percepciones_str: fmt_money(r.total_percepciones).into(),
            total_gravado_str: fmt_money(r.percepciones_gravadas).into(),
            total_exento_str: fmt_money(r.percepciones_exentas).into(),
            num_percepciones: r.num_percepciones,
            percepcion_detalle_str: percepcion_resumen(r).into(),
            salario_diario_str: fmt_money(r.salario_diario_integrado).into(),
            tipo_nomina: r.tipo_nomina.clone().into(),
            periodicidad: r.periodicidad.clone().into(),
            total_dias_empleado: fmt_dias(tot_nom).into(),
            total_dias_meses: meses.len() as i32,
            meses: ModelRc::new(VecModel::from(meses)),
            percepciones_cols: ModelRc::new(VecModel::from(percepciones_cols)),
            row_bg_color: if alt {
                slint::Color::from_rgb_u8(0xfb, 0xfc, 0xfe)
            } else {
                slint::Color::from_rgb_u8(0xff, 0xff, 0xff)
            },
            row_hover_color: slint::Color::from_rgb_u8(0xf1, 0xf5, 0xf9),
        });
    }

    d.set_nominas_list(ModelRc::new(VecModel::from(rows)));

    // Encabezados dinámicos de percepciones y ancho mínimo de
    // tabla para que el scroll horizontal aparezca solo cuando
    // las columnas no caben en el área visible.
    d.set_nominas_percepcion_headers(ModelRc::new(VecModel::from(
        state
            .percepcion_headers
            .iter()
            .map(|h| h.as_str().into())
            .collect::<Vec<SharedString>>(),
    )));
    // 1630px de columnas fijas + 110px por columna de percepción.
    let min_w = 1630.0 + (state.percepcion_headers.len() as f32) * 110.0;
    d.set_nominas_table_min_width(min_w);

    // Totales por registro patronal de obras creadas por el usuario
    let rp_items: Vec<NominaRegistroPatronalTotalItem> = state
        .totales_por_rp
        .iter()
        .map(|t| NominaRegistroPatronalTotalItem {
            registro_patronal: t.registro_patronal.clone().into(),
            obra_numero: t.obra_numero.clone().into(),
            tipo_obra: t.tipo_obra.clone().into(),
            total_dias_cedula_str: fmt_dias(t.total_dias_cedula).into(),
        })
        .collect();
    d.set_nominas_totales_por_registro_patronal(ModelRc::new(VecModel::from(rp_items)));
    d.set_nominas_faltantes_count(state.faltantes.len() as i32);

    // Resumen y KPIs sobre el conjunto filtrado.
    let s = summarize_nominas(&state.filtered_rows);
    d.set_nominas_kpi_total_str(format!("{}", s.total_nominas).into());
    d.set_nominas_kpi_empleados_str(format!("{}", s.total_empleados).into());
    d.set_nominas_kpi_dias_pagados_str(fmt_dias(s.dias_pagados).into());
    d.set_nominas_kpi_dias_cedula_str(fmt_dias(s.dias_cedula).into());
    let delta = s.dias_pagados - s.dias_cedula;
    d.set_nominas_kpi_dias_delta_str(
        if s.dias_cedula > 0.0 && delta.abs() <= 0.01 {
            "OK".to_string().into()
        } else {
            fmt_dias(delta).into()
        },
    );
    d.set_nominas_kpi_percepciones_str(fmt_money(s.total_percepciones).into());
    d.set_nominas_kpi_sin_cedula_str(format!("{}", s.nominas_sin_cedula).into());
    d.set_nominas_subtitle(
        format!(
            "{} nómina(s) · {} trabajador(es)",
            s.total_nominas, s.total_empleados
        )
        .into(),
    );

    let totales = build_mes_totals(&state.filtered_rows);
    d.set_nominas_totales_por_mes(ModelRc::new(VecModel::from(totales)));

    d.set_nominas_periodo_options(ModelRc::new(VecModel::from(
        state
            .periodo_options
            .iter()
            .map(|x| x.as_str().into())
            .collect::<Vec<SharedString>>(),
    )));
    d.set_nominas_anio_options(ModelRc::new(VecModel::from(
        state
            .anio_options
            .iter()
            .map(|x| x.as_str().into())
            .collect::<Vec<SharedString>>(),
    )));

    d.set_nominas_filter_periodo_idx(state.filter_periodo_idx as i32);
    d.set_nominas_filter_anio_idx(state.filter_anio_idx as i32);
    d.set_nominas_filter_dias_idx(state.filter_dias_idx as i32);
    d.set_nominas_only_sin_cedula(state.only_sin_cedula);
    d.set_nominas_mostrar_detalle(state.mostrar_detalle);

    d.set_nominas_page_current((state.current_page + 1) as i32);
    d.set_nominas_page_total(total_pages as i32);
    d.set_nominas_page_start_row(if state.filtered_rows.is_empty() { 0 } else { start_idx + 1 } as i32);
    d.set_nominas_page_end_row(end_idx as i32);
    d.set_nominas_total_filtered(state.filtered_rows.len() as i32);
}

/// Recalcula proporcionalmente los conceptos según la fórmula:
/// Concepto Percepcion / Valor de dia original * dias que el usuario modifico
pub fn recalcular_conceptos_proporcional(
    conceptos: &mut [NominaConceptoEditableDb],
    dias_original: f64,
    dias_nuevos: f64,
) {
    if dias_original <= 0.0 || dias_nuevos < 0.0 {
        return;
    }
    let factor = dias_nuevos / dias_original;
    for c in conceptos.iter_mut() {
        c.importe = ((c.importe_original * factor) * 100.0).round() / 100.0;
    }
}

pub fn total_conceptos(conceptos: &[NominaConceptoEditableDb]) -> f64 {
    conceptos.iter().map(|c| c.importe).sum()
}

fn map_conceptos_to_slint(conceptos: &[NominaConceptoEditableDb]) -> Vec<NominaEditableConceptoItem> {
    conceptos
        .iter()
        .map(|c| NominaEditableConceptoItem {
            id_detalle: c.id_detalle.to_string().into(),
            id_concepto: c.id_concepto.to_string().into(),
            clave: c.clave.clone().into(),
            concepto: c.concepto.clone().into(),
            importe_original: c.importe_original as f32,
            importe_original_str: fmt_money(c.importe_original).into(),
            importe_actual_str: fmt_money(c.importe).into(),
        })
        .collect()
}

fn render_faltantes_ui(d: &AppData, state: &NominasSessionState) {
    let q = state.faltantes_search.trim().to_lowercase();
    let items: Vec<NominaFaltanteItem> = state
        .faltantes
        .iter()
        .filter(|(f, _)| {
            if q.is_empty() {
                true
            } else {
                f.nombre_trabajador.to_lowercase().contains(&q)
                    || f.nss.to_lowercase().contains(&q)
                    || f.mes_periodo.to_lowercase().contains(&q)
                    || f.clave.to_lowercase().contains(&q)
            }
        })
        .map(|(f, sel)| NominaFaltanteItem {
            id_empleado: f.id_empleado.to_string().into(),
            nombre_trabajador: f.nombre_trabajador.clone().into(),
            nss: f.nss.clone().into(),
            rfc: f.rfc.clone().into(),
            mes_periodo: f.mes_periodo.clone().into(),
            anio_periodo: f.anio_periodo,
            periodo_str: format!("{} / {}", f.mes_periodo, f.anio_periodo).into(),
            dias_cedula_str: fmt_dias(f.dias_cedula).into(),
            dias_nomina_str: fmt_dias(f.dias_nomina_actuales).into(),
            dias_faltantes: f.dias_faltantes as f32,
            dias_faltantes_str: fmt_dias(f.dias_faltantes).into(),
            clave: f.clave.clone().into(),
            is_baja: f.is_baja,
            tiene_plantilla: f.id_template_sugerido.is_some(),
            template_label: f.template_label.clone().into(),
            id_template: f.id_template_sugerido.map(|id| id.to_string()).unwrap_or_default().into(),
            template_dias_str: fmt_dias(f.template_dias).into(),
            selected: *sel,
        })
        .collect();

    let selected_count = state.faltantes.iter().filter(|(_, s)| *s).count();
    let all_selected = !state.faltantes.is_empty() && selected_count == state.faltantes.len();

    d.set_nominas_faltantes_list(ModelRc::new(VecModel::from(items)));
    d.set_nominas_faltantes_count(state.faltantes.len() as i32);
    d.set_nominas_faltantes_selected_count(selected_count as i32);
    d.set_nominas_faltantes_all_selected(all_selected);
    d.set_nominas_faltantes_status_msg(state.faltantes_status_msg.clone().into());
}

/// Carga las nóminas desde la BD y refresca la vista.
pub fn sync_and_load_nominas_data(
    ui: &MainWindow,
    pool: &DbPool,
    session: &Arc<Mutex<NominasSessionState>>,
    cliente_id: Option<Uuid>,
) -> Result<(), String> {
    let mut client = pool
        .get()
        .map_err(|e| format!("Error de conexión a la base de datos: {}", e))?;

    let count_rows: i64 = client
        .query_one("SELECT COUNT(*) FROM public.reportes_nominas", &[])
        .map(|r| r.get(0))
        .unwrap_or(0);
    if count_rows == 0 {
        let _ = crate::db::nominas_reporte::copy_nominas_a_reportes(&mut client);
    }
    let rows = fetch_nominas_reporte(&mut client, cliente_id)?;
    let totales_rp = fetch_totales_dias_cedula_por_registro_patronal(&mut client, cliente_id)
        .unwrap_or_default();
    let faltantes_db = fetch_nominas_faltantes(&mut client, cliente_id)
        .unwrap_or_default();

    let mut st = session
        .lock()
        .map_err(|_| "No se pudo bloquear el estado de nóminas".to_string())?;

    st.all_rows = rows;
    st.totales_por_rp = totales_rp;
    st.faltantes = faltantes_db.into_iter().map(|f| (f, false)).collect();
    st.dias_por_empleado = build_dias_por_empleado(&st.all_rows);

    // Columnas dinámicas de percepciones: encabezados y montos
    // por concepto de cada nómina.
    st.percepcion_headers = fetch_percepcion_conceptos(&mut client)
        .unwrap_or_default();
    let matrix = fetch_percepciones_por_nomina(&mut client)
        .unwrap_or_default();
    let headers = &st.percepcion_headers;
    st.percepciones_por_nomina = matrix
        .into_iter()
        .map(|(id_nom, por_concepto)| {
            let alineados = headers
                .iter()
                .map(|h| por_concepto.get(h).copied().unwrap_or(0.0))
                .collect::<Vec<f64>>();
            (id_nom, alineados)
        })
        .collect();

    // Opciones de filtro, ordenadas de más reciente a más antigua.
    // El índice 0 es "Todos" para no filtrar por defecto.
    let mut periodos: Vec<String> = st.all_rows.iter().map(|r| r.periodo_str.clone()).collect();
    periodos.sort();
    periodos.dedup();
    let total_periodos = periodos.len();
    periodos.reverse();
    periodos.insert(0, format!("Todos los periodos ({})", total_periodos));
    st.periodo_options = periodos;

    let mut anios: Vec<String> = st
        .all_rows
        .iter()
        .map(|r| r.anio_periodo.to_string())
        .collect();
    anios.sort();
    anios.dedup();
    let total_anios = anios.len();
    anios.reverse();
    anios.insert(0, format!("Todos los años ({})", total_anios));
    st.anio_options = anios;

    st.filter_periodo_idx = 0;
    st.filter_anio_idx = 0;
    st.filter_dias_idx = 0;
    st.only_sin_cedula = false;
    st.current_page = 0;

    apply_nominas_filtering(&mut st);
    render_nominas_ui(ui, &st);

    Ok(())
}

/// Registra los callbacks de la vista de nóminas.
pub fn wire_nominas_callbacks(
    main_window: &MainWindow,
    db_pool: Option<std::sync::Arc<DbPool>>,
    session: Arc<Mutex<NominasSessionState>>,
) {
    let app_logic = main_window.global::<AppLogic>();

    // Abrir la vista de nóminas desde otro punto de la app (p. ej. el botón
    // "Cargar Nominas" de Registros de Obras).
    let ui_weak_open = main_window.as_weak();
    let pool_open = db_pool.clone();
    let sess_open = session.clone();
    app_logic.on_open_nominas_view(move || {
        let Some(win) = ui_weak_open.upgrade() else { return };
        let d = win.global::<AppData>();
        // Se habilita antes de cambiar de vista: el guard de `on_navigate_view`
        // rechaza la vista 9 mientras el flag esté en falso.
        d.set_nominas_access_allowed(true);
        d.set_current_view(9);
        if let Some(ref pool) = pool_open {
            let _ = sync_and_load_nominas_data(&win, pool, &sess_open, None);
        }
    });

    let s = session.clone();
    let ui_weak = main_window.as_weak();
    let pool_reload = db_pool.clone();
    app_logic.on_nominas_reload(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        if let Some(ref pool) = pool_reload {
            let _ = sync_and_load_nominas_data(&win, pool, &s, None);
        }
    });

    let s = session.clone();
    let ui_weak = main_window.as_weak();
    app_logic.on_nominas_set_search(move |txt| {
        if let Some(win) = ui_weak.upgrade() {
            if let Ok(mut st) = s.lock() {
                st.search_query = txt.to_string();
                st.current_page = 0;
                apply_nominas_filtering(&mut st);
                render_nominas_ui(&win, &st);
            }
        }
    });

    let s = session.clone();
    let ui_weak = main_window.as_weak();
    app_logic.on_nominas_clear_search(move || {
        if let Some(win) = ui_weak.upgrade() {
            if let Ok(mut st) = s.lock() {
                st.search_query.clear();
                st.current_page = 0;
                apply_nominas_filtering(&mut st);
                render_nominas_ui(&win, &st);
            }
        }
    });

    macro_rules! filtro_indice {
        ($cb:ident, $campo:ident) => {{
            let s = session.clone();
            let ui_weak = main_window.as_weak();
            app_logic.$cb(move |idx| {
                if let Some(win) = ui_weak.upgrade() {
                    if let Ok(mut st) = s.lock() {
                        st.$campo = idx.max(0) as usize;
                        st.current_page = 0;
                        apply_nominas_filtering(&mut st);
                        render_nominas_ui(&win, &st);
                    }
                }
            });
        }};
    }
    filtro_indice!(on_nominas_filter_periodo, filter_periodo_idx);
    filtro_indice!(on_nominas_filter_anio, filter_anio_idx);
    filtro_indice!(on_nominas_filter_dias, filter_dias_idx);

    let s = session.clone();
    let ui_weak = main_window.as_weak();
    app_logic.on_nominas_toggle_only_sin_cedula(move || {
        if let Some(win) = ui_weak.upgrade() {
            if let Ok(mut st) = s.lock() {
                st.only_sin_cedula = !st.only_sin_cedula;
                st.current_page = 0;
                apply_nominas_filtering(&mut st);
                render_nominas_ui(&win, &st);
            }
        }
    });

    let s = session.clone();
    let ui_weak = main_window.as_weak();
    app_logic.on_nominas_toggle_detalle(move || {
        if let Some(win) = ui_weak.upgrade() {
            if let Ok(mut st) = s.lock() {
                st.mostrar_detalle = !st.mostrar_detalle;
                render_nominas_ui(&win, &st);
            }
        }
    });

    let s = session.clone();
    let ui_weak = main_window.as_weak();
    app_logic.on_nominas_prev_page(move || {
        if let Some(win) = ui_weak.upgrade() {
            if let Ok(mut st) = s.lock() {
                if st.current_page > 0 {
                    st.current_page -= 1;
                    render_nominas_ui(&win, &st);
                }
            }
        }
    });

    let s = session.clone();
    let ui_weak = main_window.as_weak();
    app_logic.on_nominas_next_page(move || {
        if let Some(win) = ui_weak.upgrade() {
            if let Ok(mut st) = s.lock() {
                let last = st
                    .filtered_rows
                    .len()
                    .div_ceil(st.page_size.max(1))
                    .saturating_sub(1);
                if st.current_page < last {
                    st.current_page += 1;
                    render_nominas_ui(&win, &st);
                }
            }
        }
    });

    let pool_perc = db_pool.clone();
    let ui_weak_perc = main_window.as_weak();
    app_logic.on_nominas_open_percepciones(move |id_nomina, empleado, periodo| {
        let Some(ref pool) = pool_perc else { return };
        let Ok(id) = Uuid::parse_str(id_nomina.as_str()) else { return };
        let Ok(mut client) = pool.get() else { return };
        let Ok(items) = fetch_percepciones_nomina(&mut client, id) else { return };

        let model: Vec<NominaPercepcionItem> = items
            .iter()
            .map(|p| NominaPercepcionItem {
                clave: p.clave.clone().into(),
                concepto: p.concepto.clone().into(),
                tipo_sat: p.tipo_sat.clone().into(),
                importe_str: fmt_money(p.importe).into(),
                gravado_str: fmt_money(p.importe_gravado).into(),
                exento_str: fmt_money(p.importe_exento).into(),
            })
            .collect();
        let total: f64 = items.iter().map(|p| p.importe).sum();

        let Some(win) = ui_weak_perc.upgrade() else { return };
        let d = win.global::<AppData>();
        d.set_nominas_percepciones_detalle(ModelRc::new(VecModel::from(model)));
        d.set_nominas_percepciones_total_str(fmt_money(total).into());
        d.set_nominas_percepciones_empleado(empleado);
        d.set_nominas_percepciones_periodo(periodo);
        d.set_nominas_show_percepciones(true);
    });

    let ui_weak_close = main_window.as_weak();
    app_logic.on_nominas_close_percepciones(move || {
        if let Some(win) = ui_weak_close.upgrade() {
            win.global::<AppData>().set_nominas_show_percepciones(false);
        }
    });

    let ui_weak_excel = main_window.as_weak();
    app_logic.on_nominas_export_excel(move || {
        if let Some(win) = ui_weak_excel.upgrade() {
            win.global::<AppData>().set_nominas_show_percepciones(false);
        }
    });

    // ─── Callbacks de Edición de Días y Percepciones ───
    let pool_edit = db_pool.clone();
    let s_edit = session.clone();
    let ui_weak_edit = main_window.as_weak();
    app_logic.on_nominas_open_edit(move |id_nom_str| {
        let Some(win) = ui_weak_edit.upgrade() else { return };
        let Ok(id_nom) = Uuid::parse_str(id_nom_str.as_str()) else { return };
        let Some(ref pool) = pool_edit else { return };
        let Ok(mut client) = pool.get() else { return };

        let mut st = match s_edit.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        let Some(row) = st.all_rows.iter().find(|r| r.id_nomina == id_nom).cloned() else { return };

        st.edit_id_nomina = Some(id_nom);
        st.edit_empleado = row.nombre_trabajador.clone();
        st.edit_periodo = row.periodo_str.clone();
        st.edit_dias_original = row.num_dias_pagados;
        st.edit_dias_input = fmt_dias(row.num_dias_pagados);

        // Revisar si el empleado tiene baja según cédula de liquidaciones
        let tiene_baja = check_empleado_tiene_baja_periodo(
            &mut client,
            row.id_empleado,
            &row.mes_periodo,
            row.anio_periodo,
        )
        .unwrap_or(false);

        st.edit_is_baja = tiene_baja;
        if tiene_baja {
            st.edit_baja_warning = format!(
                "AVISO: El empleado cuenta con registro de BAJA en este periodo según la cédula de liquidaciones. Sus días pagados ({}) corresponden a los días laborados.",
                fmt_dias(row.num_dias_pagados)
            );
        } else {
            st.edit_baja_warning.clear();
        }

        let conceptos = fetch_conceptos_nomina_detalle(&mut client, id_nom).unwrap_or_default();
        let total = total_conceptos(&conceptos);
        let slint_conceptos = map_conceptos_to_slint(&conceptos);
        st.edit_conceptos = conceptos;

        let d = win.global::<AppData>();
        d.set_nominas_edit_id_nomina(id_nom_str);
        d.set_nominas_edit_empleado(st.edit_empleado.clone().into());
        d.set_nominas_edit_periodo(st.edit_periodo.clone().into());
        d.set_nominas_edit_dias_original_str(fmt_dias(st.edit_dias_original).into());
        d.set_nominas_edit_dias_input(st.edit_dias_input.clone().into());
        d.set_nominas_edit_conceptos(ModelRc::new(VecModel::from(slint_conceptos)));
        d.set_nominas_edit_total_str(fmt_money(total).into());
        d.set_nominas_edit_is_baja(st.edit_is_baja);
        d.set_nominas_edit_baja_warning(st.edit_baja_warning.clone().into());
        d.set_nominas_show_edit_modal(true);
    });

    let ui_weak_close_edit = main_window.as_weak();
    app_logic.on_nominas_close_edit(move || {
        if let Some(win) = ui_weak_close_edit.upgrade() {
            win.global::<AppData>().set_nominas_show_edit_modal(false);
        }
    });

    let s_recalc = session.clone();
    let ui_weak_recalc = main_window.as_weak();
    app_logic.on_nominas_edit_recalc_dias(move |txt| {
        let Some(win) = ui_weak_recalc.upgrade() else { return };
        let mut st = match s_recalc.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        st.edit_dias_input = txt.to_string();
        if let Ok(dias_nuevos) = txt.trim().parse::<f64>() {
            let orig_dias = st.edit_dias_original;
            if dias_nuevos >= 0.0 && orig_dias > 0.0 {
                recalcular_conceptos_proporcional(
                    &mut st.edit_conceptos,
                    orig_dias,
                    dias_nuevos,
                );
                let total = total_conceptos(&st.edit_conceptos);
                let slint_conceptos = map_conceptos_to_slint(&st.edit_conceptos);
                let d = win.global::<AppData>();
                d.set_nominas_edit_conceptos(ModelRc::new(VecModel::from(slint_conceptos)));
                d.set_nominas_edit_total_str(fmt_money(total).into());
            }
        }
        win.global::<AppData>().set_nominas_edit_dias_input(txt);
    });

    let s_update_c = session.clone();
    let ui_weak_update_c = main_window.as_weak();
    app_logic.on_nominas_edit_update_concepto(move |idx, val_str| {
        let Some(win) = ui_weak_update_c.upgrade() else { return };
        let mut st = match s_update_c.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let clean = val_str.replace('$', "").replace(',', "").trim().to_string();
        if let Ok(monto) = clean.parse::<f64>() {
            let i = idx as usize;
            if i < st.edit_conceptos.len() {
                st.edit_conceptos[i].importe = monto;
                let total = total_conceptos(&st.edit_conceptos);
                let slint_conceptos = map_conceptos_to_slint(&st.edit_conceptos);
                let d = win.global::<AppData>();
                d.set_nominas_edit_conceptos(ModelRc::new(VecModel::from(slint_conceptos)));
                d.set_nominas_edit_total_str(fmt_money(total).into());
            }
        }
    });

    let pool_save_edit = db_pool.clone();
    let s_save_edit = session.clone();
    let ui_weak_save_edit = main_window.as_weak();
    app_logic.on_nominas_save_edit(move || {
        let Some(win) = ui_weak_save_edit.upgrade() else { return };
        let Some(ref pool) = pool_save_edit else { return };
        let Ok(mut client) = pool.get() else { return };

        let (id_nom, nuevos_dias, id_y_montos) = {
            let st = match s_save_edit.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            let Some(id) = st.edit_id_nomina else { return };
            let dias = st
                .edit_dias_input
                .trim()
                .parse::<f64>()
                .unwrap_or(st.edit_dias_original);
            let montos: Vec<(Uuid, f64)> = st
                .edit_conceptos
                .iter()
                .map(|c| (c.id_detalle, c.importe))
                .collect();
            (id, dias, montos)
        };

        if let Ok(_) = update_dias_y_conceptos_nomina(&mut client, id_nom, nuevos_dias, &id_y_montos) {
            win.global::<AppData>().set_nominas_show_edit_modal(false);
            let _ = sync_and_load_nominas_data(&win, pool, &s_save_edit, None);
        }
    });

    // ─── Callbacks de Copia con Plantilla ───
    let pool_copy_modal = db_pool.clone();
    let s_copy_modal = session.clone();
    let ui_weak_copy_modal = main_window.as_weak();
    app_logic.on_nominas_open_copy_modal(move |id_nom_str, id_emp_str| {
        let Some(win) = ui_weak_copy_modal.upgrade() else { return };
        let Ok(id_emp) = Uuid::parse_str(id_emp_str.as_str()) else { return };
        let Ok(id_nom) = Uuid::parse_str(id_nom_str.as_str()) else { return };
        let Some(ref pool) = pool_copy_modal else { return };
        let Ok(mut client) = pool.get() else { return };

        let mut st = match s_copy_modal.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        let templates: Vec<NominaReporteRowDb> = st
            .all_rows
            .iter()
            .filter(|r| r.id_empleado == id_emp)
            .cloned()
            .collect();

        if templates.is_empty() {
            return;
        }

        let selected_idx = templates
            .iter()
            .position(|r| r.id_nomina == id_nom)
            .unwrap_or(0);

        let template_options: Vec<NominaTemplateOptionItem> = templates
            .iter()
            .map(|r| NominaTemplateOptionItem {
                id_nomina: r.id_nomina.to_string().into(),
                label: format!(
                    "{} ({} días - {})",
                    r.periodo_str,
                    fmt_dias(r.num_dias_pagados),
                    fmt_money(r.total_percepciones)
                )
                .into(),
                dias: r.num_dias_pagados as f32,
                dias_str: fmt_dias(r.num_dias_pagados).into(),
            })
            .collect();

        let cur_template = templates[selected_idx].clone();
        let conceptos = fetch_conceptos_nomina_detalle(&mut client, cur_template.id_nomina)
            .unwrap_or_default();
        let total = total_conceptos(&conceptos);
        let slint_conceptos = map_conceptos_to_slint(&conceptos);

        st.copy_id_empleado = Some(id_emp);
        st.copy_empleado = cur_template.nombre_trabajador.clone();
        st.copy_selected_template_idx = selected_idx;
        st.copy_target_mes_idx = 0;
        st.copy_target_anio = cur_template.anio_periodo.to_string();
        st.copy_dias_original = cur_template.num_dias_pagados;
        st.copy_dias_input = fmt_dias(cur_template.num_dias_pagados);
        st.copy_conceptos = conceptos;
        st.copy_templates = templates;

        let d = win.global::<AppData>();
        d.set_nominas_copy_id_empleado(id_emp_str);
        d.set_nominas_copy_empleado(st.copy_empleado.clone().into());
        let template_labels: Vec<SharedString> = template_options
            .iter()
            .map(|o| o.label.clone())
            .collect();
        d.set_nominas_copy_templates_list(ModelRc::new(VecModel::from(template_options)));
        d.set_nominas_copy_template_labels(ModelRc::new(VecModel::from(template_labels)));
        d.set_nominas_copy_selected_template_idx(selected_idx as i32);
        d.set_nominas_copy_target_mes_idx(0);
        d.set_nominas_copy_target_anio(st.copy_target_anio.clone().into());
        d.set_nominas_copy_dias_original_str(fmt_dias(st.copy_dias_original).into());
        d.set_nominas_copy_dias_input(st.copy_dias_input.clone().into());
        d.set_nominas_copy_conceptos(ModelRc::new(VecModel::from(slint_conceptos)));
        d.set_nominas_copy_total_str(fmt_money(total).into());
        d.set_nominas_show_copy_modal(true);
    });

    let ui_weak_close_copy = main_window.as_weak();
    app_logic.on_nominas_close_copy_modal(move || {
        if let Some(win) = ui_weak_close_copy.upgrade() {
            win.global::<AppData>().set_nominas_show_copy_modal(false);
        }
    });

    let pool_copy_t = db_pool.clone();
    let s_copy_t = session.clone();
    let ui_weak_copy_t = main_window.as_weak();
    app_logic.on_nominas_copy_select_template(move |idx| {
        let Some(win) = ui_weak_copy_t.upgrade() else { return };
        let Some(ref pool) = pool_copy_t else { return };
        let Ok(mut client) = pool.get() else { return };

        let mut st = match s_copy_t.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let i = idx.max(0) as usize;
        if i >= st.copy_templates.len() {
            return;
        }
        let (t_id_nomina, t_dias) = {
            let t = &st.copy_templates[i];
            (t.id_nomina, t.num_dias_pagados)
        };
        st.copy_selected_template_idx = i;
        st.copy_dias_original = t_dias;
        st.copy_dias_input = fmt_dias(t_dias);

        let conceptos = fetch_conceptos_nomina_detalle(&mut client, t_id_nomina).unwrap_or_default();
        let total = total_conceptos(&conceptos);
        let slint_conceptos = map_conceptos_to_slint(&conceptos);
        st.copy_conceptos = conceptos;

        let d = win.global::<AppData>();
        d.set_nominas_copy_selected_template_idx(idx);
        d.set_nominas_copy_dias_original_str(fmt_dias(st.copy_dias_original).into());
        d.set_nominas_copy_dias_input(st.copy_dias_input.clone().into());
        d.set_nominas_copy_conceptos(ModelRc::new(VecModel::from(slint_conceptos)));
        d.set_nominas_copy_total_str(fmt_money(total).into());
    });

    let s_copy_mes = session.clone();
    let ui_weak_copy_mes = main_window.as_weak();
    app_logic.on_nominas_copy_select_mes(move |idx| {
        if let Some(win) = ui_weak_copy_mes.upgrade() {
            if let Ok(mut st) = s_copy_mes.lock() {
                st.copy_target_mes_idx = idx.max(0) as usize;
                win.global::<AppData>().set_nominas_copy_target_mes_idx(idx);
            }
        }
    });

    let s_copy_anio = session.clone();
    let ui_weak_copy_anio = main_window.as_weak();
    app_logic.on_nominas_copy_set_anio(move |anio| {
        if let Some(win) = ui_weak_copy_anio.upgrade() {
            if let Ok(mut st) = s_copy_anio.lock() {
                st.copy_target_anio = anio.to_string();
                win.global::<AppData>().set_nominas_copy_target_anio(anio);
            }
        }
    });

    let s_copy_recalc = session.clone();
    let ui_weak_copy_recalc = main_window.as_weak();
    app_logic.on_nominas_copy_recalc_dias(move |txt| {
        let Some(win) = ui_weak_copy_recalc.upgrade() else { return };
        let mut st = match s_copy_recalc.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        st.copy_dias_input = txt.to_string();
        if let Ok(dias_nuevos) = txt.trim().parse::<f64>() {
            let orig_dias = st.copy_dias_original;
            if dias_nuevos >= 0.0 && orig_dias > 0.0 {
                recalcular_conceptos_proporcional(
                    &mut st.copy_conceptos,
                    orig_dias,
                    dias_nuevos,
                );
                let total = total_conceptos(&st.copy_conceptos);
                let slint_conceptos = map_conceptos_to_slint(&st.copy_conceptos);
                let d = win.global::<AppData>();
                d.set_nominas_copy_conceptos(ModelRc::new(VecModel::from(slint_conceptos)));
                d.set_nominas_copy_total_str(fmt_money(total).into());
            }
        }
        win.global::<AppData>().set_nominas_copy_dias_input(txt);
    });

    let s_copy_update_c = session.clone();
    let ui_weak_copy_update_c = main_window.as_weak();
    app_logic.on_nominas_copy_update_concepto(move |idx, val_str| {
        let Some(win) = ui_weak_copy_update_c.upgrade() else { return };
        let mut st = match s_copy_update_c.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let clean = val_str.replace('$', "").replace(',', "").trim().to_string();
        if let Ok(monto) = clean.parse::<f64>() {
            let i = idx as usize;
            if i < st.copy_conceptos.len() {
                st.copy_conceptos[i].importe = monto;
                let total = total_conceptos(&st.copy_conceptos);
                let slint_conceptos = map_conceptos_to_slint(&st.copy_conceptos);
                let d = win.global::<AppData>();
                d.set_nominas_copy_conceptos(ModelRc::new(VecModel::from(slint_conceptos)));
                d.set_nominas_copy_total_str(fmt_money(total).into());
            }
        }
    });

    let pool_submit_copy = db_pool.clone();
    let s_submit_copy = session.clone();
    let ui_weak_submit_copy = main_window.as_weak();
    app_logic.on_nominas_submit_create_nomina(move || {
        let Some(win) = ui_weak_submit_copy.upgrade() else { return };
        let Some(ref pool) = pool_submit_copy else { return };
        let Ok(mut client) = pool.get() else { return };

        let meses = [
            "ENERO", "FEBRERO", "MARZO", "ABRIL", "MAYO", "JUNIO",
            "JULIO", "AGOSTO", "SEPTIEMBRE", "OCTUBRE", "NOVIEMBRE", "DICIEMBRE",
        ];

        let (id_template, id_emp, anio_destino, mes_destino, dias_modificados, conceptos) = {
            let st = match s_submit_copy.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            let Some(id_emp) = st.copy_id_empleado else { return };
            if st.copy_templates.is_empty() { return; }
            let id_template = st.copy_templates[st.copy_selected_template_idx].id_nomina;
            let anio = st.copy_target_anio.trim().parse::<i32>().unwrap_or(2026);
            let mes_idx = st.copy_target_mes_idx.min(11);
            let mes = meses[mes_idx];
            let dias = st.copy_dias_input.trim().parse::<f64>().unwrap_or(st.copy_dias_original);
            (id_template, id_emp, anio, mes, dias, st.copy_conceptos.clone())
        };

        let custom_pairs: Vec<(Uuid, f64)> = conceptos
            .iter()
            .map(|c| (c.id_concepto, c.importe))
            .collect();

        if let Ok(_) = create_nomina_from_template(
            &mut client,
            id_emp,
            id_template,
            mes_destino,
            anio_destino,
            dias_modificados,
            Some(&custom_pairs),
        ) {
            win.global::<AppData>().set_nominas_show_copy_modal(false);
            let _ = sync_and_load_nominas_data(&win, pool, &s_submit_copy, None);
        }
    });

    // ─── Callbacks del Detector de Nóminas Faltantes y Creación Masiva ───
    let pool_faltantes = db_pool.clone();
    let s_faltantes = session.clone();
    let ui_weak_faltantes = main_window.as_weak();
    app_logic.on_nominas_open_faltantes_modal(move || {
        let Some(win) = ui_weak_faltantes.upgrade() else { return };
        let Some(ref pool) = pool_faltantes else { return };
        let Ok(mut client) = pool.get() else { return };

        let items = fetch_nominas_faltantes(&mut client, None).unwrap_or_default();
        let mut st = match s_faltantes.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        st.faltantes = items.into_iter().map(|f| (f, false)).collect();
        st.faltantes_status_msg = format!("Se detectaron {} nóminas faltantes o incompletas según las cédulas de cuotas.", st.faltantes.len());

        let d = win.global::<AppData>();
        render_faltantes_ui(&d, &st);
        d.set_nominas_show_faltantes_modal(true);
    });

    let ui_weak_close_faltantes = main_window.as_weak();
    app_logic.on_nominas_close_faltantes_modal(move || {
        if let Some(win) = ui_weak_close_faltantes.upgrade() {
            win.global::<AppData>().set_nominas_show_faltantes_modal(false);
        }
    });

    let s_toggle_f = session.clone();
    let ui_weak_toggle_f = main_window.as_weak();
    app_logic.on_nominas_faltantes_toggle_item(move |idx| {
        let Some(win) = ui_weak_toggle_f.upgrade() else { return };
        let mut st = match s_toggle_f.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let i = idx as usize;
        if i < st.faltantes.len() {
            st.faltantes[i].1 = !st.faltantes[i].1;
        }
        let d = win.global::<AppData>();
        render_faltantes_ui(&d, &st);
    });

    let s_toggle_all_f = session.clone();
    let ui_weak_toggle_all_f = main_window.as_weak();
    app_logic.on_nominas_faltantes_toggle_all(move |sel| {
        let Some(win) = ui_weak_toggle_all_f.upgrade() else { return };
        let mut st = match s_toggle_all_f.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        for item in st.faltantes.iter_mut() {
            item.1 = sel;
        }
        let d = win.global::<AppData>();
        render_faltantes_ui(&d, &st);
    });

    let pool_crear_ind = db_pool.clone();
    let s_crear_ind = session.clone();
    let ui_weak_crear_ind = main_window.as_weak();
    app_logic.on_nominas_faltantes_crear_individual(move |idx| {
        let Some(win) = ui_weak_crear_ind.upgrade() else { return };
        let Some(ref pool) = pool_crear_ind else { return };
        let Ok(mut client) = pool.get() else { return };

        let item = {
            let st = match s_crear_ind.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            let i = idx as usize;
            if i >= st.faltantes.len() { return; }
            st.faltantes[i].0.clone()
        };

        let req = if let Some(tpl) = item.id_template_sugerido {
            NominaFaltanteCreateRequest {
                id_empleado: item.id_empleado,
                id_template: tpl,
                target_mes: item.mes_periodo.clone(),
                target_anio: item.anio_periodo,
                target_dias: item.dias_faltantes,
            }
        } else {
            return;
        };

        if let Ok(count) = create_bulk_missing_nominas(&mut client, &[req]) {
            let mut st = match s_crear_ind.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            let i = idx as usize;
            if i < st.faltantes.len() {
                st.faltantes.remove(i);
            }
            st.faltantes_status_msg = format!("¡Se creó exitosamente {} nómina faltante!", count);
            let d = win.global::<AppData>();
            render_faltantes_ui(&d, &st);
            let _ = sync_and_load_nominas_data(&win, pool, &s_crear_ind, None);
        }
    });

    let pool_crear_mas = db_pool.clone();
    let s_crear_mas = session.clone();
    let ui_weak_crear_mas = main_window.as_weak();
    app_logic.on_nominas_faltantes_crear_masivo(move || {
        let Some(win) = ui_weak_crear_mas.upgrade() else { return };
        let Some(ref pool) = pool_crear_mas else { return };
        let Ok(mut client) = pool.get() else { return };

        let items_to_create: Vec<NominaFaltanteDb> = {
            let st = match s_crear_mas.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            st.faltantes
                .iter()
                .filter(|(_, sel)| *sel)
                .map(|(f, _)| f.clone())
                .collect()
        };

        let requests: Vec<NominaFaltanteCreateRequest> = items_to_create
            .into_iter()
            .filter_map(|f| {
                f.id_template_sugerido.map(|tpl| NominaFaltanteCreateRequest {
                    id_empleado: f.id_empleado,
                    id_template: tpl,
                    target_mes: f.mes_periodo,
                    target_anio: f.anio_periodo,
                    target_dias: f.dias_faltantes,
                })
            })
            .collect();

        if requests.is_empty() {
            return;
        }

        if let Ok(count) = create_bulk_missing_nominas(&mut client, &requests) {
            let mut st = match s_crear_mas.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            st.faltantes.retain(|(_, sel)| !*sel);
            st.faltantes_status_msg = format!("¡Se crearon exitosamente {} nóminas faltantes en lote!", count);
            let d = win.global::<AppData>();
            render_faltantes_ui(&d, &st);
            let _ = sync_and_load_nominas_data(&win, pool, &s_crear_mas, None);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_dias_entero_y_decimal() {
        assert_eq!(fmt_dias(30.0), "30");
        assert_eq!(fmt_dias(15.5), "15.5");
        assert_eq!(fmt_dias(0.0), "0");
        // Dentro de la tolerancia de 0.005 se redondea al entero.
        assert_eq!(fmt_dias(30.004), "30");
        // Fuera de la tolerancia se muestra un decimal: no se oculta la
        // diferencia real contra la cédula.
        assert_eq!(fmt_dias(30.04), "30.0");
        assert_eq!(fmt_dias(30.06), "30.1");
    }

    #[test]
    fn test_fmt_money_con_separadores() {
        assert_eq!(fmt_money(1234.5), "$1,234.50");
        assert_eq!(fmt_money(0.0), "$0.00");
    }

    #[test]
    fn test_fmt_fecha_dmy_y_guion() {
        let f = NaiveDate::from_ymd_opt(2026, 4, 30).unwrap();
        assert_eq!(fmt_fecha(Some(f)), "30/04/2026");
        assert_eq!(fmt_fecha(None), "-");
    }

    #[test]
    fn test_fmt_semana() {
        assert_eq!(fmt_semana(18), "S18");
        assert_eq!(fmt_semana(0), "-");
    }

    #[test]
    fn test_build_rfc_curp_sin_duplicar() {
        assert_eq!(build_rfc_curp("", "jifa981012mclmnl01"), "jifa981012mclmnl01");
        assert_eq!(
            build_rfc_curp("JIFA981012A12", "JIFA981012MCLMNL01"),
            "JIFA981012A12\nJIFA981012MCLMNL01"
        );
        assert_eq!(
            build_rfc_curp("JIFA981012MCLMNL01", "JIFA981012MCLMNL01"),
            "JIFA981012MCLMNL01"
        );
        assert_eq!(build_rfc_curp("", ""), "SIN RFC/CURP");
    }

    /// Fila de prueba con los campos que el filtrado necesita.
    fn fila(
        emp: Uuid,
        nombre: &str,
        periodo: &str,
        anio: i32,
        dias_nom: f64,
        dias_ced: f64,
    ) -> NominaReporteRowDb {
        let (mes, _) = periodo.split_once(" / ").unwrap_or((periodo, "2026"));
        NominaReporteRowDb {
            id_nomina: Uuid::now_v7(),
            id_empleado: emp,
            nombre_trabajador: nombre.into(),
            nss: "01139963936".into(),
            rfc: String::new(),
            curp: "CEAL991008HTSRCS09".into(),
            registro_patronal: "A3677458103".into(),
            periodo_str: periodo.into(),
            anio_periodo: anio,
            mes_periodo: mes.into(),
            fecha_periodo: NaiveDate::from_ymd_opt(anio, 4, 30),
            semana_fiscal: 18,
            fecha_inicial_pago: None,
            fecha_final_pago: None,
            fecha_pago: None,
            fecha_inicio_relacion_laboral: None,
            fecha_emision_cfdi: None,
            fecha_timbrado: None,
            num_dias_pagados: dias_nom,
            dias_cedula: dias_ced,
            total_sueldos: 5000.0,
            total_gravado: 0.0,
            total_exento: 0.0,
            total_percepciones: 5000.0,
            percepciones_gravadas: 0.0,
            percepciones_exentas: 0.0,
            num_percepciones: 1,
            salario_diario_integrado: 500.0,
            salario_diario_calculado: 0.0,
            tipo_nomina: String::new(),
            periodicidad: String::new(),
            version_nomina: String::new(),
            nombre_archivo: String::new(),
        }
    }

    fn estado_con(filas: Vec<NominaReporteRowDb>) -> NominasSessionState {
        let mut st = NominasSessionState::default();
        st.dias_por_empleado = build_dias_por_empleado(&filas);
        let mut p: Vec<String> = filas.iter().map(|r| r.periodo_str.clone()).collect();
        p.sort();
        p.dedup();
        p.reverse();
        st.periodo_options = p;
        let mut a: Vec<String> = filas.iter().map(|r| r.anio_periodo.to_string()).collect();
        a.sort();
        a.dedup();
        a.reverse();
        st.anio_options = a;
st.all_rows = filas;
    st.periodo_options
        .insert(0, format!("Todos los periodos ({})", st.periodo_options.len()));
    st.anio_options
        .insert(0, format!("Todos los años ({})", st.anio_options.len()));
    st
}

    #[test]
    fn test_filtro_coinciden_vs_difieren() {
        let e1 = Uuid::now_v7();
        let e2 = Uuid::now_v7();
        let e3 = Uuid::now_v7();
        let filas = vec![
            fila(e1, "IGUAL", "ABRIL / 2026", 2026, 30.0, 30.0),
            fila(e2, "DIFIERE", "ABRIL / 2026", 2026, 30.0, 15.0),
            fila(e3, "SINCEDULA", "ABRIL / 2026", 2026, 30.0, 0.0),
        ];

        // Solo coinciden
        let mut st = estado_con(filas.clone());
        st.filter_dias_idx = 1;
        apply_nominas_filtering(&mut st);
        assert_eq!(st.filtered_rows.len(), 1);
        assert_eq!(st.filtered_rows[0].nombre_trabajador, "IGUAL");

        // Solo difieren (sin cédula no cuenta como diferencia)
        let mut st = estado_con(filas.clone());
        st.filter_dias_idx = 2;
        apply_nominas_filtering(&mut st);
        assert_eq!(st.filtered_rows.len(), 1);
        assert_eq!(st.filtered_rows[0].nombre_trabajador, "DIFIERE");

        // Solo sin cédula
        let mut st = estado_con(filas);
        st.only_sin_cedula = true;
        apply_nominas_filtering(&mut st);
        assert_eq!(st.filtered_rows.len(), 1);
        assert_eq!(st.filtered_rows[0].nombre_trabajador, "SINCEDULA");
    }

    #[test]
    fn test_filtro_busqueda_por_campo() {
        let e = Uuid::now_v7();
        let filas = vec![
            fila(e, "JIMENEZ DE LA FUENTE", "ABRIL / 2026", 2026, 30.0, 30.0),
            fila(Uuid::now_v7(), "CRUZ LUNA ROBERTO", "MAYO / 2026", 2026, 15.0, 15.0),
        ];
        let mut st = estado_con(filas);
        st.search_query = "jimenez".into();
        apply_nominas_filtering(&mut st);
        assert_eq!(st.filtered_rows.len(), 1);

// La búsqueda también alcanza CURP y NSS: se da un CURP distinto por fila
    // para que el acierto sea atribuible a un solo trabajador.
    let mut st = estado_con(vec![
        fila(e, "JIMENEZ", "ABRIL / 2026", 2026, 30.0, 30.0),
        fila(Uuid::now_v7(), "OTRO", "MAYO / 2026", 2026, 15.0, 15.0),
    ]);
    if let Some(f) = st.all_rows.get_mut(1) {
        f.curp = "OTRO991008HTSRCS09".into();
        f.nss = "99999999999".into();
    }
    st.search_query = "ceal991008".into();
    apply_nominas_filtering(&mut st);
    assert_eq!(st.filtered_rows.len(), 1);
    assert_eq!(st.filtered_rows[0].nombre_trabajador, "JIMENEZ");
}

    #[test]
    fn test_filtro_periodo_y_anio() {
        let filas = vec![
            fila(Uuid::now_v7(), "A", "ABRIL / 2026", 2026, 30.0, 30.0),
            fila(Uuid::now_v7(), "B", "MAYO / 2026", 2026, 15.0, 15.0),
            fila(Uuid::now_v7(), "C", "ABRIL / 2027", 2027, 10.0, 10.0),
        ];
        // Periodo ABRIL / 2026
        let mut st = estado_con(filas.clone());
        // idx 0 = "Todos"; el periodo real está en idx >= 1.
        let idx = st.periodo_options.iter().position(|p| p == "ABRIL / 2026").unwrap();
        st.filter_periodo_idx = idx;
        apply_nominas_filtering(&mut st);
        assert_eq!(st.filtered_rows.len(), 1);
        assert_eq!(st.filtered_rows[0].nombre_trabajador, "A");

        // Año 2027
        let mut st = estado_con(filas);
        let idx = st.anio_options.iter().position(|a| a == "2027").unwrap();
        st.filter_anio_idx = idx;
        apply_nominas_filtering(&mut st);
        assert_eq!(st.filtered_rows.len(), 1);
        assert_eq!(st.filtered_rows[0].nombre_trabajador, "C");
    }

    #[test]
    fn test_dias_por_mes_acumulan_varias_nominas_del_mes() {
        // Dos quincenas del mismo mes deben sumar en el desglose mensual.
        let e = Uuid::now_v7();
        let filas = vec![
            fila(e, "JIMENEZ", "ABRIL / 2026", 2026, 15.0, 15.0),
            fila(e, "JIMENEZ", "ABRIL / 2026", 2026, 15.0, 15.0),
            fila(e, "JIMENEZ", "MAYO / 2026", 2026, 30.0, 30.0),
        ];
        let st = estado_con(filas);
        let meses = st.dias_por_empleado.get(&e).unwrap();
        assert_eq!(meses.len(), 2, "dos periodos distintos");
        // Ordenado de más antiguo a más reciente.
        assert_eq!(meses[0].0, "ABRIL / 2026");
        assert_eq!(meses[0].2, 30.0, "las dos quincenas suman 30");
        assert_eq!(meses[1].0, "MAYO / 2026");
    }

    #[test]
    fn test_dias_por_empleado_aisla_trabajadores() {
        let e1 = Uuid::now_v7();
        let e2 = Uuid::now_v7();
        let filas = vec![
            fila(e1, "UNO", "ABRIL / 2026", 2026, 30.0, 30.0),
            fila(e2, "DOS", "ABRIL / 2026", 2026, 15.0, 15.0),
        ];
        let st = estado_con(filas);
        assert_eq!(st.dias_por_empleado.get(&e1).unwrap()[0].2, 30.0);
        assert_eq!(st.dias_por_empleado.get(&e2).unwrap()[0].2, 15.0);
    }

    #[test]
    fn test_totales_por_mes_agrega() {
        let filas = vec![
            fila(Uuid::now_v7(), "A", "ABRIL / 2026", 2026, 30.0, 30.0),
            fila(Uuid::now_v7(), "B", "ABRIL / 2026", 2026, 15.0, 15.0),
            fila(Uuid::now_v7(), "C", "MAYO / 2026", 2026, 30.0, 30.0),
        ];
        let t = build_mes_totals(&filas);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].periodo, "ABRIL / 2026");
        assert_eq!(t[0].nominas, 2);
        assert_eq!(t[0].dias_pagados_str.as_str(), "45");
        assert_eq!(t[0].dias_cedula_str.as_str(), "45");
        assert_eq!(t[1].periodo, "MAYO / 2026");
        assert_eq!(t[1].nominas, 1);
    }

    #[test]
    fn test_paginacion_acota_pagina_actual() {
        let filas: Vec<_> = (0..250)
            .map(|i| {
                fila(
                    Uuid::now_v7(),
                    &format!("TRABAJADOR {}", i),
                    "ABRIL / 2026",
                    2026,
                    30.0,
                    30.0,
                )
            })
            .collect();
        let mut st = estado_con(filas);
        st.page_size = 100;
        st.current_page = 99;
        apply_nominas_filtering(&mut st);
        assert_eq!(st.filtered_rows.len(), 250);
        // Se recorta a la última página válida (250/100 = 3 páginas -> idx 2).
        assert_eq!(st.current_page, 2);
    }

    #[test]
    fn test_vacio_no_panea() {
        let mut st = NominasSessionState::default();
        st.page_size = 100;
        apply_nominas_filtering(&mut st);
        assert!(st.filtered_rows.is_empty());
        assert!(build_dias_por_empleado(&[]).is_empty());
        assert!(build_mes_totals(&[]).is_empty());
    }

    #[test]
    fn test_percepcion_resumen_legible() {
        let mut f = fila(Uuid::now_v7(), "A", "ABRIL / 2026", 2026, 30.0, 30.0);
        f.num_percepciones = 3;
        assert_eq!(percepcion_resumen(&f), "3 concepto(s)");
        f.num_percepciones = 0;
        assert_eq!(percepcion_resumen(&f), "-");
    }

    #[test]
    fn test_recalcular_conceptos_proporcional_formula() {
        // Fórmula requerida: Concepto Percepcion / Valor de dia original * dias que el usuario modifico
        let mut conceptos = vec![
            NominaConceptoEditableDb {
                id_detalle: Uuid::now_v7(),
                id_concepto: Uuid::now_v7(),
                clave: "001".into(),
                concepto: "Sueldos y Salarios".into(),
                importe_original: 1400.0,
                importe: 1400.0,
            },
            NominaConceptoEditableDb {
                id_detalle: Uuid::now_v7(),
                id_concepto: Uuid::now_v7(),
                clave: "038".into(),
                concepto: "Bono de Asistencia".into(),
                importe_original: 350.0,
                importe: 350.0,
            },
        ];

        // 7 días originales -> modificado a 6 días
        recalcular_conceptos_proporcional(&mut conceptos, 7.0, 6.0);
        // Sueldos: 1400 / 7 * 6 = 1200.00
        assert_eq!(conceptos[0].importe, 1200.0);
        // Bono: 350 / 7 * 6 = 300.00
        assert_eq!(conceptos[1].importe, 300.0);
        assert_eq!(total_conceptos(&conceptos), 1500.0);
    }
}

