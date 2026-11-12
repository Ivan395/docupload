//! Vista de asignación empleado × mes del Reporte de Liquidaciones.
//! La matriz, el panel de avance, los filtros y los planificadores de reparto son funciones puras.
//! Los callbacks de Slint ejecutan las operaciones de BD existentes (con bitácora y prorrateo)
//! y recargan el reporte tras cada acción.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use postgres::Client;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use uuid::Uuid;

use crate::db::bitacora_reportes::{deshacer_lote, ultimo_lote_revision};
use crate::db::asignacion_ops::revertir_reportes_empleado;
use crate::db::prorrateo::dias_efectivos;
use crate::db::reporte_liquidaciones::{
    add_obra_vinculacion_for_employee, batch_assign_multiple_obras_to_employees,
    batch_save_reporte_row_updates, ObraOptionDb,
    ReporteLiquidacionRowDb,
};
use crate::db::DbPool;
use crate::reporte_view_logic::{sync_and_load_reporte_data, ReporteSessionState};
use crate::{
    show_toast_message, AppData, AppLogic, AsigCelda, AsigEmpleadoFila, AsigObraChip,
    AsigObraOpcion, MainWindow,
};

/// Clave de movimiento que identifica una baja (comparación sin distinguir mayúsculas).
const CLAVE_BAJA: &str = "baja";
/// Índice de color reservado para obras que no están en el catálogo recibido.
const COLOR_OBRA_DESCONOCIDA: i32 = 9;
/// Número de colores rotativos para los chips de obra.
const NUM_COLORES: usize = 10;

/// Chip de obra dentro de una celda empleado-mes.
#[derive(Debug, Clone, PartialEq)]
pub struct ChipObra {
    pub id_obra: Uuid,
    pub numero: String,
    pub dias: i32,
    pub ids_tramos: Vec<Uuid>,
    pub color_idx: i32,
}

/// Celda de la matriz: un reporte (empleado-mes).
#[derive(Debug, Clone, PartialEq)]
pub struct CeldaMes {
    pub id_reporte: Uuid,
    pub anio: i16,
    pub mes: i16,
    pub dias_cedula: i32,
    pub disponibles: i32,
    pub asignados: i32,
    pub obras: Vec<ChipObra>,
    pub ids_tramos_sin_obra: Vec<Uuid>,
    pub es_baja: bool,
    pub inconsistente: bool,
    pub cedula_actualizada: bool,
}

/// Fila de la matriz: un empleado con una celda opcional por mes.
#[derive(Debug, Clone, PartialEq)]
pub struct FilaEmpleado {
    pub id_empleado: Uuid,
    pub nombre: String,
    pub nss: String,
    pub rfc: String,
    /// Una celda por mes de `MatrizAsignacion::meses`; `None` si no hay reporte ese mes.
    pub celdas: Vec<Option<CeldaMes>>,
    /// Obras distintas en todos los meses del empleado.
    pub num_obras: usize,
    pub tiene_inconsistencia: bool,
    pub cedula_actualizada: bool,
}

/// Matriz completa empleado × mes.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrizAsignacion {
    /// Meses (anio, mes) ordenados ascendentemente.
    pub meses: Vec<(i16, i16)>,
    /// Filas ordenadas por nombre.
    pub filas: Vec<FilaEmpleado>,
}

/// Escenario de avance según el porcentaje de empleado-mes con obra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escenario {
    TodosVinculados,
    Algunos,
    NingunoOCasiNinguno,
}

/// Resumen del panel de avance.
#[derive(Debug, Clone, PartialEq)]
pub struct ResumenAvance {
    pub total_empleado_mes: usize,
    pub con_obra: usize,
    pub sin_obra: usize,
    /// Celdas existentes marcadas como inconsistentes (incluye bajas).
    pub inconsistencias: usize,
    /// Celdas existentes con la cédula ya actualizada (incluye bajas).
    pub cedula_actualizada: usize,
    pub porcentaje: f32,
    pub escenario: Escenario,
}

/// Filtro de la lista de empleados.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiltroAsignacion {
    Todos,
    SinObra,
    UnaObra,
    DosOMas,
    ConInconsistencia,
    CedulaActualizada,
}

/// Agrupa los tramos del reporte en una matriz empleado × mes.
///
/// Se asume un único reporte por empleado-mes; si llegara más de uno, se conserva el primero.
pub fn construir_matriz(tramos: &[ReporteLiquidacionRowDb], obras: &[ObraOptionDb]) -> MatrizAsignacion {
    // Agrupa tramos por reporte (empleado-mes) con orden determinista por id.
    let mut por_reporte: BTreeMap<Uuid, Vec<&ReporteLiquidacionRowDb>> = BTreeMap::new();
    for t in tramos {
        por_reporte.entry(t.id_parent_reporte_liq).or_default().push(t);
    }

    let meses: Vec<(i16, i16)> = tramos
        .iter()
        .map(|t| (t.anio_periodo, t.mes_periodo_int))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let idx_mes: HashMap<(i16, i16), usize> = meses.iter().enumerate().map(|(i, m)| (*m, i)).collect();

    let mut filas: Vec<FilaEmpleado> = Vec::new();
    let mut idx_fila: HashMap<Uuid, usize> = HashMap::new();

    for (id_reporte, grupo) in &por_reporte {
        let primero = grupo[0];
        let idx = match idx_fila.get(&primero.id_empleado) {
            Some(&i) => i,
            None => {
                filas.push(FilaEmpleado {
                    id_empleado: primero.id_empleado,
                    nombre: primero.nombre_trabajador.clone(),
                    nss: primero.nss.clone(),
                    rfc: primero.rfc.clone(),
                    celdas: vec![None; meses.len()],
                    num_obras: 0,
                    tiene_inconsistencia: false,
                    cedula_actualizada: false,
                });
                let i = filas.len() - 1;
                idx_fila.insert(primero.id_empleado, i);
                i
            }
        };

        let Some(&pos) = idx_mes.get(&(primero.anio_periodo, primero.mes_periodo_int)) else {
            continue;
        };
        if filas[idx].celdas[pos].is_some() {
            continue;
        }
        filas[idx].celdas[pos] = Some(construir_celda(*id_reporte, grupo, obras));
    }

    // Agregados por fila: obras distintas e indicadores de inconsistencia / cédula.
    for fila in &mut filas {
        let mut ids_obras: HashSet<Uuid> = HashSet::new();
        let mut inconsistente = false;
        let mut actualizada = false;
        for c in fila.celdas.iter().flatten() {
            ids_obras.extend(c.obras.iter().map(|ch| ch.id_obra));
            inconsistente |= c.inconsistente;
            actualizada |= c.cedula_actualizada;
        }
        fila.num_obras = ids_obras.len();
        fila.tiene_inconsistencia = inconsistente;
        fila.cedula_actualizada = actualizada;
    }

    filas.sort_by(|a, b| {
        a.nombre
            .to_lowercase()
            .cmp(&b.nombre.to_lowercase())
            .then(a.id_empleado.cmp(&b.id_empleado))
    });

    MatrizAsignacion { meses, filas }
}

