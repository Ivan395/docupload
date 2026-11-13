
//! Lectura de tablas para cargas masivas (pegado desde Excel, CSV o XLSX).
//!
//! Funciones puras de parseo (fechas, montos, encabezados) más los lectores de texto y archivo.
//! Reutiliza la misma convención de `asistente_asignacion`: `TablaCruda` con (renglón base 1, celdas)
//! y el primer elemento como encabezado.

use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};
use chrono::NaiveDate;

/// Tabla cruda: pares (número de renglón base 1, celdas). El primer elemento es el encabezado.
pub type TablaCruda = Vec<(usize, Vec<String>)>;

/// Serial de Excel 0 = 1899-12-30 (convención usada por Excel para días a partir de 1900).
fn base_serial_excel() -> NaiveDate {
    NaiveDate::from_ymd_opt(1899, 12, 30).expect("fecha base de Excel válida")
}

/// Pegado de Excel (separador TAB). Si ninguna línea tiene TAB, usa la detección `;`/`,` de CSV.
/// Omite renglones totalmente vacíos.
pub fn tabla_desde_texto_pegado(texto: &str) -> TablaCruda {
    let texto = texto.strip_prefix('\u{feff}').unwrap_or(texto);
    if texto.lines().any(|l| l.contains('\t')) {
        texto
            .lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.split('\t').map(|c| c.trim().to_string()).collect::<Vec<_>>()))
            .filter(|(_, celdas)| celdas.iter().any(|c| !c.is_empty()))
            .collect()
    } else {
        tabla_csv(texto)
    }
}

/// Lee un archivo `.csv`, `.xlsx` o `.xlsm` (primera hoja).
pub fn tabla_desde_archivo(path: &Path) -> Result<TablaCruda, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "csv" => {
            let bytes = std::fs::read(path).map_err(|e| format!("No se pudo leer el archivo: {e}"))?;
            Ok(tabla_csv(&String::from_utf8_lossy(&bytes)))
        }
        "xlsx" | "xlsm" => tabla_xlsx(path),
        _ => Err("Formato no soportado: use un archivo .csv o .xlsx".to_string()),
    }
}

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

/// Convierte una celda de Excel a texto. Las fechas se devuelven como `AAAA-MM-DD`;
/// los enteros guardados como flotante no llevan decimales.
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
        Data::DateTime(dt) => {
            let serial = dt.as_f64();
            match serial_a_fecha(serial) {
                Some(f) => f.format("%Y-%m-%d").to_string(),
                None => serial.to_string(),
            }
        }
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(_) | Data::Empty => String::new(),
    }
}

/// Serial de Excel (parte entera) a fecha. Acepta solo el rango 20000..=80000 (años ~1954 a ~2119).
fn serial_a_fecha(serial: f64) -> Option<NaiveDate> {
    if !(20000.0..=80000.0).contains(&serial) {
        return None;
    }
    base_serial_excel().checked_add_signed(chrono::Duration::days(serial.trunc() as i64))
}

/// Normaliza un encabezado: mayúsculas, sin acentos, `.`/`_`/`-` tratados como espacio,
/// espacios colapsados y sin espacios al inicio o al final.
pub fn normalizar_encabezado(s: &str) -> String {
    let mapeado: String = s
        .to_uppercase()
        .chars()
        .map(|c| match c {
            'Á' => 'A',
            'É' => 'E',
            'Í' => 'I',
            'Ó' => 'O',
            'Ú' | 'Ü' => 'U',
            'Ñ' => 'N',
            '.' | '_' | '-' => ' ',
            c => c,
        })
        .collect();
    mapeado.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Índice de la primera columna del encabezado cuyo nombre normalizado coincide con algún alias.
pub fn indice_columna(encabezado: &[String], alias: &[&str]) -> Option<usize> {
    let alias: Vec<String> = alias.iter().map(|a| normalizar_encabezado(a)).collect();
    encabezado
        .iter()
        .position(|celda| alias.contains(&normalizar_encabezado(celda)))
}

/// Parsea una fecha en `DD/MM/AAAA`, `D/M/AAAA`, `AAAA-MM-DD`, `DD-MM-AAAA` o serial de Excel.
/// Acepta también texto con hora (`AAAA-MM-DD 00:00:00`, `AAAA-MM-DDT00:00:00`).
pub fn parse_fecha_flexible(s: &str) -> Option<NaiveDate> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // Fecha ISO con hora (salida de calamine para celdas de fecha).
    if let Some(prefijo) = t.get(..10) {
        let resto = &t[10..];
        if (resto.is_empty() || resto.starts_with(' ') || resto.starts_with('T'))
            && prefijo.as_bytes().get(4) == Some(&b'-')
        {
            if let Ok(f) = NaiveDate::parse_from_str(prefijo, "%Y-%m-%d") {
                return Some(f);
            }
        }
    }
    for formato in ["%d/%m/%Y", "%Y-%m-%d", "%d-%m-%Y"] {
        if let Ok(f) = NaiveDate::parse_from_str(t, formato) {
            return Some(f);
        }
    }
    // Serial de Excel (entero o decimal).
    if let Ok(serial) = t.parse::<f64>() {
        if serial.is_finite() {
            return serial_a_fecha(serial);
        }
    }
    None
}

