use chrono::NaiveDate;
use fancy_regex::{Captures, Regex};
use indexmap::IndexMap;
use std::fmt;

use crate::cedulas::regex_expressions::OCRPatterns;

fn parse_float(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%d/%m/%Y").ok()
}

#[derive(Debug, Clone, Default)]
pub struct DataEmployee {
    pub mes_periodo: Option<String>,
    pub anio_periodo: Option<String>,
    pub clave: Option<String>,
    pub fecha: Option<NaiveDate>,
    pub dias: Option<f64>,
    pub sdi: Option<f64>,
    pub lic: Option<f64>,
    pub inc: Option<f64>,
    pub aus: Option<f64>,
    pub cf: Option<f64>,
    pub exc_pat: Option<f64>,
    pub exc_obr: Option<f64>,
    pub pd_pat: Option<f64>,
    pub pd_obr: Option<f64>,
    pub gmp_pat: Option<f64>,
    pub gmp_obr: Option<f64>,
    pub rt: Option<f64>,
    pub iv_pat: Option<f64>,
    pub iv_obr: Option<f64>,
    pub gps: Option<f64>,
    pub patronal: Option<f64>,
    pub obrera: Option<f64>,
    pub subtotal: Option<f64>,
}

impl DataEmployee {
    pub fn set_field(&mut self, field_name: &str, raw_value: &str) {
        let trimmed = raw_value.trim();
        if trimmed.is_empty() {
            return;
        }

        match field_name {
            "mes_periodo" => self.mes_periodo = Some(trimmed.to_string()),
            "anio_periodo" => self.anio_periodo = Some(trimmed.to_string()),
            "clave" => self.clave = Some(trimmed.to_string()),
            "fecha" => self.fecha = parse_date(trimmed),
            "dias" => self.dias = parse_float(trimmed),
            "sdi" => self.sdi = parse_float(trimmed),
            "lic" => self.lic = parse_float(trimmed),
            "inc" => self.inc = parse_float(trimmed),
            "aus" => self.aus = parse_float(trimmed),
            "cf" => self.cf = parse_float(trimmed),
            "exc_pat" => self.exc_pat = parse_float(trimmed),
            "exc_obr" => self.exc_obr = parse_float(trimmed),
            "pd_pat" => self.pd_pat = parse_float(trimmed),
            "pd_obr" => self.pd_obr = parse_float(trimmed),
            "gmp_pat" => self.gmp_pat = parse_float(trimmed),
            "gmp_obr" => self.gmp_obr = parse_float(trimmed),
            "rt" => self.rt = parse_float(trimmed),
            "iv_pat" => self.iv_pat = parse_float(trimmed),
            "iv_obr" => self.iv_obr = parse_float(trimmed),
            "gps" => self.gps = parse_float(trimmed),
            "patronal" => self.patronal = parse_float(trimmed),
            "obrera" => self.obrera = parse_float(trimmed),
            "subtotal" => self.subtotal = parse_float(trimmed),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistroObra {
    pub registro_obra: String,
    pub dias_registro_obra: i32,
}

impl RegistroObra {
    pub fn new(registro_obra: &str, dias_registro_obra: &str) -> Self {
        Self {
            registro_obra: registro_obra.to_uppercase(),
            dias_registro_obra: dias_registro_obra.parse::<i32>().unwrap_or(0),
        }
    }
}

impl fmt::Display for RegistroObra {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.registro_obra, self.dias_registro_obra)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Employee {
    pub no_seguro_social: Option<String>,
    pub nombre: Option<String>,
    pub rfc: Option<String>,
    pub curp: Option<String>,
    pub clave_ubicacion: Option<String>,
    pub pensionado: Option<String>,
    pub mes_periodo: Option<String>,
    pub anio_periodo: Option<String>,
    pub data_employee: Vec<DataEmployee>,
    pub registro_obras: Vec<RegistroObra>,
}

#[derive(Debug, Clone)]
pub struct EmployeeFlatRow {
    pub no_seguro_social: Option<String>,
    pub nombre: Option<String>,
    pub rfc: Option<String>,
    pub curp: Option<String>,
    pub clave_ubicacion: Option<String>,
    pub pensionado: Option<String>,
    pub registro_obras: String,
    pub mes_periodo: Option<String>,
    pub anio_periodo: Option<String>,
    pub clave: Option<String>,
    pub fecha: Option<String>,
    pub dias: Option<f64>,
    pub sdi: Option<f64>,
    pub lic: Option<f64>,
    pub inc: Option<f64>,
    pub aus: Option<f64>,
    pub cf: Option<f64>,
    pub exc_pat: Option<f64>,
    pub exc_obr: Option<f64>,
    pub pd_pat: Option<f64>,
    pub pd_obr: Option<f64>,
    pub gmp_pat: Option<f64>,
    pub gmp_obr: Option<f64>,
    pub rt: Option<f64>,
    pub iv_pat: Option<f64>,
    pub iv_obr: Option<f64>,
    pub gps: Option<f64>,
    pub patronal: Option<f64>,
    pub obrera: Option<f64>,
    pub subtotal: Option<f64>,
}

pub fn is_curp_candidate(s: &str) -> bool {
    let clean = crate::validation::clean_alphanumeric_input(s);
    if clean.len() < 17 || clean.len() > 18 {
        return false;
    }
    let chars: Vec<char> = clean.chars().collect();
    chars[0..4].iter().all(|c| c.is_ascii_alphabetic())
        && chars[4..10].iter().all(|c| c.is_ascii_digit())
}

pub fn is_rfc_candidate(s: &str) -> bool {
    let clean = crate::validation::clean_alphanumeric_input(s);
    if clean.len() < 10 || clean.len() > 13 {
        return false;
    }
    let chars: Vec<char> = clean.chars().collect();
    if clean.len() == 12 {
        chars[0..3].iter().all(|c| c.is_ascii_alphabetic() || *c == '&' || *c == 'Ñ')
            && chars[3..9].iter().all(|c| c.is_ascii_digit())
    } else {
        chars[0..4].iter().all(|c| c.is_ascii_alphabetic() || *c == '&' || *c == 'Ñ')
            && chars[4..10].iter().all(|c| c.is_ascii_digit())
    }
}

/// Resultado del troceado de la línea de encabezado de un trabajador.
#[derive(Debug, Default, PartialEq)]
pub struct EncabezadoTokens {
    pub nombre: String,
    pub rfc: Option<String>,
    pub curp: Option<String>,
    pub clave_ubicacion: Option<String>,
    pub pensionado: Option<String>,
    pub registro_obras: Vec<RegistroObra>,
}

/// Intenta interpretar un token como registro de obra (con o sin días).
/// Formatos soportados:
/// - "C0617880-15", "C0640941-7", "C0640943-9", "C0640941-23"
/// - "C0640941" (formato SIROC sin días explícitos)
/// - "08-12345-67" o "081234567-30" (formato SATIC)
pub fn try_parse_obra_token(cand: &str) -> Option<RegistroObra> {
    let raw = cand.trim();
    if raw.is_empty() || raw == "—" || raw == "-" {
        return None;
    }

    // 1. Formato OBRA-DIAS (ej. C0617880-15, C0640941-7, C0640943-9)
    if let Some((obra_part, dias_part)) = raw.rsplit_once('-') {
        let clean_obra = crate::validation::clean_alphanumeric_input(obra_part);
        if let Ok(dias) = dias_part.trim().parse::<i32>() {
            if (0..=31).contains(&dias) {
                if crate::validation::validate_registro_obra(&clean_obra).is_ok()
                    || (clean_obra.starts_with('C') && (clean_obra.len() == 8 || clean_obra.len() == 9))
                {
                    return Some(RegistroObra {
                        registro_obra: clean_obra,
                        dias_registro_obra: dias,
                    });
                }
            }
        }
    }

    // 2. Formato SIROC / SATIC sin días (ej. C0640941)
    let clean_cand = crate::validation::clean_alphanumeric_input(raw);
    if crate::validation::validate_registro_obra(&clean_cand).is_ok() {
        return Some(RegistroObra {
            registro_obra: clean_cand,
            dias_registro_obra: 0,
        });
    }

    None
}

/// Extrae identificadores de pensionado (P/CV, P/IV, P, etc.) de una lista de tokens.
/// Devuelve el tipo de pensión normalizado (Option<String>) y los tokens restantes limpios.
pub fn extract_pension_info<'a>(tokens: &'a [&'a str]) -> (Option<String>, Vec<&'a str>) {
    if tokens.is_empty() {
        return (None, Vec::new());
    }

    let mut pension: Option<String> = None;
    let mut kept: Vec<&'a str> = Vec::with_capacity(tokens.len());
    let mut i = 0;

    while i < tokens.len() {
        let t = tokens[i].trim();
        let upper = t.to_uppercase();
        let normalized = upper.replace(['.', ' '], "");

        // Secuencia de 3 tokens: "P" + ("/" | "-") + ("CV" | "IV")
        if (upper == "P" || upper == "P.") && i + 2 < tokens.len() && (tokens[i + 1] == "/" || tokens[i + 1] == "-") {
            let next2 = tokens[i + 2].trim().to_uppercase().replace(['.', ' '], "");
            if next2 == "CV" {
                if pension.is_none() {
                    pension = Some("P/CV".to_string());
                }
                i += 3;
                continue;
            } else if next2 == "IV" {
                if pension.is_none() {
                    pension = Some("P/IV".to_string());
                }
                i += 3;
                continue;
            }
        }

        // Secuencias de 2 tokens:
        // "P/" o "P-" seguido de "CV" o "IV"
        // "P" seguido de "/CV", "-CV", "CV", "/IV", "-IV", "IV"
        if i + 1 < tokens.len() {
            let next = tokens[i + 1].trim().to_uppercase().replace(['.', ' '], "");
            if (normalized == "P/" || normalized == "P-") && (next == "CV" || next == "IV") {
                if pension.is_none() {
                    pension = Some(format!("P/{}", next));
                }
                i += 2;
                continue;
            }
            if normalized == "P" && (next == "/CV" || next == "-CV" || next == "CV") {
                if pension.is_none() {
                    pension = Some("P/CV".to_string());
                }
                i += 2;
                continue;
            }
            if normalized == "P" && (next == "/IV" || next == "-IV" || next == "IV") {
                if pension.is_none() {
                    pension = Some("P/IV".to_string());
                }
                i += 2;
                continue;
            }
        }

        // Token único: "P/CV", "P-CV", "PCV", "P/C.V.", "P/IV", "P-IV", "PIV", "P/I.V."
        if normalized == "P/CV" || normalized == "P-CV" || normalized == "PCV" {
            if pension.is_none() {
                pension = Some("P/CV".to_string());
            }
            i += 1;
            continue;
        }
        if normalized == "P/IV" || normalized == "P-IV" || normalized == "PIV" {
            if pension.is_none() {
                pension = Some("P/IV".to_string());
            }
            i += 1;
            continue;
        }

        // Standalone "P" o "P." al inicio si va seguido de al menos 2 tokens (apellidos y nombre)
        if i == 0 && (upper == "P" || upper == "P.") && tokens.len() >= 3 {
            if pension.is_none() {
                pension = Some("P".to_string());
            }
            i += 1;
            continue;
        }

        kept.push(tokens[i]);
        i += 1;
    }

    (pension, kept)
}

/// Limpia un nombre completo extrayendo cualquier indicador de pensión que contenga.
pub fn clean_name_and_extract_pension(raw_name: &str) -> (String, Option<String>) {
    let tokens: Vec<&str> = raw_name.split_whitespace().collect();
    if tokens.is_empty() {
        return (String::new(), None);
    }
    let (pension, clean_tokens) = extract_pension_info(&tokens);
    (clean_tokens.join(" "), pension)
}

/// Trocea la parte de la línea que sigue al NSS en tokens y se queda con los
/// identificadores anclados al final, asignando al nombre todo lo anterior.
///
/// Soporta múltiples tokens de registros de obras (ej. C0617880-15 C0640941-7 C0640943-9)
/// evitando que se queden incrustados en el nombre del trabajador.
pub fn parse_encabezado_tokens(resto: &str) -> EncabezadoTokens {
    let tokens: Vec<&str> = resto.split_whitespace().collect();
    let mut fin = tokens.len();

    let mut curp = None;
    let mut rfc = None;
    let mut clave = None;
    let mut reversed_obras: Vec<RegistroObra> = Vec::new();

    // Recorrido estrictamente de derecha a izquierda: en cada paso se decide qué
    // es el último token pendiente y se consume. Al primer token que no encaje
    // con ninguna categoría se detiene el bucle, de modo que todo lo que queda a
    // la izquierda es necesariamente el nombre.
    while fin > 0 {
        let cand = tokens[fin - 1];

        if curp.is_none() && is_curp_candidate(cand) {
            curp = Some(crate::validation::clean_alphanumeric_input(cand));
            fin -= 1;
            continue;
        }

        if rfc.is_none() && is_rfc_candidate(cand) {
            let clean_r = cand
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '&' || *c == 'Ñ')
                .collect::<String>()
                .to_uppercase();
            rfc = Some(clean_r);
            fin -= 1;
            continue;
        }

        // Múltiples tokens de obras de construcción (ej. C0640943-9, C0640941-7, C0617880-15)
        if fin > 1 {
            if let Some(ro) = try_parse_obra_token(cand) {
                reversed_obras.push(ro);
                fin -= 1;
                continue;
            }
        }

        // Clave de ubicación: token corto con al menos un dígito que no sea
        // identificador fiscal (p. ej. `01`, `C1206769-23`). Se exige que quede
        // al menos un token de nombre delante, y solo se acepta una vez.
        if clave.is_none() && fin > 1 {
            let tiene_digitos = cand.chars().any(|c| c.is_ascii_digit());
            let solo_alfanumerico = cand
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '(' || c == ')');
            if tiene_digitos && solo_alfanumerico && cand.len() <= 15 {
                clave = Some(cand.to_uppercase());
                fin -= 1;
                continue;
            }
        }

        break;
    }

    reversed_obras.reverse();

    let (pensionado, tokens_nombre) = extract_pension_info(&tokens[..fin]);
    let clean_nombre = tokens_nombre.join(" ").to_uppercase();

    EncabezadoTokens {
        nombre: clean_nombre,
        rfc,
        curp,
        clave_ubicacion: clave,
        pensionado: pensionado.map(|p| p.to_uppercase()),
        registro_obras: reversed_obras,
    }
}

impl Employee {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sanitize_employee_identifiers(
        rfc: &mut Option<String>,
        curp: &mut Option<String>,
        clave_ubicacion: &mut Option<String>,
        nss: Option<&str>,
        raw_data: &str,
    ) {
        // 1. Si clave_ubicacion tiene una CURP, moverla a curp si no existe y limpiar clave_ubicacion
        if let Some(ref ub) = clave_ubicacion.clone() {
            if is_curp_candidate(ub) {
                if curp.is_none() {
                    *curp = Some(ub.clone());
                }
                *clave_ubicacion = None;
            } else if is_rfc_candidate(ub) {
                if rfc.is_none() {
                    *rfc = Some(ub.clone());
                }
                *clave_ubicacion = None;
            }
        }

        // 2. Si rfc contiene una CURP (18 caracteres)
        if let Some(ref r) = rfc.clone() {
            if is_curp_candidate(r) {
                if curp.is_none() {
                    *curp = Some(r.clone());
                }
                *rfc = None;
            }
        }

        // 3. Si curp contiene un RFC (10-13 caracteres)
        if let Some(ref c) = curp.clone() {
            if is_rfc_candidate(c) {
                if rfc.is_none() {
                    *rfc = Some(c.clone());
                }
                *curp = None;
            }
        }

        // 4. Si clave_ubicacion es "—", "-", está vacía, no tiene dígitos (p. ej. un resto
        //    del nombre capturado por error), o es RFC/CURP, limpiarla
        if let Some(ref ub) = clave_ubicacion.clone() {
            let t = ub.trim();
            let sin_digitos = !t.chars().any(|c| c.is_ascii_digit());
            if t.is_empty()
                || t == "—"
                || t == "-"
                || sin_digitos
                || is_curp_candidate(t)
                || is_rfc_candidate(t)
            {
                *clave_ubicacion = None;
            }
        }

        // 5. Análisis token por token de la línea del empleado para capturar valores faltantes o desubicados
        let header_line = raw_data.lines().find(|l| {
            if let Some(n) = nss {
                l.contains(n)
            } else {
                false
            }
        });

        if let Some(line) = header_line {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() >= 2 {
                for tok in tokens.iter().skip(1) {
                    if is_curp_candidate(tok) && curp.is_none() {
                        *curp = Some(tok.to_string());
                    } else if is_rfc_candidate(tok) && rfc.is_none() {
                        *rfc = Some(tok.to_string());
                    }
                }

                // Asegurar nuevamente que clave_ubicacion nunca sea ni CURP ni RFC
                if clave_ubicacion.as_deref().map_or(false, |ub| is_curp_candidate(ub) || is_rfc_candidate(ub)) {
                    *clave_ubicacion = None;
                }
            }
        }

        // 6. Red de seguridad: descartar cualquier RFC/CURP que no tenga estructura fiscal válida.
        //    Impide que un fragmento de nombre (p. ej. "ALEJANDRA") quede guardado como RFC.
        if let Some(ref r) = rfc.clone() {
            if !is_rfc_candidate(r) {
                log::warn!("Descartando RFC sin estructura fiscal válida: '{}'", r);
                *rfc = None;
            }
        }
        if let Some(ref c) = curp.clone() {
            if !is_curp_candidate(c) {
                log::warn!("Descartando CURP sin estructura válida: '{}'", c);
                *curp = None;
            }
        }
    }

