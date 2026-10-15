//! Módulo de validaciones fiscales y de seguridad social mexicana
//! Valida RFC (SAT), CURP (RENAPO), NSS (IMSS) y sus consistencias cruzadas.

pub const RENAPO_ENTIDADES: &[&str] = &[
    "AS", "BC", "BS", "CC", "CL", "CM", "CS", "CH", "DF", "DG", "GT", "GR", "HG", "JC", "MC",
    "MN", "MS", "NT", "NL", "OC", "PL", "QT", "QR", "SP", "SL", "SR", "TC", "TS", "TL", "VZ",
    "YN", "ZS", "NE",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RfcInfo {
    pub clean: String,
    pub is_moral: bool,
    pub birth_yy: String,
    pub birth_mm: String,
    pub birth_dd: String,
    pub initials: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurpInfo {
    pub clean: String,
    pub initials: String,
    pub birth_yy: String,
    pub birth_mm: String,
    pub birth_dd: String,
    pub gender: char,
    pub state_code: String,
    pub check_digit: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NssInfo {
    pub clean: String,
    pub subdelegacion: String,
    pub anio_afiliacion: String,
    pub anio_nacimiento: String,
    pub consecutivo: String,
    pub check_digit: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistroPatronalInfo {
    pub clean: String,
    pub letra: char,
    pub subdelegacion: String,
    pub consecutivo: String,
    pub modalidad: String,
    pub check_digit: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistroObraTipo {
    Siroc,
    Satic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistroObraInfo {
    pub clean: String,
    pub tipo: RegistroObraTipo,
}

/// Limpia la entrada: elimina espacios, guiones, puntos y convierte a mayúsculas
pub fn clean_alphanumeric_input(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

/// Identificadores provisionales que el SAT asigna a personas SIN RFC/CURP propio
/// (residentes en el extranjero, datos incompletos, etc.).
///
/// Varias personas distintas comparten el mismo valor (por ejemplo `XAXX010101000`),
/// por lo que nunca deben usarse para emparejar un empleado con un registro existente:
/// hacerlo fusiona personas diferentes en un mismo registro y arrastra sus liquidaciones
/// (y sus registros de obra) al del primero encontrado.
pub fn is_generic_sat_identifier(value: &str) -> bool {
    let clean = clean_alphanumeric_input(value);
    matches!(
        clean.as_str(),
        // RFC genéricos para persona física y moral sin RFC propio
        "XAXX010101000"
            | "XXXX010101XXX"
            | "XEXX010101000"
            | "XEXX010101XXX"
            | "XAXA010101000"
            | "XXXX010101000"
            // CURP genérica para residentes en el extranjero
            | "XEXX010101HNEXXXA4"
            | "XXXX010101XXXXXX"
    )
}

/// Verifica si un año de 2 dígitos es bisiesto (considerando siglo 1900-2099)
fn is_leap_year(yy: u32) -> bool {
    // Si yy es divisible por 4, en el rango 1901-2099 es bisiesto (2000 fue bisiesto)
    yy % 4 == 0
}

/// Valida si una fecha YYMMDD es válida en el calendario
pub fn is_valid_yymmdd(yy: u32, mm: u32, dd: u32) -> bool {
    if !(1..=12).contains(&mm) || dd == 0 {
        return false;
    }
    let max_days = match mm {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(yy) {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    dd <= max_days
}

/// Valida un RFC (SAT)
/// - Si `expected_tipo` es `Some("MORAL")`, exige 12 caracteres.
/// - Si `expected_tipo` es `Some("FISICA")`, exige 13 caracteres.
pub fn validate_rfc(rfc: &str, expected_tipo: Option<&str>) -> Result<RfcInfo, String> {
    let clean = clean_alphanumeric_input(rfc);
    if clean.is_empty() {
        return Err("El RFC no puede estar vacío.".to_string());
    }

    let len = clean.chars().count();
    if len != 12 && len != 13 {
        return Err(format!(
            "RFC debe tener 12 (Moral) o 13 (Física) caracteres (se recibieron {}).",
            len
        ));
    }

    let is_moral = len == 12;

    if let Some(tp) = expected_tipo {
        let tp_up = tp.trim().to_uppercase();
        if (tp_up.contains("MORAL")) && !is_moral {
            return Err("RFC Moral debe tener 12 caracteres (tiene 13).".to_string());
        } else if (tp_up.contains("FISICA") || tp_up.contains("FÍSICA")) && is_moral {
            return Err("RFC Físico debe tener 13 caracteres (tiene 12).".to_string());
        }
    }

    // RFCs genéricos del SAT
    if clean == "XAXX010101000" || clean == "XEXX010101000" {
        return Ok(RfcInfo {
            clean,
            is_moral,
            birth_yy: "01".to_string(),
            birth_mm: "01".to_string(),
            birth_dd: "01".to_string(),
            initials: if is_moral { "XAX".to_string() } else { "XAXX".to_string() },
        });
    }

    let chars: Vec<char> = clean.chars().collect();

    if is_moral {
        // 3 letras iniciales
        for (i, &c) in chars.iter().take(3).enumerate() {
            if !c.is_ascii_alphabetic() && c != '&' && c != 'Ñ' {
                return Err(format!(
                    "Los primeros 3 caracteres del RFC moral deben ser letras (error en posición {}: '{}').",
                    i + 1, c
                ));
            }
        }
        let date_str: String = chars[3..9].iter().collect();
        let yy: u32 = date_str[0..2].parse().map_err(|_| "Año inválido en RFC".to_string())?;
        let mm: u32 = date_str[2..4].parse().map_err(|_| "Mes inválido en RFC".to_string())?;
        let dd: u32 = date_str[4..6].parse().map_err(|_| "Día inválido en RFC".to_string())?;

        if !is_valid_yymmdd(yy, mm, dd) {
            return Err(format!(
                "La fecha de constitución en el RFC ('{}') no es válida (Año: {:02}, Mes: {:02}, Día: {:02}).",
                date_str, yy, mm, dd
            ));
        }

        // 3 caracteres alfanuméricos de homoclave
        for (i, &c) in chars.iter().skip(9).take(3).enumerate() {
            if !c.is_ascii_alphanumeric() {
                return Err(format!(
                    "La homoclave del RFC moral debe ser alfanumérica (error en posición {}: '{}').",
                    i + 10, c
                ));
            }
        }

        Ok(RfcInfo {
            clean,
            is_moral: true,
            birth_yy: format!("{:02}", yy),
            birth_mm: format!("{:02}", mm),
            birth_dd: format!("{:02}", dd),
            initials: chars[0..3].iter().collect(),
        })
    } else {
        // 4 letras iniciales
        for (i, &c) in chars.iter().take(4).enumerate() {
            if !c.is_ascii_alphabetic() && c != '&' && c != 'Ñ' {
                return Err(format!(
                    "Los primeros 4 caracteres del RFC físico deben ser letras (error en posición {}: '{}').",
                    i + 1, c
                ));
            }
        }
        let date_str: String = chars[4..10].iter().collect();
        let yy: u32 = date_str[0..2].parse().map_err(|_| "Año inválido en RFC".to_string())?;
        let mm: u32 = date_str[2..4].parse().map_err(|_| "Mes inválido en RFC".to_string())?;
        let dd: u32 = date_str[4..6].parse().map_err(|_| "Día inválido en RFC".to_string())?;

        if !is_valid_yymmdd(yy, mm, dd) {
            return Err(format!(
                "La fecha de nacimiento en el RFC ('{}') no es válida (Año: {:02}, Mes: {:02}, Día: {:02}).",
                date_str, yy, mm, dd
            ));
        }

        // 3 caracteres alfanuméricos de homoclave
        for (i, &c) in chars.iter().skip(10).take(3).enumerate() {
            if !c.is_ascii_alphanumeric() {
                return Err(format!(
                    "La homoclave del RFC debe ser alfanumérica (error en posición {}: '{}').",
                    i + 11, c
                ));
            }
        }

        Ok(RfcInfo {
            clean,
            is_moral: false,
            birth_yy: format!("{:02}", yy),
            birth_mm: format!("{:02}", mm),
            birth_dd: format!("{:02}", dd),
            initials: chars[0..4].iter().collect(),
        })
    }
}

/// Valida una CURP (RENAPO - 18 posiciones)
pub fn validate_curp(curp: &str) -> Result<CurpInfo, String> {
    let clean = clean_alphanumeric_input(curp);
    if clean.is_empty() {
        return Err("La CURP no puede estar vacía.".to_string());
    }

    let len = clean.chars().count();
    if len != 18 {
        return Err(format!(
            "La CURP debe tener exactamente 18 posiciones. Se recibieron {} caracteres: '{}'.",
            len, clean
        ));
    }

    let chars: Vec<char> = clean.chars().collect();

    // 1-4: Cuatro letras
    for (i, &c) in chars.iter().take(4).enumerate() {
        if !c.is_ascii_alphabetic() {
            return Err(format!(
                "Las primeras 4 posiciones de la CURP deben ser letras (error en posición {}: '{}').",
                i + 1, c
            ));
        }
    }

    // 5-10: Fecha YYMMDD
    let date_str: String = chars[4..10].iter().collect();
    let yy: u32 = date_str[0..2].parse().map_err(|_| "Año inválido en CURP".to_string())?;
    let mm: u32 = date_str[2..4].parse().map_err(|_| "Mes inválido en CURP".to_string())?;
    let dd: u32 = date_str[4..6].parse().map_err(|_| "Día inválido en CURP".to_string())?;

    if !is_valid_yymmdd(yy, mm, dd) {
        return Err(format!(
            "La fecha de nacimiento en la CURP ('{}') no es una fecha válida en el calendario.",
            date_str
        ));
    }

    // 11: Sexo (H: Hombre, M: Mujer, X: No binario)
    let gender = chars[10];
    if gender != 'H' && gender != 'M' && gender != 'X' {
        return Err(format!(
            "El indicador de sexo en la CURP (posición 11) debe ser 'H', 'M' o 'X'. Se recibió '{}'.",
            gender
        ));
    }

    // 12-13: Entidad Federativa (clave de 2 letras según RENAPO)
    let state_code: String = chars[11..13].iter().collect();
    if !RENAPO_ENTIDADES.contains(&state_code.as_str()) {
        return Err(format!(
            "La clave de entidad federativa '{}' en la CURP (posiciones 12-13) no es válida según el catálogo de RENAPO.",
            state_code
        ));
    }

    // 14-16: Tres consonantes internas
    let vowels = ['A', 'E', 'I', 'O', 'U'];
    for (i, &c) in chars.iter().skip(13).take(3).enumerate() {
        if !c.is_ascii_alphabetic() || vowels.contains(&c) {
            return Err(format!(
                "La posición {} de la CURP debe ser una consonante interna válida. Se recibió '{}'.",
                i + 14, c
            ));
        }
    }

    // 17: Diferenciador de siglo (0-9 para siglo XX, A-Z para siglo XXI)
    if !chars[16].is_ascii_alphanumeric() {
        return Err(format!(
            "La posición 17 de la CURP debe ser alfanumérica. Se recibió '{}'.",
            chars[16]
        ));
    }

    // 18: Dígito verificador RENAPO (dígito 0-9)
    if !chars[17].is_ascii_digit() {
        return Err(format!(
            "El dígito verificador final de la CURP (posición 18) debe ser un número. Se recibió '{}'.",
            chars[17]
        ));
    }

    Ok(CurpInfo {
        clean,
        initials: chars[0..4].iter().collect(),
        birth_yy: format!("{:02}", yy),
        birth_mm: format!("{:02}", mm),
        birth_dd: format!("{:02}", dd),
        gender,
        state_code,
        check_digit: chars[17],
    })
}

/// Valida un Número de Seguridad Social del IMSS (NSS - 11 dígitos numéricos)
/// Incluye validación de estructura y algoritmo Luhn Modulo 10 oficial del IMSS.
pub fn validate_nss(nss: &str) -> Result<NssInfo, String> {
    let clean = clean_alphanumeric_input(nss);
    if clean.is_empty() {
        return Err("El NSS no puede estar vacío.".to_string());
    }

    let len = clean.chars().count();
    if len != 11 {
        return Err(format!(
            "El NSS del IMSS debe tener exactamente 11 dígitos numéricos. Se recibieron {} caracteres: '{}'.",
            len, clean
        ));
    }

    if !clean.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "El NSS debe contener únicamente números del 0 al 9. Se recibió '{}'.",
            clean
        ));
    }

    let digits: Vec<u32> = clean
        .chars()
        .map(|c| c.to_digit(10).unwrap_or(0))
        .collect();

    // 1-2: Subdelegación (01 - 99)
    let subdel_val = digits[0] * 10 + digits[1];
    if subdel_val == 0 {
        return Err("La subdelegación del IMSS en el NSS no puede ser '00'.".to_string());
    }

    let subdelegacion = format!("{:02}", subdel_val);
    let anio_afiliacion = format!("{}{}", digits[2], digits[3]);
    let anio_nacimiento = format!("{}{}", digits[4], digits[5]);
    let consecutivo = format!("{}{}{}{}", digits[6], digits[7], digits[8], digits[9]);
    let check_digit_char = clean.chars().nth(10).unwrap_or('0');

    // 11: Dígito verificador IMSS (Algoritmo Luhn Ponderado 1-2)
    let multipliers = [1, 2, 1, 2, 1, 2, 1, 2, 1, 2];
    let mut total_sum = 0;
    for i in 0..10 {
        let prod = digits[i] * multipliers[i];
        let sum_digits = (prod / 10) + (prod % 10);
        total_sum += sum_digits;
    }

    let expected_check = (10 - (total_sum % 10)) % 10;
    let actual_check = digits[10];

    if actual_check != expected_check {
        return Err(format!(
            "Dígito verificador de NSS inválido: el NSS '{}' tiene dígito '{}', pero según el algoritmo IMSS le corresponde '{}'.",
            clean, actual_check, expected_check
        ));
    }

    // Coherencia lógica: Año de afiliación no puede ser anterior al año de nacimiento
    let a_afil: u32 = anio_afiliacion.parse().unwrap_or(0);
    let a_nac: u32 = anio_nacimiento.parse().unwrap_or(0);

    // Consideramos año actual 2026 para proyectar siglo
    let real_afil_year = if a_afil > 26 { 1900 + a_afil } else { 2000 + a_afil };
    let real_nac_year = if a_nac > 26 { 1900 + a_nac } else { 2000 + a_nac };

    if real_afil_year < real_nac_year {
        return Err(format!(
            "Inconsistencia en el NSS: el año de afiliación al IMSS ({}) no puede ser anterior al año de nacimiento ({}) del asegurado.",
            real_afil_year, real_nac_year
        ));
    }

    Ok(NssInfo {
        clean,
        subdelegacion,
        anio_afiliacion,
        anio_nacimiento,
        consecutivo,
        check_digit: check_digit_char,
    })
}

/// Validación cruzada: RFC vs CURP para una persona física
/// Las primeras 10 posiciones (4 letras iniciales + fecha YYMMDD) deben coincidir exactamente.
pub fn cross_validate_rfc_curp(rfc: &str, curp: &str) -> Result<(), String> {
    let r_clean = clean_alphanumeric_input(rfc);
    let c_clean = clean_alphanumeric_input(curp);

    if r_clean.is_empty() || c_clean.is_empty() {
        return Ok(());
    }

    if r_clean.chars().count() == 12 {
        return Err(
            "Inconsistencia: Un RFC de 12 posiciones corresponde a una Persona Moral y no puede vincularse a una CURP personal."
                .to_string(),
        );
    }

    if r_clean.chars().count() < 10 || c_clean.chars().count() < 10 {
        return Ok(());
    }

    let r_root: String = r_clean.chars().take(10).collect();
    let c_root: String = c_clean.chars().take(10).collect();

    if r_root != c_root {
        return Err(format!(
            "Inconsistencia entre RFC y CURP: las primeras 10 posiciones no coinciden. RFC: '{}' vs CURP: '{}' (deben corresponder a las mismas iniciales y fecha de nacimiento YYMMDD).",
            r_root, c_root
        ));
    }

    Ok(())
}

/// Validación cruzada: NSS vs CURP
/// El año de nacimiento (YY) de las posiciones 5 y 6 del NSS debe coincidir con el año de la CURP.
pub fn cross_validate_nss_curp(nss: &str, curp: &str) -> Result<(), String> {
    let n_clean = clean_alphanumeric_input(nss);
    let c_clean = clean_alphanumeric_input(curp);

    if n_clean.is_empty() || c_clean.is_empty() {
        return Ok(());
    }

    if n_clean.chars().count() >= 6 && c_clean.chars().count() >= 6 {
        let nss_yy: String = n_clean.chars().skip(4).take(2).collect();
        let curp_yy: String = c_clean.chars().skip(4).take(2).collect();

        if nss_yy != curp_yy {
            return Err(format!(
                "Inconsistencia entre NSS y CURP: el año de nacimiento en el NSS ({}) no coincide con el año de la CURP ({}).",
                nss_yy, curp_yy
            ));
        }
    }

    Ok(())
}

/// Validación cruzada: NSS vs RFC de persona física
/// El año de nacimiento (YY) del NSS debe coincidir con el año del RFC.
pub fn cross_validate_nss_rfc(nss: &str, rfc: &str) -> Result<(), String> {
    let n_clean = clean_alphanumeric_input(nss);
    let r_clean = clean_alphanumeric_input(rfc);

    if n_clean.is_empty() || r_clean.is_empty() {
        return Ok(());
    }

    if r_clean.chars().count() == 12 {
        return Err(
            "El trabajador no puede tener un RFC de Persona Moral (12 posiciones). Debe ser Persona Física (13 posiciones)."
                .to_string(),
        );
    }

    if n_clean.chars().count() >= 6 && r_clean.chars().count() >= 6 {
        let nss_yy: String = n_clean.chars().skip(4).take(2).collect();
        let rfc_yy: String = r_clean.chars().skip(4).take(2).collect();

        if nss_yy != rfc_yy {
            return Err(format!(
                "Inconsistencia entre NSS y RFC: el año de nacimiento en el NSS ({}) no coincide con el año del RFC ({}).",
                nss_yy, rfc_yy
            ));
        }
    }

    Ok(())
}

/// Validación completa de datos del Cliente (RFC, CURP, Tipo Persona)
pub fn validate_cliente_data(
    nombre: &str,
    rfc: &str,
    curp: &str,
    tipo_persona: Option<&str>,
) -> Result<(), String> {
    if nombre.trim().is_empty() {
        return Err("El nombre del cliente o empresa es obligatorio.".to_string());
    }

    let rfc_info = validate_rfc(rfc, tipo_persona)?;

    let clean_curp = clean_alphanumeric_input(curp);
    if !clean_curp.is_empty() {
        if rfc_info.is_moral {
            return Err(
                "Una empresa o Persona Moral no debe tener CURP asignada. Deje el campo CURP en blanco para personas morales."
                    .to_string(),
            );
        }

        validate_curp(&clean_curp)?;
        cross_validate_rfc_curp(&rfc_info.clean, &clean_curp)?;
    }

    Ok(())
}

/// Validación completa de datos del Empleado (Nombre, RFC, CURP, NSS, cruce con Cliente)
pub fn validate_empleado_data(
    nombre: &str,
    rfc: &str,
    curp: &str,
    nss: &str,
    cliente_rfc: Option<&str>,
) -> Result<(), String> {
    if nombre.trim().is_empty() {
        return Err("El nombre del empleado es obligatorio.".to_string());
    }

    let clean_rfc = clean_alphanumeric_input(rfc);
    let clean_curp = clean_alphanumeric_input(curp);
    let clean_nss = clean_alphanumeric_input(nss);

    // Al menos uno de los tres identificadores debe ser proporcionado
    if clean_rfc.is_empty() && clean_curp.is_empty() && clean_nss.is_empty() {
        return Err(
            "Debe proporcionar al menos un identificador oficial para el empleado (RFC, CURP o NSS)."
                .to_string(),
        );
    }

    // 1. Validar RFC si existe
    if !clean_rfc.is_empty() {
        let rfc_info = validate_rfc(&clean_rfc, Some("FISICA"))?;
        if rfc_info.is_moral {
            return Err(
                "El RFC del empleado debe ser de Persona Física (13 posiciones), no Persona Moral."
                    .to_string(),
            );
        }

        // Comparar con RFC del cliente patrón para evitar errores de captura
        if let Some(c_rfc) = cliente_rfc {
            let clean_cli = clean_alphanumeric_input(c_rfc);
            if !clean_cli.is_empty() && clean_cli == clean_rfc {
                return Err(format!(
                    "El RFC ingresado para el empleado ('{}') es idéntico al RFC de la empresa / cliente patrón. Verifique la captura.",
                    clean_rfc
                ));
            }
        }
    }

    // 2. Validar CURP si existe
    if !clean_curp.is_empty() {
        validate_curp(&clean_curp)?;
    }

    // 3. Validar NSS si existe
    if !clean_nss.is_empty() {
        validate_nss(&clean_nss)?;
    }

    // 4. Validaciones cruzadas entre los identificadores presentes
    if !clean_rfc.is_empty() && !clean_curp.is_empty() {
        cross_validate_rfc_curp(&clean_rfc, &clean_curp)?;
    }

    if !clean_nss.is_empty() && !clean_curp.is_empty() {
        cross_validate_nss_curp(&clean_nss, &clean_curp)?;
    }

    if !clean_nss.is_empty() && !clean_rfc.is_empty() {
        cross_validate_nss_rfc(&clean_nss, &clean_rfc)?;
    }

    Ok(())
}

/// Valida la estructura de un Registro Patronal (IMSS NRP).
/// Formato oficial: 11 caracteres alfanuméricos.
/// - Posición 1: 1 letra (jurisdicción IMSS, 'A'..='Z')
/// - Posiciones 2 a 11: 10 dígitos numéricos ('0'..='9')
///   - Posiciones 2 y 3: Subdelegación IMSS
///   - Posiciones 4 a 8: Consecutivo patronal
///   - Posiciones 9 y 10: Modalidad de aseguramiento (ej. 10)
///   - Posición 11: Dígito verificador
pub fn validate_registro_patronal(rp: &str) -> Result<RegistroPatronalInfo, String> {
    let clean = clean_alphanumeric_input(rp);
    if clean.is_empty() {
        return Err("La estructura del registro patronal no es correcta. El campo no puede estar vacío.".to_string());
    }

    let len = clean.chars().count();
    if len != 11 {
        return Err(format!(
            "La estructura del registro patronal no es correcta. Debe tener exactamente 11 caracteres (1 letra y 10 dígitos; se ingresaron {}).",
            len
        ));
    }

    let chars: Vec<char> = clean.chars().collect();
    if !chars[0].is_ascii_alphabetic() {
        return Err("La estructura del registro patronal no es correcta. El primer carácter debe ser una letra (ej. Y58-39210-10-4).".to_string());
    }

    for (i, &c) in chars.iter().enumerate().skip(1) {
        if !c.is_ascii_digit() {
            return Err(format!(
                "La estructura del registro patronal no es correcta. La posición {} ('{}') debe ser un número.",
                i + 1,
                c
            ));
        }
    }

    let subdel: String = chars[1..3].iter().collect();
    let consecutivo: String = chars[3..8].iter().collect();
    let modalidad: String = chars[8..10].iter().collect();

    Ok(RegistroPatronalInfo {
        clean,
        letra: chars[0],
        subdelegacion: subdel,
        consecutivo,
        modalidad,
        check_digit: chars[10],
    })
}

/// Valida la estructura de un Registro de Obra de construcción (IMSS).
/// Formatos aceptados para corrección patronal en obras de construcción:
/// 1. Formato SIROC: Letra 'C' seguida de 7 u 8 dígitos (ej. C1234567 o C-1234567).
/// 2. Formato SATIC: 8 a 10 dígitos numéricos (ej. 08-12345-67 o 081234567).
pub fn validate_registro_obra(obra: &str) -> Result<RegistroObraInfo, String> {
    let clean = clean_alphanumeric_input(obra);
    if clean.is_empty() {
        return Err("La estructura del registro de obra no es correcta. El número de obra no puede estar vacío.".to_string());
    }

    let chars: Vec<char> = clean.chars().collect();
    let len = chars.len();

    if chars[0] == 'C' {
        // Formato SIROC: 'C' + 7 u 8 dígitos
        if len != 8 && len != 9 {
            return Err(format!(
                "La estructura del registro de obra no es correcta. El folio SIROC debe iniciar con 'C' y tener 7 u 8 dígitos (ej. C1234567; se ingresaron {} caracteres).",
                len
            ));
        }
        for (idx, &c) in chars.iter().enumerate().skip(1) {
            if !c.is_ascii_digit() {
                return Err(format!(
                    "La estructura del registro de obra no es correcta. En formato SIROC ('C...'), la posición {} ('{}') debe ser un número.",
                    idx + 1,
                    c
                ));
            }
        }
        Ok(RegistroObraInfo {
            clean,
            tipo: RegistroObraTipo::Siroc,
        })
    } else if chars[0].is_ascii_digit() {
        // Formato SATIC: 8 a 10 dígitos numéricos (ej. 08-12345-67)
        if len < 8 || len > 10 {
            return Err(format!(
                "La estructura del registro de obra no es correcta. El formato SATIC debe contener entre 8 y 10 dígitos (ej. 08-12345-67; se ingresaron {} dígitos).",
                len
            ));
        }
        for (idx, &c) in chars.iter().enumerate() {
            if !c.is_ascii_digit() {
                return Err(format!(
                    "La estructura del registro de obra no es correcta. La posición {} ('{}') debe ser un número.",
                    idx + 1,
                    c
                ));
            }
        }
        Ok(RegistroObraInfo {
            clean,
            tipo: RegistroObraTipo::Satic,
        })
    } else {
        Err(format!(
            "La estructura del registro de obra no es correcta. Debe iniciar con 'C' para folio SIROC (ej. C1234567) o con números para SATIC (ej. 08-12345-67; se ingresó '{}').",
            chars[0]
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_generic_sat_identifier() {
        // Genéricos del SAT: los comparten varias personas, nunca son clave de identidad
        assert!(is_generic_sat_identifier("XAXX010101000"));
        assert!(is_generic_sat_identifier("xaxx-010101-000"));
        assert!(is_generic_sat_identifier("XAXX 010101 000"));
        assert!(is_generic_sat_identifier("XEXX010101HNEXXXA4"));

        // RFC/CURP reales: sí identifican a una persona
        assert!(!is_generic_sat_identifier("JIFA981012MCLMNL01"));
        assert!(!is_generic_sat_identifier("CULM980326HPLRNG00"));
        assert!(!is_generic_sat_identifier("GON2110075C9"));
        assert!(!is_generic_sat_identifier(""));
    }

    #[test]
    fn test_valid_rfc_moral() {
        let r = validate_rfc("GON2110075C9", Some("MORAL")).unwrap();
        assert!(r.is_moral);
        assert_eq!(r.birth_yy, "21");
        assert_eq!(r.birth_mm, "10");
        assert_eq!(r.birth_dd, "07");
    }

    #[test]
    fn test_rfc_real_moral_and_fisica() {
        // Constructora del Centro SA: CDC980512AB3
        let r_moral = validate_rfc("CDC980512AB3", Some("MORAL")).unwrap();
        assert!(r_moral.is_moral);
        assert_eq!(r_moral.birth_yy, "98");
        assert_eq!(r_moral.birth_mm, "05");
        assert_eq!(r_moral.birth_dd, "12");

        // Persona Física: PEGJ800101XYZ
        let r_fisica = validate_rfc("PEGJ800101XYZ", Some("FISICA")).unwrap();
        assert!(!r_fisica.is_moral);
        assert_eq!(r_fisica.birth_yy, "80");
        assert_eq!(r_fisica.birth_mm, "01");
        assert_eq!(r_fisica.birth_dd, "01");

        // Moral con longitud física debe fallar
        assert!(validate_rfc("PEGJ800101XYZ", Some("MORAL")).is_err());
        // Física con longitud moral debe fallar
        assert!(validate_rfc("CDC980512AB3", Some("FISICA")).is_err());
    }

    #[test]
    fn test_nss_validation() {
        // NSS válido: 34-91-72-0149-2
        let nss = validate_nss("34917201492").unwrap();
        assert_eq!(nss.subdelegacion, "34");
        assert_eq!(nss.anio_afiliacion, "91");
        assert_eq!(nss.anio_nacimiento, "72");
        assert_eq!(nss.consecutivo, "0149");
        assert_eq!(nss.check_digit, '2');

        // Dígito verificador incorrecto debe fallar
        assert!(validate_nss("34917201499").is_err());

        // Longitud distinta de 11 debe fallar
        assert!(validate_nss("349172014").is_err());
    }

    #[test]
    fn test_curp_validation() {
        // CURP: PEGJ800101HDFRRN01
        let curp = validate_curp("PEGJ800101HDFRRN01").unwrap();
        assert_eq!(curp.birth_yy, "80");
        assert_eq!(curp.birth_mm, "01");
        assert_eq!(curp.birth_dd, "01");
        assert_eq!(curp.gender, 'H');
        assert_eq!(curp.state_code, "DF");

        // Sexo inválido debe fallar
        assert!(validate_curp("PEGJ800101ZDFRRN01").is_err());
        // Entidad inválida debe fallar
        assert!(validate_curp("PEGJ800101HZZRRN01").is_err());
    }

    #[test]
    fn test_cross_validations() {
        // Coherentes:
        let rfc = "PEGJ800101XYZ";
        let curp = "PEGJ800101HDFRRN01";
        // NSS nacido en 1980: 34-98-80-0149-?
        // Digits: 3, 4, 9, 8, 8, 0, 0, 1, 4, 9
        // Mults:  1, 2, 1, 2, 1, 2, 1, 2, 1, 2
        // Prods:  3*1=3, 4*2=8, 9*1=9, 8*2=16(7), 8*1=8, 0, 0, 1*2=2, 4*1=4, 9*2=18(9)
        // Sum: 3+8+9+7+8+0+0+2+4+9 = 50 -> check = 0
        let nss = "34988001490";
        assert!(validate_nss(nss).is_ok());

        assert!(cross_validate_rfc_curp(rfc, curp).is_ok());
        assert!(cross_validate_nss_curp(nss, curp).is_ok());
        assert!(cross_validate_nss_rfc(nss, rfc).is_ok());

        // Incoherencia: NSS con año 75 vs CURP con año 80
        // 34-98-75-0149-? -> 3*1=3, 4*2=8, 9*1=9, 8*2=16(7), 7*1=7, 5*2=10(1), 0, 1*2=2, 4*1=4, 9*2=18(9) = 50 -> check 0
        let nss_invalido = "34987501490";
        assert!(cross_validate_nss_curp(nss_invalido, curp).is_err());
        assert!(cross_validate_nss_rfc(nss_invalido, rfc).is_err());

        // Incoherencia RFC vs CURP en iniciales o fecha
        let curp_distinta = "HERM800101HDFRRN01";
        assert!(cross_validate_rfc_curp(rfc, curp_distinta).is_err());
    }

    #[test]
    fn test_registro_patronal_validation() {
        // RP válido estándar (con guiones y sin guiones)
        let rp_clean = validate_registro_patronal("Y5839210104").unwrap();
        assert_eq!(rp_clean.letra, 'Y');
        assert_eq!(rp_clean.subdelegacion, "58");
        assert_eq!(rp_clean.consecutivo, "39210");
        assert_eq!(rp_clean.modalidad, "10");
        assert_eq!(rp_clean.check_digit, '4');

        let rp_formatted = validate_registro_patronal("Y58-39210-10-4").unwrap();
        assert_eq!(rp_formatted.clean, "Y5839210104");

        // Casos inválidos:
        // Solo 2 caracteres como en el pantallazo del usuario "AS"
        let err_as = validate_registro_patronal("AS").unwrap_err();
        assert!(err_as.contains("La estructura del registro patronal no es correcta"));

        // No inicia con letra
        let err_no_letter = validate_registro_patronal("15839210104").unwrap_err();
        assert!(err_no_letter.contains("La estructura del registro patronal no es correcta"));
        assert!(err_no_letter.contains("primer carácter"));

        // Contiene letras en las posiciones numéricas
        let err_mid_letter = validate_registro_patronal("Y583921010A").unwrap_err();
        assert!(err_mid_letter.contains("La estructura del registro patronal no es correcta"));

        // Vacío
        assert!(validate_registro_patronal("").unwrap_err().contains("La estructura del registro patronal no es correcta"));
    }

    #[test]
    fn test_registro_obra_validation() {
        // SIROC válidos
        let s1 = validate_registro_obra("C1234567").unwrap();
        assert_eq!(s1.tipo, RegistroObraTipo::Siroc);
        assert_eq!(s1.clean, "C1234567");

        let s2 = validate_registro_obra("C-1234567").unwrap();
        assert_eq!(s2.tipo, RegistroObraTipo::Siroc);
        assert_eq!(s2.clean, "C1234567");

        let s3 = validate_registro_obra("C0012345").unwrap();
        assert_eq!(s3.tipo, RegistroObraTipo::Siroc);

        // SATIC válidos
        let sat1 = validate_registro_obra("08-12345-67").unwrap();
        assert_eq!(sat1.tipo, RegistroObraTipo::Satic);
        assert_eq!(sat1.clean, "081234567");

        let sat2 = validate_registro_obra("081234567").unwrap();
        assert_eq!(sat2.tipo, RegistroObraTipo::Satic);

        let sat3 = validate_registro_obra("08-123456-78").unwrap();
        assert_eq!(sat3.tipo, RegistroObraTipo::Satic);

        // Inválidos
        // Muy corto
        assert!(validate_registro_obra("AS").unwrap_err().contains("La estructura del registro de obra no es correcta"));
        assert!(validate_registro_obra("123").unwrap_err().contains("La estructura del registro de obra no es correcta"));
        assert!(validate_registro_obra("C12").unwrap_err().contains("La estructura del registro de obra no es correcta"));

        // No inicia con C ni con dígito
        assert!(validate_registro_obra("X1234567").unwrap_err().contains("La estructura del registro de obra no es correcta"));

        // Contiene letras en dígitos SIROC
        assert!(validate_registro_obra("C123456A").unwrap_err().contains("La estructura del registro de obra no es correcta"));

        // Contiene letras en dígitos SATIC
        assert!(validate_registro_obra("08-12A45-67").unwrap_err().contains("La estructura del registro de obra no es correcta"));

        // Vacío
        assert!(validate_registro_obra("").unwrap_err().contains("La estructura del registro de obra no es correcta"));
    }
}


