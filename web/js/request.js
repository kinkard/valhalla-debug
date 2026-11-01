import * as state from "./state.js";

/** This app's defaults that differ from Valhalla's. */
export const APP_DEFAULTS = { alternates: 2 };

/** `Spec` in src/request.rs. */
export function spec() {
  return { costing: state.get("costing"), locations: state.get("wp"), opts: { ...APP_DEFAULTS, ...state.get("opts") } };
}
