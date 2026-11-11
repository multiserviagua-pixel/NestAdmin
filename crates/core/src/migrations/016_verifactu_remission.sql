-- Migración 16 (D248–D258): remisión de los registros Verifactu a la AEAT. Todas las tablas son
-- de solo inserción (triggers): el estado de cada registro se DERIVA de lo que consta aquí, nunca
-- se actualiza. Un lote sin fila en `verifactu_batch_results` está abierto y se reenvía con el
-- mismo contenido hasta que llegue una respuesta o un `Fault` SOAP.

-- Entorno vigente (D248): el último cambio manda; sin filas, `simulador`. `authorization`
-- deja constancia de quién lo cambió (D85: ajuste y confirmación explícitos).
CREATE TABLE verifactu_environment_changes (
    id            INTEGER PRIMARY KEY,
    environment   TEXT NOT NULL CHECK (environment IN ('simulador', 'preproduccion', 'produccion')),
    authorization TEXT NOT NULL,
    changed_at    INTEGER NOT NULL
);

-- Entorno de cada registro, fijado al crearlo en la misma transacción que la emisión o la
-- anulación (D248). `produccion_heredado` marca los anteriores a esta migración (D258): su QR
-- sigue siendo el de producción y el PDF regenerado conserva los bytes (D226). Desde la
-- migración, un registro sin fila aquí es un error, no un «producción» silencioso.
CREATE TABLE verifactu_record_environments (
    record_id   INTEGER PRIMARY KEY REFERENCES verifactu_records (id),
    environment TEXT NOT NULL CHECK (
        environment IN ('simulador', 'preproduccion', 'produccion', 'produccion_heredado')
    )
);

INSERT INTO verifactu_record_environments (record_id, environment)
    SELECT id, 'produccion_heredado' FROM verifactu_records;

-- Lotes de remisión (D249). `header_json` es la cabecera del envío tal como se mandó y
-- `verifactu_batch_records` los registros en orden de cadena: con ambos se reconstruyen los
-- mismos bytes del envío (los registros son inmutables). `authorization` es
-- `obligacion_verifactu <entorno>` (R07, D250).
CREATE TABLE verifactu_batches (
    id              INTEGER PRIMARY KEY,
    environment     TEXT NOT NULL CHECK (environment IN ('simulador', 'preproduccion', 'produccion')),
    idempotency_key TEXT NOT NULL UNIQUE,
    header_json     TEXT NOT NULL,
    record_count    INTEGER NOT NULL CHECK (record_count BETWEEN 1 AND 1000),
    authorization   TEXT NOT NULL,
    started_at      INTEGER NOT NULL
);

CREATE TABLE verifactu_batch_records (
    batch_id  INTEGER NOT NULL REFERENCES verifactu_batches (id),
    position  INTEGER NOT NULL CHECK (position >= 0),
    record_id INTEGER NOT NULL REFERENCES verifactu_records (id),
    PRIMARY KEY (batch_id, position),
    UNIQUE (batch_id, record_id)
);

CREATE INDEX verifactu_batch_records_record ON verifactu_batch_records (record_id);

-- Cada intento de envío de un lote se anota ANTES de la llamada: un lote con más de un intento
-- ya pudo llegar a la AEAT, y solo entonces un duplicado (3000) o una baja repetida (3001) se
-- cuentan como aceptados (D252, ajustes 2 y 3).
CREATE TABLE verifactu_batch_attempts (
    id           INTEGER PRIMARY KEY,
    batch_id     INTEGER NOT NULL REFERENCES verifactu_batches (id),
    attempted_at INTEGER NOT NULL
);

CREATE INDEX verifactu_batch_attempts_batch ON verifactu_batch_attempts (batch_id);

-- Resultado de un lote: solo una respuesta entendida o un `Fault` SOAP lo cierran (D252).
-- Un fallo de red, un 5xx o una respuesta ilegible no escriben aquí.
CREATE TABLE verifactu_batch_results (
    batch_id          INTEGER PRIMARY KEY REFERENCES verifactu_batches (id),
    outcome           TEXT NOT NULL CHECK (outcome IN ('responded', 'fault')),
    send_state        TEXT CHECK (send_state IN ('Correcto', 'ParcialmenteCorrecto', 'Incorrecto')),
    error_code        INTEGER,
    error_description TEXT,
    wait_seconds      INTEGER NOT NULL CHECK (wait_seconds >= 0),
    csv               TEXT,
    finished_at       INTEGER NOT NULL
);

