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
        -- Column additions to accommodate all fields from cédulas and nóminas
        ALTER TABLE public.registros_patronales ADD COLUMN IF NOT EXISTS codigo_delegacion varchar(10);
        ALTER TABLE public.registros_patronales ADD COLUMN IF NOT EXISTS codigo_subdelegacion varchar(10);
        ALTER TABLE public.empleados ADD COLUMN IF NOT EXISTS clave_ubicacion varchar(50);
        ALTER TABLE public.empleados ADD COLUMN IF NOT EXISTS esta_activo boolean DEFAULT true;
        ALTER TABLE public.reportes_vinculaciones_obras_liquidaciones ALTER COLUMN id_registro_obra DROP NOT NULL;
        ALTER TABLE IF EXISTS revision_history.reportes_vinculaciones_obras_liquidaciones ALTER COLUMN id_registro_obra DROP NOT NULL;
        ALTER TABLE public.liquidaciones DROP COLUMN IF EXISTS dias;
        ALTER TABLE public.reportes_liquidaciones DROP COLUMN IF EXISTS dias;
        ALTER TABLE IF EXISTS revision_history.liquidaciones DROP COLUMN IF EXISTS dias;
        ALTER TABLE IF EXISTS revision_history.reportes_liquidaciones DROP COLUMN IF EXISTS dias;
        DELETE FROM public.reportes_vinculaciones_obras_liquidaciones WHERE id_registro_obra IS NULL;

        -- Add columns to revision_history if exists
        DO $$
        BEGIN
            IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'revision_history' AND table_name = 'registros_patronales') THEN
                ALTER TABLE revision_history.registros_patronales ADD COLUMN IF NOT EXISTS codigo_delegacion varchar(10);
                ALTER TABLE revision_history.registros_patronales ADD COLUMN IF NOT EXISTS codigo_subdelegacion varchar(10);
            END IF;
            IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'revision_history' AND table_name = 'empleados') THEN
                ALTER TABLE revision_history.empleados ADD COLUMN IF NOT EXISTS clave_ubicacion varchar(50);
                ALTER TABLE revision_history.empleados ADD COLUMN IF NOT EXISTS esta_activo boolean DEFAULT true;
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

        -- Update log_revision_history trigger function to dynamically match column names
        CREATE OR REPLACE FUNCTION public.log_revision_history()
        RETURNS TRIGGER AS $$
        DECLARE
            history_table_name TEXT := format('revision_history.%I', TG_TABLE_NAME);
            record_to_insert RECORD;
            cols TEXT;
            statement TEXT;
        BEGIN
            IF (TG_OP = 'INSERT' OR TG_OP = 'UPDATE') THEN
                record_to_insert := NEW;
            ELSEIF (TG_OP = 'DELETE') THEN
                record_to_insert := OLD;
            END IF;

            SELECT string_agg(quote_ident(column_name), ', ' ORDER BY ordinal_position)
            INTO cols
            FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = TG_TABLE_NAME;

            statement := format(
                'INSERT INTO %s (%s, modified_at, event_name) SELECT ($1).*, NOW(), $2',
                history_table_name,
                cols
            );

            EXECUTE statement USING record_to_insert, TG_OP;

            IF (TG_OP = 'DELETE') THEN
                RETURN OLD;
            ELSE
                RETURN NEW;
            END IF;
        END;
        $$ LANGUAGE plpgsql;

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
        "#,
    )?;

    log::info!("Esquema de base de datos verificado y listo.");
    Ok(())
}
