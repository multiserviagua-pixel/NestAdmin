-- Migración 17 (T18b-D1, D278–D283): configuración de la remisión por base, intentos sin envío,
-- incidencias y paro por autorización. Solo inserción: ninguna fila se actualiza ni se borra
-- (triggers), y lo que cambia se DERIVA de la última fila o de un par de filas. No reescribe
-- ningún dato de la migración 16.
--
-- Cada base es la de un obligado (D273), así que esta configuración es única por base.

-- Configuración de la remisión (D278, D283). El valor vigente es la última fila; sin filas, los
-- valores por defecto de la aplicación (sin certificado, remite el propio obligado, sin
-- representante). `certificate_source` es el JSON de `CertificateSource` (huella pública del
-- almacén o referencia opaca del PFX): nunca contiene contraseñas, PIN ni el certificado. El
-- representante se configura expresamente y no se deduce (D283). Cada fila nueva reanuda la
-- remisión parada por autorización (D281).
CREATE TABLE verifactu_remission_settings (
    id                  INTEGER PRIMARY KEY,
    certificate_source  TEXT CHECK (certificate_source IS NULL OR json_valid(certificate_source)),
    sender_capacity     TEXT NOT NULL CHECK (
        sender_capacity IN ('taxpayer', 'representative', 'attorney', 'collaborator')
    ),
    representative_nif  TEXT,
    representative_name TEXT,
    authorization       TEXT NOT NULL,
    changed_at          INTEGER NOT NULL,
    CHECK ((representative_nif IS NULL) = (representative_name IS NULL))
);

-- Huellas de certificado que solo negocian TLS 1.2 (D279): las aprende el transporte, no las
-- configura el usuario, y por eso no forman parte de la configuración (cambiarla reanudaría el
-- paro por autorización).
CREATE TABLE verifactu_tls12_preferences (
    fingerprint TEXT PRIMARY KEY CHECK (length(fingerprint) = 64),
    learned_at  INTEGER NOT NULL
);

-- Intentos sin envío (D280). Sin `attempt_id`, el intento falló antes de anotarlo (certificado o
-- clave no disponibles): no hay intento. Con `attempt_id`, el intento ya estaba anotado (D252) y
-- el transporte dijo que el cuerpo no llegó a escribirse: esta fila lo ANULA. Los intentos
-- anulados no cuentan para considerar propio un duplicado 3000 (D50, D252, D280); nada se borra.
CREATE TABLE verifactu_unsent_attempts (
    id           INTEGER PRIMARY KEY,
    batch_id     INTEGER NOT NULL REFERENCES verifactu_batches (id),
    attempt_id   INTEGER UNIQUE REFERENCES verifactu_batch_attempts (id),
    reason_code  TEXT NOT NULL CHECK (length(reason_code) BETWEEN 1 AND 64),
    attempted_at INTEGER NOT NULL
);

CREATE INDEX verifactu_unsent_attempts_batch ON verifactu_unsent_attempts (batch_id);

-- Incidencias técnicas de remisión (D282): se abre con un fallo técnico (sin respuesta, no
-- enviado, `Fault` técnico) y se cierra con el siguiente envío con respuesta. Está abierta si la
-- última fila es `open`. Un registro se generó durante una incidencia si su fecha cae entre una
-- apertura y su cierre (se deduce por el momento).
CREATE TABLE verifactu_incidents (
    id          INTEGER PRIMARY KEY,
    event       TEXT NOT NULL CHECK (event IN ('open', 'close')),
    reason_code TEXT,
    batch_id    INTEGER REFERENCES verifactu_batches (id),
    at          INTEGER NOT NULL,
    CHECK (event <> 'open' OR reason_code IS NOT NULL)
);

-- Paro por `Fault` de autorización (D281): la remisión de esta base se detiene (`halt`) hasta
-- que el usuario cambia el certificado o la configuración (`settings_change`) o declara que lo
-- arregló (`declaration`). Está parada si la última fila es `halt`.
CREATE TABLE verifactu_authorization_halts (
    id         INTEGER PRIMARY KEY,
    event      TEXT NOT NULL CHECK (event IN ('halt', 'resume')),
    batch_id   INTEGER REFERENCES verifactu_batches (id),
    error_code INTEGER,
    cause      TEXT CHECK (cause IS NULL OR cause IN ('declaration', 'settings_change')),
    at         INTEGER NOT NULL,
    CHECK (
        (event = 'halt' AND batch_id IS NOT NULL AND error_code IS NOT NULL AND cause IS NULL)
        OR (event = 'resume' AND batch_id IS NULL AND error_code IS NULL AND cause IS NOT NULL)
    )
);

CREATE TRIGGER verifactu_remission_settings_no_update
BEFORE UPDATE ON verifactu_remission_settings
BEGIN SELECT RAISE(ABORT, 'la configuración de remisión de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_remission_settings_no_delete
BEFORE DELETE ON verifactu_remission_settings
BEGIN SELECT RAISE(ABORT, 'la configuración de remisión de Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_tls12_preferences_no_update
BEFORE UPDATE ON verifactu_tls12_preferences
BEGIN SELECT RAISE(ABORT, 'una preferencia de TLS 1.2 de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_tls12_preferences_no_delete
BEFORE DELETE ON verifactu_tls12_preferences
BEGIN SELECT RAISE(ABORT, 'una preferencia de TLS 1.2 de Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_unsent_attempts_no_update
BEFORE UPDATE ON verifactu_unsent_attempts
BEGIN SELECT RAISE(ABORT, 'un intento sin envío de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_unsent_attempts_no_delete
BEFORE DELETE ON verifactu_unsent_attempts
BEGIN SELECT RAISE(ABORT, 'un intento sin envío de Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_incidents_no_update
BEFORE UPDATE ON verifactu_incidents
BEGIN SELECT RAISE(ABORT, 'una incidencia de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_incidents_no_delete
BEFORE DELETE ON verifactu_incidents
BEGIN SELECT RAISE(ABORT, 'una incidencia de Verifactu no se elimina'); END;

CREATE TRIGGER verifactu_authorization_halts_no_update
BEFORE UPDATE ON verifactu_authorization_halts
BEGIN SELECT RAISE(ABORT, 'un paro por autorización de Verifactu no se modifica'); END;
CREATE TRIGGER verifactu_authorization_halts_no_delete
BEFORE DELETE ON verifactu_authorization_halts
BEGIN SELECT RAISE(ABORT, 'un paro por autorización de Verifactu no se elimina'); END;
