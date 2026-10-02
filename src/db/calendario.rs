use std::collections::HashSet;
use chrono::{Datelike, NaiveDate};
use postgres::Client;
use uuid::Uuid;
use crate::db::revisiones::add_business_days;

#[derive(Clone, Debug, Default)]
pub struct CalendarHoliday {
    pub id: String,
    pub fecha: NaiveDate,
    pub fecha_str: String,
    pub fecha_iso: String,
    pub dia: String,
    pub motivo: String,
    pub es_sistema: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CalendarRevision {
    pub id: String,
    pub nombre: String,
    pub folio: String,
    pub cliente_nombre: String,
    pub rp_numero: String,
    pub estatus: String,
    pub estatus_imss: String,
    pub fecha_entrega_imss: Option<NaiveDate>,
    pub fecha_maxima_aprobacion_imss: Option<NaiveDate>,
    pub fecha_aprobacion_imss: Option<NaiveDate>,
    pub fecha_maxima_entrega_revision: Option<NaiveDate>,
    pub fecha_entrega_revision: Option<NaiveDate>,
    pub fecha_maxima_entrega_imss: Option<NaiveDate>,
    pub fecha_aprobacion_prorroga: Option<NaiveDate>,
    pub fecha_entrega_con_prorroga: Option<NaiveDate>,
    pub pidio_prorroga: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CalendarDayCellData {
    pub day_number: i32,
    pub month: i32,
    pub year: i32,
    pub date_str: String,
    pub date_iso: String,
    pub is_current_month: bool,
    pub is_today: bool,
    pub is_weekend: bool,
    pub is_holiday: bool,
    pub holiday_motivo: String,
    pub holiday_is_system: bool,
    pub holiday_id: String,
    pub has_milestones: bool,
    pub milestone_count: i32,
    pub summary_label: String,
    pub color_cell_bg: String,
    pub color_cell_border: String,
    pub has_events: bool,
    pub badge1_label: String,
    pub badge1_color_fg: String,
    pub badge1_color_bg: String,
    pub badge2_label: String,
    pub badge2_color_fg: String,
    pub badge2_color_bg: String,
    pub badge3_label: String,
    pub badge3_color_fg: String,
    pub badge3_color_bg: String,
    pub more_count: i32,
    pub is_highlighted: bool,
    pub highlight_color: String,
    pub highlight_label: String,
}

#[derive(Clone, Debug, Default)]
pub struct DayMilestoneDetail {
    pub rev_id: String,
    pub folio: String,
    pub nombre: String,
    pub rp_numero: String,
    pub cliente_nombre: String,
    pub milestone_name: String,
    pub milestone_desc: String,
    pub color_fg: String,
    pub color_bg: String,
    pub color_border: String,
}

pub fn spanish_weekday(d: NaiveDate) -> &'static str {
    match d.weekday() {
        chrono::Weekday::Mon => "Lunes",
        chrono::Weekday::Tue => "Martes",
        chrono::Weekday::Wed => "Miércoles",
        chrono::Weekday::Thu => "Jueves",
        chrono::Weekday::Fri => "Viernes",
        chrono::Weekday::Sat => "Sábado",
        chrono::Weekday::Sun => "Domingo",
    }
}

pub fn spanish_weekday_upper(d: NaiveDate) -> &'static str {
    match d.weekday() {
        chrono::Weekday::Mon => "LUNES",
        chrono::Weekday::Tue => "MARTES",
        chrono::Weekday::Wed => "MIÉRCOLES",
        chrono::Weekday::Thu => "JUEVES",
        chrono::Weekday::Fri => "VIERNES",
        chrono::Weekday::Sat => "SÁBADO",
        chrono::Weekday::Sun => "DOMINGO",
    }
}

pub fn spanish_month_name(month: u32) -> &'static str {
    match month {
        1 => "Enero",
        2 => "Febrero",
        3 => "Marzo",
        4 => "Abril",
        5 => "Mayo",
        6 => "Junio",
        7 => "Julio",
        8 => "Agosto",
        9 => "Septiembre",
        10 => "Octubre",
        11 => "Noviembre",
        12 => "Diciembre",
        _ => "Mes",
    }
}

