//! `GET /tiles/speeds/{z}/{x}/{y}?t=`: drivable edges with km/h `sp`, `lim`, `ff`, `cf`, `live`, and `p`
//! at `t` seconds into the local week, and `ow` on one-way edges.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use tracing::info;
use valhalla::{
    Access, DirectedEdge, EdgeUse, GraphReader, GraphTile, LatLon, NodeInfo, SpeedSources, TileId,
    level,
};

use crate::{
    AppState,
    grid::{self, Grid},
    mvt,
};

/// Tiles below this zoom are empty: even the highway level is too much data to be useful there.
const MIN_ZOOM: u8 = 6;
/// The page overzooms tiles above this zoom.
const MAX_ZOOM: u8 = 14;
const SECONDS_PER_WEEK: u32 = 604_800;
const ACCESS: Access = Access::AUTO.union(Access::TRUCK);
#[derive(Deserialize)]
pub struct TileQuery {
    t: Option<u32>,
}

pub async fn tile(
    State(state): State<AppState>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    Query(query): Query<TileQuery>,
) -> Response {
    let not_found = || StatusCode::NOT_FOUND.into_response();
    let Some(reader) = state.graph_reader.clone() else {
        return not_found();
    };
    let Ok(coord) = mvt::TileCoord::new(z, x, y) else {
        return not_found();
    };
    if z > MAX_ZOOM {
        return not_found();
    }
    if query.t.is_some_and(|t| t >= SECONDS_PER_WEEK) {
        let error = format!("t must be a second of the week, 0..{SECONDS_PER_WEEK}");
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    }

    let begin = Instant::now();
    let result = tokio::task::spawn_blocking(move || build_tile(&reader, coord, query.t)).await;
    let Ok((bytes, features, predicted)) = result else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    info!(
        "speeds {z}/{x}/{y} t={:?}: {features} edges, {predicted} predicted, {} KiB in {} ms",
        query.t,
        bytes.len() / 1024,
        begin.elapsed().as_millis()
    );
    (
        [
            (header::CONTENT_TYPE, mvt::CONTENT_TYPE),
            // Live traffic may change, everything else is fixed for the lifetime of the server
            (header::CACHE_CONTROL, "max-age=60"),
        ],
        bytes,
    )
        .into_response()
}

/// Which hierarchy levels and road classes are worth drawing at a zoom.
/// Returns `(levels, max_road_class)`; road classes go from 0 (motorway) to 7 (service/other).
fn zoom_filter(z: u8) -> (&'static [u8], u8) {
    match z {
        0..MIN_ZOOM => (&[], 0),
        // Highway level is motorway..primary, but primaries fill a whole z6-7 tile with noise
        6..=7 => (&[level::HIGHWAY], 1),
        // Tertiaries bloat dense city tiles
        8..=9 => (&[level::HIGHWAY, level::ARTERIAL], 3),
        10..=11 => (&[level::HIGHWAY, level::ARTERIAL], 4),
        12 => (&[level::HIGHWAY, level::ARTERIAL, level::LOCAL], 5),
        13 => (&[level::HIGHWAY, level::ARTERIAL, level::LOCAL], 6),
        _ => (&[level::HIGHWAY, level::ARTERIAL, level::LOCAL], 7),
    }
}

/// Meters around the tile to find edges crossing it: about half the longest edges of the level.
fn level_margin(level: u8) -> f64 {
    match level {
        level::HIGHWAY | level::ARTERIAL => 15_000.0,
        _ => 5_000.0,
    }
}

/// A node in world coordinates, and world units per meter there.
fn node_world(tile: &GraphTile, node: &NodeInfo) -> ([f64; 2], f64) {
    let ll = tile.node_latlon(node);
    let world_per_meter = 1.0 / (40_075_016.686 * ll.0.to_radians().cos());
    (mvt::project(ll.1, ll.0), world_per_meter)
}

/// Grid cells along a graph tile: ~4 km at the highway level, ~250 m at the local one.
const GRID_CELLS: f64 = 64.0;

/// Drivable edges of a graph tile as `[edge index, node index]`, and a grid over their reach.
// TODO: explore Valhalla's own bins
struct TileEdges {
    edges: Vec<[u32; 2]>,
    grid: Grid,
}

impl TileEdges {
    fn new(tile: &GraphTile) -> Self {
        let (mut edges, mut boxes) = (Vec::new(), Vec::new());
        for (n, node) in tile.nodes().iter().enumerate() {
            let (start, world_per_meter) = node_world(tile, node);
            let first = node.edge_index();
            for (i, de) in tile.node_edges(node).iter().enumerate() {
                if drivable(de) {
                    let r = de.length() as f64 * world_per_meter;
                    edges.push([first + i as u32, n as u32]);
                    boxes.push([start[0] - r, start[1] - r, start[0] + r, start[1] + r]);
                }
            }
        }
        let grid = Grid::build(&boxes, grid::bounds(&boxes), GRID_CELLS);
        Self { edges, grid }
    }
}

