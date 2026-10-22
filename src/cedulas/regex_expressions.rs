use fancy_regex::Regex;
use indexmap::IndexMap;

pub struct OCRPatterns {
    pub column_names: IndexMap<&'static str, Regex>,
    pub columns_in_file: Regex,

    // Individual business header patterns
    pub business_registro_patronal: Regex,
    pub business_rfc: Regex,
    pub business_curp: Regex,
    pub business_area_geografica: Regex,
    pub business_razon_social: Regex,
    pub business_delegacion: Regex,
    pub business_actividad: Regex,
    pub business_subdelegacion: Regex,
    pub business_domicilio: Regex,
    pub business_codigo_postal: Regex,
    pub business_entidad: Regex,
    pub business_prima: Regex,

    pub employee_data_per_employee: Regex,
    pub employee_periodo_proceso: Regex,
    pub employee_no_seguro_social_nombre_curp_rfc: Regex,
    pub employee_data: Regex,
    pub employee_fechas: Regex,
    pub employee_registro_obras: Regex,
    pub clean_replace_dias: Regex,
    pub clean_remove_spaces: Regex,
    pub clean_remove_commas: Regex,
    pub clean_remove_unnecesary_date: Regex,
    pub clean_remove_unnecesary_spaces: Regex,
    pub clean_remove_floating_colons: Regex,
    pub clean_remove_summary_blocks: Regex,
    pub employee_nss: Regex,
    pub employee_dias_cutoff: Regex,
}

