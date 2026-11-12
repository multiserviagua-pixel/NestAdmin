//! Material de prueba del transporte (B3): una CA sintética de `rcgen`, certificados de
//! servidor y de cliente y un servidor TLS mutuo en 127.0.0.1 con puerto efímero que sirve las
//! respuestas del simulador de la AEAT de `crates/verifactu`. Todo en memoria: nada se
//! versiona ni se contacta con ningún servidor externo. El servidor registra solo el tipo del
//! certificado cliente (`inspect`), la versión de TLS y los cuerpos recibidos, nunca el
//! contenido del certificado.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nestadmin_certs::{
    inspect, CertificateKind, ClientCredentials, MutualTlsTransport, TestTarget,
};
use nestadmin_verifactu::simulador::{ConfigSimulador, SimuladorAeat};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, DnValue,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::WebPkiClientVerifier;
use rustls::sign::{Signer, SigningKey};
use rustls::{RootCertStore, ServerConfig, ServerConnection, SignatureAlgorithm, SignatureScheme};

pub const HOLDER_NIF: &str = "12345678Z";
pub const ENTITY_CIF: &str = "B12345674";

pub struct Ca {
    pub der: Vec<u8>,
    issuer: Issuer<'static, KeyPair>,
}

pub fn new_ca(name: &str) -> Ca {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, name);
    params.distinguished_name = dn;
    let der = params.clone().self_signed(&key).unwrap().der().to_vec();
    Ca {
        der,
        issuer: Issuer::new(params, key),
    }
}

/// Certificado de servidor con los nombres dados en el SAN y su clave PKCS#8.
pub fn server_cert(ca: &Ca, names: &[&str]) -> (Vec<u8>, Vec<u8>) {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params =
        CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let cert = params.signed_by(&key, &ca.issuer).unwrap();
    (cert.der().to_vec(), key.serialize_der())
}

#[derive(Clone, Copy)]
pub enum Kind {
    PersonaFisica,
    Sello,
}

fn client_params(kind: Kind) -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "PRUEBA SINTETICA");
    match kind {
        Kind::PersonaFisica => {
            dn.push(
                DnType::CustomDnType(vec![2, 5, 4, 5]),
                DnValue::Utf8String(format!("IDCES-{HOLDER_NIF}")),
            );
            dn.push(
                DnType::CustomDnType(vec![2, 5, 4, 42]),
                DnValue::Utf8String("PRUEBA".into()),
            );
            dn.push(
                DnType::CustomDnType(vec![2, 5, 4, 4]),
                DnValue::Utf8String("SINTETICA".into()),
            );
        }
        Kind::Sello => {
            dn.push(DnType::OrganizationName, "ENTIDAD SINTETICA SL");
            dn.push(
                DnType::CustomDnType(vec![2, 5, 4, 97]),
                DnValue::Utf8String(format!("VATES-{ENTITY_CIF}")),
            );
        }
    }
    params.distinguished_name = dn;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params
}

pub struct ClientMaterial {
    pub cert_der: Vec<u8>,
    /// PKCS#8 (ECDSA) o PKCS#1 (RSA, `pkcs1 == true`).
    pub key_der: Vec<u8>,
    pub pkcs1: bool,
}

impl ClientMaterial {
    pub fn signing_key(&self) -> Arc<dyn SigningKey> {
        let der = if self.pkcs1 {
            PrivateKeyDer::from(rustls::pki_types::PrivatePkcs1KeyDer::from(
                self.key_der.clone(),
            ))
        } else {
            PrivateKeyDer::from(PrivatePkcs8KeyDer::from(self.key_der.clone()))
        };
        rustls::crypto::ring::sign::any_supported_type(&der).unwrap()
    }

    /// Credenciales con la clave de `ring` (ECDSA o RSA).
    pub fn credentials(&self) -> ClientCredentials {
        ClientCredentials::new(vec![self.cert_der.clone()], self.signing_key())
    }
}

pub fn client_cert(ca: &Ca, kind: Kind) -> ClientMaterial {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let cert = client_params(kind).signed_by(&key, &ca.issuer).unwrap();
    ClientMaterial {
        cert_der: cert.der().to_vec(),
        key_der: key.serialize_der(),
        pkcs1: false,
    }
}

/// Clave pública RSA (`RSAPublicKey` DER) para firmar el certificado de un sujeto.
struct RsaPublic(Vec<u8>);

impl rcgen::PublicKeyData for RsaPublic {
    fn der_bytes(&self) -> &[u8] {
        &self.0
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_RSA_SHA256
    }
}

