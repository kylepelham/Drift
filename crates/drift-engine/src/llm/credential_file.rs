//! Authenticated encrypted fallback, with its key protected by DPAPI or supplied by the headless host.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde_json::{Map, Value};

mod error;
pub use error::FileError;

const AES_HEADER: &[u8] = b"DRIFT-AES1\n";
#[cfg(windows)]
const DPAPI_HEADER: &[u8] = b"DRIFT-DPAPI1\n";

pub(super) struct ProtectedFile {
    path: PathBuf,
    protection: Protection,
}

enum Protection {
    Aes(Box<LessSafeKey>),
    #[cfg(windows)]
    Dpapi,
}

impl ProtectedFile {
    pub(super) fn open(dir: &Path) -> Result<Self, FileError> {
        let protection = match std::env::var("DRIFT_CREDENTIALS_KEY") {
            Ok(key) => Protection::Aes(aes_key(
                &base64::engine::general_purpose::STANDARD
                    .decode(key)
                    .map_err(|_| FileError::KeyEncoding)?,
            )?),
            Err(std::env::VarError::NotPresent) => platform_protection()?,
            Err(_) => return Err(FileError::KeyUnicode),
        };

        std::fs::create_dir_all(dir)?;
        let file = Self {
            path: dir.join("credentials.enc"),
            protection,
        };

        let legacy = dir.join("credentials.json");
        if legacy.exists() {
            crate::platform::private_file::restrict(&legacy)?;
            file.migrate(&legacy)?;
        }

        Ok(file)
    }

    /// Merges plaintext credentials into the encrypted store, keeping encrypted values on conflict.
    /// Removes the plaintext file only after every merged value reads back correctly.
    fn migrate(&self, legacy: &Path) -> Result<(), FileError> {
        let bytes = std::fs::read(legacy)?;
        let old: Map<String, Value> = serde_json::from_slice(&bytes).map_err(|_| FileError::LegacyJson)?;

        let mut merged = self.read()?;
        let missing: Vec<String> = old.keys().filter(|key| !merged.contains_key(*key)).cloned().collect();
        if !missing.is_empty() {
            merged.extend(old.into_iter().filter(|(key, _)| missing.contains(key)));
            self.save(&merged)?;
        }

        if self.read()? != merged {
            return Err(FileError::MigrationMismatch);
        }

        std::fs::remove_file(legacy).map_err(FileError::RemoveLegacy)
    }

    pub(super) fn read(&self) -> Result<Map<String, Value>, FileError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(error) => return Err(FileError::Io(error)),
        };

        let plaintext = self.decrypt(&bytes)?;
        serde_json::from_slice(&plaintext).map_err(|_| FileError::DecryptedJson)
    }

    pub(super) fn save(&self, map: &Map<String, Value>) -> Result<(), FileError> {
        let plaintext = serde_json::to_vec(map)?;
        let encrypted = self.encrypt(&plaintext)?;

        crate::platform::private_file::write(&self.path, &encrypted).map_err(FileError::Io)
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, FileError> {
        match &self.protection {
            Protection::Aes(key) => {
                let mut nonce = [0u8; 12];
                getrandom::fill(&mut nonce).map_err(|error| FileError::Random(error.to_string()))?;
                let mut encrypted = plaintext.to_vec();

                key.seal_in_place_append_tag(
                    Nonce::assume_unique_for_key(nonce),
                    Aad::from(AES_HEADER),
                    &mut encrypted,
                )
                .map_err(|_| FileError::Encrypt)?;

                Ok([AES_HEADER, &nonce, &encrypted].concat())
            }
            #[cfg(windows)]
            Protection::Dpapi => Ok([DPAPI_HEADER, &dpapi(plaintext, true)?].concat()),
        }
    }

    fn decrypt(&self, bytes: &[u8]) -> Result<Vec<u8>, FileError> {
        match &self.protection {
            Protection::Aes(key) => {
                let body = bytes
                    .strip_prefix(AES_HEADER)
                    .filter(|body| body.len() >= 28)
                    .ok_or(FileError::Envelope)?;
                let nonce: [u8; 12] = body[..12].try_into().unwrap();
                let mut encrypted = body[12..].to_vec();

                key.open_in_place(
                    Nonce::assume_unique_for_key(nonce),
                    Aad::from(AES_HEADER),
                    &mut encrypted,
                )
                .map(Vec::from)
                .map_err(|_| FileError::Authentication)
            }
            #[cfg(windows)]
            Protection::Dpapi => dpapi(
                bytes.strip_prefix(DPAPI_HEADER).ok_or(FileError::ProtectionMethod)?,
                false,
            ),
        }
    }
}

fn aes_key(bytes: &[u8]) -> Result<Box<LessSafeKey>, FileError> {
    UnboundKey::new(&AES_256_GCM, bytes)
        .map(|key| Box::new(LessSafeKey::new(key)))
        .map_err(|_| FileError::KeyLength)
}

#[cfg(windows)]
fn platform_protection() -> Result<Protection, FileError> {
    Ok(Protection::Dpapi)
}

