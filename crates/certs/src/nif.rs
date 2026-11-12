use nestadmin_sales::validate_tax_id;

/// Mayúsculas, sin espacios ni guiones.
pub fn normalize_tax_id(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase()
}

/// NIF, NIE o CIF español con control correcto (reutiliza `nestadmin_sales`, D80).
pub fn valid_tax_id(raw: &str) -> bool {
    validate_tax_id(raw).is_ok()
}

/// Extrae un NIF de un `serialNumber` (`IDCES-<NIF>` o NIF sin prefijo) o de un
/// `organizationIdentifier` (`VATES-<NIF>`). Devuelve `None` si no es un NIF español válido.
pub(crate) fn extract_nif(raw: &str) -> Option<String> {
    let text = normalize_tax_id(raw);
    let text = ["IDCES", "VATES"]
        .iter()
        .find_map(|p| text.strip_prefix(p))
        .unwrap_or(&text);
    valid_tax_id(text).then(|| text.to_string())
}
