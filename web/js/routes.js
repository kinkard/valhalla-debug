// Routes panel from POST /ui/route (src/route.rs). `fetch`, not `htmx.ajax`: aborting it is quiet.

import { cssVar, el } from "./dom.js";
import { APP_DEFAULTS } from "./request.js";

const DEBOUNCE_MS = 250;
const EMPTY = { type: "FeatureCollection", features: [] };
const TRIGGERS = ["wp", "costing", "opts"];

let app;
let map;
let panel;
let timer = null;
let controller = null; // AbortController of the request in flight
let current = null; // { routes: [{ legs: [[lon, lat]...], coords, lengths, elevation, speeds }] }
let stale = false;
let underExpansion = false;
let hovered = null;
let maneuversOpen = false;
let fitNext = false;
// A shared link's view wins over the first route
let keepView = false;
let popup;

export function init(a) {
  app = a;
  map = app.map;
  panel = document.getElementById("panel-routes");
  fitNext = app.state.get("map") == null;
  keepView = !fitNext;
  setupMap();
  app.on("expansion:loaded", () => ((underExpansion = true), applyLineStyle()));
  app.on("expansion:cleared", () => ((underExpansion = false), applyLineStyle()));
  setupPanel();
  app.state.on(TRIGGERS, schedule);
  app.state.on("alt", () => select(app.state.get("alt")));
  run();
}

function schedule() {
  clearTimeout(timer);
  timer = setTimeout(run, DEBOUNCE_MS);
}

function run() {
  clearTimeout(timer);
  const count = app.state.get("wp").length;
  if (count === 0) return clear();
  if (count === 1) return waitForDestination();

  const spec = app.request.spec();
  abort();
  const ctl = (controller = new AbortController());
  setAside("Routing…");
  panel.classList.add("rt-loading");
  const headers = { "content-type": "application/json" };
  fetch("/ui/route", { method: "POST", headers, body: JSON.stringify(spec), signal: ctl.signal })
    .then(async (res) => {
      if (!res.ok) throw Object.assign(new Error(`The debug server answered: ${await res.text()}`), { server: true });
      return res.text();
    })
    .then(
      (html) => {
        if (ctl !== controller) return;
        done();
        htmx.swap(panel, html, { swapStyle: "innerHTML" });
        onSwap();
      },
      (err) => {
        if (ctl.signal.aborted) return;
        done();
        const message = err instanceof TypeError ? "Can't reach the debug server." : err.message;
        const hint = err.server
          ? "Change the request to retry."
          : "Check that valhalla-debug is still running, then change anything to retry.";
        showLocalError(message, hint);
      },
    );
}

function done() {
  controller = null;
  panel.classList.remove("rt-loading");
}

function abort() {
  controller?.abort();
  done();
}

/** No waypoints left: the only case that drops the drawn route. */
function clear() {
  abort();
  const had = current != null;
  current = null;
  setStale(false);
  setHover(null);
  map.getSource("routes").setData(EMPTY);
  map.getSource("routes-maneuver").setData(EMPTY);
  showEmpty("Right-click the map to set a start and a destination.");
  if (had) app.emit("route:cleared");
}

/** One waypoint: nothing to route, but a removed destination shouldn't wipe the last route. */
function waitForDestination() {
  abort();
  showEmpty("Right-click the map to set the destination.");
  if (current) {
    setStale(true);
    panel.querySelector(".rt-stale-note").hidden = false;
  }
}

function showEmpty(text) {
  panel.innerHTML = `<h2 class="panel-title">Routes</h2><p class="rt-empty"></p>
    <p class="rt-stale-note muted" hidden>The map shows the last route, dimmed.</p>`;
  panel.querySelector(".rt-empty").textContent = text;
}

function showLocalError(message, hint) {
  panel.innerHTML = `<h2 class="panel-title">Routes <span class="aside">Not sent</span></h2>
    <div class="rt-error" role="alert"><p class="rt-error-message"></p><p></p>
    <p class="rt-stale-note muted" hidden>The map shows the last route that worked, dimmed.</p></div>`;
  const [msg, hintEl] = panel.querySelectorAll(".rt-error p");
  msg.textContent = message;
  hintEl.textContent = hint;
  markStale(message);
}

function markStale(message) {
  setStale(current != null);
  const note = panel.querySelector(".rt-stale-note");
  if (note) note.hidden = current == null;
  app.status(message, "error");
}

function setAside(text) {
  const title = panel.querySelector(".panel-title");
  if (title) (title.querySelector(".aside") ?? title.appendChild(el("span", { class: "aside" }))).textContent = text;
}

