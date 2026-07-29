//! Trezor Bridge HTTP session (127.0.0.1:21325) with hand-rolled protobuf framing.
//! Does not depend on trezor-client / bitcoin-version conflicts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rlp::RlpStream;
use serde::Serialize;
use serde_json::Value;

use crate::error::OpalError;

mod usb;
pub mod sign_utxo;
pub mod monero;
pub mod monero_sign;

pub use sign_utxo::{trezor_sign_bitcoin_tx, BitcoinSignInput, BitcoinSignOutput, BitcoinSignRequest};
pub use monero::{
    trezor_monero_credentials, trezor_monero_supported, trezor_monero_sync_key_images,
};
pub use monero_sign::{
    trezor_monero_live_refresh, trezor_sign_monero_transaction, MoneroDestEntry, MoneroFreshKeyImage,
    MoneroRctKey, MoneroRingMember, MoneroSignRequest, MoneroSignedParts, MoneroSourceEntry,
};

/// A wire-level session with a Trezor device, regardless of how it's reached
/// (native USB or Bridge's HTTP relay). Implementors only need to know how to
/// exchange one raw protobuf message; the button/passphrase/PIN state
/// machine in [`call_until`] is shared.
pub(crate) trait Transport {
    fn call_raw(&mut self, msg_type: u16, payload: &[u8]) -> Result<(u16, Vec<u8>), OpalError>;
}

// Current Trezor Suite (the "trezord-node" bridge in @trezor/transport-bridge)
// defaults to 21328; the legacy standalone Go bridge (trezord-go) used 21325.
// Try the modern port first, then fall back to the old one for people still
// running the standalone installer.
const BRIDGE_PORTS: [u16; 2] = [21328, 21325];
// Some setups only bind Bridge's HTTP listener on the IPv6 loopback interface,
// which "localhost" resolves to but a hardcoded 127.0.0.1 does not — try both
// and remember whichever one actually answers.
const BRIDGE_HOSTS: [&str; 2] = ["127.0.0.1", "localhost"];
// Both the legacy and current bridge accept any *.trezor.io origin (checked via
// a suffix match), so this is fine against either implementation.
const BRIDGE_ORIGIN: &str = "https://python.trezor.io";

static ACTIVE_ENDPOINT: Lazy<Mutex<(&'static str, u16)>> =
    Lazy::new(|| Mutex::new((BRIDGE_HOSTS[0], BRIDGE_PORTS[0])));

/// After a full Bridge miss, skip re-probing for a bit so USB ops aren't delayed
/// ~15s on every status/create call (localhost hang + 4 candidates).
static BRIDGE_DOWN_UNTIL: Lazy<Mutex<Option<std::time::Instant>>> =
    Lazy::new(|| Mutex::new(None));

fn bridge_base() -> String {
    let (host, port) = *ACTIVE_ENDPOINT.lock();
    format!("http://{host}:{port}")
}

fn bridge_marked_down() -> bool {
    matches!(
        *BRIDGE_DOWN_UNTIL.lock(),
        Some(until) if std::time::Instant::now() < until
    )
}

fn mark_bridge_down() {
    *BRIDGE_DOWN_UNTIL.lock() = Some(std::time::Instant::now() + Duration::from_secs(45));
}

fn mark_bridge_up() {
    *BRIDGE_DOWN_UNTIL.lock() = None;
}

/// All (host, port) combinations worth probing, active endpoint first.
/// Prefer `127.0.0.1` over `localhost` — on Windows, `localhost`→`::1` often
/// hangs for the full request timeout when nothing is listening on IPv6.
fn bridge_candidates() -> Vec<(&'static str, u16)> {
    let active = *ACTIVE_ENDPOINT.lock();
    let mut out = vec![active];
    for port in BRIDGE_PORTS {
        for host in BRIDGE_HOSTS {
            if (host, port) != active {
                out.push((host, port));
            }
        }
    }
    out
}

// Wire message types (trezor-common messages.proto)
const MSG_INITIALIZE: u16 = 0;
const MSG_FAILURE: u16 = 3;
const MSG_FEATURES: u16 = 17;
const MSG_PIN_MATRIX_REQUEST: u16 = 18;
const MSG_CANCEL: u16 = 20;
const MSG_BUTTON_REQUEST: u16 = 26;
const MSG_BUTTON_ACK: u16 = 27;
const MSG_GET_ADDRESS: u16 = 29;
const MSG_ADDRESS: u16 = 30;
const MSG_PASSPHRASE_REQUEST: u16 = 41;
const MSG_PASSPHRASE_ACK: u16 = 42;
const MSG_ETHEREUM_GET_ADDRESS: u16 = 56;
const MSG_ETHEREUM_ADDRESS: u16 = 57;
const MSG_ETHEREUM_SIGN_TX: u16 = 58;
const MSG_ETHEREUM_TX_REQUEST: u16 = 59;
const MSG_ETHEREUM_TX_ACK: u16 = 60;
const MSG_ETHEREUM_SIGN_TX_EIP1559: u16 = 452;
const MSG_MONERO_GET_ADDRESS: u16 = 540;
const MSG_MONERO_ADDRESS: u16 = 541;
const MSG_MONERO_GET_WATCH_KEY: u16 = 542;
const MSG_MONERO_WATCH_KEY: u16 = 543;
const MSG_SOLANA_GET_ADDRESS: u16 = 902;
const MSG_SOLANA_ADDRESS: u16 = 903;
const MSG_SOLANA_SIGN_TX: u16 = 904;
const MSG_SOLANA_TX_SIGNATURE: u16 = 905;
const MSG_TRON_GET_ADDRESS: u16 = 2200;
const MSG_TRON_ADDRESS: u16 = 2201;
const MSG_TRON_SIGN_TX: u16 = 2202;
const MSG_TRON_SIGNATURE: u16 = 2203;
const MSG_TRON_CONTRACT_REQUEST: u16 = 2204;
const MSG_TRON_TRANSFER_CONTRACT: u16 = 2205;
const MSG_TRON_TRIGGER_SMART_CONTRACT: u16 = 2206;
pub(crate) const MSG_SIGN_TX: u16 = 15;
pub(crate) const MSG_TX_REQUEST: u16 = 21;
pub(crate) const MSG_TX_ACK: u16 = 22;

static SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
static ACTIVE_SESSION_ID: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));
/// Serialize every Bridge/USB session so status polls, auto-discover, and
/// SignTx never steal the device out from under each other.
static TREZOR_IO: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize)]
pub struct TrezorStatus {
    pub available: bool,
    pub bridge_url: String,
    pub message: String,
    pub suite_required: bool,
    pub device_count: u32,
    pub session_active: bool,
    /// User-assigned device name (Features.label), when a device could be queried.
    pub device_label: Option<String>,
    /// Hardware model string (Features.model, e.g. "1", "T", "Safe 3"), when queryable.
    pub device_model: Option<String>,
    /// Internal model code (Features.internal_model, e.g. "T2T1", "T3B1"), when queryable.
    pub device_internal_model: Option<String>,
}

