import {
  createContext,
  useCallback,
  useContext,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { playErrorSound, playIncomingSound } from "../lib/sounds";

export type NotificationKind = "incoming" | "error" | "success" | "warning";

export type NotificationItem = {
  id: string;
  kind: NotificationKind;
  title: string;
  message: string;
  /** Asset ticker - when set, the toast shows that crypto icon. */
  symbol?: string;
  /** Formatted amount line (incoming), e.g. "+0.25 BTC". */
  amount?: string;
  /** Fiat equivalent of the amount, e.g. "$16,420.12". */
  fiatAmount?: string;
  /** Auto-dismiss lifetime in ms (0 = sticky). */
  duration: number;
  leaving?: boolean;
  action?: { label: string; onClick: () => void };
};

export type NotifyInput = Omit<NotificationItem, "id" | "duration" | "leaving"> & {
  duration?: number;
};

type NotificationContext = {
  items: NotificationItem[];
  notify: (item: NotifyInput) => string;
  dismiss: (id: string) => void;
};

const Ctx = createContext<NotificationContext | null>(null);

const MAX_STACK = 4;
export const NOTIFICATION_EXIT_MS = 280;

const DEFAULT_DURATIONS: Record<NotificationKind, number> = {
  incoming: 8000,
  error: 7000,
  success: 5000,
  warning: 6000,
};

export function NotificationProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<NotificationItem[]>([]);
  const autoTimers = useRef<Map<string, number>>(new Map());
  const exitTimers = useRef<Map<string, number>>(new Map());

  const removeNow = useCallback((id: string) => {
    const auto = autoTimers.current.get(id);
    if (auto != null) window.clearTimeout(auto);
    autoTimers.current.delete(id);
    const exit = exitTimers.current.get(id);
    if (exit != null) window.clearTimeout(exit);
    exitTimers.current.delete(id);
    setItems((prev) => prev.filter((n) => n.id !== id));
  }, []);

  const dismiss = useCallback(
    (id: string) => {
      const auto = autoTimers.current.get(id);
      if (auto != null) window.clearTimeout(auto);
      autoTimers.current.delete(id);

      if (exitTimers.current.has(id)) return;

      setItems((prev) => {
        const found = prev.find((n) => n.id === id);
        if (!found || found.leaving) return prev;
        return prev.map((n) => (n.id === id ? { ...n, leaving: true } : n));
      });

      const tid = window.setTimeout(() => removeNow(id), NOTIFICATION_EXIT_MS);
      exitTimers.current.set(id, tid);
    },
    [removeNow],
  );

  const notify = useCallback(
    (item: NotifyInput) => {
      const id = crypto.randomUUID();
      const duration = item.duration ?? DEFAULT_DURATIONS[item.kind];
      const entry: NotificationItem = { ...item, id, duration };
      setItems((prev) => [entry, ...prev].slice(0, MAX_STACK));
      if (item.kind === "incoming") {
        playIncomingSound();
      } else if (item.kind === "error") {
        playErrorSound();
      }
      if (duration > 0) {
        const tid = window.setTimeout(() => dismiss(id), duration);
        autoTimers.current.set(id, tid);
      }
      return id;
    },
    [dismiss],
  );

  return (
    <Ctx.Provider value={{ items, notify, dismiss }}>{children}</Ctx.Provider>
  );
}

export function useNotify() {
  const ctx = useContext(Ctx);
  if (!ctx) {
    throw new Error("useNotify must be used within NotificationProvider");
  }
  return ctx;
}
