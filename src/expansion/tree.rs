//! `/expansion` events to one record per edge and tree. Reverse-tree `settled` events carry the
//! opposite edge, matched by reversed end points.

use std::collections::hash_map::Entry;

use rustc_hash::FxHashMap;
pub use valhalla::proto::expansion::EdgeStatus as Status;

/// Not settled.
pub const NONE: u32 = u32::MAX;

/// One event of the expansion stream, as Valhalla sent it.
#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    pub id: u64,
    pub status: Status,
    /// 0 forward tree, 1 reverse tree.
    pub dir: u8,
    /// lon, lat pairs in 1e-6 degrees, in the direction the record was sent.
    pub coords: &'a [i32],
}

/// One directed edge in one search tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    /// Range in [`Tree::points`].
    pub start: u32,
    pub len: u32,
    /// Event index when the edge was first reached.
    pub rseq: u32,
    /// Event index when the edge was settled (popped), [`NONE`] if never.
    pub seq: u32,
    pub dir: u8,
    /// Final status, see [`Status`].
    pub st: u8,
}

impl Edge {
    pub fn settled(&self) -> bool {
        self.seq != NONE
    }

    /// Settle order, or reach order for edges that were never settled.
    pub fn order(&self) -> u32 {
        if self.settled() { self.seq } else { self.rseq }
    }
}

#[derive(Default, Debug)]
pub struct Tree {
    pub edges: Vec<Edge>,
    /// lon, lat in 1e-6 degrees, edges in driving direction.
    pub points: Vec<[i32; 2]>,
    /// Number of input events.
    pub records: u32,
    pub algorithm: Algorithm,
}

type Ends = ([i32; 2], [i32; 2]);

fn ends(coords: &[i32]) -> Option<Ends> {
    let n = coords.len();
    (n >= 4).then(|| ([coords[0], coords[1]], [coords[n - 2], coords[n - 1]]))
}

/// Search algorithm, inferred from the tree directions in the event stream: the pbf doesn't carry it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Algorithm {
    /// Bidirectional A*: both trees (the default for routes without a time).
    #[default]
    Bidirectional,
    /// Unidirectional A* from the origin (depart-at).
    Forward,
    /// Unidirectional reverse A* from the destination (arrive-by).
    Reverse,
}

impl Algorithm {
    pub fn infer(records: &[Record]) -> Self {
        let fwd = records.iter().any(|r| r.dir == 0);
        let rev = records.iter().any(|r| r.dir != 0);
        match (fwd, rev) {
            (true, true) => Self::Bidirectional,
            (false, true) => Self::Reverse,
            _ => Self::Forward,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Bidirectional => "bidirectional_astar",
            Self::Forward => "forward_astar",
            Self::Reverse => "reverse_astar",
        }
    }
}

/// Builds the deduplicated trees.
pub fn build(records: &[Record]) -> Tree {
    // About half the events reach a new edge, half of those in the reverse tree
    let edges = records.len() / 2;
    let mut b = Builder {
        tree: Tree {
            algorithm: Algorithm::infer(records),
            edges: Vec::with_capacity(edges),
            ..Tree::default()
        },
        index: FxHashMap::with_capacity_and_hasher(edges, Default::default()),
        rev_ends: FxHashMap::with_capacity_and_hasher(edges / 2, Default::default()),
        rev_ends_more: FxHashMap::default(),
    };

    for (seq, r) in records.iter().enumerate() {
        let seq = seq as u32;
        b.tree.records = seq + 1;
        // An edge needs two points
        if r.coords.len() < 4 {
            continue;
        }
        let dir = r.dir.min(1);
        match r.status {
            Status::Reached => {
                if b.index.contains_key(&(r.id, dir)) {
                    continue;
                }
                let i = push(&mut b.tree, r, dir, seq, false);
                b.index.insert((r.id, dir), i);
                if dir == 1
                    && let Some(e) = ends(r.coords)
                {
                    match b.rev_ends.entry(e) {
                        Entry::Vacant(v) => _ = v.insert(i),
                        Entry::Occupied(_) => b.rev_ends_more.entry(e).or_default().push(i),
                    }
                }
            }
            Status::Settled => {
                b.settle(r, dir, seq);
            }
            Status::Connected if b.tree.algorithm != Algorithm::Bidirectional => {
                // Unidirectional A* pops the destination edge with this event instead of `settled`,
                // with the label id of a settled event.
                let i = b.settle(r, dir, seq);
                b.tree.edges[i as usize].st = Status::Connected as u8;
            }
            Status::Connected => {
                // Always the forward-tree edge in driving direction. The tree
                // that popped it (`dir`) settles it with this event, without a `settled` event.
                for d in 0..2u8 {
                    let Some(&i) = b.index.get(&(r.id, d)) else {
                        continue;
                    };
                    let e = &mut b.tree.edges[i as usize];
                    if d == dir && !e.settled() {
                        e.seq = seq;
                    }
                    e.st = Status::Connected as u8;
                }
            }
        }
    }
    b.tree
}

