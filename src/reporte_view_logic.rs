use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use chrono::NaiveDate;
use uuid::Uuid;
use slint::{Model, ModelRc, VecModel, ComponentHandle, SharedString};

use crate::{
    MainWindow, AppData, AppLogic, ReporteLiquidacionRow, ReporteObraItem,
    ReporteFilterOptionItem, ReporteMesTotalItem, ReporteGuarderiaObraItem,
    ReporteObraUsuarioItem, ReporteSuaObraItem, ReporteBatchWorkerItem,
    ReporteSelectedItem, ReporteBatchObraItem, ReporteEditObraItem,
    ReporteUniqueEmpleadoItem,
};
use crate::cedulas::business::month_to_num;
use crate::db::DbPool;
use crate::db::reporte_liquidaciones::{
    ReporteLiquidacionRowDb, ObraOptionDb,
    fetch_reporte_liquidaciones, fetch_obras_for_revision, fetch_all_obras_for_revision,
    update_single_reporte_row, batch_save_reporte_row_updates,
    sync_reportes_liquidaciones_for_revision, revertir_reporte_liquidaciones_a_originales,
    export_reporte_liquidaciones_to_excel,
    compute_guarderias_kpis,
    extract_all_employee_movements, build_all_employee_movements_map,
    calculate_max_dias_obra_for_employee_movements, validate_obra_assignment_for_employee_movements,
    calculate_obra_days_in_month, parse_month_and_year, ReporteFinalHeaderMeta,
};
use crate::sua_export;

#[derive(Clone)]
pub struct ReporteSessionState {
    pub current_revision_id: Option<String>,
    pub cliente_nombre: String,
    pub rp_registro: String,
    pub revision_nombre: String,
    pub revision_folio: String,
    pub all_rows: Vec<ReporteLiquidacionRowDb>,
    pub filtered_rows: Vec<ReporteLiquidacionRowDb>,
    pub selected_ids: HashSet<Uuid>,
    pub obras_options: Vec<ObraOptionDb>,
    pub all_obras_options: Vec<ObraOptionDb>,
    pub periodo_options: Vec<String>,
    pub periodo_values: Vec<String>,
    pub current_page: usize,
    pub page_size: usize,
    pub search_query: String,
    pub filter_obra_idx: usize,
    pub filter_periodo_idx: usize,
    pub filter_estado_idx: usize,
    pub expanded_row_id: Option<Uuid>,

    // Ordenamiento multi-criterio
    pub sort_by_obra: bool,
    pub sort_obra_asc: bool,
    pub sort_by_anio: bool,
    pub sort_anio_asc: bool,
    pub sort_by_mes: bool,
    pub sort_mes_asc: bool,
    pub sort_by_rfc: bool,
    pub sort_rfc_asc: bool,

    // Filtro por empleados seleccionados
    pub emp_filter_active: bool,
    pub emp_filter_all_months: bool,
    pub emp_filter_all_obras: bool,
    pub emp_filter_worker_ids: HashSet<Uuid>,
    pub emp_filter_worker_rfcs: HashSet<String>,
    pub emp_filter_selected_months: HashSet<String>,
    pub emp_filter_selected_obras: HashSet<String>,

    // Modal de empleados
    pub emp_modal_worker_count: usize,
    pub emp_modal_worker_names_str: String,
    pub emp_modal_total_occurrences: usize,
    pub emp_modal_meses: Vec<(String, String, usize, bool)>,
    pub emp_modal_obras: Vec<(String, String, usize, bool)>,
    pub quick_select_obras: Vec<(String, String, usize, bool)>,

    // Exportación SUA
    pub sua_selected_obra_ids: HashSet<Uuid>,
    pub sua_include_unassigned: bool,

    // Datos del modal individual de edición de obras y días
    pub edit_items: Vec<ReporteEditObraItem>,

    // Filtro multi-selección de empleados únicos (menú principal)
    pub unique_empleados: Vec<ReporteUniqueEmpleadoItem>,
    pub selected_unique_emp_ids: HashSet<String>,
    pub unique_emp_search_query: String,

    // Estado del Side Drawer de Registros de Obra
    pub is_drawer_open: bool,
}

impl Default for ReporteSessionState {
    fn default() -> Self {
        Self {
            current_revision_id: None,
            cliente_nombre: String::new(),
            rp_registro: String::new(),
            revision_nombre: String::new(),
            revision_folio: String::new(),
            all_rows: Vec::new(),
            filtered_rows: Vec::new(),
            selected_ids: HashSet::new(),
            obras_options: Vec::new(),
            all_obras_options: Vec::new(),
            periodo_options: Vec::new(),
            periodo_values: Vec::new(),
            current_page: 1,
            page_size: 100,
            search_query: String::new(),
            filter_obra_idx: 0,
            filter_periodo_idx: 0,
            filter_estado_idx: 0,
            expanded_row_id: None,

            sort_by_obra: false,
            sort_obra_asc: true,
            sort_by_anio: false,
            sort_anio_asc: true,
            sort_by_mes: false,
            sort_mes_asc: true,
            sort_by_rfc: false,
            sort_rfc_asc: true,

            emp_filter_active: false,
            emp_filter_all_months: true,
            emp_filter_all_obras: true,
            emp_filter_worker_ids: HashSet::new(),
            emp_filter_worker_rfcs: HashSet::new(),
            emp_filter_selected_months: HashSet::new(),
            emp_filter_selected_obras: HashSet::new(),

            emp_modal_worker_count: 0,
            emp_modal_worker_names_str: String::new(),
            emp_modal_total_occurrences: 0,
            emp_modal_meses: Vec::new(),
            emp_modal_obras: Vec::new(),
            quick_select_obras: Vec::new(),

            sua_selected_obra_ids: HashSet::new(),
            sua_include_unassigned: false,

            edit_items: Vec::new(),

            unique_empleados: Vec::new(),
            selected_unique_emp_ids: HashSet::new(),
            unique_emp_search_query: String::new(),

            is_drawer_open: false,
        }
    }
}

/// Etiqueta de la columna RFC/CURP.
///
/// Solo une los identificadores presentes, de modo que un RFC vacío nunca
/// produce una línea en blanco delante de la CURP. Cuando no hay ninguno se
/// devuelve un marcador visible en lugar de una celda en blanco.
pub fn build_rfc_curp_label(rfc: &str, curp: &str) -> String {
    let rfc = rfc.trim();
    let curp = curp.trim();

    if !curp.is_empty() && curp.eq_ignore_ascii_case(rfc) {
        return rfc.to_string();
    }

    let parts: Vec<&str> = [rfc, curp]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();

    if parts.is_empty() {
        "SIN RFC/CURP".to_string()
    } else {
        parts.join("\n")
    }
}

pub fn get_clean_non_generic_rfc(rfc: &str) -> Option<String> {
    let trimmed = rfc.trim();
    if trimmed.is_empty() || crate::validation::is_generic_sat_identifier(trimmed) {
        None
    } else {
        Some(trimmed.to_uppercase())
    }
}

pub fn format_curr(val: f64) -> String {
    let is_neg = val < -0.001;
    let abs_val = val.abs();
    let int_part = abs_val.trunc() as u64;
    let frac_part = ((abs_val.fract() * 100.0).round() as u64).min(99);

    let int_str = int_part.to_string();
    let mut formatted_int = String::new();
    let len = int_str.len();

    for (i, c) in int_str.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            formatted_int.push(',');
        }
        formatted_int.push(c);
    }

    if is_neg {
        format!("-${}.{:02}", formatted_int, frac_part)
    } else {
        format!("${}.{:02}", formatted_int, frac_part)
    }
}

pub fn get_obra_colors(index: usize) -> (slint::Color, slint::Color, slint::Color) {
    let palettes = [
        // 0: Esmeralda / Menta suave
        (slint::Color::from_argb_u8(255, 240, 253, 244), slint::Color::from_argb_u8(255, 220, 252, 231), slint::Color::from_argb_u8(255, 22, 163, 74)),
        // 1: Azul Cielo / Índigo suave
        (slint::Color::from_argb_u8(255, 239, 246, 255), slint::Color::from_argb_u8(255, 219, 234, 254), slint::Color::from_argb_u8(255, 37, 99, 235)),
        // 2: Lavanda / Púrpura suave
        (slint::Color::from_argb_u8(255, 250, 245, 255), slint::Color::from_argb_u8(255, 243, 232, 255), slint::Color::from_argb_u8(255, 147, 51, 234)),
        // 3: Ámbar / Arena suave
        (slint::Color::from_argb_u8(255, 255, 251, 235), slint::Color::from_argb_u8(255, 254, 243, 199), slint::Color::from_argb_u8(255, 217, 119, 6)),
        // 4: Rosa / Frambuesa suave
        (slint::Color::from_argb_u8(255, 255, 241, 242), slint::Color::from_argb_u8(255, 255, 228, 230), slint::Color::from_argb_u8(255, 225, 29, 72)),
        // 5: Teal / Turquesa suave
        (slint::Color::from_argb_u8(255, 240, 253, 250), slint::Color::from_argb_u8(255, 204, 251, 241), slint::Color::from_argb_u8(255, 13, 148, 136)),
        // 6: Fucsia suave
        (slint::Color::from_argb_u8(255, 253, 244, 255), slint::Color::from_argb_u8(255, 250, 232, 255), slint::Color::from_argb_u8(255, 192, 38, 211)),
        // 7: Cian hielo
        (slint::Color::from_argb_u8(255, 236, 254, 255), slint::Color::from_argb_u8(255, 207, 250, 254), slint::Color::from_argb_u8(255, 8, 145, 178)),
        // 8: Lima / Verde oliva suave
        (slint::Color::from_argb_u8(255, 254, 252, 232), slint::Color::from_argb_u8(255, 254, 249, 195), slint::Color::from_argb_u8(255, 202, 138, 4)),
        // 9: Naranja cálido suave
        (slint::Color::from_argb_u8(255, 255, 247, 237), slint::Color::from_argb_u8(255, 254, 237, 213), slint::Color::from_argb_u8(255, 234, 88, 12)),
    ];
    palettes[index % palettes.len()]
}

pub fn recalculate_row_in_memory(row: &mut ReporteLiquidacionRowDb) {
    let dias_cedula_f = row.dias_cedula as f64;
    let sum_incidencias = row.lic + row.inc + row.aus;
    let dias_efectivos = (dias_cedula_f - sum_incidencias).max(0.0);

    // Restricción: Días obra no puede exceder los días efectivos de cédula (dias_cedula - LIC - INC - AUS)
    if (row.dias_obra as f64) > dias_efectivos {
        row.dias_obra = dias_efectivos.round() as i16;
    }

    if dias_cedula_f <= 0.0001 || row.dias_obra <= 0 {
        row.cf = 0.0;
        row.exc_pat = 0.0;
        row.exc_obr = 0.0;
        row.pd_pat = 0.0;
        row.pd_obr = 0.0;
        row.gmp_pat = 0.0;
        row.gmp_obr = 0.0;
        row.rt = 0.0;
        row.iv_pat = 0.0;
        row.iv_obr = 0.0;
        row.gps = 0.0;
        row.patronal = 0.0;
        row.obrera = 0.0;
        row.subtotal = 0.0;
        return;
    }

    // Regla de tres estricta: Valor_Prorrateado = (Valor_Original / Días_Cédula) * Días_Obra
    let factor = if row.id_registro_obra.is_some() {
        (row.dias_obra as f64) / dias_cedula_f
    } else {
        1.0
    };
    let round2 = |v: f64| (v * 100.0).round() / 100.0;

    // Respaldo de seguridad: Si los campos originales estaban en 0.0 pero la fila ya tenía importes, no perder la base
    if row.cf_orig == 0.0 && row.cf > 0.0 { row.cf_orig = row.cf; }
    if row.exc_pat_orig == 0.0 && row.exc_pat > 0.0 { row.exc_pat_orig = row.exc_pat; }
    if row.exc_obr_orig == 0.0 && row.exc_obr > 0.0 { row.exc_obr_orig = row.exc_obr; }
    if row.pd_pat_orig == 0.0 && row.pd_pat > 0.0 { row.pd_pat_orig = row.pd_pat; }
    if row.pd_obr_orig == 0.0 && row.pd_obr > 0.0 { row.pd_obr_orig = row.pd_obr; }
    if row.gmp_pat_orig == 0.0 && row.gmp_pat > 0.0 { row.gmp_pat_orig = row.gmp_pat; }
    if row.gmp_obr_orig == 0.0 && row.gmp_obr > 0.0 { row.gmp_obr_orig = row.gmp_obr; }
    if row.rt_orig == 0.0 && row.rt > 0.0 { row.rt_orig = row.rt; }
    if row.iv_pat_orig == 0.0 && row.iv_pat > 0.0 { row.iv_pat_orig = row.iv_pat; }
    if row.iv_obr_orig == 0.0 && row.iv_obr > 0.0 { row.iv_obr_orig = row.iv_obr; }
    if row.gps_orig == 0.0 && row.gps > 0.0 { row.gps_orig = row.gps; }
    if row.subtotal_orig == 0.0 && row.subtotal > 0.0 { row.subtotal_orig = row.subtotal; }

    row.cf = round2(row.cf_orig * factor);
    row.exc_pat = round2(row.exc_pat_orig * factor);
    row.exc_obr = round2(row.exc_obr_orig * factor);
    row.pd_pat = round2(row.pd_pat_orig * factor);
    row.pd_obr = round2(row.pd_obr_orig * factor);
    row.gmp_pat = round2(row.gmp_pat_orig * factor);
    row.gmp_obr = round2(row.gmp_obr_orig * factor);
    row.rt = round2(row.rt_orig * factor);
    row.iv_pat = round2(row.iv_pat_orig * factor);
    row.iv_obr = round2(row.iv_obr_orig * factor);
    row.gps = round2(row.gps_orig * factor);

    row.patronal = round2(
        row.cf + row.exc_pat + row.pd_pat + row.gmp_pat + row.rt + row.iv_pat + row.gps,
    );
    row.obrera = round2(row.exc_obr + row.pd_obr + row.gmp_obr + row.iv_obr);
    row.subtotal = round2(row.subtotal_orig * factor);
}

/// Recalcula el Tope Máx dinámico para cada obra en el modal individual (o multi-obra).
/// Para cada obra seleccionada:
///   tope_max = min(dias_activos_mes, max(0, total_disponibles - suma_otras_obras_seleccionadas))
/// Validación Visual IMSS:
///   has_error = (dias_obra > tope_max) || (dias_obra < 0) || (total_selected_days > total_disponibles)
pub fn recalc_edit_obras_topes(items: &mut [ReporteEditObraItem], total_disponibles: i32) -> bool {
    let total_selected_days: i32 = items.iter().filter(|it| it.selected).map(|it| it.dias_obra).sum();
    let mut any_error = total_selected_days > total_disponibles;

    for i in 0..items.len() {
        if items[i].selected {
            let other_days = total_selected_days - items[i].dias_obra;
            let available_for_this = (total_disponibles - other_days).max(0);
            let active_limit = if items[i].dias_activos_mes > 0 { items[i].dias_activos_mes } else { total_disponibles };
            items[i].tope_max = active_limit.min(available_for_this);
            let has_err = items[i].dias_obra > items[i].tope_max
                || items[i].dias_obra < 0
                || total_selected_days > total_disponibles;
            items[i].has_error = has_err;
            if has_err {
                any_error = true;
            }
        } else {
            let available_for_this = (total_disponibles - total_selected_days).max(0);
            let active_limit = if items[i].dias_activos_mes > 0 { items[i].dias_activos_mes } else { total_disponibles };
            items[i].tope_max = active_limit.min(available_for_this);
            items[i].has_error = false;
        }
    }

    any_error
}

/// Extrae la lista de empleados únicos del conjunto de datos actual y sincroniza con el estado de sesión.
pub fn update_unique_empleados_catalog(state: &mut ReporteSessionState, app_data: &AppData) {
    let mut emp_map: HashMap<String, (String, String, String, usize)> = HashMap::new();
    for r in &state.all_rows {
        let key = if r.id_empleado != Uuid::nil() {
            r.id_empleado.to_string()
        } else if !r.rfc.trim().is_empty() {
            r.rfc.trim().to_uppercase()
        } else {
            r.nombre_trabajador.trim().to_uppercase()
        };

        let entry = emp_map.entry(key).or_insert_with(|| {
            (
                r.nombre_trabajador.clone(),
                r.rfc.clone(),
                r.nss.clone(),
                0,
            )
        });
        entry.3 += 1;
    }

    let mut list = Vec::new();
    for (id_key, (nombre, rfc, nss, count)) in emp_map {
        let is_sel = state.selected_unique_emp_ids.contains(&id_key);
        list.push(ReporteUniqueEmpleadoItem {
            id: id_key.into(),
            nombre: nombre.into(),
            rfc: rfc.into(),
            nss: nss.into(),
            selected: is_sel,
            periodos_count: count as i32,
        });
    }

    list.sort_by(|a, b| a.nombre.to_lowercase().cmp(&b.nombre.to_lowercase()));
    state.unique_empleados = list;
    sync_unique_empleados_to_ui(state, app_data);
}

/// Sincroniza el listado y métricas del filtro de empleados únicos con la interfaz Slint.
pub fn sync_unique_empleados_to_ui(state: &ReporteSessionState, app_data: &AppData) {
    let q = state.unique_emp_search_query.trim().to_lowercase();
    let filtered: Vec<ReporteUniqueEmpleadoItem> = state.unique_empleados.iter()
        .filter(|e| {
            if q.is_empty() {
                true
            } else {
                e.nombre.to_lowercase().contains(&q)
                    || e.rfc.to_lowercase().contains(&q)
                    || e.nss.to_lowercase().contains(&q)
            }
        })
        .cloned()
        .collect();

    let sel_count = state.selected_unique_emp_ids.len() as i32;
    let is_active = sel_count > 0;

    app_data.set_reporte_unique_empleados(ModelRc::new(VecModel::from(state.unique_empleados.clone())));
    app_data.set_reporte_unique_empleados_filtered(ModelRc::new(VecModel::from(filtered)));
    app_data.set_reporte_unique_empleados_selected_count(sel_count);
    app_data.set_reporte_unique_empleados_filter_active(is_active);
}

/// Algoritmo reactivo de redistribución de días al editar manualmente una obra (Regla Amaya Aranda):
/// Si el usuario reduce días en una obra (ej. de 7 a 5), los días liberados (2) se suman automáticamente
/// a la otra obra seleccionada (ej. de 9 a 11).
pub fn redistribute_edit_obras_days(
    items: &mut [ReporteEditObraItem],
    total_disponibles: i32,
    target_id: &str,
    new_days_raw: i32,
) {
    let target_idx = match items.iter().position(|it| it.id == target_id) {
        Some(idx) => idx,
        None => return,
    };

    let item = &items[target_idx];
    if !item.selected {
        return;
    }

    let dias_activos = if item.dias_activos_mes > 0 { item.dias_activos_mes } else { total_disponibles };
    let target_val = new_days_raw.clamp(0, dias_activos.min(total_disponibles));
    let old_val = item.dias_obra;
    let delta = target_val - old_val;

    if delta < 0 {
        // Se liberaron días (-delta). Asignar los días liberados a las otras obras seleccionadas.
        items[target_idx].dias_obra = target_val;
        items[target_idx].dias_obra_str = target_val.to_string().into();
        let mut freed = -delta;

        for (idx, other) in items.iter_mut().enumerate() {
            if idx != target_idx && other.selected && freed > 0 {
                let cap_limit = if other.dias_activos_mes > 0 { other.dias_activos_mes } else { total_disponibles };
                let capacity = (cap_limit - other.dias_obra).max(0);
                let to_add = freed.min(capacity);
                other.dias_obra += to_add;
                other.dias_obra_str = other.dias_obra.to_string().into();
                freed -= to_add;
            }
        }
    } else if delta > 0 {
        // El usuario incrementó días. Tomar primero de los días libres (no asignados).
        let current_allocated: i32 = items.iter().filter(|it| it.selected).map(|it| it.dias_obra).sum();
        let unallocated = (total_disponibles - current_allocated).max(0);

        if delta <= unallocated {
            items[target_idx].dias_obra = target_val;
            items[target_idx].dias_obra_str = target_val.to_string().into();
        } else {
            let mut needed = delta - unallocated;
            for (idx, other) in items.iter_mut().enumerate() {
                if idx != target_idx && other.selected && needed > 0 {
                    let can_take = other.dias_obra.min(needed);
                    other.dias_obra -= can_take;
                    other.dias_obra_str = other.dias_obra.to_string().into();
                    needed -= can_take;
                }
            }
            let final_target = target_val - needed;
            items[target_idx].dias_obra = final_target;
            items[target_idx].dias_obra_str = final_target.to_string().into();
        }
    } else {
        items[target_idx].dias_obra = target_val;
        items[target_idx].dias_obra_str = target_val.to_string().into();
    }

    recalc_edit_obras_topes(items, total_disponibles);
}

/// Alternar selección (checkbox) de un registro de obra en el modal individual.
pub fn toggle_edit_obra(
    items: &mut [ReporteEditObraItem],
    total_disponibles: i32,
    target_id: &str,
) {
    let target_idx = match items.iter().position(|it| it.id == target_id) {
        Some(idx) => idx,
        None => return,
    };

    let new_selected = !items[target_idx].selected;
    items[target_idx].selected = new_selected;

    if new_selected {
        // Al palomear: tomar de los días no asignados hasta el tope activo de la obra
        let current_allocated: i32 = items.iter().filter(|it| it.selected).map(|it| it.dias_obra).sum();
        let unallocated = (total_disponibles - current_allocated).max(0);
        let max_possible = if items[target_idx].dias_activos_mes > 0 {
            items[target_idx].dias_activos_mes
        } else {
            total_disponibles
        };
        let assign = unallocated.min(max_possible);
        items[target_idx].dias_obra = assign;
        items[target_idx].dias_obra_str = assign.to_string().into();
    } else {
        // Al despalomear: liberar días y redistribuirlos a las obras seleccionadas restantes
        let freed = items[target_idx].dias_obra;
        items[target_idx].dias_obra = 0;
        items[target_idx].dias_obra_str = "0".into();

        if freed > 0 {
            let mut remaining_freed = freed;
            for (idx, other) in items.iter_mut().enumerate() {
                if idx != target_idx && other.selected && remaining_freed > 0 {
                    let cap_limit = if other.dias_activos_mes > 0 { other.dias_activos_mes } else { total_disponibles };
                    let capacity = (cap_limit - other.dias_obra).max(0);
                    let to_add = remaining_freed.min(capacity);
                    other.dias_obra += to_add;
                    other.dias_obra_str = other.dias_obra.to_string().into();
                    remaining_freed -= to_add;
                }
            }
        }
    }

    recalc_edit_obras_topes(items, total_disponibles);
}

/// Sincroniza bidireccionalmente la lista completa del catálogo y la lista superior de obras seleccionadas.
pub fn sync_reporte_edit_modal_models(app_data: &AppData, items: &[ReporteEditObraItem], total_disponibles: i32) {
    let asignados: i32 = items.iter().filter(|it| it.selected).map(|it| it.dias_obra).sum();
    let restantes = (total_disponibles - asignados).max(0);
    let sel_count = items.iter().filter(|it| it.selected).count() as i32;

    let selected_only: Vec<ReporteEditObraItem> = items.iter().filter(|it| it.selected).cloned().collect();
    app_data.set_reporte_edit_selected_obras(ModelRc::new(VecModel::from(selected_only)));
    app_data.set_reporte_edit_obras_list(ModelRc::new(VecModel::from(items.to_vec())));
    app_data.set_reporte_edit_dias_asignados_sum(asignados);
    app_data.set_reporte_edit_dias_restantes(restantes);
    app_data.set_reporte_edit_selected_count(sel_count);
}

/// Algoritmo reactivo inline en la tabla del Modal Masivo:
/// Al editar directamente la celda de D. Obra de un trabajador en la tabla masiva,
/// busca las filas hermanas del mismo trabajador (mismo id_parent) y redistribuye los días
/// automáticamente para mantener el tope total disponible y reflejar en tiempo real los cambios.
pub fn redistribute_batch_workers_days(
    workers: &mut [ReporteBatchWorkerItem],
    target_id: &str,
    new_days_raw: i32,
) {
    let target_idx = match workers.iter().position(|w| w.id == target_id) {
        Some(idx) => idx,
        None => return,
    };

    let parent_id = workers[target_idx].id_parent.clone();
    let total_disponibles = workers[target_idx].total_disponibles;

    let sibling_indices: Vec<usize> = workers.iter().enumerate()
        .filter(|(_, w)| w.id_parent == parent_id)
        .map(|(i, _)| i)
        .collect();

    if sibling_indices.len() <= 1 {
        let target_val = new_days_raw.clamp(0, total_disponibles);
        workers[target_idx].dias_obra = target_val;
        workers[target_idx].dias_obra_str = target_val.to_string().into();
        workers[target_idx].tope_max = total_disponibles;
        workers[target_idx].has_error = target_val > total_disponibles;
        return;
    }

    let old_val = workers[target_idx].dias_obra;
    let target_val = new_days_raw.clamp(0, total_disponibles);
    let delta = target_val - old_val;

    if delta < 0 {
        workers[target_idx].dias_obra = target_val;
        workers[target_idx].dias_obra_str = target_val.to_string().into();
        let mut freed = -delta;

        for &sib_idx in &sibling_indices {
            if sib_idx != target_idx && freed > 0 {
                workers[sib_idx].dias_obra += freed;
                workers[sib_idx].dias_obra_str = workers[sib_idx].dias_obra.to_string().into();
                freed = 0;
            }
        }
    } else if delta > 0 {
        let mut needed = delta;
        for &sib_idx in &sibling_indices {
            if sib_idx != target_idx && needed > 0 {
                let can_take = workers[sib_idx].dias_obra.min(needed);
                workers[sib_idx].dias_obra -= can_take;
                workers[sib_idx].dias_obra_str = workers[sib_idx].dias_obra.to_string().into();
                needed -= can_take;
            }
        }
        let final_target = target_val - needed;
        workers[target_idx].dias_obra = final_target;
        workers[target_idx].dias_obra_str = final_target.to_string().into();
    } else {
        workers[target_idx].dias_obra = target_val;
        workers[target_idx].dias_obra_str = target_val.to_string().into();
    }

    let sum_siblings: i32 = sibling_indices.iter().map(|&idx| workers[idx].dias_obra).sum();
    for &sib_idx in &sibling_indices {
        let other_sum = sum_siblings - workers[sib_idx].dias_obra;
        workers[sib_idx].tope_max = (total_disponibles - other_sum).max(0);
        workers[sib_idx].has_error = sum_siblings > total_disponibles;
    }
}

pub fn apply_reporte_filtering(state: &mut ReporteSessionState) {
    // Filtrar obras_options: Solo conservar obras creadas por el usuario (creado_usuario = true)
    // o aquellas que se encuentran en la tabla de liquidaciones del reporte.
    state.obras_options.retain(|o| {
        o.creado_usuario
            || state.all_rows.iter().any(|r| {
                r.id_registro_obra == Some(o.id)
                    || (!r.obra_numero.trim().is_empty()
                        && r.obra_numero.trim().eq_ignore_ascii_case(o.numero.trim()))
            })
    });

    let q = state.search_query.trim().to_lowercase();
    let filter_obra_id = if state.filter_obra_idx > 0 && state.filter_obra_idx <= state.obras_options.len() {
        Some(state.obras_options[state.filter_obra_idx - 1].id)
    } else {
        None
    };

    let filter_periodo = if state.filter_periodo_idx > 0 && state.filter_periodo_idx < state.periodo_values.len() {
        let val = &state.periodo_values[state.filter_periodo_idx];
        if val.is_empty() {
            None
        } else {
            Some(val.clone())
        }
    } else if state.filter_periodo_idx > 0 && state.filter_periodo_idx < state.periodo_options.len() {
        Some(state.periodo_options[state.filter_periodo_idx].clone())
    } else {
        None
    };

    state.filtered_rows = state
        .all_rows
        .iter()
        .filter(|r| {
            // Filtro de búsqueda textual
            if !q.is_empty() {
                let matches_name = r.nombre_trabajador.to_lowercase().contains(&q);
                let matches_rfc = r.rfc.to_lowercase().contains(&q);
                let matches_curp = r.curp.to_lowercase().contains(&q);
                let matches_nss = r.nss.to_lowercase().contains(&q);
                let matches_periodo = r.periodo_str.to_lowercase().contains(&q);
                let matches_mes = r.mes_periodo.to_lowercase().contains(&q);
                let matches_anio = r.anio_periodo.to_string().contains(&q);
                let matches_rp = r.registro_patronal.to_lowercase().contains(&q);
                let matches_obra = r.obra_numero.to_lowercase().contains(&q);

                if !matches_name
                    && !matches_rfc
                    && !matches_curp
                    && !matches_nss
                    && !matches_periodo
                    && !matches_mes
                    && !matches_anio
                    && !matches_rp
                    && !matches_obra
                {
                    return false;
                }
            }

            // Filtro de Obra
            if let Some(obra_id) = filter_obra_id {
                if r.id_registro_obra != Some(obra_id) {
                    return false;
                }
            }

            // Filtro de Periodo
            if let Some(ref p) = filter_periodo {
                if &r.periodo_str != p {
                    return false;
                }
            }

            // Filtro de Estado
            if state.filter_estado_idx == 1 {
                // Solo Asignados
                if r.id_registro_obra.is_none() {
                    return false;
                }
            } else if state.filter_estado_idx == 2 {
                // Solo Sin Asignar
                if r.id_registro_obra.is_some() {
                    return false;
                }
            }

            // Filtro por empleados seleccionados
            if state.emp_filter_active {
                let clean_rfc = get_clean_non_generic_rfc(&r.rfc);
                let is_target = state.emp_filter_worker_ids.contains(&r.id_empleado)
                    || clean_rfc.map_or(false, |clean| state.emp_filter_worker_rfcs.contains(&clean));
                if !is_target {
                    return false;
                }

                // Filtrar por meses
                if !state.emp_filter_all_months {
                    if !state.emp_filter_selected_months.contains(&r.periodo_str) {
                        return false;
                    }
                }

                // Filtrar por obras
                if !state.emp_filter_all_obras {
                    let obra_key = r.id_registro_obra.map(|u| u.to_string()).unwrap_or_else(|| "sin_asignar".to_string());
                    if !state.emp_filter_selected_obras.contains(&obra_key) {
                        return false;
                    }
                }
            }

            // Filtro Multi-Selección de Empleados Únicos (Menú Principal)
            if !state.selected_unique_emp_ids.is_empty() {
                let key = if r.id_empleado != Uuid::nil() {
                    r.id_empleado.to_string()
                } else if !r.rfc.trim().is_empty() {
                    r.rfc.trim().to_uppercase()
                } else {
                    r.nombre_trabajador.trim().to_uppercase()
                };

                let clean_rfc = get_clean_non_generic_rfc(&r.rfc);
                let is_match = state.selected_unique_emp_ids.contains(&key)
                    || (r.id_empleado != Uuid::nil() && state.selected_unique_emp_ids.contains(&r.id_empleado.to_string()))
                    || clean_rfc.map(|crfc| state.selected_unique_emp_ids.contains(&crfc)).unwrap_or(false);

                if !is_match {
                    return false;
                }
            }

            true
        })
        .cloned()
        .collect();

    // Ordenamiento multi-criterio
    let has_any_sort = state.sort_by_obra || state.sort_by_anio || state.sort_by_mes || state.sort_by_rfc;
    if has_any_sort {
        state.filtered_rows.sort_by(|a, b| {
            if state.sort_by_obra {
                let cmp = if state.sort_obra_asc {
                    a.obra_numero.cmp(&b.obra_numero).then_with(|| a.obra_label.cmp(&b.obra_label))
                } else {
                    b.obra_numero.cmp(&a.obra_numero).then_with(|| b.obra_label.cmp(&a.obra_label))
                };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }

            if state.sort_by_anio {
                let cmp = if state.sort_anio_asc {
                    a.anio_periodo.cmp(&b.anio_periodo)
                } else {
                    b.anio_periodo.cmp(&a.anio_periodo)
                };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }

            if state.sort_by_mes {
                let ma = month_to_num(&a.mes_periodo);
                let mb = month_to_num(&b.mes_periodo);
                let cmp = if state.sort_mes_asc {
                    ma.cmp(&mb)
                } else {
                    mb.cmp(&ma)
                };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }

            if state.sort_by_rfc {
                let cmp = if state.sort_rfc_asc {
                    a.rfc.cmp(&b.rfc).then_with(|| a.curp.cmp(&b.curp))
                } else {
                    b.rfc.cmp(&a.rfc).then_with(|| b.curp.cmp(&a.curp))
                };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }

            std::cmp::Ordering::Equal
        });
    }

    let total_pages = if state.filtered_rows.is_empty() {
        1
    } else {
        (state.filtered_rows.len() + state.page_size - 1) / state.page_size
    };
    if state.current_page > total_pages || state.current_page == 0 {
        state.current_page = 1;
    }
}

