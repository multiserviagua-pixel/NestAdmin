//! Vista previa, consentimiento y llamada de la fase «Aprender a responder» (E3, D169–D170, R30,
//! T33b, T33c, T33h). Parte de lo recogido en local ([`Collected`](crate::learn::Collected)) y
//! hace tres cosas, todas sin persistir cuerpos ni el texto que sale:
//!
//! - [`App::learning_preview`]: el texto literal que saldría, el destino, la huella `input_fp` y
//!   los recuentos. Es lo que ve la persona antes de consentir.
//! - Consentimiento (`rule_settings.first_learning_consent` = `{provider, model, host, input_fp,
//!   at}`), con el patrón de `RefineExpectation`: ligado a proveedor, modelo, servidor **y** a lo
//!   que se envía. Con la IA local no hace falta (`not_needed`). Se retira al cambiar de destino.
//! - [`App::run_learning`]: una sola llamada con esquema cerrado. Con un destino externo, con
//!   consentimiento vigente para esa huella y la elección «Enviar», va al proveedor elegido y,
//!   si falla **con `unavailable`, `timeout` o `rate_limited`**, a la IA local, solo en las
//!   condiciones de D101 (preparada, `local_fallback`, periodo de 10 minutos). Una respuesta no
//!   válida o cualquier otro fallo (`unauthorized`…) dejan el aprendizaje sin hacer, con el
//!   código; la persona puede repetirlo con «Usar IA local» (elección suya, no respaldo). Sin
//!   consentimiento, con «Usar IA local» o con la IA local como destino, solo a la local; esa
//!   elección usa Ollama si está preparado aunque el respaldo automático esté desactivado
//!   (`local_ai`, independiente de `local_fallback`). Nunca de local a externa ni a otro
//!   proveedor externo. Sin ninguna IA, el resultado es `Unavailable` con los recuentos.
//! - El consentimiento **se consume al pasar a `sent`** ([`App::begin_learning`]): repetir la
//!   llamada no reenvía sin un nuevo consentimiento. Y se comprueba **otra vez justo antes de
//!   llamar al externo** ([`LearnJob::run`], con un registro compartido): si se retiró entre
//!   `begin` y `run`, no se envía nada y se usa la IA local.
//! - Un fallo `unauthorized`, 402 o 404 fija la espera de IA como en las demás tareas
//!   ([`LearnRun::wait`]).
//!
//! Estado de la fase, reanudación e interfaz son de E4 y E7: aquí no se escribe nada salvo el
//! consentimiento (con constancia en el historial, sin contenido).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use nestadmin_ai::learn::{self as ai, Learned};
use nestadmin_ai::{GenerateRequest, Provider, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::learn::{input_fp, is_masked, style_line_confirmed, Collected, LearnCounts};
use crate::refine::{now_ms, ConsentView, RefineExpectation};
use crate::{
    generate_with_fallback, AiWait, App, AppError, AppResult, LocalFallback, ProviderInfo,
};

/// Ajuste (`rule_settings`) con el consentimiento de la fase de aprendizaje.
const CONSENT_KEY: &str = "first_learning_consent";

/// Lo que la persona vio en la vista previa: mismo contenido que la expectativa del afinado.
pub type LearnExpectation = RefineExpectation;

/// Qué elige la persona ante la vista previa.
#[derive(Debug, Clone)]
pub enum LearnChoice {
    /// «Enviar a <proveedor>», con lo que vio. Solo sale si el consentimiento sigue vigente.
    Send(LearnExpectation),
    /// «Usar IA local»: nada sale del equipo.
    UseLocal,
}

#[derive(Debug, Clone, Deserialize)]
struct StoredConsent {
    provider: String,
    model: String,
    #[serde(default)]
    host: String,
    input_fp: String,
    at: i64,
}

/// Qué IA respondió.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnSource {
    External,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnStatus {
    /// Hay una respuesta válida (`learned` y `source`).
    Learned,
    /// Ninguna IA respondió con una respuesta válida: «sin aprender», con recuentos.
    Unavailable,
}

/// Resultado de la llamada. Sin `Serialize` de `learned`: es contenido derivado de correos. Su
/// `Debug` no muestra lo aprendido.
#[derive(Clone)]
pub struct LearnRun {
    pub status: LearnStatus,
    pub source: Option<LearnSource>,
    pub learned: Option<Learned>,
    pub counts: LearnCounts,
    /// Por qué no se llamó al externo estando configurado: `no_consent`, `use_local`, `consent_withdrawn`.
    pub external_skipped: Option<&'static str>,
    /// Código del fallo (o de la respuesta no válida) de la llamada externa, si la hubo.
    pub external_failure: Option<String>,
    /// Código del fallo de la IA local, o `no_local_ai` si no había ninguna preparada.
    pub local_failure: Option<String>,
    /// Espera de IA que impone el fallo del externo (`unauthorized`, 404, 402): quien cierra la
    /// llamada la fija, como las demás tareas (D101, D146).
    pub wait: Option<AiWait>,
    /// Proveedor externo al que se llamó: si ya no es el activo, la espera no se vuelve a poner.
    pub used: Option<Arc<dyn Provider>>,
}

impl std::fmt::Debug for LearnRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearnRun")
            .field("status", &self.status)
            .field("source", &self.source)
            .field(
                "learned_pairs",
                &self.learned.as_ref().map(|l| l.pairs.len()),
            )
            .field("counts", &self.counts)
            .field("external_skipped", &self.external_skipped)
            .field("external_failure", &self.external_failure)
            .field("local_failure", &self.local_failure)
            .field("wait", &self.wait)
            .finish()
    }
}

