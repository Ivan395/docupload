//! Alta masiva de registros de obra (ADD-01) y adopción de obras detectadas en cédula (ADD-02):
//! callbacks del modal `ObrasLoteModal`. La lógica (interpretar, clasificar, aplicar) vive en
//! `obras_lote_logic` y `db::obras_ops`; aquí solo se orquesta la UI, el hilo de BD y el estado.

use std::sync::{Arc, Mutex};
use std::thread;

use chrono::NaiveDate;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use uuid::Uuid;

use crate::carga_masiva::{tabla_desde_archivo, tabla_desde_texto_pegado, TablaCruda};
use crate::db::obras_ops::{
    aplicar_lote_obras, fetch_obras_detectadas_cedula, fetch_obras_existentes_revision,
};
use crate::db::DbPool;
use crate::obras_lote_logic::{
    clasificar, interpretar_tabla_obras, resumen_previa, AccionFila, EstadoFila, FilaPrevia,
};
use crate::{
    load_obras_ui, mensaje_cambio_universo, show_toast_message, update_global_stats, AppData,
    AppLogic, MainWindow, ObraLoteFilaUi,
};

/// Filas de la vista previa en curso (índice = renglón de la lista en la UI).
type EstadoLote = Arc<Mutex<Vec<FilaPrevia>>>;

// ---------------------------------------------------------------------------
// Funciones puras de presentación
// ---------------------------------------------------------------------------

/// Etiqueta mostrada en la columna de estado de la vista previa.
pub fn etiqueta_estado(estado: EstadoFila) -> &'static str {
    match estado {
        EstadoFila::Valida => "✅ Válida",
        EstadoFila::SinMonto => "⚠ Sin monto",
        EstadoFila::Error => "⛔ Error",
        EstadoFila::ExisteUsuario => "🔁 Ya existe (usuario)",
        EstadoFila::ExisteCedula => "🔁 Existe en cédula",
        EstadoFila::RepetidaEnLote => "🔁 Repetida en el lote",
    }
}

/// Texto del resumen de la vista previa a partir de (crear, actualizar, adoptar, omitir, error, sin_monto).
pub fn texto_resumen(conteos: (usize, usize, usize, usize, usize, usize)) -> String {
    let (crear, actualizar, adoptar, omitir, error, sin_monto) = conteos;
    format!(
        "Crear: {crear} · Actualizar: {actualizar} · Adoptar: {adoptar} · Omitir: {omitir} · Con error: {error} · Sin monto: {sin_monto}"
    )
}

/// TSV (encabezado + una línea por obra) con las obras detectadas en cédula, para la vista previa.
pub fn tsv_obras_detectadas(obras: &[(String, Option<NaiveDate>, Option<NaiveDate>, i64)]) -> String {
    let fmt = |f: &Option<NaiveDate>| f.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default();
    let mut out = String::from("NUMERO\tINICIO\tFIN\n");
    for (numero, inicio, fin, _) in obras {
        out.push_str(&format!("{}\t{}\t{}\n", numero, fmt(inicio), fmt(fin)));
    }
    out
}

fn nombre_accion(accion: AccionFila) -> &'static str {
    match accion {
        AccionFila::Crear => "Crear",
        AccionFila::Omitir => "Omitir",
        AccionFila::Actualizar => "Actualizar",
        AccionFila::Adoptar => "Adoptar",
    }
}

fn accion_desde_nombre(nombre: &str) -> Option<AccionFila> {
    match nombre {
        "Crear" => Some(AccionFila::Crear),
        "Omitir" => Some(AccionFila::Omitir),
        "Actualizar" => Some(AccionFila::Actualizar),
        "Adoptar" => Some(AccionFila::Adoptar),
        _ => None,
    }
}

fn fila_ui(previa: &FilaPrevia) -> ObraLoteFilaUi {
    let fmt = |f: Option<NaiveDate>| f.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default();
    ObraLoteFilaUi {
        renglon: previa.fila.renglon as i32,
        numero: SharedString::from(previa.fila.numero.as_str()),
        inicio: SharedString::from(fmt(previa.fila.fecha_inicio)),
        fin: SharedString::from(fmt(previa.fila.fecha_fin)),
        monto: SharedString::from(
            previa.fila.monto.map(|m| format!("{:.2}", m)).unwrap_or_else(|| "—".to_string()),
        ),
        estado: SharedString::from(etiqueta_estado(previa.estado)),
        es_error: previa.estado == EstadoFila::Error,
        errores: SharedString::from(previa.fila.errores.join("; ")),
        accion: SharedString::from(nombre_accion(previa.accion)),
        acciones: ModelRc::new(VecModel::from(
            previa
                .acciones_posibles
                .iter()
                .map(|a| SharedString::from(nombre_accion(*a)))
                .collect::<Vec<_>>(),
        )),
    }
}