pub fn render_reporte_ui(ui: &MainWindow, state: &ReporteSessionState) {
    let app_data = ui.global::<AppData>();

    // 1. Obras opciones
    let mut obras_vec = Vec::with_capacity(state.obras_options.len());
    let mut filter_obras_labels: Vec<SharedString> = Vec::with_capacity(state.obras_options.len() + 1);
    let mut batch_obras_labels: Vec<SharedString> = Vec::with_capacity(state.obras_options.len() + 1);

    filter_obras_labels.push("Todas las obras".into());
    batch_obras_labels.push("-- Seleccionar Obra --".into());

    for o in &state.obras_options {
        obras_vec.push(ReporteObraItem {
            id: o.id.to_string().into(),
            numero: o.numero.clone().into(),
            label: o.label.clone().into(),
        });
        filter_obras_labels.push(o.label.clone().into());
        batch_obras_labels.push(o.label.clone().into());
    }
    app_data.set_reporte_obras_options(ModelRc::new(VecModel::from(obras_vec.clone())));
    app_data.set_reporte_picker_filtered_obras(ModelRc::new(VecModel::from(obras_vec)));
    app_data.set_reporte_obras_filter_labels(ModelRc::new(VecModel::from(filter_obras_labels)));
    app_data.set_reporte_obras_batch_labels(ModelRc::new(VecModel::from(batch_obras_labels)));

    // 1.1 Obras creadas por el usuario para visualización rápida de vigencias y conteo de empleados
    let mut obras_usuario_vec = Vec::new();
    let current_filtered_obra_id = if state.filter_obra_idx > 0 && state.filter_obra_idx <= state.obras_options.len() {
        Some(state.obras_options[state.filter_obra_idx - 1].id)
    } else {
        None
    };

    for o in state.obras_options.iter().filter(|o| o.creado_usuario) {
        let fi_str = o.fecha_inicio.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "--/--/----".to_string());
        let ff_str = o.fecha_fin.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "--/--/----".to_string());
        let vigencia = format!("{} al {}", fi_str, ff_str);

        // Contar cuántos empleados únicos (por NSS distinto) tienen asignado actualmente este registro de obra
        let mut emp_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut row_count = 0;
        for r in &state.all_rows {
            if r.id_registro_obra == Some(o.id) {
                let nss_key = if !r.nss.trim().is_empty() { r.nss.trim().to_string() } else { r.id_empleado.to_string() };
                emp_set.insert(nss_key);
                row_count += 1;
            }
        }
        let emp_count = emp_set.len();
        let emp_count_str = if emp_count == 1 {
            if row_count > 1 {
                format!("1 emp. único ({} reg)", row_count)
            } else {
                "1 emp. único".to_string()
            }
        } else {
            if row_count > emp_count {
                format!("{} emps. únicos ({} reg)", emp_count, row_count)
            } else {
                format!("{} emps. únicos", emp_count)
            }
        };

        let is_selected = current_filtered_obra_id == Some(o.id);

        obras_usuario_vec.push(ReporteObraUsuarioItem {
            id: o.id.to_string().into(),
            numero: o.numero.clone().into(),
            tipo_obra: o.tipo_obra.clone().into(),
            fecha_inicio: fi_str.into(),
            fecha_fin: ff_str.into(),
            vigencia_str: vigencia.into(),
            empleados_count: emp_count as i32,
            empleados_count_str: emp_count_str.into(),
            selected: is_selected,
        });
    }
    let has_obras_usuario = !obras_usuario_vec.is_empty();
    app_data.set_reporte_has_obras_usuario(has_obras_usuario);
    app_data.set_reporte_obras_usuario(ModelRc::new(VecModel::from(obras_usuario_vec)));
    app_data.set_reporte_drawer_obras_open(state.is_drawer_open);

    // 2. Opciones de periodo
    let mut periodo_vec = Vec::with_capacity(state.periodo_options.len());
    for p in &state.periodo_options {
        periodo_vec.push(p.as_str().into());
    }
    app_data.set_reporte_periodo_options(ModelRc::new(VecModel::from(periodo_vec)));

    // 3. Subtítulo dinámico con obras y periodos
    let total_procesados = state.all_rows.len();
    let obras_str = if state.obras_options.is_empty() {
        "Sin obras registradas".to_string()
    } else {
        let nums: Vec<String> = state.obras_options.iter().map(|o| o.numero.clone()).collect();
        nums.join(", ")
    };
    let subtitle = if obras_str == "Sin obras registradas" {
        format!("Liquidaciones de la revisión activa. Total: {} registros cargados.", total_procesados)
    } else {
        format!("Liquidaciones de la revisión activa [{}]. Total: {} registros cargados.", obras_str, total_procesados)
    };
    app_data.set_reporte_subtitle(subtitle.into());

    // 4. KPIs globales
    let obras_count = state.obras_options.iter().filter(|o| o.creado_usuario).count();

    let mut total_cuotas_orig = 0.0;
    let mut total_prorrateado = 0.0;

    for r in &state.all_rows {
        total_cuotas_orig += r.subtotal_orig;
        total_prorrateado += r.subtotal;
    }

    let diferencia = total_prorrateado - total_cuotas_orig;
    let dif_pct = if total_cuotas_orig > 0.0 {
        (diferencia / total_cuotas_orig) * 100.0
    } else {
        0.0
    };

    app_data.set_reporte_kpi_total_registros(total_procesados as i32);
    app_data.set_reporte_kpi_total_registros_str(format!("{} filas (100 en pág)", total_procesados).into());

    app_data.set_reporte_kpi_obras_count(obras_count as i32);
    app_data.set_reporte_kpi_obras_homologadas_str(format!("{} Homologadas", obras_count).into());

    app_data.set_reporte_kpi_subtotal_imss_str(format_curr(total_cuotas_orig).into());
    app_data.set_reporte_kpi_prorrateado_str(format_curr(total_prorrateado).into());

    let dif_str = if diferencia < 0.0 {
        format!("{} ({:.1}%)", format_curr(diferencia), dif_pct)
    } else if diferencia > 0.0 {
        format!("+{} (+{:.1}%)", format_curr(diferencia), dif_pct)
    } else {
        "$0.00 (0.0%)".to_string()
    };
    app_data.set_reporte_kpi_diferencia_str(dif_str.into());

    // 5. Paginación
    let total_rows = state.filtered_rows.len();
    let page_size = if state.page_size == 0 { 100 } else { state.page_size };
    let total_pages = if total_rows == 0 { 1 } else { (total_rows + page_size - 1) / page_size };
    let cur_page = state.current_page.clamp(1, total_pages);

    let start_idx = if total_rows == 0 { 0 } else { (cur_page - 1) * page_size };
    let end_idx = (start_idx + page_size).min(total_rows);

    let start_row_num = if total_rows == 0 { 0 } else { start_idx + 1 };
    let end_row_num = end_idx;

    app_data.set_reporte_page_current(cur_page as i32);
    app_data.set_reporte_page_total(total_pages as i32);
    app_data.set_reporte_page_start_row(start_row_num as i32);
    app_data.set_reporte_page_end_row(end_row_num as i32);
    app_data.set_reporte_total_filtered(total_rows as i32);

    // 6. Filas paginadas para la tabla Slint
    let mut page_rows = Vec::with_capacity(end_idx - start_idx);
    let mut non_baja_in_page = 0;
    let mut selected_non_baja_in_page = 0;

    for (page_offset, r) in state.filtered_rows[start_idx..end_idx].iter().enumerate() {
        let is_baja = r.clave.trim().eq_ignore_ascii_case("baja");
        let is_selected = state.selected_ids.contains(&r.id_reporte_liq);
        if !is_baja {
            non_baja_in_page += 1;
            if is_selected {
                selected_non_baja_in_page += 1;
            }
        }

        let is_assigned = r.id_registro_obra.is_some();
        let rfc_curp_str = build_rfc_curp_label(&r.rfc, &r.curp);

        let (row_bg_color, row_hover_color, row_accent_color) = if let Some(oid) = r.id_registro_obra {
            let color_idx = state.obras_options.iter().position(|o| o.id == oid).unwrap_or(0);
            get_obra_colors(color_idx)
        } else {
            (
                slint::Color::from_argb_u8(255, 255, 255, 255),
                slint::Color::from_argb_u8(255, 248, 250, 252),
                slint::Color::from_argb_u8(0, 0, 0, 0),
            )
        };

        let has_prorrateo = is_assigned && (
            (r.subtotal - r.subtotal_orig).abs() > 0.009
            || (r.dias_obra != r.dias_cedula)
        );

        let fmt_days_or_dash = |v: f64| {
            if v > 0.001 {
                if (v.fract()).abs() < 0.001 {
                    format!("{:.0}", v)
                } else {
                    format!("{:.2}", v)
                }
            } else {
                "-".to_string()
            }
        };

        let sibling_rows: Vec<_> = state.all_rows.iter().filter(|x| x.id_parent_reporte_liq == r.id_parent_reporte_liq).collect();
        let parent_rows_count = sibling_rows.len();
        let can_delete_split = parent_rows_count > 1;
        let has_multi_obra = parent_rows_count > 1;

        let sibling_idx = sibling_rows.iter().position(|x| x.id_reporte_liq == r.id_reporte_liq).unwrap_or(0) + 1;
        let multi_obra_badge = if has_multi_obra {
            format!("Obra {}/{}", sibling_idx, parent_rows_count)
        } else {
            String::new()
        };

        let total_dias_obras: i32 = sibling_rows.iter().map(|x| x.dias_obra as i32).sum();

        let vinculaciones_resumen = sibling_rows.iter().map(|x| {
            let num = if x.obra_numero.trim().is_empty() {
                "S/O".to_string()
            } else {
                x.obra_numero.trim().to_string()
            };
            format!("{}-{}", num, x.dias_obra)
        }).collect::<Vec<_>>().join(" ");

        let dias_mismatch = if is_baja {
            r.dias_obra > 0 || r.subtotal > 0.001
        } else if is_assigned {
            total_dias_obras != (r.dias_cedula as i32) || r.dias_obra < 0
        } else {
            false
        };

        let is_expanded = state.expanded_row_id == Some(r.id_reporte_liq);
        let movimientos_resumen = if is_expanded {
            let mov_rows: Vec<&ReporteLiquidacionRowDb> = state.all_rows.iter().filter(|x| {
                x.id_empleado == r.id_empleado
                    && x.anio_periodo == r.anio_periodo
                    && x.mes_periodo.trim().eq_ignore_ascii_case(r.mes_periodo.trim())
            }).collect();

            let mut lines = Vec::new();
            for (m_idx, m) in mov_rows.iter().enumerate() {
                let clave_str = if m.clave.trim().is_empty() { "NORMAL / SIN MOVIMIENTO".to_string() } else { m.clave.trim().to_uppercase() };
                let fecha_str = if m.fecha_str.trim().is_empty() || m.fecha_str == "-" { "Sin fecha".to_string() } else { m.fecha_str.clone() };
                let sdi_str = if m.sdi > 0.001 { format_curr(m.sdi) } else { "$0.00".to_string() };
                let inc_sum = m.lic + m.inc + m.aus;
                let inc_str = if inc_sum > 0.001 {
                    format!(" | Incidencias: LIC: {:.0}, INC: {:.0}, AUS: {:.0} (Descontados: {:.0} días)", m.lic, m.inc, m.aus, inc_sum)
                } else {
                    " | Sin incidencias (0 días LIC/INC/AUS)".to_string()
                };
                let obra_info = if let Some(_) = m.id_registro_obra {
                    format!(" | Obra: {} ({} días obra)", m.obra_label, m.dias_obra)
                } else {
                    " | Obra: Sin asignar".to_string()
                };
                lines.push(format!(
                    "• Tramo {}: [{}] - Fecha: {} | SDI: {} | Días Cédula: {}{}{}",
                    m_idx + 1, clave_str, fecha_str, sdi_str, m.dias_cedula, inc_str, obra_info
                ));
            }
            if lines.is_empty() {
                format!("• [NORMAL] Sin movimientos registrados en el mes de {}. Días cotizados: {}.", r.mes_periodo, r.dias_cedula)
            } else {
                lines.join("\n")
            }
        } else {
            String::new()
        };

        page_rows.push(ReporteLiquidacionRow {
            selected: is_selected,
            can_delete_split,
            is_expanded,
            movimientos_resumen: movimientos_resumen.into(),
            is_baja,
            idx: (start_idx + page_offset + 1) as i32,
            id_reporte_liq: r.id_reporte_liq.to_string().into(),
            id_liquidacion: r.id_liquidacion.to_string().into(),
            periodo: r.periodo_str.as_str().into(),
            nss: r.nss.as_str().into(),
            rfc_curp: rfc_curp_str.into(),
            nombre_trabajador: r.nombre_trabajador.as_str().into(),
            pensionado: r.pensionado.as_str().into(),
            registro_patronal: r.registro_patronal.as_str().into(),
            id_registro_obra: r.id_registro_obra.map(|u| u.to_string()).unwrap_or_default().into(),
            obra_numero: r.obra_numero.as_str().into(),
            obra_label: r.obra_label.as_str().into(),
            vinculaciones_resumen: vinculaciones_resumen.into(),
            multi_obra_badge: multi_obra_badge.into(),
            has_multi_obra,
            dias_mismatch,
            total_dias_obras,
            is_assigned,
            dias_cedula: r.dias_cedula as i32,
            dias_obra: r.dias_obra as i32,
            clave: if r.clave.trim().is_empty() { "-".into() } else { r.clave.trim().into() },
            fecha: r.fecha_str.as_str().into(),
            sdi_str: if r.sdi > 0.001 { format_curr(r.sdi).into() } else { "-".into() },
            lic_str: fmt_days_or_dash(r.lic).into(),
            inc_str: fmt_days_or_dash(r.inc).into(),
            aus_str: fmt_days_or_dash(r.aus).into(),
            has_incidencias: r.lic > 0.001 || r.inc > 0.001 || r.aus > 0.001,
            lic_val: r.lic as f32,
            inc_val: r.inc as f32,
            aus_val: r.aus as f32,
            row_bg_color,
            row_hover_color,
            row_accent_color,
            has_prorrateo,
            total_prorrateado_str: format_curr(r.subtotal).into(),
            cf_str: format_curr(r.cf).into(),
            exc_pat_str: format_curr(r.exc_pat).into(),
            exc_obr_str: format_curr(r.exc_obr).into(),
            pd_pat_str: format_curr(r.pd_pat).into(),
            pd_obr_str: format_curr(r.pd_obr).into(),
            gmp_pat_str: format_curr(r.gmp_pat).into(),
            gmp_obr_str: format_curr(r.gmp_obr).into(),
            rt_str: format_curr(r.rt).into(),
            iv_pat_str: format_curr(r.iv_pat).into(),
            iv_obr_str: format_curr(r.iv_obr).into(),
            gps_str: format_curr(r.gps).into(),
            patronal_str: format_curr(r.patronal).into(),
            obrera_str: format_curr(r.obrera).into(),
            subtotal_str: format_curr(r.subtotal).into(),
            cf_orig_str: format_curr(r.cf_orig).into(),
            exc_pat_orig_str: format_curr(r.exc_pat_orig).into(),
            exc_obr_orig_str: format_curr(r.exc_obr_orig).into(),
            pd_pat_orig_str: format_curr(r.pd_pat_orig).into(),
            pd_obr_orig_str: format_curr(r.pd_obr_orig).into(),
            gmp_pat_orig_str: format_curr(r.gmp_pat_orig).into(),
            gmp_obr_orig_str: format_curr(r.gmp_obr_orig).into(),
            rt_orig_str: format_curr(r.rt_orig).into(),
            iv_pat_orig_str: format_curr(r.iv_pat_orig).into(),
            iv_obr_orig_str: format_curr(r.iv_obr_orig).into(),
            gps_orig_str: format_curr(r.gps_orig).into(),
            patronal_orig_str: format_curr(r.patronal_orig).into(),
            obrera_orig_str: format_curr(r.obrera_orig).into(),
            subtotal_orig_str: format_curr(r.subtotal_orig).into(),
        });
    }

    let all_page_selected = non_baja_in_page > 0 && selected_non_baja_in_page == non_baja_in_page;

    // Elementos seleccionados para el combobox desplegable
    let mut selected_items = Vec::new();
    for (i, r) in state.all_rows.iter().enumerate() {
        if state.selected_ids.contains(&r.id_reporte_liq) {
            selected_items.push(ReporteSelectedItem {
                id: r.id_reporte_liq.to_string().into(),
                idx: (i + 1) as i32,
                nombre_trabajador: r.nombre_trabajador.clone().into(),
                pensionado: r.pensionado.clone().into(),
                nss: r.nss.clone().into(),
                periodo: r.periodo_str.clone().into(),
                obra_label: if r.obra_label.trim().is_empty() {
                    "Sin asignar".into()
                } else {
                    r.obra_label.clone().into()
                },
            });
        }
    }

    app_data.set_reporte_liquidaciones_list(ModelRc::new(VecModel::from(page_rows)));
    app_data.set_reporte_selected_count(state.selected_ids.len() as i32);
    app_data.set_reporte_selected_items(ModelRc::new(VecModel::from(selected_items)));
    app_data.set_reporte_all_filter_selected(all_page_selected);
    app_data.set_reporte_filter_obra_idx(state.filter_obra_idx as i32);
    app_data.set_reporte_filter_periodo_idx(state.filter_periodo_idx as i32);
    app_data.set_reporte_filter_estado_idx(state.filter_estado_idx as i32);
    app_data.set_reporte_search_query(state.search_query.as_str().into());

    // Sincronizar estado de ordenamiento multi-criterio
    app_data.set_reporte_sort_by_obra(state.sort_by_obra);
    app_data.set_reporte_sort_obra_asc(state.sort_obra_asc);
    app_data.set_reporte_sort_by_anio(state.sort_by_anio);
    app_data.set_reporte_sort_anio_asc(state.sort_anio_asc);
    app_data.set_reporte_sort_by_mes(state.sort_by_mes);
    app_data.set_reporte_sort_mes_asc(state.sort_mes_asc);
    app_data.set_reporte_sort_by_rfc(state.sort_by_rfc);
    app_data.set_reporte_sort_rfc_asc(state.sort_rfc_asc);

    let mut active_sort_count = 0;
    if state.sort_by_obra { active_sort_count += 1; }
    if state.sort_by_anio { active_sort_count += 1; }
    if state.sort_by_mes { active_sort_count += 1; }
    if state.sort_by_rfc { active_sort_count += 1; }
    app_data.set_reporte_active_sort_count(active_sort_count);

    // Sincronizar estado de filtro de empleados seleccionados
    app_data.set_reporte_emp_modal_worker_count(state.emp_modal_worker_count as i32);
    app_data.set_reporte_emp_modal_worker_names_str(state.emp_modal_worker_names_str.as_str().into());
    app_data.set_reporte_emp_modal_total_occurrences(state.emp_modal_total_occurrences as i32);
    app_data.set_reporte_emp_filter_all_months(state.emp_filter_all_months);
    app_data.set_reporte_emp_filter_all_obras(state.emp_filter_all_obras);
    app_data.set_reporte_emp_filter_active(state.emp_filter_active);

    let meses_models: Vec<ReporteFilterOptionItem> = state.emp_modal_meses.iter().map(|(id, lbl, count, sel)| {
        ReporteFilterOptionItem {
            id: id.as_str().into(),
            label: lbl.as_str().into(),
            count: *count as i32,
            selected: *sel,
        }
    }).collect();
    app_data.set_reporte_emp_modal_meses(ModelRc::new(VecModel::from(meses_models)));

    let obras_models: Vec<ReporteFilterOptionItem> = state.emp_modal_obras.iter().map(|(id, lbl, count, sel)| {
        ReporteFilterOptionItem {
            id: id.as_str().into(),
            label: lbl.as_str().into(),
            count: *count as i32,
            selected: *sel,
        }
    }).collect();
    app_data.set_reporte_emp_modal_obras(ModelRc::new(VecModel::from(obras_models)));

    // 7. Totales para el Pie de Tabla Fijo (calculados sobre filtered_rows)
    let mut total_dias_ced: i32 = 0;
    let mut total_dias_obr: i32 = 0;
    let mut total_lic: f64 = 0.0;
    let mut total_inc: f64 = 0.0;
    let mut total_aus: f64 = 0.0;
    let mut total_cf: f64 = 0.0;
    let mut total_exc_pat: f64 = 0.0;
    let mut total_exc_obr: f64 = 0.0;
    let mut total_pd_pat: f64 = 0.0;
    let mut total_pd_obr: f64 = 0.0;
    let mut total_gmp_pat: f64 = 0.0;
    let mut total_gmp_obr: f64 = 0.0;
    let mut total_rt: f64 = 0.0;
    let mut total_iv_pat: f64 = 0.0;
    let mut total_iv_obr: f64 = 0.0;
    let mut total_gps: f64 = 0.0;
    let mut total_patronal: f64 = 0.0;
    let mut total_obrera: f64 = 0.0;
    let mut total_subtotal: f64 = 0.0;

    for r in &state.filtered_rows {
        total_dias_ced += r.dias_cedula as i32;
        total_dias_obr += if r.id_registro_obra.is_some() { r.dias_obra as i32 } else { 0 };
        total_lic += r.lic;
        total_inc += r.inc;
        total_aus += r.aus;
        total_cf += r.cf;
        total_exc_pat += r.exc_pat;
        total_exc_obr += r.exc_obr;
        total_pd_pat += r.pd_pat;
        total_pd_obr += r.pd_obr;
        total_gmp_pat += r.gmp_pat;
        total_gmp_obr += r.gmp_obr;
        total_rt += r.rt;
        total_iv_pat += r.iv_pat;
        total_iv_obr += r.iv_obr;
        total_gps += r.gps;
        total_patronal += r.patronal;
        total_obrera += r.obrera;
        total_subtotal += r.subtotal;
    }

    let fmt_days_or_dash = |v: f64| {
        if v > 0.001 {
            if (v.fract()).abs() < 0.001 {
                format!("{:.0}", v)
            } else {
                format!("{:.2}", v)
            }
        } else {
            "-".to_string()
        }
    };

    app_data.set_reporte_footer_total_dias_ced_str(total_dias_ced.to_string().into());
    app_data.set_reporte_footer_total_dias_obr_str(total_dias_obr.to_string().into());
    app_data.set_reporte_footer_total_lic_str(fmt_days_or_dash(total_lic).into());
    app_data.set_reporte_footer_total_inc_str(fmt_days_or_dash(total_inc).into());
    app_data.set_reporte_footer_total_aus_str(fmt_days_or_dash(total_aus).into());
    app_data.set_reporte_footer_total_cf_str(format_curr(total_cf).into());
    app_data.set_reporte_footer_total_exc_pat_str(format_curr(total_exc_pat).into());
    app_data.set_reporte_footer_total_exc_obr_str(format_curr(total_exc_obr).into());
    app_data.set_reporte_footer_total_pd_pat_str(format_curr(total_pd_pat).into());
    app_data.set_reporte_footer_total_pd_obr_str(format_curr(total_pd_obr).into());
    app_data.set_reporte_footer_total_gmp_pat_str(format_curr(total_gmp_pat).into());
    app_data.set_reporte_footer_total_gmp_obr_str(format_curr(total_gmp_obr).into());
    app_data.set_reporte_footer_total_rt_str(format_curr(total_rt).into());
    app_data.set_reporte_footer_total_iv_pat_str(format_curr(total_iv_pat).into());
    app_data.set_reporte_footer_total_iv_obr_str(format_curr(total_iv_obr).into());
    app_data.set_reporte_footer_total_gps_str(format_curr(total_gps).into());
    app_data.set_reporte_footer_total_patronal_str(format_curr(total_patronal).into());
    app_data.set_reporte_footer_total_obrera_str(format_curr(total_obrera).into());
    app_data.set_reporte_footer_total_subtotal_str(format_curr(total_subtotal).into());

    // 8. Resumen de Totales por Mes (calculados sobre all_rows cronológicamente)
    let mut mes_totales_map: std::collections::BTreeMap<(i16, u32, String), (usize, i32, i32, f64, f64, f64, f64)> = std::collections::BTreeMap::new();
    for r in &state.all_rows {
        let m_num = month_to_num(&r.mes_periodo);
        let entry = mes_totales_map.entry((r.anio_periodo, m_num, r.periodo_str.clone()))
            .or_insert((0, 0, 0, 0.0, 0.0, 0.0, 0.0));
        entry.0 += 1;
        entry.1 += r.dias_cedula as i32;
        entry.2 += if r.id_registro_obra.is_some() { r.dias_obra as i32 } else { 0 };
        entry.3 += r.cf;
        entry.4 += r.patronal;
        entry.5 += r.obrera;
        entry.6 += r.subtotal;
    }

    let mut totales_mes_vec = Vec::new();
    for ((_anio, _m_num, p_str), (count, d_ced, d_obr, cf, pat, obr, sub)) in mes_totales_map {
        totales_mes_vec.push(ReporteMesTotalItem {
            mes_label: p_str.into(),
            empleados_count: count as i32,
            dias_cedula: d_ced,
            dias_obra: d_obr,
            cf_str: format_curr(cf).into(),
            patronal_str: format_curr(pat).into(),
            obrera_str: format_curr(obr).into(),
            subtotal_str: format_curr(sub).into(),
        });
    }
    app_data.set_reporte_totales_por_mes(ModelRc::new(VecModel::from(totales_mes_vec)));

    // 9. Guarderías por Registro de Obra creado por el usuario (Tope 18%)
    let filter_obra_id = if state.filter_obra_idx > 0 && state.filter_obra_idx <= state.obras_options.len() {
        let opt = &state.obras_options[state.filter_obra_idx - 1];
        if opt.creado_usuario {
            Some(opt.id)
        } else {
            None
        }
    } else {
        None
    };

    let target_rows_for_kpis = if state.filtered_rows.is_empty() {
        &state.all_rows
    } else {
        &state.filtered_rows
    };
    let kpis = compute_guarderias_kpis(target_rows_for_kpis, &state.obras_options, filter_obra_id);
    let guarderias_vec: Vec<ReporteGuarderiaObraItem> = kpis.into_iter().map(|k| {
        ReporteGuarderiaObraItem {
            id_obra: k.id_obra.to_string().into(),
            obra_numero: k.obra_numero.into(),
            obra_label: k.obra_label.into(),
            registros_patronales_str: k.registros_patronales_str.into(),
            total_gps: k.total_gps as f32,
            total_gps_str: format_curr(k.total_gps).into(),
            valor_guarderias: k.valor_guarderias as f32,
            valor_guarderias_str: format_curr(k.valor_guarderias).into(),
            monto_obra: k.monto_obra as f32,
            monto_obra_str: format_curr(k.monto_obra).into(),
            limite_18_obra: k.limite_18_obra as f32,
            limite_18_obra_str: format_curr(k.limite_18_obra).into(),
            supera_18: k.supera_18,
            empleados_count: k.empleados_count as i32,
        }
    }).collect();
    app_data.set_reporte_guarderias_obras(ModelRc::new(VecModel::from(guarderias_vec)));

    // Sincronizar estado del filtro multi-selección de empleados únicos
    sync_unique_empleados_to_ui(state, &app_data);
}

