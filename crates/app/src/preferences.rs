//! Preferencias por remitente (D161) y precedencia de un correo nuevo (D157, D161, D162).
//!
//! La persona fija con un clic explícito «Siempre a Informativos» o «Siempre a Pendientes» para
//! una dirección o un dominio; nada se aprende solo. Ninguna preferencia desbloquea una regla de
//! envío (D158): la compuerta de `rules.rs` no las mira. Cada cambio deja un evento del historial
//! sin contenido y, si contradice una regla activa, avisa sin bloquear.

use std::collections::HashMap;

use nestadmin_core::mail_signals::{
    decide, sender_parts, PreferenceRoute, Signal, TriageDecision, TriageSource,
};
use nestadmin_core::{normalize_key, PreferenceScope, SenderPreference};
use serde::Serialize;
use serde_json::Value;

use crate::rules::{is_public_mail_domain, SEND_DRAFT};
use crate::{App, AppError, AppResult};

/// Una regla activa de envío que la preferencia contradice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreferenceWarning {
    pub rule_id: String,
    pub rule_name: String,
}

/// Resultado de fijar una preferencia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreferenceChange {
    pub preference: SenderPreference,
    /// `false` si ya estaba así: no se escribió nada.
    pub changed: bool,
    /// Reglas activas de envío para ese remitente o dominio a las que dejaría sin efecto.
    pub warnings: Vec<PreferenceWarning>,
}

/// Preferencias cargadas para consultar por dirección y dominio.
#[derive(Debug, Clone, Default)]
pub struct PreferenceIndex {
    map: HashMap<(PreferenceScope, String), PreferenceRoute>,
}

impl PreferenceIndex {
    pub fn from_rows(rows: &[SenderPreference]) -> Self {
        let map = rows
            .iter()
            .filter_map(|p| {
                Some((
                    (PreferenceScope::from_code(&p.scope)?, p.key.clone()),
                    PreferenceRoute::from_code(&p.route)?,
                ))
            })
            .collect();
        Self { map }
    }

    /// Preferencia de un remitente: primero la de su dirección y, si no hay, la de su dominio.
    pub fn resolve(&self, from: Option<&str>) -> Option<(PreferenceRoute, PreferenceScope)> {
        let (address, domain) = sender_parts(from?)?;
        [
            (PreferenceScope::Address, address),
            (PreferenceScope::Domain, domain),
        ]
        .into_iter()
        .find_map(|(scope, key)| self.map.get(&(scope, key)).map(|route| (*route, scope)))
    }
}

/// Ruta de un correo nuevo con la precedencia completa de D161: rebote, hilo propio, preferencia
/// (dirección y luego dominio), señal fuerte, IA. La primera lectura (D133) se decide antes, en
/// `fetch_mail`: sus mensajes ya tienen tarea y no llegan aquí. `thread` se llama a lo sumo una
/// vez y nunca para un rebote; devuelve si el último mensaje del hilo es propio, o `None` si no
/// se sabe o falló la consulta (no tiene efecto). Devuelve también el ámbito de la preferencia
/// cuando fue ella quien decidió.
pub fn decide_new_mail(
    index: &PreferenceIndex,
    from: Option<&str>,
    signals: &[Signal],
    thread: impl FnOnce() -> Option<bool>,
) -> (TriageDecision, Option<PreferenceScope>) {
    let thread_own = if signals.iter().any(|s| s.is_bounce()) {
        None
    } else {
        thread()
    };
    let preference = index.resolve(from);
    let decision = decide(signals, thread_own, preference.map(|(route, _)| route));
    let scope = (decision.source == TriageSource::Preference)
        .then(|| preference.map(|(_, scope)| scope))
        .flatten();
    (decision, scope)
}

/// ¿Nombra la regla (por `sender` o `sender_domain`) a ese remitente o dominio? Una regla sin
/// ninguno de los dos (para cualquier remitente) no se cuenta: el aviso nombra a quien se verá
/// afectado.
fn rule_names(conditions: &Value, scope: PreferenceScope, key: &str) -> bool {
    let domain_of_key = match scope {
        PreferenceScope::Address => key.rsplit_once('@').map(|(_, d)| d).unwrap_or(""),
        PreferenceScope::Domain => key,
    };
    let rule_domain = conditions["sender_domain"]
        .as_str()
        .map(|d| d.trim().trim_start_matches('@').to_lowercase());
    let rule_sender = conditions["sender"].as_str().and_then(sender_parts);
    let by_domain = rule_domain.as_deref() == Some(domain_of_key);
    match scope {
        PreferenceScope::Address => {
            by_domain || rule_sender.is_some_and(|(address, _)| address == key)
        }
        PreferenceScope::Domain => by_domain || rule_sender.is_some_and(|(_, d)| d == key),
    }
}

