use chrono::NaiveDate;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::archives::collect_or_extract_files;
use crate::nominas::excel::generate_excel;
use crate::nominas::models::Employee;
use crate::nominas::parser::parse_xml_file;
use crate::sat::{create_sat_client, call_sat_webservice};

#[derive(Debug, Clone)]
pub struct UnprocessedNomina {
    pub filename: String,
    pub uuid: String,
    pub rfc_emisor: String,
    pub rfc_receptor: String,
    pub total: f64,
    pub category: String,
    pub reason: String,
    pub is_sat_error: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NominasStats {
    pub total_files: usize,
    pub valid_count: usize,
    pub duplicate_count: usize,
    pub canceled_count: usize,
    pub not_found_count: usize,
    pub parse_error_count: usize,
    pub other_invalid_count: usize,
    pub total_sueldos_validos: f64,
}

#[derive(Debug, Clone)]
pub struct NominasProcessResult {
    pub total_files: usize,
    pub valid_count: usize,
    pub invalid_count: usize,
    pub duplicate_count: usize,
    pub canceled_count: usize,
    pub not_found_count: usize,
    pub invalid_items: Vec<UnprocessedNomina>,
    pub excel_path: Option<PathBuf>,
}

pub fn process_nominas(
    input_paths: &[PathBuf],
    output_excel_path: &Path,
    temp_dir: &Path,
    progress_callback: impl Fn(usize, usize, &str) + Send + Sync,
) -> Result<NominasProcessResult, String> {
    progress_callback(0, 0, "Buscando y extrayendo nóminas XML...");

    let xml_files = collect_or_extract_files(input_paths, "xml", temp_dir, Some(&progress_callback))?;

    if xml_files.is_empty() {
        return Ok(NominasProcessResult {
            total_files: 0,
            valid_count: 0,
            invalid_count: 0,
            duplicate_count: 0,
            canceled_count: 0,
            not_found_count: 0,
            invalid_items: Vec::new(),
            excel_path: None,
        });
    }

    let total_files = xml_files.len();
    progress_callback(
        0,
        total_files,
        &format!("Se encontraron {} nóminas XML. Analizando estructura...", total_files),
    );

    // Dedicated Rayon ThreadPool for Nóminas CPU parsing
    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let cpu_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_cpus)
        .thread_name(|i| format!("nominas-cpu-{}", i))
        .build()
        .map_err(|e| format!("Error creando thread pool para nóminas: {}", e))?;

    // Dedicated high-concurrency ThreadPool for SAT SOAP network requests (I/O-bound concurrency)
    let sat_threads = (num_cpus * 4).clamp(16, 64);
    let sat_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(sat_threads)
        .thread_name(|i| format!("sat-soap-{}", i))
        .build()
        .map_err(|e| format!("Error creando thread pool para SAT: {}", e))?;

