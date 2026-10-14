//! Voice control: hold ⌥Space (commands) or ⌥⇧Space (dictation) anywhere, speak, release.
//! Audio is transcribed locally with whisper; the frontend then asks Clef (via `voice_decide`)
//! which tool to run.

mod capture;
pub mod cloud;
pub mod model;
mod stt;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Spoken commands routed through Clef.
    Command,
    /// Spoken text typed into the focused pane.
    Dictation,
}

struct Config {
    stt_model: String,
    clef_model: String,
    vocabulary: String,
    /// "en", "de" or "auto" (detect). English-only models always use "en".
    language: String,
    /// Microphone name; `None` follows the system default input.
    input_device: Option<String>,
}

pub struct Voice {
    recorder: capture::Recorder,
    stt: stt::Transcriber,
    /// Mode, start time and the device being recorded from.
    recording: Mutex<Option<(Mode, Instant, String)>>,
    transcribing: AtomicBool,
    config: Mutex<Config>,
    /// Why the global push-to-talk keys couldn't be registered, shown in Settings.
    shortcut_error: Mutex<Option<String>>,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            recorder: capture::Recorder::new(),
            stt: stt::Transcriber::new(),
            recording: Mutex::new(None),
            transcribing: AtomicBool::new(false),
            config: Mutex::new(Config {
                stt_model: model::DEFAULT_MODEL.into(),
                clef_model: "clef-flash".into(),
                vocabulary: String::new(),
                language: "en".into(),
                input_device: None,
            }),
            shortcut_error: Mutex::new(None),
        }
    }
}

fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models"))
}

fn emit_state(app: &AppHandle, state: &str, mode: Option<Mode>) {
    let _ = app.emit("voice://state", json!({ "state": state, "mode": mode }));
}

fn emit_error(app: &AppHandle, message: impl Into<String>) {
    let _ = app.emit("voice://error", json!({ "message": message.into() }));
    emit_state(app, "idle", None);
}

/// Releases the speech model before exit (see `Transcriber::unload`).
pub fn shutdown(app: &AppHandle) {
    app.state::<Voice>().stt.unload();
}

/// Warms up the configured speech model in the background, if it's installed.
pub fn preload(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let voice = app.state::<Voice>();
        let stt_model = voice.config.lock().unwrap().stt_model.clone();
        let (Ok(dir), Some(spec)) = (models_dir(&app), model::spec(&stt_model)) else {
            return;
        };
        if model::installed(&dir, spec) {
            if let Err(err) = voice.stt.preload(&model::path(&dir, spec)) {
                eprintln!("speech model preload failed: {err}");
            }
        }
    });
}

/// Below this RMS the device delivered digital silence: even a quiet room is far louder.
const SILENT: f32 = 1e-6;

fn no_sound(device: &str) -> String {
    if cfg!(target_os = "macos") {
        format!(
            "No sound from “{device}”. Allow winfer in System Settings › Privacy & Security › Microphone, or pick another microphone in Settings."
        )
    } else {
        format!("No sound from “{device}”. Check that it isn't muted, or pick another microphone in Settings.")
    }
}

/// Push-to-talk pressed.
pub fn start(app: &AppHandle, mode: Mode) {
    let voice = app.state::<Voice>();
    if voice.recording.lock().unwrap().is_some() || voice.transcribing.load(Ordering::SeqCst) {
        return;
    }
    let (stt_model, input_device) = {
        let config = voice.config.lock().unwrap();
        (config.stt_model.clone(), config.input_device.clone())
    };
    let Some(spec) = model::spec(&stt_model) else {
        return emit_error(app, "Unknown speech model");
    };
    if !models_dir(app).is_ok_and(|dir| model::installed(&dir, spec)) {
        return emit_error(app, "Download the speech model in Settings to use voice.");
    }
    let device = match voice.recorder.start(input_device) {
        Ok(device) => device,
        Err(err) => return emit_error(app, format!("Microphone unavailable: {err}")),
    };
    *voice.recording.lock().unwrap() = Some((mode, Instant::now(), device));
    emit_state(app, "listening", Some(mode));

    // Level meter for the HUD while recording.
    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let voice = app.state::<Voice>();
            if voice.recording.lock().unwrap().is_none() {
                break;
            }
            let _ = app.emit("voice://level", voice.recorder.level());
        }
    });
}

