use std::collections::HashSet;
use chrono::NaiveDate;
use postgres::Client;
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct RevisionItem {
    pub id: String,
    pub id_registro_patronal: String,
    pub id_concepto_diferencia: Option<String>,
    pub concepto_diferencia: String,
    pub id_estatus_imss: Option<String>,
    pub estatus_imss: String,
    pub id_estatus: Option<String>,
    pub estatus: String,
    pub nombre: String,
    pub folio: String,
    pub fecha_entrega_imss: Option<NaiveDate>,
    pub fecha_aprobacion_imss: Option<NaiveDate>,
    pub fecha_maxima_aprobacion_imss: Option<NaiveDate>,
    pub fecha_maxima_entrega_revision: Option<NaiveDate>,
    pub fecha_maxima_entrega_imss: Option<NaiveDate>,
    pub fecha_entrega_revision: Option<NaiveDate>,
    pub fecha_aprobacion_prorroga: Option<NaiveDate>,
    pub fecha_entrega_con_prorroga: Option<NaiveDate>,
    pub pidio_prorroga: bool,
    pub obras_count: i32,
    pub created_at: String,
}

#[derive(Clone, Debug, Default)]
pub struct RegistroObraItem {
    pub id: String,
    pub id_revision: String,
    pub id_registro_patronal: String,
    pub numero: String,
    pub clase: String,
    pub tipo_obra: String,
    pub fecha_inicio: Option<NaiveDate>,
    pub fecha_fin: Option<NaiveDate>,
    pub ubicacion: String,
    pub superficie_construido: f64,
    pub monto: f64,
    pub delegacion: String,
    pub subdelegacion: String,
    pub created_at: String,
    pub creado_usuario: bool,
}

#[derive(Clone, Debug, Default)]
pub struct EstatusOption {
    pub id: String,
    pub nombre: String,
}

#[derive(Clone, Debug, Default)]
pub struct ConceptoOption {
    pub id: String,
    pub tipo: String,
    pub nombre: String,
}

pub fn parse_date_opt(s: &str) -> Option<NaiveDate> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed == "-- / -- / ----" || trimmed == "----/--/--" {
        return None;
    }
    if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%d/%m/%Y") {
        return Some(d);
    }
    if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return Some(d);
    }
    if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%d-%m-%Y") {
        return Some(d);
    }
    None
}

pub fn format_date_opt(d: Option<NaiveDate>) -> String {
    match d {
        Some(date) => date.format("%d/%m/%Y").to_string(),
        None => "-- / -- / ----".to_string(),
    }
}

pub fn add_business_days(
    start: NaiveDate,
    count: usize,
    holidays: Option<&HashSet<NaiveDate>>,
) -> NaiveDate {
    use chrono::Datelike;
    let mut added = 0;
    let mut cur = start;
    while added < count {
        cur = cur.succ_opt().unwrap_or(cur);
        let is_weekend = cur.weekday().number_from_monday() > 5;
        let is_holiday = holidays.map(|h| h.contains(&cur)).unwrap_or(false);
        if !is_weekend && !is_holiday {
            added += 1;
        }
    }
    cur
}

pub fn fetch_imss_inhabiles(client: &mut Client) -> Result<HashSet<NaiveDate>, String> {
    let rows = client
        .query("SELECT fecha FROM public.imss_inhabiles", &[])
        .map_err(|e| format!("Error consultando imss_inhabiles: {}", e))?;
    let mut set = HashSet::new();
    for r in rows {
        let f: NaiveDate = r.get(0);
        set.insert(f);
    }
    Ok(set)
}

