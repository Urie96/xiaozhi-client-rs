use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const CLIENT_SAMPLE_RATE: u32 = 16_000;
pub const CLIENT_CHANNELS: u16 = 1;
pub const CLIENT_FRAME_DURATION_MS: u32 = 60;
pub const SERVER_SAMPLE_RATE_DEFAULT: u32 = 24_000;
pub const SERVER_FRAME_DURATION_MS_DEFAULT: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum BinaryProtocolVersion {
    V1,
    V2,
    V3,
}

impl BinaryProtocolVersion {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::V1),
            2 => Some(Self::V2),
            3 => Some(Self::V3),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_u8(self) -> u8 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
            Self::V3 => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub timestamp: u32,
    pub payload: Vec<u8>,
}

/// Decode a server->client binary audio frame. Server always sends v1
/// (raw opus) per the protocol doc, but be lenient and accept v3 too.
pub fn decode_audio_frame(version: BinaryProtocolVersion, data: &[u8]) -> Option<AudioFrame> {
    match version {
        BinaryProtocolVersion::V1 => Some(AudioFrame {
            timestamp: 0,
            payload: data.to_vec(),
        }),
        BinaryProtocolVersion::V2 => {
            if data.len() < 16 {
                return None;
            }
            let typ = u16::from_be_bytes([data[2], data[3]]);
            if typ != 0 {
                return None;
            }
            let timestamp = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
            let payload_size =
                u32::from_be_bytes([data[12], data[13], data[14], data[15]]) as usize;
            if payload_size > data.len() - 16 {
                return None;
            }
            Some(AudioFrame {
                timestamp,
                payload: data[16..16 + payload_size].to_vec(),
            })
        }
        BinaryProtocolVersion::V3 => {
            if data.len() < 4 {
                return None;
            }
            let typ = data[0];
            if typ != 0 {
                return None;
            }
            let payload_size = u16::from_be_bytes([data[2], data[3]]) as usize;
            if payload_size > data.len() - 4 {
                return None;
            }
            Some(AudioFrame {
                timestamp: 0,
                payload: data[4..4 + payload_size].to_vec(),
            })
        }
    }
}

/// Encode a client->server binary audio frame.
pub fn encode_audio_frame(version: BinaryProtocolVersion, frame: &AudioFrame) -> Vec<u8> {
    match version {
        BinaryProtocolVersion::V1 => frame.payload.clone(),
        BinaryProtocolVersion::V2 => {
            let mut out = Vec::with_capacity(16 + frame.payload.len());
            out.extend_from_slice(&2u16.to_be_bytes()); // version
            out.extend_from_slice(&0u16.to_be_bytes()); // type
            out.extend_from_slice(&0u32.to_be_bytes()); // reserved
            out.extend_from_slice(&frame.timestamp.to_be_bytes());
            out.extend_from_slice(&(frame.payload.len() as u32).to_be_bytes());
            out.extend_from_slice(&frame.payload);
            out
        }
        BinaryProtocolVersion::V3 => {
            let mut out = Vec::with_capacity(4 + frame.payload.len());
            out.push(0); // type
            out.push(0); // reserved
            out.extend_from_slice(&(frame.payload.len() as u16).to_be_bytes());
            out.extend_from_slice(&frame.payload);
            out
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct IncomingJson {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(rename = "type")]
    pub typ: String,
    #[serde(flatten)]
    pub extra: Value,
}

impl IncomingJson {
    pub fn state(&self) -> Option<&str> {
        self.extra.get("state")?.as_str()
    }
    pub fn text(&self) -> Option<&str> {
        self.extra.get("text")?.as_str()
    }
    pub fn emotion(&self) -> Option<&str> {
        self.extra.get("emotion")?.as_str()
    }
    #[allow(dead_code)]
    pub fn transport(&self) -> Option<&str> {
        self.extra.get("transport")?.as_str()
    }
    pub fn audio_params(&self) -> Option<Value> {
        self.extra.get("audio_params").cloned()
    }
}

pub fn hello(version: u8) -> Value {
    json!({
        "type": "hello",
        "version": version,
        "features": { "mcp": false, "aec": false },
        "transport": "websocket",
        "audio_params": {
            "format": "opus",
            "sample_rate": CLIENT_SAMPLE_RATE,
            "channels": CLIENT_CHANNELS,
            "frame_duration": CLIENT_FRAME_DURATION_MS,
        }
    })
}

pub fn listen_start(session_id: &str, mode: &str) -> Value {
    json!({
        "session_id": session_id,
        "type": "listen",
        "state": "start",
        "mode": mode,
    })
}

pub fn listen_stop(session_id: &str) -> Value {
    json!({ "session_id": session_id, "type": "listen", "state": "stop" })
}

pub fn abort(session_id: &str, reason: &str) -> Value {
    json!({ "session_id": session_id, "type": "abort", "reason": reason })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AudioParams {
    pub format: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_duration: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_roundtrip() {
        let frame = AudioFrame {
            timestamp: 0,
            payload: vec![1, 2, 3, 4],
        };
        let enc = encode_audio_frame(BinaryProtocolVersion::V3, &frame);
        let dec = decode_audio_frame(BinaryProtocolVersion::V3, &enc).unwrap();
        assert_eq!(dec.payload, frame.payload);
    }

    #[test]
    fn v1_raw() {
        let frame = AudioFrame {
            timestamp: 0,
            payload: vec![9, 9],
        };
        let enc = encode_audio_frame(BinaryProtocolVersion::V1, &frame);
        assert_eq!(enc, vec![9, 9]);
    }
}
