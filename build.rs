fn main() {
    slint_build::compile("ui/main_view.slint").unwrap();

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

