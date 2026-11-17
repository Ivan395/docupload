use chrono::{Datelike, NaiveDate, Utc};
use postgres::{Client, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::cedulas::business::{month_num_to_name, month_to_num, Business};
use crate::cedulas::employees::Employee as CedulaEmployee;

#[derive(Debug, Default, Clone)]
pub struct CedulasDbStats {
    pub liquidaciones_guardadas: usize,
    pub detalles_guardados: usize,
    pub vinculaciones_guardadas: usize,
    pub empleados_guardados: usize,
    pub registros_obras_guardados: usize,
    pub errores: Vec<String>,
    pub id_registro_patronal: Option<Uuid>,
    pub id_revision: Option<Uuid>,
}

/// Días de obra válidos según la cédula: 1..=31. Fuera de rango → None (dato ilegible, no se inventa).
pub fn dias_obra_validos(dias: i32) -> Option<i16> {
    if (1..=31).contains(&dias) {
        Some(dias as i16)
    } else {
        None
    }
}

/// Suma días de obra consolidados por obra, limitando el total a 31 sin imponer mínimo artificial.
pub fn sumar_dias_obra(acumulado: i16, dias: i16) -> i16 {
    acumulado.saturating_add(dias).min(31)
}

fn truncate_str(s: Option<&str>, max_len: usize) -> Option<String> {
    s.map(|val| {
        let trimmed = val.trim();
        if trimmed.chars().count() > max_len {
            trimmed.chars().take(max_len).collect()
        } else {
            trimmed.to_string()
        }
    })
}

pub fn normalize_mes_periodo(mes_opt: Option<&str>) -> String {
    let raw = mes_opt.unwrap_or("ENERO").trim();
    let num = month_to_num(raw);
    let valid_num = if (1..=12).contains(&num) { num } else { 1 };
    month_num_to_name(valid_num).to_string()
}

pub fn normalize_anio_periodo(anio_opt: Option<&str>) -> i16 {
    let curr_year = Utc::now().year() as i16;
    if let Some(s) = anio_opt {
        if let Ok(y) = s.trim().parse::<i16>() {
            if (1990..=2099).contains(&y) {
                return y;
            }
        }
    }
    curr_year
}

/// Resuelve (mes 1..=12, año 1990..=2099) de una cédula SIN valores por defecto.
/// Devuelve None si el mes o el año no se pueden leer; el llamador debe rechazar la cédula.
pub fn resolve_periodo_cedula(emp: &CedulaEmployee) -> Option<(i16, i16)> {
    let first = emp.data_employee.first();
    let en_rango_mes = |v: i16| (1..=12).contains(&v);

    // Mes: entero ya detectado en el empleado, luego texto, luego entero del primer detalle
    let mes = if en_rango_mes(emp.mes_periodo_int) {
        Some(emp.mes_periodo_int)
    } else {
        emp.mes_periodo
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| first.and_then(|de| de.mes_periodo.as_deref()))
            .map(|s| month_to_num(s.trim()))
            .filter(|n| (1..=12).contains(n))
            .map(|n| n as i16)
            .or_else(|| first.map(|de| de.mes_periodo_int).filter(|v| en_rango_mes(*v)))
    }?;

    // Año: texto del empleado o del primer detalle, solo 1990..=2099
    let anio = emp
        .anio_periodo
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| first.and_then(|de| de.anio_periodo.as_deref()))?
        .trim()
        .parse::<i16>()
        .ok()
        .filter(|y| (1990..=2099).contains(y))?;

    Some((mes, anio))
}

#[derive(Clone)]
pub struct BusinessContext {
    pub id_cliente: Uuid,
    pub id_registro_patronal: Uuid,
    pub id_revision: Uuid,
    pub reg_11: String,
    pub delegacion: Option<String>,
    pub subdelegacion: Option<String>,
    pub anios_filtro: BTreeSet<i16>,
    pub meses_filtro: BTreeSet<String>,
    pub periodos_filtro: BTreeSet<String>,
    pub periodos_int_filtro: BTreeSet<(i16, i16)>,
}

pub fn resolve_business_context(
    tx: &mut Transaction,
    biz: &Business,
) -> Result<BusinessContext, Box<dyn std::error::Error + Send + Sync>> {
    resolve_business_context_for_revision(tx, biz, None)
}

fn update_cliente_and_rp_from_biz(
    tx: &mut Transaction,
    id_cliente: Uuid,
    id_registro_patronal: Uuid,
    biz: &Business,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Actualizar Registro Patronal con la información obtenida de las cédulas
    let act_upper = biz.actividad.as_deref().map(|s| crate::cedulas::business::clean_actividad(s)).filter(|s| !s.is_empty());
    let dom_upper = biz.domicilio.as_deref().map(|s| crate::cedulas::business::clean_domicilio(s)).filter(|s| !s.is_empty());
    let cp_upper = biz.codigo_postal.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let ent_upper = biz.entidad.as_deref().map(|s| crate::cedulas::business::clean_entidad(s)).filter(|s| !s.is_empty());
    let area_upper = biz.area_geografica.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let del_upper = biz.delegacion.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let subdel_upper = biz.subdelegacion.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let cod_del_upper = biz.codigo_delegacion.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let cod_subdel_upper = biz.codigo_subdelegacion.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let prima_float = biz.prima.as_deref().and_then(|s| s.trim().parse::<f64>().ok());

    tx.execute(
        r#"
        UPDATE public.registros_patronales
        SET actividad = COALESCE($2, actividad),
            domicilio = COALESCE($3, domicilio),
            codigo_postal = COALESCE($4, codigo_postal),
            entidad = COALESCE($5, entidad),
            area_geografica = COALESCE($6, area_geografica),
            delegacion = COALESCE($7, delegacion),
            subdelegacion = COALESCE($8, subdelegacion),
            codigo_delegacion = COALESCE($9, codigo_delegacion),
            codigo_subdelegacion = COALESCE($10, codigo_subdelegacion),
            prima_riesgo = COALESCE($11, prima_riesgo)
        WHERE id = $1
        "#,
        &[
            &id_registro_patronal,
            &truncate_str(act_upper.as_deref(), 300),
            &truncate_str(dom_upper.as_deref(), 500),
            &truncate_str(cp_upper.as_deref(), 5),
            &truncate_str(ent_upper.as_deref(), 100),
            &truncate_str(area_upper.as_deref(), 5),
            &truncate_str(del_upper.as_deref(), 100),
            &truncate_str(subdel_upper.as_deref(), 100),
            &truncate_str(cod_del_upper.as_deref(), 10),
            &truncate_str(cod_subdel_upper.as_deref(), 10),
            &prima_float,
        ],
    ).inspect_err(|e| {
        let err_str = crate::db::schema::format_pg_error(e);
        crate::app_logger::log_db_error(
            "UPDATE registros_patronales (metadata cédula)",
            &["public.registros_patronales"],
            "UPDATE public.registros_patronales SET actividad = COALESCE($2, actividad) ...",
            &err_str,
        );
    })?;

    // 2. Actualizar Cliente
    let rz_upper = biz
        .razon_social
        .as_deref()
        .map(|s| s.trim().to_uppercase())
        .filter(|s| {
            !s.is_empty()
                && !s.contains("BIMESTRE DE PROCESO")
                && !s.contains("FECHA DE PROCESO")
                && !s.contains("PERIODO DE PROCESO")
        });

    // RFC: si se extrajo de la cédula, validar si no entra en conflicto con otro cliente
    let rfc_c = biz.rfc.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    }).filter(|s| !s.is_empty());

    let mut update_rfc: Option<String> = None;
    if let Some(ref rfc_str) = rfc_c {
        let rfc_trunc = truncate_str(Some(rfc_str), 13).unwrap_or_default();
        let dup_rfc = tx.query_opt(
            "SELECT id FROM public.clientes WHERE TRIM(rfc) = TRIM($1) AND id != $2",
            &[&rfc_trunc, &id_cliente],
        )?;
        if dup_rfc.is_none() {
            update_rfc = Some(rfc_trunc);
        }
    }

    // CURP: Solo si aparece en las cédulas (18 caracteres)
    let curp_c = biz.curp.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    }).filter(|s| s.len() == 18);

    let mut update_curp: Option<String> = None;
    if let Some(ref curp_str) = curp_c {
        let curp_trunc = truncate_str(Some(curp_str), 18).unwrap_or_default();
        let dup_curp = tx.query_opt(
            "SELECT id FROM public.clientes WHERE TRIM(curp) = TRIM($1) AND id != $2",
            &[&curp_trunc, &id_cliente],
        )?;
        if dup_curp.is_none() {
            update_curp = Some(curp_trunc);
        }
    }

    // Tipo Persona:
    // • Persona Física: El RFC tiene 13 caracteres (4 letras y 9 números).
    // • Persona Moral: El RFC tiene 12 caracteres (3 letras y 9 números).
    let effective_rfc = match update_rfc.clone().or_else(|| rfc_c.clone()) {
        Some(r) => Some(r),
        None => {
            let row = tx.query_opt("SELECT rfc FROM public.clientes WHERE id = $1", &[&id_cliente])?;
            row.and_then(|r| r.get::<_, Option<String>>(0))
                .map(|s| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase())
        }
    };

    let target_tipo = match effective_rfc.as_deref().map(|s| s.len()) {
        Some(13) => Some("FISICA"),
        Some(12) => Some("MORAL"),
        _ => None,
    };

    let mut target_cc_id: Option<Uuid> = None;
    if let Some(tipo_str) = target_tipo {
        let tp_id_opt: Option<Uuid> = if let Ok(Some(tp_row)) = tx.query_opt(
            "SELECT id FROM public.tipos_personas WHERE TRIM(tipo) ILIKE $1 LIMIT 1",
            &[&format!("%{}%", tipo_str)],
        ) {
            Some(tp_row.get(0))
        } else {
            let nid = Uuid::now_v7();
            let ins_res = tx.query_one(
                "INSERT INTO public.tipos_personas (id, tipo) VALUES ($1, $2) ON CONFLICT (tipo) DO UPDATE SET tipo = EXCLUDED.tipo RETURNING id",
                &[&nid, &tipo_str],
            );
            ins_res.ok().map(|r| r.get(0))
        };

        if let Some(tp_id) = tp_id_opt {
            let cc_row = tx.query_opt(
                "SELECT cc.id_regimen_fiscal FROM public.clientes c LEFT JOIN public.clasificaciones_clientes cc ON cc.id = c.id_clasificacion_cliente WHERE c.id = $1",
                &[&id_cliente],
            )?;
            let rf_id_opt: Option<Uuid> = cc_row.and_then(|r| r.get(0));

            let cc_id: Option<Uuid> = if let Some(rf_id) = rf_id_opt {
                let existing = tx.query_opt(
                    "SELECT id FROM public.clasificaciones_clientes WHERE id_tipo_persona = $1 AND id_regimen_fiscal = $2 LIMIT 1",
                    &[&tp_id, &rf_id],
                )?;
                if let Some(r) = existing {
                    Some(r.get(0))
                } else {
                    let fallback = tx.query_opt(
                        "SELECT id FROM public.clasificaciones_clientes WHERE id_tipo_persona = $1 LIMIT 1",
                        &[&tp_id],
                    )?;
                    fallback.map(|r| r.get(0))
                }
            } else {
                let existing = tx.query_opt(
                    "SELECT id FROM public.clasificaciones_clientes WHERE id_tipo_persona = $1 LIMIT 1",
                    &[&tp_id],
                )?;
                existing.map(|r| r.get(0))
            };

            let cc_id_final = match cc_id {
                Some(id) => Some(id),
                None => {
                    let rf_row = tx.query_opt("SELECT id FROM public.regimenes_fiscales LIMIT 1", &[])?;
                    if let Some(r_rf) = rf_row {
                        let rf_id: Uuid = r_rf.get(0);
                        let nid = Uuid::now_v7();
                        let ins = tx.query_one(
                            "INSERT INTO public.clasificaciones_clientes (id, id_tipo_persona, id_regimen_fiscal) VALUES ($1, $2, $3) ON CONFLICT (id_regimen_fiscal, id_tipo_persona) DO UPDATE SET id_tipo_persona = EXCLUDED.id_tipo_persona RETURNING id",
                            &[&nid, &tp_id, &rf_id],
                        )?;
                        Some(ins.get(0))
                    } else {
                        None
                    }
                }
            };
            target_cc_id = cc_id_final;
        }
    }

    tx.execute(
        r#"
        UPDATE public.clientes
        SET razon_social = COALESCE($2, razon_social),
            rfc = COALESCE($3, rfc),
            curp = COALESCE($4, curp),
            id_clasificacion_cliente = COALESCE($5, id_clasificacion_cliente)
        WHERE id = $1
        "#,
        &[
            &id_cliente,
            &truncate_str(rz_upper.as_deref(), 300),
            &update_rfc,
            &update_curp,
            &target_cc_id,
        ],
    )?;

    Ok(())
}

