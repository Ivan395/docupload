pub mod calendario;
pub mod catalogos;
pub mod cedulas;
pub mod client;
pub mod nominas;
pub mod nominas_reporte;
pub mod reporte_liquidaciones;
pub mod revisiones;
pub mod schema;

pub use calendario::*;
pub use catalogos::*;
pub use cedulas::{save_cedulas_concurrent, save_cedulas_to_db, CedulasDbStats};
pub use client::{create_client, create_pool, DbPool, PooledClient};
pub use nominas::{save_nominas_concurrent, save_nominas_to_db, delete_all_nominas, NominasDbStats};
pub use nominas_reporte::*;
pub use reporte_liquidaciones::*;
pub use revisiones::*;
pub use schema::ensure_schema;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use uuid::Uuid;
    use crate::nominas::models::*;
    use crate::cedulas::business::Business;
    use crate::cedulas::employees::{DataEmployee, Employee as CedulaEmployee, RegistroObra};

    #[test]
    fn test_db_ensure_schema_and_save_data() {
        let mut client = match create_client() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("PostgreSQL no disponible en test (skipping): {}", e);
                return;
            }
        };

        // 1. Test ensure_schema
        let schema_res = ensure_schema(&mut client);
        assert!(schema_res.is_ok(), "ensure_schema failed: {:?}", schema_res.err());

        let rows = client.query(
            r#"
            SELECT 
                e.nombre, e.rfc, e.curp, e.clave_ubicacion,
                l.id AS id_liq,
                vol.dias_registro_obra AS vol_dias,
                ro_vol.numero AS vol_obra,
                rl.id AS id_rep_liq,
                rvol.dias_registro_obra AS rvol_dias,
                ro_rvol.numero AS rvol_obra
            FROM public.empleados e
            LEFT JOIN public.liquidaciones l ON l.id_empleado = e.id
            LEFT JOIN public.vinculaciones_obras_liquidaciones vol ON vol.id_liquidacion = l.id
            LEFT JOIN public.registros_obras ro_vol ON ro_vol.id = vol.id_registro_obra
            LEFT JOIN public.reportes_liquidaciones rl ON rl.id_liquidacion = l.id
            LEFT JOIN public.reportes_vinculaciones_obras_liquidaciones rvol ON rvol.id_reporte_liquidacion = rl.id
            LEFT JOIN public.registros_obras ro_rvol ON ro_rvol.id = rvol.id_registro_obra
            WHERE e.nombre ~* 'CRUZ LUNA' OR e.nombre ~* 'JIMENEZ'
            ORDER BY e.nombre;
            "#,
            &[],
        ).unwrap();
        for r in rows {
            let nom: String = r.get(0);
            let rfc: Option<String> = r.get(1);
            let curp: Option<String> = r.get(2);
            let ubic: Option<String> = r.get(3);
            let vol_dias: Option<i16> = r.get(5);
            let vol_obra: Option<String> = r.get(6);
            let rvol_dias: Option<i16> = r.get(8);
            let rvol_obra: Option<String> = r.get(9);
            println!("DEBUG_EMP: nom='{}', rfc='{:?}', curp='{:?}', ubic='{:?}', vol_obra='{:?}', vol_dias='{:?}', rvol_obra='{:?}', rvol_dias='{:?}'",
                nom, rfc, curp, ubic, vol_obra, vol_dias, rvol_obra, rvol_dias);
        }

        // 2. Test save_nominas_to_db with sample employee
        let mut emp = Employee {
            nombrearchivo: "TEST_NOMINA_01.xml".to_string(),
            nombre: Some("JUAN PEREZ GONZALEZ".to_string()),
            numseguridadsocial: Some("12345678901".to_string()),
            rfc_receptor: Some("PEGI800101XYZ".to_string()),
            curp_receptor: Some("PEGI800101HDFRRN01".to_string()),
            rfc_emisor: Some("XAXX010101000".to_string()),
            nombre_emisor: Some("EMPRESA CONSTRUCTORA DEL NORTE SA DE CV".to_string()),
            registro_patronal: Some("Y5512345101".to_string()),
            uuid: Some(Uuid::now_v7().to_string()),
            total: Some(7500.0),
            fechainiciorellaboral: NaiveDate::from_ymd_opt(2022, 1, 15),
            salariobasecotapor: Some(500.0),
            salariodiariointegrado: Some(525.0),
            salariodiariocalculado: Some(480.0),
            salariodiariointegradocalculado: Some(510.0),
            ..Default::default()
        };
        emp.nomina.version_nomina = Some("1.2".to_string());
        emp.nomina.tipo_nomina = Some("O".to_string());
        emp.nomina.fechapago = NaiveDate::from_ymd_opt(2024, 5, 15);
        emp.nomina.fechainicialpago = NaiveDate::from_ymd_opt(2024, 5, 1);
        emp.nomina.fechafinalpago = NaiveDate::from_ymd_opt(2024, 5, 15);
        emp.nomina.numdiaspagados = Some(15.0);
        emp.nomina.total_sueldos = Some(7200.0);
        emp.nomina.total_gravado = Some(7200.0);
        emp.nomina.total_exento = Some(300.0);
        emp.nomina.periodicidad_pago = Some("04".to_string());

        emp.detalles_nomina.push(DetalleNominaItem {
            tipo_concepto_categoria: "PERCEPCIONES".to_string(),
            tipo_sat: "001".to_string(),
            clave: "001".to_string(),
            concepto: "SUELDO BASE QUINCENAL".to_string(),
            importe_gravado: Some(7200.0),
            importe_exento: Some(0.0),
            importe: 7200.0,
        });
        emp.detalles_nomina.push(DetalleNominaItem {
            tipo_concepto_categoria: "DEDUCCIONES".to_string(),
            tipo_sat: "002".to_string(),
            clave: "050".to_string(),
            concepto: "I.S.R.".to_string(),
            importe_gravado: None,
            importe_exento: None,
            importe: 650.0,
        });

        emp.cfdi.conceptos.push(ConceptoCfdiItem {
            clave_prod_serv: Some("84111505".to_string()),
            clave_unidad: Some("ACT".to_string()),
            descripcion: Some("Pago de nómina".to_string()),
            cantidad: Some(1),
            valor_unitario: Some(7500.0),
            importe: Some(7500.0),
            descuento: None,
            objeto_imp: Some("01".to_string()),
        });

        // Crear cliente, registro_patronal y revisión de prueba para la nómina
        let revision_id = {
            let mut tx = client.transaction().expect("Failed to start transaction");
            
            let cliente_id = Uuid::now_v7();
            let cliente_nombre = format!("CLIENTE TEST NOMINA {}", Uuid::now_v7());
            tx.execute(
                "INSERT INTO public.clientes (id, nombre) VALUES ($1, $2)
                 RETURNING id",
                &[&cliente_id, &cliente_nombre],
            ).expect("Failed to create test cliente");
            
            let rp_id = Uuid::now_v7();
            tx.execute(
                "INSERT INTO public.registros_patronales (id, id_cliente, registro)
                 VALUES ($1, $2, 'Y5512345101')
                 RETURNING id",
                &[&rp_id, &cliente_id],
            ).expect("Failed to create test registro_patronal");
            
            let rev_id = Uuid::now_v7();
            tx.execute(
                "INSERT INTO public.revisiones (id, id_registro_patronal, nombre, folio, fecha_entrega_revision)
                 VALUES ($1, $2, 'REV TEST NOMINA', 'REV-TEST-NOM', CURRENT_DATE)
                 RETURNING id",
                &[&rev_id, &rp_id],
            ).expect("Failed to create test revision");
            
            tx.commit().expect("Failed to commit test data");
            rev_id
        };

        let nom_stats = save_nominas_to_db(&mut client, &[emp], revision_id);
        assert!(nom_stats.is_ok(), "save_nominas_to_db failed: {:?}", nom_stats.err());
        let nom_s = nom_stats.unwrap();
        eprintln!("nom_s = {:?}", nom_s);
        assert!(nom_s.errores.is_empty(), "Errors saving nominas: {:?}", nom_s.errores);
        assert_eq!(nom_s.nominas_guardadas, 1);
        assert_eq!(nom_s.cfdis_guardados, 1);
        assert!(nom_s.detalles_guardados >= 2, "Detalles guardados: {}", nom_s.detalles_guardados);

        // 3. Test save_cedulas_to_db with sample business and employee
        let mut biz = Business::new();
        biz.razon_social = Some("EMPRESA CONSTRUCTORA DEL NORTE SA DE CV".to_string());
        biz.rfc = Some("XAXX010101000".to_string());
        let unique_rp = format!("Y55{:07}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() % 10_000_000);
        biz.registro_patronal = Some(unique_rp);
        biz.actividad = Some("CONSTRUCCION DE OBRAS DE EDIFICACION".to_string());
        biz.domicilio = Some("AV INSURGENTES SUR 123".to_string());
        biz.codigo_postal = Some("03900".to_string());
        biz.entidad = Some("CIUDAD DE MEXICO".to_string());
        biz.area_geografica = Some("A".to_string());
        biz.delegacion = Some("09 CDMX SUR".to_string());
        biz.subdelegacion = Some("02 BENITO JUAREZ".to_string());
        biz.prima = Some("7.58875".to_string());

        let mut c_emp = CedulaEmployee::new();
        c_emp.nombre = Some("JUAN PEREZ GONZALEZ".to_string());
        c_emp.no_seguro_social = Some("12345678901".to_string());
        c_emp.rfc = Some("PEGI800101XYZ".to_string());
        c_emp.curp = Some("PEGI800101HDFRRN01".to_string());
        c_emp.clave_ubicacion = Some("01".to_string());
        c_emp.mes_periodo = Some("MAYO".to_string());
        c_emp.anio_periodo = Some("2024".to_string());

        c_emp.registro_obras.push(RegistroObra {
            registro_obra: "C1234567".to_string(),
            dias_registro_obra: 15,
        });

        c_emp.data_employee.push(DataEmployee {
            mes_periodo: Some("MAYO".to_string()),
            anio_periodo: Some("2024".to_string()),
            clave: Some("ALTA".to_string()),
            fecha: NaiveDate::from_ymd_opt(2024, 5, 1),
            dias: Some(15.0),
            sdi: Some(525.0),
            cf: Some(120.0),
            rt: Some(85.0),
            patronal: Some(450.0),
            obrera: Some(120.0),
            subtotal: Some(570.0),
            ..Default::default()
        });

        biz.employees.push(c_emp);

        let ced_stats = save_cedulas_to_db(&mut client, &[biz.clone()]);
        assert!(ced_stats.is_ok(), "save_cedulas_to_db failed: {:?}", ced_stats.err());
        let ced_s = ced_stats.unwrap();
        assert_eq!(ced_s.liquidaciones_guardadas, 1);
        assert_eq!(ced_s.detalles_guardados, 1);
        assert_eq!(ced_s.vinculaciones_guardadas, 1);
        assert_eq!(ced_s.registros_obras_guardados, 1);

        // Verificar que la obra guardada desde cédula tenga creado_usuario = false
        let obra_row = client.query_one(
            "SELECT creado_usuario FROM public.registros_obras WHERE numero = 'C1234567' LIMIT 1",
            &[],
        ).expect("Obra debe existir en public.registros_obras");
        let creado_usuario: bool = obra_row.get(0);
        assert!(!creado_usuario, "La obra creada desde cédula debe tener creado_usuario = false");

        // Prueba de no duplicación: reingresar los mismos datos no debe duplicar en Postgres
        let dup_stats = save_cedulas_to_db(&mut client, &[biz]);
        assert!(dup_stats.is_ok());
        let dup_s = dup_stats.unwrap();
        assert_eq!(dup_s.liquidaciones_guardadas, 0, "No debe duplicar liquidaciones existentes");
        assert_eq!(dup_s.detalles_guardados, 0, "No debe duplicar detalles existentes");
        assert_eq!(dup_s.vinculaciones_guardadas, 0, "No debe duplicar vinculaciones existentes");

        // Prueba: verificar que un RFC/CURP o registro de obra inválido NO se guarda en registros_obras
        let mut bad_biz = Business::new();
        bad_biz.registro_patronal = Some("Y5512345101".to_string());
        bad_biz.razon_social = Some("EMPRESA CONSTRUCTORA DEL NORTE SA DE CV".to_string());
        let mut bad_emp = CedulaEmployee::new();
        bad_emp.no_seguro_social = Some("98765432101".to_string());
        bad_emp.rfc = Some("JIFA981012A12".to_string());
        bad_emp.curp = Some("JIFA981012MCLMN04".to_string());
        bad_emp.clave_ubicacion = Some("JIFA981012MCLMN04".to_string()); // erróneamente CURP en clave_ubicacion
        bad_emp.mes_periodo = Some("JUNIO".to_string());
        bad_emp.anio_periodo = Some("2024".to_string());
        bad_emp.registro_obras.push(RegistroObra {
            registro_obra: "jifa981012mclmn".to_string(), // erróneamente CURP en registro_obras
            dias_registro_obra: 10,
        });
        bad_emp.data_employee.push(DataEmployee {
            mes_periodo: Some("JUNIO".to_string()),
            anio_periodo: Some("2024".to_string()),
            clave: Some("ALTA".to_string()),
            fecha: NaiveDate::from_ymd_opt(2024, 6, 1),
            dias: Some(15.0),
            sdi: Some(525.0),
            cf: Some(120.0),
            rt: Some(85.0),
            patronal: Some(450.0),
            obrera: Some(120.0),
            subtotal: Some(570.0),
            ..Default::default()
        });
        bad_biz.employees.push(bad_emp);

        let bad_ced_stats = save_cedulas_to_db(&mut client, &[bad_biz]);
        assert!(bad_ced_stats.is_ok());
        let b_stats = bad_ced_stats.unwrap();
        assert_eq!(b_stats.registros_obras_guardados, 0, "No debe guardar registros de obra inválidos");

        let bad_count: i64 = client.query_one(
            "SELECT COUNT(*) FROM public.registros_obras WHERE numero ~* 'jifa981012mclmn'",
            &[],
        ).unwrap().get(0);
        assert_eq!(bad_count, 0, "jifa981012mclmn nunca debe existir en registros_obras");
    }

