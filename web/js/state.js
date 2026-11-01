// App state mirrored into the URL hash, see "URL format" in README.md.

import { decodeWaypoints, encodeWaypoints, round6, typed, untyped } from "./coords.js";

const codecs = new Map();
const listeners = [];
let values = {};
let writeTimer = null;

/** `encode` returning undefined or "" leaves the key out of the URL; `initial` is its value when absent. */
export function register(key, { encode = String, decode = (s) => s, initial } = {}) {
  codecs.set(key, { encode, decode, initial });
  // The URL was parsed before modules registered their keys, so this one may have landed in opts
  if (values.opts && key in values.opts) {
    const { [key]: _, ...rest } = values.opts;
    values.opts = rest;
  }
  if (!(key in values)) {
    const raw = readHash().get(key);
    values[key] = raw === undefined ? structuredClone(initial) : safeDecode(key, raw);
  }
}

export function get(key) {
  return values[key];
}

/** Shallow-merges `patch` into the state, updates the URL and notifies subscribers. */
export function set(patch) {
  const changed = [];
  for (const [key, value] of Object.entries(patch)) {
    if (key !== "opts" && !codecs.has(key)) throw new Error(`Unknown state key '${key}', register it first`);
    if (!sameValue(values[key], value)) {
      values[key] = value;
      changed.push(key);
    }
  }
  if (changed.length === 0) return;
  scheduleWrite();
  notify(changed);
}

/** Calls `fn(changedKeys)` whenever any of `keys` changes. */
export function on(keys, fn) {
  listeners.push({ keys: new Set([].concat(keys)), fn });
}

function notify(changed) {
  for (const { keys, fn } of listeners) {
    if (changed.some((k) => keys.has(k))) {
      try {
        fn(changed);
      } catch (err) {
        console.error(err);
      }
    }
  }
}

function sameValue(a, b) {
  return a === b || JSON.stringify(a) === JSON.stringify(b);
}

function safeDecode(key, raw) {
  try {
    return codecs.get(key).decode(raw);
  } catch (err) {
    console.warn(`Ignoring bad '${key}' in the URL:`, err);
    return structuredClone(codecs.get(key).initial);
  }
}

// A `+` stays a plus, as `encodeValue` never writes one for a space
const readHash = () => new Map(new URLSearchParams(location.hash.slice(1).replaceAll("+", "%2B")));

// Keeps `, ; / : ! @ =` readable, which covers coordinates, waypoint lists and timestamps.
function encodeValue(v) {
  return encodeURIComponent(v).replace(/%(2C|3B|2F|3A|40|3D)/g, (m) => decodeURIComponent(m));
}

function hashFromState() {
  const parts = [];
  for (const [key, { encode }] of codecs) {
    const encoded = encode(values[key]);
    if (encoded !== undefined && encoded !== "") parts.push(`${key}=${encodeValue(encoded)}`);
  }
  for (const [path, value] of Object.entries(values.opts ?? {})) {
    parts.push(`${encodeValue(path)}=${encodeValue(untyped(value))}`);
  }
  return "#" + parts.join("&");
}

function scheduleWrite() {
  clearTimeout(writeTimer);
  writeTimer = setTimeout(() => {
    const hash = hashFromState();
    if (location.hash !== hash) history.replaceState(null, "", location.pathname + location.search + hash);
  }, 150);
}

// Someone edited or pasted the URL: re-read everything and notify.
window.addEventListener("hashchange", () => {
  if (location.hash === hashFromState()) return;
  const hash = readHash();
  const changed = [];
  for (const [key, { initial }] of codecs) {
    const next = hash.has(key) ? safeDecode(key, hash.get(key)) : structuredClone(initial);
    if (!sameValue(values[key], next)) {
      values[key] = next;
      changed.push(key);
    }
  }
  const opts = optsFromHash(hash);
  if (!sameValue(values.opts, opts)) {
    values.opts = opts;
    changed.push("opts");
  }
  if (changed.length) notify(changed);
});

function optsFromHash(hash) {
  const opts = {};
  for (const [key, value] of hash) if (!codecs.has(key)) opts[key] = typed(value);
  return opts;
}

/** Map view `zoom/lat/lon`, as in older links. */
register("map", {
  encode: (v) => (v ? `${Math.round(v.zoom * 100) / 100}/${round6(v.lat)}/${round6(v.lon)}` : undefined),
  decode: (s) => {
    const [zoom, lat, lon] = s.split("/").map(Number);
    if ([zoom, lat, lon].some(Number.isNaN)) throw new Error(`bad map view ${s}`);
    return { zoom, lat, lon };
  },
  initial: null,
});

/** Waypoints, see `encodeWaypoints` in coords.js. */
register("wp", { encode: encodeWaypoints, decode: decodeWaypoints, initial: [] });

register("costing", { initial: "auto" });

/** Index of the selected route: 0 is the primary one, 1.. are alternates. */
register("alt", {
  initial: 0,
  encode: (v) => (v ? String(v) : undefined),
  decode: (s) => Math.max(0, parseInt(s) || 0),
});

/** Request options that diverge from Valhalla defaults, by dot path: every URL param without a codec. */
values.opts = optsFromHash(readHash());
