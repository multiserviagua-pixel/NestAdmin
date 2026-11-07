//! Cobros (D88) y plantillas recurrentes (D87). El estado de cobro y el calendario los deciden
//! `nestadmin_sales::payments` y `nestadmin_sales::recurring`; aquí solo se arman sus entradas
//! desde la base y se guarda el resultado.

use std::collections::BTreeSet;

use nestadmin_documents::IsoDate;
use nestadmin_sales::{
    add_payment, due_drafts, remove_payment, summarize, CollectionSummary, DocumentKind, Frequency,
    IssueMode, LineInput, LinkedCorrective, Payment, PaymentMethod, PeriodKey, Receivable,
    RecurringTemplate,
};
use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::Decimal;
use serde_json::json;

use super::documents::{insert_draft, load_document, DraftLine, NewDraft, SalesDocument};
use super::master::load_customer;
use super::{audit, dec, money, parse_date, SalesResult, SalesStoreError};
use crate::store::{now_ms, Store};

fn receivable_of(conn: &Connection, doc: &SalesDocument) -> SalesResult<Receivable> {
    let mut receivable = Receivable::new(
        doc.kind,
        doc.status,
        doc.totals
            .as_ref()
            .map_or(Decimal::ZERO, |totals| totals.amount_due),
        doc.due_date,
    );
    receivable.rectification = doc.rectification.map(|r| r.method);
    let mut stmt =
        conn.prepare("SELECT id FROM sales_documents WHERE rectifies_id = ?1 ORDER BY id")?;
    let ids = stmt
        .query_map([doc.id], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for id in ids {
        let corrective = load_document(conn, id)?;
        if let Some(rect) = corrective.rectification {
            receivable.correctives.push(LinkedCorrective {
                method: rect.method,
                status: corrective.status,
                amount_due: corrective
                    .totals
                    .as_ref()
                    .map_or(Decimal::ZERO, |totals| totals.amount_due),
            });
        }
    }
    Ok(receivable)
}

fn load_payments(conn: &Connection, document_id: i64) -> SalesResult<Vec<Payment>> {
    let mut stmt = conn.prepare(
        "SELECT id, date, amount, method FROM payments WHERE document_id = ?1 ORDER BY id",
    )?;
    let raws = stmt
        .query_map([document_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raws.into_iter()
        .map(|(id, date, amount, method)| {
            Ok(Payment {
                id: id as u64,
                date: parse_date("payment.date", &date)?,
                amount: dec("payment.amount", &amount)?,
                method: PaymentMethod::parse(&method)
                    .ok_or_else(|| SalesStoreError::Corrupt(format!("medio de cobro {method}")))?,
            })
        })
        .collect()
}

/// Plantilla recurrente guardada (D87).
#[derive(Debug, Clone, PartialEq)]
pub struct RecurringRecord {
    pub id: i64,
    pub template: RecurringTemplate,
    pub active: bool,
}

/// Borrador creado a partir de una plantilla.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedDraft {
    pub document_id: i64,
    pub key: PeriodKey,
    /// Quién decide emitirlo: `AutoIssueByRule` solo en el periodo actual con regla expresa.
    pub mode: IssueMode,
}

/// Resultado de una pasada: lo creado y las plantillas que no se pudieron procesar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecurringRun {
    pub created: Vec<GeneratedDraft>,
    /// Plantilla y motivo; no bloquea a las demás.
    pub failed: Vec<(i64, String)>,
}

fn frequency_of(text: &str) -> SalesResult<Frequency> {
    Frequency::parse(text).ok_or_else(|| SalesStoreError::Corrupt(format!("periodicidad {text}")))
}

fn load_template(conn: &Connection, id: i64) -> SalesResult<RecurringRecord> {
    let raw = conn
        .query_row(
            "SELECT customer_id, lines, frequency, first_day, end_day, active \
             FROM recurring_templates WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or(SalesStoreError::not_found("plantilla recurrente", id))?;
    let (customer_id, lines, frequency, first_day, end, active) = raw;
    Ok(RecurringRecord {
        id,
        template: RecurringTemplate {
            id: id as u64,
            customer_id: customer_id as u64,
            lines: serde_json::from_str(&lines)?,
            frequency: frequency_of(&frequency)?,
            first_day: parse_date("first_day", &first_day)?,
            end: end.map(|e| parse_date("end_day", &e)).transpose()?,
        },
        active: active != 0,
    })
}

fn check_template(
    conn: &Connection,
    customer_id: i64,
    lines: &[LineInput],
    frequency: Frequency,
    first_day: IsoDate,
    end: Option<IsoDate>,
) -> SalesResult<RecurringTemplate> {
    load_customer(conn, customer_id)?;
    let template = RecurringTemplate {
        id: 0,
        customer_id: customer_id as u64,
        lines: lines.to_vec(),
        frequency,
        first_day,
        end,
    };
    template.validate()?;
    Ok(template)
}

impl Store {
    // --- Cobros (D88) ---

    /// Registra un cobro. Rechaza el que supera lo pendiente, el de una factura anulada, el de
    /// una sustituida y el de una rectificativa por diferencias (se cobra en la original).
    pub fn add_sales_payment(
        &mut self,
        document_id: i64,
        date: IsoDate,
        amount: Decimal,
        method: PaymentMethod,
        today: IsoDate,
    ) -> SalesResult<(i64, CollectionSummary)> {
        let tx = self.conn.transaction()?;
        let doc = load_document(&tx, document_id)?;
        let receivable = receivable_of(&tx, &doc)?;
        let mut payments = load_payments(&tx, document_id)?;
        tx.execute(
            "INSERT INTO payments (document_id, date, amount, method, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                document_id,
                date.to_string(),
                money(amount),
                method.as_str(),
                now_ms()
            ],
        )?;
        let payment_id = tx.last_insert_rowid();
        // Si la validación falla, la transacción se descarta con el cobro insertado.
        let summary = add_payment(
            &receivable,
            &mut payments,
            Payment {
                id: payment_id as u64,
                date,
                amount,
                method,
            },
            today,
        )?;
        audit(
            &tx,
            "sales.payment_added",
            &json!({
                "payment_id": payment_id, "document_id": document_id,
                "number": doc.number, "amount": money(amount)
            }),
        )?;
        tx.commit()?;
        Ok((payment_id, summary))
    }

    /// Elimina un cobro con entrada en el historial; no afecta a Verifactu (D88).
    pub fn remove_sales_payment(
        &mut self,
        payment_id: i64,
        today: IsoDate,
    ) -> SalesResult<CollectionSummary> {
        let tx = self.conn.transaction()?;
        let document_id: i64 = tx
            .query_row(
                "SELECT document_id FROM payments WHERE id = ?1",
                [payment_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(SalesStoreError::not_found("cobro", payment_id))?;
        let doc = load_document(&tx, document_id)?;
        let receivable = receivable_of(&tx, &doc)?;
        let mut payments = load_payments(&tx, document_id)?;
        let (removed, summary) =
            remove_payment(&receivable, &mut payments, payment_id as u64, today)?;
        tx.execute("DELETE FROM payments WHERE id = ?1", [payment_id])?;
        audit(
            &tx,
            "sales.payment_removed",
            &json!({
                "payment_id": payment_id, "document_id": document_id,
                "number": doc.number, "amount": money(removed.amount)
            }),
        )?;
        tx.commit()?;
        Ok(summary)
    }

    pub fn sales_payments(&self, document_id: i64) -> SalesResult<Vec<Payment>> {
        load_document(&self.conn, document_id)?;
        load_payments(&self.conn, document_id)
    }

    /// Estado de cobro derivado, no guardado (D88).
    pub fn sales_collection(
        &self,
        document_id: i64,
        today: IsoDate,
    ) -> SalesResult<CollectionSummary> {
        let doc = load_document(&self.conn, document_id)?;
        let receivable = receivable_of(&self.conn, &doc)?;
        let payments = load_payments(&self.conn, document_id)?;
        Ok(summarize(&receivable, &payments, today)?)
    }

    // --- Recurrentes (D87) ---

    pub fn create_recurring_template(
        &mut self,
        customer_id: i64,
        lines: &[LineInput],
        frequency: Frequency,
        first_day: IsoDate,
        end: Option<IsoDate>,
    ) -> SalesResult<i64> {
        check_template(&self.conn, customer_id, lines, frequency, first_day, end)?;
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO recurring_templates (customer_id, lines, frequency, first_day, end_day, \
             active, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
            params![
                customer_id,
                serde_json::to_string(lines)?,
                frequency.as_str(),
                first_day.to_string(),
                end.map(|e| e.to_string()),
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Cambia la plantilla para los periodos futuros; los borradores ya creados no se tocan.
    pub fn update_recurring_template(
        &mut self,
        id: i64,
        customer_id: i64,
        lines: &[LineInput],
        frequency: Frequency,
        first_day: IsoDate,
        end: Option<IsoDate>,
    ) -> SalesResult<()> {
        check_template(&self.conn, customer_id, lines, frequency, first_day, end)?;
        let generated: i64 = self.conn.query_row(
            "SELECT count(*) FROM sales_documents WHERE recurring_template_id = ?1",
            [id],
            |r| r.get(0),
        )?;
        let current = load_template(&self.conn, id)?;
        if generated > 0
            && (current.template.first_day != first_day || current.template.frequency != frequency)
        {
            // Cambiar el calendario reasignaría los números de periodo ya generados.
            return Err(SalesStoreError::Conflict(
                "con borradores ya generados no se cambia el primer día ni la periodicidad".into(),
            ));
        }
        self.conn.execute(
            "UPDATE recurring_templates SET customer_id = ?2, lines = ?3, frequency = ?4, \
             first_day = ?5, end_day = ?6, updated_at = ?7 WHERE id = ?1",
            params![
                id,
                customer_id,
                serde_json::to_string(lines)?,
                frequency.as_str(),
                first_day.to_string(),
                end.map(|e| e.to_string()),
                now_ms()
            ],
        )?;
        Ok(())
    }

    pub fn set_recurring_active(&mut self, id: i64, active: bool) -> SalesResult<()> {
        let changed = self.conn.execute(
            "UPDATE recurring_templates SET active = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, active, now_ms()],
        )?;
        if changed == 0 {
            return Err(SalesStoreError::not_found("plantilla recurrente", id));
        }
        Ok(())
    }

    pub fn recurring_template(&self, id: i64) -> SalesResult<RecurringRecord> {
        load_template(&self.conn, id)
    }

    pub fn recurring_templates(&self) -> SalesResult<Vec<RecurringRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM recurring_templates ORDER BY id")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| load_template(&self.conn, id))
            .collect()
    }

    /// Crea el borrador de cada periodo vencido de las plantillas activas (D87). Es idempotente:
    /// los periodos ya generados se leen de la base y la restricción única (plantilla, periodo)
    /// impide duplicarlos aunque una pasada se corte o se repita. No emite nada: la emisión
    /// automática con regla expresa es de la capa de aplicación.
    pub fn generate_recurring_drafts(
        &mut self,
        today: IsoDate,
        express_rule_active: impl Fn(u64) -> bool,
    ) -> SalesResult<RecurringRun> {
        let tx = self.conn.transaction()?;
        let mut stmt =
            tx.prepare("SELECT id FROM recurring_templates WHERE active = 1 ORDER BY id")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut run = RecurringRun::default();
        for id in ids {
            let record = load_template(&tx, id)?;
            let mut stmt = tx.prepare(
                "SELECT recurring_period FROM sales_documents WHERE recurring_template_id = ?1",
            )?;
            let existing: BTreeSet<u32> = stmt
                .query_map([id], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .map(|p| p as u32)
                .collect();
            drop(stmt);
            let plans = match due_drafts(
                &record.template,
                today,
                &existing,
                express_rule_active(id as u64),
            ) {
                Ok(plans) => plans,
                Err(e) => {
                    run.failed.push((id, e.to_string()));
                    continue;
                }
            };
            for plan in plans {
                let mut draft = NewDraft::new(DocumentKind::Invoice);
                draft.customer_id = Some(plan.customer_id as i64);
                // Expedición: la fecha real de la generación; operación: inicio del periodo.
                draft.issue_date = Some(plan.issue_date);
                draft.operation_date = Some(plan.period_start);
                draft.recurring = Some((id, plan.key.period));
                draft.lines = plan.lines.iter().cloned().map(DraftLine::from).collect();
                let document_id = insert_draft(&tx, &draft)?;
                audit(
                    &tx,
                    "sales.recurring_draft",
                    &json!({
                        "id": document_id, "template": id, "period": plan.key.period
                    }),
                )?;
                run.created.push(GeneratedDraft {
                    document_id,
                    key: plan.key,
                    mode: plan.mode,
                });
            }
        }
        tx.commit()?;
        Ok(run)
    }
}
