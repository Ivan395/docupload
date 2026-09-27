use crate::cedulas::business::Business;
use crate::cedulas::employees::EmployeeFlatRow;
use rust_xlsxwriter::{Color, Format, FormatBorder, Workbook, Worksheet, XlsxError};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

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

pub fn make_cedula_sheet_name(
    razon_social: Option<&str>,
    registro_patronal: Option<&str>,
    existing_names: &mut HashSet<String>,
) -> String {
    let comp_short = match razon_social {
        Some(rz) if !rz.trim().is_empty() => {
            let first_word = rz
                .trim()
                .split(|c: char| c.is_whitespace() || c == ',' || c == '.')
                .find(|s| !s.is_empty())
                .unwrap_or("empresa");
            first_word.to_lowercase()
        }
        _ => "empresa".to_string(),
    };

    let reg_short = match registro_patronal {
        Some(rp) if !rp.trim().is_empty() => rp.trim().replace([' ', '/', '\\'], "_"),
        _ => "sin_registro".to_string(),
    };

    let mut base_name = format!("{}_{}", comp_short, reg_short);
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

pub struct Excel {
    filename: String,
    workbook: Workbook,
}

impl Excel {
    pub fn new(filename: &str) -> Self {
        Self {
            filename: filename.to_string(),
            workbook: Workbook::new(),
        }
    }

    pub fn create_excel(&mut self, business_data: &Business, lang: &str) -> Result<(), XlsxError> {
        let stats = crate::cedulas::processor::CedulasStats {
            total_files: 1,
            valid_files: 1,
            valid_workers: business_data.employees.len(),
            ..Default::default()
        };
        self.create_excel_full(&[("Cedula".to_string(), business_data.clone())], &[], &stats, lang)
    }

    pub fn create_excel_multi(
        &mut self,
        sheets: &[(String, Business)],
        lang: &str,
    ) -> Result<(), XlsxError> {
        let stats = crate::cedulas::processor::CedulasStats {
            total_files: sheets.len(),
            valid_files: sheets.len(),
            valid_workers: sheets.iter().map(|(_, b)| b.employees.len()).sum(),
            ..Default::default()
        };
        self.create_excel_full(sheets, &[], &stats, lang)
    }

    pub fn create_excel_full(
        &mut self,
        sheets: &[(String, Business)],
        invalid_items: &[crate::cedulas::processor::UnprocessedCedula],
        stats: &crate::cedulas::processor::CedulasStats,
        lang: &str,
    ) -> Result<(), XlsxError> {
        // 1. Sheet 1: Estadisticas
        let ws_stats = self.workbook.add_worksheet();
        Self::write_estadisticas_sheet(ws_stats, stats, sheets)?;

        // 2. Sheet 2: No_Procesadas_Omitidas
        let ws_unproc = self.workbook.add_worksheet();
        Self::write_no_procesadas_sheet(ws_unproc, invalid_items)?;

        // 3. Sheet 3+: Concentrados Válidos
        if sheets.is_empty() {
            let worksheet = self.workbook.add_worksheet();
            worksheet.set_name("Concentrado_Sin_Datos")?;
            let warn_format = Format::new()
                .set_bold()
                .set_font_color(Color::RGB(0xB91C1C));
            worksheet.write_string_with_format(
                0,
                0,
                "No se encontraron cédulas SUA válidas para concentrar. Revisa la pestaña 'No_Procesadas_Omitidas'.",
                &warn_format,
            )?;
        } else {
            for (sheet_name, business_data) in sheets {
                let worksheet = self.workbook.add_worksheet();
                worksheet.set_name(sheet_name)?;
                Self::write_business_sheet(worksheet, business_data, lang)?;
            }
        }

        self.workbook.save(&self.filename)?;
        Ok(())
    }

    fn write_estadisticas_sheet(
        worksheet: &mut Worksheet,
        stats: &crate::cedulas::processor::CedulasStats,
        sheets: &[(String, Business)],
    ) -> Result<(), XlsxError> {
        worksheet.set_name("Estadisticas")?;

        let title_format = Format::new()
            .set_bold()
            .set_font_size(14)
            .set_font_color(Color::White)
            .set_background_color(Color::RGB(0x0F766E))
            .set_align(rust_xlsxwriter::FormatAlign::Center);

        let kpi_label_format = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_background_color(Color::RGB(0xF0FDFA))
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0xCCFBF1));

        let kpi_val_format = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_align(rust_xlsxwriter::FormatAlign::Center)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0xCCFBF1))
            .set_num_format("#,##0");

        let kpi_green_format = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_font_color(Color::RGB(0x0F766E))
            .set_background_color(Color::RGB(0xCCFBF1))
            .set_align(rust_xlsxwriter::FormatAlign::Center)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0x99F6E4))
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
            .set_font_color(Color::RGB(0x0F766E))
            .set_background_color(Color::RGB(0xCCFBF1))
            .set_align(rust_xlsxwriter::FormatAlign::Right)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0x99F6E4))
            .set_num_format("$#,##0.00");

        let table_head_format = Format::new()
            .set_bold()
            .set_font_color(Color::White)
            .set_background_color(Color::RGB(0x0D9488))
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

        // Title banner
        worksheet.merge_range(0, 0, 0, 4, "DOCUUPLOAD - RESUMEN EJECUTIVO Y ESTADÍSTICAS CÉDULAS SUA IMSS", &title_format)?;

        let kpi_data = [
            ("Total de Archivos PDF Analizados", stats.total_files as f64, &kpi_val_format),
            ("Archivos Válidos con Formato Cédula SUA", stats.valid_files as f64, &kpi_green_format),
            ("Trabajadores Válidos Concentrados", stats.valid_workers as f64, &kpi_green_format),
            ("Registros / Trabajadores Duplicados (Omitidos)", stats.duplicate_workers as f64, &kpi_orange_format),
            ("Archivos Inválidos / Sin Capa de Texto OCR", stats.invalid_files as f64, &kpi_red_format),
        ];

        worksheet.write_string_with_format(2, 0, "INDICADOR MÉTRICO SUA", &table_head_format)?;
        worksheet.write_string_with_format(2, 1, "CANTIDAD", &table_head_format)?;

        for (i, (label, val, fmt)) in kpi_data.iter().enumerate() {
            let row = (3 + i) as u32;
            worksheet.write_string_with_format(row, 0, *label, &kpi_label_format)?;
            worksheet.write_number_with_format(row, 1, *val, fmt)?;
        }

        worksheet.write_string_with_format(8, 0, "Importe Total de Liquidaciones / Cuotas IMSS", &kpi_label_format)?;
        worksheet.write_number_with_format(8, 1, stats.total_cuotas, &currency_kpi_format)?;

        // Breakdown table per patronal registration
        worksheet.merge_range(11, 0, 11, 4, "DESGLOSE POR REGISTRO PATRONAL / EMPRESA", &table_head_format)?;

        let headers = ["REGISTRO PATRONAL", "RAZÓN SOCIAL", "DELEGACIÓN", "TRABAJADORES", "TOTAL LIQUIDACIONES ($)"];
        for (col, head) in headers.iter().enumerate() {
            worksheet.write_string_with_format(12, col as u16, *head, &table_head_format)?;
        }

        let mut current_row = 13;
        for (_sheet_name, biz) in sheets {
            let reg = biz.registro_patronal.as_deref().unwrap_or("SIN_REGISTRO");
            let razon = biz.razon_social.as_deref().unwrap_or("SIN_RAZON_SOCIAL");
            let del = biz.delegacion.as_deref().unwrap_or("-");
            let count = biz.employees.len();

            let total_biz: f64 = biz.employees.iter().flat_map(|e| &e.data_employee).filter_map(|d| d.subtotal).sum();

            worksheet.write_string_with_format(current_row, 0, reg, &cell_text_format)?;
            worksheet.write_string_with_format(current_row, 1, razon, &cell_text_format)?;
            worksheet.write_string_with_format(current_row, 2, del, &cell_text_format)?;
            worksheet.write_number_with_format(current_row, 3, count as f64, &cell_num_format)?;
            worksheet.write_number_with_format(current_row, 4, total_biz, &cell_curr_format)?;
            current_row += 1;
        }

        worksheet.autofit();
        Ok(())
    }

    fn write_no_procesadas_sheet(
        worksheet: &mut Worksheet,
        invalid_items: &[crate::cedulas::processor::UnprocessedCedula],
    ) -> Result<(), XlsxError> {
        worksheet.set_name("No_Procesadas_Omitidas")?;

        let header_format = Format::new()
            .set_bold()
            .set_font_color(Color::White)
            .set_background_color(Color::RGB(0x475569))
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

        let headers = [
            "#",
            "ARCHIVO",
            "REGISTRO_PATRONAL",
            "RAZON_SOCIAL",
            "NSS_IDENTIFICADOR",
            "PERIODO",
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
                "¡Excelente! No se encontraron cédulas ni trabajadores rechazados o duplicados. Todas fueron válidas.",
                &ok_format,
            )?;
        } else {
            for (idx, item) in invalid_items.iter().enumerate() {
                let row = (idx + 1) as u32;
                worksheet.write_number(row, 0, (idx + 1) as f64)?;
                worksheet.write_string_with_format(row, 1, &item.filename, &cell_format)?;
                worksheet.write_string_with_format(row, 2, &item.registro_patronal, &cell_format)?;
                worksheet.write_string_with_format(row, 3, &item.razon_social, &cell_format)?;
                worksheet.write_string_with_format(row, 4, &item.identifier, &cell_format)?;
                worksheet.write_string_with_format(row, 5, &item.periodo, &cell_format)?;

                let cat_fmt = if item.category.contains("DUPLICAD") {
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

    fn write_business_sheet(
        worksheet: &mut Worksheet,
        business_data: &Business,
        lang: &str,
    ) -> Result<(), XlsxError> {

        let sum_func = match lang.to_lowercase().as_str() {
            "spanish" | "es" | "suma" => "SUMA",
            _ => "SUM",
        };

        let header_format = Format::new()
            .set_bold()
            .set_font_color(Color::White)
            .set_background_color(Color::RGB(0x0F172A))
            .set_align(rust_xlsxwriter::FormatAlign::Center);

        let total_label_format = Format::new()
            .set_bold()
            .set_italic()
            .set_background_color(Color::RGB(0xEAEAEA))
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Double);

        let total_value_format = Format::new()
            .set_bold()
            .set_num_format("$#,##0.00")
            .set_background_color(Color::RGB(0xEAEAEA))
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Double);

        let total_days_format = Format::new()
            .set_bold()
            .set_num_format("#,##0")
            .set_background_color(Color::RGB(0xEAEAEA))
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Double);

        let grand_total_label_format = Format::new()
            .set_bold()
            .set_font_color(Color::White)
            .set_background_color(Color::RGB(0x0F172A))
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Double);

        let grand_total_value_format = Format::new()
            .set_bold()
            .set_num_format("$#,##0.00")
            .set_background_color(Color::RGB(0xD9D9D9))
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Double);

        let grand_total_days_format = Format::new()
            .set_bold()
            .set_num_format("#,##0")
            .set_background_color(Color::RGB(0xD9D9D9))
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Double);

        Self::insert_business_data(worksheet, business_data)?;
        Self::insert_column_names(worksheet, &header_format)?;

        let flat_rows: Vec<EmployeeFlatRow> = business_data
            .employees
            .par_iter()
            .flat_map(|emp| emp.to_list())
            .collect();

        let mut grouped_map: HashMap<(u32, u32, String, String), Vec<EmployeeFlatRow>> =
            HashMap::new();

        for flat_row in flat_rows {
            let year_str = flat_row
                .anio_periodo
                .clone()
                .unwrap_or_else(|| String::from("UNKNOWN"));
            let month_str = flat_row
                .mes_periodo
                .clone()
                .unwrap_or_else(|| String::from("UNKNOWN"));

            let year_num = year_str.parse::<u32>().unwrap_or(0);
            let month_num = crate::cedulas::business::month_to_num(&month_str);

            grouped_map
                .entry((year_num, month_num, year_str, month_str))
                .or_default()
                .push(flat_row);
        }

        let mut grouped_periods: Vec<((u32, u32, String, String), Vec<EmployeeFlatRow>)> =
            grouped_map.into_iter().collect();

        grouped_periods.par_sort_by(|a, b| {
            let year_cmp = a.0.0.cmp(&b.0.0);
            if year_cmp != std::cmp::Ordering::Equal {
                return year_cmp;
            }
            a.0.1.cmp(&b.0.1)
        });

        let mut row_idx: u32 = 20;
        let mut monthly_total_rows: Vec<u32> = Vec::new();

        for ((_year_num, _month_num, year, month), rows) in grouped_periods {
            let start_row = row_idx + 1;

            for flat_row in rows {
                worksheet.write_string(
                    row_idx,
                    0,
                    flat_row.no_seguro_social.as_deref().unwrap_or(""),
                )?;
                worksheet.write_string(row_idx, 1, flat_row.nombre.as_deref().unwrap_or(""))?;
                worksheet.write_string(row_idx, 2, flat_row.rfc.as_deref().unwrap_or(""))?;
                worksheet.write_string(row_idx, 3, flat_row.curp.as_deref().unwrap_or(""))?;
                worksheet.write_string(
                    row_idx,
                    4,
                    flat_row.clave_ubicacion.as_deref().unwrap_or(""),
                )?;
                worksheet.write_string(row_idx, 5, &flat_row.registro_obras)?;
                worksheet.write_string(
                    row_idx,
                    6,
                    flat_row.mes_periodo.as_deref().unwrap_or(""),
                )?;
                worksheet.write_string(
                    row_idx,
                    7,
                    flat_row.anio_periodo.as_deref().unwrap_or(""),
                )?;
                worksheet.write_string(row_idx, 8, flat_row.clave.as_deref().unwrap_or(""))?;
                worksheet.write_string(row_idx, 9, flat_row.fecha.as_deref().unwrap_or(""))?;

                Self::write_opt_cell(worksheet, row_idx, 10, flat_row.dias)?;
                Self::write_opt_cell(worksheet, row_idx, 11, flat_row.sdi)?;
                Self::write_opt_cell(worksheet, row_idx, 12, flat_row.lic)?;
                Self::write_opt_cell(worksheet, row_idx, 13, flat_row.inc)?;
                Self::write_opt_cell(worksheet, row_idx, 14, flat_row.aus)?;
                Self::write_opt_cell(worksheet, row_idx, 15, flat_row.cf)?;
                Self::write_opt_cell(worksheet, row_idx, 16, flat_row.exc_pat)?;
                Self::write_opt_cell(worksheet, row_idx, 17, flat_row.exc_obr)?;
                Self::write_opt_cell(worksheet, row_idx, 18, flat_row.pd_pat)?;
                Self::write_opt_cell(worksheet, row_idx, 19, flat_row.pd_obr)?;
                Self::write_opt_cell(worksheet, row_idx, 20, flat_row.gmp_pat)?;
                Self::write_opt_cell(worksheet, row_idx, 21, flat_row.gmp_obr)?;
                Self::write_opt_cell(worksheet, row_idx, 22, flat_row.rt)?;
                Self::write_opt_cell(worksheet, row_idx, 23, flat_row.iv_pat)?;
                Self::write_opt_cell(worksheet, row_idx, 24, flat_row.iv_obr)?;
                Self::write_opt_cell(worksheet, row_idx, 25, flat_row.gps)?;
                Self::write_opt_cell(worksheet, row_idx, 26, flat_row.patronal)?;
                Self::write_opt_cell(worksheet, row_idx, 27, flat_row.obrera)?;
                Self::write_opt_cell(worksheet, row_idx, 28, flat_row.subtotal)?;

                row_idx += 1;
            }

            let end_row = row_idx;
            monthly_total_rows.push(row_idx + 1);

            worksheet.write_string_with_format(
                row_idx,
                0,
                &format!("TOTAL {} {}", month.to_uppercase(), year),
                &total_label_format,
            )?;
            for empty_col in 1..=9 {
                worksheet.write_string_with_format(row_idx, empty_col, "", &total_label_format)?;
            }

            let start_cell_k = rust_xlsxwriter::utility::row_col_to_cell(start_row - 1, 10);
            let end_cell_k = rust_xlsxwriter::utility::row_col_to_cell(end_row - 1, 10);
            let formula_days = format!("={}({}:{})", sum_func, start_cell_k, end_cell_k);
            worksheet.write_formula_with_format(
                row_idx,
                10,
                formula_days.as_str(),
                &total_days_format,
            )?;

            let start_cell_l = rust_xlsxwriter::utility::row_col_to_cell(start_row - 1, 11);
            let end_cell_l = rust_xlsxwriter::utility::row_col_to_cell(end_row - 1, 11);
            let formula_sdi = format!("={}({}:{})", sum_func, start_cell_l, end_cell_l);
            worksheet.write_formula_with_format(
                row_idx,
                11,
                formula_sdi.as_str(),
                &total_value_format,
            )?;

            for col in 12..=28 {
                let start_cell =
                    rust_xlsxwriter::utility::row_col_to_cell(start_row - 1, col as u16);
                let end_cell = rust_xlsxwriter::utility::row_col_to_cell(end_row - 1, col as u16);
                let formula_loop = format!("={}({}:{})", sum_func, start_cell, end_cell);

                worksheet.write_formula_with_format(
                    row_idx,
                    col as u16,
                    formula_loop.as_str(),
                    &total_value_format,
                )?;
            }

            row_idx += 1;
        }

        // Grand totals row
        if !monthly_total_rows.is_empty() {
            worksheet.write_string_with_format(
                row_idx,
                0,
                "RESUMEN / GRAND TOTAL",
                &grand_total_label_format,
            )?;
            for empty_col in 1..=9 {
                worksheet.write_string_with_format(
                    row_idx,
                    empty_col,
                    "",
                    &grand_total_label_format,
                )?;
            }

            let make_grand_formula = |col_letter: &str, rows: &[u32]| -> String {
                let cell_references: Vec<String> = rows
                    .iter()
                    .map(|r| format!("{}{}", col_letter, r))
                    .collect();
                format!("={}({})", sum_func, cell_references.join(","))
            };

            let grand_days_formula = make_grand_formula("K", &monthly_total_rows);
            worksheet.write_formula_with_format(
                row_idx,
                10,
                grand_days_formula.as_str(),
                &grand_total_days_format,
            )?;

            let grand_sdi_formula = make_grand_formula("L", &monthly_total_rows);
            worksheet.write_formula_with_format(
                row_idx,
                11,
                grand_sdi_formula.as_str(),
                &grand_total_value_format,
            )?;

            for col in 12..=28 {
                let col_letter = if col >= 26 {
                    format!("A{}", (b'A' + ((col - 26) % 26)) as char)
                } else {
                    format!("{}", (b'A' + (col % 26)) as char)
                };

                let grand_loop_formula = make_grand_formula(&col_letter, &monthly_total_rows);
                worksheet.write_formula_with_format(
                    row_idx,
                    col as u16,
                    grand_loop_formula.as_str(),
                    &grand_total_value_format,
                )?;
            }
        }

        Self::adjust_width_cells(worksheet)?;
        Ok(())
    }

    fn insert_business_data(
        worksheet: &mut Worksheet,
        business_data: &Business,
    ) -> Result<(), XlsxError> {
        let business_fields = vec![
            ("registro_patronal", &business_data.registro_patronal),
            ("razon_social", &business_data.razon_social),
            ("delegacion", &business_data.delegacion),
            ("codigo_delegacion", &business_data.codigo_delegacion),
            ("actividad", &business_data.actividad),
            ("subdelegacion", &business_data.subdelegacion),
            ("codigo_subdelegacion", &business_data.codigo_subdelegacion),
            ("domicilio", &business_data.domicilio),
            ("codigo_postal", &business_data.codigo_postal),
            ("entidad", &business_data.entidad),
            ("rfc", &business_data.rfc),
            ("area_geografica", &business_data.area_geografica),
            ("prima", &business_data.prima),
        ];

        for (idx, (field_name, field_value)) in business_fields.into_iter().enumerate() {
            let row = idx as u32;
            worksheet.write_string(row, 0, field_name)?;
            worksheet.write_string(row, 1, field_value.as_deref().unwrap_or(""))?;
        }
        Ok(())
    }

    fn insert_column_names(
        worksheet: &mut Worksheet,
        header_format: &Format,
    ) -> Result<(), XlsxError> {
        let columns = vec![
            "no_seguro_social",
            "nombre",
            "rfc",
            "curp",
            "clave_ubicacion",
            "registro_obras",
            "mes_periodo",
            "anio_periodo",
            "clave",
            "fecha",
            "dias",
            "sdi",
            "lic",
            "inc",
            "aus",
            "cf",
            "exc_pat",
            "exc_obr",
            "pd_pat",
            "pd_obr",
            "gmp_pat",
            "gmp_obr",
            "rt",
            "iv_pat",
            "iv_obr",
            "gps",
            "patronal",
            "obrera",
            "subtotal",
        ];

        for (idx, col_name) in columns.iter().enumerate() {
            worksheet.write_string_with_format(19, idx as u16, *col_name, header_format)?;
        }
        Ok(())
    }

    fn write_opt_cell(
        ws: &mut Worksheet,
        r: u32,
        c: u16,
        val: Option<f64>,
    ) -> Result<(), XlsxError> {
        match val {
            Some(v) => ws.write_number(r, c, v)?,
            None => ws.write_number(r, c, 0.0)?,
        };
        Ok(())
    }

    fn adjust_width_cells(worksheet: &mut Worksheet) -> Result<(), XlsxError> {
        for col in 0..=28 {
            worksheet.set_column_width(col, 22.0)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_cedula_sheet_name_exact_examples() {
        let mut existing = HashSet::new();

        // User example 1: gondesa_A36_77458_10_3
        let name1 = make_cedula_sheet_name(
            Some("GONDESA DE MEXICO SA DE CV"),
            Some("A36_77458_10_3"),
            &mut existing,
        );
        assert_eq!(name1, "gondesa_A36_77458_10_3");

        // User example 2: machinen_A36-77252-10-3
        let name2 = make_cedula_sheet_name(
            Some("MACHINEN INDUSTRIES"),
            Some("A36-77252-10-3"),
            &mut existing,
        );
        assert_eq!(name2, "machinen_A36-77252-10-3");

        // User example 3: gondesa_A36_77459_10_7
        let name3 = make_cedula_sheet_name(
            Some("GONDESA DE MEXICO SA DE CV"),
            Some("A36_77459_10_7"),
            &mut existing,
        );
        assert_eq!(name3, "gondesa_A36_77459_10_7");

        // Collision handling
        let name4 = make_cedula_sheet_name(
            Some("GONDESA DE MEXICO SA DE CV"),
            Some("A36_77458_10_3"),
            &mut existing,
        );
        assert_eq!(name4, "gondesa_A36_77458_10_3_2");
    }

    #[test]
    fn test_make_cedula_sheet_name_max_length() {
        let mut existing = HashSet::new();
        let long_razon = "EMPRESA_SUPER_LARGA_DE_SERVICIOS_INDUSTRIALES_Y_COMERCIALES";
        let long_reg = "A36-99999-99-9-EXTRA-LONG";
        let sheet_name = make_cedula_sheet_name(Some(long_razon), Some(long_reg), &mut existing);
        assert!(sheet_name.chars().count() <= 31);
    }

    #[test]
    fn test_create_excel_multi_business() {
        let mut biz1 = Business::new();
        biz1.razon_social = Some("GONDESA DE MEXICO SA DE CV".to_string());
        biz1.registro_patronal = Some("A36_77458_10_3".to_string());

        let mut biz2 = Business::new();
        biz2.razon_social = Some("MACHINEN INDUSTRIES".to_string());
        biz2.registro_patronal = Some("A36-77252-10-3".to_string());

        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join("test_cedulas_multi.xlsx");

        let mut excel = Excel::new(&test_path.to_string_lossy());
        let sheets = vec![
            ("gondesa_A36_77458_10_3".to_string(), biz1),
            ("machinen_A36-77252-10-3".to_string(), biz2),
        ];

        let res = excel.create_excel_multi(&sheets, "spanish");
        assert!(res.is_ok());
        assert!(test_path.exists());
        let _ = std::fs::remove_file(test_path);
    }
}

