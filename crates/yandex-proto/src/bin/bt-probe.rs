//! Developer probe for the Yandex protocol layer. Never prints cookie values.

use std::process::ExitCode;

use yandex_proto::bootstrap::{RegistryEndpoints, bootstrap};
use yandex_proto::runtime::RuntimeDir;
use yandex_proto::session::Session;

const USAGE: &str = "usage: bt-probe me";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["me"] => run(me()),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(result: Result<(), (u8, String)>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            eprintln!("bt-probe: {message}");
            ExitCode::from(code)
        }
    }
}

fn me() -> Result<(), (u8, String)> {
    let dir = RuntimeDir::from_env().map_err(|e| (2, e.to_string()))?;
    let session = Session::load(&dir)
        .map_err(|e| (1, e.to_string()))?
        .ok_or_else(|| (1, "no stored session: sign in first via ./run.sh".to_owned()))?;
    let runtime = tokio::runtime::Runtime::new().map_err(|e| (1, e.to_string()))?;
    let identity = runtime
        .block_on(bootstrap(&RegistryEndpoints::production(), &session.cookies))
        .map_err(|e| (1, e.to_string()))?;
    println!("uid: {}", identity.uid.as_deref().unwrap_or("-"));
    println!("guid: {}", identity.guid);
    println!("display name: {}", identity.display_name.as_deref().unwrap_or("-"));
    Ok(())
}
