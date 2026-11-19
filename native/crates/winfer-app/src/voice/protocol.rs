//! What live voice says to OpenAI's Realtime API over a WebSocket, and what it hears back
//! (port of the event handling in `features/voice/live/session.ts`).

use serde_json::{Value, json};

pub const RATE: u32 = 24_000;
pub const DEFAULT_MODEL: &str = "gpt-realtime";
pub const DEFAULT_VOICE: &str = "marin";
pub const URL: &str = "wss://api.openai.com/v1/realtime";

pub fn url(model: &str) -> String {
    format!("{URL}?model={}", if model.trim().is_empty() { DEFAULT_MODEL } else { model.trim() })
}

/// The session the model runs: its instructions, tools and how it hears and speaks. A `bare`
/// session has none of the extras (the last thing tried when OpenAI fails the fuller ones).
pub fn session(instructions: &str, tools: &[Value], voice: &str, bare: bool) -> Value {
    if bare {
        return json!({ "type": "realtime", "instructions": instructions });
    }
    let mut s = json!({
        "type": "realtime",
        "instructions": instructions,
        "audio": {
            "input": {
                "format": { "type": "audio/pcm", "rate": RATE },
                "transcription": { "model": "gpt-4o-mini-transcribe" },
                "noise_reduction": { "type": "far_field" },
                "turn_detection": { "type": "semantic_vad", "eagerness": "auto", "create_response": true, "interrupt_response": true },
            },
            "output": { "format": { "type": "audio/pcm", "rate": RATE }, "voice": if voice.trim().is_empty() { DEFAULT_VOICE } else { voice.trim() } },
        },
    });
    if !tools.is_empty() {
        s["tools"] = Value::Array(tools.to_vec());
        s["tool_choice"] = json!("auto");
    }
    s
}

pub fn session_update(session: &Value) -> Value {
    json!({ "type": "session.update", "session": session })
}

pub fn audio_append(pcm16: &[u8]) -> Value {
    use base64::Engine;
    json!({ "type": "input_audio_buffer.append", "audio": base64::engine::general_purpose::STANDARD.encode(pcm16) })
}

pub fn function_output(call_id: &str, output: &str) -> Value {
    json!({ "type": "conversation.item.create", "item": { "type": "function_call_output", "call_id": call_id, "output": output } })
}

pub fn user_note(text: &str) -> Value {
    json!({ "type": "conversation.item.create", "item": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": text }] } })
}

pub fn response_create() -> Value {
    json!({ "type": "response.create" })
}

/// A function call the model made, from a `response.done`.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

pub fn calls_of(response: &Value) -> Vec<Call> {
    response["output"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|i| i["type"] == "function_call")
                .map(|i| Call {
                    id: i["call_id"].as_str().unwrap_or_default().to_string(),
                    name: i["name"].as_str().unwrap_or_default().to_string(),
                    args: i["arguments"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or_else(|| json!({})),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// OpenAI errors no retry can fix (billing, auth): the message to stop with.
pub fn fatal_message(error: &Value) -> Option<String> {
    let code = format!("{} {}", error["type"].as_str().unwrap_or_default(), error["code"].as_str().unwrap_or_default());
    let has = |words: &[&str]| words.iter().any(|w| code.contains(w));
    if has(&["insufficient_quota", "credit_balance", "billing"]) {
        return Some("Your OpenAI API account has no credits left. Add credits at platform.openai.com → Settings → Billing (separate from a ChatGPT plan).".into());
    }
    if has(&["invalid_api_key", "authentication"]) {
        return Some("OpenAI rejected the API key. Check it in Settings → Voice → Live voice.".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_full_session_carries_audio_settings_and_tools() {
        let tool = json!({ "type": "function", "name": "list_panes", "description": "", "parameters": { "type": "object", "properties": {} } });
        let s = session("be brief", &[tool], "cedar", false);
        assert_eq!(s["type"], "realtime");
        assert_eq!(s["audio"]["output"]["voice"], "cedar");
        assert_eq!(s["audio"]["input"]["format"]["rate"], 24_000);
        assert_eq!(s["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(s["tool_choice"], "auto");
        let bare = session("be brief", &[], "", true);
        assert!(bare.get("audio").is_none() && bare.get("tools").is_none());
        assert_eq!(session("x", &[], "", false)["audio"]["output"]["voice"], DEFAULT_VOICE);
    }

    #[test]
    fn urls_default_the_model() {
        assert_eq!(url(" "), "wss://api.openai.com/v1/realtime?model=gpt-realtime");
        assert!(url("gpt-realtime-mini").ends_with("model=gpt-realtime-mini"));
    }

    #[test]
    fn function_calls_are_read_from_the_response() {
        let r = json!({ "output": [
            { "type": "message" },
            { "type": "function_call", "call_id": "c1", "name": "read_pane", "arguments": "{\"pane\":\"2\"}" },
            { "type": "function_call", "call_id": "c2", "name": "list_panes", "arguments": "" },
        ] });
        let calls = calls_of(&r);
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].id.as_str(), calls[0].args["pane"].as_str()), ("c1", Some("2")));
        assert_eq!(calls[1].args, json!({}));
    }

    #[test]
    fn billing_and_key_errors_are_fatal() {
        assert!(fatal_message(&json!({ "type": "insufficient_quota" })).unwrap().contains("no credits"));
        assert!(fatal_message(&json!({ "code": "invalid_api_key" })).unwrap().contains("rejected"));
        assert!(fatal_message(&json!({ "type": "server_error" })).is_none());
    }

    #[test]
    fn audio_is_base64() {
        let e = audio_append(&[1, 2, 3]);
        assert_eq!(e["type"], "input_audio_buffer.append");
        assert_eq!(e["audio"], "AQID");
    }
}
