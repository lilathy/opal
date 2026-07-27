use std::collections::HashMap;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use reqwest::blocking::{Client, ClientBuilder};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::OpalError;
use crate::wallet::ChainId;

#[derive(Clone)]
pub struct HttpCtx {
    client: Client,
    custom_rpc: HashMap<String, String>,
}

struct SpotCacheEntry {
    at: Instant,
    map: HashMap<String, f64>,
}

static SPOT_CACHE: Lazy<Mutex<HashMap<String, SpotCacheEntry>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static CHART_CACHE: Lazy<Mutex<HashMap<String, (Instant, Vec<(u64, f64)>)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
/// USD → fiat multipliers (units of fiat per 1 USD).
static FX_CACHE: Lazy<Mutex<Option<(Instant, HashMap<String, f64>)>>> =
    Lazy::new(|| Mutex::new(None));

const SUPPORTED_FIATS: &[&str] = &[
    "usd", "eur", "gbp", "rub", "jpy", "cny", "krw", "brl", "try", "inr",
];

/// Background loop — keep the in-memory book hot like Exodus/Trezor.
/// UI never waits on a free-tier scraper; it only reads `SPOT_CACHE`.
pub fn start_price_book_loop() {
    std::thread::Builder::new()
        .name("opal-price-book".into())
        .spawn(|| {
            let http = match HttpCtx::new(None, HashMap::new()) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("price book: http init failed: {e}");
                    return;
                }
            };
            loop {
                if let Err(e) = http.refresh_price_book() {
                    eprintln!("price book refresh: {e}");
                }
                std::thread::sleep(Duration::from_secs(5));
            }
        })
        .ok();
}

fn json_or_io_error<T: for<'de> Deserialize<'de>>(
    status: reqwest::StatusCode,
    body: &str,
) -> Result<T, OpalError> {
    if !status.is_success() {
        let snippet: String = body.chars().take(160).collect();
        return Err(OpalError::Io(format!(
            "http {status}: {snippet}"
        )));
    }
    serde_json::from_str(body).map_err(|e| OpalError::Io(format!("json: {e}")))
}

fn normalize_fiat(fiat: &str) -> &'static str {
    match fiat.to_ascii_lowercase().as_str() {
        "eur" => "eur",
        "gbp" => "gbp",
        "rub" => "rub",
        "jpy" => "jpy",
        "cny" => "cny",
        "krw" => "krw",
        "brl" => "brl",
        "try" => "try",
        "inr" => "inr",
        _ => "usd",
    }
}

fn binance_usdt_symbol(coin_id: &str) -> Option<&'static str> {
    match coin_id {
        "bitcoin" => Some("BTCUSDT"),
        "ethereum" => Some("ETHUSDT"),
        "solana" => Some("SOLUSDT"),
        "litecoin" => Some("LTCUSDT"),
        "dogecoin" => Some("DOGEUSDT"),
        // Monero is delisted on Binance.com — XMRUSDT still returns a frozen
        // ~$118 ticker and klines ending 2024-02. Use Kraken instead.
        "monero" => None,
        "binancecoin" => Some("BNBUSDT"),
        "avalanche-2" => Some("AVAXUSDT"),
        "matic-network" => Some("POLUSDT"),
        "tron" => Some("TRXUSDT"),
        "the-open-network" => Some("TONUSDT"),
        _ => None,
    }
}

fn kline_params(days: u32) -> (&'static str, u32) {
    match days {
        1 => ("15m", 96),
        2..=7 => ("1h", 168),
        8..=30 => ("4h", 180),
        31..=90 => ("12h", 180),
        _ => ("1d", 365),
    }
}

fn synthetic_stable_series(days: u32, fx: f64) -> Vec<(u64, f64)> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let span = (days as u64).saturating_mul(86_400).max(3_600);
    let steps = 48u64;
    let step = (span / steps).max(1);
    let px = if fx > 0.0 { fx } else { 1.0 };
    (0..=steps)
        .map(|i| (now.saturating_sub(span) + i * step, px))
        .collect()
}

impl HttpCtx {
    pub fn new(tor_socks: Option<&str>, custom_rpc: HashMap<String, String>) -> Result<Self, OpalError> {
        Self::with_timeouts(tor_socks, custom_rpc, Duration::from_millis(2_500), Duration::from_millis(1_200))
    }

    /// Longer timeouts for post-restore discovery (many parallel public RPCs).
    pub fn for_discovery(tor_socks: Option<&str>) -> Result<Self, OpalError> {
        Self::with_timeouts(
            tor_socks,
            HashMap::new(),
            Duration::from_secs(12),
            Duration::from_secs(4),
        )
    }

