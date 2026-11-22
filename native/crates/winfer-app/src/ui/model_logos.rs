//! Logos of the companies behind models (Qwen, Gemma, Llama, Mistral…), for lists of local models.
//! Fetched once from @lobehub/icons (the source of winfer's agent logos too) and kept on disk.

use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The icon set's version, so the URLs never change under us.
const ICONS: &str = "https://unpkg.com/@lobehub/icons-static-svg@1.105.0/icons";

/// Words a model id starts a part with ("qwen3", "gemma", …) and the maker's icon.
const MAKERS: &[(&[&str], &str)] = &[
    (&["qwen", "qwq"], "qwen"),
    (&["gemma"], "gemma"),
    (&["gemini"], "google"),
    (&["llama", "meta"], "meta"),
    (&["mistral", "devstral", "codestral", "magistral", "ministral", "mixtral", "pixtral"], "mistral"),
    (&["deepseek"], "deepseek"),
    (&["phi"], "microsoft"),
    (&["gpt"], "openai"),
    (&["glm", "chatglm"], "zhipu"),
    (&["kimi"], "kimi"),
    (&["minimax"], "minimax"),
    (&["granite"], "ibm"),
    (&["nemotron", "nvidia"], "nvidia"),
    (&["olmo", "tulu"], "ai2"),
    (&["hermes"], "nousresearch"),
    (&["command", "aya"], "cohere"),
    (&["falcon"], "tii"),
    (&["internlm"], "internlm"),
    (&["baichuan"], "baichuan"),
    (&["smollm"], "huggingface"),
    (&["exaone"], "lg"),
    (&["seed"], "bytedance"),
    (&["hunyuan"], "hunyuan"),
    (&["lfm"], "liquid"),
];

/// Parts that must match whole, not as a start ("yi" would match "yield").
const EXACT: &[(&str, &str)] = &[("yi", "yi"), ("step", "stepfun")];

/// The maker's icon name for a model id like `local/Qwen3.6-35B-A3B`, if it's a known family.
pub fn maker_of(model: &str) -> Option<&'static str> {
    let lower = model.to_lowercase();
    let parts: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|p| !p.is_empty() && *p != "local").collect();
    for part in &parts {
        if let Some((_, slug)) = MAKERS.iter().find(|(words, _)| words.iter().any(|w| part.starts_with(w))) {
            return Some(slug);
        }
        if let Some((_, slug)) = EXACT.iter().find(|(w, _)| part == w) {
            return Some(slug);
        }
    }
    None
}

/// A logo's SVG and whether it's the one-color version (drawn in the text color).
#[derive(Clone)]
struct Svg {
    text: String,
    mono: bool,
}

/// What's fetched and drawn so far.
#[derive(Default)]
pub struct ModelLogos {
    svgs: HashMap<&'static str, Option<Svg>>,
    loading: HashSet<&'static str>,
    images: HashMap<(&'static str, u32), Arc<Image>>,
}

impl ModelLogos {
    /// The logo, drawn with `color` if it's one-color. None while it loads or when there's none;
    /// `true` in the second place means it should be fetched now.
    pub fn get(&mut self, slug: &'static str, color: Hsla) -> (Option<Arc<Image>>, bool) {
        match self.svgs.get(slug) {
            Some(Some(svg)) => {
                let rgba = Rgba::from(color);
                let key = (slug, rgba_u32(rgba));
                let svg = svg.clone();
                let image = self.images.entry(key).or_insert_with(|| Arc::new(Image::from_bytes(ImageFormat::Svg, drawable(&svg, rgba).into_bytes())));
                (Some(image.clone()), false)
            }
            Some(None) => (None, false),
            None => (None, self.loading.insert(slug)),
        }
    }

    pub fn loaded(&mut self, slug: &'static str, svg: Option<(String, bool)>) {
        self.loading.remove(slug);
        self.svgs.insert(slug, svg.map(|(text, mono)| Svg { text, mono }));
    }
}

fn rgba_u32(c: Rgba) -> u32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (b(c.r) << 24) | (b(c.g) << 16) | (b(c.b) << 8) | b(c.a)
}

/// The SVG sized in pixels (it says 1em), cropped to its drawing and, if one-color, filled with `color`.
fn drawable(svg: &Svg, color: Rgba) -> String {
    let text = svg.text.replace("width=\"1em\"", "width=\"24\"").replace("height=\"1em\"", "height=\"24\"");
    if text.contains("<title>Gemma</title>") {
        return GEMMA_SMALL.to_string();
    }
    let text = cropped(&text).unwrap_or(text);
    if svg.mono {
        let hex = format!("#{:08x}", rgba_u32(color));
        text.replace("currentColor", &hex[..7])
    } else {
        text
    }
}

