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

    // 1. Root comprobante attributes: total
    for node in doc.descendants().filter(|n| n.tag_name().name() == "comprobante") {
        if let Some(val) = node.attribute("total") {
            employee.total = val.parse().ok();
        }
    }

    // 2. Emisor: RFC and Nombre
    for emisor in doc.descendants().filter(|n| n.tag_name().name() == "emisor") {
        if let Some(val) = emisor.attribute("rfc") {
            employee.rfc_emisor = Some(val.trim().to_uppercase());
        }
        if let Some(val) = emisor.attribute("nombre") {
            employee.nombre_emisor = Some(val.trim().to_uppercase());
        }
    }

    // 3. Receptor nodes (cfdi:Receptor and nomina12:Receptor)
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
        if let Some(val) = receptor.attribute("numseguridadsocial") {
            employee.numseguridadsocial = Some(val.trim().to_uppercase());
        }
        if let Some(val) = receptor.attribute("salariobasecotapor") {
            employee.salariobasecotapor = val.parse().ok();
        }
        if let Some(val) = receptor.attribute("fechainiciorellaboral") {
            employee.fechainiciorellaboral =
                NaiveDate::parse_from_str(val.trim(), "%Y-%m-%d").ok();
        }
    }

    if !found_receptor {
        anyhow::bail!("El XML no contiene ningún nodo <Receptor>");
    }

    // 4. Timbre Fiscal Digital: UUID
    for timbre in doc.descendants().filter(|n| n.tag_name().name() == "timbrefiscaldigital") {
        if let Some(val) = timbre.attribute("uuid") {
            employee.uuid = Some(val.trim().to_uppercase());
        }
    }

    // 5. Nomina node
    let mut found_nomina = false;
    for nomina_node in doc.descendants().filter(|n| n.tag_name().name() == "nomina") {
        found_nomina = true;
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
        if employee.total.is_none() {
            if let Some(val) = nomina_node.attribute("totalpercepciones") {
                employee.total = val.parse().ok();
            }
        }
    }

    if !found_nomina {
        anyhow::bail!("El XML no contiene el complemento oficial de Nómina <Nomina>");
    }

    // 6. Percepciones and otros pagos
    let targets = ["percepcion", "otropago"];
    for node in doc.descendants() {
        if targets.contains(&node.tag_name().name()) {
            let concepto = node.attribute("concepto").unwrap_or("").to_string();
            let mut importe = node
                .attribute("importe")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);

            if node.tag_name().name() == "percepcion" {
                let gravado: f64 = node
                    .attribute("importegravado")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let exento: f64 = node
                    .attribute("importeexento")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                if gravado > 0.0 || exento > 0.0 {
                    importe = gravado + exento;
                }
            }

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
}

