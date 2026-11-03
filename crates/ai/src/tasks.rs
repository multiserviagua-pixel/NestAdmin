//! Solicitudes de extracción, clasificación y borrador, y lectura de sus salidas. Las
//! instrucciones expresan reglas del producto; nunca contienen respuestas esperadas.

use nestadmin_documents::{Evidenced, RawInvoice};
use serde_json::{json, Map, Value};

use crate::provider::{AiError, GenerateRequest, Task};

pub const INVOICE_FIELDS: [&str; 6] = [
    "supplier",
    "invoice_number",
    "date",
    "subtotal",
    "tax",
    "total",
];

pub const CATEGORIES: [&str; 5] = [
    "solicitud_factura",
    "consulta_pago",
    "incidencia_factura",
    "documentacion",
    "otros",
];

const EXTRACTION_INSTRUCTIONS: &str = "Extrae los datos de la factura del texto del usuario. \
Devuelve exactamente los campos supplier (proveedor), invoice_number (número de factura), date, \
subtotal (base imponible), tax (IVA) y total. Copia los valores que aparecen en el texto; si un \
dato no aparece, usa null. No calcules ni deduzcas valores ausentes a partir de otros. Importes \
con punto decimal y dos decimales, sin símbolo de moneda. Fechas en formato AAAA-MM-DD.";

const EMAIL_INSTRUCTIONS: &str = "Clasifica el correo del usuario y propone un borrador de \
respuesta en español. Categorías: solicitud_factura (piden una factura o su copia), consulta_pago \
(preguntan por el estado de un pago), incidencia_factura (señalan un error en una factura), \
documentacion (envían o piden documentos o certificados distintos de una factura) y otros. \
El borrador queda pendiente de revisión humana: no afirmes que se ha enviado una factura, \
confirmado un pago ni completado un registro, y no inventes importes, fechas ni números.";

pub fn extraction_request(request_id: &str, text: &str) -> GenerateRequest {
    let nullable = json!({ "type": ["string", "null"] });
    let properties: Map<String, Value> = INVOICE_FIELDS
        .iter()
        .map(|f| ((*f).to_owned(), nullable.clone()))
        .collect();
    GenerateRequest {
        request_id: request_id.to_owned(),
        task: Task::Extraction,
        instructions: EXTRACTION_INSTRUCTIONS.to_owned(),
        content: text.to_owned(),
        schema: Some(json!({
            "type": "object",
            "properties": properties,
            "required": INVOICE_FIELDS,
            "additionalProperties": false,
        })),
    }
}

pub fn email_request(request_id: &str, text: &str) -> GenerateRequest {
    GenerateRequest {
        request_id: request_id.to_owned(),
        task: Task::Classification,
        instructions: EMAIL_INSTRUCTIONS.to_owned(),
        content: text.to_owned(),
        schema: Some(json!({
            "type": "object",
            "properties": {
                "category": { "type": "string", "enum": CATEGORIES },
                "draft": { "type": "string" },
            },
            "required": ["category", "draft"],
            "additionalProperties": false,
        })),
    }
}

/// Convierte la salida de extracción en campos sin interpretar. Un campo ausente en la
/// salida, `null` o vacío queda como ausente; un tipo distinto de cadena es respuesta inválida.
pub fn parse_extraction(output: &Value) -> Result<RawInvoice, AiError> {
    let object = output
        .as_object()
        .ok_or_else(|| AiError::InvalidResponse("la extracción no es un objeto".into()))?;
    let field = |name: &str| -> Result<Option<Evidenced<String>>, AiError> {
        match object.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            // La evidencia literal es lo que devolvió el modelo; el texto original se conserva aparte.
            Some(Value::String(s)) => Ok(Some(Evidenced::with_evidence(
                s.trim().to_owned(),
                s.clone(),
            ))),
            Some(_) => Err(AiError::InvalidResponse(format!(
                "{name} no es texto ni null"
            ))),
        }
    };
    Ok(RawInvoice {
        supplier: field("supplier")?,
        invoice_number: field("invoice_number")?,
        date: field("date")?,
        currency: None,
        subtotal: field("subtotal")?,
        tax: field("tax")?,
        total: field("total")?,
    })
}

/// Categoría y borrador. Una categoría fuera de la lista es respuesta inválida.
pub fn parse_email(output: &Value) -> Result<(String, String), AiError> {
    let category = output
        .get("category")
        .and_then(Value::as_str)
        .ok_or_else(|| AiError::InvalidResponse("falta category".into()))?;
    if !CATEGORIES.contains(&category) {
        return Err(AiError::InvalidResponse(format!(
            "categoría desconocida: {category}"
        )));
    }
    let draft = output
        .get("draft")
        .and_then(Value::as_str)
        .ok_or_else(|| AiError::InvalidResponse("falta draft".into()))?;
    Ok((category.to_owned(), draft.to_owned()))
}
