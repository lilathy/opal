//! Monero ↔ Trezor wire helpers: SignTx (501-518) + LiveRefresh (552-557).
//! Spend key never leaves the device; host only supplies construction data.

use crate::error::OpalError;
use crate::trezor::{
    call_until, parse_bip32_path, proto_bytes_field, proto_get_bytes, proto_get_bytes_all,
    proto_get_varint, proto_varint_field, with_session, Transport,
};

// SignTx
pub(crate) const MSG_MONERO_TX_INIT_REQ: u16 = 501;
pub(crate) const MSG_MONERO_TX_INIT_ACK: u16 = 502;
pub(crate) const MSG_MONERO_TX_SET_INPUT_REQ: u16 = 503;
pub(crate) const MSG_MONERO_TX_SET_INPUT_ACK: u16 = 504;
pub(crate) const MSG_MONERO_TX_INPUT_VINI_REQ: u16 = 507;
pub(crate) const MSG_MONERO_TX_INPUT_VINI_ACK: u16 = 508;
pub(crate) const MSG_MONERO_TX_ALL_INPUTS_SET_REQ: u16 = 509;
pub(crate) const MSG_MONERO_TX_ALL_INPUTS_SET_ACK: u16 = 510;
pub(crate) const MSG_MONERO_TX_SET_OUTPUT_REQ: u16 = 511;
pub(crate) const MSG_MONERO_TX_SET_OUTPUT_ACK: u16 = 512;
pub(crate) const MSG_MONERO_TX_ALL_OUT_SET_REQ: u16 = 513;
pub(crate) const MSG_MONERO_TX_ALL_OUT_SET_ACK: u16 = 514;
pub(crate) const MSG_MONERO_TX_SIGN_INPUT_REQ: u16 = 515;
pub(crate) const MSG_MONERO_TX_SIGN_INPUT_ACK: u16 = 516;
pub(crate) const MSG_MONERO_TX_FINAL_REQ: u16 = 517;
pub(crate) const MSG_MONERO_TX_FINAL_ACK: u16 = 518;

// Live refresh (per-output key images)
pub(crate) const MSG_MONERO_LIVE_REFRESH_START_REQ: u16 = 552;
pub(crate) const MSG_MONERO_LIVE_REFRESH_START_ACK: u16 = 553;
pub(crate) const MSG_MONERO_LIVE_REFRESH_STEP_REQ: u16 = 554;
pub(crate) const MSG_MONERO_LIVE_REFRESH_STEP_ACK: u16 = 555;
pub(crate) const MSG_MONERO_LIVE_REFRESH_FINAL_REQ: u16 = 556;
pub(crate) const MSG_MONERO_LIVE_REFRESH_FINAL_ACK: u16 = 557;

const NETWORK_MAINNET: u64 = 0;
/// Client version 3+ → FinalAck includes opening_key for CLSAG decrypt.
const CLIENT_VERSION: u64 = 3;
/// Hard fork supporting CLSAG / Bulletproof+
const HARD_FORK: u64 = 15;
const BP_VERSION: u64 = 4; // Bulletproof+
const RSIG_TYPE_BP_PLUS: u64 = 3;

#[derive(Debug, Clone)]
pub struct MoneroRctKey {
    pub dest: Vec<u8>,
    pub commitment: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct MoneroRingMember {
    pub global_index: u64,
    pub key: MoneroRctKey,
}

#[derive(Debug, Clone)]
pub struct MoneroSourceEntry {
    pub outputs: Vec<MoneroRingMember>,
    pub real_output: u64,
    pub real_out_tx_key: Vec<u8>,
    pub real_out_additional_tx_keys: Vec<Vec<u8>>,
    pub real_output_in_tx_index: u64,
    pub amount: u64,
    pub mask: Vec<u8>,
    pub subaddr_minor: u32,
}

#[derive(Debug, Clone)]
pub struct MoneroDestEntry {
    pub amount: u64,
    pub spend_public_key: Vec<u8>,
    pub view_public_key: Vec<u8>,
    pub is_subaddress: bool,
}

#[derive(Debug, Clone)]
pub struct MoneroSignRequest {
    pub path: String,
    pub account: u32,
    pub sources: Vec<MoneroSourceEntry>,
    pub destinations: Vec<MoneroDestEntry>,
    pub change: MoneroDestEntry,
    pub fee: u64,
    pub mixin: u32,
    pub unlock_time: u64,
}

#[derive(Debug, Clone)]
pub struct MoneroSignedParts {
    pub extra: Vec<u8>,
    pub tx_prefix_hash: Vec<u8>,
    pub fee: u64,
    pub rv_type: u32,
    pub vinis: Vec<Vec<u8>>,
    pub tx_outs: Vec<Vec<u8>>,
    pub out_pks: Vec<Vec<u8>>,
    pub ecdh_infos: Vec<Vec<u8>>,
    pub signatures: Vec<Vec<u8>>,
    pub pseudo_outs: Vec<Vec<u8>>,
    pub opening_key: Vec<u8>,
    pub tx_enc_keys: Vec<u8>,
    pub range_proof: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct MoneroFreshKeyImage {
    pub salt: Vec<u8>,
    pub key_image_blob: Vec<u8>,
}

fn encode_rct_key(k: &MoneroRctKey) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(proto_bytes_field(1, &k.dest));
    b.extend(proto_bytes_field(2, &k.commitment));
    b
}

fn encode_ring_member(m: &MoneroRingMember) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(proto_varint_field(1, m.global_index));
    b.extend(proto_bytes_field(2, &encode_rct_key(&m.key)));
    b
}

