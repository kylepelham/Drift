//! ChatGPT sign-in as Codex does it: PKCE in the browser, a callback on localhost:1455, tokens by form post.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::llm::anthropic::oauth::base64url;
use crate::llm::Credential;

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
pub const CALLBACK_PORT: u16 = 1455;
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPES: &str = "openid profile email offline_access";
const DEFAULT_EXPIRES_IN: i64 = 3600;

#[derive(Clone, Debug, PartialEq)]
pub struct Started {
    pub url: String,
    pub state: String,
    pub verifier: String,
}

pub fn start() -> Started {
    let verifier = base64url(&random(32));
    let challenge = base64url(&crate::llm::anthropic::oauth::sha256(verifier.as_bytes()));
    let state = base64url(&random(32));
    let params = [
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", REDIRECT_URI),
        ("scope", SCOPES),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", &state),
        ("originator", "opencode"),
    ];
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
    Started { url: format!("{ISSUER}/oauth/authorize?{}", query.join("&")), state, verifier }
}

/// Serves one callback on 1455 and returns the code once the browser lands, checking `state`.
pub async fn wait_for_callback(expected_state: &str) -> Result<String, String> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, CALLBACK_PORT)))
        .await
        .map_err(|e| format!("port {CALLBACK_PORT} is busy: {e}"))?;
    let (mut socket, params) = next_callback(&listener, "/auth/callback").await?;
    let state_ok = params.get("state").map(String::as_str) == Some(expected_state);
    match (params.get("code"), state_ok) {
        (Some(code), true) => {
            respond(&mut socket, 200, "Drift is connected to your ChatGPT account. You can close this tab and go back to the app.").await;
            Ok(code.clone())
        }
        _ => {
            respond(&mut socket, 400, "Sign-in failed: state mismatch or missing code.").await;
            Err("callback carried a bad state or no code".into())
        }
    }
}

/// The browser's request to `path` on `listener`, with its query; anything else gets a 404 and is skipped.
pub(crate) async fn next_callback(listener: &TcpListener, path: &str) -> Result<(tokio::net::TcpStream, HashMap<String, String>), String> {
    loop {
        let (mut socket, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut buffer = vec![0u8; 8192];
        let read = socket.read(&mut buffer).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..read]);
        let Some(target) = request.split_whitespace().nth(1) else { continue };
        match target.split_once('?') {
            Some((at, query)) if at == path => return Ok((socket, parse_query(query))),
            _ => respond(&mut socket, 404, "Not found").await,
        }
    }
}

/// Answers the browser with Drift's sign-in page.
pub(crate) async fn respond(socket: &mut tokio::net::TcpStream, status: u16, text: &str) {
    let reason = if status == 200 { "OK" } else { "Error" };
    let body = include_str!("callback.html").replace("{title}", if status == 200 { "Signed in" } else { "Sign-in failed" }).replace("{text}", text);
    let response = format!("HTTP/1.1 {status} {reason}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}
pub async fn exchange(client: &reqwest::Client, code: &str, verifier: &str) -> Result<Credential, String> {
    token_request(client, &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", REDIRECT_URI), ("client_id", CLIENT_ID), ("code_verifier", verifier)]).await
}

pub async fn refresh(client: &reqwest::Client, refresh_token: &str) -> Result<Credential, String> {
    token_request(client, &[("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", CLIENT_ID)]).await
}

async fn token_request(client: &reqwest::Client, form: &[(&str, &str)]) -> Result<Credential, String> {
    let encoded: Vec<String> = form.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
    let timeouts = crate::llm::http::Timeouts::default();
    let request = client.post(format!("{ISSUER}/oauth/token")).header("content-type", "application/x-www-form-urlencoded").body(encoded.join("&"));
    let response = crate::llm::http::send(request, &timeouts).await.map_err(|e| e.to_string())?;
    let status = response.status();
    let text = crate::llm::http::bounded_body(response, &timeouts).await;
    if !status.is_success() {
        return Err(format!("token request failed ({status}): {text}"));
    }
    let json: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let field = |key: &str| json[key].as_str().map(str::to_string).ok_or_else(|| format!("token response lacks {key}"));
    let access = field("access_token")?;
    let account = json["id_token"].as_str().and_then(account_id).or_else(|| account_id(&access));
    Ok(Credential::OAuth {
        access,
        refresh: field("refresh_token")?,
        expires_at: crate::id::now_ms() + json["expires_in"].as_i64().unwrap_or(DEFAULT_EXPIRES_IN) * 1000,
        account,
    })
}

/// The ChatGPT account id lives in the JWT claims, in one of three places depending on the token.
pub fn account_id(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let found = [&claims["chatgpt_account_id"], &claims["https://api.openai.com/auth"]["chatgpt_account_id"], &claims["organizations"][0]["id"]]
        .iter()
        .find_map(|v| v.as_str().map(str::to_string));
    found
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), decode(v)))
        .collect()
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = bytes[i] == b'%' && i + 2 < bytes.len();
        let byte = escaped.then(|| u8::from_str_radix(&value[i + 1..i + 3], 16).ok()).flatten();
        match byte {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Form encoding for a value in a query or `application/x-www-form-urlencoded` body.
pub(crate) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut bits = 0u32;
    let mut count = 0;
    for byte in text.bytes().filter(|b| *b != b'=') {
        let value = TABLE.iter().position(|t| *t == byte).or(match byte {
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        })? as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

fn random(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).expect("system random source unavailable");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_has_the_codex_parameters() {
        let started = start();
        assert!(started.url.starts_with("https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(started.url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
        assert!(started.url.contains("scope=openid+profile+email+offline_access"));
        assert!(started.url.contains("codex_cli_simplified_flow=true"));
        assert!(started.url.ends_with("&originator=opencode"));
    }

    #[test]
    fn account_id_comes_from_any_of_the_claim_paths() {
        let jwt = |claims: &str| format!("h.{}.s", base64url(claims.as_bytes()));
        assert_eq!(account_id(&jwt(r#"{"chatgpt_account_id":"acc_1"}"#)), Some("acc_1".into()));
        assert_eq!(account_id(&jwt(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acc_2"}}"#)), Some("acc_2".into()));
        assert_eq!(account_id(&jwt(r#"{"organizations":[{"id":"org_3"}]}"#)), Some("org_3".into()));
        assert_eq!(account_id(&jwt(r#"{}"#)), None);
        assert_eq!(account_id("garbage"), None);
    }

    #[tokio::test]
    async fn callback_listener_accepts_the_matching_state() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let waiting = tokio::spawn(wait_for_callback("st"));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let response = reqwest::Client::new().get("http://127.0.0.1:1455/auth/callback?code=abc%20d&state=st").send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(waiting.await.unwrap().unwrap(), "abc d");
    }

    #[test]
    fn query_decoding_handles_percent_and_plus() {
        assert_eq!(decode("a%20b+c"), "a b c");
        assert_eq!(decode("100%"), "100%");
    }
}
