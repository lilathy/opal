//! Monero sync / send via local `monero-wallet-rpc`.

use sha2::{Digest, Sha256};

use crate::error::OpalError;
use crate::network::{explorer_tx_url, HttpCtx, TxRow};
use crate::wallet::xmr_rpc::{
    atomic_to_xmr_string, daemon_rpc_url, xmr_to_atomic, xmr_wallet_dir, XmrWalletRpc,
};
use crate::wallet::ChainId;

/// Deterministic wallet filename for `monero-wallet-rpc --wallet-dir`.
pub fn xmr_wallet_filename(address: &str) -> String {
    let hash = Sha256::digest(address.as_bytes());
    format!("opal_{}", hex::encode(&hash[..16]))
}

fn normalize_key_hex(key: &str) -> Result<String, OpalError> {
    let s = key.trim().trim_start_matches("0x");
    let bytes = hex::decode(s).map_err(|e| OpalError::InvalidInput(format!("xmr key hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(OpalError::InvalidInput(
            "Monero key must be 32 bytes (64 hex chars)".into(),
        ));
    }
    Ok(hex::encode(bytes))
}

fn configure_daemon(rpc: &XmrWalletRpc, http: &HttpCtx) -> Result<(), OpalError> {
    let daemon = daemon_rpc_url(http);
    // Public remote nodes are untrusted.
    let _ = rpc.set_daemon(&daemon, false);
    Ok(())
}

/// Create or open a wallet file under AppData `Opal/xmr/` via wallet-rpc.
/// Pass `spend_hex = None` (or empty) for a watch-only wallet (view key only).
pub fn xmr_ensure_wallet(
    http: &HttpCtx,
    spend_hex: Option<&str>,
    view_hex: &str,
    address: &str,
    password: &str,
) -> Result<String, OpalError> {
    crate::wallet::monero_runtime::ensure_wallet_rpc_running()?;
    if address.trim().is_empty() {
        return Err(OpalError::InvalidInput("Monero address required".into()));
    }
    let view = normalize_key_hex(view_hex)?;
    let spend = match spend_hex.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => Some(normalize_key_hex(s)?),
        None => None,
    };

    let _dir = xmr_wallet_dir()?;
    let filename = xmr_wallet_filename(address);
    let rpc = XmrWalletRpc::from_http(http)?;

    match rpc.open_wallet(&filename, password) {
        Ok(()) => {
            configure_daemon(&rpc, http)?;
            let _ = rpc.refresh(None);
            return Ok(filename);
        }
        Err(e) => {
            let msg = e.to_string().to_ascii_lowercase();
            if msg.contains("unreachable") {
                return Err(e);
            }
        }
    }

    let _ = rpc.close_wallet();

    match rpc.generate_from_keys(
        &filename,
        address,
        &view,
        spend.as_deref(),
        password,
        0,
    ) {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string().to_ascii_lowercase();
            if msg.contains("already exists") || msg.contains("cannot create wallet") {
                rpc.open_wallet(&filename, password).map_err(|open_err| {
                    OpalError::Io(format!(
                        "XMR wallet file exists but open failed ({open_err}); generate: {e}"
                    ))
                })?;
            } else {
                return Err(e);
            }
        }
    }

    configure_daemon(&rpc, http)?;
    let _ = rpc.refresh(None);
    Ok(filename)
}

fn with_open_wallet<T>(
    http: &HttpCtx,
    spend_hex: Option<&str>,
    view_hex: &str,
    address: &str,
    password: &str,
    refresh: bool,
    f: impl FnOnce(&XmrWalletRpc) -> Result<T, OpalError>,
) -> Result<T, OpalError> {
    xmr_ensure_wallet(http, spend_hex, view_hex, address, password)?;
    let rpc = XmrWalletRpc::from_http(http)?;
    let filename = xmr_wallet_filename(address);
    if rpc.get_address(0).is_err() {
        rpc.open_wallet(&filename, password)?;
        configure_daemon(&rpc, http)?;
        if refresh {
            let _ = rpc.refresh(None);
        }
    } else if refresh {
        let _ = rpc.refresh(None);
    }
    f(&rpc)
}

