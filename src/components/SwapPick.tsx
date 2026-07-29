import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { AssetIcon, ChainIcon } from "./CryptoIcons";
import { IconChevronDown } from "./UiIcons";

export type SwapPickAsset = {
  symbol: string;
  /** Optional secondary line under the token (e.g. balance). */
  detail?: string;
};

export type SwapPickGroup = {
  portfolioId: string;
  portfolioName: string;
  chain: string;
  assets: SwapPickAsset[];
};

type Props = {
  groups: SwapPickGroup[];
  /** Selected as `portfolioId:SYMBOL`. */
  value: string | null;
  onChange: (portfolioId: string, symbol: string) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  "aria-label"?: string;
};

function parseValue(value: string | null): { portfolioId: string; symbol: string } | null {
  if (!value) return null;
  const i = value.indexOf(":");
  if (i <= 0) return null;
  return { portfolioId: value.slice(0, i), symbol: value.slice(i + 1) };
}

export function SwapPick({
  groups,
  value,
  onChange,
  placeholder = "Select…",
  disabled,
  className,
  "aria-label": ariaLabel,
}: Props) {
  const triggerId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const openedOnce = useRef(false);
  const [open, setOpen] = useState(false);
  const [menuStyle, setMenuStyle] = useState<CSSProperties>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const parsed = parseValue(value);

  const selectedGroup = useMemo(
    () => groups.find((g) => g.portfolioId === parsed?.portfolioId) ?? null,
    [groups, parsed?.portfolioId],
  );
  const selectedAsset = useMemo(() => {
    if (!selectedGroup || !parsed) return null;
    return (
      selectedGroup.assets.find((a) => a.symbol.toUpperCase() === parsed.symbol.toUpperCase()) ??
      null
    );
  }, [selectedGroup, parsed]);

  // Only auto-expand the selected group the first time this menu opens -
  // never fight the user on later toggles or when parent `groups` recreate.
  useEffect(() => {
    if (!open) {
      openedOnce.current = false;
      return;
    }
    if (openedOnce.current) return;
    openedOnce.current = true;
    const id = parsed?.portfolioId;
    if (!id) return;
    const group = groups.find((g) => g.portfolioId === id);
    if (!group || group.assets.length <= 1) return;
    setExpanded((prev) => (prev[id] ? prev : { ...prev, [id]: true }));
  }, [open, parsed?.portfolioId, groups]);

  function placeMenu() {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const gap = 6;
    const width = Math.max(rect.width, 240);
    const left = Math.min(Math.max(8, rect.left), window.innerWidth - width - 8);
    const spaceBelow = window.innerHeight - rect.bottom - gap;
    const openUp = spaceBelow < 280 && rect.top > spaceBelow;
    setMenuStyle({
      position: "fixed",
      left,
      width,
      top: openUp ? undefined : rect.bottom + gap,
      bottom: openUp ? window.innerHeight - rect.top + gap : undefined,
      maxHeight: Math.min(320, openUp ? rect.top - 16 : spaceBelow),
      zIndex: 4000,
    });
  }

  useLayoutEffect(() => {
    if (!open) return;
    placeMenu();
  }, [open, expanded]);

  useEffect(() => {
    if (!open) return;
    function onDoc(e: MouseEvent) {
      const t = e.target as Node;
      if (rootRef.current?.contains(t) || menuRef.current?.contains(t)) return;
      setOpen(false);
    }
    function onKey(e: globalThis.KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    function onReposition() {
      placeMenu();
    }
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onReposition);
    window.addEventListener("scroll", onReposition, true);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onReposition);
      window.removeEventListener("scroll", onReposition, true);
    };
  }, [open]);

  function onTriggerKey(e: KeyboardEvent<HTMLButtonElement>) {
    if (disabled) return;
    if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      setOpen(true);
    }
  }

  function choose(portfolioId: string, symbol: string) {
    onChange(portfolioId, symbol);
    setOpen(false);
  }

  function toggleGroup(id: string) {
    setExpanded((prev) => ({ ...prev, [id]: !prev[id] }));
  }

  const triggerIcon: ReactNode = selectedAsset ? (
    <AssetIcon symbol={selectedAsset.symbol} size={24} />
  ) : selectedGroup ? (
    <ChainIcon chain={selectedGroup.chain} size={24} />
  ) : null;

  const triggerLabel = selectedAsset
    ? selectedAsset.symbol
    : selectedGroup
      ? selectedGroup.portfolioName
      : placeholder;

  const triggerSub =
    selectedAsset && selectedGroup ? selectedGroup.portfolioName : null;

  const menu =
    open && typeof document !== "undefined"
      ? createPortal(
          <div
            ref={menuRef}
            className="swap-pick__menu"
            role="listbox"
            aria-labelledby={triggerId}
            style={menuStyle}
          >
            {groups.map((g) => {
              const multi = g.assets.length > 1;
              const isOpen = multi ? !!expanded[g.portfolioId] : false;
              const groupSelected = g.portfolioId === parsed?.portfolioId;

              if (!multi) {
                const only = g.assets[0];
                if (!only) return null;
                const active =
                  groupSelected &&
                  only.symbol.toUpperCase() === (parsed?.symbol ?? "").toUpperCase();
                return (
                  <button
                    key={g.portfolioId}
                    type="button"
                    role="option"
                    aria-selected={active}
                    className={`swap-pick__row${active ? " is-active" : ""}`}
                    onClick={() => choose(g.portfolioId, only.symbol)}
                  >
                    <span className="swap-pick__row-icon">
                      <ChainIcon chain={g.chain} size={22} />
                    </span>
                    <span className="swap-pick__row-copy">
                      <span className="swap-pick__row-title">{g.portfolioName}</span>
                      {only.detail ? (
                        <span className="swap-pick__row-detail swap-pick__row-detail--bal">
                          {only.detail}
                        </span>
                      ) : (
                        <span className="swap-pick__row-detail">{only.symbol}</span>
                      )}
                    </span>
                  </button>
                );
              }

              return (
                <div
                  key={g.portfolioId}
                  className={`swap-pick__group${isOpen ? " is-expanded" : ""}`}
                >
                  <button
                    type="button"
                    className={`swap-pick__group-head${groupSelected ? " is-current" : ""}${isOpen ? " is-open" : ""}`}
                    aria-expanded={isOpen}
                    onClick={() => toggleGroup(g.portfolioId)}
                  >
                    <span className="swap-pick__row-icon">
                      <ChainIcon chain={g.chain} size={22} />
                    </span>
                    <span className="swap-pick__row-copy">
                      <span className="swap-pick__row-title">{g.portfolioName}</span>
                      <span className="swap-pick__row-detail">
                        {g.assets.map((a) => a.symbol).join(" · ")}
                      </span>
                    </span>
                    <IconChevronDown size={16} className="swap-pick__chevron" />
                  </button>
                  <div className="swap-pick__tokens-wrap" aria-hidden={!isOpen}>
                    <div className="swap-pick__tokens">
                      {g.assets.map((a) => {
                        const active =
                          groupSelected &&
                          a.symbol.toUpperCase() === (parsed?.symbol ?? "").toUpperCase();
                        return (
                          <button
                            key={`${g.portfolioId}:${a.symbol}`}
                            type="button"
                            role="option"
                            aria-selected={active}
                            tabIndex={isOpen ? 0 : -1}
                            className={`swap-pick__token${active ? " is-active" : ""}`}
                            onClick={() => choose(g.portfolioId, a.symbol)}
                          >
                            <span className="swap-pick__row-icon">
                              <AssetIcon symbol={a.symbol} size={20} />
                            </span>
                            <span className="swap-pick__row-copy">
                              <span className="swap-pick__row-title">{a.symbol}</span>
                              {a.detail ? (
                                <span className="swap-pick__row-detail swap-pick__row-detail--bal">
                                  {a.detail}
                                </span>
                              ) : null}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>,
          document.body,
        )
      : null;

  return (
    <div
      ref={rootRef}
      className={["swap-pick", className ?? ""].filter(Boolean).join(" ")}
    >
      <button
        ref={triggerRef}
        id={triggerId}
        type="button"
        className={`swap-pick__trigger${open ? " is-open" : ""}`}
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={onTriggerKey}
      >
        {triggerIcon ? <span className="swap-pick__trigger-icon">{triggerIcon}</span> : null}
        <span className="swap-pick__trigger-copy">
          <span
            className={
              selectedAsset
                ? "swap-pick__trigger-title"
                : "swap-pick__trigger-title is-placeholder"
            }
          >
            {triggerLabel}
          </span>
          {triggerSub ? <span className="swap-pick__trigger-sub">{triggerSub}</span> : null}
        </span>
        <IconChevronDown size={16} className="swap-pick__chevron" />
      </button>
      {menu}
    </div>
  );
}
