//! Harness de capturas de pantalla para QA visual de la vista "Reporte de Liquidaciones".
//!
//! Abre el `MainWindow` real (mismo pipeline winit/femtovg que usa la app), llena `AppData`
//! con datos FALSOS (sin base de datos), recorre varios estados de la vista y guarda cada
//! uno como imagen para que un revisor los inspeccione.
//!
//! Ejecutar:
//!   cargo test captura_reporte_liquidaciones -- --ignored --exact --nocapture
//!
//! Variables de entorno:
//!   CAPTURA_DIR  directorio de salida (por defecto /tmp/capturas_ui)
//!
//! Formato: `slint::Window::take_snapshot()` entrega RGBA; como no hay crate `png` disponible
//! sin tocar Cargo.toml, se escribe PPM (P6) y, si existe `magick`, se convierte a PNG.

use crate::*;
use slint::{Color, ComponentHandle, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel, ModelRc};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

/// Tamaños para los que se capturan los estados con sidebar abierto y cerrado.
const TAMANOS_SIDEBAR: [(u32, u32); 5] = [(1024, 640), (1280, 720), (1366, 768), (1440, 900), (1920, 1080)];
/// Tamaños para los que se capturan los modales (sidebar abierto).
const TAMANOS_MODAL: [(u32, u32); 2] = [(1280, 720), (1920, 1080)];
/// Tamaño de las capturas de pestañas.
const TAMANO_TABS: (u32, u32) = (1366, 768);
/// Espera entre cambio de estado y captura, para que el layout y las animaciones se asienten.
const INTERVALO: Duration = Duration::from_millis(350);

/// Meses del periodo de datos falsos: (año, mes, nombre).
const PERIODOS: [(i32, u32, &str); 7] = [
    (2014, 1, "Enero"),
    (2014, 2, "Febrero"),
    (2014, 3, "Marzo"),
    (2014, 4, "Abril"),
    (2014, 5, "Mayo"),
    (2015, 7, "Julio"),
    (2015, 8, "Agosto"),
];

/// Obras ficticias: (id, número, nombre, vigencia).
const OBRAS: [(&str, &str, &str, &str); 3] = [
    ("11111111-1111-4111-8111-000000000101", "OB-101", "Torre Norte Reforma", "01/03/2014 al 31/12/2014"),
    ("11111111-1111-4111-8111-000000000102", "OB-102", "Residencial Los Pinos", "01/01/2014 al 30/06/2015"),
    ("11111111-1111-4111-8111-000000000103", "OB-103", "Bodega Industrial Sur", "15/07/2015 al 31/12/2015"),
];

/// Trabajadores ficticios: (nombre, NSS, RFC, CURP).
const EMPLEADOS: [(&str, &str, &str, &str); 10] = [
    ("GARCÍA LÓPEZ MARÍA ELENA", "41068512345", "GALM850612ABC", "GALM850612MDFRPR09"),
    ("HERNÁNDEZ RUIZ JOSÉ LUIS", "52017898712", "HERJ780304KL1", "HERJ780304HDFRZS05"),
    ("MARTÍNEZ SOTO ANA KAREN", "03049023456", "MASA900815QW7", "MASA900815MJCRTN01"),
    ("RAMÍREZ CRUZ CARLOS IVÁN", "79108834567", "RACC820419TR2", "RACC820419HMCMRV08"),
    ("TORRES VEGA LUCÍA", "15058245678", "TOVL010725HX9", "TOVL010725MGTRGC02"),
    ("FLORES MENDOZA JUAN PABLO", "60117356789", "FOMJ761130PA4", "FOMJ761130HNLLNN17"),
    ("SÁNCHEZ ORTIZ ROSA ISELA", "28039167890", "SAOR891222ZE8", "SAOR891222MDFNRS06"),
    ("DOMÍNGUEZ REYES MIGUEL ÁNGEL", "44126078901", "DORM700503LB3", "DORM700503HVZMYG14"),
    ("VÁZQUEZ MORALES PATRICIA", "97016489012", "VAMP950617CT5", "VAMP950617MQTZRT03"),
    ("CASTILLO GUTIÉRREZ ERNESTO", "11095790123", "CAGE830921MU6", "CAGE830921HCHSTR10"),
];

/// Estados modales que se capturan (cada uno corresponde a un flag `show_*` de AppData).
#[derive(Clone, Copy)]
enum Modal {
    Edit,
    BatchEdit,
    Emp,
    ExportSua,
    Historial,
    ConfirmarRevertir,
}

impl Modal {
    /// Nombre del flag `show_*` en AppData (también se usa en el nombre del archivo).
    fn flag(self) -> &'static str {
        match self {
            Modal::Edit => "show_reporte_edit_modal",
            Modal::BatchEdit => "show_reporte_batch_edit_modal",
            Modal::Emp => "show_reporte_emp_modal",
            Modal::ExportSua => "show_reporte_export_sua_modal",
            Modal::Historial => "show_entregable_historial",
            Modal::ConfirmarRevertir => "show_reporte_confirmar_revertir",
        }
    }
}

/// Un estado a capturar: tamaño de ventana, sidebar, pestaña y modal opcional.
struct Paso {
    nombre: String,
    ancho: u32,
    alto: u32,
    sidebar: bool,
    tab: i32,
    modal: Option<Modal>,
}

fn construir_pasos() -> Vec<Paso> {
    let mut pasos = Vec::new();

    for (w, h) in TAMANOS_SIDEBAR {
        for sidebar in [true, false] {
            let sufijo = if sidebar { "sidebar" } else { "sin_sidebar" };
            pasos.push(Paso {
                nombre: format!("reporte_{w}x{h}_{sufijo}"),
                ancho: w,
                alto: h,
                sidebar,
                tab: 0,
                modal: None,
            });
        }
    }

    let modales = [
        Modal::Edit,
        Modal::BatchEdit,
        Modal::Emp,
        Modal::ExportSua,
        Modal::Historial,
        Modal::ConfirmarRevertir,
    ];
    for (w, h) in TAMANOS_MODAL {
        for m in modales {
            pasos.push(Paso {
                nombre: format!("modal_{}_{w}x{h}", m.flag()),
                ancho: w,
                alto: h,
                sidebar: true,
                tab: 0,
                modal: Some(m),
            });
        }
    }

    // Pestañas de reporte_tab_idx: 1 = Asignación por empleado, 2 = Asignación por obra.
    let (w, h) = TAMANO_TABS;
    for tab in [1, 2] {
        pasos.push(Paso {
            nombre: format!("tab{tab}_{w}x{h}"),
            ancho: w,
            alto: h,
            sidebar: true,
            tab,
            modal: None,
        });
    }

    pasos
}

