-- Índice local de búsqueda (D179, R32). Decide: una fila por factura/documento o correo
-- (`task_id` único), con los campos para filtrar y el texto para buscar, más una tabla FTS5 de
-- contenido externo sobre título, proveedor/remitente, número y texto, mantenida por triggers.
-- No guarda nada que la tarea no guardase ya: el texto de un documento es el extraído en local
-- (antes se descartaba tras la petición a la IA) y de un correo solo lo que la tarea conserva;
-- `detail` es el motivo corto de un fallo, nunca contenido del documento.
CREATE TABLE search_index (
    task_id     INTEGER PRIMARY KEY REFERENCES tasks (id),
    kind        TEXT NOT NULL CHECK (kind IN ('invoice', 'email')),
    status      TEXT NOT NULL CHECK (status IN ('indexed', 'no_text', 'failed')),
    title       TEXT NOT NULL DEFAULT '',
    party       TEXT NOT NULL DEFAULT '',
    number      TEXT NOT NULL DEFAULT '',
    doc_date    TEXT,
    total_minor INTEGER,
    currency    TEXT NOT NULL DEFAULT '',
    body        TEXT NOT NULL DEFAULT '',
    detail      TEXT NOT NULL DEFAULT '',
    indexed_at  INTEGER NOT NULL
);

CREATE VIRTUAL TABLE search_fts USING fts5 (
    title, party, number, body,
    content = 'search_index', content_rowid = 'task_id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER search_index_ai AFTER INSERT ON search_index BEGIN
    INSERT INTO search_fts (rowid, title, party, number, body)
    VALUES (new.task_id, new.title, new.party, new.number, new.body);
END;

CREATE TRIGGER search_index_ad AFTER DELETE ON search_index BEGIN
    INSERT INTO search_fts (search_fts, rowid, title, party, number, body)
    VALUES ('delete', old.task_id, old.title, old.party, old.number, old.body);
END;

CREATE TRIGGER search_index_au AFTER UPDATE ON search_index BEGIN
    INSERT INTO search_fts (search_fts, rowid, title, party, number, body)
    VALUES ('delete', old.task_id, old.title, old.party, old.number, old.body);
    INSERT INTO search_fts (rowid, title, party, number, body)
    VALUES (new.task_id, new.title, new.party, new.number, new.body);
END;
