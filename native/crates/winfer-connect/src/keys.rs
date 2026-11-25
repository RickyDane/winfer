//! What a pairing key (or a pair link's one-time key) stands for: the relay room both instances
//! meet in, the token that opens it, and the key that proves a connection knows the pairing.

use crate::crypto::{b64, hkdf, hkdf32};

pub const SALT: &[u8] = b"winfer-connect-v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomKeys {
    /// 16 bytes, base64url (22 chars).
    pub room: String,
    /// 32 bytes, base64url (43 chars), shown to the relay.
    pub relay_auth: String,
}

/// A paired instance's long-term keys.
#[derive(Clone)]
pub struct PairKeys {
    pub pair_key: [u8; 32],
    pub room: RoomKeys,
    /// Proves knowledge of the pairing in the per-connection handshake.
    pub auth_key: [u8; 32],
}

impl PairKeys {
    pub fn derive(pair_key: &[u8; 32]) -> PairKeys {
        PairKeys {
            pair_key: *pair_key,
            room: RoomKeys { room: b64(&hkdf(SALT, pair_key, b"room", 16)), relay_auth: b64(&hkdf(SALT, pair_key, b"relay", 32)) },
            auth_key: hkdf32(SALT, pair_key, b"auth"),
        }
    }
}

/// The one-time room and MAC key of a pair link.
pub struct LinkKeys {
    pub room: RoomKeys,
    pub auth_key: [u8; 32],
}

impl LinkKeys {
    pub fn derive(link_key: &[u8; 32]) -> LinkKeys {
        LinkKeys {
            room: RoomKeys { room: b64(&hkdf(SALT, link_key, b"pair-room", 16)), relay_auth: b64(&hkdf(SALT, link_key, b"pair-relay", 32)) },
            auth_key: hkdf32(SALT, link_key, b"pair-auth"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_ids_fit_the_relay() {
        let k = PairKeys::derive(&[1; 32]);
        assert_eq!(k.room.room.len(), 22);
        assert_eq!(k.room.relay_auth.len(), 43);
        let l = LinkKeys::derive(&[1; 32]);
        assert_ne!(l.room.room, k.room.room, "link rooms and pair rooms never collide");
    }
}
