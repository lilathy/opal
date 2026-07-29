import type { TxRow } from "./api";

export type ChartPoint = { t: number; v: number };

/** A signed balance change derived from a wallet transaction. */
export type LedgerEvent = {
  /** Unix ms */
  t: number;
  coinId: string;
  /** +receive / −spend, in the asset's native units */
  delta: number;
};

/** Map asset / chain symbols to CoinGecko ids. */
export function coinIdForSymbol(symbol: string): string | null {
  switch (symbol.trim().toUpperCase()) {
    case "BTC":
      return "bitcoin";
    case "ETH":
      return "ethereum";
    case "SOL":
      return "solana";
    case "LTC":
      return "litecoin";
    case "DOGE":
      return "dogecoin";
    case "XMR":
      return "monero";
    case "USDC":
      return "usd-coin";
    case "USDT":
      return "tether";
    case "DAI":
      return "dai";
    case "ARB":
    case "BASE":
    case "OP":
    case "LINEA":
      return "ethereum";
    case "POL":
    case "MATIC":
    case "POLYGON":
      return "matic-network";
    case "AVAX":
      return "avalanche-2";
    case "BNB":
    case "BSC":
      return "binancecoin";
    case "XDAI":
    case "GNO":
    case "GNOSIS":
      return "xdai";
    case "TRX":
    case "TRON":
      return "tron";
    case "TON":
      return "the-open-network";
    default:
      return null;
  }
}

export function coinIdForChain(chain: string): string | null {
  return coinIdForSymbol(chain);
}

/** Parse explorer/RPC timestamps which arrive as unix sec, unix ms, or ISO. */
export function parseTxTimestamp(ts: string): number | null {
  const s = ts.trim();
  if (!s) return null;
  // Pure integer unix (sec or ms).
  if (/^\d{9,12}$/.test(s)) {
    const n = Number(s);
    return Number.isFinite(n) && n > 0 ? n * 1000 : null;
  }
  if (/^\d{13,16}$/.test(s)) {
    const n = Number(s);
    return Number.isFinite(n) && n > 0 ? n : null;
  }
  // Fractional unix seconds from some explorers ("1712345678.0").
  if (/^\d{9,12}(\.\d+)?$/.test(s)) {
    const n = Number(s);
    return Number.isFinite(n) && n > 0 ? Math.floor(n * 1000) : null;
  }
  const d = Date.parse(s);
  return Number.isFinite(d) && d > 0 ? d : null;
}

/**
 * Turn wallet history into signed ledger events. Failed txs are skipped;
 * self-transfers don't change net holdings (fees on self are ignored - many
 * explorers already bake fee into the reported amount for spends).
 */
export function txsToLedger(txs: TxRow[]): LedgerEvent[] {
  const out: LedgerEvent[] = [];
  for (const tx of txs) {
    if (tx.status === "failed") continue;
    const t = parseTxTimestamp(tx.timestamp);
    if (t == null) continue;
    const coinId = coinIdForSymbol(tx.symbol);
    if (!coinId) continue;
    const amt = Number(tx.amount);
    if (!Number.isFinite(amt) || amt <= 0) continue;
    const dir = tx.direction.toLowerCase();
    if (dir === "in") {
      out.push({ t, coinId, delta: amt });
    } else if (dir === "out") {
      out.push({ t, coinId, delta: -amt });
    }
  }
  return out;
}

/**
 * Holding amount of `coinId` at time `atMs`, reconstructed from today's
 * balance by undoing every ledger event that happened after `atMs`.
 *
 * Example: received $1 eighteen minutes ago → undoing that receive yields 0
 * before it (not "held for a week").
 *
 * Important: explorers only return a recent window. If the funding receive is
 * older than that window, the residual after undoing known txs is still held
 * balance from before our history - keep it. Zeroing at the oldest *fetched*
 * tx made restored portfolios look like they received funds "today".
 *
 * If we have no ledger events for this coin, we refuse to invent history -
 * returning 0 instead of projecting today's balance across the whole chart.
 */
function amountAt(
  current: number,
  events: LedgerEvent[],
  coinId: string,
  atMs: number,
): number {
  const mine = events.filter((e) => e.coinId === coinId);
  // No known txs for this asset → cannot reconstruct growth. Never fall back
  // to mark-to-market (current × historical price).
  if (!mine.length) return 0;

  let amt = current;
  for (const e of mine) {
    if (e.t > atMs) amt -= e.delta;
  }
  // Clamp tiny float noise; never paint a negative balance from rounding.
  if (!Number.isFinite(amt) || Math.abs(amt) < 1e-12) return 0;
  return amt > 0 ? amt : 0;
}

