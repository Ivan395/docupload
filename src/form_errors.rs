//! Manejo global de errores en formularios y modales.
//!
//! - [`ErroresFormulario`] acumula TODOS los errores de un envío, uno por campo, para que la UI
//!   marque en rojo exactamente qué campos fallaron (ver `ui/form_field.slint`).
//! - [`describir_error`] traduce errores técnicos (PostgreSQL, red, RustFS) a la causa entendible
//!   para el usuario, conservando el detalle técnico.
//! - Las funciones `mostrar_*` publican los errores en `AppData` del modal abierto.

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{AppData, AppLogic, CampoErrorUi, MainWindow};

/// Errores de validación de un formulario, en el orden en que se detectaron.
/// Solo se guarda el primer error de cada campo (el más importante).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ErroresFormulario {
    campos: Vec<(String, String)>,
}

impl ErroresFormulario {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un error para `campo` si aún no tiene uno.
    pub fn agregar(&mut self, campo: &str, mensaje: impl Into<String>) {
        if !self.tiene(campo) {
            self.campos.push((campo.to_string(), mensaje.into()));
        }
    }

    /// Registra el error de un `Result` de validación. Devuelve true si era válido.
    pub fn validar<T>(&mut self, campo: &str, res: Result<T, String>) -> bool {
        match res {
            Ok(_) => true,
            Err(e) => {
                self.agregar(campo, e);
                false
            }
        }
    }

    /// Marca el campo como obligatorio. Un valor vacío o un placeholder de combo ("-- ...")
    /// cuenta como vacío. Devuelve true si el campo tiene valor.
    pub fn requerido(&mut self, campo: &str, valor: &str, etiqueta: &str) -> bool {
        if es_vacio(valor) {
            self.agregar(campo, format!("{} es obligatorio.", etiqueta));
            false
        } else {
            true
        }
    }

    pub fn tiene(&self, campo: &str) -> bool {
        self.campos.iter().any(|(c, _)| c == campo)
    }

    pub fn mensaje(&self, campo: &str) -> Option<&str> {
        self.campos.iter().find(|(c, _)| c == campo).map(|(_, m)| m.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.campos.is_empty()
    }

    pub fn len(&self) -> usize {
        self.campos.len()
    }

    pub fn campos(&self) -> &[(String, String)] {
        &self.campos
    }

    /// Texto general para el encabezado del modal.
    pub fn resumen(&self) -> String {
        resumen_para(self.campos.len())
    }
}

/// Resumen para `n` campos con error (vacío si no hay).
pub fn resumen_para(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => "Hay 1 campo con error. Corrígelo (está marcado en rojo) e intenta de nuevo.".to_string(),
        n => format!("Hay {} campos con error. Corrígelos (están marcados en rojo) e intenta de nuevo.", n),
    }
}

/// Valor vacío o placeholder de combo ("-- Seleccionar ... --").
pub fn es_vacio(valor: &str) -> bool {
    let t = valor.trim();
    t.is_empty() || t.starts_with("--")
}

/// Traduce un error técnico a una explicación entendible, conservando el detalle técnico.
pub fn describir_error(err: &str) -> String {
    let detalle = err.trim();
    match causa_conocida(detalle) {
        Some(causa) if !detalle.is_empty() => format!("{}\n\nDetalle técnico: {}", causa, detalle),
        Some(causa) => causa.to_string(),
        None if detalle.is_empty() => "Ocurrió un error desconocido.".to_string(),
        None => detalle.to_string(),
    }
}

fn causa_conocida(err: &str) -> Option<&'static str> {
    let l = err.to_lowercase();
    let tiene = |pats: &[&str]| pats.iter().any(|p| l.contains(p));

    if tiene(&["[23505]", "duplicate key", "llave duplicada", "unique constraint", "ya existe"]) {
        return Some("Ya existe un registro con esos mismos datos (valor duplicado). Revisa los datos únicos, como RFC, NSS, número de registro o folio.");
    }
    if tiene(&["[23503]", "foreign key", "llave foránea"]) {
        return Some("El registro está relacionado con otra información (por ejemplo cédulas, nóminas u obras), por eso no se puede guardar o eliminar así.");
    }
    if tiene(&["[23502]", "null value in column", "not-null"]) {
        return Some("Falta un dato obligatorio que la base de datos requiere.");
    }
    if tiene(&["[22001]", "value too long", "demasiado largo"]) {
        return Some("Uno de los valores es más largo de lo permitido.");
    }
    if tiene(&["[22p02]", "[22007]", "[22008]", "invalid input syntax", "sintaxis de entrada no válida"]) {
        return Some("Uno de los valores tiene un formato que no es válido (número, fecha o identificador).");
    }
    if tiene(&["[23514]", "check constraint"]) {
        return Some("Uno de los valores está fuera del rango permitido por las reglas del sistema.");
    }
    if tiene(&["[40p01]", "deadlock"]) {
        return Some("Otra operación estaba modificando los mismos datos al mismo tiempo. Intenta de nuevo en unos segundos.");
    }
    if tiene(&["[42501]", "permission denied", "permiso denegado"]) {
        return Some("El usuario de la base de datos no tiene permisos para esta operación.");
    }
    if tiene(&[
        "[08",
        "[57p",
        "connection refused",
        "could not connect",
        "error de conexión",
        "connection reset",
        "connection closed",
        "broken pipe",
        "timed out",
        "timeout",
        "tiempo de espera",
        "no se puede conectar",
        "sin conexión",
        "error connecting",
    ]) {
        return Some("No hay conexión con el servidor. Verifica que la base de datos o el almacenamiento estén encendidos y que la red funcione, y vuelve a intentar.");
    }
    if tiene(&["dns", "tls", "certificate", "certificado", "network", "red no disponible"]) {
        return Some("Hubo un problema de red al comunicarse con un servicio externo. Revisa tu conexión a internet.");
    }
    None
}