-- Resultado de cada registro de un lote respondido (D249). Se guardan códigos y descripciones,
-- no el XML de la respuesta. `duplicate` marca lo aceptado porque la AEAT ya lo tenía de un
-- intento anterior de este mismo lote; `unexpected_duplicate` es un duplicado que no es nuestro
-- (ajuste 3). Un registro sin línea en una respuesta parcial no tiene fila: sigue pendiente.
CREATE TABLE verifactu_record_results (
    id               INTEGER PRIMARY KEY,
    batch_id         INTEGER NOT NULL REFERENCES verifactu_batches (id),
    record_id        INTEGER NOT NULL REFERENCES verifactu_records (id),
    status           TEXT NOT NULL CHECK (
        status IN ('accepted', 'accepted_with_errors', 'rejected', 'unexpected_duplicate')
    ),
    duplicate        INTEGER NOT NULL DEFAULT 0 CHECK (duplicate IN (0, 1)),
    aeat_code        INTEGER,
    aeat_description TEXT,
    UNIQUE (batch_id, record_id)
);

CREATE INDEX verifactu_record_results_record ON verifactu_record_results (record_id);

-- Subsanación (D253): el registro rechazado y el nuevo que lo subsana. Un rechazado se subsana
-- una sola vez (clave primaria) y un registro solo subsana uno (único): doble clic, un registro.
CREATE TABLE verifactu_subsanations (
    rejected_record_id    INTEGER PRIMARY KEY REFERENCES verifactu_records (id),
    subsanation_record_id INTEGER NOT NULL UNIQUE REFERENCES verifactu_records (id),
    created_at            INTEGER NOT NULL
);

CREATE TRIGGER verifactu_environment_changes_no_update
BEFORE UPDATE ON verifactu_environment_changes
BEGIN SELECT RAISE(ABORT, 'un cambio de entorno de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_environment_changes_no_delete
BEFORE DELETE ON verifactu_environment_changes
BEGIN SELECT RAISE(ABORT, 'un cambio de entorno de Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_record_environments_no_update
BEFORE UPDATE ON verifactu_record_environments
BEGIN SELECT RAISE(ABORT, 'el entorno de un registro Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_record_environments_no_delete
BEFORE DELETE ON verifactu_record_environments
BEGIN SELECT RAISE(ABORT, 'el entorno de un registro Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_batches_no_update
BEFORE UPDATE ON verifactu_batches
BEGIN SELECT RAISE(ABORT, 'un lote de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_batches_no_delete
BEFORE DELETE ON verifactu_batches
BEGIN SELECT RAISE(ABORT, 'un lote de Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_batch_records_no_update
BEFORE UPDATE ON verifactu_batch_records
BEGIN SELECT RAISE(ABORT, 'los registros de un lote de Verifactu no se modifican'); END;
CREATE TRIGGER verifactu_batch_records_no_delete
BEFORE DELETE ON verifactu_batch_records
BEGIN SELECT RAISE(ABORT, 'los registros de un lote de Verifactu no se eliminan'); END;

CREATE TRIGGER verifactu_batch_attempts_no_update
BEFORE UPDATE ON verifactu_batch_attempts
BEGIN SELECT RAISE(ABORT, 'un intento de envío de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_batch_attempts_no_delete
BEFORE DELETE ON verifactu_batch_attempts
BEGIN SELECT RAISE(ABORT, 'un intento de envío de Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_batch_results_no_update
BEFORE UPDATE ON verifactu_batch_results
BEGIN SELECT RAISE(ABORT, 'el resultado de un lote de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_batch_results_no_delete
BEFORE DELETE ON verifactu_batch_results
BEGIN SELECT RAISE(ABORT, 'el resultado de un lote de Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_record_results_no_update
BEFORE UPDATE ON verifactu_record_results
BEGIN SELECT RAISE(ABORT, 'el resultado de un registro Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_record_results_no_delete
BEFORE DELETE ON verifactu_record_results
BEGIN SELECT RAISE(ABORT, 'el resultado de un registro Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_subsanations_no_update
BEFORE UPDATE ON verifactu_subsanations
BEGIN SELECT RAISE(ABORT, 'una subsanación de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_subsanations_no_delete
BEFORE DELETE ON verifactu_subsanations
BEGIN SELECT RAISE(ABORT, 'una subsanación de Verifactu no se elimina'); END;
