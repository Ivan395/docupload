use postgres::Client;
use uuid::Uuid;
use crate::validation::*;

#[derive(Clone, Debug, Default)]
pub struct TipoPersonaItem {
    pub id: String,
    pub tipo: String,
}

#[derive(Clone, Debug, Default)]
pub struct RegimenFiscalItem {
    pub id_clasificacion_cliente: String,
    pub id_regimen_fiscal: String,
    pub clave: String,
    pub nombre: String,
}

#[derive(Clone, Debug, Default)]
pub struct ClaseItem {
    pub id: String,
    pub nombre: String,
    pub descripcion: String,
}

#[derive(Clone, Debug, Default)]
pub struct FraccionItem {
    pub id_clasificacion_rp: String,
    pub id_fraccion: String,
    pub codigo: String,
    pub actividad: String,
}

#[derive(Clone, Debug, Default)]
pub struct CatalogosBundle {
    pub tipos_personas: Vec<TipoPersonaItem>,
    pub clases: Vec<ClaseItem>,
    pub total_clientes: i32,
    pub total_registros_patronales: i32,
    pub total_empleados: i32,
}

#[derive(Clone, Debug, Default)]
pub struct ClienteItem {
    pub id: String,
    pub id_clasificacion_cliente: String,
    pub nombre: String,
    pub razon_social: String,
    pub rfc: String,
    pub curp: String,
    pub tipo_persona: String,
    pub id_tipo_persona: String,
    pub regimen_fiscal_clave: String,
    pub regimen_fiscal_nombre: String,
    pub id_regimen_fiscal: String,
    pub registros_patronales_count: i32,
    pub empleados_count: i32,
    pub has_ingested_data: bool,
}

#[derive(Clone, Debug, Default)]
pub struct RegistroPatronalItem {
    pub id: String,
    pub id_cliente: String,
    pub cliente_nombre: String,
    pub registro: String,
    pub actividad: String,
    pub domicilio: String,
    pub codigo_postal: String,
    pub entidad: String,
    pub area_geografica: String,
    pub delegacion: String,
    pub subdelegacion: String,
    pub prima_riesgo: f64,
    pub id_clase: String,
    pub clase_nombre: String,
    pub id_fraccion: String,
    pub fraccion_codigo: String,
    pub fraccion_actividad: String,
    pub empleados_count: i32,
    pub has_ingested_data: bool,
}

#[derive(Clone, Debug, Default)]
pub struct EmpleadoItem {
    pub id: String,
    pub id_cliente: String,
    pub cliente_nombre: String,
    pub nombre: String,
    pub rfc: String,
    pub curp: String,
    pub nss: String,
    pub registro_patronal: String,
    pub sbc_diario: f64,
    pub pensionado: String,
    pub esta_activo: bool,
}

pub fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

pub fn count_chars(text: &str) -> usize {
    text.chars().count()
}

pub fn limit_to_chars(text: &str, max_chars: usize) -> (usize, String) {
    let char_count = text.chars().count();
    if char_count > max_chars {
        let truncated: String = text.chars().take(max_chars).collect();
        (max_chars, truncated)
    } else {
        (char_count, text.to_string())
    }
}

pub fn limit_to_words(text: &str, max_words: usize) -> (usize, String) {
    let mut words = Vec::new();
    let mut count = 0;
    for w in text.split_whitespace() {
        if count >= max_words {
            break;
        }
        words.push(w);
        count += 1;
    }
    (count, words.join(" "))
}

pub fn fetch_catalogos_bundle(client: &mut Client) -> Result<CatalogosBundle, String> {
    let mut bundle = CatalogosBundle::default();

    // 1. Tipos Personas
    let rows_tp = client
        .query("SELECT id, TRIM(tipo) FROM public.tipos_personas ORDER BY tipo", &[])
        .map_err(|e| format!("Error consultando tipos_personas: {}", e))?;
    for r in rows_tp {
        let id: Uuid = r.get(0);
        let tipo: String = r.get(1);
        bundle.tipos_personas.push(TipoPersonaItem {
            id: id.to_string(),
            tipo,
        });
    }

    // 2. Clases
    let rows_clases = client
        .query("SELECT id, TRIM(nombre), TRIM(descripcion) FROM public.clases ORDER BY nombre", &[])
        .map_err(|e| format!("Error consultando clases: {}", e))?;
    for r in rows_clases {
        let id: Uuid = r.get(0);
        let nombre: String = r.get(1);
        let descripcion: String = r.get(2);
        bundle.clases.push(ClaseItem {
            id: id.to_string(),
            nombre,
            descripcion,
        });
    }

    // 3. Counts
    if let Ok(r) = client.query_one("SELECT COUNT(*)::int FROM public.clientes", &[]) {
        bundle.total_clientes = r.get(0);
    }
    if let Ok(r) = client.query_one("SELECT COUNT(*)::int FROM public.registros_patronales", &[]) {
        bundle.total_registros_patronales = r.get(0);
    }
    if let Ok(r) = client.query_one("SELECT COUNT(*)::int FROM public.empleados WHERE COALESCE(esta_activo, true) = true", &[]) {
        bundle.total_empleados = r.get(0);
    }

    Ok(bundle)
}

pub fn fetch_regimenes_by_tipo_persona(
    client: &mut Client,
    tipo_persona: &str,
) -> Result<Vec<RegimenFiscalItem>, String> {
    let tipo_clean = tipo_persona.trim().to_uppercase();
    if tipo_clean.is_empty() {
        return Ok(Vec::new());
    }

    let rows = client
        .query(
            r#"
            SELECT 
                cc.id,
                rf.id,
                TRIM(rf.clave),
                TRIM(rf.nombre)
            FROM public.clasificaciones_clientes cc
            JOIN public.regimenes_fiscales rf ON cc.id_regimen_fiscal = rf.id
            JOIN public.tipos_personas tp ON cc.id_tipo_persona = tp.id
            WHERE TRIM(tp.tipo) ILIKE $1
            ORDER BY rf.clave
            "#,
            &[&format!("%{}%", tipo_clean)],
        )
        .map_err(|e| format!("Error consultando regímenes fiscales para {}: {}", tipo_clean, e))?;

    let mut list = Vec::new();
    for r in rows {
        let id_cc: Uuid = r.get(0);
        let id_rf: Uuid = r.get(1);
        let clave: String = r.get(2);
        let nombre: String = r.get(3);
        list.push(RegimenFiscalItem {
            id_clasificacion_cliente: id_cc.to_string(),
            id_regimen_fiscal: id_rf.to_string(),
            clave,
            nombre,
        });
    }

    Ok(list)
}

