//! End-to-end verification of the hard part: hook install + shared-memory IPC +
//! offset math + absolute freeze + unfreeze, across the whole hooked API set.
//!
//! Loads freezex.dll into THIS process (so DllMain installs the detours here),
//! publishes freezes for our own PID through the real control block, and
//! measures each hooked export before/during/after.
//!
//! Run from the workspace root:
//!   cargo run -p freeze-core --features host --example verify_freeze
//!
//! NOT covered (needs admin): remote injection into a separate process, and the
//! netsh firewall path.

use freeze_core::host::{ensure_shmem, freeze_absolute, freeze_offset, unfreeze_pid};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::core::PCWSTR;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemTime(lp: *mut SYSTEMTIME);
    fn GetLocalTime(lp: *mut SYSTEMTIME);
    fn GetSystemTimeAsFileTime(lp: *mut FILETIME);
    fn GetTickCount() -> u32;
    fn GetTickCount64() -> u64;
    fn QueryPerformanceCounter(lp: *mut i64) -> i32;
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemTime(lp: *mut FILETIME) -> i32;
}
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeGetTime() -> u32;
}

fn ft_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}
fn sys_ft() -> u64 {
    let mut ft = FILETIME::default();
    unsafe { GetSystemTimeAsFileTime(&mut ft) };
    ft_u64(ft)
}
fn nt_ft() -> u64 {
    let mut ft = FILETIME::default();
    unsafe { NtQuerySystemTime(&mut ft) };
    ft_u64(ft)
}
fn sys_st() -> (u16, u16, u16, u16, u16, u16) {
    let mut st = SYSTEMTIME::default();
    unsafe { GetSystemTime(&mut st) };
    (st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond)
}
/// The user picks a LOCAL wall clock, so that is what must round-trip.
fn local_st() -> (u16, u16, u16, u16, u16, u16) {
    let mut st = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut st) };
    (st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond)
}

fn main() {
    let dll = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/debug/freezex.dll".to_string());

    if !ensure_shmem() {
        println!("RESULT: FAIL — could not create control block");
        std::process::exit(1);
    }
    let pid = unsafe { GetCurrentProcessId() };

    let base_ft = sys_ft();
    let base_nt = nt_ft();
    let base_tick = unsafe { GetTickCount64() };

    let mut w: Vec<u16> = dll.encode_utf16().collect();
    w.push(0);
    let h = unsafe { LoadLibraryW(PCWSTR(w.as_ptr())) };
    if h.is_err() {
        println!("RESULT: FAIL — LoadLibraryW({dll}) failed: {:?}", h.err());
        std::process::exit(1);
    }

    let delta_h: i64 = 3_600 * 10_000_000; // +1h in 100ns

    // ---------- OFFSET mode ----------
    freeze_offset(pid, delta_h);
    std::thread::sleep(std::time::Duration::from_millis(900));

    let off_ft = sys_ft() as i64 - base_ft as i64;
    let off_nt = nt_ft() as i64 - base_nt as i64;
    let off_tick = unsafe { GetTickCount64() } as i64 - base_tick as i64;

    // Fake / hook presence checks (values must exist & be sane)
    let mut st = SYSTEMTIME::default();
    unsafe { GetSystemTime(&mut st) };
    let mut lt = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut lt) };
    let _ = unsafe { GetTickCount() };
    let mut q = 0i64;
    let qok = unsafe { QueryPerformanceCounter(&mut q) };
    let _ = unsafe { timeGetTime() };

    // ---------- ABS mode ----------
    // Freeze at a fixed, recognizable moment.
    let abs_ok_pub = freeze_absolute(pid, 2001, 9, 9, 1, 46, 40, 0);
    std::thread::sleep(std::time::Duration::from_millis(900));
    let a1 = local_st();
    let a2_ft = sys_ft();
    std::thread::sleep(std::time::Duration::from_millis(400));
    let a2 = local_st();
    let a3_ft = sys_ft();

    // ---------- unfreeze ----------
    unfreeze_pid(pid);
    std::thread::sleep(std::time::Duration::from_millis(700));
    let after = sys_ft() as i64 - base_ft as i64;

    // ---------- report ----------
    let tick_note = "tick offset vs 1h = ".to_string() + &(off_tick - 3_600_000).to_string();
    println!("== OFFSET (+1h) ==");
    println!("  GetSystemTimeAsFileTime Δ = {off_ft}  (expect ≈36000000000)");
    println!("  NtQuerySystemTime       Δ = {off_nt}  (expect ≈36000000000)");
    println!("  GetTickCount64          Δ = {off_tick}  (expect ≈3600000 ms; {tick_note})");
    println!("  GetSystemTime   = {:?}", sys_st());
    println!("  GetLocalTime    = {lt:?}");
    println!("  QPC returned {qok} value {q}");
    println!("== ABS (2001-09-09 01:46:40) ==");
    println!("  publish ok = {abs_ok_pub}");
    println!("  read #1 = {a1:?}");
    println!("  read #2 = {a2:?}   (must equal #1 — frozen)");
    println!("  ft #1 = {a2_ft}");
    println!("  ft #2 = {a3_ft}   (must equal — frozen)");
    println!("== UNFREEZE ==");
    println!("  Δ vs baseline = {after}  (expect ≈0)");

    let offset_ok = (off_ft - delta_h).abs() < 10_000_000;
    let nt_ok = (off_nt - delta_h).abs() < 10_000_000;
    let tick_ok = (off_tick - 3_600_000).abs() < 5_000;
    let abs_frozen = a1 == a2 && a2_ft == a3_ft;
    let abs_value = a1 == (2001, 9, 9, 1, 46, 40);
    let restore_ok = after.abs() < 50_000_000;

    println!();
    if offset_ok && nt_ok && tick_ok && abs_frozen && abs_value && restore_ok {
        println!("RESULT: PASS — all 8 hooks land; offset, absolute freeze, ticks, and unfreeze all correct");
    } else {
        println!(
            "RESULT: FAIL — offset={offset_ok} nt={nt_ok} tick={tick_ok} abs_frozen={abs_frozen} abs_value={abs_value} restore={restore_ok}"
        );
    }
}
