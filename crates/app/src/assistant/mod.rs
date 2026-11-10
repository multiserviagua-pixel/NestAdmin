//! Asistente de consulta del chat (D182–D184, D187, D188, D190, R33): una conversación con la IA
//! en uso que responde a preguntas sobre los documentos, los correos y el estado de la
//! aplicación con una lista cerrada de herramientas de solo lectura (nivel «libre»).
//!
//! El turno se divide en tres tiempos, como las demás llamadas con IA, para que la llamada al
//! modelo no retenga el cerrojo de `App`: `begin_assistant_turn` (con el cerrojo) valida, decide
//! a dónde va el turno (D183) y toma el turno exclusivo; `AssistantJob::run` (sin cerrojo) llama
//! a la IA paso a paso y, para cada herramienta, recibe un cierre que el llamador implementa
//! tomando el cerrojo solo mientras la herramienta lee; `finish_assistant_turn` (con el cerrojo)
//! valida las citas, guarda y audita. Ninguna llamada a la IA se hace con el cerrojo tomado.
//!
//! Nunca de local a externa ni a otro externo: el único respaldo es el de D101 (externo elegido
//! a la IA local), a cargo de `generate_with_fallback`. Se decide UNA vez por turno: tras el primer
//! respaldo, el resto del turno sigue con la IA local sin volver a intentar el externo, y el periodo
//! de 10 minutos del turno es propio: el asistente nunca lee ni escribe el de la cola.
//!
//! Sin consentimiento o sin IA externa utilizable, el asistente no arranca nada: solo usa la IA
//! local si el resolvedor `assistant_local` (que no arranca procesos) la da ya en marcha.

pub mod consent;
pub mod tools;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use nestadmin_ai::assistant::{
    assistant_request, doc_id, parse_step, skills_for, tools_for, Entry, HistoryTurn, Step, View,
};
use nestadmin_ai::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};
use nestadmin_core::{NewCall, SearchHit, SentCall};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use self::tools::{
    fragment_count, split_masked, strip_fragments, ToolCall, ToolOutput, WITHHELD_MARK,
};
use crate::learn::{is_masked, mask};
use crate::refine::now_ms;
use crate::search::QuestionAnswer;
use crate::{generate_with_fallback, App, AppError, AppResult, LocalFallback};

/// Pasos (llamadas al modelo) como mucho por turno (D182).
pub const MAX_STEPS: usize = 6;
/// Salidas no válidas del modelo tras las cuales el turno termina con error (D182).
pub const MAX_INVALID: usize = 2;
/// Últimos turnos de la conversación que salen en cada llamada (D183).
pub const MAX_HISTORY_TURNS: usize = 6;
/// Caracteres de cada turno anterior (D183).
pub const MAX_TURN_CHARS: usize = 2_000;
/// Fragmentos de texto de documentos por llamada (D183).
pub const MAX_FRAGMENTS: usize = 8;
/// Caracteres de cada fragmento (D183).
pub const MAX_FRAGMENT_CHARS: usize = 600;
/// Caracteres de datos por llamada: turnos anteriores, pregunta y resultados (D183).
pub const MAX_DATA_CHARS: usize = 12_000;
/// Caracteres de una pregunta. Más largo es un texto pegado, no una pregunta.
pub const MAX_QUESTION_CHARS: usize = 4_000;

/// Marca de un fragmento recortado.
const CUT_MARK: &str = "…[recortado]";

/// Estado en memoria del asistente: no se guarda nada de esto.
#[derive(Default)]
pub(crate) struct AssistantRuntime {
    /// Hay consentimiento vigente para el turno que se está preparando o ejecutando: lo escribe
    /// `begin` y lo borran la retirada y el cambio de destino; `run` lo mira antes de cada
    /// llamada externa.
    pub(crate) gate: Arc<AtomicBool>,
    /// Petición de cancelar el turno en curso.
    cancel: Arc<AtomicBool>,
    /// Hay un turno en curso: solo uno a la vez.
    busy: Arc<AtomicBool>,
}

/// El turno exclusivo: se libera al soltarlo, también si el trabajo entra en pánico.
struct TurnGuard(Arc<AtomicBool>);

