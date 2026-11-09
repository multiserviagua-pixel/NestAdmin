//! Perfil del negocio (E6, D163) con un proveedor espía y datos sintéticos. Se comprueba sobre las
//! peticiones reales qué sale y adónde: el perfil solo va en la redacción, nunca en la clasificación
//! ni en la extracción, solo con consentimiento vigente hacia un proveedor externo (y sin él hacia la
//! IA local), cae al cambiar de destino o de contenido, y el resultado guarda solo su huella.

use std::path::Path;
use std::sync::{Arc, Mutex};

use nestadmin_ai::tasks::draft_request;
use nestadmin_ai::{
    AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task as AiTask, Usage,
};
use nestadmin_app::{App, AppPaths, EMAIL};
use nestadmin_core::{BusinessProfile, Store, Task, TaskState};
use nestadmin_extract::{ExtractError, Extractor, Ocr};
use nestadmin_simulators::Message;
use serde_json::{json, Value};

const SENTINEL: &str = "SENTINELA-PERFIL";
const MAIL: &str = "SENTINELA-M1";

// ---- Proveedor espía ----

/// Respuestas según marcas del correo: `[[CAIDA-REDACCION]]` hace fallar la redacción del externo
/// como si no hubiera red, `[[CAIDA]]` hace fallar también la clasificación, `[[SIN-RESPUESTA]]`
/// clasifica con `needs_reply = false`, `[[CONTACTOS-PERFIL]]` devuelve un borrador con los
/// contactos del perfil y `[[CONTACTO-AJENO]]` uno con un contacto que no es del perfil.
struct Spy {
    kind: ProviderKind,
    model: String,
    host: Option<String>,
    requests: Mutex<Vec<GenerateRequest>>,
}

impl Spy {
    fn new(kind: ProviderKind, model: &str) -> Arc<Self> {
        Arc::new(Self {
            kind,
            model: model.to_owned(),
            host: (kind != ProviderKind::Ollama).then(|| "ollama.test".to_owned()),
            requests: Mutex::new(Vec::new()),
        })
    }
    fn external() -> Arc<Self> {
        Self::new(ProviderKind::OllamaCloud, "modelo-a")
    }
    fn local() -> Arc<Self> {
        Self::new(ProviderKind::Ollama, "local")
    }
    /// Peticiones de esa tarea cuyo contenido lleva la marca de un correo.
    fn of(&self, task: AiTask, mail: &str) -> Vec<GenerateRequest> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.task == task && r.content.contains(mail))
            .cloned()
            .collect()
    }
    /// Cuántas peticiones (de cualquier tarea) sobre ese correo llevan el perfil. Los correos de
    /// ejemplo del simulador también se procesan y no cuentan.
    fn with_profile(&self, mail: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.content.contains(mail))
            .filter(|r| r.instructions.contains(SENTINEL) || r.content.contains(SENTINEL))
            .count()
    }
}

impl Provider for Spy {
    fn kind(&self) -> ProviderKind {
        self.kind
    }
    fn model(&self) -> &str {
        &self.model
    }
    fn destination(&self) -> Option<String> {
        self.host.clone()
    }
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.requests.lock().unwrap().push(request.clone());
        let c = &request.content;
        if self.kind == ProviderKind::Ollama && c.contains("[[LOCAL-CAIDA]]") {
            return Err(AiError::Unavailable("la IA local no responde".into()));
        }
        if self.kind != ProviderKind::Ollama
            && (c.contains("[[CAIDA]]")
                || (request.task == AiTask::Draft && c.contains("[[CAIDA-REDACCION]]")))
        {
            return Err(AiError::Unavailable("sin conexión".into()));
        }
        let output = match request.task {
            AiTask::Extraction => json!({
                "supplier": "Proveedor Alfa Suministros SL", "invoice_number": "FS-2026-014",
                "date": "2026-10-01", "subtotal": "120.00", "tax": "25.20", "total": "145.20",
            }),
            AiTask::Classification if c.contains("[[SIN-RESPUESTA]]") => {
                json!({ "category": "solicitud_factura", "needs_reply": false })
            }
            AiTask::Classification => {
                json!({ "category": "solicitud_factura", "needs_reply": true })
            }
            AiTask::Draft if c.contains("[[CONTACTOS-PERFIL]]") => json!({
                "draft": "Llámenos al 600 111 222 o escriba a hola@taller-ficticio.test; también en https://taller-ficticio.test."
            }),
            AiTask::Draft if c.contains("[[CONTACTO-AJENO]]") => json!({
                "draft": "Escriba a pagos@banco-falso.test o llame al 611 222 333."
            }),
            AiTask::Draft => json!({ "draft": "Adjuntamos la factura solicitada." }),
            _ => json!({}),
        };
        Ok(GenerateResult {
            request_id: request.request_id.clone(),
            provider: self.kind,
            model: self.model.clone(),
            output,
            latency_ms: 1,
            usage: Usage::default(),
        })
    }
}

