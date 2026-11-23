//! Live voice: a spoken conversation with an OpenAI realtime model that operates winfer
//! (port of `features/voice/live`). The model's tools live in `ui/voice_tools.rs`; this side is
//! the audio and the connection, with no UI.

pub mod client;
pub mod io;
pub mod protocol;
pub mod resample;

/// The voice orchestrator's sessions run under made-up pane ids, one per workspace, that no
/// workspace lists (see `ui/voice_orchestrator.rs`).
pub const ORCHESTRATOR_PREFIX: &str = "voice-orch-";

pub fn orchestrator_pane(workspace_id: &str) -> String {
    format!("{ORCHESTRATOR_PREFIX}{workspace_id}")
}

/// The workspace an orchestrator's pane id belongs to, if it is one.
pub fn orchestrator_workspace(pane_id: &str) -> Option<&str> {
    pane_id.strip_prefix(ORCHESTRATOR_PREFIX).filter(|w| !w.is_empty())
}
