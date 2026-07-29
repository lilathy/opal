import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChainIcon } from "../components/CryptoIcons";
import { NumberStepper } from "../components/NumberStepper";
import { Select } from "../components/Select";
import { Switch } from "../components/Switch";
import {
  api,
  canTrezorSend,
  parseInvokeError,
  trezorAutoVerifySupported,
  trezorSupportedChains,
  type BtcAddressType,
  type PortfolioKind,
} from "../lib/api";
import { nextChainPortfolioName } from "../lib/format";
import { useNotify } from "../state/notifications";

const CHAINS: { id: string; name: string }[] = [
  { id: "btc", name: "Bitcoin" },
  { id: "eth", name: "Ethereum" },
  { id: "polygon", name: "Polygon" },
  { id: "bsc", name: "BNB Smart Chain" },
  { id: "trx", name: "Tron" },
  { id: "sol", name: "Solana" },
  { id: "ton", name: "Gram" },
  { id: "ltc", name: "Litecoin" },
  { id: "doge", name: "Dogecoin" },
  { id: "xmr", name: "Monero" },
];

type Section = "network" | "custody" | "details";
const SECTION_ORDER: Section[] = ["network", "custody", "details"];

interface Props {
  existing: { chain: string; name: string }[];
  onDone: (createdId?: string) => Promise<void>;
  onCancel: () => void;
}

