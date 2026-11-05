//! OCR local. `Tesseract` ejecuta el binario empaquetado como proceso hijo, sin red; el trait
//! permite sustituirlo en pruebas.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::ExtractError;

/// Texto reconocido en una página y la confianza media de Tesseract (0–100), ponderada por
/// la longitud de cada palabra. `None` si el motor no la informa.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrText {
    pub text: String,
    pub confidence: Option<f32>,
}

pub trait Ocr: Send + Sync {
    /// Reconoce el texto de una imagen PNG. `page` solo se usa para informar errores.
    fn recognize(&self, image: &Path, page: usize) -> Result<String, ExtractError>;

    /// Como `recognize`, con la confianza si el motor la ofrece.
    fn recognize_page(&self, image: &Path, page: usize) -> Result<OcrText, ExtractError> {
        Ok(OcrText {
            text: self.recognize(image, page)?,
            confidence: None,
        })
    }
}

/// Tiempo máximo de Tesseract por página (D66). Una página limpia tarda unos segundos; una
/// imagen muy ruidosa puede tenerlo minutos ocupado sin obtener nada útil.
pub const OCR_TIMEOUT: Duration = Duration::from_secs(90);

pub struct Tesseract {
    executable: PathBuf,
    tessdata: PathBuf,
    languages: String,
    timeout: Duration,
}

