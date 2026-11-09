//! Proveedores de IA de NestAdmin (R03, D06–D08, D12, D100): Ollama cloud por defecto (D106,
//! con clave y destino confirmado); la IA local (D06), cualquier API
//! compatible con OpenAI (D109) y Claude/Anthropic (D110) por selección explícita, con un
//! contrato común. Ningún adaptador cambia de proveedor por su cuenta; el único respaldo
//! (del externo elegido al local, D101) lo gestiona la capa de aplicación.

pub mod anthropic;
pub mod credentials;
mod http;
pub mod learn;
pub mod models;
pub mod ollama;
pub mod ollama_cloud;
mod openai_chat;
pub mod openai_compatible;
pub mod provider;
pub mod refine;
pub mod selection;
pub mod tasks;

pub use credentials::{CredentialError, CredentialStore, MemoryCredentialStore, Secret};
pub use provider::{
    AiError, GenerateRequest, GenerateResult, ModelUnavailableCause, Provider, ProviderKind, Task,
    Usage,
};
pub use selection::{build_provider, AiService, ProviderChoice};
