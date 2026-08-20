use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use sha2::{Digest, Sha256};

use crate::profile::CodexProfileError;

pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), CodexProfileError> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), CodexProfileError> {
    let parent = path.parent().ok_or(CodexProfileError::MissingParent)?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
