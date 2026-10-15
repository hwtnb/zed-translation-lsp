use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use uuid::Uuid;

use super::error::credentials::CredentialError;

/// The directory that holds the user's credentials: the ChatGPT store and the
/// Anthropic API key file.
pub(crate) fn locate() -> Result<PathBuf, CredentialError> {
    let root = dirs::config_local_dir().ok_or(CredentialError::MissingDirectory)?;
    Ok(root.join("translation-lsp"))
}

/// Tells one state of a file from another without reading it, unless the file
/// was written again within the same tick of its modification time.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Revision {
    modified: SystemTime,
    length: u64,
    #[cfg(unix)]
    inode: u64,
}

impl Revision {
    pub fn of(metadata: &fs::Metadata) -> Result<Self, CredentialError> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            modified: metadata
                .modified()
                .map_err(|source| CredentialError::io("read modification time", source))?,
            length: metadata.len(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }

    pub fn modified(&self) -> SystemTime {
        self.modified
    }
}

/// Whether the directory or the file inside it exists. One that exists must be
/// private: not a symbolic link and, on Unix, open to its owner alone. The file
/// must be a regular file; reading a named pipe would wait for its writer.
pub(crate) fn check_path(path: &Path, directory: bool) -> Result<bool, CredentialError> {
    Ok(inspect(path, directory)?.is_some())
}

/// The same check, with what it found out about a path that exists.
pub(crate) fn inspect(
    path: &Path,
    directory: bool,
) -> Result<Option<fs::Metadata>, CredentialError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(CredentialError::io("inspect path", source)),
    };
    let expected = if directory {
        metadata.is_dir()
    } else {
        metadata.is_file()
    };
    if metadata.file_type().is_symlink() || !expected {
        return Err(CredentialError::Permissions);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != if directory { 0o700 } else { 0o600 } {
            return Err(CredentialError::Permissions);
        }
    }
    Ok(Some(metadata))
}

/// A directory that exists must be private already: its permissions are the
/// user's to correct.
pub(crate) fn create(directory: &Path) -> Result<(), CredentialError> {
    if check_path(directory, true)? {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|source| CredentialError::io("create directory", source))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(directory)
        .map_err(|source| CredentialError::io("create directory", source))?;
    Ok(())
}

/// The contents are written to a temporary file beside `name`, which then takes
/// its name, so a reader finds the old contents or the new and never a part of
/// either. Returns the metadata of the new file.
pub(crate) fn replace(
    directory: &Path,
    name: &str,
    contents: &[u8],
) -> Result<fs::Metadata, CredentialError> {
    let temporary = directory.join(format!(".{name}-{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|source| CredentialError::io("create temporary file", source))?;
        file.write_all(contents)
            .map_err(|source| CredentialError::io("write credentials", source))?;
        file.sync_all()
            .map_err(|source| CredentialError::io("sync credentials", source))?;
        fs::rename(&temporary, directory.join(name))
            .map_err(|source| CredentialError::io("replace credentials", source))?;
        #[cfg(unix)]
        fs::File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| CredentialError::io("sync directory", source))?;
        file.metadata()
            .map_err(|source| CredentialError::io("read metadata", source))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
