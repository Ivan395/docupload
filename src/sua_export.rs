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
    pub tipo_trabajador: char, // '1' Permanente, '2' Eventual, '3' Construcción
}

/// Formatea una línea para `ASEG_<Folio>.TXT` (Longitud fija estricta: 164 caracteres)
pub fn format_aseg_line(rec: &SuaWorkerRecord) -> String {
    let rp = sanitize_rp(&rec.registro_patronal); // 11
    let nss = sanitize_nss(&rec.nss); // 11
    let rfc = sanitize_rfc(&rec.rfc); // 13
    let curp = sanitize_curp(&rec.curp); // 18
    let delim_name = format_sua_name_delimited(&rec.nombre_completo);
    let name_50 = if delim_name.len() > 50 {
        delim_name[..50].to_string()
    } else {
        format!("{:<50}", delim_name)
    }; // 50
    let tipo_trab = if rec.tipo_trabajador == '3' || rec.tipo_trabajador == '2' {
        rec.tipo_trabajador
    } else {
        '1'
    }; // 1
    let jornada = '0'; // 1 (0=Normal)
    let fecha = format_date_sua(rec.fecha_alta, rec.mes_periodo, rec.anio_periodo); // 8

    // SDI: 5 enteros + 2 decimales sin punto decimal (7 posiciones)
    let sdi_cents = ((rec.sdi * 100.0).round() as i64).clamp(0, 9999999);
    let sdi_str = format!("{:07}", sdi_cents); // 7

    let clave_raw = clean_ascii_sua(&rec.clave_ubicacion);
    let clave_17 = if clave_raw.len() > 17 {
        clave_raw[..17].to_string()
    } else {
        format!("{:<17}", clave_raw)
    }; // 17

    let credito_10 = "          "; // 10 espacios (sin crédito)
    let fid_8 = "00000000"; // 8 ceros
    let tipo_desc_1 = '0'; // 1
    let valor_desc_8 = "00000000"; // 8 ceros

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

    debug_assert_eq!(line.len(), 164, "La línea de ASEG debe medir exactamente 164 caracteres");
    line
}

