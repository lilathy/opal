use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::error::OpalError;
use crate::network::HttpCtx;
use crate::state::AppState;
use crate::trezor;
use crate::vault::{PortfolioKind, PortfolioRecord};
use crate::wallet::{
    derive_for_chain, detect_chain_from_address, generate_mnemonic, parse_mnemonic, ChainId,
};
use crate::wallet::service::{
    cached_balances_from_payload, fetch_balances_for_payload, fetch_history_for_address,
    fill_missing_addresses, fill_missing_addresses_dirty, resolve_address, PortfolioBalance,
    SendResult,
};
use crate::network::TxRow;

fn map_err(e: OpalError) -> String {
    e.into()
}

fn require_unlocked(state: &AppState) -> Result<(), String> {
    if state.session.lock().is_none() {
        return Err(map_err(OpalError::Locked));
    }
    Ok(())
}

fn http_from_session(session: &crate::vault::UnlockedVault) -> Result<HttpCtx, String> {
    HttpCtx::new(
        session.payload.settings.tor_socks.as_deref(),
        std::collections::HashMap::new(),
    )
    .map_err(map_err)
}

/// Run blocking work (HTTP, Argon2, …) on a worker thread so the webview /
/// async runtime never stalls. Sync Tauri commands that call `reqwest::blocking`
/// otherwise freeze the whole UI while a scrape or fee estimate is in flight.
async fn on_worker<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

#[derive(Debug, Serialize)]
pub struct ChainInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub tokens: Vec<String>,
}

#[tauri::command]
pub fn chain_list() -> Vec<ChainInfo> {
    vec![
        ChainInfo {
            id: "btc".into(),
            name: "Bitcoin".into(),
            kind: "utxo".into(),
            tokens: vec![],
        },
        ChainInfo {
            id: "eth".into(),
            name: "Ethereum".into(),
            kind: "evm".into(),
            tokens: vec!["USDC".into(), "USDT".into(), "DAI".into()],
        },
        ChainInfo {
            id: "polygon".into(),
            name: "Polygon".into(),
            kind: "evm".into(),
            tokens: vec!["USDC".into(), "USDT".into(), "DAI".into()],
        },
        ChainInfo {
            id: "bsc".into(),
            name: "BNB Smart Chain".into(),
            kind: "evm".into(),
            tokens: vec!["USDC".into(), "USDT".into(), "DAI".into()],
        },
        ChainInfo {
            id: "trx".into(),
            name: "Tron".into(),
            kind: "trx".into(),
            tokens: vec!["USDT".into(), "USDC".into()],
        },
        ChainInfo {
            id: "sol".into(),
            name: "Solana".into(),
            kind: "sol".into(),
            tokens: vec!["USDC".into(), "USDT".into()],
        },
        ChainInfo {
            id: "ton".into(),
            name: "Gram".into(),
            kind: "ton".into(),
            tokens: vec![],
        },
        ChainInfo {
            id: "ltc".into(),
            name: "Litecoin".into(),
            kind: "utxo".into(),
            tokens: vec![],
        },
        ChainInfo {
            id: "doge".into(),
            name: "Dogecoin".into(),
            kind: "utxo".into(),
            tokens: vec![],
        },
        ChainInfo {
            id: "xmr".into(),
            name: "Monero".into(),
            kind: "xmr".into(),
            tokens: vec![],
        },
    ]
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSeedRequest {
    pub word_count: u8,
}

#[tauri::command]
pub fn wallet_create_seed(
    state: State<'_, AppState>,
    request: CreateSeedRequest,
) -> Result<String, String> {
    require_unlocked(&state)?;
    let phrase = generate_mnemonic(request.word_count).map_err(map_err)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    if unlocked.payload.seed_mnemonic.is_some() {
        return Err(map_err(OpalError::InvalidInput(
            "seed already exists — restore only into a new vault".into(),
        )));
    }
    unlocked.payload.seed_mnemonic = Some(phrase.mnemonic.clone());
    unlocked.payload.seed_backed_up = false;
    state.vault.persist(unlocked).map_err(map_err)?;
    Ok(phrase.mnemonic.clone())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreSeedRequest {
    pub mnemonic: String,
    pub passphrase: Option<String>,
}

#[tauri::command]
pub fn wallet_restore_seed(
    state: State<'_, AppState>,
    request: RestoreSeedRequest,
) -> Result<(), String> {
    require_unlocked(&state)?;
    let m = parse_mnemonic(&request.mnemonic).map_err(map_err)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    if unlocked.payload.seed_mnemonic.is_some() {
        return Err(map_err(OpalError::InvalidInput(
            "seed already exists — wipe the vault before recovering a different seed".into(),
        )));
    }
    unlocked.payload.seed_mnemonic = Some(m.to_string());
    unlocked.payload.seed_backed_up = true;
    if let Some(p) = request.passphrase {
        if !p.is_empty() {
            unlocked.payload.settings.bip39_passphrase_enabled = true;
            unlocked.payload.settings.bip39_passphrase = Some(p);
        }
    }
    state.vault.persist(unlocked).map_err(map_err)
}

/// After restore (or any empty software vault), scan default paths and create
/// portfolios for addresses that hold funds / UTXO activity.
#[tauri::command]
pub async fn wallet_discover_portfolios(app: AppHandle) -> Result<Vec<PortfolioRecord>, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, mut payload) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            if unlocked.payload.seed_mnemonic.is_none() {
                return Err(map_err(OpalError::InvalidInput(
                    "create or restore a seed first".into(),
                )));
            }
            let http = HttpCtx::for_discovery(unlocked.payload.settings.tor_socks.as_deref())
                .map_err(map_err)?;
            (http, unlocked.payload.clone())
        };
        let created =
            crate::wallet::discover_funded_portfolios(&http, &mut payload).map_err(map_err)?;
        if created.is_empty() {
            return Ok(created);
        }
        let mut session = state.session.lock();
        let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
        // Re-apply only new rows (session may have changed during network I/O).
        for row in &created {
            let already = unlocked.payload.portfolios.iter().any(|p| {
                p.kind == PortfolioKind::Software
                    && p.chain == row.chain
                    && p.account_index == row.account_index
                    && p.address_type == row.address_type
                    && p.address == row.address
            });
            if !already {
                unlocked.payload.portfolios.push(row.clone());
            }
        }
        let _ = fill_missing_addresses_dirty(unlocked);
        state.vault.persist(unlocked).map_err(map_err)?;
        Ok(created)
    })
    .await
}

#[tauri::command]
pub fn wallet_confirm_backup(state: State<'_, AppState>) -> Result<(), String> {
    require_unlocked(&state)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    unlocked.payload.seed_backed_up = true;
    state.vault.persist(unlocked).map_err(map_err)
}

