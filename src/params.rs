//! Options panel: an HTMX form from `options.json`, empty unless set, defaults as placeholders.
//! `co.x` names `costing_options.<costing>.x`.

use std::sync::LazyLock;

use axum::extract::Query;
use maud::{Markup, html};
use serde::Deserialize;
use serde_json::Value;

/// Costings the form knows about, in selector order, with their button labels.
const COSTINGS: [(&str, &str); 8] = [
    ("auto", "Auto"),
    ("truck", "Truck"),
    ("bicycle", "Bicycle"),
    ("pedestrian", "Pedestrian"),
    ("motorcycle", "Motorcycle"),
    ("motor_scooter", "Scooter"),
    ("bus", "Bus"),
    ("taxi", "Taxi"),
];

/// Options shown in the essentials block instead of their group.
const ESSENTIAL_COSTING: [&str; 3] = ["use_highways", "use_tolls", "use_ferry"];

static OPTIONS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("options.json")).expect("options.json parses")
});

/// `schema` is its entry in `options.json`.
struct Opt {
    name: &'static str,
    schema: &'static Value,
}

impl Opt {
    fn default(&self) -> &Value {
        &self.schema["default"]
    }

    fn min(&self) -> Option<f64> {
        self.schema["minimum"].as_f64()
    }

    fn max(&self) -> Option<f64> {
        self.schema["maximum"].as_f64()
    }
}

/// Options of a section of `options.json`, in file order.
fn options(section: &'static Value) -> Vec<Opt> {
    let opts = section.as_object().into_iter().flatten();
    opts.map(|(name, schema)| Opt { name, schema }).collect()
}

#[derive(Deserialize)]
pub struct CostingQuery {
    costing: Option<String>,
}

impl CostingQuery {
    fn costing(&self) -> &str {
        self.costing
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .unwrap_or("auto")
    }
}

/// `GET /ui/params?costing=auto`: the whole panel, with the form for `costing`.
pub async fn panel(Query(query): Query<CostingQuery>) -> Markup {
    render_panel(query.costing())
}

/// `GET /ui/params/form?costing=auto`: only the form, swapped in when the costing changes.
pub async fn form(Query(query): Query<CostingQuery>) -> Markup {
    render_form(query.costing())
}

fn render_panel(costing: &str) -> Markup {
    html! {
        h2.panel-title {
            span { "Request options" }
            span.aside.params-summary {
                span #params-count { "Defaults" }
                button.btn.btn-ghost.params-reset-all type="button" #params-reset-all hidden {
                    "Reset all"
                }
            }
        }
        (render_form(costing))
    }
}

fn render_form(costing: &str) -> Markup {
    // Valhalla's spec gives bus and taxi auto's options
    let own_key = if matches!(costing, "bus" | "taxi") {
        "auto"
    } else {
        costing
    };
    let own = options(&OPTIONS["costings"][own_key]);
    let common = match own.is_empty() {
        true => Vec::new(),
        false => options(&OPTIONS["shared"]),
    };
    let general = options(&OPTIONS["request"]);
    let in_group = |o: &&Opt| !ESSENTIAL_COSTING.contains(&o.name);
    let label = COSTINGS
        .iter()
        .find(|(c, _)| *c == costing)
        .map_or(costing, |(_, l)| l);
    let groups = [
        ("own", format!("{label} options"), &own, "co."),
        ("common", "Shared costing options".into(), &common, "co."),
        ("request", "Request".into(), &general, ""),
    ];

    html! {
        form #params-form data-costing=(costing) autocomplete="off" {
            (costing_selector(costing))
            p.params-hint {
                @if own.is_empty() { "No form for costing " code { (costing) } ". " }
                "Descriptions are in the "
                a href="https://valhalla.github.io/valhalla/api/turn-by-turn/api-reference/" target="_blank" rel="noopener" {
                    "Valhalla API reference"
                }
                "."
            }
            div.pgroup-body.params-essentials {
                (time_rows())
                // Why a route may come without alternates, see `updateHints` in params.js
                p.prow-note.prow-warn #params-alt-hint hidden {}
                @for name in ESSENTIAL_COSTING {
                    @if let Some(opt) = own.iter().chain(&common).find(|o| o.name == name) {
                        (field(opt, &format!("co.{name}")))
                    }
                }
            }
            @for (id, title, opts, prefix) in groups {
                (group(id, &title, &opts.iter().filter(in_group).collect::<Vec<_>>(), prefix))
            }
        }
    }
}

