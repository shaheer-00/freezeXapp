//! Shared control block: the single source of truth between the host (Tauri)
//! and every injected `freezex.dll`. Lives in a named page-file mapping.
//!
//! Sync model = classic seqlock. Host is the only writer; it bumps `seq` to an
//! odd value, writes entries + `count`, then bumps to even. DLL readers spin if
//! they catch an odd `seq`, otherwise read and re-check `seq` for stability.
//! No cross-process mutex on the read path → the per-time-call lookup in the
//! target stays cheap (further cached thread-locally inside the DLL).

use std::sync::atomic::{AtomicU32, Ordering};

pub const MAGIC: u32 = 0x465A_4731; // "FZG1"
pub const VERSION: u32 = 1;
pub const MAX_ENTRIES: usize = 512;
/// Per-terminal-session namespace: works without SeTcbPrivilege, and host +
/// target normally share a session.
pub const SHMEM_NAME: &[u16] = &[
    'L' as u16, 'o' as u16, 'c' as u16, 'a' as u16, 'l' as u16, '\\' as u16,
    'F' as u16, 'r' as u16, 'e' as u16, 'e' as u16, 'z' as u16, 'e' as u16,
    'G' as u16, 'u' as u16, 'n' as u16, '.' as u16, 'C' as u16, 't' as u16,
    'l' as u16, 0,
];

pub const MODE_OFF: u32 = 0;
pub const MODE_ABS: u32 = 1;     // hard freeze at an absolute wall clock
pub const MODE_OFFSET: u32 = 2;  // real time + a fixed offset (100ns units)

/// One per-target row. `repr(C)`, `Copy`, POD — overlaid on mapped memory.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Entry {
    pub pid: u32,
    pub mode: u32,
    // Absolute local wall clock (what the user picked). UTC filetime is
    // precomputed by the host into `utc_filetime` so the DLL never touches tz.
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub dow: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
    pub utc_filetime: u64, // 100ns since 1601-01-01 UTC
    // Tick/QPC bases captured by the host at freeze time (system-global, so
    // host-captured == what the target would read). Returned verbatim in ABS
    // mode → counters stop advancing (a true freeze). OFFSET mode adds to live.
    pub tick32: u32,
    pub _pad32: u32,
    pub tick64: u64,
    pub qpc: i64,        // assumed 10 MHz (100ns units) — true on all modern x86/x64
    pub offset_100ns: i64, // OFFSET mode delta
}

#[repr(C)]
pub struct ControlBlock {
    pub magic: u32,
    pub version: u32,
    pub active: AtomicU32, // global kill switch: 0 = every hook passes through
    pub seq: AtomicU32,    // seqlock counter
    pub count: AtomicU32,  // valid rows in `entries`
    pub entries: [Entry; MAX_ENTRIES],
}

/// Consistent read of the whole table. Returns (global_active, rows).
///
/// # Safety
/// `cb` must point at a valid, mapped `ControlBlock`.
pub unsafe fn seq_read(cb: *const ControlBlock) -> (bool, Vec<Entry>) {
    let active = (*cb).active.load(Ordering::Acquire) != 0;
    loop {
        let s1 = (*cb).seq.load(Ordering::Acquire);
        if s1 & 1 == 1 {
            std::hint::spin_loop();
            continue;
        }
        let count = ((*cb).count.load(Ordering::Relaxed) as usize).min(MAX_ENTRIES);
        let base = std::ptr::addr_of!((*cb).entries) as *const Entry;
        let mut rows = Vec::with_capacity(count);
        for i in 0..count {
            rows.push(std::ptr::read_volatile(base.add(i)));
        }
        let s2 = (*cb).seq.load(Ordering::Relaxed);
        if s1 == s2 {
            return (active, rows);
        }
    }
}

/// Publish a new table. Caller is the sole writer.
///
/// # Safety
/// `cb` must point at a writable, mapped `ControlBlock`; no other writer runs.
pub unsafe fn seq_write(cb: *mut ControlBlock, active: bool, rows: &[Entry]) {
    let n = rows.len().min(MAX_ENTRIES);
    (*cb).active.store(active as u32, Ordering::Relaxed);
    (*cb).seq.fetch_add(1, Ordering::Acquire); // → odd
    let base = std::ptr::addr_of_mut!((*cb).entries) as *mut Entry;
    for i in 0..n {
        std::ptr::write_volatile(base.add(i), rows[i]);
    }
    (*cb).count.store(n as u32, Ordering::Relaxed);
    (*cb).seq.fetch_add(1, Ordering::Release); // → even
}