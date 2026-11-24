//! Advertencias de carga de cédulas de cuotas y nóminas.
//!
//! Los clientes suelen capturar con errores sus propios datos (RFC, razón social, registro patronal)
//! y los de sus trabajadores (NSS, RFC, CURP, nombre) aunque el documento sí corresponda. En lugar de
//! rechazar la carga, aquí se detectan esas discrepancias como *advertencias* para que el usuario
//! decida, por cada una, si deja el dato como viene, usa el valor sugerido o lo corrige, y qué
//! archivos carga (todos, algunos o ninguno). Ver `carga_revision_ui.rs` para el modal.
//!
//! Este módulo es lógica pura (sin UI ni BD salvo `cargar_catalogo`), para poder probarlo.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use postgres::GenericClient;
use uuid::Uuid;

use crate::validation::{clean_alphanumeric_input, cross_validate_rfc_curp, is_generic_sat_identifier, validate_curp, validate_rfc};

/// Qué dato tiene la discrepancia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CampoAdvertencia {
    RegistroPatronal,
    RfcPatron,
    RazonSocial,
    Nss,
    Rfc,
    Curp,
    Nombre,
}

impl CampoAdvertencia {
    pub fn etiqueta(&self) -> &'static str {
        match self {
            CampoAdvertencia::RegistroPatronal => "Registro Patronal",
            CampoAdvertencia::RfcPatron => "RFC del patrón",
            CampoAdvertencia::RazonSocial => "Razón social",
            CampoAdvertencia::Nss => "NSS",
            CampoAdvertencia::Rfc => "RFC",
            CampoAdvertencia::Curp => "CURP",
            CampoAdvertencia::Nombre => "Nombre",
        }
    }

    /// Datos del patrón (aplican al archivo completo) frente a datos de un trabajador.
    pub fn es_del_patron(&self) -> bool {
        matches!(self, CampoAdvertencia::RegistroPatronal | CampoAdvertencia::RfcPatron | CampoAdvertencia::RazonSocial)
    }

    /// Valida un valor corregido a mano. Devuelve el valor normalizado.
    pub fn validar_correccion(&self, valor: &str) -> Result<String, String> {
        let v = valor.trim();
        if v.is_empty() {
            return Err("Escribe el valor corregido.".to_string());
        }
        match self {
            CampoAdvertencia::Nss => validar_nss_imss(v),
            CampoAdvertencia::Rfc => validate_rfc(v, Some("FISICA")).map(|i| i.clean),
            CampoAdvertencia::RfcPatron => validate_rfc(v, None).map(|i| i.clean),
            CampoAdvertencia::Curp => validate_curp(v).map(|_| clean_alphanumeric_input(v)),
            CampoAdvertencia::RegistroPatronal => {
                let c = clean_alphanumeric_input(v);
                crate::validation::validate_registro_patronal(&c).map(|_| c)
            }
            CampoAdvertencia::Nombre | CampoAdvertencia::RazonSocial => {
                let n = normalizar_espacios(&v.to_uppercase());
                if n.chars().filter(|c| c.is_alphabetic()).count() < 3 {
                    Err("El nombre debe tener al menos 3 letras.".to_string())
                } else if self == &CampoAdvertencia::Nombre && n.chars().any(|c| c.is_ascii_digit()) {
                    Err("El nombre no debe contener números.".to_string())
                } else {
                    Ok(n)
                }
            }
        }
    }
}

/// Lo que el usuario decidió hacer con una advertencia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccionAdvertencia {
    /// Cargar el dato tal como viene en el archivo.
    Dejar,
    /// Reemplazarlo por el valor sugerido (catálogo, revisión u otras cédulas).
    UsarSugerido,
    /// Reemplazarlo por un valor capturado por el usuario (ya validado).
    Corregir(String),
}

/// Identidad original (tal como vino en el archivo) del trabajador al que aplica una advertencia.
/// Se usa para encontrar sus renglones al aplicar las decisiones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClaveTrabajador {
    pub nss: String,
    pub nombre: String,
    pub rfc: String,
    pub curp: String,
}

impl ClaveTrabajador {
    pub fn nueva(nss: Option<&str>, nombre: Option<&str>, rfc: Option<&str>, curp: Option<&str>) -> Self {
        Self {
            nss: nss.map(clean_alphanumeric_input).unwrap_or_default(),
            nombre: nombre.map(|n| normalizar_espacios(&n.to_uppercase())).unwrap_or_default(),
            rfc: rfc.map(clean_alphanumeric_input).unwrap_or_default(),
            curp: curp.map(clean_alphanumeric_input).unwrap_or_default(),
        }
    }

    pub fn descripcion(&self) -> String {
        let nombre = if self.nombre.is_empty() { "(sin nombre)" } else { self.nombre.as_str() };
        if self.nss.is_empty() {
            nombre.to_string()
        } else {
            format!("{} · NSS {}", nombre, self.nss)
        }
    }
}

