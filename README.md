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

On first start a window with the Yandex sign-in page opens. Sign in as on `telemost.yandex.ru`; the window closes once the session is verified. The session (cookies, account ids) is stored in `.runtime/session.json` with mode 0600, not in the Keychain. Sign out from the app deletes the session, the account cache and downloaded media.

`bt-probe` checks the stored session without the UI and never prints cookie values:

```sh
BT_RUNTIME_DIR="$PWD/.runtime" cargo run --locked -p yandex-proto --bin bt-probe -- me
```

## License

Original code is MIT. See [THIRD_PARTY.md](THIRD_PARTY.md) for the separately licensed GPUI input example and xattr compatibility copy.