/// Aplica un estado: apaga todos los modales, configura sidebar/pestaña y enciende el modal pedido.
fn aplicar_paso(app: &AppData, p: &Paso) {
    app.set_show_reporte_edit_modal(false);
    app.set_show_reporte_batch_edit_modal(false);
    app.set_show_reporte_emp_modal(false);
    app.set_show_reporte_export_sua_modal(false);
    app.set_show_entregable_historial(false);
    app.set_show_reporte_confirmar_revertir(false);

    app.set_is_sidebar_open(p.sidebar);
    app.set_reporte_tab_idx(p.tab);

    match p.modal {
        Some(Modal::Edit) => app.set_show_reporte_edit_modal(true),
        Some(Modal::BatchEdit) => app.set_show_reporte_batch_edit_modal(true),
        Some(Modal::Emp) => {
            app.set_reporte_filtro_modo_general(true);
            app.set_show_reporte_emp_modal(true);
        }
        Some(Modal::ExportSua) => app.set_show_reporte_export_sua_modal(true),
        Some(Modal::Historial) => app.set_show_entregable_historial(true),
        Some(Modal::ConfirmarRevertir) => app.set_show_reporte_confirmar_revertir(true),
        None => {}
    }
}

// ─── Datos falsos ───────────────────────────────────────────────────────────

/// Formato de moneda MXN con separador de miles (solo para datos ficticios).
fn moneda(v: f64) -> String {
    let s = format!("{:.2}", v.abs());
    let (entero, decimal) = s.split_once('.').unwrap_or((s.as_str(), "00"));
    let mut con_comas = String::new();
    for (i, c) in entero.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            con_comas.push(',');
        }
        con_comas.push(c);
    }
    let entero: String = con_comas.chars().rev().collect();
    let signo = if v < 0.0 { "-" } else { "" };
    format!("{signo}${entero}.{decimal}")
}

/// Días con formato corto; cero se muestra como guion (como en la vista real).
fn fmt_dias(v: f32) -> SharedString {
    if v > 0.001 {
        format!("{:.0}", v).into()
    } else {
        "-".into()
    }
}

/// Reparte un subtotal entre los 14 conceptos de la liquidación (valores ficticios).
/// Índices: 0 cf, 1 exc_pat, 2 exc_obr, 3 pd_pat, 4 pd_obr, 5 gmp_pat, 6 gmp_obr,
/// 7 rt, 8 iv_pat, 9 iv_obr, 10 gps, 11 patronal, 12 obrera, 13 subtotal.
fn desglose(sub: f64) -> [f64; 14] {
    [
        sub * 0.35, sub * 0.05, sub * 0.02, sub * 0.08, sub * 0.05, sub * 0.10, sub * 0.03,
        sub * 0.12, sub * 0.10, sub * 0.05, sub * 0.05, sub * 0.60, sub * 0.40, sub,
    ]
}

/// Colores de fondo, hover y acento por registro de obra (índice 0..3).
fn colores_obra(k: usize) -> (Color, Color, Color) {
    match k {
        0 => (Color::from_rgb_u8(239, 246, 255), Color::from_rgb_u8(219, 234, 254), Color::from_rgb_u8(37, 99, 235)),
        1 => (Color::from_rgb_u8(236, 253, 245), Color::from_rgb_u8(209, 250, 229), Color::from_rgb_u8(16, 185, 129)),
        _ => (Color::from_rgb_u8(255, 251, 235), Color::from_rgb_u8(254, 243, 199), Color::from_rgb_u8(245, 158, 11)),
    }
}

