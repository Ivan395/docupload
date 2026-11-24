//! Posibles empleados duplicados de un cliente y su corrección desde la app.
//!
//! Al cargar cédulas o nóminas con errores de captura (por ejemplo, un NSS mal escrito) se puede
//! crear un empleado nuevo para alguien que ya existía. Aquí se detectan esos pares y el usuario
//! decide: fusionarlos (con las mismas reglas que `scripts/fusionar_empleados_nss.py`) o marcarlos
//! como personas distintas para que no se vuelvan a proponer.

use std::collections::{BTreeSet, HashMap, HashSet};

use postgres::{Client, GenericClient};
use uuid::Uuid;

use crate::advertencias_carga::{clave_nombre, EmpleadoCatalogo};
use crate::db::schema::format_pg_error;
use crate::validation::is_generic_sat_identifier;

/// Pares descartados por el usuario ("no son la misma persona"). Solo aditivo.
pub(crate) const SQL_DUPLICADOS_DESCARTADOS: &str = r#"
CREATE TABLE IF NOT EXISTS public.empleados_duplicados_descartados (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    id_cliente uuid NOT NULL REFERENCES public.clientes(id) ON UPDATE CASCADE ON DELETE CASCADE,
    id_empleado_a uuid NOT NULL REFERENCES public.empleados(id) ON UPDATE CASCADE ON DELETE CASCADE,
    id_empleado_b uuid NOT NULL REFERENCES public.empleados(id) ON UPDATE CASCADE ON DELETE CASCADE,
    created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uq_empleados_duplicados_descartados UNIQUE (id_empleado_a, id_empleado_b)
);
CREATE INDEX IF NOT EXISTS idx_empleados_duplicados_descartados_cliente ON public.empleados_duplicados_descartados(id_cliente);
"#;

/// Columnas comparadas para decidir si dos liquidaciones del mismo periodo son la misma.
const DET_COLS: &str = "clave, fecha, dias, sdi, lic, inc, aus, cf, exc_pat, exc_obr, pd_pat, pd_obr, \
                        gmp_pat, gmp_obr, rt, iv_pat, iv_obr, gps, patronal, obrera, subtotal";

/// Empleado con sus conteos, para mostrarlo en el par.
#[derive(Debug, Clone, PartialEq)]
pub struct EmpleadoDuplicado {
    pub id: Uuid,
    pub nombre: String,
    pub nss: String,
    pub rfc: String,
    pub curp: String,
    pub liquidaciones: i64,
    pub nominas: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParDuplicado {
    pub a: EmpleadoDuplicado,
    pub b: EmpleadoDuplicado,
    pub motivo: String,
}

fn par_ordenado(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
    if a < b { (a, b) } else { (b, a) }
}

fn curp_real(c: &str) -> bool {
    c.len() == 18 && !is_generic_sat_identifier(c)
}

fn rfc_real(r: &str) -> bool {
    r.len() == 13 && !is_generic_sat_identifier(r)
}

/// Raíz de identidad: iniciales + fecha de nacimiento (10 primeras posiciones de CURP o RFC).
fn raiz_identidad(e: &EmpleadoCatalogo) -> Option<String> {
    if curp_real(&e.curp) {
        Some(e.curp[..10].to_string())
    } else if rfc_real(&e.rfc) {
        Some(e.rfc[..10].to_string())
    } else {
        None
    }
}

/// Detecta pares de posibles duplicados (lógica pura). Devuelve (índice a, índice b, motivo).
///
/// Criterios: misma CURP; mismo RFC real; mismo nombre (sin importar orden ni acentos); o misma
/// raíz RFC/CURP (iniciales + fecha de nacimiento) y misma primera palabra del nombre.
/// Con "mismo nombre" o "misma raíz" se ignoran los pares que tienen CURPs válidas distintas
/// (son homónimos, personas diferentes).
pub fn detectar_pares(empleados: &[EmpleadoCatalogo], descartados: &HashSet<(Uuid, Uuid)>) -> Vec<(usize, usize, String)> {
    let mut motivo_por_par: HashMap<(usize, usize), String> = HashMap::new();
    let mut agrupar = |grupos: HashMap<String, Vec<usize>>, motivo: &str, exigir_curp_compatible: bool| {
        for idx in grupos.values().filter(|v| v.len() > 1) {
            for i in 0..idx.len() {
                for j in (i + 1)..idx.len() {
                    let (x, y) = (idx[i].min(idx[j]), idx[i].max(idx[j]));
                    let (ex, ey) = (&empleados[x], &empleados[y]);
                    if exigir_curp_compatible && curp_real(&ex.curp) && curp_real(&ey.curp) && ex.curp != ey.curp {
                        continue;
                    }
                    if descartados.contains(&par_ordenado(ex.id, ey.id)) {
                        continue;
                    }
                    motivo_por_par.entry((x, y)).or_insert_with(|| motivo.to_string());
                }
            }
        }
    };

    let mut por_curp: HashMap<String, Vec<usize>> = HashMap::new();
    let mut por_rfc: HashMap<String, Vec<usize>> = HashMap::new();
    let mut por_nombre: HashMap<String, Vec<usize>> = HashMap::new();
    let mut por_raiz: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, e) in empleados.iter().enumerate() {
        if curp_real(&e.curp) {
            por_curp.entry(e.curp.clone()).or_default().push(i);
        }
        if rfc_real(&e.rfc) {
            por_rfc.entry(e.rfc.clone()).or_default().push(i);
        }
        let n = clave_nombre(&e.nombre);
        if n.split_whitespace().count() >= 2 {
            por_nombre.entry(n).or_default().push(i);
        }
        if let (Some(r), Some(p)) = (raiz_identidad(e), e.nombre.split_whitespace().next()) {
            por_raiz.entry(format!("{}|{}", r, clave_nombre(p))).or_default().push(i);
        }
    }
    agrupar(por_curp, "Misma CURP", false);
    agrupar(por_rfc, "Mismo RFC", false);
    agrupar(por_nombre, "Mismo nombre", true);
    agrupar(por_raiz, "Misma fecha de nacimiento e iniciales (RFC/CURP) y mismo primer nombre o apellido", true);

