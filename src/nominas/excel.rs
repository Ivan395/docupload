use rust_xlsxwriter::{Color, Format, FormatBorder, Workbook, Worksheet, XlsxError};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::nominas::models::Employee;
use crate::nominas::processor::{NominasStats, UnprocessedNomina};

pub fn sanitize_sheet_name(raw: &str) -> String {
    let mut cleaned = String::new();
    for c in raw.chars() {
        if c == '\\' || c == '/' || c == '?' || c == '*' || c == '[' || c == ']' || c == ':' {
            cleaned.push('_');
        } else {
            cleaned.push(c);
        }
    }
    let trimmed = cleaned.trim().trim_matches('\'');
    if trimmed.is_empty() {
        "Sheet1".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn make_nomina_sheet_name(
    nombre_emisor: Option<&str>,
    rfc_emisor: Option<&str>,
    existing_names: &mut HashSet<String>,
) -> String {
    let comp_short = match nombre_emisor {
        Some(nom) if !nom.trim().is_empty() => {
            let first_word = nom
                .trim()
                .split(|c: char| c.is_whitespace() || c == ',' || c == '.')
                .find(|s| !s.is_empty())
                .unwrap_or("empresa");
            first_word.to_lowercase()
        }
        _ => "empresa".to_string(),
    };

    let rfc_part = match rfc_emisor {
        Some(rfc) if !rfc.trim().is_empty() => {
            rfc.trim().to_uppercase().replace([' ', '/', '\\'], "_")
        }
        _ => "RFC".to_string(),
    };

    let mut base_name = format!("{}_{}", comp_short, rfc_part);
    base_name = sanitize_sheet_name(&base_name);
    if base_name.chars().count() > 31 {
        base_name = base_name.chars().take(31).collect();
    }

    let mut candidate = base_name.clone();
    let mut counter = 2;
    while existing_names.contains(&candidate.to_lowercase()) {
        let suffix = format!("_{}", counter);
        let max_base_len = 31 - suffix.len();
        let prefix: String = base_name.chars().take(max_base_len).collect();
        candidate = format!("{}{}", prefix, suffix);
        counter += 1;
    }

    existing_names.insert(candidate.to_lowercase());
    candidate
}

fn write_estadisticas_sheet(
    worksheet: &mut Worksheet,
    stats: &NominasStats,
    companies_map: &HashMap<(String, String), Vec<&Employee>>,
) -> Result<(), XlsxError> {
    worksheet.set_name("Estadisticas")?;

    let title_format = Format::new()
        .set_bold()
        .set_font_size(14)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1E3A8A))
        .set_align(rust_xlsxwriter::FormatAlign::Center);

    let kpi_label_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_background_color(Color::RGB(0xF1F5F9))
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xCBD5E1));

    let kpi_val_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xCBD5E1))
        .set_num_format("#,##0");

    let kpi_green_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::RGB(0x15803D))
        .set_background_color(Color::RGB(0xDCFCE7))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x86EFAC))
        .set_num_format("#,##0");

    let kpi_orange_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::RGB(0xC2410C))
        .set_background_color(Color::RGB(0xFFEDD5))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xFDBA74))
        .set_num_format("#,##0");

    let kpi_red_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::RGB(0xB91C1C))
        .set_background_color(Color::RGB(0xFEE2E2))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xFCA5A5))
        .set_num_format("#,##0");

    let currency_kpi_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::RGB(0x1E40AF))
        .set_background_color(Color::RGB(0xDBEAFE))
        .set_align(rust_xlsxwriter::FormatAlign::Right)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x93C5FD))
        .set_num_format("$#,##0.00");

    let percent_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xCBD5E1))
        .set_num_format("0.0%");

    let table_head_format = Format::new()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x2563EB))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin);

    let cell_text_format = Format::new()
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xE2E8F0));

    let cell_num_format = Format::new()
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xE2E8F0))
        .set_num_format("#,##0");

    let cell_curr_format = Format::new()
        .set_align(rust_xlsxwriter::FormatAlign::Right)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xE2E8F0))
        .set_num_format("$#,##0.00");

    // KPI Summary Table
    let kpi_data = [
        ("Total de Comprobantes XML Analizados", stats.total_files as f64, &kpi_val_format),
        ("Nóminas Válidas (Concentradas y Timbradas SAT)", stats.valid_count as f64, &kpi_green_format),
        ("Nóminas Duplicadas (Omitidas del Concentrado)", stats.duplicate_count as f64, &kpi_orange_format),
        ("Comprobantes Cancelados ante el SAT", stats.canceled_count as f64, &kpi_red_format),
        ("Comprobantes No Encontrados en el SAT", stats.not_found_count as f64, &kpi_red_format),
        ("Errores de Estructura / Lectura XML", (stats.parse_error_count + stats.other_invalid_count) as f64, &kpi_val_format),
    ];

    worksheet.write_string_with_format(2, 0, "INDICADOR MÉTRICO", &table_head_format)?;
    worksheet.write_string_with_format(2, 1, "CANTIDAD", &table_head_format)?;

    for (i, (label, val, fmt)) in kpi_data.iter().enumerate() {
        let row = (3 + i) as u32;
        worksheet.write_string_with_format(row, 0, *label, &kpi_label_format)?;
        worksheet.write_number_with_format(row, 1, *val, fmt)?;
    }

    let validez_rate = if stats.total_files > 0 {
        stats.valid_count as f64 / stats.total_files as f64
    } else {
        0.0
    };

    worksheet.write_string_with_format(9, 0, "Tasa de Validez y Efectividad Fiscal", &kpi_label_format)?;
    worksheet.write_number_with_format(9, 1, validez_rate, &percent_format)?;

    worksheet.write_string_with_format(10, 0, "Importe Total Bruto de Sueldos Válidos (Sueldo + 7mo Día)", &kpi_label_format)?;
    worksheet.write_number_with_format(10, 1, stats.total_sueldos_validos, &currency_kpi_format)?;

    let mut next_table_start_row = 13;
    if stats.total_septimo_dia_validos > 0.0 {
        worksheet.write_string_with_format(11, 0, "Importe Total de Séptimo Día (Incluido en Sueldos)", &kpi_label_format)?;
        worksheet.write_number_with_format(11, 1, stats.total_septimo_dia_validos, &currency_kpi_format)?;
        next_table_start_row = 14;
    }

    // Determine distinct payment periodicities present
    let standard_periodicities = [
        "Diario",
        "Semanal",
        "Decenal",
        "Catorcenal",
        "Quincenal",
        "Mensual",
        "Bimestral",
        "Unidad de obra",
        "Comisión",
        "Precio alzado",
        "Otra Periodicidad",
    ];

    let mut present_periodicities_set = HashSet::new();
    let mut has_any_septimo = false;
    for emps in companies_map.values() {
        for emp in emps {
            present_periodicities_set.insert(emp.periodicidad_display());
            if emp.get_septimo_dia_importe() > 0.0 {
                has_any_septimo = true;
            }
        }
    }

    let mut periodicities_present = Vec::new();
    for p in &standard_periodicities {
        if present_periodicities_set.contains(*p) {
            periodicities_present.push(p.to_string());
            present_periodicities_set.remove(*p);
        }
    }
    let mut rest_p: Vec<String> = present_periodicities_set.into_iter().collect();
    rest_p.sort();
    periodicities_present.extend(rest_p);

    // Build headers for the company breakdown table
    let mut comp_headers = vec![
        "RAZÓN SOCIAL EMISOR".to_string(),
        "RFC EMISOR".to_string(),
        "NÓMINAS VÁLIDAS".to_string(),
        if has_any_septimo {
            "TOTAL SUELDOS (SUELDO + 7MO DÍA) ($)".to_string()
        } else {
            "TOTAL SUELDOS ($)".to_string()
        },
    ];

    if has_any_septimo {
        comp_headers.push("TOTAL 7MO DÍA ($)".to_string());
    }

    for p in &periodicities_present {
        let p_upper = p.to_uppercase();
        let p_has_septimo = companies_map.values().any(|emps| {
            emps.iter()
                .any(|e| e.periodicidad_display() == *p && e.get_septimo_dia_importe() > 0.0)
        });

        comp_headers.push(format!("EMPLEADOS {}", p_upper));
        if p_has_septimo {
            comp_headers.push(format!("TOTAL {} (SUELDO + 7MO DÍA) ($)", p_upper));
            comp_headers.push(format!("7MO DÍA {} ($)", p_upper));
        } else {
            comp_headers.push(format!("TOTAL {} ($)", p_upper));
        }
    }

    // Title banner
    let banner_width = comp_headers.len().max(5) as u16;
    worksheet.merge_range(
        0,
        0,
        0,
        banner_width - 1,
        "DOCUUPLOAD - RESUMEN EJECUTIVO Y ESTADÍSTICAS DE NÓMINAS",
        &title_format,
    )?;

    // Company breakdown table title
    worksheet.merge_range(
        next_table_start_row,
        0,
        next_table_start_row,
        (comp_headers.len() - 1) as u16,
        "DESGLOSE POR EMPRESA / PATRÓN EMISOR",
        &table_head_format,
    )?;

    let header_row = next_table_start_row + 1;
    for (col, head) in comp_headers.iter().enumerate() {
        worksheet.write_string_with_format(header_row, col as u16, head, &table_head_format)?;
    }

    let mut current_row = header_row + 1;
    let mut company_keys: Vec<(String, String)> = companies_map.keys().cloned().collect();
    company_keys.sort();

    let mut total_nominas: usize = 0;
    let mut grand_total_sueldos: f64 = 0.0;
    let mut grand_total_septimo: f64 = 0.0;
    let mut periodicities_total_emps: HashMap<String, usize> = HashMap::new();
    let mut periodicities_total_sueldo: HashMap<String, f64> = HashMap::new();
    let mut periodicities_total_septimo: HashMap<String, f64> = HashMap::new();

    for (rfc, nombre) in &company_keys {
        let emps = &companies_map[&(rfc.clone(), nombre.clone())];
        let count = emps.len();
        let total_sueldos_co: f64 = emps.iter().map(|e| e.get_sueldo_total_con_septimo()).sum();
        let total_septimo_co: f64 = emps.iter().map(|e| e.get_septimo_dia_importe()).sum();

        total_nominas += count;
        grand_total_sueldos += total_sueldos_co;
        grand_total_septimo += total_septimo_co;

        let display_nombre = if nombre.is_empty() { "SIN_RAZON_SOCIAL" } else { nombre.as_str() };
        worksheet.write_string_with_format(current_row, 0, display_nombre, &cell_text_format)?;
        worksheet.write_string_with_format(current_row, 1, rfc, &cell_text_format)?;
        worksheet.write_number_with_format(current_row, 2, count as f64, &cell_num_format)?;
        worksheet.write_number_with_format(current_row, 3, total_sueldos_co, &cell_curr_format)?;

        let mut col_idx: u16 = 4;
        if has_any_septimo {
            worksheet.write_number_with_format(current_row, col_idx, total_septimo_co, &cell_curr_format)?;
            col_idx += 1;
        }

        for p in &periodicities_present {
            let emps_p: Vec<&&Employee> = emps.iter().filter(|e| e.periodicidad_display() == *p).collect();
            let unique_p: HashSet<&str> = emps_p.iter().map(|e| e.unique_key()).collect();
            let p_emp_count = unique_p.len();
            let p_total_sueldo_septimo: f64 = emps_p.iter().map(|e| e.get_sueldo_total_con_septimo()).sum();
            let p_total_septimo: f64 = emps_p.iter().map(|e| e.get_septimo_dia_importe()).sum();

            *periodicities_total_emps.entry(p.clone()).or_insert(0) += p_emp_count;
            *periodicities_total_sueldo.entry(p.clone()).or_insert(0.0) += p_total_sueldo_septimo;
            *periodicities_total_septimo.entry(p.clone()).or_insert(0.0) += p_total_septimo;

            worksheet.write_number_with_format(current_row, col_idx, p_emp_count as f64, &cell_num_format)?;
            col_idx += 1;
            worksheet.write_number_with_format(current_row, col_idx, p_total_sueldo_septimo, &cell_curr_format)?;
            col_idx += 1;

            let p_has_septimo = companies_map.values().any(|all_emps| {
                all_emps.iter().any(|e| e.periodicidad_display() == *p && e.get_septimo_dia_importe() > 0.0)
            });
            if p_has_septimo {
                worksheet.write_number_with_format(current_row, col_idx, p_total_septimo, &cell_curr_format)?;
                col_idx += 1;
            }
        }

        current_row += 1;
    }

    if company_keys.len() > 1 {
        let total_label_format = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_font_color(Color::White)
            .set_background_color(Color::RGB(0x1E293B))
            .set_align(rust_xlsxwriter::FormatAlign::Right)
            .set_border(FormatBorder::Thin);

        let total_num_fmt = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_font_color(Color::White)
            .set_background_color(Color::RGB(0x1E293B))
            .set_align(rust_xlsxwriter::FormatAlign::Center)
            .set_border(FormatBorder::Thin)
            .set_num_format("#,##0");

        let total_curr_fmt = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_font_color(Color::White)
            .set_background_color(Color::RGB(0x1E293B))
            .set_align(rust_xlsxwriter::FormatAlign::Right)
            .set_border(FormatBorder::Thin)
            .set_num_format("$#,##0.00");

        worksheet.merge_range(current_row, 0, current_row, 1, "TOTAL GENERAL", &total_label_format)?;
        worksheet.write_number_with_format(current_row, 2, total_nominas as f64, &total_num_fmt)?;
        worksheet.write_number_with_format(current_row, 3, grand_total_sueldos, &total_curr_fmt)?;

        let mut col_idx: u16 = 4;
        if has_any_septimo {
            worksheet.write_number_with_format(current_row, col_idx, grand_total_septimo, &total_curr_fmt)?;
            col_idx += 1;
        }

        for p in &periodicities_present {
            let p_emps = periodicities_total_emps.get(p).copied().unwrap_or(0);
            let p_sueldo = periodicities_total_sueldo.get(p).copied().unwrap_or(0.0);
            let p_septimo = periodicities_total_septimo.get(p).copied().unwrap_or(0.0);

            worksheet.write_number_with_format(current_row, col_idx, p_emps as f64, &total_num_fmt)?;
            col_idx += 1;
            worksheet.write_number_with_format(current_row, col_idx, p_sueldo, &total_curr_fmt)?;
            col_idx += 1;

            let p_has_septimo = companies_map.values().any(|all_emps| {
                all_emps.iter().any(|e| e.periodicidad_display() == *p && e.get_septimo_dia_importe() > 0.0)
            });
            if p_has_septimo {
                worksheet.write_number_with_format(current_row, col_idx, p_septimo, &total_curr_fmt)?;
                col_idx += 1;
            }
        }
    }

    worksheet.autofit();
    Ok(())
}

