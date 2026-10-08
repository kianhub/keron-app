//! Where the registration and the refresh token are kept.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::DoorError;

/// A small secret store keyed by account name.
pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, DoorError>;
    fn set(&self, account: &str, value: &str) -> Result<(), DoorError>;
    /// Removing an account that isn't there is not an error.
    fn delete(&self, account: &str) -> Result<(), DoorError>;
}

/// Generic passwords in the login Keychain (security-framework, so no
/// secret ever goes on a command line).
pub struct KeychainStore {
    service: String,
}

impl KeychainStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

/// errSecItemNotFound.
#[cfg(target_os = "macos")]
const ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
impl SecretStore for KeychainStore {
    fn get(&self, account: &str) -> Result<Option<String>, DoorError> {
        use security_framework::passwords::get_generic_password;
        match get_generic_password(&self.service, account) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| DoorError::Store(format!("{account} isn't text"))),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(DoorError::Store(format!(
                "couldn't read {account} (OSStatus {})",
                e.code()
            ))),
        }
    }

    fn set(&self, account: &str, value: &str) -> Result<(), DoorError> {
        use security_framework::passwords::set_generic_password;
        set_generic_password(&self.service, account, value.as_bytes()).map_err(|e| {
            DoorError::Store(format!("couldn't save {account} (OSStatus {})", e.code()))
        })
    }

    fn delete(&self, account: &str) -> Result<(), DoorError> {
        use security_framework::passwords::delete_generic_password;
        match delete_generic_password(&self.service, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(DoorError::Store(format!(
                "couldn't remove {account} (OSStatus {})",
                e.code()
            ))),
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl SecretStore for KeychainStore {
    fn get(&self, _account: &str) -> Result<Option<String>, DoorError> {
        let _ = &self.service;
        Err(no_keychain())
    }

    fn set(&self, _account: &str, _value: &str) -> Result<(), DoorError> {
        Err(no_keychain())
    }

    fn delete(&self, _account: &str) -> Result<(), DoorError> {
        Err(no_keychain())
    }
}

#[cfg(not(target_os = "macos"))]
fn no_keychain() -> DoorError {
    DoorError::Store("no Keychain on this platform".into())
}

/// In memory, for tests.
#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<String>, DoorError> {
        Ok(self.values.lock().unwrap().get(account).cloned())
    }

    fn set(&self, account: &str, value: &str) -> Result<(), DoorError> {
        self.values
            .lock()
            .unwrap()
            .insert(account.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), DoorError> {
        self.values.lock().unwrap().remove(account);
        Ok(())
    }
}
