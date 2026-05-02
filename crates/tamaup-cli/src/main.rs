use std::fs;
use std::io::{Cursor, Read};
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::ExitCode;

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, Subcommand};
use flate2::read::GzDecoder;
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DEFAULT_BASE_URL: &str = "https://tama.tools";
const EMBEDDED_PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";

#[derive(Debug, Parser)]
#[command(name = "tamaup", version, about = "Install and update Tama")]
struct Cli {
    #[arg(long)]
    yes: bool,
    #[arg(long)]
    offline: bool,
    #[arg(long)]
    no_modify_path: bool,
    #[arg(long)]
    no_install_lean: bool,
    #[arg(long)]
    no_install_foundry: bool,
    #[arg(long)]
    no_install_solc: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Install {
        version: Option<String>,
        #[arg(long)]
        manifest_file: Option<Utf8PathBuf>,
    },
    Use {
        version: String,
    },
    List,
    Self_ {
        #[command(subcommand)]
        command: SelfCommand,
    },
    Uninstall,
}

#[derive(Debug, Subcommand)]
enum SelfCommand {
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReleaseManifest {
    version: String,
    artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Artifact {
    platform: String,
    url: String,
    sha256: String,
}

struct PendingArchiveEntry {
    path: Utf8PathBuf,
    mode: u32,
    content: Vec<u8>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command.unwrap_or(Command::Install {
        version: None,
        manifest_file: None,
    }) {
        Command::Install {
            version,
            manifest_file,
        } => install(
            version.as_deref().unwrap_or("stable"),
            manifest_file,
            cli.offline,
        ),
        Command::Use { version } => use_version(&version),
        Command::List => list_versions(),
        Command::Self_ {
            command: SelfCommand::Update,
        } => install("stable", None, cli.offline),
        Command::Uninstall => uninstall(),
    }
}

fn install(version: &str, manifest_file: Option<Utf8PathBuf>, offline: bool) -> Result<(), String> {
    let (manifest_bytes, signature_bytes) = if let Some(path) = manifest_file {
        let sig = path.with_extension("json.minisig");
        (
            fs::read(&path).map_err(|err| err.to_string())?,
            fs::read(&sig).map_err(|err| err.to_string())?,
        )
    } else {
        if offline {
            return Err("offline install requires --manifest-file".to_string());
        }
        (
            download(&format!("{DEFAULT_BASE_URL}/manifest.json"))?,
            download(&format!("{DEFAULT_BASE_URL}/manifest.json.minisig"))?,
        )
    };
    verify_manifest_signature(&manifest_bytes, &signature_bytes)?;
    let manifest: ReleaseManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|err| err.to_string())?;
    let platform = platform()?;
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| {
            artifact.platform == platform && (version == "stable" || manifest.version == version)
        })
        .ok_or_else(|| format!("no artifact for {platform} version {version}"))?;
    let archive = if artifact.url.starts_with("file://") {
        fs::read(artifact.url.trim_start_matches("file://")).map_err(|err| err.to_string())?
    } else {
        if offline {
            return Err("offline install cannot download artifact".to_string());
        }
        download(&artifact.url)?
    };
    verify_sha256(&archive, &artifact.sha256)?;
    let version_dir = tama_home().join("versions").join(&manifest.version);
    fs::create_dir_all(&version_dir).map_err(|err| err.to_string())?;
    extract_archive(&archive, &version_dir)?;
    use_version(&manifest.version)?;
    println!("Installed Tama {}", manifest.version);
    Ok(())
}

fn use_version(version: &str) -> Result<(), String> {
    use_version_at(&tama_home(), version)
}

fn use_version_at(home: &Utf8Path, version: &str) -> Result<(), String> {
    let active = home.join("active");
    let bin = home.join("bin");
    let version_dir = home.join("versions").join(version);
    if !version_dir.is_dir() {
        return Err(format!("Tama version `{version}` is not installed"));
    }
    fs::create_dir_all(&bin).map_err(|err| err.to_string())?;
    for binary in ["tama", "tamaup"] {
        let target = binary_path(&version_dir, binary)?;
        atomic_symlink(&target, &bin.join(binary))?;
    }
    atomic_write(&active, version.as_bytes())?;
    println!("Active Tama version: {version}");
    Ok(())
}

