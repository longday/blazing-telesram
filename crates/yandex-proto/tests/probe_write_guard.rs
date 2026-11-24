//! Command-path regression for `BT_WRITE_ONLY_CHAT` in `bt-probe`: the real binary, a dummy
//! session and a loopback listener named in every proxy variable. A refused write must exit with
//! code 2 and the guard's message and print nothing on stdout. If a future edit moved the session
//! load, the client, the CSRF request or the socket ahead of the guard, the exit code (a real
//! attempt ends in 1) or the message would change; that is the primary signal.
//!
//! The guarantee that no real host is contacted comes from the OS, not from the proxy variables:
//! every `bt-probe` subprocess runs under macOS `sandbox-exec` with a profile that denies all
//! network access except loopback. The HTTP requests (registry, bootstrap, the CSRF exchange, all
//! `reqwest`, which honours the proxy variables) land on the loopback sentinel, which counts them.
//! The push socket does not: `ws.rs` connects with `tokio_tungstenite::connect_async_tls_with_config`,
//! which ignores the environment and would connect directly. Under the sandbox such a connect
//! (or any other non-loopback one) fails with EPERM before a packet leaves the machine, so a
//! regression that put a socket write ahead of the guard shows up as a changed exit code or
//! message, never as traffic to the real push host.
//!
//! Without `sandbox-exec` (non-macOS) the tests do not run the probe unsandboxed: each one prints
//! a skip message and returns. On macOS a `sandbox-exec` that exists but cannot apply the profile
//! (for instance when the tests already run inside another sandbox) fails the suite loudly.

use std::io::ErrorKind;
use std::net::TcpListener;
use std::process::{Command, Output};
use std::sync::LazyLock;

use yandex_proto::runtime::RuntimeDir;
use yandex_proto::session::{Identity, Session, SessionCookie};

const GUID: &str = "c9a71201-70b5-992e-61f2-b27e2d3fc8fb";
const OTHER_CHAT: &str = "0/0/some-other-chat";

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
/// Everything allowed except network access, which is allowed to loopback only. The later rule
/// wins, so a connect to any non-loopback address fails with EPERM before leaving the machine.
const SANDBOX_PROFILE: &str = r#"(version 1)
(allow default)
(deny network*)
(allow network* (remote ip "localhost:*"))"#;

/// Whether `sandbox-exec` exists. One that exists but cannot apply the profile panics instead of
/// silently dropping the guard coverage.
static SANDBOX_AVAILABLE: LazyLock<bool> = LazyLock::new(|| {
    match Command::new(SANDBOX_EXEC).args(["-p", SANDBOX_PROFILE, "/usr/bin/true"]).output() {
        Ok(out) => {
            assert!(
                out.status.success(),
                "{SANDBOX_EXEC} cannot apply the loopback-only profile (already inside a sandbox?): {}",
                String::from_utf8_lossy(&out.stderr)
            );
            true
        }
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(e) => panic!("{SANDBOX_EXEC} failed to start: {e}"),
    }
});

/// Whether probe subprocesses can run sandboxed. `false` (with a message) only when
/// `sandbox-exec` does not exist, i.e. off macOS: the probe is never run unsandboxed, because a
/// guard regression could then open a direct socket to a real host.
fn sandbox_ready() -> bool {
    let ready = *SANDBOX_AVAILABLE;
    if !ready {
        eprintln!("SKIPPED: {SANDBOX_EXEC} is unavailable; the probe is not run without the network sandbox");
    }
    ready
}

fn dummy_session(dir: &RuntimeDir) {
    Session {
        cookies: vec![SessionCookie {
            name: "Session_id".into(),
            value: "dummy-not-a-real-cookie".into(),
            domain: ".yandex.ru".into(),
            path: "/".into(),
            expires_unix: None,
            secure: true,
            http_only: true,
        }],
        identity: Identity { uid: Some("1".into()), guid: GUID.into(), display_name: None },
    }
    .save(dir)
    .unwrap();
}

struct Run {
    output: Output,
    connections: usize,
}

