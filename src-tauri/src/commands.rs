use serde::Deserialize;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::OpalError;
use crate::state::AppState;
use crate::vault::{AppSettings, SecurityPreset, VaultStatus};

fn map_err(e: OpalError) -> String {
    e.into()
}

async fn on_worker<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("background task failed: {e}"))?
}

#[tauri::command]
pub fn vault_status(state: State<'_, AppState>) -> Result<VaultStatus, String> {
    let session = state.session.lock();
    state
        .vault
        .status(session.as_ref())
        .map_err(map_err)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateVaultRequest {
    pub password: String,
    pub preset: SecurityPreset,
    pub wipe_after_10_failures: bool,
}

#[tauri::command]
pub async fn vault_create(
    app: AppHandle,
    request: CreateVaultRequest,
) -> Result<VaultStatus, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        let unlocked = state
            .vault
            .create(
                &request.password,
                request.preset,
                request.wipe_after_10_failures,
            )
            .map_err(map_err)?;
        *state.session.lock() = Some(unlocked);
        let session = state.session.lock();
        state
            .vault
            .status(session.as_ref())
            .map_err(map_err)
    })
    .await
}

#[derive(Debug, Deserialize)]
pub struct UnlockRequest {
    pub password: String,
}

#[tauri::command]
pub async fn vault_unlock(
    app: AppHandle,
    request: UnlockRequest,
) -> Result<VaultStatus, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        let unlocked = state.vault.unlock(&request.password).map_err(map_err)?;
        *state.session.lock() = Some(unlocked);
        let session = state.session.lock();
        state
            .vault
            .status(session.as_ref())
            .map_err(map_err)
    })
    .await
}

#[tauri::command]
pub fn vault_lock(state: State<'_, AppState>) -> Result<VaultStatus, String> {
    {
        let mut session = state.session.lock();
        if let Some(mut unlocked) = session.take() {
            if let Some(ref mut mnemonic) = unlocked.payload.seed_mnemonic {
                // Best-effort clear
                mnemonic.clear();
            }
            // MasterKey drops via Drop impl (zeroize)
            drop(unlocked);
        }
    }
    state.vault.status(None).map_err(map_err)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    let session = state.session.lock();
    let unlocked = session.as_ref().ok_or_else(|| map_err(OpalError::Locked))?;
    Ok(unlocked.payload.settings.clone())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsRequest {
    pub language: Option<String>,
    pub fiat: Option<String>,
    pub discreet_mode: Option<bool>,
    pub wipe_after_10_failures: Option<bool>,
    pub bip39_passphrase_enabled: Option<bool>,
    pub bip39_passphrase: Option<Option<String>>,
    pub tor_socks: Option<Option<String>>,
    pub auto_lock_minutes: Option<u32>,
    pub start_with_windows: Option<bool>,
    pub notifications_enabled: Option<bool>,
    pub custom_rpc: Option<std::collections::HashMap<String, String>>,
    pub fixedfloat_api_key: Option<Option<String>>,
    pub fixedfloat_api_secret: Option<Option<String>>,
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    request: UpdateSettingsRequest,
) -> Result<AppSettings, String> {
    let mut session = state.session.lock();
    let unlocked = session
        .as_mut()
        .ok_or_else(|| map_err(OpalError::Locked))?;

    if let Some(v) = request.language {
        unlocked.payload.settings.language = v;
    }
    if let Some(v) = request.fiat {
        unlocked.payload.settings.fiat = v;
    }
    if let Some(v) = request.discreet_mode {
        unlocked.payload.settings.discreet_mode = v;
    }
    if let Some(v) = request.wipe_after_10_failures {
        unlocked.payload.settings.wipe_after_10_failures = v;
    }
    if let Some(v) = request.bip39_passphrase_enabled {
        unlocked.payload.settings.bip39_passphrase_enabled = v;
        if !v {
            unlocked.payload.settings.bip39_passphrase = None;
        }
    }
    if let Some(v) = request.bip39_passphrase {
        unlocked.payload.settings.bip39_passphrase = v.filter(|s| !s.is_empty());
    }
    if let Some(v) = request.tor_socks {
        unlocked.payload.settings.tor_socks = v.filter(|s| !s.trim().is_empty());
    }
    if let Some(v) = request.auto_lock_minutes {
        unlocked.payload.settings.auto_lock_minutes = v;
    }
    if let Some(v) = request.start_with_windows {
        unlocked.payload.settings.start_with_windows = v;
    }
    if let Some(v) = request.notifications_enabled {
        unlocked.payload.settings.notifications_enabled = v;
    }
    // custom_rpc ignored — Opal uses curated public nodes only.
    unlocked.payload.settings.custom_rpc.clear();

    if let Some(v) = request.fixedfloat_api_key {
        unlocked.payload.settings.fixedfloat_api_key = v.filter(|s| !s.trim().is_empty());
    }
    if let Some(v) = request.fixedfloat_api_secret {
        unlocked.payload.settings.fixedfloat_api_secret = v.filter(|s| !s.trim().is_empty());
    }

    state.vault.persist(unlocked).map_err(map_err)?;
    Ok(unlocked.payload.settings.clone())
}

#[tauri::command]
pub fn update_settings_with_autostart(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: UpdateSettingsRequest,
) -> Result<AppSettings, String> {
    let settings = update_settings(state, request)?;
    let _ = crate::desktop::apply_autostart(&app, settings.start_with_windows);
    Ok(settings)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[tauri::command]
pub async fn change_password(
    app: AppHandle,
    request: ChangePasswordRequest,
) -> Result<(), String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        let mut session = state.session.lock();
        let unlocked = session
            .as_mut()
            .ok_or_else(|| map_err(OpalError::Locked))?;
        state
            .vault
            .change_password(
                unlocked,
                &request.current_password,
                &request.new_password,
            )
            .map_err(map_err)
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePresetRequest {
    pub password: String,
    pub preset: SecurityPreset,
}

#[tauri::command]
pub async fn change_security_preset(
    app: AppHandle,
    request: ChangePresetRequest,
) -> Result<AppSettings, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        let mut session = state.session.lock();
        let unlocked = session
            .as_mut()
            .ok_or_else(|| map_err(OpalError::Locked))?;
        state
            .vault
            .rewrap_for_preset(unlocked, &request.password, request.preset)
            .map_err(map_err)?;
        Ok(unlocked.payload.settings.clone())
    })
    .await
}

