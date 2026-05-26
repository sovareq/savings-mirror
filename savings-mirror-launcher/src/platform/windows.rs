//! Windows-specific bits.

use std::path::PathBuf;

/// Logs go to `%LOCALAPPDATA%\SavingsMirror\logs\` per Windows convention.
pub fn log_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("APPDATA"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("SavingsMirror").join("logs")
}

/// On Windows a tray-only app shows in neither the taskbar nor Alt-Tab as long
/// as we never call `with_visible(true)` on a real window. Nothing to do here.
pub fn activation_policy_accessory() {}
