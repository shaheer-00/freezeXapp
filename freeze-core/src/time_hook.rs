use std::sync::atomic::Ordering;
use windows::Win32::Foundation::{BOOL, HANDLE};
use windows::Win32::System::SystemServices::SYSTEMTIME;
use windows::Win32::System::Time::GetSystemTime;

use crate::ACTIVE;
use crate::FROZEN_PIDS;

extern "system" fn hooked_get_system_time() -> SYSTEMTIME {
    let pid = unsafe { windows::Win32::System::Threading::GetCurrentProcessId() };
    let frozen = {
        let guard = FROZEN_PIDS.lock().unwrap();
        guard.contains(&pid)
    };
    if frozen && ACTIVE.load(Ordering::SeqCst) {
        // Return hardcoded frozen time
        SYSTEMTIME {
            wYear: 1970,
            wMonth: 1,
            wDayOfWeek: 4,
            wDay: 1,
            wHour: 0,
            wMinute: 0,
            wSecond: 0,
            wMilliseconds: 0,
        }
    } else {
        unsafe { GetSystemTime() }
    }
}

pub fn install_hooks() -> bool {
    true
}