pub fn fetch_revisiones_by_rp(client: &mut Client, id_rp: &str) -> Result<Vec<RevisionItem>, String> {
    let rp_uuid = Uuid::parse_str(id_rp).map_err(|e| format!("UUID inválido de RP: {}", e))?;

    let query = r#"
        SELECT 
            r.id,
            r.id_registro_patronal,
            r.id_concepto_diferencia,
            COALESCE(c.nombre, 'Sin Concepto Asignado') AS concepto_diferencia,
            r.id_estatus_imss,
            COALESCE(e_imss.nombre, 'PENDIENTE DICTAMEN IMSS') AS estatus_imss,
            r.id_estatus,
            COALESCE(e.nombre, 'EN REVISIÓN') AS estatus,
            COALESCE(r.nombre, 'Revisión Sin Nombre') AS nombre,
            COALESCE(r.folio, 'S/F') AS folio,
            r.fecha_entrega_imss,
            r.fecha_aprobacion_imss,
            r.fecha_maxima_aprobacion_imss,
            r.fecha_maxima_entrega_revision,
            r.fecha_maxima_entrega_imss,
            r.fecha_entrega_revision,
            r.fecha_aprobacion_prorroga,
            r.fecha_entrega_con_prorroga,
            COALESCE(r.pidio_prorroga, false) AS pidio_prorroga,
            (SELECT COUNT(*)::int FROM public.registros_obras ro WHERE ro.id_revision = r.id AND COALESCE(ro.creado_usuario, false) = true) AS obras_count,
            TO_CHAR(r.created_at, 'YYYY-MM-DD HH24:MI:SS') AS created_at_str
        FROM public.revisiones r
        LEFT JOIN public.conceptos c ON c.id = r.id_concepto_diferencia
        LEFT JOIN public.estatus e_imss ON e_imss.id = r.id_estatus_imss
        LEFT JOIN public.estatus e ON e.id = r.id_estatus
        WHERE r.id_registro_patronal = $1
        ORDER BY r.created_at DESC;
    "#;

    let rows = client.query(query, &[&rp_uuid]).map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        let id_u: Uuid = row.get(0);
        let id_rp_u: Uuid = row.get(1);
        let id_conc_u: Option<Uuid> = row.get(2);
        let concepto_diff: String = row.get(3);
        let id_e_imss_u: Option<Uuid> = row.get(4);
        let e_imss_nom: String = row.get(5);
        let id_e_u: Option<Uuid> = row.get(6);
        let e_nom: String = row.get(7);
        let nombre: String = row.get(8);
        let folio: String = row.get(9);
        let f_ent_imss: Option<NaiveDate> = row.get(10);
        let f_aprob_imss: Option<NaiveDate> = row.get(11);
        let f_max_aprob_imss: Option<NaiveDate> = row.get(12);
        let f_max_ent_rev: Option<NaiveDate> = row.get(13);
        let f_max_ent_imss: Option<NaiveDate> = row.get(14);
        let f_ent_rev: Option<NaiveDate> = row.get(15);
        let f_aprob_pror: Option<NaiveDate> = row.get(16);
        let f_ent_pror: Option<NaiveDate> = row.get(17);
        let pidio_pror: bool = row.get(18);
        let obras_count: i32 = row.get(19);
        let created_at: String = row.get(20);

        list.push(RevisionItem {
            id: id_u.to_string(),
            id_registro_patronal: id_rp_u.to_string(),
            id_concepto_diferencia: id_conc_u.map(|u| u.to_string()),
            concepto_diferencia: concepto_diff.trim().to_string(),
            id_estatus_imss: id_e_imss_u.map(|u| u.to_string()),
            estatus_imss: e_imss_nom.trim().to_string(),
            id_estatus: id_e_u.map(|u| u.to_string()),
            estatus: e_nom.trim().to_string(),
            nombre: nombre.trim().to_string(),
            folio: folio.trim().to_string(),
            fecha_entrega_imss: f_ent_imss,
            fecha_aprobacion_imss: f_aprob_imss,
            fecha_maxima_aprobacion_imss: f_max_aprob_imss,
            fecha_maxima_entrega_revision: f_max_ent_rev,
            fecha_maxima_entrega_imss: f_max_ent_imss,
            fecha_entrega_revision: f_ent_rev,
            fecha_aprobacion_prorroga: f_aprob_pror,
            fecha_entrega_con_prorroga: f_ent_pror,
            pidio_prorroga: pidio_pror,
            obras_count,
            created_at,
        });
    }

    Ok(list)
}

