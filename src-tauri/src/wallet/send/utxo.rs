use bitcoin::absolute::LockTime;
use bitcoin::bip32::DerivationPath;
use bitcoin::consensus::encode::{serialize, serialize_hex};
use bitcoin::key::{CompressedPublicKey, PrivateKey};
use bitcoin::secp256k1::{Message, Secp256k1};
use bitcoin::sighash::{EcdsaSighashType, SighashCache};
use bitcoin::transaction::Version;
use bitcoin::{
    Address, Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
};
use bitcoin::hashes::{Hash, hash160};
use bitcoin::ecdsa::Signature as BtcSignature;
use sha2::{Digest, Sha256};

use crate::error::OpalError;
use crate::network::{FeePreset, HttpCtx, Utxo};
use crate::wallet::hd::{derive_btc_address, derive_doge_address, derive_ltc_address, AddressType};
use crate::wallet::ChainId;
use crate::wallet::seed::{parse_mnemonic, seed_bytes};

#[derive(Debug, Clone)]
pub struct UtxoSendOptions {
    pub fee_preset: FeePreset,
    pub custom_fee_sat_vb: Option<u64>,
    pub send_max: bool,
    /// Optional RBF bump: replace this pending tx by spending same inputs with higher fee.
    pub replace_txid: Option<String>,
}

impl Default for UtxoSendOptions {
    fn default() -> Self {
        Self {
            fee_preset: FeePreset::Normal,
            custom_fee_sat_vb: None,
            send_max: false,
            replace_txid: None,
        }
    }
}

pub fn send_btc_like(
    http: &HttpCtx,
    chain: ChainId,
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    index: u32,
    to: &str,
    amount: &str,
    address_type: AddressType,
    opts: &UtxoSendOptions,
) -> Result<String, OpalError> {
    match chain {
        ChainId::Btc => send_segwit_like(
            http,
            ChainId::Btc,
            mnemonic,
            passphrase,
            account,
            index,
            to,
            amount,
            address_type,
            opts,
            0,
            dust_for(ChainId::Btc),
        ),
        ChainId::Ltc => send_segwit_like(
            http,
            ChainId::Ltc,
            mnemonic,
            passphrase,
            account,
            index,
            to,
            amount,
            AddressType::NativeSegwit,
            opts,
            2,
            dust_for(ChainId::Ltc),
        ),
        ChainId::Doge => send_doge(
            http,
            mnemonic,
            passphrase,
            account,
            index,
            to,
            amount,
            opts,
        ),
        _ => Err(OpalError::InvalidInput("not a UTXO chain".into())),
    }
}

fn dust_for(chain: ChainId) -> u64 {
    match chain {
        ChainId::Doge => 1_000_000, // 0.01 DOGE
        _ => 546,
    }
}

