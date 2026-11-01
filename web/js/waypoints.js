// Waypoint list, markers and route-building menu items; all of it renders from and writes to `wp`.

import { formatLatLon, parseLatLon, round6, typed, untyped } from "./coords.js";
import { el as h } from "./dom.js";

/** Location types, see `Location` in src/openapi.yaml. */
const TYPES = [
  { value: "break", label: "Break", hint: "Ends a leg, u-turns allowed" },
  { value: "through", label: "Through", hint: "No new leg, no u-turn" },
  { value: "via", label: "Via", hint: "No new leg, u-turns allowed" },
  { value: "break_through", label: "Break through", hint: "Ends a leg, no u-turn" },
];
const SIDES = [
  { value: "either", label: "Either" },
  { value: "same", label: "Same" },
  { value: "opposite", label: "Opposite" },
];
/** Keys with their own controls; the other location options are edited as `key=value` text. */
const FORM_KEYS = new Set(["lat", "lon", "type", "preferred_side", "heading"]);
const COORD_INPUT = { class: "field mono wp-coord", type: "text", spellcheck: "false", autocomplete: "off" };

let app, map, state;
let ids = []; // stable keys parallel to `wp`, so rows, markers and open option panels follow reorders
let nextId = 1;
const openOptions = new Set();
const rows = new Map(); // id -> row view
const markers = new Map(); // id -> { marker, el, label, dragging }
let draft; // the empty row after the waypoints, for the next one
let hotId = null;
let listEl, panel;
let rendering = false; // removing a focused field fires its blur, which must not commit again

export function init(a) {
  app = a;
  map = a.map;
  state = a.state;
  panel = document.getElementById("panel-waypoints");
  buildPanel();
  state.on("wp", () => render());
  render();
  initMenu();
}

const list = () => state.get("wp");
const letter = (i) => (i < 26 ? String.fromCharCode(65 + i) : letter(Math.floor(i / 26) - 1) + letter(i % 26));
const point = (lngLat) => ({ lat: round6(lngLat.lat), lon: round6(lngLat.lng) });
const nameOf = (i, n) => (i === 0 ? "start" : i === n - 1 ? "destination" : `stop ${letter(i)}`);
const roleOf = (i, n) => (i === 0 ? "start" : i === n - 1 ? "end" : "mid");

function syncIds() {
  const n = list().length;
  if (ids.length !== n) {
    ids = Array.from({ length: n }, () => nextId++);
    openOptions.clear();
  }
}

/** Writes a new list with its ids. */
function commit(next, nextIds, message) {
  if (JSON.stringify(list()) === JSON.stringify(next)) return;
  ids = nextIds;
  state.set({ wp: next });
  if (message) app.status(message);
}

function insert(at, p) {
  const wp = list().slice();
  const keys = ids.slice();
  wp.splice(at, 0, p);
  keys.splice(at, 0, nextId++);
  commit(wp, keys);
}

/** Replaces waypoint `i` with `fn(waypoint)`. */
function update(i, fn, message) {
  const wp = list().slice();
  wp[i] = fn(wp[i]);
  commit(wp, ids.slice(), message);
}

/** Moves waypoint `i` to `p`, keeping its options. */
const replace = (i, p, message) => update(i, (w) => ({ ...w, ...p }), message);

function remove(i) {
  commit(
    list().filter((_, j) => j !== i),
    ids.filter((_, j) => j !== i),
  );
}

function move(from, to) {
  if (from === to || to < 0 || to >= list().length) return;
  const wp = list().slice();
  const keys = ids.slice();
  wp.splice(to, 0, ...wp.splice(from, 1));
  keys.splice(to, 0, ...keys.splice(from, 1));
  commit(wp, keys);
}

/** Sets or (with `undefined`/"") removes location options of waypoint `i`. */
function setOptions(i, patch) {
  update(i, (w) => {
    const next = { ...w, ...patch };
    for (const [key, value] of Object.entries(patch)) if (value === undefined || value === "") delete next[key];
    return next;
  });
}

