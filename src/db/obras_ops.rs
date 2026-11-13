//! Operaciones de registros de obra con efecto en el reporte de liquidaciones (bitácora, universo).
//!
//! Desactivar una obra es un borrado lógico: la fila de `registros_obras` se conserva con
//! `activa = false`, la obra se quita de los tramos del reporte y la operación queda en la bitácora
//! para poder deshacerla. Guardar una obra resincroniza el universo de reportes (ACTIVO / FUERA_PERIODO)
//! porque sus fechas determinan qué tramos quedan dentro del periodo.

use std::collections::HashMap;

use postgres::{Client, GenericClient, Transaction};
use uuid::Uuid;

use crate::db::bitacora_reportes::{cerrar_edicion, iniciar_edicion, ACCION_DESACTIVAR_OBRA};
use crate::db::reporte_liquidaciones::sync_reportes_liquidaciones_for_revision_tx;
use crate::db::revisiones::{create_obra_tx, update_obra_tx, RegistroObraItem};
use crate::db::schema::format_pg_error;

/// Efecto de una obra sobre el reporte de liquidaciones.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImpactoObra {
    pub empleados: i64,
    pub empleado_mes: i64,
    pub tramos: i64,
}

/// Reportes que cambiaron de estado al resincronizar el universo.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CambioUniverso {
    pub a_fuera: usize,
    pub a_activo: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ResultadoDesactivar {
    pub id_revision: Uuid,
    pub reportes_afectados: u64,
    pub hay_bitacora: bool,
}

const SQL_CONTAR_IMPACTO: &str = "SELECT COUNT(DISTINCT rl.id_empleado), COUNT(DISTINCT rl.id), COUNT(*) \
    FROM public.reportes_detalles_liquidaciones d \
    JOIN public.reportes_liquidaciones rl ON rl.id = d.id_reporte_liquidacion \
    WHERE d.id_registro_obra = $1";

/// Tras guardar una obra editada, un UPDATE sin cambios hace que el trigger de topes reevalúe
/// `tiene_inconsistencia` de sus tramos con las nuevas fechas.
const SQL_REEVALUAR_TRAMOS_OBRA: &str = "UPDATE public.reportes_detalles_liquidaciones \
    SET dias_registro_obra = dias_registro_obra WHERE id_registro_obra = $1";

const SQL_OBRAS_DE_TRAMOS: &str = "SELECT DISTINCT id_reporte_liquidacion \
    FROM public.reportes_detalles_liquidaciones WHERE id_registro_obra = $1";

/// Quitar obra (mismo criterio que `planear_quitar`): un tramo de la obra se borra si otro tramo del
/// mismo reporte, de otra obra o sin obra, comparte clave y fecha.
const SQL_QUITAR_BORRAR_CON_HERMANO: &str = "DELETE FROM public.reportes_detalles_liquidaciones t \
    WHERE t.id_registro_obra = $1 AND EXISTS (SELECT 1 FROM public.reportes_detalles_liquidaciones o \
    WHERE o.id_reporte_liquidacion = t.id_reporte_liquidacion AND o.id_registro_obra IS DISTINCT FROM $1 \
    AND UPPER(BTRIM(COALESCE(o.clave,''))) = UPPER(BTRIM(COALESCE(t.clave,''))) \
    AND o.fecha IS NOT DISTINCT FROM t.fecha)";

/// Si la obra tiene varios tramos del mismo movimiento, conserva uno y borra los demás.
const SQL_QUITAR_BORRAR_DUPLICADOS: &str = "DELETE FROM public.reportes_detalles_liquidaciones \
    WHERE id IN (SELECT id FROM (SELECT id, ROW_NUMBER() OVER (PARTITION BY id_reporte_liquidacion, \
    UPPER(BTRIM(COALESCE(clave,''))), fecha ORDER BY id) AS rn \
    FROM public.reportes_detalles_liquidaciones WHERE id_registro_obra = $1) x WHERE x.rn > 1)";

/// El tramo que se conserva pasa a "sin obra".
const SQL_QUITAR_DEJAR_SIN_OBRA: &str = "UPDATE public.reportes_detalles_liquidaciones \
    SET id_registro_obra = NULL, dias_registro_obra = 0 WHERE id_registro_obra = $1";

const SQL_QUITAR_RVOL: &str =
    "DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_registro_obra = $1";

