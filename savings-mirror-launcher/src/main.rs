//! savings-mirror launcher — cross-platform tray-only Rust binary.
//!
//! Lives in the menu-bar (macOS) or system-tray (Windows) and exposes:
//!   * Status line (auto-updating, label-only)
//!   * Start runtime — spawn `savings-mirror` as a child process
//!   * Stop runtime  — terminate the child
//!   * Open dashboard — launch the default browser at 127.0.0.1:8991
//!   * Quit          — stop the child, then exit
//!
//! No embedded webview; no main window. The dashboard is the existing one
//! served by `savings-mirror` itself; we just open it in the user's browser.
//!
//! Single-instance lock prevents two launchers from racing over the same
//! child process. Logs go to the per-OS standard location.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use single_instance::SingleInstance;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

mod platform;

const DASHBOARD_PORT: u16 = 8991;
const SINGLE_INSTANCE_ID: &str = "com.sovareq.savings-mirror.launcher";
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Shared launcher state — kept behind a `Mutex` because tao + tray-icon are
/// happy on the main thread but the status poller runs on its own thread.
struct State {
    child: Option<Child>,
    last_status: Status,
    log_file: Option<File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Stopped,
    Running,
    Crashed,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Stopped => "Status: stopped",
            Status::Running => "Status: running on :8991",
            Status::Crashed => "Status: stopped (crashed — see log)",
        }
    }
}

/// Resolve which `savings-mirror` binary to launch. Tries in order:
///   1. `$SAVINGS_MIRROR_BINARY` env var
///   2. `../target/release/savings-mirror` relative to the launcher binary
///      (dev convenience when running from `cargo run`)
///   3. The canonical developer-machine path `~/savings-mirror/target/release/savings-mirror`
///   4. `savings-mirror` adjacent to the launcher binary (`.app` Resources / next to .exe)
///   5. `savings-mirror` found via `PATH`
fn resolve_runtime_binary() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("SAVINGS_MIRROR_BINARY") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Ok(pb);
        }
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let candidate = parent.join("../target/release/savings-mirror");
        if candidate.is_file() {
            return Ok(candidate);
        }
        let sibling = parent.join(if cfg!(windows) {
            "savings-mirror.exe"
        } else {
            "savings-mirror"
        });
        if sibling.is_file() {
            return Ok(sibling);
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let dev_path = PathBuf::from(&home).join("savings-mirror/target/release/savings-mirror");
    if dev_path.is_file() {
        return Ok(dev_path);
    }

    // PATH fallback — let the OS find it.
    Ok(PathBuf::from(if cfg!(windows) {
        "savings-mirror.exe"
    } else {
        "savings-mirror"
    }))
}

fn open_log_file() -> Result<File> {
    let dir = platform::log_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating log dir {dir:?}"))?;
    let path = dir.join("launcher.log");
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening log file {path:?}"))
}

fn log_line(state: &mut State, msg: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{stamp}] {msg}\n");
    eprint!("{line}");
    if let Some(f) = state.log_file.as_mut() {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Cheap TCP probe: did someone already bind `127.0.0.1:port`? Used by
/// `start_runtime` to attach to a runtime that another process started
/// (Claude Code SessionStart hook, a previous launcher session that lost its
/// child handle on quit, a developer running `cargo run` in a terminal, etc.)
/// rather than blindly spawning a duplicate and hitting `Address already in
/// use` on bind.
fn dashboard_port_alive(port: u16) -> bool {
    let addr = match format!("127.0.0.1:{port}").parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

fn start_runtime(state: &mut State) -> Result<()> {
    if state.child.is_some() {
        log_line(state, "start_runtime: already running (we own the child)");
        return Ok(());
    }
    // External runtime detection — if something is already serving on
    // DASHBOARD_PORT (session-hook, leftover process, dev terminal), don't
    // spawn a duplicate. Mark as Running and let menu items / dashboard
    // links continue to work. Stop button becomes a no-op for externally-
    // owned runtimes; that limitation is documented in the menu tooltip.
    if dashboard_port_alive(DASHBOARD_PORT) {
        log_line(
            state,
            &format!(
                "start_runtime: external runtime detected on :{DASHBOARD_PORT} — attaching, not spawning"
            ),
        );
        state.last_status = Status::Running;
        return Ok(());
    }
    let bin = resolve_runtime_binary()?;
    log_line(state, &format!("start_runtime: spawning {bin:?}"));

    // Open a fresh handle so child stdio shares the log file.
    let stdout = state.log_file.as_ref().and_then(|f| f.try_clone().ok());
    let stderr = state.log_file.as_ref().and_then(|f| f.try_clone().ok());

    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null());
    if let Some(o) = stdout {
        cmd.stdout(o);
    }
    if let Some(e) = stderr {
        cmd.stderr(e);
    }

    let child = cmd.spawn().with_context(|| format!("spawning {bin:?}"))?;
    state.child = Some(child);
    state.last_status = Status::Running;
    Ok(())
}

fn stop_runtime(state: &mut State) {
    if let Some(mut child) = state.child.take() {
        log_line(state, "stop_runtime: terminating child");
        let _ = child.kill();
        // Reap so the OS doesn't leak a zombie. Short bounded wait.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                _ => break,
            }
        }
    }
    state.last_status = Status::Stopped;
}

