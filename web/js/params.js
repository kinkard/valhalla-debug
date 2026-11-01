/** Options panel: binds the server-rendered form (src/params.rs) to the `opts` and `costing` state. */

import { APP_DEFAULTS } from "./request.js";

const TIMED = new Set([1, 2, 3]); // date_time types that need a value
const INPUT_DELAY = 250;

export function init(app) {
  const { state } = app;
  const panel = document.getElementById("panel-params");
  const timers = new Map();
  /** Groups the user opened, kept open across costing swaps. */
  const openGroups = new Set();

  const $ = (sel) => panel.querySelector(sel);
  const form = () => document.getElementById("params-form");
  const fieldFor = (path) => form()?.querySelector(`[name="${CSS.escape(path)}"], [data-name="${CSS.escape(path)}"]`);
  const opts = () => state.get("opts");

  function loadPanel() {
    const url = `/ui/params?costing=${encodeURIComponent(state.get("costing"))}`;
    htmx.ajax("GET", url, { source: panel, target: panel, swap: "innerHTML" });
  }

  function loadForm() {
    if (!form()) return loadPanel();
    const url = `/ui/params/form?costing=${encodeURIComponent(state.get("costing"))}`;
    htmx.ajax("GET", url, { source: panel, target: "#params-form", swap: "outerHTML" });
  }

  panel.addEventListener("htmx:afterSwap", (e) => {
    const f = form();
    if (!f) return;
    for (const group of f.querySelectorAll("details[data-group]")) group.open = openGroups.has(group.dataset.group);
    const trigger = e.detail.requestConfig?.elt;
    if (trigger?.matches?.("[data-costing]")) return switchCosting(f.dataset.costing);
    if (f.dataset.costing !== state.get("costing")) return loadForm();
    reflect();
    openChanged();
  });

  const loadFailed = (e) => {
    const path = e.detail.requestConfig?.path ?? "";
    if (!path.startsWith("/ui/params")) return;
    const why = e.detail.xhr?.status ? `server answered ${e.detail.xhr.status}` : "server unreachable";
    if (form()) return app.status(`Failed to load the options form (${why}). Try again.`, "error");
    panel.innerHTML = `<h2 class="panel-title">Request options</h2>
      <p class="params-hint">Failed to load the options form (${why}).</p>
      <button type="button" class="btn" data-action="retry">Try again</button>`;
  };
  panel.addEventListener("htmx:responseError", loadFailed);
  panel.addEventListener("htmx:sendError", loadFailed);

  /** Costing picked in the selector: drop the `co.*` options the new costing has no field for. */
  function switchCosting(costing) {
    const next = { ...opts() };
    const dropped = Object.keys(next).filter((path) => path.startsWith("co.") && !fieldFor(path));
    for (const path of dropped) delete next[path];
    state.set({ costing, opts: next });
    reflect();
    openChanged();
    if (dropped.length) app.status(`Removed options ${costing} doesn't use: ${dropped.map((p) => p.slice(3)).join(", ")}`);
  }

  function pathOf(el) {
    return el.name || el.dataset.name || "";
  }

  function defaultOf(el) {
    const raw = el.dataset.default;
    if (raw === undefined) return undefined;
    try {
      return JSON.parse(raw);
    } catch {
      return raw;
    }
  }

  /** Typed value of a field: `{ value }`, `{ unset: true }` (empty or default) or `{ error }`. */
  function readField(el) {
    const type = el.dataset.type;
    const def = defaultOf(el);
    const unset = { unset: true };
    if (type === "bool") return el.checked === (def ?? false) ? unset : { value: el.checked };
    if (type === "list") {
      const values = [...el.querySelectorAll("input:checked")].map((i) => i.value);
      return JSON.stringify(values) === JSON.stringify(def ?? []) ? unset : { value: values };
    }
    if (type === "json") {
      const text = el.value.trim();
      if (!text) return unset;
      try {
        return { value: JSON.parse(text) };
      } catch (err) {
        return { error: `Not valid JSON, not sent: ${err.message}` };
      }
    }
    if (type === "number" || type === "integer") {
      if (el.validity?.badInput) return { error: "Not a number, not sent." };
      if (el.value === "") return unset;
      const value = Number(el.value);
      return value === def ? unset : { value };
    }
    if (el.value === "") return unset;
    return el.value === def ? unset : { value: el.value };
  }

  function writeField(el, value) {
    const type = el.dataset.type;
    const def = defaultOf(el);
    if (type === "bool") el.checked = value === undefined ? def === true : value === true;
    else if (type === "list") {
      const list = Array.isArray(value) ? value : (def ?? []);
      for (const box of el.querySelectorAll("input")) box.checked = list.includes(box.value);
    } else if (type === "json") el.value = value === undefined ? "" : JSON.stringify(value);
    else if (el.tagName === "SELECT") {
      const text = value === undefined ? "" : String(value);
      if (![...el.options].some((o) => o.value === text)) el.add(new Option(`${text} (unknown)`, text));
      el.value = text;
    } else el.value = value === undefined ? "" : String(value);

    const slider = el.parentElement.querySelector(".prow-slider");
    if (slider) {
      slider.value = value ?? def ?? 0.5;
      slider.classList.toggle("is-default", value === undefined);
    }
  }

  function showError(el, error) {
    const row = el.closest(".prow");
    if (error) el.setAttribute("aria-invalid", "true");
    else el.removeAttribute("aria-invalid");
    const box = row?.querySelector(".prow-error");
    if (box) {
      box.textContent = error ?? "";
      box.hidden = !error;
    }
    row?.classList.toggle("is-invalid", !!error);
  }

  /** Writes one field into `opts`, plus the date_time rules: a timed type needs a value. */
  function commit(el) {
    const path = pathOf(el);
    const result = readField(el);
    showError(el, result.error);
    const next = { ...opts() };
    if (result.error || result.unset) delete next[path];
    else next[path] = result.value;

    if (path === "date_time.type") {
      if (!TIMED.has(next["date_time.type"])) delete next["date_time.value"];
      else if (!next["date_time.value"]) next["date_time.value"] = nowLocal();
    }
    if (path === "date_time.value" && TIMED.has(next["date_time.type"]) && !next["date_time.value"]) {
      showError(el, "Required for depart at, arrive by and invariant.");
    }
    state.set({ opts: next });
  }

  function nowLocal() {
    const d = new Date();
    const pad = (n) => String(n).padStart(2, "0");
    const date = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
    return `${date}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }

  const TEXTY = new Set(["number", "integer", "string", "json"]);

  function reflect() {
    const f = form();
    if (!f) return;
    const current = opts();
    for (const el of f.querySelectorAll("[data-type]")) {
      const path = pathOf(el);
      if (!path) continue;
      const has = path in current;
      const editing = TEXTY.has(el.dataset.type) && el.type !== "hidden" && el === document.activeElement;
      const keepInvalid = !has && el.getAttribute("aria-invalid") === "true";
      if (!editing && !keepInvalid) {
        writeField(el, current[path]);
        showError(el, null);
      }
      el.closest(".prow")?.classList.toggle("is-set", has);
    }
    updateTime();
    updateHints();
    updateCounts();
  }

  function updateTime() {
    const type = opts()["date_time.type"];
    const row = $("#params-time-value");
    if (row) row.hidden = !TIMED.has(type);
  }

  function updateHints() {
    const hint = $("#params-alt-hint");
    if (!hint) return;
    const o = opts();
    const type = o["date_time.type"];
    let text = "";
    let fix = false;
    if ((o.alternates ?? APP_DEFAULTS.alternates) > 0) {
      if (state.get("wp").length > 2) text = "Alternates need exactly two waypoints; with more Valhalla returns none.";
      else if ((type === 0 || type === 1) && !o.prioritize_bidirectional) {
        text = "Current and depart at run time-dependent A* (under 500 km), which finds no alternates.";
        fix = true;
      } else if (type === 2) text = "Arrive by runs time-dependent reverse A*, which finds no alternates.";
    }
    hint.hidden = !text;
    hint.textContent = text;
    if (fix) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "btn btn-ghost";
      button.dataset.action = "bidir";
      button.textContent = "Turn on prioritize bidirectional";
      hint.append(" ", button);
    }
  }

  function updateCounts() {
    const n = Object.keys(opts()).length;
    const count = $("#params-count");
    if (count) count.textContent = n ? `${n} changed` : "Defaults";
    const reset = $("#params-reset-all");
    if (reset) reset.hidden = !n;
    for (const group of panel.querySelectorAll("details[data-group]")) {
      const total = group.querySelectorAll(".prow").length;
      const set = group.querySelectorAll(".prow.is-set").length;
      group.querySelector("[data-group-count]").textContent = set ? `${set} of ${total} changed` : String(total);
      group.classList.toggle("has-set", set > 0);
    }
  }

  /** Groups holding changed options open, so a shared link shows what it changes. */
  function openChanged() {
    for (const group of panel.querySelectorAll("details[data-group]")) {
      if (group.querySelector(".prow.is-set, .prow.is-invalid")) group.open = true;
    }
  }

  function onInput(e) {
    const el = e.target;
    if (el.classList.contains("prow-slider")) {
      const num = fieldFor(el.dataset.for);
      num.value = el.value;
      el.classList.remove("is-default");
      return schedule(num, e.type === "change" ? 0 : 120);
    }
    const target = el.closest("[data-type]");
    if (!target || !form()?.contains(target)) return;
    const immediate = e.type === "change" || !TEXTY.has(target.dataset.type) || target.tagName === "SELECT";
    schedule(target, immediate ? 0 : INPUT_DELAY);
  }

  function schedule(el, delay) {
    clearTimeout(timers.get(el));
    if (!delay) return commit(el);
    timers.set(
      el,
      setTimeout(() => commit(el), delay),
    );
  }

  function reset(path) {
    const next = { ...opts() };
    delete next[path];
    if (path === "date_time.type") delete next["date_time.value"];
    const el = fieldFor(path);
    if (el) {
      clearTimeout(timers.get(el));
      writeField(el, undefined);
      showError(el, null);
    }
    state.set({ opts: next });
    reflect();
  }

  panel.addEventListener("input", onInput);
  panel.addEventListener("change", onInput);
  panel.addEventListener("submit", (e) => e.preventDefault());
  // `toggle` doesn't bubble
  panel.addEventListener(
    "toggle",
    (e) => {
      const group = e.target.dataset?.group;
      if (group) {
        if (e.target.open) openGroups.add(group);
        else openGroups.delete(group);
      }
    },
    true,
  );

  panel.addEventListener("click", (e) => {
    const button = e.target.closest("button");
    if (!button) return;
    if (button.dataset.reset) return reset(button.dataset.reset);
    switch (button.id || button.dataset.action) {
      case "params-reset-all": {
        const n = Object.keys(opts()).length;
        for (const el of form()?.querySelectorAll("[aria-invalid]") ?? []) showError(el, null);
        state.set({ opts: {} });
        reflect();
        app.status(`Reset ${n} option${n === 1 ? "" : "s"} to defaults`);
        break;
      }
      case "bidir":
        state.set({ opts: { ...opts(), prioritize_bidirectional: true } });
        break;
      case "retry":
        return loadPanel();
    }
  });

  state.on(["opts", "costing"], (changed) => {
    if (changed.includes("costing") && form() && form().dataset.costing !== state.get("costing")) return loadForm();
    reflect();
  });
  state.on("wp", updateHints);

  loadPanel();
}
