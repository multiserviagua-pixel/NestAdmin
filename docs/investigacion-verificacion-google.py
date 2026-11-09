"""Genera docs/investigacion-verificacion-google.pdf: verificación de Google, seguridad y sector.

    python docs/investigacion-verificacion-google.py

Requiere reportlab y Arial (Windows) o DejaVu Sans (Linux). Recoge fuentes públicas consultadas
el 2026-11-09. Si se actualiza la investigación, se cambia aquí y se vuelve a generar el PDF.
No contiene datos de clientes, credenciales ni contenido de cuentas.
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

CANDIDATES = [
    ("C:/Windows/Fonts/arial.ttf", "C:/Windows/Fonts/arialbd.ttf", "C:/Windows/Fonts/ariali.ttf"),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
     "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
     "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf"),
]
REGULAR, BOLD, ITALIC = next(c for c in CANDIDATES if Path(c[0]).exists())
pdfmetrics.registerFont(TTFont("Sans", REGULAR))
pdfmetrics.registerFont(TTFont("Sans-Bold", BOLD))
pdfmetrics.registerFont(TTFont("Sans-Italic", ITALIC))
pdfmetrics.registerFontFamily("Sans", normal="Sans", bold="Sans-Bold", italic="Sans-Italic",
                              boldItalic="Sans-Bold")

INK = colors.HexColor("#1f2933")
ACCENT = colors.HexColor("#0f5fa8")
SOFT = colors.HexColor("#eef4fa")
WARN = colors.HexColor("#fff4e0")
RULE = colors.HexColor("#c9d6e3")
MUTED = colors.HexColor("#52606d")

body = ParagraphStyle("body", fontName="Sans", fontSize=10, leading=14.5, textColor=INK,
                      alignment=TA_LEFT, spaceAfter=6)
title = ParagraphStyle("title", parent=body, fontName="Sans-Bold", fontSize=22, leading=27,
                       textColor=ACCENT, spaceAfter=4)
subtitle = ParagraphStyle("subtitle", parent=body, fontSize=11.5, leading=16,
                          textColor=MUTED, spaceAfter=14)
h1 = ParagraphStyle("h1", parent=body, fontName="Sans-Bold", fontSize=14, leading=18,
                    textColor=ACCENT, spaceBefore=14, spaceAfter=6, keepWithNext=1)
h2 = ParagraphStyle("h2", parent=body, fontName="Sans-Bold", fontSize=10.8, leading=14,
                    spaceBefore=7, spaceAfter=3, keepWithNext=1)
cell = ParagraphStyle("cell", parent=body, fontSize=8.8, leading=11.8, spaceAfter=0)
cell_b = ParagraphStyle("cellb", parent=cell, fontName="Sans-Bold")
src = ParagraphStyle("src", parent=body, fontSize=8.8, leading=12, spaceAfter=2)

WIDTH = A4[0] - 40 * mm


def p(text, style=body):
    return Paragraph(text, style)


def a(url, text):
    return f'<a href="{url}" color="#0f5fa8"><u>{text}</u></a>'


def bullets(items):
    return ListFlowable([ListItem(p(t), leftIndent=12) for t in items],
                        bulletType="bullet", start="•", leftIndent=12, bulletFontName="Sans")


def steps(items):
    return ListFlowable([ListItem(p(t), leftIndent=16) for t in items],
                        bulletType="1", leftIndent=16, bulletFontName="Sans-Bold",
                        bulletFontSize=10)


def box(text, background=SOFT, edge=ACCENT):
    table = Table([[p(text, ParagraphStyle("boxed", parent=body, spaceAfter=0))]],
                  colWidths=[WIDTH])
    table.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), background),
        ("LINEBEFORE", (0, 0), (0, -1), 3, edge),
        ("LEFTPADDING", (0, 0), (-1, -1), 9), ("RIGHTPADDING", (0, 0), (-1, -1), 9),
        ("TOPPADDING", (0, 0), (-1, -1), 7), ("BOTTOMPADDING", (0, 0), (-1, -1), 7),
    ]))
    return KeepTogether([table, Spacer(1, 7)])


def table(rows, widths):
    total = sum(widths)
    data = [[p(text, cell_b) for text in rows[0]]]
    data += [[p(text, cell) for text in row] for row in rows[1:]]
    result = Table(data, colWidths=[WIDTH * w / total for w in widths], repeatRows=1)
    result.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, 0), SOFT),
        ("LINEBELOW", (0, 0), (-1, -1), 0.5, RULE),
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("LEFTPADDING", (0, 0), (-1, -1), 5), ("RIGHTPADDING", (0, 0), (-1, -1), 5),
        ("TOPPADDING", (0, 0), (-1, -1), 4), ("BOTTOMPADDING", (0, 0), (-1, -1), 5),
    ]))
    return [result, Spacer(1, 8)]


# Fuentes que se citan en el texto.
RSV = "https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification"
CASA_HELP = "https://support.google.com/cloud/answer/13465431"
GMAIL_SCOPES = "https://developers.google.com/gmail/api/auth/scopes"
AUDIENCE = "https://support.google.com/cloud/answer/15549945"
FAQ = "https://support.google.com/cloud/answer/13463817"
CHANGES = "https://support.google.com/cloud/answer/13464018"
EXEMPT = "https://support.google.com/cloud/answer/13464323"
SUBMIT = "https://support.google.com/cloud/answer/13461325"
TERMS = "https://developers.google.com/terms"
LIMITED = "https://developers.google.com/terms/api-services-user-data-policy"
WORKSPACE = "https://developers.google.com/workspace/workspace-api-user-data-developer-policy"
ADMIN = "https://knowledge.workspace.google.com/admin/apps/control-which-apps-access-google-workspace-data"
MIME = "https://mimestream.com/blog/casa-verified"
MAILMATE = "https://lists.freron.com/mailmate/2024-June/017271.html"
THUNDER = "https://github.com/thunderbird/thunderbird-android/security/policy"
TIERING = "https://appdefensealliance.dev/casa/casa-tiering"
LABS = "https://www.appdefensealliance.org/certification/authorized-labs"
DEEP = "https://deepstrike.io/blog/google-casa-security-assessment-2025"
GROQ = "https://console.groq.com/docs/your-data"
GROQ_TERMS = "https://console.groq.com/docs/legal/services-agreement"
GROQ_DPA = "https://console.groq.com/docs/legal/customer-data-processing-addendum"
OPENAI = "https://developers.openai.com/api/docs/guides/your-data"
ANTHROPIC = "https://privacy.claude.com/en/articles/7996866-how-long-do-you-store-my-organization-s-data"
MISTRAL = "https://docs.mistral.ai/admin/monitor-comply/zero-data-retention"
OLLAMA = "https://ollama.com/privacy"
OLLAMA_CLOUD = "https://docs.ollama.com/cloud"
APPLE = "https://security.apple.com/blog/private-cloud-compute/"
EDPB = "https://www.edpb.europa.eu/system/files/documents/2023-10/EDPB_guidelines_202007_controllerprocessor_final_es.pdf"
AEPD_EIPD = "https://www.aepd.es/preguntas-frecuentes/2-tus-obligaciones-como-responsable-del-tratamiento/10-evaluacion-de-impacto/FAQ-0226-en-que-supuestos-es-necesario-realizar-una-evaluacion-de-impacto"
AEPD_AGENT = "https://www.aepd.es/guias/orientaciones-ia-agentica.pdf"
SCC = "https://commission.europa.eu/law/law-topic/data-protection/international-dimension-data-protection/standard-contractual-clauses-scc_en"
DRIVE_OCR = "https://support.google.com/drive/answer/176692"
UPLOADS = "https://developers.google.com/workspace/drive/api/guides/manage-uploads"
LIMITS = "https://developers.google.com/workspace/drive/api/guides/limits"
VISION = "https://cloud.google.com/vision/pricing"
DOCAI = "https://cloud.google.com/document-ai/pricing"
GEMINI = "https://ai.google.dev/gemini-api/terms"
WIN_OCR = "https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr.ocrengine"
TESS = "https://tesseract-ocr.github.io/tessdoc/ImproveQuality.html"
PADDLE = "https://github.com/PaddlePaddle/PaddleOCR"

S = []

S += [p("Verificación de Google, seguridad y prácticas del sector", title),
      p("Investigación para NestAdmin · Fuentes consultadas el 9 de noviembre de 2026", subtitle)]

S.append(box(
    "<b>Cómo leer este documento.</b> No es asesoramiento legal, y no acredita que la verificación "
    "se haya solicitado ni superado. Las páginas de Google son oficiales, pero se contradicen en "
    "algunos puntos; cuando lo hacen, se indica. Cada afirmación lleva su fuente. Lo marcado "
    "<b>[inferencia]</b> es lectura propia, y lo marcado <b>[no oficial]</b> viene de proveedores o "
    "foros. Google puede cambiar sus requisitos: conviene releer las fuentes antes de actuar.",
    WARN, colors.HexColor("#c77700")))

# 1
S.append(p("1. Resumen y recomendaciones", h1))
S.append(bullets([
    "<b>Planifica que Google pedirá la evaluación CASA.</b> No hay exención escrita para "
    "aplicaciones de escritorio, y otros clientes de correo sin servidor del editor la han pasado.",
    "<b>Se puede operar mientras tanto, con límites.</b> Modo de prueba para pilotos y proyecto "
    "Internal de cada pyme con Workspace. <b>No hay vía válida para vender a muchas cuentas @gmail "
    "sin completar la verificación.</b>",
    "<b>Cifrar más no resuelve la salida de datos a la IA.</b> TLS ya protege el trayecto, y el "
    "proveedor tiene que leer el texto. Lo que protege es enviar menos y elegir bien el destino.",
    "<b>La documentación pública debe cambiar antes de solicitar nada.</b> Las frases «All "
    "processing happens locally» y «no se descargan adjuntos» ya no son ciertas, y Google evalúa "
    "esos textos.",
    "<b>El orden propuesto:</b> política y controles de datos; solicitud de verificación de Gmail; "
    "presupuesto de CASA; y añadir Drive después, en el peor caso en la renovación anual.",
    "<b>El OCR en la nube es posible sin coste con Drive, pero su velocidad está sin demostrar.</b> "
    "Hay que medirlo frente a mejoras locales antes de adoptarlo.",
    "<b>Certificaciones ISO 27001 o SOC 2: hoy no compensan.</b> Tienen sentido cuando un contrato "
    "concreto las exija.",
]))
S.append(Spacer(1, 4))
S.append(p("Decisiones y gastos que requieren tu autorización", h2))
S.append(bullets([
    "Contratar la revisión de un abogado (roles RGPD, política, términos).",
    "Pedir presupuesto a un laboratorio de CASA y aceptar su coste anual.",
    "Solicitar la verificación a Google, que exige publicar la app en producción.",
]))

# 2
S.append(p("2. Evaluación CASA", h1))
S.append(p("Qué dice Google, y dónde no coincide", h2))
S += table([
    ["Fuente", "Lo que dice"],
    [a(RSV, "Verificación de ámbitos restringidos"),
     "Vincula la evaluación al acceso a datos restringidos desde o a través de un servidor de "
     "terceros, o a guardarlos o transmitirlos en servidores."],
    [a(CASA_HELP, "Ayuda: evaluación de seguridad"),
     "La exige cada año a las apps con ámbitos restringidos, sin mencionar servidores. La describe "
     "como el paso final de la revisión."],
    [a(GMAIL_SCOPES, "Ámbitos de Gmail"),
     "<font name='Sans-Bold'>Restringidos:</font> gmail.readonly, gmail.compose, gmail.modify. "
     "<font name='Sans-Bold'>Sensible:</font> gmail.send. "
     "<font name='Sans-Bold'>No sensible:</font> gmail.labels."],
    [a(EXEMPT, "Exenciones de verificación"),
     "Uso personal, desarrollo y pruebas, datos solo de cuenta de servicio, uso interno de la "
     "organización, y apps confiadas por el administrador o instaladas desde Marketplace. "
     "<b>No cita el proceso solo en el dispositivo.</b>"],
], [28, 72])
S.append(p("<b>Conclusión [inferencia].</b> Aun con la lectura favorable, NestAdmin envía texto a un "
           "proveedor de IA elegido por el usuario. Ese proveedor es un servidor de terceros que "
           "recibe datos procedentes de ámbitos restringidos, así que el argumento de «sin "
           "servidores» es débil. Google no define este caso."))
S.append(p("Casos reales", h2))
S.append(bullets([
    f"{a(MIME, 'Mimestream')} (junio de 2024), cliente tradicional de escritorio sin nube: "
    "cumplió los requisitos de Tier 2.",
    f"{a(MAILMATE, 'MailMate')} (junio de 2024): pasó Tier 2. El dato es antiguo: su propio mensaje "
    "indica una caducidad en mayo de 2025.",
    f"{a(THUNDER, 'Thunderbird para Android y K-9 Mail')}: han completado la evaluación en Tier 2.",
    "No encontré información pública sobre Thunderbird de escritorio, Mailspring, eM Client ni Canary.",
]))
S.append(Spacer(1, 4))
S.append(p("Niveles, coste y plazos", h2))
S.append(bullets([
    f"<b>Nivel:</b> lo calcula Google, no el desarrollador ({a(TIERING, 'App Defense Alliance')}). "
    f"Hay {a(LABS, 'laboratorios autorizados')}.",
    f"<b>Tasas:</b> {a(FAQ, 'Google dice')} que no cobra al desarrollador. El coste es el del laboratorio.",
    f"<b>Coste [no oficial]:</b> {a(DEEP, 'DeepStrike')}, un proveedor, cita listas de un laboratorio: "
    "unos 675 a 855 USD en AL1 y 5.400 USD en AL2. Son orientativos: hay que pedir presupuesto y "
    "sumar correcciones y revalidaciones.",
    f"<b>Plazo:</b> {a(FAQ, 'Google estima')} 6 semanas para ámbitos restringidos, sin garantía. "
    "Más la evaluación: de 1 a 4 semanas según el nivel [no oficial].",
    f"<b>Renovación:</b> al menos cada 12 meses ({a(RSV, 'Google')}).",
    f"<b>Requisitos para solicitar:</b> publicar en producción (el modo de prueba no vale) y "
    f"aportar vídeo de demostración ({a(SUBMIT, 'Google')}).",
]))
S.append(Spacer(1, 4))
S.append(p("Añadir Drive después", h2))
S.append(p(f"{a(CHANGES, 'Una página')} dice que añadir ámbitos no obliga a rehacer una evaluación ya "
           f"completada. {a(RSV, 'Otra')} dice que la app podría tener que reevaluarse si el ámbito no "
           "estaba cubierto. Ambas coinciden en que hace falta aprobación antes de usarlo. "
           "<b>Recomendación:</b> asumir la versión estricta y preguntarlo. En el peor caso, Drive "
           "entra en la renovación anual."))

# 3
S.append(p("3. Cómo operar mientras tanto", h1))
S += table([
    ["Vía", "Límite", "Riesgo", "¿Usar?"],
    [f"{a(AUDIENCE, 'Modo de prueba')}", "100 usuarios de prueba. La autorización caduca a los 7 días.",
     "Reautorizar cada semana. No es producción.", "<b>Sí</b>, para pilotos y desarrollo."],
    [f"Proyecto Internal de la pyme con Workspace, con su client_id ({a(RSV, 'exención interna')})",
     "Solo miembros de esa organización.",
     "Configuración y soporte. Google no trata de forma expresa que el cliente traiga su proyecto "
     "[inferencia].", "<b>Sí</b>, como puente para Workspace."],
    [f"{a(FAQ, 'Publicada sin verificar')}",
     "100 usuarios nuevos en toda la vida del proyecto, sin poder reiniciarlo. Pantalla de aviso.",
     "Agota el tope y da mala imagen.", "Lo mínimo, durante el trámite."],
    [f"{a(ADMIN, 'App confiada por el administrador')}",
     "Solo Workspace.", "No consta que quite la pantalla ni el tope.", "No como solución."],
    ["Uso personal", "Tú o pocos usuarios que conozcas.", "No encaja con vender.", "No."],
    ["Crear proyectos en serie para clientes", "—",
     f"Eludir límites contradice los {a(TERMS, 'términos')} (2d). Riesgo de suspensión sin aviso (3a) "
     "[inferencia].", "<b>No.</b>"],
], [28, 28, 28, 16])

# 4
S.append(p("4. Alternativas a los ámbitos restringidos", h1))
S += table([
    ["Opción", "Valoración"],
    ["Solo gmail.send", "Sensible, sin CASA. Solo envía: sirve como modo reducido, no sustituye la lectura."],
    ["gmail.readonly en vez de modify", "También restringido. No evita la evaluación."],
    ["IMAP con contraseña de aplicación",
     "No encontré prohibición, pero Google no la recomienda, exige verificación en dos pasos y puede no "
     "estar disponible en cuentas de trabajo. Solo como opción avanzada que active el usuario."],
    ["Reenvío a un buzón del cliente",
     "No usa ámbitos, pero se pierde el control sobre Gmail. Si el buzón fuese del editor, crearía el "
     "servidor que se quiere evitar."],
    ["Complemento de Gmail", "Trabaja con el mensaje abierto, sin proceso en segundo plano. No sustituye a la cola."],
], [30, 70])

# 5
S.append(p("5. Cómo lo manejan las empresas del sector", h1))
S.append(p("Resumen de políticas y páginas de seguridad públicas. Lo extraje de lecturas automáticas: "
           "antes de copiar nada o citarlas, hay que leer el original."))
S += table([
    ["Empresa", "Lo que consta"],
    ["Canary (la más parecida)", "Correo en el dispositivo. La IA generativa va a proveedores nombrados. "
     "Declara uso limitado y que no entrena modelos generalizados."],
    ["Fyxer", "SOC 2 Tipo II, ISO 27001 y CASA 2. DPA y retención cero con su proveedor de IA."],
    ["Shortwave", "CASA 2, AES-256 y TLS 1.2+. Nombra a sus proveedores de IA."],
    ["Superhuman", "Lista pública de subencargados con proveedores de IA, y DPA. Su política contempla "
     "entrenar con contenido del usuario, con control de exclusión."],
    ["Parseur", "Servidores en la UE, DPA y retención configurable por buzón."],
    ["Docparser", "Retención de 0 a 180 días y DPA a petición. Se declara responsable de la cuenta y "
     "encargado de los documentos."],
    ["Nanonets", "SOC 2 bajo NDA y lista de subencargados."],
    ["Dext, Hubdoc", "Dext: ISO 27001 y AES-256. Hubdoc: dentro del SOC 2 de Xero."],
], [24, 76])
S.append(p("<b>Lo habitual:</b> decir que no se entrena con los datos, TLS y AES-256, DPA disponible y "
           "nombrar a los proveedores de IA. <b>Lo que diferencia:</b> proceso en el dispositivo, "
           "retención cero contractual, plazos concretos y certificaciones."))
S.append(p("<b>Nadie promete «no descargamos adjuntos».</b> Explican qué procesan, para qué y a quién va. "
           "Es el modelo que debe seguir NestAdmin."))
S.append(p("Contenido de la política de NestAdmin", h2))
S.append(steps([
    "Identidad del editor y qué datos <b>no</b> recibe.",
    "Por función: qué sale del equipo, a quién y cuándo (incluidos correo y adjuntos).",
    "Permisos de Gmail, Microsoft y Drive, y para qué sirve cada uno.",
    "Proveedor de IA elegido por el usuario, sus condiciones y reparto de roles.",
    f"Declaración de uso limitado, con aviso dentro de la app antes del consentimiento ({a(WORKSPACE, 'política de Workspace')}).",
    "Retención local, borrado y desconexión de cuentas.",
    "Dónde se guardan los tokens y cómo se protegen los datos locales.",
    "Transferencias internacionales.",
    "Telemetría, actualizaciones y soporte, aunque sean «ninguno».",
    f"Derechos, contacto y aviso de cambios. Un cambio de uso exige consentimiento antes ({a(LIMITED, 'uso limitado')}).",
]))

# 6
S.append(p("6. Seguridad de la transmisión a la IA", h1))
S.append(p(f"El cifrado de extremo a extremo impide al servicio computar sobre los datos ({a(APPLE, 'Apple')} "
           "lo explica). La excepción es el cómputo confidencial con atestación, que aún no es lo "
           "habitual. Por tanto, <b>cifrar más no oculta el texto al proveedor</b>."))
S.append(p("Controles que sí protegen, por eficacia [criterio propio]", h2))
S.append(steps([
    "<b>Minimizar:</b> solo el fragmento necesario, nunca el hilo o el adjunto entero.",
    "<b>Seudonimizar:</b> detectar NIF e IBAN en local y enviar marcadores como [NIF_1], con el mapa solo en el "
    "equipo. Sigue siendo dato personal: no debe anunciarse como anonimización.",
    "<b>Ficha por proveedor:</b> retención, entrenamiento, región y DPA, con lista de proveedores permitidos.",
    "<b>Aviso de retención cero,</b> y cómo activarla si la clave es del usuario.",
    "<b>Vista previa y registro local</b> de lo enviado.",
    "<b>Cifrado en reposo:</b> SQLCipher protege también las copias de seguridad; BitLocker protege frente al robo del equipo.",
    f"<b>Defensa contra la inyección de instrucciones,</b> que Google pide en apps de IA ({a(WORKSPACE, 'política de Workspace')}).",
]))
S.append(Spacer(1, 4))
S.append(p("Proveedores de IA", h2))
S += table([
    ["Proveedor", "Retención", "Entrenamiento", "Región"],
    [a(OLLAMA, "Ollama") + " (destino por defecto)",
     "Procesa de forma transitoria; no guarda más de lo necesario para responder.",
     "Dice que no entrena con entradas ni salidas.",
     "Pueden tratarse en EE. UU.; sin región concreta."],
    [a(GROQ, "Groq"), "Sin retención por defecto, salvo registros de hasta 30 días. Retención cero activable.",
     f"Según su {a(GROQ_TERMS, 'contrato')}, prohibido sin permiso expreso (lectura automática).",
     f"Almacena en EE. UU. {a(GROQ_DPA, 'SCC en el DPA')}."],
    [a(OPENAI, "OpenAI"), "Hasta 30 días. Retención cero con aprobación.", "No, salvo que se opte por ello.",
     "Residencia europea con controles aprobados."],
    [a(ANTHROPIC, "Anthropic"), "30 días. Retención cero por acuerdo.", "Sin verificar en el contrato.", "Sin verificar."],
    [a(MISTRAL, "Mistral"), "Retención cero con aprobación, solo sin estado.",
     "Mensajes poco coherentes: comprobar el ajuste de exclusión.", "UE por defecto según terceros; sin fuente primaria."],
], [20, 32, 26, 22])
S.append(p("Las condiciones cambian con frecuencia. Una ficha debe llevar fecha y fuente, y revisarse."))

# 7
S.append(p("7. RGPD: lo que debe revisar un abogado", h1))
S.append(box("<b>No es asesoramiento legal.</b> Lo siguiente orienta la conversación con el abogado, y "
             "buena parte es inferencia.", WARN, colors.HexColor("#c77700")))
S.append(bullets([
    f"<b>Cliente:</b> normalmente es el responsable del tratamiento. {a(GROQ_DPA, 'Groq')} lo plantea así y se "
    "declara encargado, de modo que el contrato del art. 28 sería cliente-proveedor de IA.",
    f"<b>Editor con clave del usuario y sin acceso a datos:</b> no sería encargado [inferencia]. El "
    f"{a(EDPB, 'CEPD')} decide caso a caso y no hallé un ejemplo exacto.",
    "<b>Lo que cambiaría ese rol:</b> una clave del editor, un proxy, soporte con acceso a datos o telemetría "
    "con contenido. Entonces harían falta DPA y lista de subencargados.",
    "<b>Aunque no sea encargado,</b> el editor sigue siendo responsable de web, licencias y soporte.",
    f"<b>EIPD:</b> corresponde al responsable ({a(AEPD_EIPD, 'AEPD')}). Entregar una plantilla ayuda a vender.",
    f"<b>Transferencias:</b> {a(SCC, 'cláusulas contractuales tipo')} o Data Privacy Framework, según el proveedor.",
    f"<b>AEPD:</b> publicó {a(AEPD_AGENT, 'orientaciones sobre IA agéntica')}, que no pude leer directamente.",
]))
S.append(Spacer(1, 4))
S.append(p("<b>Preguntas para el abogado:</b> confirmar el rol del editor; política y reparto de "
           "responsabilidades en los términos; base jurídica para correos de terceros; DPO o representante; "
           "transferencias; y conservación fiscal frente a borrado, incluido Verifactu."))

# 8
S.append(p("8. Producto estandarizado, por prioridad", h1))
S.append(p("Ahora, con poco coste", h2))
S.append(bullets([
    "Decidir la arquitectura de roles: mantener la clave del usuario, o una clave del editor.",
    "Política nueva, revisada por abogado, y declaración de uso limitado.",
    "Verificación de Google y CASA.",
    "Firma de código del instalador.",
    "Página de confianza pública: flujo de datos, proveedores, cifrado y contacto.",
    "Política de retención y borrado.",
    "Plan de respuesta a incidentes y canal de avisos de seguridad.",
]))
S.append(Spacer(1, 3))
S.append(p("Antes de vender a empresas", h2))
S.append(bullets([
    "Ficha de proveedores compatibles (o de subencargados, si cambian los roles).",
    "Plantilla de EIPD y cuestionario de seguridad ya respondido.",
    "Prueba de penetración externa, SBOM y actualizaciones firmadas.",
    "DPA plantilla, solo si se llegan a tratar datos de clientes.",
]))
S.append(Spacer(1, 3))
S.append(p("Más adelante", h2))
S.append(p("ISO 27001 cuando un contrato lo exija, porque se valora más en Europa; SOC 2 solo con clientes de "
           "EE. UU. Las estimaciones son poco fiables (decenas de miles de dólares el primer año, según blogs de "
           "proveedores). Sin servidores propios hay poco que auditar, y una buena documentación pesa más "
           "[opinión]."))

# 9
S.append(p("9. OCR en la nube", h1))
S.append(p("Tu condición: sin cargos, adicional y más rápido.", h2))
S += table([
    ["Opción", "¿Sin cargo por usuario?", "Observaciones"],
    [f"Conversión de Drive ({a(UPLOADS, 'subida')})", "<b>Sí.</b> Es la única sin facturación de Cloud.",
     f"{a(DRIVE_OCR, 'La ayuda')} pide 2 MB o menos y avisa de que tablas y columnas probablemente no se "
     "detecten. Exportar tiene un tope de 10 MB. Sube el original a la cuenta del usuario."],
    [a(VISION, "Cloud Vision"), "No: pide facturación activada.", "Nivel gratuito de 1.000 unidades al mes. Precios no leídos enteros."],
    [a(DOCAI, "Document AI"), "No confirmado; se espera que pida facturación.", "OCR de pago por páginas; también analizador de facturas."],
    [a(GEMINI, "Gemini"), "<b>No válido aquí.</b>",
     "Para usuarios del EEE solo servicios de pago, y no se envían datos personales a los gratuitos."],
    [a(WIN_OCR, "Windows.Media.Ocr"), "Sí, y es local.", "Depende de paquetes de idioma instalados (ya valorado en D10). Sin pruebas independientes con facturas."],
    [a(TESS, "Tesseract con preprocesado"), "Sí, y es local.", "300 ppp, enderezar, binarizar y elegir modo de página. Mejora barata."],
    [a(PADDLE, "PaddleOCR"), "Sí, y es local (Apache 2.0).", "Una prueba no oficial apunta a más precisión y más lentitud; no generalizable."],
], [26, 26, 48])
S.append(bullets([
    "<b>Velocidad:</b> no encontré datos publicados. Drive suma subida, conversión, exportación y borrado. "
    "Podría ganar en PC lentos con buena conexión, pero hay que medirlo.",
    f"<b>Términos:</b> ningún texto consultado prohíbe este uso, ni lo autoriza a escala. Las cuotas se comparten "
    f"por proyecto, y Google prevé cobrar al proyecto el exceso más adelante en 2026, con al menos 90 días de aviso "
    f"({a(LIMITS, 'límites')}).",
    "<b>RGPD:</b> las facturas contienen datos de terceros. Subirlas requiere revisión.",
    "<b>Regla del proyecto:</b> debe ser una opción expresa, nunca un respaldo silencioso.",
]))
S.append(Spacer(1, 4))
S.append(p("<b>Cómo decidir:</b> medir con facturas sintéticas la latencia (p50 y p95 por página), la precisión por "
           "campo (NIF, fecha, base, IVA, total) y los fallos, con conexión lenta y rápida. Adoptar Drive solo si "
           "mejora con claridad el p95 en un PC corriente sin empeorar los campos."))

# 10
S.append(p("10. Preguntas para Google", h1))
S.append(p(f"No encontré un canal de consulta previa. El contacto oficial es por correo con propietarios y editores "
           f"del proyecto tras solicitar la verificación ({a(SUBMIT, 'Google')}). Hay que pedir la respuesta "
           "asociada al ID del proyecto y conservarla."))
S.append(steps([
    "Con tokens y datos solo en el equipo del usuario, ¿se exige CASA?",
    "¿Cambia si el usuario envía contenido directamente a un proveedor de IA elegido por él, con consentimiento? "
    "¿Y si esa función se desactiva para los datos de Gmail?",
    "¿Qué nivel de evaluación se asignaría, según los usuarios previstos?",
    "¿Se levanta el tope de usuarios con la verificación antes de la carta de evaluación? ¿Hay plazo?",
    "¿Se pueden declarar ya los ámbitos de Drive? ¿Añadirlos después obliga a reevaluar?",
    "¿Es aceptable que la pyme use su propio proyecto Internal? ¿Y con cuentas externas?",
    "¿Encaja NestAdmin en un tipo de aplicación permitido para Gmail?",
]))

# 11
S.append(p("11. Fuentes y lo que quedó sin confirmar", h1))
S.append(p("Fuentes oficiales", h2))
for url, name in [
    (RSV, "Google: verificación de ámbitos restringidos"), (CASA_HELP, "Google: evaluación de seguridad"),
    (GMAIL_SCOPES, "Google: ámbitos de Gmail"), (AUDIENCE, "Google: audiencia y modo de prueba"),
    (FAQ, "Google: preguntas frecuentes de verificación"), (CHANGES, "Google: cambios en una app aprobada"),
    (EXEMPT, "Google: exenciones"), (SUBMIT, "Google: solicitar la verificación"),
    (TERMS, "Google: términos de las API"), (LIMITED, "Google: política de datos y uso limitado"),
    (WORKSPACE, "Google: política de datos de Workspace"), (ADMIN, "Google: control de apps en Workspace"),
    (LIMITS, "Google: límites de Drive"), (DRIVE_OCR, "Google: OCR en Drive"), (GEMINI, "Google: términos de Gemini"),
    (EDPB, "CEPD: responsable y encargado"), (AEPD_EIPD, "AEPD: evaluación de impacto"),
    (SCC, "Comisión Europea: cláusulas contractuales tipo"),
]:
    S.append(p(a(url, name), src))
S.append(p("Proveedores y terceros [no oficial respecto a Google]", h2))
for url, name in [
    (TIERING, "App Defense Alliance: niveles CASA"), (LABS, "App Defense Alliance: laboratorios"),
    (MIME, "Mimestream"), (MAILMATE, "MailMate"), (THUNDER, "Thunderbird para Android"), (DEEP, "DeepStrike"),
    (OLLAMA, "Ollama: privacidad"), (OLLAMA_CLOUD, "Ollama: nube"), (GROQ, "Groq: datos"), (OPENAI, "OpenAI: datos"),
    (ANTHROPIC, "Anthropic: retención"), (MISTRAL, "Mistral: retención cero"),
    (APPLE, "Apple: Private Cloud Compute"),
]:
    S.append(p(a(url, name), src))
S.append(p("Sin confirmar", h2))
S.append(bullets([
    "Si enviar a un proveedor de IA elegido por el usuario cuenta como servidor de terceros para Google.",
    "Si una app de escritorio queda exenta en algún caso, y si la exención interna cubre un proyecto de cada cliente.",
    "Precios actuales de laboratorios, Vision y Document AI, y si la verificación levanta el tope antes de CASA.",
    "Tipos de aplicación permitidos para Gmail: la página de verificación remite a la ficha de producto.",
    "El rol RGPD del editor, los datos de empresas de la sección 5 y las orientaciones de la AEPD.",
]))


def footer(canvas, doc):
    canvas.saveState()
    canvas.setFont("Sans", 8)
    canvas.setFillColor(MUTED)
    canvas.drawString(20 * mm, 11 * mm, "NestAdmin · Investigación · No es asesoramiento legal")
    canvas.drawRightString(A4[0] - 20 * mm, 11 * mm, f"Página {doc.page}")
    canvas.restoreState()


def main():
    target = Path(__file__).with_suffix(".pdf")
    SimpleDocTemplate(
        str(target), pagesize=A4, leftMargin=20 * mm, rightMargin=20 * mm, topMargin=18 * mm,
        bottomMargin=18 * mm, title="Verificación de Google, seguridad y prácticas del sector",
        author="NestAdmin", subject="Investigación para NestAdmin",
    ).build(S, onFirstPage=footer, onLaterPages=footer)
    print(target)


if __name__ == "__main__":
    main()
