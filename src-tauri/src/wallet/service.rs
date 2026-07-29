use serde::{Deserialize, Serialize};
use std::thread;

use crate::error::OpalError;
use crate::network::{explorer_tx_url, token_contract, token_decimals_on, FeePreset, HttpCtx, TxRow};
use crate::vault::{PortfolioKind, PortfolioRecord, UnlockedVault, VaultPayload};
use crate::wallet::send::{
    encode_erc20_transfer, eth_token_parse_units, send_btc_like, send_evm_native, send_evm_token,
    send_sol_native, send_sol_token, send_trx_native, send_trx_token, xmr_balance, xmr_history,
    xmr_send,
};
use crate::wallet::{derive_for_chain, ChainId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetBalance {
    pub symbol: String,
    pub amount: String,
    pub decimals: u32,
    pub usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioBalance {
    pub portfolio_id: String,
    pub chain: String,
    pub address: String,
    pub assets: Vec<AssetBalance>,
}

pub fn passphrase_from_vault(v: &UnlockedVault) -> String {
    passphrase_from_payload(&v.payload)
}

pub fn passphrase_from_payload(payload: &VaultPayload) -> String {
    if payload.settings.bip39_passphrase_enabled {
        payload
            .settings
            .bip39_passphrase
            .clone()
            .unwrap_or_default()
    } else {
        String::new()
    }
}

pub fn resolve_address(
    session: &UnlockedVault,
    portfolio: &PortfolioRecord,
) -> Result<String, OpalError> {
    resolve_address_payload(&session.payload, portfolio)
}

pub fn resolve_address_payload(
    payload: &VaultPayload,
    portfolio: &PortfolioRecord,
) -> Result<String, OpalError> {
    if let Some(addr) = &portfolio.address {
        if !addr.is_empty() {
            return Ok(addr.clone());
        }
    }
    if portfolio.kind == PortfolioKind::WatchOnly {
        return Err(OpalError::InvalidInput(
            "watch-only portfolio missing address".into(),
        ));
    }
    if portfolio.kind == PortfolioKind::Trezor {
        return Err(OpalError::InvalidInput(
            "verify receive address on your Trezor first".into(),
        ));
    }
    let mnemonic = payload
        .seed_mnemonic
        .as_ref()
        .ok_or_else(|| OpalError::InvalidInput("create or restore a seed first".into()))?;
    let pass = passphrase_from_payload(payload);
    let chain = ChainId::parse(&portfolio.chain)?;
    let derived = derive_for_chain(
        mnemonic,
        &pass,
        chain,
        portfolio.account_index,
        portfolio.address_index,
        false,
    )?;
    Ok(derived.address.clone())
}

/// Soft fill: skip bad rows instead of aborting the whole vault.
/// Returns true if any portfolio was updated.
pub fn fill_missing_addresses_dirty(session: &mut UnlockedVault) -> bool {
    let mnemonic = session.payload.seed_mnemonic.clone();
    let pass = passphrase_from_vault(session);
    let mut changed = false;
    for portfolio in session.payload.portfolios.iter_mut() {
        if portfolio.kind != PortfolioKind::Software {
            continue;
        }
        if portfolio.address.as_ref().is_some_and(|a| !a.is_empty()) {
            continue;
        }
        let Some(ref mnemonic) = mnemonic else {
            continue;
        };
        let Ok(chain) = ChainId::parse(&portfolio.chain) else {
            continue;
        };
        let address_type = match portfolio.address_type.as_deref() {
            Some("taproot") => crate::wallet::AddressType::Taproot,
            Some("legacy") => crate::wallet::AddressType::Legacy,
            Some("nested_segwit") => crate::wallet::AddressType::NestedSegwit,
            _ => crate::wallet::AddressType::NativeSegwit,
        };
        let Ok(derived) = crate::wallet::derive_for_chain_typed(
            mnemonic,
            &pass,
            chain,
            portfolio.account_index,
            portfolio.address_index,
            address_type,
            false,
        ) else {
            continue;
        };
        portfolio.address = Some(derived.address.clone());
        changed = true;
        if chain == ChainId::Xmr {
            if let Some(view) = derived.view_key_hex.clone() {
                if portfolio.xmr_view_key.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                    portfolio.xmr_view_key = Some(view);
                }
            }
        }
    }
    changed
}

pub fn fill_missing_addresses(session: &mut UnlockedVault) -> Result<(), OpalError> {
    let _ = fill_missing_addresses_dirty(session);
    Ok(())
}

/// Scrapes on-chain amounts for one portfolio without touching prices, so it
/// can run fully in parallel with the (independent) price book. Returns the
/// balance alongside a parallel list of market ids — one per asset — used
/// to attach fiat once prices are ready.
fn fetch_one_portfolio_balance_amounts(
    http: &HttpCtx,
    payload: &VaultPayload,
    portfolio: &PortfolioRecord,
    include_zero_tokens: bool,
) -> Option<(PortfolioBalance, Vec<Option<&'static str>>)> {
    let address = resolve_address_payload(payload, portfolio).ok()?;
    let chain = ChainId::parse(&portfolio.chain).ok()?;
    let mut assets = Vec::new();
    let mut keys: Vec<Option<&'static str>> = Vec::new();

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
            let tokens: [(&str, Option<&str>); 3] = [
                ("USDC", token_contract(chain, "USDC")),
                ("USDT", token_contract(chain, "USDT")),
                ("DAI", token_contract(chain, "DAI")),
            ];
            let (wei_res, token_raws) = thread::scope(|scope| {
                let addr_native = address.as_str();
                let native_h =
                    scope.spawn(move || http.evm_balance_wei(chain, addr_native));
                let mut token_hs = Vec::new();
                for (sym, contract) in tokens {
                    let addr_tok = address.as_str();
                    let c = contract;
                    token_hs.push(scope.spawn(move || {
                        let raw = c
                            .map(|contract| {
                                http.evm_erc20_balance(chain, contract, addr_tok).unwrap_or(0)
                            })
                            .unwrap_or(0);
                        (sym, raw)
                    }));
                }
                let wei = native_h
                    .join()
                    .unwrap_or(Err(OpalError::Io("evm join".into())));
                let token_raws: Vec<(&str, u128)> = token_hs
                    .into_iter()
                    .filter_map(|h| h.join().ok())
                    .collect();
                (wei, token_raws)
            });
            let Ok(wei) = wei_res else {
                return None;
            };
            let native = wei as f64 / 1e18;
            let symbol = chain.native_symbol();
            assets.push(AssetBalance {
                symbol: symbol.into(),
                amount: format_amount(native, 8),
                decimals: 18,
                usd: None,
            });
            keys.push(Some(chain.coingecko_id().unwrap_or("ethereum")));
            for (sym, raw) in token_raws {
                let decimals = token_decimals_on(chain, sym);
                let amt = raw as f64 / 10f64.powi(decimals as i32);
                if amt > 0.0 || include_zero_tokens {
                    let usd_key = match sym {
                        "USDC" => "usd-coin",
                        "USDT" => "tether",
                        "DAI" => "dai",
                        _ => "",
                    };
                    assets.push(AssetBalance {
                        symbol: sym.into(),
                        amount: format_amount(amt, 6),
                        decimals,
                        usd: None,
                    });
                    keys.push(Some(usd_key));
                }
            }
        }
        ChainId::Btc => {
            let sats = match http.btc_address_balance(chain, &address) {
                Ok(v) => v,
                Err(_) => return None,
            };
            let btc = sats as f64 / 1e8;
            assets.push(AssetBalance {
                symbol: "BTC".into(),
                amount: format_amount(btc, 8),
                decimals: 8,
                usd: None,
            });
            keys.push(Some("bitcoin"));
        }
        ChainId::Ltc => {
            let sats = match http.btc_address_balance(chain, &address) {
                Ok(v) => v,
                Err(_) => return None,
            };
            let amt = sats as f64 / 1e8;
            assets.push(AssetBalance {
                symbol: "LTC".into(),
                amount: format_amount(amt, 8),
                decimals: 8,
                usd: None,
            });
            keys.push(Some("litecoin"));
        }
        ChainId::Doge => {
            let sats = match http.btc_address_balance(chain, &address) {
                Ok(v) => v,
                Err(_) => return None,
            };
            let amt = sats as f64 / 1e8;
            assets.push(AssetBalance {
                symbol: "DOGE".into(),
                amount: format_amount(amt, 4),
                decimals: 8,
                usd: None,
            });
            keys.push(Some("dogecoin"));
        }
        ChainId::Sol => {
            // Native + tokens in parallel — sequential used to stack ~2.5s waits.
            // Native RPC failure → omit this portfolio so the UI keeps the last
            // good balance instead of flashing to 0.
            let (native_res, token_accounts) = thread::scope(|scope| {
                let addr = address.as_str();
                let native_h = scope.spawn(move || http.sol_balance_lamports(addr));
                let tokens_h =
                    scope.spawn(move || http.sol_all_token_balances(addr).unwrap_or_default());
                (
                    native_h.join().unwrap_or(Err(OpalError::Io("sol join".into()))),
                    tokens_h.join().unwrap_or_default(),
                )
            });
            let Ok(lamports) = native_res else {
                return None;
            };
            let sol = lamports as f64 / 1e9;
            assets.push(AssetBalance {
                symbol: "SOL".into(),
                amount: format_amount(sol, 6),
                decimals: 9,
                usd: None,
            });
            keys.push(Some("solana"));
            for (sym, mint) in [
                ("USDC", token_contract(ChainId::Sol, "USDC")),
                ("USDT", token_contract(ChainId::Sol, "USDT")),
            ] {
                if let Some(m) = mint {
                    let raw = token_accounts.get(m).copied().unwrap_or(0);
                    let amt = raw as f64 / 1e6;
                    if amt > 0.0 || include_zero_tokens {
                        let key = if sym == "USDC" { "usd-coin" } else { "tether" };
                        assets.push(AssetBalance {
                            symbol: sym.into(),
                            amount: format_amount(amt, 6),
                            decimals: 6,
                            usd: None,
                        });
                        keys.push(Some(key));
                    }
                }
            }
        }
        ChainId::Trx => {
            let tokens: [(&str, Option<&str>); 2] = [
                ("USDT", token_contract(ChainId::Trx, "USDT")),
                ("USDC", token_contract(ChainId::Trx, "USDC")),
            ];
            let (sun, token_raws) = thread::scope(|scope| {
                let addr = address.as_str();
                let native_h = scope.spawn(move || http.trx_balance_sun(addr).unwrap_or(0));
                let mut token_hs = Vec::new();
                for (sym, contract) in tokens {
                    let addr = address.as_str();
                    token_hs.push(scope.spawn(move || {
                        let raw = contract
                            .map(|c| http.trx_trc20_balance(addr, c).unwrap_or(0))
                            .unwrap_or(0);
                        (sym, raw)
                    }));
                }
                let sun = native_h.join().unwrap_or(0);
                let token_raws: Vec<(&str, u128)> =
                    token_hs.into_iter().filter_map(|h| h.join().ok()).collect();
                (sun, token_raws)
            });
            let trx = sun as f64 / 1e6;
            assets.push(AssetBalance {
                symbol: "TRX".into(),
                amount: format_amount(trx, 6),
                decimals: 6,
                usd: None,
            });
            keys.push(Some("tron"));
            for (sym, raw) in token_raws {
                let decimals = token_decimals_on(ChainId::Trx, sym);
                let amt = raw as f64 / 10f64.powi(decimals as i32);
                if amt > 0.0 || include_zero_tokens {
                    let usd_key = if sym == "USDC" { "usd-coin" } else { "tether" };
                    assets.push(AssetBalance {
                        symbol: sym.into(),
                        amount: format_amount(amt, 6),
                        decimals,
                        usd: None,
                    });
                    keys.push(Some(usd_key));
                }
            }
        }
        ChainId::Xmr => {
            let (spend, view) = xmr_keys_for_payload(payload, portfolio);
            // Hard-cap Monero sync. On timeout / missing view key, omit this
            // portfolio so the UI keeps the last good balance (never flash 0).
            let amount_str = match (&view, spend.as_deref()) {
                (Some(view_key), spend_key) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let http = http.clone();
                    let view_key = view_key.clone();
                    let spend_key = spend_key.map(|s| s.to_string());
                    let address = address.clone();
                    std::thread::spawn(move || {
                        let v = xmr_balance(
                            &http,
                            spend_key.as_deref(),
                            &view_key,
                            &address,
                            "",
                        );
                        let _ = tx.send(v);
                    });
                    match rx.recv_timeout(std::time::Duration::from_millis(2_500)) {
                        Ok(Ok(v)) => v,
                        _ => return None,
                    }
                }
                (None, _) => return None,
            };
            let xmr: f64 = amount_str.parse().unwrap_or(0.0);
            assets.push(AssetBalance {
                symbol: "XMR".into(),
                amount: format_amount(xmr, 12),
                decimals: 12,
                usd: None,
            });
            keys.push(Some("monero"));
        }
        ChainId::Ton => {
            let nano = http.ton_balance_nanoton(&address).unwrap_or(0);
            let ton = nano as f64 / 1e9;
            assets.push(AssetBalance {
                symbol: "TON".into(),
                amount: format_amount(ton, 6),
                decimals: 9,
                usd: None,
            });
            keys.push(Some("the-open-network"));
        }
    }

    Some((
        PortfolioBalance {
            portfolio_id: portfolio.id.clone(),
            chain: portfolio.chain.clone(),
            address,
            assets,
        },
        keys,
    ))
}

