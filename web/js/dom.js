// DOM helpers shared by the page modules.

/** `<tag attrs>children</tag>`: `on*` attributes add listeners, false/null/undefined ones are skipped. */
export function el(tag, attrs = {}, ...children) {
  const node = tag === "svg" ? document.createElementNS("http://www.w3.org/2000/svg", tag) : document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? "" : v);
  }
  node.append(...children.flat().filter((c) => c !== null && c !== undefined && c !== false));
  return node;
}

/** A custom property of the page, e.g. `cssVar("--ink")`. */
export const cssVar = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
