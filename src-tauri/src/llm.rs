//! A text-writing LLM for voice "smart moves": turns what the user said (plus context) into the
//! message an agent receives. Clef still decides which action runs; this only writes text.
//!
//! Providers: Cloudflare Workers AI (reuses the voice credentials) or OpenAI through the user's
//! ChatGPT plan via the official Codex CLI (`codex exec`, which uses its own ChatGPT login).

use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::env::login_env;
use crate::voice::cloud;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeRequest {
    /// "workers" or "codex".
    provider: String,
    /// Workers AI model id (e.g. "@cf/openai/gpt-oss-20b") or Codex model (empty = its default).
    #[serde(default)]
    model: String,
    system: String,
    user: String,
}

/// Pulls the text out of a Workers AI response (chat-completions or legacy `response` shape)
/// and drops reasoning blocks some models emit.
fn workers_text(result: &Value) -> Option<String> {
    let text = result["choices"][0]["message"]["content"]
        .as_str()
        .or_else(|| result["response"].as_str())?;
    let text = match (text.find("<think>"), text.find("</think>")) {
        (Some(_), Some(end)) => &text[end + "</think>".len()..],
        _ => text,
    };
    let text = text.trim().trim_matches('"').trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn workers(model: &str, system: &str, user: &str) -> Result<String, String> {
    let creds = cloud::load().ok_or("Add your Cloudflare credentials in Settings to use the prompt writer.")?;
    let model = if model.is_empty() { "@cf/openai/gpt-oss-20b" } else { model };
    let url = format!("https://api.cloudflare.com/client/v4/accounts/{}/ai/run/{model}", creds.account_id);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let body = json!({
        "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }],
        "max_tokens": 600,
        "temperature": 0.3
    });
    let mut res = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {}", creds.token))
        .send_json(&body)
        .map_err(|e| format!("couldn't reach Cloudflare: {e}"))?;
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    // Models occasionally put raw control characters in strings; parse leniently.
    let json: Value = serde_json::from_str(&text)
        .or_else(|_| serde_json::from_str(&text.replace(['\n', '\r', '\t'], " ")))
        .map_err(|e| format!("bad response from Workers AI: {e}"))?;
    if json["success"] == false {
        return Err(format!("Workers AI: {}", json["errors"][0]["message"].as_str().unwrap_or("request failed")));
    }
    workers_text(&json["result"]).ok_or_else(|| "Workers AI returned no text".into())
}

/// OpenAI through the user's ChatGPT plan: the official Codex CLI, read-only and ephemeral.
fn codex(model: &str, system: &str, user: &str) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("winfer-llm-{:x}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join("answer.txt");
    let mut cmd = std::process::Command::new("codex");
    cmd.envs(login_env())
        .current_dir(&dir)
        .args(["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "-o"])
        .arg(&out);
    if !model.is_empty() {
        cmd.args(["-m", model]);
    }
    cmd.arg(format!("{system}\n\n{user}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("couldn't run codex: {e}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return Err("codex took too long".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let answer = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    if !status.success() || answer.trim().is_empty() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            let _ = stderr.read_to_string(&mut err);
        }
        return Err(format!("codex failed: {}", err.lines().last().unwrap_or("no answer")));
    }
    Ok(answer.trim().to_string())
}

#[tauri::command]
pub async fn llm_compose(req: ComposeRequest) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || match req.provider.as_str() {
        "workers" => workers(&req.model, &req.system, &req.user),
        "codex" => codex(&req.model, &req.system, &req.user),
        other => Err(format!("unknown LLM provider {other}")),
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_response_shapes() {
        let chat = json!({ "choices": [{ "message": { "content": "<think>hmm</think>\n\"Add a dark mode.\"" } }] });
        assert_eq!(workers_text(&chat).as_deref(), Some("Add a dark mode."));
        assert_eq!(workers_text(&json!({ "response": "Run the tests." })).as_deref(), Some("Run the tests."));
        assert_eq!(workers_text(&json!({ "response": "  " })), None);
    }

    /// Live: CF creds in the Keychain/env (workers) and a logged-in Codex (codex).
    #[test]
    #[ignore]
    fn live_compose() {
        for (provider, model) in [("workers", ""), ("codex", "")] {
            let started = std::time::Instant::now();
            let result = match provider {
                "workers" => workers(model, "Rewrite as a one-sentence instruction for a coding agent. Output only it.", "Spoken: make the button blue"),
                _ => codex(model, "Rewrite as a one-sentence instruction for a coding agent. Output only it.", "Spoken: make the button blue"),
            };
            eprintln!("{provider}: {result:?} in {:?}", started.elapsed());
            assert!(result.is_ok());
        }
    }
}
