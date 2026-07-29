//! Solana native + SPL send (legacy messages, ed25519_dalek signing).
//!
//! RPC URL: uses `HttpCtx::chain_rpc(ChainId::Sol)` - do not hardcode endpoints.
//! (`rpc_url` is private on HttpCtx; `chain_rpc` is the public wrapper.)

use base64::Engine;
use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::error::OpalError;
use crate::network::{token_contract, token_decimals, FeePreset, HttpCtx};
use crate::wallet::ChainId;

/// Microlamports per compute unit for [`FeePreset::Priority`].
const PRIORITY_MICROLAMPORTS: u64 = 100_000;

/// System Program: `11111111111111111111111111111111`
const SYSTEM_PROGRAM: [u8; 32] = [0u8; 32];

/// Token Program: `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`
fn token_program_id() -> [u8; 32] {
    decode_pubkey_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
}

/// Associated Token Program: `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`
fn associated_token_program_id() -> [u8; 32] {
    decode_pubkey_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
}

/// Compute Budget: `ComputeBudget111111111111111111111111111111`
fn compute_budget_program_id() -> [u8; 32] {
    decode_pubkey_const("ComputeBudget111111111111111111111111111111")
}

fn decode_pubkey_const(s: &str) -> [u8; 32] {
    let v = bs58::decode(s)
        .into_vec()
        .expect("well-known solana program id");
    assert_eq!(v.len(), 32);
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    out
}

/// Builds and broadcasts a legacy Solana system transfer.
/// When `send_max` is true, spends `balance - getFeeForMessage` so the fee
/// payer can still pay (and ends at ~0 lamports).
pub fn send_sol_native(
    http: &HttpCtx,
    sk_hex: &str,
    from: &str,
    to: &str,
    amount_sol: &str,
    fee_preset: FeePreset,
    send_max: bool,
) -> Result<String, OpalError> {
    let (signing, from_pk) = load_signer(sk_hex, from)?;
    let to_pk = decode_pubkey(to)?;
    let recent = fetch_recent_blockhash(http)?;
    let priority = matches!(fee_preset, FeePreset::Priority);
    let url = sol_url(http);

    // Always size the transfer so amount + fee fits the live balance. This
    // covers Max (send_max) and the common case of typing the full balance.
    let bal = sol_get_balance_lamports(http, &url, &from_pk)?;
    let requested = if send_max {
        bal.saturating_sub(5_000).max(1)
    } else {
        parse_sol_to_lamports(amount_sol)?
    };
    let probe = requested.min(bal.saturating_sub(5_000).max(1));
    let probe_msg = build_native_transfer_message(&from_pk, &to_pk, &recent, probe, priority);
    let fee = sol_fee_for_message(http, &url, &probe_msg)?
        .unwrap_or(5_000)
        .saturating_add(if priority { 5_000 } else { 0 });
    let max_send = bal.saturating_sub(fee);
    if max_send == 0 {
        return Err(OpalError::InvalidInput(
            "insufficient SOL for amount plus network fee".into(),
        ));
    }
    let lamports = if send_max || requested > max_send {
        max_send
    } else {
        requested
    };

    let message = build_native_transfer_message(&from_pk, &to_pk, &recent, lamports, priority);
    sign_and_broadcast(http, &signing, &message)
}

fn build_native_transfer_message(
    from_pk: &[u8; 32],
    to_pk: &[u8; 32],
    recent: &[u8; 32],
    lamports: u64,
    priority: bool,
) -> Vec<u8> {
    let compute_budget = compute_budget_program_id();
    let mut keys: Vec<[u8; 32]> = vec![*from_pk, *to_pk, SYSTEM_PROGRAM];
    if priority {
        keys.push(compute_budget);
    }
    let num_readonly_unsigned = if priority { 2u8 } else { 1u8 };

    let mut ixs: Vec<CompiledIx> = Vec::new();
    if priority {
        ixs.push(ix_set_compute_unit_price(
            (keys.len() - 1) as u8,
            PRIORITY_MICROLAMPORTS,
        ));
    }
    ixs.push(CompiledIx {
        program_id_index: 2, // system
        accounts: vec![0, 1], // from, to
        data: {
            let mut d = Vec::with_capacity(12);
            d.extend_from_slice(&2u32.to_le_bytes()); // SystemInstruction::Transfer
            d.extend_from_slice(&lamports.to_le_bytes());
            d
        },
    });

    encode_legacy_message(1, 0, num_readonly_unsigned, &keys, recent, &ixs)
}

