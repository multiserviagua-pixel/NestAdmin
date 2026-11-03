//! Genera predicciones JSONL para `evaluation/evaluate.py` con el proveedor elegido.
//!
//! Lee solo `evaluation/data/inputs/`: las expectativas nunca llegan al modelo. Las facturas
//! se procesan en orden con un registro de duplicados vacío, como exige el protocolo. Groq
//! requiere `--provider groq` y una credencial guardada; un fallo nunca cambia de proveedor.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nestadmin_ai::tasks::{email_request, extraction_request, parse_email, parse_extraction};
use nestadmin_ai::{
    build_provider, AiError, CredentialStore, MemoryCredentialStore, ProviderChoice,
};
use nestadmin_documents::{process_invoice, DuplicateRegistry, Invoice};
use serde_json::{json, Value};

const USAGE: &str = "uso: nestadmin-predict --provider ollama|groq --out ARCHIVO.jsonl \
[--model MODELO] [--url URL_OLLAMA] [--inputs DIR] [--timeout-secs N]";

struct Args {
    choice: ProviderChoice,
    out: PathBuf,
    inputs: PathBuf,
    timeout: Duration,
}

fn parse_args() -> Result<Args, String> {
    let mut provider = None;
    let mut model = None;
    let mut url = None;
    let mut out = None;
    let mut inputs = PathBuf::from("evaluation/data/inputs");
    let mut timeout = 300;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("falta el valor de {flag}"))
        };
        match flag.as_str() {
            "--provider" => provider = Some(value()?),
            "--model" => model = Some(value()?),
            "--url" => url = Some(value()?),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--inputs" => inputs = PathBuf::from(value()?),
            "--timeout-secs" => timeout = value()?.parse().map_err(|_| "timeout inválido")?,
            other => return Err(format!("argumento desconocido: {other}")),
        }
    }
    if inputs.components().any(|c| c.as_os_str() == "expected") {
        return Err("las expectativas no pueden usarse como entrada".into());
    }
    let choice = match provider.as_deref() {
        Some("ollama") => {
            let ProviderChoice::Ollama {
                base_url,
                model: default,
            } = ProviderChoice::default()
            else {
                unreachable!()
            };
            ProviderChoice::Ollama {
                base_url: url.unwrap_or(base_url),
                model: model.unwrap_or(default),
            }
        }
        Some("groq") => ProviderChoice::Groq {
            model: model.ok_or("--model es obligatorio con groq")?,
        },
        _ => {
            return Err(
                "--provider debe ser ollama o groq (sin valor por defecto implícito)".into(),
            )
        }
    };
    Ok(Args {
        choice,
        out: out.ok_or("--out es obligatorio")?,
        inputs,
        timeout: Duration::from_secs(timeout),
    })
}

fn credentials() -> Result<Box<dyn CredentialStore>, String> {
    #[cfg(windows)]
    {
        nestadmin_ai::credentials::WindowsCredentialStore::new()
            .map(|s| Box::new(s) as Box<dyn CredentialStore>)
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Ok(Box::new(MemoryCredentialStore::default()))
    }
}

