//! Local speech-to-text with whisper.cpp (Metal on macOS).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

struct Loaded {
    path: PathBuf,
    /// Kept alive and reused: freeing a Metal-backed state mid-run trips a ggml assertion
    /// (`[rsets->data count] == 0`) and aborts the process.
    state: WhisperState,
    _ctx: WhisperContext,
}

pub struct Transcriber {
    loaded: Mutex<Option<Loaded>>,
}

impl Transcriber {
    pub fn new() -> Self {
        Self {
            loaded: Mutex::new(None),
        }
    }

    /// Frees the model. Must run before process exit: ggml's static Metal device destructor
    /// asserts that no buffers are still alive.
    pub fn unload(&self) {
        self.loaded.lock().unwrap().take();
    }

    /// Loads the model and runs one silent pass so the first real request is fast
    /// (the first-ever Metal run compiles shaders, which can take several seconds).
    pub fn preload(&self, model: &Path) -> Result<(), String> {
        self.transcribe(model, &[], "").map(|_| ())
    }

    /// Transcribes 16 kHz mono audio. `prompt` biases recognition toward known words
    /// (provider and workspace names), which fixes most misheard names.
    pub fn transcribe(
        &self,
        model: &Path,
        samples: &[f32],
        prompt: &str,
    ) -> Result<String, String> {
        let mut loaded = self.loaded.lock().unwrap();
        if loaded.as_ref().is_none_or(|l| l.path != model) {
            let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default())
                .map_err(|e| e.to_string())?;
            let state = ctx.create_state().map_err(|e| e.to_string())?;
            *loaded = Some(Loaded { path: model.to_path_buf(), state, _ctx: ctx });
        }
        let state = &mut loaded.as_mut().unwrap().state;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_no_context(true);
        params.set_single_segment(true);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_n_threads(
            std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32,
        );
        if !prompt.is_empty() {
            params.set_initial_prompt(prompt);
        }

        // Whisper expects at least ~1 s; pad short clips with silence.
        let mut audio = samples.to_vec();
        audio.resize(audio.len().max(16_000 + 1600), 0.0);
        state.full(params, &audio).map_err(|e| e.to_string())?;

        let text = state
            .as_iter()
            .filter_map(|segment| segment.to_str_lossy().ok().map(|s| s.into_owned()))
            .collect::<String>();
        Ok(clean(&text))
    }
}

/// Drops whisper's non-speech annotations like "[BLANK_AUDIO]" or "(wind blowing)".
fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_annotations() {
        assert_eq!(super::clean(" [BLANK_AUDIO] "), "");
        assert_eq!(
            super::clean(" Open three  panes (keyboard clicking)"),
            "Open three panes"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod e2e {
    use super::*;

    /// Synthesizes speech with `say`, then transcribes it with the real model.
    /// Run with: WINFER_WHISPER_MODEL=/path/ggml-base.en.bin cargo test -- --ignored
    #[test]
    #[ignore]
    fn transcribes_synthesized_command() {
        let model = PathBuf::from(std::env::var("WINFER_WHISPER_MODEL").expect("set WINFER_WHISPER_MODEL"));
        let dir = std::env::temp_dir().join(format!("winfer-say-{:x}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("speech.wav");
        let ok = std::process::Command::new("say")
            .args(["-o"])
            .arg(&wav)
            .args(["--data-format=LEI16@16000", "Open three new instances with Claude, Codex and Grok"])
            .status()
            .unwrap()
            .success();
        assert!(ok);

        // 16-bit little-endian mono PCM after the WAV header.
        let bytes = std::fs::read(&wav).unwrap();
        let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
        let samples: Vec<f32> = bytes[data..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();

        let t = Transcriber::new();
        let started = std::time::Instant::now();
        t.transcribe(&model, &samples, "Claude, Codex, Grok, Antigravity").unwrap();
        eprintln!("cold (incl. model load): {:?}", started.elapsed());
        let started = std::time::Instant::now();
        let text = t.transcribe(&model, &samples, "Claude, Codex, Grok, Antigravity").unwrap();
        eprintln!("transcript: {text:?} warm in {:?}", started.elapsed());
        let lower = text.to_lowercase();
        for word in ["open", "claude", "codex"] {
            assert!(lower.contains(word), "missing {word:?} in {text:?}");
        }
    }
}
