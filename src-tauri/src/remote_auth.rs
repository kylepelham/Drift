use crate::remote::{constant_time_eq, RemoteAccess};
use crate::store::{RemoteDevice, Store};
use axum::extract::{ConnectInfo, Extension, Path as UrlPath, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

pub(crate) const COOKIE_NAME: &str = "drift_remote";
pub(crate) const SIGN_IN_PAGE: &str = include_str!("remote_sign_in.html");
const COOKIE_MAX_AGE_SECS: u64 = 400 * 24 * 60 * 60;
const LINK_TTL: Duration = Duration::from_secs(600);
const MAX_LINKS: usize = 32;
const MAX_LINKS_PER_ADDRESS: usize = 3;
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LENGTH: usize = 8;
const PASSWORD_KEY: &str = "remote_password";
const PASSWORD_ITERATIONS: u32 = 600_000;
const FREE_FAILURES: u32 = 5;
const MAX_LOCK: Duration = Duration::from_secs(900);
const TOUCH_INTERVAL_MS: i64 = 60_000;

#[derive(Clone, Deserialize, Serialize)]
struct Password {
    username: String,
    hash: String,
}

struct Link {
    handle: String,
    code: String,
    name: String,
    address: IpAddr,
    requested_at: i64,
    expires: Instant,
    approved: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingLink {
    name: String,
    address: String,
    requested_at: i64,
}

struct Failure {
    count: u32,
    locked_until: Option<Instant>,
}

pub(crate) enum Poll {
    Pending,
    Approved(String),
    Expired,
}

/// Linked devices, in-flight device codes, the optional password, and sign-in throttling.
pub(crate) struct Auth {
    devices: HashMap<String, RemoteDevice>,
    links: Vec<Link>,
    password: Option<Password>,
    failures: HashMap<IpAddr, Failure>,
}

impl Auth {
    pub(crate) fn load(store: &Store) -> Result<Self, String> {
        let devices = store.remote_devices().map_err(|error| error.to_string())?;
        let password = store
            .app_setting(PASSWORD_KEY)
            .map_err(|error| error.to_string())?
            .and_then(|value| serde_json::from_str(&value).ok());
        Ok(Self {
            devices: devices.into_iter().map(|device| (device.token_hash.clone(), device)).collect(),
            links: Vec::new(),
            password,
            failures: HashMap::new(),
        })
    }

    /// Resolves a session token and records activity at most once a minute.
    pub(crate) fn device(&mut self, token: &str, store: &Store) -> Option<RemoteDevice> {
        let device = self.devices.get_mut(&token_hash(token))?;
        let now = now_ms();
        if now - device.last_seen_at >= TOUCH_INTERVAL_MS {
            device.last_seen_at = now;
            let _ = store.touch_remote_device(&device.id, now);
        }
        Some(device.clone())
    }

    pub(crate) fn devices(&self) -> Vec<RemoteDevice> {
        let mut devices: Vec<_> = self.devices.values().cloned().collect();
        devices.sort_by_key(|device| device.created_at);
        devices
    }

    pub(crate) fn pending(&mut self) -> Vec<PendingLink> {
        self.prune(Instant::now());
        self.links
            .iter()
            .filter(|link| !link.approved)
            .map(|link| PendingLink {
                name: link.name.clone(),
                address: link.address.to_string(),
                requested_at: link.requested_at,
            })
            .collect()
    }

    pub(crate) fn password_username(&self) -> Option<String> {
        self.password.as_ref().map(|password| password.username.clone())
    }

    fn request_link(&mut self, address: IpAddr, name: String) -> Result<(String, String), String> {
        self.prune(Instant::now());
        let from_address = self.links.iter().filter(|link| link.address == address).count();
        if self.links.len() >= MAX_LINKS || from_address >= MAX_LINKS_PER_ADDRESS {
            return Err("Too many devices are waiting to link. Try again in a few minutes.".into());
        }
        let code = loop {
            let code = random_code();
            if !self.links.iter().any(|link| link.code == code) {
                break code;
            }
        };
        let handle = random_hex(32);
        self.links.push(Link {
            handle: handle.clone(),
            code: code.clone(),
            name,
            address,
            requested_at: now_ms(),
            expires: Instant::now() + LINK_TTL,
            approved: false,
        });
        Ok((handle, code))
    }

    /// Marks the link waiting with `code` approved; the device finishes signing in on its next poll.
    pub(crate) fn approve(&mut self, code: &str) -> Result<String, String> {
        self.prune(Instant::now());
        let code = normalize_code(code);
        let link = self
            .links
            .iter_mut()
            .find(|link| !link.approved && link.code == code)
            .ok_or("No device is waiting with that code. Codes expire after 10 minutes.")?;
        link.approved = true;
        Ok(link.name.clone())
    }

    fn poll(&mut self, handle: &str, store: &Store) -> Result<Poll, String> {
        self.prune(Instant::now());
        let Some(index) = self.links.iter().position(|link| link.handle == handle) else {
            return Ok(Poll::Expired);
        };
        if !self.links[index].approved {
            return Ok(Poll::Pending);
        }
        let link = self.links.remove(index);
        self.create_device(link.name, "link", store).map(Poll::Approved)
    }

    fn create_device(&mut self, name: String, method: &str, store: &Store) -> Result<String, String> {
        let token = random_hex(32);
        let now = now_ms();
        let device = RemoteDevice {
            id: random_hex(12),
            name,
            token_hash: token_hash(&token),
            method: method.into(),
            created_at: now,
            last_seen_at: now,
        };
        store.insert_remote_device(&device).map_err(|error| error.to_string())?;
        self.devices.insert(device.token_hash.clone(), device);
        Ok(token)
    }

    /// Signs out one device, or every device when `id` is `None`.
    pub(crate) fn revoke(&mut self, id: Option<&str>, store: &Store) -> Result<(), String> {
        store.delete_remote_devices(id, None).map_err(|error| error.to_string())?;
        self.devices.retain(|_, device| id.is_some_and(|id| device.id != id));
        Ok(())
    }

    /// Replaces or clears the password; sessions created with the old one are signed out.
    pub(crate) fn set_password(&mut self, password: Option<(String, String)>, store: &Store) -> Result<(), String> {
        let password = password.map(|(username, hash)| Password { username, hash });
        match &password {
            Some(password) => {
                let encoded = serde_json::to_string(password).map_err(|error| error.to_string())?;
                store.save_app_setting(PASSWORD_KEY, &encoded)
            }
            None => store.delete_app_setting(PASSWORD_KEY),
        }
        .map_err(|error| error.to_string())?;
        store
            .delete_remote_devices(None, Some("password"))
            .map_err(|error| error.to_string())?;
        self.devices.retain(|_, device| device.method != "password");
        self.password = password;
        Ok(())
    }

    fn locked_for(&mut self, address: IpAddr, now: Instant) -> Option<Duration> {
        let until = self.failures.get(&address)?.locked_until?;
        (until > now).then(|| until - now)
    }

    fn fail(&mut self, address: IpAddr, now: Instant) {
        let failure = self.failures.entry(address).or_insert(Failure { count: 0, locked_until: None });
        failure.count += 1;
        if failure.count >= FREE_FAILURES {
            let doublings = (failure.count - FREE_FAILURES).min(5);
            failure.locked_until = Some(now + (Duration::from_secs(30) * 2u32.pow(doublings)).min(MAX_LOCK));
        }
    }

    fn prune(&mut self, now: Instant) {
        self.links.retain(|link| link.expires > now);
    }
}

pub(crate) fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("failed to read the system random source");
    bytes
}

fn random_hex(len: usize) -> String {
    (0..len).map(|_| format!("{:02x}", random_bytes::<1>()[0])).collect()
}

/// Draws unbiased characters from an alphabet without look-alikes (no 0/O, 1/I/L).
fn random_code() -> String {
    let limit = (256 / CODE_ALPHABET.len() * CODE_ALPHABET.len()) as u8;
    let mut code = String::with_capacity(CODE_LENGTH);
    while code.len() < CODE_LENGTH {
        let byte = random_bytes::<1>()[0];
        if byte < limit {
            code.push(CODE_ALPHABET[byte as usize % CODE_ALPHABET.len()] as char);
        }
    }
    code
}

pub(crate) fn normalize_code(input: &str) -> String {
    input.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect()
}

fn display_code(code: &str) -> String {
    format!("{}-{}", &code[..CODE_LENGTH / 2], &code[CODE_LENGTH / 2..])
}

fn device_name(raw: Option<String>) -> String {
    let clean: String = raw.unwrap_or_default().chars().filter(|c| !c.is_control()).collect();
    let name: String = clean.trim().chars().take(60).collect();
    if name.is_empty() { "Browser".into() } else { name }
}

pub(crate) fn hash_password(password: &str, salt: &[u8], iterations: u32) -> String {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, iterations, &mut key);
    format!(
        "pbkdf2-sha256${iterations}${}${}",
        STANDARD_NO_PAD.encode(salt),
        STANDARD_NO_PAD.encode(key)
    )
}

