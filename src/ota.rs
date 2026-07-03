use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::Identity;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct OtaResponse {
    #[serde(default)]
    pub firmware: Option<Firmware>,
    #[serde(default)]
    pub websocket: Option<WebsocketConfig>,
    #[serde(default)]
    pub mqtt: Option<serde_json::Value>,
    #[serde(default)]
    pub activation: Option<serde_json::Value>,
    #[serde(default)]
    pub server_time: Option<ServerTime>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Firmware {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub force: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct WebsocketConfig {
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub version: Option<u8>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ServerTime {
    pub timestamp: u64,
    #[serde(default)]
    pub timezone_offset: Option<i32>,
}

/// Resolved WebSocket connection info, preferring OTA, falling back to args.
#[derive(Debug, Clone)]
pub struct WsConnectInfo {
    pub url: String,
    pub token: Option<String>,
    pub version: u8,
}

pub async fn fetch_ota(
    client: &reqwest::Client,
    ota_url: &str,
    identity: &Identity,
    language: &str,
) -> Result<OtaResponse> {
    tracing::info!(ota_url, %identity.device_id, %identity.client_id, "requesting OTA");

    let req = client
        .get(ota_url)
        .header("User-Agent", identity.user_agent())
        .header("Device-Id", &identity.device_id)
        .header("Client-Id", &identity.client_id)
        .header("Accept-Language", language)
        .header("Content-Type", "application/json");
    let resp = req.send().await.with_context(|| format!("GET {ota_url}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("OTA {ota_url} returned {status}: {body}");
    }
    let text = resp.text().await.context("read OTA body")?;
    tracing::debug!(ota_url, body = %text, "OTA response body");
    let ota: OtaResponse =
        serde_json::from_str(&text).with_context(|| format!("parse OTA json from {ota_url}"))?;

    if ota.activation.is_some() {
        tracing::warn!("OTA returned `activation`; CLI does not support onboarding, ignoring");
    }
    if ota.mqtt.is_some() && ota.websocket.is_none() {
        anyhow::bail!(
            "OTA returned mqtt but no websocket config; CLI only supports websocket transport"
        );
    }
    Ok(ota)
}

impl OtaResponse {
    pub fn resolve_ws(&self, fallback_version: u8) -> Option<WsConnectInfo> {
        let ws = self.websocket.as_ref()?;
        let version = ws.version.unwrap_or(fallback_version);
        Some(WsConnectInfo {
            url: ws.url.clone(),
            token: ws.token.clone(),
            version,
        })
    }
}
