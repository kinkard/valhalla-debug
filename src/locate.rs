//! `GET /ui/locate`: a card body with the candidate edges of Valhalla's verbose `/locate` at a point.

use std::fmt::Write as _;

use axum::{
    extract::{Query, State, rejection::QueryRejection},
    http::StatusCode,
};
use maud::{Markup, html};
use serde::Deserialize;
use serde_json::{Value, json};
use valhalla::{Access, GraphId, proto};

use crate::{
    AppState,
    request::{self, Spec},
    valhalla::Error,
};

/// Query of `GET /ui/locate`, see `load` in `web/js/locate.js`.
#[derive(Deserialize)]
pub struct Params {
    lat: f64,
    lon: f64,
    /// The page's request state as JSON: costing and options to locate with.
    #[serde(deserialize_with = "json")]
    spec: Spec,
    /// GraphId value of the edge to preselect.
    edge: Option<u64>,
}

fn json<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Spec, D::Error> {
    let text = String::deserialize(d)?;
    serde_json::from_str(&text).map_err(serde::de::Error::custom)
}

impl Params {
    /// `/locate` request: the point with the page's costing and options, so candidates match what
    /// routing sees. Answered in JSON, `/locate` has no protobuf output.
    fn locate_request(&self, radius: Option<u32>) -> Result<proto::Api, String> {
        let mut location = json!({ "lat": self.lat, "lon": self.lon });
        if let Some(radius) = radius {
            location["radius"] = radius.into();
        }
        let spec = Spec {
            costing: self.spec.costing.clone(),
            locations: vec![location.as_object().cloned().unwrap_or_default()],
            opts: self.spec.opts.clone(),
        };
        let (options, _) = request::options(&spec, proto::options::Action::Locate)?;
        Ok(proto::Api {
            options: Some(proto::Options {
                has_verbose: Some(proto::options::HasVerbose::Verbose(true)),
                ..options
            }),
            ..Default::default()
        })
    }

    async fn locate(&self, state: &AppState, radius: Option<u32>) -> Result<Value, Error> {
        let api =
            (self.locate_request(radius)).map_err(|m| Error::new(StatusCode::BAD_REQUEST, m))?;
        let bytes = state.post("locate", &api).await?;
        serde_json::from_slice(&bytes).map_err(|err| {
            let message = format!("Valhalla returned invalid JSON: {err}");
            Error::new(StatusCode::BAD_GATEWAY, message)
        })
    }
}

/// Always answers a card body, errors included: the page shows whatever comes back.
pub async fn locate(
    State(state): State<AppState>,
    query: Result<Query<Params>, QueryRejection>,
) -> (StatusCode, Markup) {
    let params = match query {
        Ok(Query(params)) => params,
        Err(err) => {
            let hint = "This is a bug in the page, please report it.";
            return (err.status(), error_card(&err.body_text(), hint));
        }
    };
    let has_edge = |located: &Result<Value, Error>| {
        let mut all = located.iter().flat_map(candidates);
        all.any(|c| edge_id(c.value) == params.edge)
    };
    let mut located = params.locate(&state, None).await;
    // A clicked edge may sit a few meters off the click; widen the search once before giving up on it
    if params.edge.is_some() && located.is_ok() && !has_edge(&located) {
        let wider = params.locate(&state, Some(15)).await;
        if has_edge(&wider) {
            located = wider;
        }
    }

    match located {
        Ok(response) => (StatusCode::OK, render(&params, &response)),
        Err(err) => {
            let hint = if err.status == StatusCode::BAD_GATEWAY {
                "Check that Valhalla is running at --valhalla-url.".to_string()
            } else {
                let costing = &params.spec.costing;
                format!(
                    "Valhalla rejected the locate request for {costing}. Check the request options."
                )
            };
            (err.status, error_card(&err.to_string(), &hint))
        }
    }
}

struct CandidateRef<'a> {
    value: &'a Value,
    /// From `filtered_edges`: rejected by the costing or search filter.
    filtered: bool,
}

fn candidates(response: &Value) -> Vec<CandidateRef<'_>> {
    let list = |pointer, filtered| {
        let edges = array(response, pointer).iter();
        edges.map(move |value| CandidateRef { value, filtered })
    };
    list("/0/edges", false)
        .chain(list("/0/filtered_edges", true))
        .collect()
}

fn edge_id(edge: &Value) -> Option<u64> {
    edge.pointer("/edge_id/value").and_then(Value::as_u64)
}

