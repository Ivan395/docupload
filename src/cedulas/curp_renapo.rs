use crate::cedulas::rfc_sat::{clean_ascii_rfc, parse_apellidos_nombre};

/// Catálogo oficial de entidades federativas de RENAPO para la CURP.
pub const ENTIDADES_RENAPO: &[(&str, &str)] = &[
    ("AGUASCALIENTES", "AS"),
    ("BAJA CALIFORNIA", "BC"),
    ("BAJA CALIFORNIA SUR", "BS"),
    ("CAMPECHE", "CC"),
    ("COAHUILA", "CL"),
    ("COLIMA", "CM"),
    ("CHIAPAS", "CS"),
    ("CHIHUAHUA", "CH"),
    ("CIUDAD DE MEXICO", "DF"),
    ("DISTRITO FEDERAL", "DF"),
    ("DURANGO", "DG"),
    ("GUANAJUATO", "GT"),
    ("GUERRERO", "GR"),
    ("HIDALGO", "HG"),
    ("JALISCO", "JC"),
    ("ESTADO DE MEXICO", "MC"),
    ("MEXICO", "MC"),
    ("MICHOACAN", "MN"),
    ("MORELOS", "MS"),
    ("NAYARIT", "NT"),
    ("NUEVO LEON", "NL"),
    ("OAXACA", "OC"),
    ("PUEBLA", "PL"),
    ("QUERETARO", "QT"),
    ("QUINTANA ROO", "QR"),
    ("SAN LUIS POTOSI", "SP"),
    ("SINALOA", "SL"),
    ("SONORA", "SR"),
    ("TABASCO", "TC"),
    ("TAMAULIPAS", "TS"),
    ("TLAXCALA", "TL"),
    ("VERACRUZ", "VZ"),
    ("YUCATAN", "YN"),
    ("ZACATECAS", "ZS"),
    ("EXTRANJERO", "NE"),
];

/// Palabras inconvenientes oficiales de RENAPO.
/// Si las primeras 4 letras coinciden, la segunda letra (vocal) se sustituye por 'X'.
const PALABRAS_INCONVENIENTES_CURP: &[&str] = &[
    "BACA", "BACO", "BUEI", "BUEY", "CACA", "CACO", "CAGA", "CAGO", "CAKA", "CAKO",
    "COGE", "COGI", "COJA", "COJE", "COJI", "COJO", "COLA", "CULO", "FALO", "FETO",
    "GETA", "GUEI", "GUEY", "JETA", "JOTO", "KACA", "KACO", "KAGA", "KAGO", "KOGE",
    "KOGI", "KOJA", "KOJE", "KOJI", "KOJO", "KOLA", "KULO", "LILO", "LOKA", "LOKO",
    "MAME", "MAMO", "MEAR", "MEAS", "MEON", "MIAR", "MION", "MOCO", "MOKO", "MULA",
    "MULO", "NADA", "PEDA", "PEDO", "PENE", "PIPI", "PITO", "POPO", "PUTA", "PUTO",
    "QULO", "RATA", "ROBA", "ROBE", "ROBO", "RUIN", "SENO", "TETA", "VACA", "VAGA",
    "VAGO", "VAKA", "VUEI", "VUEY", "WUEI", "WUEY",
];

/// Extrae la primera consonante interna (no inicial) de una palabra.
/// Si no tiene consonante interna, retorna 'X'. Si es 'Ñ', retorna 'X'.
fn primera_consonante_interna(palabra: &str) -> char {
    let clean = clean_ascii_rfc(palabra);
    let is_vowel = |c: char| matches!(c, 'A' | 'E' | 'I' | 'O' | 'U');
    let mut chars = clean.chars().skip(1);
    while let Some(c) = chars.next() {
        if c.is_ascii_alphabetic() && !is_vowel(c) {
            return if c == 'Ñ' { 'X' } else { c };
        }
    }
    'X'
}