pub fn fetch_all_holidays(client: &mut Client) -> Result<Vec<CalendarHoliday>, String> {
    let has_es_sistema: bool = client
        .query_one(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.columns 
                WHERE table_schema = 'public' 
                  AND table_name = 'imss_inhabiles' 
                  AND column_name = 'es_sistema'
            )",
            &[],
        )
        .map(|r| r.get(0))
        .unwrap_or(false);

    let query_str = if has_es_sistema {
        "SELECT id, fecha, COALESCE(dia, ''), COALESCE(motivo, ''), COALESCE(es_sistema, false)
         FROM public.imss_inhabiles
         ORDER BY fecha ASC"
    } else {
        "SELECT id, fecha, COALESCE(dia, ''), COALESCE(motivo, ''), false
         FROM public.imss_inhabiles
         ORDER BY fecha ASC"
    };

    let rows = client
        .query(query_str, &[])
        .map_err(|e| format!("Error consultando días festivos IMSS: {}", e))?;

    let mut list = Vec::new();
    for row in rows {
        let id: Uuid = row.get(0);
        let fecha: NaiveDate = row.get(1);
        let dia: String = row.get(2);
        let motivo: String = row.get(3);
        let es_sistema: bool = row.get(4);

        let dia_calc = if dia.trim().is_empty() {
            spanish_weekday(fecha).to_string()
        } else {
            dia.trim().to_string()
        };

        list.push(CalendarHoliday {
            id: id.to_string(),
            fecha,
            fecha_str: fecha.format("%d/%m/%Y").to_string(),
            fecha_iso: fecha.format("%Y-%m-%d").to_string(),
            dia: dia_calc,
            motivo: motivo.trim().to_string(),
            es_sistema,
        });
    }
    Ok(list)
}

pub fn insert_user_holiday(
    client: &mut Client,
    fecha: NaiveDate,
    motivo: &str,
) -> Result<CalendarHoliday, String> {
    let motivo_trimmed = motivo.trim();
    if motivo_trimmed.is_empty() {
        return Err("El motivo del día festivo es obligatorio.".to_string());
    }

    // Validación estricta: No permitir superposición de días festivos
    let count: i64 = client
        .query_one(
            "SELECT COUNT(*)::bigint FROM public.imss_inhabiles WHERE fecha = $1",
            &[&fecha],
        )
        .map_err(|e| format!("Error verificando superposición de festivo: {}", e))?
        .get(0);

    if count > 0 {
        return Err(format!(
            "Ya existe un día festivo registrado en la fecha {}. No se permiten fechas duplicadas o superpuestas.",
            fecha.format("%d/%m/%Y")
        ));
    }

    let dia = spanish_weekday_upper(fecha);
    let row = client
        .query_one(
            "INSERT INTO public.imss_inhabiles (fecha, dia, motivo, es_sistema)
             VALUES ($1, $2, $3, false)
             RETURNING id",
            &[&fecha, &dia, &motivo_trimmed],
        )
        .map_err(|e| format!("Error registrando día festivo: {}", e))?;

    let id: Uuid = row.get(0);

    Ok(CalendarHoliday {
        id: id.to_string(),
        fecha,
        fecha_str: fecha.format("%d/%m/%Y").to_string(),
        fecha_iso: fecha.format("%Y-%m-%d").to_string(),
        dia: dia.to_string(),
        motivo: motivo_trimmed.to_string(),
        es_sistema: false,
    })
}

pub fn delete_user_holiday(client: &mut Client, id_str: &str) -> Result<(), String> {
    let uuid = Uuid::parse_str(id_str).map_err(|e| format!("ID de día festivo inválido: {}", e))?;

    let row_opt = client
        .query_opt(
            "SELECT es_sistema, fecha, motivo FROM public.imss_inhabiles WHERE id = $1",
            &[&uuid],
        )
        .map_err(|e| format!("Error consultando día festivo: {}", e))?;

    let row = match row_opt {
        Some(r) => r,
        None => return Err("El día festivo no existe o ya fue eliminado.".to_string()),
    };

    let es_sistema: bool = row.get(0);
    let fecha: NaiveDate = row.get(1);
    let motivo: String = row.get(2);

    if es_sistema {
        return Err(format!(
            "El día festivo '{}' ({}) es un festivo oficial del sistema y NO puede ser eliminado por los usuarios.",
            motivo.trim(),
            fecha.format("%d/%m/%Y")
        ));
    }

    let rows_affected = client
        .execute(
            "DELETE FROM public.imss_inhabiles WHERE id = $1 AND es_sistema = false",
            &[&uuid],
        )
        .map_err(|e| format!("Error eliminando día festivo del usuario: {}", e))?;

    if rows_affected == 0 {
        return Err("No se pudo eliminar el día festivo.".to_string());
    }

    Ok(())
}