struct NoOcr;

impl Ocr for NoOcr {
    fn recognize(&self, _image: &Path, page: usize) -> Result<String, ExtractError> {
        Err(ExtractError::OcrUnavailable(format!("página {page}")))
    }
}

// ---- Entorno ----

fn msg(id: &str, subject: &str, body: &str) -> Message {
    Message {
        id: id.into(),
        subject: Some(subject.into()),
        from: Some("cliente@clienteficticio.test".into()),
        to: Some(vec!["administracion@nestadmin.test".into()]),
        body: Some(body.into()),
        date: Some("2026-10-01".into()),
        attachments: Vec::new(),
        signals: None,
        thread_key: None,
    }
    .with_checked_signals(&[])
}

fn seed(dir: &Path, messages: Vec<Message>) {
    let state = json!({
        "version": 1,
        "state": { "messages": messages, "drafts": [], "sent": [], "files": [] },
    });
    std::fs::write(dir.join("simulador-correo.json"), state.to_string()).unwrap();
}

fn open(dir: &Path, provider: Arc<dyn Provider>) -> App {
    let extractor = Extractor::new(dir.join("sin-pdfium"), Box::new(NoOcr));
    let mut app = App::open(&AppPaths::new(dir), provider, Arc::new(extractor)).unwrap();
    app.set_mail_limit(500).unwrap();
    app
}

fn with_fallback(app: &mut App, local: &Arc<Spy>) {
    let provider = local.clone();
    app.set_local_fallback(Some(Arc::new(move || {
        Some(provider.clone() as Arc<dyn Provider>)
    })));
}

fn process_all(app: &mut App) {
    while app.process_next().unwrap().is_some() {}
}

fn input_of(task: &Task) -> Value {
    serde_json::from_str(&task.input_ref).unwrap()
}

fn mail_task(app: &App, id: &str) -> Task {
    let wanted = format!("simulador:{id}");
    let found: Vec<Task> = app
        .tasks()
        .unwrap()
        .into_iter()
        .filter(|t| {
            t.kind == EMAIL
                && input_of(t)["message_id"] == wanted
                && input_of(t).get("redraft_of").is_none()
        })
        .collect();
    assert_eq!(found.len(), 1, "una sola tarea original de {id}");
    found.into_iter().next().unwrap()
}

fn result_of(task: &Task) -> Value {
    task.result.clone().unwrap()
}

fn profile() -> BusinessProfile {
    BusinessProfile {
        display_name: "Taller Ficticio SL".into(),
        hours: "Lunes a viernes de 9:00 a 14:00".into(),
        tone: "Cercano y breve".into(),
        signature: "Taller Ficticio SL - hola@taller-ficticio.test - 600 111 222".into(),
        policies: format!(
            "Devoluciones en 14 días ({SENTINEL}). Web: https://taller-ficticio.test"
        ),
    }
}

/// Consiente a lo que muestra la vista previa de ahora.
fn grant(app: &mut App) {
    let preview = app.profile_preview().unwrap();
    let consent = preview.consent;
    app.grant_profile_consent(
        consent.provider.as_deref().unwrap(),
        consent.model.as_deref().unwrap(),
        consent.host.as_deref().unwrap_or(""),
        preview.fp.as_deref().unwrap(),
    )
    .unwrap();
}

