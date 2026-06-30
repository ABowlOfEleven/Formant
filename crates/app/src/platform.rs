//! Windows platform integration: single-instance lock + login autostart.

/// True if this is the first running instance. On Windows it holds a named
/// mutex for the process lifetime; a second launch returns false.
pub fn is_first_instance() -> bool {
    #[cfg(windows)]
    {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Foundation::{CloseHandle, GetLastError, BOOL, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;
        let name = HSTRING::from("Local\\FormantSingleInstance");
        // SAFETY: standard named-mutex creation; we intentionally leak the handle
        // on success so the mutex lives until the process exits.
        unsafe {
            match CreateMutexW(None, BOOL(1), PCWSTR(name.as_ptr())) {
                Ok(handle) => {
                    if GetLastError() == ERROR_ALREADY_EXISTS {
                        let _ = CloseHandle(handle);
                        false
                    } else {
                        true
                    }
                }
                Err(_) => true,
            }
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Show a simple informational message box.
pub fn notify(title: &str, message: &str) {
    #[cfg(windows)]
    {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
        let text = HSTRING::from(message);
        let caption = HSTRING::from(title);
        // SAFETY: valid wide strings; no owner window needed for a modal box.
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(caption.as_ptr()), MB_OK | MB_ICONINFORMATION);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (title, message);
    }
}

/// Open a URL (or path) in the user's default handler, e.g. the web browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let verb = HSTRING::from("open");
        let target = HSTRING::from(url);
        // SAFETY: ShellExecuteW with a static "open" verb and a valid wide string;
        // the returned instance handle is informational and ignored.
        unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }
    #[cfg(not(windows))]
    {
        let _ = url;
    }
}

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "Formant";

/// Whether "start Formant with Windows" is currently enabled.
pub fn autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .and_then(|k| k.get_value::<String, _>(RUN_VALUE))
            .is_ok()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Add/remove the login-autostart entry (HKCU\...\Run) pointing at this exe.
pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
        if enabled {
            let exe = std::env::current_exe()?;
            key.set_value(RUN_VALUE, &format!("\"{}\"", exe.display()))?;
        } else {
            let _ = key.delete_value(RUN_VALUE);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Ok(())
    }
}