    let mut pares: Vec<(usize, usize, String)> = motivo_por_par.into_iter().map(|((a, b), m)| (a, b, m)).collect();
    pares.sort_by(|x, y| empleados[x.0].nombre.cmp(&empleados[y.0].nombre).then(x.0.cmp(&y.0)).then(x.1.cmp(&y.1)));
    pares
}

/// Lista los posibles duplicados de un cliente.
pub fn listar_posibles_duplicados<C: GenericClient>(client: &mut C, id_cliente: Uuid) -> Result<Vec<ParDuplicado>, String> {
    let filas = client
        .query(
            "SELECT e.id, COALESCE(e.nombre, ''), COALESCE(BTRIM(e.nss), ''), COALESCE(BTRIM(e.rfc), ''), COALESCE(BTRIM(e.curp), ''),
                    (SELECT COUNT(*) FROM public.liquidaciones l WHERE l.id_empleado = e.id),
                    (SELECT COUNT(*) FROM public.nominas n WHERE n.id_empleado = e.id)
             FROM public.empleados e WHERE e.id_cliente = $1",
            &[&id_cliente],
        )
        .map_err(|e| format!("Error consultando empleados: {}", format_pg_error(&e)))?;
    let mut resumen: Vec<EmpleadoDuplicado> = Vec::with_capacity(filas.len());
    let mut catalogo: Vec<EmpleadoCatalogo> = Vec::with_capacity(filas.len());
    for r in &filas {
        let e = EmpleadoDuplicado {
            id: r.get(0),
            nombre: crate::advertencias_carga::normalizar_espacios(&r.get::<_, String>(1).to_uppercase()),
            nss: r.get::<_, String>(2),
            rfc: r.get::<_, String>(3).to_uppercase(),
            curp: r.get::<_, String>(4).to_uppercase(),
            liquidaciones: r.get(5),
            nominas: r.get(6),
        };
        catalogo.push(EmpleadoCatalogo {
            id: e.id,
            nombre: e.nombre.clone(),
            nss: e.nss.clone(),
            rfc: e.rfc.clone(),
            curp: e.curp.clone(),
        });
        resumen.push(e);
    }
    let descartados: HashSet<(Uuid, Uuid)> = client
        .query(
            "SELECT id_empleado_a, id_empleado_b FROM public.empleados_duplicados_descartados WHERE id_cliente = $1",
            &[&id_cliente],
        )
        .map(|rows| rows.iter().map(|r| par_ordenado(r.get(0), r.get(1))).collect())
        .unwrap_or_default();
    Ok(detectar_pares(&catalogo, &descartados)
        .into_iter()
        .map(|(a, b, motivo)| ParDuplicado { a: resumen[a].clone(), b: resumen[b].clone(), motivo })
        .collect())
}

