-- Migración 18 (D287-D291): identidad de cada base, perfiles de datos y traslado.
--
-- * `database_identity`: una fila inmutable por base con el perfil (normal o pruebas), el papel
--   (principal u obligado) y, en un obligado, su identificador (D288). Las bases anteriores no la
--   tienen: sus guardas de papel no actúan y siguen funcionando igual hasta el traslado.
-- * `taxpayers`: registro de obligados de la base principal, de solo inserción, con altas y
--   bajas (D273, D274).
-- * `profile_migration`: marcador del traslado de la base anterior; su paso solo avanza (D289).
-- * `sales_document_sends` y `sales_send_triggers` se reconstruyen con `taxpayer_id` y sin la
--   clave foránea hacia `sales_documents`, que pasa a vivir en la base de cada obligado (D287).

CREATE TABLE database_identity (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    profile     TEXT NOT NULL CHECK (profile IN ('normal', 'pruebas')),
    role        TEXT NOT NULL CHECK (role IN ('principal', 'obligado')),
    taxpayer_id INTEGER,
    created_at  INTEGER NOT NULL,
    CHECK (
        (role = 'principal' AND taxpayer_id IS NULL)
        OR (role = 'obligado' AND taxpayer_id IS NOT NULL AND taxpayer_id >= 1)
    )
);

CREATE TRIGGER database_identity_no_update
BEFORE UPDATE ON database_identity
BEGIN SELECT RAISE(ABORT, 'la identidad de una base no se modifica'); END;
CREATE TRIGGER database_identity_no_delete
BEFORE DELETE ON database_identity
BEGIN SELECT RAISE(ABORT, 'la identidad de una base no se elimina'); END;

CREATE TABLE taxpayers (
    id          INTEGER PRIMARY KEY,
    taxpayer_id INTEGER NOT NULL CHECK (taxpayer_id >= 1),
    event       TEXT NOT NULL CHECK (event IN ('alta', 'baja')),
    label       TEXT NOT NULL DEFAULT '',
    -- Carpeta del obligado, relativa a la del perfil (`obligados/<id>`).
    location    TEXT NOT NULL,
    at          INTEGER NOT NULL
);

-- Un alta y, como mucho, una baja por obligado; la baja exige el alta.
CREATE UNIQUE INDEX taxpayers_alta ON taxpayers (taxpayer_id) WHERE event = 'alta';
CREATE UNIQUE INDEX taxpayers_baja ON taxpayers (taxpayer_id) WHERE event = 'baja';

CREATE TRIGGER taxpayers_baja_needs_alta
BEFORE INSERT ON taxpayers
WHEN NEW.event = 'baja'
 AND NOT EXISTS (SELECT 1 FROM taxpayers WHERE taxpayer_id = NEW.taxpayer_id AND event = 'alta')
BEGIN SELECT RAISE(ABORT, 'no se da de baja a un obligado que no está de alta'); END;
CREATE TRIGGER taxpayers_no_update
BEFORE UPDATE ON taxpayers
BEGIN SELECT RAISE(ABORT, 'el registro de obligados es de solo inserción'); END;
CREATE TRIGGER taxpayers_no_delete
BEFORE DELETE ON taxpayers
BEGIN SELECT RAISE(ABORT, 'el registro de obligados es de solo inserción'); END;

-- Marcador del traslado (D289): vive en la base anterior; `step` es el último paso completado.
CREATE TABLE profile_migration (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    step         INTEGER NOT NULL CHECK (step BETWEEN 1 AND 7),
    decision     TEXT CHECK (decision IS NULL OR decision IN ('real', 'ficticia')),
    counts_json  TEXT NOT NULL CHECK (json_valid(counts_json)),
    chain_state  TEXT NOT NULL CHECK (chain_state IN ('empty', 'ok', 'broken')),
    chain_head   TEXT,
    chain_digest TEXT NOT NULL,
    started_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);