fn costing_selector(current: &str) -> Markup {
    html! {
        div.seg.params-costing role="group" aria-label="Costing" {
            @for (costing, label) in COSTINGS {
                button type="button" title=(costing)
                    aria-pressed=(if costing == current { "true" } else { "false" })
                    data-costing=(costing)
                    hx-get={"/ui/params/form?costing=" (costing)}
                    hx-target="#params-form" hx-swap="outerHTML" hx-sync="#panel-params:replace" {
                    (label)
                }
            }
        }
    }
}

/// `date_time` as a type select plus a local date-time shown only for the types that need one.
fn time_rows() -> Markup {
    let types = [
        ("0", "Current"),
        ("1", "Depart at"),
        ("2", "Arrive by"),
        ("3", "Invariant"),
    ];
    html! {
        (row("date_time.type", "Time", "date_time.type", "", html! {
            select.field id="p-date_time-type" name="date_time.type" data-type="integer" {
                option value="" { "None" }
                @for (value, label) in types { option value=(value) { (label) } }
            }
        }))
        div.prow #params-time-value hidden {
            label.prow-label for="p-date_time-value" {
                span.prow-name { "Date and time" }
                (key_markup("date_time.value"))
            }
            div.prow-ctl {
                input.field id="p-date_time-value" type="datetime-local" name="date_time.value"
                    data-type="string";
            }
            p.prow-error hidden {}
        }
        div.prow {
            label.prow-label for="p-prioritize_bidirectional" {
                span.prow-name { "Prioritize bidirectional" }
                (key_markup("prioritize_bidirectional"))
            }
            div.prow-ctl {
                input id="p-prioritize_bidirectional" type="checkbox" name="prioritize_bidirectional"
                    data-type="bool" data-default="false";
                (reset_button("prioritize_bidirectional"))
            }
        }
    }
}

/// A collapsible group of option rows.
fn group(id: &str, title: &str, opts: &[&Opt], prefix: &str) -> Markup {
    if opts.is_empty() {
        return html! {};
    }
    html! {
        details.pgroup data-group=(id) {
            summary { (title) span.aside data-group-count { (opts.len()) } }
            div.pgroup-body {
                @for opt in opts { (field(opt, &format!("{prefix}{}", opt.name))) }
            }
        }
    }
}

/// One option row: readable label with the exact key, a typed control, unit and reset.
fn field(opt: &Opt, path: &str) -> Markup {
    let unit = unit(opt.name);
    let label = readable(opt.name);
    html! { (row(path, &label, opt.name, unit, control(opt, path))) }
}

fn row(path: &str, label: &str, key: &str, unit: &str, control: Markup) -> Markup {
    let name = html! { span.prow-name { (label) } (key_markup(key)) };
    html! {
        div.prow {
            // A checkbox list is a fieldset, which a <label for> can't target
            @if control.0.starts_with("<fieldset") {
                div.prow-label { (name) }
            } @else {
                label.prow-label for=(field_id(path)) { (name) }
            }
            div.prow-ctl {
                (control)
                span.prow-unit { (unit) }
                (reset_button(path))
            }
            p.prow-error hidden {}
        }
    }
}

/// The exact option key, breakable after underscores so long keys wrap at word boundaries.
fn key_markup(key: &str) -> Markup {
    html! {
        code.prow-key {
            @for (i, part) in key.split('_').enumerate() {
                @if i > 0 { "_" wbr; }
                (part)
            }
        }
    }
}

fn reset_button(path: &str) -> Markup {
    html! {
        button.btn.btn-ghost.btn-icon.prow-reset type="button" data-reset=(path)
            title="Reset to default" aria-label={"Reset " (path)} { "\u{d7}" }
    }
}

