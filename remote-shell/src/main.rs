use axum::{routing::get, Router};
use serde::{Deserialize, Serialize};
use tower_http::services::ServeDir;

use crate::api::{index_handler, ws_handler};

mod api;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ServerLogMsg {
    LogStart {
        user: String,
        host: String,
        cwd: String,
    },
    LogOutput {
        data: String,
    },
    LogEnd {
        #[serde(rename = "exitCode")]
        exit_code: i32,
    },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientMsg {
    Input {
        data: String,
    },
    /// Execute a command in a way that we can try to capture execution status (logged wrapped execution)
    Run {
        data: String,

        #[allow(unused)]
        id: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .nest_service("/static", ServeDir::new("static"));

    let addr = "0.0.0.0:3000";
    tracing::info!("Listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