/// EIP-1559 or legacy Ethereum transfer parameters for device signing.
#[derive(Debug, Clone)]
pub struct EthereumTxParams {
    pub path: String,
    pub to: String,
    pub value_wei_hex: String,
    pub nonce_hex: String,
    pub gas_limit_hex: String,
    pub chain_id: u64,
    pub data_hex: Option<String>,
    /// Legacy gas price (wei hex). Mutually exclusive with EIP-1559 fees.
    pub gas_price_hex: Option<String>,
    pub max_fee_per_gas_hex: Option<String>,
    pub max_priority_fee_per_gas_hex: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct EnumeratedDevice {
    path: String,
    #[serde(default)]
    session: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AcquireResponse {
    session: String,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Soft detection: Bridge reachability + enumerated device count.
pub fn probe_trezor() -> TrezorStatus {
    let session_active = SESSION_ACTIVE.load(Ordering::SeqCst);

    // Fast path when Bridge was recently unreachable — don't burn ~15s on
    // localhost/IPv6 hangs every status poll.
    if !bridge_marked_down() {
        if let Ok(client) = http_client_probe() {
            if bridge_version(&client).is_ok() {
                mark_bridge_up();
                let devices = enumerate(&client).unwrap_or_default();
                let mut device_count = devices.len() as u32;
                if device_count == 0 && usb::device_present() {
                    device_count = 1;
                }
                let message = if device_count == 0 {
                    "Online".into()
                } else {
                    "Connected".to_string()
                };
                let (device_label, device_model, device_internal_model) = usb::cached_features();
                return TrezorStatus {
                    available: true,
                    bridge_url: bridge_base(),
                    message,
                    suite_required: device_count == 0,
                    device_count,
                    session_active,
                    device_label,
                    device_model,
                    device_internal_model,
                };
            }
            mark_bridge_down();
        }
    }

    // Bridge down / skipped — report USB presence without opening the device.
    if let Some(status) = usb::probe_status(session_active) {
        return status;
    }

    TrezorStatus {
        available: false,
        bridge_url: bridge_base(),
        message: "Offline".into(),
        suite_required: true,
        device_count: 0,
        session_active,
        device_label: None,
        device_model: None,
        device_internal_model: None,
    }
}

pub fn trezor_get_ethereum_address(path: &str, show_display: bool) -> Result<String, OpalError> {
    let address_n = parse_bip32_path(path)?;
    with_session(|session| {
        let mut body = Vec::new();
        for n in &address_n {
            body.extend(proto_varint_field(1, u64::from(*n)));
        }
        if show_display {
            body.extend(proto_varint_field(2, 1));
        }
        let (msg_type, payload) =
            call_until(session, MSG_ETHEREUM_GET_ADDRESS, &body, &[MSG_ETHEREUM_ADDRESS])?;
        let _ = msg_type;
        let address = proto_get_string(&payload, 2)
            .or_else(|| {
                proto_get_bytes(&payload, 1).map(|b| format!("0x{}", hex::encode(b)))
            })
            .ok_or_else(|| OpalError::InvalidInput("EthereumAddress missing address".into()))?;
        Ok(normalize_eth_address(&address))
    })
}

pub fn trezor_get_bitcoin_address(
    coin_name: &str,
    path: &str,
    script_type: &str,
    show_display: bool,
) -> Result<String, OpalError> {
    let address_n = parse_bip32_path(path)?;
    let script = parse_script_type(script_type)?;
    with_session(|session| {
        let mut body = Vec::new();
        for n in &address_n {
            body.extend(proto_varint_field(1, u64::from(*n)));
        }
        body.extend(proto_string_field(2, coin_name));
        if show_display {
            body.extend(proto_varint_field(3, 1));
        }
        body.extend(proto_varint_field(5, u64::from(script)));
        let (_msg_type, payload) = call_until(session, MSG_GET_ADDRESS, &body, &[MSG_ADDRESS])?;
        let address = proto_get_string(&payload, 1)
            .ok_or_else(|| OpalError::InvalidInput("Address missing address field".into()))?;
        Ok(address)
    })
}

#[derive(Debug, Clone)]
pub struct MoneroWatchCredentials {
    pub watch_key_hex: String,
    pub address: String,
}

pub fn trezor_get_solana_address(path: &str, show_display: bool) -> Result<String, OpalError> {
    let address_n = parse_bip32_path(path)?;
    with_session(|session| {
        let mut body = Vec::new();
        for n in &address_n {
            body.extend(proto_varint_field(1, u64::from(*n)));
        }
        if show_display {
            body.extend(proto_varint_field(2, 1));
        }
        let (_msg_type, payload) =
            call_until(session, MSG_SOLANA_GET_ADDRESS, &body, &[MSG_SOLANA_ADDRESS])?;
        proto_get_string(&payload, 1)
            .ok_or_else(|| OpalError::InvalidInput("SolanaAddress missing address".into()))
    })
}

pub fn trezor_get_tron_address(path: &str, show_display: bool) -> Result<String, OpalError> {
    let address_n = parse_bip32_path(path)?;
    with_session(|session| {
        let mut body = Vec::new();
        for n in &address_n {
            body.extend(proto_varint_field(1, u64::from(*n)));
        }
        if show_display {
            body.extend(proto_varint_field(2, 1));
        }
        let (_msg_type, payload) =
            call_until(session, MSG_TRON_GET_ADDRESS, &body, &[MSG_TRON_ADDRESS])?;
        proto_get_string(&payload, 1)
            .ok_or_else(|| OpalError::InvalidInput("TronAddress missing address".into()))
    })
}

pub fn trezor_get_monero_address(path: &str, show_display: bool) -> Result<String, OpalError> {
    let address_n = parse_bip32_path(path)?;
    with_session(|session| {
        let mut body = Vec::new();
        for n in &address_n {
            body.extend(proto_varint_field(1, u64::from(*n)));
        }
        if show_display {
            body.extend(proto_varint_field(2, 1));
        }
        body.extend(proto_varint_field(3, 0)); // MAINNET
        body.extend(proto_varint_field(4, 0)); // account
        body.extend(proto_varint_field(5, 0)); // minor
        let (_msg_type, payload) =
            call_until(session, MSG_MONERO_GET_ADDRESS, &body, &[MSG_MONERO_ADDRESS])?;
        if let Some(s) = proto_get_string(&payload, 1) {
            if !s.is_empty() {
                return Ok(s);
            }
        }
        let bytes = proto_get_bytes(&payload, 1)
            .ok_or_else(|| OpalError::InvalidInput("MoneroAddress missing address".into()))?;
        String::from_utf8(bytes)
            .map_err(|_| OpalError::InvalidInput("MoneroAddress not UTF-8".into()))
    })
}

pub fn trezor_get_monero_watch_key(path: &str) -> Result<MoneroWatchCredentials, OpalError> {
    let address_n = parse_bip32_path(path)?;
    with_session(|session| {
        let mut body = Vec::new();
        for n in &address_n {
            body.extend(proto_varint_field(1, u64::from(*n)));
        }
        body.extend(proto_varint_field(2, 0)); // MAINNET
        let (_msg_type, payload) =
            call_until(session, MSG_MONERO_GET_WATCH_KEY, &body, &[MSG_MONERO_WATCH_KEY])?;
        let watch = proto_get_bytes(&payload, 1)
            .ok_or_else(|| OpalError::InvalidInput("MoneroWatchKey missing watch_key".into()))?;
        let address = if let Some(s) = proto_get_string(&payload, 2) {
            s
        } else {
            let b = proto_get_bytes(&payload, 2)
                .ok_or_else(|| OpalError::InvalidInput("MoneroWatchKey missing address".into()))?;
            String::from_utf8(b)
                .map_err(|_| OpalError::InvalidInput("MoneroWatchKey address not UTF-8".into()))?
        };
        Ok(MoneroWatchCredentials {
            watch_key_hex: hex::encode(watch),
            address,
        })
    })
}

/// Sign a serialized Solana transaction; returns raw 64-byte signature.
pub fn trezor_sign_solana_tx(path: &str, serialized_tx: &[u8]) -> Result<Vec<u8>, OpalError> {
    let address_n = parse_bip32_path(path)?;
    with_session(|session| {
        let mut body = Vec::new();
        for n in &address_n {
            body.extend(proto_varint_field(1, u64::from(*n)));
        }
        body.extend(proto_bytes_field(2, serialized_tx));
        let (_msg_type, payload) =
            call_until(session, MSG_SOLANA_SIGN_TX, &body, &[MSG_SOLANA_TX_SIGNATURE])?;
        proto_get_bytes(&payload, 1)
            .ok_or_else(|| OpalError::InvalidInput("SolanaTxSignature missing signature".into()))
    })
}

#[derive(Debug, Clone)]
pub struct TronSignParams {
    pub path: String,
    pub ref_block_bytes: Vec<u8>,
    pub ref_block_hash: Vec<u8>,
    pub expiration: u64,
    pub timestamp: u64,
    pub fee_limit: Option<u64>,
    pub data: Option<Vec<u8>>,
    pub transfer: Option<TronTransferParams>,
    pub trigger: Option<TronTriggerParams>,
}

#[derive(Debug, Clone)]
pub struct TronTransferParams {
    pub owner_address: Vec<u8>,
    pub to_address: Vec<u8>,
    pub amount_sun: u64,
}

#[derive(Debug, Clone)]
pub struct TronTriggerParams {
    pub owner_address: Vec<u8>,
    pub contract_address: Vec<u8>,
    pub data: Vec<u8>,
}

pub fn trezor_sign_tron_tx(params: &TronSignParams) -> Result<Vec<u8>, OpalError> {
    let address_n = parse_bip32_path(&params.path)?;
    with_session(|session| {
        let mut body = Vec::new();
        for n in &address_n {
            body.extend(proto_varint_field(1, u64::from(*n)));
        }
        body.extend(proto_bytes_field(2, &params.ref_block_bytes));
        body.extend(proto_bytes_field(3, &params.ref_block_hash));
        body.extend(proto_varint_field(4, params.expiration));
        if let Some(ref d) = params.data {
            body.extend(proto_bytes_field(5, d));
        }
        body.extend(proto_varint_field(6, params.timestamp));
        if let Some(fl) = params.fee_limit {
            body.extend(proto_varint_field(7, fl));
        }

        let (msg_type, _) = call_until(
            session,
            MSG_TRON_SIGN_TX,
            &body,
            &[MSG_TRON_CONTRACT_REQUEST, MSG_TRON_SIGNATURE],
        )?;
        if msg_type == MSG_TRON_SIGNATURE {
            return Err(OpalError::InvalidInput(
                "Trezor returned signature before contract details".into(),
            ));
        }

        let (ctype, cbody) = if let Some(ref t) = params.transfer {
            let mut c = Vec::new();
            c.extend(proto_bytes_field(1, &t.owner_address));
            c.extend(proto_bytes_field(2, &t.to_address));
            c.extend(proto_varint_field(3, t.amount_sun));
            (MSG_TRON_TRANSFER_CONTRACT, c)
        } else if let Some(ref t) = params.trigger {
            let mut c = Vec::new();
            c.extend(proto_bytes_field(1, &t.owner_address));
            c.extend(proto_bytes_field(2, &t.contract_address));
            c.extend(proto_bytes_field(4, &t.data));
            (MSG_TRON_TRIGGER_SMART_CONTRACT, c)
        } else {
            return Err(OpalError::InvalidInput(
                "TronSignTx requires transfer or trigger contract".into(),
            ));
        };

        let (_mt, sig_payload) = call_until(session, ctype, &cbody, &[MSG_TRON_SIGNATURE])?;
        proto_get_bytes(&sig_payload, 1)
            .ok_or_else(|| OpalError::InvalidInput("TronSignature missing signature".into()))
    })
}

/// Sign a basic ETH transfer (legacy or EIP-1559). Returns `0x`-prefixed signed raw tx hex.
pub fn trezor_sign_ethereum_tx(params: &EthereumTxParams) -> Result<String, OpalError> {
    let address_n = parse_bip32_path(&params.path)?;
    let to = normalize_eth_address(&params.to);
    let value = decode_hex_bytes(&params.value_wei_hex)?;
    let nonce = decode_hex_bytes(&params.nonce_hex)?;
    let gas_limit = decode_hex_bytes(&params.gas_limit_hex)?;
    let data = match &params.data_hex {
        Some(h) if !h.is_empty() && h != "0x" => decode_hex_bytes(h)?,
        _ => Vec::new(),
    };

    let eip1559 = params.max_fee_per_gas_hex.is_some()
        && params.max_priority_fee_per_gas_hex.is_some();
    let legacy = params.gas_price_hex.is_some();
    if eip1559 == legacy {
        return Err(OpalError::InvalidInput(
            "provide either gas_price_hex (legacy) or max_fee_per_gas_hex + max_priority_fee_per_gas_hex (EIP-1559)"
                .into(),
        ));
    }

    with_session(|session| {
        let (sig_v, sig_r, sig_s) = if eip1559 {
            let max_fee = decode_hex_bytes(params.max_fee_per_gas_hex.as_ref().unwrap())?;
            let max_prio =
                decode_hex_bytes(params.max_priority_fee_per_gas_hex.as_ref().unwrap())?;
            let mut body = Vec::new();
            for n in &address_n {
                body.extend(proto_varint_field(1, u64::from(*n)));
            }
            body.extend(proto_bytes_field(2, &nonce));
            body.extend(proto_bytes_field(3, &max_fee));
            body.extend(proto_bytes_field(4, &max_prio));
            body.extend(proto_bytes_field(5, &gas_limit));
            body.extend(proto_string_field(6, &to));
            body.extend(proto_bytes_field(7, &value));
            let initial = if data.len() > 1024 { &data[..1024] } else { &data[..] };
            if !initial.is_empty() {
                body.extend(proto_bytes_field(8, initial));
            }
            body.extend(proto_varint_field(9, data.len() as u64));
            body.extend(proto_varint_field(10, params.chain_id));
            sign_eth_exchange(session, MSG_ETHEREUM_SIGN_TX_EIP1559, &body, &data)?
        } else {
            let gas_price = decode_hex_bytes(params.gas_price_hex.as_ref().unwrap())?;
            let mut body = Vec::new();
            for n in &address_n {
                body.extend(proto_varint_field(1, u64::from(*n)));
            }
            body.extend(proto_bytes_field(2, &nonce));
            body.extend(proto_bytes_field(3, &gas_price));
            body.extend(proto_bytes_field(4, &gas_limit));
            body.extend(proto_bytes_field(6, &value));
            let initial = if data.len() > 1024 { &data[..1024] } else { &data[..] };
            if !initial.is_empty() {
                body.extend(proto_bytes_field(7, initial));
            }
            body.extend(proto_varint_field(8, data.len() as u64));
            body.extend(proto_varint_field(9, params.chain_id));
            body.extend(proto_string_field(11, &to));
            sign_eth_exchange(session, MSG_ETHEREUM_SIGN_TX, &body, &data)?
        };

        if eip1559 {
            assemble_eip1559_raw(
                params.chain_id,
                &nonce,
                &decode_hex_bytes(params.max_priority_fee_per_gas_hex.as_ref().unwrap())?,
                &decode_hex_bytes(params.max_fee_per_gas_hex.as_ref().unwrap())?,
                &gas_limit,
                &to,
                &value,
                &data,
                sig_v,
                &sig_r,
                &sig_s,
            )
        } else {
            assemble_legacy_raw(
                &nonce,
                &decode_hex_bytes(params.gas_price_hex.as_ref().unwrap())?,
                &gas_limit,
                &to,
                &value,
                &data,
                sig_v,
                &sig_r,
                &sig_s,
            )
        }
    })
}

// ─── Session ──────────────────────────────────────────────────────────────────

pub(crate) fn with_session<T>(
    f: impl FnOnce(&mut dyn Transport) -> Result<T, OpalError>,
) -> Result<T, OpalError> {
    let _io = TREZOR_IO.try_lock().ok_or_else(|| {
        OpalError::InvalidInput(
            "Trezor is busy with another request. Finish confirming on the device, then retry."
                .into(),
        )
    })?;

    // Prefer Bridge when Suite/trezord is reachable. Skip the multi-second
    // Bridge probe when we recently learned it's down (common: no Suite open).
    if !bridge_marked_down() {
        if let Ok(probe) = http_client_probe() {
            if bridge_version(&probe).is_ok() {
                mark_bridge_up();
                if let Ok(devices) = enumerate(&probe) {
                    if !devices.is_empty() {
                        let client = http_client(Duration::from_secs(300))?;
                        return with_bridge_devices(&client, &devices, f);
                    }
                }
                // Bridge is up but sees no device — don't fight it over WinUSB.
                if usb::device_present() {
                    return Err(OpalError::InvalidInput(
                        "Trezor is plugged in but Suite/Bridge hasn't unlocked it yet. Unlock the device in Trezor Suite, then retry."
                            .into(),
                    ));
                }
                return Err(OpalError::InvalidInput(
                    "No Trezor device found. Connect and unlock the device in Trezor Suite, then retry."
                        .into(),
                ));
            }
            mark_bridge_down();
        }
    }

    // Bridge unavailable — talk to the device over native USB.
    if usb::device_present() {
        let mut session = usb::UsbSession::open()?;
        // Soft init: discovery historically ignored Initialize failures and
        // GetAddress still worked. Hard-failing here blocked every create/send.
        if let Ok((_, payload)) =
            call_until(&mut session, MSG_INITIALIZE, &[], &[MSG_FEATURES])
        {
            usb::cache_features(
                proto_get_string(&payload, 10),
                proto_get_string(&payload, 21),
                proto_get_string(&payload, 44),
            );
        }
        return f(&mut session);
    }

    Err(OpalError::InvalidInput(
        "No Trezor device found. Connect and unlock the device, then retry.".into(),
    ))
}

fn with_bridge_devices<T>(
    client: &reqwest::blocking::Client,
    devices: &[EnumeratedDevice],
    f: impl FnOnce(&mut dyn Transport) -> Result<T, OpalError>,
) -> Result<T, OpalError> {
    // Prefer a free device. Only steal an existing session when every device
    // is already claimed (common: Suite holding an idle session).
    let device = devices
        .iter()
        .find(|d| d.session.is_none())
        .unwrap_or(&devices[0]);
    let previous = device.session.as_deref().unwrap_or("null");
    let mut session = match TrezorSession::acquire(client, &device.path, previous) {
        Ok(s) => s,
        Err(first) if device.session.is_some() => {
            // Steal failed — wait briefly and retry once against a free slot.
            std::thread::sleep(Duration::from_millis(180));
            let refreshed = enumerate(client).unwrap_or_else(|_| devices.to_vec());
            let free = refreshed
                .iter()
                .find(|d| d.session.is_none())
                .unwrap_or(&refreshed[0]);
            let prev = free.session.as_deref().unwrap_or("null");
            TrezorSession::acquire(client, &free.path, prev).map_err(|_| first)?
        }
        Err(e) => return Err(e),
    };
    f(&mut session)
}

struct TrezorSession {
    client: reqwest::blocking::Client,
    session: String,
    released: AtomicBool,
}

impl Drop for TrezorSession {
    fn drop(&mut self) {
        self.release_inner();
    }
}

impl TrezorSession {
    fn acquire(
        client: &reqwest::blocking::Client,
        path: &str,
        previous: &str,
    ) -> Result<Self, OpalError> {
        let encoded = percent_encode_path(path);
        let url = format!("{}/acquire/{encoded}/{previous}", bridge_base());
        let res = client
            .post(&url)
            .header("Origin", BRIDGE_ORIGIN)
            .send()
            .map_err(|e| OpalError::Io(format!("bridge acquire: {e}")))?;
        if !res.status().is_success() {
            let body = res.text().unwrap_or_default();
            return Err(OpalError::Io(format!("bridge acquire failed: {body}")));
        }
        let parsed: AcquireResponse = res
            .json()
            .map_err(|e| OpalError::Io(format!("bridge acquire json: {e}")))?;
        SESSION_ACTIVE.store(true, Ordering::SeqCst);
        *ACTIVE_SESSION_ID.lock() = Some(parsed.session.clone());

        let mut session = Self {
            client: client.clone(),
            session: parsed.session,
            released: AtomicBool::new(false),
        };
        // Soft init — Features response clears session state on device.
        match call_until(&mut session, MSG_INITIALIZE, &[], &[MSG_FEATURES]) {
            Ok((_, payload)) => {
                usb::cache_features(
                    proto_get_string(&payload, 10),
                    proto_get_string(&payload, 21),
                    proto_get_string(&payload, 44),
                );
            }
            Err(_) => {}
        }
        Ok(session)
    }

    fn release_inner(&self) {
        if self
            .released
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let url = format!("{}/release/{}", bridge_base(), self.session);
        let _ = self
            .client
            .post(&url)
            .header("Origin", BRIDGE_ORIGIN)
            .send();
        SESSION_ACTIVE.store(false, Ordering::SeqCst);
        let mut guard = ACTIVE_SESSION_ID.lock();
        if guard.as_deref() == Some(self.session.as_str()) {
            *guard = None;
        }
    }

}

impl Transport for TrezorSession {
    fn call_raw(&mut self, msg_type: u16, payload: &[u8]) -> Result<(u16, Vec<u8>), OpalError> {
        let frame = encode_bridge_frame(msg_type, payload);
        let url = format!("{}/call/{}", bridge_base(), self.session);
        let res = self
            .client
            .post(&url)
            .header("Origin", BRIDGE_ORIGIN)
            .header("Content-Type", "text/plain")
            .body(frame)
            .send()
            .map_err(|e| OpalError::Io(format!("bridge call: {e}")))?;
        if !res.status().is_success() {
            let body = res.text().unwrap_or_default();
            return Err(OpalError::Io(format!("bridge call failed: {body}")));
        }
        let hex_body = res
            .text()
            .map_err(|e| OpalError::Io(format!("bridge call body: {e}")))?;
        decode_bridge_frame(hex_body.trim())
    }
}

/// Send a request and follow ButtonRequest / PassphraseRequest until an
/// expected type or error. Shared by every [`Transport`] implementation.
pub(crate) fn call_until(
    session: &mut (impl Transport + ?Sized),
    msg_type: u16,
    payload: &[u8],
    expected: &[u16],
) -> Result<(u16, Vec<u8>), OpalError> {
    let mut resp = session.call_raw(msg_type, payload)?;
    for _ in 0..64 {
        let (ty, data) = resp;
        if expected.contains(&ty) {
            return Ok((ty, data));
        }
        match ty {
            MSG_BUTTON_REQUEST => {
                resp = session.call_raw(MSG_BUTTON_ACK, &[])?;
            }
            MSG_PASSPHRASE_REQUEST => {
                // Ask device to collect passphrase on-device.
                let ack = proto_varint_field(3, 1); // on_device = true
                resp = session.call_raw(MSG_PASSPHRASE_ACK, &ack)?;
            }
            MSG_PIN_MATRIX_REQUEST => {
                let _ = session.call_raw(MSG_CANCEL, &[]);
                return Err(OpalError::InvalidInput(
                    "Trezor is locked with a PIN. Unlock the device in Trezor Suite first, then retry."
                        .into(),
                ));
            }
            MSG_FAILURE => {
                let msg = proto_get_string(&data, 2).unwrap_or_else(|| "device failure".into());
                return Err(OpalError::InvalidInput(format!("Trezor Failure: {msg}")));
            }
            other => {
                return Err(OpalError::InvalidInput(format!(
                    "Unexpected Trezor message type {other}"
                )));
            }
        }
    }
    Err(OpalError::InvalidInput(
        "Trezor interaction loop exceeded max rounds".into(),
    ))
}

fn sign_eth_exchange(
    session: &mut (impl Transport + ?Sized),
    msg_type: u16,
    body: &[u8],
    full_data: &[u8],
) -> Result<(u32, Vec<u8>, Vec<u8>), OpalError> {
    let mut resp = call_until(session, msg_type, body, &[MSG_ETHEREUM_TX_REQUEST])?;
    let mut offset = if full_data.len() > 1024 { 1024 } else { full_data.len() };

    loop {
        let (_ty, payload) = resp;
        if let Some(need) = proto_get_varint(&payload, 1) {
            let need = need as usize;
            let end = (offset + need).min(full_data.len());
            let chunk = &full_data[offset..end];
            offset = end;
            let ack = proto_bytes_field(1, chunk);
            resp = call_until(session, MSG_ETHEREUM_TX_ACK, &ack, &[MSG_ETHEREUM_TX_REQUEST])?;
            continue;
        }
        let v = proto_get_varint(&payload, 2)
            .ok_or_else(|| OpalError::InvalidInput("EthereumTxRequest missing signature_v".into()))?
            as u32;
        let r = proto_get_bytes(&payload, 3)
            .ok_or_else(|| OpalError::InvalidInput("EthereumTxRequest missing signature_r".into()))?;
        let s = proto_get_bytes(&payload, 4)
            .ok_or_else(|| OpalError::InvalidInput("EthereumTxRequest missing signature_s".into()))?;
        return Ok((v, r, s));
    }
}

// ─── Bridge HTTP helpers ──────────────────────────────────────────────────────

fn http_client(timeout: Duration) -> Result<reqwest::blocking::Client, OpalError> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_millis(800))
        // Bridge only ever lives on loopback — never route it through a system/VPN
        // proxy, which would otherwise silently break detection on machines that
        // have HTTP_PROXY/HTTPS_PROXY set.
        .no_proxy()
        .build()
        .map_err(|e| OpalError::Io(format!("http client: {e}")))
}

/// Short-timeout client for "is Bridge up?" probes only.
fn http_client_probe() -> Result<reqwest::blocking::Client, OpalError> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(900))
        .connect_timeout(Duration::from_millis(350))
        .no_proxy()
        .build()
        .map_err(|e| OpalError::Io(format!("http client: {e}")))
}

