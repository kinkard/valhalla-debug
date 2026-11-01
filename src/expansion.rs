//! Search expansion: Valhalla `/expansion` events to one PMTiles archive of the explored edges.
//! Tiles, as GeoJSON of Malmö -> Stockholm is ~25 MB.

mod archive;
mod tile;
mod tree;

use std::time::Instant;

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use prost::Message;
use tracing::info;
use valhalla::proto::{self, options, options::ExpansionProperties};

use crate::{
    AppState,
    grid::{self, Grid},
    mvt,
    request::{self, Spec},
    valhalla::Error,
};
use tree::{Algorithm, Edge, Record, Status};

/// Highest zoom with real tiles, MapLibre overzooms above. Picked by eye at street zoom: z10-z11 lines visibly
/// drift from the road at junctions, z13 is barely different from z12 for ~35 % more bytes.
pub const MAX_ZOOM: u8 = 12;
/// Grid cells along the longer side of the expansion bounds.
const GRID_CELLS: f64 = 256.0;
/// Tile `seq` is quantized to this many steps so the MVT value tables stay small.
const SEQ_STEPS: u32 = 16_384;

/// Cost and duration are left out: they don't match the route summary.
const PROPERTIES: [ExpansionProperties; 3] = [
    ExpansionProperties::EdgeStatus,
    ExpansionProperties::EdgeId,
    ExpansionProperties::ExpansionType,
];

/// The edges one search explored, alive while their archive is built.
pub struct Expansion {
    pub edges: Vec<Edge>,
    /// World (Web Mercator, [0, 1]) coordinates relative to `origin`.
    points: Vec<[f32; 2]>,
    origin: [f64; 2],
    /// World bounds of each edge, `[min_x, min_y, max_x, max_y]`.
    boxes: Vec<[f64; 4]>,
    /// World bounds of all edges.
    bounds: [f64; 4],
    grid: Grid,
    ends: tile::Ends,
    /// `[w, s, e, n]` in degrees.
    pub bbox: [f64; 4],
    pub records: u32,
    seq_step: u32,
    pub algorithm: Algorithm,
    pub costing: String,
}

impl Expansion {
    fn new(tree: tree::Tree, costing: String) -> Self {
        let origin = tree.points.first().map_or([0.0, 0.0], |p| project(*p));
        let points: Vec<[f32; 2]> = tree
            .points
            .iter()
            .map(|&p| {
                let w = project(p);
                [(w[0] - origin[0]) as f32, (w[1] - origin[1]) as f32]
            })
            .collect();
        // Reached-only edges stayed in the queue: the search never explored them
        let edges: Vec<Edge> = (tree.edges.into_iter())
            .filter(|e| e.st != Status::Reached as u8)
            .collect();
        let boxes: Vec<[f64; 4]> = (edges.iter())
            .map(|e| {
                let pts = &points[e.start as usize..(e.start + e.len) as usize];
                pts.iter()
                    .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
                        let (x, y) = (origin[0] + p[0] as f64, origin[1] + p[1] as f64);
                        [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
                    })
            })
            .collect();
        let bounds = grid::bounds(&boxes);
        let (w, n) = mvt::unproject([bounds[0], bounds[1]]);
        let (e, s) = mvt::unproject([bounds[2], bounds[3]]);
        Self {
            grid: Grid::build(&boxes, bounds, GRID_CELLS),
            ends: tile::Ends::build(&edges, &points),
            bbox: [w, s, e, n].map(|v| (v * 1e6).round() / 1e6),
            records: tree.records,
            seq_step: tree.records.div_ceil(SEQ_STEPS).max(1),
            algorithm: tree.algorithm,
            edges,
            points,
            origin,
            boxes,
            bounds,
            costing,
        }
    }

    fn world_points(&self, e: &Edge, out: &mut Vec<[f64; 2]>) {
        out.clear();
        let pts = &self.points[e.start as usize..(e.start + e.len) as usize];
        out.extend(
            pts.iter()
                .map(|p| [self.origin[0] + p[0] as f64, self.origin[1] + p[1] as f64]),
        );
    }
}

fn project(p: [i32; 2]) -> [f64; 2] {
    mvt::project(p[0] as f64 * 1e-6, p[1] as f64 * 1e-6)
}

