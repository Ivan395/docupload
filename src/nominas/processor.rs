use chrono::NaiveDate;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

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
    pub total_septimo_dia_validos: f64,
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
    /// Error al guardar en la base de datos (antes solo se registraba en el log).
    pub error_guardado: Option<String>,
}

pub fn process_nominas(
    input_paths: &[PathBuf],
    output_excel_path: &Path,
    temp_dir: &Path,
    progress_callback: impl Fn(usize, usize, &str) + Send + Sync,
    revision_id: Uuid,
    expected_client_rfc: Option<&str>,
    expected_rp: Option<&str>,
    // Si se indica, las discrepancias (RFC emisor/RP distintos, errores de captura de NSS, RFC,
    // CURP o nombre) se presentan al usuario para que decida qué cargar en lugar de descartarse.
    revisor: Option<&dyn Fn(Vec<String>, Vec<crate::advertencias_carga::AdvertenciaCarga>) -> crate::advertencias_carga::DecisionCarga>,
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
            error_guardado: None,
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

    // 2. Validate client RFC and Registro Patronal against active revision
    let clean_exp_rfc = expected_client_rfc.map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let clean_exp_rp = expected_rp.map(|s| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()).filter(|s| !s.is_empty() && s != "GENERAL");

    let mut context_filtered = Vec::new();
    // XML con datos del patrón distintos a la revisión: con `revisor` se presentan como advertencia
    // (el usuario decide); sin él se descartan como antes.
    let mut patron_items: HashMap<String, UnprocessedNomina> = HashMap::new();
    let mut patron_advs: Vec<crate::advertencias_carga::AdvertenciaCarga> = Vec::new();
    for (file_display_name, emp) in parsed_ok {
        // Validar RFC Emisor
        if let Some(ref exp_rfc) = clean_exp_rfc {
            let emisor = emp.rfc_emisor.as_deref().unwrap_or("").trim().to_uppercase();
            if !emisor.is_empty() && emisor != *exp_rfc {
                let mut adv = crate::advertencias_carga::advertencia_patron(
                    crate::advertencias_carga::CampoAdvertencia::RfcPatron,
                    &emp.nombrearchivo,
                    &emisor,
                    exp_rfc,
                    format!("El RFC emisor del XML ({}) no es el RFC del cliente de la revisión ({}).", emisor, exp_rfc),
                    false,
                );
                adv.solo_decidir_archivo = true;
                patron_advs.push(adv);
                patron_items.insert(emp.nombrearchivo.clone(), UnprocessedNomina {
                    filename: file_display_name.clone(),
                    uuid: emp.uuid.clone().unwrap_or_default(),
                    rfc_emisor: emisor.clone(),
                    rfc_receptor: emp.rfc_receptor.clone().unwrap_or_default(),
                    total: emp.total.unwrap_or(0.0),
                    category: "RFC EMISOR NO COINCIDE".to_string(),
                    reason: format!(
                        "El RFC emisor '{}' en el XML no coincide con el RFC '{}' registrado para la empresa en la aplicación.",
                        emisor, exp_rfc
                    ),
                    is_sat_error: false,
                });
                context_filtered.push((file_display_name, emp));
                continue;
            }
        }

        // Validar Registro Patronal si el XML lo incluye
        if let Some(ref exp_rp) = clean_exp_rp {
            if let Some(ref rp) = emp.registro_patronal {
                let clean_rp: String = rp.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase();
                if !clean_rp.is_empty() && clean_rp != *exp_rp {
                    let mut adv = crate::advertencias_carga::advertencia_patron(
                        crate::advertencias_carga::CampoAdvertencia::RegistroPatronal,
                        &emp.nombrearchivo,
                        &clean_rp,
                        exp_rp,
                        format!("El Registro Patronal del XML ({}) no es el de la revisión ({}).", clean_rp, exp_rp),
                        true,
                    );
                    adv.solo_decidir_archivo = true;
                    patron_advs.push(adv);
                    patron_items.insert(emp.nombrearchivo.clone(), UnprocessedNomina {
                        filename: file_display_name.clone(),
                        uuid: emp.uuid.clone().unwrap_or_default(),
                        rfc_emisor: emp.rfc_emisor.clone().unwrap_or_default(),
                        rfc_receptor: emp.rfc_receptor.clone().unwrap_or_default(),
                        total: emp.total.unwrap_or(0.0),
                        category: "REGISTRO PATRONAL NO COINCIDE".to_string(),
                        reason: format!(
                            "El Registro Patronal '{}' en el XML no coincide con el Registro Patronal '{}' registrado para la revisión.",
                            rp.trim(), exp_rp
                        ),
                        is_sat_error: false,
                    });
                    context_filtered.push((file_display_name, emp));
                    continue;
                }
            }
        }

        context_filtered.push((file_display_name, emp));
    }

    // 3. Deduplication check: Detect and filter duplicate UUIDs or identical receipts
    let mut seen_uuids = HashSet::new();
    let mut unique_parsed = Vec::new();
    let mut duplicate_count = 0;

    for (file_display_name, emp) in context_filtered {
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

    // Requisito mínimo para persistir: fecha final de pago y nombre del
    // trabajador. Una nómina que SUPERVIVIÓ la validación SAT pero no cumple
    // esto no puede guardarse (el modelo exige esas columnas), pero tampoco debe
    // desaparecer: antes solo se contaba con `log::warn!` y quedaba fuera tanto
    // de la base de datos como del Excel de "omitidas", así que el usuario no
    // tenía ninguna traza. Ahora cada descarte entra a `invalid_items` con su
    // motivo explícito, de modo que aparece en la hoja de no procesadas y en la
    // lista de rechazados de la UI.
    let mut dropped_missing_fields = 0usize;
    valid_employees.retain(|emp| {
        let falta_fecha = emp.nomina.fechafinalpago.is_none();
        let falta_nombre = emp.nombre.is_none();
        if !falta_fecha && !falta_nombre {
            return true;
        }

        let mut motivos: Vec<&str> = Vec::new();
        if falta_nombre {
            motivos.push("falta el nombre del trabajador");
        }
        if falta_fecha {
            motivos.push("falta la fecha final de pago");
        }
        let motivo = format!(
            "Datos incompletos (válida ante SAT pero {}): no se puede registrar en la base de datos",
            motivos.join(" y ")
        );

        log::warn!("Nómina descartada ({}): {}", emp.nombrearchivo, motivo);
        invalid_items.push(UnprocessedNomina {
            filename: emp.nombrearchivo.clone(),
            uuid: emp.uuid.as_deref().unwrap_or("").trim().to_uppercase(),
            rfc_emisor: emp.rfc_emisor.clone().unwrap_or_default(),
            rfc_receptor: emp.rfc_receptor.clone().unwrap_or_default(),
            total: emp.total.unwrap_or(0.0),
            category: "DATOS INCOMPLETOS".to_string(),
            reason: motivo,
            is_sat_error: false,
        });
        dropped_missing_fields += 1;
        false
    });

    // ── Revisión de discrepancias: el usuario decide qué nóminas cargar ──
    match revisor {
        None => {
            if !patron_items.is_empty() {
                valid_employees.retain(|e| !patron_items.contains_key(&e.nombrearchivo));
                invalid_items.extend(patron_items.into_values());
            }
        }
        Some(revisar) => {
            use crate::advertencias_carga as adv;
            let catalogo = crate::db::get_global_pool()
                .and_then(|p| p.get().ok())
                .map(|mut c| {
                    adv::cargar_catalogo(&mut *c, revision_id).unwrap_or_else(|e| {
                        log::warn!("Advertencias de nóminas: sin catálogo de empleados ({})", e);
                        adv::CatalogoEmpleados::default()
                    })
                })
                .unwrap_or_default();
            let registros: Vec<adv::RegistroTrabajador> = valid_employees
                .iter()
                .map(|e| adv::RegistroTrabajador {
                    archivo: e.nombrearchivo.clone(),
                    nss: e.numseguridadsocial.clone(),
                    nombre: e.nombre.clone(),
                    rfc: e.rfc_receptor.clone(),
                    curp: e.curp_receptor.clone(),
                })
                .collect();
            let presentes: HashSet<&str> = valid_employees.iter().map(|e| e.nombrearchivo.as_str()).collect();
            let mut advertencias = adv::detectar_trabajadores(&registros, &catalogo);
            advertencias.extend(patron_advs.into_iter().filter(|a| a.archivos.iter().any(|f| presentes.contains(f.as_str()))));
            let advertencias = adv::numerar(advertencias);
            if !advertencias.is_empty() {
                let archivos: Vec<String> = presentes
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<std::collections::BTreeSet<String>>()
                    .into_iter()
                    .collect();
                progress_callback(0, 0, "Esperando tu decisión sobre las discrepancias encontradas...");
                match revisar(archivos.clone(), advertencias.clone()) {
                    adv::DecisionCarga::Cancelar => {
                        for e in valid_employees.drain(..) {
                            invalid_items.push(UnprocessedNomina {
                                filename: e.nombrearchivo.clone(),
                                uuid: e.uuid.as_deref().unwrap_or("").trim().to_uppercase(),
                                rfc_emisor: e.rfc_emisor.clone().unwrap_or_default(),
                                rfc_receptor: e.rfc_receptor.clone().unwrap_or_default(),
                                total: e.total.unwrap_or(0.0),
                                category: "CARGA CANCELADA".to_string(),
                                reason: "Se decidió no cargar ninguna nómina al revisar las discrepancias.".to_string(),
                                is_sat_error: false,
                            });
                        }
                    }
                    adv::DecisionCarga::Cargar { incluidos, acciones } => {
                        let cambios = adv::cambios_por_trabajador(&advertencias, &acciones);
                        let mut conservadas = Vec::with_capacity(valid_employees.len());
                        for mut e in valid_employees.drain(..) {
                            if !incluidos.contains(&e.nombrearchivo) {
                                invalid_items.push(UnprocessedNomina {
                                    filename: e.nombrearchivo.clone(),
                                    uuid: e.uuid.as_deref().unwrap_or("").trim().to_uppercase(),
                                    rfc_emisor: e.rfc_emisor.clone().unwrap_or_default(),
                                    rfc_receptor: e.rfc_receptor.clone().unwrap_or_default(),
                                    total: e.total.unwrap_or(0.0),
                                    category: "OMITIDA POR EL USUARIO".to_string(),
                                    reason: "Se decidió no cargar esta nómina al revisar sus discrepancias.".to_string(),
                                    is_sat_error: false,
                                });
                                continue;
                            }
                            let clave = adv::ClaveTrabajador::nueva(
                                e.numseguridadsocial.as_deref(),
                                e.nombre.as_deref(),
                                e.rfc_receptor.as_deref(),
                                e.curp_receptor.as_deref(),
                            );
                            if let Some(c) = cambios.get(&(e.nombrearchivo.clone(), clave)) {
                                for (campo, v) in c {
                                    match campo {
                                        adv::CampoAdvertencia::Nss => e.numseguridadsocial = Some(v.clone()),
                                        adv::CampoAdvertencia::Rfc => e.rfc_receptor = Some(v.clone()),
                                        adv::CampoAdvertencia::Curp => e.curp_receptor = Some(v.clone()),
                                        adv::CampoAdvertencia::Nombre => e.nombre = Some(v.clone()),
                                        _ => {}
                                    }
                                }
                            }
                            conservadas.push(e);
                        }
                        valid_employees = conservadas;
                    }
                }
            }
        }
    }

    let other_inv = dropped_missing_fields;

    // Sort valid employees using dedicated CPU pool
    cpu_pool.install(|| {
        valid_employees.par_sort_unstable_by(|a, b| {
            let (year_a, month_a) = a.get_year_month();
            let (year_b, month_b) = b.get_year_month();
            let date_a = a
                .nomina
                .fechafinalpago
                .unwrap_or_else(|| NaiveDate::from_ymd_opt(1900, 1, 1).unwrap());
            let date_b = b
                .nomina
                .fechafinalpago
                .unwrap_or_else(|| NaiveDate::from_ymd_opt(1900, 1, 1).unwrap());
            year_a
                .cmp(&year_b)
                .then(month_a.cmp(&month_b))
                .then(date_a.cmp(&date_b))
                .then(a.nombre.cmp(&b.nombre))
        });
    });

    let total_sueldos: f64 = valid_employees
        .iter()
        .map(|e| e.get_sueldo_total_con_septimo())
        .sum();

    let total_septimo: f64 = valid_employees
        .iter()
        .map(|e| e.get_septimo_dia_importe())
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
        total_septimo_dia_validos: total_septimo,
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

    // Guardar en la base de datos PostgreSQL si hay empleados válidos
    let mut error_guardado: Option<String> = None;
    if !valid_employees.is_empty() {
        let total_val = valid_employees.len();
        progress_callback(
            0,
            total_val,
            "Conectando a PostgreSQL y preparando guardado concurrente de nóminas...",
        );

        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let pool_size = (num_cpus * 2).clamp(4, 32) as u32;

        match crate::db::create_pool(pool_size) {
            Ok(pool) => {
                let cb = |done: usize, total: usize| {
                    progress_callback(
                        done,
                        total,
                        &format!("Guardando nóminas en PostgreSQL de forma concurrente ({}/{})...", done, total),
                    );
                };

                match crate::db::save_nominas_concurrent(&pool, &valid_employees, Some(&cb), revision_id) {
                    Ok(db_stats) => {
                        log::info!(
                            "Nóminas guardadas en PostgreSQL concurrentemente: {} nóminas, {} cfdis, {} contratos, {} detalles, {} empleados",
                            db_stats.nominas_guardadas,
                            db_stats.cfdis_guardados,
                            db_stats.contratos_guardados,
                            db_stats.detalles_guardados,
                            db_stats.empleados_guardados
                        );
                        if !db_stats.errores.is_empty() {
                            for err in db_stats.errores {
                                log::warn!("Advertencia al guardar nóminas concurrentemente en BD: {}", err);
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Error al guardar nóminas concurrentes en PostgreSQL: {}", e);
                        error_guardado = Some(e.to_string());
                    }
                }
            }
            Err(e) => {
                log::warn!("No se pudo inicializar pool de PostgreSQL para guardar nóminas: {}", e);
                error_guardado = Some(format!("Sin conexión con la base de datos: {}", e));
            }
        }
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
        error_guardado,
    })
}
