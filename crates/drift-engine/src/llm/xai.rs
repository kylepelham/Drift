//! SuperGrok sign-in as xAI's Grok CLI does it: a device code the user enters in any browser
//! (RFC 8628), then tokens by form post. Requests use the access token as a Bearer key.

use std::time::Duration;

use serde_json::Value;

use crate::llm::openai::oauth::encode;
use crate::llm::{Credential, OAuthError};

/// xAI's public Grok CLI client.
pub const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const DEVICE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// xAI does not always say how long a token lives; an hour is what it gives when it does.
const DEFAULT_EXPIRES_IN: i64 = 3600;
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);
const MIN_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(not(test))]
const SLOW_DOWN: Duration = Duration::from_secs(5);
#[cfg(test)]
const SLOW_DOWN: Duration = Duration::from_millis(50);
const DEFAULT_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// A device code waiting for the user, with what to show them.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Device {
    pub device_code: String,
    /// What the user types at `verification_uri`.
    pub user_code: String,
    pub verification_uri: String,
    /// The page with the code already filled in, when xAI gives one.
    pub url: String,
    pub interval_ms: u64,
    pub lifetime_ms: u64,
}

pub async fn start(client: &reqwest::Client) -> Result<Device, OAuthError> {
    start_at(client, DEVICE_URL).await
}

async fn start_at(client: &reqwest::Client, url: &str) -> Result<Device, OAuthError> {
    let (status, json) = post(
        client,
        url,
        &[("client_id", CLIENT_ID), ("scope", SCOPE), ("referrer", "drift")],
    )
    .await?;
    if !status.is_success() {
        return Err(OAuthError::DeviceStart(status));
    }
    let field = |key: &str| {
        json[key]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| OAuthError::MissingDeviceField(key.to_owned()))
    };
    let verification_uri = field("verification_uri")?;
    let seconds = |key: &str, default: Duration| {
        json[key]
            .as_u64()
            .filter(|n| *n > 0)
            .map_or(default, Duration::from_secs)
    };
    Ok(Device {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        url: json["verification_uri_complete"]
            .as_str()
            .map_or_else(|| verification_uri.clone(), str::to_string),
        verification_uri,
        interval_ms: seconds("interval", DEFAULT_INTERVAL).max(MIN_INTERVAL).as_millis() as u64,
        lifetime_ms: seconds("expires_in", DEFAULT_LIFETIME).as_millis() as u64,
    })
}

/// Waits for the user to approve the code in their browser, polling as xAI asks.
pub async fn wait(client: &reqwest::Client, device: &Device) -> Result<Credential, OAuthError> {
    wait_at(client, TOKEN_URL, device).await
}

async fn wait_at(client: &reqwest::Client, url: &str, device: &Device) -> Result<Credential, OAuthError> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(device.lifetime_ms);
    let mut interval = Duration::from_millis(device.interval_ms);
    while tokio::time::Instant::now() < deadline {
        let (status, json) = post(
            client,
            url,
            &[
                ("grant_type", DEVICE_GRANT),
                ("client_id", CLIENT_ID),
                ("device_code", &device.device_code),
            ],
        )
        .await?;
        if status.is_success() {
            return credential(&json, None);
        }
        match json["error"].as_str() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += SLOW_DOWN,
            Some("access_denied" | "authorization_denied") => return Err(OAuthError::Declined),
            Some("expired_token") => return Err(OAuthError::Expired),
            other => {
                return Err(OAuthError::DeviceRefused {
                    status,
                    reason: other.unwrap_or("no reason given").to_owned(),
                });
            }
        }
        tokio::time::sleep(interval.min(deadline.saturating_duration_since(tokio::time::Instant::now()))).await;
    }
    Err(OAuthError::ExpiredBeforeApproval)
}

pub async fn refresh(client: &reqwest::Client, refresh_token: &str) -> Result<Credential, OAuthError> {
    refresh_at(client, TOKEN_URL, refresh_token).await
}

async fn refresh_at(client: &reqwest::Client, url: &str, refresh_token: &str) -> Result<Credential, OAuthError> {
    let (status, json) = post(
        client,
        url,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ],
    )
    .await?;
    if !status.is_success() {
        return Err(OAuthError::DeviceRefresh(status));
    }
    credential(&json, Some(refresh_token))
}

