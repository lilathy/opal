import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import QRCodeStyling from "qr-code-styling";
import {
  api,
  canTrezorSend,
  invalidatePortfolioHistory,
  isUtxoChain,
  parseInvokeError,
  peekPortfolioHistory,
  type AddressBookEntry,
  type AddressSafety,
  type FeeEstimate,
  type FeePreset,
  type PortfolioBalance,
  type PortfolioRecord,
  type TxRow,
} from "../lib/api";
import { copyWithAutoClear } from "../lib/clipboard";
import { BalanceChart } from "../components/BalanceChart";
import { AssetIcon, ChainIcon, chainTint } from "../components/CryptoIcons";
import { ProcedureShell } from "../components/ProcedureShell";
import { Select } from "../components/Select";
import { TxHistory, type HistoryFilter } from "../components/TxHistory";
import { coinIdForChain, coinIdForSymbol, txsToLedger } from "../lib/charts";
import { useFiatPrices } from "../hooks/useFiatPrices";
import {
  applyOptimisticSpend,
  assetFiatValue,
  assetsOf,
  cacheFiatPriceMatrix,
  portfolioFiatSum,
  reconcilePendingSpend,
  rememberOptimisticSpend,
} from "../lib/balances";
import { scrapePortfolioBalance } from "../lib/balanceScrape";
import {
  chainLabel,
  formatAmount,
  formatMoney,
  formatQty,
} from "../lib/format";
import { useVault } from "../state/vault";
import { useNotify } from "../state/notifications";

type Tab = "balances" | "receive" | "send" | "history";
type SendStep = "to" | "amount" | "review";
const SEND_STEPS: SendStep[] = ["to", "amount", "review"];

function kindI18nKey(kind: string): string {
  if (kind === "software") return "portfolio.kindSoftware";
  if (kind === "trezor") return "portfolio.kindTrezor";
  if (kind === "watch_only") return "portfolio.kindWatch";
  return kind;
}

/** Short illustrative address-format hint shown as input placeholder text. */
function exampleAddressForChain(chain: string): string {
  switch (chain.toLowerCase()) {
    case "btc":
      return "bc1...";
    case "ltc":
      return "ltc1...";
    case "doge":
      return "D...";
    case "xmr":
      return "4...";
    case "trx":
      return "T...";
    case "sol":
      return "5...";
    case "ton":
      return "UQ...";
    default:
      // EVM-style chains: eth, arb, base, op, linea, polygon, avax, bsc, gnosis…
      return "0x...";
  }
}

interface Props {
  portfolio: PortfolioRecord;
  balance?: PortfolioBalance;
  initialTab?: Tab;
  trezorConnected?: boolean;
  onChanged: (opts?: { reloadBalances?: boolean }) => Promise<void>;
  onBalanceChange?: (balance: PortfolioBalance) => void;
}

function AddressEmphasis({ address }: { address: string }) {
  if (!address) return null;
  const chars = [...address];
  if (chars.length <= 12) {
    return <span className="addr-full">{address}</span>;
  }
  const prefix = chars.slice(0, 6).join("");
  const suffix = chars.slice(-6).join("");
  const mid = chars.slice(6, -6).join("");
  return (
    <span className="addr-display mono">
      <span className="addr-edge">{prefix}</span>
      <span className="addr-mid">{mid}</span>
      <span className="addr-edge">{suffix}</span>
    </span>
  );
}

