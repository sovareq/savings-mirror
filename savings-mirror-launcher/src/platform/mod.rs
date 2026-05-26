//! Platform-specific helpers. Each cfg-gated module exposes the same surface:
//! - `log_dir()` returns the per-OS standard log location for SavingsMirror
//! - `activation_policy_accessory()` hides the app from the Dock/taskbar so it
//!   only shows up as a tray icon

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{activation_policy_accessory, log_dir};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{activation_policy_accessory, log_dir};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod fallback {
    use std::path::PathBuf;

    pub fn activation_policy_accessory() {}

    pub fn log_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".local/share/savings-mirror/logs")
    }
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use fallback::{activation_policy_accessory, log_dir};
