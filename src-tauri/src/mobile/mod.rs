//! The mobile companion (docs/mobile/protocol.md): paired phones reach winfer through a
//! relay, end-to-end encrypted, to follow agents, read their threads and answer them; Web Push
//! tells them when an agent finishes or asks something.
//!
//! Rust keeps the pairings, the relay connections, the crypto and the push sending. Pane
//! state lives in the webview (src/features/mobile/): phone requests go there as
//! `mobile://request`, and it answers and reports status changes with `mobile_send`.

mod crypto;
mod devices;
mod push;
mod relay;
mod threads;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

use devices::Device;

/// The relay (and the PWA, served from the same Worker) unless the user picks another.
pub const DEFAULT_RELAY_URL: &str = "https://winfer.danehub.com";

/// At most one push per pane this often.
const PUSH_THROTTLE: Duration = Duration::from_secs(5);
/// With winfer focused, the user counts as away (and gets pushes) after this long without input.
const AWAY_AFTER: Duration = Duration::from_secs(60 * 60);

/// Pushes go to the phone only when the user isn't at the desktop: no winfer window has focus,
/// or one does but nothing was typed or clicked in it for `AWAY_AFTER`.
fn should_push(focused: bool, since_input: Duration) -> bool {
    !focused || since_input >= AWAY_AFTER
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default)]
    relay_url: Option<String>,
    /// Whether the keychain has devices; skips reading it (and its prompt) when not.
    #[serde(default)]
    has_devices: bool,
}

struct Mobile {
    app: AppHandle,
    config_path: PathBuf,
    config: Mutex<Config>,
    devices: Mutex<Vec<Device>>,
    conns: Mutex<HashMap<String, relay::Conn>>,
    vapid: OnceLock<push::Vapid>,
    last_push: Mutex<HashMap<String, Instant>>,
    /// The last key, click or scroll in a winfer window (reported by the webview).
    last_input: Mutex<Instant>,
    /// Serializes keychain writes, which happen off the async threads.
    saving: Mutex<()>,
}

static MOBILE: OnceLock<Mobile> = OnceLock::new();

fn mobile() -> Result<&'static Mobile, String> {
    MOBILE.get().ok_or_else(|| "mobile companion isn't ready".to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn desktop_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let run = |cmd: &str, args: &[&str]| {
            std::process::Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
        };
        #[cfg(target_os = "macos")]
        let name = run("scutil", &["--get", "ComputerName"]).or_else(|| run("hostname", &["-s"]));
        #[cfg(not(target_os = "macos"))]
        let name = std::env::var("COMPUTERNAME").ok().or_else(|| run("hostname", &[]));
        name.unwrap_or_else(|| "winfer".into())
    })
}

impl Mobile {
    fn relay_url(&self) -> String {
        self.config.lock().unwrap().relay_url.clone().unwrap_or_else(|| DEFAULT_RELAY_URL.into())
    }

    fn vapid(&self) -> &push::Vapid {
        self.vapid.get_or_init(devices::vapid)
    }

    fn changed(&self) {
        let _ = self.app.emit("mobile://changed", ());
    }

    /// Writes the devices and config, off the calling thread (the keychain may prompt).
    fn persist(&'static self) {
        std::thread::spawn(move || {
            let _guard = self.saving.lock().unwrap();
            let devices = self.devices.lock().unwrap().clone();
            if let Err(err) = devices::save(&devices) {
                eprintln!("winfer: couldn't save paired phones: {err}");
            }
            let json = {
                let mut config = self.config.lock().unwrap();
                config.has_devices = !devices.is_empty();
                serde_json::to_string_pretty(&*config).unwrap_or_default()
            };
            let _ = std::fs::write(&self.config_path, json);
        });
    }

    fn connect(&'static self, device: &Device) {
        let Some(key) = crypto::unb64(&device.key) else { return };
        let id = device.device_id.clone();
        let conn = relay::spawn(
            self.relay_url(),
            device.room_id.clone(),
            Arc::new(crypto::Keys::derive(&key)),
            move |msg| self.handle(&id, msg),
            move || self.changed(),
        );
        self.conns.lock().unwrap().insert(device.device_id.clone(), conn);
    }

    fn send_to(&self, device: &str, msg: &Value) {
        if let Some(conn) = self.conns.lock().unwrap().get(device) {
            conn.send(msg);
        }
    }

