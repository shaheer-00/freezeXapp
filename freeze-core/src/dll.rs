//! DLL side (compiled into `freezex.dll`, `dll` feature). Installs MinHook
//! detours on the REAL kernel32/ntdll/winmm exports and serves frozen time to
//! the host process from a thread-local-cached view of the shared control
//! block. Hot path (per time call) is lock-free: one atomic gen-load + compare.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, OnceLock, RwLock};

use minhook::MinHook;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, FILE_MAP_READ, FILE_MAP_ALL_ACCESS,
};
use windows::Win32::System::Threading::GetCurrentProcessId;

use crate::common::*;

// Calendar conversions are NOT hooked — imported statically from kernel32 so a
// hook body can never recurse into itself.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn FileTimeToSystemTime(lpfiletime: *const FILETIME, lpSystemTime: *mut SYSTEMTIME) -> i32;
    fn SystemTimeToFileTime(lpSystemTime: *const SYSTEMTIME, lpfiletime: *mut FILETIME) -> i32;
}

// ---- raw C ABI signatures of the hooked exports -------------------------
type FnGetSystemTime = unsafe extern "system" fn(*mut SYSTEMTIME);
type FnGetLocalTime = unsafe extern "system" fn(*mut SYSTEMTIME);
type FnGetSysFileTime = unsafe extern "system" fn(*mut FILETIME);
type FnGetTickCount = unsafe extern "system" fn() -> u32;
type FnGetTickCount64 = unsafe extern "system" fn() -> u64;
type FnQpc = unsafe extern "system" fn(*mut i64) -> i32;
type FnNtQuerySystemTime = unsafe extern "system" fn(*mut FILETIME) -> i32; // NTSTATUS
type FnTimeGetTime = unsafe extern "system" fn() -> u32;

#[derive(Clone, Copy)]
struct Orig {
    get_system_time: FnGetSystemTime,
    get_local_time: FnGetLocalTime,
    get_sys_filetime: FnGetSysFileTime,
    get_tick: FnGetTickCount,
    get_tick64: FnGetTickCount64,
    qpc: FnQpc,
    nt_qst: FnNtQuerySystemTime,
    time_get: FnTimeGetTime,
}
static ORIG: OnceLock<Orig> = OnceLock::new();

// Fn pointers are non-nullable, so `mem::zeroed()` on `Orig` is UB (it panics /
// aborts). Seed with no-op stubs of the matching ABI instead; each is then
// overwritten by the real original from MinHook before the hooks are enabled.
unsafe extern "system" fn nop_st(_: *mut SYSTEMTIME) {}
unsafe extern "system" fn nop_ft(_: *mut FILETIME) {}
unsafe extern "system" fn nop_u32() -> u32 {
    0
}
unsafe extern "system" fn nop_u64() -> u64 {
    0
}
unsafe extern "system" fn nop_qpc(_: *mut i64) -> i32 {
    0
}
unsafe extern "system" fn nop_nt(_: *mut FILETIME) -> i32 {
    0xC000_0001u32 as i32
}

// ---- per-PID frozen config, cached process-wide then thread-locally ------
#[derive(Clone, Copy)]
struct AbsCfg {
    local_st: SYSTEMTIME,
    utc_st: SYSTEMTIME,
    utc_ft: u64,
    tick32: u32,
    tick64: u64,
    qpc: i64,
}
#[derive(Clone, Copy)]
pub enum Mode {
    Off,
    Abs(AbsCfg),
    Offset(i64), // 100ns delta added to live time
}

// HashMap::new isn't const (RandomState), so the table is built on first use.
static CACHE: LazyLock<RwLock<HashMap<u32, Mode>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
// Bumped by the refresh thread after every table swap. Lets the thread-local
// cache stay lock-free yet still observe live freeze/unfreeze/toggle changes.
static GEN: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static TL: Cell<(u64, u32, Mode)> = const { Cell::new((0, 0, Mode::Off)) };
}

#[inline]
fn ft_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}
#[inline]
fn u64_ft(v: u64) -> FILETIME {
    FILETIME { dwLowDateTime: v as u32, dwHighDateTime: (v >> 32) as u32 }
}
#[inline]
fn st_from_entry(e: &Entry) -> SYSTEMTIME {
    SYSTEMTIME {
        wYear: e.year,
        wMonth: e.month,
        wDayOfWeek: e.dow,
        wDay: e.day,
        wHour: e.hour,
        wMinute: e.minute,
        wSecond: e.second,
        wMilliseconds: e.millis,
    }
}

