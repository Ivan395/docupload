//! Capturas de QA visual de los modales con errores por campo, la tarjeta de revisión aprobada,
//! la revisión de discrepancias antes de cargar y los posibles duplicados. Datos FALSOS, sin BD.
//!
//! Ejecutar:
//!   cargo test captura_errores_modales -- --ignored --exact --nocapture
//! Variables de entorno: CAPTURA_DIR (por defecto /tmp/capturas_ui)

use crate::advertencias_carga::{self as adv, CampoAdvertencia};
use crate::form_errors::{self, ErroresFormulario};
use crate::*;
use slint::{ComponentHandle, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::cell::{Cell, RefCell};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

const INTERVALO: Duration = Duration::from_millis(450);

type Accion = Box<dyn Fn(&MainWindow)>;

fn revision(nombre: &str, aprobada: bool) -> RevisionData {
    RevisionData {
        id: format!("rev-{nombre}").into(),
        nombre: nombre.into(),
        folio: "REV-IMSS-2025-0012".into(),
        concepto_diferencia: "Diferencias en SBC".into(),
        estatus_imss: if aprobada { "APROBADA POR IMSS".into() } else { "EN ESTUDIO".into() },
        estatus: "EN PROCESO".into(),
        fecha_entrega_imss: "03/02/2025".into(),
        fecha_aprobacion_imss: if aprobada { "20/02/2025".into() } else { "-- / -- / ----".into() },
        fecha_maxima_aprobacion_imss: "24/02/2025".into(),
        fecha_maxima_entrega_revision: "18/04/2025".into(),
        fecha_maxima_entrega_imss: "02/06/2025".into(),
        fecha_entrega_revision: "-- / -- / ----".into(),
        aprobada_imss: aprobada,
        ..Default::default()
    }
}

fn advertencias_falsas() -> (Vec<String>, Vec<adv::AdvertenciaCarga>) {
    let regs = vec![
        adv::RegistroTrabajador { archivo: "2024-01_cedula.pdf".into(), nss: Some("12345678901".into()), nombre: Some("JUAN PEREZ LOPEZ".into()), rfc: Some("PELJ800101AB1".into()), curp: None },
        adv::RegistroTrabajador { archivo: "2024-02_cedula.pdf".into(), nss: Some("12345678901".into()), nombre: Some("JUAN PEREZ LOPEZ".into()), rfc: Some("PELJ800101AB1".into()), curp: None },
        adv::RegistroTrabajador { archivo: "2024-02_cedula.pdf".into(), nss: Some("32998253382".into()), nombre: Some("ANA RUIZ".into()), rfc: None, curp: None },
        adv::RegistroTrabajador { archivo: "2024-03_cedula.pdf".into(), nss: Some("32998253382".into()), nombre: Some("ANA RUlZ 2".into()), rfc: None, curp: None },
    ];
    let mut v = adv::detectar_trabajadores(&regs, &adv::CatalogoEmpleados::default());
    v.push(adv::advertencia_patron(
        CampoAdvertencia::RegistroPatronal,
        "2024-03_cedula.pdf",
        "Y5839210199",
        "Y5839210104",
        "El Registro Patronal de esta cédula (Y5839210199) no es el de la revisión (Y5839210104).".into(),
        true,
    ));
    let archivos = vec!["2024-01_cedula.pdf".into(), "2024-02_cedula.pdf".into(), "2024-03_cedula.pdf".into(), "2024-04_cedula.pdf".into()];
    (archivos, adv::numerar(v))
}

fn pasos() -> Vec<(&'static str, Accion)> {
    let mut v: Vec<(&'static str, Accion)> = Vec::new();
    v.push(("01_cliente_nuevo_vacio", Box::new(|w: &MainWindow| {
        let a = w.global::<AppData>();
        a.set_current_view(1);
        a.set_show_new_cliente_modal(true);
    })));
    v.push(("02_cliente_nuevo_errores", Box::new(|w: &MainWindow| {
        let a = w.global::<AppData>();
        a.set_new_cliente_rfc("ABC12".into());
        a.set_new_cliente_curp("PELJ".into());
        let e = validation::validar_campos_cliente("", "ABC12", "PELJ", None);
        form_errors::mostrar_errores(w, &e);
    })));
    v.push(("03_cliente_error_servidor", Box::new(|w: &MainWindow| {
        form_errors::limpiar(w);
        form_errors::mostrar_error_general(w, "No se pudo registrar el cliente", "[23505] duplicate key value violates unique constraint \"uq_clientes_rfc\"");
    })));
    v.push(("04_revision_modal_errores", Box::new(|w: &MainWindow| {
        let a = w.global::<AppData>();
        a.set_show_new_cliente_modal(false);
        a.set_current_view(3);
        a.set_show_revision_modal(true);
    })));
    v.push(("05_revision_modal_errores_b", Box::new(|w: &MainWindow| {
        let mut e = ErroresFormulario::new();
        e.agregar("nombre", "El nombre de la revisión es obligatorio.");
        e.agregar("periodo_fin", "Captura también el fin del periodo (o deja ambos vacíos).");
        e.agregar("f_ent_imss", "Formato inválido: usa DD/MM/AAAA.");
        form_errors::mostrar_errores(w, &e);
    })));
    v.push(("06_revisiones_aprobada_vs_no", Box::new(|w: &MainWindow| {
        let a = w.global::<AppData>();
        a.set_show_revision_modal(false);
        a.set_selected_rp_id("rp".into());
        a.set_selected_rp_registro("Y5839210104".into());
        a.set_revisiones_list(ModelRc::new(VecModel::from(vec![revision("Revisión aprobada", true), revision("Revisión en estudio", false)])));
    })));
    v.push(("07_carga_advertencias", Box::new(|w: &MainWindow| {
        let (archivos, advs) = advertencias_falsas();
        let a = w.global::<AppData>();
        a.set_carga_adv_titulo("Revisa las cédulas de cuotas antes de cargarlas".into());
        let archivos_ui: Vec<CargaArchivoUi> = archivos
            .iter()
            .map(|f| {
                let n = advs.iter().filter(|x| x.archivos.contains(f)).count() as i32;
                let g = advs.iter().filter(|x| x.archivos.contains(f) && x.grave).count() as i32;
                CargaArchivoUi { nombre: f.as_str().into(), incluir: f != "2024-03_cedula.pdf", advertencias: n, graves: g }
            })
            .collect();
        let items: Vec<CargaAdvertenciaUi> = advs
            .iter()
            .enumerate()
            .map(|(i, x)| CargaAdvertenciaUi {
                id: x.id as i32,
                campo: x.campo.etiqueta().into(),
                del_patron: x.campo.es_del_patron(),
                trabajador: x.trabajador.as_ref().map(|t| t.descripcion()).unwrap_or_default().into(),
                archivos: x.archivos.iter().cloned().collect::<Vec<_>>().join(", ").into(),
                n_archivos: x.archivos.len() as i32,
                encontrado: x.encontrado.as_str().into(),
                sugerido: x.sugerido.clone().unwrap_or_default().into(),
                origen: x.origen_sugerido.as_str().into(),
                mensaje: x.mensaje.as_str().into(),
                grave: x.grave,
                accion: if i == 1 { 2 } else if i == 2 { 1 } else { 0 },
                correccion: if i == 1 { "1234567890".into() } else { "".into() },
                error: if i == 1 { "El NSS alterno debe tener 11 dígitos; se recibió '1234567890'.".into() } else { "".into() },
                archivo_incluido: !x.archivos.iter().all(|f| f == "2024-03_cedula.pdf"),
                solo_decidir_archivo: false,
            })
            .collect();
        a.set_carga_adv_archivos(ModelRc::new(VecModel::from(archivos_ui)));
        a.set_carga_adv_items(ModelRc::new(VecModel::from(items)));
        a.set_carga_adv_n_total(4);
        a.set_carga_adv_n_incluidos(3);
        a.set_carga_adv_n_advertencias(advs.len() as i32);
        a.set_carga_adv_n_graves(1);
        a.set_show_carga_adv_modal(true);
    })));
    v.push(("08_duplicados", Box::new(|w: &MainWindow| {
        let a = w.global::<AppData>();
        a.set_show_carga_adv_modal(false);
        let par = |n1: &str, n2: &str, motivo: &str| ParDuplicadoUi {
            a_id: "a".into(),
            a_nombre: n1.into(),
            a_nss: "12345678903".into(),
            a_rfc: "PELJ800101AB1".into(),
            a_curp: "PELJ800101HDFRPN09".into(),
            a_liq: 12,
            a_nom: 24,
            b_id: "b".into(),
            b_nombre: n2.into(),
            b_nss: "12345678901".into(),
            b_rfc: "".into(),
            b_curp: "PELJ800101HDFRPN09".into(),
            b_liq: 1,
            b_nom: 0,
            motivo: motivo.into(),
        };
        a.set_duplicados_cliente_nombre("Constructora del Centro".into());
        a.set_duplicados_items(ModelRc::new(VecModel::from(vec![
            par("JUAN PEREZ LOPEZ", "JUAN PERES LOPEZ", "Misma CURP"),
            par("ANA RUIZ", "RUIZ ANA", "Mismo nombre"),
        ])));
        a.set_duplicados_confirmar_idx(0);
        a.set_duplicados_confirmar_conservar_a(true);
        a.set_show_duplicados_modal(true);
    })));
    v.push(("09_aviso_duplicados_y_error_sistema", Box::new(|w: &MainWindow| {
        let a = w.global::<AppData>();
        a.set_show_duplicados_modal(false);
        a.set_duplicados_pendientes(2);
        a.set_duplicados_aviso_visible(true);
        form_errors::mostrar_error_sistema(
            w,
            "No se pudo eliminar el cliente",
            &form_errors::describir_error("[23503] update or delete on table \"clientes\" violates foreign key constraint"),
        );
    })));
    v
}

fn guardar_ppm(dir: &Path, nombre: &str, buf: &SharedPixelBuffer<Rgba8Pixel>) -> std::io::Result<PathBuf> {
    let ruta = dir.join(format!("{nombre}.ppm"));
    let mut f = BufWriter::new(File::create(&ruta)?);
    write!(f, "P6\n{} {}\n255\n", buf.width(), buf.height())?;
    for px in buf.as_slice() {
        f.write_all(&[px.r, px.g, px.b])?;
    }
    f.flush()?;
    Ok(ruta)
}

#[test]
#[ignore]
fn captura_errores_modales() {
    let dir = PathBuf::from(std::env::var("CAPTURA_DIR").unwrap_or_else(|_| "/tmp/capturas_ui".to_string()));
    fs::create_dir_all(&dir).expect("no se pudo crear CAPTURA_DIR");

    let win = MainWindow::new().expect("no se pudo crear MainWindow");
    form_errors::wire_callbacks(&win);
    win.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
    win.show().expect("no se pudo mostrar la ventana");

    let pasos = Rc::new(pasos());
    let indice = Rc::new(Cell::new(0usize));
    let ppms = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
    let timer = slint::Timer::default();
    {
        let w = win.as_weak();
        let pasos = pasos.clone();
        let indice = indice.clone();
        let ppms = ppms.clone();
        let dir = dir.clone();
        timer.start(slint::TimerMode::Repeated, INTERVALO, move || {
            let Some(win) = w.upgrade() else {
                let _ = slint::quit_event_loop();
                return;
            };
            let i = indice.get();
            if i > 0 {
                if let Ok(buf) = win.window().take_snapshot() {
                    if let Ok(r) = guardar_ppm(&dir, pasos[i - 1].0, &buf) {
                        ppms.borrow_mut().push(r);
                    }
                }
            }
            if i >= pasos.len() {
                let _ = slint::quit_event_loop();
                return;
            }
            (pasos[i].1)(&win);
            win.window().request_redraw();
            indice.set(i + 1);
        });
    }
    slint::run_event_loop().expect("event loop");
    drop(timer);
    for ppm in ppms.borrow().iter() {
        let png = ppm.with_extension("png");
        if Command::new("magick").arg(ppm).arg(&png).status().map(|s| s.success()).unwrap_or(false) {
            let _ = fs::remove_file(ppm);
        }
    }
    println!("Capturas en {}: {}", dir.display(), ppms.borrow().len());
    assert_eq!(ppms.borrow().len(), pasos.len());
}