fn write_no_procesadas_sheet(
    worksheet: &mut Worksheet,
    invalid_items: &[UnprocessedNomina],
) -> Result<(), XlsxError> {
    worksheet.set_name("No_Procesadas_Omitidas")?;

    let header_format = Format::new()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x475569)) // Slate Dark
        .set_align(rust_xlsxwriter::FormatAlign::Center);

    let cell_format = Format::new()
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xE2E8F0));

    let orange_cat_format = Format::new()
        .set_bold()
        .set_font_color(Color::RGB(0xC2410C))
        .set_background_color(Color::RGB(0xFFEDD5))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin);

    let red_cat_format = Format::new()
        .set_bold()
        .set_font_color(Color::RGB(0xB91C1C))
        .set_background_color(Color::RGB(0xFEE2E2))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin);

    let currency_format = Format::new()
        .set_num_format("$#,##0.00")
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xE2E8F0));

    let headers = [
        "#",
        "NOMBRE_ARCHIVO",
        "UUID_FOLIO_FISCAL",
        "RFC_EMISOR",
        "RFC_RECEPTOR",
        "TOTAL_CFDI",
        "CATEGORÍA / ESTATUS",
        "MOTIVO DETALLADO DEL DESCARTE",
    ];

    for (col, head) in headers.iter().enumerate() {
        worksheet.write_string_with_format(0, col as u16, *head, &header_format)?;
    }

    if invalid_items.is_empty() {
        let ok_format = Format::new()
            .set_bold()
            .set_font_color(Color::RGB(0x15803D))
            .set_background_color(Color::RGB(0xDCFCE7));
        worksheet.write_string_with_format(
            1,
            1,
            "¡Excelente! No se encontraron nóminas rechazadas, duplicadas ni canceladas. Todas fueron válidas.",
            &ok_format,
        )?;
    } else {
        for (idx, item) in invalid_items.iter().enumerate() {
            let row = (idx + 1) as u32;
            worksheet.write_number(row, 0, (idx + 1) as f64)?;
            worksheet.write_string_with_format(row, 1, &item.filename, &cell_format)?;
            worksheet.write_string_with_format(row, 2, &item.uuid, &cell_format)?;
            worksheet.write_string_with_format(row, 3, &item.rfc_emisor, &cell_format)?;
            worksheet.write_string_with_format(row, 4, &item.rfc_receptor, &cell_format)?;
            worksheet.write_number_with_format(row, 5, item.total, &currency_format)?;

            let cat_fmt = if item.category.contains("DUPLICADA") {
                &orange_cat_format
            } else {
                &red_cat_format
            };

            worksheet.write_string_with_format(row, 6, &item.category, cat_fmt)?;
            worksheet.write_string_with_format(row, 7, &item.reason, &cell_format)?;
        }
    }

    worksheet.autofit();
    Ok(())
}

