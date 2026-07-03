use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use http::Request as HttpRequest;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, tungstenite::client::IntoClientRequest,
};
use tracing::{info, warn};

use crate::audio::input::InputPipeline;
use crate::audio::output::OutputPipeline;
use crate::config::{Config, Identity};
use crate::ota::{self, WsConnectInfo};
use crate::protocol::{self, BinaryProtocolVersion, IncomingJson, SERVER_SAMPLE_RATE_DEFAULT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Listening,
    Speaking,
}

pub struct ClientRuntime {
    pub protocol_version: u8,
    pub language: String,
    pub identity: Identity,
    pub config: Config,
}

impl ClientRuntime {
    pub fn new(protocol_version: u8, language: String, identity: Identity, config: Config) -> Self {
        Self {
            protocol_version,
            language,
            identity,
            config,
        }
    }

    pub fn run(
        self,
        cli_ota_url: Option<String>,
        input_device: Option<String>,
        output_device: Option<String>,
    ) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("build tokio runtime")?;
        runtime.block_on(self.async_run(cli_ota_url, input_device, output_device))
    }

    async fn async_run(
        self,
        cli_ota_url: Option<String>,
        input_device: Option<String>,
        output_device: Option<String>,
    ) -> Result<()> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;

        // 1. OTA (or fall back to stored ws_url).
        let ws_info = self.resolve_ws(&http_client, &cli_ota_url).await?;

        // 2. Connect WebSocket with headers.
        let version =
            BinaryProtocolVersion::from_u8(ws_info.version).unwrap_or(BinaryProtocolVersion::V3);
        info!(url = %ws_info.url, version = ws_info.version, "connecting websocket");

        let req = build_ws_request(&ws_info, &self.identity)?;
        let (ws_stream, resp) = connect_async(req)
            .await
            .with_context(|| format!("connect ws {}", ws_info.url))?;
        info!(status = ?resp.status(), "websocket connected");

        let (mut ws_tx, mut ws_rx) = ws_stream.split();

        // 3. Send hello.
        let hello = protocol::hello(ws_info.version);
        let hello_text = hello.to_string();
        ws_tx
            .send(Message::text(hello_text))
            .await
            .context("send hello")?;
        info!("hello sent, waiting for server hello (10s)");

        // 4. Wait for server hello.
        let server_hello = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match ws_rx.next().await {
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(json) = serde_json::from_str::<IncomingJson>(&t) {
                            if json.typ == "hello" {
                                return Ok(json);
                            }
                            warn!(typ = %json.typ, "unexpected message before hello");
                        }
                    }
                    Some(Ok(other)) => {
                        warn!(?other, "non-text frame before hello");
                    }
                    Some(Err(e)) => return Err(anyhow::anyhow!(e)),
                    None => return Err(anyhow::anyhow!("ws closed before hello")),
                }
            }
        })
        .await
        .context("wait for server hello")??;

        let session_id = server_hello.session_id.clone().unwrap_or_default();
        let server_rate = server_hello
            .audio_params()
            .as_ref()
            .and_then(|p| p.get("sample_rate").and_then(|v| v.as_u64()))
            .map(|v| v as u32)
            .unwrap_or(SERVER_SAMPLE_RATE_DEFAULT);
        info!(%session_id, server_rate, "server hello received");

        // 5. Start audio pipelines.
        let input_dev = crate::audio::pick_input(input_device.as_deref())
            .context("no input device available")?;
        let output_dev = crate::audio::pick_output(output_device.as_deref())
            .context("no output device available")?;

        let mut input = InputPipeline::start(&input_dev, version)?;
        let output = OutputPipeline::start(&output_dev, server_rate)?;

        // 6. Input drain task: forward opus frames only while listening.
        let listening = Arc::new(AtomicBool::new(false));
        let (forward_tx, mut forward_rx) = mpsc::channel::<Message>(64);
        let listening_clone = listening.clone();
        let input_task = tokio::spawn(async move {
            while let Some(frame) = input.opus_rx.recv().await {
                if listening_clone.load(Ordering::Relaxed)
                    && forward_tx.send(Message::binary(frame)).await.is_err()
                {
                    break;
                }
            }
        });

        // 7. stdin reader task (blocking in a dedicated thread).
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<()>(16);
        std::thread::spawn(move || {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            let mut reader = std::io::BufReader::new(stdin);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let _ = stdin_tx.blocking_send(());
            }
        });

        // 8. Main event loop.
        let mut state = State::Idle;
        println!("\n=== 小智客户端已连接 (session {session_id:.8}) ===");
        println!("按回车开始说话，再按回车可手动结束。Ctrl-C 退出。\n");

        loop {
            tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => {
                    info!("ctrl-c received, exiting");
                    break;
                }
                cmd = stdin_rx.recv() => {
                    if cmd.is_none() { break; }
                    match state {
                        State::Idle => {
                            // Start listening.
                            let msg = protocol::listen_start(&session_id, "auto");
                            if ws_tx.send(Message::text(msg.to_string())).await.is_err() {
                                break;
                            }
                            listening.store(true, Ordering::Relaxed);
                            state = State::Listening;
                            println!("\r[监听中] 请说话...");
                        }
                        State::Listening => {
                            // Manual stop.
                            listening.store(false, Ordering::Relaxed);
                            let msg = protocol::listen_stop(&session_id);
                            let _ = ws_tx.send(Message::text(msg.to_string())).await;
                            println!("\r[处理中] 等待服务端响应...");
                            state = State::Idle;
                        }
                        State::Speaking => {
                            // Interrupt: abort then listen.
                            output.flush();
                            let _ = ws_tx.send(Message::text(
                                protocol::abort(&session_id, "manual").to_string()
                            )).await;
                            let msg = protocol::listen_start(&session_id, "auto");
                            let _ = ws_tx.send(Message::text(msg.to_string())).await;
                            listening.store(true, Ordering::Relaxed);
                            state = State::Listening;
                            println!("\r[打断-监听中] 请说话...");
                        }
                    }
                }
                msg = forward_rx.recv() => {
                    let Some(msg) = msg else { break; };
                    if ws_tx.send(msg).await.is_err() {
                        break;
                    }
                }
                ws_msg = ws_rx.next() => {
                    match ws_msg {
                        Some(Ok(Message::Text(t))) => {
                            if let Err(e) = handle_text(t.as_str(), &output, &listening, &mut state, &session_id) {
                                warn!(%e, "handle text failed");
                            }
                        }
                        Some(Ok(Message::Binary(data))) => {
                            // Forward opus to output pipeline.
                            let frame = protocol::decode_audio_frame(version, data.as_ref());
                            if let Some(f) = frame {
                                let _ = output.opus_tx.send(f.payload).await;
                            }
                        }
                        Some(Ok(Message::Ping(p))) => {
                            let _ = ws_tx.send(Message::Pong(p)).await;
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("server closed websocket");
                            break;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            warn!(%e, "ws recv error");
                            break;
                        }
                        None => break,
                    }
                }
            }
        }

        // Cleanup.
        listening.store(false, Ordering::Relaxed);
        let _ = ws_tx.send(Message::Close(None)).await;
        input_task.abort();
        info!("client shutting down");
        Ok(())
    }

    async fn resolve_ws(
        &self,
        http_client: &reqwest::Client,
        cli_ota_url: &Option<String>,
    ) -> Result<WsConnectInfo> {
        // If ws_url is explicitly set, skip OTA.
        if let Some(url) = self.config.ws_url() {
            info!(%url, "using stored/override ws_url, skipping OTA");
            return Ok(WsConnectInfo {
                url,
                token: self.config.token(),
                version: self.protocol_version,
            });
        }

        let ota_url = self.config.effective_ota_url(cli_ota_url).ok_or_else(|| {
            anyhow::anyhow!("no ota_url provided (use --ota-url or set it in config)")
        })?;

        let ota = ota::fetch_ota(http_client, &ota_url, &self.identity, &self.language).await?;
        let info = ota
            .resolve_ws(self.protocol_version)
            .context("OTA response missing websocket config")?;
        info!(url = %info.url, "OTA resolved ws url");
        Ok(info)
    }
}