#[tauri::command]
pub fn wallet_reveal_seed(state: State<'_, AppState>) -> Result<String, String> {
    require_unlocked(&state)?;
    let session = state.session.lock();
    let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
    unlocked
        .payload
        .seed_mnemonic
        .clone()
        .ok_or_else(|| map_err(OpalError::InvalidInput("no seed".into())))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePortfolioRequest {
    pub name: String,
    pub chain: String,
    pub kind: String,
    pub account_index: Option<u32>,
    pub address: Option<String>,
    pub xmr_view_key: Option<String>,
    pub trezor_label: Option<String>,
    pub address_type: Option<String>,
    pub verify_on_device: Option<bool>,
}

#[tauri::command]
pub async fn portfolio_create(
    state: State<'_, AppState>,
    request: CreatePortfolioRequest,
) -> Result<PortfolioRecord, String> {
    require_unlocked(&state)?;
    let chain = ChainId::parse(&request.chain).map_err(map_err)?;
    let kind = match request.kind.as_str() {
        "software" => PortfolioKind::Software,
        "trezor" => PortfolioKind::Trezor,
        "watch_only" => PortfolioKind::WatchOnly,
        _ => {
            return Err(map_err(OpalError::InvalidInput(
                "kind must be software|trezor|watch_only".into(),
            )))
        }
    };

    // In-memory only, no I/O — fine to check with a short-lived lock.
    if kind == PortfolioKind::Software {
        let session = state.session.lock();
        let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
        if unlocked.payload.seed_mnemonic.is_none() {
            return Err(map_err(OpalError::InvalidInput(
                "create or restore seed first".into(),
            )));
        }
        if !unlocked.payload.seed_backed_up {
            return Err(map_err(OpalError::InvalidInput(
                "confirm seed backup before creating software portfolios".into(),
            )));
        }
    }

    let mut address = request.address;
    if kind == PortfolioKind::WatchOnly {
        let addr = address
            .clone()
            .ok_or_else(|| map_err(OpalError::InvalidInput("address required".into())))?;
        if chain == ChainId::Xmr && request.xmr_view_key.as_ref().map(|s| s.is_empty()).unwrap_or(true)
        {
            return Err(map_err(OpalError::InvalidInput(
                "XMR watch-only requires view key".into(),
            )));
        }
        if let Some(detected) = detect_chain_from_address(&addr) {
            if !detected.contains(&chain) {
                return Err(map_err(OpalError::InvalidInput(
                    "address does not match selected chain".into(),
                )));
            }
        }
        address = Some(addr);
    }

    if kind == PortfolioKind::Trezor {
        // Blocking Bridge I/O + waiting on a physical button press on the
        // device — must run off the main thread or the whole app freezes
        // until the user confirms (or it times out).
        let verify = request.verify_on_device.unwrap_or(true);
        let account = request.account_index.unwrap_or(0);
        let address_type = request.address_type.clone();
        let need_fetch = address.as_ref().map(|s| s.is_empty()).unwrap_or(true);
        let existing = address.clone();

        let fetched = tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
            let status = trezor::probe_trezor();
            if !status.available {
                return Err(map_err(OpalError::InvalidInput(status.message)));
            }
            if !need_fetch {
                return Ok(existing);
            }
            // Always silent-derive (same reliable path as Sync). On-device confirm
            // is the separate "Verify on Trezor now" button — tying Save to a
            // ButtonRequest was hanging/failing create with usb write Cancelled.
            let _ = verify;
            let fetched = fetch_trezor_address(chain, account, address_type.as_deref(), false)?;
            Ok(Some(fetched))
        })
        .await
        .map_err(|e| format!("trezor task join error: {e}"))??;
        address = fetched;
    }

    let mut xmr_view = request.xmr_view_key.clone();
    if kind == PortfolioKind::Trezor && chain == ChainId::Xmr && xmr_view.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
        let account = request.account_index.unwrap_or(0);
        if let Ok(Ok(w)) = tauri::async_runtime::spawn_blocking(move || {
            let path = format!("m/44'/128'/{account}'");
            trezor::trezor_get_monero_watch_key(&path)
        })
        .await
        {
            xmr_view = Some(w.watch_key_hex);
            if address.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                address = Some(w.address);
            }
        }
    }

    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;

    let record = PortfolioRecord {
        id: Uuid::new_v4().to_string(),
        name: request.name,
        kind,
        chain: chain.as_str().into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        account_index: request.account_index.unwrap_or(0),
        address_index: 0,
        address,
        xmr_view_key: xmr_view,
        notes: None,
        trezor_label: request.trezor_label,
        address_type: request.address_type.or_else(|| {
            if chain == ChainId::Btc {
                Some("native_segwit".into())
            } else {
                None
            }
        }),
        cached_balances_json: None,
    };

    unlocked.payload.portfolios.push(record.clone());
    let _ = fill_missing_addresses_dirty(unlocked);
    // Always persist immediately so portfolios survive crash/restart.
    state.vault.persist(unlocked).map_err(map_err)?;
    let saved = unlocked
        .payload
        .portfolios
        .iter()
        .find(|p| p.id == record.id)
        .cloned()
        .unwrap_or(record);
    Ok(saved)
}

