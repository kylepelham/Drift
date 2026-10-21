use crate::remote_auth;
use crate::store::Store;

use super::{RemoteAccess, RemotePairing, RemoteStatus};

#[tauri::command]
pub(crate) async fn remote_access_status(access: tauri::State<'_, RemoteAccess>) -> Result<RemoteStatus, String> {
    Ok(access.status().await)
}

#[tauri::command]
pub(crate) async fn remote_access_enable(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
) -> Result<RemoteStatus, String> {
    let _transition = access.transition.lock().await;
    store.save_remote_access(true).map_err(|error| error.to_string())?;
    {
        let mut config = access.config.lock().unwrap();
        config.enabled = true;
        config.error = None;
    }

    if let Err(error) = access.start().await {
        let error = error.to_string();
        {
            let mut config = access.config.lock().unwrap();
            config.enabled = false;
            config.error = Some(error.clone());
        }
        let _ = store.save_remote_access(false);
        return Err(error);
    }

    Ok(access.status().await)
}

#[tauri::command]
pub(crate) async fn remote_access_disable(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
) -> Result<RemoteStatus, String> {
    let _transition = access.transition.lock().await;
    store.save_remote_access(false).map_err(|error| error.to_string())?;
    {
        let mut config = access.config.lock().unwrap();
        config.enabled = false;
        config.error = None;
    }

    access.invalidate_streams();
    access.stop().await;

    Ok(access.status().await)
}

/// A one-time pairing code for the Drift app; showing it again replaces the last one.
#[tauri::command]
pub(crate) async fn remote_access_pairing(access: tauri::State<'_, RemoteAccess>) -> Result<RemotePairing, String> {
    access
        .pairing()
        .await
        .ok_or_else(|| "Turn on Remote Access and connect this PC to a network first.".into())
}

/// Turns the browser version on or off: its sign-in page, password sign-in, and every browser's session.
#[tauri::command]
pub(crate) async fn remote_access_set_browser(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
    enabled: bool,
) -> Result<RemoteStatus, String> {
    access.set_browser(&store, enabled).map_err(|error| error.to_string())?;

    Ok(access.status().await)
}

/// Turns reaching this PC through OpenTunnel on or off; setting the tunnel up goes on in the background.
#[tauri::command]
pub(crate) async fn remote_access_set_tunnel(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
    enabled: bool,
) -> Result<RemoteStatus, String> {
    let _transition = access.transition.lock().await;
    access
        .set_tunnel(&store, enabled)
        .await
        .map_err(|error| error.to_string())?;

    Ok(access.status().await)
}

/// Signs out one linked device, or all of them when id is omitted.
#[tauri::command]
pub(crate) async fn remote_access_revoke(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
    id: Option<String>,
) -> Result<RemoteStatus, String> {
    access
        .auth()
        .revoke(id.as_deref(), &store)
        .map_err(|error| error.to_string())?;
    access.invalidate_streams();

    Ok(access.status().await)
}

/// Enables password sign-in with these credentials, or turns it off when both are omitted.
#[tauri::command]
pub(crate) async fn remote_access_set_password(
    access: tauri::State<'_, RemoteAccess>,
    store: tauri::State<'_, Store>,
    username: Option<String>,
    password: Option<String>,
) -> Result<RemoteStatus, String> {
    let credentials = match (username, password) {
        (Some(username), Some(password)) => {
            remote_auth::validate_credentials(&username, &password).map_err(|error| error.to_string())?;
            let hash = tokio::task::spawn_blocking(move || remote_auth::new_password_hash(&password))
                .await
                .map_err(|error| error.to_string())?;
            Some((username.trim().to_string(), hash))
        }
        (None, None) => None,
        _ => return Err("Enter both a username and a password.".into()),
    };

    access
        .auth()
        .set_password(credentials, &store)
        .map_err(|error| error.to_string())?;
    access.invalidate_streams();

    Ok(access.status().await)
}