/// Certificado cliente con una clave RSA que aporta OpenSSL (`ring` no genera RSA). `None` si
/// `openssl` no está en el PATH; con `NESTADMIN_REQUIRE_OPENSSL=1` eso hace fallar la prueba.
pub fn rsa_client_cert(ca: &Ca, kind: Kind) -> Option<ClientMaterial> {
    let dir = tempfile::tempdir().ok()?;
    let path = dir.path().join("k.der");
    let ok = std::process::Command::new("openssl")
        .args(["genrsa", "-traditional", "-outform", "DER", "-out"])
        .arg(&path)
        .arg("2048")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let pkcs1 = if ok { std::fs::read(&path).ok() } else { None };
    let Some(pkcs1) = pkcs1 else {
        assert!(
            std::env::var("NESTADMIN_REQUIRE_OPENSSL").as_deref() != Ok("1"),
            "falta openssl y NESTADMIN_REQUIRE_OPENSSL=1"
        );
        println!("NO EJECUTADA: falta openssl en el PATH");
        return None;
    };
    let pair = ring::signature::RsaKeyPair::from_der(&pkcs1).unwrap();
    let public = RsaPublic(
        ring::signature::KeyPair::public_key(&pair)
            .as_ref()
            .to_vec(),
    );
    let cert = client_params(kind).signed_by(&public, &ca.issuer).unwrap();
    Some(ClientMaterial {
        cert_der: cert.der().to_vec(),
        key_der: pkcs1,
        pkcs1: true,
    })
}

pub fn ring_key(pkcs8: &[u8]) -> Arc<dyn SigningKey> {
    let der = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(pkcs8.to_vec()));
    rustls::crypto::ring::sign::any_supported_type(&der).unwrap()
}

/// Firmante falso: solo ofrece RSA PKCS#1 v1.5, como una tarjeta que no hace PSS.
#[derive(Debug)]
pub struct Pkcs1Only(pub Arc<dyn SigningKey>);

impl SigningKey for Pkcs1Only {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        let only: Vec<SignatureScheme> = offered
            .iter()
            .copied()
            .filter(|s| {
                matches!(
                    s,
                    SignatureScheme::RSA_PKCS1_SHA256
                        | SignatureScheme::RSA_PKCS1_SHA384
                        | SignatureScheme::RSA_PKCS1_SHA512
                )
            })
            .collect();
        if only.is_empty() {
            None
        } else {
            self.0.choose_scheme(&only)
        }
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        self.0.algorithm()
    }
}

// ---------------------------------------------------------------------------------------------
// Servidor TLS mutuo
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub enum Behavior {
    /// Sirve la respuesta del simulador de la AEAT (sin respuesta si este la pierde).
    Proxy,
    /// Lee la petición completa y cierra sin responder.
    CloseAfterRequest,
    /// Lee la petición y no responde (hasta que acabe la prueba).
    Hang,
    /// Responde estos bytes tal cual.
    Raw(Vec<u8>),
}

#[derive(Debug, Clone, Default)]
pub struct Seen {
    /// Saludos TLS completados con éxito.
    pub handshakes_ok: usize,
    /// Saludos que el servidor rechazó (certificado ausente o no confiable).
    pub handshakes_rejected: usize,
    /// Tipos de los certificados cliente de los saludos correctos.
    pub client_kinds: Vec<CertificateKind>,
    /// Por saludo correcto: `true` si fue TLS 1.3.
    pub tls13: Vec<bool>,
    /// Cuerpos de las peticiones recibidas.
    pub requests: Vec<Vec<u8>>,
}

/// Verificador que espera y rechaza siempre (ver [`MtlsServer::start_slow_rejecting`]).
#[derive(Debug)]
struct SlowReject {
    inner: Arc<dyn ClientCertVerifier>,
    delay: Duration,
}

impl ClientCertVerifier for SlowReject {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        thread::sleep(self.delay);
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::BadEncoding,
        ))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

