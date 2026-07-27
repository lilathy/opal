import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, parseInvokeError } from "../lib/api";
import { copyWithAutoClear } from "../lib/clipboard";
import type { SeedSetupIntent } from "../lib/seedIntent";
import { useNotify } from "../state/notifications";

interface Props {
  onDone: () => Promise<void>;
  /** Path chosen on Create vault (or sidebar). */
  initialMode?: "choose" | SeedSetupIntent;
  /** When true, user already picked Fresh/Recover — no chooser, no switching. */
  lockPath?: boolean;
}

type CreateStep = "words" | "backup" | "confirm";
type RestoreStep = "phrase" | "passphrase";

function normalizeMnemonic(raw: string): string {
  return raw
    .trim()
    .toLowerCase()
    .replace(/[\n\r\t]+/g, " ")
    .replace(/\s+/g, " ");
}

export function SeedSetup({ onDone, initialMode = "choose", lockPath = false }: Props) {
  const { t } = useTranslation();
  const { notify } = useNotify();

  function showError(message: string) {
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message,
    });
  }

  const [path, setPath] = useState<"choose" | SeedSetupIntent>(
    initialMode === "create" || initialMode === "restore" ? initialMode : "choose",
  );
  const [createStep, setCreateStep] = useState<CreateStep>("words");
  const [restoreStep, setRestoreStep] = useState<RestoreStep>("phrase");
  const [words, setWords] = useState<12 | 24>(12);
  const [mnemonic, setMnemonic] = useState("");
  const [confirm, setConfirm] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  const restoreWordCount = useMemo(() => {
    return normalizeMnemonic(mnemonic).split(" ").filter(Boolean).length;
  }, [mnemonic]);

  // If a seed was already generated (backup unfinished), jump to the write-down step.
  useEffect(() => {
    if (path !== "create" || mnemonic) return;
    let cancelled = false;
    void (async () => {
      try {
        const phrase = await api.walletRevealSeed();
        if (cancelled || !phrase) return;
        setMnemonic(phrase);
        setCreateStep("backup");
      } catch {
        /* no seed yet — stay on words */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [path, mnemonic]);

  const createSteps: CreateStep[] = ["words", "backup", "confirm"];
  const restoreSteps: RestoreStep[] = ["phrase", "passphrase"];

  const progress =
    path === "choose"
      ? null
      : path === "create"
        ? {
            current: createSteps.indexOf(createStep) + 1,
            total: createSteps.length,
          }
        : {
            current: restoreSteps.indexOf(restoreStep) + 1,
            total: restoreSteps.length,
          };

  function pickPath(next: SeedSetupIntent) {
    setPath(next);
    setCreateStep("words");
    setRestoreStep("phrase");
    setMnemonic("");
    setConfirm("");
    setPassphrase("");
  }

  function backFromCreate() {
    if (createStep === "words") {
      if (!lockPath && !mnemonic) {
        setPath("choose");
      }
      return;
    }
    if (createStep === "backup") {
      setCreateStep("words");
      return;
    }
    setConfirm("");
    setCreateStep("backup");
  }

  function backFromRestore() {
    if (restoreStep === "phrase") {
      if (!lockPath) {
        setPath("choose");
        setMnemonic("");
        setPassphrase("");
      }
      return;
    }
    setRestoreStep("phrase");
  }

  async function continueFromWords() {
    if (mnemonic) {
      setCreateStep("backup");
      return;
    }
    setBusy(true);
    try {
      let phrase: string;
      try {
        phrase = await api.walletCreateSeed(words);
      } catch {
        // Already generated earlier in this session — reuse it.
        phrase = await api.walletRevealSeed();
      }
      setMnemonic(phrase);
      setConfirm("");
      setCreateStep("backup");
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function confirmBackup() {
    if (normalizeMnemonic(confirm) !== normalizeMnemonic(mnemonic)) {
      showError(t("seed.confirmMismatch"));
      return;
    }
    setBusy(true);
    try {
      await api.walletConfirmBackup();
      await onDone();
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function restore() {
    const phrase = normalizeMnemonic(mnemonic);
    const count = phrase.split(" ").filter(Boolean).length;
    if (count !== 12 && count !== 24) {
      showError(t("seed.restoreWordCount"));
      return;
    }
    setBusy(true);
    try {
      await api.walletRestoreSeed(phrase, passphrase || undefined);
      try {
        await api.walletDiscoverPortfolios();
      } catch {
        /* best-effort — defaults may still have been written */
      }
      await onDone();
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function copySeed() {
    await copyWithAutoClear(mnemonic, 0);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 2000);
  }

  const title =
    path === "choose"
      ? t("seed.stepPathTitle")
      : path === "create"
        ? createStep === "words"
          ? t("seed.stepWordsTitle")
          : createStep === "backup"
            ? t("seed.stepBackupTitle")
            : t("seed.stepConfirmTitle")
        : restoreStep === "phrase"
          ? t("seed.stepPhraseTitle")
          : t("seed.stepPassphraseTitle");

  return (
    <div className="content seed-page">
      <div className="auth-wizard seed-wizard">
        <header className="settings-hero auth-wizard__hero">
          {progress ? (
            <p className="auth-wizard__progress">
              {t("seed.stepProgress", {
                current: progress.current,
                total: progress.total,
              })}
            </p>
          ) : null}
          <h1 className="settings-hero__title">{title}</h1>
        </header>

        {path === "choose" ? (
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

        {path === "create" && createStep === "words" ? (
          <div className="seed-panel auth-wizard__panel">
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
              {!lockPath && !mnemonic ? (
                <button type="button" className="btn btn-ghost" disabled={busy} onClick={backFromCreate}>
                  {t("common.back")}
                </button>
              ) : null}
            </div>
          </div>
        ) : null}

        {path === "create" && createStep === "backup" ? (
          <div className="auth-wizard__body">
            <ol className="seed-grid" aria-label={t("seed.stepBackupTitle")}>
              {mnemonic.split(/\s+/).filter(Boolean).map((word, i) => (
                <li key={`${i}-${word}`} className="seed-grid__cell">
                  <span className="seed-grid__n">{i + 1}</span>
                  <span className="seed-grid__w">{word}</span>
                </li>
              ))}
            </ol>
            <button type="button" className="btn btn-ghost btn-block" onClick={() => void copySeed()}>
              {copied ? t("seed.copiedCleared") : t("seed.copy")}
            </button>
            <div className="row auth-wizard__actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={!mnemonic}
                onClick={() => setCreateStep("confirm")}
              >
                {t("common.continue")}
              </button>
              <button type="button" className="btn btn-ghost" onClick={backFromCreate}>
                {t("common.back")}
              </button>
            </div>
          </div>
        ) : null}

        {path === "create" && createStep === "confirm" ? (
          <div className="auth-wizard__body">
            <div className="field">
              <label htmlFor="seed-retype">{t("seed.retype")}</label>
              <textarea
                id="seed-retype"
                className="seed-entry"
                rows={3}
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                autoFocus
              />
            </div>
            <div className="row auth-wizard__actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={busy || !confirm.trim()}
                onClick={() => void confirmBackup()}
              >
                {t("seed.confirmed")}
              </button>
              <button type="button" className="btn btn-ghost" disabled={busy} onClick={backFromCreate}>
                {t("common.back")}
              </button>
            </div>
          </div>
        ) : null}

        {path === "restore" && restoreStep === "phrase" ? (
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
                onClick={() => setRestoreStep("passphrase")}
              >
                {t("common.continue")}
              </button>
              {!lockPath ? (
                <button type="button" className="btn btn-ghost" onClick={backFromRestore}>
                  {t("common.back")}
                </button>
              ) : null}
            </div>
          </div>
        ) : null}

        {path === "restore" && restoreStep === "passphrase" ? (
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
                onClick={() => void restore()}
              >
                {busy ? t("common.loading") : t("seed.restoreSubmit")}
              </button>
              <button type="button" className="btn btn-ghost" disabled={busy} onClick={backFromRestore}>
                {t("common.back")}
              </button>
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}
