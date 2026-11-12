---
name: certificados-de-prueba
description: Cómo generar en NestAdmin certificados electrónicos sintéticos de cada tipo (persona física, representante, sello, caducado, sin clientAuth, PFX antiguo) con rcgen y cómo levantar el simulador de la AEAT con TLS mutuo. Úsela al escribir o ejecutar pruebas de crates/certs, del transporte o de la remisión con certificado.
---

# Certificados de prueba y simulador con TLS mutuo

Reglas que no cambian: **prohibido usar certificados reales, claves privadas reales, PFX reales ni NIF reales** en el repositorio, en fixtures, capturas, registros o evidencias. Todo certificado de una prueba se genera en el momento, en un directorio temporal de la prueba, y se borra al terminar. Nunca se contacta con la AEAT. Pruebas manuales sobre `tauri dev`; instalador solo para la versión final o si el usuario lo pide; git solo local.

Esta skill describe lo que prevé T18b (D278, D279, D285, R35, T18c y T18d). Los nombres de funciones y de la característica de Cargo pueden ajustarse al implementar B1 y B3; si algo no existe aún, impleméntelo siguiendo esta guía en vez de añadir un archivo de certificado.

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

- **PFX moderno:** `p12-keystore` (o equivalente) con el certificado, la clave y la CA; se escribe en el directorio temporal con la contraseña de prueba. Se carga con `CertStore::from_pkcs12` y `NO_PERSIST_KEY` para tener una firma CNG real sin tocar el almacén (B2).
- **PFX antiguo (3DES o RC2, SHA1):** `p12-keystore` no genera cifrados antiguos. Hágalo en la prueba con OpenSSL, si está instalado: `openssl pkcs12 -export -legacy -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 -inkey clave.pem -in cert.pem -certfile ca.pem -out antiguo.pfx -passout pass:prueba-sintetica` (para RC2, `-certpbe PBE-SHA1-RC2-40 -keypbe PBE-SHA1-RC2-40`), escribiendo antes los PEM en el directorio temporal. Si `openssl` no está disponible, la prueba se marca `#[ignore]` con el motivo y se anota en el informe como **no ejecutada**; no se sustituye por un archivo versionado.
- **PFX con contraseña errónea y archivo corrupto:** deben dar error propio, sin texto del sistema ni la ruta, y sin dejar nada en el almacén de DPAPI.
- Las pruebas de DPAPI y CNG son `cfg(windows)`. La parte pura (tipo, vigencia, `clientAuth`, NIF, caducidad) corre en cualquier plataforma.

## Simulador con TLS mutuo

Objetivo (B3, D279, D285): ejercitar el transporte real (`SoapTransport` con `rustls`) contra un servidor en 127.0.0.1 que **exige certificado cliente**, sin red externa.

1. Genere una CA de pruebas y, con ella, el certificado del servidor (SAN `127.0.0.1` y `localhost`) y los certificados cliente de la tabla anterior.
2. Levante el simulador de `crates/verifactu` (`SimuladorAeat`, el mismo de E5) detrás de un `rustls::ServerConfig` con `WebPkiClientVerifier` construido sobre la CA de pruebas; escucha en un puerto libre de 127.0.0.1.
3. El cliente (`rustls::ClientConfig` con `builder_with_provider(ring)`) confía en la CA de pruebas **solo en el código de la prueba**; las raíces de `webpki-roots` y los endpoints constantes de producción no se tocan. Los endpoints constantes no son configurables: el punto de entrada de pruebas es un constructor de transporte con `cfg(test)` o `cfg(debug_assertions)` que recibe el destino, igual que `set_verifactu_endpoint` en E5 (D261).
4. El simulador registra solo el **tipo** del certificado cliente y el endpoint por el que entró (personal o sello); no guarda su contenido, su NIF ni su nombre.
5. Casos mínimos: certificado válido (acepta), sin certificado cliente (el saludo falla: `certificate_missing`), certificado de otra CA (`certificate_rejected`), sello hacia el endpoint de sello y persona física hacia el personal, titular no autorizado (el simulador devuelve el `Fault` 4112 y la remisión del obligado se para: `authorization_rejected`), y un firmante falso que solo ofrece PKCS#1 v1.5 para ejercitar la caída a TLS 1.2 (D279).
6. Un intento que falla antes de escribir el cuerpo no cuenta como envío: compruebe que un lote con un 3000 posterior no se da por aceptado (D280).

Comandos habituales (ajuste los nombres de las pruebas al implementar):

```
cargo test -p nestadmin-certs
cargo test -p nestadmin-verifactu
cargo test -p nestadmin-app --test sales_remission
cargo tree -i aws-lc-rs
cargo tree -d
```

`cargo tree -i aws-lc-rs` debe salir vacío y `cargo tree -d` debe mostrar un solo `rustls`. En `tauri dev`, el simulador y los certificados sintéticos solo se usan desde el perfil de pruebas (D271); la PIN y la tarjeta virtual del TPM se prueban a mano, y si no hay hardware se anota como no probado.

## Cierre

Antes de dar por buena una prueba de certificados: ¿hay algún archivo de certificado o clave en `git status`? ¿Algún NIF o nombre de las pruebas aparece en la base, el historial o un error (centinela)? ¿Los NIF sintéticos pasan `validate_tax_id`? Informe también de lo que no se ejecutó (PFX antiguo sin OpenSSL, pruebas de Windows en otra plataforma).
