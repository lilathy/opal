use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const VAULT_FORMAT_VERSION: u32 = 1;
pub const VAULT_FILENAME: &str = "vault.opal";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityPreset {
    Fast,
    Normal,
    Paranoid,
}

impl Default for SecurityPreset {
    fn default() -> Self {
        Self::Normal
    }
}

impl SecurityPreset {
    pub fn argon2_params(self) -> (u32, u32, u32) {
        match self {
            Self::Fast => (19_456, 2, 1),
            Self::Normal => (65_536, 3, 1),
            Self::Paranoid => (262_144, 4, 1),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedBlob {
    pub nonce_b64: String,
    pub ciphertext_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultFile {
    pub version: u32,
    pub kdf: String,
    pub preset: SecurityPreset,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt_b64: String,
    pub wrapped_master_key: EncryptedBlob,
    pub payload: EncryptedBlob,
    pub failed_attempts: u32,
    pub wipe_after_failures: Option<u32>,
    #[serde(default = "default_language")]
    pub ui_language: String,
    pub created_at: String,
    pub updated_at: String,
}

fn default_language() -> String {
    "en".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub language: String,
    pub fiat: String,
    pub discreet_mode: bool,
    pub security_preset: SecurityPreset,
    pub wipe_after_10_failures: bool,
    pub bip39_passphrase_enabled: bool,
    pub tor_socks: Option<String>,
    pub auto_lock_minutes: u32,
    pub start_with_windows: bool,
    pub notifications_enabled: bool,
    /// Optional BIP39 passphrase (only used when bip39_passphrase_enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bip39_passphrase: Option<String>,
    /// Per-chain custom RPC / API base URLs.
    #[serde(default)]
    pub custom_rpc: HashMap<String, String>,
    /// Optional FixedFloat API credentials for in-app order creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixedfloat_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixedfloat_api_secret: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: "en".into(),
            fiat: "USD".into(),
            discreet_mode: false,
            security_preset: SecurityPreset::Normal,
            wipe_after_10_failures: false,
            bip39_passphrase_enabled: false,
            tor_socks: None,
            auto_lock_minutes: 5,
            start_with_windows: false,
            notifications_enabled: true,
            bip39_passphrase: None,
            custom_rpc: HashMap::new(),
            fixedfloat_api_key: None,
            fixedfloat_api_secret: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultPayload {
    pub settings: AppSettings,
    pub seed_mnemonic: Option<String>,
    #[serde(default)]
    pub seed_backed_up: bool,
    pub portfolios: Vec<PortfolioRecord>,
    pub address_book: Vec<AddressBookEntry>,
}

impl Default for VaultPayload {
    fn default() -> Self {
        Self {
            settings: AppSettings::default(),
            seed_mnemonic: None,
            seed_backed_up: false,
            portfolios: Vec::new(),
            address_book: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PortfolioKind {
    Software,
    Trezor,
    WatchOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioRecord {
    pub id: String,
    pub name: String,
    pub kind: PortfolioKind,
    /// Chain id: btc, eth, arb, base, op, sol, ltc, doge, xmr
    pub chain: String,
    pub created_at: String,
    #[serde(default)]
    pub account_index: u32,
    #[serde(default)]
    pub address_index: u32,
    /// Watch-only or cached receive address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Monero view key for watch-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xmr_view_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Trezor device id / label when kind=Trezor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trezor_label: Option<String>,
    /// BTC address style: native_segwit | taproot | legacy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_type: Option<String>,
    /// Cached balance snapshot for offline display (encrypted with vault).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_balances_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressBookEntry {
    pub id: String,
    pub label: String,
    pub chain: String,
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultPhase {
    NeedsCreate,
    Locked,
    Unlocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultStatus {
    pub phase: VaultPhase,
    pub failed_attempts: u32,
    pub wipe_after_failures: Option<u32>,
    pub preset: Option<SecurityPreset>,
    pub has_seed: bool,
    pub seed_backed_up: bool,
    pub discreet_mode: bool,
    pub language: String,
    pub fiat: String,
    pub auto_lock_minutes: u32,
    pub bip39_passphrase_enabled: bool,
    pub tor_socks: Option<String>,
    pub wipe_after_10_failures: bool,
    pub security_preset: SecurityPreset,
    pub start_with_windows: bool,
    pub notifications_enabled: bool,
}