/// Modelo de errores por campo para `AppData.form_errors`.
pub fn modelo(errs: &ErroresFormulario) -> ModelRc<CampoErrorUi> {
    let filas: Vec<CampoErrorUi> = errs
        .campos()
        .iter()
        .map(|(c, m)| CampoErrorUi { campo: c.as_str().into(), mensaje: m.as_str().into() })
        .collect();
    ModelRc::new(VecModel::from(filas))
}

/// Publica los errores de validación en el modal abierto (campos en rojo + resumen arriba).
pub fn mostrar_errores(ui: &MainWindow, errs: &ErroresFormulario) {
    let app = ui.global::<AppData>();
    app.set_form_errors(modelo(errs));
    app.set_form_error_titulo("Revisa los datos del formulario".into());
    app.set_form_error_general(errs.resumen().into());
}

/// Marca un solo campo con error (y el resumen general).
pub fn mostrar_error_campo(ui: &MainWindow, campo: &str, mensaje: &str) {
    let mut errs = ErroresFormulario::new();
    errs.agregar(campo, mensaje);
    mostrar_errores(ui, &errs);
}

/// Publica un error general (servidor, red, regla de negocio) en el modal abierto.
pub fn mostrar_error_general(ui: &MainWindow, titulo: &str, err: &str) {
    let app = ui.global::<AppData>();
    app.set_form_error_titulo(titulo.into());
    app.set_form_error_general(describir_error(err).into());
}

/// Error fuera de un modal (por ejemplo un guardado en segundo plano que falló después de cerrar
/// el modal): se muestra en el modal global de error del sistema, que no desaparece solo.
pub fn mostrar_error_sistema(ui: &MainWindow, titulo: &str, detalle: &str) {
    let app = ui.global::<AppData>();
    app.set_system_error_modal_title(titulo.into());
    app.set_system_error_modal_detail(detalle.into());
    app.set_system_error_modal_generico(true);
    app.set_show_system_error_modal(true);
}

/// Limpia todos los errores del formulario.
pub fn limpiar(ui: &MainWindow) {
    let app = ui.global::<AppData>();
    app.set_form_errors(ModelRc::new(VecModel::from(Vec::<CampoErrorUi>::new())));
    app.set_form_error_titulo("".into());
    app.set_form_error_general("".into());
}

/// Conecta los callbacks de búsqueda y limpieza de errores por campo.
pub fn wire_callbacks(main_window: &MainWindow) {
    let app_logic = main_window.global::<AppLogic>();

    app_logic.on_campo_error(|errores, campo| {
        errores
            .iter()
            .find(|e| e.campo == campo)
            .map(|e| e.mensaje)
            .unwrap_or_default()
    });

    let ui_weak = main_window.as_weak();
    app_logic.on_limpiar_error_campo(move |campo| {
        let Some(ui) = ui_weak.upgrade() else { return };
        let app = ui.global::<AppData>();
        let actuales = app.get_form_errors();
        if !actuales.iter().any(|e| e.campo == campo) {
            return;
        }
        let restantes: Vec<CampoErrorUi> = actuales.iter().filter(|e| e.campo != campo).collect();
        let n = restantes.len();
        app.set_form_errors(ModelRc::new(VecModel::from(restantes)));
        // Si el aviso general era el resumen de validación, se actualiza o desaparece
        if app.get_form_error_titulo() == "Revisa los datos del formulario" {
            if n == 0 {
                app.set_form_error_titulo("".into());
            }
            app.set_form_error_general(resumen_para(n).into());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acumula_un_error_por_campo() {
        let mut e = ErroresFormulario::new();
        e.agregar("rfc", "Formato inválido");
        e.agregar("rfc", "Otro error");
        e.requerido("nombre", "  ", "El nombre");
        e.requerido("tipo", "-- Seleccionar --", "El tipo");
        assert!(e.requerido("folio", "F-1", "El folio"));
        assert_eq!(e.len(), 3);
        assert_eq!(e.mensaje("rfc"), Some("Formato inválido"));
        assert_eq!(e.mensaje("nombre"), Some("El nombre es obligatorio."));
        assert!(e.resumen().contains("3 campos"));
    }

    #[test]
    fn validar_registra_err() {
        let mut e = ErroresFormulario::new();
        assert!(e.validar::<()>("a", Ok(())));
        assert!(!e.validar::<()>("b", Err("mal".into())));
        assert_eq!(e.mensaje("b"), Some("mal"));
    }

    #[test]
    fn describe_errores_conocidos() {
        let dup = describir_error("[23505] duplicate key value violates unique constraint \"uq_x\"");
        assert!(dup.starts_with("Ya existe un registro"));
        assert!(dup.contains("Detalle técnico: [23505]"));
        assert!(describir_error("Error de conexión con la base de datos: timed out").starts_with("No hay conexión"));
        assert!(describir_error("[23503] update or delete violates foreign key").starts_with("El registro está relacionado"));
        assert_eq!(describir_error("Mensaje propio de negocio"), "Mensaje propio de negocio");
        assert_eq!(describir_error(""), "Ocurrió un error desconocido.");
    }
}
