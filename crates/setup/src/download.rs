//! Descarga reanudable con comprobación SHA-256. El archivo parcial (`.part`) se conserva
//! tras un corte y el reintento pide solo lo que falta con `Range`; solo se adopta el
//! archivo cuando el hash coincide.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use ureq::Agent;

use crate::{Cancel, ErrorCode, SetupError};

/// Archivo fijado: URL oficial y SHA-256 esperado.
#[derive(Debug, Clone)]
pub struct Pinned {
    pub url: String,
    pub file_name: String,
    pub sha256: String,
}

/// Cliente para descargas de componentes: sin proxy implícito (D12, D37) y con plazos.
pub fn agent() -> Agent {
    Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .proxy(None)
        .build()
        .into()
}

fn io(error: std::io::Error) -> SetupError {
    SetupError::new(ErrorCode::Network, format!("archivo: {}", error.kind()))
}

fn sha256_of(path: &Path) -> Result<String, SetupError> {
    let mut file = File::open(path).map_err(io)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(io)?;
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
/// y comprobado y reanuda uno parcial. `progress` recibe bytes descargados y total.
pub fn download(
    agent: &Agent,
    pinned: &Pinned,
    dir: &Path,
    cancel: &Cancel,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<PathBuf, SetupError> {
    fs::create_dir_all(dir).map_err(io)?;
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
    let response = request.call().map_err(|_| {
        SetupError::new(
            ErrorCode::Network,
            "no se pudo conectar con el servidor de descarga",
        )
    })?;
    let status = response.status().as_u16();
    let (mut file, mut done) = match status {
        206 | 416 => (
            OpenOptions::new().append(true).open(&partial).map_err(io)?,
            offset,
        ),
        200 => (File::create(&partial).map_err(io)?, 0),
        other => {
            return Err(SetupError::new(
                ErrorCode::Network,
                format!("el servidor de descarga respondió {other}"),
            ))
        }
    };
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(|len| len + done);
    // 416: el rango ya estaba completo; solo falta comprobar el hash.
    if status != 416 {
        let mut body = response.into_body().into_reader();
        let mut buffer = vec![0u8; 1 << 16];
        let mut last_report = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(SetupError::cancelled());
            }
            let read = body.read(&mut buffer).map_err(|e| {
                SetupError::new(
                    ErrorCode::Network,
                    format!("descarga interrumpida tras {done} bytes ({})", e.kind()),
                )
            })?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read]).map_err(io)?;
            done += read as u64;
            if done - last_report >= 1 << 20 || Some(done) == total {
                last_report = done;
                progress(done, total);
            }
        }
    }
    file.sync_all().map_err(io)?;
    drop(file);
    if let Some(total) = total {
        if done < total {
            return Err(SetupError::new(
                ErrorCode::Network,
                format!("descarga incompleta ({done} de {total} bytes)"),
            ));
        }
    }
    if sha256_of(&partial)? != expected {
        let _ = fs::remove_file(&partial);
        return Err(SetupError::new(
            ErrorCode::Signature,
            format!(
                "{}: la huella SHA-256 no coincide con la fijada; se descartó",
                pinned.file_name
            ),
        ));
    }
    fs::rename(&partial, &target).map_err(io)?;
    Ok(target)
}
