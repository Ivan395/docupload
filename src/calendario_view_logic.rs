use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use chrono::{Datelike, NaiveDate};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::db::calendario::*;
use crate::db::revisiones::parse_date_opt;
use crate::db::{fetch_imss_inhabiles, DbPool};
use crate::{
    AppData, AppLogic, CalendarDayCell, CalendarDayMilestone, CalendarHolidayItem,
    CalendarRevisionItem, MainWindow,
};
use crate::show_toast_message;

pub fn parse_hex_color(hex: &str) -> (u8, u8, u8) {
    let s = hex.trim().trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
        (r, g, b)
    } else {
        (71, 85, 105)
    }
}

pub fn update_holiday_catalog_list(
    ui: &MainWindow,
    holidays: &[CalendarHoliday],
) {
    let app_data = ui.global::<AppData>();
    let current_actual_year = chrono::Local::now().date_naive().year();
    let cat_year = {
        let y = app_data.get_cal_holiday_catalog_year();
        if y > 1900 { y } else { current_actual_year }
    };
    app_data.set_cal_holiday_catalog_year(cat_year);
    let search = app_data.get_cal_holiday_search_text().to_lowercase();

    let filtered: Vec<CalendarHolidayItem> = holidays
        .iter()
        .filter(|h| {
            if h.fecha.year() != cat_year {
                return false;
            }
            if search.is_empty() {
                true
            } else {
                h.motivo.to_lowercase().contains(&search)
                    || h.fecha_str.contains(&search)
                    || h.dia.to_lowercase().contains(&search)
            }
        })
        .map(|h| CalendarHolidayItem {
            id: h.id.clone().into(),
            fecha_str: h.fecha_str.clone().into(),
            fecha_iso: h.fecha_iso.clone().into(),
            dia_semana: h.dia.clone().into(),
            motivo: h.motivo.clone().into(),
            es_sistema: h.es_sistema,
        })
        .collect();

    app_data.set_cal_holiday_catalog_count(filtered.len() as i32);
    app_data.set_cal_holidays_list(ModelRc::new(VecModel::from(filtered)));
}

