#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
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
pub mod sat;
pub mod sua_export;
pub mod updater;
pub mod validation;

use crate::calendario_view_logic::*;
use crate::db::{create_pool, ensure_schema, DbPool, CedulasDbStats};
use crate::db::catalogos::*;
use crate::db::revisiones::*;
use crate::db::nominas_reporte::check_all_data_exists;
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
use sha2::{Digest, Sha256};

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

    let models: Vec<EmpleadoData> = emps
        .into_iter()
        .filter(|emp| {
            if only_active && !emp.esta_activo {
                return false;
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
            EmpleadoData {
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
        total_monto += o.monto;
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
                monto: o.monto as f32,
                monto_str: format!("${} MXN", format_monto_mexico(o.monto)).into(),
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
                (SELECT COUNT(*) FROM public.registros_obras WHERE COALESCE(creado_usuario, false) = true)::int
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

    let (cliente_nombre, rp_registro, revision_nombre, revision_id) = if let Some(ui) = ui_weak.upgrade() {
        let app_data = ui.global::<AppData>();
        (
            app_data.get_selected_cliente_nombre().to_string(),
            app_data.get_selected_rp_registro().to_string(),
            app_data.get_selected_revision_nombre().to_string(),
            app_data.get_selected_revision_id().to_string(),
        )
    } else {
        return;
    };

    if let Some(ui) = ui_weak.upgrade() {
        show_toast_message(&ui, "Analizando archivos y verificando duplicados...");
    }

    thread::spawn(move || {
        let mut c_nom = cliente_nombre;
        let mut r_reg = rp_registro;
        let mut rev_nom = revision_nombre;
        let mut c_rfc = String::new();

        if let Some(ref pool) = pool_clone {
            if let Ok(mut client) = pool.get() {
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
                }
            }
        }

        let temp_dir_handle = tempfile::Builder::new().prefix("docu_ffacd_upload_").tempdir();
        let temp_dir = match temp_dir_handle {
            Ok(ref d) => d.path().to_path_buf(),
            Err(_) => std::env::temp_dir(),
        };

        let mut items = match crate::archives::collect_prepared_upload_items(&paths, "pdf", &temp_dir) {
            Ok(it) => it,
            Err(e) => {
                log::error!("Error preparando archivos para subida: {}", e);
                return;
            }
        };

        // Asignar a cada archivo su nombre (Cédula SUA <rp> - <mes>.pdf o nombre original si desambigua)
        let mut seen_batch_names: std::collections::HashMap<String, String> = std::collections::HashMap::new(); // gen_name -> hash
        for item in &mut items {
            let data = fs::read(&item.local_path).unwrap_or_default();
            let mut h = Sha256::new();
            h.update(&data);
            let item_hash = format!("{:x}", h.finalize());

            let gen_name = crate::cedulas::view_logic::extract_cedula_pdf_postgres_filename(&item.local_path, &r_reg);
            if let Some(prev_hash) = seen_batch_names.get(&gen_name) {
                if *prev_hash != item_hash {
                    // Contenido diferente: conservar el nombre original del archivo para no colisionar
                    let orig_name = Path::new(&item.local_path)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| gen_name.clone());
                    item.rel_path = orig_name;
                } else {
                    item.rel_path = gen_name;
                }
            } else {
                seen_batch_names.insert(gen_name.clone(), item_hash);
                item.rel_path = gen_name;
            }
        }

        if items.is_empty() {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    show_toast_message(&ui, "No se encontraron archivos de cédula PDF válidos en la selección.");
                }
            });
            return;
        }

        let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(&revision_id);

        // 1. Obtener archivos existentes en RustFS estrictamente para esta revisión
        let existing_cards = crate::rustfs::sync_and_load_cedulas_view_files(
            &c_nom,
            &r_reg,
            &rev_nom,
            &cache_dir,
        ).unwrap_or_default();

        // 2. Extraer metadatos de archivos existentes (ruta normalizada y SHA-256)
        #[allow(dead_code)]
        struct ExistingFileMeta {
            rel_path: String,
            full_sha256: String,
        }

        let mut existing_metas: Vec<ExistingFileMeta> = Vec::new();
        for card in &existing_cards {
            let full_hash = if let Ok(bytes) = fs::read(&card.path) {
                let mut h = Sha256::new();
                h.update(&bytes);
                format!("{:x}", h.finalize())
            } else {
                String::new()
            };
            existing_metas.push(ExistingFileMeta {
                rel_path: card.filename.clone(),
                full_sha256: full_hash,
            });
        }

        // 3. Consultar estado en PostgreSQL para esta revisión específica
        let mut _db_existing_liq_count = 0i64;
        if let Some(ref pool) = pool_clone
            && let Ok(mut client) = pool.get()
            && let Ok(rev_uuid) = uuid::Uuid::parse_str(&revision_id)
            && let Ok(row) = client.query_one(
                "SELECT COUNT(*) FROM public.liquidaciones WHERE id_revision = $1",
                &[&rev_uuid],
            ) {
            _db_existing_liq_count = row.get(0);
        }

        // 4. Procesar archivos: Estandarizar nombre 'Nombre Cliente__Registro Patronal__Mes__Anio.pdf',
        //    estructurar físicamente en Año/Mes, y aplicar lógica de sobreescritura si el mes ya existe.
        let mut overwritten_count = 0;
        let mut new_count = 0;

        for item in items {
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
                    &format!("Trash/{}", item.rel_path),
                    &data,
                );
                continue;
            }

            // Extraer metadatos para nombre estricto y estructura Año/Mes
            let (std_fname, anio_str, mes_str, _, _) = crate::cedulas::view_logic::extract_cedula_standard_naming(
                &item.local_path,
                &c_nom,
                &r_reg,
            );
            let structured_rel_path = format!("{}/{}/{}", anio_str, mes_str, std_fname);

            // Verificar si el mes ya existe previamente
            let existing_match = existing_cards.iter().find(|ex| {
                let fn_up = ex.filename.to_uppercase();
                fn_up.contains(&format!("__{}__{}.PDF", mes_str, anio_str))
                    || fn_up.contains(&format!("{}/{}", anio_str, mes_str))
                    || fn_up == std_fname.to_uppercase()
            });

            if let Some(old_card) = existing_match {
                overwritten_count += 1;
                log::info!("Sobreescribiendo cédula previa del mes {}/{}: {}", mes_str, anio_str, old_card.filename);

                // 1. Mover el archivo físico anterior a la papelera (trash)
                let _ = crate::rustfs::move_file_to_trash(
                    &c_nom,
                    &r_reg,
                    &rev_nom,
                    "Cedulas_de_cuotas",
                    &old_card.filename,
                );

                // Limpiar caché local
                let old_local = cache_dir.join("cedulas_de_cuotas").join(&old_card.filename);
                let _ = std::fs::remove_file(&old_local);

                // 2. En PostgreSQL, eliminar (DELETE) la información correspondiente a ese mes específico
                if let Some(ref pool) = pool_clone {
                    if let Ok(mut client) = pool.get() {
                        let anio_num: i32 = anio_str.parse().unwrap_or(2026);
                        if let Err(e) = crate::cedulas::view_logic::delete_cedula_for_month(&mut client, &revision_id, &mes_str, anio_num) {
                            log::error!("Error al eliminar liquidaciones de mes {} en BD: {}", mes_str, e);
                        }
                    }
                }
            } else {
                new_count += 1;
            }

            // 3. Guardar el nuevo archivo en la ruta correspondiente (Año/Mes)
            if let Err(e) = crate::rustfs::upload_file_to_rustfs(
                &c_nom,
                &r_reg,
                &rev_nom,
                "Cedulas_de_cuotas",
                &structured_rel_path,
                &data,
            ) {
                log::error!("Error subiendo archivo {} a RustFS: {}", structured_rel_path, e);
            }
        }

        // 5. Sincronizar y recargar la lista activa desde RustFS
        let mut final_cards = crate::rustfs::sync_and_load_cedulas_view_files(
            &c_nom,
            &r_reg,
            &rev_nom,
            &cache_dir,
        ).unwrap_or(existing_cards);

        let exp_rfc_opt = if !c_rfc.trim().is_empty() { Some(c_rfc.trim()) } else { None };
        let exp_rp_opt = if !r_reg.trim().is_empty() && r_reg != "GENERAL" { Some(r_reg.trim()) } else { None };

        for card in &mut final_cards {
            let path_buf = PathBuf::from(&card.id);
            if path_buf.exists() {
                let inspected = crate::cedulas::view_logic::inspect_cedula_file_with_context(
                    &path_buf,
                    exp_rfc_opt,
                    exp_rp_opt,
                );
                card.warning_msg = inspected.warning_msg;
            }
        }

        // 6. Actualizar la interfaz Slint
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut sess = session_clone.lock().unwrap();
                sess.current_revision_id = Some(revision_id);
                sess.files = final_cards;
                update_cedulas_view_file_list_ui(&ui, &sess);
                if ui.global::<AppData>().get_cedulas_view_state() == 0 && !sess.files.is_empty() {
                    ui.global::<AppData>().set_cedulas_view_state(1);
                }

                if overwritten_count > 0 {
                    show_toast_message(&ui, &format!("Se estructuraron {} cédula(s) nueva(s) y se sobreescribieron {} de meses existentes (movidas a papelera).", new_count, overwritten_count));
                } else {
                    show_toast_message(&ui, &format!("Se estructuraron y cargaron {} cédula(s) en carpetas Año/Mes en RustFS.", new_count));
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
    let (input_paths, client_rfc, client_nombre, revision_id, revision_nombre) = {
        let Some(ui) = ui_weak.upgrade() else { return; };
        let app_data = ui.global::<AppData>();
        let mut session = session_arc.lock().unwrap();
        if session.files.is_empty() {
            let rev_id = app_data.get_selected_revision_id().to_string();
            let c_nom = app_data.get_selected_cliente_nombre().to_string();
            let rp = app_data.get_selected_rp_registro().to_string();
            let rev_nom = app_data.get_selected_revision_nombre().to_string();
            if !rev_id.is_empty() && !c_nom.is_empty() {
                let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(&rev_id);
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
        }
        if session.files.is_empty() {
            app_data.set_cedulas_view_is_processing(false);
            show_toast_message(&ui, "Por favor agrega al menos un archivo antes de procesar.");
            return;
        }
        if app_data.get_cedulas_view_is_processing() {
            return;
        }
        app_data.set_cedulas_view_is_processing(true);
        app_data.set_cedulas_view_progress_fraction(0.02);
        app_data.set_cedulas_view_progress_text("Iniciando motor de layout SUA y auditoría documental...".into());

        let paths: Vec<PathBuf> = session.files.iter().map(|f| f.path.clone()).collect();
        let rfc = app_data.get_selected_cliente_rfc().to_string();
        let nombre = app_data.get_selected_cliente_nombre().to_string();
        let rev_id = app_data.get_selected_revision_id().to_string();
        let rev_nom = app_data.get_selected_revision_nombre().to_string();
        (paths, rfc, nombre, rev_id, rev_nom)
    };

    let ui_w = ui_weak.clone();
    let session_w = session_arc.clone();
    let pool_w = pool_opt.or_else(crate::db::get_global_pool);

    thread::spawn(move || {
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
        if let Some(ref pool) = pool_w {
            if let Ok(mut c) = pool.get() {
                if let Ok(rev_uuid) = uuid::Uuid::parse_str(&revision_id) {
                    if let Ok(row) = c.query_opt(
                        "SELECT rp.registro FROM revisiones r JOIN registros_patronales rp ON r.id_registro_patronal = rp.id WHERE r.id = $1",
                        &[&rev_uuid],
                    ) {
                        if let Some(r) = row {
                            rp_for_rustfs = r.get::<_, String>(0);
                        }
                    }
                }
            }
        }
        if rp_for_rustfs.is_empty() {
            let app_data_rp = if let Some(ui) = ui_w.upgrade() {
                ui.global::<AppData>().get_selected_rp_registro().to_string()
            } else {
                String::new()
            };
            if !app_data_rp.is_empty() {
                rp_for_rustfs = app_data_rp;
            } else {
                rp_for_rustfs = "GENERAL".to_string();
            }
        }

        // Usar los archivos que ya están en memoria/disco local si existen; solo sincronizar de RustFS si está vacío
        let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(&revision_id);
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

        let ui_for_cb = ui_w.clone();
        let progress_cb = move |current: usize, total: usize, msg: &str| {
            let frac = if total > 0 {
                0.05 + (current as f32 / total as f32) * 0.55
            } else {
                0.05
            };
            let text = msg.to_string();
            let ui_clone = ui_for_cb.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_clone.upgrade() {
                    let app_data = ui.global::<AppData>();
                    app_data.set_cedulas_view_progress_fraction(frac);
                    app_data.set_cedulas_view_progress_text(text.into());
                }
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

                // Guardar en Base de Datos PostgreSQL con progreso interactivo en la UI
                let mut db_saved_summary = String::new();
                let mut saved_stats_opt: Option<CedulasDbStats> = None;
                if let Some(ref pool) = pool_w {
                    let ui_for_db_cb = ui_w.clone();
                    let db_progress_cb = move |current: usize, total: usize, msg: &str| {
                        let frac = if total > 0 {
                            0.60 + (current as f32 / total as f32) * 0.35
                        } else {
                            0.75
                        };
                        let text = msg.to_string();
                        let ui_clone = ui_for_db_cb.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_clone.upgrade() {
                                let app_data = ui.global::<AppData>();
                                app_data.set_cedulas_view_progress_fraction(frac);
                                app_data.set_cedulas_view_progress_text(text.into());
                            }
                        });
                    };

                    match save_cedulas_audit_to_db(pool, &revision_id, &businesses, Some(&db_progress_cb)) {
                        Ok(stats) => {
                            db_saved_summary = format!(
                                "Guardadas {} liquidaciones y {} detalles de cédulas.",
                                stats.liquidaciones_guardadas, stats.detalles_guardados
                            );
                            log::info!("Guardado DB Cédulas: {:?}", stats);
                            saved_stats_opt = Some(stats);

                            // Sincronizar automáticamente hacia reportes_liquidaciones
                            if let Ok(rev_uuid) = uuid::Uuid::parse_str(&revision_id) {
                                if let Ok(mut client) = pool.get() {
                                    match crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision(&mut client, rev_uuid) {
                                        Ok(c) => log::info!("Sincronizadas {} filas hacia reportes_liquidaciones", c),
                                        Err(e) => log::error!("Error sincronizando reportes_liquidaciones: {}", e),
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Error guardando cédulas en BD: {}", e);
                            db_saved_summary = format!("Advertencia DB: {}", e);
                        }
                    }
                }

                // Actualizar sesión y Slint UI
                let ui_final = ui_w.clone();
                let session_final = session_w.clone();
                let pool_stats = pool_w.clone();

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_final.upgrade() {
                        let is_all_failed = audit_res.validos_sua == 0 || businesses.is_empty();
                        if is_all_failed {
                            {
                                let mut sess = session_final.lock().unwrap();
                                reset_cedulas_ui_to_state_zero(&ui, &mut sess);
                            }
                            let app_data = ui.global::<AppData>();
                            app_data.set_cedulas_view_is_processing(false);
                            app_data.set_cedulas_view_progress_fraction(0.0);
                            app_data.set_cedulas_view_progress_text("".into());

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
                            sess.audit_res = Some(audit_res);
                            sess.businesses = businesses;
                            sess.current_revision_id = Some(revision_id.clone());
                            update_cedulas_audit_view_ui(&ui, &mut sess);
                        }
                        let app_data = ui.global::<AppData>();
                        app_data.set_cedulas_view_is_processing(false);
                        app_data.set_cedulas_view_state(2);
                        app_data.set_cedulas_view_progress_fraction(1.0);
                        app_data.set_cedulas_view_progress_text("¡Procesamiento y guardado completado!".into());
                        app_data.set_reporte_liquidaciones_access_allowed(true);

                        // Actualizar propiedades de navegación tras guardar cédulas
                        if let Some(ref pool) = pool_stats {
                            if let Ok(mut client) = pool.get() {
                                if let Ok(rev_uuid) = Uuid::parse_str(&revision_id) {
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
                            let msg_obras = if stats.vinculaciones_guardadas > 0 {
                                format!(
                                    "Se vincularon {} liquidaciones a las obras creadas por el usuario en esta revisión. No se crearon registros de obra nuevos.",
                                    stats.vinculaciones_guardadas
                                )
                            } else {
                                "No se crearon registros de obra en la base de datos. Se conservan exclusivamente las obras creadas por el usuario.".to_string()
                            };
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
                        app_data.set_cedulas_view_progress_fraction(0.0);
                        app_data.set_cedulas_view_progress_text("".into());

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
                            show_toast_message(&ui, &format!("Error al procesar: {}", err_msg));
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
    rev_id: &str,
) {
    if rev_id.is_empty() {
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

    // 1. Obtener metadatos de la revisión (cliente, registro patronal, nombre revisión, rfc)
    let rev_meta = match uuid::Uuid::parse_str(rev_id.trim()) {
        Ok(rev_uuid) => {
            client.query_opt(
                r#"
                SELECT 
                    c.nombre AS cliente_nombre,
                    rp.registro AS rp_registro,
                    r.nombre AS revision_nombre,
                    c.rfc AS cliente_rfc
                FROM public.revisiones r
                JOIN public.registros_patronales rp ON r.id_registro_patronal = rp.id
                JOIN public.clientes c ON rp.id_cliente = c.id
                WHERE r.id = $1
                "#,
                &[&rev_uuid],
            ).ok().flatten()
        }
        Err(_) => None,
    };

    let (cliente_nombre, rp_registro, revision_nombre, cliente_rfc) = match rev_meta {
        Some(row) => (
            row.get::<_, String>(0),
            row.get::<_, String>(1),
            row.get::<_, String>(2),
            row.get::<_, Option<String>>(3).unwrap_or_default(),
        ),
        None => {
            let app_data = ui.global::<AppData>();
            (
                app_data.get_selected_cliente_nombre().to_string(),
                app_data.get_selected_rp_registro().to_string(),
                app_data.get_selected_revision_nombre().to_string(),
                app_data.get_selected_cliente_rfc().to_string(),
            )
        }
    };

    // 2. Sincronizar y descargar archivos de RustFS
    let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(rev_id);
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

    // 3. Cargar liquidaciones y calcular estadísticos desde PostgreSQL
    let db_res = load_cedulas_from_db(&mut client, rev_id);

    // 4. Actualizar sesión y componentes Slint UI
    let mut sess = session_arc.lock().unwrap();
    sess.current_revision_id = Some(rev_id.to_string());
    sess.files = rustfs_files;

    match db_res {
        Ok(Some((audit, businesses))) => {
            sess.audit_res = Some(audit);
            sess.businesses = businesses;
            update_cedulas_view_file_list_ui(ui, &sess);
            update_cedulas_audit_view_ui(ui, &mut sess);
            ui.global::<AppData>().set_cedulas_view_state(2);
        }
        Ok(None) | Err(_) => {
            sess.audit_res = None;
            sess.businesses.clear();
            sess.filtered_details.clear();
            if !sess.files.is_empty() {
                update_cedulas_view_file_list_ui(ui, &sess);
                ui.global::<AppData>().set_cedulas_view_state(1);
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
    let revision_id = app_data.get_selected_revision_id().to_string();
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
                if let Ok(rev_uuid) = uuid::Uuid::parse_str(&revision_id) {
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

                match delete_all_cedulas_for_revision(&mut client, &revision_id) {
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
        let cache_dir = std::env::temp_dir().join("docu_ffacd_rustfs_cache").join(&revision_id);
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
                    eprintln!("Error inicializando esquema de BD: {}", e);
                    log::error!("Error inicializando esquema de BD: {}", e);
                    crate::app_logger::log_db_simple_error(
                        "esquema",
                        "ensure_schema",
                        &format!("Error inicializando esquema: {}", e),
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
                // View 6 (Cédula de Cuotas) deshabilitada hasta acceder desde Registros de Obras ("Cargar Cédulas de Cuotas")
                if view_idx == 6 && !app_data.get_cedulas_access_allowed() {
                    crate::app_logger::log_ui_error(
                        "Navegación",
                        "Sidebar / Menú",
                        "Acceso Denegado a Vista",
                        &format!("No se puede acceder a {} (ID {}): acceso a cédulas no permitido", view_name, view_idx),
                    );
                    app_data.set_current_view(if !rev_id.is_empty() { 4 } else if !rp_id.is_empty() { 3 } else if !client_id.is_empty() { 2 } else { 1 });
                    return;
                }
                // View 7 (Reporte Liquidaciones) deshabilitada hasta tener revisión seleccionada
                if view_idx == 7 && rev_id.is_empty() {
                    crate::app_logger::log_ui_error(
                        "Navegación",
                        "Sidebar / Menú",
                        "Acceso Denegado a Vista",
                        &format!("No se puede acceder a {} (ID {}): requiere revisión seleccionada", view_name, view_idx),
                    );
                    app_data.set_current_view(if !rp_id.is_empty() { 3 } else if !client_id.is_empty() { 2 } else { 1 });
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
                    let need_load = {
                        let sess = cedulas_session_clone.lock().unwrap();
                        !rev_id.is_empty() && (sess.audit_res.is_none() || sess.current_revision_id.as_deref() != Some(&rev_id) || sess.files.is_empty())
                    };
                    if need_load {
                        sync_and_load_cedulas_view_data(&ui, &pool_clone, &cedulas_session_clone, &rev_id);
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
                    sync_and_load_reporte_data(&ui, &pool_clone, &reporte_session_clone, &rev_id);
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
        app_logic.on_submit_revision(move |id, nombre, folio, concepto_val, estatus_val, estatus_imss_val, f_ent_rev, f_max_ent_rev, f_ent_imss, f_max_ent_imss, f_aprob_imss, pidio_pror, f_aprob_pror, f_ent_pror| {
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

                            app_data.set_show_revision_modal(false);
                            load_revisiones_ui(&ui, pool, &rp_id);
                            update_global_stats(&ui, pool);
                            let msg = if is_edit { "Revisión actualizada exitosamente." } else { "Revisión creada exitosamente." };
                            show_toast_message(&ui, msg);
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
                                let monto_fmt = if o.monto > 0.0 { format_monto_mexico(o.monto) } else { "".to_string() };
                                let monto_preview = if o.monto > 0.0 { format!("${} MXN", monto_fmt) } else { "".to_string() };
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
                let clean_monto_str = monto_str.replace(',', "");
                let monto: f64 = clean_monto_str.trim().parse().unwrap_or(0.0);

                if let Some(ref pool) = pool_clone {
                    let mut client = match pool.get() {
                        Ok(c) => c,
                        Err(e) => {
                            show_toast_message(&ui, &format!("Error de conexión: {}", e));
                            return;
                        }
                    };

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
                    let res = if is_edit {
                        update_obra(&mut client, &item)
                    } else {
                        create_obra(&mut client, &item)
                    };

                    match res {
                        Ok(_) => {
                            app_data.set_show_obra_modal(false);
                            app_data.set_obra_form_numero_status("".into());
                            app_data.set_obra_form_numero_msg("".into());
                            load_obras_ui(&ui, pool, &rev_id);
                            update_global_stats(&ui, pool);
                            let msg = if monto > 0.0 {
                                if is_edit { "Obra actualizada exitosamente." } else { "Registro de obra creado exitosamente." }
                            } else {
                                if is_edit {
                                    "Obra actualizada. Aviso: Monto Total en 0; recuerde capturarlo para poder calcular el límite del 18% en Reporte Liquidaciones."
                                } else {
                                    "Registro de obra creado. Aviso: Monto Total en 0; recuerde capturarlo para poder calcular el límite del 18% en Reporte Liquidaciones."
                                }
                            };
                            show_toast_message(&ui, msg);
                        }
                        Err(e) => {
                            show_toast_message(&ui, &format!("Error al guardar obra: {}", e));
                        }
                    }
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
        app_logic.on_open_delete_obra_dialog(move |id, numero| {
            if let Some(ui) = ui_weak.upgrade() {
                let app_data = ui.global::<AppData>();
                app_data.set_delete_obra_id(id);
                app_data.set_delete_obra_numero(numero);
                app_data.set_show_delete_obra_modal(true);
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
                        let res = match pool.get() {
                            Ok(mut client) => delete_obra(&mut client, obra_id_str.as_str()),
                            Err(e) => Err(format!("Error obteniendo conexión a la BD: {}", e)),
                        };

                        let pool_for_ui = pool.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = ui_weak2.upgrade() {
                                match res {
                                    Ok(_) => {
                                        let app_data = ui.global::<AppData>();
                                        let rev_id = app_data.get_selected_revision_id().to_string();
                                        load_obras_ui(&ui, &pool_for_ui, &rev_id);
                                        update_global_stats(&ui, &pool_for_ui);
                                        show_toast_message(&ui, "Registro de obra eliminado correctamente.");
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
        app_logic.on_submit_new_cliente(move |nombre, rfc, razon_social, curp, tp_val, reg_val| {
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
        app_logic.on_submit_edit_cliente(move |id, nombre, razon_social, rfc, curp, tp_val, reg_val| {
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
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let client_id = ui.global::<AppData>().get_selected_cliente_id().to_string();
                    load_registros_patronales_ui(&ui, pool, &client_id);
                }
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
            if let Some(ui) = ui_weak.upgrade() {
                if let Some(ref pool) = pool_clone {
                    let client_id = ui.global::<AppData>().get_selected_cliente_id().to_string();
                    load_empleados_ui(&ui, pool, &client_id);
                }
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
                if !rev_id.is_empty() {
                    sync_and_load_cedulas_view_data(&ui, &pool_clone, &session_clone, &rev_id);
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
        app_logic.on_cargar_cedulas_from_revision(move || {
            let Some(ui) = ui_weak.upgrade() else { return; };
            let app_data = ui.global::<AppData>();
            let rev_id = app_data.get_selected_revision_id().to_string();
            if rev_id.trim().is_empty() {
                show_toast_message(&ui, "Seleccione una revisión para cargar cédulas de cuotas.");
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

                let need_load = {
                    let sess = cedulas_session_clone.lock().unwrap();
                    !rev_id.is_empty() && (sess.audit_res.is_none() || sess.current_revision_id.as_deref() != Some(&rev_id) || sess.files.is_empty())
                };
                if need_load {
                    sync_and_load_cedulas_view_data(&ui, &pool_clone, &cedulas_session_clone, &rev_id);
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

    // 55. Callbacks de Reporte Liquidaciones
    wire_reporte_liquidaciones_callbacks(&main_window, db_pool.clone(), reporte_session.clone());

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
                        crate::db::nominas_reporte::fetch_nominas_reporte(&mut client, None).ok()
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
                if let Err(e) = crate::updater::apply_update_now(&path) {
                    if let Some(ui) = ui_weak.upgrade() {
                        show_toast_message(&ui, &format!("Error al aplicar actualización: {}", e));
                    }
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
                id_empleado: Uuid::now_v7(),
                nombre_trabajador: format!("EMPLEADO TEST {:03}", i),
                rfc: format!("TEST{:04}0101", i),
                curp: format!("TEST{:04}0101HDF00", i),
                nss: format!("123456{:05}", i),
                pensionado: String::new(),
                registro_patronal: "Y54-10204-10-8".into(),
                mes_periodo: if i <= 100 { "enero".into() } else if i <= 200 { "febrero".into() } else { "marzo".into() },
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
    }
}