/// Gemma's star in its ring, without the hairline construction grid around them, which at list
/// size covers the star (same gradient as the fetched logo).
const GEMMA_SMALL: &str = r##"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><defs><linearGradient id="g" x1="20%" x2="80%" y1="80%" y2="20%"><stop offset="0%" stop-color="#446EFF"/><stop offset="37%" stop-color="#2E96FF"/><stop offset="83%" stop-color="#B1C5FF"/></linearGradient></defs><circle cx="12" cy="12" r="11.1" fill="none" stroke="url(#g)" stroke-width="1.3" opacity="0.5"/><path fill="url(#g)" d="M12 2.6C12.7 7.6 16.4 11.3 21.4 12C16.4 12.7 12.7 16.4 12 21.4C11.3 16.4 7.6 12.7 2.6 12C7.6 11.3 11.3 7.6 12 2.6Z"/></svg>"##;

fn with_view_box(text: &str, view_box: &str) -> Option<String> {
    let start = text.find("viewBox=\"")? + "viewBox=\"".len();
    let end = start + text[start..].find('"')?;
    Some(format!("{}{view_box}{}", &text[..start], &text[end..]))
}

/// The SVG with its viewBox fitted to what it draws (a square around it), so logos drawn small
/// inside their 24×24 box (Gemma's) fill the slot like the others.
fn cropped(text: &str) -> Option<String> {
    let tree = resvg::usvg::Tree::from_str(text, &resvg::usvg::Options::default()).ok()?;
    let b = tree.root().abs_stroke_bounding_box();
    let side = b.width().max(b.height());
    if side < 1.0 || side >= 23.0 {
        return None;
    }
    let (cx, cy) = (b.x() + b.width() / 2.0, b.y() + b.height() / 2.0);
    let side = side + 0.5;
    with_view_box(text, &format!("{} {} {side} {side}", cx - side / 2.0, cy - side / 2.0))
}

/// The maker's logo from the disk cache, else from the icon set (the color version if there is
/// one). Blocking: call off the UI thread. Err when the network failed (try again later).
pub fn fetch(slug: &str) -> Result<Option<(String, bool)>, String> {
    let dir = crate::backend::app().path().app_data_dir().map_err(|e| e.to_string())?.join("model-logos");
    for (name, mono) in [(format!("{slug}-color.svg"), false), (format!("{slug}.svg"), true)] {
        if let Ok(text) = std::fs::read_to_string(dir.join(&name)) {
            return Ok(Some((text, mono)));
        }
    }
    if dir.join(format!("{slug}.none")).exists() {
        return Ok(None);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(10))).build().into();
    let _ = std::fs::create_dir_all(&dir);
    for (name, mono) in [(format!("{slug}-color.svg"), false), (format!("{slug}.svg"), true)] {
        match agent.get(&format!("{ICONS}/{name}")).header("User-Agent", "winfer").call() {
            Ok(mut res) => {
                let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
                if text.trim_start().starts_with("<svg") {
                    let _ = std::fs::write(dir.join(&name), &text);
                    return Ok(Some((text, mono)));
                }
            }
            Err(ureq::Error::StatusCode(404)) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let _ = std::fs::write(dir.join(format!("{slug}.none")), "");
    Ok(None)
}

use tauri::Manager;

#[cfg(test)]
mod tests {
    use super::maker_of;

    #[test]
    fn makers_from_model_ids() {
        assert_eq!(maker_of("local/Qwen3.6-35B-A3B-oQ4e-mtp"), Some("qwen"));
        assert_eq!(maker_of("local/gemma-4-26B-A4B-it-oQ4e-mtp"), Some("gemma"));
        assert_eq!(maker_of("local/Meta-Llama-3.1-8B"), Some("meta"));
        assert_eq!(maker_of("local/devstral-small-2"), Some("mistral"));
        assert_eq!(maker_of("local/gpt-oss-20b"), Some("openai"));
        assert_eq!(maker_of("local/Phi-4-mini"), Some("microsoft"));
        assert_eq!(maker_of("local/dolphin-2.9"), None);
        assert_eq!(maker_of("local/Yi-1.5-9B"), Some("yi"));
        assert_eq!(maker_of("local/DaisyRose-27B-4bit"), None);
    }

    #[test]
    fn small_drawings_are_cropped() {
        let svg = r#"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><rect x="6" y="8" width="12" height="8"/></svg>"#;
        assert!(super::cropped(svg).unwrap().contains(r#"viewBox="5.75 5.75 12.5 12.5""#));
        let full = r#"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="24" height="24"/></svg>"#;
        assert!(super::cropped(full).is_none());
    }
}
