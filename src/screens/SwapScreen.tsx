import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AssetIcon } from "../components/CryptoIcons";
import { SwapPick, type SwapPickGroup } from "../components/SwapPick";
import {
  api,
  parseInvokeError,
  type FixedFloatOrder,
  type PortfolioBalance,
  type PortfolioRecord,
  type SwapQuote,
} from "../lib/api";
import { formatAmount, formatMoney } from "../lib/format";
import { assetsOf } from "../lib/balances";
import { useNotify } from "../state/notifications";

function fiatSign(currency: string): string {
  try {
    const parts = new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: currency || "USD",
      currencyDisplay: "narrowSymbol",
    }).formatToParts(0);
    return parts.find((p) => p.type === "currency")?.value ?? "$";
  } catch {
    return "$";
  }
}

interface Props {
  portfolios: PortfolioRecord[];
  balances: PortfolioBalance[];
  fiat: string;
  discreet: boolean;
  onAddPortfolio: () => void;
}

/** Assets Opal can execute a swap for directly, signed in-app (Solana via
 * Jupiter). Everything else is a rate lookup that finishes on the partner
 * site — kept out of the UI's vocabulary, the app just picks quietly. */
const JUPITER_ASSETS = ["SOL", "USDC", "USDT"];
const PARTNER_NAME = "FixedFloat";

const EVM_CHAINS = new Set([
  "eth",
  "arb",
  "base",
  "polygon",
  "avax",
  "bsc",
  "gnosis",
  "linea",
]);

const ASSET_NAMES: Record<string, string> = {
  BTC: "Bitcoin",
  ETH: "Ethereum",
  SOL: "Solana",
  LTC: "Litecoin",
  XMR: "Monero",
  USDC: "USD Coin",
  USDT: "Tether",
  DAI: "Dai",
  DOGE: "Dogecoin",
  TRX: "Tron",
  TON: "Toncoin",
};

type Holding = {
  key: string;
  portfolioId: string;
  portfolioName: string;
  chain: string;
  symbol: string;
  amount: string;
  usd: number | null;
  software: boolean;
};

function assetName(symbol: string): string {
  return ASSET_NAMES[symbol.toUpperCase()] ?? symbol;
}

function formatPresetAmount(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "";
  if (n >= 1) return n.toFixed(6).replace(/0+$/, "").replace(/\.$/, "");
  return n.toFixed(8).replace(/0+$/, "").replace(/\.$/, "");
}

/** Tokens you can receive into a portfolio on this chain. */
function receiveAssetsForChain(chain: string): string[] {
  const c = chain.toLowerCase();
  if (c === "sol") return ["SOL", "USDC", "USDT"];
  if (c === "btc") return ["BTC"];
  if (c === "ltc") return ["LTC"];
  if (c === "doge") return ["DOGE"];
  if (c === "xmr") return ["XMR"];
  if (c === "trx") return ["TRX", "USDT"];
  if (c === "ton") return ["TON"];
  if (EVM_CHAINS.has(c)) return ["ETH", "USDC", "USDT", "DAI"];
  return [];
}

