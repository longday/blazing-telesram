# Proposal: add-cross-platform-support

## Why

The client runs only on macOS. The user wants it to run **from source on Windows, macOS and Linux** (`run-mac.sh`, `run-linux.sh`, `run-windows.ps1` are being added by another change set; they only start `cargo run`). Today the workspace does not even compile off macOS: the three library crates use `std::os::unix`, the desktop crate links `objc2`/AppKit/Core Audio unconditionally, and sign-in is a `wry` view embedded in the GPUI window that only works on macOS and, on Linux, only under X11. `research.md` Part 1 lists every place with `file:line`.

GPUI 0.2.2 has Linux (Wayland, X11) and Windows (Direct3D 11) backends and `wry` has WebKitGTK and WebView2 backends, so the platform is reachable; the cost is in the pieces that talk to the OS directly: private file storage, sign-in, audio, notifications, window chrome, shortcuts and a few macOS words.

## What Changes

- **Private storage on every OS.** A new small crate `crates/platform` (package `bt-platform`) replaces the scattered `mode(0o700)`/`mode(0o600)` calls of `yandex-proto`, `chat-core` and `desktop` with one API: a private directory and a private file that exist owner-only from the first instant (Unix modes as today; on Windows a protected DACL granting only the current user). Behaviour on macOS is unchanged.
- **Paths and names.** The Downloads folder comes from the platform (known folder on Windows, `xdg-user-dirs` on Linux, `~/Downloads` on macOS as today). Saved file names additionally avoid what Windows forbids (reserved device names, `<>:"|?*`, trailing dots and spaces). The final name is still claimed atomically (Windows `MoveFileExW` without replace, Linux `renameat2(RENAME_NOREPLACE)` then `hard_link`); where that is impossible the download fails with a disk reason, never a copy into the final name. The local UTC offset works on Windows.
- **Sign-in.** macOS keeps the embedded child view. On Linux and Windows the web view runs in a **helper process** (the same binary started with a flag) with its own top-level window; it hands the cookies to the app over a pipe, and the app verifies and stores the session exactly as today. The app shows a small waiting window with Cancel.
- **Voice playback.** On Linux and Windows `AudioOutput` is implemented with the pure-Rust Opus decoder already measured in the media-playback research (`ogg` + `ropus`) and `rodio`/`cpal` for output. macOS keeps Core Audio.
- **Shortcuts, window and fonts.** Standard shortcuts use Ctrl where macOS uses Cmd (including Quit and the log toggle); the window is fitted to the display without AppKit; the log view uses a monospace font that exists on the platform; on Linux sessions where the compositor does not draw window decorations, the app draws a title bar with move, resize, minimise, maximise and close.
- **Platform words.** «Показать в Finder» becomes «Показать в Проводнике» on Windows and «Показать в папке» on Linux.
- **Alerts.** Notifications of incoming messages (the `add-notifications-and-badge` behaviour) on Windows and Linux, and an unread badge where the OS has one (Windows task bar overlay, Linux launcher entry): units that start only after NB-N is on `main`.
- **Build prerequisites and verification.** A `docs/platforms.md` that lists, per OS, what must be installed before `run-*.sh`/`.ps1` can build and start the app, the Linux VM of the verification plan, and the checks.
- **No behaviour change on macOS.** Every existing test passes unchanged except where a test names a shortcut or the reveal label and is rewritten to be platform-neutral.

## Capabilities

### New Capabilities

- `cross-platform-runtime`: the client starts and works on macOS, Linux and Windows with the same behaviour; what must be the same everywhere (private local storage, the Downloads folder, safe file names, local time, shortcuts, window placement and window management, voice playback, a clear message when a prerequisite is missing) and the stated per-platform differences.
- `platform-alerts`: incoming-message notifications and the unread badge on Windows and Linux.

### Modified Capabilities

