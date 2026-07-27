import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/**
 * Silently check GitHub Releases and install + relaunch when a newer
 * signed build is available. Failures are ignored (offline, first run, etc.).
 */
export async function checkAndApplyAppUpdate(): Promise<boolean> {
  try {
    const update = await check();
    if (!update) return false;
    await update.downloadAndInstall();
    await relaunch();
    return true;
  } catch {
    return false;
  }
}