fn bridge_version(client: &reqwest::blocking::Client) -> Result<Value, OpalError> {
    // Try the endpoint we last had success with first, then fall back through
    // every (host, port) combination — this recovers from both the IPv4/IPv6
    // loopback mismatch and the 21328-vs-21325 port change without the
    // caller needing to care which one is actually running.
    let mut last_err: Option<OpalError> = None;
    for (host, port) in bridge_candidates() {
        match bridge_version_at(client, host, port) {
            Ok(v) => {
                *ACTIVE_ENDPOINT.lock() = (host, port);
                return Ok(v);
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| OpalError::Io("bridge not reachable".into())))
}

fn bridge_version_at(
    client: &reqwest::blocking::Client,
    host: &str,
    port: u16,
) -> Result<Value, OpalError> {
    let base = format!("http://{host}:{port}");
    // Official API: POST /
    let post = client
        .post(format!("{base}/"))
        .header("Origin", BRIDGE_ORIGIN)
        .send();
    if let Ok(res) = post {
        if res.status().is_success() {
            if let Ok(v) = res.json::<Value>() {
                return Ok(v);
            }
        }
    }
    // Fallback used by some Suite builds
    let get = client
        .get(format!("{base}/"))
        .header("Origin", BRIDGE_ORIGIN)
        .send()
        .map_err(|e| OpalError::Io(format!("bridge status: {e}")))?;
    if get.status().is_success() {
        return get
            .json()
            .map_err(|e| OpalError::Io(format!("bridge status json: {e}")));
    }
    Err(OpalError::Io("bridge not reachable".into()))
}

fn enumerate(client: &reqwest::blocking::Client) -> Result<Vec<EnumeratedDevice>, OpalError> {
    let res = client
        .post(format!("{}/enumerate", bridge_base()))
        .header("Origin", BRIDGE_ORIGIN)
        .send()
        .map_err(|e| OpalError::Io(format!("bridge enumerate: {e}")))?;
    if !res.status().is_success() {
        let body = res.text().unwrap_or_default();
        return Err(OpalError::Io(format!("bridge enumerate failed: {body}")));
    }
    res.json()
        .map_err(|e| OpalError::Io(format!("bridge enumerate json: {e}")))
}

fn encode_bridge_frame(msg_type: u16, payload: &[u8]) -> String {
    let mut buf = Vec::with_capacity(6 + payload.len());
    buf.extend_from_slice(&msg_type.to_be_bytes());
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(payload);
    hex::encode(buf)
}

fn decode_bridge_frame(hex_body: &str) -> Result<(u16, Vec<u8>), OpalError> {
    let raw = hex::decode(hex_body.trim())
        .map_err(|e| OpalError::Io(format!("bridge response hex: {e}")))?;
    if raw.len() < 6 {
        return Err(OpalError::Io("bridge response too short".into()));
    }
    let msg_type = u16::from_be_bytes([raw[0], raw[1]]);
    let len = u32::from_be_bytes([raw[2], raw[3], raw[4], raw[5]]) as usize;
    if raw.len() < 6 + len {
        return Err(OpalError::Io("bridge response truncated".into()));
    }
    Ok((msg_type, raw[6..6 + len].to_vec()))
}

fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ─── Minimal protobuf ─────────────────────────────────────────────────────────

fn encode_varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
    out
}

