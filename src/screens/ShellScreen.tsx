import { listen } from "@tauri-apps/api/event";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { AssetIcon, ChainIcon } from "../components/CryptoIcons";
import {
  IconChevronDown,
  IconHome,
  IconPlus,
  IconSettings,
  IconSwap,
} from "../components/UiIcons";
import {
  api,
  parseInvokeError,
  type AssetBalance,
  type PortfolioBalance,
  type PortfolioRecord,
  type TrezorStatus,
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
  loadOverviewLedger,
  mergeLedgerEvents,
  saveOverviewLedger,
} from "../lib/chartCache";
import { formatMoney, formatQty } from "../lib/format";
import { BalanceChart } from "../components/BalanceChart";
import { useFiatPrices } from "../hooks/useFiatPrices";
import { useIncomingWatcher } from "../hooks/useIncomingWatcher";
import { useSortableList } from "../hooks/useSortableList";
import { SyncTrezorPanel } from "../components/SyncTrezorPanel";
import { TrezorSpinner } from "../components/TrezorSpinner";
import { useVault } from "../state/vault";
import { useNotify } from "../state/notifications";
import { AddPortfolio } from "./AddPortfolio";
import { PortfolioDetail } from "./PortfolioDetail";
import { clearSeedSetupIntent, type SeedSetupIntent } from "../lib/seedIntent";
import { SeedSetup } from "./SeedSetup";
import { SettingsScreen } from "./SettingsScreen";
import { SwapScreen } from "./SwapScreen";

type View = "home" | "settings" | "seed" | "add" | "portfolio" | "swap" | "trezor-sync";

const ORDER_KEY = "opal.portfolioOrder";

type ContextMenuState = { id: string; x: number; y: number; surface: "sidebar" | "overview" };

function kindI18nKey(kind: string): string {
  if (kind === "software") return "portfolio.kindSoftware";
  if (kind === "trezor") return "portfolio.kindTrezor";
  if (kind === "watch_only") return "portfolio.kindWatch";
  return kind;
}

function portfolioFiat(
  bal: PortfolioBalance | undefined,
  discreet: boolean,
  fiat: string,
  prices: Record<string, number>,
): string {
  if (discreet) return "••••";
  return formatMoney(portfolioFiatSum(bal, fiat, prices), fiat, false);
}