/// Pinta la lista y el resumen en la UI a partir de la vista previa.
fn pintar(ui: &MainWindow, previa: &[FilaPrevia]) {
    let app = ui.global::<AppData>();
    let filas: Vec<ObraLoteFilaUi> = previa.iter().map(fila_ui).collect();
    app.set_obras_lote_filas(ModelRc::new(VecModel::from(filas)));
    let resumen = if previa.is_empty() {
        "No se encontraron renglones de obra en el texto.".to_string()
    } else {
        texto_resumen(resumen_previa(previa))
    };
    app.set_obras_lote_resumen(SharedString::from(resumen));
}

fn limpiar_estado(estado: &EstadoLote) {
    if let Ok(mut e) = estado.lock() {
        e.clear();
    }
}

fn id_revision_seleccionada(ui: &MainWindow) -> Option<Uuid> {
    let texto = ui.global::<AppData>().get_selected_revision_id().to_string();
    match Uuid::parse_str(&texto) {
        Ok(id) => Some(id),
        Err(_) => {
            show_toast_message(ui, "Seleccione una revisión antes de continuar.");
            None
        }
    }
}

/// Clasifica el lote frente a las obras de la revisión (en hilo) y pinta la vista previa.
fn iniciar_vista_previa(
    ui: &MainWindow,
    pool: &Arc<DbPool>,
    estado: &EstadoLote,
    tabla: &TablaCruda,
) {
    let filas = match interpretar_tabla_obras(tabla) {
        Ok(f) => f,
        Err(e) => {
            show_toast_message(ui, &e);
            return;
        }
    };
    let Some(rev) = id_revision_seleccionada(ui) else { return };

    ui.global::<AppData>().set_obras_lote_procesando(true);
    ui.global::<AppData>().set_obras_lote_resumen("Clasificando renglones…".into());

    let ui_weak = ui.as_weak();
    let pool_bg = pool.clone();
    let estado_fin = estado.clone();
    thread::spawn(move || {
        let res: Result<Vec<FilaPrevia>, String> = pool_bg
            .get()
            .map_err(|e| format!("Error de conexión: {e}"))
            .and_then(|mut client| {
                let existentes = fetch_obras_existentes_revision(&mut *client, rev)?;
                Ok(clasificar(filas, &existentes))
            });
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.global::<AppData>().set_obras_lote_procesando(false);
            match res {
                Ok(previa) => {
                    pintar(&ui, &previa);
                    if let Ok(mut e) = estado_fin.lock() {
                        *e = previa;
                    }
                }
                Err(e) => {
                    ui.global::<AppData>().set_obras_lote_resumen("".into());
                    show_toast_message(&ui, &e);
                }
            }
        });
    });
}

/// Cambia la acción de una fila (solo si está permitida) y actualiza solo esa fila de la lista.
fn cambiar_accion(ui: &MainWindow, estado: &EstadoLote, indice: i32, accion: &str) {
    let Some(nueva) = accion_desde_nombre(accion) else { return };
    let Ok(mut e) = estado.lock() else { return };
    let Some(fila) = usize::try_from(indice).ok().and_then(|i| e.get_mut(i)) else { return };
    if !fila.acciones_posibles.contains(&nueva) {
        return;
    }
    fila.accion = nueva;

    let app = ui.global::<AppData>();
    let modelo = app.get_obras_lote_filas();
    if let Some(vm) = modelo.as_any().downcast_ref::<VecModel<ObraLoteFilaUi>>() {
        vm.set_row_data(indice as usize, fila_ui(fila));
    }
    app.set_obras_lote_resumen(SharedString::from(texto_resumen(resumen_previa(&e))));
}

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

