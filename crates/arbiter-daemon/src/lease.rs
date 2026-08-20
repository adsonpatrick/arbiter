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
        let lock_existed = path.exists();
        let file =
            open_file(&path).map_err(|error| contextualize("open lock file", &path, &error))?;
        if !lock_existed {
            protect_file(&file, &path)
                .map_err(|error| contextualize("protect new lock file", &path, &error))?;
        }
        fs2::FileExt::try_lock_exclusive(&file)?;
        protect_file(&file, &path)
            .map_err(|error| contextualize("protect lock file", &path, &error))?;
        drop(
            open_private(database)
                .map_err(|error| contextualize("open private database", database, &error))?,
        );
        Ok(Self { _file: file })
    }
}

fn contextualize(operation: &str, path: &Path, error: &io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("{operation} {}: {error}", path.display()),
    )
}

fn open_private(path: &Path) -> io::Result<File> {
    let file = open_file(path).map_err(|error| contextualize("open file", path, &error))?;
    protect_file(&file, path).map_err(|error| contextualize("protect file", path, &error))?;
    Ok(file)
}

fn open_file(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    windows_acl::create_private(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(unix)]
fn protect_file(file: &File, _path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
fn protect_file(_file: &File, path: &Path) -> io::Result<()> {
    windows_acl::protect(path)
}

#[cfg(not(any(unix, windows)))]
fn protect_file(_file: &File, _path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
mod windows_acl {
    use std::{fs::OpenOptions, io, path::Path};

    use windows_permissions::{
        LocalBox, SecurityDescriptor,
        constants::{SeObjectType, SecurityInformation},
        utilities::current_process_sid,
        wrappers::SetNamedSecurityInfo,
    };

    #[cfg(test)]
    use windows_permissions::wrappers::{
        ConvertSecurityDescriptorToStringSecurityDescriptor, GetNamedSecurityInfo,
    };

    fn private_descriptor(inherit_to_children: bool) -> io::Result<LocalBox<SecurityDescriptor>> {
        let inheritance = if inherit_to_children { "OICI" } else { "" };
        format!("D:P(A;{inheritance};FA;;;{})", current_process_sid()?).parse()
    }

    pub(super) fn protect(path: &Path) -> io::Result<()> {
        apply_private_descriptor(path, false)
    }

    fn protect_directory(path: &Path) -> io::Result<()> {
        apply_private_descriptor(path, true)
    }

    fn apply_private_descriptor(path: &Path, inherit_to_children: bool) -> io::Result<()> {
        let descriptor = private_descriptor(inherit_to_children)?;
        let dacl = descriptor
            .dacl()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "private ACL has no DACL"))?;
        SetNamedSecurityInfo(
            path.as_os_str(),
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
            None,
            None,
            Some(dacl),
            None,
        )
    }

    pub(super) fn create_private(path: &Path) -> io::Result<()> {
        if path.exists() {
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
        protect_directory(parent)?;
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(_) => protect(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => protect(path),
            Err(error) => Err(error),
        }
    }

    #[cfg(test)]
    pub(super) fn capture(path: &Path) -> io::Result<String> {
        let information = SecurityInformation::Dacl;
        let descriptor =
            GetNamedSecurityInfo(path.as_os_str(), SeObjectType::SE_FILE_OBJECT, information)?;
        ConvertSecurityDescriptorToStringSecurityDescriptor(&descriptor, information)?
            .into_string()
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Windows returned non-Unicode SDDL",
                )
            })
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

    #[cfg(unix)]
    #[test]
    fn database_and_lock_are_created_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let database = temporary.path().join("arbiter.db");
        let _lease = DaemonLease::acquire(&database).unwrap();

        for path in [&database, &temporary.path().join("arbiter.db.lock")] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn database_and_lock_are_created_with_protected_windows_dacls() {
        let temporary = tempdir().unwrap();
        let database = temporary.path().join("arbiter.db");
        let lock = temporary.path().join("arbiter.db.lock");
        let _lease = DaemonLease::acquire(&database).unwrap();

        for path in [&database, &lock] {
            let sddl = super::windows_acl::capture(path).unwrap();
            assert!(sddl.contains("D:P"));
            assert!(sddl.contains(";;FA;;;S-"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_creation_applies_the_dacl_at_creation_time() {
        let temporary = tempdir().unwrap();
        let path = temporary.path().join("new-private-file");

        super::windows_acl::create_private(&path).unwrap();

        let sddl = super::windows_acl::capture(&path).unwrap();
        assert!(sddl.contains("D:P"));
        assert!(sddl.contains(";;FA;;;S-"));
    }
}
