-- Propuestas del asistente del chat (D185, R34). Decide: cada acción que el asistente PREPARA queda
-- aquí como `pending` con sus argumentos ya validados por la aplicación, la versión de la tarea que
-- vio (`seen_version`, la lee la aplicación, nunca el modelo), una clave de idempotencia propia y
-- su caducidad (24 horas). `card_json` guarda la tarjeta que se mostró (con los datos reales del
-- equipo en ese momento) y `receipt_json` el recibo original de la confirmación, para devolverlo
-- sin ejecutar nada si se confirma dos veces. Solo el botón de la tarjeta ejecuta una propuesta.
-- Todo esto se guarda solo en este equipo y no sale de él; borrar la conversación borra en cascada
-- sus propuestas. `conversation_id` es NULL solo mientras el turno que la creó no se ha guardado.
CREATE TABLE assistant_proposals (
    id              INTEGER PRIMARY KEY,
    conversation_id INTEGER REFERENCES assistant_conversations (id) ON DELETE CASCADE,
    turn_id         INTEGER,
    kind            TEXT NOT NULL CHECK (kind IN (
        'approve', 'reject', 'retry', 'archive', 'confirm_not_sent', 'edit_draft',
        'fetch_mail', 'mail_limit', 'rename_account', 'sender_preference'
    )),
    task_id         INTEGER,
    seen_version    INTEGER,
    -- Huella de lo que la tarjeta mostró y de lo que determina la acción (estado, versión y hora
    -- de la tarea, y el resumen recompuesto). Si al leer o confirmar ya no coincide, la propuesta
    -- pasa a `stale` y no se ejecuta.
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
CREATE INDEX assistant_proposals_conversation ON assistant_proposals (conversation_id, status);
CREATE INDEX assistant_proposals_task ON assistant_proposals (task_id, status);

-- Origen de cada evento del historial: NULL para los de siempre y `chat` para lo que se ejecutó al
-- confirmar una propuesta del asistente.
ALTER TABLE audit_log ADD COLUMN origin TEXT;
