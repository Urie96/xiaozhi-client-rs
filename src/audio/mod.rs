pub mod input;
pub mod opus_codec;
pub mod output;
pub mod resample;

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};

pub fn list_devices() {
    let host = cpal::default_host();
    println!("== Input devices ==");
    if let Ok(iter) = host.input_devices() {
        for (i, d) in iter.enumerate() {
            let name = d.name().unwrap_or_else(|_| "?".into());
            println!("  [{i}] {name}");
        }
    }
    println!("== Output devices ==");
    if let Ok(iter) = host.output_devices() {
        for (i, d) in iter.enumerate() {
            let name = d.name().unwrap_or_else(|_| "?".into());
            println!("  [{i}] {name}");
        }
    }
}

pub fn pick_input(name_sub: Option<&str>) -> Option<Device> {
    let host = cpal::default_host();
    let dev = match name_sub {
        Some(sub) => {
            let mut found = None;
            if let Ok(iter) = host.input_devices() {
                for d in iter {
                    if let Ok(n) = d.name()
                        && n.to_lowercase().contains(&sub.to_lowercase())
                    {
                        found = Some(d);
                        break;
                    }
                }
            }
            found
        }
        None => host.default_input_device(),
    };
    if dev.is_none() {
        tracing::error!("no matching input device");
    }
    dev
}

pub fn pick_output(name_sub: Option<&str>) -> Option<Device> {
    let host = cpal::default_host();
    let dev = match name_sub {
        Some(sub) => {
            let mut found = None;
            if let Ok(iter) = host.output_devices() {
                for d in iter {
                    if let Ok(n) = d.name()
                        && n.to_lowercase().contains(&sub.to_lowercase())
                    {
                        found = Some(d);
                        break;
                    }
                }
            }
            found
        }
        None => host.default_output_device(),
    };
    if dev.is_none() {
        tracing::error!("no matching output device");
    }
    dev
}

/// Pick a supported config closest to the desired sample rate / channels.
/// Returns a concrete `SupportedStreamConfig` (already pinned to a sample
/// format + sample rate).
pub fn negotiate(
    dev: &Device,
    desired_rate: u32,
    desired_channels: u16,
    is_input: bool,
) -> Option<SupportedStreamConfig> {
    let supported: Vec<_> = if is_input {
        dev.supported_input_configs().ok()?.collect()
    } else {
        dev.supported_output_configs().ok()?.collect()
    };
    let mut best: Option<((u8, u8, u8, u32), SupportedStreamConfig)> = None;
    for range in supported {
        let min = range.min_sample_rate().0;
        let max = range.max_sample_rate().0;
        let can_match = desired_rate >= min && desired_rate <= max;
        let channels_ok = range.channels() == desired_channels;
        let score = (
            if channels_ok { 0u8 } else { 1u8 },
            if can_match { 0u8 } else { 1u8 },
            format_preference(range.sample_format()),
            min.abs_diff(desired_rate),
        );
        let cfg = if can_match {
            range.try_with_sample_rate(cpal::SampleRate(desired_rate))
        } else {
            Some(range.with_max_sample_rate())
        };
        if let Some(cfg) = cfg
            && best.as_ref().is_none_or(|(bs, _)| &score < bs)
        {
            best = Some((score, cfg));
        }
    }
    best.map(|(_, cfg)| cfg)
}

/// Lower is better. Prefer float/16-bit over 8-bit for quality.
fn format_preference(sf: SampleFormat) -> u8 {
    match sf {
        SampleFormat::F32 => 0,
        SampleFormat::F64 => 1,
        SampleFormat::I16 => 2,
        SampleFormat::U16 => 3,
        SampleFormat::I32 => 4,
        SampleFormat::U32 => 5,
        SampleFormat::I8 => 6,
        SampleFormat::U8 => 7,
        _ => 8,
    }
}
