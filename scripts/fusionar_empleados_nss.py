"""Fusiona un empleado duplicado (NSS origen) en el empleado correcto (NSS destino) y registra
el NSS origen como equivalencia del destino, para que las cargas futuras no vuelvan a duplicarlo.

Uso (desde la raíz del proyecto; requiere el paquete `psycopg`):
    python -I scripts/fusionar_empleados_nss.py <nss_origen> <nss_destino> [--motivo "texto"] [--aplicar]
Sin --aplicar todo corre dentro de una transacción que se revierte (simulación).
La conexión se toma de db_config.json en la raíz del proyecto.

Por cada liquidación L_o del empleado origen:
  * Si el destino ya tiene liquidación L_d del mismo RP/año/mes:
      - exige que detalles y obras (folio y días) sean idénticos; si no, aborta sin cambios;
      - L_d adopta archivo, origen y vinculaciones de L_o (gana la carga más reciente);
      - filas de reporte de L_o: en revisiones donde L_d ya tiene fila se borran
        (cascada a rdl/rvol); en las demás se reasignan a L_d y al empleado destino;
      - se borra L_o (cascada a detalles).
  * Si no: L_o y sus filas de reporte pasan al empleado destino.
Al final borra al empleado origen si ya no tiene referencias y registra la equivalencia
(public.empleados_nss_equivalencias; requiere haber abierto la app con esta versión).
"""
import json, os, sys, psycopg

DET_COLS = ("clave, fecha, dias, sdi, lic, inc, aus, cf, exc_pat, exc_obr, pd_pat, pd_obr, "
            "gmp_pat, gmp_obr, rt, iv_pat, iv_obr, gps, patronal, obrera, subtotal")

