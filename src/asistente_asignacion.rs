//! Asistente de asignación masiva (escenario C): asigna obras a muchos empleado-mes de una vez
//! cuando la cédula no trae obras.
//!
//! Los planificadores son funciones puras sobre la matriz (no tocan BD). El lector de archivos
//! y los callbacks de Slint son la capa de E/S. Las escrituras usan
//! `batch_assign_multiple_obras_to_employees` (bitácora y prorrateo incluidos), agrupando
//! reportes por conjunto de obras y días para minimizar llamadas.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use calamine::{open_workbook_auto, Data, Reader};
use postgres::Client;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use uuid::Uuid;

use crate::asignacion_view_logic::{construir_matriz, etiqueta_mes, CeldaMes, FilaEmpleado, MatrizAsignacion};
use crate::db::bitacora_reportes::ultimo_lote_revision;
use crate::db::reporte_liquidaciones::{batch_assign_multiple_obras_to_employees, ObraOptionDb};
use crate::db::DbPool;
use crate::reporte_view_logic::{sync_and_load_reporte_data, ReporteSessionState};
use crate::{show_toast_message, AppData, AppLogic, AsigAsistentePrevia, MainWindow};

/// Renglones de la vista previa que se envían a la UI.
const MAX_PREVIA_VISIBLE: usize = 500;
/// Máximo de renglones inválidos listados al leer un archivo.
const MAX_ERRORES_LISTADOS: usize = 20;

/// Fila leída de CSV/XLSX: un empleado-mes con una obra y días opcionales.
#[derive(Debug, Clone, PartialEq)]
pub struct FilaImportada {
    pub nss: String,
    pub anio: i16,
    pub mes: i16,
    pub obra: String,
    pub dias: Option<i16>,
}

/// Renglón de la vista previa.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviaFila {
    pub nombre: String,
    pub nss: String,
    pub etiqueta_mes: String,
    pub obras: String,
    pub resultado: String,
    pub omitido: bool,
}

/// Plan calculado por una regla: grupos (obras, reportes, días si hay una sola obra) y vista previa.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanAsistente {
    pub grupos: Vec<(Vec<Uuid>, Vec<Uuid>, Option<i16>)>,
    pub previa: Vec<PreviaFila>,
    pub aplicables: usize,
    pub omitidos: usize,
}

/// Asignación de una revisión de origen por empleado-mes (regla de copia).
#[derive(Debug, Clone, PartialEq)]
pub struct AsignacionOrigen {
    pub id_empleado: Uuid,
    pub anio: i16,
    pub mes: i16,
    pub numeros_obra: Vec<String>,
}

// ---------------------------------------------------------------------------
// Lectura de archivos
// ---------------------------------------------------------------------------

/// Lee un CSV (separador `,` o `;`) o XLSX (primera hoja) con columnas NSS, AÑO, MES, OBRA y DÍAS (opcional).
///
/// Cualquier renglón inválido hace fallar la lectura; el mensaje indica los renglones afectados.
pub fn leer_archivo_importacion(path: &Path) -> Result<Vec<FilaImportada>, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let filas = match ext.as_str() {
        "csv" => {
            let bytes = std::fs::read(path).map_err(|e| format!("No se pudo leer el archivo: {e}"))?;
            tabla_csv(&String::from_utf8_lossy(&bytes))
        }
        "xlsx" | "xlsm" => tabla_xlsx(path)?,
        _ => return Err("Formato no soportado: use un archivo .csv o .xlsx".to_string()),
    };
    interpretar_filas(&filas)
}

/// Tabla cruda: pares (número de renglón base 1, celdas). El primer elemento es el encabezado.
type TablaCruda = Vec<(usize, Vec<String>)>;

/// Parsea CSV detectando `;` o `,` por la primera línea no vacía. Omite renglones vacíos.
fn tabla_csv(texto: &str) -> TablaCruda {
    let texto = texto.strip_prefix('\u{feff}').unwrap_or(texto);
    let primera = texto.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let sep = if primera.matches(';').count() > primera.matches(',').count() {
        ';'
    } else {
        ','
    };
    texto
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| (i + 1, dividir_linea(l, sep)))
        .collect()
}

/// Divide una línea CSV respetando campos entre comillas y comillas escapadas (`""`).
fn dividir_linea(linea: &str, sep: char) -> Vec<String> {
    let mut campos = Vec::new();
    let mut actual = String::new();
    let mut en_comillas = false;
    let mut chars = linea.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if en_comillas && chars.peek() == Some(&'"') => {
                actual.push('"');
                chars.next();
            }
            '"' => en_comillas = !en_comillas,
            c if c == sep && !en_comillas => {
                campos.push(actual.trim().to_string());
                actual.clear();
            }
            c => actual.push(c),
        }
    }
    campos.push(actual.trim().to_string());
    campos
}

/// Lee la primera hoja de un libro Excel. Omite renglones totalmente vacíos.
fn tabla_xlsx(path: &Path) -> Result<TablaCruda, String> {
    let mut libro = open_workbook_auto(path).map_err(|e| format!("No se pudo abrir el libro: {e}"))?;
    let hoja = libro
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| "El libro no tiene hojas".to_string())?;
    let rango = libro
        .worksheet_range(&hoja)
        .map_err(|e| format!("No se pudo leer la hoja '{hoja}': {e}"))?;
    let inicio = rango.start().map(|(r, _)| r as usize).unwrap_or(0);
    Ok(rango
        .rows()
        .enumerate()
        .map(|(i, fila)| {
            let celdas: Vec<String> = fila.iter().map(celda_a_texto).collect();
            (inicio + i + 1, celdas)
        })
        .filter(|(_, celdas)| celdas.iter().any(|c| !c.is_empty()))
        .collect())
}

/// Convierte una celda de Excel a texto. Los números enteros guardados como flotante no llevan decimales.
fn celda_a_texto(celda: &Data) -> String {
    match celda {
        Data::String(s) => s.trim().to_string(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{f:.0}")
            } else {
                f.to_string()
            }
        }
        Data::Bool(b) => b.to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::DateTime(_) | Data::Error(_) | Data::Empty => String::new(),
    }
}