const SQL_DESACTIVAR_OBRA: &str =
    "UPDATE public.registros_obras SET activa = false, desactivada_at = now() WHERE id = $1";

const SQL_ESTADO_OBRA: &str = "SELECT id_revision, activa FROM public.registros_obras WHERE id = $1 FOR UPDATE";

const SQL_ESTADOS_UNIVERSO: &str =
    "SELECT id, estado FROM public.reportes_liquidaciones WHERE id_revision = $1";

/// Cuenta las obras (registros de obra con tramos) y los reportes/tramos que dependen de una obra.
pub fn contar_impacto_obra(client: &mut Client, id_obra: Uuid) -> Result<ImpactoObra, String> {
    let fila = client
        .query_one(SQL_CONTAR_IMPACTO, &[&id_obra])
        .map_err(|e| format!("contar_impacto_obra: {}", format_pg_error(&e)))?;
    Ok(ImpactoObra {
        empleados: fila.get(0),
        empleado_mes: fila.get(1),
        tramos: fila.get(2),
    })
}

fn leer_estados<C: GenericClient>(c: &mut C, id_revision: Uuid) -> Result<HashMap<Uuid, String>, String> {
    let filas = c
        .query(SQL_ESTADOS_UNIVERSO, &[&id_revision])
        .map_err(|e| format!("resincronizar_universo (lectura de estados): {}", format_pg_error(&e)))?;
    Ok(filas
        .iter()
        .map(|r| (r.get::<_, Uuid>(0), r.get::<_, Option<String>>(1).unwrap_or_default()))
        .collect())
}

/// Cuenta las transiciones ACTIVO <-> FUERA_PERIODO. Los reportes nuevos o borrados no cuentan.
fn contar_transiciones(antes: &HashMap<Uuid, String>, despues: &HashMap<Uuid, String>) -> CambioUniverso {
    let mut cambio = CambioUniverso::default();
    for (id, estado_antes) in antes {
        let Some(estado_despues) = despues.get(id) else {
            continue;
        };
        match (estado_antes.as_str(), estado_despues.as_str()) {
            ("ACTIVO", "FUERA_PERIODO") => cambio.a_fuera += 1,
            ("FUERA_PERIODO", "ACTIVO") => cambio.a_activo += 1,
            _ => {}
        }
    }
    cambio
}

/// Resincroniza el universo de reportes de la revisión dentro de la transacción recibida.
pub fn resincronizar_universo_tx(tx: &mut Transaction, id_revision: Uuid) -> Result<CambioUniverso, String> {
    let antes = leer_estados(tx, id_revision)?;
    sync_reportes_liquidaciones_for_revision_tx(tx, id_revision)?;
    let despues = leer_estados(tx, id_revision)?;
    Ok(contar_transiciones(&antes, &despues))
}

/// Igual que `resincronizar_universo_tx`, con su propia transacción.
pub fn resincronizar_universo(client: &mut Client, id_revision: Uuid) -> Result<CambioUniverso, String> {
    let mut tx = client
        .transaction()
        .map_err(|e| format!("resincronizar_universo (inicio): {}", format_pg_error(&e)))?;
    let cambio = resincronizar_universo_tx(&mut tx, id_revision)?;
    tx.commit()
        .map_err(|e| format!("resincronizar_universo (commit): {}", format_pg_error(&e)))?;
    Ok(cambio)
}

/// Crea o actualiza una obra y resincroniza el universo de reportes, todo en una transacción.
pub fn guardar_obra_y_resincronizar(
    client: &mut Client,
    item: &RegistroObraItem,
    es_edicion: bool,
) -> Result<(RegistroObraItem, CambioUniverso), String> {
    let id_revision = Uuid::parse_str(&item.id_revision)
        .map_err(|e| format!("ID de Revisión inválido: {}", e))?;
    let mut tx = client
        .transaction()
        .map_err(|e| format!("guardar_obra (inicio): {}", format_pg_error(&e)))?;

    let guardada = if es_edicion {
        update_obra_tx(&mut tx, item)?
    } else {
        create_obra_tx(&mut tx, item)?
    };

    if es_edicion {
        let id_obra = Uuid::parse_str(&guardada.id)
            .map_err(|e| format!("ID de Obra inválido: {}", e))?;
        tx.execute(SQL_REEVALUAR_TRAMOS_OBRA, &[&id_obra])
            .map_err(|e| format!("guardar_obra (reevaluar tramos): {}", format_pg_error(&e)))?;
    }

    let cambio = resincronizar_universo_tx(&mut tx, id_revision)?;
    tx.commit()
        .map_err(|e| format!("guardar_obra (commit): {}", format_pg_error(&e)))?;
    Ok((guardada, cambio))
}

