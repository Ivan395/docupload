//! Pestaña "Asignación por obra" del Reporte de Liquidaciones.
//! Responde "¿quién trabajó en la obra X?": tarjetas por obra (con KPIs de guarderías reutilizados)
//! y, al elegir una obra, sus empleado-mes asignados. Las funciones de cálculo son puras; los
//! callbacks de Slint ejecutan las operaciones de BD existentes y recargan el reporte.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use postgres::Client;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use uuid::Uuid;

use crate::asignacion_view_logic::{etiqueta_mes, planear_quitar};
use crate::db::asignacion_ops::aplicar_plan_tramos;
use crate::db::bitacora_reportes::{ultimo_lote_revision, ACCION_DESVINCULAR};
use crate::db::prorrateo::dias_efectivos;
use crate::db::reporte_liquidaciones::{
    add_obra_vinculacion_for_employee, batch_assign_multiple_obras_to_employees,
    compute_guarderias_kpis, ObraOptionDb, ReporteGuarderiaKpiDb, ReporteLiquidacionRowDb,
};
use crate::db::DbPool;
use crate::reporte_view_logic::{format_curr, sync_and_load_reporte_data, ReporteSessionState};
use crate::{
    show_toast_message, AppData, AppLogic, AsigObraRenglon, AsigObraTarjeta, MainWindow,
};

/// Clave de movimiento que identifica una baja (comparación sin distinguir mayúsculas).
const CLAVE_BAJA: &str = "baja";
/// Número de colores rotativos para las tarjetas de obra.
const NUM_COLORES: usize = 10;

thread_local! {
    /// Reportes (empleado-mes) marcados en la tabla de la obra seleccionada.
    static SEL_RENGLONES: RefCell<HashSet<Uuid>> = RefCell::new(HashSet::new());
    /// Reportes marcados en el modal "Agregar empleados…".
    static SEL_CANDIDATOS: RefCell<HashSet<Uuid>> = RefCell::new(HashSet::new());
}

/// Tarjeta de una obra: totales de su asignación y KPIs de guarderías (si aplican).
#[derive(Debug, Clone)]
pub struct TarjetaObra {
    pub id_obra: Uuid,
    pub numero: String,
    pub label: String,
    pub vigencia: String,
    pub monto: f64,
    pub sin_monto: bool,
    /// Empleados distintos con tramos en la obra.
    pub empleados: usize,
    /// Empleado-mes (reportes) distintos con tramos en la obra.
    pub empleado_mes: usize,
    /// Suma de días de obra de todos los tramos.
    pub dias: i32,
    /// KPIs de guarderías reutilizados; `None` para obras que no los generan.
    pub kpi: Option<ReporteGuarderiaKpiDb>,
    pub color_idx: i32,
}

/// Renglón de un reporte (empleado-mes) relacionado con una obra.
#[derive(Debug, Clone, PartialEq)]
pub struct RenglonObra {
    pub id_reporte: Uuid,
    pub id_empleado: Uuid,
    pub nombre: String,
    pub nss: String,
    pub anio: i16,
    pub mes: i16,
    /// Días de la obra en el reporte (renglones de obra) o 0 (candidatos).
    pub dias_obra: i32,
    pub dias_cedula: i32,
    /// Días efectivos del mes (cédula menos incidencias).
    pub disponibles: i32,
    /// Días ya asignados a cualquier obra del reporte.
    pub asignados: i32,
}

impl RenglonObra {
    /// Días que aún pueden asignarse al reporte (nunca negativos).
    pub fn libres(&self) -> i32 {
        (self.disponibles - self.asignados).max(0)
    }
}

fn es_baja(t: &ReporteLiquidacionRowDb) -> bool {
    t.clave.trim().eq_ignore_ascii_case(CLAVE_BAJA)
}

/// Agrupa los tramos por reporte (empleado-mes), en orden estable por id.
fn agrupar_por_reporte(rows: &[ReporteLiquidacionRowDb]) -> BTreeMap<Uuid, Vec<&ReporteLiquidacionRowDb>> {
    let mut grupos: BTreeMap<Uuid, Vec<&ReporteLiquidacionRowDb>> = BTreeMap::new();
    for t in rows {
        grupos.entry(t.id_parent_reporte_liq).or_default().push(t);
    }
    grupos
}

