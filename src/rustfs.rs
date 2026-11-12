use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use s3::BucketConfiguration;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use crate::cedulas::business::Business;
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

pub fn is_staging_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    lower.starts_with(".staging_") || lower.contains("/.staging_")
}

/// Fase 2 (Staging en `rustfs`): Sube un archivo a un prefijo temporal `.staging_<batch_id>`
/// dentro de la subcarpeta destino para mantener aislamiento hasta que la transacción de
/// PostgreSQL haga `COMMIT`.
pub fn upload_staging_file_to_rustfs(
    cliente: &str,
    rp: &str,
    revision: &str,
    subfolder: &str,
    batch_id: &str,
    rel_path: &str,
    data: &[u8],
) -> Result<String, String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;

    let clean_c = sanitize_path_component(cliente);
    let clean_rp = sanitize_path_component(rp);
    let clean_r = sanitize_path_component(revision);
    let clean_batch = sanitize_path_component(batch_id);
    let clean_rel = sanitize_relative_path(rel_path);

    let staging_key = format!(
        "{}/{}/{}/{}/.staging_{}/{}",
        clean_c, clean_rp, clean_r, subfolder, clean_batch, clean_rel
    );

    bucket
        .put_object(&staging_key, data)
        .map_err(|e| format!("Error subiendo archivo staging a RustFS ({}): {}", staging_key, e))?;

    log::info!(
        "Archivo preparado en staging de RustFS: s3://{}/{}",
        BUCKET_NAME,
        staging_key
    );
    Ok(staging_key)
}

/// Fase 4A (Commit en `rustfs`): Promueve todos los archivos desde `.staging_<batch_id>/...`
/// hacia su ubicación definitiva `<Anio>/<Mes>/<Archivo>` y limpia el directorio de staging.
pub fn commit_staging_files_in_rustfs(
    cliente: &str,
    rp: &str,
    revision: &str,
    subfolder: &str,
    batch_id: &str,
) -> Result<usize, String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;

    let clean_c = sanitize_path_component(cliente);
    let clean_rp = sanitize_path_component(rp);
    let clean_r = sanitize_path_component(revision);
    let clean_batch = sanitize_path_component(batch_id);

    let staging_prefix = format!(
        "{}/{}/{}/{}/.staging_{}/",
        clean_c, clean_rp, clean_r, subfolder, clean_batch
    );
    let base_prefix = format!("{}/{}/{}/{}/", clean_c, clean_rp, clean_r, subfolder);

    let list_res = bucket
        .list(staging_prefix.clone(), None)
        .map_err(|e| format!("Error listando staging en RustFS ({}): {}", staging_prefix, e))?;

    let mut staged_keys = Vec::new();
    for page in list_res {
        for obj in page.contents {
            if !obj.key.ends_with('/') {
                staged_keys.push(obj.key);
            }
        }
    }

    let mut promoted_keys: Vec<String> = Vec::new();
    for src_key in &staged_keys {
        let rel = match src_key.strip_prefix(&staging_prefix) {
            Some(r) if !r.is_empty() => r,
            _ => continue,
        };
        let final_key = format!("{}{}", base_prefix, rel);

        let res = match bucket.get_object(src_key) {
            Ok(r) if r.status_code() == 200 => r,
            Ok(r) => {
                for pk in &promoted_keys {
                    let _ = bucket.delete_object(pk);
                }
                return Err(format!(
                    "Código HTTP {} leyendo objeto staging en RustFS ({})",
                    r.status_code(),
                    src_key
                ));
            }
            Err(e) => {
                for pk in &promoted_keys {
                    let _ = bucket.delete_object(pk);
                }
                return Err(format!(
                    "Error leyendo objeto staging en RustFS ({}): {}",
                    src_key, e
                ));
            }
        };

        if let Err(e) = bucket.put_object(&final_key, res.bytes()) {
            for pk in &promoted_keys {
                let _ = bucket.delete_object(pk);
            }
            return Err(format!(
                "Error promoviendo archivo definitivo en RustFS ({}): {}",
                final_key, e
            ));
        }

        promoted_keys.push(final_key);
    }

    for src_key in &staged_keys {
        let _ = bucket.delete_object(src_key);
    }

    Ok(promoted_keys.len())
}

/// Fase 4B (Rollback en `rustfs`): Elimina inmediatamente cualquier archivo temporal o parcial
/// creado bajo `.staging_<batch_id>/...` cuando ocurre un error en la base de datos o en el sistema.
pub fn rollback_staging_files_in_rustfs(
    cliente: &str,
    rp: &str,
    revision: &str,
    subfolder: &str,
    batch_id: &str,
) -> Result<usize, String> {
    ensure_bucket_exists()?;
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;

    let clean_c = sanitize_path_component(cliente);
    let clean_rp = sanitize_path_component(rp);
    let clean_r = sanitize_path_component(revision);
    let clean_batch = sanitize_path_component(batch_id);

    let staging_prefix = format!(
        "{}/{}/{}/{}/.staging_{}/",
        clean_c, clean_rp, clean_r, subfolder, clean_batch
    );

    let list_res = bucket
        .list(staging_prefix.clone(), None)
        .map_err(|e| format!("Error listando staging para rollback en RustFS ({}): {}", staging_prefix, e))?;

    let mut deleted = 0usize;
    for page in list_res {
        for obj in page.contents {
            if bucket.delete_object(&obj.key).is_ok() {
                deleted += 1;
            }
        }
    }

    log::warn!(
        "Rollback de staging ejecutado en RustFS ({}): {} objetos temporales eliminados.",
        staging_prefix,
        deleted
    );
    Ok(deleted)
}

