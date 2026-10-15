/// Limpia y normaliza texto eliminando acentos, caracteres especiales y pasando a mayúsculas.
pub fn clean_ascii_rfc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'á' | 'Á' | 'à' | 'À' | 'ä' | 'Ä' | 'â' | 'Â' => out.push('A'),
            'é' | 'É' | 'è' | 'È' | 'ë' | 'Ë' | 'ê' | 'Ê' => out.push('E'),
            'í' | 'Í' | 'ì' | 'Ì' | 'ï' | 'Ï' | 'î' | 'Î' => out.push('I'),
            'ó' | 'Ó' | 'ò' | 'Ò' | 'ö' | 'Ö' | 'ô' | 'Ô' => out.push('O'),
            'ú' | 'Ú' | 'ù' | 'Ù' | 'ü' | 'Ü' | 'û' | 'Û' => out.push('U'),
            'ñ' | 'Ñ' => out.push('Ñ'),
            '&' => out.push('&'),
            c if c.is_ascii_alphanumeric() => out.push(c.to_ascii_uppercase()),
            ' ' => out.push(' '),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Separa un nombre completo en (Paterno, Materno, Nombres).
/// Reconoce prefijos compuestos típicos ("DE", "DEL", "DE LA", "LOS", "SAN", etc.).
pub fn parse_apellidos_nombre(full_name: &str) -> (String, String, String) {
    let (cleaned, _) = crate::cedulas::employees::clean_name_and_extract_pension(full_name);
    let clean = clean_ascii_rfc(&cleaned);
    let words: Vec<&str> = clean.split_whitespace().collect();
    if words.is_empty() {
        return (String::new(), String::new(), String::new());
    }

    let is_prefix = |w: &str| -> bool {
        matches!(
            w,
            "DE" | "DEL" | "LA" | "LAS" | "LOS" | "SAN" | "SANTA" | "VON" | "VAN" | "Y" | "MAC" | "MC"
        )
    };

    let mut idx = 0;

    // 1. Paterno
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

    // 2. Materno
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

    // 3. Nombres
    let nombres = if idx < words.len() {
        words[idx..].join(" ")
    } else {
        String::new()
    };

    if materno_parts.is_empty() && !paterno.is_empty() && !nombres.is_empty() {
        (paterno, String::new(), nombres)
    } else if materno_parts.len() == 1 && nombres.is_empty() && words.len() == 2 {
        (paterno, String::new(), materno)
    } else {
        (paterno, materno, nombres)
    }
}

/// Lista oficial del SAT de las 43 palabras inconvenientes/malsonantes
/// que deben sustituir su 4ª letra por una 'X'.
const PALABRAS_INCONVENIENTES_SAT: &[&str] = &[
    "BUEI", "BUEY", "CACA", "CACO", "CAGA", "CAGO", "CAKA", "CAKO", "COGE", "COGI",
    "COJA", "COJE", "COJI", "COJO", "COLA", "CULO", "FALO", "FETO", "GETA", "GUEI",
    "GUEY", "JETA", "JOTO", "KACA", "KACO", "KAGA", "KAGO", "KOGE", "KOGI", "KOJA",
    "KOJE", "KOJI", "KOJO", "KOLA", "KULO", "LILO", "LOKA", "LOKO", "MAME", "MAMO",
    "MEAR", "MEAS", "MEON", "MIAR", "MION", "MOCO", "MOKO", "MULA", "MULO", "NADA",
    "PEDA", "PEDO", "PENE", "PIPI", "PITO", "POPO", "PUTA", "PUTO", "QULO", "RATA",
    "ROBA", "ROBE", "ROBO", "RUIN", "SENO", "TETA", "VACA", "VAGA", "VAGO", "VAKA",
    "VUEI", "VUEY", "WUEI", "WUEY",
];

