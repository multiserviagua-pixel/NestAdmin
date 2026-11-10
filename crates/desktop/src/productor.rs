//! Productor del software para los registros Verifactu (fase 11, E3a). Son el nombre y el NIF del
//! editor de NestAdmin, no del usuario: se leen de `verifactu-productor.json`, en la carpeta de
//! datos de la aplicación (la de `configuracion.json`), y nunca se versionan con valores reales;
//! el repositorio solo trae `docs/verifactu-productor.ejemplo.json` con marcadores.
//!
//! Sin archivo el productor queda «no configurado» y no es un error al arrancar: presupuestos y
//! albaranes se emiten igual y emitir una factura falla con `[verifactu_not_configured]`. Con un
//! archivo inválido ocurre lo mismo y además queda el motivo, sin el contenido del archivo, para
//! que la ventana lo muestre.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use nestadmin_app::App;
use serde::{Deserialize, Serialize};

pub const FILE: &str = "verifactu-productor.json";
/// Texto de los marcadores del archivo de ejemplo.
const PLACEHOLDER: &str = "PONER-AQUI";

#[derive(Debug, Deserialize)]
struct Producer {
    name: String,
    tax_id: String,
}

/// Estado del productor para la ventana.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProducerState {
    pub configured: bool,
    /// Por qué el archivo existe pero no sirve; `None` si falta o es válido.
    pub problem: Option<String>,
}

/// Lee el archivo. `Ok(None)` si no existe; `Err` es el motivo, sin el contenido.
fn read(dir: &Path) -> Result<Option<Producer>, String> {
    let bytes = match fs::read(dir.join(FILE)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{FILE}: no se pudo leer ({:?})", e.kind())),
    };
    let producer: Producer = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "{FILE}: no es un JSON con «name» y «tax_id» (línea {}, columna {})",
            e.line(),
            e.column()
        )
    })?;
    if producer.name.contains(PLACEHOLDER) || producer.tax_id.contains(PLACEHOLDER) {
        return Err(format!("{FILE}: tiene marcadores sin rellenar"));
    }
    Ok(Some(producer))
}

/// Carga el productor del archivo en la aplicación. Deja siempre un estado coherente: si el
/// archivo falta o no sirve, el productor que hubiera antes se quita.
pub fn load(app: &mut App, dir: &Path) -> ProducerState {
    let outcome = match read(dir) {
        Ok(Some(p)) => app
            .set_verifactu_producer(&p.name, &p.tax_id)
            .map(|()| true)
            .map_err(|e| format!("{FILE}: {e}")),
        Ok(None) => Ok(false),
        Err(problem) => Err(problem),
    };
    match outcome {
        Ok(configured) => {
            if !configured {
                app.clear_verifactu_producer();
            }
            ProducerState {
                configured,
                problem: None,
            }
        }
        Err(problem) => {
            app.clear_verifactu_producer();
            ProducerState {
                configured: false,
                problem: Some(problem),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) {
        fs::write(dir.join(FILE), text).unwrap();
    }

    #[test]
    fn sin_archivo_no_hay_productor_ni_problema() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path()).unwrap().is_none());
    }

    #[test]
    fn archivo_valido_se_lee() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            r#"{ "name": "Productor Demo SL", "tax_id": "B00000034" }"#,
        );
        let p = read(dir.path()).unwrap().unwrap();
        assert_eq!(
            (p.name.as_str(), p.tax_id.as_str()),
            ("Productor Demo SL", "B00000034")
        );
    }

    #[test]
    fn archivo_invalido_da_motivo_sin_volcar_el_contenido() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r#"{ "name": "Secreto Demo SL", "tax_id": "#);
        let why = read(dir.path()).err().unwrap();
        assert!(why.contains("línea") && !why.contains("Secreto"), "{why}");
        write(dir.path(), r#"{ "name": "Secreto Demo SL" }"#);
        let why = read(dir.path()).err().unwrap();
        assert!(!why.contains("Secreto"), "{why}");
    }

    #[test]
    fn el_archivo_de_ejemplo_con_marcadores_no_cuenta_como_valido() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            include_str!("../../../docs/verifactu-productor.ejemplo.json"),
        );
        let why = read(dir.path()).err().unwrap();
        assert!(why.contains("marcadores"), "{why}");
    }
}
