//! Performance meter: engine process memory/CPU plus WebView2 (UI) processes,
//! combined with the frame-time / heap sample the theme runtime reports.

use serde_json::{json, Value};

use crate::app::State;

#[cfg(windows)]
mod win {
    use std::mem::size_of;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::{
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2},
            Threading::{GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ},
        },
    };

    pub struct Sample {
        pub ws: u64,
        pub cpu_100ns: u64,
    }

    fn ft(f: FILETIME) -> u64 {
        (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
    }

    pub fn sample(pid: u32) -> Option<Sample> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
            if h.is_null() {
                return None;
            }
            // Private working set = Task Manager's "Memory" column (shared DLL pages excluded).
            let mut pmc: PROCESS_MEMORY_COUNTERS_EX2 = std::mem::zeroed();
            pmc.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
            let ok = GetProcessMemoryInfo(h, &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS, pmc.cb);

            let (mut c, mut e, mut k, mut u) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
            let ok2 = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u);
            CloseHandle(h);
            if ok == 0 {
                return None;
            }
            Some(Sample { ws: pmc.PrivateWorkingSetSize as u64, cpu_100ns: if ok2 != 0 { ft(k) + ft(u) } else { 0 } })
        }
    }

    /// All descendant processes of ours (WebView2 browser/renderer/gpu).
    pub fn children() -> Vec<u32> {
        let me = unsafe { GetCurrentProcessId() };
        let mut all: Vec<(u32, u32)> = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap as isize == -1 {
                return Vec::new();
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut e) != 0 {
                loop {
                    all.push((e.th32ProcessID, e.th32ParentProcessID));
                    if Process32NextW(snap, &mut e) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        let mut out = Vec::new();
        let mut frontier = vec![me];
        while let Some(p) = frontier.pop() {
            for &(pid, ppid) in &all {
                if ppid == p && pid != me && !out.contains(&pid) {
                    out.push(pid);
                    frontier.push(pid);
                }
            }
        }
        out
    }

    pub fn me() -> u32 {
        unsafe { GetCurrentProcessId() }
    }
}

static LAST: std::sync::Mutex<Option<(std::time::Instant, u64, u64)>> = std::sync::Mutex::new(None);

/// Engine + UI memory (MB) and CPU% since the previous call.
pub fn processes() -> (f64, f64, f64, f64) {
    #[cfg(windows)]
    {
        let eng = win::sample(win::me());
        let kids: Vec<win::Sample> = win::children().into_iter().filter_map(win::sample).collect();
        let eng_ws = eng.as_ref().map(|s| s.ws).unwrap_or(0) as f64 / 1048576.0;
        let ui_ws = kids.iter().map(|s| s.ws).sum::<u64>() as f64 / 1048576.0;
        let eng_cpu = eng.map(|s| s.cpu_100ns).unwrap_or(0);
        let ui_cpu: u64 = kids.iter().map(|s| s.cpu_100ns).sum();
        let now = std::time::Instant::now();
        let mut last = LAST.lock().unwrap();
        let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        let (eng_pct, ui_pct) = match *last {
            Some((t, e0, u0)) => {
                let dt = now.duration_since(t).as_secs_f64().max(0.001) * 1e7 * ncpu;
                (eng_cpu.saturating_sub(e0) as f64 / dt * 100.0, ui_cpu.saturating_sub(u0) as f64 / dt * 100.0)
            }
            None => (0.0, 0.0),
        };
        *last = Some((now, eng_cpu, ui_cpu));
        (eng_ws, ui_ws, eng_pct, ui_pct)
    }
    #[cfg(not(windows))]
    {
        (0.0, 0.0, 0.0, 0.0)
    }
}

pub fn snapshot(state: &State) -> Value {
    let (eng, ui, eng_cpu, ui_cpu) = processes();
    let p = state.perf.read().unwrap().clone();
    let r1 = |x: f64| (x * 10.0).round() / 10.0;
    json!({
        "engineMb": r1(eng),
        "memoryMb": r1(ui),
        "totalMb": r1(eng + ui),
        "cpu": r1(ui_cpu),
        "engineCpu": r1(eng_cpu),
        "frameMs": r1(p["frameMs"].as_f64().unwrap_or(0.0)),
        "frameP95": r1(p["frameP95"].as_f64().unwrap_or(0.0)),
        "heapMb": r1(p["heap"].as_f64().unwrap_or(0.0) / 1048576.0),
        "nodes": p["nodes"].as_u64().unwrap_or(0),
    })
}
