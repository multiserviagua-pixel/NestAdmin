//! Clasificación de todas las tablas por papel (D287, D288). Es la fuente de verdad del reparto
//! entre la base principal y la de cada obligado: el traslado importa según ella y la migración 18
//! crea un trigger de papel por cada tabla `Principal` u `Obligado`. Una prueba exige que toda
//! tabla de una base migrada esté aquí y tenga su trigger; añadir una tabla sin clasificarla hace
//! fallar esa prueba.

/// Papel de una tabla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableClass {
    /// Solo se escribe en la base principal; en una base de obligado es un dato muerto (la copia
    /// de la base anterior en el perfil de pruebas) y los triggers impiden escribir en ella.
    Principal,
    /// Solo se escribe en la base de un obligado; en la principal está vacía y bloqueada.
    Obligado,
    /// Existe y se usa en todas las bases (identidad y auditoría).
    Todas,
    /// Andamiaje del traslado: solo se usa en la base anterior; en cualquier otra, y en la copia
    /// que viaja al perfil de pruebas, es un dato muerto.
    Muerta,
}

/// Tablas y su papel. Las tablas internas de SQLite (`sqlite_*`) y las del índice de texto
/// (`search_fts` y sus tablas sombra, ver [`is_derived`]) no figuran.
pub const CLASSIFICATION: &[(&str, TableClass)] = &[
    // Tareas, acciones y auditoría (el `audit_log` de ventas va a la base del obligado).
    ("tasks", TableClass::Principal),
    ("task_transitions", TableClass::Principal),
    ("actions", TableClass::Principal),
    ("audit_log", TableClass::Todas),
    // Reglas.
    ("rules", TableClass::Principal),
    ("rule_meta", TableClass::Principal),
    ("rule_evaluations", TableClass::Principal),
    ("rule_signals", TableClass::Principal),
    ("rule_suggestions", TableClass::Principal),
    ("rule_settings", TableClass::Principal),
    // Correo y redacción.
    ("sender_preferences", TableClass::Principal),
    ("business_profile", TableClass::Principal),
    ("reply_style", TableClass::Principal),
    // Plantillas y procesos.
    ("task_templates", TableClass::Principal),
    ("pipeline_runs", TableClass::Principal),
    ("pipeline_steps", TableClass::Principal),
    // Búsqueda y asistente.
    ("search_index", TableClass::Principal),
    ("assistant_conversations", TableClass::Principal),
    ("assistant_turns", TableClass::Principal),
    ("assistant_calls", TableClass::Principal),
    // Envíos de venta, con `taxpayer_id` (D287), y registro de obligados.
    ("sales_document_sends", TableClass::Principal),
    ("sales_send_triggers", TableClass::Principal),
    ("taxpayers", TableClass::Principal),
    // Empresa y datos maestros.
    ("company", TableClass::Obligado),
    ("company_logos", TableClass::Obligado),
    ("company_letterheads", TableClass::Obligado),
    ("customers", TableClass::Obligado),
    ("products", TableClass::Obligado),
    ("series", TableClass::Obligado),
    ("recurring_templates", TableClass::Obligado),
    ("recurring_skips", TableClass::Obligado),
    // Documentos de venta.
    ("sales_documents", TableClass::Obligado),
    ("sales_lines", TableClass::Obligado),
    ("document_links", TableClass::Obligado),
    ("payments", TableClass::Obligado),
    ("received_delivery_notes", TableClass::Obligado),
    ("installation", TableClass::Obligado),
    // Verifactu: cadena, entorno, remisión y su estado.
    ("verifactu_records", TableClass::Obligado),
    ("verifactu_environment_changes", TableClass::Obligado),
    ("verifactu_record_environments", TableClass::Obligado),
    ("verifactu_batches", TableClass::Obligado),
    ("verifactu_batch_records", TableClass::Obligado),
    ("verifactu_batch_attempts", TableClass::Obligado),
    ("verifactu_batch_results", TableClass::Obligado),
    ("verifactu_record_results", TableClass::Obligado),
    ("verifactu_subsanations", TableClass::Obligado),
    ("verifactu_remission_settings", TableClass::Obligado),
    ("verifactu_tls12_preferences", TableClass::Obligado),
    ("verifactu_unsent_attempts", TableClass::Obligado),
    ("verifactu_incidents", TableClass::Obligado),
    ("verifactu_authorization_halts", TableClass::Obligado),
    // Identidad y marcador del traslado.
    ("database_identity", TableClass::Todas),
    ("profile_migration", TableClass::Muerta),
];

/// Papel de una tabla, o `None` si no está clasificada.
pub fn class_of(table: &str) -> Option<TableClass> {
    CLASSIFICATION
        .iter()
        .find(|(name, _)| *name == table)
        .map(|(_, class)| *class)
}

/// Tablas de un papel, en el orden de [`CLASSIFICATION`].
pub fn tables_of(class: TableClass) -> impl Iterator<Item = &'static str> {
    CLASSIFICATION
        .iter()
        .filter(move |(_, c)| *c == class)
        .map(|(name, _)| *name)
}

/// El índice de texto `search_fts` y sus tablas sombra: se rellenan solas con los triggers de
/// `search_index` y no se clasifican, copian ni cuentan aparte.
pub fn is_derived(table: &str) -> bool {
    table == "search_fts" || table.starts_with("search_fts_")
}

/// Nombre del trigger de papel de una tabla (migración 18).
pub fn role_guard_name(table: &str) -> String {
    format!("role_guard_{table}")
}