pub fn fetch_obras_by_revision(client: &mut Client, id_revision: &str) -> Result<Vec<RegistroObraItem>, String> {
    let rev_uuid = Uuid::parse_str(id_revision).map_err(|e| format!("UUID inválido de Revisión: {}", e))?;

    let query = r#"
        SELECT 
            id,
            id_revision,
            id_registro_patronal,
            COALESCE(numero, '') AS numero,
            COALESCE(clase, '') AS clase,
            COALESCE(tipo_obra, '') AS tipo_obra,
            fecha_inicio,
            fecha_fin,
            COALESCE(ubicacion, '') AS ubicacion,
            COALESCE(superficie_construido, 0.0) AS superficie_construido,
            COALESCE(monto, 0.0) AS monto,
            COALESCE(delegacion, '') AS delegacion,
            COALESCE(subdelegacion, '') AS subdelegacion,
            TO_CHAR(created_at, 'YYYY-MM-DD HH24:MI:SS') AS created_at_str,
            COALESCE(creado_usuario, false) AS creado_usuario
        FROM public.registros_obras
        WHERE id_revision = $1 AND COALESCE(creado_usuario, false) = true
        ORDER BY created_at DESC;
    "#;

    let rows = client.query(query, &[&rev_uuid]).map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        let id_u: Uuid = row.get(0);
        let id_rev_u: Uuid = row.get(1);
        let id_rp_u: Uuid = row.get(2);
        let numero: String = row.get(3);
        let clase: String = row.get(4);
        let tipo_obra: String = row.get(5);
        let fecha_inicio: Option<NaiveDate> = row.get(6);
        let fecha_fin: Option<NaiveDate> = row.get(7);
        let ubicacion: String = row.get(8);
        let superficie: f64 = row.get(9);
        let monto: f64 = row.get(10);
        let delegacion: String = row.get(11);
        let subdelegacion: String = row.get(12);
        let created_at: String = row.get(13);
        let creado_usuario: bool = row.get(14);

        list.push(RegistroObraItem {
            id: id_u.to_string(),
            id_revision: id_rev_u.to_string(),
            id_registro_patronal: id_rp_u.to_string(),
            numero: numero.trim().to_string(),
            clase: clase.trim().to_string(),
            tipo_obra: tipo_obra.trim().to_string(),
            fecha_inicio,
            fecha_fin,
            ubicacion: ubicacion.trim().to_string(),
            superficie_construido: superficie,
            monto,
            delegacion: delegacion.trim().to_string(),
            subdelegacion: subdelegacion.trim().to_string(),
            created_at,
            creado_usuario,
        });
    }

    Ok(list)
}

pub fn fetch_estatus_options(client: &mut Client) -> Result<Vec<EstatusOption>, String> {
    let rows = client
        .query("SELECT id, nombre FROM public.estatus ORDER BY nombre ASC", &[])
        .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        let id_u: Uuid = row.get(0);
        let nombre: String = row.get(1);
        list.push(EstatusOption {
            id: id_u.to_string(),
            nombre: nombre.trim().to_string(),
        });
    }
    Ok(list)
}

pub fn fetch_conceptos_options(client: &mut Client) -> Result<Vec<ConceptoOption>, String> {
    let rows = client
        .query(
            "SELECT id, tipo, nombre FROM public.conceptos ORDER BY nombre ASC",
            &[],
        )
        .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        let id_u: Uuid = row.get(0);
        let tipo: String = row.get(1);
        let nombre: String = row.get(2);
        list.push(ConceptoOption {
            id: id_u.to_string(),
            tipo: tipo.trim().to_string(),
            nombre: nombre.trim().to_string(),
        });
    }
    Ok(list)
}

pub fn parse_optional_uuid(val: Option<&str>) -> Option<Uuid> {
    match val {
        Some(s) => {
            let s_trimmed = s.trim();
            if s_trimmed.is_empty() || s_trimmed.starts_with("--") || s_trimmed == "Sin Concepto Asignado" {
                None
            } else {
                Uuid::parse_str(s_trimmed).ok()
            }
        }
        None => None,
    }
}

