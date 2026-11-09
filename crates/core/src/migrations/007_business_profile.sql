-- Perfil del negocio (D163): horario, tono, firma y políticas que la persona escribe para que la
-- redacción de respuestas los use. Fila única. Tope total de 2.000 caracteres (también aquí, para
-- que ninguna ruta lo salte). La validación de contenido (IBAN, tarjeta, claves) vive en la
-- aplicación; el consentimiento para enviarlo a un proveedor externo va aparte, en
-- `rule_settings`. La tabla `company` solo sirve para precargar el formulario y no se envía.
CREATE TABLE business_profile (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    hours        TEXT NOT NULL DEFAULT '',
    tone         TEXT NOT NULL DEFAULT '',
    signature    TEXT NOT NULL DEFAULT '',
    policies     TEXT NOT NULL DEFAULT '',
    display_name TEXT,
    updated_at   INTEGER NOT NULL,
    CHECK (length(hours) + length(tone) + length(signature) + length(policies)
           + length(coalesce(display_name, '')) <= 2000)
);