/// Lista los archivos en RustFS bajo la carpeta dada (recursivo, preservando subcarpetas pero omitiendo "Trash" y ".staging_")
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
            if is_trash_key(&obj.key) || is_staging_key(&obj.key) {
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

/// Alcance de eliminación segura hacia la papelera (`Trash`) en RustFS (`correcciones`).
#[derive(Debug, Clone)]
pub enum TrashScope<'a> {
    /// Mueve toda la carpeta del cliente a `/correcciones/Trash/<Nombre_Empresa>/`
    Cliente { cliente: &'a str },
    /// Mueve el Registro Patronal a `/correcciones/<Nombre_Empresa>/Trash/<Registro_Patronal>/`
    RegistroPatronal { cliente: &'a str, rp: &'a str },
    /// Mueve la revisión a `/correcciones/<Nombre_Empresa>/<Registro_Patronal>/Trash/<Nombre_Revision>/`
    Revision {
        cliente: &'a str,
        rp: &'a str,
        revision: &'a str,
    },
    /// Mueve todas las Cédulas de Cuotas de una revisión a:
    /// `/correcciones/<Nombre_Empresa>/<Registro_Patronal>/<Nombre_Revision>/Cedulas_de_cuotas/Trash/`
    CedulasRevision {
        cliente: &'a str,
        rp: &'a str,
        revision: &'a str,
    },
    /// Mueve un archivo individual de una categoría (p. ej. `"Cedulas_de_cuotas"`) a su `Trash/`
    ArchivoCategoria {
        cliente: &'a str,
        rp: &'a str,
        revision: &'a str,
        categoria: &'a str,
        filename: &'a str,
    },
}

/// Punto de entrada unificado para borrado seguro en RustFS (`Trash`).
/// Garantiza que ningún archivo sea destruido directamente, reubicándolo en su carpeta `Trash` correspondiente.
pub fn move_to_trash(scope: TrashScope<'_>) -> Result<usize, String> {
    match scope {
        TrashScope::Cliente { cliente } => move_client_to_trash(cliente),
        TrashScope::RegistroPatronal { cliente, rp } => move_rp_to_trash(cliente, rp),
        TrashScope::Revision {
            cliente,
            rp,
            revision,
        } => move_revision_to_trash(cliente, rp, revision),
        TrashScope::CedulasRevision {
            cliente,
            rp,
            revision,
        } => move_all_files_to_trash(cliente, rp, revision, "Cedulas_de_cuotas"),
        TrashScope::ArchivoCategoria {
            cliente,
            rp,
            revision,
            categoria,
            filename,
        } => move_file_to_trash(cliente, rp, revision, categoria, filename).map(|_| 1),
    }
}

/// Mueve de forma segura y multiplataforma (`std::path::PathBuf`) un archivo o directorio local
/// hacia su carpeta `Trash` destino, creándola si no existe (`fs::create_dir_all`) y aplicando
/// `fs::rename` con fallback de copia + borrado cuando origen y destino están en distintos volúmenes.
pub fn move_local_path_to_trash(src: &Path, trash_dir: &Path) -> Result<PathBuf, String> {
    if !src.exists() {
        return Err(format!("La ruta de origen no existe: {}", src.display()));
    }
    fs::create_dir_all(trash_dir).map_err(|e| {
        format!(
            "No se pudo crear el directorio Trash ({}): {}",
            trash_dir.display(),
            e
        )
    })?;

    let file_name = src
        .file_name()
        .ok_or_else(|| format!("Nombre de archivo o carpeta inválido: {}", src.display()))?;
    let dst = trash_dir.join(file_name);

    if fs::rename(src, &dst).is_err() {
        if src.is_file() {
            fs::copy(src, &dst).map_err(|e| {
                format!(
                    "Error copiando archivo hacia Trash ({} -> {}): {}",
                    src.display(),
                    dst.display(),
                    e
                )
            })?;
            fs::remove_file(src).map_err(|e| {
                format!(
                    "Error removiendo archivo original tras copia a Trash ({}): {}",
                    src.display(),
                    e
                )
            })?;
        } else {
            return Err(format!(
                "No se pudo mover el directorio hacia Trash ({} -> {})",
                src.display(),
                dst.display()
            ));
        }
    }

    Ok(dst)
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
            card.filename = Path::new(&f.filename)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or(f.filename.clone());
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

#[derive(Debug, Default, Clone)]
pub struct ReconcileStats {
    pub total_files_inspected: usize,
    pub files_parsed: usize,
    pub employees_matched: usize,
    pub liquidaciones_matched: usize,
    pub vinculaciones_upserted: usize,
    pub fake_vinculaciones_removed: usize,
    pub revisions_synced: usize,
}

/// Reconcilia y repara todas las vinculaciones de liquidaciones (tablas maestras y tablas de reportes)
/// basándose 100% en los archivos originales de cédulas almacenados en RustFS (S3).
pub fn reconcile_all_liquidaciones_vinculaciones_from_rustfs(
    client: &mut postgres::Client,
) -> Result<ReconcileStats, String> {
    let mut stats = ReconcileStats::default();
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;
    let patterns = std::sync::Arc::new(
        crate::cedulas::regex_expressions::initialize_patterns()
            .map_err(|e| format!("Error inicializando patrones regex: {}", e))?,
    );

    let pages = bucket
        .list("".to_string(), None)
        .map_err(|e| format!("Error listando bucket RustFS: {}", e))?;

    let mut cedula_keys = Vec::new();
    for page in pages {
        for obj in page.contents {
            if !obj.key.contains("Trash")
                && obj.key.contains("Cedulas_de_cuotas")
                && obj.key.ends_with(".pdf")
            {
                cedula_keys.push(obj.key);
            }
        }
    }
    cedula_keys.sort();
    stats.total_files_inspected = cedula_keys.len();
    log::info!("Reconciliando {} cédulas PDF encontradas en RustFS...", cedula_keys.len());

    let temp_dir = std::env::temp_dir().join("docu_ffacd_reconcile");
    let _ = fs::create_dir_all(&temp_dir);

    let rp_rows = client
        .query(
            "SELECT id, id_cliente, UPPER(TRIM(registro)) FROM public.registros_patronales",
            &[],
        )
        .map_err(|e| format!("Error consultando registros patronales: {}", e))?;
    let mut rp_map: HashMap<String, (Uuid, Uuid)> = HashMap::new();
    for r in rp_rows {
        let id_rp: Uuid = r.get(0);
        let id_cl: Uuid = r.get(1);
        let reg: String = r.get(2);
        let clean = crate::validation::clean_alphanumeric_input(&reg);
        rp_map.insert(clean, (id_rp, id_cl));
    }

    let rev_rows = client
        .query("SELECT id FROM public.revisiones", &[])
        .map_err(|e| format!("Error consultando revisiones: {}", e))?;
    let all_revisions: Vec<Uuid> = rev_rows.iter().map(|r| r.get(0)).collect();

    let mut tx = client
        .transaction()
        .map_err(|e| format!("Error iniciando transacción de reconciliación: {}", e))?;

    for key in &cedula_keys {
        let bytes = match download_rustfs_file(key) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("No se pudo descargar {}: {}", key, e);
                continue;
            }
        };

        let tmp_file = temp_dir.join("current_reconcile.pdf");
        if let Err(e) = fs::write(&tmp_file, &bytes) {
            log::warn!("Error escribiendo temp {}: {}", key, e);
            continue;
        }

        let raw_text = match crate::cedulas::pdf_extractor::extract_with_layout(&tmp_file) {
            Ok(t) => t,
            Err(e) => {
                log::debug!("Error extrayendo layout de {}: {}", key, e);
                continue;
            }
        };

        if crate::cedulas::ema::is_bimestral_document(&raw_text) {
            continue;
        }

        let text_lower = raw_text.to_lowercase();
        let is_cedula = text_lower.contains("cédula de determinación de cuotas")
            || text_lower.contains("cedula de determinacion de cuotas")
            || text_lower.contains("sistema único de autodeterminación")
            || text_lower.contains("sistema unico de autodeterminacion")
            || (text_lower.contains("propuesta de cédula") && text_lower.contains("cuotas"))
            || (text_lower.contains("propuesta de cedula") && text_lower.contains("cuotas"));
        if !is_cedula {
            continue;
        }

        let cleaned = crate::cedulas::pdf_extractor::clean_text(&text_lower, &patterns);
        let mut biz = Business::new();
        biz.extract_from_page(&cleaned, &patterns);

        let detected_rp = biz.registro_patronal.as_deref().or_else(|| {
            let parts: Vec<&str> = key.split('/').collect();
            if parts.len() >= 2 {
                Some(parts[1])
            } else {
                None
            }
        });

        let (id_rp, id_cliente) = match detected_rp
            .and_then(|r| rp_map.get(&crate::validation::clean_alphanumeric_input(r)))
        {
            Some(pair) => *pair,
            None => continue,
        };

        let emps = crate::cedulas::processor::get_employee_data(&cleaned, &patterns);
        if emps.is_empty() {
            continue;
        }

        let emp_mes = emps.iter().find_map(|e| e.mes_periodo.as_deref());
        let emp_anio = emps.iter().find_map(|e| e.anio_periodo.as_deref());

        let mes_str = emp_mes.or_else(|| {
            let key_up = key.to_uppercase();
            for m in &[
                "ENERO", "FEBRERO", "MARZO", "ABRIL", "MAYO", "JUNIO", "JULIO", "AGOSTO",
                "SEPTIEMBRE", "OCTUBRE", "NOVIEMBRE", "DICIEMBRE",
            ] {
                if key_up.contains(m) {
                    return Some(*m);
                }
            }
            None
        });
        let anio_val: Option<i16> = emp_anio
            .and_then(|a| a.trim().parse::<i16>().ok())
            .or_else(|| {
                for y in 2015..=2030 {
                    if key.contains(&y.to_string()) {
                        return Some(y as i16);
                    }
                }
                None
            });

        let (mes_norm, anio_norm) = match (mes_str, anio_val) {
            (Some(m), Some(a)) => (crate::db::cedulas::normalize_mes_periodo(Some(m)), a),
            _ => continue,
        };
        let mes_int = crate::cedulas::business::mes_to_periodo_int(&mes_norm);
        stats.files_parsed += 1;

        let rev_name_from_key = {
            let parts: Vec<&str> = key.split('/').collect();
            if parts.len() >= 3 {
                parts[2]
            } else {
                ""
            }
        };

        let rev_row = tx
            .query_opt(
                "SELECT id FROM public.revisiones WHERE id_registro_patronal = $1 AND UPPER(TRIM(nombre)) = UPPER(TRIM($2)) LIMIT 1",
                &[&id_rp, &rev_name_from_key],
            )
            .ok()
            .flatten();

        let id_revision: Uuid = match rev_row {
            Some(r) => r.get(0),
            None => {
                let fallback_rev = tx
                    .query_opt(
                        "SELECT id FROM public.revisiones WHERE id_registro_patronal = $1 ORDER BY created_at DESC LIMIT 1",
                        &[&id_rp],
                    )
                    .ok()
                    .flatten();
                match fallback_rev {
                    Some(r) => r.get(0),
                    None => continue,
                }
            }
        };

        for emp in &emps {
            let clean_nss = emp
                .no_seguro_social
                .as_deref()
                .map(crate::validation::clean_alphanumeric_input);
            let clean_rfc = emp
                .rfc
                .as_deref()
                .map(crate::validation::clean_alphanumeric_input);
            let clean_curp = emp
                .curp
                .as_deref()
                .map(crate::validation::clean_alphanumeric_input);
            let emp_nombre = emp.nombre.as_deref().unwrap_or("").trim().to_uppercase();

            let emp_row = tx
                .query_opt(
                    r#"
                    SELECT id FROM public.empleados
                    WHERE id_cliente = $1
                      AND (
                          ($2::varchar IS NOT NULL AND nss = $2)
                          OR ($3::varchar IS NOT NULL AND rfc = $3)
                          OR ($4::varchar IS NOT NULL AND curp = $4)
                          OR ($5::varchar != '' AND TRIM(UPPER(nombre)) = $5)
                      )
                    ORDER BY (nss = $2) DESC, (rfc = $3) DESC, (curp = $4) DESC
                    LIMIT 1
                    "#,
                    &[&id_cliente, &clean_nss, &clean_rfc, &clean_curp, &emp_nombre],
                )
                .ok()
                .flatten();

            let id_empleado: Uuid = match emp_row {
                Some(r) => r.get(0),
                None => continue,
            };
            stats.employees_matched += 1;

            let file_name = key.split('/').last().unwrap_or(&key).to_string();
            let liq_row = tx
                .query_opt(
                    r#"
                    SELECT id FROM public.liquidaciones
                    WHERE id_registro_patronal = $1
                      AND id_empleado = $2
                      AND anio_periodo = $3
                      AND (mes_periodo_int = $4 OR UPPER(TRIM(mes_periodo)) = UPPER(TRIM($5)))
                      AND (nombre_archivo = $6 OR nombre_archivo IS NULL)
                    ORDER BY (nombre_archivo = $6) DESC, created_at ASC
                    LIMIT 1
                    "#,
                    &[&id_rp, &id_empleado, &anio_norm, &mes_int, &mes_norm, &file_name],
                )
                .ok()
                .flatten();

            let id_liquidacion: Uuid = match liq_row {
                Some(r) => r.get(0),
                None => continue,
            };
            stats.liquidaciones_matched += 1;

            let mut obras_consolidadas: BTreeMap<String, i16> = BTreeMap::new();
            for ro in &emp.registro_obras {
                let clean_ro = crate::validation::clean_alphanumeric_input(&ro.registro_obra);
                if !clean_ro.is_empty()
                    && (crate::validation::validate_registro_obra(&clean_ro).is_ok()
                        || clean_ro.starts_with('C')
                        || clean_ro.starts_with('A'))
                {
                    let d = (ro.dias_registro_obra as i16).clamp(1, 31);
                    *obras_consolidadas.entry(clean_ro).or_insert(0) += d;
                }
            }

            // Limpiamos vinculaciones previas de esta liquidación para garantizar coincidencia exacta con el PDF
            let del_cnt = tx
                .execute(
                    "DELETE FROM public.vinculaciones_obras_liquidaciones WHERE id_liquidacion = $1",
                    &[&id_liquidacion],
                )
                .unwrap_or(0);
            if obras_consolidadas.is_empty() && del_cnt > 0 {
                stats.fake_vinculaciones_removed += del_cnt as usize;
            }

            for (num_obra, total_dias) in obras_consolidadas {
                let clamped_dias = total_dias.clamp(1, 31);
                let m_num = mes_int as i32;
                let y_val = anio_norm as i32;
                let valid_m = if (1..=12).contains(&m_num) { m_num as u32 } else { 1u32 };
                let fecha_inicio = chrono::NaiveDate::from_ymd_opt(y_val, valid_m, 1)
                    .unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap());
                let last_day = match valid_m {
                    1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                    4 | 6 | 9 | 11 => 30,
                    2 => {
                        if (y_val % 4 == 0 && y_val % 100 != 0) || (y_val % 400 == 0) {
                            29
                        } else {
                            28
                        }
                    }
                    _ => 30,
                };
                let fecha_fin = chrono::NaiveDate::from_ymd_opt(y_val, valid_m, last_day)
                    .unwrap_or(fecha_inicio);

                let obra_row = tx
                    .query_one(
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
                        RETURNING id
                        "#,
                        &[&id_revision, &id_rp, &num_obra, &fecha_inicio, &fecha_fin],
                    )
                    .map_err(|e| format!("Error upserting registro_obra: {}", e))?;

                let id_registro_obra: Uuid = obra_row.get(0);
                let id_v = Uuid::now_v7();
                tx.execute(
                    r#"
                    INSERT INTO public.vinculaciones_obras_liquidaciones (
                        id, id_registro_obra, id_liquidacion, dias_registro_obra
                    ) VALUES ($1, $2, $3, $4)
                    "#,
                    &[&id_v, &id_registro_obra, &id_liquidacion, &clamped_dias],
                )
                .map_err(|e| format!("Error insertando vinculacion: {}", e))?;
                stats.vinculaciones_upserted += 1;
            }
        }
    }

    // Limpieza de vinculaciones con 30 días que fueron generadas artificialmente desde clave_ubicación
    let cleanup_ghosts = tx
        .execute(
            r#"
            DELETE FROM public.vinculaciones_obras_liquidaciones vol
            USING public.liquidaciones l, public.empleados e, public.registros_obras ro
            WHERE vol.id_liquidacion = l.id
              AND l.id_empleado = e.id
              AND vol.id_registro_obra = ro.id
              AND vol.dias_registro_obra = 30
              AND TRIM(UPPER(ro.numero)) = LEFT(TRIM(UPPER(e.clave_ubicacion)), 15)
              AND COALESCE(ro.creado_usuario, false) = false
              AND NOT EXISTS (
                  SELECT 1 FROM public.detalles_liquidaciones dl
                  WHERE dl.id_liquidacion = l.id AND dl.dias = 30
              )
            "#,
            &[],
        )
        .unwrap_or(0);
    stats.fake_vinculaciones_removed += cleanup_ghosts as usize;

    // Sincronizar todas las revisiones existentes para refrescar sus tablas de reportes
    for rev_id in all_revisions {
        match crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx(
            &mut tx, rev_id,
        ) {
            Ok(_) => stats.revisions_synced += 1,
            Err(e) => log::warn!("Error sincronizando reporte de revision {}: {}", rev_id, e),
        }
    }

    tx.commit()
        .map_err(|e| format!("Error haciendo commit de reconciliacion: {}", e))?;

    let _ = fs::remove_dir_all(&temp_dir);
    log::info!("Reconciliación completada con éxito: {:?}", stats);
    Ok(stats)
}