#[tauri::command]
pub fn portfolio_list(state: State<'_, AppState>) -> Result<Vec<PortfolioRecord>, String> {
    require_unlocked(&state)?;
    let session = state.session.lock();
    let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
    Ok(unlocked.payload.portfolios.clone())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePortfolioRequest {
    pub id: String,
    pub name: String,
}

#[tauri::command]
pub fn portfolio_rename(
    state: State<'_, AppState>,
    request: RenamePortfolioRequest,
) -> Result<(), String> {
    require_unlocked(&state)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    let p = unlocked
        .payload
        .portfolios
        .iter_mut()
        .find(|p| p.id == request.id)
        .ok_or_else(|| map_err(OpalError::InvalidInput("not found".into())))?;
    p.name = request.name;
    state.vault.persist(unlocked).map_err(map_err)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderPortfoliosRequest {
    pub order: Vec<String>,
}

/// Persist a drag-and-drop reorder. The vault's `portfolios` array order *is*
/// the display order (`portfolio_list` returns it as-stored), so this saves
/// straight into the encrypted vault rather than browser local storage —
/// survives reinstalls/backups and isn't tied to the webview's storage.
#[tauri::command]
pub fn portfolio_reorder(
    state: State<'_, AppState>,
    request: ReorderPortfoliosRequest,
) -> Result<Vec<PortfolioRecord>, String> {
    require_unlocked(&state)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;

    let mut by_id: std::collections::HashMap<String, PortfolioRecord> = unlocked
        .payload
        .portfolios
        .drain(..)
        .map(|p| (p.id.clone(), p))
        .collect();
    let mut next = Vec::with_capacity(by_id.len());
    for id in &request.order {
        if let Some(p) = by_id.remove(id) {
            next.push(p);
        }
    }
    // Anything not mentioned (shouldn't happen from a well-formed client)
    // keeps its relative order and lands at the end rather than vanishing.
    let mut leftovers: Vec<PortfolioRecord> = by_id.into_values().collect();
    leftovers.sort_by_key(|p| p.id.clone());
    next.extend(leftovers);

    unlocked.payload.portfolios = next;
    state.vault.persist(unlocked).map_err(map_err)?;
    Ok(unlocked.payload.portfolios.clone())
}

#[tauri::command]
pub fn portfolio_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    require_unlocked(&state)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    unlocked.payload.portfolios.retain(|p| p.id != id);
    state.vault.persist(unlocked).map_err(map_err)
}

#[tauri::command]
pub async fn portfolio_balances(
    app: AppHandle,
    portfolio_id: Option<String>,
) -> Result<Vec<PortfolioBalance>, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        // Snapshot under a short lock, then do network I/O unlocked so create/list never hang.
        let (http, payload) = {
            let mut session = state.session.lock();
            let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
            if fill_missing_addresses_dirty(unlocked) {
                state.vault.persist(unlocked).map_err(map_err)?;
            }
            let http = http_from_session(unlocked)?;
            (http, unlocked.payload.clone())
        };
        let bals =
            fetch_balances_for_payload(&http, &payload, portfolio_id.as_deref()).map_err(map_err)?;

        // Debounce vault writes — persisting on every 3s poll was locking the
        // session and making the next scrape wait on disk I/O.
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            use std::time::{SystemTime, UNIX_EPOCH};
            static LAST_PERSIST_MS: AtomicU64 = AtomicU64::new(0);
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let last = LAST_PERSIST_MS.load(Ordering::Relaxed);
            let should_persist = now_ms.saturating_sub(last) > 15_000;

            if should_persist {
                let mut session = state.session.lock();
                if let Some(unlocked) = session.as_mut() {
                    let mut dirty = false;
                    for bal in &bals {
                        if let Some(p) = unlocked
                            .payload
                            .portfolios
                            .iter_mut()
                            .find(|p| p.id == bal.portfolio_id)
                        {
                            if let Ok(json) = serde_json::to_string(bal) {
                                if p.cached_balances_json.as_deref() != Some(json.as_str()) {
                                    p.cached_balances_json = Some(json);
                                    dirty = true;
                                }
                            }
                        }
                    }
                    if dirty {
                        let _ = state.vault.persist(unlocked);
                        LAST_PERSIST_MS.store(now_ms, Ordering::Relaxed);
                    }
                }
            }
        }
        Ok(bals)
    })
    .await
}

/// Instant cached balances (no network). Used to paint UI while a live scrape runs.
#[tauri::command]
pub fn portfolio_balances_cached(
    state: State<'_, AppState>,
    portfolio_id: Option<String>,
) -> Result<Vec<PortfolioBalance>, String> {
    require_unlocked(&state)?;
    let session = state.session.lock();
    let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
    Ok(cached_balances_from_payload(
        &unlocked.payload,
        portfolio_id.as_deref(),
    ))
}

#[tauri::command]
pub fn portfolio_receive_address(
    state: State<'_, AppState>,
    portfolio_id: String,
) -> Result<String, String> {
    require_unlocked(&state)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    fill_missing_addresses(unlocked).map_err(map_err)?;
    let p = unlocked
        .payload
        .portfolios
        .iter()
        .find(|p| p.id == portfolio_id)
        .ok_or_else(|| map_err(OpalError::InvalidInput("not found".into())))?;
    let addr = resolve_address(unlocked, p).map_err(map_err)?;
    state.vault.persist(unlocked).map_err(map_err)?;
    Ok(addr)
}

#[tauri::command]
pub async fn portfolio_history(
    app: AppHandle,
    portfolio_id: String,
) -> Result<Vec<TxRow>, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, chain, address, xmr_keys) = {
            let mut session = state.session.lock();
            let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
            fill_missing_addresses(unlocked).map_err(map_err)?;
            let portfolio = unlocked
                .payload
                .portfolios
                .iter()
                .find(|p| p.id == portfolio_id)
                .cloned()
                .ok_or_else(|| map_err(OpalError::InvalidInput("portfolio not found".into())))?;
            let address = resolve_address(unlocked, &portfolio).map_err(map_err)?;
            let chain = ChainId::parse(&portfolio.chain).map_err(map_err)?;
            let xmr_keys = crate::wallet::service::xmr_keys_for_portfolio(unlocked, &portfolio);
            let http = http_from_session(unlocked)?;
            (http, chain, address, xmr_keys)
        };
        fetch_history_for_address(&http, chain, &address, xmr_keys).map_err(map_err)
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendRequest {
    pub portfolio_id: String,
    pub to: String,
    pub amount: String,
    pub token: Option<String>,
    pub fee_preset: Option<String>,
    pub custom_fee_sat_vb: Option<u64>,
    pub send_max: Option<bool>,
}

#[tauri::command]
pub async fn portfolio_send(
    app: AppHandle,
    request: SendRequest,
) -> Result<SendResult, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        // Snapshot everything the send needs under a short-lived lock, then
        // drop it before any network I/O. Signing/broadcasting can involve
        // several sequential RPC round trips (fee/UTXO lookups, broadcast)
        // and used to hold the global session mutex the whole time, freezing
        // Settings, balance refreshes, and every other command in the
        // meantime.
        let (http, portfolio, mnemonic, passphrase) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = http_from_session(unlocked)?;
            let portfolio = unlocked
                .payload
                .portfolios
                .iter()
                .find(|p| p.id == request.portfolio_id)
                .cloned()
                .ok_or_else(|| map_err(OpalError::InvalidInput("portfolio not found".into())))?;
            let mnemonic = unlocked.payload.seed_mnemonic.clone();
            let passphrase = crate::wallet::service::passphrase_from_vault(unlocked);
            (http, portfolio, mnemonic, passphrase)
        };
        crate::address_util::validate_address_for_chain(
            ChainId::parse(&portfolio.chain).map_err(map_err)?,
            &request.to,
        )
        .map_err(map_err)?;
        let amount_n = request.amount.trim().parse::<f64>().unwrap_or(0.0);
        if !request.send_max.unwrap_or(false) && (!(amount_n.is_finite()) || amount_n <= 0.0) {
            return Err(map_err(OpalError::InvalidInput(
                "amount must be greater than zero".into(),
            )));
        }
        let fee_preset = match request.fee_preset.as_deref() {
            Some("economy") => crate::network::FeePreset::Economy,
            Some("priority") => crate::network::FeePreset::Priority,
            _ => crate::network::FeePreset::Normal,
        };
        let utxo_opts = crate::wallet::send::UtxoSendOptions {
            fee_preset,
            custom_fee_sat_vb: request.custom_fee_sat_vb,
            send_max: request.send_max.unwrap_or(false),
            replace_txid: None,
        };
        crate::wallet::service::send_from_portfolio_opts(
            &http,
            &portfolio,
            mnemonic.as_deref(),
            &passphrase,
            &request.to,
            &request.amount,
            request.token.as_deref(),
            &utxo_opts,
            fee_preset,
        )
        .map_err(map_err)
    })
    .await
}

