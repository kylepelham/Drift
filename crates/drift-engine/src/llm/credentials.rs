//! Provider secrets in the OS keychain or an authenticated encrypted fallback.

use std::collections::BTreeSet;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::Credential;
pub use super::credential_file::FileError;

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error(transparent)]
    Keyring(#[from] keyring::Error),
    #[error(transparent)]
    File(#[from] FileError),
    #[error(transparent)]
    Unavailable(Arc<FileError>),
    #[cfg(test)]
    #[error(transparent)]
    TestFile(#[from] std::io::Error),
}

const SERVICE: &str = "dev.drift.app";
const INDEX: &str = "__providers";
#[cfg(test)]
const FALLBACK_FILE: &str = "credentials.json";
/// Windows stores secrets as UTF-16 under 2560 bytes; OAuth tokens are longer, so they are split.
const CHUNK_CHARS: usize = 1000;
const CHUNKED: &str = "\u{1}chunked:";

pub struct Credentials {
    backend: Backend,
    /// Every mutation holds this, so a compare-and-set cannot interleave with a login or logout.
    write_lock: Mutex<()>,
    /// Providers with a stored credential; keychains cannot enumerate, so we keep our own list.
    index: Mutex<BTreeSet<String>>,
    /// Providers that take no key: a local server that answered, a user's server with no key variable.
    keyless: Mutex<BTreeSet<String>>,
}

enum Backend {
    Keyring,
    Protected(super::credential_file::ProtectedFile),
    Unavailable(Arc<FileError>),
    #[cfg(test)]
    File(PathBuf),
}

impl Credentials {
    pub fn open(data_dir: &Path, prefer_file: bool) -> Self {
        #[cfg(test)]
        if prefer_file {
            return Self::open_test_file(data_dir.join(FALLBACK_FILE));
        }

        let backend = match keyring::Entry::store_status() {
            Ok(()) if !prefer_file => Backend::Keyring,
            _ => match super::credential_file::ProtectedFile::open(data_dir) {
                Ok(file) => Backend::Protected(file),
                Err(error) => Backend::Unavailable(Arc::new(error)),
            },
        };

        let this = Self {
            backend,
            write_lock: Mutex::default(),
            index: Mutex::default(),
            keyless: Mutex::default(),
        };

        let index = this
            .read(INDEX)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        *this.index.lock().unwrap() = index;

        this
    }

    #[cfg(test)]
    pub fn in_file(path: PathBuf) -> Self {
        Self::open_test_file(path)
    }

    #[cfg(test)]
    fn open_test_file(path: PathBuf) -> Self {
        let this = Self {
            backend: Backend::File(path),
            write_lock: Mutex::default(),
            index: Mutex::default(),
            keyless: Mutex::default(),
        };
        *this.index.lock().unwrap() = this
            .read(INDEX)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();

        this
    }

    /// Marks a provider as taking no key (`on`), or as needing one again.
    pub fn set_keyless(&self, provider: &str, on: bool) {
        let mut keyless = self.keyless.lock().unwrap();
        if on {
            keyless.insert(provider.into());
        } else {
            keyless.remove(provider);
        }
    }

    pub fn get(&self, provider: &str) -> Option<Credential> {
        self.read(provider).and_then(|json| serde_json::from_str(&json).ok())
    }

    /// A stored credential, else what a cloud route finds for itself, else the provider's environment
    /// variable as an API key, else, for a provider that takes none, a placeholder its server ignores.
    pub fn resolve(&self, provider: &str, env: &[String]) -> Option<Credential> {
        if let Some(stored) = self.get(provider) {
            return Some(stored);
        }

        // A cloud route's variables are keys to sign with or files to read, never an API key.
        if let Some(found) = super::ambient(provider) {
            return found.map(|source| Credential::Ambient { source });
        }

        let from_env = env
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .filter(|key| !key.is_empty());
        let keyless = || {
            self.keyless
                .lock()
                .unwrap()
                .contains(provider)
                .then(|| "none".to_string())
        };

        from_env.or_else(keyless).map(|key| Credential::ApiKey { key })
    }

    pub fn set(&self, provider: &str, credential: &Credential) -> Result<(), CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        self.set_locked(provider, credential)
    }

    fn set_locked(&self, provider: &str, credential: &Credential) -> Result<(), CredentialError> {
        self.write(provider, &serde_json::to_string(credential).unwrap())?;

        let mut index = self.index.lock().unwrap();
        index.insert(provider.into());

        self.write(INDEX, &serde_json::to_string(&*index).unwrap())
    }

    /// Writes only if the stored credential is still expected; a login or logout in between wins.
    pub fn replace_if(
        &self,
        provider: &str,
        expected: &Credential,
        credential: &Credential,
    ) -> Result<bool, CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        if self.get(provider).as_ref() != Some(expected) {
            return Ok(false);
        }

        self.set_locked(provider, credential)?;

        Ok(true)
    }

    pub fn remove(&self, provider: &str) -> Result<(), CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        self.delete(provider)?;

        let mut index = self.index.lock().unwrap();
        index.remove(provider);

        self.write(INDEX, &serde_json::to_string(&*index).unwrap())
    }

    /// A secret that is not a provider's (an MCP server's sign-in), kept out of the provider index.
    pub fn secret(&self, key: &str) -> Option<String> {
        self.read(key)
    }

    pub fn set_secret(&self, key: &str, value: &str) -> Result<(), CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        self.write(key, value)
    }

    pub fn remove_secret(&self, key: &str) -> Result<(), CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        self.delete(key)
    }

    pub fn providers(&self) -> Vec<String> {
        self.index.lock().unwrap().iter().cloned().collect()
    }

    fn read(&self, key: &str) -> Option<String> {
        match &self.backend {
            Backend::Keyring => {
                let head = keyring::Entry::new(SERVICE, key).ok()?.get_password().ok()?;
                let Some(count) = head.strip_prefix(CHUNKED).and_then(|n| n.parse::<usize>().ok()) else {
                    return Some(head);
                };

                (0..count)
                    .map(|i| {
                        keyring::Entry::new(SERVICE, &format!("{key}#{i}"))
                            .ok()?
                            .get_password()
                            .ok()
                    })
                    .collect()
            }
            Backend::Protected(file) => file.read().ok()?.get(key).and_then(Value::as_str).map(str::to_string),
            Backend::Unavailable(_) => None,
            #[cfg(test)]
            Backend::File(path) => file_map(path).get(key).and_then(Value::as_str).map(str::to_string),
        }
    }

    fn write(&self, key: &str, value: &str) -> Result<(), CredentialError> {
        match &self.backend {
            Backend::Keyring => {
                let characters: Vec<char> = value.chars().collect();
                let chunks: Vec<String> = characters
                    .chunks(CHUNK_CHARS)
                    .map(|chunk| chunk.iter().collect())
                    .collect();
                let put = |name: String, text: &str| {
                    keyring::Entry::new(SERVICE, &name)
                        .and_then(|e| e.set_password(text))
                        .map_err(CredentialError::Keyring)
                };

                if chunks.len() <= 1 {
                    return put(key.into(), value);
                }

                for (i, chunk) in chunks.iter().enumerate() {
                    put(format!("{key}#{i}"), chunk)?;
                }

                put(key.into(), &format!("{CHUNKED}{}", chunks.len()))
            }
            Backend::Protected(file) => {
                let mut map = file.read()?;
                map.insert(key.into(), Value::String(value.into()));
                file.save(&map).map_err(CredentialError::File)
            }
            Backend::Unavailable(error) => Err(CredentialError::Unavailable(error.clone())),
            #[cfg(test)]
            Backend::File(path) => {
                let mut map = file_map(path);
                map.insert(key.into(), Value::String(value.into()));
                save_file(path, &map)
            }
        }
    }

    fn delete(&self, key: &str) -> Result<(), CredentialError> {
        match &self.backend {
            Backend::Keyring => {
                let head = keyring::Entry::new(SERVICE, key)
                    .ok()
                    .and_then(|e| e.get_password().ok())
                    .unwrap_or_default();
                let count = head
                    .strip_prefix(CHUNKED)
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(0);

                for i in 0..count {
                    let _ = keyring::Entry::new(SERVICE, &format!("{key}#{i}")).and_then(|e| e.delete_credential());
                }

                match keyring::Entry::new(SERVICE, key).and_then(|entry| entry.delete_credential()) {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(error) => Err(CredentialError::Keyring(error)),
                }
            }
            Backend::Protected(file) => {
                let mut map = file.read()?;
                map.remove(key);
                file.save(&map).map_err(CredentialError::File)
            }
            Backend::Unavailable(error) => Err(CredentialError::Unavailable(error.clone())),
            #[cfg(test)]
            Backend::File(path) => {
                let mut map = file_map(path);
                map.remove(key);
                save_file(path, &map)
            }
        }
    }
}

