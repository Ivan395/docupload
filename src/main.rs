#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::collapsible_str_replace,
    clippy::doc_lazy_continuation,
    clippy::explicit_auto_deref,
    clippy::get_first,
    clippy::if_same_then_else,
    clippy::manual_clamp,
    clippy::manual_contains,
    clippy::manual_div_ceil,
    clippy::manual_flatten,
    clippy::manual_is_multiple_of,
    clippy::manual_pattern_char_comparison,
    clippy::manual_unwrap_or,
    clippy::manual_unwrap_or_default,
    clippy::map_entry,
    clippy::needless_borrow,
    clippy::needless_borrows_for_generic_args,
    clippy::needless_late_init,
    clippy::needless_range_loop,
    clippy::new_without_default,
    clippy::ptr_arg,
    clippy::redundant_closure,
    clippy::redundant_pattern_matching,
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_cast,
    clippy::unnecessary_map_or,
    clippy::unnecessary_sort_by,
    clippy::unwrap_or_default,
    clippy::vec_init_then_push,
    clippy::while_let_on_iterator
)]
// Comentario

pub mod app_logger;
pub mod archives;
pub mod calendario_view_logic;
pub mod cedulas;
pub mod db;
pub mod nominas;
pub mod nominas_upload_view_logic;
pub mod nominas_view_logic;
pub mod rustfs;
pub mod reporte_view_logic;
pub mod asignacion_view_logic;
mod asistente_asignacion;
mod carga_masiva;
mod asignacion_obra_logic;
mod obras_lote_logic;
// Alta masiva de obras (ADD-01/ADD-02)
mod obras_lote_view_logic;
// Vista Empleados: estado laboral, filtros e histórico empleado × obra (D-8 / D-10 b)
mod empleados_view_logic;
// Selector de analista de la sesión (ADD-05, D-6 a)
mod analista_view_logic;
mod revisiones_lote_logic;
// Alta masiva de revisiones (ADD-03)
mod revisiones_lote_view_logic;
mod entregable_logic;
mod operacion_ui;
pub mod sat;
pub mod sua_export;
pub mod updater;
pub mod validation;
#[cfg(test)]
mod pruebas_golden_excel;
#[cfg(test)] mod captura_ui;

use crate::calendario_view_logic::*;
use crate::db::{create_pool, ensure_schema, DbPool, CedulasDbStats};
use crate::db::catalogos::*;
use crate::db::revisiones::*;
use crate::db::nominas_reporte::check_all_data_exists;
use crate::db::ingesta::{ArchivoLote, ArchivoParaRegistrar, tipo_archivo_desde_empleados};
use crate::cedulas::view_logic::*;
use crate::reporte_view_logic::*;
use crate::nominas_view_logic::*;
use crate::nominas_upload_view_logic::NominasUploadSessionState;
use crate::validation::*;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use uuid::Uuid;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

slint::include_modules!();

#[derive(Clone, Debug)]
struct SelectedFileEntry {
    id: i32,
    path: PathBuf,
    filename: String,
    filesize_str: String,
    filesize_bytes: u64,
    extension: String,
    is_archive: bool,
}

static ID_GEN: AtomicI32 = AtomicI32::new(1);

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn create_file_entry(path: PathBuf) -> SelectedFileEntry {
    let id = ID_GEN.fetch_add(1, Ordering::SeqCst);
    let filename = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let is_archive = ext == "zip" || ext == "rar";
    let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let filesize_str = format_bytes(bytes);

    SelectedFileEntry {
        id,
        path,
        filename,
        filesize_str,
        filesize_bytes: bytes,
        extension: ext,
        is_archive,
    }
}

fn update_nominas_ui(ui: &MainWindow, entries: &[SelectedFileEntry]) {
    let app_data = ui.global::<AppData>();
    let total_bytes: u64 = entries.iter().map(|e| e.filesize_bytes).sum();
    app_data.set_nominas_total_size(format_bytes(total_bytes).into());

    let models: Vec<FileItemData> = entries
        .iter()
        .map(|e| FileItemData {
            id: e.id,
            filename: e.filename.clone().into(),
            filesize: e.filesize_str.clone().into(),
            extension: e.extension.clone().into(),
            is_archive: e.is_archive,
            rustfs_path: "".into(),
        })
        .collect();

    app_data.set_nominas_files(ModelRc::new(VecModel::from(models)));
}

fn update_cedulas_ui(ui: &MainWindow, entries: &[SelectedFileEntry]) {
    let app_data = ui.global::<AppData>();
    let total_bytes: u64 = entries.iter().map(|e| e.filesize_bytes).sum();
    app_data.set_cedulas_total_size(format_bytes(total_bytes).into());

    let models: Vec<FileItemData> = entries
        .iter()
        .map(|e| FileItemData {
            id: e.id,
            filename: e.filename.clone().into(),
            filesize: e.filesize_str.clone().into(),
            extension: e.extension.clone().into(),
            is_archive: e.is_archive,
            rustfs_path: "".into(),
        })
        .collect();

    app_data.set_cedulas_files(ModelRc::new(VecModel::from(models)));
}

/// Texto del efecto de una resincronización del universo de la revisión; vacío si no hubo cambios.
fn mensaje_cambio_universo(c: &crate::db::obras_ops::CambioUniverso) -> String {
    if c.a_fuera == 0 && c.a_activo == 0 {
        String::new()
    } else {
        format!(
            "Reporte de liquidaciones: {} fila(s) pasaron a fuera de periodo, {} regresaron al periodo.",
            c.a_fuera, c.a_activo
        )
    }
}

pub fn show_toast_message(ui: &MainWindow, message: &str) {
    let app_data = ui.global::<AppData>();

    // Determinar el tipo de alerta según el contenido del mensaje
    let msg_lower = message.to_lowercase();
    let is_error = msg_lower.contains("no se puede")
        || msg_lower.contains("error")
        || msg_lower.contains("bloqueado")
        || msg_lower.contains("inválid")
        || msg_lower.contains("invalid")
        || msg_lower.contains("falló")
        || msg_lower.contains("fallo")
        || msg_lower.contains("denegad")
        || msg_lower.contains("rechazad");

    let is_warning = !is_error && (
        msg_lower.contains("⚠️")
        || msg_lower.contains("cuidado")
        || msg_lower.contains("advertencia")
        || msg_lower.contains("selecciona")
        || msg_lower.contains("seleccione")
        || msg_lower.contains("por favor")
        || msg_lower.contains("debe ")
        || msg_lower.contains("deben ")
        || msg_lower.contains("no hay")
        || msg_lower.contains("sin registros")
    );

    let is_success = !is_error && !is_warning && (
        msg_lower.contains("✓")
        || msg_lower.contains("correctamente")
        || msg_lower.contains("éxito")
        || msg_lower.contains("exito")
        || msg_lower.contains("guardad")
        || msg_lower.contains("actualizad")
        || msg_lower.contains("descargad")
        || msg_lower.contains("completad")
        || msg_lower.contains("recargad")
        || msg_lower.contains("restablecid")
        || msg_lower.contains("limpiad")
    );

    let (toast_type, toast_icon) = if is_error {
        ("error", "⛔")
    } else if is_warning {
        ("warning", "⚠️")
    } else if is_success {
        ("success", "✓")
    } else {
        ("info", "ℹ")
    };

    app_data.set_toast_message(message.into());
    app_data.set_toast_type(toast_type.into());
    app_data.set_toast_icon(toast_icon.into());
    app_data.set_show_toast(true);

    // Si el modal individual de edición de reporte está abierto, sincronizar alerta en el modal
    if app_data.get_show_reporte_edit_modal() {
        app_data.set_reporte_edit_alert_text(message.into());
        app_data.set_reporte_edit_alert_type(toast_type.into());
        app_data.set_reporte_edit_alert_show(true);
    }

    // Si el modal masivo de edición de reporte está abierto, sincronizar alerta en el modal
    if app_data.get_show_reporte_batch_edit_modal() {
        app_data.set_reporte_batch_edit_alert_text(message.into());
        app_data.set_reporte_batch_edit_alert_type(toast_type.into());
        app_data.set_reporte_batch_edit_alert_show(true);
    }

    let duration = if is_error || is_warning {
        std::time::Duration::from_millis(4500)
    } else {
        std::time::Duration::from_millis(3000)
    };

    let ui_weak = ui.as_weak();
    slint::Timer::single_shot(duration, move || {
        if let Some(ui) = ui_weak.upgrade() {
            let ad = ui.global::<AppData>();
            ad.set_show_toast(false);
            ad.set_reporte_edit_alert_show(false);
            ad.set_reporte_batch_edit_alert_show(false);
        }
    });
}

fn run_validate_new_cliente(ui: &MainWindow) {
    let app_data = ui.global::<AppData>();
    let rfc = app_data.get_new_cliente_rfc().to_string();
    let curp = app_data.get_new_cliente_curp().to_string();
    let tp = app_data.get_new_cliente_tipo_persona_val().to_string();
    let tp_opt = if tp.to_uppercase().contains("MORAL") {
        Some("MORAL")
    } else if tp.to_uppercase().contains("FISICA") || tp.to_uppercase().contains("FÍSICA") {
        Some("FISICA")
    } else {
        None
    };

    let clean_r = clean_alphanumeric_input(&rfc);
    if clean_r.is_empty() {
        app_data.set_new_cliente_rfc_status("".into());
        app_data.set_new_cliente_rfc_msg("".into());
    } else {
        match validate_rfc(&clean_r, tp_opt) {
            Ok(info) => {
                app_data.set_new_cliente_rfc_status("valid".into());
                let msg = if info.is_moral {
                    "RFC Moral válido (12 pos)".to_string()
                } else {
                    "RFC Físico válido (13 pos)".to_string()
                };
                app_data.set_new_cliente_rfc_msg(msg.into());
            }
            Err(e) => {
                app_data.set_new_cliente_rfc_status("invalid".into());
                app_data.set_new_cliente_rfc_msg(e.into());
            }
        }
    }

    let clean_c = clean_alphanumeric_input(&curp);
    if clean_c.is_empty() {
        app_data.set_new_cliente_curp_status("".into());
        app_data.set_new_cliente_curp_msg("".into());
    } else {
        if let Some("MORAL") = tp_opt {
            app_data.set_new_cliente_curp_status("invalid".into());
            app_data.set_new_cliente_curp_msg("Persona Moral no lleva CURP".into());
        } else {
            match validate_curp(&clean_c) {
                Ok(c_info) => {
                    if !clean_r.is_empty() && clean_r.len() == 13 {
                        if let Err(cross_err) = cross_validate_rfc_curp(&clean_r, &c_info.clean) {
                            app_data.set_new_cliente_curp_status("invalid".into());
                            app_data.set_new_cliente_curp_msg(cross_err.into());
                        } else {
                            app_data.set_new_cliente_curp_status("valid".into());
                            app_data.set_new_cliente_curp_msg("CURP válida (coincide con RFC)".into());
                        }
                    } else {
                        app_data.set_new_cliente_curp_status("valid".into());
                        app_data.set_new_cliente_curp_msg("CURP válida (18 pos)".into());
                    }
                }
                Err(e) => {
                    app_data.set_new_cliente_curp_status("invalid".into());
                    app_data.set_new_cliente_curp_msg(e.into());
                }
            }
        }
    }
}

fn run_validate_edit_cliente(ui: &MainWindow) {
    let app_data = ui.global::<AppData>();
    let rfc = app_data.get_edit_cliente_rfc().to_string();
    let curp = app_data.get_edit_cliente_curp().to_string();
    let tp = app_data.get_edit_cliente_tipo_persona_val().to_string();
    let tp_opt = if tp.to_uppercase().contains("MORAL") {
        Some("MORAL")
    } else if tp.to_uppercase().contains("FISICA") || tp.to_uppercase().contains("FÍSICA") {
        Some("FISICA")
    } else {
        None
    };

    let clean_r = clean_alphanumeric_input(&rfc);
    if clean_r.is_empty() {
        app_data.set_edit_cliente_rfc_status("".into());
        app_data.set_edit_cliente_rfc_msg("".into());
    } else {
        match validate_rfc(&clean_r, tp_opt) {
            Ok(info) => {
                app_data.set_edit_cliente_rfc_status("valid".into());
                let msg = if info.is_moral {
                    "RFC Moral válido (12 pos)".to_string()
                } else {
                    "RFC Físico válido (13 pos)".to_string()
                };
                app_data.set_edit_cliente_rfc_msg(msg.into());
            }
            Err(e) => {
                app_data.set_edit_cliente_rfc_status("invalid".into());
                app_data.set_edit_cliente_rfc_msg(e.into());
            }
        }
    }

    let clean_c = clean_alphanumeric_input(&curp);
    if clean_c.is_empty() {
        app_data.set_edit_cliente_curp_status("".into());
        app_data.set_edit_cliente_curp_msg("".into());
    } else {
        if let Some("MORAL") = tp_opt {
            app_data.set_edit_cliente_curp_status("invalid".into());
            app_data.set_edit_cliente_curp_msg("Persona Moral no lleva CURP".into());
        } else {
            match validate_curp(&clean_c) {
                Ok(c_info) => {
                    if !clean_r.is_empty() && clean_r.len() == 13 {
                        if let Err(cross_err) = cross_validate_rfc_curp(&clean_r, &c_info.clean) {
                            app_data.set_edit_cliente_curp_status("invalid".into());
                            app_data.set_edit_cliente_curp_msg(cross_err.into());
                        } else {
                            app_data.set_edit_cliente_curp_status("valid".into());
                            app_data.set_edit_cliente_curp_msg("CURP válida (coincide con RFC)".into());
                        }
                    } else {
                        app_data.set_edit_cliente_curp_status("valid".into());
                        app_data.set_edit_cliente_curp_msg("CURP válida (18 pos)".into());
                    }
                }
                Err(e) => {
                    app_data.set_edit_cliente_curp_status("invalid".into());
                    app_data.set_edit_cliente_curp_msg(e.into());
                }
            }
        }
    }
}

fn run_validate_empleado(ui: &MainWindow) {
    let app_data = ui.global::<AppData>();
    let rfc = app_data.get_emp_edit_rfc().to_string();
    let curp = app_data.get_emp_edit_curp().to_string();
    let nss = app_data.get_emp_edit_nss().to_string();
    let client_rfc = app_data.get_selected_cliente_rfc().to_string();

    let clean_r = clean_alphanumeric_input(&rfc);
    let clean_c = clean_alphanumeric_input(&curp);
    let clean_n = clean_alphanumeric_input(&nss);
    let clean_cli = clean_alphanumeric_input(&client_rfc);

    // RFC
    if clean_r.is_empty() {
        app_data.set_emp_edit_rfc_status("".into());
        app_data.set_emp_edit_rfc_msg("".into());
    } else if !clean_cli.is_empty() && clean_cli == clean_r {
        app_data.set_emp_edit_rfc_status("invalid".into());
        app_data.set_emp_edit_rfc_msg("Coincide con el RFC patronal".into());
    } else {
        match validate_rfc(&clean_r, Some("FISICA")) {
            Ok(_) => {
                app_data.set_emp_edit_rfc_status("valid".into());
                app_data.set_emp_edit_rfc_msg("RFC Físico válido (13 pos)".into());
            }
            Err(e) => {
                app_data.set_emp_edit_rfc_status("invalid".into());
                app_data.set_emp_edit_rfc_msg(e.into());
            }
        }
    }

    // NSS
    if clean_n.is_empty() {
        app_data.set_emp_edit_nss_status("".into());
        app_data.set_emp_edit_nss_msg("".into());
    } else {
        match validate_nss(&clean_n) {
            Ok(n_info) => {
                app_data.set_emp_edit_nss_status("valid".into());
                app_data.set_emp_edit_nss_msg(format!("NSS IMSS válido (Subdel {})", n_info.subdelegacion).into());
            }
            Err(e) => {
                app_data.set_emp_edit_nss_status("invalid".into());
                app_data.set_emp_edit_nss_msg(e.into());
            }
        }
    }

    // CURP
    if clean_c.is_empty() {
        app_data.set_emp_edit_curp_status("".into());
        app_data.set_emp_edit_curp_msg("".into());
    } else {
        match validate_curp(&clean_c) {
            Ok(_) => {
                app_data.set_emp_edit_curp_status("valid".into());
                app_data.set_emp_edit_curp_msg("CURP RENAPO válida (18 pos)".into());
            }
            Err(e) => {
                app_data.set_emp_edit_curp_status("invalid".into());
                app_data.set_emp_edit_curp_msg(e.into());
            }
        }
    }

    // Validaciones Cruzadas
    let mut warning = String::new();
    if !clean_r.is_empty() && !clean_c.is_empty() {
        if let Err(e) = cross_validate_rfc_curp(&clean_r, &clean_c) {
            warning = e;
        }
    }
    if warning.is_empty() && !clean_n.is_empty() && !clean_c.is_empty() {
        if let Err(e) = cross_validate_nss_curp(&clean_n, &clean_c) {
            warning = e;
        }
    }
    if warning.is_empty() && !clean_n.is_empty() && !clean_r.is_empty() {
        if let Err(e) = cross_validate_nss_rfc(&clean_n, &clean_r) {
            warning = e;
        }
    }
    app_data.set_emp_edit_cross_warning(warning.into());
}

fn run_validate_rp_form(ui: &MainWindow) {
    let app_data = ui.global::<AppData>();
    let raw = app_data.get_rp_form_registro().to_string();
    let clean = clean_alphanumeric_input(&raw);
    if clean.is_empty() {
        app_data.set_rp_form_registro_status("".into());
        app_data.set_rp_form_registro_msg("".into());
    } else {
        match validate_registro_patronal(&clean) {
            Ok(_) => {
                app_data.set_rp_form_registro_status("valid".into());
                app_data.set_rp_form_registro_msg("✓ Registro patronal con estructura válida (11 caracteres)".into());
            }
            Err(e) => {
                app_data.set_rp_form_registro_status("invalid".into());
                app_data.set_rp_form_registro_msg(e.into());
            }
        }
    }
}

fn run_validate_obra_form(ui: &MainWindow) {
    let app_data = ui.global::<AppData>();
    let raw = app_data.get_obra_form_numero().to_string();
    let clean = clean_alphanumeric_input(&raw);
    if clean.is_empty() {
        app_data.set_obra_form_numero_status("".into());
        app_data.set_obra_form_numero_msg("".into());
    } else {
        match validate_registro_obra(&clean) {
            Ok(info) => {
                app_data.set_obra_form_numero_status("valid".into());
                let tipo_str = match info.tipo {
                    RegistroObraTipo::Siroc => "SIROC",
                    RegistroObraTipo::Satic => "SATIC",
                };
                app_data.set_obra_form_numero_msg(format!("✓ Registro de obra válido ({})", tipo_str).into());
            }
            Err(e) => {
                app_data.set_obra_form_numero_status("invalid".into());
                app_data.set_obra_form_numero_msg(e.into());
            }
        }
    }
}

pub fn open_folder_in_os(path: &Path) {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let abs_path = if path.is_relative() {
            std::env::current_dir().unwrap_or_default().join(path)
        } else {
            path.to_path_buf()
        };
        let path_str = abs_path.to_string_lossy().to_string();
        let clean_path = path_str.strip_prefix(r"\\?\").unwrap_or(&path_str);
        let _ = std::process::Command::new("explorer").arg(clean_path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
}


fn load_clientes_ui(ui: &MainWindow, pool: &DbPool) {
    let mut client = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error obteniendo conexión para clientes: {}", e);
            return;
        }
    };

    let clientes = match fetch_clientes(&mut client) {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error consultando clientes: {}", e);
            return;
        }
    };

    let app_data = ui.global::<AppData>();
    let search = app_data.get_clientes_search().to_lowercase();
    let filter_tab = app_data.get_clientes_filter_tab();

    let filtered: Vec<ClienteCardData> = clientes
        .into_iter()
        .filter(|c| {
            if !search.is_empty() {
                let nom = c.nombre.to_lowercase();
                let rfc = c.rfc.to_lowercase();
                let rs = c.razon_social.to_lowercase();
                if !nom.contains(&search) && !rfc.contains(&search) && !rs.contains(&search) {
                    return false;
                }
            }
            if filter_tab == 1 && !c.has_ingested_data {
                return false;
            }
            if filter_tab == 2 && c.has_ingested_data {
                return false;
            }
            true
        })
        .map(|c| {
            let status_label = if c.has_ingested_data {
                "• Nóminas & Cédulas Vinculadas".into()
            } else {
                "• Recién Creado - Sin Cédulas".into()
            };
            let reg_str = if !c.regimen_fiscal_clave.is_empty() {
                format!("{} - {}", c.regimen_fiscal_clave, c.regimen_fiscal_nombre).into()
            } else {
                "".into()
            };
            ClienteCardData {
                id: c.id.into(),
                nombre: c.nombre.into(),
                razon_social: c.razon_social.into(),
                representante_legal: c.representante_legal.into(),
                rfc: c.rfc.into(),
                curp: c.curp.into(),
                tipo_persona: c.tipo_persona.into(),
                regimen_fiscal: reg_str,
                registros_patronales_count: c.registros_patronales_count,
                empleados_count: c.empleados_count,
                has_ingested_data: c.has_ingested_data,
                status_label,
                is_sat_locked: !c.has_ingested_data,
            }
        })
        .collect();

    app_data.set_clientes_list(ModelRc::new(VecModel::from(filtered)));
}

fn load_registros_patronales_ui(ui: &MainWindow, pool: &DbPool, client_id: &str) {
    if client_id.trim().is_empty() {
        ui.global::<AppData>().set_registros_patronales_list(ModelRc::new(VecModel::default()));
        return;
    }
    let mut client = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error obteniendo conexión para RP: {}", e);
            return;
        }
    };
    let rps = match fetch_registros_patronales_by_cliente(&mut client, client_id) {
        Ok(r) => r,
        Err(e) => {
            log::error!("Error consultando registros patronales: {}", e);
            return;
        }
    };

    let app_data = ui.global::<AppData>();
    let search = app_data.get_rp_search().to_lowercase();

    let models: Vec<RegistroPatronalData> = rps
        .into_iter()
        .filter(|rp| {
            if !search.is_empty() {
                let reg = rp.registro.to_lowercase();
                let act = rp.actividad.to_lowercase();
                let cl = rp.clase_nombre.to_lowercase();
                if !reg.contains(&search) && !act.contains(&search) && !cl.contains(&search) {
                    return false;
                }
            }
            true
        })
        .map(|rp| {
            let is_valid = validate_registro_patronal(&rp.registro).is_ok();
            RegistroPatronalData {
                id: rp.id.into(),
                id_cliente: rp.id_cliente.into(),
                cliente_nombre: rp.cliente_nombre.into(),
                registro: rp.registro.into(),
                actividad: rp.actividad.into(),
                domicilio: rp.domicilio.into(),
                codigo_postal: rp.codigo_postal.into(),
                entidad: rp.entidad.into(),
                area_geografica: rp.area_geografica.into(),
                delegacion: rp.delegacion.into(),
                subdelegacion: rp.subdelegacion.into(),
                prima_riesgo: format!("{:.5}", rp.prima_riesgo).into(),
                clase_nombre: rp.clase_nombre.into(),
                fraccion_codigo: rp.fraccion_codigo.into(),
                fraccion_actividad: rp.fraccion_actividad.into(),
                empleados_count: rp.empleados_count,
                has_ingested_data: rp.has_ingested_data,
                is_valid_structure: is_valid,
            }
        })
        .collect();

    app_data.set_registros_patronales_list(ModelRc::new(VecModel::from(models)));
}

fn load_empleados_ui(ui: &MainWindow, pool: &DbPool, client_id: &str) {
    if client_id.trim().is_empty() {
        ui.global::<AppData>().set_empleados_list(ModelRc::new(VecModel::default()));
        return;
    }
    let mut client = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error obteniendo conexión para empleados: {}", e);
            return;
        }
    };
    let emps = match fetch_empleados_by_cliente(&mut client, client_id) {
        Ok(e) => e,
        Err(e) => {
            log::error!("Error consultando empleados: {}", e);
            return;
        }
    };

    let app_data = ui.global::<AppData>();
    let search = app_data.get_empleados_search().to_lowercase();
    let only_active = app_data.get_empleados_filter_active_only();
    let filtro = app_data.get_empleados_filtro();

    let models: Vec<EmpleadoData> = emps
        .into_iter()
        .filter(|emp| {
            // D-8: el filtro de activos usa el estado efectivo (cédula / fijado a mano).
            let (activo_efectivo, _) = estado_empleado_efectivo(emp);
            if only_active && !activo_efectivo {
                return false;
            }
            match filtro {
                1 if activo_efectivo => return false,
                2 if emp.obras_actuales != 0 => return false,
                3 if emp.obras_actuales < 2 => return false,
                _ => {}
            }
            if !search.is_empty() {
                let n = emp.nombre.to_lowercase();
                let r = emp.rfc.to_lowercase();
                let c = emp.curp.to_lowercase();
                let s = emp.nss.to_lowercase();
                if !n.contains(&search) && !r.contains(&search) && !c.contains(&search) && !s.contains(&search) {
                    return false;
                }
            }
            true
        })
        .map(|emp| {
            let sbc_str = if emp.sbc_diario > 0.0 {
                format!("${:.2}", emp.sbc_diario)
            } else {
                "$0.00".to_string()
            };
            let (activo_efectivo, estado_texto) = estado_empleado_efectivo(&emp);
            EmpleadoData {
                estado_texto: estado_texto.into(),
                estado_es_baja: !activo_efectivo,
                estado_manual: emp.estado_manual,
                obras_actuales: emp.obras_actuales as i32,
                ultima_obra: emp.ultima_obra.clone().into(),
                id: emp.id.into(),
                id_cliente: emp.id_cliente.into(),
                cliente_nombre: emp.cliente_nombre.into(),
                nombre: emp.nombre.into(),
                rfc: emp.rfc.into(),
                curp: emp.curp.into(),
                nss: emp.nss.into(),
                registro_patronal: emp.registro_patronal.into(),
                sbc_diario_str: sbc_str.into(),
                pensionado: emp.pensionado.into(),
                esta_activo: emp.esta_activo,
            }
        })
        .collect();

    app_data.set_empleados_list(ModelRc::new(VecModel::from(models)));
}

fn sort_revisiones_vec(list: &mut [RevisionData], field: &str, asc: bool) {
    list.sort_by(|a, b| {
        let ord = match field {
            "Folio IMSS" => a.folio.to_lowercase().cmp(&b.folio.to_lowercase()),
            "Nombre" => a.nombre.to_lowercase().cmp(&b.nombre.to_lowercase()),
            "Fecha Entrega Revisión" => a.fecha_entrega_revision.cmp(&b.fecha_entrega_revision),
            "Fecha Entrega IMSS" => a.fecha_entrega_imss.cmp(&b.fecha_entrega_imss),
            "Fecha Aprobación IMSS" => a.fecha_aprobacion_imss.cmp(&b.fecha_aprobacion_imss),
            "Fecha Con Prórroga" => a.fecha_entrega_con_prorroga.cmp(&b.fecha_entrega_con_prorroga),
            "Multi-Criterio (Todos a la vez)" => {
                a.folio.to_lowercase().cmp(&b.folio.to_lowercase())
                    .then_with(|| a.nombre.to_lowercase().cmp(&b.nombre.to_lowercase()))
                    .then_with(|| a.fecha_entrega_revision.cmp(&b.fecha_entrega_revision))
                    .then_with(|| a.fecha_entrega_imss.cmp(&b.fecha_entrega_imss))
                    .then_with(|| a.created_at_str.cmp(&b.created_at_str))
            }
            _ => a.created_at_str.cmp(&b.created_at_str),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
}

fn load_revisiones_ui(ui: &MainWindow, pool: &DbPool, rp_id: &str) {
    if rp_id.trim().is_empty() {
        ui.global::<AppData>().set_revisiones_list(ModelRc::new(VecModel::default()));
        return;
    }
    let mut client = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error obteniendo conexión para revisiones: {}", e);
            return;
        }
    };

    let revs = match fetch_revisiones_by_rp(&mut client, rp_id) {
        Ok(r) => r,
        Err(e) => {
            log::error!("Error consultando revisiones: {}", e);
            return;
        }
    };

    let app_data = ui.global::<AppData>();
    let search = app_data.get_revisiones_search().to_lowercase();
    let filter_tab = app_data.get_revisiones_filter_tab();

    let mut vigentes_count = 0;
    let mut prorrogas_count = 0;

    for r in &revs {
        if r.pidio_prorroga {
            prorrogas_count += 1;
        }
        let est = r.estatus.to_uppercase();
        if !est.contains("FINALIZADA") && !est.contains("RECHAZADA") {
            vigentes_count += 1;
        }
    }

    app_data.set_revisiones_vigentes_count(vigentes_count);
    app_data.set_revisiones_prorrogas_count(prorrogas_count);

    let mut filtered: Vec<RevisionData> = revs
        .into_iter()
        .filter(|r| {
            match filter_tab {
                1 => {
                    let est = r.estatus.to_uppercase();
                    if !est.contains("PROCESO") && !est.contains("REVISIÓN") && !est.contains("INICIADA") {
                        return false;
                    }
                }
                2 => {
                    let est_imss = r.estatus_imss.to_uppercase();
                    if !est_imss.contains("APROBADA") {
                        return false;
                    }
                }
                3 => {
                    if !r.pidio_prorroga {
                        return false;
                    }
                }
                _ => {}
            }

            if !search.is_empty() {
                let nom = r.nombre.to_lowercase();
                let fol = r.folio.to_lowercase();
                let con = r.concepto_diferencia.to_lowercase();
                if !nom.contains(&search) && !fol.contains(&search) && !con.contains(&search) {
                    return false;
                }
            }
            true
        })
        .map(|r| {
            let id_short = if r.id.len() >= 8 {
                format!("#{}", &r.id[..4])
            } else {
                format!("#{}", r.id)
            };
            RevisionData {
                id: r.id.into(),
                id_registro_patronal: r.id_registro_patronal.into(),
                id_concepto_diferencia: r.id_concepto_diferencia.unwrap_or_default().into(),
                id_estatus_imss: r.id_estatus_imss.unwrap_or_default().into(),
                id_estatus: r.id_estatus.unwrap_or_default().into(),
                nombre: r.nombre.into(),
                folio: r.folio.into(),
                concepto_diferencia: r.concepto_diferencia.into(),
                estatus_imss: r.estatus_imss.into(),
                estatus: r.estatus.into(),
                fecha_entrega_imss: format_date_opt(r.fecha_entrega_imss).into(),
                fecha_aprobacion_imss: format_date_opt(r.fecha_aprobacion_imss).into(),
                fecha_maxima_aprobacion_imss: format_date_opt(r.fecha_maxima_aprobacion_imss).into(),
                fecha_maxima_entrega_revision: format_date_opt(r.fecha_maxima_entrega_revision).into(),
                fecha_maxima_entrega_imss: format_date_opt(r.fecha_maxima_entrega_imss).into(),
                fecha_entrega_revision: format_date_opt(r.fecha_entrega_revision).into(),
                fecha_aprobacion_prorroga: format_date_opt(r.fecha_aprobacion_prorroga).into(),
                fecha_entrega_con_prorroga: format_date_opt(r.fecha_entrega_con_prorroga).into(),
                periodo_inicio: r.periodo_inicio.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                periodo_fin: r.periodo_fin.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default().into(),
                pidio_prorroga: r.pidio_prorroga,
                obras_count: r.obras_count,
                created_at_str: r.created_at.into(),
                id_db_label: id_short.into(),
            }
        })
        .collect();

    let sort_field = app_data.get_revisiones_sort_field().to_string();
    let sort_asc = app_data.get_revisiones_sort_asc();
    sort_revisiones_vec(&mut filtered, &sort_field, sort_asc);

    app_data.set_revisiones_list(ModelRc::new(VecModel::from(filtered)));
}

fn load_obras_ui(ui: &MainWindow, pool: &DbPool, revision_id: &str) {
    if revision_id.trim().is_empty() {
        ui.global::<AppData>().set_obras_list(ModelRc::new(VecModel::default()));
        return;
    }
    let mut client = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error obteniendo conexión para obras: {}", e);
            return;
        }
    };

    let obras = match fetch_obras_by_revision(&mut client, revision_id) {
        Ok(o) => o,
        Err(e) => {
            log::error!("Error consultando registros de obras: {}", e);
            return;
        }
    };

    let app_data = ui.global::<AppData>();
    let search = app_data.get_obras_search().to_lowercase();
    let filter_tab = app_data.get_obras_filter_tab();

    let mut total_sup = 0.0;
    let mut total_monto = 0.0;

    for o in &obras {
        total_sup += o.superficie_construido;
        total_monto += o.monto.unwrap_or(0.0);
    }

    app_data.set_obras_superficie_total(total_sup as f32);
    app_data.set_obras_monto_total(total_monto as f32);
    app_data.set_obras_superficie_total_str(format!("{:.2} m²", total_sup).into());
    app_data.set_obras_monto_total_str(format!("${} MXN", format_monto_mexico(total_monto)).into());

    let filtered: Vec<RegistroObraData> = obras
        .into_iter()
        .filter(|o| {
            match filter_tab {
                1 => {
                    if !o.clase.to_uppercase().contains("PRIVADA") {
                        return false;
                    }
                }
                2 => {
                    let cl = o.clase.to_uppercase();
                    if !cl.contains("PUBLICA") && !cl.contains("PÚBLICA") {
                        return false;
                    }
                }
                _ => {}
            }

            if !search.is_empty() {
                let num = o.numero.to_lowercase();
                let cl = o.clase.to_lowercase();
                let tip = o.tipo_obra.to_lowercase();
                let ubi = o.ubicacion.to_lowercase();
                if !num.contains(&search) && !cl.contains(&search) && !tip.contains(&search) && !ubi.contains(&search) {
                    return false;
                }
            }
            true
        })
        .map(|o| {
            let is_valid = validate_registro_obra(&o.numero).is_ok();
            RegistroObraData {
                id: o.id.into(),
                id_revision: o.id_revision.into(),
                id_registro_patronal: o.id_registro_patronal.into(),
                numero: o.numero.into(),
                clase: o.clase.into(),
                tipo_obra: o.tipo_obra.into(),
                fecha_inicio: format_date_opt(o.fecha_inicio).into(),
                fecha_fin: format_date_opt(o.fecha_fin).into(),
                ubicacion: o.ubicacion.into(),
                superficie_construida: o.superficie_construido as f32,
                superficie_construida_str: format!("{:.2} m²", o.superficie_construido).into(),
                monto: o.monto.unwrap_or(0.0) as f32,
                monto_str: match o.monto {
                    Some(m) => format!("${} MXN", format_monto_mexico(m)).into(),
                    None => "Sin monto".into(),
                },
                delegacion: o.delegacion.into(),
                subdelegacion: o.subdelegacion.into(),
                created_at_str: o.created_at.into(),
                is_valid_structure: is_valid,
                creado_usuario: o.creado_usuario,
            }
        })
        .collect();

    app_data.set_obras_list(ModelRc::new(VecModel::from(filtered)));
}

fn update_global_stats(ui: &MainWindow, pool: &DbPool) {
    if let Ok(mut client) = pool.get() {
        let app_data = ui.global::<AppData>();

        // 1. Cargar combos de catálogos estáticos únicamente en la primera llamada
        let need_combos = app_data.get_tipos_personas_combo().row_count() == 0;
        if need_combos {
            if let Ok(bundle) = fetch_catalogos_bundle(&mut client) {
                app_data.set_catalogos_loaded_str("100% (9 Catálogos)".into());

                // Tipos personas combo
                let mut tp_strings = vec!["-- Seleccionar Tipo Persona --".to_string()];
                for tp in bundle.tipos_personas {
                    tp_strings.push(tp.tipo);
                }
                let tp_models: Vec<SharedString> = tp_strings.into_iter().map(|s| s.into()).collect();
                app_data.set_tipos_personas_combo(ModelRc::new(VecModel::from(tp_models)));

                // Clases combo
                let mut c_strings = vec!["-- Seleccionar Clase --".to_string()];
                for cl in bundle.clases {
                    c_strings.push(format!("Clase {} ({})", cl.nombre, cl.descripcion));
                }
                let c_models: Vec<SharedString> = c_strings.into_iter().map(|s| s.into()).collect();
                app_data.set_clases_combo(ModelRc::new(VecModel::from(c_models)));
            }

            // Estatus combos
            if let Ok(estatus_opts) = fetch_estatus_options(&mut client) {
                let mut e_strings = vec!["-- Seleccionar Estatus --".to_string()];
                for eo in estatus_opts {
                    e_strings.push(eo.nombre);
                }
                let e_models: Vec<SharedString> = e_strings.into_iter().map(|s| s.into()).collect();
                app_data.set_estatus_combo(ModelRc::new(VecModel::from(e_models.clone())));
                app_data.set_estatus_imss_combo(ModelRc::new(VecModel::from(e_models)));
            }

            // Conceptos combo
            if let Ok(conc_opts) = fetch_conceptos_options(&mut client) {
                let mut con_strings = vec!["-- Seleccionar Concepto Diferencia --".to_string()];
                for co in conc_opts {
                    con_strings.push(co.nombre);
                }
                let con_models: Vec<SharedString> = con_strings.into_iter().map(|s| s.into()).collect();
                app_data.set_conceptos_combo(ModelRc::new(VecModel::from(con_models)));
            }
        }

        // 2. Consulta única consolidada para todos los contadores globales (1 round-trip veloz)
        if let Ok(r) = client.query_one(
            r#"
            SELECT 
                (SELECT COUNT(*) FROM public.clientes)::int,
                (SELECT COUNT(*) FROM public.registros_patronales)::int,
                (SELECT COUNT(*) FROM public.empleados WHERE COALESCE(esta_activo, true) = true)::int,
                (SELECT COUNT(*) FROM public.revisiones)::int,
                (SELECT COUNT(*) FROM public.registros_obras WHERE COALESCE(creado_usuario, false) = true AND activa = true)::int
            "#,
            &[],
        ) {
            let cli_cnt: i32 = r.get(0);
            let rp_cnt: i32 = r.get(1);
            let emp_cnt: i32 = r.get(2);
            let rev_cnt: i32 = r.get(3);
            let obr_cnt: i32 = r.get(4);

            app_data.set_total_clientes_count(cli_cnt);
            app_data.set_total_rp_count(rp_cnt);
            app_data.set_total_empleados_activos_count(emp_cnt);
            app_data.set_total_revisiones_count(rev_cnt);
            app_data.set_total_obras_count(obr_cnt);
        }
    }
}