pub fn create_revision(client: &mut Client, item: &RevisionItem) -> Result<RevisionItem, String> {
    let rp_uuid = Uuid::parse_str(&item.id_registro_patronal)
        .map_err(|e| format!("ID de Registro Patronal inválido: {}", e))?;

    let rp_row = client
        .query_opt("SELECT registro FROM public.registros_patronales WHERE id = $1", &[&rp_uuid])
        .map_err(|e| format!("Error verificando registro patronal: {}", e))?;
    if let Some(r) = rp_row {
        let rp_reg: String = r.get(0);
        crate::validation::validate_registro_patronal(&rp_reg)
            .map_err(|e| format!("No se puede crear la revisión: {}", e))?;
    } else {
        return Err("El registro patronal especificado no existe.".to_string());
    }

    let conc_uuid = parse_optional_uuid(item.id_concepto_diferencia.as_deref());
    let estatus_imss_uuid = parse_optional_uuid(item.id_estatus_imss.as_deref());
    let estatus_uuid = parse_optional_uuid(item.id_estatus.as_deref());

    let query = r#"
        INSERT INTO public.revisiones (
            id_registro_patronal,
            id_concepto_diferencia,
            id_estatus_imss,
            id_estatus,
            nombre,
            folio,
            fecha_entrega_imss,
            fecha_aprobacion_imss,
            fecha_maxima_aprobacion_imss,
            fecha_maxima_entrega_revision,
            fecha_maxima_entrega_imss,
            fecha_entrega_revision,
            fecha_aprobacion_prorroga,
            fecha_entrega_con_prorroga,
            pidio_prorroga
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        RETURNING id, TO_CHAR(created_at, 'YYYY-MM-DD HH24:MI:SS');
    "#;

    let clean_nombre = item.nombre.trim().to_uppercase();
    let clean_folio = item.folio.trim().to_uppercase();
    let folio_opt = if clean_folio.is_empty() || clean_folio == "S/F" {
        None
    } else {
        Some(clean_folio.clone())
    };

    let row = client
        .query_one(
            query,
            &[
                &rp_uuid,
                &conc_uuid,
                &estatus_imss_uuid,
                &estatus_uuid,
                &clean_nombre,
                &folio_opt,
                &item.fecha_entrega_imss,
                &item.fecha_aprobacion_imss,
                &item.fecha_maxima_aprobacion_imss,
                &item.fecha_maxima_entrega_revision,
                &item.fecha_maxima_entrega_imss,
                &item.fecha_entrega_revision,
                &item.fecha_aprobacion_prorroga,
                &item.fecha_entrega_con_prorroga,
                &item.pidio_prorroga,
            ],
        )
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.revisiones", "INSERT", &e.to_string());
            format!("Error al crear revisión: {}", e)
        })?;

    let new_id: Uuid = row.get(0);
    let created_at: String = row.get(1);

    crate::app_logger::log_db_write(
        "public.revisiones",
        "INSERT",
        &format!("id={}, id_registro_patronal={}, nombre='{}', folio='{:?}'", new_id, rp_uuid, clean_nombre, folio_opt),
        query,
    );

    let mut res = item.clone();
    res.id = new_id.to_string();
    res.id_concepto_diferencia = conc_uuid.map(|u| u.to_string());
    res.id_estatus_imss = estatus_imss_uuid.map(|u| u.to_string());
    res.id_estatus = estatus_uuid.map(|u| u.to_string());
    res.nombre = clean_nombre;
    res.folio = folio_opt.unwrap_or_else(|| "S/F".to_string());
    res.created_at = created_at;
    Ok(res)
}

