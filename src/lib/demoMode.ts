/**
 * Screenshot / marketing fixtures for local `vite` / `tauri:dev` only.
 * Tree-shaken / never active in production builds (`import.meta.env.DEV`).
 *
 * Opt out while developing: localStorage.setItem("opal:demo", "0")
 */
import type { PortfolioBalance, PortfolioRecord, TxRow } from "./api";
import { txsToLedger, type LedgerEvent } from "./charts";

export function isDemoMode(): boolean {
  if (!import.meta.env.DEV) return false;
  try {
    if (window.localStorage.getItem("opal:demo") === "0") return false;
  } catch {
    /* ignore */
  }
  return true;
}

type ChainDemo = {
  amount: string;
  decimals: number;
  symbol: string;
  explorer: (txid: string) => string;
};

const CHAINS: Record<string, ChainDemo> = {
  xmr: {
    amount: "1284.471829147",
    decimals: 12,
    symbol: "XMR",
    explorer: (txid) => `https://xmrchain.net/tx/${txid}`,
  },
  btc: {
    amount: "0.08742135",
    decimals: 8,
    symbol: "BTC",
    explorer: (txid) => `https://mempool.space/tx/${txid}`,
  },
  ltc: {
    amount: "14.32814722",
    decimals: 8,
    symbol: "LTC",
    explorer: (txid) => `https://blockchair.com/litecoin/transaction/${txid}`,
  },
};

type DemoTxSpec = {
  /** Days before "now" */
  daysAgo: number;
  /** Hour of day (UTC-ish, for variety) */
  hour: number;
  minute?: number;
  amount: string;
  direction: "in" | "out";
  fee?: string;
  /** Deterministic-looking hex seed fragment */
  seed: string;
};

/** Net ≈ 1284.471829147 XMR with a multi-month accumulation curve. */
const XMR_TXS: DemoTxSpec[] = [
  { daysAgo: 142, hour: 11, minute: 18, amount: "320.500000000", direction: "in", seed: "a1f3c8" },
  { daysAgo: 128, hour: 16, minute: 42, amount: "215.800000000", direction: "in", seed: "b29e01" },
  { daysAgo: 109, hour: 9, minute: 7, amount: "42.000000000", direction: "out", fee: "0.000084000", seed: "c4d712" },
  { daysAgo: 97, hour: 14, minute: 55, amount: "400.000000000", direction: "in", seed: "d8a945" },
  { daysAgo: 81, hour: 20, minute: 3, amount: "85.500000000", direction: "out", fee: "0.000091200", seed: "e01bc3" },
  { daysAgo: 64, hour: 8, minute: 29, amount: "175.250000000", direction: "in", seed: "f67d2a" },
  { daysAgo: 48, hour: 13, minute: 14, amount: "33.000000000", direction: "out", fee: "0.000078500", seed: "18af90" },
  { daysAgo: 31, hour: 17, minute: 48, amount: "98.421829147", direction: "in", seed: "29ce44" },
  { daysAgo: 19, hour: 10, minute: 22, amount: "150.000000000", direction: "in", seed: "3bd1ef" },
  { daysAgo: 7, hour: 15, minute: 6, amount: "85.000000000", direction: "in", seed: "4c02a7" },
  { daysAgo: 2, hour: 19, minute: 41, amount: "12.500000000", direction: "out", fee: "0.000102300", seed: "5d8831" },
  { daysAgo: 0, hour: 8, minute: 12, amount: "12.500000000", direction: "in", seed: "6e9fb8" },
];

/** Net ≈ 0.08742135 BTC — small side holding. */
const BTC_TXS: DemoTxSpec[] = [
  { daysAgo: 96, hour: 12, minute: 8, amount: "0.05000000", direction: "in", seed: "71a2c4" },
  { daysAgo: 61, hour: 18, minute: 33, amount: "0.03200000", direction: "in", seed: "82b3d5" },
  { daysAgo: 34, hour: 9, minute: 51, amount: "0.00500000", direction: "out", fee: "0.00001240", seed: "93c4e6" },
  { daysAgo: 15, hour: 21, minute: 4, amount: "0.01250000", direction: "in", seed: "a4d5f7" },
  { daysAgo: 4, hour: 14, minute: 27, amount: "0.00207865", direction: "out", fee: "0.00000890", seed: "b5e608" },
];

/** Net ≈ 14.32814722 LTC — small side holding. */
const LTC_TXS: DemoTxSpec[] = [
  { daysAgo: 88, hour: 10, minute: 15, amount: "8.50000000", direction: "in", seed: "c6f719" },
  { daysAgo: 52, hour: 7, minute: 44, amount: "4.20000000", direction: "in", seed: "d7082a" },
  { daysAgo: 27, hour: 16, minute: 2, amount: "1.00000000", direction: "out", fee: "0.00018000", seed: "e8193b" },
  { daysAgo: 11, hour: 12, minute: 38, amount: "3.00000000", direction: "in", seed: "f92a4c" },
  { daysAgo: 3, hour: 20, minute: 19, amount: "0.37185278", direction: "out", fee: "0.00009500", seed: "0a3b5d" },
];

