//! Clientes OAuth del editor para Gmail y Microsoft 365 (D43). No son secretos de usuario: son
//! la identidad de la aplicación registrada por el editor en Google Cloud y en Entra ID. Se leen
//! de `correo-oauth.json`, que nunca se versiona con valores reales; el repositorio solo trae
//! `correo-oauth.ejemplo.json` con marcadores. Sin archivo, o con marcadores, el proveedor
//! queda «no configurado» y la ventana lo dice en lugar de intentar iniciar sesión.

use std::fs;
use std::path::{Path, PathBuf};

use nestadmin_app::accounts::{MailProfiles, ProviderProfile};
use serde::Deserialize;

pub const FILE: &str = "correo-oauth.json";
/// Variable de entorno con la ruta del archivo, para desarrollo y pruebas manuales.
const VARIABLE: &str = "NESTADMIN_MAIL_OAUTH";
/// Texto de los marcadores del archivo de ejemplo.
const PLACEHOLDER: &str = "PONER-AQUI";

#[derive(Debug, Default, Deserialize)]
struct File {
    google: Option<Google>,
    microsoft: Option<Microsoft>,
}

#[derive(Debug, Deserialize)]
struct Google {
    client_id: String,
    client_secret: String,
}

#[derive(Debug, Deserialize)]
struct Microsoft {
    client_id: String,
}

fn filled(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && !value.contains(PLACEHOLDER)
}

/// Dónde se busca el archivo, por orden: la variable de entorno, junto al ejecutable (lo que
/// instala el editor) y la carpeta de datos local.
fn candidates(data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os(VARIABLE) {
        paths.push(PathBuf::from(path));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        paths.push(dir.join(FILE));
    }
    paths.push(data_dir.join(FILE));
    paths
}

/// Proveedores configurados y, si el archivo existe pero no se puede leer, el motivo.
pub fn load(data_dir: &Path) -> (MailProfiles, Option<String>) {
    let Some(path) = candidates(data_dir).into_iter().find(|p| p.is_file()) else {
        return (MailProfiles::default(), None);
    };
    match fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str::<File>(&text).map_err(|e| e.to_string()))
    {
        Ok(file) => (profiles(file), None),
        Err(error) => (
            MailProfiles::default(),
            Some(format!("{} no es válido: {error}", path.display())),
        ),
    }
}

fn profiles(file: File) -> MailProfiles {
    MailProfiles {
        google: file
            .google
            .filter(|g| filled(&g.client_id) && filled(&g.client_secret))
            .map(|g| ProviderProfile::google(g.client_id.trim(), g.client_secret.trim())),
        microsoft: file
            .microsoft
            .filter(|m| filled(&m.client_id))
            .map(|m| ProviderProfile::microsoft(m.client_id.trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_file_configures_nothing() {
        let text = include_str!("../correo-oauth.ejemplo.json");
        let file: File = serde_json::from_str(text).unwrap();
        let profiles = profiles(file);
        assert!(profiles.google.is_none());
        assert!(profiles.microsoft.is_none());
    }

    #[test]
    fn filled_clients_are_used() {
        let file: File = serde_json::from_str(
            r#"{ "google": { "client_id": "cliente.apps.test", "client_secret": "s" },
                 "microsoft": { "client_id": "00000000-0000-0000-0000-000000000000" } }"#,
        )
        .unwrap();
        let profiles = profiles(file);
        assert_eq!(
            profiles.google.unwrap().oauth.client_id,
            "cliente.apps.test"
        );
        assert!(profiles.microsoft.is_some());
    }

    #[test]
    fn a_missing_file_is_not_an_error_and_a_broken_one_is() {
        let dir = tempfile::tempdir().unwrap();
        let (profiles, error) = load(dir.path());
        assert!(profiles.google.is_none() && error.is_none());
        fs::write(dir.path().join(FILE), "{ no es json").unwrap();
        if std::env::var_os(VARIABLE).is_none() {
            let (_, error) = load(dir.path());
            assert!(error.unwrap().contains("no es válido"));
        }
    }
}