const svg = (paths) => {
  const node = h("svg", { viewBox: "0 0 16 16", width: 16, height: 16, "aria-hidden": "true" });
  node.innerHTML = paths;
  return node;
};
const ICONS = {
  remove: '<path d="M4.5 4.5l7 7m0-7l-7 7" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>',
  options:
    '<g stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><path d="M2.5 5h11M2.5 11h11"/></g><g fill="var(--panel)" stroke="currentColor" stroke-width="1.4"><circle cx="10.5" cy="5" r="1.8"/><circle cx="5.5" cy="11" r="1.8"/></g>',
};

function buildPanel() {
  listEl = h("ol", { class: "wp-list", "aria-label": "Waypoints, in route order" });
  const action = (act, title, onclick, label) =>
    h("button", { type: "button", class: "btn btn-ghost wp-act", "data-act": act, title, onclick }, label);
  const reverse = () => commit(list().slice().reverse(), ids.slice().reverse(), "Reversed waypoints");
  const actions = h(
    "span",
    { class: "wp-actions" },
    action("reverse", "Reverse the route order", reverse, "Reverse"),
    action("clear", "Remove all waypoints", () => commit([], []), "Clear"),
  );
  panel.replaceChildren(h("h2", { class: "panel-title" }, h("span", {}, "Waypoints"), actions), listEl);
  draft = createDraft();

  Sortable.create(listEl, {
    handle: ".wp-item[data-id] .wp-badge",
    draggable: ".wp-item[data-id]",
    animation: 120,
    forceFallback: true,
    fallbackTolerance: 3,
    ghostClass: "wp-ghost",
    dragClass: "wp-dragging",
    onEnd: (e) => {
      if (e.oldDraggableIndex === e.newDraggableIndex) return;
      // Sortable moved the DOM already; put it back so `render` stays the only writer
      const items = [...listEl.querySelectorAll(".wp-item[data-id]")];
      items.sort((a, b) => ids.indexOf(+a.dataset.id) - ids.indexOf(+b.dataset.id));
      listEl.prepend(...items);
      move(e.oldDraggableIndex, e.newDraggableIndex);
    },
  });
}

function render() {
  if (rendering) return;
  rendering = true;
  try {
    syncIds();
    // Reordering moves DOM nodes, which drops focus; put it back afterwards
    const active = document.activeElement;
    const wp = list();
    const n = wp.length;
    prune(rows, (row) => row.li.remove());
    ids.forEach((id, i) => {
      let row = rows.get(id);
      if (!row) rows.set(id, (row = createRow(id)));
      placeAt(row.li, i);
      updateRow(row, wp[i], i, n);
    });
    updateDraft(n);
    placeAt(draft.li, n);
    panel.querySelector('[data-act="reverse"]').disabled = n < 2;
    panel.querySelector('[data-act="clear"]').disabled = n === 0;
    renderMarkers();
    if (active?.isConnected && document.activeElement !== active) active.focus({ preventScroll: true });
  } finally {
    rendering = false;
  }
}

/** Drops the views of waypoints no longer in the list. */
function prune(views, destroy) {
  for (const [id, view] of views) if (!ids.includes(id)) destroy(view), views.delete(id);
}

function placeAt(el, i) {
  if (listEl.children[i] !== el) listEl.insertBefore(el, listEl.children[i] ?? null);
}

/** Type as Valhalla applies it: the first and last location are always break. */
function effectiveType(wp, i, n) {
  return i === 0 || i === n - 1 ? "break" : (wp.type ?? "break");
}

function createRow(id) {
  const badge = h("span", { class: "wp-badge" });
  const input = h("input", { ...COORD_INPUT, placeholder: "lat, lon" });
  const toggle = h(
    "button",
    { type: "button", class: "btn btn-ghost btn-icon wp-opts-toggle", title: "Location options" },
    svg(ICONS.options),
    h("span", { class: "wp-count" }),
  );
  const removeBtn = h("button", { type: "button", class: "btn btn-ghost btn-icon wp-remove" }, svg(ICONS.remove));
  const msg = h("p", { class: "wp-msg", hidden: true });
  const options = h("div", { class: "wp-opts", hidden: true });
  const li = h(
    "li",
    { class: "wp-item", "data-id": id },
    h("div", { class: "wp-row" }, badge, input, toggle, removeBtn),
    msg,
    options,
  );
  const row = { id, li, badge, input, toggle, removeBtn, msg, options, shown: "", fields: null };
  const index = () => ids.indexOf(id);

  li.addEventListener("mouseenter", () => setHot(id));
  li.addEventListener("mouseleave", () => setHot(null));
  li.addEventListener("keydown", (e) => {
    if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
    if (e.target.closest(".wp-opts")) return;
    e.preventDefault();
    move(index(), index() + (e.key === "ArrowUp" ? -1 : 1));
  });
  wireCoordInput(input, msg, () => row.shown, (p) => replace(index(), p));
  toggle.addEventListener("click", () => {
    if (openOptions.has(id)) openOptions.delete(id);
    else openOptions.add(id);
    render();
  });
  removeBtn.addEventListener("click", () => remove(index()));
  return row;
}

