//! Google Cloud access tokens from a service account key or gcloud's application-default credentials, cached.

use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine as _;
use serde_json::{Value, json};

use super::Error;

const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// A token is replaced this long before Google says it expires.
const EARLY_SECONDS: i64 = 300;

/// The credentials file in use: `GOOGLE_APPLICATION_CREDENTIALS`, else gcloud's application-default file.
fn credentials_file() -> Option<PathBuf> {
    if let Some(path) = env("GOOGLE_APPLICATION_CREDENTIALS") {
        return Some(PathBuf::from(path));
    }
    let base = if cfg!(windows) {
        env("APPDATA").map(PathBuf::from)
    } else {
        env("HOME").map(|home| PathBuf::from(home).join(".config"))
    };
    base.map(|dir| dir.join("gcloud").join("application_default_credentials.json"))
        .filter(|path| path.is_file())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.trim().is_empty())
}

/// Names the credentials file for the provider list; `None` when there is none.
pub fn detect() -> Option<String> {
    let path = credentials_file()?;
    let kind = read(&path).ok()?["type"].as_str()?.to_string();
    matches!(kind.as_str(), "service_account" | "authorized_user").then(|| format!("{kind} {}", path.display()))
}

fn read(path: &std::path::Path) -> Result<Value, Error> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::Malformed(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| Error::Malformed(format!("{}: {e}", path.display())))
}

/// Where Vertex requests go: the project and location, from the environment or the credentials file.
pub struct Target {
    pub project: String,
    pub location: String,
}

pub fn target() -> Result<Target, Error> {
    let file = credentials_file().and_then(|path| read(&path).ok()).unwrap_or_default();
    let project = env("GOOGLE_VERTEX_PROJECT")
        .or_else(|| env("GOOGLE_CLOUD_PROJECT"))
        .or_else(|| file["project_id"].as_str().map(str::to_string))
        .or_else(|| file["quota_project_id"].as_str().map(str::to_string))
        .ok_or_else(|| {
            Error::api(
                400,
                "invalid_request_error",
                "no Google Cloud project: set GOOGLE_VERTEX_PROJECT",
            )
        })?;
    let location = env("GOOGLE_VERTEX_LOCATION")
        .or_else(|| env("GOOGLE_CLOUD_LOCATION"))
        .unwrap_or_else(|| "global".into());
    Ok(Target { project, location })
}

/// A token, a hash of the credentials it came from, and when it expires; kept per credentials path.
struct Cached {
    contents: String,
    token: String,
    expires: i64,
}

static CACHE: Mutex<Option<std::collections::HashMap<PathBuf, Cached>>> = Mutex::new(None);
/// One exchange at a time: requests that need a token while one is being minted wait for it.
static MINTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A current access token, minted again only near its expiry or when the credentials (path or contents) change.
pub async fn token(client: &reqwest::Client, timeouts: &super::http::Timeouts) -> Result<String, Error> {
    token_from(
        client,
        credentials_file().ok_or(Error::Unauthenticated(String::new()))?,
        timeouts,
    )
    .await
}

async fn token_from(
    client: &reqwest::Client,
    path: PathBuf,
    timeouts: &super::http::Timeouts,
) -> Result<String, Error> {
    let text = std::fs::read_to_string(&path).map_err(|e| Error::Malformed(format!("{}: {e}", path.display())))?;
    let contents = hex_digest(&text);
    if let Some(token) = cached(&path, &contents) {
        return Ok(token);
    }
    let _minting = MINTING.lock().await;
    // Another request may have minted it while this one waited.
    if let Some(token) = cached(&path, &contents) {
        return Ok(token);
    }
    let file: Value = serde_json::from_str(&text).map_err(|e| Error::Malformed(format!("{}: {e}", path.display())))?;
    let (token, lifetime) = exchange(client, &file, timeouts).await?;
    let expires = crate::id::now_ms() / 1000 + lifetime;
    CACHE.lock().unwrap().get_or_insert_with(Default::default).insert(
        path,
        Cached {
            contents,
            token: token.clone(),
            expires,
        },
    );
    Ok(token)
}

/// The cached token if it came from these exact credentials and is not about to expire.
fn cached(path: &PathBuf, contents: &str) -> Option<String> {
    let now = crate::id::now_ms() / 1000;
    let cache = CACHE.lock().unwrap();
    cache
        .as_ref()?
        .get(path)
        .filter(|c| c.contents == contents && now < c.expires - EARLY_SECONDS)
        .map(|c| c.token.clone())
}