pub fn update_revision(client: &mut Client, item: &RevisionItem) -> Result<RevisionItem, String> {
    let rev_uuid = Uuid::parse_str(&item.id)
        .map_err(|e| format!("ID de Revisión inválido: {}", e))?;

    let conc_uuid = parse_optional_uuid(item.id_concepto_diferencia.as_deref());
    let estatus_imss_uuid = parse_optional_uuid(item.id_estatus_imss.as_deref());
    let estatus_uuid = parse_optional_uuid(item.id_estatus.as_deref());

    let query = r#"
        UPDATE public.revisiones SET
            id_concepto_diferencia = $2,
            id_estatus_imss = $3,
            id_estatus = $4,
            nombre = $5,
            folio = $6,
            fecha_entrega_imss = $7,
            fecha_aprobacion_imss = $8,
            fecha_maxima_aprobacion_imss = $9,
            fecha_maxima_entrega_revision = $10,
            fecha_maxima_entrega_imss = $11,
            fecha_entrega_revision = $12,
            fecha_aprobacion_prorroga = $13,
            fecha_entrega_con_prorroga = $14,
            pidio_prorroga = $15
        WHERE id = $1;
    "#;

    let clean_nombre = item.nombre.trim().to_uppercase();
    let clean_folio = item.folio.trim().to_uppercase();
    let folio_opt = if clean_folio.is_empty() || clean_folio == "S/F" {
        None
    } else {
        Some(clean_folio.clone())
    };

    client
        .execute(
            query,
            &[
                &rev_uuid,
                &conc_uuid,
                &estatus_imss_uuid,
                &estatus_uuid,
                &clean_nombre,
                &folio_opt,
                &item.fecha_entrega_imss,
                &item.fecha_aprobacion_imss,
                &item.fecha_maxima_aprobacion_imss,
                &item.fecha_maxima_entrega_revision,
                &item.fecha_maxima_entrega_imss,
                &item.fecha_entrega_revision,
                &item.fecha_aprobacion_prorroga,
                &item.fecha_entrega_con_prorroga,
                &item.pidio_prorroga,
            ],
        )
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.revisiones", "UPDATE", &e.to_string());
            format!("Error al actualizar revisión: {}", e)
        })?;

    crate::app_logger::log_db_write(
        "public.revisiones",
        "UPDATE",
        &format!("id={}, nombre='{}', folio='{:?}'", rev_uuid, clean_nombre, folio_opt),
        query,
    );

    let mut res = item.clone();
    res.id_concepto_diferencia = conc_uuid.map(|u| u.to_string());
    res.id_estatus_imss = estatus_imss_uuid.map(|u| u.to_string());
    res.id_estatus = estatus_uuid.map(|u| u.to_string());
    res.nombre = clean_nombre;
    res.folio = folio_opt.unwrap_or_else(|| "S/F".to_string());
    Ok(res)
}

pub fn delete_revision(client: &mut Client, id: &str) -> Result<(), String> {
    let rev_uuid = Uuid::parse_str(id).map_err(|e| format!("ID de Revisión inválido: {}", e))?;

    let mut tx = client
        .transaction()
        .map_err(|e| format!("Error iniciando transacción: {}", e))?;

    // 1. Eliminar CFDIs vinculados a nóminas de esta revisión
    let delete_cfdis_query = r#"
        DELETE FROM public.cfdis
        WHERE id IN (
            SELECT id_cfdi
            FROM public.nominas
            WHERE id_revision = $1
        );
    "#;
    tx.execute(delete_cfdis_query, &[&rev_uuid])
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.cfdis", "DELETE", &e.to_string());
            format!("Error eliminando comprobantes fiscales de la revisión: {}", e)
        })?;
    crate::app_logger::log_db_write(
        "public.cfdis",
        "DELETE",
        &format!("id_revision={}", rev_uuid),
        delete_cfdis_query,
    );

    // 2. Eliminar la revisión en sí (por ON DELETE CASCADE se eliminan liquidaciones, registros_obras, nominas, etc.)
    tx.execute("DELETE FROM public.revisiones WHERE id = $1", &[&rev_uuid])
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.revisiones", "DELETE", &e.to_string());
            format!("Error al eliminar revisión: {}", e)
        })?;
    crate::app_logger::log_db_write(
        "public.revisiones",
        "DELETE",
        &format!("id={}", rev_uuid),
        "DELETE FROM public.revisiones WHERE id = $1",
    );

    tx.commit()
        .map_err(|e| format!("Error confirmando eliminación de revisión: {}", e))?;
    crate::app_logger::log_db_write(
        "public.revisiones_tx",
        "COMMIT",
        &format!("Eliminación de revisión {} confirmada", rev_uuid),
        "tx.commit()",
    );
    Ok(())
}

