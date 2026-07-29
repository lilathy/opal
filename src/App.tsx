import { useTranslation } from "react-i18next";
import { NotificationHost } from "./components/NotificationHost";
import { useScreenshotHotkey } from "./hooks/useScreenshotHotkey";
import { isSetupSessionActive } from "./lib/seedIntent";
import { CreateVaultScreen } from "./screens/CreateVaultScreen";
import { UnlockVaultScreen } from "./screens/UnlockVaultScreen";
import { ShellScreen } from "./screens/ShellScreen";
import { useVault } from "./state/vault";

export default function App() {
  const { t } = useTranslation();
  const { status, loading } = useVault();
  useScreenshotHotkey();

  if (loading || !status) {
    return (
      <div className="app-root">
        <div className="loading-screen">{t("common.loading")}</div>
        <NotificationHost />
      </div>
    );
  }

  const seedReady = status.has_seed && status.seed_backed_up;
  // Keep the create wizard mounted through vault unlock until the seed is finished.
  const showCreateWizard =
    status.phase === "needs_create" ||
    (status.phase === "unlocked" && !seedReady && isSetupSessionActive());

  return (
    <div className="app-root">
      {showCreateWizard ? <CreateVaultScreen /> : null}
      {status.phase === "locked" ? <UnlockVaultScreen /> : null}
      {status.phase === "unlocked" && !showCreateWizard ? <ShellScreen /> : null}
      <NotificationHost />
    </div>
  );
}
