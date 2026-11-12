//! Registro de archivos de cédulas (`public.archivos_cedulas`) para la ingesta de lotes.
//!
//! Flujo previsto:
//! 1. `clasificar_lote` (solo lectura) indica qué archivos de un lote ya existen.
//! 2. `registrar_archivo_staged` inserta/actualiza el archivo en estado STAGED dentro
//!    de la transacción de guardado.
//! 3. Tras subir a RustFS, `marcar_archivo_activo` y `marcar_reemplazados_por_periodo`
//!    activan el archivo.
//! 4. `reintentar_promociones_pendientes` completa promociones que quedaron a medias.

use std::collections::HashMap;

use postgres::{Client, GenericClient};
use uuid::Uuid;

use crate::cedulas::almacen::AlmacenArchivos;

/// Archivo de un lote a clasificar o registrar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivoLote {
    pub nombre_original: String,
    pub sha256: String,
    /// "SUA" | "EMA"
    pub tipo: String,
    pub anio: i16,
    pub mes: i16,
    pub trabajadores: i32,
    pub paginas: Option<i16>,
}

/// Archivo nuevo de un lote listo para registrarse junto con sus liquidaciones.
#[derive(Debug, Clone)]
pub struct ArchivoParaRegistrar {
    /// Nombre del PDF tal como lo reporta `Employee.file_name` (clave de asociación).
    pub nombre_archivo: String,
    pub lote: ArchivoLote,
    pub clave_final: String,
    pub clave_staging: String,
}

/// Determina el tipo de archivo a partir de sus empleados.
///
/// Devuelve "SUA" si algún empleado trae registro de obras o RFC/CURP no vacíos;
/// en otro caso "EMA".
pub fn tipo_archivo_desde_empleados(emps: &[&crate::cedulas::employees::Employee]) -> &'static str {
    let no_vacio = |valor: &Option<String>| valor.as_deref().map_or(false, |v| !v.trim().is_empty());
    let es_sua = emps
        .iter()
        .any(|e| !e.registro_obras.is_empty() || no_vacio(&e.rfc) || no_vacio(&e.curp));
    if es_sua {
        "SUA"
    } else {
        "EMA"
    }
}

/// Resultado de clasificar un archivo antes de guardarlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EstadoArchivoLote {
    Nuevo,
    IdenticoYaCargado { id_archivo: Uuid, nombre_previo: String },
    DuplicadoEnLote { igual_a: String },
    MismoPeriodoCargado { archivos_previos: Vec<String>, liquidaciones_previas: i64 },
}