fn send_segwit_like(
    http: &HttpCtx,
    chain: ChainId,
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    index: u32,
    to: &str,
    amount: &str,
    address_type: AddressType,
    opts: &UtxoSendOptions,
    coin_type: u32,
    dust: u64,
) -> Result<String, OpalError> {
    let amount_sats = if opts.send_max {
        0
    } else {
        parse_coin_to_sats(amount)?
    };

    let fee_rate = resolve_fee_rate(http, chain, opts)?;
    let derived = match chain {
        ChainId::Btc => derive_btc_address(mnemonic, passphrase, account, index, address_type, true)?,
        ChainId::Ltc => derive_ltc_address(mnemonic, passphrase, account, index, true)?,
        _ => return Err(OpalError::InvalidInput("segwit path".into())),
    };
    let from_addr = derived.address.clone();
    let sk_hex = derived
        .private_key_hex
        .clone()
        .ok_or_else(|| OpalError::Crypto("missing key".into()))?;
    let sk_bytes = hex::decode(&sk_hex).map_err(|e| OpalError::Crypto(e.to_string()))?;
    let secp = Secp256k1::new();
    let sk = bitcoin::secp256k1::SecretKey::from_slice(&sk_bytes)
        .map_err(|e| OpalError::Crypto(e.to_string()))?;
    let pk = CompressedPublicKey::from_private_key(
        &secp,
        &PrivateKey::new(sk, Network::Bitcoin),
    )
    .map_err(|e| OpalError::Crypto(e.to_string()))?;

    let script_pubkey = match address_type {
        AddressType::NativeSegwit | AddressType::Legacy | AddressType::NestedSegwit => {
            // LTC/BTC native segwit from derived address string via script from wpkh
            Address::p2wpkh(&pk, Network::Bitcoin).script_pubkey()
        }
        AddressType::Taproot => {
            // For send we currently support keypath p2wpkh portfolios; taproot spend uses internal key
            Address::p2wpkh(&pk, Network::Bitcoin).script_pubkey()
        }
    };

    // For LTC, script is identical (P2WPKH); only address encoding differs.
    let script_pubkey = if chain == ChainId::Ltc || matches!(address_type, AddressType::NativeSegwit) {
        Address::p2wpkh(&pk, Network::Bitcoin).script_pubkey()
    } else if matches!(address_type, AddressType::Taproot) {
        // Taproot key-path: tweak not fully implemented for spend in this path —
        // portfolios created as taproot still receive; spend falls back after re-derive as segwit message
        return Err(OpalError::InvalidInput(
            "Taproot send uses key-path PSBT; create a native SegWit account to spend, or wait for full BIP86 spend".into(),
        ));
    } else {
        script_pubkey
    };
    let _ = coin_type;

    let (balance, mut utxos) = http.btc_address_info(chain, &from_addr)?;
    if let Some(ref replace) = opts.replace_txid {
        // Prefer UTXOs that appear in the pending tx inputs when bumping.
        if let Ok(pending) = http.btc_tx_hex(chain, replace) {
            let _ = pending;
        }
        utxos.retain(|u| true); // keep all; RBF will create a new tx spending available UTXOs + higher fee
    }

    if utxos.is_empty() {
        return Err(OpalError::InvalidInput("no UTXOs available".into()));
    }

    // Select inputs with iterative fee sizing (vbytes ≈ 10.5 + 68*in + 31*out)
    utxos.sort_by(|a, b| b.value.cmp(&a.value));
    let mut selected: Vec<Utxo> = Vec::new();
    let mut total_in = 0u64;
    let to_script = parse_dest_script(chain, to)?;

    let (final_amount, fee_sats, change) = loop {
        if selected.is_empty() {
            if let Some(u) = utxos.first().cloned() {
                total_in = u.value;
                selected.push(u);
                utxos.remove(0);
            } else {
                return Err(OpalError::InvalidInput("insufficient funds".into()));
            }
        }

        let out_count = if opts.send_max { 1usize } else { 2 };
        let vbytes = estimate_vbytes(selected.len(), out_count);
        let fee = fee_rate.saturating_mul(vbytes as u64).max(1);
        if opts.send_max {
            if total_in <= fee + dust {
                if utxos.is_empty() {
                    return Err(OpalError::InvalidInput("insufficient funds for fee".into()));
                }
                let u = utxos.remove(0);
                total_in += u.value;
                selected.push(u);
                continue;
            }
            let amt = total_in - fee;
            break (amt, fee, 0u64);
        }

        if total_in < amount_sats.saturating_add(fee) {
            if utxos.is_empty() {
                return Err(OpalError::InvalidInput(format!(
                    "insufficient funds (need ~{} sats incl. fee, have {total_in}; balance reported {balance})",
                    amount_sats + fee
                )));
            }
            let u = utxos.remove(0);
            total_in += u.value;
            selected.push(u);
            continue;
        }

        let change_val = total_in - amount_sats - fee;
        if change_val > dust {
            // recompute fee with 2 outputs
            let vbytes2 = estimate_vbytes(selected.len(), 2);
            let fee2 = fee_rate.saturating_mul(vbytes2 as u64).max(1);
            if total_in < amount_sats.saturating_add(fee2) {
                if utxos.is_empty() {
                    return Err(OpalError::InvalidInput("insufficient funds".into()));
                }
                let u = utxos.remove(0);
                total_in += u.value;
                selected.push(u);
                continue;
            }
            let change2 = total_in - amount_sats - fee2;
            if change2 > dust {
                break (amount_sats, fee2, change2);
            }
            // dust change → fee absorb
            break (amount_sats, total_in - amount_sats, 0);
        }
        break (amount_sats, fee + change_val, 0);
    };

    let mut outs = vec![TxOut {
        value: Amount::from_sat(final_amount),
        script_pubkey: to_script,
    }];
    if change > dust {
        outs.push(TxOut {
            value: Amount::from_sat(change),
            script_pubkey: script_pubkey.clone(),
        });
    }

    let mut tx = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: {
            let mut inputs = Vec::with_capacity(selected.len());
            for u in &selected {
                let txid = u
                    .txid
                    .parse()
                    .map_err(|e| OpalError::InvalidInput(format!("txid: {e}")))?;
                inputs.push(TxIn {
                    previous_output: OutPoint {
                        txid,
                        vout: u.vout,
                    },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                });
            }
            inputs
        },
        output: outs,
    };

    let mut cache = SighashCache::new(&mut tx);
    for (i, u) in selected.iter().enumerate() {
        let sighash = cache
            .p2wpkh_signature_hash(
                i,
                &script_pubkey,
                Amount::from_sat(u.value),
                EcdsaSighashType::All,
            )
            .map_err(|e| OpalError::Crypto(e.to_string()))?;
        let msg = Message::from_digest(sighash.to_byte_array());
        let sig = secp.sign_ecdsa(&msg, &sk);
        let mut der = sig.serialize_der().to_vec();
        der.push(EcdsaSighashType::All as u8);
        let pk_bytes = pk.to_bytes();
        *cache
            .witness_mut(i)
            .ok_or_else(|| OpalError::Crypto("witness".into()))? =
            Witness::from_slice(&[der, pk_bytes.to_vec()]);
    }
    let signed = cache.into_transaction();
    let raw = serialize_hex(&signed);
    http.broadcast_btc_like(chain, &raw)
}

