# Blazing Telesram

Blazing Telesram (БТ) is a native Rust + GPUI desktop client for Yandex Telemost / Messenger chats, styled after Telegram Desktop. It is not affiliated with Telegram or Yandex.

> **Risk:** there is no public API. The client speaks the unofficial protocol of the Telemost web client, using your Yandex session cookies. Yandex can change or block it at any time.

## Features

- Sign-in through the Yandex web page (two-factor and Yandex 360 steps included); the session is reused on later starts.
- Chat list in a Telegram Desktop style sidebar: avatars, previews, unread counters, pinned and muted markers, filter by name and kind, server search of chats, people and messages.
- History with reply quotes, forwarded-from lines, edited and deleted markers, image thumbnails and galleries with a viewer, file cards.
- Sending text and replies with delivery state, offline queue and per-chat drafts; edit and delete own messages; reactions and the list of who reacted.
- Voice messages played in the timeline, videos in the system player; files saved to `~/Downloads` only on request, with progress, cancel and retry.
- Live updates over the Xiva push channel with automatic reconnect; read markers and typing indicator.
- Offline cache: the last state of chats and history is visible without a network.
- Diagnostic log: terminal, in-app log window and JSON Lines files.

## Run

The app is built from source; there are no automatic builds or ready packages yet (see [TODO](#todo)). You need a Rust development environment: [rustup](https://rustup.rs). The toolchain version is pinned in `rust-toolchain.toml` and installed by the scripts.

| OS | Command | Status |
| --- | --- | --- |
| macOS | `./run-mac.sh` (needs the Xcode Command Line Tools; full Xcode is not required) | works |
| Linux | `./run-linux.sh` | in progress (change `add-cross-platform-support`); not supported yet |
| Windows | `.\run-windows.ps1` | in progress (change `add-cross-platform-support`); not supported yet |

Today the desktop crate depends unconditionally on macOS frameworks (AppKit, AVFAudio, the Dock, an embedded WebKit window for sign-in), so only macOS is supported. The Linux and Windows scripts are the entry points for the port: they check rustup, install the toolchain and run cargo, but the app is not expected to work on those systems until that change lands.

Each script installs the pinned toolchain, sets `BT_RUNTIME_DIR` to `.runtime/` in the repository (git-ignored; session, cache, previews and logs live there) and starts the app in release mode. To run cargo directly, set the variable yourself; the app refuses to start without it:

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

One command changes the real account outside the self chat: `bt-probe create-private <guid> --confirm-create` opens (creating it when missing) the private chat with that user, through the CSRF-guarded `create_private_chat` call, once and never retried. Without `--confirm-create` it prints the target and exits with code 2 without any request. Exit codes: 0 created (the chat id is printed when the reply names one), 1 not sent or rejected (nothing was created), 3 outcome unknown (the chat may exist: check the web client before repeating). The raw reply is logged only at `BT_LOG=yandex_proto::registry=trace`; share it with care.

```sh
bt-probe create-private <guid>                   # refuses: prints the target, sends nothing
bt-probe create-private <guid> --confirm-create  # sends the one request
bt-probe create-chat <title> <guid>...                   # refuses: prints title and participants, sends nothing
bt-probe create-chat <title> <guid>... --confirm-create  # sends the one request
```

`create-chat` creates a group chat through the CSRF-guarded `create_chat` call with the payload the web client's own wizard sends (description empty, not a channel, `is_public: true`, your own guid not listed); that payload was read from the web client's code and has not been confirmed against the server yet. Same rules as `create-private`: once, never retried, no request without `--confirm-create`, the same exit codes; the title must be non-blank and at most 250 characters and at least one distinct guid is required. Participants the server could not add are printed by code; the group exists anyway.

Every `bt-probe` command that writes (`send-self`, `edit-self`, `delete-self`, `react-self`, `typing-self`, `seen`, `typing`, `create-private`, `create-chat`) obeys `BT_WRITE_ONLY_CHAT` like the app: set, it limits writes to the chat it names; a write to another chat, a set but empty value and any creation (a new chat is never the named one) are refused on stderr with exit code 2 before the probe connects or sends anything.

## Diagnostics

The app logs to three places at once: the terminal (stderr), the log window (⌘⇧L or the 🐞 button next to search; «Копировать» copies it) and a JSON Lines file `.runtime/logs/<UTC time>-<pid>.jsonl`, one object per record, written as it happens. The ten newest files are kept.

`BT_LOG` selects what is logged, in [`tracing-subscriber` `EnvFilter`](https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/filter/struct.EnvFilter.html) syntax; the default is `info`. An invalid value stops the app at startup.

```sh
BT_LOG=debug ./run-mac.sh                               # every HTTP and socket request with status and timing
BT_LOG=info,yandex_proto::xiva=trace ./run-mac.sh       # plus raw Xiva frames, including message text
jq -c 'select(.level != "DEBUG")' "$(ls -t .runtime/logs/*.jsonl | head -1)"   # read the running app's log
```

No level logs cookie values, tokens or URL query strings; `trace` does log message text, so share trace logs with care. `bt-probe` uses the same `BT_LOG` and writes its log to stderr only, e.g. `BT_LOG=yandex_proto::xiva=trace bt-probe listen --watch <chat>` shows the raw frames; `--watch` adds the per-chat subscription the app makes for the open chat.

## TODO

- Automatic builds (CI) and ready-to-install packages for macOS, Windows and Linux.

## License

Original code is MIT. See [THIRD_PARTY.md](THIRD_PARTY.md) for the separately licensed GPUI input example and the patched dependency copies in `vendor/`.