fn count(dir: &Path, query: &str) -> i64 {
    rusqlite::Connection::open(dir.join("nestadmin.db"))
        .unwrap()
        .query_row(query, [], |r| r.get(0))
        .unwrap()
}

/// Una instalación con un correo `M1` y el perfil de prueba guardado.
fn setup(provider: Arc<dyn Provider>, subject: &str) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), vec![msg("M1", subject, "Hola SENTINELA-M1")]);
    let mut app = open(dir.path(), provider);
    app.save_business_profile(profile()).unwrap();
    app.fetch_mail().unwrap();
    (dir, app)
}

// ---- Qué llamadas llevan el perfil ----

#[test]
fn the_profile_goes_only_in_the_draft_call_never_in_classification_or_extraction() {
    let spy = Spy::external();
    let (dir, mut app) = setup(spy.clone(), "Consulta");
    grant(&mut app);
    app.import_document(
        "factura.txt",
        "Factura FS-2026-014 de Proveedor Alfa Suministros SL, total 145,20 EUR",
    )
    .unwrap();
    process_all(&mut app);

    let classification = spy.of(AiTask::Classification, "SENTINELA-M1");
    let draft = spy.of(AiTask::Draft, "SENTINELA-M1");
    assert_eq!((classification.len(), draft.len()), (1, 1));
    for request in spy.requests.lock().unwrap().iter() {
        let carries = request.instructions.contains(SENTINEL) || request.content.contains(SENTINEL);
        assert_eq!(
            carries,
            request.task == AiTask::Draft,
            "solo la redacción lleva el perfil: {:?}",
            request.task
        );
    }
    assert!(draft[0].instructions.contains("Taller Ficticio SL"));
    assert!(!classification[0].instructions.contains("Taller Ficticio"));
    assert_eq!(mail_task(&app, "M1").state, TaskState::AwaitingReview);
    // Una factura se extrae igual y sin perfil.
    assert!(!spy.of(AiTask::Extraction, "FS-2026-014").is_empty());
    drop(dir);
}

#[test]
fn without_consent_an_external_provider_never_gets_the_profile_but_drafting_still_works() {
    let spy = Spy::external();
    let (dir, mut app) = setup(spy.clone(), "Consulta");
    assert_eq!(app.profile_consent().unwrap().state, "missing");
    process_all(&mut app);
    assert_eq!(spy.with_profile(MAIL), 0, "ni una petición lleva el perfil");
    let task = mail_task(&app, "M1");
    assert_eq!(task.state, TaskState::AwaitingReview);
    let result = result_of(&task);
    assert_eq!(result["draft"]["body"], "Adjuntamos la factura solicitada.");
    assert!(
        result.get("profile_fp").is_none(),
        "sin perfil enviado no hay huella"
    );
    drop(dir);
}

#[test]
fn consent_is_tied_to_the_destination_and_a_change_drops_it_for_good() {
    let a = Spy::external();
    let (dir, mut app) = setup(a.clone(), "Consulta");
    grant(&mut app);
    assert_eq!(app.profile_consent().unwrap().state, "granted");

    // Otro modelo es otro destino: el consentimiento cae y no redacta con perfil.
    let b = Spy::new(ProviderKind::OllamaCloud, "modelo-b");
    app.select_provider(b.clone());
    assert_eq!(app.profile_consent().unwrap().state, "missing");
    process_all(&mut app);
    assert_eq!(b.with_profile(MAIL), 0);
    let audit = app.audit_log().unwrap();
    assert!(audit.iter().any(|e| e.detail.as_deref().is_some_and(
        |d| d.contains("consentimiento del perfil del negocio retirado: cambió el proveedor")
    )));

    // Volver al destino anterior no lo revive.
    app.select_provider(Spy::external());
    assert_eq!(app.profile_consent().unwrap().state, "missing");
    assert!(!app.profile_preview().unwrap().will_send);
    drop(dir);
}

