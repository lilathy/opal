import { listen } from "@tauri-apps/api/event";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { AnimatedMoney } from "../components/AnimatedMoney";
import { AnalyticsPanel } from "../components/AnalyticsPreview";
import { AssetIcon, ChainIcon } from "../components/CryptoIcons";
import {
  IconChevronDown,
  IconCopy,
  IconHome,
  IconPlus,
  IconSettings,
  IconSwap,
} from "../components/UiIcons";
import {
  api,
  invalidatePortfolioHistory,
  parseInvokeError,
  type AssetBalance,
  type PortfolioBalance,
  type PortfolioRecord,
  type TrezorStatus,
  type TxRow,
} from "../lib/api";
import { coinIdForSymbol, txsToLedger, type LedgerEvent } from "../lib/charts";
import {
  assetFiatValue,
  assetsOf,
  cacheFiatPriceMatrix,
  applyLiveBalances,
  mergeBalances,
  portfolioFiatSum,
  reconcilePendingSpend,
} from "../lib/balances";
import { scrapePortfolioBalance } from "../lib/balanceScrape";
import {
  fetchPriceHistoryCached,
  invalidateOverviewLedger,
  loadOverviewLedger,
  mergeLedgerEvents,
  saveOverviewLedger,
} from "../lib/chartCache";
import { formatAmount, formatQty, formatTxTime, shortHash, txTimestampDate } from "../lib/format";
import { filterDustTxs } from "../lib/txFilter";
import { BalanceChart } from "../components/BalanceChart";
import { TxIconReceived, TxIconSelf, TxIconSent } from "../components/TxIcons";
import { useFiatPrices } from "../hooks/useFiatPrices";
import { useIncomingWatcher } from "../hooks/useIncomingWatcher";
import { useSortableList } from "../hooks/useSortableList";
import { SyncTrezorPanel } from "../components/SyncTrezorPanel";
import { TrezorSpinner } from "../components/TrezorSpinner";
import { useVault } from "../state/vault";
import { useNotify } from "../state/notifications";
import { playTrezorConnectedSound } from "../lib/sounds";
import { AddPortfolio } from "./AddPortfolio";
import { PortfolioDetail } from "./PortfolioDetail";
import { clearSeedSetupIntent, type SeedSetupIntent } from "../lib/seedIntent";
import { SeedSetup } from "./SeedSetup";
import { SettingsScreen } from "./SettingsScreen";
import { SwapScreen } from "./SwapScreen";

type View = "home" | "settings" | "seed" | "add" | "portfolio" | "swap" | "trezor-sync";

const ORDER_KEY = "opal.portfolioOrder";

type ContextMenuState = { id: string; x: number; y: number };

type RecentActivityItem = {
  key: string;
  portfolioId: string;
  portfolioName: string;
  chain: string;
  tx: TxRow;
};

function kindI18nKey(kind: string): string {
  if (kind === "software") return "portfolio.kindSoftware";
  if (kind === "trezor") return "portfolio.kindTrezor";
  if (kind === "watch_only") return "portfolio.kindWatch";
  return kind;
}

function balancesFromCache(plist: PortfolioRecord[]): PortfolioBalance[] {
  const out: PortfolioBalance[] = [];
  for (const p of plist) {
    const raw = p.cached_balances_json;
    if (!raw) continue;
    try {
      out.push(JSON.parse(raw) as PortfolioBalance);
    } catch {
      /* ignore bad cache */
    }
  }
  return out;
}

function portfolioAssets(bal: PortfolioBalance | undefined): AssetBalance[] {
  return assetsOf(bal);
}

/** Native gas/ticker for a chain - excluded from nested token lists. */
function nativeSymbolForChain(chain: string): string {
  switch (chain.toLowerCase()) {
    case "btc":
      return "BTC";
    case "eth":
    case "arb":
    case "base":
    case "linea":
      return "ETH";
    case "sol":
      return "SOL";
    case "xmr":
      return "XMR";
    case "ltc":
      return "LTC";
    case "doge":
      return "DOGE";
    case "trx":
      return "TRX";
    case "ton":
      return "TON";
    case "polygon":
      return "POL";
    case "avax":
      return "AVAX";
    case "bsc":
      return "BNB";
    case "gnosis":
      return "XDAI";
    default:
      return chain.toUpperCase();
  }
}

/** Secondary tokens only (hide native SOL/ETH/BTC/…). */
function tokensUnderPortfolio(assets: AssetBalance[], chain: string): AssetBalance[] {
  const native = nativeSymbolForChain(chain).toUpperCase();
  return assets.filter((a) => {
    const sym = a.symbol.toUpperCase();
    // Gnosis native may appear as xDAI
    if (sym === native) return false;
    if (native === "XDAI" && (sym === "XDAI" || sym === "WXDAI")) return false;
    return true;
  });
}

