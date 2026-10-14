//! Microphone capture on a dedicated thread (cpal streams aren't Send on every platform).

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

pub const WHISPER_RATE: u32 = 16_000;

enum Msg {
    /// Opens the named input device, or the system default when `None`.
    Start(Option<String>, Sender<Result<String, String>>),
    Stop(Sender<Recording>),
}

/// A finished recording: mono 16 kHz samples plus the loudest callback's RMS, which tells
/// "you were quiet" apart from "the device delivered nothing" (e.g. microphone access denied).
pub struct Recording {
    pub samples: Vec<f32>,
    pub peak: f32,
}

#[derive(Default)]
struct Buffer {
    samples: Vec<f32>,
    rate: u32,
    /// RMS of the most recent callback, for the level meter.
    level: f32,
    peak: f32,
}

pub struct Recorder {
    tx: Mutex<Sender<Msg>>,
    buffer: Arc<Mutex<Buffer>>,
}

impl Recorder {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let buffer = Arc::new(Mutex::new(Buffer::default()));
        let thread_buffer = buffer.clone();
        std::thread::spawn(move || run(rx, thread_buffer));
        Self {
            tx: Mutex::new(tx),
            buffer,
        }
    }

    /// Starts recording; returns the name of the device in use.
    pub fn start(&self, device: Option<String>) -> Result<String, String> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .lock()
            .unwrap()
            .send(Msg::Start(device, reply))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    /// Stops recording and returns mono 16 kHz samples.
    pub fn stop(&self) -> Recording {
        let (reply, rx) = mpsc::channel();
        let empty = || Recording {
            samples: Vec::new(),
            peak: 0.0,
        };
        if self.tx.lock().unwrap().send(Msg::Stop(reply)).is_err() {
            return empty();
        }
        rx.recv().unwrap_or_else(|_| empty())
    }

    pub fn level(&self) -> f32 {
        self.buffer.lock().unwrap().level
    }
}

fn run(rx: Receiver<Msg>, buffer: Arc<Mutex<Buffer>>) {
    let mut stream: Option<cpal::Stream> = None;
    for msg in rx {
        match msg {
            Msg::Start(device, reply) => {
                stream = None;
                let result = open(device.as_deref(), buffer.clone()).map(|(s, name)| {
                    stream = Some(s);
                    name
                });
                let _ = reply.send(result);
            }
            Msg::Stop(reply) => {
                stream = None; // dropping stops capture
                let mut buf = buffer.lock().unwrap();
                let samples = resample(&buf.samples, buf.rate, WHISPER_RATE);
                let peak = buf.peak;
                buf.samples.clear();
                buf.level = 0.0;
                buf.peak = 0.0;
                let _ = reply.send(Recording { samples, peak });
            }
        }
    }
    drop(stream);
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

/// Names of the available microphones and the system default.
pub fn input_devices() -> (Vec<String>, Option<String>) {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .input_devices()
        .map(|devices| devices.filter_map(|d| device_name(&d)).collect())
        .unwrap_or_default();
    names.dedup();
    let default = host.default_input_device().and_then(|d| device_name(&d));
    (names, default)
}

fn open(
    wanted: Option<&str>,
    buffer: Arc<Mutex<Buffer>>,
) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    // A chosen device that's gone (unplugged headset) falls back to the system default.
    let device = wanted
        .and_then(|name| {
            host.input_devices()
                .ok()?
                .find(|d| device_name(d).as_deref() == Some(name))
        })
        .or_else(|| host.default_input_device())
        .ok_or("no microphone found")?;
    let name = device_name(&device).unwrap_or_else(|| "microphone".into());
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = config.channels() as usize;
    {
        let mut buf = buffer.lock().unwrap();
        buf.samples.clear();
        buf.rate = config.sample_rate();
        buf.peak = 0.0;
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        channels: usize,
        buffer: Arc<Mutex<Buffer>>,
    ) -> Result<cpal::Stream, String>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        device
            .build_input_stream(
                config,
                move |data: &[T], _: &_| {
                    let mut buf = buffer.lock().unwrap();
                    let mut sum = 0.0;
                    for frame in data.chunks(channels) {
                        let mono = frame.iter().map(|s| f32::from_sample_(*s)).sum::<f32>()
                            / channels as f32;
                        sum += mono * mono;
                        buf.samples.push(mono);
                    }
                    buf.level = (sum / (data.len() / channels).max(1) as f32).sqrt();
                    buf.peak = buf.peak.max(buf.level);
                },
                |err| eprintln!("microphone error: {err}"),
                None,
            )
            .map_err(|e| e.to_string())
    }

    let stream_config: cpal::StreamConfig = config.config();
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, stream_config, channels, buffer),
        SampleFormat::I16 => build::<i16>(&device, stream_config, channels, buffer),
        SampleFormat::I32 => build::<i32>(&device, stream_config, channels, buffer),
        SampleFormat::U16 => build::<u16>(&device, stream_config, channels, buffer),
        other => Err(format!("unsupported microphone format {other}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, name))
}

/// Linear-interpolation resampling; plenty for speech recognition.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let len = (input.len() as f64 / ratio) as usize;
    (0..len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx];
            let b = *input.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_length_and_shape() {
        let input: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin())
            .collect();
        let out = resample(&input, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|s| s.abs() <= 1.0));
        assert_eq!(resample(&input[..10], 16_000, 16_000).len(), 10);
    }

    /// Records one second from every microphone and prints its level.
    /// Run with: cargo test -- --ignored --nocapture records_each_microphone
    #[test]
    #[ignore]
    fn records_each_microphone() {
        let (names, default) = input_devices();
        eprintln!("default: {default:?}");
        let recorder = Recorder::new();
        for name in names {
            let device = recorder.start(Some(name.clone())).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(1));
            let r = recorder.stop();
            eprintln!("{name} → {device}: {} samples, peak {:.5}", r.samples.len(), r.peak);
            assert_eq!(device, name);
        }
    }
}