/// Extrae las 4 letras iniciales del RFC para personas físicas según la regla oficial del SAT:
/// 1. Primera letra del apellido paterno.
/// 2. Primera vocal interna del apellido paterno (no la inicial).
/// 3. Primera letra del apellido materno (o 'X' si no tiene).
/// 4. Primera letra del primer nombre (o del segundo nombre si el primero es JOSE o MARIA).
pub fn obtener_cuatro_letras_rfc(paterno: &str, materno: &str, nombres: &str) -> String {
    let p_clean = clean_ascii_rfc(paterno);
    let m_clean = clean_ascii_rfc(materno);
    let n_clean = clean_ascii_rfc(nombres);

    // Filtrar prefijos en el paterno para la raíz
    let p_words: Vec<&str> = p_clean.split_whitespace().collect();
    let p_signif = p_words
        .iter()
        .copied()
        .find(|w| !matches!(*w, "DE" | "DEL" | "LA" | "LAS" | "LOS" | "Y" | "VON" | "VAN"))
        .unwrap_or(p_words.first().copied().unwrap_or("X"));

    // 1. Primera letra del paterno
    let l1 = p_signif.chars().next().unwrap_or('X');

    // 2. Primera vocal interna del paterno (a partir del segundo carácter)
    let is_vowel = |c: char| matches!(c, 'A' | 'E' | 'I' | 'O' | 'U');
    let l2 = p_signif
        .chars()
        .skip(1)
        .find(|c| is_vowel(*c))
        .unwrap_or_else(|| p_signif.chars().nth(1).unwrap_or('X'));

    // 3. Primera letra del materno (o 'X' si no existe)
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

    // 4. Primera letra del nombre de pila:
    // Si el primer nombre es JOSE o MARIA (o J. / MA.) y hay más de un nombre, tomar el segundo.
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

    let mut cuatro = format!("{}{}{}{}", l1, l2, l3, l4);
    if PALABRAS_INCONVENIENTES_SAT.contains(&cuatro.as_str()) {
        cuatro.pop();
        cuatro.push('X');
    }
    cuatro
}

/// Calcula los dos caracteres de la homoclave según el algoritmo oficial del SAT (Anexo I y Anexo II):
/// 1. Asigna valores numéricos de dos dígitos a cada carácter del nombre completo.
/// 2. Multiplica pares de dígitos contiguos por el segundo dígito.
/// 3. Suma los productos y toma las últimas tres cifras.
/// 4. Divide entre 34: cociente y residuo se mapean en la tabla de 34 caracteres del SAT.
pub fn calcular_homoclave_sat(paterno: &str, materno: &str, nombres: &str) -> (char, char) {
    let mapping = |c: char| -> &'static str {
        match c {
            ' ' => "00",
            '0' => "00",
            '1' => "01",
            '2' => "02",
            '3' => "03",
            '4' => "04",
            '5' => "05",
            '6' => "06",
            '7' => "07",
            '8' => "08",
            '9' => "09",
            '&' => "10",
            'A' => "11",
            'B' => "12",
            'C' => "13",
            'D' => "14",
            'E' => "15",
            'F' => "16",
            'G' => "17",
            'H' => "18",
            'I' => "19",
            'J' => "21",
            'K' => "22",
            'L' => "23",
            'M' => "24",
            'N' => "25",
            'O' => "26",
            'P' => "27",
            'Q' => "28",
            'R' => "29",
            'S' => "32",
            'T' => "33",
            'U' => "34",
            'V' => "35",
            'W' => "36",
            'X' => "37",
            'Y' => "38",
            'Z' => "39",
            'Ñ' => "40",
            _ => "00",
        }
    };

    let p_clean = clean_ascii_rfc(paterno);
    let m_clean = clean_ascii_rfc(materno);
    let n_clean = clean_ascii_rfc(nombres);

    let mut full = format!("{} {} {}", p_clean, m_clean, n_clean);
    full = full.split_whitespace().collect::<Vec<_>>().join(" ");

    // Se antepone un cero según el estándar del SAT
    let mut num_str = String::from("0");
    for c in full.chars() {
        num_str.push_str(mapping(c));
    }

    let bytes = num_str.as_bytes();
    let mut suma: i64 = 0;
    if bytes.len() >= 2 {
        for i in 0..(bytes.len() - 1) {
            let n1_str = std::str::from_utf8(&bytes[i..i + 2]).unwrap_or("00");
            let n1: i64 = n1_str.parse().unwrap_or(0);
            let n2: i64 = ((bytes[i + 1] - b'0') as i64).clamp(0, 9);
            suma += n1 * n2;
        }
    }

    let ultimas3 = (suma % 1000).abs();
    let cociente = (ultimas3 / 34) as usize;
    let residuo = (ultimas3 % 34) as usize;

    // Tabla del Anexo II oficial del SAT (34 caracteres)
    let table = b"123456789ABCDEFGHIJKLMNPQRSTUVWXYZ";

    let c1 = if cociente < table.len() {
        table[cociente] as char
    } else {
        '1'
    };
    let c2 = if residuo < table.len() {
        table[residuo] as char
    } else {
        '1'
    };

    (c1, c2)
}

