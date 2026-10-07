use chrono::{Datelike, NaiveDate};
use postgres::Client;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use uuid::Uuid;

use crate::cedulas::business::month_num_to_name;

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
    pub total_gps: f64,
    pub valor_guarderias: f64,
    pub monto_obra: f64,
    pub limite_18_obra: f64,
    pub supera_18: bool,
    pub empleados_count: usize,
}

/// Calcula los KPIs de Guarderías (Tope 18%) para cada registro de obra aplicable
pub fn compute_guarderias_kpis(
    rows: &[ReporteLiquidacionRowDb],
    obras_options: &[ObraOptionDb],
    filter_obra_id: Option<Uuid>,
) -> Vec<ReporteGuarderiaKpiDb> {
    let mut result = Vec::new();
    for o in obras_options.iter().filter(|o| o.creado_usuario || o.monto > 0.0 || rows.iter().any(|r| r.id_registro_obra == Some(o.id))) {
        if let Some(target_id) = filter_obra_id {
            if o.id != target_id {
                continue;
            }
        }
        let mut total_gps_obra = 0.0;
        let mut emps_count = 0;
        for r in rows {
            if r.id_registro_obra == Some(o.id) {
                total_gps_obra += r.gps;
                emps_count += 1;
            }
        }
        let monto_obra = o.monto;
        let limite_18 = monto_obra * 0.18;
        // Para calcular el valor de las guarderías es el total de gps de todos los empleados vinculados al registro de obra dividido entre 1% (0.01):
        let valor_guarderias = total_gps_obra / 0.01;
        let supera_18 = (monto_obra > 0.0 && valor_guarderias > limite_18) || (monto_obra == 0.0 && valor_guarderias > 0.0);

        result.push(ReporteGuarderiaKpiDb {
            id_obra: o.id,
            obra_numero: o.numero.clone(),
            obra_label: o.label.clone(),
            total_gps: total_gps_obra,
            valor_guarderias,
            monto_obra,
            limite_18_obra: limite_18,
            supera_18,
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
    pub id_empleado: Uuid,
    pub nombre_trabajador: String,
    pub rfc: String,
    pub curp: String,
    pub nss: String,
    pub registro_patronal: String,
    pub mes_periodo: String,
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

/// Obtiene los registros de obra asociados a una revisión
pub fn fetch_obras_for_revision(client: &mut Client, id_revision: Uuid) -> Result<Vec<ObraOptionDb>, String> {
    let rows = client.query(
        r#"
        SELECT ro.id, ro.numero, ro.tipo_obra, ro.fecha_inicio, ro.fecha_fin,
               (COALESCE(ro.creado_usuario, false) = true OR COALESCE(ro.monto, 0.0) > 0.0) AS creado_usuario,
               COALESCE(ro.monto, 0.0) AS monto
        FROM public.registros_obras ro
        WHERE ro.id_revision = $1
           OR ro.id_registro_patronal = (SELECT r.id_registro_patronal FROM public.revisiones r WHERE r.id = $1)
        ORDER BY ro.numero ASC
        "#,
        &[&id_revision],
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

/// Sincroniza y copia a las tablas de reportes (reportes_liquidaciones, reportes_detalles_liquidaciones, reportes_vinculaciones_obras_liquidaciones)
/// las liquidaciones filtradas en base a las fechas de los registros de obras de la revisión analizada.
/// Obtiene los años analizados y los nombres de meses en español (ej. DICIEMBRE, ENERO, FEBRERO...)
/// y filtra exactamente sobre las tablas de liquidaciones.
pub fn sync_reportes_liquidaciones_for_revision(
    client: &mut Client,
    id_revision: Uuid,
) -> Result<usize, String> {
    let obras = fetch_obras_for_revision(client, id_revision)?;

    // 1. Obtener los años y los nombres de los meses en español a partir de las fechas de las obras
    let mut anios_set: BTreeSet<i16> = BTreeSet::new();
    let mut meses_set: BTreeSet<String> = BTreeSet::new();

    for o in &obras {
        if let (Some(f_ini), Some(f_fin)) = (o.fecha_inicio, o.fecha_fin) {
            let mut curr_y = f_ini.year();
            let mut curr_m = f_ini.month();
            let end_y = f_fin.year();
            let end_m = f_fin.month();

            while (curr_y < end_y) || (curr_y == end_y && curr_m <= end_m) {
                anios_set.insert(curr_y as i16);
                let mes_nom = month_num_to_name(curr_m);
                if !mes_nom.is_empty() {
                    meses_set.insert(mes_nom.to_string());
                }
                curr_m += 1;
                if curr_m > 12 {
                    curr_m = 1;
                    curr_y += 1;
                }
            }
        }
    }

    let mut tx = client.transaction().map_err(|e| format!("Error iniciando transacción: {}", e))?;

    let liq_rows = if !anios_set.is_empty() && !meses_set.is_empty() {
        let anios_vec: Vec<i16> = anios_set.into_iter().collect();
        let meses_vec: Vec<String> = meses_set.into_iter().collect();

        // Limpiar en reportes_liquidaciones aquellos registros de la revisión fuera de los años y meses filtrados
        let _ = tx.execute(
            r#"
            DELETE FROM public.reportes_liquidaciones
            WHERE id_revision = $1
              AND (
                  NOT (UPPER(TRIM(mes_periodo)) = ANY($2))
                  OR NOT (anio_periodo = ANY($3))
              )
            "#,
            &[&id_revision, &meses_vec, &anios_vec],
        );

        // Consultar con filtro exacto de años y nombres de meses en español
        tx.query(
            r#"
            SELECT l.id, l.id_empleado, l.mes_periodo, l.anio_periodo, l.nombre_archivo
            FROM public.liquidaciones l
            WHERE l.id_revision = $1
              AND UPPER(TRIM(l.mes_periodo)) = ANY($2)
              AND l.anio_periodo = ANY($3)
            ORDER BY l.anio_periodo ASC, l.mes_periodo ASC
            "#,
            &[&id_revision, &meses_vec, &anios_vec],
        ).map_err(|e| format!("Error consultando liquidaciones filtradas: {}", e))?
    } else {
        tx.query(
            r#"
            SELECT l.id, l.id_empleado, l.mes_periodo, l.anio_periodo, l.nombre_archivo
            FROM public.liquidaciones l
            WHERE l.id_revision = $1
            ORDER BY l.anio_periodo ASC, l.mes_periodo ASC
            "#,
            &[&id_revision],
        ).map_err(|e| format!("Error consultando liquidaciones: {}", e))?
    };

    let valid_obras_rows = tx.query(
        "SELECT id FROM public.registros_obras WHERE id_revision = $1",
        &[&id_revision],
    ).map_err(|e| format!("Error consultando registros de obras válidos: {}", e))?;
    let valid_obras_vec: Vec<Uuid> = valid_obras_rows.into_iter().map(|r| r.get(0)).collect();
    let valid_obras_set: HashSet<Uuid> = valid_obras_vec.iter().cloned().collect();

    let mut copied_count = 0;

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
        let anio: Option<i16> = row.try_get(3).ok();
        let nombre_arch: Option<String> = row.try_get(4).ok();

        // 1. Obtener o crear fila en reportes_liquidaciones
        let rep_liq_opt = tx.query_opt(
            "SELECT id FROM public.reportes_liquidaciones WHERE id_liquidacion = $1 LIMIT 1",
            &[&id_liq],
        ).map_err(|e| format!("Error buscando reporte_liquidacion: {}", e))?;

        let id_rep_liq = match rep_liq_opt {
            Some(r) => r.try_get::<_, Uuid>(0).unwrap_or_else(|_| Uuid::now_v7()),
            None => {
                let new_rep_id = Uuid::now_v7();
                tx.execute(
                    r#"
                    INSERT INTO public.reportes_liquidaciones (
                        id, id_liquidacion, id_revision, id_empleado, mes_periodo, anio_periodo, nombre_archivo
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7)
                    "#,
                    &[&new_rep_id, &id_liq, &id_revision, &id_emp, &mes_str, &anio, &nombre_arch],
                ).map_err(|e| format!("Error insertando reporte_liquidacion: {}", e))?;
                new_rep_id
            }
        };

        // 2. Vinculación de obra en reportes (SOLO lo que el usuario haya asignado explícitamente y pertenezca a la revisión)
        let rep_vinc_opt = tx.query_opt(
            "SELECT id, id_registro_obra, dias_registro_obra FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1 LIMIT 1",
            &[&id_rep_liq],
        ).map_err(|e| format!("Error consultando reporte vinculación: {}", e))?;

        let orig_vinc_opt = tx.query_opt(
            "SELECT id_registro_obra, dias_registro_obra FROM public.vinculaciones_obras_liquidaciones WHERE id_liquidacion = $1 LIMIT 1",
            &[&id_liq],
        ).map_err(|e| format!("Error consultando vinculación original: {}", e))?;

        let (target_obra_id, target_dias) = match orig_vinc_opt {
            Some(ref ov) => {
                let o_id: Option<Uuid> = ov.try_get::<_, Option<Uuid>>(0).ok().flatten();
                let d_obra: Option<i16> = ov.try_get::<_, Option<i16>>(1).ok().flatten();
                if let Some(oid) = o_id {
                    if valid_obras_set.contains(&oid) {
                        (Some(oid), d_obra)
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                }
            },
            None => {
                let emp_obra_row = tx.query_opt(
                    r#"
                    SELECT ro.id FROM public.registros_obras ro
                    JOIN public.empleados e ON ro.numero = LEFT(TRIM(e.clave_ubicacion), 15)
                    WHERE ro.id_revision = $1 AND e.id = $2
                    LIMIT 1
                    "#,
                    &[&id_revision, &id_emp],
                ).ok().flatten();
                if let Some(er) = emp_obra_row {
                    let oid: Uuid = er.get(0);
                    (Some(oid), Some(30))
                } else {
                    (None, None)
                }
            },
        };

        if let Some(rv) = rep_vinc_opt {
            let vid: Uuid = rv.get(0);
            let curr_rep_obra: Option<Uuid> = rv.try_get(1).ok().flatten();
            // Preservar los cambios que el usuario haya realizado en la vista de Reporte Liquidaciones.
            // Si la obra vinculada ya no pertenece a las obras válidas o es nula, actualizar o eliminar la vinculación
            if let Some(curr_obra) = curr_rep_obra {
                if !valid_obras_set.contains(&curr_obra) {
                    if let Some(toid) = target_obra_id {
                        let _ = tx.execute(
                            "UPDATE public.reportes_vinculaciones_obras_liquidaciones SET id_registro_obra = $2, dias_registro_obra = $3 WHERE id = $1",
                            &[&vid, &Some(toid), &target_dias],
                        );
                    } else {
                        tx.execute(
                            "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id = $1",
                            &[&vid],
                        ).map_err(|e| format!("Error eliminando vinculación con obra eliminada: {}", e))?;
                    }
                }
            } else if let Some(toid) = target_obra_id {
                let _ = tx.execute(
                    "UPDATE public.reportes_vinculaciones_obras_liquidaciones SET id_registro_obra = $2, dias_registro_obra = $3 WHERE id = $1",
                    &[&vid, &Some(toid), &target_dias],
                );
            } else {
                tx.execute(
                    "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id = $1",
                    &[&vid],
                ).map_err(|e| format!("Error eliminando vinculación vacía: {}", e))?;
            }
        } else if target_obra_id.is_some() {
            // Solo insertar si existe una obra válida asignada originalmente
            let new_vinc_id = Uuid::now_v7();
            tx.execute(
                r#"
                INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (
                    id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra
                ) VALUES ($1, $2, $3, $4)
                "#,
                &[&new_vinc_id, &target_obra_id, &id_rep_liq, &target_dias],
            ).map_err(|e| format!("Error insertando reporte vinculación: {}", e))?;
        }

        // 3. Detalles de liquidación: Aplicar fórmula si tiene obra asignada
        let rep_det_count: i64 = tx.query_one(
            "SELECT COUNT(*) FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion = $1",
            &[&id_rep_liq],
        ).map_err(|e| format!("Error contando reportes detalles: {}", e))?.get(0);

        if rep_det_count == 0 {
            let orig_detalles = tx.query(
                r#"
                SELECT clave, fecha, dias, sdi, lic, inc, aus, cf,
                       exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr,
                       gps, patronal, obrera, subtotal
                FROM public.detalles_liquidaciones
                WHERE id_liquidacion = $1
                "#,
                &[&id_liq],
            ).map_err(|e| format!("Error leyendo detalles originales: {}", e))?;

            for od in orig_detalles {
                let id_rep_det = Uuid::now_v7();
                let clave: Option<String> = od.get(0);
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

                let factor = if target_obra_id.is_some() && dias_det.map(|d| d > 0).unwrap_or(false) {
                    let d_ced = dias_det.unwrap() as f64;
                    let d_obr = target_dias.unwrap_or(dias_det.unwrap()) as f64;
                    (d_obr / d_ced).clamp(0.0, 10.0)
                } else {
                    1.0
                };

                let calc2 = |v: Option<f64>| v.map(|x| (x * factor * 100.0).round() / 100.0);

                let is_baja = clave.as_deref().unwrap_or("").trim().eq_ignore_ascii_case("baja");
                let det_obra_id = if !is_baja { target_obra_id } else { None };
                let det_dias_obr: Option<i16> = if !is_baja && target_obra_id.is_some() {
                    target_dias.or(dias_det)
                } else {
                    Some(0)
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
                        &calc2(cf), &calc2(exc_pat), &calc2(exc_obr), &calc2(pd_pat), &calc2(pd_obr),
                        &calc2(gmp_pat), &calc2(gmp_obr), &calc2(rt), &calc2(iv_pat), &calc2(iv_obr),
                        &calc2(gps), &calc2(patronal), &calc2(obrera), &calc2(subtotal),
                        &det_obra_id, &det_dias_obr
                    ],
                ).map_err(|e| format!("Error insertando reporte detalle calculado: {}", e))?;
            }
        }

        copied_count += 1;
    }

    // Aplicar fórmula proporcional: (concepto / dias_cedula) * dias_obra a todos los que tienen obra válida de la revisión
    if !valid_obras_vec.is_empty() {
        let _ = tx.execute(
            r#"
            UPDATE public.reportes_detalles_liquidaciones rdl
            SET 
                cf = ROUND((dl.cf * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                exc_pat = ROUND((dl.exc_pat * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                exc_obr = ROUND((dl.exc_obr * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                pd_pat = ROUND((dl.pd_pat * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                pd_obr = ROUND((dl.pd_obr * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                gmp_pat = ROUND((dl.gmp_pat * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                gmp_obr = ROUND((dl.gmp_obr * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                rt = ROUND((dl.rt * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                iv_pat = ROUND((dl.iv_pat * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                iv_obr = ROUND((dl.iv_obr * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                gps = ROUND((dl.gps * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                patronal = ROUND((dl.patronal * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                obrera = ROUND((dl.obrera * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8,
                subtotal = ROUND((dl.subtotal * ((rvol.dias_registro_obra::float8) / NULLIF(rdl.dias, 0)::float8))::numeric, 2)::float8
            FROM public.reportes_liquidaciones rl,
                 public.detalles_liquidaciones dl,
                 public.reportes_vinculaciones_obras_liquidaciones rvol
            WHERE rdl.id_reporte_liquidacion = rl.id
              AND dl.id_liquidacion = rl.id_liquidacion
              AND (dl.clave = rdl.clave OR (dl.clave IS NULL AND rdl.clave IS NULL))
              AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL))
              AND rvol.id_reporte_liquidacion = rl.id
              AND rl.id_revision = $1
              AND rvol.id_registro_obra IS NOT NULL
              AND rvol.dias_registro_obra IS NOT NULL
              AND rdl.dias IS NOT NULL AND rdl.dias > 0
              AND rvol.id_registro_obra = ANY($2);
            "#,
            &[&id_revision, &valid_obras_vec],
        );
    }

    // Asegurar que aquellos registros sin obra asignada (o con obra fuera de la revisión) tengan sus detalles exactamente iguales a la cédula original
    let _ = tx.execute(
        r#"
        UPDATE public.reportes_detalles_liquidaciones rdl
        SET 
            cf = dl.cf,
            exc_pat = dl.exc_pat,
            exc_obr = dl.exc_obr,
            pd_pat = dl.pd_pat,
            pd_obr = dl.pd_obr,
            gmp_pat = dl.gmp_pat,
            gmp_obr = dl.gmp_obr,
            rt = dl.rt,
            iv_pat = dl.iv_pat,
            iv_obr = dl.iv_obr,
            gps = dl.gps,
            patronal = dl.patronal,
            obrera = dl.obrera,
            subtotal = dl.subtotal
        FROM public.reportes_liquidaciones rl
        JOIN public.detalles_liquidaciones dl 
            ON dl.id_liquidacion = rl.id_liquidacion 
        LEFT JOIN public.reportes_vinculaciones_obras_liquidaciones rvol
            ON rvol.id_reporte_liquidacion = rl.id
        WHERE rdl.id_reporte_liquidacion = rl.id 
          AND rl.id_revision = $1
          AND (dl.clave = rdl.clave OR (dl.clave IS NULL AND rdl.clave IS NULL))
          AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL))
          AND (rvol.id_registro_obra IS NULL OR NOT (rvol.id_registro_obra = ANY($2)));
        "#,
        &[&id_revision, &valid_obras_vec],
    );

    tx.commit().map_err(|e| format!("Error en commit de sincronización: {}", e))?;
    Ok(copied_count)
}

/// Consulta todas las filas de reportes_liquidaciones para la revisión activa,
/// con sus valores prorrateados y originales listos para la interfaz Slint.
/// Sin ningún COALESCE en la consulta SQL.
pub fn fetch_reporte_liquidaciones(
    client: &mut Client,
    id_revision: Uuid,
) -> Result<Vec<ReporteLiquidacionRowDb>, String> {
    let obras = fetch_obras_for_revision(client, id_revision)?;

    // Extraer años y nombres de meses en español analizados a partir de los registros de obra
    let mut anios_set: BTreeSet<i16> = BTreeSet::new();
    let mut meses_set: BTreeSet<String> = BTreeSet::new();

    for o in &obras {
        if let (Some(f_ini), Some(f_fin)) = (o.fecha_inicio, o.fecha_fin) {
            let mut curr_y = f_ini.year();
            let mut curr_m = f_ini.month();
            let end_y = f_fin.year();
            let end_m = f_fin.month();

            while (curr_y < end_y) || (curr_y == end_y && curr_m <= end_m) {
                anios_set.insert(curr_y as i16);
                let mes_nom = month_num_to_name(curr_m);
                if !mes_nom.is_empty() {
                    meses_set.insert(mes_nom.to_string());
                }
                curr_m += 1;
                if curr_m > 12 {
                    curr_m = 1;
                    curr_y += 1;
                }
            }
        }
    }

    let base_select = r#"
    SELECT 
        rdl.id AS id_reporte_liq,
        rl.id_liquidacion,
        e.id AS id_empleado,
        e.nombre AS nombre_trabajador,
        e.rfc AS rfc,
        e.curp AS curp,
        e.nss AS nss,
        rp.registro AS registro_patronal,
        rl.mes_periodo,
        rl.anio_periodo,
        rdl.dias AS dias_cedula,
        COALESCE(rdl.id_registro_obra, rvol.id_registro_obra) AS id_registro_obra,
        COALESCE(ro_det.numero, ro.numero, '') AS obra_numero,
        COALESCE(ro_det.tipo_obra, ro.tipo_obra, '') AS tipo_obra,
        COALESCE(rdl.dias_registro_obra, rvol.dias_registro_obra, 0) AS dias_obra,
        rdl.cf,
        rdl.exc_pat,
        rdl.exc_obr,
        rdl.pd_pat,
        rdl.pd_obr,
        rdl.gmp_pat,
        rdl.gmp_obr,
        rdl.rt,
        rdl.iv_pat,
        rdl.iv_obr,
        rdl.gps,
        rdl.patronal,
        rdl.obrera,
        rdl.subtotal AS subtotal_prorrateado,
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
        COALESCE(
            ro_det.creado_usuario,
            ro.creado_usuario,
            false
        ) AS obra_creado_usuario
    FROM public.reportes_liquidaciones AS rl
    INNER JOIN public.reportes_detalles_liquidaciones AS rdl
        ON rl.id = rdl.id_reporte_liquidacion
    LEFT JOIN public.reportes_vinculaciones_obras_liquidaciones AS rvol
        ON rl.id = rvol.id_reporte_liquidacion
    LEFT JOIN public.registros_obras AS ro
        ON rvol.id_registro_obra = ro.id
    LEFT JOIN public.registros_obras AS ro_det
        ON rdl.id_registro_obra = ro_det.id
    LEFT JOIN public.detalles_liquidaciones AS dl
        ON dl.id_liquidacion = rl.id_liquidacion 
        AND (
            (COALESCE(TRIM(LOWER(dl.clave)), '') = COALESCE(TRIM(LOWER(rdl.clave)), '')
             AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL)))
            OR (
                (SELECT COUNT(*) FROM public.detalles_liquidaciones sub_dl WHERE sub_dl.id_liquidacion = rl.id_liquidacion) = 1
            )
        )
    JOIN public.empleados AS e
        ON rl.id_empleado = e.id
    JOIN public.revisiones AS rev
        ON rl.id_revision = rev.id
    JOIN public.registros_patronales AS rp
        ON rev.id_registro_patronal = rp.id
    "#;

    let order_clause = r#"
        ORDER BY rl.anio_periodo ASC,
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
            END ASC,
            e.nombre ASC;
    "#;

    let rows = if !anios_set.is_empty() && !meses_set.is_empty() {
        let anios_vec: Vec<i16> = anios_set.into_iter().collect();
        let meses_vec: Vec<String> = meses_set.into_iter().collect();
        let sql = format!(
            "{} WHERE rl.id_revision = $1 AND rl.anio_periodo = ANY($2) AND UPPER(TRIM(rl.mes_periodo)) = ANY($3) {}",
            base_select, order_clause
        );
        client.query(&sql, &[&id_revision, &anios_vec, &meses_vec])
            .map_err(|e| format!("Error consultando filas filtradas de reportes_liquidaciones: {}", e))?
    } else {
        let sql = format!(
            "{} WHERE rl.id_revision = $1 {}",
            base_select, order_clause
        );
        client.query(&sql, &[&id_revision])
            .map_err(|e| format!("Error consultando filas de reportes_liquidaciones: {}", e))?
    };

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

        let obra_label = if let Some(_) = id_obra_opt {
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

        result.push(ReporteLiquidacionRowDb {
            id_reporte_liq: id_rep_liq,
            id_parent_reporte_liq: id_parent_rep,
            id_liquidacion: id_liq,
            id_empleado: id_emp,
            nombre_trabajador: nom.trim().to_string(),
            rfc: rfc.trim().to_string(),
            curp: curp.trim().to_string(),
            nss: nss.trim().to_string(),
            registro_patronal: rp.trim().to_string(),
            mes_periodo: mes_clean.to_string(),
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
        });
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
    let mut updated_count = 0;

    for &(id_reporte_liq, dias_cedula, dias_obra, id_registro_obra_opt) in rows {
        // 1. Actualizar días cédula, obra y días obra en public.reportes_detalles_liquidaciones para este detalle específico
        tx.execute(
            r#"
            UPDATE public.reportes_detalles_liquidaciones
            SET dias = $2,
                dias_registro_obra = $3,
                id_registro_obra = $4
            WHERE id = $1
            "#,
            &[&id_reporte_liq, &dias_cedula, &Some(dias_obra), &id_registro_obra_opt],
        ).map_err(|e| format!("Error actualizando reporte detalle: {}", e))?;

        // 2. Recalcular conceptos prorrateados para esta fila de detalle específica
        let factor = if dias_cedula > 0 && id_registro_obra_opt.is_some() {
            (dias_obra as f64 / dias_cedula as f64).clamp(0.0, 10.0)
        } else {
            1.0
        };

        tx.execute(
            r#"
            UPDATE public.reportes_detalles_liquidaciones rdl
            SET 
                cf = ROUND((COALESCE(dl.cf, rdl.cf, 0.0) * $2)::numeric, 2)::float8,
                exc_pat = ROUND((COALESCE(dl.exc_pat, rdl.exc_pat, 0.0) * $2)::numeric, 2)::float8,
                exc_obr = ROUND((COALESCE(dl.exc_obr, rdl.exc_obr, 0.0) * $2)::numeric, 2)::float8,
                pd_pat = ROUND((COALESCE(dl.pd_pat, rdl.pd_pat, 0.0) * $2)::numeric, 2)::float8,
                pd_obr = ROUND((COALESCE(dl.pd_obr, rdl.pd_obr, 0.0) * $2)::numeric, 2)::float8,
                gmp_pat = ROUND((COALESCE(dl.gmp_pat, rdl.gmp_pat, 0.0) * $2)::numeric, 2)::float8,
                gmp_obr = ROUND((COALESCE(dl.gmp_obr, rdl.gmp_obr, 0.0) * $2)::numeric, 2)::float8,
                rt = ROUND((COALESCE(dl.rt, rdl.rt, 0.0) * $2)::numeric, 2)::float8,
                iv_pat = ROUND((COALESCE(dl.iv_pat, rdl.iv_pat, 0.0) * $2)::numeric, 2)::float8,
                iv_obr = ROUND((COALESCE(dl.iv_obr, rdl.iv_obr, 0.0) * $2)::numeric, 2)::float8,
                gps = ROUND((COALESCE(dl.gps, rdl.gps, 0.0) * $2)::numeric, 2)::float8,
                patronal = ROUND((COALESCE(dl.patronal, rdl.patronal, 0.0) * $2)::numeric, 2)::float8,
                obrera = ROUND((COALESCE(dl.obrera, rdl.obrera, 0.0) * $2)::numeric, 2)::float8,
                subtotal = ROUND((COALESCE(dl.subtotal, rdl.subtotal, 0.0) * $2)::numeric, 2)::float8
            FROM public.reportes_liquidaciones rl
            LEFT JOIN public.detalles_liquidaciones dl
                ON dl.id_liquidacion = rl.id_liquidacion
               AND (
                   (COALESCE(TRIM(LOWER(dl.clave)), '') = COALESCE(TRIM(LOWER(rdl.clave)), '')
                    AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL)))
                   OR (
                       (SELECT COUNT(*) FROM public.detalles_liquidaciones sub_dl WHERE sub_dl.id_liquidacion = rl.id_liquidacion) = 1
                   )
               )
            WHERE rdl.id_reporte_liquidacion = rl.id 
              AND rdl.id = $1
            "#,
            &[&id_reporte_liq, &factor],
        ).map_err(|e| format!("Error recalculando detalles en tiempo real: {}", e))?;

        // 3. Sincronizar reportes_vinculaciones_obras_liquidaciones para mantener consistencia con nóminas
        let parent_opt: Option<Uuid> = tx.query_opt(
            "SELECT id_reporte_liquidacion FROM public.reportes_detalles_liquidaciones WHERE id = $1",
            &[&id_reporte_liq],
        ).ok().flatten().and_then(|r| r.try_get(0).ok());

        if let Some(parent_id) = parent_opt {
            if let Some(obra_id) = id_registro_obra_opt {
                let dias_obra_val = dias_obra.clamp(0, 31);

                let vinc_exists = tx.query_opt(
                    "SELECT id FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1 LIMIT 1",
                    &[&parent_id],
                ).map_err(|e| format!("Error consultando vinculación: {}", e))?;

                if let Some(vr) = vinc_exists {
                    let vid: Uuid = vr.get(0);
                    tx.execute(
                        r#"
                        UPDATE public.reportes_vinculaciones_obras_liquidaciones
                        SET id_registro_obra = $2, dias_registro_obra = $3
                        WHERE id = $1
                        "#,
                        &[&vid, &Some(obra_id), &Some(dias_obra_val)],
                    ).map_err(|e| format!("Error actualizando vinculación de obra: {}", e))?;
                } else {
                    let new_vinc_id = Uuid::now_v7();
                    tx.execute(
                        r#"
                        INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (
                            id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra
                        ) VALUES ($1, $2, $3, $4)
                        "#,
                        &[&new_vinc_id, &Some(obra_id), &parent_id, &Some(dias_obra_val)],
                    ).map_err(|e| format!("Error creando vinculación de obra: {}", e))?;
                }
            } else {
                let any_other_obra: bool = tx.query_one(
                    "SELECT EXISTS(SELECT 1 FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion = $1 AND id_registro_obra IS NOT NULL)",
                    &[&parent_id],
                ).map(|r| r.get(0)).unwrap_or(false);
                if !any_other_obra {
                    let _ = tx.execute(
                        "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1",
                        &[&parent_id],
                    );
                }
            }
        }

        updated_count += 1;
    }

    tx.commit().map_err(|e| format!("Error en commit de actualización: {}", e))?;
    let _ = crate::db::nominas_reporte::copy_nominas_a_reportes(client);
    Ok(updated_count)
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

/// Actualización masiva por lote para múltiples registros seleccionados
pub fn batch_update_reporte_rows(
    client: &mut Client,
    ids: &[Uuid],
    set_obra_id: Option<Uuid>,
    set_dias_obra: Option<i16>,
    match_dias_cedula: bool,
) -> Result<usize, String> {
    if ids.is_empty() {
        return Ok(0);
    }

    let mut tx = client.transaction().map_err(|e| format!("Error iniciando tx masiva: {}", e))?;

    let mut updated_count = 0;

    for &id_rep_liq in ids {
        // Obtener días de cédula y estado actual sin COALESCE
        let row_opt = tx.query_opt(
            r#"
            SELECT rdl.dias, rvol.id_registro_obra, rvol.dias_registro_obra
            FROM public.reportes_liquidaciones rl
            LEFT JOIN public.reportes_detalles_liquidaciones rdl ON rdl.id_reporte_liquidacion = rl.id
            LEFT JOIN public.reportes_vinculaciones_obras_liquidaciones rvol ON rvol.id_reporte_liquidacion = rl.id
            WHERE rl.id = $1
            LIMIT 1
            "#,
            &[&id_rep_liq],
        ).map_err(|e| format!("Error consultando fila para actualización masiva: {}", e))?;

        if let Some(r) = row_opt {
            let dias_ced: i16 = row_get_i16(&r, 0, 0);
            let curr_obra: Option<Uuid> = r.try_get(1).ok().flatten();
            let curr_dias_obra: i16 = row_get_i16(&r, 2, 0);

            let new_obra = if set_obra_id.is_some() {
                set_obra_id
            } else {
                curr_obra
            };

            let new_dias_obra = if match_dias_cedula {
                dias_ced
            } else if let Some(d) = set_dias_obra {
                d
            } else {
                curr_dias_obra
            };

            if let Some(obra_id) = new_obra {
                let dias_obra_val = new_dias_obra.clamp(0, 31);

                let vinc_exists = tx.query_opt(
                    "SELECT id FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1 LIMIT 1",
                    &[&id_rep_liq],
                ).map_err(|e| format!("Error verificando vinculación: {}", e))?;

                if let Some(vr) = vinc_exists {
                    let vid: Uuid = match vr.try_get(0) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    tx.execute(
                        "UPDATE public.reportes_vinculaciones_obras_liquidaciones SET id_registro_obra = $2, dias_registro_obra = $3 WHERE id = $1",
                        &[&vid, &Some(obra_id), &Some(dias_obra_val)],
                    ).map_err(|e| format!("Error actualizando vinculación masiva: {}", e))?;
                } else {
                    let new_vinc_id = Uuid::now_v7();
                    tx.execute(
                        "INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra) VALUES ($1, $2, $3, $4)",
                        &[&new_vinc_id, &Some(obra_id), &id_rep_liq, &Some(dias_obra_val)],
                    ).map_err(|e| format!("Error creando vinculación masiva: {}", e))?;
                }
            } else {
                tx.execute(
                    "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1",
                    &[&id_rep_liq],
                ).map_err(|e| format!("Error eliminando vinculación masiva: {}", e))?;
            }

            // Recalcular prorrateo
            let factor = if dias_ced > 0 && new_obra.is_some() {
                (new_dias_obra as f64 / dias_ced as f64).clamp(0.0, 10.0)
            } else {
                1.0
            };

            tx.execute(
                r#"
                UPDATE public.reportes_detalles_liquidaciones rdl
                SET 
                    cf = ROUND((COALESCE(dl.cf, rdl.cf, 0.0) * $2)::numeric, 2)::float8,
                    exc_pat = ROUND((COALESCE(dl.exc_pat, rdl.exc_pat, 0.0) * $2)::numeric, 2)::float8,
                    exc_obr = ROUND((COALESCE(dl.exc_obr, rdl.exc_obr, 0.0) * $2)::numeric, 2)::float8,
                    pd_pat = ROUND((COALESCE(dl.pd_pat, rdl.pd_pat, 0.0) * $2)::numeric, 2)::float8,
                    pd_obr = ROUND((COALESCE(dl.pd_obr, rdl.pd_obr, 0.0) * $2)::numeric, 2)::float8,
                    gmp_pat = ROUND((COALESCE(dl.gmp_pat, rdl.gmp_pat, 0.0) * $2)::numeric, 2)::float8,
                    gmp_obr = ROUND((COALESCE(dl.gmp_obr, rdl.gmp_obr, 0.0) * $2)::numeric, 2)::float8,
                    rt = ROUND((COALESCE(dl.rt, rdl.rt, 0.0) * $2)::numeric, 2)::float8,
                    iv_pat = ROUND((COALESCE(dl.iv_pat, rdl.iv_pat, 0.0) * $2)::numeric, 2)::float8,
                    iv_obr = ROUND((COALESCE(dl.iv_obr, rdl.iv_obr, 0.0) * $2)::numeric, 2)::float8,
                    gps = ROUND((COALESCE(dl.gps, rdl.gps, 0.0) * $2)::numeric, 2)::float8,
                    patronal = ROUND((COALESCE(dl.patronal, rdl.patronal, 0.0) * $2)::numeric, 2)::float8,
                    obrera = ROUND((COALESCE(dl.obrera, rdl.obrera, 0.0) * $2)::numeric, 2)::float8,
                    subtotal = ROUND((COALESCE(dl.subtotal, rdl.subtotal, 0.0) * $2)::numeric, 2)::float8
                FROM public.reportes_liquidaciones rl
                LEFT JOIN public.detalles_liquidaciones dl
                    ON dl.id_liquidacion = rl.id_liquidacion
                   AND (
                       (COALESCE(TRIM(LOWER(dl.clave)), '') = COALESCE(TRIM(LOWER(rdl.clave)), '')
                        AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL)))
                       OR (
                           (SELECT COUNT(*) FROM public.detalles_liquidaciones sub_dl WHERE sub_dl.id_liquidacion = rl.id_liquidacion) = 1
                       )
                   )
                WHERE rdl.id_reporte_liquidacion = rl.id 
                  AND rl.id = $1
                "#,
                &[&id_rep_liq, &factor],
            ).map_err(|e| format!("Error recalculando detalles en lote: {}", e))?;

            updated_count += 1;
        }
    }

    tx.commit().map_err(|e| format!("Error en commit de lote masivo: {}", e))?;
    let _ = crate::db::nominas_reporte::copy_nominas_a_reportes(client);
    Ok(updated_count)
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

/// Genera la exportación en Excel (.xlsx) con el formato profesional de la vista
pub fn export_reporte_liquidaciones_to_excel(
    rows: &[ReporteLiquidacionRowDb],
    all_rows: &[ReporteLiquidacionRowDb],
    valid_user_obra_ids: Option<&HashSet<Uuid>>,
    guarderias_kpis: &[ReporteGuarderiaKpiDb],
    file_path: &Path,
    cliente_nombre: &str,
    rp_registro: &str,
    revision_nombre: &str,
) -> Result<(), String> {
    use rust_xlsxwriter::{Color, Format, FormatBorder, Workbook};

    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Prorrateo por Obra").map_err(|e| e.to_string())?;

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
    worksheet.write_string_with_format(1, 0, &format!("Empresa: {} | Registro Patronal: {} | Revisión: {}", cliente_nombre, rp_registro, revision_nombre), &subtitle_format).map_err(|e| e.to_string())?;
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

    worksheet.write_number_with_format(current_r, 11, g_dias_ced, &grand_total_int_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 12, g_dias_obr, &grand_total_int_format).map_err(|e| e.to_string())?;
    if g_lic > 0.001 {
        worksheet.write_number_with_format(current_r, 13, g_lic, &grand_total_int_format).map_err(|e| e.to_string())?;
    } else {
        worksheet.write_string_with_format(current_r, 13, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
    }
    if g_inc > 0.001 {
        worksheet.write_number_with_format(current_r, 14, g_inc, &grand_total_int_format).map_err(|e| e.to_string())?;
    } else {
        worksheet.write_string_with_format(current_r, 14, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
    }
    if g_aus > 0.001 {
        worksheet.write_number_with_format(current_r, 15, g_aus, &grand_total_int_format).map_err(|e| e.to_string())?;
    } else {
        worksheet.write_string_with_format(current_r, 15, "-", &grand_total_label_format).map_err(|e| e.to_string())?;
    }

    worksheet.write_number_with_format(current_r, 16, g_cf, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 17, g_exc_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 18, g_exc_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 19, g_pd_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 20, g_pd_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 21, g_gmp_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 22, g_gmp_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 23, g_rt, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 24, g_iv_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 25, g_iv_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 26, g_gps, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 27, g_patronal, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 28, g_obrera, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 29, g_subtotal, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 30, g_subtotal_orig, &grand_total_curr_format).map_err(|e| e.to_string())?;

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

            worksheet.write_number_with_format(kpi_r, 3, kpi.monto_obra, &kpi_curr_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 4, kpi.limite_18_obra, &kpi_green_curr_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 5, kpi.total_gps, &kpi_blue_curr_format)
                .map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 6, kpi.empleados_count as f64, &kpi_num_format)
                .map_err(|e| e.to_string())?;

            if kpi.supera_18 {
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
            worksheet.write_number_with_format(kpi_r, 1, kpi.monto_obra, &kpi_curr_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 2, kpi.limite_18_obra, &kpi_green_curr_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(kpi_r, 3, kpi.total_gps, &kpi_blue_curr_format).map_err(|e| e.to_string())?;

            kpi_r += 2;
        }
    }

    worksheet.autofit();

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
            &format!("Empresa: {} | Registro Patronal: {} | Revisión: {}", cliente_nombre, rp_registro, revision_nombre),
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
    }

    // =========================================================================
    // SHEET 3: Totales por Empleado (Todos los registros de obra de la revisión)
    // =========================================================================
    {
        let ws_emp = workbook.add_worksheet();
        ws_emp.set_name("Totales por Empleado").map_err(|e| e.to_string())?;

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
            &format!("Empresa: {} | Registro Patronal: {} | Revisión: {}", cliente_nombre, rp_registro, revision_nombre),
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
    }

    // =========================================================================
    // SHEET 4: Análisis de Guarderías (Tope 18% por Registro de Obra)
    // =========================================================================
    if !guarderias_kpis.is_empty() {
        let ws_guard = workbook.add_worksheet();
        ws_guard.set_name("Análisis de Guarderías (18%)").map_err(|e| e.to_string())?;

        ws_guard.write_string_with_format(
            0,
            0,
            "Reporte de Liquidaciones - Análisis de Guarderías (Tope 18%) - CONSTRUAUDIT",
            &title_format,
        ).map_err(|e| e.to_string())?;

        ws_guard.write_string_with_format(
            1,
            0,
            &format!("Empresa: {} | Registro Patronal: {} | Revisión: {}", cliente_nombre, rp_registro, revision_nombre),
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

            ws_guard.write_number_with_format(curr_r_s4, 3, kpi.monto_obra, &currency_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 4, kpi.limite_18_obra, &kpi_green_curr_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 5, kpi.total_gps, &kpi_blue_curr_format)
                .map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 6, kpi.empleados_count as f64, &integer_format)
                .map_err(|e| e.to_string())?;

            if kpi.supera_18 {
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
            ws_guard.write_number_with_format(curr_r_s4, 1, kpi.monto_obra, &currency_format).map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 2, kpi.limite_18_obra, &kpi_green_curr_format).map_err(|e| e.to_string())?;
            ws_guard.write_number_with_format(curr_r_s4, 3, kpi.total_gps, &kpi_blue_curr_format).map_err(|e| e.to_string())?;

            curr_r_s4 += 2;
        }

        ws_guard.autofit();
    }

    workbook.save(file_path).map_err(|e| format!("Error guardando archivo Excel: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_export_reporte_liquidaciones_excel_multi_sheet() {
        let obra_user_1 = Uuid::new_v4();
        let obra_user_2 = Uuid::new_v4();
        let obra_cedula = Uuid::new_v4();

        let emp_1 = Uuid::new_v4();
        let emp_2 = Uuid::new_v4();
        let emp_3 = Uuid::new_v4();

        let mut valid_obras = HashSet::new();
        valid_obras.insert(obra_user_1);
        valid_obras.insert(obra_user_2);

        let make_row = |emp_id: Uuid, nombre: &str, oid: Option<Uuid>, obra_num: &str, is_user: bool, mes: &str, anio: i16, dias: i16, sub: f64| -> ReporteLiquidacionRowDb {
            ReporteLiquidacionRowDb {
                id_reporte_liq: Uuid::new_v4(),
                id_parent_reporte_liq: Uuid::new_v4(),
                id_liquidacion: Uuid::new_v4(),
                id_empleado: emp_id,
                nombre_trabajador: nombre.to_string(),
                rfc: "XAXX010101000".to_string(),
                curp: "XAXX010101HDF00".to_string(),
                nss: "12345678901".to_string(),
                registro_patronal: "Y5410204108".to_string(),
                mes_periodo: mes.to_string(),
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
            }
        };

        let rows = vec![
            // Emp 1 en Obra 1 (Usuario) - Mes 1
            make_row(emp_1, "PUGA LOPEZ JOSE MARIA", Some(obra_user_1), "OBRA-USER-01", true, "ENERO", 2024, 30, 1000.0),
            // Emp 1 en Obra 1 (Usuario) - Mes 2
            make_row(emp_1, "PUGA LOPEZ JOSE MARIA", Some(obra_user_1), "OBRA-USER-01", true, "FEBRERO", 2024, 28, 900.0),
            // Emp 1 en Obra 2 (Usuario) - Mes 2
            make_row(emp_1, "PUGA LOPEZ JOSE MARIA", Some(obra_user_2), "OBRA-USER-02", true, "FEBRERO", 2024, 15, 500.0),
            // Emp 2 en Obra Cédula (NO Usuario) - debe ser omitido en Sheets 2 y 3
            make_row(emp_2, "LOPEZ PEREZ JUAN", Some(obra_cedula), "OBRA-CEDULA-AUTO", false, "ENERO", 2024, 30, 800.0),
            // Emp 3 Sin Obra - debe ser omitido en Sheets 2 y 3
            make_row(emp_3, "GONZALEZ HERNANDEZ PEDRO", None, "", false, "ENERO", 2024, 30, 700.0),
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
        let test_file = temp_dir.join(format!("test_export_liq_{}.xlsx", Uuid::new_v4()));

        let res = export_reporte_liquidaciones_to_excel(
            &rows,
            &rows,
            Some(&valid_obras),
            &guarderias_kpis,
            &test_file,
            "EMPRESA PRUEBA S.A. DE C.V.",
            "Y54-10204-10-8",
            "REVISION 2024-01",
        );

        assert!(res.is_ok(), "El reporte en Excel debe generarse correctamente: {:?}", res.err());
        assert!(test_file.exists(), "El archivo Excel generado debe existir físicamente");
        let metadata = std::fs::metadata(&test_file).unwrap();
        assert!(metadata.len() > 1000, "El archivo Excel generado debe contener datos");

        let _ = std::fs::remove_file(test_file);
    }
}


