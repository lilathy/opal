//! Address helpers: BIP21 URIs, poisoning defenses, payment QR payload.

use crate::error::OpalError;
use crate::wallet::ChainId;

/// Build a payment URI when a standard exists.
pub fn payment_uri(chain: ChainId, address: &str, amount: Option<&str>) -> String {
    match chain {
        ChainId::Btc => {
            let mut uri = format!("bitcoin:{address}");
            if let Some(a) = amount.filter(|s| !s.is_empty()) {
                uri.push_str(&format!("?amount={a}"));
            }
            uri
        }
        ChainId::Ltc => {
            let mut uri = format!("litecoin:{address}");
            if let Some(a) = amount.filter(|s| !s.is_empty()) {
                uri.push_str(&format!("?amount={a}"));
            }
            uri
        }
        ChainId::Eth
        | ChainId::Arb
        | ChainId::Base
        | ChainId::Op
        | ChainId::Polygon
        | ChainId::Avax
        | ChainId::Bsc
        | ChainId::Gnosis
        | ChainId::Linea => {
            // EIP-681 style
            let mut uri = format!("ethereum:{address}");
            if let Some(a) = amount.filter(|s| !s.is_empty()) {
                uri.push_str(&format!("?value={a}"));
            }
            uri
        }
        _ => address.to_string(),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AddressSafety {
    /// `true` when the address format is valid for the requested chain.
    pub ok: bool,
    pub warnings: Vec<String>,
    pub display_prefix: String,
    pub display_suffix: String,
    pub display_middle_masked: String,
}

fn chain_label(chain: ChainId) -> &'static str {
    match chain {
        ChainId::Btc => "Bitcoin",
        ChainId::Ltc => "Litecoin",
        ChainId::Doge => "Dogecoin",
        ChainId::Eth => "Ethereum",
        ChainId::Arb => "Arbitrum",
        ChainId::Base => "Base",
        ChainId::Op => "Optimism",
        ChainId::Polygon => "Polygon",
        ChainId::Avax => "Avalanche",
        ChainId::Bsc => "BNB Smart Chain",
        ChainId::Gnosis => "Gnosis",
        ChainId::Linea => "Linea",
        ChainId::Sol => "Solana",
        ChainId::Trx => "Tron",
        ChainId::Xmr => "Monero",
        ChainId::Ton => "TON",
    }
}

fn is_base58(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            matches!(
                c,
                '1'..='9' | 'A'..='H' | 'J'..='N' | 'P'..='Z' | 'a'..='k' | 'm'..='z'
            )
        })
}

