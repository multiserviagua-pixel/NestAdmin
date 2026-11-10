//! NIF en mayúsculas en la empresa, los clientes y la copia de la factura (D220), visto en
//! `tauri dev`: la empresa guardada con «b12345674» salía así en la factura mientras su registro
//! Verifactu llevaba «B12345674». Prueba independiente (D197). Solo datos sintéticos.

use nestadmin_core::sales::{Company, CustomerData, DraftInput, DraftLine, VerifactuContext};
use nestadmin_core::Store;
use nestadmin_documents::IsoDate;
use nestadmin_sales::{DocumentKind, LineInput, TaxRate};
use nestadmin_verifactu::{Fecha, FechaHoraHuso, Persona, SistemaInformatico};
use rust_decimal::Decimal;

fn date(text: &str) -> IsoDate {
    IsoDate::parse("test", text).unwrap()
}

fn ctx() -> VerifactuContext {
    VerifactuContext {
        system: SistemaInformatico {
            productor: Persona::con_nif("Productor Demo SL", "B00000034"),
            nombre_sistema: "NestAdmin".into(),
            id_sistema: "NA".into(),
            version: "0.1.0".into(),
            numero_instalacion: "INST-0001".into(),
            solo_verifactu: true,
            multi_ot: false,
            indicador_multiples_ot: false,
        },
        now: FechaHoraHuso::new(Fecha::new(2026, 3, 1).unwrap(), 10, 0, 0, 60).unwrap(),
    }
}

fn company(nif: &str) -> Company {
    Company {
        name: Some("Taller Demo SL".into()),
        tax_id: Some(nif.into()),
        address: Some("Calle Ficticia 1, 28000 Madrid".into()),
        email: None,
        iban: None,
        logo: None,
        logo_mime: None,
    }
}

#[test]
fn el_nif_se_guarda_y_se_copia_en_mayusculas() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("n.db")).unwrap();
    store.set_sales_company(&company(" b-12345674 ")).unwrap();
    assert_eq!(
        store.sales_company().unwrap().unwrap().tax_id.as_deref(),
        Some("B12345674")
    );
    let customer = store
        .create_customer(&CustomerData {
            name: "Cliente Uno SA".into(),
            tax_id: Some("12345678z".into()),
            address: Some("Avenida Inventada 2, 08000 Barcelona".into()),
            email: None,
        })
        .unwrap();
    assert_eq!(
        store.customer(customer).unwrap().data.tax_id.as_deref(),
        Some("12345678Z")
    );

    let mut input = DraftInput::new(DocumentKind::Invoice);
    input.customer_id = Some(customer);
    input.lines = vec![DraftLine::from(LineInput {
        description: "Servicio de montaje".into(),
        quantity: Decimal::ONE,
        unit_price: Decimal::new(100, 0),
        discount: Decimal::ZERO,
        tax_rate: TaxRate::TwentyOne,
        exemption: None,
    })];
    let id = store.create_sales_draft(&input).unwrap();
    store
        .issue_sales_invoice_registered(id, date("2026-03-01"), date("2026-03-01"), &ctx())
        .unwrap();
    let doc = store.sales_document(id).unwrap();
    assert_eq!(
        doc.issuer.unwrap().party.tax_id.as_deref(),
        Some("B12345674"),
        "la factura muestra el mismo NIF que su registro"
    );
    assert_eq!(
        store.verifactu_records().unwrap()[0].issuer_tax_id,
        "B12345674"
    );

    // Con registros, reescribir el mismo NIF en minúsculas no cuenta como cambio.
    store.set_sales_company(&company("b12345674")).unwrap();
}