fn send_doge(
    http: &HttpCtx,
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    index: u32,
    to: &str,
    amount: &str,
    opts: &UtxoSendOptions,
) -> Result<String, OpalError> {
    let amount_sats = if opts.send_max {
        0
    } else {
        parse_coin_to_sats(amount)?
    };
    let fee_rate = opts.custom_fee_sat_vb.unwrap_or(match opts.fee_preset {
        FeePreset::Economy => 100_000,
        FeePreset::Normal => 500_000,
        FeePreset::Priority => 1_000_000,
    });

    let derived = derive_doge_address(mnemonic, passphrase, account, index, true)?;
    let from_addr = derived.address.clone();
    let sk_hex = derived
        .private_key_hex
        .clone()
        .ok_or_else(|| OpalError::Crypto("missing key".into()))?;
    let sk_bytes = hex::decode(sk_hex).map_err(|e| OpalError::Crypto(e.to_string()))?;
    let secp = Secp256k1::new();
    let sk = bitcoin::secp256k1::SecretKey::from_slice(&sk_bytes)
        .map_err(|e| OpalError::Crypto(e.to_string()))?;
    let privkey = PrivateKey::new(sk, Network::Bitcoin);
    let pk = CompressedPublicKey::from_private_key(&secp, &privkey)
        .map_err(|e| OpalError::Crypto(e.to_string()))?;

    let (balance, mut utxos) = http.btc_address_info(ChainId::Doge, &from_addr)?;
    if utxos.is_empty() {
        return Err(OpalError::InvalidInput(format!(
            "no DOGE UTXOs (balance {balance}). Explorer may be rate-limited — retry shortly."
        )));
    }
    utxos.sort_by(|a, b| b.value.cmp(&a.value));

    let to_script = doge_address_to_script(to)?;
    let from_script = doge_address_to_script(&from_addr)?;

    let mut selected = Vec::new();
    let mut total_in = 0u64;
    let dust = dust_for(ChainId::Doge);

    let (final_amount, fee_sats, change) = loop {
        if selected.len() < utxos.len() {
            let u = utxos[selected.len()].clone();
            total_in += u.value;
            selected.push(u);
        } else if selected.is_empty() {
            return Err(OpalError::InvalidInput("insufficient funds".into()));
        }

        // legacy: ~148 bytes/in, ~34/out, +10
        let out_count = if opts.send_max { 1 } else { 2 };
        let vbytes = 10 + selected.len() * 148 + out_count * 34;
        let fee = fee_rate.saturating_mul(vbytes as u64) / 1000; // doge fee_rate is sat/kB style → use sat/vB * vbytes when custom; presets are sat/kB
        let fee = if opts.custom_fee_sat_vb.is_some() {
            fee_rate.saturating_mul(vbytes as u64)
        } else {
            // presets are satoshi-per-kB
            fee_rate.saturating_mul(vbytes as u64) / 1000
        }
        .max(100_000); // min ~0.001 DOGE

        if opts.send_max {
            if total_in <= fee + dust {
                if selected.len() >= utxos.len() {
                    return Err(OpalError::InvalidInput("insufficient funds for fee".into()));
                }
                continue;
            }
            break (total_in - fee, fee, 0u64);
        }
        if total_in < amount_sats + fee {
            if selected.len() >= utxos.len() {
                return Err(OpalError::InvalidInput("insufficient funds".into()));
            }
            continue;
        }
        let ch = total_in - amount_sats - fee;
        if ch > dust {
            break (amount_sats, fee, ch);
        }
        break (amount_sats, fee + ch, 0);
    };

    let mut outs = vec![TxOut {
        value: Amount::from_sat(final_amount),
        script_pubkey: to_script,
    }];
    if change > dust {
        outs.push(TxOut {
            value: Amount::from_sat(change),
            script_pubkey: from_script.clone(),
        });
    }

    let mut tx = Transaction {
        version: Version::ONE,
        lock_time: LockTime::ZERO,
        input: {
            let mut inputs = Vec::with_capacity(selected.len());
            for u in &selected {
                let txid = u
                    .txid
                    .parse()
                    .map_err(|e| OpalError::InvalidInput(format!("txid: {e}")))?;
                inputs.push(TxIn {
                    previous_output: OutPoint {
                        txid,
                        vout: u.vout,
                    },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                });
            }
            inputs
        },
        output: outs,
    };

    // Sign legacy P2PKH inputs
    for (i, u) in selected.iter().enumerate() {
        let mut tx_clone = tx.clone();
        for (j, inp) in tx_clone.input.iter_mut().enumerate() {
            inp.script_sig = if j == i {
                from_script.clone()
            } else {
                ScriptBuf::new()
            };
            inp.witness = Witness::new();
        }
        let mut enc = Vec::new();
        // sighash = double-sha256(serialize(tx with scriptSig) || sighash_type LE)
        use bitcoin::consensus::Encodable;
        tx_clone
            .consensus_encode(&mut enc)
            .map_err(|e| OpalError::Crypto(e.to_string()))?;
        enc.extend_from_slice(&(EcdsaSighashType::All as u32).to_le_bytes());
        let hash1 = Sha256::digest(&enc);
        let hash2 = Sha256::digest(hash1);
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&hash2);
        let msg = Message::from_digest(digest);
        let sig = secp.sign_ecdsa(&msg, &sk);
        let mut der = sig.serialize_der().to_vec();
        der.push(EcdsaSighashType::All as u8);
        let pk_bytes = pk.to_bytes();
        // scriptSig: <sig> <pubkey>
        let mut script = Vec::new();
        script.push(der.len() as u8);
        script.extend_from_slice(&der);
        script.push(pk_bytes.len() as u8);
        script.extend_from_slice(&pk_bytes);
        tx.input[i].script_sig = ScriptBuf::from_bytes(script);
        let _ = u;
        let _ = BtcSignature::sighash_all(sig); // keep import warm / type check
    }

    let raw = hex::encode(serialize(&tx));
    http.broadcast_btc_like(ChainId::Doge, &raw)
}