/// Cliente dueño de una revisión (para revisar duplicados tras una carga).
pub fn cliente_de_revision<C: GenericClient>(client: &mut C, id_revision: Uuid) -> Result<Option<Uuid>, String> {
    client
        .query_opt(
            "SELECT rp.id_cliente FROM public.revisiones r JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal WHERE r.id = $1",
            &[&id_revision],
        )
        .map(|r| r.map(|r| r.get(0)))
        .map_err(|e| format!("Error consultando el cliente de la revisión: {}", format_pg_error(&e)))
}

/// Marca un par como personas distintas.
pub fn descartar_par<C: GenericClient>(client: &mut C, id_cliente: Uuid, a: Uuid, b: Uuid) -> Result<(), String> {
    let (x, y) = par_ordenado(a, b);
    client
        .execute(
            "INSERT INTO public.empleados_duplicados_descartados (id_cliente, id_empleado_a, id_empleado_b)
             VALUES ($1, $2, $3) ON CONFLICT (id_empleado_a, id_empleado_b) DO NOTHING",
            &[&id_cliente, &x, &y],
        )
        .map(|_| ())
        .map_err(|e| format!("No se pudo guardar la decisión: {}", format_pg_error(&e)))
}

/// Resultado de una fusión.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResumenFusion {
    pub liquidaciones_reasignadas: u64,
    pub liquidaciones_unificadas: u64,
    pub nominas_reasignadas: u64,
    pub equivalencia_registrada: bool,
}

