import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../lib/api";
import {
  buildBalanceHistorySeries,
  buildPortfolioSeries,
  seriesChangeAbs,
  seriesChangePct,
  type ChartPoint,
  type LedgerEvent,
} from "../lib/charts";
import {
  chartsDataCache,
  fetchPriceHistoryCached,
  hydrateChartsDataCache,
  hydrateSeriesCache,
  persistSeries,
  seriesCache,
} from "../lib/chartCache";
import { formatMoney } from "../lib/format";
import { AreaChart } from "./AreaChart";

export type ChartHolding = { coinId: string; amount: number };

type Period = "1" | "7" | "30" | "90" | "365";

const PERIOD_STORAGE_KEY = "opal:chart-period";

const PERIODS: { id: Period; labelKey: string; label: string }[] = [
  { id: "1", labelKey: "chart.1d", label: "1D" },
  { id: "7", labelKey: "chart.1w", label: "1W" },
  { id: "30", labelKey: "chart.1m", label: "1M" },
  { id: "90", labelKey: "chart.3m", label: "3M" },
  { id: "365", labelKey: "chart.1y", label: "1Y" },
];

function loadSavedPeriod(): Period {
  try {
    const v = localStorage.getItem(PERIOD_STORAGE_KEY);
    if (v === "1" || v === "7" || v === "30" || v === "90" || v === "365") return v;
  } catch {
    /* private mode */
  }
  return "7";
}

function savePeriod(period: Period) {
  try {
    localStorage.setItem(PERIOD_STORAGE_KEY, period);
  } catch {
    /* private mode */
  }
}

type Props = {
  holdings: ChartHolding[];
  fiat: string;
  discreet?: boolean;
  height?: number;
  className?: string;
  /**
   * Balance-history mode:
   * - `undefined` → pure asset-price chart
   * - `null` → wallet history still loading (show loading — never a fake series)
   * - `LedgerEvent[]` → reconstruct from real txs
   */
  ledger?: LedgerEvent[] | null;
  /**
   * Live portfolio fiat total (same source as the hero number). When set, the
   * chart tip is pinned to this value so CoinGecko candle lag can't disagree
   * with the balance headline.
   */
  liveTotal?: number;
  leadingControl?: ReactNode;
};

/** Force the last point onto the live headline total. */
function pinSeriesTip(points: ChartPoint[], liveTotal: number | undefined): ChartPoint[] {
  if (
    liveTotal == null ||
    !Number.isFinite(liveTotal) ||
    liveTotal < 0 ||
    points.length < 1
  ) {
    return points;
  }
  const out = points.slice();
  const last = out[out.length - 1];
  if (Math.abs(last.v - liveTotal) < 1e-9) return points;
  out[out.length - 1] = { t: last.t, v: liveTotal };
  return out;
}

function clipToPeriod(points: ChartPoint[], days: number): ChartPoint[] {
  if (!points.length) return points;
  const normalized = points.map((p) => ({ ...p, t: toChartMs(p.t) }));
  const windowMs = days * 86_400_000;
  const now = Date.now();
  const cutoff = now - windowMs;
  const clipped = normalized.filter((p) => p.t >= cutoff);
  if (clipped.length >= 2) return clipped;
  // Stale feed (e.g. delisted Binance XMR ending Feb 2024): never paint an
  // ancient tail as if it were the selected period.
  const endT = normalized[normalized.length - 1].t;
  if (now - endT > 2 * 86_400_000) return [];
  const tail = normalized.filter((p) => p.t >= endT - windowMs);
  if (tail.length >= 2) return tail;
  return normalized.slice(-Math.min(normalized.length, 96));
}

function toChartMs(t: number): number {
  return t > 0 && t < 1e12 ? t * 1000 : t;
}

function chartsCacheKey(ids: string[], fiat: string, days: number): string {
  return `${[...ids].sort().join(",")}|${fiat.toLowerCase()}|${days}`;
}

