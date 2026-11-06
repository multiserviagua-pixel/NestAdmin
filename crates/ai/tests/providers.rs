//! T03 a nivel de biblioteca: selección local por defecto, ausencia de respaldo, errores
//! del contrato y credenciales que no se filtran. Los proveedores hablan con un servidor
//! HTTP simulado en 127.0.0.1; ninguna prueba sale a la red.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nestadmin_ai::anthropic::AnthropicProvider;
use nestadmin_ai::credentials::{ANTHROPIC_API_KEY, GROQ_API_KEY, OLLAMA_CLOUD_API_KEY};
use nestadmin_ai::groq::GroqProvider;
use nestadmin_ai::ollama::{is_cloud_model, is_loopback_http, OllamaProvider};
use nestadmin_ai::ollama_cloud::OllamaCloudProvider;
use nestadmin_ai::openai_compatible::{parse_base_url, OpenAiCompatibleProvider};
use nestadmin_ai::tasks::{email_request, extraction_request, parse_email, parse_extraction};
use nestadmin_ai::{
    build_provider, AiError, AiService, CredentialStore, MemoryCredentialStore, Provider,
    ProviderChoice, ProviderKind, Secret,
};
use serde_json::{json, Value};

const FAKE_KEY: &str = "gsk_prueba_no_real_0123456789";

struct Reply {
    status: u16,
    body: String,
    delay: Duration,
    headers: String,
}

fn reply(status: u16, body: Value) -> Reply {
    Reply {
        status,
        body: body.to_string(),
        delay: Duration::ZERO,
        headers: String::new(),
    }
}

/// Servidor que responde en orden y guarda cada petición recibida (cabeceras y cuerpo).
fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    thread::spawn(move || {
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            log.lock()
                .unwrap()
                .push(format!("{head}\r\n{}", String::from_utf8_lossy(&body)));
            thread::sleep(reply.delay);
            let _ = write!(
                stream,
                "HTTP/1.1 {} X\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.status,
                reply.headers,
                reply.body.len(),
                reply.body
            );
        }
    });
    (url, seen)
}

fn ollama_ok(content: &str) -> Reply {
    reply(
        200,
        json!({
            "model": "qwen3:8b",
            "message": { "role": "assistant", "content": content },
            "prompt_eval_count": 120,
            "eval_count": 40,
        }),
    )
}

/// `/api/show` de un modelo local: sin `remote_host` ni `remote_model`.
fn show_local() -> Reply {
    reply(200, json!({ "details": { "family": "qwen3" } }))
}

/// Almacén espía: registra los nombres de credencial pedidos.
#[derive(Default)]
struct SpyStore {
    inner: MemoryCredentialStore,
    asked: Mutex<Vec<String>>,
}

impl CredentialStore for SpyStore {
    fn get(&self, name: &str) -> Result<Option<Secret>, nestadmin_ai::CredentialError> {
        self.asked.lock().unwrap().push(name.to_owned());
        self.inner.get(name)
    }
    fn set(&self, name: &str, secret: &Secret) -> Result<(), nestadmin_ai::CredentialError> {
        self.inner.set(name, secret)
    }
    fn delete(&self, name: &str) -> Result<(), nestadmin_ai::CredentialError> {
        self.inner.delete(name)
    }
}

fn closed_port_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    url
}

fn body_of(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[test]
fn local_provider_is_the_default_choice() {
    let ProviderChoice::Ollama { base_url, model } = ProviderChoice::default() else {
        panic!("el proveedor inicial debe ser local");
    };
    assert_eq!(base_url, "http://127.0.0.1:11434");
    assert_eq!(model, "qwen3:8b");
}

#[test]
fn ollama_accepts_only_loopback_addresses() {
    for ok in [
        "http://127.0.0.1:11434",
        "http://localhost:11434/",
        "http://[::1]:11434",
        "http://127.0.0.1",
    ] {
        assert!(is_loopback_http(ok), "{ok}");
    }
    for bad in [
        "https://127.0.0.1:11434",
        "http://192.168.1.10:11434",
        "http://127.0.0.1.example.com:11434",
        "http://user@127.0.0.1:11434",
        "http://ollama.example.com",
        "http://[::1]x:11434",
        "127.0.0.1:11434",
    ] {
        assert!(!is_loopback_http(bad), "{bad}");
        assert!(matches!(
            OllamaProvider::new(bad, "m", Duration::from_secs(1)),
            Err(AiError::Unavailable(_))
        ));
    }
}

#[test]
fn ollama_extraction_sends_schema_without_reasoning_and_reports_model() {
    let output = json!({ "supplier": "Alba Solar SL", "invoice_number": "F-001", "date": "2026-01-10",
        "subtotal": "100.00", "tax": null, "total": "121.00" });
    let (url, seen) = serve(vec![show_local(), ollama_ok(&output.to_string())]);
    let provider = OllamaProvider::new(&url, "qwen3:8b", Duration::from_secs(5)).unwrap();
    let result = provider
        .generate(&extraction_request("D01", "texto sintético"))
        .unwrap();

    assert_eq!(result.provider, ProviderKind::Ollama);
    assert_eq!(result.model, "qwen3:8b");
    assert_eq!(result.request_id, "D01");
    assert_eq!(result.output, output);
    assert_eq!(result.usage.input_tokens, Some(120));
    let seen = seen.lock().unwrap();
    assert!(seen[0].starts_with("POST /api/show "));
    let request = &seen[1];
    assert!(request.starts_with("POST /api/chat "));
    let body = body_of(request);
    assert_eq!(body["stream"], false);
    assert_eq!(body["think"], false);
    assert_eq!(body["options"]["temperature"], 0);
    // Los seis campos más la lista de las demás facturas del documento (D68).
    assert_eq!(body["format"]["required"].as_array().unwrap().len(), 7);
    assert_eq!(body["messages"][1]["content"], "texto sintético");

    let raw = parse_extraction(&result.output).unwrap();
    assert_eq!(raw.tax, None, "un null del modelo es un dato ausente");
    assert_eq!(raw.total.unwrap().value, "121.00");
}

#[test]
fn contract_errors_from_the_local_provider() {
    let timeout = Duration::from_millis(400);
    let down = OllamaProvider::new(&closed_port_url(), "m", timeout).unwrap();
    assert!(matches!(
        down.generate(&extraction_request("x", "t")),
        Err(AiError::Unavailable(_))
    ));

    let (url, _) = serve(vec![
        show_local(),
        reply(500, json!({ "error": "fallo" })),
        ollama_ok("esto no es JSON"),
        ollama_ok("[1, 2]"),
        Reply {
            status: 200,
            body: "{}".into(),
            delay: Duration::from_millis(1500),
            headers: String::new(),
        },
    ]);
    let provider = OllamaProvider::new(&url, "m", timeout).unwrap();
    let request = extraction_request("x", "t");
    assert!(matches!(
        provider.generate(&request),
        Err(AiError::Unavailable(_))
    ));
    assert!(matches!(
        provider.generate(&request),
        Err(AiError::InvalidResponse(_))
    ));
    assert!(matches!(
        provider.generate(&request),
        Err(AiError::InvalidResponse(_))
    ));
    assert_eq!(provider.generate(&request), Err(AiError::Timeout));
}

#[test]
fn local_failure_never_reaches_the_external_provider() {
    // Groq está configurado y con credencial, pero no seleccionado.
    let (groq_url, groq_seen) = serve(vec![reply(200, json!({}))]);
    let credentials = MemoryCredentialStore::default();
    credentials
        .set(GROQ_API_KEY, &Secret::new(FAKE_KEY))
        .unwrap();
    let choice = ProviderChoice::Ollama {
        base_url: closed_port_url(),
        model: "qwen3:8b".into(),
    };
    let service =
        AiService::new(build_provider(&choice, &credentials, Duration::from_millis(300)).unwrap());

    for _ in 0..3 {
        let error = service
            .generate(&email_request("E01", "texto"))
            .unwrap_err();
        assert_eq!(error.code(), "unavailable");
    }
    assert_eq!(
        service.active().0,
        ProviderKind::Ollama,
        "el fallo mantiene la selección"
    );
    thread::sleep(Duration::from_millis(100));
    assert!(
        groq_seen.lock().unwrap().is_empty(),
        "ninguna petición llegó al proveedor externo"
    );
    drop(groq_url);
}

#[test]
fn external_provider_requires_explicit_choice_and_credential() {
    let empty = MemoryCredentialStore::default();
    let choice = ProviderChoice::Groq {
        model: "modelo-de-prueba".into(),
    };
    assert!(matches!(
        build_provider(&choice, &empty, Duration::from_secs(1)),
        Err(AiError::Unauthorized)
    ));

    empty.set(GROQ_API_KEY, &Secret::new(FAKE_KEY)).unwrap();
    let provider = build_provider(&choice, &empty, Duration::from_secs(1)).unwrap();
    assert_eq!(provider.kind(), ProviderKind::Groq);
}

#[test]
fn groq_sends_bearer_key_and_json_mode_and_parses_output() {
    let content =
        json!({ "category": "consulta_pago", "draft": "Revisaremos el estado del pago." });
    let (url, seen) = serve(vec![reply(
        200,
        json!({
            "model": "modelo-de-prueba",
            "choices": [{ "message": { "content": content.to_string() } }],
            "usage": { "prompt_tokens": 50, "completion_tokens": 12 },
        }),
    )]);
    let provider = GroqProvider::with_endpoint(
        &url,
        "modelo-de-prueba",
        Secret::new(FAKE_KEY),
        Duration::from_secs(5),
    );
    let result = provider
        .generate(&email_request("E05", "¿Cuándo pagan?"))
        .unwrap();
    assert_eq!(result.provider, ProviderKind::Groq);
    assert_eq!(parse_email(&result.output).unwrap().0, "consulta_pago");
    assert_eq!(result.usage.output_tokens, Some(12));

    let request = &seen.lock().unwrap()[0];
    assert!(request.starts_with("POST /chat/completions "));
    assert!(request.to_ascii_lowercase().contains(&format!(
        "authorization: bearer {}",
        FAKE_KEY.to_ascii_lowercase()
    )));
    let body = body_of(request);
    assert_eq!(body["response_format"]["type"], "json_object");
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("\"category\""));
}

