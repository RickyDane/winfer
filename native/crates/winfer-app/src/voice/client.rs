//! The live voice connection: microphone audio up, the model's voice, transcripts and function
//! calls down, over the Realtime API's WebSocket (replaces the webview's WebRTC peer).

use super::protocol::{self, Call};
use super::resample;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Listening,
    Thinking,
    Speaking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    User,
    Assistant,
}

/// What the connection reports to the app.
#[derive(Clone, Debug)]
pub enum Up {
    State(State),
    Line(Who, String),
    /// The model called functions; answer with [`Down::Results`].
    Calls(Vec<Call>),
    /// A failure; `fatal` ones aren't worth retrying.
    Error { text: String, fatal: bool },
    Closed,
}

/// What the app tells the connection.
pub enum Down {
    Results(Vec<(String, String)>),
    /// Something that happened in winfer, for the model to mention when it matters.
    Note(String),
    UpdateSession(Value),
    Mute(bool),
    Stop,
}

/// The model's voice waiting to play, at the stream's rate.
#[derive(Default)]
pub struct Player {
    queue: Mutex<VecDeque<f32>>,
}

impl Player {
    pub fn push(&self, samples: &[f32]) {
        self.queue.lock().extend(samples.iter().copied());
    }

    /// Drops what hasn't played (the user spoke over it).
    pub fn clear(&self) {
        self.queue.lock().clear();
    }

    /// Fills `out`; whatever isn't there is silence.
    pub fn pull(&self, out: &mut [f32]) {
        let mut q = self.queue.lock();
        for o in out {
            *o = q.pop_front().unwrap_or(0.0);
        }
    }

}

pub struct Config {
    pub url: String,
    pub key: String,
    pub session: Value,
}

pub struct Handle {
    down: mpsc::UnboundedSender<Down>,
}

impl Handle {
    pub fn send(&self, d: Down) {
        let _ = self.down.send(d);
    }
}

/// Connects and runs until stopped or the socket closes. `capture` delivers microphone audio as
/// mono samples at 24 kHz; the model's voice goes into `player`.
pub fn start(config: Config, up: async_channel::Sender<Up>, capture: mpsc::UnboundedReceiver<Vec<f32>>, player: Arc<Player>) -> Handle {
    let (down, down_rx) = mpsc::unbounded_channel();
    tauri::async_runtime::spawn(async move {
        let report = up.clone();
        if let Err(text) = run(config, up, capture, player, down_rx).await {
            let _ = report.send(Up::Error { text, fatal: false }).await;
        }
        let _ = report.send(Up::Closed).await;
    });
    Handle { down }
}

