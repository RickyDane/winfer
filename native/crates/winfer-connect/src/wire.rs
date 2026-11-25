//! Text frames on a transport (the relay only takes text, and drops client frames starting with `{`):
//! - `h.<base64url(JSON)>`: handshake and pairing messages, in the clear (they carry no secrets);
//! - `<base64url(AES-GCM)>`: everything after the handshake. The plaintext's first byte is `1` when
//!   more chunks of the same message follow, `0` on its last chunk.

use crate::crypto::{Sealer, b64, unb64};
use serde_json::Value;

/// Plaintext per chunk: base64 makes it ~683 KiB, under the relay's 1 MiB frame limit.
pub const CHUNK: usize = 512 * 1024;
/// Largest message a receiver assembles.
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;

pub enum Frame {
    Handshake(Value),
    Data(Vec<u8>),
}

pub fn parse(frame: &str) -> Option<Frame> {
    if let Some(rest) = frame.strip_prefix("h.") {
        return serde_json::from_slice(&unb64(rest)?).ok().map(Frame::Handshake);
    }
    unb64(frame).map(Frame::Data)
}

pub fn handshake(msg: &Value) -> String {
    format!("h.{}", b64(msg.to_string().as_bytes()))
}

/// Both directions of an established connection.
pub struct Channel {
    tx: Sealer,
    rx: Sealer,
    partial: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Not the next frame under this key: stale, replayed or forged.
    Bad,
    TooLarge,
}

impl Channel {
    pub fn new(tx_key: &[u8; 32], rx_key: &[u8; 32]) -> Channel {
        Channel { tx: Sealer::new(tx_key), rx: Sealer::new(rx_key), partial: Vec::new() }
    }

    /// One message as one or more frames.
    pub fn seal(&mut self, message: &[u8]) -> Vec<String> {
        let chunks: Vec<&[u8]> = if message.is_empty() { vec![&[][..]] } else { message.chunks(CHUNK).collect() };
        let last = chunks.len() - 1;
        chunks
            .into_iter()
            .enumerate()
            .map(|(i, chunk)| {
                let mut plain = Vec::with_capacity(chunk.len() + 1);
                plain.push(u8::from(i != last));
                plain.extend_from_slice(chunk);
                b64(&self.tx.seal(&plain))
            })
            .collect()
    }

    /// A data frame's bytes: the whole message once its last chunk is in.
    pub fn open(&mut self, sealed: &[u8]) -> Result<Option<Vec<u8>>, OpenError> {
        let plain = self.rx.open(sealed).ok_or(OpenError::Bad)?;
        let (&more, body) = plain.split_first().ok_or(OpenError::Bad)?;
        if self.partial.len() + body.len() > MAX_MESSAGE {
            self.partial.clear();
            return Err(OpenError::TooLarge);
        }
        self.partial.extend_from_slice(body);
        Ok(if more == 0 { Some(std::mem::take(&mut self.partial)) } else { None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(frame: &str) -> Vec<u8> {
        match parse(frame) {
            Some(Frame::Data(d)) => d,
            _ => panic!("not data"),
        }
    }

    #[test]
    fn frames_never_look_like_relay_control_frames() {
        let mut ch = Channel::new(&[1; 32], &[2; 32]);
        for f in ch.seal(b"{}") {
            assert!(!f.starts_with('{'));
        }
        assert!(!handshake(&serde_json::json!({"t": "hs1"})).starts_with('{'));
    }

    #[test]
    fn chunks_large_messages_and_reassembles() {
        let mut a = Channel::new(&[1; 32], &[2; 32]);
        let mut b = Channel::new(&[2; 32], &[1; 32]);
        let big: Vec<u8> = (0..CHUNK * 2 + 17).map(|i| i as u8).collect();
        let frames = a.seal(&big);
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|f| f.len() < 1 << 20));
        let mut out = None;
        for f in &frames {
            out = b.open(&data(f)).unwrap();
        }
        assert_eq!(out.unwrap(), big);
        let empty = a.seal(b"");
        assert_eq!(b.open(&data(&empty[0])).unwrap().unwrap(), b"");
    }

    #[test]
    fn handshake_frames_round_trip() {
        let msg = serde_json::json!({"t": "hs1", "n": "x"});
        match parse(&handshake(&msg)) {
            Some(Frame::Handshake(v)) => assert_eq!(v, msg),
            _ => panic!(),
        }
    }
}
