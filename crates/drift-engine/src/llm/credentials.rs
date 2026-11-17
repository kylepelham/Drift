//! Provider secrets in the OS keychain or an authenticated encrypted fallback.

mod accounts;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::Credential;
pub use super::credential_file::FileError;
pub use accounts::{Account, Profile};

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
    /// Held over each keychain read and write, so a reader never mixes two versions of a split secret.
    entry_lock: Mutex<()>,
    /// Providers with a stored credential; keychains cannot enumerate, so we keep our own list.
    index: Mutex<BTreeSet<String>>,
    /// Each provider's accounts in the order they are used, for providers signed in more than the old single way.
    accounts: Mutex<BTreeMap<String, Vec<Account>>>,
    /// Providers that take no key: a local server that answered, a user's server with no key variable.
    keyless: Mutex<BTreeSet<String>>,
}

enum Backend {
    Keyring(Box<dyn Entries>),
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
            Ok(()) if !prefer_file => Backend::Keyring(Box::new(Keychain)),
            _ => match super::credential_file::ProtectedFile::open(data_dir) {
                Ok(file) => Backend::Protected(file),
                Err(error) => Backend::Unavailable(Arc::new(error)),
            },
        };

        Self::loaded(backend)
    }

    #[cfg(test)]
    pub fn in_file(path: PathBuf) -> Self {
        Self::open_test_file(path)
    }

    #[cfg(test)]
    fn open_test_file(path: PathBuf) -> Self {
        Self::loaded(Backend::File(path))
    }

    /// Reads the provider index and account lists the backend keeps.
    fn loaded(backend: Backend) -> Self {
        let this = Self {
            backend,
            write_lock: Mutex::default(),
            entry_lock: Mutex::default(),
            index: Mutex::default(),
            accounts: Mutex::default(),
            keyless: Mutex::default(),
        };

        *this.index.lock().unwrap() = this.stored(INDEX);
        *this.accounts.lock().unwrap() = this.stored(accounts::ACCOUNTS);

        this
    }

    fn stored<T: serde::de::DeserializeOwned + Default>(&self, key: &str) -> T {
        self.read(key)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
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

    /// The credential of the provider's first account.
    pub fn get(&self, provider: &str) -> Option<Credential> {
        let first = self.accounts(provider).into_iter().next()?;
        self.account(&first.key)
    }

    /// A stored credential, else what a cloud route finds for itself, else the provider's environment
    /// variable as an API key, else, for a provider that takes none, a placeholder its server ignores.
    pub fn resolve(&self, provider: &str, env: &[String]) -> Option<Credential> {
        self.resolve_account(provider, env, |key| !self.fast_only(key))
            .map(|(_, credential)| credential)
    }

    /// As `resolve`, with the key of the account the credential is stored under; none for one found elsewhere.
    /// The first `usable` account whose credential reads is taken, else the first that reads, so a refusal says why.
    /// A Fast mode key is never that fallback: it pays only for Fast mode.
    pub fn resolve_account(
        &self,
        provider: &str,
        env: &[String],
        usable: impl Fn(&str) -> bool,
    ) -> Option<(Option<String>, Credential)> {
        let accounts = self.accounts(provider);
        let stored = |key: &str| self.account(key).map(|credential| (Some(key.to_owned()), credential));

        accounts
            .iter()
            .filter(|account| usable(&account.key))
            .find_map(|account| stored(&account.key))
            .or_else(|| {
                accounts
                    .iter()
                    .filter(|account| !account.fast_only)
                    .find_map(|account| stored(&account.key))
            })
            .or_else(|| self.unstored(provider, env).map(|credential| (None, credential)))
    }

    /// The provider's first stored API key, else one from `env`. Subscription sign-ins are passed over:
    /// they pay for chat, not for the provider's other APIs.
    pub fn api_key(&self, provider: &str, env: &[String]) -> Option<String> {
        let stored = self
            .accounts(provider)
            .into_iter()
            .filter(|account| !account.fast_only)
            .find_map(|account| match self.account(&account.key) {
                Some(Credential::ApiKey { key }) if !key.trim().is_empty() => Some(key),
                _ => None,
            });

        stored.or_else(|| {
            env.iter()
                .find_map(|name| std::env::var(name).ok().filter(|key| !key.trim().is_empty()))
        })
    }

    /// A credential the provider has without one stored: a cloud route's own, an environment key, or none needed.
    pub fn found_elsewhere(&self, provider: &str, env: &[String]) -> bool {
        self.unstored(provider, env).is_some()
    }

    fn unstored(&self, provider: &str, env: &[String]) -> Option<Credential> {
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

    /// Makes `credential` the provider's only one, replacing every account it had.
    pub fn set(&self, provider: &str, credential: &Credential) -> Result<(), CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        self.replace_all(provider, credential, Account::at(provider))
    }

    /// Writes only if the account still holds `expected`; a login or logout in between wins.
    pub fn replace_if(
        &self,
        key: &str,
        expected: &Credential,
        credential: &Credential,
    ) -> Result<bool, CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        if self.account(key).as_ref() != Some(expected) {
            return Ok(false);
        }

        self.write(key, &serde_json::to_string(credential).unwrap())?;

        Ok(true)
    }

    /// Signs the provider out of every account.
    pub fn remove(&self, provider: &str) -> Result<(), CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        for account in self.accounts(provider) {
            self.delete(&account.key)?;
        }

        self.save_accounts(provider, Vec::new())
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

    #[cfg(test)]
    pub fn providers(&self) -> Vec<String> {
        self.index.lock().unwrap().iter().cloned().collect()
    }

    fn read(&self, key: &str) -> Option<String> {
        match &self.backend {
            Backend::Keyring(entries) => {
                let _held = self.entry_lock.lock().unwrap();
                read_split(entries.as_ref(), key)
            }
            Backend::Protected(file) => file.read().ok()?.get(key).and_then(Value::as_str).map(str::to_string),
            Backend::Unavailable(_) => None,
            #[cfg(test)]
            Backend::File(path) => file_map(path).get(key).and_then(Value::as_str).map(str::to_string),
        }
    }

    fn write(&self, key: &str, value: &str) -> Result<(), CredentialError> {
        match &self.backend {
            Backend::Keyring(entries) => {
                let _held = self.entry_lock.lock().unwrap();
                Ok(write_split(entries.as_ref(), key, value)?)
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
            Backend::Keyring(entries) => {
                let _held = self.entry_lock.lock().unwrap();
                Ok(delete_split(entries.as_ref(), key)?)
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

/// One secret per name, as the OS keychain holds them.
trait Entries: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
    fn put(&self, name: &str, value: &str) -> Result<(), keyring::Error>;
    /// Succeeds when there is no such entry.
    fn remove(&self, name: &str) -> Result<(), keyring::Error>;
}

struct Keychain;

impl Entries for Keychain {
    fn get(&self, name: &str) -> Option<String> {
        keyring::Entry::new(SERVICE, name).ok()?.get_password().ok()
    }

    fn put(&self, name: &str, value: &str) -> Result<(), keyring::Error> {
        keyring::Entry::new(SERVICE, name)?.set_password(value)
    }

    fn remove(&self, name: &str) -> Result<(), keyring::Error> {
        match keyring::Entry::new(SERVICE, name).and_then(|entry| entry.delete_credential()) {
            Err(keyring::Error::NoEntry) => Ok(()),
            done => done,
        }
    }
}

/// Where a split secret's pieces are, as its head entry says: `key#N`, or `key#GEN#N` since generations.
struct Pieces {
    count: usize,
    generation: Option<String>,
}

impl Pieces {
    fn of(head: &str) -> Option<Self> {
        let rest = head.strip_prefix(CHUNKED)?;
        let (count, generation) = match rest.split_once('@') {
            Some((count, generation)) => (count, Some(generation.to_string())),
            None => (rest, None),
        };

        Some(Self {
            count: count.parse().ok()?,
            generation,
        })
    }

    fn name(&self, key: &str, i: usize) -> String {
        match &self.generation {
            Some(generation) => format!("{key}#{generation}#{i}"),
            None => format!("{key}#{i}"),
        }
    }

    fn remove(&self, entries: &dyn Entries, key: &str) {
        for i in 0..self.count {
            let _ = entries.remove(&self.name(key, i));
        }
    }
}

fn read_split(entries: &dyn Entries, key: &str) -> Option<String> {
    let head = entries.get(key)?;
    let Some(pieces) = Pieces::of(&head) else {
        return Some(head);
    };

    (0..pieces.count).map(|i| entries.get(&pieces.name(key, i))).collect()
}

/// Writes new pieces under a fresh generation and the head naming them last, so a failure leaves the old ones whole.
fn write_split(entries: &dyn Entries, key: &str, value: &str) -> Result<(), keyring::Error> {
    let stale = entries.get(key).as_deref().and_then(Pieces::of);
    let characters: Vec<char> = value.chars().collect();
    let chunks: Vec<String> = characters
        .chunks(CHUNK_CHARS)
        .map(|chunk| chunk.iter().collect())
        .collect();

    if chunks.len() <= 1 {
        entries.put(key, value)?;
    } else {
        let taken = stale.as_ref().and_then(|stale| stale.generation.clone());
        let generation = std::iter::repeat_with(|| crate::random_hex(4))
            .find(|generation| taken.as_ref() != Some(generation))
            .unwrap();
        let pieces = Pieces {
            count: chunks.len(),
            generation: Some(generation.clone()),
        };
        let written = chunks
            .iter()
            .enumerate()
            .try_for_each(|(i, chunk)| entries.put(&pieces.name(key, i), chunk))
            .and_then(|()| entries.put(key, &format!("{CHUNKED}{}@{generation}", chunks.len())));
        if let Err(error) = written {
            pieces.remove(entries, key);
            return Err(error);
        }
    }
    if let Some(stale) = stale {
        stale.remove(entries, key);
    }

    Ok(())
}

/// The head goes first, so a removal stopped partway leaves no head naming missing pieces.
fn delete_split(entries: &dyn Entries, key: &str) -> Result<(), keyring::Error> {
    let stale = entries.get(key).as_deref().and_then(Pieces::of);
    entries.remove(key)?;
    if let Some(stale) = stale {
        stale.remove(entries, key);
    }

    Ok(())
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
        let path = crate::platform::temp::test_run().join(format!("drift-cred-{}.json", crate::random_hex(4)));
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
        let path = crate::platform::temp::test_run().join(format!("drift-cred-{}.json", crate::random_hex(4)));
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

        let path = crate::platform::temp::test_run().join(format!("drift-cred-{}.json", crate::random_hex(4)));
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
        let path = crate::platform::temp::test_run().join(format!("drift-cred-{}.json", crate::random_hex(4)));
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
        let path = crate::platform::temp::test_run().join(format!("drift-cred-{}.json", crate::random_hex(4)));
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

    /// Without `loaded`, so the real keychain's own index and accounts are never read or touched.
    fn keychain(entries: Box<dyn Entries>) -> Credentials {
        Credentials {
            backend: Backend::Keyring(entries),
            write_lock: Mutex::default(),
            entry_lock: Mutex::default(),
            index: Mutex::default(),
            accounts: Mutex::default(),
            keyless: Mutex::default(),
        }
    }

    #[derive(Clone, Default)]
    struct Memory {
        entries: Arc<Mutex<BTreeMap<String, String>>>,
        /// Puts that succeed before the rest fail; `None` never fails.
        puts_left: Arc<Mutex<Option<usize>>>,
    }

    impl Memory {
        fn names(&self) -> Vec<String> {
            self.entries.lock().unwrap().keys().cloned().collect()
        }
    }

    impl Entries for Memory {
        fn get(&self, name: &str) -> Option<String> {
            self.entries.lock().unwrap().get(name).cloned()
        }

        fn put(&self, name: &str, value: &str) -> Result<(), keyring::Error> {
            if let Some(left) = self.puts_left.lock().unwrap().as_mut() {
                if *left == 0 {
                    return Err(keyring::Error::NotSupportedByStore("full".into()));
                }
                *left -= 1;
            }
            self.entries.lock().unwrap().insert(name.into(), value.into());

            Ok(())
        }

        fn remove(&self, name: &str) -> Result<(), keyring::Error> {
            self.entries.lock().unwrap().remove(name);

            Ok(())
        }
    }

    #[test]
    fn a_secret_split_the_old_way_still_reads_and_its_pieces_go_when_rewritten() {
        let memory = Memory::default();
        for (name, value) in [("k", "\u{1}chunked:2"), ("k#0", "ab"), ("k#1", "cd")] {
            memory.put(name, value).unwrap();
        }
        let store = keychain(Box::new(memory.clone()));
        assert_eq!(store.read("k").as_deref(), Some("abcd"));

        let long = "x".repeat(CHUNK_CHARS * 2 + 1);
        store.write("k", &long).unwrap();
        assert_eq!(store.read("k"), Some(long));
        assert_eq!(
            memory.names().len(),
            4,
            "the head and three new pieces: {:?}",
            memory.names()
        );
        assert!(!memory.names().contains(&"k#0".to_string()));

        store.write("k", "short").unwrap();
        assert_eq!(memory.names(), ["k"]);
        store.write("k", &"y".repeat(CHUNK_CHARS + 1)).unwrap();
        store.delete("k").unwrap();
        assert!(memory.names().is_empty());
    }

    #[test]
    fn a_split_write_that_fails_partway_leaves_the_old_secret_whole() {
        let memory = Memory::default();
        let store = keychain(Box::new(memory.clone()));
        let old = "o".repeat(CHUNK_CHARS * 2 + 1);
        store.write("k", &old).unwrap();
        let before = memory.names();

        // Failing at the second piece, then at the head once all three pieces are in.
        for puts in [1, 3] {
            *memory.puts_left.lock().unwrap() = Some(puts);
            assert!(store.write("k", &"n".repeat(CHUNK_CHARS * 2 + 1)).is_err());
            *memory.puts_left.lock().unwrap() = None;

            assert_eq!(store.read("k"), Some(old.clone()));
            assert_eq!(memory.names(), before, "the new pieces are cleared away");
        }
    }

    #[test]
    fn a_read_during_a_split_write_sees_one_whole_version() {
        let store = Arc::new(keychain(Box::new(Memory::default())));
        let versions = ["a".repeat(CHUNK_CHARS * 3), "b".repeat(CHUNK_CHARS * 2 + 7)];
        store.write("k", &versions[0]).unwrap();

        let writer = {
            let (store, versions) = (store.clone(), versions.clone());
            std::thread::spawn(move || {
                for i in 0..200 {
                    store.write("k", &versions[i % 2]).unwrap();
                }
            })
        };
        for _ in 0..200 {
            let read = store.read("k").expect("a split secret is never missing mid-write");
            assert!(versions.contains(&read), "a read mixed two versions");
        }
        writer.join().unwrap();
    }

    #[test]
    fn long_secrets_round_trip_through_the_real_keychain() {
        if keyring::Entry::store_status().is_err() {
            return;
        }

        let store = keychain(Box::new(Keychain));
        let key = format!("probe-{}", crate::random_hex(3));
        let long = Credential::OAuth {
            access: "a".repeat(2500),
            refresh: "r".repeat(700),
            expires_at: 1,
            account: Some("acc".into()),
        };

        store.write(&key, &serde_json::to_string(&long).unwrap()).unwrap();
        assert_eq!(store.account(&key), Some(long));
        let pieces = Keychain.get(&key).as_deref().and_then(Pieces::of).unwrap();
        store.delete(&key).unwrap();
        assert!(store.account(&key).is_none());
        assert!(Keychain.get(&pieces.name(&key, 0)).is_none(), "chunks are removed too");
    }
}
