//! Discover funded portfolios from a connected Trezor (no vault seed).
//!
//! Addresses come from on-device GetAddress (`show_display=false`); activity
//! checks reuse [`super::discover::address_is_active`].

use std::collections::HashSet;

use serde::Serialize;
use uuid::Uuid;

use crate::error::OpalError;
use crate::network::HttpCtx;
use crate::trezor;
use crate::vault::{PortfolioKind, PortfolioRecord, VaultPayload};
use crate::wallet::discover::{address_is_active, address_type_key, portfolio_name, styles_for};
use crate::wallet::hd::{AddressType, ChainId};

const MAX_ACCOUNTS: u32 = 2;
const UTXO_GAP: u32 = 5;

const TREZOR_CHAINS: &[ChainId] = &[
    ChainId::Btc,
    ChainId::Eth,
    ChainId::Polygon,
    ChainId::Bsc,
    ChainId::Arb,
    ChainId::Base,
    ChainId::Avax,
    ChainId::Gnosis,
    ChainId::Linea,
    ChainId::Ltc,
    ChainId::Doge,
    ChainId::Sol,
    ChainId::Trx,
    ChainId::Xmr,
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrezorDiscoverProgress {
    pub chain: String,
    pub account: u32,
    pub detail: String,
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

fn trezor_script(address_type: AddressType) -> &'static str {
    match address_type {
        AddressType::Taproot => "SPENDTAPROOT",
        AddressType::Legacy => "SPENDADDRESS",
        AddressType::NestedSegwit => "SPENDP2SHWITNESS",
        AddressType::NativeSegwit => "SPENDWITNESS",
    }
}

fn btc_path(account: u32, index: u32, address_type: AddressType) -> String {
    match address_type {
        AddressType::Taproot => format!("m/86'/0'/{account}'/0/{index}"),
        AddressType::Legacy => format!("m/44'/0'/{account}'/0/{index}"),
        AddressType::NestedSegwit => format!("m/49'/0'/{account}'/0/{index}"),
        AddressType::NativeSegwit => format!("m/84'/0'/{account}'/0/{index}"),
    }
}

fn fetch_trezor_address(
    chain: ChainId,
    account: u32,
    index: u32,
    address_type: AddressType,
) -> Result<(String, Option<String>), OpalError> {
    match chain {
        ChainId::Eth
        | ChainId::Arb
        | ChainId::Base
        | ChainId::Op
        | ChainId::Polygon
        | ChainId::Avax
        | ChainId::Bsc
        | ChainId::Gnosis
        | ChainId::Linea => {
            let path = format!("m/44'/60'/{account}'/0/{index}");
            Ok((trezor::trezor_get_ethereum_address(&path, false)?, None))
        }
        ChainId::Btc => {
            let path = btc_path(account, index, address_type);
            Ok((
                trezor::trezor_get_bitcoin_address(
                    "Bitcoin",
                    &path,
                    trezor_script(address_type),
                    false,
                )?,
                None,
            ))
        }
        ChainId::Ltc => {
            let path = format!("m/84'/2'/{account}'/0/{index}");
            Ok((
                trezor::trezor_get_bitcoin_address("Litecoin", &path, "SPENDWITNESS", false)?,
                None,
            ))
        }
        ChainId::Doge => {
            let path = format!("m/44'/3'/{account}'/0/{index}");
            Ok((
                trezor::trezor_get_bitcoin_address("Dogecoin", &path, "SPENDADDRESS", false)?,
                None,
            ))
        }
        ChainId::Sol => {
            let path = format!("m/44'/501'/{account}'/0'");
            Ok((trezor::trezor_get_solana_address(&path, false)?, None))
        }
        ChainId::Trx => {
            let path = format!("m/44'/195'/{account}'/0/0");
            Ok((trezor::trezor_get_tron_address(&path, false)?, None))
        }
        ChainId::Xmr => {
            let (addr, watch) = trezor::trezor_monero_credentials(account)?;
            Ok((addr, Some(watch)))
        }
        _ => Err(OpalError::InvalidInput(format!(
            "Trezor discovery does not support {}",
            chain.as_str()
        ))),
    }
}

fn push_trezor_record(
    payload: &mut VaultPayload,
    probe: &Probe,
    label: Option<String>,
    created: &mut Vec<PortfolioRecord>,
) {
    if payload.portfolios.iter().any(|p| {
        p.chain == probe.chain.as_str() && p.address.as_deref() == Some(probe.address.as_str())
    }) {
        return;
    }

    let record = PortfolioRecord {
        id: Uuid::new_v4().to_string(),
        name: portfolio_name(probe.chain, probe.account, probe.address_type),
        kind: PortfolioKind::Trezor,
        chain: probe.chain.as_str().into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        account_index: probe.account,
        address_index: probe.address_index,
        address: Some(probe.address.clone()),
        xmr_view_key: probe.xmr_view_key.clone(),
        notes: None,
        trezor_label: label,
        address_type: match probe.chain {
            ChainId::Btc | ChainId::Ltc => address_type_key(probe.address_type).map(|s| s.into()),
            _ => None,
        },
        cached_balances_json: None,
    };
    payload.portfolios.push(record.clone());
    created.push(record);
}

/// Scan the connected Trezor; only keep addresses with funds or history.
///
/// When `quiet` is true, only *new* funded accounts are added (used on app restart).
pub fn discover_trezor_portfolios(
    http: &HttpCtx,
    payload: &mut VaultPayload,
    quiet: bool,
    mut on_progress: impl FnMut(TrezorDiscoverProgress),
) -> Result<Vec<PortfolioRecord>, OpalError> {
    let status = trezor::probe_trezor();
    if !status.available || status.device_count == 0 {
        return Err(OpalError::InvalidInput(
            status
                .message
                .if_empty("No Trezor connected — plug in and unlock your device"),
        ));
    }
    let label = status
        .device_label
        .clone()
        .or_else(|| status.device_model.clone())
        .or_else(|| Some("Trezor".into()));

    let mut created = Vec::new();
    let mut seen = HashSet::<String>::new();
    let mut seen_group = HashSet::<String>::new();

    for &chain in TREZOR_CHAINS {
        for account in 0..MAX_ACCOUNTS {
            let styles = styles_for(chain);
            for &address_type in styles {
                let max_index = if chain.is_utxo() { UTXO_GAP } else { 1 };
                for index in 0..max_index {
                    on_progress(TrezorDiscoverProgress {
                        chain: chain.as_str().into(),
                        account,
                        detail: format!(
                            "Checking {} account {account}{}",
                            chain.as_str().to_ascii_uppercase(),
                            if chain.is_utxo() {
                                format!(" index {index}")
                            } else {
                                String::new()
                            }
                        ),
                    });

                    let (address, xmr_view) =
                        match fetch_trezor_address(chain, account, index, address_type) {
                            Ok(v) => v,
                            Err(e) => {
                                if quiet {
                                    continue;
                                }
                                on_progress(TrezorDiscoverProgress {
                                    chain: chain.as_str().into(),
                                    account,
                                    detail: format!("Skipped: {e}"),
                                });
                                continue;
                            }
                        };

                    let key = format!("{}:{}", chain.as_str(), address);
                    if !seen.insert(key) {
                        continue;
                    }

                    let active = if chain == ChainId::Xmr {
                        if let Some(ref view) = xmr_view {
                            match crate::wallet::send::xmr_balance(
                                http,
                                None,
                                view,
                                &address,
                                "",
                            ) {
                                Ok(bal) => {
                                    let n: f64 = bal.parse().unwrap_or(0.0);
                                    n > 0.0
                                }
                                Err(_) => true, // keep address if we got watch key (user can sync later)
                            }
                        } else {
                            false
                        }
                    } else {
                        address_is_active(http, chain, &address)
                    };

                    if !active {
                        if chain.is_utxo() && index > 0 {
                            break;
                        }
                        continue;
                    }

                    let group = format!(
                        "{}:{}:{}",
                        chain.as_str(),
                        account,
                        address_type_key(address_type).unwrap_or("")
                    );
                    if !seen_group.insert(group) {
                        continue;
                    }

                    let probe = Probe {
                        chain,
                        account,
                        address_index: index,
                        address_type,
                        address,
                        xmr_view_key: xmr_view,
                    };
                    let before = payload.portfolios.len();
                    push_trezor_record(payload, &probe, label.clone(), &mut created);
                    if quiet && payload.portfolios.len() == before {
                        // Already present — not a "new" find.
                    }
                }
            }
        }
    }

    let _ = quiet;
    Ok(created)
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.into()
        } else {
            self
        }
    }
}
