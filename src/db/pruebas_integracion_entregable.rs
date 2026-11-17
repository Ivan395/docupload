//! Genera, con datos REALES de la BD (solo lectura), el borrador y el entregable del Excel IMSS de una
//! revisión como archivos locales en la carpeta indicada por `ENTREGABLE_SALIDA`. No sube a RustFS ni
//! registra versiones. Ejecutar:
//! `ENTREGABLE_SALIDA=/ruta cargo test excel_entregable_con_datos_reales -- --ignored --nocapture`

use std::collections::HashSet;

use crate::db::reporte_liquidaciones::{
    compute_guarderias_kpis, export_reporte_liquidaciones_to_excel, fetch_all_obras_for_revision,
    fetch_reporte_liquidaciones, meta_con_marcadores_faltantes, missing_header_fields, ModoEntregable,
};

#[test]
fn marcadores_de_faltantes_no_inventan_datos() {
    let meta = crate::db::reporte_liquidaciones::ReporteFinalHeaderMeta {
        rfc: "XAXX010101000".into(),
        ..Default::default()
    };
    let m = meta_con_marcadores_faltantes(&meta);
    assert_eq!(m.rfc, "XAXX010101000");
    assert_eq!(m.representante_legal, "[FALTA: Representante legal]");
    assert!(missing_header_fields(&m).is_empty(), "con marcadores ya no hay campos vacíos");
}

#[test]
#[ignore]
fn excel_entregable_con_datos_reales() {
    let Ok(salida) = std::env::var("ENTREGABLE_SALIDA") else {
        println!("Defina ENTREGABLE_SALIDA con una carpeta de salida");
        return;
    };
    let mut client = match crate::db::client::create_client() {
        Ok(c) => c,
        Err(e) => {
            println!("Sin conexión a BD: {e}");
            return;
        }
    };
    // Revisión con más reportes activos (la más representativa)
    let rev: uuid::Uuid = client
        .query_one(
            "SELECT id_revision FROM public.reportes_liquidaciones WHERE estado = 'ACTIVO' \
             GROUP BY id_revision ORDER BY COUNT(*) DESC LIMIT 1",
            &[],
        )
        .expect("revisión")
        .get(0);
    let filas: Vec<_> = fetch_reporte_liquidaciones(&mut client, rev)
        .expect("filas")
        .into_iter()
        .filter(|r| r.estado == "ACTIVO")
        .collect();
    let obras = fetch_all_obras_for_revision(&mut client, rev).expect("obras");
    let usuario: HashSet<uuid::Uuid> = obras.iter().filter(|o| o.creado_usuario).map(|o| o.id).collect();
    let kpis = compute_guarderias_kpis(&filas, &obras, None);
    let meta = crate::db::entregables::fetch_reporte_header_meta(&mut client, rev).expect("meta");
    let lista = crate::entregable_logic::evaluar_lista_verificacion(&meta, &filas, &obras, false);
    println!("ENT revision={rev} filas={} faltantes={:?}", filas.len(), missing_header_fields(&meta));
    for p in &lista.puntos {
        println!("ENT punto {:?}: {} — {}", p.severidad, p.titulo, p.detalle);
    }

    let dir = std::path::Path::new(&salida);
    std::fs::create_dir_all(dir).expect("carpeta");
    let borrador = dir.join("BORRADOR_prueba.xlsx");
    export_reporte_liquidaciones_to_excel(
        &filas, &filas, Some(&usuario), &kpis, &borrador, &meta.cliente_nombre, &meta.rp_registro,
        &meta.revision_folio, &meta, ModoEntregable::Borrador { solo_vista_filtrada: false },
    )
    .expect("borrador");
    println!("ENT borrador generado: {}", borrador.display());

    // El entregable exige encabezado completo: con datos reales incompletos debe rechazarse.
    let entregable = dir.join("v1_prueba.xlsx");
    let res = export_reporte_liquidaciones_to_excel(
        &filas, &filas, Some(&usuario), &kpis, &entregable, &meta.cliente_nombre, &meta.rp_registro,
        &meta.revision_folio, &meta, ModoEntregable::Entregable { version: 1 },
    );
    if missing_header_fields(&meta).is_empty() {
        res.expect("entregable");
        println!("ENT entregable generado: {}", entregable.display());
    } else {
        assert!(res.is_err(), "el entregable debe rechazarse con encabezado incompleto");
        println!("ENT entregable rechazado correctamente: {}", res.unwrap_err());
        // Para inspeccionar el formato del entregable, se genera también con marcadores (solo prueba local).
        let completo = meta_con_marcadores_faltantes(&meta);
        export_reporte_liquidaciones_to_excel(
            &filas, &filas, Some(&usuario), &kpis, &entregable, &completo.cliente_nombre, &completo.rp_registro,
            &completo.revision_folio, &completo, ModoEntregable::Entregable { version: 1 },
        )
        .expect("entregable de inspección");
        println!("ENT entregable de inspección generado: {}", entregable.display());
    }
}

/// Diagnóstico (solo lectura): filas que devuelve fetch_reporte_liquidaciones vs tramos reales por revisión.
#[test]
#[ignore]
fn fetch_no_duplica_tramos_bd() {
    let mut client = match crate::db::client::create_client() {
        Ok(c) => c,
        Err(e) => {
            println!("Sin conexión a BD: {e}");
            return;
        }
    };
    let revs: Vec<(uuid::Uuid, i64)> = client
        .query(
            "SELECT rl.id_revision, COUNT(d.id)::bigint FROM public.reportes_liquidaciones rl \
             JOIN public.reportes_detalles_liquidaciones d ON d.id_reporte_liquidacion = rl.id \
             GROUP BY rl.id_revision",
            &[],
        )
        .expect("conteo")
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    for (rev, tramos) in revs {
        let filas = fetch_reporte_liquidaciones(&mut client, rev).expect("fetch");
        let ids: HashSet<uuid::Uuid> = filas.iter().map(|f| f.id_reporte_liq).collect();
        let revisiones: HashSet<uuid::Uuid> = filas.iter().map(|f| f.id_revision).collect();
        println!(
            "FETCH rev={rev} tramos_bd={tramos} filas_fetch={} ids_unicos={} revisiones_en_filas={}",
            filas.len(), ids.len(), revisiones.len()
        );
    }
}