/// By the option's `type` and `enum`: a list of checkboxes for an array of enum values, JSON for
/// other arrays and objects.
fn control(opt: &Opt, path: &str) -> Markup {
    let id = field_id(path);
    let default = opt.default();
    let default_attr = default_json(default);
    let kind = opt.schema["type"].as_str().unwrap_or("object");
    match (kind, opt.schema["enum"].as_array()) {
        ("boolean", _) => html! {
            input id=(id) type="checkbox" name=(path) data-type="bool"
                data-default=(if default.as_bool() == Some(true) { "true" } else { "false" });
        },
        ("array", Some(variants)) => html! {
            fieldset.prow-list id=(id) data-name=(path) data-type="list"
                data-default=[default_attr] {
                legend.sr-only { (path) }
                @for v in variants {
                    @let value = display(v);
                    label { input type="checkbox" value=(value); " " (value) }
                }
            }
        },
        (_, Some(variants)) => html! {
            select.field id=(id) name=(path) data-type="string" data-default=[default_attr] {
                option value="" {
                    @if default.is_null() { "Not set" } @else { "Default (" (display(default)) ")" }
                }
                @for v in variants {
                    @let value = display(v);
                    option value=(value) { (value) }
                }
            }
        },
        ("number" | "integer", _) => {
            let step = if kind == "integer" { "1" } else { "any" };
            let slider = opt.min() == Some(0.0) && opt.max() == Some(1.0);
            let placeholder = (!default.is_null()).then(|| display(default));
            html! {
                @if slider {
                    input.prow-slider type="range" min="0" max="1" step="0.05" data-for=(path)
                        value=(default.as_f64().unwrap_or(0.5)) tabindex="-1" aria-hidden="true";
                }
                input.field.prow-num id=(id) type="number" name=(path) data-type=(kind)
                    step=(step) min=[opt.min()] max=[opt.max()] placeholder=[placeholder]
                    data-default=[default_attr] inputmode="decimal";
            }
        }
        ("string", _) => html! {
            input.field id=(id) type="text" name=(path) data-type="string" spellcheck="false"
                placeholder=[(!default.is_null()).then(|| display(default))];
        },
        _ => html! {
            textarea.field.mono id=(id) name=(path) data-type="json" rows="2"
                spellcheck="false" placeholder="JSON" {}
        },
    }
}

fn field_id(path: &str) -> String {
    format!("p-{}", path.replace('.', "-"))
}

fn default_json(value: &Value) -> Option<String> {
    (!value.is_null()).then(|| value.to_string())
}

/// Human form of a spec value: numbers and strings as is, arrays comma separated.
fn display(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(list) => list.iter().map(display).collect::<Vec<_>>().join(", "),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Unit by Valhalla's naming conventions.
fn unit(name: &str) -> &'static str {
    match name {
        "height" | "width" | "length" | "elevation_interval" => "m",
        "weight" | "axle_load" => "t",
        "max_grade" => "%",
        _ if name.starts_with("use_") => "",
        _ if name.ends_with("_cost") || name.ends_with("_penalty") => "s",
        _ if name.ends_with("_speed") => "km/h",
        _ if name.ends_with("_distance") => "m",
        _ => "",
    }
}