/// Lo que se enviaría y a quién, para mostrarlo antes de consentir o de pedir el aprendizaje. Su
/// `Debug` no muestra el texto: solo cifras, huella y destino.
#[derive(Clone, Serialize)]
pub struct LearningPreview {
    pub provider: ProviderInfo,
    /// Servidor (host) real al que saldrían los datos; `None` con la IA local.
    pub host: Option<String>,
    /// Huella del texto exacto: la llamada rechaza si ya no coincide.
    pub input_fp: String,
    /// El proveedor es externo: los datos salen del equipo.
    pub external: bool,
    /// Con un externo, si un fallo se repetiría con la IA local si está preparada (D101).
    pub local_fallback: bool,
    /// Si hay una espera de IA, nada se enviaría al externo.
    pub blocked: Option<AiWait>,
    pub consent: ConsentView,
    /// Texto literal que viajaría (los pares entre marcadores, con alias y enmascarado).
    pub payload: String,
    pub pairs: usize,
    pub counts: LearnCounts,
    /// Qué no viaja nunca.
    pub never_sent: Vec<&'static str>,
    /// Qué puede viajar aunque se intente evitar: lo que la vista previa muestra y la persona
    /// aprueba.
    pub may_travel: Vec<&'static str>,
}

impl std::fmt::Debug for LearningPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearningPreview")
            .field("provider", &self.provider)
            .field("host", &self.host)
            .field("input_fp", &self.input_fp)
            .field("external", &self.external)
            .field("local_fallback", &self.local_fallback)
            .field("blocked", &self.blocked)
            .field("consent", &self.consent)
            .field("payload_chars", &self.payload.chars().count())
            .field("pairs", &self.pairs)
            .field("counts", &self.counts)
            .finish()
    }
}

/// Registro compartido de huellas con consentimiento vigente: lo escriben la concesión y
/// `begin_learning` y lo vacían la retirada y el cambio de destino. El trabajo lo comprueba, y lo
/// consume, justo antes de llamar al externo.
pub(crate) type ConsentGate = Arc<Mutex<HashSet<String>>>;

/// De dónde sale la IA local de una llamada.
enum LocalSource {
    /// El propio destino configurado es la IA local.
    Active(Arc<dyn Provider>),
    /// Un resolvedor de la IA local (el independiente de `local_fallback` o el respaldo de D101):
    /// se resuelve al llamar y solo vale si es Ollama.
    Resolver(LocalFallback),
    None,
}

