//! Microphone capture on a dedicated thread (cpal streams aren't Send on every platform).

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

pub const WHISPER_RATE: u32 = 16_000;

enum Msg {
    Start(Sender<Result<(), String>>),
    Stop(Sender<Vec<f32>>),
}

#[derive(Default)]
struct Buffer {
    samples: Vec<f32>,
    rate: u32,
    /// RMS of the most recent callback, for the level meter.
    level: f32,
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

    pub fn start(&self) -> Result<(), String> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .lock()
            .unwrap()
            .send(Msg::Start(reply))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    /// Stops recording and returns mono 16 kHz samples.
    pub fn stop(&self) -> Vec<f32> {
        let (reply, rx) = mpsc::channel();
        if self.tx.lock().unwrap().send(Msg::Stop(reply)).is_err() {
            return Vec::new();
        }
        rx.recv().unwrap_or_default()
    }

    pub fn level(&self) -> f32 {
        self.buffer.lock().unwrap().level
    }
}

fn run(rx: Receiver<Msg>, buffer: Arc<Mutex<Buffer>>) {
    let mut stream: Option<cpal::Stream> = None;
    for msg in rx {
        match msg {
            Msg::Start(reply) => {
                stream = None;
                let result = open(buffer.clone()).map(|s| stream = Some(s));
                let _ = reply.send(result);
            }
            Msg::Stop(reply) => {
                stream = None; // dropping stops capture
                let mut buf = buffer.lock().unwrap();
                let samples = resample(&buf.samples, buf.rate, WHISPER_RATE);
                buf.samples.clear();
                buf.level = 0.0;
                let _ = reply.send(samples);
            }
        }
    }
    drop(stream);
}

fn open(buffer: Arc<Mutex<Buffer>>) -> Result<cpal::Stream, String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or("no microphone found")?;
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = config.channels() as usize;
    {
        let mut buf = buffer.lock().unwrap();
        buf.samples.clear();
        buf.rate = config.sample_rate();
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
    Ok(stream)
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
}
