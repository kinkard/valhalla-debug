// Typed or pasted `lat, lon` and the `wp` URL value.

const NUMBER = String.raw`[-+]?(?:\d+(?:\.\d*)?|\.\d+)`;
const PAIR = new RegExp(String.raw`^\s*(${NUMBER})\s*[,\s]\s*(${NUMBER})\s*$`);

export const round6 = (x) => Math.round(Number(x) * 1e6) / 1e6;

export function formatLatLon({ lat, lon }) {
  return `${Number(lat).toFixed(6)}, ${Number(lon).toFixed(6)}`;
}

/** `lat, lon` as Google Maps copies it, also space-separated. Returns `{ point }` or `{ error }`. */
export function parseLatLon(text) {
  const m = String(text ?? "").match(PAIR);
  if (!m) return { error: "Type or paste lat, lon" };
  try {
    return { point: checked(m[1], m[2]) };
  } catch (err) {
    return { error: err.message };
  }
}

function checked(lat, lon) {
  lat = Number(lat);
  lon = Number(lon);
  if (!Number.isFinite(lat) || !Number.isFinite(lon)) throw new Error("Coordinates must be numbers");
  if (Math.abs(lat) > 90) throw new Error(`Latitude ${lat} is out of range, it must be within ±90`);
  if (Math.abs(lon) > 180) throw new Error(`Longitude ${lon} is out of range, it must be within ±180`);
  return { lat: round6(lat), lon: round6(lon) };
}

/** `wp` URL value: `;`-separated `lat,lon` with `!key=value` options, e.g. `55.6,13.0!type=through`. */
export function encodeWaypoints(list) {
  return list
    .map(({ lat, lon, ...rest }) =>
      [`${round6(lat)},${round6(lon)}`, ...Object.entries(rest).map(([k, v]) => `${k}=${untyped(v)}`)].join("!"),
    )
    .join(";");
}

/** Inverse of `encodeWaypoints`; throws on bad coordinates. */
export function decodeWaypoints(s) {
  return s
    .split(";")
    .filter(Boolean)
    .map((item) => {
      const [coord, ...rest] = item.split("!");
      const [lat, lon] = coord.split(",");
      const wp = checked(lat, lon);
      for (const kv of rest) {
        const eq = kv.indexOf("=");
        if (eq > 0) wp[kv.slice(0, eq)] = typed(kv.slice(eq + 1));
      }
      return wp;
    });
}

/** Inverse of `typed`: objects and arrays as JSON. */
export const untyped = (v) => (typeof v === "object" ? JSON.stringify(v) : String(v));

/** URL value as a JSON object/array, number or boolean, else the string. */
export function typed(s) {
  if (/^[[{]/.test(s)) {
    try {
      return JSON.parse(s);
    } catch {
      return s;
    }
  }
  if (s === "true") return true;
  if (s === "false") return false;
  if (s !== "" && /^-?\d+(\.\d+)?(e-?\d+)?$/i.test(s)) return Number(s);
  return s;
}