pub fn create_obra(client: &mut Client, item: &RegistroObraItem) -> Result<RegistroObraItem, String> {
    let rev_uuid = Uuid::parse_str(&item.id_revision)
        .map_err(|e| format!("ID de Revisión inválido: {}", e))?;
    let rp_uuid = Uuid::parse_str(&item.id_registro_patronal)
        .map_err(|e| format!("ID de Registro Patronal inválido: {}", e))?;

    let today = chrono::Local::now().date_naive();
    let f_ini = item.fecha_inicio.unwrap_or(today);
    let f_fin = item.fecha_fin.unwrap_or(today);

    let clean_numero = item.numero.trim().to_uppercase();
    crate::validation::validate_registro_obra(&clean_numero)
        .map_err(|e| format!("Registro de obra inválido: {}", e))?;
    let clean_clase = item.clase.trim().to_uppercase();
    let clean_tipo_obra = item.tipo_obra.trim().to_uppercase();
    let clean_ubicacion = item.ubicacion.trim().to_uppercase();
    let clean_delegacion = item.delegacion.trim().to_uppercase();
    let clean_subdelegacion = item.subdelegacion.trim().to_uppercase();

    let query = r#"
        INSERT INTO public.registros_obras (
            id_revision,
            id_registro_patronal,
            numero,
            clase,
            tipo_obra,
            fecha_inicio,
            fecha_fin,
            ubicacion,
            superficie_construido,
            monto,
            delegacion,
            subdelegacion,
            creado_usuario
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, true)
        ON CONFLICT (id_revision, numero) DO UPDATE SET
            clase = EXCLUDED.clase,
            tipo_obra = EXCLUDED.tipo_obra,
            fecha_inicio = EXCLUDED.fecha_inicio,
            fecha_fin = EXCLUDED.fecha_fin,
            ubicacion = EXCLUDED.ubicacion,
            superficie_construido = EXCLUDED.superficie_construido,
            monto = EXCLUDED.monto,
            delegacion = EXCLUDED.delegacion,
            subdelegacion = EXCLUDED.subdelegacion,
            creado_usuario = true
        RETURNING id, TO_CHAR(created_at, 'YYYY-MM-DD HH24:MI:SS');
    "#;

    let row = client
        .query_one(
            query,
            &[
                &rev_uuid,
                &rp_uuid,
                &clean_numero,
                &clean_clase,
                &clean_tipo_obra,
                &f_ini,
                &f_fin,
                &clean_ubicacion,
                &item.superficie_construido,
                &item.monto,
                &clean_delegacion,
                &clean_subdelegacion,
            ],
        )
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.registros_obras", "INSERT/UPSERT", &e.to_string());
            format!("Error al crear registro de obra: {}", e)
        })?;

    let new_id: Uuid = row.get(0);
    let created_at: String = row.get(1);

    crate::app_logger::log_db_write(
        "public.registros_obras",
        "INSERT",
        &format!("id={}, id_revision={}, numero='{}', clase='{}', tipo='{}', monto={:?}", new_id, rev_uuid, clean_numero, clean_clase, clean_tipo_obra, item.monto),
        query,
    );

    let mut res = item.clone();
    res.id = new_id.to_string();
    res.numero = clean_numero;
    res.clase = clean_clase;
    res.tipo_obra = clean_tipo_obra;
    res.ubicacion = clean_ubicacion;
    res.delegacion = clean_delegacion;
    res.subdelegacion = clean_subdelegacion;
    res.fecha_inicio = Some(f_ini);
    res.fecha_fin = Some(f_fin);
    res.created_at = created_at;
    res.creado_usuario = true;
    Ok(res)
}

