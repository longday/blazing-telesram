# Blazing Telesram

Blazing Telesram (БТ) is a native Rust + GPUI desktop chat prototype. It runs offline with synthetic data. No WebView, login, or connection to Yandex is included. This is an independent prototype, not affiliated with Telegram or Yandex.

- List, search and filter personal chats, groups, channels and mock meetings.
- Browse a virtualized timeline with variable-height messages and load older pages.
- Send local messages, open reply threads, and comment on mock channel posts.
- Set `BT_STATE_PATH` to persist sent mock messages and replies in a local JSON file. Otherwise they are held in memory only.

## Run

Use a Rust toolchain that supports edition 2024 and the [GPUI platform dependencies](https://crates.io/crates/gpui/0.2.2). Linux also needs a working Vulkan driver and the X11/XKB development libraries (on Debian/Ubuntu: `libxcb1-dev`, `libxkbcommon-dev`, and `libxkbcommon-x11-dev`). Other platforms have not been verified for this prototype.

```sh
cargo run --locked -p blazing-telesram
cargo test --locked --workspace
```

## MCP contract experiment

```sh
cargo run --locked -p bt-chat-core --bin telemost-mock-check
```

This reads local JSON-RPC fixtures for chat list, history and threads. Tool names come from the [Telemost MCP catalog](https://yandex.ru/dev/yandex-360/mcp/ru/telemost), but argument and response schemas are synthetic and have not been validated against the live API. The experiment makes no network calls. The desktop UI uses a separate local chat adapter.

## License

Original code is MIT. See [THIRD_PARTY.md](THIRD_PARTY.md) for the separately licensed GPUI input example and xattr compatibility copy.
