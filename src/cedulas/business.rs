use rayon::prelude::*;
use crate::cedulas::employees::Employee;

fn merge_field(
    field_name: &str,
    master: &mut Option<String>,
    new_val: &Option<String>,
    file_name: &str,
) {
    if let Some(new_v) = new_val {
        if let Some(master_v) = master {
            if master_v != new_v {
                log::warn!(
                    "CONFLICTO EN CAMPO: {} | ANTERIOR: '{}' | NUEVO: '{}' | ARCHIVO: {}",
                    field_name.to_uppercase(),
                    master_v,
                    new_v,
                    file_name
                );
            }
        } else {
            *master = Some(new_v.clone());
        }
    }
}

#[repr(i16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MesPeriodo {
    Enero = 1,
    Febrero = 2,
    Marzo = 3,
    Abril = 4,
    Mayo = 5,
    Junio = 6,
    Julio = 7,
    Agosto = 8,
    Septiembre = 9,
    Octubre = 10,
    Noviembre = 11,
    Diciembre = 12,
}

impl MesPeriodo {
    pub fn from_month_str(month: &str) -> Option<Self> {
        match month_to_num(month) {
            1 => Some(Self::Enero),
            2 => Some(Self::Febrero),
            3 => Some(Self::Marzo),
            4 => Some(Self::Abril),
            5 => Some(Self::Mayo),
            6 => Some(Self::Junio),
            7 => Some(Self::Julio),
            8 => Some(Self::Agosto),
            9 => Some(Self::Septiembre),
            10 => Some(Self::Octubre),
            11 => Some(Self::Noviembre),
            12 => Some(Self::Diciembre),
            _ => None,
        }
    }

    pub fn as_i16(self) -> i16 {
        self as i16
    }
}

pub fn mes_to_periodo_int(month: &str) -> i16 {
    MesPeriodo::from_month_str(month)
        .map(MesPeriodo::as_i16)
        .unwrap_or(0)
}

pub fn month_to_num(month: &str) -> u32 {
    let trimmed = month.trim();
    if let Ok(n) = trimmed.parse::<u32>() {
        if (1..=12).contains(&n) {
            return n;
        }
    }
    match trimmed.to_lowercase().as_str() {
        "enero" | "january" | "jan" => 1,
        "febrero" | "february" | "feb" => 2,
        "marzo" | "march" | "mar" => 3,
        "abril" | "april" | "abr" => 4,
        "mayo" | "may" => 5,
        "junio" | "june" | "jun" => 6,
        "julio" | "july" | "jul" => 7,
        "agosto" | "august" | "ago" => 8,
        "septiembre" | "setiembre" | "september" | "sep" => 9,
        "octubre" | "october" | "oct" => 10,
        "noviembre" | "november" | "nov" => 11,
        "diciembre" | "december" | "dic" => 12,
        _ => 0,
    }
}

pub fn month_num_to_name(month: u32) -> &'static str {
    match month {
        1 => "ENERO",
        2 => "FEBRERO",
        3 => "MARZO",
        4 => "ABRIL",
        5 => "MAYO",
        6 => "JUNIO",
        7 => "JULIO",
        8 => "AGOSTO",
        9 => "SEPTIEMBRE",
        10 => "OCTUBRE",
        11 => "NOVIEMBRE",
        12 => "DICIEMBRE",
        _ => "SIN_MES",
    }
}

