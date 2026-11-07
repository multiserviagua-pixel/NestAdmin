# Fuentes oficiales de `nestadmin-verifactu`

Documentación técnica de la AEAT y del W3C usada para el crate, tomada el 06/11/2026 de la
sección «Sistemas Informáticos de Facturación y VERI\*FACTU» de la sede electrónica
(`sede.agenciatributaria.gob.es/Sede/iva/sistemas-informaticos-facturacion-verifactu/informacion-tecnica.html`,
página actualizada el 13/10/2026). «Última modificación» es la cabecera `Last-Modified` del
servidor el día de la consulta. Los SHA-256 permiten detectar si la AEAT publica una versión
nueva; cuando cambien, revise el crate antes de actualizarlos.

Los documentos no se versionan en el repositorio. El aviso legal de la AEAT (apartado
«Propiedad intelectual») reserva la reproducción y distribución, por lo que tampoco se copian
los XSD: `tests/xsd/obtener.ps1` los descarga y comprueba su SHA-256.

## Documentos

| Documento | Versión y fecha | Última modificación | URL | SHA-256 |
| --- | --- | --- | --- | --- |
| Especificaciones técnicas para la generación de la huella o hash de los registros de facturación | 0.1.2, 27/08/2024 | 08/10/2024 | `https://www.agenciatributaria.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/Veri-Factu_especificaciones_huella_hash_registros.pdf` | `f4334c254bb875b417247b54315199f89d75a8c4814dfd1e86efec562653d7de` |
| Detalle de las especificaciones técnicas del código «QR» de la factura y de la «URL» del servicio de cotejo | 0.5.0, 10/12/2025 | 18/12/2025 | `https://www.agenciatributaria.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/DetalleEspecificacTecnCodigoQRfactura.pdf` | `f86b3c260d8a4963dbc18c5007732b53199156c5d1db63242e68db71501b49eb` |
| Sistemas Informáticos de Facturación: remisión voluntaria y bajo requerimiento (descripción del servicio web) | 1.0.3, 28/07/2025 | 14/10/2025 | `https://sede.agenciatributaria.gob.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/Veri-Factu_Descripcion_SWeb.pdf` | `b3570f6a308ce98a5f52001a0dc427310ad6cf7bccd60a9ee98720a59e553c02` |
| Validaciones y errores (Sistemas Informáticos de Facturación y VERI\*FACTU) | 1.2.2, 08/04/2026 | 08/04/2026 | `https://www.agenciatributaria.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/Validaciones_Errores_Veri-Factu.pdf` | `426eb926fc098a36a163f66ca5f40d9e0847ca23300bbe5008979832d3513440` |
| Lista de códigos de error (`errores.properties`) | sin versión | 30/07/2026 | `https://prewww2.aeat.es/static_files/common/internet/dep/aplicaciones/es/aeat/tikeV1.0/cont/ws/errores.properties` | `06519ceb23422bd6b0ad3bfb659e3007615050da4920781d12cff536481d5902` |
| Aclaraciones a dudas de los desarrolladores (FAQ) | 1.3, 04/12/2025 | 10/12/2025 | `https://sede.agenciatributaria.gob.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/FAQs-Desarrolladores.pdf` | `73906dc8afbbb9da35f6cb489980352b42aed66d48828fd62a00168883c09d5e` |

## Esquemas

Base de la AEAT: `https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/`.
Los XSD no llevan número de versión propio; el diseño de registro es `IDVersion = 1.0`.

| Esquema | Última modificación | SHA-256 |
| --- | --- | --- |
| `SuministroLR.xsd` | 01/10/2025 | `cbdac8d427cc5ab5d77ca48974cab0f35d6bb819c4c66db361681e3710aeba36` |
| `SuministroInformacion.xsd` | 11/01/2026 | `ee4c1655175644de44c4c25055ffeb8e5f4bb4bc3834ce8254d4222ef18c8aa1` |
| `RespuestaSuministro.xsd` | 01/10/2025 | `82acf80f785643caac13087aae66808ed721a13f08ca5218cf8ae81b695549ef` |
| `SistemaFacturacion.wsdl` (solo consultado: URL de los servicios) | 05/11/2025 | — |
| `xmldsig-core-schema.xsd` (W3C, `https://www.w3.org/TR/xmldsig-core/xmldsig-core-schema.xsd`, rev. 1.2) | 16/04/2013 | `d102ad3df7664c307e0c2c776ba4a90513b1969974d8a940bae1a77f9f21e15d` |