#[test]
fn a_consent_granted_to_an_old_destination_cannot_be_granted_to_a_stale_preview() {
    let spy = Spy::external();
    let (dir, mut app) = setup(spy, "Consulta");
    let preview = app.profile_preview().unwrap();
    let fp = preview.fp.clone().unwrap();
    // Modelo distinto del que se vio, servidor distinto y huella distinta se rechazan.
    for (provider, model, host, fp) in [
        ("ollama_cloud", "modelo-z", "ollama.test", fp.as_str()),
        ("ollama_cloud", "modelo-a", "otro.test", fp.as_str()),
        (
            "ollama_cloud",
            "modelo-a",
            "ollama.test",
            "0000000000000000",
        ),
    ] {
        assert!(
            app.grant_profile_consent(provider, model, host, fp)
                .is_err(),
            "{model} {host} {fp}"
        );
    }
    assert_eq!(app.profile_consent().unwrap().state, "missing");
    drop(dir);
}

#[test]
fn changing_the_profile_so_that_what_leaves_changes_drops_the_consent() {
    let spy = Spy::external();
    let (dir, mut app) = setup(spy.clone(), "Consulta");
    grant(&mut app);
    // Guardar lo mismo no cambia lo que sale.
    let same = app.save_business_profile(profile()).unwrap();
    assert!(!same.consent_withdrawn);
    assert_eq!(app.profile_consent().unwrap().state, "granted");
    // Cambiar el horario sí: cae, y la redacción siguiente sale sin perfil.
    let mut changed = profile();
    changed.hours = "Lunes a jueves de 9:00 a 13:00".into();
    let saved = app.save_business_profile(changed).unwrap();
    assert!(saved.consent_withdrawn);
    assert_eq!(app.profile_consent().unwrap().state, "missing");
    process_all(&mut app);
    assert_eq!(spy.with_profile(MAIL), 0);
    // Borrarlo también lo retira (si estuviera concedido).
    grant_after_resave(&mut app);
    app.clear_business_profile().unwrap();
    assert_eq!(app.profile_consent().unwrap().state, "missing");
    assert!(!app.business_profile().unwrap().saved);
    drop(dir);
}

fn grant_after_resave(app: &mut App) {
    app.save_business_profile(profile()).unwrap();
    grant(app);
    assert_eq!(app.profile_consent().unwrap().state, "granted");
}

#[test]
fn a_local_provider_uses_the_profile_without_any_consent_and_the_fingerprint_is_stored() {
    let local = Spy::local();
    let (dir, mut app) = setup(local.clone(), "Consulta");
    let consent = app.profile_consent().unwrap();
    assert!(!consent.required);
    assert_eq!(consent.state, "not_needed");
    assert!(
        app.grant_profile_consent("ollama", "local", "", "x")
            .is_err(),
        "con la IA local no hay nada que consentir"
    );
    process_all(&mut app);
    let draft = local.of(AiTask::Draft, "SENTINELA-M1");
    assert_eq!(draft.len(), 1);
    assert!(draft[0].instructions.contains(SENTINEL));
    assert!(local
        .of(AiTask::Classification, "SENTINELA-M1")
        .iter()
        .all(|r| !r.instructions.contains(SENTINEL)));
    assert!(result_of(&mail_task(&app, "M1"))["profile_fp"].is_string());
    drop(dir);
}

#[test]
fn the_local_fallback_gets_the_profile_without_consent_and_the_external_does_not() {
    let external = Spy::external();
    let local = Spy::local();
    // La redacción del externo cae: sin consentimiento, el externo no ve el perfil y el respaldo
    // local sí.
    let (dir, mut app) = setup(external.clone(), "Consulta [[CAIDA-REDACCION]]");
    with_fallback(&mut app, &local);
    process_all(&mut app);
    assert_eq!(external.with_profile(MAIL), 0);
    assert_eq!(local.of(AiTask::Draft, "SENTINELA-M1").len(), 1);
    assert!(local.of(AiTask::Draft, "SENTINELA-M1")[0]
        .instructions
        .contains(SENTINEL));
    let result = result_of(&mail_task(&app, "M1"));
    assert_eq!(result["fallback"]["reason"], "unavailable");
    assert_eq!(result["provider"], "ollama");
    let preview = app.profile_preview().unwrap();
    assert_eq!(result["profile_fp"], json!(preview.fp.unwrap()));
    // La clasificación nunca llevó el perfil, en ninguno de los dos.
    for spy in [&external, &local] {
        assert!(spy
            .of(AiTask::Classification, "SENTINELA-M1")
            .iter()
            .all(|r| !r.instructions.contains(SENTINEL)));
    }
    drop(dir);

    // Si la clasificación ya usó el respaldo local, la redacción se queda en local con el perfil.
    let external = Spy::external();
    let local = Spy::local();
    let (dir, mut app) = setup(external.clone(), "Consulta [[CAIDA]]");
    with_fallback(&mut app, &local);
    process_all(&mut app);
    assert_eq!(external.with_profile(MAIL), 0);
    assert_eq!(local.of(AiTask::Draft, "SENTINELA-M1").len(), 1);
    assert!(local.of(AiTask::Draft, "SENTINELA-M1")[0]
        .instructions
        .contains(SENTINEL));
    drop(dir);
}