/// The tokens xAI returned; a refresh that hands back no new refresh token keeps the one it used.
fn credential(json: &Value, previous_refresh: Option<&str>) -> Result<Credential, OAuthError> {
    let access = json["access_token"]
        .as_str()
        .ok_or_else(|| OAuthError::MissingDeviceField("access_token".into()))?
        .to_string();
    let refresh = json["refresh_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .or(previous_refresh)
        .ok_or_else(|| OAuthError::MissingDeviceField("refresh_token".into()))?
        .to_string();
    let expires_in = json["expires_in"].as_i64().unwrap_or(DEFAULT_EXPIRES_IN);
    Ok(Credential::OAuth {
        access,
        refresh,
        expires_at: crate::id::now_ms() + expires_in * 1000,
        account: None,
    })
}

async fn post(
    client: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<(reqwest::StatusCode, Value), OAuthError> {
    let body: Vec<String> = form
        .iter()
        .map(|(key, value)| format!("{key}={}", encode(value)))
        .collect();
    let timeouts = crate::llm::http::Timeouts::default();
    let request = client
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(body.join("&"));
    let response = crate::llm::http::send(request, &timeouts).await?;
    let status = response.status();
    let text = crate::llm::http::bounded_body(response, &timeouts).await;
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A token endpoint answering each request with the next scripted reply, recording the bodies it got.
    async fn endpoint(replies: Vec<(u16, Value)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let bodies = seen.clone();
        tokio::spawn(async move {
            for (status, body) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0u8; 8192];
                let read = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                bodies
                    .lock()
                    .unwrap()
                    .push(request.split("\r\n\r\n").nth(1).unwrap_or_default().to_string());
                let text = body.to_string();
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        (url, seen)
    }

    fn device() -> Device {
        Device {
            device_code: "dev".into(),
            user_code: "WXYZ-9876".into(),
            verification_uri: "https://accounts.x.ai/device".into(),
            url: "https://accounts.x.ai/device".into(),
            interval_ms: 10,
            lifetime_ms: 5_000,
        }
    }

    #[tokio::test]
    async fn a_device_code_is_started_and_shows_the_code_and_the_page_to_open() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let answer = serde_json::json!({ "device_code": "dev", "user_code": "WXYZ-9876", "verification_uri": "https://accounts.x.ai/device", "verification_uri_complete": "https://accounts.x.ai/device?code=WXYZ-9876", "interval": 0, "expires_in": 600 });
        let (url, seen) = endpoint(vec![(200, answer)]).await;
        let started = start_at(&reqwest::Client::new(), &url).await.unwrap();
        assert_eq!(
            (
                started.user_code.as_str(),
                started.url.as_str(),
                started.interval_ms,
                started.lifetime_ms
            ),
            (
                "WXYZ-9876",
                "https://accounts.x.ai/device?code=WXYZ-9876",
                5_000,
                600_000
            )
        );
        assert!(
            seen.lock().unwrap()[0].contains(&format!("client_id={CLIENT_ID}"))
                && seen.lock().unwrap()[0].contains("offline_access")
        );
    }

    #[tokio::test]
    async fn polling_waits_out_pending_and_slow_down_then_takes_the_tokens() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (url, seen) = endpoint(vec![
            (400, serde_json::json!({ "error": "authorization_pending" })),
            (400, serde_json::json!({ "error": "slow_down" })),
            (
                200,
                serde_json::json!({ "access_token": "a", "refresh_token": "r", "expires_in": 7200 }),
            ),
        ])
        .await;
        let mut quick = device();
        quick.interval_ms = 1;
        let started = tokio::time::Instant::now();
        let Credential::OAuth {
            access,
            refresh,
            expires_at,
            ..
        } = wait_at(&reqwest::Client::new(), &url, &quick).await.unwrap()
        else {
            panic!()
        };
        assert_eq!((access.as_str(), refresh.as_str()), ("a", "r"));
        assert!(expires_at > crate::id::now_ms() + 7_000_000);
        assert!(started.elapsed() >= SLOW_DOWN, "slow_down lengthens the wait");
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_declined_code_and_a_refresh_without_a_new_token_say_so_or_keep_the_old_one() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (url, _) = endpoint(vec![(400, serde_json::json!({ "error": "access_denied" }))]).await;
        assert_eq!(
            wait_at(&reqwest::Client::new(), &url, &device())
                .await
                .unwrap_err()
                .to_string(),
            "the sign-in was declined"
        );
        let (url, seen) = endpoint(vec![(200, serde_json::json!({ "access_token": "fresh" }))]).await;
        let Credential::OAuth { access, refresh, .. } =
            refresh_at(&reqwest::Client::new(), &url, "kept").await.unwrap()
        else {
            panic!()
        };
        assert_eq!((access.as_str(), refresh.as_str()), ("fresh", "kept"));
        assert!(seen.lock().unwrap()[0].contains("grant_type=refresh_token"));
    }
}
