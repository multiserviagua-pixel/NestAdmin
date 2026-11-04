//! Registro local por cuenta de qué clave creó cada borrador y en qué estado está cada envío
//! (D33). Se escribe de forma atómica antes de devolver el control, así que sobrevive a un
//! cierre inesperado.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nestadmin_simulators::{codes, NewDraft, SendReceipt};
use serde::{Deserialize, Serialize};

use crate::{error, MailResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DraftRecord {
    pub key: String,
    pub message_id: String,
    pub content: NewDraft,
    pub draft_id: String,
    /// ID del mensaje del borrador cuando difiere del ID del borrador (Gmail).
    pub provider_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SendRecord {
    pub key: String,
    pub draft_id: String,
    /// `None` mientras el envío está pendiente de confirmar.
    pub receipt: Option<SendReceipt>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Data {
    pub drafts: Vec<DraftRecord>,
    pub sends: Vec<SendRecord>,
}

pub(crate) struct LocalState {
    path: Option<PathBuf>,
    data: Mutex<Data>,
}

impl LocalState {
    pub(crate) fn in_memory() -> Self {
        Self {
            path: None,
            data: Mutex::new(Data::default()),
        }
    }

    /// Un archivo ilegible no se sustituye por uno vacío: se perdería la memoria de envíos.
    pub(crate) fn open(path: &Path) -> MailResult<Self> {
        let data = match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|_| {
                error(
                    codes::UNAVAILABLE,
                    format!(
                        "el registro local de la cuenta está dañado: {}",
                        path.display()
                    ),
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(e) => return Err(error(codes::UNAVAILABLE, e.to_string())),
        };
        Ok(Self {
            path: Some(path.to_owned()),
            data: Mutex::new(data),
        })
    }

    pub(crate) fn read<R>(&self, f: impl FnOnce(&Data) -> R) -> MailResult<R> {
        let data = self.data.lock().map_err(|_| poisoned())?;
        Ok(f(&data))
    }

    /// Aplica el cambio y lo guarda; si no se puede guardar, el cambio se deshace.
    pub(crate) fn mutate(&self, f: impl FnOnce(&mut Data)) -> MailResult<()> {
        let mut data = self.data.lock().map_err(|_| poisoned())?;
        let before =
            serde_json::to_string(&*data).map_err(|e| error(codes::UNAVAILABLE, e.to_string()))?;
        f(&mut data);
        if let Err(e) = self.persist(&data) {
            *data = serde_json::from_str(&before).unwrap_or_default();
            return Err(e);
        }
        Ok(())
    }

    fn persist(&self, data: &Data) -> MailResult<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| error(codes::UNAVAILABLE, e.to_string()))?;
        }
        let text = serde_json::to_string_pretty(data)
            .map_err(|e| error(codes::UNAVAILABLE, e.to_string()))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, text)
            .and_then(|_| fs::rename(&temporary, path))
            .map_err(|e| {
                error(
                    codes::UNAVAILABLE,
                    format!("no se pudo guardar el registro local: {e}"),
                )
            })
    }
}

fn poisoned() -> nestadmin_core::ConnectorError {
    error(codes::UNAVAILABLE, "estado interno no disponible")
}