/// Normaliza un hash para comparación: minúsculas y sin espacios.
fn normalizar_hash(hash: &str) -> String {
    hash.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Clasifica un lote sin acceder a la BD.
///
/// Prioridad: (a) hash ya cargado, (b) hash repetido de un archivo anterior del
/// mismo lote, (c) periodo (anio, mes) ya cargado, (d) nuevo.
/// Devuelve los estados en el mismo orden que `archivos`.
pub fn clasificar_lote_puro(
    archivos: &[ArchivoLote],
    hashes_existentes: &HashMap<String, (Uuid, String)>,
    periodos_existentes: &HashMap<(i16, i16), (Vec<String>, i64)>,
) -> Vec<EstadoArchivoLote> {
    let existentes: HashMap<String, &(Uuid, String)> = hashes_existentes
        .iter()
        .map(|(hash, valor)| (normalizar_hash(hash), valor))
        .collect();

    // hash normalizado -> nombre del primer archivo del lote con ese hash
    let mut primeros_del_lote: HashMap<String, String> = HashMap::new();

    archivos
        .iter()
        .map(|a| {
            let hash = normalizar_hash(&a.sha256);

            let estado = if let Some((id, nombre)) = existentes.get(&hash) {
                EstadoArchivoLote::IdenticoYaCargado {
                    id_archivo: *id,
                    nombre_previo: nombre.clone(),
                }
            } else if let Some(primero) = primeros_del_lote.get(&hash) {
                EstadoArchivoLote::DuplicadoEnLote { igual_a: primero.clone() }
            } else if let Some((previos, liquidaciones)) = periodos_existentes.get(&(a.anio, a.mes)) {
                EstadoArchivoLote::MismoPeriodoCargado {
                    archivos_previos: previos.clone(),
                    liquidaciones_previas: *liquidaciones,
                }
            } else {
                EstadoArchivoLote::Nuevo
            };

            primeros_del_lote
                .entry(hash)
                .or_insert_with(|| a.nombre_original.clone());

            estado
        })
        .collect()
}

/// Clasifica un lote consultando la BD. Solo realiza lecturas.
pub fn clasificar_lote<C: GenericClient>(
    c: &mut C,
    id_rp: Uuid,
    archivos: &[ArchivoLote],
) -> Result<Vec<EstadoArchivoLote>, String> {
    if archivos.is_empty() {
        return Ok(Vec::new());
    }

    let hashes: Vec<String> = archivos.iter().map(|a| normalizar_hash(&a.sha256)).collect();
    let claves_periodo: Vec<i32> = archivos
        .iter()
        .map(|a| i32::from(a.anio) * 100 + i32::from(a.mes))
        .collect();

    let filas_hash = c
        .query(
            "SELECT LOWER(BTRIM(sha256))::text, id, nombre_original
               FROM public.archivos_cedulas
              WHERE id_registro_patronal = $1
                AND LOWER(BTRIM(sha256)) = ANY($2)
                AND estado IN ('STAGED', 'ACTIVO')",
            &[&id_rp, &hashes],
        )
        .map_err(|e| format!("Error consultando hashes de archivos_cedulas: {}", crate::db::schema::format_pg_error(&e)))?;

    let mut hashes_existentes: HashMap<String, (Uuid, String)> = HashMap::new();
    for fila in filas_hash {
        let hash: String = fila.get(0);
        let id: Uuid = fila.get(1);
        let nombre: String = fila.get(2);
        hashes_existentes.entry(hash).or_insert((id, nombre));
    }

    let filas_periodo = c
        .query(
            "SELECT (anio_periodo::int * 100 + mes_periodo_int::int) AS clave,
                    COUNT(*)::bigint,
                    COALESCE(
                        array_agg(DISTINCT nombre_archivo::text)
                            FILTER (WHERE nombre_archivo IS NOT NULL),
                        '{}'::text[]
                    )
               FROM public.liquidaciones
              WHERE id_registro_patronal = $1
                AND (anio_periodo::int * 100 + mes_periodo_int::int) = ANY($2::int[])
              GROUP BY 1",
            &[&id_rp, &claves_periodo],
        )
        .map_err(|e| format!("Error consultando periodos en liquidaciones: {}", crate::db::schema::format_pg_error(&e)))?;

    let mut periodos_existentes: HashMap<(i16, i16), (Vec<String>, i64)> = HashMap::new();
    for fila in filas_periodo {
        let clave: i32 = fila.get(0);
        let liquidaciones: i64 = fila.get(1);
        let archivos_previos: Vec<String> = fila.get(2);
        let anio = (clave / 100) as i16;
        let mes = (clave % 100) as i16;
        periodos_existentes.insert((anio, mes), (archivos_previos, liquidaciones));
    }

    Ok(clasificar_lote_puro(archivos, &hashes_existentes, &periodos_existentes))
}

/// Inserta el archivo en estado STAGED o lo actualiza si ya existe (rp, sha256).
///
/// Es idempotente: un archivo ya ACTIVO conserva su estado y su `staging_key`
/// no se modifica. Devuelve el `id` del registro.
pub fn registrar_archivo_staged<C: GenericClient>(
    c: &mut C,
    id_rp: Uuid,
    id_revision: Option<Uuid>,
    a: &ArchivoLote,
    clave_final: &str,
    clave_staging: &str,
) -> Result<Uuid, String> {
    let fila = c
        .query_one(
            "INSERT INTO public.archivos_cedulas
                (id_registro_patronal, id_revision_carga, sha256, nombre_original,
                 rustfs_key, staging_key, tipo, anio_periodo, mes_periodo_int,
                 paginas, trabajadores, estado)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'STAGED')
             ON CONFLICT ON CONSTRAINT uq_archivos_cedulas_rp_sha DO UPDATE SET
                estado = CASE WHEN public.archivos_cedulas.estado = 'ACTIVO'
                              THEN 'ACTIVO' ELSE 'STAGED' END,
                rustfs_key = COALESCE(public.archivos_cedulas.rustfs_key, EXCLUDED.rustfs_key),
                staging_key = CASE WHEN public.archivos_cedulas.estado = 'ACTIVO'
                                   THEN public.archivos_cedulas.staging_key
                                   ELSE EXCLUDED.staging_key END,
                nombre_original = EXCLUDED.nombre_original
             RETURNING id",
            &[
                &id_rp,
                &id_revision,
                &normalizar_hash(&a.sha256),
                &a.nombre_original,
                &clave_final,
                &clave_staging,
                &a.tipo,
                &a.anio,
                &a.mes,
                &a.paginas,
                &a.trabajadores,
            ],
        )
        .map_err(|e| format!("Error registrando archivo STAGED '{}': {}", a.nombre_original, crate::db::schema::format_pg_error(&e)))?;

    Ok(fila.get(0))
}

