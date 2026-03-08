use anyhow::Result;
use axum::{Router, extract::Path, http::HeaderMap, routing::get};
use signoz_demo::telemetry::{TelemetryGuard, extract_parent_context, init};
use std::{net::SocketAddr, time::Duration};
use tracing::{info, info_span, warn};
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[tokio::main]
async fn main() -> Result<()> {
    let machine_id = std::env::var("MACHINE_ID").unwrap_or_else(|_| "machine-payment".to_string());
    let _guard: TelemetryGuard = init("payment-service", &machine_id)?;

    let addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:7003".to_string())
        .parse()?;

    let app = Router::new().route("/pay/{item}", get(pay));

    info!(%addr, "payment-service started");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn pay(headers: HeaderMap, Path(item): Path<String>) -> String {
    let parent_cx = extract_parent_context(&headers);
    let span = info_span!("do_payment", %item);
    let _ = span.set_parent(parent_cx);
    let _entered = span.enter();

    info!("creating payment order");
    tokio::time::sleep(Duration::from_millis(100)).await;

    if item == "fail" {
        warn!("mock payment failure");
        return "error: payment declined".to_string();
    }

    info!("payment finished");
    format!("ok: paid for {item}")
}
