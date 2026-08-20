use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginalPermissions {
    pub unix_mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_sddl: Option<String>,
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
            Ok(Self {
                unix_mode: Some(fs::metadata(path)?.permissions().mode() & 0o777),
                windows_sddl: None,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                unix_mode: None,
                windows_sddl: Some(windows_acl::capture_dacl(path)?),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Ok(Self::default())
        }
    }

    /// Returns an owner-only variant while retaining any stricter Unix mode.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the current Windows user SID cannot be read.
    pub fn private_version(&self) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                unix_mode: Some(self.unix_mode.unwrap_or(0o600) & 0o600),
                windows_sddl: None,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                unix_mode: None,
                windows_sddl: Some(windows_acl::private_sddl_no_broader(
                    self.windows_sddl.as_deref(),
                )?),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(Self::default())
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
    #[cfg(windows)]
    windows_acl::apply_sddl(path, &windows_acl::private_sddl()?)?;
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
    #[cfg(windows)]
    if let Some(sddl) = &permissions.windows_sddl {
        windows_acl::apply_sddl(temporary.path(), sddl)?;
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
    atomic_replace_with_permissions(path, bytes, &original_permissions.private_version()?)
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
    #[cfg(windows)]
    windows_acl::apply_sddl(path, &windows_acl::private_sddl()?)?;
    #[cfg(not(any(unix, windows)))]
    let _ = path;
    Ok(())
}

#[cfg(windows)]
mod windows_acl {
    use std::{ffi::OsStr, io, path::Path, process::Command};

    fn run(script: &str, environment: &[(&str, &OsStr)]) -> io::Result<String> {
        let system_root = std::env::var_os("SystemRoot")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SystemRoot is unavailable"))?;
        let executable = Path::new(&system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let mut command = Command::new(executable);
        command.env_clear().env("SystemRoot", &system_root);
        for name in ["WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ]);
        for (name, value) in environment {
            command.env(name, value);
        }
        let output = command.output()?;
        if output.status.code() == Some(13) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the original Windows ACL is too restrictive for atomic management",
            ));
        }
        if !output.status.success() {
            return Err(io::Error::other("Windows ACL operation failed"));
        }
        String::from_utf8(output.stdout)
            .map(|value| value.trim().to_owned())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub(super) fn private_sddl() -> io::Result<String> {
        let sid = run(
            "& { [Console]::Out.Write([Security.Principal.WindowsIdentity]::GetCurrent().User.Value) }",
            &[],
        )?;
        if !sid.starts_with("S-")
            || !sid
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'S' || byte == b'-')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows returned an invalid user SID",
            ));
        }
        Ok(format!("D:P(A;;FA;;;{sid})"))
    }

    pub(super) fn private_sddl_no_broader(original: Option<&str>) -> io::Result<String> {
        let Some(original) = original else {
            return private_sddl();
        };
        run(
            "& { $identity = [Security.Principal.WindowsIdentity]::GetCurrent(); $applicable = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase); [void]$applicable.Add($identity.User.Value); foreach ($group in $identity.Groups) { [void]$applicable.Add($group.Value) }; $source = [Security.AccessControl.FileSecurity]::new(); $source.SetSecurityDescriptorSddlForm($env:ARBITER_ACL_ORIGINAL); [long]$allowed = 0; [long]$denied = 0; foreach ($rule in $source.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) { if ($rule.AccessControlType -eq [Security.AccessControl.AccessControlType]::Deny) { if ($applicable.Contains($rule.IdentityReference.Value)) { $denied = $denied -bor [long]$rule.FileSystemRights } } elseif ($rule.IdentityReference.Value -eq $identity.User.Value) { $allowed = $allowed -bor [long]$rule.FileSystemRights } }; [long]$effective = $allowed -band (-bnot $denied); [long]$required = [long][Security.AccessControl.FileSystemRights]::Modify; if (($effective -band $required) -ne $required) { exit 13 }; $restricted = [Security.AccessControl.FileSecurity]::new(); $restricted.SetAccessRuleProtection($true, $false); if ($denied -ne 0) { $restricted.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($identity.User, [Security.AccessControl.FileSystemRights]$denied, [Security.AccessControl.AccessControlType]::Deny)) }; $restricted.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($identity.User, [Security.AccessControl.FileSystemRights]$effective, [Security.AccessControl.AccessControlType]::Allow)); [Console]::Out.Write($restricted.Sddl) }",
            &[("ARBITER_ACL_ORIGINAL", OsStr::new(original))],
        )
    }

    pub(super) fn capture_dacl(path: &Path) -> io::Result<String> {
        run(
            "& { [Console]::Out.Write((Get-Acl -LiteralPath $env:ARBITER_ACL_PATH).Sddl) }",
            &[("ARBITER_ACL_PATH", path.as_os_str())],
        )
    }

    pub(super) fn apply_sddl(path: &Path, sddl: &str) -> io::Result<()> {
        run(
            "& { $acl = Get-Acl -LiteralPath $env:ARBITER_ACL_PATH; $acl.SetSecurityDescriptorSddlForm($env:ARBITER_ACL_SDDL, [Security.AccessControl.AccessControlSections]::Access); Set-Acl -LiteralPath $env:ARBITER_ACL_PATH -AclObject $acl }",
            &[
                ("ARBITER_ACL_PATH", path.as_os_str()),
                ("ARBITER_ACL_SDDL", OsStr::new(sddl)),
            ],
        )
        .map(|_| ())
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use tempfile::tempdir;

    use super::{
        OriginalPermissions, atomic_replace_with_permissions, ensure_private_dir,
        harden_private_file, windows_acl,
    };

    #[test]
    fn private_paths_use_a_protected_current_user_dacl() {
        let temporary = tempdir().unwrap();
        let private_dir = temporary.path().join("private");
        ensure_private_dir(&private_dir).unwrap();
        let file = private_dir.join("secret");
        std::fs::write(&file, b"secret").unwrap();
        harden_private_file(&file).unwrap();

        let sid = windows_acl::private_sddl().unwrap();
        let directory_sddl = windows_acl::capture_dacl(&private_dir).unwrap();
        let file_sddl = windows_acl::capture_dacl(&file).unwrap();

        assert!(directory_sddl.contains("D:P"));
        assert!(file_sddl.contains("D:P"));
        assert!(directory_sddl.contains(&sid[4..]));
        assert!(file_sddl.contains(&sid[4..]));
    }

    #[test]
    fn replacement_restores_the_original_windows_dacl() {
        let temporary = tempdir().unwrap();
        let file = temporary.path().join("config.toml");
        std::fs::write(&file, b"original").unwrap();
        let original = OriginalPermissions::capture(&file, true).unwrap();
        harden_private_file(&file).unwrap();

        atomic_replace_with_permissions(&file, b"restored", &original).unwrap();

        assert_eq!(
            windows_acl::capture_dacl(&file).unwrap(),
            original.windows_sddl.unwrap()
        );
    }

    #[test]
    fn private_replacement_does_not_broaden_a_read_only_owner_dacl() {
        let temporary = tempdir().unwrap();
        let file = temporary.path().join("config.toml");
        std::fs::write(&file, b"original").unwrap();
        let private = windows_acl::private_sddl().unwrap();
        let read_only = private.replace(";;FA;;;", ";;FR;;;");
        windows_acl::apply_sddl(&file, &read_only).unwrap();
        let original = OriginalPermissions::capture(&file, true).unwrap();

        let error = original.private_version().unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
        let unchanged_sddl = windows_acl::capture_dacl(&file).unwrap();
        assert!(unchanged_sddl.contains(";;FR;;;"));
        assert!(!unchanged_sddl.contains(";;FA;;;"));
    }

    #[test]
    fn group_allow_does_not_broaden_restricted_owner_access() {
        let temporary = tempdir().unwrap();
        let file = temporary.path().join("config.toml");
        std::fs::write(&file, b"original").unwrap();
        let private = windows_acl::private_sddl().unwrap();
        let restricted_owner_with_group_allow =
            format!("{}(A;;FA;;;BU)", private.replace(";;FA;;;", ";;FR;;;"));
        windows_acl::apply_sddl(&file, &restricted_owner_with_group_allow).unwrap();
        let original = OriginalPermissions::capture(&file, true).unwrap();

        let error = original.private_version().unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
    }
}
