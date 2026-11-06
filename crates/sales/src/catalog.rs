//! Catálogo de productos y servicios (R20, D79).
//!
//! D79: producto o servicio con descripción, precio, tipo de IVA por defecto y causa de
//! exención si procede. **Una línea copia esos valores al añadirse** y se puede cambiar;
//! modificar o retirar un producto no altera ningún documento. Sin existencias (D91).
//!
//! La copia es por valor: [`Product::to_line`] devuelve un [`LineInput`] propio, sin vínculo
//! con el producto; la persistencia (migración 3, D72) guardará la línea sin clave obligatoria
//! al catálogo.
//!
//! Interpretaciones conservadoras:
//! * «Retirar» marca el producto como retirado: no se puede añadir a líneas nuevas, pero
//!   sigue existiendo (los documentos emitidos no cambian).
//! * El precio es sin IVA (como `LineInput::unit_price`) y no negativo.
//! * Las reglas de exención son las de `calc`: 0 % exige causa; otro tipo la prohíbe.

use rust_decimal::Decimal;

use crate::calc::{Exemption, LineInput, TaxQualification, TaxRate};

#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    pub id: u64,
    pub description: String,
    pub unit_price: Decimal,
    pub tax_rate: TaxRate,
    pub exemption: Option<Exemption>,
    pub retired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error("falta la descripción del producto")]
    EmptyDescription,
    #[error("el precio no puede ser negativo")]
    NegativePrice,
    #[error("el tipo 0 % exige la causa de exención o no sujeción")]
    ExemptionRequired,
    #[error("la causa de exención solo procede con el tipo 0 %")]
    ExemptionNotAllowed,
    #[error("la cantidad debe ser positiva")]
    NonPositiveQuantity,
    #[error("el descuento debe estar entre 0 y 1")]
    DiscountOutOfRange,
    #[error("el producto {0} está retirado")]
    Retired(u64),
}

