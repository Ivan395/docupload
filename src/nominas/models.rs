use chrono::NaiveDate;
use log::{debug, warn};
use crate::nominas::utils::{get_vacation_days, years_between};

#[derive(Debug, Default, Clone)]
pub struct Percepcion {
    pub concepto: String,
    pub importe: f64,
}

#[derive(Debug, Default, Clone)]
pub struct ConceptoCfdiItem {
    pub clave_prod_serv: Option<String>,
    pub clave_unidad: Option<String>,
    pub descripcion: Option<String>,
    pub cantidad: Option<i16>,
    pub valor_unitario: Option<f64>,
    pub importe: Option<f64>,
    pub descuento: Option<f64>,
    pub objeto_imp: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct DetalleNominaItem {
    pub tipo_concepto_categoria: String, // "PERCEPCIONES", "DEDUCCIONES", "OTROS PAGOS"
    pub tipo_sat: String, // e.g. "001"
    pub clave: String, // Clave interna de nómina
    pub concepto: String, // Nombre del concepto
    pub importe_gravado: Option<f64>,
    pub importe_exento: Option<f64>,
    pub importe: f64,
}

#[derive(Debug, Default, Clone)]
pub struct CfdiData {
    pub version_cfdi: Option<String>,
    pub fecha_emision: Option<chrono::DateTime<chrono::Utc>>,
    pub numero_certificado: Option<String>,
    pub moneda: Option<String>,
    pub tipo_comprobante: Option<String>,
    pub exportacion: Option<String>,
    pub metodo_pago: Option<String>,
    pub serie: Option<String>,
    pub folio: Option<String>,
    pub lugar_expedicion: Option<String>,
    pub subtotal: Option<f64>,
    pub descuento: Option<f64>,
    pub total: Option<f64>,
    pub conceptos: Vec<ConceptoCfdiItem>,
}

#[derive(Debug, Default, Clone)]
pub struct ContratoData {
    pub domicilio_fiscal: Option<String>,
    pub regimen_fiscal: Option<String>,
    pub uso_cfdi: Option<String>,
    pub fecha_inicio_relacion_laboral: Option<NaiveDate>,
    pub antiguedad: Option<i16>,
    pub cod_antiguedad: Option<String>,
    pub tipo_contrato: Option<String>,
    pub es_sindicalizado: Option<bool>,
    pub tipo_jornada: Option<String>,
    pub tipo_regimen: Option<String>,
    pub num_empleado: Option<String>,
    pub departamento: Option<String>,
    pub puesto: Option<String>,
    pub riesgo_puesto: Option<String>,
    pub periodicidad_pago: Option<String>,
    pub clave_entidad_federativa: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct TimbreFiscalData {
    pub version: Option<String>,
    pub uuid: Option<String>,
    pub fecha_timbrado: Option<chrono::DateTime<chrono::Utc>>,
    pub rfc_prov_certif: Option<String>,
    pub no_certificado_sat: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Nomina {
    pub version_nomina: Option<String>,
    pub tipo_nomina: Option<String>,
    pub fechainicialpago: Option<NaiveDate>,
    pub fechafinalpago: Option<NaiveDate>,
    pub fechapago: Option<NaiveDate>,
    pub numdiaspagados: Option<f64>,
    pub total_sueldos: Option<f64>,
    pub total_gravado: Option<f64>,
    pub total_exento: Option<f64>,
    pub total_otras_deducciones: Option<f64>,
    pub total_impuestos_retenidos: Option<f64>,
    pub total_percepciones: Option<f64>,
    pub total_deducciones: Option<f64>,
    pub total_otros_pagos: Option<f64>,
    pub periodicidad_pago: Option<String>,
    pub percepciones: Vec<Percepcion>,
}

#[derive(Debug, Default, Clone)]
pub struct Employee {
    pub nombrearchivo: String,
    pub nombre: Option<String>,
    pub numseguridadsocial: Option<String>,
    pub fechainiciorellaboral: Option<NaiveDate>,
    pub salariobasecotapor: Option<f64>,
    pub salariodiariointegrado: Option<f64>,
    pub salariodiariocalculado: Option<f64>,
    pub salariodiariointegradocalculado: Option<f64>,
    pub nomina: Nomina,

    // Fiscal CFDI attributes for SAT validation and Database saving
    pub uuid: Option<String>,
    pub rfc_emisor: Option<String>,
    pub nombre_emisor: Option<String>,
    pub regimen_fiscal_emisor: Option<String>,
    pub rfc_receptor: Option<String>,
    pub curp_receptor: Option<String>,
    pub total: Option<f64>,

    // Full CFDI & Nómina 1.2 components
    pub cfdi: CfdiData,
    pub contrato: ContratoData,
    pub timbre: TimbreFiscalData,
    pub registro_patronal: Option<String>,
    pub curp_emisor: Option<String>,
    pub detalles_nomina: Vec<DetalleNominaItem>,
}

impl Employee {
    pub fn get_sueldo_importe(&self) -> f64 {
        self.nomina
            .percepciones
            .iter()
            .filter(|p| p.concepto.contains("SUELDO"))
            .map(|p| p.importe)
            .sum()
    }

    pub fn get_septimo_dia_importe(&self) -> f64 {
        self.nomina
            .percepciones
            .iter()
            .filter(|p| {
                p.concepto.contains("SEPTIMO")
                    || p.concepto.contains("7MO")
                    || p.concepto == "SEPTIMO_DIA"
            })
            .map(|p| p.importe)
            .sum()
    }

    pub fn get_sueldo_total_con_septimo(&self) -> f64 {
        self.get_sueldo_importe() + self.get_septimo_dia_importe()
    }

    pub fn periodicidad_display(&self) -> String {
        crate::nominas::utils::normalize_periodicidad(
            self.nomina.periodicidad_pago.as_deref(),
            self.nomina.numdiaspagados,
        )
    }

    pub fn get_year_month(&self) -> (i32, u32) {
        crate::nominas::utils::extract_year_month(
            self.nomina.fechafinalpago,
            self.nomina.fechapago,
            self.nomina.fechainicialpago,
        )
    }

    pub fn get_month_name(&self) -> &'static str {
        crate::nominas::utils::month_name_spanish(self.get_year_month().1)
    }

    pub fn unique_key(&self) -> &str {
        if let Some(rfc) = self.rfc_receptor.as_deref() {
            let trimmed = rfc.trim();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        if let Some(nss) = self.numseguridadsocial.as_deref() {
            let trimmed = nss.trim();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        if let Some(nom) = self.nombre.as_deref() {
            let trimmed = nom.trim();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        &self.nombrearchivo
    }

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
        let has_septimo = self.get_septimo_dia_importe() > 0.0;

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