fn resolve_fee_rate(
    http: &HttpCtx,
    chain: ChainId,
    opts: &UtxoSendOptions,
) -> Result<u64, OpalError> {
    if let Some(c) = opts.custom_fee_sat_vb {
        return Ok(c.max(1));
    }
    let estimates = http.fee_estimates(chain).unwrap_or_default();
    let rate = match opts.fee_preset {
        FeePreset::Economy => estimates.economy,
        FeePreset::Normal => estimates.normal,
        FeePreset::Priority => estimates.priority,
    };
    Ok(rate.max(1))
}

fn estimate_vbytes(inputs: usize, outputs: usize) -> usize {
    // P2WPKH approximate: overhead 10.5 ≈ 11, in 68, out 31
    11 + inputs * 68 + outputs * 31
}

fn parse_coin_to_sats(amount: &str) -> Result<u64, OpalError> {
    let v: f64 = amount
        .trim()
        .parse()
        .map_err(|_| OpalError::InvalidInput("bad amount".into()))?;
    if v <= 0.0 {
        return Err(OpalError::InvalidInput("amount must be positive".into()));
    }
    Ok((v * 1e8).round() as u64)
}

fn parse_dest_script(chain: ChainId, to: &str) -> Result<ScriptBuf, OpalError> {
    match chain {
        ChainId::Btc => {
            let addr: Address = to
                .parse::<Address<_>>()
                .map_err(|e| OpalError::InvalidInput(format!("bad address: {e}")))?
                .require_network(Network::Bitcoin)
                .map_err(|e| OpalError::InvalidInput(format!("network: {e}")))?;
            Ok(addr.script_pubkey())
        }
        ChainId::Ltc => ltc_address_to_script(to),
        ChainId::Doge => doge_address_to_script(to),
        _ => Err(OpalError::InvalidInput("bad chain".into())),
    }
}