export function AddPortfolio({ existing, onDone, onCancel }: Props) {
  const { t } = useTranslation();
  const { notify } = useNotify();

  function showError(message: string) {
    notify({
      kind: "error",
      title: t("notifications.errorTitle"),
      message,
    });
  }
  const [section, setSection] = useState<Section>("network");
  // Which way the step pane should slide in from - set right before changing
  // `section` so the remount (keyed on section) picks up the right animation.
  const [direction, setDirection] = useState<"forward" | "back">("forward");
  const [name, setName] = useState("");
  const [chain, setChain] = useState("btc");
  const [kind, setKind] = useState<PortfolioKind>("software");
  const [address, setAddress] = useState("");
  const [xmrViewKey, setXmrViewKey] = useState("");
  const [accountIndex, setAccountIndex] = useState(0);
  const [addressType, setAddressType] = useState<BtcAddressType>("native_segwit");
  const [verifyOnDevice, setVerifyOnDevice] = useState(true);
  const [busy, setBusy] = useState(false);
  const [verifiedAddr, setVerifiedAddr] = useState<string | null>(null);

  const sections: { id: Section; label: string }[] = [
    { id: "network", label: t("portfolio.sectionNetwork") },
    { id: "custody", label: t("portfolio.sectionCustody") },
    { id: "details", label: t("portfolio.sectionDetails") },
  ];

  const kindHint =
    kind === "software"
      ? t("portfolio.kindSoftwareHint")
      : kind === "trezor"
        ? t("portfolio.kindTrezorHint")
        : t("portfolio.kindWatchHint");

  const chainChoices =
    kind === "trezor"
      ? CHAINS.filter((c) => trezorSupportedChains().includes(c.id))
      : CHAINS;

  const trezorChainAutoVerify = trezorAutoVerifySupported(chain);
  const trezorChainCanSend = canTrezorSend(chain);

  useEffect(() => {
    if (kind === "trezor" && !trezorSupportedChains().includes(chain)) {
      setChain("btc");
    }
  }, [kind, chain]);

  async function verifyTrezor() {
    setBusy(true);
    try {
      const addr = await api.trezorVerifyAddress(
        chain,
        accountIndex,
        chain === "btc" ? addressType : undefined,
      );
      setVerifiedAddr(addr);
      setAddress(addr);
    } catch (e) {
      showError(parseInvokeError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function submit() {
    setBusy(true);
    try {
      const created = await api.portfolioCreate({
        name: name.trim() || nextChainPortfolioName(chain, existing),
        chain,
        kind,
        accountIndex,
        address: address || null,
        xmrViewKey: xmrViewKey || null,
        trezorLabel: kind === "trezor" ? "Trezor" : null,
        addressType: chain === "btc" ? addressType : null,
        verifyOnDevice: kind === "trezor" ? verifyOnDevice : undefined,
      });
      await onDone(created.id);
    } catch (e) {
      showError(parseInvokeError(e).message);
      setSection("details");
    } finally {
      setBusy(false);
    }
  }

  function goNext() {
    setDirection("forward");
    if (section === "network") setSection("custody");
    else if (section === "custody") setSection("details");
  }

  function goBack() {
    setDirection("back");
    if (section === "details") setSection("custody");
    else if (section === "custody") setSection("network");
  }

  return (
    <div className="content settings-page">
      <header className="settings-hero">
        <h2 className="settings-hero__title">{t("shell.addPortfolio")}</h2>
        <p className="settings-hero__lede">{t("portfolio.addHint")}</p>
      </header>

      <div
        className="wizard-steps"
        role="list"
        aria-label={t("shell.addPortfolio")}
      >
        {sections.map((s) => {
          const stepIndex = SECTION_ORDER.indexOf(s.id);
          const currentIndex = SECTION_ORDER.indexOf(section);
          const done = stepIndex < currentIndex;
          const active = s.id === section;
          return (
            <div
              key={s.id}
              role="listitem"
              className={`wizard-step${active ? " is-active" : ""}${done ? " is-done" : ""}`}
            >
              <span className="wizard-step__bar" aria-hidden />
              <span className="wizard-step__label">{s.label}</span>
            </div>
          );
        })}
      </div>

      <div className="wizard-layout">
        <div
          className={`wizard-pane${direction === "forward" ? " wizard-pane--fwd" : " wizard-pane--back"}`}
          key={section}
        >
          {section === "network" ? (
            <div className="settings-group">
              <div className="settings-block" style={{ borderBottom: "none" }}>
                <div className="settings-control__copy" style={{ marginBottom: 14 }}>
                  <strong>{t("portfolio.chain")}</strong>
                  <span>{t("portfolio.chainHint")}</span>
                </div>
                <div className="chain-pick-list" role="listbox" aria-label={t("portfolio.chain")}>
                  {chainChoices.map((c) => (
                    <button
                      key={c.id}
                      type="button"
                      role="option"
                      aria-selected={chain === c.id}
                      className={`chain-pick${chain === c.id ? " is-selected" : ""}`}
                      onClick={() => setChain(c.id)}
                    >
                      <span className="crypto-badge">
                        <ChainIcon chain={c.id} size={28} />
                      </span>
                      <span className="chain-pick__name">{c.name}</span>
                    </button>
                  ))}
                </div>
              </div>
            </div>
          ) : null}

          {section === "custody" ? (
            <div className="settings-group">
              <div className="settings-block" style={{ borderBottom: "none" }}>
                <div className="settings-control__copy" style={{ marginBottom: 14 }}>
                  <strong>{t("portfolio.kind")}</strong>
                  <span>{kindHint}</span>
                </div>
                <div className="choice-grid" role="radiogroup" aria-label={t("portfolio.kind")}>
                  {(
                    [
                      ["software", "portfolio.kindSoftware", "portfolio.kindSoftwareHint"],
                      ["trezor", "portfolio.kindTrezor", "portfolio.kindTrezorHint"],
                      ["watch_only", "portfolio.kindWatch", "portfolio.kindWatchHint"],
                    ] as const
                  ).map(([value, title, hint]) => (
                    <button
                      key={value}
                      type="button"
                      role="radio"
                      aria-checked={kind === value}
                      className={`choice-card${kind === value ? " is-selected" : ""}`}
                      onClick={() => setKind(value)}
                    >
                      <span className="choice-card__title">{t(title)}</span>
                      <span className="choice-card__hint">{t(hint)}</span>
                    </button>
                  ))}
                </div>
              </div>
            </div>
          ) : null}

          {section === "details" ? (
            <div className="settings-group">
              <div className="settings-block">
                <div className="settings-control__copy" style={{ marginBottom: 12 }}>
                  <strong>{t("portfolio.name")}</strong>
                  <span>
                    {t("portfolio.nameFor", {
                      chain: nextChainPortfolioName(chain, existing),
                    })}
                  </span>
                </div>
                <input
                  className="control-input"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={nextChainPortfolioName(chain, existing)}
                  disabled={busy}
                />
              </div>

              <div className="settings-control">
                <div className="settings-control__copy">
                  <strong>{t("portfolio.accountIndex")}</strong>
                  <span>{t("portfolio.accountIndexHint")}</span>
                </div>
                <NumberStepper
                  value={accountIndex}
                  min={0}
                  max={100}
                  disabled={busy}
                  aria-label={t("portfolio.accountIndex")}
                  onChange={setAccountIndex}
                />
              </div>

              {chain === "btc" && kind !== "watch_only" ? (
                <div className="settings-block">
                  <div className="settings-control__copy" style={{ marginBottom: 12 }}>
                    <strong>{t("portfolio.addressType")}</strong>
                    <span>{t("portfolio.addressTypeHint")}</span>
                  </div>
                  <Select
                    value={addressType}
                    disabled={busy}
                    onChange={(v) => setAddressType(v as BtcAddressType)}
                    options={[
                      { value: "native_segwit", label: t("portfolio.addrNative") },
                      { value: "taproot", label: t("portfolio.addrTaproot") },
                      { value: "legacy", label: t("portfolio.addrLegacy") },
                    ]}
                  />
                </div>
              ) : null}

              {kind === "trezor" && trezorChainAutoVerify ? (
                <div className="settings-block">
                  <Switch
                    checked={verifyOnDevice}
                    onChange={setVerifyOnDevice}
                    disabled={busy}
                    label={t("portfolio.verifyOnDevice")}
                    hint={t("portfolio.verifyOnDeviceHint")}
                  />
                  <div className="row" style={{ marginTop: 12 }}>
                    <button
                      type="button"
                      className="btn"
                      disabled={busy}
                      onClick={() => void verifyTrezor()}
                    >
                      {busy
                        ? t("portfolio.trezorConfirmHint", {
                            defaultValue: "Confirm on your Trezor…",
                          })
                        : t("portfolio.trezorVerify")}
                    </button>
                  </div>
                  {verifiedAddr ? (
                    <p className="field-hint mono" style={{ marginTop: 10, marginBottom: 0 }}>
                      {verifiedAddr}
                    </p>
                  ) : null}
                  {!trezorChainCanSend ? (
                    <p className="field-hint" style={{ marginTop: 10, marginBottom: 0 }}>
                      {t("portfolio.trezorNoSendHint", {
                        defaultValue:
                          "Sending isn't wired up for this network yet - you'll be able to receive and track balances, but use Trezor Suite to send.",
                      })}
                    </p>
                  ) : null}
                </div>
              ) : null}

              {kind === "trezor" && !trezorChainAutoVerify ? (
                <div className="settings-block" style={{ borderBottom: "none" }}>
                  <p className="field-hint" style={{ margin: 0 }}>
                    {t("portfolio.trezorNoAutoVerifyHint", {
                      defaultValue:
                        "Gram is not supported on Trezor. Pick Bitcoin, Ethereum, Solana, Tron, or Monero instead.",
                    })}
                  </p>
                </div>
              ) : null}

              {kind === "watch_only" && (
                <div className="settings-block">
                  <div className="settings-control__copy" style={{ marginBottom: 12 }}>
                    <strong>{t("portfolio.address")}</strong>
                    <span>{t("portfolio.watchHint")}</span>
                  </div>
                  <input
                    className="control-input"
                    value={address}
                    onChange={(e) => setAddress(e.target.value)}
                    disabled={busy}
                  />
                </div>
              )}

              {kind === "watch_only" && chain === "xmr" ? (
                <div className="settings-block">
                  <div className="settings-control__copy" style={{ marginBottom: 12 }}>
                    <strong>{t("portfolio.xmrViewKey")}</strong>
                    <span>{t("portfolio.xmrViewKeyHint")}</span>
                  </div>
                  <input
                    className="control-input"
                    value={xmrViewKey}
                    onChange={(e) => setXmrViewKey(e.target.value)}
                    disabled={busy}
                  />
                </div>
              ) : null}
            </div>
          ) : null}
        </div>

        {/* Fixed footer - deliberately outside the sliding pane above so the
            Back/Continue/Save buttons never move as steps change. */}
        <div className="form-actions">
          <button type="button" className="btn btn-ghost" onClick={onCancel} disabled={busy}>
            {t("common.cancel")}
          </button>
          <div className="row">
            <button
              type="button"
              className="btn"
              disabled={busy || section === "network"}
              onClick={goBack}
            >
              {t("common.back")}
            </button>
            {section !== "details" ? (
              <button type="button" className="btn btn-primary" onClick={goNext}>
                {t("common.continue")}
              </button>
            ) : (
              <button
                type="button"
                className="btn btn-primary"
                disabled={busy}
                onClick={() => void submit()}
              >
                {busy
                  ? kind === "trezor"
                    ? t("portfolio.trezorConnectingHint", {
                        defaultValue: "Talking to Trezor…",
                      })
                    : t("common.loading")
                  : t("common.save")}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
