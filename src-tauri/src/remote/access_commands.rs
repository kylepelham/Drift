use crate::remote_auth;
use crate::store::Store;

use super::{RemoteAccess, RemoteStatus};

#[tauri::command]
pub(crate) async fn remote_access_status(access: tauri::State<'_, RemoteAccess>) -> Result<RemoteStatus, String> {
    Ok(access.status().await)
}

#[tauri::command]
pub(crate) async fn remote_access_enable(
    app: tauri::AppHandle,
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

    if let Err(error) = access.start(app).await {
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

/// Approves the device showing code and returns that device's name.
#[tauri::command]
pub(crate) fn remote_access_link(access: tauri::State<'_, RemoteAccess>, code: String) -> Result<String, String> {
    access.auth().approve(&code).map_err(|error| error.to_string())
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