pub fn wire_obras_lote_callbacks(main_window: &MainWindow, db_pool: Option<Arc<DbPool>>) {
    let app_logic = main_window.global::<AppLogic>();
    let estado: EstadoLote = Arc::new(Mutex::new(Vec::new()));

    // Abrir modal: estado limpio y texto vacío.
    {
        let ui_weak = main_window.as_weak();
        let estado = estado.clone();
        app_logic.on_obras_lote_abrir(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            limpiar_estado(&estado);
            let app = ui.global::<AppData>();
            app.set_obras_lote_texto("".into());
            app.set_obras_lote_filas(ModelRc::new(VecModel::<ObraLoteFilaUi>::default()));
            app.set_obras_lote_resumen("".into());
            app.set_obras_lote_procesando(false);
            app.set_show_obras_lote_modal(true);
        });
    }

    // Cerrar modal.
    {
        let ui_weak = main_window.as_weak();
        let estado = estado.clone();
        app_logic.on_obras_lote_cerrar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            limpiar_estado(&estado);
            ui.global::<AppData>().set_show_obras_lote_modal(false);
        });
    }

    // Vista previa desde el texto pegado.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let estado = estado.clone();
        app_logic.on_obras_lote_vista_previa(move |texto| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(ref pool) = pool else {
                show_toast_message(&ui, "Sin conexión a la base de datos.");
                return;
            };
            let tabla = tabla_desde_texto_pegado(&texto);
            iniciar_vista_previa(&ui, pool, &estado, &tabla);
        });
    }

    // Importar archivo .csv/.xlsx y mostrar su vista previa.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let estado = estado.clone();
        app_logic.on_obras_lote_elegir_archivo(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(ref pool) = pool else {
                show_toast_message(&ui, "Sin conexión a la base de datos.");
                return;
            };
            let Some(path) = rfd::FileDialog::new()
                .set_title("Seleccionar archivo de obras")
                .add_filter("CSV o Excel", &["csv", "xlsx"])
                .pick_file()
            else {
                return;
            };
            match tabla_desde_archivo(&path) {
                Ok(tabla) => iniciar_vista_previa(&ui, pool, &estado, &tabla),
                Err(e) => show_toast_message(&ui, &e),
            }
        });
    }

    // Cambiar la acción de una fila.
    {
        let ui_weak = main_window.as_weak();
        let estado = estado.clone();
        app_logic.on_obras_lote_cambiar_accion(move |indice, accion| {
            let Some(ui) = ui_weak.upgrade() else { return };
            cambiar_accion(&ui, &estado, indice, accion.as_str());
        });
    }

    // Aplicar el lote en una sola transacción.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let estado = estado.clone();
        app_logic.on_obras_lote_aplicar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let previa: Vec<FilaPrevia> = estado.lock().map(|e| e.clone()).unwrap_or_default();
            let a_aplicar = previa.iter().filter(|f| f.accion != AccionFila::Omitir).count();
            if a_aplicar == 0 {
                show_toast_message(&ui, "No hay obras por crear o actualizar.");
                return;
            }
            let Some(ref pool) = pool else {
                show_toast_message(&ui, "Sin conexión a la base de datos.");
                return;
            };
            let Some(rev) = id_revision_seleccionada(&ui) else { return };
            let rp_texto = ui.global::<AppData>().get_selected_rp_id().to_string();
            let Ok(rp) = Uuid::parse_str(&rp_texto) else {
                show_toast_message(&ui, "Seleccione un registro patronal antes de continuar.");
                return;
            };
            let sin_monto_creadas = previa
                .iter()
                .filter(|f| f.accion == AccionFila::Crear && f.fila.monto.is_none())
                .count();
            let rev_texto = rev.to_string();

            ui.global::<AppData>().set_obras_lote_procesando(true);
            let ui_weak2 = ui_weak.clone();
            let pool_bg = pool.clone();
            let pool_fin = pool.clone();
            let estado_fin = estado.clone();
            thread::spawn(move || {
                let res = pool_bg
                    .get()
                    .map_err(|e| format!("Error de conexión: {e}"))
                    .and_then(|mut client| aplicar_lote_obras(&mut client, rev, rp, &previa));
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak2.upgrade() else { return };
                    let app = ui.global::<AppData>();
                    app.set_obras_lote_procesando(false);
                    match res {
                        Ok(r) => {
                            app.set_show_obras_lote_modal(false);
                            limpiar_estado(&estado_fin);
                            load_obras_ui(&ui, &pool_fin, &rev_texto);
                            update_global_stats(&ui, &pool_fin);
                            let mut msg = format!(
                                "Alta masiva: {} creadas, {} actualizadas, {} adoptadas, {} omitidas.",
                                r.creadas, r.actualizadas, r.adoptadas, r.omitidas
                            );
                            if sin_monto_creadas > 0 {
                                msg.push_str(&format!(
                                    " {} obra(s) sin monto: no se calculará el 18%.",
                                    sin_monto_creadas
                                ));
                            }
                            let cambio = mensaje_cambio_universo(&r.cambio);
                            if !cambio.is_empty() {
                                msg.push(' ');
                                msg.push_str(&cambio);
                            }
                            show_toast_message(&ui, &msg);
                        }
                        Err(e) => show_toast_message(
                            &ui,
                            &format!(
                                "Error en el alta masiva: {e}. El lote completo se revirtió; no se aplicó ningún cambio."
                            ),
                        ),
                    }
                });
            });
        });
    }

    // ADD-02: obras detectadas en cédula -> TSV en el modal + vista previa (Existe en cédula).
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let estado = estado.clone();
        app_logic.on_obras_lote_adoptar_detectadas(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(ref pool) = pool else {
                show_toast_message(&ui, "Sin conexión a la base de datos.");
                return;
            };
            let Some(rev) = id_revision_seleccionada(&ui) else { return };

            let ui_weak2 = ui_weak.clone();
            let pool_bg = pool.clone();
            let pool_fin = pool.clone();
            let estado_fin = estado.clone();
            thread::spawn(move || {
                let res = pool_bg
                    .get()
                    .map_err(|e| format!("Error de conexión: {e}"))
                    .and_then(|mut client| fetch_obras_detectadas_cedula(&mut *client, rev));
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak2.upgrade() else { return };
                    match res {
                        Err(e) => show_toast_message(&ui, &e),
                        Ok(obras) if obras.is_empty() => {
                            show_toast_message(&ui, "No hay obras detectadas en cédulas para adoptar.");
                        }
                        Ok(obras) => {
                            let tsv = tsv_obras_detectadas(&obras);
                            limpiar_estado(&estado_fin);
                            let app = ui.global::<AppData>();
                            app.set_obras_lote_texto(SharedString::from(tsv.as_str()));
                            app.set_obras_lote_filas(ModelRc::new(VecModel::<ObraLoteFilaUi>::default()));
                            app.set_obras_lote_resumen("".into());
                            app.set_show_obras_lote_modal(true);
                            let tabla = tabla_desde_texto_pegado(&tsv);
                            iniciar_vista_previa(&ui, &pool_fin, &estado_fin, &tabla);
                        }
                    }
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etiquetas_de_estado() {
        assert_eq!(etiqueta_estado(EstadoFila::Valida), "✅ Válida");
        assert_eq!(etiqueta_estado(EstadoFila::SinMonto), "⚠ Sin monto");
        assert_eq!(etiqueta_estado(EstadoFila::Error), "⛔ Error");
        assert_eq!(etiqueta_estado(EstadoFila::ExisteUsuario), "🔁 Ya existe (usuario)");
        assert_eq!(etiqueta_estado(EstadoFila::ExisteCedula), "🔁 Existe en cédula");
        assert_eq!(etiqueta_estado(EstadoFila::RepetidaEnLote), "🔁 Repetida en el lote");
    }

    #[test]
    fn resumen_con_conteos() {
        assert_eq!(
            texto_resumen((2, 1, 0, 3, 1, 1)),
            "Crear: 2 · Actualizar: 1 · Adoptar: 0 · Omitir: 3 · Con error: 1 · Sin monto: 1"
        );
    }

    #[test]
    fn tsv_de_obras_detectadas() {
        let obras = vec![
            ("C1000001".to_string(), NaiveDate::from_ymd_opt(2025, 1, 5), NaiveDate::from_ymd_opt(2025, 12, 31), 3),
            ("C1000002".to_string(), None, None, 0),
        ];
        assert_eq!(
            tsv_obras_detectadas(&obras),
            "NUMERO\tINICIO\tFIN\nC1000001\t05/01/2025\t31/12/2025\nC1000002\t\t\n"
        );
    }

    #[test]
    fn tsv_detectadas_se_interpreta_como_lote() {
        let obras = vec![("C1000001".to_string(), NaiveDate::from_ymd_opt(2025, 1, 5), NaiveDate::from_ymd_opt(2025, 12, 31), 3)];
        let tabla = tabla_desde_texto_pegado(&tsv_obras_detectadas(&obras));
        let filas = interpretar_tabla_obras(&tabla).unwrap();
        assert_eq!(filas.len(), 1);
        assert_eq!(filas[0].numero, "C1000001");
        assert_eq!(filas[0].fecha_inicio, NaiveDate::from_ymd_opt(2025, 1, 5));
    }
}
