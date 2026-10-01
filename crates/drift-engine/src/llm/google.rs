//! Google Cloud access tokens as the client libraries find them: a service account key file or the
//! gcloud application-default credentials, exchanged for a short-lived token that is cached.

use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine as _;
use serde_json::{json, Value};

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
    let base = if cfg!(windows) { env("APPDATA").map(PathBuf::from) } else { env("HOME").map(|home| PathBuf::from(home).join(".config")) };
    base.map(|dir| dir.join("gcloud").join("application_default_credentials.json")).filter(|path| path.is_file())
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
        .ok_or_else(|| Error::api(400, "invalid_request_error", "no Google Cloud project: set GOOGLE_VERTEX_PROJECT"))?;
    let location = env("GOOGLE_VERTEX_LOCATION").or_else(|| env("GOOGLE_CLOUD_LOCATION")).unwrap_or_else(|| "global".into());
    Ok(Target { project, location })
}

static CACHE: Mutex<Option<(PathBuf, String, i64)>> = Mutex::new(None);

/// A current access token, minted again only near its expiry or when the credentials file changes.
pub async fn token(client: &reqwest::Client) -> Result<String, Error> {
    token_from(client, credentials_file().ok_or(Error::Unauthenticated)?).await
}

async fn token_from(client: &reqwest::Client, path: PathBuf) -> Result<String, Error> {
    let now = crate::id::now_ms() / 1000;
    if let Some((cached, token, expires)) = CACHE.lock().unwrap().clone() {
        if cached == path && now < expires - EARLY_SECONDS {
            return Ok(token);
        }
    }
    let file = read(&path)?;
    let form = match file["type"].as_str() {
        Some("service_account") => vec![("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer".to_string()), ("assertion", assertion(&file, now)?)],
        Some("authorized_user") => vec![
            ("grant_type", "refresh_token".to_string()),
            ("client_id", text(&file, "client_id")?),
            ("client_secret", text(&file, "client_secret")?),
            ("refresh_token", text(&file, "refresh_token")?),
        ],
        other => return Err(Error::Malformed(format!("unsupported Google credentials type {other:?}"))),
    };
    let url = file["token_uri"].as_str().unwrap_or(TOKEN_URL);
    let encoded = form.iter().map(|(key, value)| format!("{key}={}", percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC))).collect::<Vec<_>>().join("&");
    let sending = client.post(url).header("content-type", "application/x-www-form-urlencoded").body(encoded);
    let response = sending.send().await.map_err(|e| Error::Transport(e.to_string()))?;
    let status = response.status();
    let body: Value = response.json().await.map_err(|e| Error::Transport(e.to_string()))?;
    if !status.is_success() {
        return Err(Error::Unauthenticated);
    }
    let token = body["access_token"].as_str().ok_or(Error::Unauthenticated)?.to_string();
    let expires = now + body["expires_in"].as_i64().unwrap_or(3600);
    *CACHE.lock().unwrap() = Some((path, token.clone(), expires));
    Ok(token)
}

fn text(file: &Value, key: &str) -> Result<String, Error> {
    file[key].as_str().map(str::to_string).ok_or_else(|| Error::Malformed(format!("Google credentials have no {key}")))
}

/// The service account's signed claim that it may have a cloud-platform token for the next hour.
fn assertion(file: &Value, now: i64) -> Result<String, Error> {
    let encode = |value: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    let header = encode(&json!({ "alg": "RS256", "typ": "JWT" }));
    let claims = encode(&json!({ "iss": text(file, "client_email")?, "scope": SCOPE, "aud": file["token_uri"].as_str().unwrap_or(TOKEN_URL), "iat": now, "exp": now + 3600 }));
    let unsigned = format!("{header}.{claims}");
    let signature = sign_rs256(&text(file, "private_key")?, unsigned.as_bytes())?;
    Ok(format!("{unsigned}.{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)))
}

fn sign_rs256(pem: &str, message: &[u8]) -> Result<Vec<u8>, Error> {
    let der: String = pem.lines().filter(|line| !line.starts_with("-----")).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(der.trim()).map_err(|e| Error::Malformed(format!("service account key: {e}")))?;
    let key = ring::signature::RsaKeyPair::from_pkcs8(&der).map_err(|e| Error::Malformed(format!("service account key: {e}")))?;
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(&ring::signature::RSA_PKCS1_SHA256, &ring::rand::SystemRandom::new(), message, &mut signature).map_err(|_| Error::Malformed("could not sign the token request".into()))?;
    Ok(signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway 2048-bit key made for the test (node is already needed for the MCP fixtures), so none is committed.
    fn throwaway_key() -> String {
        let script = "const {generateKeyPairSync}=require('crypto');process.stdout.write(generateKeyPairSync('rsa',{modulusLength:2048,privateKeyEncoding:{type:'pkcs8',format:'pem'},publicKeyEncoding:{type:'spki',format:'pem'}}).privateKey)";
        let out = std::process::Command::new("node").args(["-e", script]).output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn a_service_account_signs_an_rs256_assertion() {
        let file = json!({ "type": "service_account", "client_email": "drift@example.iam.gserviceaccount.com", "private_key": throwaway_key(), "token_uri": TOKEN_URL });
        let jwt = assertion(&file, 1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!((claims["iss"].as_str(), claims["scope"].as_str(), claims["exp"].as_i64()), (Some("drift@example.iam.gserviceaccount.com"), Some(SCOPE), Some(1_700_003_600)));
        assert_eq!(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[2]).unwrap().len(), 256, "a 2048-bit signature");
    }

    #[tokio::test]
    async fn adc_user_credentials_are_exchanged_once_and_the_token_is_reused() {
        use axum::extract::State;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handler = |State(calls): State<std::sync::Arc<std::sync::atomic::AtomicUsize>>, body: String| async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(body.contains("grant_type=refresh%5Ftoken") || body.contains("grant_type=refresh_token"), "{body}");
            axum::Json(json!({ "access_token": "ya29.test", "expires_in": 3599 }))
        };
        let app = axum::Router::new().route("/token", axum::routing::post(handler)).with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let file = std::env::temp_dir().join(format!("drift-adc-{}.json", crate::random_hex(4)));
        std::fs::write(&file, json!({ "type": "authorized_user", "client_id": "id", "client_secret": "s", "refresh_token": "r", "token_uri": url }).to_string()).unwrap();
        let client = crate::llm::http::client();
        assert_eq!(token_from(&client, file.clone()).await.unwrap(), "ya29.test");
        assert_eq!(token_from(&client, file.clone()).await.unwrap(), "ya29.test");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "cached until near its expiry");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_bad_key_is_a_clear_error_not_a_panic() {
        let file = json!({ "type": "service_account", "client_email": "x", "private_key": "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n" });
        assert!(assertion(&file, 0).unwrap_err().to_string().contains("service account key"));
    }
}
