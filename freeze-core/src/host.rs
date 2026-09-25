//! Host (controller) side. Owns the desired freeze state in `TABLE`, mirrors it
//! into the shared control block on every change, enumerates and injects
//! target processes, and toggles per-program outbound firewall rules.
//!
//! The host is the SOLE writer of the control block; injected DLLs only read.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::common::*;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};

// Raw kernel32 imports for absolute-freeze setup. Not hooked on this side.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SystemTimeToFileTime(lpsystemtime: *const SYSTEMTIME, lpfiletime: *mut FILETIME) -> i32;
    fn FileTimeToSystemTime(lpfiletime: *const FILETIME, lpsystemtime: *mut SYSTEMTIME) -> i32;
    fn GetTickCount() -> u32;
    fn GetTickCount64() -> u64;
    fn QueryPerformanceCounter(lpperformancecount: *mut i64) -> i32;
    // NULL tz info => current zone, using the DST rule that applied ON THE GIVEN
    // DATE (not today's). This is the correct local→UTC conversion.
    fn TzSpecificLocalTimeToSystemTime(
        lptimezoneinformation: *const core::ffi::c_void,
        lplocaltime: *const SYSTEMTIME,
        lpuniversaltime: *mut SYSTEMTIME,
    ) -> i32;
}

#[inline]
fn ft_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

struct Shmem(*mut ControlBlock);
// SAFETY: the pointer addresses a page-file mapping; the seqlock in `common`
// serialises host writes against DLL reads, so sharing it across threads is
// sound. Only the publishing functions dereference it, all on the host.
unsafe impl Send for Shmem {}
unsafe impl Sync for Shmem {}

static SHMEM: OnceLock<Shmem> = OnceLock::new();
static TABLE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static GLOBAL_ACTIVE: AtomicBool = AtomicBool::new(true);

/// Create (or open) the shared control block and initialise its header. Idempotent.
/// Must run before any freeze/unfreeze has an effect on targets.
pub fn ensure_shmem() -> bool {
    if SHMEM.get().is_some() {
        publish_current();
        return true;
    }
    use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE,
    };
    use windows::core::PCWSTR;

    let size = std::mem::size_of::<ControlBlock>();
    unsafe {
        let h = match CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            None,
            PAGE_READWRITE,
            0,
            size as u32,
            PCWSTR(SHMEM_NAME.as_ptr()),
        ) {
            Ok(h) => h,
            Err(_) => return false,
        };
        let view = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, size);
        if view.Value.is_null() {
            return false;
        }
        let cb = view.Value as *mut ControlBlock;
        (*cb).magic = MAGIC;
        (*cb).version = VERSION;
        (*cb).active.store(1, Ordering::Relaxed);
        (*cb).seq.store(0, Ordering::Relaxed);
        (*cb).count.store(0, Ordering::Relaxed);
        let _ = SHMEM.set(Shmem(cb));
    }
    publish_current();
    true
}

fn publish_rows(rows: &[Entry]) {
    if let Some(Shmem(cb)) = SHMEM.get() {
        unsafe { seq_write(*cb, GLOBAL_ACTIVE.load(Ordering::Relaxed), rows) };
    }
}
fn publish_current() {
    let t = TABLE.lock().unwrap();
    publish_rows(&t);
}

/// Freeze `pid` by shifting its clock by `delta_100ns` (100ns units). No tz
/// math needed — OFFSET mode adds the delta to whatever the live time is.
pub fn freeze_offset(pid: u32, delta_100ns: i64) {
    let e = Entry {
        pid,
        mode: MODE_OFFSET,
        offset_100ns: delta_100ns,
        ..Default::default()
    };
    let mut t = TABLE.lock().unwrap();
    t.retain(|x| x.pid != pid);
    t.push(e);
    publish_rows(&t);
}

pub fn unfreeze_pid(pid: u32) {
    let mut t = TABLE.lock().unwrap();
    t.retain(|x| x.pid != pid);
    publish_rows(&t);
}

/// Global kill switch: when off, every installed hook passes time through.
pub fn set_global_active(on: bool) {
    GLOBAL_ACTIVE.store(on, Ordering::Relaxed);
    publish_current();
}

pub fn is_global_active() -> bool {
    GLOBAL_ACTIVE.load(Ordering::Relaxed)
}

pub fn snapshot() -> Vec<Entry> {
    TABLE.lock().unwrap().clone()
}

// ---- process enumeration ------------------------------------------------
pub fn list_processes() -> Vec<(u32, String)> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snap = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(h) => h,
        Err(_) => return vec![],
    };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

    let mut out = Vec::new();
    if unsafe { Process32FirstW(snap, &mut entry) }.is_ok() {
        loop {
            let name: String = entry
                .szExeFile
                .iter()
                .take_while(|&&c| c != 0)
                .map(|&c| char::from_u32(c as u32).unwrap_or('?'))
                .collect();
            out.push((entry.th32ProcessID, name));
            if unsafe { Process32NextW(snap, &mut entry) }.is_err() {
                break;
            }
        }
    }
    unsafe {
        let _ = CloseHandle(snap);
    }
    out
}