/// Construye una celda a partir de los tramos de un reporte (empleado-mes).
fn construir_celda(id_reporte: Uuid, grupo: &[&ReporteLiquidacionRowDb], obras: &[ObraOptionDb]) -> CeldaMes {
    let primero = grupo[0];
    let no_baja: Vec<&ReporteLiquidacionRowDb> = grupo.iter().copied().filter(|t| !es_baja_tramo(t)).collect();
    let es_baja = no_baja.is_empty();

    let dias_cedula: i32 = no_baja
        .iter()
        .map(|t| i32::from(t.dias_cedula))
        .max()
        .unwrap_or(0);
    let disponibles = match no_baja.first() {
        Some(t) => dias_efectivos(dias_cedula, t.lic, t.inc, t.aus),
        None => 0,
    };

    // Agrupa tramos con obra en chips, sumando días y acumulando ids de tramo.
    let mut chips: Vec<ChipObra> = Vec::new();
    let mut idx_chip: HashMap<Uuid, usize> = HashMap::new();
    let mut ids_tramos_sin_obra: Vec<Uuid> = Vec::new();

    for t in grupo {
        match t.id_registro_obra {
            Some(id_obra) => {
                let i = match idx_chip.get(&id_obra) {
                    Some(&i) => i,
                    None => {
                        chips.push(ChipObra {
                            id_obra,
                            numero: numero_obra(t, id_obra, obras),
                            dias: 0,
                            ids_tramos: Vec::new(),
                            color_idx: color_obra(id_obra, obras),
                        });
                        let i = chips.len() - 1;
                        idx_chip.insert(id_obra, i);
                        i
                    }
                };
                let chip = &mut chips[i];
                chip.dias += i32::from(t.dias_obra);
                chip.ids_tramos.push(t.id_reporte_liq);
            }
            None if !es_baja_tramo(t) => ids_tramos_sin_obra.push(t.id_reporte_liq),
            None => {}
        }
    }
    chips.sort_by(|a, b| a.numero.cmp(&b.numero).then(a.id_obra.cmp(&b.id_obra)));

    let asignados: i32 = chips.iter().map(|c| c.dias).sum();
    let inconsistente = grupo.iter().any(|t| t.tiene_inconsistencia) || asignados > disponibles;
    let cedula_actualizada = grupo.iter().any(|t| t.cedula_actualizada);

    CeldaMes {
        id_reporte,
        anio: primero.anio_periodo,
        mes: primero.mes_periodo_int,
        dias_cedula,
        disponibles,
        asignados,
        obras: chips,
        ids_tramos_sin_obra,
        es_baja,
        inconsistente,
        cedula_actualizada,
    }
}

fn es_baja_tramo(t: &ReporteLiquidacionRowDb) -> bool {
    t.clave.trim().eq_ignore_ascii_case(CLAVE_BAJA)
}

/// Número de obra del tramo; si viene vacío, se toma del catálogo.
fn numero_obra(t: &ReporteLiquidacionRowDb, id_obra: Uuid, obras: &[ObraOptionDb]) -> String {
    if !t.obra_numero.trim().is_empty() {
        return t.obra_numero.clone();
    }
    obras
        .iter()
        .find(|o| o.id == id_obra)
        .map(|o| o.numero.clone())
        .unwrap_or_default()
}

/// Índice de color del chip: posición en el catálogo módulo 10, o 9 si no existe.
fn color_obra(id_obra: Uuid, obras: &[ObraOptionDb]) -> i32 {
    obras
        .iter()
        .position(|o| o.id == id_obra)
        .map(|p| (p % NUM_COLORES) as i32)
        .unwrap_or(COLOR_OBRA_DESCONOCIDA)
}

/// Calcula el panel de avance a partir de la matriz.
pub fn resumen_avance(m: &MatrizAsignacion) -> ResumenAvance {
    let mut r = ResumenAvance {
        total_empleado_mes: 0,
        con_obra: 0,
        sin_obra: 0,
        inconsistencias: 0,
        cedula_actualizada: 0,
        porcentaje: 0.0,
        escenario: Escenario::NingunoOCasiNinguno,
    };

    for c in m.filas.iter().flat_map(|f| f.celdas.iter()).flatten() {
        if c.inconsistente {
            r.inconsistencias += 1;
        }
        if c.cedula_actualizada {
            r.cedula_actualizada += 1;
        }
        if c.es_baja {
            continue;
        }
        r.total_empleado_mes += 1;
        if c.obras.is_empty() {
            r.sin_obra += 1;
        } else {
            r.con_obra += 1;
        }
    }

    // Multiplica antes de dividir para que los umbrales (95 / 5) sean exactos en f32.
    r.porcentaje = if r.total_empleado_mes == 0 {
        0.0
    } else {
        (r.con_obra * 100) as f32 / r.total_empleado_mes as f32
    };
    r.escenario = if r.porcentaje >= 95.0 {
        Escenario::TodosVinculados
    } else if r.porcentaje <= 5.0 {
        Escenario::NingunoOCasiNinguno
    } else {
        Escenario::Algunos
    };
    r
}

/// Texto de orientación para el usuario según el escenario.
pub fn texto_escenario(r: &ResumenAvance) -> String {
    match r.escenario {
        Escenario::TodosVinculados => format!(
            "La cédula ya trae obra para casi todos: revisa las {} inconsistencias y exporta.",
            r.inconsistencias
        ),
        Escenario::Algunos => format!(
            "{} empleado-mes sin obra: usa el filtro 'Sin obra' o el asistente de asignación.",
            r.sin_obra
        ),
        Escenario::NingunoOCasiNinguno => {
            "Casi ningún empleado tiene obra: usa el asistente de asignación masiva.".to_string()
        }
    }
}

/// Filtra filas por criterio y búsqueda (sin acentos ni mayúsculas en nombre, NSS y RFC).
pub fn filtrar_filas<'a>(m: &'a MatrizAsignacion, filtro: FiltroAsignacion, busqueda: &str) -> Vec<&'a FilaEmpleado> {
    let q = normalizar(busqueda.trim());
    m.filas
        .iter()
        .filter(|f| {
            let coincide_busqueda = q.is_empty()
                || [f.nombre.as_str(), f.nss.as_str(), f.rfc.as_str()]
                    .iter()
                    .any(|s| normalizar(s).contains(q.as_str()));
            coincide_busqueda && cumple_filtro(f, filtro)
        })
        .collect()
}

fn cumple_filtro(f: &FilaEmpleado, filtro: FiltroAsignacion) -> bool {
    match filtro {
        FiltroAsignacion::Todos => true,
        FiltroAsignacion::SinObra => f
            .celdas
            .iter()
            .flatten()
            .any(|c| !c.es_baja && c.obras.is_empty()),
        FiltroAsignacion::UnaObra => f.num_obras == 1,
        FiltroAsignacion::DosOMas => f.num_obras >= 2,
        FiltroAsignacion::ConInconsistencia => f.tiene_inconsistencia,
        FiltroAsignacion::CedulaActualizada => f.cedula_actualizada,
    }
}

/// Minúsculas y sin acentos (vocales y ñ) para comparaciones de búsqueda.
fn normalizar(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            otro => otro,
        })
        .collect()
}

/// Etiqueta corta de mes en español, p. ej. "MAR 2026".
pub fn etiqueta_mes(anio: i16, mes: i16) -> String {
    const ABREVIATURAS: [&str; 12] = [
        "ENE", "FEB", "MAR", "ABR", "MAY", "JUN", "JUL", "AGO", "SEP", "OCT", "NOV", "DIC",
    ];
    let nombre = usize::try_from(i32::from(mes) - 1)
        .ok()
        .and_then(|i| ABREVIATURAS.get(i))
        .copied()
        .unwrap_or("???");
    format!("{nombre} {anio}")
}

