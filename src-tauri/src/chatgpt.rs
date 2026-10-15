//! Sign in with ChatGPT: winfer's own OAuth registration with OpenAI (developers.openai.com/siwc,
//! "ChatGPT plan usage for open-source apps"). The user signs in in the browser, names the agent
//! and allows winfer to use their ChatGPT plan; winfer then calls the OpenAI API with that token:
//! the Responses API for the LLM (prompt writer, voice planner, progress, review summaries). The
//! Decisions API isn't available with a ChatGPT plan.
//!
//! Tokens live in the OS keychain. Codex's own login is never read.

use std::io::{BufRead, BufReader};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SERVICE: &str = "dev.winfer.app";
const ACCOUNT_KEY: &str = "chatgpt-account";
const HOST_KEY: &str = "chatgpt-host-id";
const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const ISSUER: &str = "https://auth.openai.com";
const API: &str = "https://api.openai.com/v1";
const SCOPES: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// First-time sign-ins register a new client; later ones reuse the issued client id.
const REGISTER_CLIENT: &str = "dynamic_agent_client";
const AGENT_NAME: &str = "winfer";
/// The documented loopback port; only the port may vary if it's taken.
const PORTS: [u16; 4] = [1455, 1456, 1457, 1458];

/// The signed-in account. Saved as one keychain item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Account {
    client_id: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    /// Unix seconds.
    #[serde(default)]
    expires_at: u64,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
}

impl Account {
    fn signed_in(&self) -> bool {
        !self.refresh_token.is_empty()
    }
}

fn entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())
}

fn load() -> Option<Account> {
    serde_json::from_str(&entry(ACCOUNT_KEY).ok()?.get_password().ok()?).ok()
}