/// Push-to-talk released: transcribe on a worker thread and emit the text.
pub fn stop(app: &AppHandle) {
    let voice = app.state::<Voice>();
    let Some((mode, started, device)) = voice.recording.lock().unwrap().take() else {
        return;
    };
    let capture::Recording { samples, peak } = voice.recorder.stop();
    if started.elapsed() < Duration::from_millis(250)
        || samples.len() < capture::WHISPER_RATE as usize / 4
    {
        return emit_state(app, "idle", None);
    }
    if peak < SILENT {
        return emit_error(app, no_sound(&device));
    }
    voice.transcribing.store(true, Ordering::SeqCst);
    emit_state(app, "transcribing", Some(mode));

    let app = app.clone();
    std::thread::spawn(move || {
        let voice = app.state::<Voice>();
        let (stt_model, prompt, language) = {
            let config = voice.config.lock().unwrap();
            (
                config.stt_model.clone(),
                config.vocabulary.clone(),
                config.language.clone(),
            )
        };
        let started = Instant::now();
        let result = models_dir(&app).and_then(|dir| {
            let spec = model::spec(&stt_model).ok_or("unknown model")?;
            let language = if model::english_only(spec) {
                "en"
            } else {
                language.as_str()
            };
            voice
                .stt
                .transcribe(&model::path(&dir, spec), &samples, &prompt, language)
        });
        voice.transcribing.store(false, Ordering::SeqCst);
        match result {
            Ok(text) if text.is_empty() => emit_state(&app, "idle", None),
            Ok(text) => {
                let _ = app.emit("voice://transcript", json!({ "mode": mode, "text": text, "ms": started.elapsed().as_millis() as u64 }));
            }
            Err(err) => emit_error(&app, format!("Transcription failed: {err}")),
        }
    });
}

const COMMAND_KEYS: &str = if cfg!(target_os = "macos") { "⌥Space" } else { "Alt+Space" };
const DICTATION_KEYS: &str = if cfg!(target_os = "macos") {
    "⌥⇧Space"
} else {
    "Alt+Shift+Space"
};

/// Registers the global push-to-talk shortcuts; a failure is kept for Settings to show.
pub fn register_shortcuts(app: &AppHandle) {
    if let Err(err) = try_register_shortcuts(app) {
        eprintln!("voice shortcuts unavailable: {err}");
        *app.state::<Voice>().shortcut_error.lock().unwrap() = Some(err);
    }
}

/// Registers ⌥Space / ⌥⇧Space (Alt+Space / Alt+Shift+Space) as global push-to-talk shortcuts.
fn try_register_shortcuts(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{
        Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
    };

    let command = Shortcut::new(Some(Modifiers::ALT), Code::Space);
    let dictation = Shortcut::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::Space);
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, shortcut, event| {
                let mode = if *shortcut == dictation {
                    Mode::Dictation
                } else {
                    Mode::Command
                };
                match event.state() {
                    ShortcutState::Pressed => start(app, mode),
                    ShortcutState::Released => stop(app),
                }
            })
            .build(),
    )
    .map_err(|e| e.to_string())?;
    let shortcuts = app.global_shortcut();
    shortcuts
        .register(command)
        .map_err(|e| format!("{COMMAND_KEYS} is taken by another app ({e})."))?;
    shortcuts
        .register(dictation)
        .map_err(|e| format!("{DICTATION_KEYS} is taken by another app ({e})."))?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Commands

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    #[serde(flatten)]
    spec: model::ModelSpec,
    installed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStatus {
    models: Vec<ModelStatus>,
    stt_model: String,
    language: String,
    clef_model: String,
    credentials: bool,
    input_device: Option<String>,
    shortcut_error: Option<String>,
}

#[tauri::command]
pub fn voice_status(app: AppHandle, voice: State<'_, Voice>) -> Result<VoiceStatus, String> {
    let dir = models_dir(&app)?;
    let config = voice.config.lock().unwrap();
    Ok(VoiceStatus {
        models: model::MODELS
            .iter()
            .map(|m| ModelStatus {
                spec: *m,
                installed: model::installed(&dir, m),
            })
            .collect(),
        stt_model: config.stt_model.clone(),
        language: config.language.clone(),
        clef_model: config.clef_model.clone(),
        credentials: cloud::load().is_some(),
        input_device: config.input_device.clone(),
        shortcut_error: voice.shortcut_error.lock().unwrap().clone(),
    })
}