/// Infiere el sexo ('H' para hombre, 'M' para mujer) a partir del nombre completo
/// si no se proporcionó de manera explícita.
pub fn inferir_sexo(nombres: &str) -> char {
    let clean = clean_ascii_rfc(nombres);
    let words: Vec<&str> = clean.split_whitespace().collect();
    if words.is_empty() {
        return 'H';
    }

    // Nombres claramente femeninos
    let nombres_femeninos = [
        "MARIA", "MARY", "MA", "GUADALUPE", "MARISOL", "AMERICA", "VANESSA", "XIMENA", "JIMENA",
        "KAREN", "LIZETH", "INGRID", "FABIOLA", "ADRIANA", "ODALYS", "FERNANDA", "PATRICIA",
        "ALEJANDRA", "GABRIELA", "DANIELA", "LAURA", "ANA", "CARMEN", "ROSA", "SILVIA", "MONICA",
        "TERESA", "ELIZABETH", "GLORIA", "BEATRIZ", "YOLANDA", "LETICIA", "CLAUDIA", "MARTHA",
        "SUSANA", "VERONICA", "LILIANA", "LORENA", "LUCIA", "CECILIA", "ALMA", "YESSICA", "JESSICA",
        "BRENDA", "DIANA", "SANDRA", "PAOLA", "MIRIAM", "FATIMA", "KARLA", "SOFIA", "VALERIA",
        "CAMILA", "VALENTINA", "NATALIA", "REGINA", "ANDREA", "JOHANA", "YAMILETH", "DENISSE",
    ];

    for w in &words {
        if nombres_femeninos.contains(w) {
            return 'M';
        }
    }

    // Si el primer nombre termina en 'A' y no es un nombre masculino típico terminado en 'A'
    let first = words[0];
    let excepciones_masculinas = [
        "JOSE", "JUAN", "LUKA", "LUCA", "EZRA", "JOSHUA", "ISAIAS", "JONAS", "ELIAS", "MATIAS",
        "TOBIAS", "JEREMIAS", "SASHA",
    ];
    if first.ends_with('A') && !excepciones_masculinas.contains(&first) {
        return 'M';
    }

    'H'
}

/// Extrae las 4 letras iniciales de la CURP aplicando la regla oficial de RENAPO:
/// 1. Primera letra del apellido paterno.
/// 2. Primera vocal interna del apellido paterno (a partir de la 2ª letra).
/// 3. Primera letra del apellido materno (o 'X' si no existe).
/// 4. Primera letra del primer nombre de pila (o del segundo nombre si el primero es JOSE o MARIA).
/// Si la combinación forma una palabra inconveniente, la SEGUNDA letra (vocal) se sustituye por 'X'.
pub fn obtener_cuatro_letras_curp(paterno: &str, materno: &str, nombres: &str) -> String {
    let p_clean = clean_ascii_rfc(paterno);
    let m_clean = clean_ascii_rfc(materno);
    let n_clean = clean_ascii_rfc(nombres);

    let p_words: Vec<&str> = p_clean.split_whitespace().collect();
    let p_signif = p_words
        .iter()
        .copied()
        .find(|w| !matches!(*w, "DE" | "DEL" | "LA" | "LAS" | "LOS" | "Y" | "VON" | "VAN"))
        .unwrap_or(p_words.first().copied().unwrap_or("X"));

    let l1 = p_signif.chars().next().unwrap_or('X');

    let is_vowel = |c: char| matches!(c, 'A' | 'E' | 'I' | 'O' | 'U');
    let l2 = p_signif
        .chars()
        .skip(1)
        .find(|c| is_vowel(*c))
        .unwrap_or_else(|| p_signif.chars().nth(1).unwrap_or('X'));

    let m_words: Vec<&str> = m_clean.split_whitespace().collect();
    let m_signif = m_words
        .iter()
        .copied()
        .find(|w| !matches!(*w, "DE" | "DEL" | "LA" | "LAS" | "LOS" | "Y" | "VON" | "VAN"))
        .unwrap_or(m_words.first().copied().unwrap_or(""));
    let l3 = if m_signif.is_empty() {
        'X'
    } else {
        m_signif.chars().next().unwrap_or('X')
    };

    let n_words: Vec<&str> = n_clean.split_whitespace().collect();
    let n_signif = if n_words.len() > 1 {
        let first_upper = n_words[0];
        if matches!(first_upper, "JOSE" | "MARIA" | "MA" | "J") {
            n_words[1]
        } else {
            first_upper
        }
    } else {
        n_words.first().copied().unwrap_or("X")
    };
    let l4 = n_signif.chars().next().unwrap_or('X');

    let cuatro = format!("{}{}{}{}", l1, l2, l3, l4);
    if PALABRAS_INCONVENIENTES_CURP.contains(&cuatro.as_str()) {
        // En CURP la regla oficial sustituye la segunda letra (vocal) por 'X'
        format!("{}X{}{}", l1, l3, l4)
    } else {
        cuatro
    }
}

