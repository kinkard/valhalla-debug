//! Minimal MVT encoder for line layers: lon/lat to world Web Mercator in [0, 1], then to tile
//! integer coordinates in [0, EXTENT) plus a buffer.

pub use pmtiles::TileCoord;
use prost::encoding::{WireType, bytes, encode_key, encode_varint, string, uint32, uint64};
use rustc_hash::FxHashMap;

/// MapLibre stores tile geometry on an 8192 grid whatever the extent, so this is the most precision it keeps.
pub const EXTENT: u32 = 8192;
/// Buffer around the tile in tile units, so line joins and caps across tile borders look right.
pub const BUFFER: f64 = 128.0;

pub const CONTENT_TYPE: &str = "application/vnd.mapbox-vector-tile";
/// `GeomType` of a line feature.
const LINESTRING: u64 = 2;

/// World `[min_x, min_y, max_x, max_y]` of a tile, expanded by `buffer` tile units on each side.
pub fn bounds(tile: TileCoord, buffer: f64) -> [f64; 4] {
    let n = (1u64 << tile.z()) as f64;
    let pad = buffer / EXTENT as f64;
    let (x, y) = (tile.x() as f64, tile.y() as f64);
    [
        (x - pad) / n,
        (y - pad) / n,
        (x + 1.0 + pad) / n,
        (y + 1.0 + pad) / n,
    ]
}

/// Size of one tile unit at zoom `z`, in world units.
pub fn unit(z: u8) -> f64 {
    1.0 / ((1u64 << z) as f64 * EXTENT as f64)
}

/// Projects lon/lat degrees to world Web Mercator coordinates in [0, 1].
pub fn project(lon: f64, lat: f64) -> [f64; 2] {
    let lat = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    let y = 0.5 - (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln() / std::f64::consts::TAU;
    [(lon + 180.0) / 360.0, y]
}

/// Inverse of [`project`], returns `(lon, lat)`.
pub fn unproject(p: [f64; 2]) -> (f64, f64) {
    let lon = p[0] * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * p[1]))
        .sinh()
        .atan()
        .to_degrees();
    (lon, lat)
}

/// MVT commands of one line clipped to a tile and quantized; reused across lines.
#[derive(Default)]
pub struct Geometry {
    commands: Vec<u32>,
    cursor: [i32; 2],
    local: Vec<[f64; 2]>,
    part: Vec<[i32; 2]>,
    keep: Vec<bool>,
    stack: Vec<(usize, usize)>,
}

impl Geometry {
    /// Clips `world` to the buffered `tile` and quantizes it. `simplify`: Douglas-Peucker tolerance
    /// in tile units, 0 for none. `false` if nothing is left.
    pub fn line(&mut self, tile: &TileCoord, world: &[[f64; 2]], simplify: f64) -> bool {
        self.commands.clear();
        self.cursor = [0, 0];
        if world.len() < 2 {
            return false;
        }
        let scale = 1.0 / unit(tile.z());
        let [x, y] = [tile.x(), tile.y()].map(|v| v as f64 * EXTENT as f64);
        self.local.clear();
        self.local
            .extend(world.iter().map(|p| [p[0] * scale - x, p[1] * scale - y]));
        if simplify > 0.0 {
            simplify_dp(&mut self.local, simplify, &mut self.keep, &mut self.stack);
        }

        let (lo, hi) = (-BUFFER, EXTENT as f64 + BUFFER);
        self.part.clear();
        for i in 1..self.local.len() {
            match clip_segment(self.local[i - 1], self.local[i], lo, hi) {
                Some((a, b, clipped_end)) => {
                    if self.part.is_empty() {
                        self.push_point(a);
                    }
                    self.push_point(b);
                    if clipped_end {
                        self.end_part();
                    }
                }
                None => self.end_part(),
            }
        }
        self.end_part();
        !self.commands.is_empty()
    }

    fn push_point(&mut self, p: [f64; 2]) {
        let p = [p[0].round() as i32, p[1].round() as i32];
        if self.part.last() != Some(&p) {
            self.part.push(p);
        }
    }

    /// Adds the current part, unless it shrank to a point.
    fn end_part(&mut self) {
        if self.part.len() >= 2 {
            self.commands.push(command(1, 1));
            self.push_delta(self.part[0]);
            self.commands.push(command(2, self.part.len() as u32 - 1));
            for i in 1..self.part.len() {
                self.push_delta(self.part[i]);
            }
        }
        self.part.clear();
    }

