//! Keep Opal responsive when the machine is flooded with low-priority Chromes
//! (rain farmers, etc.). Best-effort Windows scheduler + anti-EcoQoS hints.
//!
//! Also owns `%AppData%/Opal/debug-perf.log` for freeze / jank diagnostics.

use parking_lot::Mutex;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static WATCHDOG_STARTED: AtomicBool = AtomicBool::new(false);
static LOG_LOCK: Mutex<()> = Mutex::new(());

/// Raise this process (and WebView2 children) so Windows prefers Opal over
/// Idle / Below-Normal browser farms. Starts a watchdog that re-applies
/// priority — farmer tools often demote every `msedgewebview2.exe`.
pub fn boost_responsiveness() {
    log_line("perf", "boost_responsiveness() enter");
    #[cfg(windows)]
    windows::boost();
    #[cfg(not(windows))]
    log_line("perf", "boost skipped (non-Windows)");
}

pub fn log_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Opal")
        .join("debug-perf.log")
}

pub fn log_line(source: &str, message: &str) {
    let _guard = LOG_LOCK.lock();
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Soft rotate ~5MB so a long session doesn't fill the disk.
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 5_000_000 {
            let bak = path.with_extension("log.prev");
            let _ = std::fs::rename(&path, bak);
        }
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = writeln!(file, "{ms}\t{source}\t{message}");
    let _ = file.flush();
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfSnapshot {
    pub log_path: String,
    pub pid: u32,
    pub priority_class: u32,
    pub priority_class_name: String,
    pub thread_priority: i32,
    pub eco_qos_disabled: bool,
    pub webview_children: Vec<WebviewChildSnap>,
    pub watchdog_running: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebviewChildSnap {
    pub pid: u32,
    pub parent_pid: u32,
    pub priority_class: u32,
    pub priority_class_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfBenchResult {
    pub samples_ms: Vec<f64>,
    pub min_ms: f64,
    pub max_ms: f64,
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub snapshot: PerfSnapshot,
    pub log_path: String,
}

pub fn snapshot() -> PerfSnapshot {
    #[cfg(windows)]
    {
        return windows::snapshot();
    }
    #[cfg(not(windows))]
    {
        PerfSnapshot {
            log_path: log_path().display().to_string(),
            pid: std::process::id(),
            priority_class: 0,
            priority_class_name: "n/a".into(),
            thread_priority: 0,
            eco_qos_disabled: false,
            webview_children: vec![],
            watchdog_running: WATCHDOG_STARTED.load(Ordering::Relaxed),
        }
    }
}

/// Cheap IPC round-trip probe used by the frontend bench.
pub fn ping_now_ms() -> f64 {
    let start = std::time::Instant::now();
    // Touch something real so this isn't a pure no-op the optimizer eats.
    let _ = snapshot().pid;
    start.elapsed().as_secs_f64() * 1000.0
}

pub fn run_bench(iters: u32) -> PerfBenchResult {
    let n = iters.clamp(5, 200);
    log_line("bench", &format!("run_bench start iters={n}"));
    #[cfg(windows)]
    windows::apply_all();

    let mut samples = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let t0 = std::time::Instant::now();
        let _ = std::process::id();
        // Simulate a tiny critical-section style touch (log lock).
        {
            let _g = LOG_LOCK.lock();
        }
        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let snap = snapshot();
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let min_ms = *sorted.first().unwrap_or(&0.0);
    let max_ms = *sorted.last().unwrap_or(&0.0);
    let avg_ms = if samples.is_empty() {
        0.0
    } else {
        samples.iter().sum::<f64>() / samples.len() as f64
    };
    let p95_idx = ((sorted.len() as f64) * 0.95).floor() as usize;
    let p95_ms = sorted
        .get(p95_idx.min(sorted.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0);
    let result = PerfBenchResult {
        samples_ms: samples,
        min_ms,
        max_ms,
        avg_ms,
        p95_ms,
        log_path: log_path().display().to_string(),
        snapshot: snap,
    };
    log_line(
        "bench",
        &format!(
            "done min={:.3} avg={:.3} p95={:.3} max={:.3} prio={}",
            result.min_ms, result.avg_ms, result.p95_ms, result.max_ms, result.snapshot.priority_class_name
        ),
    );
    result
}

/// Frontend / command entry: append a line.
pub fn append_frontend_log(message: String) {
    log_line("ui", &message);
}

#[cfg(windows)]
mod windows {
    use super::{log_line, log_path, PerfSnapshot, WebviewChildSnap, WATCHDOG_STARTED};
    use std::collections::HashSet;
    use std::mem::{size_of, zeroed};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    type Handle = *mut std::ffi::c_void;

    const HIGH_PRIORITY_CLASS: u32 = 0x0000_0080;
    const ABOVE_NORMAL_PRIORITY_CLASS: u32 = 0x0000_8000;
    const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
    const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
    const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;
    const REALTIME_PRIORITY_CLASS: u32 = 0x0000_0100;

    const THREAD_PRIORITY_HIGHEST: i32 = 2;
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_SET_INFORMATION: u32 = 0x0200;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;

    // PROCESS_INFORMATION_CLASS
    const PROCESS_POWER_THROTTLING: u32 = 4;
    const PROCESS_POWER_THROTTLING_CURRENT_VERSION: u32 = 1;
    const PROCESS_POWER_THROTTLING_EXECUTION_SPEED: u32 = 0x1;
    const PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION: u32 = 0x4;

    // THREAD_INFORMATION_CLASS (processthreadsapi.h)
    const THREAD_POWER_THROTTLING: u32 = 3;
    const THREAD_POWER_THROTTLING_CURRENT_VERSION: u32 = 1;
    const THREAD_POWER_THROTTLING_EXECUTION_SPEED: u32 = 0x1;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[repr(C)]
    struct ProcessPowerThrottlingState {
        version: u32,
        control_mask: u32,
        state_mask: u32,
    }

    #[repr(C)]
    struct ThreadPowerThrottlingState {
        version: u32,
        control_mask: u32,
        state_mask: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn GetCurrentProcessId() -> u32;
        fn GetCurrentThread() -> Handle;
        fn SetPriorityClass(process: Handle, class: u32) -> i32;
        fn GetPriorityClass(process: Handle) -> u32;
        fn SetThreadPriority(thread: Handle, priority: i32) -> i32;
        fn GetThreadPriority(thread: Handle) -> i32;
        fn SetProcessInformation(
            process: Handle,
            info_class: u32,
            info: *const std::ffi::c_void,
            size: u32,
        ) -> i32;
        fn SetThreadInformation(
            thread: Handle,
            info_class: u32,
            info: *const std::ffi::c_void,
            size: u32,
        ) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
    }

    #[link(name = "avrt")]
    extern "system" {
        fn AvSetMmThreadCharacteristicsW(task_name: *const u16, task_index: *mut u32) -> Handle;
    }

    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
    }

    fn priority_name(class: u32) -> String {
        match class {
            HIGH_PRIORITY_CLASS => "HIGH".into(),
            ABOVE_NORMAL_PRIORITY_CLASS => "ABOVE_NORMAL".into(),
            NORMAL_PRIORITY_CLASS => "NORMAL".into(),
            BELOW_NORMAL_PRIORITY_CLASS => "BELOW_NORMAL".into(),
            IDLE_PRIORITY_CLASS => "IDLE".into(),
            REALTIME_PRIORITY_CLASS => "REALTIME".into(),
            0 => "UNKNOWN/0".into(),
            other => format!("0x{other:X}"),
        }
    }

    fn exe_name(entry: &ProcessEntry32W) -> String {
        let len = entry
            .sz_exe_file
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.sz_exe_file.len());
        String::from_utf16_lossy(&entry.sz_exe_file[..len]).to_ascii_lowercase()
    }

    fn disable_eco_qos(process: Handle) -> bool {
        let state = ProcessPowerThrottlingState {
            version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            control_mask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED
                | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
            // StateMask 0 = do NOT enable throttling for controlled bits.
            state_mask: 0,
        };
        unsafe {
            SetProcessInformation(
                process,
                PROCESS_POWER_THROTTLING,
                &state as *const _ as *const std::ffi::c_void,
                size_of::<ProcessPowerThrottlingState>() as u32,
            ) != 0
        }
    }

    fn disable_thread_eco_qos(thread: Handle) -> bool {
        let state = ThreadPowerThrottlingState {
            version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
            control_mask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            state_mask: 0,
        };
        unsafe {
            SetThreadInformation(
                thread,
                THREAD_POWER_THROTTLING,
                &state as *const _ as *const std::ffi::c_void,
                size_of::<ThreadPowerThrottlingState>() as u32,
            ) != 0
        }
    }

    static MMCSS_CLAIMED: AtomicBool = AtomicBool::new(false);

    fn claim_mmcss() {
        if MMCSS_CLAIMED.swap(true, Ordering::SeqCst) {
            return;
        }
        // "Games" tells the MMCSS scheduler to favor this thread under load.
        let name: Vec<u16> = "Games\0".encode_utf16().collect();
        let mut task_index: u32 = 0;
        unsafe {
            let h = AvSetMmThreadCharacteristicsW(name.as_ptr(), &mut task_index);
            if h.is_null() {
                log_line("perf", "AvSetMmThreadCharacteristicsW failed");
                MMCSS_CLAIMED.store(false, Ordering::SeqCst);
            } else {
                log_line("perf", &format!("MMCSS Games ok taskIndex={task_index}"));
            }
        }
    }

    fn set_pid_high(pid: u32) {
        unsafe {
            let h = OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_QUERY_INFORMATION,
                0,
                pid,
            );
            if h.is_null() {
                return;
            }
            let _ = SetPriorityClass(h, HIGH_PRIORITY_CLASS);
            let _ = disable_eco_qos(h);
            let _ = CloseHandle(h);
        }
    }

    fn collect_webview_tree(opal_pid: u32) -> Vec<(u32, u32)> {
        // (pid, parent_pid) for all descendant msedgewebview2 processes under Opal.
        let mut procs: Vec<(u32, u32, String)> = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap == (-1isize as Handle) {
                return Vec::new();
            }
            let mut entry: ProcessEntry32W = zeroed();
            entry.dw_size = size_of::<ProcessEntry32W>() as u32;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    procs.push((
                        entry.th32_process_id,
                        entry.th32_parent_process_id,
                        exe_name(&entry),
                    ));
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }

        let mut out = Vec::new();
        let mut frontier = vec![opal_pid];
        let mut seen = HashSet::new();
        seen.insert(opal_pid);
        while let Some(parent) = frontier.pop() {
            for (pid, ppid, name) in &procs {
                if *ppid == parent && name == "msedgewebview2.exe" && seen.insert(*pid) {
                    out.push((*pid, *ppid));
                    frontier.push(*pid);
                }
            }
        }
        out
    }

    fn boost_webview_children(opal_pid: u32) -> usize {
        let kids = collect_webview_tree(opal_pid);
        for (pid, _) in &kids {
            if *pid != opal_pid {
                set_pid_high(*pid);
            }
        }
        kids.len()
    }

    fn read_priority(pid: u32) -> u32 {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_QUERY_INFORMATION, 0, pid);
            if h.is_null() {
                return 0;
            }
            let c = GetPriorityClass(h);
            let _ = CloseHandle(h);
            c
        }
    }

    pub fn apply_all() {
        apply_self(true);
        let pid = unsafe { GetCurrentProcessId() };
        let _ = time_begin_period_once();
        let n = boost_webview_children(pid);
        log_line("perf", &format!("boosted webview tree nodes={n}"));
    }

    fn time_begin_period_once() {
        static DONE: AtomicBool = AtomicBool::new(false);
        if DONE.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            let _ = timeBeginPeriod(1);
        }
    }

    fn apply_self(verbose: bool) {
        unsafe {
            let proc = GetCurrentProcess();
            let ok_prio = SetPriorityClass(proc, HIGH_PRIORITY_CLASS) != 0;
            let ok_thr = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) != 0;
            let ok_eco = disable_eco_qos(proc);
            let ok_teco = disable_thread_eco_qos(GetCurrentThread());
            let class = GetPriorityClass(proc);
            if verbose {
                log_line(
                    "perf",
                    &format!(
                        "apply_self setPrio={ok_prio} setThr={ok_thr} ecoOff={ok_eco} thrEcoOff={ok_teco} class={} ({})",
                        class,
                        priority_name(class)
                    ),
                );
            } else if class != HIGH_PRIORITY_CLASS {
                log_line(
                    "perf",
                    &format!(
                        "priority_demoted_repaired class={} ({}) setPrio={ok_prio}",
                        class,
                        priority_name(class)
                    ),
                );
            }
        }
        claim_mmcss();
    }

    fn reboost_cached_pids(pids: &[u32]) {
        for pid in pids {
            let class = read_priority(*pid);
            if class != HIGH_PRIORITY_CLASS {
                set_pid_high(*pid);
                log_line(
                    "perf",
                    &format!(
                        "child_demoted_repaired pid={pid} was={}",
                        priority_name(class)
                    ),
                );
            }
        }
    }

    pub fn boost() {
        time_begin_period_once();
        apply_self(true);
        let opal_pid = unsafe { GetCurrentProcessId() };
        let _ = boost_webview_children(opal_pid);
        let mut cached_pids: Vec<u32> = collect_webview_tree(opal_pid)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        cached_pids.push(opal_pid);
        log_line(
            "perf",
            &format!("boosted webview tree nodes={}", cached_pids.len().saturating_sub(1)),
        );

        if WATCHDOG_STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        thread::spawn(move || {
            log_line(
                "perf",
                "watchdog started (15s light check, 60s tree refresh)",
            );
            // Early passes catch late-spawned WebView2 without scanning every 2s.
            for delay_ms in [800_u64, 2500, 8000] {
                thread::sleep(Duration::from_millis(delay_ms));
                apply_self(false);
                let _ = boost_webview_children(opal_pid);
                cached_pids = collect_webview_tree(opal_pid)
                    .into_iter()
                    .map(|(p, _)| p)
                    .collect();
                cached_pids.push(opal_pid);
            }
            let mut tick: u64 = 0;
            loop {
                thread::sleep(Duration::from_secs(15));
                tick += 1;
                apply_self(false);
                // Cheap path: only touch known PIDs (no full process table walk).
                reboost_cached_pids(&cached_pids);
                if tick % 4 == 0 {
                    // Refresh tree occasionally — Toolhelp is expensive on Chrome farms.
                    let n = boost_webview_children(opal_pid);
                    cached_pids = collect_webview_tree(opal_pid)
                        .into_iter()
                        .map(|(p, _)| p)
                        .collect();
                    cached_pids.push(opal_pid);
                    let snap = snapshot();
                    log_line(
                        "watchdog",
                        &format!(
                            "heartbeat prio={} webviews={} ecoOff={} refreshed={}",
                            snap.priority_class_name,
                            snap.webview_children.len(),
                            snap.eco_qos_disabled,
                            n
                        ),
                    );
                }
            }
        });
    }

    pub fn snapshot() -> PerfSnapshot {
        let pid = unsafe { GetCurrentProcessId() };
        let priority_class = unsafe { GetPriorityClass(GetCurrentProcess()) };
        let thread_priority = unsafe { GetThreadPriority(GetCurrentThread()) };
        // Re-check eco flag by attempting disable again (API has no reliable getter).
        let eco_qos_disabled = disable_eco_qos(unsafe { GetCurrentProcess() });
        let kids = collect_webview_tree(pid);
        let webview_children = kids
            .into_iter()
            .filter(|(p, _)| *p != pid)
            .map(|(p, parent)| {
                let c = read_priority(p);
                WebviewChildSnap {
                    pid: p,
                    parent_pid: parent,
                    priority_class: c,
                    priority_class_name: priority_name(c),
                }
            })
            .collect();
        PerfSnapshot {
            log_path: log_path().display().to_string(),
            pid,
            priority_class,
            priority_class_name: priority_name(priority_class),
            thread_priority,
            eco_qos_disabled,
            webview_children,
            watchdog_running: WATCHDOG_STARTED.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_line_writes_file() {
        log_line("test", "unit-test-write");
        let path = log_path();
        let contents = std::fs::read_to_string(&path).expect("log should exist");
        assert!(contents.contains("unit-test-write"));
    }

    #[test]
    fn bench_runs() {
        let r = run_bench(10);
        assert_eq!(r.samples_ms.len(), 10);
        assert!(r.max_ms >= r.min_ms);
    }
}
