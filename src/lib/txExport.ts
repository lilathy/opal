import type { PortfolioRecord, TxRow } from "./api";
import { api, invalidatePortfolioHistory } from "./api";
import { chainLabel, txTimestampDate } from "./format";
import { txFiatValue } from "./txFilter";

export type TxExportRow = {
  dateUtc: string;
  timestampRaw: string;
  portfolioName: string;
  portfolioId: string;
  portfolioKind: string;
  chain: string;
  chainLabel: string;
  direction: string;
  asset: string;
  amount: string;
  fee: string;
  feeAsset: string;
  status: string;
  txid: string;
  counterparty: string;
  explorerUrl: string;
  approxValueSpot: string;
  fiat: string;
};

/** Native fee ticker for a portfolio chain. */
export function nativeSymbolForChain(chain: string): string {
  switch (chain.trim().toLowerCase()) {
    case "btc":
      return "BTC";
    case "eth":
    case "arb":
    case "base":
    case "op":
    case "linea":
      return "ETH";
    case "polygon":
      return "POL";
    case "avax":
      return "AVAX";
    case "bsc":
      return "BNB";
    case "gnosis":
      return "XDAI";
    case "trx":
      return "TRX";
    case "sol":
      return "SOL";
    case "ton":
      return "TON";
    case "ltc":
      return "LTC";
    case "doge":
      return "DOGE";
    case "xmr":
      return "XMR";
    default:
      return chain.trim().toUpperCase() || "";
  }
}

function escapeCsv(value: string): string {
  if (/[",\r\n]/.test(value)) {
    return `"${value.replace(/"/g, '""')}"`;
  }
  return value;
}

function formatUtc(d: Date | null): string {
  if (!d) return "";
  return d.toISOString().replace(/\.\d{3}Z$/, "Z");
}

function formatSpot(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "";
  if (n >= 1000) return n.toFixed(2);
  if (n >= 1) return n.toFixed(4);
  return n.toPrecision(6);
}

export function buildTxExportRows(
  portfolios: PortfolioRecord[],
  historyById: Record<string, TxRow[]>,
  fiat: string,
  spotPrices: Record<string, number>,
): TxExportRow[] {
  const rows: TxExportRow[] = [];
  for (const p of portfolios) {
    const txs = historyById[p.id] ?? [];
    const feeAsset = nativeSymbolForChain(p.chain);
    for (const tx of txs) {
      const when = txTimestampDate(tx.timestamp);
      const spot = txFiatValue(tx, spotPrices);
      rows.push({
        dateUtc: formatUtc(when),
        timestampRaw: String(tx.timestamp ?? ""),
        portfolioName: p.name,
        portfolioId: p.id,
        portfolioKind: p.kind,
        chain: p.chain,
        chainLabel: chainLabel(p.chain),
        direction: tx.direction || "unknown",
        asset: tx.symbol || "",
        amount: tx.amount ?? "",
        fee: tx.fee?.trim() ? tx.fee : "",
        feeAsset: tx.fee?.trim() ? feeAsset : "",
        status: tx.status || "",
        txid: tx.txid || "",
        counterparty: tx.counterparty?.trim() ? tx.counterparty : "",
        explorerUrl: tx.explorer_url || "",
        approxValueSpot: formatSpot(spot),
        fiat: fiat.toUpperCase(),
      });
    }
  }
  rows.sort((a, b) => {
    const ta = a.dateUtc ? Date.parse(a.dateUtc) : 0;
    const tb = b.dateUtc ? Date.parse(b.dateUtc) : 0;
    return tb - ta;
  });
  return rows;
}

const CSV_HEADERS = [
  "date_utc",
  "portfolio",
  "portfolio_id",
  "portfolio_kind",
  "chain",
  "chain_name",
  "direction",
  "asset",
  "amount",
  "fee",
  "fee_asset",
  "status",
  "txid",
  "counterparty",
  "explorer_url",
  "approx_value_spot",
  "fiat",
  "timestamp_raw",
] as const;

export function buildTxCsv(rows: TxExportRow[]): string {
  const lines = [
    CSV_HEADERS.join(","),
    ...rows.map((r) =>
      [
        r.dateUtc,
        r.portfolioName,
        r.portfolioId,
        r.portfolioKind,
        r.chain,
        r.chainLabel,
        r.direction,
        r.asset,
        r.amount,
        r.fee,
        r.feeAsset,
        r.status,
        r.txid,
        r.counterparty,
        r.explorerUrl,
        r.approxValueSpot,
        r.fiat,
        r.timestampRaw,
      ]
        .map((cell) => escapeCsv(cell))
        .join(","),
    ),
  ];
  // UTF-8 BOM helps Excel detect encoding.
  return `\uFEFF${lines.join("\r\n")}\r\n`;
}

export function defaultTxExportFilename(now = new Date()): string {
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  const d = String(now.getDate()).padStart(2, "0");
  return `opal-transactions-${y}-${m}-${d}.csv`;
}

/**
 * Fetch recent history for every portfolio and build a tax-oriented CSV.
 * Uses explorer windows (not full chain history). Spot fiat is approximate.
 */
export async function collectTxExportCsv(opts: {
  fiat: string;
  spotPrices: Record<string, number>;
  fresh?: boolean;
}): Promise<{ csv: string; count: number; portfolios: number }> {
  const portfolios = await api.portfolioList();
  if (opts.fresh) {
    invalidatePortfolioHistory();
  }
  const historyById: Record<string, TxRow[]> = {};
  await Promise.all(
    portfolios.map(async (p) => {
      try {
        historyById[p.id] = await api.portfolioHistory(p.id);
      } catch {
        historyById[p.id] = [];
      }
    }),
  );
  const rows = buildTxExportRows(
    portfolios,
    historyById,
    opts.fiat,
    opts.spotPrices,
  );
  return {
    csv: buildTxCsv(rows),
    count: rows.length,
    portfolios: portfolios.length,
  };
}
