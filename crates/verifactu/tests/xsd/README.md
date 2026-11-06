# XSD oficiales para las pruebas

Las pruebas `#[ignore]` de `tests/xml.rs` validan el XML generado (envío) y la respuesta del
simulador contra los esquemas oficiales. Los XSD de la AEAT no se versionan: el apartado
«Propiedad intelectual» de su aviso legal reserva la reproducción y la distribución. Para
ejecutarlas, en Windows:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File crates/verifactu/tests/xsd/obtener.ps1
cargo test -p nestadmin-verifactu -- --ignored
```

`obtener.ps1` descarga `SuministroLR.xsd`, `SuministroInformacion.xsd`,
`RespuestaSuministro.xsd` (AEAT) y `xmldsig-core-schema.xsd` (W3C) en esta carpeta y compara
su SHA-256 con el de `FUENTES.md`. Si difiere, avisa y sale con código 2: la AEAT ha publicado
otra versión y hay que revisar el crate. `validar.ps1` valida con `System.Xml` de .NET, sin
red (los `import` se resuelven con los archivos de esta carpeta). `.gitignore` evita subir los
`.xsd` por error.