/** Shape key — period/fiat/coins/ledger. Amounts are patched on top. */
function seriesShapeKey(
  coinKey: string,
  fiat: string,
  period: string,
  ledgerKey: string,
): string {
  return `${coinKey}|${fiat}|${period}|${ledgerKey}`;
}

/**
 * When only the live balance changed, update the last point instead of
 * rebuilding the whole path from the exchange series.
 */
function patchSeriesTip(
  prev: ChartPoint[],
  holdings: ChartHolding[],
  ledger: LedgerEvent[] | null | undefined,
  charts: Record<string, Array<[number, number]>>,
  useHistory: boolean,
): ChartPoint[] | null {
  if (prev.length < 2) return null;
  // Growth tip-patch only when every held coin has ledger coverage.
  if (useHistory && Array.isArray(ledger)) {
    const held = holdings.filter((h) => h.amount > 0);
    if (
      held.some((h) => !ledger.some((e) => e.coinId === h.coinId)) &&
      held.length > 0
    ) {
      return null;
    }
  }
  const raw = useHistory && Array.isArray(ledger)
    ? buildBalanceHistorySeries(holdings, ledger, charts)
    : buildPortfolioSeries(holdings, charts);
  if (raw.length < 2) return null;
  const nextTip = raw[raw.length - 1];
  const out = prev.slice();
  const last = out[out.length - 1];
  // Reject tip jumps that look like incomplete-history spikes (near-zero
  // path then full balance) — only in growth mode.
  if (useHistory) {
    const median = (() => {
      const vals = out.slice(0, -1).map((p) => p.v).sort((a, b) => a - b);
      if (!vals.length) return last.v;
      return vals[Math.floor(vals.length / 2)] ?? last.v;
    })();
    if (
      median >= 0 &&
      nextTip.v > median * 4 + 1 &&
      median < nextTip.v * 0.25
    ) {
      return null;
    }
  }
  // Same timeframe tip → just rewrite the value (balance moved).
  if (Math.abs(toChartMs(last.t) - toChartMs(nextTip.t)) < 120_000) {
    out[out.length - 1] = { t: last.t, v: nextTip.v };
    return out;
  }
  // New tip beyond the window → append.
  if (toChartMs(nextTip.t) > toChartMs(last.t)) {
    out.push(nextTip);
    return out;
  }
  return null;
}