/// Días de cédula y disponibles con la misma regla que la matriz de asignación.
fn dias_y_disponibles(grupo: &[&ReporteLiquidacionRowDb]) -> (i32, i32) {
    let no_baja: Vec<&ReporteLiquidacionRowDb> = grupo.iter().copied().filter(|t| !es_baja(t)).collect();
    let dias_cedula = no_baja.iter().map(|t| i32::from(t.dias_cedula)).max().unwrap_or(0);
    let disponibles = no_baja
        .first()
        .map(|t| dias_efectivos(dias_cedula, t.lic, t.inc, t.aus))
        .unwrap_or(0);
    (dias_cedula, disponibles)
}

/// Construye el renglón de un reporte. `None` si el grupo está vacío.
fn renglon_de_reporte(
    id_reporte: Uuid,
    grupo: &[&ReporteLiquidacionRowDb],
    id_obra: Uuid,
) -> Option<RenglonObra> {
    let primero = grupo.first()?;
    let (dias_cedula, disponibles) = dias_y_disponibles(grupo);
    let dias_obra = grupo
        .iter()
        .filter(|t| t.id_registro_obra == Some(id_obra))
        .map(|t| i32::from(t.dias_obra))
        .sum();
    let asignados = grupo
        .iter()
        .filter(|t| t.id_registro_obra.is_some())
        .map(|t| i32::from(t.dias_obra))
        .sum();
    Some(RenglonObra {
        id_reporte,
        id_empleado: primero.id_empleado,
        nombre: primero.nombre_trabajador.clone(),
        nss: primero.nss.clone(),
        anio: primero.anio_periodo,
        mes: primero.mes_periodo_int,
        dias_obra,
        dias_cedula,
        disponibles,
        asignados,
    })
}

/// Texto de vigencia "dd/mm/aaaa – dd/mm/aaaa" (guion si falta la fecha).
fn vigencia_texto(o: &ObraOptionDb) -> String {
    let fmt = |f: Option<NaiveDate>| f.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "—".to_string());
    format!("{} – {}", fmt(o.fecha_inicio), fmt(o.fecha_fin))
}

/// Tarjetas de todas las obras del catálogo, ordenadas por número.
///
/// Empleados y días se cuentan sobre los tramos con `id_registro_obra` igual a la obra.
/// Los KPIs de guarderías se reutilizan de `compute_guarderias_kpis`.
pub fn construir_tarjetas_obra(rows: &[ReporteLiquidacionRowDb], obras: &[ObraOptionDb]) -> Vec<TarjetaObra> {
    let kpis = compute_guarderias_kpis(rows, obras, None);
    let mut tarjetas: Vec<TarjetaObra> = obras
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let tramos: Vec<&ReporteLiquidacionRowDb> =
                rows.iter().filter(|t| t.id_registro_obra == Some(o.id)).collect();
            let empleados: BTreeSet<Uuid> = tramos.iter().map(|t| t.id_empleado).collect();
            let reportes: BTreeSet<Uuid> = tramos.iter().map(|t| t.id_parent_reporte_liq).collect();
            TarjetaObra {
                id_obra: o.id,
                numero: o.numero.clone(),
                label: o.label.clone(),
                vigencia: vigencia_texto(o),
                monto: o.monto,
                sin_monto: o.monto <= 0.0,
                empleados: empleados.len(),
                empleado_mes: reportes.len(),
                dias: tramos.iter().map(|t| i32::from(t.dias_obra)).sum(),
                kpi: kpis.iter().find(|k| k.id_obra == o.id).cloned(),
                color_idx: (i % NUM_COLORES) as i32,
            }
        })
        .collect();
    tarjetas.sort_by(|a, b| a.numero.cmp(&b.numero).then(a.id_obra.cmp(&b.id_obra)));
    tarjetas
}

