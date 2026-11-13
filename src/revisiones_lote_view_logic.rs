//! Modal de alta masiva de revisiones: cableado de callbacks, hilos de BD y RustFS, y mapeo de
//! filas a la UI. La lógica de interpretación y clasificación vive en `revisiones_lote_logic`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use chrono::NaiveDate;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use uuid::Uuid;

use crate::carga_masiva::{tabla_desde_archivo, tabla_desde_texto_pegado};
use crate::db::revisiones::{crear_revisiones_lote, fetch_revisiones_existentes, ResumenLoteRevisiones};
use crate::db::DbPool;
use crate::revisiones_lote_logic::{
    clasificar_revisiones, generar_por_prefijo, interpretar_tabla_revisiones, EstadoRevisionLote, RevisionLoteFila,
    RevisionPrevia,
};
use crate::{show_toast_message, AppData, AppLogic, MainWindow, RevisionLoteFilaUi};

/// Ancho fijo del consecutivo del generador por prefijo (REV01, REV02, ...).
const ANCHO_CONSECUTIVO: usize = 2;

/// Estado compartido del modal: filas clasificadas pendientes de aplicar.
#[derive(Default)]
struct EstadoLote {
    filas: Vec<RevisionPrevia>,
}

type Estado = Arc<Mutex<EstadoLote>>;

/// Etiqueta de estado para la lista del modal.
pub fn etiqueta_estado_revision(estado: EstadoRevisionLote) -> &'static str {
    match estado {
        EstadoRevisionLote::Valida => "✅ Válida",
        EstadoRevisionLote::Error => "⛔ Error",
        EstadoRevisionLote::NombreExiste => "🔁 Nombre ya existe",
        EstadoRevisionLote::FolioExiste => "🔁 Folio ya existe",
        EstadoRevisionLote::RepetidaEnLote => "🔁 Repetida en el lote",
    }
}

/// Resumen de la vista previa: cuántas son válidas, con error y ya existentes o repetidas.
pub fn texto_resumen_revisiones(filas: &[RevisionPrevia]) -> String {
    let contar = |pred: fn(EstadoRevisionLote) -> bool| filas.iter().filter(|p| pred(p.estado)).count();
    let validas = contar(|e| e == EstadoRevisionLote::Valida);
    let errores = contar(|e| e == EstadoRevisionLote::Error);
    let existentes = contar(|e| {
        matches!(
            e,
            EstadoRevisionLote::NombreExiste | EstadoRevisionLote::FolioExiste | EstadoRevisionLote::RepetidaEnLote
        )
    });
    format!("{validas} válidas, {errores} con error, {existentes} ya existen o se repiten")
}

fn texto_fecha(d: Option<NaiveDate>) -> String {
    d.map(|f| f.format("%d/%m/%Y").to_string()).unwrap_or_default()
}

fn texto_periodo(inicio: Option<NaiveDate>, fin: Option<NaiveDate>) -> String {
    if inicio.is_none() && fin.is_none() {
        return String::new();
    }
    format!("{} al {}", texto_fecha(inicio), texto_fecha(fin))
}

fn fila_ui(p: &RevisionPrevia) -> RevisionLoteFilaUi {
    let f = &p.fila;
    RevisionLoteFilaUi {
        renglon: f.renglon as i32,
        nombre: f.nombre.clone().into(),
        folio: f.folio.clone().unwrap_or_else(|| "S/F".to_string()).into(),
        periodo: texto_periodo(f.periodo_inicio, f.periodo_fin).into(),
        aprobacion: texto_fecha(f.fecha_aprobacion_imss).into(),
        estado: etiqueta_estado_revision(p.estado).into(),
        es_error: p.estado == EstadoRevisionLote::Error,
        errores: f.errores.join(" · ").into(),
        incluir: p.incluir,
        puede_incluir: p.estado == EstadoRevisionLote::Valida,
    }
}

/// Pinta la lista y el resumen del modal a partir del estado compartido.
fn mostrar_filas(ui: &MainWindow, filas: &[RevisionPrevia]) {
    let app = ui.global::<AppData>();
    let modelo: Vec<RevisionLoteFilaUi> = filas.iter().map(fila_ui).collect();
    app.set_revisiones_lote_filas(ModelRc::new(VecModel::from(modelo)));
    app.set_revisiones_lote_resumen(texto_resumen_revisiones(filas).into());
}

