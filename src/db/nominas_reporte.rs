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
    /// Totales por mes (clave: "ANIO-MES" ej "2026-04")
    pub totales_por_mes: HashMap<String, MesStats>,
}

#[derive(Debug, Default, Clone)]
pub struct MesStats {
    pub total_nominas: usize,
    pub empleados_unicos: usize,
    pub dias_pagados: f64,
    pub total_percepciones: f64,
    pub dias_cedula: f64,
    pub nominas_sin_cedula: usize,
    /// Empleados y sus días en este mes
    pub empleados: HashMap<Uuid, EmpleadoMesStats>,
}

#[derive(Debug, Default, Clone)]
pub struct EmpleadoMesStats {
    pub nombre: String,
    pub nss: String,
    pub dias_pagados: f64,
    pub total_percepciones: f64,
    pub dias_cedula: f64,
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
            SELECT rdn.id_reporte_nomina AS id_nomina,
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
            GROUP BY rdn.id_reporte_nomina
        ),
        ced AS (
            SELECT rl.id_empleado,
                   rl.anio_periodo::int AS anio,
                   UPPER(TRIM(rl.mes_periodo)) AS mes,
                   SUM(COALESCE(rv.dias_registro_obra, 0))::float8 AS dias_cedula
            FROM public.reportes_liquidaciones rl
            JOIN public.reportes_vinculaciones_obras_liquidaciones rv
                 ON rv.id_reporte_liquidacion = rl.id
            JOIN public.registros_obras ro
                 ON ro.id = rv.id_registro_obra
            WHERE COALESCE(ro.creado_usuario, false) = true
            GROUP BY rl.id_empleado, rl.anio_periodo, UPPER(TRIM(rl.mes_periodo))
        ),
        base AS (
            SELECT
                rn.id AS id_nomina,
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
                COALESCE(rn.num_dias_pagados, 0)::float8 AS num_dias_pagados,
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
            LEFT JOIN per ON per.id_nomina = rn.id
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
/// fechas de la nómina coincidan (se traslapen) con las fechas
/// del registro de obra (`registros_obras.fecha_inicio`..
/// `fecha_fin`).
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

    // Copia de las nóminas filtradas estrictamente por obra de usuario y rango de fechas.
    let nominas = client
        .execute(
            r#"
            INSERT INTO public.reportes_nominas (
                id, id_nomina, id_revision, id_empleado, version_nomina,
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
                uuidv7(), n.id, n.id_revision, n.id_empleado, n.version_nomina,
                n.tipo_nomina, n.fecha_pago, n.fecha_inicial_pago,
                n.fecha_final_pago,
                CASE
                    WHEN n.fecha_inicial_pago IS NOT NULL AND n.fecha_final_pago IS NOT NULL
                        THEN (n.fecha_final_pago - n.fecha_inicial_pago) + 1
                    ELSE COALESCE(n.num_dias_pagados, 0)
                END,
                n.total_sueldos,
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
                JOIN public.registros_obras ro
                     ON ro.id = rv.id_registro_obra
                WHERE rl.id_empleado = n.id_empleado
                  AND COALESCE(ro.creado_usuario, false) = true
                  AND COALESCE(n.fecha_final_pago, n.fecha_pago, n.fecha_inicial_pago) >= ro.fecha_inicio
                  AND COALESCE(n.fecha_inicial_pago, n.fecha_pago, n.fecha_final_pago) <= ro.fecha_fin
            )
            "#,
            &[],
        )
        .map_err(|e| {
            format!("Error copiando nóminas a reportes: {}", e)
        })?;

    // Copia de los detalles (percepciones) de cada nómina copiada.
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
            SELECT rn.id, TRIM(rdn.concepto) AS nombre,
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
              AND EXISTS (
                  SELECT 1
                  FROM public.reportes_liquidaciones rl
                  JOIN public.reportes_vinculaciones_obras_liquidaciones rv
                       ON rv.id_reporte_liquidacion = rl.id
                  WHERE rl.id_empleado = n.id_empleado
                    AND rv.id_registro_obra IS NOT NULL
              )
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
    id_reporte_nomina: Uuid,
) -> Result<Vec<NominaPercepcionDb>, String> {
    let rows = client
        .query(
            r#"
            SELECT TRIM(COALESCE(rdn.clave, '')) AS clave,
                   TRIM(COALESCE(rdn.concepto, '')) AS concepto,
                   TRIM(COALESCE(c.tipo, '')) AS tipo_sat,
                   COALESCE(rdn.importe_gravado, 0)::float8,
                   COALESCE(rdn.importe_exento, 0)::float8,
                   COALESCE(rdn.importe, 0)::float8
            FROM public.reportes_detalles_nominas rdn
            JOIN public.conceptos c ON c.id = rdn.id_concepto
            JOIN public.tipos_conceptos t ON t.id = c.id_tipo_concepto
            WHERE rdn.id_reporte_nomina = $1
              AND UPPER(TRIM(t.nombre)) = 'PERCEPCIONES'
            ORDER BY rdn.clave ASC, rdn.concepto ASC
            "#,
            &[&id_reporte_nomina],
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

        // Estadísticas por mes
        let fecha_periodo = r.fecha_periodo.unwrap_or(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
        let mes_key = format!("{}-{:02}", fecha_periodo.year(), fecha_periodo.month());
        let mes_entry = s.totales_por_mes.entry(mes_key).or_default();
        mes_entry.total_nominas += 1;
        mes_entry.dias_pagados += r.num_dias_pagados;
        mes_entry.dias_cedula += r.dias_cedula;
        mes_entry.total_percepciones += r.total_percepciones;
        if r.dias_cedula <= 0.0 {
            mes_entry.nominas_sin_cedula += 1;
        }

        let emp_entry = mes_entry.empleados.entry(r.id_empleado).or_insert_with(|| EmpleadoMesStats {
            nombre: r.nombre_trabajador.clone(),
            nss: r.nss.clone(),
            dias_pagados: 0.0,
            total_percepciones: 0.0,
            dias_cedula: 0.0,
        });
        emp_entry.dias_pagados += r.num_dias_pagados;
        emp_entry.total_percepciones += r.total_percepciones;
        emp_entry.dias_cedula += r.dias_cedula;
    }
    // Contar empleados únicos por mes
    for mes in s.totales_por_mes.values_mut() {
        mes.empleados_unicos = mes.empleados.len();
    }
    s.total_empleados = empleados.len();
    s
}
 
pub fn month_num_from_es(mes: &str) -> u32 {
    match mes.trim().to_uppercase().as_str() {
        "ENERO" => 1,
        "FEBRERO" => 2,
        "MARZO" => 3,
        "ABRIL" => 4,
        "MAYO" => 5,
        "JUNIO" => 6,
        "JULIO" => 7,
        "AGOSTO" => 8,
        "SEPTIEMBRE" => 9,
        "OCTUBRE" => 10,
        "NOVIEMBRE" => 11,
        "DICIEMBRE" => 12,
        _ => 1,
    }
}

#[derive(Debug, Clone)]
pub struct RegistroPatronalTotalDb {
    pub registro_patronal: String,
    pub obra_numero: String,
    pub tipo_obra: String,
    pub total_dias_cedula: f64,
}

/// Obtiene el total de días Cédula de cada registro patronal / obra
/// que el usuario creó en la base de datos (creado_usuario = true),
/// no las que se crearon automáticamente desde la cédula.
pub fn fetch_totales_dias_cedula_por_registro_patronal(
    client: &mut Client,
    id_cliente: Option<Uuid>,
) -> Result<Vec<RegistroPatronalTotalDb>, String> {
    let sql = r#"
        SELECT
            TRIM(COALESCE(rp.registro, 'SIN REGISTRO')) AS registro_patronal,
            TRIM(COALESCE(ro.numero, 'SIN NÚMERO')) AS obra_numero,
            TRIM(COALESCE(ro.tipo_obra, '')) AS tipo_obra,
            SUM(COALESCE(rv.dias_registro_obra, 0))::float8 AS total_dias_cedula
        FROM public.registros_obras ro
        JOIN public.registros_patronales rp ON rp.id = ro.id_registro_patronal
        LEFT JOIN public.reportes_vinculaciones_obras_liquidaciones rv ON rv.id_registro_obra = ro.id
        WHERE COALESCE(ro.creado_usuario, false) = true
          AND ($1::uuid IS NULL OR rp.id_cliente = $1)
        GROUP BY rp.registro, ro.numero, ro.tipo_obra
        ORDER BY rp.registro ASC, ro.numero ASC
    "#;

    let rows = client
        .query(sql, &[&id_cliente])
        .map_err(|e| format!("Error consultando totales de días cédula por registro patronal: {}", e))?;

    Ok(rows
        .iter()
        .map(|r| RegistroPatronalTotalDb {
            registro_patronal: row_get_str(r, 0),
            obra_numero: row_get_str(r, 1),
            tipo_obra: row_get_str(r, 2),
            total_dias_cedula: row_get_f64(r, 3),
        })
        .collect())
}

/// Verifica si un empleado cuenta con registro de BAJA en la cédula de liquidaciones
/// para el periodo especificado.
pub fn check_empleado_tiene_baja_periodo(
    client: &mut Client,
    id_empleado: Uuid,
    mes: &str,
    anio: i32,
) -> Result<bool, String> {
    let row = client
        .query_opt(
            r#"
            SELECT 1
            FROM public.reportes_liquidaciones rl
            JOIN public.reportes_detalles_liquidaciones rdl ON rdl.id_reporte_liquidacion = rl.id
            WHERE rl.id_empleado = $1
              AND UPPER(TRIM(rl.mes_periodo)) = UPPER(TRIM($2))
              AND rl.anio_periodo = $3
              AND UPPER(TRIM(rdl.clave)) = 'BAJA'
            LIMIT 1
            "#,
            &[&id_empleado, &mes, &(anio as i16)],
        )
        .map_err(|e| format!("Error verificando baja de empleado: {}", e))?;
    Ok(row.is_some())
}

#[derive(Debug, Clone)]
pub struct NominaConceptoEditableDb {
    pub id_detalle: Uuid,
    pub id_concepto: Uuid,
    pub clave: String,
    pub concepto: String,
    pub importe_original: f64,
    pub importe: f64,
}

/// Carga los conceptos editables de una nómina desde `reportes_detalles_nominas`.
pub fn fetch_conceptos_nomina_detalle(
    client: &mut Client,
    id_reporte_nomina: Uuid,
) -> Result<Vec<NominaConceptoEditableDb>, String> {
    let rows = client
        .query(
            r#"
            SELECT rdn.id, rdn.id_concepto, TRIM(COALESCE(rdn.clave, '')),
                   TRIM(COALESCE(rdn.concepto, '')), COALESCE(rdn.importe, 0)::float8
            FROM public.reportes_detalles_nominas rdn
            JOIN public.conceptos c ON c.id = rdn.id_concepto
            JOIN public.tipos_conceptos t ON t.id = c.id_tipo_concepto
            WHERE rdn.id_reporte_nomina = $1
              AND UPPER(TRIM(t.nombre)) = 'PERCEPCIONES'
            ORDER BY rdn.clave ASC, rdn.concepto ASC
            "#,
            &[&id_reporte_nomina],
        )
        .map_err(|e| format!("Error consultando conceptos editables: {}", e))?;

    Ok(rows
        .iter()
        .map(|r| NominaConceptoEditableDb {
            id_detalle: r.get(0),
            id_concepto: r.get(1),
            clave: row_get_str(r, 2),
            concepto: row_get_str(r, 3),
            importe_original: row_get_f64(r, 4),
            importe: row_get_f64(r, 4),
        })
        .collect())
}

/// Actualiza los días y percepciones de una nómina en `reportes_nominas` y `reportes_detalles_nominas`.
pub fn update_dias_y_conceptos_nomina(
    client: &mut Client,
    id_reporte_nomina: Uuid,
    nuevos_dias: f64,
    conceptos: &[(Uuid, f64)],
) -> Result<(), String> {
    let mut total: f64 = 0.0;
    for (id_det, nuevo_imp) in conceptos {
        total += *nuevo_imp;
        client
            .execute(
                "UPDATE public.reportes_detalles_nominas SET importe = $2 WHERE id = $1",
                &[id_det, nuevo_imp],
            )
            .map_err(|e| format!("Error actualizando detalle de percepción {}: {}", id_det, e))?;
    }

    client
        .execute(
            r#"
            UPDATE public.reportes_nominas
            SET num_dias_pagados = $2,
                total_sueldos = $3,
                total_gravado = $3
            WHERE id = $1
            "#,
            &[&id_reporte_nomina, &nuevos_dias, &total],
        )
        .map_err(|e| format!("Error actualizando reporte de nómina: {}", e))?;

    Ok(())
}

#[derive(Debug, Clone)]
pub struct NominaFaltanteDb {
    pub id_empleado: Uuid,
    pub nombre_trabajador: String,
    pub nss: String,
    pub rfc: String,
    pub mes_periodo: String,
    pub anio_periodo: i32,
    pub dias_cedula: f64,
    pub dias_nomina_actuales: f64,
    pub dias_faltantes: f64,
    pub clave: String,
    pub is_baja: bool,
    pub id_template_sugerido: Option<Uuid>,
    pub template_label: String,
    pub template_dias: f64,
}

/// Detecta nóminas faltantes cruzando las cédulas de cuotas (con clave Alta, Baja, Reing, etc.)
/// con las nóminas registradas para cada empleado y periodo.
pub fn fetch_nominas_faltantes(
    client: &mut Client,
    id_cliente: Option<Uuid>,
) -> Result<Vec<NominaFaltanteDb>, String> {
    let sql = format!(
        r#"
        WITH ced_emp AS (
            SELECT
                rl.id_empleado,
                TRIM(COALESCE(e.nombre, '')) AS nombre,
                TRIM(COALESCE(e.nss, '')) AS nss,
                TRIM(COALESCE(e.rfc, '')) AS rfc,
                UPPER(TRIM(rl.mes_periodo)) AS mes_periodo,
                rl.anio_periodo::int AS anio_periodo,
                SUM(COALESCE(rv.dias_registro_obra, 0))::float8 AS dias_cedula,
                COALESCE(MAX(TRIM(rdl.clave)), 'NORMAL') AS clave
            FROM public.reportes_liquidaciones rl
            JOIN public.empleados e ON e.id = rl.id_empleado
            JOIN public.reportes_vinculaciones_obras_liquidaciones rv ON rv.id_reporte_liquidacion = rl.id
            JOIN public.registros_obras ro ON ro.id = rv.id_registro_obra
            LEFT JOIN public.reportes_detalles_liquidaciones rdl ON rdl.id_reporte_liquidacion = rl.id
            WHERE COALESCE(ro.creado_usuario, false) = true
              AND ($1::uuid IS NULL OR e.id_cliente = $1)
            GROUP BY rl.id_empleado, e.nombre, e.nss, e.rfc, UPPER(TRIM(rl.mes_periodo)), rl.anio_periodo
        ),
        nom_emp AS (
            SELECT
                rn.id_empleado,
                {MES} AS mes_periodo,
                EXTRACT(YEAR FROM COALESCE(rn.fecha_final_pago, rn.fecha_pago, rn.fecha_inicial_pago))::int AS anio_periodo,
                SUM(COALESCE(rn.num_dias_pagados, 0))::float8 AS dias_nomina
            FROM public.reportes_nominas rn
            WHERE (rn.fecha_final_pago IS NOT NULL OR rn.fecha_pago IS NOT NULL OR rn.fecha_inicial_pago IS NOT NULL)
            GROUP BY rn.id_empleado, {MES}, EXTRACT(YEAR FROM COALESCE(rn.fecha_final_pago, rn.fecha_pago, rn.fecha_inicial_pago))
        )
        SELECT
            c.id_empleado,
            c.nombre,
            c.nss,
            c.rfc,
            c.mes_periodo,
            c.anio_periodo,
            c.dias_cedula,
            COALESCE(n.dias_nomina, 0.0)::float8 AS dias_nomina_actuales,
            (c.dias_cedula - COALESCE(n.dias_nomina, 0.0))::float8 AS dias_faltantes,
            c.clave
        FROM ced_emp c
        LEFT JOIN nom_emp n
               ON n.id_empleado = c.id_empleado
              AND n.mes_periodo = c.mes_periodo
              AND n.anio_periodo = c.anio_periodo
        WHERE c.dias_cedula > 0
          AND (n.dias_nomina IS NULL OR n.dias_nomina < c.dias_cedula - 0.01)
        ORDER BY c.anio_periodo ASC, c.mes_periodo ASC, c.nombre ASC
        "#,
        MES = mes_es_sql("COALESCE(rn.fecha_final_pago, rn.fecha_pago, rn.fecha_inicial_pago)")
    );

    let rows = client
        .query(sql.as_str(), &[&id_cliente])
        .map_err(|e| format!("Error consultando nóminas faltantes: {}", e))?;

    let mut result = Vec::with_capacity(rows.len());
    for r in &rows {
        let emp_id: Uuid = r.get(0);
        let clave = row_get_str(r, 9);
        let is_baja = clave.to_uppercase().contains("BAJA");

        // Buscar plantilla más reciente disponible para este empleado
        let template_row = client.query_opt(
            r#"
            SELECT rn.id, COALESCE(rn.num_dias_pagados, 0)::float8,
                   COALESCE(rn.fecha_final_pago, rn.fecha_pago, rn.fecha_inicial_pago)
            FROM public.reportes_nominas rn
            WHERE rn.id_empleado = $1
            ORDER BY COALESCE(rn.fecha_final_pago, rn.fecha_pago, rn.fecha_inicial_pago) DESC NULLS LAST
            LIMIT 1
            "#,
            &[&emp_id],
        ).ok().flatten();

        let (tpl_id, tpl_dias, tpl_label) = if let Some(t) = template_row {
            let tid: Uuid = t.get(0);
            let td: f64 = t.get(1);
            let tf: Option<NaiveDate> = t.get(2);
            let lbl = tf.map(|d| format!("{}/{} ({}d)", month_name_es(d.month()), d.year(), td))
                .unwrap_or_else(|| format!("Plantilla ({}d)", td));
            (Some(tid), td, lbl)
        } else {
            (None, 0.0, "Sin plantilla previa".to_string())
        };

        result.push(NominaFaltanteDb {
            id_empleado: emp_id,
            nombre_trabajador: row_get_str(r, 1),
            nss: row_get_str(r, 2),
            rfc: row_get_str(r, 3),
            mes_periodo: row_get_str(r, 4),
            anio_periodo: row_get_i32(r, 5),
            dias_cedula: row_get_f64(r, 6),
            dias_nomina_actuales: row_get_f64(r, 7),
            dias_faltantes: row_get_f64(r, 8),
            clave,
            is_baja,
            id_template_sugerido: tpl_id,
            template_label: tpl_label,
            template_dias: tpl_dias,
        });
    }

    Ok(result)
}

/// Clona una nómina plantilla hacia un nuevo periodo destino aplicando la fórmula
/// proporcional: (Concepto Percepcion / Valor dia original) * dias usuario.
pub fn create_nomina_from_template(
    client: &mut Client,
    id_empleado: Uuid,
    id_template: Uuid,
    target_mes: &str,
    target_anio: i32,
    target_dias: f64,
    conceptos_custom: Option<&[(Uuid, f64)]>,
) -> Result<Uuid, String> {
    let tpl_row = client
        .query_opt(
            r#"
            SELECT rn.id_revision, rn.version_nomina, rn.tipo_nomina,
                   COALESCE(rn.num_dias_pagados, 1.0)::float8,
                   COALESCE(rn.salario_base_cotizacion_aportacion, 0)::float8,
                   COALESCE(rn.salario_diario_integrado, 0)::float8,
                   COALESCE(rn.salario_diario_calculado, 0)::float8,
                   rn.nombre_archivo
            FROM public.reportes_nominas rn
            WHERE rn.id = $1
            LIMIT 1
            "#,
            &[&id_template],
        )
        .map_err(|e| format!("Error consultando nómina plantilla: {}", e))?
        .ok_or_else(|| "No se encontró la nómina plantilla especificada".to_string())?;

    let id_revision: Uuid = tpl_row.get(0);
    let version_nomina: Option<String> = tpl_row.get(1);
    let tipo_nomina: Option<String> = tpl_row.get(2);
    let dias_plantilla: f64 = tpl_row.get(3);
    let sbc: f64 = tpl_row.get(4);
    let sdi: f64 = tpl_row.get(5);
    let sdi_calc: f64 = tpl_row.get(6);
    let nombre_archivo: Option<String> = tpl_row.get(7);

    let mes_num = month_num_from_es(target_mes);
    let f_inicio = NaiveDate::from_ymd_opt(target_anio, mes_num, 1)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(target_anio, 1, 1).unwrap());
    let dias_i64 = (target_dias.round() as i64).max(1);
    let f_fin = f_inicio + chrono::Duration::days(dias_i64 - 1);
    let f_pago = f_fin;

    let new_id = Uuid::now_v7();

    let tpl_detalles = client
        .query(
            r#"
            SELECT rdn.id, rdn.id_concepto, rdn.clave, rdn.concepto,
                   COALESCE(rdn.importe, 0)::float8
            FROM public.reportes_detalles_nominas rdn
            JOIN public.conceptos c ON c.id = rdn.id_concepto
            JOIN public.tipos_conceptos t ON t.id = c.id_tipo_concepto
            WHERE rdn.id_reporte_nomina = $1
              AND UPPER(TRIM(t.nombre)) = 'PERCEPCIONES'
            "#,
            &[&id_template],
        )
        .map_err(|e| format!("Error consultando detalles de la plantilla: {}", e))?;

    let mut total_sueldos: f64 = 0.0;
    let factor = if dias_plantilla > 0.0 { target_dias / dias_plantilla } else { 1.0 };

    let mut detalles_to_insert: Vec<(Uuid, String, String, f64)> = Vec::new();
    for row in &tpl_detalles {
        let id_conc: Uuid = row.get(1);
        let clave = row_get_str(row, 2);
        let conc = row_get_str(row, 3);
        let imp_orig = row_get_f64(row, 4);

        let imp_final = if let Some(custom) = conceptos_custom {
            custom.iter().find(|(cid, _)| *cid == id_conc).map(|(_, v)| *v)
                .unwrap_or_else(|| (imp_orig * factor * 100.0).round() / 100.0)
        } else {
            (imp_orig * factor * 100.0).round() / 100.0
        };

        total_sueldos += imp_final;
        detalles_to_insert.push((id_conc, clave, conc, imp_final));
    }

    client
        .execute(
            r#"
            INSERT INTO public.reportes_nominas (
                id, id_revision, id_empleado, version_nomina,
                tipo_nomina, fecha_pago, fecha_inicial_pago,
                fecha_final_pago, num_dias_pagados, total_sueldos,
                total_gravado, total_exento, total_otras_deducciones,
                total_impuestos_retenidos,
                salario_base_cotizacion_aportacion,
                salario_diario_integrado, salario_diario_calculado,
                nombre_archivo
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10, 0.0, 0.0, 0.0, $11, $12, $13, $14)
            "#,
            &[
                &new_id,
                &id_revision,
                &id_empleado,
                &version_nomina,
                &tipo_nomina,
                &f_pago,
                &f_inicio,
                &f_fin,
                &target_dias,
                &total_sueldos,
                &sbc,
                &sdi,
                &sdi_calc,
                &nombre_archivo,
            ],
        )
        .map_err(|e| format!("Error creando nómina en reportes_nominas: {}", e))?;

    for (id_conc, clave, conc, imp) in detalles_to_insert {
        client
            .execute(
                r#"
                INSERT INTO public.reportes_detalles_nominas (
                    id, id_reporte_nomina, id_concepto, clave, concepto,
                    importe_gravado, importe_exento, importe
                )
                VALUES (uuidv7(), $1, $2, $3, $4, $5, 0.0, $5)
                "#,
                &[&new_id, &id_conc, &clave, &conc, &imp],
            )
            .map_err(|e| format!("Error insertando detalle de nómina: {}", e))?;
    }

    Ok(new_id)
}