fn binary_path(version_dir: &Utf8Path, binary: &str) -> Result<Utf8PathBuf, String> {
    for candidate in [
        version_dir.join("bin").join(binary),
        version_dir.join(binary),
    ] {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "installed version is missing expected binary `{binary}`"
    ))
}

fn atomic_write(path: &Utf8Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|err| err.to_string())?;
    fs::rename(&tmp, path).map_err(|err| err.to_string())
}

#[cfg(unix)]
fn atomic_symlink(target: &Utf8Path, link: &Utf8Path) -> Result<(), String> {
    let tmp = link.with_extension("tmp");
    if tmp.exists() {
        fs::remove_file(&tmp).map_err(|err| err.to_string())?;
    }
    symlink(target, &tmp).map_err(|err| err.to_string())?;
    fs::rename(&tmp, link).map_err(|err| err.to_string())
}

fn list_versions() -> Result<(), String> {
    let home = tama_home();
    let active = fs::read_to_string(home.join("active")).unwrap_or_default();
    let versions = home.join("versions");
    if versions.is_dir() {
        for entry in fs::read_dir(versions).map_err(|err| err.to_string())? {
            let entry = entry.map_err(|err| err.to_string())?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name == active.trim() {
                println!("* {name}");
            } else {
                println!("  {name}");
            }
        }
    }
    Ok(())
}

fn uninstall() -> Result<(), String> {
    let home = tama_home();
    let active = fs::read_to_string(home.join("active")).map_err(|err| err.to_string())?;
    let dir = home.join("versions").join(active.trim());
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn verify_manifest_signature(manifest: &[u8], signature: &[u8]) -> Result<(), String> {
    let key = PublicKey::from_base64(EMBEDDED_PUBLIC_KEY).map_err(|err| err.to_string())?;
    let sig_text = std::str::from_utf8(signature).map_err(|err| err.to_string())?;
    let sig = Signature::decode(sig_text).map_err(|err| err.to_string())?;
    key.verify(manifest, &sig, false)
        .map_err(|err| err.to_string())
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), String> {
    let actual = hex::encode(Sha256::digest(bytes));
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "bad artifact SHA-256: expected {expected}, got {actual}"
        ))
    }
}

fn extract_archive(bytes: &[u8], dest: &Utf8Path) -> Result<(), String> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    let mut has_tama = false;
    let mut has_tamaup = false;
    for entry in archive.entries().map_err(|err| err.to_string())? {
        let mut entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path().map_err(|err| err.to_string())?;
        if path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(format!(
                "archive path escapes install dir: {}",
                path.display()
            ));
        }
        let path = Utf8PathBuf::from_path_buf(path.to_path_buf())
            .map_err(|path| path.display().to_string())?;
        match path.as_str() {
            "bin/tama" | "tama" => has_tama = true,
            "bin/tamaup" | "tamaup" => has_tamaup = true,
            _ => return Err(format!("unexpected archive entry: {path}")),
        }
        if !entry.header().entry_type().is_file() {
            return Err(format!("unexpected archive entry: {path}"));
        }
        let mode = entry.header().mode().unwrap_or(0o755) & 0o777;
        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|err| err.to_string())?;
        entries.push(PendingArchiveEntry {
            path,
            mode,
            content,
        });
    }
    if !has_tama || !has_tamaup {
        return Err("archive is missing expected tama or tamaup binary".to_string());
    }
    for entry in entries {
        let out = dest.join(&entry.path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        fs::write(&out, entry.content).map_err(|err| err.to_string())?;
        #[cfg(unix)]
        fs::set_permissions(&out, fs::Permissions::from_mode(entry.mode))
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url).call().map_err(|err| err.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    Ok(bytes)
}

fn tama_home() -> Utf8PathBuf {
    std::env::var("TAMAUP_HOME")
        .ok()
        .map(Utf8PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| Utf8PathBuf::from(home).join(".tama"))
        })
        .unwrap_or_else(|| Utf8PathBuf::from(".tama"))
}

