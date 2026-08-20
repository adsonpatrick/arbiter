//! Safe Codex configuration management for Arbiter.

mod config_file;
mod file_security;
mod profile;

pub use file_security::{
    OriginalPermissions, atomic_replace_private, atomic_replace_with_permissions,
    ensure_private_dir, harden_private_file, write_new_private_synced,
};
pub use profile::{
    CodexProfileError, InstallReceipt, ManagedFileReceipt, install_profile, uninstall_profile,
    validate_managed_profile,
};
