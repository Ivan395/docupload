//! Verificación de librerías gráficas del sistema (Linux) antes de crear la ventana.
//!
//! Si falta alguna librería requerida por la interfaz, se informa al usuario con el comando
//! de instalación y, si hay `pkexec` y un diálogo gráfico, se ofrece instalarla.
//! Se puede omitir definiendo la variable de entorno `DOCU_SKIP_DEPS_CHECK`.

/// Punto de entrada: comprueba las dependencias del sistema. No hace nada fuera de Linux.
#[cfg(target_os = "linux")]
pub fn verificar_dependencias_sistema() {
    use std::env;

    if env::var_os("DOCU_SKIP_DEPS_CHECK").is_some() {
        return;
    }

    let requeridas = sonames_requeridos(env_presente("WAYLAND_DISPLAY"), env_presente("DISPLAY"));
    let faltantes: Vec<&str> = requeridas
        .into_iter()
        .filter(|soname| !biblioteca_disponible(soname))
        .collect();
    if faltantes.is_empty() {
        return;
    }

    let lista = faltantes.join(", ");
    let Some(gestor) = gestor_detectado() else {
        log::warn!("Faltan librerías del sistema: {lista}. No se encontró un gestor de paquetes conocido.");
        eprintln!(
            "DocuUpload: faltan librerías gráficas del sistema: {lista}\n\
             Instálelas con el gestor de paquetes de su distribución."
        );
        return;
    };

    let paquetes = paquetes_para(&faltantes, gestor);
    let args = argumentos_instalacion(gestor, &paquetes);
    let comando_manual = format!("sudo {} {}", gestor.binario(), args.join(" "));

    log::warn!("Faltan librerías del sistema: {lista}. Instalar con: {comando_manual}");
    eprintln!(
        "DocuUpload: faltan librerías gráficas del sistema: {lista}\n\
         Instálelas con: {comando_manual}"
    );

    let puede_preguntar = en_path("pkexec") && (en_path("zenity") || en_path("kdialog"));
    if !puede_preguntar {
        return;
    }

    let mensaje = format!(
        "Faltan librerías gráficas del sistema necesarias para DocuUpload:\n{lista}\n\n\
         ¿Desea instalarlas ahora? Se pedirá la contraseña de administrador."
    );
    if !confirmar_instalacion(&mensaje) {
        log::info!("El usuario omitió la instalación de dependencias del sistema.");
        return;
    }

    match std::process::Command::new("pkexec")
        .arg(gestor.binario())
        .args(&args)
        .status()
    {
        Ok(status) if status.success() => {
            log::info!("Dependencias del sistema instaladas correctamente.");
        }
        Ok(status) => {
            log::warn!("La instalación de dependencias terminó con código {:?}", status.code());
        }
        Err(e) => {
            log::warn!("No se pudo ejecutar pkexec para instalar dependencias: {e}");
        }
    }
}

/// Punto de entrada: en plataformas distintas de Linux no hay nada que verificar.
#[cfg(not(target_os = "linux"))]
pub fn verificar_dependencias_sistema() {}

/// Sonames de las librerías que Slint necesita según el servidor gráfico activo.
/// Con Wayland se usa `WAYLAND_DISPLAY`; si no, X11 con `DISPLAY`. OpenGL no es requisito
/// porque Slint cae a su renderizador por software.
#[cfg(any(target_os = "linux", test))]
fn sonames_requeridos(wayland: bool, x11: bool) -> Vec<&'static str> {
    if wayland {
        vec!["libwayland-client.so.0", "libxkbcommon.so.0"]
    } else if x11 {
        vec![
            "libX11.so.6",
            "libX11-xcb.so.1",
            "libXcursor.so.1",
            "libXi.so.6",
            "libxkbcommon.so.0",
            "libxkbcommon-x11.so.0",
        ]
    } else {
        Vec::new()
    }
}