    // 1. Parse XML files in parallel with Rayon
    let parse_counter = std::sync::atomic::AtomicUsize::new(0);
    let parse_results: Vec<Result<(String, Employee), UnprocessedNomina>> = cpu_pool.install(|| {
        xml_files
            .par_iter()
            .map(|path| {
                let file_display_name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());

                let res = match parse_xml_file(path) {
                    Ok(emp) => Ok((file_display_name, emp)),
                    Err(e) => Err(UnprocessedNomina {
                        filename: file_display_name,
                        uuid: String::new(),
                        rfc_emisor: String::new(),
                        rfc_receptor: String::new(),
                        total: 0.0,
                        category: "ERROR XML".to_string(),
                        reason: format!("Error en estructura XML / lectura: {:#}", e),
                        is_sat_error: false,
                    }),
                };

                let p_done = parse_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if p_done % 40 == 0 || p_done == total_files {
                    progress_callback(
                        p_done / 5,
                        total_files,
                        &format!("Leyendo estructura XML ({}/{})...", p_done, total_files),
                    );
                }

                res
            })
            .collect()
    });

    let mut parsed_ok = Vec::new();
    let mut invalid_items = Vec::new();
    let mut parse_error_count = 0;

    for res in parse_results {
        match res {
            Ok(item) => parsed_ok.push(item),
            Err(inv) => {
                parse_error_count += 1;
                invalid_items.push(inv);
            }
        }
    }

    // 2. Deduplication check: Detect and filter duplicate UUIDs or identical receipts
    let mut seen_uuids = HashSet::new();
    let mut unique_parsed = Vec::new();
    let mut duplicate_count = 0;

    for (file_display_name, emp) in parsed_ok {
        let uuid_key = emp.uuid.as_deref().unwrap_or("").trim().to_uppercase();
        if !uuid_key.is_empty() {
            if seen_uuids.contains(&uuid_key) {
                duplicate_count += 1;
                invalid_items.push(UnprocessedNomina {
                    filename: file_display_name,
                    uuid: uuid_key,
                    rfc_emisor: emp.rfc_emisor.clone().unwrap_or_default(),
                    rfc_receptor: emp.rfc_receptor.clone().unwrap_or_default(),
                    total: emp.total.unwrap_or(0.0),
                    category: "DUPLICADA".to_string(),
                    reason: "Nómina DUPLICADA / REPETIDA: Este UUID ya fue procesado en otro archivo de este lote".to_string(),
                    is_sat_error: false,
                });
                continue;
            }
            seen_uuids.insert(uuid_key);
        }
        unique_parsed.push((file_display_name, emp));
    }

    let total_to_validate = unique_parsed.len();

    // 3. Validate unique CFDIs with SAT SOAP WebService concurrently in parallel
    let sat_client = create_sat_client();
    let completed_counter = std::sync::atomic::AtomicUsize::new(0);
    let valid_counter = std::sync::atomic::AtomicUsize::new(0);
    let invalid_counter = std::sync::atomic::AtomicUsize::new(0);
    let canceled_counter = std::sync::atomic::AtomicUsize::new(0);
    let not_found_counter = std::sync::atomic::AtomicUsize::new(0);

    let sat_results: Vec<Result<Employee, UnprocessedNomina>> = sat_pool.install(|| {
        unique_parsed
            .into_par_iter()
            .map(|(file_display_name, employee)| {
                let uuid = employee.uuid.clone().unwrap_or_default();
                let emisor = employee.rfc_emisor.clone().unwrap_or_default();
                let receptor = employee.rfc_receptor.clone().unwrap_or_default();
                let total = employee.total.unwrap_or(0.0);

                let res = if uuid.is_empty() || emisor.is_empty() || receptor.is_empty() {
                    invalid_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Err(UnprocessedNomina {
                        filename: file_display_name,
                        uuid: uuid.clone(),
                        rfc_emisor: emisor.clone(),
                        rfc_receptor: receptor.clone(),
                        total,
                        category: "DATOS INCOMPLETOS".to_string(),
                        reason: format!(
                            "Faltan datos fiscales requeridos para consultar al SAT (UUID: '{}', Emisor: '{}', Receptor: '{}')",
                            uuid, emisor, receptor
                        ),
                        is_sat_error: false,
                    })
                } else {
                    match call_sat_webservice(&sat_client, &emisor, &receptor, total, &uuid) {
                        Ok(sat_res) => {
                            if sat_res.is_valid {
                                valid_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                Ok(employee)
                            } else {
                                invalid_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if sat_res.estado.eq_ignore_ascii_case("Cancelado") {
                                    canceled_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    Err(UnprocessedNomina {
                                        filename: file_display_name,
                                        uuid,
                                        rfc_emisor: emisor,
                                        rfc_receptor: receptor,
                                        total,
                                        category: "CANCELADA SAT".to_string(),
                                        reason: format!(
                                            "Rechazado por el SAT: El comprobante está CANCELADO (Estatus: {})",
                                            sat_res.codigo_estatus
                                        ),
                                        is_sat_error: true,
                                    })
                                } else if sat_res.estado.eq_ignore_ascii_case("No Encontrado") {
                                    not_found_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    Err(UnprocessedNomina {
                                        filename: file_display_name,
                                        uuid,
                                        rfc_emisor: emisor.clone(),
                                        rfc_receptor: receptor.clone(),
                                        total,
                                        category: "NO ENCONTRADA SAT".to_string(),
                                        reason: format!(
                                            "Rechazado por el SAT: Comprobante NO ENCONTRADO (RFC Emisor: {}, RFC Receptor: {}, Total: {:.2})",
                                            emisor, receptor, total
                                        ),
                                        is_sat_error: true,
                                    })
                                } else {
                                    Err(UnprocessedNomina {
                                        filename: file_display_name,
                                        uuid,
                                        rfc_emisor: emisor,
                                        rfc_receptor: receptor,
                                        total,
                                        category: format!("ESTADO SAT: {}", sat_res.estado),
                                        reason: format!(
                                            "Rechazado por el SAT: Estado '{}' ({})",
                                            sat_res.estado, sat_res.codigo_estatus
                                        ),
                                        is_sat_error: true,
                                    })
                                }
                            }
                        }
                        Err(e) => {
                            invalid_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            Err(UnprocessedNomina {
                                filename: file_display_name,
                                uuid,
                                rfc_emisor: emisor,
                                rfc_receptor: receptor,
                                total,
                                category: "ERROR COMUNICACIÓN SAT".to_string(),
                                reason: format!("Error de comunicación con el WebService del SAT: {}", e),
                                is_sat_error: true,
                            })
                        }
                    }
                };

                let done = completed_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let val = valid_counter.load(std::sync::atomic::Ordering::Relaxed);
                let inv = invalid_counter.load(std::sync::atomic::Ordering::Relaxed);
                let remaining = total_to_validate.saturating_sub(done);

                progress_callback(
                    done,
                    total_to_validate,
                    &format!(
                        "{}/{} analizadas ({} válidas SAT, {} duplicadas, {} rechazadas | Faltan {})",
                        done, total_to_validate, val, duplicate_count, inv, remaining
                    ),
                );

                res
            })
            .collect()
    });

    let mut valid_employees = Vec::new();
    for res in sat_results {
        match res {
            Ok(emp) => valid_employees.push(emp),
            Err(inv) => invalid_items.push(inv),
        }
    }

    // Filter valid employees (require fecha final de pago and employee name)
    let initial_valid = valid_employees.len();
    valid_employees.retain(|emp| emp.nomina.fechafinalpago.is_some() && emp.nombre.is_some());

    let other_inv = if valid_employees.len() < initial_valid {
        let diff = initial_valid - valid_employees.len();
        log::warn!("Ignorados {} registros por carecer de fecha final o nombre", diff);
        diff
    } else {
        0
    };

    // Sort valid employees using dedicated CPU pool
    cpu_pool.install(|| {
        valid_employees.par_sort_unstable_by(|a, b| {
            let date_a = a
                .nomina
                .fechafinalpago
                .unwrap_or_else(|| NaiveDate::from_ymd_opt(1900, 1, 1).unwrap());
            let date_b = b
                .nomina
                .fechafinalpago
                .unwrap_or_else(|| NaiveDate::from_ymd_opt(1900, 1, 1).unwrap());
            date_a.cmp(&date_b).then(a.nombre.cmp(&b.nombre))
        });
    });

    let total_sueldos: f64 = valid_employees
        .iter()
        .map(|e| {
            e.nomina
                .percepciones
                .iter()
                .find(|p| p.concepto.contains("SUELDO"))
                .map_or(0.0, |p| p.importe)
        })
        .sum();

    let stats = NominasStats {
        total_files,
        valid_count: valid_employees.len(),
        duplicate_count,
        canceled_count: canceled_counter.load(std::sync::atomic::Ordering::Relaxed),
        not_found_count: not_found_counter.load(std::sync::atomic::Ordering::Relaxed),
        parse_error_count,
        other_invalid_count: other_inv,
        total_sueldos_validos: total_sueldos,
    };

    let mut generated_excel_path = None;
    if total_files > 0 {
        progress_callback(
            total_files,
            total_files,
            "Generando archivo Excel de nóminas (Estadísticas, Omitidas y Concentrado)...",
        );
        if let Some(parent) = output_excel_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        generate_excel(&valid_employees, &invalid_items, &stats, output_excel_path)
            .map_err(|e| format!("Error generando Excel de Nóminas: {}", e))?;
        generated_excel_path = Some(output_excel_path.to_path_buf());
    }

    Ok(NominasProcessResult {
        total_files,
        valid_count: valid_employees.len(),
        invalid_count: invalid_items.len(),
        duplicate_count,
        canceled_count: stats.canceled_count,
        not_found_count: stats.not_found_count,
        invalid_items,
        excel_path: generated_excel_path,
    })
}