function updateRow(row, wp, i, n) {
  const L = letter(i);
  const type = effectiveType(wp, i, n);
  row.badge.textContent = L;
  row.badge.dataset.role = roleOf(i, n);
  row.badge.dataset.type = type;
  const label = TYPES.find((t) => t.value === type)?.label ?? type;
  const typeLabel = type === "break" ? "" : `, ${label.toLowerCase()} point`;
  row.badge.title = `${L}, ${nameOf(i, n)}${typeLabel}. Drag to reorder (Alt+↑/↓)`;
  row.input.title = `${L}, ${nameOf(i, n)}: lat, lon`;
  row.removeBtn.title = n === 1 ? "Clear removes the last waypoint" : `Remove ${L}`;
  row.removeBtn.disabled = n === 1;

  const text = formatLatLon(wp);
  if (text !== row.shown) {
    row.shown = text;
    row.input.value = text;
    setInvalid(row.input, row.msg, null);
  }

  const extra = Object.keys(wp).length - 2;
  row.toggle.querySelector(".wp-count").textContent = extra ? String(extra) : "";
  const open = openOptions.has(row.id);
  row.toggle.setAttribute("aria-expanded", String(open));
  row.li.classList.toggle("has-options", extra > 0);
  row.options.hidden = !open;
  if (open) renderOptions(row, wp, i, n);
}

/** The empty row after the waypoints: a point typed or pasted into it is appended. */
function createDraft() {
  const badge = h("span", { class: "wp-badge is-empty" });
  const input = h("input", COORD_INPUT);
  const msg = h("p", { class: "wp-msg", hidden: true });
  const li = h("li", { class: "wp-item" }, h("div", { class: "wp-row" }, badge, input), msg);
  wireCoordInput(input, msg, () => "", (p) => insert(list().length, p));
  return { li, badge, input };
}

function updateDraft(n) {
  draft.badge.textContent = letter(n);
  draft.badge.dataset.role = n === 0 ? "start" : "end";
  draft.input.placeholder =
    n === 0 ? "Right-click the map or paste" : n === 1 ? "Destination: right-click or paste" : "Add a destination";
  draft.input.title = `${n === 0 ? "Start" : "Destination"}: lat, lon`;
}

/** Coordinate field: Enter or blur commits, Escape reverts, a pasted `lat, lon` commits right away. */
function wireCoordInput(input, msg, current, onCommit) {
  const tryCommit = (text) => {
    if (text.trim() === current().trim()) {
      setInvalid(input, msg, null);
      return true;
    }
    const parsed = parseLatLon(text);
    setInvalid(input, msg, parsed.error);
    if (parsed.error) return false;
    onCommit(parsed.point);
    // Also covers the same point typed differently: show the canonical text again
    if (input.isConnected) input.value = current();
    return true;
  };
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (tryCommit(input.value)) input.select();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      input.value = current();
      setInvalid(input, msg, null);
    }
  });
  input.addEventListener("blur", () => {
    if (rendering || !input.isConnected) return;
    if (input.value !== current()) tryCommit(input.value);
  });
  input.addEventListener("paste", (e) => {
    const text = e.clipboardData?.getData("text") ?? "";
    const whole = input.selectionStart === 0 && input.selectionEnd === input.value.length;
    if (!whole && input.value.trim()) return; // pasting into the middle of an edit
    if (parseLatLon(text).error) return; // let it land, Enter will explain what's wrong
    e.preventDefault();
    input.value = text.trim();
    tryCommit(text);
  });
  input.addEventListener("input", () => {
    if (input.getAttribute("aria-invalid") === "true") setInvalid(input, msg, null);
  });
}

