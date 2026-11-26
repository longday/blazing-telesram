# Proposal

## Why

`ChatClient::spawn` builds its Tokio runtime with `enable_all()` (`crates/chat-core/src/client.rs`, in `spawn`). A runtime without the I/O driver makes the reqwest backend panic with "IO is disabled" (fixed in b91d942). No test protects that call: the `Fake` backend never touches a socket, so replacing `enable_all()` with `enable_time()` leaves the whole suite green. A test that did protect it, `runtime_supports_socket_io_backends`, exists only on the stale local branch `omp/task/CoreSync` (commit 453bb0f) and was never merged.

## What Changes

- Port `runtime_supports_socket_io_backends` into the `client.rs` test module on `main`: a local TCP echo server, and a backend whose `bootstrap` and `history` do a real TCP round trip through the client runtime before answering. The test passes only if the runtime has the I/O driver.
- Add the tokio feature `io-util` to the dev-dependency of `crates/chat-core/Cargo.toml` (the test uses `AsyncReadExt`/`AsyncWriteExt`; `net` is already a normal dependency).
- After the merge, delete the five stale local branches `omp/task/{CoreSync,DesktopUi,FrameCodec,ProtoTrace,SignInWindow}` (local only; the user approved).

No production code and no behavior change. No capability changes, so the change has no spec delta (`skip_specs`).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None.

## Impact

- `crates/chat-core/src/client.rs`: test module only (about 50 lines).
- `crates/chat-core/Cargo.toml`: one dev-dependency feature.
- Core lane file `client.rs`: the task must wait until no other in-flight task edits it (`openspec/changes/DEPENDENCIES.md`, rule 1).
