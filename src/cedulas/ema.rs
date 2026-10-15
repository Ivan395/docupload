use chrono::NaiveDate;
use crate::cedulas::business::{clean_actividad, clean_domicilio, clean_entidad, month_num_to_name, Business};
use crate::cedulas::employees::{is_curp_candidate, DataEmployee, Employee};

/// Determina si el texto extraído de un PDF corresponde al formato oficial IMSS
/// de Propuesta de Cédula de Determinación de Cuotas (EMA / EMI-01).
pub fn is_ema_document(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("propuesta de cédula de determinación de cuotas")
        || lower.contains("propuesta de cedula de determinacion de cuotas")
        || lower.contains("propuesta de cédula de determinacion")
        || lower.contains("emi - 01")
        || lower.contains("emi-01")
        || (lower.contains("propuesta") && lower.contains("imss") && lower.contains("cuotas"))
        || (lower.contains("subdel") && lower.contains("cotizantes") && lower.contains("enfermedades y maternidad"))
}

/// Extrae el Registro Patronal de un documento EMA (11 caracteres alfanuméricos)
/// En el IMSS EMA viene típicamente formateado en 4 segmentos como:
/// "A36 - 73455 - 10 - 3 R.F.C. IVD190827B4A" o "A36-73455-10-3"
pub fn extract_ema_rp(raw_text: &str) -> Option<String> {
    // 1. Limitar la búsqueda al encabezado (antes del detalle de trabajadores) para evitar movimientos o claves de baja
    let header_text = if let Some(pos) = raw_text.to_uppercase().find("DETALLE DE TRABAJADORES") {
        &raw_text[..pos]
    } else {
        let cutoff = raw_text.len().min(4000);
        &raw_text[..cutoff]
    };

    // 2. Buscar específicamente el patrón estándar IMSS con guiones: [Letra][2 dígitos] - [5 dígitos] - [2 dígitos] - [1 dígito]
    if let Ok(reg) = fancy_regex::Regex::new(r"(?i)\b([A-Z]\d{2})\s*-\s*(\d{5})\s*-\s*(\d{2})\s*-\s*(\d)\b") {
        if let Ok(Some(caps)) = reg.captures(header_text) {
            let p1 = caps.get(1).map_or("", |m| m.as_str());
            let p2 = caps.get(2).map_or("", |m| m.as_str());
            let p3 = caps.get(3).map_or("", |m| m.as_str());
            let p4 = caps.get(4).map_or("", |m| m.as_str());
            let clean = format!("{}{}{}{}", p1, p2, p3, p4).to_uppercase();
            if clean.len() == 11 {
                return Some(clean);
            }
        }
    }

    // 3. Buscar en la línea que contiene "R.F.C." o "RFC", extrayendo la porción previa
    for line in header_text.lines() {
        let upper = line.to_uppercase();
        if upper.contains("R.F.C.") || upper.contains("RFC") {
            let prefix = if let Some(idx) = upper.find("R.F.C.") {
                &line[..idx]
            } else if let Some(idx) = upper.find("RFC") {
                &line[..idx]
            } else {
                ""
            };
            let candidate: String = prefix.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            if (candidate.len() == 10 || candidate.len() == 11)
                && candidate.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
            {
                return Some(candidate.to_uppercase());
            }
        }
    }

    // 4. Búsqueda fallback con regex general de RP IMSS (1 letra seguida de 9 o 10 números)
    if let Ok(reg) = fancy_regex::Regex::new(r"(?i)\b([A-Z]\d{9,10})\b") {
        if let Ok(Some(mat)) = reg.find(header_text) {
            return Some(mat.as_str().trim().to_uppercase());
        }
    }

    None
}

/// Extrae el RFC del patrón de un documento EMA
pub fn extract_ema_rfc(raw_text: &str) -> Option<String> {
    let header_text = if let Some(pos) = raw_text.to_uppercase().find("DETALLE DE TRABAJADORES") {
        &raw_text[..pos]
    } else {
        let cutoff = raw_text.len().min(4000);
        &raw_text[..cutoff]
    };

    let reg = fancy_regex::Regex::new(r"(?i)r\.?f\.?c\.?[\s:]*([a-zñ&]{3,4}\d{6}[a-z0-9]{3})").ok()?;
    if let Ok(Some(caps)) = reg.captures(header_text) {
        if let Some(m) = caps.get(1) {
            return Some(m.as_str().trim().to_uppercase());
        }
    }
    None
}

