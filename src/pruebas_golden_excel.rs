//! Pruebas "golden" estructurales del Excel IMSS.
//!
//! Generan Borrador y Entregable desde filas sintéticas y verifican el contenido
//! interno del `.xlsx` (partes XML dentro del zip). No requieren base de datos.
//! Los archivos se escriben solo en `std::env::temp_dir()` con nombres únicos y
//! se eliminan al terminar cada prueba (también si la prueba falla).

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::cedulas::business::mes_to_periodo_int;
use crate::db::reporte_liquidaciones::{
    compute_guarderias_kpis, export_reporte_liquidaciones_to_excel, ModoEntregable, ObraOptionDb,
    ReporteFinalHeaderMeta, ReporteLiquidacionRowDb,
};

const HOJAS_ESPERADAS: [&str; 5] = [
    "Reporte Final",
    "Prorrateo por Obra",
    "Totales por Empleado y Obra",
    "Totales por Empleado",
    "Análisis de Guarderías (18%)",
];
const BANNER_BORRADOR: &str = "BORRADOR — NO VÁLIDO PARA ENTREGA AL IMSS";
/// Datos fijos de versiones anteriores que jamás deben aparecer en una salida.
const DATOS_FIJOS_HISTORICOS: [&str; 3] = [
    "GON2110075C9",
    "ARTURO CARRILLO",
    "15/04/2026 AL 30/06/2026",
];

/// Archivo temporal único que se elimina al salir de su ámbito.
struct ArchivoTemporal(PathBuf);

impl ArchivoTemporal {
    fn nuevo(prefijo: &str) -> Self {
        Self(std::env::temp_dir().join(format!("{prefijo}_{}.xlsx", Uuid::now_v7())))
    }
}

