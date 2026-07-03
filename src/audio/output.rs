use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Data, OutputCallbackInfo, SampleFormat, Stream};
use tokio::sync::mpsc;

use crate::audio::opus_codec::OpusDecoder;
use crate::audio::resample::LinearResampler;
use crate::protocol::SERVER_FRAME_DURATION_MS_DEFAULT;

const RING_CAP_SAMPLES: usize = 48_000; // ~2s at 24k

struct Ring {
    buf: VecDeque<i16>,
    cap: usize,
}

impl Ring {
    fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap),
            cap,
        }
    }
    fn push(&mut self, samples: &[i16]) {
        for &s in samples {
            if self.buf.len() >= self.cap {
                self.buf.pop_front();
            }
            self.buf.push_back(s);
        }
    }
    fn drain(&mut self, out: &mut [i16]) {
        for slot in out.iter_mut() {
            *slot = self.buf.pop_front().unwrap_or(0);
        }
    }
    fn clear(&mut self) {
        self.buf.clear();
    }
    fn len(&self) -> usize {
        self.buf.len()
    }
}

pub struct OutputPipeline {
    pub opus_tx: mpsc::Sender<Vec<u8>>,
    pub _stream: Stream,
    pub _decode_task: tokio::task::JoinHandle<()>,
    ring: Arc<Mutex<Ring>>,
}

impl OutputPipeline {
    pub fn start(device: &cpal::Device, server_rate: u32) -> Result<Self> {
        let cfg = crate::audio::negotiate(device, server_rate, 1, false)
            .context("negotiate output config")?;
        let sample_format = cfg.sample_format();
        let dev_rate = cfg.sample_rate().0;
        let dev_channels = cfg.channels();
        tracing::info!(
            dev_rate,
            dev_channels,
            ?sample_format,
            "output device config"
        );

        let ring = Arc::new(Mutex::new(Ring::new(RING_CAP_SAMPLES)));
        let (opus_tx, mut opus_rx) = mpsc::channel::<Vec<u8>>(128);

        let ring_clone = ring.clone();
        let decode_task = tokio::spawn(async move {
            let mut decoder =
                match OpusDecoder::new(server_rate, 1, SERVER_FRAME_DURATION_MS_DEFAULT) {
                    Ok(d) => d,
                    Err(err) => {
                        tracing::error!(%err, "create opus decoder failed");
                        return;
                    }
                };
            let frame_samples = decoder.frame_samples();
            let mut pcm = vec![0i16; frame_samples];
            let mut resampler = LinearResampler::new(server_rate, dev_rate);

            while let Some(packet) = opus_rx.recv().await {
                if packet.is_empty() {
                    let mut g = ring_clone.lock().unwrap();
                    g.clear();
                    continue;
                }
                let n = match decoder.decode(&packet, &mut pcm) {
                    Ok(n) => n,
                    Err(err) => {
                        tracing::warn!(%err, "opus decode failed");
                        continue;
                    }
                };
                let decoded = &pcm[..n];
                let resampled = resampler.process(decoded);
                if !resampled.is_empty() {
                    let mut g = ring_clone.lock().unwrap();
                    g.push(&resampled);
                }
            }
        });

        let err_cb = |err: cpal::StreamError| {
            tracing::error!(%err, "output stream error");
        };

        let sf = sample_format;
        let ch = dev_channels;
        let ring_for_cb = ring.clone();
        let stream = device
            .build_output_stream_raw(
                &cfg.config(),
                sample_format,
                move |data: &mut Data, _info: &OutputCallbackInfo| {
                    let frames = data.len() / (ch as usize * sf.sample_size());
                    let mut mono = vec![0i16; frames];
                    {
                        let mut g = ring_for_cb.lock().unwrap();
                        g.drain(&mut mono);
                    }
                    fill_output_bytes(data, sf, ch, &mono);
                },
                err_cb,
                None,
            )
            .context("build output stream")?;

        stream.play().context("start output stream")?;

        Ok(Self {
            opus_tx,
            _stream: stream,
            _decode_task: decode_task,
            ring,
        })
    }

    pub fn flush(&self) {
        let mut g = self.ring.lock().unwrap();
        g.clear();
        tracing::debug!(remaining = g.len(), "output flushed");
    }
}

/// Write interleaved mono i16 samples into a raw cpal buffer, expanding to
/// `channels` and converting to the device's sample format.
fn fill_output_bytes(data: &mut Data, sf: SampleFormat, channels: u16, mono: &[i16]) {
    let ch = channels as usize;
    let bytes = data.bytes_mut();
    match sf {
        SampleFormat::I8 => {
            for (i, &s) in mono.iter().enumerate() {
                let v = (s >> 8) as i8;
                for c in 0..ch {
                    bytes[i * ch + c] = v as u8;
                }
            }
        }
        SampleFormat::U8 => {
            for (i, &s) in mono.iter().enumerate() {
                let v = (((s as i32) >> 8) + 128) as u8;
                for c in 0..ch {
                    bytes[i * ch + c] = v;
                }
            }
        }
        SampleFormat::I16 => {
            write_interleaved(bytes, ch, mono, |s| s.to_le_bytes());
        }
        SampleFormat::U16 => {
            write_interleaved(bytes, ch, mono, |s| {
                (s as u16).wrapping_add(32768).to_le_bytes()
            });
        }
        SampleFormat::I32 => {
            write_interleaved(bytes, ch, mono, |s| ((s as i32) << 16).to_le_bytes());
        }
        SampleFormat::U32 => {
            write_interleaved(bytes, ch, mono, |s| {
                ((s as i32).wrapping_add(32768) as u32).to_le_bytes()
            });
        }
        SampleFormat::F32 => {
            write_interleaved(bytes, ch, mono, |s| ((s as f32) / 32768.0).to_le_bytes());
        }
        SampleFormat::F64 => {
            write_interleaved(bytes, ch, mono, |s| ((s as f64) / 32768.0).to_le_bytes());
        }
        _ => {
            // Unsupported format: write silence (zeros). bytes already zeroed?
            for b in bytes.iter_mut() {
                *b = 0;
            }
        }
    }
}

fn write_interleaved<const N: usize>(
    bytes: &mut [u8],
    channels: usize,
    mono: &[i16],
    encode: impl Fn(i16) -> [u8; N],
) {
    let mut idx = 0;
    for &s in mono {
        let enc = encode(s);
        for _ in 0..channels {
            bytes[idx..idx + N].copy_from_slice(&enc);
            idx += N;
        }
    }
}
