// Speed and elevation charts of every route (src/route.rs), docked at the bottom of the map.

import { cssVar, el } from "./dom.js";
import { routeColor } from "./routes.js";

const KM_PER_MI = 1.609344;
const FT_PER_M = 3.28084;
const PAD = { top: 8, right: 10, bottom: 18, left: 34 };
const EMPTY = { type: "FeatureCollection", features: [] };

let app;
let map;
let dock;
let els = {}; // `speed` and `elev` are charts: { height, caption, svg, note }
let route = null; // the last route:loaded, see `prepare`
let speed = null; // per route: { segments, max } or { error }
let elev = null; // per route: { samples, min, max } or { error }
let cursor = null; // distance under the crosshair, in route units

export function init(a) {
  app = a;
  map = app.map;
  app.state.register("prof", { initial: false, encode: (v) => (v ? "1" : undefined), decode: () => true });
  buildDock();
  map.addSource("profile-cursor", { type: "geojson", data: EMPTY });
  map.addLayer(
    {
      id: "profile-cursor",
      type: "circle",
      source: "profile-cursor",
      paint: {
        "circle-radius": ["case", ["get", "selected"], 6, 5],
        "circle-color": routeColor(),
        "circle-stroke-color": cssVar("--panel"),
        "circle-stroke-width": 2,
      },
    },
    app.slot("highlight"),
  );

  app.on("route:loaded", (detail) => {
    route = prepare(detail);
    speed = route.items.map((item, i) => speedProfile(item, detail.routes[i].speeds));
    elev = route.items.map((item, i) => elevationProfile(item, detail.routes[i].elevation));
    syncButton();
    render();
  });
  app.on("route:cleared", () => {
    route = null;
    speed = elev = null;
    render();
  });
  app.state.on("prof", () => {
    syncButton();
    render();
  });
  app.state.on("alt", render);

  // The routes fragment is re-rendered on every route, so listen on its panel
  document.getElementById("panel-routes").addEventListener("click", (e) => {
    if (!e.target.closest('[data-action="profile"]')) return;
    app.state.set({ prof: !isOpen() });
  });
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || !isOpen() || e.defaultPrevented || app.menu.isOpen()) return;
    const focus = document.activeElement;
    const idle = focus === document.body || focus === map.getCanvas();
    if (dock.contains(focus) || (idle && !document.querySelector(".maplibregl-popup"))) close();
  });
  render();
}

function isOpen() {
  return app.state.get("prof");
}

const button = () => document.querySelector('#panel-routes [data-action="profile"]');

function close() {
  app.state.set({ prof: false });
  if (dock.contains(document.activeElement)) button()?.focus();
}

function syncButton() {
  button()?.setAttribute("aria-pressed", String(isOpen()));
}

/** Per route: flat shape, cumulative distance per point (route units) and legs with their offsets. */
function prepare({ routes, units }) {
  const items = routes.map((r) => {
    const dist = [];
    const legs = [];
    let offset = 0;
    r.legs.forEach((legCoords, j) => {
      const along = [0];
      for (let k = 1; k < legCoords.length; k++) along.push(along[k - 1] + haversineKm(legCoords[k - 1], legCoords[k]));
      const shapeLen = along[along.length - 1] || 1;
      // Scaled to Valhalla's own leg length, so the chart ends where the routes table says
      const length = r.lengths[j];
      for (const a of along) dist.push(offset + (a / shapeLen) * length);
      legs.push({ offset, length });
      offset += length;
    });
    return { coords: r.coords, dist, legs, total: offset };
  });
  return { miles: units === "miles", items, total: Math.max(0, ...items.map((r) => r.total)) };
}