export function SwapScreen({ portfolios, balances, fiat, discreet, onAddPortfolio }: Props) {
  const { t } = useTranslation();
  const { notify } = useNotify();

  function showError(message: string) {
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message,
    });
  }

  const holdings = useMemo<Holding[]>(() => {
    const list: Holding[] = [];
    for (const p of portfolios) {
      const bal = balances.find((b) => b.portfolio_id === p.id);
      for (const a of assetsOf(bal)) {
        list.push({
          key: `${p.id}:${a.symbol}`,
          portfolioId: p.id,
          portfolioName: p.name,
          chain: p.chain,
          symbol: a.symbol,
          amount: a.amount,
          usd: a.usd,
          software: p.kind === "software",
        });
      }
    }
    return list.sort((a, b) => (b.usd ?? 0) - (a.usd ?? 0));
  }, [portfolios, balances]);

  const fromGroups = useMemo<SwapPickGroup[]>(() => {
    const byId = new Map<string, SwapPickGroup>();
    for (const h of holdings) {
      let g = byId.get(h.portfolioId);
      if (!g) {
        g = {
          portfolioId: h.portfolioId,
          portfolioName: h.portfolioName,
          chain: h.chain,
          assets: [],
        };
        byId.set(h.portfolioId, g);
      }
      g.assets.push({
        symbol: h.symbol,
        detail: formatAmount(h.amount, discreet, 8),
      });
    }
    return [...byId.values()];
  }, [holdings, discreet]);

  const [fromKey, setFromKey] = useState<string | null>(null);
  const [toKey, setToKey] = useState<string | null>(null);
  const [amount, setAmount] = useState("");
  const [amountUnit, setAmountUnit] = useState<"native" | "fiat">("native");
  const [quote, setQuote] = useState<SwapQuote | null>(null);
  const [pairMin, setPairMin] = useState<string | null>(null);
  const [quoting, setQuoting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [ffOrder, setFfOrder] = useState<FixedFloatOrder | null>(null);
  const [ffReady, setFfReady] = useState(false);

  useEffect(() => {
    if (fromGroups.length === 0) {
      setFromKey(null);
      return;
    }
    if (!fromKey || !fromGroups.some((g) => fromKey.startsWith(`${g.portfolioId}:`))) {
      const g = fromGroups[0];
      const preferred =
        g.assets.find((a) => a.symbol.toUpperCase() === g.chain.toUpperCase()) ?? g.assets[0];
      setFromKey(`${g.portfolioId}:${preferred.symbol}`);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fromGroups]);

  const from = holdings.find((h) => h.key === fromKey) ?? null;

  const toGroups = useMemo<SwapPickGroup[]>(() => {
    return portfolios
      .map((p) => {
        const assets = receiveAssetsForChain(p.chain)
          .filter((s) => {
            if (!from) return true;
            if (p.id === from.portfolioId && s === from.symbol.toUpperCase()) return false;
            return true;
          })
          .map((symbol) => ({ symbol }));
        return {
          portfolioId: p.id,
          portfolioName: p.name,
          chain: p.chain,
          assets,
        };
      })
      .filter((g) => g.assets.length > 0);
  }, [portfolios, from]);

  useEffect(() => {
    if (toGroups.length === 0) {
      setToKey(null);
      return;
    }
    const stillValid =
      toKey &&
      toGroups.some(
        (g) =>
          toKey.startsWith(`${g.portfolioId}:`) &&
          g.assets.some((a) => toKey === `${g.portfolioId}:${a.symbol}`),
      );
    if (!stillValid) {
      // Prefer a different portfolio than the source when possible.
      const alt = toGroups.find((g) => g.portfolioId !== from?.portfolioId) ?? toGroups[0];
      setToKey(`${alt.portfolioId}:${alt.assets[0].symbol}`);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [toGroups, from?.portfolioId]);

  const toPortfolioId = toKey?.slice(0, toKey.indexOf(":")) ?? null;
  const toSymbol = toKey && toKey.includes(":") ? toKey.slice(toKey.indexOf(":") + 1) : "";
  const toPortfolio = portfolios.find((p) => p.id === toPortfolioId) ?? null;

  const provider: "jupiter" | "fixedfloat" =
    from?.chain === "sol" &&
    toPortfolio?.chain === "sol" &&
    from.software &&
    JUPITER_ASSETS.includes(from.symbol.toUpperCase()) &&
    JUPITER_ASSETS.includes(toSymbol.toUpperCase())
      ? "jupiter"
      : "fixedfloat";

  useEffect(() => {
    void api.swapFixedfloatReady().then(setFfReady).catch(() => setFfReady(false));
  }, []);

  function selectFrom(portfolioId: string, symbol: string) {
    setFromKey(`${portfolioId}:${symbol}`);
    setAmount("");
    setAmountUnit("native");
    setQuote(null);
    setFfOrder(null);
    setPairMin(null);
  }

  function selectTo(portfolioId: string, symbol: string) {
    setToKey(`${portfolioId}:${symbol}`);
    setQuote(null);
    setFfOrder(null);
    setPairMin(null);
  }

  function setAmountPreset(fraction: number) {
    if (!from) return;
    const bal = Number(from.amount);
    if (!Number.isFinite(bal) || bal <= 0) return;
    if (amountUnit === "fiat") {
      const rate = unitRate(from);
      if (!rate) return;
      setAmount(formatPresetAmount(bal * fraction * rate));
    } else {
      setAmount(formatPresetAmount(bal * fraction));
    }
    setFfOrder(null);
  }

  function setMinFromPartner() {
    if (!from) return;
    const min = pairMin ? Number(pairMin) : NaN;
    if (Number.isFinite(min) && min > 0) {
      if (amountUnit === "fiat") {
        const rate = unitRate(from);
        if (!rate) return;
        setAmount(formatPresetAmount(min * rate));
      } else {
        setAmount(formatPresetAmount(min));
      }
      setFfOrder(null);
      return;
    }
    setAmountPreset(0.1);
  }

  function onAmountChange(raw: string) {
    const cleaned = raw.replace(/,/g, ".").replace(/[^\d.]/g, "");
    const parts = cleaned.split(".");
    const next = parts.length <= 1 ? cleaned : `${parts[0]}.${parts.slice(1).join("")}`;
    setAmount(next);
    setFfOrder(null);
  }

  function unitRate(h: Holding): number | null {
    const amt = Number(h.amount);
    if (!Number.isFinite(amt) || amt <= 0 || h.usd == null || !(h.usd > 0)) return null;
    return h.usd / amt;
  }

  function switchAmountUnit(next: "native" | "fiat") {
    if (next === amountUnit) return;
    if (!from || !amount) {
      setAmountUnit(next);
      return;
    }
    const n = Number(amount);
    const rate = unitRate(from);
    if (!Number.isFinite(n) || !rate) {
      setAmountUnit(next);
      setAmount("");
      return;
    }
    if (next === "fiat") {
      setAmount(formatPresetAmount(n * rate));
    } else {
      setAmount(formatPresetAmount(n / rate));
    }
    setAmountUnit(next);
  }

  const nativeAmount = useMemo(() => {
    if (!amount || !from) return "";
    const n = Number(amount);
    if (!Number.isFinite(n) || n <= 0) return "";
    if (amountUnit === "native") return amount;
    const rate = unitRate(from);
    if (!rate) return "";
    return formatPresetAmount(n / rate);
  }, [amount, amountUnit, from]);

  // Live quotes from FixedFloat (XML/API) or Jupiter as the amount changes.
  useEffect(() => {
    if (!from || !toSymbol || !toPortfolio) {
      setQuote(null);
      return;
    }
    let cancelled = false;
    const handle = window.setTimeout(() => {
      void (async () => {
        const probe = nativeAmount || "0";
        setQuoting(true);
        try {
          const q = await api.swapQuote(
            provider,
            from.symbol,
            toSymbol,
            probe,
            from.chain,
            toPortfolio.chain,
          );
          if (cancelled) return;
          if (q.minAmount) setPairMin(q.minAmount);
          if (nativeAmount) setQuote(q);
          else setQuote(null);
        } catch {
          if (!cancelled) {
            if (nativeAmount) setQuote(null);
          }
        } finally {
          if (!cancelled) setQuoting(false);
        }
      })();
    }, 420);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [nativeAmount, from, toSymbol, toPortfolio, provider]);

  async function swapJupiter() {
    if (!quote || !from) return;
    const fromPortfolio = portfolios.find((p) => p.id === from.portfolioId);
    const address = fromPortfolio?.address;
    if (!address) {
      showError(t("swap.needAddress"));
      return;
    }
    setBusy(true);
    try {
      await api.swapJupiterTx(quote.raw, address);
      notify({
        kind: "success",
        title: t("notifications.successTitle"),
        message: t("swap.txReady"),
      });
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function executeFixedFloatSwap() {
    if (!from || !toPortfolio || !toSymbol || !nativeAmount) return;
    if (!from.software) {
      showError(t("swap.needSoftware"));
      return;
    }
    setBusy(true);
    try {
      const result = await api.swapFixedfloatExecute({
        fromPortfolioId: from.portfolioId,
        toPortfolioId: toPortfolio.id,
        fromAsset: from.symbol,
        toAsset: toSymbol,
        amount: nativeAmount,
      });
      setFfOrder(result.order);
      setAmount("");
      setQuote(null);
      notify({
        kind: "success",
        title: t("notifications.successTitle"),
        message: t("swap.swapSubmitted", {
          id: result.order.id,
          txid: result.txid.slice(0, 10),
        }),
      });
    } catch (e) {
      const msg = parseInvokeError(e).message;
      if (/api key|api secret/i.test(msg)) {
        window.open(partnerUrl, "_blank", "noreferrer");
        return;
      }
      showError(msg);
    } finally {
      setBusy(false);
    }
  }

  function unitFiat(qty: number, holdingUsd: number | null, holdingAmt: string): number | null {
    if (!Number.isFinite(qty) || qty <= 0 || holdingUsd == null) return null;
    const amt = Number(holdingAmt);
    if (!Number.isFinite(amt) || amt <= 0) return null;
    return (holdingUsd / amt) * qty;
  }

  const amountNum = Number(nativeAmount);
  const hasAmount = nativeAmount !== "" && Number.isFinite(amountNum) && amountNum > 0;
  const overBalance =
    !!from && hasAmount && amountNum > Number(from.amount || "0") + 1e-12;
  const belowMin =
    !!pairMin &&
    hasAmount &&
    amountNum + 1e-12 < Number(pairMin);
  const quoteBlocked =
    !!quote?.errors?.some((e) => e === "LIMIT_MIN" || e === "LIMIT_MAX") || belowMin;

  const fromFiat =
    from && hasAmount ? unitFiat(amountNum, from.usd, from.amount) : null;
  // Receive fiat ≈ send fiat for a fair swap quote (same USD notionals).
  const toFiat =
    quote && fromFiat != null && Number(quote.toAmount) > 0 ? fromFiat : null;
  const toCrypto =
    quote && hasAmount && Number(quote.toAmount) > 0 ? quote.toAmount : null;

  const secondaryDisplay = (() => {
    if (amountUnit === "native") {
      return fromFiat != null
        ? formatMoney(fromFiat, fiat, discreet)
        : formatMoney(0, fiat, discreet);
    }
    return discreet
      ? "••••"
      : hasAmount
        ? `≈ ${formatAmount(nativeAmount, false, 8)} ${from?.symbol ?? ""}`
        : `≈ 0.00 ${from?.symbol ?? ""}`;
  })();

  const receivePrimary = (() => {
    if (quoting && hasAmount && !toCrypto) return "…";
    if (amountUnit === "fiat") {
      if (discreet) return "••••";
      return toFiat != null ? toFiat.toFixed(2) : "0.00";
    }
    if (toCrypto) return formatAmount(toCrypto, discreet, 8);
    return "0.00";
  })();

  const receiveSecondary = (() => {
    if (amountUnit === "fiat") {
      return discreet
        ? "••••"
        : toCrypto
          ? `≈ ${formatAmount(toCrypto, false, 8)} ${toSymbol}`
          : `≈ 0.00 ${toSymbol || ""}`;
    }
    return toFiat != null
      ? formatMoney(toFiat, fiat, discreet)
      : formatMoney(0, fiat, discreet);
  })();

  const receiveEmpty = amountUnit === "fiat" ? toFiat == null : !toCrypto;

  const partnerUrl =
    from && nativeAmount && toSymbol
      ? `https://ff.io/?from=${encodeURIComponent(from.symbol)}&to=${encodeURIComponent(
          toSymbol,
        )}&amount=${encodeURIComponent(nativeAmount)}`
      : "https://ff.io/";

  const ctaLabel = (() => {
    if (busy) return t("swap.swapping");
    if (!hasAmount) return t("swap.enterAmount");
    if (overBalance) return t("swap.overBalance");
    if (belowMin && pairMin) return t("swap.belowMin", { amount: pairMin, asset: from?.symbol ?? "" });
    if (quoting && !quote) return t("common.loading");
    if (provider === "jupiter") return t("swap.swapNow");
    if (ffReady) return t("swap.swapNow");
    return t("swap.continueOnPartner", { partner: PARTNER_NAME });
  })();

  function onCta() {
    if (!hasAmount || overBalance || belowMin || !from || busy) return;
    if (provider === "jupiter") {
      if (!quote) return;
      void swapJupiter();
      return;
    }
    if (ffReady) {
      void executeFixedFloatSwap();
      return;
    }
    window.open(partnerUrl, "_blank", "noreferrer");
  }

  if (fromGroups.length === 0) {
    return (
      <div className="content swap-page">
        <header className="settings-hero">
          <h2 className="settings-hero__title">{t("swap.title")}</h2>
          <p className="settings-hero__lede">{t("swap.disclaimer")}</p>
        </header>
        <div className="swap-empty">
          <p className="swap-empty__copy">{t("swap.empty")}</p>
          <button type="button" className="btn btn-primary" onClick={onAddPortfolio}>
            {t("swap.emptyCta")}
          </button>
        </div>
      </div>
    );
  }

  const fromLabel = from
    ? t("swap.iHave", {
        amount: formatAmount(from.amount, discreet, 8),
        asset: assetName(from.symbol),
      })
    : t("swap.from");
  const toLabel = t("swap.iWant", { asset: assetName(toSymbol || "—") });
  const canUseFiat = !!from && unitRate(from) != null;

  return (
    <div className="content swap-page">
      <header className="settings-hero">
        <h2 className="settings-hero__title">{t("swap.title")}</h2>
        <p className="settings-hero__lede">{t("swap.disclaimer")}</p>
      </header>

      <div className="swap-body anim-page">
        <div className="swap-panel">
          <section className="swap-leg">
            <div className="swap-leg__head">
              <p className="swap-leg__label">{fromLabel}</p>
              {canUseFiat ? (
                <div
                  className="segmented segmented--2 segmented--sm"
                  role="radiogroup"
                  aria-label={t("swap.amountUnit")}
                >
                  <button
                    type="button"
                    role="radio"
                    aria-checked={amountUnit === "native"}
                    className={`segmented__item${amountUnit === "native" ? " is-active" : ""}`}
                    onClick={() => switchAmountUnit("native")}
                  >
                    {from?.symbol}
                  </button>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={amountUnit === "fiat"}
                    className={`segmented__item${amountUnit === "fiat" ? " is-active" : ""}`}
                    onClick={() => switchAmountUnit("fiat")}
                  >
                    {fiat}
                  </button>
                </div>
              ) : null}
            </div>
            <div className="swap-leg__row">
              <SwapPick
                groups={fromGroups}
                value={fromKey}
                onChange={selectFrom}
                aria-label={t("swap.from")}
              />
              <div className="swap-leg__values">
                <div
                  className={`swap-amount-line${overBalance ? " is-invalid" : ""}`}
                >
                  <span className="swap-amount-prefix" aria-hidden="true">
                    {amountUnit === "fiat" ? (
                      <span className="swap-amount-prefix__sign">{fiatSign(fiat)}</span>
                    ) : from ? (
                      <AssetIcon symbol={from.symbol} size={22} />
                    ) : null}
                  </span>
                  <input
                    className="swap-amount-input"
                    inputMode="decimal"
                    placeholder="0.00"
                    value={amount}
                    onChange={(e) => onAmountChange(e.target.value)}
                    aria-label={t("swap.from")}
                    aria-invalid={overBalance || undefined}
                    size={Math.max(4, amount.length || 4)}
                  />
                </div>
                <span className="swap-amount-fiat">{secondaryDisplay}</span>
              </div>
            </div>
          </section>

          <section className="swap-leg">
            <div className="swap-leg__head">
              <p className="swap-leg__label">{toLabel}</p>
              {canUseFiat && toSymbol ? (
                <div
                  className="segmented segmented--2 segmented--sm"
                  role="radiogroup"
                  aria-label={t("swap.amountUnit")}
                >
                  <button
                    type="button"
                    role="radio"
                    aria-checked={amountUnit === "native"}
                    className={`segmented__item${amountUnit === "native" ? " is-active" : ""}`}
                    onClick={() => switchAmountUnit("native")}
                  >
                    {toSymbol}
                  </button>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={amountUnit === "fiat"}
                    className={`segmented__item${amountUnit === "fiat" ? " is-active" : ""}`}
                    onClick={() => switchAmountUnit("fiat")}
                  >
                    {fiat}
                  </button>
                </div>
              ) : null}
            </div>
            <div className="swap-leg__row">
              <SwapPick
                groups={toGroups}
                value={toKey}
                onChange={selectTo}
                aria-label={t("swap.to")}
              />
              <div className="swap-leg__values">
                <div className="swap-amount-line">
                  <span className="swap-amount-prefix" aria-hidden="true">
                    {amountUnit === "fiat" ? (
                      <span className="swap-amount-prefix__sign">{fiatSign(fiat)}</span>
                    ) : toSymbol ? (
                      <AssetIcon symbol={toSymbol} size={22} />
                    ) : null}
                  </span>
                  <span
                    className={`swap-amount-readout${receiveEmpty ? " is-empty" : ""}`}
                  >
                    {receivePrimary}
                  </span>
                </div>
                <span className="swap-amount-fiat">{receiveSecondary}</span>
              </div>
            </div>
          </section>
        </div>

        <div className="swap-presets" role="group" aria-label={t("swap.presets")}>
          <button type="button" className="swap-presets__btn" onClick={() => setMinFromPartner()}>
            {t("swap.min")}
          </button>
          <button type="button" className="swap-presets__btn" onClick={() => setAmountPreset(0.5)}>
            {t("swap.half")}
          </button>
          <button type="button" className="swap-presets__btn" onClick={() => setAmountPreset(1)}>
            {t("swap.all")}
          </button>
        </div>

        {quote && hasAmount && !quoteBlocked ? (
          <div className="swap-rate-line">
            <span>{t("swap.rate")}</span>
            <strong>
              1 {from?.symbol} ≈ {quote.rate} {toSymbol}
            </strong>
          </div>
        ) : null}

        {overBalance ? (
          <p className="swap-route-note swap-route-note--warn">{t("swap.overBalance")}</p>
        ) : belowMin && pairMin ? (
          <p className="swap-route-note swap-route-note--warn">
            {t("swap.belowMin", { amount: pairMin, asset: from?.symbol ?? "" })}
          </p>
        ) : pairMin && !hasAmount ? (
          <p className="swap-route-note swap-route-note--soft">
            {t("swap.minHint", { amount: pairMin, asset: from?.symbol ?? "" })}
          </p>
        ) : null}

        {ffOrder ? (
          <div className="swap-order">
            <div className="swap-rate-line">
              <span>{t("swap.orderId")}</span>
              <strong>{ffOrder.id}</strong>
            </div>
            <div className="swap-rate-line">
              <span>{t("swap.status")}</span>
              <strong>{ffOrder.status}</strong>
            </div>
            <p className="swap-route-note swap-route-note--soft">{t("swap.autoSubmitted")}</p>
            <a
              className="swap-route-note"
              href={ffOrder.orderUrl}
              target="_blank"
              rel="noreferrer"
            >
              {t("swap.openOrder")}
            </a>
          </div>
        ) : provider === "fixedfloat" && ffReady ? (
          <p className="swap-route-note swap-route-note--soft">{t("swap.oneClickNote")}</p>
        ) : provider === "fixedfloat" && !ffReady ? (
          <p className="swap-route-note swap-route-note--soft">
            {t("swap.partnerNote", { partner: PARTNER_NAME })}
          </p>
        ) : null}

        <button
          type="button"
          className="swap-cta"
          disabled={
            busy ||
            !hasAmount ||
            overBalance ||
            belowMin ||
            !from ||
            !toSymbol ||
            quoteBlocked ||
            (provider === "jupiter" && !quote)
          }
          onClick={onCta}
        >
          {ctaLabel}
        </button>
      </div>
    </div>
  );
}
