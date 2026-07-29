//! TON Wallet V4R2 address derivation (BIP44 path m/44'/607'/account').
//!
//! Builds the same StateInit hash as `@ton/ton` WalletContractV4 so software
//! portfolios match TON Keeper / Exodus-style receive addresses.

use sha2::{Digest, Sha256};

use crate::error::OpalError;

/// Representation hash of the fixed Wallet V4R2 code cell
/// (`Cell.fromBoc(...).hash()` from `@ton/ton`).
const WALLET_V4R2_CODE_HASH: [u8; 32] = [
    0xfe, 0xb5, 0xff, 0x68, 0x20, 0xe2, 0xff, 0x0d, 0x94, 0x83, 0xe7, 0xe0, 0xd6, 0x2c, 0x81, 0x7d,
    0x84, 0x67, 0x89, 0xfb, 0x4a, 0xe5, 0x80, 0xc8, 0x78, 0x86, 0x6d, 0x95, 0x9d, 0xab, 0xd5, 0xc0,
];

/// `code.depth()` for the V4R2 code BOC - the code cell has child refs, so
/// depth is 7 (not 0). StateInit's representation hash includes this.
const WALLET_V4R2_CODE_DEPTH: u16 = 7;

const WALLET_ID_V4R2: u32 = 698_983_191;

/// Minimal bit/byte cell builder sufficient for wallet data + StateInit.
struct CellBuilder {
    bits: Vec<bool>,
    /// (hash, depth) of each referenced cell
    refs: Vec<([u8; 32], u16)>,
}

impl CellBuilder {
    fn new() -> Self {
        Self {
            bits: Vec::new(),
            refs: Vec::new(),
        }
    }

    fn store_bit(&mut self, bit: bool) {
        self.bits.push(bit);
    }

    fn store_uint(&mut self, value: u64, n_bits: usize) {
        for i in (0..n_bits).rev() {
            self.store_bit(((value >> i) & 1) == 1);
        }
    }

    fn store_bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.store_uint(u64::from(*b), 8);
        }
    }

    fn store_ref(&mut self, hash: [u8; 32], depth: u16) {
        self.refs.push((hash, depth));
    }

    fn hash(self) -> [u8; 32] {
        let bit_len = self.bits.len();
        let refs_count = self.refs.len() as u8;
        let d1 = refs_count;
        let d2 = ((bit_len / 8) + ((bit_len + 7) / 8)) as u8;

        let mut data_bytes = vec![0u8; (bit_len + 7) / 8];
        for (i, bit) in self.bits.iter().enumerate() {
            if *bit {
                data_bytes[i / 8] |= 1 << (7 - (i % 8));
            }
        }
        if bit_len % 8 != 0 {
            let pad_bit = 7 - (bit_len % 8);
            let last = data_bytes.len() - 1;
            data_bytes[last] |= 1 << pad_bit;
        }

        let mut repr = Vec::with_capacity(2 + data_bytes.len() + self.refs.len() * 34);
        repr.push(d1);
        repr.push(d2);
        repr.extend_from_slice(&data_bytes);
        // Depths first (uint16 BE), then hashes - @ton/core getRepr order.
        for (_, depth) in &self.refs {
            repr.push((depth >> 8) as u8);
            repr.push((depth & 0xff) as u8);
        }
        for (hash, _) in &self.refs {
            repr.extend_from_slice(hash);
        }
        let digest = Sha256::digest(&repr);
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        out
    }
}

fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// Non-bounceable, URL-safe user-friendly mainnet address (UQ…).
pub fn encode_ton_address(workchain: i8, hash: &[u8; 32], bounceable: bool) -> String {
    let tag: u8 = if bounceable { 0x11 } else { 0x51 };
    let mut buf = Vec::with_capacity(36);
    buf.push(tag);
    buf.push(workchain as u8);
    buf.extend_from_slice(hash);
    let crc = crc16_ccitt(&buf);
    buf.push((crc >> 8) as u8);
    buf.push((crc & 0xff) as u8);
    base64_url_encode(&buf)
}

fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// Derive Wallet V4R2 account address hash from a 32-byte Ed25519 public key.
pub fn v4r2_address_hash(public_key: &[u8; 32]) -> [u8; 32] {
    let mut data = CellBuilder::new();
    data.store_uint(0, 32); // seqno
    data.store_uint(u64::from(WALLET_ID_V4R2), 32);
    data.store_bytes(public_key);
    data.store_bit(false); // empty plugins dict
    let data_hash = data.hash();

    let mut state = CellBuilder::new();
    state.store_bit(false); // split_depth
    state.store_bit(false); // special
    state.store_bit(true); // has code
    state.store_ref(WALLET_V4R2_CODE_HASH, WALLET_V4R2_CODE_DEPTH);
    state.store_bit(true); // has data
    state.store_ref(data_hash, 0); // data cell is a leaf
    state.store_bit(false); // library
    state.hash()
}

pub fn address_from_pubkey(public_key: &[u8; 32]) -> String {
    let hash = v4r2_address_hash(public_key);
    // Non-bounceable is what wallets show for receiving (UQ…)
    encode_ton_address(0, &hash, false)
}

pub fn looks_like_ton_address(address: &str) -> bool {
    let a = address.trim();
    if a.starts_with("EQ") || a.starts_with("UQ") || a.starts_with("kQ") || a.starts_with("0Q") {
        return a.len() >= 48;
    }
    // raw form workchain:hex
    if let Some((wc, hex)) = a.split_once(':') {
        return (wc == "0" || wc == "-1") && hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    false
}

pub fn validate_ton_address(address: &str) -> Result<(), OpalError> {
    if looks_like_ton_address(address) {
        Ok(())
    } else {
        Err(OpalError::InvalidInput(
            "not a TON address (expected EQ…/UQ… or 0:hex)".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_pubkey_v4r2_matches_ton_core() {
        // pubkey from bip39 abandon… + ed25519-hd-key m/44'/607'/0' (JS vector)
        let mut pk = [0u8; 32];
        hex::decode_to_slice(
            "7952e94118f34607c75e23258dd9220d66ccac5a3ee074125c25068e8107bfbf",
            &mut pk,
        )
        .unwrap();
        let addr = address_from_pubkey(&pk);
        assert_eq!(addr, "UQAzWZa6nM5mJev91wGc7VCSfBoIsYRqKJpV78N8Add9-RKY");
    }

    #[test]
    fn data_hash_matches_ton_core() {
        let mut pk = [0u8; 32];
        hex::decode_to_slice(
            "7952e94118f34607c75e23258dd9220d66ccac5a3ee074125c25068e8107bfbf",
            &mut pk,
        )
        .unwrap();
        let mut data = CellBuilder::new();
        data.store_uint(0, 32);
        data.store_uint(u64::from(WALLET_ID_V4R2), 32);
        data.store_bytes(&pk);
        data.store_bit(false);
        let hash = data.hash();
        assert_eq!(
            hex::encode(hash),
            "3682518914842b5a4496552643bde7b4735cee9218a87216849dc7bb3428bb4b"
        );
    }
}