impl Drop for ArchivoTemporal {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Encabezado de prueba completo con valores genéricos (no son datos reales).
fn meta_completo() -> ReporteFinalHeaderMeta {
    ReporteFinalHeaderMeta {
        delegacion: "DELEGACION PRUEBA".to_string(),
        subdelegacion: "SUBDELEGACION PRUEBA".to_string(),
        cliente_nombre: "EMPRESA PRUEBA SA DE CV".to_string(),
        rp_registro: "Y5410204108".to_string(),
        rfc: "XAXX010101000".to_string(),
        periodo_obra: "01/01/2024 AL 31/12/2024".to_string(),
        representante_legal: "REPRESENTANTE PRUEBA".to_string(),
        revision_folio: "REVISION PRUEBA 2024-01".to_string(),
    }
}

/// Construye una fila sintética con todos los campos de `ReporteLiquidacionRowDb`.
#[allow(clippy::too_many_arguments)]
fn fila(
    emp: Uuid,
    nombre: &str,
    obra: Option<(Uuid, &str)>,
    mes: &str,
    anio: i16,
    dias: i16,
    subtotal: f64,
    clave: &str,
) -> ReporteLiquidacionRowDb {
    let (oid, obra_num, es_usuario) = match obra {
        Some((id, num)) => (Some(id), num, true),
        None => (None, "", false),
    };
    ReporteLiquidacionRowDb {
        id_reporte_liq: Uuid::now_v7(),
        id_parent_reporte_liq: Uuid::now_v7(),
        id_liquidacion: Uuid::now_v7(),
        id_revision: Uuid::now_v7(),
        id_empleado: emp,
        nombre_trabajador: nombre.to_string(),
        rfc: "XAXX010101000".to_string(),
        curp: "XAXX010101HDF00".to_string(),
        nss: "12345678901".to_string(),
        pensionado: String::new(),
        registro_patronal: "Y5410204108".to_string(),
        mes_periodo: mes.to_string(),
        mes_periodo_int: mes_to_periodo_int(mes),
        anio_periodo: anio,
        periodo_str: format!("{mes}/{anio}"),
        dias_cedula: dias,
        id_registro_obra: oid,
        obra_numero: obra_num.to_string(),
        obra_label: if oid.is_some() { obra_num.to_string() } else { "⚠️ Sin Asignar".to_string() },
        dias_obra: if oid.is_some() { dias } else { 0 },
        obra_creado_usuario: es_usuario,
        clave: clave.to_string(),
        fecha: None,
        fecha_str: "-".to_string(),
        sdi: 350.0,
        lic: 0.0,
        inc: 0.0,
        aus: 0.0,
        cf: subtotal * 0.4,
        exc_pat: 0.0,
        exc_obr: 0.0,
        pd_pat: 0.0,
        pd_obr: 0.0,
        gmp_pat: 0.0,
        gmp_obr: 0.0,
        rt: 0.0,
        iv_pat: 0.0,
        iv_obr: 0.0,
        gps: subtotal * 0.05,
        patronal: subtotal * 0.8,
        obrera: subtotal * 0.2,
        subtotal,
        cf_orig: subtotal * 0.4,
        exc_pat_orig: 0.0,
        exc_obr_orig: 0.0,
        pd_pat_orig: 0.0,
        pd_obr_orig: 0.0,
        gmp_pat_orig: 0.0,
        gmp_obr_orig: 0.0,
        rt_orig: 0.0,
        iv_pat_orig: 0.0,
        iv_obr_orig: 0.0,
        gps_orig: subtotal * 0.05,
        patronal_orig: subtotal * 0.8,
        obrera_orig: subtotal * 0.2,
        subtotal_orig: subtotal,
        tiene_inconsistencia: false,
        observacion_validacion: String::new(),
        estado: if clave == "BAJA" { "BAJA".to_string() } else { "ACTIVO".to_string() },
        cedula_actualizada: false,
        editado_usuario: false,
    }
}

/// Filas sintéticas: 3 empleados x 2 meses, obra prorrateada, sin obra y una baja.
fn construir_fixture() -> (Vec<ReporteLiquidacionRowDb>, Vec<ObraOptionDb>, HashSet<Uuid>) {
    let obra_1 = Uuid::now_v7();
    let obra_2 = Uuid::now_v7();
    let emp_1 = Uuid::now_v7();
    let emp_2 = Uuid::now_v7();
    let emp_3 = Uuid::now_v7();

    let rows = vec![
        fila(emp_1, "PRUEBA PEREZ JUAN", Some((obra_1, "OBRA-PRUEBA-01")), "ENERO", 2024, 30, 1000.0, ""),
        fila(emp_1, "PRUEBA PEREZ JUAN", Some((obra_1, "OBRA-PRUEBA-01")), "FEBRERO", 2024, 28, 900.0, ""),
        fila(emp_1, "PRUEBA PEREZ JUAN", Some((obra_2, "OBRA-PRUEBA-02")), "FEBRERO", 2024, 28, 0.0, ""),
        fila(emp_2, "PRUEBA LOPEZ ANA", None, "ENERO", 2024, 30, 700.0, ""),
        // Prorrateada: 15 días en obra de 30 días de cédula
        fila(emp_2, "PRUEBA LOPEZ ANA", Some((obra_2, "OBRA-PRUEBA-02")), "FEBRERO", 2024, 30, 500.0, ""),
        // Baja: clave BAJA con días de obra
        fila(emp_3, "PRUEBA GARCIA LUIS", Some((obra_1, "OBRA-PRUEBA-01")), "ENERO", 2024, 10, 300.0, "BAJA"),
    ];

    let obras = vec![
        ObraOptionDb {
            id: obra_1,
            numero: "OBRA-PRUEBA-01".to_string(),
            tipo_obra: "EDIFICACION".to_string(),
            fecha_inicio: None,
            fecha_fin: None,
            label: "OBRA-PRUEBA-01 - OBRA DE PRUEBA".to_string(),
            creado_usuario: true,
            monto: 1_000_000.0,
        },
        ObraOptionDb {
            id: obra_2,
            numero: "OBRA-PRUEBA-02".to_string(),
            tipo_obra: "URBANIZACION".to_string(),
            fecha_inicio: None,
            fecha_fin: None,
            label: "OBRA-PRUEBA-02 - OBRA DE PRUEBA 2".to_string(),
            creado_usuario: true,
            monto: 500_000.0,
        },
    ];

    let validas: HashSet<Uuid> = obras.iter().map(|o| o.id).collect();
    (rows, obras, validas)
}

/// Genera el Excel de prueba en `path` con el encabezado y modo indicados.
fn generar(path: &Path, meta: &ReporteFinalHeaderMeta, modo: ModoEntregable) -> Result<(), String> {
    let (rows, obras, validas) = construir_fixture();
    let kpis = compute_guarderias_kpis(&rows, &obras, None);
    export_reporte_liquidaciones_to_excel(
        &rows,
        &rows,
        Some(&validas),
        &kpis,
        path,
        &meta.cliente_nombre,
        &meta.rp_registro,
        &meta.revision_folio,
        meta,
        modo,
    )
}

/// Todas las partes internas del `.xlsx` como (nombre, contenido UTF-8).
fn partes_xlsx(path: &Path) -> Vec<(String, String)> {
    let archivo = File::open(path).expect("el .xlsx generado debe poder abrirse");
    let mut zip = zip::ZipArchive::new(archivo).expect("el .xlsx debe ser un zip válido");
    let nombres: Vec<String> = zip.file_names().map(str::to_string).collect();
    nombres
        .into_iter()
        .map(|nombre| {
            let mut entrada = zip
                .by_name(&nombre)
                .unwrap_or_else(|e| panic!("no se pudo leer la parte {nombre}: {e}"));
            let mut contenido = String::new();
            entrada
                .read_to_string(&mut contenido)
                .unwrap_or_else(|e| panic!("la parte {nombre} no es UTF-8: {e}"));
            (nombre, contenido)
        })
        .collect()
}

fn parte<'a>(partes: &'a [(String, String)], nombre: &str) -> &'a str {
    partes
        .iter()
        .find(|(n, _)| n == nombre)
        .map(|(_, c)| c.as_str())
        .unwrap_or_else(|| panic!("falta la parte {nombre} en el xlsx"))
}

