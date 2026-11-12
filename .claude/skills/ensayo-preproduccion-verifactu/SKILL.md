---
name: ensayo-preproduccion-verifactu
description: Guion del ensayo final de NestAdmin en la preproducción de la AEAT, desde el perfil de pruebas, con el certificado del fabricante y datos sintéticos. Solo se ejecuta en la versión final o cuando el usuario lo pida expresamente.
---

# Ensayo en preproducción de la AEAT (T18b)

**Cuándo se ejecuta.** Solo en la versión final, o cuando el usuario lo pida expresamente en el chat. No se ejecuta como parte de una etapa, de un cierre ni de una revisión, y esta skill no lo propone por iniciativa propia. Las pruebas automáticas usan el simulador con TLS mutuo (skill `certificados-de-prueba`) y nunca contactan con la AEAT (D85, D285). La característica de compilación de red real (D285, nombre previsto `aeat-real`) está **desactivada por defecto**: sin ella la aplicación no puede hablar con la AEAT, y solo se activa para este ensayo, en una compilación de desarrollo (`tauri dev`), nunca en las pruebas automáticas ni en el paquete salvo decisión expresa del usuario para la versión final.

Reglas que no cambian: pruebas manuales sobre `tauri dev`; el instalador (`scripts/build-clean.ps1`, `tauri build`, NSIS) solo se construye para la versión final o si el usuario lo pide, y no es requisito de este ensayo; git solo local; nada de datos reales de clientes ni de personas.

## Antes de empezar (el usuario aporta lo que no es del repositorio)

- [ ] El usuario ha pedido el ensayo y ha autorizado el uso de su certificado del fabricante. **El certificado real no se copia al repositorio, a fixtures, a capturas ni a evidencias, y nunca se escribe su contenido ni su contraseña en ningún informe.** Se usa desde el almacén de Windows o importándolo con la aplicación (D278). Si el certificado pide PIN, lo teclea el usuario.
- [ ] Conocido el tipo del certificado del fabricante (persona física, representante o sello) y los NIF que constan en él. Para cubrir sello y persona física hacen falta ambos tipos; si solo hay uno, se anota qué parte del guion queda sin ensayar.
- [ ] Los datos de las facturas son sintéticos (clientes ficticios con NIF de letra de control válida, importes de prueba, textos reconocibles). El obligado de los registros es el titular del certificado o aquel para el que el certificado tiene apoderamiento; si el NIF no coincide, el usuario declara su calidad (D278) y se anota. La AEAT no publica NIF de prueba y los de sus ejemplos son solo ilustrativos; el portal de pruebas (`preportal.aeat.es`) exige autenticarse con certificado.
- [ ] Prohibidas las pruebas masivas: el portal puede bloquear el uso abusivo. El guion usa unas pocas facturas.
- [ ] Reloj del equipo correcto (el art. 7.i exige que no supere en más de 1 minuto el último registro).
- [ ] Estado de partida limpio: `git status` sin cambios sin guardar, `python scripts/check.py` y `cargo test --workspace` en verde sobre la rama.

## Preparación

1. [ ] Compile y arranque con `tauri dev` activando la característica de red real. Abra el **perfil de pruebas** desde la aplicación (D271): debe verse su aviso permanente. Preproducción solo existe en este perfil (D275); en el perfil normal, no.
2. [ ] Cree el obligado de pruebas (D273) y vincule el certificado (D278). La validación debe mostrar el tipo, la vigencia y el NIF; anote si salta el aviso de NIF distinto y declare la calidad del remitente.
3. [ ] Elija el entorno `preproduccion`. Compruebe que los endpoints son los constantes de D279 (personal: `prewww1.aeat.es`; sello: `prewww10.aeat.es`) y que la aplicación no ofrece cambiarlos.
4. [ ] Cree el cuadro de anotación (ver «Qué se anota») antes de la primera remisión.

## Guion

Para cada paso anote la hora, el identificador del lote, el estado de cada registro y los **códigos y descripciones reales** de la AEAT. No pegue en la evidencia el XML recibido con datos del certificado ni cabeceras con identificación.