/// Build an unsigned native transfer message for hardware signing.
pub fn build_sol_native_message(
    http: &HttpCtx,
    from: &str,
    to: &str,
    amount_sol: &str,
    fee_preset: FeePreset,
) -> Result<Vec<u8>, OpalError> {
    let lamports = parse_sol_to_lamports(amount_sol)?;
    let from_pk = decode_pubkey(from)?;
    let to_pk = decode_pubkey(to)?;
    let recent = fetch_recent_blockhash(http)?;
    let priority = matches!(fee_preset, FeePreset::Priority);
    Ok(build_native_transfer_message(
        &from_pk, &to_pk, &recent, lamports, priority,
    ))
}

/// Broadcast a Solana tx signed externally (e.g. Trezor).
pub fn broadcast_sol_with_signature(
    http: &HttpCtx,
    signature: &[u8],
    message: &[u8],
) -> Result<String, OpalError> {
    if signature.len() != 64 {
        return Err(OpalError::InvalidInput(
            "Solana signature must be 64 bytes".into(),
        ));
    }
    let mut tx = Vec::with_capacity(1 + 64 + message.len());
    push_compact_u16(&mut tx, 1);
    tx.extend_from_slice(signature);
    tx.extend_from_slice(message);

    let b64 = base64::engine::general_purpose::STANDARD.encode(&tx);
    let url = sol_url(http);
    let sent = http.post_json(
        &url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [b64, {"encoding": "base64", "preflightCommitment": "confirmed", "maxRetries": 3}]
        }),
    )?;
    if let Some(err) = sent.get("error") {
        return Err(OpalError::Io(format!("sol send: {err}")));
    }
    sent["result"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| OpalError::Io("no signature".into()))
}

/// SPL USDC/USDT transfer via ATA; creates destination ATA when missing.
pub fn send_sol_token(
    http: &HttpCtx,
    sk_hex: &str,
    from: &str,
    to: &str,
    amount: &str,
    symbol: &str,
) -> Result<String, OpalError> {
    let mint_str = token_contract(ChainId::Sol, symbol)
        .ok_or_else(|| OpalError::InvalidInput(format!("token {symbol} not allowlisted on sol")))?;
    let decimals = token_decimals(symbol) as u8;
    let raw_amount = parse_token_amount(amount, decimals)?;
    let (signing, from_pk) = load_signer(sk_hex, from)?;
    let to_pk = decode_pubkey(to)?;
    let mint = decode_pubkey(mint_str)?;
    let token_program = token_program_id();
    let ata_program = associated_token_program_id();

    let from_ata = derive_associated_token_address(&from_pk, &token_program, &mint)?;
    let to_ata = derive_associated_token_address(&to_pk, &token_program, &mint)?;

    if !account_exists(http, &bs58::encode(from_ata).into_string())? {
        return Err(OpalError::InvalidInput(
            "sender associated token account missing - fund/create ATA first".into(),
        ));
    }
    let need_create_dest = !account_exists(http, &bs58::encode(to_ata).into_string())?;

    let recent = fetch_recent_blockhash(http)?;

    // Keys:
    // 0 from (signer, writable) - fee payer + transfer authority
    // 1 from_ata (writable)
    // 2 to_ata (writable)
    // then readonly: [to?, mint, system?, token, ata?] depending on create
    let mut keys: Vec<[u8; 32]> = vec![from_pk, from_ata, to_ata];
    let mut to_idx = None;
    let mint_idx;
    let mut system_idx = None;
    let token_idx;
    let mut ata_prog_idx = None;

    if need_create_dest {
        to_idx = Some(keys.len() as u8);
        keys.push(to_pk);
        mint_idx = keys.len() as u8;
        keys.push(mint);
        system_idx = Some(keys.len() as u8);
        keys.push(SYSTEM_PROGRAM);
        token_idx = keys.len() as u8;
        keys.push(token_program);
        ata_prog_idx = Some(keys.len() as u8);
        keys.push(ata_program);
    } else {
        mint_idx = keys.len() as u8;
        keys.push(mint);
        token_idx = keys.len() as u8;
        keys.push(token_program);
    }

    let num_readonly_unsigned = (keys.len() - 3) as u8; // first 3 are writable (1 signed + 2 unsigned)

    let mut ixs: Vec<CompiledIx> = Vec::new();
    if need_create_dest {
        ixs.push(CompiledIx {
            program_id_index: ata_prog_idx.expect("ata program"),
            accounts: vec![
                0, // funder
                2, // ata
                to_idx.expect("to"),
                mint_idx,
                system_idx.expect("system"),
                token_idx,
            ],
            data: Vec::new(), // Create
        });
    }
    // TokenInstruction::TransferChecked = 12
    let mut transfer_data = Vec::with_capacity(10);
    transfer_data.push(12u8);
    transfer_data.extend_from_slice(&raw_amount.to_le_bytes());
    transfer_data.push(decimals);
    ixs.push(CompiledIx {
        program_id_index: token_idx,
        accounts: vec![1, mint_idx, 2, 0], // source, mint, dest, authority
        data: transfer_data,
    });

    let message = encode_legacy_message(1, 0, num_readonly_unsigned, &keys, &recent, &ixs);
    sign_and_broadcast(http, &signing, &message)
}

