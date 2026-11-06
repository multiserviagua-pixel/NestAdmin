//! Validación de NIF, NIE y CIF españoles con dígito de control (D80: la factura completa
//! exige el NIF de emisor y destinatario). Solo comprueba formato y control; no consulta
//! ningún censo. Un identificador extranjero (UE o tercer país) no se admite aquí: la
//! decisión solo cubre territorio común y NIF español, así que se rechaza como formato.

const DNI_LETTERS: &[u8; 23] = b"TRWAGMYFPDXBNJZSQVHLCKE";
const CIF_CONTROL_LETTERS: &[u8; 10] = b"JABCDEFGHI";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaxIdKind {
    /// Persona física con DNI: 8 cifras y letra.
    Nif,
    /// Extranjero: X, Y o Z, 7 cifras y letra.
    Nie,
    /// Entidad: letra, 7 cifras y control (cifra o letra según la clase).
    Cif,
    /// K, L o M (menores y residentes sin NIE): control con letra calculada como en el CIF.
    SpecialNif,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TaxIdError {
    #[error("el identificador fiscal está vacío")]
    Empty,
    #[error("el formato del identificador fiscal no es NIF, NIE ni CIF")]
    Format,
    #[error("el dígito de control del identificador fiscal no es correcto")]
    Control,
}

/// Normaliza (mayúsculas, sin espacios ni guiones) y valida.
pub fn validate_tax_id(raw: &str) -> Result<TaxIdKind, TaxIdError> {
    let text: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase();
    if text.is_empty() {
        return Err(TaxIdError::Empty);
    }
    if !text.is_ascii() || text.len() != 9 {
        return Err(TaxIdError::Format);
    }
    let b = text.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let number = |r: std::ops::Range<usize>| -> u32 {
        b[r].iter().fold(0, |a, d| a * 10 + u32::from(d - b'0'))
    };
    match b[0] {
        b'0'..=b'9' => {
            if !digits(0..8) || !b[8].is_ascii_uppercase() {
                return Err(TaxIdError::Format);
            }
            if DNI_LETTERS[(number(0..8) % 23) as usize] == b[8] {
                Ok(TaxIdKind::Nif)
            } else {
                Err(TaxIdError::Control)
            }
        }
        b'X' | b'Y' | b'Z' => {
            if !digits(1..8) || !b[8].is_ascii_uppercase() {
                return Err(TaxIdError::Format);
            }
            let prefix = u32::from(b[0] - b'X');
            let n = prefix * 10_000_000 + number(1..8);
            if DNI_LETTERS[(n % 23) as usize] == b[8] {
                Ok(TaxIdKind::Nie)
            } else {
                Err(TaxIdError::Control)
            }
        }
        b'K' | b'L' | b'M' => {
            if !digits(1..8) || !b[8].is_ascii_uppercase() {
                return Err(TaxIdError::Format);
            }
            if CIF_CONTROL_LETTERS[cif_control_digit(&b[1..8])] == b[8] {
                Ok(TaxIdKind::SpecialNif)
            } else {
                Err(TaxIdError::Control)
            }
        }
        b'A' | b'B' | b'C' | b'D' | b'E' | b'F' | b'G' | b'H' | b'J' | b'N' | b'P' | b'Q'
        | b'R' | b'S' | b'U' | b'V' | b'W' => {
            if !digits(1..8) || !b[8].is_ascii_alphanumeric() {
                return Err(TaxIdError::Format);
            }
            let digit = cif_control_digit(&b[1..8]);
            let as_digit = b[8] == b'0' + digit as u8;
            let as_letter = b[8] == CIF_CONTROL_LETTERS[digit];
            // Clases que exigen letra (P, Q, R, S, N, W) o cifra (A, B, E, H); el resto admite ambas.
            let ok = match b[0] {
                b'P' | b'Q' | b'R' | b'S' | b'N' | b'W' => as_letter,
                b'A' | b'B' | b'E' | b'H' => as_digit,
                _ => as_digit || as_letter,
            };
            if ok {
                Ok(TaxIdKind::Cif)
            } else {
                Err(TaxIdError::Control)
            }
        }
        _ => Err(TaxIdError::Format),
    }
}

/// Dígito de control del CIF sobre las 7 cifras centrales.
fn cif_control_digit(digits: &[u8]) -> usize {
    let mut sum = 0u32;
    for (i, d) in digits.iter().enumerate() {
        let v = u32::from(d - b'0');
        if i % 2 == 1 {
            sum += v;
        } else {
            let t = v * 2;
            sum += t / 10 + t % 10;
        }
    }
    ((10 - sum % 10) % 10) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    // Identificadores sintéticos: calculados con el algoritmo, no pertenecen a nadie conocido.
    #[test]
    fn d80_valid_ids_of_every_class() {
        assert_eq!(validate_tax_id("12345678Z"), Ok(TaxIdKind::Nif));
        assert_eq!(validate_tax_id("87654321X"), Ok(TaxIdKind::Nif));
        assert_eq!(validate_tax_id("X1234567L"), Ok(TaxIdKind::Nie));
        assert_eq!(validate_tax_id("Y1234567X"), Ok(TaxIdKind::Nie));
        assert_eq!(validate_tax_id("Z1234567R"), Ok(TaxIdKind::Nie));
        assert_eq!(validate_tax_id("A12345674"), Ok(TaxIdKind::Cif));
        assert_eq!(validate_tax_id("B12345674"), Ok(TaxIdKind::Cif));
        assert_eq!(validate_tax_id("P1234567D"), Ok(TaxIdKind::Cif));
        assert_eq!(validate_tax_id("Q1234567D"), Ok(TaxIdKind::Cif));
    }

    #[test]
    fn d80_normalizes_case_spaces_and_hyphens() {
        assert_eq!(validate_tax_id(" 12345678-z "), Ok(TaxIdKind::Nif));
        assert_eq!(validate_tax_id("x 1234567 l"), Ok(TaxIdKind::Nie));
    }

    #[test]
    fn d80_wrong_control_is_rejected() {
        assert_eq!(validate_tax_id("12345678A"), Err(TaxIdError::Control));
        assert_eq!(validate_tax_id("X1234567A"), Err(TaxIdError::Control));
        assert_eq!(validate_tax_id("A12345675"), Err(TaxIdError::Control));
        assert_eq!(validate_tax_id("B12345675"), Err(TaxIdError::Control));
    }

    #[test]
    fn d80_cif_class_requires_digit_or_letter() {
        // A y B exigen cifra; P y Q exigen letra.
        assert_eq!(validate_tax_id("A1234567D"), Err(TaxIdError::Control));
        assert_eq!(validate_tax_id("P12345674"), Err(TaxIdError::Control));
        assert_eq!(validate_tax_id("Q12345674"), Err(TaxIdError::Control));
        // C admite ambas.
        assert_eq!(validate_tax_id("C12345674"), Ok(TaxIdKind::Cif));
        assert_eq!(validate_tax_id("C1234567D"), Ok(TaxIdKind::Cif));
    }

    #[test]
    fn d80_special_nif_k_l_m_uses_control_letter() {
        assert_eq!(validate_tax_id("K1234567D"), Ok(TaxIdKind::SpecialNif));
        assert_eq!(validate_tax_id("L1234567D"), Ok(TaxIdKind::SpecialNif));
        assert_eq!(validate_tax_id("M1234567D"), Ok(TaxIdKind::SpecialNif));
        assert_eq!(validate_tax_id("K12345674"), Err(TaxIdError::Format));
        assert_eq!(validate_tax_id("K1234567A"), Err(TaxIdError::Control));
    }

    #[test]
    fn d80_format_errors() {
        assert_eq!(validate_tax_id(""), Err(TaxIdError::Empty));
        assert_eq!(validate_tax_id("  - "), Err(TaxIdError::Empty));
        assert_eq!(validate_tax_id("1234567Z"), Err(TaxIdError::Format));
        assert_eq!(validate_tax_id("123456789"), Err(TaxIdError::Format));
        assert_eq!(validate_tax_id("12345678"), Err(TaxIdError::Format));
        assert_eq!(validate_tax_id("T12345678"), Err(TaxIdError::Format));
        assert_eq!(validate_tax_id("X12345678"), Err(TaxIdError::Format));
        assert_eq!(validate_tax_id("ÑÑÑÑÑÑÑÑÑ"), Err(TaxIdError::Format));
        assert_eq!(validate_tax_id("1234567ZZ"), Err(TaxIdError::Format));
    }
}
