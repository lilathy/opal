use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use zeroize::Zeroize;

use super::crypto::{self, MasterKey};
use super::types::{
    AppSettings, SecurityPreset, VaultFile, VaultPayload, VaultPhase, VaultStatus,
    VAULT_FILENAME, VAULT_FORMAT_VERSION,
};
use crate::error::OpalError;

pub struct UnlockedVault {
    pub master_key: MasterKey,
    pub payload: VaultPayload,
    pub file: VaultFile,
}

pub struct VaultService {
    path: PathBuf,
}

impl VaultService {
    pub fn new() -> Result<Self, OpalError> {
        let dir = dirs::data_dir()
            .ok_or_else(|| OpalError::Io("could not resolve app data directory".into()))?
            .join("Opal");
        fs::create_dir_all(&dir).map_err(|e| OpalError::Io(e.to_string()))?;
        Ok(Self {
            path: dir.join(VAULT_FILENAME),
        })
    }

    #[allow(dead_code)] // used by tests and future tooling
    pub fn with_path(path: PathBuf) -> Result<Self, OpalError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| OpalError::Io(e.to_string()))?;
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn status(&self, session: Option<&UnlockedVault>) -> Result<VaultStatus, OpalError> {
        if let Some(unlocked) = session {
            let s = &unlocked.payload.settings;
            return Ok(VaultStatus {
                phase: VaultPhase::Unlocked,
                failed_attempts: unlocked.file.failed_attempts,
                wipe_after_failures: unlocked.file.wipe_after_failures,
                preset: Some(unlocked.file.preset),
                has_seed: unlocked.payload.seed_mnemonic.is_some(),
                seed_backed_up: unlocked.payload.seed_backed_up,
                discreet_mode: s.discreet_mode,
                language: s.language.clone(),
                fiat: s.fiat.clone(),
                auto_lock_minutes: s.auto_lock_minutes,
                bip39_passphrase_enabled: s.bip39_passphrase_enabled,
                tor_socks: s.tor_socks.clone(),
                wipe_after_10_failures: s.wipe_after_10_failures,
                security_preset: s.security_preset,
                start_with_windows: s.start_with_windows,
                notifications_enabled: s.notifications_enabled,
            });
        }

        if !self.exists() {
            let defaults = AppSettings::default();
            return Ok(VaultStatus {
                phase: VaultPhase::NeedsCreate,
                failed_attempts: 0,
                wipe_after_failures: None,
                preset: None,
                has_seed: false,
                seed_backed_up: false,
                discreet_mode: defaults.discreet_mode,
                language: defaults.language,
                fiat: defaults.fiat,
                auto_lock_minutes: defaults.auto_lock_minutes,
                bip39_passphrase_enabled: defaults.bip39_passphrase_enabled,
                tor_socks: defaults.tor_socks,
                wipe_after_10_failures: defaults.wipe_after_10_failures,
                security_preset: defaults.security_preset,
                start_with_windows: defaults.start_with_windows,
                notifications_enabled: defaults.notifications_enabled,
            });
        }

        let file = self.read_file()?;
        Ok(VaultStatus {
            phase: VaultPhase::Locked,
            failed_attempts: file.failed_attempts,
            wipe_after_failures: file.wipe_after_failures,
            preset: Some(file.preset),
            has_seed: false,
            seed_backed_up: false,
            discreet_mode: false,
            language: file.ui_language.clone(),
            fiat: "USD".into(),
            auto_lock_minutes: 5,
            bip39_passphrase_enabled: false,
            tor_socks: None,
            wipe_after_10_failures: file.wipe_after_failures == Some(10),
            security_preset: file.preset,
            start_with_windows: false,
            notifications_enabled: true,
        })
    }

    pub fn create(
        &self,
        password: &str,
        preset: SecurityPreset,
        wipe_after_10_failures: bool,
    ) -> Result<UnlockedVault, OpalError> {
        if self.exists() {
            return Err(OpalError::VaultExists);
        }
        validate_password(password)?;

        let salt_b64 = crypto::generate_salt_b64();
        let (kek, m_cost, t_cost, p_cost) = crypto::derive_kek(password, &salt_b64, preset)?;
        let master_key = MasterKey::generate();
        let wrapped_master_key = crypto::encrypt(&kek.0, &master_key.0)?;

        let mut payload = VaultPayload::default();
        payload.settings.security_preset = preset;
        payload.settings.wipe_after_10_failures = wipe_after_10_failures;

        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| OpalError::Crypto(format!("serialize payload: {e}")))?;
        let encrypted_payload = crypto::encrypt(&master_key.0, &payload_bytes)?;

        let now = Utc::now().to_rfc3339();
        let file = VaultFile {
            version: VAULT_FORMAT_VERSION,
            kdf: "argon2id".into(),
            preset,
            m_cost,
            t_cost,
            p_cost,
            salt_b64,
            wrapped_master_key,
            payload: encrypted_payload,
            failed_attempts: 0,
            wipe_after_failures: if wipe_after_10_failures {
                Some(10)
            } else {
                None
            },
            ui_language: payload.settings.language.clone(),
            created_at: now.clone(),
            updated_at: now,
        };

        self.write_file(&file)?;

        Ok(UnlockedVault {
            master_key,
            payload,
            file,
        })
    }

    pub fn unlock(&self, password: &str) -> Result<UnlockedVault, OpalError> {
        if !self.exists() {
            return Err(OpalError::VaultMissing);
        }

        let mut file = self.read_file()?;
        let kek_result = crypto::derive_kek_with_params(
            password,
            &file.salt_b64,
            file.m_cost,
            file.t_cost,
            file.p_cost,
        );

        let kek = match kek_result {
            Ok(k) => k.0,
            Err(e) => return Err(e),
        };

        let master_key_bytes = match crypto::decrypt(&kek.0, &file.wrapped_master_key) {
            Ok(bytes) => bytes,
            Err(OpalError::InvalidPassword) => {
                file.failed_attempts = file.failed_attempts.saturating_add(1);
                let wiped = if let Some(limit) = file.wipe_after_failures {
                    file.failed_attempts >= limit
                } else {
                    false
                };
                if wiped {
                    let _ = self.wipe();
                    return Err(OpalError::VaultWiped);
                }
                self.write_file(&file)?;
                return Err(OpalError::InvalidPassword);
            }
            Err(e) => return Err(e),
        };

        if master_key_bytes.len() != 32 {
            return Err(OpalError::Crypto("master key length invalid".into()));
        }
        let mut key_arr = [0u8; 32];
        key_arr.copy_from_slice(&master_key_bytes);

        let payload_bytes = match crypto::decrypt(&key_arr, &file.payload) {
            Ok(b) => b,
            Err(OpalError::InvalidPassword) => {
                // Treat payload auth failure as wrong password / corruption
                file.failed_attempts = file.failed_attempts.saturating_add(1);
                self.write_file(&file)?;
                return Err(OpalError::InvalidPassword);
            }
            Err(e) => return Err(e),
        };

        let payload: VaultPayload = serde_json::from_slice(&payload_bytes)
            .map_err(|e| OpalError::Crypto(format!("corrupt vault payload: {e}")))?;

        file.failed_attempts = 0;
        self.write_file(&file)?;

        Ok(UnlockedVault {
            master_key: MasterKey(key_arr),
            payload,
            file,
        })
    }

    pub fn persist(&self, session: &mut UnlockedVault) -> Result<(), OpalError> {
        // Keep wipe flag in sync with settings
        session.file.wipe_after_failures = if session.payload.settings.wipe_after_10_failures {
            Some(10)
        } else {
            None
        };
        session.file.preset = session.payload.settings.security_preset;
        session.file.ui_language = session.payload.settings.language.clone();

        let payload_bytes = serde_json::to_vec(&session.payload)
            .map_err(|e| OpalError::Crypto(format!("serialize payload: {e}")))?;
        session.file.payload = crypto::encrypt(&session.master_key.0, &payload_bytes)?;
        session.file.updated_at = Utc::now().to_rfc3339();
        self.write_file(&session.file)
    }

    pub fn change_password(
        &self,
        session: &mut UnlockedVault,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), OpalError> {
        validate_password(new_password)?;

        // Verify current password by unwrapping with it
        let (current_kek, _, _, _) = crypto::derive_kek_with_params(
            current_password,
            &session.file.salt_b64,
            session.file.m_cost,
            session.file.t_cost,
            session.file.p_cost,
        )?;
        crypto::decrypt(&current_kek.0, &session.file.wrapped_master_key)?;

        let preset = session.payload.settings.security_preset;
        let salt_b64 = crypto::generate_salt_b64();
        let (new_kek, m_cost, t_cost, p_cost) =
            crypto::derive_kek(new_password, &salt_b64, preset)?;

        session.file.salt_b64 = salt_b64;
        session.file.m_cost = m_cost;
        session.file.t_cost = t_cost;
        session.file.p_cost = p_cost;
        session.file.preset = preset;
        session.file.wrapped_master_key =
            crypto::encrypt(&new_kek.0, &session.master_key.0)?;
        session.file.failed_attempts = 0;
        self.persist(session)
    }

    pub fn rewrap_for_preset(
        &self,
        session: &mut UnlockedVault,
        password: &str,
        preset: SecurityPreset,
    ) -> Result<(), OpalError> {
        validate_password(password)?;
        // Verify password first
        let (current_kek, _, _, _) = crypto::derive_kek_with_params(
            password,
            &session.file.salt_b64,
            session.file.m_cost,
            session.file.t_cost,
            session.file.p_cost,
        )?;
        crypto::decrypt(&current_kek.0, &session.file.wrapped_master_key)?;

        let salt_b64 = crypto::generate_salt_b64();
        let (new_kek, m_cost, t_cost, p_cost) = crypto::derive_kek(password, &salt_b64, preset)?;
        session.payload.settings.security_preset = preset;
        session.file.salt_b64 = salt_b64;
        session.file.m_cost = m_cost;
        session.file.t_cost = t_cost;
        session.file.p_cost = p_cost;
        session.file.preset = preset;
        session.file.wrapped_master_key =
            crypto::encrypt(&new_kek.0, &session.master_key.0)?;
        self.persist(session)
    }

    pub fn wipe(&self) -> Result<(), OpalError> {
        if self.path.exists() {
            // Best-effort overwrite then remove
            if let Ok(meta) = fs::metadata(&self.path) {
                let len = meta.len() as usize;
                let mut zeros = vec![0u8; len.min(1024 * 1024)];
                let _ = fs::write(&self.path, &zeros);
                zeros.zeroize();
            }
            fs::remove_file(&self.path).map_err(|e| OpalError::Io(e.to_string()))?;
        }
        Ok(())
    }

    fn read_file(&self) -> Result<VaultFile, OpalError> {
        let raw = fs::read_to_string(&self.path).map_err(|e| OpalError::Io(e.to_string()))?;
        let file: VaultFile = serde_json::from_str(&raw)
            .map_err(|e| OpalError::Crypto(format!("vault file corrupt: {e}")))?;
        if file.version != VAULT_FORMAT_VERSION {
            return Err(OpalError::Crypto(format!(
                "unsupported vault version {}",
                file.version
            )));
        }
        if file.kdf != "argon2id" {
            return Err(OpalError::Crypto(format!("unsupported kdf {}", file.kdf)));
        }
        Ok(file)
    }

    fn write_file(&self, file: &VaultFile) -> Result<(), OpalError> {
        let raw = serde_json::to_string_pretty(file)
            .map_err(|e| OpalError::Crypto(format!("serialize vault: {e}")))?;
        let tmp = self.path.with_extension("opal.tmp");
        fs::write(&tmp, raw.as_bytes()).map_err(|e| OpalError::Io(e.to_string()))?;
        fs::rename(&tmp, &self.path).map_err(|e| OpalError::Io(e.to_string()))?;
        Ok(())
    }
}

