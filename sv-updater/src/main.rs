use anyhow::Result;
use clap::Parser;
use sv_updater::{
    Cli, ClientConfig, Commands, ServerConfig, init_tracing, load_toml_config, run_client,
    run_server,
};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    match cli.command {
        Commands::Server { config } => {
            let config: ServerConfig = load_toml_config(&config).await?;
            run_server(config).await
        }
        Commands::Client { config } => {
            let config: ClientConfig = load_toml_config(&config).await?;
            run_client(config).await
        }
    }
}