fn save(account: &Account) -> Result<(), String> {
    entry(ACCOUNT_KEY)?
        .set_password(&serde_json::to_string(account).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

/// A random (version 4) UUID as a URN, the form OpenAI expects for `ext_agent_host_id`.
fn uuid_urn() -> String {
    let mut b = rand::random::<[u8; 16]>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!("urn:uuid:{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// A stable id for this installation, required by the registration flow.
fn host_id() -> Result<String, String> {
    let entry = entry(HOST_KEY)?;
    // Ids saved before they had to be URNs were rejected by OpenAI; replace them.
    if let Ok(id) = entry.get_password()
        && id.starts_with("urn:uuid:")
    {
        return Ok(id);
    }
    let id = uuid_urn();
    entry.set_password(&id).map_err(|e| e.to_string())?;
    Ok(id)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(value: &str) -> String {
    let bytes = value.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == name).then(|| decode(v))
    })
}

fn authorize_url(params: &[(&str, &str)]) -> String {
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
    format!("{AUTHORIZE_URL}?{}", query.join("&"))
}

/// The ID token's claims. It comes straight from the token endpoint over TLS, so the claims are
/// checked (issuer, audience, expiry, nonce) without re-verifying the signature.
fn id_claims(id_token: &str) -> Result<Value, String> {
    let payload = id_token.split('.').nth(1).ok_or("malformed ID token")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| "malformed ID token")?;
    serde_json::from_slice(&bytes).map_err(|_| "malformed ID token".into())
}

fn check_id_token(claims: &Value, client_id: &str, nonce: Option<&str>) -> Result<(), String> {
    if claims["iss"].as_str() != Some(ISSUER) {
        return Err("ID token from an unexpected issuer".into());
    }
    let audience_ok = match &claims["aud"] {
        Value::String(aud) => aud == client_id,
        Value::Array(list) => list.iter().any(|a| a.as_str() == Some(client_id)),
        _ => false,
    };
    if !audience_ok {
        return Err("ID token for a different app".into());
    }
    if claims["exp"].as_u64().is_some_and(|exp| exp + 300 < now()) {
        return Err("ID token expired".into());
    }
    if let Some(nonce) = nonce
        && claims["nonce"].as_str() != Some(nonce)
    {
        return Err("ID token nonce doesn't match".into());
    }
    Ok(())
}

fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn form(params: &[(&str, &str)]) -> String {
    params.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect::<Vec<_>>().join("&")
}

/// Posts to the token endpoint; returns the token response or the OAuth error code.
fn token_request(params: &[(&str, &str)]) -> Result<Value, String> {
    let mut res = http()
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(form(params))
        .map_err(|e| format!("couldn't reach OpenAI: {e}"))?;
    let status = res.status();
    let json: Value = res.body_mut().read_json().unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(json["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("sign-in failed ({status})")));
    }
    Ok(json)
}

fn apply_tokens(account: &mut Account, tokens: &Value) {
    if let Some(t) = tokens["access_token"].as_str() {
        account.access_token = t.into();
    }
    if let Some(t) = tokens["refresh_token"].as_str() {
        account.refresh_token = t.into();
    }
    if let Some(t) = tokens["id_token"].as_str() {
        account.id_token = t.into();
    }
    account.expires_at = now() + tokens["expires_in"].as_u64().unwrap_or(3600);
    if let Some(scope) = tokens["scope"].as_str() {
        account.scopes = scope.split(' ').map(str::to_string).collect();
    }
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>winfer</title><body style=\"font:15px system-ui;margin:4em;text-align:center\"><h2>Signed in to winfer</h2><p>You can close this window.</p>";

fn fail_page(message: &str) -> String {
    format!("<!doctype html><meta charset=utf-8><title>winfer</title><body style=\"font:15px system-ui;margin:4em;text-align:center\"><h2>Sign-in didn't finish</h2><p>{}</p>", message.replace('<', "&lt;"))
}

/// Set to stop a sign-in that's waiting for the browser.
static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runs the whole browser sign-in; blocks until the user finishes, cancels, or 5 minutes pass.
fn sign_in(open: impl Fn(&str) -> Result<(), String>) -> Result<Account, String> {
    let host = host_id()?;
    let previous = load();
    let (server, port) = PORTS
        .iter()
        .find_map(|&p| tiny_http::Server::http(("127.0.0.1", p)).ok().map(|s| (s, p)))
        .ok_or("Ports 1455–1458 are in use; close the other sign-in and try again.")?;
    let redirect = format!("http://127.0.0.1:{port}/auth/callback");
    let state = random_token();
    let nonce = random_token();
    let verifier = random_token();
    let challenge = pkce_challenge(&verifier);

    // Reauthorize with the issued client id when there is one; otherwise register.
    let registered = previous.as_ref().map(|a| a.client_id.clone()).filter(|id| !id.is_empty());
    let mut params: Vec<(&str, &str)> = vec![
        ("client_id", registered.as_deref().unwrap_or(REGISTER_CLIENT)),
        ("ext_agent_host_id", &host),
        ("response_type", "code"),
        ("redirect_uri", &redirect),
        ("scope", SCOPES),
        ("resource", API),
        ("state", &state),
        ("nonce", &nonce),
        ("code_challenge_method", "S256"),
        ("code_challenge", &challenge),
    ];
    if registered.is_none() {
        params.push(("agent_name_hint", AGENT_NAME));
    }
    let hint = previous.as_ref().filter(|a| !a.id_token.is_empty()).map(|a| a.id_token.clone());
    if let Some(hint) = &hint {
        params.push(("id_token_hint", hint));
    }
    CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
    open(&authorize_url(&params))?;

    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let (code, issued) = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err("Sign-in timed out.".into());
        }
        if CANCEL.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Sign-in cancelled.".into());
        }
        let Some(request) = server.recv_timeout(left.min(Duration::from_millis(250))).map_err(|e| e.to_string())? else {
            continue;
        };
        let url = request.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        if path != "/auth/callback" {
            let _ = request.respond(tiny_http::Response::empty(404));
            continue;
        }
        let html = |body: String| {
            tiny_http::Response::from_string(body)
                .with_header("Content-Type: text/html; charset=utf-8".parse::<tiny_http::Header>().unwrap())
        };
        if query_param(query, "state").as_deref() != Some(state.as_str()) {
            let _ = request.respond(html(fail_page("The response didn't match this sign-in. Try again from winfer.")));
            return Err("Sign-in response didn't match (state).".into());
        }
        if let Some(error) = query_param(query, "error") {
            let message = if error == "access_denied" { "You declined the sign-in.".to_string() } else { format!("OpenAI: {error}") };
            let _ = request.respond(html(fail_page(&message)));
            return Err(message);
        }
        let Some(code) = query_param(query, "code") else {
            let _ = request.respond(html(fail_page("No authorization code came back.")));
            return Err("No authorization code came back.".into());
        };
        let issued = query_param(query, "client_id");
        let _ = request.respond(html(DONE_PAGE.to_string()));
        break (code, issued);
    };

    let client_id = match (&registered, issued) {
        (Some(id), Some(issued)) if *id != issued => return Err("OpenAI returned a different app registration.".into()),
        (Some(id), _) => id.clone(),
        (None, Some(issued)) if issued != REGISTER_CLIENT => issued,
        (None, _) => return Err("OpenAI didn't finish registering winfer. Try again.".into()),
    };
    let tokens = token_request(&[
        ("grant_type", "authorization_code"),
        ("client_id", &client_id),
        ("code", &code),
        ("code_verifier", &verifier),
        ("redirect_uri", &redirect),
        ("resource", API),
    ])?;
    let mut account = Account { client_id: client_id.clone(), ..Default::default() };
    apply_tokens(&mut account, &tokens);
    let claims = id_claims(&account.id_token)?;
    check_id_token(&claims, &client_id, Some(&nonce))?;
    account.email = claims["email"].as_str().map(str::to_string);
    if !account.scopes.iter().any(|s| s == PLAN_SCOPE) {
        save(&Account { client_id, id_token: account.id_token.clone(), email: account.email.clone(), ..Default::default() })?;
        return Err("Signed in, but using your ChatGPT plan wasn't allowed. Sign in again and allow it.".into());
    }
    save(&account)?;
    Ok(account)
}

