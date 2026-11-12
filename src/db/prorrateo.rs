//! Regla canónica de prorrateo de cuotas IMSS por obra ("regla de tres").
//!
//! Existe una versión en memoria (Rust, `rust_decimal`) y una versión en BD (SQL).
//! Ambas aplican el mismo redondeo "mitad lejos de cero" sobre importes de 2 decimales.
//! La conversión f64 -> Decimal replica `float8::numeric` de PostgreSQL (15 dígitos significativos).

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};

/// Importes de un tramo (cédula). Los 11 ramas y `subtotal` se usan como entrada;
/// `patronal` y `obrera` se recalculan como suma de ramas ya redondeadas.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ImportesTramo {
    pub cf: f64,
    pub exc_pat: f64,
    pub exc_obr: f64,
    pub pd_pat: f64,
    pub pd_obr: f64,
    pub gmp_pat: f64,
    pub gmp_obr: f64,
    pub rt: f64,
    pub iv_pat: f64,
    pub iv_obr: f64,
    pub gps: f64,
    pub patronal: f64,
    pub obrera: f64,
    pub subtotal: f64,
}

/// Convierte un f64 a Decimal igual que `float8::numeric` de PostgreSQL (15 dígitos significativos).
/// Valores no finitos (NaN/inf) se toman como 0 con aviso en log.
pub fn decimal_desde_f64(v: f64) -> Decimal {
    if !v.is_finite() {
        log::warn!("prorrateo: valor no finito ({v}) tratado como 0");
        return Decimal::ZERO;
    }
    match Decimal::from_scientific(&format!("{:.14e}", v)) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("prorrateo: no se pudo convertir {v} a Decimal ({e}); se usa 0");
            Decimal::ZERO
        }
    }
}

/// Días efectivos de cédula: `max(0, round_half_away(dias_cedula - (lic + inc + aus)))`.
pub fn dias_efectivos(dias_cedula: i32, lic: f64, inc: f64, aus: f64) -> i32 {
    let incidencias = decimal_desde_f64(lic) + decimal_desde_f64(inc) + decimal_desde_f64(aus);
    (Decimal::from(dias_cedula) - incidencias)
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .max(Decimal::ZERO)
        .to_i32()
        .unwrap_or(0)
}

/// Factor de prorrateo del tramo.
/// - Sin obra: 1 (importe completo).
/// - Con obra y `dias_cedula <= 0` o `dias_obra <= 0`: 0.
/// - Con obra: `min(dias_obra, dias_efectivos) / dias_cedula`, sin redondear.
pub fn factor_prorrateo(
    tiene_obra: bool,
    dias_cedula: i32,
    dias_obra: i32,
    lic: f64,
    inc: f64,
    aus: f64,
) -> Decimal {
    if !tiene_obra {
        return Decimal::ONE;
    }
    if dias_cedula <= 0 || dias_obra <= 0 {
        return Decimal::ZERO;
    }
    let efectivos = dias_efectivos(dias_cedula, lic, inc, aus);
    let dias_obra_limitados = dias_obra.min(efectivos);
    Decimal::from(dias_obra_limitados) / Decimal::from(dias_cedula)
}