/// Extrae el periodo (mes normalizado y año) de un documento EMA.
/// Ejemplo: PERIODO ... \n 01 - 2025 -> (Some("ENERO"), Some("2025"))
pub fn extract_ema_period(raw_text: &str) -> (Option<String>, Option<String>) {
    let header_text = if let Some(pos) = raw_text.to_uppercase().find("DETALLE DE TRABAJADORES") {
        &raw_text[..pos]
    } else {
        let cutoff = raw_text.len().min(4000);
        &raw_text[..cutoff]
    };

    let lines: Vec<&str> = header_text.lines().collect();

    // 1. Buscar sección PERIODO y revisar las líneas subsiguientes
    if let Ok(re) = fancy_regex::Regex::new(r"(?<![\d\-])\b(0[1-9]|1[0-2])\s*-\s*(20\d{2})\b") {
        for (i, line) in lines.iter().enumerate() {
            if line.to_uppercase().contains("PERIODO") {
                for sub_line in lines.iter().skip(i).take(4) {
                    if let Ok(Some(caps)) = re.captures(sub_line) {
                        let mes_num = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(0);
                        let anio = caps.get(2).map(|m| m.as_str().trim().to_string());
                        if (1..=12).contains(&mes_num) {
                            return (Some(month_num_to_name(mes_num).to_string()), anio);
                        }
                    }
                }
            }
        }

        // 2. Fallback: buscar cualquier (0[1-9]|1[0-2]) - (20\d{2}) en el encabezado
        if let Ok(Some(caps)) = re.captures(header_text) {
            let mes_num = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(0);
            let anio = caps.get(2).map(|m| m.as_str().trim().to_string());
            if (1..=12).contains(&mes_num) {
                return (Some(month_num_to_name(mes_num).to_string()), anio);
            }
        }
    }

    (None, None)
}

/// Extrae el nombre estandarizado de Cédula tal como se genera en PostgreSQL public.liquidaciones:
/// "Cédula SUA <registro_patronal> - <mes>.pdf"
pub fn extract_ema_postgres_filename(raw_text: &str, default_rp: &str) -> Option<String> {
    let rp = extract_ema_rp(raw_text).unwrap_or_else(|| {
        if default_rp.is_empty() || default_rp == "GENERAL" {
            "00000000000".to_string()
        } else {
            default_rp.to_string()
        }
    });

    let (mes_opt, _) = extract_ema_period(raw_text);
    let mes = mes_opt.unwrap_or_else(|| "ENERO".to_string());

    Some(format!("Cédula SUA {} - {}.pdf", rp, mes))
}

/// Parsea la fecha en formato DD-MM-YYYY o DD/MM/YYYY
fn parse_ema_date(s: &str) -> Option<NaiveDate> {
    let trimmed = s.trim();
    if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%d-%m-%Y") {
        return Some(d);
    }
    if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%d/%m/%Y") {
        return Some(d);
    }
    None
}

/// Parsea un valor flotante removiendo comas
fn parse_ema_float(s: &str) -> Option<f64> {
    s.trim().replace(',', "").parse::<f64>().ok()
}

