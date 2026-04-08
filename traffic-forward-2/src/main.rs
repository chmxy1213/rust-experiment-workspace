use axum::{
    Router,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io,
    net::{TcpListener, TcpStream},
    sync::watch,
};
use tracing::{error, info, warn};

const DEFAULT_HTTP_LISTEN_ADDR: &str = "0.0.0.0:3000";
const DEFAULT_TCP_LISTEN_ADDR: &str = "0.0.0.0:4000";
const DEFAULT_UPSTREAM_ADDR: &str = "127.0.3.9:8080";
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 3_000;

#[derive(Clone)]
struct AppConfig {
    http_listen_addr: SocketAddr,
    tcp_listen_addr: SocketAddr,
    upstream_addr: Arc<str>,
    connect_timeout: Duration,
}

impl AppConfig {
    fn from_env() -> Result<Self, String> {
        let http_listen_addr = std::env::var("HTTP_LISTEN_ADDR")
            .unwrap_or_else(|_| DEFAULT_HTTP_LISTEN_ADDR.to_string())
            .parse::<SocketAddr>()
            .map_err(|err| format!("invalid HTTP_LISTEN_ADDR: {err}"))?;

        let tcp_listen_addr = std::env::var("TCP_LISTEN_ADDR")
            .unwrap_or_else(|_| DEFAULT_TCP_LISTEN_ADDR.to_string())
            .parse::<SocketAddr>()
            .map_err(|err| format!("invalid TCP_LISTEN_ADDR: {err}"))?;

        let upstream_addr = std::env::var("UPSTREAM_ADDR")
            .unwrap_or_else(|_| DEFAULT_UPSTREAM_ADDR.to_string())
            .into();

        let connect_timeout_ms = std::env::var("CONNECT_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS);

        Ok(Self {
            http_listen_addr,
            tcp_listen_addr,
            upstream_addr,
            connect_timeout: Duration::from_millis(connect_timeout_ms),
        })
    }
}

#[tokio::main]
async fn main() {
    init_tracing();

    let config = match AppConfig::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            error!("{err}");
            std::process::exit(1);
        }
    };

    info!(
        http_listen = %config.http_listen_addr,
        tcp_listen = %config.tcp_listen_addr,
        upstream = %config.upstream_addr,
        "traffic-forward starting"
    );

    let http_listener = match TcpListener::bind(config.http_listen_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            error!("failed to bind HTTP listener: {err}");
            std::process::exit(1);
        }
    };

    let tcp_listener = match TcpListener::bind(config.tcp_listen_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            error!("failed to bind TCP listener: {err}");
            std::process::exit(1);
        }
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let http_task = tokio::spawn(run_http_server(http_listener, shutdown_rx.clone()));
    let tcp_task = tokio::spawn(run_tcp_proxy_server(
        tcp_listener,
        config.upstream_addr.clone(),
        config.connect_timeout,
        shutdown_rx,
    ));

    tokio::select! {
        result = http_task => {
            if let Err(err) = result {
                error!("HTTP task join error: {err}");
            }
            let _ = shutdown_tx.send(true);
        }
        result = tcp_task => {
            if let Err(err) = result {
                error!("TCP task join error: {err}");
            }
            let _ = shutdown_tx.send(true);
        }
        signal = tokio::signal::ctrl_c() => {
            match signal {
                Ok(()) => info!("received Ctrl+C, shutting down"),
                Err(err) => warn!("failed to listen Ctrl+C signal: {err}"),
            }
            let _ = shutdown_tx.send(true);
        }
    }

    info!("traffic-forward stopped");
}

fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "traffic_forward=info,axum=info".into());

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();
}

async fn run_http_server(listener: TcpListener, mut shutdown_rx: watch::Receiver<bool>) {
    let app = Router::new()
        .route("/", get(static_index))
        .route("/secvison_ping", get(secvison_ping))
        .fallback(get(not_found));

    info!(listen = %listener.local_addr().unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0))), "HTTP server ready");

    let shutdown_signal = async move {
        let _ = shutdown_rx.changed().await;
    };

    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await
    {
        error!("HTTP server error: {err}");
    }
}

async fn run_tcp_proxy_server(
    listener: TcpListener,
    upstream_addr: Arc<str>,
    connect_timeout: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let connection_id = Arc::new(AtomicU64::new(1));

    info!(listen = %listener.local_addr().unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0))), upstream = %upstream_addr, "TCP proxy ready");

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                info!("TCP proxy shutting down");
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((downstream, peer_addr)) => {
                        let id = connection_id.fetch_add(1, Ordering::Relaxed);
                        let upstream = upstream_addr.clone();
                        tokio::spawn(async move {
                            handle_tcp_connection(id, peer_addr, downstream, upstream, connect_timeout).await;
                        });
                    }
                    Err(err) => {
                        warn!("failed to accept TCP connection: {err}");
                    }
                }
            }
        }
    }
}

async fn handle_tcp_connection(
    connection_id: u64,
    peer_addr: SocketAddr,
    mut downstream: TcpStream,
    upstream_addr: Arc<str>,
    connect_timeout: Duration,
) {
    let started = Instant::now();
    info!(connection_id, %peer_addr, upstream = %upstream_addr, "new TCP connection");

    let upstream_result = tokio::time::timeout(connect_timeout, TcpStream::connect(&*upstream_addr)).await;

    let mut upstream = match upstream_result {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => {
            warn!(connection_id, %peer_addr, upstream = %upstream_addr, "upstream connect failed: {err}");
            return;
        }
        Err(_) => {
            warn!(connection_id, %peer_addr, upstream = %upstream_addr, timeout_ms = connect_timeout.as_millis(), "upstream connect timeout");
            return;
        }
    };

    match io::copy_bidirectional(&mut downstream, &mut upstream).await {
        Ok((bytes_to_upstream, bytes_to_downstream)) => {
            info!(
                connection_id,
                %peer_addr,
                upstream = %upstream_addr,
                bytes_to_upstream,
                bytes_to_downstream,
                duration_ms = started.elapsed().as_millis(),
                "TCP connection closed"
            );
        }
        Err(err) => {
            warn!(
                connection_id,
                %peer_addr,
                upstream = %upstream_addr,
                duration_ms = started.elapsed().as_millis(),
                "TCP forwarding error: {err}"
            );
        }
    }
}

async fn secvison_ping() -> &'static str {
    "pong"
}

async fn static_index() -> Html<&'static str> {
    Html(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>traffic-forward</title></head><body><h1>traffic-forward</h1><p>HTTP local routes are active.</p></body></html>",
    )
}

async fn not_found() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "Not Found")
}
