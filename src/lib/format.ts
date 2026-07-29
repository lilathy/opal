/** Human-readable chain / portfolio helpers for the UI. */

const CHAIN_NAMES: Record<string, string> = {
  btc: "Bitcoin",
  eth: "Ethereum",
  arb: "Arbitrum",
  base: "Base",
  polygon: "Polygon",
  avax: "Avalanche",
  bsc: "BNB Smart Chain",
  gnosis: "Gnosis",
  trx: "Tron",
  linea: "Linea",
  sol: "Solana",
  ton: "Gram",
  ltc: "Litecoin",
  doge: "Dogecoin",
  xmr: "Monero",
};

export function chainLabel(id: string): string {
  return CHAIN_NAMES[id.toLowerCase()] ?? id.toUpperCase();
}

export function kindLabel(kind: string): string {
  switch (kind) {
    case "software":
      return "Software";
    case "trezor":
      return "Trezor";
    case "watch_only":
      return "Watch";
    default:
      return kind;
  }
}

/** Next default name for a chain: BTC #1, SOL #2, … */
export function nextChainPortfolioName(
  chain: string,
  existing: { chain: string; name?: string }[],
): string {
  const key = chain.toLowerCase();
  const id = key === "ton" ? "Gram" : chain.toUpperCase();
  const count = existing.filter((p) => p.chain.toLowerCase() === key).length;
  return `${id} #${count + 1}`;
}

export function formatMoney(
  amount: number | null | undefined,
  fiat: string,
  discreet: boolean,
): string {
  if (discreet) return "••••";
  const n = amount == null || Number.isNaN(amount) ? 0 : amount;
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: fiat || "USD",
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    }).format(n);
  } catch {
    return `$${n.toFixed(2)}`;
  }
}

/** Crypto quantity number only - always numeric, never an em dash. */
export function formatAmount(
  amount: string | number | null | undefined,
  discreet: boolean,
  maxFractionDigits = 4,
): string {
  if (discreet) return "••••";
  const raw = amount == null || amount === "" ? 0 : Number(amount);
  const n = Number.isFinite(raw) ? raw : 0;
  return n.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: maxFractionDigits,
  });
}

/**
 * Display a crypto amount without trailing junk zeros
 * (e.g. `0.919030000000` → `0.91903`).
 */
export function formatCompactAmount(
  amount: string | number | null | undefined,
  maxFractionDigits = 8,
): string {
  if (amount == null || amount === "") return "";
  const raw = typeof amount === "number" ? amount : Number(String(amount).trim());
  if (!Number.isFinite(raw)) {
    const s = String(amount).trim();
    if (!s.includes(".")) return s;
    return s.replace(/(\.\d*?[1-9])0+$/, "$1").replace(/\.0+$/, "");
  }
  if (raw === 0) return "0";
  const digits =
    Math.abs(raw) >= 1 ? Math.min(6, maxFractionDigits) : maxFractionDigits;
  return raw
    .toFixed(digits)
    .replace(/(\.\d*?[1-9])0+$/, "$1")
    .replace(/\.0+$/, "")
    .replace(/\.$/, "");
}

/** Crypto quantity with symbol for list rows. */
export function formatQty(
  amount: string | number | null | undefined,
  symbol: string,
  discreet: boolean,
  maxFractionDigits = 4,
): string {
  if (discreet) return "••••";
  return `${formatAmount(amount, false, maxFractionDigits)} ${symbol}`;
}

/** Parse explorer timestamps (unix sec, unix ms, or ISO) into a Date. */
export function txTimestampDate(
  raw: string | number | null | undefined,
): Date | null {
  if (raw == null || raw === "") return null;
  const s = String(raw).trim();
  if (!s) return null;
  // Unix seconds (9-12 digits).
  if (/^\d{9,12}$/.test(s)) {
    const n = Number(s);
    if (!Number.isFinite(n) || n <= 0) return null;
    const d = new Date(n * 1000);
    return Number.isNaN(d.getTime()) ? null : d;
  }
  // Unix milliseconds (13-16 digits).
  if (/^\d{13,16}$/.test(s)) {
    const n = Number(s);
    if (!Number.isFinite(n) || n <= 0) return null;
    const d = new Date(n);
    return Number.isNaN(d.getTime()) ? null : d;
  }
  // ISO-8601 / RFC3339 (e.g. BlockCypher DOGE `received`).
  const ms = Date.parse(s);
  if (!Number.isFinite(ms) || ms <= 0) return null;
  const d = new Date(ms);
  return Number.isNaN(d.getTime()) ? null : d;
}

/** Timestamp (unix sec/ms or ISO) into a short "Jul 26, 3:04 PM" label. */
export function formatTxDate(unixSeconds: string | number | null | undefined): string {
  const d = txTimestampDate(unixSeconds);
  if (!d) return "";
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

/** Time-only label for rows already grouped under a day header. */
export function formatTxTime(unixSeconds: string | number | null | undefined): string {
  const d = txTimestampDate(unixSeconds);
  if (!d) return "";
  return d.toLocaleString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
}

function startOfLocalDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** Day bucket key + human label for section headers (Today / Yesterday / Jul 26). */
export function formatTxDayGroup(
  unixSeconds: string | number | null | undefined,
  labels: { today: string; yesterday: string; pending: string },
): { key: string; label: string } {
  const d = txTimestampDate(unixSeconds);
  if (!d) return { key: "pending", label: labels.pending };

  const day = startOfLocalDay(d);
  const today = startOfLocalDay(new Date());
  const yesterday = today - 86_400_000;

  if (day === today) return { key: `d:${day}`, label: labels.today };
  if (day === yesterday) return { key: `d:${day}`, label: labels.yesterday };

  return {
    key: `d:${day}`,
    label: d.toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
      year: d.getFullYear() !== new Date().getFullYear() ? "numeric" : undefined,
    }),
  };
}

/** Shorten a chain address/txid to `abcd1234…wxyz` for compact display. */
export function shortHash(value: string, head = 8, tail = 6): string {
  if (!value || value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}
