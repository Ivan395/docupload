use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use mupdf::{Document, TextBlockContent, TextPageFlags};

use crate::cedulas::regex_expressions::OCRPatterns;

struct CharInfo {
    ch: char,
    ox: f32,
    oy: i32,
    cwidth: f32,
}

/// Natively extracts text from a PDF page simulating the PyMuPDF layout engine
/// to maintain tabular column alignments without needing an external Python process.
fn extract_page_layout_native(page: &mupdf::Page) -> Result<String, String> {
    let text_page = page
        .to_text_page(TextPageFlags::PRESERVE_WHITESPACE | TextPageFlags::PRESERVE_LIGATURES)
        .map_err(|e| format!("Error creando TextPage en MuPDF: {}", e))?;

    let structured = text_page.structured();

    let mut chars: Vec<CharInfo> = Vec::new();
    let mut left = f32::MAX;
    let mut right = 0.0f32;

    for block in structured.blocks {
        if let TextBlockContent::Text { lines } = block.content {
            for line in lines {
                for sc in line.chars {
                    let ch = sc.ch;
                    let ox = sc.origin.x;
                    let oy = sc.origin.y.round() as i32;
                    let cwidth = (sc.quad.ur.x - sc.quad.ul.x).abs().max(1.0);

                    if ch != ' ' && ox < left {
                        left = ox;
                    }
                    if sc.quad.ur.x > right {
                        right = sc.quad.ur.x;
                    }

                    chars.push(CharInfo { ch, ox, oy, cwidth });
                }
            }
        }
    }

    if chars.is_empty() {
        return Ok(String::new());
    }

    // 1. Group rows by baseline coordinate Y (tolerance of 2px)
    let mut raw_rows: Vec<i32> = chars.iter().map(|c| c.oy).collect();
    raw_rows.sort_unstable();
    raw_rows.dedup();

    let mut curated_rows: Vec<i32> = Vec::new();
    for &r in &raw_rows {
        if curated_rows.is_empty() || (r - curated_rows.last().copied().unwrap_or(0)) > 2 {
            curated_rows.push(r);
        }
    }

    let find_row = |oy: i32| -> i32 {
        let mut best_row = curated_rows[0];
        let mut min_diff = (oy - best_row).abs();
        for &r in &curated_rows[1..] {
            let diff = (oy - r).abs();
            if diff < min_diff {
                min_diff = diff;
                best_row = r;
            }
        }
        best_row
    };

    let mut lines_map: BTreeMap<i32, Vec<CharInfo>> = BTreeMap::new();
    for c in chars {
        let target_y = find_row(c.oy);
        lines_map.entry(target_y).or_default().push(c);
    }

    // 2. Compute slot resolution (median character width)
    let mut slot = (right - left).max(1.0);
    let mut line_slots: Vec<f32> = Vec::new();
    for lchars in lines_map.values() {
        if lchars.len() >= 2 {
            let mut widths: Vec<f32> = lchars.iter().map(|c| c.cwidth).collect();
            widths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let median = widths[widths.len() / 2];
            if median > 0.0 && median < slot {
                slot = median;
            }
            line_slots.push(widths[0]);
        }
    }

    let min_slot = if line_slots.is_empty() {
        1.0
    } else {
        line_slots
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min)
            .max(0.5)
    };

    // 3. Assemble text lines with aligned spaces
    let mut output = String::new();
    for (_y, mut lchars) in lines_map {
        lchars.sort_by(|a, b| a.ox.partial_cmp(&b.ox).unwrap_or(std::cmp::Ordering::Equal));

        let mut line_text = String::new();
        let mut old_char = '\0';
        let mut old_x1 = 0.0f32;
        let mut old_ox = 0.0f32;

        for c in lchars {
            let ch = c.ch;
            let rel_ox = (c.ox - left).max(0.0);
            let x1 = rel_ox + c.cwidth;

            // Eliminate overprint
            if old_char == ch && (rel_ox - old_ox) <= c.cwidth * 0.2 {
                continue;
            }

            // Omit redundant spaces overlapping
            if ch == ' ' && (old_x1 - rel_ox) / c.cwidth > 0.8 {
                continue;
            }

            old_char = ch;

            // Character adjacent to previous?
            if rel_ox < old_x1 + min_slot {
                line_text.push(ch);
                old_x1 = x1;
                old_ox = rel_ox;
                continue;
            }

            if ch == ' ' {
                continue;
            }

            let delta = (rel_ox / slot) as i32 - line_text.chars().count() as i32;
            if rel_ox > old_x1 && delta > 1 {
                for _ in 0..delta {
                    line_text.push(' ');
                }
            }

            line_text.push(ch);
            old_x1 = x1;
            old_ox = rel_ox;
        }

        output.push_str(line_text.trim_end());
        output.push('\n');
    }

    Ok(output)
}