impl Run {
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    fn assert_refused(&self, args: &[&str], needle: &str) {
        let stderr = self.stderr();
        assert_eq!(self.output.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("BT_WRITE_ONLY_CHAT") && stderr.contains(needle), "{args:?}: {stderr}");
        assert!(self.output.stdout.is_empty(), "{args:?}: {}", String::from_utf8_lossy(&self.output.stdout));
        assert_eq!(self.connections, 0, "{args:?}: the probe sent an HTTP request through the proxy");
    }
}

/// Runs `bt-probe args` under the loopback-only sandbox with `BT_WRITE_ONLY_CHAT=limit`. Every
/// proxy variable points at a loopback listener, which counts the HTTP connections the run opens
/// (not the push socket, which the sandbox would refuse).
fn probe(args: &[&str], limit: &str, with_session: bool) -> Run {
    let sentinel = TcpListener::bind("127.0.0.1:0").unwrap();
    sentinel.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", sentinel.local_addr().unwrap());
    let runtime = tempfile::tempdir().unwrap();
    if with_session {
        dummy_session(&RuntimeDir::new(runtime.path()));
    }
    let output = Command::new(SANDBOX_EXEC)
        .args(["-p", SANDBOX_PROFILE, env!("CARGO_BIN_EXE_bt-probe")])
        .args(args)
        .env_clear()
        .env("BT_RUNTIME_DIR", runtime.path())
        .env("BT_WRITE_ONLY_CHAT", limit)
        .env("HTTP_PROXY", &proxy)
        .env("HTTPS_PROXY", &proxy)
        .env("ALL_PROXY", &proxy)
        .env("http_proxy", &proxy)
        .env("https_proxy", &proxy)
        .env("all_proxy", &proxy)
        .output()
        .unwrap();
    let mut connections = 0;
    while sentinel.accept().is_ok() {
        connections += 1;
    }
    Run { output, connections }
}

fn own_chat() -> String {
    format!("{GUID}_{GUID}")
}

#[test]
fn self_writes_to_a_chat_the_limit_does_not_name_are_refused_before_any_connection() {
    if !sandbox_ready() {
        return;
    }
    let commands: [&[&str]; 6] = [
        &["react-self", "5", "100101"],
        &["react-self", "5", "remove"],
        &["send-self", "hello"],
        &["edit-self", "5", "hello"],
        &["delete-self", "5"],
        &["typing-self"],
    ];
    for args in commands {
        probe(args, OTHER_CHAT, true).assert_refused(args, OTHER_CHAT);
    }
}

#[test]
fn named_chat_writes_are_refused_before_the_session_or_any_connection() {
    if !sandbox_ready() {
        return;
    }
    let own = own_chat();
    let commands: [Vec<&str>; 2] = [vec!["seen", &own, "5"], vec!["typing", &own]];
    for args in &commands {
        // No stored session at all: the guard answers first.
        probe(args, OTHER_CHAT, false).assert_refused(args, "nothing was sent");
        probe(args, OTHER_CHAT, true).assert_refused(args, "nothing was sent");
    }
}

#[test]
fn both_creation_commands_are_refused_whatever_the_limit_names() {
    if !sandbox_ready() {
        return;
    }
    let own = own_chat();
    let commands: [&[&str]; 2] = [&["create-private", GUID, "--confirm-create"], &["create-chat", "Team", GUID, "--confirm-create"]];
    for args in commands {
        for limit in [OTHER_CHAT, own.as_str()] {
            // With and without a stored session: the guard runs before the session is read.
            probe(args, limit, false).assert_refused(args, "a new chat can never be that chat");
            probe(args, limit, true).assert_refused(args, "a new chat can never be that chat");
        }
    }
}

#[test]
fn an_empty_limit_refuses_every_write_command() {
    if !sandbox_ready() {
        return;
    }
    let own = own_chat();
    let commands: [Vec<&str>; 5] = [
        vec!["react-self", "5", "100101"],
        vec!["send-self", "hello"],
        vec!["seen", &own, "5"],
        vec!["create-private", GUID, "--confirm-create"],
        vec!["create-chat", "Team", GUID, "--confirm-create"],
    ];
    for args in &commands {
        probe(args, "  ", true).assert_refused(args, "set but empty");
    }
}