/// Renglones (un reporte por empleado-mes) que tienen tramos en la obra, ordenados por nombre y periodo.
pub fn renglones_de_obra(rows: &[ReporteLiquidacionRowDb], id_obra: Uuid) -> Vec<RenglonObra> {
    let mut renglones: Vec<RenglonObra> = agrupar_por_reporte(rows)
        .into_iter()
        .filter(|(_, g)| g.iter().any(|t| t.id_registro_obra == Some(id_obra)))
        .filter_map(|(id, g)| renglon_de_reporte(id, &g, id_obra))
        .collect();
    renglones.sort_by(|a, b| {
        a.nombre
            .cmp(&b.nombre)
            .then(a.anio.cmp(&b.anio))
            .then(a.mes.cmp(&b.mes))
            .then(a.id_reporte.cmp(&b.id_reporte))
    });
    renglones
}

/// Reportes activos (con algún movimiento que no sea baja) que aún no tienen la obra.
/// `busqueda` filtra por nombre o NSS sin distinguir mayúsculas; vacía no filtra.
pub fn candidatos_para_obra(
    rows: &[ReporteLiquidacionRowDb],
    id_obra: Uuid,
    busqueda: &str,
) -> Vec<RenglonObra> {
    let q = busqueda.trim().to_lowercase();
    let mut candidatos: Vec<RenglonObra> = agrupar_por_reporte(rows)
        .into_iter()
        .filter(|(_, g)| g.iter().any(|t| !es_baja(t)))
        .filter(|(_, g)| !g.iter().any(|t| t.id_registro_obra == Some(id_obra)))
        .filter_map(|(id, g)| renglon_de_reporte(id, &g, id_obra))
        .filter(|r| {
            q.is_empty() || r.nombre.to_lowercase().contains(&q) || r.nss.to_lowercase().contains(&q)
        })
        .collect();
    candidatos.sort_by(|a, b| a.nombre.cmp(&b.nombre).then(a.id_reporte.cmp(&b.id_reporte)));
    candidatos
}

fn tarjeta_slint(t: &TarjetaObra) -> AsigObraTarjeta {
    let (gps, valor, limite, supera) = match &t.kpi {
        Some(k) => (
            format_curr(k.total_gps),
            format_curr(k.valor_guarderias),
            format_curr(k.limite_18_obra),
            k.supera_18,
        ),
        None => (String::new(), String::new(), String::new(), false),
    };
    AsigObraTarjeta {
        id_obra: t.id_obra.to_string().into(),
        numero: t.numero.clone().into(),
        label: t.label.clone().into(),
        vigencia: t.vigencia.clone().into(),
        monto: if t.sin_monto { SharedString::default() } else { format_curr(t.monto).into() },
        empleados: t.empleados as i32,
        empleado_mes: t.empleado_mes as i32,
        dias: t.dias,
        gps: gps.into(),
        valor_guarderia: valor.into(),
        limite_18: limite.into(),
        supera_18: supera,
        sin_monto: t.sin_monto,
        color_idx: t.color_idx,
    }
}

fn renglon_slint(r: &RenglonObra, dias: i32, seleccionado: bool) -> AsigObraRenglon {
    AsigObraRenglon {
        id_reporte: r.id_reporte.to_string().into(),
        id_empleado: r.id_empleado.to_string().into(),
        nombre: r.nombre.clone().into(),
        nss: r.nss.clone().into(),
        etiqueta_mes: etiqueta_mes(r.anio, r.mes).into(),
        dias,
        dias_cedula: r.dias_cedula,
        seleccionado,
    }
}

/// Obra seleccionada en la pestaña, si el identificador es válido y existe en el catálogo.
fn obra_seleccionada<'a>(obras: &'a [ObraOptionDb], texto: &str) -> Option<&'a ObraOptionDb> {
    let id = Uuid::parse_str(texto.trim()).ok()?;
    obras.iter().find(|o| o.id == id)
}

