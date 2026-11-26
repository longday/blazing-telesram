# Design

## Context

See proposal.md. The branch version (453bb0f) wrapped `Fake` in a `Socket` struct that delegated 5 `Backend` methods. `Backend` now has 16 methods (`backend.rs`), so the wrapper would need 16 delegations and every new method would need another.

## Decisions

- **Extend `Fake`, no wrapper.** Add `socket_addr: Option<SocketAddr>` to `FakeState` (it is `Default`, so existing tests are untouched). `Fake::bootstrap` and `Fake::history` read it (under the lock, copied out before any `.await`) and, when set, do the TCP round trip: connect, write 4 bytes, read them back, assert they match, map I/O errors to `BackendError::Network`. Alternative rejected: the `Socket` wrapper (maintenance cost above).
- **Echo server on a plain `std::thread`** with `std::net::TcpListener`, as on the branch, so the server does not depend on the runtime under test.
- **Test body** as on the branch: bind `127.0.0.1:0`, set `account`, one chat and one message, `ChatClient::spawn`, send `OpenChat`, poll events with a 5 s deadline until both `Connection(Online)` and a `History` chunk with the message have arrived.
- **Dev-dependency `io-util`** in `crates/chat-core/Cargo.toml` for `read_exact`/`write_all`. `net` is already in the normal dependencies.

## Risks / Trade-offs

- Without the I/O driver the engine task panics on the client thread; the test then sees no `Online` event and fails on the 5 s deadline rather than at the panic. Acceptable: the message names the missing events.
- Loopback socket in a unit test: no external network, no fixed port.