`SuministroInformacion.xsd` declara UTF-8 pero contiene comentarios en Latin-1;
`tests/xsd/validar.ps1` lo lee como Latin-1, lo que solo afecta a esos comentarios.

## Qué se ha tomado de cada fuente

**Huella (0.1.2).** Campos y orden: alta = `IDEmisorFactura`, `NumSerieFactura`,
`FechaExpedicionFactura`, `TipoFactura`, `CuotaTotal`, `ImporteTotal`, `Huella` (del
registro anterior), `FechaHoraHusoGenRegistro`; anulación = `IDEmisorFacturaAnulada`,
`NumSerieFacturaAnulada`, `FechaExpedicionFacturaAnulada`, `Huella`,
`FechaHoraHusoGenRegistro`. Formato `nombre=valor&…`, valores sin espacios al inicio ni al
final, campo ausente como `nombre=`, UTF-8, SHA-256 (lista L12, `TipoHuella = 01`), salida
hexadecimal en mayúsculas de 64 caracteres. Los tres ejemplos del apartado 6 son las pruebas
`vectores_oficiales_*` de `tests/huella.rs`.

**XSD.** Estructura y orden de `RegFactuSistemaFacturacion` (`Cabecera` y de 1 a 1000
`RegistroFactura`), `RegistroAlta`, `RegistroAnulacion`, `Encadenamiento`
(`PrimerRegistro = S` o `RegistroAnterior`), `SistemaInformatico`, tipos de factura `F1`,
`F2`, `F3`, `R1`–`R5`, formatos `fecha` (`DD-MM-AAAA`), `ImporteSgn12.2Type`, `Tipo2.2Type`,
`NIFType` y la respuesta (`EstadoEnvio`, `EstadoRegistro`, `TiempoEsperaEnvio`,
`RegistroDuplicado`).

**Servicio web (1.0.3).** SOAP 1.1 *document/literal*, UTF-8, HTTPS con certificado
cualificado; máximo de 1000 registros por envío; errores de esquema o de cabecera como
`Fault` con `Codigo[NNNN]` en `faultstring`; estados globales y por registro; el CSV hay que
guardarlo al recibirlo; control de flujo: tiempo de espera inicial de 60 s, actualizado en
cada respuesta, y envío cuando pasa ese tiempo o se acumulan 1000 registros, lo que ocurra
antes (art. 16.2 de la Orden HAC/1177/2024). URL del WSDL:
`https://www1.agenciatributaria.gob.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP`
(producción) y `https://prewww1.aeat.es/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP`
(pruebas); `www10`/`prewww10` son las variantes para certificado de sello.

**QR (0.5.0).** URL `https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR` (pruebas) y
`https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR` (producción) con los
parámetros obligatorios `nif`, `numserie`, `fecha` e `importe`, «URL encoding» en UTF-8,
ASCII imprimible en las cadenas; `formato=json` nunca en el QR; ISO/IEC 18004 con corrección
M, 30x30 a 40x40 mm, al menos 2 mm en blanco (6 recomendados), «QR tributario:» encima y
«VERI\*FACTU» (o «Factura verificable en la sede electrónica de la AEAT») debajo.

**Validaciones y errores (1.2.2) y `errores.properties`.** Códigos que reproduce el
simulador: 4102, 4103, 4104, 4114, 4116, 4135 y 4136 (`Fault`), 3000 (duplicado), 3001 (ya
anulado), 3002 (no existe), 2000 (huella incorrecta, registro aceptado con errores) y 1100
(error forzado de ejemplo).

**FAQ (1.3).** Comprobación antes de generar un registro (art. 7.i de la Orden): el último
registro está bien encadenado y su fecha-hora no es posterior en más de un minuto a la del
nuevo; la facturación no se interrumpe por ello.
