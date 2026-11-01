// Locate cards from "Locate here" or `locate:edge`; bodies from GET /ui/locate (src/locate.rs).

import { round6 } from "./coords.js";
import { cssVar, el } from "./dom.js";
import { formatTime } from "./speeds.js";

const SOURCE = "locate";
const ARROW = "locate-arrow";
const BUCKETS_PER_DAY = 288;

let app;
let drawer;
let list;
let count;
/** Open cards, oldest first: `{ n, lat, lon, edge, el, marker }`. */
let cards = [];
let nextN = 1;
/** Card whose edge is emphasized because the pointer is over it. */
let hoverCard = null;

export function init(a) {
  app = a;

  // `loc=lat,lon;lat,lon@edgeId`: located points, oldest first, with the selected edge
  app.state.register("loc", {
    encode: (items) =>
      items.map(({ lat, lon, edge }) => `${round6(lat)},${round6(lon)}${edge ? `@${edge}` : ""}`).join(";"),
    decode: (s) =>
      s
        .split(";")
        .filter(Boolean)
        .map((item) => {
          const [coord, edge] = item.split("@");
          const [lat, lon] = coord.split(",").map(Number);
          if (Number.isNaN(lat) || Number.isNaN(lon)) throw new Error(`bad locate point ${item}`);
          return edge && /^\d+$/.test(edge) ? { lat, lon, edge } : { lat, lon };
        }),
    initial: [],
  });

  buildDrawer();
  addLayers();

  app.menu.add({
    group: "locate",
    label: "Locate here",
    run: ({ lngLat }) => addCard(lngLat.lat, lngLat.lng),
  });
  app.on("locate:edge", ({ edgeId, lngLat }) => addCard(lngLat.lat, lngLat.lng, String(edgeId)));

  restore();
  app.state.on("loc", () => {
    if (JSON.stringify(app.state.get("loc")) !== JSON.stringify(stateItems())) restore();
  });
}

function buildDrawer() {
  drawer = el("aside", { class: "loc-drawer", "aria-label": "Locate", hidden: true });
  drawer.innerHTML = `
    <header class="loc-drawer-head">
      <strong>Locate</strong>
      <span class="aside loc-count"></span>
      <button type="button" class="btn btn-ghost" data-act="close-all">Close all</button>
    </header>
    <div class="loc-list"></div>`;
  document.getElementById("map-wrap").append(drawer);
  list = drawer.querySelector(".loc-list");
  count = drawer.querySelector(".loc-count");

  // Clear of the expansion dock; hiding or showing it also resizes it
  const dock = document.getElementById("panel-expansion");
  new ResizeObserver(() =>
    drawer.style.setProperty("--loc-dock", dock.hidden ? "0px" : `${dock.offsetHeight + 12}px`),
  ).observe(dock);

  drawer.addEventListener("click", onClick);
  drawer.addEventListener("keydown", onKeydown);
  drawer.addEventListener("pointerover", onPointerOver);
  drawer.addEventListener("pointerleave", () => setHover(null));
  drawer.addEventListener("pointermove", onChartMove);
  drawer.addEventListener("pointerout", onChartOut);
  drawer.addEventListener("focusin", onChartFocus);
}

function updateDrawer() {
  drawer.hidden = cards.length === 0;
  count.textContent = cards.length === 1 ? "1 point" : `${cards.length} points`;
}

function stateItems() {
  return cards.map(({ lat, lon, edge }) => (edge ? { lat, lon, edge } : { lat, lon }));
}

function saveState() {
  app.state.set({ loc: stateItems() });
}

/** Rebuilds all cards from the URL state (page load, pasted link, back/forward). */
function restore() {
  for (const card of [...cards]) removeCard(card, false);
  nextN = 1;
  for (const { lat, lon, edge } of app.state.get("loc")) addCard(lat, lon, edge, false);
}

function addCard(lat, lon, edge, save = true) {
  const card = { n: nextN++, lat: round6(lat), lon: round6(lon), edge: edge || null };
  card.el = el("article", { class: "loc-card" });
  card.marker = new maplibregl.Marker({ element: markerElement(card), anchor: "center" })
    .setLngLat([card.lon, card.lat])
    .addTo(app.map);
  cards.push(card);
  list.prepend(card.el);
  list.scrollTop = 0;
  updateDrawer();
  load(card);
  if (save) saveState();
}