fn proto_key(field: u32, wire: u8) -> Vec<u8> {
    encode_varint(u64::from((field << 3) | u32::from(wire)))
}

pub(crate) fn proto_varint_field(field: u32, value: u64) -> Vec<u8> {
    let mut out = proto_key(field, 0);
    out.extend(encode_varint(value));
    out
}

pub(crate) fn proto_bytes_field(field: u32, data: &[u8]) -> Vec<u8> {
    let mut out = proto_key(field, 2);
    out.extend(encode_varint(data.len() as u64));
    out.extend_from_slice(data);
    out
}

pub(crate) fn proto_string_field(field: u32, s: &str) -> Vec<u8> {
    proto_bytes_field(field, s.as_bytes())
}

fn decode_varint(data: &[u8], mut i: usize) -> Result<(u64, usize), OpalError> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        if i >= data.len() {
            return Err(OpalError::Io("protobuf varint truncated".into()));
        }
        let b = data[i];
        i += 1;
        result |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok((result, i));
        }
        shift += 7;
        if shift > 63 {
            return Err(OpalError::Io("protobuf varint overflow".into()));
        }
    }
}

#[derive(Debug)]
enum ProtoValue {
    Varint(u64),
    Bytes(Vec<u8>),
}

fn proto_parse(data: &[u8]) -> Result<Vec<(u32, ProtoValue)>, OpalError> {
    let mut fields = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let (key, ni) = decode_varint(data, i)?;
        i = ni;
        let field = (key >> 3) as u32;
        let wire = (key & 7) as u8;
        match wire {
            0 => {
                let (v, ni) = decode_varint(data, i)?;
                i = ni;
                fields.push((field, ProtoValue::Varint(v)));
            }
            2 => {
                let (len, ni) = decode_varint(data, i)?;
                i = ni;
                let end = i + len as usize;
                if end > data.len() {
                    return Err(OpalError::Io("protobuf bytes truncated".into()));
                }
                fields.push((field, ProtoValue::Bytes(data[i..end].to_vec())));
                i = end;
            }
            5 => {
                // 32-bit — skip
                if i + 4 > data.len() {
                    return Err(OpalError::Io("protobuf fixed32 truncated".into()));
                }
                i += 4;
            }
            1 => {
                if i + 8 > data.len() {
                    return Err(OpalError::Io("protobuf fixed64 truncated".into()));
                }
                i += 8;
            }
            _ => {
                return Err(OpalError::Io(format!("unsupported protobuf wire type {wire}")));
            }
        }
    }
    Ok(fields)
}

