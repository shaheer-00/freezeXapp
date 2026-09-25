//! freeze-core — the shared engine behind FreezeGun.
//!
//! Three layers, selected by feature:
//! - `common` : the cross-process control block (always compiled).
//! - `host`   : controller side — owns desired state, publishes it to shared
//!              memory, enumerates/injects processes, toggles the firewall.
//! - `dll`    : injected side — MinHook detours on the real time exports,
//!              serving frozen time from the shared block.
//!
//! A given process links EITHER `host` (the Tauri app) OR `dll` (freezex.dll);
//! they meet only at runtime through the named file mapping in `common`.

#![allow(unsafe_op_in_unsafe_fn)]

pub mod common;

#[cfg(feature = "host")]
pub mod host;

#[cfg(feature = "dll")]
pub mod dll;