/**
 * Portfolio fiat series from *real* wallet history + spot prices.
 *
 * At each price timestamp T: Σ (balance_at(T, coin) × price(T, coin)).
 * Complete history naturally zeros before the first receive (undo the receive).
 * Incomplete history keeps the unexplained residual so long-held funds don't
 * appear as a deposit on the day of the oldest fetched tx.
 *
 * Returns [] when history is missing so the UI can keep loading instead of
 * painting a fake price×balance curve.
 */
export function buildBalanceHistorySeries(
  holdings: Array<{ coinId: string; amount: number }>,
  ledger: LedgerEvent[],
  charts: Record<string, Array<[number, number]>>,
): ChartPoint[] {
  if (!holdings.length) return [];

  const currentByCoin = new Map<string, number>();
  for (const h of holdings) {
    if (h.amount > 0) {
      currentByCoin.set(h.coinId, (currentByCoin.get(h.coinId) ?? 0) + h.amount);
    }
  }
  if (!currentByCoin.size && !ledger.length) return [];

  // Held assets with no ledger events → cannot reconstruct that coin.
  // Drop them from the series instead of painting 0 for the whole window
  // and then tip-patching today's balance (classic Trezor/overview spike).
  const heldWithoutHistory = [...currentByCoin.keys()].filter(
    (id) => !ledger.some((e) => e.coinId === id),
  );
  for (const id of heldWithoutHistory) {
    currentByCoin.delete(id);
  }
  if (!currentByCoin.size) {
    return [];
  }

  let base: Array<[number, number]> | null = null;
  for (const h of holdings) {
    const s = charts[h.coinId];
    if (s?.length && (!base || s.length > base.length)) base = s;
  }
  // Prefer a price axis that covers ledger activity even if a spent coin
  // is no longer in holdings.
  for (const e of ledger) {
    const s = charts[e.coinId];
    if (s?.length && (!base || s.length > base.length)) base = s;
  }
  if (!base?.length) return [];

  const coinIds = new Set<string>([...currentByCoin.keys()]);
  for (const e of ledger) coinIds.add(e.coinId);

  const points: ChartPoint[] = [];
  for (const [rawTs] of base) {
    const ts = toMs(rawTs);

    let total = 0;
    let any = false;
    for (const coinId of coinIds) {
      const series = charts[coinId];
      if (!series?.length) continue;
      const price = nearestPrice(series, ts);
      if (price == null) continue;
      const amt = amountAt(currentByCoin.get(coinId) ?? 0, ledger, coinId, ts);
      total += amt * price;
      any = true;
    }
    if (any) points.push({ t: ts, v: total });
  }

  // Drop a leading all-zero run when the wallet truly had nothing yet
  // (complete history). Keep points when residual pre-window holdings exist.
  while (points.length > 1 && Math.abs(points[0].v) < 1e-9) {
    points.shift();
  }
  return points;
}

/**
 * Pure mark-to-market: today's amount × historical spot (asset price chart).
 * Used for the per-asset "price" mode where we intentionally show the coin's
 * market price path, not wallet balance over time.
 */
export function buildPortfolioSeries(
  holdings: Array<{ coinId: string; amount: number }>,
  charts: Record<string, Array<[number, number]>>,
): ChartPoint[] {
  if (!holdings.length) return [];

  let base: Array<[number, number]> | null = null;
  for (const h of holdings) {
    const s = charts[h.coinId];
    if (s?.length && (!base || s.length > base.length)) base = s;
  }
  if (!base?.length) return [];

  const points: ChartPoint[] = [];
  for (const [rawTs] of base) {
    const ts = toMs(rawTs);
    let total = 0;
    let any = false;
    for (const h of holdings) {
      const series = charts[h.coinId];
      if (!series?.length || h.amount <= 0) continue;
      const price = nearestPrice(series, ts);
      if (price == null) continue;
      total += h.amount * price;
      any = true;
    }
    if (any) points.push({ t: ts, v: total });
  }
  return points;
}

/**
 * CoinGecko series from our Rust layer are unix *seconds*; wallet ledger
 * events are unix *ms*. Normalize everything to ms before comparing.
 */