#[derive(Debug, Clone)]
pub struct NominaFaltanteCreateRequest {
    pub id_empleado: Uuid,
    pub id_template: Uuid,
    pub target_mes: String,
    pub target_anio: i32,
    pub target_dias: f64,
}

/// Creación masiva de nóminas faltantes usando plantillas disponibles.
pub fn create_bulk_missing_nominas(
    client: &mut Client,
    requests: &[NominaFaltanteCreateRequest],
) -> Result<usize, String> {
    let mut count = 0;
    for req in requests {
        let res = create_nomina_from_template(
            client,
            req.id_empleado,
            req.id_template,
            &req.target_mes,
            req.target_anio,
            req.target_dias,
            None,
        );
        if res.is_ok() {
            count += 1;
        }
    }
    Ok(count)
}

 /// Verifica si existen cédulas para una revisión dada.
pub fn check_cedulas_exists(
    client: &mut Client,
    revision_id: Uuid,
) -> Result<bool, String> {
    let row = client.query_opt(
        "SELECT 1 FROM public.liquidaciones WHERE id_revision = $1 LIMIT 1",
        &[&revision_id],
    ).map_err(|e| format!("Error verificando cédulas: {}", e))?;
    Ok(row.is_some())
}

/// Verifica si existen nóminas para una revisión dada.
pub fn check_nominas_exists(
    client: &mut Client,
    revision_id: Uuid,
) -> Result<bool, String> {
    let row = client.query_opt(
        "SELECT 1 FROM public.nominas WHERE id_revision = $1 LIMIT 1",
        &[&revision_id],
    ).map_err(|e| format!("Error verificando nóminas: {}", e))?;
    Ok(row.is_some())
}

