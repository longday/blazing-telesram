# Tasks

## 1. Runtime I/O regression test

- [x] 1.1 Port `runtime_supports_socket_io_backends` from 453bb0f into the `client.rs` test module (`socket_addr: Option<SocketAddr>` in `FakeState`, TCP round trip in `Fake::bootstrap` and `Fake::history`, test body and echo thread) and add tokio feature `io-util` to the dev-dependency in `crates/chat-core/Cargo.toml`. Files: `crates/chat-core/src/client.rs`, `crates/chat-core/Cargo.toml`. Verify: `cargo test -p bt-chat-core runtime_supports_socket_io_backends` passes on `main`; with `enable_all()` in `ChatClient::spawn` temporarily replaced by `enable_time()` it fails (revert the probe edit afterwards); `cargo test -p bt-chat-core` stays green and `cargo build --workspace --all-targets` has 0 warnings.

## 2. Cleanup

- [ ] 2.1 After 1.1 is merged into `main`, delete the local branches `omp/task/CoreSync`, `omp/task/DesktopUi`, `omp/task/FrameCodec`, `omp/task/ProtoTrace`, `omp/task/SignInWindow` (`git branch -D`, local only; they are not merged). Verify: `git branch --list 'omp/task/*'` prints nothing.