fn extract_clase_roman(s: &str) -> &'static str {
    let s = s.trim();
    if s.is_empty() || s.starts_with("--") {
        return "";
    }
    let text = if let Some(stripped) = s.strip_prefix("Clase ") {
        stripped.trim()
    } else {
        s
    };

    let token = text
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '-')
        .next()
        .unwrap_or("")
        .trim();

    match token {
        "I" => "I",
        "II" => "II",
        "III" => "III",
        "IV" => "IV",
        "V" => "V",
        _ => {
            if token.starts_with("III") {
                "III"
            } else if token.starts_with("II") {
                "II"
            } else if token.starts_with("IV") {
                "IV"
            } else if token.starts_with("I") {
                "I"
            } else if token.starts_with("V") {
                "V"
            } else {
                ""
            }
        }
    }
}

fn spawn_nominas_worker(
    ui_weak: slint::Weak<MainWindow>,
    nominas_paths: Vec<PathBuf>,
    last_nom: Arc<Mutex<Option<PathBuf>>>,
    pool_opt: Option<Arc<DbPool>>,
) {
    if nominas_paths.is_empty() {
        if let Some(ui) = ui_weak.upgrade() {
            show_toast_message(&ui, "Por favor agrega al menos una nómina para procesar.");
        }
        return;
    }

    if let Some(ui) = ui_weak.upgrade() {
        let app_data = ui.global::<AppData>();
        if app_data.get_nominas_is_processing() {
            return;
        }
        app_data.set_nominas_is_processing(true);
        app_data.set_nominas_ready_to_download(false);
        app_data.set_nominas_progress_fraction(0.01);
        app_data.set_nominas_progress_text("Iniciando análisis de nóminas...".into());
        app_data.set_is_processing(true);
        if app_data.get_cedulas_is_processing() {
            app_data.set_status_message("Procesando nóminas y cédulas en paralelo...".into());
        } else {
            app_data.set_status_message("Procesando nóminas...".into());
        }
    }

    let (nom_client_nom, nom_rp_reg, nom_rev_nom, nom_rev_id, nom_client_rfc) = if let Some(ui) = ui_weak.upgrade() {
        let app_data = ui.global::<AppData>();
        let c = app_data.get_selected_cliente_nombre().to_string();
        let rp = app_data.get_selected_rp_registro().to_string();
        let r = app_data.get_selected_revision_nombre().to_string();
        let rfc = app_data.get_selected_cliente_rfc().to_string();
        let rev_id_str = app_data.get_selected_revision_id().to_string();
        let rev_id = Uuid::parse_str(&rev_id_str).expect("Revision ID inválido: debe seleccionar una revisión válida en la UI");
        (
            if c.is_empty() || c == "Todos los Clientes" { "GENERAL".to_string() } else { c },
            if rp.is_empty() { "GENERAL".to_string() } else { rp },
            if r.is_empty() { "GENERAL".to_string() } else { r },
            rev_id,
            rfc,
        )
    } else {
        ("GENERAL".to_string(), "GENERAL".to_string(), "GENERAL".to_string(), Uuid::now_v7(), String::new())
    };

    let ui_w = ui_weak.clone();
    let pool_for_update = pool_opt.clone();
    thread::spawn(move || {
        // Subir nóminas a RustFS (Docker S3)
        for path in &nominas_paths {
            if let Some(fname) = path.file_name().and_then(|s| s.to_str()) {
                if let Ok(data) = fs::read(path) {
                    let _ = crate::rustfs::upload_file_to_rustfs(
                        &nom_client_nom,
                        &nom_rp_reg,
                        &nom_rev_nom,
                        "Nominas",
                        fname,
                        &data,
                    );
                }
            }
        }

        let temp_dir_handle = tempfile::Builder::new()
            .prefix("docu_ffacd_nom_")
            .tempdir();
        let temp_dir = match temp_dir_handle {
            Ok(ref d) => d.path().to_path_buf(),
            Err(_) => std::env::temp_dir(),
        };

        let output_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("output");
        let _ = fs::create_dir_all(&output_dir);
        let nominas_out_path = output_dir.join("reporte_nominas.xlsx");

        let ui_for_cb = ui_w.clone();
        let last_time = Arc::new(Mutex::new(std::time::Instant::now()));

        let progress_cb = move |current: usize, total: usize, msg: &str| {
            let frac = if total > 0 {
                (current as f32 / total as f32).clamp(0.01, 0.99)
            } else {
                0.03
            };
            let msg_str = msg.to_string();

            let should_update = {
                let mut lt = last_time.lock().unwrap();
                if lt.elapsed() >= std::time::Duration::from_millis(50) || current == total || current <= 1 {
                    *lt = std::time::Instant::now();
                    true
                } else {
                    false
                }
            };

            if should_update {
                let ui_event = ui_for_cb.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_event.upgrade() {
                        let app_data = ui.global::<AppData>();
                        app_data.set_nominas_progress_fraction(frac);
                        app_data.set_nominas_progress_text(msg_str.into());
                    }
                });
            }
        };

        let exp_rfc_opt = if !nom_client_rfc.trim().is_empty() { Some(nom_client_rfc.as_str()) } else { None };
        let exp_rp_opt = if nom_rp_reg != "GENERAL" && !nom_rp_reg.trim().is_empty() { Some(nom_rp_reg.as_str()) } else { None };
        let res = nominas::processor::process_nominas(&nominas_paths, &nominas_out_path, &temp_dir, progress_cb, nom_rev_id, exp_rfc_opt, exp_rp_opt);

        let ui_event = ui_w.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_event.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_nominas_is_processing(false);

                let ced_running = app_data.get_cedulas_is_processing();
                app_data.set_is_processing(ced_running);
                if ced_running {
                    app_data.set_status_message("Procesando cédulas...".into());
                } else {
                    app_data.set_status_message("Proceso completado".into());
                }

                match res {
                    Ok(outcome) => {
                        let has_excel = outcome.excel_path.is_some();
                        if let Some(ref p) = outcome.excel_path {
                            if let Ok(mut g) = last_nom.lock() {
                                *g = Some(p.clone());
                            }
                        }

                        app_data.set_nominas_progress_fraction(1.0);
                        app_data.set_nominas_progress_text(
                            format!("{}/{} procesadas ({} válidas SAT)", outcome.total_files, outcome.total_files, outcome.valid_count).into()
                        );
                        app_data.set_nominas_ready_to_download(has_excel);
                        app_data.set_has_results(true);

                        let mut curr_sum = app_data.get_summary();
                        curr_sum.nominas_total = outcome.total_files as i32;
                        curr_sum.nominas_valid = outcome.valid_count as i32;
                        curr_sum.nominas_invalid = outcome.invalid_count as i32;
                        curr_sum.has_nominas_excel = has_excel;
                        curr_sum.nominas_excel_path = outcome
                            .excel_path
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default()
                            .into();
                        app_data.set_summary(curr_sum);

                        let mut current_unproc: Vec<UnprocessedItemData> = app_data
                            .get_unprocessed_items()
                            .iter()
                            .filter(|it| !it.category.starts_with("NÓMINA"))
                            .collect();

                        for inv in outcome.invalid_items {
                            let cat_label = if inv.category.is_empty() {
                                "NÓMINA".to_string()
                            } else {
                                format!("NÓMINA - {}", inv.category)
                            };
                            current_unproc.push(UnprocessedItemData {
                                category: cat_label.into(),
                                filename: inv.filename.into(),
                                uuid: inv.uuid.into(),
                                reason: inv.reason.into(),
                                is_sat_error: inv.is_sat_error,
                            });
                        }
                        app_data.set_unprocessed_items(ModelRc::new(VecModel::from(current_unproc)));

                        show_toast_message(&ui, &format!("¡Nóminas procesadas: {} válidas!", outcome.valid_count));
                        if let Some(ref p) = pool_for_update {
                            load_clientes_ui(&ui, p);
                            update_global_stats(&ui, p);
                        }
                    }
                    Err(e) => {
                        app_data.set_nominas_progress_fraction(0.0);
                        app_data.set_nominas_progress_text(format!("Error: {}", e).into());
                        show_toast_message(&ui, &format!("Error al procesar nóminas: {}", e));
                    }
                }
            }
        });
    });
}

fn spawn_cedulas_worker(
    ui_weak: slint::Weak<MainWindow>,
    cedulas_paths: Vec<PathBuf>,
    last_ced: Arc<Mutex<Option<PathBuf>>>,
    pool_opt: Option<Arc<DbPool>>,
) {
    if cedulas_paths.is_empty() {
        if let Some(ui) = ui_weak.upgrade() {
            show_toast_message(&ui, "Por favor agrega al menos una cédula para procesar.");
        }
        return;
    }

    if let Some(ui) = ui_weak.upgrade() {
        let app_data = ui.global::<AppData>();
        if app_data.get_cedulas_is_processing() {
            return;
        }
        app_data.set_cedulas_is_processing(true);
        app_data.set_cedulas_ready_to_download(false);
        app_data.set_cedulas_progress_fraction(0.01);
        app_data.set_cedulas_progress_text("Iniciando análisis de cédulas...".into());
        app_data.set_is_processing(true);
        if app_data.get_nominas_is_processing() {
            app_data.set_status_message("Procesando nóminas y cédulas en paralelo...".into());
        } else {
            app_data.set_status_message("Procesando cédulas...".into());
        }
    }

    let ui_w = ui_weak.clone();
    let pool_for_update = pool_opt.clone();
    thread::spawn(move || {
        let temp_dir_handle = tempfile::Builder::new()
            .prefix("docu_ffacd_ced_")
            .tempdir();
        let temp_dir = match temp_dir_handle {
            Ok(ref d) => d.path().to_path_buf(),
            Err(_) => std::env::temp_dir(),
        };

        let output_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("output");
        let _ = fs::create_dir_all(&output_dir);
        let cedulas_out_path = output_dir.join("reporte_liquidaciones.xlsx");

        let ui_for_cb = ui_w.clone();
        let last_time = Arc::new(Mutex::new(std::time::Instant::now()));

        let progress_cb = move |current: usize, total: usize, msg: &str| {
            let frac = if total > 0 {
                (current as f32 / total as f32).clamp(0.01, 0.99)
            } else {
                0.03
            };
            let msg_str = msg.to_string();

            let should_update = {
                let mut lt = last_time.lock().unwrap();
                if lt.elapsed() >= std::time::Duration::from_millis(50) || current == total || current <= 1 {
                    *lt = std::time::Instant::now();
                    true
                } else {
                    false
                }
            };

            if should_update {
                let ui_event = ui_for_cb.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_event.upgrade() {
                        let app_data = ui.global::<AppData>();
                        app_data.set_cedulas_progress_fraction(frac);
                        app_data.set_cedulas_progress_text(msg_str.into());
                    }
                });
            }
        };

        let res = cedulas::processor::process_cedulas(&cedulas_paths, &cedulas_out_path, &temp_dir, progress_cb);

        let ui_event = ui_w.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_event.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_cedulas_is_processing(false);

                let nom_running = app_data.get_nominas_is_processing();
                app_data.set_is_processing(nom_running);
                if nom_running {
                    app_data.set_status_message("Procesando nóminas...".into());
                } else {
                    app_data.set_status_message("Proceso completado".into());
                }

                match res {
                    Ok(outcome) => {
                        let has_excel = outcome.excel_path.is_some();
                        if let Some(ref p) = outcome.excel_path {
                            if let Ok(mut g) = last_ced.lock() {
                                *g = Some(p.clone());
                            }
                        }

                        app_data.set_cedulas_progress_fraction(1.0);
                        app_data.set_cedulas_progress_text(
                            format!("{}/{} analizadas ({} válidas SUA)", outcome.total_files, outcome.total_files, outcome.valid_count).into()
                        );
                        app_data.set_cedulas_ready_to_download(has_excel);
                        app_data.set_has_results(true);

                        let mut curr_sum = app_data.get_summary();
                        curr_sum.cedulas_total = outcome.total_files as i32;
                        curr_sum.cedulas_valid = outcome.valid_count as i32;
                        curr_sum.cedulas_invalid = outcome.invalid_count as i32;
                        curr_sum.has_cedulas_excel = has_excel;
                        curr_sum.cedulas_excel_path = outcome
                            .excel_path
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default()
                            .into();
                        app_data.set_summary(curr_sum);

                        let mut current_unproc: Vec<UnprocessedItemData> = app_data
                            .get_unprocessed_items()
                            .iter()
                            .filter(|it| !it.category.starts_with("CÉDULA"))
                            .collect();

                        for inv in outcome.invalid_items {
                            let cat_label = if inv.category.is_empty() {
                                "CÉDULA".to_string()
                            } else {
                                format!("CÉDULA - {}", inv.category)
                            };
                            current_unproc.push(UnprocessedItemData {
                                category: cat_label.into(),
                                filename: inv.filename.into(),
                                uuid: inv.identifier.into(),
                                reason: inv.reason.into(),
                                is_sat_error: false,
                            });
                        }
                        app_data.set_unprocessed_items(ModelRc::new(VecModel::from(current_unproc)));

                        show_toast_message(&ui, &format!("¡Cédulas procesadas: {} válidas!", outcome.valid_count));
                        if let Some(ref p) = pool_for_update {
                            load_clientes_ui(&ui, p);
                            update_global_stats(&ui, p);
                        }
                    }
                    Err(e) => {
                        app_data.set_cedulas_progress_fraction(0.0);
                        app_data.set_cedulas_progress_text(format!("Error: {}", e).into());
                        show_toast_message(&ui, &format!("Error al procesar cédulas: {}", e));
                    }
                }
            }
        });
    });
}

/// Archivo nuevo de cédulas subido a staging en este lote, pendiente de registrar/guardar.
#[derive(Debug, Clone)]
pub struct ArchivoPendienteCedula {
    /// final_fname con hash: coincide con el file_name que el parser asigna a los trabajadores
    pub nombre_archivo: String,
    pub nombre_original: String,
    pub sha256: String,
    pub anio: i16,
    pub mes: i16,
    /// Clave definitiva en RustFS (sin prefijo .staging_)
    pub clave_final: String,
    /// Clave en RustFS dentro del prefijo .staging_<batch_id>/
    pub clave_staging: String,
}

#[derive(Clone, Default)]
pub struct CedulasViewState {
    pub files: Vec<CedulaFileCardInfo>,
    pub audit_res: Option<CedulasAuditResult>,
    pub businesses: Vec<crate::cedulas::business::Business>,
    pub filtered_details: Vec<DetalleRowModel>,
    pub current_page: usize,
    pub search_query: String,
    pub mes_filter_idx: usize,
    pub obra_filter_idx: usize,
    pub mes_options: Vec<String>,
    pub obra_options: Vec<String>,
    pub current_revision_id: Option<String>,
    pub current_rp_id: Option<String>,
    pub pending_staging_batch_id: Option<String>,
    pub pending_staged_local_paths: Vec<PathBuf>,
    pub pending_overwrite_months: Vec<(String, String)>,
    pub pending_overwrite_old_files: Vec<String>,
    /// Tarjetas de archivos previos ocultadas por un posible reemplazo; se restauran si se elige "Fusionar"
    pub pending_overwrite_old_cards: Vec<CedulaFileCardInfo>,
    /// Cliente, registro patronal y revisión usados al subir a staging (necesarios para revertirlo)
    pub pending_staging_ctx: Option<(String, String, String)>,
    /// Archivos nuevos en staging de la última subida, pendientes de registrar en BD y guardar
    pub pending_archivos: Vec<ArchivoPendienteCedula>,
}

fn update_cedulas_view_file_list_ui(ui: &MainWindow, state: &CedulasViewState) {
    let app_data = ui.global::<AppData>();
    let items: Vec<CedulaFileItem> = state.files.iter().map(|f| {
        CedulaFileItem {
            id: f.id.clone().into(),
            filename: f.filename.clone().into(),
            file_type: f.file_type.clone().into(),
            size_str: f.size_str.clone().into(),
            pages_detected: f.pages_detected.clone().into(),
            status_badge: f.status_badge.clone().into(),
            status_is_valid: f.status_is_valid,
            checksum: f.checksum.clone().into(),
            has_warning: f.has_warning,
            warning_msg: f.warning_msg.clone().into(),
        }
    }).collect();

    let total_bytes: u64 = state.files.iter().map(|f| {
        std::fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0)
    }).sum();
    let total_size_str = format_filesize(total_bytes);
    let warning_count = state.files.iter().filter(|f| f.has_warning).count() as i32;

    app_data.set_cedulas_view_files(ModelRc::new(VecModel::from(items)));
    app_data.set_cedulas_view_files_count(state.files.len() as i32);
    app_data.set_cedulas_view_total_size_str(total_size_str.into());
    app_data.set_cedulas_view_warning_count(warning_count);

    if state.files.is_empty() && state.audit_res.is_none() {
        app_data.set_cedulas_view_state(0);
        app_data.set_cedulas_view_status_banner_text("Sin archivos en memoria temporal".into());
    } else if state.audit_res.is_none() {
        app_data.set_cedulas_view_state(1);
        let banner = format!(
            "{} archivo(s) en memoria de análisis temporal | {} archivo(s) con advertencia",
            state.files.len(),
            warning_count
        );
        app_data.set_cedulas_view_status_banner_text(banner.into());
    }
}

fn apply_cedulas_details_filtering(state: &mut CedulasViewState) {
    let Some(ref audit) = state.audit_res else {
        state.filtered_details.clear();
        return;
    };

    let query = state.search_query.trim().to_lowercase();
    let selected_mes = if state.mes_filter_idx > 0 && state.mes_filter_idx < state.mes_options.len() {
        Some(&state.mes_options[state.mes_filter_idx])
    } else {
        None
    };

    let selected_obra = if state.obra_filter_idx > 0 && state.obra_filter_idx < state.obra_options.len() {
        Some(&state.obra_options[state.obra_filter_idx])
    } else {
        None
    };

    state.filtered_details = audit.detalle_rows.iter().filter(|row| {
        if !query.is_empty() {
            let n = row.nombre_trabajador.to_lowercase();
            let nss = row.nss.to_lowercase();
            let rc = row.rfc_curp.to_lowercase();
            let rp = row.registro_patronal.to_lowercase();
            let ro = row.registro_obra.to_lowercase();
            if !n.contains(&query) && !nss.contains(&query) && !rc.contains(&query) && !rp.contains(&query) && !ro.contains(&query) {
                return false;
            }
        }

        if let Some(mes) = selected_mes {
            if !row.mes_anio.contains(mes.as_str()) && !mes.contains(row.mes_anio.as_str()) {
                return false;
            }
        }

        if let Some(obra) = selected_obra {
            if &row.registro_obra != obra {
                return false;
            }
        }

        true
    }).cloned().collect();

    state.current_page = 1;
}

fn render_cedulas_details_page_ui(ui: &MainWindow, state: &CedulasViewState) {
    let app_data = ui.global::<AppData>();
    let total_rows = state.filtered_details.len();
    let page_size = 100;
    let total_pages = if total_rows == 0 {
        1
    } else {
        ((total_rows as f64) / (page_size as f64)).ceil() as usize
    };

    let current_page = state.current_page.clamp(1, total_pages);
    let start_idx = if total_rows == 0 { 0 } else { (current_page - 1) * page_size };
    let end_idx = (start_idx + page_size).min(total_rows);

    let page_rows: Vec<CedulaDetalleRow> = if total_rows > 0 && start_idx < total_rows {
        state.filtered_details[start_idx..end_idx].iter().map(|r| {
            CedulaDetalleRow {
                idx: r.idx,
                reg_num_str: r.reg_num_str.clone().into(),
                mes_anio: r.mes_anio.clone().into(),
                registro_patronal: r.registro_patronal.clone().into(),
                registro_obra: r.registro_obra.clone().into(),
                nss: r.nss.clone().into(),
                rfc_curp: r.rfc_curp.clone().into(),
                nombre_trabajador: r.nombre_trabajador.clone().into(),
                dias: r.dias,
                sdi_str: r.sdi_str.clone().into(),
                cuota_patronal_str: r.cuota_patronal_str.clone().into(),
                cuota_obrera_str: r.cuota_obrera_str.clone().into(),
                total_str: r.total_str.clone().into(),
            }
        }).collect()
    } else {
        Vec::new()
    };

    app_data.set_cedulas_detalle_list(ModelRc::new(VecModel::from(page_rows)));
    app_data.set_cedulas_detalle_total_count(total_rows as i32);
    app_data.set_cedulas_page_current(current_page as i32);
    app_data.set_cedulas_page_total(total_pages as i32);
    app_data.set_cedulas_page_start_row(if total_rows > 0 { (start_idx + 1) as i32 } else { 0 });
    app_data.set_cedulas_page_end_row(end_idx as i32);
}

fn update_cedulas_audit_view_ui(ui: &MainWindow, state: &mut CedulasViewState) {
    let audit = match state.audit_res {
        Some(ref a) => a.clone(),
        None => return,
    };

    let app_data = ui.global::<AppData>();

    // 1. KPIs principales de archivos procesados
    app_data.set_cedulas_kpi_total_analizados(audit.total_archivos_analizados as i32);
    app_data.set_cedulas_kpi_validos_sua(audit.validos_sua as i32);
    app_data.set_cedulas_kpi_validos_pct(audit.validos_pct.clone().into());
    app_data.set_cedulas_kpi_no_procesados(audit.no_procesados as i32);
    app_data.set_cedulas_kpi_no_procesados_pct(audit.no_procesados_pct.clone().into());
    app_data.set_cedulas_kpi_no_procesados_desc(audit.no_procesados_desc.clone().into());
    app_data.set_cedulas_kpi_trabajadores_unicos(audit.trabajadores_unicos as i32);
    app_data.set_cedulas_kpi_repeticiones_mensuales(audit.repeticiones_mensuales as i32);
    app_data.set_cedulas_kpi_importe_total_str(audit.importe_total_str.clone().into());
    app_data.set_cedulas_kpi_patronal_str(audit.cuota_patronal_str.clone().into());
    app_data.set_cedulas_kpi_obrera_str(audit.cuota_obrera_str.clone().into());

    // 2. Auditoría avanzada
    app_data.set_cedulas_kpi_superposiciones_count(audit.superposiciones_count as i32);
    app_data.set_cedulas_kpi_obras_fuera_revision_count(audit.obras_fuera_revision_count as i32);
    app_data.set_cedulas_kpi_empleados_obras_fuera_count(audit.empleados_obras_fuera_count as i32);
    app_data.set_cedulas_kpi_archivos_cliente_invalido_count(audit.archivos_cliente_invalido_count as i32);
    app_data.set_cedulas_kpi_sin_obras_revision_warning(audit.sin_obras_revision_warning);

    // 3. Rango de periodos
    app_data.set_cedulas_periodo_rango_str(audit.periodo_rango_str.clone().into());
    app_data.set_cedulas_periodo_sub_str(audit.periodo_sub_str.clone().into());
    app_data.set_cedulas_mensuales_count(audit.mensuales_count as i32);
    app_data.set_cedulas_bimestrales_count(audit.bimestrales_count as i32);

    // 4. SBC promedio ponderado
    app_data.set_cedulas_sbc_promedio_str(audit.sbc_promedio_str.clone().into());
    app_data.set_cedulas_sbc_sub_str(audit.sbc_sub_str.clone().into());
    app_data.set_cedulas_sbc_min_str(audit.sbc_min_str.clone().into());
    app_data.set_cedulas_sbc_max_str(audit.sbc_max_str.clone().into());

    // 5. Ramas de cuotas
    app_data.set_cedulas_ramas_rt_str(audit.ramas_rt_str.clone().into());
    app_data.set_cedulas_ramas_sub_str(audit.ramas_sub_str.clone().into());

    // 6. Totales y sumatorias
    let sumatorias: Vec<CedulaSumatoriaRow> = audit.sumatorias_rows.iter().map(|s| {
        CedulaSumatoriaRow {
            registro_patronal: s.registro_patronal.clone().into(),
            razon_social: s.razon_social.clone().into(),
            anio: s.anio.clone().into(),
            mes: s.mes.clone().into(),
            registro_obra: s.registro_obra.clone().into(),
            trabajadores_count: s.trabajadores_count,
            total_dias: s.total_dias,
            cuota_patronal_str: s.cuota_patronal_str.clone().into(),
            cuota_obrera_str: s.cuota_obrera_str.clone().into(),
            total_liquidacion_str: s.total_liquidacion_str.clone().into(),
        }
    }).collect();
    app_data.set_cedulas_sumatorias_list(ModelRc::new(VecModel::from(sumatorias)));
    app_data.set_cedulas_gran_total_trabajadores(audit.gran_total_trabajadores as i32);
    app_data.set_cedulas_gran_total_dias(audit.gran_total_dias);
    app_data.set_cedulas_gran_total_patronal_str(audit.gran_total_patronal_str.clone().into());
    app_data.set_cedulas_gran_total_obrera_str(audit.gran_total_obrera_str.clone().into());
    app_data.set_cedulas_gran_total_str(audit.gran_total_str.clone().into());

    // 7. Filtros desplegables
    let mut mes_set = HashSet::new();
    let mut obra_set = HashSet::new();
    for row in &audit.detalle_rows {
        if !row.mes_anio.is_empty() {
            mes_set.insert(row.mes_anio.clone());
        }
        if !row.registro_obra.is_empty() && row.registro_obra != "SIN REGISTRO DE OBRA" {
            obra_set.insert(row.registro_obra.clone());
        }
    }
    let mut mes_vec: Vec<String> = mes_set.into_iter().collect();
    mes_vec.sort();
    let mut obra_vec: Vec<String> = obra_set.into_iter().collect();
    obra_vec.sort();

    let mut mes_opts = vec!["Todos los Meses".to_string()];
    mes_opts.extend(mes_vec);
    let mut obra_opts = vec!["Todas las Obras".to_string()];
    obra_opts.extend(obra_vec);

    state.mes_options = mes_opts.clone();
    state.obra_options = obra_opts.clone();

    let mes_slint: Vec<SharedString> = mes_opts.into_iter().map(SharedString::from).collect();
    let obra_slint: Vec<SharedString> = obra_opts.into_iter().map(SharedString::from).collect();
    app_data.set_cedulas_detalle_mes_options(ModelRc::new(VecModel::from(mes_slint)));
    app_data.set_cedulas_detalle_obra_options(ModelRc::new(VecModel::from(obra_slint)));

    // 8. Filtrado y renderizado inicial
    apply_cedulas_details_filtering(state);
    render_cedulas_details_page_ui(ui, state);

    // 9. Cambiar estado a 2 (Processed & Saved)
    app_data.set_cedulas_view_state(2);
    app_data.set_cedulas_view_extracted_count_str(format!("{} Cédulas Extraídas", audit.validos_sua).into());
}

fn reset_cedulas_ui_to_state_zero(ui: &MainWindow, session: &mut CedulasViewState) {
    session.files.clear();
    session.audit_res = None;
    session.businesses.clear();
    session.filtered_details.clear();
    session.current_page = 1;
    session.search_query.clear();
    session.mes_filter_idx = 0;
    session.obra_filter_idx = 0;
    session.mes_options.clear();
    session.obra_options.clear();
    session.current_revision_id = None;
    session.current_rp_id = None;

    let app_data = ui.global::<AppData>();
    app_data.set_show_delete_all_cedulas_modal(false);
    app_data.set_cedulas_view_state(0);
    app_data.set_cedulas_view_files(ModelRc::new(VecModel::default()));
    app_data.set_cedulas_view_files_count(0);
    app_data.set_cedulas_view_total_size_str("0.0 MB".into());
    app_data.set_cedulas_view_extracted_count_str("0 Cédulas Extraídas".into());
    app_data.set_cedulas_view_warning_count(0);
    app_data.set_cedulas_view_status_banner_text("Sin archivos en memoria temporal".into());
    app_data.set_cedulas_view_is_processing(false);
    app_data.set_cedulas_view_progress_fraction(0.0);
    app_data.set_cedulas_view_progress_text("".into());

    app_data.set_cedulas_detalle_list(ModelRc::new(VecModel::default()));
    app_data.set_cedulas_detalle_total_count(0);
    app_data.set_cedulas_sumatorias_list(ModelRc::new(VecModel::default()));

    app_data.set_cedulas_kpi_total_analizados(0);
    app_data.set_cedulas_kpi_validos_sua(0);
    app_data.set_cedulas_kpi_no_procesados(0);
    app_data.set_cedulas_kpi_trabajadores_unicos(0);
    app_data.set_cedulas_kpi_importe_total_str("$0.00 MXN".into());
    app_data.set_cedulas_kpi_patronal_str("$0.00".into());
    app_data.set_cedulas_kpi_obrera_str("$0.00".into());
    app_data.set_cedulas_periodo_rango_str("N/D".into());
    app_data.set_cedulas_periodo_sub_str("".into());
    app_data.set_cedulas_mensuales_count(0);
    app_data.set_cedulas_bimestrales_count(0);
    app_data.set_cedulas_sbc_promedio_str("$0.00 MXN / día".into());
    app_data.set_cedulas_sbc_sub_str("".into());
    app_data.set_cedulas_sbc_min_str("$0.00".into());
    app_data.set_cedulas_sbc_max_str("$0.00".into());
    app_data.set_cedulas_ramas_rt_str("RT: 0.00000%".into());
    app_data.set_cedulas_ramas_sub_str("".into());
    app_data.set_reporte_has_missing_cedulas_warning(false);
    app_data.set_reporte_missing_cedulas_msg("".into());
    app_data.set_reporte_missing_cedulas_reason("".into());
    app_data.set_reporte_missing_cedulas_meses(ModelRc::new(VecModel::default()));
    app_data.set_reporte_inconsistencias_detalles(ModelRc::new(VecModel::default()));
    app_data.set_reporte_show_missing_cedulas_details(false);
    app_data.set_progress_percentage(0.0);
    app_data.set_current_file_name("".into());
    app_data.set_processed_count(0);
    app_data.set_total_files_count(0);
}

#[derive(Debug, Clone)]
struct CedulaProgressUpdate {
    processed_count: i32,
    total_count: i32,
    progress_percentage: f32,
    current_file_name: String,
    status_text: String,
}

fn get_tokio_handle() -> &'static tokio::runtime::Handle {
    static TOKIO_HANDLE: std::sync::OnceLock<tokio::runtime::Handle> = std::sync::OnceLock::new();
    TOKIO_HANDLE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("tokio-cedulas-runtime".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Error inicializando runtime asíncrono de Tokio");
                let _ = tx.send(rt.handle().clone());
                rt.block_on(std::future::pending::<()>());
            })
            .expect("Error creando hilo para runtime de Tokio");
        rx.recv().expect("Error obteniendo Handle de Tokio")
    })
}

