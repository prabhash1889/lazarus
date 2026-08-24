//! Structured process logging for `lazarus-hostd`.

use tracing_subscriber::EnvFilter;

/// Installs newline-delimited JSON logs on stdout. Repeated initialization is
/// harmless, which keeps in-process tests and embedded launchers composable.
pub fn init_structured_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .try_init();
}