/// XML de cada hoja (`xl/worksheets/sheetN.xml`) ordenado por número de hoja.
fn xml_de_hojas(partes: &[(String, String)]) -> Vec<(usize, &str)> {
    let mut hojas: Vec<(usize, &str)> = partes
        .iter()
        .filter_map(|(n, c)| {
            let numero = n
                .strip_prefix("xl/worksheets/sheet")?
                .strip_suffix(".xml")?
                .parse::<usize>()
                .ok()?;
            Some((numero, c.as_str()))
        })
        .collect();
    hojas.sort_by_key(|(numero, _)| *numero);
    hojas
}

/// Texto comprendido entre `inicio` y `fin` (primera ocurrencia).
fn texto_entre<'a>(xml: &'a str, inicio: &str, fin: &str) -> Option<&'a str> {
    let desde = xml.find(inicio)? + inicio.len();
    let hasta = xml[desde..].find(fin)? + desde;
    Some(&xml[desde..hasta])
}

fn encabezado_impresion(xml: &str) -> &str {
    texto_entre(xml, "<oddHeader>", "</oddHeader>")
        .unwrap_or_else(|| panic!("la hoja no tiene <oddHeader>"))
}

#[test]
fn golden_borrador_estructura_y_marcas() {
    let salida = ArchivoTemporal::nuevo("golden_borrador");
    generar(&salida.0, &meta_completo(), ModoEntregable::Borrador { solo_vista_filtrada: false })
        .expect("el borrador con encabezado completo debe generarse");
    let partes = partes_xlsx(&salida.0);

    let libro = parte(&partes, "xl/workbook.xml");
    for hoja in HOJAS_ESPERADAS {
        assert!(libro.contains(&format!("name=\"{hoja}\"")), "falta la hoja {hoja}");
    }
    assert!(libro.contains("_xlnm.Print_Titles"), "falta _xlnm.Print_Titles");

    let hojas = xml_de_hojas(&partes);
    assert_eq!(hojas.len(), HOJAS_ESPERADAS.len(), "número de hojas inesperado");

    for (numero, xml) in &hojas {
        assert!(
            encabezado_impresion(xml).contains("BORRADOR"),
            "hoja {numero} sin marca BORRADOR en el encabezado"
        );
        assert!(!xml.contains("<sheetProtection"), "el borrador no debe estar protegido (hoja {numero})");
        assert!(xml.contains("orientation=\"landscape\""), "hoja {numero} no está en horizontal");
        assert!(xml.contains("fitToPage=\"1\""), "hoja {numero} sin ajuste a página");
        assert!(xml.contains("state=\"frozen\""), "hoja {numero} sin paneles congelados");
    }

    let (_, hoja_1) = hojas[0];
    assert!(
        hoja_1.contains("<f>") || hoja_1.contains("<f "),
        "la hoja 1 debe contener fórmulas (SUMIF de totales)"
    );

    let shared = parte(&partes, "xl/sharedStrings.xml");
    assert!(shared.contains(BANNER_BORRADOR), "falta el banner de borrador");
}

