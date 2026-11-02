# About

Valhalla Debug is a tool for debugging Valhalla routes and investigating routing/data issues. It allows you to:

- **Build a route and experiment with different parameters.** Right-click the map for waypoints, drag them,
  compare alternates.
- **Visualize route expansion.** Every edge the search explored, colored by search tree and order. The slider
  replays the search to find problem spots in the road graph.
- **Investigate road graph edges.** Right-click, "Locate here": the edges Valhalla snaps to, their access and
  a week of speeds.
- **View traffic data.** Live, predicted, day and night speeds from the tileset, if provided.

<img width="1512" height="859" alt="Valhalla Debug page" src="https://github.com/user-attachments/assets/470521dd-903c-49ed-a66f-0cd440a916d1" />

*Expansion graph for a route with 3 alternatives.*

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

## URL format

The URL hash holds the whole page state, so a link reproduces it:

| Parameter | Example | Meaning |
|---|---|---|
| `map` | `11/55.668/13.158` | zoom/lat/lon |
| `wp` | `55.605,13.0038;55.7047,13.191!type=through` | `;`-separated waypoints, location options as `!key=value` |
| `costing` | `truck` | `auto` when absent |
| `alt` | `1` | selected route, 0 is the primary |
| other | `alternates=0`, `co.use_highways=0.2` | changed request options; `co.` is `costing_options.<costing>.` |

## License

All code in this project is dual-licensed under either:

- [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0) ([`LICENSE-APACHE`](LICENSE-APACHE))
- [MIT license](https://opensource.org/licenses/MIT) ([`LICENSE-MIT`](LICENSE-MIT))

at your option.
