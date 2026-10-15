use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use s3::BucketConfiguration;
use std::path::Path;
use std::fs;
use crate::cedulas::view_logic::{inspect_cedula_file, CedulaFileCardInfo};
use crate::db::client::DbPool;

pub const RUSTFS_ENDPOINT: &str = "http://127.0.0.1:9000";
pub const RUSTFS_ACCESS_KEY: &str = "rustfsadmin";
pub const RUSTFS_SECRET_KEY: &str = "rustfsadmin";
pub const BUCKET_NAME: &str = "correciones";

#[derive(Debug, Clone)]
pub struct RustfsFileInfo {
    pub key: String,
    pub filename: String,
    pub size: u64,
}

pub fn get_rustfs_bucket(bucket_name: &str) -> Result<Box<Bucket>, String> {
    let region = Region::Custom {
        region: "us-east-1".to_string(),
        endpoint: RUSTFS_ENDPOINT.to_string(),
    };
    let credentials = Credentials::new(
        Some(RUSTFS_ACCESS_KEY),
        Some(RUSTFS_SECRET_KEY),
        None,
        None,
        None,
    ).map_err(|e| format!("Error credenciales S3: {}", e))?;

    let mut bucket = Bucket::new(bucket_name, region, credentials)
        .map_err(|e| format!("Error inicializando bucket S3: {}", e))?;
    bucket.set_path_style();
    Ok(bucket)
}

/// Sanitiza un componente de ruta (nombre de cliente, registro patronal o revisión)
pub fn sanitize_path_component(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "GENERAL".to_string();
    }
    let cleaned: String = trimmed
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    let res = cleaned.trim().trim_matches('.').trim();
    if res.is_empty() {
        "GENERAL".to_string()
    } else {
        res.to_string()
    }
}

/// Verifica si el bucket "correciones" existe, y si no, lo crea
pub fn ensure_bucket_exists() -> Result<(), String> {
    let region = Region::Custom {
        region: "us-east-1".to_string(),
        endpoint: RUSTFS_ENDPOINT.to_string(),
    };
    let credentials = Credentials::new(
        Some(RUSTFS_ACCESS_KEY),
        Some(RUSTFS_SECRET_KEY),
        None,
        None,
        None,
    ).map_err(|e| format!("Error credenciales S3: {}", e))?;

    let mut bucket = Bucket::new(BUCKET_NAME, region.clone(), credentials.clone())
        .map_err(|e| format!("Error inicializando bucket S3: {}", e))?;
    bucket.set_path_style();

    match bucket.exists() {
        Ok(true) => Ok(()),
        Ok(false) => {
            log::info!("Bucket '{}' no existe en RustFS. Creándolo...", BUCKET_NAME);
            let create_res = Bucket::create_with_path_style(
                BUCKET_NAME,
                region,
                credentials,
                BucketConfiguration::default(),
            );
            match create_res {
                Ok(_) => {
                    log::info!("Bucket '{}' creado exitosamente en RustFS.", BUCKET_NAME);
                    Ok(())
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if err_str.contains("BucketAlreadyOwnedByYou") || err_str.contains("BucketAlreadyExists") {
                        Ok(())
                    } else {
                        Err(format!("Error creando bucket '{}': {}", BUCKET_NAME, err_str))
                    }
                }
            }
        }
        Err(e) => {
            let err_str = e.to_string();
            // Si el error indica que ya existe o no se pudo chequear exists, intentamos crear
            let create_res = Bucket::create_with_path_style(
                BUCKET_NAME,
                region,
                credentials,
                BucketConfiguration::default(),
            );
            match create_res {
                Ok(_) => Ok(()),
                Err(ce) => {
                    let ce_str = ce.to_string();
                    if ce_str.contains("BucketAlreadyOwnedByYou") || ce_str.contains("BucketAlreadyExists") {
                        Ok(())
                    } else {
                        Err(format!("Error verificando/creando bucket: {} / {}", err_str, ce_str))
                    }
                }
            }
        }
    }
}

