//! Preparación de componentes locales (R08, flujo 6, D36–D40; contrato «Preparación de
//! componentes locales»). El instalador NSIS copia la aplicación con PDFium, Tesseract y
//! los datos de idioma y, antes de terminar, ejecuta `prepare`: comprueba el OCR del
//! paquete, instala o reutiliza Ollama, lo arranca, descarga el modelo y verifica una
//! respuesta real con el mismo adaptador que usa la aplicación. Repetirla no duplica
//! instalaciones ni descargas completas. Ningún error activa un proveedor externo.
//! También lee la memoria del equipo y avisa, sin bloquear, si es menor que la recomendada
//! para la IA local (D41).

mod download;
mod memory;
mod system;

use std::fmt;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nestadmin_ai::ollama::OllamaProvider;
use nestadmin_ai::{AiError, GenerateRequest, Provider, Task};
use serde::Serialize;
use serde_json::{json, Value};

pub use download::{agent, download, Pinned};
pub use memory::{explain_timeouts, Memory, MemoryLevel, LOW_RAM_GB, RECOMMENDED_RAM_GB};
pub use system::{System, WindowsSystem};

/// Versión mínima de Ollama: `think` y `format` con esquema (D06, D37).
pub const MIN_OLLAMA: (u32, u32, u32) = (0, 9, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Network,
    Signature,
    InstallFailed,
    Unavailable,
    ModelFailed,
    VerifyFailed,
    OcrMissing,
    Cancelled,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Signature => "signature",
            Self::InstallFailed => "install_failed",
            Self::Unavailable => "unavailable",
            Self::ModelFailed => "model_failed",
            Self::VerifyFailed => "verify_failed",
            Self::OcrMissing => "ocr_missing",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error del contrato: código estable y mensaje legible, sin secretos ni rutas de usuario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct SetupError {
    pub code: ErrorCode,
    pub message: String,
}

impl SetupError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, "preparación cancelada por el usuario")
    }
}

/// Señal de cancelación compartida entre la ventana y la preparación.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn check(&self) -> Result<(), SetupError> {
        if self.is_cancelled() {
            Err(SetupError::cancelled())
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Missing {
    Ollama,
    OllamaOutdated,
    Model,
    Ocr,
}

/// Resultado de `check`: rápido y sin descargas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "component", rename_all = "snake_case")]
pub enum Check {
    Ready,
    Stopped,
    Missing(Missing),
}

/// `check` con la memoria del equipo, para el asistente (D41).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    #[serde(flatten)]
    pub check: Check,
    pub memory: Memory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Ocr,
    Ollama,
    Service,
    Model,
    Verify,
}

impl Step {
    pub const ALL: [Step; 5] = [
        Self::Ocr,
        Self::Ollama,
        Self::Service,
        Self::Model,
        Self::Verify,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Running,
    Done,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    pub step: Step,
    pub state: StepState,
    pub done: Option<u64>,
    pub total: Option<u64>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub ollama_version: String,
    /// `true` si se instaló o actualizó Ollama en esta preparación.
    pub ollama_installed: bool,
    pub model: String,
    pub model_digest: Option<String>,
    pub answer: String,
    pub latency_ms: u64,
    /// Memoria leída al preparar; su aviso no impide terminar (D41).
    pub memory: Memory,
}

/// Resultado de la comprobación al abrir (D39).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Startup {
    /// Groq elegido: no se comprueba nada local.
    NotLocal,
    Ready,
    /// Ollama estaba detenido y se arrancó.
    Started,
    /// Falta algo: la ventana debe mostrar el asistente.
    NeedsPreparation {
        check: Check,
    },
    Failed {
        error: SetupError,
    },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub ollama_url: String,
    pub model: String,
    pub tesseract_exe: PathBuf,
    pub tessdata_dir: PathBuf,
    pub pdfium_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub ollama_installer: Pinned,
    /// Organización que debe firmar el instalador de Ollama.
    pub publisher: String,
    /// Intentos ante fallos de red (D37: 3).
    pub attempts: u32,
    /// Espera antes del segundo intento; crece con cada intento.
    pub retry_delay: Duration,
    /// Plazo para que Ollama responda tras arrancarlo.
    pub start_timeout: Duration,
    /// Plazo de la petición de verificación (incluye cargar el modelo en memoria).
    pub verify_timeout: Duration,
}