export function ShellScreen() {
  const { t } = useTranslation();
  const { lockVault, status, refresh } = useVault();
  const { notify } = useNotify();
  const [view, setView] = useState<View>("home");
  const [seedInitialMode, setSeedInitialMode] = useState<"choose" | SeedSetupIntent>("choose");
  const [seedLockPath, setSeedLockPath] = useState(false);
  const [portfolios, setPortfolios] = useState<PortfolioRecord[]>([]);
  const [balances, setBalances] = useState<PortfolioBalance[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detailTab, setDetailTab] = useState<"balances" | "receive" | "send" | "history">(
    "balances",
  );
  const [expandedSidebar, setExpandedSidebar] = useState<Record<string, boolean>>({});
  const lastBalancesFetchAt = useRef(0);
  const portfoliosRef = useRef(portfolios);
  portfoliosRef.current = portfolios;
  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;
  const [trezorStatus, setTrezorStatus] = useState<TrezorStatus | null>(null);

  // ── Right-click context menu (rename / rescan / delete) ─────────
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [rowBusy, setRowBusy] = useState<string | null>(null);
  const renameInputRef = useRef<HTMLInputElement>(null);
  const renameDraftRef = useRef("");
  const renameCommitInFlight = useRef<string | null>(null);

  // ── Sidebar drag reorder ─────────────────────────────────────────
  // Order is seeded from the vault list (source of truth). localStorage is
  // only a one-shot migration for installs that reordered before vault
  // persistence existed.
  const [order, setOrder] = useState<string[]>(() => {
    try {
      const raw = window.localStorage.getItem(ORDER_KEY);
      return raw ? (JSON.parse(raw) as string[]) : [];
    } catch {
      return [];
    }
  });
  const orderPersistTimer = useRef<number | null>(null);
  const [editMode, setEditMode] = useState(false);
  const [txLedger, setTxLedger] = useState<LedgerEvent[] | null>(null);
  const [recentActivity, setRecentActivity] = useState<RecentActivityItem[]>([]);
  const [recentLoading, setRecentLoading] = useState(false);
  const [expandedRecent, setExpandedRecent] = useState<string | null>(null);
  const [copiedRecent, setCopiedRecent] = useState<string | null>(null);
  const sidebarListRef = useRef<HTMLDivElement>(null);

  function applyOrder(next: string[]) {
    setOrder(next);
  }

  function persistOrderToVault(next: string[]) {
    setOrder(next);
    try {
      window.localStorage.setItem(ORDER_KEY, JSON.stringify(next));
    } catch {
      /* ignore quota */
    }
    if (orderPersistTimer.current != null) {
      window.clearTimeout(orderPersistTimer.current);
    }
    orderPersistTimer.current = window.setTimeout(() => {
      orderPersistTimer.current = null;
      void api
        .portfolioReorder(next)
        .then((list) => setPortfolios(list))
        .catch(() => {});
    }, 280);
  }

  const sidebarSort = useSortableList({
    order,
    onOrderChange: applyOrder,
    onDragEnd: () => {
      setOrder((current) => {
        persistOrderToVault(current);
        return current;
      });
    },
    containerRef: sidebarListRef,
    enabled: editMode,
  });

  function toggleSidebarExpanded(id: string) {
    setExpandedSidebar((prev) => ({ ...prev, [id]: !prev[id] }));
  }

  // Incomplete seed setup after an interrupted session - resume backup only.
  const seedReady = Boolean(status?.has_seed && status?.seed_backed_up);
  useEffect(() => {
    if (seedReady) {
      clearSeedSetupIntent();
      if (view === "seed") setView("home");
      return;
    }
    if (status == null) return;
    if (status.has_seed && !status.seed_backed_up) {
      setSeedInitialMode("create");
      setSeedLockPath(true);
      if (view !== "seed") setView("seed");
    }
  }, [seedReady, status, view]);

  // Keep the in-memory drag order in sync with whatever portfolios actually
  // exist - new ones append at the end, removed ones drop out. Prefer vault
  // order when we have no local preference yet. If localStorage still holds
  // a pre-vault reorder, push it into the vault once so it survives reinstall.
  const migratedOrderToVault = useRef(false);
  useEffect(() => {
    setOrder((prev) => {
      const ids = portfolios.map((p) => p.id);
      if (!ids.length) return prev;
      const known = new Set(ids);
      if (!prev.length) return ids;
      const kept = prev.filter((id) => known.has(id));
      const missing = ids.filter((id) => !kept.includes(id));
      const next = [...kept, ...missing];
      const same = next.length === prev.length && next.every((id, i) => id === prev[i]);
      if (
        !migratedOrderToVault.current &&
        next.length &&
        next.some((id, i) => id !== ids[i])
      ) {
        migratedOrderToVault.current = true;
        void api.portfolioReorder(next).then((list) => setPortfolios(list)).catch(() => {});
      }
      return same ? prev : next;
    });
  }, [portfolios]);

  const orderedPortfolios = useMemo(() => {
    const byId = new Map(portfolios.map((p) => [p.id, p] as const));
    const ordered = order.map((id) => byId.get(id)).filter((p): p is PortfolioRecord => !!p);
    for (const p of portfolios) {
      if (!order.includes(p.id)) ordered.push(p);
    }
    return ordered;
  }, [portfolios, order]);

  function enterEditMode() {
    setExpandedSidebar({});
    setEditMode(true);
  }

  function exitEditMode() {
    sidebarSort.cancelDrag();
    setEditMode(false);
  }

  function openContextMenu(e: React.MouseEvent, id: string) {
    e.preventDefault();
    if (editMode) return;
    const x = Math.min(e.clientX, window.innerWidth - 200);
    const y = Math.min(e.clientY, window.innerHeight - 180);
    setContextMenu({ id, x, y });
  }

  useEffect(() => {
    if (!contextMenu) return;
    function onDoc() {
      setContextMenu(null);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setContextMenu(null);
    }
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [contextMenu]);

  useEffect(() => {
    if (!editMode) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") exitEditMode();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [editMode]);

  function startRename(id: string, name: string) {
    setContextMenu(null);
    renameCommitInFlight.current = null;
    renameDraftRef.current = name;
    setRenameDraft(name);
    setRenamingId(id);
    window.setTimeout(() => renameInputRef.current?.select(), 0);
  }

  async function commitRename(id: string) {
    // Guard against Enter + blur both firing for the same edit.
    if (renameCommitInFlight.current === id) return;
    renameCommitInFlight.current = id;
    // Always read from the ref - blur can fire after a re-render where a
    // stale closure still sees the pre-keystroke draft.
    const next = renameDraftRef.current.trim();
    const current = portfolios.find((p) => p.id === id);
    if (!next || !current || next === current.name) {
      setRenamingId(null);
      renameCommitInFlight.current = null;
      return;
    }
    setRowBusy(id);
    try {
      await api.portfolioRename(id, next);
      // Optimistically update local list so the new name sticks even if
      // reloadList is slow or fails once.
      setPortfolios((prev) =>
        prev.map((p) => (p.id === id ? { ...p, name: next } : p)),
      );
      setRenamingId(null);
      try {
        await reloadList();
      } catch {
        /* local name already updated */
      }
    } catch (e) {
      showError(parseInvokeError(e).message);
      renameCommitInFlight.current = null;
    } finally {
      setRowBusy(null);
      // Keep inFlight set briefly so the blur that follows Enter is ignored.
      window.setTimeout(() => {
        if (renameCommitInFlight.current === id) renameCommitInFlight.current = null;
      }, 120);
    }
  }

  async function rescanPortfolio(id: string) {
    setContextMenu(null);
    setRowBusy(id);
    try {
      await api.portfolioRescan(id);
      await reload();
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setRowBusy(null);
    }
  }

  async function deletePortfolioById(id: string) {
    setContextMenu(null);
    setRowBusy(id);
    try {
      await api.portfolioDelete(id);
      if (selectedId === id) {
        setSelectedId(null);
        setView("home");
      }
      await reloadList();
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setRowBusy(null);
    }
  }

  function showError(message: string) {
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message,
    });
  }

  async function reloadList() {
    const plist = await api.portfolioList();
    setPortfolios(plist);
    const cached = balancesFromCache(plist);
    if (cached.length) {
      setBalances((prev) => mergeBalances(prev, cached));
    }
    await refresh();
    return plist;
  }

  function upsertBalance(bal: PortfolioBalance) {
    setBalances((prev) => applyLiveBalances(prev, [bal]));
  }

  /** Scrape portfolios in parallel; apply once so the UI doesn't thrash.
   *  Prioritize the selected wallet so Overview/Detail stay snappy. */
  async function reloadBalances() {
    const list = portfoliosRef.current;
    if (!list.length) return;

    const selected = selectedIdRef.current;
    const ordered = selected
      ? [
          ...list.filter((p) => p.id === selected),
          ...list.filter((p) => p.id !== selected),
        ]
      : list;

    // Paint the open portfolio first when possible, then merge the rest.
    if (selected) {
      try {
        const first = await scrapePortfolioBalance(selected);
        if (first) {
          setBalances((prev) => applyLiveBalances(prev, [first]));
        }
      } catch {
        /* continue with full pass */
      }
    }

    const rest = ordered.filter((p) => p.id !== selected);
    const settled = await Promise.allSettled(
      rest.map((p) => scrapePortfolioBalance(p.id)),
    );
    const next: PortfolioBalance[] = [];
    for (const r of settled) {
      if (r.status === "fulfilled" && r.value) next.push(r.value);
    }
    if (next.length) {
      setBalances((prev) => applyLiveBalances(prev, next));
    }
    lastBalancesFetchAt.current = Date.now();
  }

  async function reload() {
    try {
      const instant = await api.portfolioBalancesCached();
      if (instant.length) {
        setBalances((prev) => mergeBalances(prev, instant));
      }
      await reloadList();
      void reloadBalances();
    } catch (e) {
      showError(parseInvokeError(e).message);
    }
  }

  useEffect(() => {
    void reload();
  }, []);

  useEffect(() => {
    if (status?.phase !== "unlocked") return;
    // Rust keeps a hot exchange price book in the background. UI only syncs
    // the in-memory snapshot (instant) and occasionally nudges a refresh.
    const sync = () => {
      void api.spotPricesSnapshot().then((matrix) => {
        if (matrix) cacheFiatPriceMatrix(matrix);
      });
    };
    const warm = () => {
      void api.warmSpotPrices().then((matrix) => {
        if (matrix) cacheFiatPriceMatrix(matrix);
      });
    };
    warm();
    sync();
    const snapTick = window.setInterval(() => {
      if (document.visibilityState !== "visible") return;
      sync();
    }, 3_000);
    const warmTick = window.setInterval(() => {
      if (document.visibilityState !== "visible") return;
      warm();
    }, 20_000);
    return () => {
      window.clearInterval(snapTick);
      window.clearInterval(warmTick);
    };
  }, [status?.phase]);

  // Poll while visible. Focused refresh is snappy; idle poll is gentler so
  // public RPCs aren't hammered into timeouts (which used to paint fake zeros).
  useEffect(() => {
    const maybeRefresh = () => {
      if (document.visibilityState !== "visible") return;
      if (Date.now() - lastBalancesFetchAt.current < 4_000) return;
      lastBalancesFetchAt.current = Date.now();
      void reloadBalances();
    };
    maybeRefresh();
    const tick = window.setInterval(maybeRefresh, 6_000);
    window.addEventListener("focus", maybeRefresh);
    document.addEventListener("visibilitychange", maybeRefresh);
    return () => {
      window.clearInterval(tick);
      window.removeEventListener("focus", maybeRefresh);
      document.removeEventListener("visibilitychange", maybeRefresh);
    };
  }, []);

  async function reloadTrezorStatus() {
    try {
      setTrezorStatus(await api.trezorStatus());
    } catch {
      // Best-effort - the widget just shows "not detected" on failure.
    }
  }

  useEffect(() => {
    void reloadTrezorStatus();
    const tick = window.setInterval(() => {
      if (document.visibilityState !== "visible") return;
      void reloadTrezorStatus();
    }, 15_000);
    return () => {
      window.clearInterval(tick);
    };
  }, []);

  // Play once when a Trezor appears after we already had a baseline (reconnect).
  const trezorWasConnected = useRef<boolean | null>(null);
  useEffect(() => {
    const connected =
      Boolean(trezorStatus?.available) && (trezorStatus?.device_count ?? 0) > 0;
    if (trezorWasConnected.current === null) {
      trezorWasConnected.current = connected;
      return;
    }
    if (connected && !trezorWasConnected.current) {
      playTrezorConnectedSound();
    }
    trezorWasConnected.current = connected;
  }, [trezorStatus?.available, trezorStatus?.device_count]);

  // When a Trezor becomes connected and ready, automatically rescan for funded
  // accounts (quiet). Re-runs each time the device reconnects after being offline.
  const trezorLastSyncedKey = useRef<string | null>(null);
  const trezorSyncInFlight = useRef(false);
  useEffect(() => {
    const ready =
      Boolean(trezorStatus?.available) &&
      (trezorStatus?.device_count ?? 0) > 0 &&
      !trezorStatus?.session_active;

    if (!ready) {
      // Allow a fresh sync the next time the device comes back.
      if ((trezorStatus?.device_count ?? 0) === 0) {
        trezorLastSyncedKey.current = null;
      }
      return;
    }

    const deviceKey =
      trezorStatus?.device_label ||
      trezorStatus?.device_internal_model ||
      trezorStatus?.device_model ||
      "trezor";

    if (trezorLastSyncedKey.current === deviceKey) return;
    if (trezorSyncInFlight.current) return;

    // No Trezor wallets yet → Sync UI handles discovery (avoids USB race).
    const hasTrezor = portfolios.some((p) => p.kind === "trezor");
    if (!hasTrezor) return;

    trezorLastSyncedKey.current = deviceKey;
    trezorSyncInFlight.current = true;
    let cancelled = false;

    void (async () => {
      try {
        const created = await api.trezorDiscoverPortfolios(true);
        if (cancelled) return;
        if (created.length) {
          await reload();
          notify({
            kind: "success",
            title: t("trezor.syncTitle", { defaultValue: "Sync my Trezor" }),
            message: t("trezor.syncFound", {
              count: created.length,
              defaultValue: "Added {{count}} wallet(s)",
            }),
          });
        }
        // Background KI sync for watch-only XMR - does not block scrapes when offline.
        const hasXmrTrezor =
          created.some((p) => p.chain === "xmr") ||
          portfolios.some((p) => p.kind === "trezor" && p.chain === "xmr");
        if (hasXmrTrezor && !cancelled) {
          void api.trezorSyncXmrKeyImages().catch(() => {
            /* device busy / user cancelled confirm - next reconnect retries */
          });
        }
      } catch {
        // Device may still be unlocking - clear so the next status poll retries.
        if (!cancelled) trezorLastSyncedKey.current = null;
      } finally {
        trezorSyncInFlight.current = false;
      }
    })();

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    trezorStatus?.available,
    trezorStatus?.device_count,
    trezorStatus?.session_active,
    trezorStatus?.device_label,
    trezorStatus?.device_model,
    trezorStatus?.device_internal_model,
  ]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen("opal://lock-request", () => {
      void lockVault();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, [lockVault]);

  useEffect(() => {
    let timer: number | undefined;
    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        timer = window.setTimeout(() => {
          void lockVault();
        }, 120_000);
      } else if (timer) {
        window.clearTimeout(timer);
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      if (timer) window.clearTimeout(timer);
    };
  }, [lockVault]);

  const selected = portfolios.find((p) => p.id === selectedId) ?? null;
  const selectedBalRaw = balances.find((b) => b.portfolio_id === selectedId);
  const selectedBal = selectedBalRaw
    ? reconcilePendingSpend(selectedBalRaw)
    : undefined;
  const fiat = status?.fiat ?? "USD";
  const discreet = !!status?.discreet_mode;
  const activityMinFiat = status?.activity_min_fiat ?? 0.02;
  const fiatPrices = useFiatPrices(fiat);

  useIncomingWatcher(
    balances,
    portfolios,
    !!status?.notifications_enabled,
    discreet,
    fiat,
    fiatPrices,
  );

  // Instant fiat switch: pull cached spot maps from backend (no network wait).
  useLayoutEffect(() => {
    void api.spotPricesSnapshot().then((matrix) => {
      if (matrix && Object.keys(matrix).length > 0) cacheFiatPriceMatrix(matrix);
    });
  }, [fiat]);

  const liveTotalNum = useMemo(() => {
    let sum = 0;
    for (const b of balances) {
      sum += portfolioFiatSum(b, fiat, fiatPrices);
    }
    return sum;
  }, [balances, fiat, fiatPrices]);

  const chartHoldings = useMemo(() => {
    const map = new Map<string, number>();
    for (const b of balances) {
      for (const a of assetsOf(b)) {
        const id = coinIdForSymbol(a.symbol);
        if (!id) continue;
        const amt = Number(a.amount);
        if (!Number.isFinite(amt) || amt <= 0) continue;
        map.set(id, (map.get(id) ?? 0) + amt);
      }
    }
    return [...map.entries()].map(([coinId, amount]) => ({ coinId, amount }));
  }, [balances]);

  const portfolioIdsKey = useMemo(
    () => portfolios.map((p) => p.id).sort().join("|"),
    [portfolios],
  );

  // Growth ledger for overview. Hydrate from sessionStorage instantly, then
  // merge portfolio histories as they arrive (don't wait for the slowest chain).
  useEffect(() => {
    if (!portfolios.length) {
      setTxLedger([]);
      setRecentActivity([]);
      setRecentLoading(false);
      return;
    }
    let cancelled = false;
    setRecentLoading(true);

    const cached = loadOverviewLedger(portfolioIdsKey);
    if (cached?.length) {
      setTxLedger(cached);
    } else {
      setTxLedger(null);
    }

    // Prefetch default 7D prices while history loads - chart paints the
    // moment the first ledger chunk arrives.
    const ids = [
      ...new Set(
        chartHoldings.filter((h) => h.amount > 0).map((h) => h.coinId),
      ),
    ];
    if (ids.length) {
      void fetchPriceHistoryCached(
        (coinIds, vs, days) => api.priceHistory(coinIds, vs, days),
        ids,
        fiat,
        7,
      );
    }

    let merged: LedgerEvent[] = cached ? [...cached] : [];
    const activityBag: RecentActivityItem[] = [];

    const ingest = (part: LedgerEvent[]) => {
      if (!part.length) return;
      merged = mergeLedgerEvents(merged, part);
      if (cancelled || !merged.length) return;
      setTxLedger([...merged]);
      saveOverviewLedger(portfolioIdsKey, merged);
    };

    const publishActivity = () => {
      if (cancelled) return;
      const sorted = [...activityBag].sort((a, b) => {
        const ta = txTimestampDate(a.tx.timestamp)?.getTime() ?? 0;
        const tb = txTimestampDate(b.tx.timestamp)?.getTime() ?? 0;
        return tb - ta;
      });
      setRecentActivity(sorted);
    };

    void (async () => {
      await Promise.all(
        portfolios.map(async (p) => {
          if (cancelled) return;
          try {
            const rows = await api.portfolioHistory(p.id);
            if (cancelled) return;
            ingest(txsToLedger(rows));
            for (const tx of rows) {
              activityBag.push({
                key: `${p.id}:${tx.txid}`,
                portfolioId: p.id,
                portfolioName: p.name,
                chain: p.chain,
                tx,
              });
            }
            publishActivity();
          } catch {
            /* best-effort per portfolio */
          }
        }),
      );
      if (cancelled) return;
      // Still nothing after a full pass - empty chart, not infinite spinner.
      if (!merged.length) setTxLedger([]);
      publishActivity();
      setRecentLoading(false);
    })();

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [portfolioIdsKey]);

  const recentVisible = useMemo(() => {
    const byPortfolio = new Map<string, TxRow[]>();
    for (const item of recentActivity) {
      const list = byPortfolio.get(item.portfolioId) ?? [];
      list.push(item.tx);
      byPortfolio.set(item.portfolioId, list);
    }
    const kept = new Set<string>();
    for (const [pid, rows] of byPortfolio) {
      for (const tx of filterDustTxs(rows, activityMinFiat, fiatPrices)) {
        kept.add(`${pid}:${tx.txid}`);
      }
    }
    return recentActivity.filter((item) => kept.has(item.key)).slice(0, 5);
  }, [recentActivity, activityMinFiat, fiatPrices]);

  // After leaving a portfolio, sync its balance back to the overview immediately.
  useEffect(() => {
    if (view === "portfolio" || !selectedId) return;
    const id = selectedId;
    void (async () => {
      try {
        const cached = await api.portfolioBalancesCached(id);
        const fromCache = cached.find((b) => b.portfolio_id === id);
        if (fromCache) upsertBalance(fromCache);
        const live = await scrapePortfolioBalance(id);
        if (live) upsertBalance(live);
      } catch {
        /* best-effort */
      }
    })();
  }, [view, selectedId]);

  function go(next: View) {
    setView(next);
  }

  function openPortfolio(
    id: string,
    tab: "balances" | "receive" | "send" | "history" = "balances",
  ) {
    setSelectedId(id);
    setDetailTab(tab);
    setView("portfolio");
  }

  return (
    <div className="shell anim-shell-enter">
      <aside className="sidebar anim-sidebar-enter">
        <button
          type="button"
          className="trezor-status"
          onClick={() => {
            void reloadTrezorStatus();
            go("trezor-sync");
          }}
          title={t("trezor.syncTitle", { defaultValue: "Sync my Trezor" })}
        >
          <span className="trezor-status__icon">
            <TrezorSpinner internalModel={trezorStatus?.device_internal_model} size={26} />
          </span>
          <span className="trezor-status__copy">
            <span className="trezor-status__name">
              {trezorStatus?.device_model
                ? t("shell.trezorNamed", {
                    model: trezorStatus.device_model,
                    defaultValue: "Trezor {{model}}",
                  })
                : t("shell.trezor", { defaultValue: "Trezor" })}
            </span>
            <span
              className={`trezor-status__state${
                trezorStatus?.device_count ? " is-connected" : ""
              }`}
            >
              <span className="trezor-status__dot" />
              {trezorStatus?.message ?? t("shell.trezorOffline", { defaultValue: "Offline" })}
            </span>
          </span>
        </button>

        <nav className="sidebar-nav" aria-label={t("shell.navLabel")}>
          <button
            type="button"
            className={`nav-item${view === "home" ? " is-active" : ""}`}
            onClick={() => {
              setSelectedId(null);
              go("home");
            }}
          >
            <IconHome />
            {t("shell.homeTitle")}
          </button>
          <button
            type="button"
            className={`nav-item${view === "swap" ? " is-active" : ""}`}
            onClick={() => go("swap")}
          >
            <IconSwap />
            {t("swap.title")}
          </button>
          <button
            type="button"
            className={`nav-item${view === "settings" ? " is-active" : ""}`}
            onClick={() => go("settings")}
          >
            <IconSettings />
            {t("common.settings")}
          </button>
        </nav>

        <div className="sidebar-section">
          <div className="sidebar-label-row">
            <div className="sidebar-label">{t("shell.portfolios")}</div>
            <div className="row" style={{ gap: 4 }}>
              {editMode ? (
                <button
                  type="button"
                  className="icon-btn"
                  aria-label={t("common.done", { defaultValue: "Done" })}
                  title={t("common.done", { defaultValue: "Done" })}
                  onClick={exitEditMode}
                >
                  <span style={{ fontSize: 11, fontWeight: 700 }}>{t("common.done", { defaultValue: "Done" })}</span>
                </button>
              ) : portfolios.length >= 2 ? (
                <button
                  type="button"
                  className="icon-btn"
                  aria-label={t("shell.reorder", { defaultValue: "Reorder portfolios" })}
                  title={t("shell.reorder", { defaultValue: "Reorder" })}
                  onClick={() => enterEditMode()}
                >
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden>
                    <circle cx="9" cy="6" r="1.6" />
                    <circle cx="15" cy="6" r="1.6" />
                    <circle cx="9" cy="12" r="1.6" />
                    <circle cx="15" cy="12" r="1.6" />
                    <circle cx="9" cy="18" r="1.6" />
                    <circle cx="15" cy="18" r="1.6" />
                  </svg>
                </button>
              ) : null}
              <button
                type="button"
                className="icon-btn"
                aria-label={t("shell.addPortfolio")}
                title={t("shell.addPortfolio")}
                onClick={() => go("add")}
              >
                <IconPlus size={16} />
              </button>
            </div>
          </div>
          {portfolios.length === 0 ? (
            <div className="sidebar-empty">{t("shell.emptyPortfolios")}</div>
          ) : (
            <div
              className={`portfolio-list${editMode ? " is-sorting" : " anim-stagger"}${sidebarSort.isDragging ? " is-sorting-active" : ""}`}
              ref={sidebarListRef}
            >
              {orderedPortfolios.map((p) => {
                const balRaw = balances.find((b) => b.portfolio_id === p.id);
                const bal = balRaw ? reconcilePendingSpend(balRaw) : undefined;
                const assets = portfolioAssets(bal);
                const tokens = tokensUnderPortfolio(assets, p.chain);
                const expandable = tokens.length > 0 && !editMode;
                const expanded = !!expandedSidebar[p.id];
                const active = selectedId === p.id && view === "portfolio";
                const dragging = sidebarSort.draggingId === p.id;
                return (
                  <div
                    key={p.id}
                    data-sort-id={p.id}
                    className={`sidebar-portfolio${expanded ? " is-expanded" : ""}${active ? " is-active" : ""}${dragging ? " is-sorting-item" : ""}`}
                    onPointerDown={
                      editMode ? (e) => sidebarSort.beginDrag(e, p.id) : undefined
                    }
                    onPointerMove={editMode ? sidebarSort.rowPointerMove : undefined}
                    onPointerUp={editMode ? sidebarSort.rowPointerEnd : undefined}
                    onPointerCancel={editMode ? sidebarSort.cancelDrag : undefined}
                    style={editMode ? { touchAction: "none" } : undefined}
                  >
                    <div className="sidebar-portfolio__head">
                      {editMode ? (
                        <span className="sidebar-portfolio__grip" aria-hidden>
                          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                            <circle cx="5" cy="4" r="1.25" />
                            <circle cx="11" cy="4" r="1.25" />
                            <circle cx="5" cy="8" r="1.25" />
                            <circle cx="11" cy="8" r="1.25" />
                            <circle cx="5" cy="12" r="1.25" />
                            <circle cx="11" cy="12" r="1.25" />
                          </svg>
                        </span>
                      ) : expandable ? (
                        <button
                          type="button"
                          className="sidebar-portfolio__toggle"
                          aria-expanded={expanded}
                          aria-label={
                            expanded
                              ? t("shell.collapseTokens", { defaultValue: "Hide tokens" })
                              : t("shell.expandTokens", { defaultValue: "Show tokens" })
                          }
                          onClick={() => toggleSidebarExpanded(p.id)}
                        >
                          <IconChevronDown size={18} />
                        </button>
                      ) : (
                        <span className="sidebar-portfolio__toggle-spacer" aria-hidden />
                      )}
                      {renamingId === p.id ? (
                        <div className="portfolio-item portfolio-item--renaming">
                          <span className="crypto-badge portfolio-item__icon">
                            <ChainIcon chain={p.chain} size={32} />
                          </span>
                          <input
                            ref={renameInputRef}
                            className="control-input portfolio-item__rename"
                            autoFocus
                            value={renameDraft}
                            disabled={rowBusy === p.id}
                            onChange={(e) => {
                              renameDraftRef.current = e.target.value;
                              setRenameDraft(e.target.value);
                            }}
                            onBlur={() => void commitRename(p.id)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") {
                                e.preventDefault();
                                (e.target as HTMLInputElement).blur();
                              }
                              if (e.key === "Escape") {
                                e.preventDefault();
                                renameCommitInFlight.current = p.id;
                                setRenamingId(null);
                                window.setTimeout(() => {
                                  if (renameCommitInFlight.current === p.id) {
                                    renameCommitInFlight.current = null;
                                  }
                                }, 120);
                              }
                            }}
                          />
                        </div>
                      ) : (
                        <button
                          type="button"
                          className={`portfolio-item${active ? " is-active" : ""}`}
                          onClick={() => {
                            if (editMode) return;
                            openPortfolio(p.id);
                          }}
                          onContextMenu={(e) => openContextMenu(e, p.id)}
                        >
                          <span className="crypto-badge portfolio-item__icon">
                            <ChainIcon chain={p.chain} size={32} />
                          </span>
                          <span className="portfolio-item-name">{p.name}</span>
                          <span className="portfolio-item-meta">
                            {t(kindI18nKey(p.kind))}
                          </span>
                          <AnimatedMoney
                            className="portfolio-item-bal"
                            value={portfolioFiatSum(bal, fiat, fiatPrices)}
                            fiat={fiat}
                            discreet={discreet}
                            snapKey={p.id}
                          />
                        </button>
                      )}
                    </div>
                    {expandable ? (
                      <div className="sidebar-portfolio__tokens" aria-hidden={!expanded}>
                        <div className="sidebar-portfolio__tokens-inner">
                          {tokens.map((a) => (
                            <button
                              key={a.symbol}
                              type="button"
                              className="sidebar-token"
                              tabIndex={expanded ? 0 : -1}
                              onClick={() => openPortfolio(p.id)}
                            >
                              <span className="crypto-badge">
                                <AssetIcon symbol={a.symbol} size={26} />
                              </span>
                              <span className="sidebar-token__sym">{a.symbol}</span>
                              <AnimatedMoney
                                className="sidebar-token__bal"
                                value={assetFiatValue(a, fiat, fiatPrices)}
                                fiat={fiat}
                                discreet={discreet}
                                snapKey={`${p.id}:${a.symbol}`}
                              />
                            </button>
                          ))}
                        </div>
                      </div>
                    ) : null}
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {!seedReady ? (
          <div className="sidebar-footer">
            <button
              type="button"
              className="nav-item"
              onClick={() => {
                setSeedInitialMode(status?.has_seed ? "create" : "choose");
                setSeedLockPath(Boolean(status?.has_seed));
                go("seed");
              }}
            >
              {t("shell.setupSeed")}
            </button>
          </div>
        ) : null}
      </aside>

      <section className="main">
        <div
          className="anim-page"
          style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}
        >
          {view === "settings" ? <SettingsScreen /> : null}
          {view === "swap" ? (
            <SwapScreen
              portfolios={portfolios}
              balances={balances}
              fiat={fiat}
              discreet={discreet}
              onAddPortfolio={() => go("add")}
              onFundsMoved={(portfolioIds) => {
                for (const id of portfolioIds) {
                  invalidatePortfolioHistory(id);
                }
                invalidateOverviewLedger(portfolioIdsKey);
                setTxLedger(null);
                window.setTimeout(() => {
                  void reloadBalances();
                }, 8_000);
              }}
            />
          ) : null}
          {view === "seed" && !seedReady ? (
            <SeedSetup
              key={`${seedInitialMode}-${seedLockPath ? "locked" : "free"}`}
              initialMode={seedInitialMode}
              lockPath={seedLockPath}
              onDone={async () => {
                clearSeedSetupIntent();
                await reload();
                go("home");
              }}
            />
          ) : null}
          {view === "add" ? (
            <AddPortfolio
              existing={portfolios}
              onDone={async (createdId) => {
                try {
                  const list = await reloadList();
                  const id = createdId ?? list[list.length - 1]?.id;
                  void reloadBalances();
                  if (id) openPortfolio(id);
                  else go("home");
                } catch (e) {
                  showError(parseInvokeError(e).message);
                  go("home");
                }
              }}
              onCancel={() => go(selectedId ? "portfolio" : "home")}
            />
          ) : null}

          {view === "trezor-sync" ? (
            <SyncTrezorPanel
              trezorStatus={trezorStatus}
              onDone={async () => {
                await reload();
                go("home");
              }}
              onCancel={() => go("home")}
            />
          ) : null}

          {view === "home" ? (
            <div className="content">
              <div className="home-page">
                <div className="balance-hero">
                  <p className="total-label">{t("shell.totalLabel")}</p>
                  <AnimatedMoney
                    as="p"
                    className="total-value"
                    value={liveTotalNum}
                    fiat={fiat}
                    discreet={discreet}
                    snapKey="overview"
                  />
                </div>

                <BalanceChart
                  holdings={chartHoldings}
                  ledger={txLedger}
                  liveTotal={liveTotalNum}
                  fiat={fiat}
                  discreet={discreet}
                  height={172}
                />

                <AnalyticsPanel
                  balances={balances}
                  holdings={chartHoldings}
                  ledger={txLedger}
                  liveTotal={liveTotalNum}
                  fiat={fiat}
                  discreet={discreet}
                  fiatPrices={fiatPrices}
                  portfolioCount={portfolios.length}
                  analyticsEnabled={status?.analytics_enabled !== false}
                  tileOrder={status?.analytics_tile_order}
                  hiddenTiles={status?.analytics_hidden_tiles}
                />

                <section className="recent-activity" aria-label={t("shell.recentActivity")}>
                  <div className="recent-activity__head">
                    <h3 className="recent-activity__heading">{t("shell.recentActivity")}</h3>
                  </div>
                  {recentLoading && recentVisible.length === 0 ? (
                    <div className="recent-activity__list" aria-busy="true">
                      {[0, 1, 2].map((i) => (
                        <div
                          key={i}
                          className="recent-activity__row--skel"
                          aria-hidden
                        />
                      ))}
                    </div>
                  ) : recentVisible.length === 0 ? (
                    <p className="recent-activity__empty">{t("shell.recentActivityEmpty")}</p>
                  ) : (
                    <div className="recent-activity__list anim-stagger">
                      {recentVisible.map((item) => {
                        const dirRaw = item.tx.direction.toLowerCase();
                        const dir =
                          dirRaw.includes("in") || dirRaw.includes("recv")
                            ? "in"
                            : dirRaw.includes("out") || dirRaw.includes("send")
                              ? "out"
                              : "self";
                        const title =
                          dir === "in"
                            ? t("portfolio.txTitle.in", { defaultValue: "Received" })
                            : dir === "out"
                              ? t("portfolio.txTitle.out", { defaultValue: "Sent" })
                              : t("portfolio.txTitle.self", { defaultValue: "Self-transfer" });
                        const timeLabel = formatTxTime(item.tx.timestamp);
                        const sign = dir === "out" ? "−" : dir === "in" ? "+" : "";
                        const expanded = expandedRecent === item.key;
                        const counterpartyLabel =
                          dir === "in"
                            ? t("portfolio.txCounterparty.in")
                            : t("portfolio.txCounterparty.out");
                        const statusRaw = item.tx.status.toLowerCase();
                        const statusKind = statusRaw.includes("fail")
                          ? "failed"
                          : statusRaw.includes("pending") || statusRaw.includes("unconfirmed")
                            ? "pending"
                            : "ok";
                        const statusLabel =
                          statusKind === "failed"
                            ? t("portfolio.txFailed", { defaultValue: "Failed" })
                            : statusKind === "pending"
                              ? t("portfolio.txPending", { defaultValue: "Pending" })
                              : t("portfolio.txConfirmed", { defaultValue: "Confirmed" });
                        return (
                          <div
                            key={item.key}
                            className={`recent-activity__item${expanded ? " is-expanded" : ""}`}
                          >
                            <button
                              type="button"
                              className="recent-activity__summary"
                              aria-expanded={expanded}
                              onClick={() =>
                                setExpandedRecent(expanded ? null : item.key)
                              }
                            >
                              <span className={`recent-activity__icon recent-activity__icon--${dir}`} aria-hidden>
                                {dir === "in" ? (
                                  <TxIconReceived size={28} />
                                ) : dir === "out" ? (
                                  <TxIconSent size={28} />
                                ) : (
                                  <TxIconSelf size={28} />
                                )}
                              </span>
                              <span className="recent-activity__copy">
                                <span className="recent-activity__title">{title}</span>
                                <span className="recent-activity__meta">
                                  {item.portfolioName}
                                  {timeLabel ? ` · ${timeLabel}` : ""}
                                </span>
                              </span>
                              <span className="recent-activity__values">
                                <span className={`recent-activity__qty recent-activity__qty--${dir}`}>
                                  {discreet
                                    ? "••••"
                                    : `${sign}${formatAmount(item.tx.amount, false, 6)}`}
                                </span>
                                {!discreet ? (
                                  <span className="recent-activity__asset">
                                    <AssetIcon symbol={item.tx.symbol} size={14} />
                                    <span>{item.tx.symbol}</span>
                                  </span>
                                ) : null}
                              </span>
                              <IconChevronDown size={16} className="recent-activity__chevron" />
                            </button>
                            <div
                              className="recent-activity__details-wrap"
                              aria-hidden={!expanded}
                            >
                              <div className="recent-activity__details-inner">
                                <div className="recent-activity__details">
                                  <dl className="tx-detail-list">
                                    <div className="tx-detail-list__row">
                                      <dt>{t("portfolio.asset", { defaultValue: "Asset" })}</dt>
                                      <dd className="recent-activity__asset-dd">
                                        <AssetIcon symbol={item.tx.symbol} size={18} />
                                        <span>{item.tx.symbol}</span>
                                      </dd>
                                    </div>
                                    <div className="tx-detail-list__row">
                                      <dt>{t("portfolio.txid")}</dt>
                                      <dd>
                                        {item.tx.explorer_url ? (
                                          <a
                                            className="tx-row__txid-link mono"
                                            href={item.tx.explorer_url}
                                            target="_blank"
                                            rel="noreferrer"
                                            title={t("portfolio.viewOnExplorer")}
                                          >
                                            {shortHash(item.tx.txid, 10, 8)}
                                          </a>
                                        ) : (
                                          <span className="mono">
                                            {shortHash(item.tx.txid, 10, 8)}
                                          </span>
                                        )}
                                      </dd>
                                    </div>
                                    {item.tx.counterparty ? (
                                      <div className="tx-detail-list__row">
                                        <dt>{counterpartyLabel}</dt>
                                        <dd>
                                          <span className="mono">
                                            {shortHash(item.tx.counterparty, 10, 8)}
                                          </span>
                                          <button
                                            type="button"
                                            className="tx-row__copy"
                                            onClick={() => {
                                              void navigator.clipboard.writeText(
                                                item.tx.counterparty ?? "",
                                              );
                                              setCopiedRecent(item.tx.counterparty ?? "");
                                              window.setTimeout(
                                                () => setCopiedRecent(null),
                                                1600,
                                              );
                                            }}
                                          >
                                            <IconCopy size={14} />
                                            {copiedRecent === item.tx.counterparty
                                              ? t("common.copied")
                                              : t("common.copy")}
                                          </button>
                                        </dd>
                                      </div>
                                    ) : null}
                                    <div className="tx-detail-list__row">
                                      <dt>{t("portfolio.txStatus")}</dt>
                                      <dd>
                                        <span
                                          className={`tx-row__badge tx-row__badge--${statusKind}`}
                                        >
                                          {statusLabel}
                                        </span>
                                      </dd>
                                    </div>
                                    {item.tx.fee ? (
                                      <div className="tx-detail-list__row">
                                        <dt>{t("portfolio.txFee")}</dt>
                                        <dd>
                                          {formatQty(item.tx.fee, item.tx.symbol, discreet, 8)}
                                        </dd>
                                      </div>
                                    ) : null}
                                  </dl>
                                  <div className="recent-activity__detail-actions">
                                    <button
                                      type="button"
                                      className="btn btn-sm"
                                      onClick={() =>
                                        openPortfolio(item.portfolioId, "history")
                                      }
                                    >
                                      {t("shell.viewInPortfolio", {
                                        defaultValue: "View in portfolio",
                                      })}
                                    </button>
                                  </div>
                                </div>
                              </div>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </section>
              </div>
            </div>
          ) : null}

          {view === "portfolio" && selected ? (
            <PortfolioDetail
              key={selected.id}
              portfolio={selected}
              balance={selectedBal}
              initialTab={detailTab}
              trezorConnected={
                Boolean(trezorStatus?.available) && (trezorStatus?.device_count ?? 0) > 0
              }
              onBalanceChange={upsertBalance}
              onChanged={async (opts?: { reloadBalances?: boolean }) => {
                await reloadList();
                if (opts?.reloadBalances !== false) void reloadBalances();
              }}
            />
          ) : null}
        </div>
      </section>

      {contextMenu ? (
        <div
          className="asset-menu__panel context-menu-floating"
          role="menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <button
            type="button"
            className="asset-menu__item"
            role="menuitem"
            onClick={() => {
              const p = portfolios.find((x) => x.id === contextMenu.id);
              if (p) startRename(p.id, p.name);
            }}
          >
            {t("portfolio.rename")}
          </button>
          <button
            type="button"
            className="asset-menu__item"
            role="menuitem"
            disabled={rowBusy === contextMenu.id}
            onClick={() => void rescanPortfolio(contextMenu.id)}
          >
            {t("portfolio.rescan")}
          </button>
          <button
            type="button"
            className="asset-menu__item is-danger"
            role="menuitem"
            disabled={rowBusy === contextMenu.id}
            onClick={() => void deletePortfolioById(contextMenu.id)}
          >
            {t("portfolio.delete")}
          </button>
        </div>
      ) : null}
    </div>
  );
}