#[test]
fn rejected_key_is_unauthorized_and_never_printed() {
    let (url, _) = serve(vec![reply(
        401,
        json!({ "error": { "message": "invalid api key" } }),
    )]);
    let provider =
        GroqProvider::with_endpoint(&url, "m", Secret::new(FAKE_KEY), Duration::from_secs(5));
    let error = provider.generate(&email_request("E01", "t")).unwrap_err();
    assert_eq!(error, AiError::Unauthorized);
    for text in [
        format!("{error}"),
        format!("{error:?}"),
        format!("{provider:?}"),
        format!("{:?}", Secret::new(FAKE_KEY)),
    ] {
        assert!(!text.contains(FAKE_KEY), "la clave aparece en: {text}");
    }
}

#[test]
fn model_output_outside_the_contract_is_invalid() {
    assert!(matches!(
        parse_extraction(&json!({ "total": 121.0 })),
        Err(AiError::InvalidResponse(_))
    ));
    assert!(matches!(
        parse_extraction(&json!("texto")),
        Err(AiError::InvalidResponse(_))
    ));
    let raw = parse_extraction(&json!({ "supplier": "  ", "date": null })).unwrap();
    assert!(raw.supplier.is_none() && raw.date.is_none() && raw.total.is_none());

    assert!(matches!(
        parse_email(&json!({ "category": "spam", "draft": "x" })),
        Err(AiError::InvalidResponse(_))
    ));
    assert!(matches!(
        parse_email(&json!({ "category": "otros" })),
        Err(AiError::InvalidResponse(_))
    ));
}

// Ollama cloud (D92, D93): segundo proveedor externo, mismas garantías que Groq.

const CLOUD_KEY: &str = "ollama_prueba_no_real_9876543210";

fn cloud_choice() -> ProviderChoice {
    ProviderChoice::OllamaCloud {
        model: "modelo-nube-de-prueba".into(),
    }
}

#[test]
fn ollama_cloud_requires_explicit_choice_and_its_own_credential() {
    // Sin ninguna clave y con solo la de Groq: unauthorized, sin construir cliente.
    let store = MemoryCredentialStore::default();
    assert!(matches!(
        build_provider(&cloud_choice(), &store, Duration::from_secs(1)),
        Err(AiError::Unauthorized)
    ));
    store.set(GROQ_API_KEY, &Secret::new(FAKE_KEY)).unwrap();
    assert!(matches!(
        build_provider(&cloud_choice(), &store, Duration::from_secs(1)),
        Err(AiError::Unauthorized)
    ));

    store
        .set(OLLAMA_CLOUD_API_KEY, &Secret::new(CLOUD_KEY))
        .unwrap();
    let provider = build_provider(&cloud_choice(), &store, Duration::from_secs(1)).unwrap();
    assert_eq!(provider.kind(), ProviderKind::OllamaCloud);
    assert_eq!(provider.model(), "modelo-nube-de-prueba");
    assert_eq!(provider.kind().to_string(), "ollama_cloud");
}

#[test]
fn ollama_cloud_choice_serializes_as_provider_and_model_only() {
    let text = serde_json::to_string(&cloud_choice()).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({ "provider": "ollama_cloud", "model": "modelo-nube-de-prueba" })
    );
    assert_eq!(
        serde_json::from_str::<ProviderChoice>(&text).unwrap(),
        cloud_choice()
    );
}

#[test]
fn ollama_cloud_sends_bearer_to_chat_endpoint_and_parses_native_response() {
    let content =
        json!({ "category": "consulta_pago", "draft": "Revisaremos el estado del pago." });
    let (url, seen) = serve(vec![ollama_ok(&content.to_string())]);
    let provider = OllamaCloudProvider::with_endpoint(
        &format!("{url}/api"),
        "modelo-nube-de-prueba",
        Secret::new(CLOUD_KEY),
        Duration::from_secs(5),
    );
    let result = provider
        .generate(&email_request("E05", "¿Cuándo pagan?"))
        .unwrap();
    assert_eq!(result.provider, ProviderKind::OllamaCloud);
    assert_eq!(result.usage.input_tokens, Some(120));
    assert_eq!(parse_email(&result.output).unwrap().0, "consulta_pago");

    let request = &seen.lock().unwrap()[0];
    assert!(request.starts_with("POST /api/chat "), "{request}");
    assert!(request.to_ascii_lowercase().contains(&format!(
        "authorization: bearer {}",
        CLOUD_KEY.to_ascii_lowercase()
    )));
    let body = body_of(request);
    assert_eq!(body["model"], "modelo-nube-de-prueba");
    assert_eq!(body["stream"], false);
    assert_eq!(body["think"], false);
    assert!(body["format"].is_object());
}

#[test]
fn ollama_cloud_default_endpoint_is_the_fixed_remote_api() {
    assert_eq!(
        nestadmin_ai::ollama_cloud::ENDPOINT,
        "https://ollama.com/api"
    );
}

