import { useState } from "react";
import { useTranslation } from "react-i18next";
import { TrezorSpinner } from "./TrezorSpinner";
import { api, parseInvokeError, type PortfolioRecord, type TrezorStatus } from "../lib/api";

type Props = {
  trezorStatus: TrezorStatus | null;
  onDone: (created: PortfolioRecord[]) => Promise<void>;
  onCancel: () => void;
};

/**
 * First-connect / manual Sync my Trezor - scans funded accounts on-device
 * and creates Trezor portfolios (no Suite paste).
 */
export function SyncTrezorPanel({ trezorStatus, onDone, onCancel }: Props) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [found, setFound] = useState<PortfolioRecord[] | null>(null);

  async function runSync() {
    setBusy(true);
    setError(null);
    setFound(null);
    try {
      const rows = await api.trezorDiscoverPortfolios(false);
      setFound(rows);
      if (rows.length) {
        await onDone(rows);
      }
    } catch (e) {
      setError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="content settings-page trezor-sync">
      <header className="settings-hero">
        <div className="trezor-sync__hero-icon">
          <TrezorSpinner internalModel={trezorStatus?.device_internal_model} size={56} />
        </div>
        <h2 className="settings-hero__title">
          {t("trezor.syncTitle", { defaultValue: "Sync my Trezor" })}
        </h2>
        <p className="settings-hero__lede">
          {t("trezor.syncBody", {
            defaultValue:
              "Opal will ask your unlocked Trezor for addresses and add any wallets that already have balance or activity. Confirm prompts on the device if they appear.",
          })}
        </p>
      </header>

      <div className="settings-group">
        <div className="settings-block" style={{ borderBottom: "none" }}>
          <div className="settings-control__copy" style={{ marginBottom: 12 }}>
            <strong>
              {trezorStatus?.device_count
                ? t("trezor.deviceReady", {
                    model: trezorStatus.device_model || "Trezor",
                    defaultValue: "{{model}} connected",
                  })
                : t("trezor.deviceMissing", {
                    defaultValue: "Plug in and unlock your Trezor",
                  })}
            </strong>
            <span>
              {t("trezor.syncHint", {
                defaultValue:
                  "Supports Bitcoin, Ethereum & L2s, Litecoin, Dogecoin, Solana, Tron, and Monero (Model T / Safe). Gram is not available on Trezor.",
              })}
            </span>
          </div>

          {error ? (
            <p className="form-error" role="alert">
              {error}
            </p>
          ) : null}

          {found && !busy ? (
            <p className="settings-control__copy" style={{ marginBottom: 12 }}>
              <strong>
                {found.length
                  ? t("trezor.syncFound", {
                      count: found.length,
                      defaultValue: "Added {{count}} wallet(s)",
                    })
                  : t("trezor.syncNone", {
                      defaultValue: "No funded wallets found on this device",
                    })}
              </strong>
            </p>
          ) : null}

          <div className="wizard-actions" style={{ marginTop: 8 }}>
            <button type="button" className="btn btn-ghost" onClick={onCancel} disabled={busy}>
              {found?.length
                ? t("common.done", { defaultValue: "Done" })
                : t("common.cancel", { defaultValue: "Cancel" })}
            </button>
            <button
              type="button"
              className="btn btn-primary"
              disabled={busy || !trezorStatus?.device_count}
              onClick={() => void runSync()}
            >
              {busy
                ? t("trezor.syncing", { defaultValue: "Scanning…" })
                : t("trezor.syncAction", { defaultValue: "Start sync" })}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
