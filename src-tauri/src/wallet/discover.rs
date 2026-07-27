//! Post-restore software portfolio discovery.
//!
//! Only creates portfolios for addresses that have balance and/or prior
//! on-chain activity. Probes Exodus-compatible Solana derivation and
//! common BTC/LTC address styles.

use std::collections::HashSet;
use std::thread;

use uuid::Uuid;

use crate::error::OpalError;
use crate::network::{token_contract, HttpCtx};
use crate::vault::{PortfolioKind, PortfolioRecord, VaultPayload};
use crate::wallet::hd::{
    derive_for_chain_typed, derive_sol_address_path, derive_sol_exodus, derive_sol_slip10,
    AddressType, ChainId,
};
use crate::wallet::service::passphrase_from_payload;

const DISCOVER_CHAINS: &[ChainId] = &[
    ChainId::Btc,
    ChainId::Eth,
    ChainId::Polygon,
    ChainId::Bsc,
    ChainId::Arb,
    ChainId::Base,
    ChainId::Trx,
    ChainId::Sol,
    ChainId::Ton,
    ChainId::Ltc,
    ChainId::Doge,
];

const MAX_ACCOUNTS: u32 = 2;
const UTXO_GAP: u32 = 12;

fn chain_label(chain: ChainId) -> &'static str {
    match chain {
        ChainId::Btc => "Bitcoin",
        ChainId::Eth => "Ethereum",
        ChainId::Polygon => "Polygon",
        ChainId::Bsc => "BNB Smart Chain",
        ChainId::Trx => "Tron",
        ChainId::Sol => "Solana",
        ChainId::Ton => "Gram",
        ChainId::Ltc => "Litecoin",
        ChainId::Doge => "Dogecoin",
        ChainId::Xmr => "Monero",
        ChainId::Arb => "Arbitrum",
        ChainId::Base => "Base",
        ChainId::Op => "Optimism",
        ChainId::Avax => "Avalanche",
        ChainId::Gnosis => "Gnosis",
        ChainId::Linea => "Linea",
    }
}

fn address_type_label(t: AddressType) -> Option<&'static str> {
    match t {
        AddressType::NativeSegwit => None,
        AddressType::NestedSegwit => Some("Nested SegWit"),
        AddressType::Taproot => Some("Taproot"),
        AddressType::Legacy => Some("Legacy"),
    }
}

pub fn portfolio_name(chain: ChainId, account: u32, address_type: AddressType) -> String {
    let base = chain_label(chain);
    let style = address_type_label(address_type);
    match (account, style) {
        (0, None) => base.into(),
        (0, Some(s)) => format!("{base} ({s})"),
        (n, None) => format!("{base} #{n}"),
        (n, Some(s)) => format!("{base} #{n} ({s})"),
    }
}

pub fn address_type_key(t: AddressType) -> Option<&'static str> {
    match t {
        AddressType::NativeSegwit => Some("native_segwit"),
        AddressType::NestedSegwit => Some("nested_segwit"),
        AddressType::Taproot => Some("taproot"),
        AddressType::Legacy => Some("legacy"),
    }
}

pub fn styles_for(chain: ChainId) -> &'static [AddressType] {
    match chain {
        ChainId::Btc => &[
            AddressType::NativeSegwit,
            AddressType::NestedSegwit,
            AddressType::Taproot,
            AddressType::Legacy,
        ],
        ChainId::Ltc => &[
            AddressType::NativeSegwit,
            AddressType::NestedSegwit,
            AddressType::Legacy,
        ],
        _ => &[AddressType::NativeSegwit],
    }
}

/// True when the address has a non-zero balance and/or prior on-chain activity.
pub fn address_is_active(http: &HttpCtx, chain: ChainId, address: &str) -> bool {
    if chain.is_evm() {
        if http.evm_balance_wei(chain, address).unwrap_or(0) > 0 {
            return true;
        }
        for sym in ["USDC", "USDT", "DAI"] {
            if let Some(contract) = token_contract(chain, sym) {
                if http.evm_erc20_balance(chain, contract, address).unwrap_or(0) > 0 {
                    return true;
                }
            }
        }
        return false;
    }

    match chain {
        ChainId::Btc | ChainId::Ltc => match http.get_json(&format!(
            "{}/address/{}",
            http.chain_rpc(chain),
            address
        )) {
            Ok(v) => {
                let funded = v["chain_stats"]["funded_txo_sum"].as_u64().unwrap_or(0);
                let spent = v["chain_stats"]["spent_txo_sum"].as_u64().unwrap_or(0);
                let mem_f = v["mempool_stats"]["funded_txo_sum"].as_u64().unwrap_or(0);
                let mem_s = v["mempool_stats"]["spent_txo_sum"].as_u64().unwrap_or(0);
                let balance = funded.saturating_sub(spent) + mem_f.saturating_sub(mem_s);
                let txs = v["chain_stats"]["tx_count"].as_u64().unwrap_or(0);
                balance > 0 || txs > 0
            }
            Err(_) => http.btc_address_balance(chain, address).unwrap_or(0) > 0,
        },
        ChainId::Doge => http.btc_address_balance(chain, address).unwrap_or(0) > 0,
        ChainId::Sol => {
            if http.sol_balance_lamports(address).unwrap_or(0) > 0 {
                return true;
            }
            let tokens = http.sol_all_token_balances(address).unwrap_or_default();
            if tokens.values().any(|v| *v > 0) {
                return true;
            }
            http.sol_address_has_sigs(address)
        }
        ChainId::Trx => {
            if http.trx_balance_sun(address).unwrap_or(0) > 0 {
                return true;
            }
            for sym in ["USDT", "USDC"] {
                if let Some(contract) = token_contract(ChainId::Trx, sym) {
                    if http.trx_trc20_balance(address, contract).unwrap_or(0) > 0 {
                        return true;
                    }
                }
            }
            false
        }
        ChainId::Ton => http.ton_balance_nanoton(address).unwrap_or(0) > 0,
        ChainId::Xmr => {
            // Lightweight: any non-empty address with a previous view-key wallet
            // balance is handled elsewhere; for discovery probe via public
            // explorer-less check — treat as active if wallet-rpc can see funds
            // once a view key is known. During discovery we only have the address,
            // so skip false negatives by returning false here and relying on
            // Monero watch-key + balance scrape after import for funded detection
            // when possible. Prefer probing via known view key path in trezor
            // discovery instead.
            false
        }
        _ => false,
    }
}

