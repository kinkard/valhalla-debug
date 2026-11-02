// Edges the search explored, per tree from deep (early) to light (late), from a PMTiles archive.

import { cssVar, el } from "./dom.js";

const SOURCE = "expansion";
const LAYER = "expansion-edges";
/** MapLibre global state key with the slider's search event, used by the layer filter. */
const GLOBAL_T = "expansionEvent";
/** Same as the routes' debounce, so both follow an edit together. */
const RERUN_MS = 250;
/** `stats.algorithm` values; unidirectional searches have one tree, reverse A* only the reverse one. */
const ALGORITHMS = {
  bidirectional_astar: { label: "bidirectional A*", fwd: true, rev: true },
  forward_astar: { label: "forward A*", fwd: true, rev: false },
  reverse_astar: { label: "reverse A* (arrive-by)", fwd: false, rev: true },
};
const PMTILES_JS = "https://cdn.jsdelivr.net/npm/pmtiles@4.3.0/dist/pmtiles.js";
/** MapLibre protocol serving tiles of the loaded archive: `vdexp://<key>/{z}/{x}/{y}`. */
const PROTOCOL = "vdexp";
/** Status of tiles at levels the archive skips, see `serveTile`. */
const SKIPPED_LEVEL = 410;

let app;
let map;
let dock;

/** The loaded expansion: `{ stats, archive, t }`, `archive` is `{ key, pmtiles }`, `t` the slider's event. */
let current = null;
let running = null; // AbortController of the expansion request in flight

const num = new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 }).format;

/** Items separated by a middle dot, skipping empty ones. */
function joined(items) {
  return items
    .filter(Boolean)
    .flatMap((item, i) => (i ? [el("span", { class: "exp-sep", "aria-hidden": "true" }, "\u00b7"), item] : [item]));
}

function canRun() {
  return app.state.get("wp").length >= 2;
}

async function run() {
  if (!canRun()) return app.status("Add a start and a destination to run the expansion", "error");
  running?.abort();
  const controller = (running = new AbortController());
  showLoading();
  renderEntry();
  const begin = performance.now();
  try {
    const loaded = await openArchive(await fetchArchive(app.request.spec(), controller.signal));
    if (running !== controller) return;
    running = null;
    load(loaded);
    const { stats } = loaded;
    const ms = Math.round(performance.now() - begin);
    app.status(`Expansion: ${num(stats.edges)} edges from ${num(stats.records)} search events in ${ms} ms`);
  } catch (err) {
    if (controller.signal.aborted) return;
    running = null;
    showError(err.message);
  }
}

function load({ stats, archive }) {
  removeMapData();
  current = { stats, archive, t: stats.records };
  addMapData();
  renderDock();
  app.state.set({ exp: true });
  app.emit("expansion:loaded");
}

function clear() {
  running?.abort();
  running = null;
  removeMapData();
  const had = current !== null;
  current = null;
  dock.hidden = true;
  dock.replaceChildren();
  app.state.set({ exp: null });
  renderEntry();
  if (had) app.emit("expansion:cleared");
}

function lineColor() {
  // Edges settled early are deep, late ones light
  const max = Math.max(1, current.stats.records);
  const c = (name) => cssVar(`--exp-${name}`);
  const ramp = (dir) => ["interpolate", ["linear"], ["get", "seq"], 0, c(`${dir}-near`), max, c(`${dir}-far`)];
  return ["match", ["get", "dir"], 0, ramp("fwd"), ramp("rev")];
}

function addMapData() {
  const { stats } = current;
  map.setGlobalStateProperty(GLOBAL_T, current.t);
  const tiles = [`${PROTOCOL}://${current.archive.key}/{z}/{x}/{y}`];
  map.addSource(SOURCE, { type: "vector", tiles, bounds: stats.bbox, maxzoom: stats.maxzoom });
  map.addLayer(
    {
      id: LAYER,
      type: "line",
      source: SOURCE,
      "source-layer": "edges",
      filter: ["<=", ["get", "seq"], ["global-state", GLOBAL_T]],
      layout: { "line-cap": "round", "line-join": "round" },
      paint: {
        "line-color": lineColor(),
        "line-width": ["interpolate", ["linear"], ["zoom"], 5, 1, 10, 1.4, 15, 3.1],
        "line-opacity": 0.85,
      },
    },
    app.slot("expansion"),
  );
}

function removeMapData() {
  if (map.getLayer(LAYER)) map.removeLayer(LAYER);
  if (map.getSource(SOURCE)) map.removeSource(SOURCE);
}

// The filter re-lays out the loaded tiles in the workers: one update per frame
let frame = 0;
function setEvent(t) {
  current.t = t;
  cancelAnimationFrame(frame);
  frame = requestAnimationFrame(() => map.setGlobalStateProperty(GLOBAL_T, current.t));
}

let pmtilesLib = null;
let archiveCount = 0;

/** Loads the pmtiles library once, on first use. */
function loadPmtiles() {
  pmtilesLib ??= new Promise((resolve, reject) => {
    const script = el("script", { src: PMTILES_JS, crossorigin: "anonymous" });
    script.onload = () => {
      maplibregl.addProtocol(PROTOCOL, serveTile);
      resolve(window.pmtiles);
    };
    script.onerror = () => {
      pmtilesLib = null;
      reject(new Error(`couldn't load ${PMTILES_JS}`));
    };
    document.head.append(script);
  });
  return pmtilesLib;
}

