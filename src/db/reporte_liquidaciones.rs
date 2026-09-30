use chrono::{Datelike, NaiveDate};
use postgres::Client;
use std::collections::{BTreeSet, HashSet};
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
}

#[derive(Debug, Clone)]
pub struct ReporteLiquidacionRowDb {
    pub id_reporte_liq: Uuid,
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
        SELECT ro.id, ro.numero, ro.tipo_obra, ro.fecha_inicio, ro.fecha_fin
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
            None => (None, None),
        };

        if let Some(rv) = rep_vinc_opt {
            let vid: Uuid = rv.get(0);
            let curr_rep_obra: Option<Uuid> = rv.try_get(1).ok().flatten();
            // Preservar los cambios que el usuario haya realizado en la vista de Reporte Liquidaciones.
            // Si la obra vinculada ya no pertenece a las obras válidas o es nula, eliminar la vinculación por completo
            if let Some(curr_obra) = curr_rep_obra {
                if !valid_obras_set.contains(&curr_obra) {
                    tx.execute(
                        "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id = $1",
                        &[&vid],
                    ).map_err(|e| format!("Error eliminando vinculación con obra eliminada: {}", e))?;
                }
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

                tx.execute(
                    r#"
                    INSERT INTO public.reportes_detalles_liquidaciones (
                        id, id_reporte_liquidacion, clave, fecha, dias, sdi, lic, inc, aus, cf,
                        exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr,
                        gps, patronal, obrera, subtotal
                    ) VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23
                    )
                    "#,
                    &[
                        &id_rep_det, &id_rep_liq, &clave, &fecha, &dias_det, &sdi, &lic, &inc, &aus,
                        &calc2(cf), &calc2(exc_pat), &calc2(exc_obr), &calc2(pd_pat), &calc2(pd_obr),
                        &calc2(gmp_pat), &calc2(gmp_obr), &calc2(rt), &calc2(iv_pat), &calc2(iv_obr),
                        &calc2(gps), &calc2(patronal), &calc2(obrera), &calc2(subtotal)
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
        rl.id AS id_reporte_liq,
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
        rvol.id_registro_obra,
        ro.numero AS obra_numero,
        ro.tipo_obra AS tipo_obra,
        rvol.dias_registro_obra AS dias_obra,
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
        dl.cf AS cf_orig,
        dl.exc_pat AS exc_pat_orig,
        dl.exc_obr AS exc_obr_orig,
        dl.pd_pat AS pd_pat_orig,
        dl.pd_obr AS pd_obr_orig,
        dl.gmp_pat AS gmp_pat_orig,
        dl.gmp_obr AS gmp_obr_orig,
        dl.rt AS rt_orig,
        dl.iv_pat AS iv_pat_orig,
        dl.iv_obr AS iv_obr_orig,
        dl.gps AS gps_orig,
        dl.patronal AS patronal_orig,
        dl.obrera AS obrera_orig,
        dl.subtotal AS subtotal_orig
    FROM public.reportes_liquidaciones AS rl
    INNER JOIN public.reportes_detalles_liquidaciones AS rdl
        ON rl.id = rdl.id_reporte_liquidacion
    LEFT JOIN public.reportes_vinculaciones_obras_liquidaciones AS rvol
        ON rl.id = rvol.id_reporte_liquidacion
    LEFT JOIN public.detalles_liquidaciones AS dl
        ON (dl.id_liquidacion = rl.id_liquidacion 
            AND (dl.clave = rdl.clave OR (dl.clave IS NULL AND rdl.clave IS NULL)) 
            AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL)))
    JOIN public.empleados AS e
        ON rl.id_empleado = e.id
    JOIN public.revisiones AS rev
        ON rl.id_revision = rev.id
    JOIN public.registros_patronales AS rp
        ON rev.id_registro_patronal = rp.id
    LEFT JOIN public.registros_obras AS ro
        ON rvol.id_registro_obra = ro.id
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

        result.push(ReporteLiquidacionRowDb {
            id_reporte_liq: id_rep_liq,
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
        // 1. Actualizar días cédula en reportes_detalles_liquidaciones
        tx.execute(
            "UPDATE public.reportes_detalles_liquidaciones SET dias = $2 WHERE id_reporte_liquidacion = $1",
            &[&id_reporte_liq, &dias_cedula],
        ).map_err(|e| format!("Error actualizando días en reportes_detalles_liquidaciones: {}", e))?;

        // 2. Actualizar vinculación en reportes_vinculaciones_obras_liquidaciones
        if let Some(obra_id) = id_registro_obra_opt {
            let dias_obra_val = if dias_obra > 0 {
                dias_obra.clamp(1, 31)
            } else {
                dias_cedula.clamp(1, 31)
            };

            let vinc_exists = tx.query_opt(
                "SELECT id FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1 LIMIT 1",
                &[&id_reporte_liq],
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
                    &[&new_vinc_id, &Some(obra_id), &id_reporte_liq, &Some(dias_obra_val)],
                ).map_err(|e| format!("Error creando vinculación de obra: {}", e))?;
            }
        } else {
            // Sin obra asignada: eliminar cualquier relación existente en la tabla
            tx.execute(
                "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1",
                &[&id_reporte_liq],
            ).map_err(|e| format!("Error eliminando vinculación de obra: {}", e))?;
        }

        // 3. Recalcular conceptos en public.reportes_detalles_liquidaciones
        let factor = if dias_cedula > 0 && id_registro_obra_opt.is_some() {
            (dias_obra as f64 / dias_cedula as f64).clamp(0.0, 10.0)
        } else {
            1.0
        };

        tx.execute(
            r#"
            UPDATE public.reportes_detalles_liquidaciones rdl
            SET 
                cf = ROUND((dl.cf * $2)::numeric, 2)::float8,
                exc_pat = ROUND((dl.exc_pat * $2)::numeric, 2)::float8,
                exc_obr = ROUND((dl.exc_obr * $2)::numeric, 2)::float8,
                pd_pat = ROUND((dl.pd_pat * $2)::numeric, 2)::float8,
                pd_obr = ROUND((dl.pd_obr * $2)::numeric, 2)::float8,
                gmp_pat = ROUND((dl.gmp_pat * $2)::numeric, 2)::float8,
                gmp_obr = ROUND((dl.gmp_obr * $2)::numeric, 2)::float8,
                rt = ROUND((dl.rt * $2)::numeric, 2)::float8,
                iv_pat = ROUND((dl.iv_pat * $2)::numeric, 2)::float8,
                iv_obr = ROUND((dl.iv_obr * $2)::numeric, 2)::float8,
                gps = ROUND((dl.gps * $2)::numeric, 2)::float8,
                patronal = ROUND((dl.patronal * $2)::numeric, 2)::float8,
                obrera = ROUND((dl.obrera * $2)::numeric, 2)::float8,
                subtotal = ROUND((dl.subtotal * $2)::numeric, 2)::float8
            FROM public.reportes_liquidaciones rl,
                 public.detalles_liquidaciones dl
            WHERE rdl.id_reporte_liquidacion = rl.id 
              AND dl.id_liquidacion = rl.id_liquidacion
              AND (dl.clave = rdl.clave OR (dl.clave IS NULL AND rdl.clave IS NULL))
              AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL))
              AND rl.id = $1
            "#,
            &[&id_reporte_liq, &factor],
        ).map_err(|e| format!("Error recalculando detalles en tiempo real: {}", e))?;

        updated_count += 1;
    }

    tx.commit().map_err(|e| format!("Error en commit de actualización: {}", e))?;
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
                let dias_obra_val = if new_dias_obra > 0 {
                    new_dias_obra.clamp(1, 31)
                } else {
                    dias_ced.clamp(1, 31)
                };

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
                    cf = ROUND((dl.cf * $2)::numeric, 2)::float8,
                    exc_pat = ROUND((dl.exc_pat * $2)::numeric, 2)::float8,
                    exc_obr = ROUND((dl.exc_obr * $2)::numeric, 2)::float8,
                    pd_pat = ROUND((dl.pd_pat * $2)::numeric, 2)::float8,
                    pd_obr = ROUND((dl.pd_obr * $2)::numeric, 2)::float8,
                    gmp_pat = ROUND((dl.gmp_pat * $2)::numeric, 2)::float8,
                    gmp_obr = ROUND((dl.gmp_obr * $2)::numeric, 2)::float8,
                    rt = ROUND((dl.rt * $2)::numeric, 2)::float8,
                    iv_pat = ROUND((dl.iv_pat * $2)::numeric, 2)::float8,
                    iv_obr = ROUND((dl.iv_obr * $2)::numeric, 2)::float8,
                    gps = ROUND((dl.gps * $2)::numeric, 2)::float8,
                    patronal = ROUND((dl.patronal * $2)::numeric, 2)::float8,
                    obrera = ROUND((dl.obrera * $2)::numeric, 2)::float8,
                    subtotal = ROUND((dl.subtotal * $2)::numeric, 2)::float8
                FROM public.reportes_liquidaciones rl,
                     public.detalles_liquidaciones dl
                WHERE rdl.id_reporte_liquidacion = rl.id 
                  AND dl.id_liquidacion = rl.id_liquidacion
                  AND (dl.clave = rdl.clave OR (dl.clave IS NULL AND rdl.clave IS NULL))
                  AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL))
                  AND rl.id = $1
                "#,
                &[&id_rep_liq, &factor],
            ).map_err(|e| format!("Error recalculando detalles en lote: {}", e))?;

            updated_count += 1;
        }
    }

    tx.commit().map_err(|e| format!("Error en commit de lote masivo: {}", e))?;
    Ok(updated_count)
}

