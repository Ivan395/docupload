//! Vista Empleados: histórico empleado × obra (D-10 b). Callbacks del modal `HistorialEmpleadoModal`.
//! La consulta se hace en un hilo; los modelos de UI (que contienen `SharedString`, no `Send`) se
//! construyen en el hilo de la interfaz a partir de los datos de `historial_empleados`.

use std::sync::Arc;
use std::thread;

use chrono::NaiveDate;
use slint::{ComponentHandle, ModelRc, VecModel};
use uuid::Uuid;

use crate::db::historial_empleados::{
    compactar_periodos, fetch_tramos_historial, meses_obra, MesObra, PeriodoObra,
};
use crate::db::equivalencias_nss::{
    agregar_equivalencia, eliminar_equivalencia, listar_equivalencias, EquivalenciaNss,
};
use crate::db::DbPool;
use crate::{AppData, AppLogic, EquivalenciaNssUi, HistMesUi, HistPeriodoUi, MainWindow};

// ---------------------------------------------------------------------------
// Funciones puras de presentación
// ---------------------------------------------------------------------------

/// Fecha en formato DD/MM/AAAA.
pub fn fmt_fecha(fecha: NaiveDate) -> String {
    fecha.format("%d/%m/%Y").to_string()
}

/// Rango "dd/mm – dd/mm" o "—" si no hay rango efectivo.
pub fn fmt_rango(rango: Option<(NaiveDate, NaiveDate)>) -> String {
    match rango {
        Some((ini, fin)) => format!("{} – {}", ini.format("%d/%m"), fin.format("%d/%m")),
        None => "—".to_string(),
    }
}

/// Mes en formato MM/AAAA.
pub fn fmt_mes(anio: i16, mes: i16) -> String {
    format!("{:02}/{}", mes, anio)
}

/// Fila de la tabla de periodos.
pub fn periodo_ui(p: &PeriodoObra) -> HistPeriodoUi {
    HistPeriodoUi {
        revision: p.revision.as_str().into(),
        obra: p.numero.as_str().into(),
        inicio: fmt_fecha(p.inicio).into(),
        fin: fmt_fecha(p.fin).into(),
        dias_calendario: p.dias_calendario,
        dias_obra: p.dias_obra,
    }
}

/// Fila del detalle mensual.
pub fn mes_ui(m: &MesObra) -> HistMesUi {
    HistMesUi {
        revision: m.revision.as_str().into(),
        mes: fmt_mes(m.anio, m.mes).into(),
        obra: m.numero.as_str().into(),
        dias_obra: m.dias_obra,
        rango: fmt_rango(m.rango).into(),
        estado: m.estado.as_str().into(),
    }
}

// ---------------------------------------------------------------------------
// Carga y cableado
// ---------------------------------------------------------------------------

/// Lanza la consulta del histórico del empleado indicado en `AppData` (hilo + event loop).
fn lanzar_carga(ui: &MainWindow, pool: &Arc<DbPool>) {
    let app = ui.global::<AppData>();
    let id_empleado = match Uuid::parse_str(app.get_historial_empleado_id().trim()) {
        Ok(id) => id,
        Err(_) => {
            log::error!("Histórico de empleado: ID inválido");
            app.set_historial_cargando(false);
            return;
        }
    };
    // Alcance: solo la revisión activa (si el ID no parsea, se muestran todas).
    let id_revision = if app.get_historial_solo_revision() {
        Uuid::parse_str(app.get_selected_revision_id().trim()).ok()
    } else {
        None
    };

    app.set_historial_cargando(true);
    let ui_weak = ui.as_weak();
    let pool_bg = pool.clone();
    thread::spawn(move || {
        let res: Result<(Vec<PeriodoObra>, Vec<MesObra>), String> = pool_bg
            .get()
            .map_err(|e| format!("Error de conexión: {e}"))
            .and_then(|mut client| {
                let tramos = fetch_tramos_historial(&mut *client, id_empleado, id_revision)?;
                Ok((compactar_periodos(&tramos), meses_obra(&tramos)))
            });
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let app = ui.global::<AppData>();
            app.set_historial_cargando(false);
            match res {
                Ok((periodos, meses)) => {
                    let p: Vec<HistPeriodoUi> = periodos.iter().map(periodo_ui).collect();
                    let m: Vec<HistMesUi> = meses.iter().map(mes_ui).collect();
                    app.set_historial_periodos(ModelRc::new(VecModel::from(p)));
                    app.set_historial_meses(ModelRc::new(VecModel::from(m)));
                }
                Err(e) => {
                    log::error!("Error consultando histórico de empleado: {}", e);
                    app.set_historial_periodos(ModelRc::new(VecModel::default()));
                    app.set_historial_meses(ModelRc::new(VecModel::default()));
                }
            }
        });
    });
}

