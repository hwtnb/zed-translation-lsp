use std::{collections::BTreeSet, fs, path::PathBuf, process::Command};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const REPOSITORY: &str = "https://github.com/hwtnb/zed-translation-lsp";

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

pub fn output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root())
        .output()
        .with_context(|| format!("Starting {program}"))?;
    ensure!(
        output.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

pub fn git(args: &[&str]) -> Result<String> {
    Ok(output("git", args)?.trim().to_owned())
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Mac,
    Linux,
    Windows,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    Aarch64,
    X86_64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub os: Os,
    pub arch: Arch,
    pub target: String,
}

impl Target {
    // The GitHub Actions runner that builds and tests this target.
    fn runner(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Mac, Arch::Aarch64) => "macos-15",
            (Os::Mac, Arch::X86_64) => "macos-15-intel",
            (Os::Linux, Arch::Aarch64) => "ubuntu-22.04-arm",
            (Os::Linux, Arch::X86_64) => "ubuntu-22.04",
            (Os::Windows, Arch::Aarch64) => "windows-11-arm",
            (Os::Windows, Arch::X86_64) => "windows-2025",
        }
    }
    pub fn executable(&self) -> &'static str {
        if self.os == Os::Windows {
            "translation-lsp.exe"
        } else {
            "translation-lsp"
        }
    }
    pub fn archive_name(&self, version: &str) -> String {
        let suffix = if self.os == Os::Windows {
            "zip"
        } else {
            "tar.gz"
        };
        format!("translation-lsp-v{version}-{}.{suffix}", self.target)
    }
}

pub fn targets() -> Result<Vec<Target>> {
    #[derive(Deserialize)]
    struct Matrix {
        include: Vec<Target>,
    }
    let matrix: Matrix = serde_json::from_str(&fs::read_to_string(
        root().join("crates/zed-extension/release-targets.json"),
    )?)?;
    let mut pairs = BTreeSet::new();
    for item in &matrix.include {
        let suffix = match item.os {
            Os::Mac => "apple-darwin",
            Os::Linux => "unknown-linux-gnu",
            Os::Windows => "pc-windows-msvc",
        };
        let prefix = match item.arch {
            Arch::Aarch64 => "aarch64",
            Arch::X86_64 => "x86_64",
        };
        ensure!(
            item.target == format!("{prefix}-{suffix}"),
            "Target does not match OS and architecture"
        );
        ensure!(pairs.insert((item.os, item.arch)), "Duplicate target");
    }
    ensure!(
        pairs.len() == 6,
        "Expected both architectures on all three OSes"
    );
    Ok(matrix.include)
}

pub fn matrix() -> Result<serde_json::Value> {
    let include: Vec<_> = targets()?
        .iter()
        .map(|target| {
            serde_json::json!({
                "os": target.os,
                "arch": target.arch,
                "target": target.target,
                "runner": target.runner(),
            })
        })
        .collect();
    Ok(serde_json::json!({ "include": include }))
}

pub fn target(name: &str) -> Result<Target> {
    targets()?
        .into_iter()
        .find(|t| t.target == name)
        .with_context(|| format!("Unknown target {name}"))
}

#[derive(Deserialize)]
struct CargoManifest {
    package: PackageMetadata,
}

#[derive(Deserialize)]
struct PackageMetadata {
    version: String,
    license: String,
    repository: String,
}

#[derive(Deserialize)]
struct ExtensionMetadata {
    version: String,
    id: String,
    repository: String,
    authors: Vec<String>,
}

fn manifest<T: DeserializeOwned>(path: &str) -> Result<T> {
    toml::from_str(&fs::read_to_string(root().join(path))?)
        .with_context(|| format!("Invalid metadata in {path}"))
}

pub fn version() -> Result<String> {
    let native: CargoManifest = manifest("crates/translation-lsp/Cargo.toml")?;
    let extension: CargoManifest = manifest("crates/zed-extension/Cargo.toml")?;
    let registry: ExtensionMetadata = manifest("crates/zed-extension/extension.toml")?;
    let version = native.package.version;
    ensure!(
        extension.package.version == version && registry.version == version,
        "Release versions must match"
    );
    let parts: Vec<_> = version.split('.').collect();
    ensure!(
        parts.len() == 3
            && parts.iter().all(|p| !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))),
        "Expected major.minor.patch"
    );
    Ok(version)
}

pub fn check() -> Result<()> {
    version()?;
    targets()?;
    let license = fs::read_to_string(root().join("LICENSE"))?.replace("\r\n", "\n");
    ensure!(
        license.starts_with("MIT License\n")
            && license
                == fs::read_to_string(root().join("crates/zed-extension/LICENSE"))?
                    .replace("\r\n", "\n"),
        "MIT licenses must match"
    );
    for path in [
        "crates/translation-lsp/Cargo.toml",
        "crates/zed-extension/Cargo.toml",
    ] {
        let m: CargoManifest = manifest(path)?;
        ensure!(
            m.package.license == "MIT" && m.package.repository == REPOSITORY,
            "Invalid package metadata in {path}"
        );
    }
    let m: ExtensionMetadata = manifest("crates/zed-extension/extension.toml")?;
    ensure!(
        m.id == "translation-lsp" && m.repository == REPOSITORY && !m.authors.is_empty(),
        "Invalid extension metadata"
    );
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub version: String,
    pub commit: String,
    pub dirty: bool,
    pub source_date_epoch: u64,
    pub target: String,
}

impl Source {
    pub fn current(target: &Target, allow_dirty: bool) -> Result<Self> {
        let dirty = !git(&["status", "--porcelain=v1", "--untracked-files=normal"])?.is_empty();
        ensure!(
            !dirty || allow_dirty,
            "Commit the source first, or use --allow-dirty for local checks"
        );
        Ok(Self {
            version: version()?,
            commit: git(&["rev-parse", "HEAD"])?,
            dirty,
            source_date_epoch: git(&["show", "-s", "--format=%ct", "HEAD"])?.parse()?,
            target: target.target.clone(),
        })
    }
}
