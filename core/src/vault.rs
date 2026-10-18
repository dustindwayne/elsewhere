//! Authenticated, atomic storage. Keys are fetched for each operation, never cached here.
use crate::Result;
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const HEADER: &[u8] = b"ELSEWHERE-VAULT\x01";
const LIMIT: u64 = 256 * 1024 * 1024;

pub trait KeyStore: Send + Sync {
    fn load(&self, identity: &str, create: bool) -> Result<Zeroizing<Vec<u8>>>;
}

impl<T: KeyStore + ?Sized> KeyStore for std::sync::Arc<T> {
    fn load(&self, identity: &str, create: bool) -> Result<Zeroizing<Vec<u8>>> {
        (**self).load(identity, create)
    }
}

pub(crate) fn restore_database(bytes: &[u8]) -> Result<rusqlite::Connection> {
    use rusqlite::{ffi, serialize::OwnedData, DatabaseName};
    let mut db = rusqlite::Connection::open_in_memory().map_err(|e| e.to_string())?;
    let size = u64::try_from(bytes.len()).map_err(|_| "Memory is too large")?;
    // SQLite takes ownership and may resize this allocation, so its allocator is required.
    let pointer = std::ptr::NonNull::new(unsafe { ffi::sqlite3_malloc64(size) }.cast::<u8>())
        .ok_or("Could not allocate memory database")?;
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.as_ptr(), bytes.len());
    }
    let data = unsafe { OwnedData::from_raw_nonnull(pointer, bytes.len()) };
    db.deserialize(DatabaseName::Main, data, false)
        .map_err(|e| e.to_string())?;
    db.execute_batch(
        "PRAGMA temp_store=MEMORY; PRAGMA journal_mode=MEMORY; PRAGMA secure_delete=ON;",
    )
    .map_err(|e| e.to_string())?;
    Ok(db)
}

pub struct SystemKeys;
impl KeyStore for SystemKeys {
    fn load(&self, identity: &str, create: bool) -> Result<Zeroizing<Vec<u8>>> {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            let entry = keyring::Entry::new("com.dustindwayne.elsewhere.memory", identity)
                .map_err(|e| format!("System key store unavailable: {e}"))?;
            match entry.get_secret() {
                Ok(secret) if secret.len() == 32 => Ok(Zeroizing::new(secret)),
                Ok(_) => Err("Invalid memory key in system key store".into()),
                Err(keyring::Error::NoEntry) if create => {
                    let mut key = Zeroizing::new(vec![0; 32]);
                    rand::rngs::OsRng.fill_bytes(&mut key);
                    entry
                        .set_secret(&key)
                        .map_err(|e| format!("Could not secure memory key: {e}"))?;
                    Ok(key)
                }
                Err(e) => Err(format!("Memory is locked: {e}")),
            }
        }
        #[cfg(target_os = "android")]
        {
            use keyring_core::api::CredentialStoreApi;
            let store = android_native_keyring_store::Store::new_with_configuration(
                &std::collections::HashMap::from([("name", "elsewhere-memory")]),
            )
            .map_err(|e| format!("Android key store unavailable: {e}"))?;
            let entry = store
                .build("com.dustindwayne.elsewhere.memory", identity, None)
                .map_err(|e| format!("Android key store unavailable: {e}"))?;
            match entry.get_secret() {
                Ok(secret) if secret.len() == 32 => Ok(Zeroizing::new(secret)),
                Ok(_) => Err("Invalid memory key in Android key store".into()),
                Err(keyring_core::Error::NoEntry) if create => {
                    let mut key = Zeroizing::new(vec![0; 32]);
                    rand::rngs::OsRng.fill_bytes(&mut key);
                    entry
                        .set_secret(&key)
                        .map_err(|e| format!("Could not secure memory key: {e}"))?;
                    Ok(key)
                }
                Err(e) => Err(format!("Memory is locked: {e}")),
            }
        }
        #[cfg(not(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "linux",
            target_os = "android"
        )))]
        {
            let _ = (identity, create);
            Err("A native secure key store is required on this platform".into())
        }
    }
}

pub struct Vault<K: KeyStore = SystemKeys> {
    path: PathBuf,
    identity: String,
    keys: K,
}

impl Vault<SystemKeys> {
    pub fn new(directory: &Path, owner: &str) -> Result<Self> {
        Self::with_keys(directory, owner, SystemKeys)
    }
}

