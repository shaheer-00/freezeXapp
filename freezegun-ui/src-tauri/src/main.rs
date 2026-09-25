// Tauri backend entry.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::os::windows::process::CommandExt;

fn main() {
    // --elevate-net <pid> : elevated subprocess path for NET toggle.
    // Spawned by the UI layer (relaunch_admin-style) when non-admin
    // block_net fails with access denied. Runs netsh as elevated,
    // writes result to a temp file the parent reads, then exits.
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--elevate-net" {
        let pid: u32 = match args[2].parse() {
            Ok(p) if p > 0 => p,
            _ => std::process::exit(2),
        };
        let success;
        if let Some(exe) = freeze_core::host::process_image_path(pid) {
            let rule = format!("freezeXapp::{}", exe.rsplit('\\').next().unwrap_or(&exe));
            let add = std::process::Command::new("netsh")
                .args(&["advfirewall", "firewall", "add", "rule",
                         "name", &rule, "dir", "out", "action", "block",
                         "program", &format!("\"{}\"", exe), "enable", "yes", "profile", "any"])
                .creation_flags(0x0800_0000)
                .status();
            success = add.map(|s| s.success()).unwrap_or(false);
        } else {
            success = false;
        }
        std::process::exit(if success { 0 } else { 1 });
    }
    freezegun_lib::run();
}
