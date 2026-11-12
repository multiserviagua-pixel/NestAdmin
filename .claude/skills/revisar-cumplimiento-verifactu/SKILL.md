---
name: revisar-cumplimiento-verifactu
description: Lista de revisión para cambios de NestAdmin que tocan la emisión de facturas, los registros Verifactu, la cadena, la remisión a la AEAT, el entorno, el perfil de pruebas, los obligados, el certificado o la declaración responsable. Úsela antes de dar por terminado uno de esos cambios.
---

# Revisar cambios de cumplimiento Verifactu

Reglas que no cambian: pruebas manuales sobre `tauri dev`; instalador solo para la versión final o si el usuario lo pide; git solo local, sin push ni PR; fixtures, certificados y NIF siempre sintéticos (skill `certificados-de-prueba`); nunca se contacta con la AEAT desde pruebas (D85, D285).

Fuentes de verdad: requisitos R18 y R35–R38 en `specs/v1/requisitos.md`, decisiones D71, D83–D86, D193–D203, D247–D261 y D271–D286 en `specs/v1/decisiones.md`, y los contratos de la sección «Verifactu» de `specs/v1/contratos.md`. Si el cambio contradice una decisión, deténgase y regístrelo: no lo resuelva en el código.

Lea el diff entero y compruebe cada punto en el código. Si no puede comprobar uno, dígalo; no lo dé por bueno. Cada casilla sin marcar es un hallazgo.

Código afectado habitualmente: `crates/verifactu/src/` (registros, huella, XML, simulador), `crates/core/src/sales/verifactu.rs` y `verifactu_remission.rs`, `crates/core/src/migrations/012_verifactu.sql`, `013_installation.sql` y `016_verifactu_remission.sql`, `crates/app/src/remission.rs` y `sales.rs`, `crates/desktop/src/ventas.rs` y `productor.rs`, y, cuando existan, `crates/certs/`.

## Fallo cerrado

- [ ] Un dato ausente, antiguo o sin comprobar impide la emisión o la remisión; nunca las permite. Un entorno, un tipo de perfil o una fila de obligado ausentes son un error (`storage`), no un valor por defecto (D258).
- [ ] La remisión a producción solo se activa con toda la barrera de D276 cumplida a la vez; cada condición tiene su prueba por separado.
- [ ] Sin declaración responsable de la versión exacta, o sin productor completo, no hay remisión a producción (D277).
- [ ] Un error desconocido de la AEAT o del transporte se trata como fallo (no como aceptado) y el registro sigue pendiente.
- [ ] Cadena rota o reloj atrasado detienen la emisión con un código explícito y sin consumir número (D195, D284).

## Nada de duplicados ni de pérdidas (D50, D252, D280)

- [ ] Reintentar nunca crea registros nuevos ni cambia la cadena; el lote abierto se reenvía con los mismos bytes y el mismo orden. Exija una prueba de corte entre cada par de pasos (lote creado, intento anotado, llamada, respuesta, cierre).
- [ ] Un 3000 solo cuenta como aceptado si el lote ya se había enviado antes de verdad. Los intentos que fallaron antes de escribir el cuerpo (certificado, clave, PIN, saludo TLS, caída a TLS 1.2) no se anotan como envío o se anotan aparte y no cuentan (D280). Compruebe en el diff que el certificado y la clave se obtienen **antes** de `begin_verifactu_attempt`.
- [ ] Un solo lote abierto por obligado; un lote nunca mezcla obligados ni dos registros de la misma factura (D257).
- [ ] Cerrar un lote dos veces da `conflict` y no repite efectos; la subsanación es idempotente (D253).
- [ ] La clasificación de `Fault` de D281 se mantiene: autorización (4112, 4105, 4117, 4123, 4124) para solo a ese obligado y no se reintenta; técnico (4108, 4110, 4134, 4141) se reintenta como mucho cada hora y nunca para; esquema para con aviso.

## La emisión nunca espera a la red (D250)

- [ ] Emitir y anular no llaman a la red, no esperan al certificado ni a la AEAT, y sus fallos de remisión solo se anotan. Prueba con la base sin red, sin certificado y con un lote abierto: se emite igual y el registro queda pendiente y visible.
- [ ] Ninguna llamada de red se hace con el cerrojo de la aplicación tomado (`verifactu_pump_begin`, `run`, `finish`).
- [ ] La validación del NIF con la AEAT, el cotejo del QR y el catálogo de errores son avisos: nunca bloquean la emisión.
- [ ] La única excepción a R07 es la remisión obligatoria de registros ya generados; consta como `obligacion_verifactu <entorno>` en el historial.

## Inalterabilidad y permanencia

