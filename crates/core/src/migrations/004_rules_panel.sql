-- Panel de reglas (D124, D126, D128, D129). Ninguna tabla guarda cuerpos ni textos de correo.

-- Datos de una regla que no son sus condiciones: nombre visible (D124), archivado (D128),
-- origen de la regla (reservado) y validación avanzada sin remitente (D126).
CREATE TABLE rule_meta (
    rule_id                 TEXT PRIMARY KEY,
    name                    TEXT,
    archived_at             INTEGER,
    origin_json             TEXT,
    allow_any_sender_ack_at INTEGER
);

-- Una evaluación de reglas por tarea, versión de contenido y huella de reglas (D129).
CREATE TABLE rule_evaluations (
    id              INTEGER PRIMARY KEY,
    task_id         INTEGER NOT NULL REFERENCES tasks (id),
    content_version INTEGER NOT NULL,
    action_kind     TEXT NOT NULL,
    outcome         TEXT NOT NULL,
    reason_code     TEXT NOT NULL,
    rules_fp        TEXT NOT NULL,
    checks_json     TEXT NOT NULL,
    at              INTEGER NOT NULL,
    UNIQUE (task_id, content_version, rules_fp)
);

-- Aprobar, editar o rechazar: lo que enseña qué reglas conviene sugerir (D129).
CREATE TABLE rule_signals (
    id              INTEGER PRIMARY KEY,
    kind            TEXT NOT NULL,
    action_kind     TEXT NOT NULL,
    subject_key     TEXT NOT NULL,
    edit_state      TEXT NOT NULL,
    task_id         INTEGER NOT NULL REFERENCES tasks (id),
    content_version INTEGER NOT NULL,
    source_key      TEXT,
    at              INTEGER NOT NULL,
    -- `valid`, o `pending`/`void` en una aprobación cuya acción aún no consta confirmada o
    -- no llegó a ejecutarse: las sugerencias solo cuentan las `valid` (D129).
    state           TEXT NOT NULL DEFAULT 'valid',
    UNIQUE (kind, task_id, content_version)
);

-- Reservadas para las sugerencias (etapa posterior); sin uso todavía.
CREATE TABLE rule_suggestions (
    fingerprint  TEXT PRIMARY KEY,
    status       TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    model_json   TEXT,
    decided_at   INTEGER
);

CREATE TABLE rule_settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Regla y versión que autorizaron la acción (NULL: aprobación o fila anterior a la migración).
ALTER TABLE actions ADD COLUMN rule_id TEXT;
ALTER TABLE actions ADD COLUMN rule_version INTEGER;

-- Reglas activas cuyas condiciones llevan `currency`, `sender` o `sender_domain`: hasta ahora
-- esas claves eran desconocidas y la regla nunca coincidía (D123); con D126 y D127 pasarían a
-- coincidir sin que nadie las hubiera revisado. Su última versión se sustituye por otra
-- inactiva con las mismas condiciones; reactivarla es una decisión explícita. Las reglas de
-- envío sin remitente no se tocan: fallan cerradas (D126).
INSERT INTO audit_log (at, task_id, action, status, authorization, detail)
SELECT CAST(strftime('%s', 'now') AS INTEGER) * 1000, NULL, 'rule', 'inactive', NULL,
       r.id || ' v' || (r.version + 1) || ' (' || r.action_kind || ') desactivada al migrar: sus condiciones ahora se aplican; revíselas y actívela'
FROM rules r
WHERE r.version = (SELECT MAX(version) FROM rules WHERE id = r.id)
  AND r.active = 1
  AND CASE WHEN json_valid(r.conditions)
           THEN json_type(r.conditions, '$.currency') IS NOT NULL
                OR json_type(r.conditions, '$.sender') IS NOT NULL
                OR json_type(r.conditions, '$.sender_domain') IS NOT NULL
           ELSE 0 END;

INSERT INTO rules (id, version, action_kind, conditions, active, created_at)
SELECT r.id, r.version + 1, r.action_kind, r.conditions, 0,
       CAST(strftime('%s', 'now') AS INTEGER) * 1000
FROM rules r
WHERE r.version = (SELECT MAX(version) FROM rules WHERE id = r.id)
  AND r.active = 1
  AND CASE WHEN json_valid(r.conditions)
           THEN json_type(r.conditions, '$.currency') IS NOT NULL
                OR json_type(r.conditions, '$.sender') IS NOT NULL
                OR json_type(r.conditions, '$.sender_domain') IS NOT NULL
           ELSE 0 END;