/// Desactiva una obra (borrado lógico) en su propia transacción.
pub fn desactivar_obra(client: &mut Client, id_obra: Uuid) -> Result<ResultadoDesactivar, String> {
    let mut tx = client
        .transaction()
        .map_err(|e| format!("desactivar_obra (inicio): {}", format_pg_error(&e)))?;
    let resultado = desactivar_obra_tx(&mut tx, id_obra)?;
    tx.commit()
        .map_err(|e| format!("desactivar_obra (commit): {}", format_pg_error(&e)))?;
    Ok(resultado)
}

/// Desactiva una obra dentro de la transacción recibida: la quita de los tramos (con bitácora
/// reversible), marca `activa = false` y resincroniza el universo de reportes.
pub fn desactivar_obra_tx(tx: &mut Transaction, id_obra: Uuid) -> Result<ResultadoDesactivar, String> {
    // 1. Bloqueo y validación de la obra.
    let fila = tx
        .query_opt(SQL_ESTADO_OBRA, &[&id_obra])
        .map_err(|e| format!("desactivar_obra (bloqueo de obra): {}", format_pg_error(&e)))?
        .ok_or_else(|| "La obra no existe.".to_string())?;
    let id_revision: Uuid = fila.get(0);
    let activa: bool = fila.get(1);
    if !activa {
        return Err("La obra ya fue eliminada.".to_string());
    }

    // 2. Reportes que contienen tramos de la obra.
    let filas = tx
        .query(SQL_OBRAS_DE_TRAMOS, &[&id_obra])
        .map_err(|e| format!("desactivar_obra (consulta de reportes): {}", format_pg_error(&e)))?;
    let ids: Vec<Uuid> = filas.iter().map(|r| r.get(0)).collect();

    // 3. Bitácora: foto "antes" de los reportes afectados, ligada a la obra.
    let lote = if ids.is_empty() {
        None
    } else {
        let lote = iniciar_edicion(tx, ACCION_DESACTIVAR_OBRA, &ids)
            .map_err(|e| format!("desactivar_obra (bitácora): {}", e))?;
        tx.execute(
            "UPDATE public.reportes_cambios SET id_registro_obra = $2 WHERE lote_id = $1",
            &[&lote, &id_obra],
        )
        .map_err(|e| format!("desactivar_obra (bitácora obra): {}", format_pg_error(&e)))?;
        Some(lote)
    };

    // 4. Quitar la obra de los tramos.
    let pasos = [
        ("quitar tramos con hermano", SQL_QUITAR_BORRAR_CON_HERMANO),
        ("quitar tramos duplicados", SQL_QUITAR_BORRAR_DUPLICADOS),
        ("dejar tramos sin obra", SQL_QUITAR_DEJAR_SIN_OBRA),
        ("quitar vinculaciones", SQL_QUITAR_RVOL),
    ];
    for (paso, sql) in pasos {
        tx.execute(sql, &[&id_obra])
            .map_err(|e| format!("desactivar_obra ({}): {}", paso, format_pg_error(&e)))?;
    }

    // 5. Marcar la obra como inactiva.
    tx.execute(SQL_DESACTIVAR_OBRA, &[&id_obra])
        .map_err(|e| format!("desactivar_obra (marcar inactiva): {}", format_pg_error(&e)))?;

    // 6. Cerrar la bitácora: recalcula prorrateo y regenera rvol.
    if let Some(lote) = lote {
        cerrar_edicion(tx, lote, true).map_err(|e| format!("desactivar_obra (cerrar bitácora): {}", e))?;
    }

    // 7. El universo puede cambiar al perder una obra.
    resincronizar_universo_tx(tx, id_revision)
        .map_err(|e| format!("desactivar_obra (universo): {}", e))?;

    Ok(ResultadoDesactivar {
        id_revision,
        reportes_afectados: ids.len() as u64,
        hay_bitacora: !ids.is_empty(),
    })
}

// ---------------------------------------------------------------------------
// Alta masiva de obras (lote con vista previa)
// ---------------------------------------------------------------------------

