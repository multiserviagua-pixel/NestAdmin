---
name: nueva-version-sif
description: Pasos al publicar una versión nueva de NestAdmin como sistema informático de facturación (SIF): versión, productor compilado, declaración responsable de esa versión y su historial, inventario de terceros, evidencias y trazabilidad. Úsela antes de dar por cerrada una versión.
---

# Publicar una versión nueva del SIF

Reglas que no cambian: pruebas manuales sobre `tauri dev`; git solo local, sin push, PR ni CI de GitHub; datos y certificados sintéticos. **Esta skill no construye el instalador.** El instalador (`scripts/build-clean.ps1`, `tauri build`, NSIS) solo se construye para la versión final o si el usuario lo pide expresamente; cerrar una etapa o añadir commits no son motivo, ni proponerlo. Si la versión no es la final, la skill termina con el código, la declaración y las comprobaciones, sin paquete.

Por qué importa: el art. 13 del RD 1007/2023 exige que la declaración responsable conste de modo visible en el sistema **en cada una de sus versiones**, y que el productor conserve las de todas; poner una declaración en un producto que no cumple es sancionable (D277, art. 201 bis LGT). Todo cambio de versión debe quedar amparado por una declaración (FAQ de desarrolladores v1.3, §5).

Si un paso no se puede comprobar, dígalo; no lo dé por hecho.

## 1. Decidir si hay versión nueva

- [ ] Un cambio que toque los requisitos de integridad, trazabilidad, conservación, remisión, QR o el contenido de la declaración (R18, R35–R38) o un componente propio con ciclo vinculado cambia la versión del principal y exige declaración nueva. Una personalización que no toca esos requisitos no la exige (FAQ §5, §8, §9). En caso de duda, versión nueva.
- [ ] Antes de seguir, `python scripts/check.py` y la lista de la skill `revisar-cumplimiento-verifactu` sobre los cambios desde la versión anterior.

## 2. Versión del SIF

- [ ] Decida el identificador completo de la versión (el que verá el obligado y la AEAT en `SistemaInformatico/Version`; hoy `SIF_VERSION` en `crates/app/src/sales.rs` toma `CARGO_PKG_VERSION`). Cámbielo en un solo sitio y que el resto lo lea: `Cargo.toml` de cada crate del espacio de trabajo y `crates/desktop/tauri.conf.json` deben coincidir. `grep -n "^version" crates/*/Cargo.toml` y `grep -n '"version"' crates/desktop/tauri.conf.json`.
- [ ] El nombre del sistema (`NestAdmin`) y el identificador `NA` (D199) no cambian con la versión: la versión no cambia la identidad del SIF (FAQ §4). Si cambiaran, se trata como producto distinto y se decide antes en `specs/v1/decisiones.md`.
- [ ] Actualice `Cargo.lock` con `cargo check --workspace` y confirme que solo cambian los crates propios.

## 3. Productor compilado (D277)

- [ ] Nombre, NIF, dirección y lugar de suscripción del productor se fijan al compilar la versión; no salen de `verifactu-productor.json` (ese archivo solo se lee con `cfg(debug_assertions)`, D203). Compruebe que una compilación de publicación ignora el archivo y que `docs/verifactu-productor.ejemplo.json` sigue con marcadores `PONER-AQUI`.
- [ ] El NIF del productor es válido (`nestadmin_sales::validate_tax_id`) y no es un marcador. Los datos reales del productor los aporta el usuario; no los invente ni los copie de otra fuente.
- [ ] Los campos de `SistemaInformatico` de la versión (`TipoUsoPosibleSoloVerifactu = S`, `TipoUsoPosibleMultiOT = S`) coinciden con la declaración (art. 15.1.e y f de la Orden).

## 4. Declaración responsable de la versión (12 apartados)

Escriba o actualice la declaración de **esta versión** (se conserva la de todas las anteriores, art. 13.3 RRSIF). Debe llevar el título «DECLARACIÓN RESPONSABLE DEL SISTEMA INFORMÁTICO DE FACTURACIÓN» y los apartados del art. 15.1 de la Orden HAC/1177/2024:

