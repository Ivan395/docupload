//! Abstracción de almacenamiento de PDFs de cédulas con staging y promoción idempotente.
//!
//! Flujo: `subir_staging` escribe el archivo bajo `.staging_<batch>/` dentro de la
//! carpeta de cédulas; `promover` lo copia a su clave final y elimina el staging.
//! La implementación `AlmacenRustfs` usa el bucket real; `AlmacenMemoria` sirve para pruebas.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::rustfs::{ensure_bucket_exists, get_rustfs_bucket, sanitize_path_component, BUCKET_NAME};

/// Longitud del prefijo hexadecimal del hash incluido en el nombre estándar.
const LONGITUD_HASH_NOMBRE: usize = 12;

/// Calcula el SHA-256 de los datos y lo devuelve en 64 caracteres hexadecimales minúsculas.
pub fn sha256_hex(datos: &[u8]) -> String {
    let digest = Sha256::digest(datos);
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Inserta `__<sha12>` antes de la extensión `.pdf` del nombre estándar.
/// Es idempotente: si el nombre ya termina con ese sufijo, no lo duplica.
pub fn nombre_estandar_con_hash(nombre_estandar: &str, sha256: &str) -> String {
    let corto: String = sha256.chars().take(LONGITUD_HASH_NOMBRE).collect();
    let sufijo = format!("__{}", corto);

    let base = separar_extension_pdf(nombre_estandar).unwrap_or(nombre_estandar);

    if base.ends_with(&sufijo) {
        return format!("{}.pdf", base);
    }
    format!("{}{}.pdf", base, sufijo)
}

/// Si el nombre termina en `.pdf` (sin distinguir mayúsculas), devuelve la parte previa a la extensión.
fn separar_extension_pdf(nombre: &str) -> Option<&str> {
    let longitud = nombre.len();
    if longitud >= 4 && nombre.is_char_boundary(longitud - 4) {
        let (base, ext) = nombre.split_at(longitud - 4);
        if ext.eq_ignore_ascii_case(".pdf") {
            return Some(base);
        }
    }
    None
}

/// Construye la clave final: `<Cliente>/<RP>/<Revision>/Cedulas_de_cuotas/<archivo>`.
/// Los componentes de ruta se sanitizan; en el nombre solo se reemplazan `/` y `\`.
pub fn clave_final_cedula(cliente: &str, rp: &str, revision: &str, nombre_archivo: &str) -> String {
    let nombre_limpio = nombre_archivo.replace(['/', '\\'], "_");
    format!(
        "{}/{}/{}/Cedulas_de_cuotas/{}",
        sanitize_path_component(cliente),
        sanitize_path_component(rp),
        sanitize_path_component(revision),
        nombre_limpio
    )
}

/// Deriva la clave de staging insertando `.staging_<batch_id>/` antes del último segmento.
pub fn clave_staging(clave_final: &str, batch_id: &str) -> String {
    match clave_final.rfind('/') {
        Some(pos) => {
            let (carpeta, archivo) = clave_final.split_at(pos);
            format!("{}/.staging_{}{}", carpeta, batch_id, archivo)
        }
        None => format!(".staging_{}/{}", batch_id, clave_final),
    }
}

/// Operaciones de almacenamiento para PDFs de cédulas.
pub trait AlmacenArchivos: Send + Sync {
    /// Sube los bytes a la clave de staging derivada y la devuelve.
    fn subir_staging(&self, clave_final: &str, batch_id: &str, datos: &[u8]) -> Result<String, String>;
    /// Copia staging→final y borra staging. Idempotente: si staging ya no existe pero final sí, Ok(()).
    fn promover(&self, clave_staging: &str, clave_final: &str) -> Result<(), String>;
    /// Borra una clave (Ok si no existía).
    fn descartar(&self, clave: &str) -> Result<(), String>;
    /// Indica si la clave existe en el almacén.
    fn existe(&self, clave: &str) -> Result<bool, String>;
}

/// Implementación sobre RustFS (bucket `crate::rustfs::BUCKET_NAME`).
pub struct AlmacenRustfs;

impl AlmacenArchivos for AlmacenRustfs {
    fn subir_staging(&self, clave_final: &str, batch_id: &str, datos: &[u8]) -> Result<String, String> {
        ensure_bucket_exists()?;
        let bucket = get_rustfs_bucket(BUCKET_NAME)?;
        let clave = clave_staging(clave_final, batch_id);
        let respuesta = bucket
            .put_object(&clave, datos)
            .map_err(|e| format!("Error subiendo staging de cédula a RustFS ({}): {}", clave, e))?;
        // rust-s3 sin `fail-on-err` devuelve Ok también ante 4xx/5xx: validar el status explícitamente
        if !(200..300).contains(&respuesta.status_code()) {
            return Err(format!(
                "RustFS respondió {} al subir staging ({})",
                respuesta.status_code(),
                clave
            ));
        }
        Ok(clave)
    }

    fn promover(&self, clave_staging: &str, clave_final: &str) -> Result<(), String> {
        let bucket = get_rustfs_bucket(BUCKET_NAME)?;

        let respuesta = bucket
            .get_object(clave_staging)
            .map_err(|e| format!("Error leyendo staging de RustFS ({}): {}", clave_staging, e))?;

        if respuesta.status_code() != 200 {
            // Staging ausente: solo es válido si la promoción ya se realizó antes.
            return if self.existe(clave_final)? {
                Ok(())
            } else {
                Err(format!(
                    "No se puede promover: ni staging ({}) ni final ({}) existen",
                    clave_staging, clave_final
                ))
            };
        }

        let copia = bucket
            .put_object(clave_final, respuesta.as_slice())
            .map_err(|e| format!("Error copiando cédula a ruta final ({}): {}", clave_final, e))?;
        if !(200..300).contains(&copia.status_code()) {
            return Err(format!(
                "RustFS respondió {} al copiar a la ruta final ({})",
                copia.status_code(),
                clave_final
            ));
        }

        if !self.existe(clave_final)? {
            return Err(format!(
                "La verificación posterior a la promoción falló, no existe la clave final: {}",
                clave_final
            ));
        }

        bucket
            .delete_object(clave_staging)
            .map_err(|e| format!("Error eliminando staging ({}) tras promoción: {}", clave_staging, e))?;
        Ok(())
    }

    fn descartar(&self, clave: &str) -> Result<(), String> {
        let bucket = get_rustfs_bucket(BUCKET_NAME)?;
        bucket
            .delete_object(clave)
            .map_err(|e| format!("Error eliminando archivo de RustFS ({}): {}", clave, e))?;
        Ok(())
    }

    fn existe(&self, clave: &str) -> Result<bool, String> {
        let bucket = get_rustfs_bucket(BUCKET_NAME)?;
        let respuesta = bucket
            .get_object(clave)
            .map_err(|e| format!("Error verificando existencia en RustFS ({}): {}", clave, e))?;
        match respuesta.status_code() {
            200 => Ok(true),
            404 => Ok(false),
            otro => Err(format!(
                "Respuesta inesperada de RustFS ({}) al verificar {}",
                otro, clave
            )),
        }
    }
}

/// Implementación en memoria para pruebas unitarias. No toca RustFS.
#[derive(Debug, Default)]
pub struct AlmacenMemoria {
    objetos: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl AlmacenMemoria {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lista las claves almacenadas en orden lexicográfico.
    pub fn claves(&self) -> Vec<String> {
        let objetos = self.objetos.lock().unwrap_or_else(|e| e.into_inner());
        objetos.keys().cloned().collect()
    }

    fn bloquear(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.objetos.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl AlmacenArchivos for AlmacenMemoria {
    fn subir_staging(&self, clave_final: &str, batch_id: &str, datos: &[u8]) -> Result<String, String> {
        let clave = clave_staging(clave_final, batch_id);
        self.bloquear().insert(clave.clone(), datos.to_vec());
        Ok(clave)
    }

    fn promover(&self, clave_staging: &str, clave_final: &str) -> Result<(), String> {
        let mut objetos = self.bloquear();
        match objetos.get(clave_staging).cloned() {
            Some(datos) => {
                objetos.insert(clave_final.to_string(), datos);
                objetos.remove(clave_staging);
                Ok(())
            }
            None if objetos.contains_key(clave_final) => Ok(()),
            None => Err(format!(
                "No se puede promover: ni staging ({}) ni final ({}) existen",
                clave_staging, clave_final
            )),
        }
    }

    fn descartar(&self, clave: &str) -> Result<(), String> {
        self.bloquear().remove(clave);
        Ok(())
    }

    fn existe(&self, clave: &str) -> Result<bool, String> {
        Ok(self.bloquear().contains_key(clave))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_hex_de_abc() {
        assert_eq!(sha256_hex(b"abc"), HASH_ABC);
    }

    #[test]
    fn nombre_con_hash_conserva_patron_mes_anio() {
        let nombre = nombre_estandar_con_hash("A__B__MARZO__2026.pdf", HASH_ABC);
        assert_eq!(nombre, "A__B__MARZO__2026__ba7816bf8f01.pdf");
    }

    #[test]
    fn nombre_con_hash_es_idempotente() {
        let una = nombre_estandar_con_hash("A__B__MARZO__2026.pdf", HASH_ABC);
        let dos = nombre_estandar_con_hash(&una, HASH_ABC);
        assert_eq!(una, dos);
    }

    #[test]
    fn nombre_con_hash_sin_extension_agrega_pdf() {
        assert_eq!(
            nombre_estandar_con_hash("A__B__MARZO__2026", HASH_ABC),
            "A__B__MARZO__2026__ba7816bf8f01.pdf"
        );
    }

    #[test]
    fn nombre_con_hash_extension_mayusculas() {
        assert_eq!(
            nombre_estandar_con_hash("A__B__MARZO__2026.PDF", HASH_ABC),
            "A__B__MARZO__2026__ba7816bf8f01.pdf"
        );
    }

    #[test]
    fn clave_staging_inserta_batch_antes_del_archivo() {
        assert_eq!(
            clave_staging("a/b/c/Cedulas_de_cuotas/x.pdf", "B1"),
            "a/b/c/Cedulas_de_cuotas/.staging_B1/x.pdf"
        );
    }

    #[test]
    fn clave_final_reemplaza_separadores_en_nombre() {
        let clave = clave_final_cedula("CLI", "RP1", "REV", "x/y\\z.pdf");
        assert_eq!(clave, "CLI/RP1/REV/Cedulas_de_cuotas/x_y_z.pdf");
    }

    #[test]
    fn memoria_ciclo_staging_promocion_descarte() {
        let almacen = AlmacenMemoria::new();
        let final_ = "C/R/V/Cedulas_de_cuotas/C__R__MARZO__2026__ba7816bf8f01.pdf";

        let staging = almacen.subir_staging(final_, "B1", b"abc").unwrap();
        assert!(almacen.existe(&staging).unwrap());

        almacen.promover(&staging, final_).unwrap();
        assert!(almacen.existe(final_).unwrap());
        assert!(!almacen.existe(&staging).unwrap());

        // Segunda promoción: staging ya no existe, final sí -> Ok (idempotente)
        almacen.promover(&staging, final_).unwrap();

        almacen.descartar(final_).unwrap();
        assert!(!almacen.existe(final_).unwrap());
        // Descartar algo inexistente también es Ok
        almacen.descartar(final_).unwrap();
        assert!(almacen.claves().is_empty());
    }

    #[test]
    fn memoria_promover_sin_staging_ni_final_es_error() {
        let almacen = AlmacenMemoria::new();
        let resultado = almacen.promover("C/R/V/Cedulas_de_cuotas/.staging_X/a.pdf", "C/R/V/Cedulas_de_cuotas/a.pdf");
        assert!(resultado.is_err());
    }
}
