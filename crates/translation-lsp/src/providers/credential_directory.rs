use std::{
    fs,
    path::{Path, PathBuf},
};

use super::error::credentials::CredentialError;

// The directory that holds the user's credentials: the ChatGPT store and the
// Anthropic API key file.
pub(crate) fn locate() -> Result<PathBuf, CredentialError> {
    let root = dirs::config_local_dir().ok_or(CredentialError::MissingDirectory)?;
    Ok(root.join("translation-lsp"))
}

// Whether the directory or the file inside it exists. One that exists must be
// private: not a symbolic link and, on Unix, open to its owner alone. The file
// must be a regular file; reading a named pipe would wait for its writer.
pub(crate) fn check_path(path: &Path, directory: bool) -> Result<bool, CredentialError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
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
    Ok(true)
}
