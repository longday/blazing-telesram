# Research: add-cross-platform-support

Facts gathered while writing the proposal (2026-11-28, host: source reading and the crates.io API only; nothing was built, no VM was started) and the spikes that must close the open ones before the units that depend on them. Verified means read in the source or in the registry; **[inferred]** means not observed. Every spike runs in a VM under `docs/vm.md` rules (the Linux one needs the VM of S0.1); a spike writes its answer into the results table at the end of this file, one row per question, and never contains a cookie, a token, message text or a person's name.

## Part 1: inventory of what is macOS-only today

"Hard" = the workspace does not compile for that target. "Soft" = compiles, behaves wrongly or shows a macOS word.

### Hard: does not compile on Windows (the three library crates)

| Where | What |
|-------|------|
| `crates/yandex-proto/src/session.rs:4,79-82,101-109` and tests `:135,195` | `std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt}`: `.mode(0o700)` on the runtime directory, `.mode(0o600)` on the session file |
| `crates/yandex-proto/src/bin/bt-probe.rs:1135-1147` (`check_sample_dir`) and `:2277-2280` | `PermissionsExt`: "the sample directory must have mode 0700"; a second use of the same trait |
| `crates/chat-core/src/cache.rs:14,105,111,113` and tests `:568,1059` | `.mode(0o700)` cache folder, `.mode(0o600)` database file, `set_permissions(0o600)` |
| `crates/chat-core/src/client.rs:18,591-606` (`prepare_playback_dir`), `:773` (`let mode = if landing.private { 0o600 } else { 0o666 }`), `:3046-3056` (`store_media`) and tests `:8103,8467-8469,8506,8509,8540,8590-8593,8629,8658` (`mode_of`, `std::os::unix::fs::symlink`, a `0o755` folder through `DirBuilderExt`) | `.mode(0o700)`, `.mode(0o600)`, `permissions().mode() & 0o777`, symlinks and `from_mode` in tests |
| `crates/chat-core/src/paths.rs:6,159-167,191-208` and tests `:355-359,431` | `create_partial_with_mode(.., mode)` -> `OpenOptionsExt::mode`; `commit_partial` claims the final name with `fs::hard_link` (needs a file system with hard links; no copy fallback is allowed, see Part 4) |
| `crates/desktop/src/logging.rs:9,124-125,141-145` and tests `:194,304-305` | `.mode(0o700)` log folder, `set_permissions(0o700)`, `.mode(0o600)` log file |
| `crates/desktop/src/format.rs:60-82` | `extern "C" { fn localtime_r }` with a `Tm` that has `tm_gmtoff`; `c_long` is 32 bit on Windows, and `localtime_r` does not exist there |
| Every other Unix-only test (found by a search for `os::unix`, `PermissionsExt`, `OpenOptionsExt`, `DirBuilderExt`, `MetadataExt`, `symlink(`, `.mode(`, `from_mode`) | `paths.rs:6,355-358,431`, `session.rs:4,135,195`, `cache.rs:14,568,1059`, `logging.rs:9,194,304-305`, `bt-probe.rs:1136,2277-2280`; `main.rs:322-323` passes `/Users/me` to `downloads_dir`, which is not absolute on Windows; `crates/yandex-proto/tests/probe_write_guard.rs` needs `/usr/bin/sandbox-exec` and already skips off macOS (no probe runs on Linux or Windows) |

### Hard: macOS-only crates and modules of `crates/desktop`

| Where | What |
|-------|------|
| `Cargo.toml:9,14,17,19,20` | `block2`, `objc2`, `objc2-app-kit` (features NSApplication, NSDockTile, NSResponder, NSScreen, NSWindow), `objc2-avf-audio`, `objc2-foundation`; none is target-gated, and the code that uses them is not gated either (whether these crates build at all on Linux and Windows is S0.2) |
| `Cargo.toml:13` | `gpui =0.2.2` with `runtime_shaders` ("building Metal shaders ahead needs full Xcode"): irrelevant elsewhere, to be checked that it is inert |
| `Cargo.toml:25` | `wry =0.57.3`, not target-gated: on Linux it links WebKitGTK 4.1 + GTK 3 and needs `libwebkit2gtk-4.1-dev`; on Windows it needs the WebView2 runtime at run time |
| `src/main.rs:2,4` | `mod audio;` and `mod dock;` (Core Audio, Dock tile) |
| `src/main.rs:62,64-66` | `AudioFactory(audio::output)`, `AlertsFactory(|| Platform { badge: dock::badge() })` |
| `src/main.rs:171-176,181,314-333` | `downloads_dir` reads `HOME` (not set on Windows); the tests assume `/Users/me` |
| `src/main.rs:248-272` (`fitted_window_bounds`, `pub(crate)`, also called by `src/login.rs:126`) | `NSScreen`, `NSWindow::contentRectForFrameRect_styleMask`, `MainThreadMarker`: the window size is fitted to the Mac's visible frame |
| `src/audio.rs` (all) | `AVAudioEngine`/`AVAudioFile`/`AVAudioPlayerNode` behind `AudioOutput`; `output()` at `:35`; `unsafe` blocks at `:116,147,172,204,224,234` |
| `src/dock.rs` (all) | `NSDockTile.setBadgeLabel`, tests use `MainThreadMarker` |
| `src/login.rs:143-158,170-192` | `wry::WebViewBuilder::...with_incognito(true).with_url("https://telemost.yandex.ru/").build_as_child(&*window)` on the GPUI window, a 1 s cookie poll of `webview.cookies()`, `set_bounds` on resize |

