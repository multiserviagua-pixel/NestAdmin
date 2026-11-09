//! Salidas de la espera de IA de D146 (404, 402) en la ventana: el sondeo sintético, la prueba
//! de «Probar de nuevo» y el cambio de clave. Datos sintéticos; el proveedor es el adaptador real
//! de Ollama contra un servidor simulado en 127.0.0.1. Con el proveedor externo no se llama a
//! nada: se comprueba que la prueba termina antes de construir ningún cliente.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use nestadmin_ai::{MemoryCredentialStore, ModelUnavailableCause};
use serde_json::json;

use super::*;

/// Ollama simulado: `/api/show` siempre bien; `/api/chat` con el estado y el cuerpo dados.
struct Server {
    url: String,
    chats: Arc<AtomicUsize>,
}

fn ollama(status: u16, body: Value) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let chats = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&chats);
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                continue;
            }
            let mut length = 0;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
                if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut sent = vec![0; length];
            let _ = reader.read_exact(&mut sent);
            let (status, reply) = if line.starts_with("POST /api/show ") {
                (200, json!({ "details": { "family": "qwen3" } }))
            } else {
                counter.fetch_add(1, Ordering::SeqCst);
                (status, body.clone())
            };
            let reply = reply.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    Server { url, chats }
}

fn chat_ok(content: &str) -> Value {
    json!({ "model": "m", "message": { "role": "assistant", "content": content } })
}

fn good_email() -> Value {
    chat_ok(
        &json!({ "category": "solicitud_factura", "draft": "Adjuntamos la factura." }).to_string(),
    )
}

fn local(url: &str) -> ProviderChoice {
    ProviderChoice::Ollama {
        base_url: url.to_owned(),
        model: "m".to_owned(),
    }
}

fn cloud() -> ProviderChoice {
    ProviderChoice::OllamaCloud {
        model: "gemma4:31b".to_owned(),
    }
}

/// La aplicación con `choice` como elección vigente y una espera de D146 ya puesta.
fn world(
    dir: &tempfile::TempDir,
    choice: &ProviderChoice,
    consent: bool,
) -> (SharedState, Receiver<()>) {
    let mut settings = Settings {
        provider: Some(choice.clone()),
        ..Settings::default()
    };
    if consent {
        settings.record_consent(choice);
    }
    let credentials: Arc<dyn CredentialStore> = Arc::new(MemoryCredentialStore::default());
    let mut app = App::open(
        &AppPaths::new(dir.path()),
        settings::waiting_provider(Some(choice)),
        Arc::new(extractor()),
    )
    .unwrap();
    app.set_ai_wait(Some(AiWait::ModelUnavailable {
        cause: ModelUnavailableCause::Other,
        status: 404,
    }));
    let (sender, receiver) = mpsc::channel();
    let shared = Arc::new(Shared {
        app: Mutex::new(app),
        settings: Mutex::new(settings),
        credentials,
        data_dir: dir.path().to_path_buf(),
        local_setup: Arc::new(windows_setup(dir.path())),
        sign_in: Mutex::new(None),
        mail_clients_error: None,
        wake: Mutex::new(sender),
        stopping: AtomicBool::new(false),
    });
    (shared, receiver)
}

fn wait(shared: &SharedState) -> Option<AiWait> {
    shared.app().unwrap().ai_wait()
}

#[test]
fn a_successful_probe_lifts_the_wait_and_wakes_the_queue() {
    let server = ollama(200, good_email());
    let choice = local(&server.url);
    let dir = tempfile::tempdir().unwrap();
    let (shared, woken) = world(&dir, &choice, true);
    assert!(run_probe(&shared, &choice));
    assert_eq!(wait(&shared), None);
    assert!(woken.try_recv().is_ok(), "despierta la cola");
    assert_eq!(server.chats.load(Ordering::SeqCst), 1);
}

#[test]
fn an_answer_with_the_wrong_format_also_lifts_it_because_the_service_responds() {
    // D105: la tarea fallará con `invalid_response` por su cuenta; el servicio ya responde.
    let server = ollama(200, chat_ok("esto no es JSON"));
    let choice = local(&server.url);
    let dir = tempfile::tempdir().unwrap();
    let (shared, _) = world(&dir, &choice, true);
    let outcome = probe_provider(&shared, &choice);
    assert!(matches!(
        outcome,
        Err(ProbeError::Ai(nestadmin_ai::AiError::InvalidResponse(_)))
    ));
    assert!(settle_probe(&shared, &choice, &outcome));
    assert_eq!(wait(&shared), None);
}