/// Full image path of a process (needed for the netsh program= match).
pub fn process_image_path(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 520];
    let mut size = buf.len() as u32;
    let res = unsafe {
        QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size)
    };
    unsafe {
        let _ = CloseHandle(h);
    }
    res.ok()?;
    Some(
        buf[..size as usize]
            .iter()
            .map(|&c| char::from_u32(c as u32).unwrap_or('\0'))
            .collect(),
    )
}

// ---- DLL injection (LoadLibraryW remote thread) -------------------------
pub fn inject_dll(pid: u32, dll_path: &str) -> bool {
    use windows::core::PCSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Memory::{
        VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
    };
    use windows::Win32::System::Threading::{
        CreateRemoteThread, OpenProcess, WaitForSingleObject, PROCESS_ALL_ACCESS,
    };

    let mut wide: Vec<u16> = dll_path.encode_utf16().collect();
    wide.push(0);
    let byte_len = wide.len() * 2;

    let h_proc = match unsafe { OpenProcess(PROCESS_ALL_ACCESS, false, pid) } {
        Ok(h) => h,
        Err(_) => return false,
    };

    let remote =
        unsafe { VirtualAllocEx(h_proc, None, byte_len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    if remote.is_null() {
        unsafe {
            let _ = CloseHandle(h_proc);
        }
        return false;
    }

    if unsafe { WriteProcessMemory(h_proc, remote, wide.as_ptr() as *const _, byte_len, None) }
        .is_err()
    {
        unsafe {
            let _ = VirtualFreeEx(h_proc, remote, 0, MEM_RELEASE);
            let _ = CloseHandle(h_proc);
        }
        return false;
    }

    let k32 = match unsafe { GetModuleHandleW(None) } {
        Ok(m) => m,
        Err(_) => {
            unsafe {
                let _ = VirtualFreeEx(h_proc, remote, 0, MEM_RELEASE);
                let _ = CloseHandle(h_proc);
            }
            return false;
        }
    };
    let loadlib =
        unsafe { GetProcAddress(k32, PCSTR(b"LoadLibraryW\0".as_ptr())) };
    let Some(loadlib) = loadlib else {
        unsafe {
            let _ = VirtualFreeEx(h_proc, remote, 0, MEM_RELEASE);
            let _ = CloseHandle(h_proc);
        }
        return false;
    };

    let thread_fn: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 =
        unsafe { std::mem::transmute(loadlib) };

    let h_thread = match unsafe {
        CreateRemoteThread(h_proc, None, 0, Some(thread_fn), Some(remote), 0, None)
    } {
        Ok(h) => h,
        Err(_) => {
            unsafe {
                let _ = VirtualFreeEx(h_proc, remote, 0, MEM_RELEASE);
                let _ = CloseHandle(h_proc);
            }
            return false;
        }
    };

    unsafe {
        let _ = WaitForSingleObject(h_thread, 5_000);
        let _ = CloseHandle(h_thread);
        let _ = CloseHandle(h_proc);
    }
    true
}

// ---- per-program network block (Windows Firewall) -----------------------
/// Block ALL outbound traffic for a program by full image path. Requires the
/// host to run elevated. Reversible via `unblock_network` (matches rule name).
fn rule_name(exe_path: &str) -> String {
    // Deterministic, unique-ish, valid firewall rule name.
    let base = exe_path.rsplit('\\').next().unwrap_or(exe_path);
    format!("freezeXapp::{base}")
}

fn run_netsh(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    match std::process::Command::new("netsh")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
    {
        Ok(st) => st.success(),
        Err(_) => false,
    }
}

pub fn block_network(exe_path: &str) -> bool {
    let name = rule_name(exe_path);
    run_netsh(&[
        "advfirewall", "firewall", "add", "rule",
        "name", &name,
        "dir", "out",
        "action", "block",
        "program", &format!("\"{exe_path}\""),
        "enable", "yes",
        "profile", "any",
    ])
}

pub fn unblock_network(exe_path: &str) -> bool {
    let name = rule_name(exe_path);
    run_netsh(&["advfirewall", "firewall", "delete", "rule", "name", &name])
}

#[cfg(feature = "host")]
pub fn current_pid() -> u32 {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    unsafe { GetCurrentProcessId() }
}

/// Hard-freeze `pid` at a local wall-clock moment. Computes the UTC filetime
/// from the zone bias, and captures the live tick/QPC counters so those stop
/// advancing too (a true freeze, not just a wall-clock shift).
pub fn freeze_absolute(
    pid: u32,
    year: u16,
    month: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
) -> bool {
    unsafe {
        let local = SYSTEMTIME {
            wYear: year,
            wMonth: month,
            wDayOfWeek: 0,
            wDay: day,
            wHour: hour,
            wMinute: minute,
            wSecond: second,
            wMilliseconds: millis,
        };
        // Validate + fill wDayOfWeek via a round-trip.
        let mut lf = FILETIME::default();
        if SystemTimeToFileTime(&local, &mut lf) == 0 {
            return false; // invalid date
        }
        let mut rt = SYSTEMTIME::default();
        FileTimeToSystemTime(&lf, &mut rt);

        // local -> UTC, honouring the DST rule for THAT date.
        let mut utc_st = SYSTEMTIME::default();
        if TzSpecificLocalTimeToSystemTime(std::ptr::null(), &local, &mut utc_st) == 0 {
            return false;
        }
        let mut uf = FILETIME::default();
        SystemTimeToFileTime(&utc_st, &mut uf);
        let utc_ft = ft_u64(uf);
        let mut qpc = 0i64;
        QueryPerformanceCounter(&mut qpc);

        let e = Entry {
            pid,
            mode: MODE_ABS,
            year: rt.wYear,
            month: rt.wMonth,
            day: rt.wDay,
            dow: rt.wDayOfWeek,
            hour: rt.wHour,
            minute: rt.wMinute,
            second: rt.wSecond,
            millis: rt.wMilliseconds,
            utc_filetime: utc_ft,
            tick32: GetTickCount(),
            tick64: GetTickCount64(),
            qpc,
            ..Default::default()
        };
        let mut t = TABLE.lock().unwrap();
        t.retain(|x| x.pid != pid);
        t.push(e);
        publish_rows(&t);
        true
    }
}

// ---- persistence --------------------------------------------------------
/// On-disk mirror of TABLE. Keyed by pid **and** exe path: a restart re-freezes
/// a target only if that pid is still alive running the same image. This guards
/// against Windows recycling a pid onto an unrelated process.
#[derive(Serialize, Deserialize)]
struct PersistEntry {
    pid: u32,
    exe: String,
    mode: u32,
    year: u16,
    month: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
    offset_100ns: i64,
}

fn state_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(std::path::Path::new(&base).join("FreezeGun").join("frozen.json"))
}

/// Write the current TABLE to `%APPDATA%\FreezeGun\frozen.json`.
pub fn save_state() -> bool {
    let Some(path) = state_path() else { return false };
    let rows: Vec<PersistEntry> = TABLE
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| {
            if e.mode != MODE_ABS && e.mode != MODE_OFFSET {
                return None;
            }
            let exe = process_image_path(e.pid)?;
            Some(PersistEntry {
                pid: e.pid,
                exe,
                mode: e.mode,
                year: e.year,
                month: e.month,
                day: e.day,
                hour: e.hour,
                minute: e.minute,
                second: e.second,
                millis: e.millis,
                offset_100ns: e.offset_100ns,
            })
        })
        .collect();
    let json = match serde_json::to_string_pretty(&rows) {
        Ok(j) => j,
        Err(_) => return false,
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, json).is_ok()
}

