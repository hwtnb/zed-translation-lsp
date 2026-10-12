use std::{fs, process::Command, thread, time::Duration};

use anyhow::{Context, Result, bail, ensure};

use crate::source::{self, Target};

// The version the workflows install. Another version may read licenses differently.
const CARGO_ABOUT: &str = "0.9.2";

// The crates that may stand under the standard text of a license, which names
// no copyright holder, each with why no text of its authors can be given.
const STANDARD_TEXT: &[&str] = &[
    // Neither the crate nor its repository holds a license file.
    "eventsource-stream",
];

// How often to run cargo-about when it cannot download a license file.
const ATTEMPTS: u32 = 3;
const PAUSE: Duration = Duration::from_secs(10);

// What cargo-about wrote: the rendered template, and its warnings.
struct Output {
    text: String,
    warnings: String,
}

// Lists the licenses of the native server's dependencies on these targets.
// `about.toml` names the accepted licenses; a dependency under any other fails,
// and so does one for which cargo-about has only the standard text of a license.
pub fn generate(targets: &[Target]) -> Result<Vec<u8>> {
    let install =
        format!("install it with cargo install --locked --features cli cargo-about@{CARGO_ABOUT}");
    let version = source::output("cargo", &["about", "--version"])
        .with_context(|| format!("cargo-about is required; {install}"))?;
    ensure!(
        version.trim() == format!("cargo-about {CARGO_ABOUT}"),
        "Expected cargo-about {CARGO_ABOUT}; {install}"
    );
    retry(|| run(targets), || thread::sleep(PAUSE))
}

fn run(targets: &[Target]) -> Result<Output> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("notices.txt");
    let mut command = Command::new("cargo");
    command
        .args(["about", "generate", "--locked", "--fail", "--manifest-path"])
        .arg("crates/translation-lsp/Cargo.toml")
        .arg("--output-file")
        .arg(&path);
    for target in targets {
        command.args(["--target", &target.target]);
    }
    let output = command
        .arg("xtask/notices.hbs")
        .current_dir(source::root())
        .output()
        .context("Starting cargo about")?;
    let warnings = String::from_utf8_lossy(&output.stderr).into_owned();
    ensure!(output.status.success(), "cargo about failed: {warnings}");
    Ok(Output {
        text: fs::read_to_string(path)?,
        warnings,
    })
}

// cargo-about falls back to the standard text of a license, and still succeeds,
// when the checksum of a license file differs and when a download fails. Only a
// failed download can pass on its own, so only that is tried again.
fn retry(mut run: impl FnMut() -> Result<Output>, mut pause: impl FnMut()) -> Result<Vec<u8>> {
    let mut attempt = 1;
    loop {
        let output = run()?;
        let (standard, notices) = split(&output.text);
        let unexpected = unexpected(&standard);
        ensure!(!notices.is_empty(), "cargo-about wrote no notices");
        if unexpected.is_empty() {
            return Ok(notices.as_bytes().to_owned());
        }
        let download_failed = download_failed(&output.warnings);
        if download_failed && attempt < ATTEMPTS {
            eprintln!(
                "cargo-about could not download a license file (attempt {attempt} of {ATTEMPTS}); trying again"
            );
            attempt += 1;
            pause();
            continue;
        }
        let advice = if download_failed {
            format!(
                "A license file could not be downloaded from GitHub in {attempt} attempts. Generating the notices needs network access."
            )
        } else {
            "If a license file changed, review it and update its checksum in about.toml; otherwise name the license file of the crate there.".to_owned()
        };
        bail!(
            "The notices would give only the standard text of a license, without the copyright notice of the authors, for: {}\n{}{advice}",
            unexpected.join(", "),
            reasons(&output.warnings)
        );
    }
}

// The template lists the crates under the standard text of a license first,
// one on a line, before the notices themselves.
fn split(text: &str) -> (Vec<&str>, &str) {
    let mut standard = Vec::new();
    let mut notices = text;
    while let Some((line, rest)) = notices.split_once('\n')
        && let Some(name) = line.strip_prefix("standard text: ")
    {
        standard.push(name.trim_end());
        notices = rest;
    }
    (standard, notices)
}

// The crates, each given as its name and version, that `STANDARD_TEXT` does not list.
fn unexpected<'a>(standard: &[&'a str]) -> Vec<&'a str> {
    standard
        .iter()
        .filter(|item| {
            let name = item.split(' ').next().unwrap_or_default();
            !STANDARD_TEXT.contains(&name)
        })
        .copied()
        .collect()
}

// What cargo-about says about the license files that `about.toml` names and it
// could not use, one crate on a line.
fn reasons(warnings: &str) -> String {
    const START: &str = "in clarification for crate ";
    warnings
        .lines()
        .filter_map(|line| line.split_once(START))
        .map(|(_, reason)| format!("  {}\n", reason.trim_end_matches("\u{1b}[0m")))
        .collect()
}

fn download_failed(warnings: &str) -> bool {
    warnings.contains("from remote git host")
}

#[cfg(test)]
mod tests;
