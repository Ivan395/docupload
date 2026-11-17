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
use crate::db::DbPool;
use crate::{AppData, AppLogic, HistMesUi, HistPeriodoUi, MainWindow};

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

pub fn wire_empleados_callbacks(main_window: &MainWindow, db_pool: Option<Arc<DbPool>>) {
    let app_logic = main_window.global::<AppLogic>();

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
