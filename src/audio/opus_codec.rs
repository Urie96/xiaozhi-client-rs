use anyhow::{Context, Result};

use crate::protocol::{CLIENT_FRAME_DURATION_MS, CLIENT_SAMPLE_RATE};

pub struct OpusEncoder {
    enc: opus::Encoder,
    frame_samples: usize,
}

impl OpusEncoder {
    pub fn new() -> Result<Self> {
        let mut enc = opus::Encoder::new(
            CLIENT_SAMPLE_RATE,
            cpal_to_opus_channels(crate::protocol::CLIENT_CHANNELS),
            opus::Application::Voip,
        )
        .context("create opus encoder")?;
        // 16kHz 60ms = 960 samples per frame
        let _ = enc.set_bitrate(opus::Bitrate::Bits(32000));
        let frame_samples =
            (CLIENT_SAMPLE_RATE as usize * CLIENT_FRAME_DURATION_MS as usize) / 1000;
        Ok(Self { enc, frame_samples })
    }

    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// Encode a full frame of interleaved i16 PCM.
    pub fn encode(&mut self, pcm: &[i16], out: &mut [u8]) -> Result<usize> {
        assert_eq!(pcm.len(), self.frame_samples);
        let n = self.enc.encode(pcm, out).context("opus encode")?;
        Ok(n)
    }
}

pub struct OpusDecoder {
    dec: opus::Decoder,
    frame_samples: usize,
    #[allow(dead_code)]
    sample_rate: u32,
}

impl OpusDecoder {
    pub fn new(sample_rate: u32, channels: u16, frame_duration_ms: u32) -> Result<Self> {
        let dec = opus::Decoder::new(sample_rate, cpal_to_opus_channels(channels))
            .context("create opus decoder")?;
        let frame_samples = (sample_rate as usize * frame_duration_ms as usize) / 1000;
        Ok(Self {
            dec,
            frame_samples,
            sample_rate,
        })
    }

    #[allow(dead_code)]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// Decode an opus packet into interleaved i16 PCM. `out` must be sized
    /// for at least frame_samples * channels.
    pub fn decode(&mut self, packet: &[u8], out: &mut [i16]) -> Result<usize> {
        let n = self.dec.decode(packet, out, false).context("opus decode")?;
        Ok(n)
    }
}

fn cpal_to_opus_channels(channels: u16) -> opus::Channels {
    match channels {
        1 => opus::Channels::Mono,
        2 => opus::Channels::Stereo,
        _ => opus::Channels::Mono,
    }
}
