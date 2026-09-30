use anyhow::{Context, Result};
use chrono::NaiveDate;
use roxmltree::Document;
use std::fs;
use std::path::Path;

use crate::nominas::models::{Employee, Percepcion};
use crate::nominas::utils::clean_concepto;

pub fn parse_xml_file(path: &Path) -> Result<Employee> {
    let mut parts_iter = path.components().rev();
    let file = parts_iter
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = parts_iter
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();

    let file_name = if !dir.is_empty() {
        format!("{}/{}", dir, file)
    } else {
        file.clone()
    };

    let text = fs::read_to_string(path)
        .with_context(|| format!("No se pudo leer el archivo XML: {}", file_name))?;

    let cleaned_text = text
        .replace("cfdi:", "")
        .replace("nomina12:", "")
        .replace("nomina:", "")
        .replace("tfd:", "")
        .to_lowercase();

    let doc = Document::parse(&cleaned_text)
        .with_context(|| format!("El archivo no tiene una estructura XML válida: {}", file_name))?;

    let mut employee = Employee {
        nombrearchivo: file_name.clone(),
        ..Default::default()
    };

    // Helper for parsing timestamptz
    let parse_timestamptz = |s: &str| -> Option<chrono::DateTime<chrono::Utc>> {
        let trimmed = s.trim();
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(trimmed) {
            return Some(dt.with_timezone(&chrono::Utc));
        }
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S") {
            return Some(chrono::DateTime::from_naive_utc_and_offset(ndt, chrono::Utc));
        }
        None
    };

    // 1. Root comprobante attributes
    for node in doc.descendants().filter(|n| n.tag_name().name() == "comprobante") {
        if let Some(val) = node.attribute("version") {
            employee.cfdi.version_cfdi = Some(val.trim().to_string());
        }
        if let Some(val) = node.attribute("fecha") {
            employee.cfdi.fecha_emision = parse_timestamptz(val);
        }
        if let Some(val) = node.attribute("nocertificado") {
            employee.cfdi.numero_certificado = Some(val.trim().to_string());
        }
        if let Some(val) = node.attribute("moneda") {
            employee.cfdi.moneda = Some(val.trim().to_uppercase());
        }
        if let Some(val) = node.attribute("tipodecomprobante") {
            employee.cfdi.tipo_comprobante = Some(val.trim().to_uppercase());
        }
        if let Some(val) = node.attribute("exportacion") {
            employee.cfdi.exportacion = Some(val.trim().to_string());
        }
        if let Some(val) = node.attribute("metodopago") {
            employee.cfdi.metodo_pago = Some(val.trim().to_uppercase());
        }
        if let Some(val) = node.attribute("serie") {
            employee.cfdi.serie = Some(val.trim().to_string());
        }
        if let Some(val) = node.attribute("folio") {
            employee.cfdi.folio = Some(val.trim().to_string());
        }
        if let Some(val) = node.attribute("lugarexpedicion") {
            employee.cfdi.lugar_expedicion = Some(val.trim().to_string());
        }
        if let Some(val) = node.attribute("subtotal") {
            employee.cfdi.subtotal = val.parse().ok();
        }
        if let Some(val) = node.attribute("descuento") {
            employee.cfdi.descuento = val.parse().ok();
        }
        if let Some(val) = node.attribute("total") {
            let tot = val.parse().ok();
            employee.total = tot;
            employee.cfdi.total = tot;
        }
    }

    // 2. Emisor: RFC, Nombre, RegimenFiscal, RegistroPatronal
    for emisor in doc.descendants().filter(|n| n.tag_name().name() == "emisor") {
        if let Some(val) = emisor.attribute("rfc") {
            employee.rfc_emisor = Some(val.trim().to_uppercase());
        }
        if let Some(val) = emisor.attribute("nombre") {
            employee.nombre_emisor = Some(val.trim().to_uppercase());
        }
        if let Some(val) = emisor.attribute("regimenfiscal") {
            employee.regimen_fiscal_emisor = Some(val.trim().to_string());
        }
        if let Some(val) = emisor.attribute("registropatronal") {
            employee.registro_patronal = Some(val.trim().to_uppercase());
        }
        if let Some(val) = emisor.attribute("curp") {
            employee.curp_emisor = Some(val.trim().to_uppercase());
        }
    }

    // 3. Conceptos CFDI
    for concepto_node in doc.descendants().filter(|n| n.tag_name().name() == "concepto") {
        let clave_prod_serv = concepto_node.attribute("claveprodserv").map(|s| s.trim().to_string());
        let clave_unidad = concepto_node.attribute("claveunidad").map(|s| s.trim().to_string());
        let descripcion = concepto_node.attribute("descripcion").map(|s| s.trim().to_string());
        let cantidad = concepto_node.attribute("cantidad").and_then(|s| s.parse::<i16>().ok());
        let valor_unitario = concepto_node.attribute("valorunitario").and_then(|s| s.parse::<f64>().ok());
        let importe = concepto_node.attribute("importe").and_then(|s| s.parse::<f64>().ok());
        let descuento = concepto_node.attribute("descuento").and_then(|s| s.parse::<f64>().ok());
        let objeto_imp = concepto_node.attribute("objetoimp").map(|s| s.trim().to_string());

        if clave_prod_serv.is_some() || descripcion.is_some() {
            employee.cfdi.conceptos.push(crate::nominas::models::ConceptoCfdiItem {
                clave_prod_serv,
                clave_unidad,
                descripcion,
                cantidad,
                valor_unitario,
                importe,
                descuento,
                objeto_imp,
            });
        }
    }

    // 4. Receptor nodes (cfdi:Receptor and nomina12:Receptor)
    let mut found_receptor = false;
    for receptor in doc.descendants().filter(|n| n.tag_name().name() == "receptor") {
        found_receptor = true;
        if let Some(val) = receptor.attribute("rfc") {
            employee.rfc_receptor = Some(val.trim().to_uppercase());
        }
        if let Some(val) = receptor.attribute("nombre") {
            if employee.nombre.is_none() {
                employee.nombre = Some(val.trim().to_uppercase());
            }
        }
        if let Some(val) = receptor.attribute("domiciliofiscalreceptor") {
            employee.contrato.domicilio_fiscal = Some(val.trim().to_string());
        }
        if let Some(val) = receptor.attribute("regimenfiscalreceptor") {
            employee.contrato.regimen_fiscal = Some(val.trim().to_string());
        }
        if let Some(val) = receptor.attribute("usocfdi") {
            employee.contrato.uso_cfdi = Some(val.trim().to_uppercase());
        }
        if let Some(val) = receptor.attribute("curp") {
            employee.curp_receptor = Some(val.trim().to_uppercase());
        }
        if let Some(val) = receptor.attribute("numseguridadsocial") {
            employee.numseguridadsocial = Some(val.trim().to_uppercase());
        }
        if let Some(val) = receptor.attribute("salariobasecotapor") {
            employee.salariobasecotapor = val.parse().ok();
        }
        if let Some(val) = receptor.attribute("salariodiariointegrado") {
            employee.salariodiariointegrado = val.parse().ok();
        }
        if let Some(val) = receptor.attribute("fechainiciorellaboral") {
            let parsed_d = NaiveDate::parse_from_str(val.trim(), "%Y-%m-%d").ok();
            employee.fechainiciorellaboral = parsed_d;
            employee.contrato.fecha_inicio_relacion_laboral = parsed_d;
        }
        if let Some(val) = receptor.attribute("antiguedad") {
            employee.contrato.cod_antiguedad = Some(val.trim().to_string());
            let trimmed = val.trim().to_uppercase();
            if let Some(w_pos) = trimmed.find('W') {
                if let Ok(weeks) = trimmed[1..w_pos].parse::<f64>() {
                    employee.contrato.antiguedad = Some((weeks / 52.0).floor() as i16);
                }
            } else if let Some(y_pos) = trimmed.find('Y') {
                if let Ok(years) = trimmed[1..y_pos].parse::<i16>() {
                    employee.contrato.antiguedad = Some(years);
                }
            }
        }
        if let Some(val) = receptor.attribute("tipocontrato") {
            employee.contrato.tipo_contrato = Some(val.trim().to_string());
        }
        if let Some(val) = receptor.attribute("sindicalizado") {
            let lower = val.trim().to_lowercase();
            employee.contrato.es_sindicalizado = Some(lower == "sí" || lower == "si");
        }
        if let Some(val) = receptor.attribute("tipojornada") {
            employee.contrato.tipo_jornada = Some(val.trim().to_string());
        }
        if let Some(val) = receptor.attribute("tiporegimen") {
            employee.contrato.tipo_regimen = Some(val.trim().to_string());
        }
        if let Some(val) = receptor.attribute("numempleado") {
            employee.contrato.num_empleado = Some(val.trim().to_string());
        }
        if let Some(val) = receptor.attribute("departamento") {
            employee.contrato.departamento = Some(val.trim().to_string());
        }
        if let Some(val) = receptor.attribute("puesto") {
            employee.contrato.puesto = Some(val.trim().to_string());
        }
        if let Some(val) = receptor.attribute("riesgopuesto") {
            employee.contrato.riesgo_puesto = Some(val.trim().to_string());
        }
        if let Some(val) = receptor.attribute("periodicidadpago") {
            if !val.trim().is_empty() {
                let up = val.trim().to_uppercase();
                employee.nomina.periodicidad_pago = Some(up.clone());
                employee.contrato.periodicidad_pago = Some(up);
            }
        }
        if let Some(val) = receptor.attribute("claveentfed") {
            employee.contrato.clave_entidad_federativa = Some(val.trim().to_uppercase());
        }
    }

    if !found_receptor {
        anyhow::bail!("El XML no contiene ningún nodo <Receptor>");
    }

    // 5. Timbre Fiscal Digital: UUID, Version, FechaTimbrado, NoCertificadoSAT, RfcProvCertif
    for timbre in doc.descendants().filter(|n| n.tag_name().name() == "timbrefiscaldigital") {
        if let Some(val) = timbre.attribute("version") {
            employee.timbre.version = Some(val.trim().to_string());
        }
        if let Some(val) = timbre.attribute("uuid") {
            let up = val.trim().to_uppercase();
            employee.uuid = Some(up.clone());
            employee.timbre.uuid = Some(up);
        }
        if let Some(val) = timbre.attribute("fechatimbrado") {
            employee.timbre.fecha_timbrado = parse_timestamptz(val);
        }
        if let Some(val) = timbre.attribute("rfcprovcertif") {
            employee.timbre.rfc_prov_certif = Some(val.trim().to_uppercase());
        }
        if let Some(val) = timbre.attribute("nocertificadosat") {
            employee.timbre.no_certificado_sat = Some(val.trim().to_string());
        }
    }

    // 6. Nomina node
    let mut found_nomina = false;
    for nomina_node in doc.descendants().filter(|n| n.tag_name().name() == "nomina") {
        found_nomina = true;
        if let Some(val) = nomina_node.attribute("version") {
            employee.nomina.version_nomina = Some(val.trim().to_string());
        }
        if let Some(val) = nomina_node.attribute("tiponomina") {
            if !val.trim().is_empty() {
                employee.nomina.tipo_nomina = Some(val.trim().to_uppercase());
            }
        }
        if employee.nomina.periodicidad_pago.is_none() {
            if let Some(val) = nomina_node.attribute("periodicidadpago") {
                if !val.trim().is_empty() {
                    let up = val.trim().to_uppercase();
                    employee.nomina.periodicidad_pago = Some(up.clone());
                    employee.contrato.periodicidad_pago = Some(up);
                }
            }
        }
        if let Some(val) = nomina_node.attribute("numdiaspagados") {
            employee.nomina.numdiaspagados = val.parse().ok();
        }
        if let Some(val) = nomina_node.attribute("fechainicialpago") {
            employee.nomina.fechainicialpago =
                NaiveDate::parse_from_str(val.trim(), "%Y-%m-%d").ok();
        }
        if let Some(val) = nomina_node.attribute("fechafinalpago") {
            employee.nomina.fechafinalpago =
                NaiveDate::parse_from_str(val.trim(), "%Y-%m-%d").ok();
        }
        if let Some(val) = nomina_node.attribute("fechapago") {
            employee.nomina.fechapago =
                NaiveDate::parse_from_str(val.trim(), "%Y-%m-%d").ok();
        }
        if let Some(val) = nomina_node.attribute("totalpercepciones") {
            employee.nomina.total_percepciones = val.parse().ok();
            if employee.total.is_none() {
                employee.total = employee.nomina.total_percepciones;
            }
        }
        if let Some(val) = nomina_node.attribute("totaldeducciones") {
            employee.nomina.total_deducciones = val.parse().ok();
        }
        if let Some(val) = nomina_node.attribute("totalotrospagos") {
            employee.nomina.total_otros_pagos = val.parse().ok();
        }
    }

    if !found_nomina {
        anyhow::bail!("El XML no contiene el complemento oficial de Nómina <Nomina>");
    }

    // 7. Percepciones totals and children
    for perc_group in doc.descendants().filter(|n| n.tag_name().name() == "percepciones") {
        if let Some(val) = perc_group.attribute("totalsueldos") {
            employee.nomina.total_sueldos = val.parse().ok();
        }
        if let Some(val) = perc_group.attribute("totalgravado") {
            employee.nomina.total_gravado = val.parse().ok();
        }
        if let Some(val) = perc_group.attribute("totalexento") {
            employee.nomina.total_exento = val.parse().ok();
        }
    }

    // 8. Deducciones totals
    for ded_group in doc.descendants().filter(|n| n.tag_name().name() == "deducciones") {
        if let Some(val) = ded_group.attribute("totalotrasdeducciones") {
            employee.nomina.total_otras_deducciones = val.parse().ok();
        }
        if let Some(val) = ded_group.attribute("totalimpuestosretenidos") {
            employee.nomina.total_impuestos_retenidos = val.parse().ok();
        }
    }

    // 9. Percepciones, Deducciones, OtrosPagos line items
    for node in doc.descendants() {
        let tag = node.tag_name().name();
        if tag == "percepcion" {
            let tipo = node.attribute("tipopercepcion").unwrap_or("001").trim().to_string();
            let clave = node.attribute("clave").unwrap_or("001").trim().to_string();
            let concepto = node.attribute("concepto").unwrap_or("").trim().to_string();
            let gravado: f64 = node.attribute("importegravado").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let exento: f64 = node.attribute("importeexento").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let mut importe = gravado + exento;
            if importe == 0.0 {
                importe = node.attribute("importe").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            }

            employee.detalles_nomina.push(crate::nominas::models::DetalleNominaItem {
                tipo_concepto_categoria: "PERCEPCIONES".to_string(),
                tipo_sat: tipo,
                clave,
                concepto: concepto.clone(),
                importe_gravado: Some(gravado),
                importe_exento: Some(exento),
                importe,
            });

            if importe != 0.0 {
                employee.nomina.percepciones.push(Percepcion {
                    concepto: clean_concepto(&concepto),
                    importe,
                });
            }
        } else if tag == "deduccion" {
            let tipo = node.attribute("tipodeduccion").unwrap_or("001").trim().to_string();
            let clave = node.attribute("clave").unwrap_or("001").trim().to_string();
            let concepto = node.attribute("concepto").unwrap_or("").trim().to_string();
            let importe = node.attribute("importe").and_then(|s| s.parse().ok()).unwrap_or(0.0);

            employee.detalles_nomina.push(crate::nominas::models::DetalleNominaItem {
                tipo_concepto_categoria: "DEDUCCIONES".to_string(),
                tipo_sat: tipo,
                clave,
                concepto,
                importe_gravado: None,
                importe_exento: None,
                importe,
            });
        } else if tag == "otropago" {
            let tipo = node.attribute("tipootropago").unwrap_or("001").trim().to_string();
            let clave = node.attribute("clave").unwrap_or("001").trim().to_string();
            let concepto = node.attribute("concepto").unwrap_or("").trim().to_string();
            let importe = node.attribute("importe").and_then(|s| s.parse().ok()).unwrap_or(0.0);

            employee.detalles_nomina.push(crate::nominas::models::DetalleNominaItem {
                tipo_concepto_categoria: "OTROS PAGOS".to_string(),
                tipo_sat: tipo,
                clave,
                concepto: concepto.clone(),
                importe_gravado: None,
                importe_exento: None,
                importe,
            });

            if importe != 0.0 {
                employee.nomina.percepciones.push(Percepcion {
                    concepto: clean_concepto(&concepto),
                    importe,
                });
            }
        }
    }

    employee.calculate_salaries();
    Ok(employee)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_cfdi_nomina_xml() {
        let sample_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<cfdi:Comprobante Version="4.0" Total="8500.50" Fecha="2024-05-15T12:00:00" xmlns:cfdi="http://www.sat.gob.mx/cfd/4" xmlns:nomina12="http://www.sat.gob.mx/nomina12" xmlns:tfd="http://www.sat.gob.mx/TimbreFiscalDigital">
    <cfdi:Emisor Rfc="XAXX010101000" Nombre="EMPRESA DEMO SA DE CV"/>
    <cfdi:Receptor Rfc="ROGR880101XYZ" Nombre="JUAN PEREZ LOPEZ"/>
    <cfdi:Complemento>
        <tfd:TimbreFiscalDigital UUID="34B68007-AC25-45C0-94C3-4809DB7BA826" FechaTimbrado="2024-05-15T12:05:00"/>
        <nomina12:Nomina Version="1.2" TipoNomina="O" FechaPago="2024-05-15" FechaInicialPago="2024-05-01" FechaFinalPago="2024-05-15" NumDiasPagados="15.000">
            <nomina12:Receptor NumSeguridadSocial="12345678901" SalarioBaseCotApor="550.00" FechaInicioRelLaboral="2020-01-01"/>
            <nomina12:Percepciones TotalSueldos="7500.00">
                <nomina12:Percepcion TipoPercepcion="001" Clave="001" Concepto="SUELDO QUINCENAL" ImporteGravado="7500.00" ImporteExento="0.00"/>
                <nomina12:Percepcion TipoPercepcion="038" Clave="038" Concepto="VALES DE DESPENSA" ImporteGravado="0.00" ImporteExento="1000.50"/>
            </nomina12:Percepciones>
        </nomina12:Nomina>
    </cfdi:Complemento>
</cfdi:Comprobante>"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(sample_xml.as_bytes()).unwrap();

        let emp = parse_xml_file(file.path()).unwrap();
        assert_eq!(emp.uuid.unwrap(), "34B68007-AC25-45C0-94C3-4809DB7BA826");
        assert_eq!(emp.rfc_emisor.unwrap(), "XAXX010101000");
        assert_eq!(emp.rfc_receptor.unwrap(), "ROGR880101XYZ");
        assert_eq!(emp.total.unwrap(), 8500.50);
        assert_eq!(emp.nombre.unwrap(), "JUAN PEREZ LOPEZ");
        assert_eq!(emp.numseguridadsocial.unwrap(), "12345678901");
        assert_eq!(emp.nomina.numdiaspagados.unwrap(), 15.0);
        assert_eq!(emp.salariobasecotapor.unwrap(), 550.00);
        assert!(emp.salariodiariocalculado.is_some());
        assert!(emp.salariodiariointegradocalculado.is_some());
    }

    #[test]
    fn test_parse_cfdi_nomina_xml_semanal_septimo_dia() {
        let sample_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<cfdi:Comprobante Version="4.0" Total="4200.00" Fecha="2024-05-07T12:00:00" xmlns:cfdi="http://www.sat.gob.mx/cfd/4" xmlns:nomina12="http://www.sat.gob.mx/nomina12" xmlns:tfd="http://www.sat.gob.mx/TimbreFiscalDigital">
    <cfdi:Emisor Rfc="XAXX010101000" Nombre="EMPRESA DEMO SA DE CV"/>
    <cfdi:Receptor Rfc="ROGR880101XYZ" Nombre="CARLOS RAMIREZ"/>
    <cfdi:Complemento>
        <tfd:TimbreFiscalDigital UUID="AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE" FechaTimbrado="2024-05-07T12:05:00"/>
        <nomina12:Nomina Version="1.2" TipoNomina="O" FechaPago="2024-05-07" FechaInicialPago="2024-05-01" FechaFinalPago="2024-05-07" NumDiasPagados="7.000">
            <nomina12:Receptor NumSeguridadSocial="98765432101" PeriodicidadPago="02" SalarioBaseCotApor="600.00" FechaInicioRelLaboral="2022-01-01"/>
            <nomina12:Percepciones TotalSueldos="4200.00">
                <nomina12:Percepcion TipoPercepcion="001" Clave="001" Concepto="SUELDO SEMANAL" ImporteGravado="3600.00" ImporteExento="0.00"/>
                <nomina12:Percepcion TipoPercepcion="001" Clave="002" Concepto="SEPTIMO DIA" ImporteGravado="600.00" ImporteExento="0.00"/>
            </nomina12:Percepciones>
        </nomina12:Nomina>
    </cfdi:Complemento>
</cfdi:Comprobante>"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(sample_xml.as_bytes()).unwrap();

        let emp = parse_xml_file(file.path()).unwrap();
        assert_eq!(emp.nomina.periodicidad_pago.as_deref(), Some("02"));
        assert_eq!(emp.periodicidad_display(), "Semanal");
        assert_eq!(emp.get_sueldo_importe(), 3600.00);
        assert_eq!(emp.get_septimo_dia_importe(), 600.00);
        assert_eq!(emp.get_sueldo_total_con_septimo(), 4200.00);
        assert_eq!(emp.salariodiariocalculado, Some(600.00));
    }
}

