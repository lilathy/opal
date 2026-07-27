import { useTranslation } from "react-i18next";
import type { CSSProperties } from "react";
import { AssetIcon } from "./CryptoIcons";
import type { NotificationItem, NotificationKind } from "../state/notifications";
import { useNotify } from "../state/notifications";

function KindGlyph({ kind }: { kind: NotificationKind }) {
  switch (kind) {
    case "incoming":
      return (
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden>
          <path
            d="M12 5v14M12 19l-6-6M12 19l6-6"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      );
    case "success":
      return (
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden>
          <path
            d="M6 12l4 4 8-8"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      );
    case "warning":
      return (
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden>
          <path
            d="M12 8v5M12 16h.01"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
          />
        </svg>
      );
    case "error":
      return (
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden>
          <path
            d="M8 8l8 8M16 8l-8 8"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
          />
        </svg>
      );
  }
}

function NotificationCard({
  n,
  onDismiss,
}: {
  n: NotificationItem;
  onDismiss: (id: string) => void;
}) {
  const { t } = useTranslation();
  const style =
    n.duration > 0
      ? ({ "--notification-duration": `${n.duration}ms` } as CSSProperties)
      : undefined;
  const hasValues = !!(n.amount || n.fiatAmount);

  return (
    <div
      className={`notification notification--${n.kind}${n.leaving ? " is-leaving" : " is-enter"}`}
      role={n.kind === "error" ? "alert" : "status"}
      style={style}
    >
      <span className="notification__icon" aria-hidden>
        {n.symbol ? (
          <span className="crypto-badge">
            <AssetIcon symbol={n.symbol} size={32} />
          </span>
        ) : (
          <span className={`notification__glyph notification__glyph--${n.kind}`}>
            <KindGlyph kind={n.kind} />
          </span>
        )}
      </span>

      <div className="notification__copy">
        <strong className="notification__title">{n.title}</strong>
        {n.message ? <span className="notification__message">{n.message}</span> : null}
        {n.action ? (
          <button
            type="button"
            className="notification__action"
            onClick={() => {
              n.action?.onClick();
              onDismiss(n.id);
            }}
          >
            {n.action.label}
          </button>
        ) : null}
      </div>

      {hasValues ? (
        <div className="notification__values">
          {n.amount ? <span className="notification__amount">{n.amount}</span> : null}
          {n.fiatAmount ? <span className="notification__fiat">{n.fiatAmount}</span> : null}
        </div>
      ) : null}

      <button
        type="button"
        className="notification__close"
        aria-label={t("notifications.dismiss", { defaultValue: "Dismiss" })}
        onClick={() => onDismiss(n.id)}
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
          <path
            d="M7 7l10 10M17 7L7 17"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
          />
        </svg>
      </button>

      {n.duration > 0 ? (
        <div className="notification__timer" aria-hidden>
          <span className="notification__timer-bar" />
        </div>
      ) : null}
    </div>
  );
}

export function NotificationHost() {
  const { items, dismiss } = useNotify();

  if (!items.length) return null;

  return (
    <div className="notification-host" aria-live="polite">
      {items.map((n) => (
        <NotificationCard key={n.id} n={n} onDismiss={dismiss} />
      ))}
    </div>
  );
}
