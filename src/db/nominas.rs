use chrono::Utc;
use postgres::{Client, Transaction};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use uuid::Uuid;

use crate::db::client::DbPool;
use crate::nominas::models::Employee;

#[derive(Debug, Default, Clone)]
pub struct NominasDbStats {
    pub nominas_guardadas: usize,
    pub cfdis_guardados: usize,
    pub contratos_guardados: usize,
    pub detalles_guardados: usize,
    pub empleados_guardados: usize,
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

#[derive(Hash, Eq, PartialEq, Clone)]
struct CompanyKey {
    rfc_emisor: Option<String>,
    nombre_emisor: String,
    registro_patronal: String,
    anio: i32,
    mes_num: u32,
}

fn resolve_company_context(
    tx: &mut Transaction,
    key: &CompanyKey,
    revision_id: Uuid,
) -> Result<(Uuid, Uuid, Uuid), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Cliente
    let nombre_emisor_upper = key.nombre_emisor.trim().to_uppercase();
    let rfc_patron = key.rfc_emisor.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    }).filter(|s| !s.is_empty());

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
            &[&nombre_emisor_upper],
        )?;
        if let Some(row) = row_opt {
            cliente_id = Some(row.get(0));
        }
    }

    let id_cliente = match cliente_id {
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
                &[&new_id, &nombre_emisor_upper, &nombre_emisor_upper, &rfc_val],
            )?;
            row.get(0)
        }
    };

    // 2. Registro Patronal
    let raw_reg = key.registro_patronal.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase();
    let reg_11: String = raw_reg.chars().take(11).collect();
    let reg_id_opt = tx.query_opt(
        "SELECT id FROM public.registros_patronales WHERE id_cliente = $1 AND registro = $2 LIMIT 1",
        &[&id_cliente, &reg_11],
    )?;

    let id_registro_patronal: Uuid = match reg_id_opt {
        Some(row) => row.get(0),
        None => {
            let new_id = Uuid::now_v7();
            let row = tx.query_one(
                r#"
                INSERT INTO public.registros_patronales (id, id_cliente, registro)
                VALUES ($1, $2, $3)
                ON CONFLICT (id_cliente, registro) DO UPDATE SET registro = EXCLUDED.registro
                RETURNING id
                "#,
                &[&new_id, &id_cliente, &reg_11],
            )?;
            row.get(0)
        }
    };

    // 3. Revisión - DEBE venir de la UI (no se crean sintéticas)
    let id_revision = revision_id;

    Ok((id_cliente, id_registro_patronal, id_revision))
}

