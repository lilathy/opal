import { useEffect, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { PasswordInput } from "../components/PasswordInput";
import { useNotify } from "../state/notifications";
import { useVault } from "../state/vault";

export function UnlockVaultScreen() {
  const { t } = useTranslation();
  const { unlockVault, status, errorCode, clearError } = useVault();
  const { notify } = useNotify();
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!errorCode) return;
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message: t(`errors.${errorCode}`, { defaultValue: t("errors.unknown") }),
    });
    clearError();
  }, [errorCode, notify, t, clearError]);

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    clearError();
    setBusy(true);
    try {
      await unlockVault(password);
      setPassword("");
    } catch {
      /* surfaced via errorCode */
    } finally {
      setBusy(false);
    }
  }

  const attempts = status?.failed_attempts ?? 0;

  return (
    <div className="auth-screen auth-screen--unlock">
      <form className="auth-compose" onSubmit={onSubmit}>
        <header className="auth-compose__brand">
          <h1 className="auth-headline auth-headline--solo">{t("unlock.title")}</h1>
          <p className="auth-lede">{t("unlock.subtitle")}</p>
        </header>

        <div className="auth-compose__body">
          <PasswordInput
            label={t("unlock.password")}
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
            disabled={busy}
            autoFocus
          />

          {attempts > 0 ? (
            <p className={`auth-attempts${status?.wipe_after_failures ? " is-warn" : ""}`}>
              {t("unlock.attempts", { count: attempts })}
              {status?.wipe_after_failures ? ` · ${t("unlock.wipeArmed")}` : ""}
            </p>
          ) : null}

          <button
            className="btn btn-primary btn-block auth-submit"
            type="submit"
            disabled={busy || !password}
          >
            {busy ? t("unlock.working") : t("unlock.submit")}
          </button>
        </div>
      </form>
    </div>
  );
}