#[test]
fn a_401_with_the_current_choice_turns_the_wait_into_an_invalid_key() {
    let server = ollama(401, json!({}));
    let choice = local(&server.url);
    let dir = tempfile::tempdir().unwrap();
    let (shared, _) = world(&dir, &choice, true);
    assert!(run_probe(&shared, &choice));
    assert_eq!(wait(&shared), Some(AiWait::InvalidKey));
}

#[test]
fn other_errors_leave_the_wait_as_it_is() {
    for (status, body) in [
        (500, json!({})),
        (404, json!({ "error": "no" })),
        (429, json!({})),
    ] {
        let server = ollama(status, body);
        let choice = local(&server.url);
        let dir = tempfile::tempdir().unwrap();
        let (shared, woken) = world(&dir, &choice, true);
        let before = wait(&shared);
        assert!(!run_probe(&shared, &choice), "{status}");
        assert_eq!(wait(&shared), before, "{status}");
        assert!(woken.try_recv().is_err(), "{status}");
    }
}

#[test]
fn a_probe_of_a_choice_that_is_not_the_current_one_changes_nothing() {
    let server = ollama(200, good_email());
    let other = local(&server.url);
    let dir = tempfile::tempdir().unwrap();
    let (shared, _) = world(&dir, &cloud(), true);
    assert!(!lift_wait_if_current(&shared, &other));
    assert!(!run_probe(&shared, &other));
    assert!(wait(&shared).is_some());
}

#[test]
fn without_consent_or_key_nothing_is_sent_and_the_wait_stays() {
    let dir = tempfile::tempdir().unwrap();
    // Sin confirmar el destino: ni se lee la clave ni se construye el cliente.
    let (shared, _) = world(&dir, &cloud(), false);
    assert!(matches!(
        probe_provider(&shared, &cloud()),
        Err(ProbeError::NoConsent)
    ));
    assert!(!run_probe(&shared, &cloud()));
    assert!(wait(&shared).is_some());
    // Confirmado pero sin clave guardada: el proveedor no se construye y tampoco sale nada;
    // no se confunde con un 401 del servicio.
    let dir = tempfile::tempdir().unwrap();
    let (shared, _) = world(&dir, &cloud(), true);
    assert!(matches!(
        probe_provider(&shared, &cloud()),
        Err(ProbeError::Unprepared(nestadmin_ai::AiError::Unauthorized))
    ));
    assert!(!run_probe(&shared, &cloud()));
    assert!(matches!(
        wait(&shared),
        Some(AiWait::ModelUnavailable { .. })
    ));
}

#[test]
fn the_probe_is_due_every_fifteen_minutes_only_during_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let choice = cloud();
    let (shared, _) = world(&dir, &choice, true);
    let mut probe = probe::WaitProbe::default();
    let t0 = Instant::now();
    assert_eq!(
        probe_target(&shared, &mut probe, t0),
        None,
        "empieza a contar"
    );
    assert_eq!(
        probe_target(&shared, &mut probe, t0 + Duration::from_secs(14 * 60)),
        None
    );
    assert_eq!(
        probe_target(&shared, &mut probe, t0 + Duration::from_secs(15 * 60)),
        Some(choice)
    );
    // Sin espera no hay sondeo.
    shared.app().unwrap().lift_provider_wait();
    assert_eq!(
        probe_target(&shared, &mut probe, t0 + Duration::from_secs(40 * 60)),
        None
    );
}

#[test]
fn changing_or_deleting_a_key_lifts_the_wait_only_for_the_current_provider() {
    let dir = tempfile::tempdir().unwrap();
    let (shared, woken) = world(&dir, &cloud(), true);
    let claude = ProviderChoice::Anthropic {
        model: "claude-sonnet-5-5".to_owned(),
    };
    // La clave de otro proveedor no toca la espera.
    assert!(!lift_wait_if_same_credential(&shared, &claude));
    assert!(wait(&shared).is_some());
    assert!(woken.try_recv().is_err());
    // La IA local no tiene credencial: nunca coincide.
    assert!(!lift_wait_if_same_credential(
        &shared,
        &local("http://127.0.0.1:1")
    ));
    // La del proveedor vigente sí, aunque cambie el modelo de la elección que se pasa.
    let same = ProviderChoice::OllamaCloud {
        model: "otro".to_owned(),
    };
    assert!(lift_wait_if_same_credential(&shared, &same));
    assert_eq!(wait(&shared), None);
    assert!(woken.try_recv().is_ok());
}