pub fn fetch_fracciones_by_clase(
    client: &mut Client,
    id_clase: &str,
) -> Result<Vec<FraccionItem>, String> {
    let clase_uuid = match Uuid::parse_str(id_clase.trim()) {
        Ok(u) => u,
        Err(_) => return Ok(Vec::new()),
    };

    let rows = client
        .query(
            r#"
            SELECT 
                crp.id,
                f.id,
                TRIM(f.codigo),
                TRIM(f.actividad)
            FROM public.clasificaciones_registros_patronales crp
            JOIN public.fracciones f ON crp.id_fraccion = f.id
            WHERE crp.id_clase = $1
            ORDER BY f.codigo
            "#,
            &[&clase_uuid],
        )
        .map_err(|e| format!("Error consultando fracciones para clase: {}", e))?;

    let mut list = Vec::new();
    for r in rows {
        let id_crp: Uuid = r.get(0);
        let id_f: Uuid = r.get(1);
        let codigo: String = r.get(2);
        let actividad: String = r.get(3);
        list.push(FraccionItem {
            id_clasificacion_rp: id_crp.to_string(),
            id_fraccion: id_f.to_string(),
            codigo,
            actividad,
        });
    }

    Ok(list)
}

pub fn fetch_clientes(client: &mut Client) -> Result<Vec<ClienteItem>, String> {
    let rows = client
        .query(
            r#"
            SELECT 
                c.id,
                COALESCE(c.id_clasificacion_cliente::text, ''),
                c.nombre,
                COALESCE(c.razon_social, ''),
                COALESCE(TRIM(c.rfc), ''),
                COALESCE(TRIM(c.curp), ''),
                COALESCE(TRIM(tp.tipo), ''),
                COALESCE(tp.id::text, ''),
                COALESCE(TRIM(rf.clave), ''),
                COALESCE(TRIM(rf.nombre), ''),
                COALESCE(rf.id::text, ''),
                (SELECT COUNT(*)::int FROM public.registros_patronales rp WHERE rp.id_cliente = c.id) AS rp_count,
                (SELECT COUNT(*)::int FROM public.empleados e WHERE e.id_cliente = c.id) AS emp_count,
                EXISTS (
                    SELECT 1 FROM public.registros_patronales rp 
                    JOIN public.revisiones r ON rp.id = r.id_registro_patronal 
                    WHERE rp.id_cliente = c.id
                ) AS has_ingested_data
            FROM public.clientes c
            LEFT JOIN public.clasificaciones_clientes cc ON c.id_clasificacion_cliente = cc.id
            LEFT JOIN public.tipos_personas tp ON cc.id_tipo_persona = tp.id
            LEFT JOIN public.regimenes_fiscales rf ON cc.id_regimen_fiscal = rf.id
            ORDER BY c.nombre
            "#,
            &[],
        )
        .map_err(|e| format!("Error consultando clientes: {}", e))?;

    let mut list = Vec::new();
    for r in rows {
        let id: Uuid = r.get(0);
        let id_cc: String = r.get(1);
        let nombre: String = r.get(2);
        let razon_social: String = r.get(3);
        let rfc: String = r.get(4);
        let curp: String = r.get(5);
        let tipo_persona: String = r.get(6);
        let id_tp: String = r.get(7);
        let rf_clave: String = r.get(8);
        let rf_nombre: String = r.get(9);
        let id_rf: String = r.get(10);
        let rp_count: i32 = r.get(11);
        let emp_count: i32 = r.get(12);
        let has_data: bool = r.get(13);

        list.push(ClienteItem {
            id: id.to_string(),
            id_clasificacion_cliente: id_cc,
            nombre,
            razon_social,
            rfc,
            curp,
            tipo_persona,
            id_tipo_persona: id_tp,
            regimen_fiscal_clave: rf_clave,
            regimen_fiscal_nombre: rf_nombre,
            id_regimen_fiscal: id_rf,
            registros_patronales_count: rp_count,
            empleados_count: emp_count,
            has_ingested_data: has_data,
        });
    }

    Ok(list)
}

pub fn clean_alphanumeric(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

pub fn clean_uppercase(s: &str) -> String {
    s.trim().to_uppercase()
}

pub fn clean_uppercase_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_uppercase())
    }
}

