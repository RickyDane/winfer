//! Pairing two instances: X25519 + a 6-digit code both screens show (numeric comparison).
//!
//! ```text
//! J → H  p1 {i, name, kind, e: X25519 public, n, m?: HMAC(linkAuth, "p1", i, e, n)}
//! H → J  p2 {i, name, kind, e, n, m?: HMAC(linkAuth, "p2", th1, i, e, n)}
//!        th = SHA256(both messages' fields); ss = X25519
//!        code     = HKDF(salt = link? ‖ th, ss, "sas") → 6 digits
//!        pairKey  = HKDF(…, "pair"), confirmKey = HKDF(…, "confirm")
//! both   p3 {m: HMAC(confirmKey, "p3", own id)}   when the user confirms
//! either px {reason}                               cancel
//! ```
//! The joiner (J) clicked Pair on a nearby instance or pasted a pair link from the host (H). With a
//! link, its one-time key authenticates both p1 and p2 and is mixed into every derived key, so an
//! attacker without the link can't get in the middle; on the LAN the code comparison does that.

use crate::crypto::{Ephemeral, b64, hkdf, hkdf32, hmac, hmac_verify, sha256, unb64, unb64_n};
use crate::keys::LinkKeys;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const PAIR_TTL_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Desktop,
    Server,
}

/// This instance as the other side sees it.
#[derive(Clone, Debug)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub kind: Kind,
}

/// A finished pairing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paired {
    pub peer_id: String,
    pub name: String,
    pub kind: Kind,
    pub pair_key: [u8; 32],
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PairError {
    Malformed,
    BadLink,
    BadConfirm,
    Cancelled(String),
    Unexpected,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Joiner,
    Host,
}

/// A hello's id, name, kind, X25519 public key and nonce.
type Fields = (String, String, Kind, [u8; 32], [u8; 16]);

struct Secrets {
    peer: (String, String, Kind),
    code: String,
    pair_key: [u8; 32],
    confirm_key: [u8; 32],
}

pub struct Pairing {
    side: Side,
    me: Me,
    link: Option<[u8; 32]>,
    eph: Option<Ephemeral>,
    nonce: [u8; 16],
    p1: Option<Value>,
    secrets: Option<Secrets>,
    confirmed_here: bool,
    confirmed_there: bool,
}

/// What the UI and transport should do after a step.
#[derive(Debug, Default)]
pub struct Out {
    pub send: Vec<Value>,
    /// The 6-digit code to show, once both keys are in.
    pub code: Option<String>,
    pub paired: Option<Paired>,
}

impl Pairing {
    /// `link`: the pair link's one-time key, on both sides of a link pairing.
    pub fn new(side: Side, me: Me, link: Option<[u8; 32]>) -> Pairing {
        Pairing { side, me, link, eph: Some(Ephemeral::new()), nonce: crate::crypto::random(), p1: None, secrets: None, confirmed_here: false, confirmed_there: false }
    }

    fn link_auth(&self) -> Option<[u8; 32]> {
        self.link.map(|k| LinkKeys::derive(&k).auth_key)
    }

    fn hello(&self, t: &str, extra: &[&[u8]]) -> Value {
        let e = self.eph.as_ref().map(|e| e.public).unwrap_or_default();
        let mut msg = json!({"t": t, "v": 1, "i": self.me.id, "name": self.me.name, "kind": self.me.kind, "e": b64(&e), "n": b64(&self.nonce)});
        if let Some(auth) = self.link_auth() {
            let mut parts: Vec<&[u8]> = vec![t.as_bytes()];
            parts.extend_from_slice(extra);
            parts.extend_from_slice(&[self.me.id.as_bytes(), &e, &self.nonce]);
            msg["m"] = json!(b64(&hmac(&auth, &parts)));
        }
        msg
    }