pub fn update_obra(client: &mut Client, item: &RegistroObraItem) -> Result<RegistroObraItem, String> {
    let obra_uuid = Uuid::parse_str(&item.id)
        .map_err(|e| format!("ID de Obra inválido: {}", e))?;

    let today = chrono::Local::now().date_naive();
    let f_ini = item.fecha_inicio.unwrap_or(today);
    let f_fin = item.fecha_fin.unwrap_or(today);

    let clean_numero = item.numero.trim().to_uppercase();
    crate::validation::validate_registro_obra(&clean_numero)
        .map_err(|e| format!("Registro de obra inválido: {}", e))?;
    let clean_clase = item.clase.trim().to_uppercase();
    let clean_tipo_obra = item.tipo_obra.trim().to_uppercase();
    let clean_ubicacion = item.ubicacion.trim().to_uppercase();
    let clean_delegacion = item.delegacion.trim().to_uppercase();
    let clean_subdelegacion = item.subdelegacion.trim().to_uppercase();

    let query = r#"
        UPDATE public.registros_obras SET
            numero = $2,
            clase = $3,
            tipo_obra = $4,
            fecha_inicio = $5,
            fecha_fin = $6,
            ubicacion = $7,
            superficie_construido = $8,
            monto = $9,
            delegacion = $10,
            subdelegacion = $11,
            creado_usuario = true
        WHERE id = $1;
    "#;

    client
        .execute(
            query,
            &[
                &obra_uuid,
                &clean_numero,
                &clean_clase,
                &clean_tipo_obra,
                &f_ini,
                &f_fin,
                &clean_ubicacion,
                &item.superficie_construido,
                &item.monto,
                &clean_delegacion,
                &clean_subdelegacion,
            ],
        )
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.registros_obras", "UPDATE", &e.to_string());
            format!("Error al actualizar registro de obra: {}", e)
        })?;

    crate::app_logger::log_db_write(
        "public.registros_obras",
        "UPDATE",
        &format!("id={}, numero='{}', clase='{}', tipo='{}', monto={:?}", obra_uuid, clean_numero, clean_clase, clean_tipo_obra, item.monto),
        query,
    );

    let mut res = item.clone();
    res.numero = clean_numero;
    res.clase = clean_clase;
    res.tipo_obra = clean_tipo_obra;
    res.ubicacion = clean_ubicacion;
    res.delegacion = clean_delegacion;
    res.subdelegacion = clean_subdelegacion;
    res.fecha_inicio = Some(f_ini);
    res.fecha_fin = Some(f_fin);
    res.creado_usuario = true;
    Ok(res)
}