#[test]
fn ollama_cloud_rejected_key_is_unauthorized_and_never_printed() {
    let (url, _) = serve(vec![
        reply(401, json!({ "error": "unauthorized" })),
        reply(403, json!({ "error": "forbidden" })),
    ]);
    let provider = OllamaCloudProvider::with_endpoint(
        &url,
        "m",
        Secret::new(CLOUD_KEY),
        Duration::from_secs(5),
    );
    for _ in 0..2 {
        let error = provider.generate(&email_request("E01", "t")).unwrap_err();
        assert_eq!(error, AiError::Unauthorized);
        for text in [
            format!("{error}"),
            format!("{error:?}"),
            format!("{provider:?}"),
        ] {
            assert!(!text.contains(CLOUD_KEY), "la clave aparece en: {text}");
        }
    }
}

#[test]
fn ollama_cloud_contract_errors() {
    let timeout = Duration::from_millis(400);
    let down = OllamaCloudProvider::with_endpoint(
        &closed_port_url(),
        "m",
        Secret::new(CLOUD_KEY),
        timeout,
    );
    assert!(matches!(
        down.generate(&extraction_request("x", "t")),
        Err(AiError::Unavailable(_))
    ));

    let (url, _) = serve(vec![
        ollama_ok("esto no es JSON"),
        reply(200, json!({ "message": {} })),
        Reply {
            status: 200,
            body: "{}".into(),
            delay: Duration::from_millis(1500),
            headers: String::new(),
        },
    ]);
    let provider = OllamaCloudProvider::with_endpoint(&url, "m", Secret::new(CLOUD_KEY), timeout);
    let request = extraction_request("x", "t");
    assert!(matches!(
        provider.generate(&request),
        Err(AiError::InvalidResponse(_))
    ));
    assert!(matches!(
        provider.generate(&request),
        Err(AiError::InvalidResponse(_))
    ));
    assert_eq!(provider.generate(&request), Err(AiError::Timeout));
}

#[test]
fn ollama_cloud_rate_limit_is_reported_without_waiting_or_retrying() {
    // Solo Groq espera y reintenta (D65): Ollama cloud devuelve `rate_limited` a la primera,
    // aunque `Retry-After` pida poco, y no hace una segunda petición.
    let limited = rate_limited("0");
    let (url, seen) = serve(vec![limited, ollama_ok("{}")]);
    let provider = OllamaCloudProvider::with_endpoint(
        &url,
        "m",
        Secret::new(CLOUD_KEY),
        Duration::from_secs(5),
    );
    let started = std::time::Instant::now();
    let error = provider.generate(&email_request("E01", "t")).unwrap_err();
    assert_eq!(error.code(), "rate_limited", "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(seen.lock().unwrap().len(), 1, "no hay reintento");
    for text in [
        format!("{error}"),
        format!("{error:?}"),
        format!("{provider:?}"),
    ] {
        assert!(!text.contains(CLOUD_KEY), "la clave aparece en: {text}");
    }
}

#[test]
fn local_failure_never_reaches_ollama_cloud() {
    let (_cloud_url, cloud_seen) = serve(vec![ollama_ok("{}")]);
    let credentials = SpyStore::default();
    credentials
        .set(OLLAMA_CLOUD_API_KEY, &Secret::new(CLOUD_KEY))
        .unwrap();
    credentials
        .set(GROQ_API_KEY, &Secret::new(FAKE_KEY))
        .unwrap();
    let choice = ProviderChoice::Ollama {
        base_url: closed_port_url(),
        model: "qwen3:8b".into(),
    };
    let service =
        AiService::new(build_provider(&choice, &credentials, Duration::from_millis(300)).unwrap());
    for _ in 0..3 {
        let error = service
            .generate(&email_request("E01", "texto"))
            .unwrap_err();
        assert_eq!(error.code(), "unavailable");
    }
    assert_eq!(service.active().0, ProviderKind::Ollama);
    thread::sleep(Duration::from_millis(100));
    assert!(cloud_seen.lock().unwrap().is_empty());
    assert!(
        credentials.asked.lock().unwrap().is_empty(),
        "con Ollama local no se pide ninguna clave: {:?}",
        credentials.asked.lock().unwrap()
    );
}

#[test]
fn ollama_cloud_failure_never_falls_back_to_local_or_groq() {
    // Un Ollama local y un Groq simulados, vivos, con credencial de Groq presente.
    let (_local_url, local_seen) = serve(vec![show_local(), ollama_ok("{}")]);
    let (_groq_url, groq_seen) = serve(vec![reply(200, json!({}))]);
    let credentials = SpyStore::default();
    credentials
        .set(OLLAMA_CLOUD_API_KEY, &Secret::new(CLOUD_KEY))
        .unwrap();
    credentials
        .set(GROQ_API_KEY, &Secret::new(FAKE_KEY))
        .unwrap();
    // La nube falla (puerto cerrado): el servicio devuelve el error y conserva la selección.
    // Se lee la clave como lo hace `build_provider` y se apunta a un extremo inalcanzable.
    let key = credentials.get(OLLAMA_CLOUD_API_KEY).unwrap().unwrap();
    let cloud = OllamaCloudProvider::with_endpoint(
        &closed_port_url(),
        "m",
        key,
        Duration::from_millis(300),
    );
    let service = AiService::new(Box::new(cloud));
    for _ in 0..3 {
        let error = service
            .generate(&email_request("E01", "texto"))
            .unwrap_err();
        assert_eq!(error.code(), "unavailable");
    }
    assert_eq!(service.active().0, ProviderKind::OllamaCloud);
    thread::sleep(Duration::from_millis(100));
    assert!(local_seen.lock().unwrap().is_empty(), "nada llegó al local");
    assert!(groq_seen.lock().unwrap().is_empty(), "nada llegó a Groq");
    assert_eq!(
        *credentials.asked.lock().unwrap(),
        vec![OLLAMA_CLOUD_API_KEY.to_owned()],
        "con la nube elegida nunca se pide la clave de Groq"
    );
}

#[test]
fn local_provider_rejects_aliases_of_remote_models_via_api_show() {
    for field in ["remote_host", "remote_model"] {
        let (url, seen) = serve(vec![
            reply(200, json!({ field: "https://ollama.com:443" })),
            ollama_ok("{}"),
        ]);
        let provider = OllamaProvider::new(&url, "factura", Duration::from_secs(2)).unwrap();
        let error = provider.generate(&email_request("E01", "x")).unwrap_err();
        assert!(matches!(&error, AiError::Unavailable(m) if m.contains("Ollama cloud")));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{field}: solo /api/show");
        assert!(seen[0].starts_with("POST /api/show "));
        assert_eq!(body_of(&seen[0])["model"], "factura");
    }
    // Campos vacíos no cuentan como remotos.
    let (url, seen) = serve(vec![
        reply(200, json!({ "remote_host": "", "remote_model": null })),
        ollama_ok("{}"),
    ]);
    let provider = OllamaProvider::new(&url, "factura", Duration::from_secs(2)).unwrap();
    provider.generate(&email_request("E01", "x")).ok();
    assert!(seen.lock().unwrap()[1].starts_with("POST /api/chat "));
}

#[test]
fn local_provider_fails_closed_when_api_show_fails() {
    let timeout = Duration::from_millis(400);
    // Daemon caído.
    let down = OllamaProvider::new(&closed_port_url(), "m", timeout).unwrap();
    assert!(matches!(
        down.generate(&email_request("E01", "x")),
        Err(AiError::Unavailable(_))
    ));
    // Error HTTP, JSON inválido y lentitud en /api/show; /api/chat nunca se llama.
    let (url, seen) = serve(vec![
        reply(500, json!({ "error": "fallo" })),
        reply(200, json!([1, 2])),
        Reply {
            status: 200,
            body: "{}".into(),
            delay: Duration::from_millis(1500),
            headers: String::new(),
        },
        ollama_ok("{}"),
    ]);
    let provider = OllamaProvider::new(&url, "m", timeout).unwrap();
    let request = email_request("E01", "x");
    assert!(matches!(
        provider.generate(&request),
        Err(AiError::Unavailable(_))
    ));
    assert!(matches!(
        provider.generate(&request),
        Err(AiError::InvalidResponse(_))
    ));
    assert_eq!(provider.generate(&request), Err(AiError::Timeout));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().all(|r| r.starts_with("POST /api/show ")));
}

