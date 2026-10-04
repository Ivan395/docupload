use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::db::nominas_reporte::{
    fetch_all_nominas_for_stats, summarize_nominas,
};
use crate::db::DbPool;
use crate::nominas_view_logic::NominasSessionState;
use crate::rustfs::{sanitize_path_component, sanitize_relative_path};
use crate::show_toast_message;
use crate::{AppData, AppLogic, FileItemData, MainWindow, NominaRow};

/// Estado de la vista de carga de nóminas.
///
/// Réplica de la máquina de estados de la vista de cédulas:
///   0 = en blanco (solo interfaz de carga)
///   1 = archivos seleccionados, pendientes de procesar
///   2 = procesados y guardados en la base de datos
pub struct NominasUploadSessionState {
    /// Archivos seleccionados en esta sesión, pendientes de procesar.
    pub staged_paths: Vec<PathBuf>,
    /// Archivos ya procesados y guardados.
    pub saved_count: usize,
    pub current_page: usize,
    pub page_size: usize,
    pub search_query: String,
}

impl Default for NominasUploadSessionState {
    fn default() -> Self {
        Self {
            staged_paths: Vec::new(),
            saved_count: 0,
            current_page: 0,
            page_size: 100,
            search_query: String::new(),
        }
    }
}

/// Formatea un tamaño de archivo en unidades legibles.
pub fn format_filesize(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// Determina si una extensión corresponde a un archivo comprimido.
fn is_archive_extension(ext: &str) -> bool {
    matches!(ext, "zip" | "rar")
}

/// Construye el modelo de un archivo seleccionado para la tabla de la vista.
fn build_file_item(id: usize, path: &PathBuf, rustfs_path: Option<String>) -> Option<FileItemData> {
    let metadata = std::fs::metadata(path).ok()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let filename = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("")
        .to_string();

    Some(FileItemData {
        id: id as i32,
        filename: filename.into(),
        filesize: format_filesize(metadata.len()).into(),
        extension: ext.clone().into(),
        is_archive: is_archive_extension(&ext),
        rustfs_path: rustfs_path.unwrap_or_default().into(),
    })
}

/// Añade rutas seleccionadas a la sesión y refresca la vista.
fn add_files_to_session(
    session: &Arc<Mutex<NominasUploadSessionState>>,
    paths: Vec<PathBuf>,
) {
    if let Ok(mut st) = session.lock() {
        for p in paths {
            if !st.staged_paths.iter().any(|existing| *existing == p) {
                st.staged_paths.push(p);
            }
        }
    }
}

/// Calcula el ancestro común más profundo de un conjunto de rutas.
fn common_ancestor(paths: &[PathBuf]) -> Option<PathBuf> {
    if paths.is_empty() {
        return None;
    }
    let mut anc = paths[0].parent()?.to_path_buf();
    for p in paths.iter().skip(1) {
        while !p.starts_with(&anc) {
            anc = anc.parent()?.to_path_buf();
        }
    }
    Some(anc)
}

/// Sube los archivos de nómina a RustFS conservando la ruta
/// relativa con la que se cargaron.
///
/// Destino: `correcciones/<empresa>/<registro patronal>/<revision>/
/// Nominas/<ruta_padre>/<sub_carpetas>/<nombre_archivo.xml>`
///
/// Devuelve `(subidos, errores)`.
fn upload_nominas_to_rustfs(
    paths: &[PathBuf],
    cliente: &str,
    rp: &str,
    revision: &str,
) -> (usize, usize) {
    if paths.is_empty() {
        return (0, 0);
    }

    let base = common_ancestor(paths);
    let mut ok = 0;
    let mut err = 0;

    for p in paths {
        // Ruta relativa al ancestro común; si no hay ancestro
        // (archivos sueltos de distintas unidades), se usa el
        // nombre del archivo.
        let rel = base
            .as_ref()
            .and_then(|b| p.strip_prefix(b).ok())
            .map(|r| r.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                p.file_name()
                    .map(|f| f.to_string_lossy().into_owned())
            })
            .unwrap_or_default();

        if rel.is_empty() {
            err += 1;
            continue;
        }

        match std::fs::read(p) {
            Ok(data) => {
                match crate::rustfs::upload_file_to_rustfs(
                    cliente, rp, revision, "Nominas", &rel, &data,
                ) {
                    Ok(_) => ok += 1,
                    Err(e) => {
                        log::warn!("Error subiendo {} a RustFS: {}", rel, e);
                        err += 1;
                    }
                }
            }
            Err(e) => {
                log::warn!("Error leyendo {} para RustFS: {}", p.display(), e);
                err += 1;
            }
        }
    }

    (ok, err)
}