impl Config {
    /// Valores de producción (D37, D38).
    pub fn windows(
        tesseract_exe: PathBuf,
        tessdata_dir: PathBuf,
        pdfium_dir: PathBuf,
        downloads_dir: PathBuf,
    ) -> Self {
        Self {
            ollama_url: nestadmin_ai::ollama::DEFAULT_URL.into(),
            model: nestadmin_ai::ollama::DEFAULT_MODEL.into(),
            tesseract_exe,
            tessdata_dir,
            pdfium_dir,
            downloads_dir,
            ollama_installer: Pinned {
                url: "https://github.com/ollama/ollama/releases/download/v0.40.14/OllamaSetup.exe"
                    .into(),
                file_name: "OllamaSetup-0.40.14.exe".into(),
                sha256: "e069c7cc7d4808f77684e81efa0b1f853a5190b4b8f070bc8fd63041180e3eba".into(),
            },
            publisher: "Ollama Inc.".into(),
            attempts: 3,
            retry_delay: Duration::from_secs(3),
            start_timeout: Duration::from_secs(60),
            verify_timeout: Duration::from_secs(300),
        }
    }
}

pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let digits: String = text
        .trim()
        .trim_start_matches('v')
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = digits.split('.').map(|p| p.parse::<u32>().ok());
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    ))
}

fn compatible(version: &str) -> bool {
    parse_version(version).is_some_and(|v| v >= MIN_OLLAMA)
}

/// Emisor de eventos de un paso.
struct Emit<'a> {
    sink: &'a mut dyn FnMut(Event),
}

impl Emit<'_> {
    fn send(&mut self, step: Step, state: StepState, message: impl Into<String>) {
        (self.sink)(Event {
            step,
            state,
            done: None,
            total: None,
            message: message.into(),
        });
    }

    fn bytes(&mut self, step: Step, message: &str, done: u64, total: Option<u64>) {
        (self.sink)(Event {
            step,
            state: StepState::Running,
            done: Some(done),
            total,
            message: message.to_owned(),
        });
    }
}

pub struct Setup {
    config: Config,
    system: Box<dyn System>,
    agent: ureq::Agent,
}

