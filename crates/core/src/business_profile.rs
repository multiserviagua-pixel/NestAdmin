//! Perfil del negocio (D163): cinco textos que la persona escribe y que la redacción de
//! respuestas puede usar. Aquí solo se guardan y se leen; la validación del contenido y el
//! consentimiento viven en la aplicación. Cada cambio deja un evento del historial en la misma
//! transacción, con el tamaño y nunca el texto.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::{now_ms, Store};

/// Tope total de caracteres de los cinco campos.
pub const MAX_PROFILE_CHARS: usize = 2000;

/// Textos del perfil. Un campo en blanco es un campo sin rellenar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusinessProfile {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub hours: String,
    #[serde(default)]
    pub tone: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub policies: String,
}

impl BusinessProfile {
    /// Caracteres de los cinco campos.
    pub fn chars(&self) -> usize {
        self.fields().iter().map(|(_, v)| v.chars().count()).sum()
    }

    /// Ningún campo tiene texto.
    pub fn is_blank(&self) -> bool {
        self.fields().iter().all(|(_, v)| v.trim().is_empty())
    }

    /// Campos con su nombre estable, en el orden en que se muestran y se envían.
    pub fn fields(&self) -> [(&'static str, &str); 5] {
        [
            ("display_name", &self.display_name),
            ("hours", &self.hours),
            ("tone", &self.tone),
            ("signature", &self.signature),
            ("policies", &self.policies),
        ]
    }
}

impl Store {
    /// Perfil guardado y cuándo, o `None` si nunca se guardó (o se borró).
    pub fn business_profile(&self) -> Result<Option<(BusinessProfile, i64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT display_name, hours, tone, signature, policies, updated_at FROM business_profile WHERE id = 1",
                [],
                |r| {
                    Ok((
                        BusinessProfile {
                            display_name: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                            hours: r.get(1)?,
                            tone: r.get(2)?,
                            signature: r.get(3)?,
                            policies: r.get(4)?,
                        },
                        r.get(5)?,
                    ))
                },
            )
            .optional()?)
    }

    /// Guarda (o sustituye) el perfil con su evento del historial, en una transacción. El tope
    /// también lo exige la tabla.
    pub fn save_business_profile(&mut self, profile: &BusinessProfile) -> Result<()> {
        let tx = self.conn.transaction()?;
        let at = now_ms();
        tx.execute(
            "INSERT INTO business_profile (id, hours, tone, signature, policies, display_name, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET hours = excluded.hours, tone = excluded.tone,
                 signature = excluded.signature, policies = excluded.policies,
                 display_name = excluded.display_name, updated_at = excluded.updated_at",
            params![
                profile.hours,
                profile.tone,
                profile.signature,
                profile.policies,
                Some(profile.display_name.as_str()).filter(|n| !n.is_empty()),
                at
            ],
        )?;
        tx.execute(
            "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'business_profile', 'saved', NULL, ?2)",
            params![
                at,
                format!("perfil del negocio guardado ({} caracteres)", profile.chars())
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Borra el perfil. Devuelve si existía; si no, no escribe nada.
    pub fn clear_business_profile(&mut self) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let removed = tx.execute("DELETE FROM business_profile WHERE id = 1", [])? > 0;
        if removed {
            tx.execute(
                "INSERT INTO audit_log (at, task_id, action, status, authorization, detail) VALUES (?1, NULL, 'business_profile', 'removed', NULL, 'perfil del negocio borrado')",
                params![now_ms()],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }
}
