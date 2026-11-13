
//! Alta masiva de registros de obra: interpretación de la tabla, clasificación frente a las obras
//! existentes de la revisión y resumen de la vista previa. Funciones puras (sin BD).

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use crate::carga_masiva::{indice_columna, parse_fecha_flexible, parse_monto_flexible, TablaCruda};
use crate::validation::validar_obra;

/// Fila interpretada del lote, antes de clasificar.
#[derive(Debug, Clone, PartialEq)]
pub struct ObraLoteFila {
    pub renglon: usize,
    pub numero: String,
    pub fecha_inicio: Option<NaiveDate>,
    pub fecha_fin: Option<NaiveDate>,
    pub monto: Option<f64>,
    pub clase: String,
    pub tipo_obra: String,
    pub ubicacion: String,
    pub superficie: f64,
    pub delegacion: String,
    pub subdelegacion: String,
    pub errores: Vec<String>,
}

/// Estado de una fila frente a la BD y al propio lote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoFila {
    Valida,
    SinMonto,
    Error,
    ExisteUsuario,
    ExisteCedula,
    RepetidaEnLote,
}

/// Acción elegida para una fila.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccionFila {
    Crear,
    Omitir,
    Actualizar,
    Adoptar,
}

/// Obra ya registrada en la revisión (para clasificar).
#[derive(Debug, Clone, PartialEq)]
pub struct ObraExistente {
    pub numero: String,
    pub creado_usuario: bool,
    pub activa: bool,
}

/// Fila lista para la vista previa.
#[derive(Debug, Clone, PartialEq)]
pub struct FilaPrevia {
    pub fila: ObraLoteFila,
    pub estado: EstadoFila,
    pub accion: AccionFila,
    pub acciones_posibles: Vec<AccionFila>,
}

/// Alias de columnas (ya normalizados con `normalizar_encabezado`).
const ALIAS_NUMERO: &[&str] = &["NUMERO", "NUMERO DE REGISTRO", "REGISTRO DE OBRA", "REGISTRO", "NO REGISTRO", "OBRA"];
const ALIAS_INICIO: &[&str] = &["INICIO", "FECHA INICIO", "FECHA DE INICIO"];
const ALIAS_FIN: &[&str] = &["FIN", "FECHA FIN", "FECHA FINAL", "FECHA DE TERMINO", "TERMINO"];
const ALIAS_MONTO: &[&str] = &["MONTO", "MONTO TOTAL", "IMPORTE", "VALOR"];
const ALIAS_CLASE: &[&str] = &["CLASE"];
const ALIAS_TIPO: &[&str] = &["TIPO", "TIPO DE OBRA", "TIPO OBRA"];
const ALIAS_UBICACION: &[&str] = &["UBICACION", "DOMICILIO"];
const ALIAS_SUPERFICIE: &[&str] = &["SUPERFICIE", "SUPERFICIE CONSTRUIDA", "M2"];
const ALIAS_DELEGACION: &[&str] = &["DELEGACION"];
const ALIAS_SUBDELEGACION: &[&str] = &["SUBDELEGACION"];

/// Posiciones de columnas. `None` = columna ausente.
struct Columnas {
    numero: usize,
    inicio: Option<usize>,
    fin: Option<usize>,
    monto: Option<usize>,
    clase: Option<usize>,
    tipo: Option<usize>,
    ubicacion: Option<usize>,
    superficie: Option<usize>,
    delegacion: Option<usize>,
    subdelegacion: Option<usize>,
}

impl Columnas {
    /// Orden posicional para pegado sin encabezado: NÚMERO, INICIO, FIN, MONTO, CLASE, TIPO, UBICACIÓN.
    fn posicional() -> Self {
        Columnas {
            numero: 0,
            inicio: Some(1),
            fin: Some(2),
            monto: Some(3),
            clase: Some(4),
            tipo: Some(5),
            ubicacion: Some(6),
            superficie: None,
            delegacion: None,
            subdelegacion: None,
        }
    }