fn platform() -> Result<String, String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        other => return Err(format!("unsupported OS for Tama v0.1: {other}")),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => return Err(format!("unsupported architecture for Tama v0.1: {other}")),
    };
    Ok(format!("{os}-{arch}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_sha_fails() {
        assert!(verify_sha256(b"abc", "deadbeef").is_err());
    }

    #[test]
    fn platform_is_not_windows() {
        let platform = platform().unwrap();
        assert!(!platform.contains("windows"));
    }

    #[test]
    fn use_version_updates_active_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let version_bin = home.join("versions/0.1.0/bin");
        fs::create_dir_all(&version_bin).unwrap();
        fs::write(version_bin.join("tama"), b"tama").unwrap();
        fs::write(version_bin.join("tamaup"), b"tamaup").unwrap();
        use_version_at(&home, "0.1.0").unwrap();
        assert_eq!(fs::read_to_string(home.join("active")).unwrap(), "0.1.0");
        assert!(home.join("bin/tama").exists());
        assert!(home.join("bin/tamaup").exists());
    }

    #[test]
    #[cfg(unix)]
    fn extract_archive_preserves_executable_mode() {
        let tarball = test_archive(&[
            ("bin/tama", b"tama".as_slice(), 0o755),
            ("bin/tamaup", b"tamaup".as_slice(), 0o755),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        extract_archive(&tarball, &dest).unwrap();
        let mode = fs::metadata(dest.join("bin/tama"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
    }

    #[test]
    fn extract_archive_rejects_incomplete_archive_before_writing() {
        let tarball = test_archive(&[("bin/tama", b"tama".as_slice(), 0o755)]);
        let dir = tempfile::tempdir().unwrap();
        let dest = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let err = extract_archive(&tarball, &dest).unwrap_err();

        assert!(err.contains("missing expected"));
        assert!(!dest.join("bin/tama").exists());
    }

    #[test]
    fn extract_archive_rejects_bad_entry_before_writing() {
        let tarball = test_archive(&[
            ("bin/tama", b"tama".as_slice(), 0o755),
            ("bin/extra", b"evil".as_slice(), 0o755),
            ("bin/tamaup", b"tamaup".as_slice(), 0o755),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let err = extract_archive(&tarball, &dest).unwrap_err();

        assert!(err.contains("unexpected archive entry"));
        assert!(!dest.join("bin/tama").exists());
        assert!(!dest.join("bin/tamaup").exists());
    }

    #[test]
    fn extract_archive_rejects_traversal_before_writing() {
        let tarball = test_archive_with_raw_name(b"../evil", b"evil");
        let dir = tempfile::tempdir().unwrap();
        let dest = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let err = extract_archive(&tarball, &dest).unwrap_err();

        assert!(err.contains("escapes install dir"));
        assert!(!dest.join("evil").exists());
    }

    fn test_archive(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut tarball = Vec::new();
        {
            let encoder =
                flate2::write::GzEncoder::new(&mut tarball, flate2::Compression::default());
            let mut archive = tar::Builder::new(encoder);
            for (path, contents, mode) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_path(path).unwrap();
                header.set_size(contents.len() as u64);
                header.set_mode(*mode);
                header.set_cksum();
                archive.append(&header, *contents).unwrap();
            }
            archive.finish().unwrap();
        }
        tarball
    }

    fn test_archive_with_raw_name(raw_name: &[u8], contents: &[u8]) -> Vec<u8> {
        let mut tarball = Vec::new();
        {
            let encoder =
                flate2::write::GzEncoder::new(&mut tarball, flate2::Compression::default());
            let mut archive = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.as_gnu_mut().unwrap().name[..raw_name.len()].copy_from_slice(raw_name);
            header.set_cksum();
            archive.append(&header, contents).unwrap();
            archive.finish().unwrap();
        }
        tarball
    }
}