/// The tile, its number of features and of those with a predicted speed at `t`.
fn build_tile(
    reader: &GraphReader,
    coord: mvt::TileCoord,
    t: Option<u32>,
) -> (Vec<u8>, usize, usize) {
    let mut layer = mvt::Layer::new("speeds");
    let (mut features, mut predicted) = (0, 0);
    let (levels, max_rc) = zoom_filter(coord.z());
    let [x0, y0, x1, y1] = mvt::bounds(coord, mvt::BUFFER);
    let (min_lon, max_lat) = mvt::unproject([x0.max(0.0), y0.max(0.0)]);
    let (max_lon, min_lat) = mvt::unproject([x1.min(1.0), y1.min(1.0)]);
    // Simplify to half a pixel (512 px tiles), except at the max zoom that gets overzoomed
    let simplify = if coord.z() == MAX_ZOOM { 2.0 } else { 8.0 };

    let mut shape = Vec::new();
    let mut geometry = mvt::Geometry::default();
    for &level in levels {
        let m = level_margin(level) / 111_320.0;
        let m_lon = m / min_lat.abs().max(max_lat.abs()).to_radians().cos();
        let (sw, ne) = (
            LatLon(min_lat - m, min_lon - m_lon),
            LatLon(max_lat + m, max_lon + m_lon),
        );
        for tile_id in reader.tiles_in_bbox(sw, ne, level) {
            let Some(tile) = reader.graph_tile(tile_id) else {
                continue;
            };
            let entry = cached(&tile);
            let edges = entry.get().expect("cached builds it");
            let (nodes, directed) = (tile.nodes(), tile.directededges());
            for i in edges.grid.query([x0, y0, x1, y1]) {
                let [index, node] = edges.edges[i as usize];
                let de = &directed[index as usize];
                // An edge never strays from its start node further than its length
                let ([x, y], world_per_meter) = node_world(&tile, &nodes[node as usize]);
                let r = de.length() as f64 * world_per_meter;
                if de.road_class().repr > max_rc
                    || x + r < x0
                    || x - r > x1
                    || y + r < y0
                    || y - r > y1
                {
                    continue;
                }
                edge_shape(&tile, de, &mut shape);
                if !geometry.line(&coord, &shape, simplify) {
                    continue;
                }
                let props = edge_props(&tile, de, t);
                predicted += props.iter().any(|(k, _)| *k == "p") as usize;
                let id = tile_id.graph_id(index).map(|id| id.value);
                layer.add_line(id, &geometry, &props);
                features += 1;
            }
        }
    }
    (layer.encode(), features, predicted)
}

fn drivable(de: &DirectedEdge) -> bool {
    !de.is_shortcut()
        && de.forwardaccess().intersects(ACCESS)
        && !matches!(
            de.use_type(),
            EdgeUse::kFerry
                | EdgeUse::kRailFerry
                | EdgeUse::kConstruction
                | EdgeUse::kRail
                | EdgeUse::kBus
                | EdgeUse::kEgressConnection
                | EdgeUse::kPlatformConnection
                | EdgeUse::kTransitConnection
        )
}

/// Edge shape in world coordinates, in the direction of travel.
fn edge_shape(tile: &GraphTile, de: &DirectedEdge, out: &mut Vec<[f64; 2]>) {
    out.clear();
    out.extend(tile.edgeinfo(de).shape().map(|ll| mvt::project(ll.1, ll.0)));
    if !de.forward() {
        out.reverse();
    }
}

fn edge_props<'a>(tile: &GraphTile, de: &DirectedEdge, t: Option<u32>) -> Vec<(&'a str, u64)> {
    let u = |v: u32| v as u64;
    let mut props = vec![("sp", u(de.speed()))];
    // No drawn edge back, so the page doesn't offset it to make room for one
    if !de.reverseaccess().intersects(ACCESS) {
        props.push(("ow", 1));
    }
    // Valhalla stores 0 for an unknown limit and 255 for "unlimited"; neither makes a ratio
    let limit = tile.edgeinfo(de).speed_limit;
    if (1..255).contains(&limit) {
        props.push(("lim", u(limit as u32)));
    }
    if de.free_flow_speed() > 0 {
        props.push(("ff", u(de.free_flow_speed())));
    }
    if de.constrained_flow_speed() > 0 {
        props.push(("cf", u(de.constrained_flow_speed())));
    }
    if let Some(live) = tile.live_traffic(de).speed() {
        props.push(("live", u(live as u32)));
    }
    if let Some(t) = t {
        let (p, sources) = tile.edge_speed(de, SpeedSources::PREDICTED_FLOW, false, t as u64, 0);
        if sources.contains(SpeedSources::PREDICTED_FLOW) {
            props.push(("p", u(p)));
        }
    }
    props
}
const CACHED_TILES: usize = 32;
/// Edge indexes of graph tiles, most recently used first.
static CACHE: Mutex<VecDeque<(TileId, Arc<OnceLock<TileEdges>>)>> = Mutex::new(VecDeque::new());

/// Built outside the cache lock by the first request, the other requests for the tile wait for it.
fn cached(tile: &GraphTile) -> Arc<OnceLock<TileEdges>> {
    let entry = {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let entry = match cache.iter().position(|(t, _)| *t == tile.id()) {
            Some(pos) => cache.remove(pos).expect("position is in range").1,
            None => Arc::default(),
        };
        cache.push_front((tile.id(), entry.clone()));
        cache.truncate(CACHED_TILES);
        entry
    };
    entry.get_or_init(|| TileEdges::new(tile));
    entry
}