/// Recalcula la tabla de archivos y el estado de la vista a partir de la sesión.
pub fn refresh_nominas_upload_state(ui: &MainWindow, session: &NominasUploadSessionState) {
    let d = ui.global::<AppData>();

    let staged_paths = session.staged_paths.clone();

    // Calcular el ancestro común para obtener rutas relativas
    let common_base = common_ancestor(&staged_paths);

    // Obtener componentes de la ruta RustFS desde AppData
    let rustfs_cliente = d.get_selected_cliente_nombre().to_string();
    let rustfs_rp = d.get_selected_rp_registro().to_string();
    let rustfs_revision = d.get_selected_revision_nombre().to_string();

    let items: Vec<FileItemData> = staged_paths
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let rustfs_path = common_base
                .as_ref()
                .and_then(|base| p.strip_prefix(base).ok())
                .map(|rel| {
                    let clean_c = sanitize_path_component(&rustfs_cliente);
                    let clean_rp = sanitize_path_component(&rustfs_rp);
                    let clean_rev = sanitize_path_component(&rustfs_revision);
                    let clean_rel = sanitize_relative_path(&rel.to_string_lossy());
                    format!("correcciones/{}/{}/{}/Nominas/{}", clean_c, clean_rp, clean_rev, clean_rel)
                });
            build_file_item(i, p, rustfs_path)
        })
        .collect();

    let count = items.len();
    let total_bytes: u64 = session
        .staged_paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();

    d.set_nominas_view_files(ModelRc::new(VecModel::from(items)));
    d.set_nominas_view_files_count(count as i32);
    d.set_nominas_view_total_size_str(format_filesize(total_bytes).into());

    // Estado 1 cuando hay archivos pendientes; se conserva el estado 2 si ya
    // hay nóminas guardadas y el usuario no ha seleccionado archivos nuevos.
    if count > 0 && d.get_nominas_view_state() != 2 {
        d.set_nominas_view_state(1);
        d.set_nominas_view_status_banner_text(
            "Archivos listos para procesar y guardar".into(),
        );
        d.set_nominas_view_extracted_count_str("0 Nóminas Extraídas".into());
    }

    if count == 0 && d.get_nominas_view_state() == 1 {
        d.set_nominas_view_state(0);
        d.set_nominas_view_status_banner_text("Sin archivos en memoria temporal".into());
    }
}