#[test]
fn local_provider_checks_api_show_once_per_model() {
    let (url, seen) = serve(vec![show_local(), ollama_ok("{}"), ollama_ok("{}")]);
    let provider = OllamaProvider::new(&url, "qwen3:8b", Duration::from_secs(2)).unwrap();
    for _ in 0..2 {
        provider.generate(&email_request("E01", "x")).ok();
    }
    let seen = seen.lock().unwrap();
    let shows = seen
        .iter()
        .filter(|r| r.starts_with("POST /api/show "))
        .count();
    let chats = seen
        .iter()
        .filter(|r| r.starts_with("POST /api/chat "))
        .count();
    assert_eq!((shows, chats), (1, 2));
}

#[test]
fn local_provider_rejects_cloud_models() {
    let store = MemoryCredentialStore::default();
    for model in [
        "x:cloud",
        "gemma4:cloud",
        "gpt-oss:120b-cloud",
        "x:8b-cloud",
        "X:Cloud",
        "modelo-cloud",
    ] {
        assert!(is_cloud_model(model), "{model}");
        let error = OllamaProvider::new("http://127.0.0.1:11434", model, Duration::from_secs(1))
            .err()
            .unwrap_or_else(|| panic!("{model} debía rechazarse"));
        assert!(matches!(&error, AiError::Unavailable(m) if m.contains("Ollama cloud")));
        // También a través de la selección.
        let choice = ProviderChoice::Ollama {
            base_url: "http://127.0.0.1:11434".into(),
            model: model.into(),
        };
        assert!(build_provider(&choice, &store, Duration::from_secs(1)).is_err());
    }
    for model in ["qwen3:8b", "gpt-oss:120b", "cloudy:latest", "gemma4:31b"] {
        assert!(!is_cloud_model(model), "{model}");
        assert!(
            OllamaProvider::new("http://127.0.0.1:11434", model, Duration::from_secs(1)).is_ok()
        );
    }
}

// ---------------------------------------------------------------------------------------
// Proveedor genérico compatible con OpenAI (D95) y Claude nativo (D96).
// ---------------------------------------------------------------------------------------

const COMPAT_KEY: &str = "sk-compat-prueba-no-real-1122334455";
const ANTHROPIC_KEY: &str = "sk-ant-prueba-no-real-5544332211";

fn compat_choice(base_url: &str) -> ProviderChoice {
    ProviderChoice::OpenAiCompatible {
        base_url: base_url.into(),
        model: "modelo-generico-de-prueba".into(),
    }
}

fn anthropic_choice() -> ProviderChoice {
    ProviderChoice::Anthropic {
        model: "claude-opus-5-5".into(),
    }
}

fn chat_ok(content: &str) -> Reply {
    reply(
        200,
        json!({
            "model": "modelo-generico-de-prueba",
            "choices": [{ "message": { "content": content } }],
            "usage": { "prompt_tokens": 30, "completion_tokens": 7 },
        }),
    )
}

fn messages_ok(content: Value, stop_reason: &str) -> Reply {
    reply(
        200,
        json!({
            "id": "msg_prueba",
            "type": "message",
            "model": "claude-opus-5-5",
            "role": "assistant",
            "content": content,
            "stop_reason": stop_reason,
            "usage": { "input_tokens": 80, "output_tokens": 25 },
        }),
    )
}

fn assert_never_printed(secret: &str, texts: &[String]) {
    for text in texts {
        assert!(!text.contains(secret), "la clave aparece en: {text}");
    }
}

#[test]
fn default_cloud_model_is_gemma4() {
    assert_eq!(nestadmin_ai::ollama::DEFAULT_CLOUD_MODEL, "gemma4:31b");
    assert_eq!(nestadmin_ai::anthropic::DEFAULT_MODEL, "claude-opus-5-5");
}

#[test]
fn new_external_choices_serialize_with_their_own_tag() {
    let text = serde_json::to_string(&compat_choice("https://api.example.com/v1")).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({
            "provider": "openai_compatible",
            "base_url": "https://api.example.com/v1",
            "model": "modelo-generico-de-prueba",
        })
    );
    assert_eq!(
        serde_json::from_str::<ProviderChoice>(&text).unwrap(),
        compat_choice("https://api.example.com/v1")
    );
    let text = serde_json::to_string(&anthropic_choice()).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({ "provider": "anthropic", "model": "claude-opus-5-5" })
    );
    assert_eq!(
        ProviderKind::OpenAiCompatible.to_string(),
        "openai_compatible"
    );
    assert_eq!(ProviderKind::Anthropic.to_string(), "anthropic");
}