def main():
    argv = sys.argv[1:]
    motivo = None
    if "--motivo" in argv:
        i = argv.index("--motivo")
        motivo = argv[i + 1] if i + 1 < len(argv) else None
        del argv[i:i + 2]
    args = [a for a in argv if not a.startswith("--")]
    aplicar = "--aplicar" in argv
    if len(args) != 2:
        sys.exit(__doc__)
    nss_o, nss_d = args
    raiz = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    c = json.load(open(os.path.join(raiz, "db_config.json")))
    conn = psycopg.connect(host=c["host"], port=c["port"], dbname=c["database"],
                           user=c["user"], password=c["password"])
    with conn.transaction() as tx:
        q = lambda sql, p=(): conn.execute(sql, p).fetchall()
        emp = {r[1]: r for r in q("select id, btrim(nss), id_cliente, nombre from empleados "
                                  "where btrim(nss) in (%s,%s)", (nss_o, nss_d))}
        if nss_o not in emp or nss_d not in emp:
            raise SystemExit(f"No existe alguno de los NSS: encontrados {list(emp)}")
        (eo, _, cli_o, nom_o), (ed, _, cli_d, nom_d) = emp[nss_o], emp[nss_d]
        if cli_o != cli_d:
            raise SystemExit("Los empleados pertenecen a clientes distintos: no se fusionan")
        print(f"Origen  {nss_o} {nom_o} ({eo})\nDestino {nss_d} {nom_d} ({ed})\n")

        for lo, rp, anio, mes, arch, id_arch, origen in q(
                "select id, id_registro_patronal, anio_periodo, mes_periodo_int, nombre_archivo, "
                "id_archivo, origen from liquidaciones where id_empleado=%s order by 3,4", (eo,)):
            ld = q("select id from liquidaciones where id_empleado=%s and id_registro_patronal=%s "
                   "and anio_periodo=%s and mes_periodo_int=%s", (ed, rp, anio, mes))
            tag = f"{anio}-{mes:02d}"
            if not ld:
                n = conn.execute("update reportes_liquidaciones set id_empleado=%s where id_liquidacion=%s",
                                 (ed, lo)).rowcount
                conn.execute("update liquidaciones set id_empleado=%s where id=%s", (ed, lo))
                print(f"{tag}: liquidación {lo} reasignada al destino ({n} filas de reporte)")
                continue
            ld = ld[0][0]
            # Obras se comparan por folio y días: los registros_obras son por revisión, así que
            # el mismo folio tiene id distinto según la revisión con la que se cargó.
            vin = ("(select btrim(ro.numero), v.dias_registro_obra from vinculaciones_obras_liquidaciones v "
                   "join registros_obras ro on ro.id=v.id_registro_obra where v.id_liquidacion=%s)")
            det = f"(select {DET_COLS} from detalles_liquidaciones where id_liquidacion=%s)"
            for nombre, sub in (("detalles", det), ("obras", vin)):
                dif = q(f"({sub} except all {sub}) union all ({sub} except all {sub})", (lo, ld, ld, lo))
                if dif:
                    raise SystemExit(f"{tag}: {nombre} difieren entre {lo} y {ld}: {dif}; se aborta sin cambios")
            # Gana la carga más reciente: L_d adopta archivo y vinculaciones de L_o
            conn.execute("update liquidaciones set nombre_archivo=%s, id_archivo=%s, origen=%s where id=%s",
                         (arch, id_arch, origen, ld))
            conn.execute("delete from vinculaciones_obras_liquidaciones where id_liquidacion=%s", (ld,))
            conn.execute("update vinculaciones_obras_liquidaciones set id_liquidacion=%s where id_liquidacion=%s",
                         (ld, lo))
            borr = reas = 0
            for rl, rev in q("select id, id_revision from reportes_liquidaciones where id_liquidacion=%s", (lo,)):
                if q("select 1 from reportes_liquidaciones where id_liquidacion=%s and "
                     "id_revision is not distinct from %s", (ld, rev)):
                    conn.execute("delete from reportes_liquidaciones where id=%s", (rl,)); borr += 1
                else:
                    conn.execute("update reportes_liquidaciones set id_liquidacion=%s, id_empleado=%s "
                                 "where id=%s", (ld, ed, rl)); reas += 1
            conn.execute("delete from liquidaciones where id=%s", (lo,))
            print(f"{tag}: duplicada {lo} eliminada; se conserva {ld} (archivo -> {arch}); "
                  f"filas de reporte: {borr} eliminadas, {reas} reasignadas")

        n = conn.execute(
            "insert into public.empleados_nss_equivalencias (id_cliente, nss_alterno, id_empleado, motivo) "
            "values (%s, %s, %s, %s) on conflict (id_cliente, nss_alterno) do nothing",
            (cli_d, nss_o, ed, motivo or f"Fusión de empleado duplicado {nss_o} en {nss_d}")).rowcount
        print(f"\nEquivalencia {nss_o} -> {nss_d}: {'registrada' if n else 'ya existía'}")

        refs = q("select (select count(*) from liquidaciones where id_empleado=%(e)s)"
                 " + (select count(*) from nominas where id_empleado=%(e)s)"
                 " + (select count(*) from reportes_liquidaciones where id_empleado=%(e)s)"
                 " + (select count(*) from reportes_nominas where id_empleado=%(e)s)", {"e": eo})[0][0]
        if refs == 0:
            conn.execute("delete from empleados where id=%s", (eo,))
            print(f"\nEmpleado origen {nss_o} eliminado (sin referencias)")
        else:
            print(f"\nEmpleado origen conserva {refs} referencias: NO se elimina")

        # Verificación: ninguna revisión debe tener dos filas ACTIVO del mismo nombre en un periodo
        dup = q("select rl.id_revision, rl.anio_periodo, rl.mes_periodo_int, count(*) from reportes_liquidaciones rl "
                "join empleados e on e.id=rl.id_empleado where e.nombre=%s and rl.estado='ACTIVO' "
                "group by 1,2,3 having count(*)>1", (nom_d,))
        print("Duplicados ACTIVO restantes:", dup or "ninguno")
        for a, m, n in q("select anio_periodo, mes_periodo_int, count(*) from liquidaciones l join empleados e "
                         "on e.id=l.id_empleado where e.nombre=%s group by 1,2 having count(*)>1", (nom_d,)):
            print(f"  ¡Aún hay {n} liquidaciones de {nom_d} en {a}-{m:02d}!")
        print("Liquidaciones del destino:", q("select count(*) from liquidaciones where id_empleado=%s", (ed,))[0][0])
        if not aplicar:
            print("\nSIMULACIÓN: se revierte todo. Usa --aplicar para confirmar.")
            raise psycopg.Rollback(tx)
        print("\nCAMBIOS APLICADOS (commit).")

if __name__ == "__main__":
    main()