// --- RPC helpers -----------------------------------------------------------------

fn sol_url(http: &HttpCtx) -> String {
    http.chain_rpc(ChainId::Sol)
}

/// Balance from the same RPC we broadcast on (avoids race vs multi-RPC scrape).
fn sol_get_balance_lamports(
    http: &HttpCtx,
    url: &str,
    pubkey: &[u8; 32],
) -> Result<u64, OpalError> {
    let address = bs58::encode(pubkey).into_string();
    let v = http.post_json(
        url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getBalance",
            "params": [address, {"commitment": "confirmed"}]
        }),
    )?;
    if let Some(err) = v.get("error") {
        return Err(OpalError::Io(format!("sol getBalance: {err}")));
    }
    v["result"]["value"]
        .as_u64()
        .ok_or_else(|| OpalError::Io("sol getBalance: missing value".into()))
}

fn sol_fee_for_message(
    http: &HttpCtx,
    url: &str,
    message: &[u8],
) -> Result<Option<u64>, OpalError> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(message);
    let v = http.post_json(
        url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getFeeForMessage",
            "params": [b64, {"commitment": "confirmed"}]
        }),
    )?;
    if let Some(err) = v.get("error") {
        return Err(OpalError::Io(format!("getFeeForMessage: {err}")));
    }
    // null when blockhash expired - caller falls back to 5000
    Ok(v["result"]["value"].as_u64())
}

fn fetch_recent_blockhash(http: &HttpCtx) -> Result<[u8; 32], OpalError> {
    let url = sol_url(http);
    let bh = http.post_json(
        &url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getLatestBlockhash",
            "params": [{"commitment": "confirmed"}]
        }),
    )?;
    if let Some(err) = bh.get("error") {
        return Err(OpalError::Io(format!("getLatestBlockhash: {err}")));
    }
    let blockhash_str = bh["result"]["value"]["blockhash"]
        .as_str()
        .ok_or_else(|| OpalError::Io("no blockhash".into()))?;
    let recent = bs58::decode(blockhash_str)
        .into_vec()
        .map_err(|e| OpalError::Io(format!("blockhash decode: {e}")))?;
    if recent.len() != 32 {
        return Err(OpalError::Io("bad blockhash length".into()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&recent);
    Ok(out)
}

fn account_exists(http: &HttpCtx, pubkey: &str) -> Result<bool, OpalError> {
    let url = sol_url(http);
    let v = http.post_json(
        &url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [pubkey, {"encoding": "base64"}]
        }),
    )?;
    if let Some(err) = v.get("error") {
        return Err(OpalError::Io(format!("getAccountInfo: {err}")));
    }
    Ok(!v["result"]["value"].is_null())
}

