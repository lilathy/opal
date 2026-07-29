//! Blocking JSON-RPC client for `monero-wallet-rpc`.

use std::time::Duration;

use reqwest::blocking::{Client, ClientBuilder};
use serde_json::{json, Value};

use crate::error::OpalError;
use crate::network::HttpCtx;

const DEFAULT_WALLET_RPC: &str = "http://127.0.0.1:18083";
/// First wallet refresh / restore can take a long time.
const WALLET_RPC_TIMEOUT_SECS: u64 = 600;

pub fn default_wallet_rpc_url() -> &'static str {
    DEFAULT_WALLET_RPC
}

pub fn wallet_rpc_url(_http: &HttpCtx) -> String {
    DEFAULT_WALLET_RPC.to_string()
}

pub fn daemon_rpc_url(_http: &HttpCtx) -> String {
    // Public remote node (no custom RPC). Host:port form for wallet-rpc --daemon-address.
    crate::wallet::monero_runtime::public_xmr_daemon().to_string()
}

fn normalize_json_rpc_url(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.ends_with("/json_rpc") {
        b.to_string()
    } else {
        format!("{b}/json_rpc")
    }
}

/// AppData directory for Opal Monero wallet files (`…/Opal/xmr/`).
pub fn xmr_wallet_dir() -> Result<std::path::PathBuf, OpalError> {
    let base = dirs::data_dir().ok_or_else(|| {
        OpalError::Io("could not resolve system data directory for Opal/xmr wallets".into())
    })?;
    let dir = base.join("Opal").join("xmr");
    std::fs::create_dir_all(&dir)
        .map_err(|e| OpalError::Io(format!("create {}: {e}", dir.display())))?;
    Ok(dir)
}

pub fn xmr_wallet_dir_display() -> String {
    xmr_wallet_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "Opal/xmr".into())
}

fn unreachable_err(url: &str, cause: impl std::fmt::Display) -> OpalError {
    let dir = xmr_wallet_dir_display();
    OpalError::Io(format!(
        "monero-wallet-rpc is unreachable at {url} ({cause}). \
         Opal auto-starts it from %LOCALAPPDATA%\\Opal\\monero when available; \
         wallets live in \"{dir}\"."
    ))
}

pub struct XmrWalletRpc {
    client: Client,
    url: String,
}

impl XmrWalletRpc {
    pub fn from_http(http: &HttpCtx) -> Result<Self, OpalError> {
        Self::with_timeout(http, WALLET_RPC_TIMEOUT_SECS)
    }

    /// Short-timeout client for balance polls - never inherit the 600s refresh budget.
    pub fn from_http_fast(http: &HttpCtx) -> Result<Self, OpalError> {
        Self::with_timeout(http, 2)
    }

