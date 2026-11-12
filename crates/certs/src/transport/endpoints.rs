//! Endpoints constantes de la AEAT (D279): compilados, sin variable de entorno, ajuste ni
//! archivo. Si fueran configurables se podría redirigir una conexión autenticada con el
//! certificado. Las constantes existen en todas las compilaciones (para mostrarlas y probarlas);
//! conectarse a ellas exige la característica `aeat-red` (D285).

use crate::CertificateKind;

/// Ruta del servicio `VerifactuSOAP` (WSDL `SistemaFacturacion.wsdl`).
pub const VERIFACTU_PATH: &str = "/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AeatEnvironment {
    Preproduction,
    Production,
}

/// Por dónde entra la AEAT según el tipo de certificado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Persona física y representante: `www1` o `prewww1`.
    Personal,
    /// Sello de entidad: `www10` o `prewww10`.
    Seal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub environment: AeatEnvironment,
    pub channel: Channel,
}

impl Endpoint {
    /// El endpoint que corresponde al tipo de certificado: el sello va a `www10`/`prewww10` y
    /// el resto (persona física, representante y tipo desconocido) a `www1`/`prewww1`.
    pub fn for_certificate(environment: AeatEnvironment, kind: CertificateKind) -> Self {
        let channel = match kind {
            CertificateKind::Sello => Channel::Seal,
            _ => Channel::Personal,
        };
        Self {
            environment,
            channel,
        }
    }

    pub const fn host(&self) -> &'static str {
        match (self.environment, self.channel) {
            (AeatEnvironment::Preproduction, Channel::Personal) => "prewww1.aeat.es",
            (AeatEnvironment::Preproduction, Channel::Seal) => "prewww10.aeat.es",
            (AeatEnvironment::Production, Channel::Personal) => "www1.agenciatributaria.gob.es",
            (AeatEnvironment::Production, Channel::Seal) => "www10.agenciatributaria.gob.es",
        }
    }

    pub const fn url(&self) -> &'static str {
        match (self.environment, self.channel) {
            (AeatEnvironment::Preproduction, Channel::Personal) => {
                "https://prewww1.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
            }
            (AeatEnvironment::Preproduction, Channel::Seal) => {
                "https://prewww10.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
            }
            (AeatEnvironment::Production, Channel::Personal) => {
                "https://www1.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
            }
            (AeatEnvironment::Production, Channel::Seal) => {
                "https://www10.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP"
            }
        }
    }

    pub const fn path(&self) -> &'static str {
        VERIFACTU_PATH
    }
}
