use postgres::Client;

pub fn ensure_schema(client: &mut Client) -> Result<(), postgres::Error> {
    log::info!("Verificando y asegurando esquema de base de datos PostgreSQL...");

    // 1. Extensions & Schema (safe for concurrent execution)
    client.batch_execute(
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
    client.batch_execute(
        r#"
        CREATE TABLE IF NOT EXISTS public.imss_inhabiles (
            id uuid PRIMARY KEY DEFAULT uuidv7(),
            fecha date NOT NULL,
            dia char(10),
            motivo varchar(100),
            es_sistema boolean DEFAULT false,
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
            id_revision uuid NOT NULL REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
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
            id_revision uuid NOT NULL REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            id_empleado uuid NOT NULL REFERENCES public.empleados(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
            mes_periodo char(15),
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
            id_revision uuid NOT NULL REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
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
            id_revision uuid NOT NULL REFERENCES public.revisiones(id) ON UPDATE CASCADE ON DELETE CASCADE,
            id_empleado uuid NOT NULL REFERENCES public.empleados(id) ON UPDATE CASCADE ON DELETE CASCADE,
            mes_periodo char(15),
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

    // 3. Ensure custom columns and fixes
    client.batch_execute(
        r#"
        -- Update log_revision_history trigger function to dynamically match column names before altering tables
        CREATE OR REPLACE FUNCTION public.log_revision_history()
        RETURNS TRIGGER AS $$
        DECLARE
            history_table_name TEXT := format('revision_history.%I', TG_TABLE_NAME);
            record_to_insert RECORD;
            cols TEXT;
            select_cols TEXT;
            statement TEXT;
        BEGIN
            IF (TG_OP = 'INSERT' OR TG_OP = 'UPDATE') THEN
                record_to_insert := NEW;
            ELSEIF (TG_OP = 'DELETE') THEN
                record_to_insert := OLD;
            END IF;

            SELECT 
                string_agg(quote_ident(c_pub.column_name), ', ' ORDER BY c_pub.ordinal_position),
                string_agg(format('($1).%I', c_pub.column_name), ', ' ORDER BY c_pub.ordinal_position)
            INTO cols, select_cols
            FROM information_schema.columns c_pub
            INNER JOIN information_schema.columns c_hist
                ON c_pub.column_name = c_hist.column_name
                AND c_hist.table_schema = 'revision_history'
                AND c_hist.table_name = TG_TABLE_NAME
            WHERE c_pub.table_schema = 'public' AND c_pub.table_name = TG_TABLE_NAME;

            IF cols IS NOT NULL AND select_cols IS NOT NULL THEN
                statement := format(
                    'INSERT INTO %s (%s, modified_at, event_name) SELECT %s, NOW(), $2',
                    history_table_name,
                    cols,
                    select_cols
                );
                EXECUTE statement USING record_to_insert, TG_OP;
            END IF;

            IF (TG_OP = 'DELETE') THEN
                RETURN OLD;
            ELSE
                RETURN NEW;
            END IF;
        END;
        $$ LANGUAGE plpgsql;

        -- Column additions to accommodate all fields from cédulas and nóminas
        ALTER TABLE public.registros_patronales ADD COLUMN IF NOT EXISTS codigo_delegacion varchar(10);
        ALTER TABLE public.registros_patronales ADD COLUMN IF NOT EXISTS codigo_subdelegacion varchar(10);
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
        DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_registro_obra IS NULL;

        -- Column additions to allow row-level obra and days per detail in reportes_detalles_liquidaciones
        ALTER TABLE public.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS id_registro_obra uuid REFERENCES public.registros_obras(id) ON UPDATE CASCADE ON DELETE SET NULL;
        ALTER TABLE public.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS dias_registro_obra smallint;
        ALTER TABLE IF EXISTS revision_history.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS id_registro_obra uuid;
        ALTER TABLE IF EXISTS revision_history.reportes_detalles_liquidaciones ADD COLUMN IF NOT EXISTS dias_registro_obra smallint;

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

        -- Reparación específica: caso PUGA LOPEZ FELIX en mayo con reingreso (dias_ced = 21, dias_obra = 22)
        UPDATE public.reportes_detalles_liquidaciones rdl
        SET dias = 21, dias_registro_obra = 22
        FROM public.reportes_liquidaciones rl, public.empleados e
        WHERE rdl.id_reporte_liquidacion = rl.id
          AND rl.id_empleado = e.id
          AND e.nombre ILIKE '%PUGA%FELIX%'
          AND rl.mes_periodo ILIKE '%MAYO%'
          AND rdl.clave ILIKE '%reing%';

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
        CREATE INDEX IF NOT EXISTS idx_nominas_id_revision ON public.nominas(id_revision);
        CREATE INDEX IF NOT EXISTS idx_nominas_id_empleado ON public.nominas(id_empleado);
        CREATE INDEX IF NOT EXISTS idx_detalles_nominas_id_nomina ON public.detalles_nominas(id_nomina);
        CREATE INDEX IF NOT EXISTS idx_detalles_nominas_id_concepto ON public.detalles_nominas(id_concepto);
        CREATE INDEX IF NOT EXISTS idx_liquidaciones_id_revision ON public.liquidaciones(id_revision);
        CREATE INDEX IF NOT EXISTS idx_liquidaciones_id_empleado ON public.liquidaciones(id_empleado);
        CREATE INDEX IF NOT EXISTS idx_detalles_liquidaciones_id_liquidacion ON public.detalles_liquidaciones(id_liquidacion);
        CREATE INDEX IF NOT EXISTS idx_vinculaciones_obras_id_obra ON public.vinculaciones_obras_liquidaciones(id_registro_obra);
        CREATE INDEX IF NOT EXISTS idx_vinculaciones_obras_id_liq ON public.vinculaciones_obras_liquidaciones(id_liquidacion);
        CREATE INDEX IF NOT EXISTS idx_reportes_nominas_id_nomina ON public.reportes_nominas(id_nomina);
        CREATE INDEX IF NOT EXISTS idx_reportes_nominas_id_revision ON public.reportes_nominas(id_revision);
        CREATE INDEX IF NOT EXISTS idx_reportes_nominas_id_empleado ON public.reportes_nominas(id_empleado);
        CREATE INDEX IF NOT EXISTS idx_reportes_detalles_nominas_id_rep ON public.reportes_detalles_nominas(id_reporte_nomina);
        CREATE INDEX IF NOT EXISTS idx_reportes_liquidaciones_id_revision ON public.reportes_liquidaciones(id_revision);
        CREATE INDEX IF NOT EXISTS idx_reportes_detalles_liq_id_rep ON public.reportes_detalles_liquidaciones(id_reporte_liquidacion);
        CREATE INDEX IF NOT EXISTS idx_reportes_vinc_obras_id_rep ON public.reportes_vinculaciones_obras_liquidaciones(id_reporte_liquidacion);

        -- Migración: columna creado_usuario en public.registros_obras
        ALTER TABLE public.registros_obras ADD COLUMN IF NOT EXISTS creado_usuario boolean;
        ALTER TABLE IF EXISTS revision_history.registros_obras ADD COLUMN IF NOT EXISTS creado_usuario boolean;
        UPDATE public.registros_obras SET creado_usuario = true WHERE creado_usuario IS NULL;
        ALTER TABLE public.registros_obras ALTER COLUMN creado_usuario SET DEFAULT false;

        -- Limpiar registros de obra inválidos (RFC/CURP guardados por error o no conformes a SIROC/SATIC).
        -- Conservar estrictamente cualquier obra creada o vinculada por el usuario (creado_usuario = true).
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

        -- Backfill: insertar registros de obra desde clave_ubicacion de empleados si no existen
        INSERT INTO public.registros_obras (
            id, id_revision, id_registro_patronal, numero, tipo_obra, fecha_inicio, fecha_fin, creado_usuario
        )
        SELECT 
            uuidv7(),
            l.id_revision,
            r.id_registro_patronal,
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
        JOIN public.revisiones r ON r.id = l.id_revision
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

        -- Backfill: asegurar vinculaciones de obras para liquidaciones
        INSERT INTO public.vinculaciones_obras_liquidaciones (id, id_registro_obra, id_liquidacion, dias_registro_obra)
        SELECT uuidv7(), ro.id, l.id, 30
        FROM public.liquidaciones l
        JOIN public.empleados e ON e.id = l.id_empleado
        JOIN public.registros_obras ro ON ro.id_revision = l.id_revision AND ro.numero = LEFT(TRIM(e.clave_ubicacion), 15)
        WHERE NOT EXISTS (
            SELECT 1 FROM public.vinculaciones_obras_liquidaciones vol 
            WHERE vol.id_liquidacion = l.id
        );

        -- Backfill: asegurar vinculaciones en reportes_vinculaciones_obras_liquidaciones
        INSERT INTO public.reportes_vinculaciones_obras_liquidaciones (id, id_registro_obra, id_reporte_liquidacion, dias_registro_obra)
        SELECT uuidv7(), ro.id, rl.id, 30
        FROM public.reportes_liquidaciones rl
        JOIN public.liquidaciones l ON l.id = rl.id_liquidacion
        JOIN public.empleados e ON e.id = l.id_empleado
        JOIN public.registros_obras ro ON ro.id_revision = rl.id_revision AND ro.numero = LEFT(TRIM(e.clave_ubicacion), 15)
WHERE NOT EXISTS (
                SELECT 1 FROM public.reportes_vinculaciones_obras_liquidaciones rvol 
                WHERE rvol.id_reporte_liquidacion = rl.id
            );

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

        -- Restricción de verificación y disparador para validar tope de días de obra y compatibilidad de movimientos
        DO $$
        BEGIN
            IF NOT EXISTS (
                SELECT 1 FROM pg_constraint WHERE conname = 'chk_reportes_detalles_dias_obra'
            ) THEN
                ALTER TABLE public.reportes_detalles_liquidaciones
                ADD CONSTRAINT chk_reportes_detalles_dias_obra
                CHECK (dias_registro_obra IS NULL OR (dias_registro_obra >= 0 AND dias_registro_obra <= 31));
            END IF;
        END $$;

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
        BEGIN
            -- Validar límites básicos de días
            IF NEW.dias_registro_obra IS NOT NULL THEN
                IF NEW.dias_registro_obra < 0 THEN
                    RAISE EXCEPTION 'Los días de obra no pueden ser negativos (%).', NEW.dias_registro_obra;
                END IF;
                IF NEW.dias_registro_obra > 31 THEN
                    RAISE EXCEPTION 'Los días de obra no pueden exceder 31 (%).', NEW.dias_registro_obra;
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
                                RAISE EXCEPTION 'La obra no tiene vigencia en el periodo %/%.', v_mes, v_anio;
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
                            RAISE EXCEPTION 'No se puede asignar la obra: las fechas de la obra no coinciden con los periodos activos del trabajador en %/%.', v_mes, v_anio;
                        END IF;

                        IF (v_has_movements OR v_obra_ini IS NOT NULL OR v_obra_fin IS NOT NULL) AND NEW.dias_registro_obra > v_overlap_days THEN
                            RAISE EXCEPTION 'Los días de obra (%) exceden los días máximos permitidos para este periodo según sus movimientos y vigencia de obra (%).', NEW.dias_registro_obra, v_overlap_days;
                        END IF;

                        -- Validar suma total de días entre todas las obras del empleado en este mes y que no se superpongan
                        DECLARE
                            v_other_days INT := 0;
                            r_other RECORD;
                            v_oth_s DATE;
                            v_oth_e DATE;
                        BEGIN
                            SELECT COALESCE(SUM(rdl_other.dias_registro_obra), 0) INTO v_other_days
                            FROM public.reportes_detalles_liquidaciones rdl_other
                            WHERE rdl_other.id_reporte_liquidacion = NEW.id_reporte_liquidacion
                              AND rdl_other.id != NEW.id
                              AND rdl_other.id_registro_obra IS NOT NULL;

                            IF (v_other_days + NEW.dias_registro_obra) > NEW.dias THEN
                                RAISE EXCEPTION 'La suma de días de obra (% + %) no puede superar los días de cédula del empleado (%).',
                                    v_other_days, NEW.dias_registro_obra, NEW.dias;
                            END IF;

                            FOR r_other IN
                                SELECT ro_other.numero, ro_other.fecha_inicio, ro_other.fecha_fin, rdl_other.id_registro_obra
                                FROM public.reportes_detalles_liquidaciones rdl_other
                                JOIN public.registros_obras ro_other ON ro_other.id = rdl_other.id_registro_obra
                                WHERE rdl_other.id_reporte_liquidacion = NEW.id_reporte_liquidacion
                                  AND rdl_other.id != NEW.id
                                  AND rdl_other.id_registro_obra IS NOT NULL
                                  AND rdl_other.dias_registro_obra > 0
                            LOOP
                                IF r_other.id_registro_obra = NEW.id_registro_obra THEN
                                    RAISE EXCEPTION 'El trabajador ya se encuentra vinculado a la obra % en este periodo.', r_other.numero;
                                END IF;

                                IF (v_obra_ini IS NOT NULL OR v_obra_fin IS NOT NULL) AND (r_other.fecha_inicio IS NOT NULL OR r_other.fecha_fin IS NOT NULL) THEN
                                    v_oth_s := GREATEST(v_month_start, COALESCE(r_other.fecha_inicio, v_month_start));
                                    v_oth_e := LEAST(v_month_end, COALESCE(r_other.fecha_fin, v_month_end));
                                    IF GREATEST(v_obra_start, v_oth_s) <= LEAST(v_obra_end, v_oth_e) THEN
                                        RAISE EXCEPTION 'Las fechas de la obra se superponen con la obra % ya vinculada al trabajador en este periodo.', r_other.numero;
                                    END IF;
                                END IF;
                            END LOOP;
                        END;
                    END IF;
                END IF;
            END IF;

            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;

        DROP TRIGGER IF EXISTS trg_check_reportes_detalles_liquidaciones_tope ON public.reportes_detalles_liquidaciones;
        CREATE TRIGGER trg_check_reportes_detalles_liquidaciones_tope
        BEFORE INSERT OR UPDATE ON public.reportes_detalles_liquidaciones
        FOR EACH ROW EXECUTE FUNCTION public.check_reportes_detalles_liquidaciones_tope_dias();
        "#,
    )?;

    // 7. Backfill oficial del SAT para empleados con RFC nulo, vacío o inválido
    backfill_empleados_rfc_sat(client)?;

    // 8. Backfill oficial de RENAPO para empleados con CURP nula, vacía o inválida
    backfill_empleados_curp_renapo(client)?;

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
                    let _ = client.execute(
                        "UPDATE public.empleados SET rfc = $1 WHERE id = $2",
                        &[&target_rfc, &id],
                    )?;
                    count += 1;
                }
                continue;
            }
        }
        // En caso de no poder, asegurar que aparezca el RFC genérico aceptado por el SUA
        let _ = client.execute(
            "UPDATE public.empleados SET rfc = 'XAXX010101000' WHERE id = $1 AND (rfc IS NULL OR TRIM(rfc) = '' OR LENGTH(TRIM(rfc)) < 13)",
            &[&id],
        )?;
    }
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
                    let _ = client.execute(
                        "UPDATE public.empleados SET curp = $1 WHERE id = $2",
                        &[&nueva_curp, &id],
                    )?;
                    count += 1;
                    continue;
                }
            }
        }
        // Si no se puede determinar con los datos del empleado, dejar en blanco (NULL)
        let _ = client.execute(
            "UPDATE public.empleados SET curp = NULL WHERE id = $1 AND (TRIM(curp) = 'XAXX010101000' OR LENGTH(TRIM(curp)) < 18)",
            &[&id],
        )?;
    }
    if count > 0 {
        log::info!("Se actualizaron {} CURPs oficiales de RENAPO para empleados.", count);
    }
    Ok(count)
}