    fn desde_encabezado(enc: &[String]) -> Result<Self, String> {
        let numero = indice_columna(enc, ALIAS_NUMERO)
            .ok_or_else(|| "Falta la columna NÚMERO de registro de obra".to_string())?;
        Ok(Columnas {
            numero,
            inicio: indice_columna(enc, ALIAS_INICIO),
            fin: indice_columna(enc, ALIAS_FIN),
            monto: indice_columna(enc, ALIAS_MONTO),
            clase: indice_columna(enc, ALIAS_CLASE),
            tipo: indice_columna(enc, ALIAS_TIPO),
            ubicacion: indice_columna(enc, ALIAS_UBICACION),
            superficie: indice_columna(enc, ALIAS_SUPERFICIE),
            delegacion: indice_columna(enc, ALIAS_DELEGACION),
            subdelegacion: indice_columna(enc, ALIAS_SUBDELEGACION),
        })
    }
}

/// Clave de comparación de números de registro: sin espacios y en mayúsculas.
fn clave_numero(s: &str) -> String {
    s.split_whitespace().collect::<String>().to_uppercase()
}

fn celda(celdas: &[String], idx: Option<usize>) -> &str {
    idx.and_then(|i| celdas.get(i)).map(|s| s.trim()).unwrap_or("")
}

