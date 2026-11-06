# Decisiones de detalle de `nestadmin-verifactu`

La especificación vigente de Verifactu es la de la fase 11a (PR #29,
`claude/fase-11a-erp-facturacion-spec`, decisiones D70–D90), dueña de `specs/v1/` y `PLAN.md`.
El crate implementa **D82** (registros, huella, cadena y QR), prepara **D83** (remisión: XML,
respuesta y control de flujo) y **D84** (simulador local). Las decisiones siguientes concretan
esas tres; no abren numeración propia. Al fusionar 11a deben añadirse como detalle de D82–D84 en
`specs/v1/decisiones.md` (o como decisiones nuevas tras la última vigente si 11a lo prefiere).
Cada una cita su fuente (`FUENTES.md`) y la prueba que la cubre.

Aviso de numeración (2026-11-06): `main` ya tiene D65–D70 (PR #28), con D70 «`http://localhost`
para Microsoft», y el PR #29 también usa D70 para «Facturación propia con VERI\*FACTU». El PR #26
usa D65–D67, ya ocupados en `main`. 11a debe renumerar antes de fusionar; si lo hace, las
referencias D82–D84 de este archivo se actualizan con ella.

| Detalle | Desarrolla | Decisión | Motivo y fuente | Prueba |
| --- | --- | --- | --- | --- |
| DV-1 | D82 | `crates/verifactu` es una biblioteca pura: sin base de datos, sin Tauri, sin red. El transporte HTTPS con certificado cualificado, la persistencia de la cadena y la cola de envíos quedan para otra fase. | Separar el formato oficial (verificable con vectores y XSD) de los efectos. | todo el crate |
| DV-2 | D82 | Los importes se guardan en céntimos (`i64`) y se escriben siempre con dos decimales y punto (`121.00`, `-5.00`); el mismo texto va al XML, a la huella y al QR. | La especificación de huella da por válidos uno o dos decimales; usar siempre el mismo literal evita discrepancias entre la huella propia y la que recalcula la AEAT. El ejemplo del QR usa `241.4`; se emite `241.40`, que cumple el formato `NNNNNNNNN.DD`. | `tests/huella.rs`, `tests/qr.rs` |
| DV-3 | D82 | La fecha-hora de generación se escribe siempre `AAAA-MM-DDThh:mm:ss±hh:mm`, sin fracciones ni `Z`, con el huso explícito. El huso lo decide quien llama (la aplicación, según la hora local de Windows). | Formato de los ejemplos oficiales de huella y del servicio web. | `tipos::tests`, `tests/huella.rs` |
| DV-4 | D82 | La huella se calcula sobre los valores recortados (sin espacios al inicio y al final), y el XML se escribe con el texto tal cual. | Especificación de huella, apartado 3. | `espacios_al_inicio_y_al_final_no_cuentan` |
| DV-5 | D82 | `verificar_cadena` exige que cada registro tenga la huella recalculada y que su `RegistroAnterior` identifique al registro previo con su huella; `PrimerRegistro` solo en el primero. Errores distintos para registro alterado, eslabón roto, primer registro inesperado y tramo sin inicio. | Detectar manipulación y registros perdidos o insertados. | `detecta_*`, `primer_registro_solo_al_principio` |
| DV-6 | D82 | Antes de generar un registro se comprueba el último: huella correcta y fecha-hora no posterior en más de un minuto a la nueva (`Error::RelojAtrasado`). Qué hacer ante el error (avisar, registrar un evento) lo decide la aplicación; la facturación no debe interrumpirse. | Orden HAC/1177/2024, art. 7.i, y FAQ 1.3. | `comprobacion_previa_a_generar` |
| DV-7 | D82 | `NumSerieFactura` solo admite ASCII imprimible (32-126). | La URL del QR solo admite esos caracteres en `numserie` (especificación del QR, apartado 4). | `validaciones_de_formato` |
| DV-8 | D82 | Los valores de la URL del QR se codifican en UTF-8 con `%XX` salvo los no reservados de RFC 3986; el espacio va como `%20`. | El documento exige «URL encoding» pero su ejemplo Java es una imagen; `%20` se interpreta igual en cualquier servidor. | `url_del_ejemplo_oficial` |
| DV-9 | D82 | El QR se genera con corrección M; el SVG mide 35 mm de símbolo con 6 mm de margen, con «QR tributario:» encima y «VERI\*FACTU» debajo; el PNG lleva la misma composición, zona en blanco de 4 módulos y densidad `pHYs` para 35 mm. Los textos del PNG se dibujan con un glifo propio de 5x7, sin fuentes del sistema. | Especificación del QR, apartados 1 a 3. | `tests/qr.rs` |
| DV-10 | D83 | La respuesta se lee por nombre local (los prefijos no importan) y se rechaza cualquier `DOCTYPE`. `TiempoEsperaEnvio` ausente es un error explícito; vacío (lo permite `Tipo6Type`) se trata como 60 s. | No procesar entidades de terceros; la ausencia de datos debe ser explícita (`AGENTS.md`). | `tests/respuesta.rs` |
| DV-11 | D83 | El `Fault` SOAP se devuelve como `ResultadoEnvio::Fallo` con el código de `Codigo[NNNN]` si viene. Un HTTP distinto de 200/500 o un cuerpo que no es SOAP es un error de transporte, no una respuesta de la AEAT. | Servicio web 1.0.3, apartado 5.1. | `fallo_soap_con_codigo`, `errores_forzados_y_fallo_http` |
| DV-12 | D83 | Recuperación sin efectos duplicados: si se pierde la respuesta, el reintento del mismo envío recibe 3000 con `RegistroDuplicado`; la aplicación debe tratar un duplicado cuyo estado es `Correcta` o `AceptadaConErrores` y cuya huella coincide como «ya registrado», no como rechazo. | Respuesta de duplicados (XSD, `RegistroDuplicadoType`; error 3000). | `reintento_tras_respuesta_perdida_detecta_duplicado` |
| DV-13 | D83 | Control de flujo: tras cada respuesta se respeta `TiempoEsperaEnvio` salvo que haya 1000 registros pendientes. El simulador solo cuenta los envíos anticipados porque la AEAT no publica cómo responde a ellos. | Orden HAC/1177/2024, art. 16.2; servicio web 1.0.3, 6.4.4.1. | `cuenta_envios_anticipados` |
| DV-14 | D82 | Los XSD de la AEAT no se versionan (su aviso legal reserva la distribución): `tests/xsd/obtener.ps1` los descarga y comprueba el SHA-256; la validación usa `System.Xml` de .NET y sus pruebas son `#[ignore]`. | Aviso legal de la sede, «Propiedad intelectual». | `tests/xml.rs` (`--ignored`) |
| DV-15 | D82, D60 | Versiones fijadas: `quick-xml =0.42.0`, `qrcode =0.14.1` (sin `image`), `png =0.18.1`. `THIRD_PARTY_NOTICES.md` no cambia mientras el crate no sea dependencia de `nestadmin-desktop`; cambiará cuando se enlace. | Reproducibilidad (D60). | `python scripts/third_party.py --check` |
| DV-16 | D84 | El simulador escucha solo en `127.0.0.1` y reproduce únicamente comportamientos documentados (códigos 2000, 3000-3002, 4102-4104, 4114, 4116, 4135, 4136). No comprueba firma, certificado ni censo real. | `AGENTS.md`: sin red real ni datos personales. | `tests/simulador.rs` |

## Fuera de alcance, para fases siguientes

- Registros de evento (sistemas no VERI\*FACTU) y firma XAdES: no aplican a la modalidad
  VERI\*FACTU elegida.
- Consulta de registros (`ConsultaLR`), remisión por requerimiento y subsanación guiada.
- Transporte con certificado cualificado, almacén de la cadena y cola durable de envíos.
