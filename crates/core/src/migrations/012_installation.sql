-- Número de instalación (fase 11): identifica esta copia de NestAdmin en el campo
-- `NumeroInstalacion` del sistema informático de los registros Verifactu. Se genera una sola
-- vez, es de una sola fila y de solo inserción: nunca cambia ni se borra (triggers).
CREATE TABLE installation (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    -- 16 caracteres hexadecimales en mayúsculas.
    number     TEXT NOT NULL CHECK (length(number) = 16),
    created_at INTEGER NOT NULL
);

CREATE TRIGGER installation_no_update
BEFORE UPDATE ON installation
BEGIN
    SELECT RAISE(ABORT, 'el número de instalación no se modifica');
END;

CREATE TRIGGER installation_no_delete
BEFORE DELETE ON installation
BEGIN
    SELECT RAISE(ABORT, 'el número de instalación no se borra');
END;