/// Parsea una línea de movimiento numérico de un trabajador en la EMA.
/// En la EMA, una línea de movimiento contiene al final exactamente 14 números:
/// [días] [sdi] [cf] [exc_pat] [exc_obr] [pd_pat] [pd_obr] [gmp_pat] [gmp_obr] [rt] [iv_pat] [iv_obr] [gps] [suma]
/// Y antes de esos 14 números pueden venir:
/// - [origen]
/// - [origen] [clave] [fecha]
/// - [clave] [fecha]
fn parse_ema_movement_line(line: &str, mes: &str, anio: &str) -> Option<DataEmployee> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 14 {
        return None;
    }

    // Tomar los últimos 14 tokens y verificar si son números
    let num_start_idx = tokens.len() - 14;
    let num_tokens = &tokens[num_start_idx..];

    let dias = parse_ema_float(num_tokens[0])?;
    let sdi = parse_ema_float(num_tokens[1])?;
    let cf = parse_ema_float(num_tokens[2])?;
    let exc_pat = parse_ema_float(num_tokens[3])?;
    let exc_obr = parse_ema_float(num_tokens[4])?;
    let pd_pat = parse_ema_float(num_tokens[5])?;
    let pd_obr = parse_ema_float(num_tokens[6])?;
    let gmp_pat = parse_ema_float(num_tokens[7])?;
    let gmp_obr = parse_ema_float(num_tokens[8])?;
    let rt = parse_ema_float(num_tokens[9])?;
    let iv_pat = parse_ema_float(num_tokens[10])?;
    let iv_obr = parse_ema_float(num_tokens[11])?;
    let gps = parse_ema_float(num_tokens[12])?;
    let suma = parse_ema_float(num_tokens[13])?;

    // Prefijos de la línea antes de los 14 números
    let prefix_tokens = &tokens[..num_start_idx];
    let mut clave: Option<String> = None;
    let mut fecha: Option<NaiveDate> = None;

    if prefix_tokens.len() == 2 {
        // [clave, fecha]
        clave = Some(prefix_tokens[0].trim().to_uppercase());
        fecha = parse_ema_date(prefix_tokens[1]);
    } else if prefix_tokens.len() >= 3 {
        // [origen, clave, fecha]
        clave = Some(prefix_tokens[1].trim().to_uppercase());
        fecha = parse_ema_date(prefix_tokens[2]);
    }

    let patronal = cf + exc_pat + pd_pat + gmp_pat + rt + iv_pat + gps;
    let obrera = exc_obr + pd_obr + gmp_obr + iv_obr;

    Some(DataEmployee {
        mes_periodo: Some(mes.to_string()),
        anio_periodo: Some(anio.to_string()),
        clave,
        fecha,
        dias: Some(dias),
        sdi: Some(sdi),
        lic: None,
        inc: None,
        aus: None,
        cf: Some(cf),
        exc_pat: Some(exc_pat),
        exc_obr: Some(exc_obr),
        pd_pat: Some(pd_pat),
        pd_obr: Some(pd_obr),
        gmp_pat: Some(gmp_pat),
        gmp_obr: Some(gmp_obr),
        rt: Some(rt),
        iv_pat: Some(iv_pat),
        iv_obr: Some(iv_obr),
        gps: Some(gps),
        patronal: Some(patronal),
        obrera: Some(obrera),
        subtotal: Some(suma),
    })
}

