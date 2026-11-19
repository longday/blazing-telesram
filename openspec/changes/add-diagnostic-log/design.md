## Context

See proposal.md – Why. Today diagnostics are `eprintln!` calls in `chat-core/src/client.rs`, `desktop/src/{main,login}.rs` and `bt-probe`; protocol traffic is visible only through `bt-probe listen --raw`, which turns on `XivaConfig::raw` and forwards `XivaEvent::Wire` records. GPUI and wry already log through the `log` crate, which nothing collects.

## Goals / Non-Goals

**Goals:** one logging mechanism for all crates; the user opens the log without a terminal; the developer agent reads a running app's log from a file.

**Non-Goals:** remote log upload, crash reports, a live query endpoint into the running app, log search or filtering inside the window.

## Decisions

### `tracing` facade in every crate, subscriber only in binaries
Libraries (`yandex-proto`, `chat-core`) depend on `tracing =0.1.44` and emit events; `desktop` and `bt-probe` install a `tracing-subscriber =0.3.23` registry (features `fmt`, `json`, `env-filter`, `tracing-log`, `ansi`). `tracing-log` collects GPUI's and wry's `log` records into the same outputs.
Alternative: the `log` crate with `env_logger` – no structured fields, no layered outputs. `bt-probe` makes `tracing-subscriber` a normal dependency of `yandex-proto`; a feature-gated binary would drop the probe from `cargo build --workspace`.

### Three layers over one filter
`EnvFilter` parsed from `BT_LOG` (default `info`) is applied to:
- `fmt` layer → stderr, ANSI only when stderr is a terminal;
- `fmt().json()` layer → file `logs/<UTC yyyymmdd-hhmmss>-<pid>.jsonl`, written through an unbuffered `File` behind a mutex, so each record reaches the OS when produced;
- `LogBuffer` layer (desktop only) → in-memory ring of the last 2000 records for the window.
`bt-probe` installs only the stderr layer: its stdout stays the command's output.

### Targets are crate modules
Records keep the default target (module path, e.g. `yandex_proto::xiva`), so `BT_LOG=info,yandex_proto::xiva=trace` selects one component without a custom naming scheme.

### Network trace placement
- `registry.rs` and `media.rs`: one `debug` event per HTTP exchange: method, host+path (via a helper that drops the query), status, elapsed ms; `trace` adds the registry reply body except the CSRF reply.
- `bootstrap.rs`: `info` per step with status, never bodies (they carry tokens).
- `xiva.rs`: `info` for connecting/subscribed/disconnected with reason and retry delay; `debug` per socket request (method, req_id) and its reply (status, elapsed ms); `debug` per push kind; `trace` for operation frames and decoded frame bodies. Subscribe refresh rounds log one `debug` line with the chat count, not one per chat.
- `chat-core`: `info` for engine start, cache fallback and sign-out; `warn`/`error` for the current `eprintln!` sites; `debug` for history and media fetches.
- `desktop`: sign-in origin changes (`info`), bootstrap outcome, window lifecycle.

### `--raw` replaced by trace level
`XivaConfig::raw`, `WireRecord`, `XivaEvent::Wire` and `bt-probe listen --raw` are removed; `BT_LOG=yandex_proto::xiva=trace bt-probe listen` shows the same frames. One mechanism instead of two.

### Secrets by construction
Events log chosen fields only; URLs pass through the query-dropping helper; HTTP headers, cookies and the CSRF reply are never logged. `SessionCookie` and `LoginCookie` already redact `Debug`. A test runs registry and Xiva flows against fakes with a capturing subscriber at `trace` and asserts the fake cookie and token values never appear.

### Log window
`Desktop` gets a `log_open` flag, toggled by action `ToggleLog` (Cmd+Shift+L) and a sidebar header button. The view is an overlay panel with a `uniform_list` over a snapshot of `LogBuffer`, monospace lines `HH:MM:SS.mmm LEVEL target message fields`, colored by level, scrolled to the bottom on open and on new records, and a «Копировать» button writing all lines to the clipboard. While open, a GPUI task checks the buffer's generation counter every 250 ms and re-renders on change; nothing runs while closed.
Alternative: push notifications from the layer into GPUI – the layer runs on any thread and GPUI entities are main-thread only; polling a counter is simpler and costs nothing when closed.

### Rotation and permissions
At startup `logs/` is created 0700, files 0600; the oldest files beyond the 9 newest are deleted before the new file is created. A failure to create the file logs a `warn` to the other outputs and the app continues: the log must not block reading chats.

## Risks / Trade-offs

- [Trace logs contain private message text] → `trace` is opt-in per run, files stay under the git-ignored owner-only `.runtime/`.
- [Unbuffered file writes cost a syscall per record] → acceptable at `info`/`debug` volumes; `trace` is for short sessions.
- [GPUI/wry `log` output at `info` may be noisy] → filter by target in `BT_LOG`; default stays `info` until observed noise justifies a narrower default.

## Migration Plan

`bt-probe listen --raw` users switch to `BT_LOG=yandex_proto::xiva=trace bt-probe listen`; README documents `BT_LOG` and the log window.