/// Calcula el dígito verificador oficial de RENAPO (posición 18) usando Módulo 10
/// sobre los primeros 17 caracteres de la CURP.
pub fn calcular_digito_verificador_curp(curp_17: &str) -> char {
    let char_val = |c: char| -> i64 {
        match c {
            '0'..='9' => (c as i64) - ('0' as i64),
            'A'..='N' => (c as i64) - ('A' as i64) + 10,
            'Ñ' => 24,
            'O'..='Z' => (c as i64) - ('O' as i64) + 25,
            _ => 0,
        }
    };

    let chars: Vec<char> = curp_17.chars().collect();
    let mut suma: i64 = 0;
    for (i, c) in chars.iter().take(17).enumerate() {
        let peso = 18 - (i as i64);
        suma += char_val(*c) * peso;
    }

    let rem = suma % 10;
    let dv = (10 - rem) % 10;
    (b'0' + (dv as u8)) as char
}

/// Genera la CURP oficial de 18 caracteres para un trabajador según las normas de RENAPO.
/// - `nombre`: Nombre completo del empleado (ej. "ACOSTA JUAREZ DANIEL").
/// - `rfc_opt`: RFC del empleado (si existe, se aprovechan las 4 letras y 6 dígitos de fecha).
/// - `nss_opt`: Número de Seguridad Social (permite inferir año de nacimiento si falta fecha).
/// - `sexo_opt`: Sexo opcional ('H' o 'M'). Si es None, se infiere del nombre.
/// - `entidad_opt`: Código o nombre del estado de nacimiento. Por defecto "CL" (Coahuila).
pub fn calcular_curp_oficial(
    nombre: &str,
    rfc_opt: Option<&str>,
    _nss_opt: Option<&str>,
    sexo_opt: Option<char>,
    entidad_opt: Option<&str>,
) -> String {
    let (paterno, materno, nombres) = parse_apellidos_nombre(nombre);

    // 1. Primeras 4 letras y 6 dígitos de fecha
    let mut cuatro_letras = String::new();
    let mut fecha_6 = String::new();

    if let Some(rfc) = rfc_opt {
        let clean_rfc: String = rfc.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let upper_rfc = clean_rfc.to_ascii_uppercase();
        if upper_rfc.len() >= 10 && upper_rfc != "XAXX010101000" && upper_rfc != "0000000000000" {
            let l = &upper_rfc[..4];
            let d = &upper_rfc[4..10];
            if l.chars().all(|c| c.is_ascii_alphabetic()) && d.chars().all(|c| c.is_ascii_digit()) {
                cuatro_letras = l.to_string();
                fecha_6 = d.to_string();
            }
        }
    }

    if cuatro_letras.is_empty() {
        cuatro_letras = obtener_cuatro_letras_curp(&paterno, &materno, &nombres);
    }

    // Si no se puede determinar la fecha de nacimiento real (a partir de un RFC válido con fecha),
    // la CURP no puede determinarse y se deja en blanco (String vacío).
    if fecha_6.is_empty() {
        return String::new();
    }

    // 2. Sexo (Posición 11)
    let sexo = match sexo_opt {
        Some('M') | Some('m') | Some('F') | Some('f') => 'M',
        Some('H') | Some('h') => 'H',
        _ => inferir_sexo(&nombres),
    };

    // 3. Entidad Federativa (Posiciones 12 y 13)
    let entidad_2 = if let Some(ent) = entidad_opt {
        let ent_upper = clean_ascii_rfc(ent);
        if ent_upper.len() == 2 {
            ent_upper
        } else {
            ENTIDADES_RENAPO
                .iter()
                .find(|(nombre_edo, _)| ent_upper.contains(nombre_edo))
                .map(|(_, clave)| clave.to_string())
                .unwrap_or_else(|| "CL".to_string())
        }
    } else {
        "CL".to_string()
    };

    // 4. Primeras consonantes internas (Posiciones 14, 15, 16)
    let c_paterno = primera_consonante_interna(&paterno);
    let c_materno = if materno.trim().is_empty() {
        'X'
    } else {
        primera_consonante_interna(&materno)
    };
    let c_nombre = primera_consonante_interna(&nombres);

    // 5. Diferenciador de Siglo / Homonimia (Posición 17)
    // Para personas nacidas antes del 2000 (año > 26): '0'
    // Para personas nacidas a partir del 2000 (año <= 26): 'A'
    let yy: u32 = fecha_6.get(..2).and_then(|s| s.parse().ok()).unwrap_or(90);
    let siglo = if yy <= 26 { 'A' } else { '0' };

    // 6. Cadena de 17 caracteres
    let curp_17 = format!(
        "{}{}{}{}{}{}{}{}",
        cuatro_letras, fecha_6, sexo, entidad_2, c_paterno, c_materno, c_nombre, siglo
    );

    // 7. Dígito Verificador oficial de RENAPO (Posición 18)
    let dv = calcular_digito_verificador_curp(&curp_17);

    format!("{}{}", curp_17, dv)
}