fn proto_get_string(data: &[u8], field: u32) -> Option<String> {
    let fields = proto_parse(data).ok()?;
    for (f, v) in fields {
        if f == field {
            if let ProtoValue::Bytes(b) = v {
                return String::from_utf8(b).ok();
            }
        }
    }
    None
}

pub(crate) fn proto_get_bytes(data: &[u8], field: u32) -> Option<Vec<u8>> {
    let fields = proto_parse(data).ok()?;
    for (f, v) in fields {
        if f == field {
            if let ProtoValue::Bytes(b) = v {
                return Some(b);
            }
        }
    }
    None
}

/// All bytes values for a repeated protobuf field (order preserved).
pub(crate) fn proto_get_bytes_all(data: &[u8], field: u32) -> Vec<Vec<u8>> {
    let Ok(fields) = proto_parse(data) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (f, v) in fields {
        if f == field {
            if let ProtoValue::Bytes(b) = v {
                out.push(b);
            }
        }
    }
    out
}

pub(crate) fn proto_get_varint(data: &[u8], field: u32) -> Option<u64> {
    let fields = proto_parse(data).ok()?;
    for (f, v) in fields {
        if f == field {
            if let ProtoValue::Varint(n) = v {
                return Some(n);
            }
        }
    }
    None
}