pub fn initialize_patterns() -> Result<OCRPatterns, fancy_regex::Error> {
    let mut column_names = IndexMap::new();
    column_names.insert("clave", Regex::new(r"(?m)c[l1i]ave")?);
    column_names.insert("fecha", Regex::new(r"(?m)fecha")?);
    column_names.insert("dias", Regex::new(r"(?m)d[ií1]?as")?);
    column_names.insert("sdi", Regex::new(r"(?m)s[\.\t ]*d[\.\t ]*i[\.\t ]?")?);
    column_names.insert("lic", Regex::new(r"(?m)l[1i]c[\.\t ]?")?);
    column_names.insert("inc", Regex::new(r"(?m)[1i]nc[\.\t ]?")?);
    column_names.insert("aus", Regex::new(r"(?m)aus[\.\t ]?")?);
    column_names.insert("cf", Regex::new(r"(?m)c[\.\t ]?f[\.\t ]?")?);
    column_names.insert("exc_pat", Regex::new(r"(?m)exc[\.\t ]*pat[\.\t ]?")?);
    column_names.insert("exc_obr", Regex::new(r"(?m)exc[\.\t ]*obr[\.\t ]?")?);
    column_names.insert("pd_pat", Regex::new(r"(?m)p[\.\t ]*d[\.\t ]*pat[\.\t ]?")?);
    column_names.insert("pd_obr", Regex::new(r"(?m)p[\.\t ]*d[\.\t ]*obr[\.\t ]?")?);
    column_names.insert(
        "gmp_pat",
        Regex::new(r"(?m)g[\.\t ]*m[\.\t ]*p[\.\t ]*pat[\.\t ]?")?,
    );
    column_names.insert(
        "gmp_obr",
        Regex::new(r"(?m)g[\.\t ]*m[\.\t ]*p[\.\t ]*obr[\.\t ]?")?,
    );
    column_names.insert("rt", Regex::new(r"(?m)r[\.\t ]*t[\.\t ]?")?);
    column_names.insert(
        "iv_pat",
        Regex::new(r"(?m)[i1][\.\t ]*v[\.\t ]*pat[\.\t ]?")?,
    );
    column_names.insert(
        "iv_obr",
        Regex::new(r"(?m)[i1][\.\t ]*v[\.\t ]*obr[\.\t ]?")?,
    );
    column_names.insert("gps", Regex::new(r"(?m)g[\.\t ]*p[\.\t ]*s[\.\t ]?")?);
    column_names.insert("patronal", Regex::new(r"(?m)patronal")?);
    column_names.insert("obrera", Regex::new(r"(?m)obrera")?);
    column_names.insert(
        "subtotal",
        Regex::new(r"(?m)(sub?[.\t ]*total|[.\t ]*total)")?,
    );

    let columns_in_file = Regex::new(r"c[li]ave[.]*")?;

    let business_registro_patronal = Regex::new(
        r"(?i)reg(?:istro)?[\.\t ]*patronal[\.\t ]*:?[\.\t ]*(?P<registro_patronal>[a-z0-9][a-z0-9\-_]{7,15}[a-z0-9]|[a-z0-9\-]+)",
    )?;
    let business_rfc = Regex::new(r"(?i)rfc\s*:\s*(?P<rfc>[a-z0-9\-]+)")?;
    let business_curp = Regex::new(r"(?i)\bcurp\b\s*[:\t ]*\s*(?P<curp>[a-z0-9\-]{18,22})\b")?;
    let business_area_geografica =
        Regex::new(r"(?is)area\s+geogr[áa]fica\s*:\s*\n?\s*(?P<area>[a-z])\b")?;
    let business_razon_social = Regex::new(
        r"(?i)nombre\s+o\s+raz[óo]n\s+social\s*:\s*(?P<razon_social>[^ ].*?)(?:\s{2,}|\n|\z)",
    )?;
    let business_delegacion = Regex::new(
        r"(?i)delegaci[óo]n\s+imss\s*:\s*(?P<delegacion>[a-zñáéíóúü ]+?)\s+(?P<codigo>\d+)",
    )?;
    let business_actividad =
        Regex::new(r"(?i)actividad\s*:\s*(?P<actividad>[^ ].*?)(?:\s{2,}|\n|\z)")?;
    let business_subdelegacion = Regex::new(
        r"(?i)(?P<subdelegacion>[a-zñáéíóúü ]+?)\s+(?P<codigo_subdelegacion>\d+)\s*\n[^\n]*subdelegaci[óo]n\s+imss\s*:",
    )?;
    let business_domicilio =
        Regex::new(r"(?i)domicilio\s*:\s*(?P<domicilio>[^ ].*?)(?:\s{2,}|\n|\z)")?;
    let business_codigo_postal = Regex::new(r"(?i)c[óo]digo\s+postal\s*:\s*(?P<cp>\d+)")?;
    let business_entidad = Regex::new(r"(?i)entidad\s*:\s*(?P<entidad>[^ ].*?)(?:\s{2,}|\n|\z)")?;
    let business_prima = Regex::new(r"(?i)prima\s+de\s+r\.?t\.?\s*(?P<prima>[0-9\.]+)\s*%")?;

    let employee_data_per_employee = Regex::new(
        r"(?ms)^(\d{2}[-.]?\d{2}[-.]?\d{2}[-.]?\d{4}[-.]?\d\b.*?)((?=^\d{2}[-.]?\d{2}[-.]?\d{2}[-.]?\d{4}[-.]?\d\b|\z)|(?=^p[aá]gina)|(?=^[\d \.]+[\n][\d \.]+[\n]total))",
    )?;
    let employee_periodo_proceso = Regex::new(
        r"(?m)per[íi]odo[\t \.]*de[\t \.]*proceso[:\t ]*(?P<mes_periodo>\w+)[-\.]?(?P<anio_periodo>\d{4})",
    )?;

    let employee_no_seguro_social_nombre_curp_rfc = Regex::new(
        r"(?i)(?m)(?P<no_seguro_social>\d{2}[-\.]?\d{2}[-\.]?\d{2}[-\.]?\d{4}[-\.]?\d)[\t ]+(?P<nombre>.+?)(?:(?:[\t ]+(?P<curp>\b[a-z]{4}\d{6}[a-z0-9]{7,8}\b))|(?:[\t ]+(?P<rfc>\b[a-zñ&]{3,4}-?\d{6}(?:-?[a-z0-9]{3})?\b)))?(?:(?:[\t ]+(?P<curp_alt>\b[a-z]{4}\d{6}[a-z0-9]{7,8}\b))|(?:[\t ]+(?P<rfc_alt>\b[a-zñ&]{3,4}-?\d{6}(?:-?[a-z0-9]{3})?\b)))?(?:[\t ]+(?P<clave_ubicacion>[a-z0-9\(\)\-]*\d[a-z0-9\(\)\-]*))?[\t ]*$",
    )?;

    let employee_data = Regex::new(
        r"(?m)((?P<clave>[\w/]+)[ \t]+(?P<fecha>\d{2}/\d{2}/\d{4})?[\t ])[\t ]?(?P<data_with_date>[\d\t \.]+.*)|(?P<data_without_date>^\d+[ \t][ \t\d\.]+.*)",
    )?;
    let employee_fechas =
        Regex::new(r"(?m)(?P<clave>^\w+)[ \t]+(?P<fecha>\d{2}/\d{2}/\d{4})[\n]?$")?;
    let employee_registro_obras = Regex::new(
        r"(?im)(?:folio[\.\t ]*de[\.\t ]*)?registro[\.\t ]*de[\.\t ]*obra[s]?[:]?[\.\t ]*(?P<registro_obra>[a-z0-9_-]+)[\t ]+(?:d\S*as|dias|d[ií]as|d[ií]?)[:\.\t ]*(?P<dias_registro_obra>\d+)",
    )?;

    let clean_replace_dias = Regex::new(r"(?i)\bd[\w\u{0080}-\u{024F}\-]*as\b")?;

    let clean_remove_spaces = Regex::new(r"(?m)[ ]+")?;
    let clean_remove_commas = Regex::new(r"(?m)[,]+")?;
    let clean_remove_unnecesary_date = Regex::new(r"(?m)\(homologada.+")?;
    let clean_remove_unnecesary_spaces = Regex::new(r"(?m)^[ \t]+")?;
    let clean_remove_floating_colons = Regex::new(r"(?m)^[ \t]*:[ \t]*\n")?;

    let clean_remove_summary_blocks = Regex::new(
        r"(?is)(?:\d+\s+[\d,.]+\s+[\d,.]+\s+[\d,.]+\s+[\d,.]+\s+[\d,.]+\s+[\d,.]+\s+[\d,.]+\s*[\n\r]+.*?)?total\s+de\s+d[íi]as\s+cotizados.*",
    )?;

    let employee_nss = Regex::new(r"\d{2}-\d{2}-\d{2}-\d{4}-\d")?;
    let employee_dias_cutoff = Regex::new(r"(?i)\b(?:d\S*as|dias|d[íi]as)[:\s]*\d+")?;

    Ok(OCRPatterns {
        column_names,
        columns_in_file,
        business_registro_patronal,
        business_rfc,
        business_curp,
        business_area_geografica,
        business_razon_social,
        business_delegacion,
        business_actividad,
        business_subdelegacion,
        business_domicilio,
        business_codigo_postal,
        business_entidad,
        business_prima,
        employee_data_per_employee,
        employee_periodo_proceso,
        employee_no_seguro_social_nombre_curp_rfc,
        employee_data,
        employee_fechas,
        employee_registro_obras,
        clean_replace_dias,
        clean_remove_spaces,
        clean_remove_commas,
        clean_remove_unnecesary_date,
        clean_remove_unnecesary_spaces,
        clean_remove_floating_colons,
        clean_remove_summary_blocks,
        employee_nss,
        employee_dias_cutoff,
    })
}