/// Fusiona el empleado `origen` en `destino` (mismo cliente) en una sola transacción.
///
/// Reglas (las de `scripts/fusionar_empleados_nss.py`):
/// - Liquidación del origen sin par en el destino (mismo RP y periodo): se reasigna al destino.
/// - Con par: detalles y obras (folio y días) deben ser idénticos o se cancela TODO; si lo son,
///   la del destino adopta archivo y vinculaciones de la del origen y la del origen se borra.
/// - Nóminas, reportes de nóminas y NSS alternos del origen pasan al destino.
/// - El NSS del origen queda como NSS alterno del destino (las cargas futuras no lo duplican).
/// - El destino completa RFC/CURP vacíos con los del origen; luego se borra el origen.
pub fn fusionar_empleados(client: &mut Client, origen: Uuid, destino: Uuid, motivo: &str) -> Result<ResumenFusion, String> {
    if origen == destino {
        return Err("Elige dos empleados distintos.".to_string());
    }
    let pg = |e: postgres::Error| format_pg_error(&e);
    let mut tx = client.transaction().map_err(pg)?;
    let mut resumen = ResumenFusion::default();

    let leer = |tx: &mut postgres::Transaction, id: Uuid| -> Result<(String, Uuid, String), String> {
        tx.query_opt(
            "SELECT COALESCE(BTRIM(nss), ''), id_cliente, COALESCE(nombre, '') FROM public.empleados WHERE id = $1 FOR UPDATE",
            &[&id],
        )
        .map_err(|e| format_pg_error(&e))?
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .ok_or_else(|| "Uno de los empleados ya no existe (quizá se fusionó o eliminó). Actualiza la lista.".to_string())
    };
    let (nss_o, cli_o, _nom_o) = leer(&mut tx, origen)?;
    let (_nss_d, cli_d, _nom_d) = leer(&mut tx, destino)?;
    if cli_o != cli_d {
        return Err("Los empleados pertenecen a clientes distintos: no se pueden fusionar.".to_string());
    }

    let liqs = tx
        .query(
            "SELECT id, id_registro_patronal, anio_periodo, mes_periodo_int, nombre_archivo, id_archivo, origen
             FROM public.liquidaciones WHERE id_empleado = $1 ORDER BY anio_periodo, mes_periodo_int",
            &[&origen],
        )
        .map_err(pg)?;
    for l in liqs {
        let lo: Uuid = l.get(0);
        let rp: Uuid = l.get(1);
        let anio: Option<i16> = l.get(2);
        let mes: Option<i16> = l.get(3);
        let etiqueta = format!("{}-{:02}", anio.unwrap_or(0), mes.unwrap_or(0));
        let ld: Option<Uuid> = tx
            .query_opt(
                "SELECT id FROM public.liquidaciones WHERE id_empleado = $1 AND id_registro_patronal = $2
                 AND anio_periodo IS NOT DISTINCT FROM $3 AND mes_periodo_int IS NOT DISTINCT FROM $4",
                &[&destino, &rp, &anio, &mes],
            )
            .map_err(pg)?
            .map(|r| r.get(0));
        let Some(ld) = ld else {
            tx.execute("UPDATE public.reportes_liquidaciones SET id_empleado = $1 WHERE id_liquidacion = $2", &[&destino, &lo])
                .map_err(pg)?;
            tx.execute("UPDATE public.liquidaciones SET id_empleado = $1 WHERE id = $2", &[&destino, &lo]).map_err(pg)?;
            resumen.liquidaciones_reasignadas += 1;
            continue;
        };

        // Las obras se comparan por folio y días (el id del registro de obra cambia por revisión)
        let vin = "(SELECT BTRIM(ro.numero), v.dias_registro_obra FROM public.vinculaciones_obras_liquidaciones v \
                   JOIN public.registros_obras ro ON ro.id = v.id_registro_obra WHERE v.id_liquidacion = $X)";
        let det = format!("(SELECT {} FROM public.detalles_liquidaciones WHERE id_liquidacion = $X)", DET_COLS);
        for (nombre, sub) in [("los importes (detalles)", det.as_str()), ("las obras vinculadas", vin)] {
            let a = sub.replace("$X", "$1");
            let b = sub.replace("$X", "$2");
            let sql = format!("SELECT COUNT(*) FROM (({a} EXCEPT ALL {b}) UNION ALL ({b} EXCEPT ALL {a})) d");
            let dif: i64 = tx.query_one(sql.as_str(), &[&lo, &ld]).map_err(pg)?.get(0);
            if dif > 0 {
                return Err(format!(
                    "En el periodo {} ambos empleados tienen liquidaciones con {} distintos, así que no se pueden unir automáticamente. \
                     Revisa las cédulas de ese periodo. No se hizo ningún cambio.",
                    etiqueta, nombre
                ));
            }
        }
        let arch: Option<String> = l.get(4);
        let id_arch: Option<Uuid> = l.get(5);
        let orig: Option<String> = l.get(6);
        tx.execute(
            "UPDATE public.liquidaciones SET nombre_archivo = $1, id_archivo = $2, origen = $3 WHERE id = $4",
            &[&arch, &id_arch, &orig, &ld],
        )
        .map_err(pg)?;
        tx.execute("DELETE FROM public.vinculaciones_obras_liquidaciones WHERE id_liquidacion = $1", &[&ld]).map_err(pg)?;
        tx.execute("UPDATE public.vinculaciones_obras_liquidaciones SET id_liquidacion = $1 WHERE id_liquidacion = $2", &[&ld, &lo])
            .map_err(pg)?;
        let filas = tx
            .query("SELECT id, id_revision FROM public.reportes_liquidaciones WHERE id_liquidacion = $1", &[&lo])
            .map_err(pg)?;
        for f in filas {
            let rl: Uuid = f.get(0);
            let rev: Option<Uuid> = f.get(1);
            let existe = tx
                .query_opt(
                    "SELECT 1 FROM public.reportes_liquidaciones WHERE id_liquidacion = $1 AND id_revision IS NOT DISTINCT FROM $2",
                    &[&ld, &rev],
                )
                .map_err(pg)?
                .is_some();
            if existe {
                tx.execute("DELETE FROM public.reportes_liquidaciones WHERE id = $1", &[&rl]).map_err(pg)?;
            } else {
                tx.execute(
                    "UPDATE public.reportes_liquidaciones SET id_liquidacion = $1, id_empleado = $2 WHERE id = $3",
                    &[&ld, &destino, &rl],
                )
                .map_err(pg)?;
            }
        }
        tx.execute("DELETE FROM public.liquidaciones WHERE id = $1", &[&lo]).map_err(pg)?;
        resumen.liquidaciones_unificadas += 1;
    }

    // Nóminas y su papel de trabajo
    resumen.nominas_reasignadas = tx
        .execute("UPDATE public.nominas SET id_empleado = $1 WHERE id_empleado = $2", &[&destino, &origen])
        .map_err(pg)?;
    tx.execute("UPDATE public.reportes_nominas SET id_empleado = $1 WHERE id_empleado = $2", &[&destino, &origen])
        .map_err(pg)?;
    // Filas de reporte que aún apunten al origen (sin liquidación)
    tx.execute("UPDATE public.reportes_liquidaciones SET id_empleado = $1 WHERE id_empleado = $2", &[&destino, &origen])
        .map_err(pg)?;
    // NSS alternos del origen y el propio NSS del origen
    tx.execute("UPDATE public.empleados_nss_equivalencias SET id_empleado = $1 WHERE id_empleado = $2", &[&destino, &origen])
        .map_err(pg)?;
    if nss_o.len() == 11 && nss_o.chars().all(|c| c.is_ascii_digit()) {
        let n = tx
            .execute(
                "INSERT INTO public.empleados_nss_equivalencias (id_cliente, nss_alterno, id_empleado, motivo)
                 VALUES ($1, $2, $3, $4) ON CONFLICT (id_cliente, nss_alterno) DO UPDATE SET id_empleado = EXCLUDED.id_empleado",
                &[&cli_d, &nss_o, &destino, &motivo],
            )
            .map_err(pg)?;
        resumen.equivalencia_registrada = n > 0;
    }

    // Completar datos vacíos del destino con los del origen, y borrar el origen
    tx.execute(
        r#"UPDATE public.empleados d SET
              rfc = CASE WHEN (d.rfc IS NULL OR BTRIM(d.rfc) = '' OR BTRIM(d.rfc) IN ('XAXX010101000', 'XEXX010101000'))
                         AND LENGTH(BTRIM(o.rfc)) = 13 AND BTRIM(o.rfc) NOT IN ('XAXX010101000', 'XEXX010101000')
                         THEN o.rfc ELSE d.rfc END,
              curp = CASE WHEN (d.curp IS NULL OR BTRIM(d.curp) = '') AND LENGTH(BTRIM(o.curp)) = 18 THEN o.curp ELSE d.curp END
           FROM public.empleados o
           WHERE d.id = $1 AND o.id = $2"#,
        &[&destino, &origen],
    )
    .map_err(pg)?;
    tx.execute("DELETE FROM public.empleados_duplicados_descartados WHERE id_empleado_a = $1 OR id_empleado_b = $1", &[&origen])
        .map_err(pg)?;
    tx.execute("DELETE FROM public.empleados WHERE id = $1", &[&origen]).map_err(pg)?;

    tx.commit().map_err(pg)?;
    Ok(resumen)
}