pub fn create_cliente(
    client: &mut Client,
    nombre: &str,
    rfc: &str,
    razon_social: &str,
    curp: &str,
    id_tipo_persona: &str,
    id_regimen_fiscal: &str,
) -> Result<ClienteItem, String> {
    let clean_nombre = clean_uppercase(nombre);
    if clean_nombre.is_empty() {
        return Err("El nombre del cliente no puede estar vacío.".to_string());
    }

    let clean_rfc = clean_alphanumeric_input(rfc);
    if clean_rfc.is_empty() {
        return Err("El RFC es obligatorio para registrar al cliente (solo alfanumérico).".to_string());
    }

    // Obtener nombre de tipo de persona si se proporcionó id
    let mut tipo_persona_str: Option<String> = None;
    if let Ok(tp_uuid) = Uuid::parse_str(id_tipo_persona.trim()) {
        if let Ok(Some(row)) = client.query_opt("SELECT tipo FROM public.tipos_personas WHERE id = $1", &[&tp_uuid]) {
            tipo_persona_str = Some(row.get::<usize, String>(0).trim().to_string());
        }
    }

    validate_cliente_data(&clean_nombre, &clean_rfc, curp, tipo_persona_str.as_deref())?;

    // Verificar unicidad de nombre
    let exists = client
        .query_opt(
            "SELECT 1 FROM public.clientes WHERE LOWER(TRIM(nombre)) = LOWER(TRIM($1))",
            &[&clean_nombre],
        )
        .map_err(|e| format!("Error verificando existencia del cliente: {}", e))?;

    if exists.is_some() {
        return Err(format!("Ya existe un cliente registrado con el nombre '{}'. No se pueden repetir nombres.", clean_nombre));
    }

    // Verificar unicidad de RFC
    let exists_rfc = client
        .query_opt(
            "SELECT 1 FROM public.clientes WHERE TRIM(rfc) = TRIM($1)",
            &[&clean_rfc],
        )
        .map_err(|e| format!("Error verificando existencia del RFC: {}", e))?;

    if exists_rfc.is_some() {
        return Err(format!("Ya existe un cliente registrado con el RFC '{}'.", clean_rfc));
    }

    // Obtener o crear clasificacion_cliente si se seleccionó tipo de persona y régimen
    let mut id_clasificacion_cliente: Option<Uuid> = None;
    if !id_tipo_persona.trim().is_empty() && !id_regimen_fiscal.trim().is_empty() {
        if let (Ok(tp_uuid), Ok(rf_uuid)) = (Uuid::parse_str(id_tipo_persona.trim()), Uuid::parse_str(id_regimen_fiscal.trim())) {
            let row_cc = client
                .query_opt(
                    "SELECT id FROM public.clasificaciones_clientes WHERE id_tipo_persona = $1 AND id_regimen_fiscal = $2",
                    &[&tp_uuid, &rf_uuid],
                )
                .map_err(|e| format!("Error consultando clasificación cliente: {}", e))?;
            if let Some(r) = row_cc {
                id_clasificacion_cliente = Some(r.get(0));
            } else {
                let new_cc_id = Uuid::now_v7();
                client
                    .execute(
                        "INSERT INTO public.clasificaciones_clientes (id, id_tipo_persona, id_regimen_fiscal) VALUES ($1, $2, $3) ON CONFLICT (id_regimen_fiscal, id_tipo_persona) DO UPDATE SET id_tipo_persona = EXCLUDED.id_tipo_persona",
                        &[&new_cc_id, &tp_uuid, &rf_uuid],
                    )
                    .map_err(|e| format!("Error registrando clasificación cliente: {}", e))?;
                id_clasificacion_cliente = Some(new_cc_id);
            }
        }
    }

    let r_social = clean_uppercase_opt(razon_social);
    let curp_raw = clean_alphanumeric(curp);
    let clean_curp = if curp_raw.is_empty() { None } else { Some(curp_raw) };

    let id = Uuid::now_v7();
    let query_insert_cliente = r#"
            INSERT INTO public.clientes (id, nombre, rfc, razon_social, curp, id_clasificacion_cliente)
            VALUES ($1, $2, $3, $4, $5, $6)
    "#;
    client
        .execute(
            query_insert_cliente,
            &[&id, &clean_nombre, &clean_rfc, &r_social, &clean_curp, &id_clasificacion_cliente],
        )
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.clientes", "INSERT", &e.to_string());
            format!("Error guardando cliente: {}", e)
        })?;

    crate::app_logger::log_db_write(
        "public.clientes",
        "INSERT",
        &format!("id={}, nombre='{}', rfc='{}', razon_social={:?}", id, clean_nombre, clean_rfc, r_social),
        query_insert_cliente,
    );

    Ok(ClienteItem {
        id: id.to_string(),
        id_clasificacion_cliente: id_clasificacion_cliente.map(|u| u.to_string()).unwrap_or_default(),
        nombre: clean_nombre,
        razon_social: r_social.unwrap_or_default(),
        rfc: clean_rfc,
        curp: clean_curp.unwrap_or_default(),
        tipo_persona: "".to_string(),
        id_tipo_persona: id_tipo_persona.to_string(),
        regimen_fiscal_clave: "".to_string(),
        regimen_fiscal_nombre: "".to_string(),
        id_regimen_fiscal: id_regimen_fiscal.to_string(),
        registros_patronales_count: 0,
        empleados_count: 0,
        has_ingested_data: false,
    })
}