/// Tramo a conservar o guardar: (id_tramo, dias_cedula, dias_obra, obra o None).
pub type PlanGuardado = (Uuid, i16, i16, Option<Uuid>);

/// Clave de movimiento dentro de un reporte: concepto normalizado y fecha.
fn clave_movimiento(t: &ReporteLiquidacionRowDb) -> (String, Option<NaiveDate>) {
    (t.clave.trim().to_uppercase(), t.fecha)
}

/// Mapea el índice del filtro de Slint al enum de dominio (0 = Todos).
fn filtro_desde_indice(idx: i32) -> FiltroAsignacion {
    match idx {
        1 => FiltroAsignacion::SinObra,
        2 => FiltroAsignacion::UnaObra,
        3 => FiltroAsignacion::DosOMas,
        4 => FiltroAsignacion::ConInconsistencia,
        5 => FiltroAsignacion::CedulaActualizada,
        _ => FiltroAsignacion::Todos,
    }
}

fn formatear_fecha(f: Option<NaiveDate>) -> String {
    f.map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_else(|| "—".to_string())
}

/// Vigencia de una obra en formato "dd/mm/aaaa – dd/mm/aaaa".
fn vigencia_texto(o: &ObraOptionDb) -> String {
    format!("{} – {}", formatear_fecha(o.fecha_inicio), formatear_fecha(o.fecha_fin))
}

/// Opciones de obra para la UI: las creadas por el usuario primero, color por posición.
fn obras_opciones(obras: &[ObraOptionDb]) -> Vec<AsigObraOpcion> {
    let mut ordenadas: Vec<(usize, &ObraOptionDb)> = obras.iter().enumerate().collect();
    ordenadas.sort_by_key(|(_, o)| !o.creado_usuario);
    ordenadas
        .into_iter()
        .map(|(idx, o)| AsigObraOpcion {
            id_obra: o.id.to_string().into(),
            numero: o.numero.clone().into(),
            label: o.label.clone().into(),
            vigencia: vigencia_texto(o).into(),
            color_idx: (idx % NUM_COLORES) as i32,
        })
        .collect()
}

fn chip_slint(c: &ChipObra) -> AsigObraChip {
    AsigObraChip {
        id_obra: c.id_obra.to_string().into(),
        numero: c.numero.clone().into(),
        dias: c.dias,
        color_idx: c.color_idx,
    }
}

fn celda_slint(c: &CeldaMes) -> AsigCelda {
    let obras: Vec<AsigObraChip> = c.obras.iter().map(chip_slint).collect();
    AsigCelda {
        existe: true,
        id_reporte: c.id_reporte.to_string().into(),
        etiqueta_mes: etiqueta_mes(c.anio, c.mes).into(),
        dias_cedula: c.dias_cedula,
        disponibles: c.disponibles,
        asignados: c.asignados,
        obras: ModelRc::new(VecModel::from(obras)),
        es_baja: c.es_baja,
        sin_obra: !c.es_baja && c.obras.is_empty(),
        inconsistente: c.inconsistente,
        cedula_actualizada: c.cedula_actualizada,
    }
}

/// Celda de un mes en el que el empleado no tiene reporte.
fn celda_vacia(anio: i16, mes: i16) -> AsigCelda {
    AsigCelda {
        existe: false,
        id_reporte: SharedString::default(),
        etiqueta_mes: etiqueta_mes(anio, mes).into(),
        dias_cedula: 0,
        disponibles: 0,
        asignados: 0,
        obras: ModelRc::new(VecModel::from(Vec::<AsigObraChip>::new())),
        es_baja: false,
        sin_obra: false,
        inconsistente: false,
        cedula_actualizada: false,
    }
}

fn fila_slint(f: &FilaEmpleado, meses: &[(i16, i16)]) -> AsigEmpleadoFila {
    let celdas: Vec<AsigCelda> = f
        .celdas
        .iter()
        .zip(meses.iter())
        .map(|(c, &(anio, mes))| match c {
            Some(c) => celda_slint(c),
            None => celda_vacia(anio, mes),
        })
        .collect();
    AsigEmpleadoFila {
        id_empleado: f.id_empleado.to_string().into(),
        nombre: f.nombre.clone().into(),
        nss: f.nss.clone().into(),
        rfc: f.rfc.clone().into(),
        num_obras: f.num_obras as i32,
        tiene_inconsistencia: f.tiene_inconsistencia,
        cedula_actualizada: f.cedula_actualizada,
        celdas: ModelRc::new(VecModel::from(celdas)),
    }
}

/// Pinta la pestaña "Asignación por empleado" a partir del estado de la sesión del reporte.
pub fn render_asignacion_ui(ui: &MainWindow, state: &ReporteSessionState) {
    let app_data = ui.global::<AppData>();
    let m = construir_matriz(&state.all_rows, &state.obras_options);

    let meses: Vec<SharedString> = m
        .meses
        .iter()
        .map(|&(anio, mes)| etiqueta_mes(anio, mes).into())
        .collect();
    app_data.set_asig_meses(ModelRc::new(VecModel::from(meses)));

    let filtro = filtro_desde_indice(app_data.get_asig_filtro_idx());
    let busqueda = app_data.get_asig_busqueda().to_string();
    let filas: Vec<AsigEmpleadoFila> = filtrar_filas(&m, filtro, &busqueda)
        .into_iter()
        .map(|f| fila_slint(f, &m.meses))
        .collect();
    app_data.set_asig_filas(ModelRc::new(VecModel::from(filas)));
    app_data.set_asig_obras(ModelRc::new(VecModel::from(obras_opciones(&state.obras_options))));

    let r = resumen_avance(&m);
    app_data.set_asig_total(r.total_empleado_mes as i32);
    app_data.set_asig_con_obra(r.con_obra as i32);
    app_data.set_asig_sin_obra(r.sin_obra as i32);
    app_data.set_asig_inconsistencias(r.inconsistencias as i32);
    app_data.set_asig_cedula_actualizada(r.cedula_actualizada as i32);
    app_data.set_asig_porcentaje(r.porcentaje);
    app_data.set_asig_escenario_texto(texto_escenario(&r).into());

    // Detalle del empleado seleccionado, sobre la matriz completa (no filtrada).
    let sel_id = app_data.get_asig_empleado_sel_id().to_string();
    let fila_sel = Uuid::parse_str(&sel_id)
        .ok()
        .and_then(|id| m.filas.iter().find(|f| f.id_empleado == id));
    let detalle: Vec<AsigCelda> = match fila_sel {
        Some(f) => {
            app_data.set_asig_empleado_sel_nombre(f.nombre.clone().into());
            f.celdas.iter().flatten().map(celda_slint).collect()
        }
        None => {
            // La selección ya no existe en el reporte: se limpia.
            app_data.set_asig_empleado_sel_id(SharedString::default());
            app_data.set_asig_empleado_sel_nombre(SharedString::default());
            Vec::new()
        }
    };
    app_data.set_asig_detalle(ModelRc::new(VecModel::from(detalle)));
}

/// Fila de la matriz que contiene el reporte indicado.
fn buscar_fila_de_reporte(m: &MatrizAsignacion, rep: Uuid) -> Option<&FilaEmpleado> {
    m.filas
        .iter()
        .find(|f| f.celdas.iter().flatten().any(|c| c.id_reporte == rep))
}

