//! `POST /ui/route`: the routes panel and shapes for the map. The response has the trip, the only
//! place with costs, recostings and admins.

use std::{fmt::Write as _, time::Instant};

use axum::{Json, extract::State, http::StatusCode};
use maud::{Markup, PreEscaped, html};
use prost::Message;
use serde::Serialize;
use valhalla::proto::{
    self, CodedDescription,
    options::{Action, Format, Units},
};

use crate::{
    AppState,
    request::{self, Spec},
    valhalla::Error,
};

/// Trip attributes the directions and the profile don't need, a third of a long route (Valhalla 3.9.1).
const TRIP_EXCLUDE: [&str; 13] = [
    "node.time_zone",
    "edge.max_downward_grade",
    "edge.max_upward_grade",
    "edge.mean_elevation",
    "edge.weighted_grade",
    "edge.way_id",
    "edge.id",
    "edge.density",
    "edge.speed_limit",
    "edge.traversability",
    "edge.lane_count",
    "edge.speeds_faded",
    "edge.speeds_non_faded",
];

/// Meters between the elevation samples of the profile, unless the request sets its own.
const ELEVATION_INTERVAL: f32 = 30.0;

/// `POST /ui/route` with the page's [`Spec`]: answers 200 with a panel fragment, Valhalla's errors
/// included. The page selects the route to show.
pub async fn route_panel(State(state): State<AppState>, Json(spec): Json<Spec>) -> Markup {
    let (api, mut warnings) = match request::options(&spec, Action::Route) {
        Ok((options, warnings)) => (route_request(options), warnings),
        Err(err) => return error_panel(&Error::new(StatusCode::BAD_REQUEST, err), 0),
    };

    let begin = Instant::now();
    let result = state.post("route", &api).await;
    let ms = begin.elapsed().as_millis() as u64;
    let options = api.options.as_ref().unwrap();
    let routes = match result {
        Ok(bytes) => proto::Api::decode(bytes.as_ref()).map_err(|err| err.to_string()),
        Err(err) => return error_panel(&err, ms),
    }
    .and_then(|response| {
        warnings.extend(response.info.iter().flat_map(|i| i.warnings.clone()));
        routes(response, options)
    });
    match routes {
        Ok(routes) => render_panel(options, &routes, &warnings, ms),
        Err(err) => {
            let message = format!("Valhalla answered, but not with a route: {err}");
            error_panel(&Error::new(StatusCode::BAD_GATEWAY, message), ms)
        }
    }
}

/// The page's options plus the directions and the trip with elevation, as protobuf.
fn route_request(options: proto::Options) -> proto::Api {
    let elevation_interval = match options.elevation_interval {
        0.0 => ELEVATION_INTERVAL,
        set => set,
    };
    proto::Api {
        options: Some(proto::Options {
            elevation_interval,
            format: Format::Pbf as i32,
            pbf_field_selector: Some(proto::PbfFieldSelector {
                trip: true,
                directions: true,
                ..Default::default()
            }),
            filter_action: proto::FilterAction::Exclude as i32,
            filter_attributes: TRIP_EXCLUDE.map(str::to_owned).to_vec(),
            ..options
        }),
        ..Default::default()
    }
}

/// Rendered into the panel, and serialized for the map and the profile.
#[derive(Serialize)]
struct Trip {
    summary: Summary,
    legs: Vec<Leg>,
    /// `date_time` and time zone of the first and the last location.
    depart: Option<String>,
    arrive: Option<String>,
    countries: Vec<String>,
}

#[derive(Default, Serialize)]
struct Summary {
    time: f64,
    /// In the request's units.
    length: f64,
    cost: f64,
    has_toll: bool,
    has_highway: bool,
    has_ferry: bool,
    has_time_restrictions: bool,
    /// Time by recosting name, `None` when the recosting can't traverse the route.
    recosts: Vec<(String, Option<f64>)>,
}

#[derive(Serialize)]
struct Leg {
    summary: Summary,
    /// With their cost from the trip nodes, only in the panel's markup.
    #[serde(skip)]
    maneuvers: Vec<(proto::directions_leg::Maneuver, f64)>,
    /// Encoded polyline6.
    shape: String,
    #[serde(flatten)]
    profile: Profile,
}

