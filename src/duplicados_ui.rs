//! Modal de posibles empleados duplicados (`ui/duplicados_modal.slint`) y aviso tras las cargas.

use std::sync::{Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, ModelRc, VecModel};
use uuid::Uuid;

use crate::db::duplicados_empleados::{self as dup, ParDuplicado};
use crate::db::DbPool;
use crate::{AppData, AppLogic, MainWindow, ParDuplicadoUi};

/// Pares mostrados (mismo orden que el modelo de la UI).
static PARES: Mutex<Vec<ParDuplicado>> = Mutex::new(Vec::new());

fn a_ui(p: &ParDuplicado) -> ParDuplicadoUi {
    ParDuplicadoUi {
        a_id: p.a.id.to_string().into(),
        a_nombre: p.a.nombre.as_str().into(),
        a_nss: p.a.nss.as_str().into(),
        a_rfc: p.a.rfc.as_str().into(),
        a_curp: p.a.curp.as_str().into(),
        a_liq: p.a.liquidaciones as i32,
        a_nom: p.a.nominas as i32,
        b_id: p.b.id.to_string().into(),
        b_nombre: p.b.nombre.as_str().into(),
        b_nss: p.b.nss.as_str().into(),
        b_rfc: p.b.rfc.as_str().into(),
        b_curp: p.b.curp.as_str().into(),
        b_liq: p.b.liquidaciones as i32,
        b_nom: p.b.nominas as i32,
        motivo: p.motivo.as_str().into(),
    }
}

fn mostrar(ui: &MainWindow, pares: Vec<ParDuplicado>) {
    let filas: Vec<ParDuplicadoUi> = pares.iter().map(a_ui).collect();
    let n = pares.len() as i32;
    *PARES.lock().unwrap_or_else(|e| e.into_inner()) = pares;
    let app = ui.global::<AppData>();
    app.set_duplicados_items(ModelRc::new(VecModel::from(filas)));
    app.set_duplicados_pendientes(n);
    app.set_duplicados_confirmar_idx(-1);
}

/// Recarga la lista en segundo plano.
fn recargar(ui: &MainWindow, pool: Arc<DbPool>, id_cliente: Uuid) {
    ui.global::<AppData>().set_duplicados_cargando(true);
    let w = ui.as_weak();
    thread::spawn(move || {
        let res = pool
            .get()
            .map_err(|e| format!("Error de conexión: {}", e))
            .and_then(|mut c| dup::listar_posibles_duplicados(&mut *c, id_cliente));
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = w.upgrade() else { return };
            ui.global::<AppData>().set_duplicados_cargando(false);
            match res {
                Ok(pares) => mostrar(&ui, pares),
                Err(e) => crate::form_errors::mostrar_error_general(&ui, "No se pudieron consultar los duplicados", &e),
            }
        });
    });
}

/// Tras guardar una carga: si el cliente de la revisión tiene posibles duplicados, muestra el aviso.
pub fn revisar_tras_carga(ui: slint::Weak<MainWindow>, pool: Arc<DbPool>, id_revision: Uuid) {
    thread::spawn(move || {
        let res = pool.get().map_err(|e| e.to_string()).and_then(|mut c| {
            let Some(id_cliente) = dup::cliente_de_revision(&mut *c, id_revision)? else {
                return Ok(None);
            };
            let pares = dup::listar_posibles_duplicados(&mut *c, id_cliente)?;
            let nombre: String = c
                .query_opt("SELECT nombre FROM public.clientes WHERE id = $1", &[&id_cliente])
                .ok()
                .flatten()
                .map(|r| r.get(0))
                .unwrap_or_default();
            Ok(Some((id_cliente, nombre, pares)))
        });
        match res {
            Ok(Some((id_cliente, nombre, pares))) if !pares.is_empty() => {
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui.upgrade() else { return };
                    let app = ui.global::<AppData>();
                    app.set_duplicados_cliente_id(id_cliente.to_string().into());
                    app.set_duplicados_cliente_nombre(nombre.into());
                    mostrar(&ui, pares);
                    app.set_duplicados_aviso_visible(true);
                });
            }
            Ok(_) => {}
            Err(e) => log::warn!("No se pudieron revisar duplicados tras la carga: {}", e),
        }
    });
}