fn open_dashboard(state: &mut State) {
    let url = format!("http://127.0.0.1:{DASHBOARD_PORT}");
    log_line(state, &format!("open_dashboard: {url}"));
    if let Err(e) = open::that(&url) {
        log_line(state, &format!("open_dashboard FAILED: {e}"));
    }
}

/// Poll the child to detect unexpected exits. Returns the new status if it
/// changed since `state.last_status`.
///
/// When we don't own a child handle (attached-mode, see `start_runtime`),
/// fall back to a port probe so the menu stays accurate.
fn poll_status(state: &mut State) -> Option<Status> {
    let new = match state.child.as_mut() {
        None => {
            if dashboard_port_alive(DASHBOARD_PORT) {
                Status::Running
            } else {
                Status::Stopped
            }
        }
        Some(c) => match c.try_wait() {
            Ok(Some(s)) => {
                log_line(
                    state,
                    &format!("poll_status: child exited (status={s}) — marking crashed"),
                );
                state.child = None;
                // If something else is still serving on the port (e.g. the
                // session-hook respawned faster than we polled), prefer
                // Running over the alarming "Crashed" label.
                if dashboard_port_alive(DASHBOARD_PORT) {
                    Status::Running
                } else {
                    Status::Crashed
                }
            }
            Ok(None) => Status::Running,
            Err(_) => Status::Running,
        },
    };
    if new != state.last_status {
        state.last_status = new;
        Some(new)
    } else {
        None
    }
}

/// Load the tray icon. Tries the embedded asset first; falls back to a 22×22
/// brown square so the build never breaks on a missing asset.
fn load_tray_icon() -> Result<Icon> {
    static BYTES: &[u8] = include_bytes!("../assets/tray-template.png");
    if !BYTES.is_empty() {
        let img = image::load_from_memory(BYTES)?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return Icon::from_rgba(rgba.into_raw(), w, h).map_err(|e| anyhow!(e.to_string()));
    }
    fallback_icon()
}

fn fallback_icon() -> Result<Icon> {
    // 22×22 warm-brown square, mostly opaque. Used when the proper template
    // image is absent (first build, broken asset, etc.).
    let w: u32 = 22;
    let h: u32 = 22;
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        buf.extend_from_slice(&[0x7a, 0x4e, 0x2d, 0xff]);
    }
    Icon::from_rgba(buf, w, h).map_err(|e| anyhow!(e.to_string()))
}