fn load_inputs(dir: &Path, name: &str) -> Result<Vec<(String, String)>, String> {
    let text = fs::read_to_string(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
    let cases: Vec<Value> = serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
    cases
        .iter()
        .map(|c| {
            let id = c["id"].as_str().ok_or(format!("{name}: caso sin id"))?;
            let input = &c["input"];
            // Las facturas traen `text`; los correos, `subject` y `body`.
            let text = match (input["text"].as_str(), input["body"].as_str()) {
                (Some(text), _) => text.to_owned(),
                (None, Some(body)) => match input["subject"].as_str() {
                    Some(subject) => format!("Asunto: {subject}\n\n{body}"),
                    None => body.to_owned(),
                },
                (None, None) => return Err(format!("{name}: {id} sin texto")),
            };
            Ok((id.to_owned(), text))
        })
        .collect()
}

fn error_line(id: &str, error: &AiError, latency_ms: u128) -> Value {
    json!({ "id": id, "error": error.code(), "latency_ms": latency_ms as u64 })
}

fn main() {
    if let Err(message) = run() {
        eprintln!("Error: {message}\n{USAGE}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let invoices = load_inputs(&args.inputs, "invoices.json")?;
    let emails = load_inputs(&args.inputs, "emails.json")?;
    // El almacén del sistema solo se abre si se eligió el proveedor externo.
    let store: Box<dyn CredentialStore> = match args.choice {
        ProviderChoice::Groq { .. } => credentials()?,
        ProviderChoice::Ollama { .. } => Box::new(MemoryCredentialStore::default()),
    };
    let provider =
        build_provider(&args.choice, store.as_ref(), args.timeout).map_err(|e| e.to_string())?;
    let mut out =
        fs::File::create(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let started_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let started = Instant::now();
    let mut registry = DuplicateRegistry::new();
    let mut errors = 0;
    let mut models = std::collections::BTreeSet::new();

    for (id, text) in &invoices {
        let t0 = Instant::now();
        let line = match provider
            .generate(&extraction_request(id, text))
            .and_then(|r| parse_extraction(&r.output).map(|raw| (r, raw)))
        {
            Ok((result, raw)) => {
                models.insert(result.model.clone());
                let invoice = Invoice::from_raw(&raw);
                let report = process_invoice(&mut registry, id.clone(), &invoice);
                // Un valor que no se pudo interpretar se informa tal como lo devolvió el modelo.
                let fields: serde_json::Map<String, Value> = invoice
                    .contract_fields()
                    .into_iter()
                    .map(|(name, value)| {
                        let literal = match name {
                            "supplier" => &raw.supplier,
                            "invoice_number" => &raw.invoice_number,
                            "date" => &raw.date,
                            "subtotal" => &raw.subtotal,
                            "tax" => &raw.tax,
                            _ => &raw.total,
                        };
                        let value = value.or_else(|| {
                            invoice
                                .is_unreadable(name)
                                .then(|| literal.as_ref().map(|e| e.value.clone()))
                                .flatten()
                        });
                        (name.to_owned(), value.map_or(Value::Null, Value::String))
                    })
                    .collect();
                let issues: Vec<&str> = report.issues.iter().map(|i| i.as_str()).collect();
                json!({ "id": id, "result": { "fields": fields, "issues": issues }, "latency_ms": result.latency_ms })
            }
            Err(error) => {
                errors += 1;
                error_line(id, &error, t0.elapsed().as_millis())
            }
        };
        writeln!(out, "{line}").map_err(|e| e.to_string())?;
        eprintln!(
            "{id}: {}",
            if line.get("error").is_some() {
                "error"
            } else {
                "ok"
            }
        );
    }

    for (id, text) in &emails {
        let t0 = Instant::now();
        let line = match provider
            .generate(&email_request(id, text))
            .and_then(|r| parse_email(&r.output).map(|parsed| (r, parsed)))
        {
            Ok((result, (category, draft))) => {
                models.insert(result.model.clone());
                json!({ "id": id, "result": { "category": category, "draft": draft }, "latency_ms": result.latency_ms })
            }
            Err(error) => {
                errors += 1;
                error_line(id, &error, t0.elapsed().as_millis())
            }
        };
        writeln!(out, "{line}").map_err(|e| e.to_string())?;
        eprintln!(
            "{id}: {}",
            if line.get("error").is_some() {
                "error"
            } else {
                "ok"
            }
        );
    }

    // Metadatos de la ejecución junto a las predicciones; la memoria no se mide aquí.
    let meta = json!({
        "corpus": "synthetic-v1",
        "choice": args.choice,
        "models_reported": models,
        "cases": invoices.len() + emails.len(),
        "errors": errors,
        "started_unix": started_unix,
        "duration_ms": started.elapsed().as_millis() as u64,
        "timeout_secs": args.timeout.as_secs(),
        "concurrency": 1,
        "memory": "no medida",
    });
    let meta_path = args.out.with_extension("meta.json");
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta).unwrap() + "\n",
    )
    .map_err(|e| e.to_string())?;
    eprintln!(
        "Predicciones: {}\nMetadatos: {}",
        args.out.display(),
        meta_path.display()
    );
    Ok(())
}