/// What the profile charts: the edges' speeds and elevation samples as Valhalla sent them, placed
/// along the leg in the request's units, like the leg lengths.
#[derive(Default, Serialize)]
struct Profile {
    /// `(along, height)`: meters, -32768 where Valhalla has no elevation data. Pairs, not objects:
    /// a long leg has tens of thousands.
    elevation: Vec<(f64, f32)>,
    /// `(to, km/h)` per edge, each starting where the previous one ends.
    speeds: Vec<(f64, f32)>,
}

/// The primary route and the alternates, from the directions and the trip of the same request.
fn routes(api: proto::Api, options: &proto::Options) -> Result<Vec<Trip>, String> {
    let directions = api.directions.ok_or("no directions")?;
    let trip = api.trip.ok_or("no trip")?;
    if directions.routes.len() != trip.routes.len() || directions.routes.is_empty() {
        return Err(format!(
            "{} directions for {} trips",
            directions.routes.len(),
            trip.routes.len()
        ));
    }
    let units = options.units();
    let recostings: Vec<&str> = options
        .recostings
        .iter()
        .map(|c| match &c.has_name {
            Some(proto::costing::HasName::Name(name)) => name.as_str(),
            None => "",
        })
        .collect();
    let time_text = |l: Option<&proto::Location>| {
        let l = l.filter(|l| !l.date_time.is_empty())?;
        let time = l.date_time.replace('T', " ");
        Some(match l.time_zone_name.as_str() {
            "" => time,
            zone => format!("{time} {zone}"),
        })
    };

    directions
        .routes
        .into_iter()
        .zip(&trip.routes)
        .map(|(route, trip)| {
            if route.legs.len() != trip.legs.len() {
                return Err(format!(
                    "{} legs in directions, {} in the trip",
                    route.legs.len(),
                    trip.legs.len()
                ));
            }
            let depart = time_text(route.legs.first().and_then(|l| l.location.first()));
            let arrive = time_text(route.legs.last().and_then(|l| l.location.last()));
            let legs: Vec<Leg> = route
                .legs
                .into_iter()
                .zip(&trip.legs)
                .map(|(leg, trip_leg)| self::leg(leg, trip_leg, &recostings, units))
                .collect();
            Ok(Trip {
                summary: total(&legs, &recostings),
                depart,
                arrive,
                countries: trip
                    .legs
                    .iter()
                    .flat_map(|l| &l.admin)
                    .map(|a| a.country_code.clone())
                    .collect(),
                legs,
            })
        })
        .collect()
}

fn leg(leg: proto::DirectionsLeg, trip: &proto::TripLeg, recostings: &[&str], units: Units) -> Leg {
    let elapsed = |node: Option<&proto::trip_leg::Node>| {
        node.and_then(|n| n.cost.as_ref())
            .and_then(|c| c.elapsed_cost.as_ref())
            .map_or(0.0, |c| c.cost)
    };
    let node = |i: u32| trip.node.get(i as usize);
    let last = trip.node.last();
    let s = leg.summary.unwrap_or_default();
    let recosts = last.map_or(&[][..], |n| &n.recosts[..]);
    let summary = Summary {
        time: s.time,
        length: s.length as f64,
        cost: elapsed(last),
        has_toll: s.has_toll,
        has_highway: s.has_highway,
        has_ferry: s.has_ferry,
        has_time_restrictions: s.has_time_restrictions,
        recosts: recostings
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let seconds = recosts
                    .get(i)
                    .and_then(|r| r.elapsed_cost.as_ref())
                    .map(|c| c.seconds);
                (name.to_string(), seconds)
            })
            .collect(),
    };
    let maneuvers = leg
        .maneuver
        .into_iter()
        .map(|m| {
            let cost = elapsed(node(m.end_path_index)) - elapsed(node(m.begin_path_index));
            (m, cost)
        })
        .collect();
    Leg {
        summary,
        maneuvers,
        shape: leg.shape,
        profile: profile(trip, units),
    }
}