    /// A message from a phone.
    fn handle(&'static self, device_id: &str, msg: Value) {
        let kind = msg["t"].as_str().unwrap_or_default().to_string();
        let id = msg["id"].clone();
        let now = now_ms();
        let reply = {
            let mut devices = self.devices.lock().unwrap();
            let Some(device) = devices.iter_mut().find(|d| d.device_id == device_id) else { return };
            if kind == "hello" {
                if !device.paired {
                    if device.expired(now) {
                        return;
                    }
                    device.paired = true;
                    if let Some(ua) = msg["ua"].as_str() {
                        device.name = devices::name_from_ua(ua);
                    }
                }
                device.last_seen = Some(now);
                None
            } else if !device.paired {
                return;
            } else {
                device.last_seen = Some(now);
                match kind.as_str() {
                    "push.subscribe" if msg["subscription"]["endpoint"].is_string() => {
                        device.push_subscription = Some(msg["subscription"].clone());
                        Some(json!({"t": "ack", "id": id, "ok": true}))
                    }
                    "push.subscribe" => Some(json!({"t": "ack", "id": id, "ok": false, "error": "no subscription"})),
                    "push.unsubscribe" => {
                        device.push_subscription = None;
                        Some(json!({"t": "ack", "id": id, "ok": true}))
                    }
                    _ => {
                        let _ = self.app.emit("mobile://request", json!({"device": device_id, "msg": msg}));
                        return;
                    }
                }
            }
        };
        self.persist();
        self.changed();
        match reply {
            Some(reply) => self.send_to(device_id, &reply),
            None => {
                let welcome = json!({
                    "t": "welcome",
                    "id": id,
                    "desktop": desktop_name(),
                    "version": env!("CARGO_PKG_VERSION"),
                    "vapidPublicKey": crypto::b64(&self.vapid().public),
                });
                self.send_to(device_id, &welcome);
                // The webview answers with the panes.
                let _ = self.app.emit("mobile://request", json!({"device": device_id, "msg": {"t": "panes.get"}}));
            }
        }
    }

    /// Pushes a status change to the phones that asked for notifications.
    fn maybe_push(&'static self, pane: &Value) {
        let status = pane["status"].as_str().unwrap_or_default();
        if !matches!(status, "done" | "needs-input" | "error") {
            return;
        }
        let focused = self.app.webview_windows().values().any(|w| w.is_focused().unwrap_or(false));
        if !should_push(focused, self.last_input.lock().unwrap().elapsed()) {
            return;
        }
        let pane_id = pane["id"].as_str().unwrap_or_default().to_string();
        {
            let mut last = self.last_push.lock().unwrap();
            if last.get(&pane_id).is_some_and(|t| t.elapsed() < PUSH_THROTTLE) {
                return;
            }
            last.insert(pane_id.clone(), Instant::now());
        }
        let excerpt = |v: &Value| v.as_str().map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty());
        let body = match status {
            "needs-input" => excerpt(&pane["request"]["text"]).or_else(|| excerpt(&pane["message"])).unwrap_or_else(|| "Needs your input".into()),
            "done" => excerpt(&pane["reply"]).or_else(|| excerpt(&pane["message"])).unwrap_or_else(|| "Finished".into()),
            _ => excerpt(&pane["message"]).unwrap_or_else(|| "Stopped with an error".into()),
        };
        let body: String = if body.chars().count() > 240 { format!("{}…", body.chars().take(239).collect::<String>()) } else { body };
        let verb = match status {
            "needs-input" => "needs input",
            "done" => "finished",
            _ => "error",
        };
        let title = format!("{} · {verb}", pane["title"].as_str().unwrap_or("Agent"));
        let payload = json!({"title": title, "body": body, "paneId": pane_id, "status": status});
        let targets: Vec<(String, Value)> = self
            .devices
            .lock()
            .unwrap()
            .iter()
            .filter(|d| d.paired)
            .filter_map(|d| Some((d.device_id.clone(), d.push_subscription.clone()?)))
            .collect();
        if targets.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            let mut gone = Vec::new();
            for (device, subscription) in targets {
                match push::send(self.vapid(), &subscription, &payload) {
                    push::Outcome::Sent => {}
                    push::Outcome::Gone => gone.push(device),
                    push::Outcome::Failed(err) => eprintln!("winfer: push to a phone failed: {err}"),
                }
            }
            if !gone.is_empty() {
                for d in self.devices.lock().unwrap().iter_mut().filter(|d| gone.contains(&d.device_id)) {
                    d.push_subscription = None;
                }
                self.persist();
                self.changed();
            }
        });
    }
}

