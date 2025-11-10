use std::{collections::HashMap, num::NonZero, sync::Arc, time::Instant};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use clap::Parser;
use serde::Deserialize;
use serde_json::Value;
use tokio::{fs::File, io::AsyncReadExt, signal};
use tracing::info;
use valhalla::{GraphLevel, GraphReader, LatLon};

#[derive(Parser)]
struct Config {
    /// Port to listen
    #[arg(long, default_value_t = 3000)]
    port: u16,
    /// Max threads to use
    #[arg(long, default_value_t = 4)]
    concurrency: u16,
    /// Mapbox access token to use in the frontend
    #[arg(long, env)]
    mapbox_access_token: String,
    /// Valhalla base url to send requests to
    #[arg(long, default_value = "http://localhost:8002")]
    valhalla_url: String,
    /// Path to valhalla json config file.
    /// Required for an access to valhalla graph information.
    #[arg(long)]
    valhalla_config_path: Option<String>,
}

#[derive(Clone)]
struct AppState {
    http_client: reqwest::Client,
    mapbox_access_token: Arc<str>,
    valhalla_url: Arc<str>,
    graph_reader: Option<GraphReader>,
}

fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::parse();

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(
            std::thread::available_parallelism()
                .map(NonZero::get)
                .unwrap_or(16) // fallback to 16 as max if we can't get the number of CPUs
                .min(config.concurrency as usize),
        )
        .enable_all()
        .build()
        .unwrap()
        .block_on(run(config))
}

async fn run(config: Config) {
    let graph_reader = config
        .valhalla_config_path
        .and_then(|path| valhalla::Config::from_file(path).ok())
        .and_then(|cfg| GraphReader::new(&cfg).ok());
    if graph_reader.is_some() {
        info!("Loaded Valhalla tiles. Traffic functionality is awailable!")
    }

    // build our application with a route
    let app = Router::new()
        .route("/", get(serve_index_html))
        .route("/api/request", post(forward_request))
        .route("/api/traffic/{bbox}", get(traffic))
        .with_state(AppState {
            http_client: reqwest::Client::new(),
            mapbox_access_token: config.mapbox_access_token.into(),
            valhalla_url: config.valhalla_url.into(),
            graph_reader,
        });

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.port))
        .await
        .unwrap();
    info!("Listening at http://localhost:{}", config.port);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::select! {
                _ = signal::ctrl_c() => {
                    info!("Ctrl+C received, shutting down");
                }
                _ = async {
                    signal::unix::signal(signal::unix::SignalKind::terminate())
                        .expect("failed to install SIGTERM signal handler")
                        .recv()
                        .await
                } => {
                    info!("SIGTERM received, shutting down");
                }
            }
        })
        .await
        .unwrap();
}

async fn serve_index_html(
    State(state): State<AppState>,
) -> Result<Html<String>, (StatusCode, String)> {
    let index_html = "web/index.html";
    let Ok(mut file) = File::open(index_html).await else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Failed to open {index_html}: not found"),
        ));
    };

    let mut contents = String::new();
    if let Err(err) = file.read_to_string(&mut contents).await {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read {index_html}: {err}"),
        ));
    }

    let contents = contents.replace("{{MAPBOX_ACCESS_TOKEN}}", &state.mapbox_access_token);
    Ok(Html(contents))
}

#[derive(Deserialize)]
struct RequestToForward {
    /// Valhalla API endpoint. See https://valhalla.github.io/valhalla/api for more details.
    endpoint: String,
    /// Data to send
    payload: Value,
}

