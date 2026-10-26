//! The cryptography of conversation sync (`docs/sync.md`). Everything a computer uploads is sealed with the
//! group's key, which only the user's computers hold, and signed by the computer that wrote it, so the server
//! stores bytes it can neither read nor forge.
//!
//! - The group key unlocks with the user's password: Argon2id stretches it into a master key, from which HKDF
//!   draws a login key (sent to the server, which keeps only its hash) and a wrapping key (never sent).
//! - Each object gets its own key from the group key and a random id, so a nonce can never repeat.
//! - Each computer signs with its own Ed25519 key; the device list binds every public key to the group key with
//!   an HMAC, so a server cannot slip in a key of its own.

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf::{HKDF_SHA256, KeyType, Salt};
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

/// Bumped when the object or vault layout changes; older readers refuse what they cannot open.
const VERSION: u8 = 1;
const KEY_LEN: usize = 32;
const ID_LEN: usize = 16;
const SIGNATURE_LEN: usize = 64;
/// Plaintext is padded to a multiple of this, so object sizes say little about what they hold.
const PAD: usize = 4096;
/// The most a sealed object may expand to once decompressed; a forged one cannot exhaust memory.
const MAX_PLAIN: usize = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CryptoError {
    #[error("this was written by a newer Drift")]
    Version,
    #[error("the data was changed, or sealed with another key")]
    Tampered,
    #[error("the signature does not match its computer")]
    Signature,
    #[error("the password could not be stretched: {0}")]
    Stretch(String),
    #[error("the system's random source failed")]
    Random,
    #[error("the data could not be compressed: {0}")]
    Compress(String),
    #[error("the data is larger than a sealed object may hold")]
    TooLarge,
}

/// Argon2id's cost, kept beside the vault so it can be raised later without locking out old vaults.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stretch {
    /// Memory in KiB.
    pub memory: u32,
    pub passes: u32,
    pub lanes: u32,
}

impl Stretch {
    /// Above OWASP's minimum (19 MiB, two passes), and still well under a second on a laptop.
    pub const DEFAULT: Stretch = Stretch {
        memory: 64 * 1024,
        passes: 3,
        lanes: 1,
    };
}

/// The user's password turned into what logs in to the server and what unwraps the group key.
pub struct Unlock {
    /// Sent to the server, which stores only its SHA-256; it says nothing of the wrapping key.
    pub login: [u8; KEY_LEN],
    wrap: [u8; KEY_LEN],
}

/// The key that seals every object of one sync group.
#[derive(Clone)]
pub struct GroupKey([u8; KEY_LEN]);

/// A computer's signing key, kept in the OS keychain as PKCS#8.
pub struct DeviceKey {
    pair: Ed25519KeyPair,
    pkcs8: Vec<u8>,
}

/// Who wrote an object and where it sits in that computer's log; bound into the seal, so moving it fails.
#[derive(Clone, Debug, PartialEq)]
pub struct Context<'a> {
    pub group: &'a str,
    pub device: &'a str,
    pub sequence: u64,
}

fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut bytes = [0u8; N];
    SystemRandom::new().fill(&mut bytes).map_err(|_| CryptoError::Random)?;
    Ok(bytes)
}

/// A fresh salt for a new vault's password.
pub fn new_salt() -> Result<[u8; ID_LEN], CryptoError> {
    random()
}

struct Len(usize);

impl KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

/// HKDF-SHA256 of `secret`, salted and labelled, into 32 bytes.
fn derive(secret: &[u8], salt: &[u8], label: &str) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    Salt::new(HKDF_SHA256, salt)
        .extract(secret)
        .expand(&[label.as_bytes()], Len(KEY_LEN))
        .and_then(|okm| okm.fill(&mut out))
        .expect("32 bytes is within HKDF-SHA256's output limit");
    out
}

impl Unlock {
    pub fn from_password(password: &str, salt: &[u8], stretch: Stretch) -> Result<Unlock, CryptoError> {
        let params = argon2::Params::new(stretch.memory, stretch.passes, stretch.lanes, Some(KEY_LEN))
            .map_err(|error| CryptoError::Stretch(error.to_string()))?;
        let mut master = [0u8; KEY_LEN];
        argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
            .hash_password_into(password.as_bytes(), salt, &mut master)
            .map_err(|error| CryptoError::Stretch(error.to_string()))?;

        Ok(Unlock {
            login: derive(&master, salt, "drift-sync login v1"),
            wrap: derive(&master, salt, "drift-sync wrap v1"),
        })
    }

    /// The group key sealed for the server to keep; only this password opens it.
    pub fn wrap(&self, key: &GroupKey, group: &str) -> Result<Vec<u8>, CryptoError> {
        let nonce = random::<12>()?;
        let mut sealed = key.0.to_vec();
        aead(&self.wrap)
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), vault_aad(group), &mut sealed)
            .map_err(|_| CryptoError::Tampered)?;

        Ok([&[VERSION][..], &nonce, &sealed].concat())
    }

    pub fn unwrap(&self, wrapped: &[u8], group: &str) -> Result<GroupKey, CryptoError> {
        let (&version, rest) = wrapped.split_first().ok_or(CryptoError::Tampered)?;
        if version != VERSION {
            return Err(CryptoError::Version);
        }
        let (nonce, sealed) = rest.split_at_checked(12).ok_or(CryptoError::Tampered)?;
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| CryptoError::Tampered)?;
        let mut sealed = sealed.to_vec();
        let opened = aead(&self.wrap)
            .open_in_place(nonce, vault_aad(group), &mut sealed)
            .map_err(|_| CryptoError::Tampered)?;

        Ok(GroupKey(opened.try_into().map_err(|_| CryptoError::Tampered)?))
    }
}

