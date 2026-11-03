//! Estado compartido por los simuladores: memoria protegida por un mutex y persistencia
//! opcional en JSON. Cada cambio se aplica sobre una copia y solo se adopta tras guardarla,
//! de modo que un fallo de escritura nunca deja un efecto anunciado que no persista.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{codes, error, FailureMode, SimResult};

/// Versión del formato del archivo de estado.
const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Envelope<S> {
    version: u32,
    state: S,
}

struct Inner<S> {
    state: S,
    path: Option<PathBuf>,
    failure: Option<FailureMode>,
}

pub(crate) struct SimState<S> {
    inner: Mutex<Inner<S>>,
}

impl<S: Clone + Serialize + DeserializeOwned> SimState<S> {
    pub(crate) fn in_memory(state: S) -> Self {
        Self::with(state, None)
    }

    /// Carga el archivo si existe; si no, parte de `seed` y lo crea.
    pub(crate) fn open(path: &Path, seed: impl FnOnce() -> S) -> SimResult<Self> {
        let state = if path.exists() {
            load(path)?
        } else {
            let state = seed();
            save(path, &state)?;
            state
        };
        Ok(Self::with(state, Some(path.to_owned())))
    }

    fn with(state: S, path: Option<PathBuf>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state,
                path,
                failure: None,
            }),
        }
    }

    pub(crate) fn set_failure(&self, failure: Option<FailureMode>) {
        // Configurar el fallo debe funcionar aunque otra operación haya entrado en pánico.
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.failure = failure;
    }

    fn lock(&self) -> SimResult<MutexGuard<'_, Inner<S>>> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| error(codes::UNAVAILABLE, "estado del simulador bloqueado"))?;
        if let Some(mode) = inner.failure {
            return Err(error(mode.code(), "fallo simulado"));
        }
        Ok(inner)
    }

    pub(crate) fn read<R>(&self, f: impl FnOnce(&S) -> SimResult<R>) -> SimResult<R> {
        f(&self.lock()?.state)
    }

    /// Aplica `f` a una copia del estado. `f` devuelve el resultado y si hubo cambios;
    /// solo entonces se guarda y se adopta la copia.
    pub(crate) fn mutate<R>(&self, f: impl FnOnce(&mut S) -> SimResult<(R, bool)>) -> SimResult<R> {
        let mut inner = self.lock()?;
        let mut next = inner.state.clone();
        let (result, changed) = f(&mut next)?;
        if changed {
            if let Some(path) = &inner.path {
                save(path, &next)?;
            }
            inner.state = next;
        }
        Ok(result)
    }
}

fn load<S: DeserializeOwned>(path: &Path) -> SimResult<S> {
    let text = fs::read_to_string(path).map_err(|e| {
        error(
            codes::UNAVAILABLE,
            format!("no se pudo leer el estado: {e}"),
        )
    })?;
    let envelope: Envelope<S> = serde_json::from_str(&text)
        .map_err(|e| error(codes::UNAVAILABLE, format!("estado ilegible: {e}")))?;
    if envelope.version != FORMAT_VERSION {
        return Err(error(
            codes::UNAVAILABLE,
            format!(
                "formato de estado {} no soportado (se esperaba {FORMAT_VERSION})",
                envelope.version
            ),
        ));
    }
    Ok(envelope.state)
}

/// Escribe en un archivo temporal y lo renombra para no dejar nunca un estado a medias.
fn save<S: Serialize>(path: &Path, state: &S) -> SimResult<()> {
    let envelope = Envelope {
        version: FORMAT_VERSION,
        state,
    };
    let text = serde_json::to_string_pretty(&envelope)
        .map_err(|e| error(codes::UNAVAILABLE, format!("no se pudo serializar: {e}")))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, text)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| {
            error(
                codes::UNAVAILABLE,
                format!("no se pudo guardar el estado: {e}"),
            )
        })
}
