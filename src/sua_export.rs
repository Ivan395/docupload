use chrono::NaiveDate;

/// Limpia y normaliza texto para formato ASCII estricto requerido por SUA IMSS.
/// - Convierte vocales con acento/diéresis a mayúsculas ASCII sin acento.
/// - Reemplaza la 'Ñ' por '/' según el estándar oficial de los layouts del SUA.
/// - Pasa todo a mayúsculas.
/// - Elimina caracteres no imprimibles o no ASCII.
pub fn clean_ascii_sua(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'á' | 'Á' | 'à' | 'À' | 'ä' | 'Ä' | 'â' | 'Â' => out.push('A'),
            'é' | 'É' | 'è' | 'È' | 'ë' | 'Ë' | 'ê' | 'Ê' => out.push('E'),
            'í' | 'Í' | 'ì' | 'Ì' | 'ï' | 'Ï' | 'î' | 'Î' => out.push('I'),
            'ó' | 'Ó' | 'ò' | 'Ò' | 'ö' | 'Ö' | 'ô' | 'Ô' => out.push('O'),
            'ú' | 'Ú' | 'ù' | 'Ù' | 'ü' | 'Ü' | 'û' | 'Û' => out.push('U'),
            'ñ' | 'Ñ' => out.push('/'),
            c if c.is_ascii() => {
                if c.is_ascii_graphic() || c == ' ' {
                    out.push(c.to_ascii_uppercase());
                }
            }
            _ => out.push(' '),
        }
    }
    out
}

/// Divide un nombre completo en (Primer Apellido / Paterno, Segundo Apellido / Materno, Nombres)
/// reconociendo prefijos compuestos típicos en apellidos hispanos ("DE", "DEL", "DE LA", etc.).
pub fn parse_mexican_name(full: &str) -> (String, String, String) {
    let clean = clean_ascii_sua(full);
    let words: Vec<&str> = clean.split_whitespace().collect();
    if words.is_empty() {
        return (String::new(), String::new(), String::new());
    }

    let is_prefix = |w: &str| -> bool {
        matches!(w, "DE" | "DEL" | "LA" | "LAS" | "LOS" | "SAN" | "SANTA" | "VON" | "VAN")
    };

    let mut idx = 0;

    // 1. Extraer Paterno
    let mut paterno_parts = Vec::new();
    while idx < words.len() {
        let w = words[idx];
        paterno_parts.push(w);
        idx += 1;
        if !is_prefix(w) || idx >= words.len() {
            break;
        }
    }
    let paterno = paterno_parts.join(" ");

    // 2. Extraer Materno
    let mut materno_parts = Vec::new();
    while idx < words.len() {
        let w = words[idx];
        materno_parts.push(w);
        idx += 1;
        if !is_prefix(w) || idx >= words.len() {
            break;
        }
    }
    let materno = materno_parts.join(" ");

    // 3. El resto son Nombres
    let nombres = if idx < words.len() {
        words[idx..].join(" ")
    } else {
        String::new()
    };

    // Caso especial: Si solo hay 2 palabras, la primera es Paterno y la segunda es Nombre
    if materno_parts.is_empty() && !paterno.is_empty() && !nombres.is_empty() {
        (paterno, String::new(), nombres)
    } else if materno_parts.len() == 1 && nombres.is_empty() && words.len() == 2 {
        (paterno, String::new(), materno)
    } else {
        (paterno, materno, nombres)
    }
}

/// Formatea el nombre completo para layout ASEG con delimitador `$`:
/// `APELLIDO PATERNO$APELLIDO MATERNO$NOMBRE(S)`
pub fn format_sua_name_delimited(full: &str) -> String {
    let (paterno, materno, nombres) = parse_mexican_name(full);
    if materno.is_empty() {
        format!("{}$${}", paterno, nombres)
    } else {
        format!("{}${}${}", paterno, materno, nombres)
    }
}

/// Limpia y ajusta el Registro Patronal a 11 posiciones alfanuméricas.
pub fn sanitize_rp(rp: &str) -> String {
    let clean: String = rp.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let upper = clean.to_ascii_uppercase();
    if upper.len() >= 11 {
        upper[..11].to_string()
    } else {
        format!("{:<11}", upper)
    }
}

/// Limpia y ajusta el NSS a exactamente 11 dígitos numéricos.
pub fn sanitize_nss(nss: &str) -> String {
    let digits: String = nss.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 11 {
        digits[..11].to_string()
    } else {
        format!("{:0>11}", digits)
    }
}

/// Limpia y ajusta el RFC a 13 posiciones (Persona Física: 13, Persona Moral: 12 + 1 espacio).
pub fn sanitize_rfc(rfc: &str) -> String {
    let clean: String = rfc.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let upper = clean.to_ascii_uppercase();
    if upper.len() >= 13 {
        upper[..13].to_string()
    } else {
        format!("{:<13}", upper)
    }
}

/// Resuelve un RFC válido de 13 posiciones para empleados en SUA usando el algoritmo oficial del SAT.
/// 1. Si `rfc` tiene 13 caracteres alfanuméricos válidos (distintos de ceros y no sintéticos), lo conserva.
/// 2. Si no, genera el RFC oficial completo con homoclave y dígito verificador del SAT a partir de una CURP válida.
/// 3. En caso de que el RFC esté vacío o no se pueda calcular con fecha real, coloca el genérico oficial ("XAXX010101000") que acepta el SUA.
pub fn resolver_rfc_valido(rfc: &str, curp: &str, nombre: &str, nss: &str) -> String {
    let clean_rfc: String = rfc.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let upper_rfc = clean_rfc.to_ascii_uppercase();

    if (upper_rfc.len() == 13 || upper_rfc.len() == 12)
        && upper_rfc != "0000000000000"
        && upper_rfc != "XAXX010101000"
        && !upper_rfc.starts_with("CONC")
        && upper_rfc != "PEGI800101XYZ"
        && upper_rfc != "CAEH890101C10"
        && upper_rfc != "RASF970101R39"
    {
        let l = if upper_rfc.len() == 13 { &upper_rfc[..4] } else { &upper_rfc[..3] };
        let d = if upper_rfc.len() == 13 { &upper_rfc[4..10] } else { &upper_rfc[3..9] };
        if l.chars().all(|c| c.is_ascii_alphabetic()) && d.chars().all(|c| c.is_ascii_digit()) {
            return if upper_rfc.len() == 12 { format!("{:<13}", upper_rfc) } else { upper_rfc };
        }
    }

    let clean_nom = nombre.trim();
    if !clean_nom.is_empty() && !clean_nom.eq_ignore_ascii_case("NONE") && clean_nom.chars().any(|c| c.is_alphabetic()) {
        let calc = crate::cedulas::rfc_sat::calcular_rfc_sat(nombre, Some(curp), Some(nss));
        if calc.len() == 13 && calc != "0000000000000" && calc != "XAXX010101000" {
            return calc;
        }
    }

    // En caso de que el RFC esté vacío o no se pueda calcular, colocar el genérico oficial aceptado por el SUA
    "XAXX010101000".to_string()
}

