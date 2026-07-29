import type { TxRow } from "./api";
import { coinIdForSymbol } from "./charts";

/** Fiat notional of a history row using the active spot map. */
export function txFiatValue(
  tx: Pick<TxRow, "amount" | "symbol">,
  prices: Record<string, number>,
): number {
  const qty = Number(tx.amount);
  if (!Number.isFinite(qty) || qty === 0) return 0;
  const coinId = coinIdForSymbol(tx.symbol);
  const px = coinId ? prices[coinId] : undefined;
  if (px == null || !Number.isFinite(px) || px <= 0) return 0;
  return Math.abs(qty) * px;
}

/**
 * Drop dust rows below `minFiat` (display currency).
 * Pending / failed rows still pass when they meet the threshold.
 * Rows with unknown price are kept so we don't hide real activity.
 */
export function filterDustTxs(
  rows: TxRow[],
  minFiat: number,
  prices: Record<string, number>,
): TxRow[] {
  const floor = Number.isFinite(minFiat) && minFiat > 0 ? minFiat : 0;
  if (floor <= 0) return rows;
  return rows.filter((tx) => {
    const coinId = coinIdForSymbol(tx.symbol);
    const px = coinId ? prices[coinId] : undefined;
    if (px == null || !Number.isFinite(px) || px <= 0) return true;
    return txFiatValue(tx, prices) + 1e-12 >= floor;
  });
}