/// Interpreta la tabla cruda: mapea encabezados en cualquier orden y valida cada renglón.
fn interpretar_filas(filas: &TablaCruda) -> Result<Vec<FilaImportada>, String> {
    let (_, encabezado) = filas
        .first()
        .ok_or_else(|| "El archivo está vacío".to_string())?;

    let buscar = |claves: &[&str]| {
        encabezado
            .iter()
            .position(|h| claves.contains(&normalizar_encabezado(h).as_str()))
    };
    let c_nss = buscar(&["nss"]);
    let c_anio = buscar(&["anio", "ano"]);
    let c_mes = buscar(&["mes"]);
    let c_obra = buscar(&["obra"]);
    let c_dias = buscar(&["dias"]);

    let (Some(c_nss), Some(c_anio), Some(c_mes), Some(c_obra)) = (c_nss, c_anio, c_mes, c_obra) else {
        let faltan: Vec<&str> = [
            ("NSS", c_nss.is_none()),
            ("AÑO", c_anio.is_none()),
            ("MES", c_mes.is_none()),
            ("OBRA", c_obra.is_none()),
        ]
        .iter()
        .filter(|(_, falta)| *falta)
        .map(|(nombre, _)| *nombre)
        .collect();
        return Err(format!(
            "Faltan columnas obligatorias: {}. Columnas: NSS, AÑO, MES, OBRA, DÍAS (opcional).",
            faltan.join(", ")
        ));
    };

    let mut salida = Vec::new();
    let mut errores = Vec::new();
    for (renglon, celdas) in filas.iter().skip(1) {
        let celda = |i: usize| celdas.get(i).map(|s| s.as_str()).unwrap_or("");
        let dias = c_dias.map(|i| celda(i)).unwrap_or("");
        match fila_desde(celda(c_nss), celda(c_anio), celda(c_mes), celda(c_obra), dias) {
            Ok(f) => salida.push(f),
            Err(motivo) => errores.push(format!("Renglón {renglon}: {motivo}")),
        }
    }

    if !errores.is_empty() {
        let total = errores.len();
        let mut detalle: Vec<String> = errores.into_iter().take(MAX_ERRORES_LISTADOS).collect();
        if total > MAX_ERRORES_LISTADOS {
            detalle.push(format!("… y {} renglón(es) más", total - MAX_ERRORES_LISTADOS));
        }
        return Err(format!("{total} renglón(es) inválido(s):\n{}", detalle.join("\n")));
    }
    if salida.is_empty() {
        return Err("El archivo no contiene filas de datos".to_string());
    }
    Ok(salida)
}

/// Valida y convierte los valores de un renglón.
fn fila_desde(nss: &str, anio: &str, mes: &str, obra: &str, dias: &str) -> Result<FilaImportada, String> {
    let nss = nss.trim().to_string();
    if nss.len() != 11 || !nss.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("NSS '{nss}' debe tener 11 dígitos"));
    }
    let anio: i16 = anio
        .trim()
        .parse()
        .map_err(|_| format!("AÑO '{}' no es un número", anio.trim()))?;
    if !(1990..=2100).contains(&anio) {
        return Err(format!("AÑO {anio} fuera de rango"));
    }
    let mes_num = crate::cedulas::business::month_to_num(mes);
    if !(1..=12).contains(&mes_num) {
        return Err(format!("MES '{}' no reconocido", mes.trim()));
    }
    let obra = obra.trim();
    if obra.is_empty() {
        return Err("OBRA vacía".to_string());
    }
    let dias = if dias.trim().is_empty() {
        None
    } else {
        let d: i16 = dias
            .trim()
            .parse()
            .map_err(|_| format!("DÍAS '{}' no es un entero", dias.trim()))?;
        if d <= 0 {
            return Err(format!("DÍAS ({d}) debe ser mayor a 0"));
        }
        Some(d)
    };
    Ok(FilaImportada {
        nss,
        anio,
        mes: mes_num as i16,
        obra: obra.to_string(),
        dias,
    })
}

/// Encabezado en minúsculas, sin acentos ni separadores (p. ej. "DÍAS" -> "dias").
fn normalizar_encabezado(s: &str) -> String {
    s.trim()
        .chars()
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
        .filter(|c| c.is_alphanumeric())
        .collect()
}

// ---------------------------------------------------------------------------
// Planificadores puros
// ---------------------------------------------------------------------------

/// Acumulador común a los planificadores: agrupa reportes aplicables y lleva la vista previa.
#[derive(Default)]
struct Constructor {
    grupos: BTreeMap<(Vec<Uuid>, Option<i16>), Vec<Uuid>>,
    previa: Vec<PreviaFila>,
    aplicables: usize,
    omitidos: usize,
}

impl Constructor {
    /// Registra un reporte a aplicar. Las obras se ordenan y deduplican para formar la clave del grupo.
    fn asignar(&mut self, id_reporte: Uuid, mut obras: Vec<Uuid>, dias: Option<i16>, fila: PreviaFila) {
        obras.sort();
        obras.dedup();
        self.grupos.entry((obras, dias)).or_default().push(id_reporte);
        self.previa.push(fila);
        self.aplicables += 1;
    }

    /// Registra un renglón omitido con su motivo.
    fn omitir(&mut self, fila: PreviaFila) {
        self.previa.push(fila);
        self.omitidos += 1;
    }

    fn finalizar(self) -> PlanAsistente {
        PlanAsistente {
            grupos: self
                .grupos
                .into_iter()
                .map(|((obras, dias), reportes)| (obras, reportes, dias))
                .collect(),
            previa: self.previa,
            aplicables: self.aplicables,
            omitidos: self.omitidos,
        }
    }
}

/// Recorre todas las celdas (empleado-mes) de la matriz junto con su fila de empleado.
fn celdas_de(m: &MatrizAsignacion) -> impl Iterator<Item = (&FilaEmpleado, &CeldaMes)> {
    m.filas
        .iter()
        .flat_map(|f| f.celdas.iter().flatten().map(move |c| (f, c)))
}