/// Extrae la información de la empresa (Business) desde el documento EMA
pub fn parse_ema_business(raw_text: &str) -> Business {
    let mut biz = Business::new();
    biz.area_geografica = Some("A".to_string());

    biz.registro_patronal = extract_ema_rp(raw_text);
    biz.rfc = extract_ema_rfc(raw_text);

    let lines: Vec<&str> = raw_text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();

    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();

        // Delegación: Del 05-ESTATAL COAHUILA
        if lower.starts_with("del ") || lower.starts_with("del.") {
            let part = line.trim_start_matches(|c: char| c.is_alphabetic() || c == '.' || c == ' ');
            if let Some(dash_idx) = part.find('-') {
                biz.codigo_delegacion = Some(part[..dash_idx].trim().to_string());
                biz.delegacion = Some(part[dash_idx + 1..].trim().to_uppercase());
            } else {
                biz.delegacion = Some(part.trim().to_uppercase());
            }
        }

        // Subdelegación: Subdel 03-SALTILLO
        if lower.starts_with("subdel ") || lower.starts_with("subdel.") {
            let part = line.trim_start_matches(|c: char| c.is_alphabetic() || c == '.' || c == ' ');
            if let Some(dash_idx) = part.find('-') {
                biz.codigo_subdelegacion = Some(part[..dash_idx].trim().to_string());
                biz.subdelegacion = Some(part[dash_idx + 1..].trim().to_uppercase());
            } else {
                biz.subdelegacion = Some(part.trim().to_uppercase());
            }

            // En la estructura del formato IMSS, la siguiente línea a Subdel suele ser la Razón Social
            if biz.razon_social.is_none() && i + 1 < lines.len() {
                let candidate_rs = lines[i + 1].trim();
                let clean_candidate = candidate_rs.split("PATRONAL").next().unwrap_or(candidate_rs).trim();
                if !clean_candidate.is_empty()
                    && !clean_candidate.to_lowercase().contains("r.f.c")
                    && !clean_candidate.to_lowercase().contains("rfc")
                {
                    biz.razon_social = Some(clean_candidate.to_uppercase());
                }
            }
        }

        // Fallback Razón Social: línea previa al RFC / Registro Patronal
        if biz.razon_social.is_none() && (lower.contains("r.f.c.") || lower.contains("rfc")) && i > 0 {
            let prev_line = lines[i - 1].trim();
            let clean_candidate = prev_line.split("PATRONAL").next().unwrap_or(prev_line).trim();
            if !clean_candidate.is_empty()
                && !clean_candidate.to_lowercase().contains("subdel")
                && !clean_candidate.to_lowercase().contains("delegación")
                && !clean_candidate.to_lowercase().contains("delegacion")
                && !clean_candidate.to_lowercase().contains("propuesta")
            {
                biz.razon_social = Some(clean_candidate.to_uppercase());
            }
        }

        // Domicilio y Entidad
        if biz.domicilio.is_none() && (lower.contains("r.f.c.") || lower.contains("rfc")) && i + 1 < lines.len() {
            let candidate_dom = lines[i + 1].trim();
            if !candidate_dom.is_empty() {
                biz.domicilio = Some(clean_domicilio(candidate_dom));
            }
            if i + 2 < lines.len() {
                let candidate_ent = lines[i + 2].trim();
                let clean_ent = clean_entidad(candidate_ent);
                if !clean_ent.is_empty() {
                    biz.entidad = Some(clean_ent);
                }
            }
        }

        // Código Postal: C.P. 25294
        if biz.codigo_postal.is_none() && lower.contains("c.p.") {
            if let Some(pos) = lower.find("c.p.") {
                let after = &line[pos + 4..].trim();
                let cp_digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                if cp_digits.len() == 5 {
                    biz.codigo_postal = Some(cp_digits);
                }
            }
        }

        // Actividad: 4101 CONSTRUCCION DE INMUEBLES COMERCIAL
        if biz.actividad.is_none() {
            let words: Vec<&str> = line.split_whitespace().collect();
            if words.len() >= 2 && words[0].len() == 4 && words[0].chars().all(|c| c.is_ascii_digit()) {
                let act_desc = words[1..].join(" ");
                biz.actividad = Some(clean_actividad(&act_desc));
            }
        }

        // Prima R.T.
        if biz.prima.is_none() && lower.contains("prima r.") {
            // La prima puede estar en la misma línea o en la siguiente
            let prima_str = if let Some(pos) = lower.find("prima r.") {
                let after = &line[pos..];
                after
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.')
                    .collect::<String>()
            } else {
                String::new()
            };
            if !prima_str.is_empty() {
                biz.prima = Some(prima_str);
            } else if i + 1 < lines.len() {
                let next_line = lines[i + 1].trim();
                let candidate_prima: String = next_line
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                if !candidate_prima.is_empty() && candidate_prima.contains('.') {
                    biz.prima = Some(candidate_prima);
                }
            }
        }
    }

    biz
}

