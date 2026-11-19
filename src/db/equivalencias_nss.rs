//! Equivalencias de NSS: NSS alternos que la carga de cédulas y nóminas resuelve a un empleado
//! existente (tabla `public.empleados_nss_equivalencias`, ver `schema::SQL_EQUIVALENCIAS_NSS`).
//!
//! Caso típico: el IMSS reporta a un trabajador con un NSS distinto al del catálogo y el analista
//! confirma que es la misma persona. Sin la equivalencia, cada carga crearía un empleado duplicado.

use postgres::GenericClient;
use uuid::Uuid;

/// Fila de equivalencia lista para mostrarse.
#[derive(Debug, Clone, PartialEq)]
pub struct EquivalenciaNss {
    pub id: Uuid,
    pub nss_alterno: String,
    pub motivo: String,
    /// Fecha de alta en formato DD/MM/AAAA.
    pub creada: String,
}

/// Deja solo los dígitos del NSS y exige 11 dígitos con dígito verificador IMSS válido.
/// No se aplica la coherencia de años de `validation::validate_nss`: el NSS alterno lo dicta la
/// cédula del IMSS y debe aceptarse tal cual mientras sea un NSS bien formado.
pub fn normalizar_nss_alterno(nss: &str) -> Result<String, String> {
    let limpio: String = nss.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '.').collect();
    if limpio.len() != 11 || !limpio.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("El NSS alterno debe tener 11 dígitos; se recibió '{}'.", nss.trim()));
    }
    let d: Vec<u32> = limpio.chars().filter_map(|c| c.to_digit(10)).collect();
    let suma: u32 = (0..10)
        .map(|i| {
            let p = d[i] * if i % 2 == 0 { 1 } else { 2 };
            p / 10 + p % 10
        })
        .sum();
    if (10 - suma % 10) % 10 != d[10] {
        return Err(format!("El NSS alterno '{}' tiene un dígito verificador inválido.", limpio));
    }
    Ok(limpio)
}

/// Equivalencias registradas para un empleado, de la más reciente a la más antigua.
pub fn listar_equivalencias<C: GenericClient>(
    client: &mut C,
    id_empleado: Uuid,
) -> Result<Vec<EquivalenciaNss>, String> {
    let rows = client
        .query(
            "SELECT id, nss_alterno, COALESCE(motivo, ''), COALESCE(to_char(created_at, 'DD/MM/YYYY'), '') \
             FROM public.empleados_nss_equivalencias WHERE id_empleado = $1 \
             ORDER BY created_at DESC, nss_alterno",
            &[&id_empleado],
        )
        .map_err(|e| format!("Error consultando equivalencias de NSS: {}", crate::db::format_pg_error(&e)))?;
    Ok(rows
        .iter()
        .map(|r| EquivalenciaNss {
            id: r.get(0),
            nss_alterno: r.get(1),
            motivo: r.get(2),
            creada: r.get(3),
        })
        .collect())
}

/// Registra `nss` como NSS alterno del empleado. Rechaza (sin cambiar nada):
/// - el NSS principal del propio empleado;
/// - un NSS que ya es el principal de otro empleado del cliente (hay que fusionarlos primero:
///   la equivalencia sola no movería sus liquidaciones y el duplicado seguiría contando);
/// - un NSS ya registrado como alterno, de este o de otro empleado.
pub fn agregar_equivalencia<C: GenericClient>(
    client: &mut C,
    id_empleado: Uuid,
    nss: &str,
    motivo: &str,
) -> Result<String, String> {
    let nss = normalizar_nss_alterno(nss)?;
    let pg = |e: postgres::Error| format!("Error registrando equivalencia de NSS: {}", crate::db::format_pg_error(&e));

    let emp = client
        .query_opt(
            "SELECT id_cliente, COALESCE(BTRIM(nss), '') FROM public.empleados WHERE id = $1",
            &[&id_empleado],
        )
        .map_err(pg)?
        .ok_or_else(|| "El empleado ya no existe.".to_string())?;
    let id_cliente: Uuid = emp.get(0);
    let nss_propio: String = emp.get(1);
    if nss_propio == nss {
        return Err(format!("{} ya es el NSS principal de este empleado.", nss));
    }

    if let Some(otro) = client
        .query_opt(
            "SELECT e.nombre, (SELECT COUNT(*) FROM public.liquidaciones l WHERE l.id_empleado = e.id) \
             FROM public.empleados e WHERE e.id_cliente = $1 AND BTRIM(e.nss) = $2 AND e.id <> $3 LIMIT 1",
            &[&id_cliente, &nss, &id_empleado],
        )
        .map_err(pg)?
    {
        let nombre: String = otro.get(0);
        let liq: i64 = otro.get(1);
        return Err(format!(
            "El NSS {} pertenece a otro empleado del cliente ({}, {} liquidaciones). \
             Fusiona primero ambos empleados (scripts/fusionar_empleados_nss.py) para no duplicar importes.",
            nss, nombre, liq
        ));
    }

    if let Some(dup) = client
        .query_opt(
            "SELECT e.nombre, eq.id_empleado = $3 FROM public.empleados_nss_equivalencias eq \
             JOIN public.empleados e ON e.id = eq.id_empleado \
             WHERE eq.id_cliente = $1 AND eq.nss_alterno = $2",
            &[&id_cliente, &nss, &id_empleado],
        )
        .map_err(pg)?
    {
        let nombre: String = dup.get(0);
        let mismo: bool = dup.get(1);
        return Err(if mismo {
            format!("{} ya está registrado como NSS alterno de este empleado.", nss)
        } else {
            format!("{} ya es NSS alterno de {}.", nss, nombre)
        });
    }

    let motivo = motivo.trim();
    let motivo: Option<&str> = if motivo.is_empty() { None } else { Some(motivo) };
    client
        .execute(
            "INSERT INTO public.empleados_nss_equivalencias (id_cliente, nss_alterno, id_empleado, motivo) \
             VALUES ($1, $2, $3, $4)",
            &[&id_cliente, &nss, &id_empleado, &motivo],
        )
        .map_err(pg)?;
    Ok(nss)
}

