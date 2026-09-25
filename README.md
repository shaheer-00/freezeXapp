# freezeXapp

A Windows cyberdeck that freezes the system clock for any process and severs its network egress. A kernel-mode time-hook DLL (`freezex.dll`) is injected into target processes; the host writes a per-PID freeze schedule to shared memory and injected hooks intercept `GetSystemTime`, `GetLocalTime`, `GetSystemTimeAsFileTime`, `GetTickCount`, `GetTickCount64`, `QueryPerformanceCounter`, `NtQuerySystemTime`, and `timeGetTime`.

## Features

- **Freeze by PID or name glob** — `freeze chrome` matches all chrome processes
- **Dead-stop / hold-at / shift modes** — halt clock at now, freeze at a specific datetime, or offset by `-1:30`
- **Frozen targets panel** — sortable list with per-row unfreeze, DEAD↔SHIFT toggle, and recovery chip
- **Bulk select** — checkbox column + action bar (freeze/unfreeze/inject/net)
- **Network sever** — `netsh` firewall outbound rule by exe path (requires admin)
- **State persistence** — `%APPDATA%\FreezeGun\frozen.json` survives restart
- **Delta memory** — remembers last SHIFT offset per app

## Build (Windows)

```bash
cd freezegun-ui
cargo tauri dev
```

Requires: Rust, VS 2022 C++ Build Tools, WebView2 runtime.

## Usage

Type commands in the shell prompt:

```
freeze <pid|name>     — open freeze dialog
unfreeze <pid>        — release clock
inject <pid>          — load hook payload only
net <pid>             — toggle egress block (needs admin)
install               — list installed applications
launch <path>         — start app + auto-target
halt / wake           — all hooks off / on
filter <text>         — narrow target list
```

## Legal

Built for testing, debugging, and authorized compatibility work. You are responsible for complying with the law and license terms of any software you target.
