// Se conecta a la UI en la tarea de modal de alta masiva
#![allow(dead_code)]

//! Alta masiva de revisiones de un registro patronal: interpretación de la tabla (o generador por
//! prefijo y consecutivo), clasificación frente a las revisiones existentes y vista previa.
//! Funciones puras (sin BD).

use std::collections::HashSet;

use chrono::NaiveDate;

use crate::carga_masiva::{indice_columna, parse_fecha_flexible, TablaCruda};

/// Fila interpretada del lote, antes de clasificar.
#[derive(Debug, Clone, PartialEq)]
pub struct RevisionLoteFila {
    pub renglon: usize,
    pub nombre: String,
    pub folio: Option<String>,
    pub concepto: String,
    pub estatus: String,
    pub periodo_inicio: Option<NaiveDate>,
    pub periodo_fin: Option<NaiveDate>,
    pub fecha_aprobacion_imss: Option<NaiveDate>,
    pub fecha_entrega_revision: Option<NaiveDate>,
    pub errores: Vec<String>,
}

/// Estado de una fila frente a la BD y al propio lote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoRevisionLote {
    Valida,
    Error,
    NombreExiste,
    FolioExiste,
    RepetidaEnLote,
}

/// Fila lista para la vista previa.
#[derive(Debug, Clone, PartialEq)]
pub struct RevisionPrevia {
    pub fila: RevisionLoteFila,
    pub estado: EstadoRevisionLote,
    pub incluir: bool,
}

/// Revisión ya registrada en el registro patronal (para clasificar).
#[derive(Debug, Clone, PartialEq)]
pub struct RevisionExistente {
    pub nombre: String,
    pub folio: Option<String>,
}

/// Alias de columnas (ya normalizados con `normalizar_encabezado`).
const ALIAS_NOMBRE: &[&str] = &["NOMBRE", "REVISION", "NOMBRE DE LA REVISION"];
const ALIAS_FOLIO: &[&str] = &["FOLIO"];
const ALIAS_CONCEPTO: &[&str] = &["CONCEPTO", "CONCEPTO DE DIFERENCIA"];
const ALIAS_ESTATUS: &[&str] = &["ESTATUS"];
const ALIAS_INICIO: &[&str] = &["PERIODO INICIO", "INICIO PERIODO", "INICIO"];
const ALIAS_FIN: &[&str] = &["PERIODO FIN", "FIN PERIODO", "FIN"];
const ALIAS_APROBACION: &[&str] = &["F APROBACION IMSS", "FECHA APROBACION IMSS", "APROBACION IMSS"];
const ALIAS_ENTREGA: &[&str] = &["F ENTREGA REVISION", "FECHA ENTREGA REVISION", "ENTREGA REVISION"];

/// Posiciones de columnas. `None` = columna ausente.
struct Columnas {
    nombre: usize,
    folio: Option<usize>,
    concepto: Option<usize>,
    estatus: Option<usize>,
    inicio: Option<usize>,
    fin: Option<usize>,
    aprobacion: Option<usize>,
    entrega: Option<usize>,
}

impl Columnas {
    /// Orden posicional para pegado sin encabezado: NOMBRE, FOLIO, CONCEPTO, ESTATUS, PERIODO INICIO, PERIODO FIN.
    fn posicional() -> Self {
        Columnas {
            nombre: 0,
            folio: Some(1),
            concepto: Some(2),
            estatus: Some(3),
            inicio: Some(4),
            fin: Some(5),
            aprobacion: None,
            entrega: None,
        }
    }

    fn desde_encabezado(enc: &[String]) -> Result<Self, String> {
        let nombre = indice_columna(enc, ALIAS_NOMBRE)
            .ok_or_else(|| "Falta la columna NOMBRE de la revisión".to_string())?;
        Ok(Columnas {
            nombre,
            folio: indice_columna(enc, ALIAS_FOLIO),
            concepto: indice_columna(enc, ALIAS_CONCEPTO),
            estatus: indice_columna(enc, ALIAS_ESTATUS),
            inicio: indice_columna(enc, ALIAS_INICIO),
            fin: indice_columna(enc, ALIAS_FIN),
            aprobacion: indice_columna(enc, ALIAS_APROBACION),
            entrega: indice_columna(enc, ALIAS_ENTREGA),
        })
    }
}

fn celda(celdas: &[String], idx: Option<usize>) -> &str {
    idx.and_then(|i| celdas.get(i)).map(|s| s.trim()).unwrap_or("")
}