/// Ids de empleados involucrados en los pares (para avisar cuántas personas revisar).
pub fn empleados_involucrados(pares: &[ParDuplicado]) -> BTreeSet<Uuid> {
    pares.iter().flat_map(|p| [p.a.id, p.b.id]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(nombre: &str, nss: &str, rfc: &str, curp: &str) -> EmpleadoCatalogo {
        EmpleadoCatalogo { id: Uuid::now_v7(), nombre: nombre.into(), nss: nss.into(), rfc: rfc.into(), curp: curp.into() }
    }

    #[test]
    fn detecta_por_curp_nombre_y_raiz() {
        let v = vec![
            e("JUAN PEREZ LOPEZ", "1", "", "PELJ800101HDFRPN09"),
            e("JUAN PERES LOPEZ", "2", "", "PELJ800101HDFRPN09"), // misma CURP
            e("ANA RUIZ", "3", "", ""),
            e("RUIZ ANA", "4", "", ""), // mismo nombre en otro orden
            e("MARIA GOMEZ", "5", "GOMM900202AB1", ""),
            e("MARIA GOMES", "6", "GOMM900202XY2", ""), // misma raíz RFC y primera palabra
            e("OTRO NOMBRE", "7", "", ""),
        ];
        let pares = detectar_pares(&v, &HashSet::new());
        let motivos: Vec<&str> = pares.iter().map(|p| p.2.as_str()).collect();
        assert_eq!(pares.len(), 3, "{:?}", pares);
        assert!(motivos.contains(&"Misma CURP"));
        assert!(motivos.contains(&"Mismo nombre"));
        assert!(motivos.iter().any(|m| m.starts_with("Misma fecha")));
    }

    #[test]
    fn homonimos_con_curp_distinta_no_son_duplicados() {
        let v = vec![
            e("JUAN PEREZ", "1", "", "PEXJ800101HDFRRN01"),
            e("JUAN PEREZ", "2", "", "PEXJ900101HDFRRN02"),
        ];
        assert!(detectar_pares(&v, &HashSet::new()).is_empty());
    }

    #[test]
    fn descartados_no_se_repiten() {
        let v = vec![e("ANA RUIZ", "1", "", ""), e("ANA RUIZ", "2", "", "")];
        let d = HashSet::from([par_ordenado(v[0].id, v[1].id)]);
        assert!(detectar_pares(&v, &d).is_empty());
        assert_eq!(detectar_pares(&v, &HashSet::new()).len(), 1);
    }
}
