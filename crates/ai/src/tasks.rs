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
con punto decimal y dos decimales, sin símbolo de moneda. Fechas en formato AAAA-MM-DD. \
Si el texto contiene varias facturas distintas (por ejemplo, una foto con dos facturas), pon la \
primera en esos campos y cada una de las demás en other_invoices, con los mismos seis campos y las \
mismas reglas; no mezcles datos de una factura con los de otra. Si solo hay una factura, \
other_invoices es una lista vacía.";

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
    let invoice = json!({
        "type": "object",
        "properties": properties,
        "required": INVOICE_FIELDS,
        "additionalProperties": false,
    });
    let mut top = properties.clone();
    top.insert(
        OTHER_INVOICES.into(),
        json!({ "type": "array", "items": invoice }),
    );
    let mut required: Vec<&str> = INVOICE_FIELDS.to_vec();
    required.push(OTHER_INVOICES);
    GenerateRequest {
        request_id: request_id.to_owned(),
        task: Task::Extraction,
        instructions: EXTRACTION_INSTRUCTIONS.to_owned(),
        content: text.to_owned(),
        schema: Some(json!({
            "type": "object",
            "properties": top,
            "required": required,
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

/// Facturas del mismo documento además de la primera (D68), como objetos de salida que
/// [`parse_extraction`] acepta. Sin la clave (un modelo que no la devuelve) o con la lista
/// vacía, ninguna. Un elemento que no es una factura válida hace inválida la respuesta: no se
/// descarta en silencio una factura que el modelo dijo ver.
pub fn parse_other_invoices(output: &Value) -> Result<Vec<Value>, AiError> {
    let list = match output.get(OTHER_INVOICES) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => {
            return Err(AiError::InvalidResponse(format!(
                "{OTHER_INVOICES} no es una lista"
            )))
        }
    };
    list.iter()
        .map(|item| {
            parse_extraction(item)?;
            Ok(item.clone())
        })
        // Un elemento sin ningún dato no es una factura.
        .filter(|item: &Result<Value, AiError>| match item {
            Ok(value) => INVOICE_FIELDS.iter().any(|f| {
                value
                    .get(*f)
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
            }),
            Err(_) => true,
        })
        .collect()
}

/// Clave de la salida de extracción con las demás facturas del documento.
pub const OTHER_INVOICES: &str = "other_invoices";

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