/// Sync wallet and return balance as an XMR decimal string (from atomic units).
/// Does not spawn monero-wallet-rpc — if RPC isn't already up, returns "0" so
/// background balance polls never freeze the app for ~30s.
pub fn xmr_balance(
    http: &HttpCtx,
    spend_hex: Option<&str>,
    view_hex: &str,
    address: &str,
    password: &str,
) -> Result<String, OpalError> {
    if !crate::wallet::monero_runtime::is_wallet_rpc_ready() {
        return Ok("0".into());
    }
    // Poll path: never call refresh / ensure_wallet. Those hang for minutes
    // and were the main reason balance scrapes felt frozen.
    let _ = (spend_hex, view_hex);
    let rpc = XmrWalletRpc::from_http_fast(http)?;
    let filename = xmr_wallet_filename(address);
    if rpc.get_address(0).is_err() {
        let _ = rpc.open_wallet(&filename, password);
    }
    match rpc.get_balance(0) {
        Ok(atomic) => Ok(atomic_to_xmr_string(atomic)),
        Err(_) => Ok("0".into()),
    }
}

/// Send `amount_xmr` to `to`. Requires spend key. Returns txid.
pub fn xmr_send(
    http: &HttpCtx,
    spend_hex: &str,
    view_hex: &str,
    address: &str,
    password: &str,
    to: &str,
    amount_xmr: &str,
) -> Result<String, OpalError> {
    if spend_hex.trim().is_empty() {
        return Err(OpalError::InvalidInput(
            "watch-only Monero wallet cannot send".into(),
        ));
    }
    if to.trim().is_empty() {
        return Err(OpalError::InvalidInput("destination address required".into()));
    }
    let amount = xmr_to_atomic(amount_xmr)?;
    with_open_wallet(http, Some(spend_hex), view_hex, address, password, true, |rpc| {
        let unlocked = rpc.get_unlocked_balance(0)?;
        if unlocked < amount {
            return Err(OpalError::InvalidInput(format!(
                "insufficient unlocked XMR (have {}, need {})",
                atomic_to_xmr_string(unlocked),
                atomic_to_xmr_string(amount)
            )));
        }
        rpc.transfer(to.trim(), amount)
    })
}

/// Recent transfers as simple [`TxRow`]s.
pub fn xmr_history(
    http: &HttpCtx,
    spend_hex: Option<&str>,
    view_hex: &str,
    address: &str,
    password: &str,
) -> Result<Vec<TxRow>, OpalError> {
    with_open_wallet(http, spend_hex, view_hex, address, password, true, |rpc| {
        let v = rpc.get_transfers()?;
        let mut rows = Vec::new();

        for (key, direction, status) in [
            ("in", "in", "confirmed"),
            ("out", "out", "confirmed"),
            ("pending", "out", "pending"),
            ("pool", "in", "pending"),
        ] {
            if let Some(arr) = v[key].as_array() {
                for t in arr {
                    let txid = t["txid"].as_str().unwrap_or("").to_string();
                    if txid.is_empty() {
                        continue;
                    }
                    let atomic = crate::wallet::xmr_rpc::json_u64(&t["amount"]).unwrap_or(0);
                    let fee = crate::wallet::xmr_rpc::json_u64(&t["fee"]);
                    rows.push(TxRow {
                        txid: txid.clone(),
                        amount: atomic_to_xmr_string(atomic),
                        symbol: "XMR".into(),
                        direction: direction.into(),
                        timestamp: t["timestamp"]
                            .as_u64()
                            .or_else(|| t["timestamp"].as_i64().map(|n| n as u64))
                            .map(|n| {
                                if n >= 1_000_000_000_000 {
                                    (n / 1000).to_string()
                                } else {
                                    n.to_string()
                                }
                            })
                            .or_else(|| t["timestamp"].as_str().map(|s| s.to_string()))
                            .unwrap_or_default(),
                        status: status.into(),
                        fee: (direction == "out" && fee.unwrap_or(0) > 0)
                            .then(|| atomic_to_xmr_string(fee.unwrap_or(0))),
                        counterparty: None,
                        explorer_url: explorer_tx_url(ChainId::Xmr, &txid),
                    });
                }
            }
        }

        rows.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        rows.truncate(50);
        Ok(rows)
    })
}

