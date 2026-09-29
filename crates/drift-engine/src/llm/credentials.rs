//! Provider secrets in the OS keychain, with a file fallback for hosts that have none.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;

use super::Credential;

const SERVICE: &str = "dev.drift.app";
const INDEX: &str = "__providers";
const FALLBACK_FILE: &str = "credentials.json";

pub struct Credentials {
    backend: Backend,
    /// Every mutation holds this, so a compare-and-set cannot interleave with a login or logout.
    write_lock: Mutex<()>,
    /// Providers with a stored credential; keychains cannot enumerate, so we keep our own list.
    index: Mutex<BTreeSet<String>>,
}

enum Backend {
    Keyring,
    File(PathBuf),
}

impl Credentials {
    pub fn open(data_dir: &Path, prefer_file: bool) -> Self {
        let backend = match keyring::Entry::store_status() {
            Ok(()) if !prefer_file => Backend::Keyring,
            _ => Backend::File(data_dir.join(FALLBACK_FILE)),
        };
        let this = Self { backend, write_lock: Mutex::default(), index: Mutex::default() };
        let index = this.read(INDEX).and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default();
        *this.index.lock().unwrap() = index;
        this
    }

    #[cfg(test)]
    pub fn in_file(path: PathBuf) -> Self {
        Self { backend: Backend::File(path), write_lock: Mutex::default(), index: Mutex::default() }
    }

    pub fn get(&self, provider: &str) -> Option<Credential> {
        self.read(provider).and_then(|json| serde_json::from_str(&json).ok())
    }

    /// A stored credential, else the provider's environment variable as an API key.
    pub fn resolve(&self, provider: &str, env: &[String]) -> Option<Credential> {
        self.get(provider).or_else(|| {
            env.iter()
                .find_map(|name| std::env::var(name).ok())
                .filter(|key| !key.is_empty())
                .map(|key| Credential::ApiKey { key })
        })
    }

    pub fn set(&self, provider: &str, credential: &Credential) -> Result<(), String> {
        let _held = self.write_lock.lock().unwrap();
        self.set_locked(provider, credential)
    }

    fn set_locked(&self, provider: &str, credential: &Credential) -> Result<(), String> {
        self.write(provider, &serde_json::to_string(credential).unwrap())?;
        let mut index = self.index.lock().unwrap();
        index.insert(provider.into());
        self.write(INDEX, &serde_json::to_string(&*index).unwrap())
    }

    /// Writes only if the stored credential is still xpected; a login or logout in between wins.
    pub fn replace_if(&self, provider: &str, expected: &Credential, credential: &Credential) -> Result<bool, String> {
        let _held = self.write_lock.lock().unwrap();
        if self.get(provider).as_ref() != Some(expected) {
            return Ok(false);
        }
        self.set_locked(provider, credential)?;
        Ok(true)
    }

    pub fn remove(&self, provider: &str) -> Result<(), String> {
        let _held = self.write_lock.lock().unwrap();
        self.delete(provider)?;
        let mut index = self.index.lock().unwrap();
        index.remove(provider);
        self.write(INDEX, &serde_json::to_string(&*index).unwrap())
    }

    pub fn providers(&self) -> Vec<String> {
        self.index.lock().unwrap().iter().cloned().collect()
    }

    fn read(&self, key: &str) -> Option<String> {
        match &self.backend {
            Backend::Keyring => keyring::Entry::new(SERVICE, key).ok()?.get_password().ok(),
            Backend::File(path) => file_map(path).get(key).and_then(Value::as_str).map(str::to_string),
        }
    }

    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        match &self.backend {
            Backend::Keyring => keyring::Entry::new(SERVICE, key)
                .and_then(|entry| entry.set_password(value))
                .map_err(|e| e.to_string()),
            Backend::File(path) => {
                let mut map = file_map(path);
                map.insert(key.into(), Value::String(value.into()));
                save_file(path, &map)
            }
        }
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        match &self.backend {
            Backend::Keyring => match keyring::Entry::new(SERVICE, key).and_then(|entry| entry.delete_credential()) {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(error.to_string()),
            },
            Backend::File(path) => {
                let mut map = file_map(path);
                map.remove(key);
                save_file(path, &map)
            }
        }
    }
}

fn file_map(path: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn save_file(path: &Path, map: &serde_json::Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, serde_json::to_string(map).unwrap()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_backend_round_trips_and_indexes() {
        let path = std::env::temp_dir().join(format!("drift-cred-{}.json", crate::random_hex(4)));
        let store = Credentials::in_file(path.clone());
        assert!(store.get("anthropic").is_none());
        store.set("anthropic", &Credential::ApiKey { key: "sk".into() }).unwrap();
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
        let stale = Credential::OAuth { access: "old".into(), refresh: "r".into(), expires_at: 1, account: None };
        let fresh = Credential::OAuth { access: "new".into(), refresh: "r2".into(), expires_at: 9, account: None };
        store.set("p", &stale).unwrap();
        assert!(store.replace_if("p", &stale, &fresh).unwrap());
        assert_eq!(store.get("p"), Some(fresh.clone()));
        let login = Credential::ApiKey { key: "k".into() };
        store.set("p", &login).unwrap();
        assert!(!store.replace_if("p", &fresh, &stale).unwrap(), "a login after the refresh started must win");
        assert_eq!(store.get("p"), Some(login));
        store.remove("p").unwrap();
        assert!(!store.replace_if("p", &stale, &fresh).unwrap(), "a logout must not be undone");
        assert!(store.get("p").is_none());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn concurrent_logins_and_refreshes_never_resurrect_a_replaced_credential() {
        use std::sync::Arc;
        let path = std::env::temp_dir().join(format!("drift-cred-{}.json", crate::random_hex(4)));
        let store = Arc::new(Credentials::in_file(path.clone()));
        let stale = Credential::OAuth { access: "old".into(), refresh: "r".into(), expires_at: 1, account: None };
        let refreshed = Credential::OAuth { access: "new".into(), refresh: "r2".into(), expires_at: 9, account: None };
        for _ in 0..50 {
            store.set("p", &stale).unwrap();
            let refresher = { let store = store.clone(); let (stale, refreshed) = (stale.clone(), refreshed.clone()); std::thread::spawn(move || store.replace_if("p", &stale, &refreshed).unwrap()) };
            let logout = { let store = store.clone(); std::thread::spawn(move || store.remove("p").unwrap()) };
            let replaced = refresher.join().unwrap();
            logout.join().unwrap();
            // Whichever ran first, a logout is never undone by a refresh that read the old value earlier.
            assert_eq!(store.get("p"), None, "replaced={replaced}");
        }
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn env_var_is_the_fallback() {
        let path = std::env::temp_dir().join(format!("drift-cred-{}.json", crate::random_hex(4)));
        let store = Credentials::in_file(path);
        let name = format!("DRIFT_TEST_KEY_{}", crate::random_hex(2));
        assert!(store.resolve("x", std::slice::from_ref(&name)).is_none());
        std::env::set_var(&name, "from-env");
        assert_eq!(store.resolve("x", std::slice::from_ref(&name)), Some(Credential::ApiKey { key: "from-env".into() }));
        std::env::remove_var(&name);
    }
}