#[test]
fn golden_borrador_solo_vista_filtrada_marca_encabezado() {
    let salida = ArchivoTemporal::nuevo("golden_borrador_filtrado");
    generar(&salida.0, &meta_completo(), ModoEntregable::Borrador { solo_vista_filtrada: true })
        .expect("el borrador de vista filtrada debe generarse");
    let partes = partes_xlsx(&salida.0);

    let hojas = xml_de_hojas(&partes);
    for (numero, xml) in &hojas {
        let cabecera = encabezado_impresion(xml);
        assert!(cabecera.contains("BORRADOR"), "hoja {numero} sin marca BORRADOR");
        assert!(
            cabecera.contains("SOLO VISTA FILTRADA"),
            "hoja {numero} sin marca SOLO VISTA FILTRADA en el encabezado"
        );
    }
}

#[test]
fn golden_entregable_v3_protegido_y_sin_banner() {
    let salida = ArchivoTemporal::nuevo("golden_entregable");
    generar(&salida.0, &meta_completo(), ModoEntregable::Entregable { version: 3 })
        .expect("el entregable con encabezado completo debe generarse");
    let partes = partes_xlsx(&salida.0);

    let hojas = xml_de_hojas(&partes);
    assert_eq!(hojas.len(), HOJAS_ESPERADAS.len(), "número de hojas inesperado");
    for (numero, xml) in &hojas {
        let cabecera = encabezado_impresion(xml);
        assert!(
            cabecera.contains("ENTREGABLE IMSS v3"),
            "hoja {numero} sin marca ENTREGABLE IMSS v3"
        );
        assert!(!cabecera.contains("BORRADOR"), "hoja {numero} con marca de borrador");
        assert!(xml.contains("<sheetProtection"), "hoja {numero} sin protección");
    }

    let shared = parte(&partes, "xl/sharedStrings.xml");
    assert!(!shared.contains(BANNER_BORRADOR), "el entregable no debe llevar el banner de borrador");
    assert!(!shared.contains("[FALTA:"), "el entregable no debe llevar marcadores [FALTA]");
}

#[test]
fn golden_meta_incompleto_rechaza_entregable_y_marca_borrador() {
    let mut meta = meta_completo();
    meta.representante_legal = String::new();

    let rechazado = ArchivoTemporal::nuevo("golden_sin_representante_entregable");
    let error = generar(&rechazado.0, &meta, ModoEntregable::Entregable { version: 1 })
        .expect_err("un entregable sin representante legal debe rechazarse");
    assert!(
        error.contains("Representante legal"),
        "el error debe nombrar el campo faltante: {error}"
    );

    let borrador = ArchivoTemporal::nuevo("golden_sin_representante_borrador");
    generar(&borrador.0, &meta, ModoEntregable::Borrador { solo_vista_filtrada: false })
        .expect("el borrador con campos faltantes debe generarse");
    let partes = partes_xlsx(&borrador.0);
    let shared = parte(&partes, "xl/sharedStrings.xml");
    assert!(
        shared.contains("[FALTA: Representante legal]"),
        "el borrador debe marcar el representante faltante"
    );
}

#[test]
fn golden_sin_datos_fijos_historicos() {
    let borrador = ArchivoTemporal::nuevo("golden_fijos_borrador");
    generar(&borrador.0, &meta_completo(), ModoEntregable::Borrador { solo_vista_filtrada: false })
        .expect("borrador debe generarse");
    let entregable = ArchivoTemporal::nuevo("golden_fijos_entregable");
    generar(&entregable.0, &meta_completo(), ModoEntregable::Entregable { version: 3 })
        .expect("entregable debe generarse");

    for salida in [&borrador.0, &entregable.0] {
        for (nombre, contenido) in partes_xlsx(salida) {
            for fijo in DATOS_FIJOS_HISTORICOS {
                assert!(
                    !contenido.contains(fijo),
                    "la parte {nombre} de {} contiene el dato fijo histórico {fijo}",
                    salida.display()
                );
            }
        }
    }
}