/** `{ segments, max }` or `{ error }`. `legs`: per leg the edges' `[to, km/h]`, `to` in route units. */
function speedProfile(item, legs) {
  const k = route.miles ? 1 / KM_PER_MI : 1;
  const segments = [];
  legs.forEach((leg, j) => {
    const at = item.legs[j];
    let from = 0;
    for (const [to, speed] of leg) {
      if (to > from) segments.push({ x0: at.offset + from, x1: at.offset + to, speed: speed * k });
      from = to;
    }
  });
  if (segments.length === 0) return { error: "Valhalla sent no edges" };
  // No spread: a long route has more edges than a call takes arguments
  return { segments, max: segments.reduce((m, s) => Math.max(m, s.speed), -Infinity) };
}

/** `{ samples, min, max }` or `{ error }`. `legs`: per leg the `[along, meters]` samples, -32768 without data. */
function elevationProfile(item, legs) {
  const k = route.miles ? FT_PER_M : 1;
  const samples = [];
  legs.forEach((leg, j) => {
    const at = item.legs[j];
    for (const [along, h] of leg) {
      samples.push({ x: at.offset + Math.min(along, at.length), v: h > -32767 ? h * k : null });
    }
  });
  const valid = samples.filter((s) => s.v != null);
  if (valid.length === 0 || valid.every((s) => s.v === 0)) return { error: "No elevation data" };
  // No Math.min(...): a coast to coast route has more samples than a call takes arguments
  const min = valid.reduce((m, s) => Math.min(m, s.v), Infinity);
  return { samples, min, max: valid.reduce((m, s) => Math.max(m, s.v), -Infinity) };
}

function buildDock() {
  dock = el("section", {
    id: "panel-profile",
    class: "dock",
    role: "dialog",
    "aria-label": "Speed and elevation along the routes",
    hidden: true,
  });
  const closeBtn = el(
    "button",
    { class: "btn btn-ghost btn-icon", type: "button", "aria-label": "Close the profile", title: "Close (Esc)" },
    "×",
  );
  closeBtn.addEventListener("click", close);
  const head = el("div", { class: "pf-head" }, el("h2", { class: "panel-title" }, "Route profile"), closeBtn);

  const hint = "Arrow keys move the crosshair, Enter zooms the map there.";
  const chart = (height, label) => ({
    height,
    caption: el("div", { class: "pf-caption" }),
    svg: el("svg", { class: "pf-chart", height, tabindex: 0, role: "img", "aria-label": `${label}. ${hint}` }),
    note: el("p", { class: "pf-note", hidden: true }),
  });
  const row = (c) => el("div", { class: "pf-row" }, c.caption, el("div", { class: "pf-plot" }, c.svg, c.note));
  els.speed = chart(132, "Speed along each route");
  els.elev = chart(104, "Elevation along each route");
  els.tip = el("div", { class: "pf-tip", hidden: true, "aria-hidden": "true" });
  els.body = el("div", { class: "pf-body" }, row(els.speed), row(els.elev), els.tip);
  dock.append(head, els.body);
  document.getElementById("map-wrap").append(dock);

  for (const { svg: s } of [els.speed, els.elev]) {
    s.addEventListener("pointermove", (e) => hoverAt(s, e.clientX));
    s.addEventListener("pointerleave", () => setCursor(null));
    s.addEventListener("click", (e) => {
      hoverAt(s, e.clientX);
      zoomToCursor();
    });
    s.addEventListener("keydown", (e) => onChartKey(s, e));
    s.addEventListener("blur", () => setCursor(null));
  }
  new ResizeObserver(() => render()).observe(els.body);
  // Sit above the expansion dock when it is visible; hiding it resizes it to zero
  const expansion = document.getElementById("panel-expansion");
  new ResizeObserver(() => {
    if (expansion.hidden || expansion.offsetHeight === 0) return (dock.style.bottom = "");
    const wrap = document.getElementById("map-wrap").getBoundingClientRect();
    dock.style.bottom = `${Math.round(wrap.bottom - expansion.getBoundingClientRect().top) + 8}px`;
  }).observe(expansion);
}

