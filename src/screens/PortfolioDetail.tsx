import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import QRCodeStyling from "qr-code-styling";
import {
  api,
  canTrezorSend,
  EVM_CHAINS,
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
import { invalidateOverviewLedger } from "../lib/chartCache";
import { BalanceChart } from "../components/BalanceChart";
import { AnimatedMoney } from "../components/AnimatedMoney";
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
import { demoHistoryForPortfolioId, isDemoMode } from "../lib/demoMode";
import { playSendSound } from "../lib/sounds";
import {
  chainLabel,
  formatAmount,
  formatCompactAmount,
  formatMoney,
  formatQty,
  shortHash,
} from "../lib/format";
import { TxIconSent } from "../components/TxIcons";
import { useVault } from "../state/vault";
import { useNotify } from "../state/notifications";

type Tab = "balances" | "receive" | "send" | "history";
type SendStep = "to" | "amount" | "review" | "done";
const SEND_STEPS: SendStep[] = ["to", "amount", "review"];

type SendReceipt = {
  to: string;
  amountDisplay: string;
  symbol: string;
  txid: string;
  explorerUrl: string;
};

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
  /** Full vault list - used so DEV demo history matches showcase wallets. */
  portfolios?: PortfolioRecord[];
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
  portfolios = [],
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
  const [sendReceipt, setSendReceipt] = useState<SendReceipt | null>(null);

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
    setSendReceipt(null);
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
    if (isDemoMode()) {
      if (!isCancelled?.()) setBalanceLoading(false);
      return;
    }
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
      // Best-effort - keep whatever cached/prop balance we already have.
    } finally {
      if (!isCancelled?.()) setBalanceLoading(false);
      void started;
    }
  }

  // One scrape on open. Ongoing updates come from Shell's poll via balanceProp
  // - a second 1.5s loop here was doubling RPC load on the same portfolio.
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

  // Draws (or redraws) the styled QR into the container - a plain effect
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

  // Fetch history as soon as the portfolio opens - the growth chart needs
  // real txs to reconstruct balances over time (not just the History tab).
  useEffect(() => {
    let cancelled = false;

    if (isDemoMode()) {
      const list = portfolios.length ? portfolios : [portfolio];
      const rows = demoHistoryForPortfolioId(portfolio.id, list);
      setHistory(rows);
      setHistoryLoading(false);
      return;
    }

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
          // a week of holdings" - better a flat-from-zero than a lie.
          if (!cached) setHistory([]);
        }
      } finally {
        if (!cancelled) setHistoryLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
          // allowlisted tokens (DAI, USDC, USDT, …) - otherwise a token
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
    if (isDemoMode()) {
      const list = portfolios.length ? portfolios : [portfolio];
      setHistory(demoHistoryForPortfolioId(portfolio.id, list));
      setHistoryLoading(false);
      return;
    }
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

  // The asset actually being sent - native chain symbol, or a picked token.
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
    // rate from above - fall back to the live spot feed, keyed by CoinGecko
    // id (what `api.pricesFiat()` actually returns), not by raw chain/ticker
    // strings which never matched those keys.
    const coinId = coinIdForSymbol(selectedSymbol) ?? coinIdForChain(portfolio.chain);
    return coinId ? (prices[coinId] ?? null) : null;
  }, [prices, portfolio.chain, selectedSymbol, selectedBalance]);

  const maxNative = selectedBalance?.amount ?? "0";
  const maxFiat = selectedBalance?.usd ?? 0;

  /** Estimated fee to leave behind when sweeping native (not tokens). */
  const nativeFeeReserve = useMemo(() => {
    if (token) return 0;
    if (utxo && feeEstimate?.feeSats != null) {
      return feeEstimate.feeSats / 1e8;
    }
    const c = portfolio.chain.toLowerCase();
    if (c === "sol") {
      // Base signature fee is 5000 lamports; backend re-sweeps with getFeeForMessage.
      return feePreset === "priority" ? 0.00005 : 0.000005;
    }
    if (EVM_CHAINS.has(c)) {
      // Conservative UI reserve; backend recomputes exact gas on send_max.
      if (c === "arb" || c === "base" || c === "op") return 0.00005;
      if (c === "bsc" || c === "polygon" || c === "avax") return 0.001;
      return 0.0015;
    }
    if (c === "trx") return 1;
    return 0;
  }, [token, utxo, feeEstimate, portfolio.chain, feePreset]);

  /** Spendable native units after reserving an estimated network fee. */
  const spendableNative = useMemo(() => {
    const bal = Number(maxNative);
    if (!Number.isFinite(bal) || bal <= 0) return 0;
    return Math.max(0, bal - nativeFeeReserve);
  }, [maxNative, nativeFeeReserve]);

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
  // Max sweeps are fee-adjusted (and backend re-sweeps); don't block on float dust.
  const exceedsBalance =
    !sendMax &&
    Number.isFinite(amountNum) &&
    amountNum > 0 &&
    amountNum > spendableNative + 1e-12;
  const addressValid = !!to.trim() && !!safety?.ok && !addressChecking;
  const amountValid =
    !!nativeAmount && Number.isFinite(amountNum) && amountNum > 0 && !exceedsBalance;

  const fiatHelper = useMemo(() => {
    if (!amount || !rate) return null;
    const n = Number(amount);
    if (!Number.isFinite(n)) return null;
    if (amountUnit === "fiat") {
      return `≈ ${formatCompactAmount(n / rate, 4)} ${selectedSymbol}`;
    }
    return `≈ ${(n * rate).toFixed(2)} ${fiat}`;
  }, [amount, amountUnit, rate, fiat, selectedSymbol]);

  function formatReceiptAmount(): string {
    if (amountUnit === "fiat") {
      return `${formatCompactAmount(amount, 2)} ${fiat} ≈ ${formatCompactAmount(nativeAmount, 4)} ${selectedSymbol}`;
    }
    return `${formatCompactAmount(amount, 6)} ${selectedSymbol}`;
  }

  function formatMaxAmount(native: number): string {
    if (!(native > 0)) return "";
    if (amountUnit === "fiat") {
      const fiatMax = rate ? native * rate : maxFiat;
      return fiatMax > 0 ? fiatMax.toFixed(2) : "";
    }
    const raw = native < 1 ? native.toFixed(9) : native.toFixed(8);
    return raw.replace(/0+$/, "").replace(/\.$/, "");
  }

  function fillMax() {
    setSendMax(!token);
    const next = formatMaxAmount(spendableNative);
    if (next) setAmount(next);
  }

  // Keep Max in sync when the fee estimate / preset changes.
  useEffect(() => {
    if (!sendMax || token || tab !== "send") return;
    const next = formatMaxAmount(spendableNative);
    if (next) setAmount(next);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sendMax, spendableNative, amountUnit, rate, token, tab]);

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
    const receiptTo = to.trim();
    const receiptAmount = formatReceiptAmount();
    const receiptSymbol = selectedSymbol;
    try {
      const sendAmt = Number(nativeAmount);
      const res = await api.portfolioSend({
        portfolioId: portfolio.id,
        to: receiptTo,
        amount: nativeAmount,
        token: token || null,
        feePreset: utxo || portfolio.chain.toLowerCase() === "sol" ? feePreset : null,
        sendMax: sendMax && !token ? true : null,
      });
      // Instant client-side debit - don't wait for RPC scrape to show the spend.
      if (balance && Number.isFinite(sendAmt) && sendAmt > 0) {
        const feeNative = !token ? nativeFeeReserve : 0;
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
      setSendReceipt({
        to: receiptTo,
        amountDisplay: receiptAmount,
        symbol: receiptSymbol,
        txid: res.txid,
        explorerUrl: res.explorer_url,
      });
      playSendSound();
      setSendDir("forward");
      setSendStep("done");
      setTo("");
      setAmount("");
      setSendMax(false);
      invalidatePortfolioHistory(portfolio.id);
      invalidateOverviewLedger();
      void loadHistory();
      // Refresh portfolio list/history, but skip an immediate live scrape -
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

  const portfolioTotalNum = useMemo(
    () => portfolioFiatSum(balance, fiat, fiatPrices),
    [balance, fiat, fiatPrices],
  );

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
  // `null` while history is fetching. Incomplete ledger (held coin with no
  // txs yet) → fall back to mark-to-market via `undefined` so the tip always
  // matches the live balance instead of a 0→full spike or a stuck low tip.
  const growthLedger: ReturnType<typeof txsToLedger> | null | undefined = useMemo(() => {
    if (historyLoading) return null;
    const heldMissing = chartHoldings.some(
      (h) => h.amount > 0 && !chartLedger.some((e) => e.coinId === h.coinId),
    );
    if (heldMissing) return undefined;
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
    setSendReceipt(null);
  }

  function resetSendForm() {
    setSendReceipt(null);
    setSendDir("forward");
    setSendStep("to");
    setTo("");
    setAmount("");
    setSendMax(false);
  }

  function sendGoBack() {
    if (sendStep === "done") return;
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
    if (sendStep === "review") {
      void submitSend();
    }
  }

  return (
    <div className="content content-wide asset-page">
      <div className="asset-top">
        <span
          className="crypto-badge anim-icon"
          style={{ ["--chain-tint" as string]: chainTint(portfolio.chain) }}
        >
          <ChainIcon chain={portfolio.chain} size={48} />
        </span>
        <div className="asset-top__copy">
          <h2 className="portfolio-title">{portfolio.name}</h2>
          <p className="meta-line">
            {chainLabel(portfolio.chain)} · {t(kindI18nKey(portfolio.kind))}
          </p>
        </div>
      </div>

      <div className="asset-price">
        <AnimatedMoney
          as="p"
          className="asset-price__fiat"
          value={portfolioTotalNum}
          fiat={fiat}
          discreet={discreet}
          snapKey={`${portfolio.id}:${balance ? "ready" : "empty"}`}
        />
        <p className="asset-price__qty">{primaryQty}</p>
        {!canSpend && portfolio.kind !== "watch_only" ? (
          <p className="spend-note no-spend">{t(spendKey)}</p>
        ) : null}
      </div>

      <div className="asset-page-chart">
        <BalanceChart
          holdings={chartMode === "price" ? priceHoldings : chartHoldings}
          ledger={chartMode === "growth" ? growthLedger : undefined}
          liveTotal={
            chartMode === "growth"
              ? portfolioFiatSum(balance, fiat, fiatPrices)
              : undefined
          }
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
                    <AnimatedMoney
                      className="fiat"
                      value={assetFiatValue(a, fiat, fiatPrices)}
                      fiat={fiat}
                      discreet={discreet}
                      snapKey={`${portfolio.id}:${a.symbol}:${balance ? "ready" : "empty"}`}
                    />
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
            <div className="receive-tab__panel">
              <p className="receive-tab__label">{t("portfolio.receiveAddress", { defaultValue: "Your address" })}</p>
              <AddressEmphasis address={address} />
              <div className="receive-tab__actions">
                <button
                  type="button"
                  className="btn btn-primary receive-tab__copy"
                  onClick={() => void copyAddress()}
                >
                  <span
                    key={copied ? "copied" : "copy"}
                    className={copied ? "anim-confirm" : undefined}
                  >
                    {copied ? t("portfolio.copied") : t("portfolio.copy")}
                  </span>
                </button>
                {utxo && portfolio.kind === "software" ? (
                  <button
                    type="button"
                    className="btn receive-tab__next"
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
          {sendStep === "done" && sendReceipt ? (
            <div className="send-done wizard-pane wizard-pane--fwd">
              <div className="send-done__hero">
                <TxIconSent size={48} className="send-done__emblem anim-icon" />
                <h3 className="send-done__title">
                  {t("portfolio.sendSentTitle", { defaultValue: "Transaction sent" })}
                </h3>
                <p className="send-done__amount">
                  <span className="send-done__asset" aria-hidden>
                    <AssetIcon symbol={sendReceipt.symbol} size={36} />
                  </span>
                  <span className="send-done__amount-text">{sendReceipt.amountDisplay}</span>
                </p>
              </div>

              <dl className="send-done__meta">
                <div className="send-done__meta-row">
                  <dt>{t("portfolio.to")}</dt>
                  <dd className="mono">{shortHash(sendReceipt.to, 6, 6)}</dd>
                </div>
                <div className="send-done__meta-row">
                  <dt>{t("portfolio.txid")}</dt>
                  <dd>
                    {sendReceipt.explorerUrl ? (
                      <a
                        className="send-done__txid-link mono"
                        href={sendReceipt.explorerUrl}
                        target="_blank"
                        rel="noreferrer"
                      >
                        {shortHash(sendReceipt.txid, 8, 6)}
                      </a>
                    ) : (
                      <span className="mono">{shortHash(sendReceipt.txid, 8, 6)}</span>
                    )}
                  </dd>
                </div>
              </dl>

              <div className="send-done__actions">
                <button
                  type="button"
                  className="btn btn-primary"
                  onClick={() => goTab("history")}
                >
                  {t("portfolio.viewTransactions", {
                    defaultValue: "View transactions",
                  })}
                </button>
                <button
                  type="button"
                  className="btn btn-ghost"
                  onClick={resetSendForm}
                >
                  {t("portfolio.sendAgain", { defaultValue: "Send again" })}
                </button>
              </div>
            </div>
          ) : (
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
                    <div
                      className={`to-field${
                        safety || (to.trim() && addressChecking) ? " has-foot" : ""
                      }${
                        to.trim() && safety && !safety.ok ? " is-invalid" : ""
                      }`}
                    >
                      <input
                        value={to}
                        onChange={(e) => setTo(e.target.value)}
                        disabled={!canSpend}
                        placeholder={exampleAddressForChain(portfolio.chain)}
                        spellCheck={false}
                        autoComplete="off"
                        autoFocus
                        aria-invalid={to.trim() && safety ? !safety.ok : undefined}
                      />
                      {to.trim() && addressChecking && !safety ? (
                        <p className="to-field__foot">{t("portfolio.sendAddressChecking")}</p>
                      ) : null}
                      {safety ? (
                        <p
                          className={`to-field__foot${
                            !safety.ok
                              ? " is-error"
                              : safety.warnings.length > 0
                                ? " is-warn"
                                : " is-ok"
                          }`}
                        >
                          {safety.ok && safety.warnings.length === 0 ? (
                            <>
                              <span className="to-field__icon" aria-hidden>
                                <svg width="14" height="14" viewBox="0 0 24 24" fill="none">
                                  <path
                                    d="M5 12.5l4.5 4.5L19 7.5"
                                    stroke="currentColor"
                                    strokeWidth="2.6"
                                    strokeLinecap="round"
                                    strokeLinejoin="round"
                                  />
                                </svg>
                              </span>
                              {t("portfolio.sendAddressOk")}
                            </>
                          ) : (
                            safety.warnings[0] ?? t("portfolio.sendAddressCheckFailed")
                          )}
                        </p>
                      ) : null}
                    </div>
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
                  <h3 className="send-confirm__title">
                    {t("portfolio.reviewTitle", { defaultValue: "Are you sure?" })}
                  </h3>
                  <p className="section-desc">
                    {t("portfolio.reviewHint", {
                      defaultValue: "This can't be undone.",
                    })}
                  </p>
                  <div className="review-grid">
                    <span>{t("portfolio.to")}</span>
                    <AddressEmphasis address={to.trim()} />
                    <span>{t("portfolio.asset", { defaultValue: "Asset" })}</span>
                    <strong>{selectedSymbol}</strong>
                    <span>{t("portfolio.amount")}</span>
                    <strong>
                      {amountUnit === "fiat"
                        ? `${formatCompactAmount(amount, 2)} ${fiat} ≈ ${formatCompactAmount(nativeAmount, 4)} ${selectedSymbol}`
                        : `${formatCompactAmount(amount, 6)} ${selectedSymbol}`}
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
                    ? busy
                      ? portfolio.kind === "trezor"
                        ? t("portfolio.trezorConfirmHint", {
                            defaultValue: "Confirm on your Trezor…",
                          })
                        : t("portfolio.sending", { defaultValue: "Sending…" })
                      : t("portfolio.confirmSend", { defaultValue: "Yes, send" })
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
          )}
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
          activityMinFiat={status?.activity_min_fiat ?? 0.02}
          busy={busy}
          onBumpFee={(txid) => void bumpFee(txid)}
        />
      ) : null}
    </div>
  );
}
