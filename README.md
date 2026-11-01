# About

Valhalla Debug is a tool for debugging Valhalla routes and investigating routing/data issues:
why did the router pick this road, and why not that one?

- **Build a route and experiment with options.** Right-click the map to set a start, a destination
  or a stop; drag markers or reorder the waypoint list. The route re-runs as you change waypoints or
  options. Options form fields are empty by default and show Valhalla's default as a placeholder,
  so only the options you change are sent and put into the URL.
- **Compare alternates.** Time, distance and cost of each route side by side, the best of each in bold.

```
$ ./valhalla-debug --help
Usage: valhalla-debug [OPTIONS]

Options:
      --port <PORT>
          Port to listen [default: 3000]
      --concurrency <CONCURRENCY>
          Max threads to use [default: 4]
      --valhalla-url <VALHALLA_URL>
          Valhalla base url to send requests to [default: http://localhost:8002]
      --web-dir <WEB_DIR>
          Directory with static web files [default: web]
  -h, --help
          Print help
```

## Build & Run

```sh
cargo run --release
```

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
