// Road speeds: vector tiles from /tiles/speeds (properties in src/speeds.rs) and the #panel-layers controls.

import { el } from "./dom.js";

const SOURCE = "speeds";
// MapLibre loads tiles in a worker, which can't resolve a relative URL
const TILES = `${location.origin}/tiles/speeds/{z}/{x}/{y}`;
const MIN_ZOOM = 6;
const WEEK = 604800;
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

const get = (key) => ["get", key];
const LAYERS = ["speeds-line", "speeds-closed", "speeds-hit"];

/** `value` is what gets colored. */
const MODES = {
  off: { label: "Off" },
  live: {
    label: "Live",
    value: get("live"),
    caption: "Live speed",
    empty: "No live traffic in this tileset. Start the server with --valhalla-traffic.",
  },
  pred: {
    label: "Predicted",
    value: get("p"),
    caption: "Predicted speed",
    empty: "No predicted speeds in view.",
  },
  day: {
    label: "Day",
    value: get("cf"),
    caption: "Day speed (7:00-19:00)",
    empty: "No day speeds in view.",
  },
  night: {
    label: "Night",
    value: get("ff"),
    caption: "Night (free-flow) speed",
    empty: "No night speeds in view.",
  },
};

// Slow to fast
const PALETTE = ["#7a1446", "#d42a2a", "#ee7d1a", "#cfa600", "#23a06b", "#2f7fc4"];
const KMH_STOPS = [5, 20, 40, 60, 90, 120];

let app;
let panel;
let open = false;
let tilesTimer = null;
let hovered = null;
let popup = null;

export async function init(appRef) {
  app = appRef;
  panel = document.getElementById("panel-layers");

  app.state.register("speeds", { encode, decode, initial: { mode: "off", t: null } });
  app.state.on("speeds", () => {
    apply();
    sync();
  });

  const info = await fetch("/api/info")
    .then((r) => r.json())
    .catch(() => ({ graph: false }));
  if (!info.graph) {
    renderUnavailable();
    return;
  }
  addLayers();
  open = mode() !== "off";
  apply();
  render();
  map().on("idle", updateNote);
  map().on("zoomend", updateNote);
}

const map = () => app.map;
const value = () => app.state.get("speeds");
const mode = () => value().mode;

/** `live`, `day`, `pred@<second of week>`... */
function encode({ mode, t }) {
  if (mode === "off") return undefined;
  return `${mode}${mode === "pred" && t != null ? `@${t}` : ""}`;
}

function decode(s) {
  // Ignores the `,rel` of older links
  const [m, t] = s.split(",")[0].split("@");
  if (!MODES[m]) throw new Error(`unknown speeds mode '${m}'`);
  const sec = t === undefined ? null : Number(t);
  return { mode: m, t: sec === null || !Number.isFinite(sec) ? null : snap(sec) };
}

function setValue(patch) {
  app.state.set({ speeds: { ...value(), ...patch } });
}

const snap = (sec) => (((Math.floor(sec / 300) * 300) % WEEK) + WEEK) % WEEK;

/** The browser's time of the week: predicted speeds are in the roads' local time, usually the same. */
function localNow() {
  const d = new Date();
  return snap(d.getDay() * 86400 + d.getHours() * 3600 + d.getMinutes() * 60);
}

/** `Mon 08:05` for a second of the week. */
export function formatTime(sec) {
  const day = Math.floor(sec / 86400);
  const minutes = Math.floor((sec % 86400) / 60);
  return `${DAYS[day]} ${String(Math.floor(minutes / 60)).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`;
}

function tilesUrl() {
  const { mode, t } = value();
  return mode === "pred" && t != null ? `${TILES}?t=${t}` : TILES;
}

function addLayers() {
  appliedTiles = tilesUrl();
  map().addSource(SOURCE, { type: "vector", tiles: [appliedTiles], minzoom: MIN_ZOOM, maxzoom: 14 });
  const hover = ["boolean", ["feature-state", "hover"], false];
  const zooms = [7, 11, 14, 17, 20];
  const byZoom = (v) => ["interpolate", ["exponential", 1.6], ["zoom"], ...zooms.flatMap((z, i) => [z, v[i]])];
  const widths = [0.8, 1.6, 3, 7, 14];
  // Offset to the right of travel, so the two directions of a road sit side by side
  const offset = byZoom([0.4, 1, 2, 4.5, 9]);
  const common = { type: "line", source: SOURCE, "source-layer": "speeds", layout: { visibility: "none" } };
  const add = (layer) => map().addLayer({ ...common, ...layer }, app.slot("speeds"));
  add({
    id: "speeds-line",
    layout: { ...common.layout, "line-join": "round" },
    paint: {
      "line-width": byZoom(widths.map((w) => ["case", hover, Math.max(3, w * 2), w])),
      "line-offset": offset,
      "line-opacity": 0.95,
    },
  });
  // Live closures: dashes on top of the colored line, they are what routing avoids
  add({
    id: "speeds-closed",
    filter: ["==", get("live"), 0],
    paint: { "line-width": byZoom(widths), "line-offset": offset, "line-color": "#1e242c", "line-dasharray": [1, 1] },
  });
  add({ id: "speeds-hit", paint: { "line-width": 12, "line-offset": offset, "line-opacity": 0 } });

  map().on("mousemove", "speeds-hit", onHover);
  map().on("mouseleave", "speeds-hit", clearHover);
  map().on("click", "speeds-hit", (e) => {
    const feature = e.features?.[0];
    if (!feature) return;
    e.preventDefault();
    app.emit("locate:edge", { edgeId: String(feature.id), lngLat: e.lngLat });
  });
}