pub fn generate_excel(
    employees: &[Employee],
    invalid_items: &[UnprocessedNomina],
    stats: &NominasStats,
    out_path: &Path,
) -> Result<(), XlsxError> {
    let mut workbook = Workbook::new();

    // Group valid employees by (RFC, Company Name)
    let mut companies_map: HashMap<(String, String), Vec<&Employee>> = HashMap::new();
    for emp in employees {
        let rfc = emp.rfc_emisor.clone().unwrap_or_else(|| "SIN_RFC".to_string());
        let nombre = emp.nombre_emisor.clone().unwrap_or_default();
        companies_map.entry((rfc, nombre)).or_default().push(emp);
    }

    // 1. Sheet 1: Estadísticas y KPIs
    let ws_stats = workbook.add_worksheet();
    write_estadisticas_sheet(ws_stats, stats, &companies_map)?;

    // 2. Sheet 2: Nóminas No Procesadas, Canceladas y Duplicadas
    let ws_unproc = workbook.add_worksheet();
    write_no_procesadas_sheet(ws_unproc, invalid_items)?;

    // 3. Sheet 3+: Concentrado(s) Válidos y Filtrados por Empresa
    let header_format = Format::new()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1D4ED8)) // Elegant Royal Blue
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin);

    let currency_format = Format::new()
        .set_num_format("$#,##0.00")
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xE2E8F0));

    let integer_format = Format::new()
        .set_num_format("#,##0")
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xE2E8F0));

    let cell_text_format = Format::new()
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xE2E8F0));

    let cell_center_format = Format::new()
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xE2E8F0));

    let month_section_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1E3A8A))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin);

    let subtotal_label_format = Format::new()
        .set_bold()
        .set_font_size(10)
        .set_background_color(Color::RGB(0xDBEAFE))
        .set_align(rust_xlsxwriter::FormatAlign::Right)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x93C5FD));

    let subtotal_curr_format = Format::new()
        .set_bold()
        .set_font_size(10)
        .set_background_color(Color::RGB(0xDBEAFE))
        .set_align(rust_xlsxwriter::FormatAlign::Right)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x93C5FD))
        .set_num_format("$#,##0.00");

    let subtotal_int_format = Format::new()
        .set_bold()
        .set_font_size(10)
        .set_background_color(Color::RGB(0xDBEAFE))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x93C5FD))
        .set_num_format("#,##0");

    let subtotal_center_format = Format::new()
        .set_bold()
        .set_font_size(10)
        .set_background_color(Color::RGB(0xDBEAFE))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x93C5FD));

    let grand_total_label_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1E293B))
        .set_align(rust_xlsxwriter::FormatAlign::Right)
        .set_border_bottom(FormatBorder::Double)
        .set_border_top(FormatBorder::Thin);

    let grand_total_curr_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1E293B))
        .set_align(rust_xlsxwriter::FormatAlign::Right)
        .set_border_bottom(FormatBorder::Double)
        .set_border_top(FormatBorder::Thin)
        .set_num_format("$#,##0.00");

    let grand_total_int_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1E293B))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border_bottom(FormatBorder::Double)
        .set_border_top(FormatBorder::Thin)
        .set_num_format("#,##0");

    let grand_total_center_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1E293B))
        .set_align(rust_xlsxwriter::FormatAlign::Center)
        .set_border_bottom(FormatBorder::Double)
        .set_border_top(FormatBorder::Thin);

    if companies_map.is_empty() {
        let ws_empty = workbook.add_worksheet();
        ws_empty.set_name("Concentrado_Sin_Datos")?;
        let warn_format = Format::new()
            .set_bold()
            .set_font_color(Color::RGB(0xB91C1C));
        ws_empty.write_string_with_format(
            0,
            0,
            "No se encontraron nóminas válidas y vigentes para concentrar. Revisa la pestaña 'No_Procesadas_Omitidas'.",
            &warn_format,
        )?;
    } else {
        let mut company_keys: Vec<(String, String)> = companies_map.keys().cloned().collect();
        company_keys.sort();

        let mut existing_names = HashSet::new();
        existing_names.insert("estadisticas".to_string());
        existing_names.insert("no_procesadas_omitidas".to_string());

        for (rfc, nombre) in &company_keys {
            let emps = &companies_map[&(rfc.clone(), nombre.clone())];
            let sheet_name = make_nomina_sheet_name(
                if nombre.is_empty() { None } else { Some(nombre.as_str()) },
                if rfc.is_empty() { None } else { Some(rfc.as_str()) },
                &mut existing_names,
            );

            let worksheet = workbook.add_worksheet();
            worksheet.set_name(&sheet_name)?;

            // Gather unique Conceptos (excluding SUELDO) for this company
            let mut unique_conceptos = HashSet::new();
            for emp in emps {
                for p in &emp.nomina.percepciones {
                    if !p.concepto.contains("SUELDO") {
                        unique_conceptos.insert(p.concepto.clone());
                    }
                }
            }

            let mut columns_conceptos: Vec<String> = unique_conceptos.into_iter().collect();
            columns_conceptos.sort();

            // Headers
            let mut headers = vec![
                "AÑO",
                "MES",
                "NOMBRE_ARCHIVO",
                "FECHA_INICIAL",
                "FECHA_FINAL",
                "FECHA_PAGO",
                "PERIODICIDAD",
                "NOMBRE",
                "NSS",
                "SD_CALCULADO",
                "SDI_NOMINA",
                "SDI_CALCULADO",
                "DIAS_TRABAJADOS",
                "SUELDO",
            ];
            let header_strs: Vec<&str> = columns_conceptos.iter().map(AsRef::as_ref).collect();
            headers.extend(header_strs);
            let total_cols = headers.len() as u16;

            for (col, header) in headers.iter().enumerate() {
                worksheet.write_string_with_format(0, col as u16, *header, &header_format)?;
            }

            // Group by (year, month)
            let mut month_groups: HashMap<(i32, u32), Vec<&Employee>> = HashMap::new();
            for emp in emps {
                let ym = emp.get_year_month();
                month_groups.entry(ym).or_default().push(emp);
            }

            let mut sorted_ym: Vec<(i32, u32)> = month_groups.keys().cloned().collect();
            sorted_ym.sort();

            let mut current_row: u32 = 1;

            let mut grand_total_dias: f64 = 0.0;
            let mut grand_total_sueldo: f64 = 0.0;
            let mut grand_total_conceptos: HashMap<String, f64> = HashMap::new();

            for (year, month) in sorted_ym {
                let mut month_emps = month_groups[&(year, month)].clone();
                month_emps.sort_by(|a, b| {
                    let date_a = a.nomina.fechafinalpago.unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(1900, 1, 1).unwrap());
                    let date_b = b.nomina.fechafinalpago.unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(1900, 1, 1).unwrap());
                    date_a.cmp(&date_b).then(a.nombre.cmp(&b.nombre))
                });

                let month_name = crate::nominas::utils::month_name_spanish(month);
                let banner_text = format!("PERÍODO DE NÓMINA: {} {}", month_name, year);

                worksheet.merge_range(
                    current_row,
                    0,
                    current_row,
                    total_cols - 1,
                    &banner_text,
                    &month_section_format,
                )?;
                current_row += 1;

                let mut subtotal_dias: f64 = 0.0;
                let mut subtotal_sueldo: f64 = 0.0;
                let mut subtotal_conceptos: HashMap<String, f64> = HashMap::new();

                for emp in month_emps {
                    let fecha_ini = emp
                        .nomina
                        .fechainicialpago
                        .map(|d| d.format("%Y-%m-%d").to_string())
                        .unwrap_or_default();
                    let fecha_fin = emp
                        .nomina
                        .fechafinalpago
                        .map(|d| d.format("%Y-%m-%d").to_string())
                        .unwrap_or_default();
                    let fecha_pago = emp
                        .nomina
                        .fechapago
                        .map(|d| d.format("%Y-%m-%d").to_string())
                        .unwrap_or_default();
                    let sueldo_importe = emp.get_sueldo_importe();
                    let dias_pagados = emp.nomina.numdiaspagados.unwrap_or(0.0);

                    worksheet.write_number_with_format(current_row, 0, year as f64, &cell_center_format)?;
                    worksheet.write_string_with_format(current_row, 1, month_name, &cell_center_format)?;
                    worksheet.write_string_with_format(current_row, 2, &emp.nombrearchivo, &cell_text_format)?;
                    worksheet.write_string_with_format(current_row, 3, &fecha_ini, &cell_center_format)?;
                    worksheet.write_string_with_format(current_row, 4, &fecha_fin, &cell_center_format)?;
                    worksheet.write_string_with_format(current_row, 5, &fecha_pago, &cell_center_format)?;
                    worksheet.write_string_with_format(current_row, 6, &emp.periodicidad_display(), &cell_center_format)?;
                    worksheet.write_string_with_format(current_row, 7, emp.nombre.as_deref().unwrap_or(""), &cell_text_format)?;
                    worksheet.write_string_with_format(current_row, 8, emp.numseguridadsocial.as_deref().unwrap_or(""), &cell_center_format)?;
                    worksheet.write_number_with_format(
                        current_row,
                        9,
                        emp.salariodiariocalculado.unwrap_or(0.0),
                        &currency_format,
                    )?;
                    worksheet.write_number_with_format(
                        current_row,
                        10,
                        emp.salariobasecotapor.unwrap_or(0.0),
                        &currency_format,
                    )?;
                    worksheet.write_number_with_format(
                        current_row,
                        11,
                        emp.salariodiariointegradocalculado.unwrap_or(0.0),
                        &currency_format,
                    )?;
                    worksheet.write_number_with_format(
                        current_row,
                        12,
                        dias_pagados,
                        &integer_format,
                    )?;
                    worksheet.write_number_with_format(current_row, 13, sueldo_importe, &currency_format)?;

                    subtotal_dias += dias_pagados;
                    subtotal_sueldo += sueldo_importe;
                    grand_total_dias += dias_pagados;
                    grand_total_sueldo += sueldo_importe;

                    let mut row_conceptos: HashMap<&str, f64> = HashMap::new();
                    for p in &emp.nomina.percepciones {
                        if !p.concepto.contains("SUELDO") {
                            row_conceptos.insert(&p.concepto, p.importe);
                        }
                    }

                    for (i, col_name) in columns_conceptos.iter().enumerate() {
                        let col_idx = (14 + i) as u16;
                        let val = row_conceptos.get(col_name.as_str()).copied().unwrap_or(0.0);
                        worksheet.write_number_with_format(current_row, col_idx, val, &currency_format)?;
                        *subtotal_conceptos.entry(col_name.clone()).or_insert(0.0) += val;
                        *grand_total_conceptos.entry(col_name.clone()).or_insert(0.0) += val;
                    }

                    current_row += 1;
                }

                // Month Subtotal Row
                let subtotal_label = format!("TOTAL {} {}", month_name, year);
                worksheet.merge_range(current_row, 0, current_row, 8, &subtotal_label, &subtotal_label_format)?;
                worksheet.write_string_with_format(current_row, 9, "-", &subtotal_center_format)?;
                worksheet.write_string_with_format(current_row, 10, "-", &subtotal_center_format)?;
                worksheet.write_string_with_format(current_row, 11, "-", &subtotal_center_format)?;
                worksheet.write_number_with_format(current_row, 12, subtotal_dias, &subtotal_int_format)?;
                worksheet.write_number_with_format(current_row, 13, subtotal_sueldo, &subtotal_curr_format)?;

                for (i, col_name) in columns_conceptos.iter().enumerate() {
                    let col_idx = (14 + i) as u16;
                    let val = subtotal_conceptos.get(col_name).copied().unwrap_or(0.0);
                    worksheet.write_number_with_format(current_row, col_idx, val, &subtotal_curr_format)?;
                }
                current_row += 1;

                // Blank separator row
                current_row += 1;
            }

            // Grand Total Row
            worksheet.merge_range(current_row, 0, current_row, 8, "TOTAL GENERAL ACUMULADO", &grand_total_label_format)?;
            worksheet.write_string_with_format(current_row, 9, "-", &grand_total_center_format)?;
            worksheet.write_string_with_format(current_row, 10, "-", &grand_total_center_format)?;
            worksheet.write_string_with_format(current_row, 11, "-", &grand_total_center_format)?;
            worksheet.write_number_with_format(current_row, 12, grand_total_dias, &grand_total_int_format)?;
            worksheet.write_number_with_format(current_row, 13, grand_total_sueldo, &grand_total_curr_format)?;

            for (i, col_name) in columns_conceptos.iter().enumerate() {
                let col_idx = (14 + i) as u16;
                let val = grand_total_conceptos.get(col_name).copied().unwrap_or(0.0);
                worksheet.write_number_with_format(current_row, col_idx, val, &grand_total_curr_format)?;
            }

            worksheet.autofit();
        }
    }

    workbook.save(out_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_nomina_sheet_name() {
        let mut existing = HashSet::new();
        let name1 = make_nomina_sheet_name(
            Some("GONDESA DE MEXICO SA DE CV"),
            Some("GON920101XYZ"),
            &mut existing,
        );
        assert_eq!(name1, "gondesa_GON920101XYZ");

        let name2 = make_nomina_sheet_name(
            Some("GONDESA DE MEXICO SA DE CV"),
            Some("GON920101XYZ"),
            &mut existing,
        );
        assert_eq!(name2, "gondesa_GON920101XYZ_2");

        let name3 = make_nomina_sheet_name(
            Some("MACHINEN INDUSTRIES INC"),
            Some("MAC010101ABC"),
            &mut existing,
        );
        assert_eq!(name3, "machinen_MAC010101ABC");
    }

    #[test]
    fn test_make_nomina_sheet_name_max_length() {
        let mut existing = HashSet::new();
        let long_name = "EMPRESA_CON_UN_NOMBRE_EXTREMADAMENTE_LARGO_QUE_SUPERA_TREINTA_Y_UN_CARACTERES";
        let sheet_name = make_nomina_sheet_name(Some(long_name), Some("RFC123456789"), &mut existing);
        assert!(sheet_name.chars().count() <= 31);
    }

    #[test]
    fn test_generate_excel_multi_company() {
        let mut emp1 = Employee {
            nombrearchivo: "emp1.xml".to_string(),
            nombre: Some("JUAN PEREZ".to_string()),
            rfc_emisor: Some("GON920101XYZ".to_string()),
            nombre_emisor: Some("GONDESA SA DE CV".to_string()),
            ..Default::default()
        };
        emp1.nomina.numdiaspagados = Some(15.0);

        let mut emp2 = Employee {
            nombrearchivo: "emp2.xml".to_string(),
            nombre: Some("MARIA LOPEZ".to_string()),
            rfc_emisor: Some("MAC010101ABC".to_string()),
            nombre_emisor: Some("MACHINEN CORP".to_string()),
            ..Default::default()
        };
        emp2.nomina.numdiaspagados = Some(15.0);

        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join("test_nominas_multi.xlsx");

        let stats = NominasStats {
            total_files: 2,
            valid_count: 2,
            ..Default::default()
        };

        let res = generate_excel(&[emp1, emp2], &[], &stats, &test_path);
        assert!(res.is_ok());
        assert!(test_path.exists());
        let _ = std::fs::remove_file(test_path);
    }

    #[test]
    fn test_generate_excel_with_monthly_grouping_and_septimo_dia() {
        use crate::nominas::models::Percepcion;
        use chrono::NaiveDate;

        let mut emp_semanal_may = Employee {
            nombrearchivo: "emp_sem_may.xml".to_string(),
            nombre: Some("CARLOS SEMANAL".to_string()),
            rfc_emisor: Some("AAA010101AAA".to_string()),
            nombre_emisor: Some("EMPRESA PRUEBA SA DE CV".to_string()),
            rfc_receptor: Some("CARL800101XYZ".to_string()),
            ..Default::default()
        };
        emp_semanal_may.nomina.periodicidad_pago = Some("02".to_string());
        emp_semanal_may.nomina.numdiaspagados = Some(7.0);
        emp_semanal_may.nomina.fechafinalpago = Some(NaiveDate::from_ymd_opt(2024, 5, 7).unwrap());
        emp_semanal_may.nomina.percepciones = vec![
            Percepcion { concepto: "SUELDO SEMANAL".to_string(), importe: 3000.0 },
            Percepcion { concepto: "SEPTIMO DIA".to_string(), importe: 500.0 },
        ];
        emp_semanal_may.calculate_salaries();

        let mut emp_quincenal_may = Employee {
            nombrearchivo: "emp_quin_may.xml".to_string(),
            nombre: Some("ANA QUINCENAL".to_string()),
            rfc_emisor: Some("AAA010101AAA".to_string()),
            nombre_emisor: Some("EMPRESA PRUEBA SA DE CV".to_string()),
            rfc_receptor: Some("ANAA900101XYZ".to_string()),
            ..Default::default()
        };
        emp_quincenal_may.nomina.periodicidad_pago = Some("04".to_string());
        emp_quincenal_may.nomina.numdiaspagados = Some(15.0);
        emp_quincenal_may.nomina.fechafinalpago = Some(NaiveDate::from_ymd_opt(2024, 5, 15).unwrap());
        emp_quincenal_may.nomina.percepciones = vec![
            Percepcion { concepto: "SUELDO QUINCENAL".to_string(), importe: 8000.0 },
        ];
        emp_quincenal_may.calculate_salaries();

        let mut emp_semanal_jun = Employee {
            nombrearchivo: "emp_sem_jun.xml".to_string(),
            nombre: Some("CARLOS SEMANAL".to_string()),
            rfc_emisor: Some("AAA010101AAA".to_string()),
            nombre_emisor: Some("EMPRESA PRUEBA SA DE CV".to_string()),
            rfc_receptor: Some("CARL800101XYZ".to_string()),
            ..Default::default()
        };
        emp_semanal_jun.nomina.periodicidad_pago = Some("02".to_string());
        emp_semanal_jun.nomina.numdiaspagados = Some(7.0);
        emp_semanal_jun.nomina.fechafinalpago = Some(NaiveDate::from_ymd_opt(2024, 6, 7).unwrap());
        emp_semanal_jun.nomina.percepciones = vec![
            Percepcion { concepto: "SUELDO SEMANAL".to_string(), importe: 3000.0 },
            Percepcion { concepto: "SEPTIMO DIA".to_string(), importe: 500.0 },
        ];
        emp_semanal_jun.calculate_salaries();

        let employees = vec![emp_semanal_may, emp_quincenal_may, emp_semanal_jun];
        let total_sueldos: f64 = employees.iter().map(|e| e.get_sueldo_total_con_septimo()).sum();
        let total_septimo: f64 = employees.iter().map(|e| e.get_septimo_dia_importe()).sum();

        // 3500 + 8000 + 3500 = 15000 total sueldos (including septimo dia)
        assert_eq!(total_sueldos, 15000.0);
        assert_eq!(total_septimo, 1000.0);

        let stats = NominasStats {
            total_files: 3,
            valid_count: 3,
            total_sueldos_validos: total_sueldos,
            total_septimo_dia_validos: total_septimo,
            ..Default::default()
        };

        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join("test_nominas_monthly_grouping.xlsx");

        let res = generate_excel(&employees, &[], &stats, &test_path);
        assert!(res.is_ok());
        assert!(test_path.exists());
        let _ = std::fs::remove_file(test_path);
    }
}