fn encode_source(src: &MoneroSourceEntry) -> Vec<u8> {
    let mut b = Vec::new();
    for o in &src.outputs {
        b.extend(proto_bytes_field(1, &encode_ring_member(o)));
    }
    b.extend(proto_varint_field(2, src.real_output));
    b.extend(proto_bytes_field(3, &src.real_out_tx_key));
    for k in &src.real_out_additional_tx_keys {
        b.extend(proto_bytes_field(4, k));
    }
    b.extend(proto_varint_field(5, src.real_output_in_tx_index));
    b.extend(proto_varint_field(6, src.amount));
    b.extend(proto_varint_field(7, 1)); // rct = true
    if !src.mask.is_empty() {
        b.extend(proto_bytes_field(8, &src.mask));
    }
    b.extend(proto_varint_field(10, u64::from(src.subaddr_minor)));
    b
}

fn encode_dest(d: &MoneroDestEntry) -> Vec<u8> {
    let mut addr = Vec::new();
    addr.extend(proto_bytes_field(1, &d.spend_public_key));
    addr.extend(proto_bytes_field(2, &d.view_public_key));
    let mut b = Vec::new();
    b.extend(proto_varint_field(1, d.amount));
    b.extend(proto_bytes_field(2, &addr));
    if d.is_subaddress {
        b.extend(proto_varint_field(3, 1));
    }
    b
}

fn encode_rsig_data(num_outputs: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(proto_varint_field(1, RSIG_TYPE_BP_PLUS));
    b.extend(proto_varint_field(2, 0)); // on-device BP for ≤2 outs
    // grouping: one batch covering all outputs
    b.extend(proto_varint_field(3, u64::from(num_outputs.max(1))));
    b.extend(proto_varint_field(7, BP_VERSION));
    b
}

fn encode_tsx_data(req: &MoneroSignRequest) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(proto_varint_field(1, 1)); // version
    b.extend(proto_varint_field(3, req.unlock_time));
    for d in &req.destinations {
        b.extend(proto_bytes_field(4, &encode_dest(d)));
    }
    b.extend(proto_bytes_field(5, &encode_dest(&req.change)));
    b.extend(proto_varint_field(6, req.sources.len() as u64));
    b.extend(proto_varint_field(7, u64::from(req.mixin)));
    b.extend(proto_varint_field(8, req.fee));
    b.extend(proto_varint_field(9, u64::from(req.account)));
    let n_out = (req.destinations.len() + 1) as u32; // dests + change
    b.extend(proto_bytes_field(11, &encode_rsig_data(n_out)));
    b.extend(proto_varint_field(13, CLIENT_VERSION));
    b.extend(proto_varint_field(14, HARD_FORK));
    b
}

fn encode_init(req: &MoneroSignRequest, address_n: &[u32]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(proto_varint_field(1, 0));
    for n in address_n {
        b.extend(proto_varint_field(2, u64::from(*n)));
    }
    b.extend(proto_varint_field(3, NETWORK_MAINNET));
    b.extend(proto_bytes_field(4, &encode_tsx_data(req)));
    b
}