pub(crate) fn new_password_hash(password: &str) -> String {
    hash_password(password, &random_bytes::<16>(), PASSWORD_ITERATIONS)
}

pub(crate) fn verify_password(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    let [scheme, iterations, salt, _] = parts.as_slice() else {
        return false;
    };
    let (Ok(iterations), Ok(salt)) = (iterations.parse::<u32>(), STANDARD_NO_PAD.decode(salt)) else {
        return false;
    };
    *scheme == "pbkdf2-sha256"
        && iterations > 0
        && constant_time_eq(hash_password(password, &salt, iterations).as_bytes(), stored.as_bytes())
}

pub(crate) fn validate_credentials(username: &str, password: &str) -> Result<(), String> {
    if username.trim().is_empty() || username.trim().chars().count() > 64 {
        return Err("Username must be 1 to 64 characters.".into());
    }
    if !(8..=256).contains(&password.chars().count()) {
        return Err("Password must be 8 to 256 characters.".into());
    }
    Ok(())
}

pub(crate) fn supplied_token(headers: &HeaderMap) -> Option<&str> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    bearer.or_else(|| {
        headers
            .get(header::COOKIE)?
            .to_str()
            .ok()?
            .split(';')
            .map(str::trim)
            .find_map(|value| value.strip_prefix(&format!("{COOKIE_NAME}=")))
    })
}

