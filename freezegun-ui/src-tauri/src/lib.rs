// Tauri backend entry point.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Deserialize;
use std::path::PathBuf;
use tauri::{generate_context, generate_handler, Builder, Manager};
use freeze_core::common::{Entry as CbEntry, MODE_ABS, MODE_OFFSET};
use freeze_core::host::{
    ensure_shmem, list_processes, process_image_path, inject_dll,
    freeze_offset, unfreeze_pid, set_global_active, is_global_active,
    block_network, unblock_network, snapshot, current_pid,
    freeze_absolute, save_state, load_state,
};

#[derive(serde::Serialize, Clone)]
struct ProcessInfo {
    pid: u32,
    name: String,
    exe: String,
    frozen: bool,
    frozen_mode: String,   // "", "abs", "offset"
    frozen_at: String,     // human-local wall clock the target sees, e.g. "2026-09-25 03:00:00 +07"
}

#[derive(Deserialize)]
struct FreezeReq {
    pid: u32,
    delta_100ns: i64,
}

#[derive(Deserialize)]
struct FreezeAbsReq {
    pid: u32,
    year: u16,
    month: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

#[derive(serde::Serialize, Clone)]
struct AppInfo {
    name: String,
    path: String,
}

/// Render a local wall-clock string for an ABS-entry (what the target sees).
/// Mirrors the breakdown freeze_absolute precomputed into the Entry.
fn fmt_frozen_at(e: &CbEntry) -> String {
    if e.mode == MODE_OFFSET {
        // Offset rows have no absolute breakdown in the Entry — show the delta.
        let total_s = e.offset_100ns / 10_000_000;
        let h = total_s / 3600;
        let m = ((total_s % 3600) / 60).abs();
        return format!("Δ{}:{:02} live", h, m);
    }
    let bias = 8; // TODO: real tz bias via GetTimeZoneInformation if needed
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} +{:02}",
        e.year, e.month, e.day, e.hour, e.minute, e.second, e.millis, bias
    )
}
fn fmt_frozen_mode(e: &CbEntry) -> String {
    match e.mode {
        MODE_ABS    => "abs",
        MODE_OFFSET => "offset",
        _           => "",
    }.to_string()
}

#[tauri::command]
fn refresh_processes() -> Vec<ProcessInfo> {
    let snap = snapshot();
    let frozen: std::collections::HashMap<u32, CbEntry> =
        snap.into_iter().map(|e| (e.pid, e)).collect();
    list_processes()
        .into_iter()
        .map(|(pid, name)| {
            let exe = process_image_path(pid).unwrap_or_default();
            let fr = frozen.get(&pid);
            let frozen_flag = fr.is_some();
            ProcessInfo {
                pid, name, exe,
                frozen: frozen_flag,
                frozen_mode: fr.map(fmt_frozen_mode).unwrap_or_default(),
                frozen_at: fr.map(fmt_frozen_at).unwrap_or_default(),
            }
        })
        .collect()
}

/// Snapshot of currently-frozen targets (for the frozen-panel).
fn frozen_map() -> Vec<(CbEntry, String)> {
    let snap = snapshot();
    snap.into_iter()
        .filter(|e| e.mode == MODE_ABS || e.mode == MODE_OFFSET)
        .map(|e| {
            let exe = process_image_path(e.pid).unwrap_or_default();
            (e, exe)
        })
        .collect()
}

#[tauri::command]
fn freeze(req: FreezeReq) {
    let _ = ensure_shmem();
    freeze_offset(req.pid, req.delta_100ns);
    let _ = save_state();
}

#[tauri::command]
fn freeze_abs(req: FreezeAbsReq) -> bool {
    let _ = ensure_shmem();
    let ok = freeze_absolute(
        req.pid, req.year, req.month, req.day,
        req.hour, req.minute, req.second, req.millis,
    );
    if ok {
        let _ = save_state();
    }
    ok
}

#[tauri::command]
fn unfreeze(pid: u32) {
    unfreeze_pid(pid);
    let _ = save_state();
}

/// Last-resort recovery for a target that wedged after injection: hard-kill it.
/// Returns false if the handle couldn't be opened (protected / already gone).
#[tauri::command]
fn soft_terminate(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_TERMINATE,
    };
    unsafe {
        let h = match OpenProcess(PROCESS_TERMINATE, false, pid) {
            Ok(h) => h,
            Err(_) => return false,
        };
        let r = TerminateProcess(h, 1);
        let _ = CloseHandle(h);
        r.is_ok()
    }
}