    pub fn set_values(
        &mut self,
        data: &str,
        columns: &[&str],
        date_period: Option<&Captures>,
        patterns: &OCRPatterns,
    ) {
        if let Some(dp) = date_period {
            if let Some(mes) = dp.name("mes_periodo") {
                self.mes_periodo = Some(mes.as_str().trim().to_string());
            }
            if let Some(anio) = dp.name("anio_periodo") {
                self.anio_periodo = Some(anio.as_str().trim().to_string());
            }
        }

        if let Ok(Some(caps)) = patterns
            .employee_no_seguro_social_nombre_curp_rfc
            .captures(data)
        {
            let raw_nss = caps
                .name("no_seguro_social")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            if !raw_nss.is_empty() {
                let clean_nss = raw_nss
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_uppercase();
                self.no_seguro_social = Some(clean_nss);
            }

            // A partir del NSS se resuelve el resto por tokens, no por posiciones:
            // los identificadores se toman anclados al final de la LÍNEA del
            // encabezado y el nombre recibe todo lo que quede a la izquierda.
            // `data` es un bloque multi-línea (el detalle completo del
            // trabajador), por lo que hay que recortarse a la línea que contiene
            // el NSS antes de trocear.
            let linea_encabezado = data.lines().find(|l| !raw_nss.is_empty() && l.contains(&raw_nss))
                .or_else(|| {
                    self.no_seguro_social.as_ref().and_then(|cn| {
                        data.lines().find(|l| {
                            let clean_line = crate::validation::clean_alphanumeric_input(l);
                            clean_line.contains(cn)
                        })
                    })
                });

            if let Some(linea) = linea_encabezado {
                let pos = if !raw_nss.is_empty() && linea.contains(&raw_nss) {
                    linea.find(&raw_nss).unwrap_or(0) + raw_nss.len()
                } else if let Some(ref cn) = self.no_seguro_social {
                    linea.find(cn).unwrap_or(0) + cn.len()
                } else {
                    0
                };
                let resto = linea[pos..].trim();
                let t = parse_encabezado_tokens(resto);
                if !t.nombre.is_empty() {
                    self.nombre = Some(t.nombre.to_uppercase());
                }
                self.rfc = t.rfc;
                self.curp = t.curp;
                self.clave_ubicacion = t.clave_ubicacion;
                self.pensionado = t.pensionado;
                for ro in t.registro_obras {
                    if !self.registro_obras.iter().any(|existing| existing.registro_obra == ro.registro_obra) {
                        self.registro_obras.push(ro);
                    }
                }
            }

            // Si el NSS no se encontró en la línea, se recurre al reparto posicional.
            if self.no_seguro_social.is_none() {
                if let Some(m) = caps.name("nombre") {
                    let (clean_nom, pens) = clean_name_and_extract_pension(m.as_str());
                    self.nombre = Some(clean_nom.to_uppercase());
                    if self.pensionado.is_none() {
                        self.pensionado = pens.map(|p| p.to_uppercase());
                    }
                }
                self.rfc = caps
                    .name("rfc")
                    .or_else(|| caps.name("rfc_alt"))
                    .map(|m| {
                        m.as_str()
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric() || *c == '&' || *c == 'Ñ')
                            .collect::<String>()
                            .to_uppercase()
                    });
                self.curp = caps
                    .name("curp")
                    .or_else(|| caps.name("curp_alt"))
                    .map(|m| {
                        m.as_str()
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric())
                            .collect::<String>()
                            .to_uppercase()
                    });
                self.clave_ubicacion = caps
                    .name("clave_ubicacion")
                    .map(|m| m.as_str().trim().to_uppercase());
            }

