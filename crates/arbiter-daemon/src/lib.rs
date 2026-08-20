//! Local Arbiter proxy daemon.

mod app;
mod proxy;
mod state;

pub use app::{build_router, local_bind_address, serve_local};
pub use state::AppState;
