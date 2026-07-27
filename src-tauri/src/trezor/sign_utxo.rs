//! Bitcoin-family SignTx (TxRequest / TxAck) for native SegWit / legacy P2PKH.

use crate::error::OpalError;

use super::{
    call_until, parse_bip32_path, proto_bytes_field, proto_get_bytes, proto_get_varint,
    proto_string_field, proto_varint_field, with_session, MSG_SIGN_TX, MSG_TX_ACK, MSG_TX_REQUEST,
};

/// Request type codes inside TxRequest.request_type
const REQ_TXINPUT: u64 = 0;
const REQ_TXOUTPUT: u64 = 1;
const REQ_TXMETA: u64 = 2;
const REQ_TXFINISHED: u64 = 3;
const REQ_TXEXTRADATA: u64 = 4;

#[derive(Debug, Clone)]
pub struct BitcoinSignInput {
    pub path: String,
    pub prev_hash: Vec<u8>, // 32 bytes, internal byte order (reversed txid)
    pub prev_index: u32,
    pub amount: u64,
    pub sequence: u32,
    pub script_type: u32, // SPENDWITNESS=3, SPENDADDRESS=0, etc.
    /// For segwit: empty; for legacy: scriptPubKey of the UTXO.
    pub script_sig: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BitcoinSignOutput {
    pub amount: u64,
    pub address: Option<String>,
    pub script_type: u32, // PAYTOADDRESS=0, PAYTOWITNESS=3, etc.
    pub address_n: Option<String>, // change path
}

#[derive(Debug, Clone)]
pub struct BitcoinSignRequest {
    pub coin_name: String,
    pub version: u32,
    pub lock_time: u32,
    pub inputs: Vec<BitcoinSignInput>,
    pub outputs: Vec<BitcoinSignOutput>,
}

/// Run the Trezor SignTx protocol; returns serialized signed transaction bytes.
pub fn trezor_sign_bitcoin_tx(req: &BitcoinSignRequest) -> Result<Vec<u8>, OpalError> {
    if req.inputs.is_empty() || req.outputs.is_empty() {
        return Err(OpalError::InvalidInput("SignTx needs inputs and outputs".into()));
    }

    with_session(|session| {
        let mut body = Vec::new();
        body.extend(proto_varint_field(1, req.outputs.len() as u64));
        body.extend(proto_varint_field(2, req.inputs.len() as u64));
        body.extend(proto_string_field(3, &req.coin_name));
        body.extend(proto_varint_field(4, u64::from(req.version)));
        body.extend(proto_varint_field(5, u64::from(req.lock_time)));

        let mut resp = call_until(session, MSG_SIGN_TX, &body, &[MSG_TX_REQUEST])?;
        let mut serialized = Vec::new();

        loop {
            let payload = &resp.1;
            let request_type = proto_get_varint(payload, 1).unwrap_or(REQ_TXFINISHED);
            let details_idx = proto_get_varint(payload, 2); // request_details.request_index nested — flat parse may miss
            // TxRequest.details is field 2 (message). Extract request_index (field 1) and tx_hash (field 2).
            let details = proto_get_bytes(payload, 2).unwrap_or_default();
            let req_index = proto_get_varint(&details, 1).unwrap_or(0) as usize;
            let _tx_hash = proto_get_bytes(&details, 2);
            let extra_len = proto_get_varint(&details, 3).unwrap_or(0);
            let extra_offset = proto_get_varint(&details, 4).unwrap_or(0);

            // Serialized chunk from device (field 3 of TxRequest)
            if let Some(chunk) = proto_get_bytes(payload, 3) {
                serialized.extend_from_slice(&chunk);
            }

            if request_type == REQ_TXFINISHED {
                break;
            }

            let ack = match request_type {
                REQ_TXMETA => encode_tx_meta(req),
                REQ_TXINPUT => {
                    let input = req.inputs.get(req_index).ok_or_else(|| {
                        OpalError::InvalidInput(format!("TxRequest input index {req_index}"))
                    })?;
                    encode_tx_input(input)?
                }
                REQ_TXOUTPUT => {
                    let output = req.outputs.get(req_index).ok_or_else(|| {
                        OpalError::InvalidInput(format!("TxRequest output index {req_index}"))
                    })?;
                    encode_tx_output(output)?
                }
                REQ_TXEXTRADATA => {
                    // Prev-tx extradata for legacy non-segwit inputs — we only
                    // support segwit spends where this shouldn't be needed.
                    let _ = (extra_len, extra_offset);
                    return Err(OpalError::InvalidInput(
                        "Trezor requested previous-tx extradata (legacy non-segwit). Use a native SegWit account.".into(),
                    ));
                }
                other => {
                    return Err(OpalError::InvalidInput(format!(
                        "unsupported TxRequest type {other}"
                    )));
                }
            };

            let _ = details_idx;
            resp = call_until(session, MSG_TX_ACK, &ack, &[MSG_TX_REQUEST])?;
        }

        if serialized.is_empty() {
            return Err(OpalError::InvalidInput(
                "Trezor SignTx finished without serialized transaction".into(),
            ));
        }
        Ok(serialized)
    })
}

fn encode_tx_meta(req: &BitcoinSignRequest) -> Vec<u8> {
    // TxAck.tx (field 1) = TransactionType
    let mut tx = Vec::new();
    tx.extend(proto_varint_field(1, u64::from(req.version)));
    tx.extend(proto_varint_field(4, req.inputs.len() as u64)); // inputs_cnt
    tx.extend(proto_varint_field(6, req.outputs.len() as u64)); // outputs_cnt
    tx.extend(proto_varint_field(5, u64::from(req.lock_time)));
    wrap_tx_ack(tx)
}

fn encode_tx_input(input: &BitcoinSignInput) -> Result<Vec<u8>, OpalError> {
    let path = parse_bip32_path(&input.path)?;
    let mut tin = Vec::new();
    for n in &path {
        tin.extend(proto_varint_field(1, u64::from(*n))); // address_n
    }
    tin.extend(proto_bytes_field(2, &input.prev_hash));
    tin.extend(proto_varint_field(3, u64::from(input.prev_index)));
    if !input.script_sig.is_empty() {
        tin.extend(proto_bytes_field(4, &input.script_sig));
    }
    tin.extend(proto_varint_field(5, u64::from(input.sequence)));
    tin.extend(proto_varint_field(6, u64::from(input.script_type)));
    tin.extend(proto_varint_field(8, input.amount)); // amount (segwit)

    let mut tx = Vec::new();
    tx.extend(proto_bytes_field(2, &tin)); // TransactionType.inputs = field 2
    Ok(wrap_tx_ack(tx))
}

fn encode_tx_output(output: &BitcoinSignOutput) -> Result<Vec<u8>, OpalError> {
    let mut tout = Vec::new();
    tout.extend(proto_varint_field(1, output.amount));
    tout.extend(proto_varint_field(2, u64::from(output.script_type)));
    if let Some(ref path) = output.address_n {
        for n in parse_bip32_path(path)? {
            tout.extend(proto_varint_field(3, u64::from(n)));
        }
    }
    if let Some(ref addr) = output.address {
        tout.extend(proto_string_field(4, addr));
    }

    let mut tx = Vec::new();
    // bin_outputs for change with address_n use field 3; address outputs use field 5 (outputs)
    if output.address_n.is_some() {
        tx.extend(proto_bytes_field(3, &tout)); // bin_outputs
    } else {
        tx.extend(proto_bytes_field(5, &tout)); // outputs (TxOutputType)
    }
    Ok(wrap_tx_ack(tx))
}

fn wrap_tx_ack(tx_msg: Vec<u8>) -> Vec<u8> {
    // TxAck { tx: TransactionType }
    proto_bytes_field(1, &tx_msg)
}