fn get_obra_filter_dates(
    tx: &mut Transaction,
    id_revision: Uuid,
) -> (BTreeSet<i16>, BTreeSet<String>, BTreeSet<String>, BTreeSet<(i16, i16)>) {
    let obra_dates = tx.query(
        r#"
        SELECT ro.fecha_inicio, ro.fecha_fin
        FROM public.registros_obras ro
        WHERE (ro.id_revision = $1
           OR ro.id_registro_patronal = (SELECT r.id_registro_patronal FROM public.revisiones r WHERE r.id = $1))
          AND COALESCE(ro.creado_usuario, false) = true
          AND ro.activa
        "#,
        &[&id_revision],
    ).unwrap_or_default();

    let mut anios_filtro: BTreeSet<i16> = BTreeSet::new();
    let mut meses_filtro: BTreeSet<String> = BTreeSet::new();
    let mut periodos_filtro: BTreeSet<String> = BTreeSet::new();
    let mut periodos_int_filtro: BTreeSet<(i16, i16)> = BTreeSet::new();

    for od in obra_dates {
        let f_ini: Option<chrono::NaiveDate> = od.get(0);
        let f_fin: Option<chrono::NaiveDate> = od.get(1);
        let f_fin = f_fin.or(f_ini);
        if let (Some(fi), Some(ff)) = (f_ini, f_fin) {
            let ff = ff.max(fi);
            let mut curr_y = fi.year();
            let mut curr_m = fi.month();
            let end_y = ff.year();
            let end_m = ff.month();

            while (curr_y < end_y) || (curr_y == end_y && curr_m <= end_m) {
                anios_filtro.insert(curr_y as i16);
                periodos_int_filtro.insert((curr_y as i16, curr_m as i16));
                let m_nom = month_num_to_name(curr_m);
                if !m_nom.is_empty() {
                    let m_upper = m_nom.to_uppercase();
                    meses_filtro.insert(m_upper.clone());
                    periodos_filtro.insert(format!("{}/{}", m_upper, curr_y));
                    periodos_filtro.insert(format!("{:02}/{}", curr_m, curr_y));
                    periodos_filtro.insert(format!("{}/{}", curr_m, curr_y));
                }
                curr_m += 1;
                if curr_m > 12 {
                    curr_m = 1;
                    curr_y += 1;
                }
            }
        }
    }
    (anios_filtro, meses_filtro, periodos_filtro, periodos_int_filtro)
}