fn text<'a>(v: &'a Value, pointer: &str) -> Option<&'a str> {
    v.pointer(pointer).and_then(Value::as_str)
}

fn num(v: &Value, pointer: &str) -> Option<f64> {
    v.pointer(pointer).and_then(Value::as_f64)
}

fn flag(v: &Value, pointer: &str) -> bool {
    v.pointer(pointer).and_then(Value::as_bool).unwrap_or(false)
}

fn array<'a>(v: &'a Value, pointer: &str) -> &'a [Value] {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// `level/tile/id` of a GraphId object in Valhalla's JSON.
fn graph_id_text(v: &Value, pointer: &str) -> Option<String> {
    let value = v.pointer(pointer)?.get("value")?.as_u64()?;
    Some(GraphId::new(value).to_string())
}

/// Travel modes as in Valhalla's access JSON: (json key, access bit, label).
const MODES: [(&str, Access, &str); 11] = [
    ("car", Access::AUTO, "car"),
    ("truck", Access::TRUCK, "truck"),
    ("bus", Access::BUS, "bus"),
    ("taxi", Access::TAXI, "taxi"),
    ("HOV", Access::HOV, "HOV"),
    ("motorcycle", Access::MOTORCYCLE, "motorcycle"),
    ("moped", Access::MOPED, "moped"),
    ("bicycle", Access::BICYCLE, "bicycle"),
    ("pedestrian", Access::PEDESTRIAN, "foot"),
    ("wheelchair", Access::WHEELCHAIR, "wheelchair"),
    ("emergency", Access::EMERGENCY, "emergency"),
];

