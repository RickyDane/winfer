//! Microphone and speakers for live voice, through cpal. The streams live on a thread of their
//! own (CoreAudio's aren't `Send`) until [`Audio`] is dropped.

use super::client::Player;
use super::protocol::RATE;
use super::resample::{Resampler, downmix, rms};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tokio::sync::mpsc;

pub struct Audio {
    stop: Arc<AtomicBool>,
    /// The microphone's level, 0–1 (f32 bits).
    pub level: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Audio {
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts capturing (mono 24 kHz chunks into `capture`) and playing `player`'s audio.
pub fn start(input_device: Option<String>, capture: mpsc::UnboundedSender<Vec<f32>>, player: Arc<Player>) -> Result<Audio, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let level = Arc::new(AtomicU32::new(0));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let (stop_t, level_t) = (stop.clone(), level.clone());
    let thread = std::thread::Builder::new()
        .name("winfer-voice-audio".into())
        .spawn(move || match build(input_device, capture, player, level_t) {
            Ok(streams) => {
                let _ = ready_tx.send(Ok(()));
                while !stop_t.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                drop(streams);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })
        .map_err(|e| e.to_string())?;
    match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(Audio { stop, level, thread: Some(thread) }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("The audio devices didn't start in time.".into()),
    }
}

fn build(input_device: Option<String>, capture: mpsc::UnboundedSender<Vec<f32>>, player: Arc<Player>, level: Arc<AtomicU32>) -> Result<(cpal::Stream, cpal::Stream), String> {
    let host = cpal::default_host();
    let mic = input_device
        .and_then(|name| host.input_devices().ok()?.find(|d| d.name().is_ok_and(|n| n == name)))
        .or_else(|| host.default_input_device())
        .ok_or("No microphone found. Check System Settings → Privacy → Microphone.")?;
    let speaker = host.default_output_device().ok_or("No audio output found.")?;

    let in_config = mic.default_input_config().map_err(|e| e.to_string())?;
    let (in_rate, in_channels) = (in_config.sample_rate().0, in_config.channels() as usize);
    let in_stream_config = in_config.config();
    let mut up = Resampler::new(in_rate, RATE);
    let mut feed = move |mono: Vec<f32>| {
        level.store(rms(&mono).min(1.0).to_bits(), Ordering::Relaxed);
        let out = up.process(&mono);
        if !out.is_empty() {
            let _ = capture.send(out);
        }
    };
    let on_error = |e| eprintln!("voice input: {e}");
    let input = match in_config.sample_format() {
        SampleFormat::F32 => mic.build_input_stream(&in_stream_config, move |d: &[f32], _: &_| feed(downmix(d, in_channels)), on_error, None),
        SampleFormat::I16 => mic.build_input_stream(&in_stream_config, move |d: &[i16], _: &_| feed(downmix(&d.iter().map(|s| s.to_float_sample()).collect::<Vec<f32>>(), in_channels)), on_error, None),
        SampleFormat::U16 => mic.build_input_stream(&in_stream_config, move |d: &[u16], _: &_| feed(downmix(&d.iter().map(|s| s.to_float_sample()).collect::<Vec<f32>>(), in_channels)), on_error, None),
        other => return Err(format!("Unsupported microphone format {other}")),
    }
    .map_err(|e| format!("Couldn't open the microphone: {e}"))?;

    let out_config = speaker.default_output_config().map_err(|e| e.to_string())?;
    let (out_rate, out_channels) = (out_config.sample_rate().0, out_config.channels() as usize);
    let out_stream_config = out_config.config();
    let mut down = Resampler::new(RATE, out_rate);
    let mut ready: VecDeque<f32> = VecDeque::new();
    let mut fill = move |frames: usize| -> Vec<f32> {
        while ready.len() < frames {
            let mut chunk = vec![0.0f32; 480];
            player.pull(&mut chunk);
            ready.extend(down.process(&chunk));
        }
        ready.drain(..frames).collect()
    };
    let on_error = |e| eprintln!("voice output: {e}");
    let output = match out_config.sample_format() {
        SampleFormat::F32 => speaker.build_output_stream(
            &out_stream_config,
            move |data: &mut [f32], _: &_| {
                let mono = fill(data.len() / out_channels.max(1));
                for (frame, s) in data.chunks_exact_mut(out_channels.max(1)).zip(mono) {
                    frame.fill(s);
                }
            },
            on_error,
            None,
        ),
        SampleFormat::I16 => speaker.build_output_stream(
            &out_stream_config,
            move |data: &mut [i16], _: &_| {
                let mono = fill(data.len() / out_channels.max(1));
                for (frame, s) in data.chunks_exact_mut(out_channels.max(1)).zip(mono) {
                    frame.fill(Sample::from_sample(s));
                }
            },
            on_error,
            None,
        ),
        other => return Err(format!("Unsupported output format {other}")),
    }
    .map_err(|e| format!("Couldn't open the speakers: {e}"))?;
    input.play().map_err(|e| e.to_string())?;
    output.play().map_err(|e| e.to_string())?;
    Ok((input, output))
}
