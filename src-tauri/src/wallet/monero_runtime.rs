//! Locate / spawn bundled-or-AppData `monero-wallet-rpc` against public daemons.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::Mutex as PlMutex;

use crate::error::OpalError;
use crate::wallet::xmr_rpc::{default_wallet_rpc_url, xmr_wallet_dir};

static WALLET_RPC_CHILD: Lazy<PlMutex<Option<Child>>> = Lazy::new(|| PlMutex::new(None));
static START_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

const PUBLIC_DAEMONS: &[&str] = &[
    "node.community.rino.io:18081",
    "xmr-node.cakewallet.com:18081",
    "node.supportxmr.com:18081",
];

fn monero_install_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Opal")
        .join("monero")
}

pub fn monero_wallet_rpc_path() -> Option<PathBuf> {
    let candidates = [
        monero_install_dir().join("monero-wallet-rpc.exe"),
        // Nested extract layout: monero-x86_64-w64-mingw32-v0.18.5.1/...
        monero_install_dir()
            .join("monero-x86_64-w64-mingw32-v0.18.5.1")
            .join("monero-wallet-rpc.exe"),
    ];
    candidates.into_iter().find(|p| p.is_file()).or_else(|| {
        // Any nested monero-wallet-rpc.exe under install dir
        let root = monero_install_dir();
        if !root.is_dir() {
            return None;
        }
        walkdir_find(&root, "monero-wallet-rpc.exe")
    })
}

fn walkdir_find(root: &std::path::Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|s| s.to_str()) == Some(name) {
                return Some(p);
            }
        }
    }
    None
}

fn rpc_reachable() -> bool {
    let url = format!("{}/json_rpc", default_wallet_rpc_url().trim_end_matches('/'));
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "0",
        "method": "get_version",
        "params": {}
    });
    client
        .post(&url)
        .json(&body)
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

pub fn is_wallet_rpc_ready() -> bool {
    rpc_reachable()
}

/// Ensure local monero-wallet-rpc is up (spawn if needed). Idempotent.
pub fn ensure_wallet_rpc_running() -> Result<(), OpalError> {
    let _guard = START_LOCK
        .lock()
        .map_err(|_| OpalError::Io("monero start lock poisoned".into()))?;

    if rpc_reachable() {
        return Ok(());
    }

    let exe = monero_wallet_rpc_path().ok_or_else(|| {
        OpalError::Io(format!(
            "monero-wallet-rpc not found under {}. Re-run Opal setup or place the binary there.",
            monero_install_dir().display()
        ))
    })?;

    let wallet_dir = xmr_wallet_dir()?;
    let log = wallet_dir.join("wallet-rpc.log");
    let daemon = PUBLIC_DAEMONS[0];

    // Kill previous child we spawned if any
    {
        let mut slot = WALLET_RPC_CHILD.lock();
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    let mut cmd = Command::new(&exe);
    cmd.arg("--rpc-bind-ip")
        .arg("127.0.0.1")
        .arg("--rpc-bind-port")
        .arg("18083")
        .arg("--disable-rpc-login")
        .arg("--wallet-dir")
        .arg(&wallet_dir)
        .arg("--daemon-address")
        .arg(daemon)
        .arg("--untrusted-daemon")
        .arg("--log-file")
        .arg(&log)
        .arg("--max-log-files")
        .arg("2")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd.spawn().map_err(|e| {
        OpalError::Io(format!(
            "failed to start monero-wallet-rpc ({}): {e}",
            exe.display()
        ))
    })?;
    *WALLET_RPC_CHILD.lock() = Some(child);

    // Wait until RPC answers (first boot can be slow, but don't freeze forever)
    for _ in 0..20 {
        if rpc_reachable() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    Err(OpalError::Io(format!(
        "monero-wallet-rpc started but did not become ready at {}. Check {}",
        default_wallet_rpc_url(),
        log.display()
    )))
}

pub fn public_xmr_daemon() -> &'static str {
    PUBLIC_DAEMONS[0]
}
