// Right-click context menu; modules contribute items with `add`.

import { el as h } from "./dom.js";

const items = [];
const GROUPS = ["route", "locate", "copy"];
let el;

/** An item of `group` ("route", "locate" or "copy"), hidden when `when(ctx)` is false. */
export function add({ group = "locate", label, when, run }) {
  items.push({ group, label, when, run });
}

/** `ctx`: `{ lngLat, point, target }`, `point` in map pixels, `target` "map" or e.g. "marker". */
export function open(ctx) {
  const visible = items
    .filter((item) => !item.when || item.when(ctx))
    .sort((a, b) => GROUPS.indexOf(a.group) - GROUPS.indexOf(b.group));
  if (visible.length === 0) return close();

  el.replaceChildren(h("div", { class: "menu-head" }, `${ctx.lngLat.lat.toFixed(6)}, ${ctx.lngLat.lng.toFixed(6)}`));
  visible.forEach((item, i) => {
    if (i > 0 && item.group !== visible[i - 1].group) el.append(h("hr"));
    el.append(h("button", { type: "button", role: "menuitem", onclick: () => (close(), item.run(ctx)) }, item.label));
  });

  el.hidden = false;
  const { width, height } = el.parentElement.getBoundingClientRect();
  const box = el.getBoundingClientRect();
  el.style.left = `${Math.min(ctx.point.x, width - box.width - 4)}px`;
  el.style.top = `${Math.min(ctx.point.y, height - box.height - 4)}px`;
  el.querySelector("button")?.focus({ preventScroll: true });
}

export function close() {
  if (el) el.hidden = true;
}

export function isOpen() {
  return el && !el.hidden;
}

export function init(map) {
  el = document.getElementById("context-menu");
  map.on("contextmenu", (e) => {
    e.preventDefault();
    open({ lngLat: e.lngLat, point: e.point, target: "map" });
  });
  map.on("movestart", close);
  map.on("click", close);
  document.addEventListener("keydown", (e) => {
    if (!isOpen()) return;
    if (e.key === "Escape") close();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const buttons = [...el.querySelectorAll("button")];
      const i = buttons.indexOf(document.activeElement);
      const next = buttons[(i + (e.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length];
      next?.focus();
      e.preventDefault();
    }
  });
  document.addEventListener("pointerdown", (e) => {
    if (isOpen() && !el.contains(e.target)) close();
  });
}