/// Trezor hardware send — spend key stays on device.
pub fn xmr_send_trezor(
    http: &HttpCtx,
    view_hex: &str,
    from_address: &str,
    account: u32,
    to: &str,
    amount_xmr: &str,
) -> Result<String, OpalError> {
    use crate::trezor::{
        trezor_monero_supported, trezor_monero_sync_key_images, trezor_sign_monero_transaction,
        MoneroDestEntry, MoneroSignRequest, MoneroSourceEntry,
    };

    if !trezor_monero_supported()? {
        return Err(OpalError::InvalidInput(
            "Connect and unlock a Trezor Model T / Safe to send Monero".into(),
        ));
    }
    let amount = xmr_to_atomic(amount_xmr)?;
    if amount == 0 {
        return Err(OpalError::InvalidInput("amount must be > 0".into()));
    }
    if to.trim().is_empty() {
        return Err(OpalError::InvalidInput("destination address required".into()));
    }

    xmr_ensure_wallet(http, None, view_hex, from_address, "")?;
    let rpc = XmrWalletRpc::from_http(http)?;
    let filename = xmr_wallet_filename(from_address);
    rpc.open_wallet(&filename, "")?;
    let _ = rpc.refresh(None);
    let _ = trezor_monero_sync_key_images(http, view_hex, from_address, account);

    let unlocked = rpc.get_unlocked_balance(0)?;
    let fee_budget = 100_000_000u64;
    if unlocked < amount.saturating_add(fee_budget) {
        return Err(OpalError::InvalidInput(format!(
            "Insufficient unlocked balance (have {}, need ~{})",
            atomic_to_xmr_string(unlocked),
            atomic_to_xmr_string(amount + fee_budget)
        )));
    }

    let mut available = rpc.incoming_transfers_available()?;
    available.sort_by(|a, b| b.amount.cmp(&a.amount));
    let mut selected = Vec::new();
    let mut total = 0u64;
    for t in available {
        if total >= amount.saturating_add(fee_budget) {
            break;
        }
        if t.amount == 0 {
            continue;
        }
        total = total.saturating_add(t.amount);
        selected.push(t);
        if selected.len() == 2 {
            break;
        }
    }
    if total < amount.saturating_add(fee_budget) {
        return Err(OpalError::InvalidInput(
            "Not enough unlocked outputs to cover amount + fee (this path uses at most 2 inputs)"
                .into(),
        ));
    }

    let fee = fee_budget;
    let change_amt = total.saturating_sub(amount).saturating_sub(fee);
    let mixin = 15u32;

    let dest = decode_xmr_address(to)?;
    let change = decode_xmr_address(from_address)?;
    let daemon = DaemonRpc::new(http)?;

    let mut sources = Vec::new();
    for sel in &selected {
        let real_pub = hex::decode(sel.pubkey.trim())
            .map_err(|e| OpalError::InvalidInput(format!("out pubkey: {e}")))?;
        let tx_key = hex::decode(sel.tx_pubkey.trim())
            .map_err(|e| OpalError::InvalidInput(format!("tx pubkey: {e}")))?;
        if real_pub.len() != 32 || tx_key.len() != 32 {
            return Err(OpalError::InvalidInput(
                "bad output keys from wallet-rpc — Sync my Trezor and retry".into(),
            ));
        }
        let ring = daemon.fetch_ring(sel.global_index, mixin, &real_pub)?;
        let real_output = ring
            .iter()
            .position(|m| m.global_index == sel.global_index)
            .or_else(|| ring.iter().position(|m| m.key.dest == real_pub))
            .ok_or_else(|| OpalError::Io("real output missing from ring".into()))?
            as u64;
        sources.push(MoneroSourceEntry {
            outputs: ring,
            real_output,
            real_out_tx_key: tx_key,
            real_out_additional_tx_keys: Vec::new(),
            real_output_in_tx_index: sel.internal_output_index,
            amount: sel.amount,
            mask: vec![0u8; 32],
            subaddr_minor: 0,
        });
    }

    let path = format!("m/44'/128'/{account}'");
    let req = MoneroSignRequest {
        path,
        account,
        sources,
        destinations: vec![MoneroDestEntry {
            amount,
            spend_public_key: dest.0,
            view_public_key: dest.1,
            is_subaddress: dest.2,
        }],
        change: MoneroDestEntry {
            amount: change_amt,
            spend_public_key: change.0,
            view_public_key: change.1,
            is_subaddress: false,
        },
        fee,
        mixin,
        unlock_time: 0,
    };

    let signed = trezor_sign_monero_transaction(&req)?;
    let blob = assemble_monero_tx_blob(&signed, 0)?;
    let txid = daemon.relay_tx(&blob)?;
    let _ = rpc.refresh(None);
    Ok(txid)
}

