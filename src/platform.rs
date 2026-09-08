#![allow(unsafe_code)]

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcessMemoryCounters {
    cb: u32,
    _page_fault_count: u32,
    peak_working_set_size: usize,
    _working_set_size: usize,
    _quota_peak_paged_pool: usize,
    _quota_paged_pool: usize,
    _quota_peak_nonpaged_pool: usize,
    _quota_nonpaged_pool: usize,
    _pagefile_usage: usize,
    _peak_pagefile_usage: usize,
}
#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> isize;
}
#[cfg(target_os = "windows")]
#[link(name = "psapi")]
unsafe extern "system" {
    fn GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, cb: u32) -> i32;
}
#[cfg(target_os = "windows")]
pub(crate) fn peak_rss_gb() -> f64 {
    let mut c = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        _page_fault_count: 0,
        peak_working_set_size: 0,
        _working_set_size: 0,
        _quota_peak_paged_pool: 0,
        _quota_paged_pool: 0,
        _quota_peak_nonpaged_pool: 0,
        _quota_nonpaged_pool: 0,
        _pagefile_usage: 0,
        _peak_pagefile_usage: 0,
    };

    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c,
            std::mem::size_of::<ProcessMemoryCounters>() as u32,
        );
    }
    c.peak_working_set_size as f64 / 1073741824.0
}

#[cfg(target_os = "linux")]
pub(crate) fn peak_rss_gb() -> f64 {
    let status = match std::fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(_) => return 0.0,
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: f64 = rest
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .unwrap_or(0.0);
            return kb / 1048576.0;
        }
    }
    0.0
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub(crate) fn peak_rss_gb() -> f64 {
    0.0
}

#[cfg(target_os = "windows")]
pub(crate) fn total_free_ram() -> (u64, u64) {
    #[repr(C)]
    #[derive(Default)]
    struct MemStatEx {
        len: u32,
        _load: u32,
        total: u64,
        free: u64,
        _tp: u64,
        _fp: u64,
        _tv: u64,
        _fv: u64,
        _aev: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(p: *mut MemStatEx) -> i32;
    }
    let mut m = MemStatEx {
        len: std::mem::size_of::<MemStatEx>() as u32,
        ..Default::default()
    };

    unsafe {
        GlobalMemoryStatusEx(&mut m);
    }
    (m.total, m.free)
}

#[cfg(target_os = "linux")]
pub(crate) fn total_free_ram() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let (mut total, mut free) = (0u64, 0u64);
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("MemTotal:") {
            total = v.trim().trim_end_matches("kB").trim().parse().unwrap_or(0) * 1024;
        } else if let Some(v) = line.strip_prefix("MemAvailable:") {
            free = v.trim().trim_end_matches("kB").trim().parse().unwrap_or(0) * 1024;
        }
    }
    (total, free)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub(crate) fn total_free_ram() -> (u64, u64) {
    (0, 0)
}

#[cfg(target_os = "windows")]
pub(crate) fn free_space_bytes(path: &str) -> Option<u64> {
    let root: String = if path.len() >= 2 && path.as_bytes()[1] == b':' {
        format!("{}\\", &path[..2])
    } else {
        "C:\\".to_string()
    };
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            dir: *const u16,
            free_avail: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }
    let (mut avail, mut total, mut tfree) = (0u64, 0u64, 0u64);

    unsafe {
        if GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut tfree) == 0 {
            return None;
        }
    }
    Some(avail)
}

#[cfg(target_os = "linux")]
pub(crate) fn free_space_bytes(path: &str) -> Option<u64> {
    #[repr(C)]
    #[derive(Default)]
    struct Statvfs {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: u64,
        f_flag: u64,
        f_namemax: u64,
        _spare: [i32; 6],
    }
    #[link(name = "c")]
    unsafe extern "C" {
        fn statvfs(path: *const i8, buf: *mut Statvfs) -> i32;
    }
    let c = std::ffi::CString::new(path).ok()?;
    let mut st: Statvfs = Default::default();

    unsafe {
        if statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
    }
    Some(st.f_bavail * st.f_frsize)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub(crate) fn free_space_bytes(_path: &str) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    #[test]
    fn free_space_handles_unicode_drive_relative_path() {
        let path = "C:\u{e9}.ulp";
        assert!(super::free_space_bytes(path).is_some(), "{path}");
    }
}