pub fn clean_entidad(s: &str) -> String {
    let mut text = s.trim().to_uppercase();
    let markers = [
        " PRIMA",
        " V ",
        " V3.",
        " V 3.",
        " VERSI",
        " POB.",
        " POB ",
        " MUN.",
        " ALCALD",
        " RFC",
    ];
    for marker in &markers {
        if let Some(idx) = text.find(marker) {
            text = text[..idx].trim().to_string();
        }
    }

    const ESTADOS: &[&str] = &[
        "AGUASCALIENTES", "BAJA CALIFORNIA SUR", "BAJA CALIFORNIA", "CAMPECHE", "CHIAPAS",
        "CHIHUAHUA", "CIUDAD DE MEXICO", "DISTRITO FEDERAL", "CDMX", "COAHUILA DE ZARAGOZA", "COAHUILA",
        "COLIMA", "DURANGO", "ESTADO DE MEXICO", "MEXICO", "GUANAJUATO", "GUERRERO",
        "HIDALGO", "JALISCO", "MICHOACAN DE OCAMPO", "MICHOACAN", "MORELOS", "NAYARIT",
        "NUEVO LEON", "OAXACA", "PUEBLA", "QUERETARO", "QUINTANA ROO", "SAN LUIS POTOSI",
        "SINALOA", "SONORA", "TABASCO", "TAMAULIPAS", "TLAXCALA", "VERACRUZ DE IGNACIO DE LA LLAVE",
        "VERACRUZ", "YUCATAN", "ZACATECAS",
    ];

    let norm_text = text
        .replace(['Á', 'À', 'Ä'], "A")
        .replace(['É', 'È', 'Ë'], "E")
        .replace(['Í', 'Ì', 'Ï'], "I")
        .replace(['Ó', 'Ò', 'Ö'], "O")
        .replace(['Ú', 'Ù', 'Ü'], "U");

    for &est in ESTADOS {
        let est_norm = est
            .replace(['Á', 'À', 'Ä'], "A")
            .replace(['É', 'È', 'Ë'], "E")
            .replace(['Í', 'Ì', 'Ï'], "I")
            .replace(['Ó', 'Ò', 'Ö'], "O")
            .replace(['Ú', 'Ù', 'Ü'], "U");

        if norm_text == est_norm
            || norm_text.starts_with(&format!("{} ", est_norm))
            || norm_text.starts_with(&format!("{}/", est_norm))
            || norm_text.starts_with(&format!("{}-", est_norm))
        {
            return est.to_string();
        }
    }

    text.trim_end_matches([',', '/', '-', ':', ';', '.']).trim().to_string()
}

pub fn clean_domicilio(s: &str) -> String {
    let mut text = s.trim().to_string();
    let markers = [
        "POB. MUN.",
        "POB.MUN.",
        "POB. MUN",
        "POB MUN",
        "POBLACION / MUNICIPIO",
        "POBLACIÓN / MUNICIPIO",
        "POBLACION/MUNICIPIO",
        "POBLACION",
        "POBLACIÓN",
        "ALCALDÍA:",
        "ALCALDIA:",
        "/ ALCALDÍA",
        "/ ALCALDIA",
        "ALCALDÍA",
        "ALCALDIA",
        "MUNICIPIO:",
        "CÓDIGO POSTAL",
        "CODIGO POSTAL",
        "C.P.:",
        "ENTIDAD:",
        "PRIMA DE R.T.",
    ];
    let upper = text.to_uppercase();
    let mut cut_idx = None;
    for &marker in &markers {
        if let Some(idx) = upper.find(marker) {
            cut_idx = Some(match cut_idx {
                Some(curr) => std::cmp::min(curr, idx),
                None => idx,
            });
        }
    }
    if let Some(idx) = cut_idx {
        text = text[..idx].trim().to_string();
    }
    text.trim_end_matches([',', '/', '-', ':', ';', '.']).trim().to_uppercase()
}

pub fn clean_actividad(s: &str) -> String {
    let mut text = s.trim().to_string();
    let markers = [
        "SUBDELEGACIÓN",
        "SUBDELEGACION",
        "DELEGACIÓN",
        "DELEGACION",
        "REGISTRO PATRONAL",
        "DOMICILIO",
        "CÓDIGO POSTAL",
        "CODIGO POSTAL",
    ];
    let upper = text.to_uppercase();
    let mut cut_idx = None;
    for &marker in &markers {
        if let Some(idx) = upper.find(marker) {
            cut_idx = Some(match cut_idx {
                Some(curr) => std::cmp::min(curr, idx),
                None => idx,
            });
        }
    }
    if let Some(idx) = cut_idx {
        text = text[..idx].trim().to_string();
    }
    text.trim_end_matches([',', '/', '-', ':', ';', '.']).trim().to_uppercase()
}

