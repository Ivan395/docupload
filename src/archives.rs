use rayon::prelude::*;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use walkdir::WalkDir;

static EXTRACT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn get_unique_temp_folder(temp_dir: &Path, prefix: &str) -> PathBuf {
    let id = EXTRACT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    temp_dir.join(format!("{}_{}_{}_{}", prefix, std::process::id(), millis, id))
}

/// Collects or extracts all files with `target_extension` (e.g., "xml" or "pdf")
/// from a collection of paths that may include single files, directories, `.zip`, or `.rar` archives.
/// Processes multiple files and archives concurrently in parallel.
pub fn collect_or_extract_files(
    input_paths: &[PathBuf],
    target_extension: &str,
    temp_dir: &Path,
    progress_callback: Option<&(dyn Fn(usize, usize, &str) + Send + Sync)>,
) -> Result<Vec<PathBuf>, String> {
    let target_ext_lower = target_extension.to_lowercase();

    let results: Result<Vec<Vec<PathBuf>>, String> = input_paths
        .par_iter()
        .map(|path| {
            let mut collected = Vec::new();
            if !path.exists() {
                return Ok(collected);
            }

            if path.is_dir() {
                // Scan directory recursively
                for entry in WalkDir::new(path).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                            if ext.eq_ignore_ascii_case(&target_ext_lower) {
                                collected.push(entry.path().to_path_buf());
                            }
                        }
                    }
                }
                return Ok(collected);
            }

            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();

            if ext == target_ext_lower {
                collected.push(path.clone());
            } else if ext == "zip" {
                let extracted = extract_zip(path, &target_ext_lower, temp_dir, progress_callback)?;
                collected.extend(extracted);
            } else if ext == "rar" {
                let extracted = extract_rar(path, &target_ext_lower, temp_dir)?;
                collected.extend(extracted);
            }

            Ok(collected)
        })
        .collect();

    let all_files = results?.into_iter().flatten().collect();
    Ok(all_files)
}

fn extract_zip(
    zip_path: &Path,
    target_extension: &str,
    temp_dir: &Path,
    progress_callback: Option<&(dyn Fn(usize, usize, &str) + Send + Sync)>,
) -> Result<Vec<PathBuf>, String> {
    let file = File::open(zip_path)
        .map_err(|e| format!("No se pudo abrir ZIP {:?}: {}", zip_path.file_name().unwrap_or_default(), e))?;

    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Error leyendo contenido de ZIP {:?}: {}", zip_path.file_name().unwrap_or_default(), e))?;

    let archive_stem = zip_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("zip_extract");
    let dest_folder = get_unique_temp_folder(temp_dir, archive_stem);
    fs::create_dir_all(&dest_folder)
        .map_err(|e| format!("Error creando carpeta temporal: {}", e))?;

    let mut extracted_paths = Vec::new();
    let total_in_zip = archive.len();

    for i in 0..total_in_zip {
        if let Some(cb) = progress_callback {
            if i % 30 == 0 || i + 1 == total_in_zip {
                cb(
                    i + 1,
                    total_in_zip,
                    &format!("Extrayendo archivos de ZIP: {}/{}...", i + 1, total_in_zip),
                );
            }
        }

        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Error accediendo a archivo {} en ZIP: {}", i, e))?;

        if file.is_dir() {
            continue;
        }

        let enclosed_name = match file.enclosed_name() {
            Some(path) => path.to_owned(),
            None => continue,
        };

        if let Some(ext) = enclosed_name.extension().and_then(|s| s.to_str()) {
            if ext.eq_ignore_ascii_case(target_extension) {
                let outpath = dest_folder.join(&enclosed_name);
                if let Some(parent) = outpath.parent() {
                    if !parent.exists() {
                        let _ = fs::create_dir_all(parent);
                    }
                }

                let outfile_raw = File::create(&outpath).map_err(|e| {
                    format!("Error creando archivo extraído {:?}: {}", outpath, e)
                })?;
                let mut outfile = io::BufWriter::new(outfile_raw);
                io::copy(&mut file, &mut outfile).map_err(|e| {
                    format!("Error escribiendo archivo extraído {:?}: {}", outpath, e)
                })?;
                use std::io::Write;
                outfile.flush().map_err(|e| {
                    format!("Error finalizando archivo extraído {:?}: {}", outpath, e)
                })?;

                extracted_paths.push(outpath);
            }
        }
    }

    Ok(extracted_paths)
}

fn extract_rar(
    rar_path: &Path,
    target_extension: &str,
    temp_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    let archive_stem = rar_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("rar_extract");
    let dest_folder = get_unique_temp_folder(temp_dir, archive_stem);
    fs::create_dir_all(&dest_folder)
        .map_err(|e| format!("Error creando carpeta temporal: {}", e))?;

    // Try 7z first
    let output_7z = Command::new("7z")
        .arg("x")
        .arg("-y")
        .arg(format!("-o{}", dest_folder.display()))
        .arg(rar_path)
        .output();

    let success = match output_7z {
        Ok(out) => out.status.success(),
        Err(_) => {
            // Fallback to unrar command
            let output_unrar = Command::new("unrar")
                .arg("x")
                .arg("-y")
                .arg(rar_path)
                .arg(&dest_folder)
                .output();
            match output_unrar {
                Ok(out) => out.status.success(),
                Err(e) => {
                    return Err(format!(
                        "No se encontró '7z' ni 'unrar' en el sistema para descomprimir RAR {:?}: {}",
                        rar_path.file_name().unwrap_or_default(),
                        e
                    ));
                }
            }
        }
    };

    if !success {
        return Err(format!(
            "Falló la descompresión del archivo RAR {:?}",
            rar_path.file_name().unwrap_or_default()
        ));
    }

    // Scan extracted files
    let mut extracted_paths = Vec::new();
    for entry in WalkDir::new(&dest_folder).into_iter().flatten() {
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                if ext.eq_ignore_ascii_case(target_extension) {
                    extracted_paths.push(entry.path().to_path_buf());
                }
            }
        }
    }

    Ok(extracted_paths)
}