function setInvalid(input, msg, error) {
  if (error) input.setAttribute("aria-invalid", "true");
  else input.removeAttribute("aria-invalid");
  msg.textContent = error ?? "";
  msg.hidden = !error;
}

function renderOptions(row, wp, i, n) {
  row.fields ??= buildOptions(row);
  const f = row.fields;
  const end = i === 0 || i === n - 1;
  const type = wp.type ?? "break";
  const press = (seg, value) => {
    for (const b of seg.querySelectorAll("button")) b.setAttribute("aria-pressed", String(b.dataset.value === value));
  };
  press(f.type, type);
  press(f.side, wp.preferred_side ?? "either");
  f.typeNote.textContent = end && type !== "break" ? "Valhalla treats the first and last waypoint as break" : "";
  f.typeNote.hidden = !f.typeNote.textContent;
  const more = Object.entries(wp).filter(([k]) => !FORM_KEYS.has(k));
  for (const [input, value] of [
    [f.heading, wp.heading === undefined ? "" : String(wp.heading)],
    [f.more, more.map(([k, v]) => `${k}=${untyped(v)}`).join(" ")],
  ]) {
    if (document.activeElement !== input && input.getAttribute("aria-invalid") !== "true") input.value = value;
    input.dataset.committed = value;
  }
  f.reset.disabled = Object.keys(wp).length <= 2;
}

function buildOptions(row) {
  const index = () => ids.indexOf(row.id);
  const seg = (name, choices, fallback, label) =>
    h(
      "div",
      { class: "seg wp-seg", role: "group", "aria-label": label },
      choices.map((c) =>
        h(
          "button",
          {
            type: "button",
            "data-value": c.value,
            title: c.hint,
            onclick: () => setOptions(index(), { [name]: c.value === fallback ? undefined : c.value }),
          },
          c.label,
        ),
      ),
    );
  const type = seg("type", TYPES, "break", "Location type");
  const side = seg("preferred_side", SIDES, "either", "Preferred side of street");
  const typeNote = h("p", { class: "wp-note muted", hidden: true });
  const msg = h("p", { class: "wp-msg", hidden: true });
  // A text field that commits `parse(text).patch` on Enter and blur; Escape reverts it
  const field = (attrs, parse) => {
    const input = h("input", { class: "field", type: "text", ...attrs });
    const apply = () => {
      const text = input.value.trim();
      if (text === input.dataset.committed) return setInvalid(input, msg, null);
      const { patch, error } = parse(text);
      setInvalid(input, msg, error);
      if (!error) setOptions(index(), patch);
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") apply();
      if (e.key === "Escape") {
        input.value = input.dataset.committed;
        setInvalid(input, msg, null);
      }
    });
    input.addEventListener("blur", () => rendering || apply());
    return input;
  };
  const heading = field(
    { inputmode: "numeric", placeholder: "none", title: "Direction of travel, degrees clockwise from north" },
    (text) => {
      const value = text === "" ? undefined : Number(text);
      if (value !== undefined && !(Number.isInteger(value) && value >= 0 && value <= 359)) {
        return { error: "Heading must be a whole number, 0 to 359" };
      }
      return { patch: { heading: value } };
    },
  );
  const more = field(
    { class: "field mono", placeholder: "radius=15 search_cutoff=500", title: "Other location options, key=value" },
    (text) => {
      // Unlisted keys are removed
      const keys = Object.keys(list()[index()]).filter((k) => !FORM_KEYS.has(k));
      const patch = Object.fromEntries(keys.map((k) => [k, undefined]));
      for (const item of text.split(/\s+/).filter(Boolean)) {
        const [key, value] = item.split("=");
        if (!/^[a-z_]+$/.test(key) || FORM_KEYS.has(key) || !value) return { error: `Not a key=value option: ${item}` };
        patch[key] = typed(value);
      }
      return { patch };
    },
  );
  const reset = h(
    "button",
    {
      type: "button",
      class: "btn btn-ghost wp-reset",
      onclick: () => update(index(), ({ lat, lon }) => ({ lat, lon }), `Reset options of ${letter(index())}`),
    },
    "Reset to defaults",
  );
  // A <label> would forward clicks on its text to the first button of a segmented control
  const labeled = (tag, label, ...control) =>
    h(tag, { class: "wp-field" }, h("span", { class: "wp-label" }, label), control);
  row.options.replaceChildren(
    labeled("div", "Type", type),
    typeNote,
    labeled("div", "Side", side),
    labeled("label", "Heading", heading, h("span", { class: "muted" }, "°")),
    labeled("label", "More", more),
    msg,
    h("div", { class: "wp-opts-foot" }, h("span", { class: "muted" }, "Empty fields use Valhalla defaults"), reset),
  );
  return { type, typeNote, side, heading, more, reset };
}

