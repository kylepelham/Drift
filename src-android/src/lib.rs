//! Drift for Android: drives the Drift on your PCs, on their networks or from anywhere through OpenTunnel. The
//! WebView talks to a proxy on 127.0.0.1 that pins each PC, so no certificate warning can ever appear
//! (`docs/android.md`).

mod computers;
mod native;
mod pc;
mod proxy;
mod tls;

/// Where tests write: Drift's own folder in the system's temporary directory, never loose beside it.
#[cfg(test)]
fn test_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("Drift").join("tests").join("android");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

use std::net::Ipv4Addr;
use std::sync::Arc;

use tauri::Manager;

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
    let computers = computers::Computers::load(app.path().app_data_dir()?.join("computers.json"));
    let native = app.state::<Arc<native::Native>>().inner().clone();
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let secret = pc::hex(&secret());

    let proxy = proxy::Shared::new(computers, native, secret.clone(), port);
    tauri::async_runtime::spawn(async move {
        if let Ok(listener) = tokio::net::TcpListener::from_std(listener) {
            proxy::serve(listener, proxy).await;
        }
    });

    let url = format!("http://127.0.0.1:{port}/app/enter?secret={secret}").parse()?;
    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url)).build()?;
    Ok(())
}

fn secret() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system random source works");
    bytes
}
