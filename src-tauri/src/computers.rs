//! This PC driving the Drift on another computer in its main window. The other computer is reached the way the Android
//! app reaches one (`drift-remote-client`): a proxy on 127.0.0.1 that pins it serves this build's UI and passes the UI's
//! requests to it with this PC's session. Switching to it points the window at the proxy; switching back returns the
//! window to the page it left, so this PC's own Drift picks up where it was. The proxy starts on first use only.

use std::borrow::Cow;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use drift_remote_client::computers::Computers;
use drift_remote_client::{Host, pc, proxy};
use serde::Serialize;
use tauri::{AppHandle, Manager, Url};

/// The main window's way to other computers: the proxy once started, and the page to come back to.
pub(crate) struct Linked {
    data: PathBuf,
    proxy: tokio::sync::OnceCell<proxy::Proxy>,
    home: Arc<Mutex<Option<Url>>>,
}

/// A linked computer as the switcher lists it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkedComputer {
    id: String,
    name: String,
    address: Option<String>,
    anywhere: bool,
}

/// The linked computers, and whether the sidebar shows them as tabs.
#[derive(Serialize)]
pub(crate) struct LinkedComputers {
    computers: Vec<LinkedComputer>,
    tabs: bool,
}

/// A computer that accepted this PC.
#[derive(Serialize)]
pub(crate) struct Accepted {
    id: String,
    name: String,
}

impl Linked {
    pub(crate) fn new(data: PathBuf) -> Self {
        Self {
            data,
            proxy: tokio::sync::OnceCell::new(),
            home: Arc::default(),
        }
    }

    async fn proxy(&self, app: &AppHandle) -> Result<&proxy::Proxy, String> {
        self.proxy
            .get_or_try_init(|| async {
                let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                    .await
                    .map_err(|error| error.to_string())?;
                let port = listener.local_addr().map_err(|error| error.to_string())?.port();
                let desktop = Arc::new(Desktop {
                    app: app.clone(),
                    home: self.home.clone(),
                });
                let computers = Computers::load(self.data.join("computers.json"));
                let shared = proxy::Shared::new(
                    computers,
                    desktop,
                    pc::hex(&secret()),
                    port,
                    self.data.join("computer-ui-storage.json"),
                );
                tauri::async_runtime::spawn(proxy::serve(listener, shared.clone()));

                Ok(shared)
            })
            .await
    }
}

/// This PC as the proxy's host: its own UI build, its name, and its Drift to go back to.
struct Desktop {
    app: AppHandle,
    home: Arc<Mutex<Option<Url>>>,
}

impl Host for Desktop {
    fn asset(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        self.app
            .asset_resolver()
            .get(path.to_string())
            .map(|asset| asset.bytes.into())
    }

    fn device_name(&self) -> String {
        crate::remote::computer_name()
    }

    fn broadcasts(&self) -> Vec<Ipv4Addr> {
        crate::remote::broadcasts()
    }

    fn this_computer(&self) -> Option<String> {
        Some(crate::remote::computer_name())
    }

    fn this_computer_id(&self) -> Option<String> {
        let access = self.app.state::<crate::remote::RemoteAccess>();
        Some(pc::hex(&access.authority()))
    }

    fn show_this_computer(&self) -> Result<(), String> {
        let home = self.home.lock().unwrap().clone();
        let home = home.ok_or("this window never left this PC")?;
        let window = self.app.get_webview_window("main").ok_or("the main window is closed")?;

        window.navigate(home).map_err(|error| error.to_string())
    }
}

fn secret() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system random source works");
    bytes
}

#[tauri::command]
pub(crate) async fn computers_list(
    app: AppHandle,
    linked: tauri::State<'_, Linked>,
) -> Result<LinkedComputers, String> {
    let proxy = linked.proxy(&app).await?;
    let computers = proxy.computers.lock().unwrap();
    let list = computers
        .list()
        .iter()
        .map(|computer| LinkedComputer {
            id: computer.id.clone(),
            name: computer.name.clone(),
            address: computer.addresses.first().map(ToString::to_string),
            anywhere: computer.tunnel.is_some(),
        })
        .collect();

    Ok(LinkedComputers {
        computers: list,
        tabs: computers.tabs(),
    })
}

/// Shows each computer as a tab at the top of the sidebar, or stops.
#[tauri::command]
pub(crate) async fn computers_set_tabs(
    app: AppHandle,
    linked: tauri::State<'_, Linked>,
    on: bool,
) -> Result<(), String> {
    let proxy = linked.proxy(&app).await?;
    let saved = proxy.computers.lock().unwrap().set_tabs(on);

    saved.map_err(|error| error.to_string())
}

/// The linked computers that answer now, by id.
#[tauri::command]
pub(crate) async fn computers_online(app: AppHandle, linked: tauri::State<'_, Linked>) -> Result<Vec<String>, String> {
    Ok(linked.proxy(&app).await?.online().await)
}

/// The computers with Remote Access on that answer on this PC's networks.
#[tauri::command]
pub(crate) async fn computers_nearby(
    app: AppHandle,
    linked: tauri::State<'_, Linked>,
) -> Result<Vec<proxy::Nearby>, String> {
    linked.proxy(&app).await?.nearby().await
}

/// Asks the computer at a found or typed address to accept this PC; the code both screens show while it decides.
#[tauri::command]
pub(crate) async fn computers_ask(
    app: AppHandle,
    linked: tauri::State<'_, Linked>,
    address: String,
) -> Result<proxy::Waiting, String> {
    linked.proxy(&app).await?.ask(&address).await
}

/// Waits for the computer asked to answer. Accepted, it is linked; the window stays on this PC until it is opened.
#[tauri::command]
pub(crate) async fn computers_answer(
    app: AppHandle,
    linked: tauri::State<'_, Linked>,
    id: String,
) -> Result<Accepted, String> {
    let proxy = linked.proxy(&app).await?;
    let (id, name) = proxy.answer(&id).await?;
    let _ = proxy.computers.lock().unwrap().set_active(None);

    Ok(Accepted { id, name })
}

/// Stops waiting for a computer's answer, so its user is no longer asked.
#[tauri::command]
pub(crate) async fn computers_cancel(
    app: AppHandle,
    linked: tauri::State<'_, Linked>,
    id: String,
) -> Result<(), String> {
    linked.proxy(&app).await?.cancel(&id).await;

    Ok(())
}

/// Reaches a linked computer, then shows its Drift in the main window in place of this PC's.
#[tauri::command]
pub(crate) async fn computers_open(app: AppHandle, linked: tauri::State<'_, Linked>, id: String) -> Result<(), String> {
    let proxy = linked.proxy(&app).await?;
    let computer = proxy.computers.lock().unwrap().get(&id).cloned();
    let computer = computer.ok_or("that computer is not linked")?;
    proxy.connect(&computer).await.map_err(|error| error.to_string())?;

    let window = app.get_webview_window("main").ok_or("the main window is closed")?;
    let here = window.url().map_err(|error| error.to_string())?;
    *linked.home.lock().unwrap() = Some(here);
    let entry = proxy
        .entry()
        .parse()
        .map_err(|error: url::ParseError| error.to_string())?;

    window.navigate(entry).map_err(|error| error.to_string())
}

/// Unlinks a computer, signing this PC out of it when it can be reached.
#[tauri::command]
pub(crate) async fn computers_forget(
    app: AppHandle,
    linked: tauri::State<'_, Linked>,
    id: String,
) -> Result<(), String> {
    linked.proxy(&app).await?.forget(&id);

    Ok(())
}
