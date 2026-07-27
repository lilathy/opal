use k256::ecdsa::{RecoveryId, Signature, SigningKey};
use k256::elliptic_curve::rand_core::OsRng;
use rlp::RlpStream;
use serde_json::json;
use sha3::{Digest, Keccak256};

use crate::error::OpalError;
use crate::network::{token_contract, token_decimals_on, HttpCtx};
use crate::wallet::ChainId;

pub fn send_evm_native(
    http: &HttpCtx,
    chain: ChainId,
    sk_hex: &str,
    from: &str,
    to: &str,
    amount_eth: &str,
) -> Result<String, OpalError> {
    let value = parse_eth_to_wei(amount_eth)?;
    sign_and_send(http, chain, sk_hex, from, to, value, &[], None)
}

pub fn send_evm_token(
    http: &HttpCtx,
    chain: ChainId,
    sk_hex: &str,
    from: &str,
    to: &str,
    amount: &str,
    symbol: &str,
) -> Result<String, OpalError> {
    let contract = token_contract(chain, symbol)
        .ok_or_else(|| OpalError::InvalidInput(format!("token {symbol} not allowlisted")))?;
    let decimals = token_decimals_on(chain, symbol);
    let raw = parse_units(amount, decimals)?;
    let data = encode_erc20_transfer(to, raw)?;
    sign_and_send(http, chain, sk_hex, from, contract, 0, &data, None)
}

/// Speed-up / cancel: re-broadcast same nonce with higher fees (0 value = cancel to self).
pub fn replace_evm_native(
    http: &HttpCtx,
    chain: ChainId,
    sk_hex: &str,
    from: &str,
    to: &str,
    amount_eth: &str,
    nonce: u64,
    fee_multiplier: u128,
) -> Result<String, OpalError> {
    let value = parse_eth_to_wei(amount_eth)?;
    sign_and_send(
        http,
        chain,
        sk_hex,
        from,
        to,
        value,
        &[],
        Some((nonce, fee_multiplier)),
    )
}

fn sign_and_send(
    http: &HttpCtx,
    chain: ChainId,
    sk_hex: &str,
    from: &str,
    to: &str,
    value: u128,
    data: &[u8],
    replace: Option<(u64, u128)>,
) -> Result<String, OpalError> {
    let chain_id = chain
        .chain_id_u64()
        .ok_or_else(|| OpalError::InvalidInput("not evm".into()))?;
    let nonce = if let Some((n, _)) = replace {
        n
    } else {
        let nonce_hex = http
            .eth_rpc(chain, "eth_getTransactionCount", json!([from, "pending"]))?
            .as_str()
            .unwrap_or("0x0")
            .to_string();
        crate::network::u128_from_hex(&nonce_hex)? as u64
    };

    let tip = 1_000_000_000u128;
    let fees = http
        .eth_rpc(chain, "eth_gasPrice", json!([]))?
        .as_str()
        .map(|s| crate::network::u128_from_hex(s).unwrap_or(2_000_000_000))
        .unwrap_or(2_000_000_000);
    let mult = replace.map(|(_, m)| m).unwrap_or(2);
    let max_fee = fees.saturating_mul(mult).max(tip * mult);
    let gas_limit = if data.is_empty() { 21_000u64 } else { 100_000u64 };

    let to_bytes = decode_addr(to)?;
    let mut rlp = RlpStream::new_list(9);
    rlp.append(&chain_id);
    rlp.append(&nonce);
    rlp.append(&tip.saturating_mul(mult.min(4)));
    rlp.append(&max_fee);
    rlp.append(&gas_limit);
    rlp.append(&to_bytes.as_slice());
    rlp.append(&value);
    rlp.append(&data);
    rlp.append_list::<u8, u8>(&[]);

    let payload = rlp.out();
    let mut preimage = Vec::with_capacity(payload.len() + 1);
    preimage.push(0x02);
    preimage.extend_from_slice(&payload);
    let hash = Keccak256::digest(&preimage);

    let sk_bytes = hex::decode(sk_hex.trim_start_matches("0x"))
        .map_err(|e| OpalError::Crypto(format!("sk: {e}")))?;
    let signing = SigningKey::from_slice(&sk_bytes)
        .map_err(|e| OpalError::Crypto(format!("sk: {e}")))?;
    let (sig, recid): (Signature, RecoveryId) = signing
        .sign_prehash_recoverable(&hash)
        .map_err(|e| OpalError::Crypto(format!("sign: {e}")))?;
    let sig_bytes = sig.to_bytes();
    let r = &sig_bytes[..32];
    let s = &sig_bytes[32..];
    let y_parity = u8::from(recid) as u64;

    let mut signed = RlpStream::new_list(12);
    signed.append(&chain_id);
    signed.append(&nonce);
    signed.append(&tip.saturating_mul(mult.min(4)));
    signed.append(&max_fee);
    signed.append(&gas_limit);
    signed.append(&to_bytes.as_slice());
    signed.append(&value);
    signed.append(&data);
    signed.append_list::<u8, u8>(&[]);
    signed.append(&y_parity);
    signed.append(&r);
    signed.append(&s);

    let mut raw = Vec::new();
    raw.push(0x02);
    raw.extend_from_slice(&signed.out());
    let raw_hex = format!("0x{}", hex::encode(raw));
    http.broadcast_evm(chain, &raw_hex)
}

fn decode_addr(addr: &str) -> Result<Vec<u8>, OpalError> {
    let h = addr.trim_start_matches("0x");
    let b = hex::decode(h).map_err(|e| OpalError::InvalidInput(format!("address: {e}")))?;
    if b.len() != 20 {
        return Err(OpalError::InvalidInput("address must be 20 bytes".into()));
    }
    Ok(b)
}

fn parse_eth_to_wei(amount: &str) -> Result<u128, OpalError> {
    parse_units(amount, 18)
}

pub(crate) fn parse_units(amount: &str, decimals: u32) -> Result<u128, OpalError> {
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

pub(crate) fn encode_erc20_transfer(to: &str, amount: u128) -> Result<Vec<u8>, OpalError> {
    let mut data = vec![0xa9, 0x05, 0x9c, 0xbb]; // transfer(address,uint256)
    let mut addr = decode_addr(to)?;
    let mut padded_addr = vec![0u8; 12];
    padded_addr.append(&mut addr);
    data.extend_from_slice(&padded_addr);
    let mut amt = [0u8; 32];
    let bytes = amount.to_be_bytes();
    amt[32 - bytes.len()..].copy_from_slice(&bytes);
    // amount fits in u128 so 16 bytes
    let mut amt = [0u8; 32];
    let b = amount.to_be_bytes();
    amt[16..].copy_from_slice(&b);
    data.extend_from_slice(&amt);
    Ok(data)
}

// silence unused OsRng warning if any
#[allow(dead_code)]
fn _rng() {
    let _ = OsRng;
}