pub fn resolve_business_context_for_revision(
    tx: &mut Transaction,
    biz: &Business,
    target_revision_id: Option<Uuid>,
) -> Result<BusinessContext, Box<dyn std::error::Error + Send + Sync>> {
    // Si se especificó una revisión activa (creada previamente por el usuario):
    // Preservar estrictamente el cliente, registro patronal y revisión creados por el usuario.
    if let Some(rev_id) = target_revision_id {
        let rev_row_opt = tx.query_opt(
            r#"
            SELECT r.id, r.id_registro_patronal, rp.id_cliente, rp.registro, rp.delegacion, rp.subdelegacion, r.nombre
            FROM public.revisiones r
            JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
            WHERE r.id = $1
            "#,
            &[&rev_id],
        )?;

        if let Some(row) = rev_row_opt {
            let id_revision: Uuid = row.get(0);
            let id_registro_patronal: Uuid = row.get(1);
            let id_cliente: Uuid = row.get(2);
            let reg_11: String = row.get(3);
            let del_db: Option<String> = row.get(4);
            let subdel_db: Option<String> = row.get(5);
            let del_upper = del_db.or_else(|| biz.delegacion.as_deref().map(|s| s.trim().to_uppercase()));
            let subdel_upper = subdel_db.or_else(|| biz.subdelegacion.as_deref().map(|s| s.trim().to_uppercase()));

            // Actualizar datos patronales y del cliente con los campos extraídos de las cédulas
            update_cliente_and_rp_from_biz(tx, id_cliente, id_registro_patronal, biz)?;

            let (anios_filtro, meses_filtro, periodos_filtro, periodos_int_filtro) = get_obra_filter_dates(tx, id_revision);

            return Ok(BusinessContext {
                id_cliente,
                id_registro_patronal,
                id_revision,
                reg_11,
                delegacion: del_upper,
                subdelegacion: subdel_upper,
                anios_filtro,
                meses_filtro,
                periodos_filtro,
                periodos_int_filtro,
            });
        }

        // Si rev_id no corresponde a una revisión, verificar si es directamente un registro patronal
        let rp_row_opt = tx.query_opt(
            r#"
            SELECT rp.id, rp.id_cliente, rp.registro, rp.delegacion, rp.subdelegacion
            FROM public.registros_patronales rp
            WHERE rp.id = $1
            "#,
            &[&rev_id],
        )?;

        if let Some(row) = rp_row_opt {
            let id_registro_patronal: Uuid = row.get(0);
            let id_cliente: Uuid = row.get(1);
            let reg_11: String = row.get(2);
            let del_db: Option<String> = row.get(3);
            let subdel_db: Option<String> = row.get(4);
            let del_upper = del_db.or_else(|| biz.delegacion.as_deref().map(|s| s.trim().to_uppercase()));
            let subdel_upper = subdel_db.or_else(|| biz.subdelegacion.as_deref().map(|s| s.trim().to_uppercase()));

            update_cliente_and_rp_from_biz(tx, id_cliente, id_registro_patronal, biz)?;

            let rev_row = tx.query_opt(
                "SELECT id FROM public.revisiones WHERE id_registro_patronal = $1 ORDER BY id DESC LIMIT 1",
                &[&id_registro_patronal],
            )?;

            let id_revision: Uuid = match rev_row {
                Some(r) => r.get(0),
                None => {
                    let sample_mes = biz
                        .employees
                        .first()
                        .and_then(|e| {
                            e.mes_periodo
                                .as_deref()
                                .or_else(|| e.data_employee.first().and_then(|de| de.mes_periodo.as_deref()))
                        })
                        .unwrap_or("ENERO");
                    let sample_anio = biz
                        .employees
                        .first()
                        .and_then(|e| {
                            e.anio_periodo
                                .as_deref()
                                .or_else(|| e.data_employee.first().and_then(|de| de.anio_periodo.as_deref()))
                        })
                        .unwrap_or("2024");
                    let rev_nombre = format!("Revisión SUA {} {}", sample_mes, sample_anio);
                    let rev_folio = format!("REV-SUA-{}-{}", sample_anio, month_to_num(sample_mes));
                    let rev_nombre_upper = rev_nombre.trim().to_uppercase();
                    let rev_folio_upper = rev_folio.trim().to_uppercase();
                    let rev_nombre_trun = truncate_str(Some(&rev_nombre_upper), 150).unwrap();
                    let rev_folio_trun = truncate_str(Some(&rev_folio_upper), 20).unwrap();

                    let new_id = Uuid::now_v7();
                    let ins = tx.query_one(
                        r#"
                        INSERT INTO public.revisiones (id, id_registro_patronal, nombre, folio, fecha_entrega_revision)
                        VALUES ($1, $2, $3, $4, CURRENT_DATE)
                        ON CONFLICT (id_registro_patronal, nombre) DO UPDATE SET nombre = EXCLUDED.nombre
                        RETURNING id
                        "#,
                        &[&new_id, &id_registro_patronal, &rev_nombre_trun, &rev_folio_trun],
                    )?;
                    ins.get(0)
                }
            };

            let (anios_filtro, meses_filtro, periodos_filtro, periodos_int_filtro) = get_obra_filter_dates(tx, id_revision);

            return Ok(BusinessContext {
                id_cliente,
                id_registro_patronal,
                id_revision,
                reg_11,
                delegacion: del_upper,
                subdelegacion: subdel_upper,
                anios_filtro,
                meses_filtro,
                periodos_filtro,
                periodos_int_filtro,
            });
        }
    }

    // 2. Extraer registro patronal limpio del SUA (11 caracteres alfanuméricos)
    let raw_reg = biz
        .registro_patronal
        .as_deref()
        .map(|s| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    let reg_11: String = raw_reg.chars().take(11).collect();

    // Si el Registro Patronal ya existe en la base de datos (ej. 'A3415636101' perteneciente al cliente activo),
    // recuperar exactamente su UUID real y el id_cliente al que pertenece para asegurar la integridad referencial.
    if !reg_11.is_empty() {
        let rp_by_reg_opt = tx.query_opt(
            r#"
            SELECT rp.id, rp.id_cliente, rp.registro, rp.delegacion, rp.subdelegacion
            FROM public.registros_patronales rp
            WHERE UPPER(REGEXP_REPLACE(rp.registro, '[^A-Za-z0-9]', '', 'g')) = $1
               OR UPPER(TRIM(rp.registro)) = $1
            ORDER BY rp.created_at ASC NULLS LAST, rp.id ASC
            LIMIT 1
            "#,
            &[&reg_11],
        )?;

        if let Some(row) = rp_by_reg_opt {
            let id_registro_patronal: Uuid = row.get(0);
            let id_cliente: Uuid = row.get(1);
            let reg_db: String = row.get(2);
            let del_db: Option<String> = row.get(3);
            let subdel_db: Option<String> = row.get(4);
            let del_upper = del_db.or_else(|| biz.delegacion.as_deref().map(|s| s.trim().to_uppercase()));
            let subdel_upper = subdel_db.or_else(|| biz.subdelegacion.as_deref().map(|s| s.trim().to_uppercase()));

            update_cliente_and_rp_from_biz(tx, id_cliente, id_registro_patronal, biz)?;

            let rev_row = tx.query_opt(
                "SELECT id FROM public.revisiones WHERE id_registro_patronal = $1 ORDER BY created_at DESC NULLS LAST, id DESC LIMIT 1",
                &[&id_registro_patronal],
            )?;

            let id_revision: Uuid = match rev_row {
                Some(r) => r.get(0),
                None => {
                    let sample_mes = biz
                        .employees
                        .first()
                        .and_then(|e| {
                            e.mes_periodo
                                .as_deref()
                                .or_else(|| e.data_employee.first().and_then(|de| de.mes_periodo.as_deref()))
                        })
                        .unwrap_or("ENERO");
                    let sample_anio = biz
                        .employees
                        .first()
                        .and_then(|e| {
                            e.anio_periodo
                                .as_deref()
                                .or_else(|| e.data_employee.first().and_then(|de| de.anio_periodo.as_deref()))
                        })
                        .unwrap_or("2024");
                    let rev_nombre = format!("Revisión SUA {} {}", sample_mes, sample_anio);
                    let rev_folio = format!("REV-SUA-{}-{}", sample_anio, month_to_num(sample_mes));
                    let rev_nombre_upper = rev_nombre.trim().to_uppercase();
                    let rev_folio_upper = rev_folio.trim().to_uppercase();
                    let rev_nombre_trun = truncate_str(Some(&rev_nombre_upper), 150).unwrap();
                    let rev_folio_trun = truncate_str(Some(&rev_folio_upper), 20).unwrap();

                    let new_id = Uuid::now_v7();
                    let ins = tx.query_one(
                        r#"
                        INSERT INTO public.revisiones (id, id_registro_patronal, nombre, folio, fecha_entrega_revision)
                        VALUES ($1, $2, $3, $4, CURRENT_DATE)
                        ON CONFLICT (id_registro_patronal, nombre) DO UPDATE SET nombre = EXCLUDED.nombre
                        RETURNING id
                        "#,
                        &[&new_id, &id_registro_patronal, &rev_nombre_trun, &rev_folio_trun],
                    )?;
                    ins.get(0)
                }
            };

            let (anios_filtro, meses_filtro, periodos_filtro, periodos_int_filtro) = get_obra_filter_dates(tx, id_revision);

            return Ok(BusinessContext {
                id_cliente,
                id_registro_patronal,
                id_revision,
                reg_11: reg_db,
                delegacion: del_upper,
                subdelegacion: subdel_upper,
                anios_filtro,
                meses_filtro,
                periodos_filtro,
                periodos_int_filtro,
            });
        }
    }

    // 3. Fallback: Cliente (Patrón)
    let rfc_patron = biz.rfc.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    }).filter(|s| !s.is_empty());
    let nombre_patron = biz
        .razon_social
        .as_deref()
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            biz.registro_patronal
                .as_deref()
                .map(|s| format!("PATRÓN {}", s.trim().to_uppercase()))
                .unwrap_or_else(|| "PATRÓN SUA SIN NOMBRE".to_string())
        });

    let mut cliente_id: Option<Uuid> = None;
    if let Some(ref rfc) = rfc_patron {
        let row_opt = tx.query_opt(
            "SELECT id FROM public.clientes WHERE rfc = $1 LIMIT 1",
            &[rfc],
        )?;
        if let Some(row) = row_opt {
            cliente_id = Some(row.get(0));
        }
    }

    if cliente_id.is_none() {
        let row_opt = tx.query_opt(
            "SELECT id FROM public.clientes WHERE nombre = $1 LIMIT 1",
            &[&nombre_patron],
        )?;
        if let Some(row) = row_opt {
            cliente_id = Some(row.get(0));
        }
    }

    let id_cliente: Uuid = match cliente_id {
        Some(id) => id,
        None => {
            let new_id = Uuid::now_v7();
            let rfc_val = truncate_str(rfc_patron.as_deref(), 13);
            let row = tx.query_one(
                r#"
                INSERT INTO public.clientes (id, nombre, razon_social, rfc)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (nombre) DO UPDATE SET razon_social = EXCLUDED.razon_social
                RETURNING id
                "#,
                &[&new_id, &nombre_patron, &nombre_patron, &rfc_val],
            )?;
            row.get(0)
        }
    };

    // 2. Registro Patronal
    let raw_reg = biz
        .registro_patronal
        .as_deref()
        .map(|s| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "00000000000".to_string());
    let reg_11: String = raw_reg.chars().take(11).collect();

    let prima_float = biz.prima.as_deref().and_then(|s| s.trim().parse::<f64>().ok());

    let reg_id_opt = tx.query_opt(
        "SELECT id FROM public.registros_patronales WHERE id_cliente = $1 AND registro = $2 LIMIT 1",
        &[&id_cliente, &reg_11],
    )?;

    let act_upper = biz.actividad.as_deref().map(|s| crate::cedulas::business::clean_actividad(s));
    let dom_upper = biz.domicilio.as_deref().map(|s| crate::cedulas::business::clean_domicilio(s));
    let cp_upper = biz.codigo_postal.as_deref().map(|s| s.trim().to_uppercase());
    let ent_upper = biz.entidad.as_deref().map(|s| crate::cedulas::business::clean_entidad(s));
    let area_upper = biz.area_geografica.as_deref().map(|s| s.trim().to_uppercase());
    let del_upper = biz.delegacion.as_deref().map(|s| s.trim().to_uppercase());
    let subdel_upper = biz.subdelegacion.as_deref().map(|s| s.trim().to_uppercase());
    let cod_del_upper = biz.codigo_delegacion.as_deref().map(|s| s.trim().to_uppercase());
    let cod_subdel_upper = biz.codigo_subdelegacion.as_deref().map(|s| s.trim().to_uppercase());

    let id_registro_patronal: Uuid = match reg_id_opt {
        Some(row) => {
            let id: Uuid = row.get(0);
            update_cliente_and_rp_from_biz(tx, id_cliente, id, biz)?;
            id
        }
        None => {
            let new_id = Uuid::now_v7();
            let row = tx.query_one(
                r#"
                INSERT INTO public.registros_patronales (
                    id, id_cliente, registro, actividad, domicilio, codigo_postal, entidad,
                    area_geografica, delegacion, subdelegacion, codigo_delegacion, codigo_subdelegacion, prima_riesgo
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                ON CONFLICT (id_cliente, registro) DO UPDATE SET registro = EXCLUDED.registro
                RETURNING id
                "#,
                &[
                    &new_id,
                    &id_cliente,
                    &reg_11,
                    &truncate_str(act_upper.as_deref(), 300),
                    &truncate_str(dom_upper.as_deref(), 500),
                    &truncate_str(cp_upper.as_deref(), 5),
                    &truncate_str(ent_upper.as_deref(), 100),
                    &truncate_str(area_upper.as_deref(), 5),
                    &truncate_str(del_upper.as_deref(), 100),
                    &truncate_str(subdel_upper.as_deref(), 100),
                    &truncate_str(cod_del_upper.as_deref(), 10),
                    &truncate_str(cod_subdel_upper.as_deref(), 10),
                    &prima_float,
                ],
            )?;
            let id: Uuid = row.get(0);
            update_cliente_and_rp_from_biz(tx, id_cliente, id, biz)?;
            id
        }
    };

    // 3. Revisión
    let sample_mes = biz
        .employees
        .first()
        .and_then(|e| {
            e.mes_periodo
                .as_deref()
                .or_else(|| e.data_employee.first().and_then(|de| de.mes_periodo.as_deref()))
        })
        .unwrap_or("ENERO");
    let sample_anio = biz
        .employees
        .first()
        .and_then(|e| {
            e.anio_periodo
                .as_deref()
                .or_else(|| e.data_employee.first().and_then(|de| de.anio_periodo.as_deref()))
        })
        .unwrap_or("2024");
    let rev_nombre = format!("Revisión SUA {} {}", sample_mes, sample_anio);
    let rev_folio = format!("REV-SUA-{}-{}", sample_anio, month_to_num(sample_mes));
    let rev_nombre_upper = rev_nombre.trim().to_uppercase();
    let rev_folio_upper = rev_folio.trim().to_uppercase();
    let rev_nombre_trun = truncate_str(Some(&rev_nombre_upper), 150).unwrap();
    let rev_folio_trun = truncate_str(Some(&rev_folio_upper), 20).unwrap();

    let rev_id_opt = tx.query_opt(
        "SELECT id FROM public.revisiones WHERE id_registro_patronal = $1 AND (nombre = $2 OR TRIM(folio) = TRIM($3)) LIMIT 1",
        &[&id_registro_patronal, &rev_nombre_trun, &rev_folio_trun],
    )?;

    let id_revision: Uuid = match rev_id_opt {
        Some(row) => row.get(0),
        None => {
            let new_id = Uuid::now_v7();
            let today = Utc::now().date_naive();
            let row = tx.query_one(
                r#"
                INSERT INTO public.revisiones (id, id_registro_patronal, nombre, folio, fecha_entrega_revision)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (id_registro_patronal, nombre) DO UPDATE SET nombre = EXCLUDED.nombre
                RETURNING id
                "#,
                &[&new_id, &id_registro_patronal, &rev_nombre_trun, &rev_folio_trun, &today],
            )?;
            row.get(0)
        }
    };

    let (anios_filtro, meses_filtro, periodos_filtro, periodos_int_filtro) = get_obra_filter_dates(tx, id_revision);

    Ok(BusinessContext {
        id_cliente,
        id_registro_patronal,
        id_revision,
        reg_11,
        delegacion: del_upper,
        subdelegacion: subdel_upper,
        anios_filtro,
        meses_filtro,
        periodos_filtro,
        periodos_int_filtro,
    })
}

