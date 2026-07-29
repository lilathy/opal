import { useEffect } from "react";
import { toPng } from "html-to-image";
import { api } from "../lib/api";

/**
 * Ctrl+Shift+S — capture the visible Opal UI via DOM → PNG (not native HWND capture).
 * Filters out toast notifications and writes Desktop/Opal-YYYYMMDD-HHMMSS.png.
 */
export function useScreenshotHotkey() {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey)) return;
      if (e.code !== "KeyS" && e.key.toLowerCase() !== "s") return;
      e.preventDefault();
      e.stopPropagation();
      void captureAndSave();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, []);
}

async function captureAndSave() {
  const root =
    (document.querySelector(".app-root") as HTMLElement | null) ?? document.body;
  if (!root) return;

  const width = Math.max(1, Math.round(window.innerWidth));
  const height = Math.max(1, Math.round(window.innerHeight));
  const pixelRatio = Math.min(3, Math.max(2, window.devicePixelRatio || 2));

  try {
    const dataUrl = await toPng(root, {
      cacheBust: true,
      pixelRatio,
      width,
      height,
      style: {
        width: `${width}px`,
        height: `${height}px`,
      },
      filter: (node) => {
        if (!(node instanceof HTMLElement)) return true;
        return !node.classList.contains("notification-host");
      },
    });
    await api.savePngBase64(dataUrl);
  } catch (err) {
    console.error("screenshot failed", err);
  }
}
