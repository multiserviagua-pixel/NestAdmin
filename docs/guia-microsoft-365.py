"""Genera docs/guia-microsoft-365.pdf: cómo conectar cuentas de Microsoft 365 a NestAdmin.

    python docs/guia-microsoft-365.py

Requiere reportlab y la fuente DejaVu Sans. El texto describe D31, D43, D48, D51, D66 y D70
tal como están; si cambian, se actualiza aquí y se vuelve a generar el PDF.
"""
from pathlib import Path

from reportlab.lib import colors
from reportlab.lib.enums import TA_LEFT
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import mm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import (
    KeepTogether, ListFlowable, ListItem, Paragraph, SimpleDocTemplate, Spacer,
    Table, TableStyle,
)

FONTS = "/usr/share/fonts/truetype/dejavu"
pdfmetrics.registerFont(TTFont("Sans", f"{FONTS}/DejaVuSans.ttf"))
pdfmetrics.registerFont(TTFont("Sans-Bold", f"{FONTS}/DejaVuSans-Bold.ttf"))
pdfmetrics.registerFont(TTFont("Mono", f"{FONTS}/DejaVuSansMono.ttf"))
pdfmetrics.registerFontFamily("Sans", normal="Sans", bold="Sans-Bold")

INK = colors.HexColor("#1f2933")
ACCENT = colors.HexColor("#0f5fa8")
SOFT = colors.HexColor("#eef4fa")
WARN = colors.HexColor("#fff4e0")
RULE = colors.HexColor("#c9d6e3")

body = ParagraphStyle("body", fontName="Sans", fontSize=9.6, leading=14, textColor=INK,
                      alignment=TA_LEFT, spaceAfter=5)
small = ParagraphStyle("small", parent=body, fontSize=8.4, leading=11.5)
title = ParagraphStyle("title", parent=body, fontName="Sans-Bold", fontSize=21, leading=26,
                       textColor=ACCENT, spaceAfter=4)
subtitle = ParagraphStyle("subtitle", parent=body, fontSize=11, leading=15,
                          textColor=colors.HexColor("#52606d"), spaceAfter=14)
h1 = ParagraphStyle("h1", parent=body, fontName="Sans-Bold", fontSize=13.5, leading=18,
                    textColor=ACCENT, spaceBefore=12, spaceAfter=6, keepWithNext=1)
h2 = ParagraphStyle("h2", parent=body, fontName="Sans-Bold", fontSize=10.5, leading=14,
                    spaceBefore=6, spaceAfter=3, keepWithNext=1)
code = ParagraphStyle("code", parent=body, fontName="Mono", fontSize=8.3, leading=11.5,
                      backColor=colors.HexColor("#f4f6f8"), borderPadding=(5, 6, 5, 6),
                      spaceBefore=3, spaceAfter=9, leftIndent=6, rightIndent=6)
cell = ParagraphStyle("cell", parent=body, fontSize=8.6, leading=11.5, spaceAfter=0)
cell_b = ParagraphStyle("cellb", parent=cell, fontName="Sans-Bold")


def p(text, style=body):
    return Paragraph(text, style)


def c(text):
    return f'<font name="Mono">{text}</font>'


def bullets(items, style=body):
    return ListFlowable([ListItem(p(t, style), leftIndent=12) for t in items],
                        bulletType="bullet", start="•", leftIndent=12, bulletFontName="Sans")


def steps(items):
    return ListFlowable([ListItem(p(t), leftIndent=16) for t in items],
                        bulletType="1", leftIndent=16, bulletFontName="Sans-Bold")


def box(text, background=SOFT, edge=ACCENT):
    t = Table([[p(text, body)]], colWidths=[170 * mm])
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), background),
        ("LINEBEFORE", (0, 0), (0, -1), 3, edge),
        ("LEFTPADDING", (0, 0), (-1, -1), 9), ("RIGHTPADDING", (0, 0), (-1, -1), 9),
        ("TOPPADDING", (0, 0), (-1, -1), 6), ("BOTTOMPADDING", (0, 0), (-1, -1), 4),
    ]))
    return KeepTogether([t, Spacer(1, 8)])