/// Clasifica las filas contra la BD del registro patronal seleccionado (en hilo) y muestra el resultado.
fn clasificar_en_hilo(
    ui: &MainWindow,
    pool: Option<Arc<DbPool>>,
    estado: Estado,
    filas: Vec<RevisionLoteFila>,
) {
    let app = ui.global::<AppData>();
    if app.get_revisiones_lote_procesando() {
        show_toast_message(ui, "Espera a que termine el proceso actual.");
        return;
    }
    let rp_id = app.get_selected_rp_id().to_string();
    if rp_id.trim().is_empty() {
        show_toast_message(ui, "Selecciona un registro patronal.");
        return;
    }
    let Some(pool) = pool else {
        show_toast_message(ui, "Sin conexión a la base de datos.");
        return;
    };
    let Ok(id_rp) = Uuid::parse_str(rp_id.trim()) else {
        show_toast_message(ui, "El registro patronal seleccionado no es válido.");
        return;
    };

    app.set_revisiones_lote_procesando(true);
    let ui_weak = ui.as_weak();
    thread::spawn(move || {
        let resultado = pool
            .get()
            .map_err(|e| format!("conexión a la base de datos: {e}"))
            .and_then(|mut client| {
                let existentes = fetch_revisiones_existentes(&mut *client, id_rp)?;
                Ok(clasificar_revisiones(filas, &existentes))
            });
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.global::<AppData>().set_revisiones_lote_procesando(false);
            match resultado {
                Ok(previas) => {
                    mostrar_filas(&ui, &previas);
                    if let Ok(mut e) = estado.lock() {
                        e.filas = previas;
                    }
                }
                Err(e) => {
                    log::error!("Alta masiva de revisiones: {e}");
                    show_toast_message(&ui, &format!("No se pudo validar el lote: {e}"));
                }
            }
        });
    });
}

/// Crea en hilo las revisiones marcadas, luego RustFS (en segundo plano) y recarga la UI.
fn aplicar_en_hilo(ui: &MainWindow, pool: Option<Arc<DbPool>>, holidays: Arc<RwLock<HashSet<NaiveDate>>>, estado: Estado) {
    let app = ui.global::<AppData>();
    if app.get_revisiones_lote_procesando() {
        show_toast_message(ui, "Espera a que termine el proceso actual.");
        return;
    }
    let filas: Vec<RevisionPrevia> = estado.lock().map(|e| e.filas.clone()).unwrap_or_default();
    if !filas.iter().any(|p| p.incluir) {
        show_toast_message(ui, "No hay revisiones marcadas para crear.");
        return;
    }
    let rp_id = app.get_selected_rp_id().to_string();
    let Ok(id_rp) = Uuid::parse_str(rp_id.trim()) else {
        show_toast_message(ui, "Selecciona un registro patronal.");
        return;
    };
    let Some(pool) = pool else {
        show_toast_message(ui, "Sin conexión a la base de datos.");
        return;
    };
    let cliente = app.get_selected_cliente_nombre().to_string();
    let rp_registro = app.get_selected_rp_registro().to_string();

    app.set_revisiones_lote_procesando(true);
    let ui_weak = ui.as_weak();
    let pool_ui = pool.clone();
    thread::spawn(move || {
        let resultado: Result<ResumenLoteRevisiones, String> = pool
            .get()
            .map_err(|e| format!("conexión a la base de datos: {e}"))
            .and_then(|mut client| {
                let inhabiles = holidays.read().map(|s| s.clone()).unwrap_or_default();
                crear_revisiones_lote(&mut *client, id_rp, &filas, &inhabiles)
            });

        if let Ok(resumen) = &resultado {
            let creadas = resumen.creadas.clone();
            // Estructura RustFS por revisión creada, en segundo plano; los errores solo van al log.
            thread::spawn(move || {
                for nombre in creadas {
                    if let Err(e) = crate::rustfs::ensure_revision_structure(&cliente, &rp_registro, &nombre) {
                        log::error!("Alta masiva de revisiones: estructura RustFS de '{nombre}': {e}");
                    }
                }
            });
        }

        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.global::<AppData>().set_revisiones_lote_procesando(false);
            match resultado {
                Ok(resumen) => {
                    ui.global::<AppData>().set_show_revisiones_lote_modal(false);
                    crate::load_revisiones_ui(&ui, &pool_ui, &rp_id);
                    crate::update_global_stats(&ui, &pool_ui);
                    show_toast_message(
                        &ui,
                        &format!(
                            "Alta masiva: {} revisión(es) creada(s), {} omitida(s).",
                            resumen.creadas.len(),
                            resumen.omitidas
                        ),
                    );
                }
                Err(e) => {
                    log::error!("Alta masiva de revisiones: {e}");
                    show_toast_message(
                        &ui,
                        &format!("Error en el alta masiva de revisiones: {e} (no se creó ninguna)"),
                    );
                }
            }
        });
    });
}

