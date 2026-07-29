//! Native USB transport for Trezor devices - talks to the device directly
//! over WinUSB/libusb-style bulk transfers, the same way Trezor Suite does
//! when it isn't using Bridge. No separate service needs to be installed or
//! running for this path to work.
//!
//! VID/PID, interface/endpoint numbers, and the wire framing below were
//! pulled directly from trezor-suite's own source
//! (`packages/transport-common/src/constants.ts` and
//! `packages/protocol/src/protocol-v1/{constants,encode}.ts`), not guessed.
//!
//! Prefer Bridge (`with_session`) when Suite/trezord is running - fighting it
//! for the WinUSB handle produces Windows error 5 and `usb write: Cancelled`.

use std::sync::atomic::Ordering;
use std::time::Duration;

use once_cell::sync::Lazy;
use nusb::transfer::{Buffer, In, Interrupt, Out};
use nusb::{Interface, MaybeFuture};
use parking_lot::{Mutex, MutexGuard};

use super::Transport;
use crate::error::OpalError;

const VID: u16 = 0x1209;
const PID_FIRMWARE: u16 = 0x53c1;
const PID_BOOTLOADER: u16 = 0x53c0;
const USB_INTERFACE: u8 = 0;
const EP_OUT: u8 = 0x01;
const EP_IN: u8 = 0x81;
const CHUNK_SIZE: usize = 64;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Reads must wait through on-device PIN / passphrase / button confirms.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Only one USB handle may be open at a time.
static USB_EXCLUSIVE: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Clone, Default)]
struct CachedDeviceInfo {
    label: Option<String>,
    model: Option<String>,
    internal_model: Option<String>,
}

static CACHED_INFO: Lazy<Mutex<CachedDeviceInfo>> =
    Lazy::new(|| Mutex::new(CachedDeviceInfo::default()));

fn is_trezor(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == VID && (product_id == PID_FIRMWARE || product_id == PID_BOOTLOADER)
}

fn find_device_info() -> Option<nusb::DeviceInfo> {
    let devices = nusb::list_devices().wait().ok()?;
    devices
        .into_iter()
        .find(|d| is_trezor(d.vendor_id(), d.product_id()))
}

pub fn device_present() -> bool {
    find_device_info().is_some()
}

/// Remember Features fields from a successful Initialize (USB or Bridge).
pub fn cache_features(
    label: Option<String>,
    model: Option<String>,
    internal_model: Option<String>,
) {
    let mut guard = CACHED_INFO.lock();
    if label.is_some() {
        guard.label = label;
    }
    if model.is_some() {
        guard.model = model;
    }
    if internal_model.is_some() {
        guard.internal_model = internal_model;
    }
}

pub fn cached_features() -> (Option<String>, Option<String>, Option<String>) {
    let g = CACHED_INFO.lock();
    (g.label.clone(), g.model.clone(), g.internal_model.clone())
}

/// RAII: mirror Bridge's SESSION_ACTIVE so the UI knows the device is busy.
struct SessionActiveGuard;

impl SessionActiveGuard {
    fn arm() -> Self {
        super::SESSION_ACTIVE.store(true, Ordering::SeqCst);
        Self
    }
}

impl Drop for SessionActiveGuard {
    fn drop(&mut self) {
        super::SESSION_ACTIVE.store(false, Ordering::SeqCst);
    }
}

pub struct UsbSession {
    // Drop order (reverse of declaration): endpoints → interface → exclusive
    // lock → session flag. Device must be released before we advertise idle.
    _session_flag: SessionActiveGuard,
    _exclusive: MutexGuard<'static, ()>,
    _interface: Interface,
    ep_out: nusb::Endpoint<Interrupt, Out>,
    ep_in: nusb::Endpoint<Interrupt, In>,
}

impl UsbSession {
    pub fn open() -> Result<Self, OpalError> {
        let exclusive = USB_EXCLUSIVE.lock();
        Self::open_locked(exclusive)
    }

    fn open_locked(exclusive: MutexGuard<'static, ()>) -> Result<Self, OpalError> {
        let session_flag = SessionActiveGuard::arm();
        let info = find_device_info().ok_or_else(|| {
            OpalError::InvalidInput("No Trezor device found over USB.".into())
        })?;
        let device = info.open().wait().map_err(|e| {
            OpalError::Io(format!(
                "usb open: {e} (close Trezor Suite if it's using the device, or leave Suite open and use Bridge)"
            ))
        })?;
        let interface = device.claim_interface(USB_INTERFACE).wait().map_err(|e| {
            OpalError::Io(format!(
                "usb claim interface: {e} (device busy - close Trezor Suite or retry)"
            ))
        })?;
        let ep_out = interface
            .endpoint::<Interrupt, Out>(EP_OUT)
            .map_err(|e| OpalError::Io(format!("usb out endpoint: {e}")))?;
        let ep_in = interface
            .endpoint::<Interrupt, In>(EP_IN)
            .map_err(|e| OpalError::Io(format!("usb in endpoint: {e}")))?;
        let mut session = Self {
            _session_flag: session_flag,
            _exclusive: exclusive,
            _interface: interface,
            ep_out,
            ep_in,
        };
        // Drop any stale IN reports left by a previous client / crashed transfer.
        session.drain_in();
        Ok(session)
    }