struct Builder {
    tree: Tree,
    index: FxHashMap<(u64, u8), u32>,
    /// Reverse reached edges by their (first, last) point, to match reverse settled events.
    rev_ends: FxHashMap<Ends, u32>,
    /// Other reverse reached edges with the same ends, a few hundred per search.
    rev_ends_more: FxHashMap<Ends, Vec<u32>>,
}

impl Builder {
    /// Applies a settled (or unidirectional connected) event; returns the edge index.
    fn settle(&mut self, r: &Record, dir: u8, seq: u32) -> u32 {
        let found = if dir == 0 {
            self.index.get(&(r.id, 0)).copied()
        } else {
            ends(r.coords).and_then(|(a, b)| {
                let first = self.rev_ends.get(&(b, a)).copied();
                let more = self
                    .rev_ends_more
                    .get(&(b, a))
                    .into_iter()
                    .flatten()
                    .copied();
                // The first edge with these ends not settled yet, the first one if all are
                let unsettled = |&i: &u32| !self.tree.edges[i as usize].settled();
                first.into_iter().chain(more).find(unsettled).or(first)
            })
        };
        match found {
            Some(i) => {
                let e = &mut self.tree.edges[i as usize];
                if !e.settled() {
                    e.seq = seq;
                    e.st = Status::Settled as u8;
                }
                i
            }
            // Without a reached record, settled from the start
            None => {
                let i = push(&mut self.tree, r, dir, seq, dir == 1);
                self.index.insert((r.id, dir), i);
                i
            }
        }
    }
}

