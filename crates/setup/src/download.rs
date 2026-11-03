//! Descarga reanudable con comprobación SHA-256. El archivo parcial (`.part`) se conserva
//! tras un corte y el reintento pide solo lo que falta con `Range`; solo se adopta el
//! archivo cuando el hash coincide.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use ureq::Agent;

use crate::{Progress, SetupError};

/// Archivo fijado: URL oficial, tamaño aproximado y SHA-256 esperado.
#[derive(Debug, Clone)]
pub struct Pinned {
    pub url: String,
    pub file_name: String,
    pub sha256: String,
}

pub fn agent() -> Agent {
    Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}

fn sha256_of(path: &Path) -> Result<String, SetupError> {
    let mut file = File::open(path).map_err(SetupError::io)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(SetupError::io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Descarga `pinned` en `dir` y devuelve la ruta final. Reutiliza un archivo ya completo
/// y comprobado, y reanuda uno parcial.
pub fn download(
    agent: &Agent,
    pinned: &Pinned,
    dir: &Path,
    component: &str,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, SetupError> {
    fs::create_dir_all(dir).map_err(SetupError::io)?;
    let target = dir.join(&pinned.file_name);
    let expected = pinned.sha256.to_ascii_lowercase();
    if target.is_file() && sha256_of(&target)? == expected {
        return Ok(target);
    }
    let partial = dir.join(format!("{}.part", pinned.file_name));
    let offset = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);

    let mut request = agent.get(&pinned.url);
    if offset > 0 {
        request = request.header("Range", &format!("bytes={offset}-"));
    }
    let response = request
        .call()
        .map_err(|e| SetupError::Network(format!("no se pudo conectar ({})", short(&e))))?;
    let status = response.status().as_u16();
    let (mut file, mut done) = match status {
        206 => (
            OpenOptions::new()
                .append(true)
                .open(&partial)
                .map_err(SetupError::io)?,
            offset,
        ),
        200 => (File::create(&partial).map_err(SetupError::io)?, 0),
        // El rango ya estaba completo: solo falta comprobar el hash.
        416 => (
            OpenOptions::new()
                .append(true)
                .open(&partial)
                .map_err(SetupError::io)?,
            offset,
        ),
        other => return Err(SetupError::Network(format!("estado HTTP {other}"))),
    };
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(|len| len + done);
    if status != 416 {
        let mut body = response.into_body().into_reader();
        let mut buffer = vec![0u8; 1 << 16];
        let mut last_report = 0u64;
        loop {
            let read = body.read(&mut buffer).map_err(|e| {
                SetupError::Network(format!(
                    "descarga interrumpida tras {done} bytes ({}); se reanudará al reintentar",
                    e.kind()
                ))
            })?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read]).map_err(SetupError::io)?;
            done += read as u64;
            if done - last_report >= 1 << 20 || Some(done) == total {
                last_report = done;
                progress(Progress::bytes(component, "Descargando", done, total));
            }
        }
    }
    file.sync_all().map_err(SetupError::io)?;
    drop(file);
    if let Some(total) = total {
        if done < total {
            return Err(SetupError::Network(format!(
                "descarga incompleta ({done} de {total} bytes); se reanudará al reintentar"
            )));
        }
    }
    progress(Progress::step(component, "Comprobando la integridad"));
    let actual = sha256_of(&partial)?;
    if actual != expected {
        let _ = fs::remove_file(&partial);
        return Err(SetupError::Integrity(format!(
            "{}: SHA-256 {actual} distinto del esperado; se descartó la descarga",
            pinned.file_name
        )));
    }
    fs::rename(&partial, &target).map_err(SetupError::io)?;
    Ok(target)
}

fn short(error: &ureq::Error) -> String {
    format!("{error:?}")
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or("error")
        .to_owned()
}