#[tauri::command]
pub fn vault_path(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.vault.path().display().to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultExportRequest {
    pub password: String,
    pub dest_path: String,
}

/// Copy the encrypted vault file to dest (metadata backup). Seed is inside ciphertext.
// Password KDF verification is CPU-heavy (multi-second under the Paranoid
// preset) — run off the webview's main thread so exporting doesn't freeze it.
#[tauri::command]
pub async fn vault_export(app: AppHandle, request: VaultExportRequest) -> Result<(), String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_session_or_password(&state, &request.password)?;
        let path = state.vault.path();
        if !path.exists() {
            return Err(map_err(OpalError::VaultMissing));
        }
        std::fs::copy(&path, &request.dest_path)
            .map_err(|e| map_err(OpalError::Io(e.to_string())))?;
        Ok(())
    })
    .await
}

fn require_session_or_password(state: &AppState, password: &str) -> Result<(), String> {
    if state.session.lock().is_some() {
        // Confirm password still valid by deriving KEK against file
        let _ = state.vault.unlock(password).map_err(map_err)?;
        return Ok(());
    }
    let _ = state.vault.unlock(password).map_err(map_err)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultWipeRequest {
    pub password: String,
}

/// Permanently delete the local vault so the user can create/restore from scratch.
/// Requires the current unlock password. Clears the in-memory session.
#[tauri::command]
pub async fn vault_wipe(
    app: AppHandle,
    request: VaultWipeRequest,
) -> Result<VaultStatus, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        require_session_or_password(&state, &request.password)?;
        *state.session.lock() = None;
        state.vault.wipe().map_err(map_err)?;
        state.vault.status(None).map_err(map_err)
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultImportRequest {
    pub password: String,
    pub src_path: String,
}

/// Replace local vault with an imported encrypted vault file (locks session).
// Same reasoning as vault_export: unlocking runs a real KDF, so this must be
// off the main thread or the whole app freezes while it derives the key.
#[tauri::command]
pub async fn vault_import(
    app: AppHandle,
    request: VaultImportRequest,
) -> Result<VaultStatus, String> {
    on_worker(move || {
        let state = app.state::<AppState>();
        *state.session.lock() = None;
        let dest = state.vault.path();
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::copy(&request.src_path, &dest)
            .map_err(|e| map_err(OpalError::Io(e.to_string())))?;
        let unlocked = state.vault.unlock(&request.password).map_err(map_err)?;
        *state.session.lock() = Some(unlocked);
        let session = state.session.lock();
        state.vault.status(session.as_ref()).map_err(map_err)
    })
    .await
}

#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    crate::desktop::apply_autostart(&app, enabled)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub license: String,
    pub tagline: String,
    pub trezor_disclaimer: String,
    pub source_url: String,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "Opal".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        license: "MIT".into(),
        tagline: "Quiet self-custody".into(),
        trezor_disclaimer:
            "Not affiliated with SatoshiLabs / Trezor. Trezor is a trademark of SatoshiLabs."
                .into(),
        source_url: "https://github.com/lilathy/opal".into(),
    }
}

#[tauri::command]
pub fn perf_debug_log(message: String) {
    crate::perf::append_frontend_log(message);
}

#[tauri::command]
pub fn perf_debug_snapshot() -> crate::perf::PerfSnapshot {
    crate::perf::snapshot()
}

#[tauri::command]
pub fn perf_ping() -> f64 {
    crate::perf::ping_now_ms()
}

#[tauri::command]
pub fn perf_run_bench(iters: Option<u32>) -> crate::perf::PerfBenchResult {
    crate::perf::run_bench(iters.unwrap_or(40))
}