fn handle_cedulas_upload_and_inspect(
    ui_weak: slint::Weak<MainWindow>,
    session_clone: Arc<Mutex<CedulasViewState>>,
    pool_clone: Option<Arc<DbPool>>,
    paths: Vec<PathBuf>,
) {
    if paths.is_empty() {
        return;
    }

    let (cliente_nombre, rp_registro, revision_nombre, revision_id, rp_id) = if let Some(ui) = ui_weak.upgrade() {
        let app_data = ui.global::<AppData>();
        app_data.set_cedulas_view_is_processing(true);
        app_data.set_processed_count(0);
        app_data.set_total_files_count(paths.len() as i32);
        app_data.set_progress_percentage(1.0);
        app_data.set_cedulas_view_progress_fraction(0.01);
        app_data.set_current_file_name("Escaneando y preparando archivos seleccionados...".into());
        app_data.set_cedulas_view_progress_text("Escaneando y preparando archivos seleccionados...".into());
        (
            app_data.get_selected_cliente_nombre().to_string(),
            app_data.get_selected_rp_registro().to_string(),
            app_data.get_selected_revision_nombre().to_string(),
            app_data.get_selected_revision_id().to_string(),
            app_data.get_selected_rp_id().to_string(),
        )
    } else {
        return;
    };

    let (prog_tx, mut prog_rx) = tokio::sync::mpsc::unbounded_channel::<CedulaProgressUpdate>();
    let ui_for_rx = ui_weak.clone();

    get_tokio_handle().spawn(async move {
        while let Some(upd) = prog_rx.recv().await {
            let ui_clone = ui_for_rx.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_clone.upgrade() {
                    let app_data = ui.global::<AppData>();
                    app_data.set_cedulas_view_is_processing(true);
                    app_data.set_processed_count(upd.processed_count);
                    app_data.set_total_files_count(upd.total_count);
                    app_data.set_progress_percentage(upd.progress_percentage);
                    app_data.set_cedulas_view_progress_fraction(upd.progress_percentage / 100.0);
                    app_data.set_current_file_name(upd.current_file_name.into());
                    app_data.set_cedulas_view_progress_text(upd.status_text.into());
                }
            });
        }
    });

    get_tokio_handle().spawn_blocking(move || {
        let mut c_nom = cliente_nombre;
        let mut r_reg = rp_registro;
        let mut rev_nom = revision_nombre;
        let mut c_rfc = String::new();

        if let Some(ref pool) = pool_clone
            && let Ok(mut client) = pool.get()
        {
            if let Ok(rev_uuid) = uuid::Uuid::parse_str(&revision_id) {
                if let Ok(Some(row)) = client.query_opt(
                    "SELECT c.nombre, rp.registro, r.nombre, c.rfc
                     FROM revisiones r
                     JOIN registros_patronales rp ON r.id_registro_patronal = rp.id
                     JOIN clientes c ON rp.id_cliente = c.id
                     WHERE r.id = $1",
                    &[&rev_uuid],
                ) {
                    c_nom = row.get(0);
                    r_reg = row.get(1);
                    rev_nom = row.get(2);
                    c_rfc = row.get::<_, Option<String>>(3).unwrap_or_default();
                }
            } else if let Ok(rp_uuid) = uuid::Uuid::parse_str(&rp_id)
                && let Ok(Some(row)) = client.query_opt(
                    "SELECT c.nombre, rp.registro, 'GENERAL', c.rfc
                     FROM registros_patronales rp
                     JOIN clientes c ON rp.id_cliente = c.id
                     WHERE rp.id = $1",
                    &[&rp_uuid],
                )
            {
                c_nom = row.get(0);
                r_reg = row.get(1);
                rev_nom = row.get(2);
                c_rfc = row.get::<_, Option<String>>(3).unwrap_or_default();
            }
        }

        let temp_dir_handle = tempfile::Builder::new().prefix("docu_ffacd_upload_").tempdir();
        let temp_dir = match temp_dir_handle {
            Ok(ref d) => d.path().to_path_buf(),
            Err(_) => std::env::temp_dir(),
        };

        let items = match crate::archives::collect_prepared_upload_items(&paths, "pdf", &temp_dir) {
            Ok(it) => it,
            Err(e) => {
                log::error!("Error preparando archivos para subida: {}", e);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.global::<AppData>().set_cedulas_view_is_processing(false);
                    }
                });
                return;
            }
        };

        if items.is_empty() {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    let app_data = ui.global::<AppData>();
                    app_data.set_cedulas_view_is_processing(false);
                    app_data.set_progress_percentage(0.0);
                    show_toast_message(&ui, "No se encontraron archivos de cédula PDF válidos en la selección.");
                }
            });
            return;
        }

        let total_items = items.len();
        let _ = prog_tx.send(CedulaProgressUpdate {
            processed_count: 0,
            total_count: total_items as i32,
            progress_percentage: 2.0,
            current_file_name: "Sincronizando archivos existentes...".into(),
            status_text: format!("Preparando {} archivos encontrados...", total_items),
        });

        let cache_key_dir = if !revision_id.trim().is_empty() { &revision_id } else { &rp_id };
        let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(cache_key_dir);

        // 1. Obtener archivos existentes en RustFS estrictamente para esta revisión
        let existing_cards = crate::rustfs::sync_and_load_cedulas_view_files(
            &c_nom,
            &r_reg,
            &rev_nom,
            &cache_dir,
        ).unwrap_or_default();

        // 2. Fase 2 (Preparación / Staging en rustfs):
        //    Estandarizar nombre 'Nombre Cliente__Registro Patronal__Mes__Anio.pdf',
        //    preparar en un prefijo temporal `.staging_<batch_id>/...` en rustfs y caché local,
        //    sin modificar ni borrar datos definitivos hasta que PostgreSQL confirme el COMMIT.
        let batch_id = Uuid::now_v7().simple().to_string();
        let mut overwritten_count = 0;
        let mut cleaned_months_in_batch: HashSet<(String, String)> = HashSet::new();
        // Deduplicación por SHA-256: conexión de solo lectura para consultar la BD antes de subir
        let id_rp_dedupe: Option<Uuid> = Uuid::parse_str(&rp_id).ok();
        let mut conn_dedupe = pool_clone.as_ref().and_then(|p| p.get().ok());
        let mut hashes_lote: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut omitidos: Vec<String> = Vec::new();
        let mut pendientes: Vec<ArchivoPendienteCedula> = Vec::new();
        let mut staged_local_paths: Vec<PathBuf> = Vec::new();
        let mut overwrite_months: Vec<(String, String)> = Vec::new();
        let mut overwrite_old_files: Vec<String> = Vec::new();
        let mut staged_cards: Vec<CedulaFileCardInfo> = Vec::new();

        for (idx, item) in items.into_iter().enumerate() {
            let orig_display = Path::new(&item.local_path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| item.rel_path.clone());

            let pct = ((idx + 1) as f32 / total_items.max(1) as f32) * 45.0;
            let _ = prog_tx.send(CedulaProgressUpdate {
                processed_count: (idx + 1) as i32,
                total_count: total_items as i32,
                progress_percentage: pct.max(2.0),
                current_file_name: orig_display.clone(),
                status_text: format!("Preparando en staging {} de {} archivos...", idx + 1, total_items),
            });

            let data = match fs::read(&item.local_path) {
                Ok(d) => d,
                Err(_) => continue,
            };

            if data.len() < 50 {
                let _ = crate::rustfs::upload_file_to_rustfs(
                    &c_nom,
                    &r_reg,
                    &rev_nom,
                    "Cedulas_de_cuotas",
                    &format!("Trash/{}", orig_display),
                    &data,
                );
                continue;
            }

            // Extraer metadatos para nombre estricto y validar si contiene trabajadores SUA/EMA
            let (std_fname, anio_str, mes_str, _, _, is_valid_with_workers) =
                crate::cedulas::view_logic::inspect_cedula_upload_naming(
                    &item.local_path,
                    &c_nom,
                    &r_reg,
                );

            if !is_valid_with_workers {
                // Archivo sin trabajadores o sin formato SUA/EMA válido (ej. resumen o comprobante):
                // enviarlo a Trash sin sobreescribir ni borrar la cédula válida del mes
                let _ = crate::rustfs::upload_file_to_rustfs(
                    &c_nom,
                    &r_reg,
                    &rev_nom,
                    "Cedulas_de_cuotas",
                    &format!("Trash/{}", orig_display),
                    &data,
                );
                continue;
            }

            // Identificación por hash: omitir idénticos dentro del lote o ya cargados en la BD
            let sha = crate::cedulas::almacen::sha256_hex(&data);
            if let Some(primero) = hashes_lote.get(&sha) {
                omitidos.push(format!("{}: idéntico a '{}' en esta misma selección", orig_display, primero));
                continue;
            }
            if let (Some(conn), Some(id_rp)) = (conn_dedupe.as_mut(), id_rp_dedupe) {
                let lote = crate::db::ingesta::ArchivoLote {
                    nombre_original: orig_display.clone(),
                    sha256: sha.clone(),
                    tipo: "SUA".into(),
                    anio: anio_str.parse::<i16>().unwrap_or(0),
                    mes: crate::cedulas::business::month_to_num(&mes_str) as i16,
                    trabajadores: 0,
                    paginas: None,
                };
                match crate::db::ingesta::clasificar_lote(&mut **conn, id_rp, std::slice::from_ref(&lote)) {
                    Ok(estados) => {
                        if let Some(crate::db::ingesta::EstadoArchivoLote::IdenticoYaCargado { nombre_previo, .. }) = estados.first() {
                            omitidos.push(format!("{}: ya estaba cargado como '{}'", orig_display, nombre_previo));
                            continue;
                        }
                    }
                    Err(e) => log::warn!("No se pudo clasificar {} por hash en BD: {}", orig_display, e),
                }
            }
            hashes_lote.insert(sha.clone(), orig_display.clone());

            let month_key = (anio_str.clone(), mes_str.clone());
            if cleaned_months_in_batch.insert(month_key.clone()) {
                // Primera vez que se toca este mes en el lote actual: verificar si existían archivos previos en RustFS
                let existing_matches: Vec<_> = existing_cards
                    .iter()
                    .filter(|ex| {
                        let fn_up = ex.filename.to_uppercase();
                        fn_up.contains(&format!("__{}__{}", mes_str, anio_str))
                            || fn_up.contains(&format!("{}/{}", anio_str, mes_str))
                            || fn_up == std_fname.to_uppercase()
                    })
                    .collect();

                if !existing_matches.is_empty() {
                    overwritten_count += 1;
                    overwrite_months.push((anio_str.clone(), mes_str.clone()));
                    for old_card in existing_matches {
                        overwrite_old_files.push(old_card.filename.clone());
                    }
                }
            }

            let final_fname = crate::cedulas::almacen::nombre_estandar_con_hash(&std_fname, &sha);
            let structured_rel_path = format!("{}/{}/{}", anio_str, mes_str, final_fname);

            let _ = prog_tx.send(CedulaProgressUpdate {
                processed_count: (idx + 1) as i32,
                total_count: total_items as i32,
                progress_percentage: pct.max(2.0),
                current_file_name: final_fname.clone(),
                status_text: format!("Subiendo a staging {} de {} archivos...", idx + 1, total_items),
            });

            // Guardar primero en staging de RustFS (.staging_<batch_id>/Año/Mes/Archivo.pdf)
            let clave_staging = match crate::rustfs::upload_staging_file_to_rustfs(
                &c_nom,
                &r_reg,
                &rev_nom,
                "Cedulas_de_cuotas",
                &batch_id,
                &structured_rel_path,
                &data,
            ) {
                Ok(clave) => clave,
                Err(e) => {
                log::error!("Error subiendo archivo {} a staging de RustFS: {}", structured_rel_path, e);
                let _ = crate::rustfs::rollback_staging_files_in_rustfs(
                    &c_nom,
                    &r_reg,
                    &rev_nom,
                    "Cedulas_de_cuotas",
                    &batch_id,
                );
                for p in &staged_local_paths {
                    let _ = std::fs::remove_file(p);
                }
                drop(prog_tx);
                let err_detail = format!("Error almacenando archivo '{}' en rustfs (staging): {}", structured_rel_path, e);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        let app_data = ui.global::<AppData>();
                        app_data.set_cedulas_view_is_processing(false);
                        app_data.set_is_processing(false);
                        app_data.set_cedulas_view_progress_fraction(0.0);
                        app_data.set_cedulas_view_progress_text("".into());
                        app_data.set_progress_percentage(0.0);
                        app_data.set_system_error_modal_title("Error de Almacenamiento en rustfs".into());
                        app_data.set_system_error_modal_detail(err_detail.into());
                        app_data.set_show_system_error_modal(true);
                    }
                });
                return;
                }
            };

            let local_dest = cache_dir.join("cedulas_de_cuotas").join(&structured_rel_path);
            if let Some(parent) = local_dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&local_dest, &data).is_ok() {
                staged_local_paths.push(local_dest.clone());
                // Clave definitiva con el mismo layout que usa commit_staging_files_in_rustfs
                let clave_final = format!(
                    "{}/{}/{}/Cedulas_de_cuotas/{}",
                    crate::rustfs::sanitize_path_component(&c_nom),
                    crate::rustfs::sanitize_path_component(&r_reg),
                    crate::rustfs::sanitize_path_component(&rev_nom),
                    crate::rustfs::sanitize_relative_path(&structured_rel_path),
                );
                pendientes.push(ArchivoPendienteCedula {
                    nombre_archivo: final_fname.clone(),
                    nombre_original: orig_display.clone(),
                    sha256: sha.clone(),
                    anio: anio_str.parse::<i16>().unwrap_or(0),
                    mes: crate::cedulas::business::month_to_num(&mes_str) as i16,
                    clave_final,
                    clave_staging,
                });
                let exp_rfc_opt = if !c_rfc.trim().is_empty() { Some(c_rfc.trim()) } else { None };
                let exp_rp_opt = if !r_reg.trim().is_empty() && r_reg != "GENERAL" { Some(r_reg.trim()) } else { None };
                let mut card = crate::cedulas::view_logic::inspect_cedula_file_with_context(
                    &local_dest,
                    exp_rfc_opt,
                    exp_rp_opt,
                );
                card.filename = structured_rel_path;
                staged_cards.push(card);
            }
        }

        // Combinar tarjetas previas (excluyendo las que serán sobreescritas) con las tarjetas en staging
        let old_set: HashSet<String> = overwrite_old_files.iter().cloned().collect();
        let (old_cards, mut final_cards): (Vec<CedulaFileCardInfo>, Vec<CedulaFileCardInfo>) = existing_cards
            .into_iter()
            .partition(|c| old_set.contains(&c.filename));
        final_cards.extend(staged_cards);
        drop(conn_dedupe);

        drop(prog_tx);

        // Impacto del reemplazo: consulta por mes en este hilo de fondo (nunca en el hilo de UI)
        let overwrite_summary: String = if overwrite_months.is_empty() {
            String::new()
        } else {
            let mut lines: Vec<String> = Vec::new();
            let mut db_conn = pool_clone.as_ref().and_then(|p| p.get().ok());
            let rev_uuid_opt = Uuid::parse_str(&revision_id).ok();
            for (anio, mes) in &overwrite_months {
                let mes_up = mes.trim().to_uppercase();
                let etiqueta = format!("{} {}", mes_up, anio);
                let line = match (db_conn.as_mut(), rev_uuid_opt, anio.trim().parse::<i16>()) {
                    (Some(client), Some(rev_uuid), Ok(anio_num)) => {
                        let mes_num = crate::cedulas::business::month_to_num(&mes_up) as i16;
                        match client.query_one(
                            "SELECT COUNT(DISTINCT rl.id)::bigint,
                                    COUNT(rdl.id) FILTER (WHERE rdl.id_registro_obra IS NOT NULL)::bigint
                             FROM public.reportes_liquidaciones rl
                             LEFT JOIN public.reportes_detalles_liquidaciones rdl ON rdl.id_reporte_liquidacion = rl.id
                             WHERE rl.id_revision = $1 AND rl.anio_periodo = $2
                               AND (UPPER(TRIM(rl.mes_periodo)) = $3 OR rl.mes_periodo_int = $4)",
                            &[&rev_uuid, &anio_num, &mes_up, &mes_num],
                        ) {
                            Ok(row) => {
                                let n_filas: i64 = row.get(0);
                                let n_asig: i64 = row.get(1);
                                format!("{}: {} filas de reporte, {} asignaciones de obra", etiqueta, n_filas, n_asig)
                            }
                            Err(e) => format!("{}: no se pudo consultar ({})", etiqueta, e),
                        }
                    }
                    (None, _, _) | (_, None, _) => {
                        format!("{}: no se pudo consultar (sin conexión a BD o revisión inválida)", etiqueta)
                    }
                    _ => format!("{}: no se pudo consultar (año no válido: {})", etiqueta, anio),
                };
                lines.push(line);
            }
            lines.push(format!("Archivos nuevos que reemplazan: {}", staged_local_paths.len()));
            lines.join("\n")
        };
        let staging_ctx = (c_nom.clone(), r_reg.clone(), rev_nom.clone());
        let hay_nuevos = !pendientes.is_empty();
        let omitidos_msg = omitidos.join("\n");

        // 4. Actualizar la interfaz Slint; si hay meses repetidos, pedir confirmación antes de procesar
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                {
                    let mut sess = session_clone.lock().unwrap();
                    sess.current_revision_id = Some(revision_id.clone());
                    sess.files = final_cards;
                    sess.pending_staging_batch_id = Some(batch_id);
                    sess.pending_staged_local_paths = staged_local_paths;
                    sess.pending_overwrite_months = overwrite_months;
                    sess.pending_overwrite_old_files = overwrite_old_files;
                    sess.pending_overwrite_old_cards = old_cards;
                    sess.pending_staging_ctx = Some(staging_ctx);
                    sess.pending_archivos = pendientes;
                    update_cedulas_view_file_list_ui(&ui, &sess);
                }

                let app_data = ui.global::<AppData>();
                if !omitidos_msg.is_empty() {
                    app_data.set_cedulas_duplicate_files_msg(omitidos_msg.into());
                    app_data.set_show_cedulas_duplicate_modal(true);
                }

                if !hay_nuevos {
                    // Todos los archivos eran idénticos o inválidos: no hay nada que procesar
                    app_data.set_cedulas_view_is_processing(false);
                    app_data.set_is_processing(false);
                    app_data.set_cedulas_view_progress_fraction(0.0);
                    app_data.set_cedulas_view_progress_text("".into());
                    app_data.set_progress_percentage(0.0);
                } else if overwritten_count > 0 {
                    // Sin confirmación no se procesa nada: el worker se lanza desde el modal.
                    // Se apaga el indicador de "Procesando" (su capa z:9999 taparía el modal);
                    // spawn_cedulas_view_worker lo vuelve a encender al elegir Fusionar/Reemplazar.
                    let app_data = ui.global::<AppData>();
                    app_data.set_cedulas_view_is_processing(false);
                    app_data.set_is_processing(false);
                    app_data.set_cedulas_view_progress_fraction(0.0);
                    app_data.set_cedulas_view_progress_text("".into());
                    app_data.set_progress_percentage(0.0);
                    app_data.set_cedulas_overwrite_confirm_msg(overwrite_summary.into());
                    app_data.set_show_cedulas_overwrite_confirm_modal(true);
                } else {
                    // Iniciar procesamiento y guardado atómico bidireccional (PostgreSQL + rustfs)
                    spawn_cedulas_view_worker(ui_weak.clone(), session_clone.clone(), pool_clone.clone());
                }
            }
        });
    });
}

fn spawn_cedulas_view_worker(
    ui_weak: slint::Weak<MainWindow>,
    session_arc: Arc<Mutex<CedulasViewState>>,
    pool_opt: Option<Arc<DbPool>>,
) {
    let (
        input_paths,
        client_rfc,
        client_nombre,
        selected_rp_registro,
        effective_rp_id,
        revision_id,
        revision_nombre,
        staging_batch_id_opt,
        staged_local_paths,
        overwrite_months,
        overwrite_old_files,
        archivos_pendientes,
    ) = {
        let Some(ui) = ui_weak.upgrade() else { return; };
        let app_data = ui.global::<AppData>();
        let mut session = session_arc.lock().unwrap();
        let c_nom = app_data.get_selected_cliente_nombre().to_string();
        let rp = app_data.get_selected_rp_registro().to_string();
        let rp_id_sel = app_data.get_selected_rp_id().to_string();
        let rev_id_sel = app_data.get_selected_revision_id().to_string();
        let rev_nom = app_data.get_selected_revision_nombre().to_string();

        let effective_rp = if !rp_id_sel.trim().is_empty() {
            rp_id_sel
        } else {
            rev_id_sel.clone()
        };

        if session.files.is_empty() && !effective_rp.is_empty() && !c_nom.is_empty() {
            let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(&effective_rp);
            let rustfs_cards = crate::rustfs::sync_and_load_cedulas_view_files(
                &c_nom,
                &rp,
                &rev_nom,
                &cache_dir,
            ).unwrap_or_default();
            if !rustfs_cards.is_empty() {
                session.files = rustfs_cards;
            }
        }
        if session.files.is_empty() {
            app_data.set_cedulas_view_is_processing(false);
            app_data.set_progress_percentage(0.0);
            show_toast_message(&ui, "Por favor agrega al menos un archivo antes de procesar.");
            return;
        }
        app_data.set_cedulas_view_is_processing(true);
        if app_data.get_progress_percentage() < 45.0 {
            app_data.set_progress_percentage(45.0);
            app_data.set_cedulas_view_progress_fraction(0.45);
        }
        app_data.set_total_files_count(session.files.len() as i32);
        app_data.set_cedulas_view_progress_text("Iniciando motor de layout SUA y auditoría documental...".into());

        let paths: Vec<PathBuf> = session.files.iter().map(|f| f.path.clone()).collect();
        let rfc = app_data.get_selected_cliente_rfc().to_string();
        let nombre = app_data.get_selected_cliente_nombre().to_string();
        let batch_opt = session.pending_staging_batch_id.take();
        let st_locals = std::mem::take(&mut session.pending_staged_local_paths);
        let ov_months = std::mem::take(&mut session.pending_overwrite_months);
        let ov_files = std::mem::take(&mut session.pending_overwrite_old_files);
        // Reemplazo confirmado (o sin meses repetidos): las tarjetas ocultadas ya no se restauran
        session.pending_overwrite_old_cards.clear();
        // Los archivos nuevos del lote pasan al worker de guardado (solo ellos se registran en BD)
        let archivos_pendientes = std::mem::take(&mut session.pending_archivos);
        (
            paths,
            rfc,
            nombre,
            rp,
            effective_rp,
            rev_id_sel,
            rev_nom,
            batch_opt,
            st_locals,
            ov_months,
            ov_files,
            archivos_pendientes,
        )
    };

    let ui_w = ui_weak.clone();
    let session_w = session_arc.clone();
    let pool_w = pool_opt.or_else(crate::db::get_global_pool);

    let (prog_tx, mut prog_rx) = tokio::sync::mpsc::unbounded_channel::<CedulaProgressUpdate>();
    let ui_for_rx = ui_w.clone();

    get_tokio_handle().spawn(async move {
        while let Some(upd) = prog_rx.recv().await {
            let ui_clone = ui_for_rx.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_clone.upgrade() {
                    let app_data = ui.global::<AppData>();
                    app_data.set_cedulas_view_is_processing(true);
                    app_data.set_processed_count(upd.processed_count);
                    app_data.set_total_files_count(upd.total_count);
                    app_data.set_progress_percentage(upd.progress_percentage);
                    app_data.set_cedulas_view_progress_fraction(upd.progress_percentage / 100.0);
                    app_data.set_current_file_name(upd.current_file_name.into());
                    app_data.set_cedulas_view_progress_text(upd.status_text.into());
                }
            });
        }
    });

    get_tokio_handle().spawn_blocking(move || {

        let temp_dir_handle = tempfile::Builder::new()
            .prefix("docu_ffacd_cedview_")
            .tempdir();
        let temp_dir = match temp_dir_handle {
            Ok(ref d) => d.path().to_path_buf(),
            Err(_) => std::env::temp_dir(),
        };

        let temp_excel = temp_dir.join("cedulas_sua_audit_tmp.xlsx");

        // Obras registradas en la revisión activa
        let revision_obras_registered: HashSet<String> = if let Some(ref pool) = pool_w {
            if !revision_id.is_empty() {
                if let Ok(mut c) = pool.get() {
                    if let Ok(obras) = fetch_obras_by_revision(&mut c, &revision_id) {
                        obras.into_iter().map(|o| o.numero.trim().to_uppercase()).collect()
                    } else {
                        HashSet::new()
                    }
                } else {
                    HashSet::new()
                }
            } else {
                HashSet::new()
            }
        } else {
            HashSet::new()
        };

        let target_rfc_opt = if !client_rfc.trim().is_empty() {
            Some(client_rfc.as_str())
        } else {
            None
        };
        let target_nombre_opt = if !client_nombre.trim().is_empty() && client_nombre != "Todos los Clientes" {
            Some(client_nombre.as_str())
        } else {
            None
        };

        let mut rp_for_rustfs = String::new();
        if let Some(ref pool) = pool_w
            && let Ok(mut c) = pool.get()
        {
            if let Ok(rev_uuid) = uuid::Uuid::parse_str(&revision_id)
                && let Ok(Some(r)) = c.query_opt(
                    "SELECT rp.registro FROM revisiones r JOIN registros_patronales rp ON r.id_registro_patronal = rp.id WHERE r.id = $1",
                    &[&rev_uuid],
                )
            {
                rp_for_rustfs = r.get::<_, String>(0);
            } else if let Ok(rp_uuid) = uuid::Uuid::parse_str(&effective_rp_id)
                && let Ok(Some(r)) = c.query_opt(
                    "SELECT registro FROM registros_patronales WHERE id = $1",
                    &[&rp_uuid],
                )
            {
                rp_for_rustfs = r.get::<_, String>(0);
            }
        }
        if rp_for_rustfs.is_empty() {
            if !selected_rp_registro.is_empty() {
                rp_for_rustfs = selected_rp_registro;
            } else {
                rp_for_rustfs = "GENERAL".to_string();
            }
        }

        let cache_key = if !effective_rp_id.is_empty() { effective_rp_id.clone() } else { revision_id.clone() };
        let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(&cache_key);
        let files_to_process: Vec<PathBuf> = if !input_paths.is_empty() {
            input_paths.clone()
        } else {
            let rustfs_cards = crate::rustfs::sync_and_load_cedulas_view_files(
                &client_nombre,
                &rp_for_rustfs,
                &revision_nombre,
                &cache_dir,
            ).unwrap_or_default();
            rustfs_cards.iter().map(|c| c.path.clone()).collect()
        };

        let tx_for_audit = prog_tx.clone();
        let progress_cb = move |current: usize, total: usize, file_label: &str| {
            let pct = if total > 0 {
                45.0 + (current as f32 / total as f32) * 30.0
            } else {
                45.0
            };
            let _ = tx_for_audit.send(CedulaProgressUpdate {
                processed_count: current as i32,
                total_count: total as i32,
                progress_percentage: pct,
                current_file_name: file_label.to_string(),
                status_text: format!("Procesando {} de {} archivos...", current, total),
            });
        };

        let target_rp_opt = if !rp_for_rustfs.trim().is_empty() && rp_for_rustfs != "GENERAL" {
            Some(rp_for_rustfs.as_str())
        } else {
            None
        };

        match process_and_audit_cedulas(
            &files_to_process,
            target_rfc_opt,
            target_nombre_opt,
            target_rp_opt,
            &revision_obras_registered,
            &temp_excel,
            &temp_dir,
            progress_cb,
        ) {
            Ok((audit_res, businesses, mismatch_report)) => {
                // Mover a Trash en RustFS únicamente los archivos que fallaron por completo
                for bad_item in &mismatch_report.items {
                    if let Err(e) = crate::rustfs::move_file_to_trash(
                        &client_nombre,
                        &rp_for_rustfs,
                        &revision_nombre,
                        "Cedulas_de_cuotas",
                        &bad_item.filename,
                    ) {
                        log::debug!("No se pudo mover archivo {} a Trash en RustFS: {}", bad_item.filename, e);
                    } else {
                        log::info!("Archivo con error {} movido directamente a Trash en RustFS", bad_item.filename);
                    }
                    let local_cached = cache_dir.join(&bad_item.filename);
                    let _ = std::fs::remove_file(&local_cached);
                }

                let target_sync_id = if !revision_id.trim().is_empty() {
                    revision_id.clone()
                } else {
                    effective_rp_id.clone()
                };

                // Fase 3 y Fase 4: Guardado Atómico Bidireccional (PostgreSQL + rustfs) con Rollback Seguro
                let mut db_saved_summary = String::new();
                let mut saved_stats_opt: Option<CedulasDbStats> = None;
                let mut atomic_save_err: Option<String> = None;

                if !businesses.is_empty() {
                    if let Some(ref pool) = pool_w {
                        let tx_for_db = prog_tx.clone();
                        let db_progress_cb = move |current: usize, total: usize, file_label: &str| {
                            let pct = if total > 0 {
                                75.0 + (current as f32 / total as f32) * 23.0
                            } else {
                                85.0
                            };
                            let _ = tx_for_db.send(CedulaProgressUpdate {
                                processed_count: current as i32,
                                total_count: total as i32,
                                progress_percentage: pct,
                                current_file_name: file_label.to_string(),
                                status_text: format!("Guardando atómicamente en BD y rustfs: {} de {}...", current, total),
                            });
                        };

                        let c_nom_hook = client_nombre.clone();
                        let rp_hook = rp_for_rustfs.clone();
                        let rev_nom_hook = revision_nombre.clone();
                        let batch_hook_opt = staging_batch_id_opt.clone();
                        let ov_files_hook = overwrite_old_files.clone();
                        let cache_dir_hook = cache_dir.clone();


                        // Solo se registran en BD los trabajadores de archivos NUEVOS del lote;
                        // los PDFs ya cargados no se reprocesan (la regla "gana la más reciente" los pisaría)
                        let nombres_nuevos: HashSet<&str> = archivos_pendientes
                            .iter()
                            .map(|a| a.nombre_archivo.as_str())
                            .collect();
                        let businesses_nuevos: Vec<crate::cedulas::business::Business> = businesses
                            .iter()
                            .filter_map(|b| {
                                let mut nb = b.clone();
                                nb.employees.retain(|e| {
                                    e.file_name.as_deref().map_or(false, |f| nombres_nuevos.contains(f))
                                });
                                if nb.employees.is_empty() { None } else { Some(nb) }
                            })
                            .collect();

                        let mut archivos_registrar: Vec<ArchivoParaRegistrar> = Vec::new();
                        for pend in &archivos_pendientes {
                            let emps: Vec<&crate::cedulas::employees::Employee> = businesses_nuevos
                                .iter()
                                .flat_map(|b| b.employees.iter())
                                .filter(|e| e.file_name.as_deref() == Some(pend.nombre_archivo.as_str()))
                                .collect();
                            if emps.is_empty() {
                                log::info!(
                                    "Cédulas: '{}' sin trabajadores válidos; no se registra en BD",
                                    pend.nombre_original
                                );
                                continue;
                            }
                            archivos_registrar.push(ArchivoParaRegistrar {
                                nombre_archivo: pend.nombre_archivo.clone(),
                                lote: ArchivoLote {
                                    nombre_original: pend.nombre_original.clone(),
                                    sha256: pend.sha256.clone(),
                                    tipo: tipo_archivo_desde_empleados(&emps).to_string(),
                                    anio: pend.anio,
                                    mes: pend.mes,
                                    trabajadores: emps.len() as i32,
                                    paginas: None,
                                },
                                clave_final: pend.clave_final.clone(),
                                clave_staging: pend.clave_staging.clone(),
                            });
                        }

                        // Promoción en RustFS (se ejecuta DESPUÉS del COMMIT de PostgreSQL):
                        // 1) mueve a Trash los archivos sustituidos; 2) promueve SOLO los PDFs registrados en BD
                        //    (los rechazados por la auditoría no deben llegar a la ruta final);
                        // 3) descarta el resto del staging del lote.
                        let claves_a_promover: Vec<(String, String)> = archivos_registrar
                            .iter()
                            .map(|a| (a.clave_staging.clone(), a.clave_final.clone()))
                            .collect();
                        let rustfs_commit_hook = move || -> Result<(), String> {
                            if let Some(ref batch_id) = batch_hook_opt {
                                for old_fname in &ov_files_hook {
                                    let _ = crate::rustfs::move_file_to_trash(
                                        &c_nom_hook,
                                        &rp_hook,
                                        &rev_nom_hook,
                                        "Cedulas_de_cuotas",
                                        old_fname,
                                    );
                                    let old_local = cache_dir_hook.join("cedulas_de_cuotas").join(old_fname);
                                    let _ = std::fs::remove_file(&old_local);
                                }
                                let almacen = crate::cedulas::almacen::AlmacenRustfs;
                                let mut fallos: Vec<String> = Vec::new();
                                for (staging, final_key) in &claves_a_promover {
                                    if let Err(e) = crate::cedulas::almacen::AlmacenArchivos::promover(&almacen, staging, final_key) {
                                        fallos.push(e);
                                    }
                                }
                                if !fallos.is_empty() {
                                    // El staging se conserva para que reintentar_promociones_pendientes lo complete
                                    return Err(fallos.join("; "));
                                }
                                crate::rustfs::rollback_staging_files_in_rustfs(
                                    &c_nom_hook,
                                    &rp_hook,
                                    &rev_nom_hook,
                                    "Cedulas_de_cuotas",
                                    batch_id,
                                )?;
                            }
                            Ok(())
                        };

                        if archivos_registrar.is_empty() {
                            // Sin archivos nuevos válidos: se descarta el staging del lote para no dejar huérfanos
                            log::info!("Cédulas: ningún archivo nuevo válido que registrar; se descarta el staging del lote");
                            if let Some(ref batch_id) = staging_batch_id_opt {
                                let _ = crate::rustfs::rollback_staging_files_in_rustfs(
                                    &client_nombre,
                                    &rp_for_rustfs,
                                    &revision_nombre,
                                    "Cedulas_de_cuotas",
                                    batch_id,
                                );
                            }
                            for p in &staged_local_paths {
                                let _ = std::fs::remove_file(p);
                            }
                        } else {
                            match guardar_cedulas_lote(
                                pool,
                                &target_sync_id,
                                &businesses_nuevos,
                                &archivos_registrar,
                                &overwrite_months,
                                Some(&rustfs_commit_hook),
                                Some(&db_progress_cb),
                            ) {
                                Ok(stats) => {
                                    db_saved_summary = format!(
                                        "Guardadas {} liquidaciones y {} detalles de cédulas.",
                                        stats.liquidaciones_guardadas, stats.detalles_guardados
                                    );
                                    log::info!("Guardado Atómico DB + rustfs Cédulas exitoso: {:?}", stats);
                                    saved_stats_opt = Some(stats);
                                }
                                Err(e) => {
                                    log::error!("Error en guardado atómico de cédulas (ejecutando rollback): {}", e);
                                    if let Some(ref batch_id) = staging_batch_id_opt {
                                        let _ = crate::rustfs::rollback_staging_files_in_rustfs(
                                            &client_nombre,
                                            &rp_for_rustfs,
                                            &revision_nombre,
                                            "Cedulas_de_cuotas",
                                            batch_id,
                                        );
                                    }
                                    for p in &staged_local_paths {
                                        let _ = std::fs::remove_file(p);
                                    }
                                    atomic_save_err = Some(e);
                                }
                            }
                        }
                    } else {
                        let err_msg = "No hay conexión activa con el servidor PostgreSQL. Se canceló el guardado y se limpiaron los archivos temporales en rustfs.".to_string();
                        log::error!("{}", err_msg);
                        if let Some(ref batch_id) = staging_batch_id_opt {
                            let _ = crate::rustfs::rollback_staging_files_in_rustfs(
                                &client_nombre,
                                &rp_for_rustfs,
                                &revision_nombre,
                                "Cedulas_de_cuotas",
                                batch_id,
                            );
                        }
                        for p in &staged_local_paths {
                            let _ = std::fs::remove_file(p);
                        }
                        atomic_save_err = Some(err_msg);
                    }
                }
                drop(prog_tx);

                // Si ocurrió cualquier error en PostgreSQL o en rustfs, revertir el estado visual y mostrar el modal de error
                if let Some(err_detail) = atomic_save_err {
                    let existing_committed_cards = crate::rustfs::sync_and_load_cedulas_view_files(
                        &client_nombre,
                        &rp_for_rustfs,
                        &revision_nombre,
                        &cache_dir,
                    ).unwrap_or_default();
                    let committed_db_opt = pool_w
                        .as_ref()
                        .and_then(|p| p.get().ok())
                        .and_then(|mut c| load_cedulas_from_db(&mut c, &target_sync_id).ok().flatten());

                    let ui_err = ui_w.clone();
                    let session_err = session_w.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_err.upgrade() {
                            let mut sess = session_err.lock().unwrap();
                            sess.files = existing_committed_cards;
                            if let Some((db_audit, db_businesses, _)) = committed_db_opt {
                                sess.audit_res = Some(db_audit);
                                sess.businesses = db_businesses;
                                update_cedulas_view_file_list_ui(&ui, &sess);
                                update_cedulas_audit_view_ui(&ui, &mut sess);
                                ui.global::<AppData>().set_cedulas_view_state(2);
                            } else {
                                reset_cedulas_ui_to_state_zero(&ui, &mut sess);
                            }
                            drop(sess);

                            let app_data = ui.global::<AppData>();
                            app_data.set_cedulas_view_is_processing(false);
                            app_data.set_is_processing(false);
                            app_data.set_cedulas_view_progress_fraction(0.0);
                            app_data.set_cedulas_view_progress_text("".into());
                            app_data.set_progress_percentage(0.0);
                            app_data.set_current_file_name("".into());
                            app_data.set_system_error_modal_title("Error al Guardar la Información".into());
                            app_data.set_system_error_modal_detail(err_detail.into());
                            app_data.set_show_system_error_modal(true);
                        }
                    });
                    return;
                }

                // Recargar desde BD con el filtro estricto de las obras registradas y mes_periodo_int numérico
                let mut final_audit = audit_res;
                let mut final_businesses = businesses;
                let mut diag_from_db = crate::db::reporte_liquidaciones::MissingCedulasDiagnostic::default();
                let effective_sync_id = if let Some(ref stats) = saved_stats_opt {
                    stats.id_revision.map(|u| u.to_string())
                        .or_else(|| stats.id_registro_patronal.map(|u| u.to_string()))
                        .unwrap_or_else(|| target_sync_id.clone())
                } else {
                    target_sync_id.clone()
                };
                if let Some(mut client) = pool_w.as_ref().and_then(|p| p.get().ok()) {
                    if let Ok(Some((db_audit, db_businesses, _missing_warning))) = load_cedulas_from_db(&mut client, &effective_sync_id) {
                        final_audit = db_audit;
                        final_businesses = db_businesses;
                    }
                    let diag_res = Uuid::parse_str(&effective_sync_id)
                        .map_err(|e| e.to_string())
                        .and_then(|u| crate::db::reporte_liquidaciones::check_missing_cedulas_for_revision(&mut client, u));
                    if let Ok(diag) = diag_res {
                        diag_from_db = diag;
                    }
                }

                // Actualizar sesión y Slint UI
                let ui_final = ui_w.clone();
                let session_final = session_w.clone();
                let pool_stats = pool_w.clone();
                let saved_stats_for_ui = saved_stats_opt.clone();

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_final.upgrade() {
                        let is_all_failed = final_audit.validos_sua == 0 || final_businesses.is_empty();
                        if is_all_failed {
                            {
                                let mut sess = session_final.lock().unwrap();
                                reset_cedulas_ui_to_state_zero(&ui, &mut sess);
                            }
                            let app_data = ui.global::<AppData>();
                            app_data.set_cedulas_view_is_processing(false);
                            app_data.set_cedulas_view_progress_fraction(0.0);
                            app_data.set_cedulas_view_progress_text("".into());
                            app_data.set_progress_percentage(0.0);
                            app_data.set_current_file_name("".into());

                            let ui_items: Vec<crate::MismatchDetailItem> = mismatch_report.items.into_iter().map(|it| crate::MismatchDetailItem {
                                filename: it.filename.into(),
                                tipo_error: it.error_type.into(),
                                valor_esperado: it.expected_val.into(),
                                valor_encontrado: it.found_val.into(),
                                descripcion: it.details.into(),
                            }).collect();
                            let mismatch_model = std::rc::Rc::new(slint::VecModel::from(ui_items));
                            app_data.set_cedulas_mismatch_items(mismatch_model.into());
                            app_data.set_cedulas_mismatch_expected_rfc(mismatch_report.expected_rfc.into());
                            app_data.set_cedulas_mismatch_expected_rp(mismatch_report.expected_rp.into());
                            app_data.set_cedulas_mismatch_expected_cliente(mismatch_report.expected_cliente.into());
                            app_data.set_cedulas_mismatch_title("Inconsistencias detectadas en Cédulas".into());
                            app_data.set_cedulas_mismatch_mensaje(
                                "Los archivos de cédulas seleccionados no corresponden con el Registro Patronal o RFC de la empresa registrada en esta revisión (se enviaron a la papelera Trash).".into()
                            );
                            app_data.set_show_cedulas_client_mismatch_modal(true);
                            return;
                        }

                        {
                            let mut sess = session_final.lock().unwrap();
                            let bad_names: HashSet<String> = mismatch_report.items.iter().map(|it| it.filename.clone()).collect();
                            sess.files.retain(|f| !bad_names.contains(&f.filename));
                            sess.audit_res = Some(final_audit);
                            sess.businesses = final_businesses;
                            sess.current_rp_id = Some(cache_key.clone());
                            sess.current_revision_id = if let Some(ref stats) = saved_stats_for_ui {
                                stats.id_revision.map(|u| u.to_string()).or_else(|| if !revision_id.is_empty() { Some(revision_id.clone()) } else { None })
                            } else if !revision_id.is_empty() {
                                Some(revision_id.clone())
                            } else {
                                None
                            };
                            update_cedulas_audit_view_ui(&ui, &mut sess);
                        }
                        let app_data = ui.global::<AppData>();
                        app_data.set_rp_has_cedulas(true);
                        app_data.set_reporte_has_missing_cedulas_warning(diag_from_db.has_missing);
                        app_data.set_reporte_missing_cedulas_msg(diag_from_db.summary_msg.into());
                        app_data.set_reporte_missing_cedulas_reason(diag_from_db.reason_msg.into());
                        let missing_slint: Vec<slint::SharedString> = diag_from_db
                            .missing_periods
                            .iter()
                            .map(|s| slint::SharedString::from(s.as_str()))
                            .collect();
                        let inconsistencias_slint: Vec<InconsistenciaDetalle> = diag_from_db
                            .inconsistencias
                            .iter()
                            .map(|it| InconsistenciaDetalle {
                                periodo: it.periodo.clone().into(),
                                mes_anio: it.mes_anio.clone().into(),
                                obras_vinculadas: it.obras_vinculadas.clone().into(),
                                obras_count: it.obras_count as i32,
                            })
                            .collect();
                        app_data.set_reporte_missing_cedulas_meses(ModelRc::new(VecModel::from(missing_slint)));
                        app_data.set_reporte_inconsistencias_detalles(ModelRc::new(VecModel::from(inconsistencias_slint)));
                        if !diag_from_db.has_missing {
                            app_data.set_reporte_show_missing_cedulas_details(false);
                        }
                        app_data.set_cedulas_view_is_processing(false);
                        app_data.set_cedulas_view_state(2);
                        app_data.set_cedulas_view_progress_fraction(1.0);
                        app_data.set_cedulas_view_progress_text("¡Procesamiento y guardado completado!".into());
                        app_data.set_progress_percentage(100.0);
                        app_data.set_current_file_name("¡Procesamiento y guardado completado!".into());
                        app_data.set_reporte_liquidaciones_access_allowed(true);

                        // Actualizar propiedades de navegación tras guardar cédulas
                        if let Some(ref pool) = pool_stats {
                            if let Ok(mut client) = pool.get() {
                                let effective_rev_uuid = if let Ok(rev_uuid) = Uuid::parse_str(&revision_id) {
                                    Some(rev_uuid)
                                } else if let Some(ref stats) = saved_stats_for_ui {
                                    stats.id_revision
                                } else {
                                    None
                                };
                                if let Some(rev_uuid) = effective_rev_uuid {
                                    if let Ok((has_cedulas, has_nominas, has_liquidaciones, has_obras)) = 
                                        check_all_data_exists(&mut client, rev_uuid) {
                                        app_data.set_revision_has_cedulas(has_cedulas);
                                        app_data.set_revision_has_nominas(has_nominas);
                                        app_data.set_revision_has_liquidaciones(has_liquidaciones);
                                        app_data.set_revision_has_obras(has_obras);
                                        
                                        let all_data = has_obras && has_cedulas && has_nominas && has_liquidaciones;
                                        app_data.set_revision_has_all_data(all_data);

                                        if has_nominas {
                                            app_data.set_nominas_access_allowed(true);
                                        }
                                        if has_liquidaciones || has_cedulas {
                                            app_data.set_reporte_liquidaciones_access_allowed(true);
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(ref pool) = pool_stats {
                            update_global_stats(&ui, pool);
                            load_clientes_ui(&ui, pool);
                            let selected_cid = app_data.get_selected_cliente_id().to_string();
                            if !selected_cid.is_empty() {
                                load_registros_patronales_ui(&ui, pool, &selected_cid);
                                if let Ok(mut client) = pool.get() {
                                    if let Ok(uuid_val) = Uuid::parse_str(&selected_cid) {
                                        if let Ok(Some(row)) = client.query_opt(
                                            "SELECT nombre, rfc FROM public.clientes WHERE id = $1",
                                            &[&uuid_val],
                                        ) {
                                            let c_nom: String = row.get(0);
                                            let c_rfc: Option<String> = row.get(1);
                                            app_data.set_selected_cliente_nombre(c_nom.into());
                                            app_data.set_selected_cliente_rfc(c_rfc.unwrap_or_default().trim().into());
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(stats) = saved_stats_opt {
                            let rev_title = if !revision_nombre.trim().is_empty() {
                                revision_nombre.trim().to_string()
                            } else {
                                "Revisión activa".to_string()
                            };

                            let msg_liq = if stats.liquidaciones_guardadas > 0 {
                                format!(
                                    "Se registraron {} liquidaciones y {} líneas de detalle con cuotas patronales y obreras en la base de datos.",
                                    stats.liquidaciones_guardadas, stats.detalles_guardados
                                )
                            } else {
                                "La información de estas cédulas ya se encontraba previamente registrada en la base de datos PostgreSQL. No se duplicaron liquidaciones ni detalles.".to_string()
                            };
                            let msg_obras = if stats.registros_obras_guardados > 0 || stats.vinculaciones_guardadas > 0 {
                                format!(
                                    "Se leyeron de las cédulas {} registro(s) de obra nuevos (quedan como 'detectados en cédula', no visibles en Registros de Obras hasta que los registres) y se crearon {} vinculaciones de liquidaciones con obras.",
                                    stats.registros_obras_guardados, stats.vinculaciones_guardadas
                                )
                            } else {
                                "Las cédulas no traían registros de obra nuevos ni vinculaciones nuevas.".to_string()
                            };

                            // Omitidos/avisos de la ingesta: contador y lista truncada para el modal
                            const MAX_OMITIDOS_UI: usize = 200;
                            for aviso in &stats.errores {
                                log::warn!("Cédulas — {}", aviso);
                            }
                            let omitidos_count = stats.errores.len();
                            let omitidos_msg = if omitidos_count == 0 {
                                String::new()
                            } else {
                                let mut lineas: Vec<String> = stats
                                    .errores
                                    .iter()
                                    .take(MAX_OMITIDOS_UI)
                                    .cloned()
                                    .collect();
                                if omitidos_count > MAX_OMITIDOS_UI {
                                    lineas.push(format!(
                                        "... y {} más (ver log)",
                                        omitidos_count - MAX_OMITIDOS_UI
                                    ));
                                }
                                lineas.join("\n")
                            };
                            app_data.set_cedulas_success_omitidos_count(omitidos_count as i32);
                            app_data.set_cedulas_success_omitidos_msg(omitidos_msg.into());
                            let msg_rev = format!(
                                "Todas las liquidaciones y detalles fueron asociadas a la revisión activa: '{}'. No se crearon revisiones nuevas.",
                                rev_title
                            );
                            let msg_emp = format!(
                                "Se vincularon/actualizaron {} empleados con sus datos de afiliación y SDI.",
                                stats.empleados_guardados
                            );
                            let msg_pat = "Se conservaron los datos del cliente y se actualizaron los datos patronales complementarios (delegación, prima de riesgo).".to_string();

                            app_data.set_cedulas_success_liquidaciones_msg(msg_liq.into());
                            app_data.set_cedulas_success_obras_msg(msg_obras.into());
                            app_data.set_cedulas_success_revision_msg(msg_rev.into());
                            app_data.set_cedulas_success_empleados_msg(msg_emp.into());
                            app_data.set_cedulas_success_patronal_msg(msg_pat.into());
                            app_data.set_show_cedulas_success_modal(true);
                        }

                        let msg = if db_saved_summary.is_empty() {
                            "Cédulas analizadas con éxito (modo sin BD conectada)."
                        } else {
                            &db_saved_summary
                        };
                        show_toast_message(&ui, msg);
                    }
                });
            }
            Err((err_msg, mismatch_report)) => {
                drop(prog_tx);
                if let Some(ref batch_id) = staging_batch_id_opt {
                    let _ = crate::rustfs::rollback_staging_files_in_rustfs(
                        &client_nombre,
                        &rp_for_rustfs,
                        &revision_nombre,
                        "Cedulas_de_cuotas",
                        batch_id,
                    );
                }
                for p in &staged_local_paths {
                    let _ = std::fs::remove_file(p);
                }
                // Mover a Trash en RustFS todos los archivos que no se pudieron procesar
                for bad_item in &mismatch_report.items {
                    if let Err(e) = crate::rustfs::move_file_to_trash(
                        &client_nombre,
                        &rp_for_rustfs,
                        &revision_nombre,
                        "Cedulas_de_cuotas",
                        &bad_item.filename,
                    ) {
                        log::warn!("No se pudo mover archivo {} a Trash en RustFS: {}", bad_item.filename, e);
                    } else {
                        log::info!("Archivo con error {} movido directamente a Trash en RustFS", bad_item.filename);
                    }
                    let local_cached = cache_dir.join(&bad_item.filename);
                    let _ = std::fs::remove_file(&local_cached);
                }

                let ui_err = ui_w.clone();
                let session_err = session_w.clone();
                let is_client_mismatch = err_msg.contains("CLIENT_MISMATCH") || err_msg.contains("no pertenecen a la empresa") || !mismatch_report.is_empty();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_err.upgrade() {
                        let app_data = ui.global::<AppData>();
                        app_data.set_cedulas_view_is_processing(false);
                        app_data.set_is_processing(false);
                        app_data.set_cedulas_view_progress_fraction(0.0);
                        app_data.set_cedulas_view_progress_text("".into());
                        app_data.set_progress_percentage(0.0);
                        app_data.set_current_file_name("".into());

                        if is_client_mismatch {
                            {
                                let mut sess = session_err.lock().unwrap();
                                reset_cedulas_ui_to_state_zero(&ui, &mut sess);
                            }
                            let ui_items: Vec<crate::MismatchDetailItem> = mismatch_report.items.into_iter().map(|it| crate::MismatchDetailItem {
                                filename: it.filename.into(),
                                tipo_error: it.error_type.into(),
                                valor_esperado: it.expected_val.into(),
                                valor_encontrado: it.found_val.into(),
                                descripcion: it.details.into(),
                            }).collect();
                            let mismatch_model = std::rc::Rc::new(slint::VecModel::from(ui_items));
                            app_data.set_cedulas_mismatch_items(mismatch_model.into());
                            app_data.set_cedulas_mismatch_expected_rfc(mismatch_report.expected_rfc.into());
                            app_data.set_cedulas_mismatch_expected_rp(mismatch_report.expected_rp.into());
                            app_data.set_cedulas_mismatch_expected_cliente(mismatch_report.expected_cliente.into());
                            app_data.set_cedulas_mismatch_title("Inconsistencias detectadas en Cédulas".into());
                            app_data.set_cedulas_mismatch_mensaje(
                                "Los archivos de cédulas seleccionados no corresponden con el Registro Patronal o RFC de la empresa registrada (se enviaron a la papelera Trash).".into()
                            );
                            app_data.set_show_cedulas_client_mismatch_modal(true);
                        } else {
                            app_data.set_system_error_modal_title("Error al Procesar las Cédulas".into());
                            app_data.set_system_error_modal_detail(err_msg.into());
                            app_data.set_show_system_error_modal(true);
                        }
                    }
                });
            }
        }
    });
}

fn sync_and_load_cedulas_view_data(
    ui: &MainWindow,
    pool_opt: &Option<Arc<DbPool>>,
    session_arc: &Arc<Mutex<CedulasViewState>>,
    target_id: &str,
) {
    if target_id.is_empty() {
        return;
    }

    let pool_to_use = pool_opt.as_ref().cloned().or_else(crate::db::get_global_pool);
    let Some(pool) = pool_to_use.as_ref() else { return; };
    let mut client = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error obteniendo cliente de BD para cargar cédulas: {}", e);
            return;
        }
    };

    // 1. Obtener metadatos directamente del Registro Patronal (o fallback si pasaron revision)
    let parsed_uuid = uuid::Uuid::parse_str(target_id.trim()).ok();
    let mut cliente_nombre = String::new();
    let mut rp_registro = String::new();
    let mut revision_nombre = String::new();
    let mut cliente_rfc = String::new();
    let mut effective_rp_id = target_id.to_string();

    if let Some(target_uuid) = parsed_uuid {
        // Intentar primero como Registro Patronal
        let rp_meta = client.query_opt(
            r#"
            SELECT 
                c.nombre AS cliente_nombre,
                rp.registro AS rp_registro,
                c.rfc AS cliente_rfc,
                rp.id::text AS rp_id
            FROM public.registros_patronales rp
            JOIN public.clientes c ON rp.id_cliente = c.id
            WHERE rp.id = $1
            "#,
            &[&target_uuid],
        ).ok().flatten();

        if let Some(row) = rp_meta {
            cliente_nombre = row.get::<_, String>(0);
            rp_registro = row.get::<_, String>(1);
            cliente_rfc = row.get::<_, Option<String>>(2).unwrap_or_default();
            effective_rp_id = row.get::<_, String>(3);
            let ui_rev_nom = ui.global::<AppData>().get_selected_revision_nombre().to_string();
            revision_nombre = if !ui_rev_nom.trim().is_empty() {
                ui_rev_nom
            } else {
                "GENERAL".to_string()
            };
        } else {
            // Intentar como Revisión (para compatibilidad)
            let rev_meta = client.query_opt(
                r#"
                SELECT 
                    c.nombre AS cliente_nombre,
                    rp.registro AS rp_registro,
                    r.nombre AS revision_nombre,
                    c.rfc AS cliente_rfc,
                    rp.id::text AS rp_id
                FROM public.revisiones r
                JOIN public.registros_patronales rp ON r.id_registro_patronal = rp.id
                JOIN public.clientes c ON rp.id_cliente = c.id
                WHERE r.id = $1
                "#,
                &[&target_uuid],
            ).ok().flatten();

            if let Some(row) = rev_meta {
                cliente_nombre = row.get::<_, String>(0);
                rp_registro = row.get::<_, String>(1);
                revision_nombre = row.get::<_, String>(2);
                cliente_rfc = row.get::<_, Option<String>>(3).unwrap_or_default();
                effective_rp_id = row.get::<_, String>(4);
            }
        }
    }

    if cliente_nombre.is_empty() {
        let app_data = ui.global::<AppData>();
        cliente_nombre = app_data.get_selected_cliente_nombre().to_string();
        rp_registro = app_data.get_selected_rp_registro().to_string();
        revision_nombre = app_data.get_selected_revision_nombre().to_string();
        cliente_rfc = app_data.get_selected_cliente_rfc().to_string();
    }

    // 2. Sincronizar y descargar archivos de RustFS
    let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(&effective_rp_id);
    let mut rustfs_files = crate::rustfs::sync_and_load_cedulas_view_files(
        &cliente_nombre,
        &rp_registro,
        &revision_nombre,
        &cache_dir,
    ).unwrap_or_default();

    let exp_rfc_opt = if !cliente_rfc.trim().is_empty() { Some(cliente_rfc.trim()) } else { None };
    let exp_rp_opt = if !rp_registro.trim().is_empty() && rp_registro != "GENERAL" { Some(rp_registro.trim()) } else { None };

    for card in &mut rustfs_files {
        let p = std::path::PathBuf::from(&card.id);
        if p.exists() {
            let inspected = crate::cedulas::view_logic::inspect_cedula_file_with_context(
                &p,
                exp_rfc_opt,
                exp_rp_opt,
            );
            card.warning_msg = inspected.warning_msg;
        }
    }

    // 3. Cargar liquidaciones y calcular estadísticos desde PostgreSQL (ligadas al RP / Revisión)
    let ui_rev_id = ui.global::<AppData>().get_selected_revision_id().to_string();
    let effective_query_id = if !ui_rev_id.trim().is_empty() {
        ui_rev_id.clone()
    } else {
        target_id.to_string()
    };
    let db_res = load_cedulas_from_db(&mut client, &effective_query_id);
    let diag = if let Ok(rev_or_rp_uuid) = uuid::Uuid::parse_str(effective_query_id.trim()) {
        crate::db::reporte_liquidaciones::check_missing_cedulas_for_revision(&mut client, rev_or_rp_uuid).unwrap_or_default()
    } else if let Some(target_uuid) = parsed_uuid {
        crate::db::reporte_liquidaciones::check_missing_cedulas_for_revision(&mut client, target_uuid).unwrap_or_default()
    } else {
        crate::db::reporte_liquidaciones::MissingCedulasDiagnostic::default()
    };
    let missing_slint: Vec<slint::SharedString> = diag
        .missing_periods
        .iter()
        .map(|s| slint::SharedString::from(s.as_str()))
        .collect();

    // 4. Actualizar sesión y componentes Slint UI
    let mut sess = session_arc.lock().unwrap();
    sess.current_rp_id = Some(effective_rp_id.clone());
    sess.current_revision_id = Some(target_id.to_string());
    sess.files = rustfs_files;

    let app_data = ui.global::<AppData>();
    match db_res {
        Ok(Some((audit, businesses, _missing_cedulas))) => {
            sess.audit_res = Some(audit);
            sess.businesses = businesses;
            app_data.set_rp_has_cedulas(true);
            app_data.set_reporte_has_missing_cedulas_warning(diag.has_missing);
            app_data.set_reporte_missing_cedulas_msg(slint::SharedString::from(diag.summary_msg));
            app_data.set_reporte_missing_cedulas_reason(slint::SharedString::from(diag.reason_msg));
            let inconsistencias_slint: Vec<InconsistenciaDetalle> = diag
                .inconsistencias
                .iter()
                .map(|it| InconsistenciaDetalle {
                    periodo: it.periodo.clone().into(),
                    mes_anio: it.mes_anio.clone().into(),
                    obras_vinculadas: it.obras_vinculadas.clone().into(),
                    obras_count: it.obras_count as i32,
                })
                .collect();
            app_data.set_reporte_missing_cedulas_meses(ModelRc::new(VecModel::from(missing_slint)));
            app_data.set_reporte_inconsistencias_detalles(ModelRc::new(VecModel::from(inconsistencias_slint)));
            if !diag.has_missing {
                app_data.set_reporte_show_missing_cedulas_details(false);
            }
            update_cedulas_view_file_list_ui(ui, &sess);
            update_cedulas_audit_view_ui(ui, &mut sess);
            app_data.set_cedulas_view_state(2);
        }
        Ok(None) | Err(_) => {
            sess.audit_res = None;
            sess.businesses.clear();
            sess.filtered_details.clear();
            app_data.set_rp_has_cedulas(!sess.files.is_empty());
            app_data.set_reporte_has_missing_cedulas_warning(false);
            app_data.set_reporte_missing_cedulas_msg(slint::SharedString::from(""));
            app_data.set_reporte_missing_cedulas_reason(slint::SharedString::from(""));
            app_data.set_reporte_missing_cedulas_meses(ModelRc::new(VecModel::default()));
            app_data.set_reporte_inconsistencias_detalles(ModelRc::new(VecModel::default()));
            app_data.set_reporte_show_missing_cedulas_details(false);
            if !sess.files.is_empty() {
                // Si hay archivos en disco/RustFS pero la BD aún no tiene liquidaciones procesadas, auto-procesar
                drop(sess);
                spawn_cedulas_view_worker(ui.as_weak(), session_arc.clone(), pool_opt.clone());
            } else {
                reset_cedulas_ui_to_state_zero(ui, &mut sess);
            }
        }
    }
}

fn handle_confirmar_borrar_todas_cedulas(
    ui: &MainWindow,
    session_arc: &Arc<Mutex<CedulasViewState>>,
    pool_opt: &Option<Arc<DbPool>>,
) {
    let app_data = ui.global::<AppData>();
    app_data.set_show_delete_all_cedulas_modal(false);
    let rp_id = app_data.get_selected_rp_id().to_string();
    let revision_id = app_data.get_selected_revision_id().to_string();
    let target_id = if !revision_id.is_empty() { revision_id } else { rp_id };
    let cliente_nombre = app_data.get_selected_cliente_nombre().to_string();
    let rp_registro = app_data.get_selected_rp_registro().to_string();
    let revision_nombre = app_data.get_selected_revision_nombre().to_string();
    show_toast_message(ui, "Eliminando liquidaciones y moviendo archivos a Trash...");

    let ui_weak = ui.as_weak();
    let session_clone = session_arc.clone();
    let pool_clone = pool_opt.clone();

    thread::spawn(move || {
        let mut deleted_count = 0;
        let mut error_msg: Option<String> = None;

        let mut c_nom = cliente_nombre;
        let mut r_reg = rp_registro;
        let mut rev_nom = revision_nombre;

        let pool_to_use = pool_clone.or_else(crate::db::get_global_pool);
        if let Some(ref pool) = pool_to_use {
            if let Ok(mut client) = pool.get() {
                if let Ok(parsed_uuid) = uuid::Uuid::parse_str(&target_id) {
                    if let Ok(Some(row)) = client.query_opt(
                        "SELECT c.nombre, rp.registro, 'GENERAL'
                         FROM registros_patronales rp
                         JOIN clientes c ON rp.id_cliente = c.id
                         WHERE rp.id = $1",
                        &[&parsed_uuid],
                    ) {
                        c_nom = row.get(0);
                        r_reg = row.get(1);
                        rev_nom = row.get(2);
                    } else if let Ok(Some(row)) = client.query_opt(
                        "SELECT c.nombre, rp.registro, r.nombre
                         FROM revisiones r
                         JOIN registros_patronales rp ON r.id_registro_patronal = rp.id
                         JOIN clientes c ON rp.id_cliente = c.id
                         WHERE r.id = $1",
                        &[&parsed_uuid],
                    ) {
                        c_nom = row.get(0);
                        r_reg = row.get(1);
                        rev_nom = row.get(2);
                    }
                }

                match delete_all_cedulas_for_revision(&mut client, &target_id) {
                    Ok(n) => {
                        deleted_count = n;
                        log::info!("Eliminadas {} liquidaciones de la BD", n);
                    }
                    Err(e) => {
                        log::error!("Error al eliminar liquidaciones: {}", e);
                        error_msg = Some(format!("Error al eliminar en base de datos: {}", e));
                    }
                }
            }
        }

        // Mover archivos cargados a la carpeta Trash en RustFS
        if !c_nom.is_empty() && !r_reg.is_empty() && !rev_nom.is_empty() {
            if let Err(e) = crate::rustfs::move_all_files_to_trash(
                &c_nom,
                &r_reg,
                &rev_nom,
                "Cedulas_de_cuotas",
            ) {
                log::warn!("No se pudieron mover archivos a Trash en RustFS: {}", e);
            }
        }

        // Limpiar caché local
        let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(&target_id);
        let _ = std::fs::remove_dir_all(&cache_dir);

        let pool_for_ui = pool_to_use.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(err) = error_msg {
                    show_toast_message(&ui, &err);
                    return;
                }

                // Reset session and UI state to State 0
                {
                    let mut session = session_clone.lock().unwrap();
                    reset_cedulas_ui_to_state_zero(&ui, &mut session);
                }

                if let Some(ref pool) = pool_for_ui {
                    update_global_stats(&ui, pool);
                }

                let msg = if deleted_count > 0 {
                    format!("Se eliminaron {} liquidaciones registradas y se restableció la vista.", deleted_count)
                } else {
                    "Todas las cédulas en memoria y base de datos han sido eliminadas.".to_string()
                };
                show_toast_message(&ui, &msg);
            }
        });
    });
}

