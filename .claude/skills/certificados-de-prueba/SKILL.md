---
name: certificados-de-prueba
description: Cómo generar en NestAdmin certificados electrónicos sintéticos de cada tipo (persona física, representante, sello, caducado, sin clientAuth, PFX antiguo) con rcgen y cómo levantar el simulador de la AEAT con TLS mutuo. Úsela al escribir o ejecutar pruebas de crates/certs, del transporte o de la remisión con certificado.
---

# Certificados de prueba y simulador con TLS mutuo

Reglas que no cambian: **prohibido usar certificados reales, claves privadas reales, PFX reales ni NIF reales** en el repositorio, en fixtures, capturas, registros o evidencias. Todo certificado de una prueba se genera en el momento, en un directorio temporal de la prueba, y se borra al terminar. Nunca se contacta con la AEAT. Pruebas manuales sobre `tauri dev`; instalador solo para la versión final o si el usuario lo pide; git solo local.

Esta skill describe T18b (D278, D279, D285, R35, T18c y T18d). Los nombres de funciones y de la característica de Cargo pueden ajustarse al implementar B1 y B3; si algo no existe aún, impleméntelo siguiendo esta guía en vez de añadir un archivo de certificado.

## Reglas de higiene

- [ ] Ningún `.pfx`, `.p12`, `.pem`, `.key`, `.cer` ni `.crt` se versiona. `.gitignore` ya excluye `*.key` y `*.pem`; si faltan `*.pfx`, `*.p12`, `*.cer` y `*.crt`, añádalos como cambio aparte y dígalo.
- [ ] Los certificados se crean con `tempfile` (o la carpeta de pruebas de Cargo), no en `evidence/` ni en `crates/*/tests/` como archivos.
- [ ] La contraseña de un PFX de prueba es una constante evidente de la prueba (`"prueba-sintetica"`), nunca la de un certificado real.
- [ ] Los datos personales del titular son sintéticos y reconocibles (nombre «PRUEBA SINTETICA», dominio `.invalid`, `.test` o `example.org`).
- [ ] `rcgen` y `p12-keystore` son dependencias de desarrollo (`[dev-dependencies]`) de los crates que las usan; no entran en el binario. Si cambia `Cargo.lock`, ejecute `python scripts/third_party.py` y `--check` (D60).

## NIF sintéticos con letra de control válida

Un NIF mal calculado hace fallar la validación (`nestadmin_sales::validate_tax_id`, `crates/sales/src/tax_id.rs`) y oculta errores reales. Use estos o calcule otros y compruébelos con esa función en una prueba:

- NIF de persona física: ocho cifras y la letra `TRWAGMYFPDXBNJZSQVHLCKE[número mod 23]`. Ejemplos válidos: `12345678Z` y `89890001K` (el de los ejemplos de la AEAT, solo ilustrativo).
- NIE: la inicial `X`, `Y` o `Z` vale 0, 1 o 2 delante de las siete cifras y se aplica la misma letra. Ejemplo: `X1234567L`.
- CIF (sociedades): letra, siete cifras y control; con control numérico por defecto, `B12345674` es válido (suma de las posiciones pares 12 más las impares duplicadas 14 da 26 y el control es 10 menos 6). Compruébelo con `validate_tax_id` antes de usarlo.
- Cada prueba usa un NIF distinto para el obligado, el titular y el representante cuando quiere comprobar que no coinciden. Una prueba de NIF inválido usa una letra errónea expresa y lo dice en el nombre.

## Generar con `rcgen`

Una función de ayuda por tipo en `crates/certs/tests/common/mod.rs` (o en el módulo de pruebas del crate que lo use), que devuelva el certificado y la clave en memoria, y opcionalmente un PFX en el directorio temporal. Cada certificado lleva una autoridad de pruebas propia (una CA sintética generada en la misma prueba), para poder ser de confianza en el simulador sin tocar el almacén de Windows.

