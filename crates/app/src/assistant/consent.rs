//! Consentimiento del asistente para enviar datos fuera del equipo (D183, R33): «Permitir que el
//! asistente envíe datos fuera de este equipo», desactivado por defecto. Sigue el patrón del
//! consentimiento del perfil (`profile.rs`): ligado a proveedor, modelo y servidor, auditado al
//! darlo y al retirarlo (lo hace `set_rule_setting`), retirado por `select_provider` y nunca
//! activable desde el asistente: no hay herramienta ni vía del asistente que lo conceda.

use std::sync::atomic::Ordering;

use nestadmin_ai::assistant::View;
use nestadmin_ai::ProviderKind;
use nestadmin_core::{SearchHit, SearchKind, SearchStatus, TaskState};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::tools::{search_value, Text};
use super::{
    compose, StoredEntry, MAX_DATA_CHARS, MAX_FRAGMENTS, MAX_FRAGMENT_CHARS, MAX_HISTORY_TURNS,
    MAX_STEPS, MAX_TURN_CHARS,
};
use crate::refine::now_ms;
use crate::{App, AppError, AppResult, ProviderInfo};

/// Ajuste (`rule_settings`) con el consentimiento del asistente.
const CONSENT_KEY: &str = "assistant_external_consent";

/// Aviso fijo de la vista previa (D183).
pub const ASSISTANT_NOTICE: &str = "La pregunta sale tal como usted la escribe y pueden viajar nombres propios y direcciones postales que aparezcan en los documentos. Los correos, teléfonos, IBAN, identificadores fiscales, claves, tarjetas y enlaces se sustituyen antes de salir. El dato completo se ve en la tarjeta del documento, que se pinta en este equipo.";

/// Estado del consentimiento frente al destino de ahora.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssistantConsentView {
    /// Solo hace falta con un proveedor externo.
    pub required: bool,
    /// `not_needed` (IA local), `missing`, `granted` o `stale` (cambió el destino).
    pub state: &'static str,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub host: Option<String>,
    pub at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct StoredConsent {
    provider: String,
    model: String,
    #[serde(default)]
    host: String,
    at: i64,
}

/// Topes de lo que sale en cada llamada (D183), para mostrarlos.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantLimits {
    pub steps: usize,
    pub history_turns: usize,
    pub turn_chars: usize,
    pub fragments: usize,
    pub fragment_chars: usize,
    pub data_chars: usize,
}

/// Lo que saldría en el asistente y a quién, para mostrarlo antes de consentir.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantPreview {
    pub provider: ProviderInfo,
    /// Servidor real al que saldría; `None` con la IA local.
    pub host: Option<String>,
    /// El proveedor es externo: el contenido saldría del equipo.
    pub external: bool,
    pub consent: AssistantConsentView,
    /// Hay respaldo automático a la IA local (D101).
    pub local_fallback: bool,
    /// Texto literal de un envío de ejemplo: lo que se enviaría para una pregunta de muestra con
    /// un resultado sintético, construido con las mismas funciones que el envío real.
    pub sample: String,
    pub limits: AssistantLimits,
    pub may_travel: Vec<&'static str>,
    pub never_sent: Vec<&'static str>,
    pub notice: &'static str,
}

/// Pregunta de muestra de la vista previa.
const SAMPLE_QUESTION: &str = "¿Qué factura tengo de Proveedor Ejemplo en octubre?";

fn sample_hit() -> SearchHit {
    SearchHit {
        task_id: 1,
        kind: SearchKind::Invoice,
        title: "Factura F-2026-001".to_owned(),
        party: "Proveedor Ejemplo SL".to_owned(),
        number: "F-2026-001".to_owned(),
        doc_date: Some("2026-10-05".to_owned()),
        total_minor: Some(12_100),
        currency: "EUR".to_owned(),
        task_state: TaskState::Completed,
        index_status: SearchStatus::Indexed,
        snippet:
            "Factura F-2026-001 de Proveedor Ejemplo SL por 121,00 EUR, fecha 5 de octubre de 2026"
                .to_owned(),
    }
}

impl App {
    fn assistant_destination(&self) -> (String, String, String, bool) {
        let (provider, model) = self.asked();
        let external = self.provider.kind() != ProviderKind::Ollama;
        (provider, model, self.host(), external)
    }