#[derive(Default, Clone)]
pub struct Business {
    pub registro_patronal: Option<String>,
    pub razon_social: Option<String>,
    pub delegacion: Option<String>,
    pub codigo_delegacion: Option<String>,
    pub actividad: Option<String>,
    pub subdelegacion: Option<String>,
    pub codigo_subdelegacion: Option<String>,
    pub domicilio: Option<String>,
    pub codigo_postal: Option<String>,
    pub entidad: Option<String>,
    pub rfc: Option<String>,
    pub curp: Option<String>,
    pub area_geografica: Option<String>,
    pub prima: Option<String>,
    pub employees: Vec<Employee>,
}

impl Business {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn merge_with(&mut self, other: &Business, file_name: &str) {
        merge_field("registro_patronal", &mut self.registro_patronal, &other.registro_patronal, file_name);
        merge_field("razon_social", &mut self.razon_social, &other.razon_social, file_name);
        merge_field("delegacion", &mut self.delegacion, &other.delegacion, file_name);
        merge_field("codigo_delegacion", &mut self.codigo_delegacion, &other.codigo_delegacion, file_name);
        merge_field("actividad", &mut self.actividad, &other.actividad, file_name);

        // Protección contra contaminación Delegación vs Subdelegación
        let mut sub_to_merge = other.subdelegacion.clone();
        let mut cod_sub_to_merge = other.codigo_subdelegacion.clone();
        if let Some(ref my_del) = self.delegacion {
            if sub_to_merge.as_deref() == Some(my_del.as_str()) {
                sub_to_merge = None;
            }
        }
        if let Some(ref my_cod_del) = self.codigo_delegacion {
            if cod_sub_to_merge.as_deref() == Some(my_cod_del.as_str()) {
                cod_sub_to_merge = None;
            }
        }

        merge_field("subdelegacion", &mut self.subdelegacion, &sub_to_merge, file_name);
        merge_field("codigo_subdelegacion", &mut self.codigo_subdelegacion, &cod_sub_to_merge, file_name);
        merge_field("domicilio", &mut self.domicilio, &other.domicilio, file_name);
        merge_field("codigo_postal", &mut self.codigo_postal, &other.codigo_postal, file_name);
        merge_field("entidad", &mut self.entidad, &other.entidad, file_name);
        merge_field("rfc", &mut self.rfc, &other.rfc, file_name);
        merge_field("curp", &mut self.curp, &other.curp, file_name);
        // Manejo especializado para la prima de riesgo (PRIMA):
        // La prima de riesgo del IMSS varía por año/periodo o puede venir en 0 en cédulas complementarias/diferencias.
        // Si detecta una variación, toma el valor válido (> 0) o mantiene el histórico de manera silenciosa sin warning.
        if let Some(ref new_prima_raw) = other.prima {
            let new_is_zero = new_prima_raw.trim().parse::<f64>().map(|v| v.abs() < 1e-6).unwrap_or(false);
            if let Some(ref cur_prima_raw) = self.prima {
                let cur_is_zero = cur_prima_raw.trim().parse::<f64>().map(|v| v.abs() < 1e-6).unwrap_or(false);
                if cur_is_zero && !new_is_zero {
                    self.prima = Some(new_prima_raw.clone());
                } else if !cur_is_zero && new_is_zero {
                    // Mantener el valor histórico válido silenciosamente
                } else if cur_prima_raw != new_prima_raw {
                    log::debug!(
                        "Variación de prima de riesgo detectada: anterior '{}', nueva '{}' (archivo: {})",
                        cur_prima_raw,
                        new_prima_raw,
                        file_name
                    );
                    self.prima = Some(new_prima_raw.clone());
                }
            } else {
                self.prima = Some(new_prima_raw.clone());
            }
        }
    }