/// Obtiene el nombre del estado (hasta 25 caracteres) y la clave del lugar (2 dígitos: 01 a 33)
/// a partir del código de entidad de la CURP (posiciones 11 y 12), según el catálogo oficial del SUA.
pub fn estado_y_clave_lugar_desde_curp(curp: &str) -> (&'static str, &'static str) {
    let clean: String = curp.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let upper = clean.to_ascii_uppercase();
    if upper.len() >= 13 {
        let code = &upper[11..13];
        match code {
            "AS" => ("AGUASCALIENTES", "01"),
            "BC" => ("BAJA CALIFORNIA", "02"),
            "BS" => ("BAJA CALIFORNIA SUR", "03"),
            "CC" => ("CAMPECHE", "04"),
            "CL" => ("COAHUILA", "05"),
            "CM" => ("COLIMA", "06"),
            "CS" => ("CHIAPAS", "07"),
            "CH" => ("CHIHUAHUA", "08"),
            "DF" => ("CIUDAD DE MEXICO", "09"),
            "DG" => ("DURANGO", "10"),
            "GT" => ("GUANAJUATO", "11"),
            "GR" => ("GUERRERO", "12"),
            "HG" => ("HIDALGO", "13"),
            "JC" => ("JALISCO", "14"),
            "MC" => ("ESTADO DE MEXICO", "15"),
            "MN" => ("MICHOACAN", "16"),
            "MS" => ("MORELOS", "17"),
            "NT" => ("NAYARIT", "18"),
            "NL" => ("NUEVO LEON", "19"),
            "OC" => ("OAXACA", "20"),
            "PL" => ("PUEBLA", "21"),
            "QT" => ("QUERETARO", "22"),
            "QR" => ("QUINTANA ROO", "23"),
            "SP" => ("SAN LUIS POTOSI", "24"),
            "SL" => ("SINALOA", "25"),
            "SR" => ("SONORA", "26"),
            "TC" => ("TABASCO", "27"),
            "TS" => ("TAMAULIPAS", "28"),
            "TL" => ("TLAXCALA", "29"),
            "VZ" => ("VERACRUZ", "30"),
            "YN" => ("YUCATAN", "31"),
            "ZS" => ("ZACATECAS", "32"),
            "NE" => ("EXTRANJERO", "33"),
            _ => ("COAHUILA", "05"),
        }
    } else {
        ("COAHUILA", "05")
    }
}

/// Limpia y ajusta la CURP a 18 posiciones.
pub fn sanitize_curp(curp: &str) -> String {
    let clean: String = curp.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let upper = clean.to_ascii_uppercase();
    if upper.len() >= 18 {
        upper[..18].to_string()
    } else {
        format!("{:<18}", upper)
    }
}

/// Resuelve una CURP oficial válida de 18 posiciones para empleados en SUA usando el estándar oficial de RENAPO.
/// 1. Si `curp` tiene 18 caracteres y cumple con el formato oficial de RENAPO, la conserva.
/// 2. Si no, calcula la CURP oficial con el algoritmo oficial de RENAPO sólo si cuenta con fecha de nacimiento real.
/// 3. Si no se puede calcular o determinar con los datos del empleado, la deja en blanco (18 espacios).
pub fn resolver_curp_valida(curp: &str, nombre: &str, rfc: &str, nss: &str) -> String {
    let clean: String = curp.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let upper = clean.to_ascii_uppercase();
    if upper.len() == 18 && crate::cedulas::curp_renapo::validar_curp_formato(&upper) {
        return upper;
    }

    let clean_nom = nombre.trim();
    if !clean_nom.is_empty() && !clean_nom.eq_ignore_ascii_case("NONE") && clean_nom.chars().any(|c| c.is_alphabetic()) {
        let calc = crate::cedulas::curp_renapo::calcular_curp_oficial(nombre, Some(rfc), Some(nss), None, None);
        if calc.len() == 18 && crate::cedulas::curp_renapo::validar_curp_formato(&calc) {
            return calc;
        }
    }

    // Si no se puede determinar la CURP, se deja en blanco (18 espacios) según el estándar del SUA
    format!("{:<18}", "")
}

/// Genera una CURP oficial válida de 18 caracteres en caso de que no exista o esté vacía,
/// para evitar que el programa SUA rechace el registro del trabajador por CURP ausente.
pub fn sintetizar_curp_minima(
    paterno: &str,
    materno: &str,
    nombres: &str,
    rfc: &str,
    nss: &str,
) -> String {
    let full = format!("{} {} {}", paterno, materno, nombres);
    resolver_curp_valida("", &full, rfc, nss)
}

/// Extrae la fecha de nacimiento (DDMMAAAA) y sexo ('M' o 'F') a partir de la CURP para SUA.
pub fn extraer_datos_curp(curp: &str) -> (String, char) {
    let clean: String = curp.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if clean.len() < 11 {
        return ("01011990".to_string(), 'M');
    }
    let yy_str = &clean[4..6];
    let mm_str = &clean[6..8];
    let dd_str = &clean[8..10];
    let sex_char = clean.chars().nth(10).unwrap_or('H');

    let yy: i32 = yy_str.parse().unwrap_or(90);
    let mm: i32 = mm_str.parse().unwrap_or(1).clamp(1, 12);
    let dd: i32 = dd_str.parse().unwrap_or(1).clamp(1, 31);

    let full_year = if yy <= 26 { 2000 + yy } else { 1900 + yy };
    let fecha_nac = format!("{:02}{:02}{:04}", dd, mm, full_year);
    let sexo_sua = if sex_char == 'M' { 'F' } else { 'M' };

    (fecha_nac, sexo_sua)
}

/// Formatea una fecha a `DDMMAAAA` (8 posiciones).
pub fn format_date_sua(date: Option<NaiveDate>, default_month: u32, default_year: i32) -> String {
    if let Some(d) = date {
        d.format("%d%m%Y").to_string()
    } else {
        let m = default_month.clamp(1, 12);
        let y = if default_year > 1900 { default_year } else { 2024 };
        format!("01{:02}{:04}", m, y)
    }
}

/// Registro normalizado para exportación de trabajador a SUA
#[derive(Debug, Clone)]
pub struct SuaWorkerRecord {
    pub registro_patronal: String,
    pub nss: String,
    pub rfc: String,
    pub curp: String,
    pub nombre_completo: String,
    pub fecha_alta: Option<NaiveDate>,
    pub mes_periodo: u32,
    pub anio_periodo: i32,
    pub sdi: f64,
    pub clave_ubicacion: String,
    pub num_empleado: String,
    pub tipo_trabajador: char, // Siempre '1' según especificación requerida
}

