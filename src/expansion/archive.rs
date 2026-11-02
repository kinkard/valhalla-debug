//! The tile pyramid of an expansion in one PMTiles v3 archive, stats in its metadata. pmtiles.io and
//! QGIS draw nothing at the levels it skips, see [`LEVELS`].

use std::{
    io::Cursor,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

use pmtiles::{Compression, PmTilesWriter, TileId, TileType};
use serde_json::json;
use zlib_rs::{Deflate, DeflateConfig, DeflateFlush, Status, compress_bound};

use super::{Expansion, LEVELS, MAX_ZOOM, tile};
use crate::mvt::{self, TileCoord};

pub const CONTENT_TYPE: &str = "application/vnd.pmtiles";
/// Faster than the default 6 for slightly bigger tiles.
const GZIP_LEVEL: i32 = 3;

/// Gzipped tiles with features at [`LEVELS`] in tile id order, rendered on `threads`. Lowest
/// levels first: their few tiles are the slowest.
pub fn pyramid(exp: &Expansion, threads: usize) -> Vec<(TileCoord, Vec<u8>)> {
    let candidates: Vec<TileCoord> = (LEVELS.into_iter()).flat_map(|z| reached(exp, z)).collect();
    let next = AtomicUsize::new(0);
    let mut tiles: Vec<_> = thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let (mut gzip, mut done) = (Gzip::new(), Vec::new());
                    while let Some(&c) = candidates.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let t = tile::render(exp, c);
                        if t.features > 0 {
                            done.push((c, gzip.compress(&t.bytes)));
                        }
                    }
                    done
                })
            })
            .collect();
        (workers.into_iter())
            .flat_map(|w| {
                w.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    tiles.sort_by_key(|(c, _)| TileId::from(*c));
    tiles
}

/// Tiles at `z` whose buffered bounds some edge's bounds touch: only these can have features.
fn reached(exp: &Expansion, z: u8) -> Vec<TileCoord> {
    let n = (1u64 << z) as f64;
    let pad = mvt::BUFFER / mvt::EXTENT as f64 / n;
    let at = |v: f64| ((v * n).floor().max(0.0) as u32).min((1 << z) - 1);
    let range = |b: &[f64; 4]| {
        [
            at(b[0] - pad),
            at(b[1] - pad),
            at(b[2] + pad),
            at(b[3] + pad),
        ]
    };
    // Row-major flags: tile order without sorting
    let [x0, y0, x1, y1] = range(&exp.bounds);
    let width = (x1 - x0 + 1) as usize;
    let mut hit = vec![false; width * (y1 - y0 + 1) as usize];
    for b in &exp.boxes {
        let [bx0, by0, bx1, by1] = range(b);
        for y in by0..=by1 {
            let row = (y - y0) as usize * width;
            hit[row + (bx0 - x0) as usize..=row + (bx1 - x0) as usize].fill(true);
        }
    }
    (hit.iter().enumerate())
        .filter(|(_, hit)| **hit)
        .filter_map(|(i, _)| {
            TileCoord::new(z, x0 + (i % width) as u32, y0 + (i / width) as u32).ok()
        })
        .collect()
}

/// Builds the archive, with what the page shows about the expansion in its metadata.
pub fn build(exp: &Expansion, threads: usize) -> Result<Vec<u8>, String> {
    let tiles = pyramid(exp, threads);
    let tile_bytes: usize = tiles.iter().map(|(_, b)| b.len()).sum();

    let [w, s, e, n] = exp.bbox;
    let metadata = json!({
        "name": "Valhalla expansion",
        "description": format!("{} {} search trees from valhalla-debug", exp.costing, exp.algorithm.name()),
        "generator": "valhalla-debug",
        "vector_layers": [{
            "id": tile::LAYER,
            "minzoom": 0,
            "maxzoom": MAX_ZOOM,
            "fields": {
                "seq": "Number: search event at which the edge settled (reached for a connection the tree didn't pop)",
                "dir": "Number: 0 forward tree, 1 reverse tree",
            },
        }],
        "valhalla_debug": {
            "algorithm": exp.algorithm.name(),
            "costing": exp.costing,
            "bbox": exp.bbox,
            "records": exp.records,
            "edges": exp.edges.len(),
            "levels": LEVELS,
        },
    });

    let mut buf = Cursor::new(Vec::with_capacity(tile_bytes + (1 << 16)));
    let center = [(w + e) / 2.0, (s + n) / 2.0];
    let mut writer = PmTilesWriter::new(TileType::Mvt)
        .precompressed(Compression::Gzip)
        // Gzip costs more time than it saves bytes here
        .internal_compression(Compression::None)
        .min_zoom(0)
        .max_zoom(MAX_ZOOM)
        .bounds(w, s, e, n)
        .center(center[0], center[1])
        .center_zoom(center_zoom(exp.bounds))
        .metadata(metadata.to_string())
        .create(&mut buf)
        .map_err(|e| e.to_string())?;
    for (c, bytes) in &tiles {
        writer.add_tile(*c, bytes).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}

/// Highest of [`LEVELS`] where world `bounds` fit one tile: a viewer opening the file lands on tiles.
fn center_zoom([x0, y0, x1, y1]: [f64; 4]) -> u8 {
    let fit = -(x1 - x0).max(y1 - y0).max(1e-9).log2();
    LEVELS.into_iter().rfind(|&l| l as f64 <= fit).unwrap_or(0)
}

/// One deflate state and output buffer per worker: a state allocates its window and hash tables.
struct Gzip {
    deflate: Deflate,
    out: Vec<u8>,
}

impl Gzip {
    fn new() -> Self {
        // Window bits 16 + 15: gzip's header and trailer around the deflate stream
        let config = DeflateConfig {
            level: GZIP_LEVEL,
            window_bits: 31,
            ..DeflateConfig::default()
        };
        let deflate = Deflate::new_with_config(config);
        Self {
            deflate,
            out: Vec::new(),
        }
    }

    fn compress(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.deflate.reset();
        // Room for the whole stream: gzip's wrapper is 12 bytes longer than zlib's
        self.out.resize(compress_bound(bytes.len()) + 12, 0);
        let status = (self
            .deflate
            .compress(bytes, &mut self.out, DeflateFlush::Finish))
        .expect("deflate into a big enough buffer doesn't fail");
        debug_assert_eq!(status, Status::StreamEnd);
        self.out[..self.deflate.total_out() as usize].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::*;
    use crate::expansion::tree::{self, Record, Status};

    fn gunzip(b: &[u8], len: usize) -> Vec<u8> {
        let mut out = vec![0; len];
        let config = zlib_rs::InflateConfig { window_bits: 31 };
        let (raw, rc) = zlib_rs::decompress_slice(&mut out, b, config);
        assert_eq!(rc, zlib_rs::ReturnCode::Ok);
        raw.to_vec()
    }

    /// Expansion with one settled forward edge per line (lon, lat pairs in 1e-6 degrees).
    fn fixture(lines: &[&[i32]]) -> Expansion {
        let records: Vec<Record> = lines
            .iter()
            .enumerate()
            .flat_map(|(i, coords)| {
                [Status::Reached, Status::Settled].map(|status| Record {
                    id: 2 + i as u64 * 8,
                    status,
                    dir: 0,
                    coords,
                })
            })
            .collect();
        Expansion::new(tree::build(&records), "auto".into())
    }

    /// Malmo: a 630 m edge, a 31 m one (3 tile units at z9) and one with a vertex 0.9 units off
    /// the line at z12.
    const LONG: [i32; 4] = [13_000_000, 55_600_000, 13_010_000, 55_600_000];
    const SHORT: [i32; 4] = [13_020_000, 55_600_000, 13_020_500, 55_600_000];
    const BENT: [i32; 6] = [
        13_030_000, 55_600_000, 13_030_100, 55_600_011, 13_030_200, 55_600_000,
    ];

    fn tile_at(z: u8, lonlat: [i32; 2]) -> TileCoord {
        let w = mvt::project(lonlat[0] as f64 * 1e-6, lonlat[1] as f64 * 1e-6);
        let n = (1u32 << z) as f64;
        TileCoord::new(z, (w[0] * n) as u32, (w[1] * n) as u32).unwrap()
    }

    fn vertices(t: &tile::Rendered) -> Vec<usize> {
        let tile = mvt::pb::Tile::decode(t.bytes.as_slice()).unwrap();
        tile.layers[0]
            .features
            .iter()
            .map(|f| (f.geometry.len() - 2) / 2)
            .collect()
    }

    #[test]
    fn pyramid_has_every_second_level_in_tile_id_order() {
        let exp = fixture(&[&LONG, &SHORT, &BENT]);
        let tiles = pyramid(&exp, 4);
        // Sub-pixel edges are left out of z0-z5, the levels above still get rendered
        let zooms: std::collections::BTreeSet<u8> = tiles.iter().map(|(c, _)| c.z()).collect();
        assert_eq!(zooms, (6..=MAX_ZOOM).step_by(2).collect());
        let ids: Vec<TileId> = tiles.iter().map(|(c, _)| TileId::from(*c)).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        let c = tile_at(MAX_ZOOM, [LONG[0], LONG[1]]);
        let (_, bytes) = tiles.iter().find(|(t, _)| *t == c).unwrap();
        let raw = tile::render(&exp, c).bytes;
        assert_eq!(gunzip(bytes, raw.len()), raw);
    }

    #[test]
    fn short_edges_in_a_row_join_into_one_line() {
        // Ten 31 m edges end to end: 3 tile units each at z9, too small alone, 30 together
        let coords: Vec<[i32; 4]> = (0..10)
            .map(|i| {
                let x = 13_020_000 + i * 500;
                [x, 55_600_000, x + 500, 55_600_000]
            })
            .collect();
        let lines: Vec<&[i32]> = coords.iter().map(|c| &c[..]).collect();
        let z9 = tile_at(9, [13_020_000, 55_600_000]);
        let joined = tile::render(&fixture(&lines), z9);
        assert_eq!(vertices(&joined).len(), 1, "one line");
        let alone = tile::render(&fixture(&lines[..1]), z9);
        assert_eq!(alone.features, 0, "a single short edge is still dropped");

        // Explored both ways, in any order: still one line, the opposite edges don't fold it back
        let back: Vec<[i32; 4]> = coords.iter().map(|c| [c[2], c[3], c[0], c[1]]).collect();
        let both: Vec<&[i32]> = coords
            .iter()
            .zip(&back)
            .rev()
            .flat_map(|(a, b)| [&a[..], &b[..]])
            .collect();
        let joined = tile::render(&fixture(&both), z9);
        assert_eq!(vertices(&joined).len(), 1, "one line");
    }

    #[test]
    fn pyramid_has_every_tile_with_features() {
        let exp = fixture(&[&LONG, &SHORT, &BENT]);
        let tiles = pyramid(&exp, 4);
        for z in LEVELS {
            // Every tile around the fixture, rendered whether an edge's bounds touch it or not
            let (a, b) = (
                tile_at(z, [LONG[0], LONG[1]]),
                tile_at(z, [BENT[4], BENT[5]]),
            );
            let mut want = Vec::new();
            for x in a.x().saturating_sub(1)..=b.x() + 1 {
                for y in a.y().saturating_sub(1)..=b.y() + 1 {
                    let Ok(c) = TileCoord::new(z, x, y) else {
                        continue;
                    };
                    if tile::render(&exp, c).features > 0 {
                        want.push(c);
                    }
                }
            }
            let got: Vec<_> = tiles
                .iter()
                .map(|(c, _)| *c)
                .filter(|c| c.z() == z)
                .collect();
            assert_eq!(got, want, "z{z}");
        }
    }

    #[test]
    fn max_level_keeps_full_detail() {
        let exp = fixture(&[&LONG, &SHORT, &BENT]);
        let z9 = tile_at(9, [LONG[0], LONG[1]]);
        assert_eq!(
            tile::render(&exp, z9).features,
            1,
            "short edges dropped below z10"
        );
        let last = tile_at(MAX_ZOOM, [SHORT[0], SHORT[1]]);
        assert_eq!(
            tile::render(&exp, last).features,
            3,
            "kept at the last level"
        );
        let bent = |z| {
            vertices(&tile::render(
                &fixture(&[&BENT]),
                tile_at(z, [BENT[0], BENT[1]]),
            ))
        };
        assert_eq!(bent(10), vec![2], "simplified below the last level");
        assert_eq!(bent(MAX_ZOOM), vec![3], "every vertex at the last level");
    }

    #[test]
    fn archive_header_and_metadata() {
        let exp = fixture(&[&LONG, &SHORT, &BENT]);
        let bytes = build(&exp, 4).unwrap();
        assert_eq!(&bytes[..8], b"PMTiles\x03");
        assert_eq!(bytes[97], 1, "directories uncompressed");
        assert_eq!(bytes[98], 2, "tiles gzipped");
        assert_eq!(bytes[100], 0, "min zoom");
        assert_eq!(bytes[101], MAX_ZOOM, "max zoom");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains(r#""valhalla_debug":{"algorithm":"forward_astar","costing":"auto""#),
            "{text:.0}"
        );
    }
}