pub struct MtlsServer {
    pub addr: SocketAddr,
    pub simulator: Arc<SimuladorAeat>,
    pub seen: Arc<Mutex<Seen>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MtlsServer {
    /// `client_ca`: la CA que firma los certificados cliente aceptados. `server`: certificado
    /// del servidor y su clave PKCS#8.
    pub fn start(client_ca: &[u8], server: (Vec<u8>, Vec<u8>), behavior: Behavior) -> Self {
        let verifier = Self::webpki_verifier(client_ca);
        Self::start_with_verifier(verifier, server, behavior)
    }

    fn webpki_verifier(client_ca: &[u8]) -> Arc<dyn ClientCertVerifier> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(client_ca.to_vec())).unwrap();
        WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .unwrap()
    }

    /// Un servidor que rechaza todo certificado cliente, pero tarda `delay` en hacerlo: la
    /// alerta de certificado llega mucho después de que el cliente termine su saludo TLS 1.3 y
    /// escriba el cuerpo.
    pub fn start_slow_rejecting(
        client_ca: &[u8],
        server: (Vec<u8>, Vec<u8>),
        delay: Duration,
    ) -> Self {
        let verifier = Arc::new(SlowReject {
            inner: Self::webpki_verifier(client_ca),
            delay,
        });
        Self::start_with_verifier(verifier, server, Behavior::Proxy)
    }

    fn start_with_verifier(
        verifier: Arc<dyn ClientCertVerifier>,
        server: (Vec<u8>, Vec<u8>),
        behavior: Behavior,
    ) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .unwrap()
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                vec![CertificateDer::from(server.0)],
                PrivateKeyDer::from(PrivatePkcs8KeyDer::from(server.1)),
            )
            .unwrap();
        let config = Arc::new(config);
        let simulator = Arc::new(SimuladorAeat::iniciar(ConfigSimulador::default()).unwrap());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (sim, seen2, stop2) = (simulator.clone(), seen.clone(), stop.clone());
        let thread = thread::spawn(move || {
            for conn in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(sock) = conn else { continue };
                let (config, behavior, sim, seen, stop) = (
                    config.clone(),
                    behavior.clone(),
                    sim.clone(),
                    seen2.clone(),
                    stop2.clone(),
                );
                thread::spawn(move || handle(sock, config, behavior, &sim, &seen, &stop));
            }
        });
        Self {
            addr,
            simulator,
            seen,
            stop,
            thread: Some(thread),
        }
    }

    pub fn transport(&self, server_name: &str, trusted_ca: &[u8]) -> MutualTlsTransport {
        MutualTlsTransport::for_test(TestTarget {
            addr: self.addr,
            server_name: server_name.to_string(),
            roots_der: vec![trusted_ca.to_vec()],
        })
    }

    pub fn seen(&self) -> Seen {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for MtlsServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn handle(
    mut sock: TcpStream,
    config: Arc<ServerConfig>,
    behavior: Behavior,
    sim: &SimuladorAeat,
    seen: &Mutex<Seen>,
    stop: &AtomicBool,
) {
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(mut conn) = ServerConnection::new(config) else {
        return;
    };
    while conn.is_handshaking() {
        if conn.complete_io(&mut sock).is_err() {
            // La alerta ya se intentó enviar; se cierra y se vacía lo pendiente para que el
            // cierre no se convierta en un reinicio de conexión.
            let _ = sock.shutdown(std::net::Shutdown::Write);
            let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));
            let mut sink = [0u8; 1024];
            while matches!(sock.read(&mut sink), Ok(n) if n > 0) {}
            seen.lock().unwrap().handshakes_rejected += 1;
            return;
        }
    }
    {
        let mut s = seen.lock().unwrap();
        s.handshakes_ok += 1;
        s.tls13
            .push(conn.protocol_version() == Some(rustls::ProtocolVersion::TLSv1_3));
        if let Some(der) = conn.peer_certificates().and_then(|c| c.first()) {
            if let Ok(info) = inspect(der) {
                s.client_kinds.push(info.kind);
            }
        }
    }
    let mut tls = rustls::Stream::new(&mut conn, &mut sock);
    let Some(request) = read_request(&mut tls) else {
        return;
    };
    seen.lock()
        .unwrap()
        .requests
        .push(body_of(&request).to_vec());
    match behavior {
        Behavior::CloseAfterRequest => {}
        Behavior::Hang => {
            for _ in 0..50 {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        Behavior::Raw(bytes) => {
            let _ = tls.write_all(&bytes);
            let _ = tls.flush();
            tls.conn.send_close_notify();
            let _ = tls.conn.complete_io(tls.sock);
        }
        Behavior::Proxy => {
            let Ok(mut inner) = TcpStream::connect(sim.direccion()) else {
                return;
            };
            let _ = inner.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = inner.write_all(&request);
            let mut reply = Vec::new();
            let _ = inner.read_to_end(&mut reply);
            if !reply.is_empty() {
                let _ = tls.write_all(&reply);
                let _ = tls.flush();
                tls.conn.send_close_notify();
                let _ = tls.conn.complete_io(tls.sock);
            }
        }
    }
}

fn body_of(request: &[u8]) -> &[u8] {
    request
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| &request[i + 4..])
        .unwrap_or(&[])
}

/// Lee una petición HTTP completa (cabecera y `Content-Length`).
fn read_request(r: &mut impl Read) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = r.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..i]).to_ascii_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            if data.len() >= i + 4 + len {
                return Some(data);
            }
        }
    }
}

/// Un puerto de 127.0.0.1 en el que no escucha nadie.
pub fn closed_port() -> SocketAddr {
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}
