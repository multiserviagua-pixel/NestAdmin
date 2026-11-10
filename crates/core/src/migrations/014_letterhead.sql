-- Migración 14 (D222, D223): membrete PDF de fondo de los documentos de venta. Como los
-- logotipos (migración 3), los membretes son de solo inserción y se identifican por el SHA-256 de
-- su contenido: un documento emitido cita el suyo en su copia del emisor (`issuer_snapshot`), así
-- que cambiar o quitar el membrete después no altera los ya emitidos.
CREATE TABLE company_letterheads (
    sha256      TEXT PRIMARY KEY,
    data        BLOB NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TRIGGER company_letterheads_no_update
BEFORE UPDATE ON company_letterheads
BEGIN
    SELECT RAISE(ABORT, 'los membretes son de solo inserción');
END;

CREATE TRIGGER company_letterheads_no_delete
BEFORE DELETE ON company_letterheads
BEGIN
    SELECT RAISE(ABORT, 'los membretes son de solo inserción');
END;

-- Membrete vigente de la empresa y márgenes del área útil, en milímetros enteros. Sin clave
-- ajena ni restricciones de columna para poder revertir la migración en las pruebas de esquema;
-- el contenido y el rango de los márgenes los comprueba el código (sales/master.rs).
ALTER TABLE company ADD COLUMN letterhead_sha256 TEXT;
ALTER TABLE company ADD COLUMN letterhead_top_mm INTEGER NOT NULL DEFAULT 35;
ALTER TABLE company ADD COLUMN letterhead_right_mm INTEGER NOT NULL DEFAULT 20;
ALTER TABLE company ADD COLUMN letterhead_bottom_mm INTEGER NOT NULL DEFAULT 25;
ALTER TABLE company ADD COLUMN letterhead_left_mm INTEGER NOT NULL DEFAULT 20;
