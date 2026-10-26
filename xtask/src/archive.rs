use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use flate2::{Compression, GzBuilder, read::GzDecoder};
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, Header};
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::source::{self, Arch, Os, Source, Target};

type Files = BTreeMap<String, Vec<u8>>;

pub fn check_binary(path: &Path, target: &Target) -> Result<()> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header)?;
    let arm = target.arch == Arch::Aarch64;
    let (machine, expected) = match target.os {
        Os::Windows => {
            ensure!(&header[..2] == b"MZ", "Expected a Windows PE executable");
            file.seek(SeekFrom::Start(
                u32::from_le_bytes(header[60..64].try_into()?) as u64,
            ))?;
            let mut pe = [0; 6];
            file.read_exact(&mut pe)?;
            ensure!(&pe[..4] == b"PE\0\0", "Invalid PE header");
            (
                u16::from_le_bytes(pe[4..6].try_into()?) as u32,
                if arm { 0xaa64 } else { 0x8664 },
            )
        }
        Os::Linux => {
            ensure!(
                &header[..6] == b"\x7fELF\x02\x01",
                "Expected a little-endian 64-bit ELF executable"
            );
            (
                u16::from_le_bytes(header[18..20].try_into()?) as u32,
                if arm { 183 } else { 62 },
            )
        }
        Os::Mac => {
            ensure!(
                &header[..4] == b"\xcf\xfa\xed\xfe",
                "Expected a 64-bit Mach-O executable"
            );
            (
                u32::from_le_bytes(header[4..8].try_into()?),
                if arm { 0x0100000c } else { 0x01000007 },
            )
        }
    };
    ensure!(
        machine == expected,
        "Executable architecture does not match target"
    );
    Ok(())
}

fn write(path: &Path, files: &Files, executable: &str, epoch: u64) -> Result<()> {
    let file = File::create(path)?;
    if path.extension().is_some_and(|e| e == "zip") {
        let mut archive = ZipWriter::new(file);
        for (name, data) in files {
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .last_modified_time(DateTime::default())
                .unix_permissions(if name == executable { 0o755 } else { 0o644 });
            archive.start_file(name, options)?;
            archive.write_all(data)?;
        }
        archive.finish()?;
    } else {
        let compressed = GzBuilder::new()
            .mtime(epoch.try_into()?)
            .write(file, Compression::default());
        let mut archive = Builder::new(compressed);
        for (name, data) in files {
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mtime(epoch);
            header.set_mode(if name == executable { 0o755 } else { 0o644 });
            header.set_cksum();
            archive.append_data(&mut header, name, data.as_slice())?;
        }
        archive.into_inner()?.finish()?;
    }
    Ok(())
}

fn read(path: &Path) -> Result<Files> {
    let mut files = Files::new();
    let file = File::open(path)?;
    if path.extension().is_some_and(|e| e == "zip") {
        let mut archive = ZipArchive::new(file)?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            ensure!(
                entry.is_file() && !entry.is_symlink(),
                "Expected regular archive files"
            );
            let name = entry.name()?.into_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            ensure!(
                files.insert(name, data).is_none(),
                "Duplicate archive entry"
            );
        }
    } else {
        let mut archive = Archive::new(GzDecoder::new(file));
        for entry in archive.entries()? {
            let mut entry = entry?;
            ensure!(
                entry.header().entry_type().is_file(),
                "Expected regular archive files"
            );
            let name = entry
                .path()?
                .to_str()
                .context("Non-UTF8 archive path")?
                .to_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            ensure!(
                files.insert(name, data).is_none(),
                "Duplicate archive entry"
            );
        }
    }
    Ok(files)
}

pub fn digest(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn package(
    target: &Target,
    binary: &Path,
    output: &Path,
    source: &Source,
    notices: Vec<u8>,
) -> Result<PathBuf> {
    check_binary(binary, target)?;
    ensure!(
        source.target == target.target,
        "Source target must match archive"
    );
    fs::create_dir_all(output)?;
    let archive = output.join(target.archive_name(&source.version));
    let files = Files::from([
        (target.executable().into(), fs::read(binary)?),
        ("LICENSE".into(), fs::read(source::root().join("LICENSE"))?),
        ("THIRD_PARTY_LICENSES.txt".into(), notices),
        ("SOURCE.json".into(), serde_json::to_vec_pretty(source)?),
        (
            "README.txt".into(),
            format!(
                concat!(
                    "Translation LSP {}\nTarget: {}\n\n",
                    "Extract and set lsp.translation-lsp.binary.path to the absolute path of {} ",
                    "for manual installation. Keep the license files.\n",
                    "Default mock mode contacts no provider and translates only bundled samples.\n",
                    "Settings and privacy: {}\n"
                ),
                source.version,
                target.target,
                target.executable(),
                source::REPOSITORY
            )
            .into_bytes(),
        ),
    ]);
    write(
        &archive,
        &files,
        target.executable(),
        source.source_date_epoch,
    )?;
    let name = archive
        .file_name()
        .context("Archive filename")?
        .to_str()
        .context("Archive filename encoding")?;
    fs::write(
        output.join(format!("{name}.sha256")),
        format!("{}  {name}\n", digest(&archive)?),
    )?;
    Ok(archive)
}

pub fn unpack(path: &Path, directory: &Path) -> Result<(PathBuf, Source)> {
    let files = read(path)?;
    let source: Source =
        serde_json::from_slice(files.get("SOURCE.json").context("Missing SOURCE.json")?)?;
    let target = source::target(&source.target)?;
    let expected: BTreeSet<_> = [
        target.executable(),
        "LICENSE",
        "THIRD_PARTY_LICENSES.txt",
        "SOURCE.json",
        "README.txt",
    ]
    .into_iter()
    .collect();
    ensure!(
        files.keys().map(String::as_str).collect::<BTreeSet<_>>() == expected,
        "Unexpected archive contents"
    );
    ensure!(
        path.file_name().and_then(|n| n.to_str()) == Some(&target.archive_name(&source.version))
            && source.version == source::version()?,
        "Archive name and version must match source"
    );
    for (name, data) in files {
        fs::write(directory.join(name), data)?;
    }
    let binary = directory.join(target.executable());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    }
    check_binary(&binary, &target)?;
    Ok((binary, source))
}

pub fn checksums(directory: &Path, commit: &str) -> Result<()> {
    let version = source::version()?;
    let expected: BTreeSet<_> = source::targets()?
        .iter()
        .map(|t| t.archive_name(&version))
        .collect();
    let actual: BTreeSet<_> = fs::read_dir(directory)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|n| n.ends_with(".zip") || n.ends_with(".tar.gz"))
        .collect();
    ensure!(
        actual == expected,
        "Expected exactly all six release archives"
    );
    let mut sums = String::new();
    for name in expected {
        let path = directory.join(&name);
        let sum = format!("{}  {name}\n", digest(&path)?);
        ensure!(
            fs::read_to_string(directory.join(format!("{name}.sha256")))?.replace("\r\n", "\n")
                == sum,
            "Checksum mismatch for {name}"
        );
        let temporary = tempfile::tempdir()?;
        let (_, source) = unpack(&path, temporary.path())?;
        ensure!(
            !source.dirty && source.commit == commit,
            "Only clean archives from the current commit may be collected"
        );
        sums.push_str(&sum);
    }
    fs::write(directory.join("SHA256SUMS"), sums)?;
    Ok(())
}

#[cfg(test)]
mod tests;