/// Llamada al proveedor externo elegido, con el respaldo de D101.
struct External {
    provider: Arc<dyn Provider>,
    fallback: Option<LocalFallback>,
    /// Periodo de D101: tras un `unavailable` reciente, las llamadas van directas a la local.
    until: Arc<Mutex<Option<Instant>>>,
}

/// Llamada preparada: no toca `App`, se puede ejecutar en otro hilo.
pub struct LearnJob {
    /// Proveedor externo al que se llamará; `None` si no hay que llamarlo.
    external: Option<External>,
    /// Huellas con consentimiento vigente (se comprueba otra vez antes de llamar al externo).
    gate: ConsentGate,
    input_fp: String,
    /// Líneas confirmadas por dos o más respuestas: filtran el saludo y la despedida (M3).
    style_lines: HashSet<String>,
    /// IA local para el camino sin externo («Usar IA local», sin consentimiento o destino local).
    local: LocalSource,
    request: GenerateRequest,
    pair_ids: Vec<String>,
    counts: LearnCounts,
    external_skipped: Option<&'static str>,
}

impl LearnJob {
    /// Con el externo: una llamada con el respaldo de D101 (`generate_with_fallback`): solo
    /// `unavailable`, `timeout` o `rate_limited` repiten con la IA local si está preparada, y
    /// durante el periodo de 10 minutos tras un `unavailable` va directa a ella. Cualquier otro
    /// fallo (`unauthorized`, 402, 404, petición rechazada) o una respuesta no válida dejan el
    /// aprendizaje sin hacer, con el código, sin llamar a la local. Sin externo: solo la local.
    /// Nunca de la local a un externo.
    pub fn run(self) -> LearnRun {
        let LearnJob {
            external,
            gate,
            input_fp,
            style_lines,
            local,
            request,
            pair_ids,
            counts,
            mut external_skipped,
        } = self;
        // Justo antes de enviar: si el consentimiento se retiró desde que se preparó la llamada,
        // no sale nada. La huella se consume aquí: una sola llamada externa por consentimiento.
        let had_external = external.is_some();
        let external = external.filter(|_| {
            gate.lock()
                .map(|mut live| live.remove(&input_fp))
                .unwrap_or(false)
        });
        if had_external && external.is_none() {
            // Retirar el consentimiento cancela el aprendizaje (D170): no se envía y tampoco se
            // pasa a la IA local; la fase vuelve a esperar a la persona.
            external_skipped = Some("consent_withdrawn");
            return LearnRun {
                status: LearnStatus::Unavailable,
                source: None,
                learned: None,
                counts,
                external_skipped,
                external_failure: None,
                local_failure: None,
                wait: None,
                used: None,
            };
        }
        // `is_masked` es verdadero si no queda nada que enmascarar: lo contrario es «inseguro».
        // Saludo y despedida solo valen si coinciden con una línea de dos o más respuestas propias.
        let validate = |value: &serde_json::Value| -> Result<Learned, nestadmin_ai::AiError> {
            let mut learned =
                ai::parse_learn_value_checked(value, &pair_ids, &|text| !is_masked(text))?;
            for field in [&mut learned.style.greeting, &mut learned.style.closing] {
                *field = field
                    .take()
                    .filter(|text| style_line_confirmed(text, &style_lines));
            }
            Ok(learned)
        };
        let learned = |source, learned, external_failure| LearnRun {
            status: LearnStatus::Learned,
            source: Some(source),
            learned: Some(learned),
            counts: counts.clone(),
            external_skipped,
            external_failure,
            local_failure: None,
            wait: None,
            used: None,
        };
        let unavailable = |external_failure, local_failure, wait, used| LearnRun {
            status: LearnStatus::Unavailable,
            source: None,
            learned: None,
            counts: counts.clone(),
            external_skipped,
            external_failure,
            local_failure,
            wait,
            used,
        };
        if let Some(external) = external {
            let used = Arc::clone(&external.provider);
            let attempt = generate_with_fallback(
                &*external.provider,
                external.fallback.as_ref(),
                &external.until,
                &request,
            );
            let wait = attempt.wait;
            let served_by_local = attempt.fallback.is_some();
            let reason = attempt
                .fallback
                .as_ref()
                .and_then(|f| f["reason"].as_str())
                .map(str::to_owned);
            let outcome = attempt
                .result
                .map_err(|e| e.code)
                .and_then(|r| validate(&r.output).map_err(|e| e.code().to_owned()));
            return match (outcome, served_by_local) {
                (Ok(l), false) => learned(LearnSource::External, l, None),
                (Ok(l), true) => learned(LearnSource::Local, l, reason),
                (Err(code), false) => unavailable(Some(code), None, wait, Some(used)),
                (Err(code), true) => unavailable(reason, Some(code), None, Some(used)),
            };
        }
        let provider = match local {
            LocalSource::Active(p) => Some(p),
            LocalSource::Resolver(f) => f().filter(|p| p.kind() == ProviderKind::Ollama),
            LocalSource::None => None,
        };
        let Some(provider) = provider else {
            return unavailable(None, Some("no_local_ai".to_owned()), None, None);
        };
        match provider
            .generate(&request)
            .map_err(|e| e.code().to_owned())
            .and_then(|r| validate(&r.output).map_err(|e| e.code().to_owned()))
        {
            Ok(l) => learned(LearnSource::Local, l, None),
            Err(code) => unavailable(None, Some(code), None, None),
        }
    }
}