impl Product {
    /// Valida los datos del producto (se llama al crearlo o modificarlo).
    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.description.trim().is_empty() {
            return Err(CatalogError::EmptyDescription);
        }
        if self.unit_price < Decimal::ZERO {
            return Err(CatalogError::NegativePrice);
        }
        match (&self.exemption, self.tax_rate) {
            (None, TaxRate::Zero) => Err(CatalogError::ExemptionRequired),
            (Some(e), TaxRate::Zero) if e.qualification == TaxQualification::S1 => {
                Err(CatalogError::ExemptionRequired)
            }
            (Some(_), r) if r != TaxRate::Zero => Err(CatalogError::ExemptionNotAllowed),
            _ => Ok(()),
        }
    }

    pub fn retire(&mut self) {
        self.retired = true;
    }

    /// Crea una línea copiando descripción, precio, tipo de IVA y exención (D79). La línea es
    /// independiente: el usuario puede cambiar cualquier campo y el producto no la afecta.
    pub fn to_line(&self, quantity: Decimal, discount: Decimal) -> Result<LineInput, CatalogError> {
        if self.retired {
            return Err(CatalogError::Retired(self.id));
        }
        self.validate()?;
        if quantity <= Decimal::ZERO {
            return Err(CatalogError::NonPositiveQuantity);
        }
        if discount < Decimal::ZERO || discount > Decimal::ONE {
            return Err(CatalogError::DiscountOutOfRange);
        }
        Ok(LineInput {
            description: self.description.clone(),
            quantity,
            unit_price: self.unit_price,
            discount,
            tax_rate: self.tax_rate,
            exemption: self.exemption.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::{calculate, LineMode};
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn product() -> Product {
        Product {
            id: 7,
            description: "Mantenimiento sintético".into(),
            unit_price: dec("50.00"),
            tax_rate: TaxRate::TwentyOne,
            exemption: None,
            retired: false,
        }
    }

    #[test]
    fn d79_line_copies_product_values() {
        let line = product().to_line(dec("2"), dec("0.1")).unwrap();
        assert_eq!(line.description, "Mantenimiento sintético");
        assert_eq!(line.unit_price, dec("50.00"));
        assert_eq!(line.tax_rate, TaxRate::TwentyOne);
        let t = calculate(&[line], None, LineMode::Standard).unwrap();
        assert_eq!(t.base_total.to_string(), "90.00");
        assert_eq!(t.total.to_string(), "108.90");
    }

    #[test]
    fn d79_changing_catalog_does_not_alter_existing_lines() {
        let mut p = product();
        let line = p.to_line(dec("1"), Decimal::ZERO).unwrap();
        let before = calculate(std::slice::from_ref(&line), None, LineMode::Standard).unwrap();
        p.unit_price = dec("99.00");
        p.description = "Otro".into();
        p.tax_rate = TaxRate::Ten;
        let after = calculate(std::slice::from_ref(&line), None, LineMode::Standard).unwrap();
        assert_eq!(before, after);
        assert_eq!(line.unit_price, dec("50.00"));
    }

    #[test]
    fn d79_retiring_does_not_alter_lines_but_blocks_new_ones() {
        let mut p = product();
        let line = p.to_line(dec("1"), Decimal::ZERO).unwrap();
        p.retire();
        assert_eq!(line.description, "Mantenimiento sintético");
        assert_eq!(
            p.to_line(dec("1"), Decimal::ZERO).unwrap_err(),
            CatalogError::Retired(7)
        );
    }

    #[test]
    fn d79_line_can_be_edited_after_copy() {
        let p = product();
        let mut line = p.to_line(dec("1"), Decimal::ZERO).unwrap();
        line.unit_price = dec("45.00");
        assert_eq!(p.unit_price, dec("50.00"));
    }

    #[test]
    fn d79_exempt_product_carries_cause() {
        let p = Product {
            tax_rate: TaxRate::Zero,
            exemption: Some(Exemption {
                qualification: TaxQualification::E1,
                description: Some("Exenta (causa sintética)".into()),
            }),
            ..product()
        };
        let line = p.to_line(dec("1"), Decimal::ZERO).unwrap();
        assert_eq!(
            line.exemption.as_ref().unwrap().description.as_deref(),
            Some("Exenta (causa sintética)")
        );
        assert!(calculate(&[line], None, LineMode::Standard).is_ok());
    }

    #[test]
    fn d79_validation_errors() {
        let mut p = product();
        p.description = "  ".into();
        assert_eq!(p.validate().unwrap_err(), CatalogError::EmptyDescription);
        let mut p = product();
        p.unit_price = dec("-1");
        assert_eq!(p.validate().unwrap_err(), CatalogError::NegativePrice);
        let mut p = product();
        p.tax_rate = TaxRate::Zero;
        assert_eq!(p.validate().unwrap_err(), CatalogError::ExemptionRequired);
        p.exemption = Some(Exemption::new(TaxQualification::S1));
        assert_eq!(p.validate().unwrap_err(), CatalogError::ExemptionRequired);
        let mut p = product();
        p.exemption = Some(Exemption::new(TaxQualification::E1));
        assert_eq!(p.validate().unwrap_err(), CatalogError::ExemptionNotAllowed);
    }

    #[test]
    fn d79_quantity_and_discount_checked() {
        let p = product();
        assert_eq!(
            p.to_line(Decimal::ZERO, Decimal::ZERO).unwrap_err(),
            CatalogError::NonPositiveQuantity
        );
        assert_eq!(
            p.to_line(dec("-1"), Decimal::ZERO).unwrap_err(),
            CatalogError::NonPositiveQuantity
        );
        assert_eq!(
            p.to_line(dec("1"), dec("1.01")).unwrap_err(),
            CatalogError::DiscountOutOfRange
        );
        assert_eq!(
            p.to_line(dec("1"), dec("-0.1")).unwrap_err(),
            CatalogError::DiscountOutOfRange
        );
        assert!(p.to_line(dec("1"), Decimal::ONE).is_ok());
    }
}