    /// The joiner's opening message.
    pub fn start(&mut self) -> Out {
        match self.side {
            // Once: a second p1 (the relay reconnected) would end the pairing on the host.
            Side::Joiner if self.p1.is_some() => Out::default(),
            Side::Joiner => {
                let p1 = self.hello("p1", &[]);
                self.p1 = Some(p1.clone());
                Out { send: vec![p1], ..Default::default() }
            }
            Side::Host => Out::default(),
        }
    }

    fn fields(v: &Value) -> Result<Fields, PairError> {
        let id = v["i"].as_str().filter(|s| s.len() == 22).ok_or(PairError::Malformed)?.to_string();
        let name = v["name"].as_str().unwrap_or("winfer").chars().take(64).collect();
        let kind = serde_json::from_value(v["kind"].clone()).unwrap_or_default();
        let e = unb64_n::<32>(v["e"].as_str().unwrap_or_default()).ok_or(PairError::Malformed)?;
        let n = unb64_n::<16>(v["n"].as_str().unwrap_or_default()).ok_or(PairError::Malformed)?;
        Ok((id, name, kind, e, n))
    }

    fn transcript(p1: &Value, p2: &Value) -> Result<[u8; 32], PairError> {
        let (i1, n1, _, e1, x1) = Self::fields(p1)?;
        let (i2, n2, _, e2, x2) = Self::fields(p2)?;
        Ok(sha256(&[b"winfer-connect-pair-v1", i1.as_bytes(), n1.as_bytes(), &e1, &x1, i2.as_bytes(), n2.as_bytes(), &e2, &x2]))
    }

    fn th1(p1: &Value) -> Result<[u8; 32], PairError> {
        let (i, n, _, e, x) = Self::fields(p1)?;
        Ok(sha256(&[i.as_bytes(), n.as_bytes(), &e, &x]))
    }

    fn check_link(&self, msg: &Value, t: &str, extra: &[&[u8]]) -> Result<(), PairError> {
        let Some(auth) = self.link_auth() else { return Ok(()) };
        let (i, _, _, e, n) = Self::fields(msg)?;
        let m = unb64(msg["m"].as_str().unwrap_or_default()).ok_or(PairError::BadLink)?;
        let mut parts: Vec<&[u8]> = vec![t.as_bytes()];
        parts.extend_from_slice(extra);
        parts.extend_from_slice(&[i.as_bytes(), &e, &n]);
        if hmac_verify(&auth, &parts, &m) { Ok(()) } else { Err(PairError::BadLink) }
    }

    fn derive(&mut self, p1: &Value, p2: &Value, peer_hello: &Value) -> Result<String, PairError> {
        let th = Self::transcript(p1, p2)?;
        let (id, name, kind, e, _) = Self::fields(peer_hello)?;
        if id == self.me.id {
            return Err(PairError::Unexpected);
        }
        let ss = self.eph.take().ok_or(PairError::Unexpected)?.agree(&e).ok_or(PairError::Malformed)?;
        let salt = [&self.link.unwrap_or_default()[..], &th[..]].concat();
        let sas = hkdf(&salt, &ss, b"sas", 4);
        let n = u32::from_be_bytes(sas.try_into().expect("4 bytes")) % 1_000_000;
        let code = format!("{:03} {:03}", n / 1000, n % 1000);
        self.secrets = Some(Secrets { peer: (id, name, kind), code: code.clone(), pair_key: hkdf32(&salt, &ss, b"pair"), confirm_key: hkdf32(&salt, &ss, b"confirm") });
        Ok(code)
    }

    pub fn code(&self) -> Option<&str> {
        self.secrets.as_ref().map(|s| s.code.as_str())
    }

    /// The other side's name and kind, once known.
    pub fn peer(&self) -> Option<(&str, &str, Kind)> {
        self.secrets.as_ref().map(|s| (s.peer.0.as_str(), s.peer.1.as_str(), s.peer.2))
    }

