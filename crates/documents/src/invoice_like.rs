//! Comprobación local de «parece una factura» (E5, D133). Se aplica al texto que el equipo ya
//! extrajo de un documento, **sin IA y antes de enviar nada a un proveedor**: la primera lectura
//! solo envía los documentos que la superan. No valida nada de la factura (importes, NIF…):
//! solo decide si merece que el modelo la lea. Ante la duda, el error prudente es **no enviar**.
//!
//! Heurística, deliberadamente sencilla y conservadora:
//! 1. Un documento con rasgos de **nómina** («nómina», «recibo de salarios», «líquido a
//!    percibir», «total devengado»), de **contrato** («contrato de», «cláusula», «las partes
//!    acuerdan») o de **reclamación** («reclamación», «queja», «requerimiento») en su cabecera (los
//!    primeros 600 caracteres) **no** pasa, aunque lleve NIF, «base imponible» o la palabra
//!    «factura», salvo que el documento se encabece como factura (una de sus tres primeras líneas
//!    empieza por «factura» o «invoice») y tenga además un **número de factura** y un **importe**.
//! 2. Si no, parece una factura si lleva en la cabecera «factura» o «invoice» (como palabra
//!    entera), o un rótulo de número de factura seguido de un número, y reúne al menos 3 de
//!    estas 5 señales: un importe con dos decimales, «base imponible» o «subtotal», «IVA»,
//!    «total» y un NIF/CIF/VAT.
//! 3. Un **rótulo de número** es «factura nº», «número de factura», «invoice no»… seguido de un
//!    token con alguna cifra: «factura normal» o «invoice not» no lo son.
//!
//! Se buscan sin distinguir mayúsculas ni tildes y se reconocen «N.I.F.», «C.I.F.» e «I.V.A.».

/// Motivo por el que un documento no se envía.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotInvoiceLike {
    /// Sin «factura»/«invoice» en la cabecera ni rótulo de número de factura.
    NoInvoiceTerm,
    /// Menos de 3 señales de factura.
    FewSignals,
    /// Rasgos de nómina.
    Payslip,
    /// Rasgos de contrato.
    Contract,
    /// Rasgos de reclamación.
    Claim,
}

impl NotInvoiceLike {
    pub fn code(self) -> &'static str {
        match self {
            Self::NoInvoiceTerm => "no_invoice_term",
            Self::FewSignals => "few_signals",
            Self::Payslip => "payslip",
            Self::Contract => "contract",
            Self::Claim => "claim",
        }
    }
}

/// Caracteres de la cabecera que se miran.
const HEADER_CHARS: usize = 600;
/// Señales mínimas de factura.
const MIN_SIGNALS: usize = 3;

fn fold(text: &str) -> String {
    let lowered: String = text
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'Á' => 'a',
            'é' | 'è' | 'ë' | 'É' => 'e',
            'í' | 'ì' | 'ï' | 'Í' => 'i',
            'ó' | 'ò' | 'ö' | 'Ó' => 'o',
            'ú' | 'ù' | 'ü' | 'Ú' => 'u',
            other => other.to_lowercase().next().unwrap_or(other),
        })
        .collect();
    lowered
        .replace("n.i.f.", "nif")
        .replace("n.i.f", "nif")
        .replace("c.i.f.", "cif")
        .replace("c.i.f", "cif")
        .replace("i.v.a.", "iva")
        .replace("i.v.a", "iva")
}

/// Hay un importe con dos decimales (`121,00`, `1.210,00`, `121.00`).
fn has_amount(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        if (chars[i] == ',' || chars[i] == '.')
            && i > 0
            && chars[i - 1].is_ascii_digit()
            && chars.get(i + 1).is_some_and(char::is_ascii_digit)
            && chars.get(i + 2).is_some_and(char::is_ascii_digit)
            && !chars.get(i + 3).is_some_and(char::is_ascii_digit)
        {
            return true;
        }
    }
    false
}