pub fn load_calendar_ui(
    ui: &MainWindow,
    pool: &DbPool,
    target_year: Option<i32>,
    target_month: Option<u32>,
    selected_rev_id: Option<&str>,
) {
    let mut client = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error obteniendo conexión a base de datos para calendario: {}", e);
            return;
        }
    };

    let holidays = match fetch_all_holidays(&mut client) {
        Ok(h) => h,
        Err(e) => {
            log::error!("Error consultando días festivos para calendario: {}", e);
            Vec::new()
        }
    };

    let holidays_set: HashSet<NaiveDate> = holidays.iter().map(|h| h.fecha).collect();

    let revisions = match fetch_all_revisions_for_calendar(&mut client, &holidays_set) {
        Ok(r) => r,
        Err(e) => {
            log::error!("Error consultando revisiones para calendario: {}", e);
            Vec::new()
        }
    };

    let app_data = ui.global::<AppData>();

    let year = target_year.unwrap_or_else(|| {
        let y = app_data.get_cal_view_year();
        if y > 2000 { y } else { 2026 }
    });

    let month = target_month.unwrap_or_else(|| {
        let m = app_data.get_cal_view_month();
        if (1..=12).contains(&m) { m as u32 } else { 9 }
    });

    let sel_id_storage = app_data.get_cal_selected_revision_id().to_string();
    let sel_rev_id_str = selected_rev_id.map(|s| s.to_string()).unwrap_or(sel_id_storage);
    let sel_rev_opt = if sel_rev_id_str.is_empty() { None } else { Some(sel_rev_id_str.as_str()) };

    let today = chrono::Local::now().date_naive();
    let cells_data = generate_month_cells(year, month, today, &holidays, &revisions, sel_rev_opt);

    // Month name
    let month_name = format!("{} {}", spanish_month_name(month), year);
    app_data.set_cal_view_year(year);
    app_data.set_cal_view_month(month as i32);
    app_data.set_cal_month_name(month_name.into());

    // 1. Day Cells Model
    let slint_cells: Vec<CalendarDayCell> = cells_data
        .into_iter()
        .map(|c| {
            let (bg_r, bg_g, bg_b) = parse_hex_color(&c.color_cell_bg);
            let (bd_r, bd_g, bd_b) = parse_hex_color(&c.color_cell_border);
            let (b1_r, b1_g, b1_b) = parse_hex_color(&c.badge1_color_fg);
            let (b1_br, b1_bg, b1_bb) = parse_hex_color(&c.badge1_color_bg);
            let (b2_r, b2_g, b2_b) = parse_hex_color(&c.badge2_color_fg);
            let (b2_br, b2_bg, b2_bb) = parse_hex_color(&c.badge2_color_bg);
            let (b3_r, b3_g, b3_b) = parse_hex_color(&c.badge3_color_fg);
            let (b3_br, b3_bg, b3_bb) = parse_hex_color(&c.badge3_color_bg);
            let (h_r, h_g, h_b) = parse_hex_color(&c.highlight_color);

            CalendarDayCell {
                day_number: c.day_number,
                month: c.month,
                year: c.year,
                date_str: c.date_str.into(),
                date_iso: c.date_iso.into(),
                is_current_month: c.is_current_month,
                is_today: c.is_today,
                is_weekend: c.is_weekend,
                is_holiday: c.is_holiday,
                holiday_motivo: c.holiday_motivo.into(),
                holiday_is_system: c.holiday_is_system,
                holiday_id: c.holiday_id.into(),
                has_milestones: c.has_milestones,
                milestone_count: c.milestone_count,
                summary_label: c.summary_label.into(),
                color_cell_bg: slint::Color::from_argb_u8(255, bg_r, bg_g, bg_b),
                color_cell_border: slint::Color::from_argb_u8(255, bd_r, bd_g, bd_b),
                has_events: c.has_events,
                badge1_label: c.badge1_label.into(),
                badge1_color_fg: slint::Color::from_argb_u8(255, b1_r, b1_g, b1_b),
                badge1_color_bg: slint::Color::from_argb_u8(255, b1_br, b1_bg, b1_bb),
                badge2_label: c.badge2_label.into(),
                badge2_color_fg: slint::Color::from_argb_u8(255, b2_r, b2_g, b2_b),
                badge2_color_bg: slint::Color::from_argb_u8(255, b2_br, b2_bg, b2_bb),
                badge3_label: c.badge3_label.into(),
                badge3_color_fg: slint::Color::from_argb_u8(255, b3_r, b3_g, b3_b),
                badge3_color_bg: slint::Color::from_argb_u8(255, b3_br, b3_bg, b3_bb),
                more_count: c.more_count,
                is_highlighted: c.is_highlighted,
                highlight_color: slint::Color::from_argb_u8(255, h_r, h_g, h_b),
                highlight_label: c.highlight_label.into(),
            }
        })
        .collect();
    app_data.set_cal_day_cells(ModelRc::new(VecModel::from(slint_cells)));

    // 2. Holidays List Model (Filtered by Catalog Year)
    update_holiday_catalog_list(ui, &holidays);

    // 3. Revisions List Model
    let search = app_data.get_cal_search_text().to_lowercase();
    let slint_revs: Vec<CalendarRevisionItem> = revisions
        .iter()
        .filter(|r| {
            if search.is_empty() {
                true
            } else {
                r.nombre.to_lowercase().contains(&search)
                    || r.folio.to_lowercase().contains(&search)
                    || r.cliente_nombre.to_lowercase().contains(&search)
                    || r.rp_numero.to_lowercase().contains(&search)
            }
        })
        .map(|r| CalendarRevisionItem {
            id: r.id.clone().into(),
            nombre: r.nombre.clone().into(),
            folio: r.folio.clone().into(),
            cliente_nombre: r.cliente_nombre.clone().into(),
            rp_numero: r.rp_numero.clone().into(),
            estatus: r.estatus.clone().into(),
            estatus_imss: r.estatus_imss.clone().into(),
            f_entrega_imss: r.fecha_entrega_imss.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
            f_max_aprobacion_imss: r.fecha_maxima_aprobacion_imss.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
            f_aprobacion_imss: r.fecha_aprobacion_imss.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
            f_max_entrega_revision: r.fecha_maxima_entrega_revision.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
            f_entrega_revision: r.fecha_entrega_revision.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
            f_max_entrega_imss: r.fecha_maxima_entrega_imss.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
            f_aprobacion_prorroga: r.fecha_aprobacion_prorroga.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
            f_entrega_con_prorroga: r.fecha_entrega_con_prorroga.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
            pidio_prorroga: r.pidio_prorroga,
            has_entrega_imss: r.fecha_entrega_imss.is_some(),
            has_max_aprobacion_imss: r.fecha_maxima_aprobacion_imss.is_some(),
            has_aprobacion_imss: r.fecha_aprobacion_imss.is_some(),
            has_max_entrega_rev: r.fecha_maxima_entrega_revision.is_some(),
            has_entrega_rev: r.fecha_entrega_revision.is_some(),
            has_max_entrega_imss: r.fecha_maxima_entrega_imss.is_some(),
            has_aprobacion_prorroga: r.fecha_aprobacion_prorroga.is_some(),
            has_entrega_con_prorroga: r.fecha_entrega_con_prorroga.is_some(),
        })
        .collect();
    app_data.set_cal_revisions_list(ModelRc::new(VecModel::from(slint_revs)));

    // 4. Update Selected Revision details if any
    if let Some(sel_id) = sel_rev_opt {
        if let Some(r) = revisions.iter().find(|x| x.id == sel_id) {
            let item = CalendarRevisionItem {
                id: r.id.clone().into(),
                nombre: r.nombre.clone().into(),
                folio: r.folio.clone().into(),
                cliente_nombre: r.cliente_nombre.clone().into(),
                rp_numero: r.rp_numero.clone().into(),
                estatus: r.estatus.clone().into(),
                estatus_imss: r.estatus_imss.clone().into(),
                f_entrega_imss: r.fecha_entrega_imss.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                f_max_aprobacion_imss: r.fecha_maxima_aprobacion_imss.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                f_aprobacion_imss: r.fecha_aprobacion_imss.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                f_max_entrega_revision: r.fecha_maxima_entrega_revision.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                f_entrega_revision: r.fecha_entrega_revision.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                f_max_entrega_imss: r.fecha_maxima_entrega_imss.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                f_aprobacion_prorroga: r.fecha_aprobacion_prorroga.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                f_entrega_con_prorroga: r.fecha_entrega_con_prorroga.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                pidio_prorroga: r.pidio_prorroga,
                has_entrega_imss: r.fecha_entrega_imss.is_some(),
                has_max_aprobacion_imss: r.fecha_maxima_aprobacion_imss.is_some(),
                has_aprobacion_imss: r.fecha_aprobacion_imss.is_some(),
                has_max_entrega_rev: r.fecha_maxima_entrega_revision.is_some(),
                has_entrega_rev: r.fecha_entrega_revision.is_some(),
                has_max_entrega_imss: r.fecha_maxima_entrega_imss.is_some(),
                has_aprobacion_prorroga: r.fecha_aprobacion_prorroga.is_some(),
                has_entrega_con_prorroga: r.fecha_entrega_con_prorroga.is_some(),
            };
            app_data.set_cal_selected_revision_id(sel_id.into());
            app_data.set_cal_selected_revision(item);
            app_data.set_cal_has_selected_revision(true);
        } else {
            app_data.set_cal_selected_revision_id("".into());
            app_data.set_cal_has_selected_revision(false);
        }
    } else {
        app_data.set_cal_selected_revision_id("".into());
        app_data.set_cal_has_selected_revision(false);
    }

    // 5. KPIs
    app_data.set_cal_total_holidays_count(holidays.len() as i32);
    app_data.set_cal_total_revisions_count(revisions.len() as i32);

    let mut month_milestones = 0;
    for r in &revisions {
        let dates = [
            r.fecha_entrega_imss,
            r.fecha_maxima_aprobacion_imss,
            r.fecha_aprobacion_imss,
            r.fecha_maxima_entrega_revision,
            r.fecha_entrega_revision,
            r.fecha_maxima_entrega_imss,
            r.fecha_aprobacion_prorroga,
            r.fecha_entrega_con_prorroga,
        ];
        for d_opt in dates {
            if let Some(d) = d_opt {
                if d.year() == year && d.month() == month {
                    month_milestones += 1;
                }
            }
        }
    }
    app_data.set_cal_month_milestones_count(month_milestones);
}