pub fn fetch_all_revisions_for_calendar(
    client: &mut Client,
    holidays: &HashSet<NaiveDate>,
) -> Result<Vec<CalendarRevision>, String> {
    let query = r#"
        SELECT 
            r.id,
            COALESCE(r.nombre, 'Revisión S/N'),
            COALESCE(r.folio, 'S/F'),
            COALESCE(cl.nombre, 'Sin Cliente Asignado'),
            COALESCE(rp.registro, 'S/RP'),
            COALESCE(e.nombre, 'EN REVISIÓN'),
            COALESCE(e_imss.nombre, 'PENDIENTE DICTAMEN IMSS'),
            r.fecha_entrega_imss,
            r.fecha_maxima_aprobacion_imss,
            r.fecha_aprobacion_imss,
            r.fecha_maxima_entrega_revision,
            r.fecha_entrega_revision,
            r.fecha_maxima_entrega_imss,
            r.fecha_aprobacion_prorroga,
            r.fecha_entrega_con_prorroga,
            COALESCE(r.pidio_prorroga, false)
        FROM public.revisiones r
        JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
        JOIN public.clientes cl ON cl.id = rp.id_cliente
        LEFT JOIN public.estatus e ON e.id = r.id_estatus
        LEFT JOIN public.estatus e_imss ON e_imss.id = r.id_estatus_imss
        ORDER BY r.created_at DESC;
    "#;

    let rows = client
        .query(query, &[])
        .map_err(|e| format!("Error consultando revisiones para calendario: {}", e))?;

    let mut list = Vec::new();
    for row in rows {
        let id_u: Uuid = row.get(0);
        let nombre: String = row.get(1);
        let folio: String = row.get(2);
        let cliente_nombre: String = row.get(3);
        let rp_numero: String = row.get(4);
        let estatus: String = row.get(5);
        let estatus_imss: String = row.get(6);

        let f_ent_imss: Option<NaiveDate> = row.get(7);
        let _raw_f_max_aprob: Option<NaiveDate> = row.get(8);
        let f_aprob_imss: Option<NaiveDate> = row.get(9);
        let _raw_f_max_ent_rev: Option<NaiveDate> = row.get(10);
        let f_ent_rev: Option<NaiveDate> = row.get(11);
        let _raw_f_max_ent_imss: Option<NaiveDate> = row.get(12);
        let f_aprob_pror: Option<NaiveDate> = row.get(13);
        let _raw_f_ent_pror: Option<NaiveDate> = row.get(14);
        let pidio_pror: bool = row.get(15);

        // Autocálculo de alta precisión según reglas legales IMSS:
        // 1. Fecha Máxima Aprobación IMSS: 15 días hábiles a partir de Fecha Entrega IMSS
        let f_max_aprob_imss = f_ent_imss.map(|d| add_business_days(d, 15, Some(holidays)));

        // 2. Fecha Máxima Entrega Revisión: 40 días hábiles a partir de Fecha Aprobación IMSS (o f_max_aprob_imss si pendiente)
        let base_aprob = f_aprob_imss.or(f_max_aprob_imss);
        let f_max_ent_rev = base_aprob.map(|d| add_business_days(d, 40, Some(holidays)));

        // 3. Fecha Máxima Entrega IMSS: 120 días hábiles a partir de Fecha Entrega Revisión (si se ha entregado)
        let f_max_ent_imss = f_ent_rev.map(|d| add_business_days(d, 120, Some(holidays)));

        // 4. Fecha Entrega con Prórroga: 10 días hábiles a partir de Fecha Máxima Entrega Revisión (si pidió prórroga o tiene fecha aprobación prórroga)
        let f_ent_pror = if pidio_pror || f_aprob_pror.is_some() {
            f_max_ent_rev.map(|d_max| add_business_days(d_max, 10, Some(holidays)))
        } else {
            None
        };

        list.push(CalendarRevision {
            id: id_u.to_string(),
            nombre: nombre.trim().to_string(),
            folio: folio.trim().to_string(),
            cliente_nombre: cliente_nombre.trim().to_string(),
            rp_numero: rp_numero.trim().to_string(),
            estatus: estatus.trim().to_string(),
            estatus_imss: estatus_imss.trim().to_string(),
            fecha_entrega_imss: f_ent_imss,
            fecha_maxima_aprobacion_imss: f_max_aprob_imss,
            fecha_aprobacion_imss: f_aprob_imss,
            fecha_maxima_entrega_revision: f_max_ent_rev,
            fecha_entrega_revision: f_ent_rev,
            fecha_maxima_entrega_imss: f_max_ent_imss,
            fecha_aprobacion_prorroga: f_aprob_pror,
            fecha_entrega_con_prorroga: f_ent_pror,
            pidio_prorroga: pidio_pror,
        });
    }

    Ok(list)
}

struct MilestoneBadgeDef {
    label: String,
    fg: &'static str,
    bg: &'static str,
    _border: &'static str,
}

