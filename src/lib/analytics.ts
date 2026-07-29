import type { ChartPoint, LedgerEvent } from "./charts";
import { coinIdForSymbol } from "./charts";

export type HoldingFiat = {
  symbol: string;
  coinId: string;
  amount: number;
  fiatValue: number;
};

export type DayMove = {
  /** UTC day start ms */
  t: number;
  abs: number;
  pct: number;
};

export type AssetMove = {
  symbol: string;
  coinId: string;
  pct: number;
  abs: number;
};

/** Force the last point onto the live headline total. */
export function pinSeriesTip(
  points: ChartPoint[],
  liveTotal: number | undefined,
): ChartPoint[] {
  if (
    liveTotal == null ||
    !Number.isFinite(liveTotal) ||
    points.length === 0
  ) {
    return points;
  }
  const next = points.slice();
  const last = next[next.length - 1];
  if (Math.abs(last.v - liveTotal) < 1e-6) return points;
  next[next.length - 1] = { t: last.t, v: liveTotal };
  return next;
}

export function clipToPeriod(points: ChartPoint[], days: number): ChartPoint[] {
  if (!points.length || days <= 0) return points;
  const cutoff = Date.now() - days * 86_400_000;
  const clipped = points.filter((p) => p.t >= cutoff);
  return clipped.length >= 2 ? clipped : points;
}

export function symbolForCoinId(coinId: string): string {
  switch (coinId) {
    case "bitcoin":
      return "BTC";
    case "ethereum":
      return "ETH";
    case "solana":
      return "SOL";
    case "litecoin":
      return "LTC";
    case "dogecoin":
      return "DOGE";
    case "monero":
      return "XMR";
    case "usd-coin":
      return "USDC";
    case "tether":
      return "USDT";
    case "dai":
      return "DAI";
    case "matic-network":
      return "POL";
    case "avalanche-2":
      return "AVAX";
    case "binancecoin":
      return "BNB";
    case "tron":
      return "TRX";
    case "the-open-network":
      return "TON";
    case "xdai":
      return "GNO";
    default:
      return coinId.slice(0, 6).toUpperCase();
  }
}

/** Last close per UTC calendar day. */
export function dailyCloses(points: ChartPoint[]): ChartPoint[] {
  if (!points.length) return [];
  const byDay = new Map<string, ChartPoint>();
  for (const p of points) {
    const d = new Date(p.t);
    const key = `${d.getUTCFullYear()}-${d.getUTCMonth()}-${d.getUTCDate()}`;
    byDay.set(key, p);
  }
  return [...byDay.values()].sort((a, b) => a.t - b.t);
}

/** Best / worst day-over-day moves in a portfolio value series. */
export function bestAndWorstDays(points: ChartPoint[]): {
  best: DayMove | null;
  worst: DayMove | null;
} {
  const closes = dailyCloses(points);
  let best: DayMove | null = null;
  let worst: DayMove | null = null;
  for (let i = 1; i < closes.length; i++) {
    const prev = closes[i - 1].v;
    const cur = closes[i].v;
    if (!Number.isFinite(prev) || !Number.isFinite(cur)) continue;
    const abs = cur - prev;
    const pct = prev === 0 ? (cur === 0 ? 0 : 100) : (abs / prev) * 100;
    const move: DayMove = { t: closes[i].t, abs, pct };
    if (!best || abs > best.abs) best = move;
    if (!worst || abs < worst.abs) worst = move;
  }
  return { best, worst };
}

/** Top / bottom asset by period price % change (mark-to-market path for amount=1). */
export function rankAssetMoves(
  holdings: HoldingFiat[],
  charts: Record<string, Array<[number, number]>>,
  days: number,
): { top: AssetMove | null; bottom: AssetMove | null } {
  const cutoff = Date.now() - days * 86_400_000;
  let top: AssetMove | null = null;
  let bottom: AssetMove | null = null;

  for (const h of holdings) {
    if (h.amount <= 0 || h.fiatValue <= 0) continue;
    const series = charts[h.coinId];
    if (!series || series.length < 2) continue;
    const pts = series
      .map(([ts, price]) => ({
        t: ts > 0 && ts < 1e12 ? ts * 1000 : ts,
        v: price,
      }))
      .filter((p) => p.t >= cutoff && Number.isFinite(p.v));
    if (pts.length < 2) continue;
    const first = pts[0].v;
    const last = pts[pts.length - 1].v;
    if (!Number.isFinite(first) || first === 0) continue;
    const pct = ((last - first) / first) * 100;
    const abs = (last - first) * h.amount;
    const move: AssetMove = { symbol: h.symbol, coinId: h.coinId, pct, abs };
    if (!top || pct > top.pct) top = move;
    if (!bottom || pct < bottom.pct) bottom = move;
  }
  return { top, bottom };
}

/** Approx net fiat flow from ledger using current unit values. */
export function approxNetFlow(
  ledger: LedgerEvent[],
  holdings: HoldingFiat[],
  days: number,
): number {
  const cutoff = Date.now() - days * 86_400_000;
  const unit = new Map<string, number>();
  for (const h of holdings) {
    if (h.amount > 0) unit.set(h.coinId, h.fiatValue / h.amount);
  }
  let sum = 0;
  for (const e of ledger) {
    if (e.t < cutoff) continue;
    const px = unit.get(e.coinId);
    if (px == null || !Number.isFinite(px)) continue;
    sum += e.delta * px;
  }
  return sum;
}

export function allocationSlices(
  holdings: HoldingFiat[],
  total: number,
): Array<{ label: string; pct: number; fiatValue: number; coinId: string }> {
  if (total <= 0) return [];
  const ranked = [...holdings]
    .filter((h) => h.fiatValue > 0)
    .sort((a, b) => b.fiatValue - a.fiatValue);
  if (!ranked.length) return [];

  const top = ranked.slice(0, 3);
  const rest = ranked.slice(3);
  const slices = top.map((h) => ({
    label: h.symbol,
    pct: (h.fiatValue / total) * 100,
    fiatValue: h.fiatValue,
    coinId: h.coinId,
  }));
  if (rest.length) {
    const otherVal = rest.reduce((s, h) => s + h.fiatValue, 0);
    slices.push({
      label: "Other",
      pct: (otherVal / total) * 100,
      fiatValue: otherVal,
      coinId: "other",
    });
  }
  // Normalize rounding drift
  const sumPct = slices.reduce((s, x) => s + x.pct, 0);
  if (slices.length && Math.abs(sumPct - 100) > 0.05) {
    slices[slices.length - 1].pct += 100 - sumPct;
  }
  return slices;
}

export function downsampleSpark(points: ChartPoint[], maxPoints = 24): number[] {
  if (!points.length) return [];
  if (points.length <= maxPoints) return points.map((p) => p.v);
  const out: number[] = [];
  const step = (points.length - 1) / (maxPoints - 1);
  for (let i = 0; i < maxPoints; i++) {
    const idx = Math.round(i * step);
    out.push(points[idx].v);
  }
  return out;
}

export function resolveCoinId(symbol: string): string | null {
  return coinIdForSymbol(symbol);
}