#[test]
fn with_consent_a_failed_external_draft_that_falls_back_locally_reports_the_same_fingerprint() {
    let external = Spy::external();
    let local = Spy::local();
    let (dir, mut app) = setup(external.clone(), "Consulta [[CAIDA-REDACCION]]");
    with_fallback(&mut app, &local);
    grant(&mut app);
    process_all(&mut app);
    // El externo recibió el perfil (hay consentimiento), falló, y el local lo recibió después.
    assert_eq!(external.with_profile(MAIL), 1);
    assert_eq!(local.with_profile(MAIL), 1);
    assert!(result_of(&mail_task(&app, "M1"))["profile_fp"].is_string());
    drop(dir);
}

#[test]
fn redrafting_carries_the_profile_too_and_only_with_consent() {
    let spy = Spy::external();
    let (dir, mut app) = setup(spy.clone(), "Gracias [[SIN-RESPUESTA]]");
    grant(&mut app);
    process_all(&mut app);
    let source = mail_task(&app, "M1");
    assert_eq!(
        spy.with_profile(MAIL),
        0,
        "sin respuesta no se redacta nada"
    );
    app.redraft(source.id).unwrap();
    process_all(&mut app);
    assert_eq!(spy.with_profile(MAIL), 1);
    assert_eq!(spy.of(AiTask::Draft, "SENTINELA-M1").len(), 1);
    drop(dir);
}

// ---- Qué se guarda ----

#[test]
fn the_result_keeps_only_the_fingerprint_never_the_profile_text() {
    let spy = Spy::external();
    let (dir, mut app) = setup(spy, "Consulta");
    grant(&mut app);
    let fp = app.profile_preview().unwrap().fp.unwrap();
    process_all(&mut app);
    let result = result_of(&mail_task(&app, "M1"));
    assert_eq!(result["profile_fp"], json!(fp));
    assert_eq!(fp.len(), 16);
    let text = result.to_string();
    for fragment in [
        SENTINEL,
        "Taller Ficticio",
        "taller-ficticio.test",
        "600 111 222",
        "14:00",
    ] {
        assert!(
            !text.contains(fragment),
            "{fragment} no debe guardarse en el resultado"
        );
    }
    // Ni en las tareas, ni en el historial, ni en los ajustes del panel.
    for query in [
        "SELECT count(*) FROM tasks WHERE result LIKE '%SENTINELA-PERFIL%' OR input_ref LIKE '%SENTINELA-PERFIL%'",
        "SELECT count(*) FROM audit_log WHERE detail LIKE '%SENTINELA-PERFIL%' OR detail LIKE '%Taller Ficticio%'",
        "SELECT count(*) FROM rule_settings WHERE value LIKE '%SENTINELA-PERFIL%'",
    ] {
        assert_eq!(count(dir.path(), query), 0, "{query}");
    }
    drop(dir);
}

