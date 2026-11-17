//! Ejecución de operaciones largas en segundo plano con capa global de avance.
//!
//! Las operaciones de BD (revertir, sincronizar, asignaciones) no deben correr en el hilo de la
//! interfaz: lo congelan y ningún indicador de "procesando" llega a pintarse. Este módulo activa la
//! capa `OperacionEnCursoOverlay` (AppData.operacion_en_curso), ejecuta el trabajo en otro hilo y
//! vuelve al hilo de la interfaz para aplicar el resultado y apagar la capa.

use slint::ComponentHandle;

use crate::{AppData, MainWindow};

/// Ejecuta `trabajo` en un hilo de fondo mostrando la capa de avance con `titulo` y `detalle`.
/// Al terminar, `al_terminar` se ejecuta en el hilo de la interfaz con el resultado y la capa se oculta.
/// Si ya hay una operación en curso, no inicia otra (evita dobles clics y operaciones simultáneas).
pub fn ejecutar_con_avance<T, W, D>(ui: &MainWindow, titulo: &str, detalle: &str, trabajo: W, al_terminar: D)
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    D: FnOnce(&MainWindow, T) + Send + 'static,
{
    let app_data = ui.global::<AppData>();
    if app_data.get_operacion_en_curso() {
        crate::show_toast_message(ui, "Espera a que termine la operación en curso.");
        return;
    }
    app_data.set_operacion_titulo(titulo.into());
    app_data.set_operacion_detalle(detalle.into());
    app_data.set_operacion_en_curso(true);

    let ui_weak = ui.as_weak();
    let lanzado = std::thread::Builder::new()
        .name("operacion_en_curso".into())
        .spawn(move || {
            let resultado = trabajo();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    al_terminar(&ui, resultado);
                    ui.global::<AppData>().set_operacion_en_curso(false);
                }
            });
        });
    if let Err(e) = lanzado {
        app_data.set_operacion_en_curso(false);
        crate::show_toast_message(ui, &format!("No se pudo iniciar la operación: {e}"));
    }
}

/// Texto de ayuda de atajos según el sistema operativo (en macOS, Control equivale a ⌘ Command).
pub fn texto_atajos() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘F buscar · ⌘Z deshacer · ⌘⇧A asistente · Esc limpiar"
    } else {
        "Ctrl+F buscar · Ctrl+Z deshacer · Ctrl+Shift+A asistente · Esc limpiar"
    }
}

/// Texto de ayuda de atajos de la vista de Reporte de Liquidaciones (⌘ en macOS, Ctrl en Windows/Linux).
pub fn texto_atajos_reporte() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘F buscar · ⌘⇧A asistente · Esc cerrar/limpiar"
    } else {
        "Ctrl+F buscar · Ctrl+Shift+A asistente · Esc cerrar/limpiar"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texto_atajos_reporte_coincide_con_el_sistema() {
        let t = texto_atajos_reporte();
        if cfg!(target_os = "macos") {
            assert!(t.contains('⌘'));
        } else {
            assert!(t.contains("Ctrl+F") && t.contains("Ctrl+Shift+A"));
        }
    }

    #[test]
    fn texto_atajos_coincide_con_el_sistema() {
        let t = texto_atajos();
        if cfg!(target_os = "macos") {
            assert!(t.contains('⌘'));
        } else {
            assert!(t.contains("Ctrl+F") && t.contains("Ctrl+Shift+A"));
        }
    }
}