fn has_word(text: &str, word: &str) -> bool {
    let mut from = 0;
    while let Some(found) = text[from..].find(word) {
        let start = from + found;
        let end = start + word.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        if !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric) {
            return true;
        }
        from = end;
    }
    false
}

/// Tras `rest` (lo que sigue a una etiqueta), un token con alguna cifra: `F-2026-0042`, `77`.
fn digit_token_follows(rest: &str) -> bool {
    let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || ":.º°#-".contains(c));
    let token: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '/' || *c == '_')
        .collect();
    if !token.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    // «factura 30 días», «factura 15 %»: un plazo o un porcentaje, no un número de factura.
    if token.chars().all(|c| c.is_ascii_digit()) {
        let after = rest[token.len()..].trim_start();
        if after.starts_with('%') {
            return false;
        }
        let word: String = after.chars().take_while(|c| c.is_alphabetic()).collect();
        if [
            "dia", "dias", "mes", "meses", "semana", "semanas", "hora", "horas", "ano", "anos",
            "euro", "euros", "eur", "day", "days", "month", "months", "week", "weeks", "hour",
            "hours", "year", "years",
        ]
        .contains(&word.as_str())
        {
            return false;
        }
    }
    true
}

/// El documento se encabeza como factura: una de sus tres primeras líneas con texto empieza por
/// «factura» o «invoice» (como palabra entera).
fn headed_as_invoice(folded: &str) -> bool {
    folded
        .lines()
        .map(|l| l.trim_start_matches(|c: char| !c.is_alphanumeric()))
        .filter(|l| !l.is_empty())
        .take(3)
        .any(|l| {
            let first: String = l.chars().take_while(|c| c.is_alphanumeric()).collect();
            first == "factura" || first == "invoice"
        })
}

