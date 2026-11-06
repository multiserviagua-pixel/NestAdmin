//! Proveedores de IA de NestAdmin (R03, D06–D08, D12): Ollama local por defecto; Groq,
//! Ollama cloud (D91), cualquier API compatible con OpenAI (D94) y Claude/Anthropic (D95)
//! solo por selección explícita, con un contrato común y sin respaldo automático.

pub mod anthropic;
pub mod credentials;
pub mod groq;
mod http;
pub mod ollama;
pub mod ollama_cloud;
mod openai_chat;
pub mod openai_compatible;
pub mod provider;
pub mod selection;
pub mod tasks;

pub use credentials::{CredentialError, CredentialStore, MemoryCredentialStore, Secret};
pub use provider::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind, Task, Usage};
pub use selection::{build_provider, AiService, ProviderChoice};