/// Reload `frozen.json` and re-freeze any target still alive as the same image.
/// Returns the number restored. ABS rows are rebuilt through `freeze_absolute`,
/// which recaptures fresh tick/QPC baselines — so a restored hard-freeze stays
/// internally consistent after the gap between crash and relaunch.
pub fn load_state() -> usize {
    let Some(path) = state_path() else { return 0 };
    let Ok(json) = std::fs::read_to_string(&path) else { return 0 };
    let Ok(rows) = serde_json::from_str::<Vec<PersistEntry>>(&json) else {
        return 0;
    };
    // Live pid → exe map, built once (avoid OpenProcess per persisted row).
    let live: std::collections::HashMap<u32, String> = list_processes()
        .into_iter()
        .filter_map(|(pid, _)| process_image_path(pid).map(|exe| (pid, exe)))
        .collect();
    let mut n = 0;
    for pe in &rows {
        let Some(exe) = live.get(&pe.pid) else { continue };
        if !exe.eq_ignore_ascii_case(&pe.exe) {
            continue; // pid reused by a different image — do not touch it
        }
        match pe.mode {
            MODE_OFFSET => {
                freeze_offset(pe.pid, pe.offset_100ns);
                n += 1;
            }
            MODE_ABS => {
                if freeze_absolute(
                    pe.pid, pe.year, pe.month, pe.day,
                    pe.hour, pe.minute, pe.second, pe.millis,
                ) {
                    n += 1;
                }
            }
            _ => {}
        }
    }
    n
}