/// `prioritize_bidirectional` -> "Prioritize bidirectional", with acronyms kept upper case.
fn readable(name: &str) -> String {
    let words: Vec<String> = name
        .split('_')
        .enumerate()
        .map(|(i, w)| match w {
            "hgv" | "hov2" | "hov3" | "hot" | "bss" | "osm" => w.to_uppercase(),
            _ if i == 0 => {
                let mut chars = w.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            }
            _ => w.to_owned(),
        })
        .collect();
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_tag<'a>(html: &'a str, name: &str) -> &'a str {
        let needle = format!("name=\"{name}\"");
        let at = html
            .find(&needle)
            .unwrap_or_else(|| panic!("no field {name}"));
        let start = html[..at].rfind('<').unwrap();
        let end = at + html[at..].find('>').unwrap();
        &html[start..=end]
    }

    /// Valhalla's API spec at the commit valhalla-rs builds.
    const OPENAPI: &str = "https://raw.githubusercontent.com/valhalla/valhalla/c803bdc2f712358c0376e6b6cacf7ae6baf9b7df/docs/docs/api/openapi.yaml";

    /// Options the form leaves out: chosen elsewhere in the page, set by valhalla-debug, or only for recostings.
    const SKIP: [&str; 10] = [
        "id",
        "costing",
        "costing_options",
        "locations",
        "date_time",
        "alternates",
        "prioritize_bidirectional",
        "format",
        "shape_format",
        "name",
    ];

    fn resolve<'a>(spec: &'a Value, v: &'a Value) -> &'a Value {
        match v["$ref"]
            .as_str()
            .and_then(|r| r.strip_prefix("#/components/schemas/"))
        {
            Some(name) => resolve(spec, &spec["components"]["schemas"][name]),
            None => v,
        }
    }

    /// Type, range and default of the properties of a schema and of its `allOf` parts.
    fn properties(spec: &Value, schema: &Value) -> Vec<(String, String)> {
        let schema = resolve(spec, schema);
        let parts = schema["allOf"].as_array().into_iter().flatten();
        let mut out: Vec<_> = parts.flat_map(|part| properties(spec, part)).collect();
        for (name, p) in schema["properties"].as_object().into_iter().flatten() {
            let p = resolve(spec, p);
            let mut opt = serde_json::Map::new();
            for key in ["type", "default", "minimum", "maximum", "enum"] {
                if let Some(v) = p.get(key) {
                    opt.insert(key.into(), v.clone());
                }
            }
            // An array of enum values, like `speed_types`, keeps the values of its items as `enum`
            if let Some(variants) = resolve(spec, &p["items"]).get("enum") {
                opt.insert("enum".into(), variants.clone());
            }
            if !SKIP.contains(&name.as_str()) {
                out.push((name.clone(), Value::from(opt).to_string()));
            }
        }
        out
    }

    /// `{` and one `"key": value` per line.
    fn object(entries: impl IntoIterator<Item = (String, String)>, indent: &str) -> String {
        let lines: Vec<String> = entries
            .into_iter()
            .map(|(key, value)| format!("{indent}  {}: {value}", Value::from(key)))
            .collect();
        format!("{{\n{}\n{indent}}}", lines.join(",\n"))
    }

    /// `options.json` must match Valhalla's spec: run with `--ignored` after a Valhalla bump, and with
    /// `UPDATE_OPTIONS=1` to rewrite it.
    #[test]
    #[ignore = "fetches Valhalla's spec from GitHub"]
    fn options_match_valhalla_spec() {
        let curl = std::process::Command::new("curl")
            .args(["-fsSL", OPENAPI])
            .output();
        let yaml = curl
            .ok()
            .filter(|out| out.status.success())
            .expect(OPENAPI)
            .stdout;
        let spec: Value = serde_saphyr::from_str(std::str::from_utf8(&yaml).unwrap()).unwrap();
        let schema = |name: &str| properties(&spec, &spec["components"]["schemas"][name]);
        let shared = schema("BaseCostingOptions");
        let costings = COSTINGS
            .iter()
            .filter(|(c, _)| !matches!(*c, "bus" | "taxi"));
        let costings = costings.map(|(costing, _)| {
            let costing_schema =
                &spec["components"]["schemas"]["CostingOptions"]["properties"][costing];
            let own = properties(&spec, costing_schema).into_iter();
            (
                costing.to_string(),
                object(own.filter(|o| !shared.contains(o)), "    "),
            )
        });
        let sections = [
            ("request".into(), object(schema("RouteRequest"), "  ")),
            ("shared".into(), object(shared.clone(), "  ")),
            ("costings".into(), object(costings, "  ")),
        ];
        let json = object(sections, "") + "\n";
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/options.json");
        if std::env::var_os("UPDATE_OPTIONS").is_some() {
            std::fs::write(path, &json).unwrap();
        } else {
            let message = "differs from Valhalla's spec, rerun with UPDATE_OPTIONS=1";
            assert!(json == include_str!("options.json"), "{path} {message}");
        }
    }

    #[test]
    fn placeholder_is_the_per_costing_default() {
        let auto = render_form("auto").into_string();
        assert!(input_tag(&auto, "co.top_speed").contains("placeholder=\"140\""));
        let truck = render_form("truck").into_string();
        assert!(input_tag(&truck, "co.top_speed").contains("placeholder=\"120\""));
        assert!(input_tag(&truck, "co.height").contains("placeholder=\"4.11\""));
        let bike = render_form("bicycle").into_string();
        assert!(input_tag(&bike, "co.use_roads").contains("data-default=\"0.25\""));
        assert!(bike.contains("Default (hybrid)"));
    }

    #[test]
    fn bus_and_taxi_reuse_auto_options() {
        for costing in ["bus", "taxi"] {
            let html = render_form(costing).into_string();
            assert!(input_tag(&html, "co.top_speed").contains("placeholder=\"140\""));
        }
    }

    #[test]
    fn options_of_other_costings_are_left_out() {
        let bike = render_form("bicycle").into_string();
        assert!(!bike.contains("name=\"co.use_highways\""));
        assert!(bike.contains("name=\"co.use_ferry\""));
        let auto = render_form("auto").into_string();
        assert!(auto.contains("name=\"co.toll_booth_cost\""));
        assert!(!auto.contains("name=\"co.use_lit\""));
    }

    #[test]
    fn ranges_and_types_are_on_the_field() {
        let auto = render_form("auto").into_string();
        let tag = input_tag(&auto, "co.top_speed");
        assert!(
            tag.contains("min=\"10\"") && tag.contains("max=\"252\""),
            "{tag}"
        );
        assert!(
            tag.contains("data-type=\"integer\"") && tag.contains("step=\"1\""),
            "{tag}"
        );
        assert!(input_tag(&auto, "co.use_highways").contains("max=\"1\""));
        assert!(input_tag(&auto, "date_time.type").contains("data-type=\"integer\""));
        assert!(input_tag(&auto, "co.ignore_oneways").contains("data-default=\"false\""));
        assert!(input_tag(&auto, "roundabout_exits").contains("data-default=\"true\""));
        assert!(input_tag(&auto, "units").starts_with("<select"));
        assert!(input_tag(&auto, "exclude_polygons").starts_with("<textarea"));
        assert!(auto.contains(r#"data-name="co.speed_types" data-type="list""#));
    }

    #[test]
    fn unknown_costing_renders_general_options_only() {
        let html = render_form("auto_pedestrian").into_string();
        assert!(html.contains("No form for costing"));
        assert!(!html.contains("name=\"co."));
        assert!(html.contains("name=\"units\""));
    }

    #[test]
    fn essentials_come_first_and_page_breaking_options_are_left_out() {
        let html = render_panel("auto").into_string();
        let first_group = html.find("data-group=").unwrap();
        for name in ["date_time.type", "prioritize_bidirectional", "co.use_tolls"] {
            assert!(
                html.find(&format!("name=\"{name}\"")).unwrap() < first_group,
                "{name}"
            );
        }
        for name in ["id", "costing", "format", "shape_format", "name"] {
            assert!(!html.contains(&format!("name=\"{name}\"")), "{name}");
        }
        assert!(!html.contains("name=\"co.name\""));
    }

    #[test]
    fn readable_labels() {
        assert_eq!(
            readable("prioritize_bidirectional"),
            "Prioritize bidirectional"
        );
        assert_eq!(readable("hgv_no_access_penalty"), "HGV no access penalty");
        assert_eq!(readable("include_hov2"), "Include HOV2");
    }

    #[test]
    fn units() {
        assert_eq!(unit("service_penalty"), "s");
        assert_eq!(unit("gate_cost"), "s");
        assert_eq!(unit("alley_factor"), "");
        assert_eq!(unit("top_speed"), "km/h");
        assert_eq!(unit("use_highways"), "");
        assert_eq!(unit("use_distance"), "");
        assert_eq!(unit("max_distance"), "m");
    }
}
