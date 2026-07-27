mod address_util;
mod commands;
mod desktop;
mod error;
mod network;
mod perf;
mod state;
mod trezor;
mod vault;
mod wallet;
mod wallet_commands;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = AppState::new().expect("failed to initialize Opal vault service");

    let builder = tauri::Builder::default();
    let builder = desktop::register_plugins(builder);

    builder
        .manage(app_state)
        .setup(|app| {
            perf::boost_responsiveness();
            network::start_price_book_loop();
            if let Err(e) = desktop::setup_tray(app.handle()) {
                eprintln!("tray setup: {e}");
            }
            // Lock vault when OS signals sleep / screen lock (best-effort via JS event from frontend too).
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::vault_status,
            commands::vault_create,
            commands::vault_unlock,
            commands::vault_lock,
            commands::vault_wipe,
            commands::get_settings,
            commands::update_settings,
            commands::change_password,
            commands::change_security_preset,
            commands::vault_path,
            commands::vault_export,
            commands::vault_import,
            commands::set_autostart,
            commands::app_info,
            commands::perf_debug_log,
            commands::perf_debug_snapshot,
            commands::perf_ping,
            commands::perf_run_bench,
            wallet_commands::chain_list,
            wallet_commands::wallet_create_seed,
            wallet_commands::wallet_restore_seed,
            wallet_commands::wallet_discover_portfolios,
            wallet_commands::wallet_confirm_backup,
            wallet_commands::wallet_reveal_seed,
            wallet_commands::portfolio_create,
            wallet_commands::portfolio_list,
            wallet_commands::portfolio_rename,
            wallet_commands::portfolio_delete,
            wallet_commands::portfolio_reorder,
            wallet_commands::portfolio_balances,
            wallet_commands::portfolio_balances_cached,
            wallet_commands::portfolio_receive_address,
            wallet_commands::portfolio_receive_uri,
            wallet_commands::portfolio_history,
            wallet_commands::portfolio_send,
            wallet_commands::portfolio_estimate_fee,
            wallet_commands::portfolio_next_address,
            wallet_commands::portfolio_rescan,
            wallet_commands::portfolio_bump_fee,
            wallet_commands::prices_fiat,
            wallet_commands::warm_spot_prices,
            wallet_commands::spot_prices_snapshot,
            wallet_commands::price_history,
            wallet_commands::trezor_status,
            wallet_commands::trezor_verify_address,
            wallet_commands::trezor_discover_portfolios,
            wallet_commands::trezor_sync_xmr_key_images,
            wallet_commands::detect_address_chains,
            wallet_commands::address_book_list,
            wallet_commands::address_book_add,
            wallet_commands::address_book_remove,
            wallet_commands::analyze_send_address,
            wallet_commands::swap_quote,
            wallet_commands::swap_jupiter_tx,
            wallet_commands::swap_fixedfloat_create,
            wallet_commands::swap_fixedfloat_order,
            wallet_commands::swap_fixedfloat_execute,
            wallet_commands::swap_fixedfloat_ready,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Opal");
}