/// Interpreta la tabla del lote. Si el primer renglón no reconoce ninguna columna, asume pegado sin
/// encabezado y usa el orden posicional. Los renglones totalmente vacíos se omiten.
pub fn interpretar_tabla_obras(tabla: &TablaCruda) -> Result<Vec<ObraLoteFila>, String> {
    let Some((_, primero)) = tabla.first() else {
        return Err("La tabla no tiene renglones.".to_string());
    };

    let todas_alias: [&[&str]; 10] = [
        ALIAS_NUMERO,
        ALIAS_INICIO,
        ALIAS_FIN,
        ALIAS_MONTO,
        ALIAS_CLASE,
        ALIAS_TIPO,
        ALIAS_UBICACION,
        ALIAS_SUPERFICIE,
        ALIAS_DELEGACION,
        ALIAS_SUBDELEGACION,
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

fn interpretar_renglon(renglon: usize, celdas: &[String], cols: &Columnas) -> ObraLoteFila {
    let mut errores: Vec<String> = Vec::new();

    let numero = clave_numero(celda(celdas, Some(cols.numero)));

    let txt_inicio = celda(celdas, cols.inicio);
    let mut fecha_inicio = None;
    let mut inicio_invalida = false;
    if !txt_inicio.is_empty() {
        match parse_fecha_flexible(txt_inicio) {
            Some(f) => fecha_inicio = Some(f),
            None => {
                inicio_invalida = true;
                errores.push(format!("Fecha de inicio inválida: '{txt_inicio}'"));
            }
        }
    }

    let txt_fin = celda(celdas, cols.fin);
    let mut fecha_fin = None;
    let mut fin_invalida = false;
    if !txt_fin.is_empty() {
        match parse_fecha_flexible(txt_fin) {
            Some(f) => fecha_fin = Some(f),
            None => {
                fin_invalida = true;
                errores.push(format!("Fecha de fin inválida: '{txt_fin}'"));
            }
        }
    }

    let monto = match parse_monto_flexible(celda(celdas, cols.monto)) {
        Ok(m) => m,
        Err(e) => {
            errores.push(e);
            None
        }
    };

    let superficie = match parse_monto_flexible(celda(celdas, cols.superficie)) {
        Ok(Some(v)) => v,
        Ok(None) => 0.0,
        Err(e) => {
            errores.push(e.replace("Monto", "Superficie"));
            0.0
        }
    };

    let mut clase = celda(celdas, cols.clase).to_uppercase();
    if clase.is_empty() {
        clase = "PRIVADA".to_string();
    }

    let mut fila = ObraLoteFila {
        renglon,
        numero: numero.clone(),
        fecha_inicio,
        fecha_fin,
        monto,
        clase,
        tipo_obra: celda(celdas, cols.tipo).to_string(),
        ubicacion: celda(celdas, cols.ubicacion).to_string(),
        superficie,
        delegacion: celda(celdas, cols.delegacion).to_string(),
        subdelegacion: celda(celdas, cols.subdelegacion).to_string(),
        errores: Vec::new(),
    };

    match validar_obra(&numero, fecha_inicio, fecha_fin, monto) {
        Ok(valida) => {
            fila.numero = valida.numero;
            fila.monto = valida.monto;
            fila.fecha_inicio = Some(valida.fecha_inicio);
            fila.fecha_fin = Some(valida.fecha_fin);
        }
        Err(msgs) => {
            for m in msgs {
                // Evita duplicar el mensaje de fecha ya reportado como "inválida".
                let duplicada = (inicio_invalida && m.contains("fecha de inicio"))
                    || (fin_invalida && m.contains("fecha final"));
                if !duplicada {
                    errores.push(m);
                }
            }
        }
    }

    fila.errores = errores;
    fila
}

/// Clasifica las filas del lote frente a las obras existentes de la revisión.
pub fn clasificar(filas: Vec<ObraLoteFila>, existentes: &[ObraExistente]) -> Vec<FilaPrevia> {
    let mut mapa: HashMap<String, &ObraExistente> = HashMap::new();
    for o in existentes {
        mapa.entry(clave_numero(&o.numero)).or_insert(o);
    }

    let mut vistos: HashSet<String> = HashSet::new();
    filas
        .into_iter()
        .map(|fila| {
            let clave = clave_numero(&fila.numero);
            let repetida = !clave.is_empty() && !vistos.insert(clave.clone());

            let (estado, accion, posibles): (EstadoFila, AccionFila, Vec<AccionFila>) = if !fila.errores.is_empty() {
                (EstadoFila::Error, AccionFila::Omitir, vec![AccionFila::Omitir])
            } else if repetida {
                (EstadoFila::RepetidaEnLote, AccionFila::Omitir, vec![AccionFila::Omitir])
            } else if let Some(existente) = mapa.get(&clave) {
                if existente.creado_usuario {
                    (
                        EstadoFila::ExisteUsuario,
                        AccionFila::Omitir,
                        vec![AccionFila::Omitir, AccionFila::Actualizar],
                    )
                } else {
                    (
                        EstadoFila::ExisteCedula,
                        AccionFila::Omitir,
                        vec![AccionFila::Omitir, AccionFila::Adoptar],
                    )
                }
            } else if fila.monto.is_none() {
                (EstadoFila::SinMonto, AccionFila::Crear, vec![AccionFila::Crear, AccionFila::Omitir])
            } else {
                (EstadoFila::Valida, AccionFila::Crear, vec![AccionFila::Crear, AccionFila::Omitir])
            };

            FilaPrevia {
                fila,
                estado,
                accion,
                acciones_posibles: posibles,
            }
        })
        .collect()
}

/// Conteos de la vista previa: (crear, actualizar, adoptar, omitir, error, sin_monto).
pub fn resumen_previa(p: &[FilaPrevia]) -> (usize, usize, usize, usize, usize, usize) {
    let mut r = (0, 0, 0, 0, 0, 0);
    for f in p {
        match f.accion {
            AccionFila::Crear => r.0 += 1,
            AccionFila::Actualizar => r.1 += 1,
            AccionFila::Adoptar => r.2 += 1,
            AccionFila::Omitir => r.3 += 1,
        }
        if f.estado == EstadoFila::Error {
            r.4 += 1;
        }
        if f.estado == EstadoFila::SinMonto {
            r.5 += 1;
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carga_masiva::tabla_desde_texto_pegado;

    fn d(a: i32, m: u32, dd: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(a, m, dd)
    }

    const ENCABEZADO: &str = "NUMERO\tINICIO\tFIN\tMONTO\tCLASE\tTIPO\tUBICACION";

    #[test]
    fn pegado_con_encabezado() {
        let texto = format!("{ENCABEZADO}\nc1234567\t01/01/2025\t31/12/2025\t$ 1,000.50\tpublica\tvivienda\tCDMX");
        let filas = interpretar_tabla_obras(&tabla_desde_texto_pegado(&texto)).unwrap();
        assert_eq!(filas.len(), 1);
        let f = &filas[0];
        assert_eq!(f.renglon, 2);
        assert_eq!(f.numero, "C1234567");
        assert_eq!(f.fecha_inicio, d(2025, 1, 1));
        assert_eq!(f.fecha_fin, d(2025, 12, 31));
        assert_eq!(f.monto, Some(1000.5));
        assert_eq!(f.clase, "PUBLICA");
        assert_eq!(f.tipo_obra, "vivienda");
        assert_eq!(f.ubicacion, "CDMX");
        assert_eq!(f.superficie, 0.0);
        assert!(f.errores.is_empty());
    }

    #[test]
    fn pegado_sin_encabezado_posicional() {
        let texto = "C1234567\t01/01/2025\t31/12/2025\t500\tA\tEDIFICACION\tPUEBLA";
        let filas = interpretar_tabla_obras(&tabla_desde_texto_pegado(texto)).unwrap();
        assert_eq!(filas.len(), 1);
        assert_eq!(filas[0].numero, "C1234567");
        assert_eq!(filas[0].monto, Some(500.0));
        assert_eq!(filas[0].clase, "A");
        assert_eq!(filas[0].ubicacion, "PUEBLA");
    }

    #[test]
    fn falta_columna_numero_es_error() {
        let t = tabla_desde_texto_pegado("INICIO\tFIN\n01/01/2025\t31/12/2025");
        assert_eq!(
            interpretar_tabla_obras(&t).unwrap_err(),
            "Falta la columna NÚMERO de registro de obra"
        );
    }

    #[test]
    fn lote_mixto_clasificado() {
        let texto = format!(
            "{ENCABEZADO}\n\
             C1000001\t01/01/2025\t31/12/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
             C1000002\t01/01/2025\t31/12/2025\t\tPUBLICA\tOBRA\tCDMX\n\
             C1000003\t31/12/2025\t01/01/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
             X\t01/01/2025\t31/12/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
             C1000001\t01/01/2025\t31/12/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
             C2000001\t01/01/2025\t31/12/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
             C2000002\t01/01/2025\t31/12/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
             \t\t\t\t\t\t\n"
        );
        let filas = interpretar_tabla_obras(&tabla_desde_texto_pegado(&texto)).unwrap();
        assert_eq!(filas.len(), 7);
        let existentes = vec![
            ObraExistente { numero: "C2000001".into(), creado_usuario: true, activa: true },
            ObraExistente { numero: "c2000002".into(), creado_usuario: false, activa: true },
        ];
        let p = clasificar(filas, &existentes);

        assert_eq!(p[0].estado, EstadoFila::Valida);
        assert_eq!(p[0].accion, AccionFila::Crear);
        assert_eq!(p[0].acciones_posibles, vec![AccionFila::Crear, AccionFila::Omitir]);

        assert_eq!(p[1].estado, EstadoFila::SinMonto);
        assert_eq!(p[1].accion, AccionFila::Crear);

        assert_eq!(p[2].estado, EstadoFila::Error);
        assert_eq!(p[2].accion, AccionFila::Omitir);
        assert_eq!(p[2].acciones_posibles, vec![AccionFila::Omitir]);

        assert_eq!(p[3].estado, EstadoFila::Error);
        assert_eq!(p[3].accion, AccionFila::Omitir);

        assert_eq!(p[4].estado, EstadoFila::RepetidaEnLote);
        assert_eq!(p[4].accion, AccionFila::Omitir);
        assert_eq!(p[4].acciones_posibles, vec![AccionFila::Omitir]);

        assert_eq!(p[5].estado, EstadoFila::ExisteUsuario);
        assert_eq!(p[5].accion, AccionFila::Omitir);
        assert_eq!(p[5].acciones_posibles, vec![AccionFila::Omitir, AccionFila::Actualizar]);

        assert_eq!(p[6].estado, EstadoFila::ExisteCedula);
        assert_eq!(p[6].accion, AccionFila::Omitir);
        assert_eq!(p[6].acciones_posibles, vec![AccionFila::Omitir, AccionFila::Adoptar]);
    }

    #[test]
    fn resumen_cuenta_por_accion_y_estado() {
        let texto = format!(
            "{ENCABEZADO}\n\
             C1000001\t01/01/2025\t31/12/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
             C1000002\t01/01/2025\t31/12/2025\t\tPUBLICA\tOBRA\tCDMX\n\
             C1000003\t31/12/2025\t01/01/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
             C2000001\t01/01/2025\t31/12/2025\t1000\tPUBLICA\tOBRA\tCDMX\n\
             C2000002\t01/01/2025\t31/12/2025\t1000\tPUBLICA\tOBRA\tCDMX\n"
        );
        let filas = interpretar_tabla_obras(&tabla_desde_texto_pegado(&texto)).unwrap();
        let existentes = vec![
            ObraExistente { numero: "C2000001".into(), creado_usuario: true, activa: true },
            ObraExistente { numero: "C2000002".into(), creado_usuario: false, activa: false },
        ];
        let mut p = clasificar(filas, &existentes);
        assert_eq!(resumen_previa(&p), (2, 0, 0, 3, 1, 1));
        p[3].accion = AccionFila::Actualizar;
        p[4].accion = AccionFila::Adoptar;
        assert_eq!(resumen_previa(&p), (2, 1, 1, 1, 1, 1));
    }
}