    pub fn verify_data_full(&self) -> bool {
        self.registro_patronal.is_some()
            && self.razon_social.is_some()
            && self.actividad.is_some()
            && self.domicilio.is_some()
            && self.codigo_postal.is_some()
            && self.rfc.is_some()
            && self.entidad.is_some()
            && self.area_geografica.is_some()
            && self.delegacion.is_some()
            && self.subdelegacion.is_some()
            && self.prima.is_some()
            && self.codigo_delegacion.is_some()
            && self.codigo_subdelegacion.is_some()
    }

    pub fn extract_from_page(
        &mut self,
        page: &str,
        patterns: &crate::cedulas::regex_expressions::OCRPatterns,
    ) {
        if self.registro_patronal.is_none() {
            if let Ok(Some(caps)) = patterns.business_registro_patronal.captures(page) {
                if let Some(m) = caps.name("registro_patronal") {
                    let mut s = m.as_str().trim().to_uppercase();
                    if let Some(idx) = s.find(" RFC") {
                        s = s[..idx].trim().to_string();
                    }
                    if s.ends_with("RFC") {
                        s = s.trim_end_matches("RFC").trim().to_string();
                    }
                    let clean_rp: String = s
                        .chars()
                        .filter(|c| c.is_alphanumeric())
                        .collect::<String>()
                        .to_uppercase();
                    if !clean_rp.is_empty() {
                        self.registro_patronal = Some(clean_rp);
                    }
                }
            }
        }
        if self.rfc.is_none() {
            if let Ok(Some(caps)) = patterns.business_rfc.captures(page) {
                if let Some(m) = caps.name("rfc") {
                    let clean_rfc: String = m
                        .as_str()
                        .chars()
                        .filter(|c| c.is_alphanumeric())
                        .collect::<String>()
                        .to_uppercase();
                    if !clean_rfc.is_empty() {
                        self.rfc = Some(clean_rfc);
                    }
                }
            }
        }
        if self.curp.is_none() {
            if let Ok(Some(caps)) = patterns.business_curp.captures(page) {
                if let Some(m) = caps.name("curp") {
                    let c: String = m
                        .as_str()
                        .chars()
                        .filter(|ch| ch.is_alphanumeric())
                        .collect::<String>()
                        .to_uppercase();
                    if c.len() == 18 {
                        self.curp = Some(c);
                    }
                }
            }
        }
        if self.area_geografica.is_none() {
            if let Ok(Some(caps)) = patterns.business_area_geografica.captures(page) {
                if let Some(m) = caps.name("area") {
                    self.area_geografica = Some(m.as_str().trim().to_uppercase());
                }
            }
        }
        if self.razon_social.is_none() {
            if let Ok(Some(caps)) = patterns.business_razon_social.captures(page) {
                if let Some(m) = caps.name("razon_social") {
                    let mut rz = m.as_str().trim().to_uppercase();
                    if let Some(idx) = rz.find(" DELEGACIÓN") {
                        rz = rz[..idx].trim().to_string();
                    }
                    if let Some(idx) = rz.find(" DELEGACION") {
                        rz = rz[..idx].trim().to_string();
                    }
                    if let Some(idx) = rz.find(" RFC") {
                        rz = rz[..idx].trim().to_string();
                    }
                    self.razon_social = Some(rz);
                }
            }
        }
        if self.delegacion.is_none() || self.codigo_delegacion.is_none() {
            if let Ok(Some(caps)) = patterns.business_delegacion.captures(page) {
                if self.delegacion.is_none() {
                    if let Some(m) = caps.name("delegacion").or_else(|| caps.name("delegacion_alt")) {
                        let mut del = m.as_str().trim().to_uppercase();
                        if let Some(idx) = del.find("SUBDELEG") {
                            del = del[..idx].trim().to_string();
                        }
                        if !del.is_empty() {
                            self.delegacion = Some(del);
                        }
                    }
                }
                if self.codigo_delegacion.is_none() {
                    if let Some(m) = caps.name("codigo").or_else(|| caps.name("codigo_alt")) {
                        let cod = m.as_str().trim().to_uppercase();
                        if !cod.is_empty() {
                            self.codigo_delegacion = Some(cod);
                        }
                    }
                }
            }
        }
        if self.actividad.is_none() {
            if let Ok(Some(caps)) = patterns.business_actividad.captures(page) {
                if let Some(m) = caps.name("actividad") {
                    let cleaned = clean_actividad(m.as_str());
                    if !cleaned.is_empty() {
                        self.actividad = Some(cleaned);
                    }
                }
            }
        }
        if self.subdelegacion.is_none() || self.codigo_subdelegacion.is_none() {
            if let Ok(Some(caps)) = patterns.business_subdelegacion.captures(page) {
                if self.subdelegacion.is_none() {
                    if let Some(m) = caps.name("subdelegacion").or_else(|| caps.name("subdelegacion_alt")) {
                        let mut sub = m.as_str().trim().to_uppercase();
                        if let Some(idx) = sub.find("DELEG") {
                            sub = sub[..idx].trim().to_string();
                        }
                        if !sub.is_empty() {
                            let is_same_as_del = self.delegacion.as_deref().map(|d| d == sub).unwrap_or(false);
                            if !is_same_as_del {
                                self.subdelegacion = Some(sub);
                            }
                        }
                    }
                }
                if self.codigo_subdelegacion.is_none() {
                    if let Some(m) = caps.name("codigo_subdelegacion").or_else(|| caps.name("codigo_subdelegacion_alt")) {
                        let cod = m.as_str().trim().to_uppercase();
                        if !cod.is_empty() {
                            let is_same_as_del_cod = self.codigo_delegacion.as_deref().map(|c| c == cod).unwrap_or(false);
                            if !is_same_as_del_cod || self.codigo_delegacion.is_none() {
                                self.codigo_subdelegacion = Some(cod);
                            }
                        }
                    }
                }
            }
        }
        if self.domicilio.is_none() {
            if let Ok(Some(caps)) = patterns.business_domicilio.captures(page) {
                if let Some(m) = caps.name("domicilio") {
                    let cleaned = clean_domicilio(m.as_str());
                    if !cleaned.is_empty() {
                        self.domicilio = Some(cleaned);
                    }
                }
            }
        }
        if self.codigo_postal.is_none() {
            if let Ok(Some(caps)) = patterns.business_codigo_postal.captures(page) {
                if let Some(m) = caps.name("cp") {
                    self.codigo_postal = Some(m.as_str().trim().to_uppercase());
                }
            }
        }
        if self.entidad.is_none() {
            if let Ok(Some(caps)) = patterns.business_entidad.captures(page) {
                if let Some(m) = caps.name("entidad") {
                    let cleaned = clean_entidad(m.as_str());
                    if !cleaned.is_empty() {
                        self.entidad = Some(cleaned);
                    }
                }
            }
        }
        if self.prima.is_none() {
            if let Ok(Some(caps)) = patterns.business_prima.captures(page) {
                if let Some(m) = caps.name("prima") {
                    self.prima = Some(m.as_str().trim().to_uppercase());
                }
            }
        }
    }

    pub fn sort_employee_data(&mut self) {
        self.employees.par_sort_by(|a, b| {
            let year_a: u32 = a.anio_periodo.as_deref().unwrap_or("").parse().unwrap_or(0);
            let year_b: u32 = b.anio_periodo.as_deref().unwrap_or("").parse().unwrap_or(0);

            let year_cmp = year_a.cmp(&year_b);
            if year_cmp != std::cmp::Ordering::Equal {
                return year_cmp;
            }

            let month_a = month_to_num(a.mes_periodo.as_deref().unwrap_or(""));
            let month_b = month_to_num(b.mes_periodo.as_deref().unwrap_or(""));

            let month_cmp = month_a.cmp(&month_b);
            if month_cmp != std::cmp::Ordering::Equal {
                return month_cmp;
            }

            a.nombre.cmp(&b.nombre)
        });
    }
}