#[tauri::command]
pub async fn prices_fiat(app: AppHandle) -> Result<serde_json::Value, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, fiat) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = http_from_session(unlocked)?;
            let fiat = unlocked.payload.settings.fiat.to_ascii_lowercase();
            (http, fiat)
        };
        let map = http.prices_in_fiat(&fiat).map_err(map_err)?;
        Ok(serde_json::to_value(map).unwrap_or_default())
    })
    .await
}

/// Prefetch spot prices for every supported fiat so currency switches are instant.
#[tauri::command]
pub async fn warm_spot_prices(app: AppHandle) -> Result<serde_json::Value, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let http = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            http_from_session(unlocked)?
        };
        http.fetch_all_fiat_spot_prices().map_err(map_err)?;
        let snapshot = http.spot_prices_snapshot();
        Ok(serde_json::to_value(snapshot).unwrap_or_default())
    })
    .await
}

/// Instant read of cached spot maps — no network, no worker queue wait.
#[tauri::command]
pub fn spot_prices_snapshot(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    require_unlocked(&state)?;
    let session = state.session.lock();
    let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
    let http = http_from_session(unlocked)?;
    let snapshot = http.spot_prices_snapshot();
    Ok(serde_json::to_value(snapshot).unwrap_or_default())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceHistoryRequest {
    pub coin_ids: Vec<String>,
    pub vs_currency: Option<String>,
    pub days: Option<u32>,
}

#[tauri::command]
pub async fn price_history(
    app: AppHandle,
    request: PriceHistoryRequest,
) -> Result<serde_json::Value, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, default_vs) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = http_from_session(unlocked)?;
            let vs = unlocked.payload.settings.fiat.to_ascii_lowercase();
            (http, vs)
        };
        let vs = request
            .vs_currency
            .unwrap_or(default_vs)
            .to_ascii_lowercase();
        let days = request.days.unwrap_or(7).clamp(1, 365);
        let ids: Vec<String> = request
            .coin_ids
            .into_iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .take(12)
            .collect();
        let map = http.market_charts(&ids, &vs, days);
        Ok(serde_json::to_value(map).unwrap_or_default())
    })
    .await
}

#[tauri::command]
pub async fn trezor_status() -> trezor::TrezorStatus {
    // Sync commands run on the webview's main thread in Tauri — probe_trezor()
    // does blocking network I/O, so it must be pushed onto a worker thread or
    // this (polled every few seconds by the UI) periodically freezes the app.
    tauri::async_runtime::spawn_blocking(trezor::probe_trezor)
        .await
        .unwrap_or_else(|_| trezor::TrezorStatus {
            available: false,
            bridge_url: String::new(),
            message: "Status check failed".into(),
            suite_required: true,
            device_count: 0,
            session_active: false,
            device_label: None,
            device_model: None,
            device_internal_model: None,
        })
}

fn fetch_trezor_address(
    chain: ChainId,
    account: u32,
    address_type: Option<&str>,
    show_display: bool,
) -> Result<String, String> {
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
            let path = format!("m/44'/60'/{account}'/0/0");
            trezor::trezor_get_ethereum_address(&path, show_display).map_err(map_err)
        }
        ChainId::Btc => {
            let (path, script) = match address_type {
                Some("taproot") => (format!("m/86'/0'/{account}'/0/0"), "SPENDTAPROOT"),
                Some("legacy") => (format!("m/44'/0'/{account}'/0/0"), "SPENDADDRESS"),
                Some("nested_segwit") => (format!("m/49'/0'/{account}'/0/0"), "SPENDP2SHWITNESS"),
                _ => (format!("m/84'/0'/{account}'/0/0"), "SPENDWITNESS"),
            };
            trezor::trezor_get_bitcoin_address("Bitcoin", &path, script, show_display).map_err(map_err)
        }
        ChainId::Ltc => {
            let path = format!("m/84'/2'/{account}'/0/0");
            trezor::trezor_get_bitcoin_address("Litecoin", &path, "SPENDWITNESS", show_display)
                .map_err(map_err)
        }
        ChainId::Doge => {
            let path = format!("m/44'/3'/{account}'/0/0");
            trezor::trezor_get_bitcoin_address("Dogecoin", &path, "SPENDADDRESS", show_display)
                .map_err(map_err)
        }
        ChainId::Sol => {
            let path = format!("m/44'/501'/{account}'/0'");
            trezor::trezor_get_solana_address(&path, show_display).map_err(map_err)
        }
        ChainId::Trx => {
            let path = format!("m/44'/195'/{account}'/0/0");
            trezor::trezor_get_tron_address(&path, show_display).map_err(map_err)
        }
        ChainId::Xmr => {
            let path = format!("m/44'/128'/{account}'");
            trezor::trezor_get_monero_address(&path, show_display).map_err(map_err)
        }
        _ => Err(map_err(OpalError::InvalidInput(
            "This chain is not supported on Trezor in Opal (Gram/TON has no Trezor messages)."
                .into(),
        ))),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrezorVerifyRequest {
    pub chain: String,
    pub account_index: Option<u32>,
    pub address_type: Option<String>,
}

#[tauri::command]
pub async fn trezor_verify_address(request: TrezorVerifyRequest) -> Result<String, String> {
    // Same reasoning as trezor_status: this blocks on Bridge + on-device button
    // confirmation, which can take a while — keep it off the main thread.
    tauri::async_runtime::spawn_blocking(move || {
        let chain = ChainId::parse(&request.chain).map_err(map_err)?;
        let account = request.account_index.unwrap_or(0);
        fetch_trezor_address(chain, account, request.address_type.as_deref(), true)
    })
    .await
    .map_err(|e| format!("trezor task join error: {e}"))?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrezorDiscoverRequest {
    pub quiet: Option<bool>,
}

#[tauri::command]
pub async fn trezor_discover_portfolios(
    app: AppHandle,
    request: Option<TrezorDiscoverRequest>,
) -> Result<Vec<PortfolioRecord>, String> {
    let quiet = request.and_then(|r| r.quiet).unwrap_or(false);
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, mut payload) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = HttpCtx::for_discovery(unlocked.payload.settings.tor_socks.as_deref())
                .map_err(map_err)?;
            (http, unlocked.payload.clone())
        };
        let created = crate::wallet::discover_trezor_portfolios(
            &http,
            &mut payload,
            quiet,
            |_| {},
        )
        .map_err(map_err)?;
        if created.is_empty() {
            return Ok(created);
        }
        let mut session = state.session.lock();
        let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
        for row in &created {
            let already = unlocked.payload.portfolios.iter().any(|p| {
                p.chain == row.chain && p.address == row.address
            });
            if !already {
                unlocked.payload.portfolios.push(row.clone());
            }
        }
        state.vault.persist(unlocked).map_err(map_err)?;
        Ok(created)
    })
    .await
}

