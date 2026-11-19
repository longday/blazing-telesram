## Why

Failures in the live client are invisible: errors go to scattered `eprintln!` calls, the app has no log the user can open, and the protocol traffic that explains a failure (which request, which reply, which push) is not recorded anywhere. The user runs the client daily for work, so diagnosing a problem must not need a rebuild or a debugger, and the developer agent must be able to read what the running client did, on demand.

## What Changes

- One structured log for the desktop app and `bt-probe`, replacing ad-hoc `eprintln!` diagnostics; every line has time, level, component and message, plus fields.
- Three outputs of the same records: stderr (the terminal of `./run.sh`), an in-app log window opened from the main interface, and a JSON Lines file under `BT_RUNTIME_DIR/logs/` that tools can read while the app runs.
- A network trace in the log: every HTTP request and WebSocket request with method, endpoint, status and duration; Xiva connection lifecycle; incoming push kinds. Message text and raw frame bodies appear only at the opt-in `trace` level.
- Verbosity set by `BT_LOG` (filter syntax of `tracing-subscriber`), default `info`.
- Secrets never reach any output: cookie values, CSRF tokens and URL queries are never logged.

## Capabilities

### New Capabilities

- `diagnostic-log`: recording the client's activity and protocol traffic, and exposing it in the terminal, an in-app window and a machine-readable file, without leaking secrets.

### Modified Capabilities

None.

## Impact

- New dependencies: `tracing`, `tracing-subscriber` (exact versions pinned in design).
- `crates/yandex-proto`: HTTP, WebSocket and Xiva code emit trace events; `bt-probe` diagnostics go through the log.
- `crates/chat-core`: engine lifecycle, cache and fetch events.
- `crates/desktop`: logger setup in `main.rs`, sign-in events, a log window with a toolbar button and shortcut.
- `.runtime/logs/` holds activity metadata (chat IDs, timestamps, user GUIDs); like the rest of `.runtime/` it stays git-ignored and private (0700/0600).
