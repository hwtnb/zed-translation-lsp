use std::{
    fs, io,
    path::{Path, PathBuf},
};

use zed_extension_api::{
    self as zed, Architecture, DownloadedFileType, LanguageServerInstallationStatus, Os, serde_json,
};

use super::BINARY_NAME;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

struct Release {
    directory: String,
    url: String,
    windows: bool,
}

impl Release {
    fn for_platform(os: Os, arch: Architecture) -> zed::Result<Self> {
        let os = match os {
            Os::Mac => "mac",
            Os::Linux => "linux",
            Os::Windows => "windows",
        };
        let arch = match arch {
            Architecture::Aarch64 => "aarch64",
            Architecture::X8664 => "x86_64",
            Architecture::X86 => return Err("Translation LSP does not provide 32-bit binaries. Set lsp.translation-lsp.binary.path to a compatible server.".into()),
        };
        let targets: serde_json::Value =
            serde_json::from_str(include_str!("../release-targets.json"))
                .map_err(|_| "Invalid bundled release targets")?;
        let target = targets["include"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item["os"] == os && item["arch"] == arch)
            })
            .and_then(|item| item["target"].as_str())
            .ok_or("No Translation LSP binary is available for this platform")?;
        let directory = format!("{BINARY_NAME}-v{VERSION}-{target}");
        let windows = os == "windows";
        let suffix = if windows { "zip" } else { "tar.gz" };
        Ok(Self {
            url: format!("{REPOSITORY}/releases/download/v{VERSION}/{directory}.{suffix}"),
            directory,
            windows,
        })
    }

    fn executable(&self) -> &'static str {
        if self.windows {
            "translation-lsp.exe"
        } else {
            BINARY_NAME
        }
    }

    fn file_type(&self) -> DownloadedFileType {
        if self.windows {
            DownloadedFileType::Zip
        } else {
            DownloadedFileType::GzipTar
        }
    }
}

pub(super) fn server(id: &zed::LanguageServerId) -> zed::Result<String> {
    let result: zed::Result<String> = (|| {
        let (os, arch) = zed::current_platform();
        let release = Release::for_platform(os, arch)?;
        let path = ensure(Path::new("servers"), &release, |directory| {
            zed::set_language_server_installation_status(
                id,
                &LanguageServerInstallationStatus::Downloading,
            );
            let destination = directory.to_str().ok_or("Invalid download directory")?;
            zed::download_file(&release.url, destination, release.file_type()).map_err(|error| {
                format!("Could not download Translation LSP v{VERSION}: {error}. Use lsp.translation-lsp.binary.path for a manually installed server.")
            })?;
            let executable = directory.join(release.executable());
            zed::make_file_executable(executable.to_str().ok_or("Invalid executable path")?)
        })?;
        Ok(path.to_string_lossy().into_owned())
    })();
    match &result {
        Ok(_) => zed::set_language_server_installation_status(
            id,
            &LanguageServerInstallationStatus::None,
        ),
        Err(message) => zed::set_language_server_installation_status(
            id,
            &LanguageServerInstallationStatus::Failed(message.clone()),
        ),
    }
    result
}

fn usable(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn ensure(
    root: &Path,
    release: &Release,
    download: impl FnOnce(&Path) -> zed::Result<()>,
) -> zed::Result<PathBuf> {
    let executable = install(root, release, download)?;
    remove_other_versions(root, release);
    Ok(executable)
}

fn install(
    root: &Path,
    release: &Release,
    download: impl FnOnce(&Path) -> zed::Result<()>,
) -> zed::Result<PathBuf> {
    let destination = root.join(&release.directory);
    let executable = destination.join(release.executable());
    if usable(&executable) {
        return Ok(executable);
    }
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let staging = Staging::new(root, &release.directory).map_err(|error| error.to_string())?;
    download(&staging.0)?;
    if !usable(&staging.0.join(release.executable())) {
        return Err(
            "The downloaded archive does not contain a nonempty Translation LSP executable".into(),
        );
    }
    // Another Zed window may have completed the same installation meanwhile.
    if usable(&executable) {
        return Ok(executable);
    }
    if let Err(error) = fs::rename(&staging.0, &destination)
        && !usable(&executable)
    {
        return Err(format!(
            "Could not install Translation LSP: {error}. If the cached directory is damaged, remove {} and retry.",
            destination.display()
        ));
    }
    Ok(executable)
}

// Servers of other extension versions are never started again. A directory
// that cannot be removed now, such as one whose server is still running on
// Windows, is tried again at the next start.
fn remove_other_versions(root: &Path, release: &Release) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let downloading = format!(".{}.download-", release.directory);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let other_version = name
            .strip_prefix('.')
            .unwrap_or(&name)
            .starts_with(&format!("{BINARY_NAME}-v"))
            && name != release.directory.as_str()
            && !name.starts_with(&downloading);
        if other_version {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

struct Staging(PathBuf);

impl Staging {
    fn new(root: &Path, name: &str) -> io::Result<Self> {
        for attempt in 0..1024 {
            let path = root.join(format!(".{name}.download-{attempt}"));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other(
            "Too many interrupted Translation LSP downloads; remove stale .*.download-* directories in the extension's servers directory",
        ))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests;