pub fn sync_and_load_reporte_data(
    ui: &MainWindow,
    pool_opt: &Option<Arc<DbPool>>,
    session_arc: &Arc<Mutex<ReporteSessionState>>,
    revision_id_str: &str,
) {
    let pool = match pool_opt {
        Some(p) => p,
        None => return,
    };

    let rev_uuid = match Uuid::parse_str(revision_id_str) {
        Ok(u) => u,
        Err(_) => return,
    };

    let mut client = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error obteniendo conexión a BD para reporte: {}", e);
            return;
        }
    };

    // 1. Sincronizar liquidaciones hacia tablas de reportes
    match sync_reportes_liquidaciones_for_revision(&mut client, rev_uuid) {
        Ok(c) => log::info!("Sincronizadas {} filas de reporte liquidaciones", c),
        Err(e) => log::error!("Error sincronizando reporte liquidaciones: {}", e),
    }

    // 2. Consultar filas y opciones de obras en paralelo concurrente aprovechando el pool de conexiones
    let (rows_res, obras_res, all_obras_res) = std::thread::scope(|s| {
        let pool_ref1 = pool.clone();
        let pool_ref2 = pool.clone();
        let pool_ref3 = pool.clone();

        let h1 = s.spawn(move || {
            let mut c = pool_ref1.get().map_err(|e| format!("Conexión pool: {}", e))?;
            fetch_reporte_liquidaciones(&mut c, rev_uuid)
        });
        let h2 = s.spawn(move || {
            let mut c = pool_ref2.get().map_err(|e| format!("Conexión pool: {}", e))?;
            fetch_obras_for_revision(&mut c, rev_uuid)
        });
        let h3 = s.spawn(move || {
            let mut c = pool_ref3.get().map_err(|e| format!("Conexión pool: {}", e))?;
            fetch_all_obras_for_revision(&mut c, rev_uuid)
        });

        (h1.join().unwrap(), h2.join().unwrap(), h3.join().unwrap())
    });

    let rows = match rows_res {
        Ok(r) => r,
        Err(e) => {
            log::error!("Error cargando filas de reporte: {}", e);
            Vec::new()
        }
    };

    let obras = match obras_res {
        Ok(o) => o,
        Err(e) => {
            log::error!("Error cargando obras para reporte: {}", e);
            Vec::new()
        }
    };

    // Conservar en obras_options todos los registros de obras creados por el usuario (creado_usuario = true)
    let mut obras: Vec<crate::db::reporte_liquidaciones::ObraOptionDb> = obras
        .into_iter()
        .filter(|o| o.creado_usuario)
        .collect();

    let mut all_obras = match all_obras_res {
        Ok(o) => o,
        Err(e) => {
            log::error!("Error cargando todas las obras para reporte: {}", e);
            Vec::new()
        }
    };
    for r in &rows {
        if let Some(oid) = r.id_registro_obra {
            if !all_obras.iter().any(|o| o.id == oid) {
                all_obras.push(crate::db::reporte_liquidaciones::ObraOptionDb {
                    id: oid,
                    numero: r.obra_numero.clone(),
                    tipo_obra: String::new(),
                    fecha_inicio: None,
                    fecha_fin: None,
                    label: if !r.obra_label.is_empty() { r.obra_label.clone() } else { r.obra_numero.clone() },
                    creado_usuario: r.obra_creado_usuario,
                    monto: 0.0,
                });
            }
        }
    }

    // Incorporar cualquier obra creada por el usuario o vinculada en filas que provenga de all_obras
    for o in &all_obras {
        if (o.creado_usuario || rows.iter().any(|r| r.id_registro_obra == Some(o.id)))
            && !obras.iter().any(|existing| existing.id == o.id)
        {
            obras.push(o.clone());
        }
    }

    // 3. Obtener periodos únicos ordenados cronológicamente
    let mut periodos_map: std::collections::BTreeMap<(i16, u32), (String, usize)> = std::collections::BTreeMap::new();
    for r in &rows {
        let m_num = month_to_num(&r.mes_periodo);
        let entry = periodos_map.entry((r.anio_periodo, m_num)).or_insert_with(|| (r.periodo_str.clone(), 0));
        entry.1 += 1;
    }
    let mut periodo_options = vec![format!("Todos los meses ({})", rows.len())];
    let mut periodo_values = vec!["".to_string()];
    for (_, (p_str, count)) in periodos_map {
        periodo_options.push(format!("{} ({})", p_str, count));
        periodo_values.push(p_str);
    }

    let app_data = ui.global::<AppData>();
    let client_name = app_data.get_selected_cliente_nombre().to_string();
    let rp_name = app_data.get_selected_rp_registro().to_string();
    let rev_name = app_data.get_selected_revision_nombre().to_string();
    let mut rev_folio = app_data.get_selected_revision_folio().to_string();
    if rev_folio.trim().is_empty() {
        if let Ok(Some(row)) = client.query_opt("SELECT folio FROM public.revisiones WHERE id = $1", &[&rev_uuid]) {
            rev_folio = row.get::<_, Option<String>>(0).unwrap_or_default().trim().to_string();
        }
    }
    if rev_folio.trim().is_empty() || rev_folio.trim() == "S/F" {
        rev_folio = rev_name.clone();
    }

    // 4. Actualizar estado de sesión
    {
        let mut state = session_arc.lock().unwrap();
        state.current_revision_id = Some(revision_id_str.to_string());
        state.cliente_nombre = client_name;
        state.rp_registro = rp_name;
        state.revision_nombre = rev_name;
        state.revision_folio = rev_folio;
        state.all_rows = rows;
        state.obras_options = obras;
        state.all_obras_options = all_obras;
        state.periodo_options = periodo_options;
        state.periodo_values = periodo_values;
        state.selected_ids.clear();
        state.current_page = 1;
        state.page_size = 100;
        state.search_query.clear();
        state.filter_obra_idx = 0;
        state.filter_periodo_idx = 0;
        state.filter_estado_idx = 0;

        // Reset filtros de empleados y modales
        state.emp_filter_active = false;
        state.emp_filter_all_months = true;
        state.emp_filter_all_obras = true;
        state.emp_filter_worker_ids.clear();
        state.emp_filter_worker_rfcs.clear();
        state.emp_filter_selected_months.clear();
        state.emp_filter_selected_obras.clear();
        state.emp_modal_worker_count = 0;
        state.emp_modal_worker_names_str.clear();
        state.emp_modal_total_occurrences = 0;
        state.emp_modal_meses.clear();
        state.emp_modal_obras.clear();

        state.selected_unique_emp_ids.clear();
        state.unique_emp_search_query.clear();

        ui.global::<AppData>().set_show_reporte_sort_modal(false);
        ui.global::<AppData>().set_show_reporte_emp_modal(false);

        update_unique_empleados_catalog(&mut state, &ui.global::<AppData>());
        apply_reporte_filtering(&mut state);
        render_reporte_ui(ui, &state);
    }
}

pub fn refresh_sua_modal_ui(app_data: &AppData, state: &ReporteSessionState) {
    let mut sua_obras_vec = Vec::with_capacity(state.all_obras_options.len() + 1);
    let mut selected_count = 0;

    // 1. Opción para trabajadores sin obra asignada
    let mut unassigned_emp_ids = HashSet::new();
    for r in &state.all_rows {
        if r.id_registro_obra.is_none() {
            unassigned_emp_ids.insert(r.id_empleado);
        }
    }
    if state.sua_include_unassigned {
        selected_count += 1;
    }
    sua_obras_vec.push(ReporteSuaObraItem {
        id: "__SIN_OBRA__".into(),
        numero: "Sin Asignar (Sin Obra)".into(),
        tipo_obra: "Liquidaciones sin obra".into(),
        vigencia: "Trabajadores permanentes / sin obra".into(),
        selected: state.sua_include_unassigned,
        creado_usuario: false,
        empleados_count: unassigned_emp_ids.len() as i32,
    });

    // 2. Obras registradas (todas las obras creadas y no creadas por el usuario)
    for o in &state.all_obras_options {
        let is_selected = state.sua_selected_obra_ids.contains(&o.id);
        if is_selected {
            selected_count += 1;
        }
        let fi_str = o.fecha_inicio.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "--/--/----".to_string());
        let ff_str = o.fecha_fin.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "--/--/----".to_string());
        let vigencia = format!("{} al {}", fi_str, ff_str);

        let mut emp_ids_in_obra = HashSet::new();
        for r in &state.all_rows {
            if r.id_registro_obra == Some(o.id) {
                emp_ids_in_obra.insert(r.id_empleado);
            }
        }

        sua_obras_vec.push(ReporteSuaObraItem {
            id: o.id.to_string().into(),
            numero: o.numero.clone().into(),
            tipo_obra: o.tipo_obra.clone().into(),
            vigencia: vigencia.into(),
            selected: is_selected,
            creado_usuario: o.creado_usuario,
            empleados_count: emp_ids_in_obra.len() as i32,
        });
    }

    let mut filtered_workers = HashSet::new();
    for r in &state.all_rows {
        if let Some(oid) = r.id_registro_obra {
            if state.sua_selected_obra_ids.contains(&oid) {
                filtered_workers.insert(r.id_empleado);
            }
        } else if state.sua_include_unassigned {
            filtered_workers.insert(r.id_empleado);
        }
    }

    app_data.set_reporte_sua_obras(ModelRc::new(VecModel::from(sua_obras_vec)));
    app_data.set_reporte_sua_selected_count(selected_count);
    app_data.set_reporte_sua_total_empleados_filtrados(filtered_workers.len() as i32);
}