/// Asegura la jerarquía de carpetas en RustFS:
/// Correciones -> <Nombre del cliente> -> <Registro Patronal> -> <Revision> -> Cedulas_de_cuotas
/// Correciones -> <Nombre del cliente> -> <Registro Patronal> -> <Revision> -> Nominas
pub fn ensure_revision_structure(cliente: &str, rp: &str, revision: &str) -> Result<(), String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;

    let clean_c = sanitize_path_component(cliente);
    let clean_rp = sanitize_path_component(rp);
    let clean_r = sanitize_path_component(revision);

    let cedulas_keep = format!("{}/{}/{}/Cedulas_de_cuotas/.keep", clean_c, clean_rp, clean_r);
    let nominas_keep = format!("{}/{}/{}/Nominas/.keep", clean_c, clean_rp, clean_r);

    let _ = bucket.put_object(&cedulas_keep, b"");
    let _ = bucket.put_object(&nominas_keep, b"");

    Ok(())
}

/// Sube un archivo a RustFS bajo la estructura indicada
/// Sanitiza una ruta relativa preservando subdirectorios (/), pero sanitizando cada componente
pub fn sanitize_relative_path(input: &str) -> String {
    let normalized = input.replace('\\', "/");
    let parts: Vec<String> = normalized
        .split('/')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .map(|part| sanitize_path_component(part))
        .collect();
    if parts.is_empty() {
        return "documento.pdf".to_string();
    }
    parts.join("/")
}

pub fn upload_file_to_rustfs(
    cliente: &str,
    rp: &str,
    revision: &str,
    subfolder: &str, // "Cedulas_de_cuotas" o "Nominas"
    rel_path: &str,
    data: &[u8],
) -> Result<String, String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;

    let clean_c = sanitize_path_component(cliente);
    let clean_rp = sanitize_path_component(rp);
    let clean_r = sanitize_path_component(revision);
    let clean_rel = sanitize_relative_path(rel_path);

    let object_key = format!("{}/{}/{}/{}/{}", clean_c, clean_rp, clean_r, subfolder, clean_rel);

    bucket
        .put_object(&object_key, data)
        .map_err(|e| format!("Error subiendo archivo a RustFS ({}): {}", object_key, e))?;

    log::info!("Archivo subido a RustFS exitosamente: s3://{}/{}", BUCKET_NAME, object_key);
    Ok(object_key)
}

pub fn is_trash_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    lower.starts_with("trash/") || lower.contains("/trash/")
}

/// Lista los archivos en RustFS bajo la carpeta dada (recursivo, preservando subcarpetas pero omitiendo "Trash")
pub fn list_rustfs_files(
    cliente: &str,
    rp: &str,
    revision: &str,
    subfolder: &str,
) -> Result<Vec<RustfsFileInfo>, String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;

    let clean_c = sanitize_path_component(cliente);
    let clean_rp = sanitize_path_component(rp);
    let clean_r = sanitize_path_component(revision);

    let prefix = format!("{}/{}/{}/{}/", clean_c, clean_rp, clean_r, subfolder);

    let mut results = Vec::new();
    let list_res = bucket
        .list(prefix.clone(), None)
        .map_err(|e| format!("Error listando archivos en RustFS con prefijo {}: {}", prefix, e))?;

    for list_page in list_res {
        for obj in list_page.contents {
            if obj.key.ends_with("/.keep") || obj.key.ends_with('/') {
                continue;
            }
            if is_trash_key(&obj.key) {
                continue;
            }
            let rel = obj.key.strip_prefix(&prefix).unwrap_or(&obj.key);
            if rel.is_empty() || rel == ".keep" {
                continue;
            }

            results.push(RustfsFileInfo {
                key: obj.key.clone(),
                filename: rel.to_string(),
                size: obj.size,
            });
        }
    }

    Ok(results)
}

