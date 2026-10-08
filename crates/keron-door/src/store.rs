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

impl SecretStore for KeychainStore {
    fn get(&self, account: &str) -> Result<Option<String>, DoorError> {
        let _ = (&self.service, account);
        todo!("keron-door: KeychainStore::get")
    }

    fn set(&self, account: &str, value: &str) -> Result<(), DoorError> {
        let _ = (account, value);
        todo!("keron-door: KeychainStore::set")
    }

    fn delete(&self, account: &str) -> Result<(), DoorError> {
        let _ = account;
        todo!("keron-door: KeychainStore::delete")
    }
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
