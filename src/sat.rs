use roxmltree::Document;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SatValidationResult {
    pub estado: String,          // "Vigente", "Cancelado", "No Encontrado", etc.
    pub codigo_estatus: String,  // e.g. "S - Comprobante obtenido satisfactoriamente"
    pub es_cancelable: String,
    pub estatus_cancelacion: String,
    pub is_valid: bool,
}

pub fn call_sat_webservice(
    client: &reqwest::blocking::Client,
    emisor_rfc: &str,
    receptor_rfc: &str,
    total: f64,
    uuid: &str,
) -> Result<SatValidationResult, String> {
    // SAT SOAP endpoint
    let url = "https://consultaqr.facturaelectronica.sat.gob.mx/ConsultaCFDIService.svc";

    // Formatted query string for SAT: ?re=...&rr=...&tt=...&id=...
    // Total is represented with standard 6-decimal representation or exact float
    let expresion = format!(
        "?re={}&rr={}&tt={:.6}&id={}",
        emisor_rfc.trim(),
        receptor_rfc.trim(),
        total,
        uuid.trim().to_uppercase()
    );

    let soap_payload = format!(
        r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><Consulta xmlns="http://tempuri.org/"><expresionImpresa><![CDATA[{}]]></expresionImpresa></Consulta></s:Body></s:Envelope>"#,
        expresion
    );

    let response = client
        .post(url)
        .header("Content-Type", "text/xml; charset=utf-8")
        .header("SOAPAction", "http://tempuri.org/IConsultaCFDIService/Consulta")
        .body(soap_payload)
        .send()
        .map_err(|e| format!("Error en conexión con el SAT: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "El servicio del SAT devolvió código HTTP {}",
            response.status()
        ));
    }

    let response_xml = response
        .text()
        .map_err(|e| format!("Error leyendo respuesta del SAT: {}", e))?;

    parse_sat_soap_response(&response_xml)
}

pub fn parse_sat_soap_response(xml: &str) -> Result<SatValidationResult, String> {
    let doc = Document::parse(xml)
        .map_err(|e| format!("Error analizando respuesta XML del SAT: {}", e))?;

    let mut estado = String::new();
    let mut codigo_estatus = String::new();
    let mut es_cancelable = String::new();
    let mut estatus_cancelacion = String::new();

    for node in doc.descendants() {
        match node.tag_name().name() {
            "Estado" => {
                if let Some(text) = node.text() {
                    estado = text.trim().to_string();
                }
            }
            "CodigoEstatus" => {
                if let Some(text) = node.text() {
                    codigo_estatus = text.trim().to_string();
                }
            }
            "EsCancelable" => {
                if let Some(text) = node.text() {
                    es_cancelable = text.trim().to_string();
                }
            }
            "EstatusCancelacion" => {
                if let Some(text) = node.text() {
                    estatus_cancelacion = text.trim().to_string();
                }
            }
            _ => {}
        }
    }

    if estado.is_empty() {
        return Err(format!(
            "No se encontró el nodo <Estado> en la respuesta del SAT. Respuesta: {}",
            xml
        ));
    }

    let is_valid = estado.eq_ignore_ascii_case("Vigente");

    Ok(SatValidationResult {
        estado,
        codigo_estatus,
        es_cancelable,
        estatus_cancelacion,
        is_valid,
    })
}

pub fn create_sat_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(64)
        .build()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sat_vigente() {
        let xml = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><ConsultaResponse xmlns="http://tempuri.org/"><ConsultaResult xmlns:a="http://schemas.datacontract.org/2004/07/Sat.Cfdi.Negocio.ConsultaCfdi.Servicio"><a:CodigoEstatus>S - Comprobante obtenido satisfactoriamente.</a:CodigoEstatus><a:EsCancelable>Cancelable con aceptación</a:EsCancelable><a:Estado>Vigente</a:Estado><a:EstatusCancelacion/><a:ValidacionEFOS>200</a:ValidacionEFOS></ConsultaResult></ConsultaResponse></s:Body></s:Envelope>"#;
        let res = parse_sat_soap_response(xml).unwrap();
        assert_eq!(res.estado, "Vigente");
        assert!(res.is_valid);
        assert_eq!(res.codigo_estatus, "S - Comprobante obtenido satisfactoriamente.");
    }

    #[test]
    fn test_parse_sat_cancelado() {
        let xml = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><ConsultaResponse xmlns="http://tempuri.org/"><ConsultaResult xmlns:a="http://schemas.datacontract.org/2004/07/Sat.Cfdi.Negocio.ConsultaCfdi.Servicio"><a:CodigoEstatus>S - Comprobante obtenido satisfactoriamente.</a:CodigoEstatus><a:EsCancelable>No cancelable</a:EsCancelable><a:Estado>Cancelado</a:Estado><a:EstatusCancelacion>Cancelado sin aceptación</a:EstatusCancelacion></ConsultaResult></ConsultaResponse></s:Body></s:Envelope>"#;
        let res = parse_sat_soap_response(xml).unwrap();
        assert_eq!(res.estado, "Cancelado");
        assert!(!res.is_valid);
    }

    #[test]
    fn test_parse_sat_no_encontrado() {
        let xml = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><ConsultaResponse xmlns="http://tempuri.org/"><ConsultaResult xmlns:a="http://schemas.datacontract.org/2004/07/Sat.Cfdi.Negocio.ConsultaCfdi.Servicio"><a:CodigoEstatus>N - 602: Comprobante no encontrado.</a:CodigoEstatus><a:Estado>No Encontrado</a:Estado></ConsultaResult></ConsultaResponse></s:Body></s:Envelope>"#;
        let res = parse_sat_soap_response(xml).unwrap();
        assert_eq!(res.estado, "No Encontrado");
        assert!(!res.is_valid);
    }
}