/// Carga las nóminas desde la BD y publica el estado 2 con las estadísticas.
pub fn load_nominas_upload_stats(
    ui: &MainWindow,
    pool: &DbPool,
    session: &Arc<Mutex<NominasUploadSessionState>>,
) -> Result<(), String> {
    let mut client = pool
        .get()
        .map_err(|e| format!("Error de conexión a la base de datos: {}", e))?;

    let rows = fetch_all_nominas_for_stats(&mut client, None)?;

    let d = ui.global::<AppData>();

    if rows.is_empty() {
        d.set_nominas_view_state(0);
        return Ok(());
    }

    let s = summarize_nominas(&rows);

    // Rango de periodos detectado (del más antiguo al más reciente).
    let mut periodos: Vec<String> = rows.iter().map(|r| r.periodo_str.clone()).collect();
    periodos.sort();
    periodos.dedup();
    let rango = match periodos.len() {
        0 => "-".to_string(),
        1 => periodos[0].clone(),
        n => format!("{} – {}", periodos[0], periodos[n - 1]),
    };

    d.set_nominas_view_periodo_rango_str(rango.into());
    d.set_nominas_view_periodo_sub_str(
        format!("{} periodo(s) distinto(s)", periodos.len()).into(),
    );
    d.set_nominas_view_total_trabajadores_str(s.total_empleados.to_string().into());
    d.set_nominas_view_total_dias_str(crate::nominas_view_logic::fmt_dias(s.dias_pagados).into());
    d.set_nominas_view_total_percepciones_str(
        crate::nominas_view_logic::fmt_money(s.total_percepciones).into(),
    );
    d.set_nominas_view_sbc_sub_str(
        format!("{} nómina(s) procesada(s)", s.total_nominas).into(),
    );
    d.set_nominas_view_extracted_count_str(
        format!("{} Nóminas Extraídas", s.total_nominas).into(),
    );

    // Detalle paginado: se reutiliza el mismo modelo de fila que la vista de
    // reporte para no duplicar la estructura de datos.
    let total = rows.len();
    let total_pages = total.div_ceil(session.lock().map(|s| s.page_size.max(1)).unwrap_or(100)).max(1);
    let current = {
        let st = session.lock().map_err(|_| "estado bloqueado".to_string())?;
        st.current_page.min(total_pages.saturating_sub(1))
    };
    let page_size = session.lock().map(|s| s.page_size).unwrap_or(100);
    let start = current * page_size;
    let end = (start + page_size).min(total);

    let slice: Vec<crate::db::nominas_reporte::NominaReporteRowDb> = if start < end {
        rows[start..end].to_vec()
    } else {
        Vec::new()
    };

    let detalle: Vec<NominaRow> = slice
        .iter()
        .enumerate()
        .map(|(i, r)| NominaRow {
            idx: (start + i + 1) as i32,
            id_nomina: r.id_nomina.to_string().into(),
            id_empleado: r.id_empleado.to_string().into(),
            nombre_trabajador: r.nombre_trabajador.to_uppercase().into(),
            nss: r.nss.clone().into(),
            rfc_curp: crate::nominas_view_logic::build_rfc_curp(&r.rfc, &r.curp).into(),
            registro_patronal: r.registro_patronal.clone().into(),
            periodo_str: r.periodo_str.clone().into(),
            semana_fiscal: r.semana_fiscal,
            semana_fiscal_str: crate::nominas_view_logic::fmt_semana(r.semana_fiscal).into(),
            fecha_inicial_pago_str: crate::nominas_view_logic::fmt_fecha(r.fecha_inicial_pago).into(),
            fecha_final_pago_str: crate::nominas_view_logic::fmt_fecha(r.fecha_final_pago).into(),
            fecha_pago_str: crate::nominas_view_logic::fmt_fecha(r.fecha_pago).into(),
            fecha_inicio_rel_str: crate::nominas_view_logic::fmt_fecha(r.fecha_inicio_relacion_laboral).into(),
            fecha_emision_str: crate::nominas_view_logic::fmt_fecha(r.fecha_emision_cfdi).into(),
            fecha_timbrado_str: crate::nominas_view_logic::fmt_fecha(r.fecha_timbrado).into(),
            num_dias_pagados_str: crate::nominas_view_logic::fmt_dias(r.num_dias_pagados).into(),
            dias_cedula_str: if r.dias_cedula > 0.0 {
                crate::nominas_view_logic::fmt_dias(r.dias_cedula).into()
            } else {
                "-".into()
            },
            dias_delta_str: "-".into(),
            dias_coinciden: false,
            sin_cedula: r.dias_cedula <= 0.0,
            total_percepciones_str: crate::nominas_view_logic::fmt_money(r.total_percepciones).into(),
            total_gravado_str: crate::nominas_view_logic::fmt_money(r.percepciones_gravadas).into(),
            total_exento_str: crate::nominas_view_logic::fmt_money(r.percepciones_exentas).into(),
            num_percepciones: r.num_percepciones,
            row_bg_color: slint::Color::from_rgb_u8(0xff, 0xff, 0xff),
            row_hover_color: slint::Color::from_rgb_u8(0xf1, 0xf5, 0xf9),
            percepcion_detalle_str: "-".into(),
            salario_diario_str: crate::nominas_view_logic::fmt_money(r.salario_diario_integrado).into(),
            tipo_nomina: r.tipo_nomina.clone().into(),
            periodicidad: r.periodicidad.clone().into(),
            total_dias_empleado: "-".into(),
            total_dias_meses: 0,
            meses: ModelRc::new(VecModel::from(Vec::new())),
            percepciones_cols: ModelRc::new(VecModel::from(Vec::new())),
        })
        .collect();

    d.set_nominas_view_detalle_list(ModelRc::new(VecModel::from(detalle)));
    d.set_nominas_view_detalle_total_count(total as i32);
    d.set_nominas_view_detalle_page_total(total_pages as i32);
    d.set_nominas_view_detalle_page_current((current + 1) as i32);
    d.set_nominas_view_detalle_page_start_row(if total == 0 { 0 } else { start + 1 } as i32);
    d.set_nominas_view_detalle_page_end_row(end as i32);

    d.set_nominas_view_state(2);

    if let Ok(mut st) = session.lock() {
        st.saved_count = total;
    }

    Ok(())
}