#[tauri::command]
pub async fn trezor_sync_xmr_key_images(app: AppHandle) -> Result<u32, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, portfolios) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = HttpCtx::for_discovery(unlocked.payload.settings.tor_socks.as_deref())
                .map_err(map_err)?;
            let rows: Vec<_> = unlocked
                .payload
                .portfolios
                .iter()
                .filter(|p| p.kind == PortfolioKind::Trezor && p.chain == "xmr")
                .cloned()
                .collect();
            (http, rows)
        };
        let mut n = 0u32;
        for p in portfolios {
            let Some(view) = p.xmr_view_key.as_deref() else {
                continue;
            };
            let Some(addr) = p.address.as_deref() else {
                continue;
            };
            if let Ok(c) =
                crate::trezor::trezor_monero_sync_key_images(&http, view, addr, p.account_index)
            {
                n = n.saturating_add(c as u32);
            }
        }
        Ok(n)
    })
    .await
}

#[tauri::command]
pub fn detect_address_chains(address: String) -> Vec<String> {
    detect_chain_from_address(&address)
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.as_str().to_string())
        .collect()
}

#[tauri::command]
pub fn address_book_list(state: State<'_, AppState>) -> Result<Vec<crate::vault::AddressBookEntry>, String> {
    require_unlocked(&state)?;
    let session = state.session.lock();
    let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
    Ok(unlocked.payload.address_book.clone())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressBookAddRequest {
    pub label: String,
    pub chain: String,
    pub address: String,
}

#[tauri::command]
pub fn address_book_add(
    state: State<'_, AppState>,
    request: AddressBookAddRequest,
) -> Result<crate::vault::AddressBookEntry, String> {
    require_unlocked(&state)?;
    let chain = ChainId::parse(&request.chain).map_err(map_err)?;
    crate::address_util::validate_address_for_chain(chain, &request.address).map_err(map_err)?;
    let entry = crate::vault::AddressBookEntry {
        id: Uuid::new_v4().to_string(),
        label: request.label,
        chain: chain.as_str().into(),
        address: request.address.trim().into(),
    };
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    unlocked.payload.address_book.push(entry.clone());
    state.vault.persist(unlocked).map_err(map_err)?;
    Ok(entry)
}

#[tauri::command]
pub fn address_book_remove(state: State<'_, AppState>, id: String) -> Result<(), String> {
    require_unlocked(&state)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    unlocked.payload.address_book.retain(|e| e.id != id);
    state.vault.persist(unlocked).map_err(map_err)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeAddressRequest {
    pub to: String,
    pub chain: Option<String>,
}

#[tauri::command]
pub fn analyze_send_address(
    state: State<'_, AppState>,
    request: AnalyzeAddressRequest,
) -> Result<crate::address_util::AddressSafety, String> {
    require_unlocked(&state)?;
    let session = state.session.lock();
    let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
    let contacts: Vec<String> = unlocked
        .payload
        .address_book
        .iter()
        .map(|e| e.address.clone())
        .collect();
    let recent: Vec<String> = unlocked
        .payload
        .portfolios
        .iter()
        .filter_map(|p| p.address.clone())
        .collect();
    Ok(crate::address_util::analyze_address_safety(
        &request.to,
        &recent,
        &contacts,
        request
            .chain
            .as_deref()
            .and_then(|c| ChainId::parse(c).ok()),
    ))
}

#[tauri::command]
pub fn portfolio_receive_uri(
    state: State<'_, AppState>,
    portfolio_id: String,
    amount: Option<String>,
) -> Result<String, String> {
    require_unlocked(&state)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    fill_missing_addresses(unlocked).map_err(map_err)?;
    let p = unlocked
        .payload
        .portfolios
        .iter()
        .find(|p| p.id == portfolio_id)
        .ok_or_else(|| map_err(OpalError::InvalidInput("not found".into())))?;
    let addr = resolve_address(unlocked, p).map_err(map_err)?;
    let chain = ChainId::parse(&p.chain).map_err(map_err)?;
    Ok(crate::address_util::payment_uri(
        chain,
        &addr,
        amount.as_deref(),
    ))
}

#[tauri::command]
pub async fn portfolio_estimate_fee(
    app: AppHandle,
    portfolio_id: String,
    fee_preset: Option<String>,
) -> Result<serde_json::Value, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, chain, addr, preset) = {
            let mut session = state.session.lock();
            let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
            let p = unlocked
                .payload
                .portfolios
                .iter()
                .find(|p| p.id == portfolio_id)
                .cloned()
                .ok_or_else(|| map_err(OpalError::InvalidInput("not found".into())))?;
            let chain = ChainId::parse(&p.chain).map_err(map_err)?;
            let addr = resolve_address(unlocked, &p).map_err(map_err)?;
            let http = http_from_session(unlocked)?;
            let preset = match fee_preset.as_deref() {
                Some("economy") => crate::network::FeePreset::Economy,
                Some("priority") => crate::network::FeePreset::Priority,
                _ => crate::network::FeePreset::Normal,
            };
            (http, chain, addr, preset)
        };
        if chain.is_utxo() {
            let opts = crate::wallet::send::UtxoSendOptions {
                fee_preset: preset,
                ..Default::default()
            };
            let fee = crate::wallet::send::estimate_send_fee(&http, chain, &addr, &opts)
                .map_err(map_err)?;
            let estimates = http.fee_estimates(chain).unwrap_or_default();
            Ok(serde_json::json!({
                "feeSats": fee,
                "economySatVb": estimates.economy,
                "normalSatVb": estimates.normal,
                "prioritySatVb": estimates.priority,
            }))
        } else {
            Ok(serde_json::json!({ "feeSats": null }))
        }
    })
    .await
}

#[tauri::command]
pub fn portfolio_next_address(
    state: State<'_, AppState>,
    portfolio_id: String,
) -> Result<String, String> {
    require_unlocked(&state)?;
    let mut session = state.session.lock();
    let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
    let p = unlocked
        .payload
        .portfolios
        .iter_mut()
        .find(|p| p.id == portfolio_id)
        .ok_or_else(|| map_err(OpalError::InvalidInput("not found".into())))?;
    if p.kind != crate::vault::PortfolioKind::Software {
        return Err(map_err(OpalError::InvalidInput(
            "only software UTXO portfolios rotate receive addresses".into(),
        )));
    }
    let chain = ChainId::parse(&p.chain).map_err(map_err)?;
    if !chain.is_utxo() {
        return Err(map_err(OpalError::InvalidInput(
            "address rotation is for UTXO chains".into(),
        )));
    }
    p.address_index = p.address_index.saturating_add(1);
    p.address = None;
    fill_missing_addresses(unlocked).map_err(map_err)?;
    let addr = unlocked
        .payload
        .portfolios
        .iter()
        .find(|x| x.id == portfolio_id)
        .and_then(|x| x.address.clone())
        .ok_or_else(|| map_err(OpalError::InvalidInput("derive failed".into())))?;
    state.vault.persist(unlocked).map_err(map_err)?;
    Ok(addr)
}

