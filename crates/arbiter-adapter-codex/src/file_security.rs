use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginalPermissions {
    pub unix_mode: Option<u32>,
}

pub(crate) struct PreparedFileReplacement {
    temporary: tempfile::NamedTempFile,
    target: std::path::PathBuf,
}

impl PreparedFileReplacement {
    pub(crate) fn persist(self) -> io::Result<()> {
        self.temporary
            .persist(self.target)
            .map(|_| ())
            .map_err(|error| error.error)
    }
}

impl OriginalPermissions {
    /// Captures the portable permission metadata needed for restoration.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when metadata for an existing path cannot be read.
    pub fn capture(path: &Path, existed: bool) -> io::Result<Self> {
        if !existed {
            return Ok(Self::default());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            return Ok(Self {
                unix_mode: Some(fs::metadata(path)?.permissions().mode() & 0o777),
            });
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Ok(Self::default())
        }
    }

    #[must_use]
    pub fn private_version(&self) -> Self {
        #[cfg(unix)]
        {
            Self {
                unix_mode: Some(self.unix_mode.unwrap_or(0o600) & 0o600),
            }
        }
        #[cfg(not(unix))]
        {
            Self::default()
        }
    }
}

/// Creates a directory and restricts its Unix mode to the owning user.
///
/// # Errors
///
/// Returns an I/O error when creation, metadata inspection, or permission changes fail.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)?.permissions().mode() & 0o700;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// Creates, writes, and synchronizes a new owner-only file.
///
/// # Errors
///
/// Returns an I/O error when the file already exists or cannot be written or secured.
pub fn write_new_private_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    harden_private_file(path)
}

/// Atomically replaces a file and applies the supplied portable permissions.
///
/// # Errors
///
/// Returns an I/O error when staging, synchronizing, securing, or persisting fails.
pub fn atomic_replace_with_permissions(
    path: &Path,
    bytes: &[u8],
    permissions: &OriginalPermissions,
) -> io::Result<()> {
    prepare_atomic_replace_with_permissions(path, bytes, permissions)?.persist()
}

pub(crate) fn prepare_atomic_replace_with_permissions(
    path: &Path,
    bytes: &[u8],
    permissions: &OriginalPermissions,
) -> io::Result<PreparedFileReplacement> {
    #[cfg(not(unix))]
    let _ = permissions;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    #[cfg(unix)]
    if let Some(mode) = permissions.unix_mode {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    Ok(PreparedFileReplacement {
        temporary,
        target: path.to_owned(),
    })
}

/// Atomically replaces a file with owner-only permissions, retaining stricter modes.
///
/// # Errors
///
/// Returns an I/O error when staging, synchronizing, securing, or persisting fails.
pub fn atomic_replace_private(
    path: &Path,
    bytes: &[u8],
    original_permissions: &OriginalPermissions,
) -> io::Result<()> {
    atomic_replace_with_permissions(path, bytes, &original_permissions.private_version())
}

/// Restricts an existing file's Unix mode to the owning user.
///
/// # Errors
///
/// Returns an I/O error when metadata inspection or permission changes fail.
pub fn harden_private_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)?.permissions().mode() & 0o600;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
