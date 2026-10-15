Name:           docu_ffacd
Version:        0.5.0
Release:        1%{?dist}
Summary:        DocuUpload - Procesamiento concurrente de nóminas SAT y cédulas SUA IMSS
License:        MIT OR Apache-2.0
URL:            https://github.com/Ivan395/docupload

Requires:       fontconfig
Requires:       freetype
Requires:       libX11
Requires:       libXext
Requires:       libXrender
Requires:       libxml2
Requires:       libpng
Requires:       zlib

%description
DocuUpload Fiscal & Nóminas ACD
Procesamiento concurrente y en paralelo de Nóminas CFDI con validación directa ante el WebService SOAP del SAT y Cédulas de Determinación de Cuotas del SUA IMSS con extracción de amortizaciones Infonavit y reportes en Excel.

%install
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/256x256/apps
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/64x64/apps
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/32x32/apps
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/scalable/apps

install -m 755 %{_sourcedir}/docu_ffacd %{buildroot}%{_bindir}/docu_ffacd
install -m 644 %{_sourcedir}/docu_ffacd.desktop %{buildroot}%{_datadir}/applications/docu_ffacd.desktop
install -m 644 %{_sourcedir}/app_icon_256.png %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/docu_ffacd.png
install -m 644 %{_sourcedir}/app_icon_64.png %{buildroot}%{_datadir}/icons/hicolor/64x64/apps/docu_ffacd.png
install -m 644 %{_sourcedir}/app_icon_32.png %{buildroot}%{_datadir}/icons/hicolor/32x32/apps/docu_ffacd.png
install -m 644 %{_sourcedir}/app_icon.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/docu_ffacd.svg

%files
%{_bindir}/docu_ffacd
%{_datadir}/applications/docu_ffacd.desktop
%{_datadir}/icons/hicolor/256x256/apps/docu_ffacd.png
%{_datadir}/icons/hicolor/64x64/apps/docu_ffacd.png
%{_datadir}/icons/hicolor/32x32/apps/docu_ffacd.png
%{_datadir}/icons/hicolor/scalable/apps/docu_ffacd.svg

%post
/bin/touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :
%{_bindir}/update-desktop-database &>/dev/null || :

%postun
if [ $1 -eq 0 ] ; then
    /bin/touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :
    %{_bindir}/gtk-update-icon-cache %{_datadir}/icons/hicolor &>/dev/null || :
    %{_bindir}/update-desktop-database &>/dev/null || :
fi

%changelog
* Sat Sep 26 2026 Firma Fiscal ACD <soporte@firmafiscalacd.com> - 0.5.0-1
- Versión 0.5.0 con procesamiento concurrente y en paralelo y soporte multiplataforma.
