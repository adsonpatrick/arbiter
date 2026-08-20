//! Local Arbiter proxy daemon.

mod app;
mod lease;
mod logging;
mod proxy;
mod state;

pub use app::{
    DEFAULT_SHUTDOWN_GRACE, build_router, local_bind_address, serve_listener_with_shutdown,
    serve_local, serve_local_with_shutdown, shutdown_signal,
};
pub use lease::DaemonLease;
pub use logging::{init_logging, json_subscriber};
pub use state::AppState;
