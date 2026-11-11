-- Migración 15 (D233, D235, D239): envíos de documentos de venta por correo. Cada fila es un
-- correo que el conector confirmó (recibo) con un documento adjunto; se escribe en la misma
-- transacción que cierra la tarea y, si el documento estaba emitido, lo pasa a «enviado». Es de
-- solo inserción (triggers) y la clave de idempotencia es única: repetir la escritura al
-- recuperar un corte (`INSERT OR IGNORE`) no duplica nada. No guarda el cuerpo del correo ni el
-- PDF, solo el nombre, el tamaño y el SHA-256 del adjunto.
CREATE TABLE sales_document_sends (
    id                INTEGER PRIMARY KEY,
    document_id       INTEGER NOT NULL REFERENCES sales_documents (id),
    task_id           INTEGER NOT NULL REFERENCES tasks (id),
    version           INTEGER NOT NULL,
    idempotency_key   TEXT NOT NULL UNIQUE,
    account           TEXT NOT NULL,
    account_address   TEXT,
    recipient         TEXT NOT NULL,
    subject           TEXT NOT NULL,
    attachment_name   TEXT NOT NULL,
    attachment_size   INTEGER NOT NULL CHECK (attachment_size > 0),
    attachment_sha256 TEXT NOT NULL CHECK (length(attachment_sha256) = 64),
    -- «aprobación» o «regla <id> v<n>», como en `actions.authorization`.
    authorization     TEXT NOT NULL,
    sent_message_id   TEXT,
    sent_at           INTEGER NOT NULL
);

CREATE INDEX sales_document_sends_document ON sales_document_sends (document_id, sent_at);

CREATE TRIGGER sales_document_sends_no_update
BEFORE UPDATE ON sales_document_sends
BEGIN
    SELECT RAISE(ABORT, 'un envío de documento de venta no se modifica');
END;

CREATE TRIGGER sales_document_sends_no_delete
BEFORE DELETE ON sales_document_sends
BEGIN
    SELECT RAISE(ABORT, 'un envío de documento de venta no se elimina');
END;

-- Disparo de la regla por cliente (D238): a lo sumo una tarea de envío nace al emitir cada
-- documento, aunque la emisión se reintente o se repita la llamada.
CREATE TABLE sales_send_triggers (
    document_id INTEGER PRIMARY KEY REFERENCES sales_documents (id),
    task_id     INTEGER NOT NULL REFERENCES tasks (id),
    created_at  INTEGER NOT NULL
);

CREATE TRIGGER sales_send_triggers_no_update
BEFORE UPDATE ON sales_send_triggers
BEGIN
    SELECT RAISE(ABORT, 'el disparo de un envío no se modifica');
END;

CREATE TRIGGER sales_send_triggers_no_delete
BEFORE DELETE ON sales_send_triggers
BEGIN
    SELECT RAISE(ABORT, 'el disparo de un envío no se elimina');
END;
