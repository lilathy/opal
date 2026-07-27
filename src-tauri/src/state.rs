use parking_lot::Mutex;

use crate::vault::{UnlockedVault, VaultService};

pub struct AppState {
    pub vault: VaultService,
    pub session: Mutex<Option<UnlockedVault>>,
}

impl AppState {
    pub fn new() -> Result<Self, crate::error::OpalError> {
        Ok(Self {
            vault: VaultService::new()?,
            session: Mutex::new(None),
        })
    }
}