    fn with_timeouts(
        tor_socks: Option<&str>,
        custom_rpc: HashMap<String, String>,
        timeout: Duration,
        connect_timeout: Duration,
    ) -> Result<Self, OpalError> {
        let mut builder = ClientBuilder::new()
            .timeout(timeout)
            .connect_timeout(connect_timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(8)
            .user_agent("OpalWallet/0.1");
        if let Some(socks) = tor_socks {
            let proxy_url = if socks.starts_with("socks") {
                socks.to_string()
            } else {
                format!("socks5h://{socks}")
            };
            let proxy = reqwest::Proxy::all(&proxy_url)
                .map_err(|e| OpalError::Io(format!("tor proxy: {e}")))?;
            builder = builder.proxy(proxy);
        }
        let client = builder
            .build()
            .map_err(|e| OpalError::Io(format!("http client: {e}")))?;
        Ok(Self { client, custom_rpc })
    }

    fn rpc_url(&self, chain: ChainId) -> String {
        // Public popular nodes only — no user custom RPC.
        match chain {
            ChainId::Eth => "https://ethereum.publicnode.com".into(),
            ChainId::Arb => "https://arbitrum-one.publicnode.com".into(),
            ChainId::Base => "https://base.publicnode.com".into(),
            ChainId::Op => "https://optimism.publicnode.com".into(),
            ChainId::Polygon => "https://polygon-bor.publicnode.com".into(),
            ChainId::Avax => "https://avalanche-c-chain.publicnode.com".into(),
            ChainId::Bsc => "https://bsc.publicnode.com".into(),
            ChainId::Gnosis => "https://gnosis.publicnode.com".into(),
            ChainId::Trx => "https://api.trongrid.io".into(),
            ChainId::Linea => "https://linea.publicnode.com".into(),
            ChainId::Sol => "https://solana-rpc.publicnode.com".into(),
            ChainId::Ton => "https://toncenter.com/api/v2".into(),
            ChainId::Btc => "https://mempool.space/api".into(),
            ChainId::Ltc => "https://litecoinspace.org/api".into(),
            ChainId::Doge => "https://api.blockcypher.com/v1/doge/main".into(),
            ChainId::Xmr => "http://node.community.rino.io:18081".into(),
        }
    }

    /// Public RPC endpoint for a chain.
    pub fn chain_rpc(&self, chain: ChainId) -> String {
        self.rpc_url(chain)
    }

    /// Retained for compatibility; always `None` (custom RPC removed).
    pub fn custom_rpc_get(&self, _key: &str) -> Option<&str> {
        None
    }

    pub fn eth_rpc(&self, chain: ChainId, method: &str, params: Value) -> Result<Value, OpalError> {
        let url = self.rpc_url(chain);
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        // Balance path: one attempt. Retries turned every hung public RPC into
        // a 5–10s stall and stacked across Sol/TRX token legs.
        let v = self.post_json_once(&url, &body)?;
        if let Some(err) = v.get("error") {
            if !err.is_null() {
                return Err(OpalError::Io(format!("rpc error: {err}")));
            }
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn get_json(&self, url: &str) -> Result<Value, OpalError> {
        let res = self
            .client
            .get(url)
            .send()
            .map_err(|e| OpalError::Io(format!("GET {url}: {e}")))?;
        if !res.status().is_success() {
            return Err(OpalError::Io(format!(
                "GET {url} status {}",
                res.status()
            )));
        }
        res.json()
            .map_err(|e| OpalError::Io(format!("json: {e}")))
    }

    /// POST + parse JSON, with a couple of short-backoff retries.
    ///
    /// Public RPC endpoints (Solana's mainnet-beta in particular) throttle
    /// hard and inconsistently under load — without this, a portfolio's
    /// token balances would silently come back as "0" on a 429 with no
    /// error surfaced anywhere, which is exactly why token balances used to
    /// look flaky/inconsistent from one wallet (or one refresh) to the next.
    pub fn post_json(&self, url: &str, body: &Value) -> Result<Value, OpalError> {
        // Two attempts max — a third 5s hang made balance polls feel like
        // multi-minute freezes when a public RPC stalled.
        self.post_json_retrying(url, body, 2)
    }

    /// Single-attempt POST — for best-effort per-item lookups (e.g. one row
    /// out of a history list) where a slow/failed call should just skip that
    /// row rather than pile up 12s-timeout retries that make a whole list
    /// feel like the app has hung.
    pub fn post_json_once(&self, url: &str, body: &Value) -> Result<Value, OpalError> {
        self.post_json_retrying(url, body, 1)
    }

    /// POST + parse JSON, with a couple of short-backoff retries.
    ///
    /// Public RPC endpoints (Solana's mainnet-beta in particular) throttle
    /// hard and inconsistently under load — without this, a portfolio's
    /// token balances would silently come back as "0" on a 429 with no
    /// error surfaced anywhere, which is exactly why token balances used to
    /// look flaky/inconsistent from one wallet (or one refresh) to the next.
    fn post_json_retrying(&self, url: &str, body: &Value, attempts: u32) -> Result<Value, OpalError> {
        let mut last_err = OpalError::Io("post_json: no attempts made".into());
        for attempt in 0..attempts {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(250 * u64::from(attempt)));
            }
            let res = match self.client.post(url).json(body).send() {
                Ok(r) => r,
                Err(e) => {
                    last_err = OpalError::Io(format!("POST {url}: {e}"));
                    continue;
                }
            };
            let status = res.status();
            let text = match res.text() {
                Ok(t) => t,
                Err(e) => {
                    last_err = OpalError::Io(format!("POST {url} body: {e}"));
                    continue;
                }
            };
            if !status.is_success() {
                last_err = OpalError::Io(format!(
                    "POST {url} status {status}: {}",
                    text.chars().take(160).collect::<String>()
                ));
                continue;
            }
            let v: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    last_err = OpalError::Io(format!("json: {e}"));
                    continue;
                }
            };
            // JSON-RPC endpoints (Solana, EVM) return HTTP 200 with an
            // `error` field for rate limits / bad requests — don't let that
            // silently look like a valid empty result to the caller.
            if let Some(err) = v.get("error") {
                if !err.is_null() {
                    last_err = OpalError::Io(format!("rpc error from {url}: {err}"));
                    continue;
                }
            }
            return Ok(v);
        }
        Err(last_err)
    }

    /// POST an exact JSON string with extra headers (HMAC bodies must match bytes).
    pub fn post_raw_json_with_headers(
        &self,
        url: &str,
        body: &str,
        headers: &[(&str, &str)],
    ) -> Result<Value, OpalError> {
        let mut req = self
            .client
            .post(url)
            .header("Content-Type", "application/json; charset=UTF-8")
            .header("Accept", "application/json")
            .body(body.to_string());
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = req
            .send()
            .map_err(|e| OpalError::Io(format!("POST {url}: {e}")))?;
        let status = res.status();
        let text = res
            .text()
            .map_err(|e| OpalError::Io(format!("POST {url} body: {e}")))?;
        if !status.is_success() {
            return Err(OpalError::Io(format!(
                "POST {url} status {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        serde_json::from_str(&text).map_err(|e| OpalError::Io(format!("json: {e}")))
    }

    /// GET raw response body as text.
    pub fn get_text(&self, url: &str) -> Result<String, OpalError> {
        let res = self
            .client
            .get(url)
            .send()
            .map_err(|e| OpalError::Io(format!("GET {url}: {e}")))?;
        if !res.status().is_success() {
            return Err(OpalError::Io(format!(
                "GET {url} status {}",
                res.status()
            )));
        }
        res.text()
            .map_err(|e| OpalError::Io(format!("GET {url} body: {e}")))
    }

    pub fn evm_balance_wei(&self, chain: ChainId, address: &str) -> Result<u128, OpalError> {
        let result = self.eth_rpc(chain, "eth_getBalance", json!([address, "latest"]))?;
        let hex = result
            .as_str()
            .ok_or_else(|| OpalError::Io("bad balance".into()))?;
        u128_from_hex(hex)
    }

    pub fn evm_erc20_balance(
        &self,
        chain: ChainId,
        token: &str,
        holder: &str,
    ) -> Result<u128, OpalError> {
        // balanceOf(address)
        let mut data = String::from("0x70a08231");
        data.push_str(&format!("{:0>64}", holder.trim_start_matches("0x")));
        let result = self.eth_rpc(
            chain,
            "eth_call",
            json!([{ "to": token, "data": data }, "latest"]),
        )?;
        let hex = result.as_str().unwrap_or("0x0");
        u128_from_hex(hex)
    }

    /// Balance-only UTXO chain lookup — skips the UTXO list (send path needs that).
    pub fn btc_address_balance(&self, chain: ChainId, address: &str) -> Result<u64, OpalError> {
        match chain {
            ChainId::Btc | ChainId::Ltc => {
                let base = self.rpc_url(chain);
                let v = self.get_json(&format!("{base}/address/{address}"))?;
                let funded = v["chain_stats"]["funded_txo_sum"].as_u64().unwrap_or(0);
                let spent = v["chain_stats"]["spent_txo_sum"].as_u64().unwrap_or(0);
                let mem_f = v["mempool_stats"]["funded_txo_sum"].as_u64().unwrap_or(0);
                let mem_s = v["mempool_stats"]["spent_txo_sum"].as_u64().unwrap_or(0);
                Ok(funded.saturating_sub(spent) + mem_f.saturating_sub(mem_s))
            }
            ChainId::Doge => {
                let url = format!(
                    "https://api.blockcypher.com/v1/doge/main/addrs/{address}/balance"
                );
                match self.get_json(&url) {
                    Ok(v) => Ok(v["final_balance"].as_u64().unwrap_or(0)),
                    Err(_) => {
                        let v = self.get_json(&format!(
                            "https://dogechain.info/api/v1/address/balance/{address}"
                        ))?;
                        Ok(v["balance"]
                            .as_str()
                            .and_then(|s| s.parse().ok())
                            .or_else(|| v["balance"].as_u64())
                            .unwrap_or(0))
                    }
                }
            }
            _ => Err(OpalError::InvalidInput("not utxo chain".into())),
        }
    }

    pub fn btc_address_info(&self, chain: ChainId, address: &str) -> Result<(u64, Vec<Utxo>), OpalError> {
        match chain {
            ChainId::Btc | ChainId::Ltc => {
                let base = self.rpc_url(chain);
                let v = self.get_json(&format!("{base}/address/{address}"))?;
                let funded = v["chain_stats"]["funded_txo_sum"].as_u64().unwrap_or(0);
                let spent = v["chain_stats"]["spent_txo_sum"].as_u64().unwrap_or(0);
                let mem_f = v["mempool_stats"]["funded_txo_sum"].as_u64().unwrap_or(0);
                let mem_s = v["mempool_stats"]["spent_txo_sum"].as_u64().unwrap_or(0);
                let balance = funded.saturating_sub(spent) + mem_f.saturating_sub(mem_s);
                let utxos_v = self.get_json(&format!("{base}/address/{address}/utxo"))?;
                let mut utxos = Vec::new();
                if let Some(arr) = utxos_v.as_array() {
                    for u in arr {
                        utxos.push(Utxo {
                            txid: u["txid"].as_str().unwrap_or("").into(),
                            vout: u["vout"].as_u64().unwrap_or(0) as u32,
                            value: u["value"].as_u64().unwrap_or(0),
                        });
                    }
                }
                Ok((balance, utxos))
            }
            ChainId::Doge => {
                // Prefer BlockCypher for UTXOs + balance (dogechain.info often lacks UTXO lists).
                let url = format!(
                    "https://api.blockcypher.com/v1/doge/main/addrs/{address}?unspentOnly=true&includeScript=true"
                );
                match self.get_json(&url) {
                    Ok(v) => {
                        let balance = v["final_balance"].as_u64().unwrap_or(0);
                        let mut utxos = Vec::new();
                        if let Some(arr) = v["txrefs"].as_array() {
                            for u in arr {
                                utxos.push(Utxo {
                                    txid: u["tx_hash"].as_str().unwrap_or("").into(),
                                    vout: u["tx_output_n"].as_u64().unwrap_or(0) as u32,
                                    value: u["value"].as_u64().unwrap_or(0),
                                });
                            }
                        }
                        Ok((balance, utxos))
                    }
                    Err(_) => {
                        let v = self.get_json(&format!(
                            "https://dogechain.info/api/v1/address/balance/{address}"
                        ))?;
                        let balance = v["balance"]
                            .as_str()
                            .and_then(|s| s.parse().ok())
                            .or_else(|| v["balance"].as_u64())
                            .unwrap_or(0);
                        Ok((balance, Vec::new()))
                    }
                }
            }
            _ => Err(OpalError::InvalidInput("not utxo chain".into())),
        }
    }

    pub fn sol_balance_lamports(&self, address: &str) -> Result<u64, OpalError> {
        let url = self.rpc_url(ChainId::Sol);
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getBalance",
            "params": [address]
        });
        // Native balance must stay snappy — one attempt, no retry pile-up.
        let v = self.post_json_once(&url, &body)?;
        Ok(v["result"]["value"].as_u64().unwrap_or(0))
    }

    /// True if the address has ever signed a transaction (even with zero balance).
    pub fn sol_address_has_sigs(&self, address: &str) -> bool {
        let url = self.rpc_url(ChainId::Sol);
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getSignaturesForAddress",
            "params": [address, { "limit": 1 }]
        });
        match self.post_json_once(&url, &body) {
            Ok(v) => v["result"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Every SPL token account owned by `owner`, mint → raw base-unit total.
    /// Classic Token program only on the hot path — Token-2022 + mint fallbacks
    /// were stacking multi-second RPC waits on every Solana portfolio poll.
    pub fn sol_all_token_balances(&self, owner: &str) -> Result<HashMap<String, u64>, OpalError> {
        const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
        match self.sol_token_accounts_by_program(owner, TOKEN_PROGRAM) {
            Ok(totals) => Ok(totals),
            Err(_) => {
                // Last resort: parallel per-mint (one attempt each).
                let mut totals: HashMap<String, u64> = HashMap::new();
                let mints: Vec<&str> = [
                    token_contract(ChainId::Sol, "USDC"),
                    token_contract(ChainId::Sol, "USDT"),
                ]
                .into_iter()
                .flatten()
                .collect();
                std::thread::scope(|scope| {
                    let mut handles = Vec::new();
                    for mint in &mints {
                        let http = self.clone();
                        let owner = owner;
                        let mint = *mint;
                        handles.push(scope.spawn(move || {
                            http.sol_token_balance_for_mint(owner, mint)
                                .ok()
                                .map(|raw| (mint.to_string(), raw))
                        }));
                    }
                    for h in handles {
                        if let Ok(Some((mint, raw))) = h.join() {
                            if raw > 0 {
                                totals.insert(mint, raw);
                            }
                        }
                    }
                });
                Ok(totals)
            }
        }
    }

    fn sol_token_accounts_by_program(
        &self,
        owner: &str,
        program_id: &str,
    ) -> Result<HashMap<String, u64>, OpalError> {
        let url = self.rpc_url(ChainId::Sol);
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTokenAccountsByOwner",
            "params": [
                owner,
                { "programId": program_id },
                { "encoding": "jsonParsed" }
            ]
        });
        let v = self.post_json_once(&url, &body)?;
        let mut totals: HashMap<String, u64> = HashMap::new();
        if let Some(arr) = v["result"]["value"].as_array() {
            for acc in arr {
                let info = &acc["account"]["data"]["parsed"]["info"];
                let mint = info["mint"].as_str().unwrap_or("").to_string();
                if mint.is_empty() {
                    continue;
                }
                if let Some(amt) = info["tokenAmount"]["amount"].as_str() {
                    let raw: u64 = amt.parse().unwrap_or(0);
                    *totals.entry(mint).or_insert(0) =
                        totals.get(&mint).copied().unwrap_or(0).saturating_add(raw);
                }
            }
        }
        Ok(totals)
    }

    fn sol_token_balance_for_mint(&self, owner: &str, mint: &str) -> Result<u64, OpalError> {
        let url = self.rpc_url(ChainId::Sol);
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTokenAccountsByOwner",
            "params": [
                owner,
                { "mint": mint },
                { "encoding": "jsonParsed" }
            ]
        });
        let v = self.post_json_once(&url, &body)?;
        let mut total = 0u64;
        if let Some(arr) = v["result"]["value"].as_array() {
            for acc in arr {
                if let Some(amt) = acc["account"]["data"]["parsed"]["info"]["tokenAmount"]["amount"]
                    .as_str()
                {
                    total = total.saturating_add(amt.parse().unwrap_or(0));
                }
            }
        }
        Ok(total)
    }

    /// Native TON balance in nanotons.
    pub fn ton_balance_nanoton(&self, address: &str) -> Result<u64, OpalError> {
        let base = self.rpc_url(ChainId::Ton);
        let url = format!(
            "{base}/getAddressBalance?address={}",
            urlencoding_ton(address)
        );
        let v = self.get_json(&url)?;
        if v["ok"] == false {
            return Err(OpalError::Io(format!(
                "toncenter: {}",
                v["error"].as_str().unwrap_or("unknown")
            )));
        }
        let raw = v["result"]
            .as_str()
            .or_else(|| v["result"].as_u64().map(|_| ""))
            .unwrap_or("0");
        if let Some(n) = v["result"].as_u64() {
            return Ok(n);
        }
        Ok(raw.parse().unwrap_or(0))
    }

    pub fn ton_history(&self, address: &str) -> Result<Vec<TxRow>, OpalError> {
        let base = self.rpc_url(ChainId::Ton);
        let url = format!(
            "{base}/getTransactions?address={}&limit=30",
            urlencoding_ton(address)
        );
        let v = self.get_json(&url)?;
        let mut rows = Vec::new();
        let Some(arr) = v["result"].as_array() else {
            return Ok(rows);
        };
        for tx in arr {
            let txid = tx["transaction_id"]["hash"]
                .as_str()
                .or_else(|| tx["transaction_id"].as_str())
                .unwrap_or("")
                .to_string();
            if txid.is_empty() {
                continue;
            }
            let utime = json_unix_secs(&tx["utime"]);
            if utime.is_empty() {
                continue;
            }
            let in_msg = &tx["in_msg"];
            let in_value: u64 = in_msg["value"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .or_else(|| in_msg["value"].as_u64())
                .unwrap_or(0);
            let mut out_value: u64 = 0;
            if let Some(outs) = tx["out_msgs"].as_array() {
                for o in outs {
                    out_value = out_value.saturating_add(
                        o["value"]
                            .as_str()
                            .and_then(|s| s.parse().ok())
                            .or_else(|| o["value"].as_u64())
                            .unwrap_or(0),
                    );
                }
            }
            let (direction, amount_nano, counterparty) = if in_value > 0 && out_value == 0 {
                (
                    "in",
                    in_value,
                    in_msg["source"].as_str().map(|s| s.to_string()),
                )
            } else if out_value > 0 {
                let dest = tx["out_msgs"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|o| o["destination"].as_str())
                    .map(|s| s.to_string());
                ("out", out_value, dest)
            } else {
                ("self", 0, None)
            };
            if amount_nano == 0 && direction != "self" {
                continue;
            }
            let fee = tx["fee"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| tx["fee"].as_u64())
                .filter(|&f| f > 0 && direction == "out")
                .map(|f| base_units_to_string(f as u128, 9));
            rows.push(TxRow {
                txid: txid.clone(),
                amount: base_units_to_string(amount_nano as u128, 9),
                symbol: "TON".into(),
                direction: direction.into(),
                timestamp: utime,
                status: "confirmed".into(),
                fee,
                counterparty,
                explorer_url: explorer_tx_url(ChainId::Ton, &txid),
            });
        }
        Ok(rows)
    }

    pub fn prices_usd(&self) -> Result<HashMap<String, f64>, OpalError> {
        self.prices_in_fiat("usd")
    }

    pub fn prices_in_fiat(&self, fiat: &str) -> Result<HashMap<String, f64>, OpalError> {
        let vs = normalize_fiat(fiat);
        {
            let cache = SPOT_CACHE.lock();
            if let Some(c) = cache.get(vs) {
                // Hot book — same model as Exodus/Trezor: serve memory, refresh async.
                if c.at.elapsed() < Duration::from_secs(8) {
                    return Ok(c.map.clone());
                }
            }
        }
        if let Err(e) = self.refresh_price_book() {
            let cache = SPOT_CACHE.lock();
            if let Some(c) = cache.get(vs) {
                return Ok(c.map.clone());
            }
            return Err(e);
        }
        let cache = SPOT_CACHE.lock();
        Ok(cache.get(vs).map(|c| c.map.clone()).unwrap_or_default())
    }

    /// Instant read of whatever spot map we already have — never hits the
    /// network. Used while scraping on-chain balances so price I/O cannot
    /// stall SOL/ETH balance updates.
    pub fn cached_prices_in_fiat(&self, fiat: &str) -> HashMap<String, f64> {
        let vs = normalize_fiat(fiat);
        let cache = SPOT_CACHE.lock();
        cache.get(vs).map(|c| c.map.clone()).unwrap_or_default()
    }

    /// Rebuild the multi-fiat spot book from exchange tickers + FX.
    /// No CoinGecko / free-tier scrapers.
    pub fn refresh_price_book(&self) -> Result<(), OpalError> {
        {
            let cache = SPOT_CACHE.lock();
            let all_fresh = SUPPORTED_FIATS.iter().all(|f| {
                cache
                    .get(*f)
                    .is_some_and(|c| c.at.elapsed() < Duration::from_secs(4))
            });
            if all_fresh {
                return Ok(());
            }
        }

        let usd = self.fetch_exchange_usd_spots()?;
        let fx = self.fetch_usd_fx_rates()?;
        let now = Instant::now();
        let mut cache = SPOT_CACHE.lock();
        for fiat in SUPPORTED_FIATS {
            let rate = if *fiat == "usd" {
                1.0
            } else {
                match fx.get(*fiat).copied() {
                    Some(r) if r > 0.0 => r,
                    _ => continue,
                }
            };
            let mut map = HashMap::with_capacity(usd.len());
            for (id, px) in &usd {
                map.insert(id.clone(), px * rate);
            }
            cache.insert(
                (*fiat).to_string(),
                SpotCacheEntry { at: now, map },
            );
        }
        Ok(())
    }

    /// Binance USDT book (+ Kraken for XMR) → internal coin ids.
    fn fetch_exchange_usd_spots(&self) -> Result<HashMap<String, f64>, OpalError> {
        // Prefer POL (rebrand); keep MATIC as fallback if still listed.
        const PAIRS: &[(&str, &str)] = &[
            ("BTCUSDT", "bitcoin"),
            ("ETHUSDT", "ethereum"),
            ("SOLUSDT", "solana"),
            ("LTCUSDT", "litecoin"),
            ("DOGEUSDT", "dogecoin"),
            // Skip XMRUSDT — delisted on Binance.com but still quotes a frozen price.
            ("BNBUSDT", "binancecoin"),
            ("AVAXUSDT", "avalanche-2"),
            ("POLUSDT", "matic-network"),
            ("MATICUSDT", "matic-network"),
            ("TRXUSDT", "tron"),
            ("TONUSDT", "the-open-network"),
        ];
        let url = "https://api.binance.com/api/v3/ticker/price";
        let res = self
            .client
            .get(url)
            .send()
            .map_err(|e| OpalError::Io(e.to_string()))?;
        let status = res.status();
        let body = res.text().map_err(|e| OpalError::Io(e.to_string()))?;
        if !status.is_success() {
            return Err(OpalError::Io(format!("binance http {status}")));
        }
        #[derive(Deserialize)]
        struct Tick {
            symbol: String,
            price: String,
        }
        let ticks: Vec<Tick> =
            serde_json::from_str(&body).map_err(|e| OpalError::Io(format!("binance json: {e}")))?;
        let mut want: HashMap<&str, &str> = HashMap::new();
        for (sym, id) in PAIRS {
            want.insert(*sym, *id);
        }
        let mut out = HashMap::new();
        for t in ticks {
            if let Some(id) = want.get(t.symbol.as_str()) {
                if out.contains_key(*id) {
                    continue; // first match wins (POL before MATIC)
                }
                if let Ok(p) = t.price.parse::<f64>() {
                    if p > 0.0 {
                        out.insert((*id).to_string(), p);
                    }
                }
            }
        }
        // Stablecoins ≈ $1 — wallets hardcode these.
        out.insert("tether".into(), 1.0);
        out.insert("usd-coin".into(), 1.0);
        out.insert("dai".into(), 1.0);
        out.insert("xdai".into(), 1.0);

        // Monero: always Kraken. Binance XMRUSDT is delisted and returns a
        // frozen last price (~$118 from Feb 2024) that would poison the book.
        if let Ok(xmr) = self.fetch_kraken_xmr_usd() {
            out.insert("monero".into(), xmr);
        }

        if out.len() < 4 {
            return Err(OpalError::Io("exchange spots: too few symbols".into()));
        }
        Ok(out)
    }

    fn fetch_kraken_xmr_usd(&self) -> Result<f64, OpalError> {
        let url = "https://api.kraken.com/0/public/Ticker?pair=XMRUSD";
        let res = self
            .client
            .get(url)
            .send()
            .map_err(|e| OpalError::Io(e.to_string()))?;
        let status = res.status();
        let body = res.text().map_err(|e| OpalError::Io(e.to_string()))?;
        let v: Value = json_or_io_error(status, &body)?;
        let price = v
            .pointer("/result/XXMRZUSD/c/0")
            .or_else(|| v.pointer("/result/XMRUSD/c/0"))
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|p| *p > 0.0)
            .ok_or_else(|| OpalError::Io("kraken xmr: missing price".into()))?;
        Ok(price)
    }

    /// Units of each fiat per 1 USD (open.er-api free FX book).
    fn fetch_usd_fx_rates(&self) -> Result<HashMap<String, f64>, OpalError> {
        {
            let cache = FX_CACHE.lock();
            if let Some((at, map)) = cache.as_ref() {
                if at.elapsed() < Duration::from_secs(3_600) {
                    return Ok(map.clone());
                }
            }
        }
        let url = "https://open.er-api.com/v6/latest/USD";
        let res = self
            .client
            .get(url)
            .send()
            .map_err(|e| OpalError::Io(e.to_string()))?;
        let status = res.status();
        let body = res.text().map_err(|e| OpalError::Io(e.to_string()))?;
        let v: Value = json_or_io_error(status, &body)?;
        let rates = v
            .get("rates")
            .and_then(|r| r.as_object())
            .ok_or_else(|| OpalError::Io("fx: missing rates".into()))?;
        let mut out = HashMap::new();
        out.insert("usd".into(), 1.0);
        for fiat in SUPPORTED_FIATS {
            if *fiat == "usd" {
                continue;
            }
            let key = fiat.to_ascii_uppercase();
            if let Some(n) = rates.get(&key).and_then(|x| x.as_f64()) {
                if n > 0.0 {
                    out.insert((*fiat).to_string(), n);
                }
            }
        }
        if out.len() < 2 {
            return Err(OpalError::Io("fx: too few currencies".into()));
        }
        *FX_CACHE.lock() = Some((Instant::now(), out.clone()));
        Ok(out)
    }

    fn fx_rate_for(&self, vs: &str) -> f64 {
        let vs = vs.to_ascii_lowercase();
        if vs == "usd" {
            return 1.0;
        }
        if let Ok(fx) = self.fetch_usd_fx_rates() {
            if let Some(r) = fx.get(&vs).copied() {
                if r > 0.0 {
                    return r;
                }
            }
        }
        1.0
    }

    /// All recently cached spot maps keyed by fiat code (usd, eur, …).
    pub fn spot_prices_snapshot(&self) -> HashMap<String, HashMap<String, f64>> {
        let cache = SPOT_CACHE.lock();
        let mut out = HashMap::new();
        for (vs, entry) in cache.iter() {
            if entry.at.elapsed() < Duration::from_secs(120) {
                out.insert(vs.clone(), entry.map.clone());
            }
        }
        out
    }

    pub fn warm_all_fiat_prices(&self) {
        let _ = self.refresh_price_book();
    }

    pub fn fetch_all_fiat_spot_prices(&self) -> Result<(), OpalError> {
        self.refresh_price_book()
    }

    /// Exchange klines (Binance) — timestamps unix seconds, prices in `vs`.
    pub fn market_chart(
        &self,
        coin_id: &str,
        vs: &str,
        days: u32,
    ) -> Result<Vec<(u64, f64)>, OpalError> {
        let vs = vs.to_ascii_lowercase();
        let days = days.clamp(1, 365);
        let cache_key = format!("{coin_id}:{vs}:{days}");
        {
            let cache = CHART_CACHE.lock();
            if let Some((at, series)) = cache.get(&cache_key) {
                if at.elapsed() < Duration::from_secs(900) {
                    return Ok(series.clone());
                }
            }
        }

        let fx = self.fx_rate_for(&vs);
        let series = match coin_id {
            "tether" | "usd-coin" | "dai" | "xdai" => synthetic_stable_series(days, fx),
            // Always Kraken for Monero — Binance XMRUSDT klines freeze in Feb 2024.
            "monero" => match self.fetch_kraken_xmr_ohlc(days) {
                Ok(mut pts) => {
                    if (fx - 1.0).abs() > f64::EPSILON {
                        for p in &mut pts {
                            p.1 *= fx;
                        }
                    }
                    pts
                }
                Err(e) => {
                    let cache = CHART_CACHE.lock();
                    if let Some((_, series)) = cache.get(&cache_key) {
                        return Ok(series.clone());
                    }
                    return Err(e);
                }
            },
            _ => {
                let symbol = binance_usdt_symbol(coin_id)
                    .ok_or_else(|| OpalError::Io(format!("no chart feed for {coin_id}")))?;
                let (interval, limit) = kline_params(days);
                match self.fetch_binance_klines(symbol, interval, limit) {
                    Ok(mut pts) => {
                        // Reject delisted / frozen series (last candle too old).
                        if let Some((last_ts, _)) = pts.last() {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            if now.saturating_sub(*last_ts) > 2 * 86_400 {
                                return Err(OpalError::Io(format!(
                                    "stale klines for {symbol}"
                                )));
                            }
                        }
                        if (fx - 1.0).abs() > f64::EPSILON {
                            for p in &mut pts {
                                p.1 *= fx;
                            }
                        }
                        pts
                    }
                    Err(e) => {
                        let cache = CHART_CACHE.lock();
                        if let Some((_, series)) = cache.get(&cache_key) {
                            return Ok(series.clone());
                        }
                        return Err(e);
                    }
                }
            }
        };

        CHART_CACHE
            .lock()
            .insert(cache_key, (Instant::now(), series.clone()));
        Ok(series)
    }

    fn fetch_binance_klines(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
    ) -> Result<Vec<(u64, f64)>, OpalError> {
        let url = format!(
            "https://api.binance.com/api/v3/klines?symbol={symbol}&interval={interval}&limit={limit}"
        );
        let res = self
            .client
            .get(&url)
            .send()
            .map_err(|e| OpalError::Io(e.to_string()))?;
        let status = res.status();
        let body = res.text().map_err(|e| OpalError::Io(e.to_string()))?;
        if !status.is_success() {
            return Err(OpalError::Io(format!("binance klines http {status}")));
        }
        let rows: Vec<Value> =
            serde_json::from_str(&body).map_err(|e| OpalError::Io(format!("klines json: {e}")))?;
        let mut series = Vec::with_capacity(rows.len());
        for row in rows {
            let arr = row
                .as_array()
                .ok_or_else(|| OpalError::Io("klines row".into()))?;
            let open_ms = arr
                .first()
                .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
                .ok_or_else(|| OpalError::Io("klines ts".into()))?;
            let close = arr
                .get(4)
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|p| *p > 0.0)
                .ok_or_else(|| OpalError::Io("klines close".into()))?;
            series.push((open_ms / 1000, close));
        }
        if series.is_empty() {
            return Err(OpalError::Io("klines empty".into()));
        }
        Ok(series)
    }

    fn fetch_kraken_xmr_ohlc(&self, days: u32) -> Result<Vec<(u64, f64)>, OpalError> {
        let interval = match days {
            1 => 15,
            2..=7 => 60,
            8..=30 => 240,
            31..=90 => 720,
            _ => 1440,
        };
        let url = format!(
            "https://api.kraken.com/0/public/OHLC?pair=XMRUSD&interval={interval}"
        );
        let res = self
            .client
            .get(&url)
            .send()
            .map_err(|e| OpalError::Io(e.to_string()))?;
        let status = res.status();
        let body = res.text().map_err(|e| OpalError::Io(e.to_string()))?;
        let v: Value = json_or_io_error(status, &body)?;
        let rows = v
            .pointer("/result/XXMRZUSD")
            .or_else(|| v.pointer("/result/XMRUSD"))
            .and_then(|x| x.as_array())
            .ok_or_else(|| OpalError::Io("kraken ohlc missing".into()))?;
        let mut series = Vec::with_capacity(rows.len());
        for row in rows {
            let arr = row
                .as_array()
                .ok_or_else(|| OpalError::Io("kraken ohlc row".into()))?;
            let ts = arr
                .first()
                .and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
                .ok_or_else(|| OpalError::Io("kraken ohlc ts".into()))?;
            let close = arr
                .get(4)
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|p| *p > 0.0)
                .ok_or_else(|| OpalError::Io("kraken ohlc close".into()))?;
            series.push((ts, close));
        }
        if series.is_empty() {
            return Err(OpalError::Io("kraken ohlc empty".into()));
        }
        let max_pts = match days {
            1 => 96usize,
            2..=7 => 168,
            _ => 365,
        };
        if series.len() > max_pts {
            series = series.split_off(series.len() - max_pts);
        }
        Ok(series)
    }

    pub fn market_charts(
        &self,
        coin_ids: &[String],
        vs: &str,
        days: u32,
    ) -> HashMap<String, Vec<(u64, f64)>> {
        let vs = vs.to_string();
        let mut out = HashMap::new();
        // Exchange APIs tolerate wider fan-out than free scrapers.
        const BATCH: usize = 6;
        let ids: Vec<String> = coin_ids
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        for chunk_start in (0..ids.len()).step_by(BATCH) {
            let chunk_end = (chunk_start + BATCH).min(ids.len());
            std::thread::scope(|scope| {
                let mut handles = Vec::new();
                for id in &ids[chunk_start..chunk_end] {
                    let http = self.clone();
                    let id = id.clone();
                    let vs = vs.clone();
                    handles.push(scope.spawn(move || {
                        let series = http.market_chart(&id, &vs, days).ok();
                        (id, series)
                    }));
                }
                for handle in handles {
                    if let Ok((id, Some(series))) = handle.join() {
                        out.insert(id, series);
                    }
                }
            });
        }
        out
    }

    fn tron_base(&self) -> String {
        self.rpc_url(ChainId::Trx)
    }

    /// Native TRX balance in sun (1 TRX = 1_000_000 sun).
    pub fn trx_balance_sun(&self, address: &str) -> Result<u64, OpalError> {
        let base = self.tron_base();
        let v = self.post_json_once(
            &format!("{base}/wallet/getaccount"),
            &json!({ "address": address, "visible": true }),
        )?;
        Ok(v.get("balance").and_then(|b| b.as_u64()).unwrap_or(0))
    }

    /// TRC-20 balance via constant contract call. Returns raw token units.
    pub fn trx_trc20_balance(
        &self,
        owner: &str,
        contract: &str,
    ) -> Result<u128, OpalError> {
        let base = self.tron_base();
        let owner_hex = tron_address_to_hex(owner)?;
        // ABI: balanceOf(address) — 20-byte address left-padded to 32 bytes (no 0x41 prefix).
        let addr20 = &owner_hex[2..]; // strip leading "41"
        let parameter = format!("{addr20:0>64}");
        let v = self.post_json_once(
            &format!("{base}/wallet/triggerconstantcontract"),
            &json!({
                "owner_address": owner,
                "contract_address": contract,
                "function_selector": "balanceOf(address)",
                "parameter": parameter,
                "visible": true
            }),
        )?;
        let hex = v["constant_result"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|x| x.as_str())
            .unwrap_or("0");
        let cleaned = hex.trim_start_matches('0');
        if cleaned.is_empty() {
            Ok(0)
        } else {
            u128::from_str_radix(cleaned, 16)
                .map_err(|e| OpalError::Io(format!("trc20 balance: {e}")))
        }
    }

    pub fn trx_history(&self, address: &str) -> Result<Vec<TxRow>, OpalError> {
        let base = self.tron_base();
        let url = format!("{base}/v1/accounts/{address}/transactions?limit=50&only_confirmed=true");
        match self.get_json(&url) {
            Ok(v) => {
                let mut rows = Vec::new();
                if let Some(arr) = v["data"].as_array() {
                    for t in arr.iter().take(50) {
                        let txid = t["txID"].as_str().unwrap_or("").to_string();
                        if txid.is_empty() {
                            continue;
                        }
                        let contract = t["raw_data"]["contract"]
                            .as_array()
                            .and_then(|a| a.first());
                        let value = contract
                            .and_then(|c| c["parameter"]["value"]["amount"].as_u64())
                            .unwrap_or(0);
                        let owner = contract
                            .and_then(|c| {
                                c["parameter"]["value"]["owner_address"]
                                    .as_str()
                                    .or_else(|| c["parameter"]["value"]["owner_address"].as_str())
                            })
                            .unwrap_or("");
                        let direction = if owner.eq_ignore_ascii_case(address)
                            || owner_hex_matches(owner, address)
                        {
                            "out"
                        } else {
                            "in"
                        };
                        let ts = {
                            let raw = if t["block_timestamp"].is_null() {
                                &t["raw_data"]["timestamp"]
                            } else {
                                &t["block_timestamp"]
                            };
                            json_unix_secs(raw)
                        };
                        rows.push(TxRow {
                            txid: txid.clone(),
                            amount: base_units_to_string(value as u128, 6),
                            symbol: "TRX".into(),
                            direction: direction.into(),
                            timestamp: ts,
                            status: "confirmed".into(),
                            fee: None,
                            counterparty: None,
                            explorer_url: explorer_tx_url(ChainId::Trx, &txid),
                        });
                    }
                }
                Ok(rows)
            }
            Err(_) => Ok(Vec::new()),
        }
    }

    pub fn broadcast_evm(&self, chain: ChainId, raw_tx_hex: &str) -> Result<String, OpalError> {
        let result = self.eth_rpc(chain, "eth_sendRawTransaction", json!([raw_tx_hex]))?;
        result
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| OpalError::Io("no tx hash".into()))
    }

    pub fn broadcast_btc_like(&self, chain: ChainId, raw_hex: &str) -> Result<String, OpalError> {
        if chain == ChainId::Doge {
            let body = json!({ "tx": raw_hex });
            let v = self.post_json("https://api.blockcypher.com/v1/doge/main/txs/push", &body)?;
            if let Some(h) = v["tx"]["hash"].as_str().or_else(|| v["hash"].as_str()) {
                return Ok(h.to_string());
            }
            return Err(OpalError::Io(format!("doge broadcast failed: {v}")));
        }
        let base = self.rpc_url(chain);
        let res = self
            .client
            .post(format!("{base}/tx"))
            .body(raw_hex.to_string())
            .send()
            .map_err(|e| OpalError::Io(e.to_string()))?;
        let text = res.text().map_err(|e| OpalError::Io(e.to_string()))?;
        if text.len() == 64 && hex::decode(&text).is_ok() {
            Ok(text)
        } else {
            Err(OpalError::Io(format!("broadcast failed: {text}")))
        }
    }

    /// Raw transaction hex from a block explorer (BTC/LTC).
    pub fn btc_tx_hex(&self, chain: ChainId, txid: &str) -> Result<String, OpalError> {
        if !matches!(chain, ChainId::Btc | ChainId::Ltc) {
            return Err(OpalError::InvalidInput("not a UTXO explorer chain".into()));
        }
        let base = self.rpc_url(chain);
        let res = self
            .client
            .get(format!("{base}/tx/{txid}/hex"))
            .send()
            .map_err(|e| OpalError::Io(e.to_string()))?;
        let text = res.text().map_err(|e| OpalError::Io(e.to_string()))?;
        if hex::decode(text.trim()).is_ok() {
            Ok(text.trim().to_string())
        } else {
            Err(OpalError::Io(format!("bad tx hex for {txid}")))
        }
    }

    /// Fee estimates in sat/vB (economy / normal / priority).
    pub fn fee_estimates(&self, chain: ChainId) -> Result<FeeEstimates, OpalError> {
        let defaults = match chain {
            ChainId::Btc => FeeEstimates {
                economy: 2,
                normal: 8,
                priority: 20,
            },
            ChainId::Ltc => FeeEstimates {
                economy: 1,
                normal: 5,
                priority: 15,
            },
            ChainId::Doge => FeeEstimates {
                economy: 100_000,
                normal: 500_000,
                priority: 1_000_000,
            },
            _ => FeeEstimates::default(),
        };
        if !matches!(chain, ChainId::Btc | ChainId::Ltc) {
            return Ok(defaults);
        }
        let base = self.rpc_url(chain);
        match self.get_json(&format!("{base}/fee-estimates")) {
            Ok(v) => {
                // mempool.space style: { "1": 20.1, "3": 12.0, "6": 8.0, ... }
                let priority = v
                    .get("1")
                    .and_then(|x| x.as_f64())
                    .map(|f| f.ceil() as u64)
                    .unwrap_or(defaults.priority);
                let normal = v
                    .get("3")
                    .or_else(|| v.get("6"))
                    .and_then(|x| x.as_f64())
                    .map(|f| f.ceil() as u64)
                    .unwrap_or(defaults.normal);
                let economy = v
                    .get("144")
                    .or_else(|| v.get("504"))
                    .and_then(|x| x.as_f64())
                    .map(|f| f.ceil() as u64)
                    .unwrap_or(defaults.economy);
                Ok(FeeEstimates {
                    economy: economy.max(1),
                    normal: normal.max(1),
                    priority: priority.max(1),
                })
            }
            Err(_) => Ok(defaults),
        }
    }

    fn blockscout_base(chain: ChainId) -> Option<&'static str> {
        Some(match chain {
            ChainId::Eth => "https://eth.blockscout.com/api",
            ChainId::Arb => "https://arbitrum.blockscout.com/api",
            ChainId::Base => "https://base.blockscout.com/api",
            ChainId::Op => "https://optimism.blockscout.com/api",
            ChainId::Polygon => "https://polygon.blockscout.com/api",
            ChainId::Gnosis => "https://gnosis.blockscout.com/api",
            ChainId::Linea => "https://explorer.linea.build/api",
            ChainId::Avax => "https://avalanche.routescan.io/api",
            ChainId::Bsc => "https://bsc.blockscout.com/api",
            _ => return None,
        })
    }

    pub fn evm_history(&self, chain: ChainId, address: &str) -> Result<Vec<TxRow>, OpalError> {
        let Some(base) = Self::blockscout_base(chain) else {
            return Ok(Vec::new());
        };
        let native = chain.native_symbol();
        let mut rows = Vec::new();
        // hash -> native fee we paid, so a token-transfer row (which comes
        // from a separate endpoint) can still show what gas actually cost,
        // even though the native leg itself isn't rendered as its own row.
        let mut fee_by_hash: HashMap<String, u128> = HashMap::new();

        // Native ETH-like transfers (and contract calls, which show 0 value
        // but still cost gas — e.g. approvals, the "out" leg of a swap).
        if let Ok(v) = self.get_json(&format!(
            "{base}?module=account&action=txlist&address={address}&sort=desc"
        )) {
            if let Some(arr) = v["result"].as_array() {
                for t in arr.iter().take(40) {
                    let hash = t["hash"].as_str().unwrap_or("").to_string();
                    let value: u128 = t["value"].as_str().unwrap_or("0").parse().unwrap_or(0);
                    let is_out = t["from"].as_str().unwrap_or("").eq_ignore_ascii_case(address);
                    let gas_used: u128 = t["gasUsed"].as_str().unwrap_or("0").parse().unwrap_or(0);
                    let gas_price: u128 =
                        t["gasPrice"].as_str().unwrap_or("0").parse().unwrap_or(0);
                    let fee = gas_used * gas_price;
                    if is_out && fee > 0 {
                        fee_by_hash.insert(hash.clone(), fee);
                    }

                    // Pure contract calls with no native value transferred
                    // (approvals, swap routing, etc.) aren't a useful "0 ETH"
                    // row on their own — the token-transfer pass below
                    // covers what actually moved, using the fee recorded
                    // above.
                    if value == 0 && t["input"].as_str().is_some_and(|d| d.len() > 2) {
                        continue;
                    }
                    let to = t["to"].as_str().unwrap_or("").to_string();
                    let from = t["from"].as_str().unwrap_or("").to_string();
                    rows.push(TxRow {
                        txid: hash.clone(),
                        amount: base_units_to_string(value, 18),
                        symbol: native.into(),
                        direction: if is_out {
                            if from.eq_ignore_ascii_case(&to) {
                                "self".into()
                            } else {
                                "out".into()
                            }
                        } else {
                            "in".into()
                        },
                        timestamp: json_unix_secs(&t["timeStamp"]),
                        status: if t["isError"].as_str() == Some("1") {
                            "failed".into()
                        } else {
                            "confirmed".into()
                        },
                        fee: (is_out && fee > 0).then(|| base_units_to_string(fee, 18)),
                        counterparty: Some(if is_out { to } else { from }),
                        explorer_url: explorer_tx_url(chain, &hash),
                    });
                }
            }
        }

        // ERC-20 transfers — separate endpoint, each entry already carries
        // its own token symbol/decimals so USDC/USDT/etc. show correctly
        // instead of as a misleading "0 ETH" row from the list above.
        if let Ok(v) = self.get_json(&format!(
            "{base}?module=account&action=tokentx&address={address}&sort=desc"
        )) {
            if let Some(arr) = v["result"].as_array() {
                for t in arr.iter().take(40) {
                    let hash = t["hash"].as_str().unwrap_or("").to_string();
                    let raw: u128 = t["value"].as_str().unwrap_or("0").parse().unwrap_or(0);
                    let decimals: u32 = t["tokenDecimal"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(18);
                    let symbol = t["tokenSymbol"].as_str().unwrap_or("TOKEN").to_string();
                    let is_out = t["from"].as_str().unwrap_or("").eq_ignore_ascii_case(address);
                    let to = t["to"].as_str().unwrap_or("").to_string();
                    let from = t["from"].as_str().unwrap_or("").to_string();
                    rows.push(TxRow {
                        txid: hash.clone(),
                        amount: base_units_to_string(raw, decimals),
                        symbol,
                        direction: if is_out { "out".into() } else { "in".into() },
                        timestamp: json_unix_secs(&t["timeStamp"]),
                        status: "confirmed".into(),
                        fee: fee_by_hash.get(&hash).map(|f| base_units_to_string(*f, 18)),
                        counterparty: Some(if is_out { to } else { from }),
                        explorer_url: explorer_tx_url(chain, &hash),
                    });
                }
            }
        }

        rows.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        rows.truncate(50);
        Ok(rows)
    }

    pub fn btc_history(&self, chain: ChainId, address: &str) -> Result<Vec<TxRow>, OpalError> {
        if !matches!(chain, ChainId::Btc | ChainId::Ltc) {
            return Ok(Vec::new());
        }
        let base = self.rpc_url(chain);
        let v = self.get_json(&format!("{base}/address/{address}/txs"))?;
        let symbol = chain.native_symbol().to_string();
        let mut rows = Vec::new();
        if let Some(arr) = v.as_array() {
            for t in arr.iter().take(50) {
                let txid = t["txid"].as_str().unwrap_or("").to_string();

                // A tx is "ours to send" if any input spends from our own
                // address — mempool.space embeds the previous output (and
                // its owning address) directly on each vin, so no extra
                // lookups are needed.
                let our_in: u64 = t["vin"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter(|x| x["prevout"]["scriptpubkey_address"].as_str() == Some(address))
                            .map(|x| x["prevout"]["value"].as_u64().unwrap_or(0))
                            .sum()
                    })
                    .unwrap_or(0);
                let our_out: u64 = t["vout"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter(|x| x["scriptpubkey_address"].as_str() == Some(address))
                            .map(|x| x["value"].as_u64().unwrap_or(0))
                            .sum()
                    })
                    .unwrap_or(0);

                let sending = our_in > 0;
                let (direction, amount_sats, fee) = utxo_direction(our_in, our_out, t["fee"].as_u64());

                let counterparty = if sending {
                    t["vout"].as_array().and_then(|arr| {
                        arr.iter()
                            .find(|x| x["scriptpubkey_address"].as_str().is_some_and(|a| a != address))
                            .and_then(|x| x["scriptpubkey_address"].as_str())
                            .map(|s| s.to_string())
                    })
                } else {
                    t["vin"].as_array().and_then(|arr| {
                        arr.iter()
                            .find(|x| {
                                x["prevout"]["scriptpubkey_address"]
                                    .as_str()
                                    .is_some_and(|a| a != address)
                            })
                            .and_then(|x| x["prevout"]["scriptpubkey_address"].as_str())
                            .map(|s| s.to_string())
                    })
                };

                rows.push(TxRow {
                    txid: txid.clone(),
                    amount: base_units_to_string(amount_sats as u128, 8),
                    symbol: symbol.clone(),
                    direction: direction.into(),
                    timestamp: json_unix_secs(&t["status"]["block_time"]),
                    status: if t["status"]["confirmed"].as_bool() == Some(true) {
                        "confirmed".into()
                    } else {
                        "pending".into()
                    },
                    fee: fee.map(|f| base_units_to_string(f as u128, 8)),
                    counterparty,
                    explorer_url: explorer_tx_url(chain, &txid),
                });
            }
        }
        Ok(rows)
    }

    pub fn doge_history(&self, address: &str) -> Result<Vec<TxRow>, OpalError> {
        let url = format!("https://api.blockcypher.com/v1/doge/main/addrs/{address}/full?limit=50");
        match self.get_json(&url) {
            Ok(v) => {
                let mut rows = Vec::new();
                if let Some(arr) = v["txs"].as_array() {
                    for t in arr.iter().take(50) {
                        let txid = t["hash"].as_str().unwrap_or("").to_string();
                        let confirmed = t["confirmations"].as_u64().unwrap_or(0) > 0;

                        let owns = |addrs: &Value| {
                            addrs
                                .as_array()
                                .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(address)))
                        };
                        let our_in: u64 = t["inputs"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter(|x| owns(&x["addresses"]))
                                    .map(|x| x["output_value"].as_u64().unwrap_or(0))
                                    .sum()
                            })
                            .unwrap_or(0);
                        let our_out: u64 = t["outputs"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter(|x| owns(&x["addresses"]))
                                    .map(|x| x["value"].as_u64().unwrap_or(0))
                                    .sum()
                            })
                            .unwrap_or(0);

                        let sending = our_in > 0;
                        let (direction, amount_sats, fee) =
                            utxo_direction(our_in, our_out, t["fees"].as_u64());

                        let find_other = |list: &Value| {
                            list.as_array().and_then(|arr| {
                                arr.iter().find_map(|x| {
                                    x["addresses"].as_array().and_then(|a| {
                                        a.iter()
                                            .map(|v| v.as_str().unwrap_or(""))
                                            .find(|a| *a != address && !a.is_empty())
                                            .map(|s| s.to_string())
                                    })
                                })
                            })
                        };
                        let counterparty = if sending {
                            find_other(&t["outputs"])
                        } else {
                            find_other(&t["inputs"])
                        };

                        rows.push(TxRow {
                            txid: txid.clone(),
                            amount: base_units_to_string(amount_sats as u128, 8),
                            symbol: "DOGE".into(),
                            direction: direction.into(),
                            timestamp: {
                                let mut ts = json_unix_secs(&t["received"]);
                                if ts.is_empty() {
                                    ts = json_unix_secs(&t["confirmed"]);
                                }
                                ts
                            },
                            status: if confirmed {
                                "confirmed".into()
                            } else {
                                "pending".into()
                            },
                            fee: fee.map(|f| base_units_to_string(f as u128, 8)),
                            counterparty,
                            explorer_url: explorer_tx_url(ChainId::Doge, &txid),
                        });
                    }
                }
                Ok(rows)
            }
            Err(_) => Ok(Vec::new()),
        }
    }

    pub fn sol_history(&self, address: &str) -> Result<Vec<TxRow>, OpalError> {
        let url = self.chain_rpc(ChainId::Sol);
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getSignaturesForAddress",
            "params": [address, { "limit": 40 }]
        });
        let sigs = match self.post_json(&url, &body) {
            Ok(v) => v["result"].as_array().cloned().unwrap_or_default(),
            Err(_) => return Ok(Vec::new()),
        };

        // Recent window for charts — concurrent getTransaction keeps this fast.
        let sigs: Vec<Value> = sigs.into_iter().take(40).collect();

        let mut rows: Vec<Option<TxRow>> = (0..sigs.len()).map(|_| None).collect();
        const BATCH: usize = 8;
        for chunk_start in (0..sigs.len()).step_by(BATCH) {
            let chunk_end = (chunk_start + BATCH).min(sigs.len());
            std::thread::scope(|scope| {
                let mut handles = Vec::new();
                for (offset, t) in sigs[chunk_start..chunk_end].iter().enumerate() {
                    let idx = chunk_start + offset;
                    let txid = t["signature"].as_str().unwrap_or("").to_string();
                    if txid.is_empty() {
                        continue;
                    }
                    let no_error = t["err"].is_null();
                    let timestamp = json_unix_secs(&t["blockTime"]);
                    let url = &url;
                    handles.push((
                        idx,
                        scope.spawn(move || {
                            self.sol_history_row(url, address, txid, no_error, timestamp)
                        }),
                    ));
                }
                for (idx, h) in handles {
                    if let Ok(row) = h.join() {
                        rows[idx] = Some(row);
                    }
                }
            });
        }
        Ok(rows.into_iter().flatten().collect())
    }

    fn sol_history_row(
        &self,
        url: &str,
        address: &str,
        txid: String,
        no_error: bool,
        mut timestamp: String,
    ) -> TxRow {
        let detail = self.post_json_once(
            url,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getTransaction",
                "params": [txid, { "encoding": "json", "maxSupportedTransactionVersion": 0 }]
            }),
        );

        let mut amount = "—".to_string();
        let mut direction = "unknown".to_string();
        let mut fee: Option<String> = None;
        if let Ok(v) = detail {
            if timestamp.is_empty() {
                timestamp = json_unix_secs(&v["result"]["blockTime"]);
            }
            let keys = v["result"]["transaction"]["message"]["accountKeys"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let idx = keys.iter().position(|k| k.as_str() == Some(address));
            let pre = v["result"]["meta"]["preBalances"].as_array();
            let post = v["result"]["meta"]["postBalances"].as_array();
            if let (Some(idx), Some(pre), Some(post)) = (idx, pre, post) {
                let before = pre.get(idx).and_then(|x| x.as_i64()).unwrap_or(0);
                let after = post.get(idx).and_then(|x| x.as_i64()).unwrap_or(0);
                let delta = after - before;
                if delta > 0 {
                    direction = "in".into();
                    amount = base_units_to_string(delta as u128, 9);
                } else if delta < 0 {
                    direction = "out".into();
                    amount = base_units_to_string((-delta) as u128, 9);
                } else {
                    direction = "self".into();
                    amount = "0".into();
                }
                if idx == 0 {
                    if let Some(f) = v["result"]["meta"]["fee"].as_u64() {
                        fee = Some(base_units_to_string(f as u128, 9));
                    }
                }
            }
        }

        TxRow {
            txid: txid.clone(),
            amount,
            symbol: "SOL".into(),
            direction,
            timestamp,
            status: if no_error { "confirmed".into() } else { "failed".into() },
            fee,
            counterparty: None,
            explorer_url: explorer_tx_url(ChainId::Sol, &txid),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Utxo {
    pub txid: String,
    pub vout: u32,
    pub value: u64,
}

use serde::Serialize;

/// Normalize explorer timestamps to unix-seconds strings for the UI/charts.
/// Accepts JSON numbers, numeric strings, millisecond values, and RFC3339/ISO.
fn json_unix_secs(v: &Value) -> String {
    if let Some(n) = v.as_u64() {
        return normalize_unix_secs(n);
    }
    if let Some(n) = v.as_i64() {
        if n <= 0 {
            return String::new();
        }
        return normalize_unix_secs(n as u64);
    }
    if let Some(f) = v.as_f64() {
        if f.is_finite() && f > 0.0 {
            return normalize_unix_secs(f as u64);
        }
        return String::new();
    }
    let Some(s) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return String::new();
    };
    if let Ok(n) = s.parse::<u64>() {
        return normalize_unix_secs(n);
    }
    if let Ok(f) = s.parse::<f64>() {
        if f.is_finite() && f > 0.0 {
            return normalize_unix_secs(f as u64);
        }
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return dt.timestamp().max(0).to_string();
    }
    // BlockCypher sometimes omits the `Z`; treat as UTC.
    if !s.ends_with('Z') && !s.contains('+') {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&format!("{s}Z")) {
            return dt.timestamp().max(0).to_string();
        }
    }
    String::new()
}

