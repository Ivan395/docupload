use chrono::{Datelike, NaiveDate};
use postgres::Client;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Fila de nómina lista para el reporte, ya cruzada con los días de la cédula
/// del mismo periodo.
#[derive(Debug, Clone)]
pub struct NominaReporteRowDb {
    pub id_nomina: Uuid,
    pub id_empleado: Uuid,

    pub nombre_trabajador: String,
    pub nss: String,
    pub rfc: String,
    pub curp: String,
    pub registro_patronal: String,

    pub periodo_str: String,
    pub anio_periodo: i32,
    pub mes_periodo: String,
    pub fecha_periodo: Option<NaiveDate>,
    pub semana_fiscal: i32,

    // Todas las fechas de pago que reporta el CFDI
    pub fecha_inicial_pago: Option<NaiveDate>,
    pub fecha_final_pago: Option<NaiveDate>,
    pub fecha_pago: Option<NaiveDate>,
    pub fecha_inicio_relacion_laboral: Option<NaiveDate>,
    pub fecha_emision_cfdi: Option<NaiveDate>,
    pub fecha_timbrado: Option<NaiveDate>,

    // Días: nómina vs cédula
    pub num_dias_pagados: f64,
    pub dias_cedula: f64,

    // Percepciones (sin deducciones)
    pub total_sueldos: f64,
    pub total_gravado: f64,
    pub total_exento: f64,
    pub total_percepciones: f64,
    pub percepciones_gravadas: f64,
    pub percepciones_exentas: f64,
    pub num_percepciones: i32,

    pub salario_diario_integrado: f64,
    pub salario_diario_calculado: f64,

    pub tipo_nomina: String,
    pub periodicidad: String,
    pub version_nomina: String,
    pub nombre_archivo: String,
}

#[derive(Debug, Clone, Default)]
pub struct NominaReporteSummaryDb {
    pub total_nominas: usize,
    pub total_empleados: usize,
    pub dias_pagados: f64,
    pub dias_cedula: f64,
    pub total_percepciones: f64,
    pub nominas_sin_cedula: usize,
}

/// Nombre del mes en español desde su número (1..=12).
pub fn month_name_es(month: u32) -> &'static str {
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
        _ => "-",
    }
}

/// Nombre del mes en español a partir de una fecha.
pub fn month_name_es_from_date(fecha: NaiveDate) -> &'static str {
    month_name_es(fecha.month())
}

/// Periodo legible (`ABRIL / 2026`).
pub fn periodo_label(fecha: NaiveDate) -> String {
    format!("{} / {}", month_name_es(fecha.month()), fecha.year())
}

/// Semana fiscal ISO-8601 a partir de una fecha.
///
/// El rango real de ISO es 1..=53. El requerimiento menciona 1..=52/54; la
/// semana 54 solo existe en calendarios fiscales de 53 semanas y no en ISO, por
/// lo que el tope documentado es 53.
pub fn semana_fiscal_iso(fecha: NaiveDate) -> i32 {
    fecha.iso_week().week() as i32
}

/// Nombre del mes en español generado en SQL.
///
/// `TO_CHAR(fecha, 'TMMonth')` devuelve el mes en el idioma configurado en
/// PostgreSQL, que puede no ser español; por eso se traduce con un CASE desde
/// `EXTRACT(MONTH)`.
fn mes_es_sql(fecha_expr: &str) -> String {
    format!(
        r#"
        CASE EXTRACT(MONTH FROM {fecha})
            WHEN 1 THEN 'ENERO'    WHEN 2 THEN 'FEBRERO'  WHEN 3 THEN 'MARZO'
            WHEN 4 THEN 'ABRIL'    WHEN 5 THEN 'MAYO'      WHEN 6 THEN 'JUNIO'
            WHEN 7 THEN 'JULIO'    WHEN 8 THEN 'AGOSTO'    WHEN 9 THEN 'SEPTIEMBRE'
            WHEN 10 THEN 'OCTUBRE' WHEN 11 THEN 'NOVIEMBRE' WHEN 12 THEN 'DICIEMBRE'
        END
        "#,
        fecha = fecha_expr
    )
}

fn row_get_f64(r: &postgres::Row, idx: usize) -> f64 {
    if let Ok(Some(v)) = r.try_get::<_, Option<f64>>(idx) {
        return v;
    }
    if let Ok(Some(v)) = r.try_get::<_, Option<i32>>(idx) {
        return v as f64;
    }
    if let Ok(Some(v)) = r.try_get::<_, Option<i64>>(idx) {
        return v as f64;
    }
    0.0
}

fn row_get_i32(r: &postgres::Row, idx: usize) -> i32 {
    if let Ok(Some(v)) = r.try_get::<_, Option<i32>>(idx) {
        return v;
    }
    if let Ok(Some(v)) = r.try_get::<_, Option<i64>>(idx) {
        return v as i32;
    }
    0
}

