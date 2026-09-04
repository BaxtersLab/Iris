use crate::error::{IrisError, IrisResult};
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

/// Which filter directive to use: the environment wins, the config is the
/// default.
///
/// **Why the environment wins.** `RUST_LOG` is how anyone debugs a Rust binary,
/// and a config file that silently overrode it would be a trap for the person
/// trying to see more output. But before 2026-09-04 the reverse was true and
/// worse: logging was initialised from `RUST_LOG` *only*, so `logging.level`
/// in `iris.toml` reached nothing at all — while `validate()` **rejected the
/// whole config** if that same field was not one of trace/debug/info/warn/error.
/// Iris refused to start over a value it would never read.
///
/// Kept as a pure function so the precedence is testable without touching the
/// process-global subscriber, which can only be initialised once per run.
pub fn resolve_filter_directive(config_level: &str, env_rust_log: Option<&str>) -> String {
    match env_rust_log {
        Some(v) if !v.trim().is_empty() => v.to_string(),
        _ => config_level.to_string(),
    }
}

/// Where the log file lives when `log_to_file` is set.
pub fn log_file_path(log_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(log_dir).join("iris.log")
}

/// Initialise tracing.
///
/// `log_to_file` **actually writes a file** as of 2026-09-04. It previously
/// created the directory and nothing else — the body said "detailed file
/// logging will be added in integration" — so the switch produced an empty
/// folder and no logs, which is indistinguishable from working until someone
/// goes looking for the file.
pub fn init_logging(level: &str, log_to_file: bool, log_dir: &str) -> IrisResult<()> {
    let directive = resolve_filter_directive(level, std::env::var("RUST_LOG").ok().as_deref());
    let filter = EnvFilter::try_new(&directive).unwrap_or_else(|_| EnvFilter::new("info"));
    let stdout_layer = fmt::layer().with_target(false);

    if log_to_file {
        std::fs::create_dir_all(log_dir).map_err(IrisError::Io)?;
        let path = log_file_path(log_dir);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(IrisError::Io)?;
        // Appended, not truncated: a crash is exactly when the previous run's
        // log matters most, and no ANSI colour, because a file is read later by
        // something that does not render escapes.
        let file_layer = fmt::layer()
            .with_target(false)
            .with_ansi(false)
            .with_writer(file);
        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .with(file_layer)
            .try_init()
            .map_err(|e| IrisError::Config(format!("failed to init logging: {}", e)))?;
        tracing::info!("logging to {}", path.display());
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .try_init()
            .map_err(|e| IrisError::Config(format!("failed to init logging: {}", e)))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RUST_LOG` wins, because that is how a Rust binary is debugged and a
    /// config file quietly overriding it would be a trap.
    #[test]
    fn the_environment_overrides_the_configured_level() {
        assert_eq!(resolve_filter_directive("warn", Some("debug")), "debug");
        assert_eq!(resolve_filter_directive("warn", Some("iris_ui=trace")), "iris_ui=trace");
    }

    /// **The defect this closes.** With no `RUST_LOG`, the configured level must
    /// be used — it previously reached nothing while `validate()` still rejected
    /// the config over it.
    #[test]
    fn the_configured_level_is_used_when_the_environment_is_silent() {
        assert_eq!(resolve_filter_directive("warn", None), "warn");
        assert_eq!(resolve_filter_directive("trace", None), "trace");
    }

    /// An empty or whitespace `RUST_LOG` is not a directive. Treating it as one
    /// yields an unparseable filter, which silently falls back to `info` and
    /// discards the configured level with no message.
    #[test]
    fn an_empty_environment_value_falls_through_to_the_config() {
        assert_eq!(resolve_filter_directive("error", Some("")), "error");
        assert_eq!(resolve_filter_directive("error", Some("   ")), "error");
    }

    #[test]
    fn the_log_file_lives_in_the_configured_directory() {
        assert_eq!(
            log_file_path("/var/log/iris"),
            std::path::PathBuf::from("/var/log/iris/iris.log")
        );
    }
}
