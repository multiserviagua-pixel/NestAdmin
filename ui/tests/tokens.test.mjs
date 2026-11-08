// D136: los colores de texto cumplen WCAG AA sobre los fondos en que se usan, en claro y oscuro.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const css = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

function tokens(block) {
  return Object.fromEntries([...block.matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{6})\s*;/g)].map((m) => [m[1], m[2]]));
}

const light = tokens(css.slice(0, css.indexOf("@media (prefers-color-scheme: dark)")));
const darkBlock = css.slice(css.indexOf("@media (prefers-color-scheme: dark)"));
const dark = { ...light, ...tokens(darkBlock.slice(0, darkBlock.indexOf("\n}\n"))) };

function luminance(hex) {
  const channel = (i) => {
    const v = parseInt(hex.slice(i, i + 2), 16) / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const TEXT_ON = [
  ["text", "bg"],
  ["text", "surface"],
  ["text", "surface-2"],
  ["muted", "bg"],
  ["muted", "surface"],
  ["muted", "surface-2"],
  ["brand", "surface"],
  ["brand", "brand-soft"],
  ["brand-ink", "brand"],
  ["ok", "ok-soft"],
  ["warn", "warn-soft"],
  ["error", "error-soft"],
  ["info", "info-soft"],
  ["error", "surface"],
  ["warn", "surface"],
];

for (const [name, theme] of [["claro", light], ["oscuro", dark]]) {
  test(`contraste de texto AA en tema ${name}`, () => {
    for (const [fg, bg] of TEXT_ON) {
      assert.ok(theme[fg] && theme[bg], `falta el token ${fg} o ${bg}`);
      const ratio = contrast(theme[fg], theme[bg]);
      assert.ok(ratio >= 4.5, `${fg} sobre ${bg}: ${ratio.toFixed(2)} < 4,5`);
    }
  });

  test(`el borde de los controles y el foco se distinguen en tema ${name}`, () => {
    for (const bg of ["surface", "bg"]) {
      assert.ok(contrast(theme["line-strong"], theme[bg]) >= 3, `line-strong sobre ${bg}`);
      assert.ok(contrast(theme.focus, theme[bg]) >= 3, `focus sobre ${bg}`);
    }
  });
}

test("no hay texto por debajo del mínimo de 12 px", () => {
  for (const m of css.matchAll(/--fs-[\w-]+:\s*(\d+)px/g)) assert.ok(Number(m[1]) >= 12, m[0]);
  assert.doesNotMatch(css, /font-size:\s*(0?\.\d+)rem/);
  assert.doesNotMatch(css, /text-transform:\s*uppercase/);
});
