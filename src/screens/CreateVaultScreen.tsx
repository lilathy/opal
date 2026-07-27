import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { PasswordInput } from "../components/PasswordInput";
import { Switch } from "../components/Switch";
import { api, parseInvokeError, type SecurityPreset } from "../lib/api";
import { copyWithAutoClear } from "../lib/clipboard";
import {
  clearSetupSession,
  readSetupSession,
  writeSetupSession,
  type SeedSetupIntent,
  type SetupStep,
} from "../lib/seedIntent";
import { useNotify } from "../state/notifications";
import { useVault } from "../state/vault";

function normalizeMnemonic(raw: string): string {
  return raw
    .trim()
    .toLowerCase()
    .replace(/[\n\r\t]+/g, " ")
    .replace(/\s+/g, " ");
}

function stepsFor(intent: SeedSetupIntent | null): SetupStep[] {
  if (intent === "restore") {
    return ["path", "password", "confirm", "security", "phrase", "passphrase"];
  }
  // Fresh seed (default once path is chosen, and while choosing)
  return ["path", "password", "confirm", "security", "words", "backup"];
}

export function CreateVaultScreen() {
  const { t } = useTranslation();
  const { createVault, errorCode, clearError, refresh } = useVault();
  const { notify } = useNotify();

  const saved = readSetupSession();
  const [step, setStep] = useState<SetupStep>(saved.step === "path" && !saved.vaultCreated ? "path" : saved.step);
  const [intent, setIntent] = useState<SeedSetupIntent | null>(saved.intent);
  const [vaultCreated, setVaultCreated] = useState(saved.vaultCreated);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [preset, setPreset] = useState<SecurityPreset>("normal");
  const [wipe, setWipe] = useState(false);
  const [words, setWords] = useState<12 | 24>(12);
  const [mnemonic, setMnemonic] = useState("");
  const [seedConfirm, setSeedConfirm] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [busy, setBusy] = useState(false);
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const [direction, setDirection] = useState<"forward" | "back">("forward");

  const displayError = localError ?? errorCode;
  const flow = stepsFor(intent);
  const stepIndex = Math.max(0, flow.indexOf(step));
  const mnemonicWords = useMemo(
    () => (mnemonic ? mnemonic.split(/\s+/).filter(Boolean) : []),
    [mnemonic],
  );
  const restoreWordCount = useMemo(
    () => normalizeMnemonic(mnemonic).split(" ").filter(Boolean).length,
    [mnemonic],
  );

  function goToStep(next: SetupStep) {
    const from = flow.indexOf(step);
    const to = flow.indexOf(next);
    if (to >= 0 && from >= 0) {
      setDirection(to >= from ? "forward" : "back");
    } else {
      setDirection("forward");
    }
    setStep(next);
  }

  useEffect(() => {
    writeSetupSession({ intent, step, vaultCreated });
  }, [intent, step, vaultCreated]);

  useEffect(() => {
    if (!displayError) return;
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message: t(`errors.${displayError}`, { defaultValue: t("errors.unknown") }),
    });
    setLocalError(null);
    clearError();
  }, [displayError, notify, t, clearError]);

  // Resume write-down if a seed was already generated this session.
  useEffect(() => {
    if (!vaultCreated || intent !== "create" || mnemonic) return;
    if (step !== "words" && step !== "backup") return;
    let cancelled = false;
    void (async () => {
      try {
        const phrase = await api.walletRevealSeed();
        if (cancelled || !phrase) return;
        setMnemonic(phrase);
        goToStep("backup");
      } catch {
        /* none yet */
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [vaultCreated, intent, mnemonic, step]);

  function showError(message: string) {
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message,
    });
  }

  function goBack() {
    clearError();
    setLocalError(null);
    if (stepIndex <= 0) return;
    // Don't go back into vault creation once the vault exists.
    const prev = flow[stepIndex - 1]!;
    if (vaultCreated && (prev === "security" || prev === "confirm" || prev === "password" || prev === "path")) {
      return;
    }
    goToStep(prev);
  }

  function pickPath(next: SeedSetupIntent) {
    setIntent(next);
    goToStep("password");
  }

  function continueFromPassword() {
    clearError();
    setLocalError(null);
    if (password.length < 8) return;
    goToStep("confirm");
  }

  function continueFromConfirm() {
    clearError();
    setLocalError(null);
    if (password !== confirm) {
      setLocalError("mismatch");
      return;
    }
    goToStep("security");
  }

  async function continueFromSecurity() {
    if (!intent) return;
    clearError();
    setLocalError(null);
    if (password !== confirm) {
      setLocalError("mismatch");
      goToStep("confirm");
      return;
    }
    setBusy(true);
    try {
      if (!vaultCreated) {
        writeSetupSession({
          intent,
          step: intent === "restore" ? "phrase" : "words",
          vaultCreated: true,
        });
        await createVault(password, preset, wipe);
        setVaultCreated(true);
        setPassword("");
        setConfirm("");
      }
      goToStep(intent === "restore" ? "phrase" : "words");
    } catch {
      /* errorCode set in provider — roll back the optimistic session flag */
      writeSetupSession({ intent, step: "security", vaultCreated: false });
      setVaultCreated(false);
    } finally {
      setBusy(false);
    }
  }

  async function continueFromWords() {
    if (mnemonic) {
      goToStep("backup");
      return;
    }
    setBusy(true);
    try {
      let phrase: string;
      try {
        phrase = await api.walletCreateSeed(words);
      } catch {
        phrase = await api.walletRevealSeed();
      }
      setMnemonic(phrase);
      setSeedConfirm("");
      goToStep("backup");
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function finishFresh() {
    if (normalizeMnemonic(seedConfirm) !== normalizeMnemonic(mnemonic)) {
      showError(t("seed.confirmMismatch"));
      return;
    }
    setBusy(true);
    try {
      await api.walletConfirmBackup();
      clearSetupSession();
      await refresh();
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function finishRestore() {
    const phrase = normalizeMnemonic(mnemonic);
    const count = phrase.split(" ").filter(Boolean).length;
    if (count !== 12 && count !== 24) {
      showError(t("seed.restoreWordCount"));
      return;
    }
    setBusy(true);
    setBusyLabel(t("common.loading"));
    try {
      await api.walletRestoreSeed(phrase, passphrase || undefined);
      setBusyLabel(t("seed.discovering"));
      try {
        const found = await api.walletDiscoverPortfolios();
        if (found.length > 0) {
          notify({
            kind: "success",
            title: t("seed.discoveredTitle"),
            message: t("seed.discoveredBody", { count: found.length }),
          });
        }
      } catch (e) {
        showError(parseInvokeError(e).message);
      }
      clearSetupSession();
      await refresh();
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
      setBusyLabel(null);
    }
  }

  async function copySeed() {
    await copyWithAutoClear(mnemonic, 0);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 2000);
  }

  const title =
    step === "path"
      ? t("create.stepPathTitle")
      : step === "password"
        ? t("create.stepPasswordTitle")
        : step === "confirm"
          ? t("create.stepConfirmTitle")
          : step === "security"
            ? t("create.stepSecurityTitle")
            : step === "words"
              ? t("create.stepWordsTitle")
              : step === "backup"
                ? t("create.stepBackupTitle")
                : step === "phrase"
                  ? t("create.stepPhraseTitle")
                  : t("create.stepPassphraseTitle");

  return (
    <div className="auth-screen auth-screen--create">
      <div className="auth-wizard">
        <div
          key={step}
          className={`auth-wizard-pane${direction === "forward" ? " auth-wizard-pane--fwd" : " auth-wizard-pane--back"}`}
        >
        <header className="settings-hero auth-wizard__hero">
          <p className="auth-wizard__progress">
            {t("create.stepProgress", { current: stepIndex + 1, total: flow.length })}
          </p>
          <h1 className="settings-hero__title">{title}</h1>
        </header>

        {step === "path" ? (
          <div className="seed-choice auth-wizard__choice">
            <button type="button" className="seed-choice__card" onClick={() => pickPath("create")}>
              <span className="seed-choice__title">{t("create.pathFresh")}</span>
              <span className="seed-choice__desc">{t("create.pathFreshDesc")}</span>
            </button>
            <button type="button" className="seed-choice__card" onClick={() => pickPath("restore")}>
              <span className="seed-choice__title">{t("create.pathRecover")}</span>
              <span className="seed-choice__desc">{t("create.pathRecoverDesc")}</span>
            </button>
          </div>
        ) : null}

        {step === "password" ? (
          <div className="auth-wizard__body">
            <PasswordInput
              label={t("create.password")}
              autoComplete="new-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
              minLength={8}
              autoFocus
            />
            <div className="row auth-wizard__actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={password.length < 8}
                onClick={continueFromPassword}
              >
                {t("common.continue")}
              </button>
              <button type="button" className="btn btn-ghost" onClick={goBack}>
                {t("common.back")}
              </button>
            </div>
          </div>
        ) : null}

        {step === "confirm" ? (
          <div className="auth-wizard__body">
            <PasswordInput
              label={t("create.confirm")}
              autoComplete="new-password"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              required
              minLength={8}
              autoFocus
            />
            <div className="row auth-wizard__actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={confirm.length < 8}
                onClick={continueFromConfirm}
              >
                {t("common.continue")}
              </button>
              <button type="button" className="btn btn-ghost" onClick={goBack}>
                {t("common.back")}
              </button>
            </div>
          </div>
        ) : null}

        {step === "security" ? (
          <div className="auth-wizard__body">
            <div className="field">
              <span className="field-label">{t("create.preset")}</span>
              <div className="segmented" role="radiogroup" aria-label={t("create.preset")}>
                {(
                  [
                    ["fast", "create.presetFast"],
                    ["normal", "create.presetNormal"],
                    ["paranoid", "create.presetParanoid"],
                  ] as const
                ).map(([value, label]) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={preset === value}
                    className={`segmented__item${preset === value ? " is-active" : ""}`}
                    disabled={busy}
                    onClick={() => setPreset(value)}
                  >
                    {t(label)}
                  </button>
                ))}
              </div>
              <p className="field-hint" style={{ marginBottom: 0 }}>
                {t("create.presetHint")}
              </p>
            </div>

            <Switch
              checked={wipe}
              onChange={setWipe}
              disabled={busy}
              label={t("create.wipe")}
              hint={t("create.wipeHint")}
            />

            <div className="row auth-wizard__actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={busy || !intent}
                onClick={() => void continueFromSecurity()}
              >
                {busy ? t("create.working") : t("common.continue")}
              </button>
              <button type="button" className="btn btn-ghost" disabled={busy} onClick={goBack}>
                {t("common.back")}
              </button>
            </div>
          </div>
        ) : null}

        {step === "words" ? (
          <div className="auth-wizard__body">
            <div className="field">
              <span className="field-label">{t("seed.wordCount")}</span>
              <div className="segmented segmented--2" role="radiogroup" aria-label={t("seed.wordCount")}>
                <button
                  type="button"
                  role="radio"
                  aria-checked={words === 12}
                  className={`segmented__item${words === 12 ? " is-active" : ""}`}
                  disabled={busy || Boolean(mnemonic)}
                  onClick={() => setWords(12)}
                >
                  {t("seed.words12")}
                </button>
                <button
                  type="button"
                  role="radio"
                  aria-checked={words === 24}
                  className={`segmented__item${words === 24 ? " is-active" : ""}`}
                  disabled={busy || Boolean(mnemonic)}
                  onClick={() => setWords(24)}
                >
                  {t("seed.words24")}
                </button>
              </div>
            </div>
            <div className="row auth-wizard__actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={busy}
                onClick={() => void continueFromWords()}
              >
                {busy ? t("common.loading") : t("common.continue")}
              </button>
            </div>
          </div>
        ) : null}

        {step === "backup" ? (
          <div className="auth-wizard__body">
            <ol className="seed-grid" aria-label={t("create.stepBackupTitle")}>
              {mnemonicWords.map((word, i) => (
                <li key={`${i}-${word}`} className="seed-grid__cell">
                  <span className="seed-grid__n">{i + 1}</span>
                  <span className="seed-grid__w">{word}</span>
                </li>
              ))}
            </ol>

            <button type="button" className="btn btn-ghost btn-block" onClick={() => void copySeed()}>
              {copied ? t("seed.copiedCleared") : t("seed.copy")}
            </button>

            <div className="field">
              <label htmlFor="seed-retype">{t("seed.retype")}</label>
              <textarea
                id="seed-retype"
                className="seed-entry"
                rows={3}
                value={seedConfirm}
                onChange={(e) => setSeedConfirm(e.target.value)}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                placeholder={t("seed.phrasePlaceholder")}
              />
            </div>

            <div className="row auth-wizard__actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={busy || !seedConfirm.trim() || !mnemonic}
                onClick={() => void finishFresh()}
              >
                {busy ? t("common.loading") : t("seed.confirmed")}
              </button>
              <button
                type="button"
                className="btn btn-ghost"
                disabled={busy}
                onClick={() => {
                  setSeedConfirm("");
                  goToStep("words");
                }}
              >
                {t("common.back")}
              </button>
            </div>
          </div>
        ) : null}

        {step === "phrase" ? (
          <div className="auth-wizard__body">
            <div className="field">
              <label htmlFor="seed-phrase">{t("seed.phrase")}</label>
              <textarea
                id="seed-phrase"
                className="seed-entry"
                rows={4}
                value={mnemonic}
                onChange={(e) => setMnemonic(e.target.value)}
                placeholder={t("seed.phrasePlaceholder")}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                autoComplete="off"
                autoFocus
              />
              <span className="field-hint">
                {t("seed.restoreWordCountHint", { count: restoreWordCount })}
              </span>
            </div>
            <div className="row auth-wizard__actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={restoreWordCount !== 12 && restoreWordCount !== 24}
                onClick={() => goToStep("passphrase")}
              >
                {t("common.continue")}
              </button>
            </div>
          </div>
        ) : null}

        {step === "passphrase" ? (
          <div className="auth-wizard__body">
            <div className="field">
              <label htmlFor="seed-passphrase">{t("seed.passphraseOptional")}</label>
              <input
                id="seed-passphrase"
                type="password"
                value={passphrase}
                onChange={(e) => setPassphrase(e.target.value)}
                autoComplete="off"
                autoFocus
              />
              <span className="field-hint">{t("seed.passphraseHint")}</span>
            </div>
            <div className="row auth-wizard__actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={busy}
                onClick={() => void finishRestore()}
              >
                  {busy ? busyLabel ?? t("common.loading") : t("seed.restoreSubmit")}
              </button>
              <button
                type="button"
                className="btn btn-ghost"
                disabled={busy}
                onClick={() => goToStep("phrase")}
              >
                {t("common.back")}
              </button>
            </div>
          </div>
        ) : null}
        </div>
      </div>
    </div>
  );
}