fn bad_gateway(message: String) -> Error {
    Error::new(StatusCode::BAD_GATEWAY, message)
}

fn internal(err: impl std::fmt::Display) -> Error {
    Error::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

/// `POST /api/expansion`: body is the page's [`Spec`] of a route. Answers the PMTiles archive, see
/// [`archive`].
pub async fn run(State(state): State<AppState>, Json(spec): Json<Spec>) -> Result<Response, Error> {
    let (options, _) = request::options(&spec, options::Action::Route)
        .map_err(|m| Error::new(StatusCode::BAD_REQUEST, m))?;
    // `dedupe` and `skip_opposites` stay false: the trees are deduplicated here, see `tree`
    let api = proto::Api {
        options: Some(proto::Options {
            has_expansion_action: Some(options::HasExpansionAction::ExpansionAction(
                options::Action::Route as i32,
            )),
            format: options::Format::Pbf as i32,
            expansion_properties: PROPERTIES.map(|p| p as i32).to_vec(),
            ..options
        }),
        ..Default::default()
    };
    let (costing, threads) = (spec.costing, state.threads);
    let begin = Instant::now();
    let pbf = state.post("expansion", &api).await?;
    let valhalla_ms = begin.elapsed().as_millis() as u64;

    let (exp, decode_ms, dedupe_ms) = tokio::task::spawn_blocking(move || {
        let t = Instant::now();
        let api = proto::Api::decode(pbf.as_ref())
            .map_err(|e| bad_gateway(format!("Bad expansion pbf: {e}")))?;
        drop(pbf); // expansion response is huge, drop it once not needed
        let expansion = api
            .expansion
            .ok_or_else(|| bad_gateway("Valhalla returned no expansion".to_owned()))?;
        let records = records(&expansion).map_err(bad_gateway)?;
        let decode_ms = t.elapsed().as_millis() as u64;
        let t = Instant::now();
        let tree = tree::build(&records);
        drop(records);
        drop(expansion);
        let exp = Expansion::new(tree, costing);
        Ok::<_, Error>((exp, decode_ms, t.elapsed().as_millis() as u64))
    })
    .await
    .map_err(internal)??;
    let bytes = tokio::task::spawn_blocking(move || {
        let t = Instant::now();
        let bytes = archive::build(&exp, threads).map_err(internal)?;
        info!(
            "Expansion: {} events -> {} edges, valhalla {valhalla_ms} ms, proto_decode {decode_ms} ms, dedupe {dedupe_ms} ms, archive {} ms, {:.1} MB",
            exp.records,
            exp.edges.len(),
            t.elapsed().as_millis(),
            bytes.len() as f64 / 1e6
        );
        Ok::<_, Error>(bytes)
    })
    .await
    .map_err(internal)??;
    Ok((
        [
            (header::CONTENT_TYPE, archive::CONTENT_TYPE),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}

fn records(e: &proto::Expansion) -> Result<Vec<Record<'_>>, String> {
    let n = e.geometries.len();
    let lens = [e.edge_status.len(), e.edge_id.len(), e.expansion_type.len()];
    if lens.iter().any(|&l| l != n) {
        return Err(format!(
            "Expansion arrays have different lengths ({n} geometries, {lens:?} properties); is this Valhalla too old?"
        ));
    }
    (0..n)
        .map(|i| {
            Ok(Record {
                id: e.edge_id[i],
                status: Status::try_from(e.edge_status[i])
                    .map_err(|_| format!("Unknown edge status {}", e.edge_status[i]))?,
                dir: (e.expansion_type[i] == 1) as u8,
                coords: &e.geometries[i].coords,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_validate_lengths() {
        let e = proto::Expansion {
            edge_id: vec![1],
            geometries: vec![Default::default()],
            ..Default::default()
        };
        assert!(records(&e).is_err());
    }

    #[test]
    fn reached_only_edges_are_left_out() {
        let (a, b) = ([0, 0, 10, 0], [10, 0, 20, 0]);
        let records = [
            (2, Status::Reached, &a),
            (2, Status::Settled, &a),
            (4, Status::Reached, &b),
        ]
        .map(|(id, status, coords)| Record {
            id,
            status,
            dir: 0,
            coords,
        });
        let exp = Expansion::new(tree::build(&records), "auto".into());
        assert_eq!(exp.edges.len(), 1);
    }
}
