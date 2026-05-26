//! macOS-specific bits.

use std::path::PathBuf;

/// Logs go to `~/Library/Logs/SavingsMirror/` per Apple's HIG.
pub fn log_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Library/Logs/SavingsMirror")
}

/// Sets the app's activation policy to "accessory" so it lives in the menu bar
/// only — no Dock icon, no Cmd-Tab entry. Called once at startup.
///
/// We use a tiny inline objc message rather than dragging in `objc2-app-kit`
/// just for this one call. SAFETY: NSApp is the standard shared application
/// instance; `setActivationPolicy:` is well-defined for any value of the
/// `NSApplicationActivationPolicy` enum (0 = Regular, 1 = Accessory, 2 = Prohibited).
pub fn activation_policy_accessory() {
    // The objc runtime is always present in a macOS process; we look up the
    // class and selector at runtime to avoid a hard build-time framework link.
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    // Use the objc runtime via libobjc; this is the same call NSApplication
    // makes internally. Wrapped in a no-fail Result-ish form: if the message
    // send fails we just log and move on.
    unsafe {
        type Id = *mut std::ffi::c_void;
        type Sel = *const std::ffi::c_void;
        unsafe extern "C" {
            fn objc_getClass(name: *const std::ffi::c_char) -> Id;
            fn sel_registerName(name: *const std::ffi::c_char) -> Sel;
            fn objc_msgSend();
        }
        let class_name = c"NSApplication".as_ptr();
        let shared_sel = c"sharedApplication".as_ptr();
        let policy_sel = c"setActivationPolicy:".as_ptr();

        let ns_app_class = objc_getClass(class_name);
        if ns_app_class.is_null() {
            eprintln!("activation_policy: NSApplication class not found");
            return;
        }

        // shared = [NSApplication sharedApplication]
        // NSApplicationActivationPolicyAccessory = 1
        let msg_send_get: extern "C" fn(Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        let msg_send_set: extern "C" fn(Id, Sel, i64) -> () =
            std::mem::transmute(objc_msgSend as *const ());

        let shared = msg_send_get(ns_app_class, sel_registerName(shared_sel));
        if shared.is_null() {
            eprintln!("activation_policy: sharedApplication returned nil");
            return;
        }
        msg_send_set(shared, sel_registerName(policy_sel), 1);
    }
}
