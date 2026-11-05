//! Long-lived credentials, kept in Windows Credential Manager and never in the database.
//!
//! A Google refresh token fits the store's 2,560-byte limit. Microsoft's may not, so Phase 2
//! may need to keep one key here and encrypted tokens in the database; this module is the
//! only place that would change.

use std::sync::Once;

use keyring_core::Entry;

use crate::Result;

const SERVICE: &str = "mailfunction";

fn entry(email: &str) -> Result<Entry> {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        keyring_core::set_default_store(windows_native_keyring_store::Store::new().expect("credential store"));
    });
    Ok(Entry::new(SERVICE, email)?)
}

pub fn save(email: &str, secret: &str) -> Result<()> {
    entry(email)?.set_password(secret)?;
    Ok(())
}

pub fn load(email: &str) -> Result<String> {
    Ok(entry(email)?.get_password()?)
}

pub fn delete(email: &str) -> Result<()> {
    entry(email)?.delete_credential()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_real_store() {
        let email = format!("test-{}@mailfunction.invalid", std::process::id());
        save(&email, "token").unwrap();
        assert_eq!(load(&email).unwrap(), "token");
        delete(&email).unwrap();
        assert!(load(&email).is_err());
    }
}
