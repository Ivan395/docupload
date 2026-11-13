use postgres::Client;

/// Extrae y formatea el detalle completo de un error de PostgreSQL (`PgDatabaseError`:
/// código SQLSTATE, mensaje real, detalle, sugerencia, tabla, columna, restricción y ubicación),
/// evitando que se oculte detrás de un `"db error"` genérico.
pub fn format_pg_error(err: &postgres::Error) -> String {
    if let Some(db_err) = err.as_db_error() {
        let mut parts = Vec::new();
        parts.push(format!(
            "[{}] {}",
            db_err.code().code(),
            db_err.message()
        ));
        if let Some(detail) = db_err.detail() {
            if !detail.trim().is_empty() {
                parts.push(format!("Detalle: {}", detail.trim()));
            }
        }
        if let Some(hint) = db_err.hint() {
            if !hint.trim().is_empty() {
                parts.push(format!("Sugerencia: {}", hint.trim()));
            }
        }
        if let Some(table) = db_err.table() {
            parts.push(format!("Tabla: {}", table));
        }
        if let Some(column) = db_err.column() {
            parts.push(format!("Columna: {}", column));
        }
        if let Some(constraint) = db_err.constraint() {
            parts.push(format!("Restricción: {}", constraint));
        }
        if let Some(where_) = db_err.where_() {
            if !where_.trim().is_empty() {
                parts.push(format!("Contexto SQL: {}", where_.trim()));
            }
        }
        parts.join(" | ")
    } else {
        format!("{}", err)
    }
}

/// Extrae y formatea el detalle de un error genérico que puede contener un `postgres::Error`.
pub fn format_boxed_pg_error(err: &(dyn std::error::Error + 'static)) -> String {
    if let Some(pg_err) = err.downcast_ref::<postgres::Error>() {
        format_pg_error(pg_err)
    } else {
        format!("{}", err)
    }
}

/// Fase 1 de integridad: tablas, columnas e índices nuevos. Todo idempotente.
/// Los CHECK se crean solo si no existen, mediante bloques DO.
const SQL_FASE1_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS public.archivos_cedulas (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    id_registro_patronal uuid NOT NULL REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE,
    id_revision_carga uuid REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE SET NULL,
    sha256 char(64) NOT NULL,
    nombre_original varchar(350) NOT NULL,
    rustfs_key varchar(700),
    tipo varchar(10) NOT NULL CHECK (tipo IN ('SUA','EMA')),
    anio_periodo smallint NOT NULL,
    mes_periodo_int smallint NOT NULL CHECK (mes_periodo_int BETWEEN 1 AND 12),
    paginas smallint,
    trabajadores integer,
    estado varchar(20) NOT NULL DEFAULT 'STAGED' CHECK (estado IN ('STAGED','ACTIVO','REEMPLAZADO','PAPELERA')),
    reemplazado_por uuid REFERENCES public.archivos_cedulas(id) ON DELETE SET NULL,
    created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uq_archivos_cedulas_rp_sha UNIQUE (id_registro_patronal, sha256)
);
CREATE INDEX IF NOT EXISTS idx_archivos_cedulas_rp_periodo ON public.archivos_cedulas(id_registro_patronal, anio_periodo, mes_periodo_int) WHERE estado = 'ACTIVO';
CREATE INDEX IF NOT EXISTS idx_archivos_cedulas_staged ON public.archivos_cedulas(created_at) WHERE estado = 'STAGED';
-- Clave temporal en RustFS mientras el archivo está en STAGED (necesaria para reintentar la promoción)
ALTER TABLE public.archivos_cedulas ADD COLUMN IF NOT EXISTS staging_key varchar(700);

ALTER TABLE public.liquidaciones ADD COLUMN IF NOT EXISTS id_archivo uuid REFERENCES public.archivos_cedulas(id) ON UPDATE CASCADE ON DELETE SET NULL;
ALTER TABLE public.liquidaciones ADD COLUMN IF NOT EXISTS origen varchar(5);
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'chk_liquidaciones_origen') THEN
        ALTER TABLE public.liquidaciones ADD CONSTRAINT chk_liquidaciones_origen CHECK (origen IS NULL OR origen IN ('SUA','EMA'));
    END IF;
END $$;

ALTER TABLE public.revisiones ADD COLUMN IF NOT EXISTS periodo_inicio date;
ALTER TABLE public.revisiones ADD COLUMN IF NOT EXISTS periodo_fin date;
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'chk_revisiones_periodo') THEN
        ALTER TABLE public.revisiones ADD CONSTRAINT chk_revisiones_periodo CHECK (periodo_inicio IS NULL OR periodo_fin IS NULL OR periodo_inicio <= periodo_fin);
    END IF;
END $$;

ALTER TABLE public.reportes_liquidaciones ADD COLUMN IF NOT EXISTS estado varchar(20) NOT NULL DEFAULT 'ACTIVO';
ALTER TABLE public.reportes_liquidaciones ADD COLUMN IF NOT EXISTS editado_usuario boolean NOT NULL DEFAULT false;
ALTER TABLE public.reportes_liquidaciones ADD COLUMN IF NOT EXISTS cedula_actualizada boolean NOT NULL DEFAULT false;
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'chk_reportes_liquidaciones_estado') THEN
        ALTER TABLE public.reportes_liquidaciones ADD CONSTRAINT chk_reportes_liquidaciones_estado CHECK (estado IN ('ACTIVO','FUERA_PERIODO','EXCLUIDO'));
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS public.reportes_cambios (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    id_revision uuid NOT NULL REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE CASCADE,
    id_reporte_liquidacion uuid REFERENCES public.reportes_liquidaciones(id) ON UPDATE CASCADE ON DELETE CASCADE,
    lote_id uuid NOT NULL,
    accion varchar(30) NOT NULL,
    antes jsonb,
    despues jsonb,
    usuario varchar(100),
    created_at timestamptz DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_reportes_cambios_rev ON public.reportes_cambios(id_revision, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_reportes_cambios_lote ON public.reportes_cambios(lote_id);
-- Pila de deshacer: lote DESHACER que revirtió este cambio (NULL = vigente)
ALTER TABLE public.reportes_cambios ADD COLUMN IF NOT EXISTS deshecho_por uuid;

CREATE TABLE IF NOT EXISTS public.exportaciones_reportes (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    id_revision uuid NOT NULL REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE CASCADE,
    tipo varchar(10) NOT NULL CHECK (tipo IN ('XLSX','SUA')),
    version integer NOT NULL,
    sha256 char(64) NOT NULL,
    rustfs_key varchar(700) NOT NULL,
    resumen jsonb,
    created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uq_export_rev_tipo_version UNIQUE (id_revision, tipo, version)
);

-- Respaldo de registros eliminados por procesos de depuración (permite reconstruir lo borrado)
CREATE TABLE IF NOT EXISTS public.respaldo_depuracion (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    proceso varchar(60) NOT NULL,
    tabla varchar(80) NOT NULL,
    id_registro uuid NOT NULL,
    id_sobreviviente uuid,
    datos jsonb NOT NULL,
    created_at timestamptz DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_respaldo_depuracion_proceso ON public.respaldo_depuracion(proceso, tabla);
"#;

/// Restricciones de integridad sobre datos existentes. Cada una se crea solo si no existe
/// y no hay registros que la violen; si los hay, emite un aviso y continúa sin abortar.
/// Migraciones de datos que deben ejecutarse UNA sola vez. Cada una se registra en
/// `public.migraciones_aplicadas` y se omite en arranques posteriores.
pub(crate) const SQL_MIGRACIONES_UNICAS: &str = r#"
CREATE TABLE IF NOT EXISTS public.migraciones_aplicadas (
    nombre varchar(120) PRIMARY KEY,
    aplicada_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Fase 3: la bitácora debe sobrevivir al borrado de un reporte (p. ej. "Revertir" borra y recrea
-- reportes): la FK a reportes_liquidaciones pasa de ON DELETE CASCADE a ON DELETE SET NULL.
DO $$
DECLARE nombre_fk text;
BEGIN
    SELECT c.conname INTO nombre_fk FROM pg_constraint c
    WHERE c.conrelid = 'public.reportes_cambios'::regclass AND c.contype = 'f'
      AND c.confrelid = 'public.reportes_liquidaciones'::regclass;
    IF nombre_fk IS NOT NULL AND EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = nombre_fk AND confdeltype <> 'n'
    ) THEN
        EXECUTE format('ALTER TABLE public.reportes_cambios DROP CONSTRAINT %I', nombre_fk);
        ALTER TABLE public.reportes_cambios ADD CONSTRAINT fk_reportes_cambios_reporte
            FOREIGN KEY (id_reporte_liquidacion) REFERENCES public.reportes_liquidaciones(id)
            ON UPDATE CASCADE ON DELETE SET NULL;
    END IF;
END $$;

-- Fase 3: los reportes existentes antes de introducir `editado_usuario` pudieron ser trabajados por el
-- analista (asignaciones de obra, días). No hay forma fiable de distinguirlos, así que se consideran
-- editados: la sincronización incremental nunca borrará ni reasignará su contenido.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM public.migraciones_aplicadas WHERE nombre = 'fase3_reportes_historicos_editados') THEN
        UPDATE public.reportes_liquidaciones SET editado_usuario = true WHERE editado_usuario = false;
        INSERT INTO public.migraciones_aplicadas (nombre) VALUES ('fase3_reportes_historicos_editados');
    END IF;
END $$;
"#;

/// Correcciones IMSS/SIROC F0: el papel de trabajo de nóminas deja de borrarse; se marca su estado
/// y se protegen las ediciones del analista y las nóminas sintéticas.
pub(crate) const SQL_CORRECCIONES_F0_NOMINAS: &str = r#"
ALTER TABLE public.reportes_nominas ADD COLUMN IF NOT EXISTS estado varchar(20) NOT NULL DEFAULT 'ACTIVO';
ALTER TABLE public.reportes_nominas ADD COLUMN IF NOT EXISTS editado_usuario boolean NOT NULL DEFAULT false;
ALTER TABLE public.reportes_nominas ADD COLUMN IF NOT EXISTS sintetica boolean NOT NULL DEFAULT false;
ALTER TABLE IF EXISTS revision_history.reportes_nominas ADD COLUMN IF NOT EXISTS estado varchar(20);
ALTER TABLE IF EXISTS revision_history.reportes_nominas ADD COLUMN IF NOT EXISTS editado_usuario boolean;
ALTER TABLE IF EXISTS revision_history.reportes_nominas ADD COLUMN IF NOT EXISTS sintetica boolean;
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'chk_reportes_nominas_estado') THEN
        ALTER TABLE public.reportes_nominas ADD CONSTRAINT chk_reportes_nominas_estado CHECK (estado IN ('ACTIVO','FUERA_ALCANCE'));
    END IF;
END $$;
CREATE INDEX IF NOT EXISTS idx_reportes_nominas_rev_estado ON public.reportes_nominas(id_revision, estado);

-- Una sola copia por (revisión, nómina). Solo se crea si no hay duplicados; si los hay, solo avisa.
DO $$
DECLARE n bigint;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname = 'public' AND indexname = 'uq_reportes_nominas_rev_nomina') THEN
        SELECT COUNT(*) INTO n FROM (
            SELECT 1 FROM public.reportes_nominas
            WHERE id_nomina IS NOT NULL
            GROUP BY id_revision, id_nomina
            HAVING COUNT(*) > 1
        ) g;
        IF n = 0 THEN
            CREATE UNIQUE INDEX uq_reportes_nominas_rev_nomina ON public.reportes_nominas(id_revision, id_nomina)
                WHERE id_nomina IS NOT NULL;
        ELSE
            RAISE WARNING 'uq_reportes_nominas_rev_nomina omitida: existen % grupos duplicados en reportes_nominas', n;
        END IF;
    END IF;
END $$;

-- Marcado inicial (una sola vez): sintéticas = sin nómina maestra; editadas = días o sueldos distintos de la maestra.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM public.migraciones_aplicadas WHERE nombre = 'f0_reportes_nominas_marcas') THEN
        UPDATE public.reportes_nominas SET sintetica = true WHERE id_nomina IS NULL AND sintetica = false;
        UPDATE public.reportes_nominas rn
        SET editado_usuario = true
        FROM public.nominas n
        WHERE n.id = rn.id_nomina
          AND rn.editado_usuario = false
          AND (
              (
                  ABS(COALESCE(rn.num_dias_pagados, 0) - COALESCE(n.num_dias_pagados, 0)) > 0.005
                  AND ABS(COALESCE(rn.num_dias_pagados, 0) - COALESCE((n.fecha_final_pago - n.fecha_inicial_pago) + 1, n.num_dias_pagados, 0)) > 0.005
              )
              OR ABS(COALESCE(rn.total_sueldos, 0) - COALESCE(n.total_sueldos, 0)) > 0.005
          );
        INSERT INTO public.migraciones_aplicadas (nombre) VALUES ('f0_reportes_nominas_marcas');
    END IF;
END $$;
"#;

/// F1: borrado lógico de obras, CHECK de fechas (NOT VALID + validación solo si no hay violaciones)
/// y referencia de obra en la bitácora para poder deshacer su eliminación. Solo agrega; no borra datos.
pub(crate) const SQL_CORRECCIONES_F1_OBRAS: &str = r#"
ALTER TABLE public.registros_obras ADD COLUMN IF NOT EXISTS activa boolean NOT NULL DEFAULT true;
ALTER TABLE public.registros_obras ADD COLUMN IF NOT EXISTS desactivada_at timestamptz;
ALTER TABLE IF EXISTS revision_history.registros_obras ADD COLUMN IF NOT EXISTS activa boolean;
ALTER TABLE IF EXISTS revision_history.registros_obras ADD COLUMN IF NOT EXISTS desactivada_at timestamptz;
CREATE INDEX IF NOT EXISTS idx_registros_obras_rev_activa ON public.registros_obras(id_revision) WHERE activa;

ALTER TABLE public.reportes_cambios ADD COLUMN IF NOT EXISTS id_registro_obra uuid;
ALTER TABLE IF EXISTS revision_history.reportes_cambios ADD COLUMN IF NOT EXISTS id_registro_obra uuid;

DO $$
DECLARE n bigint;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'chk_registros_obras_fechas') THEN
        ALTER TABLE public.registros_obras
            ADD CONSTRAINT chk_registros_obras_fechas CHECK (fecha_fin >= fecha_inicio) NOT VALID;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'chk_registros_obras_fechas' AND NOT convalidated) THEN
        SELECT COUNT(*) INTO n FROM public.registros_obras WHERE fecha_fin < fecha_inicio;
        IF n = 0 THEN
            ALTER TABLE public.registros_obras VALIDATE CONSTRAINT chk_registros_obras_fechas;
        ELSE
            RAISE WARNING 'chk_registros_obras_fechas sin validar: % obras con fecha_fin < fecha_inicio', n;
        END IF;
    END IF;
END $$;
"#;

pub(crate) const SQL_FASE1_RESTRICCIONES: &str = r#"
-- a) Empleados: NSS único por cliente (solo NSS no vacíos)
DO $$
DECLARE n bigint;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname = 'public' AND indexname = 'uq_empleados_cliente_nss')
       AND NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'uq_empleados_cliente_nss') THEN
        SELECT COUNT(*) INTO n FROM (
            SELECT 1 FROM public.empleados
            WHERE nss IS NOT NULL AND BTRIM(nss) <> ''
            GROUP BY id_cliente, nss
            HAVING COUNT(*) > 1
        ) g;
        IF n = 0 THEN
            CREATE UNIQUE INDEX uq_empleados_cliente_nss ON public.empleados(id_cliente, nss)
                WHERE nss IS NOT NULL AND BTRIM(nss) <> '';
        ELSE
            RAISE WARNING 'uq_empleados_cliente_nss omitida: existen % registros que la violan; ejecutar depuracion de Fase 1', n;
        END IF;
    END IF;
END $$;

-- b) Liquidaciones: mes_periodo_int en 1..12 y NOT NULL (solo si no hay filas que lo violen)
DO $$
DECLARE n_bad bigint;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'chk_liquidaciones_mes_periodo_int') THEN
        SELECT COUNT(*) INTO n_bad FROM public.liquidaciones
        WHERE mes_periodo_int IS NULL OR mes_periodo_int NOT BETWEEN 1 AND 12;
        IF n_bad = 0 THEN
            ALTER TABLE public.liquidaciones ADD CONSTRAINT chk_liquidaciones_mes_periodo_int CHECK (mes_periodo_int BETWEEN 1 AND 12);
            ALTER TABLE public.liquidaciones ALTER COLUMN mes_periodo_int SET NOT NULL;
        ELSE
            RAISE WARNING 'chk_liquidaciones_mes_periodo_int omitida: existen % registros que la violan; ejecutar depuracion de Fase 1', n_bad;
        END IF;
    END IF;