pub fn update_cliente(
    client: &mut Client,
    id: &str,
    nombre: &str,
    razon_social: &str,
    rfc: &str,
    curp: &str,
    id_tipo_persona: &str,
    id_regimen_fiscal: &str,
) -> Result<(), String> {
    let client_uuid = Uuid::parse_str(id.trim()).map_err(|_| "ID de cliente inválido".to_string())?;

    let clean_nombre = clean_uppercase(nombre);
    if clean_nombre.is_empty() {
        return Err("El nombre del cliente no puede estar vacío.".to_string());
    }

    let clean_rfc = clean_alphanumeric_input(rfc);
    if clean_rfc.is_empty() {
        return Err("El RFC es obligatorio para el cliente (solo alfanumérico).".to_string());
    }

    // Obtener nombre de tipo de persona si se proporcionó id
    let mut tipo_persona_str: Option<String> = None;
    if let Ok(tp_uuid) = Uuid::parse_str(id_tipo_persona.trim()) {
        if let Ok(Some(row)) = client.query_opt("SELECT tipo FROM public.tipos_personas WHERE id = $1", &[&tp_uuid]) {
            tipo_persona_str = Some(row.get::<usize, String>(0).trim().to_string());
        }
    }

    validate_cliente_data(&clean_nombre, &clean_rfc, curp, tipo_persona_str.as_deref())?;

    // Verificar que otro cliente no tenga el mismo nombre
    let exists = client
        .query_opt(
            "SELECT 1 FROM public.clientes WHERE LOWER(TRIM(nombre)) = LOWER(TRIM($1)) AND id != $2",
            &[&clean_nombre, &client_uuid],
        )
        .map_err(|e| format!("Error verificando existencia del nombre: {}", e))?;
    if exists.is_some() {
        return Err(format!("Ya existe otro cliente registrado con el nombre '{}'.", clean_nombre));
    }

    // Verificar que otro cliente no tenga el mismo RFC
    let exists_rfc = client
        .query_opt(
            "SELECT 1 FROM public.clientes WHERE TRIM(rfc) = TRIM($1) AND id != $2",
            &[&clean_rfc, &client_uuid],
        )
        .map_err(|e| format!("Error verificando existencia del RFC: {}", e))?;
    if exists_rfc.is_some() {
        return Err(format!("Ya existe otro cliente registrado con el RFC '{}'.", clean_rfc));
    }

    // Obtener o crear clasificacion_cliente
    let mut id_clasificacion_cliente: Option<Uuid> = None;
    if !id_tipo_persona.trim().is_empty() && !id_regimen_fiscal.trim().is_empty() {
        if let (Ok(tp_uuid), Ok(rf_uuid)) = (Uuid::parse_str(id_tipo_persona.trim()), Uuid::parse_str(id_regimen_fiscal.trim())) {
            let row_cc = client
                .query_opt(
                    "SELECT id FROM public.clasificaciones_clientes WHERE id_tipo_persona = $1 AND id_regimen_fiscal = $2",
                    &[&tp_uuid, &rf_uuid],
                )
                .map_err(|e| format!("Error consultando clasificación cliente: {}", e))?;
            if let Some(r) = row_cc {
                id_clasificacion_cliente = Some(r.get(0));
            } else {
                let new_cc_id = Uuid::now_v7();
                client
                    .execute(
                        "INSERT INTO public.clasificaciones_clientes (id, id_tipo_persona, id_regimen_fiscal) VALUES ($1, $2, $3) ON CONFLICT (id_regimen_fiscal, id_tipo_persona) DO UPDATE SET id_tipo_persona = EXCLUDED.id_tipo_persona",
                        &[&new_cc_id, &tp_uuid, &rf_uuid],
                    )
                    .map_err(|e| format!("Error registrando clasificación cliente: {}", e))?;
                id_clasificacion_cliente = Some(new_cc_id);
            }
        }
    }

    let r_social = clean_uppercase_opt(razon_social);
    let curp_raw = clean_alphanumeric(curp);
    let clean_curp = if curp_raw.is_empty() { None } else { Some(curp_raw) };

    let query_update_cliente = r#"
            UPDATE public.clientes
            SET nombre = $2,
                razon_social = $3,
                rfc = $4,
                curp = $5,
                id_clasificacion_cliente = COALESCE($6, id_clasificacion_cliente)
            WHERE id = $1
    "#;
    client
        .execute(
            query_update_cliente,
            &[&client_uuid, &clean_nombre, &r_social, &clean_rfc, &clean_curp, &id_clasificacion_cliente],
        )
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.clientes", "UPDATE", &e.to_string());
            format!("Error actualizando cliente: {}", e)
        })?;

    crate::app_logger::log_db_write(
        "public.clientes",
        "UPDATE",
        &format!("id={}, nombre='{}', rfc='{}', razon_social={:?}", client_uuid, clean_nombre, clean_rfc, r_social),
        query_update_cliente,
    );

    Ok(())
}

pub fn fetch_registros_patronales_by_cliente(
    client: &mut Client,
    id_cliente: &str,
) -> Result<Vec<RegistroPatronalItem>, String> {
    let cliente_uuid = Uuid::parse_str(id_cliente.trim())
        .map_err(|_| "ID de cliente inválido".to_string())?;

    let rows = client
        .query(
            r#"
            SELECT 
                rp.id,
                rp.id_cliente,
                c.nombre AS cliente_nombre,
                TRIM(rp.registro) AS registro,
                COALESCE(rp.actividad, ''),
                COALESCE(rp.domicilio, ''),
                COALESCE(rp.codigo_postal, ''),
                COALESCE(rp.entidad, ''),
                COALESCE(rp.area_geografica, ''),
                COALESCE(rp.delegacion, ''),
                COALESCE(rp.subdelegacion, ''),
                COALESCE(rp.prima_riesgo, 0.0),
                COALESCE(cl.id::text, ''),
                COALESCE(TRIM(cl.nombre), ''),
                COALESCE(f.id::text, ''),
                COALESCE(TRIM(f.codigo), ''),
                COALESCE(TRIM(f.actividad), ''),
                (
                    SELECT COUNT(DISTINCT l.id_empleado)::int 
                    FROM public.liquidaciones l 
                    WHERE l.id_registro_patronal = rp.id
                ) AS emp_count,
                EXISTS (
                    SELECT 1 FROM public.revisiones rev WHERE rev.id_registro_patronal = rp.id
                ) AS has_ingested_data
            FROM public.registros_patronales rp
            JOIN public.clientes c ON rp.id_cliente = c.id
            LEFT JOIN public.clasificaciones_registros_patronales crp ON rp.id_clasificacion_regimen_persona = crp.id
            LEFT JOIN public.clases cl ON crp.id_clase = cl.id
            LEFT JOIN public.fracciones f ON crp.id_fraccion = f.id
            WHERE rp.id_cliente = $1
            ORDER BY rp.registro
            "#,
            &[&cliente_uuid],
        )
        .map_err(|e| format!("Error consultando registros patronales: {}", e))?;

    let mut list = Vec::new();
    for r in rows {
        let id: Uuid = r.get(0);
        let id_cli: Uuid = r.get(1);
        let cli_nombre: String = r.get(2);
        let reg: String = r.get(3);
        let act: String = r.get(4);
        let dom: String = r.get(5);
        let cp: String = r.get(6);
        let ent: String = r.get(7);
        let area: String = r.get(8);
        let del: String = r.get(9);
        let subdel: String = r.get(10);
        let prima: f64 = r.get(11);
        let id_clase: String = r.get(12);
        let clase_nom: String = r.get(13);
        let id_fracc: String = r.get(14);
        let fracc_cod: String = r.get(15);
        let fracc_act: String = r.get(16);
        let emp_cnt: i32 = r.get(17);
        let has_data: bool = r.get(18);

        list.push(RegistroPatronalItem {
            id: id.to_string(),
            id_cliente: id_cli.to_string(),
            cliente_nombre: cli_nombre,
            registro: reg,
            actividad: act,
            domicilio: dom,
            codigo_postal: cp,
            entidad: ent,
            area_geografica: area,
            delegacion: del,
            subdelegacion: subdel,
            prima_riesgo: prima,
            id_clase,
            clase_nombre: clase_nom,
            id_fraccion: id_fracc,
            fraccion_codigo: fracc_cod,
            fraccion_actividad: fracc_act,
            empleados_count: emp_cnt,
            has_ingested_data: has_data,
        });
    }

    Ok(list)
}

