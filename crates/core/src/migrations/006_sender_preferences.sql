-- Preferencias por remitente (D161): «Siempre a Pendientes» o «Siempre a Informativos», por
-- dirección o por dominio. Las fija la persona con un clic explícito; nada las aprende. La clave
-- ya viene normalizada en minúsculas (el CHECK lo exige). Solo dirección o dominio y la ruta:
-- nunca contenido de un correo.
CREATE TABLE sender_preferences (
    scope      TEXT NOT NULL CHECK (scope IN ('address', 'domain')),
    key        TEXT NOT NULL CHECK (key <> '' AND key = lower(key) AND key = trim(key)),
    route      TEXT NOT NULL CHECK (route IN ('review', 'informative')),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);