#[test]
fn openai_compatible_base_url_accepts_public_https_and_normalizes() {
    for (raw, url, host) in [
        (
            "https://api.example.com/v1",
            "https://api.example.com/v1",
            "api.example.com",
        ),
        (
            "HTTPS://API.Example.COM/v1/",
            "https://api.example.com/v1",
            "api.example.com",
        ),
        (
            "  https://openrouter.example.ai/api/v1//  ",
            "https://openrouter.example.ai/api/v1",
            "openrouter.example.ai",
        ),
        (
            "https://api.example.com:8443",
            "https://api.example.com:8443",
            "api.example.com",
        ),
        (
            "https://api.example.com./v1",
            "https://api.example.com/v1",
            "api.example.com",
        ),
        ("https://8.8.8.8/v1", "https://8.8.8.8/v1", "8.8.8.8"),
        (
            "https://[2606:4700::1111]/v1",
            "https://[2606:4700::1111]/v1",
            "2606:4700::1111",
        ),
    ] {
        let base = parse_base_url(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
        assert_eq!(base.url, url, "{raw}");
        assert_eq!(base.host, host, "{raw}");
    }
    assert_eq!(
        parse_base_url("https://API.Example.com/v1")
            .unwrap()
            .credential_name(),
        "openai_compatible_api_key:api.example.com"
    );
}

#[test]
fn openai_compatible_base_url_rejects_unsafe_targets() {
    for bad in [
        "",
        "api.example.com/v1",
        "http://api.example.com/v1",
        "ftp://api.example.com",
        "https://",
        "https:///v1",
        // Bucle local y red local.
        "https://localhost/v1",
        "https://LOCALHOST:8443/v1",
        "https://localhost./v1",
        "https://api.localhost/v1",
        "https://127.0.0.1/v1",
        "https://127.255.255.254/v1",
        "https://[::1]/v1",
        "https://[::ffff:127.0.0.1]/v1",
        "https://10.0.0.5/v1",
        "https://172.16.0.1/v1",
        "https://172.31.255.255/v1",
        "https://192.168.1.10/v1",
        "https://169.254.169.254/latest",
        "https://0.0.0.0/v1",
        "https://100.64.0.1/v1",
        "https://[fc00::1]/v1",
        "https://[fd12:3456::1]/v1",
        "https://[fe80::1]/v1",
        "https://impresora.local/v1",
        "https://nas/v1",
        "https://servidor.lan/v1",
        "https://app.internal/v1",
        // IPv6 especiales, con zona, con puerto y formas con punto final o ceros.
        "https://[::]/v1",
        "https://[::127.0.0.1]/v1",
        "https://[64:ff9b::7f00:1]/v1",
        "https://[fe80::1%25eth0]/v1",
        "https://[::1]:443/v1",
        "https://127.0.0.1./v1",
        "https://127.000.000.001/v1",
        // Rangos reservados y de documentación.
        "https://198.18.0.1/v1",
        "https://198.19.255.254/v1",
        "https://192.0.0.8/v1",
        "https://192.0.2.10/v1",
        "https://198.51.100.7/v1",
        "https://203.0.113.9/v1",
        "https://240.0.0.1/v1",
        "https://255.255.255.255/v1",
        "https://224.0.0.1/v1",
        "https://[ff02::1]/v1",
        "https://[2001:db8::1]/v1",
        "https://[2002:7f00:1::1]/v1",
        "https://[2002:c0a8:101::1]/v1",
        // Formas numéricas que algunos resolvedores convierten en 127.0.0.1.
        "https://2130706433/v1",
        "https://0x7f.0.0.1/v1",
        "https://0177.0.0.1/v1",
        // Credenciales, query, fragmento y confusiones de autoridad.
        "https://usuario:clave@api.example.com/v1",
        "https://usuario@api.example.com/v1",
        "https://api.example.com@127.0.0.1/v1",
        "https://api.example.com/v1?key=1",
        "https://api.example.com?x=1",
        "https://api.example.com/v1#frag",
        "https://api.example.com\\@127.0.0.1/v1",
        "https://api.example.com:0/v1",
        "https://api.example.com:99999/v1",
        "https://api.example.com:abc/v1",
        "https://api example.com/v1",
        "https://-api.example.com/v1",
        "https://api..example.com/v1",
        "https://ápi.example.com/v1",
    ] {
        match parse_base_url(bad) {
            Err(AiError::Unavailable(_)) => {}
            other => panic!("{bad:?} debía rechazarse: {other:?}"),
        }
    }
}

#[test]
fn openai_compatible_requires_explicit_choice_and_the_key_of_that_host() {
    let choice = compat_choice("https://api.example.com/v1");
    let store = SpyStore::default();
    // Claves de todos los demás proveedores y de OTRO host: no valen.
    store.set(GROQ_API_KEY, &Secret::new(FAKE_KEY)).unwrap();
    store
        .set(OLLAMA_CLOUD_API_KEY, &Secret::new(CLOUD_KEY))
        .unwrap();
    store
        .set(ANTHROPIC_API_KEY, &Secret::new(ANTHROPIC_KEY))
        .unwrap();
    store
        .set(
            "openai_compatible_api_key:otro-host.example.org",
            &Secret::new(COMPAT_KEY),
        )
        .unwrap();
    assert!(matches!(
        build_provider(&choice, &store, Duration::from_secs(1)),
        Err(AiError::Unauthorized)
    ));
    assert_eq!(
        *store.asked.lock().unwrap(),
        vec!["openai_compatible_api_key:api.example.com".to_owned()],
        "solo se pide la clave del host configurado"
    );

    store
        .set(
            "openai_compatible_api_key:api.example.com",
            &Secret::new(COMPAT_KEY),
        )
        .unwrap();
    let provider = build_provider(&choice, &store, Duration::from_secs(1)).unwrap();
    assert_eq!(provider.kind(), ProviderKind::OpenAiCompatible);
    assert_eq!(provider.model(), "modelo-generico-de-prueba");
    // El host se compara sin distinguir mayúsculas.
    let upper = compat_choice("https://API.EXAMPLE.COM/v1");
    assert!(build_provider(&upper, &store, Duration::from_secs(1)).is_ok());
}

#[test]
fn openai_compatible_key_of_one_host_is_not_used_for_another_host() {
    let store = SpyStore::default();
    store
        .set(
            "openai_compatible_api_key:api.example.com",
            &Secret::new(COMPAT_KEY),
        )
        .unwrap();
    // Se cambia la URL a otro host: sin clave para él, `unauthorized` y sin cliente.
    let moved = compat_choice("https://api.otro-proveedor.example.net/v1");
    assert!(matches!(
        build_provider(&moved, &store, Duration::from_secs(1)),
        Err(AiError::Unauthorized)
    ));
    assert_eq!(
        *store.asked.lock().unwrap(),
        vec!["openai_compatible_api_key:api.otro-proveedor.example.net".to_owned()],
        "la clave del host anterior ni siquiera se lee"
    );
}

#[test]
fn openai_compatible_accepts_public_6to4_and_borders_of_reserved_ranges() {
    for ok in [
        "https://[2002:0808:0808::1]/v1", // 6to4 de 8.8.8.8
        "https://198.17.255.255/v1",
        "https://198.20.0.1/v1",
        "https://203.0.114.1/v1",
    ] {
        parse_base_url(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
}

#[test]
fn openai_compatible_key_is_bound_to_host_and_port() {
    assert_eq!(
        parse_base_url("https://api.example.com/v1")
            .unwrap()
            .credential_name(),
        "openai_compatible_api_key:api.example.com"
    );
    assert_eq!(
        parse_base_url("https://api.example.com:443/v1")
            .unwrap()
            .credential_name(),
        "openai_compatible_api_key:api.example.com",
        "el puerto por defecto no se añade al nombre"
    );
    assert_eq!(
        parse_base_url("https://api.example.com:8443/v1")
            .unwrap()
            .credential_name(),
        "openai_compatible_api_key:api.example.com:8443"
    );
    let store = SpyStore::default();
    store
        .set(
            "openai_compatible_api_key:api.example.com",
            &Secret::new(COMPAT_KEY),
        )
        .unwrap();
    let other_port = compat_choice("https://api.example.com:8443/v1");
    assert!(matches!(
        build_provider(&other_port, &store, Duration::from_secs(1)),
        Err(AiError::Unauthorized)
    ));
    assert_eq!(
        *store.asked.lock().unwrap(),
        vec!["openai_compatible_api_key:api.example.com:8443".to_owned()],
        "la clave del puerto 443 no se lee para el 8443"
    );
}

#[test]
fn openai_compatible_invalid_url_fails_before_reading_any_key() {
    let store = SpyStore::default();
    store
        .set(
            "openai_compatible_api_key:localhost",
            &Secret::new(COMPAT_KEY),
        )
        .unwrap();
    for url in [
        "http://api.example.com/v1",
        "https://localhost/v1",
        "https://u:p@api.example.com",
    ] {
        let error = build_provider(&compat_choice(url), &store, Duration::from_secs(1))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(error.code(), "unavailable", "{url}");
    }
    assert!(store.asked.lock().unwrap().is_empty());
    // El constructor público también valida.
    assert!(OpenAiCompatibleProvider::new(
        "https://127.0.0.1/v1",
        "m",
        Secret::new(COMPAT_KEY),
        Duration::from_secs(1)
    )
    .is_err());
}

#[test]
fn openai_compatible_sends_bearer_to_chat_completions_and_parses_output() {
    let content =
        json!({ "category": "consulta_pago", "draft": "Revisaremos el estado del pago." });
    let (url, seen) = serve(vec![chat_ok(&content.to_string())]);
    let provider = OpenAiCompatibleProvider::with_endpoint(
        &format!("{url}/v1/"),
        "modelo-generico-de-prueba",
        Secret::new(COMPAT_KEY),
        Duration::from_secs(5),
    );
    let result = provider
        .generate(&email_request("E05", "¿Cuándo pagan?"))
        .unwrap();
    assert_eq!(result.provider, ProviderKind::OpenAiCompatible);
    assert_eq!(parse_email(&result.output).unwrap().0, "consulta_pago");
    assert_eq!(result.usage.input_tokens, Some(30));
    let request = &seen.lock().unwrap()[0];
    assert!(
        request.starts_with("POST /v1/chat/completions "),
        "{request}"
    );
    assert!(request.to_ascii_lowercase().contains(&format!(
        "authorization: bearer {}",
        COMPAT_KEY.to_ascii_lowercase()
    )));
    let body = body_of(request);
    assert_eq!(body["model"], "modelo-generico-de-prueba");
    assert_eq!(body["response_format"]["type"], "json_object");
}

#[test]
fn openai_compatible_errors_follow_the_contract_and_hide_the_key() {
    let timeout = Duration::from_millis(400);
    let (url, _) = serve(vec![
        reply(401, json!({ "error": "invalid" })),
        reply(403, json!({ "error": "forbidden" })),
        reply(429, json!({ "error": "rate" })),
        reply(503, json!({ "error": "down" })),
        chat_ok("esto no es JSON"),
        reply(200, json!({ "choices": [] })),
        Reply {
            status: 200,
            body: "{}".into(),
            delay: Duration::from_millis(1500),
            headers: String::new(),
        },
    ]);
    let provider =
        OpenAiCompatibleProvider::with_endpoint(&url, "m", Secret::new(COMPAT_KEY), timeout);
    let request = extraction_request("x", "t");
    let mut printed = vec![format!("{provider:?}")];
    for expected in [
        "unauthorized",
        "unauthorized",
        "rate_limited",
        "unavailable",
        "invalid_response",
        "invalid_response",
        "timeout",
    ] {
        let error = provider.generate(&request).unwrap_err();
        assert_eq!(error.code(), expected, "{error}");
        printed.push(format!("{error} {error:?}"));
    }
    assert_never_printed(COMPAT_KEY, &printed);
    let down = OpenAiCompatibleProvider::with_endpoint(
        &closed_port_url(),
        "m",
        Secret::new(COMPAT_KEY),
        timeout,
    );
    assert!(matches!(
        down.generate(&request),
        Err(AiError::Unavailable(_))
    ));
}

#[test]
fn openai_compatible_does_not_follow_redirects_with_the_key() {
    let (target_url, target_seen) = serve(vec![chat_ok("{}")]);
    // Redirector mínimo: el servidor simulado general no emite cabeceras propias.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let redirector = format!("http://{}", listener.local_addr().unwrap());
    let location = format!("{target_url}/destino");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = write!(
                stream,
                "HTTP/1.1 307 X\r\nLocation: {location}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            );
        }
    });
    let provider = OpenAiCompatibleProvider::with_endpoint(
        &redirector,
        "m",
        Secret::new(COMPAT_KEY),
        Duration::from_secs(3),
    );
    let error = provider
        .generate(&extraction_request("x", "t"))
        .unwrap_err();
    assert_eq!(error.code(), "unavailable");
    thread::sleep(Duration::from_millis(100));
    assert!(
        target_seen.lock().unwrap().is_empty(),
        "la redirección no se siguió"
    );
}

#[test]
fn compat_failure_never_falls_back_to_other_providers() {
    let (_local_url, local_seen) = serve(vec![show_local(), ollama_ok("{}")]);
    let (_groq_url, groq_seen) = serve(vec![chat_ok("{}")]);
    let store = SpyStore::default();
    for (name, key) in [
        (GROQ_API_KEY, FAKE_KEY),
        (OLLAMA_CLOUD_API_KEY, CLOUD_KEY),
        (ANTHROPIC_API_KEY, ANTHROPIC_KEY),
        ("openai_compatible_api_key:api.example.com", COMPAT_KEY),
    ] {
        store.set(name, &Secret::new(key)).unwrap();
    }
    let built = build_provider(
        &compat_choice("https://api.example.com/v1"),
        &store,
        Duration::from_millis(300),
    )
    .unwrap();
    assert_eq!(built.kind(), ProviderKind::OpenAiCompatible);
    // Fallo del servicio externo (puerto cerrado): el error se devuelve y se mantiene la
    // selección. Se apunta a un extremo inalcanzable con la clave leída por `build_provider`.
    let key = store
        .get("openai_compatible_api_key:api.example.com")
        .unwrap()
        .unwrap();
    let failing = OpenAiCompatibleProvider::with_endpoint(
        &closed_port_url(),
        "m",
        key,
        Duration::from_millis(300),
    );
    let service = AiService::new(Box::new(failing));
    for _ in 0..3 {
        let error = service
            .generate(&email_request("E01", "texto"))
            .unwrap_err();
        assert_eq!(error.code(), "unavailable");
    }
    assert_eq!(service.active().0, ProviderKind::OpenAiCompatible);
    thread::sleep(Duration::from_millis(100));
    assert!(local_seen.lock().unwrap().is_empty());
    assert!(groq_seen.lock().unwrap().is_empty());
    assert_eq!(
        *store.asked.lock().unwrap(),
        vec![
            "openai_compatible_api_key:api.example.com".to_owned(),
            "openai_compatible_api_key:api.example.com".to_owned()
        ],
        "solo se piden claves del host elegido (una lectura es la del propio test)"
    );
}

#[test]
fn anthropic_requires_explicit_choice_and_its_own_credential() {
    let store = SpyStore::default();
    store.set(GROQ_API_KEY, &Secret::new(FAKE_KEY)).unwrap();
    store
        .set(OLLAMA_CLOUD_API_KEY, &Secret::new(CLOUD_KEY))
        .unwrap();
    store
        .set(
            "openai_compatible_api_key:api.anthropic.com",
            &Secret::new(COMPAT_KEY),
        )
        .unwrap();
    assert!(matches!(
        build_provider(&anthropic_choice(), &store, Duration::from_secs(1)),
        Err(AiError::Unauthorized)
    ));
    assert_eq!(
        *store.asked.lock().unwrap(),
        vec![ANTHROPIC_API_KEY.to_owned()]
    );
    store
        .set(ANTHROPIC_API_KEY, &Secret::new(ANTHROPIC_KEY))
        .unwrap();
    let provider = build_provider(&anthropic_choice(), &store, Duration::from_secs(1)).unwrap();
    assert_eq!(provider.kind(), ProviderKind::Anthropic);
    assert_eq!(provider.model(), "claude-opus-5-5");
    assert_eq!(
        nestadmin_ai::anthropic::ENDPOINT,
        "https://api.anthropic.com/v1/messages"
    );
}

#[test]
fn anthropic_sends_the_documented_headers_and_body() {
    let content =
        json!({ "category": "consulta_pago", "draft": "Revisaremos el estado del pago." });
    let (url, seen) = serve(vec![messages_ok(
        json!([
            { "type": "thinking", "thinking": "razonamiento interno {no es la salida}", "signature": "s" },
            { "type": "text", "text": content.to_string() },
        ]),
        "end_turn",
    )]);
    let provider = AnthropicProvider::with_endpoint(
        &format!("{url}/v1/messages"),
        "claude-opus-5-5",
        Secret::new(ANTHROPIC_KEY),
        Duration::from_secs(5),
    );
    let result = provider
        .generate(&email_request("E05", "¿Cuándo pagan?"))
        .unwrap();
    assert_eq!(result.provider, ProviderKind::Anthropic);
    assert_eq!(result.model, "claude-opus-5-5");
    assert_eq!(parse_email(&result.output).unwrap().0, "consulta_pago");
    assert_eq!(result.usage.input_tokens, Some(80));
    assert_eq!(result.usage.output_tokens, Some(25));

    let request = &seen.lock().unwrap()[0];
    assert!(request.starts_with("POST /v1/messages "), "{request}");
    let head = request.to_ascii_lowercase();
    assert!(head.contains(&format!(
        "x-api-key: {}",
        ANTHROPIC_KEY.to_ascii_lowercase()
    )));
    assert!(head.contains("anthropic-version: 2026-10-21"));
    assert!(head.contains("content-type: application/json"));
    assert!(!head.contains("authorization:"), "no se usa Bearer");
    let body = body_of(request);
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["max_tokens"], 16000);
    assert_eq!(body["output_config"], json!({ "effort": "low" }));
    for forbidden in [
        "temperature",
        "top_p",
        "top_k",
        "thinking",
        "fallbacks",
        "stream",
    ] {
        assert!(
            body.get(forbidden).is_none(),
            "no debe enviarse {forbidden}"
        );
    }
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "sin prefill de asistente");
    assert_eq!(messages[0]["role"], "user");
    assert!(body["system"].as_str().unwrap().contains("\"category\""));
}