| Caso | Cómo se construye con `rcgen` | Qué debe resultar |
| --- | --- | --- |
| Persona física | `CertificateParams` con `serialNumber` `IDCES-<NIF>`, `givenName`, `surname`, `commonName`, uso de clave `digitalSignature` y `ExtendedKeyUsagePurpose::ClientAuth` | tipo `Individual`; endpoint `www1`/`prewww1` |
| Representante de persona jurídica | lo anterior más `organizationName`, `organizationIdentifier` `VATES-<CIF>` y el NIF del representante en `serialNumber` | tipo `Representative`; NIF del titular y del representado leídos; endpoint `www1`/`prewww1` |
| Sello de entidad | `organizationName`, `organizationIdentifier` `VATES-<CIF>`, sin `givenName` ni `surname`, con `ClientAuth` | tipo `Seal`; endpoint `www10`/`prewww10` |
| Caducado | `not_before` y `not_after` en el pasado (relativos a un reloj sustituible) | `certificate_expired` |
| Aún no válido | `not_before` en el futuro | `certificate_expired` o su causa propia, con prueba explícita |
| Sin `clientAuth` | solo `ExtendedKeyUsagePurpose::EmailProtection` o `CodeSigning` | validación rechaza; `certificate_rejected` si llega al saludo TLS |
| NIF distinto del obligado | NIF del titular distinto del que se prueba | aviso, sin bloqueo (D278) |
| Casi caduca | `not_after` a 59, 29 y 6 días | avisos de 60, 30 y 7 días |

Para los OID que `rcgen` no trae con nombre (`organizationIdentifier`, 2.5.4.97, y `serialNumber`, 2.5.4.5) use `DnType::CustomDnType`. Los nombres exactos de los campos del subject son los que lee `x509-parser` en B1: la prueba de lectura y la de generación van juntas para que no se desvíen.

## PFX