fn sign_and_broadcast(
    http: &HttpCtx,
    signing: &SigningKey,
    message: &[u8],
) -> Result<String, OpalError> {
    let signature = signing.sign(message);
    let mut tx = Vec::with_capacity(1 + 64 + message.len());
    push_compact_u16(&mut tx, 1);
    tx.extend_from_slice(&signature.to_bytes());
    tx.extend_from_slice(message);

    let b64 = base64::engine::general_purpose::STANDARD.encode(&tx);
    let url = sol_url(http);
    let sent = http.post_json(
        &url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [b64, {"encoding": "base64", "preflightCommitment": "confirmed", "maxRetries": 3}]
        }),
    )?;
    if let Some(err) = sent.get("error") {
        return Err(OpalError::Io(format!("sol send: {err}")));
    }
    sent["result"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| OpalError::Io("no signature".into()))
}

// --- Keys / amounts --------------------------------------------------------------

fn load_signer(sk_hex: &str, from: &str) -> Result<(SigningKey, [u8; 32]), OpalError> {
    let sk_bytes = hex::decode(sk_hex.trim_start_matches("0x"))
        .map_err(|e| OpalError::Crypto(e.to_string()))?;
    if sk_bytes.len() != 32 {
        return Err(OpalError::Crypto("sol key must be 32 bytes".into()));
    }
    let mut sk_arr = [0u8; 32];
    sk_arr.copy_from_slice(&sk_bytes);
    let signing = SigningKey::from_bytes(&sk_arr);
    let from_pk = decode_pubkey(from)?;
    if signing.verifying_key().as_bytes() != &from_pk {
        return Err(OpalError::Crypto("key/address mismatch".into()));
    }
    Ok((signing, from_pk))
}