#[test]
fn anthropic_concatenates_text_blocks_and_ignores_the_rest() {
    let (url, _) = serve(vec![messages_ok(
        json!([
            { "type": "thinking", "thinking": "{\"total\": 1}", "signature": "s" },
            { "type": "text", "text": "{\"category\": \"consulta_pago\"," },
            { "type": "text", "text": " \"draft\": \"Hola\"}" },
        ]),
        "end_turn",
    )]);
    let provider = AnthropicProvider::with_endpoint(
        &url,
        "claude-opus-5-5",
        Secret::new(ANTHROPIC_KEY),
        Duration::from_secs(5),
    );
    let result = provider.generate(&email_request("E01", "t")).unwrap();
    assert_eq!(result.output["draft"], "Hola");
}

#[test]
fn anthropic_refusal_and_truncation_are_explicit_errors_without_retry() {
    let (url, seen) = serve(vec![
        messages_ok(json!([]), "refusal"),
        messages_ok(
            json!([{ "type": "text", "text": "{\"category\": \"x\"" }]),
            "max_tokens",
        ),
        // Un rechazo con texto parcial tampoco se aprovecha.
        messages_ok(
            json!([{ "type": "text", "text": "{\"category\":\"a\",\"draft\":\"b\"}" }]),
            "refusal",
        ),
        // Sin ningún bloque de texto.
        messages_ok(
            json!([{ "type": "thinking", "thinking": "x", "signature": "s" }]),
            "end_turn",
        ),
    ]);
    let provider = AnthropicProvider::with_endpoint(
        &url,
        "claude-opus-5-5",
        Secret::new(ANTHROPIC_KEY),
        Duration::from_secs(5),
    );
    let request = email_request("E01", "t");
    let first = provider.generate(&request).unwrap_err();
    assert_eq!(first.code(), "invalid_response");
    assert!(first.to_string().contains("rechazo del modelo"), "{first}");
    assert_eq!(seen.lock().unwrap().len(), 1, "un rechazo no se reintenta");
    let second = provider.generate(&request).unwrap_err();
    assert!(second.to_string().contains("max_tokens"), "{second}");
    let third = provider.generate(&request).unwrap_err();
    assert!(third.to_string().contains("rechazo del modelo"), "{third}");
    assert_eq!(
        provider.generate(&request).unwrap_err().code(),
        "invalid_response"
    );
    assert_eq!(seen.lock().unwrap().len(), 4, "una petición por llamada");
}