#[derive(Clone)]
struct Probe {
    chain: ChainId,
    account: u32,
    address_index: u32,
    address_type: AddressType,
    address: String,
    xmr_view_key: Option<String>,
}

fn push_record(payload: &mut VaultPayload, probe: &Probe, created: &mut Vec<PortfolioRecord>) {
    if payload.portfolios.iter().any(|p| {
        p.kind == PortfolioKind::Software
            && p.chain == probe.chain.as_str()
            && p.address.as_deref() == Some(probe.address.as_str())
    }) {
        return;
    }

    let record = PortfolioRecord {
        id: Uuid::new_v4().to_string(),
        name: portfolio_name(probe.chain, probe.account, probe.address_type),
        kind: PortfolioKind::Software,
        chain: probe.chain.as_str().into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        account_index: probe.account,
        address_index: probe.address_index,
        address: Some(probe.address.clone()),
        xmr_view_key: probe.xmr_view_key.clone(),
        notes: None,
        trezor_label: None,
        address_type: match probe.chain {
            ChainId::Btc | ChainId::Ltc => address_type_key(probe.address_type).map(|s| s.into()),
            _ => None,
        },
        cached_balances_json: None,
    };
    payload.portfolios.push(record.clone());
    created.push(record);
}

fn build_scan_probes(mnemonic: &str, passphrase: &str) -> Vec<Probe> {
    let mut out = Vec::new();
    let mut seen = HashSet::<String>::new();

    for &chain in DISCOVER_CHAINS {
        if chain == ChainId::Sol {
            continue; // handled below with Exodus + SLIP-0010 variants
        }
        for account in 0..MAX_ACCOUNTS {
            for &address_type in styles_for(chain) {
                let max_index = if chain.is_utxo() { UTXO_GAP } else { 1 };
                for index in 0..max_index {
                    let Ok(derived) = derive_for_chain_typed(
                        mnemonic,
                        passphrase,
                        chain,
                        account,
                        index,
                        address_type,
                        false,
                    ) else {
                        continue;
                    };
                    let key = format!("{}:{}", chain.as_str(), derived.address);
                    if !seen.insert(key) {
                        continue;
                    }
                    out.push(Probe {
                        chain,
                        account,
                        address_index: index,
                        address_type,
                        address: derived.address.clone(),
                        xmr_view_key: derived.view_key_hex.clone(),
                    });
                }
            }
        }
    }

    // Solana: Exodus (secp256k1 BIP32) first, then Phantom / Solflare SLIP-0010.
    for account in 0..MAX_ACCOUNTS {
        let variants = [
            derive_sol_exodus(mnemonic, passphrase, account, false),
            derive_sol_slip10(mnemonic, passphrase, account, false),
            derive_sol_address_path(mnemonic, passphrase, account, true, false),
        ];
        for derived in variants.into_iter().flatten() {
            let key = format!("sol:{}", derived.address);
            if !seen.insert(key) {
                continue;
            }
            out.push(Probe {
                chain: ChainId::Sol,
                account,
                address_index: 0,
                address_type: AddressType::NativeSegwit,
                address: derived.address.clone(),
                xmr_view_key: None,
            });
        }
    }

    out
}

/// Scan the restored seed; only keep addresses with funds or history.
pub fn discover_funded_portfolios(
    http: &HttpCtx,
    payload: &mut VaultPayload,
) -> Result<Vec<PortfolioRecord>, OpalError> {
    let mnemonic = payload
        .seed_mnemonic
        .clone()
        .ok_or_else(|| OpalError::InvalidInput("create or restore a seed first".into()))?;
    let passphrase = passphrase_from_payload(payload);
    let mut created = Vec::new();

    let probes = build_scan_probes(&mnemonic, &passphrase);
    let mut active: Vec<Probe> = Vec::new();
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(probes.len());
        for probe in probes {
            handles.push(scope.spawn(move || {
                let ok = address_is_active(http, probe.chain, &probe.address);
                (probe, ok)
            }));
        }
        for h in handles {
            if let Ok((probe, true)) = h.join() {
                active.push(probe);
            }
        }
    });

    active.sort_by(|a, b| {
        (
            a.chain.as_str(),
            a.account,
            address_type_key(a.address_type).unwrap_or(""),
            a.address_index,
        )
            .cmp(&(
                b.chain.as_str(),
                b.account,
                address_type_key(b.address_type).unwrap_or(""),
                b.address_index,
            ))
    });

    let mut seen_group = HashSet::<String>::new();
    for probe in active {
        let group = format!(
            "{}:{}:{}",
            probe.chain.as_str(),
            probe.account,
            address_type_key(probe.address_type).unwrap_or("")
        );
        // One portfolio per chain/account/style — lowest receive index wins.
        if !seen_group.insert(group) {
            continue;
        }
        push_record(payload, &probe, &mut created);
    }

    Ok(created)
}
