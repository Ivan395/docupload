use chrono::{Datelike, NaiveDate, Utc};
use postgres::{Client, Transaction};
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use uuid::Uuid;

use crate::cedulas::business::{month_num_to_name, month_to_num, Business};
use crate::cedulas::employees::Employee as CedulaEmployee;
use crate::db::client::DbPool;

#[derive(Debug, Default, Clone)]
pub struct CedulasDbStats {
    pub liquidaciones_guardadas: usize,
    pub detalles_guardados: usize,
    pub vinculaciones_guardadas: usize,
    pub empleados_guardados: usize,
    pub registros_obras_guardados: usize,
    pub errores: Vec<String>,
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

fn normalize_mes_periodo(mes_opt: Option<&str>) -> String {
    let raw = mes_opt.unwrap_or("ENERO").trim();
    let num = month_to_num(raw);
    let valid_num = if (1..=12).contains(&num) { num } else { 1 };
    month_num_to_name(valid_num).to_string()
}

fn normalize_anio_periodo(anio_opt: Option<&str>) -> i16 {
    let curr_year = Utc::now().year() as i16;
    if let Some(s) = anio_opt {
        if let Ok(y) = s.trim().parse::<i16>() {
            if (2000..=curr_year).contains(&y) {
                return y;
            }
        }
    }
    curr_year
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

    let _ = tx.execute(
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
    );

    // 2. Actualizar Cliente
    let rz_upper = biz.razon_social.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());

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

    let _ = tx.execute(
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
    );

    Ok(())
}

fn get_obra_filter_dates(
    tx: &mut Transaction,
    id_revision: Uuid,
) -> (BTreeSet<i16>, BTreeSet<String>) {
    let obra_dates = tx.query(
        r#"
        SELECT ro.fecha_inicio, ro.fecha_fin
        FROM public.registros_obras ro
        WHERE ro.id_revision = $1
           OR ro.id_registro_patronal = (SELECT r.id_registro_patronal FROM public.revisiones r WHERE r.id = $1)
        "#,
        &[&id_revision],
    ).unwrap_or_default();

    let mut anios_filtro: BTreeSet<i16> = BTreeSet::new();
    let mut meses_filtro: BTreeSet<String> = BTreeSet::new();

    for od in obra_dates {
        let f_ini: Option<chrono::NaiveDate> = od.get(0);
        let f_fin: Option<chrono::NaiveDate> = od.get(1);
        if let (Some(fi), Some(ff)) = (f_ini, f_fin) {
            let mut curr_y = fi.year();
            let mut curr_m = fi.month();
            let end_y = ff.year();
            let end_m = ff.month();

            while (curr_y < end_y) || (curr_y == end_y && curr_m <= end_m) {
                anios_filtro.insert(curr_y as i16);
                let m_nom = month_num_to_name(curr_m);
                if !m_nom.is_empty() {
                    meses_filtro.insert(m_nom.to_string());
                }
                curr_m += 1;
                if curr_m > 12 {
                    curr_m = 1;
                    curr_y += 1;
                }
            }
        }
    }
    (anios_filtro, meses_filtro)
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

            let (anios_filtro, meses_filtro) = get_obra_filter_dates(tx, id_revision);

            return Ok(BusinessContext {
                id_cliente,
                id_registro_patronal,
                id_revision,
                reg_11,
                delegacion: del_upper,
                subdelegacion: subdel_upper,
                anios_filtro,
                meses_filtro,
            });
        }
    }
    // 1. Cliente (Patrón)
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

    let (anios_filtro, meses_filtro) = get_obra_filter_dates(tx, id_revision);

    Ok(BusinessContext {
        id_cliente,
        id_registro_patronal,
        id_revision,
        reg_11,
        delegacion: del_upper,
        subdelegacion: subdel_upper,
        anios_filtro,
        meses_filtro,
    })
}

