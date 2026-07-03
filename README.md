# xiaozhi-client-rs

Rust CLI 客户端，模拟 ESP32 (`xiaozhi-esp32`) 与 `xiaozhi-server-rs` 的 WebSocket 协议交互。一期不做唤醒词检测，按回车开始说话，由服务端 VAD 检测说话结束。面向 Linux，后续可作为 systemd 服务常驻运行。

## 功能

- 启动时请求 `/ota`，解析 `websocket.url` / `token` / `version`，再连接 `/ws`（完整模拟 ESP32）
- WebSocket hello 握手，协商二进制音频协议 v1/v2/v3
- `cpal` 采集麦克风，重采样到 16k mono，Opus 编码上行（60ms/帧）
- 接收服务端 24k mono Opus TTS，解码重采样后用 `cpal` 播放
- 按回车触发 `listen.start`（mode=auto）；服务端 VAD 结束后自动进入 ASR/LLM/TTS
- 再按回车可手动 `listen.stop`；TTS 播放中按回车会 `abort` 后重新监听
- 打印 `stt` / `llm` / `tts` 状态
- `client_id` / `device_id` 首次启动自动生成并持久化到 `~/.config/xiaozhi-client-rs/config.toml`

## 构建

需要 `libopus`、`libasound` (ALSA) 的开发头文件（Nix 开发环境见 `shell.nix`）。

```bash
cargo build --release
```

## 使用

```bash
# 走 OTA（推荐，与 ESP32 一致）
cargo run -- --ota-url http://127.0.0.1:3116/ota

# 直连 ws，跳过 OTA
cargo run -- --ws-url ws://127.0.0.1:3116/ws --token dev-token

# 指定音频设备（名称子串匹配）
cargo run -- --ota-url http://127.0.0.1:3116/ota --input-device sysdefault --output-device sysdefault

# 列出音频设备
cargo run -- devices

# 详细日志
cargo run -- --ota-url http://127.0.0.1:3116/ota --verbose
# 或 RUST_LOG=xiaozhi_client_rs=debug
```

运行后按回车开始说话，听到回复后继续按回车进入下一轮。`Ctrl-C` 退出。

## 命令行参数

| 参数 | 默认 | 说明 |
|------|------|------|
| `--ota-url` | — | OTA URL，启动时请求获取 ws 配置 |
| `--ws-url` | — | 直连 ws，跳过 OTA |
| `--token` | — | 直连鉴权 token |
| `--protocol-version` | `3` | 二进制协议版本（OTA 返回时以 OTA 为准） |
| `--language` | `zh-CN` | `Accept-Language` 头 |
| `--config` | `~/.config/xiaozhi-client-rs/config.toml` | 配置文件路径 |
| `--input-device` | 默认输入 | 麦克风设备名子串 |
| `--output-device` | 默认输出 | 扬声器设备名子串 |
| `--verbose` | — | debug 日志 |
| `devices` 子命令 | — | 列出音频设备后退出 |

## 模块结构

```
src/
├── main.rs              # CLI 参数、日志初始化
├── config.rs            # 配置加载/持久化、身份生成
├── ota.rs               # /ota 请求与 ws 配置解析
├── protocol.rs          # hello/listen/abort JSON + binary v1/v2/v3 编解码
├── client.rs            # WebSocket 会话状态机、stdin 触发、事件循环
└── audio/
    ├── mod.rs           # cpal 设备枚举/选择/协商
    ├── input.rs         # 麦克风采集 → 重采样 → Opus 编码（raw stream）
    ├── output.rs        # Opus 解码 → 重采样 → 播放（raw stream）
    ├── opus_codec.rs    # Opus encoder/decoder 封装
    └── resample.rs      # 线性重采样（支持任意 chunk 大小）
```

## 后续 systemd 常驻

代码已把协议/音频核心与触发器（stdin）分离。后期把 `main.rs` 的 stdin 触发替换为 GPIO / Unix socket / DBus / 唤醒词模块即可，无需改协议与音频层。

## 已知限制（一期）

- 不做唤醒词检测
- 不做 AEC（播放 TTS 时停止录音，避免回灌）
- 仅 WebSocket 传输，不支持 MQTT+UDP
- 不支持 MCP / IoT 工具调用（hello 中 `features.mcp=false`）