fn save_single_nomina_tx(
    tx: &mut Transaction,
    emp: &Employee,
    id_cliente: Uuid,
    _id_registro_patronal: Uuid,
    id_revision: Uuid,
    tipo_concepto_cache: &HashMap<String, Uuid>,
    conceptos_cache: &Mutex<HashMap<(Uuid, String), Uuid>>,
    conceptos_cfdi_cache: &Mutex<HashMap<(String, String), Uuid>>,
) -> Result<(usize, usize, usize, usize, usize), Box<dyn std::error::Error + Send + Sync>> {
    let mut empleados_count = 0;
    let mut cfdis_count = 0;
    let mut contratos_count = 0;
    let mut nominas_count = 0;
    let mut detalles_count = 0;

    // Idempotencia: si el UUID del timbre ya está registrado, esta nómina ya se
    // guardó en una ejecución anterior. Se omite ANTES de crear el `cfdis` y el
    // `contratos` para no dejar filas huérfanas que queden apuntando a una
    // nómina inexistente, y porque la restricción `uq_nominas_timbre_uuid` haría
    // fallar la transacción completa.
    //
    // Devuelve ceros en todas las cuentas: no se insertó nada; la nómina ya
    // existía. La estadística `nominas_guardadas` no se incrementa.
    let timbre_uuid_parsed = emp.uuid.as_deref().and_then(|u| Uuid::parse_str(u).ok());
    if let Some(uuid_timbre) = timbre_uuid_parsed {
        let ya_existe: bool = tx
            .query_opt(
                "SELECT 1 FROM public.nominas WHERE timbre_uuid = $1 LIMIT 1",
                &[&uuid_timbre],
            )?
            .is_some();
        if ya_existe {
            log::info!(
                "Nómina con UUID {} ya registrada; se omite para no duplicar.",
                uuid_timbre
            );
            return Ok((0, 0, 0, 0, 0));
        }
    }

    // Empleado
    let emp_nombre = emp.nombre.as_deref().map(|s| s.trim().to_uppercase());
    let mut emp_rfc = emp.rfc_receptor.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    }).filter(|s| !s.is_empty() && (s.len() == 13 || s.len() == 12));
    let mut emp_curp = emp.curp_receptor.as_deref().map(|s| {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase()
    }).filter(|s| !s.is_empty() && s.len() == 18);
    let emp_nss = emp.numseguridadsocial.as_deref().map(|s| {
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
        let curp_gen = crate::cedulas::curp_renapo::calcular_curp_oficial(
            emp_nombre.as_deref().unwrap(),
            emp_rfc.as_deref(),
            emp_nss.as_deref(),
            None,
            None,
        );
        if !curp_gen.is_empty() && crate::cedulas::curp_renapo::validar_curp_formato(&curp_gen) {
            emp_curp = Some(curp_gen);
        } else {
            emp_curp = None;
        }
    }

    // Si el RFC no vino en el archivo, intentar generarlo con los datos que se tienen; en caso de no poder, usar el genérico oficial del SUA
    if emp_rfc.is_none() {
        if puede_calcular && emp_curp.is_some() {
            let rfc_gen = crate::cedulas::rfc_sat::calcular_rfc_sat(
                emp_nombre.as_deref().unwrap(),
                emp_curp.as_deref(),
                emp_nss.as_deref(),
            );
            if rfc_gen.len() == 13 && rfc_gen != "0000000000000" && rfc_gen != "XAXX010101000" {
                emp_rfc = Some(rfc_gen);
            } else {
                emp_rfc = Some("XAXX010101000".to_string());
            }
        } else {
            emp_rfc = Some("XAXX010101000".to_string());
        }
    }

    let mut empleado_id: Option<Uuid> = None;
    if let Some(ref nss) = emp_nss {
        if !nss.is_empty() {
            let row = tx.query_opt(
                "SELECT id FROM public.empleados WHERE id_cliente = $1 AND nss = $2 LIMIT 1",
                &[&id_cliente, &truncate_str(Some(nss), 11)],
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
                    &[&id_cliente, &truncate_str(Some(curp), 18)],
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
                    &[&id_cliente, &truncate_str(Some(rfc), 13)],
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
                &[&id_cliente, &truncate_str(Some(nom), 100)],
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
                SET nombre = COALESCE(NULLIF(TRIM(public.empleados.nombre), ''), $2),
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
                    nss = COALESCE(NULLIF(TRIM(public.empleados.nss), ''), $5)
                WHERE id = $1
                "#,
                &[
                    &id,
                    &truncate_str(emp_nombre.as_deref(), 100),
                    &truncate_str(emp_rfc.as_deref(), 13),
                    &truncate_str(emp_curp.as_deref(), 18),
                    &truncate_str(emp_nss.as_deref(), 11),
                ],
            )?;
            id
        }
        None => {
            let new_id = Uuid::now_v7();
            let row = tx.query_one(
                r#"
                INSERT INTO public.empleados (id, id_cliente, nombre, rfc, curp, nss)
                VALUES ($1, $2, $3, $4, $5, $6)
                RETURNING id
                "#,
                &[
                    &new_id,
                    &id_cliente,
                    &truncate_str(emp_nombre.as_deref(), 100),
                    &truncate_str(emp_rfc.as_deref(), 13),
                    &truncate_str(emp_curp.as_deref(), 18),
                    &truncate_str(emp_nss.as_deref(), 11),
                ],
            )?;
            empleados_count += 1;
            row.get(0)
        }
    };

    // CFDI
    let id_cfdi = Uuid::now_v7();
    let fecha_emision = emp.cfdi.fecha_emision.unwrap_or_else(Utc::now);
    tx.execute(
        r#"
        INSERT INTO public.cfdis (
            id, version_cfdi, fecha_emision, numero_certificado, moneda, tipo_comprobante,
            exportacion, metodo_pago, serie, folio, lugar_expedicion, subtotal, descuento, total
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14
        )
        "#,
        &[
            &id_cfdi,
            &truncate_str(emp.cfdi.version_cfdi.as_deref(), 5),
            &fecha_emision,
            &truncate_str(emp.cfdi.numero_certificado.as_deref(), 50),
            &truncate_str(emp.cfdi.moneda.as_deref(), 3),
            &truncate_str(emp.cfdi.tipo_comprobante.as_deref(), 1),
            &truncate_str(emp.cfdi.exportacion.as_deref(), 2),
            &truncate_str(emp.cfdi.metodo_pago.as_deref(), 3),
            &truncate_str(emp.cfdi.serie.as_deref(), 5),
            &truncate_str(emp.cfdi.folio.as_deref(), 20),
            &truncate_str(emp.cfdi.lugar_expedicion.as_deref(), 20),
            &emp.cfdi.subtotal,
            &emp.cfdi.descuento,
            &emp.cfdi.total.or(emp.total),
        ],
    )?;
    cfdis_count += 1;

    // Conceptos CFDI y Lista Conceptos CFDI
    for c in &emp.cfdi.conceptos {
        let clave_prod_serv = truncate_str(c.clave_prod_serv.as_deref(), 8);
        let clave_unidad = truncate_str(c.clave_unidad.as_deref(), 5);
        let desc = truncate_str(c.descripcion.as_deref(), 150);

        let id_concepto_cfdi: Uuid = if let (Some(cps), Some(ds)) = (&clave_prod_serv, &desc) {
            let key = (cps.clone(), ds.clone());
            let cached_id = {
                let cache = conceptos_cfdi_cache.lock().unwrap();
                cache.get(&key).copied()
            };

            match cached_id {
                Some(cid) => cid,
                None => {
                    let opt = tx.query_opt(
                        "SELECT id FROM public.conceptos_cfdis WHERE clave_prod_serv = $1 AND descripcion = $2 LIMIT 1",
                        &[cps, ds],
                    )?;
                    let final_id = match opt {
                        Some(r) => r.get(0),
                        None => {
                            let cid = Uuid::now_v7();
                            let r = tx.query_one(
                                r#"
                                INSERT INTO public.conceptos_cfdis (id, clave_prod_serv, clave_unidad, descripcion)
                                VALUES ($1, $2, $3, $4)
                                RETURNING id
                                "#,
                                &[&cid, &clave_prod_serv, &clave_unidad, &desc],
                            )?;
                            r.get(0)
                        }
                    };
                    conceptos_cfdi_cache.lock().unwrap().insert(key, final_id);
                    final_id
                }
            }
        } else {
            let cid = Uuid::now_v7();
            let r = tx.query_one(
                r#"
                INSERT INTO public.conceptos_cfdis (id, clave_prod_serv, clave_unidad, descripcion)
                VALUES ($1, $2, $3, $4)
                RETURNING id
                "#,
                &[&cid, &clave_prod_serv, &clave_unidad, &desc],
            )?;
            r.get(0)
        };

        let id_lista = Uuid::now_v7();
        tx.execute(
            r#"
            INSERT INTO public.lista_conceptos_cfdis (
                id, id_cfdi, id_concepto_cfdi, cantidad, valor_unitario, importe, descuento, objeto_imp
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8
            )
            "#,
            &[
                &id_lista,
                &id_cfdi,
                &id_concepto_cfdi,
                &c.cantidad,
                &c.valor_unitario,
                &c.importe,
                &c.descuento,
                &truncate_str(c.objeto_imp.as_deref(), 5),
            ],
        )?;
    }

    // Contrato
    let id_contrato = Uuid::now_v7();
    tx.execute(
        r#"
        INSERT INTO public.contratos (
            id, id_cfdi, domicilio_fiscal, regimen_fiscal, uso_cfdi, fecha_inicio_relacion_laboral,
            antiguedad, cod_antiguedad, tipo_contrato, es_sindicalizado, tipo_jornada, tipo_regimen,
            num_empleado, departamento, puesto, riesgo_puesto, periodicidad_pago, clave_entidad_federativa
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18
        )
        "#,
        &[
            &id_contrato,
            &id_cfdi,
            &truncate_str(emp.contrato.domicilio_fiscal.as_deref(), 300),
            &truncate_str(emp.contrato.regimen_fiscal.as_deref(), 3),
            &truncate_str(emp.contrato.uso_cfdi.as_deref(), 5),
            &emp.contrato.fecha_inicio_relacion_laboral.or(emp.fechainiciorellaboral),
            &emp.contrato.antiguedad,
            &truncate_str(emp.contrato.cod_antiguedad.as_deref(), 10),
            &truncate_str(emp.contrato.tipo_contrato.as_deref(), 5),
            &emp.contrato.es_sindicalizado,
            &truncate_str(emp.contrato.tipo_jornada.as_deref(), 2),
            &truncate_str(emp.contrato.tipo_regimen.as_deref(), 5),
            &truncate_str(emp.contrato.num_empleado.as_deref(), 20),
            &truncate_str(emp.contrato.departamento.as_deref(), 100),
            &truncate_str(emp.contrato.puesto.as_deref(), 100),
            &truncate_str(emp.contrato.riesgo_puesto.as_deref(), 2),
            &truncate_str(emp.contrato.periodicidad_pago.as_deref().or(emp.nomina.periodicidad_pago.as_deref()), 2),
            &truncate_str(emp.contrato.clave_entidad_federativa.as_deref(), 3),
        ],
    )?;
    contratos_count += 1;

    // Nómina
    let id_nomina = Uuid::now_v7();
    let timbre_uuid_parsed = emp.uuid.as_deref().and_then(|u| Uuid::parse_str(u).ok());

    // `salario_base_cotizacion_aportacion_calculado` se guardaba duplicando el
    // valor DECLARADO (`salario_base_cotizacion_aportacion`), porque
    // `Employee::calculate_salaries()` no produce un SBC calculado: solo deriva
    // salario diario y salario diario integrado. Una columna "_calculado" que
    // repite el declarado hace que cualquier auditoría lea una cifra ficticia,
    // así que se persiste NULL en lugar de un duplicado silencioso. El par
    // REAL calculado (SD y SDI) sí se guarda, en las columnas homónimas.
    let sbc_calculado: Option<f64> = None;

    tx.execute(
        r#"
        INSERT INTO public.nominas (
            id, id_cfdi, id_revision, id_empleado, version_nomina, tipo_nomina, fecha_pago,
            fecha_inicial_pago, fecha_final_pago, num_dias_pagados, total_sueldos, total_gravado,
            total_exento, total_otras_deducciones, total_impuestos_retenidos,
            salario_base_cotizacion_aportacion, salario_diario_integrado, salario_diario_calculado,
            salario_base_cotizacion_aportacion_calculado, salario_diario_integrado_calculado,
            timbre_version, timbre_numero_cert_sat, timbre_rfc_proveedor_cert, timbre_uuid,
            timbre_fecha, nombre_archivo
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
            $21, $22, $23, $24, $25, $26
        )
        "#,
        &[
            &id_nomina,
            &id_cfdi,
            &id_revision,
            &id_empleado,
            &truncate_str(emp.nomina.version_nomina.as_deref(), 5),
            &truncate_str(emp.nomina.tipo_nomina.as_deref(), 3),
            &emp.nomina.fechapago,
            &emp.nomina.fechainicialpago,
            &emp.nomina.fechafinalpago,
            &emp.nomina.numdiaspagados,
            &emp.nomina.total_sueldos,
            &emp.nomina.total_gravado,
            &emp.nomina.total_exento,
            &emp.nomina.total_otras_deducciones,
            &emp.nomina.total_impuestos_retenidos,
            &emp.salariobasecotapor,
            &emp.salariodiariointegrado,
            &emp.salariodiariocalculado,
            &sbc_calculado,
            &emp.salariodiariointegradocalculado,
            &truncate_str(emp.timbre.version.as_deref(), 5),
            &truncate_str(emp.timbre.no_certificado_sat.as_deref(), 50),
            &truncate_str(emp.timbre.rfc_prov_certif.as_deref(), 13),
            &timbre_uuid_parsed,
            &emp.timbre.fecha_timbrado,
            &truncate_str(Some(&emp.nombrearchivo), 350),
        ],
    )?;
    nominas_count += 1;

    // Mirror to reportes_nominas (Working paper de auditoría)
    let id_rep_nomina = Uuid::now_v7();
    tx.execute(
        r#"
        INSERT INTO public.reportes_nominas (
            id, id_nomina, id_revision, id_empleado, version_nomina, tipo_nomina, fecha_pago,
            fecha_inicial_pago, fecha_final_pago, num_dias_pagados, total_sueldos, total_gravado,
            total_exento, total_otras_deducciones, total_impuestos_retenidos,
            salario_base_cotizacion_aportacion, salario_diario_integrado, salario_diario_calculado,
            salario_base_cotizacion_aportacion_calculado, salario_diario_integrado_calculado,
            timbre_version, timbre_numero_cert_sat, timbre_rfc_proveedor_cert, timbre_uuid,
            timbre_fecha, nombre_archivo
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
            $21, $22, $23, $24, $25, $26
        )
        "#,
        &[
            &id_rep_nomina,
            &id_nomina,
            &id_revision,
            &id_empleado,
            &truncate_str(emp.nomina.version_nomina.as_deref(), 5),
            &truncate_str(emp.nomina.tipo_nomina.as_deref(), 3),
            &emp.nomina.fechapago,
            &emp.nomina.fechainicialpago,
            &emp.nomina.fechafinalpago,
            &emp.nomina.numdiaspagados,
            &emp.nomina.total_sueldos,
            &emp.nomina.total_gravado,
            &emp.nomina.total_exento,
            &emp.nomina.total_otras_deducciones,
            &emp.nomina.total_impuestos_retenidos,
            &emp.salariobasecotapor,
            &emp.salariodiariointegrado,
            &emp.salariodiariocalculado,
            &sbc_calculado,
            &emp.salariodiariointegradocalculado,
            &truncate_str(emp.timbre.version.as_deref(), 5),
            &truncate_str(emp.timbre.no_certificado_sat.as_deref(), 50),
            &truncate_str(emp.timbre.rfc_prov_certif.as_deref(), 13),
            &timbre_uuid_parsed,
            &emp.timbre.fecha_timbrado,
            &truncate_str(Some(&emp.nombrearchivo), 350),
        ],
    )?;

    // Detalles de Nómina
    for d in &emp.detalles_nomina {
        let cat_upper = d.tipo_concepto_categoria.to_uppercase();
        let id_tipo_concepto = match tipo_concepto_cache.get(&cat_upper) {
            Some(id) => *id,
            None => {
                let cid = Uuid::now_v7();
                let r = tx.query_one(
                    "INSERT INTO public.tipos_conceptos (id, nombre) VALUES ($1, $2) ON CONFLICT (nombre) DO UPDATE SET nombre = EXCLUDED.nombre RETURNING id",
                    &[&cid, &truncate_str(Some(&cat_upper), 12)],
                )?;
                r.get(0)
            }
        };

        let tipo_sat_padded = format!("{:0>3}", d.tipo_sat.trim());
        let tipo_sat_3: String = tipo_sat_padded.chars().take(3).collect();
        let concepto_desc = if d.concepto.trim().is_empty() {
            format!("CONCEPTO {}", tipo_sat_3)
        } else {
            d.concepto.clone()
        };

        let key = (id_tipo_concepto, tipo_sat_3.clone());
        let cached_id = {
            let cache = conceptos_cache.lock().unwrap();
            cache.get(&key).copied()
        };

        let id_concepto: Uuid = match cached_id {
            Some(id) => id,
            None => {
                let opt_concepto = tx.query_opt(
                    "SELECT id FROM public.conceptos WHERE id_tipo_concepto = $1 AND tipo = $2 LIMIT 1",
                    &[&id_tipo_concepto, &tipo_sat_3],
                )?;
                let final_id = match opt_concepto {
                    Some(r) => r.get(0),
                    None => {
                        let cid = Uuid::now_v7();
                        let r = tx.query_one(
                            r#"
                            INSERT INTO public.conceptos (id, id_tipo_concepto, tipo, nombre)
                            VALUES ($1, $2, $3, $4)
                            ON CONFLICT (id_tipo_concepto, tipo) DO UPDATE SET nombre = EXCLUDED.nombre
                            RETURNING id
                            "#,
                            &[&cid, &id_tipo_concepto, &tipo_sat_3, &truncate_str(Some(&concepto_desc), 250)],
                        )?;
                        r.get(0)
                    }
                };
                conceptos_cache.lock().unwrap().insert(key, final_id);
                final_id
            }
        };

        let id_detalle = Uuid::now_v7();
        let clave_trun = truncate_str(Some(&d.clave), 10).unwrap_or_else(|| "001".to_string());
        let desc_trun = truncate_str(Some(&concepto_desc), 250).unwrap();

        tx.execute(
            r#"
            INSERT INTO public.detalles_nominas (
                id, id_nomina, id_concepto, clave, concepto, importe_gravado, importe_exento, importe
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8
            )
            "#,
            &[
                &id_detalle,
                &id_nomina,
                &id_concepto,
                &clave_trun,
                &desc_trun,
                &d.importe_gravado,
                &d.importe_exento,
                &d.importe,
            ],
        )?;

        // Mirror to reportes_detalles_nominas
        let id_rep_det = Uuid::now_v7();
        tx.execute(
            r#"
            INSERT INTO public.reportes_detalles_nominas (
                id, id_reporte_nomina, id_concepto, clave, concepto, importe_gravado, importe_exento, importe
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8
            )
            "#,
            &[
                &id_rep_det,
                &id_rep_nomina,
                &id_concepto,
                &clave_trun,
                &desc_trun,
                &d.importe_gravado,
                &d.importe_exento,
                &d.importe,
            ],
        )?;

        detalles_count += 1;
    }

    Ok((empleados_count, cfdis_count, contratos_count, nominas_count, detalles_count))
}

