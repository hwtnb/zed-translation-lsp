use std::fs;

use anyhow::{Context, Result, ensure};

use crate::source::{self, Target};

// The version the workflows install. Another version may read licenses differently.
const CARGO_ABOUT: &str = "0.9.2";

// Lists the licenses of the native server's dependencies on these targets.
// `about.toml` names the accepted licenses; a dependency under any other fails.
pub fn generate(targets: &[Target]) -> Result<Vec<u8>> {
    let install =
        format!("install it with cargo install --locked --features cli cargo-about@{CARGO_ABOUT}");
    let version = source::output("cargo", &["about", "--version"])
        .with_context(|| format!("cargo-about is required; {install}"))?;
    ensure!(
        version.trim() == format!("cargo-about {CARGO_ABOUT}"),
        "Expected cargo-about {CARGO_ABOUT}; {install}"
    );
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("notices.txt");
    let path = path.to_str().context("Temporary path encoding")?;
    let mut args = vec![
        "about",
        "generate",
        "--locked",
        "--fail",
        "--manifest-path",
        "crates/translation-lsp/Cargo.toml",
        "--output-file",
        path,
    ];
    for target in targets {
        args.extend(["--target", &target.target]);
    }
    args.push("xtask/notices.hbs");
    source::output("cargo", &args)?;
    let notices = fs::read(path)?;
    ensure!(!notices.is_empty(), "cargo-about wrote no notices");
    Ok(notices)
}
