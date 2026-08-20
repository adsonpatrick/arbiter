//! Safe Codex configuration management for Arbiter.

mod config_file;
mod profile;

pub use profile::{
    CodexProfileError, InstallReceipt, install_profile, uninstall_profile, validate_managed_profile,
};