pub fn create_registro_patronal(
    client: &mut Client,
    id_cliente: &str,
    registro: &str,
    actividad: &str,
    domicilio: &str,
    codigo_postal: &str,
    entidad: &str,
    area_geografica: &str,
    delegacion: &str,
    subdelegacion: &str,
    prima_riesgo: Option<f64>,
    id_clase: Option<&str>,
    id_fraccion: Option<&str>,
) -> Result<RegistroPatronalItem, String> {
    let cliente_uuid = Uuid::parse_str(id_cliente.trim())
        .map_err(|_| "ID de cliente inválido".to_string())?;

    let clean_reg = clean_alphanumeric(registro);
    crate::validation::validate_registro_patronal(&clean_reg)?;

    let (_, raw_act) = limit_to_chars(actividad, 300);
    let clean_act = clean_uppercase(&raw_act);

    // Verify uniqueness for this client
    let exists = client
        .query_opt(
            "SELECT 1 FROM public.registros_patronales WHERE id_cliente = $1 AND TRIM(registro) = $2",
            &[&cliente_uuid, &clean_reg],
        )
        .map_err(|e| format!("Error verificando registro patronal: {}", e))?;

    if exists.is_some() {
        return Err(format!("El registro patronal '{}' ya está registrado para este cliente.", clean_reg));
    }

    // Resolver id_clasificacion_regimen_persona from clase and fraccion if provided
    let mut id_crp: Option<Uuid> = None;
    let clase_uuid_opt = id_clase.and_then(|s| Uuid::parse_str(s.trim()).ok());
    let fraccion_uuid_opt = id_fraccion.and_then(|s| Uuid::parse_str(s.trim()).ok());

    if let Some(c_uuid) = clase_uuid_opt {
        if let Some(f_uuid) = fraccion_uuid_opt {
            let row_crp = client
                .query_opt(
                    "SELECT id FROM public.clasificaciones_registros_patronales WHERE id_clase = $1 AND id_fraccion = $2",
                    &[&c_uuid, &f_uuid],
                )
                .map_err(|e| format!("Error consultando clasificación de riesgo: {}", e))?;
            if let Some(r) = row_crp {
                id_crp = Some(r.get(0));
            }
        }

        if id_crp.is_none() {
            let row_null = client
                .query_opt(
                    "SELECT id FROM public.clasificaciones_registros_patronales WHERE id_clase = $1 AND id_fraccion IS NULL LIMIT 1",
                    &[&c_uuid],
                )
                .map_err(|e| format!("Error consultando clasificación de riesgo por clase: {}", e))?;
            if let Some(r) = row_null {
                id_crp = Some(r.get(0));
            } else {
                let new_crp_id = Uuid::now_v7();
                let inserted = client
                    .query_one(
                        r#"
                        INSERT INTO public.clasificaciones_registros_patronales (id, id_clase, id_fraccion)
                        VALUES ($1, $2, NULL)
                        RETURNING id
                        "#,
                        &[&new_crp_id, &c_uuid],
                    )
                    .map_err(|e| format!("Error registrando clase de riesgo: {}", e))?;
                id_crp = Some(inserted.get(0));
            }
        }
    }

    let clean_dom = clean_uppercase_opt(domicilio);
    let clean_cp = clean_uppercase_opt(codigo_postal);
    let clean_ent = clean_uppercase_opt(entidad);
    let clean_area = clean_uppercase_opt(area_geografica);
    let clean_del = clean_uppercase_opt(delegacion);
    let clean_subdel = clean_uppercase_opt(subdelegacion);

    let id = Uuid::now_v7();
    client
        .execute(
            r#"
            INSERT INTO public.registros_patronales (
                id, id_cliente, registro, actividad, domicilio, codigo_postal,
                entidad, area_geografica, delegacion, subdelegacion, prima_riesgo,
                id_clasificacion_regimen_persona
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
            &[
                &id,
                &cliente_uuid,
                &clean_reg,
                &clean_act,
                &clean_dom,
                &clean_cp,
                &clean_ent,
                &clean_area,
                &clean_del,
                &clean_subdel,
                &prima_riesgo,
                &id_crp,
            ],
        )
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.registros_patronales", "INSERT", &e.to_string());
            format!("Error guardando registro patronal: {}", e)
        })?;

    crate::app_logger::log_db_write(
        "public.registros_patronales",
        "INSERT",
        &format!("id={}, id_cliente={}, registro='{}', actividad='{}', delegacion={:?}", id, cliente_uuid, clean_reg, clean_act, clean_del),
        "INSERT INTO public.registros_patronales ...",
    );

    Ok(RegistroPatronalItem {
        id: id.to_string(),
        id_cliente: id_cliente.to_string(),
        cliente_nombre: "".to_string(),
        registro: clean_reg,
        actividad: clean_act,
        domicilio: clean_dom.unwrap_or_default(),
        codigo_postal: clean_cp.unwrap_or_default(),
        entidad: clean_ent.unwrap_or_default(),
        area_geografica: clean_area.unwrap_or_default(),
        delegacion: clean_del.unwrap_or_default(),
        subdelegacion: clean_subdel.unwrap_or_default(),
        prima_riesgo: prima_riesgo.unwrap_or(0.0),
        id_clase: id_clase.unwrap_or("").to_string(),
        clase_nombre: "".to_string(),
        id_fraccion: id_fraccion.unwrap_or("").to_string(),
        fraccion_codigo: "".to_string(),
        fraccion_actividad: "".to_string(),
        empleados_count: 0,
        has_ingested_data: false,
    })
}