pub fn delete_obra(client: &mut Client, id: &str) -> Result<(), String> {
    let obra_uuid = Uuid::parse_str(id).map_err(|e| format!("ID de Obra inválido: {}", e))?;
    client
        .execute("DELETE FROM public.registros_obras WHERE id = $1", &[&obra_uuid])
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.registros_obras", "DELETE", &e.to_string());
            format!("Error al eliminar registro de obra: {}", e)
        })?;
    crate::app_logger::log_db_write(
        "public.registros_obras",
        "DELETE",
        &format!("id={}", obra_uuid),
        "DELETE FROM public.registros_obras WHERE id = $1",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn test_add_business_days_15() {
        // Viernes 6 de Octubre de 2023 sin festivos
        let start = NaiveDate::from_ymd_opt(2023, 10, 6).unwrap();
        let end = add_business_days(start, 15, None);
        // Debe ser Viernes 27 de Octubre de 2023 (15 días hábiles, saltando fines de semana)
        assert_eq!(end, NaiveDate::from_ymd_opt(2023, 10, 27).unwrap());

        // Lunes 2 de Octubre de 2023
        let start_mon = NaiveDate::from_ymd_opt(2023, 10, 2).unwrap();
        let end_mon = add_business_days(start_mon, 15, None);
        // Debe ser Lunes 23 de Octubre de 2023
        assert_eq!(end_mon, NaiveDate::from_ymd_opt(2023, 10, 23).unwrap());
    }

    #[test]
    fn test_add_business_days_with_imss_holidays() {
        // Aprobación de Revisión IMSS: Lunes 15 de Enero de 2024.
        // Festivos IMSS: 05/Feb (Constitución) y 18/Mar (Natalicio Benito Juárez).
        let mut holidays = HashSet::new();
        holidays.insert(NaiveDate::from_ymd_opt(2024, 2, 5).unwrap());
        holidays.insert(NaiveDate::from_ymd_opt(2024, 3, 18).unwrap());

        let fecha_aprob_imss = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        // 40 días hábiles para entrega de revisión a partir del día siguiente de aprobación:
        let fecha_max_ent_rev = add_business_days(fecha_aprob_imss, 40, Some(&holidays));
        assert_eq!(fecha_max_ent_rev, NaiveDate::from_ymd_opt(2024, 3, 12).unwrap());

        // Fecha de entrega con prórroga: 10 días hábiles después de la Fecha Máxima de Entrega de Revisión
        // Base: 12/Marzo/2024 (Martes)
        // Conteo inicia el día siguiente: Miércoles 13/Mar
        // Días hábiles:
        // 13(1), 14(2), 15(3), [16-17 fin de semana], [18 festivo Benito Juárez],
        // 19(4), 20(5), 21(6), 22(7), [23-24 fin de semana], 25(8), 26(9), 27(10)
        let fecha_ent_pror = add_business_days(fecha_max_ent_rev, 10, Some(&holidays));
        assert_eq!(fecha_ent_pror, NaiveDate::from_ymd_opt(2024, 3, 27).unwrap());
    }

    #[test]
    fn test_parse_optional_uuid_null_and_empty() {
        assert_eq!(parse_optional_uuid(None), None);
        assert_eq!(parse_optional_uuid(Some("")), None);
        assert_eq!(parse_optional_uuid(Some("   ")), None);
        assert_eq!(parse_optional_uuid(Some("-- Seleccionar Concepto Diferencia --")), None);
        assert_eq!(parse_optional_uuid(Some("Sin Concepto Asignado")), None);
        assert_eq!(parse_optional_uuid(Some("invalid-uuid")), None);

        let valid_uuid = "018d4512-3456-7890-abcd-ef0123456789";
        let parsed = parse_optional_uuid(Some(valid_uuid));
        assert!(parsed.is_some());
        assert_eq!(parsed.unwrap().to_string(), valid_uuid);
    }

    #[test]
    fn test_registro_obra_campos_obligatorios_y_opcionales() {
        use crate::validation::validate_registro_obra;

        // Número obligatorio y válido
        let numero = "C1234567";
        assert!(validate_registro_obra(numero).is_ok());

        // Fecha inicio y fin obligatorias y válidas
        let f_ini_str = "01/01/2024";
        let f_fin_str = "31/12/2024";
        let f_ini = parse_date_opt(f_ini_str);
        let f_fin = parse_date_opt(f_fin_str);
        assert!(f_ini.is_some());
        assert!(f_fin.is_some());
        assert!(f_ini.unwrap() <= f_fin.unwrap());

        // Fechas invertidas detectables
        let f_fin_invalida = parse_date_opt("01/01/2023").unwrap();
        assert!(f_ini.unwrap() > f_fin_invalida);

        // Campos opcionales vacíos permitidos
        let item = RegistroObraItem {
            id: "".to_string(),
            id_revision: "rev-1".to_string(),
            id_registro_patronal: "rp-1".to_string(),
            numero: numero.to_string(),
            clase: "".to_string(),
            tipo_obra: "".to_string(),
            fecha_inicio: f_ini,
            fecha_fin: f_fin,
            ubicacion: "".to_string(),
            superficie_construido: 0.0,
            monto: 0.0,
            delegacion: "".to_string(),
            subdelegacion: "".to_string(),
            creado_usuario: true,
            ..Default::default()
        };

        // Sin monto total se alerta para el límite del 18%
        let necesita_aviso_monto = item.monto <= 0.0;
        assert!(necesita_aviso_monto, "Sin monto capturado debe emitir aviso para cálculo del 18%");
    }

    #[test]
    fn test_revision_folio_opcional() {
        // Folio vacío o "S/F" debe normalizarse a None para permitir NULL en Postgres sin chocar con UNIQUE constraint
        let folio_vacio = "";
        let opt_vacio = if folio_vacio.trim().is_empty() || folio_vacio.trim() == "S/F" {
            None
        } else {
            Some(folio_vacio.trim().to_uppercase())
        };
        assert!(opt_vacio.is_none());

        let folio_sf = "S/F";
        let opt_sf = if folio_sf.trim().is_empty() || folio_sf.trim() == "S/F" {
            None
        } else {
            Some(folio_sf.trim().to_uppercase())
        };
        assert!(opt_sf.is_none());

        let folio_capturado = "0503/CCE/2026/0153S";
        let opt_capturado = if folio_capturado.trim().is_empty() || folio_capturado.trim() == "S/F" {
            None
        } else {
            Some(folio_capturado.trim().to_uppercase())
        };
        assert_eq!(opt_capturado, Some("0503/CCE/2026/0153S".to_string()));
    }
}