/// Balance fetch against a payload snapshot — safe to call without holding the session lock.
/// Portfolios are scraped in parallel so one slow chain doesn't serialize the rest.
pub fn fetch_balances_for_payload(
    http: &HttpCtx,
    payload: &VaultPayload,
    portfolio_id: Option<&str>,
) -> Result<Vec<PortfolioBalance>, OpalError> {
    // Always include allowlisted stables (USDC/USDT/DAI/…) even at zero so
    // the home list, sidebar, and swap picker show them by default — users
    // expect the token row to exist before they've ever held any.
    let include_zero_tokens = true;
    let targets: Vec<&PortfolioRecord> = payload
        .portfolios
        .iter()
        .filter(|p| portfolio_id.map(|id| p.id == id).unwrap_or(true))
        .collect();

    if targets.is_empty() {
        return Ok(Vec::new());
    }

    let mut slots: Vec<Option<PortfolioBalance>> = (0..targets.len()).map(|_| None).collect();
    // Price holdings in the vault's display fiat — not hard-coded USD.
    // The AssetBalance.usd field is "fiat value in the selected currency"
    // (legacy name); formatMoney then labels it with the matching code.
    let fiat = {
        let f = payload.settings.fiat.to_ascii_lowercase();
        if f.is_empty() {
            "usd".into()
        } else {
            f
        }
    };
    thread::scope(|scope| {
        // Spot fiat from the in-memory exchange book only — never block a
        // balance scrape on a price network call. Background loop + warm
        // timer keep the book hot (Exodus/Trezor model).
        let prices = http.cached_prices_in_fiat(&fiat);
        let mut amount_handles = Vec::with_capacity(targets.len());
        for (idx, portfolio) in targets.iter().copied().enumerate() {
            amount_handles.push(scope.spawn(move || {
                (idx, fetch_one_portfolio_balance_amounts(http, payload, portfolio, include_zero_tokens))
            }));
        }
        let raw: Vec<(usize, Option<(PortfolioBalance, Vec<Option<&'static str>>)>)> =
            amount_handles.into_iter().filter_map(|h| h.join().ok()).collect();
        for (idx, entry) in raw {
            if let Some((mut bal, keys)) = entry {
                for (asset, key) in bal.assets.iter_mut().zip(keys.iter()) {
                    if let Some(k) = key {
                        if let (Some(p), Ok(amt)) = (prices.get(*k), asset.amount.parse::<f64>()) {
                            asset.usd = Some(p * amt);
                        }
                    }
                }
                slots[idx] = Some(bal);
            }
        }
    });
    Ok(slots.into_iter().flatten().collect())
}

/// Read previously persisted balance snapshots (instant — no network).
pub fn cached_balances_from_payload(
    payload: &VaultPayload,
    portfolio_id: Option<&str>,
) -> Vec<PortfolioBalance> {
    let mut out = Vec::new();
    for portfolio in &payload.portfolios {
        if let Some(id) = portfolio_id {
            if portfolio.id != id {
                continue;
            }
        }
        let Some(raw) = portfolio.cached_balances_json.as_deref() else {
            continue;
        };
        if let Ok(bal) = serde_json::from_str::<PortfolioBalance>(raw) {
            out.push(bal);
        }
    }
    out
}

pub fn fetch_balances(
    http: &HttpCtx,
    session: &mut UnlockedVault,
    portfolio_id: Option<&str>,
) -> Result<Vec<PortfolioBalance>, OpalError> {
    fill_missing_addresses(session)?;
    fetch_balances_for_payload(http, &session.payload, portfolio_id)
}

pub fn fetch_history(
    http: &HttpCtx,
    session: &mut UnlockedVault,
    portfolio_id: &str,
) -> Result<Vec<TxRow>, OpalError> {
    fill_missing_addresses(session)?;
    let portfolio = session
        .payload
        .portfolios
        .iter()
        .find(|p| p.id == portfolio_id)
        .cloned()
        .ok_or_else(|| OpalError::InvalidInput("portfolio not found".into()))?;
    let address = resolve_address(session, &portfolio)?;
    let chain = ChainId::parse(&portfolio.chain)?;
    let xmr = xmr_keys_for_portfolio(session, &portfolio);
    fetch_history_for_address(http, chain, &address, xmr)
}

/// History fetch without holding the session lock.
pub fn fetch_history_for_address(
    http: &HttpCtx,
    chain: ChainId,
    address: &str,
    xmr_keys: (Option<String>, Option<String>),
) -> Result<Vec<TxRow>, OpalError> {
    match chain {
        ChainId::Eth
        | ChainId::Arb
        | ChainId::Base
        | ChainId::Op
        | ChainId::Polygon
        | ChainId::Avax
        | ChainId::Bsc
        | ChainId::Gnosis
        | ChainId::Linea => http.evm_history(chain, address),
        ChainId::Btc | ChainId::Ltc => http.btc_history(chain, address),
        ChainId::Doge => http.doge_history(address),
        ChainId::Sol => http.sol_history(address),
        ChainId::Trx => http.trx_history(address),
        ChainId::Ton => http.ton_history(address),
        ChainId::Xmr => {
            let (spend, view) = xmr_keys;
            let view_key = view.ok_or_else(|| {
                OpalError::InvalidInput("Monero view key required for history".into())
            })?;
            xmr_history(http, spend.as_deref(), &view_key, address, "")
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SendResult {
    pub txid: String,
    pub explorer_url: String,
}

/// Signs and broadcasts a send. Takes an owned snapshot of everything it
/// needs (portfolio record, mnemonic, passphrase) rather than a locked
/// session reference — this can involve several sequential network round
/// trips (fee/UTXO lookups, broadcast), and holding the global session lock
/// for that whole stretch used to freeze the rest of the app (Settings,
/// balance refreshes, switching portfolios, …) until the send finished.
pub fn send_from_portfolio_opts(
    http: &HttpCtx,
    portfolio: &PortfolioRecord,
    mnemonic: Option<&str>,
    passphrase: &str,
    to: &str,
    amount: &str,
    token: Option<&str>,
    utxo_opts: &crate::wallet::send::UtxoSendOptions,
    sol_fee: FeePreset,
) -> Result<SendResult, OpalError> {
    if portfolio.kind == PortfolioKind::WatchOnly {
        return Err(OpalError::InvalidInput("watch-only cannot send".into()));
    }
    let chain = ChainId::parse(&portfolio.chain)?;

    if portfolio.kind == PortfolioKind::Trezor {
        let txid = send_trezor_portfolio(
            http,
            portfolio,
            chain,
            to,
            amount,
            token,
            utxo_opts,
            sol_fee,
        )?;
        return Ok(SendResult {
            explorer_url: explorer_tx_url(chain, &txid),
            txid,
        });
    }

    let mnemonic = mnemonic.ok_or_else(|| OpalError::InvalidInput("no seed".into()))?;
    let pass = passphrase;
    let address_type = portfolio
        .address_type
        .as_deref()
        .and_then(|s| match s {
            "taproot" => Some(crate::wallet::AddressType::Taproot),
            "legacy" => Some(crate::wallet::AddressType::Legacy),
            "nested_segwit" => Some(crate::wallet::AddressType::NestedSegwit),
            _ => Some(crate::wallet::AddressType::NativeSegwit),
        })
        .unwrap_or(crate::wallet::AddressType::NativeSegwit);

    let derived = if chain == ChainId::Sol {
        crate::wallet::derive_sol_matching(
            mnemonic,
            pass,
            portfolio.account_index,
            portfolio.address.as_deref(),
            true,
        )?
    } else {
        crate::wallet::derive_for_chain_typed(
            mnemonic,
            pass,
            chain,
            portfolio.account_index,
            portfolio.address_index,
            address_type,
            true,
        )?
    };
    let sk_hex = derived
        .private_key_hex
        .clone()
        .ok_or_else(|| OpalError::Crypto("missing key".into()))?;

    let native = chain.native_symbol();
    let txid = match chain {
        ChainId::Eth
        | ChainId::Arb
        | ChainId::Base
        | ChainId::Op
        | ChainId::Polygon
        | ChainId::Avax
        | ChainId::Bsc
        | ChainId::Gnosis
        | ChainId::Linea => {
            if let Some(sym) = token.filter(|s| !s.eq_ignore_ascii_case(native)) {
                send_evm_token(http, chain, &sk_hex, &derived.address, to, amount, sym)?
            } else {
                send_evm_native(
                    http,
                    chain,
                    &sk_hex,
                    &derived.address,
                    to,
                    amount,
                    utxo_opts.send_max,
                )?
            }
        }
        ChainId::Btc | ChainId::Ltc | ChainId::Doge => send_btc_like(
            http,
            chain,
            mnemonic,
            pass,
            portfolio.account_index,
            portfolio.address_index,
            to,
            amount,
            address_type,
            utxo_opts,
        )?,
        ChainId::Sol => {
            if let Some(sym) = token.filter(|s| !s.eq_ignore_ascii_case("SOL")) {
                send_sol_token(http, &sk_hex, &derived.address, to, amount, sym)?
            } else {
                send_sol_native(
                    http,
                    &sk_hex,
                    &derived.address,
                    to,
                    amount,
                    sol_fee,
                    utxo_opts.send_max,
                )?
            }
        }
        ChainId::Trx => {
            if let Some(sym) = token.filter(|s| !s.eq_ignore_ascii_case("TRX")) {
                send_trx_token(http, &sk_hex, &derived.address, to, amount, sym)?
            } else {
                send_trx_native(http, &sk_hex, &derived.address, to, amount)?
            }
        }
        ChainId::Xmr => {
            let view = derived
                .view_key_hex
                .as_ref()
                .ok_or_else(|| OpalError::Crypto("missing XMR view key".into()))?;
            xmr_send(http, &sk_hex, view, &derived.address, "", to, amount)?
        }
        ChainId::Ton => {
            return Err(OpalError::InvalidInput(
                "TON sending isn't wired up yet — receive and balances work; use TON Keeper to send for now"
                    .into(),
            ));
        }
    };

    Ok(SendResult {
        explorer_url: explorer_tx_url(chain, &txid),
        txid,
    })
}

fn format_amount(v: f64, places: usize) -> String {
    let s = format!("{v:.places$}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn send_trezor_portfolio(
    http: &HttpCtx,
    portfolio: &PortfolioRecord,
    chain: ChainId,
    to: &str,
    amount: &str,
    token: Option<&str>,
    utxo_opts: &crate::wallet::send::UtxoSendOptions,
    sol_fee: FeePreset,
) -> Result<String, OpalError> {
    use crate::network::tron_address_to_hex;
    use crate::trezor::{
        trezor_sign_ethereum_tx, trezor_sign_solana_tx, trezor_sign_tron_tx, EthereumTxParams,
        TronSignParams, TronTransferParams,
    };
    use crate::wallet::send::{
        broadcast_sol_with_signature, broadcast_trx_with_signature, build_sol_native_message,
        create_trx_native_unsigned, send_btc_like_trezor,
    };

    fn tron_addr_bytes(address: &str) -> Result<Vec<u8>, OpalError> {
        let hex = tron_address_to_hex(address)?;
        hex::decode(hex).map_err(|e| OpalError::InvalidInput(format!("tron address bytes: {e}")))
    }

    let native = chain.native_symbol();
    let from = portfolio
        .address
        .as_ref()
        .ok_or_else(|| OpalError::InvalidInput("Trezor portfolio missing address".into()))?;

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
            let path = format!(
                "m/44'/60'/{}'/0/{}",
                portfolio.account_index, portfolio.address_index
            );
            let chain_id = chain
                .chain_id_u64()
                .ok_or_else(|| OpalError::InvalidInput("not evm".into()))?;

            let (to_addr, value_wei, data_hex, gas_limit_hex) =
                if let Some(sym) = token.filter(|s| !s.eq_ignore_ascii_case(native)) {
                    let contract = token_contract(chain, sym).ok_or_else(|| {
                        OpalError::InvalidInput(format!("token {sym} not allowlisted"))
                    })?;
                    let decimals = token_decimals_on(chain, sym);
                    let raw = eth_token_parse_units(amount, decimals)?;
                    let data = encode_erc20_transfer(to, raw)?;
                    (
                        contract.to_string(),
                        0u128,
                        Some(format!("0x{}", hex::encode(&data))),
                        "0x186a0",
                    )
                } else {
                    (to.to_string(), parse_eth_amount_to_wei(amount)?, None, "0x5208")
                };

            let nonce_hex = http
                .eth_rpc(chain, "eth_getTransactionCount", serde_json::json!([from, "pending"]))?
                .as_str()
                .unwrap_or("0x0")
                .to_string();
            let gas_price = http
                .eth_rpc(chain, "eth_gasPrice", serde_json::json!([]))?
                .as_str()
                .unwrap_or("0x3b9aca00")
                .to_string();
            let tip = crate::network::u128_from_hex(&gas_price).unwrap_or(1_000_000_000) / 10;
            let max_fee = crate::network::u128_from_hex(&gas_price).unwrap_or(2_000_000_000) * 2;
            let params = EthereumTxParams {
                path,
                to: to_addr,
                value_wei_hex: format!("0x{value_wei:x}"),
                nonce_hex,
                gas_limit_hex: gas_limit_hex.into(),
                chain_id,
                data_hex,
                gas_price_hex: None,
                max_fee_per_gas_hex: Some(format!("0x{max_fee:x}")),
                max_priority_fee_per_gas_hex: Some(format!("0x{tip:x}")),
            };
            let raw = trezor_sign_ethereum_tx(&params)?;
            http.broadcast_evm(chain, &raw)
        }
        ChainId::Btc | ChainId::Ltc | ChainId::Doge => {
            if token.is_some() {
                return Err(OpalError::InvalidInput("UTXO chains have no tokens".into()));
            }
            let address_type = portfolio
                .address_type
                .as_deref()
                .and_then(|s| match s {
                    "taproot" => Some(crate::wallet::AddressType::Taproot),
                    "legacy" => Some(crate::wallet::AddressType::Legacy),
                    "nested_segwit" => Some(crate::wallet::AddressType::NestedSegwit),
                    _ => Some(crate::wallet::AddressType::NativeSegwit),
                })
                .unwrap_or(crate::wallet::AddressType::NativeSegwit);
            send_btc_like_trezor(
                http,
                chain,
                from,
                portfolio.account_index,
                portfolio.address_index,
                to,
                amount,
                address_type,
                utxo_opts,
            )
        }
        ChainId::Sol => {
            if token.is_some_and(|s| !s.eq_ignore_ascii_case("SOL")) {
                return Err(OpalError::InvalidInput(
                    "Trezor Solana token sends are not supported yet — send native SOL".into(),
                ));
            }
            let path = format!("m/44'/501'/{}'/0'", portfolio.account_index);
            let message = build_sol_native_message(http, from, to, amount, sol_fee)?;
            // Trezor SolanaSignTx expects the full serialized tx in some firmwares,
            // and the message in others — Suite sends the message bytes as serialized_tx.
            let sig = trezor_sign_solana_tx(&path, &message)?;
            broadcast_sol_with_signature(http, &sig, &message)
        }
        ChainId::Trx => {
            if token.is_some_and(|s| !s.eq_ignore_ascii_case("TRX")) {
                return Err(OpalError::InvalidInput(
                    "Trezor TRC-20 sends are not supported yet — send native TRX".into(),
                ));
            }
            let path = format!("m/44'/195'/{}'/0/0", portfolio.account_index);
            let tx = create_trx_native_unsigned(http, from, to, amount)?;
            let raw = tx
                .get("raw_data")
                .ok_or_else(|| OpalError::Io("missing raw_data".into()))?;
            let ref_block_bytes = hex::decode(raw["ref_block_bytes"].as_str().unwrap_or(""))
                .map_err(|e| OpalError::Io(format!("ref_block_bytes: {e}")))?;
            let ref_block_hash = hex::decode(raw["ref_block_hash"].as_str().unwrap_or(""))
                .map_err(|e| OpalError::Io(format!("ref_block_hash: {e}")))?;
            let expiration = raw["expiration"].as_u64().unwrap_or(0);
            let timestamp = raw["timestamp"].as_u64().unwrap_or(0);
            let sun = raw["contract"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|c| c["parameter"]["value"]["amount"].as_u64())
                .unwrap_or(0);
            if sun == 0 {
                return Err(OpalError::InvalidInput("could not read TRX amount".into()));
            }
            let params = TronSignParams {
                path,
                ref_block_bytes,
                ref_block_hash,
                expiration,
                timestamp,
                fee_limit: None,
                data: None,
                transfer: Some(TronTransferParams {
                    owner_address: tron_addr_bytes(from)?,
                    to_address: tron_addr_bytes(to)?,
                    amount_sun: sun,
                }),
                trigger: None,
            };
            let sig = trezor_sign_tron_tx(&params)?;
            broadcast_trx_with_signature(http, tx, &sig)
        }
        ChainId::Xmr => {
            let view = portfolio.xmr_view_key.as_ref().ok_or_else(|| {
                OpalError::InvalidInput(
                    "Monero Trezor portfolio is missing its watch key — Sync my Trezor again".into(),
                )
            })?;
            if !crate::trezor::trezor_monero_supported()? {
                return Err(OpalError::InvalidInput(
                    "Monero on Trezor needs a Model T or Safe device — connect and unlock it"
                        .into(),
                ));
            }
            crate::wallet::send::xmr_send_trezor(
                http,
                view,
                from,
                portfolio.account_index,
                to,
                amount,
            )
        }
        _ => Err(OpalError::InvalidInput(format!(
            "Trezor {} spending isn't available in Opal",
            chain.as_str()
        ))),
    }
}

fn parse_eth_amount_to_wei(amount: &str) -> Result<u128, OpalError> {
    let amount = amount.trim();
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
    if frac.len() > 18 {
        frac.truncate(18);
    }
    while frac.len() < 18 {
        frac.push('0');
    }
    let frac_n: u128 = frac
        .parse()
        .map_err(|_| OpalError::InvalidInput("bad amount".into()))?;
    whole
        .checked_mul(10u128.pow(18))
        .and_then(|v| v.checked_add(frac_n))
        .ok_or_else(|| OpalError::InvalidInput("amount overflow".into()))
}

/// Resolve spend (optional) + view keys for an XMR portfolio.
pub fn xmr_keys_for_portfolio(
    session: &UnlockedVault,
    portfolio: &PortfolioRecord,
) -> (Option<String>, Option<String>) {
    xmr_keys_for_payload(&session.payload, portfolio)
}

fn xmr_keys_for_payload(
    payload: &VaultPayload,
    portfolio: &PortfolioRecord,
) -> (Option<String>, Option<String>) {
    if let Some(view) = portfolio
        .xmr_view_key
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return (None, Some(view));
    }
    let mnemonic = match payload.seed_mnemonic.as_ref() {
        Some(m) => m,
        None => return (None, None),
    };
    let pass = passphrase_from_payload(payload);
    match derive_for_chain(
        mnemonic,
        &pass,
        ChainId::Xmr,
        portfolio.account_index,
        portfolio.address_index,
        true,
    ) {
        Ok(d) => (d.private_key_hex.clone(), d.view_key_hex.clone()),
        Err(_) => (None, None),
    }
}
