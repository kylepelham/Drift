//! Claude subscription sign-in: the PKCE flow Claude Code uses, so Pro and Max plans work without a key.

use serde_json::{Value, json};

use crate::llm::{Credential, OAuthError};

pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_MAX: &str = "https://claude.ai/oauth/authorize";
const AUTHORIZE_CONSOLE: &str = "https://platform.claude.com/oauth/authorize";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

/// A fake token endpoint for the engine's own tests; release builds always use `TOKEN_URL`.
#[cfg(test)]
pub(crate) static TEST_TOKEN_URL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn token_url() -> String {
    #[cfg(test)]
    if let Some(url) = TEST_TOKEN_URL.lock().unwrap().clone() {
        return url;
    }

    TOKEN_URL.into()
}
const SCOPES: &str =
    "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
/// The token endpoint checks this; it is what the reference client sends.
const TOKEN_USER_AGENT: &str = "axios/1.13.6";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Claude Pro or Max subscription.
    Max,
    /// Anthropic Console account.
    Console,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Started {
    pub url: String,
    pub state: String,
    pub verifier: String,
}

pub fn start(mode: Mode) -> Started {
    let verifier = base64url(&random_bytes(64));
    let challenge = base64url(&sha256(verifier.as_bytes()));
    let state = crate::random_hex(16);
    let base = match mode {
        Mode::Max => AUTHORIZE_MAX,
        Mode::Console => AUTHORIZE_CONSOLE,
    };
    let url = format!(
        "{base}?code=true&client_id={CLIENT_ID}&response_type=code&redirect_uri={}&scope={}\
         &code_challenge={challenge}&code_challenge_method=S256&state={state}",
        encode(REDIRECT_URI),
        encode(SCOPES)
    );

    Started { url, state, verifier }
}

/// Accepts what the user pastes: `code#state`, a full callback URL, or a bare query string.
pub fn parse_callback(input: &str) -> Option<(String, String)> {
    let input = input.trim();
    if let Some((code, state)) = input.split_once('#') {
        return Some((code.into(), state.into()));
    }

    let query = input.split_once('?').map_or(input, |(_, query)| query);
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("code", value)) => code = Some(value.to_string()),
            Some(("state", value)) => state = Some(value.to_string()),
            _ => {}
        }
    }

    Some((code?, state?))
}

pub async fn exchange(
    client: &reqwest::Client,
    code: &str,
    state: &str,
    verifier: &str,
) -> Result<Credential, OAuthError> {
    let body = json!({
        "code": code,
        "state": state,
        "grant_type": "authorization_code",
        "client_id": CLIENT_ID,
        "redirect_uri": REDIRECT_URI,
        "code_verifier": verifier,
    });

    token_request(client, &body).await
}

pub async fn refresh(client: &reqwest::Client, refresh_token: &str) -> Result<Credential, OAuthError> {
    let body = json!({ "grant_type": "refresh_token", "refresh_token": refresh_token, "client_id": CLIENT_ID });

    token_request(client, &body).await
}

async fn token_request(client: &reqwest::Client, body: &Value) -> Result<Credential, OAuthError> {
    let timeouts = crate::llm::http::Timeouts::default();
    let request = client
        .post(token_url())
        .header("accept", "application/json, text/plain, */*")
        .header("user-agent", TOKEN_USER_AGENT)
        .json(body);

    let response = crate::llm::http::send(request, &timeouts).await?;
    let status = response.status();
    let text = crate::llm::http::bounded_body(response, &timeouts).await;
    if !status.is_success() {
        return Err(OAuthError::TokenResponse { status, text });
    }

    let json: Value = serde_json::from_str(&text)?;
    let field = |key: &str| {
        json[key]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| OAuthError::MissingTokenField(key.to_owned()))
    };
    let expires_in = json["expires_in"].as_i64().unwrap_or(0);

    Ok(Credential::OAuth {
        access: field("access_token")?,
        refresh: field("refresh_token")?,
        expires_at: crate::id::now_ms() + expires_in * 1000,
        account: None,
    })
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).expect("system random source unavailable");

    bytes
}

pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(data).into()
}

pub(crate) fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let bits = chunk.iter().enumerate().fold(0u32, |accumulator, (index, byte)| {
            accumulator | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..chunk.len() + 1 {
            let symbol = TABLE[((bits >> (18 - 6 * index)) & 63) as usize];
            out.push(symbol as char);
        }
    }

    out
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_carries_pkce_and_scopes() {
        let started = start(Mode::Max);
        assert!(
            started
                .url
                .starts_with("https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a")
        );
        assert!(started.url.contains("code_challenge_method=S256"));
        assert!(started.url.contains("scope=org%3Acreate_api_key%20user%3Aprofile"));
        assert!(started.url.contains(&format!("state={}", started.state)));
        assert_eq!(started.verifier.len(), 86);
        assert!(
            start(Mode::Console)
                .url
                .starts_with("https://platform.claude.com/oauth/authorize")
        );
    }

    #[test]
    fn callback_forms_are_all_accepted() {
        assert_eq!(parse_callback(" abc#st "), Some(("abc".into(), "st".into())));
        assert_eq!(
            parse_callback("https://x/cb?code=abc&state=st"),
            Some(("abc".into(), "st".into()))
        );
        assert_eq!(parse_callback("state=st&code=abc"), Some(("abc".into(), "st".into())));
        assert_eq!(parse_callback("code=abc"), None);
    }

    #[test]
    fn base64url_matches_rfc_vectors() {
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn expiry_is_strictly_past() {
        let live = Credential::OAuth {
            access: "a".into(),
            refresh: "r".into(),
            expires_at: crate::id::now_ms() + 10_000,
            account: None,
        };
        let dead = Credential::OAuth {
            access: "a".into(),
            refresh: "r".into(),
            expires_at: 1,
            account: None,
        };
        assert!(!live.is_expired());
        assert!(dead.is_expired());
        assert!(!Credential::ApiKey { key: "k".into() }.is_expired());
    }
}
