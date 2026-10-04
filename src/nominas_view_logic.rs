use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{Datelike, NaiveDate};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use uuid::Uuid;

use crate::db::nominas_reporte::{
    fetch_nominas_reporte, fetch_percepcion_conceptos,
    fetch_percepciones_nomina, fetch_percepciones_por_nomina,
    summarize_nominas, NominaReporteRowDb,
};
use crate::db::DbPool;
use crate::{
    AppData, AppLogic, MainWindow, NominaEmpleadoDiaItem, NominaMesTotalItem, NominaPercepcionItem,
    NominaRow,
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
    // 1540px de columnas fijas + ~100px por columna de percepción.
    let min_w = 1540.0 + (state.percepcion_headers.len() as f32) * 100.0;
    d.set_nominas_table_min_width(min_w);

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

    let rows = fetch_nominas_reporte(&mut client, cliente_id)?;

    let mut st = session
        .lock()
        .map_err(|_| "No se pudo bloquear el estado de nóminas".to_string())?;

    st.all_rows = rows;
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
}