    fn push_delta(&mut self, p: [i32; 2]) {
        self.commands.push(zigzag(p[0] - self.cursor[0]));
        self.commands.push(zigzag(p[1] - self.cursor[1]));
        self.cursor = p;
    }
}

/// Liang-Barsky clipping of segment `a`-`b` against the square `[lo, hi]^2`.
/// Returns the clipped segment and whether its end was cut off (so the line leaves the box).
fn clip_segment(a: [f64; 2], b: [f64; 2], lo: f64, hi: f64) -> Option<([f64; 2], [f64; 2], bool)> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for axis in 0..2 {
        for (p, q) in [(-d[axis], a[axis] - lo), (d[axis], hi - a[axis])] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    if r > t1 {
                        return None;
                    }
                    t0 = t0.max(r);
                } else {
                    if r < t0 {
                        return None;
                    }
                    t1 = t1.min(r);
                }
            }
        }
    }
    let at = |t: f64| [a[0] + t * d[0], a[1] + t * d[1]];
    Some((at(t0), at(t1), t1 < 1.0))
}

/// Douglas-Peucker in place, keeping both ends. `keep` and `stack` are scratch.
fn simplify_dp(
    points: &mut Vec<[f64; 2]>,
    epsilon: f64,
    keep: &mut Vec<bool>,
    stack: &mut Vec<(usize, usize)>,
) {
    if points.len() <= 2 {
        return;
    }
    keep.clear();
    keep.resize(points.len(), false);
    keep[0] = true;
    keep[points.len() - 1] = true;
    stack.clear();
    stack.push((0, points.len() - 1));
    let eps2 = epsilon * epsilon;
    while let Some((first, last)) = stack.pop() {
        let (a, b) = (points[first], points[last]);
        let mut max = (0.0, 0);
        for (i, &p) in points.iter().enumerate().take(last).skip(first + 1) {
            let d = segment_distance2(p, a, b);
            if d > max.0 {
                max = (d, i);
            }
        }
        if max.0 > eps2 {
            keep[max.1] = true;
            stack.push((first, max.1));
            stack.push((max.1, last));
        }
    }
    let mut i = 0;
    points.retain(|_| {
        i += 1;
        keep[i - 1]
    });
}

