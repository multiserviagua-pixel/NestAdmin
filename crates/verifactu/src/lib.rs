//! Núcleo VERI*FACTU de NestAdmin.
//!
//! Biblioteca pura: no abre bases de datos, no depende de Tauri y no sale a la red. Contiene
//! los registros de facturación de alta y anulación, la huella SHA-256 encadenada y su
//! verificación, el XML del servicio de remisión voluntaria de la AEAT y el parseo de su
//! respuesta, la URL y la imagen del QR de cotejo y un simulador local del servicio para
//! pruebas. El transporte real (HTTPS con certificado cualificado) queda fuera de este crate.
//!
//! Las fuentes oficiales usadas están en `FUENTES.md` y las decisiones en `DECISIONES.md`.

mod dom;
pub mod huella;
pub mod qr;
pub mod registro;
pub mod respuesta;
pub mod simulador;
pub mod tipos;
pub mod xml;

pub use huella::{comprobar_antes_de_generar, verificar_cadena, FalloCadena};
pub use qr::{url_cotejo, CodigoQr, Entorno};
pub use registro::{
    Calificacion, DetalleDesglose, Encadenamiento, GeneradoPor, IdFactura, Identificacion,
    ImporteRectificacion, Impuesto, Persona, RechazoPrevio, RegistroAlta, RegistroAnulacion,
    RegistroFacturacion, SistemaInformatico, TipoFactura, TipoRectificativa,
};
pub use respuesta::{
    EstadoEnvio, EstadoRegistro, FalloSoap, RegistroDuplicado, RespuestaEnvio, RespuestaLinea,
    ResultadoEnvio,
};
pub use tipos::{Fecha, FechaHoraHuso, Importe, Porcentaje};
pub use xml::{Cabecera, Envio, PersonaEs, RemisionRequerimiento, RemisionVoluntaria};

/// Máximo de registros por envío (servicio web, apartado 3, y `maxOccurs` del XSD).
pub const MAX_REGISTROS_POR_ENVIO: usize = 1000;
/// Tiempo de espera inicial entre envíos en segundos (Orden HAC/1177/2024, art. 16.2). La
/// respuesta de la AEAT lo actualiza en `TiempoEsperaEnvio`.
pub const TIEMPO_ESPERA_INICIAL_S: u32 = 60;

/// Errores de construcción, validación y parseo.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("formato incorrecto: {0}")]
    Formato(String),
    #[error("campo {campo}: {motivo}")]
    Campo { campo: String, motivo: String },
    #[error("el envío debe tener entre 1 y {MAX_REGISTROS_POR_ENVIO} registros; tiene {0}")]
    NumeroRegistros(usize),
    #[error("cadena de registros: {0}")]
    Cadena(#[from] FalloCadena),
    #[error("el último registro ({ultimo}) es posterior en más de un minuto al nuevo ({nuevo}): revise el reloj")]
    RelojAtrasado { ultimo: String, nuevo: String },
    #[error("XML: {0}")]
    Xml(String),
    #[error("respuesta de la AEAT incompleta: falta {0}")]
    RespuestaIncompleta(String),
    #[error("QR: {0}")]
    Qr(String),
}
