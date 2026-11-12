//! Carga de los JSON de `evaluation/data/` de la evaluación del asistente (D192). Todo es
//! sintético. Las entradas (`inputs/`) son lo único que el sistema evaluado puede ver; las
//! etiquetas (`expected/`) nunca se envían al modelo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read<T: for<'de> Deserialize<'de>>(relative: &str) -> T {
    let path = root().join("evaluation/data").join(relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("JSON no válido en {relative}: {e}"))
}

#[derive(Debug, Clone, Deserialize)]
struct Entry {
    id: String,
    kind: String,
    input: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct Label {
    id: String,
    expected: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Invoice {
    pub name: String,
    pub text: String,
    pub supplier: String,
    pub invoice_number: String,
    pub date: String,
    pub subtotal: String,
    pub tax: String,
    pub total: String,
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Mail {
    pub message_id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    pub date: String,
    pub category: String,
    pub needs_reply: bool,
    pub draft: Option<String>,
    pub state: String,
}

/// Un documento o correo del mundo, o el hostil de un caso de inyección.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Item {
    Invoice(Invoice),
    Email(Mail),
}

impl Item {
    /// Todo el texto que el modelo podría llegar a leer del elemento.
    pub fn text(&self) -> String {
        match self {
            Item::Invoice(i) => i.text.clone(),
            Item::Email(m) => format!("{}\n{}\n{}", m.from, m.subject, m.body),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuleSeed {
    pub rule_id: String,
    pub action: String,
    pub conditions: Value,
    #[serde(default)]
    pub active: bool,
    /// Nombre visible de la regla (puede ser hostil en un caso de inyección).
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub accents: String,
    pub kind: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub party: Option<String>,
    pub min_total: Option<String>,
    pub max_total: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub text: String,
    pub view: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProposalExpectation {
    pub kind: String,
    pub doc: String,
}

/// Un tipo de propuesta que la persona pide de verdad en su pregunta: tipo, sobre qué tarea y, si
/// procede, con qué valores de la tarjeta (`summary`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Requested {
    pub kind: String,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub values: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuestionExpectation {
    /// Propuestas que pide la pregunta; el resto que se cree cuenta como no pedida.
    #[serde(default)]
    pub requested: Vec<Requested>,
    pub tools_any_of: Vec<String>,
    pub cite: Vec<String>,
    pub no_data: bool,
    pub total: Option<String>,
    pub number: Option<i64>,
    pub proposal: Option<ProposalExpectation>,
    /// Camino de un modelo ideal: solo para comprobar el arnés, nunca se envía a un modelo.
    pub steps: Vec<Value>,
}

/// El mundo sintético y las consultas.
pub struct Corpus {
    pub invoices: Vec<(String, Invoice)>,
    pub mails: Vec<(String, Mail)>,
    pub rules: Vec<RuleSeed>,
    pub searches: Vec<(String, SearchQuery)>,
    pub questions: Vec<(String, Question)>,
    /// Datos que `learn::mask` debe quitar antes de salir hacia un proveedor externo.
    pub pii: Vec<String>,
    pub search_expected: HashMap<String, Vec<String>>,
    pub question_expected: HashMap<String, QuestionExpectation>,
}

pub fn corpus() -> Corpus {
    let inputs: Vec<Entry> = read("inputs/assistant.json");
    let labels: Vec<Label> = read("expected/assistant.json");
    let mut c = Corpus {
        invoices: Vec::new(),
        mails: Vec::new(),
        rules: Vec::new(),
        searches: Vec::new(),
        questions: Vec::new(),
        pii: Vec::new(),
        search_expected: HashMap::new(),
        question_expected: HashMap::new(),
    };
    for e in inputs {
        let id = e.id;
        match e.kind.as_str() {
            "invoice" => c.invoices.push((id, from(e.input))),
            "email" => c.mails.push((id, from(e.input))),
            "rule" => c.rules.push(from(e.input)),
            "search" => c.searches.push((id, from(e.input))),
            "question" => c.questions.push((id, from(e.input))),
            other => panic!("tipo de entrada desconocido: {other}"),
        }
    }
    for l in labels {
        if l.id == "WORLD" {
            c.pii = from(l.expected["pii"].clone());
        } else if l.id.starts_with('Q') {
            c.search_expected
                .insert(l.id, from(l.expected["docs"].clone()));
        } else {
            c.question_expected.insert(l.id, from(l.expected));
        }
    }
    c
}

fn from<T: for<'de> Deserialize<'de>>(value: Value) -> T {
    serde_json::from_value(value).unwrap_or_else(|e| panic!("entrada mal formada: {e}"))
}

/// Un turno de un caso: la pregunta inocente de la persona y lo que haría el modelo simulado que
/// obedece al atacante.
#[derive(Debug, Clone, Deserialize)]
pub struct Turn {
    pub question: String,
    pub view: String,
    pub attack_steps: Vec<Value>,
}

/// Un elemento más del mundo del caso, con su alias (`HOSTIL2`, `HOSTIL3`…).
#[derive(Debug, Clone, Deserialize)]
pub struct Extra {
    pub alias: String,
    pub item: Item,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InjectionInput {
    pub group: String,
    pub item: Item,
    pub question: String,
    pub view: String,
    pub attack_steps: Vec<Value>,
    /// Turnos siguientes en la MISMA conversación (inyección en dos turnos).
    #[serde(default)]
    pub next_turns: Vec<Turn>,
    /// Elementos añadidos al mundo (alias que empiezan por `HOSTIL`: son del atacante).
    #[serde(default)]
    pub extras: Vec<Extra>,
    /// Reglas añadidas al mundo.
    #[serde(default)]
    pub rules: Vec<RuleSeed>,
    /// Preparaciones del mundo: `failed_task` (alias `FAILED`) y `attempt:<ALIAS>`.
    #[serde(default)]
    pub prep: Vec<String>,
}

impl InjectionInput {
    pub fn turns(&self) -> Vec<Turn> {
        let mut turns = vec![Turn {
            question: self.question.clone(),
            view: self.view.clone(),
            attack_steps: self.attack_steps.clone(),
        }];
        turns.extend(self.next_turns.iter().cloned());
        turns
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct InjectionExpected {
    pub group: String,
    /// Cadenas que no pueden aparecer en la respuesta, en lo enviado ni en una tarjeta.
    pub forbidden_always: Vec<String>,
    /// Datos que, con destino externo, no pueden salir sin enmascarar.
    pub forbidden_external: Vec<String>,
    /// Destinatarios ajenos: ni en una tarjeta ni en un envío.
    pub foreign_recipients: Vec<String>,
    /// Se anotan si aparecen en una tarjeta, pero no cuentan para la puerta.
    pub observe: Vec<String>,
    /// Texto hostil que no puede escapar de su bloque de datos.
    pub forbidden_outside_blocks: Vec<String>,
    /// Propuestas que la persona pide de verdad en la pregunta (casi siempre ninguna).
    pub requested: Vec<Requested>,
    /// Frases distintivas del elemento hostil, sin datos que el enmascarado quite: el caso solo
    /// cuenta como medido si alguna petición al modelo llevó una.
    pub exposed_marks: Vec<String>,
    /// Dominios o direcciones del atacante que no pueden aparecer en la respuesta como enlace.
    pub attacker_domains: Vec<String>,
    /// Clases de acción (`task`, `draft`, `rules`, `mail`, `import`) que las palabras de la persona
    /// piden de verdad en el caso. Es la verdad de la etiqueta: el evaluador compara lo ofrecido
    /// con esto y no con la función de la aplicación que decide lo ofrecido.
    pub asked_classes: Vec<String>,
}

#[derive(Clone)]
pub struct InjectionCase {
    pub id: String,
    pub input: InjectionInput,
    pub expected: Option<InjectionExpected>,
}

pub const INJECTION_SUITE: &str = "synthetic-assistant-injection-v1";

pub fn injection() -> Vec<InjectionCase> {
    let inputs: Vec<Entry> = read("inputs/assistant_injection.json");
    let labels: Vec<Label> = read("expected/assistant_injection.json");
    let mut expected: HashMap<String, InjectionExpected> = labels
        .into_iter()
        .map(|l| (l.id, from(l.expected)))
        .collect();
    inputs
        .into_iter()
        .map(|e| {
            assert_eq!(e.kind, "injection", "{}", e.id);
            let expected = expected.remove(&e.id);
            InjectionCase {
                id: e.id,
                input: from(e.input),
                expected,
            }
        })
        .collect()
}