CREATE TRIGGER profile_migration_step_forward
BEFORE UPDATE ON profile_migration
WHEN NEW.step < OLD.step
  OR NEW.counts_json <> OLD.counts_json
  OR NEW.chain_state <> OLD.chain_state
  OR NEW.chain_head IS NOT OLD.chain_head
  OR NEW.chain_digest <> OLD.chain_digest
  OR NEW.started_at <> OLD.started_at
  OR (OLD.decision IS NOT NULL AND NEW.decision IS NOT OLD.decision)
BEGIN SELECT RAISE(ABORT, 'el marcador del traslado solo avanza'); END;
CREATE TRIGGER profile_migration_no_delete
BEFORE DELETE ON profile_migration
BEGIN SELECT RAISE(ABORT, 'el marcador del traslado no se elimina'); END;

-- Envíos de venta con obligado (D287). Las filas existentes son del obligado 1.
ALTER TABLE sales_document_sends RENAME TO sales_document_sends_old;
ALTER TABLE sales_send_triggers RENAME TO sales_send_triggers_old;

CREATE TABLE sales_document_sends (
    id                INTEGER PRIMARY KEY,
    taxpayer_id       INTEGER NOT NULL DEFAULT 1 CHECK (taxpayer_id >= 1),
    document_id       INTEGER NOT NULL,
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
    authorization     TEXT NOT NULL,
    sent_message_id   TEXT,
    sent_at           INTEGER NOT NULL
);

INSERT INTO sales_document_sends
    (id, taxpayer_id, document_id, task_id, version, idempotency_key, account, account_address,
     recipient, subject, attachment_name, attachment_size, attachment_sha256, authorization,
     sent_message_id, sent_at)
SELECT id, 1, document_id, task_id, version, idempotency_key, account, account_address,
       recipient, subject, attachment_name, attachment_size, attachment_sha256, authorization,
       sent_message_id, sent_at
FROM sales_document_sends_old;

CREATE TABLE sales_send_triggers (
    taxpayer_id INTEGER NOT NULL DEFAULT 1 CHECK (taxpayer_id >= 1),
    document_id INTEGER NOT NULL,
    task_id     INTEGER NOT NULL REFERENCES tasks (id),
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (taxpayer_id, document_id)
);

INSERT INTO sales_send_triggers (taxpayer_id, document_id, task_id, created_at)
SELECT 1, document_id, task_id, created_at FROM sales_send_triggers_old;

-- Los recuentos se conservan o la migración entera se deshace.
CREATE TEMP TABLE migration18_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO migration18_check
SELECT (SELECT count(*) FROM sales_document_sends_old) = (SELECT count(*) FROM sales_document_sends);
INSERT INTO migration18_check
SELECT (SELECT count(*) FROM sales_send_triggers_old) = (SELECT count(*) FROM sales_send_triggers);
DROP TABLE migration18_check;

DROP TABLE sales_document_sends_old;
DROP TABLE sales_send_triggers_old;

CREATE INDEX sales_document_sends_document
    ON sales_document_sends (taxpayer_id, document_id, sent_at);

-- Los triggers de inalterabilidad de la migración 15, de nuevo sobre las tablas reconstruidas.
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

-- Guardas de papel (D288): una base con identidad rechaza escribir en las tablas del otro papel.
-- Sin identidad (las bases anteriores al traslado) no actúan y todo sigue como antes.

-- Tablas del papel principal: no se escriben en la base de un obligado.

CREATE TRIGGER role_guard_tasks
BEFORE INSERT ON tasks
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: tasks es de la base principal'); END;

CREATE TRIGGER role_guard_task_transitions
BEFORE INSERT ON task_transitions
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: task_transitions es de la base principal'); END;

CREATE TRIGGER role_guard_actions
BEFORE INSERT ON actions
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: actions es de la base principal'); END;

CREATE TRIGGER role_guard_rules
BEFORE INSERT ON rules
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: rules es de la base principal'); END;

CREATE TRIGGER role_guard_rule_meta
BEFORE INSERT ON rule_meta
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: rule_meta es de la base principal'); END;

CREATE TRIGGER role_guard_rule_evaluations
BEFORE INSERT ON rule_evaluations
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: rule_evaluations es de la base principal'); END;