END $$;

-- c) Liquidaciones: un registro por patrón, empleado y periodo (NULLs tratados como iguales)
DO $$
DECLARE n bigint;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname = 'public' AND indexname = 'uq_liquidaciones_rp_emp_periodo')
       AND NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'uq_liquidaciones_rp_emp_periodo') THEN
        SELECT COUNT(*) INTO n FROM (
            SELECT 1 FROM public.liquidaciones
            GROUP BY id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int
            HAVING COUNT(*) > 1
        ) g;
        IF n = 0 THEN
            ALTER TABLE public.liquidaciones ADD CONSTRAINT uq_liquidaciones_rp_emp_periodo
                UNIQUE NULLS NOT DISTINCT (id_registro_patronal, id_empleado, anio_periodo, mes_periodo_int);
        ELSE
            RAISE WARNING 'uq_liquidaciones_rp_emp_periodo omitida: existen % registros que la violan; ejecutar depuracion de Fase 1', n;
        END IF;
    END IF;
END $$;

-- d) Reportes de liquidaciones: un registro por revisión, empleado y periodo
DO $$
DECLARE n bigint;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname = 'public' AND indexname = 'uq_rl_rev_emp_periodo')
       AND NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'uq_rl_rev_emp_periodo') THEN
        SELECT COUNT(*) INTO n FROM (
            SELECT 1 FROM public.reportes_liquidaciones
            GROUP BY id_revision, id_empleado, anio_periodo, mes_periodo_int
            HAVING COUNT(*) > 1
        ) g;
        IF n = 0 THEN
            ALTER TABLE public.reportes_liquidaciones ADD CONSTRAINT uq_rl_rev_emp_periodo
                UNIQUE NULLS NOT DISTINCT (id_revision, id_empleado, anio_periodo, mes_periodo_int);
        ELSE
            RAISE WARNING 'uq_rl_rev_emp_periodo omitida: existen % registros que la violan; ejecutar depuracion de Fase 1', n;
        END IF;
    END IF;
END $$;
"#;

/// Nombres de las restricciones de integridad de la Fase 1 que deben existir tras `SQL_FASE1_RESTRICCIONES`.
const RESTRICCIONES_FASE1: [&str; 4] = [
    "uq_empleados_cliente_nss",
    "chk_liquidaciones_mes_periodo_int",
    "uq_liquidaciones_rp_emp_periodo",
    "uq_rl_rev_emp_periodo",
];

fn exec_schema_step(
    client: &mut Client,
    step_name: &str,
    sql: &str,
) -> Result<(), postgres::Error> {
    if let Err(e) = client.batch_execute(sql) {
        let detailed = format_pg_error(&e);
        log::error!(
            "Error en ensure_schema [{}]: {}",
            step_name,
            detailed
        );
        crate::app_logger::log_db_error(
            &format!("ensure_schema::{}", step_name),
            &["public.liquidaciones", "public.reportes_liquidaciones", "revision_history"],
            step_name,
            &detailed,
        );
        return Err(e);
    }
    Ok(())
}