#[tauri::command]
pub async fn portfolio_rescan(
    app: AppHandle,
    portfolio_id: String,
) -> Result<Vec<PortfolioBalance>, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;

        // Snapshot what's needed, then release the session lock before doing
        // any network I/O. Gap discovery alone can make up to 20 sequential
        // RPC calls — holding the global lock for that whole stretch used to
        // freeze every other command (Settings, balances, …) in the meantime.
        let (http, mnemonic, passphrase, snapshot) = {
            let mut session = state.session.lock();
            let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = http_from_session(unlocked)?;
            let mnemonic = unlocked.payload.seed_mnemonic.clone();
            let passphrase = crate::wallet::service::passphrase_from_vault(unlocked);
            let snapshot = unlocked
                .payload
                .portfolios
                .iter()
                .find(|p| p.id == portfolio_id)
                .cloned();
            (http, mnemonic, passphrase, snapshot)
        };

        let mut new_index = None;
        if let Some(p) = &snapshot {
            if p.kind == crate::vault::PortfolioKind::Software {
                let chain = ChainId::parse(&p.chain).map_err(map_err)?;
                if chain.is_utxo() {
                    if let Some(m) = &mnemonic {
                        let at = match p.address_type.as_deref() {
                            Some("taproot") => crate::wallet::AddressType::Taproot,
                            Some("legacy") => crate::wallet::AddressType::Legacy,
                            Some("nested_segwit") => crate::wallet::AddressType::NestedSegwit,
                            _ => crate::wallet::AddressType::NativeSegwit,
                        };
                        let idx = crate::wallet::discover_gap(
                            m,
                            &passphrase,
                            chain,
                            p.account_index,
                            at,
                            &|addr| {
                                http.btc_address_info(chain, addr)
                                    .map(|(bal, u)| bal > 0 || !u.is_empty())
                                    .unwrap_or(false)
                            },
                            20,
                        )
                        .unwrap_or(0);
                        new_index = Some(idx);
                    }
                }
            }
        }

        // Re-acquire the lock only to apply the results and persist.
        let mut session = state.session.lock();
        let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
        if let Some(idx) = new_index {
            if let Some(target) = unlocked
                .payload
                .portfolios
                .iter_mut()
                .find(|x| x.id == portfolio_id)
            {
                target.address_index = idx;
                target.address = None;
                target.cached_balances_json = None;
            }
        }
        fill_missing_addresses(unlocked).map_err(map_err)?;
        let payload = unlocked.payload.clone();
        drop(session);

        let bals =
            fetch_balances_for_payload(&http, &payload, Some(&portfolio_id)).map_err(map_err)?;

        let mut session = state.session.lock();
        let unlocked = session.as_mut().ok_or_else(|| map_err(OpalError::Locked))?;
        // Cache stores a single PortfolioBalance object (not the Vec wrapper).
        if let Some(row) = bals.iter().find(|b| b.portfolio_id == portfolio_id).or_else(|| bals.first())
        {
            if let Ok(json) = serde_json::to_string(row) {
                if let Some(p) = unlocked
                    .payload
                    .portfolios
                    .iter_mut()
                    .find(|p| p.id == portfolio_id)
                {
                    p.cached_balances_json = Some(json);
                }
            }
        }
        state.vault.persist(unlocked).map_err(map_err)?;
        Ok(bals)
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BumpFeeRequest {
    pub portfolio_id: String,
    pub txid: String,
    pub fee_preset: Option<String>,
}

#[tauri::command]
pub async fn portfolio_bump_fee(
    app: AppHandle,
    request: BumpFeeRequest,
) -> Result<SendResult, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        // Snapshot then unlock before broadcasting — RBF bumps hit the
        // network just like a normal send and must not hold the global
        // session lock while doing so.
        let (http, p, mnemonic, pass) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = http_from_session(unlocked)?;
            let p = unlocked
                .payload
                .portfolios
                .iter()
                .find(|p| p.id == request.portfolio_id)
                .cloned()
                .ok_or_else(|| map_err(OpalError::InvalidInput("not found".into())))?;
            let mnemonic = unlocked
                .payload
                .seed_mnemonic
                .clone()
                .ok_or_else(|| map_err(OpalError::InvalidInput("no seed".into())))?;
            let pass = crate::wallet::service::passphrase_from_vault(unlocked);
            (http, p, mnemonic, pass)
        };
        let chain = ChainId::parse(&p.chain).map_err(map_err)?;
        if !matches!(chain, ChainId::Btc | ChainId::Ltc) {
            return Err(map_err(OpalError::InvalidInput(
                "RBF bump supported for BTC/LTC software wallets".into(),
            )));
        }
        if p.kind != crate::vault::PortfolioKind::Software {
            return Err(map_err(OpalError::InvalidInput(
                "software portfolio required".into(),
            )));
        }
        let preset = match request.fee_preset.as_deref() {
            Some("economy") => crate::network::FeePreset::Economy,
            Some("priority") => crate::network::FeePreset::Priority,
            _ => crate::network::FeePreset::Priority,
        };
        let opts = crate::wallet::send::UtxoSendOptions {
            fee_preset: preset,
            send_max: true,
            replace_txid: Some(request.txid),
            ..Default::default()
        };
        let at = match p.address_type.as_deref() {
            Some("taproot") => crate::wallet::AddressType::Taproot,
            Some("legacy") => crate::wallet::AddressType::Legacy,
            Some("nested_segwit") => crate::wallet::AddressType::NestedSegwit,
            _ => crate::wallet::AddressType::NativeSegwit,
        };
        let to = if let Some(addr) = p.address.clone().filter(|a| !a.is_empty()) {
            addr
        } else {
            let derived = derive_for_chain(
                &mnemonic,
                &pass,
                chain,
                p.account_index,
                p.address_index,
                false,
            )
            .map_err(map_err)?;
            derived.address.clone()
        };
        let txid = crate::wallet::send::send_btc_like(
            &http,
            chain,
            &mnemonic,
            &pass,
            p.account_index,
            p.address_index,
            &to,
            "0",
            at,
            &opts,
        )
        .map_err(map_err)?;
        Ok(SendResult {
            explorer_url: crate::network::explorer_tx_url(chain, &txid),
            txid,
        })
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapQuoteRequest {
    pub provider: String,
    pub from_asset: String,
    pub to_asset: String,
    pub amount: String,
    pub from_chain: Option<String>,
    pub to_chain: Option<String>,
}

fn load_fixedfloat_local_file() -> (Option<String>, Option<String>) {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LocalFf {
        api_key: String,
        api_secret: String,
    }

    let candidates = [
        std::env::current_dir()
            .ok()
            .map(|p| p.join("fixedfloat.local.json")),
        std::env::current_dir()
            .ok()
            .map(|p| p.join("..").join("fixedfloat.local.json")),
        // Dev: src-tauri cwd → repo root
        option_env!("CARGO_MANIFEST_DIR").map(|d| {
            std::path::Path::new(d)
                .join("..")
                .join("fixedfloat.local.json")
        }),
    ];

    for path in candidates.into_iter().flatten() {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<LocalFf>(&raw) else {
            continue;
        };
        let key = parsed.api_key.trim().to_string();
        let secret = parsed.api_secret.trim().to_string();
        if !key.is_empty() && !secret.is_empty() {
            return (Some(key), Some(secret));
        }
    }
    (None, None)
}

