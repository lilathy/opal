import { useId, useMemo, useState } from "react";
import { smoothPath, smoothYAtX, type ChartPoint } from "../lib/charts";

type Props = {
  points: ChartPoint[];
  height?: number;
  /** Force positive/negative color; auto from first→last when omitted. */
  tone?: "up" | "down" | "flat";
  className?: string;
  formatValue?: (v: number) => string;
  /** Selected chart window — controls hover timestamp formatting. */
  periodDays?: number;
};

function chartMs(t: number): number {
  return t > 0 && t < 1e12 ? t * 1000 : t;
}

function formatHoverDate(t: number, periodDays: number): string {
  const d = new Date(chartMs(t));
  if (periodDays <= 1) {
    return d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  }
  if (periodDays <= 7) {
    return d.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  }
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function AreaChart({
  points,
  height = 168,
  tone,
  className,
  formatValue = (v) => v.toFixed(2),
  periodDays = 7,
}: Props) {
  const gid = useId().replace(/:/g, "");
  // Continuous cursor-space x (viewBox units), not a snapped point index —
  // this is what lets the crosshair track the mouse 1:1 instead of jumping
  // between data points and relying on a CSS transition to "catch up"
  // (which is what caused the laggy, rubber-banding feel).
  const [hoverX, setHoverX] = useState<number | null>(null);

  const derivedTone =
    tone ??
    (points.length >= 2 && points[points.length - 1].v < points[0].v ? "down" : "up");

  // Always one of the two P&L colors — never the neon accent, even for a
  // perfectly flat line or a lone data point.
  const stroke = derivedTone === "down" ? "var(--negative)" : "var(--positive)";

  const { line, area, mapped, pad } = useMemo(() => {
    const pad = { t: 12, r: 8, b: 8, l: 8 };
    const w = 640;
    const h = height;
    if (points.length === 0) {
      return { line: "", area: "", mapped: [] as Array<{ x: number; y: number; i: number }>, pad };
    }
    const minV = Math.min(...points.map((p) => p.v));
    const maxV = Math.max(...points.map((p) => p.v));
    const span = Math.max(maxV - minV, Math.abs(maxV) * 0.02, 1e-9);
    const minT = chartMs(points[0].t);
    const maxT = chartMs(points[points.length - 1].t);
    const tSpan = Math.max(maxT - minT, 1);
    const mapped = points.map((p, i) => {
      const t = chartMs(p.t);
      const x = pad.l + ((t - minT) / tSpan) * (w - pad.l - pad.r);
      const y = pad.t + (1 - (p.v - minV) / span) * (h - pad.t - pad.b);
      return { x, y, i };
    });
    const paths = smoothPath(
      mapped.map((m) => ({ x: m.x, y: m.y })),
      h - pad.b,
    );
    return { line: paths.line, area: paths.area, mapped, pad };
  }, [points, height]);

  // Interpolate value linearly, but take Y from the same cubic the path uses
  // so the dot doesn't float above dips when the curve bends away from the chord.
  const hi = useMemo(() => {
    if (hoverX == null || mapped.length === 0) return null;
    const x = Math.min(Math.max(hoverX, mapped[0].x), mapped[mapped.length - 1].x);
    let lo = 0;
    while (lo < mapped.length - 2 && mapped[lo + 1].x < x) lo++;
    const a = mapped[lo];
    const b = mapped[Math.min(lo + 1, mapped.length - 1)];
    const span = b.x - a.x;
    const t = span > 0 ? (x - a.x) / span : 0;
    const y =
      smoothYAtX(
        mapped.map((m) => ({ x: m.x, y: m.y })),
        x,
      ) ?? a.y + (b.y - a.y) * t;
    const v = points[a.i].v + (points[b.i].v - points[a.i].v) * t;
    const nearestPoint = points[t < 0.5 ? a.i : b.i];
    return { x, y, v, date: nearestPoint.t };
  }, [hoverX, mapped, points]);

  if (!points.length) {
    return (
      <div className={`area-chart area-chart--empty ${className ?? ""}`} style={{ height }}>
        <div className="area-chart__empty" />
      </div>
    );
  }

  return (
    <div className={`area-chart ${className ?? ""}`} style={{ height }}>
      <svg
        viewBox={`0 0 640 ${height}`}
        preserveAspectRatio="none"
        className="area-chart__svg"
        onMouseLeave={() => setHoverX(null)}
        onMouseMove={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          setHoverX(((e.clientX - rect.left) / rect.width) * 640);
        }}
      >
        <defs>
          <linearGradient id={`fill-${gid}`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={stroke} stopOpacity="0.28" />
            <stop offset="100%" stopColor={stroke} stopOpacity="0" />
          </linearGradient>
        </defs>
        <path d={area} fill={`url(#fill-${gid})`} />
        <path
          d={line}
          fill="none"
          stroke={stroke}
          strokeWidth="2.25"
          strokeLinecap="round"
          strokeLinejoin="round"
          vectorEffect="non-scaling-stroke"
        />
        {hi ? (
          <line
            x1={hi.x}
            x2={hi.x}
            y1={pad.t}
            y2={height - pad.b}
            stroke="rgba(255,255,255,0.18)"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
        ) : null}
      </svg>
      {hi ? (
        <>
          {/* Rendered as real HTML (not SVG) so the chart's non-uniform x/y
             viewBox scaling can't stretch the dot into an ellipse. Position
             follows the cursor directly (no transition) — that 1:1 tracking
             *is* the smooth glide; a delay here is what read as laggy. */}
          <div
            className="area-chart__dot"
            style={{
              left: `${(hi.x / 640) * 100}%`,
              // Percent of chart height — matches viewBox Y under preserveAspectRatio=none.
              top: `${(hi.y / height) * 100}%`,
              background: stroke,
            }}
          />
          <div className="area-chart__tip" style={{ left: `${(hi.x / 640) * 100}%` }}>
            <strong>{formatValue(hi.v)}</strong>
            <span>{formatHoverDate(hi.date, periodDays)}</span>
          </div>
        </>
      ) : null}
    </div>
  );
}