/// Carga en `AppData` todo lo que lee la vista 7 (sesión, tabla, KPIs, filtros, modales y pestañas).
fn cargar_datos_falsos(app: &AppData) {
    // ── Sesión: cliente / RP / revisión seleccionados y accesos (habilitan la vista 7 en el sidebar)
    app.set_selected_cliente_id("CLI-0001".into());
    app.set_selected_cliente_nombre("Constructora Demo del Norte SA de CV".into());
    app.set_selected_cliente_rfc("CDN150320AB7".into());
    app.set_selected_cliente_has_data(true);
    app.set_selected_rp_id("RP-0001".into());
    app.set_selected_rp_registro("A4455667788".into());
    app.set_selected_rp_delegacion("Nuevo León".into());
    app.set_selected_revision_id("REV-0001".into());
    app.set_selected_revision_nombre("Revisión Bimestral 2014-2015".into());
    app.set_selected_revision_folio("F-2026-014".into());
    app.set_current_view(7);
    app.set_reporte_liquidaciones_access_allowed(true);
    app.set_revision_has_liquidaciones(true);
    app.set_revision_has_cedulas(true);
    app.set_revision_has_obras(true);
    app.set_revision_has_nominas(true);
    app.set_revision_has_all_data(true);
    app.set_rp_has_cedulas(true);
    app.set_reportes_liquidaciones_data_exists(true);
    app.set_reporte_has_missing_cedulas_warning(false);
    app.set_reporte_sin_ro(false);

    // ── Obras (combos de filtro, edición y lote)
    let obras_item: Vec<ReporteObraItem> = OBRAS
        .iter()
        .map(|(id, num, nombre, _)| ReporteObraItem {
            id: (*id).into(),
            numero: (*num).into(),
            label: format!("{num} · {nombre}").into(),
        })
        .collect();
    let mut filtro_labels: Vec<SharedString> = vec!["Todas las obras".into()];
    let mut batch_labels: Vec<SharedString> = vec!["-- Seleccionar Obra --".into()];
    for o in &obras_item {
        filtro_labels.push(o.label.clone());
        batch_labels.push(o.label.clone());
    }
    app.set_reporte_obras_options(ModelRc::new(VecModel::from(obras_item.clone())));
    app.set_reporte_picker_filtered_obras(ModelRc::new(VecModel::from(obras_item.clone())));
    app.set_reporte_obras_filter_labels(ModelRc::new(VecModel::from(filtro_labels)));
    app.set_reporte_obras_batch_labels(ModelRc::new(VecModel::from(batch_labels)));
    app.set_reporte_edit_filtered_obras(ModelRc::new(VecModel::from(obras_item.clone())));
    app.set_reporte_batch_edit_filtered_obras(ModelRc::new(VecModel::from(obras_item.clone())));
    app.set_reporte_add_filtered_obras(ModelRc::new(VecModel::from(obras_item)));

    let obras_usuario: Vec<ReporteObraUsuarioItem> = OBRAS
        .iter()
        .enumerate()
        .map(|(k, (id, num, _, vig))| {
            let (ini, fin) = vig.split_once(" al ").unwrap_or(("--/--/----", "--/--/----"));
            ReporteObraUsuarioItem {
                id: (*id).into(),
                numero: (*num).into(),
                tipo_obra: "PRIVADA".into(),
                fecha_inicio: ini.into(),
                fecha_fin: fin.into(),
                vigencia_str: (*vig).into(),
                empleados_count: 4 + k as i32,
                empleados_count_str: format!("{} emps. únicos", 4 + k).into(),
                selected: k == 0,
            }
        })
        .collect();
    app.set_reporte_has_obras_usuario(true);
    app.set_reporte_obras_usuario(ModelRc::new(VecModel::from(obras_usuario)));
    app.set_reporte_drawer_obras_open(false);

    // ── Filas de la tabla (25 registros, 7 periodos, 3 obras, 1 baja y algunas sin asignar)
    let mut filas: Vec<ReporteLiquidacionRow> = Vec::new();
    let mut sum_orig = [0f64; 14];
    let mut sum_cur = [0f64; 14];
    let (mut tot_ced, mut tot_obr) = (0i32, 0i32);
    let (mut tot_lic, mut tot_inc) = (0f32, 0f32);

    // Acumuladores por periodo: (NSS distintos, días cédula, días obra, montos actuales)
    let mut por_periodo: Vec<(HashSet<&str>, i32, i32, [f64; 14], usize)> =
        PERIODOS.iter().map(|_| (HashSet::new(), 0, 0, [0f64; 14], 0usize)).collect();
    // Acumuladores por obra: (NSS distintos, GPS actual)
    let mut por_obra: Vec<(HashSet<&str>, f64)> = OBRAS.iter().map(|_| (HashSet::new(), 0f64)).collect();

    for i in 0..25usize {
        let (nombre, nss, rfc, curp) = EMPLEADOS[i % 10];
        let p_idx = (i * 3) % 7;
        let (anio, mes, mes_nombre) = PERIODOS[p_idx];
        let baja = i == 13;
        let obra = if i % 6 == 5 || baja { None } else { Some(i % 3) };
        let asignada = obra.is_some();
        let dias_ced = 30 + (i % 2) as i32;
        let dias_obra = if asignada { dias_ced - (i % 4) as i32 * 5 } else { 0 };
        let sub_orig = 1200.0 + i as f64 * 213.47 + (i % 10) as f64 * 11.0;
        let sub = if baja {
            0.0
        } else if asignada {
            sub_orig * dias_obra as f64 / dias_ced as f64
        } else {
            sub_orig
        };
        let lic: f32 = if i % 5 == 0 { 2.0 } else { 0.0 };
        let inc: f32 = if i % 7 == 0 { 1.0 } else { 0.0 };
        let c_orig = desglose(sub_orig);
        let c_cur = desglose(sub);
        let (bg, hover, accent) = match obra {
            Some(k) => colores_obra(k),
            None => (Color::from_argb_u8(255, 255, 255, 255), Color::from_argb_u8(255, 248, 250, 252), Color::from_argb_u8(0, 0, 0, 0)),
        };
        let (obra_id, obra_num, obra_label, vinc): (SharedString, SharedString, SharedString, SharedString) = match obra {
            Some(k) => (
                OBRAS[k].0.into(),
                OBRAS[k].1.into(),
                format!("{} · {}", OBRAS[k].1, OBRAS[k].2).into(),
                format!("{}-{}", OBRAS[k].1, dias_obra).into(),
            ),
            None => (SharedString::default(), SharedString::default(), SharedString::default(), "S/O-0".into()),
        };
        let clave: SharedString = if baja { "baja".into() } else { SharedString::default() };
        let fila = ReporteLiquidacionRow {
            selected: i < 3,
            can_delete_split: false,
            is_expanded: false,
            movimientos_resumen: SharedString::default(),
            tiene_inconsistencia: false,
            observacion_validacion: SharedString::default(),
            cedula_actualizada: i % 9 == 0,
            idx: (i + 1) as i32,
            id_reporte_liq: format!("RL-{:04}", i + 1).into(),
            id_liquidacion: format!("LIQ-{:05}", 14000 + i).into(),
            periodo: format!("{:04}/{:02} - {} {}", anio, mes, mes_nombre, anio).into(),
            nss: nss.into(),
            rfc_curp: format!("{rfc} / {curp}").into(),
            nombre_trabajador: nombre.into(),
            pensionado: "NO".into(),
            registro_patronal: "A4455667788".into(),
            id_registro_obra: obra_id,
            obra_numero: obra_num,
            obra_label,
            vinculaciones_resumen: vinc,
            multi_obra_badge: SharedString::default(),
            has_multi_obra: false,
            dias_mismatch: false,
            total_dias_obras: dias_obra,
            is_assigned: asignada,
            dias_cedula: dias_ced,
            dias_obra,
            clave,
            is_baja: baja,
            fecha: format!("{:02}/{:02}/{}", 10 + i % 15, mes, anio).into(),
            sdi_str: moneda(480.0 + (i % 10) as f64 * 35.5).into(),
            lic_str: fmt_dias(lic),
            inc_str: fmt_dias(inc),
            aus_str: "-".into(),
            has_incidencias: lic + inc > 0.0,
            lic_val: lic,
            inc_val: inc,
            aus_val: 0.0,
            row_bg_color: bg,
            row_hover_color: hover,
            row_accent_color: accent,
            has_prorrateo: asignada && ((sub - sub_orig).abs() > 0.009 || dias_obra != dias_ced),
            total_prorrateado_str: moneda(sub).into(),
            cf_str: moneda(c_cur[0]).into(),
            exc_pat_str: moneda(c_cur[1]).into(),
            exc_obr_str: moneda(c_cur[2]).into(),
            pd_pat_str: moneda(c_cur[3]).into(),
            pd_obr_str: moneda(c_cur[4]).into(),
            gmp_pat_str: moneda(c_cur[5]).into(),
            gmp_obr_str: moneda(c_cur[6]).into(),
            rt_str: moneda(c_cur[7]).into(),
            iv_pat_str: moneda(c_cur[8]).into(),
            iv_obr_str: moneda(c_cur[9]).into(),
            gps_str: moneda(c_cur[10]).into(),
            patronal_str: moneda(c_cur[11]).into(),
            obrera_str: moneda(c_cur[12]).into(),
            subtotal_str: moneda(c_cur[13]).into(),
            cf_orig_str: moneda(c_orig[0]).into(),
            exc_pat_orig_str: moneda(c_orig[1]).into(),
            exc_obr_orig_str: moneda(c_orig[2]).into(),
            pd_pat_orig_str: moneda(c_orig[3]).into(),
            pd_obr_orig_str: moneda(c_orig[4]).into(),
            gmp_pat_orig_str: moneda(c_orig[5]).into(),
            gmp_obr_orig_str: moneda(c_orig[6]).into(),
            rt_orig_str: moneda(c_orig[7]).into(),
            iv_pat_orig_str: moneda(c_orig[8]).into(),
            iv_obr_orig_str: moneda(c_orig[9]).into(),
            gps_orig_str: moneda(c_orig[10]).into(),
            patronal_orig_str: moneda(c_orig[11]).into(),
            obrera_orig_str: moneda(c_orig[12]).into(),
            subtotal_orig_str: moneda(c_orig[13]).into(),
        };
        filas.push(fila);

        for k in 0..14 {
            sum_orig[k] += c_orig[k];
            sum_cur[k] += c_cur[k];
        }
        tot_ced += dias_ced;
        tot_obr += dias_obra;
        tot_lic += lic;
        tot_inc += inc;

        let acum = &mut por_periodo[p_idx];
        acum.0.insert(nss);
        acum.1 += dias_ced;
        acum.2 += dias_obra;
        for k in 0..14 {
            acum.3[k] += c_cur[k];
        }
        acum.4 += 1;

        if let Some(k) = obra {
            por_obra[k].0.insert(nss);
            por_obra[k].1 += c_cur[10];
        }
    }
    let total_filas = filas.len();
    app.set_reporte_liquidaciones_list(ModelRc::new(VecModel::from(filas.clone())));

    // Filas seleccionadas para el panel de modificación masiva (las tres primeras)
    let seleccionados: Vec<ReporteSelectedItem> = filas
        .iter()
        .take(3)
        .enumerate()
        .map(|(n, f)| ReporteSelectedItem {
            id: f.id_reporte_liq.clone(),
            idx: n as i32 + 1,
            nombre_trabajador: f.nombre_trabajador.clone(),
            pensionado: "NO".into(),
            nss: f.nss.clone(),
            periodo: f.periodo.clone(),
            obra_label: f.obra_label.clone(),
        })
        .collect();
    app.set_reporte_selected_count(seleccionados.len() as i32);
    app.set_reporte_selected_items(ModelRc::new(VecModel::from(seleccionados)));
    app.set_reporte_all_filter_selected(false);

    // ── Periodos, KPIs, subtítulo, paginación y footer
    let mut periodo_opts: Vec<SharedString> = vec![format!("Todos los meses ({total_filas})").into()];
    let mut meses_items: Vec<ReporteMesTotalItem> = Vec::new();
    for (p_idx, (nss_set, ced, obr, montos, conteo)) in por_periodo.iter().enumerate() {
        let (_, _, mes_nombre) = PERIODOS[p_idx];
        let (anio, mes, _) = PERIODOS[p_idx];
        let etiqueta = format!("{:04}/{:02} - {} {}", anio, mes, mes_nombre, anio);
        periodo_opts.push(format!("{etiqueta} ({conteo})").into());
        meses_items.push(ReporteMesTotalItem {
            mes_label: format!("{mes_nombre} {anio}").into(),
            empleados_count: nss_set.len() as i32,
            dias_cedula: *ced,
            dias_obra: *obr,
            cf_str: moneda(montos[0]).into(),
            patronal_str: moneda(montos[11]).into(),
            obrera_str: moneda(montos[12]).into(),
            subtotal_str: moneda(montos[13]).into(),
        });
    }
    app.set_reporte_periodo_options(ModelRc::new(VecModel::from(periodo_opts)));
    app.set_reporte_totales_por_mes(ModelRc::new(VecModel::from(meses_items)));
    app.set_show_reporte_totales_por_mes(true);

    let nums: Vec<&str> = OBRAS.iter().map(|o| o.1).collect();
    app.set_reporte_subtitle(
        format!("Liquidaciones de la revisión activa [{}]. Total: {total_filas} registros cargados.", nums.join(", ")).into(),
    );

    app.set_reporte_kpi_total_registros(total_filas as i32);
    app.set_reporte_kpi_total_registros_str(format!("{total_filas} filas (100 en pág)").into());
    app.set_reporte_kpi_obras_count(OBRAS.len() as i32);
    app.set_reporte_kpi_obras_homologadas_str(format!("{} Homologadas", OBRAS.len()).into());
    app.set_reporte_kpi_subtotal_imss_str(moneda(sum_orig[13]).into());
    app.set_reporte_kpi_prorrateado_str(moneda(sum_cur[13]).into());
    let diferencia = sum_cur[13] - sum_orig[13];
    let dif_pct = if sum_orig[13] > 0.0 { diferencia / sum_orig[13] * 100.0 } else { 0.0 };
    let dif_str = if diferencia < 0.0 {
        format!("{} ({:.1}%)", moneda(diferencia), dif_pct)
    } else {
        format!("+{} (+{:.1}%)", moneda(diferencia), dif_pct)
    };
    app.set_reporte_kpi_diferencia_str(dif_str.into());

    app.set_reporte_page_current(1);
    app.set_reporte_page_total(1);
    app.set_reporte_page_start_row(1);
    app.set_reporte_page_end_row(total_filas as i32);
    app.set_reporte_total_filtered(total_filas as i32);

    app.set_reporte_footer_total_dias_ced_str(tot_ced.to_string().into());
    app.set_reporte_footer_total_dias_obr_str(tot_obr.to_string().into());
    app.set_reporte_footer_total_lic_str(fmt_dias(tot_lic));
    app.set_reporte_footer_total_inc_str(fmt_dias(tot_inc));
    app.set_reporte_footer_total_aus_str("-".into());
    app.set_reporte_footer_total_cf_str(moneda(sum_cur[0]).into());
    app.set_reporte_footer_total_exc_pat_str(moneda(sum_cur[1]).into());
    app.set_reporte_footer_total_exc_obr_str(moneda(sum_cur[2]).into());
    app.set_reporte_footer_total_pd_pat_str(moneda(sum_cur[3]).into());
    app.set_reporte_footer_total_pd_obr_str(moneda(sum_cur[4]).into());
    app.set_reporte_footer_total_gmp_pat_str(moneda(sum_cur[5]).into());
    app.set_reporte_footer_total_gmp_obr_str(moneda(sum_cur[6]).into());
    app.set_reporte_footer_total_rt_str(moneda(sum_cur[7]).into());
    app.set_reporte_footer_total_iv_pat_str(moneda(sum_cur[8]).into());
    app.set_reporte_footer_total_iv_obr_str(moneda(sum_cur[9]).into());
    app.set_reporte_footer_total_gps_str(moneda(sum_cur[10]).into());
    app.set_reporte_footer_total_patronal_str(moneda(sum_cur[11]).into());
    app.set_reporte_footer_total_obrera_str(moneda(sum_cur[12]).into());
    app.set_reporte_footer_total_subtotal_str(moneda(sum_cur[13]).into());

    // ── Guarderías (tope 18 % por registro de obra)
    let montos_obra = [1_850_000.0f64, 2_400_000.0, 960_000.0];
    let guarderias: Vec<ReporteGuarderiaObraItem> = OBRAS
        .iter()
        .enumerate()
        .map(|(k, (id, num, nombre, _))| {
            let gps = por_obra[k].1;
            let valor = gps * 0.5;
            let limite = montos_obra[k] * 0.18;
            ReporteGuarderiaObraItem {
                id_obra: (*id).into(),
                obra_numero: (*num).into(),
                obra_label: format!("{num} · {nombre}").into(),
                total_gps: gps as f32,
                total_gps_str: moneda(gps).into(),
                valor_guarderias: valor as f32,
                valor_guarderias_str: moneda(valor).into(),
                monto_obra: montos_obra[k] as f32,
                monto_obra_str: moneda(montos_obra[k]).into(),
                limite_18_obra: limite as f32,
                limite_18_obra_str: moneda(limite).into(),
                supera_18: valor > limite,
                sin_monto: false,
                empleados_count: por_obra[k].0.len() as i32,
                registros_patronales_str: "A4455667788".into(),
            }
        })
        .collect();
    app.set_reporte_guarderias_obras(ModelRc::new(VecModel::from(guarderias)));

    // ── Lista de empleados únicos y filtro avanzado
    let empleados_unicos: Vec<ReporteUniqueEmpleadoItem> = EMPLEADOS
        .iter()
        .enumerate()
        .map(|(k, (nombre, nss, rfc, _))| ReporteUniqueEmpleadoItem {
            id: format!("EMP-{:03}", k + 1).into(),
            nombre: (*nombre).into(),
            rfc: (*rfc).into(),
            nss: (*nss).into(),
            selected: k < 2,
            periodos_count: 3,
        })
        .collect();
    app.set_reporte_unique_empleados(ModelRc::new(VecModel::from(empleados_unicos.clone())));
    app.set_reporte_unique_empleados_filtered(ModelRc::new(VecModel::from(empleados_unicos)));
    app.set_reporte_unique_empleados_selected_count(2);
    app.set_reporte_unique_empleados_filter_active(true);

    let anios = vec![
        ReporteFiltroOpcion { clave: "2014".into(), etiqueta: "2014".into(), grupo: "Año".into(), conteo: 5, seleccionado: false },
        ReporteFiltroOpcion { clave: "2015".into(), etiqueta: "2015".into(), grupo: "Año".into(), conteo: 2, seleccionado: true },
    ];
    app.set_reporte_filtro_anios(ModelRc::new(VecModel::from(anios)));
    let meses: Vec<ReporteFiltroOpcion> = PERIODOS
        .iter()
        .enumerate()
        .map(|(k, (anio, mes, nombre))| ReporteFiltroOpcion {
            clave: format!("{anio}-{mes:02}").into(),
            etiqueta: format!("{} {anio}", nombre.to_uppercase()).into(),
            grupo: format!("{anio}").into(),
            conteo: por_periodo[k].4 as i32,
            seleccionado: k == 5,
        })
        .collect();
    app.set_reporte_filtro_meses(ModelRc::new(VecModel::from(meses)));
    let mut obras_filtro: Vec<ReporteFiltroOpcion> = OBRAS
        .iter()
        .enumerate()
        .map(|(k, (id, num, nombre, _))| ReporteFiltroOpcion {
            clave: (*id).into(),
            etiqueta: format!("{num} · {nombre}").into(),
            grupo: "Obra".into(),
            conteo: por_obra[k].0.len() as i32,
            seleccionado: k == 0,
        })
        .collect();
    obras_filtro.push(ReporteFiltroOpcion {
        clave: "sin_asignar".into(),
        etiqueta: "Sin asignar".into(),
        grupo: "Obra".into(),
        conteo: 4,
        seleccionado: false,
    });
    app.set_reporte_filtro_obras(ModelRc::new(VecModel::from(obras_filtro)));
    app.set_reporte_filtro_modo_general(true);
    app.set_reporte_filtro_activo(false);
    app.set_reporte_filtro_criterios(0);
    app.set_reporte_filtro_coinciden(total_filas as i32);
    app.set_reporte_filtro_busqueda_emp("".into());

    let resultados: Vec<ReporteFiltroEmpleado> = EMPLEADOS
        .iter()
        .take(6)
        .enumerate()
        .map(|(k, (nombre, nss, rfc, curp))| ReporteFiltroEmpleado {
            clave: format!("EMP-{:03}", k + 1).into(),
            nombre: (*nombre).into(),
            nss: (*nss).into(),
            rfc: (*rfc).into(),
            curp: (*curp).into(),
            conteo: 2 + (k % 3) as i32,
            seleccionado: false,
        })
        .collect();
    let seleccion: Vec<ReporteFiltroEmpleado> = resultados
        .iter()
        .take(2)
        .map(|e| ReporteFiltroEmpleado { seleccionado: true, ..e.clone() })
        .collect();
    app.set_reporte_filtro_emp_resultados(ModelRc::new(VecModel::from(resultados)));
    app.set_reporte_filtro_emp_seleccionados(ModelRc::new(VecModel::from(seleccion)));

    // ── Modal de edición (fila RL-0001, asignada a OB-101)
    let fila0 = &filas[0];
    let edit_obras: Vec<ReporteEditObraItem> = OBRAS
        .iter()
        .enumerate()
        .map(|(k, (id, num, nombre, vig))| ReporteEditObraItem {
            id: (*id).into(),
            numero: (*num).into(),
            label: format!("{num} · {nombre}").into(),
            vigencia_str: (*vig).into(),
            selected: k == 0,
            dias_obra: if k == 0 { 30 } else { 0 },
            dias_obra_str: if k == 0 { "30".into() } else { "0".into() },
            tope_max: 30,
            dias_activos_mes: 30,
            has_error: false,
        })
        .collect();
    let edit_seleccionadas: Vec<ReporteEditObraItem> = edit_obras.iter().filter(|o| o.selected).cloned().collect();
    app.set_reporte_edit_row_id(fila0.id_reporte_liq.clone());
    app.set_reporte_edit_worker_name(fila0.nombre_trabajador.clone());
    app.set_reporte_edit_periodo(fila0.periodo.clone());
    app.set_reporte_edit_selected_obra_id(OBRAS[0].0.into());
    app.set_reporte_edit_selected_obra_label(format!("{} · {}", OBRAS[0].1, OBRAS[0].2).into());
    app.set_reporte_edit_search_query("".into());
    app.set_reporte_edit_dropdown_open(false);
    app.set_reporte_edit_dias_cedula_val("30".into());
    app.set_reporte_edit_dias_obra_val("30".into());
    app.set_reporte_edit_lic_info("0".into());
    app.set_reporte_edit_inc_info("0".into());
    app.set_reporte_edit_aus_info("0".into());
    app.set_reporte_edit_dias_max(30);
    app.set_reporte_edit_obra_vigencia_info(format!("Vigencia: {}", OBRAS[0].3).into());
    app.set_reporte_edit_vinculaciones_info("OB-101-30".into());
    app.set_reporte_edit_alert_show(false);
    app.set_reporte_edit_alert_text("".into());
    app.set_reporte_edit_obras_list(ModelRc::new(VecModel::from(edit_obras)));
    app.set_reporte_edit_selected_obras(ModelRc::new(VecModel::from(edit_seleccionadas)));
    app.set_reporte_edit_total_disponibles(30);
    app.set_reporte_edit_dias_asignados_sum(30);
    app.set_reporte_edit_dias_restantes(0);
    app.set_reporte_edit_selected_count(1);
    app.set_reporte_edit_has_errors(false);

    // ── Modal de modificación masiva (tres filas seleccionadas)
    let lote_workers: Vec<ReporteBatchWorkerItem> = filas
        .iter()
        .take(3)
        .map(|f| ReporteBatchWorkerItem {
            id: f.id_reporte_liq.clone(),
            id_parent: format!("PAR-{}", f.id_reporte_liq).into(),
            nss: f.nss.clone(),
            nombre: f.nombre_trabajador.clone(),
            periodo: f.periodo.clone(),
            obra_label: f.obra_label.clone(),
            dias_cedula: f.dias_cedula,
            dias_obra: f.dias_obra,
            dias_obra_str: format!("{}", f.dias_obra).into(),
            tope_max: 30,
            total_disponibles: 30,
            has_error: false,
            lic_info: "0".into(),
            inc_info: "0".into(),
            aus_info: "0".into(),
            incidencias_str: "Sin incidencias".into(),
        })
        .collect();
    let lote_obras_multi: Vec<ReporteBatchObraItem> = OBRAS
        .iter()
        .enumerate()
        .map(|(k, (id, num, nombre, vig))| ReporteBatchObraItem {
            id: (*id).into(),
            numero: (*num).into(),
            label: format!("{num} · {nombre}").into(),
            vigencia_str: (*vig).into(),
            dias_tope_est: 30,
            selected: k == 0,
        })
        .collect();
    app.set_reporte_batch_edit_workers(ModelRc::new(VecModel::from(lote_workers)));
    app.set_reporte_batch_edit_worker_count(3);
    app.set_reporte_batch_edit_selected_obra_id("__KEEP__".into());
    app.set_reporte_batch_edit_selected_obra_label("".into());
    app.set_reporte_batch_edit_search_query("".into());
    app.set_reporte_batch_edit_dropdown_open(false);
    app.set_reporte_batch_edit_dias_cedula_val("".into());
    app.set_reporte_batch_edit_dias_obra_val("".into());
    app.set_reporte_batch_edit_change_obra(false);
    app.set_reporte_batch_edit_change_dias_cedula(false);
    app.set_reporte_batch_edit_change_dias_obra(false);
    app.set_reporte_batch_edit_alert_show(false);
    app.set_reporte_batch_edit_alert_text("".into());
    app.set_reporte_batch_edit_obras_multiselect(ModelRc::new(VecModel::from(lote_obras_multi)));
    app.set_reporte_batch_edit_selected_obras_count(1);
    app.set_reporte_batch_edit_multi_obra_enabled(false);

    // ── Exportación a SUA
    let sua: Vec<ReporteSuaObraItem> = OBRAS
        .iter()
        .enumerate()
        .map(|(k, (id, num, _, vig))| ReporteSuaObraItem {
            id: (*id).into(),
            numero: (*num).into(),
            tipo_obra: "PRIVADA".into(),
            vigencia: (*vig).into(),
            empleados_count: 4 + k as i32,
            selected: k < 2,
            creado_usuario: true,
        })
        .collect();
    app.set_reporte_sua_obras(ModelRc::new(VecModel::from(sua)));
    app.set_reporte_sua_selected_count(2);
    app.set_reporte_sua_total_empleados_filtrados(10);

    // ── Historial de entregables IMSS
    let historial = vec![
        EntregableVersion {
            id: "ent-v3".into(),
            version: 3,
            tipo: "IMSS".into(),
            fecha: "12/11/2026 10:21".into(),
            hash: "9f2c4a7e1b08d3f6".into(),
            resumen: "Entregable completo, 25 filas, 3 obras".into(),
        },
        EntregableVersion {
            id: "ent-v2".into(),
            version: 2,
            tipo: "BORRADOR".into(),
            fecha: "09/11/2026 16:02".into(),
            hash: "51ab0e7d93c2f184".into(),
            resumen: "Borrador previo a la conciliación de días".into(),
        },
        EntregableVersion {
            id: "ent-v1".into(),
            version: 1,
            tipo: "IMSS".into(),
            fecha: "02/11/2026 09:45".into(),
            hash: "c07e3d52a9f16b48".into(),
            resumen: "Primera emisión".into(),
        },
    ];
    app.set_entregable_historial(ModelRc::new(VecModel::from(historial)));
    app.set_entregable_siguiente_version(4);
    app.set_entregable_puede_entregar(true);
    app.set_entregable_borrador_solo_filtrada(false);
    app.set_entregable_resumen("Revisión lista para entrega, sin bloqueos.".into());
    app.set_entregable_puntos(ModelRc::new(VecModel::from(vec![
        EntregablePunto { severidad: 1, titulo: "Obra sin cédula en un periodo".into(), detalle: "OB-103 · Julio 2015".into() },
        EntregablePunto { severidad: 2, titulo: "Filas con prorrateo".into(), detalle: "14 registros con días ajustados".into() },
    ])));

    // ── Pestañas de asignación: por empleado (tab 1) y por obra (tab 2)
    let meses_asig: Vec<String> = PERIODOS
        .iter()
        .map(|(anio, _, nombre)| format!("{} {anio}", &nombre[..3]))
        .collect();
    app.set_asig_meses(ModelRc::new(VecModel::from(meses_asig.iter().map(|m| SharedString::from(m.as_str())).collect::<Vec<_>>())));

    let asig_obras: Vec<AsigObraOpcion> = OBRAS
        .iter()
        .enumerate()
        .map(|(k, (id, num, nombre, vig))| AsigObraOpcion {
            id_obra: (*id).into(),
            numero: (*num).into(),
            label: format!("{num} · {nombre}").into(),
            vigencia: (*vig).into(),
            color_idx: k as i32,
        })
        .collect();
    app.set_asig_obras(ModelRc::new(VecModel::from(asig_obras.clone())));

    let mut con_obra = 0;
    let mut sin_obra = 0;
    let filas_asig: Vec<AsigEmpleadoFila> = EMPLEADOS
        .iter()
        .enumerate()
        .map(|(e, (nombre, nss, rfc, _))| {
            let celdas: Vec<AsigCelda> = (0..PERIODOS.len())
                .map(|m| {
                    let asignados = if m % 3 == 2 { 0 } else { 20 + (e % 3) as i32 * 3 };
                    if asignados > 0 { con_obra += 1 } else { sin_obra += 1 }
                    let k = (e + m) % 3;
                    let obras_chip = if asignados > 0 {
                        vec![AsigObraChip {
                            id_obra: OBRAS[k].0.into(),
                            numero: OBRAS[k].1.into(),
                            dias: asignados,
                            color_idx: k as i32,
                            tope: 40,
                        }]
                    } else {
                        vec![]
                    };
                    let opciones: Vec<AsigObraOpcionCelda> = OBRAS
                        .iter()
                        .enumerate()
                        .map(|(j, o)| AsigObraOpcionCelda { id_obra: o.0.into(), numero: o.1.into(), tope: 30, color_idx: j as i32 })
                        .collect();
                    AsigCelda {
                        existe: true,
                        id_reporte: format!("RL-{:04}", e * 7 + m + 1).into(),
                        etiqueta_mes: meses_asig[m].as_str().into(),
                        dias_cedula: 30,
                        disponibles: 30 - asignados,
                        asignados,
                        obras: ModelRc::new(VecModel::from(obras_chip)),
                        opciones: ModelRc::new(VecModel::from(opciones)),
                        es_baja: e == 3 && m == 6,
                        sin_obra: asignados == 0,
                        inconsistente: false,
                        cedula_actualizada: e % 4 == 0 && m == 0,
                    }
                })
                .collect();
            AsigEmpleadoFila {
                id_empleado: format!("EMP-{:03}", e + 1).into(),
                nombre: (*nombre).into(),
                nss: (*nss).into(),
                rfc: (*rfc).into(),
                num_obras: if e % 3 == 0 { 2 } else { 1 },
                tiene_inconsistencia: e == 3,
                cedula_actualizada: e % 4 == 0,
                celdas: ModelRc::new(VecModel::from(celdas)),
            }
        })
        .collect();
    let total_celdas = con_obra + sin_obra;
    app.set_asig_filas(ModelRc::new(VecModel::from(filas_asig)));
    app.set_asig_total(total_celdas);
    app.set_asig_con_obra(con_obra);
    app.set_asig_sin_obra(sin_obra);
    app.set_asig_inconsistencias(1);
    app.set_asig_cedula_actualizada(3);
    app.set_asig_porcentaje(con_obra as f32 / total_celdas.max(1) as f32 * 100.0);
    app.set_asig_escenario_texto("Escenario 1 · Asignación parcial".into());
    app.set_asig_filtro_idx(0);
    app.set_asig_busqueda("".into());
    app.set_asig_puede_deshacer(false);
    app.set_asig_procesando(false);
    app.set_asig_empleado_sel_id("EMP-001".into());
    app.set_asig_empleado_sel_nombre(EMPLEADOS[0].0.into());

    let tarjetas: Vec<AsigObraTarjeta> = OBRAS
        .iter()
        .enumerate()
        .map(|(k, (id, num, nombre, vig))| {
            let monto = montos_obra[k];
            let gps = por_obra[k].1;
            let valor = gps * 0.5;
            let limite = monto * 0.18;
            AsigObraTarjeta {
                id_obra: (*id).into(),
                numero: (*num).into(),
                label: format!("{num} · {nombre}").into(),
                vigencia: (*vig).into(),
                monto: moneda(monto).into(),
                empleados: por_obra[k].0.len() as i32,
                empleado_mes: 6 + k as i32,
                dias: 90 + k as i32 * 17,
                gps: moneda(gps).into(),
                valor_guarderia: moneda(valor).into(),
                limite_18: moneda(limite).into(),
                supera_18: valor > limite,
                sin_monto: false,
                color_idx: k as i32,
            }
        })
        .collect();
    app.set_asig_obra_tarjetas(ModelRc::new(VecModel::from(tarjetas)));
    app.set_asig_obra_sel_id(OBRAS[0].0.into());
    app.set_asig_obra_sel_titulo(format!("{} · {}", OBRAS[0].1, OBRAS[0].2).into());
    let renglones: Vec<AsigObraRenglon> = EMPLEADOS
        .iter()
        .take(8)
        .enumerate()
        .map(|(e, (nombre, nss, _, _))| AsigObraRenglon {
            id_reporte: format!("RL-{:04}", e * 7 + 1).into(),
            id_empleado: format!("EMP-{:03}", e + 1).into(),
            nombre: (*nombre).into(),
            nss: (*nss).into(),
            etiqueta_mes: meses_asig[0].as_str().into(),
            dias: 20 + (e % 3) as i32 * 3,
            dias_cedula: 30,
            seleccionado: e < 2,
        })
        .collect();
    app.set_asig_obra_renglones(ModelRc::new(VecModel::from(renglones)));
    app.set_asig_obra_candidatos(ModelRc::new(VecModel::from(Vec::<AsigObraRenglon>::new())));

    app.set_reporte_tab_idx(0);
}

