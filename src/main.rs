mod audio;
mod client;
mod config;
mod ota;
mod protocol;

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Config;

#[derive(Parser, Debug)]
#[command(name = "xiaozhi-client", version, about = "Xiaozhi Rust CLI client")]
struct Cli {
    /// OTA URL, e.g. http://127.0.0.1:8080/ota
    #[arg(long)]
    ota_url: Option<String>,

    /// Override the WebSocket url returned by OTA.
    #[arg(long)]
    ws_url: Option<String>,

    /// Override the token returned by OTA.
    #[arg(long)]
    token: Option<String>,

    /// Binary protocol version (1/2/3). Default 3.
    #[arg(long, default_value_t = 3)]
    protocol_version: u8,

    /// Language sent in Accept-Language header.
    #[arg(long, default_value = "zh-CN")]
    language: String,

    /// Config file path (default ~/.config/xiaozhi-client-rs/config.toml).
    #[arg(long)]
    config: Option<String>,

    /// Input (mic) cpal device name substring. Empty = default.
    #[arg(long)]
    input_device: Option<String>,

    /// Output (speaker) cpal device name substring. Empty = default.
    #[arg(long)]
    output_device: Option<String>,

    /// Verbose logging.
    #[arg(long)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print detected audio devices and exit.
    Devices,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                if cli.verbose {
                    "xiaozhi_client_rs=debug".into()
                } else {
                    "xiaozhi_client_rs=info".into()
                }
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut config = Config::load_or_create(&cli)?;

    if let Some(Command::Devices) = cli.command {
        audio::list_devices();
        return Ok(());
    }

    if let Some(url) = &cli.ws_url {
        config.stored.server.ws_url = Some(url.clone());
    }
    if let Some(token) = &cli.token {
        config.stored.server.token = Some(token.clone());
    }

    let runtime = client::ClientRuntime::new(
        cli.protocol_version,
        cli.language,
        config.identity().clone(),
        config.clone(),
    );
    runtime.run(cli.ota_url, cli.input_device, cli.output_device)
}
