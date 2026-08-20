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
    use std::{io, path::Path};

    use windows_permissions::{
        LocalBox, SecurityDescriptor, Trustee,
        constants::{AceType, SeObjectType, SecurityInformation},
        utilities::current_process_sid,
        wrappers::{
            ConvertSecurityDescriptorToStringSecurityDescriptor, GetNamedSecurityInfo,
            SetNamedSecurityInfo,
        },
    };

    const FILE_MODIFY: u32 = 0x0003_01BF;

    fn parse_sddl(sddl: &str) -> io::Result<LocalBox<SecurityDescriptor>> {
        sddl.parse()
    }

    fn dacl_is_protected(sddl: &str) -> bool {
        let Some((_, dacl)) = sddl.split_once("D:") else {
            return false;
        };
        let first_ace = dacl.find('(').unwrap_or(dacl.len());
        let next_section = dacl.find("S:").unwrap_or(dacl.len());
        let control_end = first_ace.min(next_section);
        dacl[..control_end].contains('P')
    }

    fn is_allow_ace(ace_type: AceType) -> bool {
        matches!(
            ace_type,
            AceType::ACCESS_ALLOWED_ACE_TYPE
                | AceType::ACCESS_ALLOWED_CALLBACK_ACE_TYPE
                | AceType::ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE
                | AceType::ACCESS_ALLOWED_OBJECT_ACE_TYPE
        )
    }

    pub(super) fn private_sddl() -> io::Result<String> {
        let sid = current_process_sid()?;
        Ok(format!("D:P(A;;FA;;;{sid})"))
    }

    pub(super) fn private_sddl_no_broader(original: Option<&str>) -> io::Result<String> {
        let Some(original) = original else {
            return private_sddl();
        };
        let descriptor = parse_sddl(original)?;
        let dacl = descriptor
            .dacl()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Windows ACL has no DACL"))?;
        let identity = current_process_sid()?;
        let mut direct_allowed = 0_u32;
        for index in 0..dacl.len() {
            let ace = dacl
                .get_ace(index)
                .expect("an ACE below the reported DACL length must exist");
            if is_allow_ace(ace.ace_type()) && ace.sid() == Some(identity.as_ref()) {
                direct_allowed |= ace.mask().bits();
            }
        }
        let trustee = Trustee::from(identity.as_ref());
        let effective = direct_allowed & dacl.effective_rights(&trustee)?.bits();
        if effective & FILE_MODIFY != FILE_MODIFY {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the original Windows ACL is too restrictive for atomic management",
            ));
        }
        Ok(format!("D:P(A;;0x{effective:08x};;;{identity})"))
    }

    pub(super) fn capture_dacl(path: &Path) -> io::Result<String> {
        let information =
            SecurityInformation::Owner | SecurityInformation::Group | SecurityInformation::Dacl;
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

    pub(super) fn apply_sddl(path: &Path, sddl: &str) -> io::Result<()> {
        let descriptor = parse_sddl(sddl)?;
        let dacl = descriptor
            .dacl()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Windows ACL has no DACL"))?;
        let protection = if dacl_is_protected(sddl) {
            SecurityInformation::ProtectedDacl
        } else {
            SecurityInformation::UnprotectedDacl
        };
        SetNamedSecurityInfo(
            path.as_os_str(),
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | protection,
            None,
            None,
            Some(dacl),
            None,
        )
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use tempfile::tempdir;

    use super::{
        OriginalPermissions, atomic_replace_with_permissions, ensure_private_dir,
        harden_private_file, windows_acl,
    };

    fn normalized_dacl(sddl: &str) -> String {
        let start = sddl.find("D:").expect("SDDL must contain a DACL");
        let dacl = &sddl[start..];
        let end = dacl.find("S:").unwrap_or(dacl.len());
        let dacl = &dacl[..end];
        let first_ace = dacl.find('(').unwrap_or(dacl.len());
        let (control, aces) = dacl.split_at(first_ace);
        format!("{}{aces}", control.replace("AI", ""))
    }

    fn has_single_protected_full_control_ace(sddl: &str) -> bool {
        let dacl = normalized_dacl(sddl);
        let Some(dacl) = dacl.strip_prefix("D:P(A;;FA;;;") else {
            return false;
        };
        let Some((principal, remainder)) = dacl.split_once(')') else {
            return false;
        };
        !principal.is_empty() && !remainder.contains('(')
    }

    #[test]
    fn private_paths_use_a_protected_current_user_dacl() {
        let temporary = tempdir().unwrap();
        let private_dir = temporary.path().join("private");
        ensure_private_dir(&private_dir).unwrap();
        let file = private_dir.join("secret");
        std::fs::write(&file, b"secret").unwrap();
        harden_private_file(&file).unwrap();

        let directory_sddl = windows_acl::capture_dacl(&private_dir).unwrap();
        let file_sddl = windows_acl::capture_dacl(&file).unwrap();

        assert!(directory_sddl.contains("D:P"));
        assert!(file_sddl.contains("D:P"));
        assert!(
            has_single_protected_full_control_ace(&directory_sddl),
            "unexpected private directory DACL: {directory_sddl}"
        );
        assert!(
            has_single_protected_full_control_ace(&file_sddl),
            "unexpected private file DACL: {file_sddl}"
        );
    }

    #[test]
    fn replacement_restores_the_original_windows_dacl() {
        let temporary = tempdir().unwrap();
        let file = temporary.path().join("config.toml");
        std::fs::write(&file, b"original").unwrap();
        let original = OriginalPermissions::capture(&file, true).unwrap();
        harden_private_file(&file).unwrap();

        atomic_replace_with_permissions(&file, b"restored", &original).unwrap();

        let restored = windows_acl::capture_dacl(&file).unwrap();
        assert_eq!(
            normalized_dacl(&restored),
            normalized_dacl(original.windows_sddl.as_deref().unwrap())
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