/// Una discrepancia detectada, agrupada: el mismo problema del mismo trabajador en varios
/// archivos (por ejemplo, 12 cédulas mensuales) es una sola advertencia.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvertenciaCarga {
    pub id: usize,
    pub campo: CampoAdvertencia,
    /// Trabajador afectado (vacío para advertencias del patrón).
    pub trabajador: Option<ClaveTrabajador>,
    pub archivos: BTreeSet<String>,
    pub encontrado: String,
    pub sugerido: Option<String>,
    /// De dónde sale el sugerido: "catálogo de empleados", "revisión", "otras cédulas"…
    pub origen_sugerido: String,
    pub mensaje: String,
    /// Discrepancia grave (por ejemplo, registro patronal distinto al de la revisión).
    pub grave: bool,
    /// El dato no se puede corregir (p. ej. datos fiscales de un CFDI timbrado): solo se decide si
    /// el archivo se carga o no.
    pub solo_decidir_archivo: bool,
}

/// Datos de un renglón de trabajador tal como vienen en el archivo.
#[derive(Debug, Clone)]
pub struct RegistroTrabajador {
    pub archivo: String,
    pub nss: Option<String>,
    pub nombre: Option<String>,
    pub rfc: Option<String>,
    pub curp: Option<String>,
}

impl RegistroTrabajador {
    pub fn clave(&self) -> ClaveTrabajador {
        ClaveTrabajador::nueva(self.nss.as_deref(), self.nombre.as_deref(), self.rfc.as_deref(), self.curp.as_deref())
    }
}

/// Empleado del catálogo del cliente.
#[derive(Debug, Clone, PartialEq)]
pub struct EmpleadoCatalogo {
    pub id: Uuid,
    pub nombre: String,
    pub nss: String,
    pub rfc: String,
    pub curp: String,
}

/// Catálogo de empleados del cliente indexado para comparar contra la carga.
#[derive(Debug, Clone, Default)]
pub struct CatalogoEmpleados {
    empleados: Vec<EmpleadoCatalogo>,
    por_nss: HashMap<String, usize>,
    por_curp: HashMap<String, usize>,
    por_rfc: HashMap<String, usize>,
    por_nombre: HashMap<String, Vec<usize>>,
    /// NSS alternos registrados (empleados_nss_equivalencias) → empleado
    alternos: HashMap<String, usize>,
}

impl CatalogoEmpleados {
    pub fn nuevo(empleados: Vec<EmpleadoCatalogo>, alternos: Vec<(String, Uuid)>) -> Self {
        let mut c = CatalogoEmpleados { empleados, ..Default::default() };
        for (i, e) in c.empleados.iter().enumerate() {
            if !e.nss.is_empty() {
                c.por_nss.entry(e.nss.clone()).or_insert(i);
            }
            if e.curp.len() == 18 && !is_generic_sat_identifier(&e.curp) {
                c.por_curp.entry(e.curp.clone()).or_insert(i);
            }
            if e.rfc.len() == 13 && !is_generic_sat_identifier(&e.rfc) {
                c.por_rfc.entry(e.rfc.clone()).or_insert(i);
            }
            let n = clave_nombre(&e.nombre);
            if !n.is_empty() {
                c.por_nombre.entry(n).or_default().push(i);
            }
        }
        for (nss, id) in alternos {
            if let Some(i) = c.empleados.iter().position(|e| e.id == id) {
                c.alternos.insert(nss, i);
            }
        }
        c
    }

    pub fn len(&self) -> usize {
        self.empleados.len()
    }

    fn por_nss_o_alterno(&self, nss: &str) -> Option<&EmpleadoCatalogo> {
        self.por_nss.get(nss).or_else(|| self.alternos.get(nss)).map(|&i| &self.empleados[i])
    }

    fn es_alterno(&self, nss: &str) -> bool {
        self.alternos.contains_key(nss)
    }
}

/// Carga el catálogo de empleados (y NSS alternos) del cliente dueño de la revisión.
pub fn cargar_catalogo<C: GenericClient>(client: &mut C, id_revision: Uuid) -> Result<CatalogoEmpleados, String> {
    let filas = client
        .query(
            "SELECT e.id, COALESCE(e.nombre, ''), COALESCE(BTRIM(e.nss), ''), COALESCE(BTRIM(e.rfc), ''), COALESCE(BTRIM(e.curp), '')
             FROM public.empleados e
             WHERE e.id_cliente = (
                SELECT rp.id_cliente FROM public.revisiones r
                JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
                WHERE r.id = $1)",
            &[&id_revision],
        )
        .map_err(|e| format!("Error consultando el catálogo de empleados: {}", crate::db::schema::format_pg_error(&e)))?;
    let empleados = filas
        .iter()
        .map(|r| EmpleadoCatalogo {
            id: r.get(0),
            nombre: normalizar_espacios(&r.get::<_, String>(1).to_uppercase()),
            nss: clean_alphanumeric_input(&r.get::<_, String>(2)),
            rfc: clean_alphanumeric_input(&r.get::<_, String>(3)),
            curp: clean_alphanumeric_input(&r.get::<_, String>(4)),
        })
        .collect();
    // La tabla de equivalencias puede no existir en bases antiguas: si falla, se ignora.
    let alternos = client
        .query(
            "SELECT eq.nss_alterno, eq.id_empleado FROM public.empleados_nss_equivalencias eq
             WHERE eq.id_cliente = (
                SELECT rp.id_cliente FROM public.revisiones r
                JOIN public.registros_patronales rp ON rp.id = r.id_registro_patronal
                WHERE r.id = $1)",
            &[&id_revision],
        )
        .map(|rows| rows.iter().map(|r| (clean_alphanumeric_input(&r.get::<_, String>(0)), r.get::<_, Uuid>(1))).collect())
        .unwrap_or_default();
    Ok(CatalogoEmpleados::nuevo(empleados, alternos))
}