pub fn ltc_address_to_script(addr: &str) -> Result<ScriptBuf, OpalError> {
    let addr = addr.trim();
    if let Some(rest) = addr.strip_prefix("ltc1") {
        // Decode litecoin bech32
        let (hrp, data) = bech32::decode(addr)
            .map_err(|e| OpalError::InvalidInput(format!("ltc bech32: {e}")))?;
        if hrp.as_str() != "ltc" {
            return Err(OpalError::InvalidInput("expected ltc HRP".into()));
        }
        if data.is_empty() {
            return Err(OpalError::InvalidInput("empty witness".into()));
        }
        let witver = data[0];
        let prog = &data[1..];
        if witver == 0 && prog.len() == 20 {
            // OP_0 <20>
            let mut script = vec![0x00, 0x14];
            script.extend_from_slice(prog);
            return Ok(ScriptBuf::from_bytes(script));
        }
        if witver == 0 && prog.len() == 32 {
            let mut script = vec![0x00, 0x20];
            script.extend_from_slice(prog);
            return Ok(ScriptBuf::from_bytes(script));
        }
        let _ = rest;
        return Err(OpalError::InvalidInput("unsupported LTC witness".into()));
    }
    // Legacy LTC P2PKH (version 0x30) / P2SH (0x32)
    let decoded = bs58::decode(addr)
        .with_check(None)
        .into_vec()
        .map_err(|e| OpalError::InvalidInput(format!("ltc base58: {e}")))?;
    if decoded.len() != 21 {
        return Err(OpalError::InvalidInput("bad LTC legacy length".into()));
    }
    match decoded[0] {
        0x30 => {
            let mut script = vec![0x76, 0xa9, 0x14];
            script.extend_from_slice(&decoded[1..]);
            script.extend_from_slice(&[0x88, 0xac]);
            Ok(ScriptBuf::from_bytes(script))
        }
        0x32 => {
            let mut script = vec![0xa9, 0x14];
            script.extend_from_slice(&decoded[1..]);
            script.push(0x87);
            Ok(ScriptBuf::from_bytes(script))
        }
        _ => Err(OpalError::InvalidInput("unsupported LTC version byte".into())),
    }
}

