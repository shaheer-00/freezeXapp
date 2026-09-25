//! `freezex.dll` — the injected payload. DllMain does the bare minimum under
//! the loader lock (kick a worker thread), then the worker installs the time
//! hooks and starts polling the host's shared control block. All real logic
//! lives in `freeze_core::dll`.

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};

fn init_worker() {
    // Install hooks first: even if the shared block isn't up yet, the hooks
    // pass time through (empty cache → Mode::Off). Then map + refresh.
    freeze_core::dll::install();
    loop {
        if let Some(cb) = freeze_core::dll::open_control() {
            freeze_core::dll::run_refresh(cb); // never returns
        }
        // Host may create the mapping after we attached — keep retrying.
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(
    _h: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    match reason {
        DLL_PROCESS_ATTACH => {
            std::thread::spawn(init_worker);
        }
        DLL_PROCESS_DETACH => {
            freeze_core::dll::uninstall();
        }
        _ => {}
    }
    1 // TRUE
}