pub fn update_registro_patronal(
    client: &mut Client,
    id: &str,
    registro: &str,
    actividad: &str,
    domicilio: &str,
    codigo_postal: &str,
    entidad: &str,
    area_geografica: &str,
    delegacion: &str,
    subdelegacion: &str,
    prima_riesgo: Option<f64>,
    id_clase: Option<&str>,
    id_fraccion: Option<&str>,
) -> Result<(), String> {
    let rp_uuid = Uuid::parse_str(id.trim())
        .map_err(|_| "ID de registro patronal inválido".to_string())?;

    let clean_reg = clean_alphanumeric(registro);
    crate::validation::validate_registro_patronal(&clean_reg)?;

    // Check if this RP exists and get id_cliente
    let check_row = client
        .query_one(
            "SELECT rp.id_cliente FROM public.registros_patronales rp WHERE rp.id = $1",
            &[&rp_uuid],
        )
        .map_err(|e| format!("Error verificando registro patronal: {}", e))?;

    let cliente_uuid: Uuid = check_row.get(0);

    // Verify duplicate NRP for same client (excluding this record)
    let exists = client
        .query_opt(
            "SELECT id FROM public.registros_patronales WHERE id_cliente = $1 AND TRIM(registro) = $2 AND id != $3",
            &[&cliente_uuid, &clean_reg, &rp_uuid],
        )
        .map_err(|e| format!("Error comprobando duplicados de NRP: {}", e))?;

    if exists.is_some() {
        return Err(format!("El registro patronal '{}' ya está registrado para este cliente.", clean_reg));
    }

    let (_, raw_act) = limit_to_chars(actividad, 300);
    let clean_act = clean_uppercase(&raw_act);

    // Resolve id_clasificacion_regimen_persona from clase and fraccion if provided
    let mut id_crp: Option<Uuid> = None;
    let clase_uuid_opt = id_clase.and_then(|s| Uuid::parse_str(s.trim()).ok());
    let fraccion_uuid_opt = id_fraccion.and_then(|s| Uuid::parse_str(s.trim()).ok());

    if let Some(c_uuid) = clase_uuid_opt {
        if let Some(f_uuid) = fraccion_uuid_opt {
            let row_crp = client
                .query_opt(
                    "SELECT id FROM public.clasificaciones_registros_patronales WHERE id_clase = $1 AND id_fraccion = $2",
                    &[&c_uuid, &f_uuid],
                )
                .map_err(|e| format!("Error consultando clasificación de riesgo: {}", e))?;
            if let Some(r) = row_crp {
                id_crp = Some(r.get(0));
            }
        }

        // If no matching (clase, fraccion) was found or no fraccion was provided, fallback to (clase, NULL)
        if id_crp.is_none() {
            let row_null = client
                .query_opt(
                    "SELECT id FROM public.clasificaciones_registros_patronales WHERE id_clase = $1 AND id_fraccion IS NULL LIMIT 1",
                    &[&c_uuid],
                )
                .map_err(|e| format!("Error consultando clasificación de riesgo por clase: {}", e))?;
            if let Some(r) = row_null {
                id_crp = Some(r.get(0));
            } else {
                let new_crp_id = Uuid::now_v7();
                let inserted = client
                    .query_one(
                        r#"
                        INSERT INTO public.clasificaciones_registros_patronales (id, id_clase, id_fraccion)
                        VALUES ($1, $2, NULL)
                        RETURNING id
                        "#,
                        &[&new_crp_id, &c_uuid],
                    )
                    .map_err(|e| format!("Error registrando clase de riesgo: {}", e))?;
                id_crp = Some(inserted.get(0));
            }
        }
    }

    let clean_dom = clean_uppercase_opt(domicilio);
    let clean_cp = clean_uppercase_opt(codigo_postal);
    let clean_ent = clean_uppercase_opt(entidad);
    let clean_area = clean_uppercase_opt(area_geografica);
    let clean_del = clean_uppercase_opt(delegacion);
    let clean_subdel = clean_uppercase_opt(subdelegacion);

    client
        .execute(
            r#"
            UPDATE public.registros_patronales
            SET registro = $2,
                actividad = $3,
                domicilio = $4,
                codigo_postal = $5,
                entidad = $6,
                area_geografica = $7,
                delegacion = $8,
                subdelegacion = $9,
                prima_riesgo = $10,
                id_clasificacion_regimen_persona = COALESCE($11, id_clasificacion_regimen_persona)
            WHERE id = $1
            "#,
            &[
                &rp_uuid,
                &clean_reg,
                &clean_act,
                &clean_dom,
                &clean_cp,
                &clean_ent,
                &clean_area,
                &clean_del,
                &clean_subdel,
                &prima_riesgo,
                &id_crp,
            ],
        )
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.registros_patronales", "UPDATE", &e.to_string());
            format!("Error actualizando registro patronal: {}", e)
        })?;

    crate::app_logger::log_db_write(
        "public.registros_patronales",
        "UPDATE",
        &format!("id={}, registro='{}', actividad='{}', delegacion={:?}", rp_uuid, clean_reg, clean_act, clean_del),
        "UPDATE public.registros_patronales SET ... WHERE id = $1",
    );

    Ok(())
}

