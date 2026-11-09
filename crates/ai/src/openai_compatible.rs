//! Adaptador genérico de la API de chat compatible con OpenAI (D109): OpenRouter, Mistral,
//! DeepSeek, OpenAI, Together, etc. Es un proveedor externo con el mismo estatus que los demás externos:
//! solo por selección explícita y con una credencial ligada al host de la URL base.
//!
//! El procesamiento local sigue siendo solo Ollama, así que la URL base debe ser `https://`
//! y no puede apuntar a este equipo ni a la red local.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use crate::credentials::{Secret, OPENAI_COMPATIBLE_API_KEY_PREFIX};
use crate::http;
use crate::openai_chat::ChatClient;
use crate::provider::{AiError, GenerateRequest, GenerateResult, Provider, ProviderKind};

/// URL base validada y normalizada, y el host al que queda ligada la credencial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUrl {
    /// `https://host[:puerto][/ruta]` sin barra final.
    pub url: String,
    /// Host en minúsculas (IPv6 sin corchetes).
    pub host: String,
    /// Puerto efectivo (443 si la URL no lo indica).
    pub port: u16,
}

impl BaseUrl {
    /// Nombre de la credencial en el almacén del sistema: una clave por host y puerto. Con el
    /// puerto por defecto (443) es `<prefijo><host>`; con otro, `<prefijo><host>:<puerto>`.
    pub fn credential_name(&self) -> String {
        if self.port == DEFAULT_PORT {
            format!("{OPENAI_COMPATIBLE_API_KEY_PREFIX}{}", self.host)
        } else {
            format!(
                "{OPENAI_COMPATIBLE_API_KEY_PREFIX}{}:{}",
                self.host, self.port
            )
        }
    }
}

const DEFAULT_PORT: u16 = 443;

fn reject(reason: &str) -> AiError {
    AiError::Unavailable(format!("URL base no admitida: {reason}"))
}

/// Valida la URL base de un proveedor compatible con OpenAI (D109). Exige `https://`, rechaza
/// credenciales, query, fragmento y cualquier host local o privado.
pub fn parse_base_url(raw: &str) -> Result<BaseUrl, AiError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(reject("está vacía"));
    }
    if raw
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '\\' || !c.is_ascii())
    {
        return Err(reject("contiene caracteres no válidos"));
    }
    let Some(rest) = raw
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
        .map(|_| &raw[8..])
    else {
        return Err(reject("solo se admite https://"));
    };
    if rest.contains('?') || rest.contains('#') {
        return Err(reject("no puede llevar query ni fragmento"));
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err(reject("no puede llevar usuario ni contraseña"));
    }
    let (host, port) = split_host_port(authority)?;
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    check_public_host(host)?;
    let shown = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let shown_port = port.map(|p| format!(":{p}")).unwrap_or_default();
    Ok(BaseUrl {
        url: format!("https://{shown}{shown_port}{}", path.trim_end_matches('/')),
        host: host.to_owned(),
        port: port.unwrap_or(DEFAULT_PORT),
    })
}

/// Nombre de la credencial para una URL base o para un host suelto (valida la URL).
pub fn credential_name_for(base_url_or_host: &str) -> Result<String, AiError> {
    let input = base_url_or_host.trim();
    if input.contains("://") {
        parse_base_url(input)
    } else {
        parse_base_url(&format!("https://{input}"))
    }
    .map(|b| b.credential_name())
}

fn split_host_port(authority: &str) -> Result<(&str, Option<u16>), AiError> {
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let end = inner.find(']').ok_or_else(|| reject("IPv6 sin cerrar"))?;
        let after = &inner[end + 1..];
        let port = match after {
            "" => None,
            _ => Some(
                after
                    .strip_prefix(':')
                    .ok_or_else(|| reject("host no válido"))?,
            ),
        };
        (&inner[..end], port)
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        None => None,
        Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => {
            match p.parse::<u16>() {
                Ok(n) if n > 0 => Some(n),
                _ => return Err(reject("puerto no válido")),
            }
        }
        Some(_) => return Err(reject("puerto no válido")),
    };
    if host.is_empty() {
        return Err(reject("falta el host"));
    }
    Ok((host, port))
}

