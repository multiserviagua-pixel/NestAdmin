//! OCR local. `Tesseract` ejecuta el binario instalado como proceso hijo, sin red; el trait
//! permite sustituirlo en pruebas.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::ExtractError;

pub trait Ocr: Send + Sync {
    /// Reconoce el texto de una imagen PNG. `page` solo se usa para informar errores.
    fn recognize(&self, image: &Path, page: usize) -> Result<String, ExtractError>;
}

pub struct Tesseract {
    executable: PathBuf,
    tessdata: PathBuf,
    languages: String,
}

impl Tesseract {
    pub fn new(executable: impl Into<PathBuf>, tessdata: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            tessdata: tessdata.into(),
            languages: "spa+eng".into(),
        }
    }

    /// Ubicación habitual del instalador de UB Mannheim en Windows.
    pub fn default_executable() -> PathBuf {
        PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe")
    }
}

impl Ocr for Tesseract {
    fn recognize(&self, image: &Path, page: usize) -> Result<String, ExtractError> {
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
        let mut command = Command::new(&self.executable);
        command
            .arg(image)
            .arg("stdout")
            .arg("--tessdata-dir")
            .arg(&self.tessdata)
            .args(["-l", &self.languages, "--psm", "3"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Sin ventana de consola al ejecutarse desde la aplicación.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let output = command.output().map_err(|e| {
            ExtractError::OcrUnavailable(format!("no se pudo ejecutar Tesseract ({})", e.kind()))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = stderr.lines().last().unwrap_or("sin detalle").trim();
            return Err(ExtractError::OcrFailed {
                page,
                message: format!("Tesseract terminó con {}: {message}", output.status),
            });
        }
        String::from_utf8(output.stdout).map_err(|_| ExtractError::OcrFailed {
            page,
            message: "salida de Tesseract no válida".into(),
        })
    }
}