/// Reads the pairings and connects to the relay; called once at startup.
pub fn start(app: AppHandle, data_dir: &std::path::Path) {
    let config_path = data_dir.join("mobile.json");
    let config: Config = std::fs::read_to_string(&config_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let has_devices = config.has_devices;
    let mobile = MOBILE.get_or_init(|| Mobile {
        app,
        config_path,
        config: Mutex::new(config),
        devices: Mutex::default(),
        conns: Mutex::default(),
        vapid: OnceLock::new(),
        last_push: Mutex::default(),
        last_input: Mutex::new(Instant::now()),
        saving: Mutex::new(()),
    });
    if !has_devices {
        return;
    }
    std::thread::spawn(move || {
        let now = now_ms();
        let all = devices::load();
        let kept: Vec<Device> = all.iter().filter(|d| !d.expired(now)).cloned().collect();
        let dropped = kept.len() != all.len();
        for device in &kept {
            mobile.connect(device);
        }
        *mobile.devices.lock().unwrap() = kept;
        if dropped {
            mobile.persist();
        }
        mobile.changed();
    });
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    device_id: String,
    name: String,
    created_at: u64,
    last_seen: Option<u64>,
    paired: bool,
    /// Until when the pairing link works, while unused.
    expires_at: Option<u64>,
    push: bool,
    #[serde(flatten)]
    conn: relay::ConnState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    relay_url: String,
    default_relay_url: &'static str,
    devices: Vec<DeviceView>,
}

fn status(m: &Mobile) -> Status {
    let now = now_ms();
    // Lock order: devices, then conns.
    let devices = m.devices.lock().unwrap();
    let conns = m.conns.lock().unwrap();
    let devices = devices
        .iter()
        .filter(|d| !d.expired(now))
        .map(|d| DeviceView {
            device_id: d.device_id.clone(),
            name: d.name.clone(),
            created_at: d.created_at,
            last_seen: d.last_seen,
            paired: d.paired,
            expires_at: (!d.paired).then_some(d.created_at + devices::PAIR_TTL_MS),
            push: d.push_subscription.is_some(),
            conn: conns.get(&d.device_id).map(|c| c.state.lock().unwrap().clone()).unwrap_or_default(),
        })
        .collect();
    Status { relay_url: m.relay_url(), default_relay_url: DEFAULT_RELAY_URL, devices }
}

#[tauri::command]
pub fn mobile_status() -> Result<Status, String> {
    Ok(status(mobile()?))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    device_id: String,
    link: String,
    /// The link as a QR code (SVG markup).
    qr_svg: String,
    expires_at: u64,
}

/// The pair link and QR code: `<relay>/#p=<base64url(JSON)>` (protocol.md "Pairing").
fn pair_link(base: &str, room: &str, key: &str, device: &str, name: &str) -> String {
    let payload = json!({"v": 1, "r": room, "k": key, "d": device, "n": name}).to_string();
    format!("{}/#p={}", base.trim().trim_end_matches('/'), crypto::b64(payload.as_bytes()))
}

/// Makes a pairing for a new phone, replacing any unused one.
#[tauri::command]
pub fn mobile_pair_start() -> Result<Pairing, String> {
    let m = mobile()?;
    let device = Device {
        device_id: crypto::b64(&crypto::random::<8>()),
        room_id: crypto::b64(&crypto::random::<16>()),
        key: crypto::b64(&crypto::random::<32>()),
        name: "New phone".into(),
        created_at: now_ms(),
        last_seen: None,
        paired: false,
        push_subscription: None,
    };
    let link = pair_link(&m.relay_url(), &device.room_id, &device.key, &device.device_id, desktop_name());
    let qr = qrcode::QrCode::new(link.as_bytes()).map_err(|e| e.to_string())?;
    let qr_svg = qr.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).quiet_zone(true).build();
    {
        let mut devices = m.devices.lock().unwrap();
        let mut conns = m.conns.lock().unwrap();
        devices.retain(|d| {
            if !d.paired {
                conns.remove(&d.device_id);
            }
            d.paired
        });
        devices.push(device.clone());
    }
    m.connect(&device);
    m.persist();
    m.changed();
    Ok(Pairing { device_id: device.device_id.clone(), link, qr_svg, expires_at: device.created_at + devices::PAIR_TTL_MS })
}

