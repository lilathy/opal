import type { ChartPoint, LedgerEvent } from "./charts";

const LEDGER_KEY = "opal:overview-ledger:";
const SERIES_KEY = "opal:chart-series:v4:";
const PRICES_KEY = "opal:chart-prices:v3:";

/** In-memory + sessionStorage price series (Binance klines). */
export const chartsDataCache = new Map<string, Record<string, Array<[number, number]>>>();
/** Built chart paths. */
export const seriesCache = new Map<string, ChartPoint[]>();

const priceInflight = new Map<
  string,
  Promise<Record<string, Array<[number, number]>>>
>();

function ssGet<T>(key: string): T | null {
  try {
    const raw = sessionStorage.getItem(key);
    if (!raw) return null;
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

function ssSet(key: string, value: unknown) {
  try {
    sessionStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* quota / private mode */
  }
}

export function loadOverviewLedger(portfolioIdsKey: string): LedgerEvent[] | null {
  if (!portfolioIdsKey) return null;
  const rows = ssGet<LedgerEvent[]>(LEDGER_KEY + portfolioIdsKey);
  return Array.isArray(rows) && rows.length > 0 ? rows : null;
}

export function saveOverviewLedger(portfolioIdsKey: string, ledger: LedgerEvent[]) {
  if (!portfolioIdsKey || !ledger.length) return;
  ssSet(LEDGER_KEY + portfolioIdsKey, ledger);
}

/** Drop overview growth ledger so charts rebuild from fresh history. */
export function invalidateOverviewLedger(portfolioIdsKey?: string) {
  try {
    if (portfolioIdsKey) {
      sessionStorage.removeItem(LEDGER_KEY + portfolioIdsKey);
      return;
    }
    const keys: string[] = [];
    for (let i = 0; i < sessionStorage.length; i++) {
      const k = sessionStorage.key(i);
      if (k?.startsWith(LEDGER_KEY)) keys.push(k);
    }
    for (const k of keys) sessionStorage.removeItem(k);
  } catch {
    /* ignore */
  }
}

export function mergeLedgerEvents(a: LedgerEvent[], b: LedgerEvent[]): LedgerEvent[] {
  const key = (e: LedgerEvent) => `${e.t}|${e.coinId}|${e.delta}`;
  const map = new Map<string, LedgerEvent>();
  for (const e of a) map.set(key(e), e);
  for (const e of b) map.set(key(e), e);
  return [...map.values()].sort((x, y) => x.t - y.t);
}

export function hydrateChartsDataCache(priceKey: string): boolean {
  if (chartsDataCache.has(priceKey)) return true;
  const hit = ssGet<Record<string, Array<[number, number]>>>(PRICES_KEY + priceKey);
  if (hit && Object.keys(hit).length > 0) {
    chartsDataCache.set(priceKey, hit);
    return true;
  }
  return false;
}

export function persistChartsData(
  priceKey: string,
  charts: Record<string, Array<[number, number]>>,
) {
  if (!priceKey || !Object.keys(charts).length) return;
  chartsDataCache.set(priceKey, charts);
  ssSet(PRICES_KEY + priceKey, charts);
}

export function hydrateSeriesCache(shapeKey: string): ChartPoint[] | null {
  const mem = seriesCache.get(shapeKey);
  if (mem && mem.length >= 2) return mem;
  const hit = ssGet<ChartPoint[]>(SERIES_KEY + shapeKey);
  if (hit && hit.length >= 2) {
    seriesCache.set(shapeKey, hit);
    return hit;
  }
  return null;
}

export function persistSeries(shapeKey: string, points: ChartPoint[]) {
  if (!shapeKey || points.length < 2) return;
  seriesCache.set(shapeKey, points);
  ssSet(SERIES_KEY + shapeKey, points);
}

/** Deduped price-history fetch; warms memory + sessionStorage. */
export async function fetchPriceHistoryCached(
  fetchFn: (
    ids: string[],
    fiat: string,
    days: number,
  ) => Promise<Record<string, Array<[number, number]>>>,
  ids: string[],
  fiat: string,
  days: number,
): Promise<Record<string, Array<[number, number]>>> {
  const sorted = [...ids].filter(Boolean).sort();
  const priceKey = `${sorted.join(",")}|${fiat.toLowerCase()}|${days}`;
  if (hydrateChartsDataCache(priceKey)) {
    return chartsDataCache.get(priceKey)!;
  }
  const existing = priceInflight.get(priceKey);
  if (existing) return existing;

  const promise = fetchFn(sorted, fiat, days)
    .then((charts) => {
      persistChartsData(priceKey, charts);
      return charts;
    })
    .finally(() => {
      priceInflight.delete(priceKey);
    });
  priceInflight.set(priceKey, promise);
  return promise;
}