/// Llena las propiedades de la pestaña conservando las selecciones (se descartan las que ya no existen).
pub fn render_asignacion_obra_ui(ui: &MainWindow, state: &ReporteSessionState) {
    let app_data = ui.global::<AppData>();

    let tarjetas: Vec<AsigObraTarjeta> = construir_tarjetas_obra(&state.all_rows, &state.obras_options)
        .iter()
        .map(tarjeta_slint)
        .collect();
    app_data.set_asig_obra_tarjetas(ModelRc::new(VecModel::from(tarjetas)));

    let sel_texto = app_data.get_asig_obra_sel_id().to_string();
    let Some(obra) = obra_seleccionada(&state.obras_options, &sel_texto) else {
        app_data.set_asig_obra_sel_id(SharedString::default());
        app_data.set_asig_obra_sel_titulo(SharedString::default());
        app_data.set_asig_obra_renglones(ModelRc::new(VecModel::from(Vec::<AsigObraRenglon>::new())));
        app_data.set_asig_obra_candidatos(ModelRc::new(VecModel::from(Vec::<AsigObraRenglon>::new())));
        SEL_RENGLONES.with(|s| s.borrow_mut().clear());
        SEL_CANDIDATOS.with(|s| s.borrow_mut().clear());
        return;
    };

    app_data.set_asig_obra_sel_titulo(format!("{} · {}", obra.numero, obra.label).into());

    let renglones = renglones_de_obra(&state.all_rows, obra.id);
    let ids: HashSet<Uuid> = renglones.iter().map(|r| r.id_reporte).collect();
    let marcados = SEL_RENGLONES.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|id| ids.contains(id));
        s.clone()
    });
    let filas: Vec<AsigObraRenglon> = renglones
        .iter()
        .map(|r| renglon_slint(r, r.dias_obra, marcados.contains(&r.id_reporte)))
        .collect();
    app_data.set_asig_obra_renglones(ModelRc::new(VecModel::from(filas)));

    let candidatos = if app_data.get_show_asig_obra_agregar_modal() {
        let busqueda = app_data.get_asig_obra_candidatos_busqueda().to_string();
        candidatos_para_obra(&state.all_rows, obra.id, &busqueda)
    } else {
        Vec::new()
    };
    let ids_c: HashSet<Uuid> = candidatos.iter().map(|r| r.id_reporte).collect();
    let marcados_c = SEL_CANDIDATOS.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|id| ids_c.contains(id));
        s.clone()
    });
    let filas_c: Vec<AsigObraRenglon> = candidatos
        .iter()
        .map(|r| renglon_slint(r, r.libres(), marcados_c.contains(&r.id_reporte)))
        .collect();
    app_data.set_asig_obra_candidatos(ModelRc::new(VecModel::from(filas_c)));
}

/// Re-renderiza solo la pestaña de asignación por obra con el estado actual de la sesión.
fn refrescar_asignacion_obra(ui: &MainWindow, session: &Arc<Mutex<ReporteSessionState>>) {
    if let Ok(st) = session.lock() {
        render_asignacion_obra_ui(ui, &st);
    }
}

/// Obra seleccionada en la UI, como identificador.
fn obra_id_de_ui(ui: &MainWindow) -> Option<Uuid> {
    Uuid::parse_str(ui.global::<AppData>().get_asig_obra_sel_id().trim()).ok()
}

/// Actualiza las propiedades de deshacer con el último lote de la revisión.
fn actualizar_deshacer_obra(ui: &MainWindow, ultimo: Option<(Uuid, String)>) {
    let app_data = ui.global::<AppData>();
    match ultimo {
        Some((_, accion)) => {
            let hora = chrono::Local::now().format("%H:%M");
            app_data.set_asig_ultimo_cambio(format!("{accion} · {hora}").into());
            app_data.set_asig_puede_deshacer(true);
        }
        None => {
            app_data.set_asig_ultimo_cambio(SharedString::default());
            app_data.set_asig_puede_deshacer(false);
        }
    }
}