impl TurnGuard {
    fn take(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(Arc::clone(flag)))
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Lo que el asistente está haciendo, para mostrarlo mientras trabaja (D182): un código cerrado y
/// nunca contenido (ni la pregunta, ni los argumentos, ni los resultados).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Progress {
    /// Espera la respuesta del modelo.
    Thinking,
    /// Ejecuta una herramienta del catálogo cerrado.
    Tool { name: String },
}

/// Resultado de un turno.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Answered,
    /// Proveedor externo sin la opción de D183 y sin IA local: no se envió nada.
    ConsentRequired,
    /// Sin IA utilizable y sin IA local.
    NotConfigured,
    Cancelled,
    Error,
}

/// A dónde va el turno, decidido en `begin` con el cerrojo (D183).
enum Route {
    /// Proveedor externo elegido, utilizable y con consentimiento vigente.
    External {
        provider: Arc<dyn Provider>,
        fallback: Option<LocalFallback>,
    },
    /// La IA local es el proveedor elegido y es utilizable.
    Local(Arc<dyn Provider>),
    /// Sin consentimiento o sin IA externa utilizable: solo la IA local si ya está en marcha al
    /// llamar (`assistant_local`, que no arranca nada); si no, el resultado `otherwise`.
    LocalOr {
        resolver: Option<LocalFallback>,
        otherwise: Outcome,
    },
}

/// Lo que el modelo vio de un paso anterior, tal como se guarda para recomponer la petición.
pub(crate) enum StoredEntry {
    Tool {
        name: String,
        args: String,
        value: Value,
    },
    Invalid {
        code: String,
    },
}

/// Turno preparado: no toca `App`, se puede ejecutar en otro hilo.
pub struct AssistantJob {
    conversation_id: Option<i64>,
    question: String,
    view: View,
    history: Vec<HistoryTurn>,
    /// Turnos anteriores que no salieron por no superar el enmascarado.
    history_withheld: usize,
    route: Route,
    gate: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    /// Proveedor y modelo del turno, para mostrarlos si no llega a haber llamada.
    info: (ProviderKind, String),
    guard: TurnGuard,
}

/// Cómo terminó la parte con IA de un turno.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RunStatus {
    Answered,
    ConsentRequired,
    NotConfigured,
    Cancelled,
    Error(String),
}

/// Lo que `run` devuelve a `finish_assistant_turn`. Lleva el turno exclusivo hasta que se guarda.
pub struct AssistantRun {
    conversation_id: Option<i64>,
    question: String,
    status: RunStatus,
    answer: String,
    citation_refs: Vec<String>,
    known_refs: HashSet<String>,
    steps: usize,
    tools: Vec<String>,
    navigate: Option<String>,
    withheld: usize,
    calls: Vec<NewCall>,
    provider: ProviderKind,
    model: String,
    local: bool,
    fallback: Option<Value>,
    _guard: TurnGuard,
}

/// Un turno del asistente tal como llega a la ventana.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantTurn {
    /// La conversación a la que pertenece. `None` si el turno no se guardó y no había una.
    pub conversation_id: Option<i64>,
    /// El turno de respuesta guardado, para «Ver lo enviado». `None` si no se guardó.
    pub turn_id: Option<i64>,
    pub outcome: Outcome,
    /// Código de error del proveedor o del protocolo (`outcome` = `error`).
    pub error_code: Option<String>,
    /// La respuesta, o el motivo si no la hay. Texto plano.
    pub answer: String,
    /// Tarjetas de los documentos citados (solo referencias que una herramienta devolvió).
    pub citations: Vec<SearchHit>,
    /// Citas descartadas por no haber sido devueltas por ninguna herramienta.
    pub dropped_citations: usize,
    /// Textos que no superaron el enmascarado y no salieron.
    pub withheld: usize,
    /// Vista a la que debe navegar la ventana.
    pub navigate: Option<String>,
    pub provider: ProviderKind,
    pub model: String,
    /// Contestó la IA local.
    pub local: bool,
    /// `{from, reason}` si respondió el respaldo local de D101.
    pub fallback: Option<Value>,
    pub steps: usize,
    pub tools: Vec<String>,
    /// Búsqueda por palabras, solo con `consent_required` y `not_configured` (D181).
    pub words: Option<QuestionAnswer>,
}

