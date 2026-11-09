-- Plantillas de pipeline y sus ejecuciones (D148). Ninguna tabla guarda cuerpos ni textos de
-- correo. Una plantilla nunca autoriza nada: solo ordena cuándo puede empezar una tarea.

-- Plantillas versionadas, como `rules`: editar crea una versión nueva y las ejecuciones en
-- curso conservan su copia (`pipeline_runs.steps_json`).
CREATE TABLE task_templates (
    id              TEXT NOT NULL,
    version         INTEGER NOT NULL,
    name            TEXT NOT NULL,
    conditions_json TEXT NOT NULL,
    steps_json      TEXT NOT NULL,
    active          INTEGER NOT NULL,
    archived_at     INTEGER,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (id, version)
);

-- Una ejecución por tarea principal (el correo). Sin columna de estado: se deriva de sus pasos.
CREATE TABLE pipeline_runs (
    id               INTEGER PRIMARY KEY,
    template_id      TEXT NOT NULL,
    template_version INTEGER NOT NULL,
    steps_json       TEXT NOT NULL,
    subject_task_id  INTEGER NOT NULL UNIQUE REFERENCES tasks (id),
    created_at       INTEGER NOT NULL
);

-- Pasos de una ejecución. Las etapas son lineales: una tarea espera a que todos los pasos de
-- etapas anteriores (no omitidos) tengan su tarea `completed`. `task_id` NULL es un adjunto
-- aún sin descargar, que también retiene las etapas posteriores. `task_id` UNIQUE ya crea el
-- índice por tarea.
CREATE TABLE pipeline_steps (
    id             INTEGER PRIMARY KEY,
    run_id         INTEGER NOT NULL REFERENCES pipeline_runs (id),
    stage          INTEGER NOT NULL,
    kind           TEXT NOT NULL,
    label          TEXT NOT NULL,
    attachment_key TEXT,
    task_id        INTEGER UNIQUE REFERENCES tasks (id),
    waived_at      INTEGER,
    waived_by_note TEXT,
    last_error     TEXT,
    UNIQUE (run_id, attachment_key)
);
CREATE INDEX pipeline_steps_run_stage ON pipeline_steps (run_id, stage);