#[cfg(test)]
fn file_map(path: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

#[cfg(test)]
fn save_file(path: &Path, map: &serde_json::Map<String, Value>) -> Result<(), CredentialError> {
    crate::platform::private_file::write(path, &serde_json::to_vec(map).unwrap()).map_err(CredentialError::TestFile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_backend_round_trips_and_indexes() {
        let path = std::env::temp_dir().join(format!("drift-cred-{}.json", crate::random_hex(4)));
        let store = Credentials::in_file(path.clone());
        assert!(store.get("anthropic").is_none());
        store
            .set("anthropic", &Credential::ApiKey { key: "sk".into() })
            .unwrap();
        assert_eq!(store.get("anthropic"), Some(Credential::ApiKey { key: "sk".into() }));
        assert_eq!(store.providers(), ["anthropic"]);
        store.remove("anthropic").unwrap();
        assert!(store.get("anthropic").is_none());
        assert!(store.providers().is_empty());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn replace_if_yields_to_a_newer_login_or_logout() {
        let path = std::env::temp_dir().join(format!("drift-cred-{}.json", crate::random_hex(4)));
        let store = Credentials::in_file(path.clone());
        let stale = Credential::OAuth {
            access: "old".into(),
            refresh: "r".into(),
            expires_at: 1,
            account: None,
        };
        let fresh = Credential::OAuth {
            access: "new".into(),
            refresh: "r2".into(),
            expires_at: 9,
            account: None,
        };

        store.set("p", &stale).unwrap();
        assert!(store.replace_if("p", &stale, &fresh).unwrap());
        assert_eq!(store.get("p"), Some(fresh.clone()));

        let login = Credential::ApiKey { key: "k".into() };
        store.set("p", &login).unwrap();
        assert!(
            !store.replace_if("p", &fresh, &stale).unwrap(),
            "a login after the refresh started must win"
        );
        assert_eq!(store.get("p"), Some(login));

        store.remove("p").unwrap();
        assert!(
            !store.replace_if("p", &stale, &fresh).unwrap(),
            "a logout must not be undone"
        );
        assert!(store.get("p").is_none());

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn concurrent_logins_and_refreshes_never_resurrect_a_replaced_credential() {
        use std::sync::Arc;

        let path = std::env::temp_dir().join(format!("drift-cred-{}.json", crate::random_hex(4)));
        let store = Arc::new(Credentials::in_file(path.clone()));
        let stale = Credential::OAuth {
            access: "old".into(),
            refresh: "r".into(),
            expires_at: 1,
            account: None,
        };
        let refreshed = Credential::OAuth {
            access: "new".into(),
            refresh: "r2".into(),
            expires_at: 9,
            account: None,
        };

        for _ in 0..50 {
            store.set("p", &stale).unwrap();
            let refresher = {
                let store = store.clone();
                let (stale, refreshed) = (stale.clone(), refreshed.clone());
                std::thread::spawn(move || store.replace_if("p", &stale, &refreshed).unwrap())
            };
            let logout = {
                let store = store.clone();
                std::thread::spawn(move || store.remove("p").unwrap())
            };

            let replaced = refresher.join().unwrap();
            logout.join().unwrap();
            // A refresh based on older credentials must never undo a concurrent logout.
            assert_eq!(store.get("p"), None, "replaced={replaced}");
        }

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn a_cloud_routes_variables_are_never_taken_for_an_api_key() {
        let path = std::env::temp_dir().join(format!("drift-cred-{}.json", crate::random_hex(4)));
        let store = Credentials::in_file(path.clone());
        let env = [
            "AWS_ACCESS_KEY_ID".to_string(),
            "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
        ];

        for provider in ["amazon-bedrock", "google-vertex", "google-vertex-anthropic"] {
            assert!(
                !matches!(store.resolve(provider, &env), Some(Credential::ApiKey { .. })),
                "{provider}"
            );
        }

        store
            .set(
                "amazon-bedrock",
                &Credential::ApiKey {
                    key: "bedrock-key".into(),
                },
            )
            .unwrap();
        assert_eq!(
            store.resolve("amazon-bedrock", &env),
            Some(Credential::ApiKey {
                key: "bedrock-key".into()
            }),
            "a key saved in Settings wins"
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn env_var_is_the_fallback() {
        let path = std::env::temp_dir().join(format!("drift-cred-{}.json", crate::random_hex(4)));
        let store = Credentials::in_file(path);
        let unset = format!("DRIFT_TEST_KEY_{}", crate::random_hex(2));
        assert!(store.resolve("x", std::slice::from_ref(&unset)).is_none());

        // PATH is always set, so the fallback is tested without changing the process environment.
        let path_value = std::env::var("PATH").unwrap();
        assert_eq!(
            store.resolve("x", &["PATH".to_string()]),
            Some(Credential::ApiKey { key: path_value })
        );
    }
}

#[cfg(test)]
mod keyring_tests {
    use super::*;

    #[test]
    fn long_secrets_round_trip_through_the_real_keychain() {
        if keyring::Entry::store_status().is_err() {
            return;
        }

        let store = Credentials {
            backend: Backend::Keyring,
            write_lock: Mutex::default(),
            index: Mutex::default(),
            keyless: Mutex::default(),
        };
        let key = format!("probe-{}", crate::random_hex(3));
        let long = Credential::OAuth {
            access: "a".repeat(2500),
            refresh: "r".repeat(700),
            expires_at: 1,
            account: Some("acc".into()),
        };

        store.set(&key, &long).unwrap();
        assert_eq!(store.get(&key), Some(long));
        store.remove(&key).unwrap();
        assert!(store.get(&key).is_none());
        assert!(
            keyring::Entry::new(SERVICE, &format!("{key}#0"))
                .unwrap()
                .get_password()
                .is_err(),
            "chunks are removed too"
        );

        store.remove(INDEX).ok();
    }
}