fn normalize_unix_secs(n: u64) -> String {
    // Values ≥ 1e12 are almost certainly milliseconds.
    if n >= 1_000_000_000_000 {
        (n / 1000).to_string()
    } else {
        n.to_string()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TxRow {
    pub txid: String,
    /// Human-readable amount in the asset's native units (already divided by
    /// decimals) — never a raw satoshi/wei/lamport integer.
    pub amount: String,
    /// Ticker shown next to `amount` — usually the chain's native symbol, but
    /// an ERC-20/TRC-20 row carries its own token symbol instead.
    pub symbol: String,
    /// "in" | "out" | "self" | "unknown"
    pub direction: String,
    pub timestamp: String,
    pub status: String,
    /// Network fee paid, in native units — only set on rows we paid for.
    pub fee: Option<String>,
    /// The other side of the transfer (sender when direction=in, recipient
    /// when direction=out), when we're able to determine it cheaply.
    pub counterparty: Option<String>,
    pub explorer_url: String,
}

/// Format a raw base-unit integer (satoshis, wei, lamports, …) as a trimmed
/// human-readable decimal string — avoids float rounding on large values.
pub fn base_units_to_string(raw: u128, decimals: u32) -> String {
    if decimals == 0 {
        return raw.to_string();
    }
    let denom = 10u128.pow(decimals);
    let whole = raw / denom;
    let frac = raw % denom;
    let frac_str = format!("{:0width$}", frac, width = decimals as usize);
    let trimmed = frac_str.trim_end_matches('0');
    if trimmed.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{trimmed}")
    }
}

/// Shared UTXO-style (BTC/LTC/DOGE) direction + net-amount math, given how
/// much of a transaction's inputs and outputs belong to *our* address.
/// Pulled out of `btc_history`/`doge_history` so the logic that decides
/// "did we send or receive, and how much" can actually be unit tested
/// without a live explorer call.
fn utxo_direction(our_in: u64, our_out: u64, fee: Option<u64>) -> (&'static str, u64, Option<u64>) {
    if our_in > 0 {
        // We signed at least one input — this is a spend of ours, possibly
        // with change coming back to the same address.
        let net = our_in.saturating_sub(our_out);
        if net == 0 {
            ("self", fee.unwrap_or(0), fee)
        } else {
            ("out", net, fee)
        }
    } else {
        ("in", our_out, None)
    }
}

pub fn explorer_tx_url(chain: ChainId, txid: &str) -> String {
    match chain {
        ChainId::Btc => format!("https://mempool.space/tx/{txid}"),
        ChainId::Ltc => format!("https://litecoinspace.org/tx/{txid}"),
        ChainId::Doge => format!("https://dogechain.info/tx/{txid}"),
        ChainId::Eth => format!("https://etherscan.io/tx/{txid}"),
        ChainId::Arb => format!("https://arbiscan.io/tx/{txid}"),
        ChainId::Base => format!("https://basescan.org/tx/{txid}"),
        ChainId::Op => format!("https://optimistic.etherscan.io/tx/{txid}"),
        ChainId::Polygon => format!("https://polygonscan.com/tx/{txid}"),
        ChainId::Avax => format!("https://snowtrace.io/tx/{txid}"),
        ChainId::Bsc => format!("https://bscscan.com/tx/{txid}"),
        ChainId::Gnosis => format!("https://gnosisscan.io/tx/{txid}"),
        ChainId::Trx => format!("https://tronscan.org/#/transaction/{txid}"),
        ChainId::Linea => format!("https://lineascan.build/tx/{txid}"),
        ChainId::Sol => format!("https://solscan.io/tx/{txid}"),
        ChainId::Ton => format!("https://tonviewer.com/transaction/{txid}"),
        ChainId::Xmr => format!("https://xmrchain.net/tx/{txid}"),
    }
}

fn urlencoding_ton(address: &str) -> String {
    address
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

pub fn u128_from_hex(hex_str: &str) -> Result<u128, OpalError> {
    let s = hex_str.trim_start_matches("0x");
    if s.is_empty() {
        return Ok(0);
    }
    u128::from_str_radix(s, 16).map_err(|e| OpalError::Io(format!("hex: {e}")))
}

/// Decode a Tron Base58Check address to hex (including leading `41`).
pub fn tron_address_to_hex(address: &str) -> Result<String, OpalError> {
    let raw = bs58::decode(address.trim())
        .with_check(None)
        .into_vec()
        .map_err(|e| OpalError::InvalidInput(format!("tron address: {e}")))?;
    if raw.len() != 21 || raw[0] != 0x41 {
        return Err(OpalError::InvalidInput("not a Tron address".into()));
    }
    Ok(hex::encode(raw))
}

fn owner_hex_matches(owner: &str, base58: &str) -> bool {
    let Ok(hex_addr) = tron_address_to_hex(base58) else {
        return false;
    };
    let o = owner.trim_start_matches("0x").to_ascii_lowercase();
    o == hex_addr || o == format!("41{}", &hex_addr[2..])
}

/// Fee urgency for UTXO and Solana sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeePreset {
    Economy,
    #[default]
    Normal,
    Priority,
}

#[derive(Debug, Clone, Copy)]
pub struct FeeEstimates {
    pub economy: u64,
    pub normal: u64,
    pub priority: u64,
}

impl Default for FeeEstimates {
    fn default() -> Self {
        Self {
            economy: 2,
            normal: 8,
            priority: 20,
        }
    }
}

/// Allowlisted token contracts.
pub fn token_contract(chain: ChainId, symbol: &str) -> Option<&'static str> {
    let sym = symbol.to_ascii_uppercase();
    match (chain, sym.as_str()) {
        (ChainId::Eth, "USDC") => Some("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
        (ChainId::Eth, "USDT") => Some("0xdAC17F958D2ee523a2206206994597C13D831ec7"),
        (ChainId::Eth, "DAI") => Some("0x6B175474E89094C44Da98b954EedeAC495271d0F"),
        (ChainId::Arb, "USDC") => Some("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
        (ChainId::Arb, "USDT") => Some("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
        (ChainId::Arb, "DAI") => Some("0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1"),
        (ChainId::Base, "USDC") => Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
        (ChainId::Base, "USDT") => Some("0xfde4C96c8593536E9F781b9eB2e2d0eF0b39Fb46"),
        (ChainId::Base, "DAI") => Some("0x50c5725949A6F0c72E6C4a641F24049A917DB0Cb"),
        (ChainId::Op, "USDC") => Some("0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85"),
        (ChainId::Op, "USDT") => Some("0x94b008aA00579c1307B0EF2c499aD98a8ce58e58"),
        (ChainId::Op, "DAI") => Some("0xDA10009cBd5D07dd0CeCc66161FC93D7c9000da1"),
        (ChainId::Polygon, "USDC") => Some("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359"),
        (ChainId::Polygon, "USDT") => Some("0xc2132D05D31c914a87C6611C10748AEb04B58e8F"),
        (ChainId::Polygon, "DAI") => Some("0x8f3Cf7ad23Cd3CaDbD9735AFf958023239c6A063"),
        (ChainId::Avax, "USDC") => Some("0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E"),
        (ChainId::Avax, "USDT") => Some("0x9702230A8Ea53601f5cD2dc00fDBc13d4dF4A8c7"),
        (ChainId::Avax, "DAI") => Some("0xd586E7F844cEa2F87f50152665BCbc2C279D3C67"),
        (ChainId::Bsc, "USDC") => Some("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d"),
        (ChainId::Bsc, "USDT") => Some("0x55d398326f99059fF775485246999027B3197955"),
        (ChainId::Bsc, "DAI") => Some("0x1AF3F329e8BE154074D8769D1FFa4eE0581C4a66"),
        (ChainId::Gnosis, "USDC") => Some("0xDDAfbb505ad214D7b80b1f830fcCc89B60fb7A83"),
        (ChainId::Gnosis, "USDT") => Some("0x4ECaBa5870353805a9F068101A40E0f32ed605C6"),
        (ChainId::Gnosis, "DAI") => Some("0xe91D153E0b41518A2Ce8Dd3D7944Fa863463a97d"),
        (ChainId::Trx, "USDT") => Some("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"),
        (ChainId::Trx, "USDC") => Some("TEkxiTehnzSmSe2XqrBj4w32RUN966rdz8"),
        (ChainId::Linea, "USDC") => Some("0x176211869cA2b568f2A7D4EE941E073a821EE1ff"),
        (ChainId::Linea, "USDT") => Some("0xA219439258ca9da29E9Cc4cE5596924745e12B93"),
        (ChainId::Sol, "USDC") => Some("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),
        (ChainId::Sol, "USDT") => Some("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"),
        _ => None,
    }
}

pub fn token_decimals(symbol: &str) -> u32 {
    match symbol.to_ascii_uppercase().as_str() {
        "USDC" | "USDT" => 6,
        "DAI" => 18,
        _ => 18,
    }
}

/// Decimals for allowlisted tokens on a specific chain (BSC pegs are 18).
pub fn token_decimals_on(chain: ChainId, symbol: &str) -> u32 {
    let sym = symbol.to_ascii_uppercase();
    match (chain, sym.as_str()) {
        (ChainId::Bsc, "USDC" | "USDT" | "DAI") => 18,
        _ => token_decimals(&sym),
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn base_units_formats_whole_and_fractional() {
        assert_eq!(base_units_to_string(0, 8), "0");
        assert_eq!(base_units_to_string(100_000_000, 8), "1");
        assert_eq!(base_units_to_string(150_000, 8), "0.0015");
        assert_eq!(base_units_to_string(123_456_789, 8), "1.23456789");
        // wei: 1.5 ETH
        assert_eq!(base_units_to_string(1_500_000_000_000_000_000, 18), "1.5");
        // no decimals at all (e.g. an integer count)
        assert_eq!(base_units_to_string(42, 0), "42");
    }

    #[test]
    fn utxo_direction_receiving() {
        // Nothing of ours was spent — this is money coming in.
        let (dir, amount, fee) = utxo_direction(0, 50_000, None);
        assert_eq!(dir, "in");
        assert_eq!(amount, 50_000);
        assert_eq!(fee, None);
    }

    #[test]
    fn utxo_direction_sending_with_change() {
        // Spent a 100k-sat input, 40k went to the recipient, 59.5k came
        // back to us as change, 500 sats fee. Net leaving our control
        // should be the recipient amount + fee, NOT the raw input value.
        let (dir, amount, fee) = utxo_direction(100_000, 59_500, Some(500));
        assert_eq!(dir, "out");
        assert_eq!(amount, 40_500);
        assert_eq!(fee, Some(500));
    }

    #[test]
    fn utxo_direction_pure_consolidation_shows_only_the_fee() {
        // All inputs are ours and all outputs come back to the same
        // address (a UTXO consolidation) — only the fee actually left,
        // net of change, so it should show as a tiny "out" of just the fee
        // rather than the full input amount.
        let (dir, amount, fee) = utxo_direction(100_000, 99_800, Some(200));
        assert_eq!(dir, "out");
        assert_eq!(amount, 200);
        assert_eq!(fee, Some(200));
    }

    #[test]
    fn utxo_direction_zero_fee_self_transfer_is_flagged_self() {
        // Degenerate edge case (no real network charges a literal 0 fee,
        // but nothing should divide-by-zero or misreport it as a spend).
        let (dir, amount, fee) = utxo_direction(100_000, 100_000, Some(0));
        assert_eq!(dir, "self");
        assert_eq!(amount, 0);
        assert_eq!(fee, Some(0));
    }

    #[test]
    fn utxo_direction_sending_no_change() {
        let (dir, amount, fee) = utxo_direction(50_000, 0, Some(300));
        assert_eq!(dir, "out");
        assert_eq!(amount, 50_000);
        assert_eq!(fee, Some(300));
    }
}
