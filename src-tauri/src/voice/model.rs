//! Whisper speech-to-text models, downloaded from Hugging Face on first use and verified.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Pinned revision of ggerganov/whisper.cpp so files never change under us.
const REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpec {
    pub id: &'static str,
    pub file: &'static str,
    pub size: u64,
    #[serde(skip)]
    pub sha256: &'static str,
    pub label: &'static str,
}

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "base.en",
        file: "ggml-base.en.bin",
        size: 147_964_211,
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        label: "Base (English, 148 MB) — fast",
    },
    ModelSpec {
        id: "small.en",
        file: "ggml-small.en.bin",
        size: 487_614_201,
        sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
        label: "Small (English, 488 MB) — more accurate",
    },
];

pub const DEFAULT_MODEL: &str = "base.en";

pub fn spec(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.id == id)
}

pub fn path(dir: &Path, spec: &ModelSpec) -> PathBuf {
    dir.join(spec.file)
}

pub fn installed(dir: &Path, spec: &ModelSpec) -> bool {
    std::fs::metadata(path(dir, spec)).is_ok_and(|m| m.len() == spec.size)
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Downloads (resuming a partial file) and verifies the model's SHA-256.
pub fn download(dir: &Path, spec: &ModelSpec) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let target = path(dir, spec);
    let part = target.with_extension("part");
    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/{REVISION}/{}",
        spec.file
    );

    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-C", "-", "-o"])
        .arg(&part)
        .arg(&url)
        .status()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !status.success() {
        return Err(format!("download failed ({status})"));
    }
    let hash = sha256_file(&part).map_err(|e| e.to_string())?;
    if hash != spec.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{} failed verification (sha256 {hash})", spec.file));
    }
    std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downloads into WINFER_MODELS_DIR through the real code path (curl + SHA-256 check).
    #[test]
    #[ignore]
    fn downloads_and_verifies_base_model() {
        let dir = PathBuf::from(std::env::var("WINFER_MODELS_DIR").expect("set WINFER_MODELS_DIR"));
        let spec = spec(DEFAULT_MODEL).unwrap();
        let path = download(&dir, spec).unwrap();
        assert!(installed(&dir, spec), "{}", path.display());
    }
}