fn handle_descargar_cedulas_excel(
    ui: &MainWindow,
    session_arc: &Arc<Mutex<CedulasViewState>>,
) {
    let (audit_opt, businesses) = {
        let session = session_arc.lock().unwrap();
        (session.audit_res.clone(), session.businesses.clone())
    };

    let Some(audit) = audit_opt else {
        show_toast_message(ui, "No hay datos de auditoría de cédulas para exportar.");
        return;
    };

    if businesses.is_empty() {
        show_toast_message(ui, "No se encontraron cédulas válidas para exportar.");
        return;
    }

    let save_path = rfd::FileDialog::new()
        .set_title("Guardar Auditoría Integral de Cédulas SUA")
        .set_file_name("Auditoria_Cedulas_Determinacion_Cuotas_SUA.xlsx")
        .add_filter("Libro de Excel (*.xlsx)", &["xlsx"])
        .save_file();

    if let Some(path) = save_path {
        match export_cedulas_comprehensive_excel(&path, &audit, &businesses) {
            Ok(_) => {
                show_toast_message(ui, &format!("Reporte Excel guardado en: {}", path.display()));
            }
            Err(e) => {
                show_toast_message(ui, &format!("Error generando Excel: {}", e));
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn ensure_linux_desktop_integration() {
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        let apps_dir = home.join(".local/share/applications");
        let icons_dir = home.join(".local/share/icons/hicolor/256x256/apps");
        let _ = fs::create_dir_all(&apps_dir);
        let _ = fs::create_dir_all(&icons_dir);

        let icon_path = icons_dir.join("docu_ffacd.png");
        let icon_bytes = include_bytes!("../ui/assets/app_icon_256.png");
        let _ = fs::write(&icon_path, icon_bytes);

        let icons_base = home.join(".local/share/icons");
        let _ = fs::write(icons_base.join("docu_ffacd.png"), icon_bytes);

        let current_exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "docu_ffacd".to_string());

        let desktop_path = apps_dir.join("docu_ffacd.desktop");
        let desktop_content = format!(
            "[Desktop Entry]\nVersion=1.0\nType=Application\nName=Firma Fiscal ACD\nGenericName=Procesamiento de Nóminas SAT y Cédulas SUA\nComment=Validación SAT SOAP y Procesamiento SUA IMSS\nExec={}\nIcon=docu_ffacd\nTerminal=false\nCategories=Office;Finance;Utility;\nStartupWMClass=docu_ffacd\nX-KDE-SubstituteUID=false\n",
            current_exe
        );
        let _ = fs::write(&desktop_path, desktop_content);
    }
}

fn main() -> Result<(), slint::PlatformError> {
    crate::app_logger::init_app_logger();
    let _ = slint::set_xdg_app_id("docu_ffacd");
    let main_window = MainWindow::new()?;
    let _ = slint::set_xdg_app_id("docu_ffacd");

    #[cfg(target_os = "linux")]
    ensure_linux_desktop_integration();

    let nominas_list = Rc::new(RefCell::new(Vec::<SelectedFileEntry>::new()));
    let cedulas_list = Rc::new(RefCell::new(Vec::<SelectedFileEntry>::new()));
    let cedulas_session = Arc::new(Mutex::new(CedulasViewState::default()));
    let reporte_session = Arc::new(Mutex::new(ReporteSessionState::default()));
    let nominas_session = Arc::new(Mutex::new(NominasSessionState::default()));
    let nominas_upload_session = Arc::new(Mutex::new(NominasUploadSessionState::default()));

    // Store generated Excel file paths for direct download/saving
    let last_nominas_excel = Arc::new(Mutex::new(Option::<PathBuf>::None));
    let last_cedulas_excel = Arc::new(Mutex::new(Option::<PathBuf>::None));
    let last_nominas_download_dir = Arc::new(Mutex::new(Option::<PathBuf>::None));
    let last_cedulas_download_dir = Arc::new(Mutex::new(Option::<PathBuf>::None));
    let app_logic = main_window.global::<AppLogic>();

    // --- DateHelper para Mini Calendarios ---
    {
        let date_helper = main_window.global::<DateHelper>();
        date_helper.on_parse_date(|s| {
            if let Some(d) = parse_date_opt(s.as_str()) {
                use chrono::Datelike;
                DateDmy {
                    day: d.day() as i32,
                    month: d.month() as i32,
                    year: d.year() as i32,
                    valid: true,
                }
            } else {
                use chrono::Datelike;
                let today = chrono::Local::now().date_naive();
                DateDmy {
                    day: today.day() as i32,
                    month: today.month() as i32,
                    year: today.year() as i32,
                    valid: false,
                }
            }
        });

        date_helper.on_get_today(|| {
            use chrono::Datelike;
            let today = chrono::Local::now().date_naive();
            DateDmy {
                day: today.day() as i32,
                month: today.month() as i32,
                year: today.year() as i32,
                valid: true,
            }
        });

        date_helper.on_format_date_mask(|s| {
            let s_str = s.trim();
            if s_str.is_empty() {
                return slint::SharedString::default();
            }
            let digits: String = s_str.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                return slint::SharedString::default();
            }
            let res = match digits.len() {
                0 => String::new(),
                1 => {
                    if s_str.ends_with('/') {
                        format!("0{}/", digits)
                    } else {
                        digits
                    }
                }
                2 => {
                    if s_str.ends_with('/') || s_str.len() == 2 {
                        format!("{}/", digits)
                    } else {
                        digits
                    }
                }
                3 => {
                    if s_str.ends_with('/') && s_str.len() == 4 {
                        format!("{}/0{}/", &digits[0..2], &digits[2..3])
                    } else {
                        format!("{}/{}", &digits[0..2], &digits[2..3])
                    }
                }
                4 => {
                    if s_str.ends_with('/') || s_str.len() <= 5 {
                        format!("{}/{}/", &digits[0..2], &digits[2..4])
                    } else {
                        format!("{}/{}", &digits[0..2], &digits[2..4])
                    }
                }
                5..=8 => {
                    format!("{}/{}/{}", &digits[0..2], &digits[2..4], &digits[4..])
                }
                _ => {
                    format!("{}/{}/{}", &digits[0..2], &digits[2..4], &digits[4..8])
                }
            };
            res.into()
        });
    }

    let (db_pool, imss_inhabiles) = match create_pool(8) {
        Ok(pool) => {
            let arc_pool = Arc::new(pool);
            crate::db::set_global_pool(arc_pool.clone());
            let mut inhabiles_set = HashSet::new();
            if let Ok(mut client) = arc_pool.get() {
                if let Err(e) = ensure_schema(&mut client) {
                    let detailed = crate::db::format_pg_error(&e);
                    eprintln!("Error inicializando esquema de BD: {}", detailed);
                    log::error!("Error inicializando esquema de BD: {}", detailed);
                    crate::app_logger::log_db_simple_error(
                        "esquema",
                        "ensure_schema",
                        &format!("Error inicializando esquema: {}", detailed),
                    );
                }
                if let Ok(set) = fetch_imss_inhabiles(&mut client) {
                    inhabiles_set = set;
                }
            }
            (Some(arc_pool), Arc::new(RwLock::new(inhabiles_set)))
        }
        Err(e) => {
            eprintln!("Advertencia: No se pudo conectar a PostgreSQL: {}", e);
            crate::app_logger::log_db_simple_error(
                "sistema",
                "create_pool",
                &format!("No se pudo inicializar el pool de conexiones: {}", e),
            );
            (None, Arc::new(RwLock::new(HashSet::new())))
        }
    };

    // Reintento de promociones de cédulas pendientes (solo filas STAGED) en segundo plano
    if let Some(pool_reintento) = db_pool.clone() {
        let spawn_reintento = std::thread::Builder::new()
            .name("reintento_promociones_cedulas".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(8));
                match pool_reintento.get() {
                    Ok(mut client) => match crate::db::ingesta::reintentar_promociones_pendientes(
                        &mut client,
                        &crate::cedulas::almacen::AlmacenRustfs,
                        5,
                    ) {
                        Ok((promovidos, fallidos)) => {
                            log::info!("Reintento de promociones de cédulas: promovidos={}, fallidos={}", promovidos, fallidos);
                        }
                        Err(e) => log::warn!("Reintento de promociones de cédulas falló: {}", e),
                    },
                    Err(e) => log::warn!("Reintento de promociones de cédulas: sin conexión al pool: {}", e),
                }
            });
        if let Err(e) = spawn_reintento {
            log::warn!("No se pudo lanzar el hilo de reintento de promociones de cédulas: {}", e);
        }
    }

    // Diagnóstico y Estado Inicial de Base de Datos PostgreSQL
    let diag = crate::db::diagnose_postgres();
    let app_data = main_window.global::<AppData>();
    app_data.set_db_connected(diag.is_connected);
    app_data.set_db_status_text(diag.status_text.as_str().into());
    app_data.set_db_status_detail(diag.detail_text.as_str().into());

    if diag.is_connected {
        crate::app_logger::log_system_info(
            "Base de Datos",
            &format!("PostgreSQL conectado y verificado: {}", diag.detail_text),
        );
    } else {
        crate::app_logger::log_db_simple_error(
            "sistema",
            "diagnostico_inicial",
            &format!("Estado: {} | Detalle: {}", diag.status_text, diag.detail_text),
        );
    }

    // Callbacks de Estado de Base de Datos y Navegación
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_show_navigation_warning(move |msg| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Navegación",
                    "Sidebar",
                    "Aviso de Navegación",
                    msg.as_str(),
                );
                show_toast_message(&ui, msg.as_str());
            }
        });
    }

    // Toggle y visibilidad del Sidebar Principal
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_toggle_sidebar(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let current = app_data.get_is_sidebar_open();
                app_data.set_is_sidebar_open(!current);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_set_sidebar_open(move |open| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_is_sidebar_open(open);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_db_status_clicked(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let diag = crate::db::diagnose_postgres();
                let app_data = ui.global::<AppData>();
                app_data.set_db_connected(diag.is_connected);
                app_data.set_db_status_text(diag.status_text.as_str().into());
                app_data.set_db_status_detail(diag.detail_text.as_str().into());
                show_toast_message(&ui, &format!("Estado de BD: {}", diag.detail_text));
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_reconnect_db(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let pool_res = crate::db::reload_global_pool(8);
                let diag = crate::db::diagnose_postgres();
                let app_data = ui.global::<AppData>();
                app_data.set_db_connected(diag.is_connected);
                app_data.set_db_status_text(diag.status_text.as_str().into());
                app_data.set_db_status_detail(diag.detail_text.as_str().into());

                if diag.is_connected {
                    if let Ok(ref new_pool) = pool_res {
                        if let Ok(mut client) = new_pool.get() {
                            let _ = ensure_schema(&mut client);
                        }
                        load_clientes_ui(&ui, new_pool);
                        update_global_stats(&ui, new_pool);
                    } else if let Some(ref pool) = pool_clone {
                        load_clientes_ui(&ui, pool);
                        update_global_stats(&ui, pool);
                    }
                    crate::app_logger::log_system_info(
                        "Reconexión BD",
                        "Conexión con PostgreSQL verificada en línea.",
                    );
                    show_toast_message(&ui, "Base de datos conectada exitosamente.");
                } else {
                    crate::app_logger::log_db_simple_error(
                        "sistema",
                        "reconnect_db",
                        &format!("{}: {}", diag.status_text, diag.detail_text),
                    );
                    show_toast_message(&ui, &format!("Reconexión fallida: {}", diag.detail_text));
                }
            }
        });
    }


    if let Some(ref pool) = db_pool {
        update_global_stats(&main_window, pool);
        load_clientes_ui(&main_window, pool);
        let pool_rustfs = pool.clone();
        thread::spawn(move || {
            crate::rustfs::ensure_all_existing_structures_in_rustfs(&pool_rustfs);
        });
    }

    // 1. Select Nominas Files
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        app_logic.on_select_nominas_files(move || {
            crate::app_logger::log_ui_event(
                "Carga Nóminas",
                "Sección Archivos",
                "Abrir Diálogo Seleccionar Nóminas",
                "Mostrando selector de archivos (XML, ZIP, RAR)",
            );
            let files = rfd::FileDialog::new()
                .set_title("Seleccionar Nóminas (XML, ZIP o RAR)")
                .add_filter("Nóminas y Archivos Comprimidos", &["xml", "zip", "rar"])
                .add_filter("Archivos XML", &["xml"])
                .add_filter("Comprimidos ZIP / RAR", &["zip", "rar"])
                .pick_files();

            if let Some(paths) = files {
                crate::app_logger::log_ui_event(
                    "Carga Nóminas",
                    "Selector de Archivos",
                    "Archivos Seleccionados",
                    &format!("{} archivos elegidos", paths.len()),
                );
                let mut list = nominas_list_clone.borrow_mut();
                for p in paths {
                    if !list.iter().any(|existing| existing.path == p) {
                        list.push(create_file_entry(p));
                    }
                }
                if let Some(ui) = ui_weak.upgrade() {
                    update_nominas_ui(&ui, &list);
                }
            }
        });
    }

    // 1b. Select Nominas Folder (Recursive Search)
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        app_logic.on_select_nominas_folder(move || {
            crate::app_logger::log_ui_event(
                "Carga Nóminas",
                "Sección Carpetas",
                "Abrir Diálogo Seleccionar Carpeta",
                "Mostrando selector de carpetas para búsqueda recursiva",
            );
            let folder = rfd::FileDialog::new()
                .set_title("Seleccionar Carpeta con Nóminas (Búsqueda Recursiva XML, ZIP, RAR)")
                .pick_folder();

            if let Some(folder_path) = folder {
                let mut found_paths = Vec::new();
                for entry in walkdir::WalkDir::new(&folder_path).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                            let ext_lower = ext.to_lowercase();
                            if ext_lower == "xml" || ext_lower == "zip" || ext_lower == "rar" {
                                found_paths.push(entry.path().to_path_buf());
                            }
                        }
                    }
                }

                let mut list = nominas_list_clone.borrow_mut();
                let initial_len = list.len();
                for p in found_paths {
                    if !list.iter().any(|existing| existing.path == p) {
                        list.push(create_file_entry(p));
                    }
                }
                let added = list.len() - initial_len;
                crate::app_logger::log_ui_event(
                    "Carga Nóminas",
                    "Selector de Carpetas",
                    "Resultado Búsqueda Recursiva",
                    &format!("Carpeta='{}', archivos agregados={}", folder_path.display(), added),
                );
                if let Some(ui) = ui_weak.upgrade() {
                    update_nominas_ui(&ui, &list);
                    if added > 0 {
                        show_toast_message(&ui, &format!("Se agregaron {} archivos de nómina encontrados recursivamente.", added));
                    } else {
                        show_toast_message(&ui, "No se encontraron archivos de nómina (XML, ZIP o RAR) en la carpeta.");
                    }
                }
            }
        });
    }

    // 2. Select Cédulas Files
    {
        let ui_weak = main_window.as_weak();
        let cedulas_list_clone = cedulas_list.clone();
        app_logic.on_select_cedulas_files(move || {
            crate::app_logger::log_ui_event(
                "Carga Cédulas",
                "Sección Archivos",
                "Abrir Diálogo Seleccionar Cédulas",
                "Mostrando selector de archivos (PDF, ZIP, RAR)",
            );
            let files = rfd::FileDialog::new()
                .set_title("Seleccionar Cédulas SUA (PDF, ZIP o RAR)")
                .add_filter("Cédulas SUA", &["pdf", "zip", "rar"])
                .add_filter("Archivos PDF", &["pdf"])
                .add_filter("Comprimidos ZIP / RAR", &["zip", "rar"])
                .pick_files();

            if let Some(paths) = files {
                crate::app_logger::log_ui_event(
                    "Carga Cédulas",
                    "Selector de Archivos",
                    "Archivos Cédula Seleccionados",
                    &format!("{} archivos elegidos", paths.len()),
                );
                let mut list = cedulas_list_clone.borrow_mut();
                for p in paths {
                    if !list.iter().any(|existing| existing.path == p) {
                        list.push(create_file_entry(p));
                    }
                }
                if let Some(ui) = ui_weak.upgrade() {
                    update_cedulas_ui(&ui, &list);
                }
            }
        });
    }

    // 2b. Select Cédulas Folder (Recursive Search)
    {
        let ui_weak = main_window.as_weak();
        let cedulas_list_clone = cedulas_list.clone();
        app_logic.on_select_cedulas_folder(move || {
            crate::app_logger::log_ui_event(
                "Carga Cédulas",
                "Sección Carpetas",
                "Abrir Diálogo Seleccionar Carpeta Cédulas",
                "Mostrando selector de carpetas para búsqueda recursiva",
            );
            let folder = rfd::FileDialog::new()
                .set_title("Seleccionar Carpeta con Cédulas SUA (Búsqueda Recursiva PDF, ZIP, RAR)")
                .pick_folder();

            if let Some(folder_path) = folder {
                let mut found_paths = Vec::new();
                for entry in walkdir::WalkDir::new(&folder_path).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                            let ext_lower = ext.to_lowercase();
                            if ext_lower == "pdf" || ext_lower == "zip" || ext_lower == "rar" {
                                found_paths.push(entry.path().to_path_buf());
                            }
                        }
                    }
                }

                let mut list = cedulas_list_clone.borrow_mut();
                let initial_len = list.len();
                for p in found_paths {
                    if !list.iter().any(|existing| existing.path == p) {
                        list.push(create_file_entry(p));
                    }
                }
                let added = list.len() - initial_len;
                crate::app_logger::log_ui_event(
                    "Carga Cédulas",
                    "Selector de Carpetas",
                    "Resultado Búsqueda Recursiva Cédulas",
                    &format!("Carpeta='{}', archivos agregados={}", folder_path.display(), added),
                );
                if let Some(ui) = ui_weak.upgrade() {
                    update_cedulas_ui(&ui, &list);
                    if added > 0 {
                        show_toast_message(&ui, &format!("Se agregaron {} cédulas/archivos encontrados recursivamente.", added));
                    } else {
                        show_toast_message(&ui, "No se encontraron cédulas SUA (PDF, ZIP o RAR) en la carpeta.");
                    }
                }
            }
        });
    }

    // 3. Remove single file from Nóminas
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        app_logic.on_remove_nomina_file(move |file_id| {
            crate::app_logger::log_ui_event(
                "Carga Nóminas",
                "Lista de Archivos",
                "Eliminar Archivo Nómina",
                &format!("file_id={}", file_id),
            );
            let mut list = nominas_list_clone.borrow_mut();
            list.retain(|item| item.id != file_id);
            if let Some(ui) = ui_weak.upgrade() {
                update_nominas_ui(&ui, &list);
            }
        });
    }

    // 4. Remove single file from Cédulas
    {
        let ui_weak = main_window.as_weak();
        let cedulas_list_clone = cedulas_list.clone();
        app_logic.on_remove_cedula_file(move |file_id| {
            crate::app_logger::log_ui_event(
                "Carga Cédulas",
                "Lista de Archivos",
                "Eliminar Archivo Cédula",
                &format!("file_id={}", file_id),
            );
            let mut list = cedulas_list_clone.borrow_mut();
            list.retain(|item| item.id != file_id);
            if let Some(ui) = ui_weak.upgrade() {
                update_cedulas_ui(&ui, &list);
            }
        });
    }

    // 5. Clear Nóminas
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        let last_nom_clone = last_nominas_excel.clone();
        let last_nom_dir_clone = last_nominas_download_dir.clone();
        app_logic.on_clear_nominas(move || {
            crate::app_logger::log_ui_event(
                "Carga Nóminas",
                "Barra Acciones",
                "Click Botón Limpiar Nóminas",
                "Eliminando todas las nóminas de la lista de procesamiento",
            );
            let mut list = nominas_list_clone.borrow_mut();
            list.clear();
            if let Ok(mut g) = last_nom_clone.lock() { *g = None; }
            if let Ok(mut g) = last_nom_dir_clone.lock() { *g = None; }

            if let Some(ui) = ui_weak.upgrade() {
                update_nominas_ui(&ui, &list);
                let app_data = ui.global::<AppData>();
                app_data.set_nominas_ready_to_download(false);
                app_data.set_nominas_has_downloaded(false);
                app_data.set_nominas_is_processing(false);
                app_data.set_nominas_progress_fraction(0.0);
                app_data.set_nominas_progress_text("".into());

                let mut sum = app_data.get_summary();
                sum.nominas_total = 0;
                sum.nominas_valid = 0;
                sum.nominas_invalid = 0;
                sum.has_nominas_excel = false;
                sum.nominas_excel_path = "".into();
                app_data.set_summary(sum);

                let remaining_unproc: Vec<UnprocessedItemData> = app_data
                    .get_unprocessed_items()
                    .iter()
                    .filter(|item| item.category != "NÓMINA")
                    .collect();
                app_data.set_unprocessed_items(ModelRc::new(VecModel::from(remaining_unproc)));

                show_toast_message(&ui, "Se eliminaron todas las nóminas seleccionadas.");
            }
        });
    }

    // 6. Clear Cédulas
    {
        let ui_weak = main_window.as_weak();
        let cedulas_list_clone = cedulas_list.clone();
        let last_ced_clone = last_cedulas_excel.clone();
        let last_ced_dir_clone = last_cedulas_download_dir.clone();
        app_logic.on_clear_cedulas(move || {
            crate::app_logger::log_ui_event(
                "Carga Cédulas",
                "Barra Acciones",
                "Click Botón Limpiar Cédulas",
                "Eliminando todas las cédulas de la lista de procesamiento",
            );
            let mut list = cedulas_list_clone.borrow_mut();
            list.clear();
            if let Ok(mut g) = last_ced_clone.lock() { *g = None; }
            if let Ok(mut g) = last_ced_dir_clone.lock() { *g = None; }

            if let Some(ui) = ui_weak.upgrade() {
                update_cedulas_ui(&ui, &list);
                let app_data = ui.global::<AppData>();
                app_data.set_cedulas_ready_to_download(false);
                app_data.set_cedulas_has_downloaded(false);
                app_data.set_cedulas_is_processing(false);
                app_data.set_cedulas_progress_fraction(0.0);
                app_data.set_cedulas_progress_text("".into());

                let mut sum = app_data.get_summary();
                sum.cedulas_total = 0;
                sum.cedulas_valid = 0;
                sum.cedulas_invalid = 0;
                sum.has_cedulas_excel = false;
                sum.cedulas_excel_path = "".into();
                app_data.set_summary(sum);

                let remaining_unproc: Vec<UnprocessedItemData> = app_data
                    .get_unprocessed_items()
                    .iter()
                    .filter(|item| item.category != "CÉDULA")
                    .collect();
                app_data.set_unprocessed_items(ModelRc::new(VecModel::from(remaining_unproc)));

                show_toast_message(&ui, "Se eliminaron todas las cédulas seleccionadas.");
            }
        });
    }

    // 7. Clear All
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        let cedulas_list_clone = cedulas_list.clone();
        let last_nom_clone = last_nominas_excel.clone();
        let last_ced_clone = last_cedulas_excel.clone();
        let last_nom_dir_clone = last_nominas_download_dir.clone();
        let last_ced_dir_clone = last_cedulas_download_dir.clone();
        app_logic.on_clear_all(move || {
            nominas_list_clone.borrow_mut().clear();
            cedulas_list_clone.borrow_mut().clear();
            if let Ok(mut g) = last_nom_clone.lock() { *g = None; }
            if let Ok(mut g) = last_ced_clone.lock() { *g = None; }
            if let Ok(mut g) = last_nom_dir_clone.lock() { *g = None; }
            if let Ok(mut g) = last_ced_dir_clone.lock() { *g = None; }

            if let Some(ui) = ui_weak.upgrade() {
                update_nominas_ui(&ui, &[]);
                update_cedulas_ui(&ui, &[]);
                let app_data = ui.global::<AppData>();
                app_data.set_has_results(false);
                app_data.set_status_message("Listo para procesar".into());
                app_data.set_nominas_ready_to_download(false);
                app_data.set_nominas_has_downloaded(false);
                app_data.set_nominas_is_processing(false);
                app_data.set_nominas_progress_fraction(0.0);
                app_data.set_nominas_progress_text("".into());
                app_data.set_cedulas_ready_to_download(false);
                app_data.set_cedulas_has_downloaded(false);
                app_data.set_cedulas_is_processing(false);
                app_data.set_cedulas_progress_fraction(0.0);
                app_data.set_cedulas_progress_text("".into());
                app_data.set_unprocessed_items(ModelRc::new(VecModel::default()));
                show_toast_message(&ui, "Se eliminaron todos los archivos seleccionados.");
            }
        });
    }

    // 8. Process Nóminas (Independent)
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        let last_nom_clone = last_nominas_excel.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_process_nominas(move || {
            let paths: Vec<PathBuf> = nominas_list_clone
                .borrow()
                .iter()
                .map(|e| e.path.clone())
                .collect();
            crate::app_logger::log_ui_event(
                "Carga Nóminas",
                "Barra Acciones",
                "Click Procesar Nóminas",
                &format!("Iniciando procesamiento de {} archivos de nómina", paths.len()),
            );
            spawn_nominas_worker(ui_weak.clone(), paths, last_nom_clone.clone(), pool_clone.clone());
        });
    }

    // 9. Process Cédulas (Independent)
    {
        let ui_weak = main_window.as_weak();
        let cedulas_list_clone = cedulas_list.clone();
        let last_ced_clone = last_cedulas_excel.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_process_cedulas(move || {
            let paths: Vec<PathBuf> = cedulas_list_clone
                .borrow()
                .iter()
                .map(|e| e.path.clone())
                .collect();
            crate::app_logger::log_ui_event(
                "Carga Cédulas",
                "Barra Acciones",
                "Click Procesar Cédulas",
                &format!("Iniciando procesamiento de {} archivos de cédula", paths.len()),
            );
            spawn_cedulas_worker(ui_weak.clone(), paths, last_ced_clone.clone(), pool_clone.clone());
        });
    }

    // 10. Process Both / Trigger available
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        let cedulas_list_clone = cedulas_list.clone();
        let last_nom_clone = last_nominas_excel.clone();
        let last_ced_clone = last_cedulas_excel.clone();
        let pool_clone = db_pool.clone();

        app_logic.on_process_files(move || {
            let nom_paths: Vec<PathBuf> = nominas_list_clone
                .borrow()
                .iter()
                .map(|e| e.path.clone())
                .collect();
            let ced_paths: Vec<PathBuf> = cedulas_list_clone
                .borrow()
                .iter()
                .map(|e| e.path.clone())
                .collect();

            crate::app_logger::log_ui_event(
                "Inicio / Procesamiento",
                "Barra Acciones",
                "Click Procesar Todos los Archivos",
                &format!("Nóminas: {}, Cédulas: {}", nom_paths.len(), ced_paths.len()),
            );

            if nom_paths.is_empty() && ced_paths.is_empty() {
                crate::app_logger::log_ui_error(
                    "Inicio / Procesamiento",
                    "Barra Acciones",
                    "Validación Sin Archivos",
                    "El usuario intentó procesar sin tener archivos agregados",
                );
                if let Some(ui) = ui_weak.upgrade() {
                    show_toast_message(&ui, "Por favor agrega al menos un archivo para procesar.");
                }
                return;
            }

            if !nom_paths.is_empty() {
                spawn_nominas_worker(ui_weak.clone(), nom_paths, last_nom_clone.clone(), pool_clone.clone());
            }
            if !ced_paths.is_empty() {
                spawn_cedulas_worker(ui_weak.clone(), ced_paths, last_ced_clone.clone(), pool_clone.clone());
            }
        });
    }

    // 11. Download/Save Nóminas Excel
    {
        let ui_weak = main_window.as_weak();
        let last_nom_clone = last_nominas_excel.clone();
        let last_nom_dir_clone = last_nominas_download_dir.clone();
        app_logic.on_download_nominas_excel(move || {
            crate::app_logger::log_ui_event(
                "Carga Nóminas",
                "Resultados",
                "Click Descargar Excel Nóminas",
                "Abriendo cuadro de diálogo para guardar reporte Excel",
            );
            let src_opt = last_nom_clone.lock().ok().and_then(|g| g.clone());
            if let Some(source) = src_opt {
                if let Some(dest) = rfd::FileDialog::new()
                    .set_title("Guardar Reporte de Nóminas Excel")
                    .set_file_name("reporte_nominas.xlsx")
                    .add_filter("Libro de Excel (*.xlsx)", &["xlsx"])
                    .save_file()
                {
                    if let Err(e) = fs::copy(&source, &dest) {
                        crate::app_logger::log_ui_error(
                            "Carga Nóminas",
                            "Guardar Archivo",
                            "Error Copiando Archivo Excel",
                            &e.to_string(),
                        );
                        if let Some(ui) = ui_weak.upgrade() {
                            show_toast_message(&ui, &format!("Error al guardar Excel: {}", e));
                        }
                    } else {
                        crate::app_logger::log_ui_event(
                            "Carga Nóminas",
                            "Guardar Archivo",
                            "Excel Guardado Exitosamente",
                            &format!("Ruta destino: {}", dest.display()),
                        );
                        if let Some(parent) = dest.parent() {
                            if let Ok(mut g) = last_nom_dir_clone.lock() {
                                *g = Some(parent.to_path_buf());
                            }
                        }
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.global::<AppData>().set_nominas_has_downloaded(true);
                            show_toast_message(&ui, &format!("Reporte de Nóminas guardado en: {}", dest.display()));
                        }
                    }
                }
            } else if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_error(
                    "Carga Nóminas",
                    "Guardar Archivo",
                    "Reporte Inexistente",
                    "Aún no se ha generado el reporte de Nóminas",
                );
                show_toast_message(&ui, "Aún no se ha generado el reporte de Nóminas.");
            }
        });
    }

    // 12. Download/Save Cédulas Excel
    {
        let ui_weak = main_window.as_weak();
        let last_ced_clone = last_cedulas_excel.clone();
        let last_ced_dir_clone = last_cedulas_download_dir.clone();
        app_logic.on_download_cedulas_excel(move || {
            crate::app_logger::log_ui_event(
                "Carga Cédulas",
                "Resultados",
                "Click Descargar Excel Cédulas",
                "Abriendo cuadro de diálogo para guardar reporte Excel",
            );
            let src_opt = last_ced_clone.lock().ok().and_then(|g| g.clone());
            if let Some(source) = src_opt {
                if let Some(dest) = rfd::FileDialog::new()
                    .set_title("Guardar Reporte de Cédulas SUA Excel")
                    .set_file_name("reporte_liquidaciones.xlsx")
                    .add_filter("Libro de Excel (*.xlsx)", &["xlsx"])
                    .save_file()
                {
                    if let Err(e) = fs::copy(&source, &dest) {
                        crate::app_logger::log_ui_error(
                            "Carga Cédulas",
                            "Guardar Archivo",
                            "Error Copiando Archivo Excel",
                            &e.to_string(),
                        );
                        if let Some(ui) = ui_weak.upgrade() {
                            show_toast_message(&ui, &format!("Error al guardar Excel: {}", e));
                        }
                    } else {
                        crate::app_logger::log_ui_event(
                            "Carga Cédulas",
                            "Guardar Archivo",
                            "Excel Guardado Exitosamente",
                            &format!("Ruta destino: {}", dest.display()),
                        );
                        if let Some(parent) = dest.parent() {
                            if let Ok(mut g) = last_ced_dir_clone.lock() {
                                *g = Some(parent.to_path_buf());
                            }
                        }
                        if let Some(ui) = ui_weak.upgrade() {
                            ui.global::<AppData>().set_cedulas_has_downloaded(true);
                            show_toast_message(&ui, &format!("Reporte de Cédulas guardado en: {}", dest.display()));
                        }
                    }
                }
            } else if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_error(
                    "Carga Cédulas",
                    "Guardar Archivo",
                    "Reporte Inexistente",
                    "Aún no se ha generado el reporte de Cédulas",
                );
                show_toast_message(&ui, "Aún no se ha generado el reporte de Cédulas.");
            }
        });
    }

    // 13. Open Nóminas Download Folder
    {
        let ui_weak = main_window.as_weak();
        let last_nom_clone = last_nominas_excel.clone();
        let last_nom_dir_clone = last_nominas_download_dir.clone();
        app_logic.on_open_nominas_folder(move || {
            let dir_opt = last_nom_dir_clone.lock().ok().and_then(|g| g.clone());
            let (target_dir, is_saved_dir) = if let Some(dir) = dir_opt {
                (dir, true)
            } else if let Some(src) = last_nom_clone.lock().ok().and_then(|g| g.clone()) {
                (src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("output")), false)
            } else {
                (std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join("output"), false)
            };

            let _ = fs::create_dir_all(&target_dir);
            open_folder_in_os(&target_dir);
            if let Some(ui) = ui_weak.upgrade() {
                let msg = if is_saved_dir {
                    format!("Abriendo carpeta de descarga: {}", target_dir.display())
                } else {
                    format!("Abriendo carpeta del reporte: {}", target_dir.display())
                };
                show_toast_message(&ui, &msg);
            }
        });
    }

    // 14. Open Cédulas Download Folder
    {
        let ui_weak = main_window.as_weak();
        let last_ced_clone = last_cedulas_excel.clone();
        let last_ced_dir_clone = last_cedulas_download_dir.clone();
        app_logic.on_open_cedulas_folder(move || {
            let dir_opt = last_ced_dir_clone.lock().ok().and_then(|g| g.clone());
            let (target_dir, is_saved_dir) = if let Some(dir) = dir_opt {
                (dir, true)
            } else if let Some(src) = last_ced_clone.lock().ok().and_then(|g| g.clone()) {
                (src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("output")), false)
            } else {
                (std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join("output"), false)
            };

            let _ = fs::create_dir_all(&target_dir);
            open_folder_in_os(&target_dir);
            if let Some(ui) = ui_weak.upgrade() {
                let msg = if is_saved_dir {
                    format!("Abriendo carpeta de descarga: {}", target_dir.display())
                } else {
                    format!("Abriendo carpeta del reporte: {}", target_dir.display())
                };
                show_toast_message(&ui, &msg);
            }
        });
    }

    // 15. Open General Output Folder
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_output_folder(move || {
            let output_dir = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("output");
            let _ = fs::create_dir_all(&output_dir);
            open_folder_in_os(&output_dir);

            if let Some(ui) = ui_weak.upgrade() {
                show_toast_message(&ui, "Abriendo carpeta de resultados...");
            }
        });
    }

    // 16. Copy text to clipboard
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_copy_text_to_clipboard(move |text| {
            let mut clipboard = arboard::Clipboard::new();
            match clipboard {
                Ok(ref mut cb) => {
                    let _ = cb.set_text(text.as_str());
                    if let Some(ui) = ui_weak.upgrade() {
                        show_toast_message(&ui, "Detalle del error copiado al portapapeles.");
                    }
                }
                Err(_) => {
                    if let Some(ui) = ui_weak.upgrade() {
                        show_toast_message(&ui, "No se pudo acceder al portapapeles.");
                    }
                }
            }
        });
    }

    // 17. Navegación entre vistas
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let cedulas_session_clone = cedulas_session.clone();
        let reporte_session_clone = reporte_session.clone();
        let nominas_session_clone = nominas_session.clone();
        let nominas_upload_session_clone = nominas_upload_session.clone();
        app_logic.on_navigate_view(move |view_idx| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let client_id = app_data.get_selected_cliente_id().to_string();
                let rp_id = app_data.get_selected_rp_id().to_string();
                let rev_id = app_data.get_selected_revision_id().to_string();

                let view_name = match view_idx {
                    0 => "Inicio / Procesamiento",
                    1 => "Clientes",
                    2 => "Registros Patronales",
                    3 => "Revisiones",
                    4 => "Registros de Obras",
                    5 => "Empleados",
                    6 => "Cédulas de Cuotas",
                    7 => "Reporte Liquidaciones",
                    8 => "Calendario",
                    9 => "Reporte Nóminas",
                    10 => "Carga Nóminas",
                    _ => "Vista Desconocida",
                };

                crate::app_logger::log_ui_event(
                    "Navegación",
                    "Sidebar / Menú",
                    "Cambio de Vista",
                    &format!("Destino: {} (ID {}), Cliente='{}', RP='{}', Rev='{}'", view_name, view_idx, client_id, rp_id, rev_id),
                );

                // Validación estricta de deshabilitación:
                // View 2 (RP) y View 5 (Empleados) requieren cliente seleccionado
                if (view_idx == 2 || view_idx == 5) && client_id.is_empty() {
                    crate::app_logger::log_ui_error(
                        "Navegación",
                        "Sidebar / Menú",
                        "Acceso Denegado a Vista",
                        &format!("No se puede acceder a {} (ID {}): requiere cliente seleccionado", view_name, view_idx),
                    );
                    app_data.set_current_view(1);
                    return;
                }
                // View 3 (Revisiones) deshabilitada hasta acceder desde RP ("Ver")
                if view_idx == 3 && rp_id.is_empty() {
                    crate::app_logger::log_ui_error(
                        "Navegación",
                        "Sidebar / Menú",
                        "Acceso Denegado a Vista",
                        &format!("No se puede acceder a {} (ID {}): requiere registro patronal seleccionado", view_name, view_idx),
                    );
                    app_data.set_current_view(if !client_id.is_empty() { 2 } else { 1 });
                    return;
                }
                // View 4 (Registros de Obras) deshabilitada hasta acceder desde Revisiones ("Ver Registros de Obras")
                if view_idx == 4 && rev_id.is_empty() {
                    crate::app_logger::log_ui_error(
                        "Navegación",
                        "Sidebar / Menú",
                        "Acceso Denegado a Vista",
                        &format!("No se puede acceder a {} (ID {}): requiere revisión seleccionada", view_name, view_idx),
                    );
                    app_data.set_current_view(if !rp_id.is_empty() { 3 } else if !client_id.is_empty() { 2 } else { 1 });
                    return;
                }
                // View 6 (Cédula de Cuotas) requiere registro patronal seleccionado
                if view_idx == 6 && rp_id.is_empty() {
                    crate::app_logger::log_ui_error(
                        "Navegación",
                        "Sidebar / Menú",
                        "Acceso Denegado a Vista",
                        &format!("No se puede acceder a {} (ID {}): requiere registro patronal seleccionado", view_name, view_idx),
                    );
                    app_data.set_current_view(if !client_id.is_empty() { 2 } else { 1 });
                    return;
                }
                if view_idx == 6 {
                    app_data.set_cedulas_access_allowed(true);
                }
                // View 7 (Reporte Liquidaciones) requiere registro patronal seleccionado
                if view_idx == 7 && rp_id.is_empty() && rev_id.is_empty() {
                    crate::app_logger::log_ui_error(
                        "Navegación",
                        "Sidebar / Menú",
                        "Acceso Denegado a Vista",
                        &format!("No se puede acceder a {} (ID {}): requiere registro patronal seleccionado", view_name, view_idx),
                    );
                    app_data.set_current_view(if !client_id.is_empty() { 2 } else { 1 });
                    return;
                }
                if view_idx == 7 {
                    app_data.set_reporte_liquidaciones_access_allowed(true);
                }

                // View 9 (Reporte Nóminas) requiere que existan nóminas cargadas
                let can_access_nominas = app_data.get_nominas_access_allowed()
                    || app_data.get_revision_has_nominas()
                    || app_data.get_nominas_data_exists();

                if view_idx == 9 && !can_access_nominas {
                    crate::app_logger::log_ui_error(
                        "Navegación",
                        "Sidebar / Menú",
                        "Acceso Denegado a Vista",
                        &format!("No se puede acceder a {} (ID {}): acceso a nóminas no permitido", view_name, view_idx),
                    );
                    app_data.set_current_view(if !rev_id.is_empty() { 4 } else if !rp_id.is_empty() { 3 } else if !client_id.is_empty() { 2 } else { 1 });
                    return;
                }
                if view_idx == 9 {
                    app_data.set_nominas_access_allowed(true);
                }

                app_data.set_current_view(view_idx);

                if view_idx == 6 {
                    let effective_rp = if !rp_id.is_empty() { rp_id.clone() } else { app_data.get_selected_rp_id().to_string() };
                    let need_load = {
                        let sess = cedulas_session_clone.lock().unwrap();
                        !effective_rp.is_empty() && (sess.audit_res.is_none() || sess.current_rp_id.as_deref() != Some(&effective_rp) || sess.files.is_empty())
                    };
                    if need_load {
                        sync_and_load_cedulas_view_data(&ui, &pool_clone, &cedulas_session_clone, &effective_rp);
                    } else {
                        let mut sess = cedulas_session_clone.lock().unwrap();
                        if sess.audit_res.is_some() {
                            update_cedulas_view_file_list_ui(&ui, &sess);
                            update_cedulas_audit_view_ui(&ui, &mut sess);
                            app_data.set_cedulas_view_state(2);
                        }
                    }
                }

                if view_idx == 7 {
                    // El reporte requiere una revisión seleccionada (nunca trabaja sobre el registro patronal)
                    if !rev_id.is_empty() {
                        sync_and_load_reporte_data(&ui, &pool_clone, &reporte_session_clone, &rev_id, false);
                    } else {
                        crate::show_toast_message(&ui, "Selecciona primero una revisión para ver el Reporte de Liquidaciones.");
                    }
                }



                if view_idx == 8 {
                    if let Some(ref pool) = pool_clone {
                        load_calendar_ui(&ui, pool, None, None, None);
                    }
                }

                if view_idx == 9 {
                    if let Some(ref pool) = pool_clone {
                        let _ = sync_and_load_nominas_data(&ui, pool, &nominas_session_clone, None);
                    }
                }

                if view_idx == 10 {
                    if let Some(ref pool) = pool_clone {
                        let _ = crate::nominas_upload_view_logic::load_nominas_upload_stats(&ui, pool, &nominas_upload_session_clone);
                    }
                }

                if let Some(ref pool) = pool_clone {
                    match view_idx {
                        1 => {
                            app_data.set_cedulas_access_allowed(false);
                            app_data.set_selected_cliente_id("".into());
                            app_data.set_selected_cliente_nombre("Todos los Clientes".into());
                            app_data.set_selected_cliente_rfc("".into());
                            app_data.set_selected_cliente_has_data(false);
                            app_data.set_selected_rp_id("".into());
                            app_data.set_selected_rp_registro("".into());
                            app_data.set_selected_rp_is_valid_structure(true);
                            app_data.set_selected_rp_delegacion("".into());
                            app_data.set_selected_rp_subdelegacion("".into());
                            app_data.set_selected_revision_id("".into());
                            app_data.set_selected_revision_nombre("".into());
                            app_data.set_selected_revision_folio("".into());
                            app_data.set_revision_has_cedulas(false);
                            app_data.set_revision_has_nominas(false);
                            app_data.set_revision_has_liquidaciones(false);
                            app_data.set_revision_has_obras(false);
                            app_data.set_revision_has_all_data(false);
                            app_data.set_reportes_liquidaciones_data_exists(false);
                            app_data.set_nominas_access_allowed(false);
                            load_clientes_ui(&ui, pool);
                        }
                        2 => {
                            app_data.set_selected_rp_id("".into());
                            app_data.set_selected_rp_registro("".into());
                            app_data.set_selected_rp_is_valid_structure(true);
                            app_data.set_selected_rp_delegacion("".into());
                            app_data.set_selected_rp_subdelegacion("".into());
                            app_data.set_selected_revision_id("".into());
                            app_data.set_selected_revision_nombre("".into());
                            app_data.set_selected_revision_folio("".into());
                            app_data.set_revision_has_cedulas(false);
                            app_data.set_revision_has_nominas(false);
                            app_data.set_revision_has_liquidaciones(false);
                            app_data.set_revision_has_obras(false);
                            app_data.set_revision_has_all_data(false);
                            app_data.set_cedulas_access_allowed(false);
                            app_data.set_reportes_liquidaciones_data_exists(false);
                            app_data.set_nominas_access_allowed(false);
                            load_registros_patronales_ui(&ui, pool, &client_id);
                        }
                        3 => {
                            app_data.set_selected_revision_id("".into());
                            app_data.set_selected_revision_nombre("".into());
                            app_data.set_selected_revision_folio("".into());
                            app_data.set_revision_has_cedulas(false);
                            app_data.set_revision_has_nominas(false);
                            app_data.set_revision_has_liquidaciones(false);
                            app_data.set_revision_has_obras(false);
                            app_data.set_revision_has_all_data(false);
                            app_data.set_cedulas_access_allowed(false);
                            app_data.set_reportes_liquidaciones_data_exists(false);
                            app_data.set_nominas_access_allowed(false);
                            load_revisiones_ui(&ui, pool, &rp_id);
                        }
                        4 => {
                            load_obras_ui(&ui, pool, &rev_id);
                        }
                        5 => {
                            load_empleados_ui(&ui, pool, &client_id);
                        }
                        _ => {}
                    }
                    update_global_stats(&ui, pool);
                }
            }
        });
    }

    // 18. Seleccionar Cliente -> Ver Registros Patronales
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_select_cliente_for_rp(move |id, nombre, rfc, has_data| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Catálogo Clientes",
                    "Tabla Clientes",
                    "Click Ver Registros Patronales",
                    &format!("Cliente: '{}' (ID: {}, RFC: {}, has_data: {})", nombre, id, rfc, has_data),
                );
                let app_data = ui.global::<AppData>();
                app_data.set_selected_cliente_id(id.clone());
                app_data.set_selected_cliente_nombre(nombre);
                app_data.set_selected_cliente_rfc(rfc);
                app_data.set_selected_cliente_has_data(has_data);
                app_data.set_rp_form_complementary_enabled(has_data);
                app_data.set_current_view(2);
                if let Some(ref pool) = pool_clone {
                    load_registros_patronales_ui(&ui, pool, id.as_str());
                    update_global_stats(&ui, pool);
                }
            }
        });
    }

    // 18b. Seleccionar RP -> Ver Revisiones
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_select_rp_for_revisiones(move |rp_id, rp_registro, subdelegacion, delegacion| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Catálogo Registros Patronales",
                    "Tabla RP",
                    "Click Ver Revisiones",
                    &format!("RP: '{}' (ID: {}, Delegación: {}, Subdelegación: {})", rp_registro, rp_id, delegacion, subdelegacion),
                );
                let app_data = ui.global::<AppData>();
                let is_valid = validate_registro_patronal(&rp_registro).is_ok();
                app_data.set_selected_rp_id(rp_id.clone());
                app_data.set_selected_rp_registro(rp_registro);
                app_data.set_selected_rp_is_valid_structure(is_valid);
                app_data.set_selected_rp_subdelegacion(subdelegacion);
                app_data.set_selected_rp_delegacion(delegacion);
                app_data.set_current_view(3);
                if let Some(ref pool) = pool_clone {
                    load_revisiones_ui(&ui, pool, rp_id.as_str());
                    update_global_stats(&ui, pool);
                }
            }
        });
    }

    // 18c. Seleccionar Revisión -> Ver Registros de Obras
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let cedulas_session_clone = cedulas_session.clone();
        app_logic.on_select_revision_for_obras(move |rev_id, rev_nombre, rev_folio| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Catálogo Revisiones",
                    "Tabla Revisiones",
                    "Click Ver Registros de Obras",
                    &format!("Revisión: '{}' (ID: {}, Folio: {})", rev_nombre, rev_id, rev_folio),
                );
                let app_data = ui.global::<AppData>();
                let prev_rev_id = app_data.get_selected_revision_id().to_string();
                if prev_rev_id != rev_id.as_str() {
                    let mut sess = cedulas_session_clone.lock().unwrap();
                    reset_cedulas_ui_to_state_zero(&ui, &mut sess);
                }
                app_data.set_selected_revision_id(rev_id.clone());
                app_data.set_selected_revision_nombre(rev_nombre);
                app_data.set_selected_revision_folio(rev_folio);
                app_data.set_current_view(4);
                if let Some(ref pool) = pool_clone {
                    load_obras_ui(&ui, pool, rev_id.as_str());
                    update_global_stats(&ui, pool);
                    
                    // Verificar qué datos existen para esta revisión y actualizar propiedades de navegación
                    if let Ok(mut client) = pool.get() {
                        if let Ok(rev_uuid) = Uuid::parse_str(rev_id.as_str()) {
                            if let Ok((has_cedulas, has_nominas, has_liquidaciones, has_obras)) = 
                                check_all_data_exists(&mut client, rev_uuid) {
                                app_data.set_revision_has_cedulas(has_cedulas);
                                app_data.set_revision_has_nominas(has_nominas);
                                app_data.set_revision_has_liquidaciones(has_liquidaciones);
                                app_data.set_revision_has_obras(has_obras);
                                
                                // La revisión está completa si tiene obras, cédulas, nóminas y liquidaciones
                                let all_data = has_obras && has_cedulas && has_nominas && has_liquidaciones;
                                app_data.set_revision_has_all_data(all_data);

                                if has_nominas {
                                    app_data.set_nominas_access_allowed(true);
                                }
                                if has_liquidaciones || has_cedulas {
                                    app_data.set_cedulas_access_allowed(true);
                                    app_data.set_reporte_liquidaciones_access_allowed(true);
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    // 19. Seleccionar Cliente -> Ver Empleados
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_select_cliente_for_empleados(move |id, nombre, rfc| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::app_logger::log_ui_event(
                    "Catálogo Clientes",
                    "Tabla Clientes",
                    "Click Ver Empleados",
                    &format!("Cliente: '{}' (ID: {}, RFC: {})", nombre, id, rfc),
                );
                let app_data = ui.global::<AppData>();
                app_data.set_selected_cliente_id(id.clone());
                app_data.set_selected_cliente_nombre(nombre);
                app_data.set_selected_cliente_rfc(rfc);
                app_data.set_current_view(5);
                if let Some(ref pool) = pool_clone {
                    load_empleados_ui(&ui, pool, id.as_str());
                    update_global_stats(&ui, pool);
                }
            }
        });
    }

    // --- Callbacks Revisiones Patronales ---
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_reload_revisiones(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let rp_id = ui.global::<AppData>().get_selected_rp_id().to_string();
                    load_revisiones_ui(&ui, pool, &rp_id);
                    update_global_stats(&ui, pool);
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_sort_revisiones(move |field, asc| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_revisiones_sort_field(field);
                app_data.set_revisiones_sort_asc(asc);
                if let Some(ref pool) = pool_clone {
                    let rp_id = app_data.get_selected_rp_id().to_string();
                    load_revisiones_ui(&ui, pool, &rp_id);
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_new_revision_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let rp_id = app_data.get_selected_rp_id().to_string();
                let rp_reg = app_data.get_selected_rp_registro().to_string();
                if rp_id.trim().is_empty() {
                    show_toast_message(&ui, "Debe seleccionar un Registro Patronal antes de crear una revisión.");
                    return;
                }
                if let Err(e) = validate_registro_patronal(&rp_reg) {
                    show_toast_message(&ui, &format!("No se puede crear la revisión: {}", e));
                    return;
                }
                app_data.set_revision_form_is_editing(false);
                app_data.set_rev_form_id("".into());
                app_data.set_rev_form_nombre("".into());
                app_data.set_rev_form_folio("".into());
                app_data.set_rev_form_concepto_val("-- Seleccionar Concepto Diferencia --".into());
                app_data.set_rev_form_estatus_val("En Revision".into());
                app_data.set_rev_form_estatus_imss_val("PENDIENTE DICTAMEN IMSS".into());
                app_data.set_rev_form_fecha_entrega_rev("".into());
                app_data.set_rev_form_fecha_max_entrega_rev("".into());
                app_data.set_rev_form_fecha_entrega_imss("".into());
                app_data.set_rev_form_fecha_max_entrega_imss("".into());
                app_data.set_rev_form_fecha_aprobacion_imss("".into());
                app_data.set_rev_form_fecha_max_aprobacion_imss("".into());
                app_data.set_rev_form_pidio_prorroga(false);
                app_data.set_rev_form_fecha_aprobacion_prorroga("".into());
                app_data.set_rev_form_fecha_entrega_con_prorroga("".into());
                app_data.set_rev_form_periodo_inicio("".into());
                app_data.set_rev_form_periodo_fin("".into());
                app_data.set_show_revision_modal(true);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_revision_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_revision_modal(false);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let holidays_clone = imss_inhabiles.clone();
        app_logic.on_select_revision_for_edit(move |rev_id| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let rp_id = app_data.get_selected_rp_id().to_string();
                if let Some(ref pool) = pool_clone {
                    if let Ok(mut client) = pool.get() {
                        if let Ok(revs) = fetch_revisiones_by_rp(&mut client, &rp_id) {
                            if let Some(r) = revs.into_iter().find(|x| x.id == rev_id.as_str()) {
                                app_data.set_revision_form_is_editing(true);
                                app_data.set_rev_form_id(r.id.into());
                                app_data.set_rev_form_nombre(r.nombre.into());
                                let folio_edit = if r.folio.trim() == "S/F" { "" } else { r.folio.trim() };
                                app_data.set_rev_form_folio(folio_edit.into());

                                let concepto_selected = if let Some(ref conc_id) = r.id_concepto_diferencia {
                                    if !conc_id.trim().is_empty()
                                        && !r.concepto_diferencia.trim().is_empty()
                                        && r.concepto_diferencia != "Sin Concepto Asignado"
                                        && !r.concepto_diferencia.starts_with("--")
                                    {
                                        r.concepto_diferencia.clone()
                                    } else {
                                        "-- Seleccionar Concepto Diferencia --".to_string()
                                    }
                                } else {
                                    "-- Seleccionar Concepto Diferencia --".to_string()
                                };
                                app_data.set_rev_form_concepto_val(concepto_selected.into());

                                let est_val = if !r.estatus.trim().is_empty() {
                                    r.estatus.clone()
                                } else {
                                    "EN REVISIÓN".to_string()
                                };
                                app_data.set_rev_form_estatus_val(est_val.into());

                                let est_imss_val = if !r.estatus_imss.trim().is_empty() {
                                    r.estatus_imss.clone()
                                } else {
                                    "PENDIENTE DICTAMEN IMSS".to_string()
                                };
                                app_data.set_rev_form_estatus_imss_val(est_imss_val.into());
                                app_data.set_rev_form_fecha_entrega_imss(format_date_opt(r.fecha_entrega_imss).into());
                                app_data.set_rev_form_fecha_aprobacion_imss(format_date_opt(r.fecha_aprobacion_imss).into());

                                let holidays_set = holidays_clone.read().unwrap();
                                let f_max_aprob = if let Some(d_ent) = r.fecha_entrega_imss {
                                    Some(add_business_days(d_ent, 15, Some(&*holidays_set)))
                                } else if let Some(d_aprob) = r.fecha_aprobacion_imss {
                                    Some(add_business_days(d_aprob, 15, Some(&*holidays_set)))
                                } else {
                                    r.fecha_maxima_aprobacion_imss
                                };
                                app_data.set_rev_form_fecha_max_aprobacion_imss(format_date_opt(f_max_aprob).into());

                                let f_max_rev = if let Some(d_aprob) = r.fecha_aprobacion_imss {
                                    Some(add_business_days(d_aprob, 40, Some(&*holidays_set)))
                                } else {
                                    r.fecha_maxima_entrega_revision
                                };
                                app_data.set_rev_form_fecha_max_entrega_rev(format_date_opt(f_max_rev).into());

                                app_data.set_rev_form_fecha_entrega_rev(format_date_opt(r.fecha_entrega_revision).into());
                                let f_max_imss = if let Some(d_rev) = r.fecha_entrega_revision {
                                    Some(add_business_days(d_rev, 120, Some(&*holidays_set)))
                                } else {
                                    r.fecha_maxima_entrega_imss
                                };
                                app_data.set_rev_form_fecha_max_entrega_imss(format_date_opt(f_max_imss).into());

                                app_data.set_rev_form_pidio_prorroga(r.pidio_prorroga);
                                app_data.set_rev_form_fecha_aprobacion_prorroga(format_date_opt(r.fecha_aprobacion_prorroga).into());
                                let f_ent_pror = if let Some(d_max) = f_max_rev {
                                    Some(add_business_days(d_max, 10, Some(&*holidays_set)))
                                } else {
                                    r.fecha_entrega_con_prorroga
                                };
                                app_data.set_rev_form_fecha_entrega_con_prorroga(format_date_opt(f_ent_pror).into());
                                app_data.set_rev_form_periodo_inicio(format_date_opt(r.periodo_inicio).into());
                                app_data.set_rev_form_periodo_fin(format_date_opt(r.periodo_fin).into());
                                app_data.set_show_revision_modal(true);

                                // Verificar qué datos existen para esta revisión y actualizar propiedades de navegación
                                if let Ok(rev_uuid) = Uuid::parse_str(rev_id.as_str()) {
                                    if let Ok((has_cedulas, has_nominas, has_liquidaciones, has_obras)) = 
                                        check_all_data_exists(&mut client, rev_uuid) {
                                        app_data.set_revision_has_cedulas(has_cedulas);
                                        app_data.set_revision_has_nominas(has_nominas);
                                        app_data.set_revision_has_liquidaciones(has_liquidaciones);
                                        app_data.set_revision_has_obras(has_obras);
                                        
                                        let all_data = has_obras && has_cedulas && has_nominas && has_liquidaciones;
                                        app_data.set_revision_has_all_data(all_data);

                                        if has_nominas {
                                            app_data.set_nominas_access_allowed(true);
                                        }
                                        if has_liquidaciones || has_cedulas {
                                            app_data.set_reporte_liquidaciones_access_allowed(true);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        let holidays_clone = imss_inhabiles.clone();
        app_logic.on_submit_revision(move |id, nombre, folio, concepto_val, estatus_val, estatus_imss_val, f_ent_rev, f_max_ent_rev, f_ent_imss, f_max_ent_imss, f_aprob_imss, pidio_pror, f_aprob_pror, f_ent_pror, per_ini, per_fin| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let rp_id = app_data.get_selected_rp_id().to_string();
                let rp_reg = app_data.get_selected_rp_registro().to_string();
                if let Err(e) = validate_registro_patronal(&rp_reg) {
                    show_toast_message(&ui, &format!("No se puede guardar la revisión: {}", e));
                    return;
                }
                if nombre.trim().is_empty() {
                    show_toast_message(&ui, "El nombre de la revisión es obligatorio.");
                    return;
                }

                // Periodo revisado: ambas fechas deben ser válidas y en orden, o ambas vacías
                let d_per_ini = parse_date_opt(per_ini.as_str());
                let d_per_fin = parse_date_opt(per_fin.as_str());
                if let (Some(i), Some(f)) = (d_per_ini, d_per_fin) {
                    if i > f {
                        show_toast_message(&ui, "El periodo revisado es inválido: el inicio es posterior al fin.");
                        return;
                    }
                }
                let (periodo_ini_val, periodo_fin_val, periodo_warn) = match (d_per_ini, d_per_fin) {
                    (Some(i), Some(f)) => (Some(i), Some(f), None),
                    (None, None) => (None, None, None),
                    _ => (
                        None,
                        None,
                        Some("Captura inicio y fin del periodo revisado (o deja ambos vacíos)."),
                    ),
                };
                if let Some(ref pool) = pool_clone {
                    let mut client = match pool.get() {
                        Ok(c) => c,
                        Err(e) => {
                            show_toast_message(&ui, &format!("Error de conexión: {}", e));
                            return;
                        }
                    };

                    let conc_id = if !concepto_val.trim().is_empty()
                        && !concepto_val.starts_with("--")
                        && concepto_val != "Sin Concepto Asignado"
                    {
                        client.query_one("SELECT id FROM public.conceptos WHERE nombre = $1 LIMIT 1", &[&concepto_val.trim()])
                            .map(|r| { let u: uuid::Uuid = r.get(0); u.to_string() }).ok()
                    } else {
                        None
                    };

                    let conc_name = if conc_id.is_some() {
                        concepto_val.trim().to_string()
                    } else {
                        "Sin Concepto Asignado".to_string()
                    };

                    let est_id = if !estatus_val.trim().is_empty() && !estatus_val.starts_with("--") {
                        let clean_est = estatus_val.trim();
                        client.query_one("SELECT id FROM public.estatus WHERE UPPER(TRIM(nombre)) = UPPER(TRIM($1)) OR UPPER(TRIM(REPLACE(nombre, 'Ó', 'O'))) = UPPER(TRIM(REPLACE($1, 'Ó', 'O'))) LIMIT 1", &[&clean_est])
                            .or_else(|_| client.query_one("SELECT id FROM public.estatus WHERE UPPER(nombre) LIKE '%' || UPPER($1) || '%' LIMIT 1", &[&clean_est]))
                            .map(|r| { let u: uuid::Uuid = r.get(0); u.to_string() }).ok()
                    } else {
                        None
                    };

                    let est_imss_id = if !estatus_imss_val.trim().is_empty() && !estatus_imss_val.starts_with("--") {
                        client.query_one("SELECT id FROM public.estatus WHERE nombre = $1 LIMIT 1", &[&estatus_imss_val.trim()])
                            .map(|r| { let u: uuid::Uuid = r.get(0); u.to_string() }).ok()
                    } else {
                        None
                    };

                    let holidays_set = holidays_clone.read().unwrap();
                    let d_aprob = parse_date_opt(f_aprob_imss.as_str());
                    let d_max_ent_rev = parse_date_opt(f_max_ent_rev.as_str())
                        .or_else(|| d_aprob.map(|d| add_business_days(d, 40, Some(&*holidays_set))));
                    let d_aprob_pror = parse_date_opt(f_aprob_pror.as_str());
                    let d_ent_pror = if pidio_pror {
                        d_max_ent_rev.map(|d_max| add_business_days(d_max, 10, Some(&*holidays_set)))
                            .or_else(|| parse_date_opt(f_ent_pror.as_str()))
                    } else {
                        None
                    };

                    let item = RevisionItem {
                        id: id.to_string(),
                        id_registro_patronal: rp_id.clone(),
                        id_concepto_diferencia: conc_id,
                        concepto_diferencia: conc_name,
                        id_estatus_imss: est_imss_id,
                        estatus_imss: estatus_imss_val.to_string(),
                        id_estatus: est_id,
                        estatus: estatus_val.to_string(),
                        nombre: nombre.to_string(),
                        folio: folio.to_string(),
                        fecha_entrega_revision: parse_date_opt(f_ent_rev.as_str()),
                        fecha_maxima_entrega_revision: d_max_ent_rev,
                        fecha_entrega_imss: parse_date_opt(f_ent_imss.as_str()),
                        fecha_maxima_entrega_imss: parse_date_opt(f_max_ent_imss.as_str()),
                        fecha_aprobacion_imss: d_aprob,
                        fecha_maxima_aprobacion_imss: parse_date_opt(app_data.get_rev_form_fecha_max_aprobacion_imss().as_str()),
                        pidio_prorroga: pidio_pror,
                        fecha_aprobacion_prorroga: d_aprob_pror,
                        fecha_entrega_con_prorroga: d_ent_pror,
                        periodo_inicio: periodo_ini_val,
                        periodo_fin: periodo_fin_val,
                        ..Default::default()
                    };

                    let is_edit = !id.is_empty();
                    let res = if is_edit {
                        update_revision(&mut client, &item)
                    } else {
                        create_revision(&mut client, &item)
                    };

                    match res {
                        Ok(_) => {
                            let client_nom = app_data.get_selected_cliente_nombre().to_string();
                            let rp_reg = app_data.get_selected_rp_registro().to_string();
                            let rev_nom = item.nombre.trim().to_uppercase();
                            thread::spawn(move || {
                                let _ = crate::rustfs::ensure_revision_structure(&client_nom, &rp_reg, &rev_nom);
                            });

                            // Al editar, el universo de reportes depende de las fechas/periodo: resincronizar.
                            if is_edit {
                                if let Ok(uuid_rev) = Uuid::parse_str(&item.id) {
                                    let ui_weak2 = ui_weak.clone();
                                    let pool_bg = pool.clone();
                                    thread::spawn(move || {
                                        let res_sync = match pool_bg.get() {
                                            Ok(mut client_bg) => crate::db::obras_ops::resincronizar_universo(&mut client_bg, uuid_rev),
                                            Err(e) => Err(format!("Error de conexión: {}", e)),
                                        };
                                        let _ = slint::invoke_from_event_loop(move || {
                                            if let Some(ui) = ui_weak2.upgrade() {
                                                match res_sync {
                                                    Ok(cambio) => {
                                                        let texto = mensaje_cambio_universo(&cambio);
                                                        if !texto.is_empty() {
                                                            show_toast_message(&ui, &texto);
                                                        }
                                                    }
                                                    Err(e) => {
                                                        show_toast_message(&ui, &format!("No se pudo resincronizar el reporte: {}", e));
                                                    }
                                                }
                                            }
                                        });
                                    });
                                }
                            }

                            app_data.set_show_revision_modal(false);
                            load_revisiones_ui(&ui, pool, &rp_id);
                            update_global_stats(&ui, pool);
                            let msg = if is_edit { "Revisión actualizada exitosamente." } else { "Revisión creada exitosamente." };
                            let msg_final = match periodo_warn {
                                Some(w) => format!("{} {}", msg, w),
                                None => msg.to_string(),
                            };
                            show_toast_message(&ui, &msg_final);
                        }
                        Err(e) => {
                            show_toast_message(&ui, &format!("Error al guardar revisión: {}", e));
                        }
                    }
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_delete_revision_dialog(move |id, nombre, folio| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_delete_revision_id(id);
                app_data.set_delete_revision_nombre(nombre);
                app_data.set_delete_revision_folio(folio);
                app_data.set_show_delete_revision_modal(true);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_delete_revision_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_revision_modal(false);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_confirm_delete_revision(move |rev_id| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_revision_modal(false);
                show_toast_message(&ui, "Eliminando revisión...");

                let ui_weak2 = ui_weak.clone();
                let pool_opt = pool_clone.clone();
                let rev_id_str = rev_id.to_string();

                thread::spawn(move || {
                    if let Some(ref pool) = pool_opt {
                        let res = match pool.get() {
                            Ok(mut client) => {
                                let mut cliente_nom = String::new();
                                let mut rp_reg = String::new();
                                let mut rev_nom = String::new();
                                if let Ok(rev_uuid) = Uuid::parse_str(&rev_id_str) {
                                    if let Ok(Some(row)) = client.query_opt(
                                        "SELECT c.nombre, rp.registro, r.nombre
                                         FROM public.revisiones r
                                         JOIN public.registros_patronales rp ON r.id_registro_patronal = rp.id
                                         JOIN public.clientes c ON rp.id_cliente = c.id
                                         WHERE r.id = $1",
                                        &[&rev_uuid],
                                    ) {
                                        cliente_nom = row.get(0);
                                        rp_reg = row.get(1);
                                        rev_nom = row.get(2);
                                    }
                                }
                                let del_res = delete_revision(&mut client, rev_id_str.as_str());
                                if del_res.is_ok() && !cliente_nom.is_empty() && !rp_reg.is_empty() && !rev_nom.is_empty() {
                                    let _ = crate::rustfs::move_revision_to_trash(&cliente_nom, &rp_reg, &rev_nom);
                                }
                                del_res
                            }
                            Err(e) => Err(format!("Error obteniendo conexión a la BD: {}", e)),
                        };

                        let pool_for_ui = pool.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak2.upgrade() {
                                match res {
                                    Ok(_) => {
                                        let app_data = ui.global::<AppData>();
                                        let rp_id = app_data.get_selected_rp_id().to_string();
                                        if app_data.get_selected_revision_id().as_str() == rev_id_str.as_str() {
                                            app_data.set_selected_revision_id("".into());
                                            app_data.set_selected_revision_nombre("".into());
                                        }
                                        load_revisiones_ui(&ui, &pool_for_ui, &rp_id);
                                        update_global_stats(&ui, &pool_for_ui);
                                        show_toast_message(&ui, "Revisión eliminada correctamente.");
                                    }
                                    Err(e) => {
                                        show_toast_message(&ui, &format!("Error al eliminar revisión: {}", e));
                                    }
                                }
                            }
                        });
                    }
                });
            }
        });
    }

    // Callbacks para cálculo automático de fechas en formulario de revisión
    {
        let ui_weak = main_window.as_weak();
        let holidays_clone = imss_inhabiles.clone();
        app_logic.on_on_fecha_entrega_imss_changed(move |date_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                if let Some(d) = parse_date_opt(date_str.as_str()) {
                    let holidays_set = holidays_clone.read().unwrap();
                    let max_aprob = add_business_days(d, 15, Some(&*holidays_set));
                    app_data.set_rev_form_fecha_max_aprobacion_imss(format_date_opt(Some(max_aprob)).into());
                } else {
                    app_data.set_rev_form_fecha_max_aprobacion_imss("".into());
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let holidays_clone = imss_inhabiles.clone();
        app_logic.on_on_fecha_aprobacion_imss_changed(move |date_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                if let Some(d) = parse_date_opt(date_str.as_str()) {
                    let holidays_set = holidays_clone.read().unwrap();
                    // 1. 40 días hábiles para Fecha Máxima Entrega Revisión (a partir del día siguiente)
                    let max_ent_rev = add_business_days(d, 40, Some(&*holidays_set));
                    app_data.set_rev_form_fecha_max_entrega_rev(format_date_opt(Some(max_ent_rev)).into());

                    // Fecha de Entrega con Prórroga: 10 días hábiles después de Fecha Máxima de Entrega Revisión
                    let ent_pror = add_business_days(max_ent_rev, 10, Some(&*holidays_set));
                    app_data.set_rev_form_fecha_entrega_con_prorroga(format_date_opt(Some(ent_pror)).into());

                    // 2. Si Fecha Máx Aprobación IMSS no se había calculado (o fecha entrega imss vacía):
                    let f_ent_imss_str = app_data.get_rev_form_fecha_entrega_imss().to_string();
                    if parse_date_opt(&f_ent_imss_str).is_none() {
                        let max_aprob = add_business_days(d, 15, Some(&*holidays_set));
                        app_data.set_rev_form_fecha_max_aprobacion_imss(format_date_opt(Some(max_aprob)).into());
                    }
                } else {
                    app_data.set_rev_form_fecha_max_entrega_rev("".into());
                    app_data.set_rev_form_fecha_entrega_con_prorroga("".into());
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let holidays_clone = imss_inhabiles.clone();
        app_logic.on_on_fecha_entrega_rev_changed(move |date_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                if let Some(d) = parse_date_opt(date_str.as_str()) {
                    let holidays_set = holidays_clone.read().unwrap();
                    // 120 días hábiles para Fecha Máxima Entrega IMSS (sin fines de semana ni días festivos)
                    let max_ent_imss = add_business_days(d, 120, Some(&*holidays_set));
                    app_data.set_rev_form_fecha_max_entrega_imss(format_date_opt(Some(max_ent_imss)).into());
                } else {
                    app_data.set_rev_form_fecha_max_entrega_imss("".into());
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let holidays_clone = imss_inhabiles.clone();
        app_logic.on_on_fecha_aprobacion_prorroga_changed(move |_date_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let f_max_str = app_data.get_rev_form_fecha_max_entrega_rev().to_string();
                if let Some(d_max) = parse_date_opt(&f_max_str) {
                    let holidays_set = holidays_clone.read().unwrap();
                    let ent_pror = add_business_days(d_max, 10, Some(&*holidays_set));
                    app_data.set_rev_form_fecha_entrega_con_prorroga(format_date_opt(Some(ent_pror)).into());
                }
            }
        });
    }

    // Modal: Abrir/Cerrar/Confirmar Eliminación de Cliente
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_delete_cliente_dialog(move |id, nombre| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_delete_cliente_id(id);
                app_data.set_delete_cliente_nombre(nombre);
                app_data.set_show_delete_cliente_modal(true);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_delete_cliente_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_cliente_modal(false);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_confirm_delete_cliente(move |cliente_id| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_cliente_modal(false);
                show_toast_message(&ui, "Eliminando cliente y registros asociados...");

                let ui_weak2 = ui_weak.clone();
                let pool_opt = pool_clone.clone();
                let cliente_id_str = cliente_id.to_string();

                thread::spawn(move || {
                    let (res, pool_for_ui) = if let Some(ref pool) = pool_opt {
                        let res = match pool.get() {
                            Ok(mut client) => {
                                let mut cliente_nom = String::new();
                                if let Ok(c_uuid) = Uuid::parse_str(&cliente_id_str) {
                                    if let Ok(Some(row)) = client.query_opt(
                                        "SELECT nombre FROM public.clientes WHERE id = $1",
                                        &[&c_uuid],
                                    ) {
                                        cliente_nom = row.get(0);
                                    }
                                }
                                let del_res = delete_cliente(&mut client, cliente_id_str.as_str());
                                if del_res.is_ok() && !cliente_nom.is_empty() {
                                    let _ = crate::rustfs::move_client_to_trash(&cliente_nom);
                                }
                                del_res
                            }
                            Err(e) => Err(format!("Error obteniendo conexión a la BD: {}", e)),
                        };
                        (res, Some(pool.clone()))
                    } else {
                        let diag = crate::db::diagnose_postgres();
                        (Err(format!("Base de datos no disponible: {}. {}", diag.status_text, diag.detail_text)), None)
                    };

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_weak2.upgrade() {
                            match res {
                                Ok(_) => {
                                    let app_data = ui.global::<AppData>();
                                    if app_data.get_selected_cliente_id().as_str() == cliente_id_str.as_str() {
                                        app_data.set_selected_cliente_id("".into());
                                        app_data.set_selected_cliente_nombre("".into());
                                        app_data.set_selected_cliente_rfc("".into());
                                        app_data.set_selected_rp_id("".into());
                                        app_data.set_selected_rp_registro("".into());
                                        app_data.set_selected_revision_id("".into());
                                        app_data.set_selected_revision_nombre("".into());
                                        if let Some(ref p) = pool_for_ui {
                                            load_registros_patronales_ui(&ui, p, "");
                                        }
                                    }
                                    if let Some(ref p) = pool_for_ui {
                                        load_clientes_ui(&ui, p);
                                        update_global_stats(&ui, p);
                                    }
                                    crate::app_logger::log_ui_event(
                                        "Clientes",
                                        "Catálogo",
                                        "Eliminar Cliente",
                                        &format!("Cliente '{}' eliminado exitosamente", cliente_id_str),
                                    );
                                    show_toast_message(&ui, "Cliente y datos asociados eliminados correctamente.");
                                }
                                Err(e) => {
                                    crate::app_logger::log_db_simple_error(
                                        "clientes",
                                        "delete_cliente",
                                        &format!("Error eliminando cliente {}: {}", cliente_id_str, e),
                                    );
                                    show_toast_message(&ui, &format!("Error al eliminar cliente: {}", e));
                                }
                            }
                        }
                    });
                });
            }
        });
    }

    // Modal: Abrir/Cerrar/Confirmar Eliminación de Registro Patronal
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_delete_rp_dialog(move |id, registro| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_delete_rp_id(id);
                app_data.set_delete_rp_registro(registro);
                app_data.set_show_delete_rp_modal(true);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_delete_rp_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_rp_modal(false);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_confirm_delete_rp(move |rp_id| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_rp_modal(false);
                show_toast_message(&ui, "Eliminando registro patronal...");

                let ui_weak2 = ui_weak.clone();
                let pool_opt = pool_clone.clone();
                let rp_id_str = rp_id.to_string();

                thread::spawn(move || {
                    if let Some(ref pool) = pool_opt {
                        let res = match pool.get() {
                            Ok(mut client) => {
                                let mut cliente_nom = String::new();
                                let mut rp_reg = String::new();
                                if let Ok(rp_uuid) = Uuid::parse_str(&rp_id_str) {
                                    if let Ok(Some(row)) = client.query_opt(
                                        "SELECT c.nombre, rp.registro
                                         FROM public.registros_patronales rp
                                         JOIN public.clientes c ON rp.id_cliente = c.id
                                         WHERE rp.id = $1",
                                        &[&rp_uuid],
                                    ) {
                                        cliente_nom = row.get(0);
                                        rp_reg = row.get(1);
                                    }
                                }
                                let del_res = delete_registro_patronal(&mut client, rp_id_str.as_str());
                                if del_res.is_ok() && !cliente_nom.is_empty() && !rp_reg.is_empty() {
                                    let _ = crate::rustfs::move_rp_to_trash(&cliente_nom, &rp_reg);
                                }
                                del_res
                            }
                            Err(e) => Err(format!("Error obteniendo conexión a la BD: {}", e)),
                        };

                        let pool_for_ui = pool.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak2.upgrade() {
                                match res {
                                    Ok(_) => {
                                        let app_data = ui.global::<AppData>();
                                        let cliente_id = app_data.get_selected_cliente_id().to_string();
                                        if app_data.get_selected_rp_id().as_str() == rp_id_str.as_str() {
                                            app_data.set_selected_rp_id("".into());
                                            app_data.set_selected_rp_registro("".into());
                                            app_data.set_selected_revision_id("".into());
                                            app_data.set_selected_revision_nombre("".into());
                                        }
                                        load_registros_patronales_ui(&ui, &pool_for_ui, &cliente_id);
                                        load_clientes_ui(&ui, &pool_for_ui);
                                        update_global_stats(&ui, &pool_for_ui);
                                        show_toast_message(&ui, "Registro patronal y datos asociados eliminados correctamente.");
                                    }
                                    Err(e) => {
                                        show_toast_message(&ui, &format!("Error al eliminar registro patronal: {}", e));
                                    }
                                }
                            }
                        });
                    }
                });
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_filter_revisiones_search(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let rp_id = ui.global::<AppData>().get_selected_rp_id().to_string();
                    load_revisiones_ui(&ui, pool, &rp_id);
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_filter_revisiones_tab(move |tab| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_revisiones_filter_tab(tab);
                if let Some(ref pool) = pool_clone {
                    let rp_id = ui.global::<AppData>().get_selected_rp_id().to_string();
                    load_revisiones_ui(&ui, pool, &rp_id);
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let holidays_clone = imss_inhabiles.clone();
        app_logic.on_toggle_rev_prorroga(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let cur = app_data.get_rev_form_pidio_prorroga();
                let next = !cur;
                app_data.set_rev_form_pidio_prorroga(next);
                if next {
                    let f_max_str = app_data.get_rev_form_fecha_max_entrega_rev().to_string();
                    if let Some(d_max) = parse_date_opt(&f_max_str) {
                        let holidays_set = holidays_clone.read().unwrap();
                        let ent_pror = add_business_days(d_max, 10, Some(&*holidays_set));
                        app_data.set_rev_form_fecha_entrega_con_prorroga(format_date_opt(Some(ent_pror)).into());
                    }
                }
            }
        });
    }

    // --- Callbacks Registros de Obras ---
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_reload_obras(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let rev_id = ui.global::<AppData>().get_selected_revision_id().to_string();
                    load_obras_ui(&ui, pool, &rev_id);
                    update_global_stats(&ui, pool);
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_new_obra_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_obra_form_is_editing(false);
                app_data.set_obra_form_id("".into());
                app_data.set_obra_form_numero("".into());
                app_data.set_obra_form_numero_status("".into());
                app_data.set_obra_form_numero_msg("".into());
                app_data.set_obra_form_clase_val("PRIVADA".into());
                app_data.set_obra_form_tipo_obra("".into());
                app_data.set_obra_form_fecha_inicio("".into());
                app_data.set_obra_form_fecha_fin("".into());
                app_data.set_obra_form_ubicacion("".into());
                app_data.set_obra_form_superficie("".into());
                app_data.set_obra_form_monto("".into());
                app_data.set_obra_form_monto_preview("".into());
                app_data.set_obra_form_delegacion(app_data.get_selected_rp_delegacion());
                app_data.set_obra_form_subdelegacion(app_data.get_selected_rp_subdelegacion());
                app_data.set_show_obra_modal(true);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_obra_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_obra_modal(false);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_select_obra_for_edit(move |obra_id| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let rev_id = app_data.get_selected_revision_id().to_string();
                if let Some(ref pool) = pool_clone {
                    if let Ok(mut client) = pool.get() {
                        if let Ok(obras) = fetch_obras_by_revision(&mut client, &rev_id) {
                            if let Some(o) = obras.into_iter().find(|x| x.id == obra_id.as_str()) {
                                app_data.set_obra_form_is_editing(true);
                                app_data.set_obra_form_id(o.id.into());
                                app_data.set_obra_form_numero(o.numero.into());
                                let clean_clase = if o.clase.trim().is_empty() || o.clase.starts_with("--") {
                                    "-- Seleccionar Clase --".to_string()
                                } else {
                                    o.clase.trim().to_uppercase()
                                };
                                app_data.set_obra_form_clase_val(clean_clase.into());
                                app_data.set_obra_form_tipo_obra(o.tipo_obra.into());
                                app_data.set_obra_form_fecha_inicio(format_date_opt(o.fecha_inicio).into());
                                app_data.set_obra_form_fecha_fin(format_date_opt(o.fecha_fin).into());
                                app_data.set_obra_form_ubicacion(o.ubicacion.into());
                                app_data.set_obra_form_superficie(format!("{:.2}", o.superficie_construido).into());
                                let monto_fmt = match o.monto { Some(m) if m > 0.0 => format_monto_mexico(m), _ => "".to_string() };
                                let monto_preview = if monto_fmt.is_empty() { "".to_string() } else { format!("${} MXN", monto_fmt) };
                                app_data.set_obra_form_monto(monto_fmt.into());
                                app_data.set_obra_form_monto_preview(monto_preview.into());
                                app_data.set_obra_form_delegacion(o.delegacion.into());
                                app_data.set_obra_form_subdelegacion(o.subdelegacion.into());
                                app_data.set_show_obra_modal(true);
                                run_validate_obra_form(&ui);
                            }
                        }
                    }
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_submit_obra(move |id, numero, clase, tipo_obra, f_ini, f_fin, ubicacion, sup_str, monto_str, delegacion, subdelegacion| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let rev_id = app_data.get_selected_revision_id().to_string();
                let rp_id = app_data.get_selected_rp_id().to_string();

                let clean_numero = clean_alphanumeric_input(&numero);
                if clean_numero.is_empty() {
                    show_toast_message(&ui, "El número de registro de obra es obligatorio.");
                    return;
                }
                if let Err(e) = validate_registro_obra(&clean_numero) {
                    show_toast_message(&ui, &format!("Estructura de registro de obra inválida: {}", e));
                    return;
                }

                let fecha_inicio_opt = parse_date_opt(f_ini.as_str());
                if fecha_inicio_opt.is_none() {
                    show_toast_message(&ui, "La fecha de inicio de la obra es obligatoria (DD/MM/AAAA).");
                    return;
                }

                let fecha_fin_opt = parse_date_opt(f_fin.as_str());
                if fecha_fin_opt.is_none() {
                    show_toast_message(&ui, "La fecha final de la obra es obligatoria (DD/MM/AAAA).");
                    return;
                }

                if let (Some(ini), Some(fin)) = (fecha_inicio_opt, fecha_fin_opt) {
                    if ini > fin {
                        show_toast_message(&ui, "La fecha de inicio no puede ser posterior a la fecha final.");
                        return;
                    }
                }

                let sup: f64 = sup_str.trim().parse().unwrap_or(0.0);
                let monto: Option<f64> = {
                    let s = monto_str.replace(',', "");
                    let s = s.trim();
                    if s.is_empty() {
                        None
                    } else {
                        match s.parse::<f64>() {
                            Ok(v) => Some(v),
                            Err(_) => {
                                show_toast_message(&ui, "El monto de la obra no es un número válido.");
                                return;
                            }
                        }
                    }
                };

                if let Some(ref pool) = pool_clone {
                    let item = RegistroObraItem {
                        id: id.to_string(),
                        id_revision: rev_id.clone(),
                        id_registro_patronal: rp_id.clone(),
                        numero: numero.to_string(),
                        clase: if clase.starts_with("--") { "PRIVADA".to_string() } else { clase.to_string() },
                        tipo_obra: tipo_obra.to_string(),
                        fecha_inicio: fecha_inicio_opt,
                        fecha_fin: fecha_fin_opt,
                        ubicacion: ubicacion.to_string(),
                        superficie_construido: sup,
                        monto,
                        delegacion: delegacion.to_string(),
                        subdelegacion: subdelegacion.to_string(),
                        ..Default::default()
                    };

                    let is_edit = !id.is_empty();
                    show_toast_message(&ui, "Guardando registro de obra...");

                    let ui_weak2 = ui_weak.clone();
                    let pool_bg = pool.clone();
                    let rev_id_bg = rev_id.clone();
                    thread::spawn(move || {
                        let res = match pool_bg.get() {
                            Ok(mut client) => crate::db::obras_ops::guardar_obra_y_resincronizar(&mut client, &item, is_edit),
                            Err(e) => Err(format!("Error de conexión: {}", e)),
                        };

                        let pool_for_ui = pool_bg.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak2.upgrade() {
                                match res {
                                    Ok((_, cambio)) => {
                                        let app_data = ui.global::<AppData>();
                                        app_data.set_show_obra_modal(false);
                                        app_data.set_obra_form_numero_status("".into());
                                        app_data.set_obra_form_numero_msg("".into());
                                        load_obras_ui(&ui, &pool_for_ui, &rev_id_bg);
                                        update_global_stats(&ui, &pool_for_ui);
                                        let msg_base = if monto.map_or(false, |m| m > 0.0) {
                                            if is_edit { "Obra actualizada exitosamente." } else { "Registro de obra creado exitosamente." }
                                        } else {
                                            if is_edit {
                                                "Obra actualizada. Aviso: Sin monto capturado; recuerde capturarlo para poder calcular el límite del 18% en Reporte Liquidaciones."
                                            } else {
                                                "Registro de obra creado. Aviso: Sin monto capturado; recuerde capturarlo para poder calcular el límite del 18% en Reporte Liquidaciones."
                                            }
                                        };
                                        let extra = mensaje_cambio_universo(&cambio);
                                        let msg = if extra.is_empty() {
                                            msg_base.to_string()
                                        } else {
                                            format!("{} {}", msg_base, extra)
                                        };
                                        show_toast_message(&ui, &msg);
                                    }
                                    Err(e) => {
                                        show_toast_message(&ui, &format!("Error al guardar obra: {}", e));
                                    }
                                }
                            }
                        });
                    });
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_obra_form_monto_changed(move |text| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let formatted = format_monto_input_realtime(text.as_str());
                app_data.set_obra_form_monto(formatted.clone().into());
                let preview = if formatted.is_empty() {
                    "".to_string()
                } else {
                    let clean = formatted.replace(',', "");
                    let val: f64 = clean.trim().parse().unwrap_or(0.0);
                    format!("${} MXN", format_monto_mexico(val))
                };
                app_data.set_obra_form_monto_preview(preview.into());
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_obra_form_monto_accepted(move |text| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let final_str = format_monto_final_from_str(text.as_str());
                app_data.set_obra_form_monto(final_str.clone().into());
                if !final_str.is_empty() {
                    app_data.set_obra_form_monto_preview(format!("${} MXN", final_str).into());
                } else {
                    app_data.set_obra_form_monto_preview("".into());
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_open_delete_obra_dialog(move |id, numero| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_delete_obra_id(id.clone());
                app_data.set_delete_obra_numero(numero);
                app_data.set_delete_obra_impacto("Calculando impacto...".into());
                app_data.set_show_delete_obra_modal(true);

                if let Some(ref pool) = pool_clone {
                    let ui_weak2 = ui_weak.clone();
                    let pool_bg = pool.clone();
                    let obra_id_str = id.to_string();
                    thread::spawn(move || {
                        let res = match Uuid::parse_str(&obra_id_str) {
                            Err(_) => Err("ID de obra inválido".to_string()),
                            Ok(uuid) => match pool_bg.get() {
                                Ok(mut client) => crate::db::obras_ops::contar_impacto_obra(&mut client, uuid),
                                Err(e) => Err(format!("Error de conexión: {}", e)),
                            },
                        };

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak2.upgrade() {
                                let texto = match res {
                                    Ok(imp) if imp.empleado_mes == 0 => {
                                        "La obra no tiene empleados asignados en el reporte de liquidaciones.".to_string()
                                    }
                                    Ok(imp) => format!(
                                        "La obra está asignada a {} empleado(s) en {} empleado-mes ({} tramos). Al eliminarla, esos tramos quedan sin obra y se recalculan los importes. Puedes deshacerlo desde Asignación por empleado → Deshacer.",
                                        imp.empleados, imp.empleado_mes, imp.tramos
                                    ),
                                    Err(e) => format!("No se pudo calcular el impacto: {}", e),
                                };
                                ui.global::<AppData>().set_delete_obra_impacto(texto.into());
                            }
                        });
                    });
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_delete_obra_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_obra_modal(false);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_confirm_delete_obra(move |obra_id| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_obra_modal(false);
                show_toast_message(&ui, "Eliminando registro de obra...");

                let ui_weak2 = ui_weak.clone();
                let pool_opt = pool_clone.clone();
                let obra_id_str = obra_id.to_string();

                thread::spawn(move || {
                    if let Some(ref pool) = pool_opt {
                        let res = match Uuid::parse_str(obra_id_str.as_str()) {
                            Err(_) => Err("ID de obra inválido".to_string()),
                            Ok(uuid) => match pool.get() {
                                Ok(mut client) => crate::db::obras_ops::desactivar_obra(&mut client, uuid),
                                Err(e) => Err(format!("Error obteniendo conexión a la BD: {}", e)),
                            },
                        };

                        let pool_for_ui = pool.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak2.upgrade() {
                                match res {
                                    Ok(r) => {
                                        let app_data = ui.global::<AppData>();
                                        let rev_id = app_data.get_selected_revision_id().to_string();
                                        load_obras_ui(&ui, &pool_for_ui, &rev_id);
                                        update_global_stats(&ui, &pool_for_ui);
                                        let mut msg = "Registro de obra eliminado.".to_string();
                                        if r.hay_bitacora {
                                            msg.push_str(&format!(
                                                " Se quitaron sus asignaciones en {} empleado-mes; puedes deshacerlo desde Asignación por empleado → Deshacer.",
                                                r.reportes_afectados
                                            ));
                                        }
                                        show_toast_message(&ui, &msg);
                                    }
                                    Err(e) => {
                                        show_toast_message(&ui, &format!("Error al eliminar registro de obra: {}", e));
                                    }
                                }
                            }
                        });
                    }
                });
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_filter_obras_search(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let rev_id = ui.global::<AppData>().get_selected_revision_id().to_string();
                    load_obras_ui(&ui, pool, &rev_id);
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_filter_obras_tab(move |tab| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_obras_filter_tab(tab);
                if let Some(ref pool) = pool_clone {
                    let rev_id = ui.global::<AppData>().get_selected_revision_id().to_string();
                    load_obras_ui(&ui, pool, &rev_id);
                }
            }
        });
    }

    // 20. Recargar Clientes (con filtro y búsqueda)
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_reload_clientes(move || {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    load_clientes_ui(&ui, pool);
                    update_global_stats(&ui, pool);
                }
            }
        });
    }

    // 21. Modal: Abrir y Cerrar Nuevo Cliente
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_new_cliente_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_new_cliente_nombre("".into());
                app_data.set_new_cliente_rfc("".into());
                app_data.set_new_cliente_razon_social("".into());
                app_data.set_new_cliente_representante_legal("".into());
                app_data.set_new_cliente_curp("".into());
                app_data.set_new_cliente_tipo_persona_val("-- Seleccionar Tipo Persona --".into());
                app_data.set_new_cliente_regimen_val("-- Seleccione primero Tipo Persona --".into());
                app_data.set_new_cliente_regimen_enabled(false);
                let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Tipo Persona --".into()];
                app_data.set_new_cliente_regimenes_combo(ModelRc::new(VecModel::from(def_combo)));
                app_data.set_new_cliente_rfc_status("".into());
                app_data.set_new_cliente_rfc_msg("".into());
                app_data.set_new_cliente_curp_status("".into());
                app_data.set_new_cliente_curp_msg("".into());
                app_data.set_show_new_cliente_modal(true);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_new_cliente_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_show_new_cliente_modal(false);
                app_data.set_new_cliente_nombre("".into());
                app_data.set_new_cliente_rfc("".into());
                app_data.set_new_cliente_razon_social("".into());
                app_data.set_new_cliente_representante_legal("".into());
                app_data.set_new_cliente_curp("".into());
            }
        });
    }

    // Modal Nuevo Cliente: Cambio en Tipo de Persona
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_change_new_tipo_persona(move |tp_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let tp_upper = tp_str.trim().to_uppercase();
                if tp_upper.contains("FÍSICA") || tp_upper.contains("FISICA") || tp_upper.contains("MORAL") {
                    if let Some(ref pool) = pool_clone {
                        if let Ok(mut client) = pool.get() {
                            if let Ok(regimenes) = fetch_regimenes_by_tipo_persona(&mut client, &tp_upper) {
                                let mut r_combo = vec!["-- Seleccionar Régimen Fiscal --".to_string()];
                                for rf in regimenes {
                                    r_combo.push(format!("{} - {}", rf.clave, rf.nombre));
                                }
                                let r_models: Vec<SharedString> = r_combo.into_iter().map(|s| s.into()).collect();
                                app_data.set_new_cliente_regimenes_combo(ModelRc::new(VecModel::from(r_models)));
                                app_data.set_new_cliente_regimen_enabled(true);
                                app_data.set_new_cliente_regimen_val("-- Seleccionar Régimen Fiscal --".into());
                                run_validate_new_cliente(&ui);
                                return;
                            }
                        }
                    }
                }
                app_data.set_new_cliente_regimen_enabled(false);
                let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Tipo Persona --".into()];
                app_data.set_new_cliente_regimenes_combo(ModelRc::new(VecModel::from(def_combo)));
                app_data.set_new_cliente_regimen_val("-- Seleccione primero Tipo Persona --".into());
                run_validate_new_cliente(&ui);
            }
        });
    }

    // Validación en tiempo real: Nuevo Cliente
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_validate_new_cliente_input(move || {
            if let Some(ui) = ui_weak.upgrade() {
                run_validate_new_cliente(&ui);
            }
        });
    }

    // 22. Enviar Nuevo Cliente con todos los campos
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_submit_new_cliente(move |nombre, rfc, razon_social, curp, tp_val, reg_val, representante_legal| {
            if let Some(ui) = ui_weak.upgrade() {
                let tp_opt = if tp_val.to_uppercase().contains("MORAL") {
                    Some("MORAL")
                } else if tp_val.to_uppercase().contains("FISICA") || tp_val.to_uppercase().contains("FÍSICA") {
                    Some("FISICA")
                } else {
                    None
                };

                if let Err(e) = validate_cliente_data(nombre.as_str(), rfc.as_str(), curp.as_str(), tp_opt) {
                    show_toast_message(&ui, &format!("Validación fallida: {}", e));
                    return;
                }

                if let Some(ref pool) = pool_clone {
                    match pool.get() {
                        Ok(mut client) => {
                            let mut id_tp = String::new();
                            let tp_trim = tp_val.trim();
                            if !tp_trim.is_empty() && !tp_trim.starts_with("--") {
                                if let Ok(row) = client.query_opt("SELECT id FROM public.tipos_personas WHERE TRIM(tipo) ILIKE $1", &[&format!("%{}%", tp_trim)]) {
                                    if let Some(r) = row {
                                        let u: uuid::Uuid = r.get(0);
                                        id_tp = u.to_string();
                                    }
                                }
                            }

                            let mut id_rf = String::new();
                            let reg_trim = reg_val.trim();
                            if !reg_trim.is_empty() && !reg_trim.starts_with("--") {
                                let clave = reg_trim.split('-').next().unwrap_or("").trim();
                                if let Ok(row) = client.query_opt("SELECT id FROM public.regimenes_fiscales WHERE TRIM(clave) = $1", &[&clave]) {
                                    if let Some(r) = row {
                                        let u: uuid::Uuid = r.get(0);
                                        id_rf = u.to_string();
                                    }
                                }
                            }

                            match create_cliente(
                                &mut client,
                                nombre.as_str(),
                                rfc.as_str(),
                                razon_social.as_str(),
                                representante_legal.as_str(),
                                curp.as_str(),
                                &id_tp,
                                &id_rf,
                            ) {
                                Ok(new_cli) => {
                                    let app_data = ui.global::<AppData>();
                                    app_data.set_show_new_cliente_modal(false);
                                    app_data.set_new_cliente_nombre("".into());
                                    app_data.set_new_cliente_rfc("".into());
                                    app_data.set_new_cliente_razon_social("".into());
                                    app_data.set_new_cliente_representante_legal("".into());
                                    app_data.set_new_cliente_curp("".into());
                                    show_toast_message(&ui, &format!("Cliente '{}' registrado exitosamente.", new_cli.nombre));
                                    load_clientes_ui(&ui, pool);
                                    update_global_stats(&ui, pool);
                                }
                                Err(e) => {
                                    show_toast_message(&ui, &e);
                                }
                            }
                        }
                        Err(e) => {
                            show_toast_message(&ui, &format!("Error de conexión con la base de datos: {}", e));
                        }
                    }
                } else {
                    let diag = crate::db::diagnose_postgres();
                    crate::app_logger::log_db_simple_error(
                        "clientes",
                        "create_cliente",
                        &format!("No se puede guardar el cliente '{}': {}. {}", nombre, diag.status_text, diag.detail_text),
                    );
                    show_toast_message(&ui, &format!("No se puede guardar: {}. {}", diag.status_text, diag.detail_text));
                }
            }
        });
    }

    // 23. Modal: Abrir y Cerrar Editar Cliente
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_open_edit_cliente_dialog(move |client_id| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    if let Ok(mut client) = pool.get() {
                        if let Ok(clientes) = fetch_clientes(&mut client) {
                            if let Some(c) = clientes.into_iter().find(|x| x.id == client_id.as_str()) {
                                let app_data = ui.global::<AppData>();
                                app_data.set_edit_cliente_id(c.id.into());
                                app_data.set_edit_cliente_nombre(c.nombre.into());
                                app_data.set_edit_cliente_razon_social(c.razon_social.into());
                                app_data.set_edit_cliente_representante_legal(c.representante_legal.into());
                                app_data.set_edit_cliente_rfc(c.rfc.into());
                                app_data.set_edit_cliente_curp(c.curp.into());
                                app_data.set_edit_cliente_has_data(c.has_ingested_data);
                                if c.tipo_persona.is_empty() {
                                    app_data.set_edit_cliente_tipo_persona_val("-- Seleccionar Tipo Persona --".into());
                                } else {
                                    app_data.set_edit_cliente_tipo_persona_val(c.tipo_persona.clone().into());
                                }

                                if !c.tipo_persona.is_empty() {
                                    if let Ok(regimenes) = fetch_regimenes_by_tipo_persona(&mut client, &c.tipo_persona) {
                                        let mut r_combo = vec!["-- Seleccionar Régimen Fiscal --".to_string()];
                                        let mut selected_reg_val = String::new();
                                        for rf in regimenes {
                                            let s = format!("{} - {}", rf.clave, rf.nombre);
                                            if rf.id_regimen_fiscal == c.id_regimen_fiscal || (!c.regimen_fiscal_clave.is_empty() && rf.clave == c.regimen_fiscal_clave) {
                                                selected_reg_val = s.clone();
                                            }
                                            r_combo.push(s);
                                        }
                                        let r_models: Vec<SharedString> = r_combo.into_iter().map(|s| s.into()).collect();
                                        app_data.set_regimenes_fiscales_combo(ModelRc::new(VecModel::from(r_models)));
                                        app_data.set_edit_cliente_regimen_enabled(true);
                                        if selected_reg_val.is_empty() {
                                            selected_reg_val = "-- Seleccionar Régimen Fiscal --".to_string();
                                        }
                                        app_data.set_edit_cliente_regimen_val(selected_reg_val.into());
                                    }
                                } else {
                                    app_data.set_edit_cliente_regimen_enabled(false);
                                    let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Tipo Persona --".into()];
                                    app_data.set_regimenes_fiscales_combo(ModelRc::new(VecModel::from(def_combo)));
                                    app_data.set_edit_cliente_regimen_val("-- Seleccione primero Tipo Persona --".into());
                                }

                                app_data.set_edit_cliente_rfc_status("".into());
                                app_data.set_edit_cliente_rfc_msg("".into());
                                app_data.set_edit_cliente_curp_status("".into());
                                app_data.set_edit_cliente_curp_msg("".into());
                                app_data.set_show_edit_cliente_modal(true);
                                run_validate_edit_cliente(&ui);
                            }
                        }
                    }
                } else {
                    let diag = crate::db::diagnose_postgres();
                    show_toast_message(&ui, &format!("No se puede editar cliente: {}. {}", diag.status_text, diag.detail_text));
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_edit_cliente_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_show_edit_cliente_modal(false);
            }
        });
    }

    // 24. Cambio en Combo de Tipo de Persona (Filtra dinámicamente regímenes fiscales correspondientes)
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_change_tipo_persona(move |tp_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let tp_upper = tp_str.trim().to_uppercase();
                if tp_upper.contains("FÍSICA") || tp_upper.contains("FISICA") || tp_upper.contains("MORAL") {
                    if let Some(ref pool) = pool_clone {
                        if let Ok(mut client) = pool.get() {
                            if let Ok(regimenes) = fetch_regimenes_by_tipo_persona(&mut client, &tp_upper) {
                                let mut r_combo = vec!["-- Seleccionar Régimen Fiscal --".to_string()];
                                for rf in regimenes {
                                    r_combo.push(format!("{} - {}", rf.clave, rf.nombre));
                                }
                                let r_models: Vec<SharedString> = r_combo.into_iter().map(|s| s.into()).collect();
                                app_data.set_regimenes_fiscales_combo(ModelRc::new(VecModel::from(r_models)));
                                app_data.set_edit_cliente_regimen_enabled(true);
                                app_data.set_edit_cliente_regimen_val("-- Seleccionar Régimen Fiscal --".into());
                                run_validate_edit_cliente(&ui);
                                return;
                            }
                        }
                    }
                }
                app_data.set_edit_cliente_regimen_enabled(false);
                let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Tipo Persona --".into()];
                app_data.set_regimenes_fiscales_combo(ModelRc::new(VecModel::from(def_combo)));
                app_data.set_edit_cliente_regimen_val("-- Seleccione primero Tipo Persona --".into());
                run_validate_edit_cliente(&ui);
            }
        });
    }

    // Validación en tiempo real: Editar Cliente
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_validate_edit_cliente_input(move || {
            if let Some(ui) = ui_weak.upgrade() {
                run_validate_edit_cliente(&ui);
            }
        });
    }

    // 25. Enviar Actualización de Cliente con todos los campos (incluyendo Nombre)
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_submit_edit_cliente(move |id, nombre, razon_social, rfc, curp, tp_val, reg_val, representante_legal| {
            if let Some(ui) = ui_weak.upgrade() {
                let tp_opt = if tp_val.to_uppercase().contains("MORAL") {
                    Some("MORAL")
                } else if tp_val.to_uppercase().contains("FISICA") || tp_val.to_uppercase().contains("FÍSICA") {
                    Some("FISICA")
                } else {
                    None
                };

                if let Err(e) = validate_cliente_data(nombre.as_str(), rfc.as_str(), curp.as_str(), tp_opt) {
                    show_toast_message(&ui, &format!("Validación fallida: {}", e));
                    return;
                }

                if let Some(ref pool) = pool_clone {
                    match pool.get() {
                        Ok(mut client) => {
                            let mut id_tp = String::new();
                            let tp_trim = tp_val.trim();
                            if !tp_trim.is_empty() && !tp_trim.starts_with("--") {
                                if let Ok(row) = client.query_opt("SELECT id FROM public.tipos_personas WHERE TRIM(tipo) ILIKE $1", &[&format!("%{}%", tp_trim)]) {
                                    if let Some(r) = row {
                                        let u: uuid::Uuid = r.get(0);
                                        id_tp = u.to_string();
                                    }
                                }
                            }

                            let mut id_rf = String::new();
                            let reg_trim = reg_val.trim();
                            if !reg_trim.is_empty() && !reg_trim.starts_with("--") {
                                let clave = reg_trim.split('-').next().unwrap_or("").trim();
                                if let Ok(row) = client.query_opt("SELECT id FROM public.regimenes_fiscales WHERE TRIM(clave) = $1", &[&clave]) {
                                    if let Some(r) = row {
                                        let u: uuid::Uuid = r.get(0);
                                        id_rf = u.to_string();
                                    }
                                }
                            }

                            match update_cliente(
                                &mut client,
                                id.as_str(),
                                nombre.as_str(),
                                razon_social.as_str(),
                                representante_legal.as_str(),
                                rfc.as_str(),
                                curp.as_str(),
                                &id_tp,
                                &id_rf,
                            ) {
                                Ok(()) => {
                                    let app_data = ui.global::<AppData>();
                                    app_data.set_show_edit_cliente_modal(false);
                                    if app_data.get_selected_cliente_id().as_str() == id.as_str() {
                                        app_data.set_selected_cliente_nombre(nombre.clone());
                                        app_data.set_selected_cliente_rfc(rfc.clone());
                                    }
                                    show_toast_message(&ui, "Cliente actualizado exitosamente.");
                                    load_clientes_ui(&ui, pool);
                                    update_global_stats(&ui, pool);
                                }
                                Err(e) => {
                                    show_toast_message(&ui, &e);
                                }
                            }
                        }
                        Err(e) => {
                            show_toast_message(&ui, &format!("Error de conexión: {}", e));
                        }
                    }
                } else {
                    let diag = crate::db::diagnose_postgres();
                    crate::app_logger::log_db_simple_error(
                        "clientes",
                        "update_cliente",
                        &format!("No se puede actualizar el cliente '{}': {}. {}", nombre, diag.status_text, diag.detail_text),
                    );
                    show_toast_message(&ui, &format!("No se puede guardar: {}. {}", diag.status_text, diag.detail_text));
                }
            }
        });
    }

    // 26. Filtro Tabs de Clientes (Todos / Con Cédulas / Nuevos)
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_filter_clientes_tab(move |tab| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_clientes_filter_tab(tab);
                if let Some(ref pool) = pool_clone {
                    load_clientes_ui(&ui, pool);
                }
            }
        });
    }

    // 27. Contador y Límite de 300 caracteres en tiempo real para Actividad Económica RP
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_update_rp_actividad(move |text| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let text_str = text.as_str();
                let count = count_chars(text_str);
                if count > 300 {
                    let (_, clamped) = limit_to_chars(text_str, 300);
                    app_data.set_rp_form_actividad(clamped.into());
                    app_data.set_rp_form_char_count(300);
                } else {
                    app_data.set_rp_form_char_count(count as i32);
                }
            }
        });
    }

    // 28. Cambio en Combo de Clase RP (Filtra dinámicamente fracciones de la clase)
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_change_rp_clase(move |clase_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let roman = extract_clase_roman(clase_str.as_str());

                if !roman.is_empty() {
                    if let Some(ref pool) = pool_clone {
                        if let Ok(mut client) = pool.get() {
                            if let Ok(row) = client.query_opt("SELECT id FROM public.clases WHERE TRIM(nombre) = $1", &[&roman]) {
                                if let Some(r) = row {
                                    let clase_uuid: uuid::Uuid = r.get(0);
                                    if let Ok(fracciones) = fetch_fracciones_by_clase(&mut client, &clase_uuid.to_string()) {
                                        let mut f_combo = vec!["-- Seleccionar Fracción --".to_string()];
                                        for f in fracciones {
                                            f_combo.push(format!("{} - {}", f.codigo, f.actividad));
                                        }
                                        let f_models: Vec<SharedString> = f_combo.into_iter().map(|s| s.into()).collect();
                                        app_data.set_fracciones_combo(ModelRc::new(VecModel::from(f_models)));
                                        app_data.set_rp_form_fraccion_val("-- Seleccionar Fracción --".into());
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
                let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Clase --".into()];
                app_data.set_fracciones_combo(ModelRc::new(VecModel::from(def_combo)));
                app_data.set_rp_form_fraccion_val("-- Seleccione primero Clase --".into());
            }
        });
    }

    // 29. Limpiar Formulario de Registro Patronal
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_clear_rp_form(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_rp_form_id("".into());
                app_data.set_rp_form_is_editing(false);
                app_data.set_rp_form_registro("".into());
                app_data.set_rp_form_actividad("".into());
                app_data.set_rp_form_char_count(0);
                app_data.set_rp_form_domicilio("".into());
                app_data.set_rp_form_codigo_postal("".into());
                app_data.set_rp_form_entidad("".into());
                app_data.set_rp_form_area_geografica("".into());
                app_data.set_rp_form_delegacion("".into());
                app_data.set_rp_form_subdelegacion("".into());
                app_data.set_rp_form_prima_riesgo("".into());
                app_data.set_rp_form_clase_val("-- Seleccionar Clase --".into());
                let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Clase --".into()];
                app_data.set_fracciones_combo(ModelRc::new(VecModel::from(def_combo)));
                app_data.set_rp_form_fraccion_val("-- Seleccione primero Clase --".into());
                app_data.set_rp_form_registro_status("".into());
                app_data.set_rp_form_registro_msg("".into());
            }
        });
    }

    // 29b. Abrir y Cerrar Diálogo de Nuevo/Edición Registro Patronal
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_new_rp_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_rp_form_id("".into());
                app_data.set_rp_form_is_editing(false);
                app_data.set_rp_form_registro("".into());
                app_data.set_rp_form_registro_status("".into());
                app_data.set_rp_form_registro_msg("".into());
                app_data.set_rp_form_actividad("".into());
                app_data.set_rp_form_char_count(0);
                app_data.set_rp_form_domicilio("".into());
                app_data.set_rp_form_codigo_postal("".into());
                app_data.set_rp_form_entidad("".into());
                app_data.set_rp_form_area_geografica("".into());
                app_data.set_rp_form_delegacion("".into());
                app_data.set_rp_form_subdelegacion("".into());
                app_data.set_rp_form_prima_riesgo("".into());
                app_data.set_rp_form_clase_val("-- Seleccionar Clase --".into());
                let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Clase --".into()];
                app_data.set_fracciones_combo(ModelRc::new(VecModel::from(def_combo)));
                app_data.set_rp_form_fraccion_val("-- Seleccione primero Clase --".into());
                app_data.set_rp_form_complementary_enabled(false);
                app_data.set_show_rp_modal(true);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_rp_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_show_rp_modal(false);
            }
        });
    }

    // 30. Guardar / Actualizar Registro Patronal
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_submit_registro_patronal(move |id, id_cliente, reg, act, dom, cp, ent, area, del, subdel, prima_str, clase_str, fracc_str| {
            if let Some(ui) = ui_weak.upgrade() {
                let clean_reg = clean_alphanumeric_input(reg.as_str());
                if let Err(e) = validate_registro_patronal(&clean_reg) {
                    show_toast_message(&ui, &format!("No se puede guardar: {}", e));
                    return;
                }

                if let Some(ref pool) = pool_clone {
                    match pool.get() {
                        Ok(mut client) => {
                            let prima_opt = prima_str.trim().parse::<f64>().ok();

                            let mut id_clase_opt: Option<String> = None;
                            let roman = extract_clase_roman(clase_str.as_str());
                            if !roman.is_empty() {
                                if let Ok(row) = client.query_opt("SELECT id FROM public.clases WHERE TRIM(nombre) = $1", &[&roman]) {
                                    if let Some(r) = row {
                                        let u: uuid::Uuid = r.get(0);
                                        id_clase_opt = Some(u.to_string());
                                    }
                                }
                            }

                            let mut id_fracc_opt: Option<String> = None;
                            let f_trim = fracc_str.trim();
                            if !f_trim.is_empty() && !f_trim.starts_with("--") {
                                let cod = f_trim.split('-').next().unwrap_or("").trim();
                                if let Ok(row) = client.query_opt("SELECT id FROM public.fracciones WHERE TRIM(codigo) = $1", &[&cod]) {
                                    if let Some(r) = row {
                                        let u: uuid::Uuid = r.get(0);
                                        id_fracc_opt = Some(u.to_string());
                                    }
                                }
                            }

                            if id.trim().is_empty() {
                                match create_registro_patronal(
                                    &mut client,
                                    id_cliente.as_str(),
                                    reg.as_str(),
                                    act.as_str(),
                                    dom.as_str(),
                                    cp.as_str(),
                                    ent.as_str(),
                                    area.as_str(),
                                    del.as_str(),
                                    subdel.as_str(),
                                    prima_opt,
                                    id_clase_opt.as_deref(),
                                    id_fracc_opt.as_deref(),
                                ) {
                                    Ok(_) => {
                                        show_toast_message(&ui, &format!("Registro Patronal '{}' registrado con éxito.", reg));
                                        let app_data = ui.global::<AppData>();
                                        app_data.set_rp_form_id("".into());
                                        app_data.set_rp_form_is_editing(false);
                                        app_data.set_rp_form_registro("".into());
                                        app_data.set_rp_form_actividad("".into());
                                        app_data.set_rp_form_char_count(0);
                                        app_data.set_rp_form_domicilio("".into());
                                        app_data.set_rp_form_codigo_postal("".into());
                                        app_data.set_rp_form_entidad("".into());
                                        app_data.set_rp_form_area_geografica("".into());
                                        app_data.set_rp_form_delegacion("".into());
                                        app_data.set_rp_form_subdelegacion("".into());
                                        app_data.set_rp_form_prima_riesgo("".into());
                                        app_data.set_rp_form_clase_val("-- Seleccionar Clase --".into());
                                        let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Clase --".into()];
                                        app_data.set_fracciones_combo(ModelRc::new(VecModel::from(def_combo)));
                                        app_data.set_rp_form_fraccion_val("-- Seleccione primero Clase --".into());
                                        app_data.set_rp_form_registro_status("".into());
                                        app_data.set_rp_form_registro_msg("".into());
                                        app_data.set_show_rp_modal(false);
                                        let client_nom = app_data.get_selected_cliente_nombre().to_string();
                                        let rp_clean = reg.clone();
                                        thread::spawn(move || {
                                            let _ = crate::rustfs::ensure_revision_structure(&client_nom, &rp_clean, "GENERAL");
                                        });
                                        load_registros_patronales_ui(&ui, pool, id_cliente.as_str());
                                        update_global_stats(&ui, pool);
                                    }
                                    Err(e) => {
                                        show_toast_message(&ui, &e);
                                    }
                                }
                            } else {
                                match update_registro_patronal(
                                    &mut client,
                                    id.as_str(),
                                    reg.as_str(),
                                    act.as_str(),
                                    dom.as_str(),
                                    cp.as_str(),
                                    ent.as_str(),
                                    area.as_str(),
                                    del.as_str(),
                                    subdel.as_str(),
                                    prima_opt,
                                    id_clase_opt.as_deref(),
                                    id_fracc_opt.as_deref(),
                                ) {
                                    Ok(()) => {
                                        show_toast_message(&ui, "Registro Patronal actualizado correctamente.");
                                        let app_data = ui.global::<AppData>();
                                        app_data.set_rp_form_id("".into());
                                        app_data.set_rp_form_is_editing(false);
                                        app_data.set_rp_form_registro("".into());
                                        app_data.set_rp_form_registro_status("".into());
                                        app_data.set_rp_form_registro_msg("".into());
                                        app_data.set_rp_form_actividad("".into());
                                        app_data.set_rp_form_char_count(0);
                                        app_data.set_rp_form_domicilio("".into());
                                        app_data.set_rp_form_codigo_postal("".into());
                                        app_data.set_rp_form_entidad("".into());
                                        app_data.set_rp_form_area_geografica("".into());
                                        app_data.set_rp_form_delegacion("".into());
                                        app_data.set_rp_form_subdelegacion("".into());
                                        app_data.set_rp_form_prima_riesgo("".into());
                                        app_data.set_rp_form_clase_val("-- Seleccionar Clase --".into());
                                        let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Clase --".into()];
                                        app_data.set_fracciones_combo(ModelRc::new(VecModel::from(def_combo)));
                                        app_data.set_rp_form_fraccion_val("-- Seleccione primero Clase --".into());
                                        app_data.set_show_rp_modal(false);
                                        load_registros_patronales_ui(&ui, pool, id_cliente.as_str());
                                        update_global_stats(&ui, pool);
                                    }
                                    Err(e) => {
                                        show_toast_message(&ui, &e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            show_toast_message(&ui, &format!("Error de conexión: {}", e));
                        }
                    }
                } else {
                    let diag = crate::db::diagnose_postgres();
                    crate::app_logger::log_db_simple_error(
                        "registros_patronales",
                        "create_rp",
                        &format!("No se puede guardar el registro patronal '{}': {}. {}", reg, diag.status_text, diag.detail_text),
                    );
                    show_toast_message(&ui, &format!("No se puede guardar: {}. {}", diag.status_text, diag.detail_text));
                }
            }
        });
    }

    // 31. Seleccionar Registro Patronal para Edición
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_select_rp_for_edit(move |rp_id| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    if let Ok(mut client) = pool.get() {
                        let app_data = ui.global::<AppData>();
                        let client_id = app_data.get_selected_cliente_id().to_string();
                        if let Ok(rps) = fetch_registros_patronales_by_cliente(&mut client, &client_id) {
                            if let Some(rp) = rps.into_iter().find(|x| x.id == rp_id.as_str()) {
                                app_data.set_rp_form_id(rp.id.into());
                                app_data.set_rp_form_is_editing(true);
                                app_data.set_rp_form_registro(rp.registro.into());
                                app_data.set_rp_form_actividad(rp.actividad.clone().into());
                                app_data.set_rp_form_char_count(count_chars(&rp.actividad) as i32);
                                app_data.set_rp_form_domicilio(rp.domicilio.into());
                                app_data.set_rp_form_codigo_postal(rp.codigo_postal.into());
                                app_data.set_rp_form_entidad(rp.entidad.into());
                                app_data.set_rp_form_area_geografica(rp.area_geografica.into());
                                app_data.set_rp_form_delegacion(rp.delegacion.into());
                                app_data.set_rp_form_subdelegacion(rp.subdelegacion.into());
                                app_data.set_rp_form_prima_riesgo(format!("{:.5}", rp.prima_riesgo).into());
                                app_data.set_rp_form_complementary_enabled(rp.has_ingested_data);
                                run_validate_rp_form(&ui);
                                app_data.set_show_rp_modal(true);

                                if !rp.clase_nombre.is_empty() {
                                    let clean_target = rp.clase_nombre.trim();
                                    let combo = app_data.get_clases_combo();
                                    let mut found_clase_label = String::new();
                                    for i in 0..combo.row_count() {
                                        if let Some(item) = combo.row_data(i) {
                                            let item_str = item.to_string();
                                            if extract_clase_roman(&item_str) == clean_target {
                                                found_clase_label = item_str;
                                                break;
                                            }
                                        }
                                    }
                                    if !found_clase_label.is_empty() {
                                        app_data.set_rp_form_clase_val(found_clase_label.into());
                                    } else {
                                        app_data.set_rp_form_clase_val("-- Seleccionar Clase --".into());
                                    }

                                    if !rp.id_clase.is_empty() {
                                        if let Ok(fracciones) = fetch_fracciones_by_clase(&mut client, &rp.id_clase) {
                                            let mut f_combo = vec!["-- Seleccionar Fracción --".to_string()];
                                            let mut found_fracc_str = String::new();
                                            let target_cod = rp.fraccion_codigo.trim();

                                            for f in fracciones {
                                                let label = format!("{} - {}", f.codigo, f.actividad);
                                                if !target_cod.is_empty() && f.codigo.trim() == target_cod {
                                                    found_fracc_str = label.clone();
                                                }
                                                f_combo.push(label);
                                            }
                                            let f_models: Vec<SharedString> = f_combo.into_iter().map(|s| s.into()).collect();
                                            app_data.set_fracciones_combo(ModelRc::new(VecModel::from(f_models)));
                                            if !found_fracc_str.is_empty() {
                                                app_data.set_rp_form_fraccion_val(found_fracc_str.into());
                                            } else {
                                                app_data.set_rp_form_fraccion_val("-- Seleccionar Fracción --".into());
                                            }
                                        }
                                    }
                                } else {
                                    app_data.set_rp_form_clase_val("-- Seleccionar Clase --".into());
                                    let def_combo: Vec<SharedString> = vec!["-- Seleccione primero Clase --".into()];
                                    app_data.set_fracciones_combo(ModelRc::new(VecModel::from(def_combo)));
                                    app_data.set_rp_form_fraccion_val("-- Seleccione primero Clase --".into());
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    // 32. Filtrar Registros Patronales (Búsqueda)
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_filter_rp_search(move || {
            if let Some(ui) = ui_weak.upgrade()
                && let Some(ref pool) = pool_clone {
                    let client_id = ui.global::<AppData>().get_selected_cliente_id().to_string();
                    load_registros_patronales_ui(&ui, pool, &client_id);
            }
        });
    }

    // 33. Modal: Abrir y Cerrar Edición de Empleado
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_new_empleado_dialog(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_emp_edit_is_new(true);
                app_data.set_emp_edit_id("".into());
                app_data.set_emp_edit_nombre("".into());
                app_data.set_emp_edit_rfc("".into());
                app_data.set_emp_edit_curp("".into());
                app_data.set_emp_edit_nss("".into());
                app_data.set_emp_edit_esta_activo(true);
                app_data.set_emp_edit_sbc("$0.00".into());
                app_data.set_emp_edit_rp("".into());
                app_data.set_emp_edit_rfc_status("".into());
                app_data.set_emp_edit_rfc_msg("".into());
                app_data.set_emp_edit_curp_status("".into());
                app_data.set_emp_edit_curp_msg("".into());
                app_data.set_emp_edit_nss_status("".into());
                app_data.set_emp_edit_nss_msg("".into());
                app_data.set_emp_edit_cross_warning("".into());
                app_data.set_show_edit_empleado_panel(true);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_edit_empleado_panel(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_show_edit_empleado_panel(false);
            }
        });
    }

    // 34. Seleccionar Empleado para Edición
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_select_empleado_for_edit(move |emp_id| {
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    if let Ok(mut client) = pool.get() {
                        let app_data = ui.global::<AppData>();
                        let client_id = app_data.get_selected_cliente_id().to_string();
                        if let Ok(emps) = fetch_empleados_by_cliente(&mut client, &client_id) {
                            if let Some(emp) = emps.into_iter().find(|x| x.id == emp_id.as_str()) {
                                app_data.set_emp_edit_is_new(false);
                                app_data.set_emp_edit_id(emp.id.into());
                                app_data.set_emp_edit_nombre(emp.nombre.into());
                                app_data.set_emp_edit_rfc(emp.rfc.clone().into());
                                app_data.set_emp_edit_curp(emp.curp.clone().into());
                                app_data.set_emp_edit_nss(emp.nss.clone().into());
                                app_data.set_emp_edit_esta_activo(emp.esta_activo);
                                let sbc_str = if emp.sbc_diario > 0.0 {
                                    format!("${:.2}", emp.sbc_diario)
                                } else {
                                    "$0.00".to_string()
                                };
                                app_data.set_emp_edit_sbc(sbc_str.into());
                                app_data.set_emp_edit_rp(emp.registro_patronal.into());
                                app_data.set_emp_edit_rfc_status("".into());
                                app_data.set_emp_edit_rfc_msg("".into());
                                app_data.set_emp_edit_curp_status("".into());
                                app_data.set_emp_edit_curp_msg("".into());
                                app_data.set_emp_edit_nss_status("".into());
                                app_data.set_emp_edit_nss_msg("".into());
                                app_data.set_emp_edit_cross_warning("".into());
                                app_data.set_show_edit_empleado_panel(true);
                                run_validate_empleado(&ui);
                            }
                        }
                    }
                }
            }
        });
    }

    // Validación en tiempo real: Empleado
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_validate_empleado_input(move || {
            if let Some(ui) = ui_weak.upgrade() {
                run_validate_empleado(&ui);
            }
        });
    }

    // Validación en tiempo real: Registro Patronal
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_validate_rp_input(move || {
            if let Some(ui) = ui_weak.upgrade() {
                run_validate_rp_form(&ui);
            }
        });
    }

    // Validación en tiempo real: Registro de Obra
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_validate_obra_input(move || {
            if let Some(ui) = ui_weak.upgrade() {
                run_validate_obra_form(&ui);
            }
        });
    }

    // 35. Guardar / Actualizar Empleado
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_submit_edit_empleado(move |id, id_cliente, nombre, rfc, curp, nss, esta_activo| {
            if let Some(ui) = ui_weak.upgrade() {
                let client_rfc = ui.global::<AppData>().get_selected_cliente_rfc().to_string();
                if let Err(e) = validate_empleado_data(
                    nombre.as_str(),
                    rfc.as_str(),
                    curp.as_str(),
                    nss.as_str(),
                    if client_rfc.is_empty() { None } else { Some(&client_rfc) },
                ) {
                    show_toast_message(&ui, &format!("Validación fallida: {}", e));
                    return;
                }

                if let Some(ref pool) = pool_clone {
                    match pool.get() {
                        Ok(mut client) => {
                            let is_new = ui.global::<AppData>().get_emp_edit_is_new();
                            if is_new {
                                match create_empleado(&mut client, id_cliente.as_str(), nombre.as_str(), rfc.as_str(), curp.as_str(), nss.as_str(), esta_activo) {
                                    Ok(_) => {
                                        show_toast_message(&ui, &format!("Empleado '{}' registrado exitosamente.", nombre));
                                        let app_data = ui.global::<AppData>();
                                        app_data.set_show_edit_empleado_panel(false);
                                        load_empleados_ui(&ui, pool, id_cliente.as_str());
                                        update_global_stats(&ui, pool);
                                    }
                                    Err(e) => {
                                        show_toast_message(&ui, &e);
                                    }
                                }
                            } else {
                                match update_empleado(&mut client, id.as_str(), nombre.as_str(), rfc.as_str(), curp.as_str(), nss.as_str(), esta_activo) {
                                    Ok(()) => {
                                        show_toast_message(&ui, "Información del empleado actualizada correctamente.");
                                        let app_data = ui.global::<AppData>();
                                        app_data.set_show_edit_empleado_panel(false);
                                        load_empleados_ui(&ui, pool, id_cliente.as_str());
                                        update_global_stats(&ui, pool);
                                    }
                                    Err(e) => {
                                        show_toast_message(&ui, &e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            show_toast_message(&ui, &format!("Error conectando a la base de datos: {}", e));
                        }
                    }
                }
            }
        });
    }

    // D-8: filtro por estado laboral / obras actuales (0 Todos, 1 Bajas, 2 Sin obra, 3 Con 2+ obras)
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_empleados_cambiar_filtro(move |filtro| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_empleados_filtro(filtro);
                if let Some(ref pool) = pool_clone {
                    let client_id = ui.global::<AppData>().get_selected_cliente_id().to_string();
                    load_empleados_ui(&ui, pool, &client_id);
                }
            }
        });
    }

    // 36. Toggle Filtro Solo Empleados Activos
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_toggle_empleados_filter_active(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let current = app_data.get_empleados_filter_active_only();
                app_data.set_empleados_filter_active_only(!current);
                if let Some(ref pool) = pool_clone {
                    let client_id = app_data.get_selected_cliente_id().to_string();
                    load_empleados_ui(&ui, pool, &client_id);
                }
            }
        });
    }

    // 37. Filtrar Empleados (Búsqueda)
    {
        let ui_weak = main_window.as_weak();
        let pool_clone = db_pool.clone();
        app_logic.on_filter_empleados_search(move || {
            if let Some(ui) = ui_weak.upgrade()
                && let Some(ref pool) = pool_clone {
                    let client_id = ui.global::<AppData>().get_selected_cliente_id().to_string();
                    load_empleados_ui(&ui, pool, &client_id);
            }
        });
    }

    // 38. Seleccionar archivos Cédulas View (PDF, ZIP, RAR)
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_select_cedulas_view_files(move || {
            let files = rfd::FileDialog::new()
                .set_title("Seleccionar Cédulas SUA (PDF, ZIP o RAR)")
                .add_filter("Cédulas y Paquetes", &["pdf", "zip", "rar"])
                .add_filter("Cédulas SUA (PDF)", &["pdf"])
                .add_filter("Comprimidos (ZIP/RAR)", &["zip", "rar"])
                .pick_files();

            if let Some(paths) = files {
                handle_cedulas_upload_and_inspect(
                    ui_weak.clone(),
                    session_clone.clone(),
                    pool_clone.clone(),
                    paths,
                );
            }
        });
    }

    // 39. Seleccionar carpeta Cédulas View (Búsqueda recursiva con preservación de estructura en RustFS)
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_select_cedulas_view_folder(move || {
            let folder = rfd::FileDialog::new()
                .set_title("Seleccionar Carpeta con Cédulas SUA (Búsqueda Recursiva)")
                .pick_folder();

            if let Some(folder_path) = folder {
                handle_cedulas_upload_and_inspect(
                    ui_weak.clone(),
                    session_clone.clone(),
                    pool_clone.clone(),
                    vec![folder_path],
                );
            }
        });
    }

    // 40. Remover archivo individual de la lista de cédulas cargadas y moverlo a Trash en RustFS
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_remove_cedula_view_file(move |file_id| {
            let id_str = file_id.as_str();
            let mut sess = session_clone.lock().unwrap();
            let target_file = sess.files.iter().find(|f| f.id == id_str).cloned();
            sess.files.retain(|f| f.id != id_str);

            let (cliente_nombre, rp_registro, revision_nombre) = if let Some(ui) = ui_weak.upgrade() {
                update_cedulas_view_file_list_ui(&ui, &sess);
                if sess.files.is_empty() && sess.audit_res.is_none() {
                    ui.global::<AppData>().set_cedulas_view_state(0);
                }
                let app_data = ui.global::<AppData>();
                (
                    app_data.get_selected_cliente_nombre().to_string(),
                    app_data.get_selected_rp_registro().to_string(),
                    app_data.get_selected_revision_nombre().to_string(),
                )
            } else {
                ("".to_string(), "".to_string(), "".to_string())
            };

            let rev_id_opt = sess.current_revision_id.clone();
            drop(sess);

            if let Some(file_info) = target_file {
                // Eliminar archivo local de caché si existe
                if file_info.path.exists() {
                    let _ = std::fs::remove_file(&file_info.path);
                }

                let pool_for_thread = pool_clone.clone();
                let ui_weak_for_thread = ui_weak.clone();
                let filename_clone = file_info.filename.clone();

                thread::spawn(move || {
                    let mut c_nom = cliente_nombre;
                    let mut r_reg = rp_registro;
                    let mut rev_nom = revision_nombre;

                    let pool_to_use = pool_for_thread.or_else(crate::db::get_global_pool);
                    if let (Some(rev_id), Some(pool)) = (rev_id_opt, pool_to_use) {
                        if let Ok(mut client) = pool.get() {
                            if let Ok(rev_uuid) = uuid::Uuid::parse_str(&rev_id) {
                                if let Ok(Some(row)) = client.query_opt(
                                    "SELECT c.nombre, rp.registro, r.nombre
                                     FROM revisiones r
                                     JOIN registros_patronales rp ON r.id_registro_patronal = rp.id
                                     JOIN clientes c ON rp.id_cliente = c.id
                                     WHERE r.id = $1",
                                    &[&rev_uuid],
                                ) {
                                    c_nom = row.get(0);
                                    r_reg = row.get(1);
                                    rev_nom = row.get(2);
                                }
                            }
                        }
                    }

                    if !c_nom.is_empty() && !r_reg.is_empty() && !rev_nom.is_empty() {
                        match crate::rustfs::move_file_to_trash(
                            &c_nom,
                            &r_reg,
                            &rev_nom,
                            "Cedulas_de_cuotas",
                            &filename_clone,
                        ) {
                            Ok(trash_key) => {
                                log::info!("Archivo movido a Trash en RustFS: {}", trash_key);
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = ui_weak_for_thread.upgrade() {
                                        show_toast_message(
                                            &ui,
                                            &format!("Archivo '{}' movido a la papelera (Trash).", filename_clone),
                                        );
                                    }
                                });
                            }
                            Err(e) => {
                                log::warn!("No se pudo mover archivo {} a Trash en RustFS: {}", filename_clone, e);
                            }
                        }
                    }
                });
            }
        });
    }

    // 41. Procesar y Guardar Cédulas en Base de Datos
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_procesar_y_guardar_cedulas(move || {
            let pool = crate::db::get_global_pool().or_else(|| pool_clone.clone());
            spawn_cedulas_view_worker(ui_weak.clone(), session_clone.clone(), pool);
        });
    }

    // 42. Descargar a Excel Multi-Sheet
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        app_logic.on_descargar_cedulas_excel(move || {
            if let Some(ui) = ui_weak.upgrade() {
                handle_descargar_cedulas_excel(&ui, &session_clone);
            }
        });
    }

    // 43. Solicitar borrar todas las cédulas (abre modal de confirmación)
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_solicitar_borrar_todas_cedulas(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_all_cedulas_modal(true);
            }
        });
    }

    // 44. Cancelar borrar todas las cédulas
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_cancelar_borrar_todas_cedulas(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_delete_all_cedulas_modal(false);
            }
        });
    }

    // 45. Confirmar borrar todas las cédulas (ejecuta borrado en BD y limpia estado)
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_confirmar_borrar_todas_cedulas(move || {
            if let Some(ui) = ui_weak.upgrade() {
                handle_confirmar_borrar_todas_cedulas(&ui, &session_clone, &pool_clone);
            }
        });
    }

    // 46. Paginación de Detalle (100 en 100)
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        app_logic.on_cedulas_cambiar_pagina(move |page_num| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut sess = session_clone.lock().unwrap();
                sess.current_page = page_num as usize;
                render_cedulas_details_page_ui(&ui, &sess);
            }
        });
    }

    // 47. Filtrar detalle por búsqueda en tiempo real
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        app_logic.on_cedulas_filtrar_busqueda(move |query| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut sess = session_clone.lock().unwrap();
                sess.search_query = query.to_string();
                apply_cedulas_details_filtering(&mut sess);
                render_cedulas_details_page_ui(&ui, &sess);
            }
        });
    }

    // 48. Filtrar detalle por mes
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        app_logic.on_cedulas_filtrar_mes(move |mes_idx| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut sess = session_clone.lock().unwrap();
                sess.mes_filter_idx = mes_idx as usize;
                apply_cedulas_details_filtering(&mut sess);
                render_cedulas_details_page_ui(&ui, &sess);
            }
        });
    }

    // 49. Filtrar detalle por obra
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        app_logic.on_cedulas_filtrar_obra(move |obra_idx| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut sess = session_clone.lock().unwrap();
                sess.obra_filter_idx = obra_idx as usize;
                apply_cedulas_details_filtering(&mut sess);
                render_cedulas_details_page_ui(&ui, &sess);
            }
        });
    }

    // 50. Recargar vista de cédulas
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_recargar_cedulas_view(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let rev_id = app_data.get_selected_revision_id().to_string();
                let rp_id = app_data.get_selected_rp_id().to_string();
                let target_id = if !rp_id.is_empty() { rp_id } else { rev_id };
                if !target_id.is_empty() {
                    sync_and_load_cedulas_view_data(&ui, &pool_clone, &session_clone, &target_id);
                    if let Some(ref pool) = pool_clone {
                        update_global_stats(&ui, pool);
                    }
                    show_toast_message(&ui, "Vista de cédulas y documentos actualizada desde RustFS y base de datos.");
                } else {
                    let mut sess = session_clone.lock().unwrap();
                    if sess.audit_res.is_some() {
                        update_cedulas_audit_view_ui(&ui, &mut sess);
                    } else {
                        update_cedulas_view_file_list_ui(&ui, &sess);
                    }
                    if let Some(ref pool) = pool_clone {
                        update_global_stats(&ui, pool);
                    }
                    show_toast_message(&ui, "Vista de cédulas actualizada.");
                }
            }
        });
    }

    // 51a. Cargar Cédulas de Cuotas desde Revisiones
    {
        let ui_weak = main_window.as_weak();
        let cedulas_session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_cargar_cedulas_from_revision(move |rp_id| {
            let Some(ui) = ui_weak.upgrade() else { return; };
            let app_data = ui.global::<AppData>();
            let rp_str = rp_id.to_string();
            let effective_rp_id = if !rp_str.trim().is_empty() {
                rp_str
            } else {
                app_data.get_selected_rp_id().to_string()
            };
            if effective_rp_id.trim().is_empty() && app_data.get_selected_revision_id().to_string().trim().is_empty() {
                show_toast_message(&ui, "Seleccione un registro patronal para cargar cédulas de cuotas.");
                return;
            }

            let files = rfd::FileDialog::new()
                .set_title("Seleccionar Cédulas SUA (PDF, ZIP o RAR)")
                .add_filter("Cédulas y Paquetes", &["pdf", "zip", "rar"])
                .add_filter("Cédulas SUA (PDF)", &["pdf"])
                .add_filter("Comprimidos (ZIP/RAR)", &["zip", "rar"])
                .pick_files();

            if let Some(paths) = files {
                app_data.set_cedulas_access_allowed(true);
                app_data.set_current_view(6);
                handle_cedulas_upload_and_inspect(
                    ui_weak.clone(),
                    cedulas_session_clone.clone(),
                    pool_clone.clone(),
                    paths,
                );
            }
        });
    }

    // 51a.2 Cargar Cédulas de Cuotas desde Revisiones (Carpeta Completa)
    {
        let ui_weak = main_window.as_weak();
        let cedulas_session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_cargar_cedulas_folder_from_revision(move |rp_id| {
            let Some(ui) = ui_weak.upgrade() else { return; };
            let app_data = ui.global::<AppData>();
            let rp_str = rp_id.to_string();
            let effective_rp_id = if !rp_str.trim().is_empty() {
                rp_str
            } else {
                app_data.get_selected_rp_id().to_string()
            };
            if effective_rp_id.trim().is_empty() && app_data.get_selected_revision_id().to_string().trim().is_empty() {
                show_toast_message(&ui, "Seleccione un registro patronal para cargar cédulas de cuotas.");
                return;
            }

            let folder = rfd::FileDialog::new()
                .set_title("Seleccionar Carpeta con Cédulas SUA (Búsqueda Recursiva)")
                .pick_folder();

            if let Some(folder_path) = folder {
                app_data.set_cedulas_access_allowed(true);
                app_data.set_current_view(6);
                handle_cedulas_upload_and_inspect(
                    ui_weak.clone(),
                    cedulas_session_clone.clone(),
                    pool_clone.clone(),
                    vec![folder_path],
                );
            }
        });
    }

    // 51a.3 Solicitar Cargar Más Cédulas desde el Resumen
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_solicitar_cargar_mas_cedulas(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_cedulas_view_state(0);
                show_toast_message(&ui, "Seleccione o arrastre nuevas cédulas para incorporar a esta revisión.");
            }
        });
    }

    // 51b. Resumen Cédulas de Cuotas desde Registros de Obras (renderiza resumen sin diálogo de carga)
    {
        let ui_weak = main_window.as_weak();
        let cedulas_session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        let render_resumen = move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_cedulas_access_allowed(true);
                app_data.set_current_view(6);
                let rev_id = app_data.get_selected_revision_id().to_string();
                let rp_id = app_data.get_selected_rp_id().to_string();
                let target_id = if !rp_id.is_empty() { rp_id } else { rev_id };

                let need_load = {
                    let sess = cedulas_session_clone.lock().unwrap();
                    !target_id.is_empty() && (sess.audit_res.is_none() || sess.current_rp_id.as_deref() != Some(&target_id) || sess.files.is_empty())
                };
                if need_load {
                    sync_and_load_cedulas_view_data(&ui, &pool_clone, &cedulas_session_clone, &target_id);
                } else {
                    let mut sess = cedulas_session_clone.lock().unwrap();
                    if sess.audit_res.is_some() {
                        update_cedulas_view_file_list_ui(&ui, &sess);
                        update_cedulas_audit_view_ui(&ui, &mut sess);
                        app_data.set_cedulas_view_state(2);
                    }
                }
            }
        };
        app_logic.on_open_resumen_cedulas_from_obras(render_resumen.clone());
        app_logic.on_open_cedulas_from_obras(render_resumen);
    }

    // 52. Cerrar modal de empresa no coincide en Cédulas
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_cedulas_client_mismatch_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_cedulas_client_mismatch_modal(false);
            }
        });
    }

    // Cerrar modal de inconsistencias en Nóminas
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_nominas_mismatch_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_nominas_mismatch_modal(false);
            }
        });
    }

    // 53. Cerrar modal informativo de qué se actualizó en Cédulas
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_cedulas_success_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_cedulas_success_modal(false);
            }
        });
    }

    // 54. Cerrar modal de archivos duplicados en Cédulas
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_cedulas_duplicate_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_cedulas_duplicate_modal(false);
            }
        });
    }

    // 54c. Confirmar reemplazo de meses ya cargados: lanza el procesamiento
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_confirmar_sobrescribir_cedulas(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_cedulas_overwrite_confirm_modal(false);
                spawn_cedulas_view_worker(ui_weak.clone(), session_clone.clone(), pool_clone.clone());
            }
        });
    }

    // 54c-bis. Fusionar meses ya cargados: procesa los archivos nuevos sin borrar meses existentes
    // (el worker hace upsert por trabajador); los archivos previos no se mandan a Trash
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        let pool_clone = db_pool.clone();
        app_logic.on_fusionar_cedulas_meses_existentes(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_cedulas_overwrite_confirm_modal(false);
                let n_archivos = {
                    let mut sess = session_clone.lock().unwrap();
                    sess.pending_overwrite_months.clear();
                    sess.pending_overwrite_old_files.clear();
                    // En fusión los PDFs previos siguen vigentes: vuelven a la lista y a la auditoría
                    let previos = std::mem::take(&mut sess.pending_overwrite_old_cards);
                    sess.files.extend(previos);
                    update_cedulas_view_file_list_ui(&ui, &sess);
                    sess.pending_archivos.len()
                };
                log::info!("Cédulas: carga en modo fusión ({} archivos nuevos)", n_archivos);
                spawn_cedulas_view_worker(ui_weak.clone(), session_clone.clone(), pool_clone.clone());
            }
        });
    }

    // 54d. Cancelar reemplazo: revierte el staging en RustFS y limpia copias locales; no toca la BD
    {
        let ui_weak = main_window.as_weak();
        let session_clone = cedulas_session.clone();
        app_logic.on_cancelar_sobrescribir_cedulas(move || {
            let Some(ui) = ui_weak.upgrade() else { return; };
            ui.global::<AppData>().set_show_cedulas_overwrite_confirm_modal(false);
            let (batch_opt, ctx_opt) = {
                let mut sess = session_clone.lock().unwrap();
                let batch = sess.pending_staging_batch_id.take();
                for p in std::mem::take(&mut sess.pending_staged_local_paths) {
                    let _ = std::fs::remove_file(&p);
                }
                sess.pending_overwrite_months.clear();
                sess.pending_overwrite_old_files.clear();
                sess.pending_overwrite_old_cards.clear();
                sess.pending_archivos.clear();
                (batch, sess.pending_staging_ctx.take())
            };
            if let Some(batch_id) = batch_opt {
                match ctx_opt {
                    Some((c_nom, r_reg, rev_nom)) => {
                        std::thread::spawn(move || {
                            if let Err(e) = crate::rustfs::rollback_staging_files_in_rustfs(
                                &c_nom,
                                &r_reg,
                                &rev_nom,
                                "Cedulas_de_cuotas",
                                &batch_id,
                            ) {
                                log::warn!("No se pudo revertir el staging de cédulas en RustFS (batch {}): {}", batch_id, e);
                            }
                        });
                    }
                    None => log::warn!("Staging de cédulas (batch {}) sin contexto de cliente/RP/revisión; no se revirtió en RustFS", batch_id),
                }
            }
            ui.global::<AppLogic>().invoke_recargar_cedulas_view();
            show_toast_message(&ui, "Carga cancelada: no se modificó ningún mes.");
        });
    }

    // 54b. Cerrar modal global de error del sistema
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_system_error_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_system_error_modal(false);
            }
        });
    }

    // 55. Callbacks de Reporte Liquidaciones
    wire_reporte_liquidaciones_callbacks(&main_window, db_pool.clone(), reporte_session.clone());
    // Alta masiva de revisiones (ADD-03)
    revisiones_lote_view_logic::wire_revisiones_lote_callbacks(&main_window, db_pool.clone(), imss_inhabiles.clone());
    // Texto de atajos según el sistema operativo (Ctrl en Windows/Linux, ⌘ en macOS)
    main_window.global::<AppData>().set_asig_atajos_texto(crate::operacion_ui::texto_atajos().into());
    main_window.global::<AppData>().set_reporte_atajos_texto(crate::operacion_ui::texto_atajos_reporte().into());
    crate::asignacion_view_logic::wire_asignacion_callbacks(&main_window, db_pool.clone(), reporte_session.clone());
    crate::asistente_asignacion::wire_asistente_callbacks(&main_window, db_pool.clone(), reporte_session.clone());
    crate::asignacion_obra_logic::wire_asignacion_obra_callbacks(&main_window, db_pool.clone(), reporte_session.clone());
    // Alta masiva de obras (ADD-01/ADD-02)
    crate::obras_lote_view_logic::wire_obras_lote_callbacks(&main_window, db_pool.clone());
    crate::empleados_view_logic::wire_empleados_callbacks(&main_window, db_pool.clone());
    // Selector de analista de la sesión (ADD-05)
    analista_view_logic::wire_analista_callbacks(&main_window, db_pool.clone());

    // 56. Callbacks de Calendario Fiscal & Revisiones IMSS
    register_calendar_callbacks(&main_window, db_pool.clone(), imss_inhabiles.clone());

    // 57. Callbacks de Reporte Nóminas
    wire_nominas_callbacks(&main_window, db_pool.clone(), nominas_session.clone());

    // 57b. Callbacks de la vista de Carga de Nóminas (máquina de estados)
    crate::nominas_upload_view_logic::wire_nominas_upload_callbacks(
        &main_window,
        db_pool.clone(),
        nominas_upload_session.clone(),
        nominas_session.clone(),
    );

    // 58. Redimensionado: publica el ancho del área de contenido para que las
    // tablas adaptativas (nóminas) ajusten qué columnas muestran.
    {
        let ui_weak_resize = main_window.as_weak();
        app_logic.on_view_resized(move |w| {
            if let Some(ui) = ui_weak_resize.upgrade() {
                let d = ui.global::<AppData>();
                if d.get_window_width() != w {
                    d.set_window_width(w);
                }
            }
        });
    }

    // La vista de nóminas no depende de una revisión seleccionada (las nóminas
    // viven en revisiones sintéticas por periodo), así que se habilita siempre
    // que el reporte tenga datos que mostrar.
    if let Some(pool_nom) = db_pool.clone() {
        let ui_weak = main_window.as_weak();
        let sess = nominas_session.clone();
        std::thread::spawn(move || {
            // Se reintenta hasta que el pool tenga una conexión disponible.
            for _ in 0..30 {
                let tiene = pool_nom
                    .get()
                    .ok()
                    .and_then(|mut client| {
                        crate::db::nominas_reporte::fetch_nominas_reporte(&mut client, crate::db::nominas_reporte::AlcanceNominas::Todo).ok()
                    })
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);

                if let Some(ui) = ui_weak.upgrade() {
                    ui.global::<AppData>().set_nominas_access_allowed(tiene);
                    if tiene {
                        let _ = sync_and_load_nominas_data(&ui, &pool_nom, &sess, None);
                    }
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
    }

    // ==========================================
    // REPARACIÓN MANUAL DE CÉDULAS DESDE RUSTFS (no se ejecuta al arrancar)
    // ==========================================
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_abrir_reparar_rustfs(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                if !app_data.get_reparar_rustfs_en_curso() {
                    app_data.set_show_reparar_rustfs_modal(true);
                }
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        app_logic.on_cancelar_reparar_rustfs(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_reparar_rustfs_modal(false);
            }
        });
    }

    {
        let ui_weak = main_window.as_weak();
        let pool_rep = db_pool.clone();
        app_logic.on_confirmar_reparar_rustfs(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let app_data = ui.global::<AppData>();
            app_data.set_show_reparar_rustfs_modal(false);
            if app_data.get_reparar_rustfs_en_curso() {
                return;
            }
            app_data.set_reparar_rustfs_en_curso(true);
            show_toast_message(&ui, "Reparación iniciada en segundo plano...");

            let pool_hilo = pool_rep.clone();
            let ui_hilo = ui_weak.clone();
            let spawn_result = std::thread::Builder::new()
                .name("rustfs_reparacion_manual".into())
                .spawn(move || {
                    // Fallo del paso 1 (limpieza): no aborta la reconciliación, pero se informa al usuario
                    let mut aviso_limpieza: Option<String> = None;
                    let resultado: Result<crate::rustfs::ReconcileStats, String> = match pool_hilo {
                        None => Err("No hay conexión a la base de datos disponible".to_string()),
                        Some(pool) => match pool.get() {
                            Err(e) => Err(format!("No se pudo obtener conexión: {}", e)),
                            Ok(mut client) => {
                                log::info!("Reparación manual: limpieza y normalización de claves en liquidaciones...");
                                if let Err(e) = crate::rustfs::clean_and_repair_detalles_liquidaciones_database(&mut client) {
                                    log::warn!("Limpieza de claves (reparación manual) falló: {}", e);
                                    aviso_limpieza = Some(e);
                                } else {
                                    log::info!("Limpieza de claves completada");
                                }
                                // La limpieza automática de tramos duplicados (cleanup_all_duplicate_reportes_detalles)
                                // ya no se ejecuta: reasignaba trabajo del analista sin respetar editado_usuario.
                                // Las restricciones UNIQUE y las operaciones atómicas evitan esos duplicados.
                                log::info!("Reparación manual: reconciliación de vinculaciones desde RustFS...");
                                match crate::rustfs::reconcile_all_liquidaciones_vinculaciones_from_rustfs(&mut client) {
                                    Ok(stats) => {
                                        log::info!("Reconciliación manual completada: {:?}", stats);
                                        Ok(stats)
                                    }
                                    Err(e) => {
                                        log::warn!("Reconciliación manual falló: {}", e);
                                        Err(e)
                                    }
                                }
                            }
                        },
                    };

                    let mensaje = match resultado {
                        Ok(stats) => format!(
                            "Reparación completada: {} PDF inspeccionados, {} vinculaciones actualizadas, {} vinculaciones inválidas eliminadas, {} revisiones sincronizadas.",
                            stats.total_files_inspected,
                            stats.vinculaciones_upserted,
                            stats.fake_vinculaciones_removed,
                            stats.revisions_synced
                        ),
                        Err(e) => format!("La reparación falló: {}", e),
                    };
                    let mensaje = match aviso_limpieza {
                        Some(e) => format!("{} (Atención: la limpieza de claves falló: {})", mensaje, e),
                        None => mensaje,
                    };

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_hilo.upgrade() {
                            ui.global::<AppData>().set_reparar_rustfs_en_curso(false);
                            show_toast_message(&ui, &mensaje);
                            ui.global::<AppLogic>().invoke_recargar_cedulas_view();
                        }
                    });
                });

            if spawn_result.is_err() {
                app_data.set_reparar_rustfs_en_curso(false);
                show_toast_message(&ui, "Error: no se pudo iniciar el hilo de reparación");
            }
        });
    }

    // ==========================================
    // SISTEMA DE ACTUALIZACIONES AUTOMÁTICAS
    // ==========================================
    let downloaded_update_file: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let latest_update_info: Arc<Mutex<Option<crate::updater::UpdateInfo>>> = Arc::new(Mutex::new(None));

    // Callback: Abrir modal de actualización
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_open_update_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_show_update_modal(true);
            }
        });
    }

    // Callback: Cerrar modal de actualización
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_close_update_modal(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                if !app_data.get_update_downloading() {
                    app_data.set_show_update_modal(false);
                }
            }
        });
    }

    // Callback: Comprobar actualizaciones manualmente
    {
        let ui_weak = main_window.as_weak();
        let update_info_store = latest_update_info.clone();
        app_logic.on_check_for_updates(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_update_checking(true);
            }
            let ui_weak_thread = ui_weak.clone();
            let store = update_info_store.clone();
            std::thread::spawn(move || {
                let res = crate::updater::check_for_updates();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak_thread.upgrade() {
                        let app_data = ui.global::<AppData>();
                        app_data.set_update_checking(false);
                        match res {
                            Ok(Some(info)) => {
                                app_data.set_update_available(true);
                                app_data.set_update_version(info.tag_name.clone().into());
                                app_data.set_update_notes(info.notes.clone().into());
                                app_data.set_update_download_url(info.download_url.clone().into());
                                app_data.set_update_asset_name(info.asset_name.clone().into());
                                app_data.set_show_update_modal(true);
                                *store.lock().unwrap() = Some(info);
                            }
                            Ok(None) => {
                                show_toast_message(&ui, "Tu versión de DocuUpload ya está actualizada (v0.5.0)");
                            }
                            Err(e) => {
                                show_toast_message(&ui, &format!("No se pudo verificar actualizaciones: {}", e));
                            }
                        }
                    }
                });
            });
        });
    }

    // Callback: Iniciar descarga de actualización
    {
        let ui_weak = main_window.as_weak();
        let file_store = downloaded_update_file.clone();
        let info_store = latest_update_info.clone();
        app_logic.on_start_update_download(move || {
            let (url, asset_name) = if let Some(info) = info_store.lock().unwrap().as_ref() {
                (info.download_url.clone(), info.asset_name.clone())
            } else if let Some(ui) = ui_weak.upgrade() {
                let ad = ui.global::<AppData>();
                (ad.get_update_download_url().to_string(), ad.get_update_asset_name().to_string())
            } else {
                return;
            };

            if url.is_empty() {
                return;
            }

            if let Some(ui) = ui_weak.upgrade() {
                let ad = ui.global::<AppData>();
                ad.set_update_downloading(true);
                ad.set_update_ready_to_install(false);
                ad.set_update_progress(0.0);
                ad.set_update_progress_text("0%".into());
                ad.set_update_status_msg("Iniciando descarga...".into());
            }

            let ui_weak_thread = ui_weak.clone();
            let store = file_store.clone();
            std::thread::spawn(move || {
                let ui_weak_progress = ui_weak_thread.clone();
                let download_res = crate::updater::download_update_file(&url, &asset_name, move |frac, status| {
                    let st = status.to_string();
                    let ui_inner = ui_weak_progress.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui_inner.upgrade() {
                            let ad = ui.global::<AppData>();
                            ad.set_update_progress(frac);
                            ad.set_update_progress_text(format!("{:.0}%", frac * 100.0).into());
                            ad.set_update_status_msg(st.into());
                        }
                    });
                });

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak_thread.upgrade() {
                        let ad = ui.global::<AppData>();
                        ad.set_update_downloading(false);
                        match download_res {
                            Ok(path) => {
                                ad.set_update_ready_to_install(true);
                                ad.set_update_progress(1.0);
                                ad.set_update_status_msg("¡Descarga completada con éxito!".into());
                                *store.lock().unwrap() = Some(path);
                                show_toast_message(&ui, "✓ Actualización descargada correctamente. Lista para instalar.");
                            }
                            Err(e) => {
                                ad.set_update_ready_to_install(false);
                                ad.set_update_status_msg(format!("Error: {}", e).into());
                                show_toast_message(&ui, &format!("Error al descargar actualización: {}", e));
                            }
                        }
                    }
                });
            });
        });
    }

    // Callback: Instalar actualización ahora
    {
        let ui_weak = main_window.as_weak();
        let file_store = downloaded_update_file.clone();
        app_logic.on_install_update_now(move || {
            let path_opt = file_store.lock().unwrap().clone();
            if let Some(path) = path_opt {
                if let Err(e) = crate::updater::apply_update_now(&path)
                    && let Some(ui) = ui_weak.upgrade() {
                        show_toast_message(&ui, &format!("Error al aplicar actualización: {}", e));
                }
            } else if let Some(ui) = ui_weak.upgrade() {
                show_toast_message(&ui, "No se encontró el archivo de actualización para instalar");
            }
        });
    }

    // Verificación automática silenciosa en segundo plano al arrancar la aplicación
    {
        let ui_weak_startup = main_window.as_weak();
        let store = latest_update_info.clone();
        std::thread::spawn(move || {
            // Esperar 3 segundos para que la UI termine de inicializarse suavemente
            std::thread::sleep(std::time::Duration::from_secs(3));
            if let Ok(Some(info)) = crate::updater::check_for_updates() {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak_startup.upgrade() {
                        let app_data = ui.global::<AppData>();
                        app_data.set_update_available(true);
                        app_data.set_update_version(info.tag_name.clone().into());
                        app_data.set_update_notes(info.notes.clone().into());
                        app_data.set_update_download_url(info.download_url.clone().into());
                        app_data.set_update_asset_name(info.asset_name.clone().into());
                        *store.lock().unwrap() = Some(info);
                    }
                });
            }
        });
    }

    main_window.run()
}