fn session_cookie(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={token}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}"
    ))
    .expect("session tokens are hex")
}

fn with_cookie(mut response: Response, cookie: HeaderValue) -> Response {
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    response
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// Routes under `/auth/` that a signed-out browser may call.
pub(crate) fn public_path(path: &str) -> bool {
    path.starts_with("/auth/") && !matches!(path, "/auth/logout" | "/auth/me")
}

/// Browser navigations that show the sign-in page instead of a bare 401.
pub(crate) fn sign_in_path(path: &str) -> bool {
    path == "/" || path == "/companion" || path.starts_with("/companion/")
}

pub(crate) fn sign_in_page() -> Response {
    Html(SIGN_IN_PAGE).into_response()
}

fn notify(app: &tauri::AppHandle) {
    let _ = app.emit("remote-access-changed", ());
}

/// The public CA certificate; installing it on a device removes the browser warning.
pub(crate) async fn certificate(State(app): State<tauri::AppHandle>) -> Response {
    let der = app.state::<RemoteAccess>().certificate();
    let mut response = der.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-x509-ca-cert"));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"drift-remote-access.cer\""),
    );
    response
}

pub(crate) async fn options(State(app): State<tauri::AppHandle>) -> Response {
    let password = app.state::<RemoteAccess>().auth().password.is_some();
    Json(json!({ "password": password })).into_response()
}

#[derive(Deserialize)]
pub(crate) struct LinkRequest {
    name: Option<String>,
}

pub(crate) async fn start_link(
    State(app): State<tauri::AppHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<LinkRequest>,
) -> Response {
    let started = app.state::<RemoteAccess>().auth().request_link(peer.ip(), device_name(request.name));
    match started {
        Ok((id, code)) => {
            notify(&app);
            Json(json!({ "id": id, "code": display_code(&code), "expiresIn": LINK_TTL.as_secs() })).into_response()
        }
        Err(error) => failure(StatusCode::TOO_MANY_REQUESTS, &error),
    }
}

pub(crate) async fn poll_link(State(app): State<tauri::AppHandle>, UrlPath(id): UrlPath<String>) -> Response {
    let polled = app.state::<RemoteAccess>().auth().poll(&id, &app.state::<Store>());
    match polled {
        Ok(Poll::Pending) => Json(json!({ "status": "pending" })).into_response(),
        Ok(Poll::Expired) => (StatusCode::NOT_FOUND, Json(json!({ "status": "expired" }))).into_response(),
        Ok(Poll::Approved(token)) => {
            notify(&app);
            with_cookie(Json(json!({ "status": "approved" })).into_response(), session_cookie(&token))
        }
        Err(error) => failure(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    username: String,
    password: String,
    name: Option<String>,
}

pub(crate) async fn login(
    State(app): State<tauri::AppHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<LoginRequest>,
) -> Response {
    let access = app.state::<RemoteAccess>();
    let address = peer.ip();
    let (locked, password) = {
        let mut auth = access.auth();
        (auth.locked_for(address, Instant::now()), auth.password.clone())
    };
    if let Some(wait) = locked {
        let message = format!("Too many attempts. Try again in {} seconds.", wait.as_secs().max(1));
        return failure(StatusCode::TOO_MANY_REQUESTS, &message);
    }
    let Some(password) = password else {
        return failure(StatusCode::NOT_FOUND, "Password sign-in is turned off.");
    };
    let _permit = access.password_checks().acquire().await;
    let supplied = request.password;
    let hash = password.hash.clone();
    let matches = tokio::task::spawn_blocking(move || verify_password(&supplied, &hash))
        .await
        .unwrap_or(false);
    let valid = matches && constant_time_eq(request.username.trim().as_bytes(), password.username.as_bytes());
    let mut auth = access.auth();
    if !valid {
        auth.fail(address, Instant::now());
        return failure(StatusCode::UNAUTHORIZED, "Wrong username or password.");
    }
    auth.failures.remove(&address);
    match auth.create_device(device_name(request.name), "password", &app.state::<Store>()) {
        Ok(token) => {
            drop(auth);
            notify(&app);
            with_cookie(StatusCode::NO_CONTENT.into_response(), session_cookie(&token))
        }
        Err(error) => failure(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

pub(crate) async fn me(Extension(device): Extension<RemoteDevice>) -> Response {
    Json(device).into_response()
}

pub(crate) async fn logout(State(app): State<tauri::AppHandle>, Extension(device): Extension<RemoteDevice>) -> Response {
    let access = app.state::<RemoteAccess>();
    if let Err(error) = access.auth().revoke(Some(&device.id), &app.state::<Store>()) {
        return failure(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    access.invalidate_streams();
    notify(&app);
    let cleared = HeaderValue::from_static("drift_remote=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0");
    with_cookie(StatusCode::NO_CONTENT.into_response(), cleared)
}

#[cfg(test)]
#[path = "remote_auth_tests.rs"]
mod tests;
