use rust_xlsxwriter::{Color, Format, Workbook, XlsxError};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::nominas::models::Employee;

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

pub fn generate_excel(employees: &[Employee], out_path: &Path) -> Result<(), XlsxError> {
    let mut workbook = Workbook::new();

    let header_format = Format::new()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1D4ED8)) // Elegant Royal Blue
        .set_align(rust_xlsxwriter::FormatAlign::Center);

    let currency_format = Format::new().set_num_format("$#,##0.00");
    let integer_format = Format::new().set_num_format("#,##0");

    // Group employees by (RFC, Company Name)
    let mut companies_map: HashMap<(String, String), Vec<&Employee>> = HashMap::new();
    for emp in employees {
        let rfc = emp.rfc_emisor.clone().unwrap_or_else(|| "SIN_RFC".to_string());
        let nombre = emp.nombre_emisor.clone().unwrap_or_default();
        companies_map.entry((rfc, nombre)).or_default().push(emp);
    }

    if companies_map.is_empty() {
        let worksheet = workbook.add_worksheet();
        worksheet.set_name("Nominas")?;
    } else {
        let mut company_keys: Vec<(String, String)> = companies_map.keys().cloned().collect();
        company_keys.sort();

        let mut existing_names = HashSet::new();

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

        let res = generate_excel(&[emp1, emp2], &test_path);
        assert!(res.is_ok());
        assert!(test_path.exists());
        let _ = std::fs::remove_file(test_path);
    }
}

