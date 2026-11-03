//! T03 a nivel de biblioteca: selección local por defecto, ausencia de respaldo, errores
//! del contrato y credenciales que no se filtran. Los proveedores hablan con un servidor
//! HTTP simulado en 127.0.0.1; ninguna prueba sale a la red.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nestadmin_ai::credentials::GROQ_API_KEY;
use nestadmin_ai::groq::GroqProvider;
use nestadmin_ai::ollama::{is_loopback_http, OllamaProvider};
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
}

fn reply(status: u16, body: Value) -> Reply {
    Reply {
        status,
        body: body.to_string(),
        delay: Duration::ZERO,
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
                "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.status,
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
    let (url, seen) = serve(vec![ollama_ok(&output.to_string())]);
    let provider = OllamaProvider::new(&url, "qwen3:8b", Duration::from_secs(5)).unwrap();
    let result = provider
        .generate(&extraction_request("D01", "texto sintético"))
        .unwrap();

    assert_eq!(result.provider, ProviderKind::Ollama);
    assert_eq!(result.model, "qwen3:8b");
    assert_eq!(result.request_id, "D01");
    assert_eq!(result.output, output);
    assert_eq!(result.usage.input_tokens, Some(120));
    let request = &seen.lock().unwrap()[0];
    assert!(request.starts_with("POST /api/chat "));
    let body = body_of(request);
    assert_eq!(body["stream"], false);
    assert_eq!(body["think"], false);
    assert_eq!(body["options"]["temperature"], 0);
    assert_eq!(body["format"]["required"].as_array().unwrap().len(), 6);
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
        reply(500, json!({ "error": "fallo" })),
        ollama_ok("esto no es JSON"),
        ollama_ok("[1, 2]"),
        Reply {
            status: 200,
            body: "{}".into(),
            delay: Duration::from_millis(1500),
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