#[test]
fn anthropic_http_errors_follow_the_contract_and_hide_the_key() {
    let timeout = Duration::from_millis(400);
    let error_body =
        |kind: &str| json!({ "type": "error", "error": { "type": kind, "message": "x" } });
    let (url, _) = serve(vec![
        reply(401, error_body("authentication_error")),
        reply(403, error_body("permission_error")),
        reply(429, error_body("rate_limit_error")),
        reply(500, error_body("api_error")),
        reply(529, error_body("overloaded_error")),
        reply(200, json!({ "content": "no es una lista" })),
        Reply {
            status: 200,
            body: "{}".into(),
            delay: Duration::from_millis(1500),
            headers: String::new(),
        },
    ]);
    let provider = AnthropicProvider::with_endpoint(&url, "m", Secret::new(ANTHROPIC_KEY), timeout);
    let request = extraction_request("x", "t");
    let mut printed = vec![format!("{provider:?}")];
    for expected in [
        "unauthorized",
        "unauthorized",
        "rate_limited",
        "unavailable",
        "unavailable",
        "invalid_response",
        "timeout",
    ] {
        let error = provider.generate(&request).unwrap_err();
        assert_eq!(error.code(), expected, "{error}");
        printed.push(format!("{error} {error:?}"));
    }
    assert_never_printed(ANTHROPIC_KEY, &printed);
    let down = AnthropicProvider::with_endpoint(
        &closed_port_url(),
        "m",
        Secret::new(ANTHROPIC_KEY),
        timeout,
    );
    assert!(matches!(
        down.generate(&request),
        Err(AiError::Unavailable(_))
    ));
}

#[test]
fn anthropic_failure_never_falls_back_to_another_provider_or_model() {
    // Local, Groq y compatible simulados y vivos, con todas las credenciales presentes.
    let (_local_url, local_seen) = serve(vec![show_local(), ollama_ok("{}")]);
    let (_groq_url, groq_seen) = serve(vec![chat_ok("{}")]);
    let (_compat_url, compat_seen) = serve(vec![chat_ok("{}")]);
    let store = SpyStore::default();
    for (name, key) in [
        (GROQ_API_KEY, FAKE_KEY),
        (OLLAMA_CLOUD_API_KEY, CLOUD_KEY),
        (ANTHROPIC_API_KEY, ANTHROPIC_KEY),
        ("openai_compatible_api_key:api.example.com", COMPAT_KEY),
    ] {
        store.set(name, &Secret::new(key)).unwrap();
    }
    // Claude rechaza y luego queda no disponible: el servicio devuelve cada error tal cual.
    let (claude_url, claude_seen) = serve(vec![
        messages_ok(json!([]), "refusal"),
        reply(529, json!({ "type": "error" })),
    ]);
    let key = store.get(ANTHROPIC_API_KEY).unwrap().unwrap();
    let service = AiService::new(Box::new(AnthropicProvider::with_endpoint(
        &claude_url,
        "claude-opus-5-5",
        key,
        Duration::from_secs(3),
    )));
    assert_eq!(
        service
            .generate(&email_request("E01", "t"))
            .unwrap_err()
            .code(),
        "invalid_response"
    );
    assert_eq!(
        service
            .generate(&email_request("E01", "t"))
            .unwrap_err()
            .code(),
        "unavailable"
    );
    assert_eq!(
        service.active(),
        (ProviderKind::Anthropic, "claude-opus-5-5")
    );
    thread::sleep(Duration::from_millis(100));
    assert_eq!(claude_seen.lock().unwrap().len(), 2);
    assert!(local_seen.lock().unwrap().is_empty());
    assert!(groq_seen.lock().unwrap().is_empty());
    assert!(compat_seen.lock().unwrap().is_empty());
    assert_eq!(
        *store.asked.lock().unwrap(),
        vec![ANTHROPIC_API_KEY.to_owned()],
        "con Claude elegido nunca se piden otras claves"
    );
}