const PIN =
  '<svg viewBox="0 0 24 32" width="28" height="37" aria-hidden="true"><path class="wp-pin-body" d="M12 30.5C12 30.5 2.75 19.6 2.75 12a9.25 9.25 0 0 1 18.5 0C21.25 19.6 12 30.5 12 30.5Z"/><circle class="wp-pin-ring" cx="12" cy="12" r="6.6"/></svg>';

function renderMarkers() {
  const wp = list();
  const n = wp.length;
  prune(markers, (m) => m.marker.remove());
  ids.forEach((id, i) => {
    let m = markers.get(id);
    if (!m) markers.set(id, (m = createMarker(id)));
    if (!m.dragging) m.marker.setLngLat([wp[i].lon, wp[i].lat]);
    m.label.textContent = letter(i);
    m.el.dataset.role = roleOf(i, n);
    m.el.dataset.type = effectiveType(wp[i], i, n);
    m.el.title = `${letter(i)}, ${nameOf(i, n)} · ${formatLatLon(wp[i])}\nDrag to move`;
    m.el.style.zIndex = String(id === hotId ? 1000 : n - i);
  });
}

function createMarker(id) {
  const label = h("span", { class: "wp-pin-label" });
  const el = h("div", { class: "wp-marker" }, h("div", { class: "wp-pin" }));
  el.firstChild.innerHTML = PIN;
  el.firstChild.append(label);
  const marker = new maplibregl.Marker({ element: el, draggable: true, anchor: "bottom" }).setLngLat([0, 0]).addTo(map);
  const m = { marker, el, label, dragging: false };
  marker.on("dragstart", () => {
    m.dragging = true;
    el.classList.add("is-dragging");
  });
  marker.on("dragend", () => {
    m.dragging = false;
    el.classList.remove("is-dragging");
    const i = ids.indexOf(id);
    if (i >= 0) replace(i, point(marker.getLngLat()));
  });
  el.addEventListener("mouseenter", () => setHot(id));
  el.addEventListener("mouseleave", () => setHot(null));
  el.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    e.stopPropagation();
    const rect = map.getContainer().getBoundingClientRect();
    app.menu.open({
      lngLat: marker.getLngLat(),
      point: { x: e.clientX - rect.left, y: e.clientY - rect.top },
      target: "marker",
    });
  });
  return m;
}

function setHot(id) {
  if (hotId === id) return;
  for (const key of [hotId, id]) {
    const on = key === id && id !== null;
    rows.get(key)?.li.classList.toggle("is-hot", on);
    const m = markers.get(key);
    if (m) {
      m.el.classList.toggle("is-hot", on);
      m.el.style.zIndex = on ? "1000" : String(list().length - ids.indexOf(key));
    }
  }
  hotId = id;
}

function initMenu() {
  const onMap = (ctx) => ctx.target === "map";
  const n = () => list().length;
  const at = (ctx) => point(ctx.lngLat);
  const add = (item) => app.menu.add({ group: "route", ...item });

  add({
    label: "Directions from here",
    when: onMap,
    run: (ctx) => (n() ? replace(0, at(ctx), "Moved the start") : insert(0, at(ctx))),
  });
  add({
    label: "Directions to here",
    when: (ctx) => onMap(ctx) && n() >= 1,
    run: (ctx) => (n() >= 2 ? replace(n() - 1, at(ctx), "Moved the destination") : insert(n(), at(ctx))),
  });
  add({
    label: "Add stop here",
    when: (ctx) => onMap(ctx) && n() >= 2,
    run: (ctx) => insert(n() - 1, at(ctx)),
  });
}