/// Compara si los renglones de detalle en la base de datos coinciden 100% de forma idéntica
/// con los renglones que se intentan guardar. Si algo cambia (días, SDI, importes, claves, etc.),
/// por muy mínimo que sea, retorna false.
fn are_details_exact_match(db_rows: &[postgres::Row], incoming: &[crate::cedulas::employees::DataEmployee]) -> bool {
    if db_rows.is_empty() && incoming.is_empty() {
        return true;
    }
    if incoming.is_empty() && db_rows.len() == 1 {
        let subtotal: Option<f64> = db_rows[0].get(20);
        if subtotal.unwrap_or(0.0).abs() < 0.005 {
            return true;
        }
    }
    if db_rows.len() != incoming.len() {
        return false;
    }

    let normalize_mov = |s: Option<&str>| -> Option<String> {
        let raw = s?.trim();
        if raw.is_empty() { return None; }
        let upper = raw.to_uppercase();
        if upper == "R" || upper == "REING" || upper.starts_with("REING") {
            Some("REING".to_string())
        } else if upper == "A" || upper == "ALTA" {
            Some("ALTA".to_string())
        } else if upper == "B" || upper == "BAJA" {
            Some("BAJA".to_string())
        } else if upper == "MS" || upper.starts_with("MODIF") {
            Some("MS".to_string())
        } else {
            Some(upper)
        }
    };

    let opt_str_eq = |a: Option<String>, b: Option<&str>| -> bool {
        let ca = normalize_mov(a.as_deref());
        let cb = normalize_mov(b);
        match (ca, cb) {
            (None, None) => true,
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(&y),
            _ => false,
        }
    };

    let opt_float_eq = |a: Option<f64>, b: Option<f64>| -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(x), Some(y)) => (x - y).abs() < 0.005,
            (Some(x), None) => x.abs() < 0.005,
            (None, Some(y)) => y.abs() < 0.005,
        }
    };

    let opt_i16_eq = |a: Option<i16>, b: Option<f64>| -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(x), Some(y)) => (x as f64 - y).abs() < 0.005,
            (Some(x), None) => x == 0,
            (None, Some(y)) => y.abs() < 0.005,
        }
    };

    let mut used = vec![false; db_rows.len()];

    for inc in incoming {
        let mut matched = false;
        for (i, row) in db_rows.iter().enumerate() {
            if used[i] {
                continue;
            }

            let db_clave: Option<String> = row.get(0);
            let db_fecha: Option<NaiveDate> = row.get(1);
            let db_dias: Option<i16> = row.get(2);
            let db_sdi: Option<f64> = row.get(3);
            let db_lic: Option<f64> = row.get(4);
            let db_inc: Option<f64> = row.get(5);
            let db_aus: Option<f64> = row.get(6);
            let db_cf: Option<f64> = row.get(7);
            let db_exc_pat: Option<f64> = row.get(8);
            let db_exc_obr: Option<f64> = row.get(9);
            let db_pd_pat: Option<f64> = row.get(10);
            let db_pd_obr: Option<f64> = row.get(11);
            let db_gmp_pat: Option<f64> = row.get(12);
            let db_gmp_obr: Option<f64> = row.get(13);
            let db_rt: Option<f64> = row.get(14);
            let db_iv_pat: Option<f64> = row.get(15);
            let db_iv_obr: Option<f64> = row.get(16);
            let db_gps: Option<f64> = row.get(17);
            let db_pat: Option<f64> = row.get(18);
            let db_obr: Option<f64> = row.get(19);
            let db_sub: Option<f64> = row.get(20);

            if opt_str_eq(db_clave, inc.clave.as_deref())
                && db_fecha == inc.fecha
                && opt_i16_eq(db_dias, inc.dias)
                && opt_float_eq(db_sdi, inc.sdi)
                && opt_float_eq(db_lic, inc.lic)
                && opt_float_eq(db_inc, inc.inc)
                && opt_float_eq(db_aus, inc.aus)
                && opt_float_eq(db_cf, inc.cf)
                && opt_float_eq(db_exc_pat, inc.exc_pat)
                && opt_float_eq(db_exc_obr, inc.exc_obr)
                && opt_float_eq(db_pd_pat, inc.pd_pat)
                && opt_float_eq(db_pd_obr, inc.pd_obr)
                && opt_float_eq(db_gmp_pat, inc.gmp_pat)
                && opt_float_eq(db_gmp_obr, inc.gmp_obr)
                && opt_float_eq(db_rt, inc.rt)
                && opt_float_eq(db_iv_pat, inc.iv_pat)
                && opt_float_eq(db_iv_obr, inc.iv_obr)
                && opt_float_eq(db_gps, inc.gps)
                && opt_float_eq(db_pat, inc.patronal)
                && opt_float_eq(db_obr, inc.obrera)
                && opt_float_eq(db_sub, inc.subtotal)
            {
                used[i] = true;
                matched = true;
                break;
            }
        }

        if !matched {
            return false;
        }
    }

    true
}

/// Describe el motivo por el que un trabajador de cédula se omite por no traer renglones de cuotas.
/// Devuelve `None` si el trabajador sí tiene renglones de cuotas.
pub fn describe_trabajador_sin_detalles(emp: &CedulaEmployee, archivo: &str) -> Option<String> {
    if !emp.data_employee.is_empty() {
        return None;
    }
    let nombre = emp.nombre.as_deref().unwrap_or("SIN NOMBRE");
    let nss = emp.no_seguro_social.as_deref().unwrap_or("N/D");
    Some(format!(
        "Trabajador '{}' (NSS {}) sin renglones de cuotas en el archivo '{}': omitido",
        nombre, nss, archivo
    ))
}

/// Busca al empleado existente de un cliente para emparejarlo con la cédula.
/// Parámetros: $1 id_cliente, $2 NSS, $3 CURP, $4 RFC real (sin genéricos SAT), $5 nombre.
/// El emparejamiento por nombre solo se acepta si ningún identificador del empleado
/// existente contradice al de la cédula (evita fusionar homónimos distintos).
const SQL_BUSCAR_EMPLEADO_CEDULA: &str = r#"
    SELECT id FROM public.empleados
    WHERE id_cliente = $1 AND (
        ($2::text IS NOT NULL AND BTRIM(nss) = $2)
        OR ($3::text IS NOT NULL AND BTRIM(curp) = $3)
        OR ($4::text IS NOT NULL AND BTRIM(rfc) = $4)
        OR ($5::text IS NOT NULL AND nombre = $5
            -- Nombre solo si ningún identificador del existente contradice al de la cédula
            AND ($2::text IS NULL OR nss IS NULL OR BTRIM(nss) = '' OR BTRIM(nss) = $2)
            AND ($3::text IS NULL OR curp IS NULL OR BTRIM(curp) = '' OR BTRIM(curp) = $3)
            AND ($4::text IS NULL OR rfc IS NULL OR BTRIM(rfc) = '' OR BTRIM(rfc) IN ('XAXX010101000', 'XEXX010101000') OR BTRIM(rfc) = $4))
    )
    ORDER BY
        CASE
            WHEN $2::text IS NOT NULL AND BTRIM(nss) = $2 THEN 1
            WHEN $3::text IS NOT NULL AND BTRIM(curp) = $3 THEN 2
            WHEN $4::text IS NOT NULL AND BTRIM(rfc) = $4 THEN 3
            ELSE 4
        END
    LIMIT 1
    "#;