            // Garantizar que self.nombre quede siempre limpio y sin prefijos de pensionado
            if let Some(ref nom) = self.nombre {
                let (clean_nom, pens) = clean_name_and_extract_pension(nom);
                self.nombre = Some(clean_nom.to_uppercase());
                if self.pensionado.is_none() {
                    self.pensionado = pens.map(|p| p.to_uppercase());
                }
            }

            Self::sanitize_employee_identifiers(
                &mut self.rfc,
                &mut self.curp,
                &mut self.clave_ubicacion,
                self.no_seguro_social.as_deref(),
                data,
            );

            // Asegurar que RFC y CURP queden en mayúsculas y sin guiones
            if let Some(ref r) = self.rfc {
                self.rfc = Some(
                    r.chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '&' || *c == 'Ñ')
                        .collect::<String>()
                        .to_uppercase(),
                );
            }
            if let Some(ref c) = self.curp {
                self.curp = Some(
                    c.chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .collect::<String>()
                        .to_uppercase(),
                );
            }
        }

        let get_base_de = || {
            let mut de = DataEmployee::default();
            if let Some(dp) = date_period {
                if let Some(mes) = dp.name("mes_periodo") {
                    de.mes_periodo = Some(mes.as_str().trim().to_uppercase());
                }
                if let Some(anio) = dp.name("anio_periodo") {
                    de.anio_periodo = Some(anio.as_str().trim().to_uppercase());
                }
            }
            de
        };

        for caps in patterns.employee_data.captures_iter(data).flatten() {
            let mut de = get_base_de();
            let data_str = if let Some(nodate) = caps.name("data_without_date") {
                nodate.as_str()
            } else if let Some(withdate) = caps.name("data_with_date") {
                withdate.as_str()
            } else {
                ""
            };

            let data_values = data_str.split_whitespace();
            for (col, val) in columns.iter().zip(data_values) {
                de.set_field(col, val);
            }
            if let Some(m) = caps.name("clave") {
                de.set_field("clave", m.as_str());
            }
            if let Some(m) = caps.name("fecha") {
                de.set_field("fecha", m.as_str());
            }

            self.data_employee.push(de);
        }

        for caps in patterns.employee_fechas.captures_iter(data).flatten() {
            let mut de = get_base_de();
            if let Some(m) = caps.name("clave") {
                de.set_field("clave", m.as_str());
            }
            if let Some(m) = caps.name("fecha") {
                de.set_field("fecha", m.as_str());
            }
            self.data_employee.push(de);
        }

        for caps in patterns
            .employee_registro_obras
            .captures_iter(data)
            .flatten()
        {
            let obra = caps.name("registro_obra").map(|m| m.as_str()).unwrap_or("");
            let dias = caps
                .name("dias_registro_obra")
                .map(|m| m.as_str())
                .unwrap_or("");
            let ro = RegistroObra::new(obra, dias);
            if let Some(existing) = self.registro_obras.iter_mut().find(|existing| existing.registro_obra == ro.registro_obra) {
                if existing.dias_registro_obra == 0 {
                    existing.dias_registro_obra = ro.dias_registro_obra;
                }
            } else {
                self.registro_obras.push(ro);
            }
        }
    }

    pub fn to_list(&self) -> Vec<EmployeeFlatRow> {
        let registro_obras_str = self
            .registro_obras
            .iter()
            .map(|ro| ro.to_string())
            .collect::<Vec<String>>()
            .join(" ");

        self.data_employee
            .iter()
            .map(|de| EmployeeFlatRow {
                no_seguro_social: self.no_seguro_social.clone(),
                nombre: self.nombre.clone(),
                rfc: self.rfc.clone(),
                curp: self.curp.clone(),
                clave_ubicacion: self.clave_ubicacion.clone(),
                pensionado: self.pensionado.clone(),
                registro_obras: registro_obras_str.clone(),
                mes_periodo: de.mes_periodo.clone(),
                anio_periodo: de.anio_periodo.clone(),
                clave: de.clave.clone(),
                fecha: de.fecha.map(|d| d.format("%d/%m/%Y").to_string()),
                dias: de.dias,
                sdi: de.sdi,
                lic: de.lic,
                inc: de.inc,
                aus: de.aus,
                cf: de.cf,
                exc_pat: de.exc_pat,
                exc_obr: de.exc_obr,
                pd_pat: de.pd_pat,
                pd_obr: de.pd_obr,
                gmp_pat: de.gmp_pat,
                gmp_obr: de.gmp_obr,
                rt: de.rt,
                iv_pat: de.iv_pat,
                iv_obr: de.iv_obr,
                gps: de.gps,
                patronal: de.patronal,
                obrera: de.obrera,
                subtotal: de.subtotal,
            })
            .collect()
    }
}

