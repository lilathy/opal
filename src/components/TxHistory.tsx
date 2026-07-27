import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { canBumpFee, type PortfolioRecord, type TxRow } from "../lib/api";
import { coinIdForChain, coinIdForSymbol } from "../lib/charts";
import {
  formatAmount,
  formatMoney,
  formatQty,
  formatTxDayGroup,
  formatTxTime,
  shortHash,
} from "../lib/format";
import { IconChevronDown, IconCopy } from "./UiIcons";
import { TxIconReceived, TxIconSelf, TxIconSent } from "./TxIcons";

export type HistoryFilter = "all" | "in" | "out" | "pending";

type Props = {
  rows: TxRow[];
  loading: boolean;
  filter: HistoryFilter;
  onFilterChange: (f: HistoryFilter) => void;
  discreet: boolean;
  portfolio: PortfolioRecord;
  fiat: string;
  fiatPrices: Record<string, number>;
  busy?: boolean;
  onBumpFee?: (txid: string) => void;
};

type DaySection = {
  key: string;
  label: string;
  rows: TxRow[];
};

function normalizeDir(direction: string): "in" | "out" | "self" {
  const d = direction.toLowerCase();
  if (d.includes("in") || d.includes("recv")) return "in";
  if (d.includes("out") || d.includes("send")) return "out";
  return "self";
}

function isPendingStatus(status: string): boolean {
  const s = status.toLowerCase();
  return s.includes("pending") || s.includes("unconfirmed");
}

function isFailedStatus(status: string): boolean {
  return status.toLowerCase().includes("fail");
}

function friendlyStatus(
  status: string,
  t: (key: string, opts?: { defaultValue?: string }) => string,
): { kind: "pending" | "failed" | "ok"; label: string } {
  if (isFailedStatus(status)) {
    return { kind: "failed", label: t("portfolio.txFailed", { defaultValue: "Failed" }) };
  }
  if (isPendingStatus(status)) {
    return { kind: "pending", label: t("portfolio.txPending", { defaultValue: "Pending" }) };
  }
  return { kind: "ok", label: t("portfolio.txConfirmed", { defaultValue: "Confirmed" }) };
}

