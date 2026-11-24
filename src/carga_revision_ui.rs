//! Modal de revisión de discrepancias antes de cargar cédulas o nóminas
//! (`ui/carga_advertencias_modal.slint`).
//!
//! El hilo de trabajo de la carga llama a [`solicitar_decision`]: se muestra el modal y el hilo
//! queda en espera hasta que el usuario elige cargar todos los archivos, solo los seleccionados o
//! ninguno. Mientras tanto, los callbacks de la UI (hilo de eventos) actualizan el estado compartido.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Mutex;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::advertencias_carga::{AccionAdvertencia, AdvertenciaCarga, DecisionCarga};
use crate::{AppData, AppLogic, CargaAdvertenciaUi, CargaArchivoUi, MainWindow};

struct EstadoRevision {
    archivos: Vec<String>,
    advertencias: Vec<AdvertenciaCarga>,
    incluidos: BTreeSet<String>,
    /// id → acción elegida (0 dejar, 1 sugerido, 2 corregir)
    accion: HashMap<usize, i32>,
    /// id → texto capturado en "Corregir"
    correccion: HashMap<usize, String>,
    /// id → error de validación de la corrección
    error: HashMap<usize, String>,
    filtro: String,
    tx: mpsc::Sender<DecisionCarga>,
}

static ESTADO: Mutex<Option<EstadoRevision>> = Mutex::new(None);

thread_local! {
    // Modelo de tarjetas visible (solo en el hilo de la UI) para actualizar filas sin recrearlas
    static MODELO_ITEMS: RefCell<Option<Rc<VecModel<CargaAdvertenciaUi>>>> = const { RefCell::new(None) };
}

/// Muestra el modal y BLOQUEA el hilo actual hasta que el usuario decide.
/// Nunca debe llamarse desde el hilo de la UI.
pub fn solicitar_decision(
    ui: slint::Weak<MainWindow>,
    tipo: &str,
    titulo: &str,
    archivos: Vec<String>,
    advertencias: Vec<AdvertenciaCarga>,
) -> DecisionCarga {
    let (tx, rx) = mpsc::channel();
    {
        let mut guard = ESTADO.lock().unwrap_or_else(|e| e.into_inner());
        // Si quedó una revisión abierta (no debería), se cancela
        if let Some(prev) = guard.take() {
            let _ = prev.tx.send(DecisionCarga::Cancelar);
        }
        let incluidos: BTreeSet<String> = archivos.iter().cloned().collect();
        *guard = Some(EstadoRevision {
            archivos,
            advertencias,
            incluidos,
            accion: HashMap::new(),
            correccion: HashMap::new(),
            error: HashMap::new(),
            filtro: String::new(),
            tx,
        });
    }
    let tipo = tipo.to_string();
    let titulo = titulo.to_string();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui.upgrade() {
            let app = ui.global::<AppData>();
            app.set_carga_adv_tipo(tipo.into());
            app.set_carga_adv_titulo(titulo.into());
            renderizar(&ui);
            app.set_show_carga_adv_modal(true);
        }
    });
    rx.recv().unwrap_or(DecisionCarga::Cancelar)
}

fn con_estado<R>(f: impl FnOnce(&mut EstadoRevision) -> R) -> Option<R> {
    let mut guard = ESTADO.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_mut().map(f)
}

fn tarjeta(e: &EstadoRevision, a: &AdvertenciaCarga) -> CargaAdvertenciaUi {
    let archivos: Vec<&str> = a.archivos.iter().map(|s| s.as_str()).collect();
    CargaAdvertenciaUi {
        id: a.id as i32,
        campo: a.campo.etiqueta().into(),
        del_patron: a.campo.es_del_patron(),
        trabajador: a.trabajador.as_ref().map(|t| t.descripcion()).unwrap_or_default().into(),
        archivos: archivos.join(", ").into(),
        n_archivos: a.archivos.len() as i32,
        encontrado: a.encontrado.as_str().into(),
        sugerido: a.sugerido.clone().unwrap_or_default().into(),
        origen: a.origen_sugerido.as_str().into(),
        mensaje: a.mensaje.as_str().into(),
        grave: a.grave,
        accion: *e.accion.get(&a.id).unwrap_or(&0),
        correccion: e.correccion.get(&a.id).cloned().unwrap_or_default().into(),
        error: e.error.get(&a.id).cloned().unwrap_or_default().into(),
        archivo_incluido: a.archivos.iter().any(|f| e.incluidos.contains(f)),
        solo_decidir_archivo: a.solo_decidir_archivo,
    }
}