#[test]
fn the_preview_is_literally_what_the_draft_request_carries() {
    let spy = Spy::external();
    let (dir, mut app) = setup(spy.clone(), "Consulta");
    grant(&mut app);
    let preview = app.profile_preview().unwrap();
    assert!(preview.has_profile && preview.external && preview.will_send);
    assert_eq!(preview.host.as_deref(), Some("ollama.test"));
    assert_eq!(preview.provider.model, "modelo-a");
    let block = preview.block.clone().unwrap();
    assert!(block.contains("Horario: Lunes a viernes de 9:00 a 14:00"));
    assert!(!block.contains("NIF"));
    process_all(&mut app);
    let sent = spy.of(AiTask::Draft, "SENTINELA-M1").remove(0);
    let base = draft_request("x", "y", None).instructions;
    assert_eq!(sent.instructions, format!("{base}\n\n{block}"));
    // Sin perfil, la vista previa no muestra nada y la petición no lleva nada.
    app.clear_business_profile().unwrap();
    let empty = app.profile_preview().unwrap();
    assert!(!empty.has_profile && empty.block.is_none() && !empty.will_send);
    drop(dir);
}

// ---- Contactos en el borrador (D160) ----

fn flags(task: &Task) -> Vec<String> {
    result_of(task)["draft_flags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn contacts_that_come_from_the_profile_are_not_flagged_and_any_other_still_is() {
    // Con el perfil enviado, sus contactos no se marcan.
    let spy = Spy::external();
    let (dir, mut app) = setup(spy, "Consulta [[CONTACTOS-PERFIL]]");
    grant(&mut app);
    process_all(&mut app);
    let task = mail_task(&app, "M1");
    assert_eq!(flags(&task), Vec::<String>::new(), "contactos del perfil");
    drop(dir);

    // Un contacto que no es del perfil se marca aunque haya perfil.
    let spy = Spy::external();
    let (dir, mut app) = setup(spy, "Consulta [[CONTACTO-AJENO]]");
    grant(&mut app);
    process_all(&mut app);
    assert_eq!(flags(&mail_task(&app, "M1")), vec!["email", "phone", "url"]);
    drop(dir);

    // Sin consentimiento el perfil no iba en la petición: nada se avala y todo se marca.
    let spy = Spy::external();
    let (dir, mut app) = setup(spy, "Consulta [[CONTACTOS-PERFIL]]");
    process_all(&mut app);
    assert_eq!(
        flags(&mail_task(&app, "M1")),
        vec!["email", "phone", "url"],
        "sin perfil enviado, los contactos se marcan"
    );
    drop(dir);
}

// ---- Validación al guardar ----

fn with_policies(text: &str) -> BusinessProfile {
    BusinessProfile {
        policies: text.into(),
        ..Default::default()
    }
}

#[test]
fn saving_blocks_iban_cards_keys_and_excess_and_only_warns_about_phone_and_email() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open(dir.path(), Spy::local());
    for (text, expected) in [
        ("Transferencias al ES91 2100 0418 4502 0005 1332", "IBAN"),
        ("Pague con 4539 1488 0343 6467 si quiere", "tarjeta"),
        ("Tarjeta 4539148803436467", "tarjeta"),
        ("Mi clave es sk-ejemplo1234567890abcd", "clave"),
        ("contraseña: Gato12345", "clave"),
        ("Token interno a1B2c3D4e5F6g7H8i9J0k1L2m3N4", "clave"),
        ("-----BEGIN PRIVATE KEY----- AAAA", "clave"),
        // Escrituras que esquivarían un examen ingenuo: espacio no separable, puntos, dos
        // espacios, guion suave, cifras de ancho completo, IBAN con otra separación y un prefijo
        // partido por un carácter invisible.
        ("Tarjeta 4539\u{a0}1488\u{a0}0343\u{a0}6467", "tarjeta"),
        ("Tarjeta 4539.1488.0343.6467", "tarjeta"),
        ("Tarjeta 4539  1488  0343  6467", "tarjeta"),
        ("Tarjeta 4539 - 1488 - 0343 - 6467", "tarjeta"),
        ("Tarjeta 4539\u{ad}1488\u{ad}0343\u{ad}6467", "tarjeta"),
        (
            "Tarjeta \u{ff14}\u{ff15}\u{ff13}\u{ff19}148803436467",
            "tarjeta",
        ),
        ("Cuenta ES 91 2100 0418 4502 0005 1332", "IBAN"),
        ("Cuenta ES91-2100-0418-4502-0005-1332", "IBAN"),
        ("Cuenta es91.2100.0418.4502.0005.1332", "IBAN"),
        ("Clave sk-\u{200b}ejemplo1234567890abcd", "clave"),
    ] {
        let error = app
            .save_business_profile(with_policies(text))
            .expect_err(text)
            .to_string();
        assert!(error.contains(expected), "{text}: {error}");
    }
    assert!(!app.business_profile().unwrap().saved, "nada se guardó");
    let too_long = with_policies(&"a ".repeat(1001));
    assert!(app
        .save_business_profile(too_long)
        .unwrap_err()
        .to_string()
        .contains("2000"));
    // Justo en el tope pasa.
    let exact = with_policies(&"x".repeat(2000));
    assert!(app.save_business_profile(exact).is_ok());

    // Un teléfono, un correo, una fecha o un número de tarjeta que no cumple Luhn no bloquean.
    let saved = app
        .save_business_profile(with_policies(
            "Llame al 600 111 222 o escriba a hola@taller-ficticio.test. Desde 2026-10-01. Pedido 4539 1488 0343 6468",
        ))
        .unwrap();
    let codes: Vec<&str> = saved.warnings.iter().map(|w| w.code).collect();
    assert!(
        codes.contains(&"phone") && codes.contains(&"email"),
        "{codes:?}"
    );
    assert!(saved.view.saved);

    // Un perfil en blanco no es un perfil.
    let blank = app.save_business_profile(with_policies("   ")).unwrap();
    assert!(!blank.view.saved);
}

