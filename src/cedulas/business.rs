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
        merge_field("subdelegacion", &mut self.subdelegacion, &other.subdelegacion, file_name);
        merge_field("codigo_subdelegacion", &mut self.codigo_subdelegacion, &other.codigo_subdelegacion, file_name);
        merge_field("domicilio", &mut self.domicilio, &other.domicilio, file_name);
        merge_field("codigo_postal", &mut self.codigo_postal, &other.codigo_postal, file_name);
        merge_field("entidad", &mut self.entidad, &other.entidad, file_name);
        merge_field("rfc", &mut self.rfc, &other.rfc, file_name);
        merge_field("curp", &mut self.curp, &other.curp, file_name);
        merge_field("area_geografica", &mut self.area_geografica, &other.area_geografica, file_name);
        merge_field("prima", &mut self.prima, &other.prima, file_name);
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
                    self.registro_patronal = Some(s);
                }
            }
        }
        if self.rfc.is_none() {
            if let Ok(Some(caps)) = patterns.business_rfc.captures(page) {
                if let Some(m) = caps.name("rfc") {
                    self.rfc = Some(m.as_str().trim().to_uppercase());
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
                if let Some(m) = caps.name("delegacion") {
                    self.delegacion = Some(m.as_str().trim().to_uppercase());
                }
                if let Some(m) = caps.name("codigo") {
                    self.codigo_delegacion = Some(m.as_str().trim().to_uppercase());
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
                if let Some(m) = caps.name("subdelegacion") {
                    self.subdelegacion = Some(m.as_str().trim().to_uppercase());
                }
                if let Some(m) = caps.name("codigo_subdelegacion") {
                    self.codigo_subdelegacion = Some(m.as_str().trim().to_uppercase());
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