fn handle_text(
    text: &str,
    output: &OutputPipeline,
    listening: &Arc<AtomicBool>,
    state: &mut State,
    session_id: &str,
) -> Result<()> {
    let json: IncomingJson = match serde_json::from_str(text) {
        Ok(j) => j,
        Err(e) => {
            warn!(%e, %text, "invalid json from server");
            return Ok(());
        }
    };

    // Ignore messages for other sessions (rare).
    if let Some(sid) = &json.session_id
        && sid != session_id
    {
        return Ok(());
    }

    match json.typ.as_str() {
        "stt" => {
            let t = json.text().unwrap_or("");
            println!("\r[你] {t}");
            // Stop forwarding mic once STT is in.
            listening.store(false, Ordering::Relaxed);
        }
        "llm" => {
            let emotion = json.emotion().unwrap_or("");
            let t = json.text().unwrap_or("");
            if !t.is_empty() {
                println!("\r[小智 {emotion}] {t}");
            }
        }
        "tts" => match json.state() {
            Some("start") => {
                listening.store(false, Ordering::Relaxed);
                *state = State::Speaking;
            }
            Some("sentence_start") => {
                if let Some(t) = json.text()
                    && !t.is_empty()
                {
                    println!("\r  >> {t}");
                }
            }
            Some("stop") => {
                output.flush();
                *state = State::Idle;
                println!("\r[就绪] 按回车继续对话");
            }
            other => {
                warn!(state = ?other, "unknown tts state");
            }
        },
        "hello" => {
            // server hello re-send (shouldn't happen post-handshake)
        }
        "goodbye" => {
            info!("server goodbye");
        }
        other => {
            warn!(typ = other, "ignored server message");
        }
    }
    Ok(())
}

fn build_ws_request(info: &WsConnectInfo, identity: &Identity) -> Result<HttpRequest<()>> {
    let mut req = info
        .url
        .as_str()
        .into_client_request()
        .with_context(|| format!("build ws request for {}", info.url))?;
    let headers = req.headers_mut();
    headers.insert("User-Agent", identity.user_agent().parse().unwrap());
    headers.insert("Device-Id", identity.device_id.parse().unwrap());
    headers.insert("Client-Id", identity.client_id.parse().unwrap());
    headers.insert(
        "Protocol-Version",
        info.version.to_string().parse().unwrap(),
    );
    if let Some(token) = &info.token {
        let auth = if token.contains(' ') {
            token.clone()
        } else {
            format!("Bearer {token}")
        };
        headers.insert("Authorization", auth.parse().unwrap());
    }
    Ok(req)
}
