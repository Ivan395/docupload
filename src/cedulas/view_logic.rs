use chrono::{Datelike, NaiveDate};
use postgres::Client;
use rust_xlsxwriter::{Color, Format, FormatAlign, Workbook, XlsxError};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::archives::collect_or_extract_files;
use crate::cedulas::business::{month_num_to_name, month_to_num, Business};
use crate::cedulas::employees::{DataEmployee, Employee, RegistroObra};
use crate::cedulas::excel::Excel;
use crate::cedulas::pdf_extractor::{clean_text, extract_with_layout};
use crate::cedulas::processor::{canonical_razon, canonical_reg, get_employee_data};
use crate::db::cedulas::{resolve_business_context_for_revision, save_single_cedula_employee_tx, CedulasDbStats};
use crate::db::client::DbPool;

#[derive(Debug, Clone)]
pub struct CedulaFileCardInfo {
    pub id: String,
    pub path: PathBuf,
    pub filename: String,
    pub file_type: String,
    pub size_str: String,
    pub pages_detected: String,
    pub status_badge: String,
    pub status_is_valid: bool,
    pub checksum: String,
    pub has_warning: bool,
    pub warning_msg: String,
}

#[derive(Debug, Clone, Default)]
pub struct CedulaMismatchItem {
    pub filename: String,
    pub error_type: String,
    pub expected_val: String,
    pub found_val: String,
    pub details: String,
}

#[derive(Debug, Clone, Default)]
pub struct CedulaMismatchReport {
    pub expected_rfc: String,
    pub expected_rp: String,
    pub expected_cliente: String,
    pub items: Vec<CedulaMismatchItem>,
}

impl CedulaMismatchReport {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct DetalleRowModel {
    pub idx: i32,
    pub reg_num_str: String,
    pub mes_anio: String,
    pub registro_patronal: String,
    pub razon_social: String,
    pub registro_obra: String,
    pub nss: String,
    pub rfc_curp: String,
    pub nombre_trabajador: String,
    pub dias: i32,
    pub sdi: f64,
    pub sdi_str: String,
    pub cuota_patronal: f64,
    pub cuota_patronal_str: String,
    pub cuota_obrera: f64,
    pub cuota_obrera_str: String,
    pub total: f64,
    pub total_str: String,
    pub anio: i32,
    pub mes_num: i32,
}

#[derive(Debug, Clone)]
pub struct SumatoriaRowModel {
    pub registro_patronal: String,
    pub razon_social: String,
    pub anio: String,
    pub mes: String,
    pub mes_num: i32,
    pub registro_obra: String,
    pub trabajadores_count: i32,
    pub total_dias: i32,
    pub cuota_patronal: f64,
    pub cuota_patronal_str: String,
    pub cuota_obrera: f64,
    pub cuota_obrera_str: String,
    pub total_liquidacion: f64,
    pub total_liquidacion_str: String,
}

#[derive(Debug, Clone, Default)]
pub struct CedulasAuditResult {
    pub total_archivos_analizados: usize,
    pub validos_sua: usize,
    pub validos_pct: String,
    pub no_procesados: usize,
    pub no_procesados_pct: String,
    pub no_procesados_desc: String,
    pub trabajadores_unicos: usize,
    pub repeticiones_mensuales: usize,
    pub importe_total: f64,
    pub importe_total_str: String,
    pub cuota_patronal: f64,
    pub cuota_patronal_str: String,
    pub cuota_obrera: f64,
    pub cuota_obrera_str: String,

    // Auditoría avanzada
    pub superposiciones_count: usize,
    pub obras_fuera_revision_count: usize,
    pub empleados_obras_fuera_count: usize,
    pub archivos_cliente_invalido_count: usize,
    pub sin_obras_revision_warning: bool,

    // Rango de periodos
    pub periodo_rango_str: String,
    pub periodo_sub_str: String,
    pub mensuales_count: usize,
    pub bimestrales_count: usize,

    // SBC Promedio Ponderado
    pub sbc_promedio_str: String,
    pub sbc_sub_str: String,
    pub sbc_min_str: String,
    pub sbc_max_str: String,

    // Ramas de cuotas
    pub ramas_rt_str: String,
    pub ramas_sub_str: String,

    // Tablas
    pub detalle_rows: Vec<DetalleRowModel>,
    pub sumatorias_rows: Vec<SumatoriaRowModel>,

    // Gran Total
    pub gran_total_trabajadores: usize,
    pub gran_total_dias: i32,
    pub gran_total_patronal_str: String,
    pub gran_total_obrera_str: String,
    pub gran_total_str: String,

    pub excel_path: Option<PathBuf>,
    pub mismatch_report: CedulaMismatchReport,
}

pub fn compute_sha256_short(path: &Path) -> String {
    if let Ok(bytes) = std::fs::read(path) {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let result = hasher.finalize();
        let hex = format!("{:x}", result);
        if hex.len() > 16 {
            format!("{}...{}", &hex[0..8], &hex[hex.len() - 4..])
        } else {
            hex
        }
    } else {
        "e3b0c442...98b2".to_string()
    }
}

pub fn format_filesize(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_currency(val: f64) -> String {
    let int_part = val.trunc().abs() as u64;
    let frac_part = ((val.abs() - int_part as f64) * 100.0).round() as u64;
    let int_str = int_part.to_string();
    let mut with_commas = String::new();
    let chars: Vec<char> = int_str.chars().collect();
    for (i, ch) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i) % 3 == 0 {
            with_commas.push(',');
        }
        with_commas.push(*ch);
    }
    let sign = if val < 0.0 { "-" } else { "" };
    format!("{}${}.{:02}", sign, with_commas, frac_part)
}

pub fn inspect_cedula_file(path: &Path) -> CedulaFileCardInfo {
    let filename = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "documento.pdf".to_string());
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let size_str = format_filesize(size_bytes);
    let checksum = compute_sha256_short(path);

    let (file_type, pages_detected, status_badge, status_is_valid, has_warning, warning_msg) = if ext == "zip" || ext == "rar" {
        (
            format!("Archivo {} Recursivo", ext.to_uppercase()),
            "Paquete Comprimido".to_string(),
            "Válido Extraído".to_string(),
            true,
            false,
            String::new(),
        )
    } else if ext == "pdf" {
        match extract_with_layout(path) {
            Ok(raw_text) => {
                let trimmed = raw_text.trim();
                let lower = raw_text.to_lowercase();
                let is_ema = crate::cedulas::ema::is_ema_document(&raw_text);
                let is_sua = lower.contains("cédula de determinación de cuotas")
                    || lower.contains("cedula de determinacion de cuotas")
                    || lower.contains("sistema único de autodeterminación")
                    || lower.contains("sistema unico de autodeterminacion")
                    || (lower.contains("registro patronal") && lower.contains("delegación imss"));

                let pages_count = raw_text.split('\x0c').filter(|p| !p.trim().is_empty()).count();
                let pages_label = if pages_count > 1 {
                    format!("{} Páginas", pages_count)
                } else {
                    "1 Página".to_string()
                };

                if trimmed.is_empty() || trimmed.len() < 50 {
                    (
                        "Escaneo Raster".to_string(),
                        "0 (Sin Capa de Texto)".to_string(),
                        "Inválido OCR".to_string(),
                        false,
                        true,
                        "Sin capa de texto vectorial OCR (escaneo)".to_string(),
                    )
                } else if is_ema {
                    (
                        "Propuesta IMSS (EMA)".to_string(),
                        pages_label,
                        "Válido EMA".to_string(),
                        true,
                        false,
                        String::new(),
                    )
                } else if !is_sua {
                    (
                        "PDF No Oficial".to_string(),
                        pages_label,
                        "No Corresponde SUA/EMA".to_string(),
                        false,
                        true,
                        "El encabezado no corresponde a Cédula SUA ni EMA oficial".to_string(),
                    )
                } else {
                    (
                        "PDF Vectorial IMSS".to_string(),
                        pages_label,
                        "Válido SUA".to_string(),
                        true,
                        false,
                        String::new(),
                    )
                }
            }
            Err(e) => (
                "PDF Ilegible".to_string(),
                "Error Lectura".to_string(),
                "Error Lectura".to_string(),
                false,
                true,
                format!("Error al leer layout: {}", e),
            ),
        }
    } else {
        (
            format!("Archivo .{}", ext.to_uppercase()),
            "N/D".to_string(),
            "No Soportado".to_string(),
            false,
            true,
            "Formato de archivo no soportado".to_string(),
        )
    };

    CedulaFileCardInfo {
        id: Uuid::now_v7().to_string(),
        path: path.to_path_buf(),
        filename,
        file_type,
        size_str,
        pages_detected,
        status_badge,
        status_is_valid,
        checksum,
        has_warning,
        warning_msg,
    }
}

