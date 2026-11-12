use serde::{Deserialize, Serialize};

/// De dónde sale el certificado de remisión de un obligado (D278). Se serializa en JSON para
/// guardarlo en la base: no lleva secretos, solo la huella pública o una referencia opaca.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CertificateSource {
    /// Un certificado del almacén de Windows, identificado por la huella SHA-256 de su DER
    /// (hexadecimal; mayúsculas o minúsculas).
    Store { thumbprint_sha256: String },
    /// Un PFX custodiado en un archivo cifrado con DPAPI; `reference` es un identificador
    /// opaco y aleatorio (32 caracteres hexadecimales) que devuelve `import_pfx`.
    Pfx { reference: String },
}