/// Motivo por el que una celda no se asigna, o `None` si es elegible.
fn motivo_exclusion(c: &CeldaMes, solo_sin_obra: bool) -> Option<&'static str> {
    if c.es_baja {
        Some("baja: no se asigna obra")
    } else if solo_sin_obra && !c.obras.is_empty() {
        Some("ya tiene obra asignada")
    } else {
        None
    }
}

/// Verbo de resultado para una celda elegible.
fn verbo(c: &CeldaMes) -> &'static str {
    if c.obras.is_empty() {
        "Asignar"
    } else {
        "Reasignar (reemplaza obras actuales)"
    }
}

fn previa_de(f: &FilaEmpleado, c: &CeldaMes, obras: String, resultado: String, omitido: bool) -> PreviaFila {
    PreviaFila {
        nombre: f.nombre.clone(),
        nss: f.nss.clone(),
        etiqueta_mes: etiqueta_mes(c.anio, c.mes),
        obras,
        resultado,
        omitido,
    }
}

fn numeros_por_id(obras: &[ObraOptionDb]) -> HashMap<Uuid, String> {
    obras.iter().map(|o| (o.id, o.numero.clone())).collect()
}

/// Índice número normalizado -> id; si un número se repite, gana la primera obra.
fn indice_obras_por_numero(obras: &[ObraOptionDb]) -> HashMap<String, Uuid> {
    let mut idx = HashMap::new();
    for o in obras {
        idx.entry(norm_clave(&o.numero)).or_insert(o.id);
    }
    idx
}

fn norm_clave(s: &str) -> String {
    s.trim().to_uppercase()
}

fn obras_texto(ids: &[Uuid], numeros: &HashMap<Uuid, String>) -> String {
    let mut nombres: Vec<&str> = ids
        .iter()
        .map(|id| numeros.get(id).map(|s| s.as_str()).unwrap_or("?"))
        .collect();
    nombres.sort();
    nombres.join(", ")
}

/// Regla 0: asigna todas las obras de la revisión; el algoritmo reparte cada día por vigencia.
pub fn planear_por_vigencia(m: &MatrizAsignacion, obras: &[ObraOptionDb], solo_sin_obra: bool) -> PlanAsistente {
    let ids: Vec<Uuid> = obras.iter().map(|o| o.id).collect();
    let texto = format!("Vigentes en cada día ({})", ids.len());
    let mut acc = Constructor::default();
    for (f, c) in celdas_de(m) {
        if let Some(motivo) = motivo_exclusion(c, solo_sin_obra) {
            acc.omitir(previa_de(f, c, String::new(), motivo.to_string(), true));
        } else if ids.is_empty() {
            acc.omitir(previa_de(f, c, String::new(), "la revisión no tiene obras".to_string(), true));
        } else {
            let fila = previa_de(f, c, texto.clone(), verbo(c).to_string(), false);
            acc.asignar(c.id_reporte, ids.clone(), None, fila);
        }
    }
    acc.finalizar()
}

/// Regla 1: asigna una sola obra a todos los empleado-mes de los meses indicados.
pub fn planear_todos_a_una_obra(
    m: &MatrizAsignacion,
    obras: &[ObraOptionDb],
    obra_id: Uuid,
    meses_rango: &[(i16, i16)],
    solo_sin_obra: bool,
) -> PlanAsistente {
    let numeros = numeros_por_id(obras);
    let mut acc = Constructor::default();
    for (f, c) in celdas_de(m) {
        if !meses_rango.contains(&(c.anio, c.mes)) {
            continue;
        }
        if let Some(motivo) = motivo_exclusion(c, solo_sin_obra) {
            acc.omitir(previa_de(f, c, String::new(), motivo.to_string(), true));
            continue;
        }
        match numeros.get(&obra_id) {
            None => acc.omitir(previa_de(
                f,
                c,
                String::new(),
                "la obra no existe en la revisión".to_string(),
                true,
            )),
            Some(numero) => {
                let fila = previa_de(f, c, numero.clone(), verbo(c).to_string(), false);
                acc.asignar(c.id_reporte, vec![obra_id], None, fila);
            }
        }
    }
    acc.finalizar()
}

/// Reportes agrupados por el archivo de importación antes de decidir si se aplican.
struct Agrupado<'a> {
    fila: &'a FilaEmpleado,
    celda: &'a CeldaMes,
    /// Pares (obra, días) tomados de cada renglón válido del reporte.
    pares: Vec<(Uuid, Option<i16>)>,
    /// Números de obra del archivo que no existen en la revisión actual.
    faltantes: BTreeSet<String>,
}