- `account-session`: «Sign in through an embedded web page» is reworded: the sign-in window is a separate top-level window on Linux and Windows, its Cancel and failure outcomes.
- `attachment-download`: «Destination and file name» (names valid on the host file system) and «Reveal in Finder» renamed «Reveal in the file manager» (the label follows the platform).
- `diagnostic-log`: «In-app log window» (the shortcut follows the platform's primary modifier).
- `media-playback`: «Playback copy» (the reveal label word).

## Impact

- New crate `crates/platform` (workspace member, depends on `libc` on Unix and `windows` on Windows). New dependencies, target-gated: `dirs` (Linux, Windows), `ogg`, `ropus`, `rodio`, `cpal` (Linux, Windows), `tao` (Linux, Windows; version fixed by the spike), notification crates (units XP-N, chosen by the spike), `windows` (Windows only; the version GPUI already resolves). `block2`, `objc2*` become macOS-only. `Cargo.lock` changes; `THIRD_PARTY.md` gets one line per new licence family.
- Code: `yandex-proto` (`session.rs`, `bin/bt-probe.rs`, `runtime.rs`), `chat-core` (`cache.rs`, `client.rs`, `paths.rs`), `desktop` (`main.rs`, `login.rs`, `format.rs`, `logging.rs`, `viewer.rs`, `timeline.rs`, `desktop.rs`, `input.rs`, `tests.rs`, new `signin_*.rs`, `audio_*.rs`, `window_chrome.rs`, and the alert backends), `docs/`. The exact ownership and the overlap with open work are in `tasks.md`.
- Specs: two new, four modified (this change's deltas).
- Dependencies of the plan: NB-B is on `main`; `add-text-selection-and-quotes` is a proposal on `main` (no code; it already uses `secondary-`); NB-N (open) and the proposal `add-user-profile-card` (open, in another worktree) share files with several units; rule 1 of `DEPENDENCIES.md` sequences them.
- Verification: macOS in `bt-vm` as today; Linux in a new Linux VM; Windows by a cross-target `cargo check` from the macOS VM plus a hand check on a real Windows machine (needs the user).

## Non-goals

- Packaging and installers: `.app` bundle, `.msi`/`.exe`, `.deb`/`.AppImage`/Flatpak, code signing, auto-update. The app runs from source only. (A `.desktop` file, an AUMID shortcut and a bundle identifier matter for notifications; the plan below makes notifications work without them where the OS allows and says so where it does not.)
- A CI workflow: not touched without an explicit request (open question Q1).
- Changing how any wire call works, any request, any UI layout, theme or string except the platform words named above.
- ARM Windows, 32-bit targets, BSDs, X11-less/Wayland-less Linux (a TTY), Linux without a Vulkan driver (`gpui` renders with Vulkan; there is no software fallback in this change).
- A cookie-paste sign-in (copying `Session_id` out of a browser): it would put the credential in the clipboard and the shell history; not offered (open question Q4).
- Mark-of-the-Web (`Zone.Identifier`) on Windows downloads, antivirus scanning of saved files, drag and drop.
- Localised system dialogs; the app's language stays Russian.

## Open questions

- **Q1. CI.** Real Windows (and a second Linux) verification is cheapest as a GitHub Actions matrix job (`cargo check`/`test` on `windows-latest`, `ubuntu-latest`, `macos-latest`). The project rule is "CI only on an explicit request". Do you want it? Default: **no**; Windows then gets compile checks from the macOS VM plus a hand check by you.
- **Q2. Who runs it on real Windows?** Tasks marked `[needs-user]` need a Windows 10/11 x64 machine with WebView2 (sign-in, ACLs, audible voice, toast, task bar badge, Explorer reveal). Default: one hand check at the end, with a script of steps in `docs/platforms.md`.
- **Q3. Linux distributions and sessions to claim.** Default: Ubuntu 24.04 (GNOME Wayland and X11) is the one that is verified; Fedora/Arch/KDE are not tested and the doc says so.
- **Q4. Sign-in without a web view.** If the web view cannot be created (no WebKitGTK package, no WebView2 runtime) the app shows what to install and quits. Default: no paste fallback.
- **Q5. Notifications and badges scope.** Windows toasts of an unpackaged app need an Application User Model ID registered by a Start-menu shortcut; the plan registers it at run time under `HKCU` without a shortcut if the spike shows that works, else the notification unit ships for Linux only and Windows toasts wait for packaging. The badge on GNOME has no standard API at all. Default: notifications on both where they work, a badge on Windows (overlay icon) and on Linux launchers that implement the Unity `LauncherEntry` interface, nothing on stock GNOME; cut XP-N and XP-B if you want less.
- **Q6. Quit shortcut on Windows.** `Ctrl+Q` is not a Windows convention; the window close button and `Alt+F4` work. Default: bind `Ctrl+Q` on Linux only.
- **Q7. Audio output without a device.** The existing `NoOutput` message is reused; no separate Linux message about missing ALSA/PulseAudio.

## Decisions taken without asking

(Each is in `design.md` with its reason; change any of them here and the design follows.)

- One change, merged as several units; none is merged with a stub or a no-op arm; backends of a platform are compiled only on that platform and their pure logic is compiled on macOS too under `cfg(test)` so the macOS gate exercises it.
- `crates/platform` is the one place for OS-specific file-system code; `std::os::unix` and `windows` calls appear nowhere else outside the desktop backends.
- Windows private storage by an explicit protected DACL at creation, not by trusting the parent's inheritance.
- Sign-in by a helper process on Linux and Windows; the in-process view stays on macOS (no regression risk there).
- The Opus decoder is `ogg` + `ropus` (measured in MP R3 to R5), not `libopus`/`cmake`, not `symphonia`.
- `cargo test --workspace` stays the macOS gate in `bt-vm`; Linux has its own gate in the Linux VM; Windows has `cargo check` only plus the hand check.
- The unread badge on macOS is not touched; on other platforms it is a separate, optional unit.