/// Ejecuta una acción de BD sobre la revisión activa y luego recarga el reporte.
///
/// Libera el lock de sesión antes de recargar. Si la acción tuvo éxito limpia las selecciones
/// y cierra el modal; en error las conserva para que el usuario reintente.
fn ejecutar_accion_obra<F>(
    ui: &MainWindow,
    pool: &Option<Arc<DbPool>>,
    session: &Arc<Mutex<ReporteSessionState>>,
    descripcion: &str,
    accion: F,
) where
    F: FnOnce(&mut Client, Uuid, &ReporteSessionState) -> Result<String, String>,
{
    let rev_str = session
        .lock()
        .ok()
        .and_then(|st| st.current_revision_id.clone());
    let Some(rev_str) = rev_str else {
        show_toast_message(ui, "Seleccione una revisión antes de continuar.");
        return;
    };
    let rev = match Uuid::parse_str(&rev_str) {
        Ok(r) => r,
        Err(_) => {
            show_toast_message(ui, "La revisión seleccionada no es válida.");
            return;
        }
    };
    let Some(pool_ref) = pool.as_ref() else {
        show_toast_message(ui, "Sin conexión a la base de datos.");
        return;
    };

    let app_data = ui.global::<AppData>();
    app_data.set_asig_procesando(true);

    let (resultado, deshacer) = match pool_ref.get() {
        Err(e) => (Err(format!("conexión a la base de datos: {e}")), None),
        Ok(mut client) => {
            let resultado = match session.lock() {
                Ok(st) => accion(&mut *client, rev, &*st),
                Err(_) => Err("estado de la sesión bloqueado".to_string()),
            };
            let deshacer = match ultimo_lote_revision(&mut *client, rev) {
                Ok(d) => d,
                Err(e) => {
                    log::error!("Asignación por obra: consulta de bitácora: {e}");
                    None
                }
            };
            (resultado, deshacer)
        }
    };

    app_data.set_asig_procesando(false);
    if resultado.is_ok() {
        SEL_RENGLONES.with(|s| s.borrow_mut().clear());
        SEL_CANDIDATOS.with(|s| s.borrow_mut().clear());
        app_data.set_show_asig_obra_agregar_modal(false);
    }
    // La recarga toma el lock de sesión, por eso se liberó arriba.
    sync_and_load_reporte_data(ui, pool, session, &rev_str, false);
    actualizar_deshacer_obra(ui, deshacer);

    match resultado {
        Ok(mensaje) => show_toast_message(ui, &mensaje),
        Err(e) => {
            log::error!("Asignación por obra ({descripcion}): {e}");
            show_toast_message(ui, &format!("No se pudo {descripcion}: {e}"));
        }
    }
}