/// Parsea todos los trabajadores y sus movimientos desde el documento EMA
pub fn parse_ema_employees(raw_text: &str, mes: &str, anio: &str) -> Vec<Employee> {
    let mut employees: Vec<Employee> = Vec::new();
    let mut current_emp: Option<Employee> = None;

    let lines: Vec<&str> = raw_text.lines().map(|l| l.trim()).collect();

    for line in lines {
        if line.is_empty() {
            continue;
        }

        // 1. Detectar si la línea es el encabezado de un asegurado:
        // En la EMA el NSS viene al inicio como "32 99 82 5338 2" (5 bloques de dígitos que suman 11)
        // o con guiones "32-99-82-5338-2" o continuo "32998253382".
        // Al final de la línea viene la CURP (18 caracteres alfanuméricos válidos).
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() >= 3 {
            let last_token = tokens[tokens.len() - 1].trim();
            if is_curp_candidate(last_token) {
                // Verificar si los primeros tokens forman un NSS de 11 dígitos
                let mut nss_acc = String::new();
                let mut nss_token_count = 0;
                for tok in &tokens[..tokens.len() - 1] {
                    if tok.chars().all(|c| c.is_ascii_digit() || c == '-') {
                        let digits: String = tok.chars().filter(|c| c.is_ascii_digit()).collect();
                        nss_acc.push_str(&digits);
                        nss_token_count += 1;
                        if nss_acc.len() == 11 {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                if nss_acc.len() == 11 {
                    // Si ya había un trabajador en progreso, guardarlo
                    if let Some(emp) = current_emp.take() {
                        employees.push(emp);
                    }

                    let curp = last_token.to_uppercase();
                    let raw_nombre = tokens[nss_token_count..tokens.len() - 1].join(" ");
                    // En el IMSS '#' representa la letra 'Ñ'
                    let clean_nombre = raw_nombre.replace('#', "Ñ").to_uppercase();

                    // Generar RFC oficial desde CURP y Nombre
                    let rfc_generado = crate::cedulas::rfc_sat::calcular_rfc_sat(
                        &clean_nombre,
                        Some(&curp),
                        Some(&nss_acc),
                    );
                    let final_rfc = if rfc_generado.len() == 13 && rfc_generado != "0000000000000" && rfc_generado != "XAXX010101000" {
                        Some(rfc_generado)
                    } else {
                        Some("XAXX010101000".to_string())
                    };

                    let mut new_emp = Employee::new();
                    new_emp.no_seguro_social = Some(nss_acc);
                    new_emp.nombre = Some(clean_nombre);
                    new_emp.curp = Some(curp);
                    new_emp.rfc = final_rfc;
                    new_emp.mes_periodo = Some(mes.to_string());
                    new_emp.anio_periodo = Some(anio.to_string());

                    current_emp = Some(new_emp);
                    continue;
                }
            }
        }

        // 2. Si no fue encabezado de trabajador, intentar parsear como renglón de movimiento
        if current_emp.is_some() {
            if let Some(de) = parse_ema_movement_line(line, mes, anio) {
                if let Some(ref mut emp) = current_emp {
                    emp.data_employee.push(de);
                }
            }
        }
    }

    if let Some(emp) = current_emp.take() {
        employees.push(emp);
    }

    employees
}

/// Parsea un documento EMA completo y retorna el Business con todos sus empleados
pub fn parse_ema_document(raw_text: &str, file_name: &str) -> Result<(Business, Vec<Employee>), String> {
    let mut biz = parse_ema_business(raw_text);
    let (mes_opt, anio_opt) = extract_ema_period(raw_text);

    let mes = mes_opt.unwrap_or_else(|| "ENERO".to_string());
    let anio = anio_opt.unwrap_or_else(|| "2025".to_string());

    let employees = parse_ema_employees(raw_text, &mes, &anio);

    if employees.is_empty() {
        return Err(format!(
            "No se encontraron trabajadores válidos en el documento EMA '{}'",
            file_name
        ));
    }

    biz.employees = employees.clone();
    Ok((biz, employees))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_EMA_TEXT: &str = r#"
Propuesta de Cédula de Determinación de Cuotas IMSS
Del 05-ESTATAL COAHUILA
Subdel 03-SALTILLO
INDUSTRIAS VC DEL DESIERTO SA DE CV
A36 - 73455 - 10 - 3 R.F.C. IVD190827B4A
CERRO DE LA CAPILLA 128 VILLAS DE GUADAL SALTILLO
COAHUILA DE ZARAGOZA
C.P. 25294 MPIO. A36 SEC. NOT. 27
4101 CONSTRUCCION DE INMUEBLES COMERCIAL

PERIODO
01 - 2025
PROPUESTA
IMSS
251017933
PRIMA R. T.
7.58875
CLASE RT
5

DETALLE DE TRABAJADORES
N° de Seguridad Social Apellidos y Nombre(s) del Asegurado CURP
MOVIMIENTOS CUOTAS ENFERMEDADES Y MATERNIDAD
32 99 82 5338 2 ACOSTA JUAREZ DANIEL AOJD821108HCLCRN06
4 31 292.54 686.60 0.00 0.00 63.48 22.67 95.22 34.01 688.20 158.70 56.68 90.69 1,896.25
02 24 01 9925 3 ALVARADO RODRIGUEZ ANA DANIELA AARA010723MCLLDNA3
4 MS 02-01-2025 30 292.54 664.45 0.00 0.00 61.43 21.94 92.15 32.91 666.00 153.58 54.85 87.76 1,835.07
4 1 261.20 22.15 0.00 0.00 1.95 0.70 2.93 1.05 19.82 4.57 1.63 2.61 57.41
18 15 96 2914 8 MERCADO GARCIA ZEFERINO MEGZ961209HZSRRF02
0 B 01-01-2025 0 0.00 0.00 0.00 0.00 0.00 0.00 0.00 0.00 0.00 0.00 0.00 0.00 0.00
"#;

    #[test]
    fn test_is_ema() {
        assert!(is_ema_document(SAMPLE_EMA_TEXT));
    }

    #[test]
    fn test_extract_ema_metadata() {
        let rp = extract_ema_rp(SAMPLE_EMA_TEXT);
        assert_eq!(rp.as_deref(), Some("A3673455103"));

        let rfc = extract_ema_rfc(SAMPLE_EMA_TEXT);
        assert_eq!(rfc.as_deref(), Some("IVD190827B4A"));

        let (mes, anio) = extract_ema_period(SAMPLE_EMA_TEXT);
        assert_eq!(mes.as_deref(), Some("ENERO"));
        assert_eq!(anio.as_deref(), Some("2025"));

        let fname = extract_ema_postgres_filename(SAMPLE_EMA_TEXT, "A3673455103");
        assert_eq!(fname.as_deref(), Some("Cédula SUA A3673455103 - ENERO.pdf"));
    }

    #[test]
    fn test_parse_ema_workers() {
        let (biz, emps) = parse_ema_document(SAMPLE_EMA_TEXT, "test.pdf").expect("Error parsing EMA");
        assert_eq!(biz.registro_patronal.as_deref(), Some("A3673455103"));
        assert_eq!(biz.razon_social.as_deref(), Some("INDUSTRIAS VC DEL DESIERTO SA DE CV"));
        assert_eq!(biz.delegacion.as_deref(), Some("ESTATAL COAHUILA"));
        assert_eq!(biz.subdelegacion.as_deref(), Some("SALTILLO"));
        assert_eq!(biz.prima.as_deref(), Some("7.58875"));

        assert_eq!(emps.len(), 3);

        // Trabajador 1: ACOSTA JUAREZ DANIEL
        let w1 = &emps[0];
        assert_eq!(w1.nombre.as_deref(), Some("ACOSTA JUAREZ DANIEL"));
        assert_eq!(w1.no_seguro_social.as_deref(), Some("32998253382"));
        assert_eq!(w1.curp.as_deref(), Some("AOJD821108HCLCRN06"));
        assert_eq!(w1.data_employee.len(), 1);
        assert_eq!(w1.data_employee[0].dias, Some(31.0));
        assert_eq!(w1.data_employee[0].sdi, Some(292.54));
        assert_eq!(w1.data_employee[0].subtotal, Some(1896.25));

        // Trabajador 2: ALVARADO RODRIGUEZ ANA DANIELA (2 movimientos)
        let w2 = &emps[1];
        assert_eq!(w2.nombre.as_deref(), Some("ALVARADO RODRIGUEZ ANA DANIELA"));
        assert_eq!(w2.data_employee.len(), 2);
        assert_eq!(w2.data_employee[0].clave.as_deref(), Some("MS"));
        assert_eq!(w2.data_employee[0].dias, Some(30.0));
        assert_eq!(w2.data_employee[0].subtotal, Some(1835.07));
        assert_eq!(w2.data_employee[1].dias, Some(1.0));
        assert_eq!(w2.data_employee[1].subtotal, Some(57.41));

        // Trabajador 3: MERCADO GARCIA ZEFERINO (Baja)
        let w3 = &emps[2];
        assert_eq!(w3.nombre.as_deref(), Some("MERCADO GARCIA ZEFERINO"));
        assert_eq!(w3.data_employee.len(), 1);
        assert_eq!(w3.data_employee[0].clave.as_deref(), Some("B"));
        assert_eq!(w3.data_employee[0].dias, Some(0.0));
    }
}