/// Serializes refreshes: refresh tokens rotate, so two at once would invalidate each other.
static REFRESH: Mutex<()> = Mutex::new(());

/// A valid access token, refreshed when it's about to expire.
fn access_token() -> Result<String, String> {
    let _guard = REFRESH.lock().unwrap();
    let mut account = load().filter(Account::signed_in).ok_or("Sign in with ChatGPT in Settings first.")?;
    if account.expires_at > now() + 120 {
        return Ok(account.access_token);
    }
    match token_request(&[
        ("grant_type", "refresh_token"),
        ("client_id", &account.client_id),
        ("refresh_token", &account.refresh_token),
    ]) {
        Ok(tokens) => {
            apply_tokens(&mut account, &tokens);
            save(&account)?;
            Ok(account.access_token)
        }
        Err(code)
            if matches!(
                code.as_str(),
                "invalid_grant" | "invalid_refresh_token" | "token_expired" | "refresh_token_expired" | "refresh_token_invalidated" | "refresh_token_reused"
            ) =>
        {
            // The grant is gone: keep the registration, drop the tokens.
            save(&Account { client_id: account.client_id, id_token: account.id_token, email: account.email, ..Default::default() })?;
            Err("Your ChatGPT sign-in expired. Sign in again in Settings.".into())
        }
        Err(code) => Err(format!("Couldn't renew the ChatGPT sign-in: {code}")),
    }
}

/// Turns an OpenAI error body into a message for the user (`code` is kept for callers).
fn api_error(status: u16, body: &Value) -> (String, String) {
    let code = body["error"]["code"].as_str().unwrap_or("").to_string();
    let message = match code.as_str() {
        "subscription_sharing_usage_limit_exceeded" => "Your ChatGPT plan's limit for winfer is used up (ChatGPT settings → Usage).".into(),
        "subscription_sharing_user_not_eligible" => "Your ChatGPT plan or workspace doesn't allow apps to use it.".into(),
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => "ChatGPT plan usage is unavailable right now; try again shortly.".into(),
        "subscription_sharing_route_not_supported" => "OpenAI doesn't allow this endpoint with a ChatGPT plan.".into(),
        "subscription_sharing_unsupported_capability" => format!("Not supported with a ChatGPT plan: {}", body["error"]["param"].as_str().unwrap_or("this request")),
        "subscription_sharing_invalid_user" => "OpenAI couldn't validate your ChatGPT sign-in.".into(),
        _ => body["error"]["message"]
            .as_str()
            .or_else(|| body["detail"].as_str())
            .map(|m| format!("OpenAI {status}: {m}"))
            .unwrap_or_else(|| format!("OpenAI request failed ({status})")),
    };
    (code, message)
}

