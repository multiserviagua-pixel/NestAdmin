// Componentes de interfaz (D136). Todo el texto se inserta como texto, nunca como HTML (D16).

export type Child = Node | string | number | null | undefined | false;
export type Attrs = Record<string, string | boolean | ((event: Event) => void) | undefined>;
export type Tone = "neutral" | "info" | "ok" | "warn" | "error";

export function h(tag: string, attrs: Attrs = {}, ...children: Child[]): HTMLElement {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === false) continue;
    if (typeof value === "function") el.addEventListener(key.replace(/^on/, ""), value);
    else if (value === true) el.setAttribute(key, "");
    else el.setAttribute(key, value);
  }
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : String(child));
  }
  return el;
}

const ICON_PATH = {
  inbox: "M2 9h3.2l1.3 2h3l1.3-2H14M3.4 3h9.2L14 9v4H2V9z",
  tasks: "M6 4h8M6 8h8M6 12h8M2.5 4h.8M2.5 8h.8M2.5 12h.8",
  history: "M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12zM8 5v3.2l2 1.8",
  rules: "M2 5h7M12.5 5H14M2 11h1.5M7 11h7M9 3.5h3.5v3H9zM3.5 9.5H7v3H3.5z",
  mail: "M2 3.5h12v9H2zM2.3 4l5.7 4.5L13.7 4",
  settings:
    "M8 10.2a2.2 2.2 0 1 0 0-4.4 2.2 2.2 0 0 0 0 4.4zM8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6 5 5M11 11l1.4 1.4M12.4 3.6 11 5M5 11l-1.4 1.4",
  setup: "M8 2v8M5 7l3 3 3-3M3 13.5h10",
} as const;

export type IconName = keyof typeof ICON_PATH;

/** Icono decorativo: el nombre accesible lo da siempre el texto que lo acompaña. */
export function icon(name: IconName): SVGElement {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(NS, "path");
  path.setAttribute("d", ICON_PATH[name]);
  svg.append(path);
  return svg;
}

export interface TabItem<K extends string> {
  key: K;
  label: string;
  count?: number;
}

/**
 * Pestañas con el patrón ARIA: una sola es tabulable y las flechas, Inicio y Fin mueven la
 * selección. `onSelect` debe repintar; `data-focus-key` permite que el foco sobreviva al repintado.
 */
export function tabs<K extends string>(options: { label: string; items: TabItem<K>[]; active: K; panelId: string; onSelect: (key: K) => void }): HTMLElement {
  const { label, items, active, panelId, onSelect } = options;
  const move = (event: Event, index: number) => {
    const key = (event as KeyboardEvent).key;
    const target =
      key === "ArrowRight" ? (index + 1) % items.length
      : key === "ArrowLeft" ? (index - 1 + items.length) % items.length
      : key === "Home" ? 0
      : key === "End" ? items.length - 1
      : -1;
    if (target < 0) return;
    event.preventDefault();
    onSelect(items[target]!.key);
    // `onSelect` repinta de forma síncrona: el foco pasa a la pestaña recién creada.
    const focusKey = `tab:${label}:${items[target]!.key}`;
    [...document.querySelectorAll<HTMLElement>("[data-focus-key]")].find((el) => el.dataset.focusKey === focusKey)?.focus();
  };
  return h(
    "div",
    { class: "tabs", role: "tablist", "aria-label": label },
    ...items.map((item, index) =>
      h(
        "button",
        {
          class: item.key === active ? "tab active" : "tab",
          role: "tab",
          type: "button",
          "aria-selected": item.key === active ? "true" : "false",
          "aria-controls": panelId,
          tabindex: item.key === active ? "0" : "-1",
          "data-focus-key": `tab:${label}:${item.key}`,
          onclick: () => onSelect(item.key),
          onkeydown: (event) => move(event, index),
        },
        item.label,
        item.count === undefined ? null : h("span", { class: "tab-count" }, item.count),
      ),
    ),
  );
}