/// Celda del mes inmediatamente anterior (calendario) al reporte indicado, si existe.
pub fn mes_anterior(fila: &FilaEmpleado, rep: Uuid) -> Option<&CeldaMes> {
    let actual = fila.celdas.iter().flatten().find(|c| c.id_reporte == rep)?;
    let (anio, mes) = if actual.mes == 1 {
        (actual.anio - 1, 12)
    } else {
        (actual.anio, actual.mes - 1)
    };
    fila.celdas
        .iter()
        .flatten()
        .find(|c| c.anio == anio && c.mes == mes)
}

/// Reparte `dias` entre los tramos de una obra en orden cronológico (fecha, luego id),
/// sin pasar del `dias_cedula` de cada tramo. Devuelve (id_tramo, dias_cedula, dias_asignados)
/// para todos los tramos recibidos; los que quedan en 0 días se reportan con 0.
pub fn planear_cambio_dias(
    tramos: &[&ReporteLiquidacionRowDb],
    dias: i32,
) -> Result<Vec<(Uuid, i16, i16)>, String> {
    let mut ordenados: Vec<&ReporteLiquidacionRowDb> = tramos.to_vec();
    ordenados.sort_by(|a, b| a.fecha.cmp(&b.fecha).then(a.id_reporte_liq.cmp(&b.id_reporte_liq)));

    let capacidad: i32 = ordenados.iter().map(|t| i32::from(t.dias_cedula).max(0)).sum();
    if dias < 0 || dias > capacidad {
        return Err(format!(
            "Los días ({dias}) deben estar entre 0 y {capacidad} (días de cédula de los movimientos de la obra)."
        ));
    }

    let mut restante = dias;
    Ok(ordenados
        .iter()
        .map(|t| {
            let tope = i32::from(t.dias_cedula).max(0);
            let asignar = restante.min(tope);
            restante -= asignar;
            // asignar <= tope <= i16::MAX, la conversión es exacta.
            (t.id_reporte_liq, t.dias_cedula, asignar as i16)
        })
        .collect())
}

/// Planea fijar `dias` de una obra en un reporte sin duplicar cuotas: reparte cronológicamente con
/// tope por tramo; un tramo que queda en 0 días se BORRA si su movimiento tiene otro tramo hermano
/// (otra obra o sin obra) y solo pasa a sin obra si es el único tramo del movimiento. Así nunca
/// coexisten una fila sin obra (factor 1) y una con obra para el mismo movimiento.
pub fn planear_cambio_dias_seguro(
    tramos_reporte: &[&ReporteLiquidacionRowDb],
    id_obra: Uuid,
    dias: i32,
) -> Result<(Vec<Uuid>, Vec<PlanGuardado>), String> {
    let tramos_obra: Vec<&ReporteLiquidacionRowDb> = tramos_reporte
        .iter()
        .copied()
        .filter(|t| t.id_registro_obra == Some(id_obra))
        .collect();
    if tramos_obra.is_empty() {
        return Err("el mes no tiene la obra indicada".to_string());
    }
    let plan = planear_cambio_dias(&tramos_obra, dias)?;
    let mut borrar = Vec::new();
    let mut guardar = Vec::new();
    for (id, dias_cedula, dias_asignados) in plan {
        if dias_asignados > 0 {
            guardar.push((id, dias_cedula, dias_asignados, Some(id_obra)));
            continue;
        }
        let tramo = tramos_obra.iter().find(|t| t.id_reporte_liq == id);
        let tiene_hermano = tramo.map_or(false, |t| {
            let clave = clave_movimiento(t);
            tramos_reporte
                .iter()
                .any(|o| o.id_reporte_liq != id && o.id_registro_obra != Some(id_obra) && clave_movimiento(o) == clave)
        });
        if tiene_hermano {
            borrar.push(id);
        } else {
            guardar.push((id, dias_cedula, 0, None));
        }
    }
    Ok((borrar, guardar))
}

/// Conserva un tramo del grupo (el de menor id) como sin obra y marca el resto para borrar.
fn conservar_uno(mut grupo: Vec<&ReporteLiquidacionRowDb>) -> Option<(Vec<Uuid>, PlanGuardado)> {
    grupo.sort_by_key(|t| t.id_reporte_liq);
    let (primero, resto) = grupo.split_first()?;
    let borrar: Vec<Uuid> = resto.iter().map(|t| t.id_reporte_liq).collect();
    Some((borrar, (primero.id_reporte_liq, primero.dias_cedula, 0, None)))
}

/// Planea quitar una obra de un reporte. Devuelve (tramos a borrar, tramos a guardar como sin obra).
///
/// Un tramo de la obra se borra solo si otro tramo del reporte (de otra obra o sin obra) comparte
/// clave y fecha. Si no hay hermano, el movimiento se conserva como sin obra; si la obra tiene
/// varios tramos del mismo movimiento, se conserva uno y se borran los demás.
pub fn planear_quitar(
    tramos_reporte: &[&ReporteLiquidacionRowDb],
    id_obra: Uuid,
) -> (Vec<Uuid>, Vec<PlanGuardado>) {
    let mut grupos: BTreeMap<(String, Option<NaiveDate>), Vec<&ReporteLiquidacionRowDb>> = BTreeMap::new();
    for t in tramos_reporte.iter().filter(|t| t.id_registro_obra == Some(id_obra)) {
        grupos.entry(clave_movimiento(t)).or_default().push(t);
    }

    let mut borrar = Vec::new();
    let mut guardar = Vec::new();
    for (clave, obra_tramos) in grupos {
        let hay_hermano = tramos_reporte
            .iter()
            .any(|o| o.id_registro_obra != Some(id_obra) && clave_movimiento(o) == clave);
        if hay_hermano {
            borrar.extend(obra_tramos.iter().map(|t| t.id_reporte_liq));
        } else if let Some((b, g)) = conservar_uno(obra_tramos) {
            borrar.extend(b);
            guardar.push(g);
        }
    }
    (borrar, guardar)
}

/// Planea dejar un reporte sin obras: por cada movimiento (clave, fecha) conserva un tramo como
/// sin obra y marca los hermanos para borrar. Devuelve (tramos a borrar, tramos a guardar).
pub fn planear_dejar_sin_obra(tramos_reporte: &[&ReporteLiquidacionRowDb]) -> (Vec<Uuid>, Vec<PlanGuardado>) {
    let mut grupos: BTreeMap<(String, Option<NaiveDate>), Vec<&ReporteLiquidacionRowDb>> = BTreeMap::new();
    for t in tramos_reporte {
        grupos.entry(clave_movimiento(t)).or_default().push(t);
    }

    let mut borrar = Vec::new();
    let mut guardar = Vec::new();
    for grupo in grupos.into_values() {
        if let Some((b, g)) = conservar_uno(grupo) {
            borrar.extend(b);
            guardar.push(g);
        }
    }
    (borrar, guardar)
}

fn parsear_uuid(texto: &str, nombre: &str) -> Result<Uuid, String> {
    Uuid::parse_str(texto.trim()).map_err(|_| format!("Identificador de {nombre} inválido."))
}

fn dias_a_i16(dias: i32) -> Result<i16, String> {
    i16::try_from(dias).map_err(|_| format!("Los días ({dias}) están fuera de rango."))
}

/// Re-renderiza solo la pestaña de asignación con el estado actual de la sesión.
fn refrescar_asignacion(ui: &MainWindow, session: &Arc<Mutex<ReporteSessionState>>) {
    if let Ok(st) = session.lock() {
        render_asignacion_ui(ui, &st);
    }
}