pub fn ensure_schema(client: &mut Client) -> Result<(), postgres::Error> {
    log::info!("Verificando y asegurando esquema de base de datos PostgreSQL...");

    // 1. Extensions & Schema (safe for concurrent execution)
    exec_schema_step(
        client,
        "1_extensions_and_schema",
        r#"
        DO $$ BEGIN
            CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
        EXCEPTION WHEN unique_violation THEN
            NULL;
        END $$;

        DO $$ BEGIN
            CREATE EXTENSION IF NOT EXISTS "pgcrypto";
        EXCEPTION WHEN unique_violation THEN
            NULL;
        END $$;

        CREATE SCHEMA IF NOT EXISTS revision_history;
        "#,
    )?;

    // 2. Base tables if they don't exist
    exec_schema_step(
        client,
        "2_base_tables",
        r#"
        CREATE TABLE IF NOT EXISTS public.imss_inhabiles (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            fecha date NOT NULL,
            dia char(10),
            motivo varchar(100),
            es_sistema boolean DEFAULT false,
            creado_sistema boolean DEFAULT true,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.tipos_personas (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            tipo char(6) NOT NULL UNIQUE,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.regimenes_fiscales (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            clave char(3) NOT NULL UNIQUE,
            nombre varchar(100) NOT NULL,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.clases (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            nombre char(3) NOT NULL UNIQUE,
            descripcion char(9) NOT NULL,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.fracciones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            codigo char(4) NOT NULL UNIQUE,
            actividad varchar(60) NOT NULL,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.clasificaciones_clientes (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_regimen_fiscal uuid REFERENCES public.regimenes_fiscales(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_tipo_persona uuid REFERENCES public.tipos_personas(id) ON UPDATE CASCADE ON DELETE CASCADE,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            CONSTRAINT uq_crps_id_regimen_fiscal_id_tipo_persona UNIQUE (id_regimen_fiscal, id_tipo_persona)
        );

        CREATE TABLE IF NOT EXISTS public.clasificaciones_registros_patronales (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_clase uuid REFERENCES public.clases(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_fraccion uuid REFERENCES public.fracciones(id) ON UPDATE CASCADE ON DELETE CASCADE,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            CONSTRAINT uq_clasificacion_riesgos_id_clase_id_fraccion UNIQUE (id_clase, id_fraccion)
        );

        CREATE TABLE IF NOT EXISTS public.clientes (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_clasificacion_cliente uuid REFERENCES public.clasificaciones_clientes(id) ON UPDATE CASCADE ON DELETE CASCADE,
            nombre varchar(300) NOT NULL UNIQUE,
            razon_social varchar(300),
            rfc char(13) UNIQUE,
            curp char(18) UNIQUE,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.entidades_federativas (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            nombre varchar(150) NOT NULL,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.delegaciones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            clave char(2) NOT NULL UNIQUE,
            nombre varchar(150) NOT NULL,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.subdelegaciones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            clave char(2) NOT NULL UNIQUE,
            nombre varchar(150) NOT NULL,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.rel_delegacion_subdelegacion (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_delegacion uuid REFERENCES public.delegaciones(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_subdelegacion uuid REFERENCES public.subdelegaciones(id) ON UPDATE CASCADE ON DELETE CASCADE,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.registros_patronales (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_cliente uuid NOT NULL REFERENCES public.clientes(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_clasificacion_regimen_persona uuid REFERENCES public.clasificaciones_registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE,
            registro char(11) NOT NULL,
            actividad varchar(300),
            domicilio varchar(500),
            codigo_postal char(5),
            entidad varchar(100),
            area_geografica char(5),
            delegacion varchar(100),
            subdelegacion varchar(100),
            prima_riesgo double precision,
            codigo_delegacion varchar(10),
            codigo_subdelegacion varchar(10),
            id_entidad_federativa uuid REFERENCES public.entidades_federativas(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_rel_del_sub uuid REFERENCES public.rel_delegacion_subdelegacion(id) ON UPDATE CASCADE ON DELETE CASCADE,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            CONSTRAINT uq_registros_patronales_id_cliente_registro UNIQUE (id_cliente, registro)
        );

        CREATE TABLE IF NOT EXISTS public.estatus (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            nombre varchar(150) NOT NULL UNIQUE,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.tipos_conceptos (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            nombre char(12) NOT NULL UNIQUE,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.conceptos (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_tipo_concepto uuid NOT NULL REFERENCES public.tipos_conceptos(id) ON UPDATE CASCADE ON DELETE CASCADE,
            tipo char(3) NOT NULL,
            nombre varchar(250) NOT NULL,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            CONSTRAINT uq_conceptos_id_tipo_concepto_tipo UNIQUE (id_tipo_concepto, tipo)
        );

        CREATE TABLE IF NOT EXISTS public.revisiones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_registro_patronal uuid NOT NULL REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_concepto_diferencia uuid REFERENCES public.conceptos(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_estatus_imss uuid REFERENCES public.estatus(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_estatus uuid REFERENCES public.estatus(id) ON UPDATE CASCADE ON DELETE CASCADE,
            nombre varchar(150),
            folio char(20),
            fecha_entrega_imss date,
            fecha_aprobacion_imss date,
            fecha_maxima_aprobacion_imss date,
            fecha_maxima_entrega_revision date,
            fecha_maxima_entrega_imss date,
            fecha_entrega_revision date,
            fecha_aprobacion_prorroga date,
            fecha_entrega_con_prorroga date,
            pidio_prorroga boolean,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            CONSTRAINT uq_revisiones_id_registro_patronal_nombre UNIQUE (id_registro_patronal, nombre),
            CONSTRAINT uq_revisiones_id_registro_patronal_folio UNIQUE (id_registro_patronal, folio)
        );

        CREATE TABLE IF NOT EXISTS public.registros_obras (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_revision uuid NOT NULL REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_registro_patronal uuid NOT NULL REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE,
            numero char(15),
            clase char(15),
            tipo_obra varchar(150),
            fecha_inicio date NOT NULL,
            fecha_fin date NOT NULL,
            ubicacion varchar(500),
            superficie_construido double precision,
            monto double precision,
            delegacion varchar(100),
            subdelegacion varchar(100),
            creado_usuario boolean DEFAULT false,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            CONSTRAINT uq_registros_obras_id_revision_numero UNIQUE (id_revision, numero)
        );

        CREATE TABLE IF NOT EXISTS public.empleados (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_cliente uuid NOT NULL REFERENCES public.clientes(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            nombre varchar(100),
            rfc char(13),
            curp char(18),
            nss char(11),
            clave_ubicacion varchar(50),
            pensionado varchar(10),
            esta_activo boolean DEFAULT true,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.cfdis (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            version_cfdi char(5),
            fecha_emision timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
            numero_certificado varchar(50),
            moneda char(3),
            tipo_comprobante char(1),
            exportacion char(2),
            metodo_pago char(3),
            serie char(5),
            folio varchar(20),
            lugar_expedicion char(20),
            subtotal double precision,
            descuento double precision,
            total double precision,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.conceptos_cfdis (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            clave_prod_serv char(8),
            clave_unidad char(5),
            descripcion varchar(150),
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.lista_conceptos_cfdis (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_cfdi uuid REFERENCES public.cfdis(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_concepto_cfdi uuid REFERENCES public.conceptos_cfdis(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            cantidad smallint,
            valor_unitario double precision,
            importe double precision,
            descuento double precision,
            objeto_imp char(5),
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.contratos (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_cfdi uuid NOT NULL REFERENCES public.cfdis(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            domicilio_fiscal varchar(300),
            regimen_fiscal char(3),
            uso_cfdi char(5),
            fecha_inicio_relacion_laboral date,
            antiguedad smallint,
            cod_antiguedad char(10),
            tipo_contrato char(5),
            es_sindicalizado boolean,
            tipo_jornada char(2),
            tipo_regimen char(5),
            num_empleado char(20),
            departamento varchar(100),
            puesto varchar(100),
            riesgo_puesto char(2),
            periodicidad_pago char(2),
            clave_entidad_federativa char(3),
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.nominas (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_cfdi uuid NOT NULL REFERENCES public.cfdis(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_registro_patronal uuid NOT NULL REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_revision uuid,
            id_empleado uuid NOT NULL REFERENCES public.empleados(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            version_nomina char(5),
            tipo_nomina char(3),
            fecha_pago date,
            fecha_inicial_pago date,
            fecha_final_pago date,
            num_dias_pagados double precision,
            total_sueldos double precision,
            total_gravado double precision,
            total_exento double precision,
            total_otras_deducciones double precision,
            total_impuestos_retenidos double precision,
            salario_base_cotizacion_aportacion double precision,
            salario_diario_integrado double precision,
            salario_diario_calculado double precision,
            salario_base_cotizacion_aportacion_calculado double precision,
            salario_diario_integrado_calculado double precision,
            timbre_version char(5),
            timbre_numero_cert_sat varchar(50),
            timbre_rfc_proveedor_cert char(13),
            timbre_uuid uuid,
            timbre_fecha timestamptz,
            nombre_archivo varchar(350),
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.detalles_nominas (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_nomina uuid NOT NULL REFERENCES public.nominas(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_concepto uuid NOT NULL REFERENCES public.conceptos(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            clave char(10) NOT NULL,
            concepto varchar(250) NOT NULL,
            importe_gravado double precision,
            importe_exento double precision,
            importe double precision,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.liquidaciones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_registro_patronal uuid NOT NULL REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_empleado uuid NOT NULL REFERENCES public.empleados(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            mes_periodo char(15),
            mes_periodo_int smallint,
            anio_periodo smallint,
            nombre_archivo varchar(350),
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.detalles_liquidaciones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_liquidacion uuid NOT NULL REFERENCES public.liquidaciones(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            clave char(17),
            fecha date,
            dias smallint,
            sdi double precision,
            lic double precision,
            inc double precision,
            aus double precision,
            cf double precision,
            exc_pat double precision,
            exc_obr double precision,
            pd_pat double precision,
            pd_obr double precision,
            gmp_pat double precision,
            gmp_obr double precision,
            rt double precision,
            iv_pat double precision,
            iv_obr double precision,
            gps double precision,
            patronal double precision,
            obrera double precision,
            subtotal double precision,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.vinculaciones_obras_liquidaciones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_registro_obra uuid NOT NULL REFERENCES public.registros_obras(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_liquidacion uuid NOT NULL REFERENCES public.liquidaciones(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            dias_registro_obra smallint,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        -- Tablas de Reportes (Working Papers)
        CREATE TABLE IF NOT EXISTS public.reportes_nominas (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_nomina uuid REFERENCES public.nominas(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_registro_patronal uuid REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_revision uuid REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_empleado uuid NOT NULL REFERENCES public.empleados(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            version_nomina char(5),
            tipo_nomina char(3),
            fecha_pago date,
            fecha_inicial_pago date,
            fecha_final_pago date,
            num_dias_pagados double precision,
            total_sueldos double precision,
            total_gravado double precision,
            total_exento double precision,
            total_otras_deducciones double precision,
            total_impuestos_retenidos double precision,
            salario_base_cotizacion_aportacion double precision,
            salario_diario_integrado double precision,
            salario_diario_calculado double precision,
            salario_base_cotizacion_aportacion_calculado double precision,
            salario_diario_integrado_calculado double precision,
            timbre_version char(5),
            timbre_numero_cert_sat varchar(50),
            timbre_rfc_proveedor_cert char(13),
            timbre_uuid uuid,
            timbre_fecha timestamptz,
            nombre_archivo varchar(350),
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.reportes_detalles_nominas (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_reporte_nomina uuid NOT NULL REFERENCES public.reportes_nominas(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_concepto uuid NOT NULL REFERENCES public.conceptos(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            clave char(10) NOT NULL,
            concepto varchar(250) NOT NULL,
            importe_gravado double precision,
            importe_exento double precision,
            importe double precision,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.reportes_liquidaciones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_liquidacion uuid REFERENCES public.liquidaciones(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_registro_patronal uuid NOT NULL REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_revision uuid NOT NULL REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_empleado uuid NOT NULL REFERENCES public.empleados(id) ON UPDATE CASCADE ON DELETE CASCADE,
            mes_periodo char(15),
            mes_periodo_int smallint,
            anio_periodo smallint,
            nombre_archivo varchar(350),
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.reportes_detalles_liquidaciones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_reporte_liquidacion uuid NOT NULL REFERENCES public.reportes_liquidaciones(id) ON UPDATE CASCADE ON DELETE CASCADE,
            clave char(17),
            fecha date,
            dias smallint,
            sdi double precision,
            lic double precision,
            inc double precision,
            aus double precision,
            cf double precision,
            exc_pat double precision,
            exc_obr double precision,
            pd_pat double precision,
            pd_obr double precision,
            gmp_pat double precision,
            gmp_obr double precision,
            rt double precision,
            iv_pat double precision,
            iv_obr double precision,
            gps double precision,
            patronal double precision,
            obrera double precision,
            subtotal double precision,
            id_registro_obra uuid REFERENCES public.registros_obras(id) ON UPDATE CASCADE ON DELETE SET NULL,
            dias_registro_obra smallint,
            tiene_inconsistencia boolean NOT NULL DEFAULT false,
            observacion_validacion text,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS public.reportes_vinculaciones_obras_liquidaciones (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            id_registro_obra uuid NOT NULL REFERENCES public.registros_obras(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_reporte_liquidacion uuid NOT NULL REFERENCES public.reportes_liquidaciones(id) ON UPDATE CASCADE ON DELETE CASCADE,
            dias_registro_obra smallint,
            created_at timestamptz DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )?;

    // 2b. Actualizar log_revision_history de forma tolerante a fallos, relajar NOT NULL en
    //     revision_history y agregar inmediatamente las columnas críticas (mes_periodo_int e id_revision)
    //     ANTES de cualquier UPDATE/DELETE de migración para que los triggers de auditoría jamás aborten
    //     la transacción ni dejen tablas sin columnas.
    exec_schema_step(
        client,
        "2b_critical_columns_and_history_safety",
        r#"
        -- Update log_revision_history trigger function to dynamically match column names using pg_attribute
        -- and wrap history insertion in an EXCEPTION block so audit history mismatches can never abort
        -- a business transaction or schema migration.
        CREATE OR REPLACE FUNCTION public.log_revision_history()
        RETURNS TRIGGER AS $$
        DECLARE
            cols TEXT;
            select_cols TEXT;
            statement TEXT;
        BEGIN
            BEGIN
                SELECT 
                    string_agg(quote_ident(a.attname), ', ' ORDER BY a.attnum),
                    string_agg(format('($1).%I', a.attname), ', ' ORDER BY a.attnum)
                INTO cols, select_cols
                FROM pg_attribute a
                JOIN pg_class c ON c.oid = a.attrelid
                JOIN pg_namespace n ON n.oid = c.relnamespace
                JOIN pg_class ch ON ch.relname = c.relname
                JOIN pg_namespace nh ON nh.oid = ch.relnamespace AND nh.nspname = 'revision_history'
                JOIN pg_attribute h ON h.attrelid = ch.oid AND h.attname = a.attname AND h.attnum > 0 AND NOT h.attisdropped
                WHERE n.nspname = 'public' AND c.relname = TG_TABLE_NAME AND a.attnum > 0 AND NOT a.attisdropped;

                IF cols IS NOT NULL AND select_cols IS NOT NULL THEN
                    statement := format(
                        'INSERT INTO revision_history.%I (%s, modified_at, event_name) SELECT %s, NOW(), $2',
                        TG_TABLE_NAME,
                        cols,
                        select_cols
                    );
                    IF (TG_OP = 'DELETE') THEN
                        EXECUTE statement USING OLD, TG_OP;
                    ELSE
                        EXECUTE statement USING NEW, TG_OP;
                    END IF;
                END IF;
            EXCEPTION WHEN OTHERS THEN
                NULL;
            END;

            IF (TG_OP = 'DELETE') THEN
                RETURN OLD;
            ELSE
                RETURN NEW;
            END IF;
        END;
        $$ LANGUAGE plpgsql;

        -- Quitar temporalmente el trigger de tope de días antes de ejecutar migraciones sobre reportes_detalles_liquidaciones
        DROP TRIGGER IF EXISTS trg_check_reportes_detalles_liquidaciones_tope ON public.reportes_detalles_liquidaciones;

        -- Agregar inmediatamente las columnas críticas en public y revision_history
        ALTER TABLE public.liquidaciones ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;
        ALTER TABLE public.liquidaciones ADD COLUMN IF NOT EXISTS mes_periodo_int smallint;
        ALTER TABLE public.reportes_liquidaciones ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;
        ALTER TABLE public.reportes_liquidaciones ADD COLUMN IF NOT EXISTS id_revision uuid;
        ALTER TABLE public.reportes_liquidaciones ADD COLUMN IF NOT EXISTS mes_periodo_int smallint;

        ALTER TABLE IF EXISTS revision_history.liquidaciones ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;
        ALTER TABLE IF EXISTS revision_history.liquidaciones ADD COLUMN IF NOT EXISTS mes_periodo_int smallint;
        ALTER TABLE IF EXISTS revision_history.reportes_liquidaciones ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;
        ALTER TABLE IF EXISTS revision_history.reportes_liquidaciones ADD COLUMN IF NOT EXISTS id_revision uuid;
        ALTER TABLE IF EXISTS revision_history.reportes_liquidaciones ADD COLUMN IF NOT EXISTS mes_periodo_int smallint;

        COMMENT ON COLUMN public.liquidaciones.mes_periodo_int IS 'Mes al que corresponde el periodo que se está liquidando en formato numerico';
        COMMENT ON COLUMN public.reportes_liquidaciones.mes_periodo_int IS 'Mes al que corresponde el periodo que se está liquidando en formato numerico';
        COMMENT ON COLUMN public.reportes_liquidaciones.id_revision IS 'Id de la revision a la que pertenece el registro de la cedula de cuota';

        -- Vincular nominas directamente al registro patronal y desacoplar obligatoriedad de revision
        ALTER TABLE public.nominas ADD COLUMN IF NOT EXISTS id_registro_patronal uuid REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
        ALTER TABLE public.nominas ALTER COLUMN id_revision DROP NOT NULL;
        ALTER TABLE public.nominas DROP CONSTRAINT IF EXISTS fk_nominas_id_revision;
        ALTER TABLE IF EXISTS revision_history.nominas ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;
        ALTER TABLE public.reportes_nominas ADD COLUMN IF NOT EXISTS id_registro_patronal uuid REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
        ALTER TABLE public.reportes_nominas ALTER COLUMN id_revision DROP NOT NULL;
        ALTER TABLE IF EXISTS revision_history.reportes_nominas ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;

        -- Población retroactiva de id_registro_patronal si existen nóminas previas con id_revision
        UPDATE public.nominas n
        SET id_registro_patronal = r.id_registro_patronal
        FROM public.revisiones r
        WHERE n.id_registro_patronal IS NULL AND n.id_revision = r.id;

        UPDATE public.reportes_nominas rn
        SET id_registro_patronal = r.id_registro_patronal
        FROM public.revisiones r
        WHERE rn.id_registro_patronal IS NULL AND rn.id_revision = r.id;

        -- Relajar NOT NULL en TODAS las columnas de revision_history y sincronizar columnas con public
        DO $$
        DECLARE
            r RECORD;
        BEGIN
            IF EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'revision_history') THEN
                FOR r IN
                    SELECT ch.relname AS tbl, h.attname AS col
                    FROM pg_attribute h
                    JOIN pg_class ch ON ch.oid = h.attrelid AND ch.relkind = 'r'
                    JOIN pg_namespace nh ON nh.oid = ch.relnamespace AND nh.nspname = 'revision_history'
                    WHERE h.attnum > 0 AND NOT h.attisdropped AND h.attnotnull
                      AND h.attname NOT IN ('modified_at', 'event_name')
                LOOP
                    EXECUTE format('ALTER TABLE revision_history.%I ALTER COLUMN %I DROP NOT NULL', r.tbl, r.col);
                END LOOP;

                FOR r IN
                    SELECT c.relname AS tbl, a.attname AS col, format_type(a.atttypid, a.atttypmod) AS typ
                    FROM pg_attribute a
                    JOIN pg_class c ON c.oid = a.attrelid AND c.relkind = 'r'
                    JOIN pg_namespace n ON n.oid = c.relnamespace AND n.nspname = 'public'
                    JOIN pg_class ch ON ch.relname = c.relname AND ch.relkind = 'r'
                    JOIN pg_namespace nh ON nh.oid = ch.relnamespace AND nh.nspname = 'revision_history'
                    WHERE a.attnum > 0 AND NOT a.attisdropped
                      AND NOT EXISTS (
                          SELECT 1 FROM pg_attribute h
                          WHERE h.attrelid = ch.oid AND h.attname = a.attname AND h.attnum > 0 AND NOT h.attisdropped
                      )
                LOOP
                    EXECUTE format('ALTER TABLE revision_history.%I ADD COLUMN IF NOT EXISTS %I %s', r.tbl, r.col, r.typ);
                END LOOP;
            END IF;
        END $$;
        "#,
    )?;

    // 3a. Estructura DDL para catálogos, columnas de cédulas y nóminas
    exec_schema_step(
        client,
        "3a_catalog_and_column_ddl",
        r#"
        -- Ensure revision_history.clasificaciones_clientes has matching column name
        DO $$
        BEGIN
            IF EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'revision_history'
                  AND table_name = 'clasificaciones_clientes'
                  AND column_name = 'id_regimen_persona'
            ) THEN
                ALTER TABLE revision_history.clasificaciones_clientes
                RENAME COLUMN id_regimen_persona TO id_regimen_fiscal;
            END IF;
        END $$;

        -- Column additions to accommodate all fields from cédulas and nóminas
        ALTER TABLE public.clientes ADD COLUMN IF NOT EXISTS representante_legal varchar(150);
        COMMENT ON COLUMN public.clientes.representante_legal IS 'Nombre del representante legal que firma los entregables ante el IMSS';
        ALTER TABLE public.registros_patronales ADD COLUMN IF NOT EXISTS codigo_delegacion varchar(10);
        ALTER TABLE public.registros_patronales ADD COLUMN IF NOT EXISTS codigo_subdelegacion varchar(10);
        ALTER TABLE public.registros_patronales ADD COLUMN IF NOT EXISTS id_entidad_federativa uuid REFERENCES public.entidades_federativas(id) ON UPDATE CASCADE ON DELETE CASCADE;
        ALTER TABLE public.registros_patronales ADD COLUMN IF NOT EXISTS id_rel_del_sub uuid REFERENCES public.rel_delegacion_subdelegacion(id) ON UPDATE CASCADE ON DELETE CASCADE;
        ALTER TABLE IF EXISTS revision_history.registros_patronales ADD COLUMN IF NOT EXISTS id_entidad_federativa uuid;
        ALTER TABLE IF EXISTS revision_history.registros_patronales ADD COLUMN IF NOT EXISTS id_rel_del_sub uuid;
        ALTER TABLE IF EXISTS revision_history.registros_patronales ADD COLUMN IF NOT EXISTS codigo_delegacion varchar(10);
        ALTER TABLE IF EXISTS revision_history.registros_patronales ADD COLUMN IF NOT EXISTS codigo_subdelegacion varchar(10);
        ALTER TABLE public.imss_inhabiles ADD COLUMN IF NOT EXISTS creado_sistema boolean DEFAULT true;
        ALTER TABLE IF EXISTS revision_history.imss_inhabiles ADD COLUMN IF NOT EXISTS creado_sistema boolean DEFAULT true;

        -- Tablas de historial de revisión y disparadores para nuevos catálogos
        CREATE TABLE IF NOT EXISTS revision_history.entidades_federativas (
            id uuid,
            nombre varchar(150),
            created_at timestamptz,
            modified_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            event_name varchar(10)
        );

        CREATE TABLE IF NOT EXISTS revision_history.delegaciones (
            id uuid,
            clave char(2),
            nombre varchar(150),
            created_at timestamptz,
            modified_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            event_name varchar(10)
        );

        CREATE TABLE IF NOT EXISTS revision_history.subdelegaciones (
            id uuid,
            clave char(2),
            nombre varchar(150),
            created_at timestamptz,
            modified_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            event_name varchar(10)
        );

        CREATE TABLE IF NOT EXISTS revision_history.rel_delegacion_subdelegacion (
            id uuid,
            id_delegacion uuid,
            id_subdelegacion uuid,
            created_at timestamptz,
            modified_at timestamptz DEFAULT CURRENT_TIMESTAMP,
            event_name varchar(10)
        );

        DROP TRIGGER IF EXISTS trg_entidades_federativas_history ON public.entidades_federativas;
        CREATE TRIGGER trg_entidades_federativas_history
        AFTER INSERT OR UPDATE OR DELETE ON public.entidades_federativas
        FOR EACH ROW EXECUTE FUNCTION public.log_revision_history();

        DROP TRIGGER IF EXISTS trg_delegaciones_history ON public.delegaciones;
        CREATE TRIGGER trg_delegaciones_history
        AFTER INSERT OR UPDATE OR DELETE ON public.delegaciones
        FOR EACH ROW EXECUTE FUNCTION public.log_revision_history();

        DROP TRIGGER IF EXISTS trg_subdelegaciones_history ON public.subdelegaciones;
        CREATE TRIGGER trg_subdelegaciones_history
        AFTER INSERT OR UPDATE OR DELETE ON public.subdelegaciones
        FOR EACH ROW EXECUTE FUNCTION public.log_revision_history();

        DROP TRIGGER IF EXISTS trg_rel_del_sub_history ON public.rel_delegacion_subdelegacion;
        CREATE TRIGGER trg_rel_del_sub_history
        AFTER INSERT OR UPDATE OR DELETE ON public.rel_delegacion_subdelegacion
        FOR EACH ROW EXECUTE FUNCTION public.log_revision_history();

        ALTER TABLE public.empleados ADD COLUMN IF NOT EXISTS clave_ubicacion varchar(50);
        ALTER TABLE public.empleados ADD COLUMN IF NOT EXISTS pensionado varchar(10);
        ALTER TABLE public.empleados ADD COLUMN IF NOT EXISTS esta_activo boolean DEFAULT true;
        ALTER TABLE public.registros_obras ADD COLUMN IF NOT EXISTS creado_usuario boolean;
        ALTER TABLE IF EXISTS revision_history.registros_obras ADD COLUMN IF NOT EXISTS creado_usuario boolean;
        ALTER TABLE public.reportes_vinculaciones_obras_liquidaciones ALTER COLUMN id_registro_obra DROP NOT NULL;
        ALTER TABLE IF EXISTS revision_history.reportes_vinculaciones_obras_liquidaciones ALTER COLUMN id_registro_obra DROP NOT NULL;
        ALTER TABLE public.liquidaciones DROP COLUMN IF EXISTS dias;
        ALTER TABLE public.reportes_liquidaciones DROP COLUMN IF EXISTS dias;
        ALTER TABLE IF EXISTS revision_history.liquidaciones DROP COLUMN IF EXISTS dias;
        ALTER TABLE IF EXISTS revision_history.reportes_liquidaciones DROP COLUMN IF EXISTS dias;

        ALTER TABLE public.liquidaciones ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;
        ALTER TABLE public.liquidaciones ADD COLUMN IF NOT EXISTS mes_periodo_int smallint;
        ALTER TABLE public.reportes_liquidaciones ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;
        ALTER TABLE public.reportes_liquidaciones ADD COLUMN IF NOT EXISTS id_revision uuid;
        ALTER TABLE public.reportes_liquidaciones ADD COLUMN IF NOT EXISTS mes_periodo_int smallint;

        COMMENT ON COLUMN public.liquidaciones.mes_periodo_int IS 'Mes al que corresponde el periodo que se está liquidando en formato numerico';
        COMMENT ON COLUMN public.reportes_liquidaciones.mes_periodo_int IS 'Mes al que corresponde el periodo que se está liquidando en formato numerico';
        COMMENT ON COLUMN public.reportes_liquidaciones.id_revision IS 'Id de la revision a la que pertenece el registro de la cedula de cuota';

        -- Vincular nominas directamente al registro patronal y desacoplar obligatoriedad de revision
        ALTER TABLE public.nominas ADD COLUMN IF NOT EXISTS id_registro_patronal uuid REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
        ALTER TABLE public.nominas ALTER COLUMN id_revision DROP NOT NULL;
        ALTER TABLE public.nominas DROP CONSTRAINT IF EXISTS fk_nominas_id_revision;
        ALTER TABLE IF EXISTS revision_history.nominas ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;
        ALTER TABLE public.reportes_nominas ADD COLUMN IF NOT EXISTS id_registro_patronal uuid REFERENCES public.registros_patronales(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
        ALTER TABLE public.reportes_nominas ALTER COLUMN id_revision DROP NOT NULL;
        ALTER TABLE IF EXISTS revision_history.reportes_nominas ADD COLUMN IF NOT EXISTS id_registro_patronal uuid;

        -- Población retroactiva de id_registro_patronal si existen nóminas previas con id_revision
        UPDATE public.nominas n
        SET id_registro_patronal = r.id_registro_patronal
        FROM public.revisiones r
        WHERE n.id_registro_patronal IS NULL AND n.id_revision = r.id;

        UPDATE public.reportes_nominas rn
        SET id_registro_patronal = r.id_registro_patronal
        FROM public.revisiones r
        WHERE rn.id_registro_patronal IS NULL AND rn.id_revision = r.id;
        "#,
    )?;

    // 3b. Migración DML de datos y depuración de registros huérfanos.
    //     Se ejecuta y CONFIRMA en su propia transacción para que todos los eventos de triggers
    //     y comprobaciones diferidas se resuelvan antes de aplicar restricciones DDL en 3c.
    exec_schema_step(
        client,
        "3b_cedulas_data_migration_dml",
        r#"
        DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_registro_obra IS NULL;

        DO $$
        BEGIN
            -- Migrar datos si aún existe id_revision en public.liquidaciones
            IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'liquidaciones' AND column_name = 'id_revision') THEN
                EXECUTE $bf1$
                    INSERT INTO public.registros_obras (
                        id, id_revision, id_registro_patronal, numero, tipo_obra, fecha_inicio, fecha_fin, creado_usuario
                    )
                    SELECT 
                        uuidv7(),
                        l.id_revision,
                        rev.id_registro_patronal,
                        LEFT(TRIM(e.clave_ubicacion), 15),
                        'REGISTRO DE OBRA CÉDULA',
                        COALESCE(
                            TO_DATE(l.anio_periodo::text || '-' || 
                                CASE UPPER(TRIM(l.mes_periodo))
                                    WHEN 'ENERO' THEN '01'
                                    WHEN 'FEBRERO' THEN '02'
                                    WHEN 'MARZO' THEN '03'
                                    WHEN 'ABRIL' THEN '04'
                                    WHEN 'MAYO' THEN '05'
                                    WHEN 'JUNIO' THEN '06'
                                    WHEN 'JULIO' THEN '07'
                                    WHEN 'AGOSTO' THEN '08'
                                    WHEN 'SEPTIEMBRE' THEN '09'
                                    WHEN 'OCTUBRE' THEN '10'
                                    WHEN 'NOVIEMBRE' THEN '11'
                                    WHEN 'DICIEMBRE' THEN '12'
                                    ELSE '01'
                                END || '-01', 'YYYY-MM-DD'),
                            CURRENT_DATE
                        ),
                        COALESCE(
                            TO_DATE(l.anio_periodo::text || '-' || 
                                CASE UPPER(TRIM(l.mes_periodo))
                                    WHEN 'ENERO' THEN '01'
                                    WHEN 'FEBRERO' THEN '02'
                                    WHEN 'MARZO' THEN '03'
                                    WHEN 'ABRIL' THEN '04'
                                    WHEN 'MAYO' THEN '05'
                                    WHEN 'JUNIO' THEN '06'
                                    WHEN 'JULIO' THEN '07'
                                    WHEN 'AGOSTO' THEN '08'
                                    WHEN 'SEPTIEMBRE' THEN '09'
                                    WHEN 'OCTUBRE' THEN '10'
                                    WHEN 'NOVIEMBRE' THEN '11'
                                    WHEN 'DICIEMBRE' THEN '12'
                                    ELSE '01'
                                END || '-01', 'YYYY-MM-DD') + INTERVAL '1 month' - INTERVAL '1 day',
                            CURRENT_DATE
                        ),
                        false
                    FROM public.liquidaciones l
                    JOIN public.revisiones rev ON rev.id = l.id_revision
                    JOIN public.empleados e ON e.id = l.id_empleado
                    WHERE e.clave_ubicacion IS NOT NULL 
                      AND TRIM(e.clave_ubicacion) <> '' 
                      AND TRIM(e.clave_ubicacion) <> '—'
                      AND TRIM(e.clave_ubicacion) <> '-'
                      AND (
                          TRIM(e.clave_ubicacion) ~* '^C-?[0-9]{7,8}$'
                          OR TRIM(e.clave_ubicacion) ~* '^[0-9]{2}-?[0-9]{5,6}-?[0-9]{2}$'
                          OR TRIM(e.clave_ubicacion) ~* '^[0-9]{8,10}$'
                      )
                    ON CONFLICT (id_revision, numero) DO NOTHING;
                $bf1$;

                EXECUTE $bf2$
                    INSERT INTO public.vinculaciones_obras_liquidaciones (id, id_registro_obra, id_liquidacion, dias_registro_obra)
                    SELECT uuidv7(), ro.id, l.id, 30
                    FROM public.liquidaciones l
                    JOIN public.empleados e ON e.id = l.id_empleado
                    JOIN public.registros_obras ro ON ro.id_revision = l.id_revision AND ro.numero = LEFT(TRIM(e.clave_ubicacion), 15)
                    WHERE NOT EXISTS (
                        SELECT 1 FROM public.vinculaciones_obras_liquidaciones vol 
                        WHERE vol.id_liquidacion = l.id
                    );
                $bf2$;

                UPDATE public.liquidaciones l
                SET id_registro_patronal = rev.id_registro_patronal
                FROM public.revisiones rev
                WHERE l.id_revision = rev.id AND l.id_registro_patronal IS NULL;
            END IF;

            -- Migrar y asegurar columnas mes_periodo_int (liquidaciones y reportes_liquidaciones) e id_revision (reportes_liquidaciones)
            IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'liquidaciones') THEN
                UPDATE public.liquidaciones
                SET mes_periodo_int = CASE UPPER(TRIM(COALESCE(mes_periodo, '')))
                    WHEN 'ENERO' THEN 1
                    WHEN 'FEBRERO' THEN 2
                    WHEN 'MARZO' THEN 3
                    WHEN 'ABRIL' THEN 4
                    WHEN 'MAYO' THEN 5
                    WHEN 'JUNIO' THEN 6
                    WHEN 'JULIO' THEN 7
                    WHEN 'AGOSTO' THEN 8
                    WHEN 'SEPTIEMBRE' THEN 9
                    WHEN 'OCTUBRE' THEN 10
                    WHEN 'NOVIEMBRE' THEN 11
                    WHEN 'DICIEMBRE' THEN 12
                    WHEN '1' THEN 1 WHEN '01' THEN 1
                    WHEN '2' THEN 2 WHEN '02' THEN 2
                    WHEN '3' THEN 3 WHEN '03' THEN 3
                    WHEN '4' THEN 4 WHEN '04' THEN 4
                    WHEN '5' THEN 5 WHEN '05' THEN 5
                    WHEN '6' THEN 6 WHEN '06' THEN 6
                    WHEN '7' THEN 7 WHEN '07' THEN 7
                    WHEN '8' THEN 8 WHEN '08' THEN 8
                    WHEN '9' THEN 9 WHEN '09' THEN 9
                    WHEN '10' THEN 10
                    WHEN '11' THEN 11
                    WHEN '12' THEN 12
                    ELSE 1
                END
                WHERE mes_periodo_int IS NULL;
            END IF;

            IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'reportes_liquidaciones') THEN
                UPDATE public.reportes_liquidaciones rl
                SET id_registro_patronal = rev.id_registro_patronal
                FROM public.revisiones rev
                WHERE rl.id_revision = rev.id AND rl.id_registro_patronal IS NULL;

                UPDATE public.reportes_liquidaciones rl
                SET id_registro_patronal = l.id_registro_patronal
                FROM public.liquidaciones l
                WHERE rl.id_liquidacion = l.id AND rl.id_registro_patronal IS NULL;

                UPDATE public.reportes_liquidaciones rl
                SET id_revision = sub.id_revision
                FROM (
                    SELECT DISTINCT ON (rvol.id_reporte_liquidacion)
                        rvol.id_reporte_liquidacion,
                        ro.id_revision
                    FROM public.reportes_vinculaciones_obras_liquidaciones rvol
                    JOIN public.registros_obras ro ON ro.id = rvol.id_registro_obra
                    WHERE ro.id_revision IS NOT NULL
                ) sub
                WHERE rl.id = sub.id_reporte_liquidacion AND rl.id_revision IS NULL;

                UPDATE public.reportes_liquidaciones rl
                SET id_revision = (
                    SELECT rev.id
                    FROM public.revisiones rev
                    WHERE rev.id_registro_patronal = rl.id_registro_patronal
                    ORDER BY rev.created_at DESC NULLS LAST, rev.id DESC
                    LIMIT 1
                )
                WHERE rl.id_revision IS NULL;

                UPDATE public.reportes_liquidaciones
                SET mes_periodo_int = CASE UPPER(TRIM(COALESCE(mes_periodo, '')))
                    WHEN 'ENERO' THEN 1
                    WHEN 'FEBRERO' THEN 2
                    WHEN 'MARZO' THEN 3
                    WHEN 'ABRIL' THEN 4
                    WHEN 'MAYO' THEN 5
                    WHEN 'JUNIO' THEN 6
                    WHEN 'JULIO' THEN 7
                    WHEN 'AGOSTO' THEN 8
                    WHEN 'SEPTIEMBRE' THEN 9
                    WHEN 'OCTUBRE' THEN 10
                    WHEN 'NOVIEMBRE' THEN 11
                    WHEN 'DICIEMBRE' THEN 12
                    WHEN '1' THEN 1 WHEN '01' THEN 1
                    WHEN '2' THEN 2 WHEN '02' THEN 2
                    WHEN '3' THEN 3 WHEN '03' THEN 3
                    WHEN '4' THEN 4 WHEN '04' THEN 4
                    WHEN '5' THEN 5 WHEN '05' THEN 5
                    WHEN '6' THEN 6 WHEN '06' THEN 6
                    WHEN '7' THEN 7 WHEN '07' THEN 7
                    WHEN '8' THEN 8 WHEN '08' THEN 8
                    WHEN '9' THEN 9 WHEN '09' THEN 9
                    WHEN '10' THEN 10
                    WHEN '11' THEN 11
                    WHEN '12' THEN 12
                    ELSE 1
                END
                WHERE mes_periodo_int IS NULL;
            END IF;

            -- Limpiar huérfanos antes de imponer NOT NULL y Foreign Keys
            IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'liquidaciones' AND column_name = 'id_registro_patronal') THEN
                DELETE FROM public.detalles_liquidaciones dl
                WHERE EXISTS (
                    SELECT 1 FROM public.liquidaciones l
                    WHERE l.id = dl.id_liquidacion
                      AND (l.id_registro_patronal IS NULL OR NOT EXISTS (SELECT 1 FROM public.registros_patronales rp WHERE rp.id = l.id_registro_patronal))
                );
                DELETE FROM public.vinculaciones_obras_liquidaciones vol
                WHERE EXISTS (
                    SELECT 1 FROM public.liquidaciones l
                    WHERE l.id = vol.id_liquidacion
                      AND (l.id_registro_patronal IS NULL OR NOT EXISTS (SELECT 1 FROM public.registros_patronales rp WHERE rp.id = l.id_registro_patronal))
                );
                DELETE FROM public.reportes_detalles_liquidaciones rdl
                WHERE EXISTS (
                    SELECT 1 FROM public.reportes_liquidaciones rl
                    JOIN public.liquidaciones l ON l.id = rl.id_liquidacion
                    WHERE rl.id = rdl.id_reporte_liquidacion
                      AND (l.id_registro_patronal IS NULL OR NOT EXISTS (SELECT 1 FROM public.registros_patronales rp WHERE rp.id = l.id_registro_patronal))
                );
                DELETE FROM public.reportes_vinculaciones_obras_liquidaciones rvol
                WHERE EXISTS (
                    SELECT 1 FROM public.reportes_liquidaciones rl
                    JOIN public.liquidaciones l ON l.id = rl.id_liquidacion
                    WHERE rl.id = rvol.id_reporte_liquidacion
                      AND (l.id_registro_patronal IS NULL OR NOT EXISTS (SELECT 1 FROM public.registros_patronales rp WHERE rp.id = l.id_registro_patronal))
                );
                DELETE FROM public.reportes_liquidaciones rl
                WHERE EXISTS (
                    SELECT 1 FROM public.liquidaciones l
                    WHERE l.id = rl.id_liquidacion
                      AND (l.id_registro_patronal IS NULL OR NOT EXISTS (SELECT 1 FROM public.registros_patronales rp WHERE rp.id = l.id_registro_patronal))
                );
                DELETE FROM public.liquidaciones l
                WHERE l.id_registro_patronal IS NULL
                   OR NOT EXISTS (SELECT 1 FROM public.registros_patronales rp WHERE rp.id = l.id_registro_patronal);
            END IF;

            IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'reportes_liquidaciones' AND column_name = 'id_registro_patronal') THEN
                DELETE FROM public.reportes_detalles_liquidaciones rdl
                WHERE EXISTS (
                    SELECT 1 FROM public.reportes_liquidaciones rl
                    WHERE rl.id = rdl.id_reporte_liquidacion
                      AND (
                          rl.id_registro_patronal IS NULL
                          OR rl.id_revision IS NULL
                          OR NOT EXISTS (SELECT 1 FROM public.registros_patronales rp WHERE rp.id = rl.id_registro_patronal)
                          OR NOT EXISTS (SELECT 1 FROM public.revisiones rev WHERE rev.id = rl.id_revision)
                      )
                );
                DELETE FROM public.reportes_vinculaciones_obras_liquidaciones rvol
                WHERE EXISTS (
                    SELECT 1 FROM public.reportes_liquidaciones rl
                    WHERE rl.id = rvol.id_reporte_liquidacion
                      AND (
                          rl.id_registro_patronal IS NULL
                          OR rl.id_revision IS NULL
                          OR NOT EXISTS (SELECT 1 FROM public.registros_patronales rp WHERE rp.id = rl.id_registro_patronal)
                          OR NOT EXISTS (SELECT 1 FROM public.revisiones rev WHERE rev.id = rl.id_revision)
                      )
                );
                DELETE FROM public.reportes_liquidaciones rl
                WHERE rl.id_registro_patronal IS NULL
                   OR rl.id_revision IS NULL
                   OR NOT EXISTS (SELECT 1 FROM public.registros_patronales rp WHERE rp.id = rl.id_registro_patronal)
                   OR NOT EXISTS (SELECT 1 FROM public.revisiones rev WHERE rev.id = rl.id_revision);
            END IF;
        END $$;
        "#,
    )?;

    // 3c. Aplicación segura de restricciones DDL (NOT NULL y Foreign Keys).
    //     Se ejecuta en una transacción limpia sin operaciones DML previas sobre reportes_liquidaciones
    //     y comprueba explícitamente is_nullable = 'YES' para no generar bloqueos ni errores 55006.
    exec_schema_step(
        client,
        "3c_cedulas_constraints_ddl",
        r#"
        -- Desvincular id_revision de liquidaciones si aún existe
        DO $$
        DECLARE
            r RECORD;
        BEGIN
            IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'liquidaciones' AND column_name = 'id_revision') THEN
                FOR r IN (
                    SELECT conname 
                    FROM pg_constraint 
                    WHERE conrelid = 'public.liquidaciones'::regclass 
                      AND contype = 'f' 
                      AND conname ILIKE '%revision%'
                ) LOOP
                    EXECUTE 'ALTER TABLE public.liquidaciones DROP CONSTRAINT IF EXISTS ' || quote_ident(r.conname);
                END LOOP;

                DROP INDEX IF EXISTS public.idx_liquidaciones_id_revision;
                DROP INDEX IF EXISTS public.idx_liquidaciones_rev_periodo;
                DROP INDEX IF EXISTS public.idx_liquidaciones_emp_rev;

                ALTER TABLE public.liquidaciones DROP COLUMN IF EXISTS id_revision CASCADE;
            END IF;
        END $$;

        -- Enforce NOT NULL y Foreign Keys condicionando a is_nullable = 'YES'
        DO $$
        BEGIN
            -- 1. liquidaciones.id_registro_patronal
            IF EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = 'liquidaciones'
                  AND column_name = 'id_registro_patronal'
                  AND is_nullable = 'YES'
            ) THEN
                ALTER TABLE public.liquidaciones ALTER COLUMN id_registro_patronal SET NOT NULL;
            END IF;

            IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_liquidaciones_registro_patronal') THEN
                ALTER TABLE public.liquidaciones ADD CONSTRAINT fk_liquidaciones_registro_patronal
                    FOREIGN KEY (id_registro_patronal) REFERENCES public.registros_patronales(id)
                    ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
            END IF;

            -- 2. reportes_liquidaciones.id_registro_patronal
            IF EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = 'reportes_liquidaciones'
                  AND column_name = 'id_registro_patronal'
                  AND is_nullable = 'YES'
            ) THEN
                ALTER TABLE public.reportes_liquidaciones ALTER COLUMN id_registro_patronal SET NOT NULL;
            END IF;

            -- 3. reportes_liquidaciones.id_revision
            IF EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = 'reportes_liquidaciones'
                  AND column_name = 'id_revision'
                  AND is_nullable = 'YES'
            ) THEN
                ALTER TABLE public.reportes_liquidaciones ALTER COLUMN id_revision SET NOT NULL;
            END IF;

            IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_reportes_liquidaciones_registro_patronal') THEN
                ALTER TABLE public.reportes_liquidaciones ADD CONSTRAINT fk_reportes_liquidaciones_registro_patronal
                    FOREIGN KEY (id_registro_patronal) REFERENCES public.registros_patronales(id)
                    ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
            END IF;

            IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_reportes_liquidaciones_revision') THEN
                ALTER TABLE public.reportes_liquidaciones ADD CONSTRAINT fk_reportes_liquidaciones_revision
                    FOREIGN KEY (id_revision) REFERENCES public.revisiones(id)
                    ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
            END IF;
        END $$;
        "#,
    )?;

    // 3d. Alineación de revision_history, índices de rendimiento y reglas de validación
    exec_schema_step(
        client,
        "3d_history_indices_and_rules",
        r#"
        -- Alineación de revision_history con public:
        --   1. Agrega a revision_history las columnas que existen en public y faltan en el historial.
        --   2. Quita NOT NULL a todas las columnas de las tablas de revision_history.
        DO $$
        DECLARE
            r RECORD;
        BEGIN
            IF EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'revision_history') THEN
                FOR r IN
                    SELECT c.relname AS tbl, a.attname AS col, format_type(a.atttypid, a.atttypmod) AS typ
                    FROM pg_attribute a
                    JOIN pg_class c ON c.oid = a.attrelid AND c.relkind = 'r'
                    JOIN pg_namespace n ON n.oid = c.relnamespace AND n.nspname = 'public'
                    JOIN pg_class ch ON ch.relname = c.relname AND ch.relkind = 'r'
                    JOIN pg_namespace nh ON nh.oid = ch.relnamespace AND nh.nspname = 'revision_history'
                    WHERE a.attnum > 0 AND NOT a.attisdropped
                      AND NOT EXISTS (
                          SELECT 1 FROM pg_attribute h
                          WHERE h.attrelid = ch.oid AND h.attname = a.attname AND h.attnum > 0 AND NOT h.attisdropped
                      )
                LOOP
                    EXECUTE format('ALTER TABLE revision_history.%I ADD COLUMN IF NOT EXISTS %I %s', r.tbl, r.col, r.typ);
                END LOOP;

                FOR r IN
                    SELECT ch.relname AS tbl, h.attname AS col
                    FROM pg_attribute h
                    JOIN pg_class ch ON ch.oid = h.attrelid AND ch.relkind = 'r'
                    JOIN pg_namespace nh ON nh.oid = ch.relnamespace AND nh.nspname = 'revision_history'
                    WHERE h.attnum > 0 AND NOT h.attisdropped AND h.attnotnull
                      AND h.attname NOT IN ('modified_at', 'event_name')
                LOOP
                    EXECUTE format('ALTER TABLE revision_history.%I ALTER COLUMN %I DROP NOT NULL', r.tbl, r.col);
                END LOOP;
            END IF;
        END $$;

        -- Column additions to allow row-level obra and days per detail in reportes_detalles_liquidaciones
        ALTER TABLE public.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS id_registro_obra uuid REFERENCES public.registros_obras(id) ON UPDATE CASCADE ON DELETE SET NULL;
        ALTER TABLE public.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS dias_registro_obra smallint;
        ALTER TABLE public.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS tiene_inconsistencia boolean NOT NULL DEFAULT false;
        ALTER TABLE public.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS observacion_validacion text;
        ALTER TABLE IF EXISTS revision_history.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS id_registro_obra uuid;
        ALTER TABLE IF EXISTS revision_history.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS dias_registro_obra smallint;
        ALTER TABLE IF EXISTS revision_history.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS tiene_inconsistencia boolean NOT NULL DEFAULT false;
        ALTER TABLE IF EXISTS revision_history.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS observacion_validacion text;

        -- Reparación histórica (deshabilitada para preservar ediciones del usuario en Reporte Liquidaciones)
        -- UPDATE public.reportes_detalles_liquidaciones rdl SET dias = dl.dias ...

        -- Inicializar id_registro_obra y dias_registro_obra en reportes_detalles_liquidaciones desde rvol si aún son nulos
        UPDATE public.reportes_detalles_liquidaciones rdl
        SET id_registro_obra = rvol.id_registro_obra,
            dias_registro_obra = CASE 
                WHEN UPPER(TRIM(COALESCE(rdl.clave, ''))) = 'BAJA' THEN 0 
                WHEN rvol.dias_registro_obra IS NOT NULL AND rvol.dias_registro_obra <= rdl.dias THEN rvol.dias_registro_obra
                ELSE rdl.dias
            END
        FROM public.reportes_vinculaciones_obras_liquidaciones rvol
        WHERE rdl.id_reporte_liquidacion = rvol.id_reporte_liquidacion
          AND rdl.id_registro_obra IS NULL
          AND rvol.id_registro_obra IS NOT NULL;

        -- Reparación específica previa (deshabilitada tras migración para no activar trigger de validación de tope de días)
        -- UPDATE public.reportes_detalles_liquidaciones rdl SET dias = 21, dias_registro_obra = 22 ...

        -- Ensure imss_inhabiles es_sistema column and unique constraint
        ALTER TABLE public.imss_inhabiles ADD COLUMN IF NOT EXISTS es_sistema boolean DEFAULT false;
        ALTER TABLE IF EXISTS revision_history.imss_inhabiles ADD COLUMN IF NOT EXISTS es_sistema boolean DEFAULT false;
        DO $$
        BEGIN
            IF NOT EXISTS (
                SELECT 1 FROM pg_constraint WHERE conname = 'uq_imss_inhabiles_fecha'
            ) AND NOT EXISTS (
                SELECT 1 FROM pg_constraint WHERE conname = 'imss_inhabiles_fecha_key'
            ) THEN
                DELETE FROM public.imss_inhabiles a USING public.imss_inhabiles b
                WHERE a.id > b.id AND a.fecha = b.fecha;

                ALTER TABLE public.imss_inhabiles ADD CONSTRAINT uq_imss_inhabiles_fecha UNIQUE (fecha);
            END IF;
        END $$;

        -- Seed standard official IMSS holidays
        INSERT INTO public.imss_inhabiles (fecha, dia, motivo, es_sistema) VALUES
            ('2024-01-01', 'LUNES', 'Año Nuevo', true),
            ('2024-02-05', 'LUNES', 'Aniversario de la Constitución', true),
            ('2024-03-18', 'LUNES', 'Natalicio de Benito Juárez', true),
            ('2024-03-28', 'JUEVES', 'Jueves Santo (Inhábil IMSS)', true),
            ('2024-03-29', 'VIERNES', 'Viernes Santo (Inhábil IMSS)', true),
            ('2024-05-01', 'MIÉRCOLES', 'Día del Trabajo', true),
            ('2024-09-16', 'LUNES', 'Día de la Independencia', true),
            ('2024-10-01', 'MARTES', 'Transmisión del Poder Ejecutivo Federal', true),
            ('2024-11-18', 'LUNES', 'Revolución Mexicana', true),
            ('2024-12-25', 'MIÉRCOLES', 'Navidad', true),

            ('2025-01-01', 'MIÉRCOLES', 'Año Nuevo', true),
            ('2025-02-03', 'LUNES', 'Aniversario de la Constitución', true),
            ('2025-03-17', 'LUNES', 'Natalicio de Benito Juárez', true),
            ('2025-04-17', 'JUEVES', 'Jueves Santo (Inhábil IMSS)', true),
            ('2025-04-18', 'VIERNES', 'Viernes Santo (Inhábil IMSS)', true),
            ('2025-05-01', 'JUEVES', 'Día del Trabajo', true),
            ('2025-09-16', 'MARTES', 'Día de la Independencia', true),
            ('2025-11-17', 'LUNES', 'Revolución Mexicana', true),
            ('2025-12-25', 'JUEVES', 'Navidad', true),

            ('2026-01-01', 'JUEVES', 'Año Nuevo', true),
            ('2026-02-02', 'LUNES', 'Aniversario de la Constitución', true),
            ('2026-03-16', 'LUNES', 'Natalicio de Benito Juárez', true),
            ('2026-04-02', 'JUEVES', 'Jueves Santo (Inhábil IMSS)', true),
            ('2026-04-03', 'VIERNES', 'Viernes Santo (Inhábil IMSS)', true),
            ('2026-05-01', 'VIERNES', 'Día del Trabajo', true),
            ('2026-09-16', 'MIÉRCOLES', 'Día de la Independencia', true),
            ('2026-11-16', 'LUNES', 'Revolución Mexicana', true),
            ('2026-12-25', 'VIERNES', 'Navidad', true),

            ('2027-01-01', 'VIERNES', 'Año Nuevo', true),
            ('2027-02-01', 'LUNES', 'Aniversario de la Constitución', true),
            ('2027-03-15', 'LUNES', 'Natalicio de Benito Juárez', true),
            ('2027-03-25', 'JUEVES', 'Jueves Santo (Inhábil IMSS)', true),
            ('2027-03-26', 'VIERNES', 'Viernes Santo (Inhábil IMSS)', true),
            ('2027-05-01', 'SÁBADO', 'Día del Trabajo', true),
            ('2027-09-16', 'JUEVES', 'Día de la Independencia', true),
            ('2027-11-15', 'LUNES', 'Revolución Mexicana', true),
            ('2027-12-25', 'SÁBADO', 'Navidad', true)
        ON CONFLICT (fecha) DO UPDATE SET es_sistema = true;

        -- Add columns to revision_history if exists
        DO $$
        BEGIN
            IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'revision_history' AND table_name = 'registros_patronales') THEN
                ALTER TABLE revision_history.registros_patronales ADD COLUMN IF NOT EXISTS codigo_delegacion varchar(10);
                ALTER TABLE revision_history.registros_patronales ADD COLUMN IF NOT EXISTS codigo_subdelegacion varchar(10);
            END IF;
            IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'revision_history' AND table_name = 'empleados') THEN
                ALTER TABLE revision_history.empleados ADD COLUMN IF NOT EXISTS clave_ubicacion varchar(50);
                ALTER TABLE revision_history.empleados ADD COLUMN IF NOT EXISTS pensionado varchar(10);
                ALTER TABLE revision_history.empleados ADD COLUMN IF NOT EXISTS esta_activo boolean DEFAULT true;
            END IF;
            IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'revision_history' AND table_name = 'registros_obras') THEN
                ALTER TABLE revision_history.registros_obras ADD COLUMN IF NOT EXISTS creado_usuario boolean;
            END IF;
        END $$;

        -- Fix lista_conceptos_cfdis foreign key and data type
        DO $$
        BEGIN
            IF EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'public' AND table_name = 'lista_conceptos_cfdis' AND column_name = 'id_concepto_cfdi' AND data_type = 'integer'
            ) THEN
                ALTER TABLE public.lista_conceptos_cfdis DROP CONSTRAINT IF EXISTS fk_lista_conceptos_cfdis_id_concepto_cfdi;
                ALTER TABLE public.lista_conceptos_cfdis ALTER COLUMN id_concepto_cfdi TYPE uuid USING NULL;
                IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'revision_history' AND table_name = 'lista_conceptos_cfdis') THEN
                    ALTER TABLE revision_history.lista_conceptos_cfdis ALTER COLUMN id_concepto_cfdi TYPE uuid USING NULL;
                END IF;
                ALTER TABLE public.lista_conceptos_cfdis ADD CONSTRAINT fk_lista_conceptos_cfdis_id_concepto_cfdi
                    FOREIGN KEY (id_concepto_cfdi) REFERENCES public.conceptos_cfdis(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
            END IF;

            -- Reparar restricción errónea si fk_lista_conceptos_cfdis_id_concepto_cfdi quedó apuntando sobre la columna id_cfdi
            IF EXISTS (
                SELECT 1 FROM information_schema.key_column_usage
                WHERE table_schema = 'public' AND table_name = 'lista_conceptos_cfdis'
                  AND constraint_name = 'fk_lista_conceptos_cfdis_id_concepto_cfdi'
                  AND column_name = 'id_cfdi'
            ) THEN
                ALTER TABLE public.lista_conceptos_cfdis DROP CONSTRAINT fk_lista_conceptos_cfdis_id_concepto_cfdi;
                ALTER TABLE public.lista_conceptos_cfdis ADD CONSTRAINT fk_lista_conceptos_cfdis_id_concepto_cfdi
                    FOREIGN KEY (id_concepto_cfdi) REFERENCES public.conceptos_cfdis(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
            END IF;
        END $$;



        -- Default catalogs: tipos_conceptos
        INSERT INTO public.tipos_conceptos (nombre) VALUES ('PERCEPCIONES') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.tipos_conceptos (nombre) VALUES ('DEDUCCIONES') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.tipos_conceptos (nombre) VALUES ('OTROS PAGOS') ON CONFLICT (nombre) DO NOTHING;

        -- Default estatus
        INSERT INTO public.estatus (nombre) VALUES ('INICIADA') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.estatus (nombre) VALUES ('EN REVISIÓN') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.estatus (nombre) VALUES ('EN PROCESO DE ACLARACIÓN') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.estatus (nombre) VALUES ('APROBADA POR EMPRESA') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.estatus (nombre) VALUES ('PENDIENTE DICTAMEN IMSS') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.estatus (nombre) VALUES ('APROBADA POR IMSS') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.estatus (nombre) VALUES ('RECHAZADA') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.estatus (nombre) VALUES ('APROBADA') ON CONFLICT (nombre) DO NOTHING;
        INSERT INTO public.estatus (nombre) VALUES ('FINALIZADA') ON CONFLICT (nombre) DO NOTHING;

        -- Default conceptos de diferencias para revisiones
        DO $$
        DECLARE
            tipo_id uuid;
        BEGIN
            SELECT id INTO tipo_id FROM public.tipos_conceptos WHERE nombre = 'PERCEPCIONES' LIMIT 1;
            IF tipo_id IS NOT NULL THEN
                INSERT INTO public.conceptos (id_tipo_concepto, tipo, nombre)
                VALUES 
                    (tipo_id, '001', 'Diferencia en Determinación de Cuotas SBC y Días Inhábiles (Art. 173)'),
                    (tipo_id, '002', 'Omisión en Avisos de Incidencias e Integración Salarial Variables'),
                    (tipo_id, '003', 'Diferencias en Factor de Integración y Prestaciones Contractuales'),
                    (tipo_id, '004', 'Rectificación de Prima de Riesgo de Trabajo y Clasificación')
                ON CONFLICT (id_tipo_concepto, tipo) DO NOTHING;
            END IF;
        END $$;

        -- Índices de Rendimiento para Claves Foráneas y Eliminaciones en Cascada
        CREATE INDEX IF NOT EXISTS idx_registros_patronales_id_cliente ON public.registros_patronales(id_cliente);
        CREATE INDEX IF NOT EXISTS idx_revisiones_id_registro_patronal ON public.revisiones(id_registro_patronal);
        CREATE INDEX IF NOT EXISTS idx_revisiones_id_concepto_diferencia ON public.revisiones(id_concepto_diferencia);
        CREATE INDEX IF NOT EXISTS idx_registros_obras_id_revision ON public.registros_obras(id_revision);
        CREATE INDEX IF NOT EXISTS idx_registros_obras_id_registro_patronal ON public.registros_obras(id_registro_patronal);
        CREATE INDEX IF NOT EXISTS idx_empleados_id_cliente ON public.empleados(id_cliente);
        CREATE INDEX IF NOT EXISTS idx_lista_conceptos_cfdis_id_cfdi ON public.lista_conceptos_cfdis(id_cfdi);
        CREATE INDEX IF NOT EXISTS idx_lista_conceptos_cfdis_id_concepto_cfdi ON public.lista_conceptos_cfdis(id_concepto_cfdi);
        CREATE INDEX IF NOT EXISTS idx_contratos_id_cfdi ON public.contratos(id_cfdi);
        CREATE INDEX IF NOT EXISTS idx_nominas_id_cfdi ON public.nominas(id_cfdi);
        CREATE INDEX IF NOT EXISTS idx_nominas_id_registro_patronal ON public.nominas(id_registro_patronal);
        CREATE INDEX IF NOT EXISTS idx_nominas_emp_rp ON public.nominas(id_empleado, id_registro_patronal);
        CREATE INDEX IF NOT EXISTS idx_nominas_id_revision ON public.nominas(id_revision);
        CREATE INDEX IF NOT EXISTS idx_nominas_id_empleado ON public.nominas(id_empleado);
        CREATE INDEX IF NOT EXISTS idx_detalles_nominas_id_nomina ON public.detalles_nominas(id_nomina);
        CREATE INDEX IF NOT EXISTS idx_detalles_nominas_id_concepto ON public.detalles_nominas(id_concepto);
        CREATE INDEX IF NOT EXISTS idx_reportes_nominas_id_rp ON public.reportes_nominas(id_registro_patronal);
        CREATE INDEX IF NOT EXISTS idx_liquidaciones_id_rp ON public.liquidaciones(id_registro_patronal);
        CREATE INDEX IF NOT EXISTS idx_liquidaciones_id_empleado ON public.liquidaciones(id_empleado);
        CREATE INDEX IF NOT EXISTS idx_detalles_liquidaciones_id_liquidacion ON public.detalles_liquidaciones(id_liquidacion);
        CREATE INDEX IF NOT EXISTS idx_vinculaciones_obras_id_obra ON public.vinculaciones_obras_liquidaciones(id_registro_obra);
        CREATE INDEX IF NOT EXISTS idx_vinculaciones_obras_id_liq ON public.vinculaciones_obras_liquidaciones(id_liquidacion);
        CREATE UNIQUE INDEX IF NOT EXISTS uq_vol_liq_obra_id ON public.vinculaciones_obras_liquidaciones(id_liquidacion, id_registro_obra);
        CREATE INDEX IF NOT EXISTS idx_reportes_nominas_id_nomina ON public.reportes_nominas(id_nomina);
        CREATE INDEX IF NOT EXISTS idx_reportes_nominas_id_revision ON public.reportes_nominas(id_revision);
        CREATE INDEX IF NOT EXISTS idx_reportes_nominas_id_empleado ON public.reportes_nominas(id_empleado);
        CREATE INDEX IF NOT EXISTS idx_reportes_detalles_nominas_id_rep ON public.reportes_detalles_nominas(id_reporte_nomina);
        CREATE INDEX IF NOT EXISTS idx_reportes_liquidaciones_id_rp ON public.reportes_liquidaciones(id_registro_patronal);
        CREATE INDEX IF NOT EXISTS idx_reportes_liquidaciones_id_revision ON public.reportes_liquidaciones(id_revision);
        CREATE INDEX IF NOT EXISTS idx_reportes_liquidaciones_id_liq ON public.reportes_liquidaciones(id_liquidacion);
        CREATE INDEX IF NOT EXISTS idx_reportes_liquidaciones_rp_periodo ON public.reportes_liquidaciones(id_registro_patronal, anio_periodo, mes_periodo);
        CREATE INDEX IF NOT EXISTS idx_reportes_liquidaciones_rp_rev_periodo_int ON public.reportes_liquidaciones(id_registro_patronal, id_revision, anio_periodo, mes_periodo_int);
        CREATE INDEX IF NOT EXISTS idx_liquidaciones_rp_periodo ON public.liquidaciones(id_registro_patronal, anio_periodo, mes_periodo);
        CREATE INDEX IF NOT EXISTS idx_liquidaciones_rp_periodo_int ON public.liquidaciones(id_registro_patronal, anio_periodo, mes_periodo_int);
        CREATE INDEX IF NOT EXISTS idx_reportes_detalles_liq_id_rep ON public.reportes_detalles_liquidaciones(id_reporte_liquidacion);
        CREATE INDEX IF NOT EXISTS idx_reportes_detalles_liq_id_obra ON public.reportes_detalles_liquidaciones(id_registro_obra);
        CREATE INDEX IF NOT EXISTS idx_reportes_vinc_obras_id_rep ON public.reportes_vinculaciones_obras_liquidaciones(id_reporte_liquidacion);
        CREATE INDEX IF NOT EXISTS idx_reportes_vinc_obras_id_obra ON public.reportes_vinculaciones_obras_liquidaciones(id_registro_obra);
        CREATE INDEX IF NOT EXISTS idx_detalles_liq_id_clave_fecha ON public.detalles_liquidaciones(id_liquidacion, clave, fecha);
        CREATE INDEX IF NOT EXISTS idx_reportes_detalles_liq_id_clave_fecha ON public.reportes_detalles_liquidaciones(id_reporte_liquidacion, clave, fecha);
        CREATE INDEX IF NOT EXISTS idx_nominas_emp_fecha ON public.nominas(id_empleado, fecha_pago DESC NULLS LAST);
        CREATE INDEX IF NOT EXISTS idx_nominas_emp_rev ON public.nominas(id_empleado, id_revision);
        CREATE INDEX IF NOT EXISTS idx_liquidaciones_emp_rp ON public.liquidaciones(id_empleado, id_registro_patronal);
        CREATE INDEX IF NOT EXISTS idx_registros_patronales_id_entidad_fed ON public.registros_patronales(id_entidad_federativa);
        CREATE INDEX IF NOT EXISTS idx_registros_patronales_id_rel_del_sub ON public.registros_patronales(id_rel_del_sub);
        CREATE INDEX IF NOT EXISTS idx_rel_del_sub_id_del ON public.rel_delegacion_subdelegacion(id_delegacion);
        CREATE INDEX IF NOT EXISTS idx_rel_del_sub_id_subdel ON public.rel_delegacion_subdelegacion(id_subdelegacion);
        CREATE INDEX IF NOT EXISTS idx_empleados_cliente_nss ON public.empleados(id_cliente, nss);
        CREATE INDEX IF NOT EXISTS idx_empleados_cliente_curp ON public.empleados(id_cliente, curp);
        CREATE INDEX IF NOT EXISTS idx_empleados_cliente_rfc ON public.empleados(id_cliente, rfc);
        CREATE INDEX IF NOT EXISTS idx_empleados_cliente_nombre ON public.empleados(id_cliente, nombre);
        CREATE INDEX IF NOT EXISTS idx_reportes_liquidaciones_rp_emp ON public.reportes_liquidaciones(id_registro_patronal, id_empleado);
        CREATE INDEX IF NOT EXISTS idx_reportes_detalles_liq_combo ON public.reportes_detalles_liquidaciones(id_reporte_liquidacion, id_registro_obra);
        CREATE INDEX IF NOT EXISTS idx_reportes_detalles_nom_rep ON public.reportes_detalles_nominas(id_reporte_nomina, id_concepto);

        -- Sincronización automática de id_entidad_federativa si coincide con el catálogo
        UPDATE public.registros_patronales rp
        SET id_entidad_federativa = ef.id
        FROM public.entidades_federativas ef
        WHERE rp.id_entidad_federativa IS NULL
          AND rp.entidad IS NOT NULL
          AND TRIM(UPPER(rp.entidad)) = TRIM(UPPER(ef.nombre));

        -- Sincronización automática de id_rel_del_sub si coinciden delegación y subdelegación
        UPDATE public.registros_patronales rp
        SET id_rel_del_sub = rds.id
        FROM public.rel_delegacion_subdelegacion rds
        JOIN public.delegaciones d ON rds.id_delegacion = d.id
        JOIN public.subdelegaciones s ON rds.id_subdelegacion = s.id
        WHERE rp.id_rel_del_sub IS NULL
          AND ((rp.codigo_delegacion IS NOT NULL AND TRIM(rp.codigo_delegacion) = TRIM(d.clave)
                AND rp.codigo_subdelegacion IS NOT NULL AND TRIM(rp.codigo_subdelegacion) = TRIM(s.clave))
               OR (rp.delegacion IS NOT NULL AND TRIM(UPPER(rp.delegacion)) = TRIM(UPPER(d.nombre))
                AND rp.subdelegacion IS NOT NULL AND TRIM(UPPER(rp.subdelegacion)) = TRIM(UPPER(s.nombre))));

        -- Migración: columna creado_usuario en public.registros_obras
        ALTER TABLE public.registros_obras ADD COLUMN IF NOT EXISTS creado_usuario boolean;
        ALTER TABLE IF EXISTS revision_history.registros_obras ADD COLUMN IF NOT EXISTS creado_usuario boolean;
        UPDATE public.registros_obras SET creado_usuario = true WHERE creado_usuario IS NULL;
        ALTER TABLE public.registros_obras ALTER COLUMN creado_usuario SET DEFAULT false;

        -- Limpiar registros de obra inválidos (RFC/CURP guardados por error o no conformes a SIROC/SATIC).
        -- Conservar estrictamente cualquier obra creada o vinculada por el usuario (creado_usuario = true).
        UPDATE public.reportes_detalles_liquidaciones rdl
        SET id_registro_obra = NULL, dias_registro_obra = 0
        FROM public.registros_obras ro
        WHERE rdl.id_registro_obra = ro.id
          AND COALESCE(ro.creado_usuario, false) = false
          AND (TRIM(ro.numero) ~* '^[A-Z]{3,4}[0-9]{6}'
           OR (TRIM(ro.numero) !~* '^C-?[0-9]{7,8}$' AND TRIM(ro.numero) !~* '^[0-9]{2}-?[0-9]{5,6}-?[0-9]{2}$' AND TRIM(ro.numero) !~* '^[0-9]{8,10}$'));

        DELETE FROM public.reportes_vinculaciones_obras_liquidaciones rvol
        USING public.registros_obras ro
        WHERE rvol.id_registro_obra = ro.id
          AND COALESCE(ro.creado_usuario, false) = false
          AND (TRIM(ro.numero) ~* '^[A-Z]{3,4}[0-9]{6}'
           OR (TRIM(ro.numero) !~* '^C-?[0-9]{7,8}$' AND TRIM(ro.numero) !~* '^[0-9]{2}-?[0-9]{5,6}-?[0-9]{2}$' AND TRIM(ro.numero) !~* '^[0-9]{8,10}$'));

        DELETE FROM public.vinculaciones_obras_liquidaciones vol
        USING public.registros_obras ro
        WHERE vol.id_registro_obra = ro.id
          AND COALESCE(ro.creado_usuario, false) = false
          AND (TRIM(ro.numero) ~* '^[A-Z]{3,4}[0-9]{6}'
           OR (TRIM(ro.numero) !~* '^C-?[0-9]{7,8}$' AND TRIM(ro.numero) !~* '^[0-9]{2}-?[0-9]{5,6}-?[0-9]{2}$' AND TRIM(ro.numero) !~* '^[0-9]{8,10}$'));

        DELETE FROM public.registros_obras
        WHERE COALESCE(creado_usuario, false) = false
          AND (TRIM(numero) ~* '^[A-Z]{3,4}[0-9]{6}'
           OR (TRIM(numero) !~* '^C-?[0-9]{7,8}$' AND TRIM(numero) !~* '^[0-9]{2}-?[0-9]{5,6}-?[0-9]{2}$' AND TRIM(numero) !~* '^[0-9]{8,10}$'));

        DO $$
        BEGIN
            IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'revision_history' AND table_name = 'registros_obras') THEN
                DELETE FROM revision_history.registros_obras
                WHERE COALESCE(creado_usuario, false) = false
                  AND (TRIM(numero) ~* '^[A-Z]{3,4}[0-9]{6}'
                   OR (TRIM(numero) !~* '^C-?[0-9]{7,8}$' AND TRIM(numero) !~* '^[0-9]{2}-?[0-9]{5,6}-?[0-9]{2}$' AND TRIM(numero) !~* '^[0-9]{8,10}$'));
            END IF;
        END $$;

        -- Actualizar restricciones de días de obra para permitir 0 días (desasignación / días en cero)
        ALTER TABLE public.reportes_vinculaciones_obras_liquidaciones DROP CONSTRAINT IF EXISTS chk_rvol_dias_registro_obra;
        ALTER TABLE public.reportes_vinculaciones_obras_liquidaciones ADD CONSTRAINT chk_rvol_dias_registro_obra CHECK (dias_registro_obra IS NULL OR (dias_registro_obra >= 0 AND dias_registro_obra <= 31));

        ALTER TABLE public.vinculaciones_obras_liquidaciones DROP CONSTRAINT IF EXISTS chk_vinculaciones_obras_liquidaciones_dias_registro_obra;
        ALTER TABLE public.vinculaciones_obras_liquidaciones ADD CONSTRAINT chk_vinculaciones_obras_liquidaciones_dias_registro_obra CHECK (dias_registro_obra IS NULL OR (dias_registro_obra >= 0 AND dias_registro_obra <= 31));

        -- Limpiar entidad, domicilio y actividad en registros_patronales que contienen sufijos de encabezado SUA
        UPDATE public.registros_patronales 
        SET entidad = TRIM(REGEXP_REPLACE(entidad, '\s*(PRIMA\s+DE\s+R\.?T\.?|V\s*[0-9]|VERSI[OÓ]N).*$', '', 'i')) 
        WHERE entidad ~* '(PRIMA|V\s*[0-9]|VERSI)';

        UPDATE public.registros_patronales 
        SET domicilio = TRIM(REGEXP_REPLACE(domicilio, '\s*(POB\.?\s*MUN\.?\s*(/|\s)?\s*ALCALD[IÍ]A|POBLACI[OÓ]N|MUNICIPIO|ALCALD[IÍ]A)\s*:?.*$', '', 'i')) 
        WHERE domicilio ~* '(POB\.?\s*MUN|POBLACI[OÓ]N|ALCALD[IÍ]A)';

        UPDATE public.registros_patronales 
        SET actividad = TRIM(REGEXP_REPLACE(actividad, '\s*(SUBDELEGACI[OÓ]N|DELEGACI[OÓ]N)\s*(IMSS)?\s*:?.*$', '', 'i')) 
        WHERE actividad ~* '(SUBDELEGACI[OÓ]N|DELEGACI[OÓ]N)';

        -- Limpiar nombres de empleados existentes que contengan identificadores de pensionado (P/CV, P/IV, etc.)
        UPDATE public.empleados
        SET pensionado = 'P/CV',
            nombre = TRIM(REGEXP_REPLACE(nombre, '^\s*P\s*[/ -]?\s*C\.?V\.?\s*', '', 'i'))
        WHERE nombre ~* '^\s*P\s*[/ -]?\s*C\.?V\.?\s*'
          AND (pensionado IS NULL OR pensionado = '');

        UPDATE public.empleados
        SET pensionado = 'P/IV',
            nombre = TRIM(REGEXP_REPLACE(nombre, '^\s*P\s*[/ -]?\s*I\.?V\.?\s*', '', 'i'))
        WHERE nombre ~* '^\s*P\s*[/ -]?\s*I\.?V\.?\s*'
          AND (pensionado IS NULL OR pensionado = '');

        UPDATE public.empleados
        SET pensionado = 'P',
            nombre = TRIM(REGEXP_REPLACE(nombre, '^\s*P\s+', '', 'i'))
        WHERE nombre ~* '^\s*P\s+'
          AND nombre !~* '^\s*P\s*[/ -]'
          AND (pensionado IS NULL OR pensionado = '');

        -- Corregir empleados donde la CURP o RFC se guardó erróneamente en clave_ubicacion
        UPDATE public.empleados
        SET curp = TRIM(clave_ubicacion), clave_ubicacion = NULL
        WHERE (curp IS NULL OR TRIM(curp) = '')
          AND TRIM(clave_ubicacion) ~* '^[A-Z]{4}[0-9]{6}[A-Z0-9]{7,8}$';

        UPDATE public.empleados
        SET rfc = TRIM(clave_ubicacion), clave_ubicacion = NULL
        WHERE (rfc IS NULL OR TRIM(rfc) = '')
          AND TRIM(clave_ubicacion) ~* '^[A-Z&Ñ]{3,4}-?[0-9]{6}(-[A-Z0-9]{3})?$';

        UPDATE public.empleados
        SET clave_ubicacion = NULL
        WHERE TRIM(clave_ubicacion) ~* '^[A-Z]{4}[0-9]{6}[A-Z0-9]{7,8}$'
           OR TRIM(clave_ubicacion) ~* '^[A-Z&Ñ]{3,4}-?[0-9]{6}(-[A-Z0-9]{3})?$';

        -- Descartar RFC que en realidad son palabras del nombre del trabajador.
        -- El parser antiguo tenía un grupo opcional demasiado permisivo que capturaba
        -- tokens alfabéticos finales (p. ej. "JIMENEZ DE LA FUENTE MARIA ALEJANDRA" ->
        -- rfc = 'ALEJANDRA'), porque las alternativas RFC/CURP/clave_ubicacion eran
        -- todas opcionales. Un RFC de persona física siempre contiene dígitos y
        -- mide 12 o 13 caracteres una vez normalizado.
        -- `rfc` es char(13): hay que comparar sobre TRIM(rfc) por el relleno.
        UPDATE public.empleados
        SET rfc = NULL
        WHERE rfc IS NOT NULL
          AND TRIM(rfc) <> ''
          AND (TRIM(rfc) !~ '[0-9]' OR LENGTH(TRIM(rfc)) NOT IN (12, 13));

        UPDATE public.empleados
        SET curp = NULL
        WHERE curp IS NOT NULL
          AND TRIM(curp) <> ''
          AND LENGTH(TRIM(curp)) <> 18;
        -- Backfill completado en bloque dinámico de migración de esquema.

        -- Idempotencia de nóminas: índice ÚNICO PARCIAL sobre el UUID del timbre.
        --
        -- Reprocesar el mismo ZIP creaba un `cfdis`, un `contratos`, una
        -- `nominas` y N `detalles_nominas` nuevos cada vez, porque el INSERT
        -- genera siempre un `uuidv7()` y la tabla no tenía ninguna restricción
        -- sobre `timbre_uuid`. El reporte de nóminas suma sus filas sin
        -- DISTINCT, así que los totales quedaban duplicados.
        --
        -- Es PARCIAL (`WHERE timbre_uuid IS NOT NULL`) porque las nóminas sin
        -- timbre legítimamenteNULL no deben colisionar entre sí.
        --
        -- La migración es SEGURA: si ya existen duplicados históricos, NO borra
        -- nada y NO crea el índice; solo avisa por log para que la resolución sea
        -- una decisión consciente del usuario.
        DO $$
        DECLARE
            dup_count BIGINT;
        BEGIN
            IF EXISTS (
                SELECT 1
                FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = 'nominas'
            ) THEN
                SELECT COUNT(*) INTO dup_count FROM (
                    SELECT timbre_uuid
                    FROM public.nominas
                    WHERE timbre_uuid IS NOT NULL
                    GROUP BY timbre_uuid
                    HAVING COUNT(*) > 1
                ) d;

                IF dup_count > 0 THEN
                    RAISE WARNING 'Nóminas duplicadas por timbre_uuid: % grupo(s). NO se creó el índice único; resuelve los duplicados y reejecuta.', dup_count;
                ELSIF NOT EXISTS (
                    SELECT 1 FROM pg_indexes
                    WHERE schemaname = 'public'
                      AND tablename = 'nominas'
                      AND indexname = 'uq_nominas_timbre_uuid'
                ) THEN
                    CREATE UNIQUE INDEX uq_nominas_timbre_uuid
                        ON public.nominas (timbre_uuid)
                        WHERE timbre_uuid IS NOT NULL;
                    RAISE NOTICE 'Índice único parcial uq_nominas_timbre_uuid creado.';
                END IF;
            END IF;
        END $$;

        -- Eliminación de restricción dura para permitir inserciones permisivas de auditoría
        DO $$
        BEGIN
            IF EXISTS (
                SELECT 1 FROM pg_constraint WHERE conname = 'chk_reportes_detalles_dias_obra'
            ) THEN
                ALTER TABLE public.reportes_detalles_liquidaciones
                DROP CONSTRAINT chk_reportes_detalles_dias_obra;
            END IF;
        END $$;

        -- Columnas de auditoría e inconsistencias permisivas en reportes_detalles_liquidaciones
        ALTER TABLE public.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS tiene_inconsistencia boolean NOT NULL DEFAULT false;
        ALTER TABLE public.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS observacion_validacion text;
        ALTER TABLE IF EXISTS revision_history.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS tiene_inconsistencia boolean NOT NULL DEFAULT false;
        ALTER TABLE IF EXISTS revision_history.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS observacion_validacion text;

        -- Función y disparador permisivo (NO-BLOQUEANTE):
        -- En lugar de RAISE EXCEPTION, marca 'tiene_inconsistencia = TRUE' y detalla la observación
        -- para que el 100% de los datos se guarden y el auditor corrija en la interfaz Slint.
        CREATE OR REPLACE FUNCTION public.check_reportes_detalles_liquidaciones_tope_dias()
        RETURNS TRIGGER AS $$
        DECLARE
            v_obra_ini DATE;
            v_obra_fin DATE;
            v_id_empleado UUID;
            v_mes VARCHAR(15);
            v_anio SMALLINT;
            v_mes_num INT;
            v_month_start DATE;
            v_month_end DATE;
            v_obra_start DATE;
            v_obra_end DATE;
            v_current_start DATE;
            v_overlap_days INT := 0;
            r_mov RECORD;
            v_first_kind TEXT := NULL;
            v_has_movements BOOLEAN := FALSE;
            v_inter_start DATE;
            v_inter_end DATE;
            v_ov_s DATE;
            v_ov_e DATE;
            v_obs TEXT := NULL;
        BEGIN
            -- Reiniciar estado de inconsistencia por defecto
            NEW.tiene_inconsistencia := FALSE;
            NEW.observacion_validacion := NULL;

            -- Validar límites básicos de días (sin abortar transacción)
            IF NEW.dias_registro_obra IS NOT NULL THEN
                IF NEW.dias_registro_obra < 0 THEN
                    NEW.tiene_inconsistencia := TRUE;
                    v_obs := format('Los días de obra no pueden ser negativos (%s).', NEW.dias_registro_obra);
                ELSIF NEW.dias_registro_obra > 31 THEN
                    NEW.tiene_inconsistencia := TRUE;
                    v_obs := format('Los días de obra (%s) exceden el tope mensual de 31 días.', NEW.dias_registro_obra);
                END IF;
            END IF;

            -- Validar compatibilidad de obra y movimientos si hay obra asignada con días > 0
            IF NEW.id_registro_obra IS NOT NULL AND NEW.dias_registro_obra IS NOT NULL AND NEW.dias_registro_obra > 0 THEN
                -- Obtener fechas de la obra
                SELECT fecha_inicio, fecha_fin INTO v_obra_ini, v_obra_fin
                FROM public.registros_obras
                WHERE id = NEW.id_registro_obra;

                -- Obtener datos del empleado y periodo
                SELECT rl.id_empleado, UPPER(TRIM(rl.mes_periodo)), rl.anio_periodo
                INTO v_id_empleado, v_mes, v_anio
                FROM public.reportes_liquidaciones rl
                WHERE rl.id = NEW.id_reporte_liquidacion;

                IF v_id_empleado IS NOT NULL AND v_mes IS NOT NULL AND v_anio IS NOT NULL THEN
                    v_mes_num := CASE v_mes
                        WHEN 'ENERO' THEN 1 WHEN 'FEBRERO' THEN 2 WHEN 'MARZO' THEN 3
                        WHEN 'ABRIL' THEN 4 WHEN 'MAYO' THEN 5 WHEN 'JUNIO' THEN 6
                        WHEN 'JULIO' THEN 7 WHEN 'AGOSTO' THEN 8 WHEN 'SEPTIEMBRE' THEN 9
                        WHEN 'OCTUBRE' THEN 10 WHEN 'NOVIEMBRE' THEN 11 WHEN 'DICIEMBRE' THEN 12
                        ELSE NULL
                    END;

                    IF v_mes_num IS NOT NULL THEN
                        v_month_start := make_date(v_anio::int, v_mes_num, 1);
                        v_month_end := (v_month_start + interval '1 month' - interval '1 day')::date;
                        v_obra_start := GREATEST(v_month_start, COALESCE(v_obra_ini, v_month_start));
                        v_obra_end := LEAST(v_month_end, COALESCE(v_obra_fin, v_month_end));

                        IF v_obra_ini IS NOT NULL OR v_obra_fin IS NOT NULL THEN
                            IF v_obra_start > v_month_end OR v_obra_end < v_month_start OR v_obra_start > v_obra_end THEN
                                NEW.tiene_inconsistencia := TRUE;
                                v_obs := COALESCE(v_obs || ' | ', '') || format('La obra no tiene vigencia en el periodo %s/%s.', v_mes, v_anio);
                            END IF;
                        END IF;

                        -- Determinar primer movimiento del empleado en este mes
                        SELECT CASE WHEN LOWER(TRIM(rdl2.clave)) LIKE '%baja%' THEN 'BAJA' ELSE 'ALTA' END
                        INTO v_first_kind
                        FROM public.reportes_detalles_liquidaciones rdl2
                        JOIN public.reportes_liquidaciones rl2 ON rdl2.id_reporte_liquidacion = rl2.id
                        WHERE rl2.id_empleado = v_id_empleado
                          AND UPPER(TRIM(rl2.mes_periodo)) = v_mes
                          AND rl2.anio_periodo = v_anio
                          AND rdl2.fecha IS NOT NULL
                          AND (LOWER(TRIM(rdl2.clave)) LIKE '%baja%' OR LOWER(TRIM(rdl2.clave)) LIKE '%alta%' OR LOWER(TRIM(rdl2.clave)) LIKE '%reing%')
                        ORDER BY rdl2.fecha ASC, (CASE WHEN LOWER(TRIM(rdl2.clave)) LIKE '%baja%' THEN 0 ELSE 1 END) ASC
                        LIMIT 1;

                        IF v_first_kind IS NOT NULL THEN
                            v_has_movements := TRUE;
                            IF v_first_kind = 'BAJA' THEN
                                v_current_start := v_month_start;
                            ELSE
                                v_current_start := NULL;
                            END IF;

                            FOR r_mov IN
                                SELECT
                                    CASE WHEN LOWER(TRIM(rdl2.clave)) LIKE '%baja%' THEN 'BAJA' ELSE 'ALTA' END AS mov_tipo,
                                    rdl2.fecha AS mov_fecha
                                FROM public.reportes_detalles_liquidaciones rdl2
                                JOIN public.reportes_liquidaciones rl2 ON rdl2.id_reporte_liquidacion = rl2.id
                                WHERE rl2.id_empleado = v_id_empleado
                                  AND UPPER(TRIM(rl2.mes_periodo)) = v_mes
                                  AND rl2.anio_periodo = v_anio
                                  AND rdl2.fecha IS NOT NULL
                                  AND (LOWER(TRIM(rdl2.clave)) LIKE '%baja%' OR LOWER(TRIM(rdl2.clave)) LIKE '%alta%' OR LOWER(TRIM(rdl2.clave)) LIKE '%reing%')
                                ORDER BY rdl2.fecha ASC, (CASE WHEN LOWER(TRIM(rdl2.clave)) LIKE '%baja%' THEN 0 ELSE 1 END) ASC
                            LOOP
                                IF r_mov.mov_tipo = 'BAJA' THEN
                                    IF v_current_start IS NOT NULL THEN
                                        v_inter_start := v_current_start;
                                        v_inter_end := LEAST(r_mov.mov_fecha, v_month_end);
                                        IF v_inter_end >= v_inter_start THEN
                                            v_ov_s := GREATEST(v_inter_start, v_obra_start);
                                            v_ov_e := LEAST(v_inter_end, v_obra_end);
                                            IF v_ov_s <= v_ov_e THEN
                                                v_overlap_days := v_overlap_days + (v_ov_e - v_ov_s + 1);
                                            END IF;
                                        END IF;
                                        v_current_start := NULL;
                                    END IF;
                                ELSE -- ALTA / REING
                                    IF v_current_start IS NULL THEN
                                        v_current_start := GREATEST(r_mov.mov_fecha, v_month_start);
                                    END IF;
                                END IF;
                            END LOOP;

                            IF v_current_start IS NOT NULL AND v_month_end >= v_current_start THEN
                                v_inter_start := v_current_start;
                                v_inter_end := v_month_end;
                                v_ov_s := GREATEST(v_inter_start, v_obra_start);
                                v_ov_e := LEAST(v_inter_end, v_obra_end);
                                IF v_ov_s <= v_ov_e THEN
                                    v_overlap_days := v_overlap_days + (v_ov_e - v_ov_s + 1);
                                END IF;
                            END IF;
                        ELSE
                            v_ov_s := GREATEST(v_month_start, v_obra_start);
                            v_ov_e := LEAST(v_month_end, v_obra_end);
                            IF v_ov_s <= v_ov_e THEN
                                v_overlap_days := (v_ov_e - v_ov_s + 1);
                            ELSE
                                v_overlap_days := 0;
                            END IF;
                        END IF;

                        IF (v_has_movements OR v_obra_ini IS NOT NULL OR v_obra_fin IS NOT NULL) AND v_overlap_days = 0 THEN
                            NEW.tiene_inconsistencia := TRUE;
                            v_obs := COALESCE(v_obs || ' | ', '') || format('Fechas de obra no coinciden con periodos activos del trabajador en %s/%s (vigencia: 0 días).', v_mes, v_anio);
                        END IF;

                        IF (v_has_movements OR v_obra_ini IS NOT NULL OR v_obra_fin IS NOT NULL) AND NEW.dias_registro_obra > v_overlap_days THEN
                            NEW.tiene_inconsistencia := TRUE;
                            v_obs := COALESCE(v_obs || ' | ', '') || format('Días de obra (%s) exceden los días de vigencia de obra (%s).', NEW.dias_registro_obra, v_overlap_days);
                        END IF;

                        -- Validar suma total de días entre todas las obras del MISMO renglón de movimiento
                        DECLARE
                            v_other_days INT := 0;
                            r_other RECORD;
                        BEGIN
                            SELECT COALESCE(SUM(rdl_other.dias_registro_obra), 0) INTO v_other_days
                            FROM public.reportes_detalles_liquidaciones rdl_other
                            WHERE rdl_other.id_reporte_liquidacion = NEW.id_reporte_liquidacion
                              AND rdl_other.id != NEW.id
                              AND rdl_other.id_registro_obra IS NOT NULL
                              AND COALESCE(UPPER(TRIM(rdl_other.clave)), '') = COALESCE(UPPER(TRIM(NEW.clave)), '')
                              AND rdl_other.fecha IS NOT DISTINCT FROM NEW.fecha;

                            IF NEW.dias IS NOT NULL AND (v_other_days + NEW.dias_registro_obra) > NEW.dias THEN
                                NEW.tiene_inconsistencia := TRUE;
                                v_obs := COALESCE(v_obs || ' | ', '') || format('La suma de días de obra (%s + %s) supera los días de cédula del empleado (%s).', v_other_days, NEW.dias_registro_obra, NEW.dias);
                            END IF;

                            FOR r_other IN
                                SELECT ro_other.numero, ro_other.fecha_inicio, ro_other.fecha_fin, rdl_other.id_registro_obra
                                FROM public.reportes_detalles_liquidaciones rdl_other
                                JOIN public.registros_obras ro_other ON ro_other.id = rdl_other.id_registro_obra
                                WHERE rdl_other.id_reporte_liquidacion = NEW.id_reporte_liquidacion
                                  AND rdl_other.id != NEW.id
                                  AND rdl_other.id_registro_obra IS NOT NULL
                                  AND rdl_other.dias_registro_obra > 0
                                  AND COALESCE(UPPER(TRIM(rdl_other.clave)), '') = COALESCE(UPPER(TRIM(NEW.clave)), '')
                                  AND rdl_other.fecha IS NOT DISTINCT FROM NEW.fecha
                            LOOP
                                IF r_other.id_registro_obra = NEW.id_registro_obra THEN
                                    NEW.tiene_inconsistencia := TRUE;
                                    v_obs := COALESCE(v_obs || ' | ', '') || format('El trabajador ya se encuentra vinculado a la obra %s en este periodo.', r_other.numero);
                                END IF;
                            END LOOP;
                        END;
                    END IF;
                END IF;
            END IF;

            NEW.observacion_validacion := v_obs;
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;

        DROP TRIGGER IF EXISTS trg_check_reportes_detalles_liquidaciones_tope ON public.reportes_detalles_liquidaciones;
        CREATE TRIGGER trg_check_reportes_detalles_liquidaciones_tope
        BEFORE INSERT OR UPDATE ON public.reportes_detalles_liquidaciones
        FOR EACH ROW EXECUTE FUNCTION public.check_reportes_detalles_liquidaciones_tope_dias();

        -- Triggers y migraciones para garantizar que TODA la información se guarde en MAYÚSCULAS
        -- y que NSS, RFC y CURP queden completamente limpios (sin '-' ni caracteres inválidos)
        CREATE OR REPLACE FUNCTION public.clean_and_uppercase_entity_fields()
        RETURNS TRIGGER AS $$
        BEGIN
            IF TG_TABLE_NAME = 'empleados' THEN
                IF NEW.nss IS NOT NULL THEN
                    NEW.nss := UPPER(REGEXP_REPLACE(NEW.nss, '[^A-Za-z0-9]', '', 'g'));
                END IF;
                IF NEW.rfc IS NOT NULL THEN
                    NEW.rfc := UPPER(REGEXP_REPLACE(NEW.rfc, '[^A-Za-z0-9&Ññ]', '', 'g'));
                END IF;
                IF NEW.curp IS NOT NULL THEN
                    NEW.curp := UPPER(REGEXP_REPLACE(NEW.curp, '[^A-Za-z0-9]', '', 'g'));
                END IF;
                IF NEW.nombre IS NOT NULL THEN
                    NEW.nombre := UPPER(NEW.nombre);
                END IF;
                IF NEW.pensionado IS NOT NULL THEN
                    NEW.pensionado := UPPER(NEW.pensionado);
                END IF;
                IF NEW.clave_ubicacion IS NOT NULL THEN
                    NEW.clave_ubicacion := UPPER(NEW.clave_ubicacion);
                END IF;
            ELSIF TG_TABLE_NAME = 'clientes' THEN
                IF NEW.rfc IS NOT NULL THEN
                    NEW.rfc := UPPER(REGEXP_REPLACE(NEW.rfc, '[^A-Za-z0-9&Ññ]', '', 'g'));
                END IF;
                IF NEW.curp IS NOT NULL THEN
                    NEW.curp := UPPER(REGEXP_REPLACE(NEW.curp, '[^A-Za-z0-9]', '', 'g'));
                END IF;
                IF NEW.nombre IS NOT NULL THEN
                    NEW.nombre := UPPER(NEW.nombre);
                END IF;
                IF NEW.razon_social IS NOT NULL THEN
                    NEW.razon_social := UPPER(NEW.razon_social);
                END IF;
                IF NEW.representante_legal IS NOT NULL THEN
                    NEW.representante_legal := UPPER(BTRIM(NEW.representante_legal));
                END IF;
            ELSIF TG_TABLE_NAME = 'registros_patronales' THEN
                IF NEW.registro IS NOT NULL THEN
                    NEW.registro := UPPER(REGEXP_REPLACE(NEW.registro, '[^A-Za-z0-9]', '', 'g'));
                END IF;
                IF NEW.actividad IS NOT NULL THEN
                    NEW.actividad := UPPER(NEW.actividad);
                END IF;
                IF NEW.domicilio IS NOT NULL THEN
                    NEW.domicilio := UPPER(NEW.domicilio);
                END IF;
            ELSIF TG_TABLE_NAME = 'registros_obras' THEN
                IF NEW.numero IS NOT NULL THEN
                    NEW.numero := UPPER(NEW.numero);
                END IF;
                IF NEW.tipo_obra IS NOT NULL THEN
                    NEW.tipo_obra := UPPER(NEW.tipo_obra);
                END IF;
            ELSIF TG_TABLE_NAME = 'revisiones' THEN
                IF NEW.nombre IS NOT NULL THEN
                    NEW.nombre := UPPER(NEW.nombre);
                END IF;
                IF NEW.folio IS NOT NULL THEN
                    NEW.folio := UPPER(NEW.folio);
                END IF;
            END IF;
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;

        DROP TRIGGER IF EXISTS trg_clean_empleados ON public.empleados;
        CREATE TRIGGER trg_clean_empleados
        BEFORE INSERT OR UPDATE ON public.empleados
        FOR EACH ROW EXECUTE FUNCTION public.clean_and_uppercase_entity_fields();

        DROP TRIGGER IF EXISTS trg_clean_clientes ON public.clientes;
        CREATE TRIGGER trg_clean_clientes
        BEFORE INSERT OR UPDATE ON public.clientes
        FOR EACH ROW EXECUTE FUNCTION public.clean_and_uppercase_entity_fields();

        DROP TRIGGER IF EXISTS trg_clean_rp ON public.registros_patronales;
        CREATE TRIGGER trg_clean_rp
        BEFORE INSERT OR UPDATE ON public.registros_patronales
        FOR EACH ROW EXECUTE FUNCTION public.clean_and_uppercase_entity_fields();

        DROP TRIGGER IF EXISTS trg_clean_obras ON public.registros_obras;
        CREATE TRIGGER trg_clean_obras
        BEFORE INSERT OR UPDATE ON public.registros_obras
        FOR EACH ROW EXECUTE FUNCTION public.clean_and_uppercase_entity_fields();

        DROP TRIGGER IF EXISTS trg_clean_rev ON public.revisiones;
        CREATE TRIGGER trg_clean_rev
        BEFORE INSERT OR UPDATE ON public.revisiones
        FOR EACH ROW EXECUTE FUNCTION public.clean_and_uppercase_entity_fields();

        -- Limpieza retroactiva de datos existentes
        UPDATE public.empleados SET
            nss = UPPER(REGEXP_REPLACE(nss, '[^A-Za-z0-9]', '', 'g')),
            rfc = UPPER(REGEXP_REPLACE(rfc, '[^A-Za-z0-9&Ññ]', '', 'g')),
            curp = UPPER(REGEXP_REPLACE(curp, '[^A-Za-z0-9]', '', 'g')),
            nombre = UPPER(nombre)
        WHERE (nss IS NOT NULL AND nss ~ '[^A-Za-z0-9]')
           OR (rfc IS NOT NULL AND rfc ~ '[^A-Za-z0-9&Ññ]')
           OR (curp IS NOT NULL AND curp ~ '[^A-Za-z0-9]')
           OR (nombre IS NOT NULL AND nombre != UPPER(nombre));
        "#,
    )?;

    // 7. Backfill oficial del SAT para empleados con RFC nulo, vacío o inválido
    backfill_empleados_rfc_sat(client)?;

    // 8. Backfill oficial de RENAPO para empleados con CURP nula, vacía o inválida
    backfill_empleados_curp_renapo(client)?;

    // 9. Eliminar tabla public.conexiones y su historial si existían previamente
    let _ = client.batch_execute(
        r#"
        DROP TABLE IF EXISTS public.conexiones CASCADE;
        DROP TABLE IF EXISTS revision_history.conexiones CASCADE;
        "#,
    );

    // 10. La limpieza/reasignación de duplicados de reportes (cleanup_all_duplicate_reportes_detalles)
    //     ya NO se ejecuta al arrancar: modificaba asignaciones de obra hechas por el analista.
    //     Solo corre desde la acción manual "Reparar desde RustFS" (Vista 6).

    // 11. Fase 1 de integridad: tablas/columnas nuevas (idempotente)
    exec_schema_step(client, "4_fase1_integridad_ddl", SQL_FASE1_DDL)?;

    // 12. Restricciones UNIQUE/CHECK de integridad (solo si no hay datos que las violen)
    exec_schema_step(client, "4b_fase1_restricciones_guardadas", SQL_FASE1_RESTRICCIONES)?;

    // 13. Migraciones de datos de una sola vez (registradas en public.migraciones_aplicadas)
    exec_schema_step(client, "5_migraciones_unicas", SQL_MIGRACIONES_UNICAS)?;

    // 14. Correcciones F0: estado/editado/sintética en reportes_nominas (no destructivo)
    exec_schema_step(client, "6_correcciones_f0_nominas", SQL_CORRECCIONES_F0_NOMINAS)?;

    // 15. Correcciones F1: borrado lógico de obras (activa), CHECK de fechas y referencia en bitácora
    exec_schema_step(client, "7_correcciones_f1_obras", SQL_CORRECCIONES_F1_OBRAS)?;

    match restricciones_fase1_pendientes(client) {
        Ok(lista) if !lista.is_empty() => log::warn!(
            "Restricciones de integridad pendientes (hay duplicados por depurar): {}",
            lista.join(", ")
        ),
        Ok(_) => {}
        Err(e) => log::warn!(
            "No se pudo verificar restricciones de integridad pendientes: {}",
            format_pg_error(&e)
        ),
    }

    log::info!("Esquema de base de datos verificado y listo.");
    Ok(())
}

/// Genera y actualiza el RFC oficial del SAT para todos los empleados que lo tengan vacío o incompleto.
/// Trata de generarlo con los datos que se tienen; en caso de no poder, asigna el RFC genérico ("XAXX010101000") aceptado por el SUA.
pub fn backfill_empleados_rfc_sat(client: &mut Client) -> Result<usize, postgres::Error> {
    let emp_rows = client.query(
        "SELECT id, nombre, curp, nss, rfc FROM public.empleados WHERE (rfc IS NULL OR TRIM(rfc) = '' OR TRIM(rfc) = 'XAXX010101000' OR LENGTH(TRIM(rfc)) < 13)",
        &[],
    )?;

    if emp_rows.is_empty() {
        return Ok(0);
    }

    let mut tx = client.transaction()?;
    let update_stmt = tx.prepare("UPDATE public.empleados SET rfc = $1 WHERE id = $2")?;
    let fallback_stmt = tx.prepare(
        "UPDATE public.empleados SET rfc = 'XAXX010101000' WHERE id = $1 AND (rfc IS NULL OR TRIM(rfc) = '' OR LENGTH(TRIM(rfc)) < 13)",
    )?;

    let mut count = 0;
    for r in emp_rows {
        let id: uuid::Uuid = r.get(0);
        let nom: Option<String> = r.get(1);
        let curp: Option<String> = r.get(2);
        let nss: Option<String> = r.get(3);
        let rfc_actual: Option<String> = r.get(4);

        if let Some(ref n) = nom {
            let clean = n.trim();
            if !clean.is_empty() && !clean.eq_ignore_ascii_case("NONE") && clean.chars().any(|c| c.is_alphabetic()) {
                let nuevo_rfc = crate::cedulas::rfc_sat::calcular_rfc_sat(
                    clean,
                    curp.as_deref(),
                    nss.as_deref(),
                );
                let target_rfc = if nuevo_rfc.len() == 13 && nuevo_rfc != "0000000000000" {
                    nuevo_rfc
                } else {
                    "XAXX010101000".to_string()
                };
                if rfc_actual.as_deref() != Some(&target_rfc) {
                    let _ = tx.execute(&update_stmt, &[&target_rfc, &id])?;
                    count += 1;
                }
                continue;
            }
        }
        // En caso de no poder, asegurar que aparezca el RFC genérico aceptado por el SUA
        let _ = tx.execute(&fallback_stmt, &[&id])?;
    }
    tx.commit()?;
    if count > 0 {
        log::info!("Se procesaron {} RFCs para empleados.", count);
    }
    Ok(count)
}

/// Genera y actualiza la CURP oficial de RENAPO para todos los empleados que la tengan vacía o incompleta,
/// sólo si se cuenta con nombre y datos suficientes para determinarla. Si no se puede determinar, la deja en blanco (NULL).
pub fn backfill_empleados_curp_renapo(client: &mut Client) -> Result<usize, postgres::Error> {
    let emp_rows = client.query(
        "SELECT id, nombre, rfc, nss FROM public.empleados WHERE (curp IS NULL OR TRIM(curp) = '' OR TRIM(curp) = 'XAXX010101000' OR LENGTH(TRIM(curp)) < 18)",
        &[],
    )?;

    if emp_rows.is_empty() {
        return Ok(0);
    }

    let mut tx = client.transaction()?;
    let update_stmt = tx.prepare("UPDATE public.empleados SET curp = $1 WHERE id = $2")?;
    let fallback_stmt = tx.prepare(
        "UPDATE public.empleados SET curp = NULL WHERE id = $1 AND (TRIM(curp) = 'XAXX010101000' OR LENGTH(TRIM(curp)) < 18)",
    )?;

    let mut count = 0;
    for r in emp_rows {
        let id: uuid::Uuid = r.get(0);
        let nom: Option<String> = r.get(1);
        let rfc: Option<String> = r.get(2);
        let nss: Option<String> = r.get(3);

        if let Some(ref n) = nom {
            let clean = n.trim();
            if !clean.is_empty() && !clean.eq_ignore_ascii_case("NONE") && clean.chars().any(|c| c.is_alphabetic()) {
                let nueva_curp = crate::cedulas::curp_renapo::calcular_curp_oficial(
                    clean,
                    rfc.as_deref(),
                    nss.as_deref(),
                    None,
                    None,
                );
                if !nueva_curp.is_empty() && crate::cedulas::curp_renapo::validar_curp_formato(&nueva_curp) {
                    let _ = tx.execute(&update_stmt, &[&nueva_curp, &id])?;
                    count += 1;
                    continue;
                }
            }
        }
        // Si no se puede determinar con los datos del empleado, dejar en blanco (NULL)
        let _ = tx.execute(&fallback_stmt, &[&id])?;
    }
    tx.commit()?;
    if count > 0 {
        log::info!("Se actualizaron {} CURPs oficiales de RENAPO para empleados.", count);
    }
    Ok(count)
}

/// Devuelve los nombres de las restricciones de integridad de la Fase 1 que aún no existen
/// (ni como restricción ni como índice). Se usa para avisar en log, sin abortar el arranque.
/// Genérica sobre `Client` o `Transaction` para poder verificarse dentro de la depuración.
pub fn restricciones_fase1_pendientes<C: postgres::GenericClient>(
    client: &mut C,
) -> Result<Vec<&'static str>, postgres::Error> {
    let mut pendientes = Vec::new();
    for nombre in RESTRICCIONES_FASE1 {
        let row = client.query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = $1::text) \
             OR EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname = 'public' AND indexname = $1::text)",
            &[&nombre],
        )?;
        let existe: bool = row.get(0);
        if !existe {
            pendientes.push(nombre);
        }
    }
    Ok(pendientes)
}

#[cfg(test)]
mod tests_fase1_ddl {
    use super::*;

    #[test]
    fn ddl_contiene_tablas_nuevas_y_no_es_destructivo() {
        assert!(SQL_FASE1_DDL.contains("CREATE TABLE IF NOT EXISTS public.archivos_cedulas"));
        assert!(SQL_FASE1_DDL.contains("public.respaldo_depuracion"));
        assert!(!SQL_FASE1_DDL.contains("RAISE EXCEPTION"));
        assert!(!SQL_FASE1_DDL.contains("DROP TABLE"));
    }

    #[test]
    fn restricciones_son_unicas_no_bloqueantes_y_no_destructivas() {
        assert!(SQL_FASE1_RESTRICCIONES.contains("NULLS NOT DISTINCT"));
        assert!(SQL_FASE1_RESTRICCIONES.contains("RAISE WARNING"));
        assert!(!SQL_FASE1_RESTRICCIONES.contains("RAISE EXCEPTION"));
        assert!(!SQL_FASE1_RESTRICCIONES.contains("DROP TABLE"));
    }

    #[test]
    fn correcciones_f0_nominas_es_aditiva_y_no_destructiva() {
        for fragmento in [
            "ADD COLUMN IF NOT EXISTS estado",
            "ADD COLUMN IF NOT EXISTS editado_usuario",
            "ADD COLUMN IF NOT EXISTS sintetica",
            "uq_reportes_nominas_rev_nomina",
            "RAISE WARNING",
            "f0_reportes_nominas_marcas",
        ] {
            assert!(
                SQL_CORRECCIONES_F0_NOMINAS.contains(fragmento),
                "falta {} en SQL_CORRECCIONES_F0_NOMINAS",
                fragmento
            );
        }
        for prohibido in ["RAISE EXCEPTION", "DROP TABLE", "DELETE FROM", "TRUNCATE"] {
            assert!(
                !SQL_CORRECCIONES_F0_NOMINAS.contains(prohibido),
                "SQL_CORRECCIONES_F0_NOMINAS no debe contener {}",
                prohibido
            );
        }
    }

    #[test]
    fn correcciones_f1_obras_es_aditiva_y_no_destructiva() {
        for fragmento in [
            "activa boolean NOT NULL DEFAULT true",
            "chk_registros_obras_fechas",
            "NOT VALID",
            "reportes_cambios ADD COLUMN IF NOT EXISTS id_registro_obra",
        ] {
            assert!(
                SQL_CORRECCIONES_F1_OBRAS.contains(fragmento),
                "falta {} en SQL_CORRECCIONES_F1_OBRAS",
                fragmento
            );
        }
        for prohibido in [
            "DELETE ",
            "DROP TABLE",
            "DROP COLUMN",
            "TRUNCATE",
            "UPDATE public.registros_obras",
        ] {
            assert!(
                !SQL_CORRECCIONES_F1_OBRAS.contains(prohibido),
                "SQL_CORRECCIONES_F1_OBRAS no debe contener {}",
                prohibido
            );
        }
    }

    #[test]
    fn nombres_de_restricciones_coinciden_con_el_sql() {
        for nombre in RESTRICCIONES_FASE1 {
            assert!(
                SQL_FASE1_RESTRICCIONES.contains(nombre),
                "falta {} en SQL_FASE1_RESTRICCIONES",
                nombre
            );
        }
    }
}