### Soft: compiles but is wrong off macOS

| Where | What |
|-------|------|
| `src/viewer.rs:105` (`REVEAL_LABEL`), test `:470`, `src/timeline.rs:578`, tests `src/tests.rs:10296` | «Показать в Finder» |
| `src/desktop.rs:1923` | `.font_family("Menlo")` for the log view (no such font on Windows/Linux; the fallback is a proportional font) |
| `src/main.rs:68-76`, `src/input.rs:1028-1040,1040` | `cmd-q` (Quit), `cmd-shift-l` (ToggleLog) have no `ctrl` form; `cmd` is the Windows/Super key elsewhere; `ctrl-cmd-space` (character palette) is a macOS gesture. `cmd-f`/`ctrl-f` and `cmd-a/c/v/x`/`ctrl-a/c/v/x` already come in pairs |
| `src/tests.rs:777,830,1679,1688,1881,1903,1917,2212,2302,2386` and the helper `with_log_shortcut` (`:2824-2847`, callers `:2841,2868,3166,3219`, keystrokes `:2847,2872,3174,3232`) | simulate `cmd-a` and `cmd-shift-l` keystrokes and bind `cmd-shift-l` themselves; they must become `secondary-*` or take the binding from `keys.rs` |
| `run.sh` | `uname -s != Darwin` guard (the other agent adds `run-mac.sh`, `run-linux.sh`, `run-windows.ps1`; they set `BT_RUNTIME_DIR=<repo>/.runtime`) |
| `crates/yandex-proto/src/runtime.rs:11` | the error text names `run.sh` |
| `docs`, specs | `openspec/specs/media-playback` (lines 157, 175), `attachment-download` (`Reveal in Finder`), `diagnostic-log` (lines 61, 64: `Cmd+Shift+L`) |

### Already portable (read, nothing to do)

- `Cargo.lock` is resolved for every target (it contains `gtk`, `webkit2gtk`, `wayland-client`, `x11rb`, `ashpd`, `webview2-com`, `windows`, `schannel`, `openssl-sys`, `native-tls`, `dirs`), so `--locked` works for a Linux or Windows build; the new crates of this change are the only additions.
- `vendor/xattr` (patched for Linux: `ENODATA`), `vendor/block`, `vendor/proc-macro-error2` are in `[patch]`; `xattr` comes through `zed-async-tar` (gpui), and its patch exists exactly because of Linux. `THIRD_PARTY.md` already records them.
- `rusqlite` (`bundled`: a C compiler is needed), `reqwest` and `tokio-tungstenite` with `native-tls` (OpenSSL headers and `pkg-config` on Linux, `schannel` on Windows, Security.framework on macOS).
- `rust-toolchain.toml` pins `1.100.0` for every platform; `rustup` is a prerequisite already stated by the run scripts.
- The clipboard (`cx.write_to_clipboard`), `cx.open_with_system` and `cx.reveal_path` are GPUI calls that exist on all three platforms (below).

## Part 2: what GPUI 0.2.2 gives on Linux and Windows (source at `~/.cargo/registry/src/*/gpui-0.2.2`)