/// Modes set in a `{"car": true, ...}` object, as `(access bit, label)`.
fn modes_in(v: Option<&Value>) -> impl Iterator<Item = (Access, &'static str)> + '_ {
    MODES
        .iter()
        .filter(move |(key, ..)| v.and_then(|v| v.get(*key)).and_then(Value::as_bool) == Some(true))
        .map(|&(_, bit, label)| (bit, label))
}

/// Access bits from a `{"car": true, ...}` object.
fn access_from_json(v: Option<&Value>) -> Option<Access> {
    v?.as_object()?;
    Some(modes_in(v).fold(Access::empty(), |acc, (bit, _)| acc | bit))
}

/// The access bit a costing model checks.
fn costing_mode(costing: &str) -> Option<Access> {
    Some(match costing {
        "auto" | "none" => Access::AUTO,
        "truck" => Access::TRUCK,
        "bus" => Access::BUS,
        "taxi" => Access::TAXI,
        "motorcycle" => Access::MOTORCYCLE,
        "motor_scooter" => Access::MOPED,
        "bicycle" | "bikeshare" => Access::BICYCLE,
        "pedestrian" | "multimodal" | "transit" => Access::PEDESTRIAN,
        _ => return None,
    })
}

/// Access in the opposite direction from the opposing candidate: `/locate` has no reverse access.
fn reverse_access(edge: &Value, all: &[CandidateRef]) -> Option<Access> {
    let way = num(edge, "/edge_info/way_id");
    let along = num(edge, "/percent_along")?;
    let forward = flag(edge, "/edge/forward");
    all.iter()
        .map(|c| c.value)
        .find(|o| {
            num(o, "/edge_info/way_id") == way
                && flag(o, "/edge/forward") != forward
                && num(o, "/percent_along").is_some_and(|p| (p + along - 1.0).abs() < 0.01)
        })
        .and_then(|o| access_from_json(o.pointer("/edge/access")))
}

/// Edge geometry as `[lon, lat]` pairs in the direction of travel.
fn edge_coords(edge: &Value) -> Vec<[f64; 2]> {
    let shape = text(edge, "/edge_info/shape").unwrap_or_default();
    let mut coords: Vec<[f64; 2]> = polyline_iter::decode(6, shape)
        .map(|(lat, lon)| [lon, lat])
        .collect();
    if !flag(edge, "/edge/forward") {
        coords.reverse();
    }
    coords
}

/// Tab label: names, or the road class when unnamed.
fn edge_title(edge: &Value) -> String {
    let names = array(edge, "/edge_info/names").iter();
    let names: Vec<&str> = names.filter_map(Value::as_str).collect();
    if names.is_empty() {
        let class = text(edge, "/edge/classification/use")
            .filter(|u| *u != "road")
            .or_else(|| text(edge, "/edge/classification/classification"))
            .unwrap_or("road");
        format!("Unnamed {}", class.replace('_', " "))
    } else {
        names.join(" \u{b7} ")
    }
}

const BUCKETS_PER_DAY: usize = 288;
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// "Tue 08:15" for a 5-minute bucket; bucket 0 is Sunday 00:00 local time, as in Valhalla.
fn bucket_label(bucket: usize) -> String {
    let day = DAYS[(bucket / BUCKETS_PER_DAY) % 7];
    format!("{day} {}", clock(bucket % BUCKETS_PER_DAY))
}

/// "08:15" for a bucket within a day; `BUCKETS_PER_DAY` gives "24:00".
fn clock(bucket_of_day: usize) -> String {
    let minutes = bucket_of_day * 5;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Time span of buckets `start..end` (end exclusive), e.g. "Tue 08:15-08:45".
fn span_label(start: usize, end: usize) -> String {
    if end <= start + 1 {
        return bucket_label(start);
    }
    if (end - 1) / BUCKETS_PER_DAY == start / BUCKETS_PER_DAY {
        let end_of_day = end - start / BUCKETS_PER_DAY * BUCKETS_PER_DAY;
        format!("{}-{}", bucket_label(start), clock(end_of_day))
    } else {
        format!("{}-{}", bucket_label(start), bucket_label(end))
    }
}

struct WeekSummary {
    min: u32,
    max: u32,
    mean: f64,
    /// First run of buckets at the weekly minimum, `start..end`.
    min_start: usize,
    min_end: usize,
}

fn summarize(speeds: &[u32]) -> Option<WeekSummary> {
    let min = *speeds.iter().min()?;
    let max = *speeds.iter().max()?;
    let mean = speeds.iter().map(|&s| f64::from(s)).sum::<f64>() / speeds.len() as f64;
    let min_start = speeds.iter().position(|&s| s == min)?;
    let min_end = min_start
        + speeds[min_start..]
            .iter()
            .take_while(|&&s| s == min)
            .count();
    Some(WeekSummary {
        min,
        max,
        mean,
        min_start,
        min_end,
    })
}

/// "lowest 34 km/h", then when, e.g. "Tue 08:15-08:45".
fn lowest_label(s: &WeekSummary) -> String {
    let span = span_label(s.min_start, s.min_end);
    format!("lowest {} km/h \u{b7} {span}", s.min)
}

// Chart geometry in SVG user units; the JS readout reads these from data attributes.
const CHART_W: f64 = 360.0;
/// Room for the highest and lowest speed labels.
const LABEL_W: f64 = 24.0;
/// Room for the minimum mark at the right end.
const INSET: f64 = 4.0;
const PLOT_W: f64 = CHART_W - LABEL_W - INSET;
/// Room above the plot for the top speed label.
const TOP: f64 = 6.0;
const ROW_H: f64 = 56.0;
const AXIS_H: f64 = 12.0;

/// Y range of the week, shared by all days: padded so that the minimum doesn't sit on the baseline, and at
/// least 10 km/h tall so that 1 km/h noise doesn't look like a traffic jam.
fn y_range(min: u32, max: u32) -> (f64, f64) {
    let (min, max) = (f64::from(min), f64::from(max));
    let span = (max - min).max(10.0);
    let mid = (min + max) / 2.0;
    let lo = (mid - span * 0.55).max(0.0);
    (lo, lo + span * 1.1)
}

/// Y of a speed in the chart.
fn speed_y(v: u32, lo: f64, hi: f64) -> f64 {
    TOP + ROW_H - (f64::from(v) - lo) / (hi - lo) * ROW_H
}

/// Line through the middle of each run of equal speeds, from edge to edge of the day: the speeds
/// are a smooth curve rounded to whole km/h, steps would draw the rounding.
fn day_path(day: &[u32], lo: f64, hi: f64) -> String {
    let Some(&first) = day.first() else {
        return String::new();
    };
    let bw = PLOT_W / BUCKETS_PER_DAY as f64;
    let mut d = format!("M{LABEL_W:.1} {:.1}", speed_y(first, lo, hi));
    let mut point = |bucket: f64, v: u32| {
        let _ = write!(d, "L{:.1} {:.1}", LABEL_W + bucket * bw, speed_y(v, lo, hi));
    };
    let mut start = 0;
    for i in 1..=day.len() {
        if i == day.len() || day[i] != day[start] {
            point((start + i) as f64 / 2.0, day[start]);
            start = i;
        }
    }
    point(day.len() as f64, day[day.len() - 1]);
    d
}

/// Predicted speeds, one day at a time: the day of the weekly minimum first, the others with the
/// day buttons. Days that reach the weekly minimum are marked, the week's extremes labeled.
fn speeds_section(edge: &Value) -> Markup {
    let speeds: Vec<u32> = array(edge, "/predicted_speeds")
        .iter()
        .filter_map(|v| v.as_u64().and_then(|s| u32::try_from(s).ok()))
        .collect();
    let Some(summary) = summarize(&speeds) else {
        return html! {
            p.loc-none { "No predicted speeds on this edge." }
        };
    };
    let (lo, hi) = y_range(summary.min, summary.max);
    let height = TOP + ROW_H + AXIS_H;
    let bw = PLOT_W / BUCKETS_PER_DAY as f64;
    let min_day = summary.min_start / BUCKETS_PER_DAY;
    // A low that runs past midnight is marked in the middle of its part on that day
    let min_end = summary.min_end.min((min_day + 1) * BUCKETS_PER_DAY);
    let min_x = LABEL_W
        + ((summary.min_start + min_end) as f64 / 2.0 - (min_day * BUCKETS_PER_DAY) as f64) * bw;
    let csv: Vec<String> = speeds.iter().map(u32::to_string).collect();
    let days: Vec<&[u32]> = speeds.chunks(BUCKETS_PER_DAY).collect();

    html! {
        section.loc-predicted {
            div.loc-predicted-head {
                strong { "Predicted speeds" }
                button.btn-link.loc-lowest type="button" data-act="lowest" title="Show this bucket" { (lowest_label(&summary)) }
                span.muted { "highest " (summary.max) " \u{b7} mean " (format!("{:.0}", summary.mean)) }
            }
            output.loc-readout aria-live="polite" {
                "Hover or use arrow keys to read a 5-minute bucket"
            }
            div.seg.loc-days role="group" aria-label="Day of the week" {
                @for (d, day) in days.iter().enumerate() {
                    @let day_min = day.iter().min().copied().unwrap_or(0);
                    button.is-min[day_min == summary.min] type="button" data-day=(d)
                        aria-pressed=(d == min_day) title={"lowest " (day_min) " km/h"} { (DAYS[d % 7]) }
                }
            }
            svg.loc-week viewBox={"0 0 " (CHART_W) " " (height)} tabindex="0" role="img"
                aria-label={"Predicted speeds for the week, " (lowest_label(&summary)) ". Arrow keys step through buckets, up and down through days."}
                data-speeds=(csv.join(",")) data-x0=(LABEL_W) data-w=(PLOT_W) data-top=(TOP) data-row=(ROW_H)
                data-lo=(format!("{lo:.1}")) data-hi=(format!("{hi:.1}")) data-min=(summary.min_start) data-day=(min_day)
            {
                @for hour in [6, 12, 18] {
                    @let x = LABEL_W + PLOT_W * f64::from(hour) / 24.0;
                    line.grid x1=(x) x2=(x) y1=(TOP) y2=(TOP + ROW_H) {}
                }
                @for v in [summary.max, summary.min] {
                    @let y = speed_y(v, lo, hi);
                    line.grid x1=(LABEL_W) x2=(LABEL_W + PLOT_W) y1=(y) y2=(y) {}
                    @if v == summary.max || summary.max != summary.min {
                        text.speed x=(LABEL_W - 3.0) y=(y + 3.0) text-anchor="end" { (v) }
                    }
                }
                @for (d, day) in days.iter().enumerate() {
                    @let line = day_path(day, lo, hi);
                    g.day.is-shown[d == min_day] data-day=(d) {
                        path.area d={(line) "V" (TOP + ROW_H) "H" (LABEL_W) "Z"} {}
                        path.line d=(line) {}
                        @if d == min_day {
                            circle.min-mark cx=(min_x) cy=(speed_y(summary.min, lo, hi)) r="3" {}
                        }
                    }
                }
                @for (hour, anchor) in [(0, "start"), (6, "middle"), (12, "middle"), (18, "middle"), (24, "end")] {
                    text.axis x=(LABEL_W + PLOT_W * f64::from(hour) / 24.0) y=(height - 2.0) text-anchor=(anchor) {
                        (format!("{hour:02}:00"))
                    }
                }
                rect.cursor x="0" y=(TOP) width=(bw.max(1.5)) height=(ROW_H) visibility="hidden" {}
                circle.cursor-dot cx="0" cy="0" r="2.5" visibility="hidden" {}
            }
        }
    }
}

/// The candidate as pretty JSON, without the 2016 predicted speeds: they don't read as an array.
fn raw_json(edge: &Value) -> String {
    let mut edge = edge.clone();
    if let Some(o) = edge.as_object_mut() {
        o.remove("predicted_speeds");
    }
    serde_json::to_string_pretty(&edge).unwrap_or_default()
}

fn render(params: &Params, response: &Value) -> Markup {
    let location = response.get(0).unwrap_or(&Value::Null);
    let all = candidates(response);
    let costing = &params.spec.costing;
    if all.is_empty() {
        let message = text(location, "/reason")
            .map(str::to_owned)
            .unwrap_or_else(|| format!("No roads within 35 km usable for {costing}."));
        return html! {
            div.loc-error role="alert" {
                p { strong { (message) } " Try another costing." }
            }
        };
    }

    let selected = (params.edge).and_then(|e| all.iter().position(|c| edge_id(c.value) == Some(e)));
    let missing_edge = params.edge.filter(|_| selected.is_none());
    let selected = selected.unwrap_or(0);
    let mode = costing_mode(costing);
    let nodes = array(location, "/nodes");
    let node = nodes.first().filter(|_| nodes.len() == 1);

    html! {
        @if let Some(edge) = missing_edge {
            p.loc-note {
                "Edge " span.mono { (GraphId::new(edge).to_string()) } " is not among the edges Valhalla snaps to here for "
                (costing) ". Showing the nearest ones instead."
            }
        }
        @if let Some(node) = node {
            p.loc-note {
                "Snapped to node "
                span.mono { (graph_id_text(node, "/node_id").unwrap_or_default()) }
                " \u{b7} " (text(node, "/type").unwrap_or("node").replace('_', " "))
                " \u{b7} " (num(node, "/edge_count").unwrap_or(0.0)) " edges"
                @if flag(node, "/traffic_signal") { " \u{b7} traffic signal" }
            }
        }
        div.loc-tabs role="tablist" aria-label="Candidate edges" {
            @for (i, c) in all.iter().enumerate() {
                @let heading = format!("{:.0}", num(c.value, "/heading").unwrap_or(0.0));
                @let title = edge_title(c.value);
                button.loc-tab.is-filtered[c.filtered] type="button" role="tab" data-i=(i)
                    aria-selected=(if i == selected { "true" } else { "false" })
                    title={(title) ", heading " (heading) "\u{b0}" @if c.filtered { ", filtered out" }}
                {
                    span.loc-dir style={"transform:rotate(" (heading) "deg)"} aria-hidden="true" { "\u{2191}" }
                    span.loc-tab-name { (title) }
                }
            }
        }
        @for (i, c) in all.iter().enumerate() {
            (candidate_panel(i, i == selected, c, &all, mode))
        }
    }
}

fn error_card(message: &str, hint: &str) -> Markup {
    html! {
        div.loc-error role="alert" {
            p { strong { (message) } }
            p.muted { (hint) }
        }
    }
}

fn access_badges(access: Option<Access>, mode: Option<Access>) -> Markup {
    let Some(access) = access else {
        return html! { span.muted { "unknown" } };
    };
    html! {
        span.loc-badges {
            @for (_, bit, label) in MODES {
                @let allowed = access.contains(bit);
                span.badge.is-yes[allowed].is-no[!allowed].is-mode[mode == Some(bit)]
                    title={(label) (if allowed { " allowed" } else { " not allowed" })} { (label) }
            }
        }
    }
}

fn speed_cell(label: &str, value: Option<String>) -> Markup {
    html! {
        div.loc-stat {
            span.muted { (label) }
            @match value {
                Some(v) => span { (v) },
                None => span.muted { "-" },
            }
        }
    }
}

fn kmh(v: Option<f64>) -> Option<String> {
    v.filter(|v| *v > 0.0).map(|v| format!("{v:.0}"))
}

fn candidate_panel(
    i: usize,
    selected: bool,
    c: &CandidateRef,
    all: &[CandidateRef],
    mode: Option<Access>,
) -> Markup {
    let e = c.value;
    let coords = edge_coords(e);
    let snap = num(e, "/correlated_lon")
        .zip(num(e, "/correlated_lat"))
        .map(|(lon, lat)| json!([lon, lat]).to_string());
    let way_id = e.pointer("/edge_info/way_id").and_then(Value::as_u64);
    let class = text(e, "/edge/classification/classification").unwrap_or("?");
    let use_ = text(e, "/edge/classification/use").unwrap_or("?");
    let forward_access = access_from_json(e.pointer("/edge/access"));
    let rev_access = reverse_access(e, all);
    let speed_limit = match e.pointer("/edge_info/speed_limit") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(v) => kmh(v.as_f64()),
        None => None,
    };
    let live_speed = num(e, "/live_speed/overall_speed").map(|v| format!("{v:.0}"));
    let restriction = |key: &str| -> Vec<&str> {
        modes_in(e.pointer(&format!("/edge/{key}")))
            .map(|(_, label)| label)
            .collect()
    };
    let start_restriction = restriction("start_restriction");
    let end_restriction = restriction("end_restriction");
    let access_restrictions = array(e, "/access_restrictions");
    let flags: Vec<&str> = [
        ("/edge/toll", "toll"),
        ("/edge/tunnel", "tunnel"),
        ("/edge/bridge", "bridge"),
        ("/edge/round_about", "roundabout"),
        ("/edge/classification/link", "link"),
        ("/edge/classification/internal", "internal"),
        ("/edge/destination_only", "destination only"),
        ("/edge/destination_only_hgv", "destination only HGV"),
        ("/edge/not_thru", "not thru"),
        ("/edge/deadend", "dead end"),
        ("/edge/access_restriction", "access restriction"),
        ("/edge/part_of_complex_restriction", "complex restriction"),
        ("/edge/traffic_signal", "traffic signal"),
        ("/edge/stop_sign", "stop sign"),
        ("/edge/yield_sign", "yield sign"),
        ("/edge/country_crossing", "country crossing"),
        ("/edge/truck_route", "truck route"),
        ("/edge/has_sign", "has sign"),
        ("/shoulder", "shoulder"),
    ]
    .into_iter()
    .filter(|(p, _)| flag(e, p))
    .map(|(_, label)| label)
    .collect();

    html! {
        section.loc-panel role="tabpanel" data-i=(i) data-edge=[edge_id(e)]
            data-geom=(json!(coords).to_string()) data-snap=[snap] hidden[!selected]
        {
            @if c.filtered {
                p.loc-note { "Filtered out by the costing or the location's search filter." }
            }
            div.loc-title {
                strong { (edge_title(e)) }
                span.muted {
                    (class.replace('_', " "))
                    @if use_ != "road" { " \u{b7} " (use_.replace('_', " ")) }
                    @if let Some(m) = num(e, "/edge/geo_attributes/length") {
                        " \u{b7} " @if m >= 1000.0 { (format!("{:.2} km", m / 1000.0)) } @else { (format!("{m:.0} m")) }
                    }
                }
            }
            div.loc-links {
                @if let Some(way) = way_id {
                    a href={"https://www.openstreetmap.org/way/" (way)} target="_blank" rel="noopener" { "way " (way) }
                }
                button.btn-link type="button" data-act="copy-json" { "Copy edge JSON" }
            }
            dl.loc-facts {
                dt { "Edge" }
                dd {
                    span.mono title=[edge_id(e)] {
                        (graph_id_text(e, "/edge_id").unwrap_or_else(|| "?".into()))
                    }
                    @if let Some(end) = graph_id_text(e, "/edge/end_node") {
                        span.muted { " \u{2192} node " span.mono { (end) } }
                    }
                }
                dt { "Speeds" }
                dd.loc-stats {
                    (speed_cell(&format!("speed, {}", text(e, "/edge/speeds/type").unwrap_or("?")), kmh(num(e, "/edge/speeds/default"))))
                    (speed_cell("free-flow", kmh(num(e, "/edge/speeds/free_flow"))))
                    (speed_cell("constrained", kmh(num(e, "/edge/speeds/constrained_flow"))))
                    (speed_cell("limit", speed_limit))
                    (speed_cell("live", live_speed))
                    @if let Some(timed) = num(e, "/timed_predicted_speed") {
                        (speed_cell("at date_time", kmh(Some(timed))))
                    }
                }
                dt { "Forward" }
                dd { (access_badges(forward_access, mode)) }
                dt { "Reverse" }
                dd { (access_badges(rev_access, mode)) }
                @if !start_restriction.is_empty() || !end_restriction.is_empty() || !access_restrictions.is_empty() {
                    dt { "Restrictions" }
                    dd {
                        @if !start_restriction.is_empty() { div { "starts complex restriction for " (start_restriction.join(", ")) } }
                        @if !end_restriction.is_empty() { div { "ends complex restriction for " (end_restriction.join(", ")) } }
                        @for r in access_restrictions {
                            div {
                                (text(r, "/type").unwrap_or("?").replace('_', " "))
                                @if let Some(v) = r.get("value") { " " span.mono { (v.to_string()) } }
                                @let modes: Vec<&str> = modes_in(Some(r)).map(|(_, label)| label).collect();
                                span.muted { " for " @if modes.is_empty() { "no modes" } @else { (modes.join(", ")) } }
                                @if flag(r, "/except_destination") { span.muted { ", except destination" } }
                            }
                        }
                    }
                }
                dt { "Attributes" }
                dd {
                    span.muted {
                        (text(e, "/edge/classification/surface").unwrap_or("?").replace('_', " "))
                        @let lanes = num(e, "/edge/lane_count").unwrap_or(0.0);
                        " \u{b7} " (lanes) @if lanes == 1.0 { " lane" } @else { " lanes" }
                        @if let Some(g) = num(e, "/edge/geo_attributes/weighted_grade") { " \u{b7} grade " (format!("{g:.1}")) "%" }
                        @if let Some(cl) = text(e, "/edge/cycle_lane").filter(|v| *v != "none") { " \u{b7} cycle lane " (cl) }
                        @if let Some(s) = text(e, "/edge/sac_scale").filter(|v| *v != "none") { " \u{b7} sac " (s) }
                    }
                    @if !flags.is_empty() {
                        span.loc-badges { @for f in &flags { span.badge.is-flag { (f) } } }
                    }
                }
                dt { "Snap" }
                dd {
                    (text(e, "/side_of_street").unwrap_or("?")) " side \u{b7} "
                    (format!("{:.0}", num(e, "/percent_along").unwrap_or(0.0) * 100.0)) "% along \u{b7} "
                    (format!("{:.1}", num(e, "/distance").unwrap_or(0.0))) " m away \u{b7} heading "
                    (format!("{:.0}", num(e, "/heading").unwrap_or(0.0))) "\u{b0}"
                }
                @if e.get("outbound_reach").is_some() || e.get("inbound_reach").is_some() {
                    dt { "Reach" }
                    dd {
                        "out " (num(e, "/outbound_reach").unwrap_or(0.0)) " \u{b7} in " (num(e, "/inbound_reach").unwrap_or(0.0))
                        span.muted { " nodes" }
                    }
                }
            }
            (speeds_section(e))
            details.loc-raw {
                summary { "Raw JSON" }
                pre.mono { (raw_json(e)) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_labels() {
        assert_eq!(bucket_label(0), "Sun 00:00");
        assert_eq!(bucket_label(287), "Sun 23:55");
        assert_eq!(bucket_label(2015), "Sat 23:55");
        // Tuesday 08:15 = 2 days + 99 buckets
        assert_eq!(span_label(675, 676), "Tue 08:15");
        assert_eq!(span_label(675, 681), "Tue 08:15-08:45");
        assert_eq!(span_label(286, 288), "Sun 23:50-24:00");
        assert_eq!(span_label(286, 290), "Sun 23:50-Mon 00:10");
    }

    #[test]
    fn test_summarize() {
        assert!(summarize(&[]).is_none());
        let mut week = vec![90; 2016];
        for s in &mut week[675..681] {
            *s = 34;
        }
        week[1000] = 34;
        week[1500] = 110;
        let s = summarize(&week).unwrap();
        assert_eq!((s.min, s.max, s.min_start, s.min_end), (34, 110, 675, 681));
        assert_eq!(lowest_label(&s), "lowest 34 km/h \u{b7} Tue 08:15-08:45");

        let flat = summarize(&[50; 288]).unwrap();
        assert_eq!((flat.min_start, flat.min_end), (0, 288));
        assert_eq!(flat.mean, 50.0);
    }

    #[test]
    fn test_y_range() {
        let (lo, hi) = y_range(34, 98);
        assert!(lo < 34.0 && hi > 98.0);
        // Flat or near-flat weeks still get a 10 km/h tall scale
        let (lo, hi) = y_range(50, 50);
        assert!((hi - lo - 11.0).abs() < 1e-9 && lo < 50.0 && hi > 50.0);
        assert_eq!(y_range(0, 2).0, 0.0);
    }

    #[test]
    fn test_day_path_through_run_middles() {
        let day = [10, 10, 20];
        let d = day_path(&day, 0.0, 20.0);
        let x = |bucket: f64| LABEL_W + bucket * PLOT_W / 288.0;
        let expected = format!(
            "M24.0 34.0L{:.1} 34.0L{:.1} 6.0L{:.1} 6.0",
            x(1.0),
            x(2.5),
            x(3.0)
        );
        assert_eq!(d, expected);
        assert_eq!(day_path(&[], 0.0, 20.0), "");
    }

    #[test]
    fn low_past_midnight_is_marked_on_its_day() {
        let mut week = vec![80; 2016];
        // Mon 23:50 - Tue 00:40
        week[288 + 286..2 * 288 + 8].fill(20);
        let svg = speeds_section(&json!({ "predicted_speeds": week })).into_string();
        let cx: f64 = svg
            .split("class=\"min-mark\" cx=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(cx < LABEL_W + PLOT_W, "{cx}");
    }

    #[test]
    fn test_speeds_section() {
        let mut week = vec![80; 2016];
        week[300] = 20;
        let svg = speeds_section(&json!({ "predicted_speeds": week })).into_string();
        assert_eq!(svg.matches("class=\"line\"").count(), 7);
        assert!(svg.contains("data-min=\"300\""));
        assert_eq!(svg.matches("is-shown").count(), 1, "one day shown");
        assert!(
            svg.contains("class=\"day is-shown\" data-day=\"1\""),
            "Monday has the minimum"
        );
        assert!(svg.contains("class=\"is-min\" type=\"button\" data-day=\"1\""));
        assert!(
            svg.contains(">80</text>") && svg.contains(">20</text>"),
            "highest and lowest labeled"
        );
        assert!(svg.contains("lowest 20 km/h \u{b7} Mon 01:00"));
    }

    #[test]
    fn raw_json_drops_predicted_speeds() {
        let v = json!({"edge_info": {"way_id": 42}, "predicted_speeds": [1, 2]});
        let raw: Value = serde_json::from_str(&raw_json(&v)).unwrap();
        assert_eq!(raw, json!({"edge_info": {"way_id": 42}}));
    }

    #[test]
    fn test_access_from_json() {
        let access = access_from_json(Some(
            &json!({"car": true, "pedestrian": false, "bicycle": true}),
        ))
        .unwrap();
        assert_eq!(access, Access::AUTO | Access::BICYCLE);
        assert_eq!(access_from_json(None), None);
    }

    #[test]
    fn test_reverse_access_from_opposing_candidate() {
        let fwd = json!({"percent_along": 0.3, "edge_info": {"way_id": 7}, "edge": {"forward": true, "access": {"car": true}}});
        let rev = json!({"percent_along": 0.7, "edge_info": {"way_id": 7}, "edge": {"forward": false, "access": {"pedestrian": true}}});
        let candidate = |value| CandidateRef {
            value,
            filtered: false,
        };
        let all = [candidate(&fwd), candidate(&rev)];
        assert_eq!(reverse_access(&fwd, &all), Some(Access::PEDESTRIAN));
        assert_eq!(reverse_access(&fwd, &all[..1]), None);
    }

    #[test]
    fn test_edge_coords_follow_direction() {
        let shape = polyline_iter::encode(6, [(55.0, 13.0), (55.1, 13.1)]);
        let fwd = json!({"edge_info": {"shape": shape}, "edge": {"forward": true}});
        let rev = json!({"edge_info": {"shape": shape}, "edge": {"forward": false}});
        assert_eq!(edge_coords(&fwd)[0], [13.0, 55.0]);
        assert_eq!(edge_coords(&rev)[0], [13.1, 55.1]);
    }

    #[test]
    fn test_params() {
        let spec = r#"{"costing":"bicycle","opts":{"co.use_roads":0.1},"locations":[]}"#;
        let parse = |v: Value| serde_json::from_value::<Params>(v);
        let p = parse(json!({"lat": 55.6, "lon": 13.0, "edge": 887353665058u64, "spec": spec}))
            .unwrap();
        assert_eq!(p.edge, Some(887353665058));
        let options = p.locate_request(None).unwrap().options.unwrap();
        let bicycle = proto::costing::Type::Bicycle as i32;
        assert_eq!(options.costing_type, bicycle);
        assert_eq!(
            options.has_verbose,
            Some(proto::options::HasVerbose::Verbose(true))
        );
        let Some(proto::costing::HasOptions::Options(co)) = &options.costings[&bicycle].has_options
        else {
            panic!("no costing options");
        };
        assert_eq!(
            co.has_use_roads,
            Some(proto::costing::options::HasUseRoads::UseRoads(0.1))
        );
        assert_eq!(options.locations.len(), 1);

        assert!(parse(json!({"lat": "x", "lon": 13.0, "spec": "{}"})).is_err());
        assert!(parse(json!({"lat": 1.0, "lon": 2.0, "spec": "[1]"})).is_err());
    }
}
