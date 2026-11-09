//! Temporización del sondeo sintético de la espera de D146 (404 o 402 del proveedor elegido).
//! Solo decide cuándo toca sondear; la prueba en sí es la sintética de D122 y no envía nada del
//! usuario.

use std::time::{Duration, Instant};

/// Cada cuánto, con la cola ociosa, se vuelve a comprobar si el proveedor ya responde.
pub const PROBE_EVERY: Duration = Duration::from_secs(15 * 60);

/// Cuenta desde que se observó la espera. No sondea nada más verla (la tarea acaba de fallar y
/// repetir enseguida solo repetiría el error): la primera vez toca pasados 15 minutos, y después
/// cada 15 minutos mientras la espera siga.
#[derive(Debug, Default)]
pub struct WaitProbe {
    since: Option<Instant>,
}

impl WaitProbe {
    /// `waiting`: hay una espera de D146 ahora. Devuelve `true` si toca sondear, y reinicia la
    /// cuenta. Sin espera, olvida la cuenta: una espera futura empieza de cero.
    pub fn due(&mut self, waiting: bool, now: Instant) -> bool {
        if !waiting {
            self.since = None;
            return false;
        }
        match self.since {
            None => {
                self.since = Some(now);
                false
            }
            Some(start) if now.duration_since(start) >= PROBE_EVERY => {
                self.since = Some(now);
                true
            }
            Some(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_probes_every_fifteen_minutes_only_while_the_wait_lasts() {
        let t0 = Instant::now();
        let at = |minutes: u64| t0 + Duration::from_secs(minutes * 60);
        let mut probe = WaitProbe::default();
        assert!(!probe.due(false, at(0)), "sin espera no sondea");
        assert!(!probe.due(true, at(1)), "al verla, empieza a contar");
        assert!(!probe.due(true, at(10)));
        assert!(!probe.due(true, at(15)), "van 14 minutos desde que se vio");
        assert!(
            probe.due(true, at(16)),
            "pasados 15 minutos desde que se vio"
        );
        assert!(!probe.due(true, at(20)));
        assert!(probe.due(true, at(31)), "y otra vez 15 minutos después");
        // Si la espera se levanta, se olvida: la siguiente cuenta de cero.
        assert!(!probe.due(false, at(40)));
        assert!(!probe.due(true, at(41)));
        assert!(!probe.due(true, at(50)));
        assert!(probe.due(true, at(56)));
    }
}
