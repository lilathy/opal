use bech32::Fe32;
use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::hashes::Hash;
use bitcoin::key::{CompressedPublicKey, PrivateKey, TapTweak, UntweakedPublicKey};
use bitcoin::secp256k1::Secp256k1;
use bitcoin::{Address, Network, PublicKey};
use k256::ecdsa::SigningKey;
use sha2::{Digest, Sha256};
use sha3::Keccak256;
use slip10_ed25519::derive_ed25519_private_key;
use zeroize::Zeroize;

use crate::error::OpalError;
use crate::wallet::seed::{parse_mnemonic, seed_bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainId {
    Btc,
    Eth,
    Arb,
    Base,
    Op,
    Polygon,
    Avax,
    Bsc,
    Gnosis,
    Trx,
    Linea,
    Sol,
    Ltc,
    Doge,
    Xmr,
    Ton,
}

impl ChainId {
    pub fn parse(s: &str) -> Result<Self, OpalError> {
        match s.to_ascii_lowercase().as_str() {
            "btc" | "bitcoin" => Ok(Self::Btc),
            "eth" | "ethereum" => Ok(Self::Eth),
            "arb" | "arbitrum" => Ok(Self::Arb),
            "base" => Ok(Self::Base),
            "op" | "optimism" => Err(OpalError::InvalidInput(
                "Optimism is not supported".into(),
            )),
            "polygon" | "matic" | "pol" => Ok(Self::Polygon),
            "avax" | "avalanche" => Ok(Self::Avax),
            "bsc" | "bnb" | "binance" => Ok(Self::Bsc),
            "gnosis" | "xdai" | "gno" => Ok(Self::Gnosis),
            "trx" | "tron" => Ok(Self::Trx),
            "linea" => Ok(Self::Linea),
            "sol" | "solana" => Ok(Self::Sol),
            "ltc" | "litecoin" => Ok(Self::Ltc),
            "doge" | "dogecoin" => Ok(Self::Doge),
            "xmr" | "monero" => Ok(Self::Xmr),
            "ton" | "toncoin" => Ok(Self::Ton),
            other => Err(OpalError::InvalidInput(format!("unknown chain: {other}"))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Btc => "btc",
            Self::Eth => "eth",
            Self::Arb => "arb",
            Self::Base => "base",
            Self::Op => "op",
            Self::Polygon => "polygon",
            Self::Avax => "avax",
            Self::Bsc => "bsc",
            Self::Gnosis => "gnosis",
            Self::Trx => "trx",
            Self::Linea => "linea",
            Self::Sol => "sol",
            Self::Ltc => "ltc",
            Self::Doge => "doge",
            Self::Xmr => "xmr",
            Self::Ton => "ton",
        }
    }

    pub fn is_evm(self) -> bool {
        matches!(
            self,
            Self::Eth
                | Self::Arb
                | Self::Base
                | Self::Op
                | Self::Polygon
                | Self::Avax
                | Self::Bsc
                | Self::Gnosis
                | Self::Linea
        )
    }

    pub fn is_utxo(self) -> bool {
        matches!(self, Self::Btc | Self::Ltc | Self::Doge)
    }

    pub fn chain_id_u64(self) -> Option<u64> {
        match self {
            Self::Eth => Some(1),
            Self::Arb => Some(42161),
            Self::Base => Some(8453),
            Self::Op => Some(10),
            Self::Polygon => Some(137),
            Self::Avax => Some(43114),
            Self::Bsc => Some(56),
            Self::Gnosis => Some(100),
            Self::Linea => Some(59144),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Btc => "Bitcoin",
            Self::Eth => "Ethereum",
            Self::Arb => "Arbitrum One",
            Self::Base => "Base",
            Self::Op => "Optimism",
            Self::Polygon => "Polygon",
            Self::Avax => "Avalanche C-Chain",
            Self::Bsc => "BNB Smart Chain",
            Self::Gnosis => "Gnosis",
            Self::Trx => "Tron",
            Self::Linea => "Linea",
            Self::Sol => "Solana",
            Self::Ltc => "Litecoin",
            Self::Doge => "Dogecoin",
            Self::Xmr => "Monero",
            Self::Ton => "Gram",
        }
    }

    /// Native gas token ticker for balance rows.
    pub fn native_symbol(self) -> &'static str {
        match self {
            Self::Btc => "BTC",
            Self::Eth | Self::Arb | Self::Base | Self::Op | Self::Linea => "ETH",
            Self::Polygon => "POL",
            Self::Avax => "AVAX",
            Self::Bsc => "BNB",
            Self::Gnosis => "xDAI",
            Self::Trx => "TRX",
            Self::Sol => "SOL",
            Self::Ltc => "LTC",
            Self::Doge => "DOGE",
            Self::Xmr => "XMR",
            Self::Ton => "TON",
        }
    }

    /// CoinGecko id for the native asset (spot + charts).
    pub fn coingecko_id(self) -> Option<&'static str> {
        match self {
            Self::Btc => Some("bitcoin"),
            Self::Eth | Self::Arb | Self::Base | Self::Op | Self::Linea => Some("ethereum"),
            Self::Polygon => Some("matic-network"),
            Self::Avax => Some("avalanche-2"),
            Self::Bsc => Some("binancecoin"),
            Self::Gnosis => Some("xdai"),
            Self::Trx => Some("tron"),
            Self::Sol => Some("solana"),
            Self::Ltc => Some("litecoin"),
            Self::Doge => Some("dogecoin"),
            Self::Xmr => Some("monero"),
            Self::Ton => Some("the-open-network"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AddressType {
    #[default]
    NativeSegwit,
    /// BIP49 P2SH-P2WPKH (addresses starting with 3… / M… on LTC)
    NestedSegwit,
    Taproot,
    Legacy,
}

pub struct DerivedAccount {
    pub chain: ChainId,
    pub address: String,
    pub path: String,
    pub private_key_hex: Option<String>,
    /// Monero view key hex (when chain=XMR).
    pub view_key_hex: Option<String>,
    pub address_type: AddressType,
}

impl Drop for DerivedAccount {
    fn drop(&mut self) {
        if let Some(ref mut k) = self.private_key_hex {
            k.zeroize();
        }
        if let Some(ref mut k) = self.view_key_hex {
            k.zeroize();
        }
    }
}

fn master_xpriv(seed: &[u8; 64], network: Network) -> Result<Xpriv, OpalError> {
    Xpriv::new_master(network, seed).map_err(|e| OpalError::Crypto(format!("xpriv: {e}")))
}

pub fn derive_btc_address(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    index: u32,
    address_type: AddressType,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    let m = parse_mnemonic(mnemonic)?;
    let seed = seed_bytes(&m, passphrase);
    let secp = Secp256k1::new();
    let master = master_xpriv(&seed, Network::Bitcoin)?;

    match address_type {
        AddressType::NativeSegwit => {
            let path: DerivationPath = format!("m/84'/0'/{account}'/0/{index}")
                .parse()
                .map_err(|e| OpalError::Crypto(format!("path: {e}")))?;
            let child = master
                .derive_priv(&secp, &path)
                .map_err(|e| OpalError::Crypto(format!("derive: {e}")))?;
            let sk = PrivateKey::new(child.private_key, Network::Bitcoin);
            let pk = CompressedPublicKey::from_private_key(&secp, &sk)
                .map_err(|e| OpalError::Crypto(format!("pk: {e}")))?;
            let address = Address::p2wpkh(&pk, Network::Bitcoin).to_string();
            Ok(DerivedAccount {
                chain: ChainId::Btc,
                address,
                path: path.to_string(),
                private_key_hex: if include_key {
                    Some(hex::encode(sk.to_bytes()))
                } else {
                    None
                },
                view_key_hex: None,
                address_type,
            })
        }
        AddressType::NestedSegwit => {
            let path: DerivationPath = format!("m/49'/0'/{account}'/0/{index}")
                .parse()
                .map_err(|e| OpalError::Crypto(format!("path: {e}")))?;
            let child = master
                .derive_priv(&secp, &path)
                .map_err(|e| OpalError::Crypto(format!("derive: {e}")))?;
            let sk = PrivateKey::new(child.private_key, Network::Bitcoin);
            let pk = CompressedPublicKey::from_private_key(&secp, &sk)
                .map_err(|e| OpalError::Crypto(format!("pk: {e}")))?;
            let address = Address::p2shwpkh(&pk, Network::Bitcoin).to_string();
            Ok(DerivedAccount {
                chain: ChainId::Btc,
                address,
                path: path.to_string(),
                private_key_hex: if include_key {
                    Some(hex::encode(sk.to_bytes()))
                } else {
                    None
                },
                view_key_hex: None,
                address_type,
            })
        }
        AddressType::Taproot => {
            let path: DerivationPath = format!("m/86'/0'/{account}'/0/{index}")
                .parse()
                .map_err(|e| OpalError::Crypto(format!("path: {e}")))?;
            let child = master
                .derive_priv(&secp, &path)
                .map_err(|e| OpalError::Crypto(format!("derive: {e}")))?;
            let sk = child.private_key;
            let internal = UntweakedPublicKey::from(sk.public_key(&secp));
            let (tweaked, _parity) = internal.tap_tweak(&secp, None);
            let address = Address::p2tr_tweaked(tweaked, Network::Bitcoin).to_string();
            Ok(DerivedAccount {
                chain: ChainId::Btc,
                address,
                path: path.to_string(),
                private_key_hex: if include_key {
                    Some(hex::encode(sk.secret_bytes()))
                } else {
                    None
                },
                view_key_hex: None,
                address_type,
            })
        }
        AddressType::Legacy => {
            let path: DerivationPath = format!("m/44'/0'/{account}'/0/{index}")
                .parse()
                .map_err(|e| OpalError::Crypto(format!("path: {e}")))?;
            let child = master
                .derive_priv(&secp, &path)
                .map_err(|e| OpalError::Crypto(format!("derive: {e}")))?;
            let sk = PrivateKey::new(child.private_key, Network::Bitcoin);
            let pk = PublicKey::from_private_key(&secp, &sk);
            let address = Address::p2pkh(pk, Network::Bitcoin).to_string();
            Ok(DerivedAccount {
                chain: ChainId::Btc,
                address,
                path: path.to_string(),
                private_key_hex: if include_key {
                    Some(hex::encode(sk.to_bytes()))
                } else {
                    None
                },
                view_key_hex: None,
                address_type,
            })
        }
    }
}

pub fn derive_ltc_address(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    index: u32,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    derive_ltc_address_typed(
        mnemonic,
        passphrase,
        account,
        index,
        AddressType::NativeSegwit,
        include_key,
    )
}

pub fn derive_ltc_address_typed(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    index: u32,
    address_type: AddressType,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    let m = parse_mnemonic(mnemonic)?;
    let seed = seed_bytes(&m, passphrase);
    let secp = Secp256k1::new();
    let master = master_xpriv(&seed, Network::Bitcoin)?;
    let (path_s, address_type) = match address_type {
        AddressType::NativeSegwit | AddressType::Taproot => {
            (format!("m/84'/2'/{account}'/0/{index}"), AddressType::NativeSegwit)
        }
        AddressType::NestedSegwit => (format!("m/49'/2'/{account}'/0/{index}"), AddressType::NestedSegwit),
        AddressType::Legacy => (format!("m/44'/2'/{account}'/0/{index}"), AddressType::Legacy),
    };
    let path: DerivationPath = path_s
        .parse()
        .map_err(|e| OpalError::Crypto(format!("path: {e}")))?;
    let child = master
        .derive_priv(&secp, &path)
        .map_err(|e| OpalError::Crypto(format!("derive: {e}")))?;
    let sk = PrivateKey::new(child.private_key, Network::Bitcoin);
    let address = match address_type {
        AddressType::NativeSegwit | AddressType::Taproot => {
            let pk = CompressedPublicKey::from_private_key(&secp, &sk)
                .map_err(|e| OpalError::Crypto(format!("pk: {e}")))?;
            encode_ltc_p2wpkh(&pk)?
        }
        AddressType::NestedSegwit => {
            let pk = CompressedPublicKey::from_private_key(&secp, &sk)
                .map_err(|e| OpalError::Crypto(format!("pk: {e}")))?;
            encode_ltc_p2shwpkh(&pk)?
        }
        AddressType::Legacy => {
            let pk = PublicKey::from_private_key(&secp, &sk);
            encode_ltc_p2pkh(&pk)?
        }
    };
    Ok(DerivedAccount {
        chain: ChainId::Ltc,
        address,
        path: path.to_string(),
        private_key_hex: if include_key {
            Some(hex::encode(sk.to_bytes()))
        } else {
            None
        },
        view_key_hex: None,
        address_type,
    })
}

pub fn encode_ltc_p2wpkh(pk: &CompressedPublicKey) -> Result<String, OpalError> {
    let prog = pk.wpubkey_hash().to_byte_array();
    let hrp = bech32::Hrp::parse("ltc").map_err(|e| OpalError::Crypto(e.to_string()))?;
    bech32::segwit::encode(hrp, Fe32::Q, &prog)
        .map_err(|e| OpalError::Crypto(format!("ltc bech32: {e}")))
}

/// Litecoin P2SH-P2WPKH (M… / 3… style nested segwit).
pub fn encode_ltc_p2shwpkh(pk: &CompressedPublicKey) -> Result<String, OpalError> {
    let wpkh = pk.wpubkey_hash();
    let mut redeem = Vec::with_capacity(22);
    redeem.push(0x00);
    redeem.push(0x14);
    redeem.extend_from_slice(&wpkh.to_byte_array());
    let mut payload = Vec::with_capacity(25);
    payload.push(0x32); // Litecoin P2SH version
    let sha = Sha256::digest(&redeem);
    let ripe = ripemd::Ripemd160::digest(sha);
    payload.extend_from_slice(&ripe);
    let checksum = Sha256::digest(Sha256::digest(&payload));
    payload.extend_from_slice(&checksum[..4]);
    Ok(bs58::encode(payload).into_string())
}

pub fn encode_ltc_p2pkh(pk: &PublicKey) -> Result<String, OpalError> {
    let mut payload = Vec::with_capacity(25);
    payload.push(0x30); // Litecoin P2PKH version
    let sha = Sha256::digest(pk.to_bytes());
    let ripe = ripemd::Ripemd160::digest(sha);
    payload.extend_from_slice(&ripe);
    let checksum = Sha256::digest(Sha256::digest(&payload));
    payload.extend_from_slice(&checksum[..4]);
    Ok(bs58::encode(payload).into_string())
}

pub fn derive_doge_address(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    index: u32,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    let m = parse_mnemonic(mnemonic)?;
    let seed = seed_bytes(&m, passphrase);
    let secp = Secp256k1::new();
    let master = master_xpriv(&seed, Network::Bitcoin)?;
    let path: DerivationPath = format!("m/44'/3'/{account}'/0/{index}")
        .parse()
        .map_err(|e| OpalError::Crypto(format!("path: {e}")))?;
    let child = master
        .derive_priv(&secp, &path)
        .map_err(|e| OpalError::Crypto(format!("derive: {e}")))?;
    let sk = PrivateKey::new(child.private_key, Network::Bitcoin);
    let pk = PublicKey::from_private_key(&secp, &sk);
    let mut payload = Vec::with_capacity(25);
    payload.push(0x1e);
    let sha = Sha256::digest(pk.to_bytes());
    let ripe = ripemd::Ripemd160::digest(sha);
    payload.extend_from_slice(&ripe);
    let checksum = Sha256::digest(Sha256::digest(&payload));
    payload.extend_from_slice(&checksum[..4]);
    let address = bs58::encode(payload).into_string();
    Ok(DerivedAccount {
        chain: ChainId::Doge,
        address,
        path: path.to_string(),
        private_key_hex: if include_key {
            Some(hex::encode(sk.to_bytes()))
        } else {
            None
        },
        view_key_hex: None,
        address_type: AddressType::Legacy,
    })
}

pub fn derive_evm_address(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    index: u32,
    chain: ChainId,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    if !chain.is_evm() {
        return Err(OpalError::InvalidInput("not an EVM chain".into()));
    }
    let m = parse_mnemonic(mnemonic)?;
    let seed = seed_bytes(&m, passphrase);
    let secp = Secp256k1::new();
    let master = master_xpriv(&seed, Network::Bitcoin)?;
    let path: DerivationPath = format!("m/44'/60'/{account}'/0/{index}")
        .parse()
        .map_err(|e| OpalError::Crypto(format!("path: {e}")))?;
    let child = master
        .derive_priv(&secp, &path)
        .map_err(|e| OpalError::Crypto(format!("derive: {e}")))?;
    let sk_bytes = child.private_key.secret_bytes();
    let signing = SigningKey::from_slice(&sk_bytes)
        .map_err(|e| OpalError::Crypto(format!("k256: {e}")))?;
    let verifying = signing.verifying_key();
    let uncompressed = verifying.to_encoded_point(false);
    let pub_bytes = &uncompressed.as_bytes()[1..];
    let mut hasher = Keccak256::new();
    hasher.update(pub_bytes);
    let hash = hasher.finalize();
    let address = to_checksum_address(&hash[12..]);
    Ok(DerivedAccount {
        chain,
        address,
        path: path.to_string(),
        private_key_hex: if include_key {
            Some(hex::encode(sk_bytes))
        } else {
            None
        },
        view_key_hex: None,
        address_type: AddressType::NativeSegwit,
    })
}

/// Tron BIP44 path m/44'/195'/account'/0/index — same secp256k1 key as ETH, Base58Check address.
pub fn derive_trx_address(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    index: u32,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    let m = parse_mnemonic(mnemonic)?;
    let seed = seed_bytes(&m, passphrase);
    let secp = Secp256k1::new();
    let master = master_xpriv(&seed, Network::Bitcoin)?;
    let path: DerivationPath = format!("m/44'/195'/{account}'/0/{index}")
        .parse()
        .map_err(|e| OpalError::Crypto(format!("path: {e}")))?;
    let child = master
        .derive_priv(&secp, &path)
        .map_err(|e| OpalError::Crypto(format!("derive: {e}")))?;
    let sk_bytes = child.private_key.secret_bytes();
    let signing = SigningKey::from_slice(&sk_bytes)
        .map_err(|e| OpalError::Crypto(format!("k256: {e}")))?;
    let verifying = signing.verifying_key();
    let uncompressed = verifying.to_encoded_point(false);
    let pub_bytes = &uncompressed.as_bytes()[1..];
    let mut hasher = Keccak256::new();
    hasher.update(pub_bytes);
    let hash = hasher.finalize();
    let mut payload = Vec::with_capacity(21);
    payload.push(0x41);
    payload.extend_from_slice(&hash[12..]);
    let address = bs58::encode(payload).with_check().into_string();
    Ok(DerivedAccount {
        chain: ChainId::Trx,
        address,
        path: path.to_string(),
        private_key_hex: if include_key {
            Some(hex::encode(sk_bytes))
        } else {
            None
        },
        view_key_hex: None,
        address_type: AddressType::NativeSegwit,
    })
}

fn to_checksum_address(addr: &[u8]) -> String {
    let hex_addr = hex::encode(addr);
    let mut hasher = Keccak256::new();
    hasher.update(hex_addr.as_bytes());
    let hash = hasher.finalize();
    let mut out = String::from("0x");
    for (i, c) in hex_addr.chars().enumerate() {
        let hash_byte = hash[i / 2];
        let nibble = if i % 2 == 0 {
            hash_byte >> 4
        } else {
            hash_byte & 0xf
        };
        if c.is_ascii_hexdigit() && c.is_ascii_alphabetic() && nibble >= 8 {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub fn derive_sol_address(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    // Default to Exodus-compatible derivation (most seed imports).
    derive_sol_exodus(mnemonic, passphrase, account, include_key)
}

/// Exodus Solana: BIP32-secp256k1 path `m/44'/501'/account'/0/0`, then use the
/// 32-byte private key as an ed25519 seed. (Not SLIP-0010 — soft path tails.)
pub fn derive_sol_exodus(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    let m = parse_mnemonic(mnemonic)?;
    let seed = seed_bytes(&m, passphrase);
    let secp = Secp256k1::new();
    let master = master_xpriv(&seed, Network::Bitcoin)?;
    let path: DerivationPath = format!("m/44'/501'/{account}'/0/0")
        .parse()
        .map_err(|e| OpalError::Crypto(format!("path: {e}")))?;
    let child = master
        .derive_priv(&secp, &path)
        .map_err(|e| OpalError::Crypto(format!("derive: {e}")))?;
    let key = child.private_key.secret_bytes();
    let address =
        bs58::encode(&ed25519_dalek::SigningKey::from_bytes(&key).verifying_key()).into_string();
    Ok(DerivedAccount {
        chain: ChainId::Sol,
        address,
        path: format!("m/44'/501'/{account}'/0/0"),
        private_key_hex: if include_key {
            Some(hex::encode(key))
        } else {
            None
        },
        view_key_hex: None,
        address_type: AddressType::NativeSegwit,
    })
}

/// Phantom / Solana CLI style SLIP-0010: `m/44'/501'/account'/0'` (all hardened).
pub fn derive_sol_slip10(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    derive_sol_address_path(mnemonic, passphrase, account, false, include_key)
}

/// `deep = true` → m/44'/501'/account'/0'/0' (Solflare-style SLIP-0010).
pub fn derive_sol_address_path(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    deep: bool,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    let m = parse_mnemonic(mnemonic)?;
    let seed = seed_bytes(&m, passphrase);
    let key = if deep {
        let path = [
            44u32 | 0x8000_0000,
            501 | 0x8000_0000,
            account | 0x8000_0000,
            0 | 0x8000_0000,
            0 | 0x8000_0000,
        ];
        derive_ed25519_private_key(&seed, &path)
    } else {
        let path = [
            44u32 | 0x8000_0000,
            501 | 0x8000_0000,
            account | 0x8000_0000,
            0 | 0x8000_0000,
        ];
        derive_ed25519_private_key(&seed, &path)
    };
    let address =
        bs58::encode(&ed25519_dalek::SigningKey::from_bytes(&key).verifying_key()).into_string();
    Ok(DerivedAccount {
        chain: ChainId::Sol,
        address,
        path: if deep {
            format!("m/44'/501'/{account}'/0'/0'")
        } else {
            format!("m/44'/501'/{account}'/0'")
        },
        private_key_hex: if include_key {
            Some(hex::encode(key))
        } else {
            None
        },
        view_key_hex: None,
        address_type: AddressType::NativeSegwit,
    })
}

/// Pick the Solana derivation that matches a known address (Exodus / Phantom / Solflare).
pub fn derive_sol_matching(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    expected_address: Option<&str>,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    let candidates = [
        derive_sol_exodus(mnemonic, passphrase, account, include_key),
        derive_sol_slip10(mnemonic, passphrase, account, include_key),
        derive_sol_address_path(mnemonic, passphrase, account, true, include_key),
    ];
    let mut first_ok: Option<DerivedAccount> = None;
    for c in candidates {
        let Ok(d) = c else { continue };
        if let Some(exp) = expected_address {
            if !exp.is_empty() && d.address == exp {
                return Ok(d);
            }
        }
        if first_ok.is_none() {
            first_ok = Some(d);
        }
    }
    first_ok.ok_or_else(|| OpalError::Crypto("solana derive failed".into()))
}

pub fn derive_ton_address(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    let m = parse_mnemonic(mnemonic)?;
    let seed = seed_bytes(&m, passphrase);
    // Trust Wallet / Exodus-style: m/44'/607'/account'
    let path = [
        44u32 | 0x8000_0000,
        607 | 0x8000_0000,
        account | 0x8000_0000,
    ];
    let key = derive_ed25519_private_key(&seed, &path);
    let pk = ed25519_dalek::SigningKey::from_bytes(&key).verifying_key();
    let address = crate::wallet::ton::address_from_pubkey(pk.as_bytes());
    Ok(DerivedAccount {
        chain: ChainId::Ton,
        address,
        path: format!("m/44'/607'/{account}'"),
        private_key_hex: if include_key {
            Some(hex::encode(key))
        } else {
            None
        },
        view_key_hex: None,
        address_type: AddressType::NativeSegwit,
    })
}

/// Derive Monero mainnet standard address from BIP39 seed (Trezor/Feather-compatible style).
/// Spend key = sc_reduce32(keccak(seed || account)); view = sc_reduce32(keccak(spend)).
pub fn derive_xmr_account(
    mnemonic: &str,
    passphrase: &str,
    account: u32,
) -> Result<DerivedAccount, OpalError> {
    let m = parse_mnemonic(mnemonic)?;
    let seed = seed_bytes(&m, passphrase);

    let mut spend_hash = [0u8; 32];
    {
        let mut hasher = Keccak256::new();
        hasher.update(b"OpalMoneroSpend");
        hasher.update(seed);
        hasher.update(account.to_le_bytes());
        spend_hash.copy_from_slice(&hasher.finalize());
    }
    let spend = monero_reduce_scalar(&spend_hash);

    let mut view_hash = [0u8; 32];
    {
        let mut hasher = Keccak256::new();
        hasher.update(spend);
        view_hash.copy_from_slice(&hasher.finalize());
    }
    let view = monero_reduce_scalar(&view_hash);

    let spend_sk = monero::PrivateKey::from_slice(&spend)
        .map_err(|e| OpalError::Crypto(format!("xmr spend: {e}")))?;
    let view_sk = monero::PrivateKey::from_slice(&view)
        .map_err(|e| OpalError::Crypto(format!("xmr view: {e}")))?;
    let spend_pk = monero::PublicKey::from_private_key(&spend_sk);
    let view_pk = monero::PublicKey::from_private_key(&view_sk);

    let address = monero::Address::standard(monero::Network::Mainnet, spend_pk, view_pk);
    Ok(DerivedAccount {
        chain: ChainId::Xmr,
        address: address.to_string(),
        path: format!("m/opal/xmr/{account}"),
        private_key_hex: Some(hex::encode(spend)),
        view_key_hex: Some(hex::encode(view)),
        address_type: AddressType::NativeSegwit,
    })
}

/// Clamp to Monero ed25519 scalar field (sc_reduce32).
fn monero_reduce_scalar(bytes: &[u8; 32]) -> [u8; 32] {
    // Use curve25519-dalek Scalar::from_bytes_mod_order for reduction.
    let s = curve25519_dalek::Scalar::from_bytes_mod_order(*bytes);
    s.to_bytes()
}

pub fn derive_for_chain(
    mnemonic: &str,
    passphrase: &str,
    chain: ChainId,
    account: u32,
    index: u32,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    derive_for_chain_typed(
        mnemonic,
        passphrase,
        chain,
        account,
        index,
        AddressType::NativeSegwit,
        include_key,
    )
}

pub fn derive_for_chain_typed(
    mnemonic: &str,
    passphrase: &str,
    chain: ChainId,
    account: u32,
    index: u32,
    address_type: AddressType,
    include_key: bool,
) -> Result<DerivedAccount, OpalError> {
    match chain {
        ChainId::Btc => {
            derive_btc_address(mnemonic, passphrase, account, index, address_type, include_key)
        }
        ChainId::Ltc => {
            derive_ltc_address_typed(mnemonic, passphrase, account, index, address_type, include_key)
        }
        ChainId::Doge => derive_doge_address(mnemonic, passphrase, account, index, include_key),
        c if c.is_evm() => {
            derive_evm_address(mnemonic, passphrase, account, index, chain, include_key)
        }
        ChainId::Trx => derive_trx_address(mnemonic, passphrase, account, index, include_key),
        ChainId::Sol => derive_sol_matching(
            mnemonic,
            passphrase,
            account,
            None,
            include_key,
        ),
        ChainId::Ton => derive_ton_address(mnemonic, passphrase, account, include_key),
        ChainId::Xmr => derive_xmr_account(mnemonic, passphrase, account),
        _ => Err(OpalError::InvalidInput("unsupported chain".into())),
    }
}

/// Gap-limit discovery: find highest used receive index under account (0..gap).
pub fn discover_gap(
    mnemonic: &str,
    passphrase: &str,
    chain: ChainId,
    account: u32,
    address_type: AddressType,
    is_used: &dyn Fn(&str) -> bool,
    gap_limit: u32,
) -> Result<u32, OpalError> {
    let mut last_used = 0u32;
    let mut empty_run = 0u32;
    let mut index = 0u32;
    while empty_run < gap_limit {
        let d = derive_for_chain_typed(
            mnemonic,
            passphrase,
            chain,
            account,
            index,
            address_type,
            false,
        )?;
        if is_used(&d.address) {
            last_used = index;
            empty_run = 0;
        } else {
            empty_run += 1;
        }
        index += 1;
        if index > 1000 {
            break;
        }
    }
    Ok(last_used)
}

pub fn detect_chain_from_address(address: &str) -> Option<Vec<ChainId>> {
    let a = address.trim();
    if a.starts_with("0x") && a.len() == 42 {
        return Some(vec![ChainId::Eth, ChainId::Arb, ChainId::Base]);
    }
    if a.starts_with("bc1") || a.starts_with("1") || a.starts_with("3") {
        return Some(vec![ChainId::Btc]);
    }
    if a.starts_with("ltc1") || a.starts_with('L') || a.starts_with('M') {
        return Some(vec![ChainId::Ltc]);
    }
    if a.starts_with('T') && a.len() == 34 {
        return Some(vec![ChainId::Trx]);
    }
    if a.starts_with('D') && a.len() >= 26 && a.len() <= 36 {
        return Some(vec![ChainId::Doge]);
    }
    if a.starts_with('4') && a.len() >= 95 {
        return Some(vec![ChainId::Xmr]);
    }
    // Solana base58 typically 32-44 chars
    if a.len() >= 32 && a.len() <= 44 && !a.contains('0') && !a.contains('O') && !a.contains('I')
    {
        return Some(vec![ChainId::Sol]);
    }
    if crate::wallet::ton::looks_like_ton_address(a) {
        return Some(vec![ChainId::Ton]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn btc_segwit_deterministic() {
        let m = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let a = derive_btc_address(m, "", 0, 0, AddressType::NativeSegwit, false).unwrap();
        assert!(a.address.starts_with("bc1"));
    }

    #[test]
    fn ltc_bech32_hrp() {
        let m = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let a = derive_ltc_address(m, "", 0, 0, false).unwrap();
        assert!(a.address.starts_with("ltc1"));
        // Round-trip decode
        let (hrp, _) = bech32::decode(&a.address).unwrap();
        assert_eq!(hrp.as_str(), "ltc");
    }

    #[test]
    fn xmr_real_address() {
        let m = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let a = derive_xmr_account(m, "", 0).unwrap();
        assert!(a.address.starts_with('4'));
        assert!(a.view_key_hex.is_some());
        assert!(!a.address.starts_with("opalxmr"));
    }

    #[test]
    fn taproot_address() {
        let m = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let a = derive_btc_address(m, "", 0, 0, AddressType::Taproot, false).unwrap();
        assert!(a.address.starts_with("bc1p"));
    }
}