fn limpiar_historial(app: &AppData) {
    app.set_historial_periodos(ModelRc::new(VecModel::<HistPeriodoUi>::default()));
    app.set_historial_meses(ModelRc::new(VecModel::<HistMesUi>::default()));
}

// ---------------------------------------------------------------------------
// NSS alternos (equivalencias) del empleado en edición
// ---------------------------------------------------------------------------

/// Fila de la lista de NSS alternos.
pub fn equivalencia_ui(e: &EquivalenciaNss) -> EquivalenciaNssUi {
    EquivalenciaNssUi {
        id: e.id.to_string().into(),
        nss: e.nss_alterno.as_str().into(),
        motivo: e.motivo.as_str().into(),
        creada: e.creada.as_str().into(),
    }
}

/// Vacía la sección de NSS alternos (lista, campos y mensaje).
pub fn limpiar_equivalencias(app: &AppData) {
    app.set_emp_equivalencias(ModelRc::new(VecModel::<EquivalenciaNssUi>::default()));
    app.set_emp_equiv_nuevo_nss("".into());
    app.set_emp_equiv_nuevo_motivo("".into());
    app.set_emp_equiv_msg("".into());
    app.set_emp_equiv_msg_error(false);
    app.set_emp_equiv_ocupado(false);
}

/// Ejecuta `op` sobre la BD en un hilo y, de vuelta en la interfaz, recarga la lista del empleado
/// `id_empleado`. Si mientras tanto se abrió otro empleado, el resultado se descarta.
/// `op` devuelve el mensaje de éxito (vacío = sin mensaje) o el error a mostrar.
fn ejecutar_equivalencias<F>(ui: &MainWindow, pool: &Arc<DbPool>, id_empleado: Uuid, op: F)
where
    F: FnOnce(&mut crate::db::PooledClient) -> Result<String, String> + Send + 'static,
{
    let app = ui.global::<AppData>();
    app.set_emp_equiv_ocupado(true);
    let ui_weak = ui.as_weak();
    let pool_bg = pool.clone();
    thread::spawn(move || {
        let res: Result<(Result<String, String>, Vec<EquivalenciaNss>), String> = pool_bg
            .get()
            .map_err(|e| format!("Error de conexión: {e}"))
            .and_then(|mut client| {
                let resultado = op(&mut client);
                // La lista se recarga aunque la operación falle, para mostrar el estado real.
                let lista = listar_equivalencias(&mut *client, id_empleado)?;
                Ok((resultado, lista))
            });
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let app = ui.global::<AppData>();
            if app.get_emp_edit_id().as_str() != id_empleado.to_string() {
                return;
            }
            app.set_emp_equiv_ocupado(false);
            match res {
                Ok((resultado, lista)) => {
                    let filas: Vec<EquivalenciaNssUi> = lista.iter().map(equivalencia_ui).collect();
                    app.set_emp_equivalencias(ModelRc::new(VecModel::from(filas)));
                    match resultado {
                        Ok(msg) => {
                            if !msg.is_empty() {
                                app.set_emp_equiv_nuevo_nss("".into());
                                app.set_emp_equiv_nuevo_motivo("".into());
                            }
                            app.set_emp_equiv_msg(msg.into());
                            app.set_emp_equiv_msg_error(false);
                        }
                        Err(e) => {
                            app.set_emp_equiv_msg(e.into());
                            app.set_emp_equiv_msg_error(true);
                        }
                    }
                }
                Err(e) => {
                    log::error!("Equivalencias de NSS: {}", e);
                    app.set_emp_equiv_msg(e.into());
                    app.set_emp_equiv_msg_error(true);
                }
            }
        });
    });
}

/// Carga los NSS alternos del empleado abierto en el panel de edición.
pub fn cargar_equivalencias(ui: &MainWindow, pool: &Arc<DbPool>) {
    let app = ui.global::<AppData>();
    limpiar_equivalencias(&app);
    if app.get_emp_edit_is_new() {
        return;
    }
    let Ok(id_empleado) = Uuid::parse_str(app.get_emp_edit_id().trim()) else { return };
    ejecutar_equivalencias(ui, pool, id_empleado, |_| Ok(String::new()));
}