let appliedTiles = null;

/** Pushes the state into the map: visibility, filter, colors, tile url. */
function apply() {
  if (!map().getSource(SOURCE)) return;
  const m = mode();
  const spec = MODES[m];
  const wasVisible = map().getLayoutProperty("speeds-line", "visibility") === "visible";
  if (m === "off") {
    for (const id of LAYERS) map().setLayoutProperty(id, "visibility", "none");
    clearHover();
    return;
  }
  const color = ["interpolate", ["linear"], spec.value, ...KMH_STOPS.flatMap((s, i) => [s, PALETTE[i]])];
  for (const id of ["speeds-line", "speeds-hit"]) map().setFilter(id, ["has", spec.value[1]]);
  map().setPaintProperty("speeds-line", "line-color", color);
  // Slowest on top, so jams don't hide under free-flowing parallel roads
  map().setLayoutProperty("speeds-line", "line-sort-key", ["-", 0, spec.value]);

  const url = tilesUrl();
  clearTimeout(tilesTimer);
  if (url !== appliedTiles) {
    const reload = () => {
      appliedTiles = url;
      map().getSource(SOURCE).setTiles([url]);
    };
    // Scrubbing the time slider fires on every step, reload tiles only once it settles
    if (wasVisible) tilesTimer = setTimeout(reload, 200);
    else reload();
  }
  for (const id of LAYERS) {
    const shown = id !== "speeds-closed" || m === "live";
    map().setLayoutProperty(id, "visibility", shown ? "visible" : "none");
  }
  updateNote();
}

function onHover(e) {
  const feature = e.features?.[0];
  if (!feature) return clearHover();
  map().getCanvas().style.cursor = "pointer";
  popup ??= new maplibregl.Popup({
    closeButton: false,
    closeOnClick: false,
    className: "speeds-tip",
    offset: 14,
    maxWidth: "300px",
  });
  if (hovered !== feature.id) {
    clearHoverState();
    hovered = feature.id;
    map().setFeatureState({ source: SOURCE, sourceLayer: "speeds", id: hovered }, { hover: true });
    popup.setDOMContent(tooltip(feature.properties));
  }
  popup.setLngLat(e.lngLat);
  if (!popup.isOpen()) popup.addTo(map());
}

function clearHoverState() {
  if (hovered !== null) map().setFeatureState({ source: SOURCE, sourceLayer: "speeds", id: hovered }, { hover: false });
  hovered = null;
}

function clearHover() {
  clearHoverState();
  map().getCanvas().style.cursor = "";
  popup?.remove();
}

/** The edge's speeds at a glance, km/h; "–" for no data. */
function tooltip(p) {
  const v = (x) => x ?? "–";
  const t = value().t;
  const at = mode() === "pred" && t != null ? ` · ${formatTime(t)} ${v(p.p)}` : "";
  const current = p[MODES[mode()].value?.[1]];
  let note = null;
  if (mode() === "live" && current === 0) note = "Closed by live traffic";
  else if (current !== undefined && p.lim) note = `${Math.round((current / p.lim) * 100)}% of the speed limit`;
  return el(
    "div",
    {},
    el("div", {}, `${v(p.lim)} limit · ${p.sp} edge`),
    el("div", {}, `night ${v(p.ff)} · day ${v(p.cf)}`),
    el("div", {}, `live ${p.live === 0 ? "closed" : v(p.live)}${at}`),
    note && el("div", { class: "speeds-tip-ratio" }, note),
  );
}

const button = (label, attrs, onclick) => el("button", { type: "button", ...attrs, onclick }, label);
const gradient = () => `linear-gradient(90deg, ${PALETTE.join(", ")})`;

function toggleButton() {
  const m = mode();
  return button(
    [
      m !== "off" && el("span", { class: "speeds-swatch", style: `background:${gradient()}`, "aria-hidden": true }),
      `Road speeds: ${MODES[m].label.toLowerCase()}`,
    ],
    { class: "speeds-toggle", "aria-expanded": String(open), "aria-controls": "speeds-body" },
    () => {
      open = !open;
      render();
      if (open) panel.querySelector(".speeds-modes [aria-pressed=true]")?.focus();
    },
  );
}