pub fn fetch_empleados_by_cliente(
    client: &mut Client,
    id_cliente: &str,
) -> Result<Vec<EmpleadoItem>, String> {
    let cliente_uuid = Uuid::parse_str(id_cliente.trim())
        .map_err(|_| "ID de cliente inválido".to_string())?;

    let rows = client
        .query(
            r#"
            SELECT 
                e.id,
                e.id_cliente,
                c.nombre AS cliente_nombre,
                COALESCE(e.nombre, ''),
                COALESCE(TRIM(e.rfc), ''),
                COALESCE(TRIM(e.curp), ''),
                COALESCE(TRIM(e.nss), ''),
                COALESCE(
                    (SELECT TRIM(rp.registro) FROM public.nominas n 
                     JOIN public.revisiones rev ON n.id_revision = rev.id 
                     JOIN public.registros_patronales rp ON rev.id_registro_patronal = rp.id 
                     WHERE n.id_empleado = e.id LIMIT 1),
                    (SELECT TRIM(rp.registro) FROM public.liquidaciones l 
                     JOIN public.registros_patronales rp ON l.id_registro_patronal = rp.id 
                     WHERE l.id_empleado = e.id LIMIT 1),
                    ''
                ) AS registro_patronal,
                COALESCE(
                    (SELECT n.salario_diario_integrado FROM public.nominas n WHERE n.id_empleado = e.id ORDER BY n.fecha_pago DESC NULLS LAST LIMIT 1),
                    0.0
                ) AS sbc_diario,
                COALESCE(e.esta_activo, true) AS esta_activo,
                COALESCE(TRIM(e.pensionado), '') AS pensionado
            FROM public.empleados e
            JOIN public.clientes c ON e.id_cliente = c.id
            WHERE e.id_cliente = $1
            ORDER BY e.nombre
            "#,
            &[&cliente_uuid],
        )
        .map_err(|e| format!("Error consultando empleados: {}", e))?;

    let mut list = Vec::new();
    for r in rows {
        let id: Uuid = r.get(0);
        let id_cli: Uuid = r.get(1);
        let cli_nombre: String = r.get(2);
        let nombre: String = r.get(3);
        let rfc: String = r.get(4);
        let curp: String = r.get(5);
        let nss: String = r.get(6);
        let reg: String = r.get(7);
        let sbc: f64 = r.get(8);
        let activo: bool = r.get(9);
        let pensionado: String = r.get(10);

        list.push(EmpleadoItem {
            id: id.to_string(),
            id_cliente: id_cli.to_string(),
            cliente_nombre: cli_nombre,
            nombre,
            rfc,
            curp,
            nss,
            registro_patronal: reg,
            sbc_diario: sbc,
            pensionado,
            esta_activo: activo,
        });
    }

    Ok(list)
}

pub fn update_empleado(
    client: &mut Client,
    id: &str,
    nombre: &str,
    rfc: &str,
    curp: &str,
    nss: &str,
    esta_activo: bool,
) -> Result<(), String> {
    let emp_uuid = Uuid::parse_str(id.trim()).map_err(|_| "ID de empleado inválido".to_string())?;

    let clean_nombre = clean_uppercase(nombre);
    if clean_nombre.is_empty() {
        return Err("El nombre del empleado no puede estar vacío.".to_string());
    }

    let check_row = client
        .query_one(
            "SELECT e.id_cliente, c.rfc FROM public.empleados e JOIN public.clientes c ON e.id_cliente = c.id WHERE e.id = $1",
            &[&emp_uuid],
        )
        .map_err(|e| format!("Error verificando empleado: {}", e))?;
    let cliente_rfc: Option<String> = check_row.get(1);

    validate_empleado_data(&clean_nombre, rfc, curp, nss, cliente_rfc.as_deref())?;

    let clean_rfc = {
        let r = clean_alphanumeric_input(rfc);
        if r.is_empty() { None } else { Some(r) }
    };
    let clean_curp = {
        let c = clean_alphanumeric_input(curp);
        if c.is_empty() { None } else { Some(c) }
    };
    let clean_nss = {
        let n = clean_alphanumeric_input(nss);
        if n.is_empty() { None } else { Some(n) }
    };

    client
        .execute(
            r#"
            UPDATE public.empleados
            SET nombre = $2,
                rfc = $3,
                curp = $4,
                nss = $5,
                esta_activo = $6
            WHERE id = $1
            "#,
            &[&emp_uuid, &clean_nombre, &clean_rfc, &clean_curp, &clean_nss, &esta_activo],
        )
        .map_err(|e| format!("Error actualizando información del empleado: {}", e))?;

    Ok(())
}

pub fn create_empleado(
    client: &mut Client,
    id_cliente: &str,
    nombre: &str,
    rfc: &str,
    curp: &str,
    nss: &str,
    esta_activo: bool,
) -> Result<EmpleadoItem, String> {
    let cliente_uuid = Uuid::parse_str(id_cliente.trim())
        .map_err(|_| "ID de cliente inválido".to_string())?;

    let clean_nombre = clean_uppercase(nombre);
    if clean_nombre.is_empty() {
        return Err("El nombre del empleado no puede estar vacío.".to_string());
    }

    let cliente_rfc: Option<String> = client
        .query_opt("SELECT rfc FROM public.clientes WHERE id = $1", &[&cliente_uuid])
        .ok()
        .flatten()
        .and_then(|r| r.get::<usize, Option<String>>(0));

    validate_empleado_data(&clean_nombre, rfc, curp, nss, cliente_rfc.as_deref())?;

    let clean_rfc = {
        let r = clean_alphanumeric_input(rfc);
        if r.is_empty() { None } else { Some(r) }
    };
    let clean_curp = {
        let c = clean_alphanumeric_input(curp);
        if c.is_empty() { None } else { Some(c) }
    };
    let clean_nss = {
        let n = clean_alphanumeric_input(nss);
        if n.is_empty() { None } else { Some(n) }
    };

    let id = Uuid::now_v7();
    client
        .execute(
            r#"
            INSERT INTO public.empleados (id, id_cliente, nombre, rfc, curp, nss, esta_activo)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
            &[&id, &cliente_uuid, &clean_nombre, &clean_rfc, &clean_curp, &clean_nss, &esta_activo],
        )
        .map_err(|e| format!("Error registrando empleado: {}", e))?;

    Ok(EmpleadoItem {
        id: id.to_string(),
        id_cliente: id_cliente.to_string(),
        cliente_nombre: "".to_string(),
        nombre: clean_nombre,
        rfc: clean_rfc.unwrap_or_default(),
        curp: clean_curp.unwrap_or_default(),
        nss: clean_nss.unwrap_or_default(),
        registro_patronal: "".to_string(),
        sbc_diario: 0.0,
        pensionado: "".to_string(),
        esta_activo,
    })
}

