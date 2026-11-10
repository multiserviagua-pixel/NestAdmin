-- Conversaciones del asistente del chat (D188, R33). Decide: los turnos de cada conversación y,
-- por cada llamada a un proveedor de IA, el texto literal que se envió, para poder mostrarlo con
-- «Ver lo enviado». Todo esto se guarda solo en este equipo y no sale de él: no se usa para
-- sugerencias ni se envía a ningún sitio salvo, turno a turno, lo que D183 permite al proveedor
-- elegido. `meta_json` guarda datos del turno (resultado, referencias citadas, proveedor, modelo),
-- nunca credenciales. Borrar una conversación borra en cascada sus turnos y sus llamadas.
CREATE TABLE assistant_conversations (
    id         INTEGER PRIMARY KEY,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE assistant_turns (
    id              INTEGER PRIMARY KEY,
    conversation_id INTEGER NOT NULL REFERENCES assistant_conversations (id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    text            TEXT NOT NULL,
    meta_json       TEXT NOT NULL DEFAULT '{}',
    created_at      INTEGER NOT NULL
);
CREATE INDEX assistant_turns_conversation ON assistant_turns (conversation_id, id);

CREATE TABLE assistant_calls (
    id         INTEGER PRIMARY KEY,
    turn_id    INTEGER NOT NULL REFERENCES assistant_turns (id) ON DELETE CASCADE,
    step       INTEGER NOT NULL,
    provider   TEXT NOT NULL,
    model      TEXT NOT NULL,
    host       TEXT NOT NULL DEFAULT '',
    local      INTEGER NOT NULL CHECK (local IN (0, 1)),
    sent_text  TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX assistant_calls_turn ON assistant_calls (turn_id, step);