/// Unpairs a phone: forgets its key and wipes its room on the relay.
#[tauri::command]
pub fn mobile_revoke(device_id: String) -> Result<(), String> {
    let m = mobile()?;
    let removed = {
        let mut devices = m.devices.lock().unwrap();
        let removed = devices.iter().position(|d| d.device_id == device_id).map(|i| devices.remove(i));
        m.conns.lock().unwrap().remove(&device_id);
        removed
    };
    let Some(device) = removed else { return Ok(()) };
    m.persist();
    m.changed();
    let base = m.relay_url();
    std::thread::spawn(move || {
        let Some(key) = crypto::unb64(&device.key) else { return };
        let auth = crypto::Keys::derive(&key).auth_token;
        let url = format!("{}/r/{}?auth={auth}", base.trim_end_matches('/'), device.room_id);
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).http_status_as_error(false).build().into();
        let _ = agent.delete(&url).call();
    });
    Ok(())
}

/// Points winfer at another relay (empty: the default) and reconnects every phone.
#[tauri::command]
pub fn mobile_set_relay_url(url: String) -> Result<(), String> {
    let m = mobile()?;
    let url = url.trim().trim_end_matches('/').to_string();
    if !url.is_empty() && !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("the relay URL starts with https://".into());
    }
    m.config.lock().unwrap().relay_url = (!url.is_empty() && url != DEFAULT_RELAY_URL).then_some(url);
    let devices = m.devices.lock().unwrap().clone();
    m.conns.lock().unwrap().clear();
    for device in &devices {
        m.connect(device);
    }
    m.persist();
    m.changed();
    Ok(())
}

/// Sends a message to one phone, or to every paired phone when `device` is null. `status`
/// messages also become push notifications.
/// The user typed, clicked or scrolled in a winfer window (throttled by the webview).
#[tauri::command]
pub fn mobile_activity() {
    if let Ok(m) = mobile() {
        *m.last_input.lock().unwrap() = Instant::now();
    }
}

#[tauri::command]
pub fn mobile_send(device: Option<String>, msg: Value) -> Result<(), String> {
    let m = mobile()?;
    match device {
        Some(device) => m.send_to(&device, &msg),
        None => {
            let paired: Vec<String> = m.devices.lock().unwrap().iter().filter(|d| d.paired).map(|d| d.device_id.clone()).collect();
            for device in paired {
                m.send_to(&device, &msg);
            }
            // Silent updates (a new title, a renamed workspace) aren't news.
            if msg["t"] == "status" && msg["silent"] != true {
                m.maybe_push(&msg["pane"]);
            }
        }
    }
    Ok(())
}

/// A pane's conversation as chat messages (see threads.rs); null when it can't be read.
#[tauri::command]
pub async fn mobile_thread(provider: String, source: String, limit: Option<usize>) -> Option<Vec<threads::Message>> {
    tauri::async_runtime::spawn_blocking(move || threads::read(&provider, &source, limit.unwrap_or(200)))
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushes_only_when_the_user_is_away_from_the_desktop() {
        let min = Duration::from_secs(60);
        assert!(should_push(false, Duration::ZERO), "another app is in front");
        assert!(!should_push(true, 5 * min), "winfer focused and in use");
        assert!(!should_push(true, 59 * min));
        assert!(should_push(true, 60 * min), "focused but untouched for an hour");
    }

    #[test]
    fn pair_links_carry_the_pairing_in_the_fragment() {
        let link = pair_link("https://relay.example/", "ROOM", "KEY", "DEV", "Mac");
        let (base, payload) = link.split_once("/#p=").unwrap();
        assert_eq!(base, "https://relay.example");
        let v: Value = serde_json::from_slice(&crypto::unb64(payload).unwrap()).unwrap();
        assert_eq!(v, json!({"v": 1, "r": "ROOM", "k": "KEY", "d": "DEV", "n": "Mac"}));
    }
}
