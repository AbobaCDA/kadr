//! Battery detection for eco crawling.
//!
//! All implementations are best-effort and fail soft: an unknown state is
//! treated as AC power, so indexing is never paused or slowed by a probe error.

use crate::config::CrawlMode;

/// True only in `eco` mode while the machine is running on battery.
pub fn should_pace(crawl_mode: CrawlMode) -> bool {
    // Balanced paces as well. It used to be eco-only, and eco-only meant the
    // default mode cheerfully kept a 3M-file crawl going while the machine was
    // already at 100% -- the crawl then competed with the desktop for every
    // core. Full remains "do not pace": that is the opt-in for a machine whose
    // owner says it is fine.
    match crawl_mode {
        CrawlMode::Full => false,
        CrawlMode::Balanced => heavy_load(),
        CrawlMode::Eco => on_battery() || heavy_load(),
    }
}

/// How many worker threads the crawl may use.
///
/// Leaving one core for the desktop is what keeps a machine responsive while
/// indexing; `full` is the only mode that may use every core. Never zero.
pub fn crawl_threads(crawl_mode: CrawlMode) -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    match crawl_mode {
        CrawlMode::Full => cores,
        CrawlMode::Balanced => cores.saturating_sub(1).max(1),
        CrawlMode::Eco => (cores / 2).max(1),
    }
}

/// True when free physical memory has dropped below what the crawl needs.
///
/// 512 MiB is the floor a decoded-image buffer and a Tantivy commit can still
/// work in. Unknown memory is never "low".
pub fn memory_pressure() -> bool {
    const FLOOR_BYTES: u64 = 512 * 1024 * 1024;
    match memory_free_and_total() {
        Some((free, _total)) => free < FLOOR_BYTES,
        None => false,
    }
}

#[cfg(target_os = "linux")]
pub fn on_battery() -> bool {
    use std::fs;

    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    let mut battery_seen = false;
    let mut charging = false;
    for entry in entries.flatten() {
        let dir = entry.path();
        let kind = fs::read_to_string(dir.join("type")).unwrap_or_default();
        if kind.trim() != "Battery" {
            continue;
        }
        battery_seen = true;
        let status = fs::read_to_string(dir.join("status")).unwrap_or_default();
        match status.trim() {
            "Discharging" => {}
            "Charging" | "Full" | "Not charging" => charging = true,
            _ => {}
        }
    }
    battery_seen && !charging
}

#[cfg(windows)]
pub fn on_battery() -> bool {
    #[repr(C)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }

    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }

    let mut status = SystemPowerStatus {
        ac_line_status: 255,
        battery_flag: 0,
        battery_life_percent: 0,
        system_status_flag: 0,
        battery_life_time: 0,
        battery_full_life_time: 0,
    };
    let ok = unsafe { GetSystemPowerStatus(&mut status) };
    ok != 0 && status.ac_line_status == 0
}

#[cfg(target_os = "macos")]
pub fn on_battery() -> bool {
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        /// Minutes remaining on battery, or a negative sentinel for
        /// unlimited/unknown.
        fn IOPSGetTimeRemainingEstimate() -> f64;
    }

    // -2.0 = unlimited (AC), -1.0 = unknown; battery estimates are >= 0.
    unsafe { IOPSGetTimeRemainingEstimate() >= 0.0 }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn on_battery() -> bool {
    false
}

#[cfg(target_os = "linux")]
pub fn heavy_load() -> bool {
    use std::fs;
    let Ok(s) = fs::read_to_string("/proc/loadavg") else {
        return false;
    };
    let Some(first) = s.split_whitespace().next() else {
        return false;
    };
    let Ok(load1) = first.parse::<f64>() else {
        return false;
    };
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    load1 > cpus as f64 * 0.8
}