impl Tesseract {
    pub fn new(executable: impl Into<PathBuf>, tessdata: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            tessdata: tessdata.into(),
            languages: "spa+eng".into(),
            timeout: OCR_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Ubicación habitual del instalador de UB Mannheim en Windows. Solo para desarrollo:
    /// la aplicación instalada usa siempre la copia empaquetada (D38, `packaged_executable`).
    pub fn default_executable() -> PathBuf {
        PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe")
    }

    /// Tesseract empaquetado: `tesseract\tesseract.exe` junto al ejecutable de la aplicación
    /// (D38).
    pub fn packaged_executable(app_dir: &Path) -> PathBuf {
        app_dir.join("tesseract").join("tesseract.exe")
    }

    /// Qué `tesseract.exe` usa la aplicación (D38). Instalada (`development == false`), siempre
    /// la copia empaquetada junto al ejecutable, aunque falte: entonces el OCR falla con
    /// `ocr_unavailable` y nunca se recurre a un Tesseract del sistema ni del `PATH`. En
    /// desarrollo: la empaquetada si existe, si no `vendor/tesseract` y, por último, la de
    /// Program Files.
    pub fn locate(app_dir: Option<&Path>, vendor_dir: &Path, development: bool) -> PathBuf {
        let packaged = app_dir.map(Self::packaged_executable);
        if !development {
            return packaged.unwrap_or_else(|| Self::packaged_executable(Path::new(".")));
        }
        [packaged, Some(Self::packaged_executable(vendor_dir))]
            .into_iter()
            .flatten()
            .find(|exe| exe.is_file())
            .unwrap_or_else(Self::default_executable)
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    fn run(&self, image: &Path, page: usize) -> Result<OcrText, ExtractError> {
        for language in self.languages.split('+') {
            if !self
                .tessdata
                .join(format!("{language}.traineddata"))
                .is_file()
            {
                return Err(ExtractError::OcrUnavailable(format!(
                    "faltan los datos de idioma {language}"
                )));
            }
        }
        let scratch = tempfile::tempdir().map_err(|e| {
            ExtractError::OcrUnavailable(format!("sin carpeta temporal ({})", e.kind()))
        })?;
        let base = scratch.path().join("ocr");
        let mut command = Command::new(&self.executable);
        command
            .arg(image)
            .arg(&base)
            .arg("--tessdata-dir")
            .arg(&self.tessdata)
            .args(["-l", &self.languages, "--psm", "3"])
            // Texto y TSV (con la confianza por palabra) en una sola pasada.
            .args(["-c", "tessedit_create_txt=1", "-c", "tessedit_create_tsv=1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Sin ventana de consola al ejecutarse desde la aplicación.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let (status, stderr) = match run_with_timeout(&mut command, self.timeout) {
            Ok(Some(finished)) => finished,
            Ok(None) => {
                return Err(ExtractError::OcrFailed {
                    page,
                    message: format!(
                        "Tesseract no terminó en {} s; la imagen puede ser ilegible",
                        self.timeout.as_secs()
                    ),
                })
            }
            Err(e) => {
                return Err(ExtractError::OcrUnavailable(format!(
                    "no se pudo ejecutar Tesseract ({})",
                    e.kind()
                )))
            }
        };
        if !status.success() {
            let message = stderr.lines().last().unwrap_or("sin detalle").trim();
            return Err(ExtractError::OcrFailed {
                page,
                message: format!("Tesseract terminó con {status}: {message}"),
            });
        }
        let read = |extension: &str| {
            std::fs::read(base.with_extension(extension))
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .ok_or_else(|| ExtractError::OcrFailed {
                    page,
                    message: "salida de Tesseract no válida".into(),
                })
        };
        let text = read("txt")?;
        let confidence = read("tsv").ok().and_then(|tsv| mean_confidence(&tsv));
        Ok(OcrText { text, confidence })
    }
}

impl Ocr for Tesseract {
    fn recognize(&self, image: &Path, page: usize) -> Result<String, ExtractError> {
        self.run(image, page).map(|r| r.text)
    }

    fn recognize_page(&self, image: &Path, page: usize) -> Result<OcrText, ExtractError> {
        self.run(image, page)
    }
}

/// Ejecuta el proceso con un límite de tiempo. `Ok(None)` si se agotó: el proceso se termina
/// y no deja nada en marcha.
pub fn run_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> std::io::Result<Option<(ExitStatus, String)>> {
    let mut child = command.spawn()?;
    // stderr se lee en otro hilo para que un proceso que escriba mucho no se bloquee.
    let reader = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // Si se agotó el tiempo no se espera al lector: un nieto del proceso podría mantener
    // abierta la tubería.
    Ok(status.map(|status| {
        let stderr = reader
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
        (status, stderr)
    }))
}

/// Confianza media de las palabras del TSV de Tesseract, ponderada por sus caracteres
/// visibles. Las filas que no son palabras llevan confianza -1 y no cuentan. `None` si no hay
/// palabras.
pub fn mean_confidence(tsv: &str) -> Option<f32> {
    let mut weighted = 0.0_f64;
    let mut weight = 0.0_f64;
    for line in tsv.lines().skip(1) {
        let columns: Vec<&str> = line.split('\t').collect();
        if columns.len() < 12 || columns[0] != "5" {
            continue;
        }
        let Ok(confidence) = columns[10].trim().parse::<f64>() else {
            continue;
        };
        let chars = crate::visible_chars(columns[11]);
        if confidence < 0.0 || chars == 0 {
            continue;
        }
        weighted += confidence * chars as f64;
        weight += chars as f64;
    }
    (weight > 0.0).then(|| (weighted / weight) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_process_that_does_not_finish_in_time_is_killed() {
        let mut slow = Command::new("ping");
        slow.args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let start = Instant::now();
        let result = run_with_timeout(&mut slow, Duration::from_millis(500)).unwrap();
        assert!(result.is_none());
        assert!(start.elapsed() < Duration::from_secs(10));

        let mut quick = Command::new("cmd");
        quick
            .args(["/C", "echo aviso 1>&2"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let (status, stderr) = run_with_timeout(&mut quick, Duration::from_secs(20))
            .unwrap()
            .unwrap();
        assert!(status.success());
        assert!(stderr.contains("aviso"));
    }

    #[test]
    fn confidence_is_weighted_by_word_length_and_ignores_non_words() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t100\t100\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t90\tFACTURA\n\
                   5\t1\t1\t1\t1\t2\t0\t0\t10\t10\t30\tF1\n\
                   5\t1\t1\t1\t1\t3\t0\t0\t10\t10\t95.5\t \n";
        let mean = mean_confidence(tsv).unwrap();
        assert!((mean - (90.0 * 7.0 + 30.0 * 2.0) / 9.0).abs() < 1e-4);
        assert_eq!(mean_confidence("level\tconf\n"), None);
    }
}
