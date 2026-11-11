//! Drift for Android: drives the Drift on your PCs, on their networks or from anywhere through OpenTunnel. The
//! WebView talks to a proxy on 127.0.0.1 that pins each PC, so no certificate warning can ever appear
//! (`docs/android.md`). The proxy and everything it knows of PCs is `drift-remote-client`, shared with the desktop.

mod native;

use std::borrow::Cow;
use std::net::Ipv4Addr;
use std::sync::Arc;

use drift_remote_client::computers::Computers;
use drift_remote_client::{Host, pc, proxy};
use rust_embed::RustEmbed;
use tauri::Manager;

#[derive(RustEmbed)]
#[folder = "../dist"]
struct Dist;

/// The phone: the UI it carries, and its name from the Kotlin plugin.
struct Phone(Arc<native::Native>);

impl Host for Phone {
    fn asset(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        Dist::get(path).map(|file| file.data)
    }

    fn device_name(&self) -> String {
        self.0.device_name()
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(native::plugin())
        .setup(|app| start(app))
        .run(tauri::generate_context!())
        .expect("failed to run Drift");
}

/// Starts the proxy on a free loopback port, then opens the WebView on it with this launch's secret.
fn start(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data = app.path().app_data_dir()?;
    let computers = Computers::load(data.join("computers.json"));
    let phone = Arc::new(Phone(app.state::<Arc<native::Native>>().inner().clone()));
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let secret = pc::hex(&secret());

    let proxy = proxy::Shared::new(computers, phone, secret, port, data.join("ui-storage.json"));
    let url = proxy.entry().parse()?;
    tauri::async_runtime::spawn(async move {
        if let Ok(listener) = tokio::net::TcpListener::from_std(listener) {
            proxy::serve(listener, proxy).await;
        }
    });

    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url)).build()?;
    Ok(())
}

fn secret() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system random source works");
    bytes
}
