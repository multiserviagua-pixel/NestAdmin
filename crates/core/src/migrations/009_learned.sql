-- Evidencia aprendida en la primera carga (D172) y enums de estilo de respuesta (D171).
-- Ninguna tabla guarda cuerpos ni textos de correo, direcciones aparte de la clave de señal
-- (`categoría|remitente`, la misma que ya guardan las aprobaciones).

-- `rule_signals.task_id` pasa a admitir NULL: una señal `learned` nace de un correo recibido y su
-- respuesta propia leídos al aprender, no de una tarea de Pendientes, y no se crean tareas falsas.
-- SQLite no cambia la nulabilidad de una columna: se reconstruye la tabla conservando los id (nadie
-- la referencia). El CHECK mantiene la regla de antes: solo `learned` puede no tener tarea.
CREATE TABLE rule_signals_new (
    id              INTEGER PRIMARY KEY,
    kind            TEXT NOT NULL,
    action_kind     TEXT NOT NULL,
    subject_key     TEXT NOT NULL,
    edit_state      TEXT NOT NULL,
    task_id         INTEGER REFERENCES tasks (id),
    content_version INTEGER NOT NULL,
    source_key      TEXT,
    at              INTEGER NOT NULL,
    state           TEXT NOT NULL DEFAULT 'valid',
    UNIQUE (kind, task_id, content_version),
    CHECK (kind = 'learned' OR task_id IS NOT NULL)
);
INSERT INTO rule_signals_new
    (id, kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, at, state)
SELECT id, kind, action_kind, subject_key, edit_state, task_id, content_version, source_key, at, state
FROM rule_signals;
DROP TABLE rule_signals;
ALTER TABLE rule_signals_new RENAME TO rule_signals;

-- Una fila `learned` por correo recibido: `source_key` guarda la fuente y el id del mensaje. En
-- SQLite los NULL de `task_id` no chocan en el UNIQUE de arriba, así que la unicidad la da este
-- índice parcial y reanudar no duplica filas.
CREATE UNIQUE INDEX rule_signals_learned_source ON rule_signals (source_key) WHERE kind = 'learned';

-- Cuántas veces el negocio respondió de cada manera en cada categoría (solo enums). La rellena
-- el aprendizaje al terminar y `draft_request` la usa para una línea de confianza.
CREATE TABLE reply_style (
    category   TEXT NOT NULL,
    reply_kind TEXT NOT NULL,
    n          INTEGER NOT NULL,
    PRIMARY KEY (category, reply_kind)
);
