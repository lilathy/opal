import { api, type PortfolioBalance } from "./api";

/** Shared in-flight set so Shell + PortfolioDetail never double-scrape one id. */
const inFlight = new Set<string>();

/**
 * Live-scrape one portfolio. Concurrent callers for the same id share the
 * in-flight request instead of stacking RPCs.
 */
export async function scrapePortfolioBalance(
  portfolioId: string,
): Promise<PortfolioBalance | null> {
  if (inFlight.has(portfolioId)) return null;
  inFlight.add(portfolioId);
  try {
    const bals = await api.portfolioBalances(portfolioId);
    return bals.find((b) => b.portfolio_id === portfolioId) ?? bals[0] ?? null;
  } catch {
    return null;
  } finally {
    inFlight.delete(portfolioId);
  }
}

export function isBalanceScrapeInFlight(portfolioId: string): boolean {
  return inFlight.has(portfolioId);
}