#[cfg(test)]
mod tests_ui {
    use super::*;

    #[test]
    fn test_main_window_callbacks() {
        let win = MainWindow::new().unwrap();
        let app_data = win.global::<AppData>();
        let app_logic = win.global::<AppLogic>();

        let ui_weak = win.as_weak();
        app_logic.on_update_rp_actividad(move |text| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                let text_str = text.as_str();
                let count = count_chars(text_str);
                if count > 300 {
                    let (_, clamped) = limit_to_chars(text_str, 300);
                    app_data.set_rp_form_actividad(clamped.into());
                    app_data.set_rp_form_char_count(300);
                } else {
                    app_data.set_rp_form_char_count(count as i32);
                }
            }
        });

        // 1. Contador en tiempo real para 8 caracteres
        let text_8 = "12345678";
        app_logic.invoke_update_rp_actividad(text_8.into());
        assert_eq!(app_data.get_rp_form_char_count(), 8);

        // 2. Contador en tiempo real actualiza a 9 caracteres al escribir una letra
        let text_9 = "123456789";
        app_logic.invoke_update_rp_actividad(text_9.into());
        assert_eq!(app_data.get_rp_form_char_count(), 9);

        // 3. Borrar caracteres reduce el contador en tiempo real
        let text_7 = "1234567";
        app_logic.invoke_update_rp_actividad(text_7.into());
        assert_eq!(app_data.get_rp_form_char_count(), 7);

        // 4. Llegar al límite de 300 caracteres
        let text_300 = "A".repeat(300);
        app_logic.invoke_update_rp_actividad(text_300.clone().into());
        assert_eq!(app_data.get_rp_form_char_count(), 300);

        // 5. Si se intenta escribir el caracter 301, se trunca a 300 caracteres y no deja escribir más
        let text_301 = format!("{}B", text_300);
        app_logic.invoke_update_rp_actividad(text_301.into());
        assert_eq!(app_data.get_rp_form_char_count(), 300);
        assert_eq!(app_data.get_rp_form_actividad().as_str(), text_300.as_str());

        // 6. Si se pega texto largo (ej. 350 caracteres), se recorta exactamente a 300
        let text_350 = "C".repeat(350);
        app_logic.invoke_update_rp_actividad(text_350.into());
        assert_eq!(app_data.get_rp_form_char_count(), 300);
        assert_eq!(app_data.get_rp_form_actividad().as_str(), "C".repeat(300).as_str());

        // 7. Test de Cédulas de Cuotas: Estados, Paginación y Filtrado
        // Estado inicial en blanco (State 0)
        assert_eq!(app_data.get_cedulas_view_state(), 0);
        assert_eq!(app_data.get_cedulas_view_files_count(), 0);

        // Carga de archivo temporal (State 1)
        let mut session = CedulasViewState::default();
        let card = CedulaFileCardInfo {
            id: "1".into(),
            path: PathBuf::from("CEDULA_SUA_OCT2023.pdf"),
            filename: "CEDULA_SUA_OCT2023.pdf".into(),
            file_type: "PDF Vectorial IMSS".into(),
            size_str: "1.4 MB".into(),
            pages_detected: "14 Páginas".into(),
            status_badge: "Válido SUA".into(),
            status_is_valid: true,
            checksum: "e3b0c442...98b2".into(),
            has_warning: false,
            warning_msg: String::new(),
        };
        session.files.push(card);
        update_cedulas_view_file_list_ui(&win, &session);
        assert_eq!(app_data.get_cedulas_view_state(), 1);
        assert_eq!(app_data.get_cedulas_view_files_count(), 1);

        // Procesamiento y guardado (State 2) con 150 registros
        let mut audit = CedulasAuditResult {
            total_archivos_analizados: 28,
            validos_sua: 26,
            validos_pct: "92.8%".into(),
            no_procesados: 2,
            no_procesados_pct: "7.2%".into(),
            no_procesados_desc: "Sin capa de texto".into(),
            trabajadores_unicos: 342,
            repeticiones_mensuales: 906,
            importe_total: 1489630.50,
            importe_total_str: "$1,489,630.50 MXN".into(),
            cuota_patronal: 1180240.0,
            cuota_patronal_str: "$1,180,240.00".into(),
            cuota_obrera: 309390.0,
            cuota_obrera_str: "$309,390.00".into(),
            superposiciones_count: 3,
            obras_fuera_revision_count: 1,
            empleados_obras_fuera_count: 12,
            archivos_cliente_invalido_count: 0,
            sin_obras_revision_warning: false,
            periodo_rango_str: "Oct 2023 – Dic 2023".into(),
            periodo_sub_str: "Bimestres 5 y 6 (3 Periodos Mensuales)".into(),
            mensuales_count: 21,
            bimestrales_count: 5,
            sbc_promedio_str: "$584.20 MXN / día".into(),
            sbc_sub_str: "Coherente con Tabulador de Obra".into(),
            sbc_min_str: "Mínimo: $248.93".into(),
            sbc_max_str: "Máximo: $2,593.50".into(),
            ramas_rt_str: "RT: 7.588%".into(),
            ramas_sub_str: "Prestaciones en Especie y Dinero".into(),
            detalle_rows: Vec::new(),
            sumatorias_rows: Vec::new(),
            gran_total_trabajadores: 342,
            gran_total_dias: 10260,
            gran_total_patronal_str: "$1,180,240.00".into(),
            gran_total_obrera_str: "$309,390.00".into(),
            gran_total_str: "$1,489,630.50".into(),
            excel_path: None,
            mismatch_report: Default::default(),
        };

        for i in 1..=150 {
            audit.detalle_rows.push(DetalleRowModel {
                idx: i,
                reg_num_str: format!("{:04}", i),
                mes_anio: "10 / 2023".into(),
                registro_patronal: "Y5512345101".into(),
                razon_social: "EMPRESA CONSTRUCTORA".into(),
                registro_obra: "OBRA-2023-CDMX-01".into(),
                nss: format!("123456789{:02}", i % 100),
                rfc_curp: "TEST123456".into(),
                nombre_trabajador: format!("TRABAJADOR {}", i),
                dias: 30,
                sdi: 500.0,
                sdi_str: "$500.00".into(),
                cuota_patronal: 1000.0,
                cuota_patronal_str: "$1,000.00".into(),
                cuota_obrera: 250.0,
                cuota_obrera_str: "$250.00".into(),
                total: 1250.0,
                total_str: "$1,250.00".into(),
                anio: 2023,
                mes_num: 10,
            });
        }

        session.audit_res = Some(audit);
        update_cedulas_audit_view_ui(&win, &mut session);

        // Validar Estado 2 y KPIs
        assert_eq!(app_data.get_cedulas_view_state(), 2);
        assert_eq!(app_data.get_cedulas_kpi_total_analizados(), 28);
        assert_eq!(app_data.get_cedulas_kpi_trabajadores_unicos(), 342);
        assert_eq!(app_data.get_cedulas_kpi_superposiciones_count(), 3);
        assert_eq!(app_data.get_cedulas_detalle_total_count(), 150);

        // Paginación: 150 filas -> 2 páginas, página 1 muestra 100 filas
        assert_eq!(app_data.get_cedulas_page_current(), 1);
        assert_eq!(app_data.get_cedulas_page_total(), 2);
        assert_eq!(app_data.get_cedulas_page_start_row(), 1);
        assert_eq!(app_data.get_cedulas_page_end_row(), 100);
        assert_eq!(app_data.get_cedulas_detalle_list().row_count(), 100);

        // Cambiar a Página 2 (filas 101 a 150)
        session.current_page = 2;
        render_cedulas_details_page_ui(&win, &session);
        assert_eq!(app_data.get_cedulas_page_current(), 2);
        assert_eq!(app_data.get_cedulas_page_start_row(), 101);
        assert_eq!(app_data.get_cedulas_page_end_row(), 150);
        assert_eq!(app_data.get_cedulas_detalle_list().row_count(), 50);

        // Filtrado por búsqueda en tiempo real
        session.search_query = "TRABAJADOR 15".into();
        apply_cedulas_details_filtering(&mut session);
        render_cedulas_details_page_ui(&win, &session);
        assert_eq!(app_data.get_cedulas_page_current(), 1);
        assert!(app_data.get_cedulas_detalle_total_count() >= 1);

        // 8. Test de Reporte de Liquidaciones: KPIs, Paginación, Filtrado y Prorrateo en tiempo real
        use uuid::Uuid;

        let obra_id_1 = Uuid::now_v7();
        let obra_id_2 = Uuid::now_v7();

        let obras = vec![
            crate::db::reporte_liquidaciones::ObraOptionDb {
                id: obra_id_1,
                numero: "C1234567".into(),
                tipo_obra: "Oficinas Torre Norte".into(),
                fecha_inicio: Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
                fecha_fin: Some(chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()),
                label: "C1234567 - Oficinas Torre Norte".into(),
                creado_usuario: true,
                monto: 1_500_000.0,
            },
            crate::db::reporte_liquidaciones::ObraOptionDb {
                id: obra_id_2,
                numero: "C1234987".into(),
                tipo_obra: "Bodega Industrial".into(),
                fecha_inicio: Some(chrono::NaiveDate::from_ymd_opt(2026, 2, 1).unwrap()),
                fecha_fin: Some(chrono::NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
                label: "C1234987 - Bodega Industrial".into(),
                creado_usuario: true,
                monto: 2_000_000.0,
            },
            crate::db::reporte_liquidaciones::ObraOptionDb {
                id: Uuid::now_v7(),
                numero: "C9999999".into(),
                tipo_obra: "Obra de Cédula".into(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "C9999999 - Obra de Cédula".into(),
                creado_usuario: false,
                monto: 800_000.0,
            },
        ];

        let mut rows = Vec::new();
        for i in 1..=250 {
            let is_assigned = i % 2 == 0;
            let oid = if is_assigned { Some(obra_id_1) } else { None };
            let obra_num = if is_assigned { "C1234567".into() } else { String::new() };
            let obra_lbl = if is_assigned { "C1234567 - Oficinas Torre Norte".into() } else { "⚠️ Sin Asignar (Pendiente)".into() };

            let dias_ced = 31;
            let dias_obr = if is_assigned { 20 } else { 0 };

            let mut r = crate::db::reporte_liquidaciones::ReporteLiquidacionRowDb {
                id_reporte_liq: Uuid::now_v7(),
                id_parent_reporte_liq: Uuid::now_v7(),
                id_liquidacion: Uuid::now_v7(),
                id_revision: Uuid::now_v7(),
                id_empleado: Uuid::now_v7(),
                nombre_trabajador: format!("EMPLEADO TEST {:03}", i),
                rfc: format!("TEST{:04}0101", i),
                curp: format!("TEST{:04}0101HDF00", i),
                nss: format!("123456{:05}", i),
                pensionado: String::new(),
                registro_patronal: "Y54-10204-10-8".into(),
                mes_periodo: if i <= 100 { "enero".into() } else if i <= 200 { "febrero".into() } else { "marzo".into() },
                mes_periodo_int: if i <= 100 { 1 } else if i <= 200 { 2 } else { 3 },
                anio_periodo: 2026,
                periodo_str: if i <= 100 { "2026-01".into() } else if i <= 200 { "2026-02".into() } else { "2026-03".into() },
                dias_cedula: dias_ced,
                id_registro_obra: oid,
                obra_numero: obra_num,
                obra_label: obra_lbl,
                dias_obra: dias_obr,
                obra_creado_usuario: is_assigned,

                clave: "08".to_string(),
                fecha: None,
                fecha_str: "-".to_string(),
                sdi: 250.0,
                lic: 0.0,
                inc: 0.0,
                aus: 0.0,

                cf: 0.0,
                exc_pat: 0.0,
                exc_obr: 0.0,
                pd_pat: 0.0,
                pd_obr: 0.0,
                gmp_pat: 0.0,
                gmp_obr: 0.0,
                rt: 0.0,
                iv_pat: 0.0,
                iv_obr: 0.0,
                gps: 0.0,
                patronal: 0.0,
                obrera: 0.0,
                subtotal: 0.0,

                cf_orig: 420.10,
                exc_pat_orig: 112.50,
                exc_obr_orig: 45.00,
                pd_pat_orig: 25.30,
                pd_obr_orig: 10.12,
                gmp_pat_orig: 38.40,
                gmp_obr_orig: 15.36,
                rt_orig: 520.40,
                iv_pat_orig: 185.00,
                iv_obr_orig: 65.00,
                gps_orig: 355.22,
                patronal_orig: 1656.92,
                obrera_orig: 135.48,
                subtotal_orig: 1792.40,
                tiene_inconsistencia: false,
                observacion_validacion: String::new(),
                estado: "ACTIVO".to_string(),
                cedula_actualizada: false,
                editado_usuario: false,
            };

            recalculate_row_in_memory(&mut r);
            rows.push(r);
        }

        // 1. Validar cálculo en tiempo real con la fórmula: cf / 31 * 20
        // Para empleado con 31 días cédula y 20 días obra:
        // factor = 20 / 31 = 0.645161...
        // cf = round(420.10 * 20 / 31) = 271.03
        let sample = &rows[1]; // i = 2 (asignado, 20 días)
        let expected_cf = (420.10f64 * 20.0 / 31.0 * 100.0).round() / 100.0;
        assert_eq!(sample.cf, expected_cf);

        // Si se modifican los días de cédula a 21 y obra a 5:
        let mut sample_mod = sample.clone();
        sample_mod.dias_cedula = 21;
        sample_mod.dias_obra = 5;
        recalculate_row_in_memory(&mut sample_mod);
        let expected_cf_mod = (420.10f64 * 5.0 / 21.0 * 100.0).round() / 100.0;
        assert_eq!(sample_mod.cf, expected_cf_mod);

        // 2. Estado de Sesión y Renderizado
        let mut state = ReporteSessionState {
            current_revision_id: Some("rev-123".into()),
            cliente_nombre: "CONSTRUCTORA DEL NORTE SA DE CV".into(),
            rp_registro: "Y54-10204-10-8".into(),
            revision_nombre: "CONTRATO SIVESA OBRA CIVIL".into(),
            all_rows: rows,
            obras_options: obras,
            periodo_options: vec!["Todos los meses".into(), "2026-01".into(), "2026-02".into(), "2026-03".into()],
            ..Default::default()
        };

        apply_reporte_filtering(&mut state);
        render_reporte_ui(&win, &state);

        // 3. Validar KPIs (Solo deben contarse y mostrarse las obras creadas por el usuario: 2 de 3)
        assert_eq!(app_data.get_reporte_kpi_total_registros(), 250);
        assert_eq!(app_data.get_reporte_kpi_obras_count(), 2);
        assert_eq!(app_data.get_reporte_guarderias_obras().row_count(), 2);
        assert_eq!(app_data.get_reporte_total_filtered(), 250);

        // 4. Validar Paginación (250 filas -> 3 páginas de 100 en 100)
        assert_eq!(app_data.get_reporte_page_current(), 1);
        assert_eq!(app_data.get_reporte_page_total(), 3);
        assert_eq!(app_data.get_reporte_page_start_row(), 1);
        assert_eq!(app_data.get_reporte_page_end_row(), 100);
        assert_eq!(app_data.get_reporte_liquidaciones_list().row_count(), 100);

        // Página 2 (101 a 200)
        state.current_page = 2;
        render_reporte_ui(&win, &state);
        assert_eq!(app_data.get_reporte_page_current(), 2);
        assert_eq!(app_data.get_reporte_page_start_row(), 101);
        assert_eq!(app_data.get_reporte_page_end_row(), 200);
        assert_eq!(app_data.get_reporte_liquidaciones_list().row_count(), 100);

        // Página 3 (201 a 250 -> 50 filas)
        state.current_page = 3;
        render_reporte_ui(&win, &state);
        assert_eq!(app_data.get_reporte_page_current(), 3);
        assert_eq!(app_data.get_reporte_page_start_row(), 201);
        assert_eq!(app_data.get_reporte_page_end_row(), 250);
        assert_eq!(app_data.get_reporte_liquidaciones_list().row_count(), 50);

        // 5. Filtrado por Búsqueda
        state.current_page = 1;
        state.search_query = "EMPLEADO TEST 015".into();
        apply_reporte_filtering(&mut state);
        render_reporte_ui(&win, &state);
        assert_eq!(state.filtered_rows.len(), 1);
        assert_eq!(app_data.get_reporte_total_filtered(), 1);
        assert_eq!(app_data.get_reporte_liquidaciones_list().row_count(), 1);

        // 6. Filtrado por Periodo (2026-01 -> 100 filas)
        state.search_query.clear();
        state.filter_periodo_idx = 1; // "2026-01"
        apply_reporte_filtering(&mut state);
        render_reporte_ui(&win, &state);
        assert_eq!(state.filtered_rows.len(), 100);
        assert_eq!(app_data.get_reporte_total_filtered(), 100);

        // 7. Filtrado por Estado (1 = Solo Asignados -> 50 en enero)
        state.filter_estado_idx = 1;
        apply_reporte_filtering(&mut state);
        render_reporte_ui(&win, &state);
        assert_eq!(state.filtered_rows.len(), 50);

        // 8. Modificación Masiva: Igualar Días de Obra a Días Cédula
        // Limpiamos filtros
        state.filter_periodo_idx = 0;
        state.filter_estado_idx = 0;
        apply_reporte_filtering(&mut state);

        for row in state.all_rows.iter_mut() {
            row.dias_obra = row.dias_cedula;
            recalculate_row_in_memory(row);
        }
        apply_reporte_filtering(&mut state);
        render_reporte_ui(&win, &state);

        // Ahora todos tienen factor = 1.0 (subtotal == subtotal_orig)
        for r in &state.all_rows {
            assert_eq!(r.subtotal, r.subtotal_orig);
        }

        // Atajo Ctrl/⌘+F en Reporte de Liquidaciones
        // Reutiliza la misma ventana: no se crea un segundo event loop (winit solo permite uno por proceso).
        {
            use slint::platform::{Key, WindowEvent};
            let ad = win.global::<AppData>();
            ad.set_current_view(7);
            ad.set_reporte_tab_idx(0);
            slint::platform::update_timers_and_animations();

            let antes = ad.get_reporte_focus_buscar();
            let ventana = win.window();
            ventana.dispatch_event(WindowEvent::WindowActiveChanged(true));
            ventana.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
            ventana.dispatch_event(WindowEvent::KeyPressed { text: "f".into() });
            ventana.dispatch_event(WindowEvent::KeyReleased { text: "f".into() });
            ventana.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
            slint::platform::update_timers_and_animations();

            assert_eq!(ad.get_reporte_focus_buscar(), antes + 1);
        }
    }
}

#[cfg(test)]
mod tests_mensajes_obras {
    use super::*;
    use crate::db::obras_ops::CambioUniverso;

    #[test]
    fn mensaje_cambio_universo_sin_cambios_es_vacio() {
        let c = CambioUniverso { a_fuera: 0, a_activo: 0 };
        assert_eq!(mensaje_cambio_universo(&c), "");
    }

    #[test]
    fn mensaje_cambio_universo_con_cambios() {
        let c = CambioUniverso { a_fuera: 2, a_activo: 1 };
        assert_eq!(
            mensaje_cambio_universo(&c),
            "Reporte de liquidaciones: 2 fila(s) pasaron a fuera de periodo, 1 regresaron al periodo."
        );
    }
}


