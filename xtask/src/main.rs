use std::{env, path::PathBuf, slice};

use anyhow::Result;
use clap::Parser;

mod archive;
mod notices;
mod smoke;
mod source;

/// Packages the native server and checks what a release is made from.
#[derive(Parser)]
enum Command {
    /// Checks that the versions, licenses, and release targets agree.
    CheckSource,
    /// Prints the release targets as a GitHub Actions matrix.
    Matrix,
    /// Prints the version that the manifests agree on.
    Version,
    /// Generates the dependency notices of all release targets and discards them.
    CheckNotices,
    /// Creates an archive and tests its extracted executable in mock mode.
    Package {
        /// One of the release targets that `matrix` prints.
        #[arg(long, value_name = "TRIPLE")]
        target: String,
        /// Defaults to the release build of the target.
        #[arg(long, value_name = "PATH")]
        binary: Option<PathBuf>,
        /// Defaults to `dist` in the repository.
        #[arg(long, value_name = "DIRECTORY")]
        output: Option<PathBuf>,
        /// Marks an archive of uncommitted source, which `checksums` rejects.
        #[arg(long)]
        allow_dirty: bool,
    },
    /// Tests the extracted executable of an archive in mock mode.
    Smoke {
        #[arg(long, value_name = "PATH")]
        archive: PathBuf,
    },
    /// Checks the archives of all release targets and writes `SHA256SUMS`.
    Checksums {
        /// Defaults to `dist` in the repository.
        #[arg(long, value_name = "PATH")]
        directory: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let root = source::root();
    match Command::parse() {
        Command::CheckSource => {
            source::check()?;
            println!("Release source metadata is consistent");
        }
        Command::Matrix => {
            source::check()?;
            println!("{}", source::matrix()?);
        }
        Command::Version => println!("{}", source::version()?),
        Command::CheckNotices => {
            let targets = source::targets()?;
            notices::generate(&targets)?;
            println!("Dependency notices can be generated for all release targets");
        }
        Command::Package {
            target,
            binary,
            output,
            allow_dirty,
        } => {
            let target = source::target(&target)?;
            let binary = binary.unwrap_or_else(|| {
                env::var_os("CARGO_TARGET_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root.join("target"))
                    .join(&target.target)
                    .join("release")
                    .join(target.executable())
            });
            let output = output.unwrap_or_else(|| root.join("dist"));
            source::check()?;
            let source = source::Source::current(&target, allow_dirty)?;
            let notices = notices::generate(slice::from_ref(&target))?;
            let path = archive::package(&target, &binary, &output, &source, notices)?;
            smoke::archive(&path)?;
            println!("{}", path.display());
        }
        Command::Smoke { archive } => smoke::archive(&archive)?,
        Command::Checksums { directory } => {
            let directory = directory.unwrap_or_else(|| root.join("dist"));
            archive::checksums(&directory, &source::git(&["rev-parse", "HEAD"])?)?;
        }
    }
    Ok(())
}
