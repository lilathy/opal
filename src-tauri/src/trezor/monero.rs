//! Monero-on-Trezor credentials + LiveRefresh / key-image import.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use curve25519_dalek::edwards::CompressedEdwardsY;
use curve25519_dalek::scalar::Scalar;
use sha3::{Digest as KeccakDigest, Keccak256};

use crate::error::OpalError;
use crate::network::HttpCtx;
use crate::trezor::{self, trezor_monero_live_refresh, MoneroFreshKeyImage};
use crate::wallet::xmr_rpc::{xmr_wallet_dir, XmrWalletRpc};

fn wallet_filename(address: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(address.as_bytes());
    format!("opal_{}", hex::encode(&hash[..16]))
}

/// Confirm the connected firmware can speak Monero (Model T / Safe).
pub fn trezor_monero_supported() -> Result<bool, OpalError> {
    let status = trezor::probe_trezor();
    if !status.available || status.device_count == 0 {
        return Ok(false);
    }
    if status.device_model.as_deref() == Some("1") {
        return Ok(false);
    }
    Ok(true)
}

/// Fetch address + view key for an account (used by create / sync).
pub fn trezor_monero_credentials(account: u32) -> Result<(String, String), OpalError> {
    if !trezor_monero_supported()? {
        return Err(OpalError::InvalidInput(
            "Monero on Trezor needs a Model T or Safe device (not Trezor One)".into(),
        ));
    }
    let path = format!("m/44'/128'/{account}'");
    let addr = trezor::trezor_get_monero_address(&path, false)?;
    let watch = trezor::trezor_get_monero_watch_key(&path)?;
    Ok((addr, watch.watch_key_hex))
}

/// Ask Trezor for key images for known outputs (corrects watch-only balances).
/// Fast scrapes stay on watch-only refresh; this runs only when a device is present.
pub fn trezor_monero_sync_key_images(
    http: &HttpCtx,
    view_hex: &str,
    address: &str,
    account: u32,
) -> Result<usize, OpalError> {
    if !trezor_monero_supported()? {
        return Ok(0);
    }
    let view = parse_view_key(view_hex)?;

    crate::wallet::monero_runtime::ensure_wallet_rpc_running()?;
    let _ = xmr_wallet_dir()?;
    let rpc = XmrWalletRpc::from_http(http)?;
    let filename = wallet_filename(address);
    let _ = rpc.open_wallet(&filename, "");
    let transfers = rpc.incoming_transfers_available().unwrap_or_default();
    if transfers.is_empty() {
        return Ok(0);
    }

    // Cap batch so reconnect sync stays interactive (user confirms once on device).
    const MAX_BATCH: usize = 64;
    let mut batch = Vec::new();
    let mut meta: Vec<(Vec<u8>, String)> = Vec::new(); // (out_key, txid) for import order
    for t in transfers.into_iter().take(MAX_BATCH) {
        let out_key = hex::decode(t.pubkey.trim()).unwrap_or_default();
        let tx_pub = hex::decode(t.tx_pubkey.trim()).unwrap_or_default();
        if out_key.len() != 32 || tx_pub.len() != 32 {
            continue;
        }
        let recv_deriv = match key_derivation(&view, &tx_pub) {
            Ok(d) => d,
            Err(_) => continue,
        };
        batch.push((
            out_key.clone(),
            recv_deriv,
            t.internal_output_index,
            0u32,
            0u32,
        ));
        meta.push((out_key, t.txid));
    }
    if batch.is_empty() {
        return Ok(0);
    }

    let path = format!("m/44'/128'/{account}'");
    let sealed = trezor_monero_live_refresh(&path, &batch)?;
    if sealed.len() != batch.len() {
        return Err(OpalError::Io(format!(
            "LiveRefresh returned {} images, expected {}",
            sealed.len(),
            batch.len()
        )));
    }

    let mut signed = Vec::new();
    for (i, fresh) in sealed.iter().enumerate() {
        let out_key = &meta[i].0;
        match decrypt_live_refresh_ki(&view, out_key, fresh) {
            Ok((ki, sig)) => {
                signed.push((hex::encode(ki), hex::encode(sig)));
            }
            Err(e) => {
                tracing_warn(format!("skip KI decrypt: {e}"));
            }
        }
    }
    if !signed.is_empty() {
        let _ = rpc.import_key_images(&signed);
    }
    let _ = rpc.refresh(None);
    Ok(signed.len())
}