use crate::obras_lote_logic::{AccionFila, EstadoFila, FilaPrevia, ObraExistente};

/// Obras de la revisión (activas e inactivas) para clasificar un lote.
pub fn fetch_obras_existentes_revision<C: GenericClient>(
    c: &mut C,
    id_revision: Uuid,
) -> Result<Vec<ObraExistente>, String> {
    let rows = c
        .query(
            "SELECT COALESCE(BTRIM(numero), '')::text AS numero,
                    COALESCE(creado_usuario, false) AS creado_usuario,
                    activa
             FROM public.registros_obras
             WHERE id_revision = $1",
            &[&id_revision],
        )
        .map_err(|e| format!("Error consultando obras de la revisión: {}", e))?;
    Ok(rows
        .iter()
        .map(|r| ObraExistente {
            numero: r.get(0),
            creado_usuario: r.get(1),
            activa: r.get(2),
        })
        .collect())
}

/// Conteo de lo aplicado por `aplicar_lote_obras`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResumenLoteObras {
    pub creadas: usize,
    pub actualizadas: usize,
    pub adoptadas: usize,
    pub omitidas: usize,
    pub cambio: CambioUniverso,
}

/// Aplica el lote dentro de una transacción existente (permite revertir en pruebas).
pub fn aplicar_lote_obras_tx(
    tx: &mut Transaction,
    id_revision: Uuid,
    id_rp: Uuid,
    filas: &[FilaPrevia],
) -> Result<ResumenLoteObras, String> {
    if let Some(bad) = filas.iter().find(|f| {
        f.accion != AccionFila::Omitir
            && matches!(f.estado, EstadoFila::Error | EstadoFila::RepetidaEnLote)
    }) {
        return Err(format!(
            "Renglón {}: la fila tiene errores y no puede aplicarse",
            bad.fila.renglon
        ));
    }

    let mut resumen = ResumenLoteObras::default();
    for previa in filas {
        let fila = &previa.fila;
        let clave = fila.numero.split_whitespace().collect::<String>().to_uppercase();
        let err_fila = |e: String| format!("Renglón {}: {}", fila.renglon, e);

        match previa.accion {
            AccionFila::Omitir => resumen.omitidas += 1,
            AccionFila::Crear => {
                let existe = tx
                    .query_opt(
                        "SELECT 1 FROM public.registros_obras WHERE id_revision = $1 AND UPPER(BTRIM(numero)) = $2",
                        &[&id_revision, &clave],
                    )
                    .map_err(|e| err_fila(format!("consultando obra: {}", e)))?
                    .is_some();
                if existe {
                    // Carrera: la obra apareció después de la vista previa.
                    resumen.omitidas += 1;
                    continue;
                }
                let item = RegistroObraItem {
                    id: String::new(),
                    id_revision: id_revision.to_string(),
                    id_registro_patronal: id_rp.to_string(),
                    numero: fila.numero.clone(),
                    clase: fila.clase.clone(),
                    tipo_obra: fila.tipo_obra.clone(),
                    fecha_inicio: fila.fecha_inicio,
                    fecha_fin: fila.fecha_fin,
                    ubicacion: fila.ubicacion.clone(),
                    superficie_construido: fila.superficie,
                    monto: fila.monto,
                    delegacion: fila.delegacion.clone(),
                    subdelegacion: fila.subdelegacion.clone(),
                    created_at: String::new(),
                    creado_usuario: true,
                };
                create_obra_tx(tx, &item).map_err(err_fila)?;
                resumen.creadas += 1;
            }
            AccionFila::Actualizar | AccionFila::Adoptar => {
                let id_obra: Uuid = tx
                    .query_opt(
                        "SELECT id FROM public.registros_obras WHERE id_revision = $1 AND UPPER(BTRIM(numero)) = $2",
                        &[&id_revision, &clave],
                    )
                    .map_err(|e| err_fila(format!("consultando obra: {}", e)))?
                    .ok_or_else(|| err_fila("la obra ya no existe en la revisión".to_string()))?
                    .get(0);
                let item = RegistroObraItem {
                    id: id_obra.to_string(),
                    id_revision: id_revision.to_string(),
                    id_registro_patronal: id_rp.to_string(),
                    numero: fila.numero.clone(),
                    clase: fila.clase.clone(),
                    tipo_obra: fila.tipo_obra.clone(),
                    fecha_inicio: fila.fecha_inicio,
                    fecha_fin: fila.fecha_fin,
                    ubicacion: fila.ubicacion.clone(),
                    superficie_construido: fila.superficie,
                    monto: fila.monto,
                    delegacion: fila.delegacion.clone(),
                    subdelegacion: fila.subdelegacion.clone(),
                    created_at: String::new(),
                    creado_usuario: true,
                };
                update_obra_tx(tx, &item).map_err(err_fila)?;
                tx.execute(
                    "UPDATE public.registros_obras SET activa = true, desactivada_at = NULL WHERE id = $1",
                    &[&id_obra],
                )
                .map_err(|e| err_fila(format!("reactivando obra: {}", e)))?;
                if previa.accion == AccionFila::Adoptar {
                    resumen.adoptadas += 1;
                } else {
                    resumen.actualizadas += 1;
                }
            }
        }
    }

    resumen.cambio = resincronizar_universo_tx(tx, id_revision)?;
    Ok(resumen)
}