/// Resolve the effective mode for `pid`. Steady state = one atomic load + two
/// compares, no lock. Re-reads the shared table only when the gen changes.
#[inline]
fn mode_for(pid: u32) -> Mode {
    TL.with(|c| {
        let (g, last, m) = c.get();
        let cur = GEN.load(Ordering::Acquire);
        if g == cur && last == pid {
            return m;
        }
        let m = CACHE
            .read()
            .map(|t| t.get(&pid).copied().unwrap_or(Mode::Off))
            .unwrap_or(Mode::Off);
        c.set((cur, pid, m));
        m
    })
}

/// Shift a SYSTEMTIME the original just produced by `off` (100ns units).
#[inline]
unsafe fn shift_st(lp: *mut SYSTEMTIME, off: i64) {
    let mut ft = FILETIME::default();
    SystemTimeToFileTime(lp as *const SYSTEMTIME, &mut ft);
    let mut ft = u64_ft(ft_u64(ft).wrapping_add(off as u64));
    FileTimeToSystemTime(&ft, lp);
}

// ---- hook bodies --------------------------------------------------------
unsafe extern "system" fn hk_get_system_time(lp: *mut SYSTEMTIME) {
    let o = match ORIG.get() {
        Some(o) => *o,
        None => return,
    };
    match mode_for(GetCurrentProcessId()) {
        Mode::Off => (o.get_system_time)(lp),
        Mode::Abs(a) => *lp = a.utc_st,
        Mode::Offset(off) => {
            (o.get_system_time)(lp);
            shift_st(lp, off);
        }
    }
}

unsafe extern "system" fn hk_get_local_time(lp: *mut SYSTEMTIME) {
    let o = match ORIG.get() {
        Some(o) => *o,
        None => return,
    };
    match mode_for(GetCurrentProcessId()) {
        Mode::Off => (o.get_local_time)(lp),
        Mode::Abs(a) => *lp = a.local_st,
        Mode::Offset(off) => {
            (o.get_local_time)(lp);
            shift_st(lp, off);
        }
    }
}

unsafe extern "system" fn hk_get_sys_filetime(lp: *mut FILETIME) {
    let o = match ORIG.get() {
        Some(o) => *o,
        None => return,
    };
    match mode_for(GetCurrentProcessId()) {
        Mode::Off => (o.get_sys_filetime)(lp),
        Mode::Abs(a) => *lp = u64_ft(a.utc_ft),
        Mode::Offset(off) => {
            (o.get_sys_filetime)(lp);
            *lp = u64_ft(ft_u64(*lp).wrapping_add(off as u64));
        }
    }
}

unsafe extern "system" fn hk_nt_query_system_time(lp: *mut FILETIME) -> i32 {
    let o = match ORIG.get() {
        Some(o) => *o,
        None => return 0xC000_0001u32 as i32, // STATUS_UNSUCCESSFUL
    };
    match mode_for(GetCurrentProcessId()) {
        Mode::Off => (o.nt_qst)(lp),
        Mode::Abs(a) => {
            *lp = u64_ft(a.utc_ft);
            0 // STATUS_SUCCESS
        }
        Mode::Offset(off) => {
            let st = (o.nt_qst)(lp);
            *lp = u64_ft(ft_u64(*lp).wrapping_add(off as u64));
            st
        }
    }
}

unsafe extern "system" fn hk_get_tick_count() -> u32 {
    let o = match ORIG.get() {
        Some(o) => *o,
        None => return 0,
    };
    match mode_for(GetCurrentProcessId()) {
        Mode::Off => (o.get_tick)(),
        Mode::Abs(a) => a.tick32,
        Mode::Offset(off) => (o.get_tick)().wrapping_add((off / 10_000) as u32),
    }
}

unsafe extern "system" fn hk_get_tick_count64() -> u64 {
    let o = match ORIG.get() {
        Some(o) => *o,
        None => return 0,
    };
    match mode_for(GetCurrentProcessId()) {
        Mode::Off => (o.get_tick64)(),
        Mode::Abs(a) => a.tick64,
        Mode::Offset(off) => (o.get_tick64)().wrapping_add((off / 10_000) as u64),
    }
}

unsafe extern "system" fn hk_time_get_time() -> u32 {
    let o = match ORIG.get() {
        Some(o) => *o,
        None => return 0,
    };
    match mode_for(GetCurrentProcessId()) {
        Mode::Off => (o.time_get)(),
        Mode::Abs(a) => a.tick32,
        Mode::Offset(off) => (o.time_get)().wrapping_add((off / 10_000) as u32),
    }
}