function render() {
  dock.hidden = !isOpen();
  if (!isOpen()) return clearCursor();
  els.speed.caption.textContent = `Speed, ${units().speed}`;
  els.elev.caption.textContent = `Elevation, ${units().elev}`;

  if (!route) {
    note(els.speed.note, "Right-click the map to set a start and a destination, the profile follows the route.");
    blank(els.speed.svg);
    blank(els.elev.svg);
    note(els.elev.note, "");
    return clearCursor();
  }

  const speedDomain = (ok) => [0, niceCeil(Math.max(...ok.map((s) => s.max)) * 1.05 || 10)];
  renderChart(els.speed, speed, speedDomain, stepPath, 4, `No speeds along these routes: ${speed[0].error}.`);
  const empty = "No elevation data for these routes: the Valhalla tiles have none here.";
  renderChart(els.elev, elev, elevationDomain, linePath, 3, empty);
  if (cursor != null) setCursor(cursor);
}

/** Draws `line(profile)` of every route without an error, or the `empty` note when all have one. */
function renderChart(chart, profiles, domain, line, yCount, empty) {
  const ok = profiles.filter((p) => !p.error);
  note(chart.note, ok.length ? "" : empty);
  if (ok.length === 0) return blank(chart.svg);
  const width = chart.svg.clientWidth || 600;
  const x = xScale(width);
  const y = scale(domain(ok), [chart.height - PAD.bottom, PAD.top]);
  const paths = drawOrder()
    .filter((i) => !profiles[i].error)
    .map((i) => path(line(profiles[i], x, y), i));
  draw(chart.svg, width, chart.height, x, y, paths, yCount);
}

function elevationDomain(ok) {
  let lo = Math.min(...ok.map((e) => e.min));
  let hi = Math.max(...ok.map((e) => e.max));
  const mid = (hi + lo) / 2;
  if (hi - lo < 10) [lo, hi] = [mid - 5, mid + 5];
  const pad = (hi - lo) * 0.08;
  return [lo - pad, hi + pad];
}

function blank(node) {
  node.replaceChildren();
  delete node.dataset.ready;
}

function note(node, text) {
  node.textContent = text;
  node.hidden = !text;
}

/** Selected route last, so it draws on top. */
function drawOrder() {
  const sel = selected();
  return route.items.map((_, i) => i).sort((a, b) => (a === sel) - (b === sel) || b - a);
}

function selected() {
  return Math.min(app.state.get("alt") ?? 0, route.items.length - 1);
}

function xScale(width) {
  return scale([0, route.total || 1], [PAD.left, Math.max(PAD.left + 10, width - PAD.right)]);
}

function scale([d0, d1], [r0, r1]) {
  const f = (v) => r0 + ((v - d0) / (d1 - d0 || 1)) * (r1 - r0);
  f.domain = [d0, d1];
  f.range = [r0, r1];
  f.invert = (p) => d0 + ((p - r0) / (r1 - r0 || 1)) * (d1 - d0);
  return f;
}

function path(d, i) {
  return `<path class="pf-line${i === selected() ? " pf-sel" : ""}" style="--c: var(--route-${i % 4})" d="${d}"/>`;
}

function stepPath({ segments }, x, y) {
  let d = "";
  let lastX = null;
  let lastY = null;
  for (const g of segments) {
    const px0 = x(g.x0).toFixed(1);
    const px1 = x(g.x1).toFixed(1);
    const py = y(g.speed).toFixed(1);
    if (lastX == null || Math.abs(lastX - px0) > 0.5) d += `M${px0} ${py}`;
    else if (lastY !== py) d += `V${py}`;
    d += `H${px1}`;
    lastX = px1;
    lastY = py;
  }
  return d;
}

function linePath({ samples }, x, y) {
  let d = "";
  let pen = false;
  for (const s of samples) {
    if (s.v == null) {
      pen = false;
      continue;
    }
    d += `${pen ? "L" : "M"}${x(s.x).toFixed(1)} ${y(s.v).toFixed(1)}`;
    pen = true;
  }
  return d;
}

