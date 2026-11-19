//! Logging policy shared by the binaries: the `BT_LOG` filter and a ceiling on dependency
//! verbosity that `BT_LOG` cannot lift.

use tracing::{Level, Metadata};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::{FilterFn, filter_fn};

pub const LOG_VAR: &str = "BT_LOG";
const DEFAULT_FILTER: &str = "info";
/// Targets allowed to log at `trace`; everything else stops at `debug`.
const OWN_CRATES: [&str; 3] = ["yandex_proto", "bt_chat_core", "blazing_telesram"];

/// Parses a `BT_LOG` value; `None` or blank selects `info`. The error names `BT_LOG`.
pub fn parse_filter(value: Option<&str>) -> Result<EnvFilter, String> {
    let value = value.filter(|v| !v.trim().is_empty()).unwrap_or(DEFAULT_FILTER);
    EnvFilter::builder().parse(value).map_err(|e| format!("{LOG_VAR}={value:?} is not a valid log filter: {e}"))
}

/// The filter from the `BT_LOG` environment variable.
pub fn filter_from_env() -> Result<EnvFilter, String> {
    match std::env::var(LOG_VAR) {
        Ok(value) => parse_filter(Some(&value)),
        Err(std::env::VarError::NotPresent) => parse_filter(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{LOG_VAR} is not valid UTF-8")),
    }
}

/// Dependencies write secrets at `trace`: tungstenite logs the whole WebSocket handshake,
/// `Cookie` header included. Install next to the `BT_LOG` filter.
pub fn dependency_cap() -> FilterFn<fn(&Metadata<'_>) -> bool> {
    filter_fn(allowed)
}

fn allowed(meta: &Metadata<'_>) -> bool {
    *meta.level() != Level::TRACE || is_own(meta.target())
}

fn is_own(target: &str) -> bool {
    OWN_CRATES.iter().any(|own| target.strip_prefix(own).is_some_and(|rest| rest.is_empty() || rest.starts_with("::")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_defaults_to_info_and_rejects_garbage() {
        assert_eq!(parse_filter(None).unwrap().to_string(), "info");
        assert_eq!(parse_filter(Some("  ")).unwrap().to_string(), "info");
        assert!(parse_filter(Some("info,yandex_proto::xiva=trace")).is_ok());
        let error = parse_filter(Some("yandex_proto=loud")).unwrap_err();
        assert!(error.contains(LOG_VAR), "{error}");
    }

    #[test]
    fn only_own_crates_log_at_trace() {
        assert!(is_own("yandex_proto"));
        assert!(is_own("yandex_proto::xiva"));
        assert!(is_own("blazing_telesram::login"));
        assert!(!is_own("yandex_proto_fake"));
        assert!(!is_own("tungstenite::handshake::client"));
    }
}