/// Registra los callbacks de la pestaña "Asignación por obra".
pub fn wire_asignacion_obra_callbacks(
    main_window: &MainWindow,
    db_pool: Option<Arc<DbPool>>,
    session: Arc<Mutex<ReporteSessionState>>,
) {
    let app_logic = main_window.global::<AppLogic>();

    // Elegir obra: cambia la tarjeta activa, limpia selecciones y cierra el modal.
    {
        let ui_weak = main_window.as_weak();
        let session = session.clone();
        app_logic.on_asig_obra_seleccionar(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let app_data = ui.global::<AppData>();
            app_data.set_asig_obra_sel_id(id);
            app_data.set_show_asig_obra_agregar_modal(false);
            SEL_RENGLONES.with(|s| s.borrow_mut().clear());
            SEL_CANDIDATOS.with(|s| s.borrow_mut().clear());
            refrescar_asignacion_obra(&ui, &session);
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let session = session.clone();
        app_logic.on_asig_obra_toggle_renglon(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Ok(id) = Uuid::parse_str(id.trim()) {
                SEL_RENGLONES.with(|s| {
                    let mut s = s.borrow_mut();
                    if !s.remove(&id) {
                        s.insert(id);
                    }
                });
            }
            refrescar_asignacion_obra(&ui, &session);
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_obra_quitar_seleccionados(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(obra_id) = obra_id_de_ui(&ui) else {
                show_toast_message(&ui, "Seleccione una obra.");
                return;
            };
            let sel: Vec<Uuid> = SEL_RENGLONES.with(|s| s.borrow().iter().copied().collect());
            if sel.is_empty() {
                show_toast_message(&ui, "Seleccione al menos un empleado-mes.");
                return;
            }
            ejecutar_accion_obra(&ui, &pool, &session, "quitar la obra de los seleccionados", move |client, _rev, st| {
                let mut borrar: Vec<Uuid> = Vec::new();
                let mut guardar: Vec<(Uuid, i16, i16, Option<Uuid>)> = Vec::new();
                let mut meses = 0usize;
                for rep in &sel {
                    let tramos: Vec<&ReporteLiquidacionRowDb> = st
                        .all_rows
                        .iter()
                        .filter(|t| t.id_parent_reporte_liq == *rep)
                        .collect();
                    if !tramos.iter().any(|t| t.id_registro_obra == Some(obra_id)) {
                        continue;
                    }
                    let (b, g) = planear_quitar(&tramos, obra_id);
                    borrar.extend(b);
                    guardar.extend(g);
                    meses += 1;
                }
                if meses == 0 {
                    return Err("los meses seleccionados ya no tienen esa obra".to_string());
                }
                // Una sola transacción y un solo lote de bitácora para todos los meses.
                aplicar_plan_tramos(client, ACCION_DESVINCULAR, &borrar, &guardar)?;
                Ok(format!(
                    "Obra quitada de {meses} mes(es): {} tramo(s) eliminado(s), {} pasan a sin obra.",
                    borrar.len(),
                    guardar.len()
                ))
            });
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let session = session.clone();
        app_logic.on_asig_obra_abrir_agregar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if obra_id_de_ui(&ui).is_none() {
                show_toast_message(&ui, "Seleccione una obra.");
                return;
            }
            let app_data = ui.global::<AppData>();
            app_data.set_asig_obra_candidatos_busqueda(SharedString::default());
            SEL_CANDIDATOS.with(|s| s.borrow_mut().clear());
            app_data.set_show_asig_obra_agregar_modal(true);
            refrescar_asignacion_obra(&ui, &session);
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let session = session.clone();
        app_logic.on_asig_obra_cerrar_agregar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let app_data = ui.global::<AppData>();
            app_data.set_show_asig_obra_agregar_modal(false);
            app_data.set_asig_obra_candidatos_busqueda(SharedString::default());
            SEL_CANDIDATOS.with(|s| s.borrow_mut().clear());
            refrescar_asignacion_obra(&ui, &session);
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let session = session.clone();
        app_logic.on_asig_obra_buscar_candidatos(move |texto| {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.global::<AppData>().set_asig_obra_candidatos_busqueda(texto);
            refrescar_asignacion_obra(&ui, &session);
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let session = session.clone();
        app_logic.on_asig_obra_toggle_candidato(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Ok(id) = Uuid::parse_str(id.trim()) {
                SEL_CANDIDATOS.with(|s| {
                    let mut s = s.borrow_mut();
                    if !s.remove(&id) {
                        s.insert(id);
                    }
                });
            }
            refrescar_asignacion_obra(&ui, &session);
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_obra_agregar_seleccionados(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(obra_id) = obra_id_de_ui(&ui) else {
                show_toast_message(&ui, "Seleccione una obra.");
                return;
            };
            let sel: Vec<Uuid> = SEL_CANDIDATOS.with(|s| s.borrow().iter().copied().collect());
            if sel.is_empty() {
                show_toast_message(&ui, "Seleccione al menos un empleado-mes.");
                return;
            }
            ejecutar_accion_obra(&ui, &pool, &session, "agregar la obra", move |client, rev, st| {
                let elegidos: Vec<RenglonObra> = candidatos_para_obra(&st.all_rows, obra_id, "")
                    .into_iter()
                    .filter(|c| sel.contains(&c.id_reporte))
                    .collect();

                // Reportes sin ninguna obra: un lote; reportes con obras: un vínculo por reporte.
                let mut sin_obra: Vec<Uuid> = Vec::new();
                let mut con_obra: Vec<(Uuid, i16)> = Vec::new();
                let mut omitidos = 0usize;
                for c in &elegidos {
                    let libres = c.libres();
                    if libres <= 0 {
                        omitidos += 1;
                        continue;
                    }
                    let ya_tiene_obra = st
                        .all_rows
                        .iter()
                        .any(|t| t.id_parent_reporte_liq == c.id_reporte && t.id_registro_obra.is_some());
                    if ya_tiene_obra {
                        let dias = i16::try_from(libres)
                            .map_err(|_| format!("Los días ({libres}) están fuera de rango."))?;
                        con_obra.push((c.id_reporte, dias));
                    } else {
                        sin_obra.push(c.id_reporte);
                    }
                }

                let mut agregados = 0usize;
                if !sin_obra.is_empty() {
                    // Sin custom_dias_obra el lote reparte los días disponibles de cada mes.
                    batch_assign_multiple_obras_to_employees(client, rev, &sin_obra, &[obra_id], None, None, None)?;
                    agregados += sin_obra.len();
                }
                for (rep, dias) in &con_obra {
                    add_obra_vinculacion_for_employee(client, *rep, obra_id, *dias).map_err(|e| {
                        format!("{agregados} mes(es) ya agregado(s) antes del error: {e}")
                    })?;
                    agregados += 1;
                }
                if agregados == 0 {
                    return Err(format!(
                        "ningún mes seleccionado tiene días disponibles ({omitidos} omitido(s))"
                    ));
                }
                Ok(format!(
                    "Obra agregada a {agregados} mes(es); {omitidos} omitido(s) sin días disponibles."
                ))
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn obra(id: u128, numero: &str, monto: f64, creado_usuario: bool) -> ObraOptionDb {
        ObraOptionDb {
            id: uid(id),
            numero: numero.to_string(),
            tipo_obra: String::new(),
            fecha_inicio: NaiveDate::from_ymd_opt(2026, 1, 1),
            fecha_fin: None,
            label: format!("Obra {numero}"),
            creado_usuario,
            monto,
        }
    }

    /// Tramo sin obra, de alta, con valores por defecto.
    fn tramo(id: u128, reporte: u128, empleado: u128, nombre: &str, mes: i16) -> ReporteLiquidacionRowDb {
        ReporteLiquidacionRowDb {
            id_reporte_liq: uid(id),
            id_parent_reporte_liq: uid(reporte),
            id_liquidacion: uid(9000),
            id_revision: uid(9001),
            id_empleado: uid(empleado),
            nombre_trabajador: nombre.to_string(),
            rfc: format!("RFC{empleado}"),
            curp: String::new(),
            nss: format!("NSS{empleado}"),
            pensionado: String::new(),
            registro_patronal: String::new(),
            mes_periodo: String::new(),
            mes_periodo_int: mes,
            anio_periodo: 2026,
            periodo_str: format!("{mes:02}/2026"),
            dias_cedula: 30,
            id_registro_obra: None,
            obra_numero: String::new(),
            obra_label: String::new(),
            dias_obra: 0,
            obra_creado_usuario: false,
            clave: "ALTA".to_string(),
            fecha: None,
            fecha_str: String::new(),
            sdi: 0.0,
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
            cf_orig: 0.0,
            exc_pat_orig: 0.0,
            exc_obr_orig: 0.0,
            pd_pat_orig: 0.0,
            pd_obr_orig: 0.0,
            gmp_pat_orig: 0.0,
            gmp_obr_orig: 0.0,
            rt_orig: 0.0,
            iv_pat_orig: 0.0,
            iv_obr_orig: 0.0,
            gps_orig: 0.0,
            patronal_orig: 0.0,
            obrera_orig: 0.0,
            subtotal_orig: 0.0,
            tiene_inconsistencia: false,
            observacion_validacion: String::new(),
            estado: "ACTIVO".to_string(),
            cedula_actualizada: false,
            editado_usuario: false,
        }
    }

    /// Asigna una obra al tramo con los días indicados.
    fn con_obra(mut t: ReporteLiquidacionRowDb, id_obra: u128, dias_obra: i16, gps: f64) -> ReporteLiquidacionRowDb {
        t.id_registro_obra = Some(uid(id_obra));
        t.dias_obra = dias_obra;
        t.gps = gps;
        t
    }

    fn baja(mut t: ReporteLiquidacionRowDb) -> ReporteLiquidacionRowDb {
        t.clave = "BAJA".to_string();
        t
    }

    fn cerca(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn asignacion_obra_tarjetas_cuentan_empleados_y_dias() {
        let obras = vec![obra(100, "001", 1000.0, true), obra(101, "002", 0.0, false)];
        let rows = vec![
            // Empleado 1 en dos meses con la obra 001 (5 días cada uno, GPS 5).
            con_obra(tramo(1, 10, 1, "Ana", 1), 100, 5, 5.0),
            con_obra(tramo(2, 11, 1, "Ana", 2), 100, 5, 5.0),
            // Empleado 2 en un mes con la obra 001 (3 días).
            con_obra(tramo(3, 12, 2, "Luis", 1), 100, 3, 0.0),
        ];
        let tarjetas = construir_tarjetas_obra(&rows, &obras);
        assert_eq!(tarjetas.len(), 2);

        let t1 = tarjetas.iter().find(|t| t.id_obra == uid(100)).expect("tarjeta 001");
        assert_eq!(t1.empleados, 2);
        assert_eq!(t1.empleado_mes, 3);
        assert_eq!(t1.dias, 13);
        assert!(!t1.sin_monto);

        // KPIs reutilizados: GPS 10 -> valor 1000 > límite 180 => supera.
        let k = t1.kpi.as_ref().expect("kpi de guarderías");
        assert!(cerca(k.total_gps, 10.0));
        assert!(cerca(k.valor_guarderias, 1000.0));
        assert!(cerca(k.limite_18_obra, 180.0));
        assert!(k.supera_18);

        let t2 = tarjetas.iter().find(|t| t.id_obra == uid(101)).expect("tarjeta 002");
        assert!(t2.sin_monto);
        assert_eq!(t2.empleados, 0);
        assert!(t2.kpi.is_none());
    }

    #[test]
    fn asignacion_obra_renglones_solo_reportes_con_la_obra() {
        let rows = vec![
            con_obra(tramo(1, 10, 1, "Zoe", 1), 100, 4, 0.0),
            tramo(2, 11, 2, "Ana", 1),
            con_obra(tramo(3, 12, 3, "Ana", 2), 100, 6, 0.0),
            con_obra(tramo(4, 12, 3, "Ana", 2), 101, 2, 0.0),
        ];
        let renglones = renglones_de_obra(&rows, uid(100));
        assert_eq!(renglones.len(), 2);
        // Ordenados por nombre y periodo.
        assert_eq!(renglones[0].nombre, "Ana");
        assert_eq!(renglones[0].mes, 2);
        assert_eq!(renglones[0].dias_obra, 6);
        assert_eq!(renglones[1].nombre, "Zoe");
        assert_eq!(renglones[1].dias_obra, 4);
    }

    #[test]
    fn asignacion_obra_candidatos_excluyen_bajas_y_asignados_y_filtran() {
        let rows = vec![
            // Ya tiene la obra 100: no es candidato.
            con_obra(tramo(1, 10, 1, "Ana", 1), 100, 4, 0.0),
            // Candidato activo sin obras.
            tramo(2, 11, 2, "Beatriz", 1),
            // Solo baja: no es candidato.
            baja(tramo(3, 12, 3, "Carlos", 1)),
            // Activo con otra obra: candidato con días libres.
            con_obra(tramo(4, 13, 4, "Diana", 1), 101, 10, 0.0),
        ];
        let todos = candidatos_para_obra(&rows, uid(100), "");
        let nombres: Vec<&str> = todos.iter().map(|c| c.nombre.as_str()).collect();
        assert_eq!(nombres, vec!["Beatriz", "Diana"]);

        let por_nss = candidatos_para_obra(&rows, uid(100), "  nss2 ");
        assert_eq!(por_nss.len(), 1);
        assert_eq!(por_nss[0].nombre, "Beatriz");

        let por_nombre = candidatos_para_obra(&rows, uid(100), "DIA");
        assert_eq!(por_nombre.len(), 1);
        assert_eq!(por_nombre[0].id_reporte, uid(13));
        // 30 de cédula - 10 asignados en la obra 101 = 20 libres.
        assert_eq!(por_nombre[0].libres(), 20);
    }

    #[test]
    fn asignacion_obra_libres_nunca_negativos() {
        let r = RenglonObra {
            id_reporte: uid(1),
            id_empleado: uid(1),
            nombre: "X".to_string(),
            nss: String::new(),
            anio: 2026,
            mes: 1,
            dias_obra: 0,
            dias_cedula: 30,
            disponibles: 10,
            asignados: 14,
        };
        assert_eq!(r.libres(), 0);
    }
}
