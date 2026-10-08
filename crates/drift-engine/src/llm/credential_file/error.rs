#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Random(String),
    #[error("DRIFT_CREDENTIALS_KEY must be base64 for 32 bytes")]
    KeyEncoding,
    #[error("DRIFT_CREDENTIALS_KEY is not valid Unicode")]
    KeyUnicode,
    #[error("DRIFT_CREDENTIALS_KEY must contain exactly 32 bytes")]
    KeyLength,
    #[error("legacy credential file could not be parsed")]
    LegacyJson,
    #[error("migrated credentials did not read back; the plaintext file was kept")]
    MigrationMismatch,
    #[error("could not remove migrated plaintext credentials: {0}")]
    RemoveLegacy(std::io::Error),
    #[error("decrypted credential file could not be parsed")]
    DecryptedJson,
    #[error("could not encrypt credentials")]
    Encrypt,
    #[error("credential file has an invalid encryption envelope")]
    Envelope,
    #[error("credential authentication failed; the key is wrong or the file is damaged")]
    Authentication,
    #[cfg(windows)]
    #[error("credential file uses a different protection method")]
    ProtectionMethod,
    #[cfg(not(windows))]
    #[error(
        "keychain unavailable; set DRIFT_CREDENTIALS_KEY to a base64-encoded 32-byte key for encrypted credential persistence"
    )]
    Unavailable,
    #[cfg(windows)]
    #[error("credential file is too large")]
    TooLarge,
    #[cfg(windows)]
    #[error("DPAPI credential protection failed: {0}")]
    Dpapi(std::io::Error),
}
