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
fn the_copied_text_comes_back_out_of_the_real_clipboard() {
    if std::env::var("MONICA_CLIPBOARD_TEST").as_deref() != Ok("1") {
        eprintln!("set MONICA_CLIPBOARD_TEST=1 to write the real clipboard");
        return;
    }
    // The shape y copies: a public line, and a comment that is not ASCII, since the
    // transfer has to survive the platform's own text encoding on the way in and back
    // out again.
    let text = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMOCKJOESTER 跳板机";
    match copy_text(text) {
        Ok(()) => {}
        // Nothing to read back where the machine has none of this platform's helpers.
        // Which helper each platform tries, and that the text is never an argument,
        // stays pinned by the unit tests in `src/clipboard.rs`.
        Err(ClipboardError::Unsupported) => {
            eprintln!("skip: this machine has no clipboard helper");
            return;
        }
        Err(other) => panic!("the clipboard refused the text: {other}"),
    }
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
    text_from(&output)
}

#[cfg(not(windows))]
fn read_clipboard() -> Option<String> {
    // The same rule, with this session's desktop readers, tried in the order the crate
    // tries its writers so the answer comes from the channel actually in use.
    let readers: [(&str, &[&str]); 3] = [
        ("wl-paste", &[][..]),
        ("xclip", &["-o", "-selection", "clipboard"][..]),
        ("xsel", &["--clipboard", "--output"][..]),
    ];
    for (program, args) in readers {
        let Ok(output) = std::process::Command::new(program).args(args).output() else {
            continue;
        };
        if let Some(text) = text_from(&output) {
            return Some(text);
        }
    }
    None
}

fn text_from(output: &std::process::Output) -> Option<String> {
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    (!text.is_empty()).then_some(text)
}