pub fn generate_month_cells(
    year: i32,
    month: u32,
    today: NaiveDate,
    holidays: &[CalendarHoliday],
    revisions: &[CalendarRevision],
    selected_rev_id: Option<&str>,
) -> Vec<CalendarDayCellData> {
    let first_day = NaiveDate::from_ymd_opt(year, month, 1)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());

    // Monday = 0, Tuesday = 1, ... Sunday = 6
    let weekday_start = (first_day.weekday().number_from_monday() - 1) as i64;
    let grid_start_date = first_day - chrono::Duration::days(weekday_start);

    let selected_rev = selected_rev_id
        .and_then(|sel_id| revisions.iter().find(|r| r.id == sel_id));

    let mut cells = Vec::with_capacity(42);

    for i in 0..42 {
        let cur_date = grid_start_date + chrono::Duration::days(i);
        let is_current_month = cur_date.month() == month;
        let is_today = cur_date == today;
        let is_weekend = cur_date.weekday().number_from_monday() > 5;

        // Check if holiday
        let hol_opt = holidays.iter().find(|h| h.fecha == cur_date);
        let is_holiday = hol_opt.is_some();
        let holiday_motivo = hol_opt.map(|h| h.motivo.clone()).unwrap_or_default();
        let holiday_is_system = hol_opt.map(|h| h.es_sistema).unwrap_or(false);
        let holiday_id = hol_opt.map(|h| h.id.clone()).unwrap_or_default();

        let mut badges: Vec<MilestoneBadgeDef> = Vec::new();
        let mut is_highlighted = false;
        let mut highlight_color = String::new();
        let mut highlight_label = String::new();

        if let Some(rev) = selected_rev {
            // Selected revision: highlight with specific distinctive milestone colors
            // 1. Fecha Entrega IMSS (Azul #2563eb)
            if rev.fecha_entrega_imss == Some(cur_date) {
                badges.push(MilestoneBadgeDef {
                    label: "Entrega IMSS".to_string(),
                    fg: "#1e40af",
                    bg: "#dbeafe",
                    _border: "#3b82f6",
                });
                is_highlighted = true;
                highlight_color = "#3b82f6".to_string();
                highlight_label = "Fecha Entrega IMSS".to_string();
            }

            // 2. Fecha Máxima Aprobación IMSS (Cian / Sky #0284c7)
            if rev.fecha_maxima_aprobacion_imss == Some(cur_date) {
                badges.push(MilestoneBadgeDef {
                    label: "Máx. Aprob. IMSS (15d)".to_string(),
                    fg: "#0369a1",
                    bg: "#e0f2fe",
                    _border: "#38bdf8",
                });
                is_highlighted = true;
                highlight_color = "#0284c7".to_string();
                highlight_label = "Fecha Máx. Aprobación IMSS (15 días hábiles)".to_string();
            }

            // 3. Fecha Aprobación IMSS (Verde Esmeralda #059669)
            if rev.fecha_aprobacion_imss == Some(cur_date) {
                badges.push(MilestoneBadgeDef {
                    label: "Aprobación IMSS".to_string(),
                    fg: "#047857",
                    bg: "#d1fae5",
                    _border: "#10b981",
                });
                is_highlighted = true;
                highlight_color = "#10b981".to_string();
                highlight_label = "Fecha Aprobación IMSS".to_string();
            }

            // 4. Fecha Máxima Entrega Revisión (Púrpura #7c3aed)
            if rev.fecha_maxima_entrega_revision == Some(cur_date) {
                badges.push(MilestoneBadgeDef {
                    label: "Máx. Entrega Rev. (40d)".to_string(),
                    fg: "#6d28d9",
                    bg: "#ede9fe",
                    _border: "#8b5cf6",
                });
                is_highlighted = true;
                highlight_color = "#7c3aed".to_string();
                highlight_label = "Fecha Máx. Entrega Revisión (40 días hábiles)".to_string();
            }

            // 5. Fecha Entrega Revisión (Teal #0d9488)
            if rev.fecha_entrega_revision == Some(cur_date) {
                badges.push(MilestoneBadgeDef {
                    label: "Entrega Revisión".to_string(),
                    fg: "#0f766e",
                    bg: "#ccfbf1",
                    _border: "#14b8a6",
                });
                is_highlighted = true;
                highlight_color = "#14b8a6".to_string();
                highlight_label = "Fecha Entrega Revisión".to_string();
            }

            // 6. Fecha Máxima Entrega IMSS (Rosa/Rojo #e11d48)
            if rev.fecha_maxima_entrega_imss == Some(cur_date) {
                badges.push(MilestoneBadgeDef {
                    label: "Máx. Entrega IMSS (120d)".to_string(),
                    fg: "#be123c",
                    bg: "#ffe4e6",
                    _border: "#f43f5e",
                });
                is_highlighted = true;
                highlight_color = "#e11d48".to_string();
                highlight_label = "Fecha Máx. Entrega IMSS (120 días hábiles)".to_string();
            }

            // 7. Fecha Aprobación Prórroga (Naranja #ea580c)
            if rev.fecha_aprobacion_prorroga == Some(cur_date) {
                badges.push(MilestoneBadgeDef {
                    label: "Aprob. Prórroga".to_string(),
                    fg: "#c2410c",
                    bg: "#ffedd5",
                    _border: "#f97316",
                });
                is_highlighted = true;
                highlight_color = "#f97316".to_string();
                highlight_label = "Fecha Aprobación Prórroga".to_string();
            }

            // 8. Fecha Entrega con Prórroga (Fucsia/Violeta #c026d3)
            if rev.fecha_entrega_con_prorroga == Some(cur_date) {
                badges.push(MilestoneBadgeDef {
                    label: "Entrega Prórroga (+10d)".to_string(),
                    fg: "#a21caf",
                    bg: "#fae8ff",
                    _border: "#d946ef",
                });
                is_highlighted = true;
                highlight_color = "#c026d3".to_string();
                highlight_label = "Fecha Entrega con Prórroga (10 días hábiles)".to_string();
            }
        } else {
            // All revisions: show badges for all milestones across revisions
            for r in revisions {
                let fol = if r.folio.is_empty() || r.folio == "S/F" {
                    r.nombre.chars().take(8).collect::<String>()
                } else {
                    r.folio.chars().take(8).collect::<String>()
                };

                if r.fecha_entrega_imss == Some(cur_date) {
                    badges.push(MilestoneBadgeDef {
                        label: format!("Entr: {}", fol),
                        fg: "#1e40af",
                        bg: "#dbeafe",
                        _border: "#3b82f6",
                    });
                }
                if r.fecha_maxima_aprobacion_imss == Some(cur_date) {
                    badges.push(MilestoneBadgeDef {
                        label: format!("Máx: {}", fol),
                        fg: "#0369a1",
                        bg: "#e0f2fe",
                        _border: "#38bdf8",
                    });
                }
                if r.fecha_aprobacion_imss == Some(cur_date) {
                    badges.push(MilestoneBadgeDef {
                        label: format!("Aprob: {}", fol),
                        fg: "#047857",
                        bg: "#d1fae5",
                        _border: "#10b981",
                    });
                }
                if r.fecha_maxima_entrega_revision == Some(cur_date) {
                    badges.push(MilestoneBadgeDef {
                        label: format!("Rev: {}", fol),
                        fg: "#6d28d9",
                        bg: "#ede9fe",
                        _border: "#8b5cf6",
                    });
                }
                if r.fecha_entrega_revision == Some(cur_date) {
                    badges.push(MilestoneBadgeDef {
                        label: format!("Ent.Rev: {}", fol),
                        fg: "#0f766e",
                        bg: "#ccfbf1",
                        _border: "#14b8a6",
                    });
                }
                if r.fecha_maxima_entrega_imss == Some(cur_date) {
                    badges.push(MilestoneBadgeDef {
                        label: format!("120d: {}", fol),
                        fg: "#be123c",
                        bg: "#ffe4e6",
                        _border: "#f43f5e",
                    });
                }
                if r.fecha_aprobacion_prorroga == Some(cur_date) {
                    badges.push(MilestoneBadgeDef {
                        label: format!("Prór: {}", fol),
                        fg: "#c2410c",
                        bg: "#ffedd5",
                        _border: "#f97316",
                    });
                }
                if r.fecha_entrega_con_prorroga == Some(cur_date) {
                    badges.push(MilestoneBadgeDef {
                        label: format!("+10d: {}", fol),
                        fg: "#a21caf",
                        bg: "#fae8ff",
                        _border: "#d946ef",
                    });
                }
            }
        }

        let milestone_count = badges.len() as i32;
        let has_milestones = milestone_count > 0;
        let has_events = is_holiday || has_milestones || is_highlighted;

        let summary_label = if milestone_count > 2 || (is_holiday && milestone_count > 1) {
            format!("● {} eventos", milestone_count + if is_holiday { 1 } else { 0 })
        } else if milestone_count == 1 && !is_holiday {
            badges.first().map(|b| b.label.clone()).unwrap_or_default()
        } else {
            String::new()
        };

        let color_cell_bg = if is_highlighted {
            "#eff6ff".to_string()
        } else if is_today {
            "#eff6ff".to_string()
        } else if is_holiday {
            "#fffbeb".to_string()
        } else if is_weekend {
            "#f8fafc".to_string()
        } else {
            "#ffffff".to_string()
        };

        let color_cell_border = if is_highlighted {
            highlight_color.clone()
        } else if is_today {
            "#3b82f6".to_string()
        } else if is_holiday {
            "#fcd34d".to_string()
        } else {
            "#e2e8f0".to_string()
        };

        let badge1_label = badges.get(0).map(|b| b.label.clone()).unwrap_or_default();
        let badge1_color_fg = badges.get(0).map(|b| b.fg.to_string()).unwrap_or_default();
        let badge1_color_bg = badges.get(0).map(|b| b.bg.to_string()).unwrap_or_default();

        let badge2_label = badges.get(1).map(|b| b.label.clone()).unwrap_or_default();
        let badge2_color_fg = badges.get(1).map(|b| b.fg.to_string()).unwrap_or_default();
        let badge2_color_bg = badges.get(1).map(|b| b.bg.to_string()).unwrap_or_default();

        let badge3_label = badges.get(2).map(|b| b.label.clone()).unwrap_or_default();
        let badge3_color_fg = badges.get(2).map(|b| b.fg.to_string()).unwrap_or_default();
        let badge3_color_bg = badges.get(2).map(|b| b.bg.to_string()).unwrap_or_default();

        let more_count = if milestone_count > 3 {
            milestone_count - 3
        } else {
            0
        };

        cells.push(CalendarDayCellData {
            day_number: cur_date.day() as i32,
            month: cur_date.month() as i32,
            year: cur_date.year(),
            date_str: cur_date.format("%d/%m/%Y").to_string(),
            date_iso: cur_date.format("%Y-%m-%d").to_string(),
            is_current_month,
            is_today,
            is_weekend,
            is_holiday,
            holiday_motivo,
            holiday_is_system,
            holiday_id,
            has_milestones,
            milestone_count,
            summary_label,
            color_cell_bg,
            color_cell_border,
            has_events,
            badge1_label,
            badge1_color_fg,
            badge1_color_bg,
            badge2_label,
            badge2_color_fg,
            badge2_color_bg,
            badge3_label,
            badge3_color_fg,
            badge3_color_bg,
            more_count,
            is_highlighted,
            highlight_color,
            highlight_label,
        });
    }

    cells
}