/// Formatea una línea para `1_ASEG_<Folio>.TXT` (Longitud fija estricta oficial SUA: 164 caracteres)
pub fn format_aseg_line(rec: &SuaWorkerRecord) -> String {
    let rp = sanitize_rp(&rec.registro_patronal); // 11
    let nss = sanitize_nss(&rec.nss); // 11

    // Resolver CURP válida (18 posiciones) según la norma oficial de RENAPO
    let curp = resolver_curp_valida(&rec.curp, &rec.nombre_completo, &rec.rfc, &rec.nss); // 18

    // Resolver RFC válido (13 posiciones) para evitar "Error en la configuración del RFC"
    let rfc = resolver_rfc_valido(&rec.rfc, &curp, &rec.nombre_completo, &rec.nss); // 13

    let delim_name = format_sua_name_delimited(&rec.nombre_completo);
    let name_50 = if delim_name.len() > 50 {
        delim_name[..50].to_string()
    } else {
        format!("{:<50}", delim_name)
    }; // 50

    // Constantes requeridas para el SUA:
    // - Tipo de trabajador: siempre clave 1
    // - Tipo de jornada: siempre clave 0
    // - Tipo de descuento: siempre clave 2
    let tipo_trab = '1'; // 1
    let jornada = '0'; // 1
    let fecha = format_date_sua(rec.fecha_alta, rec.mes_periodo, rec.anio_periodo); // 8

    // SDI: 5 enteros + 2 decimales sin punto decimal (7 posiciones)
    let sdi_cents = ((rec.sdi * 100.0).round() as i64).clamp(0, 9999999);
    let sdi_str = format!("{:07}", sdi_cents); // 7

    // Clave de ubicación: omitir si contiene palabras reservadas de estatus ("ALTA", "BAJA", etc.)
    let clave_raw = clean_ascii_sua(&rec.clave_ubicacion);
    let clave_upper = clave_raw.trim().to_ascii_uppercase();
    let clave_filtrada = if clave_upper == "ALTA" || clave_upper == "BAJA" || clave_upper == "REIN" || clave_upper == "MS" || clave_upper == "M/S" {
        String::new()
    } else {
        clave_raw
    };
    let clave_17 = if clave_filtrada.len() > 17 {
        clave_filtrada[..17].to_string()
    } else {
        format!("{:<17}", clave_filtrada)
    }; // 17

    let credito_10 = "          "; // 10 espacios (sin crédito)
    let fid_8 = "00000000"; // 8 ceros (fecha descuento)
    let tipo_desc_1 = '2'; // 1 (tipo descuento: 2)
    let valor_desc_8 = "00000000"; // 8 ceros (valor descuento)

    let line = format!(
        "{}{}{}{}{}{}{}{}{}{}{}{}{}{}",
        rp,
        nss,
        rfc,
        curp,
        name_50,
        tipo_trab,
        jornada,
        fecha,
        sdi_str,
        clave_17,
        credito_10,
        fid_8,
        tipo_desc_1,
        valor_desc_8
    );

    debug_assert_eq!(line.len(), 164, "La línea de ASEG debe medir exactamente 164 caracteres para SUA");
    line
}

/// Formatea una línea para `2_AFIL_<Folio>.TXT` (Datos Afiliatorios SUA - Longitud fija estricta: 80 caracteres)
pub fn format_afil_line(rec: &SuaWorkerRecord) -> String {
    let clean_rp = sanitize_rp(&rec.registro_patronal);
    let rp_base = &clean_rp[..10]; // 10
    let rp_dv = &clean_rp[10..11]; // 1

    let clean_nss = sanitize_nss(&rec.nss);
    let nss_base = &clean_nss[..10]; // 10
    let nss_dv = &clean_nss[10..11]; // 1

    let cp_5 = "64000"; // 5 (Código Postal default de la región)

    // Extraer fecha de nacimiento y sexo desde CURP oficial validada
    let curp = resolver_curp_valida(&rec.curp, &rec.nombre_completo, &rec.rfc, &rec.nss);
    let (fecha_nac_8, sexo_1) = extraer_datos_curp(&curp); // 8 y 1

    // Lugar de nacimiento y clave de entidad según catálogo oficial de SUA
    let (nombre_estado, clave_lugar) = estado_y_clave_lugar_desde_curp(&curp);
    let lugar_nac_25 = format!("{:<25}", nombre_estado); // 25
    let clave_entidad_2 = format!("{:0>2}", clave_lugar); // 2
    let umf_3 = "000"; // 3 dígitos numéricos (el SUA requiere que UMF sea numérica; 000 por omisión)
    let ocupacion_12 = "            "; // 12 espacios
    let tipo_salario_1 = '0'; // 1 (0=Fijo)
    let horas_1 = ' '; // 1 (1 espacio)

    let line = format!(
        "{}{}{}{}{}{}{}{}{}{}{}{}{}",
        rp_base,
        rp_dv,
        nss_base,
        nss_dv,
        cp_5,
        fecha_nac_8,
        lugar_nac_25,
        clave_entidad_2,
        umf_3,
        ocupacion_12,
        sexo_1,
        tipo_salario_1,
        horas_1
    );

    debug_assert_eq!(line.len(), 80, "La línea de AFIL debe medir exactamente 80 caracteres");
    line
}

/// Registro de movimiento para SUA (`MOVT_<Folio>.TXT`)
#[derive(Debug, Clone)]
pub struct SuaMovementRecord {
    pub registro_patronal: String,
    pub nss: String,
    pub tipo_movimiento: String, // "08" Reingreso, "07" Modificación de Salario, "11" Ausentismo, "12" Incapacidad
    pub fecha: Option<NaiveDate>,
    pub mes_periodo: u32,
    pub anio_periodo: i32,
    pub folio_incapacidad: String,
    pub dias_incidencia: i16,
    pub sdi: f64,
}

/// Formatea una línea para `MOVT_<Folio>.TXT` (Longitud fija estricta: 49 caracteres)
pub fn format_movt_line(mov: &SuaMovementRecord) -> String {
    let rp = sanitize_rp(&mov.registro_patronal); // 11
    let nss = sanitize_nss(&mov.nss); // 11
    let tipo = if mov.tipo_movimiento.len() == 2 {
        mov.tipo_movimiento.clone()
    } else {
        format!("{:0>2}", mov.tipo_movimiento)
    }; // 2
    let fecha = format_date_sua(mov.fecha, mov.mes_periodo, mov.anio_periodo); // 8

    let folio_clean = clean_ascii_sua(&mov.folio_incapacidad);
    let folio_8 = if folio_clean.is_empty() {
        "        ".to_string()
    } else if folio_clean.len() > 8 {
        folio_clean[..8].to_string()
    } else {
        format!("{:<8}", folio_clean)
    }; // 8

    let dias_2 = format!("{:02}", mov.dias_incidencia.clamp(0, 99)); // 2
    let sdi_cents = ((mov.sdi * 100.0).round() as i64).clamp(0, 9999999);
    let sdi_7 = format!("{:07}", sdi_cents); // 7

    let line = format!("{}{}{}{}{}{}{}", rp, nss, tipo, fecha, folio_8, dias_2, sdi_7);
    debug_assert_eq!(line.len(), 49, "La línea de MOVT debe medir exactamente 49 caracteres");
    line
}