function renderUnavailable() {
  panel.replaceChildren(toggleButton());
  if (!open) return;
  const text = "Road speeds are read from the tileset. Start the server with --valhalla-tiles to see them.";
  panel.append(el("div", { id: "speeds-body", class: "speeds-body" }, el("p", { class: "speeds-note" }, withFlags(text))));
}

let renderedAs = null;
/** What `render` depends on; the time of day only moves the slider. */
const shape = () => JSON.stringify([open, mode(), value().t === null ? null : Math.floor(value().t / 86400)]);

/** Re-renders the panel when its structure changes, otherwise only moves the time controls. */
function sync() {
  if (shape() !== renderedAs) return render();
  const t = value().t;
  const slider = panel.querySelector(".speeds-time input");
  if (slider && t !== null) {
    if (document.activeElement !== slider) slider.value = String(Math.floor((t % 86400) / 300));
    panel.querySelector(".speeds-clock").textContent = formatTime(t).slice(4);
  }
}

function render() {
  if (!map().getSource(SOURCE)) return renderUnavailable();
  const { mode: m, t } = value();
  renderedAs = shape();
  const focused = document.activeElement?.dataset?.focusKey;
  panel.replaceChildren(toggleButton());
  if (!open) return;

  const modes = Object.entries(MODES).map(([id, spec]) =>
    button(spec.label, { "aria-pressed": String(id === m), "data-focus-key": `mode-${id}` }, () =>
      setValue(id === "pred" && value().t == null ? { mode: id, t: localNow() } : { mode: id }),
    ),
  );
  panel.append(
    el(
      "div",
      { id: "speeds-body", class: "speeds-body" },
      el("div", { class: "seg speeds-modes", role: "group", "aria-label": "Speeds layer" }, modes),
      m === "pred" && scrubber(t ?? 0),
      m !== "off" && [legend(m), el("p", { class: "speeds-note", id: "speeds-note", role: "status" })],
    ),
  );
  if (focused) panel.querySelector(`[data-focus-key="${focused}"]`)?.focus();
  updateNote();
}

function scrubber(t) {
  const day = Math.floor(t / 86400);
  const days = DAYS.map((name, i) =>
    button(name, { "aria-pressed": String(i === day), "data-focus-key": `day-${i}` }, () =>
      setValue({ t: i * 86400 + (value().t % 86400) }),
    ),
  );
  const out = el("output", { class: "speeds-clock" });
  const slider = el("input", {
    type: "range",
    min: 0,
    max: 86400 / 300 - 1,
    step: 1,
    value: Math.floor((t % 86400) / 300),
    "data-focus-key": "slider",
    "aria-label": "Time of day, 5-minute steps",
    oninput: () => {
      const sec = day * 86400 + Number(slider.value) * 300;
      show(sec);
      setValue({ t: sec });
    },
  });
  const show = (sec) => {
    out.textContent = formatTime(sec).slice(4);
    slider.setAttribute("aria-valuetext", formatTime(sec));
  };
  show(t);
  const now = button("Now", { class: "btn", "data-focus-key": "now", title: "Your current time of the week" }, () =>
    setValue({ t: localNow() }),
  );
  return el(
    "div",
    { class: "speeds-time" },
    el("div", { class: "seg speeds-days", role: "group", "aria-label": "Day of the week" }, days),
    el("div", { class: "speeds-time-row" }, slider, out, now),
  );
}

function legend(m) {
  return el(
    "div",
    { class: "speeds-legend" },
    el("div", { class: "speeds-legend-caption" }, `${MODES[m].caption}, km/h`),
    el("div", { class: "speeds-bar", style: `background:${gradient()}` }),
    el("div", { class: "speeds-bar-labels" }, KMH_STOPS.map((s) => el("span", {}, String(s)))),
    m === "live" && el("div", { class: "speeds-closed-key" }, el("span", { "aria-hidden": true }), "Closed"),
  );
}

/** Text nodes with command line flags as `<code>`, so they don't break at their hyphens. */
function withFlags(text) {
  return text.split(/(--[a-z-]+)/).map((part, i) => (i % 2 ? el("code", { class: "mono" }, part) : part));
}

/** Explains an empty layer: too far out, or the tileset has no data for this mode here. */
function updateNote() {
  const note = document.getElementById("speeds-note");
  if (!note) return;
  const m = mode();
  let text = "";
  if (m !== "off") {
    if (map().getZoom() < MIN_ZOOM) {
      text = `Zoom in to level ${MIN_ZOOM} to see road speeds.`;
    } else if (map().isSourceLoaded(SOURCE)) {
      const key = MODES[m].value[1];
      const any = map().querySourceFeatures(SOURCE, { sourceLayer: "speeds", filter: ["has", key] }).length > 0;
      if (!any) text = MODES[m].empty;
    }
  }
  if (note.dataset.text !== text) {
    note.dataset.text = text;
    note.replaceChildren(...withFlags(text));
  }
  note.hidden = !text;
}

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && open && panel?.contains(document.activeElement)) {
    open = false;
    render();
    panel.querySelector(".speeds-toggle")?.focus();
  }
});