/** Skipped levels fail, so MapLibre stretches the parent tile; a 404 would read as an empty tile. */
async function serveTile(params, abort) {
  const m = /^[a-z]+:\/\/([^/]+)\/(\d+)\/(\d+)\/(\d+)$/.exec(params.url);
  const archive = current?.archive;
  if (!m || archive?.key !== m[1]) return { data: new Uint8Array() };
  if (!current.stats.levels.includes(Number(m[2])))
    throw new maplibregl.AJAXError(SKIPPED_LEVEL, "Level not in the archive", params.url, new Blob());
  const tile = await archive.pmtiles.getZxy(Number(m[2]), Number(m[3]), Number(m[4]), abort.signal);
  return { data: tile ? new Uint8Array(tile.data) : new Uint8Array() };
}

async function fetchArchive(spec, signal) {
  const response = await fetch("/api/expansion", {
    method: "POST",
    signal,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(spec),
  });
  if (!response.ok) {
    const text = await response.text();
    let message;
    try {
      message = JSON.parse(text).error;
    } catch {
      message = text.slice(0, 200);
    }
    throw new Error(message || `${response.status} ${response.statusText}`);
  }
  return response.arrayBuffer();
}

/** Reads the header and the stats from a PMTiles archive in memory. */
async function openArchive(buffer) {
  const lib = await loadPmtiles();
  const key = `a${++archiveCount}`;
  const pmtiles = new lib.PMTiles({
    getKey: () => key,
    getBytes: async (offset, length) => ({ data: buffer.slice(offset, offset + length) }),
  });
  const header = await pmtiles.getHeader();
  const meta = await pmtiles.getMetadata();
  const stats = { ...meta.valhalla_debug, maxzoom: header.maxZoom };
  return { stats, archive: { key, pmtiles } };
}

function closeButton(title) {
  return el(
    "button",
    { class: "btn btn-ghost btn-icon", type: "button", title, "aria-label": title, onclick: clear },
    "\u00d7",
  );
}

/** The dock's header row: title, `meta`, then `buttons` on the right. */
function head(meta, ...buttons) {
  const title = el("h2", { class: "exp-title" }, "Expansion");
  return el("div", { class: "exp-head" }, title, meta, el("span", { class: "exp-spacer" }), buttons);
}

function showLoading() {
  dock.hidden = false;
  if (current) return;
  dock.replaceChildren(
    head(
      el("span", { class: "exp-meta muted" }, "Running the search..."),
      el("button", { class: "btn btn-ghost", type: "button", onclick: clear }, "Cancel"),
    ),
    el("div", { class: "exp-loading", "aria-hidden": "true" }),
  );
}

function showError(message) {
  dock.hidden = false;
  if (current) {
    renderDock();
    app.status(`Expansion failed: ${message}`, "error");
    return;
  }
  dock.replaceChildren(
    head(
      el("span", { class: "exp-error" }, `Valhalla couldn't expand this request: ${message}`),
      el("button", { class: "btn", type: "button", onclick: run }, "Try again"),
      closeButton("Close"),
    ),
  );
}

function renderDock() {
  const { stats } = current;
  const { label, fwd, rev } = ALGORITHMS[stats.algorithm] ?? ALGORITHMS.bidirectional_astar;
  const swatch = (dir, text) => el("span", { class: "exp-key" }, el("i", { style: `--c:var(--exp-${dir})` }), text);
  const readout = el("span", { class: "exp-readout" });
  const showEvent = () => (readout.textContent = `event ${num(current.t)} of ${num(stats.records)}`);
  const slider = el("input", {
    class: "exp-slider",
    type: "range",
    min: 0,
    max: stats.records,
    value: current.t,
    "aria-label": "Search event",
    oninput: (e) => {
      setEvent(Number(e.target.value));
      showEvent();
    },
  });
  showEvent();
  dock.hidden = false;
  dock.replaceChildren(
    head(
      el(
        "span",
        { class: "exp-meta" },
        joined([label, stats.costing, `${num(stats.edges)} edges`, `${num(stats.records)} events`]),
      ),
      el("span", { class: "exp-legend" }, fwd && swatch("fwd", "forward"), rev && swatch("rev", "reverse")),
      closeButton("Clear expansion"),
    ),
    el("div", { class: "exp-replay" }, slider, readout),
  );
}

/** "Run expansion" in the routes' tools while the dock is closed; every route re-renders them. */
function renderEntry() {
  const tools = document.querySelector("#panel-routes .rt-tools");
  if (!tools) return;
  const button = () => el("button", { class: "btn exp-run", type: "button", onclick: run }, "Run expansion");
  (tools.querySelector(".exp-run") ?? tools.appendChild(button())).hidden = !dock.hidden;
}

export function init(a) {
  app = a;
  map = app.map;
  dock = document.getElementById("panel-expansion");
  // `exp=1`: the link shows the expansion, it re-runs once the page is ready
  app.state.register("exp", { encode: (v) => (v ? "1" : undefined), decode: () => true, initial: null });

  app.on("route:loaded", renderEntry);
  app.on("route:failed", renderEntry);
  // A listener stops MapLibre's own logging of every error, so the others are logged as it would
  map.on("error", (e) => {
    if (e.sourceId !== SOURCE || e.error?.status !== SKIPPED_LEVEL) return console.error(e.error ?? e);
    // MapLibre asks for the parent of a failed tile on its next tile update, which only a 404 schedules
    map._update?.();
  });

  let timer = 0;
  app.state.on(["wp", "opts", "costing"], () => {
    if (dock.hidden) return;
    clearTimeout(timer);
    if (!canRun()) return clear();
    timer = setTimeout(run, RERUN_MS);
  });

  if (app.state.get("exp") && canRun()) app.on("ready", () => run());
}