impl NominasUploadSessionState {
    /// Elimina un archivo por su índice visible.
    pub fn remove_at(&mut self, idx: usize) {
        if idx < self.staged_paths.len() {
            self.staged_paths.remove(idx);
        }
    }
}

/// Registra los callbacks de la vista de carga de nóminas.
pub fn wire_nominas_upload_callbacks(
    main_window: &MainWindow,
    db_pool: Option<Arc<DbPool>>,
    session: Arc<Mutex<NominasUploadSessionState>>,
    report_session: Arc<Mutex<NominasSessionState>>,
) {
    let app_logic = main_window.global::<AppLogic>();

    // Abrir la vista de carga desde "Cargar Nominas" de Registros
    // de Obras: es el punto de entrada, no requiere permiso previo.
    // Carga las estadísticas existentes de la BD si hay datos.
    let ui_weak = main_window.as_weak();
    let pool_oc = db_pool.clone();
    let session_oc = session.clone();
    app_logic.on_open_nominas_carga(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        let d = win.global::<AppData>();
        d.set_current_view(10);
        if let Some(ref pool) = pool_oc {
            let _ = load_nominas_upload_stats(&win, pool, &session_oc);
        }
    });

    // Seleccionar archivos
    let ui_weak = main_window.as_weak();
    let s1 = session.clone();
    app_logic.on_select_nominas_view_files(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        let files = rfd::FileDialog::new()
            .set_title("Seleccionar Nóminas (XML, ZIP o RAR)")
            .add_filter("Nóminas y Archivos Comprimidos", &["xml", "zip", "rar"])
            .add_filter("Archivos XML", &["xml"])
            .add_filter("Comprimidos ZIP / RAR", &["zip", "rar"])
            .pick_files();
        if let Some(paths) = files {
            add_files_to_session(&s1, paths);
            if let Ok(st) = s1.lock() {
                refresh_nominas_upload_state(&win, &st);
            }
        }
    });

    // Seleccionar carpeta
    let ui_weak = main_window.as_weak();
    let s2 = session.clone();
    app_logic.on_select_nominas_view_folder(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        let folder = rfd::FileDialog::new()
            .set_title("Seleccionar Carpeta con Nóminas (Búsqueda Recursiva)")
            .pick_folder();
        if let Some(folder_path) = folder {
            let mut found = Vec::new();
            for entry in walkdir::WalkDir::new(&folder_path).into_iter().flatten() {
                if entry.file_type().is_file() {
                    if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                        let e = ext.to_lowercase();
                        if e == "xml" || e == "zip" || e == "rar" {
                            found.push(entry.path().to_path_buf());
                        }
                    }
                }
            }
            add_files_to_session(&s2, found);
            if let Ok(st) = s2.lock() {
                refresh_nominas_upload_state(&win, &st);
            }
        }
    });

    // "Procesar y Guardar": parsea los XML, genera Excel, guarda en
    // PostgreSQL y sube los archivos originales a RustFS.
    let ui_weak = main_window.as_weak();
    let pool_pg = db_pool.clone();
    let s_pg = session.clone();
    app_logic.on_procesar_y_guardar_nominas(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        let d = win.global::<AppData>();

        let paths: Vec<PathBuf> = {
            let st = s_pg.lock().unwrap();
            st.staged_paths.clone()
        };
        if paths.is_empty() {
            show_toast_message(&win, "Por favor agrega al menos un archivo antes de procesar.");
            return;
        }
        if d.get_nominas_view_is_processing() {
            return;
        }
        d.set_nominas_view_is_processing(true);
        d.set_nominas_view_progress_fraction(0.02);
        d.set_nominas_view_progress_text("Iniciando procesamiento de nóminas...".into());

        // Contexto de RustFS: la ruta de destino es
        // correcciones/<empresa>/<registro patronal>/<revision>/Nominas/...
        let rustfs_cliente = d.get_selected_cliente_nombre().to_string();
        let rustfs_rp = d.get_selected_rp_registro().to_string();
        let rustfs_revision = d.get_selected_revision_nombre().to_string();
        let revision_id = d.get_selected_revision_id().to_string();
        let revision_id_parsed = Uuid::parse_str(&revision_id).expect("Revision ID inválido: debe seleccionar una revisión válida en la UI");

        let ui_w = ui_weak.clone();
        let session_w = s_pg.clone();
        let pool_w = pool_pg.clone();
        let revision_id_w = revision_id_parsed;

        std::thread::spawn(move || {
            let temp_dir_handle = tempfile::Builder::new()
                .prefix("docu_ffacd_nomview_")
                .tempdir();
            let temp_dir = match temp_dir_handle {
                Ok(ref d) => d.path().to_path_buf(),
                Err(_) => std::env::temp_dir(),
            };
            let temp_excel = temp_dir.join("nominas_procesadas_tmp.xlsx");

            let ui_for_cb = ui_w.clone();
            let progress_cb = move |current: usize, total: usize, msg: &str| {
                let frac = if total > 0 {
                    (current as f32 / total as f32).clamp(0.02, 0.98)
                } else {
                    0.05
                };
                let text = msg.to_string();
                let ui_clone = ui_for_cb.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_clone.upgrade() {
                        let app_data = ui.global::<AppData>();
                        app_data.set_nominas_view_progress_fraction(frac);
                        app_data.set_nominas_view_progress_text(text.into());
                    }
                });
            };

            let res = crate::nominas::processor::process_nominas(
                &paths, &temp_excel, &temp_dir, progress_cb, revision_id_w,
            );

            // Subida a RustFS de los archivos originales, conservando
            // la ruta relativa con la que se cargaron
            // (carpeta_padre/sub_carpeta/.../archivo.xml).
            let rustfs_ok;
            let rustfs_err;
            match res {
                Ok(ref r) if r.valid_count > 0 => {
                    let (ok, err) = upload_nominas_to_rustfs(
                        &paths,
                        &rustfs_cliente,
                        &rustfs_rp,
                        &rustfs_revision,
                    );
                    rustfs_ok = ok;
                    rustfs_err = err;
                }
                _ => {
                    rustfs_ok = 0;
                    rustfs_err = 0;
                }
            }

            // Copia en background a las tablas de reporte
            // (`reportes_nominas` + `reportes_detalles_nominas`):
            // empleados con obras creadas por el usuario y
            // fechas de nómina coincidentes con el registro
            // de obra.
            let copy_msg: Option<String> = match res {
                Ok(ref r) if r.valid_count > 0 => {
                    if let Some(ref pool) = pool_w {
                        if let Ok(mut client) = pool.get() {
                            match crate::db::nominas_reporte::copy_nominas_a_reportes(&mut client) {
                                Ok((n, d)) => Some(format!(
                                    "{} nómina(s) y {} detalle(s) copiados al reporte.",
                                    n, d
                                )),
                                Err(e) => {
                                    log::warn!("Error copiando nóminas a reportes: {}", e);
                                    Some(format!(
                                        "Error copiando al reporte: {}",
                                        e
                                    ))
                                }
                            }
                        } else {
                            Some("Sin conexión para copiar al reporte.".to_string())
                        }
                    } else {
                        Some("Base de datos no disponible para el reporte.".to_string())
                    }
                }
                _ => None,
            };

            let ui_final = ui_w.clone();
            let session_final = session_w.clone();
            let pool_final = pool_w.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_final.upgrade() else { return };
                let app_data = ui.global::<AppData>();
                app_data.set_nominas_view_is_processing(false);
                app_data.set_nominas_view_progress_fraction(0.0);
                app_data.set_nominas_view_progress_text("".into());

                match res {
                    Ok(r) => {
                        // Los archivos ya fueron procesados y guardados en BD;
                        // se retiran de la sesión y se pasa al estado 2.
                        {
                            let mut st = session_final.lock().unwrap();
                            st.staged_paths.clear();
                            st.saved_count = r.valid_count;
                            st.current_page = 0;
                        }
                        if r.valid_count > 0 {
                            app_data.set_nominas_access_allowed(true);
                        }
                        if let Some(ref pool) = pool_final {
                            if let Err(e) =
                                load_nominas_upload_stats(&ui, pool, &session_final)
                            {
                                log::warn!("Error cargando estadísticas de nóminas: {}", e);
                            }
                        }
                        let mut msg = if rustfs_err > 0 {
                            format!(
                                "Procesamiento completado: {} nóminas válidas, {} omitidas o inválidas. RustFS: {} subido(s), {} error(es).",
                                r.valid_count, r.invalid_count, rustfs_ok, rustfs_err
                            )
                        } else if rustfs_ok > 0 {
                            format!(
                                "Procesamiento completado: {} nóminas válidas, {} omitidas o inválidas. {} archivo(s) en RustFS.",
                                r.valid_count, r.invalid_count, rustfs_ok
                            )
                        } else {
                            format!(
                                "Procesamiento completado: {} nóminas válidas, {} omitidas o inválidas.",
                                r.valid_count, r.invalid_count
                            )
                        };
                        if let Some(ref extra) = copy_msg {
                            msg.push(' ');
                            msg.push_str(extra);
                        }
                        show_toast_message(&ui, &msg);
                    }
                    Err(e) => {
                        show_toast_message(&ui, &format!("Error al procesar nóminas: {}", e));
                    }
                }
            });
        });
    });

    // Eliminar un archivo
    let ui_weak = main_window.as_weak();
    let s3 = session.clone();
    app_logic.on_remove_nomina_view_file(move |idx| {
        let Some(win) = ui_weak.upgrade() else { return };
        if let Ok(mut st) = s3.lock() {
            st.remove_at(idx.max(0) as usize);
            refresh_nominas_upload_state(&win, &st);
        }
    });

    // "Realizar Reporte" → lleva a la vista de reporte (vista 9).
    // El reporte ya muestra únicamente las nóminas copiadas
    // en `reportes_nominas` (empleados con obras creadas por
    // el usuario y fechas coincidentes).
    let ui_weak = main_window.as_weak();
    let pool_rp = db_pool.clone();
    let rep_sess = report_session.clone();
    app_logic.on_realizar_reporte_nominas(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        let d = win.global::<AppData>();
        d.set_nominas_access_allowed(true);
        d.set_current_view(9);
        if let Some(ref pool) = pool_rp {
            let _ = crate::nominas_view_logic::sync_and_load_nominas_data(
                &win, pool, &rep_sess, None,
            );
        }
    });

    // Recargar
    let ui_weak = main_window.as_weak();
    let pool_rec = db_pool.clone();
    let s5 = session.clone();
    app_logic.on_recargar_nominas_view(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        if let Some(ref pool) = pool_rec {
            let _ = load_nominas_upload_stats(&win, pool, &s5);
        }
    });

    // Buscador de detalle
    let ui_weak = main_window.as_weak();
    let pool_s = db_pool.clone();
    let s6 = session.clone();
    app_logic.on_nominas_view_filtrar_busqueda(move |txt| {
        let Some(win) = ui_weak.upgrade() else { return };
        if let Ok(mut st) = s6.lock() {
            st.search_query = txt.to_string();
            st.current_page = 0;
        }
        if let Some(ref pool) = pool_s {
            let _ = load_nominas_upload_stats(&win, pool, &s6);
        }
    });

    // Cambiar de página en el detalle
    let ui_weak = main_window.as_weak();
    let pool_p = db_pool.clone();
    let s7 = session.clone();
    app_logic.on_nominas_view_cambiar_pagina(move |page| {
        let Some(win) = ui_weak.upgrade() else { return };
        if let Ok(mut st) = s7.lock() {
            st.current_page = (page.max(1) - 1) as usize;
        }
        if let Some(ref pool) = pool_p {
            let _ = load_nominas_upload_stats(&win, pool, &s7);
        }
    });

    // "Borrar Todo": abre el modal de confirmación
    let ui_weak = main_window.as_weak();
    app_logic.on_solicitar_borrar_todas_nominas(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        win.global::<AppData>().set_show_delete_all_nominas_modal(true);
    });

    // Cancelar borrado
    let ui_weak = main_window.as_weak();
    app_logic.on_cancelar_borrar_todas_nominas(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        win.global::<AppData>().set_show_delete_all_nominas_modal(false);
    });

    // Confirmar borrado total de nóminas en BD
    let ui_weak = main_window.as_weak();
    let pool_del = db_pool.clone();
    let s_del = session.clone();
    app_logic.on_confirmar_borrar_todas_nominas(move || {
        let Some(win) = ui_weak.upgrade() else { return };
        let d = win.global::<AppData>();
        d.set_show_delete_all_nominas_modal(false);

        // Contexto de RustFS para mover los archivos a Trash
        let rf_cliente = d.get_selected_cliente_nombre().to_string();
        let rf_rp = d.get_selected_rp_registro().to_string();
        let rf_revision = d.get_selected_revision_nombre().to_string();

        show_toast_message(&win, "Eliminando todas las nóminas de la base de datos...");

        let ui_w = ui_weak.clone();
        let session_w = s_del.clone();
        let pool_w = pool_del.clone();

        std::thread::spawn(move || {
            let mut deleted: u64 = 0;
            let mut error_msg: Option<String> = None;

            if let Some(ref pool) = pool_w {
                if let Ok(mut client) = pool.get() {
                    match crate::db::delete_all_nominas(&mut client) {
                        Ok(n) => {
                            deleted = n;
                            log::info!("Borradas {} filas de nóminas", n);
                        }
                        Err(e) => {
                            log::error!("Error al borrar nóminas: {}", e);
                            error_msg = Some(format!("Error al borrar en base de datos: {}", e));
                        }
                    }
                }
            }

            // Mover los archivos de RustFS a Trash (best-effort)
            if error_msg.is_none() {
                if let Err(e) = crate::rustfs::move_all_files_to_trash(
                    &rf_cliente, &rf_rp, &rf_revision, "Nominas",
                ) {
                    log::warn!("No se pudieron mover archivos a Trash en RustFS: {}", e);
                }
            }

            let ui_final = ui_w.clone();
            let session_final = session_w.clone();
            let pool_final = pool_w.clone();
            let del_count = deleted;
            let err_msg = error_msg;
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_final.upgrade() else { return };
                {
                    let mut st = session_final.lock().unwrap();
                    st.staged_paths.clear();
                    st.saved_count = 0;
                    st.current_page = 0;
                }
                if let Some(ref pool) = pool_final {
                    let _ = load_nominas_upload_stats(&ui, pool, &session_final);
                }
                match err_msg {
                    Some(e) => show_toast_message(&ui, &e),
                    None => show_toast_message(
                        &ui,
                        &format!("Se borraron {} registro(s) de nóminas de la base de datos.", del_count),
                    ),
                }
            });
        });
    });
}