    /// Consentimiento frente al destino de ahora.
    pub fn assistant_consent(&self) -> AppResult<AssistantConsentView> {
        let (provider, model, host, external) = self.assistant_destination();
        if !external {
            return Ok(AssistantConsentView {
                required: false,
                state: "not_needed",
                provider: None,
                model: None,
                host: None,
                at: None,
            });
        }
        let view = |state, p: String, m: String, h: String, at| AssistantConsentView {
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
            Some(c) if c.provider == provider && c.model == model && c.host == host => {
                view("granted", c.provider, c.model, c.host, Some(c.at))
            }
            Some(c) => view("stale", c.provider, c.model, c.host, Some(c.at)),
        })
    }

    /// El envío de ejemplo, literal: instrucciones y contenido tal como saldrían.
    fn assistant_sample(&self) -> String {
        let text = Text::new(true);
        let hit = sample_hit();
        let (value, _) = search_value(&[hit], 1, (0, 0, 0), "all", &text);
        let entries = vec![StoredEntry::Tool {
            name: "search".to_owned(),
            args: json!({ "query": "Proveedor Ejemplo octubre" }).to_string(),
            value,
        }];
        let request = compose(
            "assistant-sample",
            View::Other,
            &[],
            SAMPLE_QUESTION,
            &entries,
        );
        format!("{}\n\n{}", request.instructions, request.content)
    }

    /// Qué saldría en el asistente y a quién. Solo lectura: no llama al proveedor.
    pub fn assistant_preview(&self) -> AppResult<AssistantPreview> {
        let (_, _, host, external) = self.assistant_destination();
        Ok(AssistantPreview {
            provider: self.provider(),
            host: Some(host).filter(|h| !h.is_empty()),
            external,
            consent: self.assistant_consent()?,
            local_fallback: external && self.fallback.is_some(),
            sample: self.assistant_sample(),
            limits: AssistantLimits {
                steps: MAX_STEPS,
                history_turns: MAX_HISTORY_TURNS,
                turn_chars: MAX_TURN_CHARS,
                fragments: MAX_FRAGMENTS,
                fragment_chars: MAX_FRAGMENT_CHARS,
                data_chars: MAX_DATA_CHARS,
            },
            may_travel: vec![
                "las instrucciones y la lista de herramientas del asistente",
                "su pregunta, tal como la escribe",
                "como mucho los 6 últimos turnos de la conversación (2.000 caracteres cada uno)",
                "los resultados de las herramientas de ese turno: hasta 8 fragmentos de 600 caracteres y, en total, 12.000 caracteres por llamada, con correos, teléfonos, IBAN, identificadores fiscales, claves, tarjetas y enlaces sustituidos por marcadores",
                "referencias opacas (doc:N, acct:N) en lugar de nombres de cuenta o direcciones de buzón",
            ],
            never_sent: vec![
                "archivos, imágenes y sus huellas o rutas",
                "nombres de cuenta y direcciones de buzón",
                "credenciales",
                "el historial de auditoría",
                "el perfil del negocio",
                "más turnos que los indicados",
            ],
            notice: ASSISTANT_NOTICE,
        })
    }

    /// La persona consiente enviar lo descrito al proveedor, modelo y servidor que vio. Rechaza si
    /// algo cambió desde la vista previa. Con la IA local no hace falta.
    pub fn grant_assistant_consent(
        &mut self,
        provider: &str,
        model: &str,
        host: &str,
    ) -> AppResult<AssistantConsentView> {
        let (p, m, h, external) = self.assistant_destination();
        if !external {
            return Err(AppError::Invalid(
                "Con la IA local el asistente no envía nada fuera del equipo: no hace falta consentimiento."
                    .into(),
            ));
        }
        if (p.as_str(), m.as_str(), h.as_str()) != (provider, model, host) {
            return Err(AppError::Invalid(
                "El proveedor, el modelo o el servidor han cambiado desde que lo vio: revise la vista previa de nuevo antes de aceptar."
                    .into(),
            ));
        }
        self.store.set_rule_setting(
            CONSENT_KEY,
            &json!({ "provider": provider, "model": model, "host": host, "at": now_ms() })
                .to_string(),
            &format!(
                "consentimiento del asistente concedido: proveedor {provider}, modelo {model}, servidor {host}"
            ),
        )?;
        self.assistant_consent()
    }

    /// Retira el consentimiento: el asistente deja de enviar nada a un proveedor externo, también
    /// el turno ya preparado que todavía no ha llamado.
    pub fn withdraw_assistant_consent(&mut self) -> AppResult<AssistantConsentView> {
        self.store
            .delete_rule_setting(CONSENT_KEY, "consentimiento del asistente retirado")?;
        self.assistant.gate.store(false, Ordering::SeqCst);
        self.assistant_consent()
    }

    /// Retira el consentimiento si el proveedor, el modelo o el servidor ya no son los suyos. Se
    /// llama al cambiar de proveedor, de modelo o de servidor (`select_provider`).
    pub(crate) fn drop_assistant_consent_if_destination_changed(&mut self) {
        let Ok(Some(json)) = self.store.rule_setting(CONSENT_KEY) else {
            return;
        };
        let (provider, model) = self.asked();
        let same = serde_json::from_str::<StoredConsent>(&json)
            .is_ok_and(|c| c.provider == provider && c.model == model && c.host == self.host());
        if !same {
            let _ = self.store.delete_rule_setting(
                CONSENT_KEY,
                "consentimiento del asistente retirado: cambió el proveedor, el modelo o el servidor",
            );
            self.assistant.gate.store(false, Ordering::SeqCst);
        }
    }
}