/** Draws the routes from the fragment's `.rt-meta` JSON. */
function onSwap() {
  const metaEl = panel.querySelector(".rt-meta");
  if (!metaEl) {
    markStale(`Route failed: ${panel.querySelector(".rt-error-message")?.textContent}`);
    return app.emit("route:failed");
  }
  const meta = JSON.parse(metaEl.textContent);

  const routes = meta.routes.map((route) => {
    const legs = route.legs.map((leg) => decodePolyline(leg.shape));
    return {
      legs,
      coords: legs.flat(),
      lengths: route.legs.map((leg) => leg.summary.length),
      elevation: route.legs.map((leg) => leg.elevation),
      speeds: route.legs.map((leg) => leg.speeds),
    };
  });
  current = { routes };
  setStale(false);
  for (const d of panel.querySelectorAll("details.rt-maneuvers")) d.open = maneuversOpen;

  const alt = Math.min(app.state.get("alt") ?? 0, routes.length - 1);
  if (alt !== app.state.get("alt")) app.state.set({ alt });
  select(alt);
  fitIfOffscreen(routes);
  app.status(`${routes.length === 1 ? "1 route" : `${routes.length} routes`} ·\u2009 ${meta.ms} ms`);
  app.emit("route:loaded", { units: meta.units, routes });
}

/** Color of the feature's route `index`: `--route-0..3`, repeating. */
export function routeColor() {
  const c = (i) => cssVar(`--route-${i}`);
  return ["match", ["%", ["get", "index"], 4], 0, c(0), 1, c(1), 2, c(2), c(3)];
}

const hoverState = ["boolean", ["feature-state", "hover"], false];
const LINE_OPACITY = ["case", ["get", "selected"], 1, hoverState, 1, 0.85];

function setupMap() {
  map.addSource("routes", { type: "geojson", data: EMPTY, promoteId: "index" });
  map.addSource("routes-maneuver", { type: "geojson", data: EMPTY });
  const layout = { "line-join": "round", "line-cap": "round" };
  const sorted = { ...layout, "line-sort-key": ["case", ["get", "selected"], 100, ["-", 0, ["get", "index"]]] };
  const add = (spec, slot = "routes") =>
    map.addLayer({ type: "line", source: "routes", layout, ...spec }, app.slot(slot));
  add({
    id: "routes-casing",
    layout: sorted,
    // Every route gets a casing so it stays legible over the speeds layer
    paint: { "line-color": cssVar("--panel"), "line-width": ["case", ["get", "selected"], 12, 10] },
  });
  add({
    id: "routes-line",
    layout: sorted,
    paint: {
      "line-color": routeColor(),
      "line-width": ["case", ["get", "selected"], 6, hoverState, 6, 5],
      "line-opacity": LINE_OPACITY,
    },
  });
  add({ id: "routes-hit", paint: { "line-color": "#000", "line-opacity": 0, "line-width": 18 } });
  const maneuver = { "line-color": cssVar("--ink"), "line-width": 8, "line-opacity": 0.85 };
  add({ id: "routes-maneuver", source: "routes-maneuver", paint: maneuver }, "highlight");

  popup = new maplibregl.Popup({ closeButton: false, closeOnClick: false, className: "rt-popup", offset: 14 });
  map.on("mousemove", "routes-hit", (e) => {
    map.getCanvas().style.cursor = "pointer";
    const indexes = routesAt(e.features);
    const sel = app.state.get("alt");
    setHover(indexes.includes(sel) ? sel : indexes[0], e.lngLat);
  });
  map.on("mouseleave", "routes-hit", () => {
    map.getCanvas().style.cursor = "";
    setHover(null);
  });
  map.on("click", "routes-hit", (e) => {
    const indexes = routesAt(e.features);
    if (indexes.length === 0) return;
    // Where routes overlap, each click selects the next one there
    const i = indexes.indexOf(app.state.get("alt"));
    app.state.set({ alt: indexes[(i + 1) % indexes.length] });
  });
}

function routesAt(features) {
  const set = new Set((features ?? []).map((f) => f.properties.index));
  return [...set].sort((a, b) => a - b);
}

function draw(sel) {
  const features = current.routes.map(({ coords }, index) => ({
    type: "Feature",
    id: index,
    properties: { index, selected: index === sel },
    geometry: { type: "LineString", coordinates: coords },
  }));
  map.getSource("routes").setData({ type: "FeatureCollection", features });
}

function setStale(on) {
  stale = on;
  applyLineStyle();
}

// See-through while an expansion is shown, so the search under the routes stays visible
const EXPANSION_OPACITY = ["case", ["get", "selected"], 0.45, hoverState, 0.6, 0.3];

function applyLineStyle() {
  if (!map.getLayer("routes-line")) return;
  const opacity = stale ? 0.35 : underExpansion ? EXPANSION_OPACITY : LINE_OPACITY;
  map.setPaintProperty("routes-line", "line-opacity", opacity);
  map.setPaintProperty("routes-line", "line-dasharray", stale ? [1.5, 1.5] : undefined);
  map.setLayoutProperty("routes-casing", "visibility", stale || underExpansion ? "none" : "visible");
}

