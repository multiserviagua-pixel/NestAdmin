//! Acceso al sistema que la preparación necesita (D40): buscar, verificar, instalar y
//! arrancar Ollama, probar Tesseract y leer la memoria del equipo (D41). Las pruebas lo
//! sustituyen por un doble.

use std::path::{Path, PathBuf};
use std::process::Command;

pub trait System: Send + Sync {
    /// Ejecutable de Ollama si está instalado, aunque no esté en marcha.
    fn ollama_executable(&self) -> Option<PathBuf>;
    /// Firma Authenticode válida cuyo sujeto contiene `O=<publisher>`.
    fn verify_signature(&self, file: &Path, publisher: &str) -> Result<(), String>;
    /// Instala Ollama por usuario en silencio y espera a que termine.
    fn install_ollama(&self, installer: &Path) -> Result<(), String>;
    /// Arranca el servicio sin ventana; no espera a que responda.
    fn start_ollama(&self, executable: &Path) -> Result<(), String>;
    /// Versión de Tesseract si responde con esos datos de idioma.
    fn tesseract_version(&self, executable: &Path, tessdata: &Path) -> Result<String, String>;
    /// Memoria física `(total, libre)` en bytes; `None` si no se puede leer.
    fn memory(&self) -> Option<(u64, u64)>;
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

/// Implementación real para Windows.
pub struct WindowsSystem;

impl System for WindowsSystem {
    fn ollama_executable(&self) -> Option<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        let exe = local.join(r"Programs\Ollama\ollama.exe");
        exe.is_file().then_some(exe)
    }

    fn verify_signature(&self, file: &Path, publisher: &str) -> Result<(), String> {
        let path = file.to_string_lossy().replace('\'', "''");
        let script = format!(
            "$s = Get-AuthenticodeSignature -LiteralPath '{path}'; \
             \"$($s.Status)|$($s.SignerCertificate.Subject)\""
        );
        let output = hidden(Command::new("powershell"))
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|e| format!("no se pudo comprobar la firma ({})", e.kind()))?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let (status, subject) = text.split_once('|').unwrap_or((text.as_str(), ""));
        let organization = format!("O={publisher}");
        if status == "Valid" && subject.split(", ").any(|part| part == organization) {
            Ok(())
        } else {
            Err(format!(
                "firma «{status}» de «{subject}», no de {publisher}"
            ))
        }
    }

    fn install_ollama(&self, installer: &Path) -> Result<(), String> {
        let status = hidden(Command::new(installer))
            .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"])
            .status()
            .map_err(|e| format!("no se pudo ejecutar el instalador ({})", e.kind()))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("el instalador de Ollama terminó con {status}"))
        }
    }

    fn start_ollama(&self, executable: &Path) -> Result<(), String> {
        hidden(Command::new(executable))
            .arg("serve")
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("no se pudo iniciar Ollama ({})", e.kind()))
    }

    fn tesseract_version(&self, executable: &Path, tessdata: &Path) -> Result<String, String> {
        if !executable.is_file() {
            return Err("falta tesseract.exe".into());
        }
        let output = hidden(Command::new(executable))
            .arg("--list-langs")
            .arg("--tessdata-dir")
            .arg(tessdata)
            .output()
            .map_err(|e| format!("Tesseract no se pudo ejecutar ({})", e.kind()))?;
        let langs = String::from_utf8_lossy(&output.stdout).to_string();
        let found: Vec<&str> = langs.lines().map(str::trim).collect();
        if !output.status.success() || !["spa", "eng"].iter().all(|l| found.contains(l)) {
            return Err("Tesseract no encuentra los datos de idioma spa y eng".into());
        }
        let version = hidden(Command::new(executable))
            .arg("--version")
            .output()
            .map_err(|e| format!("Tesseract no se pudo ejecutar ({})", e.kind()))?;
        let text = String::from_utf8_lossy(&version.stdout).to_string()
            + &String::from_utf8_lossy(&version.stderr);
        Ok(text
            .lines()
            .find_map(|l| l.trim().strip_prefix("tesseract "))
            .map(|v| v.trim().trim_start_matches('v').to_owned())
            .unwrap_or_else(|| "desconocida".into()))
    }

    fn memory(&self) -> Option<(u64, u64)> {
        physical_memory()
    }
}

/// `GlobalMemoryStatusEx` de kernel32: instantáneo y sin procesos auxiliares.
#[cfg(windows)]
fn physical_memory() -> Option<(u64, u64)> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: la estructura tiene la disposición de MEMORYSTATUSEX, `length` indica su
    // tamaño como exige la API y el puntero es válido durante la llamada.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0 && status.total_phys > 0).then_some((status.total_phys, status.avail_phys))
}

#[cfg(not(windows))]
fn physical_memory() -> Option<(u64, u64)> {
    None
}
