//! Run app work on the main thread (20 Nov crash).
//!
//! The app died twice after hours of running ("Illegal Instruction", 0xc000001d). Windows' crash
//! spot was Tauri's AppHandle clone: one of its reference counts had gone bad. Tauri's handles
//! hold a counter that is only safe to touch on the main thread (tauri issue #15408), and the
//! client bars (every 150 ms), the previews watch (every 80 ms) and the activity feed (every
//! 250 ms) were cloning windows and handles on their own threads all day. Two threads touching
//! that counter at the same moment corrupt it, and much later the app crashes.
//!
//! So the busy loops do their Tauri work here: the closure runs on the main thread with the
//! app's one handle (kept here, never cloned from another thread), and the loop waits for it.

use std::sync::OnceLock;
use std::time::Duration;

use tauri::AppHandle;

static APP: OnceLock<AppHandle> = OnceLock::new();

/// Keep the app's handle (call once, from setup, on the main thread).
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// Run `f` on the main thread and wait for its answer (None if the app is closing, or the main
/// thread didn't get to it within 5 s). Don't hold a lock the main thread may need while waiting.
pub fn run<T: Send + 'static>(f: impl FnOnce(&AppHandle) -> T + Send + 'static) -> Option<T> {
    let app = APP.get()?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        if let Some(app) = APP.get() {
            let _ = tx.send(f(app));
        }
    })
    .ok()?;
    rx.recv_timeout(Duration::from_secs(5)).ok()
}

/// Write a panic's message and place to the log before the app goes (it used to vanish).
pub fn log_panics() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log::error!(
            "PANIC on thread {}: {info}\n{}",
            thread.name().unwrap_or("unnamed"),
            std::backtrace::Backtrace::force_capture()
        );
        log::logger().flush();
        before(info);
    }));
}