- `src/platform/{mac,linux,windows}` and `blade`. Windows is a native backend: `windows` 0.61, Direct3D 11, DirectWrite, DirectComposition, a manifest through `embed-resource` (default feature `windows-manifest`). The crate README still says "macOS or Linux", so Windows is the least documented target. Default features: `font-kit`, `wayland`, `x11`, `windows-manifest`.
- Linux renders with `blade-graphics` 0.7.1 over Vulkan (`ash`, loaded at run time: a Vulkan driver is required; in a VM that is `mesa-vulkan-drivers` with lavapipe), `calloop`, Wayland (`wayland-client`, dlopen) and X11 (`x11rb`, `xkbcommon`), `ashpd` (portals), `oo7`, `open`, fontconfig by dlopen, `cosmic-text`. `current_platform` picks Wayland when `WAYLAND_DISPLAY` is non-empty, else X11 when `DISPLAY` is set, else a headless platform.
- `open_with_system`: `xdg-open` (Linux), `ShellExecuteW` (Windows). `reveal_path`: Windows `SHOpenFolderAndSelectItems` (selects the file); Linux through the `xdg-desktop-portal` `OpenDirectory` request, falling back to opening the parent folder (it does **not** select the file in the fallback).
- Keystrokes: `secondary-` means `cmd` on macOS and `ctrl` elsewhere (`keystroke.rs:116-143`); `Modifiers::secondary()` is the matching test. On Linux and Windows `cmd` is the Super/Windows key. There is `window.show_character_palette()` (macOS gesture), `start_window_move`, `start_window_resize(edge)`, `window_controls()`, `request_decorations`.
- Decorations: the window asks for server-side decoration (`window.rs:997`); on Wayland the compositor answers `ClientSide` when it has no xdg-decoration server (GNOME's Mutter), and GPUI then draws **nothing**: the application must draw its own title bar and resize edges (`Decorations::Client { tiling }`, `set_client_inset`). On X11 the window manager decorates. Windows decorates natively.
- Fonts: the default family `.SystemUIFont` resolves per platform (`style.rs:406`; DirectWrite on Windows; on Linux and the fallback stack of `text_system.rs:73-84`: `.ZedMono`, `.ZedSans`, `Helvetica`, `Segoe UI`, `Ubuntu`, `Adwaita Sans`, `Cantarell`, `Noto Sans`, `DejaVu Sans`, `Arial`). The app embeds no font, so Cyrillic and colour emoji (reaction chips) depend on installed fonts: Noto/DejaVu and Noto Color Emoji on Linux, Segoe UI and Segoe UI Emoji on Windows.
- `Display::bounds()` is the whole monitor (Windows `rcMonitor`), not the work area: a fitted window must leave a margin for the task bar.
- The window implements `raw_window_handle::HasWindowHandle` on all three platforms (Win32, Wayland, Xlib/Xcb); that is what `wry`'s `build_as_child` consumes.

## Part 3: sign-in web view (`wry` 0.57.3, source read)

- macOS: WKWebView, `build_as_child` on the GPUI window works (today's `login.rs`); incognito = a non-persistent store.
- Windows: WebView2 (`webview2-com` 0.39, `windows` 0.61/0.62). `build_as_child` takes a Win32 HWND; `with_incognito` needs WebView2 runtime >= 101.0.1210.39 (Evergreen is on every Windows 11 and on updated Windows 10). Whether a WebView2 child of a **GPUI DirectComposition** window paints and receives input is **not verified anywhere** (S0.3 on a real Windows machine).
- Linux: WebKitGTK 4.1 (`webkit2gtk =2.0.2`, `gtk 0.18`). `build_as_child` works **only on X11** (an Xlib handle); on Wayland the README requires `build_gtk` on a GTK container. GTK must be initialised and its main loop pumped; `cookies()` itself spins `gtk::main_iteration()` until the answer arrives. GPUI's `calloop` loop does not pump GTK, so a web view inside the GPUI process is not possible on Linux without a second event loop thread that owns GTK, and a GTK widget cannot be a child of a GPUI-owned X window on Wayland at all.
- Therefore (design D6) the sign-in on Linux and Windows runs the web view in its own process with its own top-level window; macOS keeps the child view.
- `tao` (the window library `wry` is built to run on): current 0.38.0 on crates.io; `wry` 0.57.3's dev-dependency is `tao` 0.37, so the compatible `tao` version is fixed at apply time with `cargo tree` in the VM, not guessed.

## Part 4: audio

- The measured record is in the archived `add-media-playback/research.md` R3 to R5 (17 of 17 voice messages are Ogg Opus, mono, 48 kHz; the length is read from the file, `Voice.Duration` is up to 280 ms longer for web-recorded voices; a truncated file reports length 0).
- Candidate **C4** there: `ogg =0.9.2` + `ropus =0.12.18` (BSD-3-Clause, a Rust port of libopus) + `wide` + `safe_arch`: 17 of 17 decode with the same frame counts and SNR as libopus, a 566 s voice in 0.51 s in release, no C compiler or `cmake`. It was kept as the reserve for macOS; here it is the **decoder of Linux and Windows** (the same evidence applies, the decoder is pure Rust).
- **A bare decode loop is wrong at the end of the file** (MP R5): against Core Audio's length it gave **+48 frames** (1 ms) in 7 of the 17 files and **+4368 frames** (91 ms) in sample #10, whose last granule position is shorter than the packets. All 17 samples have pre-skip 0, so the pre-skip rule has **never been exercised** by real data. RFC 7845 §4.2 (pre-skip, output gain) and §4.4 (end trimming by the last page's granule position) define the right length; design D8 and task 4.1 implement them with fixtures for each case, and 0.4 measures them on real files.
- Output: `rodio` 0.23 over `cpal` 0.18.2 with `default-features = false` (no `symphonia`, no `lewton`) and a `SamplesBuffer` of the decoded PCM: a `Sink` gives `pause`, `play`, `stop`, `get_pos`, `empty`, and converts channel count and rate to the device's. `cpal` 0.18.2 uses WASAPI on Windows (`windows` ^0.62) and ALSA on Linux (`alsa` ^0.11, `libasound2-dev` at build time; optional `pulseaudio`/`pipewire` features). Whether `rodio` with no default features still has `SamplesBuffer` and `Sink` and what exactly it pulls in is checked by `cargo tree` in S0.4.
- Not chosen: `symphonia` 0.6.1 + `symphonia-opus` 0.1.3 (published 2026-11-12, 6.5k downloads, one maintainer: too young) and `opus` 0.4.0 (needs `cmake` and a C compiler for the bundled libopus on every machine).
- A VM has no useful sound device ([MP 0.4]: a virtual device exists for macOS; for a Linux VM it is unknown): audible output is a **[needs-user]** check on real hardware; decoding, length, position bookkeeping and the end are checked by tests and, without a device, by the `NoOutput` path.

## Part 5: Windows file system facts (std behaviour, to be confirmed by a test on Windows)

- `fs::rename` over an existing file replaces it (`store_media` relies on it): yes for `std` (MOVEFILE_REPLACE_EXISTING / POSIX semantics).
- `fs::hard_link` needs NTFS or ReFS; FAT/exFAT (a USB Downloads folder) and some network shares refuse it. On Linux: exFAT, vfat, some FUSE and network mounts refuse it. `commit_partial` claims the final name with it (`paths.rs:195`). A copy into the final name is **not** an acceptable fallback (it breaks "Complete or absent": an empty or half-written file under the final name, left behind when interrupted). The atomic no-replace claims are: Windows `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` (works on FAT/exFAT/SMB), Linux `renameat2(RENAME_NOREPLACE)` (not on vfat/exFAT/some NFS, `EINVAL`), then `hard_link`; otherwise the download fails with `Disk`. To confirm in the spikes: 0.2 compiles both calls for their targets; the Linux VM tests `RENAME_NOREPLACE` on tmpfs/ext4 and a loop-mounted vfat image; Windows behaviour on FAT is 8.3.
- Names: `<>:"/\|?*`, control characters, a trailing dot or space, and the reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`, with or without an extension) are not valid; `std` adds the `\\?\` prefix for long absolute paths itself, so `MAX_NAME_BYTES` 200 is safe.
- An open file cannot be deleted (`remove_dir_all` of a playback folder while a video player holds a copy fails): the existing cleanup only logs a warning.
- A directory in `%LOCALAPPDATA%` inherits a "this user, SYSTEM, administrators" ACL, but a repository clone at `C:\src\...` inherits `Users: Modify` from the drive root: `<repo>\.runtime` (what `run-windows.ps1` sets) is **readable by other local users** unless the app restricts it itself. An explicit protected DACL granting only the current user is the equivalent of `0700`/`0600` (D2).

## Part 6: how Linux and Windows get checked from this Mac

- `tart list` on the host: `bt-vm` (macOS), plus the base image. **No Linux VM exists**; `ghcr.io/cirruslabs/ubuntu:latest` (arm64, server, no desktop; user `admin`) is the documented starting point. `docs/vm.md` says an agent never starts or stops a VM, so S0.1 belongs to the main session.
- A Linux arm64 VM builds and tests the whole workspace and runs the GPUI window over `xvfb` (X11) and, with `weston`/`sway` headless, over Wayland, rendering by `lavapipe`; Apple's Virtualization.framework gives Linux guests a virtio GPU, but a Vulkan driver for it is **[inferred]** absent, hence lavapipe.
- Windows: no Windows guest in Tart. The check from the Mac VM is `cargo check` for `x86_64-pc-windows-msvc` through `cargo-xwin` (downloads the MSVC CRT and Windows SDK; needs `llvm`/`clang-cl`; the `gpui` build script `embed-resource` needs a resource compiler) or `x86_64-pc-windows-gnu` (needs `mingw-w64`). Either proves **compilation only**. Running Windows code (ACLs, WebView2, WASAPI, toast, `SHOpenFolderAndSelectItems`) needs a real Windows machine: **[needs-user]**, or a CI job (not done without an explicit request, see `proposal.md`, Open questions).

## Spikes

| ID | Question | Where | Result |
|----|----------|-------|--------|
| S0.1 | Create the Linux VM (`tart clone ghcr.io/cirruslabs/ubuntu:latest bt-vm-linux`, 40 GB disk), install the build and run prerequisites, write them into `docs/vm.md`. | main session, **[needs-user]** for the go-ahead | open |
| S0.2 | The compile-error inventory: on a scratch copy (never merged) cfg out the macOS modules of `desktop` and run `cargo check --workspace` for Linux (S0.1) and `x86_64-pc-windows-{msvc via xwin | gnu}` (bt-vm). Does the list equal Part 1? Does `gpui` with `runtime_shaders` still compile? Does `--locked` hold? | VM | open |
| S0.3 | Sign-in helper: (a) Linux, X11 and Wayland: a `tao` window with an incognito `wry` view loads `http://127.0.0.1:<port>/` (a local test server that sets a cookie named `Session_id`), the helper prints the cookie list to its stdout, the parent reads it; no Yandex account. (b) Windows (real machine, **[needs-user]**): the same, plus does the WebView2 window show Yandex's sign-in page and finish a real sign-in, and does `dumpbin /dependents` show the WebView2 loader linked statically (`webview2-com-sys` ships `WebView2LoaderStatic.lib`; static linking is **[inferred]**). (c) does the macOS in-process path stay as is. | Linux VM; a Windows machine | open |
| S0.4 | Audio: `cargo tree` of `rodio` 0.23 with `default-features = false` (+ the features that keep `Sink`, `SamplesBuffer` and `cpal`); decode with `ogg` + `ropus` the voice samples used in MP R3 (the VM copies are deleted: re-fetch three of them, one `SpeechKit`, one `opusconvertfromwebm`, the 566 s one, with `bt-probe download`, one live slot, reads only) **with the pre-skip and last-granule rules of design D8**, and compare the length with MP R3/R5 (a bare loop is expected +48 / +4368 frames, the rule the afinfo count); record each `OpusHead` (pre-skip, gain, channels); the `NoOutput` path on a VM without a device. Then **[needs-user]**: audible playback on a real Windows and a real Linux. | Linux VM, live slot | open |
| S0.5 | GPUI on Linux: does a window render over lavapipe (X11 and Wayland); which decoration mode a headless Wayland compositor answers; does `cx.reveal_path` and `cx.open_with_system` work; which fonts the fallback stack finds on the Ubuntu image with and without `fonts-noto-color-emoji`; **what exactly GPUI prints and which exit status it gives with `VK_DRIVER_FILES=/nonexistent`** (X11 and Wayland; source: `BladeContext::new()` is unwrapped in `Application::new()` at `wayland/client.rs:501` and `x11/client.rs:414`, so no app-level handler can run). | Linux VM | open |
| S0.6 | Windows ACL: does `D:P(A;OICI;FA;;;OW)` through `SetNamedSecurityInfoW` / `CreateFileW` with `SECURITY_ATTRIBUTES` give `icacls` output with one entry for the owner, for a file and for a directory created inside it, and can the app still read and delete them. `OW` means the owner at check time: **repeat in an elevated process** (is the owner the user or the Administrators group; does the entry then grant every administrator) and, if it does, use the token's user SID instead. | a Windows machine, **[needs-user]** | open |
| S0.7 | Notifications: Linux, a scratch binary posts a notification to a session bus (`dbus-daemon` and a notification daemon such as `dunst`/`mako`) through the candidate crate and reads back the `ActionInvoked` signal on a click; Windows, a toast of an unpackaged app with an AUMID registered under `HKCU` is shown and a click reaches the process, and `ITaskbarList3::SetOverlayIcon` shows a number. No Linux badge (design D10). Which crates, with their licences. | Linux VM; a Windows machine, **[needs-user]** | open |

### Results

(empty; each spike adds one row per answered question, with its date and who ran it)