fn push(tree: &mut Tree, r: &Record, dir: u8, seq: u32, reverse_geometry: bool) -> u32 {
    let start = tree.points.len() as u32;
    let pairs = r.coords.as_chunks::<2>().0.iter().copied();
    if reverse_geometry {
        tree.points.extend(pairs.rev());
    } else {
        tree.points.extend(pairs);
    }
    let settled = r.status != Status::Reached;
    tree.edges.push(Edge {
        start,
        len: tree.points.len() as u32 - start,
        rseq: seq,
        seq: if settled { seq } else { NONE },
        dir,
        st: r.status as u8,
    });
    tree.edges.len() as u32 - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u64, status: Status, dir: u8, coords: &[i32]) -> Record<'_> {
        Record {
            id,
            status,
            dir,
            coords,
        }
    }

    const A: &[i32] = &[0, 0, 10, 0];
    const A_REV: &[i32] = &[10, 0, 0, 0];
    const B: &[i32] = &[10, 0, 20, 0];
    const C: &[i32] = &[20, 0, 30, 0];
    const C_REV: &[i32] = &[30, 0, 20, 0];
    const D: &[i32] = &[30, 0, 40, 0];
    const D_REV: &[i32] = &[40, 0, 30, 0];

    #[test]
    fn events_without_a_line_are_counted_but_not_drawn() {
        let tree = build(&[
            rec(10, Status::Reached, 0, &[0, 0]),
            rec(20, Status::Reached, 0, B),
        ]);
        assert_eq!(tree.records, 2);
        assert_eq!(tree.edges.len(), 1);
        assert_eq!(tree.edges[0].len, 2);
    }

    /// Origin edge 10 (A), destination edge 40 (D), forward tree 10 -> 20 (B), reverse tree
    /// 40 -> 30 (C); reverse settled events use the odd opposite ids.
    fn stream() -> Vec<Record<'static>> {
        vec![
            rec(10, Status::Reached, 0, A),
            rec(40, Status::Reached, 1, D),
            rec(10, Status::Settled, 0, A),
            rec(20, Status::Reached, 0, B),
            rec(41, Status::Settled, 1, D_REV),
            rec(30, Status::Reached, 1, C),
            rec(20, Status::Settled, 0, B),
            rec(31, Status::Settled, 1, C_REV),
            rec(20, Status::Connected, 0, B),
            // duplicate reach (e.g. a second pass) is ignored
            rec(20, Status::Reached, 0, B),
        ]
    }

    #[test]
    fn reverse_edges_with_the_same_ends_settle_apart() {
        // Two ways between the same two points, reached and then settled by the reverse tree
        let tree = build(&[
            rec(30, Status::Reached, 1, C),
            rec(50, Status::Reached, 1, C),
            rec(31, Status::Settled, 1, C_REV),
            rec(51, Status::Settled, 1, C_REV),
        ]);
        let seqs: Vec<u32> = tree.edges.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [2, 3]);
    }

    #[test]
    fn dedupe_and_normalize() {
        let tree = build(&stream());
        assert_eq!(tree.records, 10);
        assert_eq!(
            tree.edges.len(),
            4,
            "reverse settled events matched their reached twins"
        );

        let [a, d, b, c] = [0, 1, 2, 3].map(|i| &tree.edges[i]);
        assert_eq!((a.dir, a.rseq, a.seq), (0, 0, 2));
        assert_eq!((d.dir, d.rseq, d.seq), (1, 1, 4));
        assert_eq!((b.rseq, b.seq), (3, 6));
        assert_eq!(b.st, Status::Connected as u8);
        // reverse settled 31 matched its reached twin 30
        assert_eq!((c.dir, c.rseq, c.seq), (1, 5, 7));
        assert_eq!(c.st, Status::Settled as u8);
        // geometry stays in driving direction
        assert_eq!(
            &tree.points[c.start as usize..(c.start + c.len) as usize],
            &[[20, 0], [30, 0]]
        );
    }

    #[test]
    fn unmatched_reverse_settled_gets_own_edge() {
        let records = vec![rec(11, Status::Settled, 1, A_REV)];
        let tree = build(&records);
        assert_eq!(tree.edges.len(), 1);
        assert_eq!(tree.edges[0].seq, 0);
        assert_eq!(
            &tree.points[..],
            &[[0, 0], [10, 0]],
            "reversed into driving direction"
        );
    }

    #[test]
    fn algorithm_from_directions() {
        assert_eq!(Algorithm::infer(&stream()), Algorithm::Bidirectional);
        let fwd = [rec(10, Status::Reached, 0, A)];
        assert_eq!(Algorithm::infer(&fwd), Algorithm::Forward);
        assert_eq!(Algorithm::infer(&[]), Algorithm::Forward);
        let rev = [rec(40, Status::Reached, 1, D)];
        assert_eq!(Algorithm::infer(&rev), Algorithm::Reverse);
    }

    /// Arrive-by (unidirectional reverse A*): every event is reverse and the final `connected`
    /// event carries the label id (opposite edge) of the origin edge.
    #[test]
    fn reverse_astar_connection() {
        let records = vec![
            rec(40, Status::Reached, 1, D),
            rec(41, Status::Settled, 1, D_REV),
            rec(30, Status::Reached, 1, C),
            rec(31, Status::Settled, 1, C_REV),
            rec(10, Status::Reached, 1, A),
            rec(11, Status::Connected, 1, A_REV),
        ];
        let tree = build(&records);
        assert_eq!(tree.algorithm, Algorithm::Reverse);
        assert_eq!(tree.edges.len(), 3);
        let a = &tree.edges[2];
        assert_eq!((a.dir, a.rseq, a.seq), (1, 4, 5));
        assert_eq!(a.st, Status::Connected as u8);
    }

    #[test]
    fn reverse_connection_settles_reverse_entry() {
        let records = vec![
            rec(30, Status::Reached, 0, C),
            rec(30, Status::Reached, 1, C),
            rec(30, Status::Connected, 1, C),
        ];
        let tree = build(&records);
        assert_eq!(tree.edges[0].seq, NONE, "forward entry was not popped");
        assert_eq!(tree.edges[0].st, Status::Connected as u8);
        assert_eq!(tree.edges[1].seq, 2);
    }
}