    fn done(&self) -> Option<Paired> {
        let s = self.secrets.as_ref()?;
        (self.confirmed_here && self.confirmed_there).then(|| Paired { peer_id: s.peer.0.clone(), name: s.peer.1.clone(), kind: s.peer.2, pair_key: s.pair_key })
    }

    /// The user checked the code and confirmed (or a server joining with a link, by itself).
    pub fn confirm(&mut self) -> Result<Out, PairError> {
        let s = self.secrets.as_ref().ok_or(PairError::Unexpected)?;
        let m = hmac(&s.confirm_key, &[b"p3", self.me.id.as_bytes()]);
        self.confirmed_here = true;
        Ok(Out { send: vec![json!({"t": "p3", "m": b64(&m)})], paired: self.done(), ..Default::default() })
    }

    pub fn cancel(reason: &str) -> Value {
        json!({"t": "px", "reason": reason})
    }

    pub fn on_message(&mut self, msg: &Value) -> Result<Out, PairError> {
        match (msg["t"].as_str(), self.side) {
            (Some("p1"), Side::Host) if self.p1.is_none() => {
                self.check_link(msg, "p1", &[])?;
                let th1 = Self::th1(msg)?;
                let p2 = self.hello("p2", &[&th1]);
                let code = self.derive(msg, &p2, msg)?;
                self.p1 = Some(msg.clone());
                Ok(Out { send: vec![p2], code: Some(code), ..Default::default() })
            }
            (Some("p2"), Side::Joiner) if self.secrets.is_none() => {
                let p1 = self.p1.clone().ok_or(PairError::Unexpected)?;
                self.check_link(msg, "p2", &[&Self::th1(&p1)?])?;
                let code = self.derive(&p1, msg, msg)?;
                Ok(Out { code: Some(code), ..Default::default() })
            }
            (Some("p3"), _) => {
                let s = self.secrets.as_ref().ok_or(PairError::Unexpected)?;
                let m = unb64(msg["m"].as_str().unwrap_or_default()).ok_or(PairError::Malformed)?;
                if !hmac_verify(&s.confirm_key, &[b"p3", s.peer.0.as_bytes()], &m) {
                    return Err(PairError::BadConfirm);
                }
                self.confirmed_there = true;
                Ok(Out { paired: self.done(), ..Default::default() })
            }
            (Some("px"), _) => Err(PairError::Cancelled(msg["reason"].as_str().unwrap_or("cancelled").to_string())),
            _ => Err(PairError::Unexpected),
        }
    }
}

/// A pair link: `winfer-connect:#p=<base64url(JSON)>`, JSON `{v, k, i, n, u, x}` (one-time key,
/// host instance id and name, relay URL, expiry in ms since the epoch).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairLink {
    pub v: u32,
    pub k: String,
    pub i: String,
    pub n: String,
    pub u: String,
    pub x: u64,
}

impl PairLink {
    pub fn new(host: &Me, relay: &str, now_ms: u64) -> (PairLink, [u8; 32]) {
        let key = crate::crypto::random::<32>();
        (PairLink { v: 1, k: b64(&key), i: host.id.clone(), n: host.name.clone(), u: relay.into(), x: now_ms + PAIR_TTL_MS }, key)
    }

    pub fn key(&self) -> Option<[u8; 32]> {
        unb64_n(&self.k)
    }

    pub fn to_url(&self) -> String {
        format!("winfer-connect:#p={}", b64(serde_json::to_string(self).unwrap_or_default().as_bytes()))
    }