fn redondear_2(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

fn a_f64(v: Decimal) -> f64 {
    v.to_f64().unwrap_or(0.0)
}

/// Aplica el factor a un importe original y redondea a 2 decimales.
fn prorratear_rama(original: f64, factor: Decimal) -> Decimal {
    redondear_2(decimal_desde_f64(original) * factor)
}

/// Prorratea todos los importes de un tramo con el factor dado.
/// Cada rama se redondea individualmente; `patronal`/`obrera` son sumas de esas ramas redondeadas.
pub fn prorratear(originales: &ImportesTramo, factor: Decimal) -> ImportesTramo {
    let cf = prorratear_rama(originales.cf, factor);
    let exc_pat = prorratear_rama(originales.exc_pat, factor);
    let exc_obr = prorratear_rama(originales.exc_obr, factor);
    let pd_pat = prorratear_rama(originales.pd_pat, factor);
    let pd_obr = prorratear_rama(originales.pd_obr, factor);
    let gmp_pat = prorratear_rama(originales.gmp_pat, factor);
    let gmp_obr = prorratear_rama(originales.gmp_obr, factor);
    let rt = prorratear_rama(originales.rt, factor);
    let iv_pat = prorratear_rama(originales.iv_pat, factor);
    let iv_obr = prorratear_rama(originales.iv_obr, factor);
    let gps = prorratear_rama(originales.gps, factor);
    let subtotal = prorratear_rama(originales.subtotal, factor);

    let patronal = cf + exc_pat + pd_pat + gmp_pat + rt + iv_pat + gps;
    let obrera = exc_obr + pd_obr + gmp_obr + iv_obr;

    ImportesTramo {
        cf: a_f64(cf),
        exc_pat: a_f64(exc_pat),
        exc_obr: a_f64(exc_obr),
        pd_pat: a_f64(pd_pat),
        pd_obr: a_f64(pd_obr),
        gmp_pat: a_f64(gmp_pat),
        gmp_obr: a_f64(gmp_obr),
        rt: a_f64(rt),
        iv_pat: a_f64(iv_pat),
        iv_obr: a_f64(iv_obr),
        gps: a_f64(gps),
        patronal: a_f64(patronal),
        obrera: a_f64(obrera),
        subtotal: a_f64(subtotal),
    }
}

/// $1: uuid[] con ids de public.reportes_liquidaciones a recalcular.
/// Replica exactamente la regla de `factor_prorrateo` y `prorratear`:
/// factor = 1 sin obra; 0 si dias_cedula<=0 o dias_obra<=0; en otro caso
/// min(dias_obra, ROUND(dias - (lic+inc+aus)) limitado a >=0) / dias.
/// Cada rama y el subtotal se redondean con ROUND(numeric, 2) (mitad lejos de cero).
/// Los tramos sin renglón de cédula correspondiente no se modifican (JOIN interno).
pub const SQL_RECALCULAR_PRORRATEO: &str = r#"
UPDATE public.reportes_detalles_liquidaciones r
SET cf = p.cf::float8,
    exc_pat = p.exc_pat::float8,
    exc_obr = p.exc_obr::float8,
    pd_pat = p.pd_pat::float8,
    pd_obr = p.pd_obr::float8,
    gmp_pat = p.gmp_pat::float8,
    gmp_obr = p.gmp_obr::float8,
    rt = p.rt::float8,
    iv_pat = p.iv_pat::float8,
    iv_obr = p.iv_obr::float8,
    gps = p.gps::float8,
    patronal = (p.cf + p.exc_pat + p.pd_pat + p.gmp_pat + p.rt + p.iv_pat + p.gps)::float8,
    obrera = (p.exc_obr + p.pd_obr + p.gmp_obr + p.iv_obr)::float8,
    subtotal = p.subtotal::float8
FROM (
    SELECT r2.id,
        ROUND(COALESCE(d.cf, 0)::numeric * f.factor, 2) AS cf,
        ROUND(COALESCE(d.exc_pat, 0)::numeric * f.factor, 2) AS exc_pat,
        ROUND(COALESCE(d.exc_obr, 0)::numeric * f.factor, 2) AS exc_obr,
        ROUND(COALESCE(d.pd_pat, 0)::numeric * f.factor, 2) AS pd_pat,
        ROUND(COALESCE(d.pd_obr, 0)::numeric * f.factor, 2) AS pd_obr,
        ROUND(COALESCE(d.gmp_pat, 0)::numeric * f.factor, 2) AS gmp_pat,
        ROUND(COALESCE(d.gmp_obr, 0)::numeric * f.factor, 2) AS gmp_obr,
        ROUND(COALESCE(d.rt, 0)::numeric * f.factor, 2) AS rt,
        ROUND(COALESCE(d.iv_pat, 0)::numeric * f.factor, 2) AS iv_pat,
        ROUND(COALESCE(d.iv_obr, 0)::numeric * f.factor, 2) AS iv_obr,
        ROUND(COALESCE(d.gps, 0)::numeric * f.factor, 2) AS gps,
        ROUND(COALESCE(d.subtotal, 0)::numeric * f.factor, 2) AS subtotal
    FROM public.reportes_detalles_liquidaciones r2
    JOIN public.reportes_liquidaciones rl ON rl.id = r2.id_reporte_liquidacion
    JOIN LATERAL (
        SELECT d.*
        FROM public.detalles_liquidaciones d
        WHERE d.id_liquidacion = rl.id_liquidacion
          AND COALESCE(BTRIM(LOWER(d.clave)), '') = COALESCE(BTRIM(LOWER(r2.clave)), '')
          AND d.fecha IS NOT DISTINCT FROM r2.fecha
        ORDER BY d.id
        LIMIT 1
    ) d ON true
    CROSS JOIN LATERAL (
        SELECT CASE
            WHEN r2.id_registro_obra IS NULL THEN 1::numeric
            WHEN COALESCE(r2.dias, 0) <= 0 OR COALESCE(r2.dias_registro_obra, 0) <= 0 THEN 0::numeric
            ELSE LEAST(
                    r2.dias_registro_obra::numeric,
                    GREATEST(ROUND(r2.dias::numeric - (COALESCE(r2.lic, 0) + COALESCE(r2.inc, 0) + COALESCE(r2.aus, 0))::numeric), 0)
                 ) / r2.dias::numeric
        END AS factor
    ) f
    WHERE r2.id_reporte_liquidacion = ANY($1)
) p
WHERE r.id = p.id
"#;

/// Recalcula en la transacción/cliente dado los importes prorrateados de los reportes indicados.
/// Devuelve el número de renglones actualizados.
pub fn recalcular_prorrateo_tx<C: postgres::GenericClient>(
    c: &mut C,
    ids_reportes: &[uuid::Uuid],
) -> Result<u64, String> {
    if ids_reportes.is_empty() {
        return Ok(0);
    }
    c.execute(SQL_RECALCULAR_PRORRATEO, &[&ids_reportes])
        .map_err(|e| crate::db::schema::format_pg_error(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).expect("decimal válido")
    }

    fn tramo_base() -> ImportesTramo {
        ImportesTramo {
            cf: 100.0,
            exc_pat: 10.0,
            exc_obr: 5.0,
            pd_pat: 2.0,
            pd_obr: 1.0,
            gmp_pat: 3.0,
            gmp_obr: 0.5,
            rt: 4.0,
            iv_pat: 6.0,
            iv_obr: 2.0,
            gps: 7.0,
            patronal: 0.0,
            obrera: 0.0,
            subtotal: 1000.0,
        }
    }

    #[test]
    fn sin_obra_factor_uno_e_importes_iguales() {
        let f = factor_prorrateo(false, 30, 0, 0.0, 0.0, 0.0);
        assert_eq!(f, Decimal::ONE);
        let r = prorratear(&tramo_base(), f);
        assert_eq!(r.cf, 100.0);
        assert_eq!(r.subtotal, 1000.0);
        assert_eq!(r.patronal, 132.0);
        assert_eq!(r.obrera, 8.5);
    }

    #[test]
    fn obra_con_cero_dias_todo_cero() {
        let f = factor_prorrateo(true, 30, 0, 0.0, 0.0, 0.0);
        assert_eq!(f, Decimal::ZERO);
        let r = prorratear(&tramo_base(), f);
        assert_eq!(r, ImportesTramo::default());
    }

    #[test]
    fn dias_cedula_cero_con_obra_es_factor_cero_sin_panico() {
        let f = factor_prorrateo(true, 0, 5, 0.0, 0.0, 0.0);
        assert_eq!(f, Decimal::ZERO);
    }

    #[test]
    fn quince_de_treinta_es_mitad_exacta() {
        let f = factor_prorrateo(true, 30, 15, 0.0, 0.0, 0.0);
        let r = prorratear(&tramo_base(), f);
        assert_eq!(r.cf, 50.0);
    }

    #[test]
    fn un_tercio_redondea_a_dos_decimales() {
        let f = factor_prorrateo(true, 3, 1, 0.0, 0.0, 0.0);
        let mut t = tramo_base();
        t.cf = 100.0;
        t.subtotal = 1000.0;
        let r = prorratear(&t, f);
        assert_eq!(r.cf, 33.33);
        assert_eq!(r.subtotal, 333.33);
    }

    #[test]
    fn redondeo_mitad_lejos_de_cero() {
        // 0.01 * 0.5 = 0.005 -> 0.01
        let t = ImportesTramo { cf: 0.01, ..Default::default() };
        let r = prorratear(&t, dec("0.5"));
        assert_eq!(r.cf, 0.01);
        // -0.01 * 0.5 = -0.005 -> -0.01
        let t_neg = ImportesTramo { cf: -0.01, ..Default::default() };
        let r_neg = prorratear(&t_neg, dec("0.5"));
        assert_eq!(r_neg.cf, -0.01);
    }

    #[test]
    fn conversion_f64_replica_float8_numeric() {
        assert_eq!(decimal_desde_f64(1.005).normalize().to_string(), "1.005");
        assert_eq!(decimal_desde_f64(0.1 + 0.2).normalize().to_string(), "0.3");
        assert_eq!(decimal_desde_f64(0.0), Decimal::ZERO);
        assert_eq!(decimal_desde_f64(-2.5), dec("-2.5"));
    }

    #[test]
    fn conversion_no_finitos_son_cero() {
        assert_eq!(decimal_desde_f64(f64::NAN), Decimal::ZERO);
        assert_eq!(decimal_desde_f64(f64::INFINITY), Decimal::ZERO);
        assert_eq!(decimal_desde_f64(f64::NEG_INFINITY), Decimal::ZERO);
    }

    #[test]
    fn incidencias_reducen_dias_efectivos() {
        // 30 - 2.4 = 27.6 -> 28 días efectivos
        assert_eq!(dias_efectivos(30, 2.4, 0.0, 0.0), 28);
        let f = factor_prorrateo(true, 30, 30, 2.4, 0.0, 0.0);
        assert_eq!(f, Decimal::from(28) / Decimal::from(30));
    }

    #[test]
    fn dias_obra_mayor_que_efectivos_se_limita() {
        let f = factor_prorrateo(true, 30, 30, 2.4, 0.0, 0.0);
        let f_limitado = factor_prorrateo(true, 30, 28, 2.4, 0.0, 0.0);
        assert_eq!(f, f_limitado);
    }

    #[test]
    fn dias_efectivos_nunca_negativos() {
        assert_eq!(dias_efectivos(10, 20.0, 0.0, 0.0), 0);
        let f = factor_prorrateo(true, 10, 5, 20.0, 0.0, 0.0);
        assert_eq!(f, Decimal::ZERO);
    }

    #[test]
    fn patronal_y_obrera_son_suma_de_ramas_redondeadas() {
        // Cada rama 0.03 * 0.5 = 0.015 -> 0.02; suma patronal = 0.04 (no 0.03)
        let t = ImportesTramo { cf: 0.03, gps: 0.03, exc_obr: 0.03, ..Default::default() };
        let r = prorratear(&t, dec("0.5"));
        assert_eq!(r.patronal, 0.04);
        assert_eq!(r.obrera, 0.02);
    }

    #[test]
    fn sql_contiene_regla_esperada() {
        assert!(SQL_RECALCULAR_PRORRATEO.contains("ROUND("));
        assert!(SQL_RECALCULAR_PRORRATEO.contains("IS NOT DISTINCT FROM"));
        assert!(SQL_RECALCULAR_PRORRATEO.contains("ANY($1)"));
        assert!(!SQL_RECALCULAR_PRORRATEO.contains("MidpointAwayFromZero"));
    }
}
