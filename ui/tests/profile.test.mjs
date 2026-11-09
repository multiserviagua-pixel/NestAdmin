import assert from "node:assert/strict";
import { test } from "node:test";
import {
  PROFILE_MAX_CHARS,
  PROFILE_NOTICE,
  applyPrefill,
  canSave,
  consentLines,
  consentSentence,
  counterText,
  destinationText,
  emptyProfile,
  findingText,
  isDirty,
  profileChars,
  savedNotice,
  statusOf,
  warningTexts,
} from "../src/profile-text.ts";

const profile = (over = {}) => ({ ...emptyProfile(), ...over });

const preview = (over = {}) => ({
  provider: { provider: "ollama_cloud", model: "gemma4:31b" },
  host: "ollama.com",
  external: true,
  has_profile: true,
  block: "Información del negocio (de confianza)…\nHorario: lunes a viernes",
  fp: "0123456789abcdef",
  blocked: null,
  will_send: false,
  local_fallback: true,
  consent: { required: true, state: "missing", provider: "ollama_cloud", model: "gemma4:31b", host: "ollama.com", at: null },
  warnings: [],
  notice: PROFILE_NOTICE,
  ...over,
});

test("el aviso fijo dice lo que manda D163", () => {
  assert.equal(PROFILE_NOTICE, "Escriba solo lo que pueda leer cualquier remitente: la IA puede citarlo en una respuesta.");
});

test("el contador suma los cinco campos por carácter y avisa del tope", () => {
  assert.equal(profileChars(profile({ hours: "áé", policies: "😀x" })), 4);
  assert.equal(counterText(profile({ hours: "x".repeat(1234) })).text, "1.234 de 2.000 caracteres");
  assert.equal(counterText(profile({ hours: "x".repeat(2000) })).over, false);
  assert.equal(counterText(profile({ hours: "x".repeat(1500), policies: "x".repeat(501) })).over, true);
  assert.equal(PROFILE_MAX_CHARS, 2000);
});

test("solo se puede guardar con cambios reales y dentro del tope", () => {
  const saved = profile({ hours: "9 a 14" });
  assert.equal(isDirty(profile({ hours: "  9 a 14  " }), saved), false, "los espacios de los extremos no cuentan");
  assert.equal(canSave(profile({ hours: "9 a 14" }), saved), false);
  assert.equal(canSave(profile({ hours: "9 a 15" }), saved), true);
  assert.equal(canSave(profile({ hours: "x".repeat(2001) }), saved), false);
  assert.equal(canSave(profile(), saved), true, "vaciarlo es un cambio (borra el perfil)");
});

test("la precarga rellena los campos vacíos y nunca pisa lo escrito", () => {
  const draft = profile({ signature: "Mi firma" });
  const prefill = profile({ display_name: "Taller Ficticio SL", signature: "Taller Ficticio SL\nCalle Falsa 1" });
  const out = applyPrefill(draft, prefill);
  assert.equal(out.display_name, "Taller Ficticio SL");
  assert.equal(out.signature, "Mi firma");
  assert.equal(applyPrefill(profile({ hours: "  " }), profile({ hours: "9 a 14" })).hours, "9 a 14", "un campo en blanco cuenta como vacío");
});

test("los bloqueos y avisos se explican con su campo y sin prometer de más", () => {
  assert.match(findingText({ field: "policies", code: "iban" }), /Políticas: hay un IBAN/);
  assert.match(findingText({ field: "signature", code: "card" }), /Firma: hay un número de tarjeta/);
  assert.match(findingText({ field: "hours", code: "secret" }), /clave o un token/);
  assert.match(findingText({ field: "total", code: "too_long" }), /supera los 2\.000 caracteres/);
  assert.match(findingText({ field: "signature", code: "phone" }), /teléfono.*cualquier remitente/);
  assert.match(findingText({ field: "signature", code: "email" }), /correo.*cualquier remitente/);
  assert.equal(warningTexts([{ field: "signature", code: "phone" }, { field: "signature", code: "phone" }]).length, 1);
});

test("el aviso al guardar dice si hay que volver a autorizar", () => {
  const view = (saved) => ({ profile: profile(), saved, updated_at: 1, chars: 0, max_chars: 2000 });
  assert.equal(savedNotice({ view: view(true), warnings: [], consent_withdrawn: false }), "Perfil guardado");
  assert.match(savedNotice({ view: view(true), warnings: [], consent_withdrawn: true }), /vuelva a autorizarlo/);
  assert.equal(savedNotice({ view: view(false), warnings: [], consent_withdrawn: false }), "Perfil borrado");
});

test("la frase del consentimiento nombra proveedor y modelo", () => {
  assert.equal(destinationText(preview()), "Ollama cloud (gemma4:31b)");
  assert.equal(consentSentence(preview()), "Se enviará a Ollama cloud (gemma4:31b) al redactar respuestas");
});

test("el estado ofrece autorizar, retirar o nada según el caso", () => {
  const missing = statusOf(preview());
  assert.deepEqual([missing.canGrant, missing.canWithdraw], [true, false]);
  assert.match(missing.line, /Sin autorización/);
  const granted = statusOf(preview({ consent: { ...preview().consent, state: "granted", at: 5 } }));
  assert.deepEqual([granted.canGrant, granted.canWithdraw], [false, true]);
  assert.match(granted.line, /ollama\.com/);
  const stale = statusOf(preview({ consent: { ...preview().consent, state: "stale", at: 5 } }));
  assert.deepEqual([stale.canGrant, stale.canWithdraw], [true, true]);
  const local = statusOf(preview({ external: false, host: null, consent: { required: false, state: "not_needed", provider: null, model: null, host: null, at: null } }));
  assert.deepEqual([local.canGrant, local.canWithdraw], [false, false]);
  assert.match(local.line, /no sale de este equipo/);
  const none = statusOf(preview({ has_profile: false, block: null, fp: null }));
  assert.deepEqual([none.canGrant, none.canWithdraw], [false, false]);
  const blocked = statusOf(preview({ blocked: "iban", block: null, fp: null }));
  assert.equal(blocked.canGrant, false);
  assert.match(blocked.line, /no puede enviarse/);
});

test("el recuadro de consentimiento repite el destino y lo que no ocurre", () => {
  const c = consentLines(preview());
  assert.equal(c.heading, "Se enviará a Ollama cloud (gemma4:31b) al redactar respuestas");
  const all = c.lines.join(" ");
  assert.match(all, /servidor ollama\.com/);
  assert.match(all, /No sale al clasificar ni al leer facturas/);
  assert.match(all, /IA local/);
  assert.match(all, /cualquiera.*se retira/);
  assert.ok(all.includes(PROFILE_NOTICE));
  assert.match(consentLines(preview({ local_fallback: false })).lines.join(" "), /No hay respaldo/);
});
