//! The app's assets: gpui-kit's icon set plus the providers' brand marks (`assets/providers`).

use gpui_kit::{AssetSource, SharedString};
use std::borrow::Cow;

macro_rules! marks {
    ($($name:literal),*) => {
        const MARKS: &[(&str, &[u8])] = &[$((concat!("providers/", $name, ".svg"), include_bytes!(concat!("../assets/providers/", $name, ".svg")))),*];
    };
}

const SOLID: &[(&str, &[u8])] = &[
    ("solid/sparkle.svg", include_bytes!("../assets/solid/sparkle.svg")),
    ("solid/bolt.svg", include_bytes!("../assets/solid/bolt.svg")),
    ("solid/flame.svg", include_bytes!("../assets/solid/flame.svg")),
    ("solid/mode-ask.svg", include_bytes!("../assets/solid/mode-ask.svg")),
    ("solid/mode-edit.svg", include_bytes!("../assets/solid/mode-edit.svg")),
    ("solid/mode-plan.svg", include_bytes!("../assets/solid/mode-plan.svg")),
    ("solid/mode-flag.svg", include_bytes!("../assets/solid/mode-flag.svg")),
    ("solid/mode-yolo.svg", include_bytes!("../assets/solid/mode-yolo.svg")),
];

marks!("agy", "claude", "codex", "cursor", "grok", "local", "muse", "openai", "opencode", "pi", "winfer");

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = MARKS.iter().chain(SOLID).find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = MARKS.iter().chain(SOLID).filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect();
        out.extend(gpui_kit::assets::AllAssets.list(path)?);
        Ok(out)
    }
}