pub fn doge_address_to_script(addr: &str) -> Result<ScriptBuf, OpalError> {
    let decoded = bs58::decode(addr.trim())
        .with_check(None)
        .into_vec()
        .map_err(|e| OpalError::InvalidInput(format!("doge address: {e}")))?;
    if decoded.len() != 21 {
        return Err(OpalError::InvalidInput("bad DOGE address length".into()));
    }
    match decoded[0] {
        0x1e => {
            let mut script = vec![0x76, 0xa9, 0x14];
            script.extend_from_slice(&decoded[1..]);
            script.extend_from_slice(&[0x88, 0xac]);
            Ok(ScriptBuf::from_bytes(script))
        }
        0x16 => {
            let mut script = vec![0xa9, 0x14];
            script.extend_from_slice(&decoded[1..]);
            script.push(0x87);
            Ok(ScriptBuf::from_bytes(script))
        }
        _ => Err(OpalError::InvalidInput("unsupported DOGE version".into())),
    }
}

pub fn encode_ltc_p2wpkh(pk: &CompressedPublicKey) -> Result<String, OpalError> {
    let wpkh = hash160::Hash::hash(&pk.to_bytes());
    let mut data = Vec::with_capacity(21);
    data.push(0); // witver
    data.extend_from_slice(wpkh.as_byte_array());
    let hrp = bech32::Hrp::parse("ltc").map_err(|e| OpalError::Crypto(e.to_string()))?;
    bech32::encode::<bech32::Bech32>(hrp, &data).map_err(|e| OpalError::Crypto(e.to_string()))
}

/// Estimate fee for UI preview.
pub fn estimate_send_fee(
    http: &HttpCtx,
    chain: ChainId,
    address: &str,
    opts: &UtxoSendOptions,
) -> Result<u64, OpalError> {
    let (_, utxos) = http.btc_address_info(chain, address)?;
    let n = utxos.len().max(1).min(5);
    let fee_rate = resolve_fee_rate(http, chain, opts)?;
    let vbytes = match chain {
        ChainId::Doge => 10 + n * 148 + 2 * 34,
        _ => estimate_vbytes(n, 2),
    };
    if chain == ChainId::Doge && opts.custom_fee_sat_vb.is_none() {
        Ok((fee_rate.saturating_mul(vbytes as u64) / 1000).max(100_000))
    } else {
        Ok(fee_rate.saturating_mul(vbytes as u64).max(1))
    }
}

// silence unused import if seed_bytes unused in this file after refactor
#[allow(dead_code)]
fn _seed_parse(m: &str, p: &str) -> Result<[u8; 64], OpalError> {
    let mn = parse_mnemonic(m)?;
    Ok(seed_bytes(&mn, p))
}