#[test]
fn anthropic_4xx_reports_only_the_short_error_type() {
    const MARK: &str = "TEXTO_MARCADO_DEL_CONTENIDO_ENVIADO";
    let timeout = Duration::from_secs(2);
    let (url, _) = serve(vec![
        reply(
            400,
            json!({ "type": "error", "error": {
                "type": "invalid_request_error",
                "message": format!("effort no admitido: {MARK}") } }),
        ),
        reply(
            400,
            json!({ "error": { "type": "Tipo Raro <script>", "message": MARK } }),
        ),
        Reply {
            status: 400,
            body: format!("no es JSON {MARK}"),
            delay: Duration::ZERO,
            headers: String::new(),
        },
    ]);
    let provider = AnthropicProvider::with_endpoint(&url, "m", Secret::new(ANTHROPIC_KEY), timeout);
    let request = extraction_request("x", "t");
    let first = provider.generate(&request).unwrap_err();
    assert!(first.to_string().contains("400"), "{first}");
    assert!(
        first.to_string().contains("invalid_request_error"),
        "{first}"
    );
    let second = provider.generate(&request).unwrap_err();
    let third = provider.generate(&request).unwrap_err();
    assert!(!second.to_string().contains("Tipo Raro"), "{second}");
    for error in [first, second, third] {
        assert_eq!(error.code(), "unavailable");
        assert!(!format!("{error} {error:?}").contains(MARK), "{error}");
        assert!(!format!("{error:?}").contains(ANTHROPIC_KEY));
    }
}

#[test]
fn json_wrapped_in_one_code_fence_is_accepted_by_all_adapters() {
    let inner = json!({ "category": "consulta_pago", "draft": "Revisaremos el estado." });
    let pretty = serde_json::to_string_pretty(&inner).unwrap();
    for wrapped in [
        format!("```json\n{pretty}\n```"),
        format!("  \n```\n{pretty}\n```\n  "),
    ] {
        // Ollama cloud (API nativa de Ollama).
        let (url, _) = serve(vec![ollama_ok(&wrapped)]);
        let cloud = OllamaCloudProvider::with_endpoint(
            &format!("{url}/api/chat"),
            "m",
            Secret::new(CLOUD_KEY),
            Duration::from_secs(5),
        );
        let out = cloud.generate(&email_request("E05", "c")).unwrap();
        assert_eq!(out.output, inner);

        // Compatible con OpenAI.
        let (url, _) = serve(vec![chat_ok(&wrapped)]);
        let compat = OpenAiCompatibleProvider::with_endpoint(
            &url,
            "m",
            Secret::new(COMPAT_KEY),
            Duration::from_secs(5),
        );
        let out = compat.generate(&email_request("E05", "c")).unwrap();
        assert_eq!(out.output, inner);

        // Anthropic.
        let (url, _) = serve(vec![messages_ok(
            json!([{ "type": "text", "text": wrapped }]),
            "end_turn",
        )]);
        let claude = AnthropicProvider::with_endpoint(
            &format!("{url}/v1/messages"),
            "m",
            Secret::new(ANTHROPIC_KEY),
            Duration::from_secs(5),
        );
        let out = claude.generate(&email_request("E05", "c")).unwrap();
        assert_eq!(out.output, inner);
    }
}

#[test]
fn code_fence_normalization_removes_only_one_wrapping_block() {
    // Dos bloques, bloque sin cerrar, texto alrededor o cercas de cuatro acentos: no es un
    // envoltorio único, así que sigue siendo `invalid_response`.
    for bad in [
        "```json\n{\"a\":1}\n```\n```json\n{\"a\":2}\n```",
        "```json\n{\"a\":1}",
        "Aquí está:\n```json\n{\"a\":1}\n```",
        "````json\n{\"a\":1}\n````",
    ] {
        let (url, _) = serve(vec![chat_ok(bad)]);
        let compat = OpenAiCompatibleProvider::with_endpoint(
            &url,
            "m",
            Secret::new(COMPAT_KEY),
            Duration::from_secs(5),
        );
        assert_eq!(
            compat
                .generate(&extraction_request("x", "t"))
                .unwrap_err()
                .code(),
            "invalid_response",
            "{bad}"
        );
    }
}

fn groq_ok(content: Value) -> Reply {
    reply(
        200,
        json!({
            "model": "m",
            "choices": [{ "message": { "content": content.to_string() } }],
        }),
    )
}

fn rate_limited(retry_after: &str) -> Reply {
    let mut r = reply(429, json!({ "error": { "message": "rate limit" } }));
    if !retry_after.is_empty() {
        r.headers = format!("Retry-After: {retry_after}\r\n");
    }
    r
}

#[test]
fn rate_limit_waits_what_the_provider_asks_and_retries_the_same_provider() {
    let content = json!({ "category": "otros", "draft": "Gracias por su mensaje." });
    let (url, seen) = serve(vec![
        rate_limited("0.2"),
        rate_limited("0"),
        groq_ok(content),
    ]);
    let provider =
        GroqProvider::with_endpoint(&url, "m", Secret::new(FAKE_KEY), Duration::from_secs(5));
    let started = std::time::Instant::now();
    let result = provider.generate(&email_request("E01", "t")).unwrap();
    assert_eq!(result.provider, ProviderKind::Groq);
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[test]
fn rate_limit_beyond_the_deadline_is_reported_without_waiting() {
    let (url, seen) = serve(vec![rate_limited("60")]);
    let provider =
        GroqProvider::with_endpoint(&url, "m", Secret::new(FAKE_KEY), Duration::from_secs(5));
    let started = std::time::Instant::now();
    let error = provider.generate(&email_request("E01", "t")).unwrap_err();
    assert_eq!(
        error,
        AiError::RateLimited {
            retry_after_ms: Some(60_000)
        }
    );
    assert_eq!(error.code(), "rate_limited");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(!format!("{error}").contains(FAKE_KEY));
}

#[test]
fn other_invoices_are_read_strictly_and_requested_in_the_schema() {
    use nestadmin_ai::tasks::{parse_other_invoices, OTHER_INVOICES};
    let request = extraction_request("x", "t");
    let schema = request.schema.unwrap();
    assert!(schema["required"]
        .as_array()
        .unwrap()
        .contains(&json!(OTHER_INVOICES)));
    assert_eq!(
        schema["properties"][OTHER_INVOICES]["items"]["required"],
        json!([
            "supplier",
            "invoice_number",
            "date",
            "subtotal",
            "tax",
            "total"
        ])
    );

    let one = json!({ "supplier": "A", "invoice_number": "1", "date": null,
                      "subtotal": null, "tax": null, "total": "10.00" });
    assert!(
        parse_other_invoices(&one).unwrap().is_empty(),
        "sin la clave"
    );
    let empty = json!({ "supplier": null, "invoice_number": null, "date": null,
                        "subtotal": null, "tax": null, "total": null });
    let output = json!({ "other_invoices": [one.clone(), empty] });
    assert_eq!(parse_other_invoices(&output).unwrap(), vec![one.clone()]);
    assert!(matches!(
        parse_other_invoices(&json!({ "other_invoices": "dos" })),
        Err(AiError::InvalidResponse(_))
    ));
    assert!(matches!(
        parse_other_invoices(&json!({ "other_invoices": [{ "total": 10.0 }] })),
        Err(AiError::InvalidResponse(_))
    ));
}