unsafe extern "system" fn hk_qpc(lp: *mut i64) -> i32 {
    let o = match ORIG.get() {
        Some(o) => *o,
        None => return 0,
    };
    match mode_for(GetCurrentProcessId()) {
        Mode::Off => (o.qpc)(lp),
        Mode::Abs(a) => {
            *lp = a.qpc;
            1
        }
        Mode::Offset(off) => {
            let r = (o.qpc)(lp);
            *lp = lp.read().wrapping_add(off); // QPC assumed 10 MHz (100ns)
            r
        }
    }
}

// ---- install / teardown -------------------------------------------------
/// Best-effort install of every detour. A module that isn't loaded in the
/// target (e.g. winmm in a console app) is skipped, not fatal — the core
/// kernel32/ntdll hooks still land. Originals are stashed BEFORE enable, so no
/// target thread can reach a hook whose original is missing (MinHook only
/// patches on enable).
pub fn install() -> bool {
    use std::mem::transmute;
    unsafe {
        let mut o = Orig {
            get_system_time: nop_st,
            get_local_time: nop_st,
            get_sys_filetime: nop_ft,
            get_tick: nop_u32,
            get_tick64: nop_u64,
            qpc: nop_qpc,
            nt_qst: nop_nt,
            time_get: nop_u32,
        };
        let mut installed = 0u32;

        macro_rules! hk {
            ($field:ident, $module:expr, $proc:expr, $detour:expr, $ty:ty) => {
                if let Ok(p) = MinHook::create_hook_api($module, $proc, $detour as *mut c_void) {
                    o.$field = transmute::<*mut c_void, $ty>(p);
                    installed += 1;
                }
            };
        }

        hk!(get_system_time, "kernel32.dll", "GetSystemTime", hk_get_system_time, FnGetSystemTime);
        hk!(get_local_time, "kernel32.dll", "GetLocalTime", hk_get_local_time, FnGetLocalTime);
        hk!(get_sys_filetime, "kernel32.dll", "GetSystemTimeAsFileTime", hk_get_sys_filetime, FnGetSysFileTime);
        hk!(get_tick, "kernel32.dll", "GetTickCount", hk_get_tick_count, FnGetTickCount);
        hk!(get_tick64, "kernel32.dll", "GetTickCount64", hk_get_tick_count64, FnGetTickCount64);
        hk!(qpc, "kernel32.dll", "QueryPerformanceCounter", hk_qpc, FnQpc);
        hk!(nt_qst, "ntdll.dll", "NtQuerySystemTime", hk_nt_query_system_time, FnNtQuerySystemTime);
        hk!(time_get, "winmm.dll", "timeGetTime", hk_time_get_time, FnTimeGetTime);

        if installed == 0 {
            return false;
        }
        let _ = ORIG.set(o);
        MinHook::enable_all_hooks().is_ok()
    }
}

pub fn uninstall() {
    unsafe {
        let _ = MinHook::disable_all_hooks();
    }
}

// ---- shared-memory open + refresh --------------------------------------
/// Map the host's control block read-only. Leaks the view for process
/// lifetime (intentional — the DLL lives as long as the target).
pub fn open_control() -> Option<*const ControlBlock> {
    unsafe {
        let h = OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(SHMEM_NAME.as_ptr())).ok()?;
        let view = MapViewOfFile(h, FILE_MAP_READ, 0, 0, std::mem::size_of::<ControlBlock>());
        if view.Value.is_null() {
            return None;
        }
        let cb = view.Value as *const ControlBlock;
        if (*cb).magic != MAGIC {
            return None;
        }
        Some(cb)
    }
}

fn build_mode(e: &Entry) -> Mode {
    match e.mode {
        MODE_ABS => unsafe {
            let mut utc_st = SYSTEMTIME::default();
            let ft = u64_ft(e.utc_filetime);
            FileTimeToSystemTime(&ft, &mut utc_st);
            Mode::Abs(AbsCfg {
                local_st: st_from_entry(e),
                utc_st,
                utc_ft: e.utc_filetime,
                tick32: e.tick32,
                tick64: e.tick64,
                qpc: e.qpc,
            })
        },
        MODE_OFFSET => Mode::Offset(e.offset_100ns),
        _ => Mode::Off,
    }
}

/// Poll loop: refresh the process cache from shared memory. 150 ms cadence —
/// toggle latency a testing tool tolerates, and zero per-call IPC cost. Never
/// returns.
pub fn run_refresh(cb: *const ControlBlock) -> ! {
    loop {
        let (active, rows) = unsafe { seq_read(cb) };
        let mut map = HashMap::new();
        if active {
            for e in rows {
                if e.pid != 0 {
                    map.insert(e.pid, build_mode(&e));
                }
            }
        }
        if let Ok(mut g) = CACHE.write() {
            *g = map;
        }
        GEN.fetch_add(1, Ordering::Release);
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}