/// Marca un archivo como ACTIVO y limpia su `staging_key`.
/// Devuelve error si el archivo no existe.
pub fn marcar_archivo_activo<C: GenericClient>(c: &mut C, id_archivo: Uuid) -> Result<(), String> {
    let filas = c
        .execute(
            "UPDATE public.archivos_cedulas
                SET estado = 'ACTIVO', staging_key = NULL
              WHERE id = $1",
            &[&id_archivo],
        )
        .map_err(|e| format!("Error activando archivo {id_archivo}: {}", crate::db::schema::format_pg_error(&e)))?;

    if filas == 0 {
        return Err(format!("Archivo {id_archivo} no encontrado al activar"));
    }
    Ok(())
}

/// Marca como REEMPLAZADO los archivos ACTIVO del mismo registro patronal y periodo
/// cuyo id difiere de `id_nuevo`. Devuelve cuántos archivos fueron reemplazados.
pub fn marcar_reemplazados_por_periodo<C: GenericClient>(
    c: &mut C,
    id_rp: Uuid,
    anio: i16,
    mes: i16,
    id_nuevo: Uuid,
) -> Result<u64, String> {
    c.execute(
        "UPDATE public.archivos_cedulas
            SET estado = 'REEMPLAZADO', reemplazado_por = $4
          WHERE id_registro_patronal = $1
            AND anio_periodo = $2
            AND mes_periodo_int = $3
            AND estado = 'ACTIVO'
            AND id <> $4",
        &[&id_rp, &anio, &mes, &id_nuevo],
    )
    .map_err(|e| format!("Error marcando reemplazados del periodo {anio}-{mes:02}: {}", crate::db::schema::format_pg_error(&e)))
}