pub fn delete_cliente(client: &mut Client, id: &str) -> Result<(), String> {
    let cliente_uuid = Uuid::parse_str(id.trim())
        .map_err(|e| format!("ID de cliente inválido: {}", e))?;

    let mut tx = client
        .transaction()
        .map_err(|e| format!("Error iniciando transacción: {}", e))?;

    // 1. Eliminar CFDIs asociados a las nóminas de las revisiones de los registros patronales de este cliente
    let delete_cfdis_query = r#"
        DELETE FROM public.cfdis
        WHERE id IN (
            SELECT n.id_cfdi
            FROM public.nominas n
            JOIN public.revisiones r ON r.id = n.id_revision
            JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
            WHERE rp.id_cliente = $1
        );
    "#;
    tx.execute(delete_cfdis_query, &[&cliente_uuid])
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.cfdis", "DELETE", &e.to_string());
            format!("Error eliminando comprobantes fiscales (CFDIs) del cliente: {}", e)
        })?;
    crate::app_logger::log_db_write(
        "public.cfdis",
        "DELETE",
        &format!("id_cliente={}", cliente_uuid),
        delete_cfdis_query,
    );

    // 2. Eliminar el cliente en sí (en cascada elimina registros_patronales, empleados, revisiones, obras, liquidaciones, etc.)
    tx.execute("DELETE FROM public.clientes WHERE id = $1", &[&cliente_uuid])
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.clientes", "DELETE", &e.to_string());
            format!("Error eliminando cliente de la base de datos: {}", e)
        })?;
    crate::app_logger::log_db_write(
        "public.clientes",
        "DELETE",
        &format!("id={}", cliente_uuid),
        "DELETE FROM public.clientes WHERE id = $1",
    );

    tx.commit()
        .map_err(|e| format!("Error confirmando eliminación de cliente: {}", e))?;
    crate::app_logger::log_db_write(
        "public.clientes_tx",
        "COMMIT",
        &format!("Eliminación de cliente {} confirmada", cliente_uuid),
        "tx.commit()",
    );
    Ok(())
}

pub fn delete_registro_patronal(client: &mut Client, id: &str) -> Result<(), String> {
    let rp_uuid = Uuid::parse_str(id.trim())
        .map_err(|e| format!("ID de Registro Patronal inválido: {}", e))?;

    let mut tx = client
        .transaction()
        .map_err(|e| format!("Error iniciando transacción: {}", e))?;

    // 1. Eliminar CFDIs asociados a las nóminas de este registro patronal
    let delete_cfdis_query = r#"
        DELETE FROM public.cfdis
        WHERE id IN (
            SELECT n.id_cfdi
            FROM public.nominas n
            JOIN public.revisiones r ON r.id = n.id_revision
            WHERE r.id_registro_patronal = $1
        );
    "#;
    tx.execute(delete_cfdis_query, &[&rp_uuid])
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.cfdis", "DELETE", &e.to_string());
            format!("Error eliminando comprobantes fiscales (CFDIs) del registro patronal: {}", e)
        })?;
    crate::app_logger::log_db_write(
        "public.cfdis",
        "DELETE",
        &format!("id_registro_patronal={}", rp_uuid),
        delete_cfdis_query,
    );

    // 2. Eliminar el registro patronal en sí (en cascada elimina revisiones, obras, liquidaciones, etc.)
    tx.execute("DELETE FROM public.registros_patronales WHERE id = $1", &[&rp_uuid])
        .map_err(|e| {
            crate::app_logger::log_db_simple_error("public.registros_patronales", "DELETE", &e.to_string());
            format!("Error eliminando registro patronal: {}", e)
        })?;
    crate::app_logger::log_db_write(
        "public.registros_patronales",
        "DELETE",
        &format!("id={}", rp_uuid),
        "DELETE FROM public.registros_patronales WHERE id = $1",
    );

    tx.commit()
        .map_err(|e| format!("Error confirmando eliminación de registro patronal: {}", e))?;
    crate::app_logger::log_db_write(
        "public.registros_patronales_tx",
        "COMMIT",
        &format!("Eliminación de registro patronal {} confirmada", rp_uuid),
        "tx.commit()",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_words_empty_and_whitespace() {
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("   \t  \n  "), 0);
        assert_eq!(count_words("una sola palabra"), 3);
        assert_eq!(count_words("  palabra   con   espacios   múltiples  "), 4);
    }

    #[test]
    fn test_limit_to_words_below_limit() {
        let text = "Construcción y edificación de obras civiles y viales";
        let (cnt, clamped) = limit_to_words(text, 300);
        assert_eq!(cnt, 8);
        assert_eq!(clamped, text);
    }

    #[test]
    fn test_limit_to_words_above_limit_clamping() {
        let many_words = (1..=350).map(|i| format!("palabra{}", i)).collect::<Vec<_>>().join(" ");
        assert_eq!(count_words(&many_words), 350);

        let (cnt, clamped) = limit_to_words(&many_words, 300);
        assert_eq!(cnt, 300);
        assert_eq!(count_words(&clamped), 300);
        assert!(clamped.ends_with("palabra300"));
        assert!(!clamped.contains("palabra301"));
    }

    #[test]
    fn test_count_and_limit_to_chars() {
        assert_eq!(count_chars(""), 0);
        assert_eq!(count_chars("hola"), 4);
        assert_eq!(count_chars("Construcción"), 12);

        let short = "Construcción de edificios";
        let (cnt, clamped) = limit_to_chars(short, 300);
        assert_eq!(cnt, short.chars().count());
        assert_eq!(clamped, short);

        let long = "A".repeat(350);
        let (cnt, clamped) = limit_to_chars(&long, 300);
        assert_eq!(cnt, 300);
        assert_eq!(clamped.chars().count(), 300);
    }

    #[test]
    fn test_clean_alphanumeric_and_uppercase() {
        // RP, RFC, NSS, CURP must be strictly alphanumeric (no '-', ' ', '@', etc.)
        assert_eq!(clean_alphanumeric("A36-77458-103"), "A3677458103");
        assert_eq!(clean_alphanumeric(" rfc-123456-abc "), "RFC123456ABC");
        assert_eq!(clean_alphanumeric("0123-45-6789-0"), "01234567890");
        assert_eq!(clean_alphanumeric("ABCD-901201-XYZ@"), "ABCD901201XYZ");

        // Normal text fields to uppercase
        assert_eq!(clean_uppercase("  obra civil y vial "), "OBRA CIVIL Y VIAL");
        assert_eq!(clean_uppercase_opt(""), None);
        assert_eq!(clean_uppercase_opt("  "), None);
        assert_eq!(clean_uppercase_opt("  ciudad de méxico "), Some("CIUDAD DE MÉXICO".to_string()));
    }
}

