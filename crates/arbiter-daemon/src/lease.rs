use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

pub struct DaemonLease {
    _file: File,
}

impl DaemonLease {
    /// Acquires exclusive ownership of the database lifecycle.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the lock file cannot be created or another
    /// Arbiter process already owns the database.
    pub fn acquire(database: &Path) -> io::Result<Self> {
        let file_name = database
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "database has no name"))?;
        let mut lock_name = file_name.to_os_string();
        lock_name.push(".lock");
        let path = database.with_file_name(lock_name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        fs2::FileExt::try_lock_exclusive(&file)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::DaemonLease;

    #[test]
    fn only_one_process_lease_can_own_a_database() {
        let temporary = tempdir().unwrap();
        let database = temporary.path().join("arbiter.db");
        let first = DaemonLease::acquire(&database).unwrap();

        assert!(DaemonLease::acquire(&database).is_err());
        assert!(temporary.path().join("arbiter.db.lock").exists());

        drop(first);
        assert!(DaemonLease::acquire(&database).is_ok());
    }
}