/// Squared distance from `p` to segment `a`-`b`.
fn segment_distance2(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let mut v = [p[0] - a[0], p[1] - a[1]];
    let d = [b[0] - a[0], b[1] - a[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    if len2 > 0.0 {
        // Where `p` projects onto the segment: 0 at `a`, 1 at `b`
        let t = ((v[0] * d[0] + v[1] * d[1]) / len2).clamp(0.0, 1.0);
        v = [v[0] - t * d[0], v[1] - t * d[1]];
    }
    v[0] * v[0] + v[1] * v[1]
}

/// One layer with deduplicated keys and values, u64 values only. Features are encoded on add, in
/// prost's field order.
pub struct Layer {
    name: String,
    /// The layer's encoded `features` fields.
    features: Vec<u8>,
    keys: Vec<String>,
    values: FxHashMap<u64, u32>,
    value_list: Vec<u64>,
    tags: Vec<u32>,
}

impl Layer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            features: Vec::new(),
            keys: Vec::new(),
            values: FxHashMap::default(),
            value_list: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// Adds a (multi)line feature from [`Geometry::line`]. Does nothing for an empty geometry.
    /// MapLibre treats id 0 as "no id" in some code paths, so prefer ids starting from 1.
    pub fn add_line(&mut self, id: Option<u64>, geometry: &Geometry, props: &[(&str, u64)]) {
        if geometry.commands.is_empty() {
            return;
        }
        self.tags.clear();
        for &(key, value) in props {
            let k = match self.keys.iter().position(|k| k == key) {
                Some(k) => k as u32,
                None => {
                    self.keys.push(key.to_owned());
                    self.keys.len() as u32 - 1
                }
            };
            let next = self.value_list.len() as u32;
            let v = *self.values.entry(value).or_insert(next);
            if v == next {
                self.value_list.push(value);
            }
            self.tags.extend([k, v]);
        }

        let len = id.map_or(0, |id| uint64::encoded_len(1, &id))
            + uint32::encoded_len_packed(2, &self.tags)
            + uint64::encoded_len(3, &LINESTRING)
            + uint32::encoded_len_packed(4, &geometry.commands);
        let out = &mut self.features;
        encode_key(2, WireType::LengthDelimited, out);
        encode_varint(len as u64, out);
        if let Some(id) = id {
            uint64::encode(1, &id, out);
        }
        uint32::encode_packed(2, &self.tags, out);
        uint64::encode(3, &LINESTRING, out);
        uint32::encode_packed(4, &geometry.commands, out);
    }

    /// A tile of just this layer, kept even when empty so clients see a consistent schema.
    pub fn encode(&self) -> Vec<u8> {
        let mut layer = Vec::new();
        let out = &mut layer;
        string::encode(1, &self.name, out);
        out.extend_from_slice(&self.features);
        string::encode_repeated(3, &self.keys, out);
        for &v in &self.value_list {
            encode_key(4, WireType::LengthDelimited, out);
            encode_varint(uint64::encoded_len(5, &v) as u64, out);
            uint64::encode(5, &v, out);
        }
        uint32::encode(5, &EXTENT, out);
        uint32::encode(15, &2, out);
        let mut tile = Vec::new();
        bytes::encode(3, &layer, &mut tile);
        tile
    }
}

fn command(id: u32, count: u32) -> u32 {
    (id & 0x7) | (count << 3)
}

fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

/// The parts of https://github.com/mapbox/vector-tile-spec/blob/master/2.1/vector_tile.proto this
/// encoder writes, for tests to decode tiles.
#[cfg(test)]
pub mod pb {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Tile {
        #[prost(message, repeated, tag = "3")]
        pub layers: Vec<Layer>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Value {
        #[prost(uint64, optional, tag = "5")]
        pub uint_value: Option<u64>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Feature {
        #[prost(uint64, optional, tag = "1")]
        pub id: Option<u64>,
        #[prost(uint32, repeated, packed = "true", tag = "2")]
        pub tags: Vec<u32>,
        #[prost(uint64, optional, tag = "3")]
        pub r#type: Option<u64>,
        #[prost(uint32, repeated, packed = "true", tag = "4")]
        pub geometry: Vec<u32>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Layer {
        #[prost(uint32, required, tag = "15")]
        pub version: u32,
        #[prost(string, required, tag = "1")]
        pub name: String,
        #[prost(message, repeated, tag = "2")]
        pub features: Vec<Feature>,
        #[prost(string, repeated, tag = "3")]
        pub keys: Vec<String>,
        #[prost(message, repeated, tag = "4")]
        pub values: Vec<Value>,
        #[prost(uint32, optional, tag = "5")]
        pub extent: Option<u32>,
    }
}

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::*;

    #[test]
    fn project_roundtrip() {
        for (lon, lat) in [(0.0, 0.0), (13.15, 55.66), (-122.42, 37.77), (179.9, -60.0)] {
            let (lon2, lat2) = unproject(project(lon, lat));
            assert!((lon - lon2).abs() < 1e-9 && (lat - lat2).abs() < 1e-9);
        }
    }

    #[test]
    fn tile_bounds() {
        let tile = TileCoord::new(1, 1, 0).unwrap();
        assert_eq!(bounds(tile, 0.0), [0.5, 0.0, 1.0, 0.5]);
    }

    /// The parts of a line's commands, in tile units.
    fn parts(g: &Geometry) -> Vec<Vec<[i32; 2]>> {
        let unzig = |v: u32| ((v >> 1) as i32) ^ -((v & 1) as i32);
        let (mut out, mut cursor) = (Vec::new(), [0i32; 2]);
        let mut c = g.commands.iter().copied();
        while let Some(cmd) = c.next() {
            if cmd & 7 == 1 {
                out.push(Vec::new());
            }
            for _ in 0..cmd >> 3 {
                cursor = [
                    cursor[0] + unzig(c.next().unwrap()),
                    cursor[1] + unzig(c.next().unwrap()),
                ];
                out.last_mut().unwrap().push(cursor);
            }
        }
        out
    }

    /// A line through tile units of the z0 tile.
    fn geometry(local: &[[i32; 2]]) -> Geometry {
        let world: Vec<[f64; 2]> = (local.iter())
            .map(|p| [p[0] as f64 / 8192.0, p[1] as f64 / 8192.0])
            .collect();
        let mut g = Geometry::default();
        assert!(g.line(&TileCoord::new(0, 0, 0).unwrap(), &world, 0.0));
        g
    }

    #[test]
    fn clip_line_crossing_tile() {
        let tile = TileCoord::new(1, 0, 0).unwrap();
        let mut g = Geometry::default();
        // Horizontal line across the whole world at y=0.25 (middle of the top-left z1 tile)
        assert!(g.line(&tile, &[[0.0, 0.25], [1.0, 0.25]], 0.0));
        assert_eq!(parts(&g), vec![vec![[0, 4096], [8320, 4096]]]);
        // Fully outside, after a line that wasn't
        assert!(!g.line(&tile, &[[0.6, 0.6], [0.9, 0.9]], 0.0));
        assert!(parts(&g).is_empty());
    }

    #[test]
    fn clip_line_leaving_and_reentering() {
        let tile = TileCoord::new(0, 0, 0).unwrap();
        let p = |x: f64, y: f64| [x / 8192.0, y / 8192.0];
        let line = [p(100., 100.), p(100., -200.), p(200., -200.), p(200., 100.)];
        let mut g = geometry(&[[7, 7], [9, 9]]);
        assert!(g.line(&tile, &line, 0.0));
        assert_eq!(
            parts(&g),
            vec![vec![[100, 100], [100, -128]], vec![[200, -128], [200, 100]]]
        );
    }

    #[test]
    fn segment_distance() {
        let (a, b) = ([0.0, 0.0], [0.0, 2.0]);
        for (p, d2) in [
            // On the segment: its ends and between them
            ([0.0, 0.0], 0.0),
            ([0.0, 1.0], 0.0),
            ([0.0, 2.0], 0.0),
            // Beside it, before its start and past its end
            ([3.0, 1.0], 9.0),
            ([0.0, -3.0], 9.0),
            ([4.0, 5.0], 25.0),
        ] {
            assert_eq!(segment_distance2(p, a, b), d2, "{p:?}");
            assert_eq!(segment_distance2(p, b, a), d2, "{p:?} reversed");
        }
        // A diagonal segment projects onto its middle, a zero-length one is a point
        assert_eq!(segment_distance2([2.0, 0.0], [0.0, 0.0], [2.0, 2.0]), 2.0);
        assert_eq!(segment_distance2([3.0, 4.0], [0.0, 0.0], [0.0, 0.0]), 25.0);
    }

    #[test]
    fn simplify_keeps_corners() {
        let mut pts = vec![[0.0, 0.0], [1.0, 0.1], [2.0, 0.0], [2.0, 5.0]];
        simplify_dp(&mut pts, 0.5, &mut Vec::new(), &mut Vec::new());
        assert_eq!(pts, vec![[0.0, 0.0], [2.0, 0.0], [2.0, 5.0]]);
    }

    #[test]
    fn encode_decode() {
        let mut layer = Layer::new("edges");
        layer.add_line(
            Some(1),
            &geometry(&[[0, 0], [10, 10], [20, 0]]),
            &[("speed", 50), ("lvl", 7)],
        );
        layer.add_line(Some(2), &geometry(&[[5, 5], [6, 6]]), &[("speed", 50)]);
        layer.add_line(None, &geometry(&[[7, 8], [9, 9]]), &[("cost", 1500)]);
        let bytes = layer.encode();

        let tile = pb::Tile::decode(bytes.as_slice()).unwrap();
        assert_eq!(tile.encode_to_vec(), bytes, "encoded as prost would");
        assert_eq!(tile.layers.len(), 1);
        let l = &tile.layers[0];
        assert_eq!(l.name, "edges");
        assert_eq!(l.version, 2);
        assert_eq!(l.extent, Some(EXTENT));
        assert_eq!(l.keys, vec!["speed", "lvl", "cost"]);
        assert_eq!(l.values.len(), 3, "speed=50 must be deduplicated");
        assert_eq!(l.values[2].uint_value, Some(1500));
        assert_eq!(l.features[0].id, Some(1));
        assert_eq!(l.features[0].r#type, Some(LINESTRING));
        assert_eq!(l.features[0].geometry, vec![9, 0, 0, 18, 20, 20, 20, 19]);
        assert_eq!(l.features[1].tags, vec![0, 0]);
        assert_eq!(l.features[2].id, None);
        assert_eq!(l.features[2].geometry, vec![9, 14, 16, 10, 4, 2]);
    }
}
