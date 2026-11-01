# About

Valhalla Debug is a tool for debugging Valhalla routes and investigating routing/data issues:
why did the router pick this road, and why not that one?

- **Build a route and experiment with options.** Right-click the map to set a start, a destination
  or a stop; drag markers or reorder the waypoint list. The route re-runs as you change waypoints or
  options. Options form fields are empty by default and show Valhalla's default as a placeholder,
  so only the options you change are sent and put into the URL.
- **Compare alternates.** Time, distance and cost of each route side by side, the best of each in bold.
- **Locate.** Right-click → "Locate here" shows the edges Valhalla's `/locate` snaps to,
  with decoded access, speeds and the full week of predicted speeds.
- **See speeds.** "Road speeds" at the top right of the map (needs `--valhalla-tiles`): live traffic
  (with `--valhalla-traffic`), predicted speeds at any 5-minute slot of the week, or typical day and
  night speeds. Hover an edge for all its speeds, click it for its week. Served as vector tiles from
  the tileset.
- **Debug the search.** "Run expansion" under the routes colors every edge Valhalla's search
  explored, by search tree and search order: a road left uncolored next to colored ones shows where
  a restriction or a mapping error kept the search out. Right-click it → "Locate here" to see why.
  A slider replays the search up to any event.

```
$ ./valhalla-debug --help
Usage: valhalla-debug [OPTIONS]

Options:
      --port <PORT>
          Port to listen [default: 3000]
      --concurrency <CONCURRENCY>
          Max threads to use, also per expansion archive [default: 4]
      --valhalla-url <VALHALLA_URL>
          Valhalla base url to send requests to [default: http://localhost:8002]
      --valhalla-tiles <VALHALLA_TILES>
          Path to Valhalla tileset tarball. Enables speed tiles
      --valhalla-traffic <VALHALLA_TRAFFIC>
          Path to valhalla traffic tarball
      --web-dir <WEB_DIR>
          Directory with static web files [default: web]
  -h, --help
          Print help
```

## Build & Run

```sh
cargo run --release -- --valhalla-tiles /path/to/tiles.tar
```

Point `--valhalla-url` at a Valhalla service that uses the same tileset. Without `--valhalla-tiles`
routing, alternates, locate and expansion still work, but speed tiles don't.

## URL format

Everything you see is in the URL hash, so a link reproduces it. Parameters are `&`-separated:

| Parameter | Example | Meaning |
|---|---|---|
| `map` | `11/55.668/13.158` | zoom/lat/lon of the map view |
| `wp` | `55.605,13.0038;55.7047,13.191!type=through` | waypoints as `lat,lon`, `;`-separated, per-location options as `!key=value` |
| `costing` | `truck` | costing model, `auto` when absent |
| `alt` | `1` | selected route, 0 is the primary |
| anything else | `alternates=0`, `date_time.type=1`, `co.use_highways=0.2` | request options by dot path; `co.` is `costing_options.<costing>.`. Only options that differ from the defaults appear: Valhalla's, except `alternates`, which defaults to 2 here |

## License

All code in this project is dual-licensed under either:

- [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0) ([`LICENSE-APACHE`](LICENSE-APACHE))
- [MIT license](https://opensource.org/licenses/MIT) ([`LICENSE-MIT`](LICENSE-MIT))

at your option.