/// Regla 2: asigna las obras indicadas en el archivo. Varias filas del mismo reporte suman obras;
/// los días del archivo solo se usan cuando el reporte termina con una sola obra.
pub fn planear_importacion(
    m: &MatrizAsignacion,
    obras: &[ObraOptionDb],
    filas: &[FilaImportada],
    solo_sin_obra: bool,
) -> PlanAsistente {
    let numeros = numeros_por_id(obras);
    let por_numero = indice_obras_por_numero(obras);

    let mut celda_por_clave: HashMap<(String, i16, i16), (&FilaEmpleado, &CeldaMes)> = HashMap::new();
    for (f, c) in celdas_de(m) {
        celda_por_clave
            .entry((f.nss.clone(), c.anio, c.mes))
            .or_insert((f, c));
    }

    let mut agrupados: BTreeMap<Uuid, Agrupado> = BTreeMap::new();
    let mut sin_reporte: Vec<PreviaFila> = Vec::new();
    for fi in filas {
        let clave = (fi.nss.clone(), fi.anio, fi.mes);
        let Some(&(f, c)) = celda_por_clave.get(&clave) else {
            sin_reporte.push(PreviaFila {
                nombre: "—".to_string(),
                nss: fi.nss.clone(),
                etiqueta_mes: etiqueta_mes(fi.anio, fi.mes),
                obras: fi.obra.clone(),
                resultado: "empleado/mes no encontrado en la revisión".to_string(),
                omitido: true,
            });
            continue;
        };
        let g = agrupados.entry(c.id_reporte).or_insert_with(|| Agrupado {
            fila: f,
            celda: c,
            pares: Vec::new(),
            faltantes: BTreeSet::new(),
        });
        match por_numero.get(&norm_clave(&fi.obra)) {
            Some(&id) => g.pares.push((id, fi.dias)),
            None => {
                g.faltantes.insert(fi.obra.trim().to_string());
            }
        }
    }

    let mut lista: Vec<Agrupado> = agrupados.into_values().collect();
    lista.sort_by(|a, b| {
        a.fila
            .nombre
            .to_lowercase()
            .cmp(&b.fila.nombre.to_lowercase())
            .then(a.celda.anio.cmp(&b.celda.anio))
            .then(a.celda.mes.cmp(&b.celda.mes))
    });

    let mut acc = Constructor::default();
    for g in lista {
        let (f, c) = (g.fila, g.celda);
        let mut ids: Vec<Uuid> = g.pares.iter().map(|(id, _)| *id).collect();
        ids.sort();
        ids.dedup();
        let dias = if ids.len() == 1 {
            g.pares.iter().rev().find_map(|(_, d)| *d)
        } else {
            None
        };
        let faltantes = g.faltantes.iter().cloned().collect::<Vec<_>>().join(", ");
        let obras_txt = obras_texto(&ids, &numeros);

        if let Some(motivo) = motivo_exclusion(c, solo_sin_obra) {
            acc.omitir(previa_de(f, c, obras_txt, motivo.to_string(), true));
        } else if ids.is_empty() {
            let motivo = format!("ninguna obra existe en la revisión: {faltantes}");
            acc.omitir(previa_de(f, c, obras_txt, motivo, true));
        } else {
            let sufijo = if faltantes.is_empty() {
                String::new()
            } else {
                format!(" · omite obra(s) inexistente(s): {faltantes}")
            };
            let fila = previa_de(f, c, obras_txt, format!("{}{sufijo}", verbo(c)), false);
            acc.asignar(c.id_reporte, ids, dias, fila);
        }
    }
    for fila in sin_reporte {
        acc.omitir(fila);
    }
    acc.finalizar()
}

/// Regla 3: copia las obras de cada empleado-mes desde otra revisión del mismo registro patronal.
/// Las obras de origen que no existen en la revisión actual se omiten y se indican en el resultado.
pub fn planear_copia(
    m: &MatrizAsignacion,
    obras: &[ObraOptionDb],
    asignaciones: &[AsignacionOrigen],
    solo_sin_obra: bool,
) -> PlanAsistente {
    let numeros = numeros_por_id(obras);
    let por_numero = indice_obras_por_numero(obras);
    let origen: HashMap<(Uuid, i16, i16), &AsignacionOrigen> = asignaciones
        .iter()
        .map(|a| ((a.id_empleado, a.anio, a.mes), a))
        .collect();

    let mut acc = Constructor::default();
    for (f, c) in celdas_de(m) {
        if let Some(motivo) = motivo_exclusion(c, solo_sin_obra) {
            acc.omitir(previa_de(f, c, String::new(), motivo.to_string(), true));
            continue;
        }
        let Some(src) = origen.get(&(f.id_empleado, c.anio, c.mes)) else {
            acc.omitir(previa_de(
                f,
                c,
                String::new(),
                "sin asignación en la revisión de origen".to_string(),
                true,
            ));
            continue;
        };

        let mut ids: Vec<Uuid> = Vec::new();
        let mut faltantes: BTreeSet<String> = BTreeSet::new();
        for numero in &src.numeros_obra {
            match por_numero.get(&norm_clave(numero)) {
                Some(&id) => ids.push(id),
                None => {
                    faltantes.insert(numero.clone());
                }
            }
        }
        ids.sort();
        ids.dedup();
        let faltantes_txt = faltantes.iter().cloned().collect::<Vec<_>>().join(", ");

        if ids.is_empty() {
            let motivo = format!("ninguna obra de origen existe en la revisión actual: {faltantes_txt}");
            acc.omitir(previa_de(f, c, String::new(), motivo, true));
            continue;
        }
        let sufijo = if faltantes.is_empty() {
            String::new()
        } else {
            format!(" · omite obra(s) inexistente(s): {faltantes_txt}")
        };
        let obras_txt = obras_texto(&ids, &numeros);
        let fila = previa_de(f, c, obras_txt, format!("{}{sufijo}", verbo(c)), false);
        acc.asignar(c.id_reporte, ids, None, fila);
    }
    acc.finalizar()
}

// ---------------------------------------------------------------------------
// Acceso a BD y estado de los callbacks
// ---------------------------------------------------------------------------

/// Estado compartido del asistente entre callbacks.
#[derive(Default)]
struct EstadoAsistente {
    /// Plan calculado en la vista previa y pendiente de aplicar.
    plan: Option<PlanAsistente>,
    /// Ids de las revisiones del mismo registro patronal, en el mismo orden que la lista de la UI.
    revisiones: Vec<Uuid>,
}

/// Otras revisiones del mismo registro patronal: (id, nombre).
fn consultar_revisiones_rp(client: &mut Client, rev: Uuid) -> Result<Vec<(Uuid, Option<String>)>, String> {
    let filas = client
        .query(
            "SELECT r.id, r.nombre FROM public.revisiones r \
             WHERE r.id_registro_patronal = (SELECT id_registro_patronal FROM public.revisiones WHERE id = $1) \
               AND r.id <> $1 ORDER BY r.created_at DESC",
            &[&rev],
        )
        .map_err(|e| format!("consulta de revisiones: {e}"))?;
    Ok(filas.iter().map(|r| (r.get(0), r.get(1))).collect())
}

