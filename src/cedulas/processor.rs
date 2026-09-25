use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::archives::collect_or_extract_files;
use crate::cedulas::business::Business;
use crate::cedulas::employees::{Columns, Employee};
use crate::cedulas::excel::Excel;
use crate::cedulas::pdf_extractor::{clean_text, extract_with_layout};
use crate::cedulas::regex_expressions::{initialize_patterns, OCRPatterns};

#[derive(Debug, Clone)]
pub struct UnprocessedCedula {
    pub filename: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct CedulasProcessResult {
    pub total_files: usize,
    pub valid_count: usize,
    pub invalid_count: usize,
    pub invalid_items: Vec<UnprocessedCedula>,
    pub excel_path: Option<PathBuf>,
}

fn get_employee_data(data: &str, patterns: &OCRPatterns) -> Vec<Employee> {
    let mut employees = Vec::new();
    let mut columns = Columns::new();
    columns.get_column_names(data, &patterns.column_names, &patterns.columns_in_file);

    let re_emp_data = &patterns.employee_data_per_employee;
    let re_period = &patterns.employee_periodo_proceso;

    let date_period_caps = re_period.captures(data).ok().flatten();

    for cap_res in re_emp_data.captures_iter(data) {
        if let Ok(caps) = cap_res {
            if let Some(m) = caps.get(1) {
                let data_emp = m.as_str().trim();
                let mut employee = Employee::new();
                employee.set_values(
                    data_emp,
                    &columns.columns_in_file,
                    date_period_caps.as_ref(),
                    patterns,
                );
                employees.push(employee);
            }
        }
    }
    employees
}

pub fn canonical_reg(reg: &str) -> String {
    reg.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

pub fn canonical_razon(razon: &str) -> String {
    razon
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

pub fn process_cedulas(
    input_paths: &[PathBuf],
    output_excel_path: &Path,
    temp_dir: &Path,
    progress_callback: impl Fn(usize, usize, &str) + Send + Sync,
) -> Result<CedulasProcessResult, String> {
    progress_callback(0, 0, "Buscando y extrayendo cédulas PDF (procesamiento 100% local)...");

    let pdf_files = collect_or_extract_files(input_paths, "pdf", temp_dir, Some(&progress_callback))?;

    if pdf_files.is_empty() {
        return Ok(CedulasProcessResult {
            total_files: 0,
            valid_count: 0,
            invalid_count: 0,
            invalid_items: Vec::new(),
            excel_path: None,
        });
    }

    let total_files = pdf_files.len();
    progress_callback(
        0,
        total_files,
        &format!("Se encontraron {} archivos PDF. Analizando localmente en paralelo (sin peticiones web)...", total_files),
    );

    let patterns = Arc::new(
        initialize_patterns().map_err(|e| format!("Error compilando expresiones regulares OCR: {}", e))?,
    );

    let completed_counter = AtomicUsize::new(0);
    let valid_counter = AtomicUsize::new(0);
    let invalid_counter = AtomicUsize::new(0);

    // Dedicated Rayon ThreadPool for Cédulas to run 100% independently from Nóminas
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .thread_name(|i| format!("cedulas-pool-{}", i))
        .build()
        .map_err(|e| format!("Error creando thread pool para cédulas: {}", e))?;

    let parse_results: Vec<Result<Vec<(Business, Vec<Employee>)>, UnprocessedCedula>> = pool.install(|| {
        pdf_files
            .par_iter()
            .map(|pdf_path| {
                let file_display_name = pdf_path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| pdf_path.to_string_lossy().into_owned());

                // 1. Extract layout text with native MuPDF
                let raw_text = match extract_with_layout(pdf_path) {
                    Ok(t) => t,
                    Err(e) => {
                        invalid_counter.fetch_add(1, Ordering::Relaxed);
                        let cur = completed_counter.fetch_add(1, Ordering::Relaxed) + 1;
                        let val = valid_counter.load(Ordering::Relaxed);
                        let rem = total_files.saturating_sub(cur);
                        progress_callback(cur, total_files, &format!("{}/{} analizadas ({} válidas SUA | Faltan {})", cur, total_files, val, rem));
                        return Err(UnprocessedCedula {
                            filename: file_display_name,
                            reason: format!("Error al extraer texto con layout: {}", e),
                        });
                    }
                };

                // 2. Check for empty text / scanned PDF
                let trimmed_text = raw_text.trim();
                if trimmed_text.is_empty() || trimmed_text.len() < 50 {
                    invalid_counter.fetch_add(1, Ordering::Relaxed);
                    let cur = completed_counter.fetch_add(1, Ordering::Relaxed) + 1;
                    let val = valid_counter.load(Ordering::Relaxed);
                    let rem = total_files.saturating_sub(cur);
                    progress_callback(cur, total_files, &format!("{}/{} analizadas ({} válidas SUA | Faltan {})", cur, total_files, val, rem));
                    return Err(UnprocessedCedula {
                        filename: file_display_name,
                        reason: "El archivo PDF no contiene texto legible (posible documento escaneado o imagen sin capa OCR)".to_string(),
                    });
                }

                // 3. Check for SUA cédula header markers
                let text_lower = raw_text.to_lowercase();
                let is_sua_cedula = text_lower.contains("cédula de determinación de cuotas")
                    || text_lower.contains("cedula de determinacion de cuotas")
                    || text_lower.contains("sistema único de autodeterminación")
                    || text_lower.contains("sistema unico de autodeterminacion")
                    || (text_lower.contains("registro patronal") && text_lower.contains("delegación imss"));

                if !is_sua_cedula {
                    invalid_counter.fetch_add(1, Ordering::Relaxed);
                    let cur = completed_counter.fetch_add(1, Ordering::Relaxed) + 1;
                    let val = valid_counter.load(Ordering::Relaxed);
                    let rem = total_files.saturating_sub(cur);
                    progress_callback(cur, total_files, &format!("{}/{} analizadas ({} válidas SUA | Faltan {})", cur, total_files, val, rem));
                    return Err(UnprocessedCedula {
                        filename: file_display_name,
                        reason: "El documento no corresponde al formato oficial de Cédula de Determinación de Cuotas del SUA (faltan encabezados)".to_string(),
                    });
                }

                // 4. Split into pages and clean
                let mut pages: Vec<String> = raw_text.split('\x0c').map(|s| s.to_string()).collect();
                if let Some(last) = pages.last() {
                    if last.trim().is_empty() {
                        pages.pop();
                    }
                }

                for page in &mut pages {
                    *page = clean_text(&page.to_lowercase(), &patterns);
                }

                // 5. Extract per-page / per-chunk business and employees, handling multi-company or multi-registration PDFs
                let mut pdf_sections: Vec<(Business, Vec<Employee>)> = Vec::new();
                let mut current_biz = Business::new();
                let mut current_emps = Vec::new();

                for page in &pages {
                    // Check if multiple registro patronal headers exist within this single page
                    let header_positions: Vec<usize> = patterns
                        .business_registro_patronal
                        .find_iter(page)
                        .filter_map(|m| m.ok().map(|mat| mat.start()))
                        .collect();

                    let chunks: Vec<&str> = if header_positions.len() > 1 {
                        let mut chs = Vec::new();
                        for i in 0..header_positions.len() {
                            let start = if i == 0 { 0 } else { header_positions[i] };
                            let end = if i + 1 < header_positions.len() { header_positions[i + 1] } else { page.len() };
                            chs.push(&page[start..end]);
                        }
                        chs
                    } else {
                        vec![page.as_str()]
                    };

                    for chunk in chunks {
                        let mut chunk_biz = Business::new();
                        chunk_biz.extract_from_page(chunk, &patterns);

                        let has_new_reg = match (&chunk_biz.registro_patronal, &current_biz.registro_patronal) {
                            (Some(new_r), Some(curr_r)) => canonical_reg(new_r) != canonical_reg(curr_r),
                            (Some(_), None) => true,
                            _ => false,
                        };

                        let has_new_razon = match (&chunk_biz.razon_social, &current_biz.razon_social) {
                            (Some(new_rz), Some(curr_rz)) => canonical_razon(new_rz) != canonical_razon(curr_rz),
                            (Some(_), None) => true,
                            _ => false,
                        };

                        let is_new_section = (has_new_reg || has_new_razon)
                            && (current_biz.registro_patronal.is_some() || current_biz.razon_social.is_some());

                        if is_new_section {
                            if !current_emps.is_empty() {
                                pdf_sections.push((current_biz.clone(), current_emps));
                                current_emps = Vec::new();
                            }
                            let mut new_biz = chunk_biz.clone();
                            if new_biz.razon_social.is_none() {
                                new_biz.razon_social = current_biz.razon_social.clone();
                            }
                            if new_biz.delegacion.is_none() {
                                new_biz.delegacion = current_biz.delegacion.clone();
                                new_biz.codigo_delegacion = current_biz.codigo_delegacion.clone();
                            }
                            if new_biz.subdelegacion.is_none() {
                                new_biz.subdelegacion = current_biz.subdelegacion.clone();
                                new_biz.codigo_subdelegacion = current_biz.codigo_subdelegacion.clone();
                            }
                            current_biz = new_biz;
                        } else {
                            current_biz.merge_with(&chunk_biz, &file_display_name);
                        }

                        let emps = get_employee_data(chunk, &patterns);
                        current_emps.extend(emps);
                    }
                }

                if !current_emps.is_empty() {
                    pdf_sections.push((current_biz, current_emps));
                }

                if pdf_sections.is_empty() || pdf_sections.iter().all(|(_, emps)| emps.is_empty()) {
                    invalid_counter.fetch_add(1, Ordering::Relaxed);
                    let cur = completed_counter.fetch_add(1, Ordering::Relaxed) + 1;
                    let val = valid_counter.load(Ordering::Relaxed);
                    let rem = total_files.saturating_sub(cur);
                    progress_callback(cur, total_files, &format!("{}/{} analizadas ({} válidas SUA | Faltan {})", cur, total_files, val, rem));
                    return Err(UnprocessedCedula {
                        filename: file_display_name,
                        reason: "No se pudieron extraer registros de trabajadores válidos en la cédula (posible formato incompatible o resumen sin desglose)".to_string(),
                    });
                }

                valid_counter.fetch_add(1, Ordering::Relaxed);
                let cur = completed_counter.fetch_add(1, Ordering::Relaxed) + 1;
                let val = valid_counter.load(Ordering::Relaxed);
                let rem = total_files.saturating_sub(cur);
                progress_callback(
                    cur,
                    total_files,
                    &format!("{}/{} analizadas localmente ({} válidas SUA | Faltan {})", cur, total_files, val, rem),
                );

                Ok(pdf_sections)
            })
            .collect()
    });

    let mut grouped_businesses: HashMap<String, Business> = HashMap::new();
    let mut invalid_items = Vec::new();
    let mut valid_count = 0;

    for res in parse_results {
        match res {
            Ok(sections) => {
                valid_count += 1;
                for (biz, emps) in sections {
                    // Grouping key: Primary is canonical registro_patronal, secondary is canonical razon_social
                    let reg_opt = biz.registro_patronal.as_deref().map(canonical_reg);
                    let razon_opt = biz.razon_social.as_deref().map(canonical_razon);

                    let key = match (reg_opt, razon_opt) {
                        (Some(r), _) if !r.is_empty() => r,
                        (_, Some(rz)) if !rz.is_empty() => rz,
                        _ => "DEFAULT_CEDULA".to_string(),
                    };

                    let entry = grouped_businesses.entry(key).or_insert_with(Business::new);
                    entry.merge_with(&biz, "");
                    entry.employees.extend(emps);
                }
            }
            Err(inv) => {
                invalid_items.push(inv);
            }
        }
    }

    let mut generated_excel_path = None;
    if valid_count > 0 && !grouped_businesses.is_empty() {
        progress_callback(
            total_files,
            total_files,
            "Generando reporte Excel de cédulas...",
        );

        if let Some(parent) = output_excel_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut keys: Vec<String> = grouped_businesses.keys().cloned().collect();
        keys.sort();

        let mut existing_sheet_names = HashSet::new();
        let mut sheets: Vec<(String, Business)> = Vec::new();

        for key in keys {
            if let Some(mut biz) = grouped_businesses.remove(&key) {
                if biz.employees.is_empty() {
                    continue;
                }
                biz.sort_employee_data();
                let sheet_name = crate::cedulas::excel::make_cedula_sheet_name(
                    biz.razon_social.as_deref(),
                    biz.registro_patronal.as_deref(),
                    &mut existing_sheet_names,
                );
                sheets.push((sheet_name, biz));
            }
        }

        if !sheets.is_empty() {
            let mut excel = Excel::new(&output_excel_path.to_string_lossy());
            excel
                .create_excel_multi(&sheets, "spanish")
                .map_err(|e| format!("Error creando archivo Excel de Cédulas: {}", e))?;

            generated_excel_path = Some(output_excel_path.to_path_buf());
        }
    }

    Ok(CedulasProcessResult {
        total_files,
        valid_count,
        invalid_count: invalid_items.len(),
        invalid_items,
        excel_path: generated_excel_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_reg() {
        assert_eq!(canonical_reg("A36_77458_10_3"), "A3677458103");
        assert_eq!(canonical_reg("A36-77458-10-3"), "A3677458103");
        assert_eq!(canonical_reg("a36 77458 10 3"), "A3677458103");
        assert_eq!(canonical_reg("A3677458103"), "A3677458103");

        // Different registration numbers must not match
        assert_ne!(canonical_reg("A36-77458-10-3"), canonical_reg("A36-77459-10-7"));
    }

    #[test]
    fn test_canonical_razon() {
        assert_eq!(canonical_razon("GONDESA DE MEXICO SA DE CV"), "GONDESA DE MEXICO SA DE CV");
        assert_eq!(canonical_razon("  gondesa   de   mexico,   s.a. de c.v. "), "GONDESA DE MEXICO SA DE CV");
    }

    #[test]
    fn test_multi_patronal_sheet_creation() {
        let mut grouped: HashMap<String, Business> = HashMap::new();

        // Business 1: Gondesa Reg 1
        let mut b1 = Business::new();
        b1.razon_social = Some("GONDESA DE MEXICO SA DE CV".to_string());
        b1.registro_patronal = Some("A36-77458-10-3".to_string());
        let mut emp1 = Employee::new();
        emp1.nombre = Some("JUAN PEREZ".to_string());
        b1.employees.push(emp1);

        // Business 2: Same company Gondesa, but Reg 2
        let mut b2 = Business::new();
        b2.razon_social = Some("GONDESA DE MEXICO SA DE CV".to_string());
        b2.registro_patronal = Some("A36_77459_10_7".to_string());
        let mut emp2 = Employee::new();
        emp2.nombre = Some("PEDRO GOMEZ".to_string());
        b2.employees.push(emp2);

        // Business 3: Machinen Reg 3
        let mut b3 = Business::new();
        b3.razon_social = Some("MACHINEN CORP".to_string());
        b3.registro_patronal = Some("A36-77252-10-3".to_string());
        let mut emp3 = Employee::new();
        emp3.nombre = Some("CARLOS RUIZ".to_string());
        b3.employees.push(emp3);

        grouped.insert(canonical_reg("A36-77458-10-3"), b1);
        grouped.insert(canonical_reg("A36_77459_10_7"), b2);
        grouped.insert(canonical_reg("A36-77252-10-3"), b3);

        assert_eq!(grouped.len(), 3);

        let mut existing_names = HashSet::new();
        let mut sheet_names = Vec::new();

        for (_key, biz) in &grouped {
            let name = crate::cedulas::excel::make_cedula_sheet_name(
                biz.razon_social.as_deref(),
                biz.registro_patronal.as_deref(),
                &mut existing_names,
            );
            sheet_names.push(name);
        }

        assert_eq!(sheet_names.len(), 3);
        assert!(sheet_names.iter().any(|s| s.contains("gondesa") && s.contains("77458")));
        assert!(sheet_names.iter().any(|s| s.contains("gondesa") && s.contains("77459")));
        assert!(sheet_names.iter().any(|s| s.contains("machinen") && s.contains("77252")));
    }
}

