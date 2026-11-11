-- Reglas, sugerencias e importación desde el asistente (D191, etapa E4): amplía el CHECK de `kind` de
-- `assistant_proposals` con `accept_suggestion`, `discard_suggestion`, `create_rule`, `edit_rule` e
-- `import`. SQLite no cambia un CHECK en su sitio: se reconstruye la tabla con las mismas columnas,
-- restricciones e índices y se copian todas las filas (también las que ya están confirmadas, con su
-- recibo), de modo que ninguna propuesta anterior se pierde ni cambia. Nada referencia esta tabla.
-- `AUTOINCREMENT`: el identificador de una propuesta no se reutiliza tras borrar conversaciones, de
-- modo que nada derivado de él (el recibo, un identificador de regla) pueda apuntar a otra cosa.
CREATE TABLE assistant_proposals_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER REFERENCES assistant_conversations (id) ON DELETE CASCADE,
    turn_id         INTEGER,
    kind            TEXT NOT NULL CHECK (kind IN (
        'approve', 'reject', 'retry', 'archive', 'confirm_not_sent', 'edit_draft',
        'fetch_mail', 'mail_limit', 'rename_account', 'sender_preference',
        'accept_suggestion', 'discard_suggestion', 'create_rule', 'edit_rule', 'import'
    )),
    task_id         INTEGER,
    seen_version    INTEGER,
    seen_fp         TEXT,
    args_json       TEXT NOT NULL,
    card_json       TEXT NOT NULL,
    idem_key        TEXT NOT NULL UNIQUE,
    status          TEXT NOT NULL DEFAULT 'pending' CHECK (status IN (
        'pending', 'executing', 'confirmed', 'rejected', 'expired', 'stale', 'failed'
    )),
    receipt_json    TEXT,
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,
    resolved_at     INTEGER
);
INSERT INTO assistant_proposals_new
    (id, conversation_id, turn_id, kind, task_id, seen_version, seen_fp, args_json, card_json,
     idem_key, status, receipt_json, created_at, expires_at, resolved_at)
SELECT id, conversation_id, turn_id, kind, task_id, seen_version, seen_fp, args_json, card_json,
       idem_key, status, receipt_json, created_at, expires_at, resolved_at
FROM assistant_proposals;
DROP TABLE assistant_proposals;
ALTER TABLE assistant_proposals_new RENAME TO assistant_proposals;
CREATE INDEX assistant_proposals_conversation ON assistant_proposals (conversation_id, status);
CREATE INDEX assistant_proposals_task ON assistant_proposals (task_id, status);