/// Reintenta promover en RustFS los archivos STAGED con más de `antiguedad_min` minutos.
///
/// Solo se marca ACTIVO (y se reemplazan los anteriores del periodo) cuando la
/// promoción en el almacén tuvo éxito. Devuelve `(promovidos, fallidos)`.
pub fn reintentar_promociones_pendientes(
    client: &mut Client,
    almacen: &dyn AlmacenArchivos,
    antiguedad_min: i32,
) -> Result<(u64, u64), String> {
    let pendientes = client
        .query(
            "SELECT id, staging_key, rustfs_key
               FROM public.archivos_cedulas
              WHERE estado = 'STAGED'
                AND staging_key IS NOT NULL
                AND rustfs_key IS NOT NULL
                AND created_at < now() - make_interval(mins => $1)",
            &[&antiguedad_min],
        )
        .map_err(|e| format!("Error consultando promociones pendientes: {}", crate::db::schema::format_pg_error(&e)))?;

    let mut promovidos: u64 = 0;
    let mut fallidos: u64 = 0;

    for fila in pendientes {
        let id: Uuid = fila.get(0);
        let staging: String = fila.get(1);
        let final_key: String = fila.get(2);

        if let Err(e) = almacen.promover(&staging, &final_key) {
            log::warn!("Promoción pendiente fallida para archivo {id} ({staging} -> {final_key}): {e}");
            fallidos += 1;
            continue;
        }

        // Solo se activa: un mismo periodo puede tener varios archivos ACTIVO legítimos (cédulas con
        // trabajadores distintos). El reemplazo por periodo solo procede si el analista eligió "Reemplazar".
        let resultado = marcar_archivo_activo(client, id);

        match resultado {
            Ok(()) => promovidos += 1,
            Err(e) => {
                log::warn!("Archivo {id} promovido en almacén pero no activado en BD: {e}");
                fallidos += 1;
            }
        }
    }

    Ok((promovidos, fallidos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archivo(nombre: &str, sha: &str, anio: i16, mes: i16) -> ArchivoLote {
        ArchivoLote {
            nombre_original: nombre.to_string(),
            sha256: sha.to_string(),
            tipo: "SUA".to_string(),
            anio,
            mes,
            trabajadores: 10,
            paginas: Some(3),
        }
    }

    #[test]
    fn clasifica_los_cuatro_estados() {
        let id = Uuid::now_v7();
        let mut hashes = HashMap::new();
        hashes.insert("aaa".to_string(), (id, "previo_hash.pdf".to_string()));

        let mut periodos = HashMap::new();
        periodos.insert((2024, 5), (vec!["viejo.pdf".to_string()], 7_i64));

        let lote = vec![
            archivo("ident.pdf", "aaa", 2025, 1),      // IdenticoYaCargado
            archivo("periodo.pdf", "bbb", 2024, 5),    // MismoPeriodoCargado
            archivo("nuevo.pdf", "ccc", 2025, 2),      // Nuevo
        ];

        let estados = clasificar_lote_puro(&lote, &hashes, &periodos);
        assert_eq!(
            estados,
            vec![
                EstadoArchivoLote::IdenticoYaCargado {
                    id_archivo: id,
                    nombre_previo: "previo_hash.pdf".to_string(),
                },
                EstadoArchivoLote::MismoPeriodoCargado {
                    archivos_previos: vec!["viejo.pdf".to_string()],
                    liquidaciones_previas: 7,
                },
                EstadoArchivoLote::Nuevo,
            ]
        );
    }

    #[test]
    fn identico_tiene_prioridad_sobre_mismo_periodo() {
        let id = Uuid::now_v7();
        let mut hashes = HashMap::new();
        hashes.insert("abc".to_string(), (id, "x.pdf".to_string()));
        let mut periodos = HashMap::new();
        periodos.insert((2024, 5), (vec!["y.pdf".to_string()], 1_i64));

        let estados = clasificar_lote_puro(&[archivo("a.pdf", "abc", 2024, 5)], &hashes, &periodos);
        assert!(matches!(estados[0], EstadoArchivoLote::IdenticoYaCargado { .. }));
    }

    #[test]
    fn duplicado_en_lote_marca_solo_el_segundo() {
        let lote = vec![
            archivo("primero.pdf", "dup", 2025, 3),
            archivo("segundo.pdf", "dup", 2025, 3),
        ];
        let estados = clasificar_lote_puro(&lote, &HashMap::new(), &HashMap::new());
        assert_eq!(estados[0], EstadoArchivoLote::Nuevo);
        assert_eq!(
            estados[1],
            EstadoArchivoLote::DuplicadoEnLote { igual_a: "primero.pdf".to_string() }
        );
    }

    #[test]
    fn hash_compara_sin_importar_mayusculas_ni_espacios() {
        let id = Uuid::now_v7();
        let mut hashes = HashMap::new();
        hashes.insert("AB CD ef".to_string(), (id, "guardado.pdf".to_string()));

        let estados = clasificar_lote_puro(
            &[archivo("nuevo.pdf", "ab cd EF", 2025, 4)],
            &hashes,
            &HashMap::new(),
        );
        assert_eq!(
            estados[0],
            EstadoArchivoLote::IdenticoYaCargado {
                id_archivo: id,
                nombre_previo: "guardado.pdf".to_string(),
            }
        );
    }

    #[test]
    fn tipo_archivo_sua_por_obras_o_rfc_curp() {
        use crate::cedulas::employees::{Employee, RegistroObra};

        let con_obra = Employee {
            registro_obras: vec![RegistroObra {
                registro_obra: "R1".to_string(),
                dias_registro_obra: 3,
            }],
            ..Default::default()
        };
        let con_rfc = Employee {
            rfc: Some("XAXX010101000".to_string()),
            ..Default::default()
        };
        let rfc_vacio = Employee {
            rfc: Some("   ".to_string()),
            curp: Some(String::new()),
            ..Default::default()
        };
        let sin_datos = Employee::default();

        assert_eq!(tipo_archivo_desde_empleados(&[&con_obra]), "SUA");
        assert_eq!(tipo_archivo_desde_empleados(&[&sin_datos, &con_rfc]), "SUA");
        assert_eq!(tipo_archivo_desde_empleados(&[&rfc_vacio, &sin_datos]), "EMA");
        assert_eq!(tipo_archivo_desde_empleados(&[]), "EMA");
    }

    #[test]
    fn lote_vacio_devuelve_vacio() {
        let estados = clasificar_lote_puro(&[], &HashMap::new(), &HashMap::new());
        assert!(estados.is_empty());
    }

    #[test]
    #[ignore]
    fn clasificar_lote_bd_solo_lectura() {
        use crate::db::client::create_client;

        let mut client = match create_client() {
            Ok(c) => c,
            Err(e) => {
                println!("PostgreSQL no disponible (skipping): {e}");
                return;
            }
        };

        let mut tx = client
            .build_transaction()
            .read_only(true)
            .start()
            .expect("No se pudo iniciar transacción de solo lectura");

        let id_rp = Uuid::now_v7();
        let sha = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
        let lote = vec![archivo("prueba.pdf", &sha, 2020, 1)];

        let estados = clasificar_lote(&mut tx, id_rp, &lote).expect("clasificar_lote falló");
        assert_eq!(estados, vec![EstadoArchivoLote::Nuevo]);

        tx.rollback().expect("rollback falló");
    }
}
