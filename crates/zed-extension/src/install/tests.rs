use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn root() -> Staging {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    Staging::new(
        &base,
        &format!(
            "translation-install-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
    )
    .unwrap()
}

#[test]
fn downloads_the_matching_version_and_platform() {
    for (os, suffix) in [
        (Os::Mac, "apple-darwin.tar.gz"),
        (Os::Linux, "unknown-linux-gnu.tar.gz"),
        (Os::Windows, "pc-windows-msvc.zip"),
    ] {
        for (arch, prefix) in [
            (Architecture::Aarch64, "aarch64"),
            (Architecture::X8664, "x86_64"),
        ] {
            let release = Release::for_platform(os, arch).unwrap();
            assert_eq!(
                release.url,
                format!(
                    "{REPOSITORY}/releases/download/v{VERSION}/translation-lsp-v{VERSION}-{prefix}-{suffix}"
                )
            );
        }
    }
    assert!(Release::for_platform(Os::Linux, Architecture::X86).is_err());
}

#[test]
fn only_completed_downloads_become_reusable_installations() {
    let root = root();
    let release = Release::for_platform(Os::Linux, Architecture::X8664).unwrap();
    let result = ensure(&root.0, &release, |path| {
        fs::write(path.join(release.executable()), b"partial").unwrap();
        Err("interrupted".into())
    });
    assert_eq!(result.unwrap_err(), "interrupted");
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
    for content in [None, Some(b"".as_slice())] {
        assert!(
            ensure(&root.0, &release, |path| {
                if let Some(content) = content {
                    fs::write(path.join(release.executable()), content).unwrap();
                }
                Ok(())
            })
            .is_err()
        );
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
    }
    let executable = ensure(&root.0, &release, |path| {
        fs::write(path.join(release.executable()), b"complete").unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(fs::read(&executable).unwrap(), b"complete");
    assert_eq!(
        ensure(&root.0, &release, |_| panic!(
            "Cached installation must work offline"
        ))
        .unwrap(),
        executable
    );
}

#[test]
fn a_completed_concurrent_installation_is_kept() {
    let root = root();
    let release = Release::for_platform(Os::Windows, Architecture::Aarch64).unwrap();
    let destination = root.0.join(&release.directory);
    let executable = ensure(&root.0, &release, |path| {
        fs::write(path.join(release.executable()), b"own download").unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join(release.executable()), b"other window").unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(fs::read(executable).unwrap(), b"other window");
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
}

#[test]
fn an_unusable_existing_directory_is_not_deleted() {
    let root = root();
    let release = Release::for_platform(Os::Linux, Architecture::X8664).unwrap();
    let destination = root.0.join(&release.directory);
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("unexpected"), b"preserve").unwrap();
    let result = ensure(&root.0, &release, |path| {
        fs::write(path.join(release.executable()), b"complete").unwrap();
        Ok(())
    });
    assert!(result.unwrap_err().contains("cached directory is damaged"));
    assert_eq!(
        fs::read(destination.join("unexpected")).unwrap(),
        b"preserve"
    );
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
}

#[test]
fn other_versions_are_removed_only_once_this_version_is_installed() {
    let root = root();
    let release = Release::for_platform(Os::Mac, Architecture::Aarch64).unwrap();
    let old = root.0.join("translation-lsp-v0.0.1-aarch64-apple-darwin");
    let interrupted = root
        .0
        .join(".translation-lsp-v0.0.1-aarch64-apple-darwin.download-0");
    let downloading = root.0.join(format!(".{}.download-7", release.directory));
    let unrelated = root.0.join("notes");
    for directory in [&old, &interrupted, &downloading, &unrelated] {
        fs::create_dir(directory).unwrap();
        fs::write(directory.join(release.executable()), b"kept").unwrap();
    }
    let remaining = || {
        let mut names: Vec<_> = fs::read_dir(&root.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        names.sort();
        names
    };

    assert!(ensure(&root.0, &release, |_| Err("offline".into())).is_err());
    let mut all = vec![
        old.clone(),
        interrupted.clone(),
        downloading.clone(),
        unrelated.clone(),
    ];
    all.sort();
    assert_eq!(remaining(), all);

    let executable = ensure(&root.0, &release, |path| {
        fs::write(path.join(release.executable()), b"complete").unwrap();
        Ok(())
    })
    .unwrap();
    let mut kept = vec![
        downloading.clone(),
        root.0.join(&release.directory),
        unrelated.clone(),
    ];
    kept.sort();
    assert_eq!(remaining(), kept);

    // A directory left by a removal that failed goes at a later start.
    fs::create_dir(&old).unwrap();
    assert_eq!(
        ensure(&root.0, &release, |_| panic!("Already installed")).unwrap(),
        executable
    );
    assert_eq!(remaining(), kept);
}