    fn with_timeout(http: &HttpCtx, secs: u64) -> Result<Self, OpalError> {
        let base = wallet_rpc_url(http);
        let url = normalize_json_rpc_url(&base);
        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(secs))
            .user_agent("OpalWallet/0.1")
            .build()
            .map_err(|e| OpalError::Io(format!("xmr wallet-rpc client: {e}")))?;
        Ok(Self { client, url })
    }

    pub fn endpoint(&self) -> &str {
        &self.url
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, OpalError> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": "0",
            "method": method,
            "params": params,
        });
        let res = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .map_err(|e| unreachable_err(&self.url, e))?;
        if !res.status().is_success() {
            return Err(unreachable_err(
                &self.url,
                format!("HTTP {}", res.status()),
            ));
        }
        let v: Value = res
            .json()
            .map_err(|e| OpalError::Io(format!("wallet-rpc json: {e}")))?;
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            return Err(OpalError::Io(format!(
                "wallet-rpc {method} error ({code}): {msg}"
            )));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn generate_from_keys(
        &self,
        filename: &str,
        address: &str,
        viewkey: &str,
        spendkey: Option<&str>,
        password: &str,
        restore_height: u64,
    ) -> Result<Value, OpalError> {
        let mut params = json!({
            "filename": filename,
            "address": address,
            "viewkey": viewkey,
            "password": password,
            "restore_height": restore_height,
            "autosave_current": true,
            "language": "English",
        });
        if let Some(sk) = spendkey.filter(|s| !s.is_empty()) {
            params["spendkey"] = json!(sk);
        }
        self.call("generate_from_keys", params)
    }

    pub fn open_wallet(&self, filename: &str, password: &str) -> Result<(), OpalError> {
        self.call(
            "open_wallet",
            json!({
                "filename": filename,
                "password": password,
                "autosave_current": true,
            }),
        )?;
        Ok(())
    }

    pub fn close_wallet(&self) -> Result<(), OpalError> {
        self.call("close_wallet", json!({ "autosave_current": true }))?;
        Ok(())
    }

    pub fn refresh(&self, start_height: Option<u64>) -> Result<Value, OpalError> {
        let params = match start_height {
            Some(h) => json!({ "start_height": h }),
            None => json!({}),
        };
        self.call("refresh", params)
    }

    pub fn set_daemon(&self, address: &str, trusted: bool) -> Result<(), OpalError> {
        self.call(
            "set_daemon",
            json!({
                "address": address,
                "trusted": trusted,
                "ssl_support": "autodetect",
            }),
        )?;
        Ok(())
    }

    /// Balance in atomic units (piconero).
    pub fn get_balance(&self, account_index: u32) -> Result<u64, OpalError> {
        let v = self.call(
            "get_balance",
            json!({ "account_index": account_index }),
        )?;
        Ok(json_u64(&v["balance"]).unwrap_or(0))
    }

    pub fn get_unlocked_balance(&self, account_index: u32) -> Result<u64, OpalError> {
        let v = self.call(
            "get_balance",
            json!({ "account_index": account_index }),
        )?;
        Ok(json_u64(&v["unlocked_balance"]).unwrap_or(0))
    }

    pub fn get_address(&self, account_index: u32) -> Result<String, OpalError> {
        let v = self.call(
            "get_address",
            json!({ "account_index": account_index }),
        )?;
        v["address"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| OpalError::Io("wallet-rpc get_address: missing address".into()))
    }

    /// Send `amount_atomic` to `to`. Returns tx hash.
    pub fn transfer(&self, to: &str, amount_atomic: u64) -> Result<String, OpalError> {
        let v = self.call(
            "transfer",
            json!({
                "destinations": [{
                    "amount": amount_atomic,
                    "address": to,
                }],
                "account_index": 0,
                "priority": 0,
                "ring_size": 16,
                "get_tx_key": true,
            }),
        )?;
        v["tx_hash"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| OpalError::Io("wallet-rpc transfer: missing tx_hash".into()))
    }

    pub fn get_transfers(&self) -> Result<Value, OpalError> {
        self.call(
            "get_transfers",
            json!({
                "in": true,
                "out": true,
                "pending": true,
                "failed": false,
                "pool": true,
                "account_index": 0,
            }),
        )
    }

    /// Unspent incoming transfers (for Trezor send construction / KI sync).
    pub fn incoming_transfers_available(&self) -> Result<Vec<XmrAvailableOut>, OpalError> {
        let v = self.call(
            "incoming_transfers",
            json!({
                "transfer_type": "available",
                "account_index": 0,
            }),
        )?;
        Ok(parse_available_outs(&v))
    }

    /// Import Trezor-computed key images so watch-only balances drop spent outs.
    /// Each entry is `(key_image_hex, signature_hex)` where signature is `c||r` (64 bytes).
    pub fn import_key_images(&self, signed: &[(String, String)]) -> Result<Value, OpalError> {
        let signed_key_images: Vec<Value> = signed
            .iter()
            .map(|(ki, sig)| {
                json!({
                    "key_image": ki,
                    "signature": sig,
                })
            })
            .collect();
        self.call(
            "import_key_images",
            json!({
                "signed_key_images": signed_key_images,
            }),
        )
    }
}

#[derive(Debug, Clone)]
pub struct XmrAvailableOut {
    pub amount: u64,
    pub global_index: u64,
    pub pubkey: String,
    pub tx_pubkey: String,
    pub internal_output_index: u64,
    pub txid: String,
}

fn parse_available_outs(v: &Value) -> Vec<XmrAvailableOut> {
    let mut out = Vec::new();
    let transfers = v["transfers"].as_array().or_else(|| v.as_array());
    let Some(arr) = transfers else {
        return out;
    };
    for t in arr {
        if t["spent"].as_bool().unwrap_or(false) {
            continue;
        }
        let amount = json_u64(&t["amount"]).unwrap_or(0);
        if amount == 0 {
            continue;
        }
        out.push(XmrAvailableOut {
            amount,
            global_index: json_u64(&t["global_index"]).unwrap_or(0),
            pubkey: t["pubkey"].as_str().unwrap_or("").to_string(),
            tx_pubkey: t["tx_pubkey"]
                .as_str()
                .or_else(|| t["key"].as_str())
                .unwrap_or("")
                .to_string(),
            internal_output_index: json_u64(&t["internal_output_index"])
                .or_else(|| json_u64(&t["output_index"]))
                .unwrap_or(0),
            txid: t["tx_hash"].as_str().unwrap_or("").to_string(),
        });
    }
    out
}

pub fn json_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().map(|n| n as u64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .or_else(|| v.as_f64().map(|f| f as u64))
}

pub fn atomic_to_xmr_string(atomic: u64) -> String {
    let xmr = atomic as f64 / 1e12;
    let s = format!("{xmr:.12}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn xmr_to_atomic(amount_xmr: &str) -> Result<u64, OpalError> {
    let v: f64 = amount_xmr
        .trim()
        .parse()
        .map_err(|_| OpalError::InvalidInput("bad XMR amount".into()))?;
    if v <= 0.0 {
        return Err(OpalError::InvalidInput("amount must be positive".into()));
    }
    if !v.is_finite() {
        return Err(OpalError::InvalidInput("bad XMR amount".into()));
    }
    let atomic = (v * 1e12).round();
    if atomic < 1.0 || atomic > u64::MAX as f64 {
        return Err(OpalError::InvalidInput("XMR amount out of range".into()));
    }
    Ok(atomic as u64)
}
