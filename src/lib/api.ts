import { timedInvoke } from "./perfDebug";

export type SecurityPreset = "fast" | "normal" | "paranoid";
export type VaultPhase = "needs_create" | "locked" | "unlocked";
export type FeePreset = "economy" | "normal" | "priority";
export type BtcAddressType = "native_segwit" | "taproot" | "legacy";
export type PortfolioKind = "software" | "trezor" | "watch_only";

export interface VaultStatus {
  phase: VaultPhase;
  failed_attempts: number;
  wipe_after_failures: number | null;
  preset: SecurityPreset | null;
  has_seed: boolean;
  seed_backed_up: boolean;
  discreet_mode: boolean;
  language: string;
  fiat: string;
  auto_lock_minutes: number;
  bip39_passphrase_enabled: boolean;
  tor_socks: string | null;
  wipe_after_10_failures: boolean;
  security_preset: SecurityPreset;
  start_with_windows: boolean;
  notifications_enabled: boolean;
  activity_min_fiat: number;
  analytics_enabled: boolean;
  analytics_tile_order: string[];
  analytics_hidden_tiles: string[];
}

export interface AppSettings {
  language: string;
  fiat: string;
  discreet_mode: boolean;
  security_preset: SecurityPreset;
  wipe_after_10_failures: boolean;
  bip39_passphrase_enabled: boolean;
  bip39_passphrase?: string | null;
  tor_socks: string | null;
  auto_lock_minutes: number;
  start_with_windows: boolean;
  notifications_enabled: boolean;
  /** Hide activity below this notional in the display fiat. */
  activity_min_fiat: number;
  analytics_enabled: boolean;
  analytics_tile_order: string[];
  analytics_hidden_tiles: string[];
  custom_rpc: Record<string, string>;
  fixedfloat_api_key?: string | null;
  fixedfloat_api_secret?: string | null;
}

export interface AppInfo {
  name: string;
  version: string;
  license: string;
  tagline: string;
  trezorDisclaimer: string;
  sourceUrl: string;
}

export interface PortfolioRecord {
  id: string;
  name: string;
  kind: PortfolioKind;
  chain: string;
  created_at: string;
  account_index: number;
  address_index: number;
  address?: string | null;
  xmr_view_key?: string | null;
  notes?: string | null;
  trezor_label?: string | null;
  address_type?: string | null;
  cached_balances_json?: string | null;
}

export interface AssetBalance {
  symbol: string;
  amount: string;
  decimals: number;
  usd: number | null;
}

export interface PortfolioBalance {
  portfolio_id: string;
  chain: string;
  address: string;
  assets: AssetBalance[];
}

export interface TxRow {
  txid: string;
  /** Human-readable, already divided by decimals - never a raw base-unit integer. */
  amount: string;
  /** Ticker to show next to `amount` - native symbol, or a token symbol for ERC-20/TRC-20 rows. */
  symbol: string;
  direction: "in" | "out" | "self" | "unknown" | string;
  timestamp: string;
  status: string;
  fee?: string | null;
  counterparty?: string | null;
  explorer_url: string;
}

export interface SendResult {
  txid: string;
  explorer_url: string;
}

export interface ChainInfo {
  id: string;
  name: string;
  kind: string;
  tokens: string[];
}

export interface TrezorStatus {
  available: boolean;
  bridge_url: string;
  message: string;
  suite_required: boolean;
  device_count: number;
  session_active: boolean;
  device_label: string | null;
  device_model: string | null;
  device_internal_model: string | null;
}

export interface AddressBookEntry {
  id: string;
  label: string;
  chain: string;
  address: string;
}

export interface AddressSafety {
  ok: boolean;
  warnings: string[];
  display_prefix: string;
  display_suffix: string;
  display_middle_masked: string;
}

export interface FeeEstimate {
  feeSats: number | null;
  economySatVb?: number;
  normalSatVb?: number;
  prioritySatVb?: number;
}

export interface CreatePortfolioRequest {
  name: string;
  chain: string;
  kind: PortfolioKind;
  accountIndex?: number;
  address?: string | null;
  xmrViewKey?: string | null;
  trezorLabel?: string | null;
  addressType?: BtcAddressType | string | null;
  verifyOnDevice?: boolean;
}

export interface SendRequest {
  portfolioId: string;
  to: string;
  amount: string;
  token?: string | null;
  feePreset?: FeePreset | string | null;
  customFeeSatVb?: number | null;
  sendMax?: boolean | null;
}

export interface OpalErrorBody {
  code: string;
  message: string;
}