/// Interpreta el texto pegado y dispara la clasificación.
fn vista_previa_texto(ui: &MainWindow, pool: Option<Arc<DbPool>>, estado: Estado, texto: &str) {
    let tabla = tabla_desde_texto_pegado(texto);
    match interpretar_tabla_revisiones(&tabla) {
        Ok(filas) => clasificar_en_hilo(ui, pool, estado, filas),
        Err(e) => show_toast_message(ui, &e),
    }
}

/// Registra los callbacks del modal de alta masiva de revisiones.
pub fn wire_revisiones_lote_callbacks(
    main_window: &MainWindow,
    db_pool: Option<Arc<DbPool>>,
    holidays: Arc<RwLock<HashSet<NaiveDate>>>,
) {
    let app_logic = main_window.global::<AppLogic>();
    let estado: Estado = Arc::new(Mutex::new(EstadoLote::default()));

    // Abrir: modal limpio.
    {
        let ui_weak = main_window.as_weak();
        let estado = estado.clone();
        app_logic.on_revisiones_lote_abrir(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Ok(mut e) = estado.lock() {
                e.filas.clear();
            }
            let app = ui.global::<AppData>();
            app.set_revisiones_lote_texto(SharedString::default());
            app.set_revisiones_lote_filas(ModelRc::new(VecModel::from(Vec::<RevisionLoteFilaUi>::new())));
            app.set_revisiones_lote_resumen(SharedString::default());
            app.set_revisiones_lote_procesando(false);
            app.set_show_revisiones_lote_modal(true);
        });
    }

    // Cerrar: descarta la vista previa.
    {
        let ui_weak = main_window.as_weak();
        let estado = estado.clone();
        app_logic.on_revisiones_lote_cerrar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Ok(mut e) = estado.lock() {
                e.filas.clear();
            }
            ui.global::<AppData>().set_show_revisiones_lote_modal(false);
        });
    }

    // Vista previa del texto pegado.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let estado = estado.clone();
        app_logic.on_revisiones_lote_vista_previa(move |texto: SharedString| {
            let Some(ui) = ui_weak.upgrade() else { return };
            vista_previa_texto(&ui, pool.clone(), estado.clone(), texto.as_str());
        });
    }

    // Importar archivo (CSV o XLSX): diálogo nativo; la lectura corre en hilo.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let estado = estado.clone();
        app_logic.on_revisiones_lote_elegir_archivo(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(path) = rfd::FileDialog::new()
                .set_title("Seleccionar archivo de revisiones")
                .add_filter("CSV o Excel", &["csv", "xlsx"])
                .pick_file()
            else {
                return;
            };
            ui.global::<AppData>().set_revisiones_lote_procesando(true);
            let ui_weak = ui.as_weak();
            let pool = pool.clone();
            let estado = estado.clone();
            thread::spawn(move || {
                let filas = tabla_desde_archivo(&path).and_then(|t| interpretar_tabla_revisiones(&t));
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    ui.global::<AppData>().set_revisiones_lote_procesando(false);
                    match filas {
                        Ok(filas) => clasificar_en_hilo(&ui, pool, estado, filas),
                        Err(e) => show_toast_message(&ui, &e),
                    }
                });
            });
        });
    }

    // Generar por prefijo y consecutivo.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let estado = estado.clone();
        app_logic.on_revisiones_lote_generar(
            move |prefijo_nombre: SharedString, prefijo_folio: SharedString, desde: SharedString, cantidad: SharedString| {
                let Some(ui) = ui_weak.upgrade() else { return };
                let (Ok(desde), Ok(cantidad)) = (desde.trim().parse::<u32>(), cantidad.trim().parse::<u32>()) else {
                    show_toast_message(&ui, "Desde y cantidad deben ser números enteros.");
                    return;
                };
                match generar_por_prefijo(
                    prefijo_nombre.as_str(),
                    prefijo_folio.as_str(),
                    desde,
                    cantidad,
                    ANCHO_CONSECUTIVO,
                ) {
                    Ok(filas) => clasificar_en_hilo(&ui, pool.clone(), estado.clone(), filas),
                    Err(e) => show_toast_message(&ui, &e),
                }
            },
        );
    }

    // Alternar la casilla de una fila (solo si es válida).
    {
        let ui_weak = main_window.as_weak();
        let estado = estado.clone();
        app_logic.on_revisiones_lote_alternar(move |indice: i32| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Ok(idx) = usize::try_from(indice) else { return };
            if let Ok(mut e) = estado.lock() {
                if let Some(p) = e.filas.get_mut(idx) {
                    if p.estado == EstadoRevisionLote::Valida {
                        p.incluir = !p.incluir;
                    }
                }
                mostrar_filas(&ui, &e.filas);
            }
        });
    }

    // Aplicar: crea las revisiones marcadas.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let holidays = holidays.clone();
        let estado = estado.clone();
        app_logic.on_revisiones_lote_aplicar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            aplicar_en_hilo(&ui, pool.clone(), holidays.clone(), estado.clone());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn previa(estado: EstadoRevisionLote) -> RevisionPrevia {
        RevisionPrevia {
            fila: RevisionLoteFila {
                renglon: 1,
                nombre: "R".into(),
                folio: None,
                concepto: String::new(),
                estatus: String::new(),
                periodo_inicio: None,
                periodo_fin: None,
                fecha_aprobacion_imss: None,
                fecha_entrega_revision: None,
                errores: Vec::new(),
            },
            estado,
            incluir: estado == EstadoRevisionLote::Valida,
        }
    }

    #[test]
    fn revisiones_lote_etiquetas_de_estado() {
        assert_eq!(etiqueta_estado_revision(EstadoRevisionLote::Valida), "✅ Válida");
        assert_eq!(etiqueta_estado_revision(EstadoRevisionLote::Error), "⛔ Error");
        assert_eq!(etiqueta_estado_revision(EstadoRevisionLote::NombreExiste), "🔁 Nombre ya existe");
        assert_eq!(etiqueta_estado_revision(EstadoRevisionLote::FolioExiste), "🔁 Folio ya existe");
        assert_eq!(etiqueta_estado_revision(EstadoRevisionLote::RepetidaEnLote), "🔁 Repetida en el lote");
    }

    #[test]
    fn revisiones_lote_resumen_cuenta_estados() {
        let filas = vec![
            previa(EstadoRevisionLote::Valida),
            previa(EstadoRevisionLote::Valida),
            previa(EstadoRevisionLote::Error),
            previa(EstadoRevisionLote::NombreExiste),
            previa(EstadoRevisionLote::FolioExiste),
            previa(EstadoRevisionLote::RepetidaEnLote),
        ];
        assert_eq!(
            texto_resumen_revisiones(&filas),
            "2 válidas, 1 con error, 3 ya existen o se repiten"
        );
        assert_eq!(texto_resumen_revisiones(&[]), "0 válidas, 0 con error, 0 ya existen o se repiten");
    }

    #[test]
    fn revisiones_lote_texto_de_periodo_y_fecha() {
        let d = NaiveDate::from_ymd_opt(2025, 1, 31);
        assert_eq!(texto_periodo(None, None), "");
        assert_eq!(texto_periodo(NaiveDate::from_ymd_opt(2025, 1, 1), d), "01/01/2025 al 31/01/2025");
        assert_eq!(texto_fecha(None), "");
    }
}
