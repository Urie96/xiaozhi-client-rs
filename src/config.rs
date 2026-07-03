use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Cli;

/// Persisted, stable identity generated on first run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub client_id: String,
    pub device_id: String,
    pub board_name: String,
}

impl Identity {
    pub fn generate() -> Self {
        let client_id = uuid::Uuid::new_v4().to_string();
        // Generate a pseudo MAC so the server logs a stable, recognizable id.
        let id = uuid::Uuid::new_v4();
        let bytes = id.as_bytes();
        let mac = format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
        );
        Self {
            client_id,
            device_id: mac,
            board_name: "rust-cli".to_string(),
        }
    }

    pub fn user_agent(&self) -> String {
        format!("Xiaozhi/{}/0.1.0 (Rust/Linux)", self.board_name)
    }
}

/// The whole config file: `[identity]` + `[server]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredConfig {
    #[serde(default = "default_identity")]
    pub identity: Identity,
    #[serde(default)]
    pub server: ServerConfig,
}

fn default_identity() -> Identity {
    Identity::generate()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ota_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ws_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub token: Option<String>,
}

/// Effective, merged config used at runtime.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Config {
    pub stored: StoredConfig,
    pub config_path: PathBuf,
    pub language: String,
    pub protocol_version: u8,
}

impl Config {
    pub fn identity(&self) -> &Identity {
        &self.stored.identity
    }

    pub fn load_or_create(cli: &Cli) -> Result<Self> {
        let config_path = match cli.config.clone() {
            Some(p) => PathBuf::from(p),
            None => default_config_path()?,
        };

        let stored = if config_path.exists() {
            let text = fs::read_to_string(&config_path)
                .with_context(|| format!("read config {}", config_path.display()))?;
            let mut cfg: StoredConfig = toml::from_str(&text)
                .with_context(|| format!("parse config {}", config_path.display()))?;
            if cfg.identity.client_id.is_empty() {
                cfg.identity = Identity::generate();
                let _ = save_config(&config_path, &cfg);
            }
            // CLI ota_url overrides stored on first run, but don't clobber.
            if cfg.server.ota_url.is_none() {
                cfg.server.ota_url = cli.ota_url.clone();
                let _ = save_config(&config_path, &cfg);
            }
            cfg
        } else {
            let cfg = StoredConfig {
                identity: Identity::generate(),
                server: ServerConfig {
                    ota_url: cli.ota_url.clone(),
                    ..Default::default()
                },
            };
            save_config(&config_path, &cfg)?;
            cfg
        };

        Ok(Config {
            stored,
            config_path,
            language: cli.language.clone(),
            protocol_version: cli.protocol_version,
        })
    }

    pub fn effective_ota_url(&self, cli_ota: &Option<String>) -> Option<String> {
        cli_ota
            .clone()
            .or_else(|| self.stored.server.ota_url.clone())
            .or_else(|| std::env::var("XIAOZHI_OTA_URL").ok())
    }

    pub fn ws_url(&self) -> Option<String> {
        self.stored.server.ws_url.clone()
    }

    pub fn token(&self) -> Option<String> {
        self.stored.server.token.clone()
    }
}

fn default_config_path() -> Result<PathBuf> {
    let dir =
        dirs::config_dir().context("could not determine config dir (XDG_CONFIG_HOME / HOME)?")?;
    Ok(dir.join("xiaozhi-client-rs").join("config.toml"))
}

fn save_config(path: &Path, cfg: &StoredConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(cfg).with_context(|| "serialize config")?;
    let mut out = String::from("# xiaozhi-client-rs config\n\n");
    out.push_str(&text);
    fs::write(path, out).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