/// Reconstruye los modelos de archivos y tarjetas (al abrir, filtrar o marcar archivos).
fn renderizar(ui: &MainWindow) {
    let Some((archivos, items, n_inc, n_tot, n_adv, n_graves, filtro)) = con_estado(|e| {
        let archivos: Vec<CargaArchivoUi> = e
            .archivos
            .iter()
            .map(|f| {
                let advs: Vec<&AdvertenciaCarga> = e.advertencias.iter().filter(|a| a.archivos.contains(f)).collect();
                CargaArchivoUi {
                    nombre: f.as_str().into(),
                    incluir: e.incluidos.contains(f),
                    advertencias: advs.len() as i32,
                    graves: advs.iter().filter(|a| a.grave).count() as i32,
                }
            })
            .collect();
        let items: Vec<CargaAdvertenciaUi> = e
            .advertencias
            .iter()
            .filter(|a| e.filtro.is_empty() || a.archivos.contains(&e.filtro))
            .map(|a| tarjeta(e, a))
            .collect();
        (
            archivos,
            items,
            e.incluidos.len() as i32,
            e.archivos.len() as i32,
            e.advertencias.len() as i32,
            e.advertencias.iter().filter(|a| a.grave).count() as i32,
            e.filtro.clone(),
        )
    }) else {
        return;
    };
    let app = ui.global::<AppData>();
    app.set_carga_adv_archivos(ModelRc::new(VecModel::from(archivos)));
    let modelo = Rc::new(VecModel::from(items));
    MODELO_ITEMS.with(|m| *m.borrow_mut() = Some(modelo.clone()));
    app.set_carga_adv_items(ModelRc::from(modelo));
    app.set_carga_adv_n_incluidos(n_inc);
    app.set_carga_adv_n_total(n_tot);
    app.set_carga_adv_n_advertencias(n_adv);
    app.set_carga_adv_n_graves(n_graves);
    app.set_carga_adv_filtro(filtro.into());
}

/// Actualiza solo la tarjeta `id` (sin recrear la lista, para no perder el foco al escribir).
fn actualizar_tarjeta(id: usize, conservar_texto: bool) {
    let Some(nueva) = con_estado(|e| e.advertencias.iter().find(|a| a.id == id).map(|a| tarjeta(e, a))).flatten() else {
        return;
    };
    MODELO_ITEMS.with(|m| {
        if let Some(modelo) = m.borrow().as_ref() {
            for i in 0..modelo.row_count() {
                if let Some(fila) = modelo.row_data(i) {
                    if fila.id == id as i32 {
                        let mut nueva = nueva.clone();
                        if conservar_texto {
                            nueva.correccion = fila.correccion.clone();
                        }
                        modelo.set_row_data(i, nueva);
                        break;
                    }
                }
            }
        }
    });
}

fn cerrar(ui: &MainWindow) {
    let app = ui.global::<AppData>();
    app.set_show_carga_adv_modal(false);
    app.set_carga_adv_items(ModelRc::new(VecModel::from(Vec::<CargaAdvertenciaUi>::new())));
    app.set_carga_adv_archivos(ModelRc::new(VecModel::from(Vec::<CargaArchivoUi>::new())));
    MODELO_ITEMS.with(|m| *m.borrow_mut() = None);
}