export function parseInvokeError(error: unknown): OpalErrorBody {
  if (typeof error === "string") {
    try {
      const parsed = JSON.parse(error) as OpalErrorBody;
      if (parsed?.code) return parsed;
    } catch {
      /* fall through */
    }
    return { code: "unknown", message: error };
  }
  if (error && typeof error === "object" && "message" in error) {
    return parseInvokeError(String((error as { message: unknown }).message));
  }
  return { code: "unknown", message: "Unknown error" };
}

const historyCache = new Map<string, { at: number; rows: TxRow[] }>();
const HISTORY_CACHE_MS = 5 * 60_000; // 5 min - overview chart must not re-RPC constantly

function historyStorageKey(portfolioId: string) {
  return `opal:hist:${portfolioId}`;
}

function loadPersistedHistory(portfolioId: string): TxRow[] | null {
  try {
    const raw = sessionStorage.getItem(historyStorageKey(portfolioId));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as { at: number; rows: TxRow[] };
    if (!Array.isArray(parsed?.rows)) return null;
    // Persist up to 30 min for instant chart paint across remounts.
    // Allow empty arrays so we don't keep refetch-spinning forever.
    if (Date.now() - parsed.at > 30 * 60_000) return null;
    return parsed.rows;
  } catch {
    return null;
  }
}

/** Sync peek for instant chart/history paint (memory then sessionStorage). */
export function peekPortfolioHistory(portfolioId: string): TxRow[] | null {
  const hit = historyCache.get(portfolioId);
  if (hit && Date.now() - hit.at < HISTORY_CACHE_MS) return hit.rows;
  return loadPersistedHistory(portfolioId);
}

function persistHistory(portfolioId: string, rows: TxRow[]) {
  try {
    sessionStorage.setItem(
      historyStorageKey(portfolioId),
      JSON.stringify({ at: Date.now(), rows }),
    );
  } catch {
    /* ignore */
  }
}

async function portfolioHistoryCached(portfolioId: string): Promise<TxRow[]> {
  const hit = historyCache.get(portfolioId);
  if (hit && Date.now() - hit.at < HISTORY_CACHE_MS) {
    return hit.rows;
  }
  const disk = loadPersistedHistory(portfolioId);
  if (disk) {
    historyCache.set(portfolioId, { at: Date.now(), rows: disk });
    // Refresh in background without blocking the chart.
    void timedInvoke<TxRow[]>("portfolio_history", { portfolioId })
      .then((rows) => {
        historyCache.set(portfolioId, { at: Date.now(), rows });
        persistHistory(portfolioId, rows);
      })
      .catch(() => undefined);
    return disk;
  }
  const rows = await timedInvoke<TxRow[]>("portfolio_history", { portfolioId });
  historyCache.set(portfolioId, { at: Date.now(), rows });
  persistHistory(portfolioId, rows);
  return rows;
}

export function invalidatePortfolioHistory(portfolioId?: string) {
  if (portfolioId) {
    historyCache.delete(portfolioId);
    try {
      sessionStorage.removeItem(`opal:hist:${portfolioId}`);
    } catch {
      /* ignore */
    }
  } else {
    historyCache.clear();
    try {
      const keys: string[] = [];
      for (let i = 0; i < sessionStorage.length; i++) {
        const k = sessionStorage.key(i);
        if (k?.startsWith("opal:hist:")) keys.push(k);
      }
      for (const k of keys) sessionStorage.removeItem(k);
    } catch {
      /* ignore */
    }
  }
}

export const UTXO_CHAINS = new Set(["btc", "ltc", "doge"]);
export const RBF_CHAINS = new Set(["btc", "ltc"]);
export const EVM_CHAINS = new Set([
  "eth",
  "arb",
  "base",
  "polygon",
  "avax",
  "bsc",
  "gnosis",
  "linea",
]);

export function isUtxoChain(chain: string): boolean {
  return UTXO_CHAINS.has(chain);
}

export function canBumpFee(chain: string, kind: PortfolioKind): boolean {
  return kind === "software" && RBF_CHAINS.has(chain);
}

/** Chains that can broadcast a spend signed on Trezor from Opal. */
export function canTrezorSend(chain: string): boolean {
  const c = chain.toLowerCase();
  return (
    EVM_CHAINS.has(c) ||
    UTXO_CHAINS.has(c) ||
    c === "sol" ||
    c === "trx" ||
    c === "xmr"
  );
}

/** Whether Add Portfolio can auto-derive an address from a connected Trezor. */
export function trezorAutoVerifySupported(chain: string): boolean {
  const c = chain.toLowerCase();
  return (
    EVM_CHAINS.has(c) ||
    UTXO_CHAINS.has(c) ||
    c === "sol" ||
    c === "trx" ||
    c === "xmr"
  );
}