/// Inserta liquidación de forma idempotente frente a la restricción única
/// (registro patronal, empleado, año, mes). Ante carrera con otra carga devuelve la fila existente.
/// Parámetros: $1 id, $2 id_registro_patronal, $3 id_empleado, $4 mes_periodo, $5 mes_periodo_int,
/// $6 anio_periodo, $7 nombre_archivo, $8 id_archivo, $9 origen.
/// Salida: id de la liquidación y `insertada` (true solo si se creó una fila nueva).
const SQL_INSERT_LIQUIDACION_UPSERT: &str = r#"
    INSERT INTO public.liquidaciones (
        id, id_registro_patronal, id_empleado, mes_periodo, mes_periodo_int, anio_periodo,
        nombre_archivo, id_archivo, origen
    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
    ON CONFLICT ON CONSTRAINT uq_liquidaciones_rp_emp_periodo DO UPDATE SET
        nombre_archivo = EXCLUDED.nombre_archivo,
        id_archivo = COALESCE(EXCLUDED.id_archivo, public.liquidaciones.id_archivo),
        origen = EXCLUDED.origen
    RETURNING id, (xmax = 0) AS insertada
    "#;

/// Inserta empleado con NSS de forma idempotente frente al índice único parcial (id_cliente, nss).
/// Parámetros: $1 id, $2 id_cliente, $3 nombre, $4 rfc, $5 curp, $6 nss, $7 clave_ubicacion, $8 pensionado.
/// Salida: id del empleado y `insertado` (true solo si se creó una fila nueva).
const SQL_INSERT_EMPLEADO_UPSERT: &str = r#"
    INSERT INTO public.empleados (id, id_cliente, nombre, rfc, curp, nss, clave_ubicacion, pensionado)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
    ON CONFLICT (id_cliente, nss) WHERE nss IS NOT NULL AND BTRIM(nss) <> '' DO UPDATE SET
        nombre = COALESCE(NULLIF(BTRIM(public.empleados.nombre), ''), EXCLUDED.nombre)
    RETURNING id, (xmax = 0) AS insertado
    "#;

/// Marca para revisión los reportes ya editados por el analista cuya liquidación cambió de detalles.
const SQL_MARCAR_CEDULA_ACTUALIZADA: &str = "UPDATE public.reportes_liquidaciones SET cedula_actualizada = true WHERE id_liquidacion = $1 AND editado_usuario = true AND cedula_actualizada = false";

/// Guarda un trabajador de cédula con sus liquidaciones, detalles y vinculaciones a obras.
/// Regla de conflicto: ante una liquidación existente del mismo RP/empleado/periodo gana la carga
/// más reciente (sin distinguir SUA de EMA). Las vinculaciones de obra solo se reemplazan cuando la
/// carga nueva trae obras. `id_archivo` registra el archivo de origen de la liquidación.
pub fn save_single_cedula_employee_tx(
    tx: &mut Transaction,
    emp: &CedulaEmployee,
    ctx: &BusinessContext,
    id_archivo: Option<Uuid>,
) -> Result<(usize, usize, usize, usize, usize), Box<dyn std::error::Error + Send + Sync>> {
    // Guarda: sin renglones de cuotas no se crea empleado, liquidación ni vinculación
    if emp.data_employee.is_empty() {
        log::warn!(
            "Trabajador sin renglones de cuotas omitido: NSS {} ({}), archivo '{}'",
            emp.no_seguro_social.as_deref().unwrap_or("N/D"),
            emp.nombre.as_deref().unwrap_or("SIN NOMBRE"),
            emp.file_name.as_deref().unwrap_or("N/D")
        );
        return Ok((0, 0, 0, 0, 0));
    }

    let mut empleados_count = 0;
    let mut liquidaciones_count = 0;
    let mut detalles_count = 0;
    let mut registros_obras_count = 0;
    let mut vinculaciones_count = 0;

    let (cleaned_nombre, extracted_pension) = match emp.nombre.as_deref() {
        Some(nom) => {
            let (cn, ep) = crate::cedulas::employees::clean_name_and_extract_pension(nom);
            (Some(cn.to_uppercase()), ep)
        }
        None => (None, None),
    };
    let emp_nombre = cleaned_nombre;
    let emp_pensionado = emp
        .pensionado
        .clone()
        .or(extracted_pension)
        .map(|p| p.trim().to_uppercase())
        .filter(|p| !p.is_empty());
    let mut emp_rfc = emp.rfc.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    }).filter(|s| !s.is_empty() && (s.len() == 13 || s.len() == 12));
    let mut emp_curp = emp.curp.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    }).filter(|s| !s.is_empty() && s.len() == 18);
    let emp_nss = emp.no_seguro_social.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    });

    let puede_calcular = emp_nombre
        .as_deref()
        .map(|n| {
            let clean = n.trim();
            !clean.is_empty() && !clean.eq_ignore_ascii_case("NONE") && clean.chars().any(|c| c.is_alphabetic())
        })
        .unwrap_or(false);

    // Si la CURP no vino en el archivo, intentar generarla sólo si se cuenta con RFC válido con fecha
    if emp_curp.is_none() && puede_calcular {
        let curp_generada = crate::cedulas::curp_renapo::calcular_curp_oficial(
            emp_nombre.as_deref().unwrap(),
            emp_rfc.as_deref(),
            emp_nss.as_deref(),
            None,
            None,
        );
        if !curp_generada.is_empty() && crate::cedulas::curp_renapo::validar_curp_formato(&curp_generada) {
            emp_curp = Some(curp_generada);
        } else {
            emp_curp = None;
        }
    }

    // Si el RFC no vino en el archivo, intentar generarlo con los datos que se tienen; en caso de no poder, usar el genérico oficial del SUA
    if emp_rfc.is_none() {
        if puede_calcular && emp_curp.is_some() {
            let rfc_generado = crate::cedulas::rfc_sat::calcular_rfc_sat(
                emp_nombre.as_deref().unwrap(),
                emp_curp.as_deref(),
                emp_nss.as_deref(),
            );
            if rfc_generado.len() == 13 && rfc_generado != "0000000000000" && rfc_generado != "XAXX010101000" {
                emp_rfc = Some(rfc_generado);
            } else {
                emp_rfc = Some("XAXX010101000".to_string());
            }
        } else {
            emp_rfc = Some("XAXX010101000".to_string());
        }
    }
    let emp_clave_ubicacion = truncate_str(emp.clave_ubicacion.as_deref(), 50).filter(|ub| {
        let clean = ub.trim();
        !clean.is_empty()
            && clean != "—"
            && clean != "-"
            && !crate::cedulas::employees::is_curp_candidate(clean)
            && !crate::cedulas::employees::is_rfc_candidate(clean)
    });

    let nss_param = emp_nss.as_deref().map(|s| truncate_str(Some(s), 11).unwrap_or_default());
    let curp_param = emp_curp.as_deref().filter(|c| !c.is_empty() && !crate::validation::is_generic_sat_identifier(c)).map(|s| truncate_str(Some(s), 18).unwrap_or_default());
    let rfc_param = emp_rfc.as_deref().filter(|r| !r.is_empty() && !crate::validation::is_generic_sat_identifier(r)).map(|s| truncate_str(Some(s), 13).unwrap_or_default());
    let nom_param = emp_nombre.as_deref().map(|s| truncate_str(Some(s), 100).unwrap_or_default());

    let mut empleado_id: Option<Uuid> = None;
    if nss_param.is_some() || curp_param.is_some() || rfc_param.is_some() || nom_param.is_some() {
        let row = tx.query_opt(
            SQL_BUSCAR_EMPLEADO_CEDULA,
            &[&ctx.id_cliente, &nss_param, &curp_param, &rfc_param, &nom_param],
        )?;
        if let Some(r) = row {
            empleado_id = Some(r.get(0));
        }
    }

    let id_empleado: Uuid = match empleado_id {
        Some(id) => {
            tx.execute(
                r#"
                UPDATE public.empleados
                SET nombre = CASE
                        WHEN public.empleados.nombre ~* '^\s*P\s*[/ -]' THEN COALESCE($2, public.empleados.nombre)
                        WHEN $2 IS NOT NULL AND TRIM(public.empleados.nombre) = '' THEN $2
                        ELSE COALESCE(NULLIF(TRIM(public.empleados.nombre), ''), $2)
                    END,
                    rfc = CASE
                        WHEN public.empleados.rfc IS NOT NULL
                             AND TRIM(public.empleados.rfc) <> ''
                             AND TRIM(public.empleados.rfc) <> 'XAXX010101000'
                             AND LENGTH(TRIM(public.empleados.rfc)) = 13
                        THEN public.empleados.rfc
                        ELSE $3
                    END,
                    curp = CASE
                        WHEN public.empleados.curp IS NOT NULL
                             AND TRIM(public.empleados.curp) <> ''
                             AND TRIM(public.empleados.curp) <> 'XAXX010101000'
                             AND LENGTH(TRIM(public.empleados.curp)) = 18
                        THEN public.empleados.curp
                        ELSE $4
                    END,
                    nss = COALESCE(NULLIF(TRIM(public.empleados.nss), ''), $5),
                    clave_ubicacion = COALESCE(public.empleados.clave_ubicacion, $6),
                    pensionado = COALESCE($7, public.empleados.pensionado)
                WHERE id = $1
                "#,
                &[
                    &id,
                    &truncate_str(emp_nombre.as_deref(), 100),
                    &truncate_str(emp_rfc.as_deref(), 13),
                    &truncate_str(emp_curp.as_deref(), 18),
                    &truncate_str(emp_nss.as_deref(), 11),
                    &emp_clave_ubicacion,
                    &truncate_str(emp_pensionado.as_deref(), 10),
                ],
            )?;
            id
        }
        None => {
            let new_id = Uuid::now_v7();
            let p_nombre = truncate_str(emp_nombre.as_deref(), 100);
            let p_rfc = truncate_str(emp_rfc.as_deref(), 13);
            let p_curp = truncate_str(emp_curp.as_deref(), 18);
            let p_nss = truncate_str(emp_nss.as_deref(), 11);
            let p_pensionado = truncate_str(emp_pensionado.as_deref(), 10);

            let nss_upsert = p_nss.clone().filter(|s| !s.trim().is_empty());
            if let Some(nss_val) = nss_upsert {
                // Con NSS: upsert contra el índice único (id_cliente, nss) para evitar duplicados en carreras
                let row = tx.query_one(
                    SQL_INSERT_EMPLEADO_UPSERT,
                    &[
                        &new_id,
                        &ctx.id_cliente,
                        &p_nombre,
                        &p_rfc,
                        &p_curp,
                        &nss_val,
                        &emp_clave_ubicacion,
                        &p_pensionado,
                    ],
                )?;
                let insertado: bool = row.get(1);
                if insertado {
                    empleados_count += 1;
                }
                row.get(0)
            } else {
                // Sin NSS no existe restricción de unicidad aplicable: INSERT simple
                let row = tx.query_one(
                    r#"
                    INSERT INTO public.empleados (id, id_cliente, nombre, rfc, curp, nss, clave_ubicacion, pensionado)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    RETURNING id
                    "#,
                    &[
                        &new_id,
                        &ctx.id_cliente,
                        &p_nombre,
                        &p_rfc,
                        &p_curp,
                        &p_nss,
                        &emp_clave_ubicacion,
                        &p_pensionado,
                    ],
                )?;
                empleados_count += 1;
                row.get(0)
            }
        }
    };

    // Liquidación
    // Periodo obligatorio: sin mes/año legibles no se asigna a ningún periodo por defecto
    let (mes_val, anio_val) = match resolve_periodo_cedula(emp) {
        Some(p) => p,
        None => {
            let nss = emp.no_seguro_social.as_deref().unwrap_or("N/D");
            let archivo = emp.file_name.as_deref().unwrap_or("N/D");
            return Err(format!(
                "Periodo (mes/año) no detectado para el trabajador NSS {} en el archivo '{}'",
                nss, archivo
            )
            .into());
        }
    };
    let mes_int: i16 = mes_val;
    let anio_norm: i16 = anio_val;
    let mes_norm: String = month_num_to_name(mes_val as u32).to_string();

    let default_nombre_arch = format!("Cédula SUA {} - {}.pdf", ctx.reg_11, mes_norm);
    let nombre_arch = emp.file_name.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&default_nombre_arch);

    // Pre-calcular Registros de Obra de la cédula antes de deduplicar liquidación (SUA vs EMA)
    let mut obras_to_process: Vec<(String, i16)> = Vec::new();
    for ro in &emp.registro_obras {
        let num_obra = truncate_str(Some(&ro.registro_obra), 15).unwrap_or_default();
        let clean_num = num_obra.trim();
        if !clean_num.is_empty() {
            match crate::validation::validate_registro_obra(clean_num) {
                Ok(info) => match dias_obra_validos(ro.dias_registro_obra) {
                    Some(dias) => obras_to_process.push((info.clean, dias)),
                    None => log::warn!(
                        "Días de obra fuera de rango ({}) para obra {} del trabajador NSS {}: vinculación descartada",
                        ro.dias_registro_obra,
                        clean_num,
                        emp.no_seguro_social.as_deref().unwrap_or("N/D")
                    ),
                },
                Err(err) => {
                    let existing_db_ro = tx.query_opt(
                        "SELECT numero FROM public.registros_obras WHERE (id_revision = $1 OR id_registro_patronal = $2) AND TRIM(numero) = TRIM($3) LIMIT 1",
                        &[&ctx.id_revision, &ctx.id_registro_patronal, &clean_num],
                    ).inspect_err(|e| {
                        let err_str = crate::db::schema::format_pg_error(e);
                        crate::app_logger::log_db_error(
                            "SELECT public.registros_obras fallback check",
                            &["public.registros_obras"],
                            "SELECT numero FROM public.registros_obras WHERE ...",
                            &err_str,
                        );
                    })?;
                    if let Some(row) = existing_db_ro {
                        let existing_num: String = row.get(0);
                        match dias_obra_validos(ro.dias_registro_obra) {
                            Some(dias) => obras_to_process.push((existing_num, dias)),
                            None => log::warn!(
                                "Días de obra fuera de rango ({}) para obra {} del trabajador NSS {}: vinculación descartada",
                                ro.dias_registro_obra,
                                clean_num,
                                emp.no_seguro_social.as_deref().unwrap_or("N/D")
                            ),
                        }
                    } else {
                        log::debug!("Ignorando registro de obra no válido desde cédula '{}': {}", clean_num, err);
                    }
                }
            }
        }
    }

    // Deduplicar obras por número consolidando días
    let mut dedup_map: std::collections::BTreeMap<String, i16> = std::collections::BTreeMap::new();
    for (num, dias) in obras_to_process {
        let entry = dedup_map.entry(num).or_insert(0);
        *entry = sumar_dias_obra(*entry, dias);
    }
    let obras_to_process: Vec<(String, i16)> = dedup_map.into_iter().collect();

    let is_authoritative_sua = !obras_to_process.is_empty() || emp.rfc.is_some() || emp.curp.is_some();
    // Origen de la carga que se registra en la liquidación
    let origen: &str = if is_authoritative_sua { "SUA" } else { "EMA" };

    // Evitar duplicados: consultar todas las liquidaciones existentes para este empleado, registro patronal y período
    let existing_liqs = tx.query(
        r#"
        SELECT l.id,
               EXISTS(
                   SELECT 1 FROM public.vinculaciones_obras_liquidaciones vol
                   WHERE vol.id_liquidacion = l.id
               ) AS has_vinc
        FROM public.liquidaciones l
        WHERE l.id_registro_patronal = $1
          AND l.id_empleado = $2
          AND (UPPER(TRIM(l.mes_periodo)) = $3 OR l.mes_periodo_int = $5)
          AND l.anio_periodo = $4
        ORDER BY has_vinc DESC, l.id ASC
        "#,
        &[&ctx.id_registro_patronal, &id_empleado, &mes_norm, &anio_norm, &mes_int],
    ).inspect_err(|e| {
        let err_str = crate::db::schema::format_pg_error(e);
        crate::app_logger::log_db_error(
            "SELECT existing_liquidaciones",
            &["public.liquidaciones"],
            "SELECT l.id, EXISTS(...) FROM public.liquidaciones l WHERE ...",
            &err_str,
        );
    })?;

    // Verificar si alguna liquidación existente coincide 100% de manera idéntica en todos sus renglones de detalle
    let mut matched_liq_id: Option<Uuid> = None;
    let mut replace_liq_id: Option<Uuid> = None;
    for row in &existing_liqs {
        let ex_id: Uuid = row.get(0);
        let det_rows = tx.query(
            r#"
            SELECT clave, fecha, dias, sdi, lic, inc, aus, cf,
                   exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr,
                   rt, iv_pat, iv_obr, gps, patronal, obrera, subtotal
            FROM public.detalles_liquidaciones
            WHERE id_liquidacion = $1
            "#,
            &[&ex_id],
        ).inspect_err(|e| {
            let err_str = crate::db::schema::format_pg_error(e);
            crate::app_logger::log_db_error(
                "SELECT detalles_liquidaciones_match",
                &["public.detalles_liquidaciones"],
                "SELECT ... FROM public.detalles_liquidaciones WHERE id_liquidacion = $1",
                &err_str,
            );
        })?;

        if are_details_exact_match(&det_rows, &emp.data_employee) {
            matched_liq_id = Some(ex_id);
            break;
        }
    }

    // Regla "gana la carga más reciente": si no hubo coincidencia exacta, los detalles existentes
    // se reemplazan por los de esta carga, sin distinguir SUA de EMA.
    if matched_liq_id.is_none() && !existing_liqs.is_empty() {
        replace_liq_id = Some(existing_liqs[0].get(0));
    }

    // Si ya existe la liquidación con detalles idénticos, reutilizar su ID y registrar la carga actual;
    // si los detalles difieren, reemplazarlos en el mismo ID;
    // de lo contrario, insertar nueva liquidación (idempotente frente a carreras).
    let id_liquidacion = if let Some(ex_id) = matched_liq_id {
        tx.execute(
            "UPDATE public.liquidaciones SET nombre_archivo = COALESCE($2, nombre_archivo), id_archivo = COALESCE($3, id_archivo), origen = $4 WHERE id = $1",
            &[&ex_id, &truncate_str(Some(nombre_arch), 350), &id_archivo, &origen],
        )?;
        ex_id
    } else {
        let target_liq_id = if let Some(rep_id) = replace_liq_id {
            tx.execute(
                r#"
                UPDATE public.liquidaciones
                SET mes_periodo = $2, mes_periodo_int = $3,
                    nombre_archivo = COALESCE($4, nombre_archivo),
                    id_archivo = COALESCE($5, id_archivo),
                    origen = $6
                WHERE id = $1
                "#,
                &[
                    &rep_id,
                    &mes_norm,
                    &mes_int,
                    &truncate_str(Some(nombre_arch), 350),
                    &id_archivo,
                    &origen,
                ],
            )?;
            tx.execute(
                "DELETE FROM public.detalles_liquidaciones WHERE id_liquidacion = $1",
                &[&rep_id],
            )?;
            // Los reportes ya editados por el analista deben revisar el cambio de detalles
            tx.execute(SQL_MARCAR_CEDULA_ACTUALIZADA, &[&rep_id])?;
            // Las vinculaciones de obra solo se reemplazan si la carga nueva trae obras; si no, se conservan
            if !obras_to_process.is_empty() {
                tx.execute(
                    "DELETE FROM public.vinculaciones_obras_liquidaciones WHERE id_liquidacion = $1",
                    &[&rep_id],
                )?;
            }
            rep_id
        } else {
            let new_id = Uuid::now_v7();
            let up_row = tx.query_one(
                SQL_INSERT_LIQUIDACION_UPSERT,
                &[
                    &new_id,
                    &ctx.id_registro_patronal,
                    &id_empleado,
                    &mes_norm,
                    &mes_int,
                    &anio_norm,
                    &truncate_str(Some(nombre_arch), 350),
                    &id_archivo,
                    &origen,
                ],
            ).inspect_err(|e| {
                let err_str = crate::db::schema::format_pg_error(e);
                crate::app_logger::log_db_error(
                    "UPSERT public.liquidaciones",
                    &["public.liquidaciones"],
                    "INSERT INTO public.liquidaciones (...) VALUES (...) ON CONFLICT ON CONSTRAINT uq_liquidaciones_rp_emp_periodo DO UPDATE ...",
                    &err_str,
                );
            })?;
            let target: Uuid = up_row.get(0);
            let insertada: bool = up_row.get(1);
            if insertada {
                liquidaciones_count += 1;
            } else {
                // Otra carga concurrente ya creó la liquidación: limpiar sus detalles para no duplicar renglones
                tx.execute(
                    "DELETE FROM public.detalles_liquidaciones WHERE id_liquidacion = $1",
                    &[&target],
                )?;
                // Los reportes ya editados por el analista deben revisar el cambio de detalles
                tx.execute(SQL_MARCAR_CEDULA_ACTUALIZADA, &[&target])?;
            }
            target
        };

        // Detalles Liquidación (100% información cruda sin filtrar en tablas maestras)
        for de in &emp.data_employee {
            let id_det = Uuid::now_v7();
            let dias_det = de.dias.map(|d| d as i16);

            tx.execute(
                r#"
                INSERT INTO public.detalles_liquidaciones (
                    id, id_liquidacion, clave, fecha, dias, sdi, lic, inc, aus, cf,
                    exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr,
                    gps, patronal, obrera, subtotal
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23
                )
                "#,
                &[
                    &id_det,
                    &target_liq_id,
                    &truncate_str(de.clave.as_deref(), 17),
                    &de.fecha,
                    &dias_det,
                    &de.sdi,
                    &de.lic,
                    &de.inc,
                    &de.aus,
                    &de.cf,
                    &de.exc_pat,
                    &de.exc_obr,
                    &de.pd_pat,
                    &de.pd_obr,
                    &de.gmp_pat,
                    &de.gmp_obr,
                    &de.rt,
                    &de.iv_pat,
                    &de.iv_obr,
                    &de.gps,
                    &de.patronal,
                    &de.obrera,
                    &de.subtotal,
                ],
            ).inspect_err(|e| {
                let err_str = crate::db::schema::format_pg_error(e);
                crate::app_logger::log_db_error(
                    "INSERT public.detalles_liquidaciones",
                    &["public.detalles_liquidaciones"],
                    "INSERT INTO public.detalles_liquidaciones (id, id_liquidacion, ...) VALUES (...)",
                    &err_str,
                );
            })?;

            detalles_count += 1;
        }

        target_liq_id
    };

    if !obras_to_process.is_empty() {
        let valid_ro_numbers: Vec<String> = obras_to_process.iter().map(|(num, _)| num.clone()).collect();
        let _ = tx.execute(
            r#"
            DELETE FROM public.vinculaciones_obras_liquidaciones vol
            USING public.registros_obras ro
            WHERE vol.id_registro_obra = ro.id
              AND vol.id_liquidacion = $1
              AND TRIM(UPPER(ro.numero)) != ALL($2)
            "#,
            &[&id_liquidacion, &valid_ro_numbers],
        );
    }

    for (num_obra, dias_vinc) in obras_to_process {
        let m_num = month_to_num(&mes_norm);
        let y_val = anio_norm as i32;
        let valid_m = if (1..=12).contains(&m_num) { m_num } else { 1 };
        let fecha_inicio = chrono::NaiveDate::from_ymd_opt(y_val, valid_m, 1)
            .unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap());
        let last_day = match valid_m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => if (y_val % 4 == 0 && y_val % 100 != 0) || (y_val % 400 == 0) { 29 } else { 28 },
            _ => 30,
        };
        let fecha_fin = chrono::NaiveDate::from_ymd_opt(y_val, valid_m, last_day)
            .unwrap_or(fecha_inicio);

        let obra_row = tx.query_one(
            r#"
            INSERT INTO public.registros_obras (
                id, id_revision, id_registro_patronal, numero, tipo_obra, fecha_inicio, fecha_fin, creado_usuario
            ) VALUES (
                uuidv7(), $1, $2, $3, 'REGISTRO DE OBRA CÉDULA', $4, $5, false
            )
            ON CONFLICT (id_revision, numero) DO UPDATE SET
                fecha_inicio = CASE
                    WHEN COALESCE(public.registros_obras.creado_usuario, false) THEN public.registros_obras.fecha_inicio
                    ELSE LEAST(public.registros_obras.fecha_inicio, EXCLUDED.fecha_inicio)
                END,
                fecha_fin = CASE
                    WHEN COALESCE(public.registros_obras.creado_usuario, false) THEN public.registros_obras.fecha_fin
                    ELSE GREATEST(public.registros_obras.fecha_fin, EXCLUDED.fecha_fin)
                END
            RETURNING id, (xmax = 0) AS is_inserted
            "#,
            &[&ctx.id_revision, &ctx.id_registro_patronal, &num_obra, &fecha_inicio, &fecha_fin],
        ).inspect_err(|e| {
            let err_str = crate::db::schema::format_pg_error(e);
            crate::app_logger::log_db_error(
                "UPSERT public.registros_obras (cédula)",
                &["public.registros_obras"],
                "INSERT INTO public.registros_obras ... ON CONFLICT (id_revision, numero) DO UPDATE ...",
                &err_str,
            );
        })?;

        let id_registro_obra: Uuid = obra_row.get(0);
        let is_inserted: bool = obra_row.get(1);
        if is_inserted {
            registros_obras_count += 1;
        }

        // Evitar duplicar vinculación maestra obra - liquidación; actualizar días exactos si ya existía
        let existing_vinc = tx.query_opt(
            r#"
            SELECT vol.id 
            FROM public.vinculaciones_obras_liquidaciones vol
            JOIN public.registros_obras ro ON ro.id = vol.id_registro_obra
            WHERE vol.id_liquidacion = $1 AND TRIM(UPPER(ro.numero)) = TRIM(UPPER($2))
            LIMIT 1
            "#,
            &[&id_liquidacion, &num_obra],
        ).inspect_err(|e| {
            let err_str = crate::db::schema::format_pg_error(e);
            crate::app_logger::log_db_error(
                "SELECT public.vinculaciones_obras_liquidaciones",
                &["public.vinculaciones_obras_liquidaciones"],
                "SELECT id FROM public.vinculaciones_obras_liquidaciones WHERE vol.id_liquidacion = $1 AND ro.numero = $2",
                &err_str,
            );
        })?;

        if let Some(v_row) = existing_vinc {
            let id_vinc_existing: Uuid = v_row.get(0);
            tx.execute(
                "UPDATE public.vinculaciones_obras_liquidaciones SET id_registro_obra = $2, dias_registro_obra = $3 WHERE id = $1",
                &[&id_vinc_existing, &id_registro_obra, &dias_vinc],
            )?;
        } else {
            let id_vinc = Uuid::now_v7();
            tx.execute(
                r#"
                INSERT INTO public.vinculaciones_obras_liquidaciones (
                    id, id_registro_obra, id_liquidacion, dias_registro_obra
                ) VALUES ($1, $2, $3, $4)
                "#,
                &[&id_vinc, &id_registro_obra, &id_liquidacion, &dias_vinc],
            ).inspect_err(|e| {
                let err_str = crate::db::schema::format_pg_error(e);
                crate::app_logger::log_db_error(
                    "INSERT public.vinculaciones_obras_liquidaciones",
                    &["public.vinculaciones_obras_liquidaciones"],
                    "INSERT INTO public.vinculaciones_obras_liquidaciones (id, id_registro_obra, id_liquidacion, dias_registro_obra) VALUES ($1, $2, $3, $4)",
                    &err_str,
                );
            })?;
            vinculaciones_count += 1;
        }
    }

    Ok((empleados_count, liquidaciones_count, detalles_count, registros_obras_count, vinculaciones_count))
}