/// Gestores de paquetes soportados, en orden de detección.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gestor {
    Apt,
    Dnf,
    Zypper,
    Pacman,
}

#[cfg(any(target_os = "linux", test))]
impl Gestor {
    fn binario(self) -> &'static str {
        match self {
            Gestor::Apt => "apt-get",
            Gestor::Dnf => "dnf",
            Gestor::Zypper => "zypper",
            Gestor::Pacman => "pacman",
        }
    }

    /// Columna de la tabla de paquetes: apt, dnf, pacman, zypper.
    fn columna(self) -> usize {
        match self {
            Gestor::Apt => 0,
            Gestor::Dnf => 1,
            Gestor::Pacman => 2,
            Gestor::Zypper => 3,
        }
    }
}

/// Tabla soname -> nombre de paquete por gestor (apt, dnf, pacman, zypper).
#[cfg(any(target_os = "linux", test))]
const TABLA_PAQUETES: &[(&str, [&str; 4])] = &[
    ("libwayland-client.so.0", ["libwayland-client0", "libwayland-client", "wayland", "libwayland-client0"]),
    ("libxkbcommon.so.0", ["libxkbcommon0", "libxkbcommon", "libxkbcommon", "libxkbcommon0"]),
    ("libxkbcommon-x11.so.0", ["libxkbcommon-x11-0", "libxkbcommon-x11", "libxkbcommon-x11", "libxkbcommon-x11-0"]),
    ("libX11.so.6", ["libx11-6", "libX11", "libx11", "libX11-6"]),
    ("libX11-xcb.so.1", ["libx11-xcb1", "libX11-xcb", "libx11", "libX11-xcb1"]),
    ("libXcursor.so.1", ["libxcursor1", "libXcursor", "libxcursor", "libXcursor1"]),
    ("libXi.so.6", ["libxi6", "libXi", "libxi", "libXi6"]),
];

/// Nombres de paquete (sin duplicados, en orden de aparición) para las librerías indicadas.
#[cfg(any(target_os = "linux", test))]
fn paquetes_para(sonames: &[&str], gestor: Gestor) -> Vec<&'static str> {
    let mut paquetes: Vec<&'static str> = Vec::new();
    for soname in sonames {
        if let Some((_, nombres)) = TABLA_PAQUETES.iter().find(|(s, _)| s == soname) {
            let nombre = nombres[gestor.columna()];
            if !paquetes.contains(&nombre) {
                paquetes.push(nombre);
            }
        }
    }
    paquetes
}

/// Argumentos para instalar `paquetes` con el gestor indicado (sin el nombre del binario).
#[cfg(any(target_os = "linux", test))]
fn argumentos_instalacion(gestor: Gestor, paquetes: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = match gestor {
        Gestor::Apt | Gestor::Dnf => vec!["install".into(), "-y".into()],
        Gestor::Zypper => vec!["--non-interactive".into(), "install".into()],
        Gestor::Pacman => vec!["-S".into(), "--noconfirm".into(), "--needed".into()],
    };
    args.extend(paquetes.iter().map(|p| p.to_string()));
    args
}

/// Indica si la variable de entorno existe y no está vacía.
#[cfg(target_os = "linux")]
fn env_presente(nombre: &str) -> bool {
    std::env::var(nombre).map(|v| !v.is_empty()).unwrap_or(false)
}

/// Indica si el ejecutable `bin` existe en alguna carpeta de `PATH`.
#[cfg(target_os = "linux")]
fn en_path(bin: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| dir.join(bin).is_file())
}

/// Primer gestor de paquetes encontrado en `PATH`.
#[cfg(target_os = "linux")]
fn gestor_detectado() -> Option<Gestor> {
    [Gestor::Apt, Gestor::Dnf, Gestor::Zypper, Gestor::Pacman]
        .into_iter()
        .find(|g| en_path(g.binario()))
}