impl<K: KeyStore> Vault<K> {
    pub fn with_keys(directory: &Path, owner: &str, keys: K) -> Result<Self> {
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let directory = directory.canonicalize().map_err(|e| e.to_string())?;
        // The owner never becomes a path component. Both owner and workspace bind the key.
        let owner_hash = format!("{:x}", Sha256::digest(owner.as_bytes()));
        let identity = format!(
            "{:x}",
            Sha256::digest(format!("{}\0{owner}", directory.display()).as_bytes())
        );
        Ok(Self {
            path: directory.join(format!("{owner_hash}.vault")),
            identity,
            keys,
        })
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn read(&self) -> Result<Zeroizing<Vec<u8>>> {
        let metadata = std::fs::metadata(&self.path).map_err(|e| e.to_string())?;
        if metadata.len() > LIMIT {
            return Err("Memory vault exceeds size limit".into());
        }
        let bytes = std::fs::read(&self.path).map_err(|e| e.to_string())?;
        if bytes.len() < HEADER.len() + 12 + 16 || !bytes.starts_with(HEADER) {
            return Err("Invalid encrypted memory vault".into());
        }
        let key = self.keys.load(&self.identity, false)?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "Invalid memory key")?;
        cipher
            .decrypt(
                Nonce::from_slice(&bytes[HEADER.len()..HEADER.len() + 12]),
                Payload {
                    msg: &bytes[HEADER.len() + 12..],
                    aad: self.identity.as_bytes(),
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| "Memory could not be authenticated".into())
    }

    pub fn write(&self, plaintext: &[u8]) -> Result<()> {
        if plaintext.len() as u64 > LIMIT - 64 {
            return Err("Memory vault exceeds size limit".into());
        }
        let key = self.keys.load(&self.identity, !self.exists())?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "Invalid memory key")?;
        let mut nonce = [0; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let encrypted = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: self.identity.as_bytes(),
                },
            )
            .map_err(|_| "Could not encrypt memory")?;
        let mut file = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())
            .map_err(|e| e.to_string())?;
        file.write_all(HEADER)
            .and_then(|_| file.write_all(&nonce))
            .and_then(|_| file.write_all(&encrypted))
            .and_then(|_| file.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        file.persist(&self.path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Creates and removes a temporary credential in the signed-in user's system key store"]
    #[cfg(target_os = "windows")]
    fn native_key_store_roundtrip() {
        let id = format!("elsewhere-test-{}", uuid::Uuid::new_v4());
        let first = SystemKeys.load(&id, true).unwrap();
        let next = SystemKeys.load(&id, false).unwrap();
        keyring::Entry::new("com.dustindwayne.elsewhere.memory", &id)
            .unwrap()
            .delete_credential()
            .unwrap();
        assert_eq!(*first, *next);
        assert!(SystemKeys.load(&id, false).is_err());
    }
    struct TestKeys(u8);
    impl KeyStore for TestKeys {
        fn load(&self, _: &str, _: bool) -> Result<Zeroizing<Vec<u8>>> {
            Ok(Zeroizing::new(vec![self.0; 32]))
        }
    }
    #[test]
    fn ciphertext_only_roundtrip_random_nonce_and_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::with_keys(dir.path(), "agent-one", TestKeys(7)).unwrap();
        let text = b"Private memory without JSON wrapping";
        vault.write(text).unwrap();
        let first = std::fs::read(&vault.path).unwrap();
        assert!(!first.windows(text.len()).any(|w| w == text));
        assert_eq!(&**vault.read().unwrap(), text);
        vault.write(text).unwrap();
        assert_ne!(first, std::fs::read(&vault.path).unwrap());
        let wrong = Vault::with_keys(dir.path(), "agent-one", TestKeys(8)).unwrap();
        assert!(wrong.read().is_err());
    }
    #[test]
    fn tampering_and_cross_owner_copy_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let a = Vault::with_keys(dir.path(), "../alice", TestKeys(7)).unwrap();
        let b = Vault::with_keys(dir.path(), "bob", TestKeys(7)).unwrap();
        a.write(b"private").unwrap();
        std::fs::copy(&a.path, &b.path).unwrap();
        assert!(b.read().is_err());
        let mut bytes = std::fs::read(&a.path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&a.path, bytes).unwrap();
        assert!(a.read().is_err());
    }
    #[test]
    fn missing_key_does_not_replace_existing_ciphertext() {
        struct Locked;
        impl KeyStore for Locked {
            fn load(&self, _: &str, _: bool) -> Result<Zeroizing<Vec<u8>>> {
                Err("locked".into())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let a = Vault::with_keys(dir.path(), "user", TestKeys(7)).unwrap();
        a.write(b"original").unwrap();
        let bytes = std::fs::read(&a.path).unwrap();
        let locked = Vault::with_keys(dir.path(), "user", Locked).unwrap();
        assert!(locked.read().is_err());
        assert!(locked.write(b"replacement").is_err());
        assert_eq!(bytes, std::fs::read(&a.path).unwrap());
    }
}
