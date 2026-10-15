use chrono::{Datelike, NaiveDate};
use unidecode::unidecode;

pub fn get_vacation_days(years: i64) -> f64 {
    let y = years.max(0).min(60);
    match y {
        0..=1 => 12.0,
        2 => 14.0,
        3 => 16.0,
        4 => 18.0,
        5..=9 => 20.0,
        10..=14 => 22.0,
        15..=19 => 24.0,
        20..=24 => 26.0,
        25..=29 => 28.0,
        30..=34 => 30.0,
        35..=39 => 32.0,
        40..=44 => 34.0,
        45..=49 => 36.0,
        50..=54 => 38.0,
        55..=60 => 40.0,
        _ => 42.0,
    }
}

pub fn years_between(start: NaiveDate, end: NaiveDate) -> i64 {
    let mut years = end.year() - start.year();
    if end.month() < start.month() || (end.month() == start.month() && end.day() < start.day()) {
        years -= 1;
    }
    years.max(0) as i64
}

pub fn clean_concepto(val: &str) -> String {
    let decoded = unidecode(val);
    let no_dots = decoded.replace('.', "");
    let underscored = no_dots.replace(' ', "_");
    underscored.trim().to_uppercase()
}

pub fn normalize_periodicidad(raw: Option<&str>, num_dias: Option<f64>) -> String {
    if let Some(p) = raw {
        let trimmed = p.trim().to_uppercase();
        match trimmed.as_str() {
            "01" => return "Diario".to_string(),
            "02" => return "Semanal".to_string(),
            "03" => return "Catorcenal".to_string(),
            "04" => return "Quincenal".to_string(),
            "05" => return "Mensual".to_string(),
            "06" => return "Bimestral".to_string(),
            "07" => return "Unidad de obra".to_string(),
            "08" => return "Comisión".to_string(),
            "09" => return "Precio alzado".to_string(),
            "10" => return "Decenal".to_string(),
            "99" => return "Otra Periodicidad".to_string(),
            s if s.contains("SEMANAL") => return "Semanal".to_string(),
            s if s.contains("CATORCENAL") => return "Catorcenal".to_string(),
            s if s.contains("QUINCENAL") => return "Quincenal".to_string(),
            s if s.contains("MENSUAL") => return "Mensual".to_string(),
            s if s.contains("DIARIO") => return "Diario".to_string(),
            s if s.contains("BIMESTRAL") => return "Bimestral".to_string(),
            s if s.contains("DECENAL") => return "Decenal".to_string(),
            s if !s.is_empty() => return s.to_string(),
            _ => {}
        }
    }

    if let Some(dias) = num_dias {
        if dias <= 1.5 {
            "Diario".to_string()
        } else if dias <= 7.5 {
            "Semanal".to_string()
        } else if dias <= 10.5 {
            "Decenal".to_string()
        } else if dias <= 14.5 {
            "Catorcenal".to_string()
        } else if dias <= 16.5 {
            "Quincenal".to_string()
        } else if dias <= 31.5 {
            "Mensual".to_string()
        } else {
            "Otra Periodicidad".to_string()
        }
    } else {
        "No Especificada".to_string()
    }
}

pub fn extract_year_month(
    fecha_fin: Option<NaiveDate>,
    fecha_pago: Option<NaiveDate>,
    fecha_ini: Option<NaiveDate>,
) -> (i32, u32) {
    if let Some(d) = fecha_fin {
        (d.year(), d.month())
    } else if let Some(d) = fecha_pago {
        (d.year(), d.month())
    } else if let Some(d) = fecha_ini {
        (d.year(), d.month())
    } else {
        (1900, 1)
    }
}

pub fn month_name_spanish(month: u32) -> &'static str {
    match month {
        1 => "ENERO",
        2 => "FEBRERO",
        3 => "MARZO",
        4 => "ABRIL",
        5 => "MAYO",
        6 => "JUNIO",
        7 => "JULIO",
        8 => "AGOSTO",
        9 => "SEPTIEMBRE",
        10 => "OCTUBRE",
        11 => "NOVIEMBRE",
        12 => "DICIEMBRE",
        _ => "DESCONOCIDO",
    }
}

