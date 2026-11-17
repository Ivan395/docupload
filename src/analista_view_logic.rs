//! Selector de analista de la sesión (ADD-05, D-6 a). Callbacks del modal `AnalistaModal` y del
//! indicador de la barra superior. Las consultas a BD se hacen en hilos; las actualizaciones de
//! UI vuelven al hilo de la interfaz con `invoke_from_event_loop`.

use std::sync::Arc;
use std::thread;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::db::analistas::{
    analista_actual, crear_o_obtener_analista, fijar_analista_actual, guardar_preferencia,
    leer_preferencia, listar_analistas, olvidar_versiones, Analista,
};
use crate::db::DbPool;
use crate::{show_toast_message, AppData, AppLogic, MainWindow};

/// Pura: devuelve el analista de `lista` cuyo nombre coincide con la preferencia guardada,
/// ignorando espacios sobrantes y mayúsculas/minúsculas.
pub fn coincide_preferencia(preferencia: &str, lista: &[Analista]) -> Option<Analista> {
    let buscado = preferencia.trim().to_lowercase();
    if buscado.is_empty() {
        return None;
    }
    lista
        .iter()
        .find(|a| a.nombre.trim().to_lowercase() == buscado)
        .cloned()
}

/// Copia los nombres de la lista a `AppData.analistas_lista` (debe llamarse en el hilo de UI).
fn set_lista(app: &AppData, lista: &[Analista]) {
    let nombres: Vec<SharedString> = lista.iter().map(|a| a.nombre.as_str().into()).collect();
    app.set_analistas_lista(ModelRc::new(VecModel::from(nombres)));
}

/// Lista los analistas en un hilo y entrega el resultado al hilo de UI.
fn listar_en_hilo<F>(pool: Arc<DbPool>, ui_weak: slint::Weak<MainWindow>, al_terminar: F)
where
    F: FnOnce(&MainWindow, Result<Vec<Analista>, String>) + Send + 'static,
{
    thread::spawn(move || {
        let res = pool
            .get()
            .map_err(|e| format!("Error de conexión: {e}"))
            .and_then(|mut client| listar_analistas(&mut *client));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_weak.upgrade() {
                al_terminar(&ui, res);
            }
        });
    });
}

pub fn wire_analista_callbacks(main_window: &MainWindow, db_pool: Option<Arc<DbPool>>) {
    let app_logic = main_window.global::<AppLogic>();

    // Arranque: restaura el último analista recordado o abre el selector.
    if let Some(pool) = db_pool.clone() {
        listar_en_hilo(pool, main_window.as_weak(), |ui, res| match res {
            Ok(lista) => {
                let app = ui.global::<AppData>();
                let preferido = leer_preferencia().and_then(|p| coincide_preferencia(&p, &lista));
                match preferido {
                    Some(a) => {
                        app.set_analista_actual_nombre(a.nombre.as_str().into());
                        fijar_analista_actual(Some(a));
                    }
                    None => {
                        set_lista(&app, &lista);
                        app.set_show_analista_modal(true);
                    }
                }
            }
            Err(e) => log::error!("Analistas al arrancar: {}", e),
        });
    }

    // Abrir el selector: recarga la lista y muestra el modal.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        app_logic.on_analista_abrir_selector(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(pool) = pool.clone() else {
                show_toast_message(&ui, "Sin conexión a la base de datos.");
                return;
            };
            listar_en_hilo(pool, ui_weak.clone(), |ui, res| match res {
                Ok(lista) => {
                    let app = ui.global::<AppData>();
                    set_lista(&app, &lista);
                    app.set_show_analista_modal(true);
                }
                Err(e) => show_toast_message(ui, &e),
            });
        });
    }

    // Elegir (o crear) el analista de la sesión.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        app_logic.on_analista_elegir(move |nombre| {
            let nombre = nombre.to_string();
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(pool) = pool.clone() else {
                show_toast_message(&ui, "Sin conexión a la base de datos.");
                return;
            };
            let ui_weak = ui_weak.clone();
            thread::spawn(move || {
                let res = pool
                    .get()
                    .map_err(|e| format!("Error de conexión: {e}"))
                    .and_then(|mut client| crear_o_obtener_analista(&mut *client, &nombre));
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak.upgrade() else { return };
                    match res {
                        Ok(a) => {
                            fijar_analista_actual(Some(a.clone()));
                            // Las versiones vistas eran de la sesión anterior.
                            olvidar_versiones();
                            if let Err(e) = guardar_preferencia(&a.nombre) {
                                log::error!("Preferencia de analista: {}", e);
                            }
                            let app = ui.global::<AppData>();
                            app.set_analista_actual_nombre(a.nombre.as_str().into());
                            app.set_analista_nuevo_nombre("".into());
                            app.set_show_analista_modal(false);
                            show_toast_message(&ui, &format!("Analista en sesión: {}", a.nombre));
                        }
                        Err(e) => show_toast_message(&ui, &e),
                    }
                });
            });
        });
    }

    // Cerrar el selector: solo si ya hay analista en sesión.
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_analista_cerrar_selector(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if analista_actual().is_some() {
                ui.global::<AppData>().set_show_analista_modal(false);
            } else {
                show_toast_message(
                    &ui,
                    "Elige o captura tu nombre para registrar la autoría de tus cambios.",
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn analista(n: u128, nombre: &str) -> Analista {
        Analista {
            id: Uuid::from_u128(n),
            nombre: nombre.to_string(),
        }
    }

    #[test]
    fn analista_coincide_preferencia_ignora_mayusculas_y_espacios() {
        let lista = vec![analista(1, "ANA LOPEZ"), analista(2, "Luis Pérez")];
        let hallado = coincide_preferencia("  ana lopez \n", &lista);
        assert_eq!(hallado, Some(analista(1, "ANA LOPEZ")));
        assert_eq!(
            coincide_preferencia("LUIS PÉREZ", &lista),
            Some(analista(2, "Luis Pérez"))
        );
    }

    #[test]
    fn analista_sin_coincidencia_o_preferencia_vacia_es_none() {
        let lista = vec![analista(1, "ANA LOPEZ")];
        assert_eq!(coincide_preferencia("OTRO", &lista), None);
        assert_eq!(coincide_preferencia("   ", &lista), None);
        assert_eq!(coincide_preferencia("ANA", &[]), None);
    }
}
