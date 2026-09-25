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

#[derive(Debug, Clone)]
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

impl Employee {
    pub fn new() -> Self {
        Self::default()
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
            if let Some(m) = caps.name("no_seguro_social") {
                self.no_seguro_social = Some(m.as_str().trim().to_string());
            }
            if let Some(m) = caps.name("nombre") {
                self.nombre = Some(m.as_str().trim().to_string());
            }
            if let Some(m) = caps.name("rfc") {
                self.rfc = Some(m.as_str().trim().to_string());
            }
            if let Some(m) = caps.name("curp") {
                self.curp = Some(m.as_str().trim().to_string());
            }
            if let Some(m) = caps.name("clave_ubicacion") {
                self.clave_ubicacion = Some(m.as_str().trim().to_string());
            }
        }

        let get_base_de = || {
            let mut de = DataEmployee::default();
            if let Some(dp) = date_period {
                if let Some(mes) = dp.name("mes_periodo") {
                    de.mes_periodo = Some(mes.as_str().trim().to_string());
                }
                if let Some(anio) = dp.name("anio_periodo") {
                    de.anio_periodo = Some(anio.as_str().trim().to_string());
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
            self.registro_obras.push(RegistroObra::new(obra, dias));
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
