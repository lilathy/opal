import { useEffect, useMemo, useState } from "react";
import {
  getPriceCacheVersion,
  resolvePricesForFiat,
  subscribePriceCache,
} from "../lib/balances";

/** Live spot map for the selected display currency — updates when cache warms. */
export function useFiatPrices(fiat: string): Record<string, number> {
  const [tick, setTick] = useState(getPriceCacheVersion());

  useEffect(() => {
    return subscribePriceCache(() => setTick(getPriceCacheVersion()));
  }, []);

  return useMemo(() => resolvePricesForFiat(fiat), [fiat, tick]);
}