impl App {
    fn learning_external(&self) -> bool {
        self.provider.kind() != ProviderKind::Ollama
    }

    /// Consentimiento de la fase frente al destino de ahora y a la huella de lo que se enviaría.
    pub fn learning_consent(&self, input_fp: &str) -> AppResult<ConsentView> {
        if !self.learning_external() {
            return Ok(ConsentView {
                required: false,
                state: "not_needed",
                provider: None,
                model: None,
                host: None,
                at: None,
            });
        }
        let (provider, model) = self.asked();
        let host = self.host();
        let view =
            |state: &'static str, p: String, m: String, h: String, at: Option<i64>| ConsentView {
                required: true,
                state,
                provider: Some(p),
                model: Some(m),
                host: Some(h).filter(|h| !h.is_empty()),
                at,
            };
        let stored = self
            .store
            .rule_setting(CONSENT_KEY)?
            .and_then(|json| serde_json::from_str::<StoredConsent>(&json).ok());
        Ok(match stored {
            None => view("missing", provider, model, host, None),
            // Proveedor, modelo, servidor y contenido: cualquier cambio lo caduca.
            Some(c)
                if c.provider == provider
                    && c.model == model
                    && c.host == host
                    && c.input_fp == input_fp =>
            {
                view("granted", c.provider, c.model, c.host, Some(c.at))
            }
            Some(c) => view("stale", c.provider, c.model, c.host, Some(c.at)),
        })
    }

    /// Retira el consentimiento si el proveedor, el modelo o el servidor ya no son los suyos
    /// (como el del afinado, en lugar de dejarlo dormido). Se llama al cambiar de proveedor.
    pub(crate) fn drop_learning_consent_if_destination_changed(&mut self) {
        let Ok(Some(json)) = self.store.rule_setting(CONSENT_KEY) else {
            return;
        };
        let (provider, model) = self.asked();
        let same = serde_json::from_str::<StoredConsent>(&json)
            .is_ok_and(|c| c.provider == provider && c.model == model && c.host == self.host());
        if !same {
            let _ = self.store.delete_rule_setting(
                CONSENT_KEY,
                "consentimiento de aprender a responder retirado: cambió el proveedor, el modelo o el servidor",
            );
            self.clear_learning_gate();
        }
    }

    /// Vacía el registro de consentimientos vigentes: un trabajo ya preparado no enviará nada.
    fn clear_learning_gate(&self) {
        if let Ok(mut live) = self.learning.gate.lock() {
            live.clear();
        }
    }

    /// Resolvedor de la IA local para la elección explícita «Usar IA local»: el independiente del
    /// respaldo (`local_ai`) o, si no lo hay, el del respaldo de D101. Es una elección de la
    /// persona, no un respaldo: no depende de `local_fallback`.
    fn local_resolver(&self) -> LocalSource {
        self.learning
            .local_ai
            .clone()
            .or_else(|| self.fallback.clone())
            .map_or(LocalSource::None, LocalSource::Resolver)
    }

    /// Lo que saldría y a quién. Solo lectura: no llama al proveedor.
    pub fn learning_preview(&self, collected: &Collected) -> AppResult<LearningPreview> {
        let external = self.learning_external();
        Ok(LearningPreview {
            provider: self.provider(),
            host: self.provider.destination(),
            input_fp: collected.input_fp.clone(),
            external,
            local_fallback: external && self.fallback.is_some(),
            blocked: self.ai_wait,
            consent: self.learning_consent(&collected.input_fp)?,
            payload: collected.payload.clone(),
            pairs: collected.pairs.len(),
            counts: collected.counts.clone(),
            never_sent: vec![
                "direcciones de correo, y el nombre y la parte local de la dirección del remitente (se sustituyen por alias R1, R2…)",
                "IBAN, tarjetas, claves, teléfonos, DNI, NIE, CIF y enlaces (enmascarados)",
                "la firma de las respuestas (se detecta en local; si queda una línea con aspecto de firma, el par no sale)",
                "el texto citado de los hilos (si no se reconoce, el par no sale)",
                "respuestas que no iban dirigidas al remitente (reenvíos internos)",
                "adjuntos y documentos",
                "credenciales y tokens",
            ],
            may_travel: vec![
                "otros nombres propios (de terceros o de la empresa) que aparezcan en el texto",
                "direcciones postales y cualquier dato que no tenga un formato reconocible",
                "lo que ve en esta vista previa es exactamente lo que se enviaría",
            ],
        })
    }

    /// Lo recogido debe ser coherente: texto no vacío y huella calculada sobre ese mismo texto.
    fn check_collected(collected: &Collected) -> AppResult<()> {
        if collected.pairs.is_empty() || collected.payload.is_empty() {
            return Err(AppError::Invalid(
                "No hay pares que enviar: no hay nada que aprender.".into(),
            ));
        }
        if input_fp(&collected.payload) != collected.input_fp {
            return Err(AppError::Invalid(
                "Lo recogido ha cambiado: vuelva a generar la vista previa.".into(),
            ));
        }
        Ok(())
    }

    fn check_expectation(&self, collected: &Collected, e: &LearnExpectation) -> AppResult<()> {
        if self.asked() != (e.provider.clone(), e.model.clone()) || self.host() != e.host {
            return Err(AppError::Invalid(
                "El proveedor, el modelo o el servidor han cambiado desde que vio la vista previa: revísela de nuevo antes de enviar."
                    .into(),
            ));
        }
        if e.input_fp != collected.input_fp {
            return Err(AppError::Invalid(
                "Lo que se iba a enviar ha cambiado desde que vio la vista previa: revísela de nuevo antes de enviar."
                    .into(),
            ));
        }
        Ok(())
    }

    /// La persona consiente enviar lo que vio al proveedor, modelo y servidor que vio. Rechaza si
    /// algo cambió. Con la IA local no hace falta. Queda en el historial, sin contenido.
    pub fn grant_learning_consent(
        &mut self,
        collected: &Collected,
        expectation: &LearnExpectation,
    ) -> AppResult<ConsentView> {
        if !self.learning_external() {
            return Err(AppError::Invalid(
                "Con la IA local los correos no salen del equipo: no hace falta consentimiento."
                    .into(),
            ));
        }
        Self::check_collected(collected)?;
        self.check_expectation(collected, expectation)?;
        self.store.set_rule_setting(
            CONSENT_KEY,
            &json!({
                "provider": expectation.provider,
                "model": expectation.model,
                "host": expectation.host,
                "input_fp": expectation.input_fp,
                "at": now_ms(),
            })
            .to_string(),
            &format!(
                "consentimiento de aprender a responder concedido: proveedor {}, modelo {}, servidor {}, {} pares",
                expectation.provider,
                expectation.model,
                expectation.host,
                collected.pairs.len()
            ),
        )?;
        if let Ok(mut live) = self.learning.gate.lock() {
            live.insert(expectation.input_fp.clone());
        }
        self.learning_consent(&collected.input_fp)
    }

    /// Retira el consentimiento. Retirarlo dos veces no cambia nada. Una llamada ya preparada
    /// pero aún no enviada tampoco saldrá.
    pub fn withdraw_learning_consent(&mut self, input_fp: &str) -> AppResult<ConsentView> {
        self.store.delete_rule_setting(
            CONSENT_KEY,
            "consentimiento de aprender a responder retirado",
        )?;
        self.clear_learning_gate();
        self.learning_consent(input_fp)
    }

    /// Prepara la llamada. Rechaza, sin enviar nada, si lo recogido es incoherente, si con
    /// «Enviar» el proveedor, el modelo, el servidor o la huella ya no son los de la vista previa,
    /// o si hay una espera de IA y se iba a usar el externo. Con el externo, **consume** el
    /// consentimiento (queda `missing`): repetir no reenvía sin un consentimiento nuevo.
    pub fn begin_learning(
        &mut self,
        collected: &Collected,
        choice: &LearnChoice,
    ) -> AppResult<LearnJob> {
        Self::check_collected(collected)?;
        if let LearnChoice::Send(expectation) = choice {
            self.check_expectation(collected, expectation)?;
        }
        let request =
            ai::learn_request(&format!("learn-{}", collected.input_fp), &collected.payload);
        let pair_ids = collected.pairs.iter().map(|p| p.pair_id.clone()).collect();
        let (external, local, external_skipped) = if !self.learning_external() {
            // El destino configurado es la IA local: nunca hay llamada externa.
            (None, LocalSource::Active(Arc::clone(&self.provider)), None)
        } else {
            let local = self.local_resolver();
            match choice {
                LearnChoice::UseLocal => (None, local, Some("use_local")),
                LearnChoice::Send(_) => {
                    if self.learning_consent(&collected.input_fp)?.state != "granted" {
                        (None, local, Some("no_consent"))
                    } else if self.ai_wait.is_some() {
                        return Err(AppError::Invalid(
                            "La IA está en espera de configuración: corríjala antes de enviar, o elija usar la IA local."
                                .into(),
                        ));
                    } else {
                        // Se anota la huella para la comprobación de última hora y se consume el
                        // consentimiento guardado: esta llamada es la única que ampara.
                        if let Ok(mut live) = self.learning.gate.lock() {
                            live.insert(collected.input_fp.clone());
                        }
                        self.store.delete_rule_setting(
                            CONSENT_KEY,
                            "consentimiento de aprender a responder consumido: se envía lo que vio",
                        )?;
                        (
                            Some(External {
                                provider: Arc::clone(&self.provider),
                                fallback: self.fallback.clone(),
                                until: Arc::clone(&self.fallback_until),
                            }),
                            local,
                            None,
                        )
                    }
                }
            }
        };
        Ok(LearnJob {
            external,
            gate: Arc::clone(&self.learning.gate),
            input_fp: collected.input_fp.clone(),
            style_lines: collected.style_lines.clone(),
            local,
            request,
            pair_ids,
            counts: collected.counts.clone(),
            external_skipped,
        })
    }

    /// Prepara y ejecuta la llamada (ver [`LearnJob::run`]). Una llamada externa como mucho, y a
    /// lo sumo otra a la IA local.
    pub fn run_learning(
        &mut self,
        collected: &Collected,
        choice: &LearnChoice,
    ) -> AppResult<LearnRun> {
        Ok(self.begin_learning(collected, choice)?.run())
    }
}