function toMs(ts: number): number {
  return ts > 0 && ts < 1e12 ? ts * 1000 : ts;
}

function nearestPrice(series: Array<[number, number]>, tsMs: number): number | null {
  let lo = 0;
  let hi = series.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const t = toMs(series[mid][0]);
    if (t === tsMs) return series[mid][1];
    if (t < tsMs) lo = mid + 1;
    else hi = mid - 1;
  }
  const a = series[Math.max(0, hi)];
  const b = series[Math.min(series.length - 1, lo)];
  if (!a) return b?.[1] ?? null;
  if (!b) return a[1];
  const aT = toMs(a[0]);
  const bT = toMs(b[0]);
  return Math.abs(aT - tsMs) <= Math.abs(bT - tsMs) ? a[1] : b[1];
}

export function seriesChangePct(points: ChartPoint[]): number | null {
  if (points.length < 2) return null;
  const first = points[0].v;
  const last = points[points.length - 1].v;
  if (!Number.isFinite(first) || !Number.isFinite(last)) return null;
  if (first === 0) return last === 0 ? 0 : 100;
  if (Math.abs(last - first) < 1e-9) return 0;
  return ((last - first) / first) * 100;
}

/** Absolute fiat delta for the same window as `seriesChangePct` (last − first). */
export function seriesChangeAbs(points: ChartPoint[]): number | null {
  if (points.length < 2) return null;
  const first = points[0].v;
  const last = points[points.length - 1].v;
  if (!Number.isFinite(first) || !Number.isFinite(last)) return null;
  return last - first;
}

/** Catmull-Rom → cubic bezier SVG path for a smooth area chart. */
export function smoothPath(
  points: Array<{ x: number; y: number }>,
  closeY: number,
): { line: string; area: string } {
  if (!points.length) return { line: "", area: "" };
  if (points.length === 1) {
    const p = points[0];
    const line = `M ${p.x} ${p.y}`;
    const area = `${line} L ${p.x} ${closeY} L ${p.x} ${closeY} Z`;
    return { line, area };
  }

  let line = `M ${points[0].x} ${points[0].y}`;
  for (let i = 0; i < points.length - 1; i++) {
    const { cp1x, cp1y, cp2x, cp2y, p2 } = segmentControls(points, i);
    line += ` C ${cp1x} ${cp1y}, ${cp2x} ${cp2y}, ${p2.x} ${p2.y}`;
  }
  const first = points[0];
  const last = points[points.length - 1];
  const area = `${line} L ${last.x} ${closeY} L ${first.x} ${closeY} Z`;
  return { line, area };
}

function segmentControls(points: Array<{ x: number; y: number }>, i: number) {
  const p0 = points[Math.max(0, i - 1)];
  const p1 = points[i];
  const p2 = points[i + 1];
  const p3 = points[Math.min(points.length - 1, i + 2)];
  return {
    p1,
    p2,
    cp1x: p1.x + (p2.x - p0.x) / 6,
    cp1y: p1.y + (p2.y - p0.y) / 6,
    cp2x: p2.x - (p3.x - p1.x) / 6,
    cp2y: p2.y - (p3.y - p1.y) / 6,
  };
}

/**
 * Y on the same cubic path `smoothPath` draws, at a given X.
 * Linear chords float above dips - this keeps the hover dot glued to the curve.
 */
export function smoothYAtX(
  points: Array<{ x: number; y: number }>,
  x: number,
): number | null {
  if (!points.length) return null;
  if (points.length === 1) return points[0].y;
  const clamped = Math.min(Math.max(x, points[0].x), points[points.length - 1].x);
  let i = 0;
  while (i < points.length - 2 && points[i + 1].x < clamped) i++;
  const { p1, p2, cp1x, cp1y, cp2x, cp2y } = segmentControls(points, i);

  const bx = (t: number) => {
    const u = 1 - t;
    return u * u * u * p1.x + 3 * u * u * t * cp1x + 3 * u * t * t * cp2x + t * t * t * p2.x;
  };
  const by = (t: number) => {
    const u = 1 - t;
    return u * u * u * p1.y + 3 * u * u * t * cp1y + 3 * u * t * t * cp2y + t * t * t * p2.y;
  };

  let lo = 0;
  let hiT = 1;
  for (let k = 0; k < 28; k++) {
    const mid = (lo + hiT) / 2;
    if (bx(mid) < clamped) lo = mid;
    else hiT = mid;
  }
  return by((lo + hiT) / 2);
}