fn fixedfloat_creds_from_settings(
    settings: &crate::vault::AppSettings,
) -> (Option<String>, Option<String>) {
    let (file_key, file_secret) = load_fixedfloat_local_file();
    let key = settings
        .fixedfloat_api_key
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("OPAL_FIXEDFLOAT_API_KEY").ok().filter(|s| !s.is_empty()))
        .or(file_key);
    let secret = settings
        .fixedfloat_api_secret
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("OPAL_FIXEDFLOAT_API_SECRET")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or(file_secret);
    (key, secret)
}

#[tauri::command]
pub async fn swap_fixedfloat_ready(app: AppHandle) -> Result<bool, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let session = state.session.lock();
        let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
        let (k, s) = fixedfloat_creds_from_settings(&unlocked.payload.settings);
        Ok(k.is_some() && s.is_some())
    })
    .await
}

#[tauri::command]
pub async fn swap_quote(
    app: AppHandle,
    request: SwapQuoteRequest,
) -> Result<crate::wallet::swap::SwapQuote, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, ff_key, ff_secret) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = http_from_session(unlocked)?;
            let (k, s) = fixedfloat_creds_from_settings(&unlocked.payload.settings);
            (http, k, s)
        };
        match request.provider.as_str() {
            "jupiter" => {
                let input = crate::wallet::swap::default_mint_for_symbol(&request.from_asset)
                    .ok_or_else(|| map_err(OpalError::InvalidInput("unknown from asset".into())))?;
                let output = crate::wallet::swap::default_mint_for_symbol(&request.to_asset)
                    .ok_or_else(|| map_err(OpalError::InvalidInput("unknown to asset".into())))?;
                let decimals = if request.from_asset.eq_ignore_ascii_case("SOL") {
                    9
                } else {
                    6
                };
                let atomic = parse_decimal_to_atomic(&request.amount, decimals).map_err(map_err)?;
                crate::wallet::swap::jupiter_quote(
                    &http,
                    input,
                    output,
                    atomic,
                    50,
                    &request.from_asset,
                    &request.to_asset,
                )
                .map_err(map_err)
            }
            "fixedfloat" => {
                let from_chain = request.from_chain.as_deref().unwrap_or("");
                let to_chain = request.to_chain.as_deref().unwrap_or("");
                let from_ccy = crate::wallet::swap::fixedfloat_ccy(&request.from_asset, from_chain)
                    .ok_or_else(|| {
                        map_err(OpalError::InvalidInput(format!(
                            "FixedFloat does not support {} on {}",
                            request.from_asset, from_chain
                        )))
                    })?;
                let to_ccy = crate::wallet::swap::fixedfloat_ccy(&request.to_asset, to_chain)
                    .ok_or_else(|| {
                        map_err(OpalError::InvalidInput(format!(
                            "FixedFloat does not support {} on {}",
                            request.to_asset, to_chain
                        )))
                    })?;
                let req = crate::wallet::swap::FixedFloatQuoteRequest {
                    from_ccy,
                    to_ccy,
                    amount: request.amount,
                    direction: "from".into(),
                };
                crate::wallet::swap::fixedfloat_rate(
                    &http,
                    &req,
                    ff_key.as_deref(),
                    ff_secret.as_deref(),
                )
                .map_err(map_err)
            }
            _ => Err(map_err(OpalError::InvalidInput(
                "provider must be jupiter|fixedfloat".into(),
            ))),
        }
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedFloatCreateRequest {
    pub from_asset: String,
    pub to_asset: String,
    pub from_chain: String,
    pub to_chain: String,
    pub amount: String,
    pub to_address: String,
}

#[tauri::command]
pub async fn swap_fixedfloat_create(
    app: AppHandle,
    request: FixedFloatCreateRequest,
) -> Result<crate::wallet::swap::FixedFloatOrder, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, ff_key, ff_secret) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = http_from_session(unlocked)?;
            let (k, s) = fixedfloat_creds_from_settings(&unlocked.payload.settings);
            (http, k, s)
        };
        let api_key = ff_key.ok_or_else(|| {
            map_err(OpalError::InvalidInput(
                "Add your FixedFloat API key in Settings to create swaps in Opal.".into(),
            ))
        })?;
        let api_secret = ff_secret.ok_or_else(|| {
            map_err(OpalError::InvalidInput(
                "Add your FixedFloat API secret in Settings to create swaps in Opal.".into(),
            ))
        })?;
        let from_ccy = crate::wallet::swap::fixedfloat_ccy(&request.from_asset, &request.from_chain)
            .ok_or_else(|| map_err(OpalError::InvalidInput("unsupported from asset".into())))?;
        let to_ccy = crate::wallet::swap::fixedfloat_ccy(&request.to_asset, &request.to_chain)
            .ok_or_else(|| map_err(OpalError::InvalidInput("unsupported to asset".into())))?;
        crate::wallet::swap::fixedfloat_create_order(
            &http,
            &from_ccy,
            &to_ccy,
            &request.amount,
            &request.to_address,
            &api_key,
            &api_secret,
        )
        .map_err(map_err)
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedFloatOrderRequest {
    pub id: String,
    pub token: String,
}

