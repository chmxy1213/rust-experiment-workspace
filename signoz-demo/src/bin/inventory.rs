use anyhow::Result;
use axum::{Router, extract::Path, http::HeaderMap, routing::get};
use signoz_demo::telemetry::{TelemetryGuard, extract_parent_context, init};
use std::{net::SocketAddr, time::Duration};
use tracing::{info, info_span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[tokio::main]
async fn main() -> Result<()> {
    let machine_id =
        std::env::var("MACHINE_ID").unwrap_or_else(|_| "machine-inventory".to_string());
    let _guard: TelemetryGuard = init("inventory-service", &machine_id)?;

    let addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:7002".to_string())
        .parse()?;

    let app = Router::new().route("/reserve/{item}", get(reserve));

    info!(%addr, "inventory-service started");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn reserve(headers: HeaderMap, Path(item): Path<String>) -> String {
    let parent_cx = extract_parent_context(&headers);
    let span = info_span!("reserve_inventory", %item);
    let _ = span.set_parent(parent_cx);
    let _entered = span.enter();

    info!("checking stock");
    tokio::time::sleep(Duration::from_millis(80)).await;
    info!("stock reserved");
    format!("ok: reserved {item}")
}