pub fn wire_callbacks(main_window: &MainWindow, db_pool: Option<Arc<DbPool>>) {
    let app_logic = main_window.global::<AppLogic>();

    {
        let w = main_window.as_weak();
        let pool = db_pool.clone();
        app_logic.on_duplicados_abrir(move |id_cliente| {
            let Some(ui) = w.upgrade() else { return };
            let app = ui.global::<AppData>();
            let id_txt = if id_cliente.is_empty() { app.get_selected_cliente_id().to_string() } else { id_cliente.to_string() };
            let Ok(id) = Uuid::parse_str(&id_txt) else {
                crate::show_toast_message(&ui, "Selecciona un cliente para revisar sus empleados duplicados.");
                return;
            };
            if id_cliente.is_empty() {
                app.set_duplicados_cliente_nombre(app.get_selected_cliente_nombre());
            }
            app.set_duplicados_cliente_id(id_txt.into());
            app.set_duplicados_items(ModelRc::new(VecModel::from(Vec::<ParDuplicadoUi>::new())));
            app.set_show_duplicados_modal(true);
            match pool.clone() {
                Some(p) => recargar(&ui, p, id),
                None => crate::form_errors::mostrar_error_general(&ui, "Sin conexión con la base de datos", "No hay conexión con la base de datos."),
            }
        });
    }

    {
        let w = main_window.as_weak();
        app_logic.on_duplicados_cerrar(move || {
            let Some(ui) = w.upgrade() else { return };
            let app = ui.global::<AppData>();
            app.set_show_duplicados_modal(false);
            app.set_duplicados_confirmar_idx(-1);
        });
    }

    {
        let w = main_window.as_weak();
        app_logic.on_duplicados_pedir_fusion(move |idx, conservar_a| {
            let Some(ui) = w.upgrade() else { return };
            let app = ui.global::<AppData>();
            app.set_duplicados_confirmar_idx(idx);
            app.set_duplicados_confirmar_conservar_a(conservar_a);
        });
    }

    {
        let w = main_window.as_weak();
        app_logic.on_duplicados_cancelar_fusion(move || {
            if let Some(ui) = w.upgrade() {
                ui.global::<AppData>().set_duplicados_confirmar_idx(-1);
            }
        });
    }

    {
        let w = main_window.as_weak();
        let pool = db_pool.clone();
        app_logic.on_duplicados_confirmar_fusion(move || {
            let Some(ui) = w.upgrade() else { return };
            let app = ui.global::<AppData>();
            let idx = app.get_duplicados_confirmar_idx();
            let conservar_a = app.get_duplicados_confirmar_conservar_a();
            let par = usize::try_from(idx).ok().and_then(|i| PARES.lock().unwrap_or_else(|e| e.into_inner()).get(i).cloned());
            let (Some(par), Some(pool)) = (par, pool.clone()) else { return };
            let Ok(id_cliente) = Uuid::parse_str(&app.get_duplicados_cliente_id()) else { return };
            let (destino, origen) = if conservar_a { (par.a.clone(), par.b.clone()) } else { (par.b.clone(), par.a.clone()) };
            app.set_duplicados_cargando(true);
            let w2 = ui.as_weak();
            thread::spawn(move || {
                let motivo = format!(
                    "Fusión de empleado duplicado {} (NSS {}) en {} (NSS {}) desde la app",
                    origen.nombre, origen.nss, destino.nombre, destino.nss
                );
                let res = pool
                    .get()
                    .map_err(|e| format!("Error de conexión: {}", e))
                    .and_then(|mut c| dup::fusionar_empleados(&mut c, origen.id, destino.id, &motivo));
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w2.upgrade() else { return };
                    let app = ui.global::<AppData>();
                    app.set_duplicados_cargando(false);
                    app.set_duplicados_confirmar_idx(-1);
                    match res {
                        Ok(r) => {
                            crate::show_toast_message(
                                &ui,
                                &format!(
                                    "Empleados unidos en «{}»: {} liquidación(es) reasignadas, {} unificadas y {} nómina(s) movidas.",
                                    destino.nombre,
                                    r.liquidaciones_reasignadas,
                                    r.liquidaciones_unificadas,
                                    r.nominas_reasignadas
                                ),
                            );
                            // Refrescar el padrón si está mostrando a este cliente
                            if ui.global::<AppData>().get_selected_cliente_id() == id_cliente.to_string().as_str() {
                                crate::load_empleados_ui(&ui, &pool, &id_cliente.to_string());
                            }
                            recargar(&ui, pool, id_cliente);
                        }
                        Err(e) => crate::form_errors::mostrar_error_general(&ui, "No se pudieron unir los empleados", &e),
                    }
                });
            });
        });
    }

    {
        let w = main_window.as_weak();
        let pool = db_pool.clone();
        app_logic.on_duplicados_descartar(move |idx| {
            let Some(ui) = w.upgrade() else { return };
            let app = ui.global::<AppData>();
            let par = usize::try_from(idx).ok().and_then(|i| PARES.lock().unwrap_or_else(|e| e.into_inner()).get(i).cloned());
            let (Some(par), Some(pool)) = (par, pool.clone()) else { return };
            let Ok(id_cliente) = Uuid::parse_str(&app.get_duplicados_cliente_id()) else { return };
            app.set_duplicados_cargando(true);
            let w2 = ui.as_weak();
            thread::spawn(move || {
                let res = pool
                    .get()
                    .map_err(|e| format!("Error de conexión: {}", e))
                    .and_then(|mut c| dup::descartar_par(&mut *c, id_cliente, par.a.id, par.b.id));
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w2.upgrade() else { return };
                    ui.global::<AppData>().set_duplicados_cargando(false);
                    match res {
                        Ok(()) => recargar(&ui, pool, id_cliente),
                        Err(e) => crate::form_errors::mostrar_error_general(&ui, "No se pudo guardar la decisión", &e),
                    }
                });
            });
        });
    }
}