/// Parsea un monto. Quita `$`, `MXN` y espacios. Vacío → `Ok(None)`.
/// Si hay `.` y `,`, el último es el decimal. Si solo hay `,` con 1-2 dígitos al final, es decimal;
/// si no, es separador de miles.
pub fn parse_monto_flexible(s: &str) -> Result<Option<f64>, String> {
    let original = s.trim();
    let limpio: String = original
        .replace("MXN", "")
        .replace("mxn", "")
        .replace('$', "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if limpio.is_empty() {
        return Ok(None);
    }

    let ultimo_punto = limpio.rfind('.');
    let ultima_coma = limpio.rfind(',');
    let normalizado = match (ultimo_punto, ultima_coma) {
        (Some(p), Some(c)) => {
            if p > c {
                // "1,234.56": el punto es decimal, las comas son miles.
                limpio.replace(',', "")
            } else {
                // "1.234,56": la coma es decimal, los puntos son miles.
                limpio.replace('.', "").replace(',', ".")
            }
        }
        (None, Some(c)) => {
            let digitos_tras = limpio[c + 1..].chars().count();
            if (1..=2).contains(&digitos_tras) && limpio.matches(',').count() == 1 {
                limpio.replace(',', ".")
            } else {
                limpio.replace(',', "")
            }
        }
        (Some(_), None) => {
            if limpio.matches('.').count() > 1 {
                limpio.replace('.', "")
            } else {
                limpio
            }
        }
        (None, None) => limpio,
    };

    match normalizado.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(Some(v)),
        _ => Err(format!("Monto inválido: '{original}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fechas_en_los_cuatro_formatos_y_serial() {
        let esperado = NaiveDate::from_ymd_opt(2025, 1, 31);
        assert_eq!(parse_fecha_flexible("31/01/2025"), esperado);
        assert_eq!(parse_fecha_flexible("31/1/2025"), esperado);
        assert_eq!(parse_fecha_flexible("2025-01-31"), esperado);
        assert_eq!(parse_fecha_flexible("31-01-2025"), esperado);
        assert_eq!(parse_fecha_flexible("45658"), NaiveDate::from_ymd_opt(2025, 1, 1));
        assert_eq!(parse_fecha_flexible("45658.75"), NaiveDate::from_ymd_opt(2025, 1, 1));
    }

    #[test]
    fn fechas_con_hora_de_calamine_y_invalidas() {
        let esperado = NaiveDate::from_ymd_opt(2025, 1, 31);
        assert_eq!(parse_fecha_flexible("2025-01-31 00:00:00"), esperado);
        assert_eq!(parse_fecha_flexible("2025-01-31T00:00:00"), esperado);
        assert_eq!(parse_fecha_flexible(""), None);
        assert_eq!(parse_fecha_flexible("mañana"), None);
        assert_eq!(parse_fecha_flexible("31/02/2025"), None);
        assert_eq!(parse_fecha_flexible("12"), None);
    }

    #[test]
    fn montos_formatos_comunes() {
        assert_eq!(parse_monto_flexible("1,234.56"), Ok(Some(1234.56)));
        assert_eq!(parse_monto_flexible("1.234,56"), Ok(Some(1234.56)));
        assert_eq!(parse_monto_flexible("1234,5"), Ok(Some(1234.5)));
        assert_eq!(parse_monto_flexible("$ 2,000,000"), Ok(Some(2_000_000.0)));
        assert_eq!(parse_monto_flexible("1,234"), Ok(Some(1234.0)));
        assert_eq!(parse_monto_flexible("1.5"), Ok(Some(1.5)));
        assert_eq!(parse_monto_flexible("150000 MXN"), Ok(Some(150_000.0)));
    }

    #[test]
    fn montos_vacio_e_invalido() {
        assert_eq!(parse_monto_flexible(""), Ok(None));
        assert_eq!(parse_monto_flexible("   "), Ok(None));
        assert_eq!(parse_monto_flexible("$"), Ok(None));
        assert_eq!(parse_monto_flexible("abc"), Err("Monto inválido: 'abc'".to_string()));
    }

    #[test]
    fn normalizar_quita_acentos_y_separadores() {
        assert_eq!(normalizar_encabezado("  Número de  Registro. "), "NUMERO DE REGISTRO");
        assert_eq!(normalizar_encabezado("Delegación_Subdelegación"), "DELEGACION SUBDELEGACION");
        assert_eq!(normalizar_encabezado("fecha-inicio"), "FECHA INICIO");
    }

    #[test]
    fn indice_columna_por_alias() {
        let enc: Vec<String> = ["Fecha de Inicio", "Registro de obra", "Monto"].iter().map(|s| s.to_string()).collect();
        assert_eq!(indice_columna(&enc, &["REGISTRO DE OBRA", "OBRA"]), Some(1));
        assert_eq!(indice_columna(&enc, &["INICIO", "FECHA DE INICIO"]), Some(0));
        assert_eq!(indice_columna(&enc, &["CLASE"]), None);
    }

    #[test]
    fn pegado_con_tabulador() {
        let t = tabla_desde_texto_pegado("NUMERO\tINICIO\tMONTO\nC1234567\t01/01/2025\t1,000\n\t\t\n");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].0, 1);
        assert_eq!(t[1].0, 2);
        assert_eq!(t[1].1, vec!["C1234567", "01/01/2025", "1,000"]);
    }

    #[test]
    fn pegado_sin_tabulador_usa_csv() {
        let t = tabla_desde_texto_pegado("NUMERO;INICIO\nC1234567;01/01/2025\n");
        assert_eq!(t[1].1, vec!["C1234567", "01/01/2025"]);
        let t2 = tabla_desde_texto_pegado("NUMERO,INICIO\n\"C1234567\",2025-01-01");
        assert_eq!(t2[1].1, vec!["C1234567", "2025-01-01"]);
    }

    #[test]
    fn archivo_extension_no_soportada() {
        let r = tabla_desde_archivo(Path::new("obras.txt"));
        assert_eq!(r.unwrap_err(), "Formato no soportado: use un archivo .csv o .xlsx");
    }
}