fn check_public_host(host: &str) -> Result<(), AiError> {
    if host.contains(':') {
        let ip: Ipv6Addr = host.parse().map_err(|_| reject("IPv6 no válida"))?;
        return if is_blocked_ip(&IpAddr::V6(ip)) {
            Err(local_only())
        } else {
            Ok(())
        };
    }
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return if is_blocked_ip(&IpAddr::V4(ip)) {
            Err(local_only())
        } else {
            Ok(())
        };
    }
    let labels: Vec<&str> = host.split('.').collect();
    let well_formed = labels.iter().all(|l| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    if !well_formed {
        return Err(reject("host no válido"));
    }
    if labels.len() < 2 {
        // `localhost`, `nas`, `servidor`: nombres de la red local.
        return Err(local_only());
    }
    // Un TLD empieza por letra: descarta `2130706433`, `0x7f.1` y otras formas numéricas que
    // algunos resolvedores interpretan como direcciones IPv4 (por ejemplo, de bucle local).
    if !labels[labels.len() - 1].starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Err(reject("host no válido"));
    }
    const LOCAL_SUFFIXES: [&str; 6] = [
        "localhost",
        "local",
        "internal",
        "lan",
        "home.arpa",
        "localdomain",
    ];
    if LOCAL_SUFFIXES
        .iter()
        .any(|s| host == *s || host.ends_with(&format!(".{s}")))
    {
        return Err(local_only());
    }
    Ok(())
}

fn local_only() -> AiError {
    reject(
        "apunta a este equipo o a la red local; el procesamiento local es solo Ollama \
         (proveedor local)",
    )
}

/// Clasificación única de direcciones no públicas (D109): la usan la validación sintáctica de
/// la URL y el resolvedor que filtra el DNS al conectar.
pub(crate) fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => blocked_v6(v6),
    }
}

fn blocked_v4(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast() // 224.0.0.0/4
        || o[0] == 0
        || o[0] >= 240 // 240.0.0.0/4 reservada y 255.255.255.255
        || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64.0.0/10
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15
        || (o[0] == 192 && o[1] == 0 && (o[2] == 0 || o[2] == 2)) // 192.0.0.0/24, 192.0.2.0/24
        || (o[0] == 198 && o[1] == 51 && o[2] == 100) // 198.51.100.0/24
        || (o[0] == 203 && o[1] == 0 && o[2] == 113) // 203.0.113.0/24
}

fn blocked_v6(ip: &Ipv6Addr) -> bool {
    let s = ip.segments();
    if let Some(v4) = ip.to_ipv4() {
        return blocked_v4(&v4);
    }
    // 64:ff9b::/96 (NAT64) lleva una IPv4 en los 32 bits finales.
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6].iter().all(|x| *x == 0) {
        let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return blocked_v4(&v4);
    }
    // 2002::/16 (6to4) lleva una IPv4 en los bits 16..48.
    if s[0] == 0x2002 {
        let v4 = Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8);
        return blocked_v4(&v4);
    }
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast() // ff00::/8
        || (s[0] & 0xfe00) == 0xfc00 // fc00::/7
        || (s[0] & 0xffc0) == 0xfe80 // fe80::/10
        || (s[0] & 0xffc0) == 0xfec0 // fec0::/10 (obsoleta)
        || (s[0] == 0x2001 && s[1] == 0x0db8) // 2001:db8::/32 (documentación)
}

/// Adaptador externo (D109). Solo se construye tras la selección explícita del usuario y con
/// la credencial de ese host; nunca se usa como respaldo de otro proveedor.
pub struct OpenAiCompatibleProvider {
    client: ChatClient,
    /// `host` o `host:puerto` (si no es el 443) de la URL base.
    destination: String,
}

impl OpenAiCompatibleProvider {
    /// Valida la URL base (D109). Sin redirecciones: la clave solo viaja al host validado.
    pub fn new(
        base_url: &str,
        model: &str,
        api_key: Secret,
        timeout: Duration,
    ) -> Result<Self, AiError> {
        let base = parse_base_url(base_url)?;
        let destination = if base.port == DEFAULT_PORT {
            base.host.clone()
        } else {
            format!("{}:{}", base.host, base.port)
        };
        Ok(Self {
            client: ChatClient::new(&base.url, model, api_key, http::agent_external(timeout)),
            destination,
        })
    }

    /// Para pruebas contra un servidor simulado en 127.0.0.1: agente sin el filtro de DNS y
    /// sin validar la URL. No se expone a la configuración de usuario.
    #[doc(hidden)]
    pub fn with_endpoint(endpoint: &str, model: &str, api_key: Secret, timeout: Duration) -> Self {
        Self {
            client: ChatClient::new(endpoint, model, api_key, http::agent_no_redirects(timeout)),
            destination: endpoint.to_owned(),
        }
    }
}

impl std::fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatibleProvider")
            .field("endpoint", &self.client.endpoint)
            .field("model", &self.client.model)
            .field("api_key", &self.client.api_key)
            .finish()
    }
}

impl Provider for OpenAiCompatibleProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAiCompatible
    }

    fn model(&self) -> &str {
        &self.client.model
    }

    fn destination(&self) -> Option<String> {
        Some(self.destination.clone())
    }

    fn configured_url(&self) -> Option<String> {
        Some(self.client.endpoint.clone())
    }

    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResult, AiError> {
        self.client
            .generate(ProviderKind::OpenAiCompatible, request)
    }
}