function draw(node, width, height, x, y, paths, yCount) {
  node.setAttribute("viewBox", `0 0 ${width} ${height}`);
  const [r0, r1] = x.range;
  const grid = ticks(...y.domain, yCount)
    .map((v) => {
      const py = y(v).toFixed(1);
      return `<line class="pf-grid" x1="${r0}" x2="${r1}" y1="${py}" y2="${py}"/>
        <text class="pf-tick" x="${r0 - 6}" y="${py}" dy="0.32em" text-anchor="end">${fmt(v)}</text>`;
    })
    .join("");
  const xt = ticks(0, x.domain[1], Math.max(2, Math.floor((r1 - r0) / 90)));
  const xticks = xt
    .map((v, k) => {
      const label = k === xt.length - 1 ? `${fmtDist(v)} ${units().dist}` : fmtDist(v);
      const anchor = k === xt.length - 1 && x(v) > r1 - 30 ? "end" : "middle";
      return `<text class="pf-tick" x="${x(v).toFixed(1)}" y="${height - 4}" text-anchor="${anchor}">${label}</text>`;
    })
    .join("");
  const y0 = y.range[0];
  // Where the selected route reaches a via point
  const vias = route.items[selected()].legs.slice(1).map(({ offset }) => {
    const px = x(offset).toFixed(1);
    return `<line class="pf-via" x1="${px}" x2="${px}" y1="${PAD.top}" y2="${y0}"/>`;
  });
  node.innerHTML = `${grid}${vias.join("")}<line class="pf-axis" x1="${r0}" x2="${r1}" y1="${y0}" y2="${y0}"/>${xticks}
    ${paths.join("")}
    <line class="pf-cross" y1="${PAD.top}" y2="${y0}" x1="-10" x2="-10"/>`;
  node.dataset.ready = "1";
}

function hoverAt(node, clientX) {
  if (!route || !node.dataset.ready) return;
  const box = node.getBoundingClientRect();
  const x = xScale(node.clientWidth);
  const d = x.invert(clientX - box.left);
  setCursor(Math.max(0, Math.min(route.total, d)), node);
}

function onChartKey(node, e) {
  if (!route) return;
  const stepBy = (route.total / 100) * (e.shiftKey ? 10 : 1);
  let d = cursor ?? 0;
  if (e.key === "ArrowRight") d += stepBy;
  else if (e.key === "ArrowLeft") d -= stepBy;
  else if (e.key === "Home") d = 0;
  else if (e.key === "End") d = route.total;
  else if (e.key === "Enter") return zoomToCursor();
  else return;
  e.preventDefault();
  setCursor(Math.max(0, Math.min(route.total, d)), node);
}

let tipAnchor = null;

function setCursor(d, anchor = tipAnchor) {
  cursor = d;
  tipAnchor = anchor;
  if (d == null || !route) return clearCursor();
  const px = xScale(els.speed.svg.clientWidth || 600)(d).toFixed(1);
  moveCross(px);
  renderTip(d, +px, anchor ?? els.speed.svg);
  const sel = selected();
  const features = route.items
    .map((item, index) => (d <= item.total ? { index, at: pointAt(item, d) } : null))
    .filter((f) => f?.at)
    .sort((a, b) => (a.index === sel) - (b.index === sel))
    .map(({ index, at }) => ({
      type: "Feature",
      properties: { index, selected: index === sel },
      geometry: { type: "Point", coordinates: at },
    }));
  map.getSource("profile-cursor")?.setData({ type: "FeatureCollection", features });
}

function clearCursor() {
  cursor = null;
  els.tip.hidden = true;
  moveCross(-10);
  map.getSource("profile-cursor")?.setData(EMPTY);
}

function moveCross(px) {
  for (const { svg } of [els.speed, els.elev]) {
    const line = svg.querySelector(".pf-cross");
    line?.setAttribute("x1", px);
    line?.setAttribute("x2", px);
  }
}