const TXS_BY_CHAIN: Record<string, DemoTxSpec[]> = {
  xmr: XMR_TXS,
  btc: BTC_TXS,
  ltc: LTC_TXS,
};

function padHex(seed: string, len: number): string {
  let out = seed.replace(/[^0-9a-f]/gi, "").toLowerCase();
  while (out.length < len) {
    out += ((out.length * 17 + 13) % 16).toString(16);
  }
  return out.slice(0, len);
}

function fakeTxid(chain: string, seed: string, index: number): string {
  const body = padHex(`${seed}${chain}${index.toString(16)}`, 64);
  return body;
}

function tsFor(daysAgo: number, hour: number, minute = 0): string {
  const d = new Date();
  d.setUTCDate(d.getUTCDate() - daysAgo);
  d.setUTCHours(hour, minute, (daysAgo * 17 + hour) % 60, 0);
  return Math.floor(d.getTime() / 1000).toString();
}

function fakeUtxoAddress(chain: string, seed: string, index: number): string {
  const body = padHex(`addr${seed}${index}`, 33);
  if (chain === "btc") return `bc1q${body.slice(0, 38)}`;
  return `ltc1q${body.slice(0, 38)}`;
}

function buildTxRows(chain: string): TxRow[] {
  const meta = CHAINS[chain];
  const specs = TXS_BY_CHAIN[chain];
  if (!meta || !specs) return [];

  return specs.map((spec, i) => {
    const txid = fakeTxid(chain, spec.seed, i);
    const counterparty =
      spec.direction === "out" && chain !== "xmr"
        ? fakeUtxoAddress(chain, spec.seed, i)
        : null;

    return {
      txid,
      amount: spec.amount,
      symbol: meta.symbol,
      direction: spec.direction,
      timestamp: tsFor(spec.daysAgo, spec.hour, spec.minute ?? 0),
      status: "confirmed",
      fee: spec.fee ?? null,
      counterparty,
      explorer_url: meta.explorer(txid),
    } satisfies TxRow;
  });
}

/** First funded portfolio per demo chain gets the showcase balance. */
export function buildDemoBalances(portfolios: PortfolioRecord[]): PortfolioBalance[] {
  const claimed = new Set<string>();
  const out: PortfolioBalance[] = [];

  for (const p of portfolios) {
    const chain = p.chain.toLowerCase();
    const meta = CHAINS[chain];
    if (!meta) {
      out.push({
        portfolio_id: p.id,
        chain: p.chain,
        address: p.address ?? "",
        assets: [],
      });
      continue;
    }

    const isShowcase = !claimed.has(chain);
    if (isShowcase) claimed.add(chain);

    out.push({
      portfolio_id: p.id,
      chain: p.chain,
      address: p.address ?? `demo-${chain}-${p.id.slice(0, 8)}`,
      assets: isShowcase
        ? [
            {
              symbol: meta.symbol,
              amount: meta.amount,
              decimals: meta.decimals,
              usd: null,
            },
          ]
        : [
            {
              symbol: meta.symbol,
              amount: "0",
              decimals: meta.decimals,
              usd: null,
            },
          ],
    });
  }

  return out;
}

export type DemoActivityItem = {
  key: string;
  portfolioId: string;
  portfolioName: string;
  chain: string;
  tx: TxRow;
};

/**
 * Build overview ledger + recent activity for the first portfolio of each
 * demo chain. Duplicate chain wallets stay empty so totals stay coherent.
 */
export function buildDemoOverview(portfolios: PortfolioRecord[]): {
  ledger: LedgerEvent[];
  activity: DemoActivityItem[];
  historyByPortfolio: Map<string, TxRow[]>;
} {
  const claimed = new Set<string>();
  const historyByPortfolio = new Map<string, TxRow[]>();
  const activity: DemoActivityItem[] = [];
  let ledger: LedgerEvent[] = [];

  for (const p of portfolios) {
    const chain = p.chain.toLowerCase();
    if (!CHAINS[chain] || claimed.has(chain)) {
      historyByPortfolio.set(p.id, []);
      continue;
    }
    claimed.add(chain);

    const rows = buildTxRows(chain);
    historyByPortfolio.set(p.id, rows);
    ledger = ledger.concat(txsToLedger(rows));
    for (const tx of rows) {
      activity.push({
        key: `${p.id}:${tx.txid}`,
        portfolioId: p.id,
        portfolioName: p.name,
        chain: p.chain,
        tx,
      });
    }
  }

  activity.sort((a, b) => {
    const ta = Number(a.tx.timestamp) || 0;
    const tb = Number(b.tx.timestamp) || 0;
    return tb - ta;
  });

  return { ledger, activity, historyByPortfolio };
}

/** Resolve demo txs for a portfolio id given the full list (showcase-aware). */
export function demoHistoryForPortfolioId(
  portfolioId: string,
  portfolios: PortfolioRecord[],
): TxRow[] {
  const { historyByPortfolio } = buildDemoOverview(portfolios);
  return historyByPortfolio.get(portfolioId) ?? [];
}