fn tracing_warn(msg: String) {
    // Avoid pulling a logger crate; stderr is enough for desktop debug.
    eprintln!("[opal][xmr-ki] {msg}");
}

fn parse_view_key(hex_key: &str) -> Result<[u8; 32], OpalError> {
    let raw = hex::decode(hex_key.trim())
        .map_err(|e| OpalError::InvalidInput(format!("bad XMR view key: {e}")))?;
    if raw.len() != 32 {
        return Err(OpalError::InvalidInput("XMR view key must be 32 bytes".into()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&raw);
    Ok(out)
}

/// Monero `generate_key_derivation`: `8 * (a * R)`.
fn key_derivation(view: &[u8; 32], tx_pub: &[u8]) -> Result<Vec<u8>, OpalError> {
    let compressed = CompressedEdwardsY::from_slice(tx_pub)
        .map_err(|_| OpalError::Crypto("bad tx pubkey".into()))?;
    let point = compressed
        .decompress()
        .ok_or_else(|| OpalError::Crypto("tx pubkey not on curve".into()))?;
    let a = Scalar::from_bytes_mod_order(*view);
    let der = (a * point).mul_by_cofactor();
    Ok(der.compress().to_bytes().to_vec())
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut h = Keccak256::new();
    KeccakDigest::update(&mut h, data);
    let dig = KeccakDigest::finalize(h);
    let mut out = [0u8; 32];
    out.copy_from_slice(&dig);
    out
}

fn keccak_2hash(data: &[u8]) -> [u8; 32] {
    keccak256(&keccak256(data))
}

/// HMAC-Keccak256 matching Trezor `crypto_helpers.compute_hmac` (block = 136).
fn hmac_keccak(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BS: usize = 136;
    let key_digest;
    let key = if key.len() > BS {
        key_digest = keccak256(key);
        key_digest.as_slice()
    } else {
        key
    };
    let mut key_block = [0u8; BS];
    key_block[..key.len()].copy_from_slice(key);

    let mut inner_pad = key_block;
    for b in &mut inner_pad {
        *b ^= 0x36;
    }
    let mut outer_pad = key_block;
    for b in &mut outer_pad {
        *b ^= 0x5c;
    }

    let mut inner = Keccak256::new();
    KeccakDigest::update(&mut inner, inner_pad);
    KeccakDigest::update(&mut inner, msg);
    let inner_dig = KeccakDigest::finalize(inner);

    let mut outer = Keccak256::new();
    KeccakDigest::update(&mut outer, outer_pad);
    KeccakDigest::update(&mut outer, inner_dig);
    let dig = KeccakDigest::finalize(outer);
    let mut out = [0u8; 32];
    out.copy_from_slice(&dig);
    out
}

/// Host-side `compute_enc_key(view, out_key, salt)` for LiveRefresh.
fn compute_enc_key(view: &[u8; 32], out_key: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut passwd_inp = Vec::with_capacity(32 + out_key.len());
    passwd_inp.extend_from_slice(view);
    passwd_inp.extend_from_slice(out_key);
    let passwd = keccak_2hash(&passwd_inp);
    hmac_keccak(salt, &passwd)
}

fn chacha_open(key: &[u8], blob: &[u8]) -> Result<Vec<u8>, OpalError> {
    if blob.len() < 12 + 16 {
        return Err(OpalError::Io("encrypted key image too short".into()));
    }
    let nonce = Nonce::from_slice(&blob[..12]);
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| OpalError::Io(format!("chacha key: {e}")))?;
    cipher
        .decrypt(nonce, &blob[12..])
        .map_err(|_| OpalError::Io("key image decrypt failed".into()))
}

fn decrypt_live_refresh_ki(
    view: &[u8; 32],
    out_key: &[u8],
    fresh: &MoneroFreshKeyImage,
) -> Result<([u8; 32], [u8; 64]), OpalError> {
    if fresh.salt.len() != 32 {
        return Err(OpalError::Io("LiveRefresh salt must be 32 bytes".into()));
    }
    let key = compute_enc_key(view, out_key, &fresh.salt);
    let plain = chacha_open(&key, &fresh.key_image_blob)?;
    if plain.len() != 96 {
        return Err(OpalError::Io(format!(
            "LiveRefresh plaintext len {}, expected 96",
            plain.len()
        )));
    }
    let mut ki = [0u8; 32];
    let mut sig = [0u8; 64];
    ki.copy_from_slice(&plain[..32]);
    sig.copy_from_slice(&plain[32..96]);
    Ok((ki, sig))
}
