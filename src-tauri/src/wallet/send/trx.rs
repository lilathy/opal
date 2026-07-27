//! Tron native TRX + TRC-20 sends via TronGrid.

use k256::ecdsa::{RecoveryId, Signature, SigningKey};
use serde_json::{json, Value};

use crate::error::OpalError;
use crate::network::{token_contract, token_decimals_on, tron_address_to_hex, HttpCtx};
use crate::wallet::ChainId;

pub fn send_trx_native(
    http: &HttpCtx,
    sk_hex: &str,
    from: &str,
    to: &str,
    amount_trx: &str,
) -> Result<String, OpalError> {
    let mut tx = create_trx_native_unsigned(http, from, to, amount_trx)?;
    let base = http.chain_rpc(ChainId::Trx);
    sign_and_broadcast(http, &base, sk_hex, &mut tx)
}

/// Create an unsigned TRX transfer via TronGrid (for hardware signing).
pub fn create_trx_native_unsigned(
    http: &HttpCtx,
    from: &str,
    to: &str,
    amount_trx: &str,
) -> Result<Value, OpalError> {
    let sun = parse_units(amount_trx, 6)?;
    let base = http.chain_rpc(ChainId::Trx);
    let tx = http.post_json(
        &format!("{base}/wallet/createtransaction"),
        &json!({
            "owner_address": from,
            "to_address": to,
            "amount": sun,
            "visible": true
        }),
    )?;
    if tx.get("txID").and_then(|v| v.as_str()).is_none() {
        let err = tx
            .get("Error")
            .or_else(|| tx.get("message"))
            .cloned()
            .unwrap_or_else(|| json!("createtransaction failed"));
        return Err(OpalError::Io(format!("createtransaction: {err}")));
    }
    Ok(tx)
}

/// Attach an external secp256k1 signature (65 bytes r||s||v) and broadcast.
pub fn broadcast_trx_with_signature(
    http: &HttpCtx,
    mut tx: Value,
    signature: &[u8],
) -> Result<String, OpalError> {
    let base = http.chain_rpc(ChainId::Trx);
    let txid = tx
        .get("txID")
        .and_then(|v| v.as_str())
        .ok_or_else(|| OpalError::Io("missing txID".into()))?
        .to_string();
    tx.as_object_mut()
        .ok_or_else(|| OpalError::Io("tx not object".into()))?
        .insert("signature".into(), json!([hex::encode(signature)]));
    let res = http.post_json(&format!("{base}/wallet/broadcasttransaction"), &tx)?;
    let ok = res.get("result").and_then(|r| r.as_bool()).unwrap_or(false);
    if !ok {
        let msg = res
            .get("message")
            .and_then(|m| m.as_str())
            .or_else(|| res.get("code").and_then(|c| c.as_str()))
            .unwrap_or("broadcast failed");
        let decoded = hex::decode(msg)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_else(|| msg.to_string());
        return Err(OpalError::Io(format!("broadcast: {decoded}")));
    }
    Ok(res
        .get("txid")
        .and_then(|t| t.as_str())
        .unwrap_or(&txid)
        .to_string())
}

pub fn send_trx_token(
    http: &HttpCtx,
    sk_hex: &str,
    from: &str,
    to: &str,
    amount: &str,
    symbol: &str,
) -> Result<String, OpalError> {
    let contract = token_contract(ChainId::Trx, symbol)
        .ok_or_else(|| OpalError::InvalidInput(format!("token {symbol} not allowlisted")))?;
    let decimals = token_decimals_on(ChainId::Trx, symbol);
    let raw = parse_units(amount, decimals)?;
    let to_hex = tron_address_to_hex(to)?;
    let addr20 = &to_hex[2..];
    let mut parameter = format!("{addr20:0>64}");
    let mut amt = [0u8; 32];
    let b = raw.to_be_bytes();
    amt[16..].copy_from_slice(&b);
    parameter.push_str(&hex::encode(amt));

    let base = http.chain_rpc(ChainId::Trx);
    let triggered = http.post_json(
        &format!("{base}/wallet/triggersmartcontract"),
        &json!({
            "owner_address": from,
            "contract_address": contract,
            "function_selector": "transfer(address,uint256)",
            "parameter": parameter,
            "fee_limit": 100_000_000u64,
            "call_value": 0,
            "visible": true
        }),
    )?;
    let mut tx = triggered
        .get("transaction")
        .cloned()
        .ok_or_else(|| OpalError::Io(format!("triggersmartcontract: {triggered}")))?;
    if triggered
        .get("result")
        .and_then(|r| r.get("result"))
        .and_then(|r| r.as_bool())
        == Some(false)
    {
        let msg = triggered["result"]["message"].as_str().unwrap_or("failed");
        return Err(OpalError::Io(format!("trigger: {msg}")));
    }
    sign_and_broadcast(http, &base, sk_hex, &mut tx)
}

