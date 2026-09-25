use chrono::NaiveDate;
use log::{debug, warn};
use crate::nominas::utils::{get_vacation_days, years_between};

#[derive(Debug, Default, Clone)]
pub struct Percepcion {
    pub concepto: String,
    pub importe: f64,
}

#[derive(Debug, Default, Clone)]
pub struct Nomina {
    pub fechainicialpago: Option<NaiveDate>,
    pub fechafinalpago: Option<NaiveDate>,
    pub fechapago: Option<NaiveDate>,
    pub numdiaspagados: Option<f64>,
    pub percepciones: Vec<Percepcion>,
}

#[derive(Debug, Default, Clone)]
pub struct Employee {
    pub nombrearchivo: String,
    pub nombre: Option<String>,
    pub numseguridadsocial: Option<String>,
    pub fechainiciorellaboral: Option<NaiveDate>,
    pub salariobasecotapor: Option<f64>,
    pub salariodiariocalculado: Option<f64>,
    pub salariodiariointegradocalculado: Option<f64>,
    pub nomina: Nomina,

    // Fiscal CFDI attributes for SAT validation
    pub uuid: Option<String>,
    pub rfc_emisor: Option<String>,
    pub nombre_emisor: Option<String>,
    pub rfc_receptor: Option<String>,
    pub total: Option<f64>,
}

impl Employee {
    pub fn calculate_salaries(&mut self) {
        let dias_pagados = self.nomina.numdiaspagados.unwrap_or(0.0);
        if dias_pagados <= 0.0 {
            warn!(
                "Employee {} has 0 or missing dias pagados, skipping salary calculation.",
                self.nombre.as_deref().unwrap_or("UNKNOWN")
            );
            return;
        }

        let mut dias_trabajados = dias_pagados;
        let has_septimo = self
            .nomina
            .percepciones
            .iter()
            .any(|p| p.concepto == "SEPTIMO_DIA");

        if let Some(sueldo_per) = self
            .nomina
            .percepciones
            .iter()
            .find(|p| p.concepto.contains("SUELDO"))
        {
            if has_septimo {
                let sunday_per_week = (dias_pagados / 7.0).floor();
                dias_trabajados = dias_pagados - sunday_per_week;
            }

            let sd_calculado = (sueldo_per.importe / dias_trabajados * 100.0).round() / 100.0;
            self.salariodiariocalculado = Some(sd_calculado);

            if let (Some(inicio), Some(fin)) = (self.fechainiciorellaboral, self.nomina.fechafinalpago) {
                let antiguedad = years_between(inicio, fin);
                let vacation_days = get_vacation_days(antiguedad);

                let aguinaldo = 15.0 / 365.0;
                let prima_vacacional = (vacation_days * 0.25) / 365.0;
                let factor_integracion = aguinaldo + prima_vacacional + 1.0;

                let sdi = (sd_calculado * factor_integracion * 100.0).round() / 100.0;
                self.salariodiariointegradocalculado = Some(sdi);

                debug!(
                    "Calculated salaries for {}: SD={}, SDI={}",
                    self.nombre.as_deref().unwrap_or("UNKNOWN"),
                    sd_calculado,
                    sdi
                );
            } else {
                warn!(
                    "Missing start/end dates for {}, can't calculate SDI",
                    self.nombre.as_deref().unwrap_or("UNKNOWN")
                );
            }
        }
    }
}