pub fn get_milestones_for_date(
    date: NaiveDate,
    revisions: &[CalendarRevision],
) -> Vec<DayMilestoneDetail> {
    let mut details = Vec::new();
    for r in revisions {
        let fol = if r.folio.is_empty() || r.folio == "S/F" {
            "S/F".to_string()
        } else {
            r.folio.clone()
        };

        if r.fecha_entrega_imss == Some(date) {
            details.push(DayMilestoneDetail {
                rev_id: r.id.clone(),
                folio: fol.clone(),
                nombre: r.nombre.clone(),
                rp_numero: r.rp_numero.clone(),
                cliente_nombre: r.cliente_nombre.clone(),
                milestone_name: "Fecha Entrega IMSS".to_string(),
                milestone_desc: "Fecha de entrega formal del expediente. Base legal para el inicio del cómputo de plazos.".to_string(),
                color_fg: "#1e40af".to_string(),
                color_bg: "#dbeafe".to_string(),
                color_border: "#3b82f6".to_string(),
            });
        }

        if r.fecha_maxima_aprobacion_imss == Some(date) {
            details.push(DayMilestoneDetail {
                rev_id: r.id.clone(),
                folio: fol.clone(),
                nombre: r.nombre.clone(),
                rp_numero: r.rp_numero.clone(),
                cliente_nombre: r.cliente_nombre.clone(),
                milestone_name: "Fecha Máx. Aprobación IMSS (15 días hábiles)".to_string(),
                milestone_desc: "Plazo límite legal de 15 días hábiles del Instituto para emitir respuesta/aprobación.".to_string(),
                color_fg: "#0369a1".to_string(),
                color_bg: "#e0f2fe".to_string(),
                color_border: "#38bdf8".to_string(),
            });
        }

        if r.fecha_aprobacion_imss == Some(date) {
            details.push(DayMilestoneDetail {
                rev_id: r.id.clone(),
                folio: fol.clone(),
                nombre: r.nombre.clone(),
                rp_numero: r.rp_numero.clone(),
                cliente_nombre: r.cliente_nombre.clone(),
                milestone_name: "Fecha Aprobación IMSS".to_string(),
                milestone_desc: "Fecha formal en que el IMSS emitió la aprobación de la revisión patronal.".to_string(),
                color_fg: "#047857".to_string(),
                color_bg: "#d1fae5".to_string(),
                color_border: "#10b981".to_string(),
            });
        }

        if r.fecha_maxima_entrega_revision == Some(date) {
            details.push(DayMilestoneDetail {
                rev_id: r.id.clone(),
                folio: fol.clone(),
                nombre: r.nombre.clone(),
                rp_numero: r.rp_numero.clone(),
                cliente_nombre: r.cliente_nombre.clone(),
                milestone_name: "Fecha Máx. Entrega Revisión (40 días hábiles)".to_string(),
                milestone_desc: "Plazo improrrogable de 40 días hábiles tras la aprobación IMSS para entrega del dictamen.".to_string(),
                color_fg: "#6d28d9".to_string(),
                color_bg: "#ede9fe".to_string(),
                color_border: "#8b5cf6".to_string(),
            });
        }

        if r.fecha_entrega_revision == Some(date) {
            details.push(DayMilestoneDetail {
                rev_id: r.id.clone(),
                folio: fol.clone(),
                nombre: r.nombre.clone(),
                rp_numero: r.rp_numero.clone(),
                cliente_nombre: r.cliente_nombre.clone(),
                milestone_name: "Fecha Entrega Revisión".to_string(),
                milestone_desc: "Fecha efectiva en que se entregó el dictamen y documentación de la revisión.".to_string(),
                color_fg: "#0f766e".to_string(),
                color_bg: "#ccfbf1".to_string(),
                color_border: "#14b8a6".to_string(),
            });
        }

        if r.fecha_maxima_entrega_imss == Some(date) {
            details.push(DayMilestoneDetail {
                rev_id: r.id.clone(),
                folio: fol.clone(),
                nombre: r.nombre.clone(),
                rp_numero: r.rp_numero.clone(),
                cliente_nombre: r.cliente_nombre.clone(),
                milestone_name: "Fecha Máx. Entrega IMSS (120 días hábiles)".to_string(),
                milestone_desc: "Plazo límite total de 120 días hábiles computados a partir de la entrega de revisión.".to_string(),
                color_fg: "#be123c".to_string(),
                color_bg: "#ffe4e6".to_string(),
                color_border: "#f43f5e".to_string(),
            });
        }

        if r.fecha_aprobacion_prorroga == Some(date) {
            details.push(DayMilestoneDetail {
                rev_id: r.id.clone(),
                folio: fol.clone(),
                nombre: r.nombre.clone(),
                rp_numero: r.rp_numero.clone(),
                cliente_nombre: r.cliente_nombre.clone(),
                milestone_name: "Fecha Aprobación Prórroga".to_string(),
                milestone_desc: "Fecha en que se autorizó formalmente la extensión de prórroga para la revisión.".to_string(),
                color_fg: "#c2410c".to_string(),
                color_bg: "#ffedd5".to_string(),
                color_border: "#f97316".to_string(),
            });
        }

        if r.fecha_entrega_con_prorroga == Some(date) {
            details.push(DayMilestoneDetail {
                rev_id: r.id.clone(),
                folio: fol.clone(),
                nombre: r.nombre.clone(),
                rp_numero: r.rp_numero.clone(),
                cliente_nombre: r.cliente_nombre.clone(),
                milestone_name: "Fecha Entrega con Prórroga (10 días hábiles)".to_string(),
                milestone_desc: "Plazo extendido de 10 días hábiles adicionales a partir de la Fecha Máx. Entrega Revisión.".to_string(),
                color_fg: "#a21caf".to_string(),
                color_bg: "#fae8ff".to_string(),
                color_border: "#d946ef".to_string(),
            });
        }
    }
    details
}