    fn drain_in(&mut self) {
        for _ in 0..8 {
            let completion = self
                .ep_in
                .transfer_blocking(Buffer::new(CHUNK_SIZE), Duration::from_millis(20));
            match completion.into_result() {
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    }

    fn write_chunk(&mut self, chunk: [u8; CHUNK_SIZE]) -> Result<(), OpalError> {
        let mut last_err = None;
        for attempt in 0..3 {
            let buf: Buffer = chunk.to_vec().into();
            match self
                .ep_out
                .transfer_blocking(buf, WRITE_TIMEOUT)
                .into_result()
            {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let msg = format!("{e:?}");
                    let retryable = msg.contains("Cancelled")
                        || msg.contains("TimedOut")
                        || msg.contains("Timeout")
                        || msg.contains("Stalled");
                    last_err = Some(msg);
                    if !retryable || attempt == 2 {
                        break;
                    }
                    self.drain_in();
                    std::thread::sleep(Duration::from_millis(40));
                }
            }
        }
        Err(OpalError::Io(format!(
            "usb write: {}",
            last_err.unwrap_or_else(|| "unknown".into())
        )))
    }

    fn read_chunk(&mut self) -> Result<[u8; CHUNK_SIZE], OpalError> {
        // One long wait - do not slice/cancel/retry. Cancelling an IN transfer
        // on Windows can discard a reply the device already sent (ButtonAck).
        let completion = self
            .ep_in
            .transfer_blocking(Buffer::new(CHUNK_SIZE), READ_TIMEOUT);
        let actual_len = completion.actual_len;
        let buffer = completion.into_result().map_err(|e| {
            let msg = format!("{e:?}");
            if msg.contains("Cancelled") || msg.contains("TimedOut") || msg.contains("Timeout") {
                OpalError::Io(
                    "usb read: timed out waiting for the device (confirm or cancel on Trezor)"
                        .into(),
                )
            } else {
                OpalError::Io(format!("usb read: {msg}"))
            }
        })?;
        if actual_len == CHUNK_SIZE {
            let mut out = [0u8; CHUNK_SIZE];
            out.copy_from_slice(&buffer[..CHUNK_SIZE]);
            return Ok(out);
        }
        // Spurious zero-length interrupt completion - retry once quickly.
        if actual_len == 0 {
            let completion = self
                .ep_in
                .transfer_blocking(Buffer::new(CHUNK_SIZE), READ_TIMEOUT);
            let actual_len = completion.actual_len;
            let buffer = completion
                .into_result()
                .map_err(|e| OpalError::Io(format!("usb read: {e:?}")))?;
            if actual_len == CHUNK_SIZE {
                let mut out = [0u8; CHUNK_SIZE];
                out.copy_from_slice(&buffer[..CHUNK_SIZE]);
                return Ok(out);
            }
        }
        Err(OpalError::Io(format!(
            "usb read: short packet ({actual_len} bytes)"
        )))
    }

    fn write_message(&mut self, encoded: &[u8]) -> Result<(), OpalError> {
        let mut offset = 0usize;
        let mut is_first = true;
        while offset < encoded.len() {
            let mut chunk = [0u8; CHUNK_SIZE];
            let dest_start = if is_first {
                0
            } else {
                chunk[0] = MAGIC;
                1
            };
            let cap = CHUNK_SIZE - dest_start;
            let n = (encoded.len() - offset).min(cap);
            chunk[dest_start..dest_start + n].copy_from_slice(&encoded[offset..offset + n]);
            self.write_chunk(chunk)?;
            offset += n;
            is_first = false;
        }
        Ok(())
    }
}

const MAGIC: u8 = 0x3f; // '?'
const HEADER_TAG: u8 = 0x23; // '#'

impl Transport for UsbSession {
    fn call_raw(&mut self, msg_type: u16, payload: &[u8]) -> Result<(u16, Vec<u8>), OpalError> {
        let mut encoded = Vec::with_capacity(9 + payload.len());
        encoded.push(MAGIC);
        encoded.push(HEADER_TAG);
        encoded.push(HEADER_TAG);
        encoded.extend_from_slice(&msg_type.to_be_bytes());
        encoded.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        encoded.extend_from_slice(payload);
        self.write_message(&encoded)?;

        let first = self.read_chunk()?;
        if first[0] != MAGIC || first[1] != HEADER_TAG || first[2] != HEADER_TAG {
            return Err(OpalError::Io("usb read: bad response header".into()));
        }
        let resp_type = u16::from_be_bytes([first[3], first[4]]);
        let len = u32::from_be_bytes([first[5], first[6], first[7], first[8]]) as usize;

        let mut data = Vec::with_capacity(len);
        let avail = CHUNK_SIZE - 9;
        let take = avail.min(len);
        data.extend_from_slice(&first[9..9 + take]);
        while data.len() < len {
            let chunk = self.read_chunk()?;
            let take = (CHUNK_SIZE - 1).min(len - data.len());
            data.extend_from_slice(&chunk[1..1 + take]);
        }
        Ok((resp_type, data))
    }
}

/// Soft status check - **never opens the USB device**. Opening on every UI
/// poll was hanging for 15s (`usb write: Cancelled`) and blocking verify/send.
/// Returns `None` when no USB Trezor is present so the caller can fall back
/// to Bridge.
pub fn probe_status(session_active: bool) -> Option<super::TrezorStatus> {
    find_device_info()?;
    let (device_label, device_model, device_internal_model) = cached_features();
    Some(super::TrezorStatus {
        available: true,
        bridge_url: "usb".into(),
        message: "Connected".into(),
        suite_required: false,
        device_count: 1,
        session_active: session_active || super::SESSION_ACTIVE.load(Ordering::SeqCst),
        device_label,
        device_model,
        device_internal_model,
    })
}
