use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};
use serde::Serialize;
use signoz_demo::telemetry::{TelemetryGuard, current_trace_id, init, inject_current_context};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tracing::{error, info, info_span};

#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
    inventory_url: String,
    payment_url: String,
}

#[derive(Serialize)]
struct CheckoutResponse {
    ok: bool,
    item: String,
    trace_id: String,
    inventory: String,
    payment: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let machine_id = std::env::var("MACHINE_ID").unwrap_or_else(|_| "machine-gateway".to_string());
    let _guard: TelemetryGuard = init("gateway-service", &machine_id)?;

    let addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:7001".to_string())
        .parse()?;

    let state = Arc::new(AppState {
        client: reqwest::Client::new(),
        inventory_url: std::env::var("INVENTORY_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:7002".to_string()),
        payment_url: std::env::var("PAYMENT_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:7003".to_string()),
    });

    let app = Router::new()
        .route("/checkout/{item}", get(checkout))
        .with_state(state);

    info!(%addr, "gateway-service started");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn checkout(
    State(state): State<Arc<AppState>>,
    Path(item): Path<String>,
) -> Json<CheckoutResponse> {
    let span = info_span!("checkout", %item);
    let _entered = span.enter();

    tokio::time::sleep(Duration::from_millis(120)).await;

    let inventory = call_service(&state.client, &state.inventory_url, "reserve", &item).await;
    let payment = call_service(&state.client, &state.payment_url, "pay", &item).await;

    let trace_id = current_trace_id();
    info!(%trace_id, "checkout completed");

    Json(CheckoutResponse {
        ok: inventory.starts_with("ok") && payment.starts_with("ok"),
        item,
        trace_id,
        inventory,
        payment,
    })
}

async fn call_service(client: &reqwest::Client, base_url: &str, op: &str, item: &str) -> String {
    let url = format!("{base_url}/{op}/{item}");
    let mut headers = reqwest::header::HeaderMap::new();
    inject_current_context(&mut headers);

    let call_span = info_span!("call_downstream", %url);
    let _entered = call_span.enter();

    match client.get(url).headers(headers).send().await {
        Ok(resp) => match resp.text().await {
            Ok(text) => {
                info!(response = %text, "downstream call ok");
                text
            }
            Err(err) => {
                error!(error = %err, "read downstream response failed");
                format!("error: read response failed: {err}")
            }
        },
        Err(err) => {
            error!(error = %err, "downstream call failed");
            format!("error: call failed: {err}")
        }
    }
}
