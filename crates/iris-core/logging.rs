use crate::error::{IrisError, IrisResult};
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

pub fn init_logging(level: &str, log_to_file: bool, log_dir: &str) -> IrisResult<()> {
    // Basic stdout logging with level filter
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = fmt::layer().with_target(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init()
        .map_err(|e| IrisError::Config(format!("failed to init logging: {}", e)))?;

    if log_to_file {
        // Ensure log directory exists; detailed file logging will be added in integration.
        std::fs::create_dir_all(log_dir).map_err(IrisError::Io)?;
    }

    Ok(())
}