fn vault_aad(group: &str) -> Aad<Vec<u8>> {
    Aad::from(format!("drift-sync vault v1|{group}").into_bytes())
}

fn aead(key: &[u8; KEY_LEN]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).expect("a 32-byte ChaCha20-Poly1305 key"))
}

impl GroupKey {
    pub fn generate() -> Result<GroupKey, CryptoError> {
        random().map(GroupKey)
    }

    /// The raw key, for the OS keychain on a computer the user asked to remember.
    pub fn bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> GroupKey {
        GroupKey(bytes)
    }

    /// Compresses, pads and seals `plain`, then signs the result as `device`.
    pub fn seal(&self, device: &DeviceKey, context: &Context, plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if plain.len() > MAX_PLAIN {
            return Err(CryptoError::TooLarge);
        }
        let id = random::<ID_LEN>()?;
        let mut body = padded(&zstd::encode_all(plain, 9).map_err(|error| CryptoError::Compress(error.to_string()))?);
        let header = [&[VERSION][..], &id].concat();
        aead(&derive(&self.0, &id, "drift-sync object v1"))
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key([0; 12]),
                object_aad(&header, context),
                &mut body,
            )
            .map_err(|_| CryptoError::Tampered)?;

        let signed = [header, body].concat();
        let signature = device.pair.sign(&signed);
        Ok([signed, signature.as_ref().to_vec()].concat())
    }

    /// Checks `object` was signed by `signer` for `context`, then opens it.
    pub fn open(&self, signer: &[u8], context: &Context, object: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let split = object.len().checked_sub(SIGNATURE_LEN).ok_or(CryptoError::Tampered)?;
        let (signed, signature) = object.split_at(split);
        UnparsedPublicKey::new(&ED25519, signer)
            .verify(signed, signature)
            .map_err(|_| CryptoError::Signature)?;
        let (header, body) = signed.split_at_checked(1 + ID_LEN).ok_or(CryptoError::Tampered)?;
        if header[0] != VERSION {
            return Err(CryptoError::Version);
        }

        let mut body = body.to_vec();
        let opened = aead(&derive(&self.0, &header[1..], "drift-sync object v1"))
            .open_in_place(
                Nonce::assume_unique_for_key([0; 12]),
                object_aad(header, context),
                &mut body,
            )
            .map_err(|_| CryptoError::Tampered)?;
        decompress(unpadded(opened)?)
    }

    /// The tag binding a computer's public key to this group, which a server without the key cannot make.
    pub fn vouch(&self, device: &str, public: &[u8]) -> Vec<u8> {
        hmac::sign(&self.vouching(), &vouched(device, public)).as_ref().to_vec()
    }

    pub fn vouched_for(&self, device: &str, public: &[u8], tag: &[u8]) -> bool {
        hmac::verify(&self.vouching(), &vouched(device, public), tag).is_ok()
    }

    fn vouching(&self) -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, &derive(&self.0, b"", "drift-sync devices v1"))
    }

    /// A name for content that is the same on every computer of the group, and means nothing to anyone else:
    /// an attachment's hash becomes this, so the server cannot tell which known file it is.
    pub fn blind(&self, name: &str) -> String {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &derive(&self.0, b"", "drift-sync names v1"));
        crate::hex_bytes(&hmac::sign(&key, name.as_bytes()).as_ref()[..ID_LEN])
    }
}

fn vouched(device: &str, public: &[u8]) -> Vec<u8> {
    [device.as_bytes(), &[0], public].concat()
}

fn object_aad(header: &[u8], context: &Context) -> Aad<Vec<u8>> {
    let mut aad = header.to_vec();
    for field in [context.group, context.device] {
        aad.extend_from_slice(&(field.len() as u64).to_be_bytes());
        aad.extend_from_slice(field.as_bytes());
    }
    aad.extend_from_slice(&context.sequence.to_be_bytes());
    Aad::from(aad)
}

/// A four-byte length, the data, then zeros up to the next multiple of [`PAD`].
fn padded(data: &[u8]) -> Vec<u8> {
    let len = u32::try_from(data.len()).expect("a batch is far below 4 GiB");
    let mut out = len.to_be_bytes().to_vec();
    out.extend_from_slice(data);
    out.resize(out.len().div_ceil(PAD) * PAD, 0);
    out
}

fn unpadded(data: &[u8]) -> Result<&[u8], CryptoError> {
    let (len, rest) = data.split_first_chunk::<4>().ok_or(CryptoError::Tampered)?;
    rest.get(..u32::from_be_bytes(*len) as usize)
        .ok_or(CryptoError::Tampered)
}

fn decompress(data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    use std::io::Read;

    let mut out = Vec::new();
    zstd::Decoder::new(data)
        .map_err(|error| CryptoError::Compress(error.to_string()))?
        .take(MAX_PLAIN as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|error| CryptoError::Compress(error.to_string()))?;
    if out.len() > MAX_PLAIN {
        return Err(CryptoError::Tampered);
    }
    Ok(out)
}

impl DeviceKey {
    pub fn generate() -> Result<DeviceKey, CryptoError> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| CryptoError::Random)?;
        DeviceKey::from_pkcs8(pkcs8.as_ref())
    }

    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<DeviceKey, CryptoError> {
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| CryptoError::Tampered)?;
        Ok(DeviceKey {
            pair,
            pkcs8: pkcs8.to_vec(),
        })
    }

    /// The private key, for the OS keychain.
    pub fn pkcs8(&self) -> &[u8] {
        &self.pkcs8
    }

    pub fn public(&self) -> &[u8] {
        self.pair.public_key().as_ref()
    }

    /// Signs the server's login challenge, proving this computer holds its key.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.pair.sign(message).as_ref().to_vec()
    }
}

#[cfg(test)]
mod tests;
