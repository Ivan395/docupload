fn main() {
    // Estilo fijo claro: evita que los widgets estándar sigan el modo oscuro del SO (Windows/macOS/GNOME/KDE)
    // y queden ilegibles sobre los fondos claros de la aplicación.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-light".into());
    slint_build::compile_with_config("ui/main_view.slint", config).unwrap();

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("ui/assets/app_icon.ico");
        res.set("FileDescription", "DocUpload - Procesamiento de Nóminas SAT y Cédulas SUA");
        res.set("ProductName", "DocUpload");
        res.set("ProductVersion", "0.5.0.0");
        res.set("FileVersion", "0.5.0.0");
        res.set("LegalCopyright", "Copyright © 2026 Firma Fiscal ACD");
        let _ = res.compile().unwrap();
    }
}