/// Every edge's speed and elevation samples, the samples `elevation_sampling_interval` apart from
/// the start of the edge.
fn profile(trip_leg: &proto::TripLeg, units: Units) -> Profile {
    let per_km = match units {
        Units::Kilometers => 1.0,
        Units::Miles => 1.0 / 1.609344,
    };
    let mut out = Profile::default();
    let mut start_km = 0.0;
    for edge in trip_leg.node.iter().filter_map(|n| n.edge.as_ref()) {
        let end_km = start_km + edge.length_km as f64;
        let step_km = edge.elevation_sampling_interval as f64 / 1000.0;
        for (i, &height) in edge.elevation.iter().enumerate() {
            let along = (start_km + i as f64 * step_km).min(end_km);
            out.elevation.push((round3(along * per_km), height));
        }
        out.speeds.push((round3(end_km * per_km), edge.speed));
        start_km = end_km;
    }
    out
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Route totals: a recosting is untraversable when any of its legs is.
fn total(legs: &[Leg], recostings: &[&str]) -> Summary {
    let mut s = Summary {
        recosts: recostings
            .iter()
            .map(|name| (name.to_string(), Some(0.0)))
            .collect(),
        ..Default::default()
    };
    for leg in legs {
        let l = &leg.summary;
        s.time += l.time;
        s.length += l.length;
        s.cost += l.cost;
        s.has_toll |= l.has_toll;
        s.has_highway |= l.has_highway;
        s.has_ferry |= l.has_ferry;
        s.has_time_restrictions |= l.has_time_restrictions;
        for ((_, total), (_, leg)) in s.recosts.iter_mut().zip(&l.recosts) {
            *total = total.zip(*leg).map(|(a, b)| a + b);
        }
    }
    s
}

impl Trip {
    /// Route-wide flags, as (badge label, explanation).
    fn flags(&self) -> Vec<(&'static str, &'static str)> {
        let s = &self.summary;
        let mut countries = self.countries.iter().filter(|c| !c.is_empty());
        let border = countries
            .next()
            .is_some_and(|first| countries.any(|c| c != first));
        let rough = self
            .legs
            .iter()
            .flat_map(|l| &l.maneuvers)
            .any(|(m, _)| m.portions_unpaved);
        let timed = "Passes edges with time-dependent access restrictions";
        [
            (s.has_highway, "Highway", "Uses highways"),
            (s.has_toll, "Toll", "Uses toll roads"),
            (s.has_ferry, "Ferry", "Uses a ferry"),
            (s.has_time_restrictions, "Timed access", timed),
            (border, "Border", "Crosses a country border"),
            (rough, "Unpaved", "Includes unpaved or rough roads"),
        ]
        .into_iter()
        .filter_map(|(on, label, title)| on.then_some((label, title)))
        .collect()
    }
}

const MINUS: char = '\u{2212}';

/// Overpass draws `·` flush left, a thin space after it evens out the gaps.
const SEP: &str = " \u{b7}\u{2009} ";

/// `45 s`, `18 min 36 s`, `5 h 47 min`.
fn format_duration(seconds: f64) -> String {
    let total = seconds.abs().round() as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    match (h, m) {
        (0, 0) => format!("{s} s"),
        (0, _) if s == 0 => format!("{m} min"),
        (0, _) => format!("{m} min {s} s"),
        _ if m == 0 => format!("{h} h"),
        _ => format!("{h} h {m} min"),
    }
}

/// Valhalla lengths are in km or miles: `48 m`, `20.6 km`, `512 km`, `264 ft`, `12.3 mi`.
fn format_distance(length: f64, units: Units) -> String {
    let length = length.abs();
    let unit = if units == Units::Miles { "mi" } else { "km" };
    match units {
        Units::Kilometers if length < 1.0 => format!("{:.0} m", length * 1000.0),
        Units::Miles if length < 0.1 => format!("{:.0} ft", length * 5280.0),
        _ if length < 100.0 => format!("{length:.1} {unit}"),
        _ => format!("{length:.0} {unit}"),
    }
}

/// Costs are unitless and usually in the hundreds or thousands, so decimals only help small ones.
fn format_cost(cost: f64) -> String {
    if cost.abs() < 100.0 {
        format!("{cost:.1}")
    } else {
        format!("{cost:.0}")
    }
}

fn route_name(index: usize) -> String {
    match index {
        0 => "Primary".into(),
        _ => format!("Alt {index}"),
    }
}

fn route_title(index: usize) -> String {
    match index {
        0 => "Primary route".into(),
        _ => format!("Alternate {index}"),
    }
}

fn color_var(index: usize) -> String {
    format!("--c: var(--route-{})", index % 4)
}

/// What `web/js/routes.js` reads from the panel: the shapes for the map, the legs for the profile.
#[derive(Serialize)]
struct Meta<'a> {
    ms: u64,
    units: &'static str,
    routes: &'a [Trip],
}

