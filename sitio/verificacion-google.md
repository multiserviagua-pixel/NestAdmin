# Material para la verificación de Google OAuth (Gmail API)

Textos en inglés para la consola de Google Cloud («Verification Center» / pantalla de consentimiento) y guion del vídeo de demostración. No contienen datos reales: el vídeo debe grabarse solo con cuentas y correos de prueba. Este documento no acredita que la verificación se haya solicitado ni superado.

## Justificación del ámbito `gmail.modify`

Llamadas que hace hoy el conector (`crates/mail/src/gmail.rs`): `users.messages.list` (bandeja de entrada), `users.messages.get`, `users.drafts.create` (respuesta en el hilo), `users.drafts.send` y `users.drafts.get` (reconciliación de envíos, D33). Ninguna borra, archiva ni cambia etiquetas.

Texto para pegar (≈ 900 caracteres):

```text
NestAdmin is a Windows desktop app that helps small businesses handle incoming email. It reads messages in the user's inbox, creates a reply draft inside the original thread, and sends that draft only after the user reviews and approves it in the app, or when a rule the user explicitly created and enabled applies. gmail.readonly cannot create or send drafts, and gmail.compose cannot read inbox messages, so neither alone supports this read-draft-send flow. gmail.modify covers it with a single scope; NestAdmin never uses it to delete, archive or relabel mail. All processing happens locally on the user's computer: Gmail data travels only between the user's device and Google, and the developer operates no servers that receive, store or process it. Users can disconnect the account in the app or revoke access at myaccount.google.com/permissions.
```

> **Para el editor:** la combinación `gmail.readonly` + `gmail.compose` cubriría exactamente las mismas llamadas, sin la capacidad de cambiar etiquetas o mover a la papelera que concede `gmail.modify`. Google puede pedir esa combinación como alternativa más restringida. Si lo pide, cambiar el ámbito afecta a D31 y a `crates/mail/src/oauth.rs`, y habría que actualizar la política de privacidad, este texto y el vídeo.

## Guion del vídeo de demostración (YouTube, «no listado»)

Requisitos previos:

- Una cuenta de Gmail **de prueba** creada para la demostración, añadida como usuario de prueba en el proyecto de Google Cloud, y otra cuenta de prueba que le envíe un correo sintético (por ejemplo, una solicitud de presupuesto inventada).
- La aplicación compilada con el `client_id` de producción del proyecto que se va a verificar (el mismo que figura en la solicitud).
- Navegador con la barra de direcciones visible y sin otras pestañas ni marcadores personales. Grabar en inglés o con subtítulos en inglés.

Texto de narración o subtítulos (en inglés):

1. **Intro (0:00).** "This is NestAdmin, a Windows desktop app for small businesses. This video shows how it uses the Gmail scope https://www.googleapis.com/auth/gmail.modify. Only test accounts and test emails are used."
2. **Start sign-in (0:15).** Open NestAdmin → Mail → Add account → Google. "The app opens Google's sign-in page in the default browser."
3. **Consent screen (0:30).** Stay on the Google page and **zoom into the address bar** so the full URL is readable, including `client_id=…`, `redirect_uri=http://127.0.0.1:…`, `scope=https://www.googleapis.com/auth/gmail.modify` and `code_challenge_method=S256`. "The OAuth client ID shown here is the one submitted for verification. The only scope requested is gmail.modify." Choose the test account, show the app name «NestAdmin», the requested permission and the links to the privacy policy and terms, then click Allow.
4. **Back in the app (1:15).** "The browser shows that sign-in is complete. Tokens are stored in Windows Credential Manager." Show the account listed as connected.
5. **Read a test email (1:30).** Click "Check mail". Open the synthetic test message. "NestAdmin reads inbox messages to show and classify them. Processing uses a local AI model on this computer."
6. **Draft (2:00).** Show the reply draft that NestAdmin prepared. Switch to Gmail on the web (test account) → Drafts, and show that the draft is inside the original thread. "The draft is created in the same thread. Nothing has been sent yet."
7. **Review (2:30).** Back in NestAdmin, open the review view, edit one sentence of the draft. "Sending requires the user's review and approval by default."
8. **Approved send (2:50).** Click Approve and send. Show the action in the app history. In Gmail on the web, show the message in Sent and in the thread. "The draft is sent only after approval. NestAdmin never deletes email."
9. **Disconnect (3:20).** In NestAdmin → Mail, click Disconnect on the account and confirm. "Disconnecting removes the tokens from this computer." Then open https://myaccount.google.com/permissions, show NestAdmin and click Remove access. "Users can also revoke access from their Google Account at any time."
10. **Close (3:50).** "The privacy policy is at https://nestadmin.humannaia.com/en/privacy/. Thank you."

Comprobaciones antes de subir el vídeo:

- [ ] La URL completa de la pantalla de consentimiento y el `client_id` se leen en el vídeo.
- [ ] El nombre de la app en la pantalla de consentimiento es exactamente «NestAdmin».
- [ ] No aparece ningún dato real (correos, nombres, direcciones, tokens, claves) ni el contenido de otras pestañas o notificaciones.
- [ ] Se ven las tres funciones que usan el ámbito: leer, crear borrador y enviar tras aprobación.
- [ ] El vídeo es «no listado» (no privado), para que el equipo de Google pueda verlo.