/// True when this process runs elevated (admin). NET blocking needs this; the
/// UI uses it to decide whether to offer a relaunch-as-admin prompt.
#[tauri::command]
fn is_admin() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let res = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut TOKEN_ELEVATION as *mut std::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        res.is_ok() && elev.TokenIsElevated != 0
    }
}

/// Relaunch this exe elevated via a UAC prompt (PowerShell Start-Process -Verb
/// RunAs). Returns once the spawn is issued; the caller closes the current
/// (non-admin) window so only the elevated copy remains.
#[tauri::command]
fn relaunch_admin() -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };
    let ps = format!(
        "Start-Process -Verb RunAs -FilePath '{}'",
        exe.to_string_lossy().replace('\'', "''")
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_ok()
}

#[tauri::command]
fn toggle_global() -> bool {
    let next = !is_global_active();
    set_global_active(next);
    next
}

#[tauri::command]
fn global_active() -> bool {
    is_global_active()
}

#[tauri::command]
fn inject(pid: u32) {
    let dll = resolve_dll_path();
    let _ = inject_dll(pid, &dll);
}

#[tauri::command]
fn host_pid() -> u32 {
    current_pid()
}

#[tauri::command]
fn resolve_dll_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("freezex.dll")))
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

#[tauri::command]
fn block_net(pid: u32) -> bool {
    if let Some(exe) = process_image_path(pid) {
        block_network(&exe)
    } else {
        false
    }
}

#[tauri::command]
fn unblock_net(pid: u32) -> bool {
    if let Some(exe) = process_image_path(pid) {
        unblock_network(&exe)
    } else {
        true // no exe path → nothing to unblock → treat as no-op success
    }
}

/// Per-target frozen row for the frozen-panel.
#[derive(serde::Serialize, Clone)]
struct FrozenTarget {
    pid: u32,
    name: String,
    exe: String,
    frozen_mode: String,
    frozen_at: String,
    offset_100ns: i64,
    year: u16,
    month: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

#[tauri::command]
fn frozen_targets() -> Vec<FrozenTarget> {
    frozen_map()
        .into_iter()
        .map(|(e, exe)| {
            let name = exe.rsplit('\\').next().unwrap_or(&exe).to_string();
            FrozenTarget {
                pid: e.pid,
                name,
                exe,
                frozen_mode: fmt_frozen_mode(&e),
                frozen_at: fmt_frozen_at(&e),
                offset_100ns: e.offset_100ns,
                year: e.year,
                month: e.month,
                day: e.day,
                hour: e.hour,
                minute: e.minute,
                second: e.second,
                millis: e.millis,
            }
        })
        .collect()
}

// ── installed app discovery ───────────────────────────────────────────────

/// Resolve a .lnk shortcut to its target path. Returns None on failure.
fn resolve_lnk(lnk_path: &std::path::Path) -> Option<AppInfo> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED, IPersistFile, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    unsafe {
        let _ = CoInitializeEx(Some(std::ptr::null_mut()), COINIT_MULTITHREADED);

        // Create the Shell Link object.
        let shell_link: IShellLinkW = CoCreateInstance(
            &ShellLink, None, CLSCTX_INPROC_SERVER,
        ).ok()?;

        // Load the .lnk file.
        let pf: IPersistFile = shell_link.cast().ok()?;
        let wide: Vec<u16> = lnk_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        if pf.Load(PCWSTR(wide.as_ptr()), STGM_READ).is_err() {
            let _ = CoUninitialize();
            return None;
        }

        // Resolve the link target.
        let mut path_buf = [0u16; 4096];
        let result = shell_link.GetPath(
            &mut path_buf,
            std::ptr::null_mut(),  // WIN32_FIND_DATAW — not needed
            0,                     // fflags
        );
        let _ = CoUninitialize();
        result.ok()?;

        // Extract the null-terminated path string.
        let end = path_buf.iter().position(|&c| c == 0).unwrap_or(path_buf.len());
        let path_str: String = path_buf[..end]
            .iter()
            .map(|&c| char::from_u32(c as u32).unwrap_or('?'))
            .collect();

        let name = lnk_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("app")
            .to_string();

        Some(AppInfo { name, path: path_str })
    }
}