/// Lo que se guarda de un turno junto a su texto (en `meta_json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TurnMeta {
    pub outcome: Option<String>,
    pub citation_refs: Vec<String>,
    pub dropped_citations: usize,
    pub withheld: usize,
    pub navigate: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub local: bool,
    pub fallback: Option<Value>,
    pub steps: usize,
    pub tools: Vec<String>,
    /// Código del fallo si el turno terminó con `error`.
    pub error_code: Option<String>,
}

/// Un turno de una conversación guardada.
#[derive(Debug, Clone, Serialize)]
pub struct ConversationTurn {
    pub id: i64,
    /// `user` o `assistant`.
    pub role: String,
    pub text: String,
    pub created_at: i64,
    #[serde(flatten)]
    pub meta: TurnMeta,
    /// Tarjetas de los documentos citados, tal como están ahora en el índice.
    pub citations: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantConversation {
    pub id: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub turns: Vec<ConversationTurn>,
}

// ---- Composición de la petición: topes (D183) ----

/// Quita lo más antiguo hasta que los datos quepan: primero los turnos anteriores, después los
/// resultados de pasos viejos (siempre queda el último) y, si aun así no cabe, recorta el
/// último resultado. Nunca toca la pregunta actual.
fn fit(
    mut history: Vec<HistoryTurn>,
    question: &str,
    mut entries: Vec<Entry>,
) -> (Vec<HistoryTurn>, Vec<Entry>) {
    let total = |h: &[HistoryTurn], e: &[Entry]| {
        question.chars().count()
            + h.iter().map(|t| t.text.chars().count()).sum::<usize>()
            + e.iter().map(Entry::chars).sum::<usize>()
    };
    while total(&history, &entries) > MAX_DATA_CHARS && !history.is_empty() {
        history.remove(0);
    }
    while total(&history, &entries) > MAX_DATA_CHARS && entries.len() > 1 {
        entries.remove(0);
    }
    let over = total(&history, &entries).saturating_sub(MAX_DATA_CHARS);
    if over > 0 {
        if let Some(Entry::Tool { output, .. }) = entries.last_mut() {
            let keep = output
                .chars()
                .count()
                .saturating_sub(over + CUT_MARK.chars().count());
            *output = output.chars().take(keep).collect::<String>() + CUT_MARK;
        }
    }
    (history, entries)
}

/// La petición de un paso con los topes de D183: como mucho 8 fragmentos (los de pasos antiguos
/// se quitan primero) y 12.000 caracteres de datos. Es la única función que la compone, para el
/// envío real y para la vista previa.
pub(crate) fn compose(
    request_id: &str,
    view: View,
    history: &[HistoryTurn],
    question: &str,
    entries: &[StoredEntry],
) -> GenerateRequest {
    let mut budget = MAX_FRAGMENTS;
    let mut prepared: Vec<Entry> = Vec::new();
    for entry in entries.iter().rev() {
        prepared.push(match entry {
            StoredEntry::Tool { name, args, value } => {
                let mut value = value.clone();
                let n = fragment_count(&value);
                if n <= budget {
                    budget -= n;
                } else {
                    strip_fragments(&mut value);
                }
                Entry::Tool {
                    name: name.clone(),
                    args: args.clone(),
                    output: value.to_string(),
                }
            }
            StoredEntry::Invalid { code } => Entry::Invalid { code: code.clone() },
        });
    }
    prepared.reverse();
    let (history, prepared) = fit(history.to_vec(), question, prepared);
    assistant_request(request_id, view, &history, question, &prepared)
}

/// Texto literal de una llamada, tal como se guarda para «Ver lo enviado».
fn sent_text(request: &GenerateRequest) -> String {
    format!("{}\n\n{}", request.instructions, request.content)
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Texto fijo de cada fallo (sin contenido del proveedor ni de los documentos).
fn error_message(code: &str) -> String {
    match code {
        "unauthorized" => "El proveedor de IA rechazó la clave. Corríjala en Configuración.",
        "timeout" => "El proveedor de IA no respondió a tiempo. Inténtelo de nuevo.",
        "rate_limited" => {
            "El proveedor de IA limita las peticiones. Espere un momento e inténtelo de nuevo."
        }
        "no_credit" => "La cuenta del proveedor de IA no tiene crédito.",
        "model_unavailable" => {
            "El proveedor de IA no encuentra el modelo o la dirección configurados. Revíselos en Configuración."
        }
        "request_rejected" => "El proveedor de IA rechazó esta consulta.",
        "unavailable" => "El proveedor de IA no está disponible. Inténtelo de nuevo más tarde.",
        "invalid_response" => {
            "La IA no devolvió una respuesta válida tras dos intentos; no se ha hecho nada."
        }
        "steps_exhausted" => {
            "La consulta necesitó más pasos de los permitidos y se detuvo sin respuesta."
        }
        _ => "El asistente no pudo completar la consulta.",
    }
    .to_owned()
}

impl AssistantJob {
    /// Llama a la IA paso a paso (máximo `MAX_STEPS`) y ejecuta las herramientas con `tools`, que
    /// toma el cerrojo de `App` solo mientras lee. Con el externo: una llamada por paso con el
    /// respaldo de D101 y comprobando justo antes que el consentimiento sigue vigente; sin él,
    /// no sale nada. Un fallo del proveedor termina el turno con su código y sin efectos.
    pub fn run(self, tools: &mut dyn FnMut(&ToolCall) -> ToolOutput) -> AssistantRun {
        self.run_with_progress(tools, &mut |_| {})
    }

    /// Como `run`, y avisa a `progress` al empezar cada paso: `Thinking` antes de cada llamada al
    /// modelo y `Tool` antes de cada herramienta.
    pub fn run_with_progress(
        self,
        tools: &mut dyn FnMut(&ToolCall) -> ToolOutput,
        progress: &mut dyn FnMut(Progress),
    ) -> AssistantRun {
        let AssistantJob {
            conversation_id,
            question,
            view,
            history,
            history_withheld,
            route,
            gate,
            cancel,
            info,
            guard,
        } = self;
        let mut state = AssistantRun {
            conversation_id,
            question: question.clone(),
            status: RunStatus::Answered,
            answer: String::new(),
            citation_refs: Vec::new(),
            known_refs: HashSet::new(),
            steps: 0,
            tools: Vec::new(),
            navigate: None,
            withheld: history_withheld,
            calls: Vec::new(),
            provider: info.0,
            model: info.1,
            local: false,
            fallback: None,
            _guard: guard,
        };
        let external = matches!(route, Route::External { .. });
        // Periodo de respaldo propio del turno: nunca el compartido con la cola.
        let turn_until: Mutex<Option<Instant>> = Mutex::new(None);
        // Tras el primer respaldo del turno, todo lo que queda va a la IA local.
        let mut pinned_local = false;
        let local_target: Option<Arc<dyn Provider>> = match &route {
            Route::External { .. } => None,
            Route::Local(provider) => Some(Arc::clone(provider)),
            Route::LocalOr { resolver, .. } => resolver
                .as_ref()
                .and_then(|resolve| resolve())
                // Solo vale si es Ollama: la IA local nunca es un servicio externo.
                .filter(|p| p.kind() == ProviderKind::Ollama),
        };
        if let Route::LocalOr { otherwise, .. } = &route {
            if local_target.is_none() {
                state.status = match otherwise {
                    Outcome::ConsentRequired => RunStatus::ConsentRequired,
                    _ => RunStatus::NotConfigured,
                };
                return state;
            }
        }
        if let Some(local) = &local_target {
            state.provider = local.kind();
            state.model = local.model().to_owned();
        }
        let offered: Vec<&'static str> = tools_for(&skills_for(view, &question))
            .into_iter()
            .map(|t| t.name)
            .collect();
        let mut entries: Vec<StoredEntry> = Vec::new();
        let mut invalid = 0usize;
        loop {
            if cancel.load(Ordering::SeqCst) {
                state.status = RunStatus::Cancelled;
                return state;
            }
            if state.steps >= MAX_STEPS {
                state.status = RunStatus::Error("steps_exhausted".into());
                return state;
            }
            // Justo antes de enviar: si el consentimiento se retiró, no sale nada.
            if external && !pinned_local && !gate.load(Ordering::SeqCst) {
                state.status = RunStatus::ConsentRequired;
                return state;
            }
            state.steps += 1;
            progress(Progress::Thinking);
            let request = compose(
                &format!("assistant-{}-{}", now_ms(), state.steps),
                view,
                &history,
                &question,
                &entries,
            );
            let step_local: bool;
            let (result, fallback, host) = match (&route, &local_target) {
                (Route::External { fallback, .. }, _) if pinned_local => {
                    // Ya cayó a la local en este turno: no se vuelve a intentar el externo.
                    step_local = true;
                    let result = fallback
                        .as_ref()
                        .and_then(|resolve| resolve())
                        .filter(|p| p.kind() == ProviderKind::Ollama)
                        .map_or_else(
                            || Err("unavailable".to_owned()),
                            |local| {
                                local
                                    .generate(&request)
                                    .map_err(|e: AiError| e.code().to_owned())
                            },
                        );
                    (result, None, String::new())
                }
                (Route::External { provider, fallback }, _) => {
                    let attempt = generate_with_fallback(
                        &**provider,
                        fallback.as_ref(),
                        &turn_until,
                        &request,
                    );
                    step_local = attempt.fallback.is_some();
                    (
                        attempt.result.map_err(|e| e.code),
                        attempt.fallback,
                        provider.destination().unwrap_or_default(),
                    )
                }
                (_, Some(local)) => {
                    step_local = true;
                    (
                        local
                            .generate(&request)
                            .map_err(|e: AiError| e.code().to_owned()),
                        None,
                        String::new(),
                    )
                }
                _ => unreachable!("sin destino se salió antes"),
            };
            if fallback.is_some() {
                pinned_local = true;
            }
            let (call_provider, call_model, local_answered) = match &result {
                Ok(r) => (
                    r.provider,
                    r.model.clone(),
                    r.provider == ProviderKind::Ollama,
                ),
                Err(_) => (state.provider, state.model.clone(), step_local),
            };
            state.calls.push(NewCall {
                step: u32::try_from(state.steps).unwrap_or(u32::MAX),
                provider: call_provider.to_string(),
                model: call_model.clone(),
                host: if local_answered { String::new() } else { host },
                local: local_answered,
                sent_text: sent_text(&request),
            });
            if cancel.load(Ordering::SeqCst) {
                // La llamada en curso no se puede abortar: se descarta su resultado.
                state.status = RunStatus::Cancelled;
                return state;
            }
            let GenerateResult {
                provider,
                model,
                output,
                ..
            } = match result {
                Ok(r) => r,
                Err(code) => {
                    state.status = RunStatus::Error(code);
                    return state;
                }
            };
            state.provider = provider;
            state.model = model;
            state.local = provider == ProviderKind::Ollama;
            if state.fallback.is_none() {
                state.fallback = fallback;
            }
            let step = match parse_step(&output) {
                Ok(step) => step,
                Err(error) => {
                    invalid += 1;
                    if invalid >= MAX_INVALID {
                        state.status = RunStatus::Error("invalid_response".into());
                        return state;
                    }
                    let code = match error {
                        AiError::InvalidResponse(code) => code,
                        other => other.code().to_owned(),
                    };
                    entries.push(StoredEntry::Invalid { code });
                    continue;
                }
            };
            match step {
                Step::Answer { text, citations } => {
                    state.answer = text;
                    state.citation_refs = citations;
                    state.status = RunStatus::Answered;
                    return state;
                }
                Step::Tool { name, args } => {
                    if !offered.contains(&name.as_str()) {
                        // Del catálogo, pero no de los bloques de esta vista: también es no válida.
                        invalid += 1;
                        if invalid >= MAX_INVALID {
                            state.status = RunStatus::Error("invalid_response".into());
                            return state;
                        }
                        entries.push(StoredEntry::Invalid {
                            code: "tool_not_offered".into(),
                        });
                        continue;
                    }
                    if state.steps >= MAX_STEPS {
                        // No quedan pasos para usar el resultado: no se ejecuta.
                        state.status = RunStatus::Error("steps_exhausted".into());
                        return state;
                    }
                    if cancel.load(Ordering::SeqCst) {
                        state.status = RunStatus::Cancelled;
                        return state;
                    }
                    progress(Progress::Tool { name: name.clone() });
                    let call = ToolCall {
                        name: name.clone(),
                        args: args.clone(),
                        external,
                    };
                    let out = tools(&call);
                    state.withheld += out.withheld;
                    state.known_refs.extend(out.refs.iter().cloned());
                    if out.navigate.is_some() {
                        state.navigate = out.navigate.clone();
                    }
                    if !state.tools.contains(&name) {
                        state.tools.push(name.clone());
                    }
                    entries.push(StoredEntry::Tool {
                        name,
                        args: Value::Object(args).to_string(),
                        value: out.value,
                    });
                }
            }
        }
    }
}

/// Turnos anteriores listos para salir: como mucho `MAX_HISTORY_TURNS`, de `MAX_TURN_CHARS`
/// cada uno. Un turno del asistente que no fue una respuesta no cuenta (ni la pregunta que lo
/// provocó). Con un destino externo, lo que el asistente respondió antes (quizá con la IA local y
/// datos completos) pasa por el enmascarado y, si no lo supera, no sale.
fn history_for(
    stored: Vec<nestadmin_core::StoredTurn>,
    external: bool,
) -> (Vec<HistoryTurn>, usize) {
    let mut keep: Vec<(bool, String)> = Vec::new();
    for turn in stored {
        let from_user = turn.role == "user";
        if !from_user {
            let outcome = turn.meta["outcome"].as_str();
            if outcome != Some("answered") {
                // Quita también la pregunta que la provocó.
                if keep.last().is_some_and(|(user, _)| *user) {
                    keep.pop();
                }
                continue;
            }
        }
        keep.push((from_user, turn.text));
    }
    let mut withheld = 0;
    let mut out = Vec::new();
    for (from_user, text) in keep {
        let text = if external && !from_user {
            // Se enmascara el turno ENTERO y después se recorta: un dato partido por el corte
            // quedaría en dos mitades sin enmascarar.
            let masked = mask(&text);
            let (head, _) = split_masked(masked.trim(), MAX_TURN_CHARS);
            if is_masked(head) {
                head.to_owned()
            } else {
                withheld += 1;
                WITHHELD_MARK.to_owned()
            }
        } else {
            clip(&text, MAX_TURN_CHARS)
        };
        out.push(HistoryTurn { from_user, text });
    }
    (out, withheld)
}

impl App {
    /// Prepara un turno (con el cerrojo): valida la pregunta, decide a dónde va (D183) y toma el
    /// turno exclusivo. No llama a ningún proveedor ni a la IA local, y no guarda nada.
    ///
    /// 1. IA local elegida y utilizable: la local; sin consentimiento.
    /// 2. Proveedor externo utilizable y con consentimiento vigente: el externo (con el respaldo
    ///    de D101).
    /// 3. Externo sin consentimiento: solo la IA local si el resolvedor del asistente la da ya en
    ///    marcha (sin arrancar nada); si no, `consent_required`.
    /// 4. Sin IA utilizable: igual con la IA local; si no, `not_configured`.
    pub fn begin_assistant_turn(
        &mut self,
        conversation_id: Option<i64>,
        text: &str,
        view: &str,
    ) -> AppResult<AssistantJob> {
        let question = text.trim();
        if question.is_empty() {
            return Err(AppError::Invalid("Escriba una pregunta.".into()));
        }
        if question.chars().count() > MAX_QUESTION_CHARS {
            return Err(AppError::Invalid(format!(
                "La pregunta es demasiado larga (como mucho {MAX_QUESTION_CHARS} caracteres)."
            )));
        }
        let Some(guard) = TurnGuard::take(&self.assistant.busy) else {
            return Err(AppError::Invalid(
                "Ya hay una consulta en curso: espere a que termine o cancélela.".into(),
            ));
        };
        let stored = match conversation_id {
            Some(id) => {
                if self.store.assistant_conversation(id)?.is_none() {
                    return Err(AppError::Core(
                        nestadmin_core::CoreError::ConversationNotFound(id),
                    ));
                }
                self.store.assistant_recent_turns(id, MAX_HISTORY_TURNS)?
            }
            None => Vec::new(),
        };
        let external_provider = self.provider.kind() != ProviderKind::Ollama;
        let consent_granted = external_provider && self.assistant_consent()?.state == "granted";
        // La IA local del asistente no arranca nada y solo existe con el respaldo local activado:
        // ni `learning.local_ai` ni `fallback` valen, porque pueden arrancar Ollama.
        let local_source = self.assistant_local.clone();
        let route = if !external_provider {
            if self.ai_wait.is_none() {
                Route::Local(Arc::clone(&self.provider))
            } else {
                Route::LocalOr {
                    resolver: None,
                    otherwise: Outcome::NotConfigured,
                }
            }
        } else if self.ai_wait.is_some() {
            Route::LocalOr {
                resolver: local_source,
                otherwise: Outcome::NotConfigured,
            }
        } else if consent_granted {
            // Consentimiento vigente: lo comprueba `run` antes de cada llamada.
            self.assistant.gate.store(true, Ordering::SeqCst);
            Route::External {
                provider: Arc::clone(&self.provider),
                fallback: self.fallback.clone(),
            }
        } else {
            Route::LocalOr {
                resolver: local_source,
                otherwise: Outcome::ConsentRequired,
            }
        };
        let external = matches!(route, Route::External { .. });
        let (history, history_withheld) = history_for(stored, external);
        self.assistant.cancel.store(false, Ordering::SeqCst);
        let info = (self.provider.kind(), self.provider.model().to_owned());
        Ok(AssistantJob {
            conversation_id,
            question: question.to_owned(),
            view: View::parse(view),
            history,
            history_withheld,
            route,
            gate: Arc::clone(&self.assistant.gate),
            cancel: Arc::clone(&self.assistant.cancel),
            info,
            guard,
        })
    }

    /// Pide cancelar el turno en curso: se comprueba entre pasos. Sin turno en curso no hace
    /// nada.
    pub fn assistant_cancel(&self) {
        if self.assistant.busy.load(Ordering::SeqCst) {
            self.assistant.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Cierra el turno (con el cerrojo): valida las citas, guarda la conversación y audita. Un
    /// error o una cancelación guardan el turno (con su resultado y la pregunta) solo si llegó a
    /// salir alguna llamada, para que «Ver lo enviado» la muestre (D183); ese turno no entra en el
    /// historial que se envía después ni en la validación de citas.
    pub fn finish_assistant_turn(&mut self, run: AssistantRun) -> AppResult<AssistantTurn> {
        let AssistantRun {
            conversation_id,
            question,
            status,
            answer,
            citation_refs,
            known_refs,
            steps,
            tools,
            navigate,
            withheld,
            calls,
            provider,
            model,
            local,
            fallback,
            _guard,
        } = run;
        let mut turn = AssistantTurn {
            conversation_id,
            turn_id: None,
            outcome: Outcome::Error,
            error_code: None,
            answer: String::new(),
            citations: Vec::new(),
            dropped_citations: 0,
            withheld,
            navigate,
            provider,
            model,
            local,
            fallback,
            steps,
            tools,
            words: None,
        };
        let audit_status = match status {
            RunStatus::Answered => {
                // Solo valen las referencias que una herramienta devolvió en este turno.
                let mut ids = Vec::new();
                let mut dropped = 0;
                for reference in &citation_refs {
                    match doc_id(reference).filter(|_| known_refs.contains(reference)) {
                        Some(id) if !ids.contains(&id) => ids.push(id),
                        Some(_) => {}
                        None => dropped += 1,
                    }
                }
                let cards = self.store.search_hits(&ids)?;
                // Una referencia válida que ya no está en el índice tampoco se cita.
                dropped += ids.len() - cards.len();
                turn.outcome = Outcome::Answered;
                turn.answer = answer;
                turn.dropped_citations = dropped;
                turn.citations = cards;
                if turn.fallback.is_some() {
                    "fallback"
                } else {
                    "answered"
                }
            }
            RunStatus::ConsentRequired | RunStatus::NotConfigured => {
                let consent = status == RunStatus::ConsentRequired;
                turn.outcome = if consent {
                    Outcome::ConsentRequired
                } else {
                    Outcome::NotConfigured
                };
                turn.answer = if consent {
                    "El asistente no puede usar el proveedor de IA externo sin su permiso: active en Configuración «Permitir que el asistente envíe datos fuera de este equipo» o prepare la IA local. Mientras tanto, le muestro una búsqueda por las palabras de su pregunta, hecha en este equipo y sin IA.".to_owned()
                } else {
                    let why = self
                        .ai_wait_message(None)
                        .unwrap_or_else(|| "Falta configurar la IA en Configuración.".to_owned());
                    format!("El asistente no está disponible. {why} Mientras tanto, le muestro una búsqueda por las palabras de su pregunta, hecha en este equipo y sin IA.")
                };
                // Si la búsqueda falla, el turno sigue: el motivo ya está en `answer`.
                turn.words = self.search_question(&question).ok();
                if consent {
                    "consent_required"
                } else {
                    "not_configured"
                }
            }
            RunStatus::Cancelled => {
                turn.outcome = Outcome::Cancelled;
                turn.answer = "Consulta cancelada.".to_owned();
                "cancelled"
            }
            RunStatus::Error(code) => {
                turn.outcome = Outcome::Error;
                turn.answer = error_message(&code);
                turn.error_code = Some(code);
                "error"
            }
        };
        // Guarda lo que la persona puede seguir leyendo (una respuesta o su motivo) y, aunque no
        // sea una respuesta, todo turno del que salió alguna llamada.
        if matches!(
            turn.outcome,
            Outcome::Answered | Outcome::ConsentRequired | Outcome::NotConfigured
        ) || !calls.is_empty()
        {
            let meta = TurnMeta {
                outcome: Some(
                    match turn.outcome {
                        Outcome::Answered => "answered",
                        Outcome::ConsentRequired => "consent_required",
                        Outcome::NotConfigured => "not_configured",
                        Outcome::Cancelled => "cancelled",
                        Outcome::Error => "error",
                    }
                    .to_owned(),
                ),
                citation_refs: turn
                    .citations
                    .iter()
                    .map(|c| format!("doc:{}", c.task_id))
                    .collect(),
                dropped_citations: turn.dropped_citations,
                withheld: turn.withheld,
                navigate: turn.navigate.clone(),
                provider: Some(turn.provider.to_string()),
                model: Some(turn.model.clone()),
                local: turn.local,
                fallback: turn.fallback.clone(),
                steps: turn.steps,
                tools: turn.tools.clone(),
                error_code: turn.error_code.clone(),
            };
            let saved = self.store.assistant_save_exchange(
                conversation_id,
                &question,
                &turn.answer,
                &json!(meta),
                &calls,
            )?;
            turn.conversation_id = Some(saved.conversation_id);
            turn.turn_id = Some(saved.turn_id);
        }
        // Los hechos, sin el texto de la pregunta ni de la respuesta.
        self.store.record_note(
            "assistant_turn",
            audit_status,
            &json!({
                "steps": turn.steps,
                "tools": turn.tools,
                "provider": turn.provider,
                "model": turn.model,
                "local": turn.local,
                "fallback": turn.fallback.as_ref().and_then(|f| f["reason"].as_str()),
                "citations": turn.citations.len(),
                "dropped_citations": turn.dropped_citations,
                "withheld": turn.withheld,
                "error_code": turn.error_code,
            }),
        )?;
        Ok(turn)
    }

    /// Una conversación guardada con sus turnos (la indicada o, sin indicar, la última).
    pub fn assistant_conversation(
        &self,
        id: Option<i64>,
    ) -> AppResult<Option<AssistantConversation>> {
        let stored = match id {
            Some(id) => self.store.assistant_conversation(id)?,
            None => self.store.assistant_last_conversation()?,
        };
        let Some(stored) = stored else {
            return Ok(None);
        };
        let mut turns = Vec::new();
        for turn in stored.turns {
            let meta: TurnMeta = serde_json::from_value(turn.meta).unwrap_or_default();
            let ids: Vec<i64> = meta
                .citation_refs
                .iter()
                .filter_map(|r| doc_id(r))
                .collect();
            let citations = self.store.search_hits(&ids)?;
            turns.push(ConversationTurn {
                id: turn.id,
                role: turn.role,
                text: turn.text,
                created_at: turn.created_at,
                meta,
                citations,
            });
        }
        Ok(Some(AssistantConversation {
            id: stored.id,
            created_at: stored.created_at,
            updated_at: stored.updated_at,
            turns,
        }))
    }

    /// Lo que se envió en las llamadas de un turno, tal cual, guardado solo en el equipo.
    pub fn assistant_sent(&self, turn_id: i64) -> AppResult<Vec<SentCall>> {
        Ok(self.store.assistant_sent(turn_id)?)
    }

    /// Borra una conversación (con sus turnos y llamadas) o todas. Devuelve cuántas.
    pub fn assistant_clear(&mut self, id: Option<i64>) -> AppResult<usize> {
        let removed = match id {
            Some(id) => usize::from(self.store.assistant_delete(id)?),
            None => self.store.assistant_delete_all()?,
        };
        self.store.record_note(
            "assistant_cleared",
            "cleared",
            &json!({ "scope": if id.is_some() { "one" } else { "all" }, "conversations": removed }),
        )?;
        Ok(removed)
    }
}
