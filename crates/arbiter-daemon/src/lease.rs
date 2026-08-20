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
        let file = open_file(&path)?;
        if !lock_existed {
            protect_file(&file, &path)?;
        }
        fs2::FileExt::try_lock_exclusive(&file)?;
        protect_file(&file, &path)?;
        drop(open_private(database)?);
        Ok(Self { _file: file })
    }
}

fn open_private(path: &Path) -> io::Result<File> {
    let file = open_file(path)?;
    protect_file(&file, path)?;
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
    use std::{io, path::Path, process::Command};

    fn run(script: &str, path: &Path) -> io::Result<String> {
        let system_root = std::env::var_os("SystemRoot")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SystemRoot is unavailable"))?;
        let executable = Path::new(&system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let mut command = Command::new(executable);
        command
            .env_clear()
            .env("SystemRoot", &system_root)
            .env("ARBITER_SECURE_PATH", path.as_os_str())
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ]);
        for name in ["WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let output = command.output()?;
        if !output.status.success() {
            return Err(io::Error::other("Windows database ACL operation failed"));
        }
        String::from_utf8(output.stdout)
            .map(|value| value.trim().to_owned())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub(super) fn protect(path: &Path) -> io::Result<()> {
        run(
            "& { $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $acl = Get-Acl -LiteralPath $env:ARBITER_SECURE_PATH; $acl.SetSecurityDescriptorSddlForm(\"D:P(A;;FA;;;$sid)\", [Security.AccessControl.AccessControlSections]::Access); Set-Acl -LiteralPath $env:ARBITER_SECURE_PATH -AclObject $acl }",
            path,
        )
        .map(|_| ())
    }

    pub(super) fn create_private(path: &Path) -> io::Result<()> {
        run(
            "& { $identity = [Security.Principal.WindowsIdentity]::GetCurrent(); $security = [Security.AccessControl.FileSecurity]::new(); $security.SetAccessRuleProtection($true, $false); $security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($identity.User, [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.AccessControlType]::Allow)); $share = [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete; $stream = [IO.FileStream]::new($env:ARBITER_SECURE_PATH, [IO.FileMode]::OpenOrCreate, [Security.AccessControl.FileSystemRights]::FullControl, $share, 1, [IO.FileOptions]::None, $security); $stream.Dispose() }",
            path,
        )
        .map(|_| ())
    }

    #[cfg(test)]
    pub(super) fn capture(path: &Path) -> io::Result<String> {
        run(
            "& { [Console]::Out.Write((Get-Acl -LiteralPath $env:ARBITER_SECURE_PATH).Sddl) }",
            path,
        )
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
