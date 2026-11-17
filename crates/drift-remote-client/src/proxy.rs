//! This device's side of a PC: a server on 127.0.0.1 that the WebView treats as the PC's gateway. It serves the
//! bundled UI, answers the app's own computers page, and passes everything else to the chosen PC over TLS that
//! trusts only that PC, adding this device's session. Other programs on the device cannot use it: every request
//! needs the cookie only the WebView was given, with this launch's secret.

mod api;
mod forward;
mod storage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use axum::Router;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};

use crate::Host;
use crate::computers::{Computer, Computers};
use crate::pc::{self, Asking, Pc, PcError};

const COOKIE: &str = "drift_app";
/// How long a linked computer has to answer before it is shown as offline.
const ONLINE_CHECK: std::time::Duration = std::time::Duration::from_secs(4);

/// The PC requests go to now, and the session they carry.
#[derive(Clone)]
pub struct Route {
    pub id: String,
    pub pc: Pc,
    pub token: String,
}

/// A computer that answered nearby, as the computers page lists it.
#[derive(Clone, Debug, Serialize)]
pub struct Nearby {
    pub id: String,
    pub name: String,
    pub address: String,
    pub linked: bool,
}

/// A computer asked to accept this device, as the page shows it while its user decides.
#[derive(Clone, Debug, Serialize)]
pub struct Waiting {
    pub id: String,
    pub name: String,
    pub code: String,
}

pub struct Shared {
    pub computers: Mutex<Computers>,
    pub route: Mutex<Option<Route>>,
    pub host: Arc<dyn Host>,
    /// The computer asked to accept this device, by the id the page waits on; asking another replaces it.
    asking: Mutex<Option<(String, Asking)>>,
    /// Held while looking for the chosen PC again, so a burst of requests looks once.
    reaching: tokio::sync::Mutex<()>,
    secret: String,
    port: u16,
    /// The page's saved values (`storage`), and the last snapshot written, held while writing so writes go one at a time.
    storage: PathBuf,
    storing: Mutex<storage::Storing>,
}

pub type Proxy = Arc<Shared>;

/// Why a request could not be passed on.
pub(crate) enum Unrouted {
    NoneChosen,
    Unreachable,
}

impl Shared {
    pub fn new(computers: Computers, host: Arc<dyn Host>, secret: String, port: u16, storage: PathBuf) -> Proxy {
        Arc::new(Self {
            computers: Mutex::new(computers),
            route: Mutex::new(None),
            host,
            asking: Mutex::default(),
            reaching: tokio::sync::Mutex::new(()),
            secret,
            port,
            storage,
            storing: Mutex::default(),
        })
    }

    /// Where requests go now; after a dropped connection the chosen PC is looked for again, so an open page recovers.
    pub(crate) async fn destination(&self) -> Result<Route, Unrouted> {
        if let Some(route) = self.route.lock().unwrap().clone() {
            return Ok(route);
        }

        let _one_search = self.reaching.lock().await;
        if let Some(route) = self.route.lock().unwrap().clone() {
            return Ok(route);
        }
        let active = self.computers.lock().unwrap().active().cloned();
        let computer = active.ok_or(Unrouted::NoneChosen)?;
        // A PC reached but not saved still routes; only no route at all is unreachable.
        let _ = self.connect(&computer).await;

        self.route.lock().unwrap().clone().ok_or(Unrouted::Unreachable)
    }

    /// Reaches a linked PC, makes it the one requests go to, and remembers it for the next launch.
    pub async fn connect(&self, computer: &Computer) -> Result<(), PcError> {
        let (pc, reach) = pc::reach(computer).await?;
        let mut computers = self.computers.lock().unwrap();
        let refreshed = computers.refresh(&computer.id, &pc.endpoint, reach);
        let activated = computers.set_active(Some(&computer.id));
        drop(computers);

        *self.route.lock().unwrap() = Some(Route {
            id: computer.id.clone(),
            pc,
            token: computer.token.clone(),
        });
        refreshed.and(activated).map_err(PcError::Unsaved)
    }