/// `warnings` of the request, the same for every route.
fn render_panel(
    options: &proto::Options,
    routes: &[Trip],
    warnings: &[CodedDescription],
    ms: u64,
) -> Markup {
    let units = options.units();
    let best = |f: fn(&Summary) -> f64| {
        routes
            .iter()
            .map(|t| f(&t.summary))
            .fold(f64::INFINITY, f64::min)
    };
    let (best_time, best_length, best_cost) =
        (best(|s| s.time), best(|s| s.length), best(|s| s.cost));
    let compare = routes.len() > 1;
    let requested = match options.has_alternates {
        Some(proto::options::HasAlternates::Alternates(n)) => n,
        None => 0,
    };
    let count = format!(
        "{} route{}",
        routes.len(),
        if routes.len() == 1 { "" } else { "s" }
    );
    let meta = Meta {
        ms,
        units: units.as_str_name(),
        routes,
    };

    html! {
        h2.panel-title { "Routes" span.aside { (count) (SEP) (ms) " ms" } }
        div.rt-table role="group" aria-label="Route comparison" {
            div.rt-head aria-hidden="true" {
                span {} span { "Time" } span { "Distance" } span { "Cost" }
            }
            @for (i, trip) in routes.iter().enumerate() {
                @let s = &trip.summary;
                @let tip = format!("{}{SEP}{}", format_duration(s.time), format_distance(s.length, units));
                button.rt-row type="button" data-index=(i) aria-pressed=(i == 0) style=(color_var(i))
                    data-name=(route_title(i)) data-tip=(tip) {
                    span.rt-name { i.rt-swatch {} (route_name(i)) }
                    span.rt-val.rt-best[compare && s.time == best_time] { (format_duration(s.time)) }
                    span.rt-val.rt-best[compare && s.length == best_length] { (format_distance(s.length, units)) }
                    span.rt-val.rt-best[compare && s.cost == best_cost] { (format_cost(s.cost)) }
                }
            }
        }
        div.rt-tools {
            div.rt-stepper role="group" aria-label="Alternates to request" {
                span.rt-stepper-label { "Alternates" }
                button.btn.btn-icon data-alternates=(requested.saturating_sub(1)) disabled[requested == 0]
                    aria-label="Request one alternate less" { (MINUS) }
                output.rt-stepper-value { (requested) }
                button.btn.btn-icon data-alternates=(requested + 1) aria-label="Request one more alternate" { "+" }
            }
            button.btn.btn-ghost.rt-profile type="button" data-action="profile" aria-pressed="false"
                aria-controls="panel-profile" title="Chart speed and elevation along every route" { "Speed & elevation" }
        }
        @for (i, trip) in routes.iter().enumerate() {
            (route_details(i, trip, units, warnings))
        }
        script.rt-meta type="application/json" {
            // `<` only occurs inside JSON strings, where `<` means the same: `</script>` can't end it early
            (PreEscaped(serde_json::to_string(&meta).unwrap_or_default().replace('<', "\\u003c")))
        }
    }
}

