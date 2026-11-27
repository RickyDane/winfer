//! Videos: played by the viewer's webview (see `html`) in a small player page, with the file
//! served in byte ranges, which media elements need to start and to seek.

use super::extension;
use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use wry::http::Response;

/// The containers the system webviews play.
const VIDEO: [&str; 4] = ["mp4", "m4v", "mov", "webm"];

/// The most one range response holds: the player asks for the rest as it goes, so a long video
/// is never read whole.
pub(super) const CHUNK: u64 = 1024 * 1024;

/// The query that asks for a video's player page instead of its bytes.
pub(super) const PLAYER: &str = "player";

/// Stops whatever plays on the page: a hidden webview would go on sounding.
pub(super) const PAUSE_JS: &str = "document.querySelectorAll('video, audio').forEach(function (m) { m.pause(); });";

pub(super) fn is_video(path: &str) -> bool {
    VIDEO.contains(&extension(path).as_str())
}

/// The page that plays the video at `src` (a URL path). The `#t` makes it show the first frame
/// before playing, instead of black.
pub(super) fn player_page(src: &str) -> String {
    let src = src.replace('&', "&amp;").replace('"', "&quot;");
    format!(
        r#"<!doctype html>
<meta charset="utf-8">
<style>
html, body {{ margin: 0; height: 100%; overflow: hidden; background: #000; }}
video {{ width: 100%; height: 100%; object-fit: contain; outline: none; }}
p {{ display: none; height: 100%; margin: 0; padding: 0 24px; align-items: center; justify-content: center; text-align: center; color: #aaa; font: 12px system-ui, sans-serif; }}
</style>
<video src="{src}#t=0.001" controls playsinline preload="metadata"></video>
<p>This video can't be played here: its format or codec isn't supported.</p>
<script>
var video = document.querySelector('video');
video.addEventListener('error', function () {{
  video.style.display = 'none';
  document.querySelector('p').style.display = 'flex';
}});
</script>
"#
    )
}

/// The first and last byte a `Range` header asks of a `len` byte file, at most `CHUNK` of them;
/// `None` when it can't be satisfied. Of several ranges only the first is served.
pub(super) fn byte_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let (from, to) = header.trim().strip_prefix("bytes=")?.split(',').next()?.trim().split_once('-')?;
    let last = len.checked_sub(1)?;
    let (start, end) = if from.is_empty() {
        // `-500`: the last 500 bytes.
        (len.saturating_sub(to.parse::<u64>().ok().filter(|n| *n > 0)?), last)
    } else {
        (from.parse().ok()?, if to.is_empty() { last } else { to.parse::<u64>().ok()?.min(last) })
    };
    (start <= end).then(|| (start, end.min(start.saturating_add(CHUNK - 1))))
}

/// The part of `file` a `Range` header asks for.
pub(super) fn serve_range(file: &Path, mime: &str, header: &str) -> std::io::Result<Response<Cow<'static, [u8]>>> {
    let mut source = std::fs::File::open(file)?;
    let len = source.metadata()?.len();
    let response = Response::builder().header("Content-Type", mime).header("Cache-Control", "no-store").header("Accept-Ranges", "bytes");
    let Some((start, end)) = byte_range(header, len) else {
        return Ok(response.status(416).header("Content-Range", format!("bytes */{len}")).body(Cow::Owned(Vec::new())).unwrap());
    };
    source.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((end - start + 1) as usize);
    source.take(end - start + 1).read_to_end(&mut bytes)?;
    Ok(response
        .status(206)
        .header("Content-Range", format!("bytes {start}-{end}/{len}"))
        .header("Content-Length", bytes.len())
        .body(Cow::Owned(bytes))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::{CHUNK, byte_range, is_video, player_page, serve_range};

    #[test]
    fn recognizes_videos() {
        assert!(is_video("/a/demo.mp4"));
        assert!(is_video("/a/Screen Recording.MOV"));
        assert!(is_video("/a/clip.webm"));
        assert!(!is_video("/a/.mp4"));
        assert!(!is_video("/a/shot.png"));
    }

    #[test]
    fn reads_range_headers() {
        assert_eq!(byte_range("bytes=0-1", 10), Some((0, 1)));
        assert_eq!(byte_range("bytes=4-", 10), Some((4, 9)));
        assert_eq!(byte_range("bytes=4-99", 10), Some((4, 9)));
        assert_eq!(byte_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(byte_range("bytes=-30", 10), Some((0, 9)));
        assert_eq!(byte_range("bytes=1-2, 5-6", 10), Some((1, 2)));
        assert_eq!(byte_range("bytes=0-", 3 * CHUNK), Some((0, CHUNK - 1)));
        assert_eq!(byte_range("bytes=10-", 10), None);
        assert_eq!(byte_range("bytes=5-4", 10), None);
        assert_eq!(byte_range("bytes=-0", 10), None);
        assert_eq!(byte_range("bytes=0-", 0), None);
        assert_eq!(byte_range("chunks=0-1", 10), None);
    }

    #[test]
    fn serves_parts_of_a_file() {
        let file = std::env::temp_dir().join(format!("winfer-range-{}.mp4", std::process::id()));
        std::fs::write(&file, "0123456789").unwrap();
        let part = serve_range(&file, "video/mp4", "bytes=2-4").unwrap();
        assert_eq!(part.status(), 206);
        assert_eq!(part.headers()["Content-Range"], "bytes 2-4/10");
        assert_eq!(part.headers()["Content-Length"], "3");
        assert_eq!(part.body().as_ref(), b"234");
        let past = serve_range(&file, "video/mp4", "bytes=20-").unwrap();
        assert_eq!(past.status(), 416);
        assert_eq!(past.headers()["Content-Range"], "bytes */10");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn player_page_escapes_the_source() {
        assert!(player_page("/a/x&\"y.mp4").contains("src=\"/a/x&amp;&quot;y.mp4#t=0.001\""));
    }
}