/// Intenta cargar la librería dinámica; se descarta inmediatamente después.
#[cfg(target_os = "linux")]
fn biblioteca_disponible(soname: &str) -> bool {
    unsafe { libloading::Library::new(soname) }.is_ok()
}

/// Pregunta al usuario con zenity o kdialog. Devuelve `true` si respondió que sí.
#[cfg(target_os = "linux")]
fn confirmar_instalacion(mensaje: &str) -> bool {
    use std::process::Command;

    let resultado = if en_path("zenity") {
        Command::new("zenity")
            .args(["--question", "--title=DocuUpload", &format!("--text={mensaje}")])
            .status()
    } else {
        Command::new("kdialog")
            .args(["--title", "DocuUpload", "--yesno", mensaje])
            .status()
    };
    resultado.map(|s| s.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonames_wayland_tienen_prioridad_sobre_x11() {
        assert_eq!(
            sonames_requeridos(true, true),
            vec!["libwayland-client.so.0", "libxkbcommon.so.0"]
        );
    }

    #[test]
    fn sonames_x11_sin_wayland() {
        let s = sonames_requeridos(false, true);
        assert_eq!(s.len(), 6);
        assert!(s.contains(&"libX11.so.6"));
        assert!(s.contains(&"libxkbcommon-x11.so.0"));
    }

    #[test]
    fn sin_servidor_grafico_no_se_requiere_nada() {
        assert!(sonames_requeridos(false, false).is_empty());
    }

    #[test]
    fn paquetes_apt_y_dnf() {
        let x11 = sonames_requeridos(false, true);
        assert_eq!(
            paquetes_para(&x11, Gestor::Apt),
            vec!["libx11-6", "libx11-xcb1", "libxcursor1", "libxi6", "libxkbcommon0", "libxkbcommon-x11-0"]
        );
        assert_eq!(
            paquetes_para(&x11, Gestor::Dnf),
            vec!["libX11", "libX11-xcb", "libXcursor", "libXi", "libxkbcommon", "libxkbcommon-x11"]
        );
    }

    #[test]
    fn paquetes_pacman_sin_duplicados() {
        // libX11 y libX11-xcb comparten el paquete "libx11" en pacman
        let x11 = sonames_requeridos(false, true);
        let paquetes = paquetes_para(&x11, Gestor::Pacman);
        assert_eq!(paquetes.iter().filter(|p| **p == "libx11").count(), 1);
        assert!(!paquetes.contains(&"wayland"));
        assert_eq!(paquetes.len(), 5);
    }

    #[test]
    fn paquetes_wayland_zypper() {
        let wl = sonames_requeridos(true, false);
        assert_eq!(
            paquetes_para(&wl, Gestor::Zypper),
            vec!["libwayland-client0", "libxkbcommon0"]
        );
        assert_eq!(paquetes_para(&wl, Gestor::Pacman), vec!["wayland", "libxkbcommon"]);
    }

    #[test]
    fn soname_desconocido_se_ignora() {
        assert!(paquetes_para(&["libdesconocida.so.1"], Gestor::Apt).is_empty());
    }

    #[test]
    fn argumentos_por_gestor() {
        assert_eq!(argumentos_instalacion(Gestor::Apt, &["a", "b"]), vec!["install", "-y", "a", "b"]);
        assert_eq!(argumentos_instalacion(Gestor::Dnf, &["a"]), vec!["install", "-y", "a"]);
        assert_eq!(
            argumentos_instalacion(Gestor::Zypper, &["a"]),
            vec!["--non-interactive", "install", "a"]
        );
        assert_eq!(
            argumentos_instalacion(Gestor::Pacman, &["a"]),
            vec!["-S", "--noconfirm", "--needed", "a"]
        );
    }

    #[test]
    fn binarios_por_gestor() {
        assert_eq!(Gestor::Apt.binario(), "apt-get");
        assert_eq!(Gestor::Pacman.binario(), "pacman");
    }
}
