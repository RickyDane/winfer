//! Voice control: hold ⌥Space (commands) or ⌥⇧Space (dictation) anywhere, speak, release.
//! Audio is transcribed locally with whisper; the frontend then asks Clef (via `voice_decide`)
//! which tool to run.

mod capture;
mod cloud;
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
}

pub struct Voice {
    recorder: capture::Recorder,
    stt: stt::Transcriber,
    recording: Mutex<Option<(Mode, Instant)>>,
    transcribing: AtomicBool,
    config: Mutex<Config>,
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
            }),
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
        let (Ok(dir), Some(spec)) = (models_dir(&app), model::spec(&stt_model)) else { return };
        if model::installed(&dir, spec) {
            if let Err(err) = voice.stt.preload(&model::path(&dir, spec)) {
                eprintln!("speech model preload failed: {err}");
            }
        }
    });
}

/// Push-to-talk pressed.
pub fn start(app: &AppHandle, mode: Mode) {
    let voice = app.state::<Voice>();
    if voice.recording.lock().unwrap().is_some() || voice.transcribing.load(Ordering::SeqCst) {
        return;
    }
    let stt_model = voice.config.lock().unwrap().stt_model.clone();
    let Some(spec) = model::spec(&stt_model) else {
        return emit_error(app, "Unknown speech model");
    };
    if !models_dir(app).is_ok_and(|dir| model::installed(&dir, spec)) {
        return emit_error(app, "Download the speech model in Settings to use voice.");
    }
    if let Err(err) = voice.recorder.start() {
        return emit_error(app, format!("Microphone unavailable: {err}"));
    }
    *voice.recording.lock().unwrap() = Some((mode, Instant::now()));
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
    let Some((mode, started)) = voice.recording.lock().unwrap().take() else {
        return;
    };
    let samples = voice.recorder.stop();
    if started.elapsed() < Duration::from_millis(250)
        || samples.len() < capture::WHISPER_RATE as usize / 4
    {
        return emit_state(app, "idle", None);
    }
    voice.transcribing.store(true, Ordering::SeqCst);
    emit_state(app, "transcribing", Some(mode));

    let app = app.clone();
    std::thread::spawn(move || {
        let voice = app.state::<Voice>();
        let (stt_model, prompt) = {
            let config = voice.config.lock().unwrap();
            (config.stt_model.clone(), config.vocabulary.clone())
        };
        let started = Instant::now();
        let result = models_dir(&app).and_then(|dir| {
            let spec = model::spec(&stt_model).ok_or("unknown model")?;
            voice
                .stt
                .transcribe(&model::path(&dir, spec), &samples, &prompt)
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

/// Registers ⌥Space / ⌥⇧Space as global push-to-talk shortcuts.
pub fn register_shortcuts(app: &AppHandle) -> Result<(), String> {
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
        .map_err(|e| format!("⌥Space is taken by another app: {e}"))?;
    shortcuts
        .register(dictation)
        .map_err(|e| format!("⌥⇧Space is taken by another app: {e}"))?;
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
    clef_model: String,
    credentials: bool,
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
        clef_model: config.clef_model.clone(),
        credentials: cloud::load().is_some(),
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
}

#[tauri::command]
pub fn voice_configure(app: AppHandle, voice: State<'_, Voice>, config: VoiceConfig) {
    let mut c = voice.config.lock().unwrap();
    if let Some(m) = config.stt_model.filter(|m| model::spec(m).is_some() && *m != c.stt_model) {
        c.stt_model = m;
        preload(&app);
    }
    if let Some(m) = config
        .clef_model
        .filter(|m| m == "clef-flash" || m == "clef")
    {
        c.clef_model = m;
    }
    if let Some(words) = config.vocabulary {
        c.vocabulary = words.join(", ");
    }
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