/// Native in-process PDF layout extraction using MuPDF crate
pub fn extract_with_mupdf_native(pdf_path: &Path) -> Result<String, String> {
    let doc = Document::open(pdf_path)
        .map_err(|e| format!("MuPDF no pudo abrir el archivo PDF: {}", e))?;

    let page_count = doc
        .page_count()
        .map_err(|e| format!("Error leyendo páginas con MuPDF: {}", e))?;

    let mut full_text = String::new();

    for i in 0..page_count {
        let page = doc
            .load_page(i)
            .map_err(|e| format!("Error cargando página {} con MuPDF: {}", i + 1, e))?;

        let page_text = extract_page_layout_native(&page)?;
        full_text.push_str(&page_text);
        full_text.push('\x0c'); // Formfeed page separator
    }

    Ok(full_text)
}

/// Fallback extraction using pymupdf CLI if needed
fn extract_with_pymupdf_cli(pdf_path: &Path) -> Result<String, String> {
    let custom_bin = Path::new("/home/ivan395/.local/bin/pymupdf");
    let cmd_name = if custom_bin.exists() {
        "/home/ivan395/.local/bin/pymupdf"
    } else {
        "pymupdf"
    };

    let mut output = Command::new(cmd_name)
        .env("PYTHONIOENCODING", "utf-8")
        .arg("gettext")
        .arg("-mode")
        .arg("layout")
        .arg("-output")
        .arg("/dev/stdout")
        .arg(pdf_path)
        .output();

    if output.is_err() {
        output = Command::new("python3")
            .env("PYTHONIOENCODING", "utf-8")
            .arg("-m")
            .arg("fitz")
            .arg("gettext")
            .arg("-mode")
            .arg("layout")
            .arg("-output")
            .arg("/dev/stdout")
            .arg(pdf_path)
            .output();
    }

    match output {
        Ok(out) => {
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            } else {
                let err_msg = String::from_utf8_lossy(&out.stderr);
                Err(format!("PyMuPDF falló al procesar el archivo: {}", err_msg))
            }
        }
        Err(e) => Err(format!(
            "No se pudo ejecutar pymupdf CLI de respaldo: {}",
            e
        )),
    }
}

/// Primary extraction function:
/// Tries native in-process extraction via MuPDF crate first.
/// Falls back to PyMuPDF CLI if native extraction encounters an unexpected error.
pub fn extract_with_layout(pdf_path: &Path) -> Result<String, String> {
    if !pdf_path.exists() {
        return Err("El archivo PDF no existe o la ruta es inválida".to_string());
    }

    // 1. Try native MuPDF first (independent, standalone, fast)
    match extract_with_mupdf_native(pdf_path) {
        Ok(text) => Ok(text),
        Err(native_err) => {
            log::warn!(
                "MuPDF nativo retornó error ({}), intentando con CLI de respaldo...",
                native_err
            );
            // 2. Fallback to CLI
            extract_with_pymupdf_cli(pdf_path)
        }
    }
}

pub fn clean_text(text: &str, patterns: &OCRPatterns) -> String {
    let mut current_text = text.to_string();

    let mut last_nss_start = 0;
    let mut has_employees = false;
    let mut safe_cutoff = 0;
    if let Some(last_match) = patterns
        .employee_nss
        .find_iter(&current_text)
        .flatten()
        .last()
    {
        last_nss_start = last_match.start();
        safe_cutoff = last_match.end() + 350;
        has_employees = true;
    }

    let mut truncated = false;
    if has_employees {
        if let Some(last_dias_match) = patterns
            .employee_dias_cutoff
            .find_iter(&current_text)
            .flatten()
            .last()
        {
            if last_dias_match.start() > last_nss_start {
                current_text.truncate(last_dias_match.end());
                truncated = true;
            }
        }
    }

    if !truncated {
        if let Ok(Some(mat)) = patterns.clean_remove_summary_blocks.find(&current_text) {
            let match_start = mat.start();
            if match_start > safe_cutoff {
                current_text.truncate(match_start);
            }
        }
    }

    current_text = current_text.replace("dã-as", "dias");
    current_text = current_text.replace("dãías", "dias");

    let res = patterns
        .clean_remove_floating_colons
        .replace_all(&current_text, "");
    let res = patterns.clean_remove_spaces.replace_all(&res, " ");
    let res = patterns.clean_remove_commas.replace_all(&res, "");
    let res = patterns.clean_remove_unnecesary_date.replace_all(&res, "");
    let res = patterns
        .clean_remove_unnecesary_spaces
        .replace_all(&res, "");

    res.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_invalid_path() {
        let res = extract_with_layout(Path::new("/non/existent/path.pdf"));
        assert!(res.is_err());
    }

    #[test]
    fn test_clean_text_normalizations() {
        let patterns = crate::cedulas::regex_expressions::initialize_patterns().unwrap();
        let sample = "dã-as trabajados   5  : \n  1,234.56";
        let cleaned = clean_text(sample, &patterns);
        assert!(!cleaned.contains("dã-as"));
        assert!(!cleaned.contains(","));
    }
}