impl Setup {
    pub fn new(config: Config, system: Box<dyn System>) -> Self {
        Self {
            config,
            system,
            agent: download::agent(),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn get_json(&self, path: &str, timeout: Duration) -> Option<Value> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .timeout_connect(Some(timeout.min(Duration::from_secs(2))))
            .proxy(None)
            .build()
            .into();
        agent
            .get(&format!("{}{path}", self.config.ollama_url))
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()
    }

    /// Versión del Ollama en marcha, si responde.
    fn running_version(&self) -> Option<String> {
        self.get_json("/api/version", Duration::from_secs(3))
            .and_then(|v| v["version"].as_str().map(str::to_owned))
    }

    /// Digest del modelo configurado si Ollama lo tiene; `None` si no lo tiene.
    fn model_digest(&self) -> Option<Option<String>> {
        let tags = self.get_json("/api/tags", Duration::from_secs(5))?;
        let model = tags["models"].as_array()?.iter().find(|m| {
            m["name"].as_str() == Some(&self.config.model)
                || m["model"].as_str() == Some(&self.config.model)
        })?;
        Some(model["digest"].as_str().map(str::to_owned))
    }

    fn ocr_files_present(&self) -> bool {
        self.config.pdfium_dir.join("pdfium.dll").is_file()
            && self.config.tesseract_exe.is_file()
            && ["spa", "eng"].iter().all(|l| {
                self.config
                    .tessdata_dir
                    .join(format!("{l}.traineddata"))
                    .is_file()
            })
    }

    /// Comprobación rápida, sin descargas ni instalaciones.
    pub fn check(&self) -> Check {
        if !self.ocr_files_present() {
            return Check::Missing(Missing::Ocr);
        }
        match self.running_version() {
            Some(version) if !compatible(&version) => Check::Missing(Missing::OllamaOutdated),
            Some(_) => match self.model_digest() {
                Some(_) => Check::Ready,
                None => Check::Missing(Missing::Model),
            },
            None if self.system.ollama_executable().is_some() => Check::Stopped,
            None => Check::Missing(Missing::Ollama),
        }
    }

    /// Memoria física del equipo y su aviso (D41). Instantánea; no bloquea nada.
    pub fn memory(&self) -> Memory {
        Memory::assess(self.system.memory())
    }

    /// `check` y la memoria del equipo.
    pub fn status(&self) -> Status {
        Status {
            check: self.check(),
            memory: self.memory(),
        }
    }

    /// Comprobación al abrir la aplicación (D39): arranca un Ollama detenido; si falta
    /// algo no descarga nada y pide el asistente.
    pub fn startup(&self, local_provider: bool) -> Startup {
        if !local_provider {
            return Startup::NotLocal;
        }
        match self.check() {
            Check::Ready => Startup::Ready,
            Check::Stopped => match self.start_and_wait(&Cancel::default()) {
                Ok(version) if compatible(&version) => match self.model_digest() {
                    Some(_) => Startup::Started,
                    None => Startup::NeedsPreparation {
                        check: Check::Missing(Missing::Model),
                    },
                },
                Ok(_) => Startup::NeedsPreparation {
                    check: Check::Missing(Missing::OllamaOutdated),
                },
                Err(error) => Startup::Failed { error },
            },
            check => Startup::NeedsPreparation { check },
        }
    }

    /// Prepara lo que falte, en orden, y verifica el modelo.
    pub fn prepare(
        &self,
        events: &mut dyn FnMut(Event),
        cancel: &Cancel,
    ) -> Result<Report, SetupError> {
        let mut emit = Emit { sink: events };
        let mut current = Step::Ocr;
        let result = self.run(&mut emit, cancel, &mut current);
        if let Err(error) = &result {
            emit.send(current, StepState::Failed, error.message.clone());
        }
        result
    }

    fn run(
        &self,
        emit: &mut Emit<'_>,
        cancel: &Cancel,
        current: &mut Step,
    ) -> Result<Report, SetupError> {
        // Memoria (D41): solo informa; con poca, la verificación explica un tiempo agotado.
        let memory = self.memory();

        // OCR del paquete (D38): solo se comprueba.
        *current = Step::Ocr;
        emit.send(Step::Ocr, StepState::Running, "Comprobando el OCR incluido");
        if !self.ocr_files_present() {
            return Err(SetupError::new(
                ErrorCode::OcrMissing,
                "faltan componentes de OCR del paquete; reinstale NestAdmin",
            ));
        }
        let tesseract = self
            .system
            .tesseract_version(&self.config.tesseract_exe, &self.config.tessdata_dir)
            .map_err(|reason| {
                SetupError::new(
                    ErrorCode::OcrMissing,
                    format!("{reason}; reinstale NestAdmin"),
                )
            })?;
        emit.send(
            Step::Ocr,
            StepState::Done,
            format!("Tesseract {tesseract} y PDFium listos"),
        );
        cancel.check()?;

        // Ollama: reutilizar, o instalar si falta o es anterior a la mínima.
        *current = Step::Ollama;
        let mut installed = false;
        match self.running_version() {
            Some(version) if compatible(&version) => emit.send(
                Step::Ollama,
                StepState::Skipped,
                format!("Ollama {version} ya instalado"),
            ),
            Some(version) => {
                emit.send(
                    Step::Ollama,
                    StepState::Running,
                    format!("Ollama {version} es anterior a la mínima; se actualizará"),
                );
                self.install_ollama(emit, cancel)?;
                installed = true;
            }
            None if self.system.ollama_executable().is_some() => emit.send(
                Step::Ollama,
                StepState::Skipped,
                "Ollama instalado y detenido",
            ),
            None => {
                emit.send(Step::Ollama, StepState::Running, "Ollama no está instalado");
                self.install_ollama(emit, cancel)?;
                installed = true;
            }
        }
        cancel.check()?;

        // Servicio: arrancar si no responde; una versión anterior descubierta al arrancar
        // se actualiza y se vuelve a arrancar.
        *current = Step::Service;
        let version = match self.running_version() {
            Some(version) if compatible(&version) && !installed => {
                emit.send(
                    Step::Service,
                    StepState::Skipped,
                    "Ollama ya está en marcha",
                );
                version
            }
            Some(version) if compatible(&version) => {
                emit.send(Step::Service, StepState::Done, "Ollama en marcha");
                version
            }
            _ => {
                emit.send(Step::Service, StepState::Running, "Iniciando Ollama");
                let mut version = self.start_and_wait(cancel)?;
                if !compatible(&version) && !installed {
                    *current = Step::Ollama;
                    emit.send(
                        Step::Ollama,
                        StepState::Running,
                        format!("Ollama {version} es anterior a la mínima; se actualizará"),
                    );
                    self.install_ollama(emit, cancel)?;
                    installed = true;
                    *current = Step::Service;
                    emit.send(Step::Service, StepState::Running, "Reiniciando Ollama");
                    version = self.start_and_wait(cancel)?;
                }
                if !compatible(&version) {
                    return Err(SetupError::new(
                        ErrorCode::InstallFailed,
                        format!("Ollama sigue en la versión {version} tras actualizarlo"),
                    ));
                }
                emit.send(
                    Step::Service,
                    StepState::Done,
                    format!("Ollama {version} en marcha"),
                );
                version
            }
        };
        cancel.check()?;

        // Modelo.
        *current = Step::Model;
        let digest = match self.model_digest() {
            Some(digest) => {
                emit.send(
                    Step::Model,
                    StepState::Skipped,
                    format!("Modelo {} ya descargado", self.config.model),
                );
                digest
            }
            None => {
                self.retrying(emit, Step::Model, cancel, |emit| {
                    self.pull_model(emit, cancel)
                })?;
                emit.send(
                    Step::Model,
                    StepState::Done,
                    format!("Modelo {} descargado", self.config.model),
                );
                self.model_digest().flatten()
            }
        };
        cancel.check()?;

        // Verificación con el adaptador de la aplicación.
        *current = Step::Verify;
        emit.send(
            Step::Verify,
            StepState::Running,
            "Pidiendo una respuesta al modelo local",
        );
        let (answer, latency_ms) = self.verify(&memory)?;
        emit.send(
            Step::Verify,
            StepState::Done,
            format!("El modelo respondió «{answer}»"),
        );
        Ok(Report {
            ollama_version: version,
            ollama_installed: installed,
            model: self.config.model.clone(),
            model_digest: digest,
            answer,
            latency_ms,
            memory,
        })
    }

    /// Repite `work` ante fallos de red, hasta `attempts` veces con espera creciente.
    fn retrying<T>(
        &self,
        emit: &mut Emit<'_>,
        step: Step,
        cancel: &Cancel,
        mut work: impl FnMut(&mut Emit<'_>) -> Result<T, SetupError>,
    ) -> Result<T, SetupError> {
        let attempts = self.config.attempts.max(1);
        let mut attempt = 1;
        loop {
            match work(emit) {
                Err(error) if error.code == ErrorCode::Network && attempt < attempts => {
                    attempt += 1;
                    emit.send(
                        step,
                        StepState::Running,
                        format!("Reintento {attempt} de {attempts}: {}", error.message),
                    );
                    let deadline = Instant::now() + self.config.retry_delay * (attempt - 1);
                    while Instant::now() < deadline {
                        cancel.check()?;
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
                other => return other,
            }
        }
    }

    fn install_ollama(&self, emit: &mut Emit<'_>, cancel: &Cancel) -> Result<(), SetupError> {
        let installer = self.retrying(emit, Step::Ollama, cancel, |emit| {
            download(
                &self.agent,
                &self.config.ollama_installer,
                &self.config.downloads_dir,
                cancel,
                &mut |done, total| emit.bytes(Step::Ollama, "Descargando Ollama", done, total),
            )
        })?;
        if let Err(reason) = self
            .system
            .verify_signature(&installer, &self.config.publisher)
        {
            let _ = std::fs::remove_file(&installer);
            return Err(SetupError::new(
                ErrorCode::Signature,
                format!("el instalador de Ollama no tiene una firma válida: {reason}"),
            ));
        }
        cancel.check()?;
        emit.send(Step::Ollama, StepState::Running, "Instalando Ollama");
        let result = self
            .system
            .install_ollama(&installer)
            .map_err(|reason| SetupError::new(ErrorCode::InstallFailed, reason));
        let _ = std::fs::remove_file(&installer);
        result?;
        emit.send(Step::Ollama, StepState::Done, "Ollama instalado");
        Ok(())
    }

    /// Arranca Ollama y espera a que responda; devuelve su versión.
    fn start_and_wait(&self, cancel: &Cancel) -> Result<String, SetupError> {
        if let Some(version) = self.running_version() {
            return Ok(version);
        }
        let executable = self.system.ollama_executable().ok_or_else(|| {
            SetupError::new(
                ErrorCode::Unavailable,
                "no se encuentra el ejecutable de Ollama",
            )
        })?;
        self.system
            .start_ollama(&executable)
            .map_err(|reason| SetupError::new(ErrorCode::Unavailable, reason))?;
        let deadline = Instant::now() + self.config.start_timeout;
        while Instant::now() < deadline {
            cancel.check()?;
            if let Some(version) = self.running_version() {
                return Ok(version);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(SetupError::new(
            ErrorCode::Unavailable,
            format!(
                "Ollama no respondió en {} s tras iniciarlo",
                self.config.start_timeout.as_secs()
            ),
        ))
    }

    /// Descarga el modelo con la API de Ollama, que reanuda las capas ya descargadas.
    fn pull_model(&self, emit: &mut Emit<'_>, cancel: &Cancel) -> Result<(), SetupError> {
        let network = |message: &str| SetupError::new(ErrorCode::Network, message);
        let response = self
            .agent
            .post(&format!("{}/api/pull", self.config.ollama_url))
            .send_json(json!({ "model": self.config.model, "stream": true }))
            .map_err(|_| network("Ollama no responde a la descarga del modelo"))?;
        if !response.status().is_success() {
            return Err(SetupError::new(
                ErrorCode::ModelFailed,
                format!(
                    "Ollama respondió {} al descargar el modelo",
                    response.status().as_u16()
                ),
            ));
        }
        let reader = BufReader::new(response.into_body().into_reader());
        for line in reader.lines() {
            cancel.check()?;
            let line = line.map_err(|_| network("la descarga del modelo se interrumpió"))?;
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(error) = event["error"].as_str() {
                return Err(SetupError::new(
                    ErrorCode::ModelFailed,
                    format!("Ollama rechazó la descarga del modelo: {error}"),
                ));
            }
            match event["status"].as_str() {
                Some("success") => return Ok(()),
                Some(_) if event["total"].is_u64() => emit.bytes(
                    Step::Model,
                    "Descargando el modelo",
                    event["completed"].as_u64().unwrap_or(0),
                    event["total"].as_u64(),
                ),
                Some(status) => emit.send(Step::Model, StepState::Running, status),
                None => {}
            }
        }
        Err(network("la descarga del modelo terminó sin confirmación"))
    }

    fn verify(&self, memory: &Memory) -> Result<(String, u64), SetupError> {
        let provider = OllamaProvider::new(
            &self.config.ollama_url,
            &self.config.model,
            self.config.verify_timeout,
        )
        .map_err(|e| SetupError::new(ErrorCode::Unavailable, e.to_string()))?;
        let request = GenerateRequest {
            request_id: "verificacion-instalacion".into(),
            task: Task::Classification,
            instructions: "Comprobación de instalación. Responde con la palabra listo.".into(),
            content: "Responde con la palabra listo.".into(),
            schema: Some(json!({
                "type": "object",
                "properties": { "respuesta": { "type": "string" } },
                "required": ["respuesta"],
            })),
        };
        let result = provider.generate(&request).map_err(|error| match memory.explain(error) {
            AiError::Unavailable(reason) => SetupError::new(ErrorCode::Unavailable, reason),
            other => SetupError::new(ErrorCode::VerifyFailed, other.to_string()),
        })?;
        let answer = result.output["respuesta"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_owned();
        if answer.is_empty() {
            return Err(SetupError::new(
                ErrorCode::VerifyFailed,
                "el modelo devolvió una respuesta vacía",
            ));
        }
        Ok((answer, result.latency_ms))
    }
}
