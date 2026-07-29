import type { AssetBalance, PortfolioBalance } from "./api";
import { coinIdForSymbol } from "./charts";

const fiatPriceCache = new Map<string, Record<string, number>>();
const fiatBtcRates = new Map<string, number>();
let priceCacheVersion = 0;
const listeners = new Set<() => void>();

function normFiat(fiat: string): string {
  return fiat.trim().toUpperCase();
}

function bumpPriceCache() {
  priceCacheVersion += 1;
  for (const fn of listeners) fn();
}

export function subscribePriceCache(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function getPriceCacheVersion(): number {
  return priceCacheVersion;
}

/** Guaranteed array - cached JSON / partial RPC payloads sometimes omit it. */
export function assetsOf(bal: PortfolioBalance | null | undefined): AssetBalance[] {
  const a = bal?.assets;
  return Array.isArray(a) ? a : [];
}

/** Normalize a balance row so `.assets` is always iterable. */
export function normalizeBalance(bal: PortfolioBalance): PortfolioBalance {
  return {
    ...bal,
    assets: assetsOf(bal),
  };
}

export function normalizeBalances(list: PortfolioBalance[]): PortfolioBalance[] {
  return list.filter(Boolean).map(normalizeBalance);
}

/** Store spot prices for a fiat code (USD, EUR, …). */
export function cacheFiatPrices(fiat: string, prices: Record<string, number>) {
  if (!prices || Object.keys(prices).length === 0) return;
  fiatPriceCache.set(normFiat(fiat), prices);
  if (prices.bitcoin != null && Number.isFinite(prices.bitcoin)) {
    fiatBtcRates.set(normFiat(fiat), prices.bitcoin);
  }
  bumpPriceCache();
}

/** Bulk-import from backend warm_spot_prices / spot_prices_snapshot. */
export function cacheFiatPriceMatrix(matrix: Record<string, Record<string, number>>) {
  for (const [fiat, prices] of Object.entries(matrix)) {
    cacheFiatPrices(fiat, prices);
  }
}

function scalePriceMap(
  prices: Record<string, number>,
  ratio: number,
): Record<string, number> {
  const out: Record<string, number> = {};
  for (const [k, v] of Object.entries(prices)) {
    if (Number.isFinite(v)) out[k] = v * ratio;
  }
  return out;
}

/**
 * Resolve spot prices for a fiat - direct cache, or synthesize from any cached
 * fiat via BTC cross-rate so first switch never waits on network.
 */
export function resolvePricesForFiat(fiat: string): Record<string, number> {
  const target = normFiat(fiat);
  const direct = fiatPriceCache.get(target);
  if (direct && Object.keys(direct).length > 0) return direct;

  const targetBtc = fiatBtcRates.get(target);
  if (targetBtc != null && Number.isFinite(targetBtc)) {
    for (const [cachedFiat, prices] of fiatPriceCache) {
      const baseBtc = fiatBtcRates.get(cachedFiat) ?? prices.bitcoin;
      if (!baseBtc || !Number.isFinite(baseBtc)) continue;
      const ratio = targetBtc / baseBtc;
      if (!Number.isFinite(ratio) || ratio <= 0) continue;
      return scalePriceMap(prices, ratio);
    }
  }

  // Last resort: pivot through USD if we only know USD coin map + target BTC rate
  const usd = fiatPriceCache.get("USD");
  const usdBtc = fiatBtcRates.get("USD") ?? usd?.bitcoin;
  if (usd && usdBtc && targetBtc != null && Number.isFinite(targetBtc)) {
    return scalePriceMap(usd, targetBtc / usdBtc);
  }

  return {};
}

/** Fiat value for one asset row in the selected display currency. */
export function assetFiatValue(
  asset: AssetBalance,
  fiat: string,
  prices?: Record<string, number>,
): number {
  const px = prices ?? resolvePricesForFiat(fiat);
  const coinId = coinIdForSymbol(asset.symbol);
  const amt = Number(asset.amount);
  if (coinId && px[coinId] != null && Number.isFinite(amt)) {
    return amt * px[coinId];
  }
  return asset.usd ?? 0;
}

export function portfolioFiatSum(
  bal: PortfolioBalance | undefined,
  fiat: string,
  prices?: Record<string, number>,
): number {
  if (!bal) return 0;
  const px = prices ?? resolvePricesForFiat(fiat);
  let sum = 0;
  for (const a of assetsOf(bal)) {
    sum += assetFiatValue(a, fiat, px);
  }
  return sum;
}

function nativeSum(bal: PortfolioBalance): number {
  let sum = 0;
  for (const a of assetsOf(bal)) {
    const n = Number(a.amount);
    if (Number.isFinite(n)) sum += n;
  }
  return sum;
}

function fiatSum(bal: PortfolioBalance): number {
  let sum = 0;
  for (const a of assetsOf(bal)) {
    if (a.usd != null && Number.isFinite(a.usd)) sum += a.usd;
  }
  return sum;
}

/** Prefer the balance snapshot that reflects more on-chain value. */
export function balanceIsRicher(a: PortfolioBalance, b: PortfolioBalance): boolean {
  const amtA = nativeSum(a);
  const amtB = nativeSum(b);
  if (amtB > amtA + 1e-12) return true;
  if (amtA > amtB + 1e-12) return false;
  return fiatSum(b) > fiatSum(a);
}

/** Merge balance rows - never let stale zeros overwrite a fresher non-zero row. */
export function mergeBalances(
  prev: PortfolioBalance[],
  incoming: PortfolioBalance[],
): PortfolioBalance[] {
  const map = new Map<string, PortfolioBalance>();
  for (const b of prev) map.set(b.portfolio_id, normalizeBalance(b));
  for (const b of normalizeBalances(incoming)) {
    const existing = map.get(b.portfolio_id);
    const incomingBal = reconcilePendingSpend(b);
    if (hasPendingSpend(b.portfolio_id)) {
      // After a send, never let a richer cached/stale row win.
      map.set(
        b.portfolio_id,
        existing ? preferLowerNative(existing, incomingBal) : incomingBal,
      );
      continue;
    }
    if (!existing || balanceIsRicher(existing, incomingBal)) {
      map.set(b.portfolio_id, incomingBal);
    }
  }
  return [...map.values()];
}

/** Always apply a live scrape - amounts can go up or down. */
export function applyLiveBalances(
  prev: PortfolioBalance[],
  incoming: PortfolioBalance[],
): PortfolioBalance[] {
  const clean = normalizeBalances(incoming);
  if (!clean.length) return prev;
  const map = new Map<string, PortfolioBalance>();
  for (const b of prev) map.set(b.portfolio_id, normalizeBalance(b));
  for (const b of clean) {
    const existing = map.get(b.portfolio_id);
    // Failed / timed-out scrapes often arrive as empty assets OR a single
    // native row at "0". Never let those wipe a known non-zero balance -
    // except after an optimistic spend (user intentionally emptied).
    if (
      existing &&
      nativeSum(b) <= 1e-12 &&
      nativeSum(existing) > 1e-12 &&
      !hasPendingSpend(b.portfolio_id)
    ) {
      continue;
    }
    const incomingBal = reconcilePendingSpend(b);
    if (hasPendingSpend(b.portfolio_id) && existing) {
      map.set(b.portfolio_id, preferLowerNative(existing, incomingBal));
    } else {
      map.set(b.portfolio_id, incomingBal);
    }
  }
  return [...map.values()];
}

/** Subtract a spend from a local balance row (instant UI before RPC catches up). */
export function applyOptimisticSpend(
  bal: PortfolioBalance,
  opts: {
    /** Asset symbol being sent (e.g. SOL, USDC). */
    assetSymbol: string;
    amount: number;
    /** Native-chain fee in native units (SOL/BTC/ETH…), if known. */
    feeNative?: number;
    /** Native symbol for fee deduction (e.g. SOL). */
    nativeSymbol: string;
  },
): PortfolioBalance {
  const amount = Number(opts.amount);
  const fee = Number(opts.feeNative ?? 0);
  if (!Number.isFinite(amount) || amount <= 0) return bal;

  const assetSym = opts.assetSymbol.toUpperCase();
  const nativeSym = opts.nativeSymbol.toUpperCase();
  const sameAsset = assetSym === nativeSym;

  return {
    ...bal,
    assets: assetsOf(bal).map((a) => {
      const sym = a.symbol.toUpperCase();
      let debit = 0;
      if (sym === assetSym) debit += amount;
      if (fee > 0 && sym === nativeSym && !sameAsset) debit += fee;
      if (fee > 0 && sameAsset && sym === assetSym) debit += fee;
      if (debit <= 0) return a;

      const cur = Number(a.amount);
      if (!Number.isFinite(cur)) return a;
      const next = Math.max(0, cur - debit);
      const usd =
        a.usd != null && Number.isFinite(a.usd) && cur > 0
          ? a.usd * (next / cur)
          : a.usd;
      // Keep decimal string style close to what scrapes return.
      const decimals = a.decimals ?? 8;
      const fixed = next.toFixed(Math.min(decimals, 12)).replace(/\.?0+$/, "");
      return { ...a, amount: fixed || "0", usd };
    }),
  };
}

type PendingSpend = {
  portfolioId: string;
  /** Upper bound per asset while the chain scrape may still be stale. */
  ceilings: Record<string, number>;
  at: number;
};

const PENDING_SPENDS: PendingSpend[] = [];
const PENDING_TTL_MS = 180_000;

function prunePendingSpends(now = Date.now()) {
  for (let i = PENDING_SPENDS.length - 1; i >= 0; i--) {
    if (now - PENDING_SPENDS[i].at > PENDING_TTL_MS) PENDING_SPENDS.splice(i, 1);
  }
}

/** Remember an optimistic debit so stale scrapes can't restore the pre-send balance. */
export function rememberOptimisticSpend(bal: PortfolioBalance): void {
  prunePendingSpends();
  const ceilings: Record<string, number> = {};
  for (const a of assetsOf(bal)) {
    const n = Number(a.amount);
    if (Number.isFinite(n)) ceilings[a.symbol.toUpperCase()] = n;
  }
  const portfolioId = bal.portfolio_id;
  // Replace any prior pending for this portfolio.
  for (let i = PENDING_SPENDS.length - 1; i >= 0; i--) {
    if (PENDING_SPENDS[i].portfolioId === portfolioId) PENDING_SPENDS.splice(i, 1);
  }
  PENDING_SPENDS.push({
    portfolioId,
    ceilings,
    at: Date.now(),
  });
}

export function hasPendingSpend(portfolioId: string): boolean {
  prunePendingSpends();
  return PENDING_SPENDS.some((p) => p.portfolioId === portfolioId);
}

/** Cap used by the incoming watcher so a stale scrape can't look like a receive. */
export function pendingAmountCeiling(
  portfolioId: string,
  symbol: string,
): number | null {
  prunePendingSpends();
  const pending = PENDING_SPENDS.find((p) => p.portfolioId === portfolioId);
  if (!pending) return null;
  const ceiling = pending.ceilings[symbol.toUpperCase()];
  return ceiling != null && Number.isFinite(ceiling) ? ceiling : null;
}

function preferLowerNative(a: PortfolioBalance, b: PortfolioBalance): PortfolioBalance {
  return nativeSum(a) <= nativeSum(b) + 1e-12 ? a : b;
}

/**
 * Clamp a scraped balance so it cannot jump back above a recent optimistic
 * spend. Once the chain is at or below the ceiling, the pending entry clears.
 */
export function reconcilePendingSpend(bal: PortfolioBalance): PortfolioBalance {
  prunePendingSpends();
  const idx = PENDING_SPENDS.findIndex((p) => p.portfolioId === bal.portfolio_id);
  if (idx < 0) return bal;
  const pending = PENDING_SPENDS[idx];

  let stillPending = false;
  const assets = assetsOf(bal).map((a) => {
    const sym = a.symbol.toUpperCase();
    const ceiling = pending.ceilings[sym];
    if (ceiling == null || !Number.isFinite(ceiling)) return a;
    const live = Number(a.amount);
    if (!Number.isFinite(live)) return a;
    // Chain already at/below optimistic → keep guarding while equal so a
    // later stale scrape can't restore the pre-send balance. Only drop the
    // guard once the scrape is strictly below (fees, dust) or TTL expires.
    if (live <= ceiling + 1e-9) {
      if (live >= ceiling - 1e-9) stillPending = true;
      return a;
    }
    stillPending = true;
    const cur = live;
    const next = Math.max(0, ceiling);
    const usd =
      a.usd != null && Number.isFinite(a.usd) && cur > 0 ? a.usd * (next / cur) : a.usd;
    const decimals = a.decimals ?? 8;
    const fixed = next.toFixed(Math.min(decimals, 12)).replace(/\.?0+$/, "");
    return { ...a, amount: fixed || "0", usd };
  });

  if (!stillPending) PENDING_SPENDS.splice(idx, 1);
  return { ...bal, assets };
}

