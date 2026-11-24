# Blazing Telesram

Blazing Telesram (БТ) is a native Rust + GPUI desktop client for Yandex Telemost / Messenger chats, styled after Telegram Desktop. It is not affiliated with Telegram or Yandex.

Current status: **read-only**. It shows your chat list, history, image thumbnails and file cards, and receives new messages live. It cannot send messages, mark chats as read or show typing. Chats and history are cached locally, so the last state is visible offline.

> **Risk:** there is no public API. The client speaks the unofficial protocol of the Telemost web client, using your Yandex session cookies. Yandex can change or block it at any time.

## Run

macOS only. Needs rustup and the Xcode Command Line Tools (full Xcode is not required).

```sh
./run.sh
```

`run.sh` installs the pinned toolchain, sets `BT_RUNTIME_DIR` to `.runtime/` in the repository (git-ignored) and starts the app. To run cargo directly, set the variable yourself; the app refuses to start without it:

```sh
BT_RUNTIME_DIR="$PWD/.runtime" cargo run --locked --release -p blazing-telesram
cargo test --locked --workspace
```

## Sign-in

On first start a window with the Yandex sign-in page opens. Sign in as on `telemost.yandex.ru`; the window closes once the session is verified. The session (cookies, account ids) is stored in `.runtime/session.json` with mode 0600, not in the Keychain. Sign out from the app deletes the session, the account cache and previews, but keeps the files you saved.

«Скачать» on a file saves it to `~/Downloads`, never overwrites an existing file and never downloads on its own.

`bt-probe` checks the stored session without the UI and never prints cookie values:

```sh
BT_RUNTIME_DIR="$PWD/.runtime" cargo run --locked -p yandex-proto --bin bt-probe -- me
```

Two more read-only commands inspect the profile and chat member endpoints; both print the parsed reply (`{:#?}`: guids, display names, roles, status text) on stdout, a header line goes to stderr; keep it out of commits and logs:

```sh
bt-probe whoami                                  # the signed-in account: UserInfo and UserStatusInfo
bt-probe members <chat>                          # get_chat_members, first page of 50 users of one chat id
```

`members` answers for group chats only: private chats, the self chat and (for the account it was recorded on) channels are refused with `no_permissions_for_this_chat_operation`. The client has no `get_group_members` call: it answers `bad_request` for every shape.

## Diagnostics

The app logs to three places at once: the terminal (stderr), the log window (⌘⇧L or the 🐞 button next to search; «Копировать» copies it) and a JSON Lines file `.runtime/logs/<UTC time>-<pid>.jsonl`, one object per record, written as it happens. The ten newest files are kept.

`BT_LOG` selects what is logged, in [`tracing-subscriber` `EnvFilter`](https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/filter/struct.EnvFilter.html) syntax; the default is `info`. An invalid value stops the app at startup.

```sh
BT_LOG=debug ./run.sh                                   # every HTTP and socket request with status and timing
BT_LOG=info,yandex_proto::xiva=trace ./run.sh           # plus raw Xiva frames, including message text
jq -c 'select(.level != "DEBUG")' "$(ls -t .runtime/logs/*.jsonl | head -1)"   # read the running app's log
```

No level logs cookie values, tokens or URL query strings; `trace` does log message text, so share trace logs with care. `bt-probe` uses the same `BT_LOG` and writes its log to stderr only, e.g. `BT_LOG=yandex_proto::xiva=trace bt-probe listen --watch <chat>` shows the raw frames; `--watch` adds the per-chat subscription the app makes for the open chat.

## License

Original code is MIT. See [THIRD_PARTY.md](THIRD_PARTY.md) for the separately licensed GPUI input example and the patched dependency copies in `vendor/`.