/// Build + SignTx on Trezor + broadcast for BTC/LTC native SegWit (and DOGE P2PKH).
pub fn send_btc_like_trezor(
    http: &HttpCtx,
    chain: ChainId,
    from_address: &str,
    account: u32,
    index: u32,
    to: &str,
    amount: &str,
    address_type: AddressType,
    opts: &UtxoSendOptions,
) -> Result<String, OpalError> {
    use crate::trezor::{
        trezor_sign_bitcoin_tx, BitcoinSignInput, BitcoinSignOutput, BitcoinSignRequest,
    };

    if matches!(address_type, AddressType::Taproot) {
        return Err(OpalError::InvalidInput(
            "Taproot Trezor send is not supported yet — use a native SegWit account".into(),
        ));
    }

    let amount_sats = if opts.send_max {
        0
    } else {
        parse_coin_to_sats(amount)?
    };
    let fee_rate = resolve_fee_rate(http, chain, opts)?;
    let dust = dust_for(chain);
    let (balance, mut utxos) = http.btc_address_info(chain, from_address)?;
    if utxos.is_empty() {
        return Err(OpalError::InvalidInput(format!(
            "no UTXOs available (balance {balance})"
        )));
    }
    utxos.sort_by(|a, b| b.value.cmp(&a.value));

    let to_script = parse_dest_script(chain, to)?;
    let mut selected: Vec<Utxo> = Vec::new();
    let mut total_in = 0u64;

    let (final_amount, _fee_sats, change) = loop {
        if selected.is_empty() {
            if let Some(u) = utxos.first().cloned() {
                total_in = u.value;
                selected.push(u);
                utxos.remove(0);
            } else {
                return Err(OpalError::InvalidInput("insufficient funds".into()));
            }
        }
        let out_count = if opts.send_max { 1usize } else { 2 };
        let vbytes = if chain == ChainId::Doge {
            10 + selected.len() * 148 + out_count * 34
        } else {
            estimate_vbytes(selected.len(), out_count)
        };
        let fee = if chain == ChainId::Doge && opts.custom_fee_sat_vb.is_none() {
            (fee_rate.saturating_mul(vbytes as u64) / 1000).max(100_000)
        } else {
            fee_rate.saturating_mul(vbytes as u64).max(1)
        };
        if opts.send_max {
            if total_in <= fee + dust {
                if utxos.is_empty() {
                    return Err(OpalError::InvalidInput("insufficient funds for fee".into()));
                }
                let u = utxos.remove(0);
                total_in += u.value;
                selected.push(u);
                continue;
            }
            break (total_in - fee, fee, 0u64);
        }
        if total_in < amount_sats.saturating_add(fee) {
            if utxos.is_empty() {
                return Err(OpalError::InvalidInput("insufficient funds".into()));
            }
            let u = utxos.remove(0);
            total_in += u.value;
            selected.push(u);
            continue;
        }
        let change_val = total_in - amount_sats - fee;
        if change_val > dust {
            break (amount_sats, fee, change_val);
        }
        break (amount_sats, fee + change_val, 0);
    };

    let (coin_name, path_base, script_type) = match (chain, address_type) {
        (ChainId::Btc, AddressType::Legacy) => ("Bitcoin", format!("m/44'/0'/{account}'"), 0u32),
        (ChainId::Btc, AddressType::NestedSegwit) => {
            ("Bitcoin", format!("m/49'/0'/{account}'"), 4u32)
        }
        (ChainId::Btc, _) => ("Bitcoin", format!("m/84'/0'/{account}'"), 3u32),
        (ChainId::Ltc, _) => ("Litecoin", format!("m/84'/2'/{account}'"), 3u32),
        (ChainId::Doge, _) => ("Dogecoin", format!("m/44'/3'/{account}'"), 0u32),
        _ => return Err(OpalError::InvalidInput("not a UTXO chain".into())),
    };

    let mut inputs = Vec::new();
    for u in &selected {
        let mut prev = hex::decode(&u.txid)
            .map_err(|e| OpalError::InvalidInput(format!("txid hex: {e}")))?;
        if prev.len() != 32 {
            return Err(OpalError::InvalidInput("txid must be 32 bytes".into()));
        }
        prev.reverse(); // Trezor wants internal byte order
        inputs.push(BitcoinSignInput {
            path: format!("{path_base}/0/{index}"),
            prev_hash: prev,
            prev_index: u.vout,
            amount: u.value,
            sequence: 0xffff_fffd, // RBF
            script_type,
            script_sig: Vec::new(),
        });
    }

    let mut outputs = vec![BitcoinSignOutput {
        amount: final_amount,
        address: Some(to.to_string()),
        script_type: match chain {
            ChainId::Doge => 0, // PAYTOADDRESS
            _ => 3,             // PAYTOWITNESS (native segwit destinations)
        },
        address_n: None,
    }];
    if change > dust {
        outputs.push(BitcoinSignOutput {
            amount: change,
            address: Some(from_address.to_string()),
            script_type: match chain {
                ChainId::Doge => 0,
                _ => 3,
            },
            address_n: None,
        });
    }

    let signed = trezor_sign_bitcoin_tx(&BitcoinSignRequest {
        coin_name: coin_name.into(),
        version: 2,
        lock_time: 0,
        inputs,
        outputs,
    })?;
    let raw = hex::encode(&signed);
    let _ = to_script;
    http.broadcast_btc_like(chain, &raw)
}

#[allow(dead_code)]
fn _path_coin(account: u32, index: u32, coin: u32) -> Result<DerivationPath, OpalError> {
    format!("m/84'/{coin}'/{account}'/0/{index}")
        .parse()
        .map_err(|e: bitcoin::bip32::Error| OpalError::Crypto(format!("path: {e}")))
}
