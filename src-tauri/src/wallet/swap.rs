//! Cross-chain / SOL swaps: Jupiter (SOL) + FixedFloat (others).
//!
//! FixedFloat `/api/v2/*` requires API key + HMAC. Live quotes and pair
//! minimums also work without keys via the public XML rate export.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::error::OpalError;
use crate::network::HttpCtx;
use crate::wallet::ChainId;

type HmacSha256 = Hmac<Sha256>;

static FF_RATES_CACHE: Lazy<Mutex<Option<(Instant, String)>>> = Lazy::new(|| Mutex::new(None));

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapQuote {
    pub provider: String,
    pub from_asset: String,
    pub to_asset: String,
    pub from_amount: String,
    pub to_amount: String,
    pub rate: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_amount: Option<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedFloatOrder {
    pub id: String,
    pub token: String,
    pub status: String,
    pub from_amount: String,
    pub to_amount: String,
    pub deposit_address: String,
    pub deposit_tag: Option<String>,
    pub to_address: String,
    pub from_ccy: String,
    pub to_ccy: String,
    pub order_url: String,
    pub raw: Value,
}

/// Map an Opal portfolio asset to a FixedFloat currency code.
pub fn fixedfloat_ccy(symbol: &str, chain: &str) -> Option<String> {
    let sym = symbol.trim().to_ascii_uppercase();
    let chain = chain.trim().to_ascii_lowercase();
    match (sym.as_str(), chain.as_str()) {
        ("BTC", _) => Some("BTC".into()),
        ("LTC", _) => Some("LTC".into()),
        ("DOGE", _) => Some("DOGE".into()),
        ("XMR", _) => Some("XMR".into()),
        ("SOL", "sol") => Some("SOL".into()),
        ("TRX", "trx") => Some("TRX".into()),
        ("ETH", "eth") => Some("ETH".into()),
        ("ETH", "arb") => Some("ETHARBITRUM".into()),
        ("ETH", "base") => Some("ETHBASE".into()),
        ("ETH", "bsc") => Some("ETHBSC".into()),
        ("USDC", "eth") => Some("USDCETH".into()),
        ("USDC", "sol") => Some("USDCSOL".into()),
        ("USDC", "arb") => Some("USDCARBITRUM".into()),
        ("USDC", "base") => Some("USDCBASE".into()),
        ("USDC", "polygon") => Some("USDCMATIC".into()),
        ("USDC", "avax") => Some("USDCAVAX".into()),
        ("USDC", "bsc") => Some("USDCBSC".into()),
        ("USDT", "eth") => Some("USDT".into()),
        ("USDT", "sol") => Some("USDTSOL".into()),
        ("USDT", "trx") => Some("USDTTRC".into()),
        ("USDT", "arb") => Some("USDTARBITRUM".into()),
        ("USDT", "polygon") => Some("USDTMATIC".into()),
        ("USDT", "avax") => Some("USDTAVAX".into()),
        ("USDT", "bsc") => Some("USDTBSC".into()),
        // Native gas tokens that FF lists under network-specific ETH codes only.
        ("ETH", _) => None,
        (s, _) => Some(s.to_string()),
    }
}

fn atomic_to_decimal(atomic: &str, decimals: u32) -> String {
    let Ok(n) = atomic.parse::<u128>() else {
        return "0".into();
    };
    let base = 10u128.pow(decimals);
    let whole = n / base;
    let frac = n % base;
    if decimals == 0 {
        return whole.to_string();
    }
    let frac_s = format!("{:0width$}", frac, width = decimals as usize);
    let frac_trim = frac_s.trim_end_matches('0');
    if frac_trim.is_empty() {
        format!("{whole}.0")
    } else {
        format!("{whole}.{frac_trim}")
    }
}

/// Jupiter quote for SOL → USDC/USDT (or reverse) on Solana mainnet.
pub fn jupiter_quote(
    http: &HttpCtx,
    input_mint: &str,
    output_mint: &str,
    amount_atomic: u64,
    slippage_bps: u16,
    from_symbol: &str,
    to_symbol: &str,
) -> Result<SwapQuote, OpalError> {
    let url = format!(
        "https://quote-api.jup.ag/v6/quote?inputMint={input_mint}&outputMint={output_mint}&amount={amount_atomic}&slippageBps={slippage_bps}"
    );
    let v = http.get_json(&url)?;
    let out_atomic = v["outAmount"].as_str().unwrap_or("0");
    let in_fallback = amount_atomic.to_string();
    let in_atomic = v["inAmount"].as_str().unwrap_or(&in_fallback);
    let in_decimals: u32 = if from_symbol.eq_ignore_ascii_case("SOL") {
        9
    } else {
        6
    };
    let out_decimals: u32 = if to_symbol.eq_ignore_ascii_case("SOL") {
        9
    } else {
        6
    };
    let from_amount = atomic_to_decimal(in_atomic, in_decimals);
    let to_amount = atomic_to_decimal(out_atomic, out_decimals);
    let rate = {
        let a = from_amount.parse::<f64>().unwrap_or(0.0);
        let b = to_amount.parse::<f64>().unwrap_or(0.0);
        if a > 0.0 {
            format!("{:.8}", b / a)
        } else {
            "0".into()
        }
    };
    Ok(SwapQuote {
        provider: "jupiter".into(),
        from_asset: from_symbol.to_ascii_uppercase(),
        to_asset: to_symbol.to_ascii_uppercase(),
        from_amount,
        to_amount,
        rate,
        min_amount: None,
        max_amount: None,
        errors: vec![],
        raw: v,
    })
}

/// Build a Jupiter swap transaction (base64) for the user wallet to sign & send.
pub fn jupiter_swap_transaction(
    http: &HttpCtx,
    quote_response: &Value,
    user_public_key: &str,
) -> Result<String, OpalError> {
    let body = json!({
        "quoteResponse": quote_response,
        "userPublicKey": user_public_key,
        "wrapAndUnwrapSol": true,
        "dynamicComputeUnitLimit": true,
    });
    let v = http.post_json("https://quote-api.jup.ag/v6/swap", &body)?;
    v["swapTransaction"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| OpalError::Io(format!("jupiter swap missing tx: {v}")))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedFloatQuoteRequest {
    pub from_ccy: String,
    pub to_ccy: String,
    pub amount: String,
    pub direction: String, // "from" | "to"
}

fn json_num_or_str(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(n) = v.as_f64() {
        return Some(trim_float(n));
    }
    if let Some(n) = v.as_i64() {
        return Some(n.to_string());
    }
    None
}

fn trim_float(n: f64) -> String {
    let s = format!("{n:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s
    }
}

fn parse_qty_prefix(s: &str) -> Option<String> {
    let tok = s.split_whitespace().next()?.trim();
    if tok.is_empty() {
        return None;
    }
    // Validate numeric
    tok.parse::<f64>().ok()?;
    Some(tok.to_string())
}

fn xml_tag<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(block[start..end].trim())
}

fn fetch_fixed_rates_xml(http: &HttpCtx) -> Result<String, OpalError> {
    {
        let guard = FF_RATES_CACHE.lock().unwrap();
        if let Some((at, xml)) = guard.as_ref() {
            if at.elapsed() < Duration::from_secs(45) {
                return Ok(xml.clone());
            }
        }
    }
    let xml = http.get_text("https://ff.io/rates/fixed.xml")?;
    let mut guard = FF_RATES_CACHE.lock().unwrap();
    *guard = Some((Instant::now(), xml.clone()));
    Ok(xml)
}

fn quote_from_xml_rates(
    http: &HttpCtx,
    from_ccy: &str,
    to_ccy: &str,
    amount: &str,
) -> Result<SwapQuote, OpalError> {
    let xml = fetch_fixed_rates_xml(http)?;
    let needle_from = format!("<from>{from_ccy}</from>");
    let needle_to = format!("<to>{to_ccy}</to>");
    let mut found: Option<&str> = None;
    for part in xml.split("<item>").skip(1) {
        let end = part.find("</item>").unwrap_or(part.len());
        let block = &part[..end];
        if block.contains(&needle_from) && block.contains(&needle_to) {
            found = Some(block);
            break;
        }
    }
    let block = found.ok_or_else(|| {
        OpalError::InvalidInput(format!(
            "FixedFloat does not list {from_ccy} → {to_ccy}"
        ))
    })?;

    let rate_in: f64 = xml_tag(block, "in")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let rate_out: f64 = xml_tag(block, "out")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let min_amount = xml_tag(block, "minamount").and_then(parse_qty_prefix);
    let max_amount = xml_tag(block, "maxamount").and_then(parse_qty_prefix);

    let amt = amount.trim().parse::<f64>().unwrap_or(0.0);
    let to_amount = if rate_in > 0.0 && amt > 0.0 {
        trim_float(amt * (rate_out / rate_in))
    } else {
        "0".into()
    };
    let rate = if rate_in > 0.0 {
        trim_float(rate_out / rate_in)
    } else {
        "0".into()
    };

    let mut errors = Vec::new();
    if let Some(ref min) = min_amount {
        if let Ok(m) = min.parse::<f64>() {
            if amt > 0.0 && amt + 1e-12 < m {
                errors.push("LIMIT_MIN".into());
            }
        }
    }
    if let Some(ref max) = max_amount {
        if let Ok(m) = max.parse::<f64>() {
            if amt > m + 1e-12 {
                errors.push("LIMIT_MAX".into());
            }
        }
    }

    Ok(SwapQuote {
        provider: "fixedfloat".into(),
        from_asset: from_ccy.into(),
        to_asset: to_ccy.into(),
        from_amount: amount.trim().into(),
        to_amount,
        rate,
        min_amount,
        max_amount,
        errors,
        raw: json!({
            "source": "xml",
            "from": from_ccy,
            "to": to_ccy,
            "in": rate_in,
            "out": rate_out,
        }),
    })
}

fn ff_sign(secret: &str, body: &str) -> Result<String, OpalError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| OpalError::Io(format!("hmac: {e}")))?;
    mac.update(body.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn fixedfloat_api_post(
    http: &HttpCtx,
    method: &str,
    body: &Value,
    api_key: &str,
    api_secret: &str,
) -> Result<Value, OpalError> {
    let data = serde_json::to_string(body).map_err(|e| OpalError::Io(format!("json: {e}")))?;
    let sign = ff_sign(api_secret, &data)?;
    let url = format!("https://ff.io/api/v2/{method}");
    let v = http.post_raw_json_with_headers(
        &url,
        &data,
        &[("X-API-KEY", api_key), ("X-API-SIGN", &sign)],
    )?;
    let code = v.get("code").and_then(|c| {
        c.as_i64()
            .or_else(|| c.as_str().and_then(|s| s.parse().ok()))
    });
    if code != Some(0) {
        let msg = v
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("FixedFloat API error");
        return Err(OpalError::Io(format!("FixedFloat {method}: {msg}")));
    }
    Ok(v)
}

fn quote_from_api_price(
    http: &HttpCtx,
    req: &FixedFloatQuoteRequest,
    api_key: &str,
    api_secret: &str,
) -> Result<SwapQuote, OpalError> {
    let amount_n: f64 = req
        .amount
        .trim()
        .parse()
        .map_err(|_| OpalError::InvalidInput("bad amount".into()))?;
    let body = json!({
        "type": "fixed",
        "fromCcy": req.from_ccy,
        "toCcy": req.to_ccy,
        "direction": req.direction,
        "amount": amount_n,
    });
    let v = fixedfloat_api_post(http, "price", &body, api_key, api_secret)?;
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let to_amount = data
        .get("to")
        .and_then(|t| t.get("amount"))
        .and_then(json_num_or_str)
        .or_else(|| data.get("toAmount").and_then(json_num_or_str))
        .unwrap_or_else(|| "0".into());
    let from_amount = data
        .get("from")
        .and_then(|t| t.get("amount"))
        .and_then(json_num_or_str)
        .or_else(|| data.get("fromAmount").and_then(json_num_or_str))
        .unwrap_or_else(|| req.amount.clone());
    let rate = data
        .get("rate")
        .and_then(json_num_or_str)
        .unwrap_or_else(|| "-".into());
    let min_amount = data
        .pointer("/from/min")
        .and_then(json_num_or_str)
        .or_else(|| data.get("min").and_then(json_num_or_str));
    let max_amount = data
        .pointer("/from/max")
        .and_then(json_num_or_str)
        .or_else(|| data.get("max").and_then(json_num_or_str));
    let errors = data
        .get("errors")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    Ok(SwapQuote {
        provider: "fixedfloat".into(),
        from_asset: req.from_ccy.clone(),
        to_asset: req.to_ccy.clone(),
        from_amount,
        to_amount,
        rate,
        min_amount,
        max_amount,
        errors,
        raw: v,
    })
}

/// FixedFloat rate quote. Uses signed `/price` when credentials are present;
/// otherwise falls back to the public XML rates export (includes min/max).
pub fn fixedfloat_rate(
    http: &HttpCtx,
    req: &FixedFloatQuoteRequest,
    api_key: Option<&str>,
    api_secret: Option<&str>,
) -> Result<SwapQuote, OpalError> {
    if let (Some(key), Some(secret)) = (api_key, api_secret) {
        if !key.is_empty() && !secret.is_empty() {
            match quote_from_api_price(http, req, key, secret) {
                Ok(q) => return Ok(q),
                Err(_) => {
                    // Fall through to XML so the UI still shows a live rate.
                }
            }
        }
    }
    quote_from_xml_rates(http, &req.from_ccy, &req.to_ccy, &req.amount)
}

pub fn fixedfloat_create_order(
    http: &HttpCtx,
    from_ccy: &str,
    to_ccy: &str,
    amount: &str,
    to_address: &str,
    api_key: &str,
    api_secret: &str,
) -> Result<FixedFloatOrder, OpalError> {
    let amount_n: f64 = amount
        .trim()
        .parse()
        .map_err(|_| OpalError::InvalidInput("bad amount".into()))?;
    let body = json!({
        "type": "fixed",
        "fromCcy": from_ccy,
        "toCcy": to_ccy,
        "direction": "from",
        "amount": amount_n,
        "toAddress": to_address,
    });
    let v = fixedfloat_api_post(http, "create", &body, api_key, api_secret)?;
    let data = v
        .get("data")
        .cloned()
        .ok_or_else(|| OpalError::Io("FixedFloat create: missing data".into()))?;
    let id = data
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let token = data
        .get("token")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() || token.is_empty() {
        return Err(OpalError::Io("FixedFloat create: missing id/token".into()));
    }
    let deposit_address = data
        .pointer("/from/address")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if deposit_address.is_empty() {
        return Err(OpalError::Io(
            "FixedFloat create: missing deposit address".into(),
        ));
    }
    let deposit_tag = data
        .pointer("/from/tag")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let from_amount = data
        .pointer("/from/amount")
        .and_then(json_num_or_str)
        .unwrap_or_else(|| amount.trim().into());
    let to_amount = data
        .pointer("/to/amount")
        .and_then(json_num_or_str)
        .unwrap_or_else(|| "0".into());
    let to_addr = data
        .pointer("/to/address")
        .and_then(|x| x.as_str())
        .unwrap_or(to_address)
        .to_string();
    let status = data
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or("NEW")
        .to_string();
    let order_url = format!("https://ff.io/order/{id}?token={token}");

    Ok(FixedFloatOrder {
        id,
        token,
        status,
        from_amount,
        to_amount,
        deposit_address,
        deposit_tag,
        to_address: to_addr,
        from_ccy: from_ccy.into(),
        to_ccy: to_ccy.into(),
        order_url,
        raw: v,
    })
}

pub fn fixedfloat_order_status(
    http: &HttpCtx,
    id: &str,
    token: &str,
    api_key: &str,
    api_secret: &str,
) -> Result<FixedFloatOrder, OpalError> {
    let body = json!({ "id": id, "token": token });
    let v = fixedfloat_api_post(http, "order", &body, api_key, api_secret)?;
    let data = v
        .get("data")
        .cloned()
        .ok_or_else(|| OpalError::Io("FixedFloat order: missing data".into()))?;
    let deposit_address = data
        .pointer("/from/address")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let deposit_tag = data
        .pointer("/from/tag")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let from_amount = data
        .pointer("/from/amount")
        .and_then(json_num_or_str)
        .unwrap_or_else(|| "0".into());
    let to_amount = data
        .pointer("/to/amount")
        .and_then(json_num_or_str)
        .unwrap_or_else(|| "0".into());
    let to_addr = data
        .pointer("/to/address")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let status = data
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let order_url = format!("https://ff.io/order/{id}?token={token}");
    Ok(FixedFloatOrder {
        id: id.into(),
        token: token.into(),
        status,
        from_amount,
        to_amount,
        deposit_address,
        deposit_tag,
        to_address: to_addr,
        from_ccy: data
            .pointer("/from/code")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .into(),
        to_ccy: data
            .pointer("/to/code")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .into(),
        order_url,
        raw: v,
    })
}

pub fn default_mint_for_symbol(symbol: &str) -> Option<&'static str> {
    match symbol.to_ascii_uppercase().as_str() {
        "SOL" => Some("So11111111111111111111111111111111111111112"),
        "USDC" => Some("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),
        "USDT" => Some("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"),
        _ => None,
    }
}

pub fn suggest_provider(from_chain: ChainId, to_chain: ChainId) -> &'static str {
    if from_chain == ChainId::Sol && to_chain == ChainId::Sol {
        "jupiter"
    } else {
        "fixedfloat"
    }
}
