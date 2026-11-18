//! Keys at rest: the master key and where it lives, the subkeys derived from
//! it, and sealing payloads with XChaCha20-Poly1305.
//!
//! - One 256-bit master key per generation, stored by a [`KeySource`]: the
//!   login keychain on macOS (service `app.cliprust`, account `master-<gen>`),
//!   or memory in tests.
//! - HKDF-SHA256 derives the `secrets` (payloads), `dedup` (keyed checksums)
//!   and `db` (SQLCipher) subkeys.
//! - A sealed payload is `0x01 ‖ nonce(24) ‖ ciphertext ‖ tag(16)`, with the
//!   AAD `cliprust/v1/<item_id>/<position>/<format>`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{Tag, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

/// The Keychain service of the master keys.
pub const SERVICE: &str = "app.cliprust";
const ENVELOPE_V1: u8 = 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const RECOVERY_PREFIX: &str = "cliprust-key";

pub type MasterKey = Zeroizing<[u8; 32]>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The key is not in the store (`errSecItemNotFound`).
    #[error("ClipRust's key is missing from the Keychain")]
    Missing,
    /// The user cancelled, or the keychain is locked or denied access. Not
    /// the same as missing: nothing may be deleted or replaced because of it.
    #[error("the Keychain did not hand out ClipRust's key ({0})")]
    Unavailable(String),
    /// `add` never replaces a key.
    #[error("a ClipRust key with this generation is already in the Keychain")]
    Exists,
    #[error("Keychain error: {0}")]
    Other(String),
}

/// Where master keys live. Keys are only ever added or deleted, never replaced.
pub trait KeySource: Send + Sync {
    fn load(&self, generation: u32) -> Result<MasterKey, KeyError>;
    /// Adds a new key; fails with [`KeyError::Exists`] instead of replacing one.
    fn add(&self, generation: u32, key: &[u8; 32]) -> Result<(), KeyError>;
    /// Deletes a key; a missing key is not an error.
    fn delete(&self, generation: u32) -> Result<(), KeyError>;
    /// The generations that have a key, in any order.
    fn generations(&self) -> Result<Vec<u32>, KeyError>;
}

/// Keys held in memory only: for tests, never for real data.
#[derive(Default)]
pub struct MemoryKeys {
    keys: Mutex<HashMap<u32, MasterKey>>,
}