fn is_hex_body(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Emphasize prefix/suffix and flag near-matches / homoglyphs.
/// When `chain` is set, `ok` reflects whether the address is valid for that chain.
pub fn analyze_address_safety(
    to: &str,
    recent: &[String],
    contacts: &[String],
    chain: Option<ChainId>,
) -> AddressSafety {
    let to = to.trim();
    let mut warnings = Vec::new();
    let chars: Vec<char> = to.chars().collect();
    let prefix: String = chars.iter().take(6).collect();
    let suffix: String = if chars.len() > 6 {
        chars[chars.len().saturating_sub(6)..].iter().collect()
    } else {
        String::new()
    };
    let middle = if chars.len() > 12 {
        format!("{}…{}", prefix, suffix)
    } else {
        to.to_string()
    };

    let mut ok = true;
    if let Some(chain) = chain {
        if let Err(err) = validate_address_for_chain(chain, to) {
            ok = false;
            let msg = err.to_string();
            // Prefer a clear user-facing line when format is wrong.
            warnings.push(if msg.contains("not a ") || msg.contains("Invalid") {
                format!(
                    "Not a valid {} address — check the destination carefully",
                    chain_label(chain)
                )
            } else {
                msg
            });
        }
    } else if to.is_empty() {
        ok = false;
    }

    // Homoglyph / lookalike characters (advisory — does not flip ok by itself)
    if to.chars().any(|c| {
        matches!(
            c,
            'а' | 'е' | 'о' | 'р' | 'с' | 'х' | 'і' | 'ӏ' | 'ɑ' | 'ɡ' | 'і'
        ) || (!c.is_ascii() && to.starts_with("0x"))
    }) {
        warnings.push("Address contains non-ASCII lookalike characters".into());
    }

    for known in recent.iter().chain(contacts.iter()) {
        if known.eq_ignore_ascii_case(to) {
            continue;
        }
        let k: Vec<char> = known.chars().collect();
        if k.len() < 12 || chars.len() < 12 {
            continue;
        }
        let same_prefix = k.iter().take(6).eq(chars.iter().take(6));
        let same_suffix = k[k.len() - 6..] == chars[chars.len() - 6..];
        if same_prefix && same_suffix && known != to {
            warnings.push(format!(
                "Near-match to a known address (same prefix/suffix): {}…{}",
                &known[..6.min(known.len())],
                &known[known.len().saturating_sub(4)..]
            ));
        } else if same_prefix && !same_suffix {
            warnings.push(
                "Shares prefix with a known address but different ending — check carefully".into(),
            );
        }
    }

    AddressSafety {
        ok,
        warnings,
        display_prefix: prefix,
        display_suffix: suffix,
        display_middle_masked: middle,
    }
}

pub fn validate_address_for_chain(chain: ChainId, address: &str) -> Result<(), OpalError> {
    let a = address.trim();
    if a.is_empty() {
        return Err(OpalError::InvalidInput("address required".into()));
    }
    // Strip common payment-URI wrappers people paste by accident.
    let a = a
        .strip_prefix("bitcoin:")
        .or_else(|| a.strip_prefix("litecoin:"))
        .or_else(|| a.strip_prefix("ethereum:"))
        .or_else(|| a.strip_prefix("solana:"))
        .unwrap_or(a);
    let a = a.split('?').next().unwrap_or(a).trim();
    if a.is_empty() {
        return Err(OpalError::InvalidInput("address required".into()));
    }

    match chain {
        ChainId::Btc => {
            if a.starts_with("bc1") {
                if a.len() < 14 || a.len() > 90 || !a.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Err(OpalError::InvalidInput("not a Bitcoin address".into()));
                }
            } else if a.starts_with('1') || a.starts_with('3') {
                // Legacy / P2SH base58 — typically 26–35 chars
                if a.len() < 26 || a.len() > 35 || !is_base58(a) {
                    return Err(OpalError::InvalidInput("not a Bitcoin address".into()));
                }
            } else {
                return Err(OpalError::InvalidInput("not a Bitcoin address".into()));
            }
        }
        ChainId::Ltc => {
            if a.starts_with("ltc1") {
                if a.len() < 14 || a.len() > 90 {
                    return Err(OpalError::InvalidInput("not a Litecoin address".into()));
                }
            } else if (a.starts_with('L') || a.starts_with('M')) && a.len() >= 26 && a.len() <= 35
            {
                // ok
            } else {
                return Err(OpalError::InvalidInput("not a Litecoin address".into()));
            }
        }
        ChainId::Doge => {
            if !(a.starts_with('D') && a.len() >= 26 && a.len() <= 36) {
                return Err(OpalError::InvalidInput("not a Dogecoin address".into()));
            }
        }
        ChainId::Eth
        | ChainId::Arb
        | ChainId::Base
        | ChainId::Op
        | ChainId::Polygon
        | ChainId::Avax
        | ChainId::Bsc
        | ChainId::Gnosis
        | ChainId::Linea => {
            if !(a.starts_with("0x") && a.len() == 42 && is_hex_body(&a[2..])) {
                return Err(OpalError::InvalidInput("not an EVM address".into()));
            }
        }
        ChainId::Sol => {
            // Reject clearly foreign formats first.
            if a.starts_with("0x")
                || a.starts_with("bc1")
                || a.starts_with("ltc1")
                || a.starts_with("EQ")
                || a.starts_with("UQ")
                || (a.starts_with('T') && a.len() == 34)
                || ((a.starts_with('4') || a.starts_with('8')) && a.len() >= 95)
            {
                return Err(OpalError::InvalidInput("not a Solana address".into()));
            }
            if a.len() < 32 || a.len() > 44 || !is_base58(a) {
                return Err(OpalError::InvalidInput("not a Solana address".into()));
            }
        }
        ChainId::Trx => {
            if !(a.starts_with('T') && a.len() == 34) {
                return Err(OpalError::InvalidInput("not a Tron address".into()));
            }
        }
        ChainId::Xmr => {
            if !(a.starts_with('4') || a.starts_with('8')) || a.len() < 95 {
                return Err(OpalError::InvalidInput("not a Monero address".into()));
            }
        }
        ChainId::Ton => {
            return crate::wallet::ton::validate_ton_address(a);
        }
    }
    Ok(())
}

/// PNG QR is generated on the frontend; backend returns the URI/payload only.
pub fn receive_payload(chain: ChainId, address: &str, amount: Option<&str>) -> String {
    payment_uri(chain, address, amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage_and_cross_chain() {
        assert!(validate_address_for_chain(ChainId::Btc, "abc").is_err());
        assert!(validate_address_for_chain(ChainId::Sol, "abc").is_err());
        assert!(validate_address_for_chain(
            ChainId::Sol,
            "0x742d35Cc6634C0532925a3b844Bc454e4438f44e"
        )
        .is_err());
        assert!(validate_address_for_chain(
            ChainId::Eth,
            "So11111111111111111111111111111111111111112"
        )
        .is_err());
        assert!(validate_address_for_chain(
            ChainId::Eth,
            "0x742d35Cc6634C0532925a3b844Bc454e4438f44e"
        )
        .is_ok());
        assert!(validate_address_for_chain(
            ChainId::Sol,
            "So11111111111111111111111111111111111111112"
        )
        .is_ok());
        assert!(validate_address_for_chain(ChainId::Btc, "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4").is_ok());
    }

    #[test]
    fn analyze_marks_invalid_chain() {
        let s = analyze_address_safety("abc", &[], &[], Some(ChainId::Eth));
        assert!(!s.ok);
        assert!(!s.warnings.is_empty());
    }
}
