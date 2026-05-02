use std::fs;
use std::io::{Cursor, Read};
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
    let home = tama_home();
    let active = home.join("active");
    let bin = home.join("bin");
    fs::create_dir_all(&bin).map_err(|err| err.to_string())?;
    fs::write(&active, version).map_err(|err| err.to_string())?;
    println!("Active Tama version: {version}");
    Ok(())
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
        let allowed = matches!(path.as_str(), "bin/tama" | "bin/tamaup" | "tama" | "tamaup");
        if !allowed || !entry.header().entry_type().is_file() {
            return Err(format!("unexpected archive entry: {path}"));
        }
        let out = dest.join(&path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|err| err.to_string())?;
        fs::write(out, content).map_err(|err| err.to_string())?;
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
}
