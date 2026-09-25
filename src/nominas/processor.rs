use chrono::NaiveDate;
use rayon::prelude::*;
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
    pub reason: String,
    pub is_sat_error: bool,
}

#[derive(Debug, Clone)]
pub struct NominasProcessResult {
    pub total_files: usize,
    pub valid_count: usize,
    pub invalid_count: usize,
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

    // Dedicated Rayon ThreadPool for Nóminas (with extra threads for concurrent SAT SOAP network requests)
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(8);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .thread_name(|i| format!("nominas-pool-{}", i))
        .build()
        .map_err(|e| format!("Error creando thread pool para nóminas: {}", e))?;

    // 1. Parse XML files in parallel with Rayon
    let parse_counter = std::sync::atomic::AtomicUsize::new(0);
    let parse_results: Vec<Result<(String, Employee), UnprocessedNomina>> = pool.install(|| {
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
    let mut parsed_results = Vec::new();
    let mut invalid_items = Vec::new();

    for res in parse_results {
        match res {
            Ok(item) => parsed_results.push(item),
            Err(inv) => invalid_items.push(inv),
        }
    }

    let total_to_validate = parsed_results.len();

    // 2. Validate with SAT SOAP WebService concurrently in parallel using dedicated pool
    let sat_client = create_sat_client();
    let completed_counter = std::sync::atomic::AtomicUsize::new(0);
    let valid_counter = std::sync::atomic::AtomicUsize::new(0);
    let invalid_counter = std::sync::atomic::AtomicUsize::new(0);

    let sat_results: Vec<Result<Employee, UnprocessedNomina>> = pool.install(|| {
        parsed_results
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
                            let msg = if sat_res.estado.eq_ignore_ascii_case("Cancelado") {
                                format!(
                                    "Rechazado por el SAT: El comprobante está CANCELADO (Estatus: {})",
                                    sat_res.codigo_estatus
                                )
                            } else if sat_res.estado.eq_ignore_ascii_case("No Encontrado") {
                                format!(
                                    "Rechazado por el SAT: Comprobante NO ENCONTRADO en los registros del SAT (RFC Emisor: {}, RFC Receptor: {}, Total: {:.2})",
                                    emisor, receptor, total
                                )
                            } else {
                                format!(
                                    "Rechazado por el SAT: Estado '{}' ({})",
                                    sat_res.estado, sat_res.codigo_estatus
                                )
                            };

                            Err(UnprocessedNomina {
                                filename: file_display_name,
                                uuid,
                                reason: msg,
                                is_sat_error: true,
                            })
                        }
                    }
                    Err(e) => {
                        invalid_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        Err(UnprocessedNomina {
                            filename: file_display_name,
                            uuid,
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
                    "{}/{} procesadas ({} válidas, {} rechazadas | Faltan {})",
                    done, total_to_validate, val, inv, remaining
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

    if valid_employees.len() < initial_valid {
        let diff = initial_valid - valid_employees.len();
        log::warn!("Ignorados {} registros por carecer de fecha final o nombre", diff);
    }

    // Sort valid employees using dedicated pool
    pool.install(|| {
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

    let mut generated_excel_path = None;
    if !valid_employees.is_empty() {
        progress_callback(
            total_files,
            total_files,
            "Generando archivo Excel de nóminas...",
        );
        if let Some(parent) = output_excel_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        generate_excel(&valid_employees, output_excel_path)
            .map_err(|e| format!("Error generando Excel de Nóminas: {}", e))?;
        generated_excel_path = Some(output_excel_path.to_path_buf());
    }

    Ok(NominasProcessResult {
        total_files,
        valid_count: valid_employees.len(),
        invalid_count: invalid_items.len(),
        invalid_items,
        excel_path: generated_excel_path,
    })
}