fn route_details(index: usize, trip: &Trip, units: Units, warnings: &[CodedDescription]) -> Markup {
    let s = &trip.summary;
    let flags = trip.flags();
    let maneuvers: usize = trip.legs.iter().map(|l| l.maneuvers.len()).sum();

    html! {
        section.rt-detail data-index=(index) hidden[index > 0] style=(color_var(index)) aria-label=(route_title(index)) {
            h3.rt-detail-title { i.rt-swatch {} (route_title(index)) }
            dl.rt-dl {
                dt { "Time" }
                dd { (format_duration(s.time)) }
                dt { "Distance" }
                dd { (format_distance(s.length, units)) }
                dt { "Cost" }
                dd { (format!("{:.1}", s.cost)) }
                @for (name, time) in &s.recosts {
                    dt { "Time " span.mono { (name) } }
                    dd {
                        @match time {
                            Some(t) => (format_duration(*t)),
                            None => span.muted { "not traversable" },
                        }
                    }
                }
                @if let Some(depart) = &trip.depart {
                    dt { "Departs" }
                    dd { (depart) }
                }
                @if let Some(arrive) = &trip.arrive {
                    dt { "Arrives" }
                    dd { (arrive) }
                }
                @if !flags.is_empty() {
                    dt { "Uses" }
                    dd.rt-flags { @for (label, title) in &flags { span.rt-flag title=(title) { (label) } } }
                }
            }
            @if !warnings.is_empty() {
                ul.rt-warnings {
                    @for w in warnings {
                        li { (w.description) span.muted { " (" (w.code) ")" } }
                    }
                }
            }
            @if trip.legs.len() > 1 {
                ol.rt-legs {
                    @for (i, leg) in trip.legs.iter().enumerate() {
                        li {
                            span { "Leg " (i + 1) }
                            span { (format_duration(leg.summary.time)) }
                            span { (format_distance(leg.summary.length, units)) }
                            span { (format_cost(leg.summary.cost)) }
                        }
                    }
                }
            }
            details.rt-maneuvers {
                summary { "Maneuvers " span.muted { "(" (maneuvers) ")" } }
                @for (leg_index, leg) in trip.legs.iter().enumerate() {
                    @if trip.legs.len() > 1 {
                        div.rt-leg-head { "Leg " (leg_index + 1) }
                    }
                    ol.rt-man-list {
                        @for (m, cost) in &leg.maneuvers {
                            li.rt-man tabindex="0" data-leg=(leg_index) data-begin=(m.begin_shape_index)
                                data-end=(m.end_shape_index) title=(format!("Maneuver type {}, shape {}..{}", m.r#type, m.begin_shape_index, m.end_shape_index)) {
                                span { (m.text_instruction) }
                                span.rt-man-meta { (maneuver_meta(m, *cost, units)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// `48 m · 4 s · cost 4.8 · toll`
fn maneuver_meta(m: &proto::directions_leg::Maneuver, cost: f64, units: Units) -> String {
    let mut out = format!(
        "{}{SEP}{}{SEP}cost {cost:.1}",
        format_distance(m.length as f64, units),
        format_duration(m.time),
    );
    for (on, label) in [
        (m.portions_toll, "toll"),
        (m.portions_ferry, "ferry"),
        (m.portions_unpaved, "unpaved"),
        (m.has_time_restrictions, "timed access"),
    ] {
        if on {
            let _ = write!(out, "{SEP}{label}");
        }
    }
    out
}

/// What to do about the common Valhalla errors, see `valhalla/src/exceptions.cc`.
fn error_hint(code: u64) -> Option<&'static str> {
    Some(match code {
        100 => "Valhalla could not parse the request.",
        125 => "Valhalla doesn't know this costing. Pick another one.",
        150 => "Too many locations for this server's service_limits. Remove some waypoints.",
        154 | 166 => {
            "The route is longer than this server's service_limits allow for the costing. Move the waypoints closer or use a server with higher limits."
        }
        160..=163 | 165 => {
            "Check date_time: depart-at and arrive-by need a value like 2026-10-30T08:00."
        }
        170 => {
            "The waypoints are in parts of the graph that are not connected. Move a waypoint onto the main network."
        }
        171 => {
            "A waypoint is too far from any road this costing can use. Move it onto a road, or raise the location's radius or search_cutoff."
        }
        172 => {
            "Every pair of waypoints is farther apart than the breakage distance. Move them closer."
        }
        442 => {
            "No path connects the waypoints with this costing. Check access restrictions, closures and exclude options, or try another costing. Trucks default to 21.64 m, 4.11 m tall and 21.77 t, which size restrictions in town centres often rule out."
        }
        _ => return None,
    })
}

/// No `.rt-meta`: the page reads the message from `.rt-error-message`.
fn error_panel(err: &Error, ms: u64) -> Markup {
    let hint = match err.code {
        Some(code) => error_hint(code),
        None if err.message.starts_with("Failed to reach Valhalla") => {
            Some("Check that Valhalla is running at --valhalla-url.")
        }
        None => None,
    };
    html! {
        h2.panel-title { "Routes" span.aside { "Failed" @if ms > 0 { (SEP) (ms) " ms" } } }
        div.rt-error role="alert" {
            p.rt-error-message {
                (err.message)
                @if let Some(code) = err.code { " " span.muted { "(error_code " (code) ")" } }
            }
            @if let Some(hint) = hint { p { (hint) } }
            p.rt-stale-note.muted hidden { "The map shows the last route that worked, dimmed." }
        }
        // Valhalla answers `/expansion` for failed routes too
        div.rt-tools {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use valhalla::proto::trip_leg;

    fn options(opts: serde_json::Value) -> proto::Options {
        let spec = Spec {
            costing: "auto".into(),
            opts: opts.as_object().unwrap().clone(),
            ..Default::default()
        };
        request::options(&spec, Action::Route).unwrap().0
    }

    fn cost(seconds: f64, cost: f64) -> Option<trip_leg::PathCost> {
        Some(trip_leg::PathCost {
            elapsed_cost: Some(trip_leg::Cost { seconds, cost }),
            ..Default::default()
        })
    }

    /// One route of one leg over two edges, 1 km of motorway then 0.5 km of residential road.
    fn api() -> proto::Api {
        use proto::{directions_leg::Maneuver, trip_leg::Edge};
        let edge = |length_km, speed, elevation: Vec<f32>| Edge {
            length_km,
            speed,
            elevation_sampling_interval: 500.0,
            elevation,
            ..Default::default()
        };
        let node = |edge, cost| trip_leg::Node {
            edge,
            cost,
            ..Default::default()
        };
        let recosted = trip_leg::Node {
            recosts: vec![cost(100.0, 0.0).unwrap(), trip_leg::PathCost::default()],
            ..node(None, cost(90.0, 140.0))
        };
        let maneuver = |text: &str, begin_path_index, end_path_index, length, time| Maneuver {
            text_instruction: text.into(),
            begin_path_index,
            end_path_index,
            length,
            time,
            ..Default::default()
        };
        let location = |date_time: &str| proto::Location {
            date_time: date_time.into(),
            time_zone_name: "Europe/Stockholm".into(),
            ..Default::default()
        };
        proto::Api {
            directions: Some(proto::Directions {
                routes: vec![proto::DirectionsRoute {
                    legs: vec![proto::DirectionsLeg {
                        summary: Some(proto::Summary {
                            length: 1.5,
                            time: 90.0,
                            has_toll: true,
                            ..Default::default()
                        }),
                        maneuver: vec![
                            maneuver("Drive east on <Baltzarsgatan>.", 0, 1, 1.0, 40.0),
                            maneuver("Turn right.", 1, 2, 0.5, 50.0),
                        ],
                        shape: "_p~iF~ps|U".into(),
                        location: vec![location("2026-10-30T08:00"), location("2026-10-30T08:02")],
                        ..Default::default()
                    }],
                }],
            }),
            trip: Some(proto::Trip {
                routes: vec![proto::TripRoute {
                    legs: vec![proto::TripLeg {
                        node: vec![
                            node(
                                Some(edge(1.0, 110.0, vec![10.0, 20.0, 30.0])),
                                cost(0.0, 0.0),
                            ),
                            node(
                                Some(edge(0.5, 30.0, vec![30.0, -32768.0])),
                                cost(40.0, 60.0),
                            ),
                            recosted,
                        ],
                        admin: vec![
                            trip_leg::Admin {
                                country_code: "SE".into(),
                                ..Default::default()
                            },
                            trip_leg::Admin {
                                country_code: "DK".into(),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            }),
            ..Default::default()
        }
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(0.4), "0 s");
        assert_eq!(format_duration(45.2), "45 s");
        assert_eq!(format_duration(1115.742), "18 min 36 s");
        assert_eq!(format_duration(120.0), "2 min");
        assert_eq!(format_duration(3600.0), "1 h");
        assert_eq!(format_duration(20843.0), "5 h 47 min");
    }

    #[test]
    fn distances() {
        assert_eq!(format_distance(0.048, Units::Kilometers), "48 m");
        assert_eq!(format_distance(20.568, Units::Kilometers), "20.6 km");
        assert_eq!(format_distance(612.4, Units::Kilometers), "612 km");
        assert_eq!(format_distance(0.05, Units::Miles), "264 ft");
        assert_eq!(format_distance(12.34, Units::Miles), "12.3 mi");
    }

    #[test]
    fn panel_shows_the_route() {
        let options = options(json!({ "alternates": 2 }));
        let html =
            render_panel(&options, &routes(api(), &options).unwrap(), &[], 143).into_string();
        assert!(html.contains("1 route ·\u{2009} 143 ms"));
        assert!(html.contains(r#"data-index="0" aria-pressed="true""#));
        assert!(html.contains("1 min 30 s") && html.contains("1.5 km") && html.contains("140"));
        assert!(html.contains("Toll") && html.contains("Border"));
        assert!(html.contains("Departs") && html.contains("2026-10-30 08:00 Europe/Stockholm"));
        assert!(html.contains("Drive east on &lt;Baltzarsgatan&gt;."));
        // The data for the map and the profile can't close its script element early
        let meta = html
            .split(r#"class="rt-meta" type="application/json">"#)
            .nth(1)
            .unwrap();
        let meta: serde_json::Value =
            serde_json::from_str(meta.strip_suffix("</script>").unwrap()).unwrap();
        let leg = &meta["routes"][0]["legs"][0];
        assert_eq!(leg["shape"], "_p~iF~ps|U");
        assert_eq!(meta["units"], "kilometers");
        assert_eq!(leg["summary"]["length"], 1.5);
        assert_eq!(leg["speeds"][0], json!([1.0, 110.0]));
        assert_eq!(leg["elevation"][0], json!([0.0, 10.0]));
        assert!(leg.get("maneuvers").is_none());
    }

    #[test]
    fn costs_come_from_the_trip_nodes() {
        let options = options(json!({
            "recostings": [{"costing": "auto", "name": "fast"}, {"costing": "bicycle", "name": "bike"}],
        }));
        let trip = &routes(api(), &options).unwrap()[0];
        let costs: Vec<f64> = trip.legs[0].maneuvers.iter().map(|m| m.1).collect();
        assert_eq!(costs, [60.0, 80.0]);
        assert_eq!(trip.summary.cost, 140.0);
        // Recostings without a cost are not traversable
        assert_eq!(
            trip.summary.recosts,
            [("fast".to_owned(), Some(100.0)), ("bike".to_owned(), None)]
        );
    }

    #[test]
    fn profile_follows_the_edges() {
        let km = options(json!({}));
        let profile = &routes(api(), &km).unwrap()[0].legs[0].profile;
        // Every sample as Valhalla sent it: the second edge starts with the first one's last
        // sample and has no data at its end
        assert_eq!(
            profile.elevation,
            [
                (0.0, 10.0),
                (0.5, 20.0),
                (1.0, 30.0),
                (1.0, 30.0),
                (1.5, -32768.0)
            ]
        );
        assert_eq!(profile.speeds, [(1.0, 110.0), (1.5, 30.0)]);

        // Positions in the request's units, values as Valhalla sent them
        let miles = options(json!({ "units": "miles" }));
        let profile = &routes(api(), &miles).unwrap()[0].legs[0].profile;
        assert_eq!(profile.elevation[1], (0.311, 20.0));
        assert_eq!(profile.speeds[0], (0.621, 110.0));
    }

    #[test]
    fn error_panel_shows_code_and_hint() {
        let err = Error {
            code: Some(171),
            ..Error::new(StatusCode::BAD_REQUEST, "No suitable edges near location")
        };
        let html = error_panel(&err, 12).into_string();
        assert!(html.contains("Failed ·\u{2009} 12 ms"));
        assert!(html.contains("(error_code 171)"));
        assert!(html.contains("too far from any road"));
        assert!(!html.contains("rt-meta"));
    }
}