// ─── Path / hex / script helpers ──────────────────────────────────────────────

pub(crate) fn parse_bip32_path(path: &str) -> Result<Vec<u32>, OpalError> {
    let path = path.trim();
    let path = path.strip_prefix('m').unwrap_or(path);
    let path = path.strip_prefix('/').unwrap_or(path);
    if path.is_empty() {
        return Err(OpalError::InvalidInput("empty derivation path".into()));
    }
    let mut out = Vec::new();
    for part in path.split('/') {
        if part.is_empty() {
            continue;
        }
        let hardened = part.ends_with('\'') || part.ends_with('h') || part.ends_with('H');
        let num_str = part.trim_end_matches(['\'', 'h', 'H']);
        let n: u32 = num_str
            .parse()
            .map_err(|_| OpalError::InvalidInput(format!("bad path segment: {part}")))?;
        if hardened {
            out.push(n | 0x8000_0000);
        } else {
            out.push(n);
        }
    }
    if out.is_empty() {
        return Err(OpalError::InvalidInput("empty derivation path".into()));
    }
    Ok(out)
}

fn parse_script_type(s: &str) -> Result<u32, OpalError> {
    match s.trim().to_ascii_uppercase().as_str() {
        "SPENDADDRESS" | "P2PKH" | "0" => Ok(0),
        "SPENDMULTISIG" | "1" => Ok(1),
        "SPENDWITNESS" | "P2WPKH" | "3" => Ok(3),
        "SPENDP2SHWITNESS" | "P2SH-P2WPKH" | "4" => Ok(4),
        "SPENDTAPROOT" | "P2TR" | "5" => Ok(5),
        other => Err(OpalError::InvalidInput(format!(
            "unknown script_type '{other}' (use SPENDADDRESS|SPENDWITNESS|SPENDP2SHWITNESS|SPENDTAPROOT)"
        ))),
    }
}