/** Etiqueta de estado: siempre lleva texto; el color nunca es la única señal. */
export function statusChip(text: Child, tone: Tone = "neutral"): HTMLElement {
  return h("span", { class: `chip tone-${tone}` }, text);
}

/** Incidencia con su porqué. Sin `why`, solo la etiqueta (para filas, que ya son botones). */
export function flag(label: string, tone: Tone = "warn", why?: string): HTMLElement {
  return h("span", { class: "flag-wrap" }, statusChip(label, tone), why ? helpTip(why, `Por qué: ${label}`) : null);
}

let helpTipId = 0;

/**
 * Ayuda propia: se abre con el ratón y con el foco, se cierra con Esc y se puede pasar el ratón
 * por encima. Nunca debe contener lo único que avisa de algo crítico.
 */
export function helpTip(text: string, label = "Más información"): HTMLElement {
  const id = `help-${++helpTipId}`;
  const tip = h("span", { class: "help-pop", id, role: "tooltip", hidden: true }, text);
  const open = () => {
    tip.hidden = false;
    // Posición fija para que un panel con desplazamiento no la recorte.
    const box = trigger.getBoundingClientRect();
    const width = Math.min(320, window.innerWidth - 16);
    tip.style.width = `${width}px`;
    tip.style.left = `${Math.max(8, Math.min(box.left, window.innerWidth - width - 8))}px`;
    const below = box.bottom + 6;
    tip.style.top = below + tip.offsetHeight > window.innerHeight - 8 ? `${Math.max(8, box.top - tip.offsetHeight - 6)}px` : `${below}px`;
  };
  const close = () => {
    tip.hidden = true;
  };
  const trigger = h("button", { class: "help-trigger", type: "button", "aria-label": label, "aria-describedby": id }, "?");
  return h(
    "span",
    {
      class: "help",
      onmouseenter: open,
      onmouseleave: () => {
        if (!wrapperHasFocus()) close();
      },
      onfocusin: open,
      onfocusout: close,
      onkeydown: (event) => {
        if ((event as KeyboardEvent).key === "Escape" && !tip.hidden) {
          event.stopPropagation();
          close();
        }
      },
    },
    trigger,
    tip,
  );
  function wrapperHasFocus() {
    return document.activeElement === trigger;
  }
}

export function emptyState(title: string, hint?: Child, action?: Child): HTMLElement {
  return h("div", { class: "empty-state" }, h("p", { class: "empty-title" }, title), hint ? h("p", { class: "empty-hint" }, hint) : null, action);
}

export function sectionHeader(title: string, ...actions: Child[]): HTMLElement {
  return h("div", { class: "section-header" }, h("h3", {}, title), actions.length ? h("div", { class: "section-actions" }, ...actions) : null);
}

/** Tabla densa con cabecera. Las columnas `numeric` se alinean a la derecha. */
export function dataTable(columns: Array<{ label: string; numeric?: boolean }>, rows: Child[][]): HTMLElement {
  const cls = (numeric?: boolean) => (numeric ? "numeric" : undefined);
  return h(
    "div",
    { class: "table-wrap" },
    h(
      "table",
      { class: "grid" },
      h("thead", {}, h("tr", {}, ...columns.map((c) => h("th", { class: cls(c.numeric), scope: "col" }, c.label)))),
      h("tbody", {}, ...rows.map((row) => h("tr", {}, ...row.map((cell, i) => h("td", { class: cls(columns[i]?.numeric) }, cell))))),
    ),
  );
}

/** Barra de acciones fija al pie de un panel. `hint` explica por qué algo está bloqueado. */
export function actionBar(hint: Child, ...buttons: Child[]): HTMLElement {
  return h("div", { class: "action-bar" }, h("div", { class: "action-buttons" }, ...buttons), hint ? h("p", { class: "action-hint", role: "status" }, hint) : null);
}