pub fn register_calendar_callbacks(
    main_window: &MainWindow,
    pool_opt: Option<Arc<DbPool>>,
    imss_inhabiles: Arc<RwLock<HashSet<NaiveDate>>>,
) {
    let app_logic = main_window.global::<AppLogic>();

    // Mes Anterior
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_prev_month(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let app_data = ui.global::<AppData>();
                    let cur_y = app_data.get_cal_view_year();
                    let cur_m = app_data.get_cal_view_month();
                    let (new_y, new_m) = if cur_m <= 1 {
                        (cur_y - 1, 12)
                    } else {
                        (cur_y, (cur_m - 1) as u32)
                    };
                    load_calendar_ui(&ui, pool, Some(new_y), Some(new_m), None);
                }
            }
        });
    }

    // Mes Siguiente
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_next_month(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let app_data = ui.global::<AppData>();
                    let cur_y = app_data.get_cal_view_year();
                    let cur_m = app_data.get_cal_view_month();
                    let (new_y, new_m) = if cur_m >= 12 {
                        (cur_y + 1, 1)
                    } else {
                        (cur_y, (cur_m + 1) as u32)
                    };
                    load_calendar_ui(&ui, pool, Some(new_y), Some(new_m), None);
                }
            }
        });
    }

    // Ir a Hoy
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_today(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let today = chrono::Local::now().date_naive();
                    load_calendar_ui(&ui, pool, Some(today.year()), Some(today.month()), None);
                }
            }
        });
    }

    // Cambiar Año
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_set_year(move |y| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let m = ui.global::<AppData>().get_cal_view_month() as u32;
                    load_calendar_ui(&ui, pool, Some(y), Some(m), None);
                }
            }
        });
    }

    // Seleccionar Revisión
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_select_revision(move |rev_id| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let rev_id_str = rev_id.to_string();
                    load_calendar_ui(&ui, pool, None, None, Some(&rev_id_str));
                }
            }
        });
    }

    // Limpiar Selección de Revisión
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_clear_selected_revision(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    ui.global::<AppData>().set_cal_selected_revision_id("".into());
                    ui.global::<AppData>().set_cal_has_selected_revision(false);
                    load_calendar_ui(&ui, pool, None, None, Some(""));
                }
            }
        });
    }

    // Buscar Revisiones
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_search_revisions(move |text| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    ui.global::<AppData>().set_cal_search_text(text);
                    load_calendar_ui(&ui, pool, None, None, None);
                }
            }
        });
    }

    // Cambiar Tab
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_switch_tab(move |tab| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_cal_active_tab(tab);
                if tab == 1 {
                    // Cada vez que salgas y vuelvas a entrar a "Catalogos dias festivos imss" debe de mostrar por defecto el catalogo del anio actual
                    let current_year = chrono::Local::now().date_naive().year();
                    app_data.set_cal_holiday_catalog_year(current_year);
                    app_data.set_cal_holiday_search_text("".into());
                    if let Some(ref pool) = pool_clone {
                        if let Ok(mut client) = pool.get() {
                            let holidays = fetch_all_holidays(&mut client).unwrap_or_default();
                            update_holiday_catalog_list(&ui, &holidays);
                        }
                    }
                }
            }
        });
    }

    // Catálogo Festivos: Año Anterior
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_holiday_catalog_prev_year(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let cur_y = ui.global::<AppData>().get_cal_holiday_catalog_year();
                    ui.global::<AppData>().set_cal_holiday_catalog_year(cur_y - 1);
                    if let Ok(mut client) = pool.get() {
                        let holidays = fetch_all_holidays(&mut client).unwrap_or_default();
                        update_holiday_catalog_list(&ui, &holidays);
                    }
                }
            }
        });
    }

    // Catálogo Festivos: Año Siguiente
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_holiday_catalog_next_year(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let cur_y = ui.global::<AppData>().get_cal_holiday_catalog_year();
                    ui.global::<AppData>().set_cal_holiday_catalog_year(cur_y + 1);
                    if let Ok(mut client) = pool.get() {
                        let holidays = fetch_all_holidays(&mut client).unwrap_or_default();
                        update_holiday_catalog_list(&ui, &holidays);
                    }
                }
            }
        });
    }

    // Catálogo Festivos: Buscador
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_holiday_search(move |text| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    ui.global::<AppData>().set_cal_holiday_search_text(text);
                    if let Ok(mut client) = pool.get() {
                        let holidays = fetch_all_holidays(&mut client).unwrap_or_default();
                        update_holiday_catalog_list(&ui, &holidays);
                    }
                }
            }
        });
    }

    // Abrir Modal Agregar Festivo
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_cal_open_add_holiday_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_cal_new_holiday_date("".into());
                app_data.set_cal_new_holiday_motivo("".into());
                app_data.set_cal_holiday_error_msg("".into());
                app_data.set_cal_show_add_holiday_modal(true);
            }
        });
    }

    // Cerrar Modal Agregar Festivo
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_cal_close_add_holiday_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_cal_show_add_holiday_modal(false);
            }
        });
    }

    // Enviar / Guardar Nuevo Festivo con Recálculo Inteligente de Revisiones Afectadas
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        let holidays_rw = imss_inhabiles.clone();
        app_logic.on_cal_submit_add_holiday(move |date_str, motivo| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let pool = match pool_clone {
                    Some(ref p) => p,
                    None => {
                        app_data.set_cal_holiday_error_msg("No hay conexión con la base de datos.".into());
                        return;
                    }
                };

                let fecha = match parse_date_opt(date_str.as_str()) {
                    Some(d) => d,
                    None => {
                        app_data.set_cal_holiday_error_msg("Fecha inválida. Utilice el formato DD/MM/YYYY.".into());
                        return;
                    }
                };

                let motivo_str = motivo.trim().to_string();
                if motivo_str.is_empty() {
                    app_data.set_cal_holiday_error_msg("El motivo del día festivo es obligatorio.".into());
                    return;
                }

                let mut client = match pool.get() {
                    Ok(c) => c,
                    Err(e) => {
                        app_data.set_cal_holiday_error_msg(format!("Error de conexión: {}", e).into());
                        return;
                    }
                };

                match insert_user_holiday(&mut client, fecha, &motivo_str) {
                    Ok(_) => {
                        let updated_set = match fetch_imss_inhabiles(&mut client) {
                            Ok(s) => s,
                            Err(e) => {
                                log::error!("Error consultando imss_inhabiles tras inserción: {}", e);
                                HashSet::new()
                            }
                        };

                        if let Ok(mut lock) = holidays_rw.write() {
                            *lock = updated_set.clone();
                        }

                        // Recalcular exactamente las revisiones cuyas fechas son afectadas por este nuevo día festivo
                        let affected_count = match recalculate_affected_revisions(&mut client, &updated_set) {
                            Ok(cnt) => cnt,
                            Err(e) => {
                                log::error!("Error recalculando revisiones afectadas: {}", e);
                                0
                            }
                        };

                        app_data.set_cal_show_add_holiday_modal(false);

                        if affected_count > 0 {
                            show_toast_message(
                                &ui,
                                &format!(
                                    "✓ Festivo guardado. Se recalcularon las fechas de {} revisión(es) afectadas.",
                                    affected_count
                                ),
                            );
                        } else {
                            show_toast_message(
                                &ui,
                                "✓ Día festivo registrado exitosamente en catálogo IMSS.",
                            );
                        }

                        load_calendar_ui(&ui, pool, None, None, None);
                    }
                    Err(e) => {
                        app_data.set_cal_holiday_error_msg(e.into());
                    }
                }
            }
        });
    }

    // Solicitar confirmación de eliminación de festivo
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_cal_prompt_delete_holiday(move |id, motivo, fecha| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_cal_delete_holiday_id(id);
                app_data.set_cal_delete_holiday_motivo(motivo);
                app_data.set_cal_delete_holiday_fecha(fecha);
                app_data.set_cal_show_delete_holiday_modal(true);
            }
        });
    }

    // Cancelar eliminación de festivo
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_cal_cancel_delete_holiday(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_cal_show_delete_holiday_modal(false);
            }
        });
    }

    // Confirmar eliminación de festivo con Recálculo de fechas
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        let holidays_rw = imss_inhabiles.clone();
        app_logic.on_cal_confirm_delete_holiday(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let pool = match pool_clone {
                    Some(ref p) => p,
                    None => return,
                };
                let id = app_data.get_cal_delete_holiday_id().to_string();

                let mut client = match pool.get() {
                    Ok(c) => c,
                    Err(e) => {
                        show_toast_message(&ui, &format!("Error de base de datos: {}", e));
                        return;
                    }
                };

                match delete_user_holiday(&mut client, &id) {
                    Ok(_) => {
                        let updated_set = match fetch_imss_inhabiles(&mut client) {
                            Ok(s) => s,
                            Err(_) => HashSet::new(),
                        };

                        if let Ok(mut lock) = holidays_rw.write() {
                            *lock = updated_set.clone();
                        }

                        let affected_count = match recalculate_affected_revisions(&mut client, &updated_set) {
                            Ok(cnt) => cnt,
                            Err(_) => 0,
                        };

                        app_data.set_cal_show_delete_holiday_modal(false);

                        if affected_count > 0 {
                            show_toast_message(
                                &ui,
                                &format!(
                                    "✓ Festivo eliminado. Se actualizaron {} revisión(es) afectadas.",
                                    affected_count
                                ),
                            );
                        } else {
                            show_toast_message(&ui, "✓ Día festivo eliminado correctamente.");
                        }

                        load_calendar_ui(&ui, pool, None, None, None);
                    }
                    Err(e) => {
                        app_data.set_cal_show_delete_holiday_modal(false);
                        show_toast_message(&ui, &format!("Error: {}", e));
                    }
                }
            }
        });
    }

    // Clic en Día -> Despliega Cuadro de Detalle con toda la información
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_day_clicked(move |date_iso| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let date = match parse_date_opt(date_iso.as_str()) {
                    Some(d) => d,
                    None => return,
                };

                let pool = match pool_clone {
                    Some(ref p) => p,
                    None => return,
                };
                let mut client = match pool.get() {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!("Error db en cal_day_clicked: {}", e);
                        return;
                    }
                };

                let holidays = fetch_all_holidays(&mut client).unwrap_or_default();
                let holidays_set: HashSet<NaiveDate> = holidays.iter().map(|h| h.fecha).collect();
                let revisions = fetch_all_revisions_for_calendar(&mut client, &holidays_set).unwrap_or_default();

                let holiday_opt = holidays.iter().find(|h| h.fecha == date);
                let today = chrono::Local::now().date_naive();
                let is_today = date == today;

                app_data.set_cal_detail_date_str(date.format("%d/%m/%Y").to_string().into());
                app_data.set_cal_detail_date_weekday(spanish_weekday(date).into());
                app_data.set_cal_detail_is_today(is_today);

                if let Some(hol) = holiday_opt {
                    app_data.set_cal_detail_is_holiday(true);
                    app_data.set_cal_detail_holiday_motivo(hol.motivo.clone().into());
                    app_data.set_cal_detail_holiday_is_system(hol.es_sistema);
                } else {
                    app_data.set_cal_detail_is_holiday(false);
                    app_data.set_cal_detail_holiday_motivo("".into());
                    app_data.set_cal_detail_holiday_is_system(false);
                }

                let milestones = get_milestones_for_date(date, &revisions);
                let slint_milestones: Vec<CalendarDayMilestone> = milestones
                    .into_iter()
                    .map(|m| {
                        let (fg_r, fg_g, fg_b) = parse_hex_color(&m.color_fg);
                        let (bg_r, bg_g, bg_b) = parse_hex_color(&m.color_bg);
                        let (bd_r, bd_g, bd_b) = parse_hex_color(&m.color_border);

                        CalendarDayMilestone {
                            rev_id: m.rev_id.into(),
                            folio: m.folio.into(),
                            nombre: m.nombre.into(),
                            rp_numero: m.rp_numero.into(),
                            cliente_nombre: m.cliente_nombre.into(),
                            milestone_name: m.milestone_name.into(),
                            milestone_desc: m.milestone_desc.into(),
                            color_fg: slint::Color::from_argb_u8(255, fg_r, fg_g, fg_b),
                            color_bg: slint::Color::from_argb_u8(255, bg_r, bg_g, bg_b),
                            color_border: slint::Color::from_argb_u8(255, bd_r, bd_g, bd_b),
                        }
                    })
                    .collect();

                app_data.set_cal_detail_milestones_count(slint_milestones.len() as i32);
                app_data.set_cal_detail_milestones(ModelRc::new(VecModel::from(slint_milestones)));

                app_data.set_cal_new_holiday_date(date.format("%d/%m/%Y").to_string().into());
                app_data.set_cal_show_day_detail_modal(true);
            }
        });
    }

    // Cerrar Modal Detalle de Día
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_cal_close_day_detail_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_cal_show_day_detail_modal(false);
            }
        });
    }

    // Inspeccionar Revisión desde el Modal de Detalle
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = pool_opt.clone();
        app_logic.on_cal_inspect_revision_from_detail(move |rev_id| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    ui.global::<AppData>().set_cal_show_day_detail_modal(false);
                    let rev_id_str = rev_id.to_string();
                    load_calendar_ui(&ui, pool, None, None, Some(&rev_id_str));
                }
            }
        });
    }

    // Ir a la vista de revisiones seleccionando esa revisión
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_cal_go_to_revision_view(move |rev_id| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_selected_revision_id(rev_id);
                ui.global::<AppLogic>().invoke_navigate_view(3);
            }
        });
    }
}