/// Compara si los renglones de detalle en la base de datos coinciden 100% de forma idéntica
/// con los renglones que se intentan guardar. Si algo cambia (días, SDI, importes, claves, etc.),
/// por muy mínimo que sea, retorna false.
fn are_details_exact_match(db_rows: &[postgres::Row], incoming: &[crate::cedulas::employees::DataEmployee]) -> bool {
    if db_rows.len() != incoming.len() {
        return false;
    }

    let opt_str_eq = |a: Option<String>, b: Option<&str>| -> bool {
        let ca = a.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());
        let cb = b.map(|s| s.trim()).filter(|s| !s.is_empty());
        match (ca, cb) {
            (None, None) => true,
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
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

pub fn save_single_cedula_employee_tx(
    tx: &mut Transaction,
    emp: &CedulaEmployee,
    ctx: &BusinessContext,
) -> Result<(usize, usize, usize, usize, usize), Box<dyn std::error::Error + Send + Sync>> {
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
            r#"
            SELECT id FROM public.empleados
            WHERE id_cliente = $1 AND (
                ($2::text IS NOT NULL AND nss = $2)
                OR ($3::text IS NOT NULL AND curp = $3)
                OR ($4::text IS NOT NULL AND rfc = $4)
                OR ($5::text IS NOT NULL AND nombre = $5)
            )
            ORDER BY
                CASE
                    WHEN $2::text IS NOT NULL AND nss = $2 THEN 1
                    WHEN $3::text IS NOT NULL AND curp = $3 THEN 2
                    WHEN $4::text IS NOT NULL AND rfc = $4 THEN 3
                    ELSE 4
                END
            LIMIT 1
            "#,
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
            let row = tx.query_one(
                r#"
                INSERT INTO public.empleados (id, id_cliente, nombre, rfc, curp, nss, clave_ubicacion, pensionado)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING id
                "#,
                &[
                    &new_id,
                    &ctx.id_cliente,
                    &truncate_str(emp_nombre.as_deref(), 100),
                    &truncate_str(emp_rfc.as_deref(), 13),
                    &truncate_str(emp_curp.as_deref(), 18),
                    &truncate_str(emp_nss.as_deref(), 11),
                    &emp_clave_ubicacion,
                    &truncate_str(emp_pensionado.as_deref(), 10),
                ],
            )?;
            empleados_count += 1;
            row.get(0)
        }
    };

    // Liquidación
    let emp_mes = emp.mes_periodo.as_deref().or_else(|| {
        emp.data_employee.first().and_then(|de| de.mes_periodo.as_deref())
    });
    let emp_anio = emp.anio_periodo.as_deref().or_else(|| {
        emp.data_employee.first().and_then(|de| de.anio_periodo.as_deref())
    });
    let mes_norm = normalize_mes_periodo(emp_mes);
    let anio_norm = normalize_anio_periodo(emp_anio);

    let nombre_arch = format!("Cédula SUA {} - {}.pdf", ctx.reg_11, mes_norm);

    // Evitar duplicados: consultar todas las liquidaciones existentes para este empleado, revisión y período
    let existing_liqs = tx.query(
        r#"
        SELECT id FROM public.liquidaciones
        WHERE id_revision = $1 AND id_empleado = $2 AND mes_periodo = $3 AND anio_periodo = $4
        "#,
        &[&ctx.id_revision, &id_empleado, &mes_norm, &anio_norm],
    )?;

    // Verificar si alguna liquidación existente coincide 100% de manera idéntica en todos sus renglones de detalle
    let mut found_exact_duplicate = false;
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
        )?;

        if are_details_exact_match(&det_rows, &emp.data_employee) {
            found_exact_duplicate = true;
            break;
        }
    }

    // Si todo es exactamente lo mismo, no guardar duplicados
    if found_exact_duplicate {
        return Ok((empleados_count, 0, 0, 0, 0));
    }

    // Si no existe o algo cambió (por muy mínimo que sea), guardarlo
    let id_liquidacion = Uuid::now_v7();
    tx.execute(
        r#"
        INSERT INTO public.liquidaciones (
            id, id_revision, id_empleado, mes_periodo, anio_periodo, nombre_archivo
        ) VALUES (
            $1, $2, $3, $4, $5, $6
        )
        "#,
        &[
            &id_liquidacion,
            &ctx.id_revision,
            &id_empleado,
            &mes_norm,
            &anio_norm,
            &truncate_str(Some(&nombre_arch), 350),
        ],
    )?;
    liquidaciones_count += 1;

    // Determinar si la liquidación cae dentro de los años y meses de las obras de la revisión (pre-calculado en contexto)
    let should_mirror = if ctx.anios_filtro.is_empty() || ctx.meses_filtro.is_empty() {
        true
    } else {
        let mes_check = mes_norm.trim().to_uppercase();
        let anio_check = anio_norm;
        ctx.anios_filtro.contains(&anio_check) && ctx.meses_filtro.contains(&mes_check)
    };

    let id_rep_liq_opt = if should_mirror {
        let new_id = Uuid::now_v7();
        tx.execute(
            r#"
            INSERT INTO public.reportes_liquidaciones (
                id, id_liquidacion, id_revision, id_empleado, mes_periodo, anio_periodo, nombre_archivo
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7
            )
            "#,
            &[
                &new_id,
                &id_liquidacion,
                &ctx.id_revision,
                &id_empleado,
                &mes_norm,
                &anio_norm,
                &truncate_str(Some(&nombre_arch), 350),
            ],
        )?;
        Some(new_id)
    } else {
        None
    };

    // Detalles Liquidación
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
                &id_liquidacion,
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
        )?;

        detalles_count += 1;
    }

    // Registros de Obra y Vinculaciones
    let mut obras_to_process: Vec<(String, i16)> = Vec::new();
    for ro in &emp.registro_obras {
        let num_obra = truncate_str(Some(&ro.registro_obra), 15).unwrap_or_default();
        let clean_num = num_obra.trim();
        if !clean_num.is_empty() {
            match crate::validation::validate_registro_obra(clean_num) {
                Ok(info) => {
                    obras_to_process.push((info.clean, (ro.dias_registro_obra as i16).clamp(1, 31)));
                }
                Err(err) => {
                    log::warn!("Ignorando registro de obra no válido desde cédula '{}': {}", clean_num, err);
                }
            }
        }
    }
    if obras_to_process.is_empty() {
        if let Some(ref ub) = emp.clave_ubicacion {
            let num_obra = truncate_str(Some(ub), 15).unwrap_or_default();
            let clean_num = num_obra.trim();
            if !clean_num.is_empty() && clean_num != "—" && clean_num != "-" {
                match crate::validation::validate_registro_obra(clean_num) {
                    Ok(info) => {
                        obras_to_process.push((info.clean, 30));
                    }
                    Err(err) => {
                        log::warn!("Ignorando clave_ubicacion como registro de obra porque no cumple verificación '{}': {}", clean_num, err);
                    }
                }
            }
        }
    }

    let mut final_rep_liq_id = id_rep_liq_opt;
    let mut processed_obras: Vec<(Uuid, i16)> = Vec::new();

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
                fecha_inicio = LEAST(public.registros_obras.fecha_inicio, EXCLUDED.fecha_inicio),
                fecha_fin = GREATEST(public.registros_obras.fecha_fin, EXCLUDED.fecha_fin)
            RETURNING id, (xmax = 0) AS is_inserted
            "#,
            &[&ctx.id_revision, &ctx.id_registro_patronal, &num_obra, &fecha_inicio, &fecha_fin],
        )?;

        let id_registro_obra: Uuid = obra_row.get(0);
        let is_inserted: bool = obra_row.get(1);
        if is_inserted {
            registros_obras_count += 1;
        }

        processed_obras.push((id_registro_obra, dias_vinc));

        // Evitar duplicar vinculación obra - liquidación
        let existing_vinc = tx.query_opt(
            "SELECT id FROM public.vinculaciones_obras_liquidaciones WHERE id_registro_obra = $1 AND id_liquidacion = $2 LIMIT 1",
            &[&id_registro_obra, &id_liquidacion],
        )?;

        if existing_vinc.is_none() {
            let id_vinc = Uuid::now_v7();
            tx.execute(
                r#"
                INSERT INTO public.vinculaciones_obras_liquidaciones (
                    id, id_registro_obra, id_liquidacion, dias_registro_obra
                ) VALUES ($1, $2, $3, $4)
                "#,
                &[&id_vinc, &id_registro_obra, &id_liquidacion, &dias_vinc],
            )?;
            vinculaciones_count += 1;
        }

        // Asegurar reporte_liquidacion si aún no existe para reflejar la obra en Reporte Liquidaciones
        if final_rep_liq_id.is_none() {
            let rep_liq_opt = tx.query_opt(
                "SELECT id FROM public.reportes_liquidaciones WHERE id_liquidacion = $1 LIMIT 1",
                &[&id_liquidacion],
            )?;
            let assigned_id = match rep_liq_opt {
                Some(r) => r.get(0),
                None => {
                    let new_rep_id = Uuid::now_v7();
                    tx.execute(
                        r#"
                        INSERT INTO public.reportes_liquidaciones (
                            id, id_liquidacion, id_revision, id_empleado, mes_periodo, anio_periodo, nombre_archivo
                        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
                        "#,
                        &[&new_rep_id, &id_liquidacion, &ctx.id_revision, &id_empleado, &mes_norm, &anio_norm, &truncate_str(Some(&nombre_arch), 350)],
                    )?;
                    new_rep_id
                }
            };
            final_rep_liq_id = Some(assigned_id);
        }

        // Asegurar vinculación en reportes_vinculaciones_obras_liquidaciones para cada obra
        if let Some(id_rep_liq) = final_rep_liq_id {
            let existing_rep_vinc = tx.query_opt(
                "SELECT id FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1 AND id_registro_obra = $2 LIMIT 1",
                &[&id_rep_liq, &Some(id_registro_obra)],
            )?;

            if let Some(erv) = existing_rep_vinc {
                let vid: Uuid = erv.get(0);
                let _ = tx.execute(
                    "UPDATE public.reportes_vinculaciones_obras_liquidaciones SET dias_registro_obra = $2 WHERE id = $1",
                    &[&vid, &Some(dias_vinc)],
                );
            } else {
                let id_rep_vinc = Uuid::now_v7();
                tx.execute(
                    r#"
                    INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (
                        id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra
                    ) VALUES ($1, $2, $3, $4)
                    "#,
                    &[&id_rep_vinc, &Some(id_registro_obra), &id_rep_liq, &Some(dias_vinc)],
                )?;
            }
        }
    }

    // Mirror to reportes_detalles_liquidaciones con prorrateo de obra aplicado para cada obra vinculada
    if let Some(id_rep_liq) = final_rep_liq_id {
        if processed_obras.is_empty() {
            for de in &emp.data_employee {
                let id_rep_det = Uuid::now_v7();
                let dias_det = de.dias.map(|d| d as i16);
                tx.execute(
                    r#"
                    INSERT INTO public.reportes_detalles_liquidaciones (
                        id, id_reporte_liquidacion, clave, fecha, dias, sdi, lic, inc, aus, cf,
                        exc_pat, exc_obr, pd_pat, pd_obr, gmp_pat, gmp_obr, rt, iv_pat, iv_obr,
                        gps, patronal, obrera, subtotal, id_registro_obra, dias_registro_obra
                    ) VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, NULL, NULL
                    )
                    "#,
                    &[
                        &id_rep_det,
                        &id_rep_liq,
                        &truncate_str(de.clave.as_deref().map(|s| s.to_uppercase()).as_deref(), 17),
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
                )?;
            }
        } else {
            let mut baja_inserted = false;
            for (_idx, (obra_id, d_o)) in processed_obras.iter().enumerate() {
                for de in &emp.data_employee {
                    let is_baja = de.clave.as_deref().map(|c| c.trim().eq_ignore_ascii_case("BAJA")).unwrap_or(false);
                    if is_baja {
                        if baja_inserted {
                            continue;
                        }
                        baja_inserted = true;
                    }

                    let id_rep_det = Uuid::now_v7();
                    let dias_det = de.dias.map(|d| d as i16);

                    let (factor, dias_ro) = match dias_det {
                        Some(d_c) if d_c > 0 => {
                            let f = (*d_o as f64 / d_c as f64).clamp(0.0, 10.0);
                            let dro = if is_baja { 0 } else if *d_o <= d_c { *d_o } else { d_c };
                            (f, Some(dro))
                        }
                        _ => {
                            (1.0, Some(if is_baja { 0 } else { *d_o }))
                        }
                    };

                    let prorate = |val: Option<f64>| -> Option<f64> {
                        if is_baja {
                            Some(0.0)
                        } else if factor != 1.0 {
                            val.map(|v| ((v * factor * 100.0).round()) / 100.0)
                        } else {
                            val
                        }
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
                            &id_rep_det,
                            &id_rep_liq,
                            &truncate_str(de.clave.as_deref().map(|s| s.to_uppercase()).as_deref(), 17),
                            &de.fecha,
                            &dias_det,
                            &de.sdi,
                            &de.lic,
                            &de.inc,
                            &de.aus,
                            &prorate(de.cf),
                            &prorate(de.exc_pat),
                            &prorate(de.exc_obr),
                            &prorate(de.pd_pat),
                            &prorate(de.pd_obr),
                            &prorate(de.gmp_pat),
                            &prorate(de.gmp_obr),
                            &prorate(de.rt),
                            &prorate(de.iv_pat),
                            &prorate(de.iv_obr),
                            &prorate(de.gps),
                            &prorate(de.patronal),
                            &prorate(de.obrera),
                            &prorate(de.subtotal),
                            &Some(obra_id),
                            &dias_ro,
                        ],
                    )?;
                }
            }
        }
    }

    Ok((empleados_count, liquidaciones_count, detalles_count, registros_obras_count, vinculaciones_count))
}