function markerElement(card) {
  const button = el("button", { type: "button", class: "loc-marker", title: `Locate card ${card.n}` }, String(card.n));
  button.addEventListener("click", (e) => {
    e.stopPropagation();
    card.el.scrollIntoView({ block: "nearest", behavior: "smooth" });
    card.el.classList.remove("is-flash");
    void card.el.offsetWidth; // restart the animation
    card.el.classList.add("is-flash");
  });
  button.addEventListener("pointerenter", () => setHover(card));
  button.addEventListener("pointerleave", () => setHover(null));
  return button;
}

function removeCard(card, save = true) {
  card.request?.abort();
  card.marker.remove();
  card.el.remove();
  cards = cards.filter((c) => c !== card);
  if (hoverCard === card) hoverCard = null;
  if (cards.length === 0) nextN = 1;
  updateDrawer();
  updateMap();
  if (save) saveState();
}

/** Locate request from the page's costing and options; the server adds `verbose` and the point. */
async function load(card) {
  card.request?.abort();
  const request = (card.request = new AbortController());
  // The server locates the card's point instead of the waypoints
  const spec = { ...app.request.spec(), locations: [] };
  const params = new URLSearchParams({ lat: card.lat, lon: card.lon, spec: JSON.stringify(spec) });
  if (card.edge) params.set("edge", card.edge);

  card.el.replaceChildren(headerElement(card, `${spec.costing} · locating edges…`));
  let body;
  try {
    // Errors come with a card body too
    body = await (await fetch(`/ui/locate?${params}`, { signal: request.signal })).text();
  } catch {
    if (request.signal.aborted) return;
    const error = el(
      "div",
      { class: "loc-error", role: "alert" },
      el("p", {}, el("strong", {}, "Failed to reach the valhalla-debug server.")),
      el("p", { class: "muted" }, "Check that it is still running, then locate again."),
    );
    return card.el.replaceChildren(headerElement(card, ""), error);
  }
  card.el.replaceChildren(headerElement(card, spec.costing));
  card.el.insertAdjacentHTML("beforeend", body);
  const n = card.el.querySelectorAll(".loc-tab").length;
  if (n) card.el.querySelector(".loc-where .muted").textContent += ` · ${n === 1 ? "1 edge" : `${n} edges`}`;
  const panel = card.el.querySelector(".loc-panel:not([hidden])");
  if (panel?.dataset.edge && card.edge !== panel.dataset.edge) {
    card.edge = panel.dataset.edge;
    saveState();
  }
  updateMap();
}

/** `meta`: the costing and edge count. */
function headerElement(card, meta) {
  const header = el("header", { class: "loc-head" });
  header.innerHTML = `
    <span class="loc-num">${card.n}</span>
    <div class="loc-where"><span class="mono">${card.lat.toFixed(6)}, ${card.lon.toFixed(6)}</span><span class="muted"></span></div>
    <button class="btn btn-ghost btn-icon" type="button" data-act="rerun" title="Locate again with the current options" aria-label="Locate again">
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13 8a5 5 0 1 1-1.5-3.6M13 2.5v2.5h-2.5"/></svg>
    </button>
    <button class="btn btn-ghost btn-icon" type="button" data-act="zoom" title="Zoom to this point" aria-label="Zoom to this point">
      <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="4.5"/><path d="M8 1v3M8 12v3M1 8h3M12 8h3"/></svg>
    </button>
    <button class="btn btn-ghost btn-icon" type="button" data-act="close" title="Close" aria-label="Close">
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8"/></svg>
    </button>`;
  header.querySelector(".muted").textContent = meta;
  return header;
}

function select(card, i) {
  for (const tab of card.el.querySelectorAll(".loc-tab"))
    tab.setAttribute("aria-selected", String(tab.dataset.i === String(i)));
  for (const panel of card.el.querySelectorAll(".loc-panel")) panel.hidden = panel.dataset.i !== String(i);
  const panel = card.el.querySelector(`.loc-panel[data-i="${i}"]`);
  card.edge = panel?.dataset.edge || null;
  saveState();
  updateMap();
}

function cardOf(el) {
  const article = el.closest(".loc-card");
  return cards.find((c) => c.el === article);
}