/// Actualiza las propiedades de deshacer con el último lote de la revisión.
fn actualizar_deshacer(ui: &MainWindow, ultimo: Option<(Uuid, String)>) {
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
/// Siempre: marca procesando, libera el lock de sesión antes de recargar (la recarga lo toma),
/// actualiza deshacer y muestra un toast con el mensaje o el error.
fn ejecutar_accion<F>(
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
                    log::error!("Asignación por empleado: consulta de bitácora: {e}");
                    None
                }
            };
            (resultado, deshacer)
        }
    };

    app_data.set_asig_procesando(false);
    // La recarga toma el lock de sesión, por eso se liberó arriba.
    sync_and_load_reporte_data(ui, pool, session, &rev_str, false);
    actualizar_deshacer(ui, deshacer);

    match resultado {
        Ok(mensaje) => show_toast_message(ui, &mensaje),
        Err(e) => {
            log::error!("Asignación por empleado ({descripcion}): {e}");
            show_toast_message(ui, &format!("No se pudo {descripcion}: {e}"));
        }
    }
}

/// Registra los callbacks de la pestaña "Asignación por empleado".
pub fn wire_asignacion_callbacks(
    main_window: &MainWindow,
    db_pool: Option<Arc<DbPool>>,
    session: Arc<Mutex<ReporteSessionState>>,
) {
    let app_logic = main_window.global::<AppLogic>();

    // Búsqueda, filtro y selección: solo re-renderizan la pestaña.
    {
        let ui_weak = main_window.as_weak();
        let session = session.clone();
        app_logic.on_asig_buscar(move |texto| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_asig_busqueda(texto);
                refrescar_asignacion(&ui, &session);
            }
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let session = session.clone();
        app_logic.on_asig_filtrar(move |idx| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_asig_filtro_idx(idx);
                refrescar_asignacion(&ui, &session);
            }
        });
    }
    {
        let ui_weak = main_window.as_weak();
        let session = session.clone();
        app_logic.on_asig_seleccionar_empleado(move |id| {
            if let Some(ui) = ui_weak.upgrade() {
                ui.global::<AppData>().set_asig_empleado_sel_id(id);
                refrescar_asignacion(&ui, &session);
            }
        });
    }

    // Vincular: primera obra de la celda reparte días; obra adicional inserta un tramo hermano.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_vincular(move |rep, obra, dias| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let rep_id = match parsear_uuid(&rep, "reporte") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            let obra_id = match parsear_uuid(&obra, "obra") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            if dias <= 0 {
                show_toast_message(&ui, "Los días de obra deben ser mayores a 0.");
                return;
            }
            ejecutar_accion(&ui, &pool, &session, "vincular la obra", move |client, rev, st| {
                let dias16 = dias_a_i16(dias)?;
                let ya_tiene_obra = st
                    .all_rows
                    .iter()
                    .any(|t| t.id_parent_reporte_liq == rep_id && t.id_registro_obra.is_some());
                if ya_tiene_obra {
                    add_obra_vinculacion_for_employee(client, rep_id, obra_id, dias16)?;
                } else {
                    batch_assign_multiple_obras_to_employees(
                        client,
                        rev,
                        &[rep_id],
                        &[obra_id],
                        None,
                        Some(dias16),
                        None,
                    )?;
                }
                Ok(format!("Obra vinculada con {dias} días."))
            });
        });
    }

    // Reemplazar: cambia la obra de los tramos de la obra actual, sin tocar días.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_reemplazar(move |rep, actual, nueva| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let ids = (
                parsear_uuid(&rep, "reporte"),
                parsear_uuid(&actual, "obra"),
                parsear_uuid(&nueva, "obra"),
            );
            let (rep_id, actual_id, nueva_id) = match ids {
                (Ok(a), Ok(b), Ok(c)) => (a, b, c),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                    show_toast_message(&ui, &e);
                    return;
                }
            };
            if actual_id == nueva_id {
                show_toast_message(&ui, "Sin cambios: la obra nueva es la misma.");
                return;
            }
            ejecutar_accion(&ui, &pool, &session, "reemplazar la obra", move |client, _rev, st| {
                let tramos: Vec<PlanGuardado> = st
                    .all_rows
                    .iter()
                    .filter(|t| t.id_parent_reporte_liq == rep_id && t.id_registro_obra == Some(actual_id))
                    .map(|t| (t.id_reporte_liq, t.dias_cedula, t.dias_obra, Some(nueva_id)))
                    .collect();
                if tramos.is_empty() {
                    return Err("el mes no tiene la obra indicada".to_string());
                }
                let n = batch_save_reporte_row_updates(client, &tramos)?;
                Ok(format!("Obra reemplazada en {n} tramo(s)."))
            });
        });
    }

    // Cambiar días: reparte los días de la obra en orden cronológico con tope por tramo.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_cambiar_dias(move |rep, obra, dias| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let rep_id = match parsear_uuid(&rep, "reporte") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            let obra_id = match parsear_uuid(&obra, "obra") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            if dias <= 0 {
                show_toast_message(&ui, "Los días de obra deben ser mayores a 0.");
                return;
            }
            ejecutar_accion(&ui, &pool, &session, "cambiar los días de la obra", move |client, _rev, st| {
                let tramos: Vec<&ReporteLiquidacionRowDb> = st
                    .all_rows
                    .iter()
                    .filter(|t| t.id_parent_reporte_liq == rep_id)
                    .collect();
                let (borrar, guardar) = planear_cambio_dias_seguro(&tramos, obra_id, dias)?;
                // Una sola transacción y un solo lote de bitácora para toda la acción.
                crate::db::asignacion_ops::aplicar_plan_tramos(
                    client,
                    crate::db::bitacora_reportes::ACCION_DIAS,
                    &borrar,
                    &guardar,
                )?;
                Ok(format!(
                    "Días de obra fijados en {dias}; {} tramo(s) actualizados, {} tramo(s) hermanos eliminados.",
                    guardar.len(),
                    borrar.len()
                ))
            });
        });
    }

    // Quitar obra: borra tramos con hermano y deja el resto como sin obra.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_quitar(move |rep, obra| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let rep_id = match parsear_uuid(&rep, "reporte") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            let obra_id = match parsear_uuid(&obra, "obra") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            ejecutar_accion(&ui, &pool, &session, "quitar la obra", move |client, _rev, st| {
                let tramos: Vec<&ReporteLiquidacionRowDb> = st
                    .all_rows
                    .iter()
                    .filter(|t| t.id_parent_reporte_liq == rep_id)
                    .collect();
                if !tramos.iter().any(|t| t.id_registro_obra == Some(obra_id)) {
                    return Err("el mes no tiene la obra indicada".to_string());
                }
                let (borrar, guardar) = planear_quitar(&tramos, obra_id);
                // Una sola transacción y un solo lote de bitácora para toda la acción.
                crate::db::asignacion_ops::aplicar_plan_tramos(
                    client,
                    crate::db::bitacora_reportes::ACCION_DESVINCULAR,
                    &borrar,
                    &guardar,
                )?;
                Ok(format!(
                    "Obra quitada: {} tramo(s) eliminado(s), {} pasan a sin obra.",
                    borrar.len(),
                    guardar.len()
                ))
            });
        });
    }

    // Dejar sin obra: un tramo por movimiento queda sin obra; los hermanos se borran.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_dejar_sin_obra(move |rep| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let rep_id = match parsear_uuid(&rep, "reporte") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            ejecutar_accion(&ui, &pool, &session, "dejar el mes sin obra", move |client, _rev, st| {
                let tramos: Vec<&ReporteLiquidacionRowDb> = st
                    .all_rows
                    .iter()
                    .filter(|t| t.id_parent_reporte_liq == rep_id)
                    .collect();
                if tramos.is_empty() {
                    return Err("el mes no tiene movimientos".to_string());
                }
                let (borrar, guardar) = planear_dejar_sin_obra(&tramos);
                // Una sola transacción y un solo lote de bitácora para toda la acción.
                crate::db::asignacion_ops::aplicar_plan_tramos(
                    client,
                    crate::db::bitacora_reportes::ACCION_DESVINCULAR,
                    &borrar,
                    &guardar,
                )?;
                Ok(format!("Mes sin obra: {} tramo(s) eliminado(s).", borrar.len()))
            });
        });
    }

    // Aplicar las obras de un mes a otros meses del mismo empleado.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_aplicar_a_meses(move |rep, solo_sin_obra| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let rep_id = match parsear_uuid(&rep, "reporte") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            ejecutar_accion(&ui, &pool, &session, "aplicar las obras a otros meses", move |client, rev, st| {
                let m = construir_matriz(&st.all_rows, &st.obras_options);
                let fila = buscar_fila_de_reporte(&m, rep_id)
                    .ok_or_else(|| "el reporte no existe en la matriz".to_string())?;
                let origen = fila
                    .celdas
                    .iter()
                    .flatten()
                    .find(|c| c.id_reporte == rep_id)
                    .ok_or_else(|| "el reporte no existe en la matriz".to_string())?;
                if origen.obras.is_empty() {
                    return Err("el mes origen no tiene obras asignadas".to_string());
                }
                let obras: Vec<Uuid> = origen.obras.iter().map(|c| c.id_obra).collect();
                let destinos: Vec<Uuid> = fila
                    .celdas
                    .iter()
                    .flatten()
                    .filter(|c| {
                        c.id_reporte != rep_id
                            && !c.es_baja
                            && (!solo_sin_obra || c.obras.is_empty())
                    })
                    .map(|c| c.id_reporte)
                    .collect();
                if destinos.is_empty() {
                    return Err("no hay meses destino que cumplan el criterio".to_string());
                }
                batch_assign_multiple_obras_to_employees(client, rev, &destinos, &obras, None, None, None)?;
                Ok(format!(
                    "Obras de {} aplicadas a {} mes(es); la distribución respeta vigencias y movimientos.",
                    etiqueta_mes(origen.anio, origen.mes),
                    destinos.len()
                ))
            });
        });
    }

    // Copiar las obras del mes inmediatamente anterior al mes indicado.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_copiar_mes_anterior(move |rep| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let rep_id = match parsear_uuid(&rep, "reporte") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            ejecutar_accion(&ui, &pool, &session, "copiar el mes anterior", move |client, rev, st| {
                let m = construir_matriz(&st.all_rows, &st.obras_options);
                let fila = buscar_fila_de_reporte(&m, rep_id)
                    .ok_or_else(|| "el reporte no existe en la matriz".to_string())?;
                let actual = fila
                    .celdas
                    .iter()
                    .flatten()
                    .find(|c| c.id_reporte == rep_id)
                    .ok_or_else(|| "el reporte no existe en la matriz".to_string())?;
                if actual.es_baja {
                    return Err("el mes es una baja y no admite obras".to_string());
                }
                let anterior = mes_anterior(fila, rep_id)
                    .ok_or_else(|| "no existe reporte del mes anterior para este empleado".to_string())?;
                if anterior.obras.is_empty() {
                    return Err("el mes anterior no tiene obras asignadas".to_string());
                }
                let obras: Vec<Uuid> = anterior.obras.iter().map(|c| c.id_obra).collect();
                batch_assign_multiple_obras_to_employees(client, rev, &[rep_id], &obras, None, None, None)?;
                Ok(format!(
                    "Obras de {} copiadas a {}.",
                    etiqueta_mes(anterior.anio, anterior.mes),
                    etiqueta_mes(actual.anio, actual.mes)
                ))
            });
        });
    }

    // Revertir al empleado: devuelve sus reportes a la cédula.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_revertir_empleado(move |id_emp| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let emp_id = match parsear_uuid(&id_emp, "empleado") {
                Ok(v) => v,
                Err(e) => { show_toast_message(&ui, &e); return; }
            };
            ejecutar_accion(&ui, &pool, &session, "revertir al empleado", move |client, rev, _st| {
                let n = revertir_reportes_empleado(client, rev, emp_id)?;
                Ok(format!("Empleado revertido a la cédula: {n} reporte(s)."))
            });
        });
    }

    // Deshacer el último lote de la revisión.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        app_logic.on_asig_deshacer(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            ejecutar_accion(&ui, &pool, &session, "deshacer el cambio", |client, rev, _st| {
                match ultimo_lote_revision(client, rev)? {
                    None => Ok("No hay cambios para deshacer.".to_string()),
                    Some((lote, accion)) => {
                        deshacer_lote(client, lote)?;
                        Ok(format!("Se deshizo: {accion}"))
                    }
                }
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

    fn obra(id: u128, numero: &str) -> ObraOptionDb {
        ObraOptionDb {
            id: uid(id),
            numero: numero.to_string(),
            tipo_obra: String::new(),
            fecha_inicio: None,
            fecha_fin: None,
            label: numero.to_string(),
            creado_usuario: false,
            monto: 0.0,
        }
    }

    /// Tramo sin obra, de alta, con valores por defecto.
    fn tramo(
        id: u128,
        reporte: u128,
        empleado: u128,
        nombre: &str,
        anio: i16,
        mes: i16,
        dias_cedula: i16,
    ) -> ReporteLiquidacionRowDb {
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
            anio_periodo: anio,
            periodo_str: format!("{mes:02}/{anio}"),
            dias_cedula,
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

    /// Asigna una obra al tramo.
    fn con_obra(mut t: ReporteLiquidacionRowDb, id_obra: u128, numero: &str, dias_obra: i16) -> ReporteLiquidacionRowDb {
        t.id_registro_obra = Some(uid(id_obra));
        t.obra_numero = numero.to_string();
        t.dias_obra = dias_obra;
        t
    }

    /// Escenario base: 3 empleados, 2 meses, 2 obras.
    fn datos_base() -> (Vec<ReporteLiquidacionRowDb>, Vec<ObraOptionDb>) {
        let obras = vec![obra(100, "001"), obra(101, "002")];

        // Ana: Feb 30d con obra 001 (cédula actualizada); Mar 31d repartidos 15+16 en 001 y 002.
        let mut ana_feb = con_obra(tramo(1, 11, 1, "Ana Lopez", 2026, 2, 30), 100, "001", 30);
        ana_feb.cedula_actualizada = true;
        let ana_mar_1 = con_obra(tramo(2, 12, 1, "Ana Lopez", 2026, 3, 31), 100, "001", 15);
        let ana_mar_2 = con_obra(tramo(3, 12, 1, "Ana Lopez", 2026, 3, 31), 101, "002", 16);

        // Carla: Feb 30d con obra 002 y tramo marcado con inconsistencia.
        let mut carla_feb = con_obra(tramo(4, 21, 2, "Carla Méndez", 2026, 2, 30), 101, "002", 30);
        carla_feb.tiene_inconsistencia = true;

        // Ignacio: Feb 30d con 2 días de licencia y sin obra; Mar es baja.
        let mut ignacio_feb = tramo(5, 31, 3, "Ignacio Nunez", 2026, 2, 30);
        ignacio_feb.lic = 2.0;
        let mut ignacio_mar = tramo(6, 32, 3, "Ignacio Nunez", 2026, 3, 0);
        ignacio_mar.clave = "BAJA".to_string();

        let tramos = vec![ana_feb, ana_mar_1, ana_mar_2, carla_feb, ignacio_feb, ignacio_mar];
        (tramos, obras)
    }

    #[test]
    fn construir_matriz_agrupa_meses_filas_y_chips() {
        let (tramos, obras) = datos_base();
        let m = construir_matriz(&tramos, &obras);

        assert_eq!(m.meses, vec![(2026, 2), (2026, 3)]);
        let nombres: Vec<&str> = m.filas.iter().map(|f| f.nombre.as_str()).collect();
        assert_eq!(nombres, vec!["Ana Lopez", "Carla Méndez", "Ignacio Nunez"]);

        // Ana Feb: un chip de 30 días, consistente.
        let ana = &m.filas[0];
        let feb = ana.celdas[0].as_ref().expect("Ana Feb existe");
        assert_eq!(feb.dias_cedula, 30);
        assert_eq!(feb.disponibles, 30);
        assert_eq!(feb.asignados, 30);
        assert_eq!(feb.obras.len(), 1);
        assert_eq!(feb.obras[0].numero, "001");
        assert_eq!(feb.obras[0].dias, 30);
        assert_eq!(feb.obras[0].ids_tramos, vec![uid(1)]);
        assert_eq!(feb.obras[0].color_idx, 0);
        assert!(feb.ids_tramos_sin_obra.is_empty());
        assert!(!feb.inconsistente);
        assert!(feb.cedula_actualizada);

        // Ana Mar: dos obras (15 + 16) que suman 31 sobre 31 disponibles.
        let mar = ana.celdas[1].as_ref().expect("Ana Mar existe");
        assert_eq!(mar.obras.len(), 2);
        assert_eq!(mar.obras[0].numero, "001");
        assert_eq!(mar.obras[0].dias, 15);
        assert_eq!(mar.obras[0].ids_tramos, vec![uid(2)]);
        assert_eq!(mar.obras[1].numero, "002");
        assert_eq!(mar.obras[1].dias, 16);
        assert_eq!(mar.obras[1].color_idx, 1);
        assert_eq!(mar.asignados, 31);
        assert_eq!(mar.disponibles, 31);
        assert!(!mar.inconsistente);

        // Ana usa dos obras distintas en total.
        assert_eq!(ana.num_obras, 2);
        assert!(ana.cedula_actualizada);
        assert!(!ana.tiene_inconsistencia);

        // Carla: una obra, inconsistencia por bandera del tramo.
        let carla = &m.filas[1];
        assert_eq!(carla.num_obras, 1);
        assert!(carla.celdas[1].is_none());
        let carla_feb = carla.celdas[0].as_ref().expect("Carla Feb existe");
        assert!(carla_feb.inconsistente);
        assert!(carla.tiene_inconsistencia);

        // Ignacio Feb: licencia de 2 días descontada, sin obra.
        let ignacio = &m.filas[2];
        assert_eq!(ignacio.num_obras, 0);
        let ign_feb = ignacio.celdas[0].as_ref().expect("Ignacio Feb existe");
        assert_eq!(ign_feb.disponibles, 28);
        assert_eq!(ign_feb.asignados, 0);
        assert!(ign_feb.obras.is_empty());
        assert_eq!(ign_feb.ids_tramos_sin_obra, vec![uid(5)]);
        assert!(!ign_feb.es_baja);

        // Ignacio Mar: baja, no cuenta como sin obra.
        let ign_mar = ignacio.celdas[1].as_ref().expect("Ignacio Mar existe");
        assert!(ign_mar.es_baja);
        assert!(ign_mar.ids_tramos_sin_obra.is_empty());
    }

    #[test]
    fn color_de_obra_desconocida_es_9() {
        let tramos = vec![con_obra(tramo(1, 11, 1, "Ana", 2026, 1, 30), 555, "999", 30)];
        let m = construir_matriz(&tramos, &[]);
        let chip = &m.filas[0].celdas[0].as_ref().unwrap().obras[0];
        assert_eq!(chip.color_idx, 9);
        assert_eq!(chip.numero, "999");
    }

    #[test]
    fn resumen_y_escenario_del_caso_base() {
        let (tramos, obras) = datos_base();
        let r = resumen_avance(&construir_matriz(&tramos, &obras));

        // Celdas no baja: Ana Feb, Ana Mar, Carla Feb, Ignacio Feb.
        assert_eq!(r.total_empleado_mes, 4);
        assert_eq!(r.con_obra, 3);
        assert_eq!(r.sin_obra, 1);
        assert_eq!(r.inconsistencias, 1);
        assert_eq!(r.cedula_actualizada, 1);
        assert!((r.porcentaje - 75.0).abs() < f32::EPSILON);
        assert_eq!(r.escenario, Escenario::Algunos);
        assert!(texto_escenario(&r).starts_with("1 empleado-mes sin obra"));
    }

    #[test]
    fn umbrales_de_escenario() {
        let obras = vec![obra(100, "001")];
        let armar = |con: usize| {
            let tramos: Vec<ReporteLiquidacionRowDb> = (0..20u128)
                .map(|i| {
                    let t = tramo(1000 + i, 2000 + i, 3000 + i, &format!("Emp {i:02}"), 2026, 1, 30);
                    if (i as usize) < con {
                        con_obra(t, 100, "001", 30)
                    } else {
                        t
                    }
                })
                .collect();
            resumen_avance(&construir_matriz(&tramos, &obras))
        };

        let todos = armar(19); // 95 %
        assert_eq!(todos.escenario, Escenario::TodosVinculados);
        assert!(texto_escenario(&todos).contains("revisa las 0 inconsistencias"));

        let algunos = armar(10); // 50 %
        assert_eq!(algunos.escenario, Escenario::Algunos);

        let ninguno = armar(1); // 5 %
        assert_eq!(ninguno.escenario, Escenario::NingunoOCasiNinguno);
        assert_eq!(
            texto_escenario(&ninguno),
            "Casi ningún empleado tiene obra: usa el asistente de asignación masiva."
        );
    }

    #[test]
    fn filtros_y_busqueda() {
        let (tramos, obras) = datos_base();
        let m = construir_matriz(&tramos, &obras);
        let nombres = |v: Vec<&FilaEmpleado>| -> Vec<String> { v.iter().map(|f| f.nombre.clone()).collect() };

        assert_eq!(filtrar_filas(&m, FiltroAsignacion::Todos, "").len(), 3);
        assert_eq!(nombres(filtrar_filas(&m, FiltroAsignacion::SinObra, "")), vec!["Ignacio Nunez"]);
        assert_eq!(nombres(filtrar_filas(&m, FiltroAsignacion::UnaObra, "")), vec!["Carla Méndez"]);
        assert_eq!(nombres(filtrar_filas(&m, FiltroAsignacion::DosOMas, "")), vec!["Ana Lopez"]);
        assert_eq!(nombres(filtrar_filas(&m, FiltroAsignacion::ConInconsistencia, "")), vec!["Carla Méndez"]);
        assert_eq!(nombres(filtrar_filas(&m, FiltroAsignacion::CedulaActualizada, "")), vec!["Ana Lopez"]);

        // Búsqueda sin acentos ni mayúsculas, y por NSS.
        assert_eq!(nombres(filtrar_filas(&m, FiltroAsignacion::Todos, "MENDEZ")), vec!["Carla Méndez"]);
        assert_eq!(nombres(filtrar_filas(&m, FiltroAsignacion::Todos, "  nunez ")), vec!["Ignacio Nunez"]);
        assert_eq!(nombres(filtrar_filas(&m, FiltroAsignacion::Todos, "NSS1")), vec!["Ana Lopez"]);
        assert!(filtrar_filas(&m, FiltroAsignacion::Todos, "inexistente").is_empty());
    }

    /// Tramo de un movimiento concreto (clave ALTA y día de marzo 2026) con obra opcional.
    fn movimiento(id: u128, reporte: u128, dia: u32, obra: Option<u128>, dias_cedula: i16) -> ReporteLiquidacionRowDb {
        let mut t = tramo(id, reporte, 1, "Ana Lopez", 2026, 3, dias_cedula);
        t.fecha = NaiveDate::from_ymd_opt(2026, 3, dia);
        if let Some(o) = obra {
            t.id_registro_obra = Some(uid(o));
            t.obra_numero = format!("{o:03}");
        }
        t
    }

    #[test]
    fn planear_cambio_dias_reparte_en_orden_cronologico_con_tope() {
        let a = movimiento(1, 10, 1, Some(100), 10);
        let b = movimiento(2, 10, 16, Some(100), 15);
        let c = movimiento(3, 10, 5, Some(100), 20);
        let refs = vec![&b, &a, &c];

        // Capacidad 45: día 1 toma 10, día 5 toma 20, día 16 queda en 0.
        let plan = planear_cambio_dias(&refs, 30).expect("reparto válido");
        assert_eq!(plan, vec![(uid(1), 10, 10), (uid(3), 20, 20), (uid(2), 15, 0)]);

        // Exacto a la capacidad: todos los tramos llenos.
        let total = planear_cambio_dias(&refs, 45).expect("capacidad completa");
        assert_eq!(total.iter().map(|p| i32::from(p.2)).sum::<i32>(), 45);

        // Por encima de la capacidad se rechaza.
        assert!(planear_cambio_dias(&refs, 46).is_err());
        assert!(planear_cambio_dias(&refs, -1).is_err());
    }

    #[test]
    fn planear_quitar_sin_hermano_conserva_el_movimiento_como_sin_obra() {
        let x = movimiento(1, 10, 1, Some(100), 30);
        let (borrar, guardar) = planear_quitar(&[&x], uid(100));
        assert!(borrar.is_empty());
        assert_eq!(guardar, vec![(uid(1), 30, 0, None)]);
    }

    #[test]
    fn cambio_dias_tramo_en_cero_con_hermano_se_borra_no_duplica_cuota() {
        // Obra 100 en dos movimientos (día 1: 10 d, día 2: 20 d); en el día 2 existe una hermana con obra 101.
        // Fijar 8 días: el tramo del día 2 queda en 0 y, como tiene hermana, se BORRA (no pasa a sin obra,
        // lo que dejaría una fila al 100% junto a la de la obra 101).
        let x1 = movimiento(1, 10, 1, Some(100), 10);
        let x2 = movimiento(2, 10, 2, Some(100), 20);
        let y = movimiento(3, 10, 2, Some(101), 20);
        let (borrar, guardar) = planear_cambio_dias_seguro(&[&x1, &x2, &y], uid(100), 8).expect("plan");
        assert_eq!(borrar, vec![uid(2)]);
        assert_eq!(guardar, vec![(uid(1), 10, 8, Some(uid(100)))]);
    }

    #[test]
    fn cambio_dias_tramo_en_cero_sin_hermano_queda_sin_obra() {
        let x1 = movimiento(1, 10, 1, Some(100), 10);
        let x2 = movimiento(2, 10, 2, Some(100), 20);
        let (borrar, guardar) = planear_cambio_dias_seguro(&[&x1, &x2], uid(100), 8).expect("plan");
        assert!(borrar.is_empty());
        assert_eq!(guardar, vec![(uid(1), 10, 8, Some(uid(100))), (uid(2), 20, 0, None)]);
    }

    #[test]
    fn planear_quitar_con_hermano_borra_solo_el_tramo_de_la_obra() {
        let x = movimiento(1, 10, 1, Some(100), 30);
        let y = movimiento(2, 10, 1, Some(101), 30);
        let (borrar, guardar) = planear_quitar(&[&x, &y], uid(100));
        assert_eq!(borrar, vec![uid(1)]);
        assert!(guardar.is_empty());
    }

    #[test]
    fn planear_quitar_no_borra_el_unico_tramo_de_un_movimiento() {
        // Dos tramos de la misma obra para el mismo movimiento y sin hermano externo:
        // se conserva uno como sin obra y solo se borra el duplicado.
        let x1 = movimiento(1, 10, 1, Some(100), 30);
        let x2 = movimiento(2, 10, 1, Some(100), 30);
        let (borrar, guardar) = planear_quitar(&[&x1, &x2], uid(100));
        assert_eq!(borrar, vec![uid(2)]);
        assert_eq!(guardar, vec![(uid(1), 30, 0, None)]);
    }

    #[test]
    fn planear_dejar_sin_obra_conserva_un_tramo_por_movimiento() {
        // Dos obras en el mismo movimiento (día 1) y otro movimiento distinto (día 2).
        let x = movimiento(1, 10, 1, Some(100), 30);
        let y = movimiento(2, 10, 1, Some(101), 30);
        let w = movimiento(3, 10, 2, Some(100), 15);
        let (borrar, guardar) = planear_dejar_sin_obra(&[&x, &y, &w]);
        assert_eq!(borrar, vec![uid(2)]);
        assert_eq!(guardar, vec![(uid(1), 30, 0, None), (uid(3), 15, 0, None)]);
    }

    #[test]
    fn mes_anterior_inexistente_devuelve_none() {
        // Feb (reporte 11) y Abr (reporte 12): falta Mar, así que Abr no tiene mes anterior.
        let feb = tramo(1, 11, 1, "Ana Lopez", 2026, 2, 30);
        let abr = tramo(2, 12, 1, "Ana Lopez", 2026, 4, 30);
        let m = construir_matriz(&[feb, abr], &[]);
        let fila = &m.filas[0];
        assert!(mes_anterior(fila, uid(12)).is_none());
        assert!(mes_anterior(fila, uid(11)).is_none());
    }

    #[test]
    fn mes_anterior_encuentra_el_mes_calendario_previo() {
        // Feb (11), Mar (13) y Abr (12): el anterior de Abr es Mar y el de Mar es Feb.
        let feb = tramo(1, 11, 1, "Ana Lopez", 2026, 2, 30);
        let abr = tramo(2, 12, 1, "Ana Lopez", 2026, 4, 30);
        let mar = tramo(3, 13, 1, "Ana Lopez", 2026, 3, 30);
        let m = construir_matriz(&[feb, abr, mar], &[]);
        let fila = &m.filas[0];
        assert_eq!(mes_anterior(fila, uid(12)).map(|c| c.id_reporte), Some(uid(13)));
        assert_eq!(mes_anterior(fila, uid(13)).map(|c| c.id_reporte), Some(uid(11)));
    }

    #[test]
    fn etiqueta_mes_en_espanol() {
        assert_eq!(etiqueta_mes(2026, 3), "MAR 2026");
        assert_eq!(etiqueta_mes(2025, 12), "DIC 2025");
        assert_eq!(etiqueta_mes(2026, 13), "??? 2026");
    }
}