/// Interpreta la tabla del lote. Si el primer renglón no reconoce ninguna columna, asume pegado sin
/// encabezado y usa el orden posicional. Los renglones totalmente vacíos se omiten.
pub fn interpretar_tabla_revisiones(tabla: &TablaCruda) -> Result<Vec<RevisionLoteFila>, String> {
    let Some((_, primero)) = tabla.first() else {
        return Err("La tabla no tiene renglones.".to_string());
    };

    let todas_alias: [&[&str]; 8] = [
        ALIAS_NOMBRE,
        ALIAS_FOLIO,
        ALIAS_CONCEPTO,
        ALIAS_ESTATUS,
        ALIAS_INICIO,
        ALIAS_FIN,
        ALIAS_APROBACION,
        ALIAS_ENTREGA,
    ];
    let hay_encabezado = todas_alias.iter().any(|alias| indice_columna(primero, alias).is_some());

    let (cols, datos): (Columnas, &[(usize, Vec<String>)]) = if hay_encabezado {
        (Columnas::desde_encabezado(primero)?, &tabla[1..])
    } else {
        (Columnas::posicional(), &tabla[..])
    };

    let mut filas = Vec::new();
    for (renglon, celdas) in datos {
        if celdas.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        filas.push(interpretar_renglon(*renglon, celdas, &cols));
    }
    Ok(filas)
}

fn interpretar_renglon(renglon: usize, celdas: &[String], cols: &Columnas) -> RevisionLoteFila {
    let mut errores: Vec<String> = Vec::new();

    let nombre = celda(celdas, Some(cols.nombre)).to_uppercase();
    if nombre.is_empty() {
        errores.push("El nombre de la revisión es obligatorio.".to_string());
    }

    let folio_txt = celda(celdas, cols.folio).to_uppercase();
    let folio = if folio_txt.is_empty() || folio_txt == "S/F" {
        None
    } else {
        Some(folio_txt)
    };

    // Parsea una fecha opcional; texto no vacío que no parsea → error y `invalida = true`.
    let parsear = |txt: &str, campo: &str, errores: &mut Vec<String>| -> (Option<NaiveDate>, bool) {
        if txt.is_empty() {
            return (None, false);
        }
        match parse_fecha_flexible(txt) {
            Some(f) => (Some(f), false),
            None => {
                errores.push(format!("{campo} inválida: '{txt}'"));
                (None, true)
            }
        }
    };

    let (periodo_inicio, inicio_invalida) = parsear(
        celda(celdas, cols.inicio),
        "Fecha de inicio del periodo",
        &mut errores,
    );
    let (periodo_fin, fin_invalida) = parsear(celda(celdas, cols.fin), "Fecha de fin del periodo", &mut errores);
    let (fecha_aprobacion_imss, _) = parsear(
        celda(celdas, cols.aprobacion),
        "Fecha de aprobación IMSS",
        &mut errores,
    );
    let (fecha_entrega_revision, _) = parsear(
        celda(celdas, cols.entrega),
        "Fecha de entrega de la revisión",
        &mut errores,
    );

    if !inicio_invalida && !fin_invalida {
        match (periodo_inicio, periodo_fin) {
            (Some(i), Some(f)) if i > f => {
                errores.push("El periodo revisado es inválido: el inicio es posterior al fin.".to_string())
            }
            (Some(_), None) | (None, Some(_)) => {
                errores.push("Captura inicio y fin del periodo revisado (o deja ambos vacíos).".to_string())
            }
            _ => {}
        }
    }

    RevisionLoteFila {
        renglon,
        nombre,
        folio,
        concepto: celda(celdas, cols.concepto).to_string(),
        estatus: celda(celdas, cols.estatus).to_string(),
        periodo_inicio,
        periodo_fin,
        fecha_aprobacion_imss,
        fecha_entrega_revision,
        errores,
    }
}

/// Genera `cantidad` revisiones con nombre `prefijo_nombre + consecutivo` (desde `desde`, con `ancho`
/// dígitos) y folio `prefijo_folio + consecutivo` (None si el prefijo de folio está vacío).
pub fn generar_por_prefijo(
    prefijo_nombre: &str,
    prefijo_folio: &str,
    desde: u32,
    cantidad: u32,
    ancho: usize,
) -> Result<Vec<RevisionLoteFila>, String> {
    let pn = prefijo_nombre.trim().to_uppercase();
    if pn.is_empty() {
        return Err("Captura el prefijo del nombre.".to_string());
    }
    if !(1..=200).contains(&cantidad) {
        return Err("La cantidad a generar debe estar entre 1 y 200.".to_string());
    }
    let pf = prefijo_folio.trim().to_uppercase();

    let filas = (0..cantidad)
        .map(|i| {
            let n = desde as u64 + i as u64;
            let folio = if pf.is_empty() {
                None
            } else {
                Some(format!("{}{:0ancho$}", pf, n, ancho = ancho))
            };
            RevisionLoteFila {
                renglon: i as usize + 1,
                nombre: format!("{}{:0ancho$}", pn, n, ancho = ancho),
                folio,
                concepto: String::new(),
                estatus: String::new(),
                periodo_inicio: None,
                periodo_fin: None,
                fecha_aprobacion_imss: None,
                fecha_entrega_revision: None,
                errores: Vec::new(),
            }
        })
        .collect();
    Ok(filas)
}

