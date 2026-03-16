mod app;
mod config;
mod web;

pub mod generated {
    tonic::include_proto!("svupdater");
}

pub use app::{init_tracing, load_toml_config, run_client, run_server};
pub use config::{BuildTarget, Cli, ClientConfig, Commands, ServerConfig};