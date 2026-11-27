//! Panel frames for plugins that record a panel (the screen recorder). In the Tauri app the browser and
//! Android panels draw into canvases plugins can read; here they draw outside the plugin webview. So the
//! panels leave their newest picture here (cheap: the bytes they got or the image they show), and while a
//! plugin reads a panel (the host asks for it), each new picture goes to the host as an image file.

use gpui_kit::RenderImage;
use parking_lot::Mutex;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Browser,
    Android,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Browser, Source::Android];

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "browser" => Some(Self::Browser),
            "android" => Some(Self::Android),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Android => "android",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// An encoded picture: its bytes and MIME type.
pub struct Frame {
    pub bytes: Arc<Vec<u8>>,
    pub mime: &'static str,
}

#[derive(Clone)]
enum Picture {
    /// JPEG or PNG, as the panel got it.
    Encoded(Arc<Vec<u8>>),
    /// What the panel shows (BGRA).
    Image(Arc<RenderImage>),
}

#[derive(Default)]
struct Slot {
    picture: Option<Picture>,
    /// Counts pictures, so each goes out once.
    serial: u64,
    sent: u64,
}

static WANTED: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];
static SLOTS: Mutex<[Slot; 2]> = Mutex::new([Slot { picture: None, serial: 0, sent: 0 }, Slot { picture: None, serial: 0, sent: 0 }]);

fn keep(source: Source, picture: Picture) {
    let mut slots = SLOTS.lock();
    let slot = &mut slots[source.index()];
    slot.picture = Some(picture);
    slot.serial += 1;
}

/// The panel's newest picture as it arrived (JPEG or PNG).
pub fn offer_encoded(source: Source, bytes: &[u8]) {
    keep(source, Picture::Encoded(Arc::new(bytes.to_vec())));
}

/// The panel's newest decoded picture.
pub fn offer_image(source: Source, image: &Arc<RenderImage>) {
    keep(source, Picture::Image(image.clone()));
}

/// A plugin starts or stops reading `source`; when it starts, the current picture goes out first.
pub fn set_wanted(source: Source, on: bool) {
    WANTED[source.index()].store(on, Ordering::Relaxed);
    if on {
        SLOTS.lock()[source.index()].sent = 0;
    }
}

pub fn wanted(source: Source) -> bool {
    WANTED[source.index()].load(Ordering::Relaxed)
}

pub fn any_wanted() -> bool {
    Source::ALL.iter().any(|s| wanted(*s))
}

pub fn clear() {
    for source in Source::ALL {
        set_wanted(source, false);
    }
}

fn jpeg(image: &RenderImage) -> Option<Vec<u8>> {
    let size = image.size(0);
    let (width, height) = (u32::try_from(size.width.0).ok()?, u32::try_from(size.height.0).ok()?);
    let bgra = image.as_bytes(0)?;
    let mut rgb = Vec::with_capacity(bgra.len() / 4 * 3);
    for p in bgra.chunks_exact(4) {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    let rgb = image::RgbImage::from_raw(width, height, rgb)?;
    let mut out = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&rgb).ok()?;
    Some(out.into_inner())
}

/// The panel's newest picture for a phone, when it's newer than `after` (a serial this returned):
/// its serial, JPEG at most `max_width` wide, and the picture's own size. Encodes, so call it off
/// the main thread.
pub fn newest_jpeg(source: Source, after: u64, max_width: u32, quality: u8) -> Option<(u64, Vec<u8>, u32, u32)> {
    let (serial, picture) = {
        let slots = SLOTS.lock();
        let slot = &slots[source.index()];
        (slot.serial, slot.picture.clone()?)
    };
    if serial == after {
        return None;
    }
    let rgb = match picture {
        Picture::Encoded(bytes) => image::load_from_memory(&bytes).ok()?.into_rgb8(),
        Picture::Image(image) => {
            let size = image.size(0);
            let (width, height) = (u32::try_from(size.width.0).ok()?, u32::try_from(size.height.0).ok()?);
            let bgra = image.as_bytes(0)?;
            let mut rgb = Vec::with_capacity(bgra.len() / 4 * 3);
            for p in bgra.chunks_exact(4) {
                rgb.extend_from_slice(&[p[2], p[1], p[0]]);
            }
            image::RgbImage::from_raw(width, height, rgb)?
        }
    };
    let (width, height) = rgb.dimensions();
    let max_width = max_width.max(160);
    let rgb = if width > max_width {
        let h = ((height as f64) * (max_width as f64) / (width as f64)).round().max(1.0) as u32;
        image::imageops::resize(&rgb, max_width, h, image::imageops::FilterType::Triangle)
    } else {
        rgb
    };
    let mut out = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(20, 95)).encode_image(&rgb).ok()?;
    Some((serial, out.into_inner(), width, height))
}

/// The pictures of read panels that haven't gone out yet. Encodes decoded ones, so call it off the main thread.
pub fn take() -> Vec<(Source, Frame)> {
    let due: Vec<(Source, Picture)> = {
        let mut slots = SLOTS.lock();
        Source::ALL
            .iter()
            .filter(|s| wanted(**s))
            .filter_map(|s| {
                let slot = &mut slots[s.index()];
                if slot.sent == slot.serial {
                    return None;
                }
                slot.sent = slot.serial;
                slot.picture.clone().map(|p| (*s, p))
            })
            .collect()
    };
    due.into_iter()
        .filter_map(|(source, picture)| {
            let frame = match picture {
                Picture::Encoded(bytes) => Frame { mime: if bytes.starts_with(b"\x89PNG") { "image/png" } else { "image/jpeg" }, bytes },
                Picture::Image(image) => Frame { bytes: Arc::new(jpeg(&image)?), mime: "image/jpeg" },
            };
            Some((source, frame))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::smallvec;

    #[test]
    fn sends_each_picture_once_while_wanted() {
        clear();
        offer_encoded(Source::Browser, b"\xff\xd8one");
        assert!(take().is_empty(), "nobody reads the browser");

        // Starting to read sends the picture there already is (a still page sends no new ones).
        set_wanted(Source::Browser, true);
        let frames = take();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].1.mime, "image/jpeg");
        assert!(take().is_empty(), "sent once");
        offer_encoded(Source::Browser, b"\x89PNGtwo");
        assert_eq!(take()[0].1.mime, "image/png");

        let bgra = image::RgbaImage::from_raw(2, 2, vec![255; 16]).unwrap();
        offer_image(Source::Android, &Arc::new(RenderImage::new(smallvec![image::Frame::new(bgra)])));
        set_wanted(Source::Android, true);
        let frames = take();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].1.bytes.starts_with(b"\xff\xd8"));
        clear();
        assert!(!any_wanted());
    }
}