function portfolioQty(
  bal: PortfolioBalance | undefined,
  discreet: boolean,
  fallbackSymbol: string,
): string {
  if (discreet) return "••••";
  const a = bal?.assets?.[0];
  return formatQty(a?.amount ?? 0, a?.symbol ?? fallbackSymbol, false);
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

/** Native gas/ticker for a chain — excluded from nested token lists. */
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
  const [expandedOverview, setExpandedOverview] = useState<Record<string, boolean>>({});
  const [expandedSidebar, setExpandedSidebar] = useState<Record<string, boolean>>({});
  const lastBalancesFetchAt = useRef(0);
  const portfoliosRef = useRef(portfolios);
  portfoliosRef.current = portfolios;
  const [trezorStatus, setTrezorStatus] = useState<TrezorStatus | null>(null);

  // ── Right-click context menu (rename / rescan / delete) ─────────
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  // The same portfolio can be visible in both the sidebar and the overview
  // list at once — track *where* the rename started so only that one row
  // switches into edit mode. Two simultaneously-autofocused inputs would
  // otherwise steal focus from one another and instantly blur-commit,
  // making rename look like it "does nothing".
  const [renameSurface, setRenameSurface] = useState<"sidebar" | "overview" | null>(null);
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
  const [scanBusy, setScanBusy] = useState(false);
  const [txLedger, setTxLedger] = useState<LedgerEvent[] | null>(null);
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

  function toggleOverviewExpanded(id: string) {
    setExpandedOverview((prev) => ({ ...prev, [id]: !prev[id] }));
  }

  function toggleSidebarExpanded(id: string) {
    setExpandedSidebar((prev) => ({ ...prev, [id]: !prev[id] }));
  }

  // Incomplete seed setup after an interrupted session — resume backup only.
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
  // exist — new ones append at the end, removed ones drop out. Prefer vault
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

  function openContextMenu(e: React.MouseEvent, id: string, surface: "sidebar" | "overview") {
    e.preventDefault();
    if (editMode) return;
    const x = Math.min(e.clientX, window.innerWidth - 200);
    const y = Math.min(e.clientY, window.innerHeight - 180);
    setContextMenu({ id, x, y, surface });
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

  function startRename(id: string, name: string, surface: "sidebar" | "overview") {
    setContextMenu(null);
    renameCommitInFlight.current = null;
    renameDraftRef.current = name;
    setRenameDraft(name);
    setRenameSurface(surface);
    setRenamingId(id);
    window.setTimeout(() => renameInputRef.current?.select(), 0);
  }

  async function commitRename(id: string) {
    // Guard against Enter + blur both firing for the same edit.
    if (renameCommitInFlight.current === id) return;
    renameCommitInFlight.current = id;
    // Always read from the ref — blur can fire after a re-render where a
    // stale closure still sees the pre-keystroke draft.
    const next = renameDraftRef.current.trim();
    const current = portfolios.find((p) => p.id === id);
    if (!next || !current || next === current.name) {
      setRenamingId(null);
      setRenameSurface(null);
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
      setRenameSurface(null);
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

  /** Scrape each portfolio on its own so one hung RPC can't block the rest. */
  async function reloadBalances() {
    const list = portfoliosRef.current;
    if (!list.length) return;

    const jobs = list.map(async (p) => {
      const bal = await scrapePortfolioBalance(p.id);
      if (bal) setBalances((prev) => applyLiveBalances(prev, [bal]));
    });

    await Promise.allSettled(jobs);
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

  // Poll balances while visible. Scrapes themselves are fast now (~2–3s);
  // don't hammer public RPCs every 1.5s on top of PortfolioDetail.
  useEffect(() => {
    const maybeRefresh = () => {
      if (document.visibilityState !== "visible") return;
      if (Date.now() - lastBalancesFetchAt.current < 2_500) return;
      lastBalancesFetchAt.current = Date.now();
      void reloadBalances();
    };
    maybeRefresh();
    const tick = window.setInterval(maybeRefresh, 3_000);
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
      // Best-effort — the widget just shows "not detected" on failure.
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
        // Background KI sync for watch-only XMR — does not block scrapes when offline.
        const hasXmrTrezor =
          created.some((p) => p.chain === "xmr") ||
          portfolios.some((p) => p.kind === "trezor" && p.chain === "xmr");
        if (hasXmrTrezor && !cancelled) {
          void api.trezorSyncXmrKeyImages().catch(() => {
            /* device busy / user cancelled confirm — next reconnect retries */
          });
        }
      } catch {
        // Device may still be unlocking — clear so the next status poll retries.
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

  const totalFiat = useMemo(() => {
    let sum = 0;
    for (const b of balances) {
      sum += portfolioFiatSum(b, fiat, fiatPrices);
    }
    return formatMoney(sum, fiat, discreet);
  }, [balances, discreet, fiat, fiatPrices]);

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
      return;
    }
    let cancelled = false;

    const cached = loadOverviewLedger(portfolioIdsKey);
    if (cached?.length) {
      setTxLedger(cached);
    } else {
      setTxLedger(null);
    }

    // Prefetch default 7D prices while history loads — chart paints the
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

    const ingest = (part: LedgerEvent[]) => {
      if (!part.length) return;
      merged = mergeLedgerEvents(merged, part);
      if (cancelled || !merged.length) return;
      setTxLedger([...merged]);
      saveOverviewLedger(portfolioIdsKey, merged);
    };

    void (async () => {
      await Promise.all(
        portfolios.map(async (p) => {
          if (cancelled) return;
          try {
            const rows = await api.portfolioHistory(p.id);
            if (cancelled) return;
            ingest(txsToLedger(rows));
          } catch {
            /* best-effort per portfolio */
          }
        }),
      );
      if (cancelled) return;
      // Still nothing after a full pass — empty chart, not infinite spinner.
      if (!merged.length) setTxLedger([]);
    })();

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [portfolioIdsKey]);

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

  async function scanSeedWallets() {
    if (!seedReady || scanBusy) return;
    setScanBusy(true);
    try {
      const found = await api.walletDiscoverPortfolios();
      await reload();
      notify({
        kind: found.length ? "success" : "warning",
        title: found.length ? t("seed.discoveredTitle") : t("shell.scanWallets"),
        message: found.length
          ? t("seed.discoveredBody", { count: found.length })
          : t("shell.emptyPortfolios"),
      });
    } catch (e) {
      notify({
        kind: "error",
        title: t("notifications.errorTitle"),
        message: parseInvokeError(e).message,
      });
    } finally {
      setScanBusy(false);
    }
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
                  <span style={{ fontSize: 11, fontWeight: 600 }}>{t("common.done", { defaultValue: "Done" })}</span>
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
                      {renamingId === p.id && renameSurface === "sidebar" ? (
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
                                setRenameSurface(null);
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
                          onContextMenu={(e) => openContextMenu(e, p.id, "sidebar")}
                        >
                          <span className="crypto-badge portfolio-item__icon">
                            <ChainIcon chain={p.chain} size={32} />
                          </span>
                          <span className="portfolio-item-name">{p.name}</span>
                          <span className="portfolio-item-meta">
                            {t(kindI18nKey(p.kind))}
                          </span>
                          <span className="portfolio-item-bal">
                            {portfolioFiat(bal, discreet, fiat, fiatPrices)}
                          </span>
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
                              <span className="sidebar-token__bal">
                                {discreet
                                  ? "••••"
                                  : formatMoney(assetFiatValue(a, fiat, fiatPrices), fiat, false)}
                              </span>
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
                  <p className="total-value anim-balance">{totalFiat}</p>
                </div>

                <BalanceChart
                  holdings={chartHoldings}
                  ledger={txLedger}
                  fiat={fiat}
                  discreet={discreet}
                  height={172}
                />

                <div>
                  <div className="assets-toolbar">
                    <h3>{t("shell.portfolios")}</h3>
                  </div>
                  {portfolios.length === 0 ? (
                    <div className="empty-state" style={{ marginTop: 12 }}>
                      <p style={{ margin: "0 0 12px" }}>{t("shell.emptyPortfolios")}</p>
                      <div className="row" style={{ gap: 10, flexWrap: "wrap", justifyContent: "center" }}>
                        {seedReady ? (
                          <button
                            type="button"
                            className="btn btn-primary"
                            disabled={scanBusy}
                            onClick={() => void scanSeedWallets()}
                          >
                            {scanBusy ? t("shell.scanningWallets") : t("shell.scanWallets")}
                          </button>
                        ) : null}
                        <button
                          type="button"
                          className="btn"
                          onClick={() => go("trezor-sync")}
                        >
                          {t("trezor.syncTitle", { defaultValue: "Sync my Trezor" })}
                        </button>
                      </div>
                    </div>
                  ) : (
                    <div className="token-list anim-stagger" style={{ marginTop: 8 }}>
                      {orderedPortfolios.map((p) => {
                        const balRaw = balances.find((b) => b.portfolio_id === p.id);
                        const bal = balRaw ? reconcilePendingSpend(balRaw) : undefined;
                        const assets = portfolioAssets(bal);
                        const tokens = tokensUnderPortfolio(assets, p.chain);
                        const expandable = tokens.length > 0;
                        const expanded = !!expandedOverview[p.id];
                        return (
                          <div
                            key={p.id}
                            className={`portfolio-group${expanded ? " is-expanded" : ""}`}
                          >
                            <div className="portfolio-group__head">
                              {expandable ? (
                                <button
                                  type="button"
                                  className="portfolio-group__toggle"
                                  aria-expanded={expanded}
                                  aria-label={
                                    expanded
                                      ? t("shell.collapseTokens", { defaultValue: "Hide tokens" })
                                      : t("shell.expandTokens", { defaultValue: "Show tokens" })
                                  }
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    toggleOverviewExpanded(p.id);
                                  }}
                                >
                                  <IconChevronDown size={20} />
                                </button>
                              ) : (
                                <span className="portfolio-group__toggle-spacer" aria-hidden />
                              )}
                              {renamingId === p.id && renameSurface === "overview" ? (
                                <div className="portfolio-group__main portfolio-group__main--renaming">
                                  <span className="crypto-badge portfolio-group__icon">
                                    <ChainIcon chain={p.chain} size={32} />
                                  </span>
                                  <input
                                    ref={renameInputRef}
                                    className="control-input portfolio-group__rename"
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
                                        setRenameSurface(null);
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
                                  className="portfolio-group__main"
                                  onClick={() => openPortfolio(p.id)}
                                  onContextMenu={(e) => openContextMenu(e, p.id, "overview")}
                                >
                                  <span className="crypto-badge portfolio-group__icon">
                                    <ChainIcon chain={p.chain} size={32} />
                                  </span>
                                  <span className="token-row__meta">
                                    <span className="token-row__name">{p.name}</span>
                                    <span className="token-row__symbol">
                                      {t(kindI18nKey(p.kind))}
                                    </span>
                                  </span>
                                  <span className="token-row__values">
                                    <span className="token-row__fiat">
                                      {portfolioFiat(bal, discreet, fiat, fiatPrices)}
                                    </span>
                                    <span className="token-row__qty">
                                      {portfolioQty(bal, discreet, p.chain.toUpperCase())}
                                    </span>
                                  </span>
                                </button>
                              )}
                            </div>
                            {expandable ? (
                              <div
                                className="portfolio-group__tokens"
                                aria-hidden={!expanded}
                              >
                                <div className="portfolio-group__tokens-inner">
                                  {tokens.map((a) => (
                                    <button
                                      key={a.symbol}
                                      type="button"
                                      className="token-row token-row--nested"
                                      tabIndex={expanded ? 0 : -1}
                                      onClick={() => openPortfolio(p.id)}
                                    >
                                      <span className="crypto-badge">
                                        <AssetIcon symbol={a.symbol} size={28} />
                                      </span>
                                      <span className="token-row__meta">
                                        <span className="token-row__name">{a.symbol}</span>
                                        <span className="token-row__symbol">
                                          {p.chain.toUpperCase()}
                                        </span>
                                      </span>
                                      <span className="token-row__values">
                                        <span className="token-row__fiat">
                                          {formatMoney(assetFiatValue(a, fiat, fiatPrices), fiat, discreet)}
                                        </span>
                                        <span className="token-row__qty">
                                          {formatQty(a.amount, a.symbol, discreet)}
                                        </span>
                                      </span>
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
              </div>
            </div>
          ) : null}

          {view === "portfolio" && selected ? (
            <PortfolioDetail
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
              if (p) startRename(p.id, p.name, contextMenu.surface);
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
