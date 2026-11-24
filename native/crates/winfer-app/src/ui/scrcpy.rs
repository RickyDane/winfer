//! A client for the scrcpy server (github.com/Genymobile/scrcpy), used by the Android mirror.
//!
//! The server runs on the device through `app_process`, streams the screen as H.264 with one
//! packet per frame (so a frame is decoded the moment it arrives, instead of when the next one
//! starts) and takes touch, scroll, key and text events over a socket, so a drag plays live on
//! the device instead of as one `adb shell input` process per gesture. The protocol is internal to
//! scrcpy and changes between versions: this speaks VERSION only (its `doc/develop.md`,
//! `app/src/control_msg.c`).

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use tauri::Manager;
use winfer_backend::android;

const VERSION: &str = "5.0.1";
/// From the release's SHA256SUMS.txt.
const SHA256: &str = "764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536";
const DEVICE_PATH: &str = "/data/local/tmp/winfer-scrcpy-server.jar";
const CODEC_H264: u32 = 0x6832_3634;
/// The longest side of the video: sharp in the side panel, light to decode.
const MAX_SIDE: u32 = 1280;

const ACTION_DOWN: u8 = 0;
const ACTION_UP: u8 = 1;
const ACTION_MOVE: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Touch {
    Down,
    Move,
    Up,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The server jar, downloaded once into the app's data folder and checked against SHA256.
fn server_jar() -> Result<PathBuf, String> {
    server_jar_in(crate::backend::app().path().app_data_dir().map_err(|e| e.to_string())?.join("scrcpy"))
}

fn server_jar_in(dir: PathBuf) -> Result<PathBuf, String> {
    let path = dir.join(format!("scrcpy-server-v{VERSION}"));
    if std::fs::read(&path).is_ok_and(|b| sha256_hex(&b) == SHA256) {
        return Ok(path);
    }
    let url = format!("https://github.com/Genymobile/scrcpy/releases/download/v{VERSION}/scrcpy-server-v{VERSION}");
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build().into();
    let bytes = agent
        .get(&url)
        .header("User-Agent", "winfer")
        .call()
        .and_then(|mut res| res.body_mut().read_to_vec())
        .map_err(|e| format!("couldn't download the scrcpy server: {e}"))?;
    if sha256_hex(&bytes) != SHA256 {
        return Err("the downloaded scrcpy server doesn't match its checksum".into());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let part = path.with_extension("part");
    std::fs::write(&part, &bytes).and_then(|_| std::fs::rename(&part, &path)).map_err(|e| e.to_string())?;
    Ok(path)
}

fn adb_run(serial: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = android::adb(Some(serial))?;
    cmd.args(args);
    android::run(cmd)
}

/// Waits for the server to connect to `listener`; fails early if it exits.
fn accept(listener: &TcpListener, server: &mut Child, log: &Mutex<String>, deadline: Instant) -> Result<TcpStream, String> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Accepted sockets inherit the listener's non-blocking mode on macOS.
                stream.set_nonblocking(false).map_err(|e| e.to_string())?;
                stream.set_nodelay(true).ok();
                return Ok(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        if let Ok(Some(_)) = server.try_wait() {
            std::thread::sleep(Duration::from_millis(100)); // let the log reader catch the last lines
            let log = log.lock().trim().to_string();
            return Err(if log.is_empty() { "the scrcpy server exited".into() } else { log });
        }
        if Instant::now() > deadline {
            return Err("the scrcpy server didn't connect".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn read_u32(r: &mut impl Read) -> std::io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

/// What a 12-byte packet header announces.
#[derive(Debug, PartialEq)]
enum Header {
    /// A new capture session (first one, or after a rotation): the video's size.
    Session(u32, u32),
    Media { config: bool, size: usize },
}

fn parse_header(h: &[u8; 12]) -> Header {
    let word = |i: usize| u32::from_be_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
    if h[0] & 0x80 != 0 {
        Header::Session(word(4), word(8))
    } else {
        Header::Media { config: h[0] & 0x40 != 0, size: word(8) as usize }
    }
}

fn position(buf: &mut Vec<u8>, (x, y): (i32, i32), (w, h): (u32, u32)) {
    buf.extend(x.to_be_bytes());
    buf.extend(y.to_be_bytes());
    buf.extend((w as u16).to_be_bytes());
    buf.extend((h as u16).to_be_bytes());
}

/// A finger touch (pointer id "generic finger", so drags scroll like a real touch).
fn touch_msg(touch: Touch, at: (i32, i32), size: (u32, u32)) -> Vec<u8> {
    let action = match touch {
        Touch::Down => ACTION_DOWN,
        Touch::Move => ACTION_MOVE,
        Touch::Up => ACTION_UP,
    };
    let mut buf = vec![2, action];
    buf.extend((-2i64 as u64).to_be_bytes());
    position(&mut buf, at, size);
    buf.extend(if touch == Touch::Up { 0u16 } else { 0xffff }.to_be_bytes());
    buf.extend(0u32.to_be_bytes()); // action button
    buf.extend(0u32.to_be_bytes()); // buttons
    buf
}

/// Scroll amounts in [-16, 16], sent as 16-bit fixed point of amount / 16.
fn scroll_fixed(v: f32) -> i16 {
    ((v / 16.0).clamp(-1.0, 1.0) * 32768.0).clamp(-32768.0, 32767.0) as i16
}

fn scroll_msg(at: (i32, i32), size: (u32, u32), h: f32, v: f32) -> Vec<u8> {
    let mut buf = vec![3];
    position(&mut buf, at, size);
    buf.extend(scroll_fixed(h).to_be_bytes());
    buf.extend(scroll_fixed(v).to_be_bytes());
    buf.extend(0u32.to_be_bytes());
    buf
}

fn key_msg(down: bool, code: u32) -> Vec<u8> {
    let mut buf = vec![0, if down { ACTION_DOWN } else { ACTION_UP }];
    buf.extend(code.to_be_bytes());
    buf.extend(0u32.to_be_bytes()); // repeat
    buf.extend(0u32.to_be_bytes()); // meta state
    buf
}

/// At most 300 bytes per message, cut at a character boundary.
fn text_msgs(text: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(300);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (chunk, tail) = rest.split_at(end);
        let mut buf = vec![1];
        buf.extend((chunk.len() as u32).to_be_bytes());
        buf.extend(chunk.as_bytes());
        out.push(buf);
        rest = tail;
    }
    out
}

/// A running mirror; stops the server when dropped.
pub struct Session {
    control: Mutex<TcpStream>,
    video: TcpStream,
    /// The current video size; touches are given in it.
    size: Arc<Mutex<(u32, u32)>>,
    server: Mutex<Option<Child>>,
}

impl Session {
    /// Starts the server on `serial` and sends each frame (complete Annex B access units, the
    /// parameter sets in front of keyframes) to `frames` until the session is dropped. Blocking.
    pub fn start(serial: &str, frames: Sender<Vec<u8>>) -> Result<Session, String> {
        Self::start_with(server_jar()?, serial, frames)
    }

    fn start_with(jar: PathBuf, serial: &str, frames: Sender<Vec<u8>>) -> Result<Session, String> {
        adb_run(serial, &["push", &jar.to_string_lossy(), DEVICE_PATH])?;
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let scid = rand::random::<u32>() & 0x7fff_ffff;
        let socket = format!("localabstract:scrcpy_{scid:08x}");
        adb_run(serial, &["reverse", &socket, &format!("tcp:{port}")])?;
        let started = Self::connect(serial, scid, &listener, frames);
        let _ = adb_run(serial, &["reverse", "--remove", &socket]);
        started
    }

    fn connect(serial: &str, scid: u32, listener: &TcpListener, frames: Sender<Vec<u8>>) -> Result<Session, String> {
        let mut cmd = android::adb(Some(serial))?;
        cmd.args(["shell", &format!("CLASSPATH={DEVICE_PATH}"), "app_process", "/", "com.genymobile.scrcpy.Server", VERSION])
            .arg(format!("scid={scid:08x}"))
            .args(["log_level=warn", "audio=false", "max_fps=60", "video_bit_rate=8000000", "clipboard_autosync=false", "send_device_meta=false"])
            .arg(format!("max_size={MAX_SIDE}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut server = cmd.spawn().map_err(|e| format!("couldn't run adb: {e}"))?;
        // The server's last words, for the error when it fails to start.
        let log = Arc::new(Mutex::new(String::new()));
        for out in [server.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>), server.stderr.take().map(|o| Box::new(o) as Box<dyn Read + Send>)].into_iter().flatten() {
            let log = log.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let mut log = log.lock();
                    if log.len() < 2000 {
                        log.push_str(&line);
                        log.push('\n');
                    }
                }
            });
        }
        let stop = |mut server: Child, e: String| {
            let _ = server.kill();
            let _ = server.wait();
            Err(e)
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        // Sockets open in this order: video, then control (audio is off).
        let mut video = match accept(listener, &mut server, &log, deadline) {
            Ok(s) => s,
            Err(e) => return stop(server, e),
        };
        let control = match accept(listener, &mut server, &log, deadline) {
            Ok(s) => s,
            Err(e) => return stop(server, e),
        };
        video.set_read_timeout(Some(Duration::from_secs(10))).ok();
        let size = (|| -> Result<(u32, u32), String> {
            let codec = read_u32(&mut video).map_err(|e| e.to_string())?;
            if codec != CODEC_H264 {
                return Err(format!("unexpected video codec {codec:#x}"));
            }
            let mut h = [0; 12];
            video.read_exact(&mut h).map_err(|e| e.to_string())?;
            match parse_header(&h) {
                Header::Session(w, h) => Ok((w, h)),
                other => Err(format!("unexpected first packet {other:?}")),
            }
        })();
        let size = match size {
            Ok(s) => Arc::new(Mutex::new(s)),
            Err(e) => return stop(server, e),
        };
        video.set_read_timeout(None).ok();

        let reader = video.try_clone().map_err(|e| e.to_string())?;
        let shared = size.clone();
        std::thread::spawn(move || video_loop(reader, shared, frames));
        // Device messages (clipboard, acks) aren't used; read them so the server never blocks.
        if let Ok(mut device) = control.try_clone() {
            std::thread::spawn(move || {
                let mut sink = [0; 4096];
                while matches!(device.read(&mut sink), Ok(n) if n > 0) {}
            });
        }
        Ok(Session { control: Mutex::new(control), video, size, server: Mutex::new(Some(server)) })
    }

    pub fn size(&self) -> (u32, u32) {
        *self.size.lock()
    }

    fn send(&self, msg: &[u8]) {
        let _ = self.control.lock().write_all(msg);
    }

    fn point(&self, (x, y): (f32, f32)) -> ((i32, i32), (u32, u32)) {
        let size = self.size();
        (((x.clamp(0.0, 1.0) * size.0 as f32).round() as i32, (y.clamp(0.0, 1.0) * size.1 as f32).round() as i32), size)
    }

    /// A touch at a point given as a fraction of the screen (0..1 both ways).
    pub fn touch(&self, touch: Touch, at: (f32, f32)) {
        let (at, size) = self.point(at);
        self.send(&touch_msg(touch, at, size));
    }

    /// Scroll by `h`/`v` wheel notches (positive: right / up).
    pub fn scroll(&self, at: (f32, f32), h: f32, v: f32) {
        let (at, size) = self.point(at);
        self.send(&scroll_msg(at, size, h, v));
    }

    pub fn key(&self, code: u32) {
        self.send(&[key_msg(true, code), key_msg(false, code)].concat());
    }

    pub fn text(&self, text: &str) {
        for msg in text_msgs(text) {
            self.send(&msg);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Closing the sockets ends the server; the process is reaped off the UI thread.
        let _ = self.video.shutdown(Shutdown::Both);
        let _ = self.control.lock().shutdown(Shutdown::Both);
        if let Some(mut server) = self.server.lock().take() {
            std::thread::spawn(move || {
                let _ = server.kill();
                let _ = server.wait();
            });
        }
    }
}

/// Reads packets until the socket closes; the config packet (SPS/PPS) goes in front of the next frame.
fn video_loop(mut video: TcpStream, size: Arc<Mutex<(u32, u32)>>, frames: Sender<Vec<u8>>) {
    let mut config: Vec<u8> = Vec::new();
    let mut h = [0; 12];
    while video.read_exact(&mut h).is_ok() {
        match parse_header(&h) {
            Header::Session(w, hh) => *size.lock() = (w, hh),
            Header::Media { config: is_config, size: len } => {
                let mut packet = std::mem::take(&mut config);
                let at = packet.len();
                packet.resize(at + len, 0);
                if video.read_exact(&mut packet[at..]).is_err() {
                    return;
                }
                if is_config {
                    config = packet;
                } else if frames.send(packet).is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_and_media_headers() {
        assert_eq!(parse_header(&[0x80, 0, 0, 0, 0, 0, 2, 0xd0, 0, 0, 5, 0]), Header::Session(720, 1280));
        assert_eq!(parse_header(&[0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 30]), Header::Media { config: true, size: 30 });
        assert_eq!(parse_header(&[0x20, 0, 0, 0, 0, 0, 0, 9, 0, 1, 0, 0]), Header::Media { config: false, size: 65536 });
    }

    #[test]
    fn serializes_like_scrcpy() {
        // app/tests/test_control_msg_serialize.c, test_serialize_inject_touch_event (finger, pressure 1).
        let msg = touch_msg(Touch::Down, (100, 200), (1080, 1920));
        assert_eq!(msg.len(), 32);
        assert_eq!(&msg[..10], &[2, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe]);
        assert_eq!(&msg[10..22], &[0, 0, 0, 100, 0, 0, 0, 200, 0x04, 0x38, 0x07, 0x80]);
        assert_eq!(&msg[22..24], &[0xff, 0xff]);
        assert_eq!(touch_msg(Touch::Up, (0, 0), (1, 1))[22..24], [0, 0]);
        assert_eq!(key_msg(true, 66), vec![0, 0, 0, 0, 0, 66, 0, 0, 0, 0, 0, 0, 0, 0]);
        let scroll = scroll_msg((260, 1026), (1080, 1920), 16.0, -16.0);
        assert_eq!(scroll.len(), 21);
        assert_eq!(&scroll[13..17], &[0x7f, 0xff, 0x80, 0x00]);
        assert_eq!(text_msgs("hello"), vec![vec![1, 0, 0, 0, 5, b'h', b'e', b'l', b'l', b'o']]);
        let long = "é".repeat(200);
        assert!(text_msgs(&long).iter().all(|m| m.len() <= 305 && std::str::from_utf8(&m[5..]).is_ok()));
    }

    /// Against the first adb device: `cargo test -p winfer-app --bin winfer-native live_session -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_session() {
        let jar = server_jar_in(std::env::temp_dir().join("winfer-scrcpy-test")).unwrap();
        let out = String::from_utf8(adb_run_any(&["devices"])).unwrap();
        let serial = out.lines().skip(1).find_map(|l| l.strip_suffix("\tdevice")).expect("no device").to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        let t = Instant::now();
        let session = Session::start_with(jar, &serial, tx).unwrap();
        println!("started in {:?}, size {:?}", t.elapsed(), session.size());
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        println!("first frame after {:?}: {} bytes, starts {:02x?}", t.elapsed(), first.len(), &first[..8]);
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let (mut dec, mut conv, mut frames) = (Duration::ZERO, Duration::ZERO, 0u32);
        let mut take = |f: &[u8]| {
            let t = Instant::now();
            let Ok(Some(yuv)) = decoder.decode(f) else { return false };
            dec += t.elapsed();
            let t = Instant::now();
            let (w, h) = openh264::formats::YUVSource::dimensions(&yuv);
            let mut rgba = vec![0; w * h * 4];
            yuv.write_rgba8(&mut rgba);
            conv += t.elapsed();
            frames += 1;
            true
        };
        assert!(take(&first), "first frame didn't decode");
        // Input-to-frame latency, then the frames of the drag it starts.
        let mut lat = Vec::new();
        for i in 0..8 {
            while let Ok(f) = rx.recv_timeout(Duration::from_millis(500)) {
                take(&f);
            }
            let t = Instant::now();
            session.touch(Touch::Down, (0.5, 0.75));
            session.touch(Touch::Move, (0.5, if i % 2 == 0 { 0.35 } else { 0.85 }));
            let f = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            lat.push(t.elapsed().as_millis());
            take(&f);
            session.touch(Touch::Up, (0.5, 0.35));
        }
        let t = Instant::now();
        let mut n = 0;
        while let Ok(f) = rx.recv_timeout(Duration::from_millis(500)) {
            take(&f);
            n += 1;
        }
        println!("input->frame ms {lat:?}; {n} frames in the last {:?}", t.elapsed());
        println!("per frame: decode {:?}, rgba {:?} ({frames} frames)", dec / frames.max(1), conv / frames.max(1));
        assert!(frames > 8);
    }

    fn adb_run_any(args: &[&str]) -> Vec<u8> {
        let mut cmd = android::adb(None).unwrap();
        cmd.args(args);
        android::run(cmd).unwrap()
    }
}