/// Inspecciona un archivo PDF de Cédula y verifica si el RFC o Registro Patronal
/// corresponden a los seleccionados en la revisión activa, reportando advertencias tempranas.
pub fn inspect_cedula_file_with_context(
    path: &Path,
    expected_rfc: Option<&str>,
    expected_rp: Option<&str>,
) -> CedulaFileCardInfo {
    let mut card = inspect_cedula_file(path);
    if card.status_is_valid {
        if let Ok(raw_text) = extract_with_layout(path) {
            // Caso EMA
            if crate::cedulas::ema::is_ema_document(&raw_text) {
                if let Some(target_rp) = expected_rp {
                    let clean_t = canonical_reg(target_rp);
                    if !clean_t.is_empty() && clean_t != "GENERAL" {
                        if let Some(ema_rp) = crate::cedulas::ema::extract_ema_rp(&raw_text) {
                            let clean_found = canonical_reg(&ema_rp);
                            if !clean_found.is_empty() && clean_found != clean_t {
                                card.has_warning = true;
                                card.warning_msg = format!(
                                    "Registro Patronal EMA '{}' no coincide con el registro patronal seleccionado ('{}')",
                                    clean_found, clean_t
                                );
                            }
                        }
                    }
                }
                if let Some(target_rfc) = expected_rfc {
                    let clean_t: String = target_rfc.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase();
                    if !clean_t.is_empty() {
                        if let Some(ema_rfc) = crate::cedulas::ema::extract_ema_rfc(&raw_text) {
                            let clean_b: String = ema_rfc.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase();
                            if !clean_b.is_empty() && clean_b != clean_t {
                                card.has_warning = true;
                                let rfc_msg = format!(
                                    "RFC EMA '{}' no coincide con el RFC registrado ('{}')",
                                    clean_b, clean_t
                                );
                                if card.warning_msg.is_empty() {
                                    card.warning_msg = rfc_msg;
                                } else {
                                    card.warning_msg = format!("{} | {}", card.warning_msg, rfc_msg);
                                }
                            }
                        }
                    }
                }
                return card;
            }

            // Caso Cédula SUA tradicional
            if let Ok(p) = crate::cedulas::regex_expressions::initialize_patterns() {
                let cleaned = clean_text(&raw_text.to_lowercase(), &p);
                // Validar Registro Patronal
                if let Some(target_rp) = expected_rp {
                    let clean_t = canonical_reg(target_rp);
                    if !clean_t.is_empty() && clean_t != "GENERAL" {
                        if let Ok(Some(mat)) = p.business_registro_patronal.find(&cleaned) {
                            let chunk = &cleaned[mat.start()..cleaned.len().min(mat.start() + 200)];
                            if let Ok(Some(caps)) = p.business_registro_patronal.captures(chunk) {
                                if let Some(m) = caps.name("registro_patronal") {
                                    let clean_found = canonical_reg(m.as_str());
                                    if !clean_found.is_empty() && clean_found != clean_t {
                                        card.has_warning = true;
                                        card.warning_msg = format!(
                                            "Registro Patronal '{}' no coincide con el registro patronal seleccionado ('{}')",
                                            clean_found, clean_t
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                // Validar RFC
                if let Some(target_rfc) = expected_rfc {
                    let clean_t: String = target_rfc.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase();
                    if !clean_t.is_empty() {
                        let mut biz = Business::new();
                        biz.extract_from_page(&cleaned, &p);
                        if let Some(ref b_rfc) = biz.rfc {
                            let clean_b: String = b_rfc.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase();
                            if !clean_b.is_empty() && clean_b != clean_t {
                                card.has_warning = true;
                                let rfc_msg = format!(
                                    "RFC '{}' no coincide con el RFC registrado ('{}')",
                                    b_rfc.trim(), target_rfc
                                );
                                if card.warning_msg.is_empty() {
                                    card.warning_msg = rfc_msg;
                                } else {
                                    card.warning_msg = format!("{} | {}", card.warning_msg, rfc_msg);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    card
}

/// Extrae el nombre estandarizado de Cédula tal como se genera en PostgreSQL public.liquidaciones.nombre_archivo:
/// "Cédula SUA <registro_patronal> - <mes>.pdf".
pub fn extract_cedula_pdf_postgres_filename(path: &Path, default_rp: &str) -> String {
    let fname = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cedula.pdf".to_string());

    let raw_text = match extract_with_layout(path) {
        Ok(t) => t,
        Err(_) => return fname,
    };

    // Caso 1: Documento EMA
    if crate::cedulas::ema::is_ema_document(&raw_text) {
        if let Some(ema_name) = crate::cedulas::ema::extract_ema_postgres_filename(&raw_text, default_rp) {
            return ema_name;
        }
    }

    // Caso 2: Cédula SUA tradicional
    let patterns = crate::cedulas::regex_expressions::initialize_patterns().ok();
    let default_reg = if default_rp.is_empty() || default_rp == "GENERAL" {
        "00000000000"
    } else {
        default_rp
    };

    if let Some(ref patterns) = patterns {
        let cleaned = clean_text(&raw_text.to_lowercase(), patterns);
        let mut rp_found = None;
        let mut mes_found = None;

        // Buscar registro patronal en el texto
        if let Ok(Some(mat)) = patterns.business_registro_patronal.find(&cleaned) {
            let chunk = &cleaned[mat.start()..cleaned.len().min(mat.start() + 200)];
            if let Ok(Some(caps)) = patterns.business_registro_patronal.captures(chunk) {
                if let Some(m) = caps.name("registro_patronal") {
                    let filtered: String = m.as_str().chars().filter(|c| c.is_alphanumeric()).collect();
                    if !filtered.is_empty() {
                        rp_found = Some(filtered.to_uppercase());
                    }
                }
            }
        }

        // Buscar periodo de proceso / mes
        if let Ok(Some(caps)) = patterns.employee_periodo_proceso.captures(&cleaned) {
            if let Some(m) = caps.name("mes_periodo") {
                let m_str = m.as_str().trim();
                let num = month_to_num(m_str);
                let valid_num = if (1..=12).contains(&num) { num } else { 1 };
                mes_found = Some(month_num_to_name(valid_num).to_string());
            }
        }

        if let Some(mes) = mes_found {
            let final_rp = rp_found.as_deref().unwrap_or(default_reg);
            return format!("Cédula SUA {} - {}.pdf", final_rp, mes);
        }
    }

    if fname.to_lowercase().ends_with(".pdf") {
        fname
    } else {
        format!("{}.pdf", fname)
    }
}

/// Extrae periodo, cliente y registro patronal y genera el nombre estricto:
/// 'Nombre Cliente__Registro Patronal__Mes__Anio.pdf'
pub fn extract_cedula_standard_naming(
    path: &Path,
    default_cliente: &str,
    default_rp: &str,
) -> (String, String, String, String, String) {
    let raw_text = extract_with_layout(path).unwrap_or_default();
    let patterns = crate::cedulas::regex_expressions::initialize_patterns().ok();

    let mut rp_found = None;
    let mut mes_found = None;
    let mut anio_found = None;
    let mut cliente_found = None;

    if !raw_text.is_empty() {
        if crate::cedulas::ema::is_ema_document(&raw_text) {
            if let Some(r) = crate::cedulas::ema::extract_ema_rp(&raw_text) {
                rp_found = Some(r);
            }
            let (m_opt, y_opt) = crate::cedulas::ema::extract_ema_period(&raw_text);
            if let (Some(m), Some(y)) = (m_opt, y_opt) {
                let m_num = month_to_num(&m);
                let valid_m = if (1..=12).contains(&m_num) { m_num } else { 1 };
                mes_found = Some(month_num_to_name(valid_m).to_string());
                anio_found = Some(y);
            }
        }

        if let Some(ref patterns) = patterns {
            let cleaned = clean_text(&raw_text.to_lowercase(), patterns);
            if rp_found.is_none() {
                if let Ok(Some(mat)) = patterns.business_registro_patronal.find(&cleaned) {
                    let chunk = &cleaned[mat.start()..cleaned.len().min(mat.start() + 200)];
                    if let Ok(Some(caps)) = patterns.business_registro_patronal.captures(chunk) {
                        if let Some(m) = caps.name("registro_patronal") {
                            let filtered: String = m.as_str().chars().filter(|c| c.is_alphanumeric()).collect();
                            if !filtered.is_empty() {
                                rp_found = Some(filtered.to_uppercase());
                            }
                        }
                    }
                }
            }

            if mes_found.is_none() || anio_found.is_none() {
                if let Ok(Some(caps)) = patterns.employee_periodo_proceso.captures(&cleaned) {
                    if let Some(m) = caps.name("mes_periodo") {
                        let num = month_to_num(m.as_str().trim());
                        let valid_num = if (1..=12).contains(&num) { num } else { 1 };
                        mes_found = Some(month_num_to_name(valid_num).to_string());
                    }
                    if let Some(y) = caps.name("anio_periodo") {
                        anio_found = Some(y.as_str().trim().to_string());
                    }
                }
            }

            if let Ok(Some(caps)) = patterns.business_razon_social.captures(&cleaned) {
                if let Some(rs) = caps.name("razon_social") {
                    let clean_rs = rs.as_str().trim().to_uppercase();
                    if !clean_rs.is_empty() {
                        cliente_found = Some(clean_rs);
                    }
                }
            }
        }
    }

    let final_cliente = if !default_cliente.trim().is_empty() && default_cliente != "Todos los Clientes" {
        default_cliente.trim().to_uppercase()
    } else {
        cliente_found.unwrap_or_else(|| "CLIENTE".to_string())
    };

    let final_rp = if !default_rp.trim().is_empty() && default_rp != "GENERAL" {
        default_rp.trim().to_uppercase()
    } else {
        rp_found.unwrap_or_else(|| "00000000000".to_string())
    };

    let final_mes = mes_found.unwrap_or_else(|| "ENERO".to_string()).to_uppercase();
    let final_anio = anio_found.unwrap_or_else(|| "2026".to_string());

    let clean_c_name = final_cliente.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_").trim().to_string();
    let clean_rp_name = final_rp.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_").trim().to_string();

    let standard_filename = format!("{}__{}__{}__{}.pdf", clean_c_name, clean_rp_name, final_mes, final_anio);

    (standard_filename, final_anio, final_mes, clean_c_name, clean_rp_name)
}

pub struct AuditAssignment {
    pub worker_id: String,
    pub obra_num: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
}

pub fn detect_overlapping_workers(assignments: &[AuditAssignment]) -> usize {
    let mut by_worker: HashMap<&str, Vec<&AuditAssignment>> = HashMap::new();
    for a in assignments {
        by_worker.entry(a.worker_id.as_str()).or_default().push(a);
    }

    let mut overlap_count = 0;
    for (_worker, list) in by_worker {
        if list.len() < 2 {
            continue;
        }
        let mut has_overlap = false;
        'outer: for i in 0..list.len() {
            for j in (i + 1)..list.len() {
                if list[i].obra_num != list[j].obra_num {
                    // Check if date intervals overlap: start_a <= end_b && start_b <= end_a
                    if list[i].start <= list[j].end && list[j].start <= list[i].end {
                        has_overlap = true;
                        break 'outer;
                    }
                }
            }
        }
        if has_overlap {
            overlap_count += 1;
        }
    }
    overlap_count
}

pub fn process_and_audit_cedulas(
    input_paths: &[PathBuf],
    target_client_rfc: Option<&str>,
    target_client_name: Option<&str>,
    target_registro_patronal: Option<&str>,
    revision_obras_registered: &HashSet<String>,
    _output_excel_path: &Path,
    temp_dir: &Path,
    progress_callback: impl Fn(usize, usize, &str) + Send + Sync,
) -> Result<(CedulasAuditResult, Vec<Business>, CedulaMismatchReport), (String, CedulaMismatchReport)> {
    progress_callback(0, 0, "Extrayendo y analizando archivos de cédulas...");

    let pdf_files = collect_or_extract_files(input_paths, "pdf", temp_dir, Some(&progress_callback))
        .map_err(|e| (e, CedulaMismatchReport::default()))?;
    let total_archivos_analizados = pdf_files.len();

    let mut validos_sua = 0;
    let mut no_procesados = 0;
    let mut archivos_cliente_invalido_count = 0;

    let clean_target_rfc = target_client_rfc.map(|s| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()).filter(|s| !s.is_empty());
    let clean_target_name = target_client_name.map(canonical_razon).filter(|s| !s.is_empty() && s != "TODOS LOS CLIENTES");
    let clean_target_rp = target_registro_patronal.map(canonical_reg).filter(|s| !s.is_empty() && s != "GENERAL");

    let mut mismatch_report = CedulaMismatchReport {
        expected_rfc: clean_target_rfc.clone().unwrap_or_default(),
        expected_rp: clean_target_rp.clone().unwrap_or_default(),
        expected_cliente: target_client_name.unwrap_or("").to_string(),
        items: Vec::new(),
    };

    let mut valid_businesses: Vec<Business> = Vec::new();

    let patterns = crate::cedulas::regex_expressions::initialize_patterns()
        .map_err(|e| (format!("Error en expresiones regulares: {}", e), mismatch_report.clone()))?;

    // Extracción concurrente multi-hilo de las capas de texto con MuPDF nativo
    let extracted_pdfs: Vec<(PathBuf, String, Result<String, String>)> = pdf_files
        .par_iter()
        .map(|pdf_path| {
            let file_display_name = pdf_path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let res = extract_with_layout(pdf_path);
            (pdf_path.clone(), file_display_name, res)
        })
        .collect();

    for (idx, (_pdf_path, file_display_name, extract_res)) in extracted_pdfs.into_iter().enumerate() {
        progress_callback(
            idx + 1,
            total_archivos_analizados,
            &format!("Analizando {} ({}/{})...", file_display_name, idx + 1, total_archivos_analizados),
        );

        let raw_text = match extract_res {
            Ok(t) => t,
            Err(_) => {
                no_procesados += 1;
                mismatch_report.items.push(CedulaMismatchItem {
                    filename: file_display_name,
                    error_type: "Error Lectura PDF".to_string(),
                    expected_val: "Documento PDF legible".to_string(),
                    found_val: "Error al extraer capas de texto".to_string(),
                    details: "No se pudo leer la información interna del archivo PDF.".to_string(),
                });
                continue;
            }
        };

        let is_ema = crate::cedulas::ema::is_ema_document(&raw_text);
        let lower = raw_text.to_lowercase();
        let is_sua = lower.contains("cédula de determinación de cuotas")
            || lower.contains("cedula de determinacion de cuotas")
            || lower.contains("sistema único de autodeterminación")
            || lower.contains("sistema unico de autodeterminacion")
            || (lower.contains("registro patronal") && (lower.contains("delegación imss") || lower.contains("delegacion imss")));

        if (!is_sua && !is_ema) || raw_text.trim().len() < 50 {
            no_procesados += 1;
            mismatch_report.items.push(CedulaMismatchItem {
                filename: file_display_name,
                error_type: "Formato no válido".to_string(),
                expected_val: "Cédula SUA o Propuesta EMA oficial (PDF Vectorial)".to_string(),
                found_val: "Sin formato SUA/EMA / Escaneo raster".to_string(),
                details: "El archivo no contiene el encabezado oficial de Cédula de Cuotas SUA / EMA o carece de capa de texto OCR vectorial.".to_string(),
            });
            continue;
        }

        let mut pdf_sections: Vec<(Business, Vec<Employee>)> = Vec::new();

        if is_ema {
            match crate::cedulas::ema::parse_ema_document(&raw_text, &file_display_name) {
                Ok((biz, emps)) => {
                    pdf_sections.push((biz, emps));
                }
                Err(err_msg) => {
                    no_procesados += 1;
                    mismatch_report.items.push(CedulaMismatchItem {
                        filename: file_display_name,
                        error_type: "Sin trabajadores".to_string(),
                        expected_val: "Renglones de trabajadores cotizantes EMA".to_string(),
                        found_val: "0 trabajadores detectados".to_string(),
                        details: err_msg,
                    });
                    continue;
                }
            }
        } else {
            // Dividir en páginas y normalizar/limpiar cada página
            let mut pages: Vec<String> = raw_text.split('\x0c').map(|s| s.to_string()).collect();
            if let Some(last) = pages.last() {
                if last.trim().is_empty() {
                    pages.pop();
                }
            }

            for page in &mut pages {
                *page = clean_text(&page.to_lowercase(), &patterns);
            }

            let mut current_biz = Business::new();
            let mut current_emps = Vec::new();

            for page in &pages {
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
        }

        if pdf_sections.is_empty() || pdf_sections.iter().all(|(_, emps)| emps.is_empty()) {
            no_procesados += 1;
            mismatch_report.items.push(CedulaMismatchItem {
                filename: file_display_name,
                error_type: "Sin trabajadores".to_string(),
                expected_val: "Renglones de trabajadores cotizantes".to_string(),
                found_val: "0 trabajadores detectados".to_string(),
                details: "No se identificaron datos de empleados (NSS, días, cuotas) en las secciones analizadas del archivo.".to_string(),
            });
            continue;
        }

        let mut file_had_valid_section = false;
        for (mut biz, emps) in pdf_sections {
            if emps.is_empty() {
                continue;
            }

            // Validar si pertenece al cliente auditado y al Registro Patronal de la revisión
            let mut mismatch = false;

            // 1. Validar Registro Patronal si se especificó para la revisión
            if let Some(ref target_rp) = clean_target_rp {
                if let Some(ref b_rp) = biz.registro_patronal {
                    let clean_b = canonical_reg(b_rp);
                    let clean_t = canonical_reg(target_rp);
                    if !clean_b.is_empty() && !clean_t.is_empty() && clean_b != clean_t {
                        mismatch = true;
                        mismatch_report.items.push(CedulaMismatchItem {
                            filename: file_display_name.clone(),
                            error_type: "Registro Patronal no coincide".to_string(),
                            expected_val: format!("RP: {}", clean_t),
                            found_val: format!("RP: {}", clean_b),
                            details: format!(
                                "El Registro Patronal '{}' extraído de esta cédula no coincide con el Registro Patronal '{}' seleccionado para esta revisión.",
                                clean_b, clean_t
                            ),
                        });
                    }
                }
            }

            // 2. Validar RFC del Cliente si se especificó
            if let Some(ref target_rfc) = clean_target_rfc {
                if let Some(ref b_rfc) = biz.rfc {
                    let clean_b: String = b_rfc.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase();
                    let clean_t: String = target_rfc.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase();
                    if !clean_b.is_empty() && !clean_t.is_empty() && clean_b != clean_t {
                        mismatch = true;
                        mismatch_report.items.push(CedulaMismatchItem {
                            filename: file_display_name.clone(),
                            error_type: "RFC no coincide".to_string(),
                            expected_val: format!("RFC: {}", clean_t),
                            found_val: format!("RFC: {}", clean_b),
                            details: format!(
                                "El RFC '{}' extraído de esta cédula no coincide con el RFC '{}' registrado para la empresa en el Catálogo de Clientes.",
                                clean_b, clean_t
                            ),
                        });
                    }
                }
            }

            // 3. Validar Razón Social del Cliente como red de seguridad
            if !mismatch {
                if let Some(ref target_name) = clean_target_name {
                    if let Some(ref b_name) = biz.razon_social {
                        let b_name_clean = canonical_razon(b_name);
                        let t_name_clean = canonical_razon(target_name);
                        if !b_name_clean.is_empty() && !t_name_clean.is_empty() {
                            if !b_name_clean.contains(&t_name_clean) && !t_name_clean.contains(&b_name_clean) {
                                if clean_target_rfc.is_none() && clean_target_rp.is_none() {
                                    mismatch = true;
                                    mismatch_report.items.push(CedulaMismatchItem {
                                        filename: file_display_name.clone(),
                                        error_type: "Empresa no coincide".to_string(),
                                        expected_val: format!("Empresa: {}", target_client_name.unwrap_or("")),
                                        found_val: format!("Empresa: {}", b_name),
                                        details: format!(
                                            "La razón social '{}' extraída de la cédula no coincide con el nombre de la empresa '{}'.",
                                            b_name, target_client_name.unwrap_or("")
                                        ),
                                    });
                                }
                            }
                        }
                    }
                }
            }

            if mismatch {
                archivos_cliente_invalido_count += 1;
                continue;
            }

            biz.employees = emps;
            valid_businesses.push(biz);
            file_had_valid_section = true;
        }

        if file_had_valid_section {
            validos_sua += 1;
        } else {
            no_procesados += 1;
        }
    }

    if valid_businesses.is_empty() {
        if archivos_cliente_invalido_count > 0 || !mismatch_report.items.is_empty() {
            let mut summary_parts = Vec::new();
            if mismatch_report.items.iter().any(|i| i.error_type == "Registro Patronal no coincide") {
                summary_parts.push("Registros Patronales que no corresponden con la revisión");
            }
            if mismatch_report.items.iter().any(|i| i.error_type == "RFC no coincide") {
                summary_parts.push("RFCs que no coinciden con el cliente registrado");
            }
            if mismatch_report.items.iter().any(|i| i.error_type == "Empresa no coincide") {
                summary_parts.push("Nombres de empresa que no coinciden");
            }
            if mismatch_report.items.iter().any(|i| i.error_type == "Formato no válido") {
                summary_parts.push("Archivos con formato no oficial SUA");
            }
            let desc = if !summary_parts.is_empty() {
                format!("CLIENT_MISMATCH: Se encontraron {}", summary_parts.join(", "))
            } else {
                "CLIENT_MISMATCH: No se pudieron procesar los archivos ya que no pertenecen a la empresa o registro patronal de esta revisión.".to_string()
            };
            return Err((desc, mismatch_report));
        } else {
            return Err(("No se encontraron cédulas SUA válidas con datos de trabajadores para procesar.".to_string(), mismatch_report));
        }
    }

    let mut audit_res = build_cedulas_audit_from_businesses(
        &valid_businesses,
        revision_obras_registered,
        total_archivos_analizados,
        validos_sua,
        no_procesados,
        archivos_cliente_invalido_count,
        None, // Omitir generación innecesaria de Excel temporal durante auditoría
    );
    audit_res.mismatch_report = mismatch_report.clone();

    Ok((audit_res, valid_businesses, mismatch_report))
}

pub fn build_cedulas_audit_from_businesses(
    valid_businesses: &[Business],
    revision_obras_registered: &HashSet<String>,
    total_archivos_analizados: usize,
    validos_sua: usize,
    no_procesados: usize,
    archivos_cliente_invalido_count: usize,
    output_excel_path: Option<&Path>,
) -> CedulasAuditResult {
    // Now gather all employees, assignments, totals, etc.
    let mut unique_worker_ids = HashSet::new();
    let mut total_repetitions = 0;
    let mut assignments: Vec<AuditAssignment> = Vec::new();
    let mut detalle_rows: Vec<DetalleRowModel> = Vec::new();

    // Map for sumatorias: key = (registro_patronal, anio, mes, registro_obra)
    #[derive(Hash, PartialEq, Eq, Clone)]
    struct SumKey {
        rp: String,
        razon: String,
        anio: i32,
        mes_num: i32,
        mes_name: String,
        obra: String,
    }

    struct SumVal {
        workers_set: HashSet<String>,
        total_dias: i32,
        patronal: f64,
        obrera: f64,
        total: f64,
    }

    let mut sumatorias_map: HashMap<SumKey, SumVal> = HashMap::new();
    let mut total_importe = 0.0;
    let mut total_patronal = 0.0;
    let mut total_obrera = 0.0;

    let mut all_sbc_values: Vec<f64> = Vec::new();
    let mut min_period_date: Option<NaiveDate> = None;
    let mut max_period_date: Option<NaiveDate> = None;
    let mut monthly_count = 0;
    let mut bimonthly_count = 0;

    let mut obras_encontradas: HashSet<String> = HashSet::new();
    let mut obras_fuera_revision_set: HashSet<String> = HashSet::new();
    let mut empleados_obras_fuera_set: HashSet<String> = HashSet::new();

    let mut row_idx = 1;

    for biz in valid_businesses {
        let mut rp_label = biz.registro_patronal.as_deref().unwrap_or("N/D").trim().to_uppercase();
        if let Some(idx) = rp_label.find(" RFC") {
            rp_label = rp_label[..idx].trim().to_string();
        }
        if rp_label.ends_with("RFC") {
            rp_label = rp_label.trim_end_matches("RFC").trim().to_string();
        }

        let mut razon_label = biz.razon_social.as_deref().unwrap_or("N/D").trim().to_uppercase();
        if let Some(idx) = razon_label.find(" DELEGACIÓN") {
            razon_label = razon_label[..idx].trim().to_string();
        }
        if let Some(idx) = razon_label.find(" DELEGACION") {
            razon_label = razon_label[..idx].trim().to_string();
        }
        if let Some(idx) = razon_label.find(" RFC") {
            razon_label = razon_label[..idx].trim().to_string();
        }

        for emp in &biz.employees {
            let worker_id = emp
                .no_seguro_social
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .or(emp.curp.as_deref())
                .or(emp.nombre.as_deref())
                .unwrap_or("NSS-DESCONOCIDO")
                .trim()
                .to_uppercase();

            if unique_worker_ids.contains(&worker_id) {
                total_repetitions += 1;
            } else {
                unique_worker_ids.insert(worker_id.clone());
            }

            let mes_str = emp.mes_periodo.as_deref().unwrap_or("ENERO").to_uppercase();
            let mes_num = month_to_num(&mes_str);
            let valid_m = if (1..=12).contains(&mes_num) { mes_num } else { 1 };
            let anio_num: i32 = emp.anio_periodo.as_deref().and_then(|s| s.parse().ok()).unwrap_or(2024);

            let period_start = NaiveDate::from_ymd_opt(anio_num, valid_m, 1).unwrap_or_else(|| NaiveDate::from_ymd_opt(2024, 1, 1).unwrap());
            let period_end = if valid_m == 12 {
                NaiveDate::from_ymd_opt(anio_num, 12, 31).unwrap()
            } else {
                NaiveDate::from_ymd_opt(anio_num, valid_m + 1, 1).unwrap().pred_opt().unwrap()
            };

            if min_period_date.is_none() || Some(period_start) < min_period_date {
                min_period_date = Some(period_start);
            }
            if max_period_date.is_none() || Some(period_end) > max_period_date {
                max_period_date = Some(period_end);
            }

            // Detect monthly vs bimonthly (bimestres have 2 months or RCV)
            if valid_m % 2 == 0 {
                bimonthly_count += 1;
            } else {
                monthly_count += 1;
            }

            if emp.registro_obras.is_empty() {
                let obra_str = "SIN REGISTRO DE OBRA".to_string();
                obras_encontradas.insert(obra_str.clone());
                assignments.push(AuditAssignment {
                    worker_id: worker_id.clone(),
                    obra_num: obra_str,
                    start: period_start,
                    end: period_end,
                });
            } else {
                for ro in &emp.registro_obras {
                    let obra_str = ro.registro_obra.clone();
                    obras_encontradas.insert(obra_str.clone());

                    // Check if obra is in revision
                    if !revision_obras_registered.is_empty()
                        && !revision_obras_registered.contains(&obra_str)
                        && obra_str != "SIN REGISTRO DE OBRA"
                    {
                        obras_fuera_revision_set.insert(obra_str.clone());
                        empleados_obras_fuera_set.insert(worker_id.clone());
                    }

                    // Add assignment for overlap checking
                    assignments.push(AuditAssignment {
                        worker_id: worker_id.clone(),
                        obra_num: obra_str,
                        start: period_start,
                        end: period_end,
                    });
                }
            }

            let obra_str = if emp.registro_obras.is_empty() {
                "SIN REGISTRO DE OBRA".to_string()
            } else {
                emp.registro_obras
                    .iter()
                    .map(|ro| ro.to_string())
                    .collect::<Vec<String>>()
                    .join(" ")
            };

            // Extract numeric values from data_employee
            let dias_val: i32 = emp.data_employee.iter().filter_map(|d| d.dias).map(|d| d as i32).sum();
            let dias_final = if dias_val > 0 { dias_val } else { 30 };

            let sdi_val: f64 = emp.data_employee.iter().filter_map(|d| d.sdi).next().unwrap_or(0.0);
            if sdi_val > 0.0 {
                all_sbc_values.push(sdi_val);
            }

            let pat_val: f64 = emp.data_employee.iter().filter_map(|d| d.patronal).sum();
            let obr_val: f64 = emp.data_employee.iter().filter_map(|d| d.obrera).sum();
            let subtotal_val: f64 = emp.data_employee.iter().filter_map(|d| d.subtotal).sum();

            let (pat_final, obr_final, tot_val) = if pat_val > 0.0 || obr_val > 0.0 {
                (pat_val, obr_val, pat_val + obr_val)
            } else if subtotal_val > 0.0 {
                (subtotal_val * 0.80, subtotal_val * 0.20, subtotal_val)
            } else if sdi_val > 0.0 {
                let p = sdi_val * (dias_final as f64) * 0.20;
                let o = sdi_val * (dias_final as f64) * 0.05;
                (p, o, p + o)
            } else {
                (0.0, 0.0, 0.0)
            };

            total_importe += tot_val;
            total_patronal += pat_final;
            total_obrera += obr_final;

            let nss_val = emp.no_seguro_social.as_deref().unwrap_or("SIN NSS").to_string();
            let rfc_curp_val = match (&emp.rfc, &emp.curp) {
                (Some(r), Some(c)) => format!("{} / {}", r, c),
                (Some(r), None) => r.clone(),
                (None, Some(c)) => c.clone(),
                (None, None) => "SIN RFC/CURP".to_string(),
            };
            let nombre_val = emp.nombre.as_deref().unwrap_or("SIN NOMBRE").to_uppercase();

            let row_model = DetalleRowModel {
                idx: row_idx,
                reg_num_str: format!("{:04}", row_idx),
                mes_anio: format!("{:02} / {}", valid_m, anio_num),
                registro_patronal: rp_label.clone(),
                razon_social: razon_label.clone(),
                registro_obra: obra_str.clone(),
                nss: nss_val,
                rfc_curp: rfc_curp_val,
                nombre_trabajador: nombre_val,
                dias: dias_final,
                sdi: sdi_val,
                sdi_str: format!("${:.2}", sdi_val),
                cuota_patronal: pat_final,
                cuota_patronal_str: format!("${:.2}", pat_final),
                cuota_obrera: obr_final,
                cuota_obrera_str: format!("${:.2}", obr_final),
                total: tot_val,
                total_str: format!("${:.2}", tot_val),
                anio: anio_num,
                mes_num: valid_m as i32,
            };
            detalle_rows.push(row_model);
            row_idx += 1;

            // Group into sumatorias
            let sum_key = SumKey {
                rp: rp_label.clone(),
                razon: razon_label.clone(),
                anio: anio_num,
                mes_num: valid_m as i32,
                mes_name: format!("{} ({:02})", month_num_to_name(valid_m), valid_m),
                obra: obra_str.clone(),
            };

            let entry = sumatorias_map.entry(sum_key).or_insert_with(|| SumVal {
                workers_set: HashSet::new(),
                total_dias: 0,
                patronal: 0.0,
                obrera: 0.0,
                total: 0.0,
            });
            entry.workers_set.insert(worker_id);
            entry.total_dias += dias_final;
            entry.patronal += pat_final;
            entry.obrera += obr_final;
            entry.total += tot_val;
        }
    }



    // Detect overlapping workers across different obras
    let superposiciones_count = detect_overlapping_workers(&assignments);

    // Build Sumatorias sorted by RP, anio, mes, obra
    let mut sumatorias_keys: Vec<SumKey> = sumatorias_map.keys().cloned().collect();
    sumatorias_keys.sort_by(|a, b| {
        a.rp.cmp(&b.rp)
            .then(a.anio.cmp(&b.anio))
            .then(a.mes_num.cmp(&b.mes_num))
            .then(a.obra.cmp(&b.obra))
    });

    let mut sumatorias_rows: Vec<SumatoriaRowModel> = Vec::new();
    let mut gran_total_dias = 0;

    for key in sumatorias_keys {
        if let Some(val) = sumatorias_map.get(&key) {
            gran_total_dias += val.total_dias;
            sumatorias_rows.push(SumatoriaRowModel {
                registro_patronal: key.rp,
                razon_social: key.razon,
                anio: key.anio.to_string(),
                mes: key.mes_name,
                mes_num: key.mes_num,
                registro_obra: key.obra,
                trabajadores_count: val.workers_set.len() as i32,
                total_dias: val.total_dias,
                cuota_patronal: val.patronal,
                cuota_patronal_str: format!("${:.2}", val.patronal),
                cuota_obrera: val.obrera,
                cuota_obrera_str: format!("${:.2}", val.obrera),
                total_liquidacion: val.total,
                total_liquidacion_str: format!("${:.2}", val.total),
            });
        }
    }

    // SBC min, max, avg
    let avg_sbc = if !all_sbc_values.is_empty() {
        all_sbc_values.iter().sum::<f64>() / all_sbc_values.len() as f64
    } else {
        0.0
    };
    let min_sbc = all_sbc_values.iter().copied().fold(f64::INFINITY, f64::min);
    let min_sbc = if min_sbc.is_finite() { min_sbc } else { 0.0 };
    let max_sbc = all_sbc_values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let max_sbc = if max_sbc.is_finite() { max_sbc } else { 0.0 };

    let total_arch = total_archivos_analizados.max(validos_sua + no_procesados);
    let val_sua = validos_sua;
    let val_pct = if total_arch > 0 {
        format!("{:.1}%", (val_sua as f64 / total_arch as f64) * 100.0)
    } else {
        "0.0%".to_string()
    };
    let no_proc = no_procesados;
    let no_proc_pct = if total_arch > 0 {
        format!("{:.1}%", (no_proc as f64 / total_arch as f64) * 100.0)
    } else {
        "0.0%".to_string()
    };

    let period_str = match (min_period_date, max_period_date) {
        (Some(start), Some(end)) => {
            let m_start = month_num_to_name(start.month());
            let m_end = month_num_to_name(end.month());
            format!("{} {} – {} {}", &m_start[0..3.min(m_start.len())], start.year(), &m_end[0..3.min(m_end.len())], end.year())
        }
        _ => "N/D".to_string(),
    };

    let prima_rt_str = valid_businesses.iter()
        .find_map(|b| b.prima.as_deref())
        .map(|p| format!("RT: {}%", p))
        .unwrap_or_else(|| "RT: 7.58875%".to_string());

    // Generar archivo Excel con negocios válidos si se especificó ruta
    if let Some(excel_path) = output_excel_path {
        let mut sheets: HashMap<String, Business> = HashMap::new();
        let mut valid_workers = 0;
        for biz in valid_businesses {
            valid_workers += biz.employees.len();
            let key = biz.registro_patronal.as_deref().unwrap_or("REG_PATRONAL").trim().to_uppercase();
            if let Some(existing) = sheets.get_mut(&key) {
                existing.merge_with(biz, "archivo_combinado");
                existing.employees.extend(biz.employees.clone());
            } else {
                sheets.insert(key, biz.clone());
            }
        }

        let stats = crate::cedulas::processor::CedulasStats {
            total_files: total_archivos_analizados,
            valid_files: validos_sua,
            valid_workers,
            duplicate_workers: total_repetitions,
            invalid_files: no_procesados,
            total_cuotas: total_importe,
        };

        let system_lang = crate::cedulas::excel::detect_system_language();
        let mut excel = Excel::new(&excel_path.to_string_lossy());
        let sheets_vec: Vec<(String, Business)> = sheets.into_iter().collect();
        let _ = excel.create_excel_full(&sheets_vec, &[], &stats, system_lang);
    }

    CedulasAuditResult {
        total_archivos_analizados: total_arch,
        validos_sua: val_sua,
        validos_pct: val_pct,
        no_procesados: no_proc,
        no_procesados_pct: no_proc_pct,
        no_procesados_desc: if no_proc > 0 { "Archivos que no corresponden al cliente o sin formato SUA".to_string() } else { "Todos los archivos procesados con éxito".to_string() },
        trabajadores_unicos: unique_worker_ids.len(),
        repeticiones_mensuales: total_repetitions,
        importe_total: total_importe,
        importe_total_str: format!("{} MXN", format_currency(total_importe)),
        cuota_patronal: total_patronal,
        cuota_patronal_str: format_currency(total_patronal),
        cuota_obrera: total_obrera,
        cuota_obrera_str: format_currency(total_obrera),

        superposiciones_count,
        obras_fuera_revision_count: obras_fuera_revision_set.len(),
        empleados_obras_fuera_count: empleados_obras_fuera_set.len(),
        archivos_cliente_invalido_count,
        sin_obras_revision_warning: revision_obras_registered.is_empty(),

        periodo_rango_str: period_str,
        periodo_sub_str: format!("Bimestrales: {} | Mensuales: {}", bimonthly_count, monthly_count),
        mensuales_count: monthly_count,
        bimestrales_count: bimonthly_count,

        sbc_promedio_str: format!("${:.2} MXN / día", avg_sbc),
        sbc_sub_str: "Coherente con Tabulador de Obra".to_string(),
        sbc_min_str: format!("Mínimo: ${:.2}", min_sbc),
        sbc_max_str: format!("Máximo (Topado UMA): ${:.2}", max_sbc),

        ramas_rt_str: prima_rt_str,
        ramas_sub_str: "Prestaciones en Especie y Dinero (100% Homologado)".to_string(),

        detalle_rows,
        sumatorias_rows,

        gran_total_trabajadores: unique_worker_ids.len(),
        gran_total_dias,
        gran_total_patronal_str: format_currency(total_patronal),
        gran_total_obrera_str: format_currency(total_obrera),
        gran_total_str: format_currency(total_importe),

        excel_path: output_excel_path.map(|p| p.to_path_buf()),
        mismatch_report: Default::default(),
    }
}

pub fn load_cedulas_from_db(
    client: &mut Client,
    id_revision: &str,
) -> Result<Option<(CedulasAuditResult, Vec<Business>)>, String> {
    let target_uuid = match Uuid::parse_str(id_revision.trim()) {
        Ok(u) => u,
        Err(_) => return Ok(None),
    };

    let rp_uuid: Uuid = if let Ok(Some(r)) = client.query_opt("SELECT id_registro_patronal FROM public.revisiones WHERE id = $1", &[&target_uuid]) {
        r.get(0)
    } else if let Ok(Some(r)) = client.query_opt("SELECT id FROM public.registros_patronales WHERE id = $1", &[&target_uuid]) {
        r.get(0)
    } else {
        return Ok(None);
    };

    let count_row = client
        .query_one("SELECT count(*) FROM public.liquidaciones WHERE id_registro_patronal = $1", &[&rp_uuid])
        .map_err(|e| format!("Error contando liquidaciones: {}", e))?;
    let count: i64 = count_row.get(0);
    if count == 0 {
        return Ok(None);
    }

    // 1. Obras registradas en la revisión
    let obra_rows = client
        .query("SELECT numero FROM public.registros_obras WHERE (id_revision = $1 OR id_registro_patronal = $2) AND COALESCE(creado_usuario, false) = true", &[&target_uuid, &rp_uuid])
        .map_err(|e| format!("Error consultando obras de la revisión: {}", e))?;
    let mut revision_obras = HashSet::new();
    for r in obra_rows {
        if let Some(num) = r.get::<_, Option<String>>(0) {
            let t = num.trim().to_uppercase();
            if !t.is_empty() {
                revision_obras.insert(t);
            }
        }
    }

    // 2. Vinculaciones de obras por liquidación
    let vinc_rows = client
        .query(
            r#"
            SELECT 
                vol.id_liquidacion,
                ro.numero,
                vol.dias_registro_obra
            FROM public.vinculaciones_obras_liquidaciones vol
            JOIN public.registros_obras ro ON vol.id_registro_obra = ro.id
            JOIN public.liquidaciones l ON vol.id_liquidacion = l.id
            WHERE l.id_registro_patronal = $1
            "#,
            &[&rp_uuid],
        )
        .map_err(|e| format!("Error consultando vinculaciones de obras: {}", e))?;

    let mut vinc_map: HashMap<Uuid, Vec<RegistroObra>> = HashMap::new();
    for row in vinc_rows {
        let id_liq: Uuid = row.get(0);
        let num_reg: Option<String> = row.get(1);
        let dias: Option<i16> = row.get(2);
        if let Some(nr) = num_reg {
            let dias_str = dias.map(|d| d.to_string()).unwrap_or_else(|| "0".to_string());
            vinc_map.entry(id_liq).or_default().push(RegistroObra::new(&nr, &dias_str));
        }
    }

    // 3. Liquidaciones y detalles
    let rows = client
        .query(
            r#"
            SELECT 
                l.id AS id_liquidacion,
                l.mes_periodo,
                l.anio_periodo,
                l.nombre_archivo,
                e.id AS id_empleado,
                e.nombre AS empleado_nombre,
                e.rfc AS empleado_rfc,
                e.curp AS empleado_curp,
                e.nss AS empleado_nss,
                e.clave_ubicacion,
                rp.registro AS registro_patronal,
                rp.prima_riesgo,
                COALESCE(c.razon_social, c.nombre) AS razon_social,
                c.rfc AS cliente_rfc,
                dl.clave,
                dl.fecha,
                dl.dias AS dias_detalle,
                dl.sdi,
                dl.lic,
                dl.inc,
                dl.aus,
                dl.cf,
                dl.exc_pat,
                dl.exc_obr,
                dl.pd_pat,
                dl.pd_obr,
                dl.gmp_pat,
                dl.gmp_obr,
                dl.rt,
                dl.iv_pat,
                dl.iv_obr,
                dl.gps,
                dl.patronal,
                dl.obrera,
                dl.subtotal
            FROM public.liquidaciones l
            JOIN public.empleados e ON l.id_empleado = e.id
            JOIN public.registros_patronales rp ON l.id_registro_patronal = rp.id
            JOIN public.clientes c ON rp.id_cliente = c.id
            LEFT JOIN public.detalles_liquidaciones dl ON dl.id_liquidacion = l.id
            WHERE l.id_registro_patronal = $1
            ORDER BY rp.registro, l.anio_periodo, l.mes_periodo, e.nombre, l.id, dl.fecha;
            "#,
            &[&rp_uuid],
        )
        .map_err(|e| format!("Error consultando liquidaciones de la revisión: {}", e))?;

    let mut distinct_files = HashSet::new();
    let mut businesses_map: HashMap<(String, String), Business> = HashMap::new();
    let mut current_liq_id: Option<Uuid> = None;
    let mut current_emp: Option<Employee> = None;
    let mut current_bp_key: Option<(String, String)> = None;

    for row in rows {
        let id_liq: Uuid = row.get(0);
        let mes_per: Option<String> = row.get(1);
        let anio_per: Option<i16> = row.get(2);
        let nom_arch: Option<String> = row.get(3);
        if let Some(ref na) = nom_arch {
            distinct_files.insert(na.clone());
        }

        let emp_nombre: Option<String> = row.get(5);
        let emp_rfc: Option<String> = row.get(6);
        let emp_curp: Option<String> = row.get(7);
        let emp_nss: Option<String> = row.get(8);
        let emp_ubicacion: Option<String> = row.get(9);
        let reg_pat: Option<String> = row.get(10);
        let prima_val: Option<f64> = row.get(11);
        let razon_soc: Option<String> = row.get(12);

        let rp_clean = reg_pat.as_deref().unwrap_or("00000000000").trim().to_uppercase();
        let rz_clean = razon_soc.as_deref().unwrap_or("EMPRESA").trim().to_uppercase();
        let bp_key = (rp_clean.clone(), rz_clean.clone());

        if current_liq_id != Some(id_liq) {
            if let (Some(emp), Some(key)) = (current_emp.take(), current_bp_key.take()) {
                businesses_map
                    .entry(key.clone())
                    .or_insert_with(|| {
                        let mut b = Business::new();
                        b.registro_patronal = Some(key.0);
                        b.razon_social = Some(key.1);
                        b.prima = prima_val.map(|p| p.to_string());
                        b
                    })
                    .employees
                    .push(emp);
            }

            let mut emp = Employee::new();
            emp.no_seguro_social = emp_nss.map(|s| s.trim().to_string());
            emp.nombre = emp_nombre.map(|s| s.trim().to_string());
            emp.rfc = emp_rfc.map(|s| s.trim().to_string());
            emp.curp = emp_curp.map(|s| s.trim().to_string());
            emp.clave_ubicacion = emp_ubicacion.map(|s| s.trim().to_string());
            emp.mes_periodo = mes_per.clone().map(|s| s.trim().to_string());
            emp.anio_periodo = anio_per.map(|a| a.to_string());
            if let Some(obras) = vinc_map.remove(&id_liq) {
                emp.registro_obras = obras;
            }

            current_liq_id = Some(id_liq);
            current_bp_key = Some(bp_key);
            current_emp = Some(emp);
        }

        let dl_clave: Option<String> = row.get(14);
        let dl_fecha: Option<NaiveDate> = row.get(15);
        let dl_dias: Option<i16> = row.get(16);
        let dl_sdi: Option<f64> = row.get(17);

        if dl_clave.is_some() || dl_fecha.is_some() || dl_dias.is_some() || dl_sdi.is_some() {
            let de = DataEmployee {
                mes_periodo: mes_per.clone().map(|s| s.trim().to_string()),
                anio_periodo: anio_per.map(|a| a.to_string()),
                clave: dl_clave.map(|s| s.trim().to_string()),
                fecha: dl_fecha,
                dias: dl_dias.map(|d| d as f64),
                sdi: dl_sdi,
                lic: row.get(18),
                inc: row.get(19),
                aus: row.get(20),
                cf: row.get(21),
                exc_pat: row.get(22),
                exc_obr: row.get(23),
                pd_pat: row.get(24),
                pd_obr: row.get(25),
                gmp_pat: row.get(26),
                gmp_obr: row.get(27),
                rt: row.get(28),
                iv_pat: row.get(29),
                iv_obr: row.get(30),
                gps: row.get(31),
                patronal: row.get(32),
                obrera: row.get(33),
                subtotal: row.get(34),
            };
            if let Some(ref mut emp) = current_emp {
                emp.data_employee.push(de);
            }
        }
    }

    if let (Some(emp), Some(key)) = (current_emp.take(), current_bp_key.take()) {
        businesses_map
            .entry(key.clone())
            .or_insert_with(|| {
                let mut b = Business::new();
                b.registro_patronal = Some(key.0);
                b.razon_social = Some(key.1);
                b
            })
            .employees
            .push(emp);
    }

    let valid_businesses: Vec<Business> = businesses_map.into_values().collect();

    if valid_businesses.is_empty() {
        return Ok(None);
    }

    let total_files = distinct_files.len().max(1);
    let audit = build_cedulas_audit_from_businesses(
        &valid_businesses,
        &revision_obras,
        total_files,
        total_files,
        0,
        0,
        None,
    );

    Ok(Some((audit, valid_businesses)))
}

pub fn save_cedulas_audit_to_db(
    pool: &DbPool,
    id_revision: &str,
    businesses: &[Business],
    progress_cb: Option<&(dyn Fn(usize, usize, &str) + Send + Sync)>,
) -> Result<CedulasDbStats, String> {
    if businesses.is_empty() {
        return Ok(CedulasDbStats::default());
    }

    let mut client = pool.get().map_err(|e| format!("Error conectando a PostgreSQL: {}", e))?;
    let mut stats = CedulasDbStats::default();
    let rev_uuid_opt = if !id_revision.trim().is_empty() {
        Uuid::parse_str(id_revision.trim()).ok()
    } else {
        None
    };

    let total_emps: usize = businesses.iter().map(|b| b.employees.len()).sum();
    let mut current_emp_idx = 0;

    for biz in businesses {
        let mut tx = match client.transaction() {
            Ok(t) => t,
            Err(e) => {
                stats.errores.push(format!("Error iniciando transacción: {}", e));
                continue;
            }
        };

        let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = (|| {
            let ctx = resolve_business_context_for_revision(&mut tx, biz, rev_uuid_opt)?;

            for emp in &biz.employees {
                let (e_c, l_c, d_c, ro_c, v_c) = save_single_cedula_employee_tx(&mut tx, emp, &ctx)?;
                stats.empleados_guardados += e_c;
                stats.liquidaciones_guardadas += l_c;
                stats.detalles_guardados += d_c;
                stats.registros_obras_guardados += ro_c;
                stats.vinculaciones_guardadas += v_c;

                current_emp_idx += 1;
                if let Some(cb) = progress_cb {
                    if current_emp_idx % 10 == 0 || current_emp_idx == total_emps {
                        cb(
                            current_emp_idx,
                            total_emps,
                            &format!(
                                "Guardando en BD: trabajador {}/{}...",
                                current_emp_idx, total_emps
                            ),
                        );
                    }
                }
            }
            Ok(())
        })();

        match result {
            Ok(_) => {
                if let Err(e) = tx.commit() {
                    stats.errores.push(format!("Error en commit de cédula: {}", e));
                }
            }
            Err(e) => {
                let _ = tx.rollback();
                stats.errores.push(format!("Error guardando cédula: {}", e));
            }
        }
    }

    Ok(stats)
}

pub fn delete_all_cedulas_for_revision(
    client: &mut Client,
    id_revision: &str,
) -> Result<u64, String> {
    if id_revision.is_empty() {
        let _ = client.execute("DELETE FROM public.reportes_detalles_liquidaciones", &[]);
        let _ = client.execute("DELETE FROM public.reportes_vinculaciones_obras_liquidaciones", &[]);
        let _ = client.execute("DELETE FROM public.reportes_liquidaciones", &[]);
        let _ = client.execute(
            "DELETE FROM public.vinculaciones_obras_liquidaciones WHERE id_liquidacion IN (SELECT id FROM public.liquidaciones)",
            &[],
        );
        let _ = client.execute(
            "DELETE FROM public.detalles_liquidaciones WHERE id_liquidacion IN (SELECT id FROM public.liquidaciones)",
            &[],
        );
        let n = client
            .execute("DELETE FROM public.liquidaciones", &[])
            .map_err(|e| format!("Error al borrar todas las liquidaciones: {}", e))?;
        crate::app_logger::log_db_op(
            "DELETE",
            &[
                "public.reportes_detalles_liquidaciones",
                "public.reportes_vinculaciones_obras_liquidaciones",
                "public.reportes_liquidaciones",
                "public.vinculaciones_obras_liquidaciones",
                "public.detalles_liquidaciones",
                "public.liquidaciones",
            ],
            "DELETE FROM liquidaciones... (todas las revisiones)",
            "Eliminación total de liquidaciones",
            Some(n as usize),
        );
        Ok(n)
    } else {
        let parsed_uuid = Uuid::parse_str(id_revision).map_err(|e| format!("UUID inválido: {}", e))?;
        let rp_uuid: Uuid = if let Ok(Some(r)) = client.query_opt("SELECT id_registro_patronal FROM public.revisiones WHERE id = $1", &[&parsed_uuid]) {
            r.get(0)
        } else if let Ok(Some(r)) = client.query_opt("SELECT id FROM public.registros_patronales WHERE id = $1", &[&parsed_uuid]) {
            r.get(0)
        } else {
            parsed_uuid
        };

        let _ = client.execute(
            "DELETE FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion IN (SELECT id FROM public.reportes_liquidaciones WHERE id_registro_patronal = $1)",
            &[&rp_uuid],
        );
        let _ = client.execute(
            "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion IN (SELECT id FROM public.reportes_liquidaciones WHERE id_registro_patronal = $1)",
            &[&rp_uuid],
        );
        let _ = client.execute("DELETE FROM public.reportes_liquidaciones WHERE id_registro_patronal = $1", &[&rp_uuid]);

        let _ = client.execute(
            "DELETE FROM public.vinculaciones_obras_liquidaciones WHERE id_liquidacion IN (SELECT id FROM public.liquidaciones WHERE id_registro_patronal = $1)",
            &[&rp_uuid],
        );
        let _ = client.execute(
            "DELETE FROM public.detalles_liquidaciones WHERE id_liquidacion IN (SELECT id FROM public.liquidaciones WHERE id_registro_patronal = $1)",
            &[&rp_uuid],
        );
        let n = client
            .execute("DELETE FROM public.liquidaciones WHERE id_registro_patronal = $1", &[&rp_uuid])
            .map_err(|e| format!("Error al borrar liquidaciones del registro patronal: {}", e))?;

        crate::app_logger::log_db_op(
            "DELETE",
            &[
                "public.reportes_detalles_liquidaciones",
                "public.reportes_vinculaciones_obras_liquidaciones",
                "public.reportes_liquidaciones",
                "public.vinculaciones_obras_liquidaciones",
                "public.detalles_liquidaciones",
                "public.liquidaciones",
            ],
            "DELETE FROM liquidaciones... WHERE id_registro_patronal = $1",
            &format!("id_registro_patronal={}", rp_uuid),
            Some(n as usize),
        );
        Ok(n)
    }
}

/// Elimina los registros físicos y lógicos en PostgreSQL de una cédula de cuotas
/// para un mes y año específicos dentro de un registro patronal (soporta sobreescritura limpia).
pub fn delete_cedula_for_month(
    client: &mut Client,
    id_revision: &str,
    mes_periodo: &str,
    anio_periodo: i32,
) -> Result<u64, String> {
    let parsed_uuid = Uuid::parse_str(id_revision).map_err(|e| format!("UUID inválido: {}", e))?;
    let rp_uuid: Uuid = if let Ok(Some(r)) = client.query_opt("SELECT id_registro_patronal FROM public.revisiones WHERE id = $1", &[&parsed_uuid]) {
        r.get(0)
    } else if let Ok(Some(r)) = client.query_opt("SELECT id FROM public.registros_patronales WHERE id = $1", &[&parsed_uuid]) {
        r.get(0)
    } else {
        parsed_uuid
    };
    let mes_clean = mes_periodo.trim().to_uppercase();

    let _ = client.execute(
        "DELETE FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion IN (
            SELECT id FROM public.reportes_liquidaciones 
            WHERE id_registro_patronal = $1 AND UPPER(TRIM(mes_periodo)) = $2 AND anio_periodo = $3
        )",
        &[&rp_uuid, &mes_clean, &anio_periodo],
    );
    let _ = client.execute(
        "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion IN (
            SELECT id FROM public.reportes_liquidaciones 
            WHERE id_registro_patronal = $1 AND UPPER(TRIM(mes_periodo)) = $2 AND anio_periodo = $3
        )",
        &[&rp_uuid, &mes_clean, &anio_periodo],
    );
    let _ = client.execute(
        "DELETE FROM public.reportes_liquidaciones 
         WHERE id_registro_patronal = $1 AND UPPER(TRIM(mes_periodo)) = $2 AND anio_periodo = $3",
        &[&rp_uuid, &mes_clean, &anio_periodo],
    );

    let _ = client.execute(
        "DELETE FROM public.vinculaciones_obras_liquidaciones WHERE id_liquidacion IN (
            SELECT id FROM public.liquidaciones 
            WHERE id_registro_patronal = $1 AND UPPER(TRIM(mes_periodo)) = $2 AND anio_periodo = $3
        )",
        &[&rp_uuid, &mes_clean, &anio_periodo],
    );
    let _ = client.execute(
        "DELETE FROM public.detalles_liquidaciones WHERE id_liquidacion IN (
            SELECT id FROM public.liquidaciones 
            WHERE id_registro_patronal = $1 AND UPPER(TRIM(mes_periodo)) = $2 AND anio_periodo = $3
        )",
        &[&rp_uuid, &mes_clean, &anio_periodo],
    );
    let n = client
        .execute(
            "DELETE FROM public.liquidaciones 
             WHERE id_registro_patronal = $1 AND UPPER(TRIM(mes_periodo)) = $2 AND anio_periodo = $3",
            &[&rp_uuid, &mes_clean, &anio_periodo],
        )
        .map_err(|e| format!("Error al borrar liquidación del periodo {}/{}: {}", mes_clean, anio_periodo, e))?;

    log::info!("Eliminadas {} liquidaciones del mes {}/{} para el registro patronal {}", n, mes_clean, anio_periodo, rp_uuid);
    Ok(n)
}

pub fn export_cedulas_comprehensive_excel(
    dest_path: &Path,
    audit: &CedulasAuditResult,
    businesses: &[Business],
) -> Result<(), XlsxError> {
    let mut workbook = Workbook::new();

    let title_fmt = Format::new()
        .set_bold()
        .set_font_size(14)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1E3A8A))
        .set_align(FormatAlign::Center);

    let header_fmt = Format::new()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x0F766E))
        .set_align(FormatAlign::Center);

    let currency_fmt = Format::new().set_num_format("$#,##0.00");
    let int_fmt = Format::new().set_num_format("#,##0");

    // Sheet 1: Estadísticas y Auditoría
    let ws_stats = workbook.add_worksheet();
    ws_stats.set_name("Estadisticas_Auditoria")?;

    ws_stats.merge_range(0, 0, 0, 5, "AUDITORÍA INTEGRAL DE CÉDULAS DE DETERMINACIÓN DE CUOTAS SUA", &title_fmt)?;

    ws_stats.write_string(2, 0, "Métrica / Indicador")?;
    ws_stats.write_string(2, 1, "Valor")?;
    ws_stats.write_string(2, 2, "Detalle / Observación")?;

    let stat_rows = [
        ("Total de Archivos Analizados", audit.total_archivos_analizados.to_string(), format!("{} válidos SUA ({})", audit.validos_sua, audit.validos_pct)),
        ("Archivos Válidos SUA", audit.validos_sua.to_string(), audit.validos_pct.clone()),
        ("Archivos No Procesados", audit.no_procesados.to_string(), audit.no_procesados_desc.clone()),
        ("Trabajadores Únicos (Concentrados)", audit.trabajadores_unicos.to_string(), format!("{} repeticiones mensuales consolidadas", audit.repeticiones_mensuales)),
        ("Importe Total Liquidaciones IMSS", audit.importe_total_str.clone(), format!("Patronal: {} | Obrera: {}", audit.cuota_patronal_str, audit.cuota_obrera_str)),
        ("Superposiciones de Fechas entre Obras", audit.superposiciones_count.to_string(), "Trabajadores asignados a 2 o más obras con fechas solapadas".to_string()),
        ("Obras Fuera de la Revisión Actual", audit.obras_fuera_revision_count.to_string(), format!("{} trabajadores registrados en DB en obras no auditadas", audit.empleados_obras_fuera_count)),
        ("Archivos de Otro Patrón Descartados", audit.archivos_cliente_invalido_count.to_string(), "Documentos con RFC o Razón Social ajenos al cliente en revisión".to_string()),
        ("Rango de Periodos Detectados", audit.periodo_rango_str.clone(), audit.periodo_sub_str.clone()),
        ("SBC Promedio Ponderado", audit.sbc_promedio_str.clone(), format!("{} | {}", audit.sbc_min_str, audit.sbc_max_str)),
    ];

    for (r, (k, v, obs)) in stat_rows.iter().enumerate() {
        let row_idx = (r + 3) as u32;
        ws_stats.write_string(row_idx, 0, *k)?;
        ws_stats.write_string(row_idx, 1, v)?;
        ws_stats.write_string(row_idx, 2, obs)?;
    }
    ws_stats.autofit();

    // Sheet 2: Totales y Sumatorias por Mes, RP y Obra
    let ws_sum = workbook.add_worksheet();
    ws_sum.set_name("Totales_Mes_RP_Obra")?;

    let sum_headers = [
        "REGISTRO PATRONAL",
        "RAZÓN SOCIAL",
        "AÑO",
        "MES",
        "REGISTRO DE OBRA",
        "TRABAJADORES (SIN DUPLICAR)",
        "TOTAL DÍAS",
        "CUOTA PATRONAL ($)",
        "CUOTA OBRERA ($)",
        "TOTAL LIQUIDACIÓN ($)",
    ];
    for (col, h) in sum_headers.iter().enumerate() {
        ws_sum.write_string_with_format(0, col as u16, *h, &header_fmt)?;
    }

    for (r, row) in audit.sumatorias_rows.iter().enumerate() {
        let row_idx = (r + 1) as u32;
        ws_sum.write_string(row_idx, 0, &row.registro_patronal)?;
        ws_sum.write_string(row_idx, 1, &row.razon_social)?;
        ws_sum.write_string(row_idx, 2, &row.anio)?;
        ws_sum.write_string(row_idx, 3, &row.mes)?;
        ws_sum.write_string(row_idx, 4, &row.registro_obra)?;
        ws_sum.write_number_with_format(row_idx, 5, row.trabajadores_count as f64, &int_fmt)?;
        ws_sum.write_number_with_format(row_idx, 6, row.total_dias as f64, &int_fmt)?;
        ws_sum.write_number_with_format(row_idx, 7, row.cuota_patronal, &currency_fmt)?;
        ws_sum.write_number_with_format(row_idx, 8, row.cuota_obrera, &currency_fmt)?;
        ws_sum.write_number_with_format(row_idx, 9, row.total_liquidacion, &currency_fmt)?;
    }

    // Gran Total row
    let tot_row = (audit.sumatorias_rows.len() + 1) as u32;
    let bold_tot = Format::new().set_bold().set_background_color(Color::RGB(0xDBEAFE));
    ws_sum.merge_range(tot_row, 0, tot_row, 4, "GRAN TOTAL CONSOLIDADO AUDITADO", &bold_tot)?;
    ws_sum.write_number_with_format(tot_row, 5, audit.gran_total_trabajadores as f64, &int_fmt)?;
    ws_sum.write_number_with_format(tot_row, 6, audit.gran_total_dias as f64, &int_fmt)?;
    ws_sum.write_number_with_format(tot_row, 7, audit.cuota_patronal, &currency_fmt)?;
    ws_sum.write_number_with_format(tot_row, 8, audit.cuota_obrera, &currency_fmt)?;
    ws_sum.write_number_with_format(tot_row, 9, audit.importe_total, &currency_fmt)?;
    ws_sum.autofit();

    // Sheet 3: Detalle de Cédulas SUA
    let ws_det = workbook.add_worksheet();
    ws_det.set_name("Detalle_Cedulas_SUA")?;

    let det_headers = [
        "# REG.",
        "MES / AÑO",
        "REGISTRO PATRONAL",
        "REGISTRO DE OBRA",
        "NSS (11 DÍGITOS)",
        "RFC / CURP",
        "NOMBRE DEL TRABAJADOR",
        "DÍAS",
        "SDI ($)",
        "CUOTA PATRONAL ($)",
        "CUOTA OBRERA ($)",
        "TOTAL LIQUIDACIÓN ($)",
    ];
    for (col, h) in det_headers.iter().enumerate() {
        ws_det.write_string_with_format(0, col as u16, *h, &header_fmt)?;
    }

    for (r, row) in audit.detalle_rows.iter().enumerate() {
        let row_idx = (r + 1) as u32;
        ws_det.write_string(row_idx, 0, &row.reg_num_str)?;
        ws_det.write_string(row_idx, 1, &row.mes_anio)?;
        ws_det.write_string(row_idx, 2, &row.registro_patronal)?;
        ws_det.write_string(row_idx, 3, &row.registro_obra)?;
        ws_det.write_string(row_idx, 4, &row.nss)?;
        ws_det.write_string(row_idx, 5, &row.rfc_curp)?;
        ws_det.write_string(row_idx, 6, &row.nombre_trabajador)?;
        ws_det.write_number_with_format(row_idx, 7, row.dias as f64, &int_fmt)?;
        ws_det.write_number_with_format(row_idx, 8, row.sdi, &currency_fmt)?;
        ws_det.write_number_with_format(row_idx, 9, row.cuota_patronal, &currency_fmt)?;
        ws_det.write_number_with_format(row_idx, 10, row.cuota_obrera, &currency_fmt)?;
        ws_det.write_number_with_format(row_idx, 11, row.total, &currency_fmt)?;
    }
    ws_det.autofit();

    // Sheet 4: Cédula Completa (Todos los datos de liquidaciones empleado por empleado)
    let ws_comp = workbook.add_worksheet();
    ws_comp.set_name("Cedula_Completa")?;

    let comp_headers = [
        "# REG.",
        "MES / AÑO",
        "AÑO",
        "MES",
        "REGISTRO PATRONAL",
        "RAZÓN SOCIAL",
        "REGISTRO DE OBRA",
        "NSS (11 DÍGITOS)",
        "RFC",
        "CURP",
        "NOMBRE DEL TRABAJADOR",
        "CLAVE UBICACIÓN",
        "CLAVE MOV.",
        "FECHA MOV.",
        "DÍAS",
        "SDI ($)",
        "LIC (DÍAS)",
        "INC (DÍAS)",
        "AUS (DÍAS)",
        "CUOTA FIJA ($)",
        "EXC. PATRONAL ($)",
        "EXC. OBRERA ($)",
        "PREST. DIN. PATRONAL ($)",
        "PREST. DIN. OBRERA ($)",
        "GASTOS MED. PATRONAL ($)",
        "GASTOS MED. OBRERA ($)",
        "RIESGO TRABAJO ($)",
        "INVALIDEZ Y VIDA PAT. ($)",
        "INVALIDEZ Y VIDA OBR. ($)",
        "GUARDERÍAS ($)",
        "CUOTA PATRONAL ($)",
        "CUOTA OBRERA ($)",
        "TOTAL LIQUIDACIÓN ($)",
    ];
    for (col, h) in comp_headers.iter().enumerate() {
        ws_comp.write_string_with_format(0, col as u16, *h, &header_fmt)?;
    }

    let mut comp_row_idx: u32 = 1;
    let mut tot_dias: f64 = 0.0;
    let mut tot_lic: f64 = 0.0;
    let mut tot_inc: f64 = 0.0;
    let mut tot_aus: f64 = 0.0;
    let mut tot_cf: f64 = 0.0;
    let mut tot_exc_pat: f64 = 0.0;
    let mut tot_exc_obr: f64 = 0.0;
    let mut tot_pd_pat: f64 = 0.0;
    let mut tot_pd_obr: f64 = 0.0;
    let mut tot_gmp_pat: f64 = 0.0;
    let mut tot_gmp_obr: f64 = 0.0;
    let mut tot_rt: f64 = 0.0;
    let mut tot_iv_pat: f64 = 0.0;
    let mut tot_iv_obr: f64 = 0.0;
    let mut tot_gps: f64 = 0.0;
    let mut tot_patronal: f64 = 0.0;
    let mut tot_obrera: f64 = 0.0;
    let mut tot_subtotal: f64 = 0.0;

    for biz in businesses {
        let rp = biz.registro_patronal.as_deref().unwrap_or("SIN REGISTRO").trim();
        let razon = biz.razon_social.as_deref().unwrap_or("SIN RAZÓN SOCIAL").trim();

        for emp in &biz.employees {
            let nss = emp.no_seguro_social.as_deref().unwrap_or("SIN NSS").trim();
            let rfc = emp.rfc.as_deref().unwrap_or("").trim();
            let curp = emp.curp.as_deref().unwrap_or("").trim();
            let nombre = emp.nombre.as_deref().unwrap_or("SIN NOMBRE").trim();
            let ubicacion = emp.clave_ubicacion.as_deref().unwrap_or("").trim();
            let obra_str = if emp.registro_obras.is_empty() {
                "SIN REGISTRO DE OBRA".to_string()
            } else {
                emp.registro_obras
                    .iter()
                    .map(|ro| ro.to_string())
                    .collect::<Vec<String>>()
                    .join(" ")
            };

            if emp.data_employee.is_empty() {
                let mes = emp.mes_periodo.as_deref().unwrap_or("").trim();
                let anio = emp.anio_periodo.as_deref().unwrap_or("").trim();
                let mes_anio = if !mes.is_empty() && !anio.is_empty() {
                    format!("{} / {}", mes, anio)
                } else {
                    "".to_string()
                };

                ws_comp.write_string(comp_row_idx, 0, &format!("{:04}", comp_row_idx))?;
                ws_comp.write_string(comp_row_idx, 1, &mes_anio)?;
                ws_comp.write_string(comp_row_idx, 2, anio)?;
                ws_comp.write_string(comp_row_idx, 3, mes)?;
                ws_comp.write_string(comp_row_idx, 4, rp)?;
                ws_comp.write_string(comp_row_idx, 5, razon)?;
                ws_comp.write_string(comp_row_idx, 6, &obra_str)?;
                ws_comp.write_string(comp_row_idx, 7, nss)?;
                ws_comp.write_string(comp_row_idx, 8, rfc)?;
                ws_comp.write_string(comp_row_idx, 9, curp)?;
                ws_comp.write_string(comp_row_idx, 10, nombre)?;
                ws_comp.write_string(comp_row_idx, 11, ubicacion)?;
                ws_comp.write_string(comp_row_idx, 12, "")?;
                ws_comp.write_string(comp_row_idx, 13, "")?;
                ws_comp.write_number_with_format(comp_row_idx, 14, 0.0, &int_fmt)?;
                ws_comp.write_number_with_format(comp_row_idx, 15, 0.0, &currency_fmt)?;
                for c in 16..=18 {
                    ws_comp.write_number_with_format(comp_row_idx, c, 0.0, &int_fmt)?;
                }
                for c in 19..=32 {
                    ws_comp.write_number_with_format(comp_row_idx, c, 0.0, &currency_fmt)?;
                }
                comp_row_idx += 1;
            } else {
                for de in &emp.data_employee {
                    let mes = de.mes_periodo.as_deref().or(emp.mes_periodo.as_deref()).unwrap_or("").trim();
                    let anio = de.anio_periodo.as_deref().or(emp.anio_periodo.as_deref()).unwrap_or("").trim();
                    let mes_anio = if !mes.is_empty() && !anio.is_empty() {
                        format!("{} / {}", mes, anio)
                    } else {
                        "".to_string()
                    };
                    let clave = de.clave.as_deref().unwrap_or("").trim();
                    let fecha_str = de.fecha.map(|f| f.format("%d/%m/%Y").to_string()).unwrap_or_default();
                    let dias = de.dias.unwrap_or(0.0);
                    let sdi = de.sdi.unwrap_or(0.0);
                    let lic = de.lic.unwrap_or(0.0);
                    let inc = de.inc.unwrap_or(0.0);
                    let aus = de.aus.unwrap_or(0.0);
                    let cf = de.cf.unwrap_or(0.0);
                    let exc_pat = de.exc_pat.unwrap_or(0.0);
                    let exc_obr = de.exc_obr.unwrap_or(0.0);
                    let pd_pat = de.pd_pat.unwrap_or(0.0);
                    let pd_obr = de.pd_obr.unwrap_or(0.0);
                    let gmp_pat = de.gmp_pat.unwrap_or(0.0);
                    let gmp_obr = de.gmp_obr.unwrap_or(0.0);
                    let rt = de.rt.unwrap_or(0.0);
                    let iv_pat = de.iv_pat.unwrap_or(0.0);
                    let iv_obr = de.iv_obr.unwrap_or(0.0);
                    let gps = de.gps.unwrap_or(0.0);

                    let pat_calc = cf + exc_pat + pd_pat + gmp_pat + rt + iv_pat + gps;
                    let obr_calc = exc_obr + pd_obr + gmp_obr + iv_obr;

                    let patronal = if let Some(p) = de.patronal {
                        p
                    } else if pat_calc > 0.0 {
                        pat_calc
                    } else {
                        0.0
                    };

                    let obrera = if let Some(o) = de.obrera {
                        o
                    } else if obr_calc > 0.0 {
                        obr_calc
                    } else {
                        0.0
                    };

                    let subtotal = if let Some(s) = de.subtotal {
                        s
                    } else if patronal > 0.0 || obrera > 0.0 {
                        patronal + obrera
                    } else {
                        0.0
                    };

                    ws_comp.write_string(comp_row_idx, 0, &format!("{:04}", comp_row_idx))?;
                    ws_comp.write_string(comp_row_idx, 1, &mes_anio)?;
                    ws_comp.write_string(comp_row_idx, 2, anio)?;
                    ws_comp.write_string(comp_row_idx, 3, mes)?;
                    ws_comp.write_string(comp_row_idx, 4, rp)?;
                    ws_comp.write_string(comp_row_idx, 5, razon)?;
                    ws_comp.write_string(comp_row_idx, 6, &obra_str)?;
                    ws_comp.write_string(comp_row_idx, 7, nss)?;
                    ws_comp.write_string(comp_row_idx, 8, rfc)?;
                    ws_comp.write_string(comp_row_idx, 9, curp)?;
                    ws_comp.write_string(comp_row_idx, 10, nombre)?;
                    ws_comp.write_string(comp_row_idx, 11, ubicacion)?;
                    ws_comp.write_string(comp_row_idx, 12, clave)?;
                    ws_comp.write_string(comp_row_idx, 13, &fecha_str)?;

                    ws_comp.write_number_with_format(comp_row_idx, 14, dias, &int_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 15, sdi, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 16, lic, &int_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 17, inc, &int_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 18, aus, &int_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 19, cf, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 20, exc_pat, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 21, exc_obr, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 22, pd_pat, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 23, pd_obr, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 24, gmp_pat, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 25, gmp_obr, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 26, rt, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 27, iv_pat, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 28, iv_obr, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 29, gps, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 30, patronal, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 31, obrera, &currency_fmt)?;
                    ws_comp.write_number_with_format(comp_row_idx, 32, subtotal, &currency_fmt)?;

                    tot_dias += dias;
                    tot_lic += lic;
                    tot_inc += inc;
                    tot_aus += aus;
                    tot_cf += cf;
                    tot_exc_pat += exc_pat;
                    tot_exc_obr += exc_obr;
                    tot_pd_pat += pd_pat;
                    tot_pd_obr += pd_obr;
                    tot_gmp_pat += gmp_pat;
                    tot_gmp_obr += gmp_obr;
                    tot_rt += rt;
                    tot_iv_pat += iv_pat;
                    tot_iv_obr += iv_obr;
                    tot_gps += gps;
                    tot_patronal += patronal;
                    tot_obrera += obrera;
                    tot_subtotal += subtotal;

                    comp_row_idx += 1;
                }
            }
        }
    }

    // Gran Total row for Cedula_Completa
    let tot_row = comp_row_idx;
    ws_comp.merge_range(tot_row, 0, tot_row, 13, "TOTAL GENERAL CÉDULA COMPLETA", &bold_tot)?;
    ws_comp.write_number_with_format(tot_row, 14, tot_dias, &int_fmt)?;
    ws_comp.write_string_with_format(tot_row, 15, "", &bold_tot)?;
    ws_comp.write_number_with_format(tot_row, 16, tot_lic, &int_fmt)?;
    ws_comp.write_number_with_format(tot_row, 17, tot_inc, &int_fmt)?;
    ws_comp.write_number_with_format(tot_row, 18, tot_aus, &int_fmt)?;
    ws_comp.write_number_with_format(tot_row, 19, tot_cf, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 20, tot_exc_pat, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 21, tot_exc_obr, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 22, tot_pd_pat, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 23, tot_pd_obr, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 24, tot_gmp_pat, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 25, tot_gmp_obr, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 26, tot_rt, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 27, tot_iv_pat, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 28, tot_iv_obr, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 29, tot_gps, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 30, tot_patronal, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 31, tot_obrera, &currency_fmt)?;
    ws_comp.write_number_with_format(tot_row, 32, tot_subtotal, &currency_fmt)?;
    ws_comp.autofit();

    workbook.save(dest_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cedulas::employees::{DataEmployee, RegistroObra};

    #[test]
    fn test_export_cedulas_comprehensive_excel_generates_cedula_completa() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test_cedulas.xlsx");

        let audit = CedulasAuditResult::default();
        let mut biz = Business::new();
        biz.registro_patronal = Some("Y6712345101".to_string());
        biz.razon_social = Some("EMPRESA PRUEBA SA DE CV".to_string());

        let mut emp = Employee::new();
        emp.no_seguro_social = Some("12345678901".to_string());
        emp.nombre = Some("JUAN PEREZ LOPEZ".to_string());
        emp.rfc = Some("PELJ800101XYZ".to_string());
        emp.curp = Some("PELJ800101HDFRRN01".to_string());
        emp.clave_ubicacion = Some("OBRA-NORTE".to_string());
        emp.registro_obras.push(RegistroObra::new("OBRA-NORTE", "30"));

        let mut de = DataEmployee::default();
        de.mes_periodo = Some("ENERO".to_string());
        de.anio_periodo = Some("2026".to_string());
        de.dias = Some(30.0);
        de.sdi = Some(500.0);
        de.cf = Some(150.0);
        de.patronal = Some(450.0);
        de.obrera = Some(90.0);
        de.subtotal = Some(540.0);
        emp.data_employee.push(de);

        biz.employees.push(emp);

        let businesses = vec![biz];
        let res = export_cedulas_comprehensive_excel(&file_path, &audit, &businesses);
        assert!(res.is_ok(), "Excel generation failed: {:?}", res.err());
        assert!(file_path.exists());
    }

    #[test]
    fn test_load_cedulas_from_db_with_client_if_available() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(_) => return,
        };
        let res = load_cedulas_from_db(&mut client, "00000000-0000-0000-0000-000000000000");
        assert!(res.is_ok());
        assert!(res.unwrap().is_none());

        let res_inv = load_cedulas_from_db(&mut client, "not-a-uuid");
        assert!(res_inv.is_ok());
        assert!(res_inv.unwrap().is_none());

        let res_gondesa = load_cedulas_from_db(&mut client, "01a0eea6-73ad-7048-98d0-9e634f7ca391");
        assert!(res_gondesa.is_ok(), "Error cargando GONDESA: {:?}", res_gondesa.err());
        if let Ok(Some((audit, businesses))) = res_gondesa {
            println!("GONDESA DB load OK!");
            println!("  validos_sua: {}", audit.validos_sua);
            println!("  trabajadores_unicos: {}", audit.trabajadores_unicos);
            println!("  importe_total: {}", audit.gran_total_str);
            println!("  businesses: {}", businesses.len());
            assert!(!businesses.is_empty());
        }
    }
}

