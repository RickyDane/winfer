//! The per-connection handshake between two paired instances (on the relay and on the LAN alike).
//!
//! Roles are fixed so the two sides never race: the instance with the smaller id initiates. A
//! responder that (re)connects asks for a handshake with `hs0` (no content, just a nudge).
//! ```text
//! R → I  hs0                                     (optional)
//! I → R  hs1 {i: I, n: Ni}
//! R → I  hs2 {i: R, n: Nr, m: HMAC(auth, "resp", Ni, Nr, I, R)}
//! I → R  hs3 {m: HMAC(auth, "init", Ni, Nr, I, R)}
//! ```
//! Both nonces are fresh, so recorded messages are useless; each direction gets its own key,
//! `HKDF(salt = Ni‖Nr, pairKey, "i2r" | "r2i")`, with counter nonces (see `crypto::Sealer`).

use crate::crypto::{b64, hkdf32, hmac, hmac_verify, random, unb64_n};
use crate::keys::PairKeys;
use crate::wire::Channel;
use serde_json::{Value, json};

pub const VERSION: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Responder,
}

impl Role {
    pub fn of(me: &str, peer: &str) -> Role {
        if me < peer { Role::Initiator } else { Role::Responder }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HsError {
    Malformed,
    WrongPeer,
    BadProof,
    Unexpected,
}

enum State {
    Idle,
    /// Initiator: sent hs1 with this nonce.
    Sent([u8; 16]),
    /// Responder: answered hs1; the channel is ready once hs3 checks out.
    Answered { ni: [u8; 16], nr: [u8; 16] },
}

pub struct Handshake {
    pub role: Role,
    me: String,
    peer: String,
    keys: PairKeys,
    state: State,
}

/// What a handshake message led to.
#[derive(Default)]
pub struct Step {
    pub reply: Option<Value>,
    pub channel: Option<Channel>,
}

impl Handshake {
    pub fn new(me: &str, peer: &str, keys: PairKeys) -> Handshake {
        Handshake { role: Role::of(me, peer), me: me.into(), peer: peer.into(), keys, state: State::Idle }
    }

    /// The opening message, on every new connection or retry: the initiator's hs1, the responder's hs0.
    pub fn start(&mut self) -> Value {
        if self.role != Role::Initiator {
            self.state = State::Idle;
            return json!({"t": "hs0"});
        }
        let ni = random::<16>();
        self.state = State::Sent(ni);
        json!({"t": "hs1", "v": VERSION, "i": self.me, "n": b64(&ni)})
    }

    fn ids(&self) -> (&[u8], &[u8]) {
        match self.role {
            Role::Initiator => (self.me.as_bytes(), self.peer.as_bytes()),
            Role::Responder => (self.peer.as_bytes(), self.me.as_bytes()),
        }
    }

    fn channel(&self, ni: &[u8; 16], nr: &[u8; 16]) -> Channel {
        let salt = [&ni[..], &nr[..]].concat();
        let i2r = hkdf32(&salt, &self.keys.pair_key, b"i2r");
        let r2i = hkdf32(&salt, &self.keys.pair_key, b"r2i");
        match self.role {
            Role::Initiator => Channel::new(&i2r, &r2i),
            Role::Responder => Channel::new(&r2i, &i2r),
        }
    }

    pub fn on_message(&mut self, msg: &Value) -> Result<Step, HsError> {
        let nonce = |v: &Value| unb64_n::<16>(v["n"].as_str().unwrap_or_default()).ok_or(HsError::Malformed);
        let mac = |v: &Value| crate::crypto::unb64(v["m"].as_str().unwrap_or_default()).ok_or(HsError::Malformed);
        match (msg["t"].as_str(), self.role) {
            (Some("hs0"), Role::Initiator) => Ok(Step { reply: Some(self.start()), channel: None }),
            (Some("hs1"), Role::Responder) => {
                if msg["i"].as_str() != Some(self.peer.as_str()) {
                    return Err(HsError::WrongPeer);
                }
                // A new hs1 always restarts: the initiator reconnected.
                let ni = nonce(msg)?;
                let nr = random::<16>();
                let (i, r) = self.ids();
                let m = hmac(&self.keys.auth_key, &[b"resp", &ni, &nr, i, r]);
                self.state = State::Answered { ni, nr };
                Ok(Step { reply: Some(json!({"t": "hs2", "v": VERSION, "i": self.me, "n": b64(&nr), "m": b64(&m)})), channel: None })
            }
            (Some("hs2"), Role::Initiator) => {
                let State::Sent(ni) = self.state else { return Err(HsError::Unexpected) };
                if msg["i"].as_str() != Some(self.peer.as_str()) {
                    return Err(HsError::WrongPeer);
                }
                let nr = nonce(msg)?;
                let (i, r) = self.ids();
                if !hmac_verify(&self.keys.auth_key, &[b"resp", &ni, &nr, i, r], &mac(msg)?) {
                    return Err(HsError::BadProof);
                }
                let m = hmac(&self.keys.auth_key, &[b"init", &ni, &nr, i, r]);
                let channel = self.channel(&ni, &nr);
                self.state = State::Idle;
                Ok(Step { reply: Some(json!({"t": "hs3", "m": b64(&m)})), channel: Some(channel) })
            }
            (Some("hs3"), Role::Responder) => {
                let State::Answered { ni, nr } = self.state else { return Err(HsError::Unexpected) };
                let (i, r) = self.ids();
                if !hmac_verify(&self.keys.auth_key, &[b"init", &ni, &nr, i, r], &mac(msg)?) {
                    return Err(HsError::BadProof);
                }
                let channel = self.channel(&ni, &nr);
                self.state = State::Idle;
                Ok(Step { reply: None, channel: Some(channel) })
            }
            _ => Err(HsError::Unexpected),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Frame, parse};

    const A: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const B: &str = "BBBBBBBBBBBBBBBBBBBBBB";

    fn pair(key: [u8; 32]) -> (Handshake, Handshake) {
        (Handshake::new(A, B, PairKeys::derive(&key)), Handshake::new(B, A, PairKeys::derive(&key)))
    }

    fn run(i: &mut Handshake, r: &mut Handshake) -> (Channel, Channel) {
        let hs1 = i.start();
        assert_eq!(r.start(), json!({"t": "hs0"}));
        let hs2 = r.on_message(&hs1).unwrap().reply.unwrap();
        let step = i.on_message(&hs2).unwrap();
        let ci = step.channel.unwrap();
        let cr = r.on_message(&step.reply.unwrap()).unwrap().channel.unwrap();
        (ci, cr)
    }

    fn open(ch: &mut Channel, frame: &str) -> Vec<u8> {
        let Some(Frame::Data(d)) = parse(frame) else { panic!() };
        ch.open(&d).unwrap().unwrap()
    }

    #[test]
    fn establishes_both_directions() {
        let (mut i, mut r) = pair([9; 32]);
        assert_eq!(i.role, Role::Initiator);
        let (mut ci, mut cr) = run(&mut i, &mut r);
        let f = ci.seal(b"to responder");
        assert_eq!(open(&mut cr, &f[0]), b"to responder");
        let f = cr.seal(b"to initiator");
        assert_eq!(open(&mut ci, &f[0]), b"to initiator");
    }

    #[test]
    fn wrong_pairing_key_fails() {
        let mut i = Handshake::new(A, B, PairKeys::derive(&[1; 32]));
        let mut r = Handshake::new(B, A, PairKeys::derive(&[2; 32]));
        let hs2 = r.on_message(&i.start()).unwrap().reply.unwrap();
        assert_eq!(i.on_message(&hs2).err(), Some(HsError::BadProof));
    }

    #[test]
    fn replayed_handshake_gets_nowhere() {
        let (mut i, mut r) = pair([9; 32]);
        let hs1 = i.start();
        let hs2 = r.on_message(&hs1).unwrap().reply.unwrap();
        let hs3 = i.on_message(&hs2).unwrap().reply.unwrap();
        r.on_message(&hs3).unwrap();
        // An attacker replays the old hs1 and hs3: the responder's fresh nonce breaks the old proof.
        r.on_message(&hs1).unwrap();
        assert_eq!(r.on_message(&hs3).err(), Some(HsError::BadProof));
    }

    #[test]
    fn responder_can_ask_for_a_handshake() {
        let (mut i, mut r) = pair([9; 32]);
        let hs1 = i.on_message(&r.start()).unwrap().reply.unwrap();
        assert_eq!(hs1["t"], "hs1");
        assert!(r.on_message(&hs1).unwrap().reply.is_some());
    }

    #[test]
    fn rejects_other_instances() {
        let (mut i, _) = pair([9; 32]);
        let mut stranger = Handshake::new("DDDDDDDDDDDDDDDDDDDDDD", "CCCCCCCCCCCCCCCCCCCCCC", PairKeys::derive(&[9; 32]));
        assert_eq!(stranger.on_message(&i.start()).err(), Some(HsError::WrongPeer));
    }
}
