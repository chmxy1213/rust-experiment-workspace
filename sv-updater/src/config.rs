use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(author, version, about = "Build once, push many updater")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Server {
        #[arg(long)]
        config: PathBuf,
    },
    Client {
        #[arg(long)]
        config: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_http_listen")]
    pub http_listen: String,
    #[serde(default = "default_grpc_listen")]
    pub grpc_listen: String,
    #[serde(default = "default_workspace_dir")]
    pub workspace_dir: String,
    #[serde(default)]
    pub build_targets: Vec<BuildTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildTarget {
    pub name: String,
    pub build_script: String,
    pub artifact_path: String,
    pub destination_path: String,
    #[serde(default)]
    pub pre_hooks: Vec<String>,
    #[serde(default)]
    pub post_hooks: Vec<String>,
    #[serde(default)]
    pub required_labels: Vec<String>,
    #[serde(default = "default_true")]
    pub executable: bool,
    #[serde(default = "default_backup_suffix")]
    pub backup_suffix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub server_addr: String,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default = "default_client_root_dir")]
    pub root_dir: String,
    #[serde(default = "default_heartbeat_seconds")]
    pub heartbeat_seconds: u64,
}

impl ServerConfig {
    pub fn workspace_dir(&self) -> PathBuf {
        resolve_path(Path::new("."), &self.workspace_dir)
    }

    pub fn find_target(&self, name: &str) -> Option<BuildTarget> {
        self.build_targets
            .iter()
            .find(|target| target.name == name)
            .cloned()
    }
}

fn resolve_path(base_dir: &Path, raw_path: &str) -> PathBuf {
    let path = PathBuf::from(raw_path);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn default_http_listen() -> String {
    "0.0.0.0:8088".to_string()
}

fn default_grpc_listen() -> String {
    "0.0.0.0:50061".to_string()
}

fn default_workspace_dir() -> String {
    ".".to_string()
}

fn default_client_root_dir() -> String {
    ".".to_string()
}

fn default_heartbeat_seconds() -> u64 {
    5
}

fn default_backup_suffix() -> String {
    "bak".to_string()
}

fn default_true() -> bool {
    true
}