// ─── Guardado de imágenes ───────────────────────────────────────────────────

/// Escribe el buffer como PPM binario (P6), descartando el canal alfa.
fn guardar_ppm(dir: &Path, nombre: &str, buf: &SharedPixelBuffer<Rgba8Pixel>) -> std::io::Result<PathBuf> {
    let ruta = dir.join(format!("{nombre}.ppm"));
    let mut f = BufWriter::new(File::create(&ruta)?);
    write!(f, "P6\n{} {}\n255\n", buf.width(), buf.height())?;
    let mut rgb = Vec::with_capacity(buf.as_slice().len() * 3);
    for px in buf.as_slice() {
        rgb.extend_from_slice(&[px.r, px.g, px.b]);
    }
    f.write_all(&rgb)?;
    f.flush()?;
    Ok(ruta)
}

/// Convierte los PPM escritos con `magick` y borra cada PPM convertido.
/// Si `magick` no está instalado, se conservan los PPM.
fn convertir_ppm_a_png(ppms: &[PathBuf]) {
    for ppm in ppms {
        let png = ppm.with_extension("png");
        match Command::new("magick").arg(ppm).arg(&png).status() {
            Ok(s) if s.success() => {
                let _ = fs::remove_file(ppm);
            }
            Ok(s) => eprintln!("magick fallo ({s}) para {}; se conserva el PPM", ppm.display()),
            Err(e) => {
                eprintln!("magick no disponible ({e}); se conservan los PPM");
                return;
            }
        }
    }
}