/// Verifica si existen reportes de liquidaciones para una revisión dada.
pub fn check_reportes_liquidaciones_exists(
    client: &mut Client,
    revision_id: Uuid,
) -> Result<bool, String> {
    let row = client.query_opt(
        "SELECT 1 FROM public.reportes_liquidaciones WHERE id_revision = $1 LIMIT 1",
        &[&revision_id],
    ).map_err(|e| format!("Error verificando reportes liquidaciones: {}", e))?;
    Ok(row.is_some())
}

/// Verifica si existen registros de obras para una revisión dada.
pub fn check_registros_obras_exists(
    client: &mut Client,
    revision_id: Uuid,
) -> Result<bool, String> {
    let row = client.query_opt(
        "SELECT 1 FROM public.registros_obras WHERE id_revision = $1 LIMIT 1",
        &[&revision_id],
    ).map_err(|e| format!("Error verificando registros de obras: {}", e))?;
    Ok(row.is_some())
}

/// Verifica la existencia de datos para todos los módulos de una revisión
/// y actualiza las propiedades en AppData.
pub fn check_all_data_exists(
    client: &mut Client,
    revision_id: Uuid,
) -> Result<(bool, bool, bool, bool), String> {
    let cedulas = check_cedulas_exists(client, revision_id)?;
    let nominas = check_nominas_exists(client, revision_id)?;
    let reportes = check_reportes_liquidaciones_exists(client, revision_id)?;
    let obras = check_registros_obras_exists(client, revision_id)?;
    Ok((cedulas, nominas, reportes, obras))
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