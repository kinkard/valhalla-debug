import * as state from "./state.js";
import * as menu from "./menu.js";
import * as request from "./request.js";

import * as waypoints from "./waypoints.js";
import * as params from "./params.js";
import * as routes from "./routes.js";
import * as speeds from "./speeds.js";
import * as profile from "./profile.js";

const DEFAULT_VIEW = { zoom: 11, lat: 55.668221, lon: 13.15792 };
const view = state.get("map") ?? DEFAULT_VIEW;

const map = new maplibregl.Map({
  container: "map",
  style: "https://tiles.openfreemap.org/styles/positron",
  center: [view.lon, view.lat],
  zoom: view.zoom,
  attributionControl: { compact: true },
});
map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "bottom-right");
map.addControl(new maplibregl.ScaleControl(), "bottom-left");
map.getCanvas().addEventListener("contextmenu", (e) => e.preventDefault());

map.on("moveend", () => {
  const c = map.getCenter();
  state.set({ map: { zoom: map.getZoom(), lat: c.lat, lon: c.lng } });
});
state.on("map", () => {
  const v = state.get("map");
  const c = map.getCenter();
  const moved = Math.abs(c.lat - v?.lat) > 1e-6 || Math.abs(c.lng - v?.lon) > 1e-6;
  if (moved || Math.abs(map.getZoom() - v?.zoom) > 0.01) map.jumpTo({ center: [v.lon, v.lat], zoom: v.zoom });
});

// Layer slots, bottom to top. A module adds its layers with `beforeId: app.slot(name)`.
const SLOTS = ["speeds", "routes", "highlight"];

const bus = new EventTarget();
let statusTimer = null;

/** Everything modules get in `init(app)`. */
const app = {
  map,
  state,
  menu,
  request,
  slot: (name) => `slot-${name}`,
  /** `kind` "info" fades after a while, "error" stays. */
  status(text, kind = "info") {
    const el = document.getElementById("status");
    el.textContent = text;
    el.dataset.kind = kind;
    clearTimeout(statusTimer);
    if (kind === "info" && text) statusTimer = setTimeout(() => (el.textContent = ""), 8000);
  },
  copy: (text, what) =>
    navigator.clipboard.writeText(text).then(
      () => app.status(`Copied ${what}`),
      (err) => app.status(`Failed to copy: ${err.message}`, "error"),
    ),
  /** App-wide events, e.g. "route:loaded" with `detail: { units, routes }`. */
  on: (type, fn) => bus.addEventListener(type, (e) => fn(e.detail)),
  emit: (type, detail) => bus.dispatchEvent(new CustomEvent(type, { detail })),
};
window.app = app; // handy in the devtools console

const fixed = (x) => x.toFixed(6);
menu.add({
  group: "copy",
  label: "Copy lat, lon",
  run: ({ lngLat }) => app.copy(`${fixed(lngLat.lat)},${fixed(lngLat.lng)}`, "lat, lon"),
});

const modules = { waypoints, speeds, params, profile, routes };

map.on("load", () => {
  for (const name of SLOTS) {
    // Invisible anchors that keep the modules' layers in order
    map.addLayer({ id: `slot-${name}`, type: "background", layout: { visibility: "none" } });
  }
  menu.init(map);
  // Routes last: its first request must not carry the other modules' URL keys in `opts`
  for (const [name, module] of Object.entries(modules)) {
    // Throws and async rejections alike
    (async () => module.init?.(app))().catch((err) => {
      console.error(`Failed to init ${name}:`, err);
      app.status(`Failed to init ${name}: ${err.message}`, "error");
    });
  }
  app.emit("ready");
});
