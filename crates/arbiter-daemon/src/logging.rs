use tracing::Subscriber;
use tracing_subscriber::{EnvFilter, Registry, fmt::MakeWriter, layer::SubscriberExt};

pub fn json_subscriber<Writer>(writer: Writer, filter: EnvFilter) -> impl Subscriber + Send + Sync
where
    Writer: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    Registry::default()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().json().with_writer(writer))
}

/// Installs privacy-safe JSON logging configured through `RUST_LOG`.
///
/// # Errors
///
/// Returns an error when another global tracing subscriber is already installed.
pub fn init_logging() -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("arbiter_daemon=info"));
    tracing::subscriber::set_global_default(json_subscriber(std::io::stderr, filter))
}
