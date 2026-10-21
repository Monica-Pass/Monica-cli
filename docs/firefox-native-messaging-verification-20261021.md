# Firefox Native Messaging verification — 2026-10-21

CLI version remains 1.0.101, protocol 2, integration revision 2. The target is
desktop Firefox 153+ with add-on ID `monica@monica-pass.github.io`.

Firefox's documented startup arguments are the complete native manifest path
followed by the add-on ID. This matches Mozilla's subprocess arguments
`[hostInfo.path, context.extension.id]` in
[NativeMessaging.sys.mjs](https://github.com/mozilla-firefox/firefox/blob/main/toolkit/components/extensions/NativeMessaging.sys.mjs)
and [MDN Native messaging](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Native_messaging#connection-based_messaging).

The helper accepts that exact ID after an absolute path, which remains launch
context only. Chromium still requires its existing exact origin syntax.
Firefox's installer manifest has exactly one `allowed_extensions` entry and
uses `HKCU:\Software\Mozilla\NativeMessagingHosts\app.monica.cli`. Each browser
has a separate manifest. IDs scope uninstall; no IDs retains all-browser
uninstall. Diagnostics remain read-only and report the supported Firefox ID.

Validation on Windows GNU with Rust 1.97.0:

- Format, strict all-target Clippy, the two native-host startup unit tests and
  release native-host build passed. Rejected cases include other/case-changed
  IDs, relative manifest paths, mixed Chromium/Firefox arguments and extra
  arguments. The preceding full suite passed 372 tests, 0 failed, 3 ignored;
  these two new binary tests are additional. No library or vault logic changed
  for Firefox.
- Release executable diagnostics and real framed hello passed for both
  Chromium and Firefox argument layouts. Invalid callers exited unsuccessfully
  without protocol output; neither startup path created configuration state.
- `scripts/test-browser-installer.ps1` passed in Windows PowerShell 5.1. It
  remaps the process's HKCU drive to a unique test subtree and uses a temporary
  portable helper. Real browser registrations are never touched. The probe
  checks three independent manifests, exact Firefox allowlisting, invalid ID
  rejection, diagnostic detection of a tampered Firefox manifest, preserving
  the other manifests during individual installs/updates, Firefox-only removal
  preserving Edge/Chrome, and all-browser removal. Temporary files and the
  test registry subtree are removed afterward.

CI runs the executable smoke on Linux/Windows, and the isolated installer probe
on Windows before packaging. Actual Firefox browser/extension acceptance is
owned and recorded by the extension repository; these CLI startup and installer
probes do not establish real browser UI acceptance. Other operating systems
still require manual registration. Rust builds continue using the existing
E: target cache through the repository junction.