/// Quita una equivalencia. Devuelve false si ya no existía.
pub fn eliminar_equivalencia<C: GenericClient>(client: &mut C, id: Uuid) -> Result<bool, String> {
    client
        .execute("DELETE FROM public.empleados_nss_equivalencias WHERE id = $1", &[&id])
        .map(|n| n > 0)
        .map_err(|e| format!("Error quitando equivalencia de NSS: {}", crate::db::format_pg_error(&e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normaliza_guiones_y_valida_digito_verificador() {
        assert_eq!(normalizar_nss_alterno("02-17-64-7117-1").unwrap(), "02176471171");
        assert_eq!(normalizar_nss_alterno(" 43816538771 ").unwrap(), "43816538771");
        assert!(normalizar_nss_alterno("02176471172").is_err(), "dígito verificador incorrecto");
        assert!(normalizar_nss_alterno("0217647117").is_err(), "10 dígitos");
        assert!(normalizar_nss_alterno("0217647117A").is_err(), "letras");
        assert!(normalizar_nss_alterno("").is_err());
    }

    /// Flujo completo contra PostgreSQL dentro de una transacción que siempre se revierte.
    #[test]
    fn agregar_listar_y_eliminar_equivalencias() {
        let mut client = match crate::db::client::create_client() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("PostgreSQL no disponible en test (skipping): {}", e);
                return;
            }
        };
        // DDL fuera de la transacción: CREATE INDEX IF NOT EXISTS toma un bloqueo compartido de la
        // tabla aunque el índice exista y, dentro de transacciones concurrentes, provoca interbloqueos.
        if let Err(e) = client.batch_execute(crate::db::schema::SQL_EQUIVALENCIAS_NSS) {
            eprintln!("No se pudo asegurar la tabla de equivalencias (skipping): {}", e);
            return;
        }
        let mut tx = match client.transaction() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("No se pudo abrir transacción (skipping): {}", e);
                return;
            }
        };

        let id_cliente = Uuid::now_v7();
        tx.execute(
            "INSERT INTO public.clientes (id, nombre) VALUES ($1, $2)",
            &[&id_cliente, &format!("CLIENTE PRUEBA EQUIV {}", Uuid::now_v7())],
        )
        .expect("cliente de prueba");
        let (sergio, otro) = (Uuid::now_v7(), Uuid::now_v7());
        for (id, nombre, nss) in [(sergio, "PRUEBA SERGIO", "43816538771"), (otro, "PRUEBA OTRO", "43947909248")] {
            tx.execute(
                "INSERT INTO public.empleados (id, id_cliente, nombre, nss) VALUES ($1, $2, $3, $4)",
                &[&id, &id_cliente, &nombre, &nss],
            )
            .expect("empleado de prueba");
        }

        assert_eq!(agregar_equivalencia(&mut tx, sergio, "02-17-64-7117-1", " cédula IMSS ").unwrap(), "02176471171");
        let lista = listar_equivalencias(&mut tx, sergio).unwrap();
        assert_eq!(lista.len(), 1);
        assert_eq!(lista[0].nss_alterno, "02176471171");
        assert_eq!(lista[0].motivo, "cédula IMSS");

        // Rechazos
        let e = agregar_equivalencia(&mut tx, sergio, "02176471171", "").unwrap_err();
        assert!(e.contains("ya está registrado"), "{e}");
        let e = agregar_equivalencia(&mut tx, otro, "02176471171", "").unwrap_err();
        assert!(e.contains("PRUEBA SERGIO"), "{e}");
        let e = agregar_equivalencia(&mut tx, sergio, "43816538771", "").unwrap_err();
        assert!(e.contains("NSS principal"), "{e}");
        let e = agregar_equivalencia(&mut tx, sergio, "43947909248", "").unwrap_err();
        assert!(e.contains("Fusiona primero"), "{e}");

        assert!(eliminar_equivalencia(&mut tx, lista[0].id).unwrap());
        assert!(!eliminar_equivalencia(&mut tx, lista[0].id).unwrap());
        assert!(listar_equivalencias(&mut tx, sergio).unwrap().is_empty());

        let _ = tx.rollback();
    }
}