#[test]
fn a_profile_that_somehow_fails_the_check_is_never_sent() {
    let spy = Spy::local();
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), vec![msg("M1", "Consulta", "Hola SENTINELA-M1")]);
    let mut app = open(dir.path(), spy.clone());
    app.save_business_profile(profile()).unwrap();
    app.fetch_mail().unwrap();
    // Una base editada a mano con un IBAN: la vista previa lo dice y la redacción no lo envía.
    rusqlite::Connection::open(dir.path().join("nestadmin.db"))
        .unwrap()
        .execute(
            "UPDATE business_profile SET policies = ?1",
            ["Pagos al ES91 2100 0418 4502 0005 1332 (SENTINELA-PERFIL)"],
        )
        .unwrap();
    let preview = app.profile_preview().unwrap();
    assert_eq!(preview.blocked, Some("iban"));
    assert!(preview.block.is_none() && !preview.will_send);
    process_all(&mut app);
    assert_eq!(spy.with_profile(MAIL), 0);
    assert_eq!(mail_task(&app, "M1").state, TaskState::AwaitingReview);
}

// ---- Precarga ----

#[test]
fn prefill_uses_the_company_name_address_and_email_but_never_the_tax_id_or_the_iban() {
    use nestadmin_core::sales::Company;
    let dir = tempfile::tempdir().unwrap();
    let app = open(dir.path(), Spy::local());
    assert_eq!(
        app.business_profile_prefill().unwrap(),
        BusinessProfile::default()
    );
    drop(app);
    // Los datos de la empresa se guardan por la capa de ventas (aún sin pantalla propia).
    let mut store = Store::open(&dir.path().join("nestadmin.db")).unwrap();
    store
        .set_sales_company(&Company {
            name: Some("Taller Ficticio SL".into()),
            tax_id: Some("B12345674".into()),
            address: Some("Calle Falsa 1, Ciudad Ficticia".into()),
            email: Some("hola@taller-ficticio.test".into()),
            iban: Some("ES9121000418450200051332".into()),
            ..Default::default()
        })
        .unwrap();
    drop(store);
    let app = open(dir.path(), Spy::local());
    let prefill = app.business_profile_prefill().unwrap();
    assert_eq!(prefill.display_name, "Taller Ficticio SL");
    assert!(prefill.signature.contains("Calle Falsa 1"));
    assert!(prefill.signature.contains("hola@taller-ficticio.test"));
    let all = format!("{prefill:?}");
    assert!(!all.contains("B12345674") && !all.contains("ES9121"));
    // La precarga no guarda nada ni envía nada.
    assert!(!app.business_profile().unwrap().saved);
}