/// Formatea una línea para `AFIL_<Folio>.TXT` (Longitud fija estricta: 168 caracteres)
pub fn format_afil_line(rec: &SuaWorkerRecord) -> String {
    let clean_rp = sanitize_rp(&rec.registro_patronal);
    let rp_base = &clean_rp[..10]; // 10
    let rp_dv = &clean_rp[10..11]; // 1

    let clean_nss = sanitize_nss(&rec.nss);
    let nss_base = &clean_nss[..10]; // 10
    let nss_dv = &clean_nss[10..11]; // 1

    let (paterno, materno, nombres) = parse_mexican_name(&rec.nombre_completo);
    let materno_27 = if materno.len() > 27 {
        materno[..27].to_string()
    } else {
        format!("{:<27}", materno)
    }; // 27
    let paterno_27 = if paterno.len() > 27 {
        paterno[..27].to_string()
    } else {
        format!("{:<27}", paterno)
    }; // 27
    let nombres_27 = if nombres.len() > 27 {
        nombres[..27].to_string()
    } else {
        format!("{:<27}", nombres)
    }; // 27

    // Salario: 4 enteros y 2 decimales sin punto decimal (6 posiciones)
    let sdi_cents = ((rec.sdi * 100.0).round() as i64).clamp(0, 999999);
    let sdi_str = format!("{:06}", sdi_cents); // 6

    let filler_6 = "      "; // 6
    let tipo_trab = if rec.tipo_trabajador == '3' || rec.tipo_trabajador == '2' {
        rec.tipo_trabajador
    } else {
        '1'
    }; // 1
    let tipo_salario = '0'; // 1 (0=Fijo)
    let semana_red = '0'; // 1 (0=Normal)
    let fecha = format_date_sua(rec.fecha_alta, rec.mes_periodo, rec.anio_periodo); // 8
    let umf = "000"; // 3
    let filler_2 = "  "; // 2
    let tipo_mov = "08"; // 2 (08=Reingreso/Alta)
    let guia = "00000"; // 5

    let clave_emp = if !rec.num_empleado.trim().is_empty() {
        clean_ascii_sua(&rec.num_empleado)
    } else {
        clean_ascii_sua(&rec.clave_ubicacion)
    };
    let clave_10 = if clave_emp.len() > 10 {
        clave_emp[..10].to_string()
    } else {
        format!("{:<10}", clave_emp)
    }; // 10

    let filler_1 = " "; // 1
    let curp_18 = sanitize_curp(&rec.curp); // 18
    let constante_formato = '9'; // 1

    let line = format!(
        "{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}",
        rp_base,
        rp_dv,
        nss_base,
        nss_dv,
        materno_27,
        paterno_27,
        nombres_27,
        sdi_str,
        filler_6,
        tipo_trab,
        tipo_salario,
        semana_red,
        fecha,
        umf,
        filler_2,
        tipo_mov,
        guia,
        clave_10,
        filler_1,
        curp_18,
        constante_formato
    );

    debug_assert_eq!(line.len(), 168, "La línea de AFIL debe medir exactamente 168 caracteres");
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
        '1'
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

/// Genera el paquete completo de 5 archivos SUA con formato estricto y validado:
/// 1. ASEG_<Folio>.TXT (Altas de trabajadores - 164 chars)
/// 2. AFIL_<Folio>.TXT (Datos afiliatorios - 168 chars)
/// 3. MOVT_<Folio>.TXT (Movimientos salariales y ausencias - 49 chars)
/// 4. CRED_<Folio>.TXT (Créditos Infonavit - 51 chars)
/// 5. INCAP.TXT (Incapacidades - 49 chars)
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

    let aseg_filename = format!("ASEG_{}.TXT", folio);
    let afil_filename = format!("AFIL_{}.TXT", folio);
    let movt_filename = format!("MOVT_{}.TXT", folio);
    let cred_filename = format!("CRED_{}.TXT", folio);
    let incap_filename = "INCAP.TXT".to_string();

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
        let is_obra = emp_rows.iter().any(|r| r.id_registro_obra.is_some() || !r.obra_numero.trim().is_empty());
        let tipo_trab = if is_obra { '3' } else { '1' };

        let worker = SuaWorkerRecord {
            registro_patronal: rp.clone(),
            nss: nss.clone(),
            rfc: best_rfc,
            curp: best_curp,
            nombre_completo: best_nombre,
            fecha_alta: first.fecha,
            mes_periodo: m_first,
            anio_periodo: first.anio_periodo as i32,
            sdi: first.sdi,
            clave_ubicacion: if !first.clave.trim().is_empty() {
                first.clave.clone()
            } else {
                first.obra_numero.clone()
            },
            num_empleado: first.clave.clone(),
            tipo_trabajador: tipo_trab,
        };

        // Movimiento inicial de Reingreso / Alta ("08")
        movements.push(SuaMovementRecord {
            registro_patronal: rp.clone(),
            nss: nss.clone(),
            tipo_movimiento: "08".to_string(),
            fecha: first.fecha,
            mes_periodo: m_first,
            anio_periodo: first.anio_periodo as i32,
            folio_incapacidad: String::new(),
            dias_incidencia: 0,
            sdi: first.sdi,
        });

        // Revisar cambios de SDI y ausentismos/licencias en los periodos
        let mut prev_sdi = first.sdi;
        for r in &emp_rows {
            let m_curr = crate::cedulas::business::month_to_num(&r.mes_periodo).clamp(1, 12);

            // Modificación de salario ("07") si el SDI cambia significativamente
            if (r.sdi - prev_sdi).abs() > 0.01 && r.sdi > 0.0 {
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
            }

            // Ausentismo ("11")
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

            // Licencia / Ausentismo ("11")
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

            // Incapacidad ("12")
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
    for m in &movements {
        movt_content.push_str(&format_movt_line(m));
        movt_content.push_str("\r\n");
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
        movt_count: movements.len(),
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
            tipo_trabajador: '3',
        };

        let line = format_aseg_line(&rec);
        assert_eq!(line.len(), 164, "Debe ser de exactamente 164 caracteres");
        assert!(line.starts_with("Y631234510112345678901ROAE800101XYZROAE800101HDFRNR01"));
        assert!(line.contains("02092024"));
        assert!(line.contains("0034850"));
    }

    #[test]
    fn test_format_afil_line_exact_168() {
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
            tipo_trabajador: '3',
        };

        let line = format_afil_line(&rec);
        assert_eq!(line.len(), 168, "Debe ser de exactamente 168 caracteres");
        assert!(line.ends_with('9'), "El último carácter debe ser '9'");
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
}