/// Genera la exportación en Excel (.xlsx) con el formato profesional de la vista
pub fn export_reporte_liquidaciones_to_excel(
    rows: &[ReporteLiquidacionRowDb],
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
        "Registro de Obra", "Días Cédula", "Días Obra",
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
            // Reg. Patronal en blanco si está sin asignar o pendiente; traer solamente el registro patronal, no el texto complementario
            let rp_clean = if row.registro_patronal.trim().is_empty()
                || row.registro_patronal.to_lowercase().contains("sin asignar")
                || row.registro_patronal.to_lowercase().contains("pendiente")
            {
                ""
            } else {
                let s = row.registro_patronal.trim();
                if let Some((code, _)) = s.split_once(" - ") {
                    code.trim()
                } else if let Some((code, _)) = s.split_once(' ') {
                    code.trim()
                } else {
                    s
                }
            };

            // Registro de Obra en blanco si no tiene asignación; traer solamente el número de registro de obra, no el texto complementario
            let obra_clean = if row.id_registro_obra.is_none()
                || row.obra_label.to_lowercase().contains("sin asignar")
                || row.obra_label.to_lowercase().contains("pendiente")
            {
                ""
            } else if !row.obra_numero.trim().is_empty() {
                row.obra_numero.trim()
            } else if let Some((num, _)) = row.obra_label.split_once(" - ") {
                num.trim()
            } else {
                row.obra_label.trim()
            };

            worksheet.write_number_with_format(current_r, 0, global_idx as f64, &integer_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 1, &row.periodo_str, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 2, &row.nss, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 3, &row.rfc, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 4, &row.curp, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 5, &row.nombre_trabajador, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 6, rp_clean, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_string_with_format(current_r, 7, obra_clean, &text_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 8, row.dias_cedula as f64, &integer_format).map_err(|e| e.to_string())?;
            if row.id_registro_obra.is_some() {
                worksheet.write_number_with_format(current_r, 9, row.dias_obra as f64, &integer_format).map_err(|e| e.to_string())?;
                m_dias_obr += row.dias_obra as f64;
            } else {
                worksheet.write_string_with_format(current_r, 9, "", &text_format).map_err(|e| e.to_string())?;
            }

            worksheet.write_number_with_format(current_r, 10, row.cf, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 11, row.exc_pat, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 12, row.exc_obr, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 13, row.pd_pat, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 14, row.pd_obr, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 15, row.gmp_pat, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 16, row.gmp_obr, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 17, row.rt, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 18, row.iv_pat, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 19, row.iv_obr, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 20, row.gps, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 21, row.patronal, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 22, row.obrera, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 23, row.subtotal, &currency_format).map_err(|e| e.to_string())?;
            worksheet.write_number_with_format(current_r, 24, row.subtotal_orig, &currency_format).map_err(|e| e.to_string())?;

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
        worksheet.write_string_with_format(current_r, 6, "", &month_total_label_format).map_err(|e| e.to_string())?;
        worksheet.write_string_with_format(current_r, 7, "", &month_total_label_format).map_err(|e| e.to_string())?;

        worksheet.write_number_with_format(current_r, 8, m_dias_ced, &month_total_int_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 9, m_dias_obr, &month_total_int_format).map_err(|e| e.to_string())?;

        worksheet.write_number_with_format(current_r, 10, m_cf, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 11, m_exc_pat, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 12, m_exc_obr, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 13, m_pd_pat, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 14, m_pd_obr, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 15, m_gmp_pat, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 16, m_gmp_obr, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 17, m_rt, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 18, m_iv_pat, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 19, m_iv_obr, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 20, m_gps, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 21, m_patronal, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 22, m_obrera, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 23, m_subtotal, &month_total_curr_format).map_err(|e| e.to_string())?;
        worksheet.write_number_with_format(current_r, 24, m_subtotal_orig, &month_total_curr_format).map_err(|e| e.to_string())?;

        current_r += 1;

        // Acumular a Total General
        g_dias_ced += m_dias_ced;
        g_dias_obr += m_dias_obr;
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
    worksheet.write_string_with_format(current_r, 6, "", &grand_total_label_format).map_err(|e| e.to_string())?;
    worksheet.write_string_with_format(current_r, 7, "", &grand_total_label_format).map_err(|e| e.to_string())?;

    worksheet.write_number_with_format(current_r, 8, g_dias_ced, &grand_total_int_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 9, g_dias_obr, &grand_total_int_format).map_err(|e| e.to_string())?;

    worksheet.write_number_with_format(current_r, 10, g_cf, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 11, g_exc_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 12, g_exc_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 13, g_pd_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 14, g_pd_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 15, g_gmp_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 16, g_gmp_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 17, g_rt, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 18, g_iv_pat, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 19, g_iv_obr, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 20, g_gps, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 21, g_patronal, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 22, g_obrera, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 23, g_subtotal, &grand_total_curr_format).map_err(|e| e.to_string())?;
    worksheet.write_number_with_format(current_r, 24, g_subtotal_orig, &grand_total_curr_format).map_err(|e| e.to_string())?;

    worksheet.autofit();
    workbook.save(file_path).map_err(|e| format!("Error guardando archivo Excel: {}", e))?;
    Ok(())
}