- [ ] a) nombre del sistema; b) código identificador (`NA`); c) identificador completo de la versión;
- [ ] d) componentes de hardware y software, con descripción breve y funcionalidades (revise el inventario del punto 5 y los componentes de otros productores, cada uno con su declaración por versiones, art. 15.4);
- [ ] e) solo VERI\*FACTU; f) varios obligados; g) tipos de firma de NO VERI\*FACTU (no aplica: NestAdmin no ofrece esa modalidad, D71; dígalo expresamente);
- [ ] h) nombre o razón social del productor; i) NIF; j) dirección postal completa;
- [ ] k) declaración de cumplimiento del art. 29.2.j de la LGT, del RD 1007/2023, de la Orden y de las especificaciones de la sede, **para la versión indicada**;
- [ ] l) fecha (día, mes y año) y lugar (localidad y país) de suscripción, que fija el productor al firmarla, no el código.

Después:

- [ ] La declaración está en el binario (no es un archivo editable por el cliente) y se muestra en «Acerca de», exportable a PDF; al arrancar la aplicación comprueba que existe la de su versión exacta y, si falta, la remisión a producción queda bloqueada (prueba de esa ruta, D276, D277).
- [ ] El historial de declaraciones incluye la versión anterior y la nueva, ordenadas, y se pueden consultar todas desde la aplicación.
- [ ] El contenido final de la declaración, su firma y la decisión de publicarla los valida el productor o su asesor (D86); la aplicación no lo hace por ellos y esta skill no la da por validada. Si el usuario aún no la ha revisado, la versión no se publica y se anota como pendiente.

## 5. Inventario de terceros (D60)

- [ ] `python scripts/third_party.py` regenera `THIRD_PARTY_NOTICES.md` desde `Cargo.lock` y `ui/package-lock.json`; después `python scripts/third_party.py --check` debe pasar. Revise las dependencias nuevas de la versión (en T18b: `rustls-cng`, `webpki-roots`, `x509-parser` y las de desarrollo `rcgen` y `p12-keystore`) y su licencia declarada.
- [ ] Las dependencias de desarrollo no deben salir en el inventario del binario; compruébelo.
- [ ] `cargo tree -i aws-lc-rs` sale vacío y `cargo tree -d` muestra un solo `rustls` (D279).
- [ ] Lo que depende del instalador (`vendor/THIRD_PARTY_NOTICES.txt` con `--textos`) se hace solo en la versión final; no lo ejecute antes.

## 6. Pruebas, evidencias y trazabilidad

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` y, si toca la interfaz, `npm test` y `npm run build` en `ui/`; después `python scripts/check.py`. Anote la salida literal, incluidos fallos y pasos omitidos.
- [ ] Recorrido en `tauri dev` con datos sintéticos de lo que cambió, con capturas y un resumen en `evidence/` (por ejemplo `evidence/t18-<tema>-tauri-dev.md`).
- [ ] Actualice la matriz `specs/v1/trazabilidad.md` (filas T18b–T18g y las que toque) solo con evidencia reproducible: enlace a la evidencia, versión, fecha y comandos. No marque `Cumplido` lo que depende de la preproducción real o de hardware.
- [ ] Actualice `PLAN.md` y `specs/v1/tareas.md` si cambia el alcance, y anote el cambio en `specs/v1/decisiones.md` (con número del bloque reservado en `numeracion.md`).

## 7. Cierre

Commit local en español con el trabajo de la versión; nada de push ni PR. Informe: versión, archivos cambiados, estado de la declaración (pendiente de validación del productor si lo está), resultado literal de las comprobaciones y lo que no se pudo verificar. Recuerde que el instalador y el ensayo en preproducción (skill `ensayo-preproduccion-verifactu`) quedan para la versión final o cuando el usuario lo pida.
