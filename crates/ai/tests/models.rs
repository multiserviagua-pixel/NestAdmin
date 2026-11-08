//! D122: lista de modelos de cada proveedor contra servidores simulados en 127.0.0.1 (datos
//! sintéticos, ninguna petición sale a la red): análisis, paginación, tope de 500, entradas
//! remotas del Ollama local, modelo recomendado primero, errores del contrato sin la clave,
//! sin redirecciones y sin leer ninguna clave con una URL compatible no válida.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nestadmin_ai::models::{list_models, list_models_with_endpoint, MAX_MODELS};
use nestadmin_ai::{AiError, ProviderChoice, Secret};
use serde_json::{json, Value};

const KEY: &str = "clave_sintetica_d122_no_real";
const TIMEOUT: Duration = Duration::from_secs(5);

fn reply(status: u16, body: Value) -> (u16, String) {
    (status, body.to_string())
}

/// Servidor que responde en orden y guarda la cabecera de cada petición.
fn serve(replies: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    thread::spawn(move || {
        for (status, body) in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            log.lock().unwrap().push(head);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, seen)
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn request_line(head: &str) -> &str {
    head.lines().next().unwrap()
}

fn cloud() -> ProviderChoice {
    ProviderChoice::OllamaCloud { model: "m".into() }
}

fn local() -> ProviderChoice {
    ProviderChoice::Ollama {
        base_url: "http://127.0.0.1:11434".into(),
        model: "qwen3:8b".into(),
    }
}

fn compat() -> ProviderChoice {
    ProviderChoice::OpenAiCompatible {
        base_url: "https://api.ejemplo.test/v1".into(),
        model: "m".into(),
    }
}

fn claude() -> ProviderChoice {
    ProviderChoice::Anthropic { model: "m".into() }
}

fn secret() -> Secret {
    Secret::new(KEY)
}

#[test]
fn ollama_cloud_lists_tags_with_the_bearer_and_puts_the_recommended_model_first() {
    let (url, seen) = serve(vec![reply(
        200,
        json!({ "models": [
            { "name": "gpt-oss:120b" },
            { "name": "gemma4:31b" },
            { "name": "gpt-oss:120b" },
            { "model": "solo-model:1b" },
            { "name": "  " },
        ]}),
    )]);
    let models =
        list_models_with_endpoint(&cloud(), &format!("{url}/api"), Some(&secret()), TIMEOUT)
            .unwrap();
    assert_eq!(models, ["gemma4:31b", "gpt-oss:120b", "solo-model:1b"]);
    let seen = seen.lock().unwrap();
    assert_eq!(request_line(&seen[0]), "GET /api/tags HTTP/1.1");
    assert_eq!(
        header(&seen[0], "authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
}

#[test]
fn local_ollama_lists_tags_without_a_key_and_skips_remote_entries() {
    let (url, seen) = serve(vec![reply(
        200,
        json!({ "models": [
            { "name": "llama3:8b" },
            { "name": "alias-nube", "remote_model": "gpt-oss:120b-cloud", "remote_host": "https://ollama.com:443" },
            { "name": "solo-host", "remote_host": "https://ollama.com:443" },
            { "name": "qwen3:8b", "remote_model": "", "remote_host": null },
        ]}),
    )]);
    let models = list_models_with_endpoint(&local(), &url, None, TIMEOUT).unwrap();
    assert_eq!(models, ["qwen3:8b", "llama3:8b"], "recomendado primero");
    let seen = seen.lock().unwrap();
    assert_eq!(request_line(&seen[0]), "GET /api/tags HTTP/1.1");
    assert!(header(&seen[0], "authorization").is_none());
}

#[test]
fn openai_compatible_lists_models_with_the_bearer_in_the_order_returned() {
    let (url, seen) = serve(vec![reply(
        200,
        json!({ "object": "list", "data": [
            { "id": "zeta" }, { "id": "alfa" }, { "id": "zeta" }, { "object": "x" }
        ]}),
    )]);
    let models =
        list_models_with_endpoint(&compat(), &format!("{url}/v1"), Some(&secret()), TIMEOUT)
            .unwrap();
    assert_eq!(models, ["zeta", "alfa"]);
    let seen = seen.lock().unwrap();
    assert_eq!(request_line(&seen[0]), "GET /v1/models HTTP/1.1");
    assert_eq!(
        header(&seen[0], "authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
}

#[test]
fn claude_follows_has_more_with_after_id_and_sends_its_own_headers() {
    let (url, seen) = serve(vec![
        reply(
            200,
            json!({ "data": [{ "id": "claude-a" }, { "id": "claude-b" }],
                    "has_more": true, "last_id": "claude-b" }),
        ),
        reply(
            200,
            json!({ "data": [{ "id": "claude-c" }], "has_more": false, "last_id": "claude-c" }),
        ),
    ]);
    let models = list_models_with_endpoint(
        &claude(),
        &format!("{url}/v1/models"),
        Some(&secret()),
        TIMEOUT,
    )
    .unwrap();
    assert_eq!(models, ["claude-a", "claude-b", "claude-c"]);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(request_line(&seen[0]).starts_with("GET /v1/models?limit="));
    assert!(!request_line(&seen[0]).contains("after_id"));
    assert!(request_line(&seen[1]).contains("after_id=claude-b"));
    for head in seen.iter() {
        assert_eq!(header(head, "x-api-key"), Some(KEY));
        assert_eq!(
            header(head, "anthropic-version"),
            Some(nestadmin_ai::anthropic::API_VERSION)
        );
        assert!(header(head, "authorization").is_none());
    }
}

#[test]
fn claude_rejects_a_pagination_that_cannot_advance_or_has_a_hostile_id() {
    let (url, _) = serve(vec![reply(
        200,
        json!({ "data": [{ "id": "a" }], "has_more": true, "last_id": "a&x=1" }),
    )]);
    let error = list_models_with_endpoint(
        &claude(),
        &format!("{url}/v1/models"),
        Some(&secret()),
        TIMEOUT,
    )
    .unwrap_err();
    assert!(matches!(error, AiError::InvalidResponse(_)), "{error:?}");

    let page = reply(
        200,
        json!({ "data": [{ "id": "a" }], "has_more": true, "last_id": "a" }),
    );
    let (url, _) = serve(vec![page.clone(), page]);
    let error = list_models_with_endpoint(
        &claude(),
        &format!("{url}/v1/models"),
        Some(&secret()),
        TIMEOUT,
    )
    .unwrap_err();
    assert!(matches!(error, AiError::InvalidResponse(_)), "{error:?}");
}

#[test]
fn the_list_is_capped_at_500_names_keeping_the_recommended_model_first() {
    let mut models: Vec<Value> = (0..700)
        .map(|i| json!({ "name": format!("m{i}") }))
        .collect();
    models.push(json!({ "name": "gemma4:31b" }));
    let (url, _) = serve(vec![reply(200, json!({ "models": models }))]);
    let listed =
        list_models_with_endpoint(&cloud(), &format!("{url}/api"), Some(&secret()), TIMEOUT)
            .unwrap();
    assert_eq!(listed.len(), MAX_MODELS);
    assert_eq!(listed[0], "gemma4:31b");
    assert_eq!(listed[1], "m0");

    let data: Vec<Value> = (0..900).map(|i| json!({ "id": format!("c{i}") })).collect();
    let (url, _) = serve(vec![reply(200, json!({ "data": data }))]);
    let listed =
        list_models_with_endpoint(&compat(), &format!("{url}/v1"), Some(&secret()), TIMEOUT)
            .unwrap();
    assert_eq!(listed.len(), MAX_MODELS);
}

#[test]
fn errors_follow_the_contract_and_never_show_the_key() {
    for (status, expected) in [(401, AiError::Unauthorized), (403, AiError::Unauthorized)] {
        for choice in [cloud(), compat(), claude()] {
            let (url, _) = serve(vec![reply(
                status,
                json!({ "error": { "message": format!("clave {KEY} rechazada") } }),
            )]);
            let error =
                list_models_with_endpoint(&choice, &url, Some(&secret()), TIMEOUT).unwrap_err();
            assert_eq!(error, expected, "{choice:?}");
            assert!(!format!("{error:?} {error}").contains(KEY));
        }
    }
    let (url, _) = serve(vec![reply(500, json!({}))]);
    let error = list_models_with_endpoint(&compat(), &url, Some(&secret()), TIMEOUT).unwrap_err();
    assert!(matches!(error, AiError::Unavailable(_)), "{error:?}");
    let (url, _) = serve(vec![reply(429, json!({}))]);
    let error = list_models_with_endpoint(&compat(), &url, Some(&secret()), TIMEOUT).unwrap_err();
    assert!(matches!(error, AiError::RateLimited { .. }), "{error:?}");
    let (url, _) = serve(vec![reply(200, json!({ "otra": [] }))]);
    let error = list_models_with_endpoint(&claude(), &url, Some(&secret()), TIMEOUT).unwrap_err();
    assert!(matches!(error, AiError::InvalidResponse(_)), "{error:?}");
    // Sin servidor: el error tampoco cita la clave.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let error =
        list_models_with_endpoint(&compat(), &closed, Some(&secret()), TIMEOUT).unwrap_err();
    assert!(matches!(error, AiError::Unavailable(_)), "{error:?}");
    assert!(!format!("{error:?}").contains(KEY));
}

#[test]
fn a_redirect_is_not_followed_and_the_key_never_reaches_the_target() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let location = format!("http://{}/robada", target.local_addr().unwrap());
    let redirector = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", redirector.local_addr().unwrap());
    thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = redirector.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 307 X\r\nLocation: {location}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            );
        }
    });
    for choice in [cloud(), compat(), claude()] {
        let endpoint = if matches!(choice, ProviderChoice::OllamaCloud { .. }) {
            format!("{url}/api")
        } else {
            url.clone()
        };
        let error =
            list_models_with_endpoint(&choice, &endpoint, Some(&secret()), TIMEOUT).unwrap_err();
        assert!(
            matches!(error, AiError::Unavailable(_)),
            "{choice:?}: {error:?}"
        );
        assert!(!format!("{error:?}").contains(KEY));
    }
    thread::sleep(Duration::from_millis(150));
    assert!(
        target.accept().is_err(),
        "nadie se conectó al destino de la redirección"
    );
}

#[test]
fn an_invalid_compatible_url_fails_before_any_request_or_key_is_used() {
    for base_url in [
        "https://localhost/v1",
        "http://api.ejemplo.test/v1",
        "https://127.0.0.1/v1",
        "",
        "https://usuario:clave@api.ejemplo.test/v1",
    ] {
        let choice = ProviderChoice::OpenAiCompatible {
            base_url: base_url.into(),
            model: "m".into(),
        };
        let error = list_models(&choice, Some(&secret()), TIMEOUT).unwrap_err();
        assert!(
            matches!(error, AiError::Unavailable(_)),
            "{base_url}: {error:?}"
        );
        assert!(!format!("{error:?}").contains(KEY));
    }
}

#[test]
fn an_external_provider_without_a_key_is_unauthorized_and_groq_and_remote_local_are_refused() {
    for choice in [cloud(), compat(), claude()] {
        assert_eq!(
            list_models(&choice, None, TIMEOUT),
            Err(AiError::Unauthorized),
            "{choice:?}"
        );
    }
    assert!(list_models(
        &ProviderChoice::Groq { model: "m".into() },
        Some(&secret()),
        TIMEOUT
    )
    .is_err());
    let remote = ProviderChoice::Ollama {
        base_url: "http://192.0.2.1:11434".into(),
        model: "m".into(),
    };
    assert!(matches!(
        list_models(&remote, None, TIMEOUT),
        Err(AiError::Unavailable(_))
    ));
}