/// Colapsa espacios múltiples y recorta.
pub fn normalizar_espacios(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Clave para comparar nombres: mayúsculas, sin acentos ni signos, palabras ordenadas
/// (así "PEREZ LOPEZ JUAN" y "JUAN PÉREZ LÓPEZ" coinciden).
pub fn clave_nombre(nombre: &str) -> String {
    let sin_acentos: String = nombre
        .to_uppercase()
        .chars()
        .map(|c| match c {
            'Á' | 'À' | 'Ä' => 'A',
            'É' | 'È' | 'Ë' => 'E',
            'Í' | 'Ì' | 'Ï' => 'I',
            'Ó' | 'Ò' | 'Ö' => 'O',
            'Ú' | 'Ù' | 'Ü' => 'U',
            'Ñ' => 'N',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    let mut palabras: Vec<&str> = sin_acentos.split_whitespace().collect();
    palabras.sort_unstable();
    palabras.join(" ")
}

/// NSS del IMSS: 11 dígitos con dígito verificador válido (sin reglas de años, que dan falsos
/// positivos con NSS reales).
pub fn validar_nss_imss(nss: &str) -> Result<String, String> {
    crate::db::equivalencias_nss::normalizar_nss_alterno(nss)
}

fn es_rfc_real(rfc: &str) -> bool {
    !rfc.is_empty() && !is_generic_sat_identifier(rfc)
}

/// Acumulador que agrupa advertencias iguales de varios archivos.
#[derive(Default)]
struct Acumulador {
    mapa: BTreeMap<(CampoAdvertencia, ClaveTrabajador, String), AdvertenciaCarga>,
}

impl Acumulador {
    #[allow(clippy::too_many_arguments)]
    fn agregar(
        &mut self,
        campo: CampoAdvertencia,
        clave: &ClaveTrabajador,
        archivo: &str,
        encontrado: &str,
        sugerido: Option<String>,
        origen: &str,
        mensaje: String,
    ) {
        let k = (campo, clave.clone(), encontrado.to_string());
        let adv = self.mapa.entry(k).or_insert_with(|| AdvertenciaCarga {
            id: 0,
            campo,
            trabajador: Some(clave.clone()),
            archivos: BTreeSet::new(),
            encontrado: encontrado.to_string(),
            sugerido: sugerido.filter(|s| !s.is_empty() && s != encontrado),
            origen_sugerido: origen.to_string(),
            mensaje,
            grave: false,
            solo_decidir_archivo: false,
        });
        adv.archivos.insert(archivo.to_string());
    }
}

/// Valor más frecuente (y, ante empate, el primero en orden).
fn mayoritario(conteo: &HashMap<String, usize>) -> Option<String> {
    let mut v: Vec<(&String, &usize)> = conteo.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    v.first().map(|(s, _)| (*s).clone())
}

/// Detecta discrepancias en los datos de trabajadores de una carga.
///
/// Reglas (todas son advertencias; el usuario decide):
/// - Formato: NSS (11 dígitos + verificador), RFC de persona física, CURP, y RFC vs CURP.
/// - Nombre vacío o con números.
/// - Contra el catálogo del cliente: el NSS existe con otro nombre/CURP/RFC; o el NSS no existe
///   pero su CURP/RFC/nombre sí (posible NSS mal capturado o duplicado).
/// - Dentro de la misma carga: un NSS con nombres distintos, o un nombre con NSS distintos.
pub fn detectar_trabajadores(registros: &[RegistroTrabajador], catalogo: &CatalogoEmpleados) -> Vec<AdvertenciaCarga> {
    let mut acc = Acumulador::default();

    // Coherencia interna de la carga
    let mut nombres_por_nss: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let mut nss_por_nombre: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for r in registros {
        let c = r.clave();
        if !c.nss.is_empty() && !c.nombre.is_empty() {
            *nombres_por_nss.entry(c.nss.clone()).or_default().entry(c.nombre.clone()).or_default() += 1;
            *nss_por_nombre.entry(clave_nombre(&c.nombre)).or_default().entry(c.nss.clone()).or_default() += 1;
        }
    }

    for r in registros {
        let c = r.clave();
        let archivo = r.archivo.as_str();
        let en_catalogo = if c.nss.is_empty() { None } else { catalogo.por_nss_o_alterno(&c.nss) };

        // ── Nombre ──
        if c.nombre.is_empty() {
            acc.agregar(
                CampoAdvertencia::Nombre,
                &c,
                archivo,
                "",
                en_catalogo.map(|e| e.nombre.clone()),
                "catálogo de empleados",
                "Falta el nombre del trabajador.".to_string(),
            );
        } else if c.nombre.chars().any(|ch| ch.is_ascii_digit()) {
            acc.agregar(
                CampoAdvertencia::Nombre,
                &c,
                archivo,
                &c.nombre,
                en_catalogo.map(|e| e.nombre.clone()),
                "catálogo de empleados",
                "El nombre contiene números; puede ser un error de captura.".to_string(),
            );
        }

        // ── NSS ──
        let nss_valido = !c.nss.is_empty() && validar_nss_imss(&c.nss).is_ok();
        let sugerido_nss_por_otros = || -> Option<&EmpleadoCatalogo> {
            if !c.curp.is_empty() {
                if let Some(&i) = catalogo.por_curp.get(&c.curp) {
                    return Some(&catalogo.empleados[i]);
                }
            }
            if es_rfc_real(&c.rfc) {
                if let Some(&i) = catalogo.por_rfc.get(&c.rfc) {
                    return Some(&catalogo.empleados[i]);
                }
            }
            None
        };
        if c.nss.is_empty() {
            let s = sugerido_nss_por_otros();
            acc.agregar(
                CampoAdvertencia::Nss,
                &c,
                archivo,
                "",
                s.map(|e| e.nss.clone()),
                "catálogo de empleados",
                "Falta el NSS del trabajador.".to_string(),
            );
        } else if !nss_valido {
            let s = sugerido_nss_por_otros();
            let motivo = validar_nss_imss(&c.nss).err().unwrap_or_default();
            acc.agregar(CampoAdvertencia::Nss, &c, archivo, &c.nss, s.map(|e| e.nss.clone()), "catálogo de empleados", motivo);
        }

        // ── RFC ──
        let mut rfc_valido = false;
        if es_rfc_real(&c.rfc) {
            match validate_rfc(&c.rfc, Some("FISICA")) {
                Ok(i) if i.is_moral => acc.agregar(
                    CampoAdvertencia::Rfc,
                    &c,
                    archivo,
                    &c.rfc,
                    en_catalogo.map(|e| e.rfc.clone()).filter(|r| es_rfc_real(r)),
                    "catálogo de empleados",
                    "El RFC del trabajador debe ser de persona física (13 posiciones).".to_string(),
                ),
                Ok(_) => rfc_valido = true,
                Err(e) => acc.agregar(
                    CampoAdvertencia::Rfc,
                    &c,
                    archivo,
                    &c.rfc,
                    en_catalogo.map(|e| e.rfc.clone()).filter(|r| es_rfc_real(r)),
                    "catálogo de empleados",
                    e,
                ),
            }
        }

        // ── CURP ──
        let mut curp_valida = false;
        if !c.curp.is_empty() && !is_generic_sat_identifier(&c.curp) {
            match validate_curp(&c.curp) {
                Ok(_) => curp_valida = true,
                Err(e) => acc.agregar(
                    CampoAdvertencia::Curp,
                    &c,
                    archivo,
                    &c.curp,
                    en_catalogo.map(|e| e.curp.clone()).filter(|x| x.len() == 18),
                    "catálogo de empleados",
                    e,
                ),
            }
        }
        if rfc_valido && curp_valida {
            if let Err(e) = cross_validate_rfc_curp(&c.rfc, &c.curp) {
                acc.agregar(
                    CampoAdvertencia::Rfc,
                    &c,
                    archivo,
                    &c.rfc,
                    en_catalogo.map(|e| e.rfc.clone()).filter(|r| es_rfc_real(r) && *r != c.rfc),
                    "catálogo de empleados",
                    e,
                );
            }
        }

        // ── Contra el catálogo ──
        if let Some(e) = en_catalogo {
            if !c.nombre.is_empty() && !e.nombre.is_empty() && clave_nombre(&c.nombre) != clave_nombre(&e.nombre) {
                acc.agregar(
                    CampoAdvertencia::Nombre,
                    &c,
                    archivo,
                    &c.nombre,
                    Some(e.nombre.clone()),
                    "catálogo de empleados",
                    format!("En el catálogo, el NSS {} pertenece a «{}».", c.nss, e.nombre),
                );
            }
            if curp_valida && e.curp.len() == 18 && e.curp != c.curp {
                acc.agregar(
                    CampoAdvertencia::Curp,
                    &c,
                    archivo,
                    &c.curp,
                    Some(e.curp.clone()),
                    "catálogo de empleados",
                    format!("En el catálogo, este trabajador tiene la CURP {}.", e.curp),
                );
            }
            if rfc_valido && es_rfc_real(&e.rfc) && e.rfc.len() == 13 && e.rfc != c.rfc {
                acc.agregar(
                    CampoAdvertencia::Rfc,
                    &c,
                    archivo,
                    &c.rfc,
                    Some(e.rfc.clone()),
                    "catálogo de empleados",
                    format!("En el catálogo, este trabajador tiene el RFC {}.", e.rfc),
                );
            }
        } else if nss_valido && !catalogo.es_alterno(&c.nss) {
            // NSS nuevo: ¿ya existe la persona con otro NSS?
            if let Some(e) = sugerido_nss_por_otros().filter(|e| !e.nss.is_empty() && e.nss != c.nss) {
                let por = if !c.curp.is_empty() && e.curp == c.curp { "CURP" } else { "RFC" };
                acc.agregar(
                    CampoAdvertencia::Nss,
                    &c,
                    archivo,
                    &c.nss,
                    Some(e.nss.clone()),
                    "catálogo de empleados",
                    format!(
                        "La {} de este trabajador ya está en el catálogo con el NSS {} («{}»). Puede ser un NSS mal capturado.",
                        por, e.nss, e.nombre
                    ),
                );
            } else if !c.nombre.is_empty() {
                if let Some(idx) = catalogo.por_nombre.get(&clave_nombre(&c.nombre)) {
                    let otros: Vec<&EmpleadoCatalogo> =
                        idx.iter().map(|&i| &catalogo.empleados[i]).filter(|e| !e.nss.is_empty() && e.nss != c.nss).collect();
                    if let Some(e) = otros.first() {
                        acc.agregar(
                            CampoAdvertencia::Nss,
                            &c,
                            archivo,
                            &c.nss,
                            Some(e.nss.clone()),
                            "catálogo de empleados",
                            format!(
                                "Ya existe «{}» en el catálogo con el NSS {}. Si es la misma persona, se crearía un empleado duplicado.",
                                e.nombre, e.nss
                            ),
                        );
                    }
                }
            }
        }

        // ── Coherencia dentro de la carga ──
        if !c.nss.is_empty() && !c.nombre.is_empty() {
            if let Some(nombres) = nombres_por_nss.get(&c.nss) {
                let distintos: HashSet<String> = nombres.keys().map(|n| clave_nombre(n)).collect();
                if distintos.len() > 1 {
                    if let Some(may) = mayoritario(nombres) {
                        if clave_nombre(&may) != clave_nombre(&c.nombre) {
                            acc.agregar(
                                CampoAdvertencia::Nombre,
                                &c,
                                archivo,
                                &c.nombre,
                                Some(may.clone()),
                                "otras cédulas de esta carga",
                                format!("El mismo NSS aparece como «{}» en otros archivos de esta carga.", may),
                            );
                        }
                    }
                }
            }
            if let Some(nss_set) = nss_por_nombre.get(&clave_nombre(&c.nombre)) {
                if nss_set.len() > 1 {
                    if let Some(may) = mayoritario(nss_set) {
                        if may != c.nss {
                            acc.agregar(
                                CampoAdvertencia::Nss,
                                &c,
                                archivo,
                                &c.nss,
                                Some(may.clone()),
                                "otras cédulas de esta carga",
                                format!(
                                    "«{}» aparece con el NSS {} en otros archivos de esta carga. Si es la misma persona, uno de los dos NSS está mal.",
                                    c.nombre, may
                                ),
                            );
                        }
                    }
                }
            }
        }
    }

    acc.mapa.into_values().collect()
}

/// Advertencia de un dato del patrón (aplica a todo el archivo).
pub fn advertencia_patron(
    campo: CampoAdvertencia,
    archivo: &str,
    encontrado: &str,
    sugerido: &str,
    mensaje: String,
    grave: bool,
) -> AdvertenciaCarga {
    AdvertenciaCarga {
        id: 0,
        campo,
        trabajador: None,
        archivos: BTreeSet::from([archivo.to_string()]),
        encontrado: encontrado.to_string(),
        sugerido: Some(sugerido.to_string()).filter(|s| !s.is_empty()),
        origen_sugerido: "revisión".to_string(),
        mensaje,
        grave,
        solo_decidir_archivo: false,
    }
}

/// Ordena (patrón y graves primero) y numera las advertencias.
pub fn numerar(mut advs: Vec<AdvertenciaCarga>) -> Vec<AdvertenciaCarga> {
    advs.sort_by(|a, b| {
        b.grave
            .cmp(&a.grave)
            .then(b.campo.es_del_patron().cmp(&a.campo.es_del_patron()))
            .then(a.archivos.iter().next().cmp(&b.archivos.iter().next()))
            .then(a.trabajador.cmp(&b.trabajador))
            .then(a.campo.cmp(&b.campo))
    });
    for (i, a) in advs.iter_mut().enumerate() {
        a.id = i;
    }
    advs
}

/// Valor final de un campo según la decisión (None = dejar como viene).
pub fn valor_decidido(adv: &AdvertenciaCarga, accion: &AccionAdvertencia) -> Option<String> {
    match accion {
        AccionAdvertencia::Dejar => None,
        AccionAdvertencia::UsarSugerido => adv.sugerido.clone(),
        AccionAdvertencia::Corregir(v) => Some(v.clone()),
    }
}

/// Decisión final del usuario sobre la carga.
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionCarga {
    /// No cargar nada.
    Cancelar,
    /// Cargar los archivos `incluidos` aplicando las acciones por advertencia (id → acción).
    Cargar {
        incluidos: BTreeSet<String>,
        acciones: HashMap<usize, AccionAdvertencia>,
    },
}

/// Cambios a aplicar a los renglones de un trabajador de un archivo: campo → valor nuevo.
pub type CambiosTrabajador = HashMap<CampoAdvertencia, String>;

/// Agrupa las decisiones de trabajadores en (archivo, clave original) → cambios.
pub fn cambios_por_trabajador(
    advertencias: &[AdvertenciaCarga],
    acciones: &HashMap<usize, AccionAdvertencia>,
) -> HashMap<(String, ClaveTrabajador), CambiosTrabajador> {
    let mut out: HashMap<(String, ClaveTrabajador), CambiosTrabajador> = HashMap::new();
    for adv in advertencias {
        let Some(clave) = adv.trabajador.as_ref() else { continue };
        if adv.solo_decidir_archivo {
            continue;
        }
        let Some(accion) = acciones.get(&adv.id) else { continue };
        let Some(valor) = valor_decidido(adv, accion) else { continue };
        for archivo in &adv.archivos {
            out.entry((archivo.clone(), clave.clone())).or_default().insert(adv.campo, valor.clone());
        }
    }
    out
}

/// Decisiones sobre datos del patrón: archivo → (campo → valor nuevo, o None si se deja).
pub fn cambios_por_archivo_patron(
    advertencias: &[AdvertenciaCarga],
    acciones: &HashMap<usize, AccionAdvertencia>,
) -> HashMap<String, HashMap<CampoAdvertencia, Option<String>>> {
    let mut out: HashMap<String, HashMap<CampoAdvertencia, Option<String>>> = HashMap::new();
    for adv in advertencias.iter().filter(|a| a.trabajador.is_none() && !a.solo_decidir_archivo) {
        let accion = acciones.get(&adv.id).cloned().unwrap_or(AccionAdvertencia::Dejar);
        for archivo in &adv.archivos {
            out.entry(archivo.clone()).or_default().insert(adv.campo, valor_decidido(adv, &accion));
        }
    }
    out
}

/// Aplica la decisión del usuario a las cédulas auditadas (en memoria, antes de guardar):
/// - quita los trabajadores de los archivos omitidos (y devuelve esos archivos);
/// - en archivos con discrepancias del patrón marca la cédula para no tocar el catálogo del cliente
///   y, si se eligió sugerido/corrección, reemplaza el dato del patrón;
/// - reemplaza NSS/RFC/CURP/nombre de los trabajadores según cada advertencia.
pub fn aplicar_a_cedulas(
    businesses: &mut Vec<crate::cedulas::business::Business>,
    advertencias: &[AdvertenciaCarga],
    acciones: &HashMap<usize, AccionAdvertencia>,
    incluidos: &BTreeSet<String>,
    archivos_revisados: &[String],
) -> Vec<String> {
    let omitidos: BTreeSet<String> = archivos_revisados.iter().filter(|f| !incluidos.contains(*f)).cloned().collect();

    // 1. Archivos omitidos
    if !omitidos.is_empty() {
        for b in businesses.iter_mut() {
            b.employees.retain(|e| e.file_name.as_deref().is_none_or(|f| !omitidos.contains(f)));
        }
        businesses.retain(|b| !b.employees.is_empty());
    }

    // 2. Datos del patrón
    let patron = cambios_por_archivo_patron(advertencias, acciones);
    for b in businesses.iter_mut() {
        let archivo = b.employees.first().and_then(|e| e.file_name.clone()).unwrap_or_default();
        if let Some(cambios) = patron.get(&archivo) {
            b.omitir_actualizar_catalogo = true;
            for (campo, valor) in cambios {
                let Some(v) = valor.clone() else { continue };
                match campo {
                    CampoAdvertencia::RegistroPatronal => b.registro_patronal = Some(v),
                    CampoAdvertencia::RfcPatron => b.rfc = Some(v),
                    CampoAdvertencia::RazonSocial => b.razon_social = Some(v),
                    _ => {}
                }
            }
        }
    }

    // 3. Datos de trabajadores: se localizan con la identidad ORIGINAL antes de modificar nada
    let cambios = cambios_por_trabajador(advertencias, acciones);
    if !cambios.is_empty() {
        let mut pendientes: Vec<(usize, usize, CambiosTrabajador)> = Vec::new();
        for (bi, b) in businesses.iter().enumerate() {
            for (ei, e) in b.employees.iter().enumerate() {
                let clave = ClaveTrabajador::nueva(
                    e.no_seguro_social.as_deref(),
                    e.nombre.as_deref(),
                    e.rfc.as_deref(),
                    e.curp.as_deref(),
                );
                let archivo = e.file_name.clone().unwrap_or_default();
                if let Some(c) = cambios.get(&(archivo, clave)) {
                    pendientes.push((bi, ei, c.clone()));
                }
            }
        }
        for (bi, ei, c) in pendientes {
            let e = &mut businesses[bi].employees[ei];
            for (campo, v) in c {
                match campo {
                    CampoAdvertencia::Nss => e.no_seguro_social = Some(v),
                    CampoAdvertencia::Rfc => e.rfc = Some(v),
                    CampoAdvertencia::Curp => e.curp = Some(v),
                    CampoAdvertencia::Nombre => e.nombre = Some(v),
                    _ => {}
                }
            }
        }
    }

    omitidos.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(archivo: &str, nss: &str, nombre: &str, rfc: &str, curp: &str) -> RegistroTrabajador {
        let o = |s: &str| if s.is_empty() { None } else { Some(s.to_string()) };
        RegistroTrabajador { archivo: archivo.into(), nss: o(nss), nombre: o(nombre), rfc: o(rfc), curp: o(curp) }
    }

    fn emp(nombre: &str, nss: &str, rfc: &str, curp: &str) -> EmpleadoCatalogo {
        EmpleadoCatalogo { id: Uuid::now_v7(), nombre: nombre.into(), nss: nss.into(), rfc: rfc.into(), curp: curp.into() }
    }

    // NSS válidos (dígito verificador correcto)
    const NSS_A: &str = "12345678903";
    const NSS_B: &str = "32998253382";

    #[test]
    fn nss_valido_de_prueba() {
        assert!(validar_nss_imss(NSS_A).is_ok());
        assert!(validar_nss_imss(NSS_B).is_ok());
        assert!(validar_nss_imss("12345678901").is_err());
    }

    #[test]
    fn nss_invalido_genera_advertencia_agrupada() {
        let regs = vec![
            reg("ene.pdf", "12345678901", "JUAN PEREZ", "", ""),
            reg("feb.pdf", "12345678901", "JUAN PEREZ", "", ""),
        ];
        let advs = detectar_trabajadores(&regs, &CatalogoEmpleados::default());
        let nss: Vec<_> = advs.iter().filter(|a| a.campo == CampoAdvertencia::Nss).collect();
        assert_eq!(nss.len(), 1);
        assert_eq!(nss[0].archivos.len(), 2);
    }

    #[test]
    fn nss_del_catalogo_con_otro_nombre() {
        let cat = CatalogoEmpleados::nuevo(vec![emp("JUAN PEREZ LOPEZ", NSS_A, "", "")], vec![]);
        let advs = detectar_trabajadores(&[reg("a.pdf", NSS_A, "JAUN PERES LOPEZ", "", "")], &cat);
        let n = advs.iter().find(|a| a.campo == CampoAdvertencia::Nombre).expect("advertencia de nombre");
        assert_eq!(n.sugerido.as_deref(), Some("JUAN PEREZ LOPEZ"));
        // Mismo nombre en otro orden o con acentos: sin advertencia
        let advs = detectar_trabajadores(&[reg("a.pdf", NSS_A, "LÓPEZ JUAN PÉREZ", "", "")], &cat);
        assert!(advs.iter().all(|a| a.campo != CampoAdvertencia::Nombre));
    }

    #[test]
    fn posible_duplicado_por_nombre() {
        let cat = CatalogoEmpleados::nuevo(vec![emp("ANA RUIZ", NSS_A, "", "")], vec![]);
        let advs = detectar_trabajadores(&[reg("a.pdf", NSS_B, "ANA RUIZ", "", "")], &cat);
        let n = advs.iter().find(|a| a.campo == CampoAdvertencia::Nss).expect("advertencia de NSS");
        assert_eq!(n.sugerido.as_deref(), Some(NSS_A));
        // Si el NSS nuevo es un alterno registrado del mismo empleado, no se advierte
        let e = emp("ANA RUIZ", NSS_A, "", "");
        let id = e.id;
        let cat2 = CatalogoEmpleados::nuevo(vec![e], vec![(NSS_B.to_string(), id)]);
        let advs = detectar_trabajadores(&[reg("a.pdf", NSS_B, "ANA RUIZ", "", "")], &cat2);
        assert!(advs.is_empty(), "{:?}", advs);
    }

    #[test]
    fn coherencia_interna_mismo_nss_otro_nombre() {
        let regs = vec![
            reg("ene.pdf", NSS_A, "JUAN PEREZ", "", ""),
            reg("feb.pdf", NSS_A, "JUAN PEREZ", "", ""),
            reg("mar.pdf", NSS_A, "JAUN PEREZ", "", ""),
        ];
        let advs = detectar_trabajadores(&regs, &CatalogoEmpleados::default());
        let n: Vec<_> = advs.iter().filter(|a| a.campo == CampoAdvertencia::Nombre).collect();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].encontrado, "JAUN PEREZ");
        assert_eq!(n[0].sugerido.as_deref(), Some("JUAN PEREZ"));
        assert!(n[0].archivos.contains("mar.pdf"));
    }

    #[test]
    fn aplica_decisiones_por_archivo_y_clave() {
        let regs = vec![reg("ene.pdf", "12345678901", "JUAN PEREZ", "", "")];
        let advs = numerar(detectar_trabajadores(&regs, &CatalogoEmpleados::default()));
        let id = advs.iter().find(|a| a.campo == CampoAdvertencia::Nss).unwrap().id;
        let acciones = HashMap::from([(id, AccionAdvertencia::Corregir(NSS_A.to_string()))]);
        let cambios = cambios_por_trabajador(&advs, &acciones);
        let clave = regs[0].clave();
        assert_eq!(cambios[&("ene.pdf".to_string(), clave)][&CampoAdvertencia::Nss], NSS_A);
    }

    #[test]
    fn aplicar_a_cedulas_omite_y_corrige() {
        use crate::cedulas::business::Business;
        use crate::cedulas::employees::Employee;
        let mk = |archivo: &str, nss: &str, nombre: &str| Employee {
            no_seguro_social: Some(nss.to_string()),
            nombre: Some(nombre.to_string()),
            file_name: Some(archivo.to_string()),
            ..Default::default()
        };
        let mut bs = vec![
            Business { employees: vec![mk("ene.pdf", "12345678901", "JUAN PEREZ")], ..Default::default() },
            Business { employees: vec![mk("feb.pdf", NSS_B, "ANA RUIZ")], ..Default::default() },
        ];
        let mut advs: Vec<AdvertenciaCarga> = Vec::new();
        advs.push(advertencia_patron(CampoAdvertencia::RfcPatron, "ene.pdf", "AAA010101AAA", "BBB010101BBB", "m".into(), false));
        let regs = vec![reg("ene.pdf", "12345678901", "JUAN PEREZ", "", "")];
        advs.extend(detectar_trabajadores(&regs, &CatalogoEmpleados::default()));
        let advs = numerar(advs);
        let id_nss = advs.iter().find(|a| a.campo == CampoAdvertencia::Nss).unwrap().id;
        let id_rfc = advs.iter().find(|a| a.campo == CampoAdvertencia::RfcPatron).unwrap().id;
        let acciones = HashMap::from([
            (id_nss, AccionAdvertencia::Corregir(NSS_A.to_string())),
            (id_rfc, AccionAdvertencia::UsarSugerido),
        ]);
        let incluidos = BTreeSet::from(["ene.pdf".to_string()]);
        let omitidos = aplicar_a_cedulas(&mut bs, &advs, &acciones, &incluidos, &["ene.pdf".to_string(), "feb.pdf".to_string()]);
        assert_eq!(omitidos, vec!["feb.pdf".to_string()]);
        assert_eq!(bs.len(), 1);
        assert!(bs[0].omitir_actualizar_catalogo);
        assert_eq!(bs[0].rfc.as_deref(), Some("BBB010101BBB"));
        assert_eq!(bs[0].employees[0].no_seguro_social.as_deref(), Some(NSS_A));
    }

    #[test]
    fn validar_correcciones() {
        assert!(CampoAdvertencia::Nss.validar_correccion("123").is_err());
        assert_eq!(CampoAdvertencia::Nss.validar_correccion(NSS_A).unwrap(), NSS_A);
        assert!(CampoAdvertencia::Nombre.validar_correccion("J1").is_err());
        assert_eq!(CampoAdvertencia::Nombre.validar_correccion("  juan   perez ").unwrap(), "JUAN PEREZ");
        assert!(CampoAdvertencia::Curp.validar_correccion("XYZ").is_err());
    }

    #[test]
    fn patron_primero_al_numerar() {
        let t = detectar_trabajadores(&[reg("a.pdf", "", "JUAN", "", "")], &CatalogoEmpleados::default());
        let mut v = t;
        v.push(advertencia_patron(CampoAdvertencia::RegistroPatronal, "a.pdf", "X", "Y", "m".into(), true));
        let v = numerar(v);
        assert_eq!(v[0].campo, CampoAdvertencia::RegistroPatronal);
        assert_eq!(v[0].id, 0);
    }
}