- **PFX moderno:** `p12-keystore` con el certificado, la clave (PKCS#8 de `rcgen`) y la CA; se escribe en memoria con la contraseña de prueba. Lo analiza `import_pfx` con `PFXImportCertStore` y `PKCS12_NO_PERSIST_KEY`, sin tocar el almacén (B2); la clave queda en la propiedad `CERT_KEY_CONTEXT` del certificado, que `CryptAcquireCertificatePrivateKey` no sirve, y por eso `acquire` la lee a mano. Las pruebas de B2 están en `crates/certs/tests/windows_pfx.rs` y `windows_store.rs`, con el material en `tests/pfx_support/`.
- **PFX antiguo (3DES o RC2, SHA1):** `p12-keystore` escribe 3DES con HMAC-SHA1 (`EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc` y `MacAlgorithm::HmacSha1`), que ya basta para la prueba sin OpenSSL. Para RC2-40 y claves RSA, la prueba usa OpenSSL si está en el PATH (OpenSSL 3 con `-legacy`): `openssl pkcs12 -export -legacy -certpbe PBE-SHA1-RC2-40 -keypbe PBE-SHA1-RC2-40 -macalg sha1 -inkey clave.pem -in cert.pem -out antiguo.pfx -passout pass:prueba-sintetica` (para 3DES, `PBE-SHA1-3DES`), con los PEM en un directorio temporal. Si `openssl` no está disponible o no admite `-legacy`, la prueba imprime `NO EJECUTADA` y no falla; anótelo en el informe como **no ejecutada**. No se sustituye por un archivo versionado. Con `NESTADMIN_REQUIRE_OPENSSL=1`, la ausencia de `openssl` hace fallar esas pruebas en vez de saltarlas. Los certificados de OpenSSL tienen la vigencia del momento en que se generan: la prueba usa el reloj real, no `NOW_MS`.
- **PFX con contraseña errónea y archivo corrupto:** deben dar error propio, sin texto del sistema ni la ruta, y sin dejar nada en el almacén de DPAPI.
- Las pruebas de DPAPI y CNG son `cfg(windows)`. **Prueba manual de `NeedsUser` (sin hardware no se puede automatizar):** en `tauri dev`, con un certificado cuya clave pida PIN o confirmación (tarjeta o DNIe con minidriver, tarjeta virtual del TPM con política de PIN, o un certificado importado con protección fuerte), `acquire` en `Silent` debe devolver `certificate_needs_user` sin mostrar ninguna ventana ni bloquearse, y en `Interactive` Windows debe mostrar su diálogo; con la tarjeta retirada, también `certificate_needs_user`. B2 solo prueba en automático la traducción de los códigos de Windows (`map_os_error`). Si no hay hardware, anótelo como no probado. La parte pura (tipo, vigencia, `clientAuth`, NIF, caducidad) corre en cualquier plataforma.

## Simulador con TLS mutuo

Implementado en B3 (D279, D285). Ejercita el transporte real (`MutualTlsTransport`, `rustls` con `ring`) contra un servidor en 127.0.0.1 que **exige certificado cliente**, sin red externa. Vive en `crates/certs/tests/mtls_support/mod.rs`; cada archivo de pruebas lo importa con `mod mtls_support;` (como `pfx_support`).

```rust
let ca = new_ca("AC PRUEBA SINTETICA");                       // CA sintética de rcgen
let server = MtlsServer::start(&ca.der, server_cert(&ca, &["localhost"]), Behavior::Proxy);
let creds = client_cert(&ca, Kind::PersonaFisica).credentials(); // o Kind::Sello
let transport = server.transport("localhost", &ca.der);       // MutualTlsTransport::for_test
let report = transport.send(endpoint, b"<x/>", &creds, TlsPreference::Auto);
let seen = server.seen();   // tipos de certificado cliente, TLS 1.3 o 1.2, cuerpos recibidos
```

- `MtlsServer::start(client_ca, (cert_servidor, clave_pkcs8), comportamiento)` levanta un hilo con `rustls::ServerConfig` y `WebPkiClientVerifier` sobre la CA dada, en `127.0.0.1:0`, y sirve las respuestas de `SimuladorAeat` (`server.simulator`; `estado().fallo_http_siguiente`, `perder_siguiente_respuesta`, etc.). Comportamientos: `Proxy` (respuesta del simulador), `CloseAfterRequest` (lee el cuerpo y cierra), `Hang` (lee y no contesta) y `Raw(bytes)` (respuesta literal: `chunked`, tamaño, basura).
- El destino se inyecta con `MutualTlsTransport::for_test(TestTarget { addr, server_name, roots_der })`, que solo existe con la característica `test-support` (las pruebas del crate la activan como autodependencia de desarrollo; el escritorio no). Los endpoints constantes y la red real (`aeat-red`) no se tocan.
- El servidor registra solo el **tipo** del certificado cliente (`inspect`), la versión de TLS y los cuerpos recibidos; nunca su contenido, NIF ni nombre. `seen.requests` vacío prueba que el cuerpo nunca llegó (D280).
- Certificado de otra CA: `new_ca("OTRA")` y `client_cert(&otra, ..)`; sin certificado: `ClientCredentials::new(vec![], clave)`; nombre que no coincide: `server.transport("otro.example.invalid", ..)`; puerto cerrado: `closed_port()`.
- Firmante falso que solo ofrece PKCS#1 v1.5: `Pkcs1Only(Arc<dyn SigningKey>)` envolviendo la clave de `rsa_client_cert(&ca, kind)`, que genera la clave RSA con `openssl genrsa -traditional` (ring no genera RSA). Sin `openssl` la prueba imprime `NO EJECUTADA` y no falla; con `NESTADMIN_REQUIRE_OPENSSL=1` falla. Ejercita la caída a TLS 1.2 y `needs_tls12`.
- Con una clave de Windows: `transport_windows_pfx.rs` importa un PFX sintético con `import_pfx`, obtiene el firmante con `acquire` y lo pasa con `ClientCredentials::from(&signer)` (la CA del PFX está en `Material::ca_der`).
- Pendiente de D1 (no de B3): titular no autorizado (el simulador devuelve el `Fault` 4112 y la remisión del obligado se para: `authorization_rejected`) y un lote con un 3000 posterior tras un intento sin envío (D280), que dependen de la integración con la bomba.

Comandos habituales (ajuste los nombres de las pruebas al implementar):

```
cargo test -p nestadmin-certs
cargo test -p nestadmin-certs --features aeat-red --test transport_endpoints
cargo clippy -p nestadmin-certs --all-targets --features aeat-red -- -D warnings
cargo test -p nestadmin-verifactu
cargo test -p nestadmin-app --test sales_remission
cargo tree -i aws-lc-rs
cargo tree -d
```

`cargo tree -i aws-lc-rs` debe salir vacío y `cargo tree -d` debe mostrar un solo `rustls`. En `tauri dev`, el simulador y los certificados sintéticos solo se usan desde el perfil de pruebas (D271); la PIN y la tarjeta virtual del TPM se prueban a mano, y si no hay hardware se anota como no probado.

## Cierre

Antes de dar por buena una prueba de certificados: ¿hay algún archivo de certificado o clave en `git status`? ¿Algún NIF o nombre de las pruebas aparece en la base, el historial o un error (centinela)? ¿Los NIF sintéticos pasan `validate_tax_id`? Informe también de lo que no se ejecutó (PFX antiguo sin OpenSSL, pruebas de Windows en otra plataforma).
