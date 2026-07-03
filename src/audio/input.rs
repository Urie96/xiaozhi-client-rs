use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Data, InputCallbackInfo, SampleFormat, Stream};
use tokio::sync::mpsc;

use crate::audio::opus_codec::OpusEncoder;
use crate::audio::resample::LinearResampler;
use crate::protocol::{
    AudioFrame, BinaryProtocolVersion, CLIENT_CHANNELS, CLIENT_FRAME_DURATION_MS,
    CLIENT_SAMPLE_RATE, encode_audio_frame,
};

/// Microphone capture + opus encode pipeline.
///
/// Yields *encoded* opus frames (already framed per the negotiated binary
/// protocol version) on `opus_rx`.
pub struct InputPipeline {
    pub opus_rx: mpsc::Receiver<Vec<u8>>,
    pub _stream: Stream,
    pub _encode_task: tokio::task::JoinHandle<()>,
}

impl InputPipeline {
    pub fn start(device: &cpal::Device, version: BinaryProtocolVersion) -> Result<Self> {
        let cfg = crate::audio::negotiate(device, CLIENT_SAMPLE_RATE, CLIENT_CHANNELS, true)
            .context("negotiate input config")?;
        let sample_format = cfg.sample_format();
        let dev_rate = cfg.sample_rate().0;
        let dev_channels = cfg.channels();
        tracing::info!(
            dev_rate,
            dev_channels,
            ?sample_format,
            "input device config"
        );

        let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<i16>>(64);
        let (opus_tx, opus_rx) = mpsc::channel::<Vec<u8>>(64);

        let mut enc = OpusEncoder::new()?;
        let frame_samples = enc.frame_samples();

        let encode_task = tokio::spawn(async move {
            let mut buf: Vec<i16> = Vec::with_capacity(frame_samples);
            let mut frame_idx: u32 = 0;
            while let Some(chunk) = pcm_rx.recv().await {
                buf.extend_from_slice(&chunk);
                while buf.len() >= frame_samples {
                    let mut pcm = vec![0i16; frame_samples];
                    pcm.copy_from_slice(&buf[..frame_samples]);
                    buf.drain(..frame_samples);
                    let mut out = vec![0u8; 4000];
                    match enc.encode(&pcm, &mut out) {
                        Ok(n) => {
                            let payload = out[..n].to_vec();
                            let frame = AudioFrame {
                                timestamp: frame_idx.wrapping_mul(CLIENT_FRAME_DURATION_MS),
                                payload,
                            };
                            let bytes = encode_audio_frame(version, &frame);
                            frame_idx = frame_idx.wrapping_add(1);
                            if opus_tx.send(bytes).await.is_err() {
                                return;
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%err, "opus encode failed");
                        }
                    }
                }
            }
        });

        let err_cb = |err: cpal::StreamError| {
            tracing::error!(%err, "input stream error");
        };

        let need_resample = dev_rate != CLIENT_SAMPLE_RATE;
        let mut resampler = if need_resample {
            Some(LinearResampler::new(dev_rate, CLIENT_SAMPLE_RATE))
        } else {
            None
        };
        let sf = sample_format;
        let ch = dev_channels;

        let stream = device
            .build_input_stream_raw(
                &cfg.config(),
                sample_format,
                move |data: &Data, _info: &InputCallbackInfo| {
                    let mono = bytes_to_mono_i16(data, sf, ch);
                    let final_samples = if let Some(r) = resampler.as_mut() {
                        r.process(&mono)
                    } else {
                        mono
                    };
                    if !final_samples.is_empty() {
                        let _ = pcm_tx.try_send(final_samples);
                    }
                },
                err_cb,
                None,
            )
            .context("build input stream")?;

        stream.play().context("start input stream")?;

        Ok(Self {
            opus_rx,
            _stream: stream,
            _encode_task: encode_task,
        })
    }
}

/// Interpret a raw ALSA/cpal byte buffer as interleaved samples of the given
/// format, downmix to mono, and convert to i16.
fn bytes_to_mono_i16(data: &Data, sf: SampleFormat, channels: u16) -> Vec<i16> {
    let bytes = data.bytes();
    let ch = channels.max(1) as usize;
    match sf {
        SampleFormat::I8 => {
            let samples = cast_bytes::<i8>(bytes);
            downmix(samples, ch, |s| {
                (s as i32 * 257).clamp(-32768, 32767) as i16
            })
        }
        SampleFormat::U8 => {
            let samples = cast_bytes::<u8>(bytes);
            downmix(samples, ch, |s| {
                ((s as i32 - 128) * 257).clamp(-32768, 32767) as i16
            })
        }
        SampleFormat::I16 => {
            let samples = cast_bytes::<i16>(bytes);
            downmix(samples, ch, |s| s)
        }
        SampleFormat::U16 => {
            let samples = cast_bytes::<u16>(bytes);
            downmix(samples, ch, |s| {
                (s as i32 - 32768).clamp(-32768, 32767) as i16
            })
        }
        SampleFormat::I32 => {
            let samples = cast_bytes::<i32>(bytes);
            downmix(samples, ch, |s| (s >> 16).clamp(-32768, 32767) as i16)
        }
        SampleFormat::U32 => {
            let samples = cast_bytes::<u32>(bytes);
            downmix(samples, ch, |s| ((s as i64 - 2_147_483_648) >> 16) as i16)
        }
        SampleFormat::F32 => {
            let samples = cast_bytes::<f32>(bytes);
            downmix(samples, ch, |s| {
                (s * 32767.0).clamp(-32768.0, 32767.0) as i16
            })
        }
        SampleFormat::F64 => {
            let samples = cast_bytes::<f64>(bytes);
            downmix(samples, ch, |s| {
                (s * 32767.0).clamp(-32768.0, 32767.0) as i16
            })
        }
        SampleFormat::I64 | SampleFormat::U64 => {
            tracing::warn!(?sf, "unsupported sample format, treating as silence");
            Vec::new()
        }
        _ => {
            tracing::warn!(?sf, "unsupported sample format, treating as silence");
            Vec::new()
        }
    }
}

fn cast_bytes<T>(bytes: &[u8]) -> &[T] {
    let len = bytes.len() / std::mem::size_of::<T>();
    let (_, mid, _) = unsafe { bytes.align_to::<T>() };
    if mid.len() == len {
        mid
    } else {
        // Fallback: re-interpret raw pointer (handles unaligned head).
        unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const T, len) }
    }
}

fn downmix<T: Copy>(samples: &[T], channels: usize, to_i16: impl Fn(T) -> i16) -> Vec<i16> {
    let mut out = Vec::with_capacity(samples.len() / channels);
    for chunk in samples.chunks(channels) {
        let sum: i32 = chunk.iter().map(|&s| to_i16(s) as i32).sum();
        out.push((sum / chunk.len() as i32) as i16);
    }
    out
}