function onClick(e) {
  const tab = e.target.closest(".loc-tab");
  if (tab) return select(cardOf(tab), tab.dataset.i);

  const day = e.target.closest(".loc-days button");
  if (day) {
    const week = day.closest(".loc-predicted").querySelector("svg.loc-week");
    const d = +day.dataset.day;
    if (week._pinned == null) return showDay(week, d);
    // Same time of day on the picked day
    week._pinned = d * BUCKETS_PER_DAY + (week._pinned % BUCKETS_PER_DAY);
    return showAll(week, week._pinned);
  }

  const svg = e.target.closest("svg.loc-week");
  if (svg) {
    const bucket = bucketAt(svg, e);
    svg._pinned = svg._pinned === bucket ? null : bucket;
    return showAll(svg, bucket);
  }

  const action = e.target.closest("[data-act]");
  if (!action) return;
  const card = cardOf(action);
  switch (action.dataset.act) {
    case "close-all":
      for (const c of [...cards]) removeCard(c, false);
      return saveState();
    case "close":
      return card && removeCard(card);
    case "rerun":
      return card && load(card);
    case "zoom":
      return card && app.map.flyTo({ center: [card.lon, card.lat], zoom: Math.max(app.map.getZoom(), 17) });
    case "lowest": {
      const week = action.closest(".loc-predicted").querySelector("svg.loc-week");
      week._pinned = +week.dataset.min;
      return showAll(week, week._pinned);
    }
    case "copy-json":
      return app.copy(action.closest(".loc-panel").querySelector(".loc-raw pre").textContent, "edge JSON");
  }
}