/// Run LiveRefresh for a batch of received outputs so watch-only balances
/// drop spent outs correctly. Returns encrypted KI blobs (salt + ciphertext)
/// for `import_key_images` after host-side decrypt with the view key is done
/// by wallet-rpc when we pass pre-formed key image hex - see `xmr_send`.
pub fn trezor_monero_live_refresh(
    path: &str,
    transfers: &[(Vec<u8>, Vec<u8>, u64, u32, u32)],
) -> Result<Vec<MoneroFreshKeyImage>, OpalError> {
    // (out_key, recv_deriv, real_out_idx, sub_major, sub_minor)
    if transfers.is_empty() {
        return Ok(Vec::new());
    }
    let address_n = parse_bip32_path(path)?;
    with_session(|session| {
        let mut start = Vec::new();
        for n in &address_n {
            start.extend(proto_varint_field(1, u64::from(*n)));
        }
        start.extend(proto_varint_field(2, NETWORK_MAINNET));
        call_until(
            session,
            MSG_MONERO_LIVE_REFRESH_START_REQ,
            &start,
            &[MSG_MONERO_LIVE_REFRESH_START_ACK],
        )?;

        let mut out = Vec::with_capacity(transfers.len());
        for (out_key, recv_deriv, real_out_idx, major, minor) in transfers {
            let mut step = Vec::new();
            step.extend(proto_bytes_field(1, out_key));
            step.extend(proto_bytes_field(2, recv_deriv));
            step.extend(proto_varint_field(3, *real_out_idx));
            step.extend(proto_varint_field(4, u64::from(*major)));
            step.extend(proto_varint_field(5, u64::from(*minor)));
            let (_, ack) = call_until(
                session,
                MSG_MONERO_LIVE_REFRESH_STEP_REQ,
                &step,
                &[MSG_MONERO_LIVE_REFRESH_STEP_ACK],
            )?;
            out.push(MoneroFreshKeyImage {
                salt: proto_get_bytes(&ack, 1).unwrap_or_default(),
                key_image_blob: proto_get_bytes(&ack, 2).unwrap_or_default(),
            });
        }

        call_until(
            session,
            MSG_MONERO_LIVE_REFRESH_FINAL_REQ,
            &[],
            &[MSG_MONERO_LIVE_REFRESH_FINAL_ACK],
        )?;
        Ok(out)
    })
}

struct SealedInput {
    src: MoneroSourceEntry,
    vini: Vec<u8>,
    vini_hmac: Vec<u8>,
    pseudo_out: Vec<u8>,
    pseudo_out_hmac: Vec<u8>,
    pseudo_out_alpha: Vec<u8>,
    spend_key: Vec<u8>,
    orig_idx: u32,
}

/// Full SignTx handshake. Returns sealed pieces the host assembles + relays.
pub fn trezor_sign_monero_transaction(
    req: &MoneroSignRequest,
) -> Result<MoneroSignedParts, OpalError> {
    if req.sources.is_empty() {
        return Err(OpalError::InvalidInput("no Monero inputs to sign".into()));
    }
    if req.destinations.is_empty() {
        return Err(OpalError::InvalidInput("no Monero destinations".into()));
    }
    if req.destinations.len() + 1 > 2 {
        // Device computes BP for ≤2 outputs; more needs host offload (not in this slice).
        return Err(OpalError::InvalidInput(
            "Trezor XMR send currently supports one destination + change (2 outputs)".into(),
        ));
    }
    let address_n = parse_bip32_path(&req.path)?;

    with_session(|session| sign_on_session(session, req, &address_n))
}

