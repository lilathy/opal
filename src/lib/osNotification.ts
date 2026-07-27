import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

/** Fire a native OS notification when the app is in the background. */
export async function sendOsNotification(
  title: string,
  body: string,
): Promise<void> {
  try {
    if (!(await ensureNotificationPermission())) return;
    sendNotification({ title, body });
  } catch {
    /* Browser / non-Tauri builds */
  }
}

/** Request OS permission when the user enables notifications in settings. */
export async function ensureNotificationPermission(): Promise<boolean> {
  try {
    let granted = await isPermissionGranted();
    if (!granted) {
      const perm = await requestPermission();
      granted = perm === "granted";
    }
    return granted;
  } catch {
    return false;
  }
}