/// Valida las correcciones de los archivos incluidos y arma la decisión final.
fn armar_decision(e: &mut EstadoRevision) -> Result<DecisionCarga, usize> {
    let mut acciones: HashMap<usize, AccionAdvertencia> = HashMap::new();
    let mut invalidas = 0;
    e.error.clear();
    for a in &e.advertencias {
        let incluida = a.archivos.iter().any(|f| e.incluidos.contains(f));
        match e.accion.get(&a.id).copied().unwrap_or(0) {
            1 if a.sugerido.is_some() => {
                acciones.insert(a.id, AccionAdvertencia::UsarSugerido);
            }
            2 => {
                let texto = e.correccion.get(&a.id).cloned().unwrap_or_default();
                match a.campo.validar_correccion(&texto) {
                    Ok(v) => {
                        acciones.insert(a.id, AccionAdvertencia::Corregir(v));
                    }
                    Err(err) => {
                        if incluida {
                            e.error.insert(a.id, err);
                            invalidas += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if invalidas > 0 {
        return Err(invalidas);
    }
    Ok(DecisionCarga::Cargar { incluidos: e.incluidos.clone(), acciones })
}

pub fn wire_callbacks(main_window: &MainWindow) {
    let app_logic = main_window.global::<AppLogic>();

    let w = main_window.as_weak();
    app_logic.on_carga_adv_toggle_archivo(move |nombre| {
        let Some(ui) = w.upgrade() else { return };
        con_estado(|e| {
            let n = nombre.to_string();
            if !e.incluidos.remove(&n) {
                e.incluidos.insert(n);
            }
        });
        renderizar(&ui);
    });

    let w = main_window.as_weak();
    app_logic.on_carga_adv_todos(move |incluir| {
        let Some(ui) = w.upgrade() else { return };
        con_estado(|e| {
            e.incluidos = if incluir { e.archivos.iter().cloned().collect() } else { BTreeSet::new() };
        });
        renderizar(&ui);
    });

    let w = main_window.as_weak();
    app_logic.on_carga_adv_filtrar(move |nombre| {
        let Some(ui) = w.upgrade() else { return };
        con_estado(|e| e.filtro = nombre.to_string());
        renderizar(&ui);
    });

    app_logic.on_carga_adv_accion(move |id, accion| {
        let id = id as usize;
        con_estado(|e| {
            e.accion.insert(id, accion);
            e.error.remove(&id);
            // Al elegir "Corregir" se precarga el dato tal como viene, para editar solo el error
            if accion == 2 && !e.correccion.contains_key(&id) {
                if let Some(a) = e.advertencias.iter().find(|a| a.id == id) {
                    e.correccion.insert(id, a.encontrado.clone());
                }
            }
        });
        actualizar_tarjeta(id, false);
    });

    app_logic.on_carga_adv_correccion(move |id, texto| {
        let id = id as usize;
        con_estado(|e| {
            e.correccion.insert(id, texto.to_string());
            let campo = e.advertencias.iter().find(|a| a.id == id).map(|a| a.campo);
            match campo.map(|c| c.validar_correccion(texto.as_str())) {
                Some(Err(err)) if !texto.trim().is_empty() => {
                    e.error.insert(id, err);
                }
                _ => {
                    e.error.remove(&id);
                }
            }
        });
        actualizar_tarjeta(id, true);
    });

    let w = main_window.as_weak();
    app_logic.on_carga_adv_accion_masiva(move |accion| {
        let Some(ui) = w.upgrade() else { return };
        con_estado(|e| {
            let visibles: Vec<(usize, bool)> = e
                .advertencias
                .iter()
                .filter(|a| e.filtro.is_empty() || a.archivos.contains(&e.filtro))
                .map(|a| (a.id, a.sugerido.is_some()))
                .collect();
            for (id, tiene_sugerido) in visibles {
                if accion == 1 && !tiene_sugerido {
                    continue;
                }
                e.accion.insert(id, accion);
                e.error.remove(&id);
            }
        });
        renderizar(&ui);
    });

    let w = main_window.as_weak();
    app_logic.on_carga_adv_confirmar(move |modo| {
        let Some(ui) = w.upgrade() else { return };
        let resultado = con_estado(|e| {
            if modo == 0 {
                e.incluidos = e.archivos.iter().cloned().collect();
            } else if e.incluidos.is_empty() {
                return Err("Selecciona al menos un archivo, o usa «No cargar ninguno».".to_string());
            }
            armar_decision(e).map_err(|n| {
                format!(
                    "Hay {} corrección(es) con un valor inválido (marcadas en rojo). Corrígelas, o elige «Dejar como viene» o «Usar sugerido».",
                    n
                )
            })
        });
        match resultado {
            Some(Ok(decision)) => {
                let estado = ESTADO.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(estado) = estado {
                    let _ = estado.tx.send(decision);
                }
                cerrar(&ui);
            }
            Some(Err(msg)) => {
                renderizar(&ui);
                crate::form_errors::mostrar_error_general(&ui, "Revisa antes de cargar", &msg);
            }
            None => cerrar(&ui),
        }
    });

    let w = main_window.as_weak();
    app_logic.on_carga_adv_cancelar(move || {
        let Some(ui) = w.upgrade() else { return };
        let estado = ESTADO.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(estado) = estado {
            let _ = estado.tx.send(DecisionCarga::Cancelar);
        }
        cerrar(&ui);
    });
}
