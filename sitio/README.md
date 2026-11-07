# Sitio web de NestAdmin

Sitio estático para `https://nestadmin.humannaia.com` (dominio con doble «n»). Existe para la verificación de marca y de ámbitos restringidos de Google OAuth (Gmail API) y la verificación de editor de Microsoft (D47 en [decisiones.md](../specs/v1/decisiones.md)). No forma parte de la aplicación ni cambia sus requisitos.

- Solo HTML y CSS: sin JavaScript, cookies, analítica, fuentes externas ni formularios. Se lee sin iniciar sesión y se adapta al móvil; respeta el modo oscuro del sistema.
- Rutas relativas: funciona en cualquier alojamiento estático y abriendo los archivos en local.
- **No está publicado.** Publicarlo es decisión del editor.

## Páginas

| Página | Español | Inglés |
| --- | --- | --- |
| Inicio | [`index.html`](index.html) → `/` | [`en/index.html`](en/index.html) → `/en/` |
| Política de privacidad | [`privacidad/`](privacidad/index.html) | [`en/privacy/`](en/privacy/index.html) |
| Condiciones del servicio | [`condiciones/`](condiciones/index.html) | [`en/terms/`](en/terms/index.html) |
| Soporte | [`soporte/`](soporte/index.html) | [`en/support/`](en/support/index.html) |

Todas enlazan privacidad y condiciones en la cabecera y en el pie. [`verificacion-google.md`](verificacion-google.md) contiene la justificación del ámbito `gmail.modify` y el guion del vídeo; no se publica (ver «Despliegue»).

## Marcadores que hay que rellenar antes de publicar

Los datos legales y de contacto no se han inventado. Sustituya en todos los `.html`:

| Marcador | Qué poner |
| --- | --- |
| `[RAZÓN SOCIAL]` | Nombre legal del editor (persona o empresa) |
| `[PAÍS Y DIRECCIÓN]` | País y dirección postal del editor |
| `[CORREO]@humannaia.com` | Buzón de contacto y soporte, que debe existir y recibir correo (también en los enlaces `mailto:`) |
| `[FECHA DE VIGENCIA]` | Fecha desde la que rigen la política y las condiciones |
| `[LEY APLICABLE]` | Legislación y, si procede, tribunales competentes (solo en condiciones) |

Para comprobar que no queda ninguno:

```bash
grep -rn "\[RAZÓN SOCIAL\]\|\[PAÍS Y DIRECCIÓN\]\|\[CORREO\]\|\[FECHA DE VIGENCIA\]\|\[LEY APLICABLE\]" sitio --include=*.html
```

Revise también los textos legales con quien asesore al editor: son una base fiel a lo que hace la aplicación, no asesoramiento jurídico.

## Lo que el sitio afirma y de dónde sale

Cada afirmación sobre la aplicación se apoya en la especificación o el código; si la aplicación cambia, actualice el sitio:

- Procesamiento local, Ollama con Qwen3 8B por defecto y Groq solo por elección expresa sin cambio automático: R03, D06, D07.
- Credenciales en el Administrador de credenciales de Windows y fuera de los registros: R09, D08, D13.
- OAuth con PKCE, bucle local `127.0.0.1`, ámbitos `gmail.modify` y Graph `offline_access User.Read Mail.ReadWrite Mail.Send` (`User.Read` solo para la dirección del buzón, D48), solo HTTPS y sin proxy: D31, D32, D12.
- Lectura de la bandeja de entrada, cuerpo en texto, adjuntos ignorados, borrador en el hilo: D34 y `crates/mail/src/gmail.rs`.
- Envío solo con revisión o regla activa: R07, D17. Un envío por clave: D33, D41.
- Datos en `%LOCALAPPDATA%\NestAdmin`, que el desinstalador conserva: D18, D36.
- Desconectar una cuenta desde la ventana borra sus tokens: D45, en el PR de correo real en la aplicación (pendiente de fusionar cuando se escribió esto). **No publique el sitio ni grabe el vídeo hasta que esa función esté en la versión que se distribuye.**
- Requisitos de Windows 10/11 de 64 bits y 16 GB de RAM recomendados: R12 y el brief del editor.

## Despliegue

Sirve cualquier alojamiento estático con HTTPS (GitHub Pages, Cloudflare Pages, Netlify, el hosting del dominio…):

1. Rellene los marcadores.
2. Publique el **contenido** de `sitio/` como raíz del sitio, excluyendo `README.md` y `verificacion-google.md` (son documentación interna). Deben responder `https://nestadmin.humannaia.com/`, `/privacidad/`, `/condiciones/`, `/soporte/`, `/en/`, `/en/privacy/`, `/en/terms/` y `/en/support/`.
3. En el DNS de `humannaia.com`, cree para `nestadmin` el registro que indique el alojamiento (normalmente un `CNAME`) y active HTTPS.
4. Compruebe desde una ventana privada que las páginas cargan sin iniciar sesión, sin redirecciones a otro dominio y sin avisos de certificado.

## Verificación del dominio en Google Search Console

Google exige que el dominio de la página principal, la política y las condiciones esté verificado por un propietario o editor del proyecto de Google Cloud.

1. Con la cuenta de Google que administra el proyecto, entre en [Google Search Console](https://search.google.com/search-console) → Añadir propiedad → **Dominio** → `humannaia.com`.
2. Añada en el DNS de `humannaia.com` el registro `TXT` `google-site-verification=…` que muestra la consola y pulse Verificar (la propagación puede tardar).
3. Si la cuenta que verifica no es la del proyecto, añada la del proyecto como propietario en Search Console.

## Campos de la pantalla de consentimiento (Google Cloud → Google Auth Platform)

| Campo | Valor |
| --- | --- |
| Nombre de la aplicación | `NestAdmin` |
| Correo de asistencia al usuario | `[CORREO]@humannaia.com` |
| Página principal de la aplicación | `https://nestadmin.humannaia.com/` |
| Política de privacidad | `https://nestadmin.humannaia.com/privacidad/` (o `/en/privacy/`) |
| Condiciones del servicio | `https://nestadmin.humannaia.com/condiciones/` (o `/en/terms/`) |
| Dominios autorizados | `humannaia.com` |
| Ámbitos | `https://www.googleapis.com/auth/gmail.modify` |
| Justificación y vídeo | [`verificacion-google.md`](verificacion-google.md) |

El logotipo, si se sube, también pasa la verificación de marca; no se incluye ninguno en el sitio.

## Verificación de editor de Microsoft

En el registro de la aplicación de Microsoft Entra: **Personalización de marca y propiedades** → URL de la página principal `https://nestadmin.humannaia.com/`, términos `https://nestadmin.humannaia.com/en/terms/`, privacidad `https://nestadmin.humannaia.com/en/privacy/`. La verificación de editor exige además una cuenta de Microsoft Partner Network verificada y un dominio de editor verificado (Entra genera un archivo `microsoft-identity-association.json` que debe publicarse en `/.well-known/` del dominio elegido, por ejemplo `https://nestadmin.humannaia.com/.well-known/microsoft-identity-association.json`); esos pasos dependen de la cuenta del editor y no se cubren aquí.