fn decode_hex_bytes(s: &str) -> Result<Vec<u8>, OpalError> {
    let h = s.trim().trim_start_matches("0x");
    if h.is_empty() {
        return Ok(Vec::new());
    }
    if h.len() % 2 != 0 {
        return Err(OpalError::InvalidInput(format!("odd-length hex: {s}")));
    }
    hex::decode(h).map_err(|e| OpalError::InvalidInput(format!("hex: {e}")))
}

fn normalize_eth_address(addr: &str) -> String {
    let a = addr.trim();
    if a.starts_with("0x") || a.starts_with("0X") {
        format!("0x{}", &a[2..])
    } else {
        format!("0x{a}")
    }
}

fn strip_leading_zeros(b: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < b.len() && b[i] == 0 {
        i += 1;
    }
    &b[i..]
}

fn pad32(b: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let src = if b.len() > 32 { &b[b.len() - 32..] } else { b };
    out[32 - src.len()..].copy_from_slice(src);
    out
}

fn decode_addr20(addr: &str) -> Result<Vec<u8>, OpalError> {
    let b = decode_hex_bytes(addr)?;
    if b.len() != 20 {
        return Err(OpalError::InvalidInput("to address must be 20 bytes".into()));
    }
    Ok(b)
}

fn assemble_eip1559_raw(
    chain_id: u64,
    nonce: &[u8],
    max_priority: &[u8],
    max_fee: &[u8],
    gas_limit: &[u8],
    to: &str,
    value: &[u8],
    data: &[u8],
    sig_v: u32,
    sig_r: &[u8],
    sig_s: &[u8],
) -> Result<String, OpalError> {
    let to_bytes = decode_addr20(to)?;
    let y_parity = if sig_v >= 27 { sig_v - 27 } else { sig_v } as u64;
    let r = pad32(sig_r);
    let s = pad32(sig_s);

    let mut stream = RlpStream::new_list(12);
    stream.append(&chain_id);
    append_rlp_int(&mut stream, nonce);
    append_rlp_int(&mut stream, max_priority);
    append_rlp_int(&mut stream, max_fee);
    append_rlp_int(&mut stream, gas_limit);
    stream.append(&to_bytes.as_slice());
    append_rlp_int(&mut stream, value);
    stream.append(&data);
    stream.append_list::<u8, u8>(&[]); // empty access list
    stream.append(&y_parity);
    stream.append(&strip_leading_zeros(&r));
    stream.append(&strip_leading_zeros(&s));

    let mut raw = Vec::new();
    raw.push(0x02);
    raw.extend_from_slice(&stream.out());
    Ok(format!("0x{}", hex::encode(raw)))
}

