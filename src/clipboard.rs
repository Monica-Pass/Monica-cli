//! Clipboard bridge for the TUI. Only fields the vault already published as public
//! metadata reach this module: an SSH public key line, a key fingerprint, a GPG user
//! id. Private key material and gateway tokens have no code path here, and nothing in
//! this module reads the vault or decrypts a payload.
use thiserror::Error;

/// Longest text accepted. A public key line, a fingerprint and a user id all stay far
/// below this, so a longer value means a whole key ring was handed over by mistake.
pub const MAX_CLIPBOARD_CHARS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ClipboardError {
    #[error("another window holds the clipboard, try again in a moment")]
    Busy,
    #[error("nothing to copy")]
    Empty,
    #[error("the text is too long for the clipboard ({characters} characters)")]
    TooLarge { characters: usize },
    #[error("clipboard rejected the text (os error {code})")]
    Os { code: u32 },
    #[error("copying to the clipboard is not implemented on this platform")]
    Unsupported,
}

/// Replaces the clipboard text. Callers pass an explicit public field.
pub fn copy_text(text: &str) -> Result<(), ClipboardError> {
    let characters = text.chars().count();
    if characters == 0 {
        return Err(ClipboardError::Empty);
    }
    if characters > MAX_CLIPBOARD_CHARS {
        return Err(ClipboardError::TooLarge { characters });
    }
    copy(text)
}

#[cfg(windows)]
fn copy(text: &str) -> Result<(), ClipboardError> {
    use std::ptr;
    use std::thread::sleep;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_BUSY, GetLastError, HWND, WIN32_ERROR,
    };
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, OpenClipboard};

    // A process that is already holding the clipboard answers with one of these two;
    // anything else is a real failure and should not be retried.
    const CONTENTION: [WIN32_ERROR; 2] = [ERROR_ACCESS_DENIED, ERROR_BUSY];
    const RETRIES: usize = 8;
    const RETRY_WAIT: Duration = Duration::from_millis(5);

    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    for _ in 0..RETRIES {
        // SAFETY: a null window handle means the clipboard is owned by this thread
        // rather than a window, which is the documented shape for a console process.
        let opened = unsafe { OpenClipboard(ptr::null_mut::<std::ffi::c_void>() as HWND) };
        if opened != 0 {
            let result = write_text(&units);
            // SAFETY: every path that gets here opened the clipboard exactly once and
            // has not closed it yet.
            unsafe { CloseClipboard() };
            return result;
        }
        // SAFETY: reads this thread's last error code; no memory is dereferenced.
        let code = unsafe { GetLastError() };
        if !CONTENTION.contains(&code) {
            return Err(ClipboardError::Os { code });
        }
        sleep(RETRY_WAIT);
    }
    Err(ClipboardError::Busy)
}

#[cfg(windows)]
fn write_text(units: &[u16]) -> Result<(), ClipboardError> {
    use std::mem::size_of_val;
    use std::ptr;
    use windows_sys::Win32::Foundation::{GetLastError, GlobalFree};
    use windows_sys::Win32::System::DataExchange::{EmptyClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };

    // CF_UNICODETEXT from winuser.h. windows-sys 0.61 only publishes the constant behind
    // the Win32_System_Ole feature, which would pull the whole ole module into the binary.
    const CF_UNICODETEXT: u32 = 13;

    let size = size_of_val(units);
    // SAFETY: the clipboard is open on this thread. `units` stays alive for the whole
    // block and `size` bytes are copied into it. The allocated block is transferred to
    // the clipboard on success and freed here on every failure path, never both.
    unsafe {
        if EmptyClipboard() == 0 {
            return Err(ClipboardError::Os {
                code: GetLastError(),
            });
        }
        let memory = GlobalAlloc(GMEM_MOVEABLE, size);
        if memory.is_null() {
            return Err(ClipboardError::Os {
                code: GetLastError(),
            });
        }
        let destination = GlobalLock(memory);
        if destination.is_null() {
            let code = GetLastError();
            GlobalFree(memory);
            return Err(ClipboardError::Os { code });
        }
        ptr::copy_nonoverlapping(units.as_ptr() as *const u8, destination as *mut u8, size);
        GlobalUnlock(memory);
        if SetClipboardData(CF_UNICODETEXT, memory).is_null() {
            let code = GetLastError();
            GlobalFree(memory);
            return Err(ClipboardError::Os { code });
        }
        // On success the clipboard owns the block: freeing it here would corrupt it.
        Ok(())
    }
}

#[cfg(not(windows))]
fn copy(_text: &str) -> Result<(), ClipboardError> {
    Err(ClipboardError::Unsupported)
}
