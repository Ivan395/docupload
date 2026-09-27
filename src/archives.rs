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
                // Scan directory recursively for direct files and nested archives
                let mut dir_files = Vec::new();
                for entry in WalkDir::new(path).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        let p = entry.path();
                        if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                            let ext_lower = ext.to_lowercase();
                            if ext_lower == target_ext_lower {
                                dir_files.push(p.to_path_buf());
                            } else if ext_lower == "zip" {
                                if let Ok(extracted) = extract_zip(p, &target_ext_lower, temp_dir, progress_callback) {
                                    dir_files.extend(extracted);
                                }
                            } else if ext_lower == "rar" {
                                if let Ok(extracted) = extract_rar(p, &target_ext_lower, temp_dir) {
                                    dir_files.extend(extracted);
                                }
                            }
                        }
                    }
                }
                return Ok(dir_files);
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
    let zip_filename = zip_path.file_name().unwrap_or_default().to_string_lossy().into_owned();

    // 1. First pass: inspect central directory to find all matching entries
    let matching_entries: Vec<(usize, PathBuf)> = {
        let file = File::open(zip_path)
            .map_err(|e| format!("No se pudo abrir ZIP {}: {}", zip_filename, e))?;

        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| format!("Error leyendo contenido de ZIP {}: {}", zip_filename, e))?;

        let total_in_zip = archive.len();
        let mut list = Vec::new();

        for i in 0..total_in_zip {
            if let Ok(entry) = archive.by_index_raw(i) {
                if !entry.is_dir() {
                    if let Some(enclosed) = entry.enclosed_name() {
                        if let Some(ext) = enclosed.extension().and_then(|s| s.to_str()) {
                            if ext.eq_ignore_ascii_case(target_extension) {
                                list.push((i, enclosed.to_path_buf()));
                            }
                        }
                    }
                }
            }
        }
        list
    };

    if matching_entries.is_empty() {
        return Ok(Vec::new());
    }

    let total_matching = matching_entries.len();
    let archive_stem = zip_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("zip_extract");
    let dest_folder = get_unique_temp_folder(temp_dir, archive_stem);
    fs::create_dir_all(&dest_folder)
        .map_err(|e| format!("Error creando carpeta temporal: {}", e))?;

    // 2. Extract entries in parallel across Rayon worker threads
    let extracted_counter = std::sync::atomic::AtomicUsize::new(0);

    // Chunk size balanced for CPU cache & thread utilization
    let chunk_size = (total_matching / rayon::current_num_threads().max(1)).clamp(8, 64);

    let extracted_chunks: Result<Vec<Vec<PathBuf>>, String> = matching_entries
        .par_chunks(chunk_size)
        .map(|chunk| {
            // Open a separate file handle per chunk/thread to allow full parallel seeking and decompression
            let file = File::open(zip_path)
                .map_err(|e| format!("Error abriendo ZIP en hilo de extracción: {}", e))?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|e| format!("Error accediendo a ZIP en hilo de extracción: {}", e))?;

            let mut chunk_paths = Vec::with_capacity(chunk.len());

            for &(idx, ref enclosed_name) in chunk {
                let mut file_entry = archive
                    .by_index(idx)
                    .map_err(|e| format!("Error accediendo a archivo {} en ZIP: {}", idx, e))?;

                let outpath = dest_folder.join(enclosed_name);
                if let Some(parent) = outpath.parent() {
                    if !parent.exists() {
                        let _ = fs::create_dir_all(parent);
                    }
                }

                let outfile_raw = File::create(&outpath).map_err(|e| {
                    format!("Error creando archivo extraído {:?}: {}", outpath, e)
                })?;
                let mut outfile = io::BufWriter::with_capacity(64 * 1024, outfile_raw);
                io::copy(&mut file_entry, &mut outfile).map_err(|e| {
                    format!("Error escribiendo archivo extraído {:?}: {}", outpath, e)
                })?;
                use std::io::Write;
                outfile.flush().map_err(|e| {
                    format!("Error finalizando archivo extraído {:?}: {}", outpath, e)
                })?;

                chunk_paths.push(outpath);

                let current = extracted_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if let Some(cb) = progress_callback {
                    if current % 40 == 0 || current == total_matching {
                        cb(
                            current,
                            total_matching,
                            &format!(
                                "Extrayendo {} archivos en paralelo: {}/{}...",
                                target_extension.to_uppercase(),
                                current,
                                total_matching
                            ),
                        );
                    }
                }
            }

            Ok(chunk_paths)
        })
        .collect();

    let all_extracted = extracted_chunks?.into_iter().flatten().collect();
    Ok(all_extracted)
}

fn get_7z_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        for p in &[
            r"C:\Program Files\7-Zip\7z.exe",
            r"C:\Program Files (x86)\7-Zip\7z.exe",
        ] {
            let path = Path::new(p);
            if path.exists() {
                candidates.push(path.to_path_buf());
            }
        }
    }
    candidates.push(PathBuf::from("7z"));
    candidates
}

fn get_unrar_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        for p in &[
            r"C:\Program Files\WinRAR\UnRAR.exe",
            r"C:\Program Files\WinRAR\WinRAR.exe",
            r"C:\Program Files (x86)\WinRAR\UnRAR.exe",
            r"C:\Program Files (x86)\WinRAR\WinRAR.exe",
        ] {
            let path = Path::new(p);
            if path.exists() {
                candidates.push(path.to_path_buf());
            }
        }
    }
    candidates.push(PathBuf::from("unrar"));
    candidates
}

fn create_archive_cmd(bin: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(bin);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    cmd
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

    let mut success = false;
    let mut executed_any = false;
    let mut last_error = String::new();

    // 1. Probar 7z (PATH o carpetas estándar en Windows)
    for bin in get_7z_candidates() {
        let mut cmd = create_archive_cmd(&bin);
        cmd.arg("x")
            .arg("-y")
            .arg(format!("-o{}", dest_folder.display()))
            .arg(rar_path);

        if let Ok(out) = cmd.output() {
            executed_any = true;
            if out.status.success() {
                success = true;
                break;
            } else {
                last_error = String::from_utf8_lossy(&out.stderr).to_string();
            }
        }
    }

    // 2. Si falló 7z, probar unrar (PATH o WinRAR en Windows)
    if !success {
        for bin in get_unrar_candidates() {
            let mut cmd = create_archive_cmd(&bin);
            cmd.arg("x")
                .arg("-y")
                .arg(rar_path)
                .arg(&dest_folder);

            if let Ok(out) = cmd.output() {
                executed_any = true;
                if out.status.success() {
                    success = true;
                    break;
                } else {
                    last_error = String::from_utf8_lossy(&out.stderr).to_string();
                }
            }
        }
    }

    if !executed_any {
        return Err(format!(
            "No se encontró '7z' ni 'unrar' en el sistema (ni en PATH ni en directorios comunes de instalación) para descomprimir RAR {:?}",
            rar_path.file_name().unwrap_or_default()
        ));
    }

    if !success {
        return Err(format!(
            "Falló la descompresión del archivo RAR {:?}. {}",
            rar_path.file_name().unwrap_or_default(),
            last_error.trim()
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