export function BalanceChart({
  holdings,
  fiat,
  discreet = false,
  height = 168,
  className,
  ledger,
  liveTotal,
  leadingControl,
}: Props) {
  const { t } = useTranslation();
  const [period, setPeriod] = useState<Period>(loadSavedPeriod);
  const periodRef = useRef(period);
  periodRef.current = period;
  const fetchGen = useRef(0);
  const holdingsRef = useRef(holdings);
  holdingsRef.current = holdings;
  const ledgerRef = useRef(ledger);
  ledgerRef.current = ledger;

  const useHistory = ledger !== undefined;
  const ledgerReady = Array.isArray(ledger);
  const ledgerPending = useHistory && !ledgerReady;
  const hasHoldings = holdings.some((h) => h.amount > 0);
  // Empty ledger with holdings = no real growth path. Use MTM (price×balance)
  // so the tip still tracks the live total instead of a blank chart.
  const emptyGrowth =
    useHistory && ledgerReady && hasHoldings && (ledger?.length ?? 0) === 0;
  // Held coins missing from the ledger → growth would under-count then tip-spike.
  // Fall back to mark-to-market until history catches up.
  const incompleteGrowth =
    useHistory &&
    ledgerReady &&
    hasHoldings &&
    !emptyGrowth &&
    holdings.some(
      (h) => h.amount > 0 && !(ledger as LedgerEvent[]).some((e) => e.coinId === h.coinId),
    );
  const effectiveHistory = useHistory && !emptyGrowth && !incompleteGrowth;
  // Only `null` (still fetching) blocks paint.
  const blockPaint = ledgerPending;

  const coinKey = useMemo(
    () =>
      holdings
        .filter((h) => h.amount > 0)
        .map((h) => h.coinId)
        .sort()
        .join(","),
    [holdings],
  );
  const ledgerKey = useMemo(() => {
    if (!useHistory) return "price";
    if (ledgerPending) return "pending";
    if (emptyGrowth || incompleteGrowth) return "mtm";
    return `${ledger!.length}:${ledger!
      .slice(0, 8)
      .map((e) => `${e.t}:${e.coinId}:${e.delta}`)
      .join("|")}`;
  }, [useHistory, ledgerPending, emptyGrowth, incompleteGrowth, ledger]);

  const amountsKey = useMemo(
    () =>
      holdings
        .map((h) => `${h.coinId}:${h.amount}`)
        .sort()
        .join(","),
    [holdings],
  );

  const shapeKey = seriesShapeKey(coinKey, fiat, period, ledgerKey);
  const priceKey = useMemo(() => {
    const ids = [
      ...new Set([
        ...holdings.filter((h) => h.amount > 0).map((h) => h.coinId),
        ...(ledgerReady ? ledger!.map((e) => e.coinId) : []),
      ]),
    ];
    return chartsCacheKey(ids, fiat, Number(period));
  }, [holdings, ledger, ledgerReady, fiat, period]);

  const [points, setPoints] = useState<ChartPoint[]>(() => {
    // Instant paint from last good series if we already know the shape.
    return [];
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [pricesReady, setPricesReady] = useState(() => hydrateChartsDataCache(priceKey));

  // Hydrate persisted series as soon as shape is known (revisit / remount).
  useEffect(() => {
    if (blockPaint) return;
    const hit = hydrateSeriesCache(shapeKey);
    if (hit && hit.length >= 2) {
      setPoints(hit);
      setLoading(false);
    }
  }, [shapeKey, blockPaint]);

  useEffect(() => {
    if (!hasHoldings) {
      setPoints([]);
      setLoading(false);
      setError(false);
    }
  }, [hasHoldings, coinKey]);

  // Fetch price history only when the price key changes — never on every balance tick.
  useEffect(() => {
    if (blockPaint && !coinKey) return;
    const ids = priceKey.split("|")[0];
    if (!ids) return;

    if (hydrateChartsDataCache(priceKey)) {
      setPricesReady(true);
      return;
    }

    let cancelled = false;
    const gen = ++fetchGen.current;
    setPricesReady(false);
    if (!points.length) setLoading(true);

    void (async () => {
      try {
        const idList = ids.split(",").filter(Boolean);
        if (!idList.length) {
          setPricesReady(true);
          setLoading(false);
          return;
        }
        const charts = await fetchPriceHistoryCached(
          (coinIds, vs, days) => api.priceHistory(coinIds, vs, days),
          idList,
          fiat,
          Number(period),
        );
        if (cancelled || fetchGen.current !== gen) return;
        setPricesReady(true);
        setError(Object.keys(charts).length === 0);
      } catch {
        if (!cancelled && fetchGen.current === gen) {
          setPricesReady(true);
          setError(true);
          setLoading(false);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [priceKey, blockPaint]);

  // Build or patch the visible series from cached prices.
  useEffect(() => {
    // Keep the last good series on screen while ledger is still loading —
    // wiping caused a loading flash and made the chart feel broken.
    if (blockPaint) {
      return;
    }
    if (!pricesReady) return;

    const charts = chartsDataCache.get(priceKey);
    if (!charts || Object.keys(charts).length === 0) {
      if (!points.length) {
        setPoints([]);
        setLoading(false);
        setError(true);
      }
      return;
    }

    const cached = seriesCache.get(shapeKey);
    const h = holdingsRef.current;
    const led = ledgerRef.current;

    // Tip-patch only within the same growth/price shape — never across modes.
    if (cached && cached.length >= 2) {
      const patched = patchSeriesTip(cached, h, led, charts, effectiveHistory);
      if (patched) {
        const clipped = clipToPeriod(patched, Number(period));
        seriesCache.set(shapeKey, clipped);
        persistSeries(shapeKey, clipped);
        setPoints(clipped);
        setLoading(false);
        setError(false);
        return;
      }
    }

    const raw = effectiveHistory
      ? buildBalanceHistorySeries(h, led as LedgerEvent[], charts)
      : buildPortfolioSeries(h, charts);
    const series = clipToPeriod(raw, Number(period));
    seriesCache.set(shapeKey, series);
    persistSeries(shapeKey, series);
    setPoints(series);
    setLoading(false);
    setError(false);
    // amountsKey intentionally included so tip patches when balance moves
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    shapeKey,
    amountsKey,
    pricesReady,
    priceKey,
    blockPaint,
    emptyGrowth,
    incompleteGrowth,
    effectiveHistory,
    period,
    ledgerKey,
  ]);

  // Display series: tip always matches the hero live total (spot), not a
  // stale CoinGecko candle that can be tens of dollars off.
  const displayPoints = useMemo(
    () => pinSeriesTip(points, liveTotal),
    [points, liveTotal],
  );

  const changeRaw = seriesChangePct(displayPoints);
  const changeAbsRaw = seriesChangeAbs(displayPoints);
  const hasSeries = displayPoints.length > 0;
  const change = changeRaw ?? (hasSeries ? 0 : null);
  const changeAbs = changeAbsRaw ?? (hasSeries ? 0 : null);
  const tone = change == null || change >= 0 ? "up" : "down";
  // blockPaint always wins — never keep a stale SOL-price series visible
  // while growth history is still loading.
  const waiting = blockPaint || ((loading || !pricesReady) && !hasSeries);
  const showStatus = waiting || error || !hasSeries;

  return (
    <div className={`balance-chart ${className ?? ""}`}>
      <div className="balance-chart__meta">
        <div className="balance-chart__change">
          {discreet ? (
            <span className="is-muted">••••</span>
          ) : waiting ? (
            <span className="is-muted">{t("common.loading")}</span>
          ) : error ? (
            <span className="is-muted">
              {t("chart.loadError", { defaultValue: "Couldn't load chart" })}
            </span>
          ) : change == null || changeAbs == null ? (
            <span className="is-muted">{t("chart.noData")}</span>
          ) : (
            <span className={`delta delta--${tone}`}>
              {changeAbs > 0 ? "+" : changeAbs < 0 ? "−" : ""}
              {formatMoney(Math.abs(changeAbs), fiat, false)}
              <span className="delta__pct">
                {" "}
                ({change > 0 ? "+" : ""}
                {change.toFixed(2)}%)
              </span>
            </span>
          )}
        </div>
        <div className="balance-chart__controls">
          {leadingControl}
          <div className="period-chips" role="tablist" aria-label={t("chart.period")}>
            {PERIODS.map((p) => (
              <button
                key={p.id}
                type="button"
                role="tab"
                aria-selected={period === p.id}
                className={`period-chip${period === p.id ? " is-active" : ""}`}
                onClick={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  setPeriod(p.id);
                  savePeriod(p.id);
                }}
              >
                {t(p.labelKey, { defaultValue: p.label })}
              </button>
            ))}
          </div>
        </div>
      </div>
      <div className={`balance-chart__frame${waiting ? " is-loading" : ""}`}>
        <AreaChart
          points={discreet || waiting ? [] : displayPoints}
          height={height}
          tone={tone}
          periodDays={Number(period)}
          formatValue={(v) => formatMoney(v, fiat, false)}
        />
        {waiting ? (
          <div className="balance-chart__overlay" aria-live="polite">
            {t("common.loading")}
          </div>
        ) : null}
        {!waiting && error && !hasSeries ? (
          <div className="balance-chart__overlay is-error" aria-live="polite">
            {t("chart.loadError", { defaultValue: "Couldn't load chart" })}
          </div>
        ) : null}
        {!waiting && !error && !hasSeries && showStatus ? (
          <div className="balance-chart__overlay is-empty" aria-live="polite">
            {emptyGrowth
              ? t("chart.noHistory", { defaultValue: "No transaction history yet" })
              : t("chart.noData")}
          </div>
        ) : null}
      </div>
    </div>
  );
}