    /// Saves a PC just linked and connects to it at the address it answered on.
    pub fn adopt(&self, computer: Computer, pc: Pc) -> Result<(), PcError> {
        let mut computers = self.computers.lock().unwrap();
        let saved = computers.upsert(computer.clone());
        let activated = computers.set_active(Some(&computer.id));
        drop(computers);

        *self.route.lock().unwrap() = Some(Route {
            id: computer.id,
            pc,
            token: computer.token,
        });
        saved.and(activated).map_err(PcError::Unsaved)
    }

    /// The computers with Remote Access on that answer on this device's networks, this device's own left out.
    pub async fn nearby(&self) -> Result<Vec<Nearby>, String> {
        let found = crate::nearby::discover(self.host.broadcasts(), drift_pairing::PORT)
            .await
            .map_err(|error| error.to_string())?;
        let own = self.host.this_computer_id();
        let computers = self.computers.lock().unwrap();

        Ok(found
            .into_iter()
            .filter(|found| own.as_deref() != Some(found.announcement.id.as_str()))
            .map(|found| Nearby {
                linked: computers.get(&found.announcement.id).is_some(),
                id: found.announcement.id,
                name: found.announcement.name,
                address: found.address.to_string(),
            })
            .collect())
    }

    /// The linked computers that answer within a few seconds, by id.
    pub async fn online(&self) -> Vec<String> {
        let computers = self.computers.lock().unwrap().list().to_vec();
        let checks = computers.into_iter().map(|computer| async move {
            let answered = tokio::time::timeout(ONLINE_CHECK, pc::reach(&computer)).await;
            matches!(answered, Ok(Ok(_))).then_some(computer.id)
        });

        futures_util::future::join_all(checks)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Asks the computer at a found or typed address to accept this device; the code to show while it decides.
    pub async fn ask(&self, address: &str) -> Result<Waiting, String> {
        let address = pc::resolve(address).await.map_err(|error| error.to_string())?;
        let device = self.device_name().await;
        let own = self.host.this_computer_id();
        let asking = pc::ask(address, &device, own.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        let waiting = Waiting {
            id: random_id(),
            name: asking.name.clone(),
            code: asking.code.clone(),
        };

        let replaced = self.asking.lock().unwrap().replace((waiting.id.clone(), asking));
        if let Some((_, replaced)) = replaced {
            tokio::spawn(async move { pc::cancel(&replaced).await });
        }
        Ok(waiting)
    }

    /// Waits for the computer asked to answer. Accepted, it is linked and requests go to it; its id and name.
    pub async fn answer(&self, id: &str) -> Result<(String, String), String> {
        let asking = self.asking(id)?;
        let answered = pc::answer(&asking).await;
        let still_asked = self
            .asking
            .lock()
            .unwrap()
            .take_if(|(current, _)| current == id)
            .is_some();
        if !still_asked {
            return Err("cancelled".into());
        }

        let (computer, pc) = answered.map_err(|error| error.to_string())?;
        let linked = (computer.id.clone(), computer.name.clone());
        self.adopt(computer, pc).map_err(|error| error.to_string())?;
        Ok(linked)
    }

    /// Stops waiting, and tells the computer so its user is no longer asked.
    pub async fn cancel(&self, id: &str) {
        let taken = self.asking.lock().unwrap().take_if(|(current, _)| current == id);
        if let Some((_, asking)) = taken {
            pc::cancel(&asking).await;
        }
    }

    fn asking(&self, id: &str) -> Result<Asking, String> {
        let current = self.asking.lock().unwrap();
        match current.as_ref() {
            Some((current, asking)) if current == id => Ok(asking.clone()),
            _ => Err("that request is no longer waiting; connect again".into()),
        }
    }

    /// Removes a PC from this device, signing it out of the PC in the background when it can be reached.
    pub fn forget(&self, id: &str) {
        let computer = self.computers.lock().unwrap().get(id).cloned();
        let Some(computer) = computer else {
            return;
        };

        let mut route = self.route.lock().unwrap();
        if route.as_ref().is_some_and(|route| route.id == computer.id) {
            *route = None;
        }
        drop(route);
        let _ = self.computers.lock().unwrap().remove(&computer.id);

        tokio::spawn(async move {
            if let Ok((pc, _)) = pc::reach(&computer).await {
                pc.sign_out(&computer.token).await;
            }
        });
    }

    /// Leaves the PC requests went to for this device's own Drift, which the next launch also opens on.
    pub fn leave(&self) -> Result<(), String> {
        self.host.show_this_computer()?;
        let _ = self.computers.lock().unwrap().set_active(None);
        Ok(())
    }

    /// How the PC lists this device, read off the async threads.
    pub async fn device_name(&self) -> String {
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || host.device_name())
            .await
            .unwrap_or_else(|_| "Drift".into())
    }

    /// The address the WebView opens to enter with this launch's secret.
    pub fn entry(&self) -> String {
        format!("http://127.0.0.1:{}/app/enter?secret={}", self.port, self.secret)
    }
}

pub fn router(proxy: Proxy) -> Router {
    Router::new()
        .route("/app/enter", get(enter))
        .route("/app", get(main_page))
        .route("/app/computers", get(computers_page))
        .nest("/app/api", api::routes().merge(storage::routes()))
        .route("/auth/me", any(forward::forward))
        .route("/auth/logout", any(forward::forward))
        .route("/engine/{*rest}", any(forward::forward))
        .route("/api/{*rest}", any(forward::forward))
        .fallback(asset)
        .layer(middleware::from_fn_with_state(proxy.clone(), gate))
        .with_state(proxy)
}

pub async fn serve(listener: tokio::net::TcpListener, proxy: Proxy) {
    let _ = axum::serve(listener, router(proxy)).await;
}

/// Only this launch's WebView: the right host (no DNS rebinding) and the cookie `/app/enter` gave it.
async fn gate(State(proxy): State<Proxy>, request: Request, next: Next) -> Response {
    let expected = format!("127.0.0.1:{}", proxy.port);
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if host != Some(expected.as_str()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.uri().path() == "/app/enter" || holds_secret(request.headers(), &proxy.secret) {
        return next.run(request).await;
    }

    StatusCode::UNAUTHORIZED.into_response()
}

fn holds_secret(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .any(|(name, value)| name == COOKIE && same(value.as_bytes(), secret.as_bytes()))
}

fn random_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system random source works");
    pc::hex(&bytes)
}

/// Compares in time that does not depend on where the inputs differ.
fn same(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// The WebView's first request: trades the launch secret in the URL for the cookie every later request carries.
async fn enter(State(proxy): State<Proxy>, Query(query): Query<HashMap<String, String>>) -> Response {
    let supplied = query.get("secret").map(String::as_bytes).unwrap_or_default();
    if !same(supplied, proxy.secret.as_bytes()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let cookie = format!("{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/", proxy.secret);
    let mut response = Redirect::to("/app").into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }

    response
}

/// Drift itself, connected to the chosen PC; the computers page when there is none or it cannot be reached.
async fn main_page(State(proxy): State<Proxy>) -> Response {
    let active = proxy.computers.lock().unwrap().active().cloned();
    let Some(computer) = active else {
        return Redirect::to("/app/computers").into_response();
    };

    let routed = proxy
        .route
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|route| route.id == computer.id);
    if !routed && let Err(problem) = proxy.connect(&computer).await {
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("problem", &problem.to_string())
            .finish();
        return Redirect::to(&format!("/app/computers?{query}")).into_response();
    }

    let Some(page) = proxy.host.asset("index.html") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let page = storage::with_restore(&String::from_utf8_lossy(&page));
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        page,
    )
        .into_response()
}

async fn computers_page(State(proxy): State<Proxy>) -> Response {
    file(&proxy, "app.html")
}

async fn asset(State(proxy): State<Proxy>, request: Request) -> Response {
    file(&proxy, request.uri().path().trim_start_matches('/'))
}

fn file(proxy: &Proxy, path: &str) -> Response {
    let Some(found) = proxy.host.asset(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let kind = mime_guess::from_path(path).first_or_octet_stream();
    // Hashed bundle files never change; the pages must, so a new build is picked up.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };

    (
        [(header::CONTENT_TYPE, kind.as_ref()), (header::CACHE_CONTROL, cache)],
        found.into_owned(),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
