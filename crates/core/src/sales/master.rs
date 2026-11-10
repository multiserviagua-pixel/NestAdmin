//! Datos maestros: empresa (D90), clientes, catálogo (D79) y albaranes recibidos (D74).
//! Modificar o retirar cualquiera de ellos no altera un documento emitido, que lleva su copia.

use nestadmin_documents::IsoDate;
use nestadmin_sales::{Exemption, Product, ReceivedStatus};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    audit, clean, clean_tax_id, dec, parse_date, plain, qualification_from, tax_rate_from,
    SalesResult, SalesStoreError,
};
use crate::store::{now_ms, Store};

/// Datos del único emisor de la instalación (D90).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Company {
    pub name: Option<String>,
    pub tax_id: Option<String>,
    pub address: Option<String>,
    pub email: Option<String>,
    pub iban: Option<String>,
    pub logo: Option<Vec<u8>>,
    pub logo_mime: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerData {
    pub name: String,
    pub tax_id: Option<String>,
    pub address: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Customer {
    pub id: i64,
    pub data: CustomerData,
    pub retired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedDeliveryNote {
    pub id: i64,
    pub supplier_name: String,
    pub supplier_tax_id: Option<String>,
    pub supplier_number: String,
    pub date: IsoDate,
    pub status: ReceivedStatus,
    pub document_ref: Option<String>,
    pub invoice_ref: Option<String>,
}

const LOGO_MIMES: [&str; 2] = ["image/png", "image/jpeg"];
/// Tope razonable del logotipo (D90); un PNG o JPEG de un membrete pesa mucho menos.
const MAX_LOGO_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn load_company(conn: &Connection) -> SalesResult<Option<Company>> {
    Ok(conn
        .query_row(
            "SELECT c.name, c.tax_id, c.address, c.email, c.iban, l.data, l.mime              FROM company c LEFT JOIN company_logos l ON l.sha256 = c.logo_sha256 WHERE c.id = 1",
            [],
            |r| {
                Ok(Company {
                    name: r.get(0)?,
                    tax_id: r.get(1)?,
                    address: r.get(2)?,
                    email: r.get(3)?,
                    iban: r.get(4)?,
                    logo: r.get(5)?,
                    logo_mime: r.get(6)?,
                })
            },
        )
        .optional()?)
}

/// SHA-256 (hexadecimal) del logotipo actual de la empresa, si lo hay.
pub(crate) fn company_logo_sha256(conn: &Connection) -> SalesResult<Option<String>> {
    Ok(conn
        .query_row("SELECT logo_sha256 FROM company WHERE id = 1", [], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(data)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

pub(crate) fn load_customer(conn: &Connection, id: i64) -> SalesResult<Customer> {
    conn.query_row(
        "SELECT id, name, tax_id, address, email, retired FROM customers WHERE id = ?1",
        [id],
        customer_row,
    )
    .optional()?
    .ok_or(SalesStoreError::not_found("cliente", id))
}

fn customer_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Customer> {
    Ok(Customer {
        id: r.get(0)?,
        data: CustomerData {
            name: r.get(1)?,
            tax_id: r.get(2)?,
            address: r.get(3)?,
            email: r.get(4)?,
        },
        retired: r.get::<_, i64>(5)? != 0,
    })
}

fn check_customer(data: &CustomerData) -> SalesResult<()> {
    if data.name.trim().is_empty() {
        return Err(SalesStoreError::invalid(
            "customer.name",
            "falta el nombre del cliente",
        ));
    }
    Ok(())
}

pub(crate) fn load_product(conn: &Connection, id: i64) -> SalesResult<Product> {
    let raw = conn
        .query_row(
            "SELECT description, unit_price, tax_rate, qualification, exemption_description, \
             retired FROM products WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or(SalesStoreError::not_found("producto", id))?;
    product_from(id, raw)
}

type ProductRaw = (String, String, String, Option<String>, Option<String>, i64);

fn product_from(id: i64, raw: ProductRaw) -> SalesResult<Product> {
    let (description, price, rate, qualification, exemption_description, retired) = raw;
    let exemption = match qualification {
        Some(q) => Some(Exemption {
            qualification: qualification_from(&q)?,
            description: exemption_description,
        }),
        None => None,
    };
    Ok(Product {
        id: id as u64,
        description,
        unit_price: dec("unit_price", &price)?,
        tax_rate: tax_rate_from(&rate)?,
        exemption,
        retired: retired != 0,
    })
}

type ReceivedRaw = (
    i64,
    String,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn received_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReceivedRaw> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
    ))
}

fn received_from(raw: ReceivedRaw) -> SalesResult<ReceivedDeliveryNote> {
    let (
        id,
        supplier_name,
        supplier_tax_id,
        supplier_number,
        date,
        status,
        document_ref,
        invoice_ref,
    ) = raw;
    Ok(ReceivedDeliveryNote {
        id,
        supplier_name,
        supplier_tax_id,
        supplier_number,
        date: parse_date("received.date", &date)?,
        status: match status.as_str() {
            "received" => ReceivedStatus::Received,
            "reviewed" => ReceivedStatus::Reviewed,
            other => {
                return Err(SalesStoreError::Corrupt(format!(
                    "estado de albarán recibido {other}"
                )))
            }
        },
        document_ref,
        invoice_ref,
    })
}

const RECEIVED_COLS: &str =
    "id, supplier_name, supplier_tax_id, supplier_number, date, status, document_ref, invoice_ref";

impl Store {
    // --- Empresa (D90) ---

    pub fn sales_company(&self) -> SalesResult<Option<Company>> {
        load_company(&self.conn)
    }

    /// Guarda los datos del emisor. Los documentos ya emitidos no cambian: llevan su copia.
    pub fn set_sales_company(&mut self, company: &Company) -> SalesResult<()> {
        if company.logo.is_some() != company.logo_mime.is_some() {
            return Err(SalesStoreError::invalid(
                "company.logo",
                "el logotipo y su tipo MIME van juntos",
            ));
        }
        if let (Some(logo), Some(mime)) = (&company.logo, &company.logo_mime) {
            if !LOGO_MIMES.contains(&mime.as_str()) {
                return Err(SalesStoreError::invalid(
                    "company.logo_mime",
                    "el logotipo debe ser PNG o JPEG",
                ));
            }
            if logo.is_empty() || logo.len() > MAX_LOGO_BYTES {
                return Err(SalesStoreError::invalid(
                    "company.logo",
                    "el logotipo está vacío o supera los 2 MB",
                ));
            }
        }
        let tx = self.conn.transaction()?;
        // Con registros Verifactu el NIF del emisor consta en la cadena (D193, D213): no cambia.
        let has_records: i64 =
            tx.query_row("SELECT count(*) FROM verifactu_records", [], |r| r.get(0))?;
        if has_records > 0 {
            let current = load_company(&tx)?.and_then(|c| c.tax_id);
            if clean_tax_id(&company.tax_id) != clean_tax_id(&current) {
                return Err(SalesStoreError::Conflict(
                    "el NIF de la empresa no se puede cambiar: ya hay registros Verifactu".into(),
                ));
            }
        }
        // El logotipo se guarda una sola vez por contenido y nunca se modifica: los documentos
        // emitidos citan el suyo por su hash (D76, D90).
        let logo_sha256 = match (&company.logo, &company.logo_mime) {
            (Some(logo), Some(mime)) => {
                let hash = sha256_hex(logo);
                tx.execute(
                    "INSERT OR IGNORE INTO company_logos (sha256, mime, data, created_at)                      VALUES (?1, ?2, ?3, ?4)",
                    params![hash, mime, logo, now_ms()],
                )?;
                Some(hash)
            }
            _ => None,
        };
        tx.execute(
            "INSERT INTO company (id, name, tax_id, address, email, iban, logo_sha256, updated_at)              VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)              ON CONFLICT(id) DO UPDATE SET name = excluded.name, tax_id = excluded.tax_id,              address = excluded.address, email = excluded.email, iban = excluded.iban,              logo_sha256 = excluded.logo_sha256, updated_at = excluded.updated_at",
            params![
                clean(&company.name),
                clean_tax_id(&company.tax_id),
                clean(&company.address),
                clean(&company.email),
                clean(&company.iban),
                logo_sha256,
                now_ms(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Logotipo (contenido y tipo MIME) con ese SHA-256, incluso si la empresa ya usa otro: así
    /// la copia de un documento emitido sigue pudiendo mostrarse (D76). `None` si no existe.
    /// Comprueba que el contenido guardado corresponde al hash.
    pub fn company_logo_by_hash(&self, sha256: &str) -> SalesResult<Option<(Vec<u8>, String)>> {
        let found: Option<(Vec<u8>, String)> = self
            .conn
            .query_row(
                "SELECT data, mime FROM company_logos WHERE sha256 = ?1",
                [sha256],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((data, _)) = &found {
            if sha256_hex(data) != sha256 {
                return Err(SalesStoreError::Corrupt(format!("logotipo {sha256}")));
            }
        }
        Ok(found)
    }

    // --- Clientes ---

    pub fn create_customer(&mut self, data: &CustomerData) -> SalesResult<i64> {
        check_customer(data)?;
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO customers (name, tax_id, address, email, retired, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
            params![
                data.name.trim(),
                clean_tax_id(&data.tax_id),
                clean(&data.address),
                clean(&data.email),
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_customer(&mut self, id: i64, data: &CustomerData) -> SalesResult<()> {
        check_customer(data)?;
        let changed = self.conn.execute(
            "UPDATE customers SET name = ?2, tax_id = ?3, address = ?4, email = ?5, updated_at = ?6 \
             WHERE id = ?1",
            params![
                id,
                data.name.trim(),
                clean_tax_id(&data.tax_id),
                clean(&data.address),
                clean(&data.email),
                now_ms()
            ],
        )?;
        if changed == 0 {
            return Err(SalesStoreError::not_found("cliente", id));
        }
        Ok(())
    }

    pub fn customer(&self, id: i64) -> SalesResult<Customer> {
        load_customer(&self.conn, id)
    }

    pub fn customers(&self, include_retired: bool) -> SalesResult<Vec<Customer>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, tax_id, address, email, retired FROM customers \
             WHERE retired = 0 OR ?1 ORDER BY name COLLATE NOCASE, id",
        )?;
        let rows = stmt.query_map([include_retired], customer_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Retira al cliente del uso nuevo; sus documentos se conservan.
    pub fn retire_customer(&mut self, id: i64) -> SalesResult<()> {
        let changed = self.conn.execute(
            "UPDATE customers SET retired = 1, updated_at = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        if changed == 0 {
            return Err(SalesStoreError::not_found("cliente", id));
        }
        Ok(())
    }

    // --- Catálogo (D79) ---

    /// Alta de un producto o servicio; el `id` del argumento se ignora.
    pub fn create_product(&mut self, product: &Product) -> SalesResult<i64> {
        product.validate()?;
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO products (description, unit_price, tax_rate, qualification, \
             exemption_description, retired, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                product.description.trim(),
                super::money(product.unit_price),
                plain(product.tax_rate.percent()),
                product.exemption.as_ref().map(|e| e.qualification.as_str()),
                product
                    .exemption
                    .as_ref()
                    .and_then(|e| e.description_text().map(str::to_owned)),
                product.retired,
                now,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_product(&mut self, id: i64, product: &Product) -> SalesResult<()> {
        product.validate()?;
        let changed = self.conn.execute(
            "UPDATE products SET description = ?2, unit_price = ?3, tax_rate = ?4, \
             qualification = ?5, exemption_description = ?6, retired = ?7, updated_at = ?8 \
             WHERE id = ?1",
            params![
                id,
                product.description.trim(),
                super::money(product.unit_price),
                plain(product.tax_rate.percent()),
                product.exemption.as_ref().map(|e| e.qualification.as_str()),
                product
                    .exemption
                    .as_ref()
                    .and_then(|e| e.description_text().map(str::to_owned)),
                product.retired,
                now_ms(),
            ],
        )?;
        if changed == 0 {
            return Err(SalesStoreError::not_found("producto", id));
        }
        Ok(())
    }

    pub fn product(&self, id: i64) -> SalesResult<Product> {
        load_product(&self.conn, id)
    }

    pub fn products(&self, include_retired: bool) -> SalesResult<Vec<Product>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, description, unit_price, tax_rate, qualification, exemption_description, \
             retired FROM products WHERE retired = 0 OR ?1 ORDER BY description COLLATE NOCASE, id",
        )?;
        let raws = stmt
            .query_map([include_retired], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, i64>(6)?,
                    ),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter()
            .map(|(id, raw)| product_from(id, raw))
            .collect()
    }

    pub fn retire_product(&mut self, id: i64) -> SalesResult<()> {
        let changed = self.conn.execute(
            "UPDATE products SET retired = 1, updated_at = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        if changed == 0 {
            return Err(SalesStoreError::not_found("producto", id));
        }
        Ok(())
    }

    // --- Albaranes recibidos (D74); sin IA en esta capa ---

    pub fn create_received_delivery_note(
        &mut self,
        supplier_name: &str,
        supplier_tax_id: Option<&str>,
        supplier_number: &str,
        date: IsoDate,
    ) -> SalesResult<i64> {
        if supplier_name.trim().is_empty() {
            return Err(SalesStoreError::invalid(
                "supplier_name",
                "falta el proveedor",
            ));
        }
        if supplier_number.trim().is_empty() {
            return Err(SalesStoreError::invalid(
                "supplier_number",
                "falta el número del albarán del proveedor",
            ));
        }
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO received_delivery_notes (supplier_name, supplier_tax_id, supplier_number, \
             date, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'received', ?5, ?5)",
            params![
                supplier_name.trim(),
                supplier_tax_id.map(str::trim).filter(|s| !s.is_empty()),
                // Conserva el número del proveedor tal cual (D73).
                supplier_number,
                date.to_string(),
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn received_delivery_note(&self, id: i64) -> SalesResult<ReceivedDeliveryNote> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {RECEIVED_COLS} FROM received_delivery_notes WHERE id = ?1"),
                [id],
                received_row,
            )
            .optional()?
            .ok_or(SalesStoreError::not_found("albarán recibido", id))?;
        received_from(raw)
    }

    pub fn received_delivery_notes(&self) -> SalesResult<Vec<ReceivedDeliveryNote>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RECEIVED_COLS} FROM received_delivery_notes ORDER BY date, id"
        ))?;
        let raws = stmt
            .query_map([], received_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter().map(received_from).collect()
    }

    /// `received → reviewed` (D74), con los vínculos opcionales a la factura recibida.
    pub fn review_received_delivery_note(
        &mut self,
        id: i64,
        document_ref: Option<&str>,
        invoice_ref: Option<&str>,
    ) -> SalesResult<()> {
        let tx = self.conn.transaction()?;
        let raw = tx
            .query_row(
                &format!("SELECT {RECEIVED_COLS} FROM received_delivery_notes WHERE id = ?1"),
                [id],
                received_row,
            )
            .optional()?
            .ok_or(SalesStoreError::not_found("albarán recibido", id))?;
        received_from(raw)?
            .status
            .transition(ReceivedStatus::Reviewed)?;
        let changed = tx.execute(
            "UPDATE received_delivery_notes SET status = 'reviewed', document_ref = ?2,              invoice_ref = ?3, updated_at = ?4 WHERE id = ?1 AND status = 'received'",
            params![id, document_ref, invoice_ref, now_ms()],
        )?;
        if changed != 1 {
            return Err(SalesStoreError::Conflict(format!(
                "el albarán recibido {id} cambió de estado durante la revisión"
            )));
        }
        audit(
            &tx,
            "sales.received_reviewed",
            &serde_json::json!({ "id": id }),
        )?;
        tx.commit()?;
        Ok(())
    }
}