#[tauri::command]
pub async fn swap_fixedfloat_order(
    app: AppHandle,
    request: FixedFloatOrderRequest,
) -> Result<crate::wallet::swap::FixedFloatOrder, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let (http, ff_key, ff_secret) = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            let http = http_from_session(unlocked)?;
            let (k, s) = fixedfloat_creds_from_settings(&unlocked.payload.settings);
            (http, k, s)
        };
        let api_key = ff_key.ok_or_else(|| {
            map_err(OpalError::InvalidInput(
                "FixedFloat API key required.".into(),
            ))
        })?;
        let api_secret = ff_secret.ok_or_else(|| {
            map_err(OpalError::InvalidInput(
                "FixedFloat API secret required.".into(),
            ))
        })?;
        crate::wallet::swap::fixedfloat_order_status(
            &http,
            &request.id,
            &request.token,
            &api_key,
            &api_secret,
        )
        .map_err(map_err)
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedFloatExecuteRequest {
    pub from_portfolio_id: String,
    pub to_portfolio_id: String,
    pub from_asset: String,
    pub to_asset: String,
    pub amount: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedFloatExecuteResult {
    pub order: crate::wallet::swap::FixedFloatOrder,
    pub txid: String,
    pub explorer_url: String,
}

fn is_native_asset(symbol: &str, chain: &str) -> bool {
    let sym = symbol.trim().to_ascii_uppercase();
    let chain = chain.trim().to_ascii_lowercase();
    match chain.as_str() {
        "sol" => sym == "SOL",
        "btc" => sym == "BTC",
        "ltc" => sym == "LTC",
        "doge" => sym == "DOGE",
        "xmr" => sym == "XMR",
        "trx" => sym == "TRX",
        "ton" => sym == "TON",
        "eth" | "arb" | "base" | "op" | "polygon" | "avax" | "bsc" | "gnosis" | "linea" => {
            sym == "ETH"
        }
        _ => sym == chain.to_ascii_uppercase(),
    }
}

/// Create a FixedFloat order and immediately send the deposit from the source
/// portfolio — one-click automated swap.
#[tauri::command]
pub async fn swap_fixedfloat_execute(
    app: AppHandle,
    request: FixedFloatExecuteRequest,
) -> Result<FixedFloatExecuteResult, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;

        let (http, ff_key, ff_secret, from_portfolio, to_portfolio, mnemonic, passphrase) = {
            let mut session = state.session.lock();
            let unlocked = session
                .as_mut()
                .ok_or_else(|| map_err(OpalError::Locked))?;
            let http = http_from_session(unlocked)?;
            let (k, s) = fixedfloat_creds_from_settings(&unlocked.payload.settings);
            let from_portfolio = unlocked
                .payload
                .portfolios
                .iter()
                .find(|p| p.id == request.from_portfolio_id)
                .cloned()
                .ok_or_else(|| map_err(OpalError::InvalidInput("from portfolio not found".into())))?;
            let to_portfolio = unlocked
                .payload
                .portfolios
                .iter()
                .find(|p| p.id == request.to_portfolio_id)
                .cloned()
                .ok_or_else(|| map_err(OpalError::InvalidInput("to portfolio not found".into())))?;
            if from_portfolio.kind != PortfolioKind::Software {
                return Err(map_err(OpalError::InvalidInput(
                    "Automated swaps need a software portfolio as the source.".into(),
                )));
            }
            let mnemonic = unlocked.payload.seed_mnemonic.clone();
            let passphrase = crate::wallet::service::passphrase_from_vault(unlocked);
            // Ensure destination has a receive address before creating the order.
            let _ = fill_missing_addresses_dirty(unlocked);
            let to_portfolio = unlocked
                .payload
                .portfolios
                .iter()
                .find(|p| p.id == request.to_portfolio_id)
                .cloned()
                .ok_or_else(|| map_err(OpalError::InvalidInput("to portfolio not found".into())))?;
            (http, k, s, from_portfolio, to_portfolio, mnemonic, passphrase)
        };

        let api_key = ff_key.ok_or_else(|| {
            map_err(OpalError::InvalidInput(
                "Add your FixedFloat API key to enable one-click swaps.".into(),
            ))
        })?;
        let api_secret = ff_secret.ok_or_else(|| {
            map_err(OpalError::InvalidInput(
                "Add your FixedFloat API secret to enable one-click swaps.".into(),
            ))
        })?;

        let to_address = to_portfolio.address.clone().filter(|s| !s.is_empty()).ok_or_else(|| {
            map_err(OpalError::InvalidInput(
                "Destination portfolio has no receive address yet.".into(),
            ))
        })?;

        let from_ccy =
            crate::wallet::swap::fixedfloat_ccy(&request.from_asset, &from_portfolio.chain)
                .ok_or_else(|| {
                    map_err(OpalError::InvalidInput(format!(
                        "FixedFloat does not support {} on {}",
                        request.from_asset, from_portfolio.chain
                    )))
                })?;
        let to_ccy = crate::wallet::swap::fixedfloat_ccy(&request.to_asset, &to_portfolio.chain)
            .ok_or_else(|| {
                map_err(OpalError::InvalidInput(format!(
                    "FixedFloat does not support {} on {}",
                    request.to_asset, to_portfolio.chain
                )))
            })?;

        let order = crate::wallet::swap::fixedfloat_create_order(
            &http,
            &from_ccy,
            &to_ccy,
            &request.amount,
            &to_address,
            &api_key,
            &api_secret,
        )
        .map_err(map_err)?;

        if order
            .deposit_tag
            .as_ref()
            .map(|t| !t.is_empty())
            .unwrap_or(false)
        {
            return Err(map_err(OpalError::InvalidInput(format!(
                "This FixedFloat deposit needs a memo/tag ({}), which Opal cannot attach automatically yet. Open {} to finish.",
                order.deposit_tag.as_deref().unwrap_or(""),
                order.order_url
            ))));
        }

        let token = if is_native_asset(&request.from_asset, &from_portfolio.chain) {
            None
        } else {
            Some(request.from_asset.clone())
        };

        let fee_preset = crate::network::FeePreset::Normal;
        let utxo_opts = crate::wallet::send::UtxoSendOptions {
            fee_preset,
            custom_fee_sat_vb: None,
            send_max: false,
            replace_txid: None,
        };

        let send = crate::wallet::service::send_from_portfolio_opts(
            &http,
            &from_portfolio,
            mnemonic.as_deref(),
            &passphrase,
            &order.deposit_address,
            &order.from_amount,
            token.as_deref(),
            &utxo_opts,
            fee_preset,
        )
        .map_err(|e| {
            map_err(OpalError::Io(format!(
                "Order {} created, but the deposit failed: {e}. Send {} to {} (see {}).",
                order.id,
                order.from_amount,
                order.deposit_address,
                order.order_url
            )))
        })?;

        let order = crate::wallet::swap::fixedfloat_order_status(
            &http,
            &order.id,
            &order.token,
            &api_key,
            &api_secret,
        )
        .unwrap_or(order);

        Ok(FixedFloatExecuteResult {
            order,
            txid: send.txid,
            explorer_url: send.explorer_url,
        })
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JupiterSwapTxRequest {
    pub quote_raw: serde_json::Value,
    pub user_public_key: String,
}

#[tauri::command]
pub async fn swap_jupiter_tx(
    app: AppHandle,
    request: JupiterSwapTxRequest,
) -> Result<String, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_unlocked(&state)?;
        let http = {
            let session = state.session.lock();
            let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
            http_from_session(unlocked)?
        };
        crate::wallet::swap::jupiter_swap_transaction(
            &http,
            &request.quote_raw,
            &request.user_public_key,
        )
        .map_err(map_err)
    })
    .await
}

fn parse_decimal_to_atomic(amount: &str, decimals: u32) -> Result<u64, OpalError> {
    let amount = amount.trim();
    let (whole, frac) = match amount.split_once('.') {
        Some((w, f)) => (w, f),
        None => (amount, ""),
    };
    let whole: u64 = if whole.is_empty() { 0 } else {
        whole.parse().map_err(|_| OpalError::InvalidInput("bad amount".into()))?
    };
    let mut frac = frac.to_string();
    if frac.len() > decimals as usize { frac.truncate(decimals as usize); }
    while frac.len() < decimals as usize { frac.push('0'); }
    let frac_n: u64 = if frac.is_empty() { 0 } else {
        frac.parse().map_err(|_| OpalError::InvalidInput("bad amount".into()))?
    };
    let base = 10u64.checked_pow(decimals).ok_or_else(|| OpalError::InvalidInput("decimals".into()))?;
    whole.checked_mul(base).and_then(|v| v.checked_add(frac_n))
        .ok_or_else(|| OpalError::InvalidInput("overflow".into()))
}
