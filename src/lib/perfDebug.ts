import { invoke } from "@tauri-apps/api/core";

export interface PerfSnapshot {
  logPath: string;
  pid: number;
  priorityClass: number;
  priorityClassName: string;
  threadPriority: number;
  ecoQosDisabled: boolean;
  webviewChildren: Array<{
    pid: number;
    parentPid: number;
    priorityClass: number;
    priorityClassName: string;
  }>;
  watchdogRunning: boolean;
}

export interface PerfBenchResult {
  samplesMs: number[];
  minMs: number;
  maxMs: number;
  avgMs: number;
  p95Ms: number;
  snapshot: PerfSnapshot;
  logPath: string;
}

const STALL_MS = 50;
const IPC_SLOW_MS = 80;
const SUMMARY_EVERY_MS = 10_000;

let started = false;
let frames = 0;
let stalls = 0;
let maxFrame = 0;
let lastTs = 0;
let lastSummary = 0;
let lastLogAt = 0;

async function rustLog(message: string) {
  try {
    await invoke("perf_debug_log", { message });
  } catch {
    // Backend may not be ready during first paint.
    console.debug("[opal-perf]", message);
  }
}

function onFrame(ts: number) {
  if (lastTs > 0) {
    const dt = ts - lastTs;
    frames += 1;
    if (dt > maxFrame) maxFrame = dt;
    if (dt >= STALL_MS) {
      stalls += 1;
      // Rate-limit individual stall lines so a hard freeze doesn't spam disk.
      if (ts - lastLogAt > 250) {
        lastLogAt = ts;
        void rustLog(
          `frame_stall dtMs=${dt.toFixed(1)} visibility=${document.visibilityState}`,
        );
      }
    }
  }
  lastTs = ts;

  if (ts - lastSummary >= SUMMARY_EVERY_MS) {
    void rustLog(
      `frame_summary frames=${frames} stalls=${stalls} maxMs=${maxFrame.toFixed(1)} visibility=${document.visibilityState}`,
    );
    frames = 0;
    stalls = 0;
    maxFrame = 0;
    lastSummary = ts;
  }

  requestAnimationFrame(onFrame);
}

/** Wrap invoke so slow IPC shows up in debug-perf.log without changing call sites. */
export async function timedInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const t0 = performance.now();
  try {
    const result = await invoke<T>(cmd, args);
    const ms = performance.now() - t0;
    if (ms >= IPC_SLOW_MS) {
      void rustLog(`ipc_slow cmd=${cmd} ms=${ms.toFixed(1)}`);
    }
    return result;
  } catch (e) {
    const ms = performance.now() - t0;
    void rustLog(`ipc_error cmd=${cmd} ms=${ms.toFixed(1)} err=${String(e)}`);
    throw e;
  }
}

export function startPerfDebug() {
  if (started) return;
  started = true;
  lastSummary = performance.now();
  void rustLog("ui_perf_monitor_start");
  void invoke<PerfSnapshot>("perf_debug_snapshot")
    .then((snap) => {
      void rustLog(
        `snapshot prio=${snap.priorityClassName} ecoOff=${snap.ecoQosDisabled} webviews=${snap.webviewChildren.length} watchdog=${snap.watchdogRunning} log=${snap.logPath}`,
      );
      for (const w of snap.webviewChildren) {
        void rustLog(
          `webview_child pid=${w.pid} parent=${w.parentPid} prio=${w.priorityClassName}`,
        );
      }
    })
    .catch((e) => {
      void rustLog(`snapshot_failed ${String(e)}`);
    });

  // Run a short bench once so the log has baseline numbers on every launch.
  void invoke<PerfBenchResult>("perf_run_bench", { iters: 30 })
    .then((r) => {
      void rustLog(
        `bench_boot min=${r.minMs.toFixed(3)} avg=${r.avgMs.toFixed(3)} p95=${r.p95Ms.toFixed(3)} max=${r.maxMs.toFixed(3)} prio=${r.snapshot.priorityClassName}`,
      );
    })
    .catch((e) => {
      void rustLog(`bench_boot_failed ${String(e)}`);
    });

  requestAnimationFrame(onFrame);

  window.addEventListener("freeze", () => {
    void rustLog("page_lifecycle freeze");
  });
  window.addEventListener("resume", () => {
    void rustLog("page_lifecycle resume");
  });
  document.addEventListener("visibilitychange", () => {
    void rustLog(`visibility ${document.visibilityState}`);
  });
}