#[tauri::command]
pub async fn voice_download_model(app: AppHandle, id: String) -> Result<(), String> {
    let spec = model::spec(&id).ok_or("unknown model")?;
    let dir = models_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || model::download(&dir, spec))
        .await
        .map_err(|e| e.to_string())??;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceConfig {
    stt_model: Option<String>,
    clef_model: Option<String>,
    /// Words to bias recognition toward (providers, workspaces, panes).
    vocabulary: Option<Vec<String>>,
    language: Option<String>,
    /// Microphone name; an empty string means the system default.
    input_device: Option<String>,
}

#[tauri::command]
pub fn voice_configure(app: AppHandle, voice: State<'_, Voice>, config: VoiceConfig) {
    let mut c = voice.config.lock().unwrap();
    if let Some(m) = config
        .stt_model
        .filter(|m| model::spec(m).is_some() && *m != c.stt_model)
    {
        c.stt_model = m;
        preload(&app);
    }
    if let Some(m) = config
        .clef_model
        .filter(|m| m == "clef-flash" || m == "clef")
    {
        c.clef_model = m;
    }
    if let Some(lang) = config
        .language
        .filter(|l| matches!(l.as_str(), "en" | "de" | "auto"))
    {
        c.language = lang;
    }
    if let Some(device) = config.input_device {
        c.input_device = (!device.is_empty()).then_some(device);
    }
    if let Some(words) = config.vocabulary {
        c.vocabulary = words.join(", ");
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevices {
    devices: Vec<String>,
    default: Option<String>,
}

#[tauri::command]
pub async fn voice_input_devices() -> Result<InputDevices, String> {
    // Enumerating CoreAudio devices can block briefly; keep it off the main thread.
    let (devices, default) = tauri::async_runtime::spawn_blocking(capture::input_devices)
        .await
        .map_err(|e| e.to_string())?;
    Ok(InputDevices { devices, default })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicTest {
    device: String,
    /// Loudest RMS level heard, 0–1.
    peak: f32,
}

/// Records ~1.5 s from the configured microphone and reports how loud it was.
#[tauri::command]
pub async fn voice_test_microphone(app: AppHandle) -> Result<MicTest, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let voice = app.state::<Voice>();
        if voice.recording.lock().unwrap().is_some() {
            return Err("Voice is recording right now.".to_string());
        }
        let input_device = voice.config.lock().unwrap().input_device.clone();
        let device = voice.recorder.start(input_device)?;
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(1500) {
            std::thread::sleep(Duration::from_millis(50));
            let _ = app.emit("voice://level", voice.recorder.level());
        }
        let peak = voice.recorder.stop().peak;
        let _ = app.emit("voice://level", 0.0);
        if peak < SILENT {
            return Err(no_sound(&device));
        }
        Ok(MicTest { device, peak })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn voice_set_credentials(account_id: String, token: String) -> Result<(), String> {
    cloud::save(&account_id, &token)
}

#[tauri::command]
pub fn voice_clear_credentials() -> Result<(), String> {
    cloud::clear()
}

/// Sends a System One request to Clef and returns the answers.
#[tauri::command]
pub async fn voice_decide(voice: State<'_, Voice>, body: Value) -> Result<Value, String> {
    let creds = cloud::load()
        .ok_or("Add your Cloudflare account ID and API token in Settings to use voice commands.")?;
    let model = voice.config.lock().unwrap().clef_model.clone();
    let mut body = body;
    body["model"] = json!(model);
    tauri::async_runtime::spawn_blocking(move || cloud::decide(&creds, &model, &body))
        .await
        .map_err(|e| e.to_string())?
}

/// Round-trips a tiny request; returns latency in ms.
#[tauri::command]
pub async fn voice_test_connection(voice: State<'_, Voice>) -> Result<u64, String> {
    let started = Instant::now();
    let body = json!({
        "state": "The user said: open a new terminal",
        "questions": { "is_command": { "type": "noul", "instructions": "Is this a request to do something?" } }
    });
    voice_decide(voice, body).await?;
    Ok(started.elapsed().as_millis() as u64)
}

#[tauri::command]
pub fn voice_start(app: AppHandle, mode: Mode) {
    start(&app, mode);
}

#[tauri::command]
pub fn voice_stop(app: AppHandle) {
    stop(&app);
}
