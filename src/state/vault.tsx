import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { api, type VaultStatus, parseInvokeError } from "../lib/api";

interface VaultContextValue {
  status: VaultStatus | null;
  loading: boolean;
  errorCode: string | null;
  refresh: () => Promise<void>;
  createVault: (
    password: string,
    preset: VaultStatus["security_preset"],
    wipeAfter10: boolean,
  ) => Promise<void>;
  unlockVault: (password: string) => Promise<void>;
  lockVault: () => Promise<void>;
  wipeVault: (password: string) => Promise<void>;
  clearError: () => void;
  bumpActivity: () => void;
}

const VaultContext = createContext<VaultContextValue | null>(null);

export function VaultProvider({ children }: { children: ReactNode }) {
  const { i18n } = useTranslation();
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [errorCode, setErrorCode] = useState<string | null>(null);
  const lastActivity = useRef(Date.now());

  const refresh = useCallback(async () => {
    const next = await api.vaultStatus();
    setStatus(next);
    if (next.language && next.language !== i18n.language) {
      await i18n.changeLanguage(next.language);
    }
  }, [i18n]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        await refresh();
      } catch (e) {
        if (!cancelled) setErrorCode(parseInvokeError(e).code);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [refresh]);

  const createVault = useCallback(
    async (
      password: string,
      preset: VaultStatus["security_preset"],
      wipeAfter10: boolean,
    ) => {
      setErrorCode(null);
      try {
        const next = await api.vaultCreate(password, preset, wipeAfter10);
        setStatus(next);
        lastActivity.current = Date.now();
      } catch (e) {
        const err = parseInvokeError(e);
        setErrorCode(err.code);
        throw e;
      }
    },
    [],
  );

  const unlockVault = useCallback(async (password: string) => {
    setErrorCode(null);
    try {
      const next = await api.vaultUnlock(password);
      setStatus(next);
      lastActivity.current = Date.now();
      if (next.language) await i18n.changeLanguage(next.language);
    } catch (e) {
      const err = parseInvokeError(e);
      setErrorCode(err.code);
      try {
        await refresh();
      } catch {
        /* ignore */
      }
      throw e;
    }
  }, [i18n, refresh]);

  const lockVault = useCallback(async () => {
    const next = await api.vaultLock();
    setStatus(next);
  }, []);

  const wipeVault = useCallback(async (password: string) => {
    setErrorCode(null);
    try {
      const next = await api.vaultWipe(password);
      setStatus(next);
      try {
        sessionStorage.clear();
      } catch {
        /* private mode */
      }
    } catch (e) {
      const err = parseInvokeError(e);
      setErrorCode(err.code);
      throw e;
    }
  }, []);

  const bumpActivity = useCallback(() => {
    lastActivity.current = Date.now();
  }, []);

  useEffect(() => {
    if (status?.phase !== "unlocked") return;
    const minutes = status.auto_lock_minutes;
    if (!minutes || minutes <= 0) return;

    const id = window.setInterval(() => {
      const idleMs = Date.now() - lastActivity.current;
      if (idleMs >= minutes * 60_000) {
        void lockVault();
      }
    }, 5_000);

    const onActivity = () => {
      lastActivity.current = Date.now();
    };
    window.addEventListener("pointerdown", onActivity);
    window.addEventListener("keydown", onActivity);

    return () => {
      window.clearInterval(id);
      window.removeEventListener("pointerdown", onActivity);
      window.removeEventListener("keydown", onActivity);
    };
  }, [status?.phase, status?.auto_lock_minutes, lockVault]);

  const value = useMemo(
    () => ({
      status,
      loading,
      errorCode,
      refresh,
      createVault,
      unlockVault,
      lockVault,
      wipeVault,
      clearError: () => setErrorCode(null),
      bumpActivity,
    }),
    [
      status,
      loading,
      errorCode,
      refresh,
      createVault,
      unlockVault,
      lockVault,
      wipeVault,
      bumpActivity,
    ],
  );

  return <VaultContext.Provider value={value}>{children}</VaultContext.Provider>;
}

export function useVault() {
  const ctx = useContext(VaultContext);
  if (!ctx) throw new Error("useVault must be used within VaultProvider");
  return ctx;
}