fn main() -> Result<()> {
    // Re-anchor CWD before anything that touches the filesystem — LaunchServices
    // launches with CWD=`/` which is read-only on modern macOS, breaking
    // CWD-relative lock files. Also opens a startup-trace log to disk so
    // failures from a Finder double-click are visible even when stderr is
    // swallowed by the OS.
    let safe_cwd = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::env::set_current_dir(&safe_cwd);

    let mut startup_log = open_log_file().ok();
    let write_trace = |f: &mut Option<File>, msg: &str| {
        if let Some(file) = f.as_mut() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(file, "[{stamp}] trace: {msg}");
            let _ = file.flush();
        }
    };
    write_trace(&mut startup_log, "main() entered");

    // Single-instance lock — prevents two launchers racing the child PID.
    // If we lose the race (an instance already runs), open the dashboard
    // instead so a Finder double-click never looks like a no-op.
    let instance = match SingleInstance::new(SINGLE_INSTANCE_ID) {
        Ok(i) => {
            write_trace(&mut startup_log, "single-instance lock acquired");
            i
        }
        Err(e) => {
            write_trace(&mut startup_log, &format!("single-instance error: {e}"));
            return Err(anyhow!("creating single-instance lock: {e}"));
        }
    };
    if !instance.is_single() {
        write_trace(
            &mut startup_log,
            "another instance running — opening dashboard",
        );
        let url = format!("http://127.0.0.1:{DASHBOARD_PORT}");
        let _ = open::that(&url);
        return Ok(());
    }

    write_trace(&mut startup_log, "calling activation_policy_accessory");
    platform::activation_policy_accessory();
    write_trace(&mut startup_log, "activation_policy_accessory returned");

    // Reuse the startup-trace handle as the runtime log file.
    let log_file = startup_log;
    let state = Arc::new(Mutex::new(State {
        child: None,
        last_status: Status::Stopped,
        log_file,
    }));

    // Build the menu.
    let menu = Menu::new();
    let status_item = MenuItem::new(Status::Stopped.label(), false, None);
    let start_item = MenuItem::new("Start runtime", true, None);
    let stop_item = MenuItem::new("Stop runtime", true, None);
    let dash_item = MenuItem::new("Open dashboard", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append(&status_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&start_item)?;
    menu.append(&stop_item)?;
    menu.append(&dash_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    let start_id = start_item.id().clone();
    let stop_id = stop_item.id().clone();
    let dash_id = dash_item.id().clone();
    let quit_id = quit_item.id().clone();

    {
        let mut s = state.lock().expect("state mutex");
        log_line(&mut s, "creating event loop");
    }

    // Build the event loop BEFORE the tray-icon. On macOS, tao's EventLoop
    // initialises the shared NSApplication; tray-icon must be constructed
    // afterwards so it can register against that NSApp. Building the tray
    // first works by accident in some contexts (e.g. cargo run from a TTY)
    // but fails silently when launched via LaunchServices.
    let event_loop = EventLoopBuilder::new().build();

    {
        let mut s = state.lock().expect("state mutex");
        log_line(&mut s, "loading tray icon");
    }
    let icon = load_tray_icon().context("loading tray icon")?;
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip("SavingsMirror")
        .build()?;
    {
        let mut s = state.lock().expect("state mutex");
        log_line(&mut s, "tray icon built — entering event loop");
    }

    // Auto-start the runtime on launch. Without this, a Finder double-click
    // looks like a no-op: LSUIElement=true hides the Dock-icon, so the only
    // user-visible feedback is the menubar item — which most users miss. If
    // the spawn fails we log and keep the tray alive so the user can retry
    // via "Start runtime".
    //
    // The dashboard auto-open is skipped when SAVINGS_MIRROR_NO_DASHBOARD=1,
    // which the Claude Code SessionStart hook sets — otherwise every session
    // would spawn a fresh browser tab.
    {
        let mut s = state.lock().expect("state mutex");
        match start_runtime(&mut s) {
            Ok(()) => {
                if std::env::var("SAVINGS_MIRROR_NO_DASHBOARD").as_deref() != Ok("1") {
                    let url = format!("http://127.0.0.1:{DASHBOARD_PORT}");
                    let _ = open::that(&url);
                }
            }
            Err(e) => log_line(&mut s, &format!("auto-start FAILED: {e}")),
        }
    }

    let menu_channel = MenuEvent::receiver();
    let mut last_poll = Instant::now();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200));

        // Menu events.
        while let Ok(ev) = menu_channel.try_recv() {
            let mut s = state.lock().expect("state mutex");
            if ev.id == start_id {
                if let Err(e) = start_runtime(&mut s) {
                    log_line(&mut s, &format!("start_runtime FAILED: {e}"));
                }
            } else if ev.id == stop_id {
                stop_runtime(&mut s);
            } else if ev.id == dash_id {
                open_dashboard(&mut s);
            } else if ev.id == quit_id {
                stop_runtime(&mut s);
                *control_flow = ControlFlow::Exit;
            }
            status_item.set_text(s.last_status.label());
        }

        // Status poller.
        if last_poll.elapsed() >= POLL_INTERVAL {
            last_poll = Instant::now();
            let mut s = state.lock().expect("state mutex");
            if let Some(new) = poll_status(&mut s) {
                status_item.set_text(new.label());
            }
        }

        if let Event::LoopDestroyed = event {
            let mut s = state.lock().expect("state mutex");
            stop_runtime(&mut s);
        }
    });
}