async fn run(config: Config, up: async_channel::Sender<Up>, mut capture: mpsc::UnboundedReceiver<Vec<f32>>, player: Arc<Player>, mut down: mpsc::UnboundedReceiver<Down>) -> Result<(), String> {
    let mut request = config.url.as_str().into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert("Authorization", format!("Bearer {}", config.key).parse().map_err(|_| "the API key has characters a header can't carry".to_string())?);
    let (ws, _) = tokio::time::timeout(std::time::Duration::from_secs(15), tokio_tungstenite::connect_async(request))
        .await
        .map_err(|_| "Timed out connecting to OpenAI.".to_string())?
        .map_err(|e| format!("Couldn't connect to OpenAI: {e}"))?;
    let (mut sink, mut stream) = ws.split();
    let send = |v: Value| Message::text(v.to_string());
    sink.send(send(protocol::session_update(&config.session))).await.map_err(|e| e.to_string())?;
    let _ = up.send(Up::State(State::Listening)).await;

    let (mut responding, mut user_speaking, mut muted) = (false, false, false);
    let mut notes: Vec<String> = Vec::new();
    let mut pending_calls: usize = 0;
    let mut outputs: Vec<(String, String)> = Vec::new();
    let mut speaking = false;
    loop {
        tokio::select! {
            chunk = capture.recv() => {
                let Some(samples) = chunk else { break };
                if !muted {
                    sink.send(send(protocol::audio_append(&resample::to_pcm16(&samples)))).await.map_err(|e| e.to_string())?;
                }
            }
            message = down.recv() => {
                match message {
                    None | Some(Down::Stop) => break,
                    Some(Down::Mute(m)) => muted = m,
                    Some(Down::UpdateSession(s)) => sink.send(send(protocol::session_update(&s))).await.map_err(|e| e.to_string())?,
                    Some(Down::Note(text)) => {
                        notes.push(text);
                    }
                    Some(Down::Results(results)) => {
                        pending_calls = pending_calls.saturating_sub(results.len());
                        outputs.extend(results);
                        if pending_calls == 0 {
                            for (id, output) in outputs.drain(..) {
                                sink.send(send(protocol::function_output(&id, &output))).await.map_err(|e| e.to_string())?;
                            }
                            sink.send(send(protocol::response_create())).await.map_err(|e| e.to_string())?;
                        }
                    }
                }
            }
            incoming = stream.next() => {
                let Some(incoming) = incoming else { break };
                let text = match incoming.map_err(|e| e.to_string())? {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Ok(event) = serde_json::from_str::<Value>(&text) else { continue };
                match event["type"].as_str().unwrap_or_default() {
                    "input_audio_buffer.speech_started" => {
                        user_speaking = true;
                        // Talking over the model: stop its voice at once.
                        player.clear();
                        speaking = false;
                        let _ = up.send(Up::State(State::Listening)).await;
                    }
                    "input_audio_buffer.speech_stopped" => user_speaking = false,
                    "response.created" => {
                        responding = true;
                        let _ = up.send(Up::State(State::Thinking)).await;
                    }
                    "response.output_audio.delta" | "response.audio.delta" => {
                        use base64::Engine;
                        if let Some(bytes) = event["delta"].as_str().and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok()) {
                            player.push(&resample::from_pcm16(&bytes));
                            if !speaking {
                                speaking = true;
                                let _ = up.send(Up::State(State::Speaking)).await;
                            }
                        }
                    }
                    "conversation.item.input_audio_transcription.completed" => {
                        if let Some(t) = event["transcript"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
                            let _ = up.send(Up::Line(Who::User, t.to_string())).await;
                        }
                    }
                    "response.output_audio_transcript.done" | "response.audio_transcript.done" => {
                        if let Some(t) = event["transcript"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
                            let _ = up.send(Up::Line(Who::Assistant, t.to_string())).await;
                        }
                    }
                    "response.done" => {
                        responding = false;
                        speaking = false;
                        let calls = protocol::calls_of(&event["response"]);
                        if event["response"]["status"] == "failed" {
                            let error = &event["response"]["status_details"]["error"];
                            let fatal = protocol::fatal_message(error);
                            let text = fatal.clone().unwrap_or_else(|| error["message"].as_str().unwrap_or("OpenAI couldn't answer.").to_string());
                            let _ = up.send(Up::Error { text, fatal: fatal.is_some() }).await;
                        }
                        if calls.is_empty() {
                            let _ = up.send(Up::State(State::Listening)).await;
                        } else {
                            pending_calls += calls.len();
                            let _ = up.send(Up::State(State::Thinking)).await;
                            let _ = up.send(Up::Calls(calls)).await;
                        }
                    }
                    "error" => {
                        let error = &event["error"];
                        let fatal = protocol::fatal_message(error);
                        let text = fatal.clone().unwrap_or_else(|| error["message"].as_str().unwrap_or("OpenAI reported an error.").to_string());
                        let _ = up.send(Up::Error { text, fatal: fatal.is_some() }).await;
                        if fatal.is_some() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        // Events from the app wait until the model is free.
        if !responding && !user_speaking && pending_calls == 0 && !notes.is_empty() {
            let text = notes.drain(..).map(|n| format!("[winfer] {n}")).collect::<Vec<_>>().join("\n");
            sink.send(send(protocol::user_note(&text))).await.map_err(|e| e.to_string())?;
            sink.send(send(protocol::response_create())).await.map_err(|e| e.to_string())?;
        }
    }
    let _ = sink.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    impl super::Player {
        fn pending(&self) -> usize {
            self.queue.lock().len()
        }
    }

    use super::*;
    use serde_json::json;
    use tokio::net::TcpListener;

    /// A stand-in for the Realtime API: collects what the client sends, plays a script back.
    async fn mock(script: Vec<Value>) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_hdr_async(socket, |req: &tokio_tungstenite::tungstenite::handshake::server::Request, res| {
                assert_eq!(req.headers().get("authorization").and_then(|v| v.to_str().ok()), Some("Bearer sk-test"));
                Ok(res)
            })
            .await
            .unwrap();
            let mut received = Vec::new();
            let mut script = script.into_iter();
            while let Some(Ok(msg)) = ws.next().await {
                let Message::Text(t) = msg else { continue };
                let v: Value = serde_json::from_str(&t).unwrap();
                let kind = v["type"].as_str().unwrap_or_default().to_string();
                received.push(v);
                // Each of these messages from the client triggers the next scripted step.
                if kind == "session.update" || kind == "input_audio_buffer.append" || kind == "response.create" {
                    if let Some(next) = script.next() {
                        ws.send(Message::text(next.to_string())).await.unwrap();
                    }
                }
                if kind == "response.create" && script.len() == 0 {
                    break;
                }
            }
            received
        });
        (url, task)
    }

    async fn next_up(rx: &async_channel::Receiver<Up>) -> Up {
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.expect("an event in time").expect("channel open")
    }

    #[tokio::test]
    async fn a_conversation_with_a_tool_call() {
        use base64::Engine;
        let voice = base64::engine::general_purpose::STANDARD.encode(resample::to_pcm16(&[0.5, -0.5, 0.25]));
        let script = vec![
            json!({ "type": "input_audio_buffer.speech_started" }),
            json!({ "type": "conversation.item.input_audio_transcription.completed", "transcript": "what are my agents doing" }),
            json!({ "type": "response.output_audio.delta", "delta": voice }),
            json!({ "type": "response.done", "response": { "status": "completed", "output": [{ "type": "function_call", "call_id": "c1", "name": "list_panes", "arguments": "{}" }] } }),
            json!({ "type": "response.output_audio_transcript.done", "transcript": "Two agents are working." }),
        ];
        let (url, server) = mock(script).await;
        let (up_tx, up_rx) = async_channel::unbounded();
        let (cap_tx, cap_rx) = mpsc::unbounded_channel();
        let player = Arc::new(Player::default());
        let session = protocol::session("be brief", &[], "marin", false);
        let handle = start(Config { url, key: "sk-test".into(), session }, up_tx, cap_rx, player.clone());
        assert!(matches!(next_up(&up_rx).await, Up::State(State::Listening)));
        // Microphone audio goes up as base64 PCM and drives the mock's script forward.
        cap_tx.send(vec![0.1; 480]).unwrap();
        loop {
            match next_up(&up_rx).await {
                Up::Line(Who::User, t) => {
                    assert_eq!(t, "what are my agents doing");
                    break;
                }
                Up::State(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        cap_tx.send(vec![0.1; 480]).unwrap();
        cap_tx.send(vec![0.1; 480]).unwrap();
        let calls = loop {
            match next_up(&up_rx).await {
                Up::Calls(c) => break c,
                Up::State(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        };
        assert_eq!((calls[0].id.as_str(), calls[0].name.as_str()), ("c1", "list_panes"));
        assert_eq!(player.pending(), 3);
        handle.send(Down::Results(vec![("c1".into(), "Workspace: 2 panes".into())]));
        let received = server.await.unwrap();
        let kinds: Vec<&str> = received.iter().map(|v| v["type"].as_str().unwrap()).collect();
        assert_eq!(kinds[0], "session.update");
        assert!(kinds.contains(&"input_audio_buffer.append"));
        let out = received.iter().find(|v| v["type"] == "conversation.item.create").expect("the tool result");
        assert_eq!((out["item"]["call_id"].as_str(), out["item"]["output"].as_str()), (Some("c1"), Some("Workspace: 2 panes")));
        assert_eq!(*kinds.last().unwrap(), "response.create");
    }

    #[tokio::test]
    async fn speaking_over_the_model_clears_its_voice() {
        let script = vec![json!({ "type": "input_audio_buffer.speech_started" })];
        let (url, _server) = mock(script).await;
        let (up_tx, up_rx) = async_channel::unbounded();
        let (_cap_tx, cap_rx) = mpsc::unbounded_channel();
        let player = Arc::new(Player::default());
        player.push(&[0.3; 100]);
        let handle = start(Config { url, key: "sk-test".into(), session: json!({}) }, up_tx, cap_rx, player.clone());
        assert!(matches!(next_up(&up_rx).await, Up::State(State::Listening)));
        assert!(matches!(next_up(&up_rx).await, Up::State(State::Listening)));
        assert_eq!(player.pending(), 0);
        handle.send(Down::Stop);
        loop {
            if matches!(next_up(&up_rx).await, Up::Closed) {
                break;
            }
        }
    }

    #[tokio::test]
    async fn a_refused_connection_is_reported() {
        let (up_tx, up_rx) = async_channel::unbounded();
        let (_cap_tx, cap_rx) = mpsc::unbounded_channel();
        let _handle = start(Config { url: "ws://127.0.0.1:1".into(), key: "k".into(), session: json!({}) }, up_tx, cap_rx, Arc::new(Player::default()));
        assert!(matches!(next_up(&up_rx).await, Up::Error { .. }));
        assert!(matches!(next_up(&up_rx).await, Up::Closed));
    }

    #[test]
    fn the_player_pads_with_silence() {
        let p = Player::default();
        p.push(&[1.0, 2.0]);
        let mut out = [9.0; 4];
        p.pull(&mut out);
        assert_eq!(out, [1.0, 2.0, 0.0, 0.0]);
    }
}
