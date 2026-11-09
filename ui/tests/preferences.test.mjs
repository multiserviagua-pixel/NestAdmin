import assert from "node:assert/strict";
import { test } from "node:test";
import {
  canOfferPreference,
  isPublicDomain,
  preferenceAuditText,
  preferenceChoices,
  preferenceConfirmText,
  preferenceFor,
  preferenceLine,
  preferenceSavedText,
  preferenceWarningText,
  senderParts,
} from "../src/preferences-text.ts";
import { informativeReason } from "../src/triage-text.ts";
import { auditStatusLabel } from "../src/rules-text.ts";

const pref = (scope, key, route) => ({ scope, key, route, updated_at: 1 });

test("la dirección y el dominio se normalizan como en el backend", () => {
  assert.deepEqual(senderParts('"Ana, S.A." <ANA+Tag@Alfa.Test>'), { address: "ana+tag@alfa.test", domain: "alfa.test" });
  assert.deepEqual(senderParts("ana@alfa.test"), { address: "ana@alfa.test", domain: "alfa.test" });
  for (const bad of ["", "sin-arroba", "@alfa.test", "ana@", "ana@sinpunto", "a b@alfa.test", "ana@.alfa.test", null, undefined, 3]) {
    assert.equal(senderParts(bad), null, String(bad));
  }
});

test("manda la preferencia de la dirección y, si no hay, la del dominio", () => {
  const prefs = [pref("domain", "alfa.test", "informative"), pref("address", "ana@alfa.test", "review")];
  assert.equal(preferenceFor(prefs, "Ana <ANA@alfa.test>").route, "review");
  assert.equal(preferenceFor(prefs, "luis@alfa.test").scope, "domain");
  assert.equal(preferenceFor(prefs, "luis@otro.test"), null);
  assert.equal(preferenceFor(prefs, "sin dirección"), null);
});

test("solo se ofrece la acción si hay dirección y la ruta no es ya la vigente", () => {
  const prefs = [pref("address", "ana@alfa.test", "informative")];
  assert.equal(canOfferPreference(prefs, "ana@alfa.test", "informative"), false);
  assert.equal(canOfferPreference(prefs, "ana@alfa.test", "review"), true);
  assert.equal(canOfferPreference([], "ana@alfa.test", "informative"), true);
  assert.equal(canOfferPreference([], null, "informative"), false);
});

test("un dominio de correo público no se ofrece entero, solo la dirección", () => {
  assert.equal(isPublicDomain(" GMAIL.com "), true);
  assert.equal(isPublicDomain("alfa.test"), false);
  assert.deepEqual(preferenceChoices("ana@gmail.com").map((c) => c.scope), ["address"]);
  assert.deepEqual(preferenceChoices("ana@alfa.test").map((c) => [c.scope, c.key]), [
    ["address", "ana@alfa.test"],
    ["domain", "alfa.test"],
  ]);
  assert.deepEqual(preferenceChoices("sin dirección"), []);
});

test("la confirmación explica el efecto y no promete más de lo que hace", () => {
  const informative = preferenceConfirmText("informative", "Ana <ana@alfa.test>");
  assert.match(informative, /ana@alfa\.test/);
  assert.match(informative, /sin pasar por la IA/);
  assert.match(informative, /Los que ya tiene no cambian/);
  const review = preferenceConfirmText("review", "ana@alfa.test");
  assert.match(review, /nunca se guardarán solos en Informativos/);
  assert.match(review, /señales de envío automático/);
  assert.match(preferenceConfirmText("informative", null), /este remitente/);
});

test("el aviso de una regla contradicha nombra las reglas y no bloquea", () => {
  assert.equal(preferenceWarningText({ warnings: [] }), null);
  assert.match(preferenceWarningText({ warnings: [{ rule_id: "a", rule_name: "Clientes" }] }), /la regla activa «Clientes»/);
  const two = preferenceWarningText({ warnings: [{ rule_id: "a", rule_name: "A" }, { rule_id: "b", rule_name: "B" }] });
  assert.match(two, /las reglas activas «A» y «B»/);
});

test("textos de guardado y de lista", () => {
  const p = pref("domain", "alfa.test", "informative");
  assert.equal(preferenceLine(p), "Todo @alfa.test: siempre a informativos");
  assert.equal(preferenceSavedText({ preference: p, changed: true, warnings: [] }), "Preferencia guardada. Todo @alfa.test: siempre a informativos");
  assert.match(preferenceSavedText({ preference: p, changed: false, warnings: [] }), /^Ya estaba fijada/);
});

test("el Historial muestra la preferencia en palabras y no el JSON", () => {
  const set = JSON.stringify({ scope: "address", key: "ana@alfa.test", route: "informative", previous: null });
  assert.equal(preferenceAuditText(set), "ana@alfa.test: siempre a informativos (antes: sin preferencia)");
  const changed = JSON.stringify({ scope: "domain", key: "alfa.test", route: "review", previous: "informative" });
  assert.equal(preferenceAuditText(changed), "Todo @alfa.test: siempre a pendientes (antes: siempre a informativos)");
  const removed = JSON.stringify({ scope: "address", key: "ana@alfa.test", previous: "review" });
  assert.equal(preferenceAuditText(removed), "ana@alfa.test: preferencia quitada (era: siempre a pendientes)");
  assert.equal(preferenceAuditText("texto libre"), "texto libre");
  assert.equal(preferenceAuditText(null), null);
  assert.equal(auditStatusLabel("sender_preference", "set"), "Fijada");
  assert.equal(auditStatusLabel("sender_preference", "removed"), "Quitada");
});

test("motivo de un informativo por hilo o por preferencia", () => {
  assert.equal(informativeReason({ triage: { route: "informativo", codes: [], source: "thread", already_replied: true } }), "Ya respondió en este hilo");
  // El hilo manda aunque el correo también llevara una señal fuerte.
  assert.equal(
    informativeReason({ triage: { route: "informativo", codes: ["noreply_address"], source: "thread", already_replied: true }, message: { signals: ["noreply_address"] } }),
    "Ya respondió en este hilo",
  );
  assert.match(informativeReason({ triage: { route: "informativo", codes: [], source: "preference", preference_scope: "address" } }), /este remitente va siempre a Informativos/);
  assert.match(informativeReason({ triage: { route: "informativo", codes: [], source: "preference", preference_scope: "domain" } }), /este dominio va siempre a Informativos/);
  // Sin cambios para el resto.
  assert.equal(informativeReason({ triage: { route: "informativo", codes: ["noreply_address"], source: "local" } }), "Enviado desde una dirección que no admite respuestas");
});