/// Asignaciones (obras por empleado-mes) de una revisión de origen.
fn consultar_asignaciones(client: &mut Client, origen: Uuid) -> Result<Vec<AsignacionOrigen>, String> {
    let filas = client
        .query(
            "SELECT rl.id_empleado, rl.anio_periodo, rl.mes_periodo_int, \
                    array_agg(DISTINCT BTRIM(UPPER(ro.numero))) \
             FROM public.reportes_liquidaciones rl \
             JOIN public.reportes_detalles_liquidaciones d ON d.id_reporte_liquidacion = rl.id \
             JOIN public.registros_obras ro ON ro.id = d.id_registro_obra \
             WHERE rl.id_revision = $1 \
             GROUP BY 1, 2, 3",
            &[&origen],
        )
        .map_err(|e| format!("consulta de asignaciones de origen: {e}"))?;
    Ok(filas
        .iter()
        .map(|r| AsignacionOrigen {
            id_empleado: r.get(0),
            anio: r.get(1),
            mes: r.get(2),
            numeros_obra: r
                .get::<_, Vec<Option<String>>>(3)
                .into_iter()
                .flatten()
                .filter(|s| !s.trim().is_empty())
                .collect(),
        })
        .collect())
}

/// Revisión activa de la sesión, si es válida.
fn revision_actual(session: &Arc<Mutex<ReporteSessionState>>) -> Option<Uuid> {
    session
        .lock()
        .ok()
        .and_then(|st| st.current_revision_id.clone())
        .and_then(|s| Uuid::parse_str(&s).ok())
}

/// Id de la obra elegida en el modal, tomado del mismo modelo que muestra la UI.
fn obra_seleccionada(app: &AppData) -> Result<Uuid, String> {
    let idx = app.get_asig_asistente_obra_idx();
    app.get_asig_obras()
        .row_data(idx as usize)
        .and_then(|o| Uuid::parse_str(&o.id_obra).ok())
        .ok_or_else(|| "Elija una obra.".to_string())
}

/// Calcula el plan según la regla elegida. Pura salvo las consultas de la regla de copia.
fn calcular_plan(
    ui: &MainWindow,
    pool: &Option<Arc<DbPool>>,
    session: &Arc<Mutex<ReporteSessionState>>,
    estado: &Arc<Mutex<EstadoAsistente>>,
) -> Result<PlanAsistente, String> {
    let app = ui.global::<AppData>();
    let regla = app.get_asig_asistente_regla();
    let solo_sin_obra = app.get_asig_asistente_alcance() == 0;

    let st = session
        .lock()
        .map_err(|_| "estado de la sesión bloqueado".to_string())?;
    let m = construir_matriz(&st.all_rows, &st.obras_options);

    match regla {
        0 => Ok(planear_por_vigencia(&m, &st.obras_options, solo_sin_obra)),
        1 => {
            let obra_id = obra_seleccionada(&app)?;
            let desde = app.get_asig_asistente_mes_desde_idx();
            let hasta = app.get_asig_asistente_mes_hasta_idx();
            if desde < 0 || hasta < desde || hasta as usize >= m.meses.len() {
                return Err("Rango de meses inválido.".to_string());
            }
            let rango = &m.meses[desde as usize..=hasta as usize];
            Ok(planear_todos_a_una_obra(&m, &st.obras_options, obra_id, rango, solo_sin_obra))
        }
        2 => {
            let ruta = app.get_asig_asistente_archivo().to_string();
            if ruta.trim().is_empty() {
                return Err("Elija un archivo CSV o XLSX.".to_string());
            }
            let filas = leer_archivo_importacion(Path::new(&ruta))?;
            Ok(planear_importacion(&m, &st.obras_options, &filas, solo_sin_obra))
        }
        3 => {
            let idx = app.get_asig_asistente_revision_idx();
            let origen = estado
                .lock()
                .map_err(|_| "estado del asistente bloqueado".to_string())?
                .revisiones
                .get(idx as usize)
                .copied()
                .ok_or_else(|| "Seleccione una revisión de origen.".to_string())?;
            let pool_ref = pool
                .as_ref()
                .ok_or_else(|| "Sin conexión a la base de datos.".to_string())?;
            let mut client = pool_ref
                .get()
                .map_err(|e| format!("conexión a la base de datos: {e}"))?;
            let asignaciones = consultar_asignaciones(&mut client, origen)?;
            Ok(planear_copia(&m, &st.obras_options, &asignaciones, solo_sin_obra))
        }
        _ => Err("Regla no válida.".to_string()),
    }
}

/// Vuelca el plan a las propiedades de la vista previa y pasa al paso 2.
fn mostrar_previa(ui: &MainWindow, plan: PlanAsistente, estado: &Arc<Mutex<EstadoAsistente>>) {
    let app = ui.global::<AppData>();
    let total = plan.previa.len();
    let filas: Vec<AsigAsistentePrevia> = plan
        .previa
        .iter()
        .take(MAX_PREVIA_VISIBLE)
        .map(|f| AsigAsistentePrevia {
            nombre: f.nombre.clone().into(),
            nss: f.nss.clone().into(),
            etiqueta_mes: f.etiqueta_mes.clone().into(),
            obras: f.obras.clone().into(),
            resultado: f.resultado.clone().into(),
            omitido: f.omitido,
        })
        .collect();

    let mut resumen = format!(
        "{} empleado-mes se asignarán · {} omitidos",
        plan.aplicables, plan.omitidos
    );
    if total > MAX_PREVIA_VISIBLE {
        resumen.push_str(&format!(" · mostrando {MAX_PREVIA_VISIBLE} de {total}"));
    }

    app.set_asig_asistente_previa(ModelRc::new(VecModel::from(filas)));
    app.set_asig_asistente_resumen(SharedString::from(resumen));
    app.set_asig_asistente_puede_aplicar(plan.aplicables > 0);
    app.set_asig_asistente_paso(2);
    if let Ok(mut e) = estado.lock() {
        e.plan = Some(plan);
    }
}

