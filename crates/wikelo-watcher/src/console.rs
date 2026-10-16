//! The executable is a Windows GUI program (double-click opens the settings window, no
//! console flashes by). Subcommands typed in a terminal still need somewhere to print:
//! we attach to the parent's console and point the standard handles at it, unless the
//! caller already redirected them (pipes from a script, the daemon's log file).

#[cfg(windows)]
pub fn attach_parent() {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetConsoleWindow, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    // SAFETY: plain Win32 calls on this process's own console and standard handles.
    unsafe {
        if GetConsoleWindow() != 0 {
            return;
        }
        let redirected: Vec<(u32, bool)> = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .into_iter()
            .map(|which| {
                let h = GetStdHandle(which);
                (which, h != 0 && h != INVALID_HANDLE_VALUE)
            })
            .collect();
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        for (which, already) in redirected {
            if already {
                continue;
            }
            let name: Vec<u16> = if which == STD_INPUT_HANDLE { "CONIN$" } else { "CONOUT$" }.encode_utf16().chain(std::iter::once(0)).collect();
            let h = CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, 0);
            if h != INVALID_HANDLE_VALUE {
                SetStdHandle(which, h);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn attach_parent() {}

/// Is there a console to read a key press from?
#[cfg(windows)]
pub fn has_console() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    // SAFETY: plain Win32 query.
    unsafe { GetConsoleWindow() != 0 }
}

#[cfg(not(windows))]
pub fn has_console() -> bool {
    true
}

/// Last resort for errors when there is no console at all (double-click).
#[cfg(windows)]
pub fn message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "Wikelist Watcher".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(0, text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(windows))]
pub fn message_box(text: &str) {
    eprintln!("{text}");
}
