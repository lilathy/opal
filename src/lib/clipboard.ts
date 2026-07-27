let clearTimer: ReturnType<typeof setTimeout> | null = null;

/** Copy text; auto-clear clipboard after `clearAfterMs` (0 = clear ASAP). */
export async function copyWithAutoClear(
  text: string,
  clearAfterMs = 30_000,
): Promise<void> {
  if (clearTimer) {
    clearTimeout(clearTimer);
    clearTimer = null;
  }
  await navigator.clipboard.writeText(text);
  const delay = Math.max(0, clearAfterMs);
  clearTimer = setTimeout(() => {
    void navigator.clipboard.writeText("").catch(() => {
      /* ignore */
    });
    clearTimer = null;
  }, delay);
}
