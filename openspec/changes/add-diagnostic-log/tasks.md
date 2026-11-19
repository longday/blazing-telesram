# Tasks

## 1. Logging core

- [x] 1.1 Add `tracing =0.1.44` to `yandex-proto`, `chat-core`, `desktop` and `tracing-subscriber =0.3.23` (`fmt`, `json`, `env-filter`, `tracing-log`, `ansi`) to `yandex-proto` and `desktop`; verify `cargo build --locked --workspace --all-targets` has 0 warnings
- [x] 1.2 Desktop `logging` module: `BT_LOG` filter (default `info`, invalid → error naming `BT_LOG`), stderr layer, JSONL file layer in `logs/` (0700/0600, keep 10 newest, unbuffered), `LogBuffer` ring layer (2000 records, generation counter); verify unit tests for filter parsing, rotation, permissions, ring capacity and that a written record parses as JSON with `timestamp`, `level`, `target`, `fields`
- [x] 1.3 Install the logger first in `main.rs`; replace every `eprintln!` in `desktop` and `chat-core` with `tracing` events at the level the design assigns; verify `grep eprintln!` finds none except the pre-logger startup error in `main.rs`, and the workspace tests pass

## 2. Network trace

- [x] 2.1 Query-dropping URL helper and `debug` events for registry and media HTTP exchanges (method, host+path, status, elapsed), registry reply bodies at `trace` except CSRF; bootstrap steps at `info` without bodies; verify with existing fake-server tests plus a capture assertion that the history/users request line appears without query or body at `debug`
- [x] 2.2 Xiva events: connection lifecycle at `info`, socket request/reply at `debug` with elapsed time, push kinds at `debug`, operation frames and frame bodies at `trace`, one `debug` line per subscribe round; remove `XivaConfig::raw`, `WireRecord`, `XivaEvent::Wire` and `bt-probe listen --raw`; verify xiva tests updated (the raw-mode test becomes a trace-capture test) and pass
- [x] 2.3 `bt-probe` installs the stderr logger with `BT_LOG`; its diagnostics go through `tracing`, its command output stays on stdout; verify `BT_LOG=yandex_proto=debug bt-probe me` prints request lines on stderr and the identity on stdout
- [x] 2.5 Dependency trace ceiling (`yandex_proto::log::dependency_cap`) in both binaries; verify a real local WebSocket handshake under `BT_LOG=trace` through the `log` bridge leaks neither the fake cookie nor the URL query, and the test fails with the cap removed
- [x] 2.4 Secret test: run registry, bootstrap and Xiva flows against fakes under a capturing `trace` subscriber and assert fake cookie, CSRF token and URL query values never appear; verify it passes

## 3. Log window

- [x] 3.1 `ToggleLog` action (Cmd+Shift+L), sidebar header button, overlay with level-colored monospace lines, bottom-anchored list, «Копировать» to clipboard, 250 ms generation poll while open; the chat view takes focus on open so window shortcuts resolve; verify headless GPUI test: shortcut opens it, a new record appears while open, copy puts the lines on the clipboard, «Закрыть» hides it
- [x] 3.2 README: `BT_LOG` syntax and examples, log file location, the window shortcut, `--raw` replacement; verify the documented `bt-probe` commands run as written (the `./run.sh` lines are exercised by 4.1)

## 4. Integration

- [ ] 4.1 Live run with the stored session: `./run.sh` with `BT_LOG=debug` produces stderr lines and a growing JSONL file readable by another process while the app runs, with no cookie value present (scan the file for every stored cookie value without printing them)
