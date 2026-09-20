//! The clipboard bridge. A default `cargo test` run must never overwrite what the
//! developer last copied, so the only test that really writes the system clipboard is
//! gated behind `MONICA_CLIPBOARD_TEST=1` and reads the value back out of a separate
//! process rather than trusting the call that just made it.

use monica_pass_cli::clipboard::{ClipboardError, MAX_CLIPBOARD_CHARS, copy_text};

#[test]
fn empty_and_over_long_text_are_refused_before_the_os_is_called() {
    assert_eq!(copy_text(""), Err(ClipboardError::Empty));
    assert_eq!(
        copy_text(&"f".repeat(MAX_CLIPBOARD_CHARS + 1)),
        Err(ClipboardError::TooLarge {
            characters: MAX_CLIPBOARD_CHARS + 1
        })
    );
}

#[test]
#[cfg(not(windows))]
fn a_platform_without_a_backend_says_so_instead_of_shelling_out() {
    assert_eq!(
        copy_text("ssh-ed25519 AAAAMOCK"),
        Err(ClipboardError::Unsupported)
    );
}

#[test]
#[cfg(windows)]
fn a_public_key_line_round_trips_through_the_real_clipboard() {
    if std::env::var("MONICA_CLIPBOARD_TEST").as_deref() != Ok("1") {
        eprintln!("set MONICA_CLIPBOARD_TEST=1 to write the real clipboard");
        return;
    }
    // The shape y copies: a public line, and a comment that is not ASCII, since the
    // transfer has to survive UTF-16 on the way in and back out again.
    let text = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMOCKJOESTER 跳板机";
    copy_text(text).expect("the clipboard refused the text");
    assert_eq!(read_clipboard(), Some(text.to_owned()));
}

#[cfg(windows)]
fn read_clipboard() -> Option<String> {
    // A separate process, so nothing in this crate's own memory can vouch for itself.
    // [Console]::OutputEncoding has to be pinned or a redirected PowerShell hands back
    // the OEM codepage and the non-ASCII tail arrives as question marks.
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8;Get-Clipboard",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_owned(),
    )
}