fn sign_on_session(
    session: &mut dyn Transport,
    req: &MoneroSignRequest,
    address_n: &[u32],
) -> Result<MoneroSignedParts, OpalError> {
    let init = encode_init(req, address_n);
    let (_, init_ack) = call_until(
        session,
        MSG_MONERO_TX_INIT_REQ,
        &init,
        &[MSG_MONERO_TX_INIT_ACK],
    )?;
    // Field 1 = repeated hmacs for destinations (incl. change), in order.
    let dest_hmacs = proto_get_bytes_all(&init_ack, 1);
    let expected_hmac = req.destinations.len() + 1;
    if dest_hmacs.len() < expected_hmac {
        return Err(OpalError::Io(format!(
            "Trezor InitAck returned {} destination HMACs, expected {expected_hmac}",
            dest_hmacs.len()
        )));
    }

    let mut sealed: Vec<SealedInput> = Vec::with_capacity(req.sources.len());
    for (orig_idx, src) in req.sources.iter().enumerate() {
        let body = proto_bytes_field(1, &encode_source(src));
        let (_, ack) = call_until(
            session,
            MSG_MONERO_TX_SET_INPUT_REQ,
            &body,
            &[MSG_MONERO_TX_SET_INPUT_ACK],
        )?;
        sealed.push(SealedInput {
            src: src.clone(),
            vini: proto_get_bytes(&ack, 1).unwrap_or_default(),
            vini_hmac: proto_get_bytes(&ack, 2).unwrap_or_default(),
            pseudo_out: proto_get_bytes(&ack, 3).unwrap_or_default(),
            pseudo_out_hmac: proto_get_bytes(&ack, 4).unwrap_or_default(),
            pseudo_out_alpha: proto_get_bytes(&ack, 5).unwrap_or_default(),
            spend_key: proto_get_bytes(&ack, 6).unwrap_or_default(),
            orig_idx: orig_idx as u32,
        });
    }

    // Sort by key-image (first 32 bytes of vini after tag) - firmware expects
    // InputVini in KI order. Heuristic: sort by vini bytes.
    sealed.sort_by(|a, b| a.vini.cmp(&b.vini));

    for s in &sealed {
        let mut body = Vec::new();
        body.extend(proto_bytes_field(1, &encode_source(&s.src)));
        body.extend(proto_bytes_field(2, &s.vini));
        body.extend(proto_bytes_field(3, &s.vini_hmac));
        body.extend(proto_bytes_field(4, &s.pseudo_out));
        body.extend(proto_bytes_field(5, &s.pseudo_out_hmac));
        body.extend(proto_varint_field(6, u64::from(s.orig_idx)));
        call_until(
            session,
            MSG_MONERO_TX_INPUT_VINI_REQ,
            &body,
            &[MSG_MONERO_TX_INPUT_VINI_ACK],
        )?;
    }

    call_until(
        session,
        MSG_MONERO_TX_ALL_INPUTS_SET_REQ,
        &[],
        &[MSG_MONERO_TX_ALL_INPUTS_SET_ACK],
    )?;

    let mut all_dests: Vec<&MoneroDestEntry> = req.destinations.iter().collect();
    all_dests.push(&req.change);

    let mut tx_outs = Vec::new();
    let mut out_pks = Vec::new();
    let mut ecdh_infos = Vec::new();
    let mut range_proof = Vec::new();
    for (i, d) in all_dests.iter().enumerate() {
        let mut body = Vec::new();
        body.extend(proto_bytes_field(1, &encode_dest(d)));
        body.extend(proto_bytes_field(2, &dest_hmacs[i]));
        let (_, ack) = call_until(
            session,
            MSG_MONERO_TX_SET_OUTPUT_REQ,
            &body,
            &[MSG_MONERO_TX_SET_OUTPUT_ACK],
        )?;
        tx_outs.push(proto_get_bytes(&ack, 1).unwrap_or_default());
        out_pks.push(proto_get_bytes(&ack, 4).unwrap_or_default());
        ecdh_infos.push(proto_get_bytes(&ack, 5).unwrap_or_default());
        if let Some(rsig_msg) = proto_get_bytes(&ack, 3) {
            // Nested MoneroTransactionRsigData.rsig (field 5)
            if let Some(rsig) = proto_get_bytes(&rsig_msg, 5) {
                if !rsig.is_empty() {
                    range_proof = rsig;
                }
            }
        }
    }

    let (_, all_out) = call_until(
        session,
        MSG_MONERO_TX_ALL_OUT_SET_REQ,
        &[],
        &[MSG_MONERO_TX_ALL_OUT_SET_ACK],
    )?;
    let extra = proto_get_bytes(&all_out, 1).unwrap_or_default();
    let tx_prefix_hash = proto_get_bytes(&all_out, 2).unwrap_or_default();
    let (fee, rv_type) = if let Some(rv) = proto_get_bytes(&all_out, 4) {
        (
            proto_get_varint(&rv, 1).unwrap_or(req.fee),
            proto_get_varint(&rv, 3).unwrap_or(5) as u32,
        )
    } else {
        (req.fee, 5)
    };

    let mut signatures = Vec::new();
    let mut pseudo_outs = Vec::new();
    let mut vinis = Vec::new();
    for s in &sealed {
        let mut body = Vec::new();
        body.extend(proto_bytes_field(1, &encode_source(&s.src)));
        body.extend(proto_bytes_field(2, &s.vini));
        body.extend(proto_bytes_field(3, &s.vini_hmac));
        body.extend(proto_bytes_field(4, &s.pseudo_out));
        body.extend(proto_bytes_field(5, &s.pseudo_out_hmac));
        body.extend(proto_bytes_field(6, &s.pseudo_out_alpha));
        body.extend(proto_bytes_field(7, &s.spend_key));
        body.extend(proto_varint_field(8, u64::from(s.orig_idx)));
        let (_, ack) = call_until(
            session,
            MSG_MONERO_TX_SIGN_INPUT_REQ,
            &body,
            &[MSG_MONERO_TX_SIGN_INPUT_ACK],
        )?;
        signatures.push(proto_get_bytes(&ack, 1).unwrap_or_default());
        let po = proto_get_bytes(&ack, 2).unwrap_or_else(|| s.pseudo_out.clone());
        pseudo_outs.push(po);
        vinis.push(s.vini.clone());
    }

    let (_, final_ack) = call_until(
        session,
        MSG_MONERO_TX_FINAL_REQ,
        &[],
        &[MSG_MONERO_TX_FINAL_ACK],
    )?;
    let opening_key = proto_get_bytes(&final_ack, 5).unwrap_or_default();
    let tx_enc_keys = proto_get_bytes(&final_ack, 4).unwrap_or_default();

    Ok(MoneroSignedParts {
        extra,
        tx_prefix_hash,
        fee,
        rv_type,
        vinis,
        tx_outs,
        out_pks,
        ecdh_infos,
        signatures,
        pseudo_outs,
        opening_key,
        tx_enc_keys,
        range_proof,
    })
}
