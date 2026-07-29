import { api, type PortfolioBalance } from "./api";

/** Shared in-flight scrapes so Shell + PortfolioDetail await the same promise. */
const inFlight = new Map<string, Promise<PortfolioBalance | null>>();

/**
 * Live-scrape one portfolio. Concurrent callers for the same id share the
 * in-flight request instead of stacking RPCs or getting `null`.
 */
export async function scrapePortfolioBalance(
  portfolioId: string,
): Promise<PortfolioBalance | null> {
  const existing = inFlight.get(portfolioId);
  if (existing) return existing;

  const job = (async () => {
    try {
      const bals = await api.portfolioBalances(portfolioId);
      return bals.find((b) => b.portfolio_id === portfolioId) ?? bals[0] ?? null;
    } catch {
      return null;
    } finally {
      inFlight.delete(portfolioId);
    }
  })();

  inFlight.set(portfolioId, job);
  return job;
}
