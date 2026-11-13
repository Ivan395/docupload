use chrono::{Datelike, NaiveDate};
use postgres::{Client, Transaction};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use uuid::Uuid;

use crate::cedulas::business::{mes_to_periodo_int, month_num_to_name, month_to_num};
use crate::db::bitacora_reportes::{
    self, ACCION_ASIGNAR_LOTE, ACCION_DESVINCULAR, ACCION_DIAS, ACCION_REVERTIR, ACCION_VINCULAR,
};

#[derive(Debug, Clone)]
pub struct ObraOptionDb {
    pub id: Uuid,
    pub numero: String,
    pub tipo_obra: String,
    pub fecha_inicio: Option<NaiveDate>,
    pub fecha_fin: Option<NaiveDate>,
    pub label: String,
    pub creado_usuario: bool,
    pub monto: f64,
}

#[derive(Debug, Clone)]
pub struct ReporteGuarderiaKpiDb {
    pub id_obra: Uuid,
    pub obra_numero: String,
    pub obra_label: String,
    pub registros_patronales_str: String,
    pub total_gps: f64,
    pub valor_guarderias: f64,
    pub monto_obra: f64,
    pub limite_18_obra: f64,
    pub supera_18: bool,
    pub sin_monto: bool,
    pub empleados_count: usize,
}

/// Calcula los KPIs de Guarderías (Tope 18%) para cada registro de obra aplicable
pub fn compute_guarderias_kpis(
    rows: &[ReporteLiquidacionRowDb],
    obras_options: &[ObraOptionDb],
    filter_obra_id: Option<Uuid>,
) -> Vec<ReporteGuarderiaKpiDb> {
    let mut result = Vec::new();
    for o in obras_options.iter() {
        // Excluir estrictamente cualquier registro de obra que no haya sido creado manualmente por el usuario
        if !o.creado_usuario {
            continue;
        }
        if let Some(target_id) = filter_obra_id {
            if o.id != target_id {
                continue;
            }
        }
        let mut total_gps_obra = 0.0;
        let mut emps_count = 0;
        let mut rp_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for r in rows {
            if r.id_registro_obra == Some(o.id)
                || (!r.obra_numero.trim().is_empty() && r.obra_numero.trim().eq_ignore_ascii_case(o.numero.trim()))
            {
                total_gps_obra += r.gps;
                emps_count += 1;
                if !r.registro_patronal.trim().is_empty() {
                    rp_set.insert(r.registro_patronal.trim().to_string());
                }
            }
        }
        let rp_vec: Vec<String> = rp_set.into_iter().take(3).collect();
        let registros_patronales_str = if rp_vec.is_empty() {
            "Reg. Patronal: S/D".to_string()
        } else {
            format!("Reg. Patronales: {}", rp_vec.join(" • "))
        };
        let monto_obra = o.monto;
        let limite_18 = monto_obra * 0.18;
        // Para calcular el valor de las guarderías es el total de gps de todos los empleados vinculados al registro de obra dividido entre 1% (0.01):
        let valor_guarderias = total_gps_obra / 0.01;
        // Sin monto capturado (0.0 o ausente) no hay base para el 18%: no se marca alerta.
        let sin_monto = monto_obra.is_nan() || monto_obra <= 0.0;
        let supera_18 = !sin_monto && valor_guarderias > limite_18;

        result.push(ReporteGuarderiaKpiDb {
            id_obra: o.id,
            obra_numero: o.numero.clone(),
            obra_label: o.label.clone(),
            registros_patronales_str,
            total_gps: total_gps_obra,
            valor_guarderias,
            monto_obra,
            limite_18_obra: limite_18,
            supera_18,
            sin_monto,
            empleados_count: emps_count,
        });
    }
    result
}

#[derive(Debug, Clone)]
pub struct ReporteLiquidacionRowDb {
    pub id_reporte_liq: Uuid,
    pub id_parent_reporte_liq: Uuid,
    pub id_liquidacion: Uuid,
    pub id_revision: Uuid,
    pub id_empleado: Uuid,
    pub nombre_trabajador: String,
    pub rfc: String,
    pub curp: String,
    pub nss: String,
    pub pensionado: String,
    pub registro_patronal: String,
    pub mes_periodo: String,
    pub mes_periodo_int: i16,
    pub anio_periodo: i16,
    pub periodo_str: String,
    pub dias_cedula: i16,
    pub id_registro_obra: Option<Uuid>,
    pub obra_numero: String,
    pub obra_label: String,
    pub dias_obra: i16,
    pub obra_creado_usuario: bool,

    pub clave: String,
    pub fecha: Option<NaiveDate>,
    pub fecha_str: String,
    pub sdi: f64,
    pub lic: f64,
    pub inc: f64,
    pub aus: f64,

    // Prorrateados
    pub cf: f64,
    pub exc_pat: f64,
    pub exc_obr: f64,
    pub pd_pat: f64,
    pub pd_obr: f64,
    pub gmp_pat: f64,
    pub gmp_obr: f64,
    pub rt: f64,
    pub iv_pat: f64,
    pub iv_obr: f64,
    pub gps: f64,
    pub patronal: f64,
    pub obrera: f64,
    pub subtotal: f64,

    // Originales
    pub cf_orig: f64,
    pub exc_pat_orig: f64,
    pub exc_obr_orig: f64,
    pub pd_pat_orig: f64,
    pub pd_obr_orig: f64,
    pub gmp_pat_orig: f64,
    pub gmp_obr_orig: f64,
    pub rt_orig: f64,
    pub iv_pat_orig: f64,
    pub iv_obr_orig: f64,
    pub gps_orig: f64,
    pub patronal_orig: f64,
    pub obrera_orig: f64,
    pub subtotal_orig: f64,

    // Auditoría e inconsistencias
    pub tiene_inconsistencia: bool,
    pub observacion_validacion: String,

    // Estado de reporte y marcas de auditoría (columnas de public.reportes_liquidaciones)
    pub estado: String,
    pub cedula_actualizada: bool,
    pub editado_usuario: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ReporteLiquidacionSummaryDb {
    pub total_registros: usize,
    pub total_obras_revision: usize,
    pub subtotal_imss_original: f64,
    pub subtotal_prorrateado: f64,
    pub diferencia_ajuste: f64,
    pub porcentaje_diferencia: f64,
    pub subtitle: String,
}

fn row_get_i16(r: &postgres::Row, idx: usize, default: i16) -> i16 {
    if let Ok(v) = r.try_get::<_, i16>(idx) {
        return v;
    }
    if let Ok(v) = r.try_get::<_, i32>(idx) {
        return v as i16;
    }
    if let Ok(v) = r.try_get::<_, i64>(idx) {
        return v as i16;
    }
    if let Ok(Some(v)) = r.try_get::<_, Option<i16>>(idx) {
        return v;
    }
    if let Ok(Some(v)) = r.try_get::<_, Option<i32>>(idx) {
        return v as i16;
    }
    default
}

fn row_get_f64(r: &postgres::Row, idx: usize, default: f64) -> f64 {
    if let Ok(v) = r.try_get::<_, f64>(idx) {
        return v;
    }
    if let Ok(Some(v)) = r.try_get::<_, Option<f64>>(idx) {
        return v;
    }
    if let Ok(v) = r.try_get::<_, i64>(idx) {
        return v as f64;
    }
    if let Ok(v) = r.try_get::<_, i32>(idx) {
        return v as f64;
    }
    default
}

/// Contexto resuelto de Registro Patronal para una revisión o directamente para un Registro Patronal:
/// (rev_uuid_opt, primary_rp_uuid, clean_rp_registro, all_matching_rp_uuids)
pub fn resolve_rp_context(
    client: &mut Client,
    id_target: Uuid,
) -> Result<(Option<Uuid>, Uuid, String, Vec<Uuid>), String> {
    // 1. Intentar como ID de revisión
    let rev_row = client.query_opt(
        r#"
        SELECT r.id, r.id_registro_patronal, COALESCE(rp.registro, '')
        FROM public.revisiones r
        JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
        WHERE r.id = $1
        "#,
        &[&id_target],
    ).map_err(|e| format!("Error consultando revisión {}: {}", id_target, e))?;

    let (rev_uuid_opt, id_rp, rp_reg) = if let Some(r) = rev_row {
        let rev_id: Uuid = r.get(0);
        let rp_id: Uuid = r.get(1);
        let reg: String = r.get(2);
        (Some(rev_id), rp_id, reg)
    } else {
        // 2. Intentar como ID de registro patronal directo
        let rp_row = client.query_opt(
            "SELECT id, COALESCE(registro, '') FROM public.registros_patronales WHERE id = $1",
            &[&id_target],
        ).map_err(|e| format!("Error consultando registro patronal {}: {}", id_target, e))?;

        if let Some(r) = rp_row {
            let rp_id: Uuid = r.get(0);
            let reg: String = r.get(1);
            (None, rp_id, reg)
        } else {
            return Err(format!("No se encontró registro patronal o revisión para ID {}", id_target));
        }
    };

    let clean_reg = rp_reg.trim().to_uppercase();
    let mut all_rp_uuids = vec![id_rp];
    if !clean_reg.is_empty() {
        for r in client.query(
            "SELECT id FROM public.registros_patronales WHERE UPPER(TRIM(registro)) = UPPER(TRIM($1)) OR id = $2",
            &[&clean_reg, &id_rp],
        ).unwrap_or_default() {
            let u: Uuid = r.get(0);
            if !all_rp_uuids.contains(&u) {
                all_rp_uuids.push(u);
            }
        }
    }

    Ok((rev_uuid_opt, id_rp, clean_reg, all_rp_uuids))
}

pub fn resolve_rp_context_tx(
    tx: &mut Transaction,
    id_target: Uuid,
) -> Result<(Option<Uuid>, Uuid, String, Vec<Uuid>), String> {
    let rev_row = tx.query_opt(
        r#"
        SELECT r.id, r.id_registro_patronal, COALESCE(rp.registro, '')
        FROM public.revisiones r
        JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
        WHERE r.id = $1
        "#,
        &[&id_target],
    ).map_err(|e| format!("Error consultando revisión {}: {}", id_target, e))?;

    let (rev_uuid_opt, id_rp, rp_reg) = if let Some(r) = rev_row {
        let rev_id: Uuid = r.get(0);
        let rp_id: Uuid = r.get(1);
        let reg: String = r.get(2);
        (Some(rev_id), rp_id, reg)
    } else {
        let rp_row = tx.query_opt(
            "SELECT id, COALESCE(registro, '') FROM public.registros_patronales WHERE id = $1",
            &[&id_target],
        ).map_err(|e| format!("Error consultando registro patronal {}: {}", id_target, e))?;

        if let Some(r) = rp_row {
            let rp_id: Uuid = r.get(0);
            let reg: String = r.get(1);
            (None, rp_id, reg)
        } else {
            return Err(format!("No se encontró registro patronal o revisión para ID {}", id_target));
        }
    };

    let clean_reg = rp_reg.trim().to_uppercase();
    let mut all_rp_uuids = vec![id_rp];
    if !clean_reg.is_empty() {
        for r in tx.query(
            "SELECT id FROM public.registros_patronales WHERE UPPER(TRIM(registro)) = UPPER(TRIM($1)) OR id = $2",
            &[&clean_reg, &id_rp],
        ).unwrap_or_default() {
            let u: Uuid = r.get(0);
            if !all_rp_uuids.contains(&u) {
                all_rp_uuids.push(u);
            }
        }
    }

    Ok((rev_uuid_opt, id_rp, clean_reg, all_rp_uuids))
}

/// Obtiene los registros de obra asociados a una revisión o registro patronal
pub fn fetch_obras_for_revision(client: &mut Client, id_revision: Uuid) -> Result<Vec<ObraOptionDb>, String> {
    let (rev_uuid_opt, _id_rp, _clean_reg, all_rp_uuids) = resolve_rp_context(client, id_revision)?;

    let rows = client.query(
        r#"
        SELECT ro.id, ro.numero, ro.tipo_obra, ro.fecha_inicio, ro.fecha_fin,
               COALESCE(ro.creado_usuario, false) AS creado_usuario,
               COALESCE(ro.monto, 0.0) AS monto
        FROM public.registros_obras ro
        WHERE ro.activa AND (
            ($1::uuid IS NOT NULL AND (
                ro.id_revision = $1
                OR ro.id IN (
                    SELECT rdl.id_registro_obra
                    FROM public.reportes_detalles_liquidaciones rdl
                    JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
                    WHERE rl.id_revision = $1 AND rdl.id_registro_obra IS NOT NULL
                )
                OR ro.id IN (
                    SELECT rvol.id_registro_obra
                    FROM public.reportes_vinculaciones_obras_liquidaciones rvol
                    JOIN public.reportes_liquidaciones rl ON rl.id = rvol.id_reporte_liquidacion
                    WHERE rl.id_revision = $1 AND rvol.id_registro_obra IS NOT NULL
                )
            ))
            OR (
                $1::uuid IS NULL AND (
                    ro.id_registro_patronal = ANY($2)
                    OR ro.id IN (
                        SELECT rdl.id_registro_obra
                        FROM public.reportes_detalles_liquidaciones rdl
                        JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
                        WHERE rl.id_registro_patronal = ANY($2) AND rdl.id_registro_obra IS NOT NULL
                    )
                    OR ro.id IN (
                        SELECT rvol.id_registro_obra
                        FROM public.reportes_vinculaciones_obras_liquidaciones rvol
                        JOIN public.reportes_liquidaciones rl ON rl.id = rvol.id_reporte_liquidacion
                        WHERE rl.id_registro_patronal = ANY($2) AND rvol.id_registro_obra IS NOT NULL
                    )
                )
            )
        )
        ORDER BY ro.numero ASC
        "#,
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| format!("Error consultando registros de obra: {}", e))?;

    let mut result = Vec::new();
    for r in rows {
        let id: Uuid = r.get(0);
        let num: String = r.get(1);
        let tipo: Option<String> = r.try_get(2).ok().flatten();
        let f_ini: Option<NaiveDate> = r.try_get(3).ok().flatten();
        let f_fin: Option<NaiveDate> = r.try_get(4).ok().flatten();
        let creado_usuario: bool = r.try_get(5).unwrap_or(false);
        let monto: f64 = r.try_get(6).unwrap_or(0.0);
        let tipo_clean = tipo.unwrap_or_default();
        let label = if tipo_clean.trim().is_empty() {
            num.trim().to_string()
        } else {
            format!("{} - {}", num.trim(), tipo_clean.trim())
        };
        result.push(ObraOptionDb {
            id,
            numero: num.trim().to_string(),
            tipo_obra: tipo_clean.trim().to_string(),
            fecha_inicio: f_ini,
            fecha_fin: f_fin,
            label,
            creado_usuario,
            monto,
        });
    }
    Ok(result)
}

/// Obtiene todos los registros de obra (creados por el usuario y no creados por el usuario)
/// asociados a la revisión o registro patronal, o referenciados en sus liquidaciones.
pub fn fetch_all_obras_for_revision(client: &mut Client, id_revision: Uuid) -> Result<Vec<ObraOptionDb>, String> {
    let (rev_uuid_opt, _id_rp, _clean_reg, all_rp_uuids) = resolve_rp_context(client, id_revision)?;

    let rows = client.query(
        r#"
        SELECT ro.id, ro.numero, ro.tipo_obra, ro.fecha_inicio, ro.fecha_fin,
               COALESCE(ro.creado_usuario, false) AS creado_usuario,
               COALESCE(ro.monto, 0.0) AS monto
        FROM public.registros_obras ro
        WHERE ro.activa AND (
            ($1::uuid IS NOT NULL AND (
                ro.id_revision = $1
                OR ro.id IN (
                    SELECT rdl.id_registro_obra
                    FROM public.reportes_detalles_liquidaciones rdl
                    JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
                    WHERE rl.id_revision = $1 AND rdl.id_registro_obra IS NOT NULL
                )
                OR ro.id IN (
                    SELECT rvol.id_registro_obra
                    FROM public.reportes_vinculaciones_obras_liquidaciones rvol
                    JOIN public.reportes_liquidaciones rl ON rl.id = rvol.id_reporte_liquidacion
                    WHERE rl.id_revision = $1 AND rvol.id_registro_obra IS NOT NULL
                )
            ))
            OR (
                $1::uuid IS NULL AND (
                    ro.id_registro_patronal = ANY($2)
                    OR ro.id IN (
                        SELECT rdl.id_registro_obra
                        FROM public.reportes_detalles_liquidaciones rdl
                        JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
                        WHERE rl.id_registro_patronal = ANY($2) AND rdl.id_registro_obra IS NOT NULL
                    )
                    OR ro.id IN (
                        SELECT rvol.id_registro_obra
                        FROM public.reportes_vinculaciones_obras_liquidaciones rvol
                        JOIN public.reportes_liquidaciones rl ON rl.id = rvol.id_reporte_liquidacion
                        WHERE rl.id_registro_patronal = ANY($2) AND rvol.id_registro_obra IS NOT NULL
                    )
                )
            )
        )
        ORDER BY ro.creado_usuario DESC, ro.numero ASC
        "#,
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| format!("Error consultando todos los registros de obra: {}", e))?;

    let mut result = Vec::new();
    for r in rows {
        let id: Uuid = r.get(0);
        let num: String = r.get(1);
        let tipo: Option<String> = r.try_get(2).ok().flatten();
        let f_ini: Option<NaiveDate> = r.try_get(3).ok().flatten();
        let f_fin: Option<NaiveDate> = r.try_get(4).ok().flatten();
        let creado_usuario: bool = r.try_get(5).unwrap_or(false);
        let monto: f64 = r.try_get(6).unwrap_or(0.0);
        let tipo_clean = tipo.unwrap_or_default();
        let label = if tipo_clean.trim().is_empty() {
            num.trim().to_string()
        } else {
            format!("{} - {}", num.trim(), tipo_clean.trim())
        };
        result.push(ObraOptionDb {
            id,
            numero: num.trim().to_string(),
            tipo_obra: tipo_clean.trim().to_string(),
            fecha_inicio: f_ini,
            fecha_fin: f_fin,
            label,
            creado_usuario,
            monto,
        });
    }
    Ok(result)
}

/// Extrae el conjunto de claves exactas de periodo ("MES/AÑO", ej. "ENERO/2024")
/// correspondientes a los rangos de fechas (fecha_inicio, fecha_fin) de las obras creadas por el usuario.
pub fn extract_period_keys_from_obras(obras: &[ObraOptionDb]) -> BTreeSet<String> {
    let mut periods = BTreeSet::new();
    for o in obras {
        let f_ini_opt = o.fecha_inicio;
        let f_fin_opt = o.fecha_fin.or(f_ini_opt);
        if let (Some(f_ini), Some(f_fin)) = (f_ini_opt, f_fin_opt) {
            let mut curr_y = f_ini.year();
            let mut curr_m = f_ini.month();
            let end_y = f_fin.year();
            let end_m = f_fin.month();

            while (curr_y < end_y) || (curr_y == end_y && curr_m <= end_m) {
                let mes_nom = month_num_to_name(curr_m);
                if !mes_nom.is_empty() {
                    let m_upper = mes_nom.to_uppercase();
                    periods.insert(format!("{}/{}", m_upper, curr_y));
                    periods.insert(format!("{:02}/{}", curr_m, curr_y));
                    periods.insert(format!("{}/{}", curr_m, curr_y));
                }
                curr_m += 1;
                if curr_m > 12 {
                    curr_m = 1;
                    curr_y += 1;
                }
            }
        }
    }
    periods
}

/// Detalle de una inconsistencia detectada entre periodos de ejecución de obras del usuario y cédulas faltantes
#[derive(Debug, Clone, Default)]
pub struct InconsistenciaDetalleDb {
    pub periodo: String,
    pub mes_anio: String,
    pub obras_vinculadas: String,
    pub obras_count: usize,
}

/// Diagnóstico detallado de cédulas de cuotas faltantes vs periodos de ejecución de obras
#[derive(Debug, Clone, Default)]
pub struct MissingCedulasDiagnostic {
    pub has_missing: bool,
    pub missing_periods: Vec<String>,
    pub inconsistencias: Vec<InconsistenciaDetalleDb>,
    pub summary_msg: String,
    pub reason_msg: String,
}

/// Diagnóstico de cédulas de cuotas faltantes en base a los periodos de vigencia de los registros de obra (ROs)
/// creados por el usuario comparados contra las cédulas (liquidaciones) registradas en el Registro Patronal.
pub fn check_missing_cedulas_for_revision(
    client: &mut Client,
    id_target: Uuid,
) -> Result<MissingCedulasDiagnostic, String> {
    let (rev_uuid_opt, _id_rp, clean_reg, all_rp_uuids) = resolve_rp_context(client, id_target)?;

    // 1. Obtener los rangos de fecha y etiquetas de las obras CREADAS POR EL USUARIO correspondientes a esta revisión o RP
    let obra_rows = client.query(
        r#"
        SELECT 
            COALESCE(ro.numero, '') AS numero,
            COALESCE(ro.tipo_obra, '') AS tipo_obra,
            ro.fecha_inicio,
            ro.fecha_fin
        FROM public.registros_obras ro
        WHERE (
            ($1::uuid IS NOT NULL AND ro.id_revision = $1)
            OR ($1::uuid IS NULL AND ro.id_registro_patronal = ANY($2))
        )
        AND COALESCE(ro.creado_usuario, false) = true
        AND ro.activa
        ORDER BY ro.fecha_inicio ASC
        "#,
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| format!("Error consultando registros de obra creados por el usuario para verificación de cédulas: {}", e))?;

    let mut month_to_obras: std::collections::BTreeMap<(i32, i16), BTreeSet<String>> = std::collections::BTreeMap::new();
    for r in &obra_rows {
        let num: String = r.get(0);
        let tipo: String = r.get(1);
        let f_ini: Option<NaiveDate> = r.get(2);
        let f_fin: Option<NaiveDate> = r.get(3);

        let obra_label = if !num.trim().is_empty() && !tipo.trim().is_empty() {
            format!("{} ({})", num.trim(), tipo.trim())
        } else if !num.trim().is_empty() {
            num.trim().to_string()
        } else if !tipo.trim().is_empty() {
            tipo.trim().to_string()
        } else {
            "Obra sin número".to_string()
        };

        if let Some(ini) = f_ini {
            let fin = f_fin.unwrap_or(ini).max(ini);
            let mut curr_y = ini.year();
            let mut curr_m = ini.month() as i16;
            let end_y = fin.year();
            let end_m = fin.month() as i16;
            while (curr_y < end_y) || (curr_y == end_y && curr_m <= end_m) {
                month_to_obras.entry((curr_y, curr_m)).or_default().insert(obra_label.clone());
                curr_m += 1;
                if curr_m > 12 {
                    curr_m = 1;
                    curr_y += 1;
                }
            }
        }
    }

    if month_to_obras.is_empty() {
        return Ok(MissingCedulasDiagnostic::default());
    }

    // 2. Consultar periodos disponibles en liquidaciones (usando mes_periodo_int numérico con fallback a mes_periodo)
    let avail_rows = client
        .query(
            "SELECT DISTINCT mes_periodo_int, mes_periodo, anio_periodo FROM public.liquidaciones WHERE id_registro_patronal = ANY($1)",
            &[&all_rp_uuids],
        )
        .map_err(|e| {
            format!(
                "Error consultando periodos de liquidaciones disponibles: {}",
                crate::db::schema::format_pg_error(&e)
            )
        })?;

    let mut available_periods: BTreeSet<(i32, i16)> = BTreeSet::new();
    for r in &avail_rows {
        let m_int_db: Option<i16> = r.try_get(0).ok().flatten();
        let m_str: Option<String> = r.try_get(1).ok().flatten();
        let y_val: Option<i16> = r.try_get(2).ok().flatten();
        if let Some(y) = y_val {
            let m_int = match m_int_db {
                Some(v) if (1..=12).contains(&v) => v,
                _ => m_str.as_deref().map(mes_to_periodo_int).unwrap_or(0),
            };
            if (1..=12).contains(&m_int) {
                available_periods.insert((y as i32, m_int));
            }
        }
    }

    // 3. Diferencia: periodos requeridos por las obras del usuario que no cuentan con cédula (comparación numérica exacta (i32, i16))
    let mut missing_periods = Vec::new();
    let mut inconsistencias = Vec::new();

    for (period, obras_set) in &month_to_obras {
        if !available_periods.contains(period) {
            let (y, m_int) = *period;
            let m = m_int as u32;
            let mes_nombre = month_num_to_name(m);
            let mes_anio = format!("{:04}/{:02}", y, m);
            let periodo_str = format!("{:04}/{:02} - {} {}", y, m, mes_nombre, y);
            let obras_list: Vec<String> = obras_set.iter().cloned().collect();
            let obras_count = obras_list.len();
            let obras_vinculadas = obras_list.join(", ");

            inconsistencias.push(InconsistenciaDetalleDb {
                periodo: periodo_str.clone(),
                mes_anio,
                obras_vinculadas,
                obras_count,
            });
            missing_periods.push(periodo_str);
        }
    }

    let has_missing = !inconsistencias.is_empty();
    let count = inconsistencias.len();

    let summary_msg = if has_missing {
        if count == 1 {
            "Falta 1 cédula de cuotas por ingresar en los periodos de obra. El reporte estará incompleto.".to_string()
        } else {
            format!("Faltan {} cédulas de cuotas por ingresar en los periodos de obra. El reporte estará incompleto.", count)
        }
    } else {
        String::new()
    };

    let reason_msg = if has_missing {
        let rp_suffix = if !clean_reg.is_empty() {
            format!(" correspondientes al Registro Patronal {}.", clean_reg)
        } else {
            " correspondientes al Registro Patronal.".to_string()
        };
        format!(
            "Se detectaron registros de obra activos creados por el usuario en estos periodos, pero no se cargaron las cédulas de cuotas mensuales (SUA/Emisión IMSS){rp_suffix} Conforme al RACERF y a la normativa SIROC del IMSS, estas cédulas son indispensables para validar las emisiones, determinar las bases salariales y efectuar el prorrateo de cuotas obrero-patronales en los periodos de ejecución de las obras.",
        )
    } else {
        String::new()
    };

    Ok(MissingCedulasDiagnostic {
        has_missing,
        missing_periods,
        inconsistencias,
        summary_msg,
        reason_msg,
    })
}


/// Expresión SQL del mes normalizado (1..12) de `public.reportes_liquidaciones` (alias `rl`).
const SQL_MES_RL: &str = "LEAST(GREATEST(COALESCE(NULLIF(rl.mes_periodo_int, 0), CASE UPPER(TRIM(rl.mes_periodo)) WHEN 'ENERO' THEN 1 WHEN 'FEBRERO' THEN 2 WHEN 'MARZO' THEN 3 WHEN 'ABRIL' THEN 4 WHEN 'MAYO' THEN 5 WHEN 'JUNIO' THEN 6 WHEN 'JULIO' THEN 7 WHEN 'AGOSTO' THEN 8 WHEN 'SEPTIEMBRE' THEN 9 WHEN 'OCTUBRE' THEN 10 WHEN 'NOVIEMBRE' THEN 11 WHEN 'DICIEMBRE' THEN 12 ELSE 1 END)::int, 1), 12)";

/// Condición SQL booleana: el mes (`anio`, `mes`, expresiones enteras) pertenece al universo de la
/// revisión indicada por `rev`, que es una expresión SQL del id de revisión (p. ej. `"$3"` o
/// `"rl.id_revision"`). Granularidad mensual (mes completo), INTERSECCIÓN de:
/// - `en_periodo`: si la revisión tiene `periodo_inicio` y `periodo_fin`, el mes debe traslapar ese
///   rango; si no, true.
/// - `en_obras` (decisión D1): el mes debe traslapar la vigencia de ALGÚN Registro de Obra del usuario
///   (`creado_usuario = true` y `activa`) de la revisión. Reglas por RO:
///   - Sin RO del usuario en la revisión ⇒ universo vacío (false).
///   - `fecha_inicio` NULL ⇒ la RO no aporta meses.
///   - `fecha_fin` NULL ⇒ la RO cubre solo el mes de `fecha_inicio`.
///   - `fecha_fin < fecha_inicio` ⇒ se usa `fecha_inicio` como fin (GREATEST).
///   Los traslapes entre RO se unen naturalmente por el EXISTS.
/// Resultado: `en_periodo AND en_obras`.
///
/// Los marcadores son comentarios de bloque (`/* ... */`) y no de línea, para no depender de los saltos
/// de línea del SQL que se envía.
fn sql_en_universo(rev: &str, anio: &str, mes: &str) -> String {
    let md = format!("MAKE_DATE(({anio})::int, ({mes})::int, 1)");
    let fin_mes = format!("({md} + INTERVAL '1 month' - INTERVAL '1 day')::date");
    format!(
        r#"(
            /* en_periodo */
            CASE
                WHEN (SELECT rv.periodo_inicio IS NOT NULL AND rv.periodo_fin IS NOT NULL FROM public.revisiones rv WHERE rv.id = {rev}) THEN
                    {md} <= (SELECT rv.periodo_fin FROM public.revisiones rv WHERE rv.id = {rev})
                    AND {fin_mes} >= (SELECT rv.periodo_inicio FROM public.revisiones rv WHERE rv.id = {rev})
                ELSE true
            END
            AND
            /* en_obras */
            EXISTS (
                SELECT 1 FROM public.registros_obras ro
                WHERE ro.id_revision = {rev}
                  AND COALESCE(ro.creado_usuario, false) = true
                  AND ro.activa
                  AND ro.fecha_inicio IS NOT NULL
                  AND {md} <= GREATEST(COALESCE(ro.fecha_fin, ro.fecha_inicio), ro.fecha_inicio)
                  AND {fin_mes} >= ro.fecha_inicio
            )
        )"#
    )
}

/// Reevalúa ACTIVO / FUERA_PERIODO de los reportes según el universo de SU PROPIA revisión (rl.id_revision).
/// No inserta ni borra. Devuelve (pasaron_a_fuera, regresaron_a_activo).
pub fn reevaluar_universo_tx(
    tx: &mut Transaction,
    rev_uuid_opt: Option<Uuid>,
    all_rp_uuids: &[Uuid],
) -> Result<(u64, u64), String> {
    let en_universo_rl = sql_en_universo("rl.id_revision", "rl.anio_periodo", SQL_MES_RL);

    let sql_marcar_fuera = format!(
        r#"
        UPDATE public.reportes_liquidaciones rl
        SET estado = 'FUERA_PERIODO'
        WHERE rl.id_registro_patronal = ANY($2)
          AND ($1::uuid IS NULL OR rl.id_revision = $1)
          AND rl.estado = 'ACTIVO'
          AND NOT {en_universo_rl}
        "#
    );
    let pasaron_a_fuera = tx.execute(
        sql_marcar_fuera.as_str(),
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| {
        let err_str = crate::db::schema::format_pg_error(&e);
        crate::app_logger::log_db_error(
            "UPDATE reportes_liquidaciones estado FUERA_PERIODO",
            &["public.reportes_liquidaciones"],
            "UPDATE public.reportes_liquidaciones SET estado = 'FUERA_PERIODO' WHERE ...",
            &err_str,
        );
        format!(
            "Error marcando reportes fuera de periodo: {}",
            err_str
        )
    })?;

    let sql_reactivar = format!(
        r#"
        UPDATE public.reportes_liquidaciones rl
        SET estado = 'ACTIVO'
        WHERE rl.id_registro_patronal = ANY($2)
          AND ($1::uuid IS NULL OR rl.id_revision = $1)
          AND rl.estado = 'FUERA_PERIODO'
          AND {en_universo_rl}
        "#
    );
    let regresaron_a_activo = tx.execute(
        sql_reactivar.as_str(),
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| {
        let err_str = crate::db::schema::format_pg_error(&e);
        crate::app_logger::log_db_error(
            "UPDATE reportes_liquidaciones estado ACTIVO",
            &["public.reportes_liquidaciones"],
            "UPDATE public.reportes_liquidaciones SET estado = 'ACTIVO' WHERE ...",
            &err_str,
        );
        format!(
            "Error reactivando reportes dentro de periodo: {}",
            err_str
        )
    })?;

    Ok((pasaron_a_fuera, regresaron_a_activo))
}

/// Reevalúa el universo de una revisión en su propia transacción (se usa al abrir la vista).
pub fn reevaluar_universo_revision(client: &mut Client, id_revision: Uuid) -> Result<(u64, u64), String> {
    let (rev_uuid_opt, _id_rp, _clean_reg, all_rp_uuids) = resolve_rp_context(client, id_revision)?;
    let mut tx = client
        .transaction()
        .map_err(|e| format!("Error iniciando transacción: {}", crate::db::schema::format_pg_error(&e)))?;
    let conteos = reevaluar_universo_tx(&mut tx, rev_uuid_opt, &all_rp_uuids)?;
    tx.commit()
        .map_err(|e| format!("Error en commit de reevaluación de universo: {}", crate::db::schema::format_pg_error(&e)))?;
    Ok(conteos)
}

/// true si la revisión tiene al menos un Registro de Obra del usuario activo con fecha de inicio.
pub fn revision_tiene_ro_usuario(client: &mut Client, id_revision: Uuid) -> Result<bool, String> {
    client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM public.registros_obras WHERE id_revision = $1 AND COALESCE(creado_usuario,false) AND activa AND fecha_inicio IS NOT NULL)",
            &[&id_revision],
        )
        .map(|r| r.get(0))
        .map_err(|e| format!("Error consultando registros de obra del usuario: {}", crate::db::schema::format_pg_error(&e)))
}

/// Versión transaccional de `sync_reportes_liquidaciones_for_revision` para ejecutarse dentro
/// de la misma transacción atómica que inserta en `public.liquidaciones`.
///
/// Comportamiento NO destructivo: nunca borra filas de `reportes_liquidaciones`. Los meses fuera del
/// universo de la revisión pasan a `FUERA_PERIODO` y regresan a `ACTIVO` si reingresan; los `EXCLUIDO`
/// no se tocan. Las ediciones del analista (`editado_usuario = true`) no se modifican ni se borran.
/// Los importes se recalculan con la regla canónica de prorrateo (`recalcular_prorrateo_tx`).
pub fn sync_reportes_liquidaciones_for_revision_tx(
    tx: &mut Transaction,
    id_revision: Uuid,
) -> Result<usize, String> {
    let (rev_uuid_opt, _id_rp, _clean_reg, all_rp_uuids) = resolve_rp_context_tx(tx, id_revision)?;

    let active_rev_id: Uuid = match rev_uuid_opt {
        Some(r) => r,
        None => {
            let row = tx
                .query_opt(
                    "SELECT id FROM public.revisiones WHERE id_registro_patronal = ANY($1) ORDER BY created_at DESC NULLS LAST, id DESC LIMIT 1",
                    &[&all_rp_uuids],
                )
                .map_err(|e| {
                    format!(
                        "Error consultando revisión activa para sincronizar reportes: {}",
                        crate::db::schema::format_pg_error(&e)
                    )
                })?;
            match row {
                Some(r) => r.get(0),
                None => return Ok(0),
            }
        }
    };

    // 1. Universo de la revisión (ver `sql_en_universo`). Cada reporte se evalúa contra el universo de SU
    // PROPIA revisión (rl.id_revision). Se usa UPDATE en lugar de DELETE: los meses fuera del universo quedan
    // como FUERA_PERIODO con sus ediciones intactas, y reaparecen como ACTIVO si cambian las obras del usuario.
    reevaluar_universo_tx(tx, rev_uuid_opt, &all_rp_uuids)?;

    // 1.1 (Eliminado) Los duplicados por empleado/periodo los impide la restricción uq_rl_rev_emp_periodo.

    // 1.2 Desvincular obras fantasma en reportes NO editados por el analista: vinculaciones que ya no existen
    // en la tabla maestra vinculaciones_obras_liquidaciones. Los reportes editados conservan sus vínculos.
    tx.execute(
        r#"
        DELETE FROM public.reportes_vinculaciones_obras_liquidaciones rvol
        WHERE rvol.id_reporte_liquidacion IN (
            SELECT rl.id FROM public.reportes_liquidaciones rl
            WHERE rl.id_registro_patronal = ANY($2)
              AND ($1::uuid IS NULL OR rl.id_revision = $1)
              AND rl.editado_usuario = false
        )
        -- La obra se compara por NÚMERO, no por id: las liquidaciones son por registro patronal y se
        -- comparten entre revisiones, y cada revisión tiene su propia fila de registros_obras para la misma
        -- obra. Comparar ids marcaba como "fantasma" una obra válida de la cédula en otra revisión.
        AND NOT EXISTS (
            SELECT 1 FROM public.vinculaciones_obras_liquidaciones vol
            JOIN public.reportes_liquidaciones rl ON rl.id_liquidacion = vol.id_liquidacion
            JOIN public.registros_obras ro_m ON ro_m.id = vol.id_registro_obra
            JOIN public.registros_obras ro_r ON ro_r.id = rvol.id_registro_obra
            WHERE rl.id = rvol.id_reporte_liquidacion
              AND BTRIM(UPPER(ro_m.numero)) = BTRIM(UPPER(ro_r.numero))
        )
        AND EXISTS (
            SELECT 1 FROM public.registros_obras ro
            WHERE ro.id = rvol.id_registro_obra
              AND COALESCE(ro.creado_usuario, false) = false
        )
        "#,
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| {
        let err_str = crate::db::schema::format_pg_error(&e);
        crate::app_logger::log_db_error(
            "DELETE ghost reportes_vinculaciones_obras_liquidaciones",
            &["public.reportes_vinculaciones_obras_liquidaciones"],
            "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones ...",
            &err_str,
        );
        format!("Error depurando vinculaciones no existentes en cédulas: {}", err_str)
    })?;

    // 1.3 Limpiar renglones de detalle con obras no autorizadas, solo en reportes NO editados por el analista.
    tx.execute(
        r#"
        DELETE FROM public.reportes_detalles_liquidaciones rdl
        WHERE rdl.id_reporte_liquidacion IN (
            SELECT rl.id FROM public.reportes_liquidaciones rl
            WHERE rl.id_registro_patronal = ANY($2)
              AND ($1::uuid IS NULL OR rl.id_revision = $1)
              AND rl.editado_usuario = false
        )
        AND rdl.id_registro_obra IS NOT NULL
        AND NOT EXISTS (
            SELECT 1 FROM public.reportes_vinculaciones_obras_liquidaciones rvol
            WHERE rvol.id_reporte_liquidacion = rdl.id_reporte_liquidacion
              AND rvol.id_registro_obra = rdl.id_registro_obra
        )
        AND EXISTS (
            SELECT 1 FROM public.registros_obras ro
            WHERE ro.id = rdl.id_registro_obra
              AND COALESCE(ro.creado_usuario, false) = false
        )
        "#,
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| {
        let err_str = crate::db::schema::format_pg_error(&e);
        crate::app_logger::log_db_error(
            "DELETE ghost reportes_detalles_liquidaciones",
            &["public.reportes_detalles_liquidaciones"],
            "DELETE FROM public.reportes_detalles_liquidaciones ...",
            &err_str,
        );
        format!("Error depurando detalles con obras inexistentes: {}", err_str)
    })?;

    // 2. Inserción de liquidaciones en public.reportes_liquidaciones dentro del universo de la revisión.
    // DISTINCT ON elige una única liquidación por empleado y periodo (la de obras y cuotas completas).
    // Los reportes ya existentes se respetan mediante ON CONFLICT sobre uq_rl_rev_emp_periodo.
    let en_universo_l = sql_en_universo("$3", "l.anio_periodo", "l.calc_mes_int");
    let sql_insert_reportes = format!(
        r#"
        INSERT INTO public.reportes_liquidaciones (
            id, id_liquidacion, id_registro_patronal, id_revision, id_empleado,
            mes_periodo, mes_periodo_int, anio_periodo, nombre_archivo
        )
        SELECT DISTINCT ON (l.id_registro_patronal, l.id_empleado, l.anio_periodo, l.calc_mes_int)
            uuidv7(),
            l.id,
            l.id_registro_patronal,
            $1::uuid,
            l.id_empleado,
            l.mes_periodo,
            l.calc_mes_int,
            l.anio_periodo,
            l.nombre_archivo
        FROM (
            SELECT
                l_inner.*,
                LEAST(GREATEST(COALESCE(NULLIF(l_inner.mes_periodo_int, 0), CASE UPPER(TRIM(l_inner.mes_periodo))
                    WHEN 'ENERO' THEN 1 WHEN 'FEBRERO' THEN 2 WHEN 'MARZO' THEN 3
                    WHEN 'ABRIL' THEN 4 WHEN 'MAYO' THEN 5 WHEN 'JUNIO' THEN 6
                    WHEN 'JULIO' THEN 7 WHEN 'AGOSTO' THEN 8 WHEN 'SEPTIEMBRE' THEN 9
                    WHEN 'OCTUBRE' THEN 10 WHEN 'NOVIEMBRE' THEN 11 WHEN 'DICIEMBRE' THEN 12
                    ELSE 1 END)::int, 1), 12)::smallint AS calc_mes_int
            FROM public.liquidaciones l_inner
            WHERE l_inner.id_registro_patronal = ANY($2)
        ) l
        WHERE {en_universo_l}
        ORDER BY
            l.id_registro_patronal,
            l.id_empleado,
            l.anio_periodo,
            l.calc_mes_int,
            (EXISTS (SELECT 1 FROM public.vinculaciones_obras_liquidaciones vol WHERE vol.id_liquidacion = l.id)) DESC,
            (EXISTS (SELECT 1 FROM public.detalles_liquidaciones dl WHERE dl.id_liquidacion = l.id AND COALESCE(dl.cf, 0) > 0)) DESC,
            (EXISTS (SELECT 1 FROM public.detalles_liquidaciones dl WHERE dl.id_liquidacion = l.id AND COALESCE(dl.subtotal, 0) > 0)) DESC,
            l.created_at ASC
        ON CONFLICT ON CONSTRAINT uq_rl_rev_emp_periodo DO NOTHING
        "#
    );
    let inserted_count = tx.execute(
        sql_insert_reportes.as_str(),
        &[&active_rev_id, &all_rp_uuids, &active_rev_id],
    ).map_err(|e| {
        let err_str = crate::db::schema::format_pg_error(&e);
        crate::app_logger::log_db_error(
            "INSERT reportes_liquidaciones SELECT",
            &["public.reportes_liquidaciones", "public.liquidaciones", "public.registros_obras"],
            "INSERT INTO public.reportes_liquidaciones SELECT DISTINCT ON ... ON CONFLICT DO NOTHING",
            &err_str,
        );
        format!(
            "Error insertando reporte de liquidación: {}",
            err_str
        )
    })?;

    // 3. Asegurar que las obras referenciadas en vinculaciones_obras_liquidaciones existan en registros_obras para la revisión activa
    if let Some(rev_id) = rev_uuid_opt {
        tx.execute(
            r#"
            INSERT INTO public.registros_obras (
                id, id_revision, id_registro_patronal, numero, clase, tipo_obra,
                fecha_inicio, fecha_fin, ubicacion, superficie_construido, monto,
                delegacion, subdelegacion, creado_usuario
            )
            SELECT DISTINCT ON (TRIM(UPPER(ro_src.numero)))
                uuidv7(),
                $1::uuid,
                ro_src.id_registro_patronal,
                ro_src.numero,
                ro_src.clase,
                ro_src.tipo_obra,
                ro_src.fecha_inicio,
                ro_src.fecha_fin,
                ro_src.ubicacion,
                ro_src.superficie_construido,
                ro_src.monto,
                ro_src.delegacion,
                ro_src.subdelegacion,
                false
            FROM public.vinculaciones_obras_liquidaciones vol
            JOIN public.liquidaciones l ON l.id = vol.id_liquidacion
            JOIN public.registros_obras ro_src ON ro_src.id = vol.id_registro_obra
            WHERE l.id_registro_patronal = ANY($2)
              AND NOT EXISTS (
                  SELECT 1 FROM public.registros_obras ro_tgt
                  WHERE ro_tgt.id_revision = $1
                    AND TRIM(UPPER(ro_tgt.numero)) = TRIM(UPPER(ro_src.numero))
              )
            ON CONFLICT (id_revision, numero) DO NOTHING;
            "#,
            &[&rev_id, &all_rp_uuids],
        ).map_err(|e| {
            let err_str = crate::db::schema::format_pg_error(&e);
            format!("Error asegurando registros de obra en revisión activa: {}", err_str)
        })?;
    }

    // Copiar Vinculaciones de Obra hacia public.reportes_vinculaciones_obras_liquidaciones
    tx.execute(
        r#"
        INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra)
        SELECT uuidv7(), ro_tgt.id, rl.id, vol.dias_registro_obra
        FROM public.reportes_liquidaciones rl
        JOIN public.vinculaciones_obras_liquidaciones vol ON vol.id_liquidacion = rl.id_liquidacion
        JOIN public.registros_obras ro_src ON ro_src.id = vol.id_registro_obra
        JOIN public.registros_obras ro_tgt ON (
            (rl.id_revision IS NOT NULL AND ro_tgt.id_revision = rl.id_revision AND TRIM(UPPER(ro_tgt.numero)) = TRIM(UPPER(ro_src.numero)))
            OR (rl.id_revision IS NULL AND ro_tgt.id = ro_src.id)
        ) AND ro_tgt.activa
        WHERE rl.id_registro_patronal = ANY($2)
          AND ($1::uuid IS NULL OR rl.id_revision = $1)
          AND NOT EXISTS (
              SELECT 1 FROM public.reportes_vinculaciones_obras_liquidaciones rvol
              WHERE rvol.id_reporte_liquidacion = rl.id
                AND rvol.id_registro_obra = ro_tgt.id
          )
        "#,
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| {
        let err_str = crate::db::schema::format_pg_error(&e);
        crate::app_logger::log_db_error(
            "INSERT reportes_vinculaciones_obras_liquidaciones (master vinculaciones)",
            &["public.reportes_vinculaciones_obras_liquidaciones", "public.vinculaciones_obras_liquidaciones"],
            "INSERT INTO public.reportes_vinculaciones_obras_liquidaciones SELECT ... FROM vinculaciones_obras_liquidaciones",
            &err_str,
        );
        format!(
            "Error copiando vinculaciones de obra a reportes: {}",
            err_str
        )
    })?;

    let missing_det_row = tx.query_one(
        r#"
        SELECT COUNT(*) FROM public.detalles_liquidaciones dl
        JOIN public.reportes_liquidaciones rl ON dl.id_liquidacion = rl.id_liquidacion
        WHERE rl.id_registro_patronal = ANY($1)
          AND ($2::uuid IS NULL OR rl.id_revision = $2)
          AND NOT EXISTS (
              SELECT 1 FROM public.reportes_detalles_liquidaciones rdl
              WHERE rdl.id_reporte_liquidacion = rl.id
                AND (
                    (rdl.clave IS NULL AND dl.clave IS NULL)
                    OR TRIM(LOWER(COALESCE(rdl.clave, ''))) = TRIM(LOWER(COALESCE(dl.clave, '')))
                )
                AND (
                    (rdl.fecha IS NULL AND dl.fecha IS NULL)
                    OR rdl.fecha = dl.fecha
                )
          )
        "#,
        &[&all_rp_uuids, &rev_uuid_opt],
    ).map_err(|e| {
        format!(
            "Error verificando detalles pendientes: {}",
            crate::db::schema::format_pg_error(&e)
        )
    })?;
    let missing_det: i64 = missing_det_row.get(0);

    if inserted_count > 0 || missing_det > 0 {
        tx.execute(
            r#"
            INSERT INTO public.reportes_detalles_liquidaciones (
                id, id_reporte_liquidacion, clave, fecha, dias, sdi, lic, inc, aus, cf,
                exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr,
                gps, patronal, obrera, subtotal, id_registro_obra, dias_registro_obra
            )
            SELECT
                uuidv7(),
                rl.id,
                dl.clave,
                dl.fecha,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0 ELSE dl.dias END,
                dl.sdi,
                dl.lic,
                dl.inc,
                dl.aus,
                dl.cf,
                dl.exc_pat,
                dl.exc_obr,
                dl.pd_pat,
                dl.pd_obr,
                dl.gmp_pat,
                dl.gmp_obr,
                dl.rt,
                dl.iv_pat,
                dl.iv_obr,
                dl.gps,
                dl.patronal,
                dl.obrera,
                dl.subtotal,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN NULL ELSE rvol.id_registro_obra END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0 ELSE COALESCE(rvol.dias_registro_obra, dl.dias) END
            FROM public.detalles_liquidaciones dl
            JOIN public.reportes_liquidaciones rl ON dl.id_liquidacion = rl.id_liquidacion
            LEFT JOIN public.reportes_vinculaciones_obras_liquidaciones rvol ON rvol.id_reporte_liquidacion = rl.id
            WHERE rl.id_registro_patronal = ANY($1)
              AND ($2::uuid IS NULL OR rl.id_revision = $2)
              AND NOT EXISTS (
                  SELECT 1 FROM public.reportes_detalles_liquidaciones rdl
                  WHERE rdl.id_reporte_liquidacion = rl.id
                    AND (
                        (rdl.clave IS NULL AND dl.clave IS NULL)
                        OR TRIM(LOWER(COALESCE(rdl.clave, ''))) = TRIM(LOWER(COALESCE(dl.clave, '')))
                    )
                    AND (
                        (rdl.fecha IS NULL AND dl.fecha IS NULL)
                        OR rdl.fecha = dl.fecha
                    )
              )
            "#,
            &[&all_rp_uuids, &rev_uuid_opt],
        ).map_err(|e| {
            let err_str = crate::db::schema::format_pg_error(&e);
            crate::app_logger::log_db_error(
                "INSERT INTO reportes_detalles_liquidaciones SELECT",
                &["public.reportes_detalles_liquidaciones", "public.detalles_liquidaciones"],
                "INSERT INTO public.reportes_detalles_liquidaciones SELECT ... FROM public.detalles_liquidaciones",
                &err_str,
            );
            format!(
                "Error insertando detalles en reportes_detalles_liquidaciones: {}",
                err_str
            )
        })?;
    }

    // Asignar la obra de la vinculación (rvol) a los renglones sin obra, SOLO en reportes NO editados
    // por el analista: si dejó un tramo sin obra de forma intencional, se respeta.
    tx.execute(
        r#"
        UPDATE public.reportes_detalles_liquidaciones rdl
        SET id_registro_obra = rvol.id_registro_obra,
            dias_registro_obra = COALESCE(rvol.dias_registro_obra, rdl.dias)
        FROM public.reportes_vinculaciones_obras_liquidaciones rvol
        WHERE rvol.id_reporte_liquidacion = rdl.id_reporte_liquidacion
          AND rdl.id_registro_obra IS NULL
          AND LOWER(TRIM(COALESCE(rdl.clave, ''))) != 'baja'
          AND EXISTS (
              SELECT 1 FROM public.reportes_liquidaciones rl
              WHERE rl.id = rdl.id_reporte_liquidacion
                AND rl.editado_usuario = false
                -- Solo la revisión que se sincroniza (antes afectaba a todas las revisiones)
                AND rl.id_revision = $1
          )
        "#,
        &[&active_rev_id],
    ).map_err(|e| {
        let err_str = crate::db::schema::format_pg_error(&e);
        crate::app_logger::log_db_error(
            "UPDATE reportes_detalles_liquidaciones (backfill id_registro_obra)",
            &["public.reportes_detalles_liquidaciones", "public.reportes_vinculaciones_obras_liquidaciones"],
            "UPDATE public.reportes_detalles_liquidaciones SET id_registro_obra = rvol.id_registro_obra ...",
            &err_str,
        );
        format!("Error actualizando registro de obra en detalles: {}", err_str)
    })?;

    // 2.1 Tramos huérfanos: renglones de reporte que ya no tienen correspondencia (clave normalizada y fecha)
    // en detalles_liquidaciones de su liquidación.
    // - Reportes NO editados: el tramo se elimina (refleja la cédula vigente).
    // - Reportes editados: el tramo se conserva y el reporte se marca cedula_actualizada para revisión.
    tx.execute(
        r#"
        DELETE FROM public.reportes_detalles_liquidaciones rdl
        USING public.reportes_liquidaciones rl
        WHERE rdl.id_reporte_liquidacion = rl.id
          AND rl.id_registro_patronal = ANY($1)
          AND ($2::uuid IS NULL OR rl.id_revision = $2)
          AND rl.editado_usuario = false
          AND NOT EXISTS (
              SELECT 1 FROM public.detalles_liquidaciones dl
              WHERE dl.id_liquidacion = rl.id_liquidacion
                AND COALESCE(BTRIM(LOWER(dl.clave)), '') = COALESCE(BTRIM(LOWER(rdl.clave)), '')
                AND dl.fecha IS NOT DISTINCT FROM rdl.fecha
          )
        "#,
        &[&all_rp_uuids, &rev_uuid_opt],
    ).map_err(|e| {
        let err_str = crate::db::schema::format_pg_error(&e);
        crate::app_logger::log_db_error(
            "DELETE tramos huérfanos reportes_detalles_liquidaciones",
            &["public.reportes_detalles_liquidaciones", "public.detalles_liquidaciones"],
            "DELETE FROM public.reportes_detalles_liquidaciones rdl USING public.reportes_liquidaciones rl WHERE ...",
            &err_str,
        );
        format!("Error eliminando tramos huérfanos en reportes: {}", err_str)
    })?;

    tx.execute(
        r#"
        UPDATE public.reportes_liquidaciones rl
        SET cedula_actualizada = true
        WHERE rl.id_registro_patronal = ANY($1)
          AND ($2::uuid IS NULL OR rl.id_revision = $2)
          AND rl.editado_usuario = true
          AND EXISTS (
              SELECT 1 FROM public.reportes_detalles_liquidaciones rdl
              WHERE rdl.id_reporte_liquidacion = rl.id
                AND NOT EXISTS (
                    SELECT 1 FROM public.detalles_liquidaciones dl
                    WHERE dl.id_liquidacion = rl.id_liquidacion
                      AND COALESCE(BTRIM(LOWER(dl.clave)), '') = COALESCE(BTRIM(LOWER(rdl.clave)), '')
                      AND dl.fecha IS NOT DISTINCT FROM rdl.fecha
                )
          )
        "#,
        &[&all_rp_uuids, &rev_uuid_opt],
    ).map_err(|e| {
        let err_str = crate::db::schema::format_pg_error(&e);
        crate::app_logger::log_db_error(
            "UPDATE reportes_liquidaciones cedula_actualizada",
            &["public.reportes_liquidaciones", "public.reportes_detalles_liquidaciones", "public.detalles_liquidaciones"],
            "UPDATE public.reportes_liquidaciones SET cedula_actualizada = true WHERE ...",
            &err_str,
        );
        format!("Error marcando reportes con cédula actualizada: {}", err_str)
    })?;

    // Eliminar vinculaciones a obras fuera de la revisión, SOLO en reportes NO editados por el analista.
    tx.execute(
        r#"
        DELETE FROM public.reportes_vinculaciones_obras_liquidaciones rvol
        USING public.reportes_liquidaciones rl
        WHERE rvol.id_reporte_liquidacion = rl.id
          AND rl.id_registro_patronal = ANY($2)
          -- Solo la revisión que se sincroniza: sin este filtro se borraban las vinculaciones de las
          -- demás revisiones del mismo registro patronal (sus obras son filas de OTRA revisión).
          AND ($1::uuid IS NULL OR rl.id_revision = $1)
          AND rl.editado_usuario = false
          AND rvol.id_registro_obra IS NOT NULL
          AND rvol.id_registro_obra NOT IN (
              SELECT ro.id FROM public.registros_obras ro
              WHERE (
                  ($1::uuid IS NOT NULL AND ro.id_revision = $1)
                  OR ($1::uuid IS NULL AND ro.id_registro_patronal = ANY($2))
              )
          )
        "#,
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| {
        format!(
            "Error depurando vinculaciones fuera de revisión: {}",
            crate::db::schema::format_pg_error(&e)
        )
    })?;

    // 3. Recalcular importes prorrateados de los tramos de los reportes de la revisión con la regla canónica.
    // Sustituye la restauración de importes de cédula original, que anulaba el prorrateo por días de obra.
    let ids_reportes: Vec<Uuid> = tx
        .query(
            r#"
            SELECT id FROM public.reportes_liquidaciones
            WHERE id_registro_patronal = ANY($1)
              AND ($2::uuid IS NULL OR id_revision = $2)
            "#,
            &[&all_rp_uuids, &rev_uuid_opt],
        )
        .map_err(|e| {
            format!(
                "Error consultando reportes para recalcular prorrateo: {}",
                crate::db::schema::format_pg_error(&e)
            )
        })?
        .iter()
        .map(|r| r.get(0))
        .collect();

    crate::db::prorrateo::recalcular_prorrateo_tx(tx, &ids_reportes)
        .map_err(|e| format!("Error recalculando prorrateo de tramos tras sincronizar: {}", e))?;

    let copied_count = (inserted_count as i64 + missing_det) as usize;

    Ok(copied_count)
}

/// Sincroniza y copia a las tablas de reportes (reportes_liquidaciones, reportes_detalles_liquidaciones, reportes_vinculaciones_obras_liquidaciones)
/// las liquidaciones filtradas en base a las fechas de los registros de obras creados por el usuario en la revisión analizada.
pub fn sync_reportes_liquidaciones_for_revision(
    client: &mut Client,
    id_revision: Uuid,
) -> Result<usize, String> {
    let mut tx = client
        .transaction()
        .map_err(|e| format!("Error iniciando transacción: {}", crate::db::schema::format_pg_error(&e)))?;
    let copied_count = sync_reportes_liquidaciones_for_revision_tx(&mut tx, id_revision)?;
    tx.commit()
        .map_err(|e| format!("Error en commit de sincronización: {}", crate::db::schema::format_pg_error(&e)))?;

    crate::app_logger::log_db_op(
        "SYNC",
        &[
            "public.reportes_liquidaciones",
            "public.reportes_detalles_liquidaciones",
            "public.reportes_vinculaciones_obras_liquidaciones",
        ],
        "sync_reportes_liquidaciones_for_revision",
        &format!("id_revision={}, registros_sincronizados={}", id_revision, copied_count),
        Some(copied_count),
    );
    Ok(copied_count)
}

/// Revierte todos los cambios en Reporte Liquidaciones para la revisión indicada:
/// Elimina los registros modificados en reportes_detalles_liquidaciones y reportes_vinculaciones_obras_liquidaciones,
/// restaura los valores originales desde detalles_liquidaciones (tal como venían en los archivos de cédulas),
/// re-asigna la vinculación original de obra (desde vinculaciones_obras_liquidaciones o clave_ubicacion)
/// y recalcula los topes y prorrateos en base a los valores por defecto y movimientos del trabajador.
pub fn revertir_reporte_liquidaciones_a_originales(
    client: &mut Client,
    id_revision: Uuid,
) -> Result<usize, String> {
    let obras = fetch_obras_for_revision(client, id_revision)?;

    // 1. Obtener las claves exactas de periodo ("MES/AÑO") a partir de las fechas de las obras creadas por el usuario
    let period_keys_set = extract_period_keys_from_obras(&obras);
    let period_keys_vec: Vec<String> = period_keys_set.into_iter().collect();

    let mut tx = client.transaction().map_err(|e| format!("Error iniciando transacción: {}", e))?;
    let (rev_uuid_opt, id_rp, _clean_reg, all_rp_uuids) = resolve_rp_context_tx(&mut tx, id_revision)?;
    let active_rev_id: Uuid = match rev_uuid_opt {
        Some(r) => r,
        None => {
            let row = tx
                .query_opt(
                    "SELECT id FROM public.revisiones WHERE id_registro_patronal = ANY($1) ORDER BY created_at DESC NULLS LAST, id DESC LIMIT 1",
                    &[&all_rp_uuids],
                )
                .map_err(|e| format!("Error consultando revisión activa para revertir reportes: {}", e))?;
            match row {
                Some(r) => r.get(0),
                None => id_revision,
            }
        }
    };

    // Mapa de obras válidas con sus rangos de fechas
    let valid_obras_rows = tx.query(
        "SELECT id, numero, fecha_inicio, fecha_fin FROM public.registros_obras WHERE activa AND (($1::uuid IS NOT NULL AND id_revision = $1) OR ($1::uuid IS NULL AND id_registro_patronal = ANY($2)))",
        &[&rev_uuid_opt, &all_rp_uuids],
    ).map_err(|e| format!("Error consultando registros de obras: {}", e))?;

    let mut obras_map: HashMap<Uuid, (Option<NaiveDate>, Option<NaiveDate>)> = HashMap::new();
    let mut numero_to_obra: HashMap<String, (Uuid, Option<NaiveDate>, Option<NaiveDate>)> = HashMap::new();
    for r in valid_obras_rows {
        let oid: Uuid = r.get(0);
        let num: Option<String> = r.try_get(1).ok();
        let f_ini: Option<NaiveDate> = r.try_get(2).ok().flatten();
        let f_fin: Option<NaiveDate> = r.try_get(3).ok().flatten();
        obras_map.insert(oid, (f_ini, f_fin));
        if let Some(n) = num {
            let n_clean = n.trim().to_uppercase();
            if !n_clean.is_empty() {
                numero_to_obra.insert(n_clean, (oid, f_ini, f_fin));
            }
        }
    }

    // Bitácora: foto "antes" de todos los reportes que esta reversión limpia o reconstruye.
    // Mismo alcance que los DELETE de este bloque.
    let ids_reportes: Vec<Uuid> = tx.query(
        "SELECT id FROM public.reportes_liquidaciones WHERE id_registro_patronal = ANY($1) AND ($2::uuid IS NULL OR id_revision = $2)",
        &[&all_rp_uuids, &rev_uuid_opt],
    ).map_err(|e| format!("Error consultando reportes para bitácora: {}", e))?
    .into_iter()
    .map(|r| r.get::<_, Uuid>(0))
    .collect();
    let lote = bitacora_reportes::iniciar_edicion(&mut tx, ACCION_REVERTIR, &ids_reportes)
        .map_err(|e| format!("Error iniciando bitácora de edición: {}", e))?;

    // 2. Limpiar todos los detalles y vinculaciones actuales de reportes para este registro patronal y revisión
    tx.execute(
        r#"
        DELETE FROM public.reportes_detalles_liquidaciones
        WHERE id_reporte_liquidacion IN (
            SELECT id FROM public.reportes_liquidaciones
            WHERE id_registro_patronal = ANY($1) AND ($2::uuid IS NULL OR id_revision = $2)
        )
        "#,
        &[&all_rp_uuids, &rev_uuid_opt],
    ).map_err(|e| format!("Error limpiando reportes_detalles_liquidaciones: {}", e))?;

    tx.execute(
        r#"
        DELETE FROM public.reportes_vinculaciones_obras_liquidaciones
        WHERE id_reporte_liquidacion IN (
            SELECT id FROM public.reportes_liquidaciones
            WHERE id_registro_patronal = ANY($1) AND ($2::uuid IS NULL OR id_revision = $2)
        )
        "#,
        &[&all_rp_uuids, &rev_uuid_opt],
    ).map_err(|e| format!("Error limpiando reportes_vinculaciones_obras_liquidaciones: {}", e))?;

    // 3. Consultar las liquidaciones correspondientes al periodo analizado
    let liq_rows = if !period_keys_vec.is_empty() {
        // Limpiar en reportes_liquidaciones aquellos registros fuera del periodo
        tx.execute(
            r#"
            DELETE FROM public.reportes_liquidaciones
            WHERE id_registro_patronal = ANY($1)
              AND ($3::uuid IS NULL OR id_revision = $3)
              AND NOT ((UPPER(TRIM(mes_periodo)) || '/' || anio_periodo::text) = ANY($2))
            "#,
            &[&all_rp_uuids, &period_keys_vec, &rev_uuid_opt],
        ).map_err(|e| format!("Error limpiando reportes_liquidaciones fuera de periodo: {}", e))?;

        tx.query(
            r#"
            SELECT DISTINCT ON (l.id_registro_patronal, l.id_empleado, l.anio_periodo, COALESCE(l.mes_periodo_int, 1))
                l.id, l.id_empleado, l.mes_periodo, l.mes_periodo_int, l.anio_periodo, l.nombre_archivo
            FROM public.liquidaciones l
            WHERE l.id_registro_patronal = ANY($1)
              AND (UPPER(TRIM(l.mes_periodo)) || '/' || l.anio_periodo::text) = ANY($2)
            ORDER BY l.id_registro_patronal, l.id_empleado, l.anio_periodo, COALESCE(l.mes_periodo_int, 1),
                (EXISTS (SELECT 1 FROM public.vinculaciones_obras_liquidaciones vol WHERE vol.id_liquidacion = l.id)) DESC,
                (EXISTS (SELECT 1 FROM public.detalles_liquidaciones dl WHERE dl.id_liquidacion = l.id AND COALESCE(dl.cf, 0) > 0)) DESC,
                (EXISTS (SELECT 1 FROM public.detalles_liquidaciones dl WHERE dl.id_liquidacion = l.id AND COALESCE(dl.subtotal, 0) > 0)) DESC,
                l.created_at ASC
            "#,
            &[&all_rp_uuids, &period_keys_vec],
        ).map_err(|e| format!("Error consultando liquidaciones filtradas: {}", e))?
    } else {
        tx.execute(
            r#"
            DELETE FROM public.reportes_liquidaciones
            WHERE id_registro_patronal = ANY($1)
              AND ($2::uuid IS NULL OR id_revision = $2)
            "#,
            &[&all_rp_uuids, &rev_uuid_opt],
        ).map_err(|e| format!("Error limpiando reportes_liquidaciones sin periodos: {}", e))?;
        Vec::new()
    };

    let mut restored_count = 0;

    for row in liq_rows {
        let id_liq: Uuid = match row.try_get(0) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id_emp: Uuid = match row.try_get(1) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mes_str: Option<String> = row.try_get(2).ok();
        let mes_int_db: Option<i16> = row.try_get(3).ok();
        let anio: Option<i16> = row.try_get(4).ok();
        let nombre_arch: Option<String> = row.try_get(5).ok();
        let mes_int_val: i16 = mes_int_db.unwrap_or_else(|| {
            mes_to_periodo_int(mes_str.as_deref().unwrap_or("ENERO"))
        });

        // Asegurar reporte_liquidacion único por empleado y periodo
        let rep_liq_opt = tx.query_opt(
            "SELECT id FROM public.reportes_liquidaciones WHERE id_empleado = $1 AND id_revision = $2 AND anio_periodo = $3 AND mes_periodo_int = $4 LIMIT 1",
            &[&id_emp, &active_rev_id, &anio, &mes_int_val],
        ).map_err(|e| format!("Error buscando reporte_liquidacion: {}", e))?;

        let id_rep_liq = match rep_liq_opt {
            Some(r) => r.try_get::<_, Uuid>(0).unwrap_or_else(|_| Uuid::now_v7()),
            None => {
                let new_rep_id = Uuid::now_v7();
                tx.execute(
                    r#"
                    INSERT INTO public.reportes_liquidaciones (
                        id, id_liquidacion, id_registro_patronal, id_revision, id_empleado,
                        mes_periodo, mes_periodo_int, anio_periodo, nombre_archivo
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    "#,
                    &[&new_rep_id, &id_liq, &id_rp, &active_rev_id, &id_emp, &mes_str, &mes_int_val, &anio, &nombre_arch],
                ).map_err(|e| format!("Error insertando reporte_liquidacion: {}", e))?;
                new_rep_id
            }
        };

        // Identificar la(s) obra(s) y días originales tal como venían en los archivos de cédulas de obra
        let orig_vinc_rows = tx.query(
            r#"
            SELECT vol.id_registro_obra, vol.dias_registro_obra, ro.numero
            FROM public.vinculaciones_obras_liquidaciones vol
            JOIN public.registros_obras ro ON ro.id = vol.id_registro_obra
            WHERE vol.id_liquidacion = $1
            "#,
            &[&id_liq],
        ).map_err(|e| format!("Error consultando vinculación original: {}", e))?;

        let mut target_vincs: Vec<(Option<Uuid>, Option<i16>)> = Vec::new();
        for ov in orig_vinc_rows {
            let o_id: Option<Uuid> = ov.try_get::<_, Option<Uuid>>(0).ok().flatten();
            let d_obra: Option<i16> = ov.try_get::<_, Option<i16>>(1).ok().flatten();
            let o_num: String = ov.try_get::<_, Option<String>>(2).ok().flatten().unwrap_or_default().trim().to_uppercase();
            let target_oid = if let Some(t_info) = numero_to_obra.get(&o_num) {
                Some(t_info.0)
            } else if let Some(oid) = o_id.filter(|id| obras_map.contains_key(id)) {
                Some(oid)
            } else {
                None
            };
            if let Some(toid) = target_oid {
                target_vincs.push((Some(toid), d_obra));
            }
        }

        if target_vincs.is_empty() {
            target_vincs.push((None, None));
        }

        // Extraer movimientos del trabajador en este mes a partir de detalles_liquidaciones
        let mes_clean = mes_str.as_deref().unwrap_or("").trim();
        let anio_val = anio.unwrap_or(0);
        let mov_rows = tx.query(
            r#"
            SELECT dl.clave, dl.fecha
            FROM public.detalles_liquidaciones dl
            JOIN public.liquidaciones l ON dl.id_liquidacion = l.id
            WHERE l.id_empleado = $1
              AND UPPER(TRIM(l.mes_periodo)) = UPPER(TRIM($2))
              AND l.anio_periodo = $3
              AND dl.fecha IS NOT NULL
            ORDER BY dl.fecha ASC
            "#,
            &[&id_emp, &mes_clean, &anio_val],
        ).unwrap_or_default();

        let mut movements: Vec<EmployeeMovement> = Vec::new();
        for mr in mov_rows {
            let clv: String = mr.try_get::<_, Option<String>>(0).ok().flatten().unwrap_or_default();
            if let Ok(f) = mr.try_get::<_, NaiveDate>(1) {
                let c_low = clv.trim().to_lowercase();
                if c_low.contains("baja") {
                    movements.push(EmployeeMovement { kind: MovementKind::Baja, fecha: f });
                } else if c_low.contains("alta") || c_low.contains("reing") {
                    movements.push(EmployeeMovement { kind: MovementKind::Alta, fecha: f });
                }
            }
        }
        movements.sort_by(|a, b| a.fecha.cmp(&b.fecha).then_with(|| a.kind.cmp(&b.kind)));
        movements.dedup();

        // Consultar los detalles originales tal como vinieron en las cédulas
        let orig_detalles = tx.query(
            r#"
            SELECT clave, fecha, dias, sdi, lic, inc, aus, cf,
                   exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr,
                   gps, patronal, obrera, subtotal
            FROM public.detalles_liquidaciones
            WHERE id_liquidacion = $1
            ORDER BY fecha ASC NULLS FIRST, clave ASC NULLS FIRST
            "#,
            &[&id_liq],
        ).map_err(|e| format!("Error leyendo detalles originales para reversión: {}", e))?;

        let mut baja_inserted = false;
        for (target_obra_id, target_dias) in &target_vincs {
            let mut max_dias_for_vinc: Option<i16> = None;
            let mut obra_assigned_for_vinc: Option<Uuid> = None;

            for od in &orig_detalles {
                let clave: Option<String> = od.get(0);
                let is_baja = clave.as_deref().unwrap_or("").trim().to_lowercase().contains("baja");
                if is_baja {
                    if baja_inserted {
                        continue;
                    }
                    baja_inserted = true;
                }

                let id_rep_det = Uuid::now_v7();
                let fecha: Option<NaiveDate> = od.get(1);
                let dias_det: Option<i16> = od.get(2);
                let sdi: Option<f64> = od.get(3);
                let lic: Option<f64> = od.get(4);
                let inc: Option<f64> = od.get(5);
                let aus: Option<f64> = od.get(6);

                let cf: Option<f64> = od.get(7);
                let exc_pat: Option<f64> = od.get(8);
                let exc_obr: Option<f64> = od.get(9);
                let pd_pat: Option<f64> = od.get(10);
                let pd_obr: Option<f64> = od.get(11);
                let gmp_pat: Option<f64> = od.get(12);
                let gmp_obr: Option<f64> = od.get(13);
                let rt: Option<f64> = od.get(14);
                let iv_pat: Option<f64> = od.get(15);
                let iv_obr: Option<f64> = od.get(16);
                let gps: Option<f64> = od.get(17);
                let patronal: Option<f64> = od.get(18);
                let obrera: Option<f64> = od.get(19);
                let subtotal: Option<f64> = od.get(20);

                let (det_obra_id, det_dias_obr) = if is_baja || target_obra_id.is_none() {
                    (None, if is_baja { Some(0) } else { None })
                } else {
                    let target_oid = target_obra_id.unwrap();
                    let d_ced = dias_det.unwrap_or(0);
                    let final_dias = target_dias.unwrap_or(d_ced);
                    obra_assigned_for_vinc = Some(target_oid);
                    max_dias_for_vinc = Some(max_dias_for_vinc.unwrap_or(0).max(final_dias));
                    (Some(target_oid), Some(final_dias))
                };

                tx.execute(
                    r#"
                    INSERT INTO public.reportes_detalles_liquidaciones (
                        id, id_reporte_liquidacion, clave, fecha, dias, sdi, lic, inc, aus, cf,
                        exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr,
                        gps, patronal, obrera, subtotal, id_registro_obra, dias_registro_obra
                    ) VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25
                    )
                    "#,
                    &[
                        &id_rep_det, &id_rep_liq, &clave, &fecha, &dias_det, &sdi, &lic, &inc, &aus,
                        &cf, &exc_pat, &exc_obr, &pd_pat, &pd_obr,
                        &gmp_pat, &gmp_obr, &rt, &iv_pat, &iv_obr,
                        &gps, &patronal, &obrera, &subtotal,
                        &det_obra_id, &det_dias_obr
                    ],
                ).map_err(|e| format!("Error insertando detalle revertido: {}", e))?;

                restored_count += 1;
            }

            // Si se asignó obra en alguno de los detalles, reflejarla en reportes_vinculaciones_obras_liquidaciones
            if let (Some(o_id), Some(d_vinc)) = (obra_assigned_for_vinc, max_dias_for_vinc) {
                let new_vinc_id = Uuid::now_v7();
                tx.execute(
                    r#"
                    INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (
                        id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra
                    ) VALUES ($1, $2, $3, $4)
                    "#,
                    &[&new_vinc_id, &Some(o_id), &id_rep_liq, &Some(d_vinc)],
                ).map_err(|e| format!("Error insertando vinculación revertida: {}", e))?;
            }
        }
    }

    // Bitácora: la reversión restaura originales, así que NO marca editado_usuario.
    // Los reportes borrados conservan su foto "antes" (FK ON DELETE SET NULL en reportes_cambios).
    bitacora_reportes::cerrar_edicion(&mut tx, lote, false)
        .map_err(|e| format!("Error cerrando bitácora de edición: {}", e))?;

    // La reversión borra y recrea reportes: cerrar_edicion no alcanza a los recreados (ids nuevos),
    // así que se recalcula explícitamente el prorrateo canónico de todo el alcance revertido.
    let ids_recreados: Vec<Uuid> = tx.query(
        "SELECT id FROM public.reportes_liquidaciones WHERE id_registro_patronal = ANY($1) AND ($2::uuid IS NULL OR id_revision = $2)",
        &[&all_rp_uuids, &rev_uuid_opt],
    ).map_err(|e| format!("Error consultando reportes recreados: {}", crate::db::schema::format_pg_error(&e)))?
    .into_iter()
    .map(|r| r.get::<_, Uuid>(0))
    .collect();
    crate::db::prorrateo::recalcular_prorrateo_tx(&mut tx, &ids_recreados)
        .map_err(|e| format!("Error recalculando prorrateo tras la reversión: {}", e))?;

    tx.commit().map_err(|e| format!("Error en commit de reversión: {}", e))?;
    crate::app_logger::log_db_op(
        "REVERT_ORIGINAL",
        &[
            "public.reportes_detalles_liquidaciones",
            "public.reportes_vinculaciones_obras_liquidaciones",
        ],
        "revertir_reporte_liquidaciones_a_originales",
        &format!("id_revision={}, registros_restaurados={}", id_revision, restored_count),
        Some(restored_count),
    );
    Ok(restored_count)
}

/// Consulta las filas de reportes_liquidaciones ACTIVO de la revisión que caen dentro del universo
/// (meses de los Registros de Obra del usuario), con sus valores prorrateados y originales listos para la
/// interfaz Slint. El universo se aplica en SQL (`sql_en_universo`), fuente única de verdad: los meses fuera
/// de las RO no se consultan ni se devuelven. Sin ningún COALESCE en la consulta SQL.
pub fn fetch_reporte_liquidaciones(
    client: &mut Client,
    id_revision: Uuid,
) -> Result<Vec<ReporteLiquidacionRowDb>, String> {
    let (rev_uuid_opt, _id_rp, clean_reg, all_rp_uuids) = resolve_rp_context(client, id_revision)?;

    // Verificar si public.reportes_liquidaciones tiene registros para este registro patronal y revisión;
    // Si no tiene registros pero public.liquidaciones sí los tiene, sincronizar al vuelo.
    let count_rep_liq: i64 = client
        .query_one(
            "SELECT COUNT(*)::bigint FROM public.reportes_liquidaciones WHERE id_registro_patronal = ANY($1) AND ($2::uuid IS NULL OR id_revision = $2)",
            &[&all_rp_uuids, &rev_uuid_opt],
        )
        .map(|r| r.get(0))
        .unwrap_or(0);

    let count_base_liq: i64 = client
        .query_one(
            "SELECT COUNT(*)::bigint FROM public.liquidaciones WHERE id_registro_patronal = ANY($1)",
            &[&all_rp_uuids],
        )
        .map(|r| r.get(0))
        .unwrap_or(0);

    if count_rep_liq == 0 && count_base_liq > 0 {
        log::info!(
            "fetch_reporte_liquidaciones: Auto-sincronizando {} liquidaciones para RP {}",
            count_base_liq,
            clean_reg
        );
        let _ = sync_reportes_liquidaciones_for_revision(client, id_revision);
    }

    let base_select = r#"
    SELECT 
        COALESCE(rdl.id, rl.id) AS id_reporte_liq,
        rl.id_liquidacion,
        e.id AS id_empleado,
        COALESCE(e.nombre, 'TRABAJADOR SIN REGISTRO') AS nombre_trabajador,
        COALESCE(e.rfc, '') AS rfc,
        COALESCE(e.curp, '') AS curp,
        COALESCE(e.nss, '') AS nss,
        COALESCE(rp.registro, '') AS registro_patronal,
        rl.mes_periodo,
        rl.anio_periodo,
        COALESCE(rdl.dias, 0) AS dias_cedula,
        COALESCE(rdl.id_registro_obra, (SELECT rvol.id_registro_obra FROM public.reportes_vinculaciones_obras_liquidaciones rvol WHERE rvol.id_reporte_liquidacion = rl.id LIMIT 1)) AS id_registro_obra,
        COALESCE(ro_det.numero, '') AS obra_numero,
        COALESCE(ro_det.tipo_obra, '') AS tipo_obra,
        COALESCE(rdl.dias_registro_obra, (SELECT rvol.dias_registro_obra FROM public.reportes_vinculaciones_obras_liquidaciones rvol WHERE rvol.id_reporte_liquidacion = rl.id AND rvol.id_registro_obra = ro_det.id LIMIT 1), rdl.dias, 0) AS dias_obra,
        COALESCE(rdl.cf, 0.0) AS cf,
        COALESCE(rdl.exc_pat, 0.0) AS exc_pat,
        COALESCE(rdl.exc_obr, 0.0) AS exc_obr,
        COALESCE(rdl.pd_pat, 0.0) AS pd_pat,
        COALESCE(rdl.pd_obr, 0.0) AS pd_obr,
        COALESCE(rdl.gmp_pat, 0.0) AS gmp_pat,
        COALESCE(rdl.gmp_obr, 0.0) AS gmp_obr,
        COALESCE(rdl.rt, 0.0) AS rt,
        COALESCE(rdl.iv_pat, 0.0) AS iv_pat,
        COALESCE(rdl.iv_obr, 0.0) AS iv_obr,
        COALESCE(rdl.gps, 0.0) AS gps,
        COALESCE(rdl.patronal, 0.0) AS patronal,
        COALESCE(rdl.obrera, 0.0) AS obrera,
        COALESCE(rdl.subtotal, 0.0) AS subtotal_prorrateado,
        COALESCE(dl.cf, rdl.cf, 0.0) AS cf_orig,
        COALESCE(dl.exc_pat, rdl.exc_pat, 0.0) AS exc_pat_orig,
        COALESCE(dl.exc_obr, rdl.exc_obr, 0.0) AS exc_obr_orig,
        COALESCE(dl.pd_pat, rdl.pd_pat, 0.0) AS pd_pat_orig,
        COALESCE(dl.pd_obr, rdl.pd_obr, 0.0) AS pd_obr_orig,
        COALESCE(dl.gmp_pat, rdl.gmp_pat, 0.0) AS gmp_pat_orig,
        COALESCE(dl.gmp_obr, rdl.gmp_obr, 0.0) AS gmp_obr_orig,
        COALESCE(dl.rt, rdl.rt, 0.0) AS rt_orig,
        COALESCE(dl.iv_pat, rdl.iv_pat, 0.0) AS iv_pat_orig,
        COALESCE(dl.iv_obr, rdl.iv_obr, 0.0) AS iv_obr_orig,
        COALESCE(dl.gps, rdl.gps, 0.0) AS gps_orig,
        COALESCE(dl.patronal, rdl.patronal, 0.0) AS patronal_orig,
        COALESCE(dl.obrera, rdl.obrera, 0.0) AS obrera_orig,
        COALESCE(dl.subtotal, rdl.subtotal, 0.0) AS subtotal_orig,
        COALESCE(rdl.clave, '') AS clave,
        rdl.fecha,
        COALESCE(rdl.sdi, 0.0) AS sdi,
        COALESCE(rdl.lic, 0.0) AS lic,
        COALESCE(rdl.inc, 0.0) AS inc,
        COALESCE(rdl.aus, 0.0) AS aus,
        rl.id AS id_parent_reporte_liq,
        COALESCE(ro_det.creado_usuario, false) AS obra_creado_usuario,
        COALESCE(TRIM(e.pensionado), '') AS pensionado,
        rl.id_revision,
        rl.mes_periodo_int,
        COALESCE(rdl.tiene_inconsistencia, false) AS tiene_inconsistencia,
        COALESCE(rdl.observacion_validacion, '') AS observacion_validacion,
        rl.estado::text AS estado_reporte,
        rl.cedula_actualizada,
        rl.editado_usuario
    FROM public.reportes_liquidaciones AS rl
    LEFT JOIN public.reportes_detalles_liquidaciones AS rdl
        ON rl.id = rdl.id_reporte_liquidacion
    LEFT JOIN public.registros_obras AS ro_det
        ON ro_det.id = COALESCE(rdl.id_registro_obra, (
            SELECT rvol.id_registro_obra 
            FROM public.reportes_vinculaciones_obras_liquidaciones rvol 
            WHERE rvol.id_reporte_liquidacion = rl.id 
            LIMIT 1
        ))
    LEFT JOIN LATERAL (
        SELECT sub_dl.*
        FROM public.detalles_liquidaciones sub_dl
        WHERE sub_dl.id_liquidacion = rl.id_liquidacion
        ORDER BY 
            (CASE 
                WHEN (sub_dl.clave = rdl.clave OR (sub_dl.clave IS NULL AND rdl.clave IS NULL)) 
                     AND (sub_dl.fecha = rdl.fecha OR (sub_dl.fecha IS NULL AND rdl.fecha IS NULL)) THEN 0 
                WHEN TRIM(LOWER(COALESCE(sub_dl.clave, ''))) = TRIM(LOWER(COALESCE(rdl.clave, ''))) THEN 1 
                ELSE 2 
            END) ASC
        LIMIT 1
    ) dl ON true
    LEFT JOIN public.empleados AS e
        ON rl.id_empleado = e.id
    LEFT JOIN public.registros_patronales AS rp
        ON rl.id_registro_patronal = rp.id
    "#;

    let order_clause = r#"
        ORDER BY rl.anio_periodo ASC,
            COALESCE(
                rl.mes_periodo_int,
                CASE UPPER(TRIM(rl.mes_periodo))
                    WHEN 'ENERO' THEN 1
                    WHEN 'FEBRERO' THEN 2
                    WHEN 'MARZO' THEN 3
                    WHEN 'ABRIL' THEN 4
                    WHEN 'MAYO' THEN 5
                    WHEN 'JUNIO' THEN 6
                    WHEN 'JULIO' THEN 7
                    WHEN 'AGOSTO' THEN 8
                    WHEN 'SEPTIEMBRE' THEN 9
                    WHEN 'OCTUBRE' THEN 10
                    WHEN 'NOVIEMBRE' THEN 11
                    WHEN 'DICIEMBRE' THEN 12
                    ELSE 99
                END
            ) ASC,
            e.nombre ASC;
    "#;

    // Universo por fila (su propia revisión); solo ACTIVO. Meses fuera de las RO no se consultan ni se pintan.
    let universo = sql_en_universo("rl.id_revision", "rl.anio_periodo", SQL_MES_RL);
    let where_clause = format!(
        "WHERE rl.id_registro_patronal = ANY($1) AND ($2::uuid IS NULL OR rl.id_revision = $2) AND rl.estado::text = 'ACTIVO' AND {universo}"
    );
    let full_sql = format!("{} {} {}", base_select, where_clause, order_clause);
    let rows = client.query(&full_sql, &[&all_rp_uuids, &rev_uuid_opt])
        .map_err(|e| format!("Error consultando filas de reportes_liquidaciones: {}", e))?;

    crate::app_logger::log_db_read(
        &[
            "public.reportes_liquidaciones",
            "public.reportes_detalles_liquidaciones",
            "public.reportes_vinculaciones_obras_liquidaciones",
            "public.detalles_liquidaciones",
            "public.empleados",
            "public.registros_obras",
        ],
        "SELECT rdl.id AS id_reporte_liq, rl.id_liquidacion, e.id AS id_empleado ... FROM reportes_liquidaciones rl JOIN reportes_detalles_liquidaciones rdl ...",
        &format!("id_revision={}", id_revision),
        rows.len(),
    );

    let mut result = Vec::with_capacity(rows.len());

    for r in rows {
        let id_rep_liq: Uuid = match r.try_get(0) {
            Ok(v) => v,
            Err(e) => {
                log::error!("Error deserializando id_reporte_liq: {}", e);
                continue;
            }
        };
        let id_liq: Uuid = r.try_get(1).unwrap_or_default();
        let id_emp: Uuid = r.try_get(2).unwrap_or_default();
        let nom: String = r.try_get::<_, Option<String>>(3).ok().flatten().unwrap_or_default();
        let rfc: String = r.try_get::<_, Option<String>>(4).ok().flatten().unwrap_or_default();
        let curp: String = r.try_get::<_, Option<String>>(5).ok().flatten().unwrap_or_default();
        let nss: String = r.try_get::<_, Option<String>>(6).ok().flatten().unwrap_or_default();
        let rp: String = r.try_get::<_, Option<String>>(7).ok().flatten().unwrap_or_default();
        let mes: String = r.try_get::<_, Option<String>>(8).ok().flatten().unwrap_or_default();
        let anio: i16 = row_get_i16(&r, 9, 0);
        let dias_ced: i16 = row_get_i16(&r, 10, 0);
        let id_obra_opt: Option<Uuid> = r.try_get(11).ok().flatten();
        let obra_num: String = r.try_get::<_, Option<String>>(12).ok().flatten().unwrap_or_default();
        let tipo_obra: String = r.try_get::<_, Option<String>>(13).ok().flatten().unwrap_or_default();
        let dias_obra: i16 = if id_obra_opt.is_some() { row_get_i16(&r, 14, 0) } else { 0 };

        let mes_clean = mes.trim().to_uppercase();
        let periodo_str = if !mes_clean.is_empty() {
            format!("{} / {}", mes_clean, anio)
        } else {
            format!("{:04}", anio)
        };

        let obra_label = if id_obra_opt.is_some() {
            if tipo_obra.trim().is_empty() {
                obra_num.trim().to_string()
            } else {
                format!("{} - {}", obra_num.trim(), tipo_obra.trim())
            }
        } else {
            "⚠️ Sin Asignar (Pendiente)".to_string()
        };

        let clave_raw: String = r.try_get::<_, Option<String>>(43).ok().flatten().unwrap_or_default();
        let fecha_opt: Option<NaiveDate> = r.try_get(44).ok().flatten();
        let fecha_str = fecha_opt
            .map(|d| d.format("%d/%m/%Y").to_string())
            .unwrap_or_else(|| "-".to_string());
        let sdi = row_get_f64(&r, 45, 0.0);
        let lic = row_get_f64(&r, 46, 0.0);
        let inc = row_get_f64(&r, 47, 0.0);
        let aus = row_get_f64(&r, 48, 0.0);

        let id_parent_rep: Uuid = r.try_get(49).unwrap_or_default();
        let obra_creado_usr: bool = r.try_get(50).unwrap_or(false);
        let emp_pensionado: String = r.try_get::<_, Option<String>>(51).ok().flatten().unwrap_or_default();
        let row_id_revision: Uuid = r.try_get(52).unwrap_or(id_revision);
        let row_mes_int: i16 = r
            .try_get::<_, Option<i16>>(53)
            .ok()
            .flatten()
            .unwrap_or_else(|| mes_to_periodo_int(&mes_clean));
        let tiene_inconsistencia: bool = r.try_get(54).unwrap_or(false);
        let observacion_validacion: String = r.try_get::<_, Option<String>>(55).ok().flatten().unwrap_or_default();
        // Columnas 56-58: estado, cedula_actualizada, editado_usuario (con valores por defecto si faltan)
        let estado: String = r.try_get::<_, Option<String>>(56).ok().flatten().unwrap_or_else(|| "ACTIVO".to_string());
        let cedula_actualizada: bool = r.try_get::<_, Option<bool>>(57).ok().flatten().unwrap_or(false);
        let editado_usuario: bool = r.try_get::<_, Option<bool>>(58).ok().flatten().unwrap_or(false);

        result.push(ReporteLiquidacionRowDb {
            id_reporte_liq: id_rep_liq,
            id_parent_reporte_liq: id_parent_rep,
            id_liquidacion: id_liq,
            id_revision: row_id_revision,
            id_empleado: id_emp,
            nombre_trabajador: nom.trim().to_string(),
            rfc: rfc.trim().to_string(),
            curp: curp.trim().to_string(),
            nss: nss.trim().to_string(),
            pensionado: emp_pensionado.trim().to_string(),
            registro_patronal: rp.trim().to_string(),
            mes_periodo: mes_clean.to_string(),
            mes_periodo_int: row_mes_int,
            anio_periodo: anio,
            periodo_str,
            dias_cedula: dias_ced,
            id_registro_obra: id_obra_opt,
            obra_numero: obra_num.trim().to_string(),
            obra_label,
            dias_obra,
            obra_creado_usuario: obra_creado_usr,

            clave: clave_raw.trim().to_string(),
            fecha: fecha_opt,
            fecha_str,
            sdi,
            lic,
            inc,
            aus,

            cf: row_get_f64(&r, 15, 0.0),
            exc_pat: row_get_f64(&r, 16, 0.0),
            exc_obr: row_get_f64(&r, 17, 0.0),
            pd_pat: row_get_f64(&r, 18, 0.0),
            pd_obr: row_get_f64(&r, 19, 0.0),
            gmp_pat: row_get_f64(&r, 20, 0.0),
            gmp_obr: row_get_f64(&r, 21, 0.0),
            rt: row_get_f64(&r, 22, 0.0),
            iv_pat: row_get_f64(&r, 23, 0.0),
            iv_obr: row_get_f64(&r, 24, 0.0),
            gps: row_get_f64(&r, 25, 0.0),
            patronal: row_get_f64(&r, 26, 0.0),
            obrera: row_get_f64(&r, 27, 0.0),
            subtotal: row_get_f64(&r, 28, 0.0),

            cf_orig: row_get_f64(&r, 29, 0.0),
            exc_pat_orig: row_get_f64(&r, 30, 0.0),
            exc_obr_orig: row_get_f64(&r, 31, 0.0),
            pd_pat_orig: row_get_f64(&r, 32, 0.0),
            pd_obr_orig: row_get_f64(&r, 33, 0.0),
            gmp_pat_orig: row_get_f64(&r, 34, 0.0),
            gmp_obr_orig: row_get_f64(&r, 35, 0.0),
            rt_orig: row_get_f64(&r, 36, 0.0),
            iv_pat_orig: row_get_f64(&r, 37, 0.0),
            iv_obr_orig: row_get_f64(&r, 38, 0.0),
            gps_orig: row_get_f64(&r, 39, 0.0),
            patronal_orig: row_get_f64(&r, 40, 0.0),
            obrera_orig: row_get_f64(&r, 41, 0.0),
            subtotal_orig: row_get_f64(&r, 42, 0.0),

            tiene_inconsistencia,
            observacion_validacion,
            estado,
            cedula_actualizada,
            editado_usuario,
        });
    }

    // Registro de versiones para el bloqueo optimista (D-6 a). Un fallo aquí no rompe la carga.
    if let Some(rev) = rev_uuid_opt {
        match client.query(
            "SELECT id, version FROM public.reportes_liquidaciones WHERE id_revision = $1",
            &[&rev],
        ) {
            Ok(filas) => {
                let pares: Vec<(Uuid, i32)> = filas.iter().map(|r| (r.get(0), r.get(1))).collect();
                crate::db::analistas::registrar_versiones(&pares);
            }
            Err(e) => log::warn!(
                "No se pudo registrar versiones de reportes para bloqueo optimista: {}",
                crate::db::schema::format_pg_error(&e)
            ),
        }
    }

    Ok(result)
}

/// Actualiza en tiempo real los días o el registro de obra de un empleado en la base de datos,
/// recalculando automáticamente los conceptos prorrateados en public.reportes_detalles_liquidaciones
/// Actualiza en base de datos las filas del reporte en una única transacción atómica.
/// Actualiza días cédula en reportes_liquidaciones, vinculación en reportes_vinculaciones_obras_liquidaciones
/// y recalcula conceptos prorrateados en reportes_detalles_liquidaciones.
pub fn batch_save_reporte_row_updates(
    client: &mut Client,
    rows: &[(Uuid, i16, i16, Option<Uuid>)],
) -> Result<usize, String> {
    if rows.is_empty() {
        return Ok(0);
    }

    let mut tx = client.transaction().map_err(|e| format!("Error iniciando tx de actualización: {}", e))?;

    // Bitácora: foto "antes" de los reportes afectados (rows[i].0 son ids de TRAMO)
    let ids_tramos: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let ids_reportes = bitacora_reportes::reportes_de_tramos(&mut tx, &ids_tramos)
        .map_err(|e| format!("Error resolviendo reportes para bitácora: {}", e))?;
    let lote = bitacora_reportes::iniciar_edicion(&mut tx, ACCION_DIAS, &ids_reportes)
        .map_err(|e| format!("Error iniciando bitácora de edición: {}", e))?;

    let mut updated_count = 0;

    for &(id_reporte_liq, dias_cedula, dias_obra, id_registro_obra_opt) in rows {
        // 1. Obtener la información del detalle, incidencias y sus padres (reportes_liquidaciones y liquidaciones)
        let parent_info = tx.query_opt(
            r#"
            SELECT 
                rdl.id_reporte_liquidacion, 
                rl.id_liquidacion, 
                rdl.clave, 
                rdl.fecha,
                (COALESCE(rdl.lic, 0.0) + COALESCE(rdl.inc, 0.0) + COALESCE(rdl.aus, 0.0))::float8
            FROM public.reportes_detalles_liquidaciones rdl
            JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
            WHERE rdl.id = $1
            "#,
            &[&id_reporte_liq],
        ).map_err(|e| format!("Error consultando detalle de reporte: {}", e))?;

        let (parent_id, id_liq, clave_opt, fecha_opt, _sum_inc) = match parent_info {
            Some(r) => {
                let pid: Uuid = r.get(0);
                let lid: Uuid = r.get(1);
                let clv: Option<String> = r.try_get(2).ok().flatten();
                let fch: Option<NaiveDate> = r.try_get(3).ok().flatten();
                let sinc: f64 = r.try_get(4).unwrap_or(0.0);
                (pid, lid, clv, fch, sinc)
            }
            None => continue,
        };

        let dias_obra_val = dias_obra.clamp(0, 31);
        let dias_base = dias_cedula as f64;
        let factor = if dias_base > 0.0 && id_registro_obra_opt.is_some() {
            (dias_obra_val as f64 / dias_base).clamp(0.0, 10.0)
        } else {
            1.0
        };

        // 2. Actualizar días cédula, obra, días obra y recalcular conceptos prorrateados en una sola sentencia atómica
        match tx.execute(
            r#"
            UPDATE public.reportes_detalles_liquidaciones rdl
            SET 
                dias = $2,
                dias_registro_obra = $3,
                id_registro_obra = $4,
                cf = ROUND((COALESCE(dl.cf, rdl.cf, 0.0) * $5)::numeric, 2)::float8,
                exc_pat = ROUND((COALESCE(dl.exc_pat, rdl.exc_pat, 0.0) * $5)::numeric, 2)::float8,
                exc_obr = ROUND((COALESCE(dl.exc_obr, rdl.exc_obr, 0.0) * $5)::numeric, 2)::float8,
                pd_pat = ROUND((COALESCE(dl.pd_pat, rdl.pd_pat, 0.0) * $5)::numeric, 2)::float8,
                pd_obr = ROUND((COALESCE(dl.pd_obr, rdl.pd_obr, 0.0) * $5)::numeric, 2)::float8,
                gmp_pat = ROUND((COALESCE(dl.gmp_pat, rdl.gmp_pat, 0.0) * $5)::numeric, 2)::float8,
                gmp_obr = ROUND((COALESCE(dl.gmp_obr, rdl.gmp_obr, 0.0) * $5)::numeric, 2)::float8,
                rt = ROUND((COALESCE(dl.rt, rdl.rt, 0.0) * $5)::numeric, 2)::float8,
                iv_pat = ROUND((COALESCE(dl.iv_pat, rdl.iv_pat, 0.0) * $5)::numeric, 2)::float8,
                iv_obr = ROUND((COALESCE(dl.iv_obr, rdl.iv_obr, 0.0) * $5)::numeric, 2)::float8,
                gps = ROUND((COALESCE(dl.gps, rdl.gps, 0.0) * $5)::numeric, 2)::float8,
                patronal = ROUND((COALESCE(dl.patronal, rdl.patronal, 0.0) * $5)::numeric, 2)::float8,
                obrera = ROUND((COALESCE(dl.obrera, rdl.obrera, 0.0) * $5)::numeric, 2)::float8,
                subtotal = ROUND((COALESCE(dl.subtotal, rdl.subtotal, 0.0) * $5)::numeric, 2)::float8
            FROM public.detalles_liquidaciones dl
            WHERE rdl.id = $1
              AND dl.id_liquidacion = $6
              AND (
                  (COALESCE(TRIM(LOWER(dl.clave)), '') = COALESCE(TRIM(LOWER(rdl.clave)), '')
                   AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL)))
                  OR (
                      (SELECT COUNT(*) FROM public.detalles_liquidaciones count_dl WHERE count_dl.id_liquidacion = $6) = 1
                  )
              )
            "#,
            &[&id_reporte_liq, &dias_cedula, &Some(dias_obra_val), &id_registro_obra_opt, &factor, &id_liq],
        ) {
            Ok(n) => {
                crate::app_logger::log_db_op(
                    "UPDATE",
                    &["public.reportes_detalles_liquidaciones", "public.detalles_liquidaciones"],
                    "UPDATE reportes_detalles_liquidaciones (dias, obra, prorrateo)",
                    &format!("id={}, factor={:.4}, dias_base={:.1}, dias_cedula={}, dias_obra={}", id_reporte_liq, factor, dias_base, dias_cedula, dias_obra_val),
                    Some(n as usize),
                );
            }
            Err(e) => {
                crate::app_logger::log_db_error(
                    "UPDATE",
                    &["public.reportes_detalles_liquidaciones"],
                    "UPDATE consolidado reportes_detalles_liquidaciones",
                    &e.to_string(),
                );
                return Err(format!("Error actualizando reporte detalle y recálculo: {}", e));
            }
        }

        // 3. Sincronizar días en public.detalles_liquidaciones para mantener consistencia con la base de datos
        match tx.execute(
            r#"
            UPDATE public.detalles_liquidaciones
            SET dias = $2
            WHERE id_liquidacion = $1
              AND (
                  (COALESCE(TRIM(LOWER(clave)), '') = COALESCE(TRIM(LOWER($3)), '')
                   AND (fecha = $4 OR (fecha IS NULL AND $4 IS NULL)))
                  OR (
                      (SELECT COUNT(*) FROM public.detalles_liquidaciones sub_dl WHERE sub_dl.id_liquidacion = $1) = 1
                  )
              )
            "#,
            &[&id_liq, &dias_cedula, &clave_opt, &fecha_opt],
        ) {
            Ok(n) => {
                crate::app_logger::log_db_op(
                    "UPDATE",
                    &["public.detalles_liquidaciones"],
                    "UPDATE public.detalles_liquidaciones SET dias=$2 WHERE id_liquidacion=$1",
                    &format!("id_liq={}, dias_cedula={}, clave={:?}, fecha={:?}", id_liq, dias_cedula, clave_opt, fecha_opt),
                    Some(n as usize),
                );
            }
            Err(e) => {
                crate::app_logger::log_db_error(
                    "UPDATE",
                    &["public.detalles_liquidaciones"],
                    "UPDATE public.detalles_liquidaciones SET dias=$2",
                    &e.to_string(),
                );
            }
        }

        // 5. Sincronizar reportes_vinculaciones_obras_liquidaciones para este id_reporte_liquidacion
        tx.execute(
            "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1",
            &[&parent_id],
        ).map_err(|e| format!("Error eliminando vinculaciones en reporte: {}", e))?;
        tx.execute(
            r#"
            INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (
                id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra
            )
            SELECT uuidv7(), id_registro_obra, id_reporte_liquidacion, SUM(COALESCE(dias_registro_obra, 0))::smallint
            FROM public.reportes_detalles_liquidaciones
            WHERE id_reporte_liquidacion = $1 AND id_registro_obra IS NOT NULL
            GROUP BY id_registro_obra, id_reporte_liquidacion
            "#,
            &[&parent_id],
        ).map_err(|e| format!("Error actualizando vinculaciones en reporte: {}", e))?;

        // La tabla MAESTRA vinculaciones_obras_liquidaciones refleja solo el PDF de la cédula:
        // las ediciones del analista viven únicamente en las tablas de reporte (rdl/rvol).

        updated_count += 1;
    }

    // Bitácora: foto "después" + prorrateo canónico, dentro de la misma tx
    bitacora_reportes::cerrar_edicion(&mut tx, lote, true)
        .map_err(|e| format!("Error cerrando bitácora de edición: {}", e))?;

    match tx.commit() {
        Ok(_) => {
            crate::app_logger::log_db_op(
                "COMMIT",
                &[
                    "public.reportes_detalles_liquidaciones",
                    "public.detalles_liquidaciones",
                    "public.reportes_vinculaciones_obras_liquidaciones",
                    "public.vinculaciones_obras_liquidaciones",
                ],
                "COMMIT TRANSACTION (guardado exitoso de modificaciones en reporte)",
                &format!("Lote finalizado con éxito: {} filas persistidas", updated_count),
                Some(updated_count),
            );
        }
        Err(e) => {
            crate::app_logger::log_db_error(
                "COMMIT",
                &["public.reportes_detalles_liquidaciones"],
                "COMMIT TRANSACTION",
                &e.to_string(),
            );
            return Err(format!("Error en commit de actualización: {}", e));
        }
    }
    Ok(updated_count)
}

/// Limpia y repara duplicados en `public.reportes_detalles_liquidaciones` en todas las revisiones:
/// 1. Si un empleado tiene múltiples filas vinculadas a la misma obra en el mismo mes y movimiento,
///    se verifica si en `vinculaciones_obras_liquidaciones` existen otras obras asignadas que no están en reportes_detalles_liquidaciones.
/// 2. Si existen obras faltantes, se reasigna la fila duplicada a la obra faltante con sus días y prorrateo correspondiente.
/// 3. Si no hay obras faltantes (duplicados redundantes idénticos), se eliminan las filas duplicadas excedentes conservando la primaria.
/// 4. Sincroniza `public.reportes_vinculaciones_obras_liquidaciones`.
/// 5. Re-evalúa el trigger de auditoría de días/inconsistencias.
pub fn cleanup_all_duplicate_reportes_detalles(client: &mut Client) -> Result<(usize, usize), String> {
    let mut tx = client.transaction().map_err(|e| format!("Error iniciando tx de limpieza: {}", e))?;

    // 1. Obtener todos los reportes_liquidaciones con filas duplicadas (misma obra y movimiento o misma obra no baja)
    let dup_parents = tx.query(
        r#"
        SELECT DISTINCT rdl.id_reporte_liquidacion, rl.id_liquidacion
        FROM public.reportes_detalles_liquidaciones rdl
        JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
        WHERE rdl.id_registro_obra IS NOT NULL
          AND NOT (LOWER(TRIM(COALESCE(rdl.clave, ''))) = 'baja')
        GROUP BY rdl.id_reporte_liquidacion, rl.id_liquidacion, rdl.id_registro_obra,
                 COALESCE(TRIM(LOWER(rdl.clave)), ''), COALESCE(rdl.fecha, '1900-01-01'::date)
        HAVING COUNT(*) > 1
        "#,
        &[],
    ).map_err(|e| format!("Error consultando duplicados: {}", e))?;

    let mut reassigned_count = 0;
    let mut deleted_count = 0;
    let mut affected_parents: HashSet<Uuid> = HashSet::new();

    for parent_row in dup_parents {
        let parent_id: Uuid = parent_row.get(0);
        let id_liq: Uuid = parent_row.get(1);
        affected_parents.insert(parent_id);

        // Obras actualmente presentes en este empleado
        let cur_obras_rows = tx.query(
            "SELECT DISTINCT id_registro_obra FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion = $1 AND id_registro_obra IS NOT NULL",
            &[&parent_id],
        ).unwrap_or_default();
        let mut cur_obras: HashSet<Uuid> = cur_obras_rows.into_iter().filter_map(|r| r.get::<_, Option<Uuid>>(0)).collect();

        // Obras maestras en vinculaciones_obras_liquidaciones
        let master_vols = tx.query(
            "SELECT id_registro_obra, COALESCE(dias_registro_obra, 30::smallint) FROM public.vinculaciones_obras_liquidaciones WHERE id_liquidacion = $1 AND id_registro_obra IS NOT NULL",
            &[&id_liq],
        ).unwrap_or_default();
        let mut missing_master_obras: Vec<(Uuid, i16)> = Vec::new();
        for mvr in master_vols {
            if let Some(roid) = mvr.get::<_, Option<Uuid>>(0) {
                if !cur_obras.contains(&roid) {
                    let d: i16 = row_get_i16(&mvr, 1, 30);
                    missing_master_obras.push((roid, d));
                }
            }
        }

        // Consultar grupos duplicados para este parent_id
        let dup_groups = tx.query(
            r#"
            SELECT id_registro_obra, COALESCE(TRIM(LOWER(clave)), ''), COALESCE(fecha, '1900-01-01'::date), COUNT(*)
            FROM public.reportes_detalles_liquidaciones
            WHERE id_reporte_liquidacion = $1
              AND id_registro_obra IS NOT NULL
              AND NOT (LOWER(TRIM(COALESCE(clave, ''))) = 'baja')
            GROUP BY id_registro_obra, COALESCE(TRIM(LOWER(clave)), ''), COALESCE(fecha, '1900-01-01'::date)
            HAVING COUNT(*) > 1
            "#,
            &[&parent_id],
        ).unwrap_or_default();

        for dg in dup_groups {
            let roid: Option<Uuid> = dg.get(0);
            let clv_val: String = dg.get(1);
            let f_val: NaiveDate = dg.get(2);

            let dup_rows = tx.query(
                r#"
                SELECT id, dias, cf, exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr, gps, patronal, obrera, subtotal
                FROM public.reportes_detalles_liquidaciones
                WHERE id_reporte_liquidacion = $1
                  AND id_registro_obra = $2
                  AND COALESCE(TRIM(LOWER(clave)), '') = $3
                  AND COALESCE(fecha, '1900-01-01'::date) = $4
                ORDER BY created_at ASC, id ASC
                "#,
                &[&parent_id, &roid, &clv_val, &f_val],
            ).unwrap_or_default();

            if dup_rows.len() <= 1 {
                continue;
            }

            // Consultar valores originales en detalles_liquidaciones para cálculo exacto de prorrateo
            let orig_dl_opt = tx.query_opt(
                r#"
                SELECT dl.cf, dl.exc_pat, dl.exc_obr, dl.pd_pat, dl.pd_obr, dl.gmp_pat, dl.gmp_obr, dl.rt, dl.iv_pat, dl.iv_obr, dl.gps, dl.patronal, dl.obrera, dl.subtotal
                FROM public.detalles_liquidaciones dl
                WHERE dl.id_liquidacion = $1
                  AND COALESCE(TRIM(LOWER(dl.clave)), '') = $2
                  AND COALESCE(dl.fecha, '1900-01-01'::date) = $3
                LIMIT 1
                "#,
                &[&id_liq, &clv_val, &f_val],
            ).unwrap_or(None);

            // Mantener dup_rows[0], procesar dup_rows[1..]
            for extra_row in &dup_rows[1..] {
                let extra_id: Uuid = extra_row.get(0);
                let dias_ced: i16 = row_get_i16(extra_row, 1, 30);

                if let Some((missing_roid, missing_dias)) = missing_master_obras.pop() {
                    // Reasignar la fila a la obra maestra faltante
                    cur_obras.insert(missing_roid);
                    let f = if dias_ced > 0 {
                        (missing_dias as f64 / dias_ced as f64).clamp(0.0, 10.0)
                    } else {
                        1.0
                    };
                    let calc_p = |orig_val: f64| -> f64 {
                        (orig_val * f * 100.0).round() / 100.0
                    };

                    let (cf_p, ep_p, eo_p, pp_p, po_p, gp_p, go_p, rt_p, ip_p, io_p, gps_p, pat_p, obr_p, sub_p) = if let Some(ref odl) = orig_dl_opt {
                        (
                            calc_p(odl.get(0)), calc_p(odl.get(1)), calc_p(odl.get(2)),
                            calc_p(odl.get(3)), calc_p(odl.get(4)), calc_p(odl.get(5)),
                            calc_p(odl.get(6)), calc_p(odl.get(7)), calc_p(odl.get(8)),
                            calc_p(odl.get(9)), calc_p(odl.get(10)), calc_p(odl.get(11)),
                            calc_p(odl.get(12)), calc_p(odl.get(13)),
                        )
                    } else {
                        (
                            calc_p(extra_row.get(2)), calc_p(extra_row.get(3)), calc_p(extra_row.get(4)),
                            calc_p(extra_row.get(5)), calc_p(extra_row.get(6)), calc_p(extra_row.get(7)),
                            calc_p(extra_row.get(8)), calc_p(extra_row.get(9)), calc_p(extra_row.get(10)),
                            calc_p(extra_row.get(11)), calc_p(extra_row.get(12)), calc_p(extra_row.get(13)),
                            calc_p(extra_row.get(14)), calc_p(extra_row.get(15)),
                        )
                    };

                    tx.execute(
                        r#"
                        UPDATE public.reportes_detalles_liquidaciones
                        SET id_registro_obra = $1, dias_registro_obra = $2,
                            cf = $3, exc_pat = $4, exc_obr = $5, pd_pat = $6, pd_obr = $7,
                            gmp_pat = $8, gmp_obr = $9, rt = $10, iv_pat = $11, iv_obr = $12,
                            gps = $13, patronal = $14, obrera = $15, subtotal = $16,
                            tiene_inconsistencia = false, observacion_validacion = ''
                        WHERE id = $17
                        "#,
                        &[
                            &missing_roid, &missing_dias,
                            &cf_p, &ep_p, &eo_p, &pp_p, &po_p, &gp_p, &go_p, &rt_p, &ip_p, &io_p, &gps_p, &pat_p, &obr_p, &sub_p,
                            &extra_id,
                        ],
                    ).map_err(|e| format!("Error reasignando obra faltante a fila duplicada {}: {}", extra_id, e))?;
                    reassigned_count += 1;
                } else {
                    // No hay más obras maestras pendientes: eliminar el clon redundante
                    tx.execute(
                        "DELETE FROM public.reportes_detalles_liquidaciones WHERE id = $1",
                        &[&extra_id],
                    ).map_err(|e| format!("Error eliminando fila duplicada {}: {}", extra_id, e))?;
                    deleted_count += 1;
                }
            }
        }
    }

    // 2. Limpiar duplicados donde id_registro_obra es NULL
    let null_dups = tx.query(
        r#"
        SELECT id_reporte_liquidacion, COALESCE(TRIM(LOWER(clave)), ''), COALESCE(fecha, '1900-01-01'::date), COUNT(*)
        FROM public.reportes_detalles_liquidaciones
        WHERE id_registro_obra IS NULL
        GROUP BY id_reporte_liquidacion, COALESCE(TRIM(LOWER(clave)), ''), COALESCE(fecha, '1900-01-01'::date)
        HAVING COUNT(*) > 1
        "#,
        &[],
    ).unwrap_or_default();

    for nd in null_dups {
        let parent_id: Uuid = nd.get(0);
        let clv: String = nd.get(1);
        let f: NaiveDate = nd.get(2);
        affected_parents.insert(parent_id);

        let n_rows = tx.query(
            r#"
            SELECT id FROM public.reportes_detalles_liquidaciones
            WHERE id_reporte_liquidacion = $1
              AND id_registro_obra IS NULL
              AND COALESCE(TRIM(LOWER(clave)), '') = $2
              AND COALESCE(fecha, '1900-01-01'::date) = $3
            ORDER BY created_at ASC, id ASC
            "#,
            &[&parent_id, &clv, &f],
        ).unwrap_or_default();

        for extra in &n_rows[1..] {
            let eid: Uuid = extra.get(0);
            tx.execute("DELETE FROM public.reportes_detalles_liquidaciones WHERE id = $1", &[&eid])
                .map_err(|e| format!("Error eliminando nulo duplicado {}: {}", eid, e))?;
            deleted_count += 1;
        }
    }

    // 3. Sincronizar reportes_vinculaciones_obras_liquidaciones y re-evaluar triggers para todos los padres afectados
    for pid in affected_parents {
        tx.execute(
            "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1",
            &[&pid],
        ).map_err(|e| format!("Error limpiando vinculaciones reporte {}: {}", pid, crate::db::schema::format_pg_error(&e)))?;

        tx.execute(
            r#"
            INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (
                id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra
            )
            SELECT uuidv7(), id_registro_obra, id_reporte_liquidacion, LEAST(31::smallint, GREATEST(0::smallint, SUM(COALESCE(dias_registro_obra, 0))::smallint))
            FROM public.reportes_detalles_liquidaciones
            WHERE id_reporte_liquidacion = $1 AND id_registro_obra IS NOT NULL
            GROUP BY id_registro_obra, id_reporte_liquidacion
            "#,
            &[&pid],
        ).map_err(|e| format!("Error reinsertando vinculaciones reporte {}: {}", pid, crate::db::schema::format_pg_error(&e)))?;

        let _ = tx.execute(
            "UPDATE public.reportes_detalles_liquidaciones SET id = id WHERE id_reporte_liquidacion = $1",
            &[&pid],
        );
    }

    tx.commit().map_err(|e| format!("Error commit de limpieza global: {}", e))?;
    Ok((reassigned_count, deleted_count))
}

/// Actualiza un registro individual de reporte de liquidaciones
pub fn update_single_reporte_row(
    client: &mut Client,
    id_reporte_liq: Uuid,
    dias_cedula: i16,
    dias_obra: i16,
    id_registro_obra_opt: Option<Uuid>,
) -> Result<(), String> {
    batch_save_reporte_row_updates(client, &[(id_reporte_liq, dias_cedula, dias_obra, id_registro_obra_opt)])?;
    Ok(())
}

/// Verifica si dos obras se superponen en fechas dentro de un mes y año específico
pub fn check_obras_date_overlap_in_month(
    mes_str: &str,
    anio: i16,
    periodo_str: &str,
    obra1_ini: Option<NaiveDate>,
    obra1_fin: Option<NaiveDate>,
    obra2_ini: Option<NaiveDate>,
    obra2_fin: Option<NaiveDate>,
) -> bool {
    let (m, y) = match parse_month_and_year(mes_str, anio, periodo_str) {
        Some((m, y)) => (m, y as i32),
        None => return false,
    };
    let (month_start, month_end, _) = match get_month_date_range(m, y) {
        Some(r) => r,
        None => return false,
    };

    let o1_has_dates = obra1_ini.is_some() || obra1_fin.is_some();
    let o2_has_dates = obra2_ini.is_some() || obra2_fin.is_some();

    if o1_has_dates && o2_has_dates {
        let s1 = obra1_ini.unwrap_or(month_start).max(month_start);
        let e1 = obra1_fin.unwrap_or(month_end).min(month_end);
        let s2 = obra2_ini.unwrap_or(month_start).max(month_start);
        let e2 = obra2_fin.unwrap_or(month_end).min(month_end);

        if s1 <= e1 && s2 <= e2 {
            let ov_s = s1.max(s2);
            let ov_e = e1.min(e2);
            if ov_s <= ov_e {
                return true; // Hay superposición de fechas
            }
        }
    }
    false
}

/// Agrega una vinculación adicional de obra para un empleado en un periodo determinado
pub fn add_obra_vinculacion_for_employee(
    client: &mut Client,
    id_parent_reporte_liq: Uuid,
    id_registro_obra: Uuid,
    dias_obra: i16,
) -> Result<ReporteLiquidacionRowDb, String> {
    if dias_obra <= 0 {
        return Err("Los días de obra deben ser mayores a 0.".to_string());
    }

    let mut tx = client.transaction().map_err(|e| format!("Error iniciando tx: {}", e))?;

    // Bitácora: foto "antes" del reporte padre (id de REPORTE)
    let lote = bitacora_reportes::iniciar_edicion(&mut tx, ACCION_VINCULAR, &[id_parent_reporte_liq])
        .map_err(|e| format!("Error iniciando bitácora de edición: {}", e))?;

    // 1. Obtener datos del padre (reportes_liquidaciones) y detalle base
    let base_info = tx.query_opt(
        r#"
        SELECT rl.id_liquidacion, rl.id_revision, rl.id_empleado, rl.mes_periodo, rl.anio_periodo,
               rdl.dias, rdl.clave, rdl.fecha,
               COALESCE(rdl.sdi, 0.0)::float8, COALESCE(rdl.lic, 0.0)::float8,
               COALESCE(rdl.inc, 0.0)::float8, COALESCE(rdl.aus, 0.0)::float8,
               e.nombre, e.rfc, e.curp, e.nss, rp.registro,
               COALESCE(dl.cf, rdl.cf, 0.0)::float8,
               COALESCE(dl.exc_pat, rdl.exc_pat, 0.0)::float8,
               COALESCE(dl.exc_obr, rdl.exc_obr, 0.0)::float8,
               COALESCE(dl.pd_pat, rdl.pd_pat, 0.0)::float8,
               COALESCE(dl.pd_obr, rdl.pd_obr, 0.0)::float8,
               COALESCE(dl.gmp_pat, rdl.gmp_pat, 0.0)::float8,
               COALESCE(dl.gmp_obr, rdl.gmp_obr, 0.0)::float8,
               COALESCE(dl.rt, rdl.rt, 0.0)::float8,
               COALESCE(dl.iv_pat, rdl.iv_pat, 0.0)::float8,
               COALESCE(dl.iv_obr, rdl.iv_obr, 0.0)::float8,
               COALESCE(dl.gps, rdl.gps, 0.0)::float8,
               COALESCE(dl.patronal, rdl.patronal, 0.0)::float8,
               COALESCE(dl.obrera, rdl.obrera, 0.0)::float8,
               COALESCE(dl.subtotal, rdl.subtotal, 0.0)::float8,
               COALESCE(TRIM(e.pensionado), '')::varchar,
               rl.mes_periodo_int
        FROM public.reportes_liquidaciones rl
        JOIN public.reportes_detalles_liquidaciones rdl ON rdl.id_reporte_liquidacion = rl.id
        JOIN public.empleados e ON e.id = rl.id_empleado
        JOIN public.registros_patronales rp ON rp.id = rl.id_registro_patronal
        LEFT JOIN public.detalles_liquidaciones dl ON dl.id_liquidacion = rl.id_liquidacion
             AND (COALESCE(TRIM(LOWER(dl.clave)), '') = COALESCE(TRIM(LOWER(rdl.clave)), ''))
             AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL))
        WHERE rl.id = $1
        ORDER BY rdl.id ASC
        LIMIT 1
        "#,
        &[&id_parent_reporte_liq],
    ).map_err(|e| format!("Error consultando información base del empleado: {}", e))?
    .ok_or_else(|| "No se encontró el registro de liquidación del empleado.".to_string())?;

    let id_liq: Uuid = base_info.get(0);
    let id_rev: Uuid = base_info.get(1);
    let id_emp: Uuid = base_info.get(2);
    let mes_periodo: String = base_info.get::<_, Option<String>>(3).unwrap_or_default(); // puede ser NULL
    let anio_periodo: i16 = base_info.get(4);
    let dias_cedula: i16 = base_info.get(5);
    let clave: Option<String> = base_info.try_get(6).ok().flatten();
    let fecha: Option<NaiveDate> = base_info.try_get(7).ok().flatten();
    let sdi: f64 = base_info.get(8);
    let lic: f64 = base_info.get(9);
    let inc: f64 = base_info.get(10);
    let aus: f64 = base_info.get(11);
    let emp_nombre: String = base_info.get::<_, Option<String>>(12).unwrap_or_default(); // puede ser NULL
    let emp_rfc: String = base_info.get::<_, Option<String>>(13).unwrap_or_default(); // puede ser NULL
    let emp_curp: String = base_info.get::<_, Option<String>>(14).unwrap_or_default(); // puede ser NULL
    let emp_nss: String = base_info.get::<_, Option<String>>(15).unwrap_or_default(); // puede ser NULL
    let reg_pat: String = base_info.get::<_, Option<String>>(16).unwrap_or_default(); // puede ser NULL

    let cf_orig: f64 = base_info.get(17);
    let exc_pat_orig: f64 = base_info.get(18);
    let exc_obr_orig: f64 = base_info.get(19);
    let pd_pat_orig: f64 = base_info.get(20);
    let pd_obr_orig: f64 = base_info.get(21);
    let gmp_pat_orig: f64 = base_info.get(22);
    let gmp_obr_orig: f64 = base_info.get(23);
    let rt_orig: f64 = base_info.get(24);
    let iv_pat_orig: f64 = base_info.get(25);
    let iv_obr_orig: f64 = base_info.get(26);
    let gps_orig: f64 = base_info.get(27);
    let patronal_orig: f64 = base_info.get(28);
    let obrera_orig: f64 = base_info.get(29);
    let subtotal_orig: f64 = base_info.get(30);
    let emp_pensionado: String = base_info.try_get(31).unwrap_or_default();
    let mes_periodo_int: i16 = base_info
        .try_get::<_, Option<i16>>(32)
        .ok()
        .flatten()
        .unwrap_or_else(|| mes_to_periodo_int(&mes_periodo));

    let periodo_str = format!("{}/{}", mes_periodo, anio_periodo);

    // 2. Obtener datos de la nueva obra
    let new_obra_row = tx.query_opt(
        "SELECT numero, tipo_obra, fecha_inicio, fecha_fin, COALESCE(creado_usuario, false) FROM public.registros_obras WHERE id = $1",
        &[&id_registro_obra],
    ).map_err(|e| format!("Error consultando nueva obra: {}", e))?
    .ok_or_else(|| "La obra seleccionada no existe.".to_string())?;

    let new_obra_numero: String = new_obra_row.get(0);
    let new_obra_tipo: Option<String> = new_obra_row.try_get(1).ok().flatten();
    let _new_obra_ini: Option<NaiveDate> = new_obra_row.try_get(2).ok().flatten();
    let _new_obra_fin: Option<NaiveDate> = new_obra_row.try_get(3).ok().flatten();
    let new_obra_creado_usuario: bool = new_obra_row.get(4);
    let new_obra_tipo_clean = new_obra_tipo.unwrap_or_default();
    let new_obra_label = if new_obra_tipo_clean.trim().is_empty() {
        new_obra_numero.trim().to_string()
    } else {
        format!("{} - {}", new_obra_numero.trim(), new_obra_tipo_clean.trim())
    };

    // 3. Consultar detalles existentes para validar suma de días y superposición
    let existing_detalles = tx.query(
        r#"
        SELECT rdl.id, rdl.id_registro_obra, COALESCE(rdl.dias_registro_obra, 0)::smallint,
               ro.numero, ro.fecha_inicio, ro.fecha_fin
        FROM public.reportes_detalles_liquidaciones rdl
        LEFT JOIN public.registros_obras ro ON ro.id = rdl.id_registro_obra
        WHERE rdl.id_reporte_liquidacion = $1
        "#,
        &[&id_parent_reporte_liq],
    ).map_err(|e| format!("Error consultando vinculaciones existentes: {}", e))?;

    let mut sum_existing_days = 0i16;
    for r in &existing_detalles {
        let existing_oid: Option<Uuid> = r.try_get(1).ok().flatten();
        let existing_dias: i16 = r.get(2);
        if let Some(oid) = existing_oid {
            if oid == id_registro_obra {
                return Err(format!("El trabajador ya está vinculado a la obra '{}' en este periodo.", new_obra_label));
            }
            if existing_dias > 0 {
                sum_existing_days += existing_dias;
            }
        }
    }

    if sum_existing_days + dias_obra > dias_cedula {
        return Err(format!(
            "La suma total de días asignados a las obras ({} existentes + {} nuevos = {}) supera los días de cédula del empleado ({}).",
            sum_existing_days,
            dias_obra,
            sum_existing_days + dias_obra,
            dias_cedula
        ));
    }

    // 4. Calcular importes prorrateados: (Valor_Original / Días_Cédula) * Días_Obra
    let dias_base = dias_cedula as f64;
    let factor = if dias_base > 0.0 {
        (dias_obra as f64 / dias_base).clamp(0.0, 10.0)
    } else {
        0.0
    };

    let round2 = |v: f64| (v * factor * 100.0).round() / 100.0;
    let cf = round2(cf_orig);
    let exc_pat = round2(exc_pat_orig);
    let exc_obr = round2(exc_obr_orig);
    let pd_pat = round2(pd_pat_orig);
    let pd_obr = round2(pd_obr_orig);
    let gmp_pat = round2(gmp_pat_orig);
    let gmp_obr = round2(gmp_obr_orig);
    let rt = round2(rt_orig);
    let iv_pat = round2(iv_pat_orig);
    let iv_obr = round2(iv_obr_orig);
    let gps = round2(gps_orig);
    let patronal = round2(patronal_orig);
    let obrera = round2(obrera_orig);
    let subtotal = round2(subtotal_orig);

    let new_detail_id = Uuid::now_v7();

    // 5. Insertar nuevo detalle
    tx.execute(
        r#"
        INSERT INTO public.reportes_detalles_liquidaciones (
            id, id_reporte_liquidacion, clave, fecha, dias, sdi, lic, inc, aus,
            cf, exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr,
            gps, patronal, obrera, subtotal, id_registro_obra, dias_registro_obra
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9,
            $10, $11, $12, $13, $14, $15, $16, $17, $18, $19,
            $20, $21, $22, $23, $24, $25
        )
        "#,
        &[
            &new_detail_id, &id_parent_reporte_liq, &clave, &fecha, &dias_cedula, &sdi, &lic, &inc, &aus,
            &cf, &exc_pat, &exc_obr, &pd_pat, &pd_obr, &gmp_pat, &gmp_obr, &rt, &iv_pat, &iv_obr,
            &gps, &patronal, &obrera, &subtotal, &Some(id_registro_obra), &Some(dias_obra),
        ],
    ).map_err(|e| format!("Error insertando nueva vinculación de detalle: {}", e))?;

    // 6. Sincronizar reportes_vinculaciones_obras_liquidaciones
    tx.execute(
        "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1",
        &[&id_parent_reporte_liq],
    ).map_err(|e| format!("Error eliminando vinculaciones en reporte: {}", e))?;
    tx.execute(
        r#"
        INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (
            id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra
        )
        SELECT uuidv7(), id_registro_obra, id_reporte_liquidacion, SUM(COALESCE(dias_registro_obra, 0))::smallint
        FROM public.reportes_detalles_liquidaciones
        WHERE id_reporte_liquidacion = $1 AND id_registro_obra IS NOT NULL
        GROUP BY id_registro_obra, id_reporte_liquidacion
        "#,
        &[&id_parent_reporte_liq],
    ).map_err(|e| format!("Error actualizando vinculaciones en reporte: {}", e))?;

    // La tabla MAESTRA vinculaciones_obras_liquidaciones refleja solo el PDF de la cédula:
    // las ediciones del analista viven únicamente en las tablas de reporte (rdl/rvol).

    // Bitácora: prorrateo canónico y foto "después" antes del commit
    bitacora_reportes::cerrar_edicion(&mut tx, lote, true)
        .map_err(|e| format!("Error cerrando bitácora de edición: {}", e))?;

    tx.commit().map_err(|e| format!("Error en commit de nueva vinculación: {}", e))?;

    // Relectura post-commit: los importes devueltos son los recalculados por la regla canónica
    let importes = client.query_one(
        r#"
        SELECT cf::float8, exc_pat::float8, exc_obr::float8, pd_pat::float8, pd_obr::float8,
               gmp_pat::float8, gmp_obr::float8, rt::float8, iv_pat::float8, iv_obr::float8,
               gps::float8, patronal::float8, obrera::float8, subtotal::float8
        FROM public.reportes_detalles_liquidaciones
        WHERE id = $1
        "#,
        &[&new_detail_id],
    ).map_err(|e| format!("Error releyendo importes recalculados: {}", e))?;
    let cf: f64 = importes.get(0);
    let exc_pat: f64 = importes.get(1);
    let exc_obr: f64 = importes.get(2);
    let pd_pat: f64 = importes.get(3);
    let pd_obr: f64 = importes.get(4);
    let gmp_pat: f64 = importes.get(5);
    let gmp_obr: f64 = importes.get(6);
    let rt: f64 = importes.get(7);
    let iv_pat: f64 = importes.get(8);
    let iv_obr: f64 = importes.get(9);
    let gps: f64 = importes.get(10);
    let patronal: f64 = importes.get(11);
    let obrera: f64 = importes.get(12);
    let subtotal: f64 = importes.get(13);

    Ok(ReporteLiquidacionRowDb {
        id_reporte_liq: new_detail_id,
        id_parent_reporte_liq,
        id_liquidacion: id_liq,
        id_revision: id_rev,
        id_empleado: id_emp,
        nombre_trabajador: emp_nombre,
        rfc: emp_rfc,
        curp: emp_curp,
        nss: emp_nss,
        pensionado: emp_pensionado,
        registro_patronal: reg_pat,
        mes_periodo,
        mes_periodo_int,
        anio_periodo,
        periodo_str,
        dias_cedula,
        id_registro_obra: Some(id_registro_obra),
        obra_numero: new_obra_numero,
        obra_label: new_obra_label,
        dias_obra,
        obra_creado_usuario: new_obra_creado_usuario,
        clave: clave.unwrap_or_default(),
        fecha,
        fecha_str: fecha.map(|f| f.format("%d/%m/%Y").to_string()).unwrap_or_default(),
        sdi,
        lic,
        inc,
        aus,
        cf,
        exc_pat,
        exc_obr,
        pd_pat,
        pd_obr,
        gmp_pat,
        gmp_obr,
        rt,
        iv_pat,
        iv_obr,
        gps,
        patronal,
        obrera,
        subtotal,
        cf_orig,
        exc_pat_orig,
        exc_obr_orig,
        pd_pat_orig,
        pd_obr_orig,
        gmp_pat_orig,
        gmp_obr_orig,
        rt_orig,
        iv_pat_orig,
        iv_obr_orig,
        gps_orig,
        patronal_orig,
        obrera_orig,
        subtotal_orig,
        tiene_inconsistencia: false,
        observacion_validacion: String::new(),
        estado: "ACTIVO".to_string(),
        cedula_actualizada: false,
        editado_usuario: false,
    })
}

/// Elimina un detalle de vinculación de obra adicional de un empleado
pub fn delete_obra_vinculacion_detail(
    client: &mut Client,
    id_reporte_liq: Uuid,
) -> Result<Uuid, String> {
    let mut tx = client.transaction().map_err(|e| format!("Error iniciando tx: {}", e))?;

    let row = tx.query_opt(
        r#"
        SELECT rdl.id_reporte_liquidacion, rl.id_liquidacion, rdl.clave
        FROM public.reportes_detalles_liquidaciones rdl
        JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
        WHERE rdl.id = $1
        "#,
        &[&id_reporte_liq],
    ).map_err(|e| format!("Error consultando fila para eliminar: {}", e))?
    .ok_or_else(|| "No se encontró el detalle de liquidación a eliminar.".to_string())?;

    let parent_id: Uuid = row.get(0);
    let id_liq: Uuid = row.get(1);
    let clave: Option<String> = row.try_get(2).ok().flatten();
    if clave.as_deref().unwrap_or("").trim().to_lowercase().contains("baja") {
        return Err("No se puede eliminar un renglón de movimiento de baja.".to_string());
    }

    let count: i64 = tx.query_one(
        "SELECT COUNT(*) FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion = $1",
        &[&parent_id],
    ).map_err(|e| format!("Error contando detalles: {}", e))?.get(0);

    let orig_count: i64 = tx.query_one(
        "SELECT COUNT(*) FROM public.detalles_liquidaciones WHERE id_liquidacion = $1",
        &[&id_liq],
    ).map_err(|e| format!("Error contando detalles originales: {}", e))?.get(0);

    if count <= orig_count {
        return Err("No se puede eliminar una fila original de la cédula. Si deseas desvincular la obra, modifícala a 'Sin Asignar'.".to_string());
    }

    // Bitácora: foto "antes" con el padre calculado ANTES de borrar el tramo
    let lote = bitacora_reportes::iniciar_edicion(&mut tx, ACCION_DESVINCULAR, &[parent_id])
        .map_err(|e| format!("Error iniciando bitácora de edición: {}", e))?;

    tx.execute(
        "DELETE FROM public.reportes_detalles_liquidaciones WHERE id = $1",
        &[&id_reporte_liq],
    ).map_err(|e| format!("Error eliminando detalle de reporte: {}", e))?;

    // Sincronizar reportes_vinculaciones_obras_liquidaciones
    tx.execute(
        "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1",
        &[&parent_id],
    ).map_err(|e| format!("Error eliminando vinculaciones en reporte: {}", e))?;
    tx.execute(
        r#"
        INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (
            id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra
        )
        SELECT uuidv7(), id_registro_obra, id_reporte_liquidacion, SUM(COALESCE(dias_registro_obra, 0))::smallint
        FROM public.reportes_detalles_liquidaciones
        WHERE id_reporte_liquidacion = $1 AND id_registro_obra IS NOT NULL
        GROUP BY id_registro_obra, id_reporte_liquidacion
        "#,
        &[&parent_id],
    ).map_err(|e| format!("Error actualizando vinculaciones en reporte: {}", e))?;

    // La tabla MAESTRA vinculaciones_obras_liquidaciones refleja solo el PDF de la cédula:
    // las ediciones del analista viven únicamente en las tablas de reporte (rdl/rvol).

    // Bitácora: foto "después" y prorrateo canónico antes del commit
    bitacora_reportes::cerrar_edicion(&mut tx, lote, true)
        .map_err(|e| format!("Error cerrando bitácora de edición: {}", e))?;

    tx.commit().map_err(|e| format!("Error en commit al eliminar detalle: {}", e))?;

    Ok(parent_id)
}

/// Candidato de obra para la asignación de días
#[derive(Debug, Clone)]
pub struct ObraAllocationCandidate {
    pub id: Uuid,
    pub numero: String,
    pub fecha_inicio: Option<NaiveDate>,
    pub fecha_fin: Option<NaiveDate>,
    pub tipo_obra: Option<String>,
    pub creado_usuario: bool,
}

/// Distribuye cronológicamente los días disponibles de un empleado entre múltiples obras seleccionadas,
/// respetando vigencias activas, movimientos de baja/alta e incidencias.
/// Otorga prioridad a las obras creadas por el usuario en la revisión (`creado_usuario = true`)
/// sobre obras genéricas de cédula (`creado_usuario = false`).
pub fn allocate_days_across_obras(
    obras: &[ObraAllocationCandidate],
    target_total_dias: i16,
    dias_cedula: i16,
    lic: f64,
    inc: f64,
    aus: f64,
    mes_str: &str,
    anio: i16,
    periodo_str: &str,
    movements: &[EmployeeMovement],
) -> Vec<(Uuid, i16)> {
    if obras.is_empty() {
        return Vec::new();
    }
    if target_total_dias <= 0 {
        return obras.iter().map(|o| (o.id, 0)).collect();
    }

    // Caso de obra única
    if obras.len() == 1 {
        let (cap, _) = calculate_max_dias_obra_for_employee_movements(
            target_total_dias,
            lic,
            inc,
            aus,
            mes_str,
            anio,
            periodo_str,
            obras[0].fecha_inicio,
            obras[0].fecha_fin,
            movements,
        );
        return vec![(obras[0].id, target_total_dias.min(cap).max(0))];
    }

    let (month, year) = match parse_month_and_year(mes_str, anio, periodo_str) {
        Some(pair) => pair,
        None => {
            let mut res = Vec::new();
            let mut rem = target_total_dias;
            for o in obras {
                let d = rem;
                rem = 0;
                res.push((o.id, d));
            }
            return res;
        }
    };

    let (month_start, month_end, _) = match get_month_date_range(month, year as i32) {
        Some(r) => r,
        None => {
            let mut res = Vec::new();
            let mut rem = target_total_dias;
            for o in obras {
                let d = rem;
                rem = 0;
                res.push((o.id, d));
            }
            return res;
        }
    };

    let active_intervals = calculate_employee_active_intervals(month_start, month_end, movements);
    if active_intervals.is_empty() {
        return obras.iter().map(|o| (o.id, 0)).collect();
    }

    let mut active_dates = Vec::new();
    let mut curr = month_start;
    while curr <= month_end {
        if active_intervals.iter().any(|(s, e)| curr >= *s && curr <= *e) {
            active_dates.push(curr);
        }
        if let Some(next) = curr.succ_opt() {
            curr = next;
        } else {
            break;
        }
    }

    if active_dates.is_empty() {
        return obras.iter().map(|o| (o.id, 0)).collect();
    }

    // Topes individuales de cada obra bajo movimientos
    let mut max_cap_map: HashMap<Uuid, i16> = HashMap::new();
    for o in obras {
        let (cap, _) = calculate_max_dias_obra_for_employee_movements(
            dias_cedula,
            lic,
            inc,
            aus,
            mes_str,
            anio,
            periodo_str,
            o.fecha_inicio,
            o.fecha_fin,
            movements,
        );
        max_cap_map.insert(o.id, cap);
    }

    let mut day_counts: HashMap<Uuid, i16> = HashMap::new();
    for o in obras {
        day_counts.insert(o.id, 0);
    }

    // Simulación día a día
    for &d in &active_dates {
        let candidates: Vec<(usize, &ObraAllocationCandidate)> = obras
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                let after_start = o.fecha_inicio.map_or(true, |ini| d >= ini);
                let before_end = o.fecha_fin.map_or(true, |fin| d <= fin);
                after_start && before_end
            })
            .collect();

        if candidates.is_empty() {
            continue;
        }

        let best = candidates.into_iter().min_by(|&(idx_a, a), &(idx_b, b)| {
            // 1. creado_usuario: true tiene mayor prioridad que false
            b.creado_usuario.cmp(&a.creado_usuario)
                // 2. Mayor fecha de inicio (asignación o transición más reciente a la obra)
                .then_with(|| {
                    let a_ini = a.fecha_inicio.unwrap_or(month_start);
                    let b_ini = b.fecha_inicio.unwrap_or(month_start);
                    b_ini.cmp(&a_ini)
                })
                // 3. Duración total más acotada (obra específica vs general)
                .then_with(|| {
                    let a_span = match (a.fecha_inicio, a.fecha_fin) {
                        (Some(s), Some(e)) => (e - s).num_days(),
                        _ => 99999,
                    };
                    let b_span = match (b.fecha_inicio, b.fecha_fin) {
                        (Some(s), Some(e)) => (e - s).num_days(),
                        _ => 99999,
                    };
                    a_span.cmp(&b_span)
                })
                // 4. Si están empatadas, preferir la que tiene menos días asignados hasta el momento (reparto equitativo)
                .then_with(|| {
                    let a_cnt = day_counts.get(&a.id).copied().unwrap_or(0);
                    let b_cnt = day_counts.get(&b.id).copied().unwrap_or(0);
                    a_cnt.cmp(&b_cnt)
                })
                // 5. Orden original
                .then_with(|| idx_a.cmp(&idx_b))
        });

        if let Some((_, best_obra)) = best {
            *day_counts.entry(best_obra.id).or_insert(0) += 1;
        }
    }

    let sum_assigned: i16 = day_counts.values().sum();

    let mut final_allocations: HashMap<Uuid, i16> = HashMap::new();
    if sum_assigned == target_total_dias {
        final_allocations = day_counts.clone();
    } else if target_total_dias < sum_assigned {
        // Escalamiento proporcional si el usuario limitó los días
        let mut cur_sum = 0i16;
        for o in obras {
            let raw = day_counts.get(&o.id).copied().unwrap_or(0);
            let scaled = if sum_assigned > 0 {
                ((raw as f64 * target_total_dias as f64) / sum_assigned as f64).round() as i16
            } else {
                0
            };
            let val = scaled.min(raw);
            final_allocations.insert(o.id, val);
            cur_sum += val;
        }
        let mut diff = target_total_dias - cur_sum;
        if diff > 0 {
            let mut sorted = obras.to_vec();
            sorted.sort_by(|a, b| {
                let cnt_a = day_counts.get(&a.id).copied().unwrap_or(0);
                let cnt_b = day_counts.get(&b.id).copied().unwrap_or(0);
                cnt_b.cmp(&cnt_a)
            });
            for o in &sorted {
                if diff == 0 { break; }
                let cur = final_allocations.get(&o.id).copied().unwrap_or(0);
                let raw = day_counts.get(&o.id).copied().unwrap_or(0);
                if cur < raw {
                    let add = (raw - cur).min(diff);
                    *final_allocations.get_mut(&o.id).unwrap() += add;
                    diff -= add;
                }
            }
        } else if diff < 0 {
            let mut to_sub = -diff;
            let mut sorted = obras.to_vec();
            sorted.sort_by(|a, b| {
                let cnt_a = final_allocations.get(&a.id).copied().unwrap_or(0);
                let cnt_b = final_allocations.get(&b.id).copied().unwrap_or(0);
                cnt_b.cmp(&cnt_a)
            });
            for o in &sorted {
                if to_sub == 0 { break; }
                let cur = final_allocations.get(&o.id).copied().unwrap_or(0);
                if cur > 0 {
                    let sub = cur.min(to_sub);
                    *final_allocations.get_mut(&o.id).unwrap() -= sub;
                    to_sub -= sub;
                }
            }
        }
    } else {
        // target_total_dias > sum_assigned
        final_allocations = day_counts.clone();
        let mut diff = target_total_dias - sum_assigned;
        let mut sorted = obras.to_vec();
        sorted.sort_by(|a, b| {
            b.creado_usuario.cmp(&a.creado_usuario)
                .then_with(|| b.fecha_inicio.cmp(&a.fecha_inicio))
        });
        for o in &sorted {
            if diff <= 0 { break; }
            let cur = final_allocations.get(&o.id).copied().unwrap_or(0);
            let cap = max_cap_map.get(&o.id).copied().unwrap_or(dias_cedula);
            if cur < cap {
                let can_add = (cap - cur).min(diff);
                *final_allocations.get_mut(&o.id).unwrap() += can_add;
                diff -= can_add;
            }
        }
    }

    // Ordenar resultados para que la obra principal (creado_usuario = true, más días asignados) quede en la fila primaria (idx == 0)
    let mut result_obras = obras.to_vec();
    result_obras.sort_by(|a, b| {
        b.creado_usuario.cmp(&a.creado_usuario)
            .then_with(|| {
                let cnt_a = final_allocations.get(&a.id).copied().unwrap_or(0);
                let cnt_b = final_allocations.get(&b.id).copied().unwrap_or(0);
                cnt_b.cmp(&cnt_a)
            })
            .then_with(|| {
                let a_ini = a.fecha_inicio.unwrap_or(month_start);
                let b_ini = b.fecha_inicio.unwrap_or(month_start);
                a_ini.cmp(&b_ini)
            })
    });

    result_obras.into_iter().map(|o| {
        let d = final_allocations.get(&o.id).copied().unwrap_or(0);
        (o.id, d)
    }).collect()
}

/// Estructura con el resultado de la asignación masiva de 1 o más obras a empleados
#[derive(Debug, Clone)]
pub struct BatchAssignMultiObrasResult {
    pub employees_updated: usize,
    pub obras_linked_count: usize,
    pub rows_created: usize,
    pub rows_updated: usize,
    pub rows_deleted: usize,
}

/// Asigna y vincula 1 o más obras a múltiples empleados en lote, calculando días y topes aceptables
pub fn batch_assign_multiple_obras_to_employees(
    client: &mut Client,
    _id_revision: Uuid,
    parent_ids: &[Uuid],
    selected_obra_ids: &[Uuid],
    custom_dias_cedula: Option<i16>,
    custom_dias_obra: Option<i16>,
    custom_worker_dias_obra: Option<&HashMap<Uuid, i16>>,
) -> Result<BatchAssignMultiObrasResult, String> {
    if parent_ids.is_empty() {
        return Ok(BatchAssignMultiObrasResult {
            employees_updated: 0,
            obras_linked_count: 0,
            rows_created: 0,
            rows_updated: 0,
            rows_deleted: 0,
        });
    }
    if selected_obra_ids.is_empty() {
        return Err("No se seleccionó ninguna obra para vincular.".to_string());
    }

    let mut tx = client.transaction().map_err(|e| format!("Error iniciando tx: {}", crate::db::schema::format_pg_error(&e)))?;

    // 1. Obtener detalles de las obras seleccionadas ordenadas cronológicamente
    let mut obra_candidates = Vec::new();
    for &oid in selected_obra_ids {
        let row_opt = tx.query_opt(
            "SELECT id, numero, fecha_inicio, fecha_fin, tipo_obra, COALESCE(creado_usuario, false) FROM public.registros_obras WHERE id = $1",
            &[&oid],
        ).map_err(|e| format!("Error consultando obra {}: {}", oid, crate::db::schema::format_pg_error(&e)))?;

        if let Some(r) = row_opt {
            let id: Uuid = match r.try_get(0) {
                Ok(uid) => uid,
                Err(_) => continue,
            };
            let numero: String = r.try_get(1).unwrap_or_default();
            let f_ini: Option<NaiveDate> = r.try_get(2).ok().flatten();
            let f_fin: Option<NaiveDate> = r.try_get(3).ok().flatten();
            let tipo: Option<String> = r.try_get(4).ok().flatten();
            let creado_usr: bool = r.try_get(5).unwrap_or(false);
            obra_candidates.push(ObraAllocationCandidate {
                id,
                numero,
                fecha_inicio: f_ini,
                fecha_fin: f_fin,
                tipo_obra: tipo,
                creado_usuario: creado_usr,
            });
        }
    }

    if obra_candidates.is_empty() {
        return Err("Ninguna de las obras seleccionadas existe en la base de datos.".to_string());
    }

    // Bitácora: foto "antes" de los reportes del lote (parent_ids son ids de REPORTE)
    let lote = bitacora_reportes::iniciar_edicion(&mut tx, ACCION_ASIGNAR_LOTE, parent_ids)
        .map_err(|e| format!("Error iniciando bitácora de edición: {}", e))?;

    let mut employees_updated = 0;
    let mut rows_created = 0;
    let mut rows_updated = 0;
    let mut rows_deleted = 0;

    let mut unique_parents: Vec<Uuid> = parent_ids.to_vec();
    unique_parents.sort();
    unique_parents.dedup();

    for parent_id in unique_parents {
        // Asegurar que todas las filas de detalles_liquidaciones existan en reportes_detalles_liquidaciones
        let _ = tx.execute(
            r#"
            INSERT INTO public.reportes_detalles_liquidaciones (
                id, id_reporte_liquidacion, dias, clave, fecha,
                sdi, lic, inc, aus, cf, exc_pat, exc_obr, pd_pat, pd_obr,
                gmp_pat, gmp_obr, rt, iv_pat, iv_obr, gps, patronal, obrera, subtotal,
                id_registro_obra, dias_registro_obra, tiene_inconsistencia, observacion_validacion,
                created_at
            )
            SELECT
                uuidv7(), rl.id, 
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0 ELSE dl.dias END,
                dl.clave, dl.fecha,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN NULL ELSE dl.sdi END,
                dl.lic, dl.inc, dl.aus,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.cf END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.exc_pat END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.exc_obr END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.pd_pat END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.pd_obr END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.gmp_pat END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.gmp_obr END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.rt END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.iv_pat END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.iv_obr END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.gps END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.patronal END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.obrera END,
                CASE WHEN LOWER(TRIM(COALESCE(dl.clave, ''))) = 'baja' THEN 0.0 ELSE dl.subtotal END,
                NULL, NULL, false, '',
                CURRENT_TIMESTAMP
            FROM public.reportes_liquidaciones rl
            JOIN public.detalles_liquidaciones dl ON dl.id_liquidacion = rl.id_liquidacion
            WHERE rl.id = $1
              AND NOT EXISTS (
                  SELECT 1 FROM public.reportes_detalles_liquidaciones rdl
                  WHERE rdl.id_reporte_liquidacion = rl.id
                    AND (
                        (rdl.clave IS NULL AND dl.clave IS NULL)
                        OR TRIM(LOWER(COALESCE(rdl.clave, ''))) = TRIM(LOWER(COALESCE(dl.clave, '')))
                    )
                    AND (
                        (rdl.fecha IS NULL AND dl.fecha IS NULL)
                        OR rdl.fecha = dl.fecha
                    )
              )
            "#,
            &[&parent_id],
        );

        // Obtener detalles base del empleado
        let base_rows = tx.query(
            r#"
            SELECT rdl.id, rdl.id_registro_obra,
                   COALESCE(rdl.dias, dl.dias, 0)::smallint,
                   COALESCE(rdl.dias_registro_obra, 0)::smallint,
                   rdl.clave, rdl.fecha,
                   COALESCE(rdl.sdi, 0.0)::float8, COALESCE(rdl.lic, 0.0)::float8,
                   COALESCE(rdl.inc, 0.0)::float8, COALESCE(rdl.aus, 0.0)::float8,
                   rl.id_liquidacion, rl.id_empleado, rl.mes_periodo, COALESCE(rl.anio_periodo, 0)::smallint,
                   COALESCE(dl.cf, rdl.cf, 0.0)::float8,
                   COALESCE(dl.exc_pat, rdl.exc_pat, 0.0)::float8,
                   COALESCE(dl.exc_obr, rdl.exc_obr, 0.0)::float8,
                   COALESCE(dl.pd_pat, rdl.pd_pat, 0.0)::float8,
                   COALESCE(dl.pd_obr, rdl.pd_obr, 0.0)::float8,
                   COALESCE(dl.gmp_pat, rdl.gmp_pat, 0.0)::float8,
                   COALESCE(dl.gmp_obr, rdl.gmp_obr, 0.0)::float8,
                   COALESCE(dl.rt, rdl.rt, 0.0)::float8,
                   COALESCE(dl.iv_pat, rdl.iv_pat, 0.0)::float8,
                   COALESCE(dl.iv_obr, rdl.iv_obr, 0.0)::float8,
                   COALESCE(dl.gps, rdl.gps, 0.0)::float8,
                   COALESCE(dl.patronal, rdl.patronal, 0.0)::float8,
                   COALESCE(dl.obrera, rdl.obrera, 0.0)::float8,
                   COALESCE(dl.subtotal, rdl.subtotal, 0.0)::float8
            FROM public.reportes_detalles_liquidaciones rdl
            JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
            LEFT JOIN public.detalles_liquidaciones dl ON dl.id_liquidacion = rl.id_liquidacion
                 AND (COALESCE(TRIM(LOWER(dl.clave)), '') = COALESCE(TRIM(LOWER(rdl.clave)), ''))
                 AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL))
            WHERE rdl.id_reporte_liquidacion = $1
            ORDER BY rdl.id ASC
            "#,
            &[&parent_id],
        ).map_err(|e| format!("Error consultando registros del empleado: {}", crate::db::schema::format_pg_error(&e)))?;

        if base_rows.is_empty() {
            continue;
        }

        // Separar registros de baja (movimientos de auditoría sin días laborales) de los registros de trabajo
        let is_row_baja = |r: &postgres::Row| -> bool {
            let clv: Option<String> = r.try_get(4).ok().flatten();
            let d: i16 = row_get_i16(r, 2, 0);
            let clv_low = clv.as_deref().unwrap_or("").trim().to_lowercase();
            clv_low.contains("baja") || clv_low == "b" || (d == 0 && !clv_low.is_empty())
        };

        let mut work_rows: Vec<&postgres::Row> = Vec::new();
        let mut baja_rows: Vec<&postgres::Row> = Vec::new();
        for r in &base_rows {
            if is_row_baja(r) {
                baja_rows.push(r);
            } else {
                work_rows.push(r);
            }
        }

        // Si no hay filas de trabajo (ej. solo hay renglón de baja con 0 días), no asignar obra a este empleado
        if work_rows.is_empty() {
            continue;
        }

        let primary_work_row = work_rows[0];
        let primary_rdl_id: Uuid = match primary_work_row.try_get(0) {
            Ok(id) => id,
            Err(_) => continue,
        };
        let base_dias_cedula: i16 = row_get_i16(primary_work_row, 2, 30);
        let clave: Option<String> = primary_work_row.try_get(4).ok().flatten();
        let fecha: Option<NaiveDate> = primary_work_row.try_get(5).ok().flatten();
        let sdi: f64 = row_get_f64(primary_work_row, 6, 0.0);
        let lic: f64 = row_get_f64(primary_work_row, 7, 0.0);
        let inc: f64 = row_get_f64(primary_work_row, 8, 0.0);
        let aus: f64 = row_get_f64(primary_work_row, 9, 0.0);
        let id_liq: Uuid = primary_work_row.try_get(10).unwrap_or_default();
        let _id_emp: Uuid = primary_work_row.try_get(11).unwrap_or_default();
        let mes_periodo: String = primary_work_row.try_get::<_, Option<String>>(12).ok().flatten().unwrap_or_default();
        let anio_periodo: i16 = row_get_i16(primary_work_row, 13, 2024);
        let cf_orig: f64 = row_get_f64(primary_work_row, 14, 0.0);
        let exc_pat_orig: f64 = row_get_f64(primary_work_row, 15, 0.0);
        let exc_obr_orig: f64 = row_get_f64(primary_work_row, 16, 0.0);
        let pd_pat_orig: f64 = row_get_f64(primary_work_row, 17, 0.0);
        let pd_obr_orig: f64 = row_get_f64(primary_work_row, 18, 0.0);
        let gmp_pat_orig: f64 = row_get_f64(primary_work_row, 19, 0.0);
        let gmp_obr_orig: f64 = row_get_f64(primary_work_row, 20, 0.0);
        let rt_orig: f64 = row_get_f64(primary_work_row, 21, 0.0);
        let iv_pat_orig: f64 = row_get_f64(primary_work_row, 22, 0.0);
        let iv_obr_orig: f64 = row_get_f64(primary_work_row, 23, 0.0);
        let gps_orig: f64 = row_get_f64(primary_work_row, 24, 0.0);
        let patronal_orig: f64 = row_get_f64(primary_work_row, 25, 0.0);
        let obrera_orig: f64 = row_get_f64(primary_work_row, 26, 0.0);
        let subtotal_orig: f64 = row_get_f64(primary_work_row, 27, 0.0);

        let is_baja = false; // work_rows contiene exclusivamente filas laborales activas

        let final_dias_cedula = custom_dias_cedula.unwrap_or(base_dias_cedula).max(0);
        let dias_efectivos = (final_dias_cedula as f64 - lic - inc - aus).round().max(0.0) as i16;

        let worker_inline_dias = custom_worker_dias_obra.and_then(|m| m.get(&parent_id).or_else(|| m.get(&primary_rdl_id))).copied();

        let target_total_dias = if let Some(global_obr) = custom_dias_obra {
            global_obr.min(dias_efectivos).max(0)
        } else if let Some(inline_obr) = worker_inline_dias {
            if inline_obr > 0 {
                inline_obr.min(dias_efectivos).max(0)
            } else {
                dias_efectivos
            }
        } else {
            dias_efectivos
        };

        let periodo_str = format!("{}/{}", mes_periodo, anio_periodo);

        let mut movs = Vec::new();
        // Incluir movimientos de bajas de este trabajador en este periodo
        for br in &baja_rows {
            if let Ok(Some(fb)) = br.try_get::<_, Option<NaiveDate>>(5) {
                movs.push(EmployeeMovement { kind: MovementKind::Baja, fecha: fb });
            }
        }
        // Incluir movimientos de altas / reingresos
        for wr in &work_rows {
            if let Ok(Some(cl)) = wr.try_get::<_, Option<String>>(4) {
                let cl_low = cl.trim().to_lowercase();
                if cl_low == "alta" || cl_low == "reing" || cl_low == "reingreso" {
                    if let Ok(Some(fa)) = wr.try_get::<_, Option<NaiveDate>>(5) {
                        movs.push(EmployeeMovement { kind: MovementKind::Alta, fecha: fa });
                    }
                }
            }
        }
        // Consultar movimientos en detalles_liquidaciones por seguridad
        let addl_movs = tx.query(
            "SELECT clave, fecha FROM public.detalles_liquidaciones WHERE id_liquidacion = $1 AND fecha IS NOT NULL",
            &[&id_liq],
        ).unwrap_or_default();
        for mr in addl_movs {
            let cl: String = mr.try_get::<_, Option<String>>(0).ok().flatten().unwrap_or_default().trim().to_lowercase();
            if let Ok(Some(f)) = mr.try_get::<_, Option<NaiveDate>>(1) {
                if cl.contains("baja") || cl == "b" {
                    movs.push(EmployeeMovement { kind: MovementKind::Baja, fecha: f });
                } else if cl.contains("alta") || cl.contains("reing") {
                    movs.push(EmployeeMovement { kind: MovementKind::Alta, fecha: f });
                }
            }
        }
        movs.sort_by(|a, b| a.fecha.cmp(&b.fecha).then_with(|| a.kind.cmp(&b.kind)));
        movs.dedup();

        // 2. Calcular los días aceptables para cada obra seleccionada usando asignación día a día y prioridad
        let obra_allocations = allocate_days_across_obras(
            &obra_candidates,
            target_total_dias,
            final_dias_cedula,
            lic,
            inc,
            aus,
            &mes_periodo,
            anio_periodo,
            &periodo_str,
            &movs,
        );

        // 3. Aplicar las asignaciones a la BD
        // secondary_existing_obras excluye estrictamente primary_rdl_id para que las filas secundarias (idx > 0)
        // no sobreescriban la fila primaria.
        let secondary_existing_obras: HashMap<Option<Uuid>, Uuid> = work_rows.iter().filter_map(|r| {
            let rid: Uuid = r.try_get(0).ok()?;
            if rid == primary_rdl_id {
                return None;
            }
            let roid: Option<Uuid> = r.try_get(1).ok().flatten();
            Some((roid, rid))
        }).collect();

        let target_selected_set: HashSet<Uuid> = obra_allocations.iter().map(|(oid, _)| *oid).collect();

        // Eliminar ÚNICAMENTE filas adicionales de split creadas para obras que ya no están seleccionadas.
        // NUNCA eliminar primary_rdl_id, NUNCA eliminar filas de baja, NUNCA eliminar filas sin obra (roid = None).
        for r in &work_rows {
            let rid: Uuid = match r.try_get(0) {
                Ok(id) => id,
                Err(_) => continue,
            };
            if rid == primary_rdl_id {
                continue;
            }
            let roid: Option<Uuid> = r.try_get(1).ok().flatten();
            let should_delete = match roid {
                Some(oid) => !target_selected_set.contains(&oid),
                None => false,
            };
            if should_delete {
                tx.execute(
                    "DELETE FROM public.reportes_detalles_liquidaciones WHERE id = $1",
                    &[&rid],
                ).map_err(|e| format!("Error eliminando detalle antiguo: {}", crate::db::schema::format_pg_error(&e)))?;

                if let Some(oid) = roid {
                    tx.execute(
                        "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1 AND id_registro_obra = $2",
                        &[&parent_id, &oid],
                    ).map_err(|e| format!("Error eliminando vinculación reporte: {}", crate::db::schema::format_pg_error(&e)))?;
                }
                rows_deleted += 1;
            }
        }

        let calc_prorrateo = |val_orig: f64, d_obr: i16, d_ced: i16| -> f64 {
            if is_baja || d_ced == 0 || d_obr == 0 {
                0.0
            } else {
                let f = (d_obr as f64 / d_ced as f64).clamp(0.0, 10.0);
                (val_orig * f * 100.0).round() / 100.0
            }
        };

        // Procesar asignaciones
        for (idx, (oid, d_obr)) in obra_allocations.iter().enumerate() {
            let cf_p = calc_prorrateo(cf_orig, *d_obr, final_dias_cedula);
            let exc_pat_p = calc_prorrateo(exc_pat_orig, *d_obr, final_dias_cedula);
            let exc_obr_p = calc_prorrateo(exc_obr_orig, *d_obr, final_dias_cedula);
            let pd_pat_p = calc_prorrateo(pd_pat_orig, *d_obr, final_dias_cedula);
            let pd_obr_p = calc_prorrateo(pd_obr_orig, *d_obr, final_dias_cedula);
            let gmp_pat_p = calc_prorrateo(gmp_pat_orig, *d_obr, final_dias_cedula);
            let gmp_obr_p = calc_prorrateo(gmp_obr_orig, *d_obr, final_dias_cedula);
            let rt_p = calc_prorrateo(rt_orig, *d_obr, final_dias_cedula);
            let iv_pat_p = calc_prorrateo(iv_pat_orig, *d_obr, final_dias_cedula);
            let iv_obr_p = calc_prorrateo(iv_obr_orig, *d_obr, final_dias_cedula);
            let gps_p = calc_prorrateo(gps_orig, *d_obr, final_dias_cedula);
            let patronal_p = calc_prorrateo(patronal_orig, *d_obr, final_dias_cedula);
            let obrera_p = calc_prorrateo(obrera_orig, *d_obr, final_dias_cedula);
            let subtotal_p = calc_prorrateo(subtotal_orig, *d_obr, final_dias_cedula);

            if idx == 0 {
                tx.execute(
                    r#"
                    UPDATE public.reportes_detalles_liquidaciones
                    SET dias = $1, dias_registro_obra = $2, id_registro_obra = $3,
                        cf = $4, exc_pat = $5, exc_obr = $6, pd_pat = $7, pd_obr = $8,
                        gmp_pat = $9, gmp_obr = $10, rt = $11, iv_pat = $12, iv_obr = $13,
                        gps = $14, patronal = $15, obrera = $16, subtotal = $17,
                        tiene_inconsistencia = false, observacion_validacion = ''
                    WHERE id = $18
                    "#,
                    &[
                        &final_dias_cedula, d_obr, &oid,
                        &cf_p, &exc_pat_p, &exc_obr_p, &pd_pat_p, &pd_obr_p,
                        &gmp_pat_p, &gmp_obr_p, &rt_p, &iv_pat_p, &iv_obr_p,
                        &gps_p, &patronal_p, &obrera_p, &subtotal_p,
                        &primary_rdl_id,
                    ],
                ).map_err(|e| format!("Error actualizando fila primaria: {}", crate::db::schema::format_pg_error(&e)))?;
                rows_updated += 1;
            } else {
                if let Some(&existing_rid) = secondary_existing_obras.get(&Some(*oid)) {
                    tx.execute(
                        r#"
                        UPDATE public.reportes_detalles_liquidaciones
                        SET dias = $1, dias_registro_obra = $2, id_registro_obra = $3,
                            cf = $4, exc_pat = $5, exc_obr = $6, pd_pat = $7, pd_obr = $8,
                            gmp_pat = $9, gmp_obr = $10, rt = $11, iv_pat = $12, iv_obr = $13,
                            gps = $14, patronal = $15, obrera = $16, subtotal = $17,
                            tiene_inconsistencia = false, observacion_validacion = ''
                        WHERE id = $18
                        "#,
                        &[
                            &final_dias_cedula, d_obr, &oid,
                            &cf_p, &exc_pat_p, &exc_obr_p, &pd_pat_p, &pd_obr_p,
                            &gmp_pat_p, &gmp_obr_p, &rt_p, &iv_pat_p, &iv_obr_p,
                            &gps_p, &patronal_p, &obrera_p, &subtotal_p,
                            &existing_rid,
                        ],
                    ).map_err(|e| format!("Error actualizando fila secundaria existente: {}", crate::db::schema::format_pg_error(&e)))?;
                    rows_updated += 1;
                } else {
                    let new_rid = Uuid::now_v7();
                    tx.execute(
                        r#"
                        INSERT INTO public.reportes_detalles_liquidaciones (
                            id, id_reporte_liquidacion, dias, dias_registro_obra, id_registro_obra,
                            clave, fecha, sdi, lic, inc, aus,
                            cf, exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr,
                            gps, patronal, obrera, subtotal,
                            tiene_inconsistencia, observacion_validacion,
                            created_at
                        )
                        VALUES (
                            $1, $2, $3, $4, $5,
                            $6, $7, $8, $9, $10, $11,
                            $12, $13, $14, $15, $16, $17, $18, $19, $20, $21,
                            $22, $23, $24, $25,
                            false, '',
                            CURRENT_TIMESTAMP
                        )
                        "#,
                        &[
                            &new_rid, &parent_id, &final_dias_cedula, d_obr, &oid,
                            &clave, &fecha, &sdi, &lic, &inc, &aus,
                            &cf_p, &exc_pat_p, &exc_obr_p, &pd_pat_p, &pd_obr_p,
                            &gmp_pat_p, &gmp_obr_p, &rt_p, &iv_pat_p, &iv_obr_p,
                            &gps_p, &patronal_p, &obrera_p, &subtotal_p,
                        ],
                    ).map_err(|e| format!("Error insertando fila secundaria: {}", crate::db::schema::format_pg_error(&e)))?;
                    rows_created += 1;
                }
            }
        }

        // Sincronizar días en public.detalles_liquidaciones si se especificó custom_dias_cedula
        if let Some(d_ced) = custom_dias_cedula {
            let _ = tx.execute(
                r#"
                UPDATE public.detalles_liquidaciones
                SET dias = $2
                WHERE id_liquidacion = $1
                  AND (
                      (COALESCE(TRIM(LOWER(clave)), '') = COALESCE(TRIM(LOWER($3)), '')
                       AND (fecha = $4 OR (fecha IS NULL AND $4 IS NULL)))
                      OR (
                          (SELECT COUNT(*) FROM public.detalles_liquidaciones sub_dl WHERE sub_dl.id_liquidacion = $1) = 1
                      )
                  )
                "#,
                &[&id_liq, &d_ced, &clave, &fecha],
            );
        }

        // Re-evaluar triggers de auditoría e inconsistencias de forma atómica para todas las filas del empleado
        let _ = tx.execute(
            "UPDATE public.reportes_detalles_liquidaciones SET id = id WHERE id_reporte_liquidacion = $1",
            &[&parent_id],
        );

        // Sincronizar reportes_vinculaciones_obras_liquidaciones
        tx.execute(
            "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1",
            &[&parent_id],
        ).map_err(|e| format!("Error eliminando vinculaciones en reporte: {}", crate::db::schema::format_pg_error(&e)))?;
        tx.execute(
            r#"
            INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (
                id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra
            )
            SELECT uuidv7(), id_registro_obra, id_reporte_liquidacion, SUM(COALESCE(dias_registro_obra, 0))::smallint
            FROM public.reportes_detalles_liquidaciones
            WHERE id_reporte_liquidacion = $1 AND id_registro_obra IS NOT NULL
            GROUP BY id_registro_obra, id_reporte_liquidacion
            "#,
            &[&parent_id],
        ).map_err(|e| format!("Error actualizando vinculaciones en reporte: {}", crate::db::schema::format_pg_error(&e)))?;

        // La tabla MAESTRA vinculaciones_obras_liquidaciones refleja solo el PDF de la cédula:
        // las ediciones del analista viven únicamente en las tablas de reporte (rdl/rvol).

        employees_updated += 1;
    }

    // Bitácora: foto "después" y prorrateo canónico antes del commit
    bitacora_reportes::cerrar_edicion(&mut tx, lote, true)
        .map_err(|e| format!("Error cerrando bitácora de edición: {}", e))?;

    tx.commit().map_err(|e| format!("Error en commit de asignación masiva de obras: {}", crate::db::schema::format_pg_error(&e)))?;

    Ok(BatchAssignMultiObrasResult {
        employees_updated,
        obras_linked_count: selected_obra_ids.len(),
        rows_created,
        rows_updated,
        rows_deleted,
    })
}

fn clean_rp_text(s: &str) -> &str {
    if s.trim().is_empty()
        || s.to_lowercase().contains("sin asignar")
        || s.to_lowercase().contains("pendiente")
    {
        ""
    } else {
        let trimmed = s.trim();
        if let Some((code, _)) = trimmed.split_once(" - ") {
            code.trim()
        } else if let Some((code, _)) = trimmed.split_once(' ') {
            code.trim()
        } else {
            trimmed
        }
    }
}

fn clean_obra_text<'a>(obra_num: &'a str, obra_label: &'a str, has_obra: bool) -> &'a str {
    if !has_obra
        || obra_label.to_lowercase().contains("sin asignar")
        || obra_label.to_lowercase().contains("pendiente")
    {
        ""
    } else if !obra_num.trim().is_empty() {
        obra_num.trim()
    } else if let Some((num, _)) = obra_label.split_once(" - ") {
        num.trim()
    } else {
        obra_label.trim()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReporteFinalHeaderMeta {
    pub delegacion: String,
    pub subdelegacion: String,
    pub cliente_nombre: String,
    pub rp_registro: String,
    pub rfc: String,
    pub periodo_obra: String,
    pub representante_legal: String,
    pub revision_folio: String,
}

/// Copia del encabezado con cada campo obligatorio vacío reemplazado por "[FALTA: <campo>]".
/// Solo para borradores: deja visible qué falta capturar en lugar de inventar un valor.
pub fn meta_con_marcadores_faltantes(meta: &ReporteFinalHeaderMeta) -> ReporteFinalHeaderMeta {
    let marca = |valor: &str, etiqueta: &str| -> String {
        if valor.trim().is_empty() { format!("[FALTA: {etiqueta}]") } else { valor.to_string() }
    };
    ReporteFinalHeaderMeta {
        delegacion: marca(&meta.delegacion, "Delegación IMSS"),
        subdelegacion: marca(&meta.subdelegacion, "Subdelegación IMSS"),
        cliente_nombre: marca(&meta.cliente_nombre, "Razón social"),
        rp_registro: marca(&meta.rp_registro, "Registro patronal"),
        rfc: marca(&meta.rfc, "RFC"),
        periodo_obra: marca(&meta.periodo_obra, "Periodo"),
        representante_legal: marca(&meta.representante_legal, "Representante legal"),
        revision_folio: marca(&meta.revision_folio, "Folio"),
    }
}

/// Modo de exportación del Excel IMSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModoEntregable {
    /// Excel de trabajo: nunca se bloquea. `solo_vista_filtrada` indica si solo
    /// contiene las filas de la vista filtrada (marcado en el encabezado).
    Borrador { solo_vista_filtrada: bool },
    Entregable { version: i32 },
}

/// Escapa un texto para encabezado/pie de Excel (cada `&` literal se duplica)
/// y lo recorta para que su salida no supere `max_chars` caracteres.
fn escapar_texto_impresion(texto: &str, max_chars: usize) -> String {
    let mut salida = String::new();
    let mut usados = 0usize;
    for c in texto.chars() {
        let costo = if c == '&' { 2 } else { 1 };
        if usados + costo > max_chars {
            break;
        }
        if c == '&' {
            salida.push_str("&&");
        } else {
            salida.push(c);
        }
        usados += costo;
    }
    salida
}

/// Configuración de impresión oficial: carta horizontal, 1 página de ancho,
/// fila de encabezado de tabla repetida e inmovilizada, márgenes estándar.
fn configurar_impresion(
    ws: &mut rust_xlsxwriter::Worksheet,
    fila_encabezado_tabla: u32,
    ultima_columna: u16,
) -> Result<(), String> {
    // Límite de columnas de Excel (XFD = 16383)
    if ultima_columna > 16_383 {
        return Err(format!("Columna final fuera de rango para impresión: {}", ultima_columna));
    }
    ws.set_landscape();
    ws.set_paper_size(1);
    ws.set_print_fit_to_pages(1, 0);
    ws.set_repeat_rows(fila_encabezado_tabla, fila_encabezado_tabla).map_err(|e| e.to_string())?;
    ws.set_freeze_panes(fila_encabezado_tabla + 1, 0).map_err(|e| e.to_string())?;
    ws.set_margins(0.4, 0.4, 0.75, 0.75, 0.3, 0.3);
    Ok(())
}

/// Escribe un total como fórmula `SUMIF` sobre las filas de subtotal por mes
/// (identificadas por el prefijo "TOTAL" en `clave_col`), con su valor en caché.
/// Ignorar las filas de detalle evita el doble conteo. Sin filas, escribe el número.
#[allow(clippy::too_many_arguments)]
fn escribir_total_sumif(
    ws: &mut rust_xlsxwriter::Worksheet,
    fila_total: u32,
    col: u16,
    primera_fila: u32,
    ultima_fila: u32,
    clave_col: u16,
    valor: f64,
    formato: &rust_xlsxwriter::Format,
) -> Result<(), String> {
    if ultima_fila < primera_fila {
        ws.write_number_with_format(fila_total, col, valor, formato).map_err(|e| e.to_string())?;
        return Ok(());
    }
    // Referencias A1 (base 1) del rango de filas
    let r0 = primera_fila + 1;
    let r1 = ultima_fila + 1;
    let letra = rust_xlsxwriter::utility::column_number_to_name(col);
    let letra_clave = rust_xlsxwriter::utility::column_number_to_name(clave_col);
    let formula = format!(
        "SUMIF(${letra_clave}${r0}:${letra_clave}${r1},\"TOTAL*\",{letra}{r0}:{letra}{r1})"
    );
    ws.write_formula_with_format(
        fila_total,
        col,
        rust_xlsxwriter::Formula::new(formula).set_result(valor.to_string()),
        formato,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Devuelve la lista de campos obligatorios vacíos del encabezado oficial (vacía = válido).
pub fn missing_header_fields(meta: &ReporteFinalHeaderMeta) -> Vec<&'static str> {
    let campos: [(&str, &'static str); 8] = [
        (meta.cliente_nombre.as_str(), "Razón social del cliente"),
        (meta.rp_registro.as_str(), "Registro patronal"),
        (meta.rfc.as_str(), "RFC del cliente"),
        (meta.representante_legal.as_str(), "Representante legal"),
        (meta.revision_folio.as_str(), "Folio de la revisión"),
        (meta.periodo_obra.as_str(), "Periodo de obra/revisión"),
        (meta.delegacion.as_str(), "Delegación IMSS"),
        (meta.subdelegacion.as_str(), "Subdelegación IMSS"),
    ];
    campos
        .iter()
        .filter(|(valor, _)| valor.trim().is_empty())
        .map(|(_, etiqueta)| *etiqueta)
        .collect()
}

fn format_nss_styled(nss: &str) -> String {
    let digits: String = nss.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 11 {
        format!("{}-{}-{}-{}-{}", &digits[0..2], &digits[2..4], &digits[4..6], &digits[6..10], &digits[10..11])
    } else {
        nss.to_string()
    }
}

fn format_mes_col(mes_str: &str, anio: i16, periodo_str: &str) -> String {
    if let Some((m, y)) = parse_month_and_year(mes_str, anio, periodo_str) {
        let short_m = match m {
            1 => "ene", 2 => "feb", 3 => "mar", 4 => "abr",
            5 => "may", 6 => "jun", 7 => "jul", 8 => "ago",
            9 => "sep", 10 => "oct", 11 => "nov", 12 => "dic",
            _ => "mes",
        };
        format!("{}-{:02}", short_m, y % 100)
    } else {
        mes_str.to_string()
    }
}

fn format_month_total_label(periodo_key: &str, sample_row: Option<&ReporteLiquidacionRowDb>) -> (String, String) {
    if let Some(r) = sample_row {
        if let Some((m, y)) = parse_month_and_year(&r.mes_periodo, r.anio_periodo, &r.periodo_str) {
            let full_m = match m {
                1 => "ENERO", 2 => "FEBRERO", 3 => "MARZO", 4 => "ABRIL",
                5 => "MAYO", 6 => "JUNIO", 7 => "JULIO", 8 => "AGOSTO",
                9 => "SEPTIEMBRE", 10 => "OCTUBRE", 11 => "NOVIEMBRE", 12 => "DICIEMBRE",
                _ => "PERIODO",
            };
            let short_m = match m {
                1 => "ene", 2 => "feb", 3 => "mar", 4 => "abr",
                5 => "may", 6 => "jun", 7 => "jul", 8 => "ago",
                9 => "sep", 10 => "oct", 11 => "nov", 12 => "dic",
                _ => "mes",
            };
            return (format!("TOTAL {} {} zzz", full_m, y), format!("{}-{:02}", short_m, y % 100));
        }
    }
    (format!("TOTAL {} zzz", periodo_key), periodo_key.to_string())
}

/// Genera la exportación en Excel (.xlsx) con el formato profesional de la vista
pub fn export_reporte_liquidaciones_to_excel(
    rows: &[ReporteLiquidacionRowDb],
    all_rows: &[ReporteLiquidacionRowDb],
    valid_user_obra_ids: Option<&HashSet<Uuid>>,
    guarderias_kpis: &[ReporteGuarderiaKpiDb],
    file_path: &Path,
    cliente_nombre: &str,
    rp_registro: &str,
    revision_folio: &str,
    header_meta: &ReporteFinalHeaderMeta,
    modo: ModoEntregable,
) -> Result<(), String> {
    use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, ProtectionOptions, Workbook};

    // Ningún dato fijo: el encabezado oficial exige todos los campos capturados
    // Entregable: el encabezado oficial debe estar completo. Borrador: se permite con marcadores
    // explícitos "[FALTA: ...]" (no son datos inventados y el documento lleva la marca BORRADOR).
    let faltantes = missing_header_fields(header_meta);
    let meta_efectivo: ReporteFinalHeaderMeta = if faltantes.is_empty() {
        header_meta.clone()
    } else if matches!(modo, ModoEntregable::Entregable { .. }) {
        return Err(format!(
            "No se puede generar el entregable: faltan datos obligatorios: {}",
            faltantes.join(", ")
        ));
    } else {
        meta_con_marcadores_faltantes(header_meta)
    };
    let header_meta = &meta_efectivo;

    let mut workbook = Workbook::new();

    let delegacion = header_meta.delegacion.trim();
    let subdelegacion = header_meta.subdelegacion.trim();
    let emp_nombre = header_meta.cliente_nombre.trim();
    let rp_reg = header_meta.rp_registro.trim();
    let rfc_val = header_meta.rfc.trim();
    let per_obra = header_meta.periodo_obra.trim();
    let rep_legal = header_meta.representante_legal.trim();
    let fol_rev = header_meta.revision_folio.trim();

    // Configuración oficial de encabezados y pies de página para impresión en Excel:
    // Superior izquierdo: Razón Social
    // Superior derecho: Folio de la Revisión
    // Inferior izquierdo: Representante de la empresa
    // Inferior derecho: Número de páginas (Página &P de &N)
    let clean_rz = escapar_texto_impresion(emp_nombre, 80);
    let clean_fol = escapar_texto_impresion(fol_rev, 40);
    let clean_rep = escapar_texto_impresion(rep_legal, 80);

    // Marca central: borrador visible o versión del entregable (límite de Excel: 255 caracteres)
    let centro_encabezado = match modo {
        ModoEntregable::Borrador { solo_vista_filtrada } => {
            let mut texto = "&\"-,Bold\"&KFF0000BORRADOR — NO VÁLIDO PARA ENTREGA".to_string();
            if solo_vista_filtrada {
                texto.push_str(" — SOLO VISTA FILTRADA");
            }
            texto
        }
        ModoEntregable::Entregable { version } => format!("ENTREGABLE IMSS v{}", version),
    };

    let print_header_str = format!("&L{}&C{}&R{}", clean_rz, centro_encabezado, clean_fol);
    let print_footer_str = format!("&L{}&RPágina &P de &N", clean_rep);

    // =========================================================================
    // SHEET 0: Reporte Final (Estilos oficiales aplicados a datos exportados)
    // =========================================================================
    {
        let ws_final = workbook.add_worksheet();
        ws_final.set_name("Reporte Final").map_err(|e| e.to_string())?;
        ws_final.set_header(&print_header_str);
        ws_final.set_footer(&print_footer_str);
        ws_final.set_header_footer_align_with_page(true);

        // Anchos de columna exactos para los 19 campos
        let col_widths: [u16; 19] = [
            18, // 0: nss
            32, // 1: nombre
            10, // 2: mes
             8, // 3: dias
            13, // 4: sdi
            13, // 5: cf
            11, // 6: exc pat
            11, // 7: exc obr
            12, // 8: pd pat
            12, // 9: pd obr
            12, // 10: gmp pat
            12, // 11: gmp obr
            13, // 12: rt
            12, // 13: iv pat
            12, // 14: iv obr
            12, // 15: gps
            14, // 16: patronal
            13, // 17: obrera
            15, // 18: subtotal
        ];

        for (col_idx, width) in col_widths.iter().enumerate() {
            ws_final.set_column_width(col_idx as u16, *width).map_err(|e| e.to_string())?;
        }

        // Formatos de "Reporte Final"
        let top_title_fmt = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_align(FormatAlign::Center)
            .set_font_color(Color::RGB(0x000000));

        let meta_lbl_fmt = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_align(FormatAlign::Right)
            .set_font_color(Color::RGB(0x000000));

        let meta_val_fmt = Format::new()
            .set_font_size(10)
            .set_align(FormatAlign::Left)
            .set_font_color(Color::RGB(0x000000));

        let table_hdr_fmt = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_background_color(Color::RGB(0x000000))
            .set_font_color(Color::RGB(0xFFFFFF))
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0x404040));

        let row_txt_center = Format::new()
            .set_font_size(10)
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0xD1D5DB));

        let row_txt_left = Format::new()
            .set_font_size(10)
            .set_align(FormatAlign::Left)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0xD1D5DB));

        let row_int_center = Format::new()
            .set_font_size(10)
            .set_num_format("#,##0")
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0xD1D5DB));

        let row_curr_right = Format::new()
            .set_font_size(10)
            .set_num_format("$#,##0.00;($#,##0.00);\"$ -\"")
            .set_align(FormatAlign::Right)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0xD1D5DB));

        let month_tot_lbl = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_align(FormatAlign::Left)
            .set_align(FormatAlign::VerticalCenter)
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Thin);

        let month_tot_center = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter)
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Thin);

        let month_tot_int = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_num_format("#,##0")
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter)
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Thin);

        let month_tot_curr = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_num_format("$#,##0.00;($#,##0.00);\"$ -\"")
            .set_align(FormatAlign::Right)
            .set_align(FormatAlign::VerticalCenter)
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Thin);

        let grand_tot_lbl = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_background_color(Color::RGB(0x000000))
            .set_font_color(Color::RGB(0xFFFFFF))
            .set_align(FormatAlign::Left)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0x404040));

        let grand_tot_int = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_background_color(Color::RGB(0x000000))
            .set_font_color(Color::RGB(0xFFFFFF))
            .set_num_format("#,##0")
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0x404040));

        let grand_tot_curr = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_background_color(Color::RGB(0x000000))
            .set_font_color(Color::RGB(0xFFFFFF))
            .set_num_format("$#,##0.00;($#,##0.00);\"$ -\"")
            .set_align(FormatAlign::Right)
            .set_align(FormatAlign::VerticalCenter)
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0x404040));

        let sig_top_border = Format::new()
            .set_border_top(FormatBorder::Thin)
            .set_border_top_color(Color::RGB(0x000000));

        let sig_text_fmt = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter);

        let kpi_pct_fmt = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter);

        let kpi_yellow_box = Format::new()
            .set_bold()
            .set_font_size(11)
            .set_background_color(Color::RGB(0xFFFF00))
            .set_font_color(Color::RGB(0x000000))
            .set_border(FormatBorder::Thin)
            .set_border_color(Color::RGB(0x000000))
            .set_num_format("$#,##0.00;($#,##0.00);\"$ -\"")
            .set_align(FormatAlign::Center)
            .set_align(FormatAlign::VerticalCenter);

        // Encabezado Superior (Filas 0 a 3)
        ws_final.merge_range(0, 0, 0, 18, "INSTITUTO MEXICANO DEL SEGURO SOCIAL", &top_title_fmt).map_err(|e| e.to_string())?;
        ws_final.merge_range(1, 0, 1, 18, &format!("ÓRGANO DE OPERACIÓN ADMINISTRATIVA DESCONCENTRADA: {}", delegacion.to_uppercase()), &top_title_fmt).map_err(|e| e.to_string())?;
        ws_final.merge_range(2, 0, 2, 18, &format!("SUBDELEGACIÓN: {}", subdelegacion.to_uppercase()), &top_title_fmt).map_err(|e| e.to_string())?;
        ws_final.merge_range(3, 0, 3, 18, "LIQUIDACIONES", &top_title_fmt).map_err(|e| e.to_string())?;

        // Metadatos (Filas 4 a 9)
        let meta_rows = [
            ("NOMBRE DE LA EMPRESA", emp_nombre),
            ("REGISTRO PATRONAL", rp_reg),
            ("RFC", rfc_val),
            ("PERIODO DE OBRA", per_obra),
            ("NOMBRE DEL PATRON O REPRESENTANTE LEGAL", rep_legal),
            ("FOLIO CORRECCION", fol_rev),
        ];

        for (idx, (label, val)) in meta_rows.iter().enumerate() {
            let r = 4 + idx as u32;
            ws_final.merge_range(r, 2, r, 8, *label, &meta_lbl_fmt).map_err(|e| e.to_string())?;
            ws_final.merge_range(r, 9, r, 16, *val, &meta_val_fmt).map_err(|e| e.to_string())?;
        }

        // Marca visible de borrador en la fila libre (índice 10) bajo los metadatos
        if let ModoEntregable::Borrador { solo_vista_filtrada } = modo {
            let borrador_fmt = Format::new()
                .set_bold()
                .set_font_size(12)
                .set_font_color(Color::RGB(0xFF0000))
                .set_align(FormatAlign::Center)
                .set_align(FormatAlign::VerticalCenter);
            let leyenda = if solo_vista_filtrada {
                "BORRADOR — NO VÁLIDO PARA ENTREGA AL IMSS (solo vista filtrada)"
            } else {
                "BORRADOR — NO VÁLIDO PARA ENTREGA AL IMSS"
            };
            ws_final
                .merge_range(10, 0, 10, 18, leyenda, &borrador_fmt)
                .map_err(|e| e.to_string())?;
        }

        // Fila 11 (Índice 11 = fila 12 en Excel): Encabezados de Tabla
        let table_header_row = 11;
        let final_headers = [
            "nss", "nombre", "mes", "dias", "sdi",
            "cf", "exc pat", "exc obr", "pd pat", "pd obr",
            "gmp pat", "gmp obr", "rt", "iv pat", "iv obr",
            "gps", "patronal", "obrera", "subtotal"
        ];

        for (c, h) in final_headers.iter().enumerate() {
            ws_final.write_string_with_format(table_header_row, c as u16, *h, &table_hdr_fmt).map_err(|e| e.to_string())?;
        }

        // Agrupar filas cronológicamente
        let mut sorted_groups: Vec<((i32, u32), String, Vec<&ReporteLiquidacionRowDb>)> = Vec::new();
        for row in rows {
            let (m, y) = parse_month_and_year(&row.mes_periodo, row.anio_periodo, &row.periodo_str)
                .unwrap_or((0, 0));
            let p_key = if !row.periodo_str.is_empty() {
                row.periodo_str.clone()
            } else {
                format!("{:02}/{}", m, y)
            };

            if let Some(g) = sorted_groups.iter_mut().find(|(my, pk, _)| *my == (y, m) || pk == &p_key) {
                g.2.push(row);
            } else {
                sorted_groups.push(((y, m), p_key, vec![row]));
            }
        }
        sorted_groups.sort_by_key(|(my, _, _)| *my);

        let mut curr_rf = table_header_row + 1;
        let mut grand_dias: f64 = 0.0;
        let mut grand_sdi: f64 = 0.0;
        let mut grand_cf: f64 = 0.0;
        let mut grand_exc_pat: f64 = 0.0;
        let mut grand_exc_obr: f64 = 0.0;
        let mut grand_pd_pat: f64 = 0.0;
        let mut grand_pd_obr: f64 = 0.0;
        let mut grand_gmp_pat: f64 = 0.0;
        let mut grand_gmp_obr: f64 = 0.0;
        let mut grand_rt: f64 = 0.0;
        let mut grand_iv_pat: f64 = 0.0;
        let mut grand_iv_obr: f64 = 0.0;
        let mut grand_gps: f64 = 0.0;
        let mut grand_patronal: f64 = 0.0;
        let mut grand_obrera: f64 = 0.0;
        let mut grand_subtotal: f64 = 0.0;

        for ((_y, _m), periodo_key, mut group_rows) in sorted_groups {
            // Ordenar por nombre de trabajador ascendente
            group_rows.sort_by(|a, b| {
                a.nombre_trabajador.to_lowercase()
                    .cmp(&b.nombre_trabajador.to_lowercase())
                    .then_with(|| a.nss.cmp(&b.nss))
            });

            let mut m_dias: f64 = 0.0;
            let mut m_sdi: f64 = 0.0;
            let mut m_cf: f64 = 0.0;
            let mut m_exc_pat: f64 = 0.0;
            let mut m_exc_obr: f64 = 0.0;
            let mut m_pd_pat: f64 = 0.0;
            let mut m_pd_obr: f64 = 0.0;
            let mut m_gmp_pat: f64 = 0.0;
            let mut m_gmp_obr: f64 = 0.0;
            let mut m_rt: f64 = 0.0;
            let mut m_iv_pat: f64 = 0.0;
            let mut m_iv_obr: f64 = 0.0;
            let mut m_gps: f64 = 0.0;
            let mut m_patronal: f64 = 0.0;
            let mut m_obrera: f64 = 0.0;
            let mut m_subtotal: f64 = 0.0;

            let sample_first = group_rows.first().copied();

            for row in group_rows {
                let nss_formatted = format_nss_styled(&row.nss);
                let nombre_lower = row.nombre_trabajador.to_lowercase();
                let mes_col = format_mes_col(&row.mes_periodo, row.anio_periodo, &row.periodo_str);

                ws_final.write_string_with_format(curr_rf, 0, &nss_formatted, &row_txt_center).map_err(|e| e.to_string())?;
                ws_final.write_string_with_format(curr_rf, 1, &nombre_lower, &row_txt_left).map_err(|e| e.to_string())?;
                ws_final.write_string_with_format(curr_rf, 2, &mes_col, &row_txt_center).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 3, row.dias_obra as f64, &row_int_center).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 4, row.sdi, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 5, row.cf, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 6, row.exc_pat, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 7, row.exc_obr, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 8, row.pd_pat, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 9, row.pd_obr, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 10, row.gmp_pat, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 11, row.gmp_obr, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 12, row.rt, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 13, row.iv_pat, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 14, row.iv_obr, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 15, row.gps, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 16, row.patronal, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 17, row.obrera, &row_curr_right).map_err(|e| e.to_string())?;
                ws_final.write_number_with_format(curr_rf, 18, row.subtotal, &row_curr_right).map_err(|e| e.to_string())?;

                m_dias += row.dias_obra as f64;
                m_sdi += row.sdi;
                m_cf += row.cf;
                m_exc_pat += row.exc_pat;
                m_exc_obr += row.exc_obr;
                m_pd_pat += row.pd_pat;
                m_pd_obr += row.pd_obr;
                m_gmp_pat += row.gmp_pat;
                m_gmp_obr += row.gmp_obr;
                m_rt += row.rt;
                m_iv_pat += row.iv_pat;
                m_iv_obr += row.iv_obr;
                m_gps += row.gps;
                m_patronal += row.patronal;
                m_obrera += row.obrera;
                m_subtotal += row.subtotal;

                curr_rf += 1;
            }

            // Fila Total por Mes
            let (tot_label, tot_mes_col) = format_month_total_label(&periodo_key, sample_first);
            ws_final.write_string_with_format(curr_rf, 0, &tot_label, &month_tot_lbl).map_err(|e| e.to_string())?;
            ws_final.write_string_with_format(curr_rf, 1, "", &month_tot_lbl).map_err(|e| e.to_string())?;
            ws_final.write_string_with_format(curr_rf, 2, &tot_mes_col, &month_tot_center).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 3, m_dias, &month_tot_int).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 4, m_sdi, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 5, m_cf, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 6, m_exc_pat, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 7, m_exc_obr, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 8, m_pd_pat, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 9, m_pd_obr, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 10, m_gmp_pat, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 11, m_gmp_obr, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 12, m_rt, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 13, m_iv_pat, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 14, m_iv_obr, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 15, m_gps, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 16, m_patronal, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 17, m_obrera, &month_tot_curr).map_err(|e| e.to_string())?;
            ws_final.write_number_with_format(curr_rf, 18, m_subtotal, &month_tot_curr).map_err(|e| e.to_string())?;

            grand_dias += m_dias;
            grand_sdi += m_sdi;
            grand_cf += m_cf;
            grand_exc_pat += m_exc_pat;
            grand_exc_obr += m_exc_obr;
            grand_pd_pat += m_pd_pat;
            grand_pd_obr += m_pd_obr;
            grand_gmp_pat += m_gmp_pat;
            grand_gmp_obr += m_gmp_obr;
            grand_rt += m_rt;
            grand_iv_pat += m_iv_pat;
            grand_iv_obr += m_iv_obr;
            grand_gps += m_gps;
            grand_patronal += m_patronal;
            grand_obrera += m_obrera;
            grand_subtotal += m_subtotal;

            curr_rf += 1;
        }

        // Fila RESUMEN / GRAND TOTAL
        let grand_total_row = curr_rf;
        ws_final.write_string_with_format(grand_total_row, 0, "RESUMEN / GRAND TOTAL", &grand_tot_lbl).map_err(|e| e.to_string())?;
        ws_final.write_string_with_format(grand_total_row, 1, "", &grand_tot_lbl).map_err(|e| e.to_string())?;
        ws_final.write_string_with_format(grand_total_row, 2, "", &grand_tot_lbl).map_err(|e| e.to_string())?;
        // Totales como fórmulas SUMIF sobre las filas de subtotal por mes (sin doble conteo)
        let primera_det = table_header_row + 1;
        let ultima_det = grand_total_row - 1;
        let totales_final: [(u16, f64, &Format); 16] = [
            (3, grand_dias, &grand_tot_int),
            (4, grand_sdi, &grand_tot_curr),
            (5, grand_cf, &grand_tot_curr),
            (6, grand_exc_pat, &grand_tot_curr),
            (7, grand_exc_obr, &grand_tot_curr),
            (8, grand_pd_pat, &grand_tot_curr),
            (9, grand_pd_obr, &grand_tot_curr),
            (10, grand_gmp_pat, &grand_tot_curr),
            (11, grand_gmp_obr, &grand_tot_curr),
            (12, grand_rt, &grand_tot_curr),
            (13, grand_iv_pat, &grand_tot_curr),
            (14, grand_iv_obr, &grand_tot_curr),
            (15, grand_gps, &grand_tot_curr),
            (16, grand_patronal, &grand_tot_curr),
            (17, grand_obrera, &grand_tot_curr),
            (18, grand_subtotal, &grand_tot_curr),
        ];
        for (col, valor, fmt) in totales_final {
            escribir_total_sumif(ws_final, grand_total_row, col, primera_det, ultima_det, 0, valor, fmt)?;
        }

        // Activar autofiltro en la tabla
        let _ = ws_final.autofilter(table_header_row, 0, grand_total_row, 18);
        configurar_impresion(ws_final, table_header_row, 18)?;

        // Bloque de Firma y Valor Guarderías (1%)
        let sig_r = grand_total_row + 5;
        // Línea de firma en columna 1 (nombre)
        ws_final.write_string_with_format(sig_r - 1, 1, "", &sig_top_border).map_err(|e| e.to_string())?;

        let rep_title = format!("L.C. {}", rep_legal.to_uppercase());
        let apod_title = format!("APODERADO LEGAL DE LA EMPRESA {}", emp_nombre.to_uppercase());
        ws_final.write_string_with_format(sig_r, 1, &rep_title, &sig_text_fmt).map_err(|e| e.to_string())?;
        ws_final.write_string_with_format(sig_r + 1, 1, &apod_title, &sig_text_fmt).map_err(|e| e.to_string())?;

        // Indicador 1% y Total Guarderías
        ws_final.write_string_with_format(sig_r, 15, "1%", &kpi_pct_fmt).map_err(|e| e.to_string())?;
        let valor_guarderias_1pct = grand_gps / 0.01;
        ws_final.write_number_with_format(sig_r + 1, 15, valor_guarderias_1pct, &kpi_yellow_box).map_err(|e| e.to_string())?;
    }

    // =========================================================================
    // SHEET 1: Prorrateo por Obra
    // =========================================================================
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Prorrateo por Obra").map_err(|e| e.to_string())?;
    worksheet.set_header(&print_header_str);
    worksheet.set_footer(&print_footer_str);
    worksheet.set_header_footer_align_with_page(true);

    // Formatos generales
    let title_format = Format::new()
        .set_bold()
        .set_font_size(14)
        .set_font_color(Color::RGB(0x0F172A));

    let subtitle_format = Format::new()
        .set_font_size(10)
        .set_font_color(Color::RGB(0x475569));

    let header_format = Format::new()
        .set_bold()
        .set_font_size(10)
        .set_background_color(Color::RGB(0x1E3A8A))
        .set_font_color(Color::RGB(0xFFFFFF))
        .set_border(FormatBorder::Thin);

    let currency_format = Format::new()
        .set_num_format("$#,##0.00")
        .set_border(FormatBorder::Thin);

    let integer_format = Format::new()
        .set_num_format("#,##0")
        .set_border(FormatBorder::Thin);

    let text_format = Format::new()
        .set_border(FormatBorder::Thin);

    // Formatos para Subtotal por Mes
    let month_total_label_format = Format::new()
        .set_bold()
        .set_font_size(10)
        .set_background_color(Color::RGB(0xDBEAFE))
        .set_font_color(Color::RGB(0x1E40AF))
        .set_border(FormatBorder::Thin);

    let month_total_curr_format = Format::new()
        .set_bold()
        .set_font_size(10)
        .set_background_color(Color::RGB(0xDBEAFE))
        .set_font_color(Color::RGB(0x1E40AF))
        .set_num_format("$#,##0.00")
        .set_border(FormatBorder::Thin);

    let month_total_int_format = Format::new()
        .set_bold()
        .set_font_size(10)
        .set_background_color(Color::RGB(0xDBEAFE))
        .set_font_color(Color::RGB(0x1E40AF))
        .set_num_format("#,##0")
        .set_border(FormatBorder::Thin);

    // Formatos para Total General
    let grand_total_label_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_background_color(Color::RGB(0x1E3A8A))
        .set_font_color(Color::RGB(0xFFFFFF))
        .set_border(FormatBorder::Medium);

    let grand_total_curr_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_background_color(Color::RGB(0x1E3A8A))
        .set_font_color(Color::RGB(0xFFFFFF))
        .set_num_format("$#,##0.00")
        .set_border(FormatBorder::Medium);

    let grand_total_int_format = Format::new()
        .set_bold()
        .set_font_size(11)
        .set_background_color(Color::RGB(0x1E3A8A))
        .set_font_color(Color::RGB(0xFFFFFF))
        .set_num_format("#,##0")
        .set_border(FormatBorder::Medium);

    // Encabezado del Reporte
    worksheet.write_string_with_format(0, 0, "Reporte de Liquidaciones y Prorrateo por Obra - CONSTRUAUDIT", &title_format).map_err(|e| e.to_string())?;
    worksheet.write_string_with_format(1, 0, &format!("Empresa: {} | Registro Patronal: {} | Revisión: {}", cliente_nombre, rp_registro, revision_folio), &subtitle_format).map_err(|e| e.to_string())?;
    worksheet.write_string_with_format(2, 0, "Fórmula: (Monto Cédula / Días Cédula) * Días Obra", &subtitle_format).map_err(|e| e.to_string())?;

    // Encabezados sin la columna "Total Prorrateado"
    let headers = [
        "#", "Periodo", "NSS", "RFC", "CURP", "Nombre del Trabajador", "Reg. Patronal",
        "Registro de Obra", "Clave", "Fecha", "SDI", "Días Cédula", "Días Obra",
        "Lic.", "Inc.", "Aus.",
        "Cuota Fija", "Exc. Pat.", "Exc. Obr.", "PD Pat.", "PD Obr.", "GMP Pat.", "GMP Obr.",
        "Riesgo Trab.", "IV Pat.", "IV Obr.", "Guard y PS", "Suma Patronal", "Suma Obrera", "Subtotal", "Subtotal Cédula"
    ];

    let header_row = 4;
    for (col, h) in headers.iter().enumerate() {
        worksheet.write_string_with_format(header_row, col as u16, *h, &header_format).map_err(|e| e.to_string())?;
    }

    // Agrupar filas por mes / periodo
    let mut groups: Vec<(String, Vec<&ReporteLiquidacionRowDb>)> = Vec::new();
    for row in rows {
        let p = if !row.periodo_str.is_empty() {
            row.periodo_str.clone()
        } else {
            "Sin Periodo".to_string()
        };
        if let Some(g) = groups.iter_mut().find(|(k, _)| k == &p) {
            g.1.push(row);
        } else {
            groups.push((p, vec![row]));
        }
    }

    // Acumuladores de Total General
    let mut g_dias_ced: f64 = 0.0;
    let mut g_dias_obr: f64 = 0.0;
    let mut g_lic: f64 = 0.0;
    let mut g_inc: f64 = 0.0;
    let mut g_aus: f64 = 0.0;
    let mut g_cf: f64 = 0.0;
    let mut g_exc_pat: f64 = 0.0;
    let mut g_exc_obr: f64 = 0.0;
    let mut g_pd_pat: f64 = 0.0;
    let mut g_pd_obr: f64 = 0.0;
    let mut g_gmp_pat: f64 = 0.0;
    let mut g_gmp_obr: f64 = 0.0;
    let mut g_rt: f64 = 0.0;
    let mut g_iv_pat: f64 = 0.0;
    let mut g_iv_obr: f64 = 0.0;
    let mut g_gps: f64 = 0.0;
    let mut g_patronal: f64 = 0.0;
    let mut g_obrera: f64 = 0.0;
    let mut g_subtotal: f64 = 0.0;
    let mut g_subtotal_orig: f64 = 0.0;

    let mut current_r = header_row + 1;
    let mut global_idx = 1;

    for (periodo_key, group_rows) in &groups {
        let mut m_dias_ced: f64 = 0.0;
        let mut m_dias_obr: f64 = 0.0;
        let mut m_lic: f64 = 0.0;
        let mut m_inc: f64 = 0.0;
        let mut m_aus: f64 = 0.0;
        let mut m_cf: f64 = 0.0;
        let mut m_exc_pat: f64 = 0.0;
        let mut m_exc_obr: f64 = 0.0;
        let mut m_pd_pat: f64 = 0.0;
        let mut m_pd_obr: f64 = 0.0;
        let mut m_gmp_pat: f64 = 0.0;
        let mut m_gmp_obr: f64 = 0.0;
        let mut m_rt: f64 = 0.0;
        let mut m_iv_pat: f64 = 0.0;
        let mut m_iv_obr: f64 = 0.0;
        let mut m_gps: f64 = 0.0;
        let mut m_patronal: f64 = 0.0;
        let mut m_obrera: f64 = 0.0;
        let mut m_subtotal: f64 = 0.0;
        let mut m_subtotal_orig: f64 = 0.0;

        for row in group_rows {
            let rp_clean = clean_rp_text(&row.registro_patronal);
            let obra_clean = clean_obra_text(&row.obra_numero, &row.obra_label, row.id_registro_obra.is_some());

            worksheet.write_number_with_format(current_r, 0, global_idx as f64, &integer_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 1, &row.periodo_str, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 2, &row.nss, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 3, &row.rfc, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 4, &row.curp, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 5, &row.nombre_trabajador, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 6, rp_clean, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 7, obra_clean, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 8, &row.clave, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 9, &row.fecha_str, &text_format).map_err(|e| e.to_string())?;
            if row.sdi > 0.001 {
                worksheet.write_number_with_format(current_r, 10, row.sdi, &currency_format).map_err(|e| e.to_string())?;
            } else {
                worksheet.write_string_with_format(current_r, 10, "-", &text_format).map_err(|e| e.to_string())?;
            }
            worksheet.write_number_with_format(current_r, 11, row.dias_cedula as f64, &integer_format).map_err(|e| e.to_string())?;
            if row.id_registro_obra.is_some() {
                worksheet.write_number_with_format(current_r, 12, row.dias_obra as f64, &integer_format).map_err(|e| e.to_string())?;
                m_dias_obr += row.dias_obra as f64;
            } else {
                worksheet.write_string_with_format(current_r, 12, "", &text_format).map_err(|e| e.to_string())?;
            }
            if row.lic > 0.001 {
                worksheet.write_number_with_format(current_r, 13, row.lic, &integer_format).map_err(|e| e.to_string())?;
                m_lic += row.lic;
            } else {
                worksheet.write_string_with_format(current_r, 13, "-", &text_format).map_err(|e| e.to_string())?;
            }
            if row.inc > 0.001 {
                worksheet.write_number_with_format(current_r, 14, row.inc, &integer_format).map_err(|e| e.to_string())?;
                m_inc += row.inc;
            } else {
                worksheet.write_string_with_format(current_r, 14, "-", &text_format).map_err(|e| e.to_string())?;
            }
            if row.aus > 0.001 {
                worksheet.write_number_with_format(current_r, 15, row.aus, &integer_format).map_err(|e| e.to_string())?;
                m_aus += row.aus;
            } else {
                worksheet.write_string_with_format(current_r, 15, "-", &text_format).map_err(|e| e.to_string())?;
            }

            worksheet.write_number_with_format(current_r, 16, row.cf, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 17, row.exc_pat, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 18, row.exc_obr, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 19, row.pd_pat, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 20, row.pd_obr, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 21, row.gmp_pat, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 22, row.gmp_obr, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 23, row.rt, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 24, row.iv_pat, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 25, row.iv_obr, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 26, row.gps, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 27, row.patronal, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 28, row.obrera, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 29, row.subtotal, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 30, row.subtotal_orig, &currency_format).map_err(|e| e.to_string())?;

            m_dias_ced += row.dias_cedula as f64;
            m_cf += row.cf;
            m_exc_pat += row.exc_pat;
            m_exc_obr += row.exc_obr;
            m_pd_pat += row.pd_pat;
            m_pd_obr += row.pd_obr;
            m_gmp_pat += row.gmp_pat;
            m_gmp_obr += row.gmp_obr;
            m_rt += row.rt;
            m_iv_pat += row.iv_pat;
            m_iv_obr += row.iv_obr;
            m_gps += row.gps;
            m_patronal += row.patronal;
            m_obrera += row.obrera;
            m_subtotal += row.subtotal;
            m_subtotal_orig += row.subtotal_orig;

            current_r += 1;
            global_idx += 1;
        }

        // Fila de Total por Mes
        for c in 0..=4 {
            worksheet.write_string_with_format(current_r, c as u16, "", &month_total_label_format).map_err(|e| e.to_string())?;
        }
        worksheet.write_string_with_format(current_r, 5, &format!("TOTAL {}", periodo_key), &month_total_label_format).map_err(|e| e.to_string())?;
        for c in 6..=10 {
            worksheet.write_string_with_format(current_r, c as u16, "", &month_total_label_format).map_err(|e| e.to_string())?;
        }

        worksheet.write_number_with_format(current_r, 11, m_dias_ced, &month_total_int_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 12, m_dias_obr, &month_total_int_format).map_err(|e| e.to_string())?;
        if m_lic > 0.001 {
            worksheet.write_number_with_format(current_r, 13, m_lic, &month_total_int_format).map_err(|e| e.to_string())?;
        } else {
            worksheet.write_string_with_format(current_r, 13, "-", &month_total_label_format).map_err(|e| e.to_string())?;
        }
        if m_inc > 0.001 {
            worksheet.write_number_with_format(current_r, 14, m_inc, &month_total_int_format).map_err(|e| e.to_string())?;
        } else {
            worksheet.write_string_with_format(current_r, 14, "-", &month_total_label_format).map_err(|e| e.to_string())?;
        }
        if m_aus > 0.001 {
            worksheet.write_number_with_format(current_r, 15, m_aus, &month_total_int_format).map_err(|e| e.to_string())?;
        } else {
            worksheet.write_string_with_format(current_r, 15, "-", &month_total_label_format).map_err(|e| e.to_string())?;
        }

        worksheet.write_number_with_format(current_r, 16, m_cf, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 17, m_exc_pat, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 18, m_exc_obr, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 19, m_pd_pat, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 20, m_pd_obr, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 21, m_gmp_pat, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 22, m_gmp_obr, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 23, m_rt, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 24, m_iv_pat, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 25, m_iv_obr, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 26, m_gps, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 27, m_patronal, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 28, m_obrera, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 29, m_subtotal, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 30, m_subtotal_orig, &month_total_curr_format).map_err(|e| e.to_string())?;

        current_r += 1;

        // Acumular a Total General
        g_dias_ced += m_dias_ced;
        g_dias_obr += m_dias_obr;
        g_lic += m_lic;
        g_inc += m_inc;
        g_aus += m_aus;
        g_cf += m_cf;
        g_exc_pat += m_exc_pat;
        g_exc_obr += m_exc_obr;
        g_pd_pat += m_pd_pat;
        g_pd_obr += m_pd_obr;
        g_gmp_pat += m_gmp_pat;
        g_gmp_obr += m_gmp_obr;
        g_rt += m_rt;
        g_iv_pat += m_iv_pat;
        g_iv_obr += m_iv_obr;
        g_gps += m_gps;
        g_patronal += m_patronal;
        g_obrera += m_obrera;
        g_subtotal += m_subtotal;
        g_subtotal_orig += m_subtotal_orig;
    }

    // Fila de Total General
    for c in 0..=4 {
        worksheet.write_string_with_format(current_r, c as u16, "", &grand_total_label_format).map_err(|e| e.to_string())?;
    }
    worksheet.write_string_with_format(current_r, 5, "TOTAL GENERAL", &grand_total_label_format).map_err(|e| e.to_string())?;
    for c in 6..=10 {
        worksheet.write_string_with_format(current_r, c as u16, "", &grand_total_label_format).map_err(|e| e.to_string())?;
    }

    // Totales generales como fórmulas SUMIF sobre las filas de subtotal por mes
    let primera_det_p = header_row + 1;
    let ultima_det_p = current_r - 1;
    escribir_total_sumif(worksheet, current_r, 11, primera_det_p, ultima_det_p, 5, g_dias_ced, &grand_total_int_format)?;
    escribir_total_sumif(worksheet, current_r, 12, primera_det_p, ultima_det_p, 5, g_dias_obr, &grand_total_int_format)?;
    for (col, valor) in [(13u16, g_lic), (14u16, g_inc), (15u16, g_aus)] {
        if valor > 0.001 {
            escribir_total_sumif(worksheet, current_r, col, primera_det_p, ultima_det_p, 5, valor, &grand_total_int_format)?;
        } else {
            worksheet.write_string_with_format(current_r, col, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
        }
    }
    let totales_p: [(u16, f64); 15] = [
        (16, g_cf),
        (17, g_exc_pat),
        (18, g_exc_obr),
        (19, g_pd_pat),
        (20, g_pd_obr),
        (21, g_gmp_pat),
        (22, g_gmp_obr),
        (23, g_rt),
        (24, g_iv_pat),
        (25, g_iv_obr),
        (26, g_gps),
        (27, g_patronal),
        (28, g_obrera),
        (29, g_subtotal),
        (30, g_subtotal_orig),
    ];
    for (col, valor) in totales_p {
        escribir_total_sumif(worksheet, current_r, col, primera_det_p, ultima_det_p, 5, valor, &grand_total_curr_format)?;
    }

    // =========================================================================
    // SECCIÓN POST-TABLA: Análisis de Guarderías (Tope 18% por Registro de Obra)
    // =========================================================================
    if !guarderias_kpis.is_empty() {
        let mut kpi_r = current_r + 3;

        let kpi_sec_title_format = Format::new()
            .set_bold()
            .set_font_size(12)
            .set_font_color(Color::RGB(0x1E3A8A));

        let kpi_sec_sub_format = Format::new()
            .set_font_size(9)
            .set_italic()
            .set_font_color(Color::RGB(0x475569));

        let kpi_tbl_hdr_format = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_background_color(Color::RGB(0x1E3A8A))
            .set_font_color(Color::RGB(0xFFFFFF))
            .set_border(FormatBorder::Thin);

        let kpi_text_format = Format::new()
            .set_font_size(10)
            .set_border(FormatBorder::Thin);

        let kpi_num_format = Format::new()
            .set_num_format("#,##0")
            .set_font_size(10)
            .set_border(FormatBorder::Thin);

        let kpi_curr_format = Format::new()
            .set_num_format("$#,##0.00")
            .set_font_size(10)
            .set_border(FormatBorder::Thin);

        let kpi_warn_curr_format = Format::new()
            .set_bold()
            .set_num_format("$#,##0.00")
            .set_font_size(10)
            .set_background_color(Color::RGB(0xFEF9C3))
            .set_font_color(Color::RGB(0x854D0E))
            .set_border(FormatBorder::Thin);

        let kpi_green_curr_format = Format::new()
            .set_bold()
            .set_num_format("$#,##0.00")
            .set_font_size(10)
            .set_background_color(Color::RGB(0xF0FDF4))
            .set_font_color(Color::RGB(0x15803D))
            .set_border(FormatBorder::Thin);

        let kpi_blue_curr_format = Format::new()
            .set_bold()
            .set_num_format("$#,##0.00")
            .set_font_size(10)
            .set_font_color(Color::RGB(0x2563EB))
            .set_border(FormatBorder::Thin);

        let kpi_status_ok_format = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_background_color(Color::RGB(0xDCFCE7))
            .set_font_color(Color::RGB(0x166534))
            .set_border(FormatBorder::Thin);

        let kpi_status_warn_format = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_background_color(Color::RGB(0xFEF08A))
            .set_font_color(Color::RGB(0x854D0E))
            .set_border(FormatBorder::Thin);

        worksheet.write_string_with_format(
            kpi_r,
            0,
            "ANÁLISIS DE GUARDERÍAS (TOPE 18% POR REGISTRO DE OBRA)",
            &kpi_sec_title_format,
        ).map_err(|e| e.to_string())?;

        kpi_r += 1;
        worksheet.write_string_with_format(
            kpi_r,
            0,
            "Fórmula en vivo: Valor Guarderías = Total Cuotas GPS / 0.01 (1%) | Límite 18% = Valor Total de la Obra * 18%",
            &kpi_sec_sub_format,
        ).map_err(|e| e.to_string())?;

        kpi_r += 2;

        let kpi_headers = [
            "#",
            "Registro de Obra",
            "Valor Guarderías (1% GPS)",
            "Valor Total de la Obra",
            "Límite 18% de la Obra",
            "Total Cuotas GPS (IMSS)",
            "Registros Vinculados",
            "Estatus (Tope 18%)",
        ];

        for (col, h) in kpi_headers.iter().enumerate() {
            worksheet.write_string_with_format(kpi_r, col as u16, *h, &kpi_tbl_hdr_format)
                .map_err(|e| e.to_string())?;
        }

        let mut tot_valor_guarderias = 0.0;
        let mut tot_monto_obra = 0.0;
        let mut tot_limite_18 = 0.0;
        let mut tot_gps = 0.0;
        let mut tot_registros = 0;
        let mut hay_alerta = false;

        for (idx, kpi) in guarderias_kpis.iter().enumerate() {
            kpi_r += 1;
            worksheet.write_number_with_format(kpi_r, 0, (idx + 1) as f64, &integer_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(kpi_r, 1, &kpi.obra_label, &kpi_text_format)
                .map_err(|e| e.to_string())?;

            if kpi.supera_18 {
                worksheet.write_number_with_format(kpi_r, 2, kpi.valor_guarderias, &kpi_warn_curr_format)
                    .map_err(|e| e.to_string())?;
                hay_alerta = true;
            } else {
                worksheet.write_number_with_format(kpi_r, 2, kpi.valor_guarderias, &kpi_curr_format)
                    .map_err(|e| e.to_string())?;
            }

            if kpi.sin_monto {
                worksheet.write_string_with_format(kpi_r, 3, "N/D", &kpi_text_format)
                    .map_err(|e| e.to_string())?;
                worksheet.write_string_with_format(kpi_r, 4, "N/D", &kpi_text_format)
                    .map_err(|e| e.to_string())?;
            } else {
                worksheet.write_number_with_format(kpi_r, 3, kpi.monto_obra, &kpi_curr_format)
                    .map_err(|e| e.to_string())?;
                worksheet.write_number_with_format(kpi_r, 4, kpi.limite_18_obra, &kpi_green_curr_format)
                    .map_err(|e| e.to_string())?;
            }
            worksheet.write_number_with_format(kpi_r, 5, kpi.total_gps, &kpi_blue_curr_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 6, kpi.empleados_count as f64, &kpi_num_format)
                .map_err(|e| e.to_string())?;

            if kpi.sin_monto {
                worksheet.write_string_with_format(kpi_r, 7, "⚪ SIN MONTO (N/D)", &kpi_text_format)
                    .map_err(|e| e.to_string())?;
            } else if kpi.supera_18 {
                worksheet.write_string_with_format(kpi_r, 7, "⚠️ SUPERA EL 18%", &kpi_status_warn_format)
                    .map_err(|e| e.to_string())?;
            } else {
                worksheet.write_string_with_format(kpi_r, 7, "✅ CUMPLE (<= 18%)", &kpi_status_ok_format)
                    .map_err(|e| e.to_string())?;
            }

            tot_valor_guarderias += kpi.valor_guarderias;
            tot_monto_obra += kpi.monto_obra;
            tot_limite_18 += kpi.limite_18_obra;
            tot_gps += kpi.total_gps;
            tot_registros += kpi.empleados_count;
        }

        if guarderias_kpis.len() > 1 {
            kpi_r += 1;
            worksheet.write_string_with_format(kpi_r, 0, "", &grand_total_label_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(kpi_r, 1, "TOTAL CONSOLIDADO", &grand_total_label_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 2, tot_valor_guarderias, &grand_total_curr_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 3, tot_monto_obra, &grand_total_curr_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 4, tot_limite_18, &grand_total_curr_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 5, tot_gps, &grand_total_curr_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 6, tot_registros as f64, &grand_total_int_format)
                .map_err(|e| e.to_string())?;
            let tot_status = if hay_alerta { "⚠️ OBRAS EN ALERTA" } else { "✅ TODAS CUMPLEN" };
            worksheet.write_string_with_format(kpi_r, 7, tot_status, &grand_total_label_format)
                .map_err(|e| e.to_string())?;
        }

        // Tarjetas individuales de cada Obra (idéntico a la vista en la aplicación)
        kpi_r += 3;
        for kpi in guarderias_kpis {
            let card_header_text = format!("🏗️ Registro de Obra: {} ({} registros vinculados)", kpi.obra_label, kpi.empleados_count);
            worksheet.write_string_with_format(kpi_r, 0, &card_header_text, &kpi_sec_title_format)
                .map_err(|e| e.to_string())?;

            if kpi.sin_monto {
                kpi_r += 1;
                worksheet.write_string_with_format(
                    kpi_r,
                    0,
                    "Sin monto capturado: no se calcula el límite del 18%",
                    &kpi_sec_sub_format,
                ).map_err(|e| e.to_string())?;
            }

            if kpi.supera_18 {
                kpi_r += 1;
                worksheet.write_string_with_format(
                    kpi_r,
                    0,
                    "⚠️ ADVERTENCIA: Guarderías supera el 18% del valor de la obra",
                    &kpi_status_warn_format,
                ).map_err(|e| e.to_string())?;
            }

            kpi_r += 1;
            worksheet.write_string_with_format(kpi_r, 0, "VALOR GUARDERÍAS (1% GPS)", &kpi_tbl_hdr_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(kpi_r, 1, "VALOR TOTAL DE LA OBRA", &kpi_tbl_hdr_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(kpi_r, 2, "LÍMITE 18% DE LA OBRA", &kpi_tbl_hdr_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(kpi_r, 3, "TOTAL CUOTAS GPS (IMSS)", &kpi_tbl_hdr_format).map_err(|e| e.to_string())?;

            kpi_r += 1;
            if kpi.supera_18 {
                worksheet.write_number_with_format(kpi_r, 0, kpi.valor_guarderias, &kpi_warn_curr_format).map_err(|e| e.to_string())?;
            } else {
                worksheet.write_number_with_format(kpi_r, 0, kpi.valor_guarderias, &kpi_curr_format).map_err(|e| e.to_string())?;
            }
            if kpi.sin_monto {
                worksheet.write_string_with_format(kpi_r, 1, "N/D", &kpi_text_format).map_err(|e| e.to_string())?;
                worksheet.write_string_with_format(kpi_r, 2, "N/D", &kpi_text_format).map_err(|e| e.to_string())?;
            } else {
                worksheet.write_number_with_format(kpi_r, 1, kpi.monto_obra, &kpi_curr_format).map_err(|e| e.to_string())?;
                worksheet.write_number_with_format(kpi_r, 2, kpi.limite_18_obra, &kpi_green_curr_format).map_err(|e| e.to_string())?;
            }
            worksheet.write_number_with_format(kpi_r, 3, kpi.total_gps, &kpi_blue_curr_format).map_err(|e| e.to_string())?;

            kpi_r += 2;
        }
    }

    worksheet.autofit();
    configurar_impresion(worksheet, header_row, 30)?;

    let is_valid_user_obra = |r: &ReporteLiquidacionRowDb| -> bool {
        if let Some(oid) = r.id_registro_obra {
            if let Some(valid_ids) = valid_user_obra_ids {
                return valid_ids.contains(&oid);
            }
            return r.obra_creado_usuario;
        }
        false
    };

    // =========================================================================
    // SHEET 2: Totales por Empleado y por Obra (Solo registros de obra de la revisión)
    // =========================================================================
    {
        let ws_emp_obra = workbook.add_worksheet();
        ws_emp_obra.set_name("Totales por Empleado y Obra").map_err(|e| e.to_string())?;
        ws_emp_obra.set_header(&print_header_str);
        ws_emp_obra.set_footer(&print_footer_str);
        ws_emp_obra.set_header_footer_align_with_page(true);

        // Encabezado del Reporte
        ws_emp_obra.write_string_with_format(
            0,
            0,
            "Reporte de Liquidaciones - Totales por Empleado y Registro de Obra - CONSTRUAUDIT",
            &title_format,
        ).map_err(|e| e.to_string())?;

        ws_emp_obra.write_string_with_format(
            1,
            0,
            &format!("Empresa: {} | Registro Patronal: {} | Revisión: {}", cliente_nombre, rp_registro, revision_folio),
            &subtitle_format,
        ).map_err(|e| e.to_string())?;

        ws_emp_obra.write_string_with_format(
            2,
            0,
            "Filtro: Registros de Obra de la Revisión | Totales acumulados de todos los meses",
            &subtitle_format,
        ).map_err(|e| e.to_string())?;

        let headers_s2 = [
            "#", "NSS", "RFC", "CURP", "Nombre del Trabajador", "Reg. Patronal",
            "Registro de Obra", "Días Cédula", "Días Obra",
            "Lic.", "Inc.", "Aus.",
            "Cuota Fija", "Exc. Pat.", "Exc. Obr.", "PD Pat.", "PD Obr.", "GMP Pat.", "GMP Obr.",
            "Riesgo Trab.", "IV Pat.", "IV Obr.", "Guard y PS", "Suma Patronal", "Suma Obrera", "Subtotal", "Subtotal Cédula"
        ];

        let header_row_s2 = 4;
        for (col, h) in headers_s2.iter().enumerate() {
            ws_emp_obra.write_string_with_format(header_row_s2, col as u16, *h, &header_format).map_err(|e| e.to_string())?;
        }

        struct AggEmpObra {
            nss: String,
            rfc: String,
            curp: String,
            nombre_trabajador: String,
            registro_patronal: String,
            obra_numero: String,
            dias_cedula: i64,
            dias_obra: i64,
            lic: f64,
            inc: f64,
            aus: f64,
            cf: f64,
            exc_pat: f64,
            exc_obr: f64,
            pd_pat: f64,
            pd_obr: f64,
            gmp_pat: f64,
            gmp_obr: f64,
            rt: f64,
            iv_pat: f64,
            iv_obr: f64,
            gps: f64,
            patronal: f64,
            obrera: f64,
            subtotal: f64,
            subtotal_orig: f64,
        }

        let mut groups_s2: HashMap<(Uuid, Uuid), AggEmpObra> = HashMap::new();

        for r in all_rows {
            if !is_valid_user_obra(r) {
                continue;
            }
            let obra_id = r.id_registro_obra.unwrap();
            let key = (r.id_empleado, obra_id);

            let entry = groups_s2.entry(key).or_insert_with(|| {
                AggEmpObra {
                    nss: r.nss.clone(),
                    rfc: r.rfc.clone(),
                    curp: r.curp.clone(),
                    nombre_trabajador: r.nombre_trabajador.clone(),
                    registro_patronal: clean_rp_text(&r.registro_patronal).to_string(),
                    obra_numero: clean_obra_text(&r.obra_numero, &r.obra_label, true).to_string(),
                    dias_cedula: 0,
                    dias_obra: 0,
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
                    patronal: 0.0,
                    obrera: 0.0,
                    subtotal: 0.0,
                    subtotal_orig: 0.0,
                }
            });

            entry.dias_cedula += r.dias_cedula as i64;
            entry.dias_obra += r.dias_obra as i64;
            entry.lic += r.lic;
            entry.inc += r.inc;
            entry.aus += r.aus;
            entry.cf += r.cf;
            entry.exc_pat += r.exc_pat;
            entry.exc_obr += r.exc_obr;
            entry.pd_pat += r.pd_pat;
            entry.pd_obr += r.pd_obr;
            entry.gmp_pat += r.gmp_pat;
            entry.gmp_obr += r.gmp_obr;
            entry.rt += r.rt;
            entry.iv_pat += r.iv_pat;
            entry.iv_obr += r.iv_obr;
            entry.gps += r.gps;
            entry.patronal += r.patronal;
            entry.obrera += r.obrera;
            entry.subtotal += r.subtotal;
            entry.subtotal_orig += r.subtotal_orig;
        }

        let mut list_s2: Vec<AggEmpObra> = groups_s2.into_values().collect();
        list_s2.sort_by(|a, b| {
            a.nombre_trabajador.to_lowercase()
                .cmp(&b.nombre_trabajador.to_lowercase())
                .then_with(|| a.obra_numero.cmp(&b.obra_numero))
        });

        let mut s2_dias_ced: f64 = 0.0;
        let mut s2_dias_obr: f64 = 0.0;
        let mut s2_lic: f64 = 0.0;
        let mut s2_inc: f64 = 0.0;
        let mut s2_aus: f64 = 0.0;
        let mut s2_cf: f64 = 0.0;
        let mut s2_exc_pat: f64 = 0.0;
        let mut s2_exc_obr: f64 = 0.0;
        let mut s2_pd_pat: f64 = 0.0;
        let mut s2_pd_obr: f64 = 0.0;
        let mut s2_gmp_pat: f64 = 0.0;
        let mut s2_gmp_obr: f64 = 0.0;
        let mut s2_rt: f64 = 0.0;
        let mut s2_iv_pat: f64 = 0.0;
        let mut s2_iv_obr: f64 = 0.0;
        let mut s2_gps: f64 = 0.0;
        let mut s2_patronal: f64 = 0.0;
        let mut s2_obrera: f64 = 0.0;
        let mut s2_subtotal: f64 = 0.0;
        let mut s2_subtotal_orig: f64 = 0.0;

        let mut curr_r_s2 = header_row_s2 + 1;
        for (idx, item) in list_s2.iter().enumerate() {
            ws_emp_obra.write_number_with_format(curr_r_s2, 0, (idx + 1) as f64, &integer_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_string_with_format(curr_r_s2, 1, &item.nss, &text_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_string_with_format(curr_r_s2, 2, &item.rfc, &text_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_string_with_format(curr_r_s2, 3, &item.curp, &text_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_string_with_format(curr_r_s2, 4, &item.nombre_trabajador, &text_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_string_with_format(curr_r_s2, 5, &item.registro_patronal, &text_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_string_with_format(curr_r_s2, 6, &item.obra_numero, &text_format).map_err(|e| e.to_string())?;

            ws_emp_obra.write_number_with_format(curr_r_s2, 7, item.dias_cedula as f64, &integer_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 8, item.dias_obra as f64, &integer_format).map_err(|e| e.to_string())?;

            if item.lic > 0.001 {
                ws_emp_obra.write_number_with_format(curr_r_s2, 9, item.lic, &integer_format).map_err(|e| e.to_string())?;
            } else {
                ws_emp_obra.write_string_with_format(curr_r_s2, 9, "-", &text_format).map_err(|e| e.to_string())?;
            }
            if item.inc > 0.001 {
                ws_emp_obra.write_number_with_format(curr_r_s2, 10, item.inc, &integer_format).map_err(|e| e.to_string())?;
            } else {
                ws_emp_obra.write_string_with_format(curr_r_s2, 10, "-", &text_format).map_err(|e| e.to_string())?;
            }
            if item.aus > 0.001 {
                ws_emp_obra.write_number_with_format(curr_r_s2, 11, item.aus, &integer_format).map_err(|e| e.to_string())?;
            } else {
                ws_emp_obra.write_string_with_format(curr_r_s2, 11, "-", &text_format).map_err(|e| e.to_string())?;
            }

            ws_emp_obra.write_number_with_format(curr_r_s2, 12, item.cf, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 13, item.exc_pat, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 14, item.exc_obr, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 15, item.pd_pat, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 16, item.pd_obr, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 17, item.gmp_pat, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 18, item.gmp_obr, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 19, item.rt, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 20, item.iv_pat, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 21, item.iv_obr, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 22, item.gps, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 23, item.patronal, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 24, item.obrera, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 25, item.subtotal, &currency_format).map_err(|e| e.to_string())?;
            ws_emp_obra.write_number_with_format(curr_r_s2, 26, item.subtotal_orig, &currency_format).map_err(|e| e.to_string())?;

            s2_dias_ced += item.dias_cedula as f64;
            s2_dias_obr += item.dias_obra as f64;
            s2_lic += item.lic;
            s2_inc += item.inc;
            s2_aus += item.aus;
            s2_cf += item.cf;
            s2_exc_pat += item.exc_pat;
            s2_exc_obr += item.exc_obr;
            s2_pd_pat += item.pd_pat;
            s2_pd_obr += item.pd_obr;
            s2_gmp_pat += item.gmp_pat;
            s2_gmp_obr += item.gmp_obr;
            s2_rt += item.rt;
            s2_iv_pat += item.iv_pat;
            s2_iv_obr += item.iv_obr;
            s2_gps += item.gps;
            s2_patronal += item.patronal;
            s2_obrera += item.obrera;
            s2_subtotal += item.subtotal;
            s2_subtotal_orig += item.subtotal_orig;

            curr_r_s2 += 1;
        }

        // Fila Total General Sheet 2
        for c in 0..=3 {
            ws_emp_obra.write_string_with_format(curr_r_s2, c as u16, "", &grand_total_label_format).map_err(|e| e.to_string())?;
        }
        ws_emp_obra.write_string_with_format(curr_r_s2, 4, "TOTAL GENERAL", &grand_total_label_format).map_err(|e| e.to_string())?;
        for c in 5..=6 {
            ws_emp_obra.write_string_with_format(curr_r_s2, c as u16, "", &grand_total_label_format).map_err(|e| e.to_string())?;
        }

        ws_emp_obra.write_number_with_format(curr_r_s2, 7, s2_dias_ced, &grand_total_int_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 8, s2_dias_obr, &grand_total_int_format).map_err(|e| e.to_string())?;

        if s2_lic > 0.001 {
            ws_emp_obra.write_number_with_format(curr_r_s2, 9, s2_lic, &grand_total_int_format).map_err(|e| e.to_string())?;
        } else {
            ws_emp_obra.write_string_with_format(curr_r_s2, 9, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
        }
        if s2_inc > 0.001 {
            ws_emp_obra.write_number_with_format(curr_r_s2, 10, s2_inc, &grand_total_int_format).map_err(|e| e.to_string())?;
        } else {
            ws_emp_obra.write_string_with_format(curr_r_s2, 10, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
        }
        if s2_aus > 0.001 {
            ws_emp_obra.write_number_with_format(curr_r_s2, 11, s2_aus, &grand_total_int_format).map_err(|e| e.to_string())?;
        } else {
            ws_emp_obra.write_string_with_format(curr_r_s2, 11, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
        }

        ws_emp_obra.write_number_with_format(curr_r_s2, 12, s2_cf, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 13, s2_exc_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 14, s2_exc_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 15, s2_pd_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 16, s2_pd_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 17, s2_gmp_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 18, s2_gmp_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 19, s2_rt, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 20, s2_iv_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 21, s2_iv_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 22, s2_gps, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 23, s2_patronal, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 24, s2_obrera, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 25, s2_subtotal, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp_obra.write_number_with_format(curr_r_s2, 26, s2_subtotal_orig, &grand_total_curr_format).map_err(|e| e.to_string())?;

        ws_emp_obra.autofit();
        configurar_impresion(ws_emp_obra, header_row_s2, 26)?;
    }

    // =========================================================================
    // SHEET 3: Totales por Empleado (Todos los registros de obra de la revisión)
    // =========================================================================
    {
        let ws_emp = workbook.add_worksheet();
        ws_emp.set_name("Totales por Empleado").map_err(|e| e.to_string())?;
        ws_emp.set_header(&print_header_str);
        ws_emp.set_footer(&print_footer_str);
        ws_emp.set_header_footer_align_with_page(true);

        // Encabezado del Reporte
        ws_emp.write_string_with_format(
            0,
            0,
            "Reporte de Liquidaciones - Totales por Empleado (Todos los Registros de Obra) - CONSTRUAUDIT",
            &title_format,
        ).map_err(|e| e.to_string())?;

        ws_emp.write_string_with_format(
            1,
            0,
            &format!("Empresa: {} | Registro Patronal: {} | Revisión: {}", cliente_nombre, rp_registro, revision_folio),
            &subtitle_format,
        ).map_err(|e| e.to_string())?;

        ws_emp.write_string_with_format(
            2,
            0,
            "Filtro: Consolidados de Registros de Obra de la Revisión | Acumulado de todos los meses de la revisión",
            &subtitle_format,
        ).map_err(|e| e.to_string())?;

        let headers_s3 = [
            "#", "NSS", "RFC", "CURP", "Nombre del Trabajador", "Reg. Patronal",
            "Registros de Obra", "Días Cédula", "Días Obra",
            "Lic.", "Inc.", "Aus.",
            "Cuota Fija", "Exc. Pat.", "Exc. Obr.", "PD Pat.", "PD Obr.", "GMP Pat.", "GMP Obr.",
            "Riesgo Trab.", "IV Pat.", "IV Obr.", "Guard y PS", "Suma Patronal", "Suma Obrera", "Subtotal", "Subtotal Cédula"
        ];

        let header_row_s3 = 4;
        for (col, h) in headers_s3.iter().enumerate() {
            ws_emp.write_string_with_format(header_row_s3, col as u16, *h, &header_format).map_err(|e| e.to_string())?;
        }

        struct AggEmpTotal {
            nss: String,
            rfc: String,
            curp: String,
            nombre_trabajador: String,
            registro_patronal: String,
            obras_set: BTreeSet<String>,
            dias_cedula: i64,
            dias_obra: i64,
            lic: f64,
            inc: f64,
            aus: f64,
            cf: f64,
            exc_pat: f64,
            exc_obr: f64,
            pd_pat: f64,
            pd_obr: f64,
            gmp_pat: f64,
            gmp_obr: f64,
            rt: f64,
            iv_pat: f64,
            iv_obr: f64,
            gps: f64,
            patronal: f64,
            obrera: f64,
            subtotal: f64,
            subtotal_orig: f64,
        }

        let mut groups_s3: HashMap<Uuid, AggEmpTotal> = HashMap::new();

        for r in all_rows {
            if !is_valid_user_obra(r) {
                continue;
            }

            let entry = groups_s3.entry(r.id_empleado).or_insert_with(|| {
                AggEmpTotal {
                    nss: r.nss.clone(),
                    rfc: r.rfc.clone(),
                    curp: r.curp.clone(),
                    nombre_trabajador: r.nombre_trabajador.clone(),
                    registro_patronal: clean_rp_text(&r.registro_patronal).to_string(),
                    obras_set: BTreeSet::new(),
                    dias_cedula: 0,
                    dias_obra: 0,
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
                    patronal: 0.0,
                    obrera: 0.0,
                    subtotal: 0.0,
                    subtotal_orig: 0.0,
                }
            });

            let ob_num = clean_obra_text(&r.obra_numero, &r.obra_label, true);
            if !ob_num.is_empty() {
                entry.obras_set.insert(ob_num.to_string());
            }

            entry.dias_cedula += r.dias_cedula as i64;
            entry.dias_obra += r.dias_obra as i64;
            entry.lic += r.lic;
            entry.inc += r.inc;
            entry.aus += r.aus;
            entry.cf += r.cf;
            entry.exc_pat += r.exc_pat;
            entry.exc_obr += r.exc_obr;
            entry.pd_pat += r.pd_pat;
            entry.pd_obr += r.pd_obr;
            entry.gmp_pat += r.gmp_pat;
            entry.gmp_obr += r.gmp_obr;
            entry.rt += r.rt;
            entry.iv_pat += r.iv_pat;
            entry.iv_obr += r.iv_obr;
            entry.gps += r.gps;
            entry.patronal += r.patronal;
            entry.obrera += r.obrera;
            entry.subtotal += r.subtotal;
            entry.subtotal_orig += r.subtotal_orig;
        }

        let mut list_s3: Vec<AggEmpTotal> = groups_s3.into_values().collect();
        list_s3.sort_by(|a, b| {
            a.nombre_trabajador.to_lowercase()
                .cmp(&b.nombre_trabajador.to_lowercase())
        });

        let mut s3_dias_ced: f64 = 0.0;
        let mut s3_dias_obr: f64 = 0.0;
        let mut s3_lic: f64 = 0.0;
        let mut s3_inc: f64 = 0.0;
        let mut s3_aus: f64 = 0.0;
        let mut s3_cf: f64 = 0.0;
        let mut s3_exc_pat: f64 = 0.0;
        let mut s3_exc_obr: f64 = 0.0;
        let mut s3_pd_pat: f64 = 0.0;
        let mut s3_pd_obr: f64 = 0.0;
        let mut s3_gmp_pat: f64 = 0.0;
        let mut s3_gmp_obr: f64 = 0.0;
        let mut s3_rt: f64 = 0.0;
        let mut s3_iv_pat: f64 = 0.0;
        let mut s3_iv_obr: f64 = 0.0;
        let mut s3_gps: f64 = 0.0;
        let mut s3_patronal: f64 = 0.0;
        let mut s3_obrera: f64 = 0.0;
        let mut s3_subtotal: f64 = 0.0;
        let mut s3_subtotal_orig: f64 = 0.0;

        let mut curr_r_s3 = header_row_s3 + 1;
        for (idx, item) in list_s3.iter().enumerate() {
            let obras_str = item.obras_set.iter().cloned().collect::<Vec<_>>().join(", ");

            ws_emp.write_number_with_format(curr_r_s3, 0, (idx + 1) as f64, &integer_format).map_err(|e| e.to_string())?;
            ws_emp.write_string_with_format(curr_r_s3, 1, &item.nss, &text_format).map_err(|e| e.to_string())?;
            ws_emp.write_string_with_format(curr_r_s3, 2, &item.rfc, &text_format).map_err(|e| e.to_string())?;
            ws_emp.write_string_with_format(curr_r_s3, 3, &item.curp, &text_format).map_err(|e| e.to_string())?;
            ws_emp.write_string_with_format(curr_r_s3, 4, &item.nombre_trabajador, &text_format).map_err(|e| e.to_string())?;
            ws_emp.write_string_with_format(curr_r_s3, 5, &item.registro_patronal, &text_format).map_err(|e| e.to_string())?;
            ws_emp.write_string_with_format(curr_r_s3, 6, &obras_str, &text_format).map_err(|e| e.to_string())?;

            ws_emp.write_number_with_format(curr_r_s3, 7, item.dias_cedula as f64, &integer_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 8, item.dias_obra as f64, &integer_format).map_err(|e| e.to_string())?;

            if item.lic > 0.001 {
                ws_emp.write_number_with_format(curr_r_s3, 9, item.lic, &integer_format).map_err(|e| e.to_string())?;
            } else {
                ws_emp.write_string_with_format(curr_r_s3, 9, "-", &text_format).map_err(|e| e.to_string())?;
            }
            if item.inc > 0.001 {
                ws_emp.write_number_with_format(curr_r_s3, 10, item.inc, &integer_format).map_err(|e| e.to_string())?;
            } else {
                ws_emp.write_string_with_format(curr_r_s3, 10, "-", &text_format).map_err(|e| e.to_string())?;
            }
            if item.aus > 0.001 {
                ws_emp.write_number_with_format(curr_r_s3, 11, item.aus, &integer_format).map_err(|e| e.to_string())?;
            } else {
                ws_emp.write_string_with_format(curr_r_s3, 11, "-", &text_format).map_err(|e| e.to_string())?;
            }

            ws_emp.write_number_with_format(curr_r_s3, 12, item.cf, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 13, item.exc_pat, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 14, item.exc_obr, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 15, item.pd_pat, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 16, item.pd_obr, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 17, item.gmp_pat, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 18, item.gmp_obr, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 19, item.rt, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 20, item.iv_pat, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 21, item.iv_obr, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 22, item.gps, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 23, item.patronal, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 24, item.obrera, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 25, item.subtotal, &currency_format).map_err(|e| e.to_string())?;
            ws_emp.write_number_with_format(curr_r_s3, 26, item.subtotal_orig, &currency_format).map_err(|e| e.to_string())?;

            s3_dias_ced += item.dias_cedula as f64;
            s3_dias_obr += item.dias_obra as f64;
            s3_lic += item.lic;
            s3_inc += item.inc;
            s3_aus += item.aus;
            s3_cf += item.cf;
            s3_exc_pat += item.exc_pat;
            s3_exc_obr += item.exc_obr;
            s3_pd_pat += item.pd_pat;
            s3_pd_obr += item.pd_obr;
            s3_gmp_pat += item.gmp_pat;
            s3_gmp_obr += item.gmp_obr;
            s3_rt += item.rt;
            s3_iv_pat += item.iv_pat;
            s3_iv_obr += item.iv_obr;
            s3_gps += item.gps;
            s3_patronal += item.patronal;
            s3_obrera += item.obrera;
            s3_subtotal += item.subtotal;
            s3_subtotal_orig += item.subtotal_orig;

            curr_r_s3 += 1;
        }

        // Fila Total General Sheet 3
        for c in 0..=3 {
            ws_emp.write_string_with_format(curr_r_s3, c as u16, "", &grand_total_label_format).map_err(|e| e.to_string())?;
        }
        ws_emp.write_string_with_format(curr_r_s3, 4, "TOTAL GENERAL", &grand_total_label_format).map_err(|e| e.to_string())?;
        for c in 5..=6 {
            ws_emp.write_string_with_format(curr_r_s3, c as u16, "", &grand_total_label_format).map_err(|e| e.to_string())?;
        }

        ws_emp.write_number_with_format(curr_r_s3, 7, s3_dias_ced, &grand_total_int_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 8, s3_dias_obr, &grand_total_int_format).map_err(|e| e.to_string())?;

        if s3_lic > 0.001 {
            ws_emp.write_number_with_format(curr_r_s3, 9, s3_lic, &grand_total_int_format).map_err(|e| e.to_string())?;
        } else {
            ws_emp.write_string_with_format(curr_r_s3, 9, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
        }
        if s3_inc > 0.001 {
            ws_emp.write_number_with_format(curr_r_s3, 10, s3_inc, &grand_total_int_format).map_err(|e| e.to_string())?;
        } else {
            ws_emp.write_string_with_format(curr_r_s3, 10, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
        }
        if s3_aus > 0.001 {
            ws_emp.write_number_with_format(curr_r_s3, 11, s3_aus, &grand_total_int_format).map_err(|e| e.to_string())?;
        } else {
            ws_emp.write_string_with_format(curr_r_s3, 11, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
        }

        ws_emp.write_number_with_format(curr_r_s3, 12, s3_cf, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 13, s3_exc_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 14, s3_exc_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 15, s3_pd_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 16, s3_pd_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 17, s3_gmp_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 18, s3_gmp_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 19, s3_rt, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 20, s3_iv_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 21, s3_iv_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 22, s3_gps, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 23, s3_patronal, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 24, s3_obrera, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 25, s3_subtotal, &grand_total_curr_format).map_err(|e| e.to_string())?;
        ws_emp.write_number_with_format(curr_r_s3, 26, s3_subtotal_orig, &grand_total_curr_format).map_err(|e| e.to_string())?;

        ws_emp.autofit();
        configurar_impresion(ws_emp, header_row_s3, 26)?;
    }

    // =========================================================================
    // SHEET 4: Análisis de Guarderías (Tope 18% por Registro de Obra)
    // =========================================================================
    if !guarderias_kpis.is_empty() {
        let ws_guard = workbook.add_worksheet();
        ws_guard.set_name("Análisis de Guarderías (18%)").map_err(|e| e.to_string())?;
        ws_guard.set_header(&print_header_str);
        ws_guard.set_footer(&print_footer_str);
        ws_guard.set_header_footer_align_with_page(true);

        ws_guard.write_string_with_format(
            0,
            0,
            "Reporte de Liquidaciones - Análisis de Guarderías (Tope 18%) - CONSTRUAUDIT",
            &title_format,
        ).map_err(|e| e.to_string())?;

        ws_guard.write_string_with_format(
            1,
            0,
            &format!("Empresa: {} | Registro Patronal: {} | Revisión: {}", cliente_nombre, rp_registro, revision_folio),
            &subtitle_format,
        ).map_err(|e| e.to_string())?;

        ws_guard.write_string_with_format(
            2,
            0,
            "Fórmula en vivo: Valor Guarderías = Total GPS / 0.01 (1%) | Límite = 18% del Valor de la Obra",
            &subtitle_format,
        ).map_err(|e| e.to_string())?;

        let kpi_tbl_hdr_format = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_background_color(Color::RGB(0x1E3A8A))
            .set_font_color(Color::RGB(0xFFFFFF))
            .set_border(FormatBorder::Thin);

        let kpi_warn_curr_format = Format::new()
            .set_bold()
            .set_num_format("$#,##0.00")
            .set_font_size(10)
            .set_background_color(Color::RGB(0xFEF9C3))
            .set_font_color(Color::RGB(0x854D0E))
            .set_border(FormatBorder::Thin);

        let kpi_green_curr_format = Format::new()
            .set_bold()
            .set_num_format("$#,##0.00")
            .set_font_size(10)
            .set_background_color(Color::RGB(0xF0FDF4))
            .set_font_color(Color::RGB(0x15803D))
            .set_border(FormatBorder::Thin);

        let kpi_blue_curr_format = Format::new()
            .set_bold()
            .set_num_format("$#,##0.00")
            .set_font_size(10)
            .set_font_color(Color::RGB(0x2563EB))
            .set_border(FormatBorder::Thin);

        let kpi_status_ok_format = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_background_color(Color::RGB(0xDCFCE7))
            .set_font_color(Color::RGB(0x166534))
            .set_border(FormatBorder::Thin);

        let kpi_status_warn_format = Format::new()
            .set_bold()
            .set_font_size(10)
            .set_background_color(Color::RGB(0xFEF08A))
            .set_font_color(Color::RGB(0x854D0E))
            .set_border(FormatBorder::Thin);

        let kpi_headers = [
            "#",
            "Registro de Obra",
            "Valor Guarderías (1% GPS)",
            "Valor Total de la Obra",
            "Límite 18% de la Obra",
            "Total Cuotas GPS (IMSS)",
            "Registros Vinculados",
            "Estatus (Tope 18%)",
        ];

        let header_row_s4 = 4;
        for (col, h) in kpi_headers.iter().enumerate() {
            ws_guard.write_string_with_format(header_row_s4, col as u16, *h, &kpi_tbl_hdr_format)
                .map_err(|e| e.to_string())?;
        }

        let mut curr_r_s4 = header_row_s4;
        let mut tot_val_guard = 0.0;
        let mut tot_monto = 0.0;
        let mut tot_lim = 0.0;
        let mut tot_g = 0.0;
        let mut tot_regs = 0;
        let mut hay_alerta_s4 = false;

        for (idx, kpi) in guarderias_kpis.iter().enumerate() {
            curr_r_s4 += 1;
            ws_guard.write_number_with_format(curr_r_s4, 0, (idx + 1) as f64, &integer_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_string_with_format(curr_r_s4, 1, &kpi.obra_label, &text_format)
                .map_err(|e| e.to_string())?;

            if kpi.supera_18 {
                ws_guard.write_number_with_format(curr_r_s4, 2, kpi.valor_guarderias, &kpi_warn_curr_format)
                    .map_err(|e| e.to_string())?;
                hay_alerta_s4 = true;
            } else {
                ws_guard.write_number_with_format(curr_r_s4, 2, kpi.valor_guarderias, &currency_format)
                    .map_err(|e| e.to_string())?;
            }

            if kpi.sin_monto {
                ws_guard.write_string_with_format(curr_r_s4, 3, "N/D", &text_format)
                    .map_err(|e| e.to_string())?;
                ws_guard.write_string_with_format(curr_r_s4, 4, "N/D", &text_format)
                    .map_err(|e| e.to_string())?;
            } else {
                ws_guard.write_number_with_format(curr_r_s4, 3, kpi.monto_obra, &currency_format)
                    .map_err(|e| e.to_string())?;
                ws_guard.write_number_with_format(curr_r_s4, 4, kpi.limite_18_obra, &kpi_green_curr_format)
                    .map_err(|e| e.to_string())?;
            }
            ws_guard.write_number_with_format(curr_r_s4, 5, kpi.total_gps, &kpi_blue_curr_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 6, kpi.empleados_count as f64, &integer_format)
                .map_err(|e| e.to_string())?;

            if kpi.sin_monto {
                ws_guard.write_string_with_format(curr_r_s4, 7, "⚪ SIN MONTO (N/D)", &text_format)
                    .map_err(|e| e.to_string())?;
            } else if kpi.supera_18 {
                ws_guard.write_string_with_format(curr_r_s4, 7, "⚠️ SUPERA EL 18%", &kpi_status_warn_format)
                    .map_err(|e| e.to_string())?;
            } else {
                ws_guard.write_string_with_format(curr_r_s4, 7, "✅ CUMPLE (<= 18%)", &kpi_status_ok_format)
                    .map_err(|e| e.to_string())?;
            }

            tot_val_guard += kpi.valor_guarderias;
            tot_monto += kpi.monto_obra;
            tot_lim += kpi.limite_18_obra;
            tot_g += kpi.total_gps;
            tot_regs += kpi.empleados_count;
        }

        if guarderias_kpis.len() > 1 {
            curr_r_s4 += 1;
            ws_guard.write_string_with_format(curr_r_s4, 0, "", &grand_total_label_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_string_with_format(curr_r_s4, 1, "TOTAL CONSOLIDADO", &grand_total_label_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 2, tot_val_guard, &grand_total_curr_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 3, tot_monto, &grand_total_curr_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 4, tot_lim, &grand_total_curr_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 5, tot_g, &grand_total_curr_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 6, tot_regs as f64, &grand_total_int_format)
                .map_err(|e| e.to_string())?;
            let tot_status = if hay_alerta_s4 { "⚠️ OBRAS EN ALERTA" } else { "✅ TODAS CUMPLEN" };
            ws_guard.write_string_with_format(curr_r_s4, 7, tot_status, &grand_total_label_format)
                .map_err(|e| e.to_string())?;
        }

        // Tarjetas individuales por obra en Sheet 4
        curr_r_s4 += 3;
        for kpi in guarderias_kpis {
            let card_header_text = format!("🏗️ Registro de Obra: {} ({} registros vinculados)", kpi.obra_label, kpi.empleados_count);
            ws_guard.write_string_with_format(curr_r_s4, 0, &card_header_text, &title_format)
                .map_err(|e| e.to_string())?;

            if kpi.sin_monto {
                curr_r_s4 += 1;
                ws_guard.write_string_with_format(
                    curr_r_s4,
                    0,
                    "Sin monto capturado: no se calcula el límite del 18%",
                    &text_format,
                ).map_err(|e| e.to_string())?;
            }

            if kpi.supera_18 {
                curr_r_s4 += 1;
                ws_guard.write_string_with_format(
                    curr_r_s4,
                    0,
                    "⚠️ ADVERTENCIA: Guarderías supera el 18% del valor de la obra",
                    &kpi_status_warn_format,
                ).map_err(|e| e.to_string())?;
            }

            curr_r_s4 += 1;
            ws_guard.write_string_with_format(curr_r_s4, 0, "VALOR GUARDERÍAS (1% GPS)", &header_format).map_err(|e| e.to_string())?;
            ws_guard.write_string_with_format(curr_r_s4, 1, "VALOR TOTAL DE LA OBRA", &header_format).map_err(|e| e.to_string())?;
            ws_guard.write_string_with_format(curr_r_s4, 2, "LÍMITE 18% DE LA OBRA", &header_format).map_err(|e| e.to_string())?;
            ws_guard.write_string_with_format(curr_r_s4, 3, "TOTAL CUOTAS GPS (IMSS)", &header_format).map_err(|e| e.to_string())?;

            curr_r_s4 += 1;
            if kpi.supera_18 {
                ws_guard.write_number_with_format(curr_r_s4, 0, kpi.valor_guarderias, &kpi_warn_curr_format).map_err(|e| e.to_string())?;
            } else {
                ws_guard.write_number_with_format(curr_r_s4, 0, kpi.valor_guarderias, &currency_format).map_err(|e| e.to_string())?;
            }
            if kpi.sin_monto {
                ws_guard.write_string_with_format(curr_r_s4, 1, "N/D", &text_format).map_err(|e| e.to_string())?;
                ws_guard.write_string_with_format(curr_r_s4, 2, "N/D", &text_format).map_err(|e| e.to_string())?;
            } else {
                ws_guard.write_number_with_format(curr_r_s4, 1, kpi.monto_obra, &currency_format).map_err(|e| e.to_string())?;
                ws_guard.write_number_with_format(curr_r_s4, 2, kpi.limite_18_obra, &kpi_green_curr_format).map_err(|e| e.to_string())?;
            }
            ws_guard.write_number_with_format(curr_r_s4, 3, kpi.total_gps, &kpi_blue_curr_format).map_err(|e| e.to_string())?;

            curr_r_s4 += 2;
        }

        ws_guard.autofit();
        configurar_impresion(ws_guard, header_row_s4, 7)?;
    }

    // Entregable: hojas protegidas sin contraseña; selección y autofiltro permitidos
    if matches!(modo, ModoEntregable::Entregable { .. }) {
        let opciones_proteccion = ProtectionOptions {
            select_locked_cells: true,
            select_unlocked_cells: true,
            use_autofilter: true,
            ..ProtectionOptions::new()
        };
        for ws in workbook.worksheets_mut() {
            ws.protect_with_options(&opciones_proteccion);
        }
    }

    workbook.save(file_path).map_err(|e| format!("Error guardando archivo Excel: {}", e))?;
    Ok(())
}

/// Parsea el mes (1..=12) y el año a partir de `mes_str`, `anio` y/o `periodo_str`.
pub fn parse_month_and_year(mes_str: &str, anio: i16, periodo_str: &str) -> Option<(u32, i32)> {
    let mut month = month_to_num(mes_str);
    let mut year = anio as i32;

    if (month == 0 || year <= 1900) && !periodo_str.is_empty() {
        let parts: Vec<&str> = periodo_str
            .split(|c| c == '/' || c == '-' || c == ' ')
            .filter(|s| !s.trim().is_empty())
            .collect();
        for part in &parts {
            let p_trim = part.trim();
            if let Ok(y) = p_trim.parse::<i32>() {
                if (1900..=2100).contains(&y) {
                    year = y;
                }
            }
            if month == 0 {
                let m = month_to_num(p_trim);
                if (1..=12).contains(&m) {
                    month = m;
                }
            }
        }
    }

    if (1..=12).contains(&month) && year >= 1900 {
        Some((month, year))
    } else {
        None
    }
}

/// Obtiene el rango de fechas (primer día, último día) y número total de días de un mes dado.
pub fn get_month_date_range(month: u32, year: i32) -> Option<(NaiveDate, NaiveDate, i16)> {
    let start_date = NaiveDate::from_ymd_opt(year, month, 1)?;
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_month_start = NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    let end_date = next_month_start.pred_opt()?;
    let days_in_month = (end_date - start_date).num_days() as i16 + 1;
    Some((start_date, end_date, days_in_month))
}

/// Calcula el número de días que una obra estuvo activa dentro de un mes específico,
/// basándose en su fecha de inicio y fecha de fin.
/// Devuelve `None` si la obra no tiene restricción de fechas (ambas fechas `None`).
pub fn calculate_obra_days_in_month(
    month: u32,
    year: i32,
    obra_fecha_inicio: Option<NaiveDate>,
    obra_fecha_fin: Option<NaiveDate>,
) -> Option<i16> {
    let (month_start, month_end, _) = get_month_date_range(month, year)?;

    match (obra_fecha_inicio, obra_fecha_fin) {
        (Some(f_ini), Some(f_fin)) => {
            let active_start = f_ini.max(month_start);
            let active_end = f_fin.min(month_end);
            if active_start <= active_end {
                Some(((active_end - active_start).num_days() as i16 + 1).max(0))
            } else {
                Some(0)
            }
        }
        (Some(f_ini), None) => {
            let active_start = f_ini.max(month_start);
            if active_start <= month_end {
                Some(((month_end - active_start).num_days() as i16 + 1).max(0))
            } else {
                Some(0)
            }
        }
        (None, Some(f_fin)) => {
            let active_end = f_fin.min(month_end);
            if active_end >= month_start {
                Some(((active_end - month_start).num_days() as i16 + 1).max(0))
            } else {
                Some(0)
            }
        }
        (None, None) => None,
    }
}

/// Extrae las fechas de movimiento de un empleado en un mes específico (Baja, Alta/Reingreso).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MovementKind {
    Baja,
    Alta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmployeeMovement {
    pub kind: MovementKind,
    pub fecha: NaiveDate,
}

/// Extrae todos los movimientos cronológicos de un empleado en un mes específico (Alta, Baja, Reingreso, etc.).
/// Detecta claves como "baja", "alta", "reing" en el registro actual y en registros hermanos en el mismo mes y año.
pub fn extract_all_employee_movements(
    id_empleado: Uuid,
    mes_periodo: &str,
    anio_periodo: i16,
    row_clave: &str,
    row_fecha: Option<NaiveDate>,
    all_rows: &[ReporteLiquidacionRowDb],
) -> Vec<EmployeeMovement> {
    let mut movements = Vec::new();

    let is_baja = |c: &str| c.trim().to_lowercase().contains("baja");
    let is_alta_reing = |c: &str| {
        let s = c.trim().to_lowercase();
        s.contains("alta") || s.contains("reing")
    };

    if let Some(f) = row_fecha {
        if is_baja(row_clave) {
            movements.push(EmployeeMovement { kind: MovementKind::Baja, fecha: f });
        } else if is_alta_reing(row_clave) {
            movements.push(EmployeeMovement { kind: MovementKind::Alta, fecha: f });
        }
    }

    for other in all_rows {
        if other.id_empleado == id_empleado
            && other.anio_periodo == anio_periodo
            && other.mes_periodo.trim().eq_ignore_ascii_case(mes_periodo.trim())
        {
            if let Some(f) = other.fecha {
                if is_baja(&other.clave) {
                    movements.push(EmployeeMovement { kind: MovementKind::Baja, fecha: f });
                } else if is_alta_reing(&other.clave) {
                    movements.push(EmployeeMovement { kind: MovementKind::Alta, fecha: f });
                }
            }
        }
    }

    movements.sort_by(|a, b| {
        a.fecha.cmp(&b.fecha).then_with(|| a.kind.cmp(&b.kind))
    });
    movements.dedup();
    movements
}

/// Extrae las fechas de movimiento de un empleado en un mes específico (Baja, Alta/Reingreso).
/// Wrapper de compatibilidad que retorna (primera_baja, ultima_alta).
pub fn extract_employee_movements(
    id_empleado: Uuid,
    mes_periodo: &str,
    anio_periodo: i16,
    row_clave: &str,
    row_fecha: Option<NaiveDate>,
    all_rows: &[ReporteLiquidacionRowDb],
) -> (Option<NaiveDate>, Option<NaiveDate>) {
    let movs = extract_all_employee_movements(id_empleado, mes_periodo, anio_periodo, row_clave, row_fecha, all_rows);
    let mut baja_fecha: Option<NaiveDate> = None;
    let mut alta_fecha: Option<NaiveDate> = None;
    for m in movs {
        match m.kind {
            MovementKind::Baja => {
                baja_fecha = match baja_fecha {
                    Some(cur) => Some(cur.min(m.fecha)),
                    None => Some(m.fecha),
                };
            }
            MovementKind::Alta => {
                alta_fecha = match alta_fecha {
                    Some(cur) => Some(cur.max(m.fecha)),
                    None => Some(m.fecha),
                };
            }
        }
    }
    (baja_fecha, alta_fecha)
}

/// Construye un mapa eficiente con todos los movimientos ordenados por (id_empleado, mes_periodo_normalizado, anio_periodo).
pub fn build_all_employee_movements_map(
    all_rows: &[ReporteLiquidacionRowDb],
) -> HashMap<(Uuid, String, i16), Vec<EmployeeMovement>> {
    let mut map: HashMap<(Uuid, String, i16), Vec<EmployeeMovement>> = HashMap::new();
    let is_baja = |c: &str| c.trim().to_lowercase().contains("baja");
    let is_alta_reing = |c: &str| {
        let s = c.trim().to_lowercase();
        s.contains("alta") || s.contains("reing")
    };

    for r in all_rows {
        if let Some(f) = r.fecha {
            let key = (r.id_empleado, r.mes_periodo.trim().to_uppercase(), r.anio_periodo);
            let entry = map.entry(key).or_default();
            if is_baja(&r.clave) {
                entry.push(EmployeeMovement { kind: MovementKind::Baja, fecha: f });
            } else if is_alta_reing(&r.clave) {
                entry.push(EmployeeMovement { kind: MovementKind::Alta, fecha: f });
            }
        }
    }

    for entry in map.values_mut() {
        entry.sort_by(|a, b| {
            a.fecha.cmp(&b.fecha).then_with(|| a.kind.cmp(&b.kind))
        });
        entry.dedup();
    }

    map
}

/// Construye un mapa con los movimientos (Baja, Alta/Reingreso)
/// indexado por (id_empleado, mes_periodo_normalizado, anio_periodo).
pub fn build_employee_movements_map(
    all_rows: &[ReporteLiquidacionRowDb],
) -> HashMap<(Uuid, String, i16), (Option<NaiveDate>, Option<NaiveDate>)> {
    let full_map = build_all_employee_movements_map(all_rows);
    let mut map = HashMap::new();
    for (k, movs) in full_map {
        let mut baja: Option<NaiveDate> = None;
        let mut alta: Option<NaiveDate> = None;
        for m in movs {
            match m.kind {
                MovementKind::Baja => {
                    baja = match baja {
                        Some(cur) => Some(cur.min(m.fecha)),
                        None => Some(m.fecha),
                    };
                }
                MovementKind::Alta => {
                    alta = match alta {
                        Some(cur) => Some(cur.max(m.fecha)),
                        None => Some(m.fecha),
                    };
                }
            }
        }
        map.insert(k, (baja, alta));
    }
    map
}

/// Calcula los intervalos activos [inicio, fin] de un empleado en un mes según todos sus movimientos.
/// Maneja 1 o múltiples movimientos continuos o alternados (Alta -> Baja -> Reing -> Baja...).
pub fn calculate_employee_active_intervals(
    month_start: NaiveDate,
    month_end: NaiveDate,
    movements: &[EmployeeMovement],
) -> Vec<(NaiveDate, NaiveDate)> {
    if movements.is_empty() {
        return vec![(month_start, month_end)];
    }

    let mut intervals = Vec::new();
    // Si el primer movimiento registrado en el mes es Baja, el trabajador ya estaba activo desde el inicio del mes.
    // Si es Alta o Reingreso, el trabajador ingresó en esa fecha.
    let mut current_start = if movements[0].kind == MovementKind::Baja {
        Some(month_start)
    } else {
        None
    };

    for m in movements {
        match m.kind {
            MovementKind::Baja => {
                if let Some(start) = current_start {
                    let end = m.fecha.min(month_end);
                    if end >= start {
                        intervals.push((start, end));
                    }
                    current_start = None;
                }
            }
            MovementKind::Alta => {
                if current_start.is_none() {
                    let start = m.fecha.max(month_start);
                    if start <= month_end {
                        current_start = Some(start);
                    }
                }
            }
        }
    }

    if let Some(start) = current_start {
        if month_end >= start {
            intervals.push((start, month_end));
        }
    }

    intervals
}

/// Valida si una obra puede asignarse a un empleado en un mes específico considerando todos sus movimientos.
pub fn validate_obra_assignment_for_employee_movements(
    obra_fecha_inicio: Option<NaiveDate>,
    obra_fecha_fin: Option<NaiveDate>,
    movements: &[EmployeeMovement],
    mes_str: &str,
    anio: i16,
    periodo_str: &str,
) -> Result<(), String> {
    let (m, y) = match parse_month_and_year(mes_str, anio, periodo_str) {
        Some((m, y)) => (m, y),
        None => return Ok(()),
    };

    let (month_start, month_end, _) = match get_month_date_range(m, y as i32) {
        Some(r) => r,
        None => return Ok(()),
    };

    let active_intervals = calculate_employee_active_intervals(month_start, month_end, movements);
    if active_intervals.is_empty() {
        return Err("El trabajador no registra actividad en este mes.".to_string());
    }

    let has_obra_restriction = obra_fecha_inicio.is_some() || obra_fecha_fin.is_some();
    if has_obra_restriction {
        let obra_start = obra_fecha_inicio.unwrap_or(month_start).max(month_start);
        let obra_end = obra_fecha_fin.unwrap_or(month_end).min(month_end);

        if obra_start > month_end || obra_end < month_start || obra_start > obra_end {
            return Err("Las fechas de la obra no tienen vigencia en este mes.".to_string());
        }

        let mut overlap_days = 0i16;
        for (act_start, act_end) in &active_intervals {
            let s = (*act_start).max(obra_start);
            let e = (*act_end).min(obra_end);
            if s <= e {
                overlap_days += ((e - s).num_days() as i16 + 1).max(0);
            }
        }

        if overlap_days == 0 {
            return Err("Las fechas de la obra no coinciden con los periodos activos del trabajador en este mes.".to_string());
        }
    }

    Ok(())
}

/// Valida si una obra puede asignarse a un empleado en un mes específico (wrapper de compatibilidad).
pub fn validate_obra_assignment_for_employee(
    obra_fecha_inicio: Option<NaiveDate>,
    obra_fecha_fin: Option<NaiveDate>,
    baja_fecha: Option<NaiveDate>,
    alta_fecha: Option<NaiveDate>,
    mes_str: &str,
    anio: i16,
    periodo_str: &str,
) -> Result<(), String> {
    let mut movs = Vec::new();
    if let Some(fb) = baja_fecha {
        movs.push(EmployeeMovement { kind: MovementKind::Baja, fecha: fb });
    }
    if let Some(fa) = alta_fecha {
        movs.push(EmployeeMovement { kind: MovementKind::Alta, fecha: fa });
    }
    movs.sort_by(|a, b| a.fecha.cmp(&b.fecha).then_with(|| a.kind.cmp(&b.kind)));
    validate_obra_assignment_for_employee_movements(
        obra_fecha_inicio,
        obra_fecha_fin,
        &movs,
        mes_str,
        anio,
        periodo_str,
    )
}

/// Calcula el número máximo de días de obra que un empleado puede tener en un periodo específico considerando todos sus movimientos.
pub fn calculate_max_dias_obra_for_employee_movements(
    dias_cedula: i16,
    lic: f64,
    inc: f64,
    aus: f64,
    mes_str: &str,
    anio: i16,
    periodo_str: &str,
    obra_fecha_inicio: Option<NaiveDate>,
    obra_fecha_fin: Option<NaiveDate>,
    movements: &[EmployeeMovement],
) -> (i16, Option<i16>) {
    let dias_efectivos = crate::db::prorrateo::dias_efectivos(i32::from(dias_cedula), lic, inc, aus) as i16;

    let (month, year) = match parse_month_and_year(mes_str, anio, periodo_str) {
        Some((m, y)) => (m, y),
        None => return (dias_efectivos, None),
    };

    let (month_start, month_end, _) = match get_month_date_range(month, year as i32) {
        Some(r) => r,
        None => return (dias_efectivos, None),
    };

    let active_intervals = calculate_employee_active_intervals(month_start, month_end, movements);
    if active_intervals.is_empty() {
        return (0, Some(0));
    }

    let has_obra_restriction = obra_fecha_inicio.is_some() || obra_fecha_fin.is_some();
    let has_emp_restriction = !movements.is_empty();

    let obra_start = obra_fecha_inicio.unwrap_or(month_start).max(month_start);
    let obra_end = obra_fecha_fin.unwrap_or(month_end).min(month_end);

    if has_obra_restriction && (obra_start > month_end || obra_end < month_start || obra_start > obra_end) {
        return (0, Some(0));
    }

    let mut overlap_days = 0i16;
    for (act_start, act_end) in &active_intervals {
        let s = (*act_start).max(obra_start);
        let e = (*act_end).min(obra_end);
        if s <= e {
            overlap_days += ((e - s).num_days() as i16 + 1).max(0);
        }
    }

    let dias_obra_en_mes = if has_obra_restriction || has_emp_restriction {
        Some(overlap_days)
    } else {
        None
    };

    let max_dias = if let Some(d_obra) = dias_obra_en_mes {
        dias_efectivos.min(d_obra)
    } else {
        dias_efectivos
    };

    (max_dias, dias_obra_en_mes)
}

/// Calcula el número máximo de días de obra que un empleado puede tener en un periodo específico (wrapper de compatibilidad):
/// Retorna `(max_dias, dias_obra_activos_en_el_mes)`.
pub fn calculate_max_dias_obra_for_employee(
    dias_cedula: i16,
    lic: f64,
    inc: f64,
    aus: f64,
    mes_str: &str,
    anio: i16,
    periodo_str: &str,
    obra_fecha_inicio: Option<NaiveDate>,
    obra_fecha_fin: Option<NaiveDate>,
    baja_fecha: Option<NaiveDate>,
    alta_fecha: Option<NaiveDate>,
) -> (i16, Option<i16>) {
    let mut movs = Vec::new();
    if let Some(fb) = baja_fecha {
        movs.push(EmployeeMovement { kind: MovementKind::Baja, fecha: fb });
    }
    if let Some(fa) = alta_fecha {
        movs.push(EmployeeMovement { kind: MovementKind::Alta, fecha: fa });
    }
    movs.sort_by(|a, b| a.fecha.cmp(&b.fecha).then_with(|| a.kind.cmp(&b.kind)));
    calculate_max_dias_obra_for_employee_movements(
        dias_cedula,
        lic,
        inc,
        aus,
        mes_str,
        anio,
        periodo_str,
        obra_fecha_inicio,
        obra_fecha_fin,
        &movs,
    )
}

#[derive(Debug, Clone, Default)]
pub struct AutoCorrectionStats {
    pub total_analizados: usize,
    pub corregidos_dias_excedidos: usize,
    pub corregidos_dias_faltantes: usize,
    pub corregidos_bajas_cero: usize,
    pub obras_auto_vinculadas: usize,
    pub total_corregidos: usize,
    pub resumen: String,
}

/// Verifica y corrige automáticamente los días y vinculaciones de obras para todos los empleados de la revisión:
/// 1. Asegura que renglones de BAJA con días cédula 0 tengan días de obra en 0 y cuotas en $0.
/// 2. Corrige topes a nivel de renglón si días_registro_obra excede los días de cédula de dicho renglón (ej. 27 en renglón de 23 o 4).
/// 3. Identifica si la suma total de días asignados al empleado en el mes es menor a sus días efectivos de cédula
///    (ej. asignado 15 + 7 = 22 días cuando la cédula es de 31 o 23 días).
/// 4. Vincula automáticamente la(s) obra(s) activa(s) faltante(s) de la revisión en ese periodo (ej. C0640943) para completar los días.
/// 5. Recalcula el prorrateo financiero de manera exacta proporcional a los días asignados.
pub fn verify_and_autocorrect_employee_obras(
    client: &mut Client,
    id_revision: Uuid,
) -> Result<AutoCorrectionStats, String> {
    let mut stats = AutoCorrectionStats::default();

    let obras = fetch_obras_for_revision(client, id_revision)?;
    if obras.is_empty() {
        stats.resumen = "No hay registros de obras válidos en esta revisión para verificar.".to_string();
        return Ok(stats);
    }

    let (opt_rev_uuid, _canonical_rp_id, _clean_reg, all_rp_uuids) = match resolve_rp_context(client, id_revision) {
        Ok(res) => res,
        Err(_) => return Ok(stats),
    };

    let parent_rows = client.query(
        r#"
        SELECT rl.id, rl.id_liquidacion, rl.id_empleado, rl.mes_periodo, rl.anio_periodo, e.nombre
        FROM public.reportes_liquidaciones rl
        JOIN public.empleados e ON e.id = rl.id_empleado
        WHERE rl.id_registro_patronal = ANY($1)
          AND ($2::uuid IS NULL OR rl.id_revision = $2)
        ORDER BY rl.anio_periodo ASC, COALESCE(rl.mes_periodo_int, 99) ASC, rl.mes_periodo ASC, e.nombre ASC
        "#,
        &[&all_rp_uuids, &opt_rev_uuid],
    ).map_err(|e| format!("Error consultando reportes_liquidaciones: {}", e))?;

    stats.total_analizados = parent_rows.len();

    let mut tx = client.transaction().map_err(|e| format!("Error iniciando tx: {}", e))?;

    for p in &parent_rows {
        let parent_id: Uuid = p.get(0);
        let id_liq: Uuid = p.get(1);
        let mes_periodo: String = p.get(3);
        let anio_periodo: i16 = p.get(4);
        let emp_nombre: String = p.get(5);

        let periodo_str = format!("{}/{}", mes_periodo, anio_periodo);
        let (month, year) = match parse_month_and_year(&mes_periodo, anio_periodo, &periodo_str) {
            Some(pair) => pair,
            None => continue,
        };

        let (_month_start, _month_end, _days_in_month) = match get_month_date_range(month, year as i32) {
            Some(r) => r,
            None => continue,
        };

        let mut det_rows = tx.query(
            r#"
            SELECT rdl.id, rdl.clave, rdl.fecha, rdl.dias, rdl.id_registro_obra,
                   COALESCE(rdl.dias_registro_obra, 0)::smallint,
                   COALESCE(rdl.sdi, 0.0)::float8,
                   COALESCE(rdl.lic, 0.0)::float8,
                   COALESCE(rdl.inc, 0.0)::float8,
                   COALESCE(rdl.aus, 0.0)::float8,
                   COALESCE(dl.cf, rdl.cf, 0.0)::float8,
                   COALESCE(dl.exc_pat, rdl.exc_pat, 0.0)::float8,
                   COALESCE(dl.exc_obr, rdl.exc_obr, 0.0)::float8,
                   COALESCE(dl.pd_pat, rdl.pd_pat, 0.0)::float8,
                   COALESCE(dl.pd_obr, rdl.pd_obr, 0.0)::float8,
                   COALESCE(dl.gmp_pat, rdl.gmp_pat, 0.0)::float8,
                   COALESCE(dl.gmp_obr, rdl.gmp_obr, 0.0)::float8,
                   COALESCE(dl.rt, rdl.rt, 0.0)::float8,
                   COALESCE(dl.iv_pat, rdl.iv_pat, 0.0)::float8,
                   COALESCE(dl.iv_obr, rdl.iv_obr, 0.0)::float8,
                   COALESCE(dl.gps, rdl.gps, 0.0)::float8,
                   COALESCE(dl.patronal, rdl.patronal, 0.0)::float8,
                   COALESCE(dl.obrera, rdl.obrera, 0.0)::float8,
                   COALESCE(dl.subtotal, rdl.subtotal, 0.0)::float8
            FROM public.reportes_detalles_liquidaciones rdl
            LEFT JOIN public.detalles_liquidaciones dl
                ON dl.id_liquidacion = $2
               AND (COALESCE(TRIM(LOWER(dl.clave)), '') = COALESCE(TRIM(LOWER(rdl.clave)), ''))
               AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL))
            WHERE rdl.id_reporte_liquidacion = $1
            ORDER BY rdl.id ASC
            "#,
            &[&parent_id, &id_liq],
        ).map_err(|e| format!("Error consultando detalles para {}: {}", emp_nombre, e))?;

        if det_rows.is_empty() {
            continue;
        }

        // Paso A: Corregir bajas y topes individuales
        for r in &det_rows {
            let det_id: Uuid = r.get(0);
            let clave_opt: Option<String> = r.get(1);
            let dias_ced: Option<i16> = r.get(3);
            let cur_dias_ro: i16 = r.get(5);
            let is_baja = clave_opt.as_deref().map(|c| c.trim().to_lowercase().contains("baja")).unwrap_or(false);

            if is_baja && dias_ced.unwrap_or(0) == 0 && cur_dias_ro > 0 {
                tx.execute(
                    r#"
                    UPDATE public.reportes_detalles_liquidaciones
                    SET dias_registro_obra = 0,
                        cf = 0, exc_pat = 0, exc_obr = 0, pd_pat = 0, pd_obr = 0,
                        gmp_pat = 0, gmp_obr = 0, rt = 0, iv_pat = 0, iv_obr = 0,
                        gps = 0, patronal = 0, obrera = 0, subtotal = 0
                    WHERE id = $1
                    "#,
                    &[&det_id],
                ).map_err(|e| format!("Error corrigiendo baja para {}: {}", emp_nombre, e))?;
                stats.corregidos_bajas_cero += 1;
                stats.total_corregidos += 1;
            } else if let Some(dc) = dias_ced {
                if dc > 0 && cur_dias_ro > dc {
                    let new_dro = dc;
                    let cf_orig: f64 = r.get(10);
                    let exc_pat_orig: f64 = r.get(11);
                    let exc_obr_orig: f64 = r.get(12);
                    let pd_pat_orig: f64 = r.get(13);
                    let pd_obr_orig: f64 = r.get(14);
                    let gmp_pat_orig: f64 = r.get(15);
                    let gmp_obr_orig: f64 = r.get(16);
                    let rt_orig: f64 = r.get(17);
                    let iv_pat_orig: f64 = r.get(18);
                    let iv_obr_orig: f64 = r.get(19);
                    let gps_orig: f64 = r.get(20);
                    let patronal_orig: f64 = r.get(21);
                    let obrera_orig: f64 = r.get(22);
                    let subtotal_orig: f64 = r.get(23);

                    let factor = (new_dro as f64 / dc as f64).clamp(0.0, 10.0);
                    let round2 = |v: f64| ((v * factor * 100.0).round()) / 100.0;

                    tx.execute(
                        r#"
                        UPDATE public.reportes_detalles_liquidaciones
                        SET dias_registro_obra = $2,
                            cf = $3, exc_pat = $4, exc_obr = $5, pd_pat = $6, pd_obr = $7,
                            gmp_pat = $8, gmp_obr = $9, rt = $10, iv_pat = $11, iv_obr = $12,
                            gps = $13, patronal = $14, obrera = $15, subtotal = $16
                        WHERE id = $1
                        "#,
                        &[
                            &det_id, &new_dro,
                            &round2(cf_orig), &round2(exc_pat_orig), &round2(exc_obr_orig),
                            &round2(pd_pat_orig), &round2(pd_obr_orig), &round2(gmp_pat_orig),
                            &round2(gmp_obr_orig), &round2(rt_orig), &round2(iv_pat_orig),
                            &round2(iv_obr_orig), &round2(gps_orig), &round2(patronal_orig),
                            &round2(obrera_orig), &round2(subtotal_orig),
                        ],
                    ).map_err(|e| format!("Error ajustando días excedidos para {}: {}", emp_nombre, e))?;
                    stats.corregidos_dias_excedidos += 1;
                    stats.total_corregidos += 1;
                }
            }
        }

        // Recargar detalles actualizados
        det_rows = tx.query(
            r#"
            SELECT rdl.id, rdl.clave, rdl.fecha, rdl.dias, rdl.id_registro_obra,
                   COALESCE(rdl.dias_registro_obra, 0)::smallint,
                   COALESCE(rdl.sdi, 0.0)::float8,
                   COALESCE(rdl.lic, 0.0)::float8,
                   COALESCE(rdl.inc, 0.0)::float8,
                   COALESCE(rdl.aus, 0.0)::float8,
                   COALESCE(dl.cf, rdl.cf, 0.0)::float8,
                   COALESCE(dl.exc_pat, rdl.exc_pat, 0.0)::float8,
                   COALESCE(dl.exc_obr, rdl.exc_obr, 0.0)::float8,
                   COALESCE(dl.pd_pat, rdl.pd_pat, 0.0)::float8,
                   COALESCE(dl.pd_obr, rdl.pd_obr, 0.0)::float8,
                   COALESCE(dl.gmp_pat, rdl.gmp_pat, 0.0)::float8,
                   COALESCE(dl.gmp_obr, rdl.gmp_obr, 0.0)::float8,
                   COALESCE(dl.rt, rdl.rt, 0.0)::float8,
                   COALESCE(dl.iv_pat, rdl.iv_pat, 0.0)::float8,
                   COALESCE(dl.iv_obr, rdl.iv_obr, 0.0)::float8,
                   COALESCE(dl.gps, rdl.gps, 0.0)::float8,
                   COALESCE(dl.patronal, rdl.patronal, 0.0)::float8,
                   COALESCE(dl.obrera, rdl.obrera, 0.0)::float8,
                   COALESCE(dl.subtotal, rdl.subtotal, 0.0)::float8
            FROM public.reportes_detalles_liquidaciones rdl
            LEFT JOIN public.detalles_liquidaciones dl
                ON dl.id_liquidacion = $2
               AND (COALESCE(TRIM(LOWER(dl.clave)), '') = COALESCE(TRIM(LOWER(rdl.clave)), ''))
               AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL))
            WHERE rdl.id_reporte_liquidacion = $1
            ORDER BY rdl.id ASC
            "#,
            &[&parent_id, &id_liq],
        ).map_err(|e| format!("Error recargando detalles para {}: {}", emp_nombre, e))?;

        let mut seen_movs = std::collections::HashSet::new();
        let mut total_cedula_days: i16 = 0;
        for r in &det_rows {
            let cl: Option<String> = r.get(1);
            let fe: Option<NaiveDate> = r.get(2);
            let is_baja = cl.as_deref().map(|c| c.trim().to_lowercase().contains("baja")).unwrap_or(false);
            if is_baja { continue; }
            let key = (cl, fe);
            if seen_movs.insert(key) {
                total_cedula_days += r.get::<_, Option<i16>>(3).unwrap_or(0);
            }
        }

        let total_obra_days: i16 = det_rows.iter()
            .filter_map(|r| {
                let oid: Option<Uuid> = r.get(4);
                let cl: Option<String> = r.get(1);
                let is_baja = cl.as_deref().map(|c| c.trim().to_lowercase().contains("baja")).unwrap_or(false);
                if is_baja || oid.is_none() { None } else { Some(r.get::<_, i16>(5)) }
            })
            .sum();

        // Paso B: Verificar si faltan días para cubrir la cédula (solo en caso de registro único)
        if total_cedula_days > 0 && total_obra_days < total_cedula_days && det_rows.len() == 1 {
            let missing_days = total_cedula_days - total_obra_days;
            let last_assigned = &det_rows[0];
            let oid: Option<Uuid> = last_assigned.get(4);
            let cl: Option<String> = last_assigned.get(1);
            let is_baja = cl.as_deref().map(|c| c.trim().to_lowercase().contains("baja")).unwrap_or(false);
            if oid.is_some() && !is_baja {
                let last_id: Uuid = last_assigned.get(0);
                let cur_dro: i16 = last_assigned.get(5);
                let dc: i16 = last_assigned.get::<_, Option<i16>>(3).unwrap_or(total_cedula_days);
                let new_dro = (cur_dro + missing_days).min(dc);

                let cf_orig: f64 = last_assigned.get(10);
                let exc_pat_orig: f64 = last_assigned.get(11);
                let exc_obr_orig: f64 = last_assigned.get(12);
                let pd_pat_orig: f64 = last_assigned.get(13);
                let pd_obr_orig: f64 = last_assigned.get(14);
                let gmp_pat_orig: f64 = last_assigned.get(15);
                let gmp_obr_orig: f64 = last_assigned.get(16);
                let rt_orig: f64 = last_assigned.get(17);
                let iv_pat_orig: f64 = last_assigned.get(18);
                let iv_obr_orig: f64 = last_assigned.get(19);
                let gps_orig: f64 = last_assigned.get(20);
                let patronal_orig: f64 = last_assigned.get(21);
                let obrera_orig: f64 = last_assigned.get(22);
                let subtotal_orig: f64 = last_assigned.get(23);

                let factor = (new_dro as f64 / dc.max(1) as f64).clamp(0.0, 10.0);
                let round2 = |v: f64| ((v * factor * 100.0).round()) / 100.0;

                tx.execute(
                    r#"
                    UPDATE public.reportes_detalles_liquidaciones
                    SET dias_registro_obra = $2,
                        cf = $3, exc_pat = $4, exc_obr = $5, pd_pat = $6, pd_obr = $7,
                        gmp_pat = $8, gmp_obr = $9, rt = $10, iv_pat = $11, iv_obr = $12,
                        gps = $13, patronal = $14, obrera = $15, subtotal = $16
                    WHERE id = $1
                    "#,
                    &[
                        &last_id, &new_dro,
                        &round2(cf_orig), &round2(exc_pat_orig), &round2(exc_obr_orig),
                        &round2(pd_pat_orig), &round2(pd_obr_orig), &round2(gmp_pat_orig),
                        &round2(gmp_obr_orig), &round2(rt_orig), &round2(iv_pat_orig),
                        &round2(iv_obr_orig), &round2(gps_orig), &round2(patronal_orig),
                        &round2(obrera_orig), &round2(subtotal_orig),
                    ],
                ).map_err(|e| format!("Error completando días para {}: {}", emp_nombre, e))?;

                stats.corregidos_dias_faltantes += 1;
                stats.total_corregidos += 1;
            }
        }
    }

    tx.commit().map_err(|e| format!("Error confirmando autocorrección: {}", e))?;

    stats.resumen = format!(
        "Verificación completada: {} empleados analizados. {} correcciones aplicadas ({} días faltantes completados, {} días excedidos ajustados, {} bajas a 0d, {} obras auto-vinculadas).",
        stats.total_analizados,
        stats.total_corregidos,
        stats.corregidos_dias_faltantes,
        stats.corregidos_dias_excedidos,
        stats.corregidos_bajas_cero,
        stats.obras_auto_vinculadas,
    );

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn sql_en_universo_une_periodo_y_obras_con_and() {
        let sql = sql_en_universo("$3", "rl.anio_periodo", SQL_MES_RL);
        let pos_periodo = sql.find("/* en_periodo */").expect("falta el marcador en_periodo");
        let pos_obras = sql.find("/* en_obras */").expect("falta el marcador en_obras");
        assert!(pos_periodo < pos_obras, "en_periodo debe ir antes que en_obras");
        let entre = &sql[pos_periodo..pos_obras];
        assert!(
            entre.trim_end().ends_with("END\n            AND"),
            "en_periodo debe cerrar con END y unirse con AND a en_obras: {entre:?}"
        );
        assert_eq!(sql.matches("$3").count(), 4, "el parámetro $3 (revisión) debe usarse en ambas partes (3 en en_periodo y 1 en en_obras)");
        assert!(!sql.contains("--"), "el SQL no debe usar comentarios de línea");
    }

    #[test]
    fn sql_en_universo_correlacionado_no_usa_parametros() {
        let sql = sql_en_universo("rl.id_revision", "rl.anio_periodo", SQL_MES_RL);
        assert!(!sql.contains('$'), "con rev correlacionada el SQL no debe contener parámetros `$`");
        assert!(sql.contains("rl.id_revision"), "la revisión debe referirse a la fila");
    }

    #[test]
    fn sql_en_universo_sin_ro_no_es_universo_total() {
        let sql = sql_en_universo("rl.id_revision", "rl.anio_periodo", SQL_MES_RL);
        let pos_obras = sql.find("/* en_obras */").expect("falta el marcador en_obras");
        let en_obras = &sql[pos_obras..];
        assert!(!en_obras.contains("ELSE true"), "sin RO el universo debe ser vacío, no total");
        assert!(en_obras.contains("fecha_inicio IS NOT NULL"), "las RO sin fecha_inicio no deben aportar meses");
        assert!(
            en_obras.contains("GREATEST(COALESCE(ro.fecha_fin, ro.fecha_inicio), ro.fecha_inicio)"),
            "fecha_fin NULL debe cubrir solo el mes de inicio y fecha_fin < fecha_inicio debe tratarse como inicio"
        );
    }

    fn create_mock_row(emp_id: Uuid, nombre: &str, oid: Option<Uuid>, obra_num: &str, is_user: bool, mes: &str, anio: i16, dias: i16, sub: f64) -> ReporteLiquidacionRowDb {
        ReporteLiquidacionRowDb {
            id_reporte_liq: Uuid::now_v7(),
            id_parent_reporte_liq: Uuid::now_v7(),
            id_liquidacion: Uuid::now_v7(),
            id_revision: Uuid::now_v7(),
            id_empleado: emp_id,
            nombre_trabajador: nombre.to_string(),
            rfc: "XAXX010101000".to_string(),
            curp: "XAXX010101HDF00".to_string(),
            nss: "12345678901".to_string(),
            pensionado: String::new(),
            registro_patronal: "Y5410204108".to_string(),
            mes_periodo: mes.to_string(),
            mes_periodo_int: mes_to_periodo_int(mes),
            anio_periodo: anio,
            periodo_str: format!("{}/{}", mes, anio),
            dias_cedula: dias,
            id_registro_obra: oid,
            obra_numero: obra_num.to_string(),
            obra_label: if oid.is_some() { obra_num.to_string() } else { "⚠️ Sin Asignar".to_string() },
            dias_obra: dias,
            obra_creado_usuario: is_user,
            clave: "".to_string(),
            fecha: None,
            fecha_str: "-".to_string(),
            sdi: 350.0,
            lic: 0.0,
            inc: 0.0,
            aus: 0.0,
            cf: sub * 0.4,
            exc_pat: 0.0,
            exc_obr: 0.0,
            pd_pat: 0.0,
            pd_obr: 0.0,
            gmp_pat: 0.0,
            gmp_obr: 0.0,
            rt: 0.0,
            iv_pat: 0.0,
            iv_obr: 0.0,
            gps: sub * 0.05,
            patronal: sub * 0.8,
            obrera: sub * 0.2,
            subtotal: sub,
            cf_orig: sub * 0.4,
            exc_pat_orig: 0.0,
            exc_obr_orig: 0.0,
            pd_pat_orig: 0.0,
            pd_obr_orig: 0.0,
            gmp_pat_orig: 0.0,
            gmp_obr_orig: 0.0,
            rt_orig: 0.0,
            iv_pat_orig: 0.0,
            iv_obr_orig: 0.0,
            gps_orig: sub * 0.05,
            patronal_orig: sub * 0.8,
            obrera_orig: sub * 0.2,
            subtotal_orig: sub,
            tiene_inconsistencia: false,
            observacion_validacion: String::new(),
            estado: "ACTIVO".to_string(),
            cedula_actualizada: false,
            editado_usuario: false,
        }
    }

    #[test]
    fn missing_header_fields_detecta_vacios() {
        let vacio = ReporteFinalHeaderMeta::default();
        assert_eq!(missing_header_fields(&vacio).len(), 8);

        let completo = ReporteFinalHeaderMeta {
            delegacion: "DELEGACION".to_string(),
            subdelegacion: "SUBDELEGACION".to_string(),
            cliente_nombre: "EMPRESA SA DE CV".to_string(),
            rp_registro: "RP-1".to_string(),
            rfc: "XAXX010101000".to_string(),
            periodo_obra: "01/01/2024 AL 31/12/2024".to_string(),
            representante_legal: "REPRESENTANTE".to_string(),
            revision_folio: "FOLIO-1".to_string(),
        };
        assert!(missing_header_fields(&completo).is_empty());

        let mut rep_blank = completo.clone();
        rep_blank.representante_legal = "   ".to_string();
        assert!(missing_header_fields(&rep_blank).contains(&"Representante legal"));
    }

    #[test]
    fn test_export_reporte_liquidaciones_excel_multi_sheet() {
        let obra_user_1 = Uuid::now_v7();
        let obra_user_2 = Uuid::now_v7();
        let obra_cedula = Uuid::now_v7();

        let emp_1 = Uuid::now_v7();
        let emp_2 = Uuid::now_v7();
        let emp_3 = Uuid::now_v7();

        let mut valid_obras = HashSet::new();
        valid_obras.insert(obra_user_1);
        valid_obras.insert(obra_user_2);

        let rows = vec![
            // Emp 1 en Obra 1 (Usuario) - Mes 1
            create_mock_row(emp_1, "PUGA LOPEZ JOSE MARIA", Some(obra_user_1), "OBRA-USER-01", true, "ENERO", 2024, 30, 1000.0),
            // Emp 1 en Obra 1 (Usuario) - Mes 2
            create_mock_row(emp_1, "PUGA LOPEZ JOSE MARIA", Some(obra_user_1), "OBRA-USER-01", true, "FEBRERO", 2024, 28, 900.0),
            // Emp 1 en Obra 2 (Usuario) - Mes 2
            create_mock_row(emp_1, "PUGA LOPEZ JOSE MARIA", Some(obra_user_2), "OBRA-USER-02", true, "FEBRERO", 2024, 15, 500.0),
            // Emp 2 en Obra Cédula (NO Usuario) - debe ser omitido en Sheets 2 y 3
            create_mock_row(emp_2, "LOPEZ PEREZ JUAN", Some(obra_cedula), "OBRA-CEDULA-AUTO", false, "ENERO", 2024, 30, 800.0),
            // Emp 3 Sin Obra - debe ser omitido en Sheets 2 y 3
            create_mock_row(emp_3, "GONZALEZ HERNANDEZ PEDRO", None, "", false, "ENERO", 2024, 30, 700.0),
        ];

        let obras_options = vec![
            ObraOptionDb {
                id: obra_user_1,
                numero: "OBRA-USER-01".to_string(),
                tipo_obra: "EDIFICACION".to_string(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "OBRA-USER-01 - EDIFICIO TORRE".to_string(),
                creado_usuario: true,
                monto: 1_000_000.0,
            },
            ObraOptionDb {
                id: obra_user_2,
                numero: "OBRA-USER-02".to_string(),
                tipo_obra: "URBANIZACION".to_string(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "OBRA-USER-02 - CALLE PRINCIPAL".to_string(),
                creado_usuario: true,
                monto: 500_000.0,
            },
        ];

        let guarderias_kpis = compute_guarderias_kpis(&rows, &obras_options, None);
        assert_eq!(guarderias_kpis.len(), 2);
        assert_eq!(guarderias_kpis[0].empleados_count, 2);
        assert_eq!(guarderias_kpis[1].empleados_count, 1);
        assert!(!guarderias_kpis[0].supera_18);

        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("test_export_liq_{}.xlsx", Uuid::now_v7()));

        let res = export_reporte_liquidaciones_to_excel(
            &rows,
            &rows,
            Some(&valid_obras),
            &guarderias_kpis,
            &test_file,
            "EMPRESA PRUEBA S.A. DE C.V.",
            "Y54-10204-10-8",
            "REVISION 2024-01",
            &ReporteFinalHeaderMeta {
                delegacion: "DELEGACION DE PRUEBA".to_string(),
                subdelegacion: "SUBDELEGACION DE PRUEBA".to_string(),
                cliente_nombre: "EMPRESA DE PRUEBA SA DE CV".to_string(),
                rp_registro: "Y54-10204-10-8".to_string(),
                rfc: "XAXX010101000".to_string(),
                periodo_obra: "01/01/2024 AL 31/12/2024".to_string(),
                representante_legal: "REPRESENTANTE DE PRUEBA".to_string(),
                revision_folio: "REVISION 2024-01".to_string(),
            },
            ModoEntregable::Entregable { version: 1 },
        );

        assert!(res.is_ok(), "El reporte en Excel debe generarse correctamente: {:?}", res.err());
        assert!(test_file.exists(), "El archivo Excel generado debe existir físicamente");
        let metadata = std::fs::metadata(&test_file).unwrap();
        assert!(metadata.len() > 1000, "El archivo Excel generado debe contener datos");

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn test_compute_guarderias_kpis_solo_obras_creado_usuario() {
        let obra_user = Uuid::now_v7();
        let obra_cedula_con_monto = Uuid::now_v7();
        let obra_cedula_sin_monto = Uuid::now_v7();

        let emp_1 = Uuid::now_v7();
        let emp_2 = Uuid::now_v7();

        let rows = vec![
            create_mock_row(emp_1, "EMPLEADO 1", Some(obra_user), "OBRA-USER", true, "ENERO", 2024, 30, 500.0),
            create_mock_row(emp_2, "EMPLEADO 2", Some(obra_cedula_con_monto), "OBRA-CEDULA-MONTO", false, "ENERO", 2024, 30, 400.0),
            create_mock_row(emp_2, "EMPLEADO 2", Some(obra_cedula_sin_monto), "OBRA-CEDULA-CERO", false, "ENERO", 2024, 30, 300.0),
        ];

        let obras_options = vec![
            ObraOptionDb {
                id: obra_user,
                numero: "OBRA-USER".to_string(),
                tipo_obra: "EDIFICACION".to_string(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "OBRA-USER".to_string(),
                creado_usuario: true,
                monto: 1_000_000.0,
            },
            ObraOptionDb {
                id: obra_cedula_con_monto,
                numero: "OBRA-CEDULA-MONTO".to_string(),
                tipo_obra: "CÉDULA".to_string(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "OBRA-CEDULA-MONTO".to_string(),
                creado_usuario: false,
                monto: 500_000.0, // Tiene monto > 0 pero NO fue creada por el usuario
            },
            ObraOptionDb {
                id: obra_cedula_sin_monto,
                numero: "OBRA-CEDULA-CERO".to_string(),
                tipo_obra: "CÉDULA".to_string(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "OBRA-CEDULA-CERO".to_string(),
                creado_usuario: false,
                monto: 0.0,
            },
        ];

        // 1. Sin filtro de obra: solo debe incluir obra_user
        let kpis = compute_guarderias_kpis(&rows, &obras_options, None);
        assert_eq!(kpis.len(), 1, "Solo la obra creada por el usuario debe tener KPIs");
        assert_eq!(kpis[0].id_obra, obra_user);
        assert_eq!(kpis[0].empleados_count, 1);

        // 2. Con filtro de obra apuntando a una obra no creada por el usuario: debe retornar vacío
        let kpis_cedula = compute_guarderias_kpis(&rows, &obras_options, Some(obra_cedula_con_monto));
        assert!(kpis_cedula.is_empty(), "Obras no creadas por el usuario no deben generar KPIs");

        // 3. Con filtro de obra apuntando a la obra creada por el usuario: debe retornar 1
        let kpis_filtrado_user = compute_guarderias_kpis(&rows, &obras_options, Some(obra_user));
        assert_eq!(kpis_filtrado_user.len(), 1);
        assert_eq!(kpis_filtrado_user[0].id_obra, obra_user);
    }

    #[test]
    fn test_calculate_max_dias_obra_con_baja_ejemplo_usuario() {
        // Ejemplo del usuario:
        // Obra: C1234567
        // Fecha Inicio: 18/03/2026
        // Fecha Fin:    30/05/2026
        // Dias Cedula: 12
        // Lic: 0, Inc: 0, Aus: 0
        // Clave: Baja, Fecha: 12/03/2026
        let f_ini_obra = NaiveDate::from_ymd_opt(2026, 3, 18);
        let f_fin_obra = NaiveDate::from_ymd_opt(2026, 5, 30);
        let f_baja = NaiveDate::from_ymd_opt(2026, 3, 12);

        let (max_dias, dias_obra) = calculate_max_dias_obra_for_employee(
            12,
            0.0,
            0.0,
            0.0,
            "MARZO",
            2026,
            "03/2026",
            f_ini_obra,
            f_fin_obra,
            f_baja,
            None,
        );

        // La obra inició después de la baja, tope debe ser 0 días
        assert_eq!(max_dias, 0);
        assert_eq!(dias_obra, Some(0));

        // La validación debe rechazar asignar esta obra
        let validation = validate_obra_assignment_for_employee(
            f_ini_obra,
            f_fin_obra,
            f_baja,
            None,
            "MARZO",
            2026,
            "03/2026",
        );
        assert!(validation.is_err());
    }

    #[test]
    fn test_calculate_max_dias_otra_obra_que_si_cubre_dias_trabajados() {
        // Si le asigna otra obra donde la obra cubra los días que pudo haber trabajado (01/03/2026 a 30/05/2026)
        let f_ini_obra = NaiveDate::from_ymd_opt(2026, 3, 1);
        let f_fin_obra = NaiveDate::from_ymd_opt(2026, 5, 30);
        let f_baja = NaiveDate::from_ymd_opt(2026, 3, 12);

        let (max_dias, dias_obra) = calculate_max_dias_obra_for_employee(
            12,
            0.0,
            0.0,
            0.0,
            "MARZO",
            2026,
            "03/2026",
            f_ini_obra,
            f_fin_obra,
            f_baja,
            None,
        );

        assert_eq!(max_dias, 12);
        assert_eq!(dias_obra, Some(12));

        let validation = validate_obra_assignment_for_employee(
            f_ini_obra,
            f_fin_obra,
            f_baja,
            None,
            "MARZO",
            2026,
            "03/2026",
        );
        assert!(validation.is_ok());
    }

    #[test]
    fn test_extract_employee_movements_registros_multiples_mismo_mes() {
        let emp_id = Uuid::now_v7();
        let f_baja = NaiveDate::from_ymd_opt(2024, 3, 12);

        // Registro 1: Fila de movimiento en SUA (Clave: Baja, Fecha: 12/03/2024, Días: 0)
        let mut row1 = create_mock_row(emp_id, "RUIZ OYERVIDES RIGOBERTO", None, "", false, "MARZO", 2024, 0, 0.0);
        row1.clave = "BAJA".to_string();
        row1.fecha = f_baja;

        // Registro 2: Fila de liquidación regular (Clave vacía, Fecha None, Días: 12)
        let row2 = create_mock_row(emp_id, "RUIZ OYERVIDES RIGOBERTO", None, "", false, "MARZO", 2024, 12, 500.0);

        let all_rows = vec![row1.clone(), row2.clone()];

        // Al evaluar row2, debe detectar la baja ocurrida en el mismo mes a partir de row1
        let (baja_det, alta_det) = extract_employee_movements(
            row2.id_empleado,
            &row2.mes_periodo,
            row2.anio_periodo,
            &row2.clave,
            row2.fecha,
            &all_rows,
        );

        assert_eq!(baja_det, f_baja);
        assert_eq!(alta_det, None);

        // Mapa de movimientos
        let map = build_employee_movements_map(&all_rows);
        let key = (emp_id, "MARZO".to_string(), 2024);
        assert_eq!(map.get(&key), Some(&(f_baja, None)));
    }

    #[test]
    fn test_multiples_movimientos_alternados_en_mismo_mes() {
        let f_baja = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let f_reing = NaiveDate::from_ymd_opt(2026, 3, 20).unwrap();

        let movs = vec![
            EmployeeMovement { kind: MovementKind::Baja, fecha: f_baja },
            EmployeeMovement { kind: MovementKind::Alta, fecha: f_reing },
        ];

        let month_start = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let month_end = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();

        let intervals = calculate_employee_active_intervals(month_start, month_end, &movs);
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[0], (month_start, f_baja)); // 01/03 al 10/03 (10 días)
        assert_eq!(intervals[1], (f_reing, month_end));  // 20/03 al 31/03 (12 días)

        // Obra A: 18/03 al 31/03
        let obra_a_ini = NaiveDate::from_ymd_opt(2026, 3, 18);
        let obra_a_fin = NaiveDate::from_ymd_opt(2026, 3, 31);
        let (max_a, dias_a) = calculate_max_dias_obra_for_employee_movements(
            30, 0.0, 0.0, 0.0, "MARZO", 2026, "03/2026", obra_a_ini, obra_a_fin, &movs
        );
        // Coincide únicamente con el segundo intervalo (20/03 a 31/03) = 12 días
        assert_eq!(max_a, 12);
        assert_eq!(dias_a, Some(12));

        // Obra B: 12/03 al 18/03 (cae justo en el periodo inactivo del trabajador)
        let obra_b_ini = NaiveDate::from_ymd_opt(2026, 3, 12);
        let obra_b_fin = NaiveDate::from_ymd_opt(2026, 3, 18);
        let (max_b, dias_b) = calculate_max_dias_obra_for_employee_movements(
            30, 0.0, 0.0, 0.0, "MARZO", 2026, "03/2026", obra_b_ini, obra_b_fin, &movs
        );
        assert_eq!(max_b, 0);
        assert_eq!(dias_b, Some(0));
        let val_b = validate_obra_assignment_for_employee_movements(
            obra_b_ini, obra_b_fin, &movs, "MARZO", 2026, "03/2026"
        );
        assert!(val_b.is_err());

        // Obra C: Todo el mes (01/03 al 31/03)
        let (max_c, dias_c) = calculate_max_dias_obra_for_employee_movements(
            30, 0.0, 0.0, 0.0, "MARZO", 2026, "03/2026", Some(month_start), Some(month_end), &movs
        );
        // Total activo: 10 + 12 = 22 días
        assert_eq!(max_c, 22);
        assert_eq!(dias_c, Some(22));
    }

    #[test]
    fn test_alta_baja_reing_baja_secuencia_completa() {
        let f_alta1 = NaiveDate::from_ymd_opt(2026, 3, 5).unwrap();
        let f_baja1 = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let f_alta2 = NaiveDate::from_ymd_opt(2026, 3, 15).unwrap();
        let f_baja2 = NaiveDate::from_ymd_opt(2026, 3, 25).unwrap();

        let movs = vec![
            EmployeeMovement { kind: MovementKind::Alta, fecha: f_alta1 },
            EmployeeMovement { kind: MovementKind::Baja, fecha: f_baja1 },
            EmployeeMovement { kind: MovementKind::Alta, fecha: f_alta2 },
            EmployeeMovement { kind: MovementKind::Baja, fecha: f_baja2 },
        ];

        let month_start = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let month_end = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();

        let intervals = calculate_employee_active_intervals(month_start, month_end, &movs);
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[0], (f_alta1, f_baja1)); // 05/03 al 10/03 (6 días)
        assert_eq!(intervals[1], (f_alta2, f_baja2)); // 15/03 al 25/03 (11 días)

        // Obra del 08/03 al 20/03:
        // En primer intervalo (05/03-10/03): solapa 08/03 al 10/03 = 3 días
        // En segundo intervalo (15/03-25/03): solapa 15/03 al 20/03 = 6 días
        // Total solapamiento = 3 + 6 = 9 días
        let obra_ini = NaiveDate::from_ymd_opt(2026, 3, 8);
        let obra_fin = NaiveDate::from_ymd_opt(2026, 3, 20);
        let (max_dias, dias_obra) = calculate_max_dias_obra_for_employee_movements(
            20, 0.0, 0.0, 0.0, "MARZO", 2026, "03/2026", obra_ini, obra_fin, &movs
        );
        assert_eq!(max_dias, 9);
        assert_eq!(dias_obra, Some(9));
    }

    #[test]
    fn test_allocate_days_across_obras_caso_usuario_octubre() {
        let oid_usuario = Uuid::now_v7();
        let oid_cedula = Uuid::now_v7();

        let obras = vec![
            ObraAllocationCandidate {
                id: oid_usuario,
                numero: "C0995849".to_string(),
                fecha_inicio: NaiveDate::from_ymd_opt(2024, 10, 14),
                fecha_fin: NaiveDate::from_ymd_opt(2024, 11, 15),
                tipo_obra: Some("CONSTRUCCION".to_string()),
                creado_usuario: true,
            },
            ObraAllocationCandidate {
                id: oid_cedula,
                numero: "C0995853".to_string(),
                fecha_inicio: NaiveDate::from_ymd_opt(2024, 10, 1),
                fecha_fin: NaiveDate::from_ymd_opt(2024, 11, 30),
                tipo_obra: Some("REGISTRO DE OBRA CÉDULA".to_string()),
                creado_usuario: false,
            },
        ];

        let res = allocate_days_across_obras(
            &obras,
            31,
            31,
            0.0,
            0.0,
            0.0,
            "OCTUBRE",
            2024,
            "10/2024",
            &[],
        );

        assert_eq!(res.len(), 2);
        assert_eq!(res[0].0, oid_usuario);
        assert_eq!(res[0].1, 18, "C0995849 debe tener 18 días en Octubre (del 14 al 31)");

        assert_eq!(res[1].0, oid_cedula);
        assert_eq!(res[1].1, 13, "C0995853 debe tener 13 días en Octubre (del 1 al 13)");
    }

    #[test]
    fn test_allocate_days_across_obras_caso_usuario_noviembre() {
        let oid_usuario = Uuid::now_v7();
        let oid_cedula = Uuid::now_v7();

        let obras = vec![
            ObraAllocationCandidate {
                id: oid_usuario,
                numero: "C0995849".to_string(),
                fecha_inicio: NaiveDate::from_ymd_opt(2024, 10, 14),
                fecha_fin: NaiveDate::from_ymd_opt(2024, 11, 15),
                tipo_obra: Some("CONSTRUCCION".to_string()),
                creado_usuario: true,
            },
            ObraAllocationCandidate {
                id: oid_cedula,
                numero: "C0995853".to_string(),
                fecha_inicio: NaiveDate::from_ymd_opt(2024, 10, 1),
                fecha_fin: NaiveDate::from_ymd_opt(2024, 11, 30),
                tipo_obra: Some("REGISTRO DE OBRA CÉDULA".to_string()),
                creado_usuario: false,
            },
        ];

        let res = allocate_days_across_obras(
            &obras,
            30,
            30,
            0.0,
            0.0,
            0.0,
            "NOVIEMBRE",
            2024,
            "11/2024",
            &[],
        );

        assert_eq!(res.len(), 2);
        assert_eq!(res[0].0, oid_usuario);
        assert_eq!(res[0].1, 15, "C0995849 debe tener 15 días en Noviembre (del 1 al 15)");

        assert_eq!(res[1].0, oid_cedula);
        assert_eq!(res[1].1, 15, "C0995853 debe tener 15 días en Noviembre (del 16 al 30)");
    }

    #[test]
    fn test_allocate_days_across_obras_con_baja() {
        let oid_usuario = Uuid::now_v7();
        let oid_cedula = Uuid::now_v7();

        let obras = vec![
            ObraAllocationCandidate {
                id: oid_usuario,
                numero: "C0995849".to_string(),
                fecha_inicio: NaiveDate::from_ymd_opt(2024, 10, 14),
                fecha_fin: NaiveDate::from_ymd_opt(2024, 11, 15),
                tipo_obra: Some("CONSTRUCCION".to_string()),
                creado_usuario: true,
            },
            ObraAllocationCandidate {
                id: oid_cedula,
                numero: "C0995853".to_string(),
                fecha_inicio: NaiveDate::from_ymd_opt(2024, 10, 1),
                fecha_fin: NaiveDate::from_ymd_opt(2024, 11, 30),
                tipo_obra: Some("REGISTRO DE OBRA CÉDULA".to_string()),
                creado_usuario: false,
            },
        ];

        let movs = vec![
            EmployeeMovement {
                kind: MovementKind::Baja,
                fecha: NaiveDate::from_ymd_opt(2024, 10, 20).unwrap(),
            },
        ];

        let res = allocate_days_across_obras(
            &obras,
            20,
            20,
            0.0,
            0.0,
            0.0,
            "OCTUBRE",
            2024,
            "10/2024",
            &movs,
        );

        assert_eq!(res.len(), 2);
        assert_eq!(res[0].0, oid_usuario);
        assert_eq!(res[0].1, 7, "C0995849 del 14 al 20 = 7 días");

        assert_eq!(res[1].0, oid_cedula);
        assert_eq!(res[1].1, 13, "C0995853 del 1 al 13 = 13 días");
    }

    #[test]

    #[ignore]

    // Escribe en la BD y/o RustFS configurados (producción): ejecutar solo explícitamente con --ignored
    fn test_verify_no_duplicate_rows_in_reportes_detalles() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(_) => return,
        };

        println!("=== VERIFYING GLOBAL CLEANUP FOR DUPLICATES ===");
        let cleanup_res = cleanup_all_duplicate_reportes_detalles(&mut client);
        assert!(cleanup_res.is_ok(), "cleanup_all_duplicate_reportes_detalles failed: {:?}", cleanup_res.err());

        // 1. Verificar que no existan filas duplicadas idénticas (mismo parent, misma obra, misma clave y misma fecha)
        let exact_dup_count: i64 = client.query_one(
            r#"
            SELECT COUNT(*) FROM (
                SELECT 1
                FROM public.reportes_detalles_liquidaciones
                WHERE id_registro_obra IS NOT NULL
                  AND NOT (LOWER(TRIM(COALESCE(clave, ''))) = 'baja')
                GROUP BY id_reporte_liquidacion, id_registro_obra,
                         COALESCE(TRIM(LOWER(clave)), ''), COALESCE(fecha, '1900-01-01'::date)
                HAVING COUNT(*) > 1
            ) sub
            "#,
            &[],
        ).unwrap().get(0);
        assert_eq!(exact_dup_count, 0, "Existen grupos duplicados en reportes_detalles_liquidaciones!");

        // 2. Verificar que no existan duplicados en reportes_vinculaciones_obras_liquidaciones
        let rvol_dup_count: i64 = client.query_one(
            r#"
            SELECT COUNT(*) FROM (
                SELECT 1
                FROM public.reportes_vinculaciones_obras_liquidaciones
                WHERE id_registro_obra IS NOT NULL
                GROUP BY id_reporte_liquidacion, id_registro_obra
                HAVING COUNT(*) > 1
            ) sub
            "#,
            &[],
        ).unwrap().get(0);
        assert_eq!(rvol_dup_count, 0, "Existen duplicados en reportes_vinculaciones_obras_liquidaciones!");

        // 3. Verificar que las filas de Bermúdez no presenten inconsistencias de negocio
        let rev_encino = Uuid::parse_str("01a1b22a-b302-71bc-9b9f-9a10eb34eb0d").unwrap();
        let rows_after = fetch_reporte_liquidaciones(&mut client, rev_encino).unwrap();
        let bermudez_rows: Vec<_> = rows_after.into_iter().filter(|r| r.nombre_trabajador.contains("BERMUDEZ")).collect();
        for br in &bermudez_rows {
            assert!(!br.tiene_inconsistencia, "Bermúdez fila {} no debe tener inconsistencia: {}", br.periodo_str, br.observacion_validacion);
        }
    }

    #[test]
    fn guarderias_kpi_sin_monto_no_alerta_18() {
        let oid_sin_monto = Uuid::now_v7();
        let oid_con_monto = Uuid::now_v7();
        // sub = 100 -> gps = 5.0 -> valor guarderías = 500.0
        let rows = vec![
            create_mock_row(Uuid::now_v7(), "EMP SIN MONTO", Some(oid_sin_monto), "OBRA-SM", true, "ENERO", 2025, 30, 100.0),
            create_mock_row(Uuid::now_v7(), "EMP CON MONTO", Some(oid_con_monto), "OBRA-CM", true, "ENERO", 2025, 30, 100.0),
        ];
        let obras = vec![
            ObraOptionDb {
                id: oid_sin_monto,
                numero: "OBRA-SM".to_string(),
                tipo_obra: "EDIFICACION".to_string(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "OBRA-SM".to_string(),
                creado_usuario: true,
                monto: 0.0,
            },
            ObraOptionDb {
                id: oid_con_monto,
                numero: "OBRA-CM".to_string(),
                tipo_obra: "EDIFICACION".to_string(),
                fecha_inicio: None,
                fecha_fin: None,
                label: "OBRA-CM".to_string(),
                creado_usuario: true,
                monto: 1000.0,
            },
        ];

        let kpis = compute_guarderias_kpis(&rows, &obras, None);
        assert_eq!(kpis.len(), 2);

        // Sin monto: no hay alerta falsa del 18%
        assert!(kpis[0].sin_monto);
        assert!(!kpis[0].supera_18);
        assert_eq!(kpis[0].monto_obra, 0.0);

        // Con monto 1000: límite 180 < valor 500 -> supera
        assert!(!kpis[1].sin_monto);
        assert!(kpis[1].supera_18);
    }
}