// ─── Test ───────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn captura_reporte_liquidaciones() {
    let dir = PathBuf::from(std::env::var("CAPTURA_DIR").unwrap_or_else(|_| "/tmp/capturas_ui".to_string()));
    fs::create_dir_all(&dir).expect("no se pudo crear CAPTURA_DIR");

    let win = MainWindow::new().expect("no se pudo crear MainWindow");
    cargar_datos_falsos(&win.global::<AppData>());
    win.show().expect("no se pudo mostrar la ventana");

    let pasos = Rc::new(construir_pasos());
    let total = pasos.len();
    let indice = Rc::new(Cell::new(0usize));
    let ppms = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
    let errores = Rc::new(RefCell::new(Vec::<String>::new()));

    // Cada disparo: captura el paso anterior (ya asentado) y aplica el siguiente.
    let timer = slint::Timer::default();
    {
        let win_weak = win.as_weak();
        let pasos = pasos.clone();
        let indice = indice.clone();
        let ppms = ppms.clone();
        let errores = errores.clone();
        let dir = dir.clone();
        timer.start(slint::TimerMode::Repeated, INTERVALO, move || {
            let Some(win) = win_weak.upgrade() else {
                let _ = slint::quit_event_loop();
                return;
            };
            let i = indice.get();
            if i > 0 {
                let anterior = &pasos[i - 1];
                match win.window().take_snapshot() {
                    Ok(buf) => match guardar_ppm(&dir, &anterior.nombre, &buf) {
                        Ok(ruta) => ppms.borrow_mut().push(ruta),
                        Err(e) => errores.borrow_mut().push(format!("{}: {e}", anterior.nombre)),
                    },
                    Err(e) => errores.borrow_mut().push(format!("{}: take_snapshot: {e}", anterior.nombre)),
                }
            }
            if i >= pasos.len() {
                let _ = slint::quit_event_loop();
                return;
            }
            let paso = &pasos[i];
            win.window().set_size(slint::LogicalSize::new(paso.ancho as f32, paso.alto as f32));
            aplicar_paso(&win.global::<AppData>(), paso);
            win.window().request_redraw();
            indice.set(i + 1);
        });
    }

    slint::run_event_loop().expect("error en el event loop");
    drop(timer);

    let ppms = ppms.borrow().clone();
    convertir_ppm_a_png(&ppms);

    let errores = errores.borrow();
    println!("Capturas: {} de {} en {}", ppms.len(), total, dir.display());
    assert!(errores.is_empty(), "errores al capturar: {:?}", *errores);
    assert_eq!(ppms.len(), total, "no se generaron todas las capturas");
}