#[test]
    fn test_concurrent_db_insertion() {
        let pool = match create_pool(4) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("PostgreSQL pool no disponible en test (skipping): {}", e);
                return;
            }
        };

        // Crear registro_patronal y revisión de prueba para las nóminas
        let (_rp_id, revision_id) = {
            let mut client = pool.get().expect("Failed to get connection from pool");
            let mut tx = client.transaction().expect("Failed to start transaction");
            
            // Crear cliente de prueba primero (nombre único para evitar conflictos)
            let cliente_id = Uuid::now_v7();
            let cliente_nombre = format!("CLIENTE TEST CONCURRENTE {}", Uuid::now_v7());
            tx.execute(
                "INSERT INTO public.clientes (id, nombre) VALUES ($1, $2)
                 RETURNING id",
                &[&cliente_id, &cliente_nombre],
            ).expect("Failed to create test cliente");
            
            let rp_id = Uuid::now_v7();
            tx.execute(
                "INSERT INTO public.registros_patronales (id, id_cliente, registro)
                 VALUES ($1, $2, 'Y5512345101')
                 RETURNING id",
                &[&rp_id, &cliente_id],
            ).expect("Failed to create test registro_patronal");
            
            let rev_id = Uuid::now_v7();
            tx.execute(
                "INSERT INTO public.revisiones (id, id_registro_patronal, nombre, folio, fecha_entrega_revision)
                 VALUES ($1, $2, 'REV TEST CONCURRENTE', 'REV-TEST-CONC', CURRENT_DATE)
                 RETURNING id",
                &[&rev_id, &rp_id],
            ).expect("Failed to create test revision");
            
            tx.commit().expect("Failed to commit test data");
            (rp_id, rev_id)
        };

        // Generate multiple employee nóminas
        let mut nominas = Vec::new();
        for i in 1..=10 {
            let mut emp = Employee {
                nombrearchivo: format!("CONCURRENT_NOM_{:02}.xml", i),
                nombre: Some(format!("TRABAJADOR CONCURRENTE {}", i)),
                numseguridadsocial: Some(format!("999999999{:02}", i)),
                rfc_receptor: Some(format!("CONC{:02}0101XYZ", i)),
                curp_receptor: Some(format!("CONC{:02}0101HDFRRN01", i)),
                rfc_emisor: Some("XAXX010101000".to_string()),
                nombre_emisor: Some("EMPRESA CONSTRUCTORA DEL NORTE SA DE CV".to_string()),
                registro_patronal: Some("Y5512345101".to_string()),
                uuid: Some(Uuid::now_v7().to_string()),
                total: Some(5000.0 + (i as f64) * 100.0),
                fechainiciorellaboral: NaiveDate::from_ymd_opt(2023, 1, 1),
                salariobasecotapor: Some(400.0),
                salariodiariointegrado: Some(420.0),
                salariodiariocalculado: Some(390.0),
                salariodiariointegradocalculado: Some(415.0),
                ..Default::default()
            };
            emp.nomina.version_nomina = Some("1.2".to_string());
            emp.nomina.tipo_nomina = Some("O".to_string());
            emp.nomina.fechapago = NaiveDate::from_ymd_opt(2024, 6, 15);
            emp.nomina.fechainicialpago = NaiveDate::from_ymd_opt(2024, 6, 1);
            emp.nomina.fechafinalpago = NaiveDate::from_ymd_opt(2024, 6, 15);
            emp.nomina.numdiaspagados = Some(15.0);
            emp.nomina.total_sueldos = Some(5000.0);
            emp.nomina.total_gravado = Some(5000.0);
            emp.nomina.total_exento = Some(0.0);
            emp.nomina.periodicidad_pago = Some("04".to_string());

            emp.detalles_nomina.push(DetalleNominaItem {
                tipo_concepto_categoria: "PERCEPCIONES".to_string(),
                tipo_sat: "001".to_string(),
                clave: "001".to_string(),
                concepto: "SUELDO BASE".to_string(),
                importe_gravado: Some(5000.0),
                importe_exento: Some(0.0),
                importe: 5000.0,
            });

            nominas.push(emp);
        }

        let cb = |_done, _total| {};
        let nom_stats = save_nominas_concurrent(&pool, &nominas, Some(&cb), revision_id);

        assert!(nom_stats.is_ok(), "save_nominas_concurrent failed: {:?}", nom_stats.err());
        let s = nom_stats.unwrap();
        assert_eq!(s.nominas_guardadas, 10);
        assert_eq!(s.cfdis_guardados, 10);
        assert_eq!(s.detalles_guardados, 10);
        assert!(s.errores.is_empty(), "Errors during concurrent nominas insert: {:?}", s.errores);

        // Generate multiple cédula employees
        let mut biz = Business::new();
        biz.razon_social = Some("EMPRESA CONSTRUCTORA DEL NORTE SA DE CV".to_string());
        biz.rfc = Some("XAXX010101000".to_string());
        let unique_rp = format!("Z99{:07}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() % 10_000_000);
        biz.registro_patronal = Some(unique_rp);
        biz.actividad = Some("CONSTRUCCION DE OBRAS DE EDIFICACION".to_string());
        biz.codigo_postal = Some("03900".to_string());
        biz.entidad = Some("CIUDAD DE MEXICO".to_string());
        biz.delegacion = Some("09 CDMX SUR".to_string());
        biz.subdelegacion = Some("02 BENITO JUAREZ".to_string());

        for i in 1..=10 {
            let mut c_emp = CedulaEmployee::new();
            c_emp.nombre = Some(format!("TRABAJADOR CONCURRENTE {}", i));
            c_emp.no_seguro_social = Some(format!("999999999{:02}", i));
            c_emp.rfc = Some(format!("CONC{:02}0101XYZ", i));
            c_emp.curp = Some(format!("CONC{:02}0101HDFRRN01", i));
            c_emp.clave_ubicacion = Some(format!("OBRA-CONCURRENTE-{:02}", i % 3));
            c_emp.mes_periodo = Some("JUNIO".to_string());
            c_emp.anio_periodo = Some("2024".to_string());

            c_emp.data_employee.push(DataEmployee {
                mes_periodo: Some("JUNIO".to_string()),
                anio_periodo: Some("2024".to_string()),
                clave: Some("ALTA".to_string()),
                fecha: NaiveDate::from_ymd_opt(2024, 6, 1),
                dias: Some(15.0),
                sdi: Some(420.0),
                cf: Some(100.0),
                rt: Some(70.0),
                patronal: Some(380.0),
                obrera: Some(100.0),
                subtotal: Some(480.0),
                ..Default::default()
            });

            c_emp.registro_obras.push(RegistroObra {
                registro_obra: format!("C123456{}", i % 3),
                dias_registro_obra: 15,
            });

            biz.employees.push(c_emp);
        }

        let ced_stats = save_cedulas_concurrent(&pool, &[biz.clone()], Some(&cb));
        assert!(ced_stats.is_ok(), "save_cedulas_concurrent failed: {:?}", ced_stats.err());
        let cs = ced_stats.unwrap();
        assert_eq!(cs.liquidaciones_guardadas, 10);
        assert_eq!(cs.detalles_guardados, 10);
        assert_eq!(cs.vinculaciones_guardadas, 10);
        assert!(cs.errores.is_empty(), "Errors during concurrent cedulas insert: {:?}", cs.errores);

        // Deduplication: re-ejecutar guardado concurrente con los mismos datos debe insertar 0 duplicados
        let dup_stats = save_cedulas_concurrent(&pool, &[biz], Some(&cb));
        assert!(dup_stats.is_ok());
        let d_cs = dup_stats.unwrap();
        assert_eq!(d_cs.liquidaciones_guardadas, 0, "No debe duplicar liquidaciones concurrentes");
        assert_eq!(d_cs.detalles_guardados, 0, "No debe duplicar detalles concurrentes");
        assert_eq!(d_cs.vinculaciones_guardadas, 0, "No debe duplicar vinculaciones concurrentes");
    }
}

