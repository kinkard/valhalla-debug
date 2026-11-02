//! Vector tiles of expansion edges.

use std::cell::Cell;

use rustc_hash::FxHashMap;

use crate::{
    grid,
    mvt::{self, TileCoord},
};

use super::{Expansion, MAX_ZOOM, tree::Edge};

/// Lines smaller than this many tile units are dropped below [`DETAIL_ZOOM`]; edges too small alone
/// are joined into longer lines first, see [`chains`].
const MIN_UNITS: f64 = 8.0;
const DETAIL_ZOOM: u8 = 10;
/// Douglas-Peucker tolerance in tile units below the max zoom.
const SIMPLIFY_UNITS: f64 = 2.0;
pub const LAYER: &str = "edges";

pub struct Rendered {
    pub bytes: Vec<u8>,
    pub features: usize,
}

/// One tile, every edge and vertex kept at [`MAX_ZOOM`] for overzoom. Lines have `seq`, the event that
/// explored them, and `dir`, 0 forward and 1 reverse.
pub fn render(exp: &Expansion, tile: TileCoord) -> Rendered {
    let bbox = mvt::bounds(tile, mvt::BUFFER);
    let min_size = if tile.z() < DETAIL_ZOOM {
        MIN_UNITS * mvt::unit(tile.z())
    } else {
        0.0
    };
    let simplify = if tile.z() < MAX_ZOOM {
        SIMPLIFY_UNITS
    } else {
        0.0
    };
    let q = |seq: u32| (seq / exp.seq_step * exp.seq_step) as u64;

    let mut layer = mvt::Layer::new(LAYER);
    let mut features = 0;
    let mut geometry = mvt::Geometry::default();
    let mut add = |world: &[[f64; 2]], seq: u32, dir: u8| {
        if geometry.line(&tile, world, simplify) {
            features += 1;
            layer.add_line(None, &geometry, &[("seq", q(seq)), ("dir", dir as u64)]);
        }
    };
    let mut world = Vec::new();
    let mut small = Vec::new();
    for i in exp.grid.query(bbox) {
        if min_size > 0.0 && extent(exp.boxes[i as usize]) < min_size {
            small.push(i);
        } else {
            let e = &exp.edges[i as usize];
            exp.world_points(e, &mut world);
            add(&world, e.order(), e.dir);
        }
    }
    for chain in chains(exp, &small) {
        if extent(chain.bounds) >= min_size {
            chain.world_points(exp, &mut world);
            add(&world, chain.seq, chain.dir);
        }
    }
    Rendered {
        bytes: layer.encode(),
        features,
    }
}

/// Edges at each end node of a tree, CSR, for [`chains`].
pub struct Ends {
    /// Nodes at the first and last point of each edge.
    nodes: Vec<[u32; 2]>,
    offsets: Vec<u32>,
    edges: Vec<u32>,
}