pub fn wire_empleados_callbacks(main_window: &MainWindow, db_pool: Option<Arc<DbPool>>) {
    let app_logic = main_window.global::<AppLogic>();

    // Agregar NSS alterno al empleado en edición.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        app_logic.on_equivalencia_nss_agregar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let app = ui.global::<AppData>();
            if app.get_emp_equiv_ocupado() || app.get_emp_equiv_nuevo_nss().trim().is_empty() {
                return;
            }
            let (Some(pool), Ok(id_empleado)) = (pool.as_ref(), Uuid::parse_str(app.get_emp_edit_id().trim())) else {
                app.set_emp_equiv_msg("Sin conexión a la base de datos.".into());
                app.set_emp_equiv_msg_error(true);
                return;
            };
            let nss = app.get_emp_equiv_nuevo_nss().to_string();
            let motivo = app.get_emp_equiv_nuevo_motivo().to_string();
            ejecutar_equivalencias(&ui, pool, id_empleado, move |client| {
                let nss = agregar_equivalencia(&mut **client, id_empleado, &nss, &motivo)?;
                log::info!("Equivalencia de NSS registrada: {} -> empleado {}", nss, id_empleado);
                Ok(format!("NSS {} registrado como alterno.", nss))
            });
        });
    }

    // Quitar un NSS alterno.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        app_logic.on_equivalencia_nss_eliminar(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let app = ui.global::<AppData>();
            if app.get_emp_equiv_ocupado() {
                return;
            }
            let (Some(pool), Ok(id_empleado), Ok(id_eq)) = (
                pool.as_ref(),
                Uuid::parse_str(app.get_emp_edit_id().trim()),
                Uuid::parse_str(id.trim()),
            ) else {
                return;
            };
            ejecutar_equivalencias(&ui, pool, id_empleado, move |client| {
                eliminar_equivalencia(&mut **client, id_eq)?;
                log::info!("Equivalencia de NSS {} quitada del empleado {}", id_eq, id_empleado);
                Ok("NSS alterno quitado.".to_string())
            });
        });
    }

    // Abrir histórico de un empleado.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        app_logic.on_historial_empleado_abrir(move |id, nombre| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let app = ui.global::<AppData>();
            app.set_historial_empleado_id(id);
            app.set_historial_empleado_nombre(nombre);
            limpiar_historial(&app);
            app.set_show_historial_empleado(true);
            if let Some(ref pool) = pool {
                lanzar_carga(&ui, pool);
            } else {
                app.set_historial_cargando(false);
            }
        });
    }

    // Cerrar histórico.
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_historial_empleado_cerrar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let app = ui.global::<AppData>();
            app.set_show_historial_empleado(false);
            app.set_historial_cargando(false);
            limpiar_historial(&app);
        });
    }

    // Alternar alcance: todas las revisiones / solo la revisión activa.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        app_logic.on_historial_empleado_alternar_alcance(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let app = ui.global::<AppData>();
            app.set_historial_solo_revision(!app.get_historial_solo_revision());
            limpiar_historial(&app);
            if let Some(ref pool) = pool {
                lanzar_carga(&ui, pool);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(anio: i32, mes: u32, dia: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(anio, mes, dia).expect("fecha de prueba")
    }

    #[test]
    fn fmt_fecha_formato_dmy() {
        assert_eq!(fmt_fecha(d(2026, 3, 5)), "05/03/2026");
    }

    #[test]
    fn fmt_rango_con_y_sin_datos() {
        assert_eq!(fmt_rango(Some((d(2026, 3, 1), d(2026, 3, 15)))), "01/03 – 15/03");
        assert_eq!(fmt_rango(None), "—");
    }

    #[test]
    fn fmt_mes_con_cero_a_la_izquierda() {
        assert_eq!(fmt_mes(2026, 3), "03/2026");
        assert_eq!(fmt_mes(2025, 12), "12/2025");
    }

    #[test]
    fn periodo_ui_copia_campos_y_formatea_fechas() {
        let p = PeriodoObra {
            id_revision: Uuid::from_u128(1),
            revision: "Rev 1".to_string(),
            numero: "OBRA-7".to_string(),
            inicio: d(2026, 1, 1),
            fin: d(2026, 1, 31),
            dias_calendario: 31,
            dias_obra: 22,
        };
        let ui = periodo_ui(&p);
        assert_eq!(ui.revision.as_str(), "Rev 1");
        assert_eq!(ui.obra.as_str(), "OBRA-7");
        assert_eq!(ui.inicio.as_str(), "01/01/2026");
        assert_eq!(ui.fin.as_str(), "31/01/2026");
        assert_eq!(ui.dias_calendario, 31);
        assert_eq!(ui.dias_obra, 22);
    }

    #[test]
    fn mes_ui_sin_rango_muestra_guion() {
        let m = MesObra {
            revision: "Rev 1".to_string(),
            anio: 2026,
            mes: 2,
            numero: "OBRA-7".to_string(),
            dias_obra: 0,
            rango: None,
            estado: "ACTIVO".to_string(),
        };
        let ui = mes_ui(&m);
        assert_eq!(ui.mes.as_str(), "02/2026");
        assert_eq!(ui.rango.as_str(), "—");
        assert_eq!(ui.estado.as_str(), "ACTIVO");
    }
}
