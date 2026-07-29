import { open, save } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnalyticsTilesEditor } from "../components/AnalyticsTilesEditor";
import { PasswordInput } from "../components/PasswordInput";
import { ProcedureShell } from "../components/ProcedureShell";
import { Select } from "../components/Select";
import { Switch } from "../components/Switch";
import {
  api,
  parseInvokeError,
  type AppSettings,
  type SecurityPreset,
} from "../lib/api";
import { ANALYTICS_TILE_IDS, type AnalyticsTileId } from "../lib/analyticsTiles";
import { cacheFiatPriceMatrix, resolvePricesForFiat, assetFiatValue } from "../lib/balances";
import { formatMoney } from "../lib/format";
import { ensureNotificationPermission } from "../lib/osNotification";
import { playErrorSound, playSendSound, playSwapSound, playTrezorConnectedSound } from "../lib/sounds";
import {
  collectTxExportCsv,
  defaultTxExportFilename,
} from "../lib/txExport";
import { useNotify } from "../state/notifications";
import { useVault } from "../state/vault";

const FIATS = ["USD", "EUR", "GBP", "RUB", "JPY", "CNY", "KRW", "BRL", "TRY", "INR"];
const LANGS: { code: string; label: string }[] = [
  { code: "en", label: "English" },
  { code: "ru", label: "Русский" },
  { code: "zh", label: "中文" },
  { code: "es", label: "Español" },
  { code: "pt", label: "Português" },
  { code: "de", label: "Deutsch" },
  { code: "fr", label: "Français" },
  { code: "ja", label: "日本語" },
  { code: "ko", label: "한국어" },
  { code: "ar", label: "العربية" },
];

type PasswordStep = "current" | "new" | "confirm";
const PASSWORD_STEPS: PasswordStep[] = ["current", "new", "confirm"];