/** Chains offered when custody = Trezor (no Gram/TON - no Trezor messages). */
export function trezorSupportedChains(): string[] {
  return [
    "btc",
    "eth",
    "polygon",
    "bsc",
    "arb",
    "base",
    "ltc",
    "doge",
    "sol",
    "trx",
    "xmr",
  ];
}

export const api = {
  vaultStatus: () => timedInvoke<VaultStatus>("vault_status"),
  vaultCreate: (password: string, preset: SecurityPreset, wipeAfter10Failures: boolean) =>
    timedInvoke<VaultStatus>("vault_create", {
      request: { password, preset, wipeAfter10Failures },
    }),
  vaultUnlock: (password: string) =>
    timedInvoke<VaultStatus>("vault_unlock", { request: { password } }),
  vaultLock: () => timedInvoke<VaultStatus>("vault_lock"),
  vaultWipe: (password: string) =>
    timedInvoke<VaultStatus>("vault_wipe", { request: { password } }),
  getSettings: () => timedInvoke<AppSettings>("get_settings"),
  updateSettings: (patch: Record<string, unknown>) =>
    timedInvoke<AppSettings>("update_settings", { request: patch }),
  changePassword: (currentPassword: string, newPassword: string) =>
    timedInvoke<void>("change_password", { request: { currentPassword, newPassword } }),
  changeSecurityPreset: (password: string, preset: SecurityPreset) =>
    timedInvoke<AppSettings>("change_security_preset", { request: { password, preset } }),
  vaultPath: () => timedInvoke<string>("vault_path"),
  vaultExport: (password: string, destPath: string) =>
    timedInvoke<void>("vault_export", { request: { password, destPath } }),
  vaultImport: (password: string, srcPath: string) =>
    timedInvoke<VaultStatus>("vault_import", { request: { password, srcPath } }),
  writeTextFile: (destPath: string, contents: string) =>
    timedInvoke<void>("write_text_file", { request: { destPath, contents } }),
  setAutostart: (enabled: boolean) => timedInvoke<void>("set_autostart", { enabled }),
  appInfo: () => timedInvoke<AppInfo>("app_info"),

  chainList: () => timedInvoke<ChainInfo[]>("chain_list"),
  walletCreateSeed: (wordCount: 12 | 24) =>
    timedInvoke<string>("wallet_create_seed", { request: { wordCount } }),
  walletRestoreSeed: (mnemonic: string, passphrase?: string) =>
    timedInvoke<void>("wallet_restore_seed", { request: { mnemonic, passphrase } }),
  walletDiscoverPortfolios: () =>
    timedInvoke<PortfolioRecord[]>("wallet_discover_portfolios"),
  walletConfirmBackup: () => timedInvoke<void>("wallet_confirm_backup"),
  walletRevealSeed: () => timedInvoke<string>("wallet_reveal_seed"),

  portfolioList: () => timedInvoke<PortfolioRecord[]>("portfolio_list"),
  portfolioCreate: (request: CreatePortfolioRequest) =>
    timedInvoke<PortfolioRecord>("portfolio_create", { request }),
  portfolioRename: (id: string, name: string) =>
    timedInvoke<void>("portfolio_rename", { request: { id, name } }),
  portfolioDelete: (id: string) => timedInvoke<void>("portfolio_delete", { id }),
  portfolioReorder: (order: string[]) =>
    timedInvoke<PortfolioRecord[]>("portfolio_reorder", { request: { order } }),
  portfolioBalances: (portfolioId?: string) =>
    timedInvoke<PortfolioBalance[]>("portfolio_balances", { portfolioId: portfolioId ?? null }),
  portfolioBalancesCached: (portfolioId?: string) =>
    timedInvoke<PortfolioBalance[]>("portfolio_balances_cached", {
      portfolioId: portfolioId ?? null,
    }),
  portfolioReceiveAddress: (portfolioId: string) =>
    timedInvoke<string>("portfolio_receive_address", { portfolioId }),
  portfolioReceiveUri: (portfolioId: string, amount?: string) =>
    timedInvoke<string>("portfolio_receive_uri", {
      portfolioId,
      amount: amount ?? null,
    }),
  portfolioHistory: (portfolioId: string) => portfolioHistoryCached(portfolioId),
  portfolioSend: (request: SendRequest) =>
    timedInvoke<SendResult>("portfolio_send", { request }),
  portfolioEstimateFee: (portfolioId: string, feePreset?: FeePreset | string) =>
    timedInvoke<FeeEstimate>("portfolio_estimate_fee", {
      portfolioId,
      feePreset: feePreset ?? null,
    }),
  portfolioNextAddress: (portfolioId: string) =>
    timedInvoke<string>("portfolio_next_address", { portfolioId }),
  portfolioRescan: (portfolioId: string) =>
    timedInvoke<PortfolioBalance[]>("portfolio_rescan", { portfolioId }),
  portfolioBumpFee: (portfolioId: string, txid: string, feePreset?: FeePreset | string) =>
    timedInvoke<SendResult>("portfolio_bump_fee", {
      request: { portfolioId, txid, feePreset: feePreset ?? null },
    }),

  pricesFiat: () => timedInvoke<Record<string, number>>("prices_fiat"),
  warmSpotPrices: () =>
    timedInvoke<Record<string, Record<string, number>>>("warm_spot_prices"),
  spotPricesSnapshot: () =>
    timedInvoke<Record<string, Record<string, number>>>("spot_prices_snapshot"),
  priceHistory: (coinIds: string[], vsCurrency?: string, days?: number) =>
    timedInvoke<Record<string, Array<[number, number]>>>("price_history", {
      request: {
        coinIds,
        vsCurrency: vsCurrency ?? null,
        days: days ?? null,
      },
    }),
  trezorStatus: () => timedInvoke<TrezorStatus>("trezor_status"),
  trezorVerifyAddress: (
    chain: string,
    accountIndex?: number,
    addressType?: BtcAddressType | string,
  ) =>
    timedInvoke<string>("trezor_verify_address", {
      request: {
        chain,
        accountIndex: accountIndex ?? null,
        addressType: addressType ?? null,
      },
    }),
  trezorDiscoverPortfolios: (quiet = false) =>
    timedInvoke<PortfolioRecord[]>("trezor_discover_portfolios", {
      request: { quiet },
    }),
  trezorSyncXmrKeyImages: () => timedInvoke<number>("trezor_sync_xmr_key_images"),
  detectAddressChains: (address: string) =>
    timedInvoke<string[]>("detect_address_chains", { address }),

  addressBookList: () => timedInvoke<AddressBookEntry[]>("address_book_list"),
  addressBookAdd: (label: string, chain: string, address: string) =>
    timedInvoke<AddressBookEntry>("address_book_add", {
      request: { label, chain, address },
    }),
  addressBookRemove: (id: string) => timedInvoke<void>("address_book_remove", { id }),
  analyzeSendAddress: (to: string, chain?: string) =>
    timedInvoke<AddressSafety>("analyze_send_address", {
      request: { to, chain: chain ?? null },
    }),
  swapQuote: (
    provider: "jupiter" | "fixedfloat",
    fromAsset: string,
    toAsset: string,
    amount: string,
    fromChain?: string | null,
    toChain?: string | null,
  ) =>
    timedInvoke<SwapQuote>("swap_quote", {
      request: {
        provider,
        fromAsset,
        toAsset,
        amount,
        fromChain: fromChain ?? null,
        toChain: toChain ?? null,
      },
    }),
  swapJupiterTx: (quoteRaw: unknown, userPublicKey: string) =>
    timedInvoke<string>("swap_jupiter_tx", {
      request: { quoteRaw, userPublicKey },
    }),
  swapFixedfloatCreate: (request: {
    fromAsset: string;
    toAsset: string;
    fromChain: string;
    toChain: string;
    amount: string;
    toAddress: string;
  }) => timedInvoke<FixedFloatOrder>("swap_fixedfloat_create", { request }),
  swapFixedfloatOrder: (id: string, token: string) =>
    timedInvoke<FixedFloatOrder>("swap_fixedfloat_order", {
      request: { id, token },
    }),
  swapFixedfloatExecute: (request: {
    fromPortfolioId: string;
    toPortfolioId: string;
    fromAsset: string;
    toAsset: string;
    amount: string;
  }) => timedInvoke<FixedFloatExecuteResult>("swap_fixedfloat_execute", { request }),
  swapFixedfloatReady: () => timedInvoke<boolean>("swap_fixedfloat_ready"),
};

export interface FixedFloatExecuteResult {
  order: FixedFloatOrder;
  txid: string;
  explorerUrl: string;
}

export interface SwapQuote {
  provider: string;
  fromAsset: string;
  toAsset: string;
  fromAmount: string;
  toAmount: string;
  rate: string;
  minAmount?: string | null;
  maxAmount?: string | null;
  errors?: string[];
  raw: unknown;
}

export interface FixedFloatOrder {
  id: string;
  token: string;
  status: string;
  fromAmount: string;
  toAmount: string;
  depositAddress: string;
  depositTag?: string | null;
  toAddress: string;
  fromCcy: string;
  toCcy: string;
  orderUrl: string;
  raw: unknown;
}
