import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  allocationSlices,
  approxNetFlow,
  bestAndWorstDays,
  clipToPeriod,
  downsampleSpark,
  pinSeriesTip,
  rankAssetMoves,
  type HoldingFiat,
} from "../lib/analytics";
import { resolveAnalyticsLayout } from "../lib/analyticsTiles";
import { api, type PortfolioBalance } from "../lib/api";
import { assetFiatValue, assetsOf } from "../lib/balances";
import {
  buildBalanceHistorySeries,
  buildPortfolioSeries,
  coinIdForSymbol,
  seriesChangeAbs,
  seriesChangePct,
  type ChartPoint,
  type LedgerEvent,
} from "../lib/charts";
import { fetchPriceHistoryCached } from "../lib/chartCache";
import { formatMoney } from "../lib/format";
import { ASSET_COLORS } from "./CryptoIcons";

type Props = {
  balances: PortfolioBalance[];
  holdings: Array<{ coinId: string; amount: number }>;
  ledger: LedgerEvent[] | null;
  liveTotal: number;
  fiat: string;
  discreet: boolean;
  fiatPrices: Record<string, number>;
  portfolioCount: number;
  analyticsEnabled?: boolean;
  tileOrder?: string[];
  hiddenTiles?: string[];
};

type Tone = "up" | "down" | "flat";

type Slice = { label: string; pct: number; color: string };

type Tile =
  | {
      id: string;
      style: "stat" | "split" | "spark";
      label: string;
      value: string;
      hint?: string;
      tone: Tone;
      spark?: number[];
      empty?: boolean;
    }
  | {
      id: string;
      style: "donut";
      label: string;
      tone: Tone;
      slices: Slice[];
      empty?: boolean;
    };

function toneOf(n: number | null | undefined): Tone {
  if (n == null || !Number.isFinite(n) || Math.abs(n) < 1e-9) return "flat";
  return n > 0 ? "up" : "down";
}

function fmtSignedMoney(n: number, fiat: string, discreet: boolean): string {
  if (discreet) return "••••";
  const sign = n > 0 ? "+" : n < 0 ? "−" : "";
  return `${sign}${formatMoney(Math.abs(n), fiat, false)}`;
}

function fmtPct(n: number, discreet: boolean): string {
  if (discreet) return "••••";
  const sign = n > 0 ? "+" : n < 0 ? "−" : "";
  return `${sign}${Math.abs(n).toFixed(2)}%`;
}

function fmtDay(t: number): string {
  try {
    return new Date(t).toLocaleDateString(undefined, {
      weekday: "short",
      month: "short",
      day: "numeric",
    });
  } catch {
    return "";
  }
}

function colorForSymbol(symbol: string): string {
  const key = symbol.toLowerCase();
  if (key === "other") return "rgba(196, 192, 186, 0.55)";
  return ASSET_COLORS[key] ?? "#8a857e";
}

function smoothLinePath(points: { x: number; y: number }[]): string {
  if (points.length < 2) return "";
  let d = `M ${points[0].x.toFixed(2)} ${points[0].y.toFixed(2)}`;
  for (let i = 0; i < points.length - 1; i++) {
    const p0 = points[i === 0 ? 0 : i - 1];
    const p1 = points[i];
    const p2 = points[i + 1];
    const p3 = points[i + 2] ?? p2;
    const cp1x = p1.x + (p2.x - p0.x) / 6;
    const cp1y = p1.y + (p2.y - p0.y) / 6;
    const cp2x = p2.x - (p3.x - p1.x) / 6;
    const cp2y = p2.y - (p3.y - p1.y) / 6;
    d += ` C ${cp1x.toFixed(2)} ${cp1y.toFixed(2)}, ${cp2x.toFixed(2)} ${cp2y.toFixed(2)}, ${p2.x.toFixed(2)} ${p2.y.toFixed(2)}`;
  }
  return d;
}