fn sign_and_broadcast(
    http: &HttpCtx,
    base: &str,
    sk_hex: &str,
    tx: &mut Value,
) -> Result<String, OpalError> {
    let txid = tx
        .get("txID")
        .and_then(|v| v.as_str())
        .ok_or_else(|| OpalError::Io("missing txID".into()))?
        .to_string();
    let hash: [u8; 32] = hex::decode(&txid)
        .map_err(|e| OpalError::Crypto(format!("txid: {e}")))?
        .try_into()
        .map_err(|_| OpalError::Crypto("txid must be 32 bytes".into()))?;

    let sk_bytes = hex::decode(sk_hex.trim_start_matches("0x"))
        .map_err(|e| OpalError::Crypto(format!("sk: {e}")))?;
    let signing = SigningKey::from_slice(&sk_bytes)
        .map_err(|e| OpalError::Crypto(format!("sk: {e}")))?;
    let (sig, recid): (Signature, RecoveryId) = signing
        .sign_prehash_recoverable(&hash)
        .map_err(|e| OpalError::Crypto(format!("sign: {e}")))?;
    let sig_bytes = sig.to_bytes();
    let mut full = Vec::with_capacity(65);
    full.extend_from_slice(&sig_bytes);
    full.push(u8::from(recid));

    tx.as_object_mut()
        .ok_or_else(|| OpalError::Io("tx not object".into()))?
        .insert("signature".into(), json!([hex::encode(full)]));

    let res = http.post_json(&format!("{base}/wallet/broadcasttransaction"), tx)?;
    let ok = res.get("result").and_then(|r| r.as_bool()).unwrap_or(false);
    if !ok {
        let msg = res
            .get("message")
            .and_then(|m| m.as_str())
            .or_else(|| res.get("code").and_then(|c| c.as_str()))
            .unwrap_or("broadcast failed");
        // TronGrid sometimes returns hex-encoded message
        let decoded = hex::decode(msg)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_else(|| msg.to_string());
        return Err(OpalError::Io(format!("broadcast: {decoded}")));
    }
    Ok(res
        .get("txid")
        .and_then(|t| t.as_str())
        .unwrap_or(&txid)
        .to_string())
}

fn parse_units(amount: &str, decimals: u32) -> Result<u128, OpalError> {
    let amount = amount.trim();
    if amount.is_empty() {
        return Err(OpalError::InvalidInput("amount required".into()));
    }
    let (whole, frac) = match amount.split_once('.') {
        Some((w, f)) => (w, f),
        None => (amount, ""),
    };
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole
            .parse()
            .map_err(|_| OpalError::InvalidInput("bad amount".into()))?
    };
    let mut frac = frac.to_string();
    if frac.len() > decimals as usize {
        frac.truncate(decimals as usize);
    }
    while frac.len() < decimals as usize {
        frac.push('0');
    }
    let frac_n: u128 = if frac.is_empty() {
        0
    } else {
        frac.parse()
            .map_err(|_| OpalError::InvalidInput("bad amount".into()))?
    };
    let base = 10u128
        .checked_pow(decimals)
        .ok_or_else(|| OpalError::InvalidInput("decimals overflow".into()))?;
    whole
        .checked_mul(base)
        .and_then(|v| v.checked_add(frac_n))
        .ok_or_else(|| OpalError::InvalidInput("amount overflow".into()))
}
