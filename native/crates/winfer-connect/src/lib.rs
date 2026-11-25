//! Winfer Connect: pairing, encryption and transports between winfer instances
//! (docs/connect/protocol.md). Free of GPUI so the desktop app and headless `winferd` share it.

pub mod crypto;
pub mod handshake;
pub mod keys;
pub mod lan;
pub mod link;
pub mod manager;
pub mod pairing;
pub mod peers;
pub mod store;
pub mod transport;
pub mod wire;

pub use manager::{Connect, Event, PairStatus, PeerView, Status};