function Spark({ values, tone }: { values: number[]; tone: Tone }) {
  const w = 160;
  const h = 36;
  const padX = 2;
  const padY = 6;
  if (values.length < 2) {
    return <div className="analytics-tile__spark analytics-tile__spark--empty" />;
  }
  const min = Math.min(...values);
  const max = Math.max(...values);
  const range = Math.max(max - min, 1e-9);
  const points = values.map((v, i) => ({
    x: padX + (i / Math.max(values.length - 1, 1)) * (w - padX * 2),
    y: padY + (1 - (v - min) / range) * (h - padY * 2),
  }));
  const line = smoothLinePath(points);
  const area = `${line} L ${points[points.length - 1].x.toFixed(2)} ${h} L ${points[0].x.toFixed(2)} ${h} Z`;
  const stroke =
    tone === "down" ? "var(--negative)" : tone === "up" ? "var(--positive)" : "var(--accent)";
  const gradId = `analytics-spark-${tone}`;

  return (
    <svg
      className="analytics-tile__spark"
      viewBox={`0 0 ${w} ${h}`}
      preserveAspectRatio="none"
      aria-hidden
    >
      <defs>
        <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={stroke} stopOpacity="0.28" />
          <stop offset="100%" stopColor={stroke} stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={area} fill={`url(#${gradId})`} />
      <path
        d={line}
        fill="none"
        stroke={stroke}
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

function Donut({ slices }: { slices: Slice[] }) {
  const size = 76;
  const stroke = 11;
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const gap = slices.length > 1 ? 3 : 0;
  const usable = c - gap * slices.length;
  let offset = 0;

  return (
    <svg className="analytics-tile__donut" viewBox={`0 0 ${size} ${size}`} aria-hidden>
      <g transform={`rotate(-90 ${size / 2} ${size / 2})`}>
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          stroke="rgba(255,255,255,0.06)"
          strokeWidth={stroke}
        />
        {slices.map((s) => {
          const len = Math.max((s.pct / 100) * usable, 0);
          const node = (
            <circle
              key={s.label}
              className="analytics-tile__donut-seg"
              cx={size / 2}
              cy={size / 2}
              r={r}
              fill="none"
              stroke={s.color}
              strokeWidth={stroke}
              strokeDasharray={`${len} ${c - len}`}
              strokeDashoffset={-offset}
              strokeLinecap="butt"
            />
          );
          offset += len + gap;
          return node;
        })}
      </g>
    </svg>
  );
}

export function AnalyticsPanel({
  balances,
  holdings,
  ledger,
  liveTotal,
  fiat,
  discreet,
  fiatPrices,
  portfolioCount,
  analyticsEnabled = true,
  tileOrder,
  hiddenTiles,
}: Props) {
  const { t } = useTranslation();
  const [charts, setCharts] = useState<Record<string, Array<[number, number]>> | null>(null);
  const [pricesLoading, setPricesLoading] = useState(false);

  const assets: HoldingFiat[] = useMemo(() => {
    const map = new Map<string, HoldingFiat>();
    for (const bal of balances) {
      for (const a of assetsOf(bal)) {
        const coinId = coinIdForSymbol(a.symbol);
        if (!coinId) continue;
        const amt = Number(a.amount);
        if (!Number.isFinite(amt) || amt <= 0) continue;
        const fiatValue = assetFiatValue(a, fiat, fiatPrices);
        const prev = map.get(coinId);
        if (prev) {
          prev.amount += amt;
          prev.fiatValue += fiatValue;
        } else {
          map.set(coinId, {
            symbol: a.symbol.toUpperCase(),
            coinId,
            amount: amt,
            fiatValue,
          });
        }
      }
    }
    return [...map.values()].sort((a, b) => b.fiatValue - a.fiatValue);
  }, [balances, fiat, fiatPrices]);

  const coinIds = useMemo(
    () => [...new Set(holdings.filter((h) => h.amount > 0).map((h) => h.coinId))],
    [holdings],
  );

  useEffect(() => {
    if (!coinIds.length) {
      setCharts(null);
      return;
    }
    let cancelled = false;
    setPricesLoading(true);
    void fetchPriceHistoryCached(
      (ids, vs, days) => api.priceHistory(ids, vs, days),
      coinIds,
      fiat,
      30,
    )
      .then((data) => {
        if (!cancelled) setCharts(data);
      })
      .catch(() => {
        if (!cancelled) setCharts(null);
      })
      .finally(() => {
        if (!cancelled) setPricesLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [coinIds.join("|"), fiat]);

  const series30 = useMemo(() => {
    if (!charts || !holdings.length) return [] as ChartPoint[];
    const led = ledger;
    let raw =
      led && led.length
        ? buildBalanceHistorySeries(holdings, led, charts)
        : ([] as ChartPoint[]);
    // Incomplete ledger coverage → fall back to mark-to-market so tiles still fill.
    if (raw.length < 2) {
      raw = buildPortfolioSeries(holdings, charts);
    }
    return pinSeriesTip(clipToPeriod(raw, 30), liveTotal);
  }, [charts, holdings, ledger, liveTotal]);

  const series7 = useMemo(() => clipToPeriod(series30, 7), [series30]);

  const change30Abs = seriesChangeAbs(series30);
  const change30Pct = seriesChangePct(series30);
  const change7Abs = seriesChangeAbs(series7);
  const change7Pct = seriesChangePct(series7);
  const { best, worst } = useMemo(() => bestAndWorstDays(series30), [series30]);
  const { top: topMover } = useMemo(
    () => (charts ? rankAssetMoves(assets, charts, 7) : { top: null, bottom: null }),
    [assets, charts],
  );

  const slices = useMemo(() => {
    const rows = allocationSlices(assets, liveTotal);
    return rows.map((r) => ({
      label: r.label,
      pct: r.pct,
      color: colorForSymbol(r.label),
    }));
  }, [assets, liveTotal]);

  const largest = assets[0] ?? null;
  const netFlow = useMemo(
    () => (ledger?.length ? approxNetFlow(ledger, assets, 30) : 0),
    [ledger, assets],
  );
  const sparkValues = useMemo(() => downsampleSpark(series7, 20), [series7]);
  const waiting = pricesLoading && series30.length < 2;
  const dash = discreet ? "••••" : "—";

  const tiles: Tile[] = [
    {
      id: "change30",
      style: "stat",
      label: t("analytics.change30", { defaultValue: "30d change" }),
      value:
        waiting || change30Abs == null
          ? dash
          : fmtSignedMoney(change30Abs, fiat, discreet),
      hint:
        waiting || change30Pct == null
          ? t("common.loading", { defaultValue: "Loading…" })
          : fmtPct(change30Pct, discreet),
      tone: waiting ? "flat" : toneOf(change30Abs),
      empty: !waiting && change30Abs == null,
    },
    {
      id: "change7",
      style: "stat",
      label: t("analytics.change7", { defaultValue: "7d change" }),
      value:
        waiting || change7Abs == null ? dash : fmtSignedMoney(change7Abs, fiat, discreet),
      hint:
        waiting || change7Pct == null
          ? t("common.loading", { defaultValue: "Loading…" })
          : fmtPct(change7Pct, discreet),
      tone: waiting ? "flat" : toneOf(change7Abs),
      empty: !waiting && change7Abs == null,
    },
    {
      id: "spark7",
      style: "spark",
      label: t("analytics.trajectory7", { defaultValue: "7d trajectory" }),
      value:
        waiting || change7Pct == null ? dash : fmtPct(change7Pct, discreet),
      hint: t("analytics.thisWeek", { defaultValue: "This week" }),
      tone: waiting ? "flat" : toneOf(change7Pct),
      spark: discreet ? [] : sparkValues,
      empty: !waiting && sparkValues.length < 2,
    },
    {
      id: "bestDay",
      style: "split",
      label: t("analytics.bestDay", { defaultValue: "Best day" }),
      value: !best ? dash : fmtSignedMoney(best.abs, fiat, discreet),
      hint: best
        ? `${fmtDay(best.t)} · ${fmtPct(best.pct, discreet)}`
        : t("analytics.needHistory", { defaultValue: "Need more history" }),
      tone: toneOf(best?.abs),
      empty: !best,
    },
    {
      id: "worstDay",
      style: "split",
      label: t("analytics.worstDay", { defaultValue: "Worst day" }),
      value: !worst ? dash : fmtSignedMoney(worst.abs, fiat, discreet),
      hint: worst
        ? `${fmtDay(worst.t)} · ${fmtPct(worst.pct, discreet)}`
        : t("analytics.needHistory", { defaultValue: "Need more history" }),
      tone: toneOf(worst?.abs),
      empty: !worst,
    },
    {
      id: "topMover",
      style: "split",
      label: t("analytics.topMover", { defaultValue: "Top mover" }),
      value: topMover ? topMover.symbol : dash,
      hint: topMover
        ? `${fmtPct(topMover.pct, discreet)} · ${fmtSignedMoney(topMover.abs, fiat, discreet)}`
        : t("analytics.needPrices", { defaultValue: "Need price history" }),
      tone: toneOf(topMover?.pct),
      empty: !topMover,
    },
    {
      id: "allocation",
      style: "donut",
      label: t("analytics.allocation", { defaultValue: "Allocation" }),
      tone: "flat",
      slices: discreet
        ? []
        : slices.map((s) => ({ ...s, pct: Math.max(s.pct, 0) })),
      empty: slices.length === 0,
    },
    {
      id: "largest",
      style: "stat",
      label: t("analytics.largest", { defaultValue: "Largest holding" }),
      value: largest ? largest.symbol : dash,
      hint: largest
        ? discreet
          ? "••••"
          : `${formatMoney(largest.fiatValue, fiat, false)} · ${((largest.fiatValue / Math.max(liveTotal, 1e-9)) * 100).toFixed(0)}%`
        : t("analytics.noHoldings", { defaultValue: "No holdings yet" }),
      tone: "flat",
      empty: !largest,
    },
    {
      id: "flow",
      style: "stat",
      label: t("analytics.netFlow30", { defaultValue: "30d net flow" }),
      value:
        ledger == null
          ? dash
          : !ledger.length
            ? dash
            : fmtSignedMoney(netFlow, fiat, discreet),
      hint:
        ledger == null
          ? t("common.loading", { defaultValue: "Loading…" })
          : t("analytics.netFlowHint", {
              defaultValue: "{{count}} portfolios",
              count: portfolioCount,
            }),
      tone: ledger == null || !ledger.length ? "flat" : toneOf(netFlow),
      empty: ledger != null && !ledger.length,
    },
  ];

  if (!analyticsEnabled) {
    return null;
  }

  if (!balances.length && liveTotal <= 0) {
    return null;
  }

  const { visible } = resolveAnalyticsLayout(tileOrder, hiddenTiles);
  const byId = new Map(tiles.map((tile) => [tile.id, tile]));
  const shown = visible
    .map((id) => byId.get(id))
    .filter((tile): tile is Tile => tile != null);

  if (!shown.length) {
    return null;
  }

  return (
    <section className="analytics-preview" aria-label={t("analytics.title")}>
      <div className="analytics-preview__head">
        <h3 className="analytics-preview__heading">{t("analytics.title")}</h3>
      </div>
      <div className="analytics-preview__grid">
        {shown.map((tile) => (
          <article
            key={tile.id}
            className={`analytics-tile analytics-tile--${tile.style}${
              tile.tone ? ` is-${tile.tone}` : ""
            }${tile.empty ? " is-empty" : ""}`}
          >
            <p className="analytics-tile__label">{tile.label}</p>

            {tile.style === "donut" ? (
              tile.slices.length > 0 ? (
                <div className="analytics-tile__donut-row">
                  <Donut slices={tile.slices} />
                  <ul className="analytics-tile__legend">
                    {tile.slices.map((s) => (
                      <li key={s.label}>
                        <span
                          className="analytics-tile__swatch"
                          style={{ background: s.color }}
                        />
                        <span className="analytics-tile__legend-name">{s.label}</span>
                        <span className="analytics-tile__legend-pct">
                          {discreet ? "••" : `${s.pct.toFixed(0)}%`}
                        </span>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : (
                <div className="analytics-tile__body">
                  <p className="analytics-tile__value">{dash}</p>
                  <p className="analytics-tile__hint">
                    {t("analytics.noHoldings", { defaultValue: "No holdings yet" })}
                  </p>
                </div>
              )
            ) : (
              <>
                <div className="analytics-tile__body">
                  <p className="analytics-tile__value">{tile.value}</p>
                  {tile.hint ? (
                    <p
                      className={`analytics-tile__hint${
                        tile.tone === "up" || tile.tone === "down"
                          ? ` delta--${tile.tone}`
                          : ""
                      }`}
                    >
                      {tile.hint}
                    </p>
                  ) : null}
                </div>
                {tile.style === "spark" ? (
                  <Spark values={tile.spark ?? []} tone={tile.tone} />
                ) : null}
              </>
            )}
          </article>
        ))}
      </div>
    </section>
  );
}
