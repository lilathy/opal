import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { PortfolioBalance, PortfolioRecord } from "../lib/api";
import {
  assetFiatValue,
  assetsOf,
  pendingAmountCeiling,
} from "../lib/balances";
import { formatMoney, formatQty } from "../lib/format";
import { sendOsNotification } from "../lib/osNotification";
import { useNotify } from "../state/notifications";

function amountKey(portfolioId: string, symbol: string): string {
  return `${portfolioId}:${symbol.toUpperCase()}`;
}

/**
 * Detect incoming funds by diffing polled balances. Only fires after the first
 * baseline snapshot so startup / cache-to-live transitions stay silent.
 *
 * While an optimistic send is pending, amounts are capped to the post-spend
 * ceiling so a stale RPC snapshot can't look like a receive of the spent coins.
 */
export function useIncomingWatcher(
  balances: PortfolioBalance[],
  portfolios: PortfolioRecord[],
  enabled: boolean,
  discreet: boolean,
  fiat: string,
  prices: Record<string, number>,
) {
  const { t } = useTranslation();
  const { notify } = useNotify();
  const baselineReady = useRef(false);
  const prevAmounts = useRef<Map<string, number>>(new Map());

  useEffect(() => {
    if (!enabled) {
      baselineReady.current = false;
      prevAmounts.current = new Map();
      return;
    }

    const current = new Map<string, number>();
    for (const bal of balances) {
      for (const asset of assetsOf(bal)) {
        let amt = Number(asset.amount);
        if (!Number.isFinite(amt)) continue;
        const ceiling = pendingAmountCeiling(bal.portfolio_id, asset.symbol);
        if (ceiling != null && amt > ceiling) amt = ceiling;
        current.set(amountKey(bal.portfolio_id, asset.symbol), amt);
      }
    }

    if (!baselineReady.current) {
      prevAmounts.current = current;
      baselineReady.current = true;
      return;
    }

    const portfolioName = (id: string) =>
      portfolios.find((p) => p.id === id)?.name ?? t("notifications.portfolioFallback");

    for (const [key, newAmt] of current) {
      if (!prevAmounts.current.has(key)) continue;
      const oldAmt = prevAmounts.current.get(key) ?? 0;
      const delta = newAmt - oldAmt;
      if (delta <= 1e-12) continue;

      const colon = key.indexOf(":");
      const portfolioId = key.slice(0, colon);
      const symbol = key.slice(colon + 1);

      // Stale post-send scrape — ignore entirely.
      const ceiling = pendingAmountCeiling(portfolioId, symbol);
      if (ceiling != null && newAmt > ceiling + 1e-9) continue;

      const name = portfolioName(portfolioId);

      const title = t("notifications.incomingTitle");
      const amount = discreet
        ? "••••"
        : `+${formatQty(delta, symbol, false, 6)}`;

      const fiatRaw = assetFiatValue(
        { symbol, amount: String(delta), decimals: 8, usd: null },
        fiat,
        prices,
      );
      const fiatAmount =
        Number.isFinite(fiatRaw) && fiatRaw > 0
          ? formatMoney(fiatRaw, fiat, discreet)
          : undefined;

      notify({
        kind: "incoming",
        title,
        message: name,
        symbol,
        amount,
        fiatAmount,
      });

      if (document.hidden) {
        void sendOsNotification(
          title,
          discreet
            ? t("notifications.incomingDiscreet", { name })
            : fiatAmount
              ? t("notifications.incomingBodyFiat", {
                  amount: formatQty(delta, symbol, false, 6),
                  fiat: fiatAmount,
                  name,
                  defaultValue: `${formatQty(delta, symbol, false, 6)} (${fiatAmount}) in ${name}`,
                })
              : t("notifications.incomingBody", {
                  amount: formatQty(delta, symbol, false, 6),
                  name,
                }),
        );
      }
    }

    prevAmounts.current = current;
  }, [balances, portfolios, enabled, discreet, fiat, prices, notify, t]);
}