pub fn recalculate_affected_revisions(
    client: &mut Client,
    holidays: &HashSet<NaiveDate>,
) -> Result<usize, String> {
    let rows = client
        .query(
            "SELECT 
                id, 
                fecha_entrega_imss, 
                fecha_aprobacion_imss, 
                fecha_maxima_aprobacion_imss, 
                fecha_maxima_entrega_revision, 
                fecha_maxima_entrega_imss, 
                fecha_entrega_revision, 
                fecha_aprobacion_prorroga, 
                fecha_entrega_con_prorroga, 
                COALESCE(pidio_prorroga, false) 
             FROM public.revisiones",
            &[],
        )
        .map_err(|e| format!("Error consultando revisiones para recálculo: {}", e))?;

    let mut affected_count = 0;

    for row in rows {
        let rev_id: Uuid = row.get(0);
        let f_ent_imss: Option<NaiveDate> = row.get(1);
        let f_aprob_imss: Option<NaiveDate> = row.get(2);
        let current_max_aprob: Option<NaiveDate> = row.get(3);
        let current_max_ent_rev: Option<NaiveDate> = row.get(4);
        let current_max_ent_imss: Option<NaiveDate> = row.get(5);
        let f_ent_rev: Option<NaiveDate> = row.get(6);
        let f_aprob_pror: Option<NaiveDate> = row.get(7);
        let current_ent_pror: Option<NaiveDate> = row.get(8);
        let pidio_pror: bool = row.get(9);

        // Recalculate legal deadlines with the current set of holidays
        let new_max_aprob = f_ent_imss.map(|d| add_business_days(d, 15, Some(holidays)));
        let base_aprob = f_aprob_imss.or(new_max_aprob);
        let new_max_ent_rev = base_aprob.map(|d| add_business_days(d, 40, Some(holidays)));
        let new_max_ent_imss = f_ent_rev.map(|d| add_business_days(d, 120, Some(holidays)));
        let new_ent_pror = if pidio_pror || f_aprob_pror.is_some() {
            new_max_ent_rev.map(|d_max| add_business_days(d_max, 10, Some(holidays)))
        } else {
            None
        };

        let has_changed = new_max_aprob != current_max_aprob
            || new_max_ent_rev != current_max_ent_rev
            || new_max_ent_imss != current_max_ent_imss
            || new_ent_pror != current_ent_pror;

        if has_changed {
            client
                .execute(
                    "UPDATE public.revisiones SET
                        fecha_maxima_aprobacion_imss = $2,
                        fecha_maxima_entrega_revision = $3,
                        fecha_maxima_entrega_imss = $4,
                        fecha_entrega_con_prorroga = $5
                     WHERE id = $1",
                    &[
                        &rev_id,
                        &new_max_aprob,
                        &new_max_ent_rev,
                        &new_max_ent_imss,
                        &new_ent_pror,
                    ],
                )
                .map_err(|e| format!("Error actualizando fechas de revisión {}: {}", rev_id, e))?;

            affected_count += 1;
        }
    }

    Ok(affected_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autocalculation_matches_user_specification() {
        // User example from prompt:
        // Fecha Entrega IMSS: 16/07/2026
        // Fecha Maxima Aprobacion: 06/08/2026 (15 días hábiles)
        // Fecha Aprobacion IMSS: 06/08/2026
        // Fecha Maxima Entrega Revision: 02/10/2026 (40 días hábiles)
        // Fecha Aprobacion Prorroga: 28/09/2026
        // Fecha Entrega con Prorroga: 16/10/2026 (10 días hábiles a partir de Fecha Maxima Entrega Revision)

        let mut holidays = HashSet::new();
        // Standard Mexican holiday in September: 16 de Septiembre
        holidays.insert(NaiveDate::from_ymd_opt(2026, 9, 16).unwrap());

        let f_ent_imss = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();

        // 1. 15 días hábiles
        let f_max_aprob = add_business_days(f_ent_imss, 15, Some(&holidays));
        assert_eq!(
            f_max_aprob,
            NaiveDate::from_ymd_opt(2026, 8, 6).unwrap(),
            "Fecha Máxima Aprobación IMSS debe ser 06/08/2026"
        );

        // 2. 40 días hábiles desde Aprobación IMSS (06/08/2026)
        let f_aprob_imss = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        let f_max_ent_rev = add_business_days(f_aprob_imss, 40, Some(&holidays));
        assert_eq!(
            f_max_ent_rev,
            NaiveDate::from_ymd_opt(2026, 10, 2).unwrap(),
            "Fecha Máxima Entrega Revisión debe ser 02/10/2026"
        );

        // 3. 10 días hábiles desde Fecha Máxima Entrega Revisión (02/10/2026)
        let f_ent_pror = add_business_days(f_max_ent_rev, 10, Some(&holidays));
        assert_eq!(
            f_ent_pror,
            NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(),
            "Fecha Entrega con Prórroga debe ser 16/10/2026"
        );
    }

    #[test]
    fn test_holiday_impact_on_dates() {
        let mut holidays = HashSet::new();
        let f_ent_imss = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();

        // Sin festivos intermedios
        let f_max_aprob_orig = add_business_days(f_ent_imss, 15, Some(&holidays));
        assert_eq!(f_max_aprob_orig, NaiveDate::from_ymd_opt(2026, 8, 6).unwrap());

        // Si se inserta un festivo fuera del rango de la revisión (p. ej. 01/12/2026)
        holidays.insert(NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
        let f_max_aprob_after_unrelated = add_business_days(f_ent_imss, 15, Some(&holidays));
        // No debe cambiar la fecha de esta revisión
        assert_eq!(f_max_aprob_after_unrelated, f_max_aprob_orig);

        // Si se inserta un festivo que sí cae en el lapso (p. ej. 20/07/2026 lunes)
        holidays.insert(NaiveDate::from_ymd_opt(2026, 7, 20).unwrap());
        let f_max_aprob_after_related = add_business_days(f_ent_imss, 15, Some(&holidays));
        // Se debe recorrer un día hábil más: del jueves 06/08 al viernes 07/08
        assert_eq!(f_max_aprob_after_related, NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
    }

    #[test]
    fn test_generate_month_cells_count_and_boundaries() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        let cells = generate_month_cells(2026, 9, today, &[], &[], None);

        assert_eq!(cells.len(), 42, "Debe generar exactamente 42 celdas de cuadrícula");

        // September 2026: 1st is Tuesday (day of week Monday=0, Tuesday=1).
        // First cell is Monday 31/08/2026
        assert_eq!(cells[0].day_number, 31);
        assert_eq!(cells[0].month, 8);
        assert!(!cells[0].is_current_month);

        // Cell 1 is Tuesday 01/09/2026
        assert_eq!(cells[1].day_number, 1);
        assert_eq!(cells[1].month, 9);
        assert!(cells[1].is_current_month);

        // Cell for today (30/09/2026): cell index 1 + 29 = 30
        assert_eq!(cells[30].day_number, 30);
        assert!(cells[30].is_today);
    }
}