function onKeydown(e) {
  const tab = e.target.closest(".loc-tab");
  if (tab && (e.key === "ArrowRight" || e.key === "ArrowLeft")) {
    const tabs = [...tab.parentElement.querySelectorAll(".loc-tab")];
    const next = tabs[(tabs.indexOf(tab) + (e.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length];
    next.focus();
    select(cardOf(next), next.dataset.i);
    return e.preventDefault();
  }
  const svg = e.target.closest("svg.loc-week");
  if (svg) onChartKey(svg, e);
}

function onPointerOver(e) {
  setHover(cardOf(e.target) ?? null);
}

function setHover(card) {
  if (hoverCard === card) return;
  hoverCard = card;
  for (const c of cards) c.marker.getElement().classList.toggle("is-hover", c === card);
  updateMap();
}

function speedsOf(svg) {
  return (svg._speeds ??= svg.dataset.speeds.split(",").map(Number));
}

function bucketAt(svg, e) {
  const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(svg.getScreenCTM().inverse());
  const { x0, w, day } = svg.dataset;
  const t = Math.min(BUCKETS_PER_DAY - 1, Math.max(0, Math.floor(((p.x - x0) / w) * BUCKETS_PER_DAY)));
  return Math.min(speedsOf(svg).length - 1, day * BUCKETS_PER_DAY + t);
}

/** Shows day `d` of the week on `svg` and presses its button. */
function showDay(svg, d) {
  svg.dataset.day = d;
  for (const g of svg.querySelectorAll("g.day")) g.classList.toggle("is-shown", +g.dataset.day === d);
  for (const b of svg.closest(".loc-predicted").querySelectorAll(".loc-days button")) {
    b.setAttribute("aria-pressed", String(+b.dataset.day === d));
  }
}

function showBucket(svg, bucket) {
  const speeds = speedsOf(svg);
  const readout = svg.closest(".loc-predicted").querySelector(".loc-readout");
  const cursor = svg.querySelector(".cursor");
  const dot = svg.querySelector(".cursor-dot");
  for (const el of [cursor, dot]) {
    el.setAttribute("visibility", bucket == null ? "hidden" : "visible");
    el.classList.toggle("is-pinned", bucket != null && svg._pinned === bucket);
  }
  if (bucket == null) return (readout.textContent = "Hover or use arrow keys to read a 5-minute bucket");
  showDay(svg, Math.floor(bucket / BUCKETS_PER_DAY));
  const { x0, w, top, row, lo, hi } = svg.dataset;
  const x = +x0 + ((bucket % BUCKETS_PER_DAY) * w) / BUCKETS_PER_DAY;
  cursor.setAttribute("x", x);
  dot.setAttribute("cx", x + w / BUCKETS_PER_DAY / 2);
  dot.setAttribute("cy", +top + +row - ((speeds[bucket] - lo) / (hi - lo)) * row);
  const min = speeds[+svg.dataset.min];
  const delta = speeds[bucket] - min;
  readout.textContent =
    `${formatTime(bucket * 300)} · ${speeds[bucket]} km/h · bucket ${bucket}` +
    (delta ? ` · +${delta} over the lowest` : " · the lowest") +
    (svg._pinned === bucket ? " · pinned" : "");
}

/** Shows `bucket` on `svg` and the same bucket on every other visible chart, to compare cards. */
function showAll(svg, bucket) {
  showBucket(svg, bucket);
  for (const other of drawer.querySelectorAll(".loc-panel:not([hidden]) svg.loc-week")) {
    if (other !== svg) showBucket(other, bucket ?? other._pinned ?? null);
  }
}

function onChartMove(e) {
  const svg = e.target.closest?.("svg.loc-week");
  if (svg) showAll(svg, bucketAt(svg, e));
}

function onChartOut(e) {
  const svg = e.target.closest?.("svg.loc-week");
  if (svg && !svg.contains(e.relatedTarget)) showAll(svg, svg._pinned ?? null);
}

function onChartFocus(e) {
  const svg = e.target.closest?.("svg.loc-week");
  if (svg && svg._pinned == null) {
    svg._pinned = +svg.dataset.min;
    showBucket(svg, svg._pinned);
  }
}

function onChartKey(svg, e) {
  const steps = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -BUCKETS_PER_DAY, ArrowDown: BUCKETS_PER_DAY };
  const total = speedsOf(svg).length;
  let bucket = svg._pinned ?? +svg.dataset.min;
  if (e.key in steps) bucket += steps[e.key] * (e.shiftKey && Math.abs(steps[e.key]) === 1 ? 12 : 1);
  else if (e.key === "Home") bucket = +svg.dataset.min;
  else return;
  e.preventDefault();
  svg._pinned = (bucket + total) % total;
  showAll(svg, svg._pinned);
}

function addLayers() {
  const map = app.map;
  map.addSource(SOURCE, { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addImage(ARROW, arrowImage(), { pixelRatio: 2 });
  const add = (layer) => map.addLayer({ source: SOURCE, ...layer }, app.slot("highlight"));
  const kind = (k) => ["==", ["get", "kind"], k];
  const round = { "line-cap": "round", "line-join": "round" };
  // Wider when hovered; zoom must be the outermost expression
  const width = (on, off) => {
    const at = (k) => ["case", ["get", "hover"], on * k, off * k];
    return ["interpolate", ["linear"], ["zoom"], 12, at(0.6), 17, at(1)];
  };

  add({
    id: "locate-casing",
    type: "line",
    filter: kind("edge"),
    layout: round,
    paint: { "line-color": cssVar("--ink"), "line-width": width(15, 11), "line-opacity": 0.9 },
  });
  add({
    id: "locate-edge",
    type: "line",
    filter: kind("edge"),
    layout: round,
    paint: { "line-color": cssVar("--warn"), "line-width": width(10, 7) },
  });
  add({
    id: "locate-arrows",
    type: "symbol",
    filter: kind("edge"),
    layout: {
      "symbol-placement": "line",
      "symbol-spacing": 48,
      "icon-image": ARROW,
      "icon-size": ["interpolate", ["linear"], ["zoom"], 12, 0.6, 17, 1],
      "icon-allow-overlap": true,
      "icon-ignore-placement": true,
      "icon-rotation-alignment": "map",
    },
  });
  add({
    id: "locate-snap",
    type: "line",
    filter: kind("snap"),
    paint: { "line-color": cssVar("--ink"), "line-width": 1.5, "line-dasharray": [2, 2] },
  });
  add({
    id: "locate-snapped",
    type: "circle",
    filter: kind("snapped"),
    paint: {
      "circle-radius": 4,
      "circle-color": cssVar("--warn"),
      "circle-stroke-color": cssVar("--ink"),
      "circle-stroke-width": 1.5,
    },
  });
}

/** Chevron pointing along the line, drawn on top of the highlighted edge. */
function arrowImage() {
  const ctx = new OffscreenCanvas(24, 24).getContext("2d");
  Object.assign(ctx, { strokeStyle: "#1e242c", lineWidth: 3.5, lineCap: "round", lineJoin: "round" });
  ctx.stroke(new Path2D("M9 6L15 12L9 18"));
  return ctx.getImageData(0, 0, 24, 24);
}

function updateMap() {
  const feature = (properties, type, coordinates) => ({ type: "Feature", properties, geometry: { type, coordinates } });
  const features = [];
  for (const card of cards) {
    const panel = card.el.querySelector(".loc-panel:not([hidden])");
    if (!panel) continue;
    const coords = (panel._geom ??= JSON.parse(panel.dataset.geom || "[]"));
    if (coords.length > 1) features.push(feature({ kind: "edge", hover: hoverCard === card }, "LineString", coords));
    if (panel.dataset.snap) {
      const snap = JSON.parse(panel.dataset.snap);
      features.push(feature({ kind: "snap" }, "LineString", [[card.lon, card.lat], snap]));
      features.push(feature({ kind: "snapped" }, "Point", snap));
    }
  }
  app.map.getSource(SOURCE)?.setData({ type: "FeatureCollection", features });
}