/// Registro de incapacidad para `INCAP.TXT` (Longitud fija estricta: 49 caracteres)
#[derive(Debug, Clone)]
pub struct SuaIncapRecord {
    pub registro_patronal: String,
    pub nss: String,
    pub fecha: Option<NaiveDate>,
    pub mes_periodo: u32,
    pub anio_periodo: i32,
    pub folio: String,
    pub dias: i16,
    pub sdi: f64,
}

/// Formatea una línea para `INCAP.TXT` (Longitud fija estricta: 49 caracteres)
pub fn format_incap_line(inc: &SuaIncapRecord) -> String {
    let rp = sanitize_rp(&inc.registro_patronal); // 11
    let nss = sanitize_nss(&inc.nss); // 11
    let tipo = "12"; // 2 (Incapacidad)
    let fecha = format_date_sua(inc.fecha, inc.mes_periodo, inc.anio_periodo); // 8

    let folio_clean = clean_ascii_sua(&inc.folio);
    let folio_8 = if folio_clean.is_empty() {
        "IN000001".to_string()
    } else if folio_clean.len() > 8 {
        folio_clean[..8].to_string()
    } else {
        format!("{:<8}", folio_clean)
    }; // 8

    let dias_2 = format!("{:02}", inc.dias.clamp(1, 99)); // 2
    let sdi_cents = ((inc.sdi * 100.0).round() as i64).clamp(0, 9999999);
    let sdi_7 = format!("{:07}", sdi_cents); // 7

    let line = format!("{}{}{}{}{}{}{}", rp, nss, tipo, fecha, folio_8, dias_2, sdi_7);
    debug_assert_eq!(line.len(), 49, "La línea de INCAP debe medir exactamente 49 caracteres");
    line
}

/// Registro de crédito INFONAVIT para `CRED_<Folio>.TXT` (Longitud fija: 51 caracteres)
#[derive(Debug, Clone)]
pub struct SuaCredRecord {
    pub registro_patronal: String,
    pub nss: String,
    pub num_credito: String,
    pub tipo_movimiento: String, // "15" Inicio
    pub fecha: Option<NaiveDate>,
    pub mes_periodo: u32,
    pub anio_periodo: i32,
    pub tipo_descuento: char, // '1' Porcentaje, '2' Cuota Fija, '3' VSM
    pub valor_descuento: f64,
}

/// Formatea una línea para `CRED_<Folio>.TXT` (Longitud fija: 51 caracteres)
pub fn format_cred_line(cred: &SuaCredRecord) -> String {
    let rp = sanitize_rp(&cred.registro_patronal); // 11
    let nss = sanitize_nss(&cred.nss); // 11

    let num_cred_digits: String = cred.num_credito.chars().filter(|c| c.is_ascii_digit()).collect();
    let num_cred_10 = if num_cred_digits.len() >= 10 {
        num_cred_digits[..10].to_string()
    } else {
        format!("{:0>10}", num_cred_digits)
    }; // 10

    let tipo_mov = if cred.tipo_movimiento.len() == 2 {
        cred.tipo_movimiento.clone()
    } else {
        "15".to_string()
    }; // 2

    let fecha = format_date_sua(cred.fecha, cred.mes_periodo, cred.anio_periodo); // 8
    let tipo_desc = if matches!(cred.tipo_descuento, '1' | '2' | '3') {
        cred.tipo_descuento
    } else {
        '2'
    }; // 1

    let val_cents = ((cred.valor_descuento * 100.0).round() as i64).clamp(0, 99999999);
    let val_8 = format!("{:08}", val_cents); // 8

    let line = format!("{}{}{}{}{}{}{}", rp, nss, num_cred_10, tipo_mov, fecha, tipo_desc, val_8);
    debug_assert_eq!(line.len(), 51, "La línea de CRED debe medir exactamente 51 caracteres");
    line
}

/// Resultado del proceso de exportación a archivos SUA IMSS
#[derive(Debug, Clone)]
pub struct SuaExportResult {
    pub folder: std::path::PathBuf,
    pub aseg_filename: String,
    pub aseg_count: usize,
    pub afil_filename: String,
    pub afil_count: usize,
    pub movt_filename: String,
    pub movt_count: usize,
    pub cred_filename: String,
    pub cred_count: usize,
    pub incap_filename: String,
    pub incap_count: usize,
    pub total_workers: usize,
}