/// Clave de comparación de nombres y folios: sin espacios sobrantes y en mayúsculas.
fn clave(s: &str) -> String {
    s.trim().to_uppercase()
}

/// Clasifica las filas del lote frente a las revisiones existentes del registro patronal.
pub fn clasificar_revisiones(filas: Vec<RevisionLoteFila>, existentes: &[RevisionExistente]) -> Vec<RevisionPrevia> {
    let nombres_existentes: HashSet<String> = existentes.iter().map(|r| clave(&r.nombre)).collect();
    let folios_existentes: HashSet<String> = existentes
        .iter()
        .filter_map(|r| r.folio.as_deref())
        .map(clave)
        .filter(|f| !f.is_empty() && f != "S/F")
        .collect();

    let mut vistos_nombres: HashSet<String> = HashSet::new();
    let mut vistos_folios: HashSet<String> = HashSet::new();

    filas
        .into_iter()
        .map(|fila| {
            if !fila.errores.is_empty() {
                return RevisionPrevia {
                    fila,
                    estado: EstadoRevisionLote::Error,
                    incluir: false,
                };
            }

            let nombre = clave(&fila.nombre);
            let folio = fila.folio.as_deref().map(clave);
            let repetida = vistos_nombres.contains(&nombre)
                || folio.as_ref().is_some_and(|f| vistos_folios.contains(f));
            vistos_nombres.insert(nombre.clone());
            if let Some(f) = &folio {
                vistos_folios.insert(f.clone());
            }

            let estado = if repetida {
                EstadoRevisionLote::RepetidaEnLote
            } else if nombres_existentes.contains(&nombre) {
                EstadoRevisionLote::NombreExiste
            } else if folio.as_ref().is_some_and(|f| folios_existentes.contains(f)) {
                EstadoRevisionLote::FolioExiste
            } else {
                EstadoRevisionLote::Valida
            };

            RevisionPrevia {
                fila,
                estado,
                incluir: estado == EstadoRevisionLote::Valida,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carga_masiva::tabla_desde_texto_pegado;

    fn d(a: i32, m: u32, dd: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(a, m, dd)
    }

    #[test]
    fn tabla_con_encabezado_y_fechas_en_tres_formatos() {
        let texto = "nombre\tfolio\tconcepto\testatus\tperiodo inicio\tperiodo fin\tf aprobacion imss\tf entrega revision\n\
                     rev a\tf-1\tDiferencia\tPagado\t01/01/2025\t2025-01-31\t31-03-2025\t\n\
                     \t\t\t\t\t\t\t\n";
        let filas = interpretar_tabla_revisiones(&tabla_desde_texto_pegado(texto)).unwrap();
        assert_eq!(filas.len(), 1);
        let f = &filas[0];
        assert_eq!(f.renglon, 2);
        assert_eq!(f.nombre, "REV A");
        assert_eq!(f.folio.as_deref(), Some("F-1"));
        assert_eq!(f.concepto, "Diferencia");
        assert_eq!(f.estatus, "Pagado");
        assert_eq!(f.periodo_inicio, d(2025, 1, 1));
        assert_eq!(f.periodo_fin, d(2025, 1, 31));
        assert_eq!(f.fecha_aprobacion_imss, d(2025, 3, 31));
        assert_eq!(f.fecha_entrega_revision, None);
        assert!(f.errores.is_empty());
    }

    #[test]
    fn sin_encabezado_usa_orden_posicional() {
        let texto = "REV01\tF01\tDiferencia\tPagado\t01/01/2025\t31/01/2025\nREV02\tS/F\t\t\t\t";
        let filas = interpretar_tabla_revisiones(&tabla_desde_texto_pegado(texto)).unwrap();
        assert_eq!(filas.len(), 2);
        assert_eq!(filas[0].nombre, "REV01");
        assert_eq!(filas[0].folio.as_deref(), Some("F01"));
        assert_eq!(filas[0].estatus, "Pagado");
        assert_eq!(filas[0].periodo_fin, d(2025, 1, 31));
        assert_eq!(filas[1].folio, None);
        assert!(filas[1].errores.is_empty());
    }

    #[test]
    fn encabezado_sin_nombre_es_error() {
        let t = tabla_desde_texto_pegado("FOLIO\tESTATUS\nF01\tPagado");
        assert_eq!(
            interpretar_tabla_revisiones(&t).unwrap_err(),
            "Falta la columna NOMBRE de la revisión"
        );
    }

    #[test]
    fn periodo_incompleto_e_invertido_y_fecha_invalida() {
        let texto = "NOMBRE\tPERIODO INICIO\tPERIODO FIN\tF APROBACION IMSS\n\
                     R1\t01/01/2025\t\t\n\
                     R2\t31/01/2025\t01/01/2025\t\n\
                     R3\tmañana\t\t\n\
                     \t\t\t\n";
        let filas = interpretar_tabla_revisiones(&tabla_desde_texto_pegado(texto)).unwrap();
        assert_eq!(filas.len(), 3);
        assert_eq!(
            filas[0].errores,
            vec!["Captura inicio y fin del periodo revisado (o deja ambos vacíos).".to_string()]
        );
        assert_eq!(
            filas[1].errores,
            vec!["El periodo revisado es inválido: el inicio es posterior al fin.".to_string()]
        );
        assert_eq!(filas[2].errores, vec!["Fecha de inicio del periodo inválida: 'mañana'".to_string()]);
    }

    #[test]
    fn nombre_vacio_es_error() {
        let texto = "NOMBRE\tFOLIO\n\tF01";
        let filas = interpretar_tabla_revisiones(&tabla_desde_texto_pegado(texto)).unwrap();
        assert_eq!(filas[0].errores, vec!["El nombre de la revisión es obligatorio.".to_string()]);
    }

    #[test]
    fn generador_ancho_dos_con_folios() {
        let filas = generar_por_prefijo(" rev ", "f", 1, 3, 2).unwrap();
        let nombres: Vec<&str> = filas.iter().map(|f| f.nombre.as_str()).collect();
        assert_eq!(nombres, vec!["REV01", "REV02", "REV03"]);
        let folios: Vec<Option<&str>> = filas.iter().map(|f| f.folio.as_deref()).collect();
        assert_eq!(folios, vec![Some("F01"), Some("F02"), Some("F03")]);

        let sin_folio = generar_por_prefijo("REV", "  ", 9, 2, 2).unwrap();
        assert_eq!(sin_folio[1].nombre, "REV10");
        assert_eq!(sin_folio[0].folio, None);
    }

    #[test]
    fn generador_valida_entradas() {
        assert_eq!(
            generar_por_prefijo("REV", "F", 1, 0, 2).unwrap_err(),
            "La cantidad a generar debe estar entre 1 y 200."
        );
        assert_eq!(
            generar_por_prefijo("REV", "F", 1, 201, 2).unwrap_err(),
            "La cantidad a generar debe estar entre 1 y 200."
        );
        assert!(generar_por_prefijo("REV", "F", 1, 200, 2).is_ok());
        assert_eq!(generar_por_prefijo("  ", "F", 1, 1, 2).unwrap_err(), "Captura el prefijo del nombre.");
    }

    #[test]
    fn clasificacion_existente_repetida_y_valida() {
        let existentes = vec![RevisionExistente {
            nombre: "rev01".into(),
            folio: Some("F09".into()),
        }];
        let mut filas = generar_por_prefijo("REV", "F", 1, 3, 2).unwrap();
        // REV01 ya existe por nombre; REV02 tendrá folio existente; REV03 repetida en lote.
        filas[1].folio = Some("F09".into());
        filas.push(generar_por_prefijo("REV", "F", 3, 1, 2).unwrap().remove(0));
        let p = clasificar_revisiones(filas, &existentes);

        assert_eq!(p[0].estado, EstadoRevisionLote::NombreExiste);
        assert!(!p[0].incluir);
        assert_eq!(p[1].estado, EstadoRevisionLote::FolioExiste);
        assert!(!p[1].incluir);
        assert_eq!(p[2].estado, EstadoRevisionLote::Valida);
        assert!(p[2].incluir);
        assert_eq!(p[3].estado, EstadoRevisionLote::RepetidaEnLote);
        assert!(!p[3].incluir);
    }

    #[test]
    fn clasificacion_errores_y_folio_repetido_en_lote() {
        let mut filas = generar_por_prefijo("A", "X", 1, 2, 1).unwrap();
        filas[1].folio = Some("X1".into());
        filas[0].errores.push("Fecha inválida".into());
        let p = clasificar_revisiones(filas, &[]);
        assert_eq!(p[0].estado, EstadoRevisionLote::Error);
        assert!(!p[0].incluir);
        // La fila con error no cuenta como "vista": la segunda es válida.
        assert_eq!(p[1].estado, EstadoRevisionLote::Valida);

        let mut dup = generar_por_prefijo("A", "X", 1, 2, 1).unwrap();
        dup[1].folio = Some("X1".into());
        let p2 = clasificar_revisiones(dup, &[]);
        assert_eq!(p2[0].estado, EstadoRevisionLote::Valida);
        assert_eq!(p2[1].estado, EstadoRevisionLote::RepetidaEnLote);
    }
}
