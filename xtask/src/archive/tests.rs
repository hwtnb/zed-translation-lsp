use tempfile::NamedTempFile;

use super::*;

#[test]
fn checksum_matches_the_sha256_known_vector() -> Result<()> {
    let file = NamedTempFile::new()?;
    fs::write(file.path(), b"abc")?;
    assert_eq!(
        digest(file.path())?,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    Ok(())
}

fn binary(target: &Target) -> Vec<u8> {
    let mut bytes = vec![0; 128];
    let arm = target.arch == Arch::Aarch64;
    match target.os {
        Os::Mac => {
            bytes[..4].copy_from_slice(b"\xcf\xfa\xed\xfe");
            bytes[4..8]
                .copy_from_slice(&(if arm { 0x0100000cu32 } else { 0x01000007 }).to_le_bytes());
        }
        Os::Linux => {
            bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
            bytes[18..20].copy_from_slice(&(if arm { 183u16 } else { 62 }).to_le_bytes());
        }
        Os::Windows => {
            bytes[..2].copy_from_slice(b"MZ");
            bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
            bytes[64..68].copy_from_slice(b"PE\0\0");
            bytes[68..70].copy_from_slice(&(if arm { 0xaa64u16 } else { 0x8664 }).to_le_bytes());
        }
    }
    bytes
}

fn source(target: &Target) -> Source {
    Source {
        version: source::version().unwrap(),
        commit: "a".repeat(40),
        dirty: false,
        source_date_epoch: 1700000000,
        target: target.target.clone(),
    }
}

#[test]
fn metadata_and_all_six_archives_match_their_executables() -> Result<()> {
    source::check()?;
    let temporary = tempfile::tempdir()?;
    for target in source::targets()? {
        let input = temporary.path().join(target.executable());
        fs::write(&input, binary(&target))?;
        let path = package(
            &target,
            &input,
            temporary.path(),
            &source(&target),
            b"license".to_vec(),
        )?;
        let original = fs::read(&path)?;
        package(
            &target,
            &input,
            temporary.path(),
            &source(&target),
            b"license".to_vec(),
        )?;
        assert_eq!(original, fs::read(&path)?);
        let extracted = tempfile::tempdir()?;
        let (actual, info) = unpack(&path, extracted.path())?;
        assert_eq!(fs::read(&actual)?, binary(&target));
        assert_eq!(info.target, target.target);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&actual)?.permissions().mode() & 0o777, 0o755);
        }
        let wrong = source::targets()?
            .into_iter()
            .find(|t| t.os == target.os && t.arch != target.arch)
            .unwrap();
        assert!(check_binary(&actual, &wrong).is_err());
    }
    Ok(())
}

#[test]
fn unexpected_paths_are_rejected_before_writing() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let target = source::target("aarch64-pc-windows-msvc")?;
    let path = temporary
        .path()
        .join(target.archive_name(&source::version()?));
    let files = Files::from([
        ("../outside".into(), b"bad".to_vec()),
        ("SOURCE.json".into(), serde_json::to_vec(&source(&target))?),
    ]);
    write(&path, &files, target.executable(), 1700000000)?;
    let extracted = temporary.path().join("extracted");
    fs::create_dir(&extracted)?;
    assert!(unpack(&path, &extracted).is_err());
    assert_eq!(fs::read_dir(extracted)?.count(), 0);
    assert!(!temporary.path().join("outside").exists());
    Ok(())
}

#[test]
fn collection_rejects_damage_dirty_sources_wrong_commits_and_missing_targets() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path();
    for target in source::targets()? {
        let input = directory.join(target.executable());
        fs::write(&input, binary(&target))?;
        package(
            &target,
            &input,
            directory,
            &source(&target),
            b"license".to_vec(),
        )?;
    }
    let commit = "a".repeat(40);
    checksums(directory, &commit)?;
    assert_eq!(
        fs::read_to_string(directory.join("SHA256SUMS"))?
            .lines()
            .count(),
        6
    );
    assert!(checksums(directory, &"b".repeat(40)).is_err());
    let target = source::targets()?.remove(0);
    let archive = directory.join(target.archive_name(&source::version()?));
    let original = fs::read(&archive)?;
    fs::write(&archive, b"damaged")?;
    assert!(checksums(directory, &commit).is_err());
    fs::write(&archive, original)?;
    let input = directory.join(target.executable());
    fs::write(&input, binary(&target))?;
    let mut dirty = source(&target);
    dirty.dirty = true;
    package(&target, &input, directory, &dirty, b"license".to_vec())?;
    assert!(checksums(directory, &commit).is_err());
    fs::remove_file(archive)?;
    assert!(checksums(directory, &commit).is_err());
    Ok(())
}
