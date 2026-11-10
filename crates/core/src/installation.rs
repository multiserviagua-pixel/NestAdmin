//! Número de instalación (fase 11, migración 13): identificador estable de esta copia de
//! NestAdmin para el campo `NumeroInstalacion` del sistema informático Verifactu. Se genera la
//! primera vez que hace falta y se guarda en una tabla de una sola fila y solo inserción.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::store::now_ms;
use crate::Store;

/// Longitud del número de instalación: 16 caracteres hexadecimales en mayúsculas.
pub const INSTALLATION_ID_LEN: usize = 16;

/// Deriva un número de instalación de datos no secretos: la semilla de quien llama (por
/// ejemplo la ruta de los datos), el instante en nanosegundos y el identificador del proceso.
fn generate(seed: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(b"nestadmin-installation-v1");
    hasher.update(seed.as_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher
        .finalize()
        .iter()
        .take(INSTALLATION_ID_LEN / 2)
        .map(|b| format!("{b:02X}"))
        .collect()
}

impl Store {
    /// Número de instalación guardado, si ya se generó.
    pub fn installation_number(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT number FROM installation WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Devuelve el número de instalación y, si aún no existe, lo genera y lo guarda. Es
    /// atómico: si dos llamadas compiten (incluso desde dos procesos), ambas obtienen el mismo.
    /// `seed` aporta datos no secretos de la instalación (por ejemplo la ruta de los datos).
    pub fn ensure_installation_number(&mut self, seed: &str) -> Result<String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row("SELECT number FROM installation WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?;
        let number = match existing {
            Some(number) => number,
            None => {
                let number = generate(seed);
                tx.execute(
                    "INSERT INTO installation (id, number, created_at) VALUES (1, ?1, ?2)",
                    rusqlite::params![number, now_ms()],
                )?;
                number
            }
        };
        tx.commit()?;
        Ok(number)
    }
}