async fn forward_request(
    State(state): State<AppState>,
    Json(request): Json<RequestToForward>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let url = format!("{}/{}", state.valhalla_url, request.endpoint);
    let begin = Instant::now();
    let response = state
        .http_client
        .post(&url)
        .json(&request.payload)
        .send()
        .await;
    info!(
        "Fetched data from {url} in {}ms",
        begin.elapsed().as_millis()
    );

    response
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?
        .json()
        .await
        .map(Json)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SpeedSource {
    #[default]
    Live,
    Day,
    Night,
}

#[derive(Default, Deserialize)]
struct TrafficQuery {
    source: SpeedSource,
}

async fn traffic(
    State(state): State<AppState>,
    Path(bbox): Path<String>,
    Query(query): Query<TrafficQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let Some(bbox) = parse_bbox(&bbox) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Bad bbox, expecting 'min_lat,min_lon;max_lat,max_lon'".to_string(),
        ));
    };
    info!("Reading traffic for {bbox:?}");

    let Some(reader) = &state.graph_reader else {
        return Err((
            StatusCode::IM_A_TEAPOT,
            "Traffic information was not enabled".to_string(),
        ));
    };

    let begin = Instant::now();
    let edges = [GraphLevel::Highway, GraphLevel::Arterial, GraphLevel::Local]
        .into_iter()
        .map(|level| reader.tiles_in_bbox(bbox.0, bbox.1, level))
        // Limit number of traffic tiles we fetch
        .scan(0, |count, tiles| {
            *count += tiles.len();
            if *count < 20 { Some(tiles) } else { None }
        })
        .flatten()
        .flat_map(|tile_id| reader.graph_tile(tile_id))
        // todo: this is really heavy compute operation
        .flat_map(|tile| {
            tile.directededges()
                .iter()
                .filter_map(|edge| {
                    match query.source {
                        SpeedSource::Live => tile.live_speed(edge),
                        SpeedSource::Day => {
                            let s = edge.constrained_flow_speed();
                            if s == 0 { None } else { Some(s) }
                        }
                        SpeedSource::Night => {
                            let s = edge.free_flow_speed();
                            if s == 0 { None } else { Some(s) }
                        }
                    }
                    .map(|speed| {
                        (
                            tile.edgeinfo(edge).shape,
                            speed as f32 / edge.speed() as f32,
                        )
                    })
                })
                .collect::<Vec<_>>()
        })
        .map(|(shape, normalized_speed)| {
            (
                shape,
                // Convert normalized speed [0.0, 1.0] to a jam factor [10.0, 0.0]
                10 - (normalized_speed * 10.0).round() as i32,
            )
        })
        .collect::<HashMap<_, _>>();
    info!(
        "Fetched {} traffic edges in {}ms",
        edges.len(),
        begin.elapsed().as_millis()
    );
    Ok(Json(serde_json::to_value(edges).unwrap()))
}

fn parse_coordinate(coord: &str) -> Option<LatLon> {
    let (lat, lon) = coord.split_once(',')?;
    let lat = lat.parse::<f64>().ok()?;
    let lon = lon.parse::<f64>().ok()?;
    Some(LatLon(lat, lon))
}

fn parse_bbox(bbox: &str) -> Option<(LatLon, LatLon)> {
    let (min, max) = bbox.split_once(';')?;
    let min = parse_coordinate(min)?;
    let max = parse_coordinate(max)?;
    Some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bbox() {
        assert_eq!(
            parse_bbox("55.0,13.0;56.0,14.0"),
            Some((LatLon(55.0, 13.0), LatLon(56.0, 14.0)))
        );
        assert_eq!(
            parse_bbox("37.7749,-122.4194;34.0522,-118.2437"),
            Some((LatLon(37.7749, -122.4194), LatLon(34.0522, -118.2437)))
        );

        // missing semicolon
        assert_eq!(parse_bbox("37.7749,-122.4194 34.0522,-118.2437"), None);
        // missing comma
        assert_eq!(parse_bbox("37.7749 -122.4194;34.0522,-118.2437"), None);
        assert_eq!(parse_bbox("37.7749;-122.4194;34.0522,-118.2437"), None);
        assert_eq!(parse_bbox("37.7749-122.4194;34.0522,-118.2437"), None);
        assert_eq!(parse_bbox("37.7749,-122.4194;34.0522 -118.2437"), None);
        assert_eq!(parse_bbox("37.7749,-122.4194;34.0522;-118.2437"), None);
        assert_eq!(parse_bbox("37.7749,-122.4194;34.0522-118.2437"), None);
        // not a number
        assert_eq!(parse_bbox("invalid;34.0522,-118.2437"), None);
        assert_eq!(parse_bbox("37.7749,invalid;34.0522,-118.2437"), None);
        assert_eq!(parse_bbox("37.7749,-122.4194;invalid,-118.2437"), None);
        assert_eq!(parse_bbox("37.7749,-122.4194;34.0522,invalid"), None);
    }
}