fn assemble_legacy_raw(
    nonce: &[u8],
    gas_price: &[u8],
    gas_limit: &[u8],
    to: &str,
    value: &[u8],
    data: &[u8],
    sig_v: u32,
    sig_r: &[u8],
    sig_s: &[u8],
) -> Result<String, OpalError> {
    let to_bytes = decode_addr20(to)?;
    let r = pad32(sig_r);
    let s = pad32(sig_s);

    let mut stream = RlpStream::new_list(9);
    append_rlp_int(&mut stream, nonce);
    append_rlp_int(&mut stream, gas_price);
    append_rlp_int(&mut stream, gas_limit);
    stream.append(&to_bytes.as_slice());
    append_rlp_int(&mut stream, value);
    stream.append(&data);
    stream.append(&(sig_v as u64));
    stream.append(&strip_leading_zeros(&r));
    stream.append(&strip_leading_zeros(&s));
    Ok(format!("0x{}", hex::encode(stream.out())))
}

fn append_rlp_int(stream: &mut RlpStream, bytes: &[u8]) {
    let stripped = strip_leading_zeros(bytes);
    if stripped.is_empty() {
        stream.append(&0u64);
    } else if stripped.len() <= 8 {
        let mut buf = [0u8; 8];
        buf[8 - stripped.len()..].copy_from_slice(stripped);
        let n = u64::from_be_bytes(buf);
        stream.append(&n);
    } else {
        stream.append(&stripped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hardened() {
        let p = parse_bip32_path("m/44'/60'/0'/0/0").unwrap();
        assert_eq!(p, vec![0x8000002c, 0x8000003c, 0x80000000, 0, 0]);
    }

    #[test]
    fn frame_roundtrip() {
        let payload = proto_string_field(2, "0xabc");
        let hex = encode_bridge_frame(MSG_ETHEREUM_ADDRESS, &payload);
        let (ty, data) = decode_bridge_frame(&hex).unwrap();
        assert_eq!(ty, MSG_ETHEREUM_ADDRESS);
        assert_eq!(proto_get_string(&data, 2).unwrap(), "0xabc");
    }

    #[test]
    fn script_type_parse() {
        assert_eq!(parse_script_type("SPENDWITNESS").unwrap(), 3);
        assert_eq!(parse_script_type("p2tr").unwrap(), 5);
    }

    /// Needs a real device plugged in — not run by default.
    /// `cargo test --lib trezor::tests::probe_real_device -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_real_device() {
        let status = probe_trezor();
        println!("{status:#?}");
        assert!(status.available, "expected a Trezor to be detected");
    }

    #[test]
    #[ignore]
    fn usb_raw_initialize() {
        let mut session = usb::UsbSession::open().expect("open usb session");
        let (ty, payload) = session
            .call_raw(MSG_INITIALIZE, &[])
            .expect("call_raw failed");
        println!("resp type = {ty} (expected MSG_FEATURES = {MSG_FEATURES})");
        println!("payload ({} bytes) = {}", payload.len(), hex::encode(&payload));
        let fields = proto_parse(&payload).unwrap();
        for (f, v) in &fields {
            println!("field {f}: {v:?}");
        }
    }
}