/// Aplica el lote en UNA transacción: cualquier error revierte todo.
pub fn aplicar_lote_obras(
    client: &mut Client,
    id_revision: Uuid,
    id_rp: Uuid,
    filas: &[FilaPrevia],
) -> Result<ResumenLoteObras, String> {
    let mut tx = client
        .transaction()
        .map_err(|e| format!("No se pudo iniciar la transacción: {}", e))?;
    let resumen = aplicar_lote_obras_tx(&mut tx, id_revision, id_rp, filas)?;
    tx.commit()
        .map_err(|e| format!("No se pudo confirmar el lote de obras: {}", e))?;
    Ok(resumen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapa(pares: &[(Uuid, &str)]) -> HashMap<Uuid, String> {
        pares.iter().map(|(id, e)| (*id, e.to_string())).collect()
    }

    #[test]
    fn contar_transiciones_cuenta_solo_cambios_de_estado() {
        let (a, b, c, d, nuevo) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3), Uuid::from_u128(4), Uuid::from_u128(5));
        let antes = mapa(&[(a, "ACTIVO"), (b, "ACTIVO"), (c, "FUERA_PERIODO"), (d, "FUERA_PERIODO")]);
        let despues = mapa(&[(a, "FUERA_PERIODO"), (b, "ACTIVO"), (c, "ACTIVO"), (d, "FUERA_PERIODO"), (nuevo, "ACTIVO")]);
        assert_eq!(
            contar_transiciones(&antes, &despues),
            CambioUniverso { a_fuera: 1, a_activo: 1 }
        );
    }

    #[test]
    fn contar_transiciones_ignora_reportes_borrados_y_nuevos() {
        let (a, nuevo) = (Uuid::from_u128(10), Uuid::from_u128(11));
        let antes = mapa(&[(a, "ACTIVO")]);
        let despues = mapa(&[(nuevo, "FUERA_PERIODO")]);
        assert_eq!(contar_transiciones(&antes, &despues), CambioUniverso::default());
    }

    #[test]
    fn quitar_obra_conserva_regla_de_hermanos() {
        assert!(SQL_QUITAR_BORRAR_CON_HERMANO.contains("IS DISTINCT FROM $1"));
        assert!(SQL_QUITAR_BORRAR_CON_HERMANO.contains("fecha IS NOT DISTINCT FROM"));
        assert!(SQL_QUITAR_DEJAR_SIN_OBRA.contains("id_registro_obra = NULL"));
    }

    #[test]
    fn reevaluar_tramos_no_cambia_valores() {
        assert!(SQL_REEVALUAR_TRAMOS_OBRA.contains("dias_registro_obra = dias_registro_obra"));
    }

    #[test]
    fn ninguna_operacion_borra_obras_fisicamente() {
        for sql in [
            SQL_CONTAR_IMPACTO,
            SQL_REEVALUAR_TRAMOS_OBRA,
            SQL_OBRAS_DE_TRAMOS,
            SQL_QUITAR_BORRAR_CON_HERMANO,
            SQL_QUITAR_BORRAR_DUPLICADOS,
            SQL_QUITAR_DEJAR_SIN_OBRA,
            SQL_QUITAR_RVOL,
            SQL_DESACTIVAR_OBRA,
            SQL_ESTADO_OBRA,
            SQL_ESTADOS_UNIVERSO,
        ] {
            assert!(!sql.contains("DELETE FROM public.registros_obras"), "{}", sql);
        }
    }
}