fn decode_pubkey(s: &str) -> Result<[u8; 32], OpalError> {
    let bytes = bs58::decode(s.trim())
        .into_vec()
        .map_err(|e| OpalError::InvalidInput(format!("pubkey: {e}")))?;
    if bytes.len() != 32 {
        return Err(OpalError::InvalidInput("bad pubkey length".into()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn parse_sol_to_lamports(amount: &str) -> Result<u64, OpalError> {
    parse_decimal_to_u64(amount, 9)
}

fn parse_token_amount(amount: &str, decimals: u8) -> Result<u64, OpalError> {
    parse_decimal_to_u64(amount, decimals)
}

fn parse_decimal_to_u64(amount: &str, decimals: u8) -> Result<u64, OpalError> {
    let amount = amount.trim();
    if amount.is_empty() {
        return Err(OpalError::InvalidInput("amount required".into()));
    }
    let (whole, frac) = match amount.split_once('.') {
        Some((w, f)) => (w, f),
        None => (amount, ""),
    };
    if whole.starts_with('-') {
        return Err(OpalError::InvalidInput("amount must be positive".into()));
    }
    let whole_n: u128 = if whole.is_empty() {
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
        .checked_pow(decimals as u32)
        .ok_or_else(|| OpalError::InvalidInput("decimals overflow".into()))?;
    let total = whole_n
        .checked_mul(base)
        .and_then(|v| v.checked_add(frac_n))
        .ok_or_else(|| OpalError::InvalidInput("amount overflow".into()))?;
    if total == 0 {
        return Err(OpalError::InvalidInput("amount must be positive".into()));
    }
    u64::try_from(total).map_err(|_| OpalError::InvalidInput("amount too large".into()))
}

// --- ATA / PDA -------------------------------------------------------------------

/// `findProgramAddress([wallet, TOKEN_PROGRAM_ID, mint], ASSOCIATED_TOKEN_PROGRAM_ID)`
fn derive_associated_token_address(
    wallet: &[u8; 32],
    token_program: &[u8; 32],
    mint: &[u8; 32],
) -> Result<[u8; 32], OpalError> {
    let ata_program = associated_token_program_id();
    find_program_address(&[wallet.as_slice(), token_program.as_slice(), mint.as_slice()], &ata_program)
}

fn find_program_address(seeds: &[&[u8]], program_id: &[u8; 32]) -> Result<[u8; 32], OpalError> {
    for bump in (0..=255u8).rev() {
        let bump_slice = [bump];
        let mut with_bump: Vec<&[u8]> = seeds.to_vec();
        with_bump.push(&bump_slice);
        if let Ok(addr) = create_program_address(&with_bump, program_id) {
            return Ok(addr);
        }
    }
    Err(OpalError::Crypto("unable to find program address".into()))
}

fn create_program_address(seeds: &[&[u8]], program_id: &[u8; 32]) -> Result<[u8; 32], ()> {
    for seed in seeds {
        if seed.len() > 32 {
            return Err(());
        }
    }
    let mut hasher = Sha256::new();
    for seed in seeds {
        hasher.update(seed);
    }
    hasher.update(program_id);
    hasher.update(b"ProgramDerivedAddress");
    let hash: [u8; 32] = hasher.finalize().into();
    if is_on_curve(&hash) {
        return Err(());
    }
    Ok(hash)
}

fn is_on_curve(bytes: &[u8; 32]) -> bool {
    CompressedEdwardsY(*bytes).decompress().is_some()
}

// --- Legacy message encoding -----------------------------------------------------

struct CompiledIx {
    program_id_index: u8,
    accounts: Vec<u8>,
    data: Vec<u8>,
}

fn ix_set_compute_unit_price(program_id_index: u8, microlamports: u64) -> CompiledIx {
    // ComputeBudgetInstruction::SetComputeUnitPrice = 3
    let mut data = Vec::with_capacity(9);
    data.push(3u8);
    data.extend_from_slice(&microlamports.to_le_bytes());
    CompiledIx {
        program_id_index,
        accounts: Vec::new(),
        data,
    }
}

fn encode_legacy_message(
    num_required_signatures: u8,
    num_readonly_signed: u8,
    num_readonly_unsigned: u8,
    keys: &[[u8; 32]],
    recent_blockhash: &[u8; 32],
    ixs: &[CompiledIx],
) -> Vec<u8> {
    let mut message = Vec::new();
    message.push(num_required_signatures);
    message.push(num_readonly_signed);
    message.push(num_readonly_unsigned);
    push_compact_u16(&mut message, keys.len() as u16);
    for k in keys {
        message.extend_from_slice(k);
    }
    message.extend_from_slice(recent_blockhash);
    push_compact_u16(&mut message, ixs.len() as u16);
    for ix in ixs {
        message.push(ix.program_id_index);
        push_compact_u16(&mut message, ix.accounts.len() as u16);
        message.extend_from_slice(&ix.accounts);
        push_compact_u16(&mut message, ix.data.len() as u16);
        message.extend_from_slice(&ix.data);
    }
    message
}

fn push_compact_u16(buf: &mut Vec<u8>, mut val: u16) {
    // Solana shortvec / compact-u16
    let mut continue_bit = true;
    while continue_bit {
        let mut byte = (val & 0x7f) as u8;
        val >>= 7;
        if val == 0 {
            continue_bit = false;
        } else {
            byte |= 0x80;
        }
        buf.push(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;

    #[test]
    fn usdc_ata_known_shape() {
        // Smoke: PDA derivation for a known wallet/mint must succeed and be off-curve.
        let wallet = decode_pubkey("11111111111111111111111111111112").unwrap();
        let mint = decode_pubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
        let ata = derive_associated_token_address(&wallet, &token_program_id(), &mint).unwrap();
        assert!(!is_on_curve(&ata));
    }

    #[test]
    fn parse_sol_amount() {
        assert_eq!(parse_sol_to_lamports("1").unwrap(), 1_000_000_000);
        assert_eq!(parse_sol_to_lamports("0.5").unwrap(), 500_000_000);
    }

    #[test]
    fn native_message_signature_verifies_locally() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let from = *sk.verifying_key().as_bytes();
        let to = [9u8; 32];
        let recent = [3u8; 32];
        let message = build_native_transfer_message(&from, &to, &recent, 1_000, false);
        let sig = sk.sign(&message);
        sk.verifying_key()
            .verify(&message, &sig)
            .expect("dalek verify");

        // Wire format: shortvec(1) || sig || message
        let mut tx = Vec::new();
        push_compact_u16(&mut tx, 1);
        tx.extend_from_slice(&sig.to_bytes());
        tx.extend_from_slice(&message);
        assert_eq!(tx[0], 1);
        assert_eq!(&tx[1..65], &sig.to_bytes());
        assert_eq!(&tx[65..], &message);

        // Header: 1 sig, 0 readonly signed, 1 readonly unsigned, 3 keys
        assert_eq!(message[0], 1);
        assert_eq!(message[1], 0);
        assert_eq!(message[2], 1);
        assert_eq!(message[3], 3); // compact-u16 for 3 keys
        assert_eq!(&message[4..36], &from);
    }

    #[test]
    fn dalek_matches_nacl_vector() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let msg = b"hello solana message bytes";
        let sig = sk.sign(msg);
        assert_eq!(
            hex::encode(sk.verifying_key().as_bytes()),
            "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c"
        );
        assert_eq!(
            hex::encode(sig.to_bytes()),
            "d004bd54d2df5fc8a452881f9992c90cc05ca7ac6c7ab57d9364cbc22f1730951f7713b529e126af88d8282c6fcd6758f8bf41b32b9de56ba5e05ea7402a9e0e"
        );
    }

    #[test]
    fn signed_tx_accepted_by_publicnode_sigverify() {
        // Live RPC: a correctly signed (unfunded) transfer must fail on fee payer
        // balance - NOT signature verification. Regresses wire format + dalek signing.
        use std::collections::HashMap;
        let http = match HttpCtx::new(None, HashMap::new()) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("skip network test: {e}");
                return;
            }
        };
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let from = *sk.verifying_key().as_bytes();
        let to = [9u8; 32];
        let recent = match fetch_recent_blockhash(&http) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skip network test: {e}");
                return;
            }
        };
        let message = build_native_transfer_message(&from, &to, &recent, 1_000, false);
        let err = match sign_and_broadcast(&http, &sk, &message) {
            Ok(sig) => panic!("expected simulation failure, got sig {sig}"),
            Err(e) => e.to_string(),
        };
        assert!(
            !err.to_lowercase().contains("signature verification"),
            "sigverify failed - wire format or signing bug: {err}"
        );
        assert!(
            err.contains("fee")
                || err.contains("Account")
                || err.contains("insufficient")
                || err.contains("simulate")
                || err.contains("rpc error"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn exodus_and_slip10_send_pass_sigverify() {
        use crate::wallet::hd::{derive_sol_exodus, derive_sol_slip10};
        use std::collections::HashMap;
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let http = match HttpCtx::new(None, HashMap::new()) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("skip network test: {e}");
                return;
            }
        };
        let to = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
        for (label, derived) in [
            ("exodus", derive_sol_exodus(mnemonic, "", 0, true).unwrap()),
            ("slip10", derive_sol_slip10(mnemonic, "", 0, true).unwrap()),
        ] {
            let sk = derived.private_key_hex.clone().unwrap();
            let from = derived.address.clone();
            let err = match send_sol_native(
                &http,
                &sk,
                &from,
                to,
                "0.000001",
                FeePreset::Normal,
                false,
            ) {
                Ok(s) => panic!("{label}: unexpected ok {s}"),
                Err(e) => e.to_string(),
            };
            eprintln!("{label} ({from}): {err}");
            assert!(
                !err.to_lowercase().contains("signature verification"),
                "{label} sigverify failed: {err}"
            );
        }
    }
}