pub fn wire_reporte_liquidaciones_callbacks(
    main_window: &MainWindow,
    db_pool: Option<Arc<DbPool>>,
    reporte_session: Arc<Mutex<ReporteSessionState>>,
) {
    let app_logic = main_window.global::<AppLogic>();

    let edit_modal_sel_model: Rc<RefCell<Option<Rc<VecModel<ReporteEditObraItem>>>>> = Rc::new(RefCell::new(None));
    let edit_modal_cat_model: Rc<RefCell<Option<Rc<VecModel<ReporteEditObraItem>>>>> = Rc::new(RefCell::new(None));

    // 1. Abrir vista de Reporte Liquidaciones
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_open_reporte_liquidaciones(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let rev_id = app_data.get_selected_revision_id().to_string();
                crate::app_logger::log_ui_action(
                    "Reporte Liquidaciones",
                    "Navegación",
                    "Botón Abrir Reporte",
                    "Abrir vista de Reporte Liquidaciones",
                    &format!("id_revision={}", rev_id),
                );
                if rev_id.is_empty() {
                    crate::app_logger::log_ui_error(
                        "Reporte Liquidaciones",
                        "Navegación",
                        "Validación",
                        "No hay revisión seleccionada",
                    );
                    crate::show_toast_message(&ui, "Seleccione primero una revisión.");
                    return;
                }
                app_data.set_reporte_liquidaciones_access_allowed(true);
                app_data.set_current_view(7);
                sync_and_load_reporte_data(&ui, &pool_clone, &session_clone, &rev_id);
            }
        });
    }

    // 2. Recargar reporte
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reload_reporte_liquidaciones(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let rev_id = ui.global::<AppData>().get_selected_revision_id().to_string();
                crate::app_logger::log_ui_action(
                    "Reporte Liquidaciones",
                    "Barra Superior",
                    "Botón Recargar",
                    "Recargar datos del reporte",
                    &format!("id_revision={}", rev_id),
                );
                if !rev_id.is_empty() {
                    sync_and_load_reporte_data(&ui, &pool_clone, &session_clone, &rev_id);
                    crate::show_toast_message(&ui, "Reporte de liquidaciones recargado.");
                }
            }
        });
    }

    // 3. Filtrar búsqueda textual
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_filtrar_busqueda(move |query| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_input(
                    "Reporte Liquidaciones",
                    "Filtros",
                    "Textbox Búsqueda",
                    "query",
                    &query,
                );
                let mut state = session_clone.lock().unwrap();
                state.search_query = query.to_string();
                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 4. Filtrar por Registro de Obra
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_filtrar_obra(move |idx| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_input(
                    "Reporte Liquidaciones",
                    "Filtros",
                    "Combobox Obra",
                    "filter_obra_idx",
                    &format!("{}", idx),
                );
                let mut state = session_clone.lock().unwrap();
                state.filter_obra_idx = idx as usize;
                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 4.1. Seleccionar / Filtrar por Registro de Obra creado por el usuario desde la tarjeta interactiva
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_seleccionar_obra_usuario(move |obra_id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_input(
                    "Reporte Liquidaciones",
                    "Panel Obras Usuario",
                    "Tarjeta Obra Clic",
                    "obra_id",
                    &obra_id_str,
                );
                let mut state = session_clone.lock().unwrap();
                let obra_id_str = obra_id_str.to_string();
                if let Ok(target_uuid) = Uuid::parse_str(&obra_id_str) {
                    if let Some(pos) = state.obras_options.iter().position(|o| o.id == target_uuid) {
                        let target_idx = pos + 1; // 0 is "Todas las obras"
                        if state.filter_obra_idx == target_idx {
                            // Toggle off: volver a todas las obras
                            state.filter_obra_idx = 0;
                        } else {
                            state.filter_obra_idx = target_idx;
                        }
                        apply_reporte_filtering(&mut state);
                        render_reporte_ui(&ui, &state);
                    }
                }
            }
        });
    }

    // 5. Filtrar por Periodo
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_filtrar_periodo(move |idx| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_input(
                    "Reporte Liquidaciones",
                    "Filtros",
                    "Combobox Periodo",
                    "filter_periodo_idx",
                    &format!("{}", idx),
                );
                let mut state = session_clone.lock().unwrap();
                state.filter_periodo_idx = idx as usize;
                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 6. Filtrar por Estado
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_filtrar_estado(move |idx| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_input(
                    "Reporte Liquidaciones",
                    "Filtros",
                    "Combobox Estado",
                    "filter_estado_idx",
                    &format!("{}", idx),
                );
                let mut state = session_clone.lock().unwrap();
                state.filter_estado_idx = idx as usize;
                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 7. Cambiar página
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_cambiar_pagina(move |page| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_action(
                    "Reporte Liquidaciones",
                    "Paginación",
                    "Botón Cambiar Página",
                    "Navegación de página",
                    &format!("nueva_pagina={}", page),
                );
                let mut state = session_clone.lock().unwrap();
                state.current_page = page.max(1) as usize;
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 8. Toggle selección de fila individual
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_select_row(move |id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_action(
                    "Reporte Liquidaciones",
                    "Tabla Principal",
                    "Fila",
                    "Toggle Selección Individual",
                    &format!("id={}", id_str),
                );
                if let Ok(id) = Uuid::parse_str(&id_str) {
                    let mut state = session_clone.lock().unwrap();
                    if let Some(r) = state.all_rows.iter().find(|row| row.id_reporte_liq == id) {
                        if r.clave.trim().eq_ignore_ascii_case("baja") {
                            crate::show_toast_message(&ui, "Los registros dados de baja están bloqueados para edición.");
                            return;
                        }
                    }
                    if state.selected_ids.contains(&id) {
                        state.selected_ids.remove(&id);
                    } else {
                        state.selected_ids.insert(id);
                    }
                    render_reporte_ui(&ui, &state);
                }
            }
        });
    }

    // 9. Toggle seleccionar todos los filtrados
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_select_all(move |select_all| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_action(
                    "Reporte Liquidaciones",
                    "Tabla Principal",
                    "Checkbox Seleccionar Todo",
                    "Alternar Selección Masiva",
                    &format!("select_all={}", select_all),
                );
                let mut state = session_clone.lock().unwrap();
                if select_all {
                    let ids: Vec<Uuid> = state.filtered_rows.iter()
                        .filter(|r| !r.clave.trim().eq_ignore_ascii_case("baja"))
                        .map(|r| r.id_reporte_liq)
                        .collect();
                    for id in ids {
                        state.selected_ids.insert(id);
                    }
                } else {
                    state.selected_ids.clear();
                }
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 9.1. Limpiar todos los seleccionados
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_limpiar_seleccion(move || {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_action(
                    "Reporte Liquidaciones",
                    "Tabla Principal",
                    "Botón Limpiar Selección",
                    "Limpiar registros seleccionados",
                    "",
                );
                let mut state = session_clone.lock().unwrap();
                state.selected_ids.clear();
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 9.2. Alternar expansión de fila para ver detalles de movimientos del empleado
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_expand_row(move |id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Ok(id) = Uuid::parse_str(&id_str) {
                    let mut state = session_clone.lock().unwrap();
                    if state.expanded_row_id == Some(id) {
                        state.expanded_row_id = None;
                    } else {
                        state.expanded_row_id = Some(id);
                    }
                    render_reporte_ui(&ui, &state);
                }
            }
        });
    }

    // 10. Helper interno para abrir modal unificado
    let open_unified_modal = {
        let session_clone = reporte_session.clone();
        let sel_holder = edit_modal_sel_model.clone();
        let cat_holder = edit_modal_cat_model.clone();
        move |ui: &crate::MainWindow, id_str: &str| {
            let app_data = ui.global::<AppData>();
            let mut state = session_clone.lock().unwrap();

            if let Ok(id) = Uuid::parse_str(id_str) {
                // Si hay múltiples empleados seleccionados y el clic fue en uno de ellos, abrir modal masivo
                if state.selected_ids.len() > 1 && state.selected_ids.contains(&id) {
                    drop(state);
                    let app_logic = ui.global::<AppLogic>();
                    app_logic.invoke_reporte_open_batch_edit_modal();
                    return;
                }
                let maybe_r = state.all_rows.iter().find(|row| row.id_reporte_liq == id).cloned();
                if let Some(r) = maybe_r {
                    crate::app_logger::log_ui_event(
                        "Reporte Liquidaciones",
                        "Tabla",
                        "Click Botón Editar Fila (Abrir Modal)",
                        &format!("id_reporte_liq={}, trabajador={}, periodo={}", id_str, r.nombre_trabajador, r.periodo_str),
                    );

                    // Validación: No permitir editar registros con clave "Baja"
                    if r.clave.trim().eq_ignore_ascii_case("baja") {
                        crate::app_logger::log_ui_error(
                            "Reporte Liquidaciones",
                            "Modal Edición",
                            "Validación Clave Baja",
                            "No se puede editar un registro con clave 'Baja'",
                        );
                        crate::show_toast_message(&ui, "No se puede editar un registro con clave 'Baja'.");
                        return;
                    }

                    app_data.set_reporte_edit_row_id(id_str.into());
                    app_data.set_reporte_edit_worker_name(r.nombre_trabajador.clone().into());
                    app_data.set_reporte_edit_periodo(r.periodo_str.clone().into());

                    let (obra_id_str, obra_label_str) = if let Some(oid) = r.id_registro_obra {
                        (oid.to_string(), r.obra_label.clone())
                    } else {
                        (String::new(), "⚠️ Sin Asignar (Pendiente)".to_string())
                    };

                    app_data.set_reporte_edit_selected_obra_id(obra_id_str.into());
                    app_data.set_reporte_edit_selected_obra_label(obra_label_str.clone().into());
                    app_data.set_reporte_edit_search_query(obra_label_str.into());

                    let lic_val = r.lic;
                    let inc_val = r.inc;
                    let aus_val = r.aus;

                    let (obra_ini, obra_fin, vigencia_str) = if let Some(oid) = r.id_registro_obra {
                        state.obras_options.iter().find(|o| o.id == oid)
                            .map(|o| {
                                let v = match (o.fecha_inicio, o.fecha_fin) {
                                    (Some(ini), Some(fin)) => format!("Vigencia: {} al {}", ini.format("%d/%m/%Y"), fin.format("%d/%m/%Y")),
                                    (Some(ini), None) => format!("Inicio: {}", ini.format("%d/%m/%Y")),
                                    (None, Some(fin)) => format!("Fin: {}", fin.format("%d/%m/%Y")),
                                    (None, None) => "Sin vigencia registrada".to_string(),
                                };
                                (o.fecha_inicio, o.fecha_fin, v)
                            })
                            .unwrap_or((None, None, String::new()))
                    } else {
                        (None, None, String::new())
                    };

                    let movs = extract_all_employee_movements(
                        r.id_empleado,
                        &r.mes_periodo,
                        r.anio_periodo,
                        &r.clave,
                        r.fecha,
                        &state.all_rows,
                    );

                    let (dias_max, dias_obra_en_mes) = calculate_max_dias_obra_for_employee_movements(
                        r.dias_cedula,
                        lic_val,
                        inc_val,
                        aus_val,
                        &r.mes_periodo,
                        r.anio_periodo,
                        &r.periodo_str,
                        obra_ini,
                        obra_fin,
                        &movs,
                    );

                    app_data.set_reporte_edit_lic_info(if lic_val > 0.001 { format!("{:.0}", lic_val) } else { "0".to_string() }.into());
                    app_data.set_reporte_edit_inc_info(if inc_val > 0.001 { format!("{:.0}", inc_val) } else { "0".to_string() }.into());
                    app_data.set_reporte_edit_aus_info(if aus_val > 0.001 { format!("{:.0}", aus_val) } else { "0".to_string() }.into());
                    app_data.set_reporte_edit_dias_max(dias_max as i32);

                    let info_msg = if r.id_registro_obra.is_some() {
                        if let Err(e) = validate_obra_assignment_for_employee_movements(
                            obra_ini,
                            obra_fin,
                            &movs,
                            &r.mes_periodo,
                            r.anio_periodo,
                            &r.periodo_str,
                        ) {
                            format!("⚠️ {}. Límite asignable: 0 días", e)
                        } else if let Some(d_obra) = dias_obra_en_mes {
                            format!(
                                "📅 Días activos en este mes: {} días ({}) • Límite asignable: {} días",
                                d_obra, vigencia_str, dias_max
                            )
                        } else {
                            format!("📅 {} • Límite asignable: {} días", vigencia_str, dias_max)
                        }
                    } else {
                        "⚠️ Selecciona un registro de obra para vincular al trabajador.".to_string()
                    };
                    app_data.set_reporte_edit_obra_vigencia_info(info_msg.into());

                    app_data.set_reporte_edit_dias_cedula_val(r.dias_cedula.to_string().into());
                    let default_dias_obra = if r.id_registro_obra.is_some() {
                        if (r.dias_obra as i32) <= (dias_max as i32) {
                            (r.dias_obra as i32).max(0)
                        } else {
                            dias_max as i32
                        }
                    } else {
                        0
                    };
                    app_data.set_reporte_edit_dias_obra_val(default_dias_obra.to_string().into());

                    let (m_num, y_num) = parse_month_and_year(&r.mes_periodo, r.anio_periodo, &r.periodo_str).unwrap_or((1, 2024));
                    let days_in_month = crate::db::reporte_liquidaciones::get_month_date_range(m_num, y_num as i32).map(|(_, _, d)| d as i32).unwrap_or(30);
                    let sum_incidencias = (r.lic + r.inc + r.aus).round() as i32;

                    let mut sorted_obras = state.obras_options.clone();
                    sorted_obras.sort_by(|a, b| {
                        let a_act = calculate_obra_days_in_month(m_num, y_num, a.fecha_inicio, a.fecha_fin).map(|d| d > 0).unwrap_or(true);
                        let b_act = calculate_obra_days_in_month(m_num, y_num, b.fecha_inicio, b.fecha_fin).map(|d| d > 0).unwrap_or(true);
                        (b.creado_usuario, b_act, &a.numero).cmp(&(a.creado_usuario, a_act, &b.numero))
                    });

                    let mut obras_vec = Vec::with_capacity(sorted_obras.len());
                    for o in &sorted_obras {
                        let is_act = calculate_obra_days_in_month(m_num, y_num, o.fecha_inicio, o.fecha_fin).map(|d| d > 0).unwrap_or(true);
                        let tag = if o.creado_usuario {
                            if is_act { "⭐ [Usuario - Vigente] " } else { "⭐ [Usuario] " }
                        } else {
                            if is_act { "📁 [Cédula - Vigente] " } else { "📁 [Cédula] " }
                        };
                        obras_vec.push(ReporteObraItem {
                            id: o.id.to_string().into(),
                            numero: o.numero.clone().into(),
                            label: format!("{}{}", tag, o.label).into(),
                        });
                    }

                    // Fórmula estricta Tope Máximo Total = (Días del Mes según periodo) - (LIC + INC + AUS)
                    // Si dias_cedula es menor que el mes (ej. alta/baja intermedia), se respeta dias_cedula - incidencias
                    let cedula_days = r.dias_cedula as i32;
                    let total_disp = if cedula_days > 0 && cedula_days < days_in_month {
                        (cedula_days - sum_incidencias).max(0)
                    } else {
                        (days_in_month - sum_incidencias).max(0)
                    };
                    app_data.set_reporte_edit_total_disponibles(total_disp);

                    // Buscar todos los registros hermanos del empleado en este mes/periodo
                    let sibling_rows: Vec<ReporteLiquidacionRowDb> = state.all_rows.iter().filter(|x| {
                        x.id_parent_reporte_liq == r.id_parent_reporte_liq
                            || (x.id_empleado == r.id_empleado
                                && x.mes_periodo.trim().eq_ignore_ascii_case(r.mes_periodo.trim())
                                && x.anio_periodo == r.anio_periodo)
                    }).cloned().collect();

                    let mut edit_obras = Vec::with_capacity(sorted_obras.len());
                    for o in &sorted_obras {
                        let dias_act = match calculate_obra_days_in_month(m_num, y_num, o.fecha_inicio, o.fecha_fin) {
                            Some(d) => d as i32,
                            None => days_in_month, // Sin fechas registradas: activa todo el mes
                        };
                        let is_act = dias_act > 0;
                        let tag = if o.creado_usuario {
                            if is_act { "⭐ [Usuario - Vigente] " } else { "⭐ [Usuario] " }
                        } else {
                            if is_act { "📁 [Cédula - Vigente] " } else { "📁 [Cédula] " }
                        };
                        let vigencia_str = match (o.fecha_inicio, o.fecha_fin) {
                            (Some(ini), Some(fin)) => format!("Vigencia: {} al {}", ini.format("%d/%m/%Y"), fin.format("%d/%m/%Y")),
                            (Some(ini), None) => format!("Inicio: {}", ini.format("%d/%m/%Y")),
                            (None, Some(fin)) => format!("Fin: {}", fin.format("%d/%m/%Y")),
                            (None, None) => "Sin vigencia registrada".to_string(),
                        };

                        // Vincular por UUID o por número de registro patronal de obra
                        let maybe_sib = sibling_rows.iter().find(|s| {
                            if let Some(s_oid) = s.id_registro_obra {
                                s_oid == o.id
                            } else if !s.obra_numero.trim().is_empty() {
                                s.obra_numero.trim().eq_ignore_ascii_case(o.numero.trim())
                            } else {
                                false
                            }
                        });
                        let selected = maybe_sib.is_some();
                        let d_obra = maybe_sib.map(|s| s.dias_obra as i32).unwrap_or(0);

                        edit_obras.push(ReporteEditObraItem {
                            id: o.id.to_string().into(),
                            numero: o.numero.clone().into(),
                            label: format!("{}{}", tag, o.label).into(),
                            vigencia_str: vigencia_str.into(),
                            selected,
                            dias_obra: d_obra,
                            dias_obra_str: d_obra.to_string().into(),
                            tope_max: dias_act,
                            dias_activos_mes: dias_act,
                            has_error: false,
                        });
                    }

                    let any_error = recalc_edit_obras_topes(&mut edit_obras, total_disp);

                    let asignados: i32 = edit_obras.iter().filter(|it| it.selected).map(|it| it.dias_obra).sum();
                    let restantes = (total_disp - asignados).max(0);
                    let sel_count = edit_obras.iter().filter(|it| it.selected).count() as i32;

                    let selected_only: Vec<ReporteEditObraItem> = edit_obras.iter().filter(|it| it.selected).cloned().collect();
                    let sel_model = Rc::new(VecModel::from(selected_only));
                    let cat_model = Rc::new(VecModel::from(edit_obras.clone()));

                    *sel_holder.borrow_mut() = Some(sel_model.clone());
                    *cat_holder.borrow_mut() = Some(cat_model.clone());
                    state.edit_items = edit_obras;

                    app_data.set_reporte_edit_selected_obras(ModelRc::from(sel_model));
                    app_data.set_reporte_edit_obras_list(ModelRc::from(cat_model));
                    app_data.set_reporte_edit_dias_asignados_sum(asignados);
                    app_data.set_reporte_edit_dias_restantes(restantes);
                    app_data.set_reporte_edit_selected_count(sel_count);
                    app_data.set_reporte_edit_has_errors(any_error);

                    let total_dias_asignados: i32 = sibling_rows.iter().map(|x| x.dias_obra as i32).sum();
                    let mut vinc_lines = Vec::new();
                    vinc_lines.push(format!("Total días cédula: {} | Días vinculados a obras: {} / {}", r.dias_cedula, total_dias_asignados, r.dias_cedula));
                    for (s_idx, sib) in sibling_rows.iter().enumerate() {
                        let current_marker = if sib.id_reporte_liq == r.id_reporte_liq { " [Esta Fila]" } else { "" };
                        let obra_txt = if sib.obra_label.trim().is_empty() { "Sin asignar".to_string() } else { sib.obra_label.clone() };
                        vinc_lines.push(format!("• Obra {}: {} ({} días){}", s_idx + 1, obra_txt, sib.dias_obra, current_marker));
                    }
                    let info_vinc_str = vinc_lines.join("\n");
                    app_data.set_reporte_edit_vinculaciones_info(info_vinc_str.into());

                    app_data.set_reporte_edit_filtered_obras(ModelRc::new(VecModel::from(obras_vec)));
                    app_data.set_reporte_edit_dropdown_open(false);
                    app_data.set_reporte_edit_alert_show(false);
                    app_data.set_reporte_edit_alert_text("".into());
                    app_data.set_show_reporte_edit_modal(true);
                }
            }
        }
    };

    // 10. Abrir modal unificado (nuevo y retrocompatible)
    {
        let ui_weak = main_window.as_weak();
        let open_fn = open_unified_modal.clone();
        app_logic.on_reporte_open_unified_modal(move |id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                open_fn(&ui, &id_str);
            }
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let open_fn = open_unified_modal.clone();
        app_logic.on_reporte_open_obra_picker(move |id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                open_fn(&ui, &id_str);
            }
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let open_fn = open_unified_modal;
        app_logic.on_reporte_open_edit_dias(move |id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                open_fn(&ui, &id_str);
            }
        });
    }

    // 11. Filtrar obras en modal unificado
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_filter_edit_obras(move |q| {
            if let Some(ui) = ui_weak.upgrade() {
                let q_lower = q.trim().to_lowercase();
                let state = session_clone.lock().unwrap();
                let mut obras_vec = Vec::new();
                for o in &state.obras_options {
                    if q_lower.is_empty() || o.numero.to_lowercase().contains(&q_lower) || o.label.to_lowercase().contains(&q_lower) {
                        obras_vec.push(ReporteObraItem {
                            id: o.id.to_string().into(),
                            numero: o.numero.clone().into(),
                            label: o.label.clone().into(),
                        });
                    }
                }
                ui.global::<AppData>().set_reporte_edit_filtered_obras(ModelRc::new(VecModel::from(obras_vec)));
            }
        });
    }

    // 12. Seleccionar obra en combobox del modal
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_select_edit_obra(move |obra_id, obra_label| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Reporte Liquidaciones",
                    "Modal Edición Unificada",
                    "Selección de Obra",
                    &format!("obra_id={}, label={}", obra_id, obra_label),
                );
                let app_data = ui.global::<AppData>();
                app_data.set_reporte_edit_selected_obra_id(obra_id.clone());
                app_data.set_reporte_edit_selected_obra_label(obra_label.clone());
                app_data.set_reporte_edit_search_query(obra_label);

                let state = session_clone.lock().unwrap();
                let cur_id_str = app_data.get_reporte_edit_row_id().to_string();
                if let Ok(cur_uuid) = Uuid::parse_str(&cur_id_str) {
                    if let Some(row) = state.all_rows.iter().find(|r| r.id_reporte_liq == cur_uuid) {
                        let movs = extract_all_employee_movements(
                            row.id_empleado,
                            &row.mes_periodo,
                            row.anio_periodo,
                            &row.clave,
                            row.fecha,
                            &state.all_rows,
                        );

                        if obra_id.is_empty() {
                            app_data.set_reporte_edit_dias_obra_val("0".into());
                            let (dias_max, _) = calculate_max_dias_obra_for_employee_movements(
                                row.dias_cedula,
                                row.lic,
                                row.inc,
                                row.aus,
                                &row.mes_periodo,
                                row.anio_periodo,
                                &row.periodo_str,
                                None,
                                None,
                                &movs,
                            );
                            app_data.set_reporte_edit_dias_max(dias_max as i32);
                            app_data.set_reporte_edit_obra_vigencia_info("⚠️ Sin obra seleccionada".into());
                        } else {
                            let (obra_ini, obra_fin, vigencia_str) = match Uuid::parse_str(&obra_id) {
                                Ok(oid) => state.obras_options.iter().find(|o| o.id == oid)
                                    .map(|o| {
                                        let v = match (o.fecha_inicio, o.fecha_fin) {
                                            (Some(ini), Some(fin)) => format!("Vigencia: {} al {}", ini.format("%d/%m/%Y"), fin.format("%d/%m/%Y")),
                                            (Some(ini), None) => format!("Inicio: {}", ini.format("%d/%m/%Y")),
                                            (None, Some(fin)) => format!("Fin: {}", fin.format("%d/%m/%Y")),
                                            (None, None) => "Sin vigencia registrada".to_string(),
                                        };
                                        (o.fecha_inicio, o.fecha_fin, v)
                                    })
                                    .unwrap_or((None, None, String::new())),
                                Err(_) => (None, None, String::new()),
                            };

                            // Validación de compatibilidad: obra iniciada después de baja o fuera de fechas según movimientos
                            if let Err(e) = validate_obra_assignment_for_employee_movements(
                                obra_ini,
                                obra_fin,
                                &movs,
                                &row.mes_periodo,
                                row.anio_periodo,
                                &row.periodo_str,
                            ) {
                                crate::app_logger::log_ui_error(
                                    "Reporte Liquidaciones",
                                    "Modal Edición Unificada",
                                    "Validación Obra Incompatible",
                                    &format!("No se puede asignar esta obra: {}", e),
                                );
                                crate::show_toast_message(&ui, &format!("No se puede asignar esta obra: {}", e));
                                app_data.set_reporte_edit_selected_obra_id("".into());
                                app_data.set_reporte_edit_selected_obra_label("⚠️ Sin Asignar (Pendiente)".into());
                                app_data.set_reporte_edit_search_query("".into());
                                app_data.set_reporte_edit_dias_obra_val("0".into());
                                app_data.set_reporte_edit_dias_max(0);
                                app_data.set_reporte_edit_obra_vigencia_info(format!("⚠️ {}", e).into());
                                app_data.set_reporte_edit_dropdown_open(false);
                                return;
                            }

                            let cur_cedula: i16 = app_data.get_reporte_edit_dias_cedula_val().trim().parse().unwrap_or(row.dias_cedula);
                            let (max_dias, dias_obra_en_mes) = calculate_max_dias_obra_for_employee_movements(
                                cur_cedula,
                                row.lic,
                                row.inc,
                                row.aus,
                                &row.mes_periodo,
                                row.anio_periodo,
                                &row.periodo_str,
                                obra_ini,
                                obra_fin,
                                &movs,
                            );

                            app_data.set_reporte_edit_dias_max(max_dias as i32);
                            app_data.set_reporte_edit_dias_obra_val(max_dias.to_string().into());

                            let info_msg = if let Some(d_obra) = dias_obra_en_mes {
                                format!(
                                    "📅 Días activos en este mes: {} días ({}) • Límite asignable: {} días",
                                    d_obra, vigencia_str, max_dias
                                )
                            } else {
                                format!("📅 {} • Límite asignable: {} días", vigencia_str, max_dias)
                            };
                            app_data.set_reporte_edit_obra_vigencia_info(info_msg.into());
                        }
                    }
                }

                let mut obras_vec = Vec::with_capacity(state.obras_options.len());
                for o in &state.obras_options {
                    obras_vec.push(ReporteObraItem {
                        id: o.id.to_string().into(),
                        numero: o.numero.clone().into(),
                        label: o.label.clone().into(),
                    });
                }
                app_data.set_reporte_edit_filtered_obras(ModelRc::new(VecModel::from(obras_vec)));
                app_data.set_reporte_edit_dropdown_open(false);
            }
        });
    }

    // 12b. Cambio en tiempo real de días cédula en el modal
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_edit_dias_cedula_changed(move |new_dias_str| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_input(
                    "Reporte Liquidaciones",
                    "Modal Edición Unificada",
                    "Input Días Cédula",
                    "dias_cedula",
                    &new_dias_str,
                );
                let app_data = ui.global::<AppData>();
                let state = session_clone.lock().unwrap();
                let cur_id_str = app_data.get_reporte_edit_row_id().to_string();
                if let Ok(cur_uuid) = Uuid::parse_str(&cur_id_str) {
                    if let Some(row) = state.all_rows.iter().find(|r| r.id_reporte_liq == cur_uuid) {
                        let movs = extract_all_employee_movements(
                            row.id_empleado,
                            &row.mes_periodo,
                            row.anio_periodo,
                            &row.clave,
                            row.fecha,
                            &state.all_rows,
                        );

                        let selected_oid_str = app_data.get_reporte_edit_selected_obra_id().to_string();
                        let (obra_ini, obra_fin, vigencia_str) = if !selected_oid_str.is_empty() {
                            match Uuid::parse_str(&selected_oid_str) {
                                Ok(oid) => state.obras_options.iter().find(|o| o.id == oid)
                                    .map(|o| {
                                        let v = match (o.fecha_inicio, o.fecha_fin) {
                                            (Some(ini), Some(fin)) => format!("Vigencia: {} al {}", ini.format("%d/%m/%Y"), fin.format("%d/%m/%Y")),
                                            (Some(ini), None) => format!("Inicio: {}", ini.format("%d/%m/%Y")),
                                            (None, Some(fin)) => format!("Fin: {}", fin.format("%d/%m/%Y")),
                                            (None, None) => "Sin vigencia registrada".to_string(),
                                        };
                                        (o.fecha_inicio, o.fecha_fin, v)
                                    })
                                    .unwrap_or((None, None, String::new())),
                                Err(_) => (None, None, String::new()),
                            }
                        } else {
                            (None, None, String::new())
                        };

                        let new_cedula: i16 = new_dias_str.trim().parse().unwrap_or(row.dias_cedula);
                        let (max_dias, dias_obra_en_mes) = calculate_max_dias_obra_for_employee_movements(
                            new_cedula,
                            row.lic,
                            row.inc,
                            row.aus,
                            &row.mes_periodo,
                            row.anio_periodo,
                            &row.periodo_str,
                            obra_ini,
                            obra_fin,
                            &movs,
                        );

                        app_data.set_reporte_edit_dias_max(max_dias as i32);
                        let cur_obra: i16 = app_data.get_reporte_edit_dias_obra_val().trim().parse().unwrap_or(0);
                        if cur_obra > max_dias {
                            app_data.set_reporte_edit_dias_obra_val(max_dias.to_string().into());
                        }

                        if !selected_oid_str.is_empty() {
                            let info_msg = if let Some(d_obra) = dias_obra_en_mes {
                                format!(
                                    "📅 Días activos de la obra en este mes: {} días ({}) • Límite asignable: {} días",
                                    d_obra, vigencia_str, max_dias
                                )
                            } else {
                                format!("📅 {} • Límite asignable: {} días", vigencia_str, max_dias)
                            };
                            app_data.set_reporte_edit_obra_vigencia_info(info_msg.into());
                        }
                    }
                }
            }
        });
    }

    // 13. Alternar desplegable combobox
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_toggle_edit_dropdown(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let new_val = !app_data.get_reporte_edit_dropdown_open();
                crate::app_logger::log_ui_event(
                    "Reporte Liquidaciones",
                    "Modal Edición Unificada",
                    "Alternar Dropdown Obras",
                    &format!("abierto={}", new_val),
                );
                app_data.set_reporte_edit_dropdown_open(new_val);
            }
        });
    }

    // 14. Guardar cambios del modal unificado
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_submit_unified_edit(move |id_str, obra_id_str, dias_ced_str, dias_obr_str| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Reporte Liquidaciones",
                    "Modal Edición Unificada",
                    "Click Botón Guardar Cambios",
                    &format!("id={}, obra_id={}, dias_cedula={}, dias_obra={}", id_str, obra_id_str, dias_ced_str, dias_obr_str),
                );

                let row_uuid = match Uuid::parse_str(&id_str) {
                    Ok(u) => u,
                    Err(_) => {
                        crate::app_logger::log_ui_error(
                            "Reporte Liquidaciones",
                            "Modal Edición Unificada",
                            "Error Parse UUID",
                            &format!("UUID inválido: {}", id_str),
                        );
                        return;
                    }
                };

                let new_dias_ced = dias_ced_str.trim().parse::<i16>().unwrap_or(30).max(1);
                let raw_dias_obr = dias_obr_str.trim().parse::<i16>().unwrap_or(0).max(0);

                let obra_uuid_opt = if obra_id_str.trim().is_empty() {
                    None
                } else {
                    Uuid::parse_str(obra_id_str.trim()).ok()
                };

                // Validación: Días Cédula y Días Obra no deben exceder 31 días
                if new_dias_ced > 31 || raw_dias_obr > 31 {
                    let err_msg = format!("Validación: Los días no pueden exceder 31 (Cédula: {}, Obra: {}).", new_dias_ced, raw_dias_obr);
                    crate::app_logger::log_ui_error(
                        "Reporte Liquidaciones",
                        "Modal Edición Unificada",
                        "Validación Límite 31 Días",
                        &err_msg,
                    );
                    crate::show_toast_message(
                        &ui,
                        &err_msg,
                    );
                    return;
                }

                // Obtener datos del empleado y sus movimientos en el mes
                let (lic_val, inc_val, aus_val, mes_periodo, anio_periodo, periodo_str, movs, parent_id) = {
                    let state = session_clone.lock().unwrap();
                    if let Some(r) = state.all_rows.iter().find(|row| row.id_reporte_liq == row_uuid) {
                        let m = extract_all_employee_movements(
                            r.id_empleado,
                            &r.mes_periodo,
                            r.anio_periodo,
                            &r.clave,
                            r.fecha,
                            &state.all_rows,
                        );
                        (r.lic, r.inc, r.aus, r.mes_periodo.clone(), r.anio_periodo, r.periodo_str.clone(), m, r.id_parent_reporte_liq)
                    } else {
                        (0.0, 0.0, 0.0, String::new(), 0, String::new(), Vec::new(), Uuid::nil())
                    }
                };

                // Validación multi-obra: Suma de días asignados a las obras no debe superar días cédula
                {
                    let state = session_clone.lock().unwrap();
                    let other_obras_days: i16 = state.all_rows.iter()
                        .filter(|r| r.id_parent_reporte_liq == parent_id && r.id_reporte_liq != row_uuid)
                        .map(|r| r.dias_obra)
                        .sum();

                    if other_obras_days + raw_dias_obr > new_dias_ced {
                        let err_msg = format!(
                            "Restricción: La suma de días entre las obras ({} ya asignados en otras obras + {} en esta = {}) supera los días de cédula del trabajador ({}).",
                            other_obras_days, raw_dias_obr, other_obras_days + raw_dias_obr, new_dias_ced
                        );
                        crate::app_logger::log_ui_error(
                            "Reporte Liquidaciones",
                            "Modal Edición Unificada",
                            "Validación Suma Días Multi-Obra",
                            &err_msg,
                        );
                        crate::show_toast_message(&ui, &err_msg);
                        return;
                    }
                }

                let (obra_ini, obra_fin, dias_obra_en_mes, vigencia_str) = if let Some(oid) = obra_uuid_opt {
                    let state = session_clone.lock().unwrap();
                    state.obras_options.iter().find(|o| o.id == oid)
                        .map(|o| {
                            let v = match (o.fecha_inicio, o.fecha_fin) {
                                (Some(ini), Some(fin)) => format!("vigencia {} al {}", ini.format("%d/%m/%Y"), fin.format("%d/%m/%Y")),
                                (Some(ini), None) => format!("inicio {}", ini.format("%d/%m/%Y")),
                                (None, Some(fin)) => format!("fin {}", fin.format("%d/%m/%Y")),
                                (None, None) => "sin fechas registradas".to_string(),
                            };
                            let d = parse_month_and_year(&mes_periodo, anio_periodo, &periodo_str)
                                .and_then(|(m, y)| calculate_obra_days_in_month(m, y, o.fecha_inicio, o.fecha_fin));
                            (o.fecha_inicio, o.fecha_fin, d, v)
                        })
                        .unwrap_or((None, None, None, String::new()))
                } else {
                    (None, None, None, String::new())
                };

                // Validación: si tiene obra seleccionada, verificar compatibilidad con movimientos y no superposición
                if let Some(new_oid) = obra_uuid_opt {
                    if let Err(e) = validate_obra_assignment_for_employee_movements(
                        obra_ini,
                        obra_fin,
                        &movs,
                        &mes_periodo,
                        anio_periodo,
                        &periodo_str,
                    ) {
                        crate::app_logger::log_ui_error(
                            "Reporte Liquidaciones",
                            "Modal Edición Unificada",
                            "Validación Asignación Obra",
                            &format!("Error compatibilidad: {}", e),
                        );
                        crate::show_toast_message(&ui, &format!("No se puede asignar esta obra: {}", e));
                        return;
                    }

                    // Verificar superposición con otras obras ya vinculadas a este empleado
                    let state = session_clone.lock().unwrap();
                    for other in state.all_rows.iter().filter(|r| r.id_parent_reporte_liq == parent_id && r.id_reporte_liq != row_uuid) {
                        if let Some(other_oid) = other.id_registro_obra {
                            if other_oid == new_oid {
                                let err_msg = format!("El trabajador ya tiene asignada la obra '{}' en otra fila de este periodo.", other.obra_label);
                                crate::show_toast_message(&ui, &err_msg);
                                return;
                            }
                        }
                    }
                }

                let (max_permitido, _) = calculate_max_dias_obra_for_employee_movements(
                    new_dias_ced,
                    lic_val,
                    inc_val,
                    aus_val,
                    &mes_periodo,
                    anio_periodo,
                    &periodo_str,
                    obra_ini,
                    obra_fin,
                    &movs,
                );

                if obra_uuid_opt.is_some() && raw_dias_obr > max_permitido {
                    let total_inc = lic_val + inc_val + aus_val;
                    let razon = if let Err(e) = validate_obra_assignment_for_employee_movements(
                        obra_ini,
                        obra_fin,
                        &movs,
                        &mes_periodo,
                        anio_periodo,
                        &periodo_str,
                    ) {
                        e
                    } else if let Some(d_obra) = dias_obra_en_mes {
                        format!(
                            "Días activos de la obra en el mes: {} ({}), Días efectivos trabajador: {:.0} (Cédula {} - Incidencias {:.0})",
                            d_obra, vigencia_str, (new_dias_ced as f64 - total_inc).max(0.0), new_dias_ced, total_inc
                        )
                    } else {
                        format!(
                            "Días efectivos trabajador: {:.0} (Cédula {} - Incidencias {:.0})",
                            (new_dias_ced as f64 - total_inc).max(0.0), new_dias_ced, total_inc
                        )
                    };
                    let err_msg = format!(
                        "Restricción: Días Obra ({}) supera el máximo permitido ({}) para este empleado en {}. ({})",
                        raw_dias_obr, max_permitido, periodo_str, razon
                    );
                    crate::app_logger::log_ui_error(
                        "Reporte Liquidaciones",
                        "Modal Edición Unificada",
                        "Validación Límite Días Obra",
                        &err_msg,
                    );
                    crate::show_toast_message(
                        &ui,
                        &err_msg,
                    );
                    return;
                }

                let app_data = ui.global::<AppData>();
                app_data.set_show_reporte_edit_modal(false);

                let new_dias_obr = if obra_uuid_opt.is_some() {
                    if dias_obr_str.trim().is_empty() {
                        max_permitido
                    } else {
                        raw_dias_obr
                    }
                } else {
                    0
                };

                {
                    let mut state = session_clone.lock().unwrap();
                    let obra_info = if let Some(oid) = obra_uuid_opt {
                        state.obras_options.iter().find(|o| o.id == oid).cloned()
                    } else {
                        None
                    };

                    if let Some(row) = state.all_rows.iter_mut().find(|r| r.id_reporte_liq == row_uuid) {
                        row.dias_cedula = new_dias_ced;
                        row.dias_obra = new_dias_obr;
                        row.id_registro_obra = obra_uuid_opt;
                        if let Some(ref o) = obra_info {
                            row.obra_numero = o.numero.clone();
                            row.obra_label = o.label.clone();
                            row.obra_creado_usuario = o.creado_usuario;
                        } else {
                            row.obra_numero = String::new();
                            row.obra_label = "⚠️ Sin Asignar (Pendiente)".to_string();
                            row.obra_creado_usuario = false;
                        }
                        recalculate_row_in_memory(row);
                    }

                    // Sincronizar dias_cedula en filas hermanas del mismo empleado
                    for r in state.all_rows.iter_mut() {
                        if r.id_parent_reporte_liq == parent_id && r.id_reporte_liq != row_uuid {
                            r.dias_cedula = new_dias_ced;
                            recalculate_row_in_memory(r);
                        }
                    }

                    apply_reporte_filtering(&mut state);
                    render_reporte_ui(&ui, &state);
                }

                // Persistir en PostgreSQL en segundo plano
                if let Some(ref pool) = pool_clone {
                    let pool_bg = pool.clone();
                    std::thread::spawn(move || {
                        if let Ok(mut client) = pool_bg.get() {
                            if let Err(e) = update_single_reporte_row(
                                &mut client,
                                row_uuid,
                                new_dias_ced,
                                new_dias_obr,
                                obra_uuid_opt,
                            ) {
                                log::error!("Error persistiendo actualización unificada: {}", e);
                            }
                        }
                    });
                }

                crate::show_toast_message(&ui, "Registro de obra y días actualizados correctamente.");
            }
        });
    }

    // Callbacks reactivos del Modal Multi-Obra
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        let sel_holder = edit_modal_sel_model.clone();
        let cat_holder = edit_modal_cat_model.clone();
        app_logic.on_reporte_edit_toggle_obra(move |obra_id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let total_disponibles = app_data.get_reporte_edit_total_disponibles();
                let mut state = session_clone.lock().unwrap();

                if let Some(pos) = state.edit_items.iter().position(|it| it.id == obra_id_str) {
                    let was_selected = state.edit_items[pos].selected;
                    let now_selected = !was_selected;
                    state.edit_items[pos].selected = now_selected;
                    if now_selected {
                        let current_allocated: i32 = state.edit_items.iter()
                            .enumerate()
                            .filter(|(idx, it)| it.selected && *idx != pos)
                            .map(|(_, it)| it.dias_obra)
                            .sum();
                        let unallocated = (total_disponibles - current_allocated).max(0);
                        let max_possible = if state.edit_items[pos].dias_activos_mes > 0 {
                            state.edit_items[pos].dias_activos_mes
                        } else {
                            total_disponibles
                        };
                        let assign = unallocated.min(max_possible);
                        state.edit_items[pos].dias_obra = assign;
                        state.edit_items[pos].dias_obra_str = assign.to_string().into();
                    } else {
                        state.edit_items[pos].dias_obra = 0;
                        state.edit_items[pos].dias_obra_str = "0".into();
                    }
                }

                let any_error = recalc_edit_obras_topes(&mut state.edit_items, total_disponibles);
                let asignados: i32 = state.edit_items.iter().filter(|it| it.selected).map(|it| it.dias_obra).sum();
                let restantes = (total_disponibles - asignados).max(0);
                let sel_count = state.edit_items.iter().filter(|it| it.selected).count() as i32;

                let selected_only: Vec<ReporteEditObraItem> = state.edit_items.iter().filter(|it| it.selected).cloned().collect();
                let sel_model = Rc::new(VecModel::from(selected_only));
                let cat_model = Rc::new(VecModel::from(state.edit_items.clone()));

                *sel_holder.borrow_mut() = Some(sel_model.clone());
                *cat_holder.borrow_mut() = Some(cat_model.clone());

                app_data.set_reporte_edit_selected_obras(ModelRc::from(sel_model));
                app_data.set_reporte_edit_obras_list(ModelRc::from(cat_model));
                app_data.set_reporte_edit_dias_asignados_sum(asignados);
                app_data.set_reporte_edit_dias_restantes(restantes);
                app_data.set_reporte_edit_selected_count(sel_count);
                app_data.set_reporte_edit_has_errors(any_error);

                if app_data.get_reporte_edit_alert_show() {
                    app_data.set_reporte_edit_alert_show(false);
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        let sel_holder = edit_modal_sel_model.clone();
        let cat_holder = edit_modal_cat_model.clone();
        app_logic.on_reporte_edit_obra_dias_changed(move |obra_id_str, dias_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let total_disponibles = app_data.get_reporte_edit_total_disponibles();
                let mut state = session_clone.lock().unwrap();

                let parsed_val = dias_str.trim().parse::<i32>().unwrap_or(0);
                if let Some(it) = state.edit_items.iter_mut().find(|it| it.id == obra_id_str) {
                    it.dias_obra = parsed_val;
                    it.dias_obra_str = dias_str.clone();
                }

                let any_error = recalc_edit_obras_topes(&mut state.edit_items, total_disponibles);
                let asignados: i32 = state.edit_items.iter().filter(|it| it.selected).map(|it| it.dias_obra).sum();
                let restantes = (total_disponibles - asignados).max(0);
                let sel_count = state.edit_items.iter().filter(|it| it.selected).count() as i32;

                app_data.set_reporte_edit_dias_asignados_sum(asignados);
                app_data.set_reporte_edit_dias_restantes(restantes);
                app_data.set_reporte_edit_selected_count(sel_count);
                app_data.set_reporte_edit_has_errors(any_error);

                // Sincronizar en el modelo persistente seleccionado (sel_model) usando set_row_data
                // Para la fila activa que el usuario está editando, NO tocamos dias_obra_str para que
                // Slint no reinicie el texto ni retire el foco del TextInput.
                if let Some(ref sel_model) = *sel_holder.borrow() {
                    let active_id = obra_id_str.as_str();
                    for r_idx in 0..sel_model.row_count() {
                        if let Some(mut existing) = sel_model.row_data(r_idx) {
                            if let Some(updated) = state.edit_items.iter().find(|it| it.id == existing.id) {
                                let is_active = existing.id.as_str() == active_id;
                                let needs_update = if is_active {
                                    existing.tope_max != updated.tope_max || existing.has_error != updated.has_error
                                } else {
                                    existing.tope_max != updated.tope_max
                                        || existing.has_error != updated.has_error
                                        || existing.dias_obra != updated.dias_obra
                                        || existing.dias_obra_str != updated.dias_obra_str
                                };

                                if needs_update {
                                    existing.tope_max = updated.tope_max;
                                    existing.has_error = updated.has_error;
                                    if !is_active {
                                        existing.dias_obra = updated.dias_obra;
                                        existing.dias_obra_str = updated.dias_obra_str.clone();
                                    }
                                    sel_model.set_row_data(r_idx, existing);
                                }
                            }
                        }
                    }
                }

                // Sincronizar en el catálogo inferior
                if let Some(ref cat_model) = *cat_holder.borrow() {
                    for r_idx in 0..cat_model.row_count() {
                        if let Some(mut existing) = cat_model.row_data(r_idx) {
                            if let Some(updated) = state.edit_items.iter().find(|it| it.id == existing.id) {
                                if existing.dias_obra != updated.dias_obra
                                    || existing.tope_max != updated.tope_max
                                    || existing.has_error != updated.has_error
                                {
                                    existing.dias_obra = updated.dias_obra;
                                    existing.dias_obra_str = updated.dias_obra_str.clone();
                                    existing.tope_max = updated.tope_max;
                                    existing.has_error = updated.has_error;
                                    cat_model.set_row_data(r_idx, existing);
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_submit_unified_edit_multi(move |id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let row_uuid = match Uuid::parse_str(&id_str) {
                    Ok(u) => u,
                    Err(_) => return,
                };

                let app_data = ui.global::<AppData>();
                let total_disponibles = app_data.get_reporte_edit_total_disponibles();
                let asignados = app_data.get_reporte_edit_dias_asignados_sum();

                if app_data.get_reporte_edit_has_errors() {
                    let err_msg = "No se puede guardar: existen obras con días que exceden su tope dinámico.";
                    app_data.set_reporte_edit_alert_text(err_msg.into());
                    app_data.set_reporte_edit_alert_type("error".into());
                    app_data.set_reporte_edit_alert_show(true);
                    crate::show_toast_message(&ui, err_msg);
                    return;
                }

                if asignados > total_disponibles {
                    let err_msg = format!("Restricción: La suma de días asignados ({}) supera el total disponible de este empleado ({}).", asignados, total_disponibles);
                    app_data.set_reporte_edit_alert_text(err_msg.clone().into());
                    app_data.set_reporte_edit_alert_type("error".into());
                    app_data.set_reporte_edit_alert_show(true);
                    crate::show_toast_message(&ui, &err_msg);
                    return;
                }

                let selected_obras: Vec<(Uuid, String, String, i16)> = {
                    let state = session_clone.lock().unwrap();
                    state.edit_items.iter()
                        .filter(|it| it.selected)
                        .filter_map(|it| {
                            Uuid::parse_str(&it.id).ok().map(|oid| {
                                (oid, it.numero.to_string(), it.label.to_string(), it.dias_obra as i16)
                            })
                        })
                        .collect()
                };

                let (parent_id, sibling_ids) = {
                    let state = session_clone.lock().unwrap();
                    if let Some(r) = state.all_rows.iter().find(|x| x.id_reporte_liq == row_uuid) {
                        let sibs: Vec<Uuid> = state.all_rows.iter()
                            .filter(|x| x.id_parent_reporte_liq == r.id_parent_reporte_liq)
                            .map(|x| x.id_reporte_liq)
                            .collect();
                        (r.id_parent_reporte_liq, sibs)
                    } else {
                        (Uuid::nil(), Vec::new())
                    }
                };

                if parent_id == Uuid::nil() {
                    return;
                }

                {
                    let mut state = session_clone.lock().unwrap();

                    if selected_obras.is_empty() {
                        if let Some(row) = state.all_rows.iter_mut().find(|x| x.id_reporte_liq == row_uuid) {
                            row.id_registro_obra = None;
                            row.obra_numero = String::new();
                            row.obra_label = "⚠️ Sin Asignar (Pendiente)".to_string();
                            row.dias_obra = 0;
                            row.obra_creado_usuario = false;
                            recalculate_row_in_memory(row);
                        }
                        state.all_rows.retain(|x| !(x.id_parent_reporte_liq == parent_id && x.id_reporte_liq != row_uuid));
                    } else {
                        let (first_oid, first_num, first_lbl, first_dias) = selected_obras[0].clone();
                        let first_creado = state.obras_options.iter().find(|o| o.id == first_oid).map(|o| o.creado_usuario).unwrap_or(false);

                        if let Some(row) = state.all_rows.iter_mut().find(|x| x.id_reporte_liq == row_uuid) {
                            row.id_registro_obra = Some(first_oid);
                            row.obra_numero = first_num;
                            row.obra_label = first_lbl;
                            row.dias_obra = first_dias;
                            row.obra_creado_usuario = first_creado;
                            recalculate_row_in_memory(row);
                        }

                        let mut current_sib_idx = 0;
                        let extra_sibs: Vec<Uuid> = sibling_ids.into_iter().filter(|&id| id != row_uuid).collect();

                        for (sub_oid, sub_num, sub_lbl, sub_dias) in selected_obras.into_iter().skip(1) {
                            let sub_creado = state.obras_options.iter().find(|o| o.id == sub_oid).map(|o| o.creado_usuario).unwrap_or(false);
                            if current_sib_idx < extra_sibs.len() {
                                let sib_id = extra_sibs[current_sib_idx];
                                if let Some(row) = state.all_rows.iter_mut().find(|x| x.id_reporte_liq == sib_id) {
                                    row.id_registro_obra = Some(sub_oid);
                                    row.obra_numero = sub_num;
                                    row.obra_label = sub_lbl;
                                    row.dias_obra = sub_dias;
                                    row.obra_creado_usuario = sub_creado;
                                    recalculate_row_in_memory(row);
                                }
                                current_sib_idx += 1;
                            } else {
                                if let Some(base_row) = state.all_rows.iter().find(|x| x.id_reporte_liq == row_uuid).cloned() {
                                    let mut new_split = base_row;
                                    new_split.id_reporte_liq = Uuid::now_v7();
                                    new_split.id_registro_obra = Some(sub_oid);
                                    new_split.obra_numero = sub_num;
                                    new_split.obra_label = sub_lbl;
                                    new_split.dias_obra = sub_dias;
                                    new_split.obra_creado_usuario = sub_creado;
                                    recalculate_row_in_memory(&mut new_split);
                                    state.all_rows.push(new_split);
                                }
                            }
                        }

                        if current_sib_idx < extra_sibs.len() {
                            let to_remove: HashSet<Uuid> = extra_sibs[current_sib_idx..].iter().cloned().collect();
                            state.all_rows.retain(|x| !to_remove.contains(&x.id_reporte_liq));
                        }
                    }

                    apply_reporte_filtering(&mut state);
                    render_reporte_ui(&ui, &state);
                }

                if let Some(ref pool) = pool_clone {
                    let pool_bg = pool.clone();
                    let current_worker_rows: Vec<ReporteLiquidacionRowDb> = {
                        let state = session_clone.lock().unwrap();
                        state.all_rows.iter()
                            .filter(|x| x.id_parent_reporte_liq == parent_id)
                            .cloned()
                            .collect()
                    };
                    std::thread::spawn(move || {
                        if let Ok(mut client) = pool_bg.get() {
                            for r in &current_worker_rows {
                                let _ = update_single_reporte_row(
                                    &mut client,
                                    r.id_reporte_liq,
                                    r.dias_cedula,
                                    r.dias_obra,
                                    r.id_registro_obra,
                                );
                            }
                        }
                    });
                }

                app_data.set_show_reporte_edit_modal(false);
                app_data.set_reporte_edit_alert_show(false);
                crate::show_toast_message(&ui, "Asignación de obras y días guardada correctamente.");
            }
        });
    }

    // 15. Cerrar modal unificado
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_close_edit_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_reporte_edit_alert_show(false);
                app_data.set_show_reporte_edit_modal(false);
            }
        });
    }
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_close_obra_picker(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_reporte_edit_modal(false);
            }
        });
    }
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_close_edit_dias(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_reporte_edit_modal(false);
            }
        });
    }

    // Callbacks del filtro multi-selección de empleados únicos
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_unique_empleado(move |emp_id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let id_s = emp_id_str.to_string();
                let app_data = ui.global::<AppData>();
                let mut state = session_clone.lock().unwrap();

                let is_now_selected = if state.selected_unique_emp_ids.contains(&id_s) {
                    state.selected_unique_emp_ids.remove(&id_s);
                    false
                } else {
                    state.selected_unique_emp_ids.insert(id_s.clone());
                    true
                };

                for emp in state.unique_empleados.iter_mut() {
                    if emp.id.as_str() == id_s {
                        emp.selected = is_now_selected;
                    }
                }

                apply_reporte_filtering(&mut state);
                sync_unique_empleados_to_ui(&state, &app_data);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_unique_empleados_select_all(move |select_all| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let mut state = session_clone.lock().unwrap();

                if select_all {
                    for emp in state.unique_empleados.iter_mut() {
                        emp.selected = true;
                    }
                    state.selected_unique_emp_ids = state.unique_empleados.iter().map(|e| e.id.to_string()).collect();
                } else {
                    state.selected_unique_emp_ids.clear();
                    for emp in state.unique_empleados.iter_mut() {
                        emp.selected = false;
                    }
                }

                apply_reporte_filtering(&mut state);
                sync_unique_empleados_to_ui(&state, &app_data);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_unique_empleados_search_changed(move |txt| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let mut state = session_clone.lock().unwrap();
                state.unique_emp_search_query = txt.to_string();
                sync_unique_empleados_to_ui(&state, &app_data);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_limpiar_unique_empleados_filter(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let mut state = session_clone.lock().unwrap();
                state.selected_unique_emp_ids.clear();
                for emp in state.unique_empleados.iter_mut() {
                    emp.selected = false;
                }
                apply_reporte_filtering(&mut state);
                sync_unique_empleados_to_ui(&state, &app_data);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // Callbacks del Side Drawer de Registros de Obra
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_drawer_obras(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                state.is_drawer_open = !state.is_drawer_open;
                ui.global::<AppData>().set_reporte_drawer_obras_open(state.is_drawer_open);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_set_drawer_obras(move |open| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                state.is_drawer_open = open;
                ui.global::<AppData>().set_reporte_drawer_obras_open(open);
            }
        });
    }

    // Callbacks para vincular a otra obra adicional
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_open_add_obra_modal(move |id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let id = match Uuid::parse_str(&id_str) {
                    Ok(u) => u,
                    Err(_) => return,
                };
                let state = session_clone.lock().unwrap();
                let row = match state.all_rows.iter().find(|r| r.id_reporte_liq == id) {
                    Some(r) => r,
                    None => return,
                };

                let parent_id = row.id_parent_reporte_liq;
                let worker_rows: Vec<&ReporteLiquidacionRowDb> = state.all_rows.iter()
                    .filter(|r| r.id_parent_reporte_liq == parent_id)
                    .collect();

                let dias_used: i16 = worker_rows.iter()
                    .map(|r| r.dias_obra)
                    .sum();
                let dias_cedula = row.dias_cedula;
                let dias_avail = (dias_cedula - dias_used).max(0);

                let linked_obras: Vec<Uuid> = worker_rows.iter()
                    .filter_map(|r| r.id_registro_obra)
                    .collect();

                // Filtrar obras que no estén ya vinculadas a este empleado
                let filtered_obras: Vec<ReporteObraItem> = state.obras_options.iter()
                    .filter(|o| !linked_obras.contains(&o.id))
                    .map(|o| ReporteObraItem {
                        id: o.id.to_string().into(),
                        numero: o.numero.clone().into(),
                        label: o.label.clone().into(),
                    })
                    .collect();

                let app_data = ui.global::<AppData>();
                app_data.set_show_reporte_edit_modal(false);
                app_data.set_reporte_add_row_id(id_str);
                app_data.set_reporte_add_worker_name(row.nombre_trabajador.clone().into());
                app_data.set_reporte_add_periodo(row.periodo_str.clone().into());
                app_data.set_reporte_add_dias_cedula(dias_cedula as i32);
                app_data.set_reporte_add_dias_used(dias_used as i32);
                app_data.set_reporte_add_dias_available(dias_avail as i32);
                app_data.set_reporte_add_selected_obra_id("".into());
                app_data.set_reporte_add_selected_obra_label("".into());
                app_data.set_reporte_add_search_query("".into());
                app_data.set_reporte_add_dropdown_open(false);
                app_data.set_reporte_add_filtered_obras(ModelRc::new(VecModel::from(filtered_obras)));
                app_data.set_reporte_add_dias_obra_val(if dias_avail > 0 { dias_avail.to_string().into() } else { "0".into() });
                app_data.set_reporte_add_obra_vigencia_info("".into());
                app_data.set_reporte_add_alert_show(false);
                app_data.set_show_reporte_add_obra_modal(true);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_close_add_obra_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_reporte_add_alert_show(false);
                app_data.set_show_reporte_add_obra_modal(false);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_filter_add_obras(move |q| {
            if let Some(ui) = ui_weak.upgrade() {
                let q_lower = q.trim().to_lowercase();
                let state = session_clone.lock().unwrap();
                let app_data = ui.global::<AppData>();
                let row_id_str = app_data.get_reporte_add_row_id().to_string();
                let linked_obras: Vec<Uuid> = if let Ok(id) = Uuid::parse_str(&row_id_str) {
                    if let Some(row) = state.all_rows.iter().find(|r| r.id_reporte_liq == id) {
                        state.all_rows.iter()
                            .filter(|r| r.id_parent_reporte_liq == row.id_parent_reporte_liq)
                            .filter_map(|r| r.id_registro_obra)
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };

                let mut obras_vec = Vec::new();
                for o in &state.obras_options {
                    if linked_obras.contains(&o.id) {
                        continue;
                    }
                    if q_lower.is_empty() || o.numero.to_lowercase().contains(&q_lower) || o.label.to_lowercase().contains(&q_lower) {
                        obras_vec.push(ReporteObraItem {
                            id: o.id.to_string().into(),
                            numero: o.numero.clone().into(),
                            label: o.label.clone().into(),
                        });
                    }
                }
                app_data.set_reporte_add_filtered_obras(ModelRc::new(VecModel::from(obras_vec)));
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_select_add_obra(move |obra_id, obra_label| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_reporte_add_selected_obra_id(obra_id.clone());
                app_data.set_reporte_add_selected_obra_label(obra_label.clone());
                app_data.set_reporte_add_search_query(obra_label);
                app_data.set_reporte_add_dropdown_open(false);

                let state = session_clone.lock().unwrap();
                let cur_id_str = app_data.get_reporte_add_row_id().to_string();
                if let Ok(cur_uuid) = Uuid::parse_str(&cur_id_str) {
                    if let Some(row) = state.all_rows.iter().find(|r| r.id_reporte_liq == cur_uuid) {
                        let new_oid = match Uuid::parse_str(&obra_id) {
                            Ok(u) => u,
                            Err(_) => return,
                        };
                        let new_obra = match state.obras_options.iter().find(|o| o.id == new_oid) {
                            Some(o) => o,
                            None => return,
                        };

                        let vigencia_str = match (new_obra.fecha_inicio, new_obra.fecha_fin) {
                            (Some(ini), Some(fin)) => format!("Vigencia: {} al {}", ini.format("%d/%m/%Y"), fin.format("%d/%m/%Y")),
                            (Some(ini), None) => format!("Inicio: {}", ini.format("%d/%m/%Y")),
                            (None, Some(fin)) => format!("Fin: {}", fin.format("%d/%m/%Y")),
                            (None, None) => "Sin fechas de vigencia registradas".to_string(),
                        };

                        let dias_en_mes = parse_month_and_year(&row.mes_periodo, row.anio_periodo, &row.periodo_str)
                            .and_then(|(m, y)| calculate_obra_days_in_month(m, y, new_obra.fecha_inicio, new_obra.fecha_fin));

                        let info = if let Some(d) = dias_en_mes {
                            format!("📅 {} • Días activos de la obra en este mes: {} días", vigencia_str, d)
                        } else {
                            format!("📅 {}", vigencia_str)
                        };
                        app_data.set_reporte_add_obra_vigencia_info(info.into());

                        app_data.set_reporte_add_alert_show(false);

                        let dias_avail = app_data.get_reporte_add_dias_available();
                        let max_dias = if let Some(d) = dias_en_mes {
                            (dias_avail as i16).min(d)
                        } else {
                            dias_avail as i16
                        };
                        app_data.set_reporte_add_dias_obra_val(max_dias.max(0).to_string().into());
                    }
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_toggle_add_dropdown(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let new_val = !app_data.get_reporte_add_dropdown_open();
                app_data.set_reporte_add_dropdown_open(new_val);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_submit_add_obra(move |row_id_str, obra_id_str, dias_obra_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let row_uuid = match Uuid::parse_str(&row_id_str) {
                    Ok(u) => u,
                    Err(_) => return,
                };
                let obra_uuid = match Uuid::parse_str(obra_id_str.trim()) {
                    Ok(u) => u,
                    Err(_) => {
                        crate::show_toast_message(&ui, "Por favor selecciona una obra válida.");
                        return;
                    }
                };
                let dias_obra: i16 = match dias_obra_str.trim().parse() {
                    Ok(d) if d > 0 && d <= 31 => d,
                    _ => {
                        crate::show_toast_message(&ui, "Los días de obra deben ser un número entero entre 1 y 31.");
                        return;
                    }
                };

                let parent_id = {
                    let state = session_clone.lock().unwrap();
                    match state.all_rows.iter().find(|r| r.id_reporte_liq == row_uuid) {
                        Some(r) => r.id_parent_reporte_liq,
                        None => {
                            crate::show_toast_message(&ui, "No se encontró el registro base del trabajador.");
                            return;
                        }
                    }
                };

                // Validaciones en cliente antes de BD
                {
                    let state = session_clone.lock().unwrap();
                    let cur_row = state.all_rows.iter().find(|r| r.id_reporte_liq == row_uuid).unwrap();
                    let worker_rows: Vec<&ReporteLiquidacionRowDb> = state.all_rows.iter()
                        .filter(|r| r.id_parent_reporte_liq == parent_id)
                        .collect();

                    let sum_used: i16 = worker_rows.iter().map(|r| r.dias_obra).sum();
                    if sum_used + dias_obra > cur_row.dias_cedula {
                        let err_msg = format!(
                            "La suma de días asignados ({} existentes + {} nuevos = {}) supera los días de cédula del trabajador ({}).",
                            sum_used, dias_obra, sum_used + dias_obra, cur_row.dias_cedula
                        );
                        let app_data = ui.global::<AppData>();
                        app_data.set_reporte_add_alert_show(true);
                        app_data.set_reporte_add_alert_text(err_msg.clone().into());
                        app_data.set_reporte_add_alert_type("error".into());
                        crate::show_toast_message(&ui, &err_msg);
                        return;
                    }

                    let new_obra = match state.obras_options.iter().find(|o| o.id == obra_uuid) {
                        Some(o) => o,
                        None => {
                            crate::show_toast_message(&ui, "La obra seleccionada no existe.");
                            return;
                        }
                    };

                    if let Some((m, y)) = parse_month_and_year(&cur_row.mes_periodo, cur_row.anio_periodo, &cur_row.periodo_str) {
                        if let Some(tope_mes) = calculate_obra_days_in_month(m, y, new_obra.fecha_inicio, new_obra.fecha_fin) {
                            if dias_obra > tope_mes {
                                let err_msg = format!("Los días asignados ({}) superan los días activos de la obra en el periodo ({} días).", dias_obra, tope_mes);
                                let app_data = ui.global::<AppData>();
                                app_data.set_reporte_add_alert_show(true);
                                app_data.set_reporte_add_alert_text(err_msg.clone().into());
                                app_data.set_reporte_add_alert_type("error".into());
                                crate::show_toast_message(&ui, &err_msg);
                                return;
                            }
                        }
                    }

                    for other in &worker_rows {
                        if let Some(other_oid) = other.id_registro_obra {
                            if other_oid == obra_uuid {
                                let err_msg = format!("El trabajador ya está vinculado a la obra '{}' en este periodo.", other.obra_label);
                                let app_data = ui.global::<AppData>();
                                app_data.set_reporte_add_alert_show(true);
                                app_data.set_reporte_add_alert_text(err_msg.clone().into());
                                app_data.set_reporte_add_alert_type("error".into());
                                crate::show_toast_message(&ui, &err_msg);
                                return;
                            }
                        }
                    }
                }

                // Ejecutar inserción en base de datos
                if let Some(ref pool) = pool_clone {
                    let mut client = match pool.get() {
                        Ok(c) => c,
                        Err(e) => {
                            crate::show_toast_message(&ui, &format!("Error conectando a BD: {}", e));
                            return;
                        }
                    };

                    match crate::db::reporte_liquidaciones::add_obra_vinculacion_for_employee(
                        &mut client,
                        parent_id,
                        obra_uuid,
                        dias_obra,
                    ) {
                        Ok(new_row_db) => {
                            {
                                let mut state = session_clone.lock().unwrap();
                                state.all_rows.push(new_row_db);
                                apply_reporte_filtering(&mut state);
                                render_reporte_ui(&ui, &state);
                            }
                            let app_data = ui.global::<AppData>();
                            app_data.set_show_reporte_add_obra_modal(false);
                            app_data.set_reporte_add_alert_show(false);
                            crate::show_toast_message(&ui, "Trabajador vinculado exitosamente a la obra adicional.");
                        }
                        Err(e) => {
                            let app_data = ui.global::<AppData>();
                            app_data.set_reporte_add_alert_show(true);
                            app_data.set_reporte_add_alert_text(e.clone().into());
                            app_data.set_reporte_add_alert_type("error".into());
                            crate::show_toast_message(&ui, &format!("Error vinculando obra: {}", e));
                        }
                    }
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_delete_split_row(move |id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let row_uuid = match Uuid::parse_str(&id_str) {
                    Ok(u) => u,
                    Err(_) => return,
                };

                let can_delete = {
                    let state = session_clone.lock().unwrap();
                    if let Some(r) = state.all_rows.iter().find(|row| row.id_reporte_liq == row_uuid) {
                        let count = state.all_rows.iter()
                            .filter(|x| x.id_parent_reporte_liq == r.id_parent_reporte_liq)
                            .count();
                        count > 1
                    } else {
                        false
                    }
                };

                if !can_delete {
                    crate::show_toast_message(&ui, "No se puede eliminar la única fila del empleado. Si deseas desvincular la obra, modifícala a 'Sin Asignar'.");
                    return;
                }

                if let Some(ref pool) = pool_clone {
                    let mut client = match pool.get() {
                        Ok(c) => c,
                        Err(e) => {
                            crate::show_toast_message(&ui, &format!("Error conectando a BD: {}", e));
                            return;
                        }
                    };

                    match crate::db::reporte_liquidaciones::delete_obra_vinculacion_detail(&mut client, row_uuid) {
                        Ok(_) => {
                            {
                                let mut state = session_clone.lock().unwrap();
                                state.all_rows.retain(|r| r.id_reporte_liq != row_uuid);
                                apply_reporte_filtering(&mut state);
                                render_reporte_ui(&ui, &state);
                            }
                            crate::show_toast_message(&ui, "Vinculación de obra eliminada correctamente.");
                        }
                        Err(e) => {
                            crate::show_toast_message(&ui, &format!("Error eliminando vinculación: {}", e));
                        }
                    }
                }
            }
        });
    }

    // 17. Batch: Solicitar confirmación para igualar Días de Obra a Cédula
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        let request_fn = move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let state = session_clone.lock().unwrap();

                let count = if !state.selected_ids.is_empty() {
                    state.selected_ids.len()
                } else {
                    state.filtered_rows.len()
                };

                if count == 0 {
                    crate::show_toast_message(&ui, "No hay trabajadores seleccionados para modificar.");
                    return;
                }

                let msg = if !state.selected_ids.is_empty() {
                    format!("¿Estás seguro de asignar los Días Obra máximos permitidos a los {} trabajadores seleccionados en la base de datos?", count)
                } else {
                    format!("¿Estás seguro de asignar los Días Obra máximos permitidos a los {} trabajadores del filtro actual en la base de datos?", count)
                };

                app_data.set_reporte_confirm_batch_msg(msg.into());
                app_data.set_show_reporte_confirm_batch_dias_modal(true);
            }
        };

        let req_clone = request_fn.clone();
        app_logic.on_reporte_request_confirm_igualar_dias(move || {
            crate::app_logger::log_ui_event(
                "Reporte Liquidaciones",
                "Barra Acciones Masivas",
                "Click Botón Igualar Días",
                "Solicitando modal de confirmación para asignar Días Obra máximos",
            );
            req_clone();
        });
        app_logic.on_reporte_batch_igualar_dias(move || {
            crate::app_logger::log_ui_event(
                "Reporte Liquidaciones",
                "Barra Acciones Masivas",
                "Click Botón Batch Igualar Días",
                "Invocando confirmación de igualar días",
            );
            request_fn();
        });
    }

    // 18. Cancelar confirmación de igualar días
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_cancel_confirm_igualar_dias(move || {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Reporte Liquidaciones",
                    "Modal Confirmar Igualar Días",
                    "Click Botón Cancelar",
                    "Cerrando modal sin cambios",
                );
                ui.global::<AppData>().set_show_reporte_confirm_batch_dias_modal(false);
            }
        });
    }

    // 19. Ejecutar confirmación de igualar días
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_execute_confirm_igualar_dias(move || {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Reporte Liquidaciones",
                    "Modal Confirmar Igualar Días",
                    "Click Botón Confirmar Igualar Días",
                    "Ejecutando asignación masiva de días de obra máximos",
                );
                ui.global::<AppData>().set_show_reporte_confirm_batch_dias_modal(false);

                let mut count_modified = 0;

                let rows_for_db: Vec<(Uuid, i16, i16, Option<Uuid>)> = {
                    let mut state = session_clone.lock().unwrap();
                    let ids_to_change: HashSet<Uuid> = if state.selected_ids.is_empty() {
                        state.filtered_rows.iter()
                            .map(|r| r.id_reporte_liq)
                            .collect()
                    } else {
                        state.selected_ids.clone()
                    };

                    let obras_map: HashMap<Uuid, (Option<NaiveDate>, Option<NaiveDate>)> = state
                        .obras_options
                        .iter()
                        .map(|o| (o.id, (o.fecha_inicio, o.fecha_fin)))
                        .collect();

                    let movements_map = build_all_employee_movements_map(&state.all_rows);
                    let mut modified_ids = HashSet::new();

                    for row in state.all_rows.iter_mut() {
                        if ids_to_change.contains(&row.id_reporte_liq) {
                            let (obra_ini, obra_fin) = row.id_registro_obra
                                .and_then(|oid| obras_map.get(&oid).copied())
                                .unwrap_or((None, None));

                            let key = (row.id_empleado, row.mes_periodo.trim().to_uppercase(), row.anio_periodo);
                            let movs = movements_map.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);

                            // Validación de compatibilidad: si la obra no coincide con los periodos activos, no modificar nada
                            if row.id_registro_obra.is_some() {
                                if validate_obra_assignment_for_employee_movements(
                                    obra_ini,
                                    obra_fin,
                                    movs,
                                    &row.mes_periodo,
                                    row.anio_periodo,
                                    &row.periodo_str,
                                ).is_err() {
                                    continue;
                                }
                            }

                            let (max_permitido, _) = calculate_max_dias_obra_for_employee_movements(
                                row.dias_cedula,
                                row.lic,
                                row.inc,
                                row.aus,
                                &row.mes_periodo,
                                row.anio_periodo,
                                &row.periodo_str,
                                obra_ini,
                                obra_fin,
                                movs,
                            );

                            // Si tiene movimientos y el tope es 0, no modificar nada
                            if max_permitido == 0 && !movs.is_empty() {
                                continue;
                            }

                            row.dias_obra = max_permitido;
                            recalculate_row_in_memory(row);
                            count_modified += 1;
                            modified_ids.insert(row.id_reporte_liq);
                        }
                    }

                    let rows = state
                        .all_rows
                        .iter()
                        .filter(|r| modified_ids.contains(&r.id_reporte_liq))
                        .map(|r| (r.id_reporte_liq, r.dias_cedula, r.dias_obra, r.id_registro_obra))
                        .collect();

                    apply_reporte_filtering(&mut state);
                    render_reporte_ui(&ui, &state);

                    rows
                };

                // Persistir en PostgreSQL
                if let Some(ref pool) = pool_clone {
                    let pool_bg = pool.clone();
                    std::thread::spawn(move || {
                        if let Ok(mut client) = pool_bg.get() {
                            if let Err(e) = batch_save_reporte_row_updates(&mut client, &rows_for_db) {
                                log::error!("Error persistiendo igualar días en BD: {}", e);
                            }
                        }
                    });
                }

                crate::show_toast_message(
                    &ui,
                    &format!("Se asignaron los días de obra máximos a {} registros.", count_modified),
                );
            }
        });
    }

    // Batch: Poner Días Obra en 0 para la selección
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_batch_poner_dias_cero(move || {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Reporte Liquidaciones",
                    "Barra Acciones Masivas",
                    "Click Botón Días Obra en 0",
                    "Asignando 0 días de obra a la selección actual",
                );
                let mut count_modified = 0;

                let rows_for_db: Vec<(Uuid, i16, i16, Option<Uuid>)> = {
                    let mut state = session_clone.lock().unwrap();
                    let ids_to_change: HashSet<Uuid> = if state.selected_ids.is_empty() {
                        state.filtered_rows.iter()
                            .filter(|r| !r.clave.trim().eq_ignore_ascii_case("baja"))
                            .map(|r| r.id_reporte_liq)
                            .collect()
                    } else {
                        state.selected_ids.clone()
                    };

                    for row in state.all_rows.iter_mut() {
                        if ids_to_change.contains(&row.id_reporte_liq) {
                            row.dias_obra = 0;
                            recalculate_row_in_memory(row);
                            count_modified += 1;
                        }
                    }

                    let rows = state
                        .all_rows
                        .iter()
                        .filter(|r| ids_to_change.contains(&r.id_reporte_liq))
                        .map(|r| (r.id_reporte_liq, r.dias_cedula, r.dias_obra, r.id_registro_obra))
                        .collect();

                    apply_reporte_filtering(&mut state);
                    render_reporte_ui(&ui, &state);

                    rows
                };

                // Persistir en PostgreSQL
                if let Some(ref pool) = pool_clone {
                    let pool_bg = pool.clone();
                    std::thread::spawn(move || {
                        if let Ok(mut client) = pool_bg.get() {
                            if let Err(e) = batch_save_reporte_row_updates(&mut client, &rows_for_db) {
                                log::error!("Error persistiendo días obra 0 en BD: {}", e);
                            }
                        }
                    });
                }

                crate::show_toast_message(
                    &ui,
                    &format!("Se asignaron 0 días de obra a {} registros.", count_modified),
                );
            }
        });
    }

    // Batch: Asignar Días Obra arbitrarios (incluyendo 0) a la selección
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_batch_apply_dias_obra(move |dias_str| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_input(
                    "Reporte Liquidaciones",
                    "Barra Acciones Masivas",
                    "Input Días Obra Masivo",
                    "dias_obra_masivo",
                    &dias_str,
                );
                let trimmed = dias_str.trim();
                let target_dias = match trimmed.parse::<i16>() {
                    Ok(d) if (0..=31).contains(&d) => d,
                    _ => {
                        crate::app_logger::log_ui_error(
                            "Reporte Liquidaciones",
                            "Barra Acciones Masivas",
                            "Validación Días Inválidos",
                            &format!("Número de días inválido: '{}'", dias_str),
                        );
                        crate::show_toast_message(&ui, "Por favor ingresa un número de días válido entre 0 y 31.");
                        return;
                    }
                };

                let mut count_modified = 0;

                let rows_for_db: Vec<(Uuid, i16, i16, Option<Uuid>)> = {
                    let mut state = session_clone.lock().unwrap();
                    let ids_to_change: HashSet<Uuid> = if state.selected_ids.is_empty() {
                        state.filtered_rows.iter()
                            .map(|r| r.id_reporte_liq)
                            .collect()
                    } else {
                        state.selected_ids.clone()
                    };

                    let obras_map: HashMap<Uuid, (Option<NaiveDate>, Option<NaiveDate>)> = state
                        .obras_options
                        .iter()
                        .map(|o| (o.id, (o.fecha_inicio, o.fecha_fin)))
                        .collect();

                    let movements_map = build_all_employee_movements_map(&state.all_rows);
                    let mut modified_ids = HashSet::new();

                    for row in state.all_rows.iter_mut() {
                        if ids_to_change.contains(&row.id_reporte_liq) {
                            let (obra_ini, obra_fin) = row.id_registro_obra
                                .and_then(|oid| obras_map.get(&oid).copied())
                                .unwrap_or((None, None));

                            let key = (row.id_empleado, row.mes_periodo.trim().to_uppercase(), row.anio_periodo);
                            let movs = movements_map.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);

                            if row.id_registro_obra.is_some() {
                                if validate_obra_assignment_for_employee_movements(
                                    obra_ini,
                                    obra_fin,
                                    movs,
                                    &row.mes_periodo,
                                    row.anio_periodo,
                                    &row.periodo_str,
                                ).is_err() {
                                    continue;
                                }
                            }

                            let (max_permitido, _) = calculate_max_dias_obra_for_employee_movements(
                                row.dias_cedula,
                                row.lic,
                                row.inc,
                                row.aus,
                                &row.mes_periodo,
                                row.anio_periodo,
                                &row.periodo_str,
                                obra_ini,
                                obra_fin,
                                movs,
                            );

                            if max_permitido == 0 && !movs.is_empty() {
                                continue;
                            }

                            row.dias_obra = target_dias.min(max_permitido);
                            recalculate_row_in_memory(row);
                            count_modified += 1;
                            modified_ids.insert(row.id_reporte_liq);
                        }
                    }

                    let rows = state
                        .all_rows
                        .iter()
                        .filter(|r| modified_ids.contains(&r.id_reporte_liq))
                        .map(|r| (r.id_reporte_liq, r.dias_cedula, r.dias_obra, r.id_registro_obra))
                        .collect();

                    apply_reporte_filtering(&mut state);
                    render_reporte_ui(&ui, &state);

                    rows
                };

                // Limpiar input
                ui.global::<AppData>().set_reporte_batch_dias_obra_input("".into());

                // Persistir en PostgreSQL
                if let Some(ref pool) = pool_clone {
                    let pool_bg = pool.clone();
                    std::thread::spawn(move || {
                        if let Ok(mut client) = pool_bg.get() {
                            if let Err(e) = batch_save_reporte_row_updates(&mut client, &rows_for_db) {
                                log::error!("Error persistiendo asignación de días obra en BD: {}", e);
                            }
                        }
                    });
                }

                crate::show_toast_message(
                    &ui,
                    &format!("Se asignaron {} días de obra a {} registros.", target_dias, count_modified),
                );
            }
        });
    }

    // Helper: Calcula la lista de trabajadores para el modal masivo con su respectivo Tope Máx
    fn compute_batch_workers_list(
        state: &ReporteSessionState,
        selected_obra_id_str: &str,
        forced_cedula: Option<i16>,
    ) -> Vec<ReporteBatchWorkerItem> {
        let movs_map = build_all_employee_movements_map(&state.all_rows);

        let chosen_obra_opt: Option<Option<&ObraOptionDb>> = if selected_obra_id_str == "__KEEP__" {
            None
        } else if selected_obra_id_str == "__NONE__" || selected_obra_id_str.is_empty() {
            Some(None)
        } else {
            let oid_opt = Uuid::parse_str(selected_obra_id_str).ok();
            Some(oid_opt.and_then(|oid| state.obras_options.iter().find(|o| o.id == oid)))
        };

        let mut batch_workers = Vec::new();
        for r in state.all_rows.iter() {
            if state.selected_ids.contains(&r.id_reporte_liq) {
                let cur_obra = state.obras_options.iter().find(|o| Some(o.id) == r.id_registro_obra || (!r.obra_numero.trim().is_empty() && r.obra_numero.trim().eq_ignore_ascii_case(o.numero.trim())));

                let target_obra = match chosen_obra_opt {
                    None => cur_obra,
                    Some(maybe_o) => maybe_o,
                };

                let movs = movs_map
                    .get(&(r.id_empleado, r.mes_periodo.trim().to_uppercase(), r.anio_periodo))
                    .cloned()
                    .unwrap_or_default();

                let (m_num, y_num) = parse_month_and_year(&r.mes_periodo, r.anio_periodo, &r.periodo_str).unwrap_or((1, 2024));
                let days_in_month = crate::db::reporte_liquidaciones::get_month_date_range(m_num, y_num as i32).map(|(_, _, d)| d as i32).unwrap_or(30);
                let sum_incidencias = (r.lic + r.inc + r.aus).round() as i32;

                let cedula_val = forced_cedula.unwrap_or(r.dias_cedula) as i32;

                let total_disp = if cedula_val > 0 && cedula_val < days_in_month {
                    (cedula_val - sum_incidencias).max(0)
                } else {
                    (days_in_month - sum_incidencias).max(0)
                };

                let tope_max: i32 = match target_obra {
                    Some(o) => {
                        let d_act = match calculate_obra_days_in_month(m_num, y_num, o.fecha_inicio, o.fecha_fin) {
                            Some(d) => d as i32,
                            None => days_in_month,
                        };
                        if validate_obra_assignment_for_employee_movements(
                            o.fecha_inicio,
                            o.fecha_fin,
                            &movs,
                            &r.mes_periodo,
                            r.anio_periodo,
                            &r.periodo_str,
                        ).is_err() {
                            0
                        } else {
                            let (max_dias, _) = calculate_max_dias_obra_for_employee_movements(
                                cedula_val as i16,
                                r.lic,
                                r.inc,
                                r.aus,
                                &r.mes_periodo,
                                r.anio_periodo,
                                &r.periodo_str,
                                o.fecha_inicio,
                                o.fecha_fin,
                                &movs,
                            );
                            (max_dias as i32).min(d_act).min(total_disp)
                        }
                    }
                    None => total_disp,
                };

                let lic_str = if r.lic > 0.001 { format!("{:.0}", r.lic) } else { "0".to_string() };
                let inc_str = if r.inc > 0.001 { format!("{:.0}", r.inc) } else { "0".to_string() };
                let aus_str = if r.aus > 0.001 { format!("{:.0}", r.aus) } else { "0".to_string() };
                let incidencias_str = if sum_incidencias > 0 {
                    format!("LIC: {} • INC: {} • AUS: {}", lic_str, inc_str, aus_str)
                } else {
                    "Sin incidencias".to_string()
                };

                batch_workers.push(ReporteBatchWorkerItem {
                    id: r.id_reporte_liq.to_string().into(),
                    id_parent: r.id_parent_reporte_liq.to_string().into(),
                    nss: r.nss.clone().into(),
                    nombre: r.nombre_trabajador.clone().into(),
                    periodo: r.periodo_str.clone().into(),
                    obra_label: r.obra_label.clone().into(),
                    dias_cedula: cedula_val,
                    dias_obra: r.dias_obra as i32,
                    dias_obra_str: r.dias_obra.to_string().into(),
                    tope_max,
                    total_disponibles: total_disp,
                    has_error: (r.dias_obra as i32) > tope_max,
                    lic_info: lic_str.into(),
                    inc_info: inc_str.into(),
                    aus_info: aus_str.into(),
                    incidencias_str: incidencias_str.into(),
                });
            }
        }
        batch_workers
    }

    // Batch Modal: Abrir modal de modificación masiva de obra y días
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_open_batch_edit_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let state = session_clone.lock().unwrap();
                if state.selected_ids.is_empty() {
                    crate::show_toast_message(&ui, "No hay empleados seleccionados para modificar.");
                    return;
                }

                let batch_workers = compute_batch_workers_list(&state, "__KEEP__", None);
                let count = batch_workers.len() as i32;
                let mut obras_vec = Vec::with_capacity(state.obras_options.len());
                let mut multi_obras_vec = Vec::with_capacity(state.obras_options.len());
                for o in &state.obras_options {
                    obras_vec.push(ReporteObraItem {
                        id: o.id.to_string().into(),
                        numero: o.numero.clone().into(),
                        label: o.label.clone().into(),
                    });

                    let vigencia_str = match (o.fecha_inicio, o.fecha_fin) {
                        (Some(ini), Some(fin)) => format!("{} al {}", ini.format("%d/%m/%Y"), fin.format("%d/%m/%Y")),
                        (Some(ini), None) => format!("Desde {}", ini.format("%d/%m/%Y")),
                        (None, Some(fin)) => format!("Hasta {}", fin.format("%d/%m/%Y")),
                        (None, None) => String::new(),
                    };
                    let tope_est = if let Some(first_sel_id) = state.selected_ids.iter().next() {
                        if let Some(r) = state.all_rows.iter().find(|x| x.id_reporte_liq == *first_sel_id) {
                            parse_month_and_year(&r.mes_periodo, r.anio_periodo, &r.periodo_str)
                                .and_then(|(m, y)| calculate_obra_days_in_month(m, y, o.fecha_inicio, o.fecha_fin))
                                .unwrap_or(30)
                        } else {
                            30
                        }
                    } else {
                        30
                    };
                    multi_obras_vec.push(ReporteBatchObraItem {
                        id: o.id.to_string().into(),
                        numero: o.numero.clone().into(),
                        label: o.label.clone().into(),
                        vigencia_str: vigencia_str.into(),
                        dias_tope_est: tope_est as i32,
                        selected: false,
                    });
                }

                let app_data = ui.global::<AppData>();
                app_data.set_reporte_batch_edit_worker_count(count);
                app_data.set_reporte_batch_edit_workers(ModelRc::new(VecModel::from(batch_workers)));
                app_data.set_reporte_batch_edit_filtered_obras(ModelRc::new(VecModel::from(obras_vec)));
                app_data.set_reporte_batch_edit_obras_multiselect(ModelRc::new(VecModel::from(multi_obras_vec)));
                app_data.set_reporte_batch_edit_selected_obras_count(0);
                app_data.set_reporte_batch_edit_multi_obra_enabled(false);
                app_data.set_reporte_batch_edit_selected_obra_id("__KEEP__".into());
                app_data.set_reporte_batch_edit_selected_obra_label("🔄 Mantener Obras Actuales (Sin Cambios)".into());
                app_data.set_reporte_batch_edit_search_query("🔄 Mantener Obras Actuales (Sin Cambios)".into());
                app_data.set_reporte_batch_edit_dias_cedula_val("".into());
                app_data.set_reporte_batch_edit_dias_obra_val("".into());
                app_data.set_reporte_batch_edit_change_obra(false);
                app_data.set_reporte_batch_edit_change_dias_cedula(false);
                app_data.set_reporte_batch_edit_change_dias_obra(false);
                app_data.set_reporte_batch_edit_dropdown_open(false);
                app_data.set_reporte_batch_edit_alert_show(false);
                app_data.set_reporte_batch_edit_alert_text("".into());
                app_data.set_show_reporte_batch_edit_modal(true);
            }
        });
    }

    // Batch Modal: Cerrar modal de modificación masiva
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_close_batch_edit_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_reporte_batch_edit_alert_show(false);
                app_data.set_show_reporte_batch_edit_modal(false);
                app_data.set_reporte_batch_edit_dropdown_open(false);
            }
        });
    }

    // Batch Modal: Alternar modo de asignación 1 obra vs múltiples obras
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_batch_edit_set_multi_mode(move |enabled| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_reporte_batch_edit_multi_obra_enabled(enabled);
                if enabled {
                    app_data.set_reporte_batch_edit_change_obra(true);
                    app_data.set_reporte_batch_edit_dropdown_open(false);
                }
            }
        });
    }

    // Batch Modal: Seleccionar/deseleccionar obra en modo multi-obra
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_batch_edit_toggle_obra(move |obra_id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let current_model = app_data.get_reporte_batch_edit_obras_multiselect();
                let mut updated_vec = Vec::new();
                let mut sel_count = 0;
                let mut selected_uuids = Vec::new();

                for i in 0..current_model.row_count() {
                    if let Some(mut item) = current_model.row_data(i) {
                        if item.id == obra_id_str {
                            item.selected = !item.selected;
                        }
                        if item.selected {
                            sel_count += 1;
                            if let Ok(uid) = Uuid::parse_str(&item.id) {
                                selected_uuids.push(uid);
                            }
                        }
                        updated_vec.push(item);
                    }
                }

                // Recalcular dinámicamente los topes de TODOS los trabajadores seleccionados
                // iterando individualmente sobre su mes, año e incidencias
                let workers_model = app_data.get_reporte_batch_edit_workers();
                let mut workers = Vec::new();
                for i in 0..workers_model.row_count() {
                    if let Some(w) = workers_model.row_data(i) {
                        workers.push(w);
                    }
                }

                {
                    let state = session_clone.lock().unwrap();

                    for worker in &mut workers {
                        let w_uuid = Uuid::parse_str(&worker.id).ok();
                        if let Some(r) = state.all_rows.iter().find(|x| Some(x.id_reporte_liq) == w_uuid) {
                            let (m_num, y_num) = parse_month_and_year(&r.mes_periodo, r.anio_periodo, &r.periodo_str).unwrap_or((1, 2024));
                            let days_in_month = crate::db::reporte_liquidaciones::get_month_date_range(m_num, y_num as i32).map(|(_, _, d)| d as i32).unwrap_or(30);
                            let sum_incidencias = (r.lic + r.inc + r.aus).round() as i32;
                            let cedula_val = worker.dias_cedula;

                            let total_disp = if cedula_val > 0 && cedula_val < days_in_month {
                                (cedula_val - sum_incidencias).max(0)
                            } else {
                                (days_in_month - sum_incidencias).max(0)
                            };

                            worker.total_disponibles = total_disp;

                            if selected_uuids.is_empty() {
                                worker.tope_max = total_disp;
                            } else {
                                let mut combined_active = 0i32;
                                for sel_id in &selected_uuids {
                                    if let Some(o) = state.obras_options.iter().find(|o| o.id == *sel_id) {
                                        let d_act = match calculate_obra_days_in_month(m_num, y_num, o.fecha_inicio, o.fecha_fin) {
                                            Some(d) => d as i32,
                                            None => days_in_month,
                                        };
                                        combined_active = combined_active.max(d_act);
                                    }
                                }
                                worker.tope_max = total_disp.min(combined_active);
                            }

                            worker.has_error = worker.dias_obra > worker.tope_max;
                        }
                    }
                }

                app_data.set_reporte_batch_edit_workers(ModelRc::new(VecModel::from(workers)));
                app_data.set_reporte_batch_edit_obras_multiselect(ModelRc::new(VecModel::from(updated_vec)));
                app_data.set_reporte_batch_edit_selected_obras_count(sel_count);
                if sel_count > 0 {
                    app_data.set_reporte_batch_edit_alert_show(false);
                    app_data.set_reporte_batch_edit_change_obra(true);
                }
            }
        });
    }

    // Batch Modal: Edición reactiva inline de Días Obra en la tabla de trabajadores (Regla Amaya Aranda)
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_batch_worker_dias_changed(move |worker_id_str, new_dias_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let model = app_data.get_reporte_batch_edit_workers();
                let mut workers = Vec::new();
                for i in 0..model.row_count() {
                    if let Some(w) = model.row_data(i) {
                        workers.push(w);
                    }
                }

                let parsed_val = new_dias_str.trim().parse::<i32>().unwrap_or(0);
                redistribute_batch_workers_days(&mut workers, &worker_id_str, parsed_val);
                app_data.set_reporte_batch_edit_workers(ModelRc::new(VecModel::from(workers)));
            }
        });
    }

    // Batch Modal: Filtrar obras en modal masivo
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_filter_batch_edit_obras(move |q| {
            if let Some(ui) = ui_weak.upgrade() {
                let q_lower = q.trim().to_lowercase();
                let state = session_clone.lock().unwrap();
                let mut obras_vec = Vec::new();
                for o in &state.obras_options {
                    if q_lower.is_empty() || o.numero.to_lowercase().contains(&q_lower) || o.label.to_lowercase().contains(&q_lower) {
                        obras_vec.push(ReporteObraItem {
                            id: o.id.to_string().into(),
                            numero: o.numero.clone().into(),
                            label: o.label.clone().into(),
                        });
                    }
                }
                ui.global::<AppData>().set_reporte_batch_edit_filtered_obras(ModelRc::new(VecModel::from(obras_vec)));
            }
        });
    }

    // Batch Modal: Seleccionar obra en modal masivo
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_select_batch_edit_obra(move |obra_id, obra_label| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let is_keep = obra_id.as_str() == "__KEEP__";
                app_data.set_reporte_batch_edit_selected_obra_id(obra_id.clone());
                app_data.set_reporte_batch_edit_selected_obra_label(obra_label.clone());
                app_data.set_reporte_batch_edit_search_query(obra_label);
                app_data.set_reporte_batch_edit_change_obra(!is_keep);
                app_data.set_reporte_batch_edit_dropdown_open(false);

                let state = session_clone.lock().unwrap();
                let cedula_text = app_data.get_reporte_batch_edit_dias_cedula_val().trim().to_string();
                let forced_cedula = if app_data.get_reporte_batch_edit_change_dias_cedula() && !cedula_text.is_empty() {
                    cedula_text.parse::<i16>().ok()
                } else {
                    None
                };

                let batch_workers = compute_batch_workers_list(&state, obra_id.as_str(), forced_cedula);
                app_data.set_reporte_batch_edit_workers(ModelRc::new(VecModel::from(batch_workers.clone())));

                // Si hay trabajadores para los cuales esta obra da tope 0 por incompatibilidad de fechas, informar en la alerta del modal:
                let zero_count = batch_workers.iter().filter(|w| w.tope_max == 0).count();
                if !is_keep && obra_id.as_str() != "__NONE__" && zero_count > 0 {
                    let msg = if zero_count == 1 {
                        "⚠️ Atención: 1 trabajador seleccionado no tiene días activos compatibles con esta obra (Tope Máx: 0 días).".to_string()
                    } else {
                        format!("⚠️ Atención: {} trabajadores seleccionados no tienen días activos compatibles con esta obra (Tope Máx: 0 días).", zero_count)
                    };
                    app_data.set_reporte_batch_edit_alert_text(msg.into());
                    app_data.set_reporte_batch_edit_alert_type("warning".into());
                    app_data.set_reporte_batch_edit_alert_show(true);
                } else {
                    app_data.set_reporte_batch_edit_alert_show(false);
                }

                let mut obras_vec = Vec::with_capacity(state.obras_options.len());
                for o in &state.obras_options {
                    obras_vec.push(ReporteObraItem {
                        id: o.id.to_string().into(),
                        numero: o.numero.clone().into(),
                        label: o.label.clone().into(),
                    });
                }
                app_data.set_reporte_batch_edit_filtered_obras(ModelRc::new(VecModel::from(obras_vec)));
            }
        });
    }

    // Batch Modal: Cambio en Días Cédula para recalcular tope máx en tiempo real
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_batch_edit_dias_cedula_changed(move |ced_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let cur_obra = app_data.get_reporte_batch_edit_selected_obra_id().to_string();
                let forced_cedula = ced_str.trim().parse::<i16>().ok();
                let state = session_clone.lock().unwrap();
                let batch_workers = compute_batch_workers_list(&state, &cur_obra, forced_cedula);
                app_data.set_reporte_batch_edit_workers(ModelRc::new(VecModel::from(batch_workers)));
            }
        });
    }

    // Batch Modal: Alternar desplegable en modal masivo
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_batch_edit_dropdown(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let new_state = !app_data.get_reporte_batch_edit_dropdown_open();
                app_data.set_reporte_batch_edit_dropdown_open(new_state);
                if new_state {
                    let state = session_clone.lock().unwrap();
                    let mut obras_vec = Vec::with_capacity(state.obras_options.len());
                    for o in &state.obras_options {
                        obras_vec.push(ReporteObraItem {
                            id: o.id.to_string().into(),
                            numero: o.numero.clone().into(),
                            label: o.label.clone().into(),
                        });
                    }
                    app_data.set_reporte_batch_edit_filtered_obras(ModelRc::new(VecModel::from(obras_vec)));
                }
            }
        });
    }

    // Batch Modal: Aplicar modificación masiva a todos los seleccionados
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_submit_batch_edit(move |obra_id_str, change_obra, dias_ced_str, change_ced, dias_obr_str, change_obr| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Reporte Liquidaciones",
                    "Modal Edición Masiva",
                    "Click Botón Guardar Cambios Masivos",
                    &format!("obra='{}' (cambiar={}), dias_ced='{}' (cambiar={}), dias_obr='{}' (cambiar={})",
                        obra_id_str, change_obra, dias_ced_str, change_ced, dias_obr_str, change_obr
                    ),
                );

                let app_data = ui.global::<AppData>();
                let is_multi_obra = app_data.get_reporte_batch_edit_multi_obra_enabled();

                if is_multi_obra {
                    let multi_model = app_data.get_reporte_batch_edit_obras_multiselect();
                    let mut selected_obra_uuids = Vec::new();
                    for i in 0..multi_model.row_count() {
                        if let Some(item) = multi_model.row_data(i) {
                            if item.selected {
                                if let Ok(u) = Uuid::parse_str(&item.id) {
                                    selected_obra_uuids.push(u);
                                }
                            }
                        }
                    }

                    if selected_obra_uuids.is_empty() {
                        app_data.set_reporte_batch_edit_alert_text("Por favor selecciona al menos 1 obra de la lista para vincular.".into());
                        app_data.set_reporte_batch_edit_alert_type("error".into());
                        app_data.set_reporte_batch_edit_alert_show(true);
                        crate::show_toast_message(&ui, "Selecciona al menos 1 obra para vincular a los trabajadores.");
                        return;
                    }

                    let target_ced_opt = if change_ced {
                        match dias_ced_str.trim().parse::<i16>() {
                            Ok(d) if (1..=31).contains(&d) => Some(d),
                            _ => {
                                crate::show_toast_message(&ui, "Los Días Cédula deben ser un número entre 1 y 31.");
                                return;
                            }
                        }
                    } else {
                        None
                    };

                    let target_obr_opt = if change_obr {
                        match dias_obr_str.trim().parse::<i16>() {
                            Ok(d) if (0..=31).contains(&d) => Some(d),
                            _ => {
                                crate::show_toast_message(&ui, "Los Días Obra deben ser un número entre 0 y 31.");
                                return;
                            }
                        }
                    } else {
                        None
                    };

                    let (parent_ids, revision_uuid) = {
                        let state = session_clone.lock().unwrap();
                        let mut p_set = HashSet::new();
                        for rid in &state.selected_ids {
                            if let Some(r) = state.all_rows.iter().find(|x| x.id_reporte_liq == *rid) {
                                p_set.insert(r.id_parent_reporte_liq);
                            }
                        }
                        let rev_uuid = state.current_revision_id.as_deref().and_then(|id_str| Uuid::parse_str(id_str).ok()).unwrap_or(Uuid::nil());
                        (p_set.into_iter().collect::<Vec<Uuid>>(), rev_uuid)
                    };

                    if parent_ids.is_empty() {
                        crate::show_toast_message(&ui, "No se encontraron empleados base para vincular.");
                        return;
                    }

                    if let Some(ref pool) = pool_clone {
                        let mut client = match pool.get() {
                            Ok(c) => c,
                            Err(e) => {
                                crate::show_toast_message(&ui, &format!("Error conectando a BD: {}", e));
                                return;
                            }
                        };

                        match crate::db::reporte_liquidaciones::batch_assign_multiple_obras_to_employees(
                            &mut client,
                            revision_uuid,
                            &parent_ids,
                            &selected_obra_uuids,
                            target_ced_opt,
                            target_obr_opt,
                        ) {
                            Ok(batch_res) => {
                                match crate::db::reporte_liquidaciones::fetch_reporte_liquidaciones(&mut client, revision_uuid) {
                                    Ok(refreshed_rows) => {
                                        let mut state = session_clone.lock().unwrap();
                                        state.all_rows = refreshed_rows;
                                        update_unique_empleados_catalog(&mut state, &ui.global::<AppData>());
                                        apply_reporte_filtering(&mut state);
                                        render_reporte_ui(&ui, &state);
                                    }
                                    Err(e) => {
                                        log::error!("Error refrescando filas tras asignación múltiple: {}", e);
                                    }
                                }

                                app_data.set_show_reporte_batch_edit_modal(false);
                                crate::show_toast_message(
                                    &ui,
                                    &format!("Se vincularon exitosamente {} obras a {} empleados.",
                                        batch_res.obras_linked_count, batch_res.employees_updated
                                    ),
                                );
                                return;
                            }
                            Err(err_msg) => {
                                app_data.set_reporte_batch_edit_alert_text(err_msg.clone().into());
                                app_data.set_reporte_batch_edit_alert_type("error".into());
                                app_data.set_reporte_batch_edit_alert_show(true);
                                crate::show_toast_message(&ui, &err_msg);
                                return;
                            }
                        }
                    }
                    return;
                }

                if !change_obra && !change_ced && !change_obr {
                    crate::app_logger::log_ui_error(
                        "Reporte Liquidaciones",
                        "Modal Edición Masiva",
                        "Validación Ningún Campo Seleccionado",
                        "El usuario no activó ningún campo a modificar",
                    );
                    crate::show_toast_message(
                        &ui,
                        "Por favor activa al menos un campo a modificar (Obra, Días Cédula o Días Obra).",
                    );
                    return;
                }

                let target_ced_opt = if change_ced {
                    match dias_ced_str.trim().parse::<i16>() {
                        Ok(d) if (1..=31).contains(&d) => Some(d),
                        _ => {
                            crate::app_logger::log_ui_error(
                                "Reporte Liquidaciones",
                                "Modal Edición Masiva",
                                "Validación Días Cédula Fuera de Rango",
                                &format!("Valor inválido: '{}'", dias_ced_str),
                            );
                            crate::show_toast_message(&ui, "Los Días Cédula deben ser un número entre 1 y 31.");
                            return;
                        }
                    }
                } else {
                    None
                };

                let target_obr_opt = if change_obr {
                    match dias_obr_str.trim().parse::<i16>() {
                        Ok(d) if (0..=31).contains(&d) => Some(d),
                        _ => {
                            crate::app_logger::log_ui_error(
                                "Reporte Liquidaciones",
                                "Modal Edición Masiva",
                                "Validación Días Obra Fuera de Rango",
                                &format!("Valor inválido: '{}'", dias_obr_str),
                            );
                            crate::show_toast_message(&ui, "Los Días Obra deben ser un número entre 0 y 31.");
                            return;
                        }
                    }
                } else {
                    None
                };

                let mut count = 0;
                let mut modified_fields = Vec::new();

                let rows_for_db: Vec<(Uuid, i16, i16, Option<Uuid>)> = {
                    let mut state = session_clone.lock().unwrap();
                    let ids_to_modify: HashSet<Uuid> = state.selected_ids.clone();
                    if ids_to_modify.is_empty() {
                        crate::show_toast_message(&ui, "No hay filas seleccionadas.");
                        return;
                    }

                    let obra_target: Option<Option<ObraOptionDb>> = if change_obra {
                        let trimmed = obra_id_str.trim();
                        if trimmed == "__NONE__" || trimmed.is_empty() {
                            Some(None) // Desvincular obra
                        } else if trimmed == "__KEEP__" {
                            None // No modificar obra
                        } else {
                            match Uuid::parse_str(trimmed) {
                                Ok(oid) => {
                                    let found = state.obras_options.iter().find(|o| o.id == oid).cloned();
                                    Some(found)
                                }
                                Err(_) => None,
                            }
                        }
                    } else {
                        None
                    };

                    if change_obra {
                        if let Some(ref opt) = obra_target {
                            if opt.is_some() {
                                modified_fields.push("Obra");
                            } else {
                                modified_fields.push("Desvincular Obra");
                            }
                        }
                    }
                    if target_ced_opt.is_some() {
                        modified_fields.push("Días Cédula");
                    }
                    if target_obr_opt.is_some() {
                        modified_fields.push("Días Obra");
                    }

                    let obras_map: HashMap<Uuid, (Option<NaiveDate>, Option<NaiveDate>)> = state
                        .obras_options
                        .iter()
                        .map(|o| (o.id, (o.fecha_inicio, o.fecha_fin)))
                        .collect();

                    let movements_map = build_all_employee_movements_map(&state.all_rows);
                    let mut modified_ids = HashSet::new();
                    let mut clamped_count = 0;

                    for row in state.all_rows.iter_mut() {
                        if ids_to_modify.contains(&row.id_reporte_liq) {
                            let key = (row.id_empleado, row.mes_periodo.trim().to_uppercase(), row.anio_periodo);
                            let movs = movements_map.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);

                            if change_obra {
                                if let Some(ref o_opt) = obra_target {
                                    match o_opt {
                                        Some(o) => {
                                            if validate_obra_assignment_for_employee_movements(
                                                o.fecha_inicio,
                                                o.fecha_fin,
                                                movs,
                                                &row.mes_periodo,
                                                row.anio_periodo,
                                                &row.periodo_str,
                                            ).is_err() {
                                                // La obra no es compatible con el trabajador (ej. inició después de su baja)
                                                continue;
                                            }
                                            row.id_registro_obra = Some(o.id);
                                            row.obra_numero = o.numero.clone();
                                            row.obra_label = o.label.clone();
                                            row.obra_creado_usuario = o.creado_usuario;
                                        }
                                        None => {
                                            row.id_registro_obra = None;
                                            row.obra_numero = String::new();
                                            row.obra_label = "⚠️ Sin Asignar (Pendiente)".to_string();
                                            row.obra_creado_usuario = false;
                                        }
                                    }
                                }
                            }

                            let (obra_ini, obra_fin) = row.id_registro_obra
                                .and_then(|oid| obras_map.get(&oid).copied())
                                .unwrap_or((None, None));

                            // Si tiene obra vinculada pero es incompatible con sus movimientos, omitir modificaciones para esta obra
                            if row.id_registro_obra.is_some() {
                                if validate_obra_assignment_for_employee_movements(
                                    obra_ini,
                                    obra_fin,
                                    movs,
                                    &row.mes_periodo,
                                    row.anio_periodo,
                                    &row.periodo_str,
                                ).is_err() {
                                    continue;
                                }
                            }

                            if let Some(dias_ced) = target_ced_opt {
                                row.dias_cedula = dias_ced;
                            }

                            let (max_permitido, _) = calculate_max_dias_obra_for_employee_movements(
                                row.dias_cedula,
                                row.lic,
                                row.inc,
                                row.aus,
                                &row.mes_periodo,
                                row.anio_periodo,
                                &row.periodo_str,
                                obra_ini,
                                obra_fin,
                                movs,
                            );

                            if let Some(dias_obr) = target_obr_opt {
                                let valid_obr = dias_obr.min(max_permitido);
                                if valid_obr < dias_obr {
                                    clamped_count += 1;
                                }
                                row.dias_obra = valid_obr;
                            } else if change_obra {
                                if let Some(ref o_opt) = obra_target {
                                    if o_opt.is_some() {
                                        row.dias_obra = max_permitido;
                                    } else {
                                        row.dias_obra = 0;
                                    }
                                }
                            } else if row.dias_obra > max_permitido {
                                row.dias_obra = max_permitido;
                                clamped_count += 1;
                            }

                            recalculate_row_in_memory(row);
                            count += 1;
                            modified_ids.insert(row.id_reporte_liq);
                        }
                    }

                    let rows = state
                        .all_rows
                        .iter()
                        .filter(|r| modified_ids.contains(&r.id_reporte_liq))
                        .map(|r| (r.id_reporte_liq, r.dias_cedula, r.dias_obra, r.id_registro_obra))
                        .collect();

                    apply_reporte_filtering(&mut state);
                    render_reporte_ui(&ui, &state);

                    if clamped_count > 0 {
                        modified_fields.push("Días Obra ajustados al límite");
                    }

                    rows
                };

                // Cerrar modal
                ui.global::<AppData>().set_show_reporte_batch_edit_modal(false);
                ui.global::<AppData>().set_reporte_batch_edit_dropdown_open(false);

                // Persistir en segundo plano
                if let Some(ref pool) = pool_clone {
                    let pool_bg = pool.clone();
                    std::thread::spawn(move || {
                        if let Ok(mut client) = pool_bg.get() {
                            if let Err(e) = batch_save_reporte_row_updates(&mut client, &rows_for_db) {
                                log::error!("Error persistiendo modificación masiva de modal en BD: {}", e);
                            }
                        }
                    });
                }

                let desc = modified_fields.join(", ");
                crate::show_toast_message(
                    &ui,
                    &format!("Modificación masiva aplicada a {} empleados ({}).", count, desc),
                );
            }
        });
    }

    // 18. Batch: Aplicar Registro de Obra, Días Cédula y/o Días Obra a la selección
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_batch_apply(move |obra_idx, dias_cedula_str, dias_obra_str, _apply_all| {
            if let Some(ui) = ui_weak.upgrade() {
                let custom_dias_ced_opt = dias_cedula_str.trim().parse::<i16>().ok();
                let custom_dias_obr_opt = dias_obra_str.trim().parse::<i16>().ok();
                let obra_idx_usize = obra_idx as usize;

                if obra_idx_usize == 0 && custom_dias_ced_opt.is_none() && custom_dias_obr_opt.is_none() {
                    crate::show_toast_message(
                        &ui,
                        "Por favor especifica al menos un campo a modificar (Obra, Días Cédula o Días Obra).",
                    );
                    return;
                }

                let mut count = 0;
                let mut modified_fields = Vec::new();

                let rows_for_db: Vec<(Uuid, i16, i16, Option<Uuid>)> = {
                    let mut state = session_clone.lock().unwrap();
                    let ids_to_modify: HashSet<Uuid> = if state.selected_ids.is_empty() {
                        state.filtered_rows.iter()
                            .map(|r| r.id_reporte_liq)
                            .collect()
                    } else {
                        state.selected_ids.clone()
                    };

                    let obra_info = if obra_idx_usize > 0 && obra_idx_usize <= state.obras_options.len() {
                        Some(state.obras_options[obra_idx_usize - 1].clone())
                    } else {
                        None
                    };

                    if obra_info.is_some() {
                        modified_fields.push("Obra");
                    }
                    if custom_dias_ced_opt.is_some() {
                        modified_fields.push("Días Cédula");
                    }
                    if custom_dias_obr_opt.is_some() {
                        modified_fields.push("Días Obra");
                    }

                    let obras_map: HashMap<Uuid, (Option<NaiveDate>, Option<NaiveDate>)> = state
                        .obras_options
                        .iter()
                        .map(|o| (o.id, (o.fecha_inicio, o.fecha_fin)))
                        .collect();

                    let movements_map = build_all_employee_movements_map(&state.all_rows);
                    let mut modified_ids = HashSet::new();
                    let mut clamped_count = 0;

                    for row in state.all_rows.iter_mut() {
                        if ids_to_modify.contains(&row.id_reporte_liq) {
                            let key = (row.id_empleado, row.mes_periodo.trim().to_uppercase(), row.anio_periodo);
                            let movs = movements_map.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);

                            if let Some(ref o) = obra_info {
                                if validate_obra_assignment_for_employee_movements(
                                    o.fecha_inicio,
                                    o.fecha_fin,
                                    movs,
                                    &row.mes_periodo,
                                    row.anio_periodo,
                                    &row.periodo_str,
                                ).is_err() {
                                    continue;
                                }
                                row.id_registro_obra = Some(o.id);
                                row.obra_numero = o.numero.clone();
                                row.obra_label = o.label.clone();
                                row.obra_creado_usuario = o.creado_usuario;
                            }

                            let (obra_ini, obra_fin) = row.id_registro_obra
                                .and_then(|oid| obras_map.get(&oid).copied())
                                .unwrap_or((None, None));

                            if row.id_registro_obra.is_some() {
                                if validate_obra_assignment_for_employee_movements(
                                    obra_ini,
                                    obra_fin,
                                    movs,
                                    &row.mes_periodo,
                                    row.anio_periodo,
                                    &row.periodo_str,
                                ).is_err() {
                                    continue;
                                }
                            }

                            if let Some(dias_ced) = custom_dias_ced_opt {
                                row.dias_cedula = dias_ced.max(1);
                            }

                            let (max_permitido, _) = calculate_max_dias_obra_for_employee_movements(
                                row.dias_cedula,
                                row.lic,
                                row.inc,
                                row.aus,
                                &row.mes_periodo,
                                row.anio_periodo,
                                &row.periodo_str,
                                obra_ini,
                                obra_fin,
                                movs,
                            );

                            if let Some(dias_obr) = custom_dias_obr_opt {
                                let valid_obr = dias_obr.max(0).min(max_permitido);
                                if valid_obr < dias_obr {
                                    clamped_count += 1;
                                }
                                row.dias_obra = valid_obr;
                            } else if obra_info.is_some() {
                                row.dias_obra = max_permitido;
                            } else if row.dias_obra > max_permitido {
                                row.dias_obra = max_permitido;
                                clamped_count += 1;
                            }
                            recalculate_row_in_memory(row);
                            count += 1;
                            modified_ids.insert(row.id_reporte_liq);
                        }
                    }

                    let rows = state
                        .all_rows
                        .iter()
                        .filter(|r| modified_ids.contains(&r.id_reporte_liq))
                        .map(|r| (r.id_reporte_liq, r.dias_cedula, r.dias_obra, r.id_registro_obra))
                        .collect();

                    apply_reporte_filtering(&mut state);
                    render_reporte_ui(&ui, &state);

                    if clamped_count > 0 {
                        modified_fields.push("Días Obra ajustados a Días Cédula");
                    }

                    rows
                };

                // Limpiar inputs del panel masivo tras aplicar
                ui.global::<AppData>().set_reporte_batch_dias_cedula_input("".into());
                ui.global::<AppData>().set_reporte_batch_dias_obra_input("".into());

                // Persistir exclusivamente en tablas de reportes (public.reportes_*)
                if let Some(ref pool) = pool_clone {
                    let pool_bg = pool.clone();
                    std::thread::spawn(move || {
                        if let Ok(mut client) = pool_bg.get() {
                            if let Err(e) = batch_save_reporte_row_updates(&mut client, &rows_for_db) {
                                log::error!("Error persistiendo modificación masiva en BD: {}", e);
                            }
                        }
                    });
                }

                let desc = modified_fields.join(", ");
                crate::show_toast_message(
                    &ui,
                    &format!("Modificación masiva aplicada a {} registros ({}).", count, desc),
                );
            }
        });
    }

    // 19. Exportar a Excel
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_exportar_reporte_liquidaciones_excel(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let state = session_clone.lock().unwrap();
                if state.filtered_rows.is_empty() {
                    crate::show_toast_message(&ui, "No hay filas en el reporte para exportar.");
                    return;
                }

                let output_dir = std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("output");
                let _ = std::fs::create_dir_all(&output_dir);

                let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
                let clean_rev = state.revision_nombre.replace(['/', '\\', ' '], "_");
                let filename = format!("Reporte_Liquidaciones_{}_{}.xlsx", clean_rev, timestamp);
                let out_file = output_dir.join(&filename);

                let valid_user_obra_ids: HashSet<Uuid> = state
                    .obras_options
                    .iter()
                    .filter(|o| o.creado_usuario)
                    .map(|o| o.id)
                    .collect();

                let filter_obra_id = if state.filter_obra_idx > 0 && state.filter_obra_idx <= state.obras_options.len() {
                    Some(state.obras_options[state.filter_obra_idx - 1].id)
                } else {
                    None
                };

                let guarderias_kpis = compute_guarderias_kpis(
                    &state.filtered_rows,
                    &state.obras_options,
                    filter_obra_id,
                );

                let mut delegacion = "COAHUILA".to_string();
                let mut subdelegacion = "SALTILLO".to_string();
                let mut rfc = "GON2110075C9".to_string();
                let representante_legal = "ARTURO CARRILLO DOMINGUEZ".to_string();

                if let Some(ref pool) = pool_clone {
                    if let Ok(mut client) = pool.get() {
                        if let Some(ref rev_id_str) = state.current_revision_id {
                            if let Ok(rev_uuid) = Uuid::parse_str(rev_id_str) {
                                if let Ok(Some(row)) = client.query_opt(
                                    r#"
                                    SELECT c.rfc, rp.delegacion, rp.subdelegacion
                                    FROM public.revisiones r
                                    JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
                                    JOIN public.clientes c ON c.id = rp.id_cliente
                                    WHERE r.id = $1
                                    "#,
                                    &[&rev_uuid],
                                ) {
                                    if let Some(r) = row.get::<_, Option<String>>(0) {
                                        if !r.trim().is_empty() { rfc = r.trim().to_string(); }
                                    }
                                    if let Some(d) = row.get::<_, Option<String>>(1) {
                                        if !d.trim().is_empty() { delegacion = d.trim().to_string(); }
                                    }
                                    if let Some(sd) = row.get::<_, Option<String>>(2) {
                                        if !sd.trim().is_empty() { subdelegacion = sd.trim().to_string(); }
                                    }
                                }
                            }
                        }
                    }
                }

                let periodo_obra = if state.filter_obra_idx > 0 && state.filter_obra_idx <= state.obras_options.len() {
                    let o = &state.obras_options[state.filter_obra_idx - 1];
                    match (o.fecha_inicio, o.fecha_fin) {
                        (Some(fi), Some(ff)) => format!("{} AL {}", fi.format("%d/%m/%Y"), ff.format("%d/%m/%Y")),
                        (Some(fi), None) => format!("DESDE {}", fi.format("%d/%m/%Y")),
                        (None, Some(ff)) => format!("HASTA {}", ff.format("%d/%m/%Y")),
                        (None, None) => "15/04/2026 AL 30/06/2026".to_string(),
                    }
                } else {
                    let mut min_fi: Option<chrono::NaiveDate> = None;
                    let mut max_ff: Option<chrono::NaiveDate> = None;
                    for o in &state.obras_options {
                        if let Some(fi) = o.fecha_inicio {
                            min_fi = Some(min_fi.map(|m| m.min(fi)).unwrap_or(fi));
                        }
                        if let Some(ff) = o.fecha_fin {
                            max_ff = Some(max_ff.map(|m| m.max(ff)).unwrap_or(ff));
                        }
                    }
                    match (min_fi, max_ff) {
                        (Some(fi), Some(ff)) => format!("{} AL {}", fi.format("%d/%m/%Y"), ff.format("%d/%m/%Y")),
                        (Some(fi), None) => format!("DESDE {}", fi.format("%d/%m/%Y")),
                        (None, Some(ff)) => format!("HASTA {}", ff.format("%d/%m/%Y")),
                        (None, None) => "15/04/2026 AL 30/06/2026".to_string(),
                    }
                };

                let header_meta = ReporteFinalHeaderMeta {
                    delegacion,
                    subdelegacion,
                    cliente_nombre: state.cliente_nombre.clone(),
                    rp_registro: state.rp_registro.clone(),
                    rfc,
                    periodo_obra,
                    representante_legal,
                    revision_folio: state.revision_folio.clone(),
                };

                match export_reporte_liquidaciones_to_excel(
                    &state.filtered_rows,
                    &state.all_rows,
                    Some(&valid_user_obra_ids),
                    &guarderias_kpis,
                    &out_file,
                    &state.cliente_nombre,
                    &state.rp_registro,
                    &state.revision_folio,
                    Some(&header_meta),
                ) {
                    Ok(_) => {
                        crate::show_toast_message(
                            &ui,
                            &format!("Excel generado exitosamente: {}", filename),
                        );
                        crate::open_folder_in_os(&output_dir);
                    }
                    Err(e) => {
                        log::error!("Error exportando excel de liquidaciones: {}", e);
                        crate::show_toast_message(&ui, &format!("Error exportando Excel: {}", e));
                    }
                }
            }
        });
    }

    // 20. Revertir cambios
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_revertir_reporte_liquidaciones(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let rev_id = ui.global::<AppData>().get_selected_revision_id().to_string();
                crate::app_logger::log_ui_event(
                    "Reporte Liquidaciones",
                    "Barra Superior",
                    "Click Botón Revertir Cambios",
                    &format!("revision_id={}", rev_id),
                );
                if !rev_id.is_empty() {
                    if let Ok(rev_uuid) = Uuid::parse_str(&rev_id) {
                        if let Some(pool) = &pool_clone {
                            match pool.get() {
                                Ok(mut client) => {
                                    match revertir_reporte_liquidaciones_a_originales(&mut client, rev_uuid) {
                                        Ok(count) => {
                                            log::info!("Revertidos {} registros de reporte liquidaciones a sus valores originales.", count);
                                            sync_and_load_reporte_data(&ui, &pool_clone, &session_clone, &rev_id);
                                            crate::show_toast_message(&ui, "Cambios revertidos exitosamente a los valores originales de las cédulas.");
                                        }
                                        Err(e) => {
                                            crate::app_logger::log_ui_error(
                                                "Reporte Liquidaciones",
                                                "Barra Superior",
                                                "Error al Revertir Cambios",
                                                &e,
                                            );
                                            log::error!("Error revirtiendo reporte: {}", e);
                                            crate::show_toast_message(&ui, &format!("Error al revertir cambios: {}", e));
                                        }
                                    }
                                }
                                Err(e) => {
                                    crate::app_logger::log_ui_error(
                                        "Reporte Liquidaciones",
                                        "Barra Superior",
                                        "Error Conexión BD Revertir",
                                        &e.to_string(),
                                    );
                                    log::error!("Error de conexión al pool para reversión: {}", e);
                                    crate::show_toast_message(&ui, "Error de conexión a la base de datos.");
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    // 21. Abrir modal de ordenamiento multi-criterio
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_open_sort_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_reporte_sort_modal(true);
            }
        });
    }

    // 23. Cerrar modal de ordenamiento multi-criterio
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_close_sort_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_reporte_sort_modal(false);
            }
        });
    }

    // 24. Toggle criterio de ordenamiento
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_sort_criterion(move |criterion| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                match criterion.as_str() {
                    "obra" => state.sort_by_obra = !state.sort_by_obra,
                    "anio" => state.sort_by_anio = !state.sort_by_anio,
                    "mes" => state.sort_by_mes = !state.sort_by_mes,
                    "rfc" => state.sort_by_rfc = !state.sort_by_rfc,
                    _ => {}
                }
                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 25. Toggle dirección de ordenamiento
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_sort_direction(move |criterion| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                match criterion.as_str() {
                    "obra" => state.sort_obra_asc = !state.sort_obra_asc,
                    "anio" => state.sort_anio_asc = !state.sort_anio_asc,
                    "mes" => state.sort_mes_asc = !state.sort_mes_asc,
                    "rfc" => state.sort_rfc_asc = !state.sort_rfc_asc,
                    _ => {}
                }
                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 26. Restablecer ordenamiento
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_reset_sorting(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                state.sort_by_obra = false;
                state.sort_obra_asc = true;
                state.sort_by_anio = false;
                state.sort_anio_asc = true;
                state.sort_by_mes = false;
                state.sort_mes_asc = true;
                state.sort_by_rfc = false;
                state.sort_rfc_asc = true;
                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);
                crate::show_toast_message(&ui, "Ordenamiento restablecido.");
            }
        });
    }

    // 27. Abrir modal de explorar y filtrar empleados seleccionados
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_open_empleados_filter_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();

                let (worker_ids, worker_rfcs, worker_names) = if !state.selected_ids.is_empty() {
                    let mut w_ids = HashSet::new();
                    let mut w_rfcs = HashSet::new();
                    let mut w_names = Vec::new();
                    let mut seen = HashSet::new();
                    for r in &state.all_rows {
                        if state.selected_ids.contains(&r.id_reporte_liq) {
                            w_ids.insert(r.id_empleado);
                            if let Some(clean) = get_clean_non_generic_rfc(&r.rfc) {
                                w_rfcs.insert(clean);
                            }
                            if seen.insert(r.nombre_trabajador.clone()) {
                                w_names.push(r.nombre_trabajador.clone());
                            }
                        }
                    }
                    (w_ids, w_rfcs, w_names)
                } else if state.emp_filter_active && !state.emp_filter_worker_ids.is_empty() {
                    let mut w_names = Vec::new();
                    let mut seen = HashSet::new();
                    for r in &state.all_rows {
                        let clean_rfc = get_clean_non_generic_rfc(&r.rfc);
                        if state.emp_filter_worker_ids.contains(&r.id_empleado)
                            || clean_rfc.map_or(false, |clean| state.emp_filter_worker_rfcs.contains(&clean))
                        {
                            if seen.insert(r.nombre_trabajador.clone()) {
                                w_names.push(r.nombre_trabajador.clone());
                            }
                        }
                    }
                    (state.emp_filter_worker_ids.clone(), state.emp_filter_worker_rfcs.clone(), w_names)
                } else {
                    crate::show_toast_message(&ui, "Selecciona al menos una casilla de empleado en la tabla.");
                    return;
                };

                let mut total_occurrences = 0;
                let mut month_counts: std::collections::BTreeMap<(i16, u32, String), usize> = std::collections::BTreeMap::new();
                let mut obra_counts_map: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

                for r in &state.all_rows {
                    let clean_rfc = get_clean_non_generic_rfc(&r.rfc);
                    let matches = worker_ids.contains(&r.id_empleado)
                        || clean_rfc.map_or(false, |clean| worker_rfcs.contains(&clean));
                    if matches {
                        total_occurrences += 1;

                        let m_num = month_to_num(&r.mes_periodo);
                        let entry = month_counts.entry((r.anio_periodo, m_num, r.periodo_str.clone())).or_insert(0);
                        *entry += 1;

                        let obra_key = if let Some(oid) = r.id_registro_obra {
                            oid.to_string()
                        } else {
                            "sin_asignar".to_string()
                        };
                        *obra_counts_map.entry(obra_key).or_insert(0) += 1;
                    }
                }

                state.emp_modal_worker_count = worker_names.len();
                state.emp_modal_worker_names_str = worker_names.join(", ");
                state.emp_modal_total_occurrences = total_occurrences;
                state.emp_filter_worker_ids = worker_ids;
                state.emp_filter_worker_rfcs = worker_rfcs;

                state.emp_modal_meses = month_counts.into_iter().map(|((anio, _m_num, periodo_str), count)| {
                    let label = if periodo_str.contains('/') {
                        periodo_str.clone()
                    } else {
                        format!("{} {}", periodo_str, anio)
                    };
                    let is_sel = if state.emp_filter_selected_months.is_empty() {
                        true
                    } else {
                        state.emp_filter_selected_months.contains(&periodo_str)
                    };
                    (periodo_str, label, count, is_sel)
                }).collect();

                let mut emp_modal_obras = Vec::new();
                for o in &state.all_obras_options {
                    let count = *obra_counts_map.get(&o.id.to_string()).unwrap_or(&0);
                    let is_sel = if state.emp_filter_selected_obras.is_empty() {
                        true
                    } else {
                        state.emp_filter_selected_obras.contains(&o.id.to_string())
                    };
                    emp_modal_obras.push((o.id.to_string(), o.label.clone(), count, is_sel));
                }
                let unassigned_count = *obra_counts_map.get("sin_asignar").unwrap_or(&0);
                let is_sel = if state.emp_filter_selected_obras.is_empty() {
                    true
                } else {
                    state.emp_filter_selected_obras.contains("sin_asignar")
                };
                emp_modal_obras.push(("sin_asignar".to_string(), "⚠️ Sin Asignar (Pendientes)".to_string(), unassigned_count, is_sel));

                for (k, cnt) in &obra_counts_map {
                    if k != "sin_asignar" && !state.all_obras_options.iter().any(|o| o.id.to_string() == *k) {
                        let is_sel = if state.emp_filter_selected_obras.is_empty() {
                            true
                        } else {
                            state.emp_filter_selected_obras.contains(k)
                        };
                        emp_modal_obras.push((k.clone(), format!("Obra {}", k), *cnt, is_sel));
                    }
                }

                state.emp_modal_obras = emp_modal_obras;

                ui.global::<AppData>().set_show_reporte_emp_modal(true);
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 28. Cerrar modal de empleados
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_close_empleados_filter_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_reporte_emp_modal(false);
            }
        });
    }

    // 29. Seleccionar todas las apariciones en todos los meses
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_select_all_months_for_selected_employees(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                let worker_ids = state.emp_filter_worker_ids.clone();
                let worker_rfcs = state.emp_filter_worker_rfcs.clone();

                let target_ids: Vec<Uuid> = state.all_rows.iter()
                    .filter(|r| {
                        if r.clave.trim().eq_ignore_ascii_case("baja") {
                            return false;
                        }
                        let clean_rfc = get_clean_non_generic_rfc(&r.rfc);
                        worker_ids.contains(&r.id_empleado)
                            || clean_rfc.map_or(false, |clean| worker_rfcs.contains(&clean))
                    })
                    .map(|r| r.id_reporte_liq)
                    .collect();

                let count = target_ids.len();
                state.selected_ids.extend(target_ids);

                ui.global::<AppData>().set_show_reporte_emp_modal(false);
                render_reporte_ui(&ui, &state);
                crate::show_toast_message(
                    &ui,
                    &format!("Se seleccionaron {} apariciones de los empleados en todos los meses.", count),
                );
            }
        });
    }

    // 30. Set filtro todos los meses (true = todos, false = algunos)
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_set_emp_filter_all_months(move |all_m| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                state.emp_filter_all_months = all_m;
                if all_m {
                    for m in state.emp_modal_meses.iter_mut() {
                        m.3 = true;
                    }
                }
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 31. Set filtro todas las obras (true = todas, false = algunas)
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_set_emp_filter_all_obras(move |all_o| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                state.emp_filter_all_obras = all_o;
                if all_o {
                    for o in state.emp_modal_obras.iter_mut() {
                        o.3 = true;
                    }
                }
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 32. Toggle mes en modal
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_emp_filter_month(move |m_id| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                let m_id_str = m_id.as_str();
                for m in state.emp_modal_meses.iter_mut() {
                    if m.0 == m_id_str {
                        m.3 = !m.3;
                        break;
                    }
                }
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 33. Toggle obra en modal
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_emp_filter_obra(move |o_id| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                let o_id_str = o_id.as_str();
                for o in state.emp_modal_obras.iter_mut() {
                    if o.0 == o_id_str {
                        o.3 = !o.3;
                        break;
                    }
                }
                render_reporte_ui(&ui, &state);
            }
        });
    }

    // 34. Aplicar filtro de empleados
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_apply_emp_filter(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                let sel_months: HashSet<String> = state.emp_modal_meses.iter()
                    .filter(|(_, _, _, sel)| *sel)
                    .map(|(id, _, _, _)| id.clone())
                    .collect();
                let sel_obras: HashSet<String> = state.emp_modal_obras.iter()
                    .filter(|(_, _, _, sel)| *sel)
                    .map(|(id, _, _, _)| id.clone())
                    .collect();

                state.emp_filter_selected_months = sel_months;
                state.emp_filter_selected_obras = sel_obras;

                state.emp_filter_active = true;
                state.current_page = 1;
                ui.global::<AppData>().set_show_reporte_emp_modal(false);

                apply_reporte_filtering(&mut state);

                // Seleccionar automáticamente las filas resultantes del filtro (excluyendo bajas)
                state.selected_ids = state.filtered_rows.iter()
                    .filter(|r| !r.clave.trim().eq_ignore_ascii_case("baja"))
                    .map(|r| r.id_reporte_liq)
                    .collect();

                render_reporte_ui(&ui, &state);

                crate::show_toast_message(
                    &ui,
                    &format!("Filtro de empleados activo: {} filas encontradas.", state.filtered_rows.len()),
                );
            }
        });
    }

    // 35. Limpiar filtro de empleados
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_clear_emp_filter(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                state.emp_filter_active = false;
                state.emp_filter_all_months = true;
                state.emp_filter_all_obras = true;
                state.emp_filter_selected_months.clear();
                state.emp_filter_selected_obras.clear();
                state.emp_filter_worker_ids.clear();
                state.emp_filter_worker_rfcs.clear();
                state.selected_ids.clear();
                ui.global::<AppData>().set_show_reporte_emp_modal(false);

                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);

                crate::show_toast_message(&ui, "Filtro de empleados removido y selección limpiada.");
            }
        });
    }

    // 35b. Limpiar todos los filtros (búsqueda, obra, periodo, estado, empleados) y deseleccionar
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_limpiar_todos_filtros(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                state.search_query.clear();
                state.filter_obra_idx = 0;
                state.filter_periodo_idx = 0;
                state.filter_estado_idx = 0;
                state.emp_filter_active = false;
                state.emp_filter_all_months = true;
                state.emp_filter_all_obras = true;
                state.emp_filter_selected_months.clear();
                state.emp_filter_selected_obras.clear();
                state.emp_filter_worker_ids.clear();
                state.emp_filter_worker_rfcs.clear();
                state.selected_ids.clear();
                state.selected_unique_emp_ids.clear();
                state.unique_emp_search_query.clear();
                for emp in state.unique_empleados.iter_mut() {
                    emp.selected = false;
                }

                let app_data = ui.global::<AppData>();
                app_data.set_reporte_search_query("".into());
                app_data.set_reporte_filter_obra_idx(0);
                app_data.set_reporte_filter_periodo_idx(0);
                app_data.set_reporte_filter_estado_idx(0);
                app_data.set_reporte_emp_filter_active(false);
                app_data.set_reporte_unique_empleados_search("".into());
                app_data.set_show_reporte_emp_modal(false);

                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);

                crate::show_toast_message(&ui, "Filtros removidos y selección limpiada.");
            }
        });
    }

    // 36. Abrir modal de selección rápida por obra
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_open_select_obras_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();

                // Calcular qué obras ya tienen filas seleccionadas
                let mut selected_obra_ids: HashSet<String> = HashSet::new();
                for id in &state.selected_ids {
                    if let Some(row) = state.all_rows.iter().find(|r| r.id_reporte_liq == *id) {
                        if let Some(oid) = row.id_registro_obra {
                            selected_obra_ids.insert(oid.to_string());
                        } else {
                            selected_obra_ids.insert("sin_asignar".to_string());
                        }
                    }
                }

                let mut obras_items: Vec<(String, String, usize, bool)> = Vec::new();

                for o in &state.all_obras_options {
                    let count = state.all_rows.iter()
                        .filter(|r| r.id_registro_obra == Some(o.id))
                        .map(|r| r.id_empleado)
                        .collect::<HashSet<Uuid>>()
                        .len();
                    let is_selected = selected_obra_ids.contains(&o.id.to_string());
                    obras_items.push((o.id.to_string(), o.label.clone(), count, is_selected));
                }

                let unassigned_count = state.all_rows.iter()
                    .filter(|r| r.id_registro_obra.is_none())
                    .map(|r| r.id_empleado)
                    .collect::<HashSet<Uuid>>()
                    .len();
                let is_selected = selected_obra_ids.contains("sin_asignar");
                obras_items.push(("sin_asignar".to_string(), "⚠️ Sin Asignar (Pendientes)".to_string(), unassigned_count, is_selected));

                state.quick_select_obras = obras_items;

                let models: Vec<ReporteFilterOptionItem> = state.quick_select_obras.iter().map(|(id, lbl, cnt, sel)| {
                    ReporteFilterOptionItem {
                        id: id.as_str().into(),
                        label: lbl.as_str().into(),
                        count: *cnt as i32,
                        selected: *sel,
                    }
                }).collect();

                let app_data = ui.global::<AppData>();
                app_data.set_reporte_quick_select_obras(ModelRc::new(VecModel::from(models)));
                app_data.set_show_reporte_select_obras_modal(true);
            }
        });
    }

    // 37. Cerrar modal de selección rápida por obra
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_close_select_obras_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_reporte_select_obras_modal(false);
            }
        });
    }

    // 38. Toggle opción en modal de selección rápida por obra
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_quick_select_obra(move |id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                for item in state.quick_select_obras.iter_mut() {
                    if item.0 == id_str.as_str() {
                        item.3 = !item.3;
                    }
                }

                let models: Vec<ReporteFilterOptionItem> = state.quick_select_obras.iter().map(|(id, lbl, cnt, sel)| {
                    ReporteFilterOptionItem {
                        id: id.as_str().into(),
                        label: lbl.as_str().into(),
                        count: *cnt as i32,
                        selected: *sel,
                    }
                }).collect();

                ui.global::<AppData>().set_reporte_quick_select_obras(ModelRc::new(VecModel::from(models)));
            }
        });
    }

    // 39. Marcar/Desmarcar todas en selección rápida por obra
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_quick_select_all_obras(move |select_all| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                for item in state.quick_select_obras.iter_mut() {
                    item.3 = select_all;
                }

                let models: Vec<ReporteFilterOptionItem> = state.quick_select_obras.iter().map(|(id, lbl, cnt, sel)| {
                    ReporteFilterOptionItem {
                        id: id.as_str().into(),
                        label: lbl.as_str().into(),
                        count: *cnt as i32,
                        selected: *sel,
                    }
                }).collect();

                ui.global::<AppData>().set_reporte_quick_select_obras(ModelRc::new(VecModel::from(models)));
            }
        });
    }

    // 40. Aplicar selección rápida por obra
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_apply_quick_select_obras(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                let sel_ids: HashSet<String> = state.quick_select_obras.iter()
                    .filter(|(_, _, _, sel)| *sel)
                    .map(|(id, _, _, _)| id.clone())
                    .collect();

                if sel_ids.is_empty() {
                    crate::show_toast_message(&ui, "Por favor marca al menos un registro de obra para seleccionar.");
                    return;
                }

                let mut newly_selected = HashSet::new();
                for row in &state.all_rows {
                    if row.clave.trim().eq_ignore_ascii_case("baja") {
                        continue;
                    }
                    let matches = match row.id_registro_obra {
                        Some(oid) => sel_ids.contains(&oid.to_string()),
                        None => sel_ids.contains("sin_asignar"),
                    };
                    if matches {
                        newly_selected.insert(row.id_reporte_liq);
                    }
                }

                state.selected_ids = newly_selected;
                let sel_count = state.selected_ids.len();

                ui.global::<AppData>().set_show_reporte_select_obras_modal(false);

                apply_reporte_filtering(&mut state);
                render_reporte_ui(&ui, &state);

                crate::show_toast_message(
                    &ui,
                    &format!("Se seleccionaron {} registros de empleados correspondientes a las obras elegidas.", sel_count),
                );
            }
        });
    }

    // 41. Desvincular Obra en Lote
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_batch_desvincular_obra(move || {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Reporte Liquidaciones",
                    "Barra Acciones Masivas",
                    "Click Botón Desvincular Obra en Lote",
                    "Desvinculando registro de obra de los empleados seleccionados",
                );
                let mut count = 0;
                let rows_for_db: Vec<(Uuid, i16, i16, Option<Uuid>)> = {
                    let mut state = session_clone.lock().unwrap();
                    let ids_to_modify: HashSet<Uuid> = if state.selected_ids.is_empty() {
                        state.filtered_rows.iter()
                            .filter(|r| !r.clave.trim().eq_ignore_ascii_case("baja"))
                            .map(|r| r.id_reporte_liq)
                            .collect()
                    } else {
                        state.selected_ids.clone()
                    };

                    if ids_to_modify.is_empty() {
                        crate::app_logger::log_ui_error(
                            "Reporte Liquidaciones",
                            "Barra Acciones Masivas",
                            "Validación Sin Registros para Desvincular",
                            "No hay registros seleccionados para desvincular",
                        );
                        crate::show_toast_message(&ui, "No hay registros seleccionados para desvincular.");
                        return;
                    }

                    for row in state.all_rows.iter_mut() {
                        if ids_to_modify.contains(&row.id_reporte_liq) {
                            if row.clave.trim().eq_ignore_ascii_case("baja") {
                                continue;
                            }
                            row.id_registro_obra = None;
                            row.obra_numero.clear();
                            row.obra_label = "⚠️ Sin Asignar (Pendiente)".to_string();
                            row.dias_obra = 0;
                            row.obra_creado_usuario = false;
                            recalculate_row_in_memory(row);
                            count += 1;
                        }
                    }

                    let rows = state
                        .all_rows
                        .iter()
                        .filter(|r| ids_to_modify.contains(&r.id_reporte_liq))
                        .map(|r| (r.id_reporte_liq, r.dias_cedula, r.dias_obra, r.id_registro_obra))
                        .collect();

                    apply_reporte_filtering(&mut state);
                    render_reporte_ui(&ui, &state);

                    rows
                };

                if count > 0 {
                    if let Some(ref pool) = pool_clone {
                        let pool_bg = pool.clone();
                        std::thread::spawn(move || {
                            if let Ok(mut client) = pool_bg.get() {
                                if let Err(e) = batch_save_reporte_row_updates(&mut client, &rows_for_db) {
                                    log::error!("Error persistiendo desvinculación masiva en BD: {}", e);
                                }
                            }
                        });
                    }

                    crate::show_toast_message(
                        &ui,
                        &format!("Se desvincularon {} registros de obra exitosamente.", count),
                    );
                }
            }
        });
    }

    // 29. Abrir modal Exportar SUA
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_abrir_modal_exportar_sua(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let mut state = session_clone.lock().unwrap();

                if state.all_rows.is_empty() {
                    crate::show_toast_message(&ui, "No hay liquidaciones cargadas para exportar.");
                    return;
                }

                // Selección por defecto: los registros de obra que el usuario creó para esa revisión
                state.sua_selected_obra_ids.clear();
                let user_created_ids: Vec<Uuid> = state.all_obras_options
                    .iter()
                    .filter(|o| o.creado_usuario)
                    .map(|o| o.id)
                    .collect();

                if !user_created_ids.is_empty() {
                    for id in user_created_ids {
                        state.sua_selected_obra_ids.insert(id);
                    }
                } else {
                    // Si ninguna fue creada por el usuario, seleccionar todas por defecto
                    let all_ids: Vec<Uuid> = state.all_obras_options.iter().map(|o| o.id).collect();
                    for id in all_ids {
                        state.sua_selected_obra_ids.insert(id);
                    }
                }

                refresh_sua_modal_ui(&app_data, &state);
                app_data.set_show_reporte_export_sua_modal(true);
            }
        });
    }

    // 30. Cerrar modal Exportar SUA
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_reporte_cerrar_modal_exportar_sua(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_reporte_export_sua_modal(false);
            }
        });
    }

    // 31. Toggle selección de obra para SUA
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_toggle_sua_obra(move |obra_id_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                if obra_id_str == "__SIN_OBRA__" {
                    state.sua_include_unassigned = !state.sua_include_unassigned;
                } else if let Ok(oid) = Uuid::parse_str(obra_id_str.as_str()) {
                    if state.sua_selected_obra_ids.contains(&oid) {
                        state.sua_selected_obra_ids.remove(&oid);
                    } else {
                        state.sua_selected_obra_ids.insert(oid);
                    }
                }
                let app_data = ui.global::<AppData>();
                refresh_sua_modal_ui(&app_data, &state);
            }
        });
    }

    // 32. Seleccionar todas o ninguna obra para SUA
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_sua_select_all_obras(move |select_all| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                state.sua_selected_obra_ids.clear();
                if select_all {
                    let all_ids: Vec<Uuid> = state.all_obras_options.iter().map(|o| o.id).collect();
                    for id in all_ids {
                        state.sua_selected_obra_ids.insert(id);
                    }
                    state.sua_include_unassigned = true;
                } else {
                    state.sua_include_unassigned = false;
                }
                let app_data = ui.global::<AppData>();
                refresh_sua_modal_ui(&app_data, &state);
            }
        });
    }

    // 32.1. Seleccionar preset para SUA: "all", "obras", "sin_obra", "none"
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_sua_select_preset(move |preset| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut state = session_clone.lock().unwrap();
                state.sua_selected_obra_ids.clear();
                match preset.as_str() {
                    "all" => {
                        let all_ids: Vec<Uuid> = state.all_obras_options.iter().map(|o| o.id).collect();
                        for id in all_ids {
                            state.sua_selected_obra_ids.insert(id);
                        }
                        state.sua_include_unassigned = true;
                    }
                    "obras" => {
                        let all_ids: Vec<Uuid> = state.all_obras_options.iter().map(|o| o.id).collect();
                        for id in all_ids {
                            state.sua_selected_obra_ids.insert(id);
                        }
                        state.sua_include_unassigned = false;
                    }
                    "sin_obra" => {
                        state.sua_include_unassigned = true;
                    }
                    _ => {
                        // "none" u otros
                        state.sua_include_unassigned = false;
                    }
                }
                let app_data = ui.global::<AppData>();
                refresh_sua_modal_ui(&app_data, &state);
            }
        });
    }

    // 33. Generar paquete de archivos SUA
    {
        let ui_weak = main_window.as_weak();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_generar_sua(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let (selected_obra_ids, include_unassigned, all_rows, folio_rev, all_obras) = {
                    let state = session_clone.lock().unwrap();
                    (
                        state.sua_selected_obra_ids.clone(),
                        state.sua_include_unassigned,
                        state.all_rows.clone(),
                        state.revision_folio.clone(),
                        state.all_obras_options.clone(),
                    )
                };

                if selected_obra_ids.is_empty() && !include_unassigned {
                    crate::show_toast_message(&ui, "Debe seleccionar al menos una opción (obra o sin asignar) para exportar.");
                    return;
                }

                // Filtrar los empleados en base a los registros de obra y/o sin asignar seleccionados
                let filtered_rows: Vec<ReporteLiquidacionRowDb> = all_rows
                    .into_iter()
                    .filter(|r| {
                        if let Some(oid) = r.id_registro_obra {
                            selected_obra_ids.contains(&oid)
                        } else {
                            include_unassigned
                        }
                    })
                    .collect();

                if filtered_rows.is_empty() {
                    crate::show_toast_message(&ui, "No se encontraron empleados para las opciones seleccionadas.");
                    return;
                }

                // Selección de carpeta de destino
                let folder_opt = rfd::FileDialog::new()
                    .set_title("Seleccionar Carpeta para Guardar Archivos SUA")
                    .pick_folder();

                let folder = match folder_opt {
                    Some(f) => f,
                    None => return,
                };

                match sua_export::export_sua_files(&folder, &folio_rev, &filtered_rows, &all_obras) {
                    Ok(res) => {
                        let msg = format!(
                            "Archivos SUA exportados exitosamente en:\n{}\n• 1. ASEG: {} trabajadores\n• 2. AFIL: {} afiliados\n• 3. MovObra: {} vinculaciones a obra\n• 4. MOVT: {} movimientos\n• 5. CRED: {}\n• 6. INCAP: {} incapacidades",
                            res.folder.display(),
                            res.aseg_count,
                            res.afil_count,
                            res.mov_obra_count,
                            res.movt_count,
                            res.cred_filename,
                            res.incap_count
                        );
                        crate::show_toast_message(&ui, &msg);
                        crate::open_folder_in_os(&folder);
                        ui.global::<AppData>().set_show_reporte_export_sua_modal(false);
                    }
                    Err(e) => {
                        log::error!("Error exportando archivos SUA: {}", e);
                        crate::show_toast_message(&ui, &format!("Error exportando SUA: {}", e));
                    }
                }
            }
        });
    }

    // 25. Callback para verificar y autocorregir días de obras
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let session_clone = reporte_session.clone();
        app_logic.on_reporte_verificar_y_corregir_dias(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let id_rev_str = {
                    let state = session_clone.lock().unwrap();
                    state.current_revision_id.clone()
                };
                let id_revision = match id_rev_str.and_then(|s| Uuid::parse_str(&s).ok()) {
                    Some(id) => id,
                    None => {
                        crate::show_toast_message(&ui, "No hay una revisión activa seleccionada.");
                        return;
                    }
                };

                let pool = match pool_clone {
                    Some(ref p) => p,
                    None => {
                        crate::show_toast_message(&ui, "No hay conexión a la base de datos.");
                        return;
                    }
                };

                let mut client = match pool.get() {
                    Ok(c) => c,
                    Err(e) => {
                        crate::show_toast_message(&ui, &format!("Error obteniendo cliente BD: {}", e));
                        return;
                    }
                };

                match crate::db::reporte_liquidaciones::verify_and_autocorrect_employee_obras(&mut client, id_revision) {
                    Ok(stats) => {
                        crate::show_toast_message(&ui, &stats.resumen);
                        if let Ok(fresh_rows) = crate::db::reporte_liquidaciones::fetch_reporte_liquidaciones(&mut client, id_revision) {
                            let mut state = session_clone.lock().unwrap();
                            state.all_rows = fresh_rows;
                            update_unique_empleados_catalog(&mut state, &ui.global::<AppData>());
                            apply_reporte_filtering(&mut state);
                            render_reporte_ui(&ui, &state);
                        }
                    }
                    Err(e) => {
                        crate::show_toast_message(&ui, &format!("Error verificando y corrigiendo días: {}", e));
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::build_rfc_curp_label;

    #[test]
    fn rfc_vacio_no_deja_linea_en_blanco_antes_de_curp() {
        // Caso JIMENEZ DE LA FUENTE MARIA ALEJANDRA: solo CURP
        assert_eq!(
            build_rfc_curp_label("", "jifa981012mclmnl01"),
            "jifa981012mclmnl01"
        );
    }

    #[test]
    fn rfc_nulo_tratado_como_vacio() {
        assert_eq!(
            build_rfc_curp_label("   ", "jifa981012mclmnl01"),
            "jifa981012mclmnl01"
        );
    }

    #[test]
    fn rfc_y_curp_se_unen_en_dos_lineas() {
        assert_eq!(
            build_rfc_curp_label("JIFA981012A12", "JIFA981012MCLMNL01"),
            "JIFA981012A12\nJIFA981012MCLMNL01"
        );
    }

    #[test]
    fn rfc_igual_a_curp_no_se_duplica() {
        assert_eq!(
            build_rfc_curp_label("JIFA981012MCLMNL01", "JIFA981012MCLMNL01"),
            "JIFA981012MCLMNL01"
        );
    }

    #[test]
    fn solo_rfc() {
        assert_eq!(build_rfc_curp_label("JIFA981012A12", ""), "JIFA981012A12");
    }

    #[test]
    fn test_dias_efectivos_formula_con_incidencias() {
        let dias_cedula = 30;
        let lic = 3.0;
        let inc = 2.0;
        let aus = 1.0;
        let dias_max = (dias_cedula as f64 - lic - inc - aus).round().max(0.0) as i16;
        assert_eq!(dias_max, 24);

        // Sin incidencias
        let dias_max_cero = (31 as f64 - 0.0 - 0.0 - 0.0).round().max(0.0) as i16;
        assert_eq!(dias_max_cero, 31);

        // Incidencias mayores que días cédula nunca es menor a 0
        let dias_neg = (10 as f64 - 8.0 - 5.0 - 0.0).round().max(0.0) as i16;
        assert_eq!(dias_neg, 0);
    }

    #[test]
    fn test_guarderias_18_porciento_calculo() {
        // Obra de 1,000,000. Límite 18% = 180,000
        let monto_obra = 1_000_000.0;
        let limite_18 = monto_obra * 0.18;
        assert_eq!(limite_18, 180_000.0);

        // Total GPS de empleados = 1,500.00
        // Valor Guarderías = 1,500.00 / 1% = 150,000.00
        let total_gps = 1_500.00;
        let valor_guarderias = total_gps / 0.01;
        assert_eq!(valor_guarderias, 150_000.00);
        let supera_18 = (monto_obra > 0.0 && valor_guarderias > limite_18) || (monto_obra == 0.0 && valor_guarderias > 0.0);
        assert!(!supera_18);

        // Total GPS de empleados = 2,000.00 -> 200,000.00 > 180,000.00
        let total_gps_alto = 2_000.00;
        let valor_guarderias_alto = total_gps_alto / 0.01;
        assert_eq!(valor_guarderias_alto, 200_000.00);
        let supera_18_alto = (monto_obra > 0.0 && valor_guarderias_alto > limite_18) || (monto_obra == 0.0 && valor_guarderias_alto > 0.0);
        assert!(supera_18_alto);
    }

    #[test]
    fn test_calculo_dias_obra_septiembre_ejemplo_usuario() {
        use chrono::NaiveDate;
        use crate::db::reporte_liquidaciones::calculate_max_dias_obra_for_employee;

        // Obra: C0971944
        // Fecha Inicio: 02/09/2024
        // Fecha Fin: 30/09/2024
        // Dias Cedula: 30
        let f_ini = NaiveDate::from_ymd_opt(2024, 9, 2);
        let f_fin = NaiveDate::from_ymd_opt(2024, 9, 30);
        let (max_dias, dias_obra_mes) = calculate_max_dias_obra_for_employee(
            30,
            0.0,
            0.0,
            0.0,
            "SEPTIEMBRE",
            2024,
            "09/2024",
            f_ini,
            f_fin,
            None,
            None,
        );
        // Debe ser 29 días por defecto y como máximo
        assert_eq!(dias_obra_mes, Some(29));
        assert_eq!(max_dias, 29);
    }

    #[test]
    fn test_calculo_dias_obra_fulanito1_y_fulanito2_marzo_ejemplo_usuario() {
        use chrono::NaiveDate;
        use crate::db::reporte_liquidaciones::calculate_max_dias_obra_for_employee;

        // Obra: C0896231
        // Fecha Inicio: 18/03/2024
        // Fecha Fin: 15/05/2024
        let f_ini = NaiveDate::from_ymd_opt(2024, 3, 18);
        let f_fin = NaiveDate::from_ymd_opt(2024, 5, 15);

        // Fulanito 1: Dias Cedula = 30
        let (max_f1, dias_obra_mes_f1) = calculate_max_dias_obra_for_employee(
            30,
            0.0,
            0.0,
            0.0,
            "MARZO",
            2024,
            "03/2024",
            f_ini,
            f_fin,
            None,
            None,
        );
        assert_eq!(dias_obra_mes_f1, Some(14));
        assert_eq!(max_f1, 14, "Fulanito 1 debe tener 14 días máximo en marzo");

        // Fulanito 2: Dias Cedula = 10
        let (max_f2, dias_obra_mes_f2) = calculate_max_dias_obra_for_employee(
            10,
            0.0,
            0.0,
            0.0,
            "MARZO",
            2024,
            "03/2024",
            f_ini,
            f_fin,
            None,
            None,
        );
        assert_eq!(dias_obra_mes_f2, Some(14));
        assert_eq!(max_f2, 10, "Fulanito 2 debe tener 10 días máximo en marzo");

        // Validación independiente: el máximo no aplica igual a ambos
        assert_ne!(max_f1, max_f2);
    }

    #[test]
    fn test_calculo_dias_obra_con_incidencias() {
        use chrono::NaiveDate;
        use crate::db::reporte_liquidaciones::calculate_max_dias_obra_for_employee;

        let f_ini = NaiveDate::from_ymd_opt(2024, 3, 18);
        let f_fin = NaiveDate::from_ymd_opt(2024, 5, 15);

        // Fulanito con 30 días cédula pero 5 días de incapacidad:
        // Días efectivos = 30 - 5 = 25.
        // Días obra en marzo = 14.
        // Max = min(25, 14) = 14.
        let (max_dias, _) = calculate_max_dias_obra_for_employee(
            30,
            0.0,
            5.0,
            0.0,
            "MARZO",
            2024,
            "03/2024",
            f_ini,
            f_fin,
            None,
            None,
        );
        assert_eq!(max_dias, 14);

        // Fulanito con 30 días cédula y 23 días de incidencias:
        // Días efectivos = 30 - 23 = 7.
        // Días obra en marzo = 14.
        // Max = min(7, 14) = 7.
        let (max_dias_bajo, _) = calculate_max_dias_obra_for_employee(
            30,
            10.0,
            10.0,
            3.0,
            "MARZO",
            2024,
            "03/2024",
            f_ini,
            f_fin,
            None,
            None,
        );
        assert_eq!(max_dias_bajo, 7);
    }

    #[test]
    fn test_calculo_dias_obra_con_clave_baja_ejemplo_usuario_view() {
        use chrono::NaiveDate;
        use crate::db::reporte_liquidaciones::{calculate_max_dias_obra_for_employee, validate_obra_assignment_for_employee};

        // Obra C1234567: Inicio 18/03/2026, Fin 30/05/2026
        // Empleado con Baja 12/03/2026, Dias Cedula 12
        let f_ini = NaiveDate::from_ymd_opt(2026, 3, 18);
        let f_fin = NaiveDate::from_ymd_opt(2026, 5, 30);
        let f_baja = NaiveDate::from_ymd_opt(2026, 3, 12);

        let (max_dias, _) = calculate_max_dias_obra_for_employee(
            12,
            0.0,
            0.0,
            0.0,
            "MARZO",
            2026,
            "03/2026",
            f_ini,
            f_fin,
            f_baja,
            None,
        );
        assert_eq!(max_dias, 0, "Obra iniciada después de baja debe dar tope 0");

        let val = validate_obra_assignment_for_employee(
            f_ini,
            f_fin,
            f_baja,
            None,
            "MARZO",
            2026,
            "03/2026",
        );
        assert!(val.is_err(), "No debe permitirse asignar una obra iniciada después de baja");
    }

    #[test]
    fn test_filtro_obras_solo_creadas_por_usuario_o_con_datos() {
        use uuid::Uuid;
        use crate::db::reporte_liquidaciones::{ObraOptionDb, ReporteLiquidacionRowDb};

        let id_c0896231 = Uuid::now_v7();
        let id_c0896232 = Uuid::now_v7();
        let id_c0909820 = Uuid::now_v7();
        let id_c0944829 = Uuid::now_v7();
        let id_c0971944 = Uuid::now_v7();

        let obras = vec![
            ObraOptionDb {
                id: id_c0896231,
                numero: "C0896231".into(),
                tipo_obra: "".into(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "C0896231".into(),
                creado_usuario: true, // Creado por usuario, 0 filas -> debe conservarse
                monto: 0.0,
            },
            ObraOptionDb {
                id: id_c0896232,
                numero: "C0896232".into(),
                tipo_obra: "".into(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "C0896232".into(),
                creado_usuario: false, // Creado por cédula, tiene filas -> debe conservarse
                monto: 0.0,
            },
            ObraOptionDb {
                id: id_c0909820,
                numero: "C0909820".into(),
                tipo_obra: "".into(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "C0909820".into(),
                creado_usuario: false, // Creado por cédula, 0 filas -> DEBE ELIMINARSE DE FILTROS
                monto: 0.0,
            },
            ObraOptionDb {
                id: id_c0944829,
                numero: "C0944829".into(),
                tipo_obra: "".into(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "C0944829".into(),
                creado_usuario: false, // Creado por cédula, tiene filas -> debe conservarse
                monto: 0.0,
            },
            ObraOptionDb {
                id: id_c0971944,
                numero: "C0971944".into(),
                tipo_obra: "".into(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "C0971944".into(),
                creado_usuario: true, // Creado por usuario, 0 filas -> debe conservarse
                monto: 0.0,
            },
        ];

        let make_dummy_row = |oid: Uuid, num: &str| -> ReporteLiquidacionRowDb {
            ReporteLiquidacionRowDb {
                id_reporte_liq: Uuid::now_v7(),
                id_parent_reporte_liq: Uuid::now_v7(),
                id_liquidacion: Uuid::now_v7(),
                id_empleado: Uuid::now_v7(),
                nombre_trabajador: "TRABAJADOR".into(),
                rfc: "XAXX010101000".into(),
                curp: "XAXX010101HDF00".into(),
                nss: "12345678901".into(),
                pensionado: String::new(),
                registro_patronal: "Y5410204108".into(),
                mes_periodo: "ENERO".into(),
                anio_periodo: 2024,
                periodo_str: "01/2024".into(),
                dias_cedula: 30,
                id_registro_obra: Some(oid),
                obra_numero: num.into(),
                obra_label: num.into(),
                dias_obra: 30,
                obra_creado_usuario: false,
                clave: "".into(),
                fecha: None,
                fecha_str: "-".into(),
                sdi: 350.0,
                lic: 0.0,
                inc: 0.0,
                aus: 0.0,
                cf: 100.0,
                exc_pat: 0.0,
                exc_obr: 0.0,
                pd_pat: 0.0,
                pd_obr: 0.0,
                gmp_pat: 0.0,
                gmp_obr: 0.0,
                rt: 0.0,
                iv_pat: 0.0,
                iv_obr: 0.0,
                gps: 10.0,
                patronal: 80.0,
                obrera: 20.0,
                subtotal: 100.0,
                cf_orig: 100.0,
                exc_pat_orig: 0.0,
                exc_obr_orig: 0.0,
                pd_pat_orig: 0.0,
                pd_obr_orig: 0.0,
                gmp_pat_orig: 0.0,
                gmp_obr_orig: 0.0,
                rt_orig: 0.0,
                iv_pat_orig: 0.0,
                iv_obr_orig: 0.0,
                gps_orig: 10.0,
                patronal_orig: 80.0,
                obrera_orig: 20.0,
                subtotal_orig: 100.0,
            }
        };

        // Filas asociadas solo a C0896232 y C0944829
        let rows = vec![
            make_dummy_row(id_c0896232, "C0896232"),
            make_dummy_row(id_c0944829, "C0944829"),
        ];

        let mut state = ReporteSessionState {
            all_rows: rows,
            obras_options: obras,
            ..Default::default()
        };

        apply_reporte_filtering(&mut state);

        // Se deben conservar 4 obras: C0896231 (user), C0896232 (en tabla), C0944829 (en tabla), C0971944 (user)
        assert_eq!(state.obras_options.len(), 4);
        assert!(state.obras_options.iter().any(|o| o.numero == "C0896231"));
        assert!(state.obras_options.iter().any(|o| o.numero == "C0896232"));
        assert!(state.obras_options.iter().any(|o| o.numero == "C0944829"));
        assert!(state.obras_options.iter().any(|o| o.numero == "C0971944"));

        // C0909820 no fue creada por usuario y no tiene filas, debe haber sido eliminada
        assert!(!state.obras_options.iter().any(|o| o.numero == "C0909820"));
    }

    #[test]
    fn test_modificacion_masiva_asignacion_obra_y_dias() {
        let obra_id_nueva = Uuid::now_v7();
        let obra_nueva = ObraOptionDb {
            id: obra_id_nueva,
            numero: "OBRA-MASIVA-01".to_string(),
            label: "OBRA-MASIVA-01 - PROYECTO ALPHA".to_string(),
            tipo_obra: "Construcción".to_string(),
            fecha_inicio: Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            fecha_fin: Some(NaiveDate::from_ymd_opt(2026, 1, 31).unwrap()),
            creado_usuario: true,
            monto: 0.0,
        };

        let row1_id = Uuid::now_v7();
        let row2_id = Uuid::now_v7();
        let row3_id = Uuid::now_v7();

        // 3 empleados:
        // Emp 1: Sin incidencias, mes 31 días (enero 2026)
        // Emp 2: Con 5 días de incapacidad (inc: 5.0), tope 26 días
        // Emp 3: Clave "baja", no debe modificarse
        let make_row = |id: Uuid, nombre: &str, clave: &str, inc: f64| -> ReporteLiquidacionRowDb {
            ReporteLiquidacionRowDb {
                id_reporte_liq: id,
                id_parent_reporte_liq: Uuid::now_v7(),
                id_liquidacion: Uuid::now_v7(),
                id_empleado: Uuid::now_v7(),
                periodo_str: "01/2026".to_string(),
                mes_periodo: "Enero".to_string(),
                anio_periodo: 2026,
                nss: "12345678901".to_string(),
                rfc: "TEST010101XX1".to_string(),
                curp: "TEST010101HDF00".to_string(),
                nombre_trabajador: nombre.to_string(),
                pensionado: String::new(),
                registro_patronal: "Y123456710".to_string(),
                id_registro_obra: None,
                obra_numero: String::new(),
                obra_label: "⚠️ Sin Asignar (Pendiente)".to_string(),
                obra_creado_usuario: false,
                dias_cedula: 31,
                dias_obra: 0,
                clave: clave.to_string(),
                fecha: None,
                fecha_str: "-".to_string(),
                sdi: 500.0,
                lic: 0.0,
                inc,
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
                subtotal: 0.0,
                cf_orig: 100.0,
                exc_pat_orig: 0.0,
                exc_obr_orig: 0.0,
                pd_pat_orig: 0.0,
                pd_obr_orig: 0.0,
                gmp_pat_orig: 0.0,
                gmp_obr_orig: 0.0,
                rt_orig: 0.0,
                iv_pat_orig: 0.0,
                iv_obr_orig: 0.0,
                gps_orig: 10.0,
                patronal_orig: 80.0,
                obrera_orig: 20.0,
                subtotal_orig: 100.0,
            }
        };

        let mut rows = vec![
            make_row(row1_id, "TRABAJADOR UNO", "01", 0.0),
            make_row(row2_id, "TRABAJADOR DOS", "01", 5.0),
            make_row(row3_id, "TRABAJADOR BAJA", "BAJA", 0.0),
        ];

        let selected: HashSet<Uuid> = [row1_id, row2_id, row3_id].into_iter().collect();

        // Simular aplicación masiva:
        // Obra: obra_nueva
        // Días Cédula: 30
        // Días Obra: 30 (Emp 1 aceptará 30, Emp 2 se topará a 25 porque 30 - 5 inc = 25)
        let obras_map: HashMap<Uuid, (Option<NaiveDate>, Option<NaiveDate>)> = [(
            obra_id_nueva,
            (obra_nueva.fecha_inicio, obra_nueva.fecha_fin),
        )]
        .into_iter()
        .collect();

        let movs_map = build_all_employee_movements_map(&rows);

        for row in rows.iter_mut() {
            if selected.contains(&row.id_reporte_liq) {
                if row.clave.trim().eq_ignore_ascii_case("baja") {
                    continue;
                }

                // Asignar obra
                row.id_registro_obra = Some(obra_nueva.id);
                row.obra_numero = obra_nueva.numero.clone();
                row.obra_label = obra_nueva.label.clone();
                row.obra_creado_usuario = obra_nueva.creado_usuario;

                // Asignar días cédula
                row.dias_cedula = 30;

                // Calcular max permitido
                let (obra_ini, obra_fin) = row
                    .id_registro_obra
                    .and_then(|oid| obras_map.get(&oid).copied())
                    .unwrap_or((None, None));

                let movs = movs_map
                    .get(&(row.id_empleado, row.mes_periodo.trim().to_uppercase(), row.anio_periodo))
                    .cloned()
                    .unwrap_or_default();

                let (max_permitido, _) = calculate_max_dias_obra_for_employee_movements(
                    row.dias_cedula,
                    row.lic,
                    row.inc,
                    row.aus,
                    &row.mes_periodo,
                    row.anio_periodo,
                    &row.periodo_str,
                    obra_ini,
                    obra_fin,
                    &movs,
                );

                // Asignar 30 días obra con tope
                row.dias_obra = 30.min(max_permitido);
                recalculate_row_in_memory(row);
            }
        }

        // Validaciones:
        let r1 = rows.iter().find(|r| r.id_reporte_liq == row1_id).unwrap();
        assert_eq!(r1.id_registro_obra, Some(obra_id_nueva));
        assert_eq!(r1.obra_numero, "OBRA-MASIVA-01");
        assert_eq!(r1.dias_cedula, 30);
        assert_eq!(r1.dias_obra, 30);
        assert!(r1.subtotal > 0.0);

        let r2 = rows.iter().find(|r| r.id_reporte_liq == row2_id).unwrap();
        assert_eq!(r2.id_registro_obra, Some(obra_id_nueva));
        assert_eq!(r2.dias_cedula, 30);
        // Emp 2 tenía 5 de incapacidad: 30 - 5 = 25 máx
        assert_eq!(r2.dias_obra, 25);
        assert!(r2.subtotal > 0.0);

        // Emp 3 tenía clave baja: no fue modificado
        let r3 = rows.iter().find(|r| r.id_reporte_liq == row3_id).unwrap();
        assert_eq!(r3.id_registro_obra, None);
        assert_eq!(r3.dias_obra, 0);
    }
}

