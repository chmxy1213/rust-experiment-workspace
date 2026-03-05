use axum::{routing::get, Router};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use crate::api::{index_handler, ws_handler};
use crate::static_files::static_handler;

mod api;
mod command;
mod static_files;
mod windows_deps;

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

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Install Windows dependencies (winpty and clink)
    InstallDeps,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::InstallDeps) => {
            if let Err(e) = windows_deps::install_deps() {
                eprintln!("Failed to install dependencies: {}", e);
                std::process::exit(1);
            }
            return;
        }
        None => {}
    }

    tracing_subscriber::fmt::init();

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/static/*file", get(static_handler));

    let addr = "0.0.0.0:3000";
    tracing::info!("Listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
