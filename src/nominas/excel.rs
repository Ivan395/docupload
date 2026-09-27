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

    // Title banner
    worksheet.merge_range(0, 0, 0, 4, "DOCUUPLOAD - RESUMEN EJECUTIVO Y ESTADÍSTICAS DE NÓMINAS", &title_format)?;

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

    worksheet.write_string_with_format(10, 0, "Importe Total Bruto de Sueldos Válidos", &kpi_label_format)?;
    worksheet.write_number_with_format(10, 1, stats.total_sueldos_validos, &currency_kpi_format)?;

    // Company breakdown table
    worksheet.merge_range(13, 0, 13, 3, "DESGLOSE POR EMPRESA / PATRÓN EMISOR", &table_head_format)?;

    let comp_headers = ["RAZÓN SOCIAL EMISOR", "RFC EMISOR", "NÓMINAS VÁLIDAS", "TOTAL SUELDOS ($)"];
    for (col, head) in comp_headers.iter().enumerate() {
        worksheet.write_string_with_format(14, col as u16, *head, &table_head_format)?;
    }

    let mut current_row = 15;
    let mut company_keys: Vec<(String, String)> = companies_map.keys().cloned().collect();
    company_keys.sort();

    for (rfc, nombre) in company_keys {
        let emps = &companies_map[&(rfc.clone(), nombre.clone())];
        let count = emps.len();
        let total_sueldos: f64 = emps
            .iter()
            .map(|e| {
                e.nomina
                    .percepciones
                    .iter()
                    .find(|p| p.concepto.contains("SUELDO"))
                    .map_or(0.0, |p| p.importe)
            })
            .sum();

        let display_nombre = if nombre.is_empty() { "SIN_RAZON_SOCIAL" } else { &nombre };
        worksheet.write_string_with_format(current_row, 0, display_nombre, &cell_text_format)?;
        worksheet.write_string_with_format(current_row, 1, &rfc, &cell_text_format)?;
        worksheet.write_number_with_format(current_row, 2, count as f64, &cell_num_format)?;
        worksheet.write_number_with_format(current_row, 3, total_sueldos, &cell_curr_format)?;
        current_row += 1;
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
        .set_align(rust_xlsxwriter::FormatAlign::Center);

    let currency_format = Format::new().set_num_format("$#,##0.00");
    let integer_format = Format::new().set_num_format("#,##0");

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
                "NOMBRE_ARCHIVO",
                "FECHA_INICIAL",
                "FECHA_FINAL",
                "FECHA_PAGO",
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

            for (col, header) in headers.iter().enumerate() {
                worksheet.write_string_with_format(0, col as u16, *header, &header_format)?;
            }

            // Write Data rows
            for (row_idx, emp) in emps.iter().enumerate() {
                let row = (row_idx + 1) as u32;

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
                let sueldo_importe = emp
                    .nomina
                    .percepciones
                    .iter()
                    .find(|p| p.concepto.contains("SUELDO"))
                    .map_or(0.0, |p| p.importe);

                worksheet.write_string(row, 0, &emp.nombrearchivo)?;
                worksheet.write_string(row, 1, &fecha_ini)?;
                worksheet.write_string(row, 2, &fecha_fin)?;
                worksheet.write_string(row, 3, &fecha_pago)?;
                worksheet.write_string(row, 4, emp.nombre.as_deref().unwrap_or(""))?;
                worksheet.write_string(row, 5, emp.numseguridadsocial.as_deref().unwrap_or(""))?;
                worksheet.write_number_with_format(
                    row,
                    6,
                    emp.salariodiariocalculado.unwrap_or(0.0),
                    &currency_format,
                )?;
                worksheet.write_number_with_format(
                    row,
                    7,
                    emp.salariobasecotapor.unwrap_or(0.0),
                    &currency_format,
                )?;
                worksheet.write_number_with_format(
                    row,
                    8,
                    emp.salariodiariointegradocalculado.unwrap_or(0.0),
                    &currency_format,
                )?;
                worksheet.write_number_with_format(
                    row,
                    9,
                    emp.nomina.numdiaspagados.unwrap_or(0.0),
                    &integer_format,
                )?;
                worksheet.write_number_with_format(row, 10, sueldo_importe, &currency_format)?;

                let mut row_conceptos: HashMap<&str, f64> = HashMap::new();
                for p in &emp.nomina.percepciones {
                    row_conceptos.insert(&p.concepto, p.importe);
                }

                for (i, col_name) in columns_conceptos.iter().enumerate() {
                    let col_idx = (11 + i) as u16;
                    let val = row_conceptos.get(col_name.as_str()).unwrap_or(&0.0);
                    worksheet.write_number_with_format(row, col_idx, *val, &currency_format)?;
                }
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
}