    /// Accepts the link with or without its scheme, and surrounding whitespace or quotes.
    pub fn parse(text: &str) -> Option<PairLink> {
        let text = text.trim().trim_matches(|c| c == '\'' || c == '"');
        let payload = text.rsplit_once("p=").map_or(text, |(_, p)| p);
        let link: PairLink = serde_json::from_slice(&unb64(payload)?).ok()?;
        (link.v == 1 && link.key().is_some() && link.i.len() == 22).then_some(link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(c: char, kind: Kind) -> Me {
        Me { id: c.to_string().repeat(22), name: format!("machine-{c}"), kind }
    }

    /// Runs both sides; returns their codes and results.
    fn pair(link_j: Option<[u8; 32]>, link_h: Option<[u8; 32]>) -> Result<(String, String, Paired, Paired), PairError> {
        let mut j = Pairing::new(Side::Joiner, me('J', Kind::Server), link_j);
        let mut h = Pairing::new(Side::Host, me('H', Kind::Desktop), link_h);
        let p1 = j.start().send.remove(0);
        let mut out_h = h.on_message(&p1)?;
        let out_j = j.on_message(&out_h.send.remove(0))?;
        let (cj, ch) = (out_j.code.unwrap(), out_h.code.unwrap());
        let c_j = j.confirm()?.send.remove(0);
        let mut c_h = h.confirm()?;
        assert!(c_h.paired.is_none());
        let pj = j.on_message(&c_h.send.remove(0))?.paired.unwrap();
        let ph = h.on_message(&c_j)?.paired.unwrap();
        Ok((cj, ch, pj, ph))
    }

    #[test]
    fn lan_pairing_agrees_on_code_and_key() {
        let (cj, ch, pj, ph) = pair(None, None).unwrap();
        assert_eq!(cj, ch);
        assert_eq!(cj.len(), 7);
        assert_eq!(pj.pair_key, ph.pair_key);
        assert_eq!(pj.peer_id, "H".repeat(22));
        assert_eq!(ph.peer_id, "J".repeat(22));
        assert_eq!(ph.kind, Kind::Server);
        assert_eq!(pj.name, "machine-H");
    }

    #[test]
    fn link_pairing_needs_the_link() {
        let k = [5; 32];
        let (cj, ch, pj, ph) = pair(Some(k), Some(k)).unwrap();
        assert_eq!((cj, pj.pair_key), (ch, ph.pair_key));
        assert_eq!(pair(Some([6; 32]), Some(k)).err(), Some(PairError::BadLink));
        assert_eq!(pair(None, Some(k)).err(), Some(PairError::BadLink));
    }

    #[test]
    fn a_man_in_the_middle_shows_different_codes() {
        // Mallory talks to each side with her own keys: the codes differ (except 1 in a million).
        let mut j = Pairing::new(Side::Joiner, me('J', Kind::Desktop), None);
        let mut h = Pairing::new(Side::Host, me('H', Kind::Desktop), None);
        let mut m_h = Pairing::new(Side::Host, me('M', Kind::Desktop), None);
        let mut m_j = Pairing::new(Side::Joiner, me('N', Kind::Desktop), None);
        let p1 = j.start().send.remove(0);
        let mut to_j = m_h.on_message(&p1).unwrap();
        let code_j = j.on_message(&to_j.send.remove(0)).unwrap().code.unwrap();
        let p1m = m_j.start().send.remove(0);
        let code_h = h.on_message(&p1m).unwrap().code.unwrap();
        assert_ne!(code_j, code_h);
    }

    #[test]
    fn cancel_ends_it() {
        let mut h = Pairing::new(Side::Host, me('H', Kind::Desktop), None);
        assert_eq!(h.on_message(&Pairing::cancel("no")).err(), Some(PairError::Cancelled("no".into())));
    }

    #[test]
    fn links_round_trip() {
        let (link, key) = PairLink::new(&me('H', Kind::Desktop), "https://winfer.danehub.com", 1000);
        let url = link.to_url();
        assert!(url.starts_with("winfer-connect:#p="));
        let back = PairLink::parse(&format!("  '{url}' ")).unwrap();
        assert_eq!(back, link);
        assert_eq!(back.key().unwrap(), key);
        assert_eq!(back.x, 1000 + PAIR_TTL_MS);
        assert!(PairLink::parse("winfer-connect:#p=garbage").is_none());
    }
}