fn decode_xmr_address(addr: &str) -> Result<(Vec<u8>, Vec<u8>, bool), OpalError> {
    use monero::util::address::{Address, AddressType};
    use std::str::FromStr;
    let a = Address::from_str(addr.trim())
        .map_err(|e| OpalError::InvalidInput(format!("invalid Monero address: {e}")))?;
    Ok((
        a.public_spend.as_bytes().to_vec(),
        a.public_view.as_bytes().to_vec(),
        matches!(a.addr_type, AddressType::SubAddress),
    ))
}

fn assemble_monero_tx_blob(
    parts: &crate::trezor::MoneroSignedParts,
    unlock_time: u64,
) -> Result<Vec<u8>, OpalError> {
    if parts.vinis.is_empty() || parts.tx_outs.is_empty() {
        return Err(OpalError::Io("Trezor returned empty vin/vout".into()));
    }
    if parts.signatures.len() != parts.vinis.len() {
        return Err(OpalError::Io("signature count mismatch".into()));
    }
    let mut sigs = parts.signatures.clone();
    if parts.opening_key.len() == 32 {
        for sig in &mut sigs {
            if let Ok(plain) = chacha_open(&parts.opening_key, sig) {
                *sig = plain;
            }
        }
    }
    let mut out = Vec::new();
    write_varint(&mut out, 2);
    write_varint(&mut out, unlock_time);
    write_varint(&mut out, parts.vinis.len() as u64);
    for v in &parts.vinis {
        out.extend_from_slice(v);
    }
    write_varint(&mut out, parts.tx_outs.len() as u64);
    for o in &parts.tx_outs {
        out.extend_from_slice(o);
    }
    write_varint(&mut out, parts.extra.len() as u64);
    out.extend_from_slice(&parts.extra);
    out.push(parts.rv_type as u8);
    write_varint(&mut out, parts.fee);
    for e in &parts.ecdh_infos {
        out.extend_from_slice(e);
    }
    for pk in &parts.out_pks {
        out.extend_from_slice(pk);
    }
    if !parts.range_proof.is_empty() {
        out.extend_from_slice(&parts.range_proof);
    }
    for s in &sigs {
        out.extend_from_slice(s);
    }
    for p in &parts.pseudo_outs {
        out.extend_from_slice(p);
    }
    Ok(out)
}

fn write_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn chacha_open(key: &[u8], blob: &[u8]) -> Result<Vec<u8>, OpalError> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{ChaCha20Poly1305, Nonce};
    if blob.len() < 12 + 16 {
        return Err(OpalError::Io("encrypted CLSAG too short".into()));
    }
    let nonce = Nonce::from_slice(&blob[..12]);
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| OpalError::Io(format!("chacha key: {e}")))?;
    cipher
        .decrypt(nonce, &blob[12..])
        .map_err(|_| OpalError::Io("CLSAG decrypt failed".into()))
}

struct DaemonRpc {
    client: reqwest::blocking::Client,
    url: String,
}

impl DaemonRpc {
    fn new(http: &HttpCtx) -> Result<Self, OpalError> {
        use crate::wallet::xmr_rpc::daemon_rpc_url;
        let base = daemon_rpc_url(http);
        let url = if base.contains("/json_rpc") {
            base
        } else {
            let host = base.trim().trim_end_matches('/');
            let host = if host.starts_with("http") {
                host.to_string()
            } else {
                format!("http://{host}")
            };
            format!("{host}/json_rpc")
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| OpalError::Io(format!("daemon client: {e}")))?;
        Ok(Self { client, url })
    }

    fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, OpalError> {
        let body = serde_json::json!({
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
            .map_err(|e| OpalError::Io(format!("daemon {method}: {e}")))?;
        let v: serde_json::Value = res
            .json()
            .map_err(|e| OpalError::Io(format!("daemon json: {e}")))?;
        if let Some(err) = v.get("error") {
            return Err(OpalError::Io(format!("daemon {method}: {err}")));
        }
        Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }

    fn fetch_ring(
        &self,
        real_index: u64,
        mixin: u32,
        real_dest: &[u8],
    ) -> Result<Vec<crate::trezor::MoneroRingMember>, OpalError> {
        use crate::trezor::{MoneroRctKey, MoneroRingMember};
        let need = mixin as u64 + 1;
        let start = real_index.saturating_sub(mixin as u64 / 2);
        let mut indices: Vec<u64> = (0..need).map(|i| start + i).collect();
        if !indices.contains(&real_index) {
            indices.push(real_index);
        }
        indices.sort_unstable();
        indices.dedup();
        let outputs: Vec<_> = indices
            .iter()
            .map(|i| serde_json::json!({ "amount": 0, "index": i }))
            .collect();
        let result = self.call(
            "get_outs",
            serde_json::json!({ "outputs": outputs, "get_txid": false }),
        )?;
        let arr = result["outs"]
            .as_array()
            .ok_or_else(|| OpalError::Io("get_outs missing outs".into()))?;
        let mut ring = Vec::new();
        for (i, o) in arr.iter().enumerate() {
            let idx = indices.get(i).copied().unwrap_or(real_index);
            let dest = hex::decode(o["key"].as_str().unwrap_or("")).unwrap_or_default();
            let commitment =
                hex::decode(o["mask"].as_str().unwrap_or("")).unwrap_or_else(|_| vec![0u8; 32]);
            if dest.len() != 32 {
                continue;
            }
            ring.push(MoneroRingMember {
                global_index: idx,
                key: MoneroRctKey { dest, commitment },
            });
        }
        if !ring.iter().any(|m| m.key.dest == real_dest) {
            ring.push(MoneroRingMember {
                global_index: real_index,
                key: MoneroRctKey {
                    dest: real_dest.to_vec(),
                    commitment: vec![0u8; 32],
                },
            });
        }
        ring.sort_by_key(|m| m.global_index);
        if ring.len() < 2 {
            return Err(OpalError::Io(
                "could not build a decoy ring from the Monero daemon — try again shortly".into(),
            ));
        }
        Ok(ring)
    }

    fn relay_tx(&self, blob: &[u8]) -> Result<String, OpalError> {
        use sha3::{Digest, Keccak256};
        let hex_blob = hex::encode(blob);
        match self.call(
            "send_raw_transaction",
            serde_json::json!({ "tx_as_hex": hex_blob, "do_not_relay": false }),
        ) {
            Ok(v) => {
                if v["status"].as_str() == Some("Failed")
                    || v.get("double_spend").and_then(|x| x.as_bool()) == Some(true)
                {
                    return Err(OpalError::Io(format!("relay rejected: {v}")));
                }
                if let Some(h) = v["tx_hash"].as_str() {
                    return Ok(h.to_string());
                }
                let mut h = Keccak256::new();
                h.update(blob);
                Ok(hex::encode(h.finalize()))
            }
            Err(_) => {
                let base = self.url.replace("/json_rpc", "");
                let res = self
                    .client
                    .post(format!("{base}/sendrawtransaction"))
                    .json(&serde_json::json!({ "tx_as_hex": hex_blob }))
                    .send()
                    .map_err(|e| OpalError::Io(format!("sendrawtransaction: {e}")))?;
                let v: serde_json::Value = res
                    .json()
                    .map_err(|e| OpalError::Io(format!("sendraw json: {e}")))?;
                if let Some(h) = v["tx_hash"].as_str().or_else(|| v["txid"].as_str()) {
                    return Ok(h.to_string());
                }
                Err(OpalError::Io(format!("broadcast failed: {v}")))
            }
        }
    }
}
