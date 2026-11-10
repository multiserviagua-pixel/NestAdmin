import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// En Tauri 2 el complemento de diálogos hace asíncronos `window.confirm` y `window.alert`: un
// `confirm(...)` sin `await` es una promesa, siempre verdadera, y la acción se ejecutaba antes
// de que la persona contestara. Toda confirmación pasa por `ask`, que espera la respuesta.
const source = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

// `prompt` no lo sustituye el complemento (solo `alert` y `confirm`): sigue siendo síncrono.
test("no queda ningún confirm ni alert sin esperar fuera de ask", () => {
  const bare = [...source.matchAll(/(?<![\w.])(confirm|alert)\(/g)].map((m) => m[0]);
  assert.deepEqual(bare, []);
  const direct = [...source.matchAll(/window\.(confirm|alert)\(/g)].map((m) => m[0]);
  assert.deepEqual(direct, ["window.confirm("], "solo ask llama a window.confirm");
});

test("ask espera la respuesta y cada llamada a ask lleva await", () => {
  assert.match(source, /async function ask\(message: string\): Promise<boolean> \{\s*return await \(window\.confirm\(message\)/);
  const calls = [...source.matchAll(/(\bawait\s+)?\bask\(/g)].filter((m) => !source.slice(m.index - 15, m.index).includes("function"));
  assert.ok(calls.length >= 8, `llamadas a ask: ${calls.length}`);
  for (const m of calls) assert.ok(m[1], `ask sin await en la posición ${m.index}`);
});
