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
    let act_upper = biz.actividad.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let dom_upper = biz.domicilio.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let cp_upper = biz.codigo_postal.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
    let ent_upper = biz.entidad.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
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

            return Ok(BusinessContext {
                id_cliente,
                id_registro_patronal,
                id_revision,
                reg_11,
                delegacion: del_upper,
                subdelegacion: subdel_upper,
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

    let act_upper = biz.actividad.as_deref().map(|s| s.trim().to_uppercase());
    let dom_upper = biz.domicilio.as_deref().map(|s| s.trim().to_uppercase());
    let cp_upper = biz.codigo_postal.as_deref().map(|s| s.trim().to_uppercase());
    let ent_upper = biz.entidad.as_deref().map(|s| s.trim().to_uppercase());
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
    let sample_mes = biz.employees.first().and_then(|e| e.mes_periodo.as_deref()).unwrap_or("ENERO");
    let sample_anio = biz.employees.first().and_then(|e| e.anio_periodo.as_deref()).unwrap_or("2024");
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

    Ok(BusinessContext {
        id_cliente,
        id_registro_patronal,
        id_revision,
        reg_11,
        delegacion: del_upper,
        subdelegacion: subdel_upper,
    })
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

    let emp_nombre = emp.nombre.as_deref().map(|s| s.trim().to_uppercase());
    let emp_rfc = emp.rfc.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    });
    let emp_curp = emp.curp.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    });
    let emp_nss = emp.no_seguro_social.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    });
    let emp_clave_ubicacion = truncate_str(emp.clave_ubicacion.as_deref(), 50).filter(|ub| {
        let clean = ub.trim();
        !clean.is_empty()
            && clean != "—"
            && clean != "-"
            && !crate::cedulas::employees::is_curp_candidate(clean)
            && !crate::cedulas::employees::is_rfc_candidate(clean)
    });

    let mut empleado_id: Option<Uuid> = None;
    if let Some(ref nss) = emp_nss {
        if !nss.is_empty() {
            let row = tx.query_opt(
                "SELECT id FROM public.empleados WHERE id_cliente = $1 AND nss = $2 LIMIT 1",
                &[&ctx.id_cliente, &truncate_str(Some(nss), 11)],
            )?;
            if let Some(r) = row {
                empleado_id = Some(r.get(0));
            }
        }
    }
    if empleado_id.is_none() {
        if let Some(ref curp) = emp_curp {
            if !curp.is_empty() && !crate::validation::is_generic_sat_identifier(curp) {
                let row = tx.query_opt(
                    "SELECT id FROM public.empleados WHERE id_cliente = $1 AND curp = $2 LIMIT 1",
                    &[&ctx.id_cliente, &truncate_str(Some(curp), 18)],
                )?;
                if let Some(r) = row {
                    empleado_id = Some(r.get(0));
                }
            }
        }
    }
    if empleado_id.is_none() {
        if let Some(ref rfc) = emp_rfc {
            if !rfc.is_empty() && !crate::validation::is_generic_sat_identifier(rfc) {
                let row = tx.query_opt(
                    "SELECT id FROM public.empleados WHERE id_cliente = $1 AND rfc = $2 LIMIT 1",
                    &[&ctx.id_cliente, &truncate_str(Some(rfc), 13)],
                )?;
                if let Some(r) = row {
                    empleado_id = Some(r.get(0));
                }
            }
        }
    }
    if empleado_id.is_none() {
        if let Some(ref nom) = emp_nombre {
            let row = tx.query_opt(
                "SELECT id FROM public.empleados WHERE id_cliente = $1 AND nombre = $2 LIMIT 1",
                &[&ctx.id_cliente, &truncate_str(Some(nom), 100)],
            )?;
            if let Some(r) = row {
                empleado_id = Some(r.get(0));
            }
        }
    }

    let id_empleado: Uuid = match empleado_id {
        Some(id) => {
            tx.execute(
                r#"
                UPDATE public.empleados
                SET nombre = COALESCE(public.empleados.nombre, $2),
                    rfc = COALESCE(public.empleados.rfc, $3),
                    curp = COALESCE(public.empleados.curp, $4),
                    nss = COALESCE(public.empleados.nss, $5),
                    clave_ubicacion = COALESCE(public.empleados.clave_ubicacion, $6)
                WHERE id = $1
                "#,
                &[
                    &id,
                    &truncate_str(emp_nombre.as_deref(), 100),
                    &truncate_str(emp_rfc.as_deref(), 13),
                    &truncate_str(emp_curp.as_deref(), 18),
                    &truncate_str(emp_nss.as_deref(), 11),
                    &emp_clave_ubicacion,
                ],
            )?;
            id
        }
        None => {
            let new_id = Uuid::now_v7();
            let row = tx.query_one(
                r#"
                INSERT INTO public.empleados (id, id_cliente, nombre, rfc, curp, nss, clave_ubicacion)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
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
                ],
            )?;
            empleados_count += 1;
            row.get(0)
        }
    };

    // Liquidación
    let mes_norm = normalize_mes_periodo(emp.mes_periodo.as_deref());
    let anio_norm = normalize_anio_periodo(emp.anio_periodo.as_deref());

    let nombre_arch = format!("Cédula SUA {} - {}.pdf", ctx.reg_11, mes_norm);

    // Evitar duplicados: verificar si ya existe liquidación para este empleado en esta revisión y período
    let existing_liq = tx.query_opt(
        r#"
        SELECT id FROM public.liquidaciones
        WHERE id_revision = $1 AND id_empleado = $2 AND mes_periodo = $3 AND anio_periodo = $4
        LIMIT 1
        "#,
        &[&ctx.id_revision, &id_empleado, &mes_norm, &anio_norm],
    )?;

    let (id_liquidacion, _is_new_liq) = match existing_liq {
        Some(row) => (row.get::<_, Uuid>(0), false),
        None => {
            let id_liq = Uuid::now_v7();
            tx.execute(
                r#"
                INSERT INTO public.liquidaciones (
                    id, id_revision, id_empleado, mes_periodo, anio_periodo, nombre_archivo
                ) VALUES (
                    $1, $2, $3, $4, $5, $6
                )
                "#,
                &[
                    &id_liq,
                    &ctx.id_revision,
                    &id_empleado,
                    &mes_norm,
                    &anio_norm,
                    &truncate_str(Some(&nombre_arch), 350),
                ],
            )?;
            liquidaciones_count += 1;
            (id_liq, true)
        }
    };

    // Determinar si la liquidación cae dentro de los años y meses de las obras de la revisión
    let obra_dates = tx.query(
        r#"
        SELECT ro.fecha_inicio, ro.fecha_fin
        FROM public.registros_obras ro
        WHERE ro.id_revision = $1
           OR ro.id_registro_patronal = (SELECT r.id_registro_patronal FROM public.revisiones r WHERE r.id = $1)
        "#,
        &[&ctx.id_revision],
    )?;

    let mut anios_filtro: BTreeSet<i16> = BTreeSet::new();
    let mut meses_filtro: BTreeSet<String> = BTreeSet::new();

    for od in obra_dates {
        let f_ini: Option<NaiveDate> = od.get(0);
        let f_fin: Option<NaiveDate> = od.get(1);
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

    let should_mirror = if anios_filtro.is_empty() || meses_filtro.is_empty() {
        true
    } else {
        let mes_check = mes_norm.trim().to_uppercase();
        let anio_check = anio_norm;
        anios_filtro.contains(&anio_check) && meses_filtro.contains(&mes_check)
    };

    let id_rep_liq_opt = if should_mirror {
        let rep_opt = tx.query_opt(
            "SELECT id FROM public.reportes_liquidaciones WHERE id_liquidacion = $1 LIMIT 1",
            &[&id_liquidacion],
        )?;
        let id_rep = match rep_opt {
            Some(r) => r.get(0),
            None => {
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
                new_id
            }
        };
        Some(id_rep)
    } else {
        None
    };

    // Detalles Liquidación: insertar solo si no existen previamente para esta liquidación
    let existing_det_count: i64 = tx.query_one(
        "SELECT COUNT(*) FROM public.detalles_liquidaciones WHERE id_liquidacion = $1",
        &[&id_liquidacion],
    )?.get(0);

    if existing_det_count == 0 {
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
    }

    // Mirror to reportes_detalles_liquidaciones si aplica por filtro de fechas
    if let Some(id_rep_liq) = id_rep_liq_opt {
        let rep_det_count: i64 = tx.query_one(
            "SELECT COUNT(*) FROM public.reportes_detalles_liquidaciones WHERE id_reporte_liquidacion = $1",
            &[&id_rep_liq],
        )?.get(0);

        if rep_det_count == 0 {
            for de in &emp.data_employee {
                let id_rep_det = Uuid::now_v7();
                let dias_det = de.dias.map(|d| d as i16);
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
                        &id_rep_det,
                        &id_rep_liq,
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
            }
        }
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

        // Asegurar vinculación en reportes_vinculaciones_obras_liquidaciones
        if let Some(id_rep_liq) = final_rep_liq_id {
            let existing_rep_vinc = tx.query_opt(
                "SELECT id FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_reporte_liquidacion = $1 LIMIT 1",
                &[&id_rep_liq],
            )?;

            if let Some(erv) = existing_rep_vinc {
                let vid: Uuid = erv.get(0);
                let _ = tx.execute(
                    "UPDATE public.reportes_vinculaciones_obras_liquidaciones SET id_registro_obra = $2, dias_registro_obra = $3 WHERE id = $1",
                    &[&vid, &Some(id_registro_obra), &Some(dias_vinc)],
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

    // Si tiene obra asignada en reportes_vinculaciones_obras_liquidaciones, recalcular y guardar detalles en reportes_detalles_liquidaciones
    if let Some(id_rep_liq) = final_rep_liq_id {
        let rv_info = tx.query_opt(
            r#"
            SELECT rvol.id_registro_obra, rvol.dias_registro_obra, rdl.dias
            FROM public.reportes_vinculaciones_obras_liquidaciones rvol
            JOIN public.reportes_detalles_liquidaciones rdl ON rdl.id_reporte_liquidacion = rvol.id_reporte_liquidacion
            WHERE rvol.id_reporte_liquidacion = $1
            LIMIT 1
            "#,
            &[&id_rep_liq],
        )?;

        if let Some(rvr) = rv_info {
            let id_ro: Option<Uuid> = rvr.get(0);
            let d_obra: Option<i16> = rvr.get(1);
            let d_ced: Option<i16> = rvr.get(2);

            if id_ro.is_some() && d_obra.is_some() && d_ced.is_some() && d_ced.unwrap() > 0 {
                let factor = (d_obra.unwrap() as f64 / d_ced.unwrap() as f64).clamp(0.0, 10.0);
                let _ = tx.execute(
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
                );
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