/// Rótulo de número de factura seguido de un número: «factura nº 12», «invoice no. 77», pero no
/// «factura normal» ni «invoice not».
fn has_number_label(text: &str) -> bool {
    [
        "factura n",
        "factura no",
        "factura num",
        "factura numero",
        "numero de factura",
        "n de factura",
        "no. de factura",
        "invoice no",
        "invoice number",
        "invoice #",
        "invoice n",
    ]
    .iter()
    .any(|label| {
        let mut from = 0;
        while let Some(found) = text[from..].find(label) {
            let start = from + found;
            let end = start + label.len();
            let before_ok = !text[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
            if before_ok && digit_token_follows(&text[end..]) {
                return true;
            }
            from = end;
        }
        false
    })
}

/// Un número de factura: el rótulo, o «factura»/«invoice» en la cabecera seguida de un número.
fn has_invoice_number(folded: &str, header: &str) -> bool {
    if has_number_label(folded) {
        return true;
    }
    ["factura", "invoice"].iter().any(|word| {
        let mut from = 0;
        while let Some(found) = header[from..].find(word) {
            let start = from + found;
            let end = start + word.len();
            let before_ok = !header[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
            let after_ok = !header[end..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);
            if before_ok && after_ok && digit_token_follows(&header[end..]) {
                return true;
            }
            from = end;
        }
        false
    })
}

/// `Ok(())` si el texto parece una factura; si no, el motivo.
pub fn looks_like_invoice(text: &str) -> Result<(), NotInvoiceLike> {
    let folded = fold(text);
    let header: String = folded.chars().take(HEADER_CHARS).collect();
    let amount = has_amount(&folded);
    // Un documento de otro tipo solo pasa si se encabeza como factura (primeras líneas) y además
    // tiene número de factura e importe: un NIF, una «base imponible» o una factura citada en
    // una reclamación o un contrato no bastan.
    let invoice_proof =
        amount && headed_as_invoice(&folded) && has_invoice_number(&folded, &header);
    let marked = |terms: &[&str]| terms.iter().any(|t| header.contains(t));
    if marked(&[
        "nomina",
        "recibo de salarios",
        "liquido a percibir",
        "total devengado",
    ]) && !invoice_proof
    {
        return Err(NotInvoiceLike::Payslip);
    }
    if marked(&["contrato de", "clausula", "las partes acuerdan", "reunidos"]) && !invoice_proof {
        return Err(NotInvoiceLike::Contract);
    }
    if marked(&["reclamacion", "queja", "requerimiento"]) && !invoice_proof {
        return Err(NotInvoiceLike::Claim);
    }
    let term =
        has_word(&header, "factura") || has_word(&header, "invoice") || has_number_label(&folded);
    if !term {
        return Err(NotInvoiceLike::NoInvoiceTerm);
    }
    let signals = [
        amount,
        folded.contains("base imponible") || has_word(&folded, "subtotal"),
        has_word(&folded, "iva") || has_word(&folded, "vat"),
        has_word(&folded, "total"),
        has_word(&folded, "nif") || has_word(&folded, "cif"),
    ]
    .iter()
    .filter(|s| **s)
    .count();
    if signals < MIN_SIGNALS {
        return Err(NotInvoiceLike::FewSignals);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVOICE: &str =
        "FACTURA F-2026-0042\nProveedor Alfa Sintética SL\nNIF B00000000\nFecha: 2026-10-05\n\
        Base imponible: 100,00 EUR\nIVA 21%: 21,00 EUR\nTotal factura: 121,00 EUR\n";

    #[test]
    fn a_synthetic_invoice_passes_in_spanish_and_english() {
        assert_eq!(looks_like_invoice(INVOICE), Ok(()));
        let english = "Invoice No. 77\nAcme Synthetic Ltd\nVAT ID GB000000000\nSubtotal 100.00\nVAT 20.00\nTotal 120.00";
        assert_eq!(looks_like_invoice(english), Ok(()));
        assert_eq!(looks_like_invoice(&INVOICE.to_uppercase()), Ok(()));
    }

    #[test]
    fn a_bare_invoice_with_dotted_abbreviations_passes() {
        assert_eq!(
            looks_like_invoice("FACTURA / N.I.F. / I.V.A. / TOTAL 121,00"),
            Ok(())
        );
        assert_eq!(
            looks_like_invoice("Factura\nC.I.F. A00000000\nI.V.A. 21%\nTotal 242,00 EUR"),
            Ok(())
        );
    }

    #[test]
    fn a_synthetic_payslip_does_not_pass() {
        let payslip = "NÓMINA octubre 2026\nEmpresa Sintética SL\nTotal devengado: 2.000,00\nIRPF: 300,00\nLíquido a percibir: 1.700,00\nTotal 2.000,00 NIF 00000000T";
        assert_eq!(looks_like_invoice(payslip), Err(NotInvoiceLike::Payslip));
        let with_base = format!("{payslip}\nBase imponible 1.900,00");
        assert_eq!(looks_like_invoice(&with_base), Err(NotInvoiceLike::Payslip));
    }

    #[test]
    fn a_contract_or_a_claim_with_nif_or_taxable_base_but_no_invoice_number_does_not_pass() {
        let contract = "CONTRATO DE PRESTACIÓN DE SERVICIOS\nReunidos las partes...\nCláusula 4. El prestador emitirá una factura mensual de 1.000,00 euros más IVA. Total anual 12.000,00. NIF B00000000";
        assert_eq!(looks_like_invoice(contract), Err(NotInvoiceLike::Contract));
        let with_base = format!("{contract}\nBase imponible 12.000,00");
        assert_eq!(
            looks_like_invoice(&with_base),
            Err(NotInvoiceLike::Contract)
        );
        let claim = "RECLAMACIÓN\nSobre la factura del último trimestre, que no hemos recibido.\nNIF B00000000\nBase imponible aproximada 500,00. IVA. Total 605,00";
        assert_eq!(looks_like_invoice(claim), Err(NotInvoiceLike::Claim));
        let letter =
            "Estimado cliente:\nLe recordamos que recibirá una factura el mes próximo.\nUn saludo.";
        assert_eq!(looks_like_invoice(letter), Err(NotInvoiceLike::FewSignals));
    }

    #[test]
    fn a_contract_that_also_carries_an_invoice_number_and_amount_may_pass() {
        let text = "FACTURA F-2026-0042\nContrato de servicios nº 7, cláusula 3\nNIF B00000000\nBase imponible 100,00\nIVA 21,00\nTotal 121,00";
        assert_eq!(looks_like_invoice(text), Ok(()));
        let text = "CONTRATO DE SERVICIOS\nAnexo: FACTURA F-2026-0042\nNIF B00000000\nBase imponible 100,00\nIVA 21,00\nTotal 121,00";
        assert_eq!(looks_like_invoice(text), Err(NotInvoiceLike::Contract));
    }

    #[test]
    fn a_claim_or_contract_quoting_an_invoice_with_figures_does_not_pass() {
        let claim = "RECLAMACIÓN\nSobre la factura nº 12 de 605,00 euros.\nBase imponible 500,00\nIVA 105,00\nTotal 605,00\nNIF B00000000";
        assert_eq!(looks_like_invoice(claim), Err(NotInvoiceLike::Claim));
        let contract = "CONTRATO DE SERVICIOS\nSe emitirá la factura nº 12 por 1.000,00 euros.\nNIF B00000000\nIVA 210,00\nTotal 1.210,00";
        assert_eq!(looks_like_invoice(contract), Err(NotInvoiceLike::Contract));
    }

    #[test]
    fn a_term_in_days_is_not_an_invoice_number_and_no_dot_is_a_label() {
        assert!(!has_number_label("pago de la factura 30 dias"));
        assert!(!has_invoice_number("factura 30 dias", "factura 30 dias"));
        assert!(!has_invoice_number("factura 15 %", "factura 15 %"));
        assert!(has_invoice_number("factura 30", "factura 30"));
        assert!(has_number_label("factura no. 12"));
        assert!(!has_number_label("factura no pagada"));
        let text =
            "Factura no. 12\nProveedor Alfa Sintética SL\nNIF B00000000\nIVA 21,00\nTotal 121,00";
        assert_eq!(looks_like_invoice(text), Ok(()));
    }

    #[test]
    fn the_number_label_needs_a_number_and_word_boundaries() {
        assert!(has_number_label("factura n 12"));
        assert!(has_number_label("factura nº 12"));
        assert!(has_number_label("invoice no. 77"));
        assert!(has_number_label("numero de factura: f-2026-1"));
        assert!(!has_number_label("factura normal de servicios"));
        assert!(!has_number_label("invoice not received 2026"));
        assert!(!has_number_label("la factura numerosa"));
        assert!(!has_number_label("xfactura n 12"));
    }

    #[test]
    fn a_document_with_the_word_but_without_figures_does_not_pass() {
        assert_eq!(
            looks_like_invoice("Factura pendiente de recibir.\nGracias."),
            Err(NotInvoiceLike::FewSignals)
        );
        assert_eq!(looks_like_invoice(""), Err(NotInvoiceLike::NoInvoiceTerm));
        assert_eq!(
            looks_like_invoice("Lista de la compra: pan, leche"),
            Err(NotInvoiceLike::NoInvoiceTerm)
        );
        assert_eq!(
            looks_like_invoice("Factura normal de servicios. NIF B00000000. Gracias."),
            Err(NotInvoiceLike::FewSignals)
        );
    }

    #[test]
    fn the_word_inside_another_word_is_not_a_term() {
        assert!(!has_word("facturacion", "factura"));
        assert!(has_word("la factura.", "factura"));
        assert!(has_amount("total 1.210,00 eur") && !has_amount("12345") && !has_amount("1,5"));
    }
}