function renderTip(d, px, anchor) {
  const u = units();
  const rows = route.items.map((item, i) => {
    const key = el("i", { class: "pf-key", "aria-hidden": "true" });
    const row = (...cells) => el("div", { class: "pf-tip-row", style: `--c: var(--route-${i % 4})` }, key, cells);
    if (d > item.total) return row(el("span", { class: "muted" }, "ended"));
    const g = speed[i].segments && segmentAt(speed[i].segments, d);
    const v = elev[i].samples && sampleAt(elev[i].samples, d);
    const height = v != null && el("span", {}, `${fmt(v)} ${u.elev}`);
    return row(el("strong", {}, g ? `${fmt(g.speed)} ${u.speed}` : "–"), height);
  });
  els.tip.replaceChildren(...rows);
  els.tip.hidden = false;
  const body = els.body.getBoundingClientRect();
  const box = anchor.getBoundingClientRect();
  const w = els.tip.offsetWidth;
  const left = box.left - body.left + px;
  const right = box.right - body.left;
  els.tip.style.left = `${Math.max(0, left + 12 + w > right ? left - 12 - w : left + 12)}px`;
  els.tip.style.top = `${box.top - body.top + PAD.top}px`;
}

function zoomToCursor() {
  if (cursor == null || !route) return;
  const item = route.items[selected()];
  const at = pointAt(item, Math.min(cursor, item.total));
  if (at) map.easeTo({ center: at, zoom: Math.max(map.getZoom(), 15) });
}

/** Index of the first of the sorted `items` whose `key` is at least `d`, the last one if none is. */
function search(items, key, d) {
  let lo = 0;
  let hi = items.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (key(items[mid]) < d) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

function segmentAt(segments, d) {
  const g = segments[search(segments, (s) => s.x1, d)];
  return g && d >= g.x0 - 1e-9 && d <= g.x1 + 1e-9 ? g : null;
}

function sampleAt(samples, d) {
  const lo = search(samples, (s) => s.x, d);
  const [a, b] = [samples[lo - 1], samples[lo]];
  if (!b || b.v == null) return null;
  if (!a || a.v == null || b.x === a.x) return b.v;
  return a.v + ((d - a.x) / (b.x - a.x)) * (b.v - a.v);
}

function pointAt({ dist, coords }, d) {
  if (coords.length === 0) return null;
  const lo = search(dist, (v) => v, d);
  if (lo === 0) return coords[0];
  const t = (d - dist[lo - 1]) / (dist[lo] - dist[lo - 1] || 1);
  const [a, b] = [coords[lo - 1], coords[lo]];
  return [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
}

function units() {
  return route?.miles ? { dist: "mi", speed: "mph", elev: "ft" } : { dist: "km", speed: "km/h", elev: "m" };
}

function fmt(v) {
  return (Math.round(v) || 0).toLocaleString("en-US").replace("-", "\u2212");
}

function fmtDist(v) {
  return Number.isInteger(v) ? String(v) : v.toFixed(1);
}

function niceStep(span, count) {
  const raw = span / Math.max(1, count);
  const p = 10 ** Math.floor(Math.log10(raw || 1));
  const f = raw / p;
  return (f < 1.5 ? 1 : f < 3 ? 2 : f < 7 ? 5 : 10) * p;
}

function niceCeil(v) {
  const step = niceStep(v, 4);
  return Math.ceil(v / step) * step;
}

function ticks(lo, hi, count) {
  const step = niceStep(hi - lo, count);
  const out = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-6; v += step) out.push(+v.toFixed(6));
  return out;
}

function haversineKm([lon1, lat1], [lon2, lat2]) {
  const rad = Math.PI / 180;
  const dLat = (lat2 - lat1) * rad;
  const dLon = (lon2 - lon1) * rad;
  const h = Math.sin(dLat / 2) ** 2 + Math.cos(lat1 * rad) * Math.cos(lat2 * rad) * Math.sin(dLon / 2) ** 2;
  return 12742 * Math.asin(Math.min(1, Math.sqrt(h)));
}
