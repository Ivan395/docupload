#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod archives;
pub mod cedulas;
pub mod nominas;
pub mod sat;

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
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
        })
        .collect();

    app_data.set_cedulas_files(ModelRc::new(VecModel::from(models)));
}

fn show_toast_message(ui: &MainWindow, message: &str) {
    let app_data = ui.global::<AppData>();
    app_data.set_toast_message(message.into());
    app_data.set_show_toast(true);

    let ui_weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_secs(3), move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<AppData>().set_show_toast(false);
        }
    });
}

fn open_folder_in_os(path: &Path) {
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

fn spawn_nominas_worker(
    ui_weak: slint::Weak<MainWindow>,
    nominas_paths: Vec<PathBuf>,
    last_nom: Arc<Mutex<Option<PathBuf>>>,
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

    let ui_w = ui_weak.clone();
    thread::spawn(move || {
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

        let res = nominas::processor::process_nominas(&nominas_paths, &nominas_out_path, &temp_dir, progress_cb);

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
                            .filter(|it| it.category != "NÓMINA")
                            .collect();

                        for inv in outcome.invalid_items {
                            current_unproc.push(UnprocessedItemData {
                                category: "NÓMINA".into(),
                                filename: inv.filename.into(),
                                uuid: inv.uuid.into(),
                                reason: inv.reason.into(),
                                is_sat_error: inv.is_sat_error,
                            });
                        }
                        app_data.set_unprocessed_items(ModelRc::new(VecModel::from(current_unproc)));

                        show_toast_message(&ui, &format!("¡Nóminas procesadas: {} válidas!", outcome.valid_count));
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
                            .filter(|it| it.category != "CÉDULA")
                            .collect();

                        for inv in outcome.invalid_items {
                            current_unproc.push(UnprocessedItemData {
                                category: "CÉDULA".into(),
                                filename: inv.filename.into(),
                                uuid: "".into(),
                                reason: inv.reason.into(),
                                is_sat_error: false,
                            });
                        }
                        app_data.set_unprocessed_items(ModelRc::new(VecModel::from(current_unproc)));

                        show_toast_message(&ui, &format!("¡Cédulas procesadas: {} válidas!", outcome.valid_count));
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

fn main() -> Result<(), slint::PlatformError> {
    let _ = slint::set_xdg_app_id("docu_ffacd");
    let main_window = MainWindow::new()?;

    let nominas_list = Rc::new(RefCell::new(Vec::<SelectedFileEntry>::new()));
    let cedulas_list = Rc::new(RefCell::new(Vec::<SelectedFileEntry>::new()));

    // Store generated Excel file paths for direct download/saving
    let last_nominas_excel = Arc::new(Mutex::new(Option::<PathBuf>::None));
    let last_cedulas_excel = Arc::new(Mutex::new(Option::<PathBuf>::None));
    let last_nominas_download_dir = Arc::new(Mutex::new(Option::<PathBuf>::None));
    let last_cedulas_download_dir = Arc::new(Mutex::new(Option::<PathBuf>::None));

    let app_logic = main_window.global::<AppLogic>();

    // 1. Select Nominas Files
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        app_logic.on_select_nominas_files(move || {
            let files = rfd::FileDialog::new()
                .set_title("Seleccionar Nóminas (XML, ZIP o RAR)")
                .add_filter("Nóminas y Archivos Comprimidos", &["xml", "zip", "rar"])
                .add_filter("Archivos XML", &["xml"])
                .add_filter("Comprimidos ZIP / RAR", &["zip", "rar"])
                .pick_files();

            if let Some(paths) = files {
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

    // 2. Select Cédulas Files
    {
        let ui_weak = main_window.as_weak();
        let cedulas_list_clone = cedulas_list.clone();
        app_logic.on_select_cedulas_files(move || {
            let files = rfd::FileDialog::new()
                .set_title("Seleccionar Cédulas SUA (PDF, ZIP o RAR)")
                .add_filter("Cédulas SUA", &["pdf", "zip", "rar"])
                .add_filter("Archivos PDF", &["pdf"])
                .add_filter("Comprimidos ZIP / RAR", &["zip", "rar"])
                .pick_files();

            if let Some(paths) = files {
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

    // 3. Remove single file from Nóminas
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        app_logic.on_remove_nomina_file(move |file_id| {
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
        app_logic.on_process_nominas(move || {
            let paths: Vec<PathBuf> = nominas_list_clone
                .borrow()
                .iter()
                .map(|e| e.path.clone())
                .collect();
            spawn_nominas_worker(ui_weak.clone(), paths, last_nom_clone.clone());
        });
    }

    // 9. Process Cédulas (Independent)
    {
        let ui_weak = main_window.as_weak();
        let cedulas_list_clone = cedulas_list.clone();
        let last_ced_clone = last_cedulas_excel.clone();
        app_logic.on_process_cedulas(move || {
            let paths: Vec<PathBuf> = cedulas_list_clone
                .borrow()
                .iter()
                .map(|e| e.path.clone())
                .collect();
            spawn_cedulas_worker(ui_weak.clone(), paths, last_ced_clone.clone());
        });
    }

    // 10. Process Both / Trigger available
    {
        let ui_weak = main_window.as_weak();
        let nominas_list_clone = nominas_list.clone();
        let cedulas_list_clone = cedulas_list.clone();
        let last_nom_clone = last_nominas_excel.clone();
        let last_ced_clone = last_cedulas_excel.clone();

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

            if nom_paths.is_empty() && ced_paths.is_empty() {
                if let Some(ui) = ui_weak.upgrade() {
                    show_toast_message(&ui, "Por favor agrega al menos un archivo para procesar.");
                }
                return;
            }

            if !nom_paths.is_empty() {
                spawn_nominas_worker(ui_weak.clone(), nom_paths, last_nom_clone.clone());
            }
            if !ced_paths.is_empty() {
                spawn_cedulas_worker(ui_weak.clone(), ced_paths, last_ced_clone.clone());
            }
        });
    }

    // 11. Download/Save Nóminas Excel
    {
        let ui_weak = main_window.as_weak();
        let last_nom_clone = last_nominas_excel.clone();
        let last_nom_dir_clone = last_nominas_download_dir.clone();
        app_logic.on_download_nominas_excel(move || {
            let src_opt = last_nom_clone.lock().ok().and_then(|g| g.clone());
            if let Some(source) = src_opt {
                if let Some(dest) = rfd::FileDialog::new()
                    .set_title("Guardar Reporte de Nóminas Excel")
                    .set_file_name("reporte_nominas.xlsx")
                    .add_filter("Libro de Excel (*.xlsx)", &["xlsx"])
                    .save_file()
                {
                    if let Err(e) = fs::copy(&source, &dest) {
                        if let Some(ui) = ui_weak.upgrade() {
                            show_toast_message(&ui, &format!("Error al guardar Excel: {}", e));
                        }
                    } else {
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
            let src_opt = last_ced_clone.lock().ok().and_then(|g| g.clone());
            if let Some(source) = src_opt {
                if let Some(dest) = rfd::FileDialog::new()
                    .set_title("Guardar Reporte de Cédulas SUA Excel")
                    .set_file_name("reporte_liquidaciones.xlsx")
                    .add_filter("Libro de Excel (*.xlsx)", &["xlsx"])
                    .save_file()
                {
                    if let Err(e) = fs::copy(&source, &dest) {
                        if let Some(ui) = ui_weak.upgrade() {
                            show_toast_message(&ui, &format!("Error al guardar Excel: {}", e));
                        }
                    } else {
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

    main_window.run()
}
