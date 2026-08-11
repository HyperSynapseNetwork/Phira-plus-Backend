//! Structured logging initialization.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialize the tracing subscriber from `RUST_LOG` or a sane default.
/// Never logs secrets (callers must avoid logging them).
pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("ppb_server=info,tower_http=info,sqlx=warn"));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent_safe() {
        // try_init already used elsewhere; calling again returns Err which we ignore.
        init();
    }
}