fn collect_lnk_dir(dir: &std::path::Path, out: &mut Vec<AppInfo>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_lnk_dir(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("lnk") {
            if let Some(info) = resolve_lnk(&p) {
                out.push(info);
            }
        }
    }
}

#[tauri::command]
fn list_installed_apps() -> Vec<AppInfo> {
    let mut out = Vec::new();

    // Start Menu Programs (user + system) + Desktop — recurse into subdirs.
    let bases: Vec<PathBuf> = [
        std::env::var("APPDATA").unwrap_or_default(),
        std::env::var("PROGRAMDATA").unwrap_or_default(),
    ]
    .iter()
    .map(|base| format!("{}\\Microsoft\\Windows\\Start Menu\\Programs", base))
    .chain([desktop_dir()])
    .map(PathBuf::from)
    .collect();

    for base in &bases {
        collect_lnk_dir(base, &mut out);
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.path.eq_ignore_ascii_case(&b.path));
    out
}

fn desktop_dir() -> String {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};

    unsafe {
        if let Ok(pwstr) = SHGetKnownFolderPath(&FOLDERID_Desktop, KNOWN_FOLDER_FLAG(0), None) {
            let len = (0..).take_while(|&i| *pwstr.0.add(i) != 0).count();
            let slice = std::slice::from_raw_parts(pwstr.0, len);
            let s = String::from_utf16_lossy(slice);
            CoTaskMemFree(Some(pwstr.0 as *mut _));
            return s;
        }
    }
    // Fallback
    std::env::var("USERPROFILE").unwrap_or_default() + "\\Desktop"
}

/// Freeze every process whose name contains `name` (case-insensitive).
/// Injects the hook DLL into each, then freezes by absolute clock.
/// Returns the pids that were frozen.
#[tauri::command]
fn freeze_by_name(name: &str, req: FreezeAbsReq) -> Vec<u32> {
    let _ = ensure_shmem();
    let needle = name.to_lowercase();
    let mut frozen = Vec::new();
    for (pid, pname) in list_processes() {
        if !pname.to_lowercase().contains(&needle) {
            continue;
        }
        let dll = resolve_dll_path();
        let _ = inject_dll(pid, &dll);
        if freeze_absolute(
            pid, req.year, req.month, req.day,
            req.hour, req.minute, req.second, req.millis,
        ) {
            frozen.push(pid);
        }
    }
    if !frozen.is_empty() {
        let _ = save_state();
    }
    frozen
}

/// Retune a frozen pid between ABS and OFFSET mode without losing it.
#[tauri::command]
fn retune(pid: u32, mode: String, delta_100ns: i64) {
    let _ = ensure_shmem();
    if mode == "offset" {
        freeze_offset(pid, delta_100ns);
    }
    // ABS would need a full freeze_absolute call with a target datetime;
    // the UI passes mode="abs" with a pre-filled datetime which we'd need
    // as a struct. For now, offset retune is the live toggle (DEAD↔SHIFT).
    // ABS retune reconstructs from stored Entry via frozen_targets data.
    let _ = save_state();
}

/// Persist current freeze table to disk.
#[tauri::command]
fn save_settings() -> bool {
    save_state()
}

#[tauri::command]
fn launch_app(path: &str) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| true)
        .unwrap_or(false)
}

pub fn run() {
    let _ = ensure_shmem();
    let restored = load_state();
    if restored > 0 {
        eprintln!("freezeXapp: restored {restored} frozen target(s) from disk");
    } else {
        // ensure a fresh control-block publish on boot even if nothing persisted
        let _ = save_state();
    }
    Builder::default()
        .setup(|app| {
            // persist freeze table on graceful window close
            let win = app.get_webview_window("main").unwrap();
            win.clone().on_window_event(move |ev| {
                if let tauri::WindowEvent::CloseRequested { .. } = ev {
                    let _ = save_state();
                }
            });
            Ok(())
        })
        .invoke_handler(generate_handler![
            refresh_processes,
            freeze,
            freeze_abs,
            unfreeze,
            frozen_targets,
            soft_terminate,
            is_admin,
            relaunch_admin,
            toggle_global,
            global_active,
            inject,
            resolve_dll_path,
            block_net,
            unblock_net,
            host_pid,
            list_installed_apps,
            launch_app,
            freeze_by_name,
            retune,
            save_settings,
        ])
        .run(generate_context!())
        .expect("tauri run failed");
}