def table(rows, widths):
    data = [[p(x, cell_b if i == 0 else cell) for x in row] for i, row in enumerate(rows)]
    t = Table(data, colWidths=[w * mm for w in widths], repeatRows=1)
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, 0), SOFT),
        ("GRID", (0, 0), (-1, -1), 0.4, RULE),
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("LEFTPADDING", (0, 0), (-1, -1), 5), ("RIGHTPADDING", (0, 0), (-1, -1), 5),
        ("TOPPADDING", (0, 0), (-1, -1), 4), ("BOTTOMPADDING", (0, 0), (-1, -1), 4),
    ]))
    # Las tablas largas pueden partirse entre páginas (la cabecera se repite).
    return t if len(rows) > 5 else KeepTogether([t, Spacer(1, 8)])


def footer(canvas, doc):
    canvas.saveState()
    canvas.setFont("Sans", 7.5)
    canvas.setFillColor(colors.HexColor("#7b8794"))
    canvas.drawString(20 * mm, 12 * mm, "NestAdmin · Guía de conexión de Microsoft 365 · noviembre de 2026")
    canvas.drawRightString(190 * mm, 12 * mm, f"Página {doc.page}")
    canvas.restoreState()


story = [
    p("Conectar cuentas de Microsoft 365 a NestAdmin", title),
    p("Registro de la aplicación en Microsoft Entra ID, configuración de NestAdmin y recorrido de "
      "prueba con una cuenta real (T13)", subtitle),
    box("<b>Antes de empezar.</b> Esta guía corresponde a la versión de NestAdmin que redirige el "
        f"inicio de sesión de Microsoft a {c('http://localhost')} (decisión D70, 6 de noviembre de "
        "2026). Una instalación anterior usaba 127.0.0.1 y podía fallar con el error AADSTS50011. "
        "Instale una versión construida después de ese cambio."),

    p("1. Qué necesita", h1),
    bullets([
        "Una cuenta con permiso para registrar aplicaciones en Microsoft Entra ID: administrador "
        "global, administrador de aplicaciones o un usuario al que el inquilino se lo permita.",
        "Una <b>cuenta de prueba</b> de Microsoft 365 u Outlook con buzón. No use un buzón con "
        "correo de clientes para el primer ensayo.",
        "El equipo de Windows con NestAdmin instalado y una copia del repositorio para generar el "
        "archivo de configuración.",
        "Unos 20 minutos. No hace falta tarjeta de crédito ni suscripción de Azure.",
    ]),

    p("Qué hace NestAdmin con la cuenta", h2),
    p("NestAdmin inicia sesión como <b>aplicación pública de escritorio</b>: código de autorización "
      "con PKCE, sin secreto de cliente. Habla solo con login.microsoftonline.com y "
      "graph.microsoft.com, sin intermediarios. Lee la bandeja de entrada y los adjuntos, crea "
      "borradores de respuesta en el hilo y envía solo lo que usted aprueba o lo que autoriza una "
      "regla activa. Los tokens se guardan en el Administrador de credenciales de Windows "
      f"({c('mail/microsoft-N')}), nunca en archivos ni en registros."),
    table([
        ["Permiso (delegado)", "Para qué lo usa NestAdmin"],
        [c("offline_access"), "Renovar la sesión sin pedirle que vuelva a entrar cada hora."],
        [c("User.Read"), "Leer la dirección del buzón para mostrarla en la revisión (D48)."],
        [c("Mail.ReadWrite"), "Leer la bandeja de entrada y los adjuntos, y crear borradores de respuesta."],
        [c("Mail.Send"), "Enviar un borrador aprobado, una sola vez por aprobación."],
    ], [45, 125]),

    p("2. Registrar la aplicación en Entra ID", h1),
    steps([
        f"Abra {c('https://entra.microsoft.com')} e inicie sesión con la cuenta administradora.",
        "Vaya a <b>Identidad › Aplicaciones › Registros de aplicaciones</b> y pulse "
        "<b>Nuevo registro</b>.",
        "<b>Nombre:</b> NestAdmin. Es lo que verán los usuarios en la pantalla de consentimiento.",
        "<b>Tipos de cuenta compatibles:</b> elija <b>Cuentas de cualquier directorio organizativo "
        "y cuentas personales de Microsoft</b>. NestAdmin usa el punto de acceso "
        f"{c('common')}, que no admite una aplicación de un solo inquilino (error AADSTS50194). "
        "Si solo quiere cuentas de trabajo, elija <b>Cuentas de cualquier directorio "
        "organizativo</b>; las de Outlook.com no podrán entrar.",
        "<b>URI de redirección:</b> en el desplegable elija <b>Cliente público/nativo (móvil y "
        f"escritorio)</b> y escriba exactamente {c('http://localhost')}, sin puerto ni barra final. "
        "Microsoft ignora el puerto con localhost, y NestAdmin usa uno libre distinto cada vez.",
        "Pulse <b>Registrar</b>.",
    ]),
    box("<b>No elija la plataforma «Web».</b> Con ella Microsoft exige un secreto de cliente "
        "(error AADSTS7000218) y NestAdmin, como aplicación de escritorio, no usa ninguno. "
        "Tampoco hace falta crear secretos ni certificados.", WARN, colors.HexColor("#d68910")),

    p("3. Autenticación", h1),
    steps([
        "En la aplicación recién creada, abra <b>Autenticación</b>.",
        f"Compruebe que en <b>Aplicaciones móviles y de escritorio</b> aparece {c('http://localhost')}. "
        "Si no aparece, pulse <b>Agregar una plataforma › Aplicaciones móviles y de escritorio</b> "
        "y añádala como URI personalizada.",
        "En <b>Configuración avanzada</b>, ponga <b>Permitir flujos de clientes públicos</b> en "
        "<b>Sí</b> y pulse <b>Guardar</b>.",
    ]),

    p("4. Permisos de API", h1),
    steps([
        "Abra <b>Permisos de API › Agregar un permiso › Microsoft Graph › Permisos delegados</b>.",
        f"Marque {c('offline_access')}, {c('User.Read')}, {c('Mail.ReadWrite')} y {c('Mail.Send')} "
        f"y pulse <b>Agregar permisos</b>. {c('User.Read')} suele venir ya añadido.",
        "Si su organización no deja que los usuarios consientan por sí mismos, pulse <b>Conceder "
        "consentimiento de administrador para</b> su inquilino. Si no, cada usuario lo aceptará "
        "al conectar su cuenta.",
    ]),
    p("No añada permisos de aplicación (sin usuario) ni otros permisos: NestAdmin no los usa y "
      "pedir de más complica la verificación del editor.", small),

    p("5. Copiar el identificador de la aplicación", h1),
    p("En <b>Información general</b>, copie el <b>Id. de aplicación (cliente)</b>, un GUID de 36 "
      f"caracteres con la forma {c('xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx')}. No es un secreto, pero tampoco debe entrar en el repositorio. No necesita "
      "el Id. de directorio (inquilino)."),

    p("6. Dar el identificador a NestAdmin", h1),
    p("NestAdmin busca el archivo " + c("correo-oauth.json") + " en este orden: la ruta de la "
      "variable " + c("NESTADMIN_MAIL_OAUTH") + ", junto a " + c("nestadmin.exe") + " y en "
      + c("%LOCALAPPDATA%\\NestAdmin") + " (D43). Genérelo <b>fuera del repositorio</b> con el "
      "script incluido. En PowerShell, desde la carpeta del repositorio:"),
    p("powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\correo-oauth.ps1 `<br/>"
      "&nbsp;&nbsp;-MicrosoftClientId 00000000-0000-0000-0000-000000000000 `<br/>"
      "&nbsp;&nbsp;-Salida C:\\NestAdmin-config\\correo-oauth.json", code),
    p("Si también tiene el cliente de Google, añada " + c("-Google C:\\ruta\\client_secret_….json")
      + " en la misma orden y el archivo llevará los dos. El script comprueba que el "
      "identificador es un GUID y se niega a escribir dentro del repositorio."),
    KeepTogether([p("Elija una de estas opciones:", h2), table([
        ["Opción", "Cuándo", "Cómo"],
        ["Copiar a los datos", "Probar en este equipo sin reconstruir",
         "Copie el archivo a " + c("%LOCALAPPDATA%\\NestAdmin\\correo-oauth.json") +
         " y cierre NestAdmin del todo (icono del área de notificación › «Salir de NestAdmin por completo»). Después vuelva a abrirlo."],
        ["Variable de entorno", "Probar con un archivo en otra carpeta",
         "Defina " + c("NESTADMIN_MAIL_OAUTH") + " con la ruta completa y reinicie NestAdmin."],
        ["En el instalador", "Repartir NestAdmin a otros equipos",
         "Construya con " + c("scripts\\build-clean.ps1 -ClienteOAuth C:\\NestAdmin-config\\correo-oauth.json") +
         " (D51). Solo al cerrar una etapa: la construcción es lenta."],
    ], [32, 44, 94])]),

    p("7. Conectar la cuenta en NestAdmin", h1),
    steps([
        "Abra NestAdmin y vaya a <b>Correo</b>. El botón <b>Conectar Microsoft 365 / Outlook</b> "
        "debe estar activo. Si aparece desactivado con el aviso de que el editor no ha configurado "
        "el proveedor, NestAdmin no encuentra el archivo o aún tiene «PONER-AQUI».",
        "Escriba un nombre para la cuenta (opcional, por ejemplo «Facturación») y pulse el botón.",
        "Se abre el navegador del sistema. Inicie sesión con la cuenta de prueba y acepte los "
        "permisos. Tiene 5 minutos.",
        "El navegador muestra «Inicio de sesión completado». Vuelva a NestAdmin: la cuenta aparece "
        "conectada con la dirección del buzón.",
        "Pulse <b>Consultar correo</b>. El aviso indica los mensajes nuevos y los adjuntos "
        "importados como documentos.",
    ]),

    p("8. Recorrido de prueba con la cuenta real (T13)", h1),
    p("Use solo datos inventados. Para las facturas, sirven los archivos sintéticos del repositorio "
      f"({c('crates/extract/tests/fixtures/dos-facturas.jpg')} o un PDF de prueba). Anote el "
      "resultado de cada paso: «correcto» o el código de error que aparezca."),
    table([
        ["N.º", "Qué hacer", "Resultado esperado"],
        ["1", "Desde otra cuenta, envíe al buzón de prueba un correo pidiendo una copia de factura, "
              "con una factura sintética adjunta (PDF o foto).",
         "Tras <b>Consultar correo</b>, el correo está en la cola con cuenta, remitente y fecha. "
         "El adjunto entra como documento con origen «correo», y con dos facturas en la foto se crean dos revisiones."],
        ["2", "Pulse <b>Consultar correo</b> otra vez.", "No se duplica nada (0 nuevos)."],
        ["3", "Revise el borrador propuesto, edítelo si quiere y apruébelo.",
         "El aviso dice que la respuesta se envió desde la cuenta. En Outlook, Elementos enviados tiene una sola respuesta, en el mismo hilo."],
        ["4", "Con otro correo: apruebe la respuesta y corte la red justo después (modo avión o cable). "
              "Vuelva a conectar y pulse <b>Reintentar</b> en la tarea.",
         "La tarea se reconcilia como enviada o pide confirmar. Nunca hay dos envíos del mismo borrador."],
        ["5", "Renombre la cuenta.", "Cambia el nombre; el origen de los correos ya leídos se conserva."],
        ["6", "Desconecte la cuenta.",
         "Desaparece de la lista y del Administrador de credenciales (" + c("mail/microsoft-N") + ")."],
    ], [12, 76, 82]),
    p("No incluya en la evidencia direcciones, asuntos ni contenidos reales: basta con fecha, "
      "versión de NestAdmin, resultado de cada paso y códigos de error.", small),

    p("9. Problemas frecuentes", h1),
    table([
        ["Mensaje o síntoma", "Causa y solución"],
        ["AADSTS50011: la URI de redirección no coincide",
         f"Falta {c('http://localhost')} en «Aplicaciones móviles y de escritorio», o NestAdmin es "
         "una versión anterior a D70, que usa 127.0.0.1. Revise el paso 3 o actualice NestAdmin."],
        ["AADSTS7000218: se requiere client_assertion o client_secret",
         "La URI está en la plataforma «Web» o los flujos de cliente público no están permitidos. Revise los pasos 2 y 3."],
        ["AADSTS50194: la aplicación no es multiinquilino",
         "Se registró para un solo inquilino. En <b>Autenticación › Tipos de cuenta compatibles</b>, cámbielo como indica el paso 2."],
        ["AADSTS700016: no se encontró la aplicación",
         "El identificador de " + c("correo-oauth.json") + " no es el de la aplicación, o la cuenta "
         "es personal y el registro solo admite cuentas de trabajo."],
        ["«Necesita la aprobación del administrador» (AADSTS65001 o 90094)",
         "El inquilino no deja consentir a los usuarios. Un administrador pulsa <b>Conceder consentimiento de administrador</b> (paso 4)."],
        ["El botón de Microsoft está desactivado",
         "NestAdmin no encuentra " + c("correo-oauth.json") + ", o tiene «PONER-AQUI». Revise el paso 6 y cierre NestAdmin del todo antes de abrirlo."],
        ["El navegador no vuelve a NestAdmin o «no se completó a tiempo»",
         "Han pasado los 5 minutos, o un antivirus o cortafuegos bloquea las conexiones a localhost. Permita NestAdmin en red local y vuelva a intentarlo."],
        ["La cuenta pasa a «Sin sesión»",
         "El administrador revocó la sesión, cambió la contraseña o caducó el token de renovación. Pulse <b>Volver a iniciar sesión</b> en la cuenta."],
    ], [58, 112]),

    p("10. Para repartir NestAdmin a otras organizaciones", h1),
    p("Con el registro anterior, las cuentas de otros inquilinos verán «editor no comprobado» en el "
      "consentimiento y algunos administradores bloquearán la aplicación. Para evitarlo, Microsoft "
      "pide la <b>verificación de editor</b>: una cuenta verificada de Microsoft AI Cloud Partner "
      "Program, un dominio de editor verificado y, en <b>Personalización de marca y propiedades</b>, "
      "las URL de inicio, términos y privacidad del sitio " + c("https://nestadmin.humannaia.com") +
      ". El detalle está en " + c("sitio/README.md") + "."),
    Spacer(1, 6),
    box("<b>Resumen.</b> Registro multiinquilino › plataforma «móvil y escritorio» con "
        f"{c('http://localhost')} › flujos públicos «Sí» › cuatro permisos delegados de Graph › "
        f"Id. de aplicación en {c('correo-oauth.json')} › Conectar en NestAdmin › recorrido de prueba."),
]

out = Path(__file__).with_suffix(".pdf")
SimpleDocTemplate(str(out), pagesize=A4, leftMargin=20 * mm, rightMargin=20 * mm,
                  topMargin=18 * mm, bottomMargin=20 * mm, title="Conectar Microsoft 365 a NestAdmin",
                  author="NestAdmin", subject="Guía de configuración de Microsoft Entra ID"
                  ).build(story, onFirstPage=footer, onLaterPages=footer)
print(out)