pub fn save_cedulas_to_db(
    client: &mut Client,
    businesses: &[Business],
) -> Result<CedulasDbStats, String> {
    let mut stats = CedulasDbStats::default();

    for biz in businesses {
        let mut tx = match client.transaction() {
            Ok(t) => t,
            Err(e) => {
                stats.errores.push(format!("Error iniciando transacción para cédula: {}", e));
                continue;
            }
        };

        let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = (|| {
            let ctx = resolve_business_context(&mut tx, biz)?;
            for emp in &biz.employees {
                if let Some(msg) = describe_trabajador_sin_detalles(emp, emp.file_name.as_deref().unwrap_or("N/D")) {
                    stats.errores.push(msg);
                    continue;
                }
                let (e_c, l_c, d_c, ro_c, v_c) = save_single_cedula_employee_tx(&mut tx, emp, &ctx, None)?;
                stats.empleados_guardados += e_c;
                stats.liquidaciones_guardadas += l_c;
                stats.detalles_guardados += d_c;
                stats.registros_obras_guardados += ro_c;
                stats.vinculaciones_guardadas += v_c;
            }
            Ok(())
        })();

        match result {
            Ok(_) => {
                if let Err(e) = tx.commit() {
                    stats.errores.push(format!("Error haciendo commit para cédula: {}", crate::db::schema::format_pg_error(&e)));
                }
            }
            Err(e) => {
                let _ = tx.rollback();
                stats.errores.push(format!("Error guardando cédula: {}", crate::db::schema::format_boxed_pg_error(e.as_ref())));
            }
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_cliente_and_rp_from_biz() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(_) => return, // Skip if test database is not available
        };

        let mut tx = match client.transaction() {
            Ok(t) => t,
            Err(_) => return,
        };

        let c_id = Uuid::now_v7();
        let c_name = format!("CLI_TEST_{}", Uuid::now_v7());
        tx.execute(
            "INSERT INTO public.clientes (id, nombre, razon_social, rfc, curp) VALUES ($1, $2, 'OLD RAZON', 'XAXX01010100', 'OLD123456HDFRRN01')",
            &[&c_id, &c_name],
        ).unwrap();

        let rp_id = Uuid::now_v7();
        let unique_reg = format!("Z{:010}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_micros());
        let reg_11: String = unique_reg.chars().take(11).collect();
        tx.execute(
            "INSERT INTO public.registros_patronales (id, id_cliente, registro, actividad, domicilio, codigo_postal, prima_riesgo) VALUES ($1, $2, $3, 'OLD ACT', 'OLD DOM', '00000', 1.0)",
            &[&rp_id, &c_id, &reg_11],
        ).unwrap();

        // 1. Business sin CURP y con RFC de 12 caracteres (3 letras y 9 números) -> MORAL
        let mut biz = Business::new();
        biz.razon_social = Some("NUEVA RAZON SOCIAL SA DE CV".to_string());
        biz.rfc = Some("XAXX01010100".to_string()); // 12 chars -> MORAL
        biz.curp = None;
        biz.actividad = Some("NUEVA ACTIVIDAD".to_string());
        biz.domicilio = Some("CALLE NUEVA 123".to_string());
        biz.codigo_postal = Some("54321".to_string());
        biz.entidad = Some("ESTADO DE MEXICO".to_string());
        biz.area_geografica = Some("B".to_string());
        biz.delegacion = Some("DEL NUEVA".to_string());
        biz.subdelegacion = Some("SUBDEL NUEVA".to_string());
        biz.prima = Some("6.5".to_string());

        update_cliente_and_rp_from_biz(&mut tx, c_id, rp_id, &biz).unwrap();

        // Verificar datos en RP
        let row_rp = tx.query_one(
            "SELECT actividad, domicilio, TRIM(codigo_postal), entidad, TRIM(area_geografica), delegacion, subdelegacion, prima_riesgo FROM public.registros_patronales WHERE id = $1",
            &[&rp_id],
        ).unwrap();
        assert_eq!(row_rp.get::<_, Option<String>>(0).as_deref(), Some("NUEVA ACTIVIDAD"));
        assert_eq!(row_rp.get::<_, Option<String>>(1).as_deref(), Some("CALLE NUEVA 123"));
        assert_eq!(row_rp.get::<_, Option<String>>(2).as_deref(), Some("54321"));
        assert_eq!(row_rp.get::<_, Option<String>>(3).as_deref(), Some("ESTADO DE MEXICO"));
        assert_eq!(row_rp.get::<_, Option<String>>(4).as_deref(), Some("B"));
        assert_eq!(row_rp.get::<_, Option<String>>(5).as_deref(), Some("DEL NUEVA"));
        assert_eq!(row_rp.get::<_, Option<String>>(6).as_deref(), Some("SUBDEL NUEVA"));
        assert_eq!(row_rp.get::<_, Option<f64>>(7), Some(6.5));

        // Verificar datos en Cliente (curp preservada, razon social actualizada, tipo persona MORAL)
        let row_c = tx.query_one(
            r#"
            SELECT c.razon_social, TRIM(c.curp), TRIM(tp.tipo)
            FROM public.clientes c
            LEFT JOIN public.clasificaciones_clientes cc ON cc.id = c.id_clasificacion_cliente
            LEFT JOIN public.tipos_personas tp ON tp.id = cc.id_tipo_persona
            WHERE c.id = $1
            "#,
            &[&c_id],
        ).unwrap();
        assert_eq!(row_c.get::<_, Option<String>>(0).as_deref(), Some("NUEVA RAZON SOCIAL SA DE CV"));
        assert_eq!(row_c.get::<_, Option<String>>(1).as_deref(), Some("OLD123456HDFRRN01"));
        assert_eq!(row_c.get::<_, Option<String>>(2).as_deref(), Some("MORAL"));

        // 2. Ahora cédula con CURP nueva y RFC de 13 caracteres (4 letras y 9 números) -> FISICA
        let mut biz2 = Business::new();
        biz2.rfc = Some("XAXX010101000".to_string()); // 13 chars -> FISICA
        biz2.curp = Some("NEWC123456HDFRRN02".to_string());
        update_cliente_and_rp_from_biz(&mut tx, c_id, rp_id, &biz2).unwrap();

        let row_c2 = tx.query_one(
            r#"
            SELECT TRIM(c.curp), TRIM(tp.tipo)
            FROM public.clientes c
            LEFT JOIN public.clasificaciones_clientes cc ON cc.id = c.id_clasificacion_cliente
            LEFT JOIN public.tipos_personas tp ON tp.id = cc.id_tipo_persona
            WHERE c.id = $1
            "#,
            &[&c_id],
        ).unwrap();
        assert_eq!(row_c2.get::<_, Option<String>>(0).as_deref(), Some("NEWC123456HDFRRN02"));
        assert_eq!(row_c2.get::<_, Option<String>>(1).as_deref(), Some("FISICA"));

        let _ = tx.rollback();
    }
}


#[cfg(test)]
mod tests_periodo {
    use super::*;
    use crate::cedulas::employees::DataEmployee;

    #[test]
    fn mes_int_y_anio_directos() {
        let emp = CedulaEmployee {
            mes_periodo_int: 5,
            anio_periodo: Some("2026".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_periodo_cedula(&emp), Some((5, 2026)));
    }

    #[test]
    fn mes_texto_y_anio_con_espacios() {
        let emp = CedulaEmployee {
            mes_periodo_int: 0,
            mes_periodo: Some("marzo".to_string()),
            anio_periodo: Some(" 2025 ".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_periodo_cedula(&emp), Some((3, 2025)));
    }

    #[test]
    fn mes_no_reconocido_es_none() {
        let emp = CedulaEmployee {
            mes_periodo_int: 0,
            mes_periodo: Some("XYZ".to_string()),
            anio_periodo: Some("2026".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_periodo_cedula(&emp), None);
    }

    #[test]
    fn anio_ausente_es_none() {
        let emp = CedulaEmployee {
            mes_periodo_int: 4,
            data_employee: vec![DataEmployee {
                mes_periodo_int: 4,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(resolve_periodo_cedula(&emp), None);
    }

    #[test]
    fn anio_fuera_de_rango_es_none() {
        let emp = CedulaEmployee {
            mes_periodo_int: 2,
            anio_periodo: Some("1985".to_string()),
            ..Default::default()
        };
        assert_eq!(resolve_periodo_cedula(&emp), None);
    }

    #[test]
    fn periodo_desde_primer_detalle() {
        let emp = CedulaEmployee {
            data_employee: vec![DataEmployee {
                mes_periodo: Some("JUNIO".to_string()),
                anio_periodo: Some("2024".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(resolve_periodo_cedula(&emp), Some((6, 2024)));
    }
}

#[cfg(test)]
mod tests_sin_detalles {
    use super::*;
    use crate::cedulas::employees::{DataEmployee, Employee};

    #[test]
    fn describe_trabajador_sin_renglones_incluye_nombre_nss_y_omitido() {
        let emp = Employee {
            nombre: Some("JUAN PEREZ".to_string()),
            no_seguro_social: Some("12345678901".to_string()),
            ..Default::default()
        };
        let msg = describe_trabajador_sin_detalles(&emp, "cedula.pdf").expect("debe describirse");
        assert!(msg.contains("JUAN PEREZ"));
        assert!(msg.contains("12345678901"));
        assert!(msg.contains("omitido"));
    }

    #[test]
    fn describe_trabajador_con_renglones_devuelve_none() {
        let emp = Employee {
            data_employee: vec![DataEmployee::default()],
            ..Default::default()
        };
        assert!(describe_trabajador_sin_detalles(&emp, "cedula.pdf").is_none());
    }

    #[test]
    fn describe_trabajador_sin_datos_usa_placeholders() {
        let emp = Employee::default();
        let msg = describe_trabajador_sin_detalles(&emp, "cedula.pdf").expect("debe describirse");
        assert!(msg.contains("SIN NOMBRE"));
        assert!(msg.contains("N/D"));
    }
}

#[cfg(test)]
mod tests_dias_obra {
    use super::{dias_obra_validos, sumar_dias_obra};

    #[test]
    fn dias_fuera_de_rango_se_descartan() {
        assert_eq!(dias_obra_validos(0), None);
        assert_eq!(dias_obra_validos(-3), None);
        assert_eq!(dias_obra_validos(32), None);
    }

    #[test]
    fn dias_validos_se_conservan() {
        assert_eq!(dias_obra_validos(1), Some(1));
        assert_eq!(dias_obra_validos(31), Some(31));
        assert_eq!(dias_obra_validos(15), Some(15));
    }

    #[test]
    fn suma_limitada_a_31() {
        assert_eq!(sumar_dias_obra(20, 15), 31);
        assert_eq!(sumar_dias_obra(10, 5), 15);
        assert_eq!(sumar_dias_obra(i16::MAX, 5), 31);
    }
}

#[cfg(test)]
mod tests_emparejamiento {
    use super::*;

    /// Colapsa espacios en blanco para que las comparaciones de texto SQL no dependan del formato.
    fn normalizar(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn sql_no_acepta_nombre_suelto_con_identificador_distinto() {
        let sql = normalizar(SQL_BUSCAR_EMPLEADO_CEDULA);
        // Guardas de no-contradicción dentro de la rama de nombre
        assert!(sql.contains("OR BTRIM(nss) = $2)"), "falta guarda de NSS");
        assert!(sql.contains("OR BTRIM(curp) = $3)"), "falta guarda de CURP");
        assert!(sql.contains("OR BTRIM(rfc) = $4)"), "falta guarda de RFC");
        // La rama de nombre suelta ya no debe existir
        assert!(
            !sql.contains("OR ($5::text IS NOT NULL AND nombre = $5)"),
            "la rama de nombre suelta sigue presente"
        );
    }

    #[test]
    fn homonimo_con_nss_distinto_no_se_fusiona() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("PostgreSQL no disponible en test (skipping): {}", e);
                return;
            }
        };
        let mut tx = match client.transaction() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("No se pudo abrir transacción (skipping): {}", e);
                return;
            }
        };

        // Cliente de prueba (RFC omitido para no chocar con el UNIQUE de genéricos ya existentes)
        let id_cliente = Uuid::now_v7();
        let nombre_cliente = format!("CLIENTE PRUEBA HOMONIMOS {}", Uuid::now_v7());
        tx.execute(
            "INSERT INTO public.clientes (id, nombre) VALUES ($1, $2)",
            &[&id_cliente, &nombre_cliente],
        )
        .expect("insert de cliente de prueba");

        let nombre_empleado = "JUAN PEREZ LOPEZ";
        tx.execute(
            "INSERT INTO public.empleados (id, id_cliente, nombre, nss) VALUES ($1, $2, $3, $4)",
            &[&Uuid::now_v7(), &id_cliente, &nombre_empleado, &"12345678901"],
        )
        .expect("insert de empleado de prueba");

        let nombre_param: Option<String> = Some(nombre_empleado.to_string());

        // NSS distinto: el homónimo no debe emparejarse
        let nss_distinto: Option<String> = Some("10987654321".to_string());
        let filas_distinto = tx
            .query(
                SQL_BUSCAR_EMPLEADO_CEDULA,
                &[&id_cliente, &nss_distinto, &None::<String>, &None::<String>, &nombre_param],
            )
            .expect("consulta con NSS distinto");
        assert_eq!(filas_distinto.len(), 0, "un homónimo con NSS distinto se fusionó");

        // Sin NSS en la cédula: el nombre sigue sirviendo
        let filas_sin_nss = tx
            .query(
                SQL_BUSCAR_EMPLEADO_CEDULA,
                &[&id_cliente, &None::<String>, &None::<String>, &None::<String>, &nombre_param],
            )
            .expect("consulta sin NSS");
        assert_eq!(filas_sin_nss.len(), 1, "el nombre sin contradicción debe emparejar");

        // Siempre revertir: no se deja ningún dato de prueba
        let _ = tx.rollback();
    }
}

#[cfg(test)]
mod tests_upsert_sql {
    use super::*;

    /// Colapsa espacios en blanco para que las comparaciones de texto SQL no dependan del formato.
    fn normalizar(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn insert_liquidacion_es_idempotente_y_registra_origen() {
        let sql = normalizar(SQL_INSERT_LIQUIDACION_UPSERT);
        assert!(
            sql.contains("ON CONFLICT ON CONSTRAINT uq_liquidaciones_rp_emp_periodo"),
            "falta ON CONFLICT sobre la restricción única de liquidaciones"
        );
        assert!(sql.contains("RETURNING id, (xmax = 0)"), "falta RETURNING id, (xmax = 0)");
        assert!(sql.contains("id_archivo"), "falta registro de id_archivo");
        assert!(sql.contains("origen"), "falta registro de origen");
    }

    #[test]
    fn insert_empleado_es_idempotente_por_nss() {
        let sql = normalizar(SQL_INSERT_EMPLEADO_UPSERT);
        assert!(
            sql.contains("ON CONFLICT (id_cliente, nss) WHERE nss IS NOT NULL"),
            "falta ON CONFLICT sobre el índice único parcial de NSS"
        );
        assert!(sql.contains("RETURNING id, (xmax = 0)"), "falta RETURNING id, (xmax = 0)");
    }
}

#[cfg(test)]
mod tests_cedula_actualizada {
    use super::*;

    /// Colapsa espacios en blanco para que las comparaciones de texto SQL no dependan del formato.
    fn normalizar(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn marca_solo_reportes_editados_de_la_liquidacion() {
        let sql = normalizar(SQL_MARCAR_CEDULA_ACTUALIZADA);
        assert!(sql.contains("editado_usuario = true"), "falta filtro de reportes editados");
        assert!(sql.contains("cedula_actualizada = true"), "falta marca cedula_actualizada");
        assert!(sql.contains("id_liquidacion = $1"), "falta filtro por id_liquidacion");
        assert!(!sql.contains("DELETE"), "la marca no debe borrar datos");
    }
}