/// The text of a Responses API stream (`stream: true` is required with a ChatGPT plan).
fn read_stream(reader: impl std::io::Read) -> Result<String, String> {
    let mut text = String::new();
    for line in BufReader::new(reader).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(event) = serde_json::from_str::<Value>(data.trim()) else { continue };
        match event["type"].as_str() {
            Some("response.output_text.delta") => text.push_str(event["delta"].as_str().unwrap_or("")),
            Some("response.completed") => return Ok(text),
            Some("response.failed") | Some("error") => {
                let error = if event["response"]["error"].is_object() { &event["response"]["error"] } else { &event["error"] };
                return Err(api_error(200, &json!({ "error": error })).1);
            }
            _ => {}
        }
    }
    Err("OpenAI's answer stopped before it finished.".into())
}

/// One Responses API call with the user's ChatGPT plan; returns the answer text.
pub fn respond(model: &str, instructions: &str, input: &str) -> Result<String, String> {
    let token = access_token()?;
    let body = json!({
        "model": if model.is_empty() { DEFAULT_MODEL } else { model },
        "instructions": instructions,
        "input": [{ "role": "user", "content": [{ "type": "input_text", "text": input }] }],
        "store": false,
        "stream": true,
    });
    let mut res = http()
        .post(&format!("{API}/responses"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "text/event-stream")
        .send_json(&body)
        .map_err(|e| format!("couldn't reach OpenAI: {e}"))?;
    let status = res.status().as_u16();
    if !(200..300).contains(&status) {
        let json: Value = res.body_mut().read_json().unwrap_or(Value::Null);
        return Err(api_error(status, &json).1);
    }
    read_stream(res.body_mut().as_reader())
}

/// Default model for the LLM: fast enough for voice, smart enough to plan.
pub const DEFAULT_MODEL: &str = "gpt-6-luna";
/// Models the signed-in account can use.
pub fn models() -> Result<Vec<String>, String> {
    let token = access_token()?;
    let mut res = http()
        .get(&format!("{API}/models"))
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| format!("couldn't reach OpenAI: {e}"))?;
    let status = res.status().as_u16();
    let json: Value = res.body_mut().read_json().unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        return Err(api_error(status, &json).1);
    }
    Ok(listed_models(&json))
}