export function PortfolioDetail({
  portfolio,
  balance: balanceProp,
  initialTab = "balances",
  trezorConnected = false,
  onChanged,
  onBalanceChange,
}: Props) {
  const { t } = useTranslation();
  const { status } = useVault();
  const { notify } = useNotify();
  const [tab, setTab] = useState<Tab>(initialTab);
  // The shared list only recomputes balances every ~2 minutes, so a portfolio
  // you just funded (or spent from) looks "empty"/stale until that timer
  // fires. Seed from the cached prop for an instant paint, then always kick
  // off a live, portfolio-scoped fetch so the open portfolio is never stuck
  // showing outdated numbers.
  const [balance, setBalance] = useState<PortfolioBalance | undefined>(balanceProp);
  const [balanceLoading, setBalanceLoading] = useState(!balanceProp);
  const [address, setAddress] = useState(balance?.address ?? portfolio.address ?? "");
  const [qrUri, setQrUri] = useState<string | null>(null);
  const qrContainerRef = useRef<HTMLDivElement>(null);
  const [history, setHistory] = useState<TxRow[]>(() => peekPortfolioHistory(portfolio.id) ?? []);
  const [historyLoading, setHistoryLoading] = useState(
    () => peekPortfolioHistory(portfolio.id) == null,
  );
  const [historyFilter, setHistoryFilter] = useState<HistoryFilter>("all");
  const [chartMode, setChartMode] = useState<"growth" | "price">("price");

  const [to, setTo] = useState("");
  const [amount, setAmount] = useState("");
  const [amountUnit, setAmountUnit] = useState<"native" | "fiat">("native");
  const [token, setToken] = useState("");
  const [feePreset, setFeePreset] = useState<FeePreset>("normal");
  const [feeEstimate, setFeeEstimate] = useState<FeeEstimate | null>(null);
  const [prices, setPrices] = useState<Record<string, number>>({});
  const [sendBalance, setSendBalance] = useState<PortfolioBalance | undefined>(undefined);
  const [addressBook, setAddressBook] = useState<AddressBookEntry[]>([]);
  const [safety, setSafety] = useState<AddressSafety | null>(null);
  const [addressChecking, setAddressChecking] = useState(false);
  const [sendMax, setSendMax] = useState(false);
  const [sendStep, setSendStep] = useState<SendStep>("to");
  const [sendDir, setSendDir] = useState<"forward" | "back">("forward");

  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  const utxo = isUtxoChain(portfolio.chain);
  const symbol = portfolio.chain.toUpperCase();
  const fiat = status?.fiat ?? "USD";
  const discreet = !!status?.discreet_mode;
  const fiatPrices = useFiatPrices(fiat);

  const canSpend =
    (portfolio.kind === "software" && portfolio.chain !== "ton") ||
    (portfolio.kind === "trezor" &&
      canTrezorSend(portfolio.chain) &&
      trezorConnected);
  const spendKey =
    portfolio.kind === "watch_only"
      ? "portfolio.spendWatch"
      : portfolio.kind === "trezor"
        ? !trezorConnected
          ? "portfolio.spendTrezorDisconnected"
          : canTrezorSend(portfolio.chain)
            ? "portfolio.spendTrezor"
            : "portfolio.spendTrezorUnsupported"
        : portfolio.chain === "ton"
          ? "portfolio.spendTonUnsupported"
          : "portfolio.spendSoftware";

  function showError(message: string) {
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message,
    });
  }

  function showSuccess(message: string) {
    notify({
      kind: "success",
      title: t("notifications.successTitle"),
      message,
    });
  }

  function showHistoryError(message: string) {
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message: message || t("portfolio.historyError"),
      action: {
        label: t("portfolio.txRetry"),
        onClick: () => void loadHistory(),
      },
    });
  }

  useEffect(() => {
    setTab(initialTab);
    setSendStep("to");
    setSendDir("forward");
    setBalance(balanceProp);
    setBalanceLoading(!balanceProp);
    setAddress(balanceProp?.address ?? portfolio.address ?? "");
    setQrUri(null);
    setHistory([]);
    setChartMode("price");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [portfolio.id, initialTab]);

  useEffect(() => {
    if (!balanceProp) {
      setBalance(undefined);
      return;
    }
    setBalance(reconcilePendingSpend(balanceProp));
  }, [balanceProp]);

  useEffect(() => {
    if (!status?.fiat) return;
    void api.spotPricesSnapshot().then((matrix) => {
      if (matrix && Object.keys(matrix).length > 0) {
        cacheFiatPriceMatrix(matrix);
        const px = matrix[status.fiat.toLowerCase()] ?? matrix[status.fiat.toUpperCase()];
        if (px) setPrices(px);
      }
    });
  }, [status?.fiat]);

  async function refreshLiveBalance(isCancelled?: () => boolean) {
    const started = Date.now();
    try {
      const mine = await scrapePortfolioBalance(portfolio.id);
      if (isCancelled?.()) return;
      if (mine) {
        const next = reconcilePendingSpend(mine);
        setBalance(next);
        onBalanceChange?.(next);
      }
    } catch {
      // Best-effort — keep whatever cached/prop balance we already have.
    } finally {
      if (!isCancelled?.()) setBalanceLoading(false);
      void started;
    }
  }

  // One scrape on open. Ongoing updates come from Shell's poll via balanceProp
  // — a second 1.5s loop here was doubling RPC load on the same portfolio.
  useEffect(() => {
    let cancelled = false;
    void refreshLiveBalance(() => cancelled);
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [portfolio.id]);

  async function loadReceive(isCancelled?: () => boolean) {
    try {
      const [addr, uri] = await Promise.all([
        api.portfolioReceiveAddress(portfolio.id),
        api.portfolioReceiveUri(portfolio.id),
      ]);
      if (isCancelled?.()) return;
      setAddress(addr);
      setQrUri(uri);
    } catch (e) {
      if (isCancelled?.()) return;
      showError(parseInvokeError(e).message);
    }
  }

  useEffect(() => {
    if (tab !== "receive") return;
    let cancelled = false;
    void loadReceive(() => cancelled);
    return () => {
      cancelled = true;
    };
  }, [tab, portfolio.id]);

  // Draws (or redraws) the styled QR into the container — a plain effect
  // rather than an imperative library ref, so it survives the receive tab
  // unmounting/remounting (e.g. tab switches) and address refreshes alike.
  useEffect(() => {
    if (tab !== "receive" || !qrUri) return;
    const container = qrContainerRef.current;
    if (!container) return;
    container.innerHTML = "";
    const qr = new QRCodeStyling({
      width: 176,
      height: 176,
      type: "svg",
      data: qrUri,
      margin: 4,
      qrOptions: { errorCorrectionLevel: "M" },
      dotsOptions: { type: "extra-rounded", color: "#ede6dc" },
      cornersSquareOptions: { type: "extra-rounded", color: "#ede6dc" },
      cornersDotOptions: { type: "dot", color: "#ede6dc" },
      backgroundOptions: { color: "transparent" },
    });
    qr.append(container);
    return () => {
      container.innerHTML = "";
    };
  }, [tab, qrUri]);

  // Fetch history as soon as the portfolio opens — the growth chart needs
  // real txs to reconstruct balances over time (not just the History tab).
  useEffect(() => {
    let cancelled = false;
    const cached = peekPortfolioHistory(portfolio.id);
    if (cached) {
      setHistory(cached);
      setHistoryLoading(false);
    } else {
      setHistoryLoading(true);
    }
    void (async () => {
      try {
        const rows = await api.portfolioHistory(portfolio.id);
        if (!cancelled) setHistory(rows);
      } catch (e) {
        if (!cancelled) {
          showHistoryError(parseInvokeError(e).message);
          // Empty ledger still tells the chart "history loaded; don't fake
          // a week of holdings" — better a flat-from-zero than a lie.
          if (!cached) setHistory([]);
        }
      } finally {
        if (!cancelled) setHistoryLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [portfolio.id]);

  useEffect(() => {
    if (tab !== "send") return;
    let cancelled = false;
    void (async () => {
      try {
        const [book, px, fee, bals] = await Promise.all([
          api.addressBookList().catch(() => [] as AddressBookEntry[]),
          api.pricesFiat().catch(() => ({}) as Record<string, number>),
          utxo
            ? api.portfolioEstimateFee(portfolio.id, feePreset).catch(() => null)
            : Promise.resolve(null),
          // Scoped (not the shared list) fetch, which includes zero-balance
          // allowlisted tokens (DAI, USDC, USDT, …) — otherwise a token
          // you've never held would never appear as something to pick here.
          api.portfolioBalances(portfolio.id).catch(() => [] as PortfolioBalance[]),
        ]);
        if (cancelled) return;
        setAddressBook(book.filter((e) => e.chain === portfolio.chain || !e.chain));
        setPrices(px);
        setFeeEstimate(fee);
        setSendBalance(bals.find((b) => b.portfolio_id === portfolio.id));
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tab, portfolio.id, portfolio.chain, feePreset, utxo]);

  useEffect(() => {
    if (tab !== "send") return;
    if (!to.trim()) {
      setSafety(null);
      setAddressChecking(false);
      return;
    }
    let cancelled = false;
    setAddressChecking(true);
    const id = window.setTimeout(() => {
      void api
        .analyzeSendAddress(to, portfolio.chain)
        .then((s) => {
          if (!cancelled) setSafety(s);
        })
        .catch(() => {
          if (!cancelled) {
            setSafety({
              ok: false,
              warnings: [
                t("portfolio.sendAddressCheckFailed", {
                  defaultValue: "Couldn't verify this address.",
                }),
              ],
              display_prefix: "",
              display_suffix: "",
              display_middle_masked: to.trim(),
            });
          }
        })
        .finally(() => {
          if (!cancelled) setAddressChecking(false);
        });
    }, 280);
    return () => {
      cancelled = true;
      window.clearTimeout(id);
    };
  }, [tab, to, portfolio.chain, t]);

  useEffect(() => {
    if (sendStep !== "review" || !safety?.ok || safety.warnings.length === 0) return;
    notify({
      kind: "warning",
      title: t("notifications.warningTitle"),
      message: safety.warnings.join(" · "),
    });
  }, [sendStep, safety, notify, t]);

  async function loadHistory() {
    setHistoryLoading(true);
    invalidatePortfolioHistory(portfolio.id);
    try {
      setHistory(await api.portfolioHistory(portfolio.id));
    } catch (e) {
      showHistoryError(parseInvokeError(e).message);
    } finally {
      setHistoryLoading(false);
    }
  }

  async function copyAddress() {
    await copyWithAutoClear(address, 30_000);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 2000);
  }

  async function nextAddress() {
    setBusy(true);
    try {
      const addr = await api.portfolioNextAddress(portfolio.id);
      setAddress(addr);
      await loadReceive();
      await onChanged();
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  // The asset actually being sent — native chain symbol, or a picked token.
  const selectedSymbol = token || symbol;

  const selectedBalance = useMemo(
    () =>
      (sendBalance ? assetsOf(sendBalance) : assetsOf(balance)).find(
        (a) => a.symbol.toUpperCase() === selectedSymbol.toUpperCase(),
      ),
    [sendBalance, balance, selectedSymbol],
  );

  const rate = useMemo(() => {
    if (selectedBalance) {
      const amt = Number(selectedBalance.amount);
      if (Number.isFinite(amt) && amt > 0 && selectedBalance.usd != null) {
        return selectedBalance.usd / amt;
      }
    }
    // Zero-balance (or newly-created) portfolios have nothing to derive a
    // rate from above — fall back to the live spot feed, keyed by CoinGecko
    // id (what `api.pricesFiat()` actually returns), not by raw chain/ticker
    // strings which never matched those keys.
    const coinId = coinIdForSymbol(selectedSymbol) ?? coinIdForChain(portfolio.chain);
    return coinId ? (prices[coinId] ?? null) : null;
  }, [prices, portfolio.chain, selectedSymbol, selectedBalance]);

  const maxNative = selectedBalance?.amount ?? "0";
  const maxFiat = selectedBalance?.usd ?? 0;

  /** Spendable native units after reserving an estimated network fee (UTXO native only). */
  const spendableNative = useMemo(() => {
    const bal = Number(maxNative);
    if (!Number.isFinite(bal) || bal <= 0) return 0;
    const sendingNative = !token;
    if (utxo && sendingNative && feeEstimate?.feeSats != null) {
      const fee = feeEstimate.feeSats / 1e8;
      return Math.max(0, bal - fee);
    }
    return bal;
  }, [maxNative, utxo, token, feeEstimate]);

  const nativeAmount = useMemo(() => {
    if (!amount) return "";
    const n = Number(amount);
    if (!Number.isFinite(n)) return "";
    if (n < 0) return "";
    if (amountUnit === "fiat") {
      if (!rate) return "";
      return (n / rate).toFixed(8).replace(/0+$/, "").replace(/\.$/, "");
    }
    return amount;
  }, [amount, amountUnit, rate]);

  const amountNum = Number(nativeAmount);
  const exceedsBalance =
    Number.isFinite(amountNum) && amountNum > 0 && amountNum > spendableNative + 1e-12;
  const addressValid = !!to.trim() && !!safety?.ok && !addressChecking;
  const amountValid =
    !!nativeAmount && Number.isFinite(amountNum) && amountNum > 0 && !exceedsBalance;

  const fiatHelper = useMemo(() => {
    if (!amount || !rate) return null;
    const n = Number(amount);
    if (!Number.isFinite(n)) return null;
    if (amountUnit === "fiat") {
      const native = n / rate;
      return `≈ ${native.toFixed(8).replace(/0+$/, "").replace(/\.$/, "")} ${selectedSymbol}`;
    }
    return `≈ ${(n * rate).toFixed(2)} ${fiat}`;
  }, [amount, amountUnit, rate, fiat, selectedSymbol]);

  function fillMax() {
    setSendMax(utxo && !token);
    if (amountUnit === "fiat") {
      const fiatMax = rate ? spendableNative * rate : maxFiat;
      if (fiatMax > 0) setAmount(fiatMax.toFixed(2));
      return;
    }
    if (spendableNative > 0) {
      const raw =
        spendableNative < 1
          ? spendableNative.toFixed(8)
          : String(spendableNative);
      setAmount(raw.replace(/0+$/, "").replace(/\.$/, ""));
    }
  }

  function onAmountChange(raw: string) {
    setSendMax(false);
    // Allow digits + one decimal separator only.
    const cleaned = raw.replace(/,/g, ".").replace(/[^\d.]/g, "");
    const parts = cleaned.split(".");
    const next =
      parts.length <= 1 ? cleaned : `${parts[0]}.${parts.slice(1).join("")}`;
    setAmount(next);
  }

  async function submitSend() {
    if (!addressValid || !amountValid) return;
    setBusy(true);
    try {
      const sendAmt = Number(nativeAmount);
      await api.portfolioSend({
        portfolioId: portfolio.id,
        to: to.trim(),
        amount: nativeAmount,
        token: token || null,
        feePreset: utxo ? feePreset : null,
        sendMax: sendMax && utxo && !token ? true : null,
      });
      // Instant client-side debit — don't wait for RPC scrape to show the spend.
      if (balance && Number.isFinite(sendAmt) && sendAmt > 0) {
        const feeNative =
          utxo && feeEstimate?.feeSats != null
            ? feeEstimate.feeSats / 1e8
            : portfolio.chain === "sol" && !token
              ? 0.000005
              : 0;
        const next = applyOptimisticSpend(balance, {
          assetSymbol: token || symbol,
          amount: sendAmt,
          feeNative,
          nativeSymbol: symbol,
        });
        rememberOptimisticSpend(next);
        setBalance(next);
        setSendBalance(next);
        onBalanceChange?.(next);
      }
      setSendStep("to");
      setSendDir("forward");
      setTo("");
      setAmount("");
      setSendMax(false);
      invalidatePortfolioHistory(portfolio.id);
      void loadHistory();
      // Refresh portfolio list/history, but skip an immediate live scrape —
      // RPC often still returns the pre-send balance and desyncs the sidebar.
      void onChanged({ reloadBalances: false });
      window.setTimeout(() => {
        void refreshLiveBalance();
      }, 12_000);
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function bumpFee(txid: string) {
    setBusy(true);
    try {
      const res = await api.portfolioBumpFee(portfolio.id, txid, "priority");
      showSuccess(res.txid);
      await loadHistory();
      await onChanged();
      await refreshLiveBalance();
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  const portfolioTotal = useMemo(() => {
    return formatMoney(portfolioFiatSum(balance, fiat, fiatPrices), fiat, discreet);
  }, [balance, discreet, fiat, fiatPrices]);

  const primaryQty = useMemo(() => {
    const assets = assetsOf(balance);
    const primary =
      assets.find((a) => a.symbol.toUpperCase() === symbol) ?? assets[0];
    if (!primary) return formatQty(0, symbol, discreet);
    return formatQty(primary.amount, primary.symbol, discreet);
  }, [balance, discreet, symbol]);

  const chartHoldings = useMemo(() => {
    const map = new Map<string, number>();
    for (const a of assetsOf(balance)) {
      const id = coinIdForSymbol(a.symbol) ?? coinIdForChain(portfolio.chain);
      if (!id) continue;
      const amt = Number(a.amount);
      if (!Number.isFinite(amt) || amt <= 0) continue;
      map.set(id, (map.get(id) ?? 0) + amt);
    }
    return [...map.entries()].map(([coinId, amount]) => ({ coinId, amount }));
  }, [balance, portfolio.chain]);

  const chartLedger = useMemo(() => txsToLedger(history), [history]);
  // `null` only while history is still fetching. Empty txs → `[]` (noData),
  // never keep growth mode spinning forever / never invent MTM.
  // Held coin with no matching ledger events → empty (noData), not a 0→full tip spike.
  const growthLedger: ReturnType<typeof txsToLedger> | null = useMemo(() => {
    if (historyLoading) return null;
    const heldMissing = chartHoldings.some(
      (h) => h.amount > 0 && !chartLedger.some((e) => e.coinId === h.coinId),
    );
    if (heldMissing) return [];
    return chartLedger;
  }, [historyLoading, chartHoldings, chartLedger]);

  const assetOptions = useMemo(() => {
    const assets = sendBalance ? assetsOf(sendBalance) : assetsOf(balance);
    const nativeAsset = assets.find((a) => a.symbol.toUpperCase() === symbol);
    const nativeOpt = {
      value: "",
      label: nativeAsset?.symbol ?? symbol,
      leading: <AssetIcon symbol={nativeAsset?.symbol ?? symbol} size={28} />,
    };
    const tokenOpts = assets
      .filter((a) => a.symbol.toUpperCase() !== symbol)
      .map((a) => ({
        value: a.symbol,
        label: a.symbol,
        leading: <AssetIcon symbol={a.symbol} size={28} />,
      }));
    return [nativeOpt, ...tokenOpts];
  }, [sendBalance, balance, symbol]);

  const hasTokens = assetOptions.length > 1;

  const priceHoldings = useMemo(() => {
    const id = coinIdForSymbol(symbol) ?? coinIdForChain(portfolio.chain);
    if (!id) return [];
    return [{ coinId: id, amount: 1 }];
  }, [symbol, portfolio.chain]);

  function goTab(k: Tab) {
    setTab(k);
    setSendStep("to");
    setSendDir("forward");
  }

  function sendGoBack() {
    const idx = SEND_STEPS.indexOf(sendStep);
    setSendDir("back");
    if (idx <= 0) return;
    setSendStep(SEND_STEPS[idx - 1]);
  }

  function sendGoNext() {
    if (sendStep === "to") {
      if (!canSpend || !addressValid) return;
      setSendDir("forward");
      setSendStep("amount");
      return;
    }
    if (sendStep === "amount") {
      if (!amountValid) return;
      setSendDir("forward");
      setSendStep("review");
      return;
    }
    void submitSend();
  }

  return (
    <div className={`content content-wide asset-page${tab === "history" ? " asset-page--history" : ""}`}>
      <div className="asset-top">
        <span
          className="crypto-badge anim-icon"
          style={{ ["--chain-tint" as string]: chainTint(portfolio.chain) }}
        >
          <ChainIcon chain={portfolio.chain} size={tab === "history" ? 40 : 48} />
        </span>
        <div className="asset-top__copy">
          <h2 className="portfolio-title">{portfolio.name}</h2>
          <p className="meta-line">
            {chainLabel(portfolio.chain)} · {t(kindI18nKey(portfolio.kind))}
          </p>
        </div>
      </div>

      <div className="asset-price">
        <p className="asset-price__fiat anim-balance">{portfolioTotal}</p>
        {tab !== "history" ? <p className="asset-price__qty">{primaryQty}</p> : null}
        {!canSpend && portfolio.kind !== "watch_only" && tab !== "history" ? (
          <p className="spend-note no-spend">{t(spendKey)}</p>
        ) : null}
      </div>

      {tab !== "history" ? (
        <div className="asset-page-chart">
          <BalanceChart
            holdings={chartMode === "price" ? priceHoldings : chartHoldings}
            ledger={chartMode === "growth" ? growthLedger : undefined}
            fiat={fiat}
            discreet={discreet}
            height={160}
            leadingControl={
              <div className="chart-mode-toggle" role="tablist">
                <button
                  type="button"
                  role="tab"
                  aria-selected={chartMode === "price"}
                  className={`chart-mode-toggle__item${chartMode === "price" ? " is-active" : ""}`}
                  onClick={() => setChartMode("price")}
                >
                  {symbol}
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={chartMode === "growth"}
                  className={`chart-mode-toggle__item${chartMode === "growth" ? " is-active" : ""}`}
                  onClick={() => setChartMode("growth")}
                >
                  {t("chart.portfolioGrowth", { defaultValue: "Portfolio" })}
                </button>
              </div>
            }
          />
        </div>
      ) : null}

      <nav className="nav-tabs secondary" aria-label={t("portfolio.tabsLabel")}>
        {(["balances", "receive", "send", "history"] as const).map((k) => (
          <button
            key={k}
            type="button"
            className={`nav-tab${tab === k ? " is-active" : ""}`}
            onClick={() => goTab(k)}
          >
            {t(`portfolio.tab.${k}`)}
          </button>
        ))}
      </nav>

      {tab === "balances" ? (
        <div>
          {balanceLoading && assetsOf(balance).length === 0 ? (
            <div className="asset-list">
              <div className="asset-row asset-row--skeleton" />
              <div className="asset-row asset-row--skeleton" />
            </div>
          ) : assetsOf(balance).length === 0 ? (
            <p className="muted">{t("portfolio.noBalances")}</p>
          ) : (
            <div className="asset-list anim-stagger">
              {assetsOf(balance).map((a) => (
                <div key={a.symbol} className="asset-row">
                  <span className="asset-row-left">
                    <span className="crypto-badge">
                      <AssetIcon symbol={a.symbol} size={32} />
                    </span>
                    <span className="sym">{a.symbol}</span>
                  </span>
                  <span className="amt">
                    {formatAmount(a.amount, discreet)}
                    <span className="fiat">{formatMoney(assetFiatValue(a, fiat, fiatPrices), fiat, discreet)}</span>
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      ) : null}

      {tab === "receive" ? (
        <div className="stack receive-tab">
          <div className="receive-tab__grid">
            {qrUri ? (
              <div className="qr-code qr-code--sm" ref={qrContainerRef} />
            ) : null}
            <div className="receive-tab__addr">
              <AddressEmphasis address={address} />
              <div className="row" style={{ flexWrap: "wrap" }}>
                <button
                  type="button"
                  className="btn btn-primary"
                  onClick={() => void copyAddress()}
                >
                  {copied ? t("portfolio.copied") : t("portfolio.copy")}
                </button>
                {utxo && portfolio.kind === "software" ? (
                  <button
                    type="button"
                    className="btn"
                    disabled={busy}
                    onClick={() => void nextAddress()}
                  >
                    {t("portfolio.nextAddress")}
                  </button>
                ) : null}
              </div>
            </div>
          </div>
        </div>
      ) : null}

      {tab === "send" ? (
        <div className="stack">
          {!canSpend && portfolio.kind !== "watch_only" ? (
            <p className="section-desc" style={{ marginTop: 0 }}>
              {t(spendKey)}
            </p>
          ) : null}
          <ProcedureShell
            ariaLabel={t("portfolio.send")}
            direction={sendDir}
            activeId={sendStep}
            steps={[
              { id: "to", label: t("portfolio.sendStepTo") },
              { id: "amount", label: t("portfolio.sendStepAmount") },
              { id: "review", label: t("portfolio.sendStepReview") },
            ]}
          >
            {sendStep === "to" ? (
              <>
                <p className="section-desc" style={{ marginTop: 0 }}>
                  {t("portfolio.sendToHint")}
                </p>
                <div className="field">
                  <label>{t("portfolio.to")}</label>
                  <input
                    value={to}
                    onChange={(e) => setTo(e.target.value)}
                    disabled={!canSpend}
                    placeholder={exampleAddressForChain(portfolio.chain)}
                    spellCheck={false}
                    autoComplete="off"
                    autoFocus
                    className={
                      to.trim() && safety && !safety.ok ? "is-invalid" : undefined
                    }
                    aria-invalid={to.trim() && safety ? !safety.ok : undefined}
                  />
                </div>
                {addressBook.length > 0 ? (
                  <Select
                    label={t("portfolio.addressBook")}
                    value=""
                    placeholder={t("portfolio.pickContact")}
                    onChange={(id) => {
                      const entry = addressBook.find((x) => x.id === id);
                      if (entry) setTo(entry.address);
                    }}
                    options={addressBook.map((e) => ({
                      value: e.id,
                      label: `${e.label} · ${e.address.slice(0, 10)}...`,
                    }))}
                  />
                ) : null}
                {to.trim() && addressChecking ? (
                  <p className="field-hint">{t("portfolio.sendAddressChecking")}</p>
                ) : null}
                {safety ? (
                  <div
                    className={`safety-box${safety.ok ? "" : " safety-error"}${
                      safety.ok && safety.warnings.length > 0 ? " safety-warn" : ""
                    }`}
                  >
                    <AddressEmphasis address={to.trim()} />
                    {safety.warnings.map((w) => (
                      <p
                        key={w}
                        className="field-hint"
                        style={{
                          color: safety.ok ? "var(--warning)" : "var(--negative)",
                        }}
                      >
                        {w}
                      </p>
                    ))}
                    {safety.ok && safety.warnings.length === 0 ? (
                      <p className="field-hint" style={{ color: "var(--positive)" }}>
                        {t("portfolio.sendAddressOk")}
                      </p>
                    ) : null}
                  </div>
                ) : null}
              </>
            ) : null}

            {sendStep === "amount" ? (
              <>
                <p className="section-desc" style={{ marginTop: 0 }}>
                  {t("portfolio.sendAmountHint")}
                </p>
                {hasTokens ? (
                  <Select
                    className="select--asset"
                    label={t("portfolio.asset", { defaultValue: "Asset" })}
                    value={token}
                    onChange={(v) => {
                      setToken(v);
                      setSendMax(false);
                      setAmount("");
                    }}
                    options={assetOptions}
                  />
                ) : null}
                <div className="field">
                  <div className="field-label-row">
                    <label>{t("portfolio.amount")}</label>
                    {rate ? (
                      <div
                        className="segmented segmented--2 segmented--sm"
                        role="radiogroup"
                        aria-label={t("portfolio.amountUnit", { defaultValue: "Amount unit" })}
                      >
                        {(["native", "fiat"] as const).map((u) => (
                          <button
                            key={u}
                            type="button"
                            role="radio"
                            aria-checked={amountUnit === u}
                            className={`segmented__item${amountUnit === u ? " is-active" : ""}`}
                            onClick={() => {
                              setAmountUnit(u);
                              setSendMax(false);
                              setAmount("");
                            }}
                          >
                            {u === "native" ? selectedSymbol : fiat}
                          </button>
                        ))}
                      </div>
                    ) : null}
                  </div>
                  <div className={`amount-input-group${exceedsBalance ? " is-invalid" : ""}`}>
                    <span className="amount-input-group__unit">
                      {amountUnit === "native" ? selectedSymbol : fiat}
                    </span>
                    <input
                      value={amount}
                      inputMode="decimal"
                      placeholder="0"
                      onChange={(e) => onAmountChange(e.target.value)}
                      autoFocus
                      aria-invalid={exceedsBalance || undefined}
                    />
                  </div>
                  <div className="field-hint-row">
                    {exceedsBalance ? (
                      <p className="field-hint field-hint--error">
                        {t("portfolio.sendExceedsBalance")}
                      </p>
                    ) : fiatHelper ? (
                      <p className="field-hint">{fiatHelper}</p>
                    ) : (
                      <span />
                    )}
                    <button
                      type="button"
                      className="max-balance-btn"
                      onClick={fillMax}
                      disabled={spendableNative <= 0}
                    >
                      {t("portfolio.maxBalance", { defaultValue: "Max" })}:{" "}
                      {amountUnit === "native"
                        ? formatQty(spendableNative, selectedSymbol, discreet)
                        : formatMoney(
                            rate ? spendableNative * rate : maxFiat,
                            fiat,
                            discreet,
                          )}
                    </button>
                  </div>
                </div>
                {utxo ? (
                  <div className="field">
                    <label>{t("portfolio.feePreset")}</label>
                    <div className="segmented" role="radiogroup" aria-label={t("portfolio.feePreset")}>
                      {(["economy", "normal", "priority"] as const).map((p) => (
                        <button
                          key={p}
                          type="button"
                          role="radio"
                          aria-checked={feePreset === p}
                          className={`segmented__item${feePreset === p ? " is-active" : ""}`}
                          onClick={() => setFeePreset(p)}
                        >
                          {t(`portfolio.fee.${p}`)}
                        </button>
                      ))}
                    </div>
                    {feeEstimate?.feeSats != null ? (
                      <p className="field-hint">
                        {t("portfolio.feeEstimate", { sats: feeEstimate.feeSats })}
                      </p>
                    ) : null}
                  </div>
                ) : null}
              </>
            ) : null}

            {sendStep === "review" ? (
              <>
                <h3 style={{ margin: 0 }}>{t("portfolio.reviewTitle")}</h3>
                <p className="section-desc">{t("portfolio.reviewHint")}</p>
                <div className="review-grid">
                  <span>{t("portfolio.to")}</span>
                  <AddressEmphasis address={to.trim()} />
                  <span>{t("portfolio.asset", { defaultValue: "Asset" })}</span>
                  <strong>{selectedSymbol}</strong>
                  <span>{t("portfolio.amount")}</span>
                  <strong>
                    {amountUnit === "fiat"
                      ? `${amount} ${fiat} (≈ ${nativeAmount} ${selectedSymbol})`
                      : `${amount} ${selectedSymbol}`}
                  </strong>
                  {utxo ? (
                    <>
                      <span>{t("portfolio.feePreset")}</span>
                      <strong>{t(`portfolio.fee.${feePreset}`)}</strong>
                    </>
                  ) : null}
                </div>
              </>
            ) : null}

            <div className="row" style={{ marginTop: 8 }}>
              <button
                type="button"
                className="btn btn-primary"
                disabled={
                  busy ||
                  !canSpend ||
                  (sendStep === "to" && !addressValid) ||
                  (sendStep === "amount" && !amountValid) ||
                  (sendStep === "review" && (!addressValid || !amountValid))
                }
                onClick={() => sendGoNext()}
              >
                {sendStep === "review"
                  ? busy && portfolio.kind === "trezor"
                    ? t("portfolio.trezorConfirmHint", {
                        defaultValue: "Confirm on your Trezor…",
                      })
                    : t("portfolio.confirmSend")
                  : t("portfolio.sendContinue")}
              </button>
              {sendStep !== "to" ? (
                <button
                  type="button"
                  className="btn btn-ghost"
                  disabled={busy}
                  onClick={sendGoBack}
                >
                  {t("common.back")}
                </button>
              ) : null}
            </div>
          </ProcedureShell>
        </div>
      ) : null}

      {tab === "history" ? (
        <TxHistory
          rows={history}
          loading={historyLoading}
          filter={historyFilter}
          onFilterChange={setHistoryFilter}
          discreet={discreet}
          portfolio={portfolio}
          fiat={fiat}
          fiatPrices={fiatPrices}
          busy={busy}
          onBumpFee={(txid) => void bumpFee(txid)}
        />
      ) : null}
    </div>
  );
}
