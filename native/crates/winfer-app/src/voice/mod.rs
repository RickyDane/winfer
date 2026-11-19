//! Live voice: a spoken conversation with an OpenAI realtime model that operates winfer
//! (port of `features/voice/live`). The model's tools live in `ui/voice_tools.rs`; this side is
//! the audio and the connection, with no UI.

pub mod client;
pub mod io;
pub mod protocol;
pub mod resample;