/// Mueve un archivo dentro de RustFS hacia la subcarpeta "Trash":
/// Correciones -> <Nombre del cliente> -> <Registro Patronal> -> <Revision> -> <Subcarpeta> -> Trash -> <Archivo>
pub fn move_file_to_trash(
    cliente: &str,
    rp: &str,
    revision: &str,
    subfolder: &str,
    filename: &str,
) -> Result<String, String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;

    let clean_c = sanitize_path_component(cliente);
    let clean_rp = sanitize_path_component(rp);
    let clean_r = sanitize_path_component(revision);
    let clean_fn = sanitize_path_component(filename);

    let expected_src_key = format!("{}/{}/{}/{}/{}", clean_c, clean_rp, clean_r, subfolder, clean_fn);
    let expected_dst_key = format!("{}/{}/{}/{}/Trash/{}", clean_c, clean_rp, clean_r, subfolder, clean_fn);

    let mut actual_src_key = expected_src_key.clone();
    let mut actual_dst_key = expected_dst_key.clone();
    let mut file_data: Option<Vec<u8>> = None;

    if let Ok(res) = bucket.get_object(&expected_src_key) {
        if res.status_code() == 200 {
            file_data = Some(res.bytes().to_vec());
        }
    }

    if file_data.is_none() {
        // Fallback: buscar dentro de la subcarpeta de esta revisión específica
        let search_prefix = format!("{}/{}/{}/{}/", clean_c, clean_rp, clean_r, subfolder);
        if let Ok(all_pages) = bucket.list(search_prefix, None) {
            'outer: for page in all_pages {
                for obj in page.contents {
                    let k = &obj.key;
                    if !is_trash_key(k) && (k.ends_with(&clean_fn) || k.ends_with(filename)) {
                        if let Ok(res) = bucket.get_object(k) {
                            if res.status_code() == 200 {
                                actual_src_key = k.clone();
                                let target_needle = format!("/{}/", subfolder);
                                let target_replacement = format!("/{}/Trash/", subfolder);
                                actual_dst_key = k.replacen(&target_needle, &target_replacement, 1);
                                file_data = Some(res.bytes().to_vec());
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }

    let bytes = match file_data {
        Some(b) => b,
        None => {
            return Err(format!(
                "No se encontró el archivo '{}' en RustFS bajo {}/{}/{}/{}",
                filename, clean_c, clean_rp, clean_r, subfolder
            ));
        }
    };

    // Subir a la carpeta Trash
    bucket
        .put_object(&actual_dst_key, &bytes)
        .map_err(|e| format!("Error guardando archivo en Trash de RustFS ({}): {}", actual_dst_key, e))?;

    // Eliminar de la carpeta original
    bucket
        .delete_object(&actual_src_key)
        .map_err(|e| format!("Error eliminando archivo original en RustFS ({}): {}", actual_src_key, e))?;

    log::info!(
        "Archivo movido a Trash en RustFS exitosamente: {} -> {}",
        actual_src_key, actual_dst_key
    );

    Ok(actual_dst_key)
}

/// Mueve todos los archivos de una subcarpeta en RustFS a su respectiva subcarpeta "Trash",
/// conservando su estructura interna y eliminándolos de la ubicación activa.
pub fn move_all_files_to_trash(
    cliente: &str,
    rp: &str,
    revision: &str,
    subfolder: &str,
) -> Result<usize, String> {
    let files = list_rustfs_files(cliente, rp, revision, subfolder)?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;
    let mut moved_count = 0;

    for f in files {
        let actual_src_key = f.key;
        let target_needle = format!("/{}/", subfolder);
        let target_replacement = format!("/{}/Trash/", subfolder);
        let actual_dst_key = actual_src_key.replacen(&target_needle, &target_replacement, 1);

        if let Ok(res) = bucket.get_object(&actual_src_key) {
            if res.status_code() == 200 {
                let bytes = res.bytes();
                if bucket.put_object(&actual_dst_key, bytes).is_ok() {
                    let _ = bucket.delete_object(&actual_src_key);
                    moved_count += 1;
                    log::info!("Archivo movido a Trash en RustFS: {} -> {}", actual_src_key, actual_dst_key);
                }
            }
        }
    }
    Ok(moved_count)
}

/// Mueve todos los archivos de un cliente a la carpeta "Trash" en la raíz del bucket:
/// <Cliente>/... -> Trash/<Cliente>/...
pub fn move_client_to_trash(cliente: &str) -> Result<usize, String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;
    let clean_c = sanitize_path_component(cliente);
    let prefix = format!("{}/", clean_c);

    let list_res = bucket
        .list(prefix.clone(), None)
        .map_err(|e| format!("Error listando archivos para mover cliente a Trash: {}", e))?;

    let mut moved_count = 0;
    for list_page in list_res {
        for obj in list_page.contents {
            if obj.key.to_lowercase().starts_with("trash/") {
                continue;
            }
            let actual_src_key = obj.key;
            let actual_dst_key = format!("Trash/{}", actual_src_key);

            if let Ok(res) = bucket.get_object(&actual_src_key) {
                if res.status_code() == 200 {
                    let bytes = res.bytes();
                    if bucket.put_object(&actual_dst_key, bytes).is_ok() {
                        let _ = bucket.delete_object(&actual_src_key);
                        moved_count += 1;
                        log::info!("Cliente movido a Trash en RustFS: {} -> {}", actual_src_key, actual_dst_key);
                    }
                }
            }
        }
    }
    Ok(moved_count)
}

/// Mueve todos los archivos de un Registro Patronal a la carpeta "Trash" dentro del cliente:
/// <Cliente>/<RP>/... -> <Cliente>/Trash/<RP>/...
pub fn move_rp_to_trash(cliente: &str, rp: &str) -> Result<usize, String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;
    let clean_c = sanitize_path_component(cliente);
    let clean_rp = sanitize_path_component(rp);
    let prefix = format!("{}/{}/", clean_c, clean_rp);

    let list_res = bucket
        .list(prefix.clone(), None)
        .map_err(|e| format!("Error listando archivos para mover RP a Trash: {}", e))?;

    let mut moved_count = 0;
    for list_page in list_res {
        for obj in list_page.contents {
            if is_trash_key(&obj.key) {
                continue;
            }
            let actual_src_key = obj.key;
            let rel = match actual_src_key.strip_prefix(&format!("{}/", clean_c)) {
                Some(r) => r,
                None => continue,
            };
            let actual_dst_key = format!("{}/Trash/{}", clean_c, rel);

            if let Ok(res) = bucket.get_object(&actual_src_key) {
                if res.status_code() == 200 {
                    let bytes = res.bytes();
                    if bucket.put_object(&actual_dst_key, bytes).is_ok() {
                        let _ = bucket.delete_object(&actual_src_key);
                        moved_count += 1;
                        log::info!("RP movido a Trash en RustFS: {} -> {}", actual_src_key, actual_dst_key);
                    }
                }
            }
        }
    }
    Ok(moved_count)
}

/// Mueve todos los archivos de una Revisión a la carpeta "Trash" dentro de su RP:
/// <Cliente>/<RP>/<Revision>/... -> <Cliente>/<RP>/Trash/<Revision>/...
pub fn move_revision_to_trash(cliente: &str, rp: &str, revision: &str) -> Result<usize, String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;
    let clean_c = sanitize_path_component(cliente);
    let clean_rp = sanitize_path_component(rp);
    let clean_r = sanitize_path_component(revision);
    let prefix = format!("{}/{}/{}/", clean_c, clean_rp, clean_r);

    let list_res = bucket
        .list(prefix.clone(), None)
        .map_err(|e| format!("Error listando archivos para mover revisión a Trash: {}", e))?;

    let mut moved_count = 0;
    for list_page in list_res {
        for obj in list_page.contents {
            if is_trash_key(&obj.key) {
                continue;
            }
            let actual_src_key = obj.key;
            let rel = match actual_src_key.strip_prefix(&format!("{}/{}/", clean_c, clean_rp)) {
                Some(r) => r,
                None => continue,
            };
            let actual_dst_key = format!("{}/{}/Trash/{}", clean_c, clean_rp, rel);

            if let Ok(res) = bucket.get_object(&actual_src_key) {
                if res.status_code() == 200 {
                    let bytes = res.bytes();
                    if bucket.put_object(&actual_dst_key, bytes).is_ok() {
                        let _ = bucket.delete_object(&actual_src_key);
                        moved_count += 1;
                        log::info!("Revisión movida a Trash en RustFS: {} -> {}", actual_src_key, actual_dst_key);
                    }
                }
            }
        }
    }
    Ok(moved_count)
}

/// Descarga un archivo de RustFS por su key
pub fn download_rustfs_file(key: &str) -> Result<Vec<u8>, String> {
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;
    let response = bucket
        .get_object(key)
        .map_err(|e| format!("Error descargando archivo de RustFS ({}): {}", key, e))?;
    Ok(response.bytes().to_vec())
}

/// Sincroniza y descarga las cédulas desde RustFS hacia un directorio local de caché,
/// generando los `CedulaFileCardInfo` correspondientes para restaurar la vista de archivos analizados.
pub fn sync_and_load_cedulas_view_files(
    cliente: &str,
    rp: &str,
    revision: &str,
    cache_dir: &Path,
) -> Result<Vec<CedulaFileCardInfo>, String> {
    let files = list_rustfs_files(cliente, rp, revision, "Cedulas_de_cuotas")?;
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let cedulas_cache = cache_dir.join("cedulas_de_cuotas");
    let _ = fs::create_dir_all(&cedulas_cache);

    let mut cards = Vec::new();
    for f in files {
        let local_path = cedulas_cache.join(&f.filename);
        if let Some(parent) = local_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut download_needed = true;
        if local_path.exists() {
            if let Ok(meta) = fs::metadata(&local_path) {
                if meta.len() == f.size {
                    download_needed = false;
                }
            }
        }

        if download_needed {
            match download_rustfs_file(&f.key) {
                Ok(bytes) => {
                    if let Err(e) = fs::write(&local_path, bytes) {
                        log::error!("Error escribiendo caché local de {}: {}", f.filename, e);
                        continue;
                    }
                }
                Err(e) => {
                    log::error!("Error descargando {} desde RustFS: {}", f.key, e);
                    continue;
                }
            }
        }

        if local_path.exists() {
            let mut card = inspect_cedula_file(&local_path);
            card.id = local_path.to_string_lossy().to_string();
            card.filename = f.filename.clone();
            cards.push(card);
        }
    }

    Ok(cards)
}

/// Sincroniza todas las revisiones y registros patronales existentes en la base de datos
/// para asegurar que tengan su jerarquía de carpetas en RustFS
pub fn ensure_all_existing_structures_in_rustfs(pool: &DbPool) {
    let mut client = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("No se pudo conectar a la base de datos para sincronizar estructuras en RustFS: {}", e);
            return;
        }
    };

    if let Err(e) = ensure_bucket_exists() {
        log::warn!("RustFS no disponible para crear bucket: {}", e);
        return;
    }

    // 1. Clientes con registros patronales y revisiones
    let rows = match client.query(
        r#"
        SELECT 
            c.nombre AS cliente_nombre,
            rp.registro AS rp_registro,
            r.nombre AS revision_nombre
        FROM revisiones r
        JOIN registros_patronales rp ON r.id_registro_patronal = rp.id
        JOIN clientes c ON rp.id_cliente = c.id
        "#,
        &[],
    ) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Error consultando revisiones para RustFS: {}", e);
            return;
        }
    };

    for row in rows {
        let c_nombre: String = row.get(0);
        let rp_reg: String = row.get(1);
        let r_nom: String = row.get(2);
        let _ = ensure_revision_structure(&c_nombre, &rp_reg, &r_nom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rustfs_full_flow() {
        let res = ensure_bucket_exists();
        if let Err(e) = res {
            eprintln!("RustFS no disponible para test (skipping): {}", e);
            return;
        }

        // 1. Ensure revision structure
        let struct_res = ensure_revision_structure("TEST_CLIENTE", "A1234567890", "REV_2026_01");
        assert!(struct_res.is_ok(), "ensure_revision_structure failed: {:?}", struct_res.err());

        // 2. Upload sample file
        let sample_data = b"CONTENIDO DE PRUEBA CEDULA SUA";
        let upload_res = upload_file_to_rustfs(
            "TEST_CLIENTE",
            "A1234567890",
            "REV_2026_01",
            "Cedulas_de_cuotas",
            "cedula_ejemplo.pdf",
            sample_data,
        );
        assert!(upload_res.is_ok(), "upload_file_to_rustfs failed: {:?}", upload_res.err());

        // 3. List files
        let list_res = list_rustfs_files(
            "TEST_CLIENTE",
            "A1234567890",
            "REV_2026_01",
            "Cedulas_de_cuotas",
        );
        assert!(list_res.is_ok(), "list_rustfs_files failed: {:?}", list_res.err());
        let files = list_res.unwrap();
        assert!(files.iter().any(|f| f.filename == "cedula_ejemplo.pdf"), "Uploaded file not found in list");

        // 4. Download file
        let key = format!("{}/{}/{}/Cedulas_de_cuotas/cedula_ejemplo.pdf", "TEST_CLIENTE", "A1234567890", "REV_2026_01");
        let dl_res = download_rustfs_file(&key);
        assert!(dl_res.is_ok(), "download_rustfs_file failed: {:?}", dl_res.err());
        assert_eq!(dl_res.unwrap(), sample_data);

        // 5. Move to trash
        let trash_res = move_file_to_trash(
            "TEST_CLIENTE",
            "A1234567890",
            "REV_2026_01",
            "Cedulas_de_cuotas",
            "cedula_ejemplo.pdf",
        );
        assert!(trash_res.is_ok(), "move_file_to_trash failed: {:?}", trash_res.err());
        let trash_key = trash_res.unwrap();
        assert!(trash_key.contains("/Trash/cedula_ejemplo.pdf"));

        // 6. Verify list_rustfs_files does NOT include the file anymore
        let list_after = list_rustfs_files(
            "TEST_CLIENTE",
            "A1234567890",
            "REV_2026_01",
            "Cedulas_de_cuotas",
        ).unwrap();
        assert!(!list_after.iter().any(|f| f.filename == "cedula_ejemplo.pdf"), "Deleted file should NOT be in list_rustfs_files");

        // 7. Cleanup test objects
        let bucket = get_rustfs_bucket(BUCKET_NAME).unwrap();
        let _ = bucket.delete_object(&trash_key);
        let _ = bucket.delete_object(&format!("{}/{}/{}/Cedulas_de_cuotas/.keep", "TEST_CLIENTE", "A1234567890", "REV_2026_01"));
        let _ = bucket.delete_object(&format!("{}/{}/{}/Nominas/.keep", "TEST_CLIENTE", "A1234567890", "REV_2026_01"));
    }

    #[test]
    fn test_upload_nested_structure_and_move_all_to_trash() {
        if let Err(e) = ensure_bucket_exists() {
            eprintln!("RustFS no disponible para test (skipping): {}", e);
            return;
        }

        let cl = "TEST_NESTED";
        let rp = "B9988776655";
        let rev = "REV_ESTRUCTURA_2026";
        let _ = ensure_revision_structure(cl, rp, rev);

        // Subir con subdirectorios
        let f1 = "obras/fase_1/cedula_semana_1.pdf";
        let f2 = "resumen_bimestral.pdf";
        let up1 = upload_file_to_rustfs(cl, rp, rev, "Cedulas_de_cuotas", f1, b"DATA 1");
        let up2 = upload_file_to_rustfs(cl, rp, rev, "Cedulas_de_cuotas", f2, b"DATA 2");
        assert!(up1.is_ok(), "{:?}", up1.err());
        assert!(up2.is_ok(), "{:?}", up2.err());

        // Verificar que list_rustfs_files encuentra ambos con sus rutas relativas
        let list1 = list_rustfs_files(cl, rp, rev, "Cedulas_de_cuotas").unwrap();
        assert_eq!(list1.len(), 2);
        assert!(list1.iter().any(|f| f.filename == f1));
        assert!(list1.iter().any(|f| f.filename == f2));

        // Mover todos a Trash
        let moved = move_all_files_to_trash(cl, rp, rev, "Cedulas_de_cuotas").unwrap();
        assert_eq!(moved, 2);

        // Verificar que list_rustfs_files ahora devuelve 0 porque omite Trash
        let list_after = list_rustfs_files(cl, rp, rev, "Cedulas_de_cuotas").unwrap();
        assert_eq!(list_after.len(), 0);

        // Limpiar objetos de prueba
        let bucket = get_rustfs_bucket(BUCKET_NAME).unwrap();
        let _ = bucket.delete_object(&format!("{}/{}/{}/Cedulas_de_cuotas/Trash/{}", cl, rp, rev, f1));
        let _ = bucket.delete_object(&format!("{}/{}/{}/Cedulas_de_cuotas/Trash/{}", cl, rp, rev, f2));
        let _ = bucket.delete_object(&format!("{}/{}/{}/Cedulas_de_cuotas/.keep", cl, rp, rev));
        let _ = bucket.delete_object(&format!("{}/{}/{}/Nominas/.keep", cl, rp, rev));
    }

    #[test]
    fn test_list_gondesa_files() {
        let files = list_rustfs_files(
            "GONDESA",
            "A3677458103",
            "CONTRATO SIVESA OBRA CIVIL",
            "Cedulas_de_cuotas",
        );
        if let Ok(f_list) = files {
            println!("GONDESA files count: {}", f_list.len());
            for f in &f_list {
                println!(" - {} ({} bytes)", f.filename, f.size);
            }
        }
    }

    #[test]
    fn test_sync_and_load_gondesa_view_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cards = sync_and_load_cedulas_view_files(
            "GONDESA",
            "A3677458103",
            "CONTRATO SIVESA OBRA CIVIL",
            temp_dir.path(),
        );
        assert!(cards.is_ok(), "sync_and_load failed: {:?}", cards.err());
        let list = cards.unwrap();
        println!("GONDESA inspected cards count: {}", list.len());
        for c in &list {
            println!("Card: {} | Type: {} | Pages: {} | Status: {}", c.filename, c.file_type, c.pages_detected, c.status_badge);
        }
        assert!(list.len() >= 9, "Expected at least 9 cards, got {}", list.len());
    }

    #[test]
    fn test_move_revision_rp_client_to_trash() {
        if let Err(e) = ensure_bucket_exists() {
            eprintln!("RustFS no disponible para test (skipping): {}", e);
            return;
        }

        let cl = "TEST_TRASH_SCOPES";
        let rp = "C1122334455";
        let rev = "REV_2026_TEST";

        // 1. Upload files
        let _ = upload_file_to_rustfs(cl, rp, rev, "Nominas", "nomina1.xml", b"<xml>1</xml>");
        let _ = upload_file_to_rustfs(cl, rp, rev, "Cedulas_de_cuotas", "cedula1.pdf", b"pdf");

        // 2. Move revision to trash: <Cliente>/<RP>/Trash/<Revision>/...
        let moved_rev = move_revision_to_trash(cl, rp, rev).unwrap();
        assert!(moved_rev >= 2);
        let list_rev = list_rustfs_files(cl, rp, rev, "Nominas").unwrap();
        assert_eq!(list_rev.len(), 0);

        // Upload another file under RP for testing move_rp_to_trash
        let rev2 = "REV_2026_TEST_2";
        let _ = upload_file_to_rustfs(cl, rp, rev2, "Nominas", "nomina2.xml", b"<xml>2</xml>");
        let moved_rp = move_rp_to_trash(cl, rp).unwrap();
        assert!(moved_rp >= 1);
        let list_rp = list_rustfs_files(cl, rp, rev2, "Nominas").unwrap();
        assert_eq!(list_rp.len(), 0);

        // Upload another file under client for testing move_client_to_trash
        let rp2 = "D9988776655";
        let _ = upload_file_to_rustfs(cl, rp2, "REV_3", "Nominas", "nomina3.xml", b"<xml>3</xml>");
        let moved_client = move_client_to_trash(cl).unwrap();
        assert!(moved_client >= 1);
        let list_client = list_rustfs_files(cl, rp2, "REV_3", "Nominas").unwrap();
        assert_eq!(list_client.len(), 0);

        // Cleanup
        let bucket = get_rustfs_bucket(BUCKET_NAME).unwrap();
        if let Ok(pages) = bucket.list(format!("{}/", cl), None) {
            for page in pages {
                for obj in page.contents {
                    let _ = bucket.delete_object(&obj.key);
                }
            }
        }
        if let Ok(pages) = bucket.list(format!("Trash/{}/", cl), None) {
            for page in pages {
                for obj in page.contents {
                    let _ = bucket.delete_object(&obj.key);
                }
            }
        }
    }
}
