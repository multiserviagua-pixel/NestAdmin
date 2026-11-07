-- Migración 3 (D03, D72): facturación de venta. Importes como texto decimal con dos decimales
-- (D04, solo EUR); fechas como texto ISO AAAA-MM-DD.

CREATE TABLE company (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    name        TEXT,
    tax_id      TEXT,
    address     TEXT,
    email       TEXT,
    iban        TEXT,
    logo        BLOB,
    logo_mime   TEXT,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE customers (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    tax_id      TEXT,
    address     TEXT,
    email       TEXT,
    retired     INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE products (
    id                    INTEGER PRIMARY KEY,
    description           TEXT NOT NULL,
    unit_price            TEXT NOT NULL,
    tax_rate              TEXT NOT NULL,
    qualification         TEXT,
    exemption_description TEXT,
    retired               INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL
);

-- Último número y última fecha de expedición por serie (tipo y año, D73).
CREATE TABLE series (
    kind        TEXT NOT NULL,
    year        INTEGER NOT NULL CHECK (year BETWEEN 1 AND 9999),
    last        INTEGER NOT NULL CHECK (last >= 0),
    last_date   TEXT,
    PRIMARY KEY (kind, year)
);

CREATE TABLE recurring_templates (
    id          INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers (id),
    lines       TEXT NOT NULL,
    frequency   TEXT NOT NULL CHECK (frequency IN ('monthly', 'quarterly', 'annual')),
    first_day   TEXT NOT NULL,
    end_day     TEXT,
    active      INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE sales_documents (
    id                          INTEGER PRIMARY KEY,
    kind                        TEXT NOT NULL CHECK (kind IN
        ('quote', 'delivery_note', 'invoice', 'simplified_invoice', 'corrective_invoice')),
    status                      TEXT NOT NULL CHECK (status IN
        ('draft', 'issued', 'sent', 'accepted', 'rejected', 'expired', 'converted',
         'delivered', 'invoiced', 'voided')),
    series_year                 INTEGER,
    sequence                    INTEGER CHECK (sequence IS NULL OR sequence > 0),
    revision                    INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    superseded_by               INTEGER REFERENCES sales_documents (id),
    customer_id                 INTEGER REFERENCES customers (id),
    issue_date                  TEXT,
    operation_date              TEXT,
    due_date                    TEXT,
    withholding_percent         TEXT,
    currency                    TEXT NOT NULL DEFAULT 'EUR' CHECK (currency = 'EUR'),
    issuer_snapshot             TEXT,
    recipient_snapshot          TEXT,
    totals                      TEXT,
    base_total                  TEXT,
    tax_total                   TEXT,
    total                       TEXT,
    amount_due                  TEXT,
    rectifies_id                INTEGER REFERENCES sales_documents (id),
    rectification_kind          TEXT,
    rectification_method        TEXT,
    rectified_base              TEXT,
    rectified_tax               TEXT,
    recipient_requested_tax_id  INTEGER NOT NULL DEFAULT 0 CHECK (recipient_requested_tax_id IN (0, 1)),
    art_4_2_declared            INTEGER NOT NULL DEFAULT 0 CHECK (art_4_2_declared IN (0, 1)),
    recurring_template_id       INTEGER REFERENCES recurring_templates (id),
    recurring_period            INTEGER,
    created_at                  INTEGER NOT NULL,
    updated_at                  INTEGER NOT NULL,
    CHECK ((sequence IS NULL) = (series_year IS NULL)),
    UNIQUE (kind, series_year, sequence, revision),
    UNIQUE (recurring_template_id, recurring_period)
);
CREATE INDEX sales_documents_kind_status ON sales_documents (kind, status);
CREATE INDEX sales_documents_customer ON sales_documents (customer_id);
CREATE INDEX sales_documents_rectifies ON sales_documents (rectifies_id);

CREATE TABLE sales_lines (
    id                    INTEGER PRIMARY KEY,
    document_id           INTEGER NOT NULL REFERENCES sales_documents (id) ON DELETE CASCADE,
    position              INTEGER NOT NULL,
    description           TEXT NOT NULL,
    quantity              TEXT NOT NULL,
    unit_price            TEXT NOT NULL,
    discount              TEXT NOT NULL,
    tax_rate              TEXT NOT NULL,
    qualification         TEXT,
    exemption_description TEXT,
    amount                TEXT,
    product_id            INTEGER REFERENCES products (id),
    UNIQUE (document_id, position)
);

CREATE TABLE document_links (
    id          INTEGER PRIMARY KEY,
    from_id     INTEGER NOT NULL REFERENCES sales_documents (id),
    to_id       INTEGER NOT NULL REFERENCES sales_documents (id),
    relation    TEXT NOT NULL CHECK (relation IN ('converted', 'rectifies', 'revision')),
    created_at  INTEGER NOT NULL
);
CREATE INDEX document_links_from ON document_links (from_id);
CREATE INDEX document_links_to ON document_links (to_id);

CREATE TABLE payments (
    id          INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES sales_documents (id),
    date        TEXT NOT NULL,
    amount      TEXT NOT NULL,
    method      TEXT NOT NULL CHECK (method IN ('transfer', 'cash', 'card', 'other')),
    created_at  INTEGER NOT NULL
);
CREATE INDEX payments_document ON payments (document_id);

CREATE TABLE received_delivery_notes (
    id              INTEGER PRIMARY KEY,
    supplier_name   TEXT NOT NULL,
    supplier_tax_id TEXT,
    supplier_number TEXT NOT NULL,
    date            TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('received', 'reviewed')),
    document_ref    TEXT,
    invoice_ref     TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

-- Defensa en profundidad (D73, D74, D81): lo emitido no cambia de contenido y un número
-- consumido nunca se reutiliza, aunque otra capa se equivoque. Tras emitir solo pueden cambiar
-- status, superseded_by y updated_at (el vencimiento es contenido y queda fijado).
CREATE TRIGGER sales_documents_frozen
BEFORE UPDATE ON sales_documents
WHEN OLD.status <> 'draft' AND (
       NEW.status = 'draft'
    OR NEW.kind IS NOT OLD.kind
    OR NEW.series_year IS NOT OLD.series_year
    OR NEW.sequence IS NOT OLD.sequence
    OR NEW.revision IS NOT OLD.revision
    OR NEW.customer_id IS NOT OLD.customer_id
    OR NEW.issue_date IS NOT OLD.issue_date
    OR NEW.operation_date IS NOT OLD.operation_date
    OR NEW.due_date IS NOT OLD.due_date
    OR NEW.withholding_percent IS NOT OLD.withholding_percent
    OR NEW.currency IS NOT OLD.currency
    OR NEW.issuer_snapshot IS NOT OLD.issuer_snapshot
    OR NEW.recipient_snapshot IS NOT OLD.recipient_snapshot
    OR NEW.totals IS NOT OLD.totals
    OR NEW.base_total IS NOT OLD.base_total
    OR NEW.tax_total IS NOT OLD.tax_total
    OR NEW.total IS NOT OLD.total
    OR NEW.amount_due IS NOT OLD.amount_due
    OR NEW.rectifies_id IS NOT OLD.rectifies_id
    OR NEW.rectification_kind IS NOT OLD.rectification_kind
    OR NEW.rectification_method IS NOT OLD.rectification_method
    OR NEW.rectified_base IS NOT OLD.rectified_base
    OR NEW.rectified_tax IS NOT OLD.rectified_tax
    OR NEW.recipient_requested_tax_id IS NOT OLD.recipient_requested_tax_id
    OR NEW.art_4_2_declared IS NOT OLD.art_4_2_declared
    OR NEW.recurring_template_id IS NOT OLD.recurring_template_id
    OR NEW.recurring_period IS NOT OLD.recurring_period
)
BEGIN
    SELECT RAISE(ABORT, 'un documento emitido no se modifica');
END;

CREATE TRIGGER sales_documents_issue_needs_number
BEFORE UPDATE ON sales_documents
WHEN OLD.status = 'draft' AND NEW.status <> 'draft' AND NEW.sequence IS NULL
BEGIN
    SELECT RAISE(ABORT, 'un documento emitido necesita número');
END;

CREATE TRIGGER sales_documents_no_delete
BEFORE DELETE ON sales_documents
WHEN OLD.status <> 'draft'
BEGIN
    SELECT RAISE(ABORT, 'un documento emitido no se elimina');
END;

CREATE TRIGGER sales_lines_no_insert
BEFORE INSERT ON sales_lines
WHEN (SELECT status FROM sales_documents WHERE id = NEW.document_id) <> 'draft'
BEGIN
    SELECT RAISE(ABORT, 'las líneas de un documento emitido no se modifican');
END;

CREATE TRIGGER sales_lines_no_update
BEFORE UPDATE ON sales_lines
WHEN (SELECT status FROM sales_documents WHERE id = OLD.document_id) <> 'draft'
  OR (SELECT status FROM sales_documents WHERE id = NEW.document_id) <> 'draft'
BEGIN
    SELECT RAISE(ABORT, 'las líneas de un documento emitido no se modifican');
END;

CREATE TRIGGER sales_lines_no_delete
BEFORE DELETE ON sales_lines
WHEN (SELECT status FROM sales_documents WHERE id = OLD.document_id) <> 'draft'
BEGIN
    SELECT RAISE(ABORT, 'las líneas de un documento emitido no se modifican');
END;

CREATE TRIGGER series_never_backwards
BEFORE UPDATE ON series
WHEN NEW.last < OLD.last OR NEW.kind <> OLD.kind OR NEW.year <> OLD.year
BEGIN
    SELECT RAISE(ABORT, 'el correlativo de una serie no retrocede');
END;

CREATE TRIGGER series_no_delete
BEFORE DELETE ON series
BEGIN
    SELECT RAISE(ABORT, 'una serie no se elimina');
END;