export function TxHistory({
  rows,
  loading,
  filter,
  onFilterChange,
  discreet,
  portfolio,
  fiat,
  fiatPrices,
  busy = false,
  onBumpFee,
}: Props) {
  const { t } = useTranslation();
  const [expandedTx, setExpandedTx] = useState<string | null>(null);
  const [copiedField, setCopiedField] = useState<string | null>(null);

  const filtered = useMemo(() => {
    return rows.filter((h) => {
      const dir = h.direction.toLowerCase();
      const st = h.status.toLowerCase();
      if (filter === "in") return dir.includes("in") || dir.includes("recv");
      if (filter === "out") return dir.includes("out") || dir.includes("send");
      if (filter === "pending") return st.includes("pending") || st.includes("unconfirmed");
      return true;
    });
  }, [rows, filter]);

  const sections = useMemo((): DaySection[] => {
    const map = new Map<string, DaySection>();
    const order: string[] = [];
    for (const row of filtered) {
      const { key, label } = formatTxDayGroup(row.timestamp, {
        today: t("portfolio.txToday", { defaultValue: "Today" }),
        yesterday: t("portfolio.txYesterday", { defaultValue: "Yesterday" }),
        pending: t("portfolio.txPending", { defaultValue: "Pending" }),
      });
      let section = map.get(key);
      if (!section) {
        section = { key, label, rows: [] };
        map.set(key, section);
        order.push(key);
      }
      section.rows.push(row);
    }
    return order.map((k) => map.get(k)!);
  }, [filtered, t]);

  function copyValue(value: string) {
    void navigator.clipboard.writeText(value);
    setCopiedField(value);
    window.setTimeout(() => {
      setCopiedField((cur) => (cur === value ? null : cur));
    }, 1500);
  }

  function feeFiat(fee: string, symbol: string): string {
    const qty = Number(fee);
    if (!Number.isFinite(qty) || qty <= 0) return formatMoney(0, fiat, discreet);
    // Network fees are paid in the chain's native asset.
    const coinId =
      coinIdForChain(portfolio.chain) ?? coinIdForSymbol(symbol);
    const px = coinId ? fiatPrices[coinId] : undefined;
    if (px == null || !(px > 0)) {
      return formatQty(fee, symbol, discreet, 8);
    }
    return formatMoney(qty * px, fiat, discreet);
  }

  return (
    <div className="stack tx-history">
      <div className="tx-toolbar">
        <div className="segmented segmented--4" role="tablist" aria-label={t("portfolio.history")}>
          {(["all", "in", "out", "pending"] as const).map((f) => (
            <button
              key={f}
              type="button"
              role="tab"
              aria-selected={filter === f}
              className={`segmented__item${filter === f ? " is-active" : ""}`}
              onClick={() => onFilterChange(f)}
            >
              {t(`portfolio.filter.${f}`)}
            </button>
          ))}
        </div>
      </div>

      {loading && rows.length === 0 ? (
        <div className="tx-list" aria-busy="true">
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="tx-row tx-row--skeleton" aria-hidden style={{ animationDelay: `${i * 60}ms` }}>
              <span className="tx-row__icon tx-row__icon--skel" />
              <span className="tx-row__skeleton-lines">
                <span />
                <span />
              </span>
              <span className="tx-row__skeleton-amt" />
            </div>
          ))}
        </div>
      ) : filtered.length === 0 ? (
        <div className="empty-state tx-empty">
          <p className="tx-empty__title">{t("portfolio.noHistory")}</p>
          <p className="tx-empty__hint">{t("portfolio.noHistoryHint")}</p>
        </div>
      ) : (
        <div className="tx-sections anim-stagger">
          {sections.map((section) => (
            <section key={section.key} className="tx-day">
              <h4 className="tx-day__label">{section.label}</h4>
              <div className="tx-list">
                {section.rows.map((h) => {
                  const pending = isPendingStatus(h.status);
                  const failed = isFailedStatus(h.status);
                  const dir = normalizeDir(h.direction);
                  const known = h.amount !== "—";
                  const counterpartyLabel =
                    dir === "in"
                      ? t("portfolio.txCounterparty.in")
                      : t("portfolio.txCounterparty.out");
                  const expanded = expandedTx === h.txid;
                  const status = friendlyStatus(h.status, t);
                  const title = failed
                    ? t("portfolio.txFailed", { defaultValue: "Failed" })
                    : t(`portfolio.txTitle.${dir}`, {
                        defaultValue:
                          dir === "in" ? "Received" : dir === "out" ? "Sent" : "Self-transfer",
                      });
                  const timeLabel =
                    formatTxTime(h.timestamp) ||
                    (pending ? t("portfolio.txPending", { defaultValue: "Pending" }) : "");

                  return (
                    <div
                      key={h.txid}
                      className={`tx-row${expanded ? " is-expanded" : ""}${failed ? " is-failed" : ""}${pending ? " is-pending" : ""}`}
                    >
                      <button
                        type="button"
                        className="tx-row__summary"
                        onClick={() => setExpandedTx(expanded ? null : h.txid)}
                        aria-expanded={expanded}
                      >
                        <span
                          className={`tx-row__icon tx-row__icon--${dir}${failed ? " is-failed" : ""}`}
                        >
                          {dir === "in" ? (
                            <TxIconReceived size={32} />
                          ) : dir === "out" ? (
                            <TxIconSent size={32} />
                          ) : (
                            <TxIconSelf size={32} />
                          )}
                        </span>

                        <div className="tx-row__main">
                          <div className="tx-row__top">
                            <span className="tx-row__title">{title}</span>
                            {status.kind !== "ok" ? (
                              <span className={`tx-row__badge tx-row__badge--${status.kind}`}>
                                {status.label}
                              </span>
                            ) : null}
                          </div>
                          <div className="tx-row__meta">
                            {timeLabel ? <span>{timeLabel}</span> : null}
                            {h.counterparty ? (
                              <>
                                {timeLabel ? (
                                  <span className="tx-row__sep" aria-hidden>
                                    ·
                                  </span>
                                ) : null}
                                <span className="tx-row__counterparty">
                                  {counterpartyLabel}{" "}
                                  <span className="mono">{shortHash(h.counterparty)}</span>
                                </span>
                              </>
                            ) : null}
                          </div>
                        </div>

                        <div className="tx-row__values">
                          <span className={`tx-row__amount tx-row__amount--${dir}`}>
                            {discreet
                              ? "••••"
                              : known
                                ? `${dir === "out" ? "−" : dir === "in" ? "+" : ""}${formatAmount(h.amount, false, 8)}`
                                : "—"}
                          </span>
                          {!discreet && known ? (
                            <span className="tx-row__symbol">{h.symbol}</span>
                          ) : null}
                        </div>

                        <IconChevronDown size={16} className="tx-row__chevron" />
                      </button>

                      <div className="tx-row__details-wrap" aria-hidden={!expanded}>
                        <div className="tx-row__details-inner">
                          <div className="tx-row__details">
                            <dl className="tx-detail-list">
                              <div className="tx-detail-list__row">
                                <dt>{t("portfolio.txid")}</dt>
                                <dd>
                                  <a
                                    className="tx-row__txid-link mono"
                                    href={h.explorer_url}
                                    target="_blank"
                                    rel="noreferrer"
                                    title={t("portfolio.viewOnExplorer")}
                                  >
                                    {shortHash(h.txid, 10, 8)}
                                  </a>
                                </dd>
                              </div>

                              {h.counterparty ? (
                                <div className="tx-detail-list__row">
                                  <dt>{counterpartyLabel}</dt>
                                  <dd>
                                    <span className="mono">{shortHash(h.counterparty, 10, 8)}</span>
                                    <button
                                      type="button"
                                      className="tx-row__copy"
                                      onClick={() => copyValue(h.counterparty ?? "")}
                                    >
                                      <IconCopy size={14} />
                                      {copiedField === h.counterparty
                                        ? t("common.copied")
                                        : t("common.copy")}
                                    </button>
                                  </dd>
                                </div>
                              ) : null}

                              <div className="tx-detail-list__row">
                                <dt>{t("portfolio.txStatus")}</dt>
                                <dd>
                                  <span className={`tx-row__badge tx-row__badge--${status.kind}`}>
                                    {status.label}
                                  </span>
                                </dd>
                              </div>

                              {h.fee ? (
                                <div className="tx-detail-list__row">
                                  <dt>{t("portfolio.txFee")}</dt>
                                  <dd>{feeFiat(h.fee, h.symbol)}</dd>
                                </div>
                              ) : null}
                            </dl>

                            {pending && canBumpFee(portfolio.chain, portfolio.kind) && onBumpFee ? (
                              <div className="tx-row__detail-actions">
                                <button
                                  type="button"
                                  className="btn btn-sm"
                                  disabled={busy}
                                  onClick={() => onBumpFee(h.txid)}
                                >
                                  {t("portfolio.bumpFee")}
                                </button>
                              </div>
                            ) : null}
                          </div>
                        </div>
                      </div>
                    </div>
                  );
                })}
              </div>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}