CREATE TRIGGER role_guard_rule_signals
BEFORE INSERT ON rule_signals
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: rule_signals es de la base principal'); END;

CREATE TRIGGER role_guard_rule_suggestions
BEFORE INSERT ON rule_suggestions
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: rule_suggestions es de la base principal'); END;

CREATE TRIGGER role_guard_rule_settings
BEFORE INSERT ON rule_settings
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: rule_settings es de la base principal'); END;

CREATE TRIGGER role_guard_sender_preferences
BEFORE INSERT ON sender_preferences
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: sender_preferences es de la base principal'); END;

CREATE TRIGGER role_guard_business_profile
BEFORE INSERT ON business_profile
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: business_profile es de la base principal'); END;

CREATE TRIGGER role_guard_reply_style
BEFORE INSERT ON reply_style
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: reply_style es de la base principal'); END;

CREATE TRIGGER role_guard_task_templates
BEFORE INSERT ON task_templates
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: task_templates es de la base principal'); END;

CREATE TRIGGER role_guard_pipeline_runs
BEFORE INSERT ON pipeline_runs
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: pipeline_runs es de la base principal'); END;

CREATE TRIGGER role_guard_pipeline_steps
BEFORE INSERT ON pipeline_steps
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: pipeline_steps es de la base principal'); END;

CREATE TRIGGER role_guard_search_index
BEFORE INSERT ON search_index
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: search_index es de la base principal'); END;

CREATE TRIGGER role_guard_assistant_conversations
BEFORE INSERT ON assistant_conversations
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: assistant_conversations es de la base principal'); END;

CREATE TRIGGER role_guard_assistant_turns
BEFORE INSERT ON assistant_turns
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: assistant_turns es de la base principal'); END;

CREATE TRIGGER role_guard_assistant_calls
BEFORE INSERT ON assistant_calls
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: assistant_calls es de la base principal'); END;

CREATE TRIGGER role_guard_sales_document_sends
BEFORE INSERT ON sales_document_sends
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: sales_document_sends es de la base principal'); END;

CREATE TRIGGER role_guard_sales_send_triggers
BEFORE INSERT ON sales_send_triggers
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: sales_send_triggers es de la base principal'); END;

CREATE TRIGGER role_guard_taxpayers
BEFORE INSERT ON taxpayers
WHEN (SELECT role FROM database_identity) = 'obligado'
BEGIN SELECT RAISE(ABORT, 'wrong_database: taxpayers es de la base principal'); END;

-- Tablas del papel de obligado: no se escriben en la base principal.

CREATE TRIGGER role_guard_company
BEFORE INSERT ON company
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: company es de la base de un obligado'); END;

CREATE TRIGGER role_guard_company_logos
BEFORE INSERT ON company_logos
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: company_logos es de la base de un obligado'); END;

CREATE TRIGGER role_guard_company_letterheads
BEFORE INSERT ON company_letterheads
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: company_letterheads es de la base de un obligado'); END;

CREATE TRIGGER role_guard_customers
BEFORE INSERT ON customers
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: customers es de la base de un obligado'); END;

CREATE TRIGGER role_guard_products
BEFORE INSERT ON products
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: products es de la base de un obligado'); END;

CREATE TRIGGER role_guard_series
BEFORE INSERT ON series
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: series es de la base de un obligado'); END;

CREATE TRIGGER role_guard_recurring_templates
BEFORE INSERT ON recurring_templates
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: recurring_templates es de la base de un obligado'); END;

CREATE TRIGGER role_guard_recurring_skips
BEFORE INSERT ON recurring_skips
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: recurring_skips es de la base de un obligado'); END;

CREATE TRIGGER role_guard_sales_documents
BEFORE INSERT ON sales_documents
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: sales_documents es de la base de un obligado'); END;

CREATE TRIGGER role_guard_sales_lines
BEFORE INSERT ON sales_lines
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: sales_lines es de la base de un obligado'); END;

CREATE TRIGGER role_guard_document_links
BEFORE INSERT ON document_links
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: document_links es de la base de un obligado'); END;

