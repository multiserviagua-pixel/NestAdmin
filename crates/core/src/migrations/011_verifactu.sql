-- Registros de facturación Verifactu (fase 11): alta y anulación de cada factura, encadenados
-- por su huella y escritos en la misma transacción que la emisión o la anulación. Los registros
-- no se modifican ni se borran (triggers); la cadena no se bifurca (índices únicos): cada
-- registro tiene como máximo un sucesor y solo el primero carece de anterior. `record_json`
-- guarda el registro completo para poder reconstruirlo, verificarlo y remitirlo después; las
-- demás columnas son una copia consultable de sus campos principales.
CREATE TABLE verifactu_records (
    id             INTEGER PRIMARY KEY,
    kind           TEXT NOT NULL CHECK (kind IN ('alta', 'anulacion')),
    document_id    INTEGER NOT NULL REFERENCES sales_documents (id),
    issuer_tax_id  TEXT NOT NULL,
    invoice_number TEXT NOT NULL,
    -- DD-MM-AAAA, tal como va en el registro.
    issue_date     TEXT NOT NULL,
    -- Solo en el alta (F1, F2, R1 a R5); NULL en la anulación.
    invoice_type   TEXT,
    -- Texto con dos decimales; NULL en la anulación.
    total          TEXT,
    tax_total      TEXT,
    previous_id    INTEGER REFERENCES verifactu_records (id),
    previous_hash  TEXT,
    -- AAAA-MM-DDThh:mm:ss±hh:mm
    generated_at   TEXT NOT NULL,
    -- SHA-256 en hexadecimal y mayúsculas.
    hash           TEXT NOT NULL UNIQUE CHECK (length(hash) = 64),
    record_json    TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    CHECK (
        (kind = 'alta' AND invoice_type IS NOT NULL AND total IS NOT NULL
            AND tax_total IS NOT NULL)
        OR (kind = 'anulacion' AND invoice_type IS NULL AND total IS NULL
            AND tax_total IS NULL)
    ),
    CHECK ((previous_id IS NULL) = (previous_hash IS NULL))
);

CREATE UNIQUE INDEX verifactu_records_previous
    ON verifactu_records (previous_id) WHERE previous_id IS NOT NULL;
CREATE UNIQUE INDEX verifactu_records_single_first
    ON verifactu_records ((previous_id IS NULL)) WHERE previous_id IS NULL;
CREATE INDEX verifactu_records_document ON verifactu_records (document_id);

CREATE TRIGGER verifactu_records_no_update
BEFORE UPDATE ON verifactu_records
BEGIN
    SELECT RAISE(ABORT, 'un registro Verifactu no se modifica');
END;

CREATE TRIGGER verifactu_records_no_delete
BEFORE DELETE ON verifactu_records
BEGIN
    SELECT RAISE(ABORT, 'un registro Verifactu no se elimina');
END;