#[cfg(not(windows))]
fn platform_protection() -> Result<Protection, FileError> {
    Err(FileError::Unavailable)
}

#[cfg(windows)]
fn dpapi(bytes: &[u8], protect: bool) -> Result<Vec<u8>, FileError> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into().map_err(|_| FileError::TooLarge)?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    // SAFETY: DPAPI borrows live input bytes and returns a LocalAlloc buffer copied before LocalFree.
    unsafe {
        let success = if protect {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if success == 0 {
            return Err(FileError::Dpapi(std::io::Error::last_os_error()));
        }

        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_files_authenticate_and_replace_atomically() {
        let dir = std::env::temp_dir().join(format!("drift-protected-{}", crate::random_hex(4)));
        let file = ProtectedFile {
            path: dir.join("credentials.enc"),
            protection: Protection::Aes(aes_key(&[42u8; 32]).unwrap()),
        };
        let map = Map::from_iter([("provider".into(), Value::String("secret-token".into()))]);
        file.save(&map).unwrap();
        assert_eq!(file.read().unwrap(), map);
        let first = std::fs::read(&file.path).unwrap();
        assert!(!String::from_utf8_lossy(&first).contains("secret-token"));

        file.save(&map).unwrap();
        assert_ne!(
            std::fs::read(&file.path).unwrap(),
            first,
            "every save uses a fresh nonce"
        );

        let wrong = ProtectedFile {
            path: file.path.clone(),
            protection: Protection::Aes(aes_key(&[43u8; 32]).unwrap()),
        };
        let error = wrong.read().unwrap_err();
        assert!(matches!(error, FileError::Authentication));
        assert_eq!(
            error.to_string(),
            "credential authentication failed; the key is wrong or the file is damaged"
        );

        let mut damaged = first;
        *damaged.last_mut().unwrap() ^= 1;
        std::fs::write(&file.path, damaged).unwrap();
        assert!(file.read().is_err());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn legacy_plaintext_is_removed_only_after_a_verified_encrypted_save() {
        let dir = std::env::temp_dir().join(format!("drift-migrate-secret-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let map = Map::from_iter([("p".into(), Value::String("legacy-private-key".into()))]);
        std::fs::write(dir.join("credentials.json"), serde_json::to_vec(&map).unwrap()).unwrap();
        let file = ProtectedFile::open(&dir).unwrap();
        assert_eq!(file.read().unwrap(), map);
        assert!(!dir.join("credentials.json").exists());
        assert!(!String::from_utf8_lossy(&std::fs::read(&file.path).unwrap()).contains("legacy-private-key"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_keys_merge_into_an_existing_encrypted_store_which_wins_on_conflict() {
        let dir = std::env::temp_dir().join(format!("drift-merge-secret-{}", crate::random_hex(4)));
        let file = ProtectedFile {
            path: dir.join("credentials.enc"),
            protection: Protection::Aes(aes_key(&[7u8; 32]).unwrap()),
        };
        file.save(&Map::from_iter([("anthropic".into(), Value::String("newer".into()))]))
            .unwrap();
        let legacy = dir.join("credentials.json");
        std::fs::write(&legacy, r#"{"anthropic":"older","openai":"only-in-legacy"}"#).unwrap();
        file.migrate(&legacy).unwrap();
        let merged = file.read().unwrap();
        assert_eq!(
            (merged["anthropic"].as_str(), merged["openai"].as_str()),
            (Some("newer"), Some("only-in-legacy"))
        );
        assert!(!legacy.exists());
        std::fs::write(&legacy, "not json").unwrap();
        assert!(
            file.migrate(&legacy).is_err() && legacy.exists(),
            "an unreadable legacy file is kept, never deleted"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_protects_the_fallback_without_a_sidecar_key() {
        let dir = std::env::temp_dir().join(format!("drift-dpapi-{}", crate::random_hex(4)));
        let file = ProtectedFile {
            path: dir.join("credentials.enc"),
            protection: Protection::Dpapi,
        };
        let map = Map::from_iter([("p".into(), Value::String("private-key-value".into()))]);
        file.save(&map).unwrap();
        assert_eq!(file.read().unwrap(), map);
        assert!(!String::from_utf8_lossy(&std::fs::read(&file.path).unwrap()).contains("private-key-value"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn crypto_validation_errors_keep_the_existing_text() {
        let file = ProtectedFile {
            path: PathBuf::new(),
            protection: Protection::Aes(aes_key(&[42u8; 32]).unwrap()),
        };
        let error = file.decrypt(b"not an envelope").unwrap_err();
        assert!(matches!(error, FileError::Envelope));
        assert_eq!(error.to_string(), "credential file has an invalid encryption envelope");

        let error = aes_key(&[42u8; 31]).err().unwrap();
        assert!(matches!(error, FileError::KeyLength));
        assert_eq!(error.to_string(), "DRIFT_CREDENTIALS_KEY must contain exactly 32 bytes");
    }
}