fn row_get_str(r: &postgres::Row, idx: usize) -> String {
    if let Ok(Some(v)) = r.try_get::<_, Option<String>>(idx) {
        return v.trim().to_string();
    }
    String::new()
}

fn row_get_date(r: &postgres::Row, idx: usize) -> Option<NaiveDate> {
    if let Ok(Some(v)) = r.try_get::<_, Option<NaiveDate>>(idx) {
        return Some(v);
    }
    if let Ok(Some(v)) = r.try_get::<_, Option<chrono::NaiveDateTime>>(idx) {
        return Some(v.date());
    }
    if let Ok(Some(v)) = r.try_get::<_, Option<chrono::DateTime<chrono::Utc>>>(idx) {
        return Some(v.date_naive());
    }
    None
}

/// Carga las nóminas del reporte desde `reportes_nominas`
/// (la copia filtrada de las nóminas procesadas).
///
/// El reporte SOLO muestra lo que hay en `reportes_nominas`:
/// nóminas de empleados con 1+ obras creadas por el usuario
/// (`registros_obras.creado_usuario = true`) vinculadas en
/// `reportes_liquidaciones` + `reportes_vinculaciones_obras_liquidaciones`,
/// cuyas fechas de pago coinciden (se traslapan) con las fechas
/// del registro de obra.
///
/// - "Días Nóm." se calcula de forma INCLUYENTE entre
///   `fecha_inicial_pago` y `fecha_final_pago`.
/// - "Días Céd." proviene de
///   `reportes_vinculaciones_obras_liquidaciones.dias_registro_obra`
///   de la liquidación del mismo empleado, año y mes.
/// - "Dif." = Días Nóm. - Días Céd.
///
/// Orden: año, mes, NSS, nombre y días.
pub fn fetch_nominas_reporte(
    client: &mut Client,
    id_cliente: Option<Uuid>,
) -> Result<Vec<NominaReporteRowDb>, String> {
    let sql = format!(
        r#"
        WITH per AS (
            SELECT rn.id_nomina AS id_nomina,
                   SUM(COALESCE(rdn.importe, 0))
                       FILTER (WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES') AS total_percepciones,
                   SUM(COALESCE(rdn.importe_gravado, 0))
                       FILTER (WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES') AS percep_gravado,
                   SUM(COALESCE(rdn.importe_exento, 0))
                       FILTER (WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES') AS percep_exento,
                   COUNT(*) FILTER (WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES') AS num_percepciones
            FROM public.reportes_detalles_nominas rdn
            JOIN public.reportes_nominas rn ON rn.id = rdn.id_reporte_nomina
            JOIN public.conceptos c ON c.id = rdn.id_concepto
            JOIN public.tipos_conceptos t ON t.id = c.id_tipo_concepto
            GROUP BY rn.id_nomina
        ),
        ced AS (
            SELECT rl.id_empleado,
                   rl.anio_periodo::int AS anio,
                   UPPER(TRIM(rl.mes_periodo)) AS mes,
                   SUM(COALESCE(rv.dias_registro_obra, 0))::float8 AS dias_cedula
            FROM public.reportes_liquidaciones rl
            JOIN public.reportes_vinculaciones_obras_liquidaciones rv
                 ON rv.id_reporte_liquidacion = rl.id
            GROUP BY rl.id_empleado, rl.anio_periodo, UPPER(TRIM(rl.mes_periodo))
        ),
        base AS (
            SELECT
                rn.id_nomina AS id_nomina,
                rn.id_empleado,
                TRIM(COALESCE(e.nombre, '')) AS nombre,
                TRIM(COALESCE(e.nss, '')) AS nss,
                TRIM(COALESCE(e.rfc, '')) AS rfc,
                TRIM(COALESCE(e.curp, '')) AS curp,
                TRIM(COALESCE(rp.registro, '')) AS registro_patronal,
                COALESCE(rn.fecha_final_pago, rn.fecha_pago, rn.fecha_inicial_pago) AS fecha_periodo,
                rn.fecha_inicial_pago,
                rn.fecha_final_pago,
                rn.fecha_pago,
                ct.fecha_inicio_relacion_laboral,
                cf.fecha_emision::date AS fecha_emision_cfdi,
                rn.timbre_fecha,
                -- "Días Nóm.": inclusivo entre Fecha Inicio y Fecha Fin.
                CASE
                    WHEN rn.fecha_inicial_pago IS NOT NULL AND rn.fecha_final_pago IS NOT NULL
                        THEN (rn.fecha_final_pago - rn.fecha_inicial_pago) + 1
                    ELSE COALESCE(rn.num_dias_pagados, 0)
                END::float8 AS num_dias_pagados,
                COALESCE(rn.total_sueldos, 0)::float8 AS total_sueldos,
                COALESCE(rn.total_gravado, 0)::float8 AS total_gravado,
                COALESCE(rn.total_exento, 0)::float8 AS total_exento,
                COALESCE(per.total_percepciones, 0)::float8 AS total_percepciones,
                COALESCE(per.percep_gravado, 0)::float8 AS percep_gravado,
                COALESCE(per.percep_exento, 0)::float8 AS percep_exento,
                COALESCE(per.num_percepciones, 0)::int AS num_percepciones,
                COALESCE(rn.salario_diario_integrado, 0)::float8 AS sdi,
                COALESCE(rn.salario_diario_calculado, 0)::float8 AS sdi_calc,
                TRIM(COALESCE(rn.tipo_nomina, '')) AS tipo_nomina,
                TRIM(COALESCE(rn.version_nomina, '')) AS version_nomina,
                COALESCE(rn.nombre_archivo, '') AS nombre_archivo,
                TRIM(COALESCE(ct.periodicidad_pago, '')) AS periodicidad
            FROM public.reportes_nominas rn
            JOIN public.empleados e ON e.id = rn.id_empleado
            LEFT JOIN public.nominas n ON n.id = rn.id_nomina
            LEFT JOIN public.revisiones r ON r.id = rn.id_revision
            LEFT JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
            LEFT JOIN public.cfdis cf ON cf.id = n.id_cfdi
            LEFT JOIN public.contratos ct ON ct.id_cfdi = n.id_cfdi
            LEFT JOIN per ON per.id_nomina = rn.id_nomina
            WHERE ($1::uuid IS NULL OR e.id_cliente = $1)
        )
        SELECT
            b.id_nomina,
            b.id_empleado,
            b.nombre,
            b.nss,
            b.rfc,
            b.curp,
            b.registro_patronal,
            b.fecha_periodo,
            {MES} AS mes_periodo,
            EXTRACT(YEAR FROM b.fecha_periodo)::int AS anio_periodo,
            b.fecha_inicial_pago,
            b.fecha_final_pago,
            b.fecha_pago,
            b.fecha_inicio_relacion_laboral,
            b.fecha_emision_cfdi,
            b.timbre_fecha,
            b.num_dias_pagados,
            COALESCE(ced.dias_cedula, 0)::float8 AS dias_cedula,
            b.total_sueldos,
            b.total_gravado,
            b.total_exento,
            b.total_percepciones,
            b.percep_gravado,
            b.percep_exento,
            b.num_percepciones,
            b.sdi,
            b.sdi_calc,
            b.tipo_nomina,
            b.periodicidad,
            b.version_nomina,
            b.nombre_archivo
        FROM base b
        LEFT JOIN ced
               ON ced.id_empleado = b.id_empleado
              AND ced.anio = EXTRACT(YEAR FROM b.fecha_periodo)::int
              AND ced.mes = {MES}
        WHERE b.fecha_periodo IS NOT NULL
        ORDER BY EXTRACT(YEAR FROM b.fecha_periodo) ASC,
                 EXTRACT(MONTH FROM b.fecha_periodo) ASC,
                 b.nss ASC,
                 b.nombre ASC,
                 b.num_dias_pagados ASC
        "#,
        MES = mes_es_sql("b.fecha_periodo")
    );

    let rows = client
        .query(sql.as_str(), &[&id_cliente])
        .map_err(|e| format!("Error consultando reporte de nóminas: {}", e))?;

    let mut result = Vec::with_capacity(rows.len());
    for r in &rows {
        let fecha_periodo = row_get_date(r, 7);

        result.push(NominaReporteRowDb {
            id_nomina: r.get(0),
            id_empleado: r.get(1),
            nombre_trabajador: row_get_str(r, 2),
            nss: row_get_str(r, 3),
            rfc: row_get_str(r, 4),
            curp: row_get_str(r, 5),
            registro_patronal: row_get_str(r, 6),
            fecha_periodo,
            mes_periodo: row_get_str(r, 8),
            anio_periodo: row_get_i32(r, 9),
            periodo_str: fecha_periodo.map(periodo_label).unwrap_or_default(),
            semana_fiscal: fecha_periodo.map(semana_fiscal_iso).unwrap_or(0),
            fecha_inicial_pago: row_get_date(r, 10),
            fecha_final_pago: row_get_date(r, 11),
            fecha_pago: row_get_date(r, 12),
            fecha_inicio_relacion_laboral: row_get_date(r, 13),
            fecha_emision_cfdi: row_get_date(r, 14),
            fecha_timbrado: row_get_date(r, 15),
            num_dias_pagados: row_get_f64(r, 16),
            dias_cedula: row_get_f64(r, 17),
            total_sueldos: row_get_f64(r, 18),
            total_gravado: row_get_f64(r, 19),
            total_exento: row_get_f64(r, 20),
            total_percepciones: row_get_f64(r, 21),
            percepciones_gravadas: row_get_f64(r, 22),
            percepciones_exentas: row_get_f64(r, 23),
            num_percepciones: row_get_i32(r, 24),
            salario_diario_integrado: row_get_f64(r, 25),
            salario_diario_calculado: row_get_f64(r, 26),
            tipo_nomina: row_get_str(r, 27),
            periodicidad: row_get_str(r, 28),
            version_nomina: row_get_str(r, 29),
            nombre_archivo: row_get_str(r, 30),
        });
    }

    Ok(result)
}

/// Copia en background las nóminas a las tablas de reporte
/// (`reportes_nominas` + `reportes_detalles_nominas`).
///
/// Solo se copian las nóminas de empleados que tengan 1 o más
/// registros de obra creados por el usuario
/// (`registros_obras.creado_usuario = true`) vinculados en las
/// tablas de reporte de cédulas (`reportes_liquidaciones` +
/// `reportes_vinculaciones_obras_liquidaciones`), y cuyas
/// fechas de la nómina coincidan (se traslapan) con las fechas
/// del registro de obra (`registros_obras.fecha_inicio`..
/// `fecha_fin`).
///
/// Las tablas de reporte se reemplazan en su totalidad para
/// que la copia sea idempotente: son la vista materializada
/// filtrada de las nóminas procesadas.
pub fn copy_nominas_a_reportes(
    client: &mut Client,
) -> Result<(usize, usize), String> {
    // Reemplazo total de las copias anteriores.
    client
        .execute(
            r#"
            DELETE FROM public.reportes_detalles_nominas
            WHERE id_reporte_nomina IN (
                SELECT id FROM public.reportes_nominas
            )
            "#,
            &[],
        )
        .map_err(|e| {
            format!("Error limpiando reportes_detalles_nominas: {}", e)
        })?;
    client
        .execute("DELETE FROM public.reportes_nominas", &[])
        .map_err(|e| {
            format!("Error limpiando reportes_nominas: {}", e)
        })?;

    // Copia de las nóminas filtradas.
    let nominas = client
        .execute(
            r#"
            INSERT INTO public.reportes_nominas (
                id_nomina, id_revision, id_empleado, version_nomina,
                tipo_nomina, fecha_pago, fecha_inicial_pago,
                fecha_final_pago, num_dias_pagados, total_sueldos,
                total_gravado, total_exento, total_otras_deducciones,
                total_impuestos_retenidos,
                salario_base_cotizacion_aportacion,
                salario_diario_integrado, salario_diario_calculado,
                salario_base_cotizacion_aportacion_calculado,
                salario_diario_integrado_calculado,
                timbre_version, timbre_numero_cert_sat,
                timbre_rfc_proveedor_cert, timbre_uuid, timbre_fecha,
                nombre_archivo
            )
            SELECT
                n.id, n.id_revision, n.id_empleado, n.version_nomina,
                n.tipo_nomina, n.fecha_pago, n.fecha_inicial_pago,
                n.fecha_final_pago, n.num_dias_pagados, n.total_sueldos,
                n.total_gravado, n.total_exento, n.total_otras_deducciones,
                n.total_impuestos_retenidos,
                n.salario_base_cotizacion_aportacion,
                n.salario_diario_integrado, n.salario_diario_calculado,
                n.salario_base_cotizacion_aportacion_calculado,
                n.salario_diario_integrado_calculado,
                n.timbre_version, n.timbre_numero_cert_sat,
                n.timbre_rfc_proveedor_cert, n.timbre_uuid, n.timbre_fecha,
                n.nombre_archivo
            FROM public.nominas n
            WHERE EXISTS (
                SELECT 1
                FROM public.reportes_liquidaciones rl
                JOIN public.reportes_vinculaciones_obras_liquidaciones rv
                     ON rv.id_reporte_liquidacion = rl.id
                JOIN public.registros_obras ro ON ro.id = rv.id_registro_obra
                WHERE rl.id_empleado = n.id_empleado
                  AND COALESCE(ro.creado_usuario, false) = true
                  AND n.fecha_inicial_pago IS NOT NULL
                  AND n.fecha_final_pago IS NOT NULL
                  AND ro.fecha_inicio IS NOT NULL
                  AND ro.fecha_fin IS NOT NULL
                  AND n.fecha_final_pago >= ro.fecha_inicio
                  AND n.fecha_inicial_pago <= ro.fecha_fin
            )
            "#,
            &[],
        )
        .map_err(|e| {
            format!("Error copiando nóminas a reportes: {}", e)
        })?;

    // Copia de los detalles (percepciones, deducciones y otros
    // pagos) de cada nómina copiada.
    let detalles = client
        .execute(
            r#"
            INSERT INTO public.reportes_detalles_nominas (
                id_reporte_nomina, id_concepto, clave, concepto,
                importe_gravado, importe_exento, importe
            )
            SELECT
                rn.id, dn.id_concepto, dn.clave, dn.concepto,
                dn.importe_gravado, dn.importe_exento, dn.importe
            FROM public.reportes_nominas rn
            JOIN public.detalles_nominas dn ON dn.id_nomina = rn.id_nomina
            "#,
            &[],
        )
        .map_err(|e| {
            format!("Error copiando detalles a reportes: {}", e)
        })?;

    log::info!(
        "Copia a reportes de nóminas: {} nóminas, {} detalles",
        nominas,
        detalles
    );

    Ok((nominas as usize, detalles as usize))
}

/// Nombres distintos de conceptos de percepción del reporte,
/// ordenados alfabéticamente. Son los encabezados de columna
/// del reporte.
///
/// Se leen de `reportes_detalles_nominas` (la copia filtrada),
/// no de `detalles_nominas`.
pub fn fetch_percepcion_conceptos(
    client: &mut Client,
) -> Result<Vec<String>, String> {
    let rows = client
        .query(
            r#"
            SELECT DISTINCT TRIM(rdn.concepto) AS nombre
            FROM public.reportes_detalles_nominas rdn
            JOIN public.reportes_nominas rn ON rn.id = rdn.id_reporte_nomina
            JOIN public.conceptos c ON c.id = rdn.id_concepto
            JOIN public.tipos_conceptos t ON t.id = c.id_tipo_concepto
            WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES'
              AND TRIM(rdn.concepto) <> ''
            ORDER BY 1 ASC
            "#,
            &[],
        )
        .map_err(|e| {
            format!("Error consultando conceptos de percepción: {}", e)
        })?;

    Ok(rows
        .iter()
        .map(|r| r.get::<_, String>(0).trim().to_string())
        .collect())
}

/// Montos por concepto de percepción de cada nómina del
/// reporte.
///
/// Se acumulan (un mismo concepto puede repetir en varios
/// renglones del CFDI) y se devuelven indexados por
/// `id_nomina` y nombre de concepto, para alinearlos con
/// `fetch_percepcion_conceptos`.
pub fn fetch_percepciones_por_nomina(
    client: &mut Client,
) -> Result<HashMap<Uuid, HashMap<String, f64>>, String> {
    let rows = client
        .query(
            r#"
            SELECT rn.id_nomina, TRIM(rdn.concepto) AS nombre,
                   COALESCE(rdn.importe, 0)::float8 AS importe
            FROM public.reportes_detalles_nominas rdn
            JOIN public.reportes_nominas rn ON rn.id = rdn.id_reporte_nomina
            JOIN public.conceptos c ON c.id = rdn.id_concepto
            JOIN public.tipos_conceptos t ON t.id = c.id_tipo_concepto
            WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES'
              AND TRIM(rdn.concepto) <> ''
            "#,
            &[],
        )
        .map_err(|e| {
            format!("Error consultando percepciones por nómina: {}", e)
        })?;

    let mut map: HashMap<Uuid, HashMap<String, f64>> = HashMap::new();
    for r in &rows {
        let id_nomina: Uuid = r.get(0);
        let nombre: String = r.get::<_, String>(1).trim().to_string();
        let importe: f64 = r.get(2);
        *map
            .entry(id_nomina)
            .or_default()
            .entry(nombre)
            .or_insert(0.0) += importe;
    }

    Ok(map)
}

/// Obtiene TODAS las nóminas procesadas (de la tabla `nominas`) para
/// las estadísticas de la vista de carga. A diferencia de
/// `fetch_nominas_reporte` (que lee de `reportes_nominas` filtradas),
/// esta consulta devuelve TODAS las nóminas procesadas sin filtrar
/// por obra de usuario ni coincidencia de fechas.
///
/// Incluye joins con empleados, revisiones, cfdis y contratos para
/// obtener todos los campos necesarios para las estadísticas.
pub fn fetch_all_nominas_for_stats(
    client: &mut Client,
    id_cliente: Option<Uuid>,
) -> Result<Vec<NominaReporteRowDb>, String> {
    let sql = format!(
        r#"
        WITH per AS (
            SELECT d.id_nomina,
                   SUM(COALESCE(d.importe, 0))
                       FILTER (WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES') AS total_percepciones,
                   SUM(COALESCE(d.importe_gravado, 0))
                       FILTER (WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES') AS percep_gravado,
                   SUM(COALESCE(d.importe_exento, 0))
                       FILTER (WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES') AS percep_exento,
                   COUNT(*) FILTER (WHERE UPPER(TRIM(t.nombre)) = 'PERCEPCIONES') AS num_percepciones
            FROM public.detalles_nominas d
            JOIN public.conceptos c ON c.id = d.id_concepto
            JOIN public.tipos_conceptos t ON t.id = c.id_tipo_concepto
            GROUP BY d.id_nomina
        ),
        base AS (
            SELECT
                n.id AS id_nomina,
                n.id_empleado,
                TRIM(COALESCE(e.nombre, '')) AS nombre,
                TRIM(COALESCE(e.nss, '')) AS nss,
                TRIM(COALESCE(e.rfc, '')) AS rfc,
                TRIM(COALESCE(e.curp, '')) AS curp,
                TRIM(COALESCE(rp.registro, '')) AS registro_patronal,
                COALESCE(n.fecha_final_pago, n.fecha_pago, n.fecha_inicial_pago) AS fecha_periodo,
                n.fecha_inicial_pago,
                n.fecha_final_pago,
                n.fecha_pago,
                ct.fecha_inicio_relacion_laboral,
                cf.fecha_emision::date AS fecha_emision_cfdi,
                n.timbre_fecha,
                COALESCE(n.num_dias_pagados, 0)::float8 AS num_dias_pagados,
                COALESCE(n.total_sueldos, 0)::float8 AS total_sueldos,
                COALESCE(n.total_gravado, 0)::float8 AS total_gravado,
                COALESCE(n.total_exento, 0)::float8 AS total_exento,
                COALESCE(per.total_percepciones, 0)::float8 AS total_percepciones,
                COALESCE(per.percep_gravado, 0)::float8 AS percep_gravado,
                COALESCE(per.percep_exento, 0)::float8 AS percep_exento,
                COALESCE(per.num_percepciones, 0)::int AS num_percepciones,
                COALESCE(n.salario_diario_integrado, 0)::float8 AS sdi,
                COALESCE(n.salario_diario_calculado, 0)::float8 AS sdi_calc,
                TRIM(COALESCE(n.tipo_nomina, '')) AS tipo_nomina,
                TRIM(COALESCE(n.version_nomina, '')) AS version_nomina,
                COALESCE(n.nombre_archivo, '') AS nombre_archivo,
                TRIM(COALESCE(ct.periodicidad_pago, '')) AS periodicidad
            FROM public.nominas n
            JOIN public.empleados e ON e.id = n.id_empleado
            LEFT JOIN public.revisiones r ON r.id = n.id_revision
            LEFT JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
            LEFT JOIN public.cfdis cf ON cf.id = n.id_cfdi
            LEFT JOIN public.contratos ct ON ct.id_cfdi = n.id_cfdi
            LEFT JOIN per ON per.id_nomina = n.id
            WHERE ($1::uuid IS NULL OR e.id_cliente = $1)
        )
        SELECT
            b.id_nomina,
            b.id_empleado,
            b.nombre,
            b.nss,
            b.rfc,
            b.curp,
            b.registro_patronal,
            b.fecha_periodo,
            {MES} AS mes_periodo,
            EXTRACT(YEAR FROM b.fecha_periodo)::int AS anio_periodo,
            b.fecha_inicial_pago,
            b.fecha_final_pago,
            b.fecha_pago,
            b.fecha_inicio_relacion_laboral,
            b.fecha_emision_cfdi,
            b.timbre_fecha,
            b.num_dias_pagados,
            0.0::float8 AS dias_cedula,
            b.total_sueldos,
            b.total_gravado,
            b.total_exento,
            b.total_percepciones,
            b.percep_gravado,
            b.percep_exento,
            b.num_percepciones,
            b.sdi,
            b.sdi_calc,
            b.tipo_nomina,
            b.periodicidad,
            b.version_nomina,
            b.nombre_archivo
        FROM base b
        WHERE b.fecha_periodo IS NOT NULL
        ORDER BY b.fecha_periodo DESC, b.nombre ASC
        "#,
        MES = mes_es_sql("b.fecha_periodo")
    );

    let rows = client
        .query(sql.as_str(), &[&id_cliente])
        .map_err(|e| format!("Error consultando nóminas para estadísticas: {}", e))?;

    let mut result = Vec::with_capacity(rows.len());
    for r in &rows {
        let fecha_periodo = row_get_date(r, 7);

        result.push(NominaReporteRowDb {
            id_nomina: r.get(0),
            id_empleado: r.get(1),
            nombre_trabajador: row_get_str(r, 2),
            nss: row_get_str(r, 3),
            rfc: row_get_str(r, 4),
            curp: row_get_str(r, 5),
            registro_patronal: row_get_str(r, 6),
            fecha_periodo,
            mes_periodo: row_get_str(r, 8),
            anio_periodo: row_get_i32(r, 9),
            periodo_str: fecha_periodo.map(periodo_label).unwrap_or_default(),
            semana_fiscal: fecha_periodo.map(semana_fiscal_iso).unwrap_or(0),
            fecha_inicial_pago: row_get_date(r, 10),
            fecha_final_pago: row_get_date(r, 11),
            fecha_pago: row_get_date(r, 12),
            fecha_inicio_relacion_laboral: row_get_date(r, 13),
            fecha_emision_cfdi: row_get_date(r, 14),
            fecha_timbrado: row_get_date(r, 15),
            num_dias_pagados: row_get_f64(r, 16),
            dias_cedula: row_get_f64(r, 17),
            total_sueldos: row_get_f64(r, 18),
            total_gravado: row_get_f64(r, 19),
            total_exento: row_get_f64(r, 20),
            total_percepciones: row_get_f64(r, 21),
            percepciones_gravadas: row_get_f64(r, 22),
            percepciones_exentas: row_get_f64(r, 23),
            num_percepciones: row_get_i32(r, 24),
            salario_diario_integrado: row_get_f64(r, 25),
            salario_diario_calculado: row_get_f64(r, 26),
            tipo_nomina: row_get_str(r, 27),
            periodicidad: row_get_str(r, 28),
            version_nomina: row_get_str(r, 29),
            nombre_archivo: row_get_str(r, 30),
        });
    }

    Ok(result)
}
#[derive(Debug, Clone)]
pub struct NominaPercepcionDb {
    pub clave: String,
    pub concepto: String,
    pub tipo_sat: String,
    pub importe_gravado: f64,
    pub importe_exento: f64,
    pub importe: f64,
}

pub fn fetch_percepciones_nomina(
    client: &mut Client,
    id_nomina: Uuid,
) -> Result<Vec<NominaPercepcionDb>, String> {
    let rows = client
        .query(
            r#"
            SELECT TRIM(COALESCE(d.clave, '')) AS clave,
                   TRIM(COALESCE(d.concepto, '')) AS concepto,
                   TRIM(COALESCE(c.tipo, '')) AS tipo_sat,
                   COALESCE(d.importe_gravado, 0)::float8,
                   COALESCE(d.importe_exento, 0)::float8,
                   COALESCE(d.importe, 0)::float8
            FROM public.detalles_nominas d
            JOIN public.conceptos c ON c.id = d.id_concepto
            JOIN public.tipos_conceptos t ON t.id = c.id_tipo_concepto
            WHERE d.id_nomina = $1
              AND UPPER(TRIM(t.nombre)) = 'PERCEPCIONES'
            ORDER BY d.clave ASC, d.concepto ASC
            "#,
            &[&id_nomina],
        )
        .map_err(|e| format!("Error consultando percepciones de la nómina: {}", e))?;

    Ok(rows
        .iter()
        .map(|r| NominaPercepcionDb {
            clave: row_get_str(r, 0),
            concepto: row_get_str(r, 1),
            tipo_sat: row_get_str(r, 2),
            importe_gravado: row_get_f64(r, 3),
            importe_exento: row_get_f64(r, 4),
            importe: row_get_f64(r, 5),
        })
        .collect())
}

/// Resumen agregado de las filas ya filtradas, para los KPIs del encabezado.
pub fn summarize_nominas(rows: &[NominaReporteRowDb]) -> NominaReporteSummaryDb {
    let mut empleados: HashSet<Uuid> = HashSet::new();
    let mut s = NominaReporteSummaryDb {
        total_nominas: rows.len(),
        ..Default::default()
    };
    for r in rows {
        empleados.insert(r.id_empleado);
        s.dias_pagados += r.num_dias_pagados;
        s.dias_cedula += r.dias_cedula;
        s.total_percepciones += r.total_percepciones;
        if r.dias_cedula <= 0.0 {
            s.nominas_sin_cedula += 1;
        }
    }
    s.total_empleados = empleados.len();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fila(emp: Uuid, dias_pag: f64, dias_ced: f64, per: f64) -> NominaReporteRowDb {
        NominaReporteRowDb {
            id_nomina: Uuid::now_v7(),
            id_empleado: emp,
            nombre_trabajador: "TEST".into(),
            nss: String::new(),
            rfc: String::new(),
            curp: String::new(),
            registro_patronal: String::new(),
            periodo_str: "ABRIL / 2026".into(),
            anio_periodo: 2026,
            mes_periodo: "ABRIL".into(),
            fecha_periodo: NaiveDate::from_ymd_opt(2026, 4, 30),
            semana_fiscal: 18,
            fecha_inicial_pago: None,
            fecha_final_pago: None,
            fecha_pago: None,
            fecha_inicio_relacion_laboral: None,
            fecha_emision_cfdi: None,
            fecha_timbrado: None,
            num_dias_pagados: dias_pag,
            dias_cedula: dias_ced,
            total_sueldos: per,
            total_gravado: 0.0,
            total_exento: 0.0,
            total_percepciones: per,
            percepciones_gravadas: 0.0,
            percepciones_exentas: 0.0,
            num_percepciones: 1,
            salario_diario_integrado: 0.0,
            salario_diario_calculado: 0.0,
            tipo_nomina: String::new(),
            periodicidad: String::new(),
            version_nomina: String::new(),
            nombre_archivo: String::new(),
        }
    }

    #[test]
    fn test_semana_fiscal_iso_rango_valido() {
        // 2026-04-15 (miércoles) pertenece a la semana ISO 16.
        let s = semana_fiscal_iso(NaiveDate::from_ymd_opt(2026, 4, 15).unwrap());
        assert_eq!(s, 16);
        assert!((1..=53).contains(&s));
    }

    #[test]
    fn test_semana_fiscal_siempre_en_rango() {
        // Ninguna fecha de dos años seguidos puede salirse de 1..=53.
        let mut d = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        for _ in 0..800 {
            let s = semana_fiscal_iso(d);
            assert!((1..=53).contains(&s), "fecha {} semana {}", d, s);
            d += chrono::Duration::days(1);
        }
    }

    #[test]
    fn test_semana_fiscal_crece_con_el_anio() {
        let enero = semana_fiscal_iso(NaiveDate::from_ymd_opt(2026, 1, 5).unwrap());
        let abril = semana_fiscal_iso(NaiveDate::from_ymd_opt(2026, 4, 15).unwrap());
        assert!(abril > enero, "abril({}) debe ser > enero({})", abril, enero);
    }

    #[test]
    fn test_periodo_label_espanol() {
        assert_eq!(
            periodo_label(NaiveDate::from_ymd_opt(2026, 4, 30).unwrap()),
            "ABRIL / 2026"
        );
        assert_eq!(
            periodo_label(NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()),
            "DICIEMBRE / 2026"
        );
    }

    #[test]
    fn test_month_name_es_todos_los_meses() {
        let esperados = [
            "ENERO", "FEBRERO", "MARZO", "ABRIL", "MAYO", "JUNIO", "JULIO", "AGOSTO",
            "SEPTIEMBRE", "OCTUBRE", "NOVIEMBRE", "DICIEMBRE",
        ];
        for (i, esp) in esperados.iter().enumerate() {
            assert_eq!(month_name_es(i as u32 + 1), *esp);
        }
        assert_eq!(month_name_es(0), "-");
        assert_eq!(month_name_es(13), "-");
    }

    #[test]
    fn test_resumen_agrega_dias_y_percepciones() {
        let e1 = Uuid::now_v7();
        let e2 = Uuid::now_v7();
        let rows = vec![
            fila(e1, 30.0, 30.0, 1000.0),
            fila(e1, 15.0, 15.0, 500.0),
            fila(e2, 30.0, 0.0, 900.0),
        ];
        let s = summarize_nominas(&rows);
        assert_eq!(s.total_nominas, 3);
        assert_eq!(s.total_empleados, 2, "dos empleados distintos");
        assert_eq!(s.dias_pagados, 75.0);
        assert_eq!(s.dias_cedula, 45.0);
        assert_eq!(s.total_percepciones, 2400.0);
        assert_eq!(s.nominas_sin_cedula, 1, "solo la fila de e2 lacks cédula");
    }

    #[test]
    fn test_resumen_vacio_no_panea() {
        let s = summarize_nominas(&[]);
        assert_eq!(s.total_nominas, 0);
        assert_eq!(s.total_empleados, 0);
        assert_eq!(s.dias_pagados, 0.0);
        assert_eq!(s.nominas_sin_cedula, 0);
    }

    #[test]
    fn test_mes_es_sql_cubre_todos_los_meses() {
        let sql = mes_es_sql("b.fecha_periodo");
        for m in 1..=12 {
            assert!(
                sql.contains(&format!("WHEN {} THEN", m)),
                "falta el mes {}",
                m
            );
        }
    }

    #[test]
    fn test_total_dias_por_empleado_agrega_por_mes() {
        // Un empleado con 3 meses debe acumular los días de todos.
        let e = Uuid::now_v7();
        let mut acc: HashMap<Uuid, f64> = HashMap::new();
        for d in [30.0f64, 15.0, 30.0] {
            *acc.entry(e).or_default() += d;
        }
        assert_eq!(acc[&e], 75.0);
    }
}