impl Ends {
    /// End points of edges in the same tree are one node when exactly equal.
    pub fn build(edges: &[Edge], points: &[[f32; 2]]) -> Self {
        let mut ids: FxHashMap<(u8, [u32; 2]), u32> =
            FxHashMap::with_capacity_and_hasher(edges.len() * 2, Default::default());
        let mut node = |dir: u8, p: u32| {
            let next = ids.len() as u32;
            *ids.entry((dir, points[p as usize].map(f32::to_bits)))
                .or_insert(next)
        };
        let nodes: Vec<[u32; 2]> = (edges.iter())
            .map(|e| [node(e.dir, e.start), node(e.dir, e.start + e.len - 1)])
            .collect();
        let mut offsets = vec![0; ids.len() + 1];
        for &n in nodes.iter().flatten() {
            offsets[n as usize + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut fill = offsets.clone();
        let mut at = vec![0; nodes.len() * 2];
        for (i, ends) in nodes.iter().enumerate() {
            for &n in ends {
                at[fill[n as usize] as usize] = i as u32;
                fill[n as usize] += 1;
            }
        }
        Self {
            nodes,
            offsets,
            edges: at,
        }
    }

    /// Edges starting or ending at `node`, in edge order.
    fn at(&self, node: u32) -> &[u32] {
        let n = node as usize;
        &self.edges[self.offsets[n] as usize..self.offsets[n + 1] as usize]
    }
}

/// Edges of one tree joined end to end.
struct Chain {
    /// Edges in line order, `true` for one walked against its points.
    steps: Vec<(u32, bool)>,
    bounds: [f64; 4],
    /// The latest event of its edges: the chain shows once the search explored all of it.
    seq: u32,
    dir: u8,
}

impl Chain {
    fn world_points(&self, exp: &Expansion, out: &mut Vec<[f64; 2]>) {
        out.clear();
        let mut edge = Vec::new();
        for (i, &(k, back)) in self.steps.iter().enumerate() {
            exp.world_points(&exp.edges[k as usize], &mut edge);
            if back {
                edge.reverse();
            }
            // Joined edges share their end point
            out.extend(&edge[(i > 0) as usize..]);
        }
    }
}

/// Joins `edges` of one tree end to end, either direction, so roads of short edges stay visible.
/// Opposite edges of a road already in the chain are dropped, not folded back.
fn chains(exp: &Expansion, edges: &[u32]) -> Vec<Chain> {
    if edges.is_empty() {
        return Vec::new();
    }
    let nodes = &exp.ends.nodes;
    // Among `edges` and not joined yet; all false again once every edge is taken below
    let mut free = FREE.take();
    free.resize(free.len().max(exp.edges.len()), false);
    for &k in edges {
        free[k as usize] = true;
    }
    let mut out = Vec::new();
    for &first in edges {
        if !std::mem::take(&mut free[first as usize]) {
            continue;
        }
        let mut chain = Chain {
            steps: Vec::new(),
            bounds: exp.boxes[first as usize],
            seq: exp.edges[first as usize].order(),
            dir: exp.edges[first as usize].dir,
        };
        let [start, end] = nodes[first as usize];
        // Grows the tail, then the head: `forward` walks away from the first edge's end
        let mut tail = Vec::new();
        for forward in [true, false] {
            let (mut tip, mut came_from) = if forward { (end, start) } else { (start, end) };
            loop {
                let mut pick = None;
                for &k in exp.ends.at(tip) {
                    if !free[k as usize] {
                        continue;
                    }
                    let [a, b] = nodes[k as usize];
                    let other = if a == tip { b } else { a };
                    if other == came_from {
                        free[k as usize] = false; // the same road the other way
                    } else if pick.is_none() {
                        pick = Some((k, other));
                    }
                }
                let Some((k, other)) = pick else { break };
                free[k as usize] = false;
                chain.seq = chain.seq.max(exp.edges[k as usize].order());
                chain.bounds = grid::bounds(&[chain.bounds, exp.boxes[k as usize]]);
                // The tail runs away from `tip`, the head towards it
                let from_tip = nodes[k as usize][0] == tip;
                if forward {
                    tail.push((k, !from_tip));
                } else {
                    chain.steps.push((k, from_tip));
                }
                (came_from, tip) = (tip, other);
            }
        }
        chain.steps.reverse();
        chain.steps.push((first, false));
        chain.steps.extend(tail);
        out.push(chain);
    }
    debug_assert!(edges.iter().all(|&k| !free[k as usize]));
    FREE.set(free);
    out
}

thread_local! {
    /// [`chains`]' flags by edge, one per archive worker thread instead of one per tile.
    static FREE: Cell<Vec<bool>> = const { Cell::new(Vec::new()) };
}

/// The longer side of `[min_x, min_y, max_x, max_y]`.
fn extent(b: [f64; 4]) -> f64 {
    (b[2] - b[0]).max(b[3] - b[1])
}