export function SettingsScreen() {
  const { t, i18n } = useTranslation();
  const { refresh, lockVault, wipeVault } = useVault();
  const { notify } = useNotify();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  // Which single field is currently saving — scoped per-control so flipping
  // one switch doesn't dim every other control on the page at once.
  const [busyField, setBusyField] = useState<string | null>(null);
  const [presetBusy, setPresetBusy] = useState(false);
  const [passwordBusy, setPasswordBusy] = useState(false);
  const [backupBusy, setBackupBusy] = useState(false);
  const [taxExportBusy, setTaxExportBusy] = useState(false);
  const [wipeBusy, setWipeBusy] = useState(false);
  const [wipePassword, setWipePassword] = useState("");
  const [passwordStep, setPasswordStep] = useState<PasswordStep | null>(null);
  const [passwordDir, setPasswordDir] = useState<"forward" | "back">("forward");

  const [presetPassword, setPresetPassword] = useState("");
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [vaultPassword, setVaultPassword] = useState("");

  function showError(code: string) {
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message: t(`errors.${code}`, { defaultValue: t("errors.unknown") }),
    });
  }

  function showSuccess(message: string) {
    notify({
      kind: "success",
      title: t("notifications.successTitle"),
      message,
    });
  }

  function previewIncomingNotification() {
    const fiat = settings?.fiat ?? "USD";
    const prices = resolvePricesForFiat(fiat);
    const delta = 0.25;
    const fiatRaw = assetFiatValue(
      { symbol: "BTC", amount: String(delta), decimals: 8, usd: null },
      fiat,
      prices,
    );
    notify({
      kind: "incoming",
      title: t("notifications.incomingTitle"),
      message: t("notifications.testIncomingMeta", {
        defaultValue: "Demo portfolio",
      }),
      symbol: "BTC",
      amount: "+0.25 BTC",
      fiatAmount:
        Number.isFinite(fiatRaw) && fiatRaw > 0
          ? formatMoney(fiatRaw, fiat, false)
          : formatMoney(16250, fiat, false),
    });
  }

  function previewSendSound() {
    playSendSound();
  }

  function previewSwapSound() {
    playSwapSound();
  }

  function previewTrezorSound() {
    playTrezorConnectedSound();
  }

  function previewErrorSound() {
    playErrorSound();
  }

  useEffect(() => {
    void (async () => {
      try {
        const s = await api.getSettings();
        setSettings(s);
      } catch (e) {
        showError(parseInvokeError(e).code);
      }
    })();
    void api.warmSpotPrices().then((matrix) => {
      if (matrix) cacheFiatPriceMatrix(matrix);
    });
  }, []);

  async function savePatch(patch: Record<string, unknown>, field: string) {
    setBusyField(field);
    try {
      const next = await api.updateSettings(patch);
      setSettings(next);
      if (patch.language && typeof patch.language === "string") {
        await i18n.changeLanguage(patch.language);
      }
      await refresh();
    } catch (e) {
      showError(parseInvokeError(e).code);
    } finally {
      setBusyField((cur) => (cur === field ? null : cur));
    }
  }

  async function onToggleAutostart(enabled: boolean) {
    setBusyField("startWithWindows");
    try {
      await api.setAutostart(enabled);
      const next = await api.updateSettings({ startWithWindows: enabled });
      setSettings(next);
    } catch (e) {
      showError(parseInvokeError(e).code);
    } finally {
      setBusyField((cur) => (cur === "startWithWindows" ? null : cur));
    }
  }

  async function onChangePreset(preset: SecurityPreset) {
    if (!presetPassword) {
      showError("invalid_password");
      return;
    }
    setPresetBusy(true);
    try {
      const next = await api.changeSecurityPreset(presetPassword, preset);
      setSettings(next);
      setPresetPassword("");
      showSuccess(t("settings.presetChanged"));
    } catch (e) {
      showError(parseInvokeError(e).code);
    } finally {
      setPresetBusy(false);
    }
  }

  function resetPasswordProcedure() {
    setPasswordStep(null);
    setPasswordDir("forward");
    setCurrentPassword("");
    setNewPassword("");
    setConfirmPassword("");
  }

  function startPasswordProcedure() {
    setPasswordDir("forward");
    setCurrentPassword("");
    setNewPassword("");
    setConfirmPassword("");
    setPasswordStep("current");
  }

  function passwordGoBack() {
    if (!passwordStep) return;
    const idx = PASSWORD_STEPS.indexOf(passwordStep);
    setPasswordDir("back");
    if (idx <= 0) {
      resetPasswordProcedure();
      return;
    }
    setPasswordStep(PASSWORD_STEPS[idx - 1]);
  }

  function passwordGoNext() {
    if (!passwordStep) return;
    if (passwordStep === "current") {
      if (!currentPassword) {
        showError("invalid_password");
        return;
      }
      setPasswordDir("forward");
      setPasswordStep("new");
      return;
    }
    if (passwordStep === "new") {
      if (newPassword.length < 8) {
        showError("weak_password");
        return;
      }
      setPasswordDir("forward");
      setPasswordStep("confirm");
      return;
    }
    void submitPasswordChange();
  }

  async function submitPasswordChange() {
    if (newPassword !== confirmPassword) {
      showError("mismatch");
      return;
    }
    setPasswordBusy(true);
    try {
      await api.changePassword(currentPassword, newPassword);
      resetPasswordProcedure();
      showSuccess(t("settings.passwordChanged"));
    } catch (err) {
      showError(parseInvokeError(err).code);
    } finally {
      setPasswordBusy(false);
    }
  }

  async function exportVault() {
    if (!vaultPassword) {
      showError("invalid_password");
      return;
    }
    const dest = await save({
      title: t("settings.exportVault"),
      defaultPath: "opal-vault.backup",
      filters: [{ name: "Opal vault", extensions: ["backup", "opal", "bin"] }],
    });
    if (!dest) return;
    setBackupBusy(true);
    try {
      await api.vaultExport(vaultPassword, dest);
      showSuccess(t("settings.exportDone"));
    } catch (e) {
      showError(parseInvokeError(e).code);
    } finally {
      setBackupBusy(false);
    }
  }

  async function exportTransactions() {
    const dest = await save({
      title: t("settings.taxExportButton"),
      defaultPath: defaultTxExportFilename(),
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (!dest) return;

    setTaxExportBusy(true);
    try {
      const fiat = settings?.fiat ?? "USD";
      const spotPrices = resolvePricesForFiat(fiat);
      const { csv, count } = await collectTxExportCsv({
        fiat,
        spotPrices,
        fresh: true,
      });
      if (count === 0) {
        notify({
          kind: "warning",
          title: t("notifications.successTitle"),
          message: t("settings.taxExportEmpty"),
        });
        return;
      }
      await api.writeTextFile(dest, csv);
      showSuccess(t("settings.taxExportDone", { count }));
    } catch (e) {
      showError(parseInvokeError(e).code);
    } finally {
      setTaxExportBusy(false);
    }
  }

  async function importVault() {
    if (!vaultPassword) {
      showError("invalid_password");
      return;
    }
    const src = await open({
      title: t("settings.importVault"),
      multiple: false,
      filters: [{ name: "Opal vault", extensions: ["backup", "opal", "bin"] }],
    });
    if (!src || Array.isArray(src)) return;
    setBackupBusy(true);
    try {
      await api.vaultImport(vaultPassword, src);
      showSuccess(t("settings.importDone"));
      await lockVault();
      await refresh();
    } catch (e) {
      showError(parseInvokeError(e).code);
    } finally {
      setBackupBusy(false);
    }
  }

  async function wipeEntireWallet() {
    if (!wipePassword) {
      showError("invalid_password");
      return;
    }
    const ok = window.confirm(t("settings.wipeWalletHint"));
    if (!ok) return;
    setWipeBusy(true);
    try {
      await wipeVault(wipePassword);
      setWipePassword("");
      showSuccess(t("settings.wipeWalletDone"));
    } catch (e) {
      showError(parseInvokeError(e).code);
    } finally {
      setWipeBusy(false);
    }
  }

  if (!settings) {
    return (
      <div className="content">
        <p className="field-hint">{t("common.loading")}</p>
      </div>
    );
  }

  return (
    <div className="content settings-page">
      <header className="settings-hero">
        <h2 className="settings-hero__title">{t("settings.title")}</h2>
      </header>

      <div className="settings-stack">
        <section className="settings-chapter" aria-labelledby="settings-general">
          <h3 id="settings-general" className="settings-chapter__title">
            {t("settings.sectionGeneral")}
          </h3>
          <div className="settings-group">
            <div className="settings-section">
              <h4 className="settings-section__head">
                {t("settings.groupPreferences", { defaultValue: "Preferences" })}
              </h4>
              <div className="settings-control">
                <div className="settings-control__copy">
                  <strong>{t("settings.language")}</strong>
                  <span>{t("settings.languageHint")}</span>
                </div>
                <Select
                  compact
                  value={settings.language}
                  disabled={busyField === "language"}
                  onChange={(v) => void savePatch({ language: v }, "language")}
                  options={LANGS.map((l) => ({ value: l.code, label: l.label }))}
                />
              </div>

              <div className="settings-control">
                <div className="settings-control__copy">
                  <strong>{t("settings.fiat")}</strong>
                  <span>{t("settings.fiatHint")}</span>
                </div>
                <Select
                  compact
                  value={settings.fiat}
                  disabled={busyField === "fiat"}
                  onChange={(v) => void savePatch({ fiat: v }, "fiat")}
                  options={FIATS.map((f) => ({ value: f, label: f }))}
                />
              </div>

              <div className="settings-control">
                <div className="settings-control__copy">
                  <strong>{t("settings.autolock")}</strong>
                  <span>{t("settings.autolockHint")}</span>
                </div>
                <input
                  className="control-input"
                  type="text"
                  inputMode="numeric"
                  pattern="[0-9]*"
                  defaultValue={String(settings.auto_lock_minutes)}
                  disabled={busyField === "autoLockMinutes"}
                  aria-label={t("settings.autolock")}
                  onBlur={(e) => {
                    const digits = e.target.value.replace(/[^0-9]/g, "");
                    const n = Math.min(
                      240,
                      Math.max(0, digits === "" ? 0 : Number(digits)),
                    );
                    e.target.value = String(n);
                    if (n === settings.auto_lock_minutes) return;
                    void savePatch({ autoLockMinutes: n }, "autoLockMinutes");
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      (e.target as HTMLInputElement).blur();
                    }
                  }}
                />
              </div>

              <div className="settings-control">
                <div className="settings-control__copy">
                  <strong>{t("settings.activityMin")}</strong>
                  <span>{t("settings.activityMinHint")}</span>
                </div>
                <input
                  className="control-input"
                  type="text"
                  inputMode="decimal"
                  defaultValue={String(settings.activity_min_fiat ?? 0.02)}
                  disabled={busyField === "activityMinFiat"}
                  aria-label={t("settings.activityMin")}
                  onBlur={(e) => {
                    const cleaned = e.target.value.replace(/,/g, ".").replace(/[^\d.]/g, "");
                    const n = Number(cleaned);
                    const next =
                      cleaned === "" || !Number.isFinite(n) || n < 0 ? 0 : Math.min(n, 1_000_000);
                    e.target.value = String(next);
                    if (next === (settings.activity_min_fiat ?? 0.02)) return;
                    void savePatch({ activityMinFiat: next }, "activityMinFiat");
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      (e.target as HTMLInputElement).blur();
                    }
                  }}
                />
              </div>
            </div>

            <div className="settings-section">
              <h4 className="settings-section__head">
                {t("settings.groupPrivacy", { defaultValue: "Privacy" })}
              </h4>
              <Switch
                checked={settings.discreet_mode}
                disabled={busyField === "discreetMode"}
                label={t("settings.discreet")}
                hint={t("settings.discreetHint")}
                onChange={(v) => void savePatch({ discreetMode: v }, "discreetMode")}
              />

              <Switch
                checked={settings.notifications_enabled}
                disabled={busyField === "notificationsEnabled"}
                label={t("settings.notifications")}
                hint={t("settings.notificationsHint")}
                onChange={(v) => {
                  if (v) void ensureNotificationPermission();
                  void savePatch({ notificationsEnabled: v }, "notificationsEnabled");
                }}
              />

              <div className="settings-inline-field settings-sound-previews">
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={previewIncomingNotification}
                >
                  {t("notifications.testIncoming")}
                </button>
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={previewSendSound}
                >
                  {t("notifications.testSend", {
                    defaultValue: "Preview send sound",
                  })}
                </button>
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={previewSwapSound}
                >
                  {t("notifications.testSwap", {
                    defaultValue: "Preview swap sound",
                  })}
                </button>
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={previewTrezorSound}
                >
                  {t("notifications.testTrezor", {
                    defaultValue: "Preview Trezor sound",
                  })}
                </button>
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={previewErrorSound}
                >
                  {t("notifications.testError", {
                    defaultValue: "Preview error sound",
                  })}
                </button>
              </div>
            </div>

            <div className="settings-section">
              <h4 className="settings-section__head">
                {t("settings.groupAnalytics", { defaultValue: "Analytics" })}
              </h4>
              <AnalyticsTilesEditor
                enabled={settings.analytics_enabled !== false}
                order={settings.analytics_tile_order ?? []}
                hidden={settings.analytics_hidden_tiles ?? []}
                busy={
                  busyField === "analyticsEnabled" ||
                  busyField === "analyticsTiles" ||
                  busyField === "analyticsReset"
                }
                onEnabledChange={(v) =>
                  void savePatch({ analyticsEnabled: v }, "analyticsEnabled")
                }
                onLayoutChange={(
                  nextOrder: AnalyticsTileId[],
                  nextHidden: AnalyticsTileId[],
                ) => {
                  void savePatch(
                    {
                      analyticsTileOrder: nextOrder,
                      analyticsHiddenTiles: nextHidden,
                    },
                    "analyticsTiles",
                  );
                }}
                onReset={() => {
                  void savePatch(
                    {
                      analyticsTileOrder: [...ANALYTICS_TILE_IDS],
                      analyticsHiddenTiles: [],
                    },
                    "analyticsReset",
                  );
                }}
              />
            </div>

            <div className="settings-section">
              <h4 className="settings-section__head">
                {t("settings.groupStartup", { defaultValue: "Startup" })}
              </h4>
              <Switch
                checked={settings.start_with_windows}
                disabled={busyField === "startWithWindows"}
                label={t("settings.startWithWindows")}
                hint={t("settings.startWithWindowsHint")}
                onChange={(v) => void onToggleAutostart(v)}
              />
            </div>
          </div>
        </section>

        <section className="settings-chapter" aria-labelledby="settings-security">
          <h3 id="settings-security" className="settings-chapter__title">
            {t("settings.sectionSecurity")}
          </h3>
          <div className="settings-group">
            <div className="settings-section">
              <h4 className="settings-section__head">
                {t("settings.groupRecovery", { defaultValue: "Recovery" })}
              </h4>
              <Switch
                checked={settings.wipe_after_10_failures}
                disabled={busyField === "wipeAfter10Failures"}
                label={t("settings.wipe")}
                hint={t("settings.wipeHint")}
                onChange={(v) =>
                  void savePatch({ wipeAfter10Failures: v }, "wipeAfter10Failures")
                }
              />

              <Switch
                checked={settings.bip39_passphrase_enabled}
                disabled={busyField === "bip39PassphraseEnabled"}
                label={t("settings.bip39")}
                hint={t("settings.bip39Hint")}
                onChange={(v) =>
                  void savePatch({ bip39PassphraseEnabled: v }, "bip39PassphraseEnabled")
                }
              />

              {settings.bip39_passphrase_enabled ? (
                <div className="settings-inline-field">
                  <PasswordInput
                    label={t("settings.bip39Value")}
                    defaultValue={settings.bip39_passphrase ?? ""}
                    disabled={busyField === "bip39Passphrase"}
                    onBlur={(e) =>
                      void savePatch(
                        {
                          bip39Passphrase: e.target.value.trim()
                            ? e.target.value
                            : null,
                        },
                        "bip39Passphrase",
                      )
                    }
                  />
                  <p className="field-hint">{t("settings.bip39ValueHint")}</p>
                </div>
              ) : null}
            </div>

            <div className="settings-section">
              <h4 className="settings-section__head">{t("settings.preset")}</h4>
              <p className="settings-section__hint">{t("settings.presetHint")}</p>
              <PasswordInput
                label={t("settings.currentPassword")}
                value={presetPassword}
                onChange={(e) => setPresetPassword(e.target.value)}
                disabled={presetBusy}
                autoComplete="current-password"
              />
              <div className="segmented" role="radiogroup" aria-label={t("settings.preset")}>
                {(
                  [
                    ["fast", "create.presetFast"],
                    ["normal", "create.presetNormal"],
                    ["paranoid", "create.presetParanoid"],
                  ] as const
                ).map(([p, label]) => (
                  <button
                    key={p}
                    type="button"
                    role="radio"
                    aria-checked={settings.security_preset === p}
                    className={`segmented__item${settings.security_preset === p ? " is-active" : ""}`}
                    disabled={presetBusy}
                    onClick={() => void onChangePreset(p)}
                  >
                    {t(label)}
                  </button>
                ))}
              </div>
            </div>

            <div className="settings-section">
              <h4 className="settings-section__head">{t("settings.passwordSection")}</h4>
              <p className="settings-section__hint">{t("settings.passwordSectionHint")}</p>
              {passwordStep == null ? (
                <button
                  type="button"
                  className="btn btn-primary"
                  onClick={startPasswordProcedure}
                >
                  {t("settings.passwordStart")}
                </button>
              ) : (
                <ProcedureShell
                  ariaLabel={t("settings.passwordSection")}
                  direction={passwordDir}
                  activeId={passwordStep}
                  steps={[
                    {
                      id: "current",
                      label: t("settings.passwordStepCurrent"),
                    },
                    { id: "new", label: t("settings.passwordStepNew") },
                    {
                      id: "confirm",
                      label: t("settings.passwordStepConfirm"),
                    },
                  ]}
                >
                  {passwordStep === "current" ? (
                    <>
                      <p className="section-desc" style={{ marginTop: 0 }}>
                        {t("settings.passwordCurrentHint")}
                      </p>
                      <PasswordInput
                        label={t("settings.currentPassword")}
                        value={currentPassword}
                        onChange={(e) => setCurrentPassword(e.target.value)}
                        required
                        disabled={passwordBusy}
                        autoComplete="current-password"
                        autoFocus
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            passwordGoNext();
                          }
                        }}
                      />
                    </>
                  ) : null}
                  {passwordStep === "new" ? (
                    <>
                      <p className="section-desc" style={{ marginTop: 0 }}>
                        {t("settings.passwordNewHint")}
                      </p>
                      <PasswordInput
                        label={t("settings.newPassword")}
                        value={newPassword}
                        onChange={(e) => setNewPassword(e.target.value)}
                        required
                        minLength={8}
                        disabled={passwordBusy}
                        autoComplete="new-password"
                        autoFocus
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            passwordGoNext();
                          }
                        }}
                      />
                    </>
                  ) : null}
                  {passwordStep === "confirm" ? (
                    <>
                      <p className="section-desc" style={{ marginTop: 0 }}>
                        {t("settings.passwordConfirmHint")}
                      </p>
                      <PasswordInput
                        label={t("settings.confirmPassword")}
                        value={confirmPassword}
                        onChange={(e) => setConfirmPassword(e.target.value)}
                        required
                        minLength={8}
                        disabled={passwordBusy}
                        autoComplete="new-password"
                        autoFocus
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            passwordGoNext();
                          }
                        }}
                      />
                    </>
                  ) : null}
                  <div className="row" style={{ marginTop: 12 }}>
                    <button
                      type="button"
                      className="btn btn-primary"
                      disabled={passwordBusy}
                      onClick={() => passwordGoNext()}
                    >
                      {passwordStep === "confirm"
                        ? t("settings.changePassword")
                        : t("common.continue")}
                    </button>
                    <button
                      type="button"
                      className="btn btn-ghost"
                      disabled={passwordBusy}
                      onClick={passwordGoBack}
                    >
                      {passwordStep === "current"
                        ? t("settings.cancelProcedure")
                        : t("common.back")}
                    </button>
                  </div>
                </ProcedureShell>
              )}
            </div>

            <div className="settings-section">
              <h4 className="settings-section__head">{t("settings.tor")}</h4>
              <p className="settings-section__hint">{t("settings.torHint")}</p>
              <input
                className="control-input"
                type="text"
                placeholder="127.0.0.1:9050"
                defaultValue={settings.tor_socks ?? ""}
                disabled={busyField === "torSocks"}
                onBlur={(e) =>
                  void savePatch(
                    { torSocks: e.target.value.trim() ? e.target.value.trim() : null },
                    "torSocks",
                  )
                }
              />
            </div>

            <div className="settings-section">
              <h4 className="settings-section__head">{t("settings.fixedfloat")}</h4>
              <p className="settings-section__hint">{t("settings.fixedfloatHint")}</p>
              <label className="field">
                <span>{t("settings.fixedfloatKey")}</span>
                <input
                  className="control-input"
                  type="password"
                  autoComplete="off"
                  defaultValue={settings.fixedfloat_api_key ?? ""}
                  disabled={busyField === "ffKey"}
                  onBlur={(e) =>
                    void savePatch(
                      {
                        fixedfloatApiKey: e.target.value.trim()
                          ? e.target.value.trim()
                          : null,
                      },
                      "ffKey",
                    )
                  }
                />
              </label>
              <label className="field" style={{ marginTop: 10 }}>
                <span>{t("settings.fixedfloatSecret")}</span>
                <input
                  className="control-input"
                  type="password"
                  autoComplete="off"
                  defaultValue={settings.fixedfloat_api_secret ?? ""}
                  disabled={busyField === "ffSecret"}
                  onBlur={(e) =>
                    void savePatch(
                      {
                        fixedfloatApiSecret: e.target.value.trim()
                          ? e.target.value.trim()
                          : null,
                      },
                      "ffSecret",
                    )
                  }
                />
              </label>
            </div>
          </div>
        </section>

        <section className="settings-chapter" aria-labelledby="settings-backup">
          <h3 id="settings-backup" className="settings-chapter__title">
            {t("settings.sectionBackup")}
          </h3>
          <div className="settings-group">
            <div className="settings-section">
              <h4 className="settings-section__head">{t("settings.vaultBackup")}</h4>
              <p className="settings-section__hint">{t("settings.vaultBackupHint")}</p>
              <PasswordInput
                label={t("settings.currentPassword")}
                value={vaultPassword}
                onChange={(e) => setVaultPassword(e.target.value)}
                disabled={backupBusy}
                autoComplete="current-password"
              />
              <div className="row">
                <button
                  type="button"
                  className="btn"
                  disabled={backupBusy}
                  onClick={() => void exportVault()}
                >
                  {t("settings.exportVault")}
                </button>
                <button
                  type="button"
                  className="btn"
                  disabled={backupBusy}
                  onClick={() => void importVault()}
                >
                  {t("settings.importVault")}
                </button>
              </div>
            </div>

            <div className="settings-section">
              <h4 className="settings-section__head">{t("settings.taxExport")}</h4>
              <p className="settings-section__hint">{t("settings.taxExportHint")}</p>
              <div className="settings-tax-export">
                <button
                  type="button"
                  className="btn"
                  disabled={taxExportBusy}
                  onClick={() => void exportTransactions()}
                >
                  {taxExportBusy
                    ? t("settings.taxExportBusy")
                    : t("settings.taxExportButton")}
                </button>
                <p className="settings-tax-export__meta">
                  {t("settings.taxExportMeta", {
                    defaultValue: "CSV · all portfolios · recent explorer history",
                  })}
                </p>
              </div>
            </div>

            <div className="settings-section">
              <h4 className="settings-section__head">{t("settings.wipeWallet")}</h4>
              <p className="settings-section__hint">{t("settings.wipeWalletHint")}</p>
              <PasswordInput
                label={t("settings.wipeWalletPassword")}
                value={wipePassword}
                onChange={(e) => setWipePassword(e.target.value)}
                disabled={wipeBusy}
                autoComplete="current-password"
              />
              <div className="row" style={{ marginTop: 12 }}>
                <button
                  type="button"
                  className="btn btn-danger"
                  disabled={wipeBusy || !wipePassword}
                  onClick={() => void wipeEntireWallet()}
                >
                  {wipeBusy ? t("settings.wipeWalletBusy") : t("settings.wipeWalletConfirm")}
                </button>
              </div>
            </div>
          </div>
        </section>
      </div>
    </div>
  );
}