CREATE TRIGGER role_guard_payments
BEFORE INSERT ON payments
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: payments es de la base de un obligado'); END;

CREATE TRIGGER role_guard_received_delivery_notes
BEFORE INSERT ON received_delivery_notes
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: received_delivery_notes es de la base de un obligado'); END;

CREATE TRIGGER role_guard_installation
BEFORE INSERT ON installation
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: installation es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_records
BEFORE INSERT ON verifactu_records
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_records es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_environment_changes
BEFORE INSERT ON verifactu_environment_changes
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_environment_changes es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_record_environments
BEFORE INSERT ON verifactu_record_environments
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_record_environments es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_batches
BEFORE INSERT ON verifactu_batches
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_batches es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_batch_records
BEFORE INSERT ON verifactu_batch_records
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_batch_records es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_batch_attempts
BEFORE INSERT ON verifactu_batch_attempts
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_batch_attempts es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_batch_results
BEFORE INSERT ON verifactu_batch_results
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_batch_results es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_record_results
BEFORE INSERT ON verifactu_record_results
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_record_results es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_subsanations
BEFORE INSERT ON verifactu_subsanations
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_subsanations es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_remission_settings
BEFORE INSERT ON verifactu_remission_settings
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_remission_settings es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_tls12_preferences
BEFORE INSERT ON verifactu_tls12_preferences
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_tls12_preferences es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_unsent_attempts
BEFORE INSERT ON verifactu_unsent_attempts
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_unsent_attempts es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_incidents
BEFORE INSERT ON verifactu_incidents
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_incidents es de la base de un obligado'); END;

CREATE TRIGGER role_guard_verifactu_authorization_halts
BEFORE INSERT ON verifactu_authorization_halts
WHEN (SELECT role FROM database_identity) = 'principal'
BEGIN SELECT RAISE(ABORT, 'wrong_database: verifactu_authorization_halts es de la base de un obligado'); END;

-- Entornos por perfil (D271, D275): en pruebas no existe `produccion` y en el perfil normal no
-- existe ningún otro entorno. `produccion_heredado` (D258) tampoco cabe en el perfil normal.
CREATE TRIGGER identity_env_changes_pruebas
BEFORE INSERT ON verifactu_environment_changes
WHEN (SELECT profile FROM database_identity) = 'pruebas' AND NEW.environment = 'produccion'
BEGIN SELECT RAISE(ABORT, 'un perfil de pruebas no admite el entorno de producción'); END;
CREATE TRIGGER identity_env_changes_normal
BEFORE INSERT ON verifactu_environment_changes
WHEN (SELECT profile FROM database_identity) = 'normal' AND NEW.environment <> 'produccion'
BEGIN SELECT RAISE(ABORT, 'el perfil normal solo admite el entorno de producción'); END;

CREATE TRIGGER identity_batches_pruebas
BEFORE INSERT ON verifactu_batches
WHEN (SELECT profile FROM database_identity) = 'pruebas' AND NEW.environment = 'produccion'
BEGIN SELECT RAISE(ABORT, 'un perfil de pruebas no admite el entorno de producción'); END;
CREATE TRIGGER identity_batches_normal
BEFORE INSERT ON verifactu_batches
WHEN (SELECT profile FROM database_identity) = 'normal' AND NEW.environment <> 'produccion'
BEGIN SELECT RAISE(ABORT, 'el perfil normal solo admite el entorno de producción'); END;

CREATE TRIGGER identity_record_env_pruebas
BEFORE INSERT ON verifactu_record_environments
WHEN (SELECT profile FROM database_identity) = 'pruebas' AND NEW.environment = 'produccion'
BEGIN SELECT RAISE(ABORT, 'un perfil de pruebas no admite el entorno de producción'); END;
CREATE TRIGGER identity_record_env_normal
BEFORE INSERT ON verifactu_record_environments
WHEN (SELECT profile FROM database_identity) = 'normal' AND NEW.environment <> 'produccion'
BEGIN SELECT RAISE(ABORT, 'el perfil normal solo admite el entorno de producción'); END;