/// Calcula el dígito verificador del SAT (posición 13) usando el algoritmo Módulo 11
/// sobre los primeros 12 caracteres del RFC.
pub fn calcular_digito_verificador_sat(rfc_12: &str) -> char {
    let val_map = |c: char| -> i64 {
        match c {
            '0'..='9' => (c as i64) - ('0' as i64),
            'A'..='N' => (c as i64) - ('A' as i64) + 10,
            '&' => 24,
            'O'..='Z' => (c as i64) - ('O' as i64) + 25,
            ' ' => 37,
            'Ñ' => 38,
            _ => 0,
        }
    };

    let chars: Vec<char> = rfc_12.chars().collect();
    let mut suma: i64 = 0;
    for (i, c) in chars.iter().take(12).enumerate() {
        let peso = 13 - (i as i64);
        suma += val_map(*c) * peso;
    }

    let rem = suma % 11;
    if rem == 0 {
        '0'
    } else {
        let dv = 11 - rem;
        if dv == 10 {
            'A'
        } else {
            (b'0' + (dv as u8)) as char
        }
    }
}

/// Genera el RFC oficial de 13 caracteres para una persona física utilizando el algoritmo del SAT.
/// - Si la persona tiene CURP válida, se toma la base oficial de 10 caracteres (`curp[..10]`).
/// - Si la persona tiene CURP válida con fecha real, se toma la base oficial de 10 caracteres (`curp[..10]`).
/// - Si no tiene CURP válida o no se puede determinar la fecha de nacimiento real, retorna el RFC genérico ("XAXX010101000") que acepta el programa del SUA.
/// - La homoclave (posiciones 11 y 12) se calcula mediante el algoritmo oficial Módulo 34 del SAT sobre el nombre completo.
/// - El dígito verificador (posición 13) se calcula mediante el algoritmo Módulo 11 del SAT sobre los primeros 12 caracteres.
pub fn calcular_rfc_sat(nombre: &str, curp_opt: Option<&str>, _nss_opt: Option<&str>) -> String {
    let (paterno, materno, nombres) = parse_apellidos_nombre(nombre);
    if paterno.is_empty() && nombres.is_empty() {
        return "XAXX010101000".to_string();
    }

    // 1. Obtener los 10 primeros caracteres (4 letras + 6 dígitos)
    let base_10 = if let Some(curp) = curp_opt {
        let clean_c: String = curp.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let upper_c = clean_c.to_ascii_uppercase();
        if upper_c.len() >= 10
            && !upper_c.starts_with("XAXX")
            && !upper_c.starts_with("CONC")
            && upper_c != "CAEH890101HCLSNS06"
            && upper_c != "RASF970101HCLNNR00"
            && upper_c != "JIFA981012HCLXXX08"
            && upper_c != "PEGI800101HDFRRN01"
        {
            let l = &upper_c[..4];
            let d = &upper_c[4..10];
            if l.chars().all(|c| c.is_ascii_alphabetic()) && d.chars().all(|c| c.is_ascii_digit()) {
                let mm: u32 = d[2..4].parse().unwrap_or(0);
                let dd: u32 = d[4..6].parse().unwrap_or(0);
                if (1..=12).contains(&mm) && (1..=31).contains(&dd) {
                    Some(upper_c[..10].to_string())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let base_10 = match base_10 {
        Some(b) => b,
        None => return "XAXX010101000".to_string(),
    };

    // 2. Calcular homoclave oficial SAT (posiciones 11 y 12)
    let (h1, h2) = calcular_homoclave_sat(&paterno, &materno, &nombres);

    // 3. Cadena de 12 caracteres
    let rfc_12 = format!("{}{}{}", base_10, h1, h2);

    // 4. Calcular dígito verificador oficial SAT (posición 13)
    let dv = calcular_digito_verificador_sat(&rfc_12);

    format!("{}{}", rfc_12, dv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ejemplo_oficial_sat_emma_gomez_diaz() {
        let homoclave = calcular_homoclave_sat("GOMEZ", "DIAZ", "EMMA");
        assert_eq!(homoclave, ('G', 'R'));

        let rfc12 = format!("GODE561231{}{}", homoclave.0, homoclave.1);
        let dv = calcular_digito_verificador_sat(&rfc12);
        assert_eq!(dv, '8');

        assert_eq!(format!("{}{}", rfc12, dv), "GODE561231GR8");
    }

    #[test]
    fn test_rfc_empleado_con_curp() {
        let rfc = calcular_rfc_sat("ACOSTA JUAREZ DANIEL", Some("AOJD821108HCLCRN06"), Some("32998253382"));
        assert_eq!(rfc.len(), 13);
        assert!(rfc.starts_with("AOJD821108"));
    }

    #[test]
    fn test_rfc_sin_curp_retorna_generico_sua() {
        let rfc = calcular_rfc_sat("CASTRO ENCINA JOSE HUMBERTO", None, Some("32088956753"));
        assert_eq!(rfc, "XAXX010101000");
    }
}