#[test]
fn local_mail_without_the_ai_is_untouched_by_the_profile() {
    // Un correo informativo se cierra en local y nunca llama a ningún proveedor, haya o no perfil.
    let spy = Spy::external();
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        vec![Message {
            from: Some("Avisos <no-reply@servicio.test>".into()),
            ..msg("M1", "Aviso", "Hola SENTINELA-M1")
        }],
    );
    let mut app = open(dir.path(), spy.clone());
    app.save_business_profile(profile()).unwrap();
    grant(&mut app);
    app.fetch_mail().unwrap();
    process_all(&mut app);
    assert_eq!(spy.of(AiTask::Draft, "SENTINELA-M1").len(), 0);
    assert_eq!(spy.with_profile(MAIL), 0);
}

#[test]
fn a_cut_between_saving_the_profile_and_dropping_the_consent_still_sends_nothing() {
    // Si el guardado llegó a la base y el borrado del consentimiento no, el consentimiento queda
    // dormido con la huella anterior: no coincide con el bloque de ahora y no sale nada.
    let spy = Spy::external();
    let (dir, mut app) = setup(spy.clone(), "Consulta");
    grant(&mut app);
    rusqlite::Connection::open(dir.path().join("nestadmin.db"))
        .unwrap()
        .execute(
            "UPDATE business_profile SET hours = 'Horario cambiado a mano'",
            [],
        )
        .unwrap();
    assert_eq!(app.profile_consent().unwrap().state, "stale");
    assert!(!app.profile_preview().unwrap().will_send);
    process_all(&mut app);
    assert_eq!(spy.with_profile(MAIL), 0);
    assert!(result_of(&mail_task(&app, "M1"))
        .get("profile_fp")
        .is_none());
}

#[test]
fn during_the_open_period_the_local_goes_first_and_the_external_gets_the_profile_only_with_consent()
{
    for consent in [false, true] {
        let external = Spy::external();
        let local = Spy::local();
        let dir = tempfile::tempdir().unwrap();
        seed(
            dir.path(),
            vec![
                msg("P1", "Consulta [[CAIDA]]", "Hola SENTINELA-P1"),
                msg("P2", "Consulta [[LOCAL-CAIDA]]", "Hola SENTINELA-P2"),
                msg(
                    "P3",
                    "Consulta [[LOCAL-CAIDA]] [[CAIDA-REDACCION]]",
                    "Hola SENTINELA-P3",
                ),
            ],
        );
        let mut app = open(dir.path(), external.clone());
        with_fallback(&mut app, &local);
        app.save_business_profile(profile()).unwrap();
        if consent {
            grant(&mut app);
        }
        app.fetch_mail().unwrap();
        // P1 abre el periodo de 10 minutos con un `unavailable`.
        app.process_next().unwrap().unwrap();
        assert!(app.fallback_period_active());
        process_all(&mut app);

        // P2: en el periodo va primero a la IA local (que falla) y luego al externo, que recibe el
        // perfil solo con consentimiento; la IA local lo recibió siempre.
        let (p2_ext, p2_local) = (
            external.with_profile("SENTINELA-P2"),
            local.with_profile("SENTINELA-P2"),
        );
        assert_eq!(
            (p2_ext, p2_local),
            (usize::from(consent), 1),
            "consent={consent}"
        );
        let task = mail_task(&app, "P2");
        assert_eq!(task.state, TaskState::AwaitingReview);
        assert_eq!(
            result_of(&task).get("profile_fp").is_some(),
            consent,
            "la huella solo consta si el perfil salió de verdad hacia quien redactó"
        );
        // P3: el externo también falla en la redacción y la local también: la tarea no llega a
        // revisión, no queda ningún borrador ni huella y no se envía nada.
        let failed = mail_task(&app, "P3");
        assert_ne!(failed.state, TaskState::AwaitingReview);
        assert!(failed
            .result
            .as_ref()
            .is_none_or(|r| r.get("profile_fp").is_none() && r.get("draft").is_none()));
        // Ninguna petición de clasificación lleva el perfil, en ningún proveedor.
        for spy in [&external, &local] {
            assert!(spy
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.task == AiTask::Classification)
                .all(|r| !r.instructions.contains(SENTINEL)));
        }
        assert_eq!(app.simulators().0.sent().unwrap().len(), 0);
    }
}