/// Aplica cada grupo con una llamada por lote. Devuelve (reportes asignados, lotes aplicados).
fn aplicar_grupos(client: &mut Client, rev: Uuid, plan: &PlanAsistente) -> Result<(usize, usize), String> {
    let total_lotes = plan.grupos.len();
    let mut reportes_total = 0usize;
    for (k, (obras, reportes, dias)) in plan.grupos.iter().enumerate() {
        batch_assign_multiple_obras_to_employees(client, rev, reportes, obras, None, *dias, None).map_err(
            |e| format!("lote {} de {total_lotes}: {e} (lotes previos aplicados: {k})", k + 1),
        )?;
        reportes_total += reportes.len();
    }
    Ok((reportes_total, total_lotes))
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

// ---------------------------------------------------------------------------
// Callbacks
// ---------------------------------------------------------------------------

/// Registra los callbacks del asistente de asignación masiva.
pub fn wire_asistente_callbacks(
    main_window: &MainWindow,
    db_pool: Option<Arc<DbPool>>,
    session: Arc<Mutex<ReporteSessionState>>,
) {
    let app_logic = main_window.global::<AppLogic>();
    let estado = Arc::new(Mutex::new(EstadoAsistente::default()));

    // Abrir: paso 1 con valores por defecto y lista de revisiones del mismo registro patronal.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        let estado = estado.clone();
        app_logic.on_asig_asistente_abrir(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(rev) = revision_actual(&session) else {
                show_toast_message(&ui, "Seleccione una revisión antes de continuar.");
                return;
            };
            let consulta = match pool.as_ref() {
                None => Err("Sin conexión a la base de datos.".to_string()),
                Some(p) => match p.get() {
                    Err(e) => Err(format!("conexión a la base de datos: {e}")),
                    Ok(mut client) => consultar_revisiones_rp(&mut *client, rev),
                },
            };
            let revisiones = match consulta {
                Ok(r) => r,
                Err(e) => {
                    log::error!("Asistente de asignación: {e}");
                    show_toast_message(&ui, &format!("No se pudo abrir el asistente: {e}"));
                    return;
                }
            };

            let textos: Vec<SharedString> = revisiones
                .iter()
                .map(|(id, nombre)| {
                    let nombre = nombre.as_deref().unwrap_or("").trim();
                    if nombre.is_empty() {
                        SharedString::from(id.to_string())
                    } else {
                        SharedString::from(nombre)
                    }
                })
                .collect();
            if let Ok(mut e) = estado.lock() {
                e.plan = None;
                e.revisiones = revisiones.iter().map(|(id, _)| *id).collect();
            }

            let app = ui.global::<AppData>();
            let meses = app.get_asig_meses().row_count();
            app.set_asig_asistente_paso(1);
            app.set_asig_asistente_regla(0);
            app.set_asig_asistente_alcance(0);
            app.set_asig_asistente_obra_idx(0);
            app.set_asig_asistente_mes_desde_idx(0);
            app.set_asig_asistente_mes_hasta_idx(meses.saturating_sub(1) as i32);
            app.set_asig_asistente_archivo(SharedString::default());
            app.set_asig_asistente_revisiones(ModelRc::new(VecModel::from(textos)));
            app.set_asig_asistente_revision_idx(0);
            app.set_asig_asistente_previa(ModelRc::new(VecModel::from(Vec::<AsigAsistentePrevia>::new())));
            app.set_asig_asistente_resumen(SharedString::default());
            app.set_asig_asistente_puede_aplicar(false);
            app.set_show_asig_asistente(true);
        });
    }

    // Elegir archivo: diálogo nativo (CSV o XLSX); guarda la ruta en el modal.
    {
        let ui_weak = main_window.as_weak();
        app_logic.on_asig_asistente_elegir_archivo(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Seleccionar archivo de asignación")
                .add_filter("CSV o Excel", &["csv", "xlsx"])
                .pick_file()
            {
                ui.global::<AppData>()
                    .set_asig_asistente_archivo(SharedString::from(path.display().to_string()));
            }
        });
    }

    // Vista previa: calcula el plan según la regla y pasa al paso 2.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        let estado = estado.clone();
        app_logic.on_asig_asistente_vista_previa(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            match calcular_plan(&ui, &pool, &session, &estado) {
                Ok(plan) => mostrar_previa(&ui, plan, &estado),
                Err(e) => show_toast_message(&ui, &e),
            }
        });
    }

    // Atrás: regresa al paso 1 y descarta el plan calculado.
    {
        let ui_weak = main_window.as_weak();
        let estado = estado.clone();
        app_logic.on_asig_asistente_atras(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Ok(mut e) = estado.lock() {
                e.plan = None;
            }
            ui.global::<AppData>().set_asig_asistente_paso(1);
        });
    }

    // Aplicar: un lote de BD por conjunto de obras; luego recarga y deshacer.
    {
        let ui_weak = main_window.as_weak();
        let pool = db_pool.clone();
        let session = session.clone();
        let estado = estado.clone();
        app_logic.on_asig_asistente_aplicar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let plan = estado.lock().ok().and_then(|e| e.plan.clone());
            let Some(plan) = plan.filter(|p| !p.grupos.is_empty()) else {
                show_toast_message(&ui, "No hay empleado-mes por asignar. Revise la vista previa.");
                return;
            };
            let Some(rev_str) = session
                .lock()
                .ok()
                .and_then(|st| st.current_revision_id.clone())
            else {
                show_toast_message(&ui, "Seleccione una revisión antes de continuar.");
                return;
            };
            let Ok(rev) = Uuid::parse_str(&rev_str) else {
                show_toast_message(&ui, "La revisión seleccionada no es válida.");
                return;
            };
            let Some(pool_ref) = pool.as_ref() else {
                show_toast_message(&ui, "Sin conexión a la base de datos.");
                return;
            };

            let app = ui.global::<AppData>();
            app.set_asig_procesando(true);
            let (resultado, deshacer) = match pool_ref.get() {
                Err(e) => (Err(format!("conexión a la base de datos: {e}")), None),
                Ok(mut client) => {
                    let resultado = aplicar_grupos(&mut client, rev, &plan);
                    let deshacer = match ultimo_lote_revision(&mut *client, rev) {
                        Ok(d) => d,
                        Err(e) => {
                            log::error!("Asistente de asignación: consulta de bitácora: {e}");
                            None
                        }
                    };
                    (resultado, deshacer)
                }
            };
            app.set_asig_procesando(false);

            // La recarga toma el lock de sesión; no debe estar retenido aquí.
            sync_and_load_reporte_data(&ui, &pool, &session, &rev_str, false);
            actualizar_deshacer(&ui, deshacer);

            match resultado {
                Ok((n, k)) => {
                    app.set_show_asig_asistente(false);
                    if let Ok(mut e) = estado.lock() {
                        e.plan = None;
                    }
                    show_toast_message(
                        &ui,
                        &format!(
                            "Asistente: {n} empleado-mes asignados en {k} lote(s). Usa Deshacer para revertir lote por lote."
                        ),
                    );
                }
                Err(e) => {
                    log::error!("Asistente de asignación (aplicar): {e}");
                    show_toast_message(&ui, &format!("No se pudo aplicar el asistente: {e}"));
                }
            }
        });
    }

    // Cerrar: oculta el modal y descarta el plan.
    {
        let ui_weak = main_window.as_weak();
        let estado = estado.clone();
        app_logic.on_asig_asistente_cerrar(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Ok(mut e) = estado.lock() {
                e.plan = None;
            }
            ui.global::<AppData>().set_show_asig_asistente(false);
        });
    }
}