#[cfg(target_os = "macos")]
pub fn heavy_load() -> bool {
    // `getloadavg` is a 1-minute load average read, available on every macOS
    // release, so eco mode paces the crawl on a busy Mac the same way it does
    // on Linux. Failures degrade to "not heavy" (never throttles).
    let mut averages = [0.0f64; 3];
    let reported =
        unsafe { libc::getloadavg(averages.as_mut_ptr(), averages.len() as libc::c_int) };
    if reported < 1 {
        return false;
    }
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    averages[0] > cpus as f64 * 0.8
}

#[cfg(windows)]
pub fn heavy_load() -> bool {
    // Windows has no load average, and the previous stub returned `false`,
    // which meant eco mode never backed off there at all. Two `GetSystemTimes`
    // samples 120 ms apart give the CPU utilisation Task Manager shows.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    impl FileTime {
        fn ticks(self) -> u64 {
            ((self.high as u64) << 32) | self.low as u64
        }
    }
    unsafe extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }

    fn sample() -> Option<(u64, u64, u64)> {
        let (mut idle, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default());
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return None;
        }
        Some((idle.ticks(), kernel.ticks(), user.ticks()))
    }

    let Some((idle_a, kernel_a, user_a)) = sample() else {
        return false;
    };
    std::thread::sleep(std::time::Duration::from_millis(120));
    let Some((idle_b, kernel_b, user_b)) = sample() else {
        return false;
    };

    // Kernel time includes idle time on Windows, hence the subtraction.
    let idle = idle_b.saturating_sub(idle_a) as f64;
    let busy =
        (kernel_b.saturating_sub(kernel_a) + user_b.saturating_sub(user_a)) as f64 - idle;
    let total = idle + busy;
    if total <= 0.0 {
        return false;
    }
    (busy / total) > 0.8
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn heavy_load() -> bool {
    false
}

/// Free and total physical memory in bytes, when the platform can tell.
///
/// `None` means "unknown", and every caller must treat it as "no limit" rather
/// than "no memory left".
#[cfg(windows)]
pub fn memory_free_and_total() -> Option<(u64, u64)> {
    #[repr(C)]
    #[derive(Default)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 {
        return None;
    }
    Some((status.avail_phys, status.total_phys))
}

#[cfg(target_os = "linux")]
pub fn memory_free_and_total() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut free = None;
    let mut total = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("MemFree:") => free = parts.next().and_then(|v| v.parse::<u64>().ok()),
            Some("MemTotal:") => total = parts.next().and_then(|v| v.parse::<u64>().ok()),
            _ => {}
        }
    }
    // MemFree excludes reclaimable cache; add it back so a healthy machine does
    // not look full.
    let cached = text
        .lines()
        .find(|l| l.starts_with("Cached:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    Some(((free? + cached) * 1024, total? * 1024))
}

#[cfg(target_os = "macos")]
pub fn memory_free_and_total() -> Option<(u64, u64)> {
    // `vm_stat` is not worth a syscall dance here; the caller falls back to no
    // limit, which is the safe direction.
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn memory_free_and_total() -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_never_paces() {
        // `full` is the opt-in for "use the machine": it must never throttle,
        // whatever the load says.
        assert!(!should_pace(CrawlMode::Full));
    }

    #[test]
    fn balanced_and_eco_leave_a_core_for_the_desktop() {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        assert_eq!(crawl_threads(CrawlMode::Full), cores);
        assert!(crawl_threads(CrawlMode::Balanced) < cores || cores == 1);
        assert!(crawl_threads(CrawlMode::Eco) <= cores);
        // Never zero: a crawl with no threads never finishes.
        for mode in [CrawlMode::Full, CrawlMode::Balanced, CrawlMode::Eco] {
            assert!(crawl_threads(mode) >= 1);
        }
    }

    /// Eco mode calls on_battery() and heavy_load() which do OS I/O. Whatever
    /// they return, should_pace must not panic.
    #[test]
    fn eco_does_not_panic() {
        let _ = should_pace(CrawlMode::Eco);
    }

    /// on_battery and heavy_load must always return a bool without panicking
    /// regardless of the host environment.
    #[test]
    fn os_probes_never_panic() {
        let _ = on_battery();
        let _ = heavy_load();
    }
}