impl MemoryKeys {
    fn keys(&self) -> std::sync::MutexGuard<'_, HashMap<u32, MasterKey>> {
        self.keys.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl KeySource for MemoryKeys {
    fn load(&self, generation: u32) -> Result<MasterKey, KeyError> {
        self.keys()
            .get(&generation)
            .cloned()
            .ok_or(KeyError::Missing)
    }

    fn add(&self, generation: u32, key: &[u8; 32]) -> Result<(), KeyError> {
        let mut keys = self.keys();
        if keys.contains_key(&generation) {
            return Err(KeyError::Exists);
        }
        keys.insert(generation, Zeroizing::new(*key));
        Ok(())
    }

    fn delete(&self, generation: u32) -> Result<(), KeyError> {
        self.keys().remove(&generation);
        Ok(())
    }

    fn generations(&self) -> Result<Vec<u32>, KeyError> {
        Ok(self.keys().keys().copied().collect())
    }
}

/// The platform's key store: the login keychain on macOS, none elsewhere
/// (keys then stay unencrypted, as before). Tests always get a fresh
/// in-memory store, so they never touch the real keychain.
pub fn default_keys() -> Option<Arc<dyn KeySource>> {
    #[cfg(test)]
    {
        Some(Arc::new(MemoryKeys::default()))
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        static KEYCHAIN: std::sync::OnceLock<Arc<keychain::Keychain>> = std::sync::OnceLock::new();
        Some(KEYCHAIN.get_or_init(Arc::default).clone())
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
pub mod keychain {
    //! Generic-password items in the login (file-based) keychain.

    use std::collections::HashMap;
    use std::sync::Mutex;

    use core_foundation::data::CFData;
    use security_framework::item::{
        ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, Limit,
    };
    use security_framework::passwords::{delete_generic_password, get_generic_password};
    use zeroize::{Zeroize, Zeroizing};

    use super::{KeyError, KeySource, MasterKey, SERVICE};

    const NOT_FOUND: i32 = -25300;
    const DUPLICATE: i32 = -25299;
    /// User cancelled, interaction not allowed (locked), auth failed, no keychain.
    const UNAVAILABLE: [i32; 5] = [-128, -25308, -25293, -25294, -25307];

    /// The login keychain, with each key read at most once per process.
    #[derive(Default)]
    pub struct Keychain {
        cache: Mutex<HashMap<u32, MasterKey>>,
    }

    fn account(generation: u32) -> String {
        format!("master-{generation}")
    }

    fn error(e: security_framework::base::Error) -> KeyError {
        match e.code() {
            NOT_FOUND => KeyError::Missing,
            DUPLICATE => KeyError::Exists,
            c if UNAVAILABLE.contains(&c) => KeyError::Unavailable(e.to_string()),
            _ => KeyError::Other(e.to_string()),
        }
    }

    impl Keychain {
        fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<u32, MasterKey>> {
            self.cache.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    impl KeySource for Keychain {
        fn load(&self, generation: u32) -> Result<MasterKey, KeyError> {
            if let Some(key) = self.cache().get(&generation) {
                return Ok(key.clone());
            }
            let mut bytes = get_generic_password(SERVICE, &account(generation)).map_err(error)?;
            let key = <[u8; 32]>::try_from(bytes.as_slice())
                .map(Zeroizing::new)
                .map_err(|_| KeyError::Other("the stored key has the wrong length".into()));
            bytes.zeroize();
            let key = key?;
            self.cache().insert(generation, key.clone());
            Ok(key)
        }

        fn add(&self, generation: u32, key: &[u8; 32]) -> Result<(), KeyError> {
            let mut options = ItemAddOptions::new(ItemAddValue::Data {
                class: ItemClass::generic_password(),
                data: CFData::from_buffer(key),
            });
            options
                .set_service(SERVICE)
                .set_account_name(account(generation))
                .set_label("ClipRust key")
                .set_description("Encrypts the API keys ClipRust stores");
            options.add().map_err(error)?;
            self.cache().insert(generation, Zeroizing::new(*key));
            Ok(())
        }

        fn delete(&self, generation: u32) -> Result<(), KeyError> {
            self.cache().remove(&generation);
            match delete_generic_password(SERVICE, &account(generation)).map_err(error) {
                Err(KeyError::Missing) => Ok(()),
                other => other,
            }
        }

        fn generations(&self) -> Result<Vec<u32>, KeyError> {
            // Attributes only, so listing never needs the keychain's permission.
            let found = ItemSearchOptions::new()
                .class(ItemClass::generic_password())
                .service(SERVICE)
                .load_attributes(true)
                .limit(Limit::All)
                .search();
            let results = match found.map_err(error) {
                Err(KeyError::Missing) => return Ok(Vec::new()),
                other => other?,
            };
            Ok(results
                .iter()
                .filter_map(|r| r.simplify_dict())
                .filter_map(|d| d.get("acct")?.strip_prefix("master-")?.parse().ok())
                .collect())
        }
    }
}

/// A payload failed to open: tampered, moved to another row, or a wrong key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a sealed payload failed its integrity check")]
pub struct OpenError;

/// The keys derived from one master key.
pub struct Subkeys {
    secrets: Zeroizing<[u8; 32]>,
    dedup: Zeroizing<[u8; 32]>,
    db: Zeroizing<[u8; 32]>,
}

impl Subkeys {
    pub fn derive(master: &[u8; 32]) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(b"cliprust/v1"), master);
        let expand = |info: &[u8]| {
            let mut out = Zeroizing::new([0u8; 32]);
            hk.expand(info, out.as_mut())
                .expect("32 bytes is a valid HKDF-SHA256 length");
            out
        };
        Self {
            secrets: expand(b"secrets"),
            dedup: expand(b"dedup"),
            db: expand(b"db"),
        }
    }

    /// Encrypts `plain` (already compressed) for one stored representation.
    pub fn seal(&self, aad: &str, plain: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).expect("the OS random number generator failed");
        let mut out = Vec::with_capacity(1 + NONCE_LEN + plain.len() + TAG_LEN);
        out.push(ENVELOPE_V1);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(plain);
        let tag = self
            .cipher()
            .encrypt_inout_detached(
                &XNonce::from(nonce),
                aad.as_bytes(),
                (&mut out[1 + NONCE_LEN..]).into(),
            )
            .expect("payloads are far below the XChaCha20 limit");
        out.extend_from_slice(&tag);
        out
    }

    /// Decrypts what [`Self::seal`] produced for the same `aad`.
    pub fn open(&self, aad: &str, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, OpenError> {
        if sealed.len() < 1 + NONCE_LEN + TAG_LEN || sealed[0] != ENVELOPE_V1 {
            return Err(OpenError);
        }
        let (head, tag) = sealed.split_at(sealed.len() - TAG_LEN);
        let nonce = XNonce::try_from(&head[1..1 + NONCE_LEN]).map_err(|_| OpenError)?;
        let tag = Tag::try_from(tag).map_err(|_| OpenError)?;
        let mut buf = Zeroizing::new(head[1 + NONCE_LEN..].to_vec());
        self.cipher()
            .decrypt_inout_detached(&nonce, aad.as_bytes(), buf.as_mut_slice().into(), &tag)
            .map_err(|_| OpenError)?;
        Ok(buf)
    }

    /// The dedup checksum of a key item: HMAC-SHA256(dedup, kind ‖ 0 ‖ rep0),
    /// so a guessed value cannot be checked against the file.
    pub fn checksum(&self, kind: &str, rep0: &[u8]) -> i64 {
        let mut mac = self.mac();
        mac.update(kind.as_bytes());
        mac.update(&[0]);
        mac.update(rep0);
        let tag = mac.finalize().into_bytes();
        i64::from_be_bytes(tag[..8].try_into().expect("HMAC-SHA256 is 32 bytes"))
    }

    /// A tag stored next to the data, telling a wrong key from corrupt data.
    pub fn check_tag(&self) -> String {
        let mut mac = self.mac();
        mac.update(b"cliprust/v1/key-check");
        hex(&mac.finalize().into_bytes()[..16])
    }

    /// The SQLCipher raw key literal, `x'<64 hex>'` (no key derivation).
    pub fn db_key(&self) -> Zeroizing<String> {
        let mut s = Zeroizing::new(String::with_capacity(67));
        s.push_str("x'");
        s.push_str(&hex(self.db.as_ref()));
        s.push('\'');
        s
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new_from_slice(self.secrets.as_ref()).expect("32-byte key")
    }

    fn mac(&self) -> Hmac<Sha256> {
        <Hmac<Sha256> as KeyInit>::new_from_slice(self.dedup.as_ref()).expect("any key length")
    }
}

/// Binds a sealed payload to its row, so ciphertext cannot be swapped between rows.
pub fn aad(item_id: i64, position: i64, format: &str) -> String {
    format!("cliprust/v1/{item_id}/{position}/{format}")
}

pub fn new_master() -> MasterKey {
    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::fill(key.as_mut()).expect("the OS random number generator failed");
    key
}

/// `cliprust-key-<gen>-<64 hex>`: the master key, for a password manager.
pub fn recovery_code(generation: u32, master: &[u8; 32]) -> Zeroizing<String> {
    let mut s = Zeroizing::new(String::with_capacity(90));
    let _ = write!(s, "{RECOVERY_PREFIX}-{generation}-");
    s.push_str(&hex(master));
    s
}

/// Reads a recovery code, ignoring case and whitespace.
pub fn parse_recovery_code(code: &str) -> Option<(u32, MasterKey)> {
    let mut clean: Zeroizing<String> = Zeroizing::new(
        code.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase(),
    );
    let parsed = (|| {
        let rest = clean.strip_prefix(RECOVERY_PREFIX)?.strip_prefix('-')?;
        let (generation, key_hex) = rest.split_once('-')?;
        let generation: u32 = generation.parse().ok().filter(|g| *g > 0)?;
        if key_hex.len() != 64 {
            return None;
        }
        let mut key = Zeroizing::new([0u8; 32]);
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(key_hex.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some((generation, key))
    })();
    clean.zeroize();
    parsed
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_payloads_bind_to_their_row_and_codes_round_trip() {
        let master = new_master();
        let keys = Subkeys::derive(&master);
        let sealed = keys.seal(&aad(1, 0, "text"), b"sk-live-value");
        assert_eq!(
            keys.open(&aad(1, 0, "text"), &sealed).unwrap().as_slice(),
            b"sk-live-value"
        );
        assert_eq!(keys.open(&aad(2, 0, "text"), &sealed), Err(OpenError));
        assert_eq!(keys.open(&aad(1, 1, "text"), &sealed), Err(OpenError));
        let other = Subkeys::derive(&new_master());
        assert_eq!(other.open(&aad(1, 0, "text"), &sealed), Err(OpenError));
        assert_ne!(keys.check_tag(), other.check_tag());
        assert_ne!(keys.checksum("text", b"a"), other.checksum("text", b"a"));

        let code = recovery_code(7, &master);
        let (generation, back) = parse_recovery_code(&code.to_uppercase()).unwrap();
        assert_eq!((generation, *back), (7, *master));
        assert!(parse_recovery_code("cliprust-key-7-00").is_none());
    }
}