fn hex_digest(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Trades the credentials for a token and its lifetime in seconds, within the route's time limits.
async fn exchange(
    client: &reqwest::Client,
    file: &Value,
    timeouts: &super::http::Timeouts,
) -> Result<(String, i64), Error> {
    let now = crate::id::now_ms() / 1000;
    let form = match file["type"].as_str() {
        Some("service_account") => vec![
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer".to_string()),
            ("assertion", assertion(file, now)?),
        ],
        Some("authorized_user") => vec![
            ("grant_type", "refresh_token".to_string()),
            ("client_id", text(file, "client_id")?),
            ("client_secret", text(file, "client_secret")?),
            ("refresh_token", text(file, "refresh_token")?),
        ],
        other => {
            return Err(Error::Malformed(format!(
                "unsupported Google credentials type {other:?}"
            )));
        }
    };
    let url = file["token_uri"].as_str().unwrap_or(TOKEN_URL);
    let encoded = form
        .iter()
        .map(|(key, value)| {
            format!(
                "{key}={}",
                percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let sending = client
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(encoded);
    let response = super::http::send(sending, timeouts).await?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body: Value = serde_json::from_str(&super::http::bounded_body(response, timeouts).await).unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(token_error(status, &body).with_headers(&headers));
    }
    let token = body["access_token"]
        .as_str()
        .ok_or_else(|| Error::Malformed("the token response had no access_token".into()))?
        .to_string();
    Ok((token, body["expires_in"].as_i64().unwrap_or(3600)))
}

/// Credentials Google refuses are unauthenticated; its own trouble (rate limits, server faults) is worth retrying.
fn token_error(status: u16, body: &Value) -> Error {
    let kind = body["error"].as_str().unwrap_or("token_exchange_failed");
    let message = body["error_description"].as_str().unwrap_or(kind);
    let refused = [
        "invalid_grant",
        "invalid_client",
        "unauthorized_client",
        "invalid_scope",
    ]
    .contains(&kind);
    if refused || matches!(status, 401 | 403) {
        return Error::Unauthenticated(message.to_string());
    }
    Error::api(status, kind, format!("Google token exchange: {message}"))
}

fn text(file: &Value, key: &str) -> Result<String, Error> {
    file[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Malformed(format!("Google credentials have no {key}")))
}

/// The service account's signed claim that it may have a cloud-platform token for the next hour.
fn assertion(file: &Value, now: i64) -> Result<String, Error> {
    let encode = |value: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    let header = encode(&json!({ "alg": "RS256", "typ": "JWT" }));
    let claims = encode(
        &json!({ "iss": text(file, "client_email")?, "scope": SCOPE, "aud": file["token_uri"].as_str().unwrap_or(TOKEN_URL), "iat": now, "exp": now + 3600 }),
    );
    let unsigned = format!("{header}.{claims}");
    let signature = sign_rs256(&text(file, "private_key")?, unsigned.as_bytes())?;
    Ok(format!(
        "{unsigned}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
    ))
}

fn sign_rs256(pem: &str, message: &[u8]) -> Result<Vec<u8>, Error> {
    let der: String = pem.lines().filter(|line| !line.starts_with("-----")).collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(der.trim())
        .map_err(|e| Error::Malformed(format!("service account key: {e}")))?;
    let key = ring::signature::RsaKeyPair::from_pkcs8(&der)
        .map_err(|e| Error::Malformed(format!("service account key: {e}")))?;
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        message,
        &mut signature,
    )
    .map_err(|_| Error::Malformed("could not sign the token request".into()))?;
    Ok(signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway 2048-bit key made for the test (node is already needed for the MCP fixtures), so none is committed.
    fn throwaway_key() -> String {
        let script = "const {generateKeyPairSync}=require('crypto');process.stdout.write(generateKeyPairSync('rsa',{modulusLength:2048,privateKeyEncoding:{type:'pkcs8',format:'pem'},publicKeyEncoding:{type:'spki',format:'pem'}}).privateKey)";
        let out = std::process::Command::new("node")
            .args(["-e", script])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn a_service_account_signs_an_rs256_assertion() {
        let file = json!({ "type": "service_account", "client_email": "drift@example.iam.gserviceaccount.com", "private_key": throwaway_key(), "token_uri": TOKEN_URL });
        let jwt = assertion(&file, 1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[1])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            (claims["iss"].as_str(), claims["scope"].as_str(), claims["exp"].as_i64()),
            (
                Some("drift@example.iam.gserviceaccount.com"),
                Some(SCOPE),
                Some(1_700_003_600)
            )
        );
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[2])
                .unwrap()
                .len(),
            256,
            "a 2048-bit signature"
        );
    }

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::llm::http::Timeouts;

    /// A local token endpoint answering every exchange with `status` and `body` after `delay`; returns its URL and call count.
    async fn endpoint(
        status: u16,
        body: Value,
        headers: Vec<(&'static str, &'static str)>,
        delay: Duration,
    ) -> (String, Arc<AtomicUsize>) {
        use axum::extract::State;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = move |State(calls): State<Arc<AtomicUsize>>, form: String| {
            let (body, headers) = (body.clone(), headers.clone());
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                assert!(form.contains("grant_type="), "{form}");
                tokio::time::sleep(delay).await;
                let mut response = axum::response::IntoResponse::into_response((
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    axum::Json(body),
                ));
                for (name, value) in headers {
                    response.headers_mut().insert(name, value.parse().unwrap());
                }
                response
            }
        };
        let app = axum::Router::new()
            .route("/token", axum::routing::post(handler))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, calls)
    }

    fn user_file(url: &str, refresh: &str) -> PathBuf {
        let file = std::env::temp_dir().join(format!("drift-adc-{}.json", crate::random_hex(4)));
        write_user(&file, url, refresh);
        file
    }

    fn write_user(file: &PathBuf, url: &str, refresh: &str) {
        std::fs::write(file, json!({ "type": "authorized_user", "client_id": "id", "client_secret": "s", "refresh_token": refresh, "token_uri": url }).to_string()).unwrap();
    }

    #[tokio::test]
    async fn one_exchange_serves_concurrent_requests_until_the_credentials_change() {
        let (url, calls) = endpoint(
            200,
            json!({ "access_token": "ya29.test", "expires_in": 3599 }),
            vec![],
            Duration::from_millis(150),
        )
        .await;
        let file = user_file(&url, "r1");
        let client = crate::llm::http::client();
        let limits = Timeouts::default();
        let all = futures_util::future::join_all((0..5).map(|_| token_from(&client, file.clone(), &limits))).await;
        assert!(all.iter().all(|t| t.as_deref().ok() == Some("ya29.test")));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "concurrent requests wait for the one exchange"
        );
        token_from(&client, file.clone(), &Timeouts::default()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "cached until near its expiry");
        write_user(&file, &url, "r2");
        token_from(&client, file.clone(), &Timeouts::default()).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "new contents at the same path are new credentials"
        );
        let _ = std::fs::remove_file(file);
    }

    #[tokio::test]
    async fn refused_credentials_and_googles_own_trouble_are_told_apart() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = crate::llm::http::client();
        let (url, _) = endpoint(
            400,
            json!({ "error": "invalid_grant", "error_description": "Token has been expired or revoked." }),
            vec![],
            Duration::ZERO,
        )
        .await;
        assert!(matches!(
            token_from(&client, user_file(&url, "r"), &Timeouts::default()).await,
            Err(Error::Unauthenticated(_))
        ));
        let (url, _) = endpoint(
            503,
            json!({ "error": "backend_error" }),
            vec![("retry-after", "7")],
            Duration::ZERO,
        )
        .await;
        let busy = token_from(&client, user_file(&url, "r"), &Timeouts::default())
            .await
            .unwrap_err();
        assert!(
            matches!(busy, Error::Api { status: 503, retryable: true, retry_after: Some(wait), .. } if wait == Duration::from_secs(7)),
            "{busy:?}"
        );
        let (url, _) = endpoint(200, json!({ "access_token": "late" }), vec![], Duration::from_secs(5)).await;
        let quick = Timeouts {
            headers: Duration::from_millis(200),
            idle: Duration::from_secs(1),
        };
        assert!(
            matches!(
                token_from(&client, user_file(&url, "r"), &quick).await,
                Err(Error::Transport(_))
            ),
            "bounded by the route's time limit"
        );
    }

    #[test]
    fn a_bad_key_is_a_clear_error_not_a_panic() {
        let file = json!({ "type": "service_account", "client_email": "x", "private_key": "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n" });
        assert!(
            assertion(&file, 0)
                .unwrap_err()
                .to_string()
                .contains("service account key")
        );
    }
}