pub fn save_nominas_concurrent(
    pool: &DbPool,
    employees: &[Employee],
    progress_cb: Option<&(dyn Fn(usize, usize) + Send + Sync)>,
    revision_id: Uuid,
) -> Result<NominasDbStats, String> {
    if employees.is_empty() {
        return Ok(NominasDbStats::default());
    }

    let mut init_client = pool.get().map_err(|e| format!("Error de conexión al pool: {}", e))?;

    // Pre-cache tipos_conceptos
    let mut tipo_concepto_cache: HashMap<String, Uuid> = HashMap::new();
    let rows = init_client
        .query("SELECT id, TRIM(nombre) FROM public.tipos_conceptos", &[])
        .map_err(|e| format!("Error consultando tipos_conceptos: {}", e))?;
    for row in rows {
        let id: Uuid = row.get(0);
        let nombre: String = row.get(1);
        tipo_concepto_cache.insert(nombre.to_uppercase(), id);
    }

    // Pre-cache conceptos
    let conceptos_cache = Mutex::new(HashMap::new());
    if let Ok(c_rows) = init_client.query("SELECT id, id_tipo_concepto, TRIM(tipo) FROM public.conceptos", &[]) {
        let mut cache = conceptos_cache.lock().unwrap();
        for r in c_rows {
            let id: Uuid = r.get(0);
            let id_tipo: Uuid = r.get(1);
            let tipo: String = r.get(2);
            cache.insert((id_tipo, tipo), id);
        }
    }

    // Pre-cache conceptos_cfdis
    let conceptos_cfdi_cache = Mutex::new(HashMap::new());
    if let Ok(cc_rows) = init_client.query("SELECT id, TRIM(clave_prod_serv), TRIM(descripcion) FROM public.conceptos_cfdis", &[]) {
        let mut cache = conceptos_cfdi_cache.lock().unwrap();
        for r in cc_rows {
            let id: Uuid = r.get(0);
            let cps: String = r.get(1);
            let desc: String = r.get(2);
            cache.insert((cps, desc), id);
        }
    }

    // Pre-resolve all unique companies
    let mut company_map: HashMap<CompanyKey, (Uuid, Uuid, Uuid)> = HashMap::new();
    {
        let mut tx = init_client
            .transaction()
            .map_err(|e| format!("Error iniciando transacción de precálculo: {}", e))?;

        for emp in employees {
            let rfc_emisor = emp
                .rfc_emisor
                .as_deref()
                .map(|s| s.trim().to_uppercase())
                .filter(|s| !s.is_empty());
            let nombre_emisor = emp
                .nombre_emisor
                .as_deref()
                .map(|s| s.trim().to_uppercase())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| rfc_emisor.clone().unwrap_or_else(|| "CLIENTE SIN NOMBRE".to_string()));

            let raw_reg = emp
                .registro_patronal
                .as_deref()
                .map(|s| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "00000000000".to_string());

            let (anio, mes_num) = emp.get_year_month();

            let key = CompanyKey {
                rfc_emisor,
                nombre_emisor,
                registro_patronal: raw_reg,
                anio,
                mes_num,
            };

            if !company_map.contains_key(&key) {
                let context = resolve_company_context(&mut tx, &key, revision_id)
                    .map_err(|e| format!("Error resolviendo cliente/empresa: {}", e))?;
                company_map.insert(key, context);
            }
        }

        tx.commit()
            .map_err(|e| format!("Error confirmando precálculo de empresas: {}", e))?;
    }

    // Concurrently process all employees in parallel
    let total = employees.len();
    let done_counter = AtomicUsize::new(0);
    let nominas_count = AtomicUsize::new(0);
    let cfdis_count = AtomicUsize::new(0);
    let contratos_count = AtomicUsize::new(0);
    let detalles_count = AtomicUsize::new(0);
    let empleados_count = AtomicUsize::new(0);
    let errores = Mutex::new(Vec::new());

    employees.par_iter().for_each(|emp| {
        let rfc_emisor = emp
            .rfc_emisor
            .as_deref()
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty());
        let nombre_emisor = emp
            .nombre_emisor
            .as_deref()
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| rfc_emisor.clone().unwrap_or_else(|| "CLIENTE SIN NOMBRE".to_string()));

        let raw_reg = emp
            .registro_patronal
            .as_deref()
            .map(|s| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "00000000000".to_string());

        let (anio, mes_num) = emp.get_year_month();

        let key = CompanyKey {
            rfc_emisor,
            nombre_emisor,
            registro_patronal: raw_reg,
            anio,
            mes_num,
        };

        let &(id_cliente, id_registro_patronal, id_revision) = match company_map.get(&key) {
            Some(ctx) => ctx,
            None => {
                errores.lock().unwrap().push(format!(
                    "Contexto de empresa no encontrado para nómina: {}",
                    emp.nombrearchivo
                ));
                return;
            }
        };

        let mut client = match pool.get() {
            Ok(c) => c,
            Err(e) => {
                errores.lock().unwrap().push(format!("Error obteniendo conexión del pool: {}", e));
                return;
            }
        };

        let mut tx = match client.transaction() {
            Ok(t) => t,
            Err(e) => {
                errores.lock().unwrap().push(format!("Error iniciando transacción: {}", e));
                return;
            }
        };

        match save_single_nomina_tx(
            &mut tx,
            emp,
            id_cliente,
            id_registro_patronal,
            id_revision,
            &tipo_concepto_cache,
            &conceptos_cache,
            &conceptos_cfdi_cache,
        ) {
            Ok((e_c, c_c, ct_c, n_c, d_c)) => {
                if let Err(e) = tx.commit() {
                    errores.lock().unwrap().push(format!(
                        "Error haciendo commit para {}: {}",
                        emp.nombrearchivo, e
                    ));
                } else {
                    empleados_count.fetch_add(e_c, Ordering::Relaxed);
                    cfdis_count.fetch_add(c_c, Ordering::Relaxed);
                    contratos_count.fetch_add(ct_c, Ordering::Relaxed);
                    nominas_count.fetch_add(n_c, Ordering::Relaxed);
                    detalles_count.fetch_add(d_c, Ordering::Relaxed);
                }
            }
            Err(e) => {
                let _ = tx.rollback();
                errores.lock().unwrap().push(format!(
                    "Error guardando nómina ({}): {:#?}",
                    emp.nombrearchivo, e
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

    let stats = NominasDbStats {
        nominas_guardadas: nominas_count.load(Ordering::Relaxed),
        cfdis_guardados: cfdis_count.load(Ordering::Relaxed),
        contratos_guardados: contratos_count.load(Ordering::Relaxed),
        detalles_guardados: detalles_count.load(Ordering::Relaxed),
        empleados_guardados: empleados_count.load(Ordering::Relaxed),
        errores: errores.into_inner().unwrap(),
    };

    Ok(stats)
}

pub fn save_nominas_to_db(
    client: &mut Client,
    employees: &[Employee],
    revision_id: Uuid,
) -> Result<NominasDbStats, String> {
    let mut stats = NominasDbStats::default();

    let mut tipo_concepto_cache: HashMap<String, Uuid> = HashMap::new();
    let rows = client
        .query("SELECT id, TRIM(nombre) FROM public.tipos_conceptos", &[])
        .map_err(|e| format!("Error consultando tipos_conceptos: {}", e))?;
    for row in rows {
        let id: Uuid = row.get(0);
        let nombre: String = row.get(1);
        tipo_concepto_cache.insert(nombre.to_uppercase(), id);
    }

    let conceptos_cache = Mutex::new(HashMap::new());
    if let Ok(c_rows) = client.query("SELECT id, id_tipo_concepto, TRIM(tipo) FROM public.conceptos", &[]) {
        let mut cache = conceptos_cache.lock().unwrap();
        for r in c_rows {
            let id: Uuid = r.get(0);
            let id_tipo: Uuid = r.get(1);
            let tipo: String = r.get(2);
            cache.insert((id_tipo, tipo), id);
        }
    }

    let conceptos_cfdi_cache = Mutex::new(HashMap::new());
    if let Ok(cc_rows) = client.query("SELECT id, TRIM(clave_prod_serv), TRIM(descripcion) FROM public.conceptos_cfdis", &[]) {
        let mut cache = conceptos_cfdi_cache.lock().unwrap();
        for r in cc_rows {
            let id: Uuid = r.get(0);
            let cps: String = r.get(1);
            let desc: String = r.get(2);
            cache.insert((cps, desc), id);
        }
    }

    for emp in employees {
        let mut tx = match client.transaction() {
            Ok(t) => t,
            Err(e) => {
                stats.errores.push(format!("Error iniciando transacción: {}", e));
                continue;
            }
        };

        let rfc_emisor = emp
            .rfc_emisor
            .as_deref()
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty());
        let nombre_emisor = emp
            .nombre_emisor
            .as_deref()
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| rfc_emisor.clone().unwrap_or_else(|| "CLIENTE SIN NOMBRE".to_string()));

        let raw_reg = emp
            .registro_patronal
            .as_deref()
            .map(|s| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "00000000000".to_string());

        let (anio, mes_num) = emp.get_year_month();

        let key = CompanyKey {
            rfc_emisor,
            nombre_emisor,
            registro_patronal: raw_reg,
            anio,
            mes_num,
        };

        let context = match resolve_company_context(&mut tx, &key, revision_id) {
            Ok(ctx) => ctx,
            Err(e) => {
                let _ = tx.rollback();
                stats.errores.push(format!("Error resolviendo empresa {}: {:#?}", emp.nombrearchivo, e));
                continue;
            }
        };

        let (id_cliente, id_registro_patronal, id_revision) = context;

        match save_single_nomina_tx(
            &mut tx,
            emp,
            id_cliente,
            id_registro_patronal,
            id_revision,
            &tipo_concepto_cache,
            &conceptos_cache,
            &conceptos_cfdi_cache,
        ) {
            Ok((e_c, c_c, ct_c, n_c, d_c)) => {
                if let Err(e) = tx.commit() {
                    stats.errores.push(format!("Error haciendo commit: {:?}", e));
                } else {
                    stats.empleados_guardados += e_c;
                    stats.cfdis_guardados += c_c;
                    stats.contratos_guardados += ct_c;
                    stats.nominas_guardadas += n_c;
                    stats.detalles_guardados += d_c;
                }
            }
            Err(e) => {
                let _ = tx.rollback();
                stats.errores.push(format!("Error guardando nómina ({}): {:#?}", emp.nombrearchivo, e));
            }
        }
    }

    Ok(stats)
}

/// Elimina TODAS las nóminas y sus detalles de la base de
/// datos, junto con las copias de las tablas de reporte.
///
/// Orden de borrado (respeta FKs):
///   1. `reportes_detalles_nominas` / `reportes_nominas`
///   2. `detalles_nominas` / `nominas`
///   3. Purga de huérfanos: `lista_conceptos_cfdis`,
///      `conceptos_cfdis`, `contratos` y `cfdis` que ya
///      no referen ninguna nómina restante.
///
/// Los catálogos (`tipos_conceptos`, `conceptos`) son
/// inmutables y nunca se tocan.
pub fn delete_all_nominas(client: &mut Client) -> Result<u64, String> {
    let rd = client
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
            format!("Error eliminando reportes_detalles_nominas: {}", e)
        })?;
    let rn = client
        .execute("DELETE FROM public.reportes_nominas", &[])
        .map_err(|e| format!("Error eliminando reportes_nominas: {}", e))?;
    let dn = client
        .execute("DELETE FROM public.detalles_nominas", &[])
        .map_err(|e| format!("Error eliminando detalles de nóminas: {}", e))?;
    let n = client
        .execute("DELETE FROM public.nominas", &[])
        .map_err(|e| format!("Error eliminando nóminas: {}", e))?;

    // Purga de CFDI, contratos y conceptos de CFDI que
    // quedaron huérfanos tras borrar las nóminas.
    let lc = client
        .execute(
            r#"
            DELETE FROM public.lista_conceptos_cfdis
            WHERE id_cfdi IN (
                SELECT cf.id FROM public.cfdis cf
                WHERE NOT EXISTS (
                    SELECT 1 FROM public.nominas n2
                    WHERE n2.id_cfdi = cf.id
                )
            )
            "#,
            &[],
        )
        .map_err(|e| {
            format!("Error eliminando lista_conceptos_cfdis: {}", e)
        })?;
    let cc = client
        .execute(
            r#"
            DELETE FROM public.conceptos_cfdis cc
            WHERE NOT EXISTS (
                SELECT 1 FROM public.lista_conceptos_cfdis lc
                WHERE lc.id_concepto_cfdi = cc.id
            )
            "#,
            &[],
        )
        .map_err(|e| {
            format!("Error eliminando conceptos_cfdis: {}", e)
        })?;
    let ct = client
        .execute(
            r#"
            DELETE FROM public.contratos ct
            WHERE NOT EXISTS (
                SELECT 1 FROM public.cfdis cf
                WHERE cf.id = ct.id_cfdi
            )
            "#,
            &[],
        )
        .map_err(|e| {
            format!("Error eliminando contratos: {}", e)
        })?;
    let cf = client
        .execute(
            r#"
            DELETE FROM public.cfdis cf
            WHERE NOT EXISTS (
                SELECT 1 FROM public.nominas n3
                WHERE n3.id_cfdi = cf.id
            )
            "#,
            &[],
        )
        .map_err(|e| format!("Error eliminando CFDI: {}", e))?;

    log::info!(
        "Borrado total de nóminas: {} reportes_detalles, {} reportes, \
         {} detalles, {} nominas, {} lista_cfdi, {} conceptos_cfdi, \
         {} contratos, {} cfdi",
        rd, rn, dn, n, lc, cc, ct, cf
    );
    Ok(rd + rn + dn + n + lc + cc + ct + cf)
}