pub struct Columns {
    pub columns_in_file: Vec<&'static str>,
}

impl Columns {
    pub fn new() -> Self {
        Self {
            columns_in_file: Vec::new(),
        }
    }

    pub fn get_column_names(
        &mut self,
        line: &str,
        column_names_patterns: &IndexMap<&'static str, Regex>,
        columns_in_file_re: &Regex,
    ) {
        let line_trimmed = line.trim();

        if let Ok(Some(_)) = columns_in_file_re.find(line_trimmed) {
            for (k, v) in column_names_patterns {
                if let Ok(Some(_)) = v.find(line_trimmed) {
                    if !self.columns_in_file.contains(k) {
                        self.columns_in_file.push(k);
                    }
                }
            }
            self.columns_in_file
                .retain(|x| *x != "clave" && *x != "fecha");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cedulas::regex_expressions::initialize_patterns;

    #[test]
    fn test_parse_employee_rfc_curp_and_ubicacion() {
        let patterns = initialize_patterns().unwrap();
        let sample = "01-85-64-0012-3   PEREZ LOPEZ JUAN   PELJ640101A12   PELJ640101HMCNLN01   01\n";
        let mut emp = Employee::new();
        emp.set_values(sample, &[], None, &patterns);

        assert_eq!(emp.no_seguro_social.as_deref(), Some("01856400123"));
        assert_eq!(emp.nombre.as_deref(), Some("PEREZ LOPEZ JUAN"));
        assert_eq!(emp.rfc.as_deref(), Some("PELJ640101A12"));
        assert_eq!(emp.curp.as_deref(), Some("PELJ640101HMCNLN01"));
        assert_eq!(emp.clave_ubicacion.as_deref(), Some("01"));
    }

    #[test]
    fn test_parse_employee_curp_first_then_rfc() {
        let patterns = initialize_patterns().unwrap();
        let sample = "01-85-64-0012-3   PEREZ LOPEZ JUAN   PELJ640101HMCNLN01   PELJ640101A12   01\n";
        let mut emp = Employee::new();
        emp.set_values(sample, &[], None, &patterns);

        assert_eq!(emp.no_seguro_social.as_deref(), Some("01856400123"));
        assert_eq!(emp.nombre.as_deref(), Some("PEREZ LOPEZ JUAN"));
        assert_eq!(emp.curp.as_deref(), Some("PELJ640101HMCNLN01"));
        assert_eq!(emp.rfc.as_deref(), Some("PELJ640101A12"));
        assert_eq!(emp.clave_ubicacion.as_deref(), Some("01"));
    }

    #[test]
    fn test_parse_employee_rfc_curp_no_ubicacion_curp_never_in_clave_ubicacion() {
        let patterns = initialize_patterns().unwrap();
        // Caso exacto reportado: RFC seguido de CURP sin clave de ubicación
        let sample = "01-85-64-0012-3   JIMENEZ FABILA ALEJANDRO   JIFA981012A12   JIFA981012MCLMN04\n";
        let mut emp = Employee::new();
        emp.set_values(sample, &[], None, &patterns);

        assert_eq!(emp.no_seguro_social.as_deref(), Some("01856400123"));
        assert_eq!(emp.nombre.as_deref(), Some("JIMENEZ FABILA ALEJANDRO"));
        assert_eq!(emp.rfc.as_deref(), Some("JIFA981012A12"));
        assert_eq!(emp.curp.as_deref(), Some("JIFA981012MCLMN04"));
        assert_eq!(emp.clave_ubicacion, None, "La CURP nunca debe guardarse en clave_ubicacion");
    }

    #[test]
    fn test_parse_employee_curp_only() {
        let patterns = initialize_patterns().unwrap();
        let sample = "01-85-64-0012-3   JIMENEZ FABILA ALEJANDRO   JIFA981012MCLMN04\n";
        let mut emp = Employee::new();
        emp.set_values(sample, &[], None, &patterns);

        assert_eq!(emp.curp.as_deref(), Some("JIFA981012MCLMN04"));
        assert_eq!(emp.rfc, None);
        assert_eq!(emp.clave_ubicacion, None);
    }

    /// Regresión: el nombre completo es "JIMENEZ DE LA FUENTE MARIA ALEJANDRA".
    /// "ALEJANDRA" es parte del nombre y nunca debe interpretarse como RFC.
    #[test]
    fn test_nombre_completo_no_se_trunca_en_rfc() {
        let patterns = initialize_patterns().unwrap();
        let sample = "03-19-98-8841-7   JIMENEZ DE LA FUENTE MARIA ALEJANDRA   JIFA981012MCLMNL01\n";
        let mut emp = Employee::new();
        emp.set_values(sample, &[], None, &patterns);

        assert_eq!(emp.nombre.as_deref(), Some("JIMENEZ DE LA FUENTE MARIA ALEJANDRA"));
        assert_eq!(emp.curp.as_deref(), Some("JIFA981012MCLMNL01"));
        assert_eq!(emp.rfc, None, "Un fragmento del nombre no debe guardarse como RFC");
    }

    /// Regresión: sin CURP/RFC en la línea, el último token del nombre no debe
    /// terminar guardado como clave_ubicacion (que luego se usa como registro de obra).
    #[test]
    fn test_resto_del_nombre_no_se_guarda_como_clave_ubicacion() {
        let patterns = initialize_patterns().unwrap();
        let sample = "03-19-98-8841-7   JIMENEZ DE LA FUENTE MARIA ALEJANDRA\n";
        let mut emp = Employee::new();
        emp.set_values(sample, &[], None, &patterns);

        assert_eq!(emp.nombre.as_deref(), Some("JIMENEZ DE LA FUENTE MARIA ALEJANDRA"));
        assert_eq!(emp.clave_ubicacion, None);
    }

    /// Regresión: un RFC genérico del SAT es válido como dato y debe conservarse,
    /// pero no debe confundirse con un fragmento del nombre.
    #[test]
    fn test_rfc_generico_se_conserva_pero_no_se_confunde_con_nombre() {
        let patterns = initialize_patterns().unwrap();
        let sample = "02-17-93-2524-2   CRUZ LUNA ROBERTO   XAXX-010101-000\n";
        let mut emp = Employee::new();
        emp.set_values(sample, &[], None, &patterns);

        assert_eq!(emp.nombre.as_deref(), Some("CRUZ LUNA ROBERTO"));
        assert_eq!(emp.rfc.as_deref(), Some("XAXX010101000"));
    }

    #[test]
    fn test_parse_employee_pensionado_pcv() {
        let patterns = initialize_patterns().unwrap();
        // Caso exacto de la cédula de septiembre: FLORES RUIZ GUILLERMO
        let sample = "32-85-59-4018-8  P/CV  FLORES RUIZ GUILLERMO    FORG590625HCLLZL01\n";
        let mut emp = Employee::new();
        emp.set_values(sample, &[], None, &patterns);

        assert_eq!(emp.no_seguro_social.as_deref(), Some("32855940188"));
        assert_eq!(emp.nombre.as_deref(), Some("FLORES RUIZ GUILLERMO"));
        assert_eq!(emp.pensionado.as_deref(), Some("P/CV"));
        assert_eq!(emp.curp.as_deref(), Some("FORG590625HCLLZL01"));
    }

    #[test]
    fn test_parse_employee_pensionado_lazarin_and_marin() {
        let patterns = initialize_patterns().unwrap();
        // Casos adicionales de la cédula: LAZARIN ALONZO y MARIN LOPEZ
        let s1 = "32-80-64-1338-4  P/CV  LAZARIN ALONZO RAMON MANUEL EPIGMENIO    LAAR640111HCLZLM00\n";
        let mut emp1 = Employee::new();
        emp1.set_values(s1, &[], None, &patterns);
        assert_eq!(emp1.nombre.as_deref(), Some("LAZARIN ALONZO RAMON MANUEL EPIGMENIO"));
        assert_eq!(emp1.pensionado.as_deref(), Some("P/CV"));
        assert_eq!(emp1.curp.as_deref(), Some("LAAR640111HCLZLM00"));

        let s2 = "32-89-66-5891-4  P/CV  MARIN LOPEZ ANDRES    MALA660204HCLRPN09\n";
        let mut emp2 = Employee::new();
        emp2.set_values(s2, &[], None, &patterns);
        assert_eq!(emp2.nombre.as_deref(), Some("MARIN LOPEZ ANDRES"));
        assert_eq!(emp2.pensionado.as_deref(), Some("P/CV"));
        assert_eq!(emp2.curp.as_deref(), Some("MALA660204HCLRPN09"));
    }

    #[test]
    fn test_parse_employee_pensionado_piv() {
        let patterns = initialize_patterns().unwrap();
        let s = "32-11-22-3344-5  P/IV  LOPEZ HERNANDEZ CARLOS    LOHC800101HDF00\n";
        let mut emp = Employee::new();
        emp.set_values(s, &[], None, &patterns);
        assert_eq!(emp.nombre.as_deref(), Some("LOPEZ HERNANDEZ CARLOS"));
        assert_eq!(emp.pensionado.as_deref(), Some("P/IV"));
    }

    #[test]
    fn test_parse_multiple_obras_in_header() {
        let patterns = initialize_patterns().unwrap();
        // Caso exacto de la imagen: BERMUDEZ RAMIREZ MIGUEL ANGEL con 3 obras vinculadas
        let s = "43-94-79-0924-8  BERMUDEZ RAMIREZ MIGUEL ANGEL  C0617880-15 C0640941-7 C0640943-9\n";
        let mut emp = Employee::new();
        emp.set_values(s, &[], None, &patterns);

        assert_eq!(emp.no_seguro_social.as_deref(), Some("43947909248"));
        assert_eq!(emp.nombre.as_deref(), Some("BERMUDEZ RAMIREZ MIGUEL ANGEL"));
        assert_eq!(emp.registro_obras.len(), 3);
        assert_eq!(emp.registro_obras[0].registro_obra, "C0617880");
        assert_eq!(emp.registro_obras[0].dias_registro_obra, 15);
        assert_eq!(emp.registro_obras[1].registro_obra, "C0640941");
        assert_eq!(emp.registro_obras[1].dias_registro_obra, 7);
        assert_eq!(emp.registro_obras[2].registro_obra, "C0640943");
        assert_eq!(emp.registro_obras[2].dias_registro_obra, 9);

        let rows = emp.to_list();
        assert!(!rows.is_empty() || emp.registro_obras.len() == 3);
    }

    #[test]
    fn test_parse_multiple_obras_folio_registro_de_obra() {
        let patterns = initialize_patterns().unwrap();
        // Caso exacto de la imagen del usuario:
        // HERAS BAUTISTA OMAR ABACUC con 2 registros de obra: C0701031 (14 días) y C0701034 (17 días)
        let s = "43-06-79-1194-1  HERAS BAUTISTA OMAR ABACUC  AAAA-900101-\n\
                 31 205.00 0 0 0 608.50 0.00 0.00 44.49 15.89 66.73 23.83 482.27 111.21 39.72 63.55 1,376.75 79.44 1,456.19\n\
                 Folio de Registro de Obra: C0701031 DĂ-as 14\n\
                 Folio de Registro de Obra: C0701034 DĂ-as 17\n";
        let cleaned = crate::cedulas::pdf_extractor::clean_text(s, &patterns);
        let mut emp = Employee::new();
        emp.set_values(&cleaned, &[], None, &patterns);

        assert_eq!(emp.no_seguro_social.as_deref(), Some("43067911941"));
        assert_eq!(emp.nombre.as_deref(), Some("HERAS BAUTISTA OMAR ABACUC"));
        assert_eq!(emp.registro_obras.len(), 2);
        assert_eq!(emp.registro_obras[0].registro_obra, "C0701031");
        assert_eq!(emp.registro_obras[0].dias_registro_obra, 14);
        assert_eq!(emp.registro_obras[1].registro_obra, "C0701034");
        assert_eq!(emp.registro_obras[1].dias_registro_obra, 17);
    }
}