- [ ] Toda tabla nueva de registro, lote, intento, resultado, incidencia, aceptación o declaración es de solo inserción, con triggers que rechazan `UPDATE` y `DELETE`, y con su prueba de rechazo (modelo: `crates/core/tests/verifactu_store.rs` y `verifactu_remission_store.rs`).
- [ ] Una migración no reescribe ni borra registros existentes; prueba de migración desde la versión anterior.
- [ ] En `verifactu_environment_changes`, desde `produccion` no se vuelve a otro entorno (trigger, D276); la fila de tipo de perfil es inmutable y un perfil de pruebas no llega a producción (D271).
- [ ] Numeración sin huecos ni reutilización de números anulados; fecha de expedición no futura ni anterior al último registro; campos fiscales bloqueados al emitir.

## Reloj de 1 minuto (art. 7.i de la Orden)

- [ ] Antes de generar cada registro se verifica la cadena entera y que el último no supera en más de 60 s la fecha y hora nueva. Falla con `verifactu_chain_broken` o `verifactu_clock_behind` y sin consumir número.
- [ ] La prueba usa el reloj sustituible (`SalesClock`, D201), no el del sistema.

## Perfil de pruebas, entornos y obligados

- [ ] El perfil normal genera sus registros en `produccion` desde el primero; `simulador` y `preproduccion` solo existen en el perfil de pruebas, que muestra su aviso permanente (D271, D275).
- [ ] El perfil normal no contiene nunca obligados ni datos ficticios; ningún dato de prueba se escribe en él, ni siquiera en una prueba automática.
- [ ] El traslado al perfil de pruebas (D272) conserva el orden: comprobar, copiar con la API de SQLite, volver a comprobar, marcador, base nueva y puntero atómico; hay prueba de corte tras cada paso y no se borra nada.
- [ ] Una base por obligado (D273): ninguna consulta ni tabla mezcla obligados; el obligado activo se muestra siempre y la bomba recorre todos. `IndicadorMultiplesOT` se calcula (cuenta también las bajas) y no lo puede fijar el usuario; `NumeroInstalacion` no se repite nunca (D274).
- [ ] `Representante` solo si se configuró expresamente; nunca se deduce de que el NIF del certificado no coincida (D283).
- [ ] `Incidencia = S` se decide al abrir el lote y un lote ya abierto no cambia de bytes (D282).

## Endpoints constantes

- [ ] Las URL de la AEAT (`www1`, `www10`, `prewww1`, `prewww10`, cotejo `www2`/`prewww2`) son constantes compiladas: ninguna variable de entorno, ajuste, archivo ni argumento las cambia (D279). `grep` en el diff de `env::var`, `configuracion.json` y `set_verifactu_endpoint` fuera de `cfg(debug_assertions)` o de pruebas.
- [ ] El sello usa `www10` o `prewww10`; la persona física y el representante, `www1` o `prewww1`.
- [ ] Las URL reales solo se enlazan con la característica de compilación de red real (D285), desactivada por defecto. `cargo test --workspace` no la activa.
- [ ] Las variables de simulación (`NESTADMIN_VERIFACTU_SIM`) siguen limitadas a `cfg(debug_assertions)` y al entorno `simulador` (D254).

## Certificado y datos personales

- [ ] Nada del certificado ni de su titular (contenido, contraseña, PIN, NIF, nombre, huella ni subject) llega a la base, al historial, a los registros de log, a los mensajes de error, a los avisos de la ventana ni a los artefactos. Los errores son códigos: `certificate_missing`, `certificate_expired`, `certificate_needs_user`, `certificate_key_unsupported`, `certificate_rejected`, `authorization_rejected`.
- [ ] Prueba con centinelas: un certificado sintético con un NIF y un nombre inconfundibles, y una búsqueda de esos textos en la base, el historial y las salidas de error y de registro.
- [ ] El PFX va en un archivo DPAPI de usuario con entropía por instalación y `CRYPTPROTECT_UI_FORBIDDEN`, nunca en el Administrador de credenciales; los secretos pasan por `zeroize` y no se clonan sin necesidad (D278).
- [ ] La aplicación nunca pide ni guarda el PIN; si la clave lo necesita, el registro queda en `certificate_needs_user` hasta «Remitir ahora».
- [ ] El certificado no se instala en el almacén de Windows sin consentimiento.
- [ ] Ningún certificado real, clave, PFX ni NIF real en el repositorio, fixtures, capturas ni evidencias.
- [ ] Sin respaldo hacia otro destino: el certificado solo se usa en el transporte de remisión.

## Cierre

Ejecute y anote la salida literal de: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo tree -i aws-lc-rs` (debe salir vacío), `cargo tree -d` (un solo `rustls`) y `python scripts/check.py`. Pase `security-review` y `code-review` en nivel `medium`; sobre `crates/certs` añada `spec-security-review`. Pida a `arquitecto` que revise el diff sin darle su conclusión, obligatorio en B2, D1, C1 y D2. No marque criterios de la matriz como cumplidos sin evidencia reproducible, y no construya el instalador. Informe de lo que no se ha podido verificar, sobre todo lo que depende de Windows, de hardware (PIN, tarjeta, TPM) o de la preproducción real.