/// Model ids from `/v1/models`: `{ models: [{ slug, visibility }] }` for a ChatGPT plan (or the
/// API's `{ data: [{ id }] }`), without hidden ones.
fn listed_models(json: &Value) -> Vec<String> {
    let list = json["models"].as_array().or_else(|| json["data"].as_array());
    list.map(|list| {
        list.iter()
            .filter(|m| m["visibility"].as_str().is_none_or(|v| v == "list"))
            .filter_map(|m| m["slug"].as_str().or_else(|| m["id"].as_str()).map(str::to_string))
            .collect()
    })
    .unwrap_or_default()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptStatus {
    signed_in: bool,
    email: Option<String>,
}

impl ChatGptStatus {
    pub fn signed_in(&self) -> bool {
        self.signed_in
    }
}

#[tauri::command]
pub fn chatgpt_status() -> ChatGptStatus {
    let account = load().unwrap_or_default();
    ChatGptStatus { signed_in: account.signed_in(), email: account.email }
}

#[tauri::command]
pub async fn chatgpt_sign_in(app: tauri::AppHandle) -> Result<ChatGptStatus, String> {
    use tauri_plugin_opener::OpenerExt;
    let account = tauri::async_runtime::spawn_blocking(move || {
        sign_in(|url| app.opener().open_url(url, None::<&str>).map_err(|e| format!("couldn't open the browser: {e}")))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(ChatGptStatus { signed_in: true, email: account.email })
}

#[tauri::command]
pub fn chatgpt_cancel_sign_in() {
    CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Forgets the tokens; the app registration is kept so the next sign-in skips registering.
/// (Disconnecting winfer from the ChatGPT account happens in ChatGPT's settings.)
#[tauri::command]
pub fn chatgpt_sign_out() -> Result<(), String> {
    if let Some(account) = load() {
        save(&Account { client_id: account.client_id, ..Default::default() })?;
    }
    Ok(())
}

#[tauri::command]
pub async fn chatgpt_models() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(models).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_the_rfc_example() {
        // RFC 7636, appendix B.
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn host_ids_are_uuid_urns() {
        let id = uuid_urn();
        let uuid = id.strip_prefix("urn:uuid:").unwrap();
        let parts: Vec<&str> = uuid.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12]);
        assert!(parts[2].starts_with('4'));
        assert!(matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(id, uuid_urn());
    }

    #[test]
    fn builds_the_authorize_url() {
        let url = authorize_url(&[("client_id", REGISTER_CLIENT), ("redirect_uri", "http://127.0.0.1:1455/auth/callback"), ("scope", SCOPES)]);
        assert!(url.starts_with("https://auth.openai.com/api/accounts/authorize?client_id=dynamic_agent_client&"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback"));
        assert!(url.contains("scope=openid%20profile%20email%20offline_access%20resource.invoke%20chatgpt.tokens.use.direct"));
    }

    #[test]
    fn reads_the_callback_query() {
        let q = "code=abc%2B1&scope=chatgpt.tokens.use.direct+email&state=xyz&client_id=oaiapp_123";
        assert_eq!(query_param(q, "code").as_deref(), Some("abc+1"));
        assert_eq!(query_param(q, "scope").as_deref(), Some("chatgpt.tokens.use.direct email"));
        assert_eq!(query_param(q, "client_id").as_deref(), Some("oaiapp_123"));
        assert_eq!(query_param(q, "error"), None);
    }

    fn id_token(claims: Value) -> String {
        format!("x.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    #[test]
    fn checks_the_id_token_claims() {
        let good = json!({ "iss": ISSUER, "aud": "oaiapp_1", "exp": now() + 600, "nonce": "n1", "sub": "u1", "email": "a@b.c" });
        let claims = id_claims(&id_token(good.clone())).unwrap();
        assert!(check_id_token(&claims, "oaiapp_1", Some("n1")).is_ok());
        assert!(check_id_token(&claims, "oaiapp_2", Some("n1")).is_err());
        assert!(check_id_token(&claims, "oaiapp_1", Some("n2")).is_err());
        let mut other = good.clone();
        other["iss"] = json!("https://evil.example");
        assert!(check_id_token(&other, "oaiapp_1", Some("n1")).is_err());
        let mut listed = good;
        listed["aud"] = json!(["oaiapp_1", "x"]);
        assert!(check_id_token(&listed, "oaiapp_1", Some("n1")).is_ok());
    }

    #[test]
    fn reads_a_responses_stream() {
        let stream = "event: response.created\ndata: {\"type\":\"response.created\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Add a \"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"dark mode.\"}\n\ndata: {\"type\":\"response.completed\"}\n\n";
        assert_eq!(read_stream(stream.as_bytes()).unwrap(), "Add a dark mode.");
        let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\"}}}\n";
        assert!(read_stream(failed.as_bytes()).unwrap_err().contains("limit"));
        assert!(read_stream("data: {\"type\":\"response.output_text.delta\",\"delta\":\"cut\"}\n".as_bytes()).is_err());
    }

    #[test]
    fn lists_visible_models() {
        let plan = json!({ "models": [{ "slug": "gpt-6-luna", "visibility": "list" }, { "slug": "gpt-reserve", "visibility": "hide" }] });
        assert_eq!(listed_models(&plan), ["gpt-6-luna"]);
        assert_eq!(listed_models(&json!({ "data": [{ "id": "gpt-6-sol" }] })), ["gpt-6-sol"]);
    }

    #[test]
    fn explains_plan_errors() {
        let body = json!({ "error": { "code": "subscription_sharing_route_not_supported", "message": "nope" } });
        let (code, message) = api_error(403, &body);
        assert_eq!(code, "subscription_sharing_route_not_supported");
        assert!(message.contains("doesn't allow this endpoint"));
        assert!(api_error(401, &json!({ "detail": "bad token" })).1.contains("bad token"));
    }
}