/// Valida si una cadena cumple estrictamente con el formato oficial de CURP de 18 caracteres.
pub fn validar_curp_formato(curp: &str) -> bool {
    let clean: String = curp.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let upper = clean.to_ascii_uppercase();
    if upper.len() != 18 {
        return false;
    }

    // Excluir patrones sintéticos conocidos o inválidos
    if upper == "CAEH890101HCLSNS06"
        || upper == "RASF970101HCLNNR00"
        || upper == "JIFA981012HCLXXX08"
        || upper == "PEGI800101HDFRRN01"
        || &upper[13..16] == "XXX"
        || upper.starts_with("CONC")
        || upper.starts_with("XAXX")
    {
        return false;
    }

    let l4 = &upper[..4];
    let d6 = &upper[4..10];
    let s = upper.chars().nth(10).unwrap_or(' ');
    let ent = &upper[11..13];
    let c3 = &upper[13..16];
    let sig = upper.chars().nth(16).unwrap_or(' ');
    let dv = upper.chars().nth(17).unwrap_or(' ');

    if !l4.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !d6.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    let mm: u32 = d6[2..4].parse().unwrap_or(0);
    let dd: u32 = d6[4..6].parse().unwrap_or(0);
    if !(1..=12).contains(&mm) || !(1..=31).contains(&dd) {
        return false;
    }

    if s != 'H' && s != 'M' {
        return false;
    }
    if !ENTIDADES_RENAPO.iter().any(|(_, clave)| *clave == ent) {
        return false;
    }
    if !c3.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !sig.is_ascii_alphanumeric() {
        return false;
    }
    if !dv.is_ascii_digit() {
        return false;
    }

    // Verificar dígito verificador
    let calc_dv = calcular_digito_verificador_curp(&upper[..17]);
    calc_dv == dv
}

/// Helper para verificar si una CURP es oficial y válida.
pub fn curp_es_valida(curp: &str) -> bool {
    validar_curp_formato(curp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculo_curp_daniel_acosta() {
        let curp = calcular_curp_oficial(
            "ACOSTA JUAREZ DANIEL",
            Some("AOJD821108XYZ"),
            Some("32998253382"),
            Some('H'),
            Some("CL"),
        );
        assert_eq!(curp, "AOJD821108HCLCRN06");
        assert!(validar_curp_formato(&curp));
    }

    #[test]
    fn test_calculo_curp_america_carrizales() {
        let curp = calcular_curp_oficial(
            "CARRIZALES OLIVERA AMERICA YAMILETH",
            Some("CAOA050318XYZ"),
            Some("26200567340"),
            Some('M'),
            Some("CL"),
        );
        assert_eq!(curp, "CAOA050318MCLRLMA8");
        assert!(validar_curp_formato(&curp));
    }

    #[test]
    fn test_calculo_curp_luis_zavala() {
        let curp = calcular_curp_oficial(
            "ZAVALA CARDONA LUIS FERNANDO",
            Some("ZACL020108XYZ"),
            Some("68170261207"),
            Some('H'),
            Some("CL"),
        );
        assert_eq!(curp, "ZACL020108HCLVRSA1");
        assert!(validar_curp_formato(&curp));
    }

    #[test]
    fn test_calculo_curp_marisol_castillo() {
        let curp = calcular_curp_oficial(
            "CASTILLO SANCHEZ MARISOL",
            Some("CASM830208XYZ"),
            Some("39988207914"),
            Some('M'),
            Some("DF"),
        );
        assert_eq!(curp, "CASM830208MDFSNR07");
        assert!(validar_curp_formato(&curp));
    }
}
