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
