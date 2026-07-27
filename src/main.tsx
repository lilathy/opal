import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./i18n";
import "./styles/global.css";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { startPerfDebug } from "./lib/perfDebug";
import { checkAndApplyAppUpdate } from "./lib/updater";
import { NotificationProvider } from "./state/notifications";
import { VaultProvider } from "./state/vault";

startPerfDebug();

// Auto-update in the background once the UI is mounting (production installs only).
void checkAndApplyAppUpdate();

/** Accidental double-clicks only count as a single click app-wide. */
function installClickGuard() {
  const block = (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
    e.stopImmediatePropagation();
  };

  document.addEventListener(
    "click",
    (e) => {
      if (e.detail > 1) block(e);
    },
    true,
  );
  document.addEventListener("dblclick", block, true);
}

installClickGuard();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <VaultProvider>
        <NotificationProvider>
          <App />
        </NotificationProvider>
      </VaultProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);