pub fn save_cedulas_concurrent(
    pool: &DbPool,
    businesses: &[Business],
    progress_cb: Option<&(dyn Fn(usize, usize) + Send + Sync)>,
) -> Result<CedulasDbStats, String> {
    if businesses.is_empty() {
        return Ok(CedulasDbStats::default());
    }

    let mut init_client = pool.get().map_err(|e| format!("Error de conexión al pool: {}", e))?;

    // Pre-resolve business contexts
    let mut biz_contexts: Vec<(BusinessContext, Vec<CedulaEmployee>)> = Vec::new();
    {
        let mut tx = init_client
            .transaction()
            .map_err(|e| format!("Error iniciando transacción de contexto de cédulas: {}", e))?;

        for biz in businesses {
            let ctx = resolve_business_context(&mut tx, biz)
                .map_err(|e| format!("Error resolviendo patrón de cédula: {}", e))?;
            biz_contexts.push((ctx, biz.employees.clone()));
        }

        tx.commit()
            .map_err(|e| format!("Error confirmando contextos de patrones: {}", e))?;
    }

    // Flatten all employees with their business context
    let mut tasks: Vec<(BusinessContext, CedulaEmployee)> = Vec::new();
    for (ctx, emps) in biz_contexts {
        for emp in emps {
            tasks.push((ctx.clone(), emp));
        }
    }

    let total = tasks.len();
    let done_counter = AtomicUsize::new(0);
    let liquidaciones_count = AtomicUsize::new(0);
    let detalles_count = AtomicUsize::new(0);
    let vinculaciones_count = AtomicUsize::new(0);
    let empleados_count = AtomicUsize::new(0);
    let registros_obras_count = AtomicUsize::new(0);
    let errores = Mutex::new(Vec::new());

    tasks.par_iter().for_each(|(ctx, emp)| {
        let mut client = match pool.get() {
            Ok(c) => c,
            Err(e) => {
                errores.lock().unwrap().push(format!("Error obteniendo conexión del pool para cédula: {}", e));
                return;
            }
        };

        let mut tx = match client.transaction() {
            Ok(t) => t,
            Err(e) => {
                errores.lock().unwrap().push(format!("Error iniciando transacción para trabajador de cédula: {}", e));
                return;
            }
        };

        match save_single_cedula_employee_tx(&mut tx, emp, ctx) {
            Ok((e_c, l_c, d_c, ro_c, v_c)) => {
                if let Err(e) = tx.commit() {
                    errores.lock().unwrap().push(format!(
                        "Error haciendo commit para trabajador {}: {}",
                        emp.nombre.as_deref().unwrap_or("N/D"), e
                    ));
                } else {
                    empleados_count.fetch_add(e_c, Ordering::Relaxed);
                    liquidaciones_count.fetch_add(l_c, Ordering::Relaxed);
                    detalles_count.fetch_add(d_c, Ordering::Relaxed);
                    registros_obras_count.fetch_add(ro_c, Ordering::Relaxed);
                    vinculaciones_count.fetch_add(v_c, Ordering::Relaxed);
                }
            }
            Err(e) => {
                let _ = tx.rollback();
                errores.lock().unwrap().push(format!(
                    "Error guardando trabajador de cédula ({}): {:#?}",
                    emp.nombre.as_deref().unwrap_or("N/D"), e
                ));
            }
        }

        let done = done_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(cb) = progress_cb {
            if done % 10 == 0 || done == total {
                cb(done, total);
            }
        }
    });

    let stats = CedulasDbStats {
        liquidaciones_guardadas: liquidaciones_count.load(Ordering::Relaxed),
        detalles_guardados: detalles_count.load(Ordering::Relaxed),
        vinculaciones_guardadas: vinculaciones_count.load(Ordering::Relaxed),
        empleados_guardados: empleados_count.load(Ordering::Relaxed),
        registros_obras_guardados: registros_obras_count.load(Ordering::Relaxed),
        errores: errores.into_inner().unwrap(),
    };

    crate::app_logger::log_db_op(
        "INSERT/UPDATE_CONCURRENT",
        &[
            "public.empleados",
            "public.liquidaciones",
            "public.detalles_liquidaciones",
            "public.registros_obras",
            "public.vinculaciones_obras_liquidaciones",
        ],
        "save_cedulas_concurrent",
        &format!(
            "empleados={}, liquidaciones={}, detalles={}, obras={}, vinculaciones={}, errores={}",
            stats.empleados_guardados, stats.liquidaciones_guardadas, stats.detalles_guardados,
            stats.registros_obras_guardados, stats.vinculaciones_guardadas, stats.errores.len()
        ),
        Some(stats.detalles_guardados),
    );

    Ok(stats)
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
                let (e_c, l_c, d_c, ro_c, v_c) = save_single_cedula_employee_tx(&mut tx, emp, &ctx)?;
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
                    stats.errores.push(format!("Error haciendo commit para cédula: {}", e));
                }
            }
            Err(e) => {
                let _ = tx.rollback();
                stats.errores.push(format!("Error guardando cédula: {}", e));
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

