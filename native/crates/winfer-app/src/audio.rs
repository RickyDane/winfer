//! Native audio input through cpal: the microphone list and test in Settings → Voice.

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host.input_devices().map(|list| list.filter_map(|d| d.name().ok()).collect()).unwrap_or_default();
    names.dedup();
    names
}

fn rms(samples: impl Iterator<Item = f32>) -> f32 {
    let (mut sum, mut n) = (0.0f32, 0u32);
    for s in samples {
        sum += s * s;
        n += 1;
    }
    if n == 0 { 0.0 } else { (sum / n as f32).sqrt() }
}

/// Listens for two seconds: the device's name and how loud it got. `level` follows the RMS live.
/// Blocks, so call it off the UI thread.
pub fn test_microphone(wanted: Option<String>, level: Arc<AtomicU32>) -> Result<(String, f32), String> {
    let host = cpal::default_host();
    let found = wanted.as_ref().and_then(|name| host.input_devices().ok()?.find(|d| d.name().is_ok_and(|n| n == *name)));
    let device = found.or_else(|| host.default_input_device()).ok_or("No microphone found")?;
    let name = device.name().unwrap_or_else(|_| "the microphone".into());
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let peak = Arc::new(AtomicU32::new(0));
    let note = {
        let (peak, level) = (peak.clone(), level.clone());
        move |v: f32| {
            level.store(v.to_bits(), Ordering::Relaxed);
            if v > f32::from_bits(peak.load(Ordering::Relaxed)) {
                peak.store(v.to_bits(), Ordering::Relaxed);
            }
        }
    };
    let on_error = |e| eprintln!("microphone: {e}");
    let stream_config = config.config();
    let stream = match config.sample_format() {
        SampleFormat::F32 => device.build_input_stream(&stream_config, move |d: &[f32], _: &_| note(rms(d.iter().copied())), on_error, None),
        SampleFormat::I16 => device.build_input_stream(&stream_config, move |d: &[i16], _: &_| note(rms(d.iter().map(|s| *s as f32 / 32768.0))), on_error, None),
        SampleFormat::U16 => device.build_input_stream(&stream_config, move |d: &[u16], _: &_| note(rms(d.iter().map(|s| (*s as f32 - 32768.0) / 32768.0))), on_error, None),
        other => return Err(format!("Unsupported sample format {other}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(stream);
    Ok((name, f32::from_bits(peak.load(Ordering::Relaxed))))
}

#[cfg(test)]
mod tests {
    use super::rms;

    #[test]
    fn rms_of_samples() {
        assert_eq!(rms(std::iter::empty()), 0.0);
        assert!((rms([0.5f32, -0.5].into_iter()) - 0.5).abs() < 1e-6);
    }
}
