use std::{num::NonZero, sync::Arc};

use ::valhalla::GraphReader;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, header},
    routing::{get, post},
};
use clap::Parser;
use tokio::signal;
use tower_http::{
    compression::CompressionLayer, services::ServeDir, set_header::SetResponseHeader,
};
use tracing::info;

mod grid;
mod mvt;
mod params;
mod request;
mod route;
mod speeds;
mod valhalla;

#[derive(Parser)]
struct Config {
    /// Port to listen
    #[arg(long, default_value_t = 3000)]
    port: u16,
    /// Max threads to use
    #[arg(long, default_value_t = 4)]
    concurrency: u16,
    /// Valhalla base url to send requests to
    #[arg(long, default_value = "http://localhost:8002")]
    valhalla_url: String,
    /// Path to Valhalla tileset tarball. Enables speed tiles.
    #[arg(long)]
    valhalla_tiles: Option<String>,
    /// Path to valhalla traffic tarball.
    #[arg(long)]
    valhalla_traffic: Option<String>,
    /// Directory with static web files
    #[arg(long, default_value = "web")]
    web_dir: String,
}

#[derive(Clone)]
struct AppState {
    http_client: reqwest::Client,
    /// `--valhalla-url` without a trailing slash.
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
        .valhalla_tiles
        .map(|path| {
            ::valhalla::ConfigBuilder {
                mjolnir: ::valhalla::config::Mjolnir {
                    tile_extract: path,
                    traffic_extract: config.valhalla_traffic.unwrap_or_default(),
                    ..Default::default()
                },
                ..Default::default()
            }
            .build()
        })
        .and_then(|cfg| GraphReader::new(&cfg).ok());
    if graph_reader.is_some() {
        info!("Loaded Valhalla tiles, speed tiles are available")
    }

    // build our application with a route
    let app = Router::new()
        .route("/api/info", get(info))
        .route("/ui/route", post(route::route_panel))
        .route("/ui/params", get(params::panel))
        .route("/ui/params/form", get(params::form))
        .route("/tiles/speeds/{z}/{x}/{y}", get(speeds::tile))
        // Revalidate static files on every load (cheap 304s via ETag): browsers, Safari especially,
        // otherwise reuse stale ES modules next to fresh ones and the page breaks after an update
        .fallback_service(SetResponseHeader::if_not_present(
            ServeDir::new(&config.web_dir),
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(CompressionLayer::new())
        .with_state(AppState {
            http_client: reqwest::Client::new(),
            valhalla_url: config.valhalla_url.trim_end_matches('/').into(),
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

/// `GET /api/info`: what this server can do, so the page can hide unavailable features.
async fn info(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "graph": state.graph_reader.is_some() }))
}
