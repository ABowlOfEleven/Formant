//! Lightweight file logging and a crash handler.
//!
//! Release builds run with `windows_subsystem = "windows"`, so there is no
//! console and an unhandled panic would vanish without a trace. This writes a
//! rolling log to `%APPDATA%\Formant\log.txt` and installs a panic hook that
//! records the panic (with a backtrace) and shows the user a dialog pointing at
//! the log.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();

/// `%APPDATA%\Formant\log.txt`.
fn log_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Formant").join("log.txt"))
}

/// Open the log (rotating an oversized one aside) and install the panic hook.
pub fn init() {
    let file = open_log();
    let _ = LOG.set(Mutex::new(file));
    install_panic_hook();
    line(&format!(
        "Formant {} started ({})",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) { "debug" } else { "release" }
    ));
}

fn open_log() -> Option<File> {
    let path = log_path()?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Keep the log from growing without bound: once past ~256 KB, roll it aside.
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 256 * 1024 {
            let _ = std::fs::rename(&path, path.with_extension("old.txt"));
        }
    }
    OpenOptions::new().create(true).append(true).open(&path).ok()
}

/// Append a timestamped line to the log (and to stderr in debug builds).
pub fn line(msg: &str) {
    let stamped = format!("{} {}\n", timestamp(), msg);
    #[cfg(debug_assertions)]
    eprint!("{stamped}");
    if let Some(lock) = LOG.get() {
        if let Ok(mut guard) = lock.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = file.write_all(stamped.as_bytes());
                let _ = file.flush();
            }
        }
    }
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture();
        line(&format!("PANIC at {location}: {payload}\n{backtrace}"));
        crash_dialog(&payload, &location);
        previous(info);
    }));
}

/// A short timestamp, local time where available.
fn timestamp() -> String {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::GetLocalTime;
        // SAFETY: GetLocalTime reads the system clock and returns a SYSTEMTIME.
        let st = unsafe { GetLocalTime() };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
    }
    #[cfg(not(windows))]
    {
        String::from("--")
    }
}

/// Tell the user the app crashed and where the log is.
fn crash_dialog(payload: &str, location: &str) {
    #[cfg(windows)]
    {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let where_log = log_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "the Formant config folder".to_string());
        let text = HSTRING::from(format!(
            "Formant hit an unexpected error and needs to close.\n\n{payload}\n(at {location})\n\nA log was saved to:\n{where_log}"
        ));
        let caption = HSTRING::from("Formant crashed");
        // SAFETY: valid wide strings; no window handle needed for a modal box.
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(caption.as_ptr()), MB_OK | MB_ICONERROR);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (payload, location);
    }
}