/** Fits the view only when it shows little of the routes, so rerouting never yanks a zoomed-in view. */
function fitIfOffscreen(routes) {
  const coords = routes.flatMap((r) => r.coords);
  if (coords.length === 0) return;
  if (keepView) return (keepView = false);
  // West, south, east, north
  const box = bounds(coords).toArray().flat();
  const view = map.getBounds().toArray().flat();
  const area = (r) => Math.max(0, r[2] - r[0]) * Math.max(0, r[3] - r[1]);
  const overlap = area(box.map((v, k) => (k < 2 ? Math.max : Math.min)(v, view[k])));
  const visible = overlap / (Math.min(area(box), area(view)) || 1e-12);
  if (fitNext || visible < 0.25) map.fitBounds(box, { padding: 48, maxZoom: 16, duration: fitNext ? 0 : 600 });
  fitNext = false;
}

function select(index) {
  if (!current) return;
  const sel = Math.max(0, Math.min(index ?? 0, current.routes.length - 1));
  for (const row of panel.querySelectorAll(".rt-row"))
    row.setAttribute("aria-pressed", String(+row.dataset.index === sel));
  for (const d of panel.querySelectorAll(".rt-detail")) d.hidden = +d.dataset.index !== sel;
  map.getSource("routes-maneuver").setData(EMPTY);
  draw(sel);
}

function setHover(index, lngLat) {
  const next = index ?? null;
  if (next !== hovered) {
    if (hovered != null) map.setFeatureState({ source: "routes", id: hovered }, { hover: false });
    if (next != null) map.setFeatureState({ source: "routes", id: next }, { hover: true });
    for (const row of panel.querySelectorAll(".rt-row")) row.classList.toggle("is-hover", +row.dataset.index === next);
    hovered = next;
  }
  const row = next == null ? null : panel.querySelector(`.rt-row[data-index="${next}"]`);
  if (!lngLat || !row) return popup.remove();
  const note = stale && el("div", {}, "Out of date: the last request failed");
  const box = el("div", {}, el("strong", {}, row.dataset.name), el("div", {}, row.dataset.tip), note);
  popup.setLngLat(lngLat).setDOMContent(box).addTo(map);
}

function maneuverCoords(el) {
  const section = el.closest(".rt-detail");
  const route = current?.routes[+section?.dataset.index];
  const leg = route?.legs[+el.dataset.leg];
  return leg ? leg.slice(+el.dataset.begin, +el.dataset.end + 1) : [];
}

function highlightManeuver(el) {
  const coords = el ? maneuverCoords(el) : [];
  const geometry = coords.length > 1 ? { type: "LineString", coordinates: coords } : null;
  map.getSource("routes-maneuver").setData(geometry ? { type: "Feature", properties: {}, geometry } : EMPTY);
}

function zoomToManeuver(el) {
  const coords = maneuverCoords(el);
  if (coords.length === 0) return;
  map.fitBounds(bounds(coords), { padding: 80, maxZoom: 17 });
}

const bounds = (coords) => coords.reduce((b, c) => b.extend(c), new maplibregl.LngLatBounds());

function setupPanel() {
  panel.addEventListener(
    "toggle",
    (e) => {
      if (e.target.matches?.("details.rt-maneuvers")) maneuversOpen = e.target.open;
    },
    true,
  );

  panel.addEventListener("click", (e) => {
    const row = e.target.closest(".rt-row");
    if (row) return app.state.set({ alt: +row.dataset.index });
    const step = e.target.closest("[data-alternates]");
    if (step) {
      const opts = { ...app.state.get("opts"), alternates: +step.dataset.alternates };
      if (opts.alternates === APP_DEFAULTS.alternates) delete opts.alternates;
      return app.state.set({ opts });
    }
    const man = e.target.closest(".rt-man");
    if (man) zoomToManeuver(man);
  });
  panel.addEventListener("keydown", (e) => {
    const man = e.target.closest?.(".rt-man");
    if (man && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      zoomToManeuver(man);
    }
  });

  panel.addEventListener("mouseover", (e) => {
    const row = e.target.closest(".rt-row");
    setHover(row ? +row.dataset.index : null);
    highlightManeuver(e.target.closest(".rt-man"));
  });
  panel.addEventListener("mouseleave", () => {
    setHover(null);
    highlightManeuver(null);
  });
  panel.addEventListener("focusin", (e) => highlightManeuver(e.target.closest(".rt-man")));
}

/** Decodes Valhalla's encoded polyline6 into [lon, lat] pairs. */
function decodePolyline(str) {
  const out = [];
  let i = 0;
  let lat = 0;
  let lon = 0;
  const next = () => {
    let result = 0;
    let shift = 0;
    let b;
    do {
      b = str.charCodeAt(i++) - 63;
      result |= (b & 0x1f) << shift;
      shift += 5;
    } while (b >= 0x20);
    return result & 1 ? ~(result >> 1) : result >> 1;
  };
  while (i < str.length) {
    lat += next();
    lon += next();
    out.push([lon / 1e6, lat / 1e6]);
  }
  return out;
}