/// Genera el paquete completo de 5 archivos SUA con formato estricto y orden numérico para importación:
/// 1. 1_ASEG_<Folio>.TXT (Datos de los Trabajadores / Altas - 168 chars)
///    ¿Por qué va primero?: Es el archivo principal que da de alta o registra la información base
///    de los nuevos trabajadores en el sistema (NSS, nombres, CURP, etc.). El SUA requiere que la ficha
///    del trabajador exista antes de poder asignarle cualquier evento posterior.
/// 2. 2_AFIL_<Folio>.TXT (Datos Afiliatorios Adicionales - 80 chars)
///    ¿Por qué va segundo?: Complementa la información personal de los colaboradores que ya fueron
///    identificados o creados previamente en el sistema (lugar de nacimiento, código postal, tipo de salario, etc.).
/// 3. 3_MOVT_<Folio>.TXT (Movimientos Afiliatorios - 49 chars)
///    ¿Por qué va tercero?: Una vez que el trabajador ya está registrado en la base de datos, se suben
///    sus incidencias contractuales y de estatus, tales como reingresos, modificaciones de salario diario integrado o bajas.
/// 4. 4_CRED_<Folio>.TXT (Amortizaciones de Crédito INFONAVIT / MOVCR - 51 chars)
///    ¿Por qué va cuarto?: Contiene las incidencias relacionadas con los créditos de vivienda de los trabajadores
///    (inicio de retención, suspensión o modificación de descuentos). Requiere que el trabajador ya cuente con
///    un estatus vigente dentro de la empresa en el SUA.
/// 5. 5_INCAP_<Folio>.TXT (Datos de Incapacidades - 49 chars)
///    ¿Por qué va al último?: Registra los folios y días de incapacidades médicas otorgadas por el IMSS.
///    Se sube al final porque el sistema valida de forma estricta que correspondan a un trabajador activo y que
///    no se traslapen con fechas donde el empleado ya se encuentre dado de baja.
pub fn export_sua_files(
    folder_path: &std::path::Path,
    folio_revision: &str,
    rows: &[crate::db::reporte_liquidaciones::ReporteLiquidacionRowDb],
) -> Result<SuaExportResult, String> {
    if rows.is_empty() {
        return Err("No hay registros disponibles para exportar al SUA.".to_string());
    }

    if let Err(e) = std::fs::create_dir_all(folder_path) {
        return Err(format!("No se pudo crear la carpeta de destino: {}", e));
    }

    let clean_folio: String = folio_revision
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let folio = if clean_folio.trim().is_empty() {
        "REV".to_string()
    } else {
        clean_folio
    };

    let aseg_filename = format!("1_ASEG_{}.TXT", folio);
    let afil_filename = format!("2_AFIL_{}.TXT", folio);
    let movt_filename = format!("3_MOVT_{}.TXT", folio);
    let cred_filename = format!("4_CRED_{}.TXT", folio);
    let incap_filename = format!("5_INCAP_{}.TXT", folio);

    // 1. Agrupar filas por trabajador único (RP + NSS)
    let mut workers_map: std::collections::BTreeMap<(String, String), Vec<&crate::db::reporte_liquidaciones::ReporteLiquidacionRowDb>> = std::collections::BTreeMap::new();
    for r in rows {
        let rp = sanitize_rp(&r.registro_patronal);
        let nss = sanitize_nss(&r.nss);
        workers_map.entry((rp, nss)).or_default().push(r);
    }

    let mut workers: Vec<SuaWorkerRecord> = Vec::with_capacity(workers_map.len());
    let mut movements: Vec<SuaMovementRecord> = Vec::new();
    let mut incaps: Vec<SuaIncapRecord> = Vec::new();
    let creds: Vec<SuaCredRecord> = Vec::new();

    let mut incap_counter = 1;

    for ((rp, nss), mut emp_rows) in workers_map {
        // Ordenar cronológicamente las filas del empleado
        emp_rows.sort_by_key(|r| {
            let m = crate::cedulas::business::month_to_num(&r.mes_periodo);
            (r.anio_periodo, m, r.fecha)
        });

        let first = emp_rows[0];

        // Tomar el mejor nombre, RFC y CURP disponibles
        let mut best_nombre = first.nombre_trabajador.clone();
        let mut best_rfc = first.rfc.clone();
        let mut best_curp = first.curp.clone();

        for r in &emp_rows {
            if best_nombre.trim().is_empty() && !r.nombre_trabajador.trim().is_empty() {
                best_nombre = r.nombre_trabajador.clone();
            }
            if best_rfc.trim().is_empty() && !r.rfc.trim().is_empty() {
                best_rfc = r.rfc.clone();
            }
            if best_curp.trim().is_empty() && !r.curp.trim().is_empty() {
                best_curp = r.curp.clone();
            }
        }

        let m_first = crate::cedulas::business::month_to_num(&first.mes_periodo).clamp(1, 12);

        let cl_mov = first.clave.trim().to_ascii_uppercase();
        let es_palabra_mov = cl_mov == "ALTA" || cl_mov == "BAJA" || cl_mov == "REIN" || cl_mov == "MS" || cl_mov == "M/S";
        let ubicacion_final = if es_palabra_mov || cl_mov.is_empty() {
            first.obra_numero.clone()
        } else {
            first.clave.clone()
        };
        let num_emp_final = if es_palabra_mov {
            String::new()
        } else {
            first.clave.clone()
        };

        let curp_resuelto = resolver_curp_valida(&best_curp, &best_nombre, &best_rfc, &nss);
        let rfc_resuelto = resolver_rfc_valido(&best_rfc, &curp_resuelto, &best_nombre, &nss);
        let worker = SuaWorkerRecord {
            registro_patronal: rp.clone(),
            nss: nss.clone(),
            rfc: rfc_resuelto,
            curp: curp_resuelto,
            nombre_completo: best_nombre,
            fecha_alta: first.fecha,
            mes_periodo: m_first,
            anio_periodo: first.anio_periodo as i32,
            sdi: first.sdi,
            clave_ubicacion: ubicacion_final,
            num_empleado: num_emp_final,
            tipo_trabajador: '1',
        };

        // En SUA, los trabajadores vigentes ya son registrados y dados de alta mediante el archivo 1_ASEG.
        // El archivo 3_MOVT es exclusivamente para incidencias y movimientos posteriores:
        // - "02": Baja
        // - "08": Reingreso (únicamente si es posterior a una baja, idx > 0)
        // - "07": Modificación de salario (cuando cambia el SDI respecto al inicial o clave M/S)
        // - "11": Ausentismo / Licencia sin goce de sueldo
        // NO se debe enviar movimiento "08" al trabajador vigente inicial, pues el SUA lo rechaza con:
        // "Trabajador vigente, registre primero el movimiento de baja".
        let mut prev_sdi = first.sdi;
        for (idx, r) in emp_rows.iter().enumerate() {
            let m_curr = crate::cedulas::business::month_to_num(&r.mes_periodo).clamp(1, 12);
            let cl_upper = r.clave.trim().to_ascii_uppercase();

            // 1. Baja ("02") si está indicada explícitamente en la fila
            if cl_upper.contains("BAJA") || cl_upper == "02" {
                movements.push(SuaMovementRecord {
                    registro_patronal: rp.clone(),
                    nss: nss.clone(),
                    tipo_movimiento: "02".to_string(),
                    fecha: r.fecha,
                    mes_periodo: m_curr,
                    anio_periodo: r.anio_periodo as i32,
                    folio_incapacidad: String::new(),
                    dias_incidencia: 0,
                    sdi: r.sdi,
                });
            }
            // 2. Reingreso ("08") SOLO para movimientos posteriores (idx > 0) que indiquen reingreso explícito
            else if idx > 0 && (cl_upper.contains("REIN") || cl_upper == "08") {
                movements.push(SuaMovementRecord {
                    registro_patronal: rp.clone(),
                    nss: nss.clone(),
                    tipo_movimiento: "08".to_string(),
                    fecha: r.fecha,
                    mes_periodo: m_curr,
                    anio_periodo: r.anio_periodo as i32,
                    folio_incapacidad: String::new(),
                    dias_incidencia: 0,
                    sdi: r.sdi,
                });
                prev_sdi = r.sdi;
            }
            // 3. Modificación de salario ("07"):
            // En SUA, una modificación de salario REQUIERE estrictamente que el nuevo SDI sea DIFERENTE al anterior.
            // Si el SDI es igual al previo ((r.sdi - prev_sdi).abs() <= 0.01), el SUA rechaza el movimiento con:
            // "El Salario Diario Integrado no puede ser igual al anterior".
            else if idx > 0 && (r.sdi - prev_sdi).abs() > 0.01 && r.sdi > 0.0 {
                movements.push(SuaMovementRecord {
                    registro_patronal: rp.clone(),
                    nss: nss.clone(),
                    tipo_movimiento: "07".to_string(),
                    fecha: r.fecha,
                    mes_periodo: m_curr,
                    anio_periodo: r.anio_periodo as i32,
                    folio_incapacidad: String::new(),
                    dias_incidencia: 0,
                    sdi: r.sdi,
                });
                prev_sdi = r.sdi;
            } else if r.sdi > 0.0 {
                prev_sdi = r.sdi;
            }

            // 4. Ausentismo ("11")
            let aus_dias = r.aus.round() as i16;
            if aus_dias > 0 {
                movements.push(SuaMovementRecord {
                    registro_patronal: rp.clone(),
                    nss: nss.clone(),
                    tipo_movimiento: "11".to_string(),
                    fecha: r.fecha,
                    mes_periodo: m_curr,
                    anio_periodo: r.anio_periodo as i32,
                    folio_incapacidad: String::new(),
                    dias_incidencia: aus_dias,
                    sdi: r.sdi,
                });
            }

            // 5. Licencia / Ausentismo ("11")
            let lic_dias = r.lic.round() as i16;
            if lic_dias > 0 {
                movements.push(SuaMovementRecord {
                    registro_patronal: rp.clone(),
                    nss: nss.clone(),
                    tipo_movimiento: "11".to_string(),
                    fecha: r.fecha,
                    mes_periodo: m_curr,
                    anio_periodo: r.anio_periodo as i32,
                    folio_incapacidad: String::new(),
                    dias_incidencia: lic_dias,
                    sdi: r.sdi,
                });
            }

            // 6. Incapacidad ("12")
            let inc_dias = r.inc.round() as i16;
            if inc_dias > 0 {
                incaps.push(SuaIncapRecord {
                    registro_patronal: rp.clone(),
                    nss: nss.clone(),
                    fecha: r.fecha,
                    mes_periodo: m_curr,
                    anio_periodo: r.anio_periodo as i32,
                    folio: format!("IN{:06}", incap_counter),
                    dias: inc_dias,
                    sdi: r.sdi,
                });
                incap_counter += 1;
            }
        }

        workers.push(worker);
    }

    // Ordenar trabajadores alfabéticamente
    workers.sort_by(|a, b| a.nombre_completo.cmp(&b.nombre_completo));

    // 2. Generar contenidos con terminación CRLF (\r\n) estricta para SUA
    let mut aseg_content = String::new();
    for w in &workers {
        aseg_content.push_str(&format_aseg_line(w));
        aseg_content.push_str("\r\n");
    }

    let mut afil_content = String::new();
    for w in &workers {
        afil_content.push_str(&format_afil_line(w));
        afil_content.push_str("\r\n");
    }

    let mut movt_content = String::new();
    let mut seen_mov_lines = std::collections::HashSet::new();
    let mut unique_movt_count = 0;
    for m in &movements {
        let line = format_movt_line(m);
        if seen_mov_lines.insert(line.clone()) {
            movt_content.push_str(&line);
            movt_content.push_str("\r\n");
            unique_movt_count += 1;
        }
    }

    let mut cred_content = String::new();
    for c in &creds {
        cred_content.push_str(&format_cred_line(c));
        cred_content.push_str("\r\n");
    }

    let mut incap_content = String::new();
    for i in &incaps {
        incap_content.push_str(&format_incap_line(i));
        incap_content.push_str("\r\n");
    }

    // 3. Escribir archivos al disco
    let aseg_path = folder_path.join(&aseg_filename);
    let afil_path = folder_path.join(&afil_filename);
    let movt_path = folder_path.join(&movt_filename);
    let cred_path = folder_path.join(&cred_filename);
    let incap_path = folder_path.join(&incap_filename);

    std::fs::write(&aseg_path, aseg_content.as_bytes())
        .map_err(|e| format!("Error guardando {}: {}", aseg_filename, e))?;
    std::fs::write(&afil_path, afil_content.as_bytes())
        .map_err(|e| format!("Error guardando {}: {}", afil_filename, e))?;
    std::fs::write(&movt_path, movt_content.as_bytes())
        .map_err(|e| format!("Error guardando {}: {}", movt_filename, e))?;
    std::fs::write(&cred_path, cred_content.as_bytes())
        .map_err(|e| format!("Error guardando {}: {}", cred_filename, e))?;
    std::fs::write(&incap_path, incap_content.as_bytes())
        .map_err(|e| format!("Error guardando {}: {}", incap_filename, e))?;

    Ok(SuaExportResult {
        folder: folder_path.to_path_buf(),
        aseg_filename,
        aseg_count: workers.len(),
        afil_filename,
        afil_count: workers.len(),
        movt_filename,
        movt_count: unique_movt_count,
        cred_filename,
        cred_count: creds.len(),
        incap_filename,
        incap_count: incaps.len(),
        total_workers: workers.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn test_clean_ascii_sua() {
        assert_eq!(clean_ascii_sua("MARTÍN PEÑA"), "MARTIN PE/A");
        assert_eq!(clean_ascii_sua("José Ángel González"), "JOSE ANGEL GONZALEZ");
        assert_eq!(clean_ascii_sua("GÜEREÑA"), "GUERE/A");
    }

    #[test]
    fn test_parse_mexican_name() {
        let (p, m, n) = parse_mexican_name("GONZALEZ PEREZ JUAN CARLOS");
        assert_eq!(p, "GONZALEZ");
        assert_eq!(m, "PEREZ");
        assert_eq!(n, "JUAN CARLOS");

        let (p, m, n) = parse_mexican_name("DE LA CRUZ HERNANDEZ JOSE");
        assert_eq!(p, "DE LA CRUZ");
        assert_eq!(m, "HERNANDEZ");
        assert_eq!(n, "JOSE");

        let (p, m, n) = parse_mexican_name("PEREZ JUAN");
        assert_eq!(p, "PEREZ");
        assert_eq!(m, "");
        assert_eq!(n, "JUAN");
    }

    #[test]
    fn test_format_aseg_line_exact_164() {
        let rec = SuaWorkerRecord {
            registro_patronal: "Y6312345101".into(),
            nss: "12345678901".into(),
            rfc: "ROAE800101XYZ".into(),
            curp: "ROAE800101HDFRNR01".into(),
            nombre_completo: "RODRIGUEZ ALVAREZ ENRIQUE".into(),
            fecha_alta: Some(NaiveDate::from_ymd_opt(2024, 9, 2).unwrap()),
            mes_periodo: 9,
            anio_periodo: 2024,
            sdi: 348.50,
            clave_ubicacion: "OBRA-C01".into(),
            num_empleado: "EMP001".into(),
            tipo_trabajador: '1',
        };

        let line = format_aseg_line(&rec);
        assert_eq!(line.len(), 164, "Debe ser de exactamente 164 caracteres para SUA");
        assert!(line.starts_with("Y631234510112345678901ROAE800101XYZROAE800101HDFRNR01"));
        assert!(line.contains("02092024"));
        assert!(line.contains("0034850"));
        // Comprobar tipo trabajador '1', jornada '0' y tipo descuento '2'
        assert_eq!(&line[103..105], "10"); // tipo trab '1' + jornada '0'
        assert_eq!(&line[155..156], "2"); // tipo descuento '2'
        assert!(line.ends_with("200000000")); // tipo desc '2' + 8 ceros
    }

    #[test]
    fn test_format_aseg_line_curp_en_blanco_si_no_se_determina() {
        // Caso: CURP vacía y movimiento ALTA como clave -> CURP en blanco (18 espacios) y RFC genérico XAXX010101000
        let rec = SuaWorkerRecord {
            registro_patronal: "A3673455103".into(),
            nss: "32088956753".into(),
            rfc: "XAXX010101000".into(),
            curp: "".into(),
            nombre_completo: "CASTRO ENCINA JOSE HUMBERTO".into(),
            fecha_alta: Some(NaiveDate::from_ymd_opt(2024, 9, 24).unwrap()),
            mes_periodo: 9,
            anio_periodo: 2024,
            sdi: 261.20,
            clave_ubicacion: "ALTA".into(),
            num_empleado: "ALTA".into(),
            tipo_trabajador: '1',
        };

        let line = format_aseg_line(&rec);
        assert_eq!(line.len(), 164, "Debe ser de exactamente 164 caracteres");
        // Debe contener 18 espacios de CURP en blanco al no poder determinarse
        assert!(line.contains("                  "));
        // Debe contener el RFC genérico aceptado por el SUA
        assert!(line.contains("XAXX010101000"));
        // La clave 'ALTA' no debe contaminar el campo de ubicación
        assert!(!line.contains("ALTA"));
        // Termina con tipo descuento '2' y 8 ceros
        assert!(line.ends_with("200000000"));
    }

    #[test]
    fn test_resolver_rfc_valido() {
        // Empleado con RFC vacío pero CURP válida: genera RFC oficial SAT de 13 caracteres
        let rfc1 = resolver_rfc_valido("", "AOJD821108HCLCRN06", "ACOSTA JUAREZ DANIEL", "32998253382");
        assert_eq!(rfc1.len(), 13);
        assert!(rfc1.starts_with("AOJD821108"));

        // Empleado sin CURP: genera RFC genérico XAXX010101000 aceptado por el SUA
        let rfc2 = resolver_rfc_valido("", "", "ACOSTA JUAREZ DANIEL", "32998253382");
        assert_eq!(rfc2, "XAXX010101000");

        // Empleado con RFC completo de 13 caracteres existente
        let rfc3 = resolver_rfc_valido("AOJD821108ABC", "AOJD821108HCLCRN06", "ACOSTA JUAREZ DANIEL", "32998253382");
        assert_eq!(rfc3, "AOJD821108ABC");
    }

    #[test]
    fn test_estado_y_clave_lugar_catalogo() {
        // Coahuila
        let (edo, cve) = estado_y_clave_lugar_desde_curp("AOJD821108HCLCRN06");
        assert_eq!(edo, "COAHUILA");
        assert_eq!(cve, "05");

        // CDMX
        let (edo, cve) = estado_y_clave_lugar_desde_curp("CASM830208MDFSNR07");
        assert_eq!(edo, "CIUDAD DE MEXICO");
        assert_eq!(cve, "09");

        // Zacatecas
        let (edo, cve) = estado_y_clave_lugar_desde_curp("COTG930602HZSNRD02");
        assert_eq!(edo, "ZACATECAS");
        assert_eq!(cve, "32");

        // Aguascalientes
        let (edo, cve) = estado_y_clave_lugar_desde_curp("EACG990219HASSMR03");
        assert_eq!(edo, "AGUASCALIENTES");
        assert_eq!(cve, "01");

        // Extranjero
        let (edo, cve) = estado_y_clave_lugar_desde_curp("RUJA910101HNEXXX00");
        assert_eq!(edo, "EXTRANJERO");
        assert_eq!(cve, "33");
    }

    #[test]
    fn test_format_afil_line_exact_80() {
        let rec = SuaWorkerRecord {
            registro_patronal: "Y6312345101".into(),
            nss: "12345678901".into(),
            rfc: "ROAE800101XYZ".into(),
            curp: "ROAE800101HDFRNR01".into(),
            nombre_completo: "RODRIGUEZ ALVAREZ ENRIQUE".into(),
            fecha_alta: Some(NaiveDate::from_ymd_opt(2024, 9, 2).unwrap()),
            mes_periodo: 9,
            anio_periodo: 2024,
            sdi: 348.50,
            clave_ubicacion: "OBRA-C01".into(),
            num_empleado: "EMP001".into(),
            tipo_trabajador: '1',
        };

        let line = format_afil_line(&rec);
        assert_eq!(line.len(), 80, "Debe ser de exactamente 80 caracteres para SUA");
        assert!(line.starts_with("Y63123451011234567890164000"));
        assert!(line.contains("CIUDAD DE MEXICO"));
        assert!(line.contains("09")); // Clave lugar 09 para DF
    }

    #[test]
    fn test_format_movt_line_exact_49() {
        let mov = SuaMovementRecord {
            registro_patronal: "Y6312345101".into(),
            nss: "12345678901".into(),
            tipo_movimiento: "08".into(),
            fecha: Some(NaiveDate::from_ymd_opt(2024, 9, 2).unwrap()),
            mes_periodo: 9,
            anio_periodo: 2024,
            folio_incapacidad: "".into(),
            dias_incidencia: 0,
            sdi: 348.50,
        };

        let line = format_movt_line(&mov);
        assert_eq!(line.len(), 49, "Debe ser de exactamente 49 caracteres");
    }

    #[test]
    fn test_format_incap_line_exact_49() {
        let inc = SuaIncapRecord {
            registro_patronal: "Y6312345101".into(),
            nss: "12345678901".into(),
            fecha: Some(NaiveDate::from_ymd_opt(2024, 9, 10).unwrap()),
            mes_periodo: 9,
            anio_periodo: 2024,
            folio: "IN123456".into(),
            dias: 5,
            sdi: 348.50,
        };

        let line = format_incap_line(&inc);
        assert_eq!(line.len(), 49, "Debe ser de exactamente 49 caracteres");
        assert!(line.contains("IN123456"));
        assert!(line.contains("05"));
    }

    #[test]
    fn test_format_cred_line_exact_51() {
        let cred = SuaCredRecord {
            registro_patronal: "Y6312345101".into(),
            nss: "12345678901".into(),
            num_credito: "0812345678".into(),
            tipo_movimiento: "15".into(),
            fecha: Some(NaiveDate::from_ymd_opt(2024, 9, 1).unwrap()),
            mes_periodo: 9,
            anio_periodo: 2024,
            tipo_descuento: '1',
            valor_descuento: 25.0,
        };

        let line = format_cred_line(&cred);
        assert_eq!(line.len(), 51, "Debe ser de exactamente 51 caracteres");
    }

    fn make_test_row(
        rp: &str,
        nss: &str,
        nombre: &str,
        mes: &str,
        anio: i16,
        fecha: Option<NaiveDate>,
        sdi: f64,
        clave: &str,
    ) -> crate::db::reporte_liquidaciones::ReporteLiquidacionRowDb {
        crate::db::reporte_liquidaciones::ReporteLiquidacionRowDb {
            id_reporte_liq: uuid::Uuid::now_v7(),
            id_parent_reporte_liq: uuid::Uuid::now_v7(),
            id_liquidacion: uuid::Uuid::now_v7(),
            id_empleado: uuid::Uuid::now_v7(),
            nombre_trabajador: nombre.into(),
            rfc: "PEJU800101XYZ".into(),
            curp: "PEJU800101HDFRNR01".into(),
            nss: nss.into(),
            registro_patronal: rp.into(),
            mes_periodo: mes.into(),
            anio_periodo: anio,
            periodo_str: format!("{}/{}", mes, anio),
            dias_cedula: 30,
            id_registro_obra: None,
            obra_numero: "OBRA1".into(),
            obra_label: "OBRA 1".into(),
            dias_obra: 30,
            obra_creado_usuario: false,
            clave: clave.into(),
            fecha,
            fecha_str: fecha.map(|d| d.to_string()).unwrap_or_default(),
            sdi,
            lic: 0.0,
            inc: 0.0,
            aus: 0.0,
            cf: 0.0,
            exc_pat: 0.0,
            exc_obr: 0.0,
            pd_pat: 0.0,
            pd_obr: 0.0,
            gmp_pat: 0.0,
            gmp_obr: 0.0,
            rt: 0.0,
            iv_pat: 0.0,
            iv_obr: 0.0,
            gps: 0.0,
            patronal: 800.0,
            obrera: 200.0,
            subtotal: 1000.0,
            cf_orig: 0.0,
            exc_pat_orig: 0.0,
            exc_obr_orig: 0.0,
            pd_pat_orig: 0.0,
            pd_obr_orig: 0.0,
            gmp_pat_orig: 0.0,
            gmp_obr_orig: 0.0,
            rt_orig: 0.0,
            iv_pat_orig: 0.0,
            iv_obr_orig: 0.0,
            gps_orig: 0.0,
            patronal_orig: 800.0,
            obrera_orig: 200.0,
            subtotal_orig: 1000.0,
        }
    }

    #[test]
    fn test_export_sua_filenames_order() {
        let temp_dir = std::env::temp_dir().join(format!("sua_test_{}", uuid::Uuid::now_v7()));
        let mut row = make_test_row(
            "Y6312345101",
            "12345678901",
            "PEREZ JUAN",
            "09",
            2024,
            Some(NaiveDate::from_ymd_opt(2024, 9, 1).unwrap()),
            350.0,
            "01",
        );
        row.inc = 1.0;

        let res = export_sua_files(&temp_dir, "REV1", &[row]).expect("export should succeed");
        assert_eq!(res.aseg_filename, "1_ASEG_REV1.TXT");
        assert_eq!(res.afil_filename, "2_AFIL_REV1.TXT");
        assert_eq!(res.movt_filename, "3_MOVT_REV1.TXT");
        assert_eq!(res.cred_filename, "4_CRED_REV1.TXT");
        assert_eq!(res.incap_filename, "5_INCAP_REV1.TXT");

        assert!(temp_dir.join("1_ASEG_REV1.TXT").exists());
        assert!(temp_dir.join("2_AFIL_REV1.TXT").exists());
        assert!(temp_dir.join("3_MOVT_REV1.TXT").exists());
        assert!(temp_dir.join("4_CRED_REV1.TXT").exists());
        assert!(temp_dir.join("5_INCAP_REV1.TXT").exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_trabajador_vigente_no_genera_movimiento_08_espurio() {
        let temp_dir = std::env::temp_dir().join(format!("sua_test_vigente_{}", uuid::Uuid::now_v7()));
        let row = make_test_row(
            "A3673455103",
            "01725406910",
            "PEREZ JUAN",
            "09",
            2024,
            Some(NaiveDate::from_ymd_opt(2024, 9, 3).unwrap()),
            261.20,
            "01",
        );

        let res = export_sua_files(&temp_dir, "REV1", &[row]).expect("export should succeed");
        // No debe generar movimiento 08 para un trabajador vigente sin incidencias
        assert_eq!(res.movt_count, 0, "No debe haber movimientos para un trabajador vigente sin incidencias");

        let movt_file = temp_dir.join(&res.movt_filename);
        let movt_content = std::fs::read_to_string(&movt_file).unwrap();
        assert!(movt_content.is_empty(), "El archivo MOVT debe estar vacío si no hay incidencias");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_trabajador_con_baja_y_modificacion_salario() {
        let temp_dir = std::env::temp_dir().join(format!("sua_test_movs_{}", uuid::Uuid::now_v7()));
        let row1 = make_test_row(
            "A3673455103",
            "01725406910",
            "PEREZ JUAN",
            "08",
            2024,
            Some(NaiveDate::from_ymd_opt(2024, 8, 1).unwrap()),
            261.20,
            "01",
        );

        let mut row2 = row1.clone();
        row2.mes_periodo = "09".into();
        row2.fecha = Some(NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        row2.sdi = 310.50; // Cambio de salario

        let mut row3 = row1.clone();
        row3.mes_periodo = "09".into();
        row3.fecha = Some(NaiveDate::from_ymd_opt(2024, 9, 15).unwrap());
        row3.clave = "BAJA".into(); // Movimiento de baja

        let res = export_sua_files(&temp_dir, "REV2", &[row1, row2, row3]).expect("export should succeed");
        assert_eq!(res.movt_count, 2, "Debe registrar modificación de salario (07) y baja (02)");

        let movt_file = temp_dir.join(&res.movt_filename);
        let movt_content = std::fs::read_to_string(&movt_file).unwrap();
        assert!(movt_content.contains("07"), "Debe contener el tipo de movimiento 07");
        assert!(movt_content.contains("02"), "Debe contener el tipo de movimiento 02");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_trabajador_con_mismo_sdi_no_genera_movimiento_07() {
        let temp_dir = std::env::temp_dir().join(format!("sua_test_sdi_igual_{}", uuid::Uuid::now_v7()));
        let row1 = make_test_row(
            "A3673455103",
            "05239919045",
            "PEREZ JUAN",
            "03",
            2024,
            Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()),
            261.20,
            "01",
        );

        let mut row2 = row1.clone();
        row2.mes_periodo = "05".into();
        row2.fecha = Some(NaiveDate::from_ymd_opt(2024, 5, 1).unwrap());
        row2.sdi = 261.20; // Mismo salario que el anterior
        row2.clave = "07".into(); // Clave 07 o M/S en la cédula, pero salario idéntico

        let res = export_sua_files(&temp_dir, "REV3", &[row1, row2]).expect("export should succeed");
        // No debe generar movimiento 07 porque el salario no cambió (evita error SUA: "El Salario Diario Integrado no puede ser igual al anterior")
        assert_eq!(res.movt_count, 0, "No debe haber movimiento 07 si el salario es igual");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