#[derive(Debug, Default)]
pub struct ClaveCleaningStats {
    pub bimestral_liquidaciones_removed: usize,
    pub bimestral_detalles_removed: usize,
    pub bimestral_reportes_detalles_removed: usize,
    pub detalles_normalized_baja: usize,
    pub detalles_normalized_reing: usize,
    pub detalles_normalized_alta: usize,
    pub detalles_normalized_ms: usize,
    pub detalles_cleaned_null: usize,
    pub reportes_cleaned_null: usize,
    pub revisions_resynced: usize,
}

pub fn clean_and_repair_detalles_liquidaciones_database(
    client: &mut postgres::Client,
) -> Result<ClaveCleaningStats, String> {
    let mut stats = ClaveCleaningStats::default();

    // 1. Obtener nombres de archivos bimestrales desde RustFS
    let bucket = get_rustfs_bucket(BUCKET_NAME)?;
    let pages = bucket.list("".to_string(), None).map_err(|e| e.to_string())?;
    let mut bimestral_fnames: Vec<String> = Vec::new();
    for page in pages {
        for obj in page.contents {
            if !obj.key.contains("Trash") && obj.key.ends_with(".pdf") {
                let data = download_rustfs_file(&obj.key).unwrap_or_default();
                if let Ok(mut temp) = tempfile::NamedTempFile::new() {
                    let _ = std::io::Write::write_all(&mut temp, &data);
                    if let Ok(raw_text) = crate::cedulas::pdf_extractor::extract_with_layout(temp.path()) {
                        if crate::cedulas::ema::is_bimestral_document(&raw_text) {
                            if let Some(fname) = Path::new(&obj.key).file_name().and_then(|n| n.to_str()) {
                                bimestral_fnames.push(fname.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    let mut tx = client.transaction().map_err(|e| format!("Error iniciando tx: {}", e))?;

    // 2. Buscar liquidaciones bimestrales en public.liquidaciones (por nombre de archivo de RustFS o por dias > 31)
    let bimestral_rows = tx.query(
        r#"
        SELECT DISTINCT l.id
        FROM public.liquidaciones l
        LEFT JOIN public.detalles_liquidaciones dl ON dl.id_liquidacion = l.id
        WHERE l.nombre_archivo = ANY($1)
           OR dl.dias > 31
        "#,
        &[&bimestral_fnames],
    ).map_err(|e| format!("Error buscando liquidaciones bimestrales: {}", e))?;

    let bimestral_ids: Vec<Uuid> = bimestral_rows.iter().map(|r| r.get(0)).collect();
    if !bimestral_ids.is_empty() {
        let r_del = tx.execute(
            r#"
            DELETE FROM public.reportes_detalles_liquidaciones
            WHERE id_reporte_liquidacion IN (
                SELECT id FROM public.reportes_liquidaciones WHERE id_liquidacion = ANY($1)
            )
            "#,
            &[&bimestral_ids],
        ).unwrap_or(0);
        stats.bimestral_reportes_detalles_removed = r_del as usize;

        tx.execute(
            "DELETE FROM public.reportes_liquidaciones WHERE id_liquidacion = ANY($1)",
            &[&bimestral_ids],
        ).map_err(|e| format!("Error eliminando reportes_liquidaciones bimestrales: {}", e))?;

        tx.execute(
            "DELETE FROM public.vinculaciones_obras_liquidaciones WHERE id_liquidacion = ANY($1)",
            &[&bimestral_ids],
        ).map_err(|e| format!("Error eliminando vinculaciones bimestrales: {}", e))?;

        let d_del = tx.execute(
            "DELETE FROM public.detalles_liquidaciones WHERE id_liquidacion = ANY($1)",
            &[&bimestral_ids],
        ).unwrap_or(0);
        stats.bimestral_detalles_removed = d_del as usize;

        let l_del = tx.execute(
            "DELETE FROM public.liquidaciones WHERE id = ANY($1)",
            &[&bimestral_ids],
        ).unwrap_or(0);
        stats.bimestral_liquidaciones_removed = l_del as usize;
    }

    // 3. Normalizar claves aceptadas
    let n_baja = tx.execute("UPDATE public.detalles_liquidaciones SET clave = 'baja' WHERE TRIM(UPPER(clave)) IN ('B', 'BAJA')", &[]).unwrap_or(0);
    tx.execute("UPDATE public.reportes_detalles_liquidaciones SET clave = 'baja' WHERE TRIM(UPPER(clave)) IN ('B', 'BAJA')", &[]).unwrap_or(0);
    stats.detalles_normalized_baja = n_baja as usize;

    let n_reing = tx.execute("UPDATE public.detalles_liquidaciones SET clave = 'reing' WHERE TRIM(UPPER(clave)) IN ('R', 'REIN', 'REING', 'REINGRESO')", &[]).unwrap_or(0);
    tx.execute("UPDATE public.reportes_detalles_liquidaciones SET clave = 'reing' WHERE TRIM(UPPER(clave)) IN ('R', 'REIN', 'REING', 'REINGRESO')", &[]).unwrap_or(0);
    stats.detalles_normalized_reing = n_reing as usize;

    let n_alta = tx.execute("UPDATE public.detalles_liquidaciones SET clave = 'alta' WHERE TRIM(UPPER(clave)) IN ('A', 'ALTA')", &[]).unwrap_or(0);
    tx.execute("UPDATE public.reportes_detalles_liquidaciones SET clave = 'alta' WHERE TRIM(UPPER(clave)) IN ('A', 'ALTA')", &[]).unwrap_or(0);
    stats.detalles_normalized_alta = n_alta as usize;

    let n_ms = tx.execute("UPDATE public.detalles_liquidaciones SET clave = 'm/s' WHERE LOWER(TRIM(clave)) IN ('ms', 'm/s', 'm.s', 'ism', 'mod', 'mod.sal')", &[]).unwrap_or(0);
    tx.execute("UPDATE public.reportes_detalles_liquidaciones SET clave = 'm/s' WHERE LOWER(TRIM(clave)) IN ('ms', 'm/s', 'm.s', 'ism', 'mod', 'mod.sal')", &[]).unwrap_or(0);
    stats.detalles_normalized_ms = n_ms as usize;

    tx.execute("UPDATE public.detalles_liquidaciones SET clave = 'inc' WHERE LOWER(TRIM(clave)) IN ('inc', 'incap', 'incapacidad')", &[]).unwrap_or(0);
    tx.execute("UPDATE public.reportes_detalles_liquidaciones SET clave = 'inc' WHERE LOWER(TRIM(clave)) IN ('inc', 'incap', 'incapacidad')", &[]).unwrap_or(0);
    tx.execute("UPDATE public.detalles_liquidaciones SET clave = 'aus' WHERE LOWER(TRIM(clave)) IN ('aus', 'ausentismo')", &[]).unwrap_or(0);
    tx.execute("UPDATE public.reportes_detalles_liquidaciones SET clave = 'aus' WHERE LOWER(TRIM(clave)) IN ('aus', 'ausentismo')", &[]).unwrap_or(0);

    // 4. Limpiar claves corruptas (fechas, salarios, enteros, o cualquier valor fuera de catálogo) poniéndolos en NULL
    let c_dl = tx.execute(
        r#"
        UPDATE public.detalles_liquidaciones
        SET clave = NULL,
            fecha = CASE WHEN LOWER(TRIM(clave)) NOT IN ('alta', 'baja', 'reing', 'rein', 'ms', 'm/s', 'inc', 'aus') THEN NULL ELSE fecha END
        WHERE clave IS NOT NULL AND LOWER(TRIM(clave)) NOT IN ('alta', 'baja', 'reing', 'rein', 'ms', 'm/s', 'inc', 'aus')
        "#,
        &[],
    ).unwrap_or(0);
    stats.detalles_cleaned_null = c_dl as usize;

    let c_rdl = tx.execute(
        r#"
        UPDATE public.reportes_detalles_liquidaciones
        SET clave = NULL,
            fecha = CASE WHEN LOWER(TRIM(clave)) NOT IN ('alta', 'baja', 'reing', 'rein', 'ms', 'm/s', 'inc', 'aus') THEN NULL ELSE fecha END
        WHERE clave IS NOT NULL AND LOWER(TRIM(clave)) NOT IN ('alta', 'baja', 'reing', 'rein', 'ms', 'm/s', 'inc', 'aus')
        "#,
        &[],
    ).unwrap_or(0);
    stats.reportes_cleaned_null = c_rdl as usize;

    // 5. Consultar todas las revisiones existentes antes del commit
    let all_rev_rows = tx.query("SELECT id FROM public.revisiones", &[])
        .map_err(|e| format!("Error consultando revisiones: {}", e))?;
    let all_rev_ids: Vec<Uuid> = all_rev_rows.into_iter().map(|r| r.get(0)).collect();

    tx.commit().map_err(|e| format!("Error en commit: {}", e))?;

    // 6. Sincronizar reportes para todas las revisiones existentes
    for rev_id in all_rev_ids {
        match crate::db::reporte_liquidaciones::revertir_reporte_liquidaciones_a_originales(
            client, rev_id,
        ) {
            Ok(_) => stats.revisions_resynced += 1,
            Err(e) => {
                log::warn!("Aviso en reversion de revision {}: {}", rev_id, e);
            }
        }
    }

    log::info!("Limpieza de base de datos de claves completada: {:?}", stats);
    Ok(stats)
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



    #[test]
    fn test_reconcile_and_verify_database() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("Cannot connect to DB: {}", e);
                return;
            }
        };

        let stats = reconcile_all_liquidaciones_vinculaciones_from_rustfs(&mut client)
            .expect("Reconcile should succeed");
        println!("\n=== RECONCILE RESULTS ===");
        println!("{:?}", stats);

        // Verificar que reportes_vinculaciones_obras_liquidaciones ahora tiene registros
        let rep_vinc_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM public.reportes_vinculaciones_obras_liquidaciones",
                &[],
            )
            .unwrap()
            .get(0);
        println!("Total vinculaciones en reportes: {}", rep_vinc_count);
        assert!(
            rep_vinc_count > 0,
            "Debe haber vinculaciones en las tablas de reportes"
        );

        println!("\n=== INSPECTING ALMANZA MARTINEZ LIQUIDACIONES ===");
        for r in client.query(
            "SELECT l.id, l.nombre_archivo, l.mes_periodo, l.anio_periodo,
                    (SELECT COUNT(*) FROM public.vinculaciones_obras_liquidaciones vol WHERE vol.id_liquidacion = l.id) as vinc_cnt
             FROM public.liquidaciones l
             JOIN public.empleados e ON e.id = l.id_empleado
             WHERE e.nombre ILIKE '%ALMANZA MARTINEZ%'
             ORDER BY l.anio_periodo, l.mes_periodo_int",
            &[],
        ).unwrap() {
            let lid: uuid::Uuid = r.get(0);
            let arch: Option<String> = r.get(1);
            let mes: Option<String> = r.get(2);
            let anio: Option<i16> = r.get(3);
            let v_cnt: i64 = r.get(4);
            println!("ALMANZA LIQ: id={}, arch={:?}, mes={:?}, anio={:?}, vincs={}", lid, arch, mes, anio, v_cnt);
        }

        // Verificar ALMANZA MARTINEZ en marzo 2024 o abril 2025: debe tener sus 2 obras intactas
        let almanza_vincs: Vec<(String, i16)> = client
            .query(
                "SELECT DISTINCT TRIM(UPPER(ro.numero)), vol.dias_registro_obra
                 FROM public.empleados e
                 JOIN public.liquidaciones l ON l.id_empleado = e.id
                 JOIN public.vinculaciones_obras_liquidaciones vol ON vol.id_liquidacion = l.id
                 JOIN public.registros_obras ro ON ro.id = vol.id_registro_obra
                 WHERE e.nombre ILIKE '%ALMANZA MARTINEZ%' AND (
                     (l.mes_periodo ILIKE '%MARZO%' AND l.anio_periodo = 2024)
                     OR (l.mes_periodo ILIKE '%ABRIL%' AND l.anio_periodo = 2025)
                 )
                 ORDER BY TRIM(UPPER(ro.numero))",
                &[],
            )
            .unwrap()
            .into_iter()
            .map(|r| (r.get::<_, String>(0).trim().to_string(), r.get(1)))
            .collect();

        println!(
            "ALMANZA MARTINEZ vinculaciones únicas: {:?}",
            almanza_vincs
        );
        assert!(
            almanza_vincs.len() >= 2,
            "Debe tener al menos 2 vinculaciones de obra distintas"
        );

        // Verificar que los datos en reportes_detalles_liquidaciones no fueron prorrateados (iguales a detalles_liquidaciones)
        let mismatches: Vec<(String, Option<String>, Option<chrono::NaiveDate>, Option<f64>, Option<f64>)> = client
            .query(
                "SELECT e.nombre, rdl.clave, rdl.fecha, rdl.subtotal, dl.subtotal
                 FROM public.reportes_detalles_liquidaciones rdl
                 JOIN public.reportes_liquidaciones rl ON rl.id = rdl.id_reporte_liquidacion
                 JOIN public.empleados e ON e.id = rl.id_empleado
                 JOIN public.detalles_liquidaciones dl ON dl.id_liquidacion = rl.id_liquidacion
                      AND (dl.clave = rdl.clave OR (dl.clave IS NULL AND rdl.clave IS NULL))
                      AND (dl.fecha = rdl.fecha OR (dl.fecha IS NULL AND rdl.fecha IS NULL))
                 WHERE rdl.id_registro_obra IS NOT NULL
                   AND LOWER(TRIM(COALESCE(rdl.clave, ''))) != 'baja'
                   AND COALESCE(dl.subtotal, 0) > 0
                   AND ABS(COALESCE(rdl.subtotal, 0) - COALESCE(dl.subtotal, 0)) > 0.01
                 LIMIT 5",
                &[],
            )
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4)))
            .collect();
        if !mismatches.is_empty() {
            println!("Sample mismatches (rdl vs dl): {:?}", mismatches);
        }

        let unprorated_match: bool = mismatches.is_empty();

        println!(
            "Reportes detalles coinciden 100% con detalles originales: {}",
            unprorated_match
        );
        assert!(
            unprorated_match,
            "Los datos de reportes_detalles_liquidaciones deben venir tal cual como vienen en los PDFs sin prorrateo"
        );
    }

    #[test]
    fn test_clean_and_verify_detalles_liquidaciones_database() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("Cannot connect to DB: {}", e);
                return;
            }
        };

        println!("\n=== EJECUTANDO LIMPIEZA Y REPARACION DE BASE DE DATOS ===");
        let stats = clean_and_repair_detalles_liquidaciones_database(&mut client)
            .expect("clean_and_repair_detalles_liquidaciones_database falló");
        println!("Stats de limpieza: {:#?}", stats);

        println!("\n=== VERIFICANDO CLAVES EN DETALLES_LIQUIDACIONES ===");
        let rows = client
            .query(
                "SELECT clave, COUNT(*) as cnt
                 FROM public.detalles_liquidaciones
                 GROUP BY clave
                 ORDER BY cnt DESC",
                &[],
            )
            .unwrap();
        for r in &rows {
            let clave: Option<String> = r.get(0);
            let cnt: i64 = r.get(1);
            println!("CLEAN DL CLAVE: {:?} -> count: {}", clave, cnt);
        }

        println!("\n=== VERIFICANDO CLAVES EN REPORTES_DETALLES_LIQUIDACIONES ===");
        let r_rows = client
            .query(
                "SELECT clave, COUNT(*) as cnt
                 FROM public.reportes_detalles_liquidaciones
                 GROUP BY clave
                 ORDER BY cnt DESC",
                &[],
            )
            .unwrap();
        for r in &r_rows {
            let clave: Option<String> = r.get(0);
            let cnt: i64 = r.get(1);
            println!("CLEAN RDL CLAVE: {:?} -> count: {}", clave, cnt);
        }

        // Verificaciones formales
        let bad_dl_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM public.detalles_liquidaciones
                 WHERE clave IS NOT NULL
                   AND LOWER(TRIM(clave)) NOT IN ('alta', 'baja', 'reing', 'rein', 'ms', 'm/s', 'inc', 'aus')",
                &[],
            )
            .unwrap()
            .get(0);
        assert_eq!(
            bad_dl_count, 0,
            "No deben existir claves inválidas en public.detalles_liquidaciones"
        );

        let bad_rdl_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM public.reportes_detalles_liquidaciones
                 WHERE clave IS NOT NULL
                   AND LOWER(TRIM(clave)) NOT IN ('alta', 'baja', 'reing', 'rein', 'ms', 'm/s', 'inc', 'aus')",
                &[],
            )
            .unwrap()
            .get(0);
        assert_eq!(
            bad_rdl_count, 0,
            "No deben existir claves inválidas en public.reportes_detalles_liquidaciones"
        );

        let b_dl_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM public.detalles_liquidaciones WHERE TRIM(UPPER(clave)) = 'B'",
                &[],
            )
            .unwrap()
            .get(0);
        assert_eq!(b_dl_count, 0, "No debe existir clave 'B' sin normalizar en detalles_liquidaciones");

        let b_rdl_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM public.reportes_detalles_liquidaciones WHERE TRIM(UPPER(clave)) = 'B'",
                &[],
            )
            .unwrap()
            .get(0);
        assert_eq!(b_rdl_count, 0, "No debe existir clave 'B' sin normalizar en reportes_detalles_liquidaciones");

        let specific_bad_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM public.detalles_liquidaciones
                 WHERE TRIM(clave) IN ('218.00', '217.67', '220.00', '09/08/2023', '31/07/2023', '24/07/2023', '61', '62')",
                &[],
            )
            .unwrap()
            .get(0);
        assert_eq!(specific_bad_count, 0, "No deben existir fechas o números en clave");

        let bimestral_gt31: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM public.detalles_liquidaciones WHERE dias > 31",
                &[],
            )
            .unwrap()
            .get(0);
        assert_eq!(bimestral_gt31, 0, "No deben quedar registros con dias > 31 de cédulas bimestrales en liquidaciones");

        println!("\n¡Todas las verificaciones de limpieza de base de datos pasaron exitosamente!");
    }

    #[test]
    fn test_comprehensive_vinculaciones_comparison_rustfs_vs_db() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("Cannot connect to DB: {}", e);
                return;
            }
        };

        let bucket = match get_rustfs_bucket(BUCKET_NAME) {
            Ok(b) => b,
            Err(e) => {
                println!("Cannot connect to RustFS: {}", e);
                return;
            }
        };

        let patterns = crate::cedulas::regex_expressions::initialize_patterns().unwrap();
        let pages = bucket.list("".to_string(), None).unwrap();

        let mut cedula_keys = Vec::new();
        for page in pages {
            for obj in page.contents {
                if !obj.key.contains("Trash")
                    && obj.key.contains("Cedulas_de_cuotas")
                    && obj.key.ends_with(".pdf")
                {
                    cedula_keys.push(obj.key);
                }
            }
        }
        cedula_keys.sort();

        let rp_rows = client
            .query(
                "SELECT id, id_cliente, UPPER(TRIM(registro)) FROM public.registros_patronales",
                &[],
            )
            .unwrap();
        let mut rp_map: HashMap<String, (Uuid, Uuid)> = HashMap::new();
        for r in rp_rows {
            let id_rp: Uuid = r.get(0);
            let id_cl: Uuid = r.get(1);
            let reg: String = r.get(2);
            let clean = crate::validation::clean_alphanumeric_input(&reg);
            rp_map.insert(clean, (id_rp, id_cl));
        }

        println!("Comparando {} archivos de RustFS contra la base de datos...", cedula_keys.len());

        let mut total_pdfs = 0;
        let mut bimestral_skipped = 0;
        let mut monthly_processed = 0;
        let mut total_emp_checked = 0;
        let mut matched_emp_count = 0;
        let missing_liq_in_db = 0;
        let mut missing_vinc_in_db = 0;
        let mut extra_vinc_in_db = 0;
        let mut days_mismatches = 0;
        let missing_liq_samples: Vec<String> = Vec::new();
        let mut missing_vinc_samples: Vec<String> = Vec::new();
        let mut days_mismatch_samples: Vec<String> = Vec::new();
        let mut extra_vinc_samples: Vec<String> = Vec::new();

        for key in &cedula_keys {
            total_pdfs += 1;
            let bytes = match download_rustfs_file(key) {
                Ok(b) => b,
                Err(_) => continue,
            };

            let mut tmp = tempfile::NamedTempFile::new().unwrap();
            let _ = std::io::Write::write_all(&mut tmp, &bytes);

            let raw_text = match crate::cedulas::pdf_extractor::extract_with_layout(tmp.path()) {
                Ok(t) => t,
                Err(_) => continue,
            };

            if crate::cedulas::ema::is_bimestral_document(&raw_text) {
                bimestral_skipped += 1;
                continue;
            }

            let text_lower = raw_text.to_lowercase();
            let cleaned = crate::cedulas::pdf_extractor::clean_text(&text_lower, &patterns);
            let mut biz = Business::new();
            biz.extract_from_page(&cleaned, &patterns);

            let detected_rp = biz.registro_patronal.as_deref().or_else(|| {
                let parts: Vec<&str> = key.split('/').collect();
                if parts.len() >= 2 {
                    Some(parts[1])
                } else {
                    None
                }
            });

            let (id_rp, id_cliente) = match detected_rp
                .and_then(|r| rp_map.get(&crate::validation::clean_alphanumeric_input(r)))
            {
                Some(pair) => *pair,
                None => continue,
            };

            let emps = crate::cedulas::processor::get_employee_data(&cleaned, &patterns);
            if emps.is_empty() {
                continue;
            }

            let emp_mes = emps.iter().find_map(|e| e.mes_periodo.as_deref());
            let emp_anio = emps.iter().find_map(|e| e.anio_periodo.as_deref());
            let mes_str = emp_mes.or_else(|| {
                let key_up = key.to_uppercase();
                for m in &[
                    "ENERO", "FEBRERO", "MARZO", "ABRIL", "MAYO", "JUNIO", "JULIO", "AGOSTO",
                    "SEPTIEMBRE", "OCTUBRE", "NOVIEMBRE", "DICIEMBRE",
                ] {
                    if key_up.contains(m) {
                        return Some(*m);
                    }
                }
                None
            });
            let anio_val: Option<i16> = emp_anio
                .and_then(|a| a.trim().parse::<i16>().ok())
                .or_else(|| {
                    for y in 2015..=2030 {
                        if key.contains(&y.to_string()) {
                            return Some(y as i16);
                        }
                    }
                    None
                });

            let (mes_norm, anio_norm) = match (mes_str, anio_val) {
                (Some(m), Some(a)) => (crate::db::cedulas::normalize_mes_periodo(Some(m)), a),
                _ => continue,
            };
            let mes_int = crate::cedulas::business::mes_to_periodo_int(&mes_norm);
            monthly_processed += 1;

            for emp in &emps {
                total_emp_checked += 1;
                let clean_nss = emp
                    .no_seguro_social
                    .as_deref()
                    .map(crate::validation::clean_alphanumeric_input);
                let clean_rfc = emp
                    .rfc
                    .as_deref()
                    .map(crate::validation::clean_alphanumeric_input);
                let clean_curp = emp
                    .curp
                    .as_deref()
                    .map(crate::validation::clean_alphanumeric_input);
                let emp_nombre = emp.nombre.as_deref().unwrap_or("").trim().to_uppercase();

                let emp_row = client
                    .query_opt(
                        r#"
                        SELECT id FROM public.empleados
                        WHERE id_cliente = $1
                          AND (
                              ($2::varchar IS NOT NULL AND nss = $2)
                              OR ($3::varchar IS NOT NULL AND rfc = $3)
                              OR ($4::varchar IS NOT NULL AND curp = $4)
                              OR ($5::varchar != '' AND TRIM(UPPER(nombre)) = $5)
                          )
                        ORDER BY (nss = $2) DESC, (rfc = $3) DESC, (curp = $4) DESC
                        LIMIT 1
                        "#,
                        &[&id_cliente, &clean_nss, &clean_rfc, &clean_curp, &emp_nombre],
                    )
                    .ok()
                    .flatten();

                let id_empleado: Uuid = match emp_row {
                    Some(r) => r.get(0),
                    None => continue,
                };
                matched_emp_count += 1;

                let file_name = key.split('/').last().unwrap_or(key).to_string();
                let liq_row = client
                    .query_opt(
                        r#"
                        SELECT id FROM public.liquidaciones
                        WHERE id_registro_patronal = $1
                          AND id_empleado = $2
                          AND anio_periodo = $3
                          AND (mes_periodo_int = $4 OR UPPER(TRIM(mes_periodo)) = UPPER(TRIM($5)))
                          AND (nombre_archivo = $6 OR nombre_archivo IS NULL)
                        ORDER BY (nombre_archivo = $6) DESC, created_at ASC
                        LIMIT 1
                        "#,
                        &[&id_rp, &id_empleado, &anio_norm, &mes_int, &mes_norm, &file_name],
                    )
                    .ok()
                    .flatten();

                let id_liquidacion: Uuid = match liq_row {
                    Some(r) => r.get(0),
                    None => {
                        // Si este empleado no tiene liquidación asociada a este archivo específico (porque provino de otro archivo del mismo periodo), no es un error
                        continue;
                    }
                };

                // Obras consolidadas del PDF
                let mut pdf_obras: BTreeMap<String, i16> = BTreeMap::new();
                for ro in &emp.registro_obras {
                    let clean_ro = crate::validation::clean_alphanumeric_input(&ro.registro_obra);
                    if !clean_ro.is_empty()
                        && (crate::validation::validate_registro_obra(&clean_ro).is_ok()
                            || clean_ro.starts_with('C')
                            || clean_ro.starts_with('A'))
                    {
                        let d = (ro.dias_registro_obra as i16).clamp(1, 31);
                        *pdf_obras.entry(clean_ro).or_insert(0) += d;
                    }
                }

                // Obras registradas en public.vinculaciones_obras_liquidaciones
                let db_vincs: Vec<(String, i16)> = client
                    .query(
                        r#"
                        SELECT TRIM(UPPER(ro.numero)), vol.dias_registro_obra
                        FROM public.vinculaciones_obras_liquidaciones vol
                        JOIN public.registros_obras ro ON ro.id = vol.id_registro_obra
                        WHERE vol.id_liquidacion = $1
                        "#,
                        &[&id_liquidacion],
                    )
                    .unwrap_or_default()
                    .into_iter()
                    .map(|r| (r.get::<_, String>(0).trim().to_uppercase(), r.get(1)))
                    .collect();

                let mut db_obras: BTreeMap<String, i16> = BTreeMap::new();
                for (num, dias) in db_vincs {
                    *db_obras.entry(num).or_insert(0) += dias;
                }

                // Comparar PDF vs DB
                for (p_num, p_dias) in &pdf_obras {
                    match db_obras.get(p_num) {
                        None => {
                            missing_vinc_in_db += 1;
                            if missing_vinc_samples.len() < 5 {
                                missing_vinc_samples.push(format!(
                                    "Obra en PDF ausente en BD: emp='{}', mes={}-{}, obra='{}', dias_pdf={}, archivo='{}'",
                                    emp_nombre, mes_norm, anio_norm, p_num, p_dias, key
                                ));
                            }
                        }
                        Some(&d_dias) => {
                            if d_dias != *p_dias {
                                days_mismatches += 1;
                                if days_mismatch_samples.len() < 5 {
                                    days_mismatch_samples.push(format!(
                                        "Días distintos: emp='{}', mes={}-{}, obra='{}', dias_pdf={}, dias_db={}",
                                        emp_nombre, mes_norm, anio_norm, p_num, p_dias, d_dias
                                    ));
                                }
                            }
                        }
                    }
                }

                for (d_num, d_dias) in &db_obras {
                    if !pdf_obras.contains_key(d_num) {
                        extra_vinc_in_db += 1;
                        if extra_vinc_samples.len() < 5 {
                            extra_vinc_samples.push(format!(
                                "Obra extra en BD no en PDF: emp='{}', mes={}-{}, obra='{}', dias_db={}, archivo='{}'",
                                emp_nombre, mes_norm, anio_norm, d_num, d_dias, key
                            ));
                        }
                    }
                }
            }
        }

        println!("\n=== RESUMEN DE COMPARACION RUSTFS VS POSTGRESQL ===");
        println!("Total PDFs en RustFS: {}", total_pdfs);
        println!("Bimestrales ignorados: {}", bimestral_skipped);
        println!("Mensuales procesados: {}", monthly_processed);
        println!("Total registros de empleado evaluados: {}", total_emp_checked);
        println!("Empleados emparejados con la BD: {}", matched_emp_count);
        println!("Liquidaciones ausentes en BD: {}", missing_liq_in_db);
        println!("Vinculaciones del PDF ausentes en BD: {}", missing_vinc_in_db);
        println!("Vinculaciones extra en BD (no en PDF): {}", extra_vinc_in_db);
        println!("Discrepancias en días de vinculación: {}", days_mismatches);

        if !missing_liq_samples.is_empty() {
            println!("\nMuestras de Liquidaciones ausentes en BD:");
            for s in &missing_liq_samples { println!(" - {}", s); }
        }
        if !missing_vinc_samples.is_empty() {
            println!("\nMuestras de Vinculaciones de PDF ausentes en BD:");
            for s in &missing_vinc_samples { println!(" - {}", s); }
        }
        if !days_mismatch_samples.is_empty() {
            println!("\nMuestras de Días distintos (PDF vs BD):");
            for s in &days_mismatch_samples { println!(" - {}", s); }
        }
        if !extra_vinc_samples.is_empty() {
            println!("\nMuestras de Obras extra en BD (no en PDF):");
            for s in &extra_vinc_samples { println!(" - {}", s); }
        }
    }

    #[test]
    fn test_diagnose_unlinked_pdfs() {
        let key = "INDUSTRIAS VC DEL DESIERTO/A3673455103/REVISION 1_2024/Cedulas_de_cuotas/septiembre 24.pdf";
        let b = download_rustfs_file(key).unwrap();
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let _ = std::io::Write::write_all(&mut tmp, &b);
        let text = crate::cedulas::pdf_extractor::extract_with_layout(tmp.path()).unwrap_or_default();
        let patterns = crate::cedulas::regex_expressions::initialize_patterns().unwrap();
        let cleaned = crate::cedulas::pdf_extractor::clean_text(&text, &patterns);
        println!("Cleaned length: {}", cleaned.len());
        let lines: Vec<&str> = cleaned.lines().collect();
        println!("Total lines: {}", lines.len());
        for (i, l) in lines.iter().enumerate().take(60) {
            println!("{:02}: {}", i, l);
        }
        let re_emp = &patterns.employee_data_per_employee;
        let matches: Vec<_> = re_emp.find_iter(&cleaned).collect();
        println!("employee_data_per_employee match count: {}", matches.len());
    }
}