1. **Alta.** Emita una factura completa y espere la remisión (o «Remitir ahora»). Resultado esperado: aceptada, con CSV. Anote el código y la fecha.
2. **Altas en lote.** Emita tres o cuatro facturas más (completa, simplificada y una con IVA mixto) y compruebe que van en un solo lote, que se respeta `TiempoEsperaEnvio` (60 s inicial) y que el estado derivado es el correcto.
3. **Cotejo del QR.** Abra el QR de cada factura de pruebas en `https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR?...` (la URL del propio QR en el entorno de pruebas) y anote el resultado del cotejo. Verifique que el QR del PDF es el de pruebas y no el de producción (D248).
4. **Anulación.** Anule una de las facturas aceptadas. La anulación va en un lote distinto del alta (D257). Resultado esperado: aceptada. Anote también qué hace la AEAT al repetir la baja (código 3001) para contrastarlo con el simulador (D252).
5. **Rechazo.** Provoque un rechazo legítimo con datos de prueba (por ejemplo un NIF de destinatario no censado con tipo distinto de 07, o el otro caso de rechazo que documente la AEAT que no obligue a prueba masiva). Anote el código y la descripción reales y compruebe que el catálogo en lenguaje llano (D286) explica ese código.
6. **Subsanación.** Subsane el rechazado con «Subsanar» (D253): alta con `Subsanacion = S` y `RechazoPrevio = X`. Anote la respuesta de la AEAT y compruebe el estado derivado `subsanated`/aceptado.
7. **Aceptado con errores.** Si algún paso devuelve «aceptado con errores», anótelo y compruebe cómo lo muestra la aplicación.
8. **Sello frente a persona física.** Si dispone de ambos certificados, repita el alta con cada uno: la persona física o el representante entran por `prewww1` y el sello por `prewww10` (D279). Anote si cada certificado es admitido por su endpoint y el comportamiento al usar el endpoint contrario (debería ser un error de autorización, no un envío válido). Compruebe que la aplicación elige el endpoint por el tipo del certificado.
9. **`Representante`.** Configure expresamente el representante del obligado (D283) con los datos del remitente y remita un alta. Anote la respuesta y los códigos 4105, 4117, 4123 o 4124 si aparecen. Compruebe también que sin configurarlo no se envía (nunca se deduce).
10. **Autorización.** Con un certificado que no esté autorizado para ese obligado, o con un NIF de obligado al que el certificado no represente, compruebe el 4112: la remisión de ese obligado se detiene con `authorization_rejected` y no se reintenta (D281). Restablezca el certificado correcto y continúe.
11. **`Incidencia`.** Simule un fallo técnico (sin red durante la remisión, con el equipo desconectado) y vuelva a conectar: el lote abierto se reenvía con los mismos bytes y sin duplicar (D252, D280); los registros emitidos durante la incidencia hacen que el **siguiente** lote nuevo se abra con `Incidencia = S` (D282). Contraste con lo que la AEAT acepta o rechaza y anote si objeta el `Incidencia = S` en un lote reabierto o la ausencia en uno reenviado (desviación consciente de D282).
12. **Duplicado.** Provoque el reenvío de un lote ya respondido (cortando antes de recibir la respuesta, con la aplicación) y anote el código de duplicado real (3000) y cómo trata la aplicación el `unexpected_duplicate` (D252).
13. **Varios obligados (si se dispone de dos).** Remita para dos obligados con sus lotes y compruebe que nunca se mezclan (D273, D257) y que `IndicadorMultiplesOT` pasa a `S`.
14. **Reloj y cadena.** Compruebe que la cadena de cada obligado se verifica antes de cada registro y que un reloj atrasado detiene la emisión sin consumir número (D195, D284).

## Qué se anota (y qué no)

Un cuadro en `evidence/t18b-preproduccion.md` (se crea al ejecutar el ensayo; no existe antes) con, por paso: fecha, versión de la aplicación, entorno, tipo de certificado (sin su contenido ni sus datos), resultado y el código y la descripción de la AEAT. Solo recuentos, códigos y descripciones públicas de la AEAT; ni el certificado, ni la contraseña, ni NIF ni nombres reales, ni capturas que los muestren. Después:

- [ ] Compare cada código real con lo que hace el simulador (`crates/verifactu`, D253, DV-17) y anote las diferencias: respuestas de duplicado, baja repetida, aceptado con errores, rechazo de subsanación, `Fault` de autorización, nombre y orden de los campos de la respuesta.
- [ ] Cada diferencia se convierte en un cambio del simulador, de la clasificación de `Fault` (D281) o de la decisión que corresponda, con su número en el bloque reservado de `specs/v1/numeracion.md`; no se corrige solo la prueba.
- [ ] Actualice la fila T18b de `specs/v1/trazabilidad.md` con el estado real (`Parcial` o `Cumplido`) y el enlace a la evidencia. Lo que no se pudo ensayar (un certificado de sello, la tarjeta, el PIN) se deja como **no probado** y no se marca cumplido.
- [ ] Anule en preproducción todas las facturas de prueba que hayan quedado aceptadas (la FAQ de integridad exige anular y remitir cada factura de prueba), y deje el perfil de pruebas con su aviso.

## Después del ensayo

- [ ] Compruebe que el perfil normal no se tocó: ningún dato de prueba, ningún obligado ficticio (D271, consulta DGT V1042-26).
- [ ] Desactive de nuevo la característica de red real y confirme que `cargo test --workspace` pasa sin ella.
- [ ] Revise `git status`: ninguna salida del ensayo con datos del certificado, ningún PFX, ninguna captura con NIF reales.
- [ ] Informe: pasos ejecutados y no ejecutados, códigos reales anotados, diferencias con el simulador, y lo que queda para la revisión fiscal externa antes de activar producción. No construya el instalador ni active producción por su cuenta: la barrera de remisión (D276) y la aceptación del obligado son del usuario.
