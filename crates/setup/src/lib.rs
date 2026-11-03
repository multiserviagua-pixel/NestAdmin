//! Preparación del equipo (R08, flujo 6, D11). El instalador NSIS de Tauri copia la
//! aplicación con PDFium y los datos de idioma, e instala WebView2 si falta; este módulo
//! completa el resto desde la propia aplicación: detecta Ollama, el modelo y Tesseract,
//! reutiliza lo compatible, descarga e instala solo lo que falta con progreso y reintentos
//! y verifica una respuesta real del modelo. Cada paso que necesita permisos (UAC) los
//! pide en ese momento; si el usuario los deniega, el paso queda fallido y reintentable.

mod download;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use download::{agent, download, Pinned};

/// Versiones mínimas compatibles. Ollama necesita `think` y `format` con esquema.
pub const MIN_OLLAMA: (u32, u32, u32) = (0, 9, 0);
pub const TESSERACT_MAJOR: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Webview2,
    Ollama,
    Model,
    Tesseract,
    Pdfium,
    Tessdata,
}

impl Component {
    pub const ALL: [Component; 6] = [
        Self::Webview2,
        Self::Pdfium,
        Self::Tessdata,
        Self::Tesseract,
        Self::Ollama,
        Self::Model,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webview2 => "webview2",
            Self::Ollama => "ollama",
            Self::Model => "model",
            Self::Tesseract => "tesseract",
            Self::Pdfium => "pdfium",
            Self::Tessdata => "tessdata",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    Present {
        version: Option<String>,
    },
    /// Instalado pero no en marcha (Ollama sin servicio).
    Stopped {
        version: Option<String>,
    },
    Missing,
    Incompatible {
        found: String,
        required: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub component: Component,
    #[serde(flatten)]
    pub state: State,
    /// Si NestAdmin puede instalarlo o arrancarlo; los componentes incluidos en el
    /// instalador solo se reparan reinstalando la aplicación.
    pub installable: bool,
    pub detail: String,
}

impl Status {
    pub fn ready(&self) -> bool {
        matches!(self.state, State::Present { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Progress {
    pub component: String,
    pub step: String,
    pub done: Option<u64>,
    pub total: Option<u64>,
}

impl Progress {
    pub fn step(component: &str, step: &str) -> Self {
        Self {
            component: component.to_owned(),
            step: step.to_owned(),
            done: None,
            total: None,
        }
    }
    pub fn bytes(component: &str, step: &str, done: u64, total: Option<u64>) -> Self {
        Self {
            component: component.to_owned(),
            step: step.to_owned(),
            done: Some(done),
            total,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetupError {
    #[error("red: {0}")]
    Network(String),
    #[error("integridad: {0}")]
    Integrity(String),
    #[error("archivo: {0}")]
    Io(String),
    #[error("instalación: {0}")]
    Install(String),
    #[error("permiso denegado o instalación cancelada por el usuario")]
    Cancelled,
    #[error("IA local: {0}")]
    Model(String),
    #[error("{0}")]
    NotInstallable(String),
}

impl SetupError {
    pub(crate) fn io(error: std::io::Error) -> Self {
        Self::Io(error.kind().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verification {
    pub model: String,
    pub answer: String,
    pub latency_ms: u64,
}

/// Rutas y fuentes; `Config::windows` da los valores de producción.
#[derive(Debug, Clone)]
pub struct Config {
    pub ollama_url: String,
    pub model: String,
    pub ollama_exe: PathBuf,
    pub tesseract_exe: PathBuf,
    pub pdfium_dir: PathBuf,
    pub tessdata_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub ollama_installer: Pinned,
    pub tesseract_installer: Pinned,
}

impl Config {
    /// Componentes fijados en decisiones.md (Ollama 0.40.14, Tesseract 5.4.0).
    pub fn windows(
        pdfium_dir: PathBuf,
        tessdata_dir: PathBuf,
        tesseract_exe: PathBuf,
        downloads_dir: PathBuf,
    ) -> Self {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            ollama_url: "http://127.0.0.1:11434".into(),
            model: "qwen3:8b".into(),
            ollama_exe: local.join(r"Programs\Ollama\ollama.exe"),
            tesseract_exe,
            pdfium_dir,
            tessdata_dir,
            downloads_dir,
            ollama_installer: Pinned {
                url: "https://github.com/ollama/ollama/releases/download/v0.40.14/OllamaSetup.exe".into(),
                file_name: "OllamaSetup-0.40.14.exe".into(),
                sha256: "e069c7cc7d4808f77684e81efa0b1f853a5190b4b8f070bc8fd63041180e3eba".into(),
            },
            tesseract_installer: Pinned {
                url: "https://github.com/UB-Mannheim/tesseract/releases/download/v5.4.0.20240606/tesseract-ocr-w64-setup-5.4.0.20240606.exe".into(),
                file_name: "tesseract-ocr-w64-setup-5.4.0.20240606.exe".into(),
                sha256: "c885fff6998e0608ba4bb8ab51436e1c6775c2bafc2559a19b423e18678b60c9".into(),
            },
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

pub struct Setup {
    config: Config,
    agent: ureq::Agent,
}

impl Setup {
    pub fn new(config: Config) -> Self {
        Self {
            config,
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

    pub fn detect(&self) -> Vec<Status> {
        Component::ALL.iter().map(|c| self.detect_one(*c)).collect()
    }

    pub fn detect_one(&self, component: Component) -> Status {
        let status = |state: State, installable: bool, detail: &str| Status {
            component,
            state,
            installable,
            detail: detail.to_owned(),
        };
        match component {
            Component::Webview2 => match webview2_version() {
                Some(version) => status(
                    State::Present {
                        version: Some(version),
                    },
                    false,
                    "Motor de la ventana de Windows.",
                ),
                None => status(
                    State::Missing,
                    false,
                    "El instalador de NestAdmin lo instala si falta.",
                ),
            },
            Component::Pdfium => {
                let dll = self.config.pdfium_dir.join("pdfium.dll");
                if dll.is_file() {
                    status(
                        State::Present { version: None },
                        false,
                        "Incluido con NestAdmin.",
                    )
                } else {
                    status(
                        State::Missing,
                        false,
                        "Reinstale NestAdmin para recuperarlo.",
                    )
                }
            }
            Component::Tessdata => {
                let missing: Vec<&str> = ["spa", "eng"]
                    .into_iter()
                    .filter(|l| {
                        !self
                            .config
                            .tessdata_dir
                            .join(format!("{l}.traineddata"))
                            .is_file()
                    })
                    .collect();
                if missing.is_empty() {
                    status(
                        State::Present {
                            version: Some("spa, eng".into()),
                        },
                        false,
                        "Datos de idioma del OCR, incluidos con NestAdmin.",
                    )
                } else {
                    status(
                        State::Missing,
                        false,
                        &format!("Faltan {}; reinstale NestAdmin.", missing.join(", ")),
                    )
                }
            }
            Component::Tesseract => match tesseract_version(&self.config.tesseract_exe) {
                Some(version) => match parse_version(&version) {
                    Some((major, ..)) if major == TESSERACT_MAJOR => status(
                        State::Present {
                            version: Some(version),
                        },
                        true,
                        "OCR local para documentos escaneados.",
                    ),
                    _ => status(
                        State::Incompatible {
                            found: version,
                            required: format!("{TESSERACT_MAJOR}.x"),
                        },
                        true,
                        "Se instalará la versión 5.4.0.",
                    ),
                },
                None => status(
                    State::Missing,
                    true,
                    "Necesario para PDF escaneados (unos 50 MB; pedirá permiso de administrador).",
                ),
            },
            Component::Ollama => {
                let running = self
                    .get_json("/api/version", Duration::from_secs(3))
                    .and_then(|v| v["version"].as_str().map(str::to_owned));
                match running {
                    Some(version) => match parse_version(&version) {
                        Some(found) if found >= MIN_OLLAMA => status(
                            State::Present {
                                version: Some(version),
                            },
                            true,
                            "Servicio de IA local en 127.0.0.1.",
                        ),
                        _ => status(
                            State::Incompatible {
                                found: version,
                                required: format!(
                                    "{}.{}.{} o posterior",
                                    MIN_OLLAMA.0, MIN_OLLAMA.1, MIN_OLLAMA.2
                                ),
                            },
                            true,
                            "Se instalará la versión 0.40.14.",
                        ),
                    },
                    None if self.config.ollama_exe.is_file() => status(
                        State::Stopped { version: None },
                        true,
                        "Instalado pero sin iniciar; NestAdmin puede arrancarlo.",
                    ),
                    None => status(State::Missing, true, "Servicio de IA local (unos 1,7 GB)."),
                }
            }
            Component::Model => {
                let Some(tags) = self.get_json("/api/tags", Duration::from_secs(5)) else {
                    return status(State::Missing, true, "Requiere Ollama en marcha.");
                };
                let found = tags["models"].as_array().and_then(|models| {
                    models.iter().find(|m| {
                        m["name"].as_str() == Some(&self.config.model)
                            || m["model"].as_str() == Some(&self.config.model)
                    })
                });
                match found {
                    Some(model) => status(
                        State::Present {
                            version: model["digest"]
                                .as_str()
                                .map(|d| d.chars().take(12).collect()),
                        },
                        true,
                        &format!("Modelo {}.", self.config.model),
                    ),
                    None => status(
                        State::Missing,
                        true,
                        &format!("Modelo {} (unos 5,2 GB).", self.config.model),
                    ),
                }
            }
        }
    }

    /// Instala o arranca un componente. Es seguro repetirlo: lo ya presente no se toca y
    /// las descargas se reanudan.
    pub fn install(
        &self,
        component: Component,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(), SetupError> {
        let name = component.as_str();
        match component {
            Component::Webview2 | Component::Pdfium | Component::Tessdata => {
                Err(SetupError::NotInstallable(
                    "este componente se incluye con el instalador de NestAdmin; reinstálelo".into(),
                ))
            }
            Component::Tesseract => {
                if self.detect_one(component).ready() {
                    return Ok(());
                }
                let installer = download(
                    &self.agent,
                    &self.config.tesseract_installer,
                    &self.config.downloads_dir,
                    name,
                    progress,
                )?;
                progress(Progress::step(name, "Instalando (Windows pedirá permiso)"));
                run_elevated(&installer, &["/S"])?;
                if self.detect_one(component).ready() {
                    Ok(())
                } else {
                    Err(SetupError::Install(
                        "el instalador terminó, pero Tesseract no responde".into(),
                    ))
                }
            }
            Component::Ollama => {
                match self.detect_one(component).state {
                    State::Present { .. } => return Ok(()),
                    State::Stopped { .. } => {}
                    State::Missing | State::Incompatible { .. } => {
                        let installer = download(
                            &self.agent,
                            &self.config.ollama_installer,
                            &self.config.downloads_dir,
                            name,
                            progress,
                        )?;
                        progress(Progress::step(name, "Instalando"));
                        // Instalación por usuario: no necesita permisos de administrador.
                        let code = hidden(Command::new(&installer))
                            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"])
                            .status()
                            .map_err(|e| {
                                SetupError::Install(format!(
                                    "no se pudo ejecutar el instalador ({})",
                                    e.kind()
                                ))
                            })?;
                        if !code.success() {
                            return Err(SetupError::Install(format!(
                                "el instalador de Ollama terminó con {code}"
                            )));
                        }
                    }
                }
                progress(Progress::step(name, "Iniciando el servicio"));
                self.start_ollama()
            }
            Component::Model => {
                if self.detect_one(component).ready() {
                    return Ok(());
                }
                self.pull_model(progress)
            }
        }
    }

    fn start_ollama(&self) -> Result<(), SetupError> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut launched = false;
        while Instant::now() < deadline {
            if self.detect_one(Component::Ollama).ready() {
                return Ok(());
            }
            if !launched && self.config.ollama_exe.is_file() {
                hidden(Command::new(&self.config.ollama_exe))
                    .arg("serve")
                    .spawn()
                    .map_err(|e| {
                        SetupError::Install(format!("no se pudo iniciar Ollama ({})", e.kind()))
                    })?;
                launched = true;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        Err(SetupError::Install(
            "Ollama no respondió en 60 s tras iniciarlo".into(),
        ))
    }

    /// Descarga el modelo mediante la API de Ollama, que reanuda las capas ya descargadas.
    fn pull_model(&self, progress: &mut dyn FnMut(Progress)) -> Result<(), SetupError> {
        let name = Component::Model.as_str();
        let response = self
            .agent
            .post(&format!("{}/api/pull", self.config.ollama_url))
            .send_json(json!({ "model": self.config.model, "stream": true }))
            .map_err(|_| {
                SetupError::Network("Ollama no responde; instálelo o inícielo primero".into())
            })?;
        if !response.status().is_success() {
            return Err(SetupError::Network(format!(
                "Ollama devolvió el estado {}",
                response.status().as_u16()
            )));
        }
        let reader = std::io::BufReader::new(response.into_body().into_reader());
        let mut finished = false;
        for line in std::io::BufRead::lines(reader) {
            let line = line.map_err(|e| {
                SetupError::Network(format!(
                    "descarga del modelo interrumpida ({}); se reanudará al reintentar",
                    e.kind()
                ))
            })?;
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(error) = event["error"].as_str() {
                return Err(SetupError::Network(format!("Ollama: {error}")));
            }
            let step = event["status"].as_str().unwrap_or("");
            if step == "success" {
                finished = true;
            }
            progress(Progress {
                component: name.into(),
                step: step.to_owned(),
                done: event["completed"].as_u64(),
                total: event["total"].as_u64(),
            });
        }
        if finished {
            Ok(())
        } else {
            Err(SetupError::Network(
                "la descarga del modelo terminó sin confirmación; se reanudará al reintentar"
                    .into(),
            ))
        }
    }

    /// Pide una respuesta real al modelo local (R08: la instalación termina comprobándolo).
    pub fn verify_model(&self) -> Result<Verification, SetupError> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(300)))
            .timeout_connect(Some(Duration::from_secs(2)))
            .http_status_as_error(false)
            .proxy(None)
            .build()
            .into();
        let started = Instant::now();
        let mut response = agent
            .post(&format!("{}/api/chat", self.config.ollama_url))
            .send_json(json!({
                "model": self.config.model,
                "messages": [{ "role": "user", "content": "Responde únicamente con la palabra: listo" }],
                "stream": false,
                "think": false,
                "options": { "temperature": 0 },
            }))
            .map_err(|_| SetupError::Model("Ollama no responde".into()))?;
        let status = response.status().as_u16();
        let body: Value = response
            .body_mut()
            .read_json()
            .map_err(|_| SetupError::Model(format!("respuesta no válida (HTTP {status})")))?;
        if let Some(error) = body["error"].as_str() {
            return Err(SetupError::Model(error.to_owned()));
        }
        let answer = body["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_owned();
        if answer.is_empty() {
            return Err(SetupError::Model(
                "el modelo devolvió una respuesta vacía".into(),
            ));
        }
        Ok(Verification {
            model: self.config.model.clone(),
            answer,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }
}

#[cfg_attr(not(windows), allow(unused_mut))]
fn hidden(mut command: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Código de Windows `ERROR_CANCELLED`: el usuario rechazó el diálogo UAC.
const ERROR_CANCELLED: i32 = 1223;
/// Código propio del guion cuando `Start-Process` falla por otro motivo.
const LAUNCH_FAILED: i32 = 0x4E41;

/// Ejecuta un instalador con elevación (UAC) y espera. Si el usuario deniega el permiso,
/// devuelve `Cancelled` para que el paso se pueda reintentar.
fn run_elevated(installer: &Path, args: &[&str]) -> Result<(), SetupError> {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let arguments = args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(",");
    let script = format!(
        "try {{ $p = Start-Process -FilePath {} -ArgumentList {} -Verb RunAs -Wait -PassThru -ErrorAction Stop; exit $p.ExitCode }} \
         catch {{ if ($_.Exception.NativeErrorCode -eq {ERROR_CANCELLED}) {{ exit {ERROR_CANCELLED} }} else {{ exit {LAUNCH_FAILED} }} }}",
        quote(&installer.to_string_lossy()),
        arguments
    );
    let status = hidden(Command::new("powershell"))
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map_err(|e| SetupError::Install(format!("no se pudo pedir permisos ({})", e.kind())))?;
    match status.code() {
        Some(0) => Ok(()),
        Some(ERROR_CANCELLED) => Err(SetupError::Cancelled),
        Some(LAUNCH_FAILED) => Err(SetupError::Install(
            "Windows no pudo iniciar el instalador con permisos de administrador".into(),
        )),
        other => Err(SetupError::Install(format!(
            "el instalador terminó con el código {}",
            other.map_or("desconocido".into(), |c| c.to_string())
        ))),
    }
}

fn tesseract_version(exe: &Path) -> Option<String> {
    if !exe.is_file() {
        return None;
    }
    let output = hidden(Command::new(exe)).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    text.lines()
        .find_map(|l| l.trim().strip_prefix("tesseract "))
        .map(|v| v.trim().trim_start_matches('v').to_owned())
}

/// Versión de WebView2 registrada por Windows, si existe.
fn webview2_version() -> Option<String> {
    const CLIENT: &str = r"\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    for key in [
        format!(r"HKLM\SOFTWARE\WOW6432Node{CLIENT}"),
        format!(r"HKCU\Software{CLIENT}"),
        format!(r"HKLM\SOFTWARE{CLIENT}"),
    ] {
        let output = hidden(Command::new("reg"))
            .args(["query", &key, "/v", "pv"])
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        if let Some(version) = text
            .lines()
            .find(|l| l.trim_start().starts_with("pv"))
            .and_then(|l| l.split_whitespace().last())
            .filter(|v| *v != "0.0.0.0")
        {
            return Some(version.to_owned());
        }
    }
    None
}