fn validate_password(password: &str) -> Result<(), OpalError> {
    if password.chars().count() < 8 {
        return Err(OpalError::WeakPassword(
            "password must be at least 8 characters".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_vault() -> VaultService {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("opal-test-{nanos}.opal"));
        let _ = fs::remove_file(&path);
        VaultService::with_path(path).unwrap()
    }

    #[test]
    fn create_unlock_wrong_password_and_settings() {
        let svc = temp_vault();
        let mut session = svc
            .create("correct-horse-battery", SecurityPreset::Fast, true)
            .unwrap();
        assert!(svc.exists());
        assert_eq!(session.file.wipe_after_failures, Some(10));

        session.payload.settings.language = "ru".into();
        session.payload.settings.bip39_passphrase_enabled = true;
        svc.persist(&mut session).unwrap();

        drop(session);

        assert!(matches!(
            svc.unlock("wrong-password"),
            Err(OpalError::InvalidPassword)
        ));
        let status = svc.status(None).unwrap();
        assert_eq!(status.failed_attempts, 1);
        assert_eq!(status.language, "ru");

        let unlocked = svc.unlock("correct-horse-battery").unwrap();
        assert!(unlocked.payload.settings.bip39_passphrase_enabled);
        assert_eq!(unlocked.file.failed_attempts, 0);
        let _ = fs::remove_file(svc.path());
    }

    #[test]
    fn portfolios_survive_relock() {
        use crate::vault::{PortfolioKind, PortfolioRecord};

        let svc = temp_vault();
        let mut session = svc
            .create("correct-horse-battery", SecurityPreset::Fast, false)
            .unwrap();
        session.payload.seed_mnemonic = Some(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
                .into(),
        );
        session.payload.seed_backed_up = true;
        session.payload.portfolios.push(PortfolioRecord {
            id: "p1".into(),
            name: "BTC".into(),
            kind: PortfolioKind::Software,
            chain: "btc".into(),
            created_at: "now".into(),
            account_index: 0,
            address_index: 0,
            address: Some("bc1qtest".into()),
            xmr_view_key: None,
            notes: None,
            trezor_label: None,
            address_type: Some("native_segwit".into()),
            cached_balances_json: None,
        });
        svc.persist(&mut session).unwrap();
        drop(session);

        let unlocked = svc.unlock("correct-horse-battery").unwrap();
        assert_eq!(unlocked.payload.portfolios.len(), 1);
        assert_eq!(unlocked.payload.portfolios[0].name, "BTC");
        assert_eq!(
            unlocked.payload.portfolios[0].address.as_deref(),
            Some("bc1qtest")
        );
        let _ = fs::remove_file(svc.path());
    }
}