impl App {
    /// Todas las preferencias, en orden estable.
    pub fn sender_preferences(&self) -> AppResult<Vec<SenderPreference>> {
        Ok(self.store.sender_preferences()?)
    }

    /// Fija «Siempre a Informativos» (`informative`) o «Siempre a Pendientes» (`review`) para una
    /// dirección (`address`) o un dominio (`domain`). La clave se normaliza en minúsculas
    /// (de `Nombre <a@b>` se toma la dirección). Un dominio de correo público no se admite: abarca
    /// a cualquier usuario del servicio. Deja un evento del historial sin contenido y avisa, sin
    /// bloquear, si contradice una regla activa de envío. Solo cambia los correos nuevos.
    pub fn set_sender_preference(
        &mut self,
        scope: &str,
        key: &str,
        route: &str,
    ) -> AppResult<PreferenceChange> {
        let (scope, key, route) = validated(scope, key, route)?;
        let previous = self.store.set_sender_preference(scope, &key, route)?;
        let warnings = if route == PreferenceRoute::Informative {
            self.preference_warnings(scope, &key)?
        } else {
            Vec::new()
        };
        let preference = self
            .store
            .sender_preferences()?
            .into_iter()
            .find(|p| p.scope == scope.as_str() && p.key == key)
            .ok_or_else(|| AppError::Invalid("la preferencia no se guardó".into()))?;
        Ok(PreferenceChange {
            preference,
            changed: previous != Some(route),
            warnings,
        })
    }

    /// Quita una preferencia (deja un evento del historial). Devuelve si existía.
    pub fn remove_sender_preference(&mut self, scope: &str, key: &str) -> AppResult<bool> {
        let scope = PreferenceScope::from_code(scope).ok_or_else(unknown_scope)?;
        let key = normalize_key(scope, key).ok_or_else(invalid_key)?;
        Ok(self.store.remove_sender_preference(scope, &key)?)
    }

    /// Reglas activas de envío (no archivadas) que nombran a ese remitente o dominio.
    fn preference_warnings(
        &self,
        scope: PreferenceScope,
        key: &str,
    ) -> AppResult<Vec<PreferenceWarning>> {
        Ok(self
            .rule_views()?
            .into_iter()
            .filter(|v| {
                v.rule.active
                    && v.archived_at.is_none()
                    && v.rule.action_kind == SEND_DRAFT
                    && rule_names(&v.rule.conditions, scope, key)
            })
            .map(|v| PreferenceWarning {
                rule_id: v.rule.id,
                rule_name: v.name,
            })
            .collect())
    }
}

fn unknown_scope() -> AppError {
    AppError::Invalid("ámbito desconocido: use «address» o «domain»".into())
}

fn invalid_key() -> AppError {
    AppError::Invalid("la dirección o el dominio no es válido".into())
}

/// Valida una preferencia como la fija `set_sender_preference`: ámbito y ruta conocidos, clave
/// normalizada y, por dominio, que no sea de un servicio de correo público. Sin efectos. La usa
/// también el asistente al preparar la propuesta (D185).
pub(crate) fn validated(
    scope: &str,
    key: &str,
    route: &str,
) -> AppResult<(PreferenceScope, String, PreferenceRoute)> {
    let (scope, key, route) = parse(scope, key, route)?;
    if scope == PreferenceScope::Domain && is_public_mail_domain(&key) {
        return Err(AppError::Invalid(format!(
            "{key} es un servicio de correo público: una preferencia por dominio abarcaría a cualquiera que lo use. Fíjela para una dirección concreta"
        )));
    }
    Ok((scope, key, route))
}

fn parse(
    scope: &str,
    key: &str,
    route: &str,
) -> AppResult<(PreferenceScope, String, PreferenceRoute)> {
    let scope = PreferenceScope::from_code(scope).ok_or_else(unknown_scope)?;
    let route = PreferenceRoute::from_code(route).ok_or_else(|| {
        AppError::Invalid("ruta desconocida: use «review» o «informative»".into())
    })?;
    let key = normalize_key(scope, key).ok_or_else(invalid_key)?;
    Ok((scope, key, route))
}