#[cfg(test)]
mod tests {

    /// Valida contra la BD configurada las consultas (solo SELECT) del asistente:
    /// `cargo test consultas_asistente_bd -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn consultas_asistente_bd() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("Sin conexión a BD: {e}");
                return;
            }
        };
        let revs: Vec<uuid::Uuid> = client
            .query("SELECT DISTINCT id_revision FROM public.reportes_liquidaciones", &[])
            .expect("revisiones")
            .iter()
            .map(|r| r.get(0))
            .collect();
        let mut total_asig = 0usize;
        for rev in &revs {
            let otras = super::consultar_revisiones_rp(&mut client, *rev).expect("consultar_revisiones_rp");
            let asig = super::consultar_asignaciones(&mut client, *rev).expect("consultar_asignaciones");
            total_asig += asig.len();
            println!("ASIST rev={rev} otras_revisiones_rp={} empleado_mes_con_obra={}", otras.len(), asig.len());
        }
        assert!(!revs.is_empty() || total_asig == 0);
    }

    use super::*;
    use crate::asignacion_view_logic::ChipObra;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn obra(n: u128, numero: &str) -> ObraOptionDb {
        ObraOptionDb {
            id: id(n),
            numero: numero.to_string(),
            tipo_obra: String::new(),
            fecha_inicio: None,
            fecha_fin: None,
            label: numero.to_string(),
            creado_usuario: false,
            monto: 0.0,
        }
    }

    /// Celda de prueba. `obras` son ids de obra ya asignados (vacío = sin obra).
    fn celda(id_reporte: u128, anio: i16, mes: i16, obras: &[u128], es_baja: bool) -> CeldaMes {
        CeldaMes {
            id_reporte: id(id_reporte),
            anio,
            mes,
            dias_cedula: 30,
            disponibles: 30,
            asignados: 0,
            obras: obras
                .iter()
                .map(|&o| ChipObra {
                    id_obra: id(o),
                    numero: String::new(),
                    dias: 10,
                    ids_tramos: Vec::new(),
                    color_idx: 0,
                })
                .collect(),
            ids_tramos_sin_obra: Vec::new(),
            es_baja,
            inconsistente: false,
            cedula_actualizada: false,
        }
    }

    fn fila(n: u128, nombre: &str, nss: &str, celdas: Vec<Option<CeldaMes>>) -> FilaEmpleado {
        FilaEmpleado {
            id_empleado: id(n),
            nombre: nombre.to_string(),
            nss: nss.to_string(),
            rfc: String::new(),
            celdas,
            num_obras: 0,
            tiene_inconsistencia: false,
            cedula_actualizada: false,
        }
    }

    const NSS1: &str = "12345678901";
    const NSS2: &str = "10987654321";

    /// Matriz: E1 (NSS1) con Ene sin obra (r1) y Feb con obra A (r2); E2 (NSS2) con Ene baja (r3).
    fn matriz_base() -> MatrizAsignacion {
        MatrizAsignacion {
            meses: vec![(2026, 1), (2026, 2)],
            filas: vec![
                fila(1, "Ana", NSS1, vec![Some(celda(1, 2026, 1, &[], false)), Some(celda(2, 2026, 2, &[100], false))]),
                fila(2, "Beto", NSS2, vec![Some(celda(3, 2026, 1, &[], true)), None]),
            ],
        }
    }

    fn obras_base() -> Vec<ObraOptionDb> {
        vec![obra(100, "A"), obra(101, "B")]
    }

    #[test]
    fn por_vigencia_solo_sin_obra_omite_con_motivo() {
        let plan = planear_por_vigencia(&matriz_base(), &obras_base(), true);
        assert_eq!(plan.aplicables, 1);
        assert_eq!(plan.omitidos, 2);
        assert_eq!(plan.grupos.len(), 1);
        let (obras, reportes, dias) = &plan.grupos[0];
        assert_eq!(obras, &vec![id(100), id(101)]);
        assert_eq!(reportes, &vec![id(1)]);
        assert_eq!(*dias, None);
        assert!(plan.previa.iter().any(|f| f.omitido && f.resultado.contains("ya tiene obra")));
        assert!(plan.previa.iter().any(|f| f.omitido && f.resultado.contains("baja")));
    }

    #[test]
    fn por_vigencia_todos_reasigna_y_agrupa() {
        let plan = planear_por_vigencia(&matriz_base(), &obras_base(), false);
        assert_eq!(plan.aplicables, 2);
        assert_eq!(plan.omitidos, 1);
        assert_eq!(plan.grupos.len(), 1);
        assert_eq!(plan.grupos[0].1.len(), 2);
    }

    #[test]
    fn todos_a_una_obra_respeta_rango_de_meses() {
        let plan = planear_todos_a_una_obra(&matriz_base(), &obras_base(), id(101), &[(2026, 2)], false);
        assert_eq!(plan.previa.len(), 1);
        assert_eq!(plan.aplicables, 1);
        assert_eq!(plan.grupos[0].0, vec![id(101)]);
        assert_eq!(plan.grupos[0].1, vec![id(2)]);
    }

    #[test]
    fn todos_a_una_obra_inexistente_omite() {
        let plan = planear_todos_a_una_obra(&matriz_base(), &obras_base(), id(999), &[(2026, 1), (2026, 2)], true);
        assert_eq!(plan.aplicables, 0);
        assert!(plan.grupos.is_empty());
        assert!(plan.previa.iter().all(|f| f.omitido));
    }

    #[test]
    fn importacion_agrupa_obras_y_omite_inexistentes() {
        let filas = vec![
            FilaImportada { nss: NSS1.into(), anio: 2026, mes: 1, obra: "A".into(), dias: Some(10) },
            FilaImportada { nss: NSS1.into(), anio: 2026, mes: 1, obra: "b".into(), dias: None },
            FilaImportada { nss: NSS2.into(), anio: 2026, mes: 1, obra: "ZZ".into(), dias: None },
            FilaImportada { nss: "99999999999".into(), anio: 2026, mes: 1, obra: "A".into(), dias: None },
        ];
        let mut m = matriz_base();
        // Ana Ene sin obra es el único reporte válido; Beto Ene es baja.
        m.filas[1].celdas[0] = Some(celda(3, 2026, 1, &[], false));
        let plan = planear_importacion(&m, &obras_base(), &filas, true);
        assert_eq!(plan.aplicables, 1);
        assert_eq!(plan.grupos.len(), 1);
        assert_eq!(plan.grupos[0].0, vec![id(100), id(101)]);
        assert_eq!(plan.grupos[0].2, None);
        assert_eq!(plan.omitidos, 2);
        assert!(plan.previa.iter().any(|f| f.resultado.contains("no encontrado")));
    }

    #[test]
    fn importacion_usa_dias_con_una_sola_obra() {
        let filas = vec![FilaImportada { nss: NSS1.into(), anio: 2026, mes: 1, obra: "A".into(), dias: Some(12) }];
        let plan = planear_importacion(&matriz_base(), &obras_base(), &filas, true);
        assert_eq!(plan.aplicables, 1);
        assert_eq!(plan.grupos[0].2, Some(12));
    }

    #[test]
    fn copia_omite_obras_inexistentes_y_meses_sin_origen() {
        let origen = vec![AsignacionOrigen {
            id_empleado: id(1),
            anio: 2026,
            mes: 1,
            numeros_obra: vec!["B".into(), "NOEXISTE".into()],
        }];
        let m = MatrizAsignacion {
            meses: vec![(2026, 1), (2026, 2)],
            filas: vec![fila(1, "Ana", NSS1, vec![Some(celda(1, 2026, 1, &[], false)), Some(celda(2, 2026, 2, &[], false))])],
        };
        let plan = planear_copia(&m, &obras_base(), &origen, true);
        assert_eq!(plan.aplicables, 1);
        assert_eq!(plan.omitidos, 1);
        assert_eq!(plan.grupos[0].0, vec![id(101)]);
        assert!(plan.previa[0].resultado.contains("NOEXISTE"));
        assert!(plan.previa[1].resultado.contains("sin asignación"));
    }

    #[test]
    fn copia_todas_las_obras_inexistentes_omite() {
        let origen = vec![AsignacionOrigen { id_empleado: id(1), anio: 2026, mes: 1, numeros_obra: vec!["X".into()] }];
        let m = MatrizAsignacion {
            meses: vec![(2026, 1)],
            filas: vec![fila(1, "Ana", NSS1, vec![Some(celda(1, 2026, 1, &[], false))])],
        };
        let plan = planear_copia(&m, &obras_base(), &origen, true);
        assert_eq!(plan.aplicables, 0);
        assert!(plan.grupos.is_empty());
        assert_eq!(plan.omitidos, 1);
    }

    fn ruta_temporal(nombre: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("asistente_{}_{}_{nombre}", std::process::id(), nanos))
    }

    #[test]
    fn lector_csv_acepta_punto_y_coma_encabezados_en_otro_orden() {
        let ruta = ruta_temporal("ok.csv");
        let contenido = "\u{feff}OBRA;DÍAS;MES;AÑO;NSS\nA;15;Marzo;2026;12345678901\n\nB;;3;2026;10987654321\n";
        std::fs::write(&ruta, contenido).expect("escribir CSV temporal");
        let resultado = leer_archivo_importacion(&ruta);
        let _ = std::fs::remove_file(&ruta);
        let filas = resultado.expect("CSV válido");
        assert_eq!(filas.len(), 2);
        assert_eq!(filas[0], FilaImportada { nss: NSS1.into(), anio: 2026, mes: 3, obra: "A".into(), dias: Some(15) });
        assert_eq!(filas[1].dias, None);
        assert_eq!(filas[1].mes, 3);
    }

    #[test]
    fn lector_csv_reporta_renglon_invalido() {
        let ruta = ruta_temporal("malo.csv");
        let contenido = "NSS,AÑO,MES,OBRA\n12345678901,2026,1,A\n123,2026,1,A\n";
        std::fs::write(&ruta, contenido).expect("escribir CSV temporal");
        let resultado = leer_archivo_importacion(&ruta);
        let _ = std::fs::remove_file(&ruta);
        let err = resultado.expect_err("NSS corto debe fallar");
        assert!(err.contains("Renglón 3"), "mensaje: {err}");
    }

    #[test]
    fn lector_csv_exige_columnas_obligatorias() {
        let ruta = ruta_temporal("faltan.csv");
        std::fs::write(&ruta, "NSS,MES\n12345678901,1\n").expect("escribir CSV temporal");
        let resultado = leer_archivo_importacion(&ruta);
        let _ = std::fs::remove_file(&ruta);
        let err = resultado.expect_err("faltan columnas");
        assert!(err.contains("AÑO") && err.contains("OBRA"), "mensaje: {err}");
    }

    #[test]
    fn dividir_linea_respeta_comillas() {
        assert_eq!(dividir_linea("\"A, B\",2", ','), vec!["A, B".to_string(), "2".to_string()]);
        assert_eq!(dividir_linea("\"di\"\"jo\";x", ';'), vec!["di\"jo".to_string(), "x".to_string()]);
    }
}
