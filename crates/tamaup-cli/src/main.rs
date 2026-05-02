use std::fs;
use std::io::{Cursor, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::{Command as ProcessCommand, ExitCode, Stdio};

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, Subcommand};
use flate2::read::GzDecoder;
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DEFAULT_BASE_URL: &str = "https://tama.tools";
const EMBEDDED_PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
const DEFAULT_LEAN_TOOLCHAIN: &str = "leanprover/lean4:v4.22.0";
const DEFAULT_LEAN_VERSION: &str = "4.22.0";
const DEFAULT_SOLC_VERSION: &str = "0.8.33";
const RELEASE_MANIFEST_SCHEMA: &str = "tama.release-manifest.v1";

#[derive(Debug, Parser)]
#[command(name = "tamaup", version, about = "Install and update Tama")]
struct Cli {
    #[arg(long, global = true)]
    yes: bool,
    #[arg(long, global = true)]
    offline: bool,
    #[arg(long, global = true)]
    no_modify_path: bool,
    #[arg(long, global = true)]
    no_install_lean: bool,
    #[arg(long, global = true)]
    no_install_foundry: bool,
    #[arg(long, global = true)]
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
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    stable: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    artifacts: Vec<Artifact>,
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Release {
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

#[derive(Debug, Clone, Copy)]
struct BootstrapOptions {
    yes: bool,
    offline: bool,
    no_install_lean: bool,
    no_install_foundry: bool,
    no_install_solc: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct ToolchainPresence {
    lean: bool,
    lake: bool,
    forge: bool,
    solc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BootstrapAction {
    Lean,
    Foundry,
    Solc,
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
    let bootstrap = BootstrapOptions {
        yes: cli.yes,
        offline: cli.offline,
        no_install_lean: cli.no_install_lean,
        no_install_foundry: cli.no_install_foundry,
        no_install_solc: cli.no_install_solc,
    };
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
            bootstrap,
        ),
        Command::Use { version } => use_version(&version),
        Command::List => list_versions(),
        Command::Self_ {
            command: SelfCommand::Update,
        } => install("stable", None, bootstrap),
        Command::Uninstall => uninstall(),
    }
}

fn install(
    version: &str,
    manifest_file: Option<Utf8PathBuf>,
    bootstrap: BootstrapOptions,
) -> Result<(), String> {
    bootstrap_toolchain(bootstrap)?;
    let (manifest_bytes, signature_bytes) = if let Some(path) = manifest_file {
        let sig = path.with_extension("json.minisig");
        (
            fs::read(&path).map_err(|err| err.to_string())?,
            fs::read(&sig).map_err(|err| err.to_string())?,
        )
    } else {
        if bootstrap.offline {
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
    validate_release_manifest(&manifest)?;
    let platform = platform()?;
    let (selected_version, artifact) = select_artifact(&manifest, &platform, version)?;
    let archive = if artifact.url.starts_with("file://") {
        fs::read(artifact.url.trim_start_matches("file://")).map_err(|err| err.to_string())?
    } else {
        if bootstrap.offline {
            return Err("offline install cannot download artifact".to_string());
        }
        download(&artifact.url)?
    };
    verify_sha256(&archive, &artifact.sha256)?;
    let home = tama_home();
    install_archive_at(&home, &selected_version, &archive)?;
    use_version_at(&home, &selected_version)?;
    println!("Installed Tama {selected_version}");
    Ok(())
}

fn select_artifact(
    manifest: &ReleaseManifest,
    platform: &str,
    requested: &str,
) -> Result<(String, Artifact), String> {
    if !manifest.releases.is_empty() {
        let version = if requested == "stable" {
            manifest
                .stable
                .as_deref()
                .ok_or_else(|| "release manifest is missing stable version".to_string())?
        } else {
            requested
        };
        let release = manifest
            .releases
            .iter()
            .find(|release| release.version == version)
            .ok_or_else(|| format!("no release version {version}"))?;
        let artifact = release
            .artifacts
            .iter()
            .find(|artifact| artifact.platform == platform)
            .ok_or_else(|| format!("no artifact for {platform} version {version}"))?;
        return Ok((release.version.clone(), artifact.clone()));
    }

    let Some(manifest_version) = manifest.version.as_deref() else {
        return Err("release manifest has no releases".to_string());
    };
    if requested != "stable" && requested != manifest_version {
        return Err(format!("no artifact for {platform} version {requested}"));
    }
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == platform)
        .ok_or_else(|| format!("no artifact for {platform} version {requested}"))?;
    Ok((manifest_version.to_string(), artifact.clone()))
}

fn validate_release_manifest(manifest: &ReleaseManifest) -> Result<(), String> {
    if let Some(schema) = &manifest.schema {
        if schema != RELEASE_MANIFEST_SCHEMA {
            return Err(format!("unsupported release manifest schema `{schema}`"));
        }
    }
    if let Some(stable) = &manifest.stable {
        validate_release_version(stable)?;
    }
    if let Some(version) = &manifest.version {
        validate_release_version(version)?;
    }
    validate_artifacts(&manifest.artifacts)?;
    for release in &manifest.releases {
        validate_release_version(&release.version)?;
        validate_artifacts(&release.artifacts)?;
    }
    Ok(())
}

fn use_version(version: &str) -> Result<(), String> {
    use_version_at(&tama_home(), version)
}

fn use_version_at(home: &Utf8Path, version: &str) -> Result<(), String> {
    validate_release_version(version)?;
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
    uninstall_at(&home)
}

fn uninstall_at(home: &Utf8Path) -> Result<(), String> {
    let active = fs::read_to_string(home.join("active")).unwrap_or_default();
    let active = active.trim();
    remove_file_if_exists(&home.join("bin/tama"))?;
    if !active.is_empty() {
        validate_release_version(active)?;
        remove_file_if_exists(&home.join("versions").join(active).join("bin/tama"))?;
    }
    remove_file_if_exists(&home.join("active"))
}

fn remove_file_if_exists(path: &Utf8Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to remove `{path}`: {err}")),
    }
}

fn verify_manifest_signature(manifest: &[u8], signature: &[u8]) -> Result<(), String> {
    let key = PublicKey::from_base64(EMBEDDED_PUBLIC_KEY).map_err(|err| err.to_string())?;
    let sig_text = std::str::from_utf8(signature).map_err(|err| err.to_string())?;
    let sig = Signature::decode(sig_text).map_err(|err| err.to_string())?;
    key.verify(manifest, &sig, false)
        .map_err(|err| err.to_string())
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), String> {
    validate_sha256(expected)?;
    let actual = hex::encode(Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "bad artifact SHA-256: expected {expected}, got {actual}"
        ))
    }
}

fn install_archive_at(home: &Utf8Path, version: &str, archive: &[u8]) -> Result<(), String> {
    validate_release_version(version)?;
    let versions = home.join("versions");
    fs::create_dir_all(&versions).map_err(|err| err.to_string())?;
    let version_dir = versions.join(version);
    let suffix = std::process::id();
    let stage = versions.join(format!(".install-{version}-{suffix}"));
    let previous = versions.join(format!(".previous-{version}-{suffix}"));
    remove_dir_if_exists(&stage)?;
    remove_dir_if_exists(&previous)?;

    fs::create_dir_all(&stage).map_err(|err| err.to_string())?;
    if let Err(err) = extract_archive(archive, &stage) {
        let _ = fs::remove_dir_all(&stage);
        return Err(err);
    }

    if version_dir.exists() {
        fs::rename(&version_dir, &previous).map_err(|err| err.to_string())?;
    }
    match fs::rename(&stage, &version_dir) {
        Ok(()) => {
            remove_dir_if_exists(&previous)?;
            Ok(())
        }
        Err(err) => {
            if previous.exists() {
                let _ = fs::rename(&previous, &version_dir);
            }
            Err(err.to_string())
        }
    }
}

fn remove_dir_if_exists(path: &Utf8Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to remove `{path}`: {err}")),
    }
}

fn validate_artifacts(artifacts: &[Artifact]) -> Result<(), String> {
    for artifact in artifacts {
        validate_manifest_string(&artifact.platform, "artifact platform")?;
        validate_manifest_string(&artifact.url, "artifact URL")?;
        validate_sha256(&artifact.sha256)?;
    }
    Ok(())
}

fn validate_release_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || !version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '+' | '-'))
    {
        return Err(format!("unsafe release version `{version}`"));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("invalid artifact SHA-256 `{value}`"));
    }
    Ok(())
}

fn validate_manifest_string(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().any(|ch| ch.is_control()) {
        return Err(format!("unsafe release manifest field `{label}`"));
    }
    Ok(())
}

fn bootstrap_toolchain(opts: BootstrapOptions) -> Result<(), String> {
    let actions = bootstrap_actions(opts, detect_toolchain_presence())?;
    for action in actions {
        match action {
            BootstrapAction::Lean => install_lean_toolchain()?,
            BootstrapAction::Foundry => install_foundry()?,
            BootstrapAction::Solc => install_solc()?,
        }
    }
    Ok(())
}

fn detect_toolchain_presence() -> ToolchainPresence {
    ToolchainPresence {
        lean: command_version_matches("lean", DEFAULT_LEAN_VERSION),
        lake: command_exists("lake"),
        forge: command_exists("forge"),
        solc: command_version_matches("solc", DEFAULT_SOLC_VERSION),
    }
}

fn command_exists(name: &str) -> bool {
    command_version(name).is_some()
}

fn command_version_matches(name: &str, expected: &str) -> bool {
    command_version(name).is_some_and(|output| version_output_matches(&output, expected))
}

fn command_version(name: &str) -> Option<String> {
    let output = ProcessCommand::new(name)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    Some(text.trim().to_string())
}

fn version_output_matches(output: &str, expected: &str) -> bool {
    output
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.')
        .any(|token| token == expected)
}

fn bootstrap_actions(
    opts: BootstrapOptions,
    presence: ToolchainPresence,
) -> Result<Vec<BootstrapAction>, String> {
    let mut actions = Vec::new();
    if (!presence.lean || !presence.lake) && !opts.no_install_lean {
        require_bootstrap_allowed(opts, "Lean/Lake")?;
        actions.push(BootstrapAction::Lean);
    }
    if !presence.forge && !opts.no_install_foundry {
        require_bootstrap_allowed(opts, "Foundry")?;
        actions.push(BootstrapAction::Foundry);
    }
    if !presence.solc && !opts.no_install_solc {
        require_bootstrap_allowed(opts, "solc")?;
        actions.push(BootstrapAction::Solc);
    }
    Ok(actions)
}

fn require_bootstrap_allowed(opts: BootstrapOptions, tool: &str) -> Result<(), String> {
    if opts.offline {
        return Err(format!(
            "{tool} is missing or incompatible and cannot be installed while --offline is set"
        ));
    }
    if !opts.yes {
        return Err(format!(
            "{tool} is missing or incompatible; rerun with --yes to install it or pass the matching --no-install-* flag to skip bootstrap"
        ));
    }
    Ok(())
}

fn install_lean_toolchain() -> Result<(), String> {
    let script = download("https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh")?;
    run_shell_script(
        "elan-init.sh",
        &script,
        &["-y", "--default-toolchain", "none"],
    )?;
    let elan = home_tool(".elan/bin/elan").unwrap_or_else(|| Utf8PathBuf::from("elan"));
    run_command_path(
        &elan,
        &["toolchain", "install", DEFAULT_LEAN_TOOLCHAIN],
        "elan toolchain install",
    )
}

fn install_foundry() -> Result<(), String> {
    let script = download("https://foundry.paradigm.xyz")?;
    run_shell_script("foundryup bootstrap", &script, &[])?;
    let foundryup =
        home_tool(".foundry/bin/foundryup").unwrap_or_else(|| Utf8PathBuf::from("foundryup"));
    run_command_path(&foundryup, &[], "foundryup")
}

fn install_solc() -> Result<(), String> {
    run_command(
        "python3",
        &["-m", "pip", "install", "--user", "solc-select"],
        "python3 -m pip install solc-select",
    )?;
    run_command(
        "solc-select",
        &["install", DEFAULT_SOLC_VERSION],
        "solc-select install",
    )?;
    run_command(
        "solc-select",
        &["use", DEFAULT_SOLC_VERSION],
        "solc-select use",
    )
}

fn home_tool(rel: &str) -> Option<Utf8PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| Utf8PathBuf::from(home).join(rel))
}

fn run_shell_script(name: &str, script: &[u8], args: &[&str]) -> Result<(), String> {
    let mut child = ProcessCommand::new("sh")
        .arg("-s")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| format!("failed to run {name}: {err}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| format!("failed to open stdin for {name}"))?
        .write_all(script)
        .map_err(|err| format!("failed to write {name}: {err}"))?;
    let status = child
        .wait()
        .map_err(|err| format!("failed to wait for {name}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} failed with status {status}"))
    }
}

fn run_command(program: &str, args: &[&str], display: &str) -> Result<(), String> {
    let status = ProcessCommand::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| format!("failed to run {display}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{display} failed with status {status}"))
    }
}

fn run_command_path(path: &Utf8Path, args: &[&str], display: &str) -> Result<(), String> {
    let status = ProcessCommand::new(path.as_std_path())
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| format!("failed to run {display}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{display} failed with status {status}"))
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
    fn bootstrap_requires_consent_or_opt_out_for_missing_tools() {
        let presence = ToolchainPresence {
            lean: false,
            lake: false,
            forge: false,
            solc: false,
        };
        let err = bootstrap_actions(
            BootstrapOptions {
                yes: false,
                offline: false,
                no_install_lean: false,
                no_install_foundry: false,
                no_install_solc: false,
            },
            presence,
        )
        .unwrap_err();
        assert!(err.contains("--yes"));

        let err = bootstrap_actions(
            BootstrapOptions {
                yes: true,
                offline: true,
                no_install_lean: false,
                no_install_foundry: false,
                no_install_solc: false,
            },
            presence,
        )
        .unwrap_err();
        assert!(err.contains("--offline"));

        let actions = bootstrap_actions(
            BootstrapOptions {
                yes: true,
                offline: false,
                no_install_lean: true,
                no_install_foundry: false,
                no_install_solc: true,
            },
            presence,
        )
        .unwrap();
        assert_eq!(actions, vec![BootstrapAction::Foundry]);
    }

    #[test]
    fn bootstrap_treats_incompatible_version_as_missing_tool() {
        let presence = ToolchainPresence {
            lean: true,
            lake: true,
            forge: true,
            solc: false,
        };

        let err = bootstrap_actions(
            BootstrapOptions {
                yes: false,
                offline: false,
                no_install_lean: false,
                no_install_foundry: false,
                no_install_solc: false,
            },
            presence,
        )
        .unwrap_err();
        assert!(err.contains("missing or incompatible"));

        let actions = bootstrap_actions(
            BootstrapOptions {
                yes: true,
                offline: false,
                no_install_lean: false,
                no_install_foundry: false,
                no_install_solc: false,
            },
            presence,
        )
        .unwrap();
        assert_eq!(actions, vec![BootstrapAction::Solc]);
    }

    #[test]
    fn tool_version_matching_uses_exact_version_token() {
        assert!(version_output_matches(
            "Lean (version 4.22.0, x86_64-unknown-linux-gnu)",
            DEFAULT_LEAN_VERSION
        ));
        assert!(version_output_matches(
            "Version: 0.8.33+commit.64118f21",
            DEFAULT_SOLC_VERSION
        ));
        assert!(!version_output_matches(
            "Version: 0.8.330+commit.64118f21",
            DEFAULT_SOLC_VERSION
        ));
    }

    #[test]
    fn release_manifest_selects_stable_and_specific_versions() {
        let manifest = ReleaseManifest {
            schema: Some(RELEASE_MANIFEST_SCHEMA.to_string()),
            stable: Some("0.2.0".to_string()),
            version: None,
            artifacts: vec![],
            releases: vec![
                Release {
                    version: "0.1.0".to_string(),
                    artifacts: vec![Artifact {
                        platform: "linux-x86_64".to_string(),
                        url: "file:///tmp/tama-0.1.0.tar.gz".to_string(),
                        sha256: "old".to_string(),
                    }],
                },
                Release {
                    version: "0.2.0".to_string(),
                    artifacts: vec![Artifact {
                        platform: "linux-x86_64".to_string(),
                        url: "file:///tmp/tama-0.2.0.tar.gz".to_string(),
                        sha256: "new".to_string(),
                    }],
                },
            ],
        };

        let (version, artifact) = select_artifact(&manifest, "linux-x86_64", "stable").unwrap();
        assert_eq!(version, "0.2.0");
        assert_eq!(artifact.sha256, "new");

        let (version, artifact) = select_artifact(&manifest, "linux-x86_64", "0.1.0").unwrap();
        assert_eq!(version, "0.1.0");
        assert_eq!(artifact.sha256, "old");
    }

    #[test]
    fn release_manifest_rejects_unknown_schema() {
        let manifest = ReleaseManifest {
            schema: Some("tama.release-manifest.v2".to_string()),
            stable: Some("0.1.0".to_string()),
            version: None,
            artifacts: vec![],
            releases: vec![],
        };

        let err = validate_release_manifest(&manifest).unwrap_err();
        assert!(err.contains("unsupported release manifest schema"));
    }

    #[test]
    fn release_manifest_rejects_unsafe_fields() {
        let mut manifest = ReleaseManifest {
            schema: Some(RELEASE_MANIFEST_SCHEMA.to_string()),
            stable: Some("../evil".to_string()),
            version: None,
            artifacts: vec![],
            releases: vec![],
        };
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("unsafe release version"));

        manifest.stable = Some("0.1.0".to_string());
        manifest.releases = vec![Release {
            version: "0.1.0".to_string(),
            artifacts: vec![Artifact {
                platform: "linux-x86_64".to_string(),
                url: "file:///tmp/tama.tar.gz\nTAMA_INJECTED=1".to_string(),
                sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            }],
        }];
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("unsafe release manifest field"));

        manifest.releases[0].artifacts[0].url = "file:///tmp/tama.tar.gz".to_string();
        manifest.releases[0].artifacts[0].sha256 = "not-a-sha".to_string();
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("invalid artifact SHA-256"));
    }

    #[test]
    fn release_manifest_keeps_legacy_single_version_shape() {
        let manifest = ReleaseManifest {
            schema: None,
            stable: None,
            version: Some("0.1.0".to_string()),
            artifacts: vec![Artifact {
                platform: "linux-x86_64".to_string(),
                url: "file:///tmp/tama-0.1.0.tar.gz".to_string(),
                sha256: "legacy".to_string(),
            }],
            releases: vec![],
        };

        let (version, artifact) = select_artifact(&manifest, "linux-x86_64", "stable").unwrap();
        assert_eq!(version, "0.1.0");
        assert_eq!(artifact.sha256, "legacy");
    }

    #[test]
    fn install_accepts_global_bootstrap_flags_after_subcommand() {
        let cli = Cli::try_parse_from([
            "tamaup",
            "install",
            "--yes",
            "--offline",
            "--manifest-file",
            "manifest.json",
        ])
        .unwrap();
        assert!(cli.yes);
        assert!(cli.offline);
        match cli.command.unwrap() {
            Command::Install { manifest_file, .. } => {
                assert_eq!(manifest_file.unwrap(), Utf8PathBuf::from("manifest.json"));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::try_parse_from(["tamaup", "install", "0.1.0", "--no-install-solc"]).unwrap();
        assert!(cli.no_install_solc);
        match cli.command.unwrap() {
            Command::Install { version, .. } => assert_eq!(version.as_deref(), Some("0.1.0")),
            other => panic!("unexpected command: {other:?}"),
        }
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
    fn use_version_rejects_unsafe_version_paths() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let err = use_version_at(&home, "../evil").unwrap_err();

        assert!(err.contains("unsafe release version"));
    }

    #[test]
    fn uninstall_removes_tama_but_keeps_tamaup() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let version_bin = home.join("versions/0.1.0/bin");
        fs::create_dir_all(&version_bin).unwrap();
        fs::write(version_bin.join("tama"), b"tama").unwrap();
        fs::write(version_bin.join("tamaup"), b"tamaup").unwrap();
        use_version_at(&home, "0.1.0").unwrap();

        uninstall_at(&home).unwrap();

        assert!(!home.join("bin/tama").exists());
        assert!(!version_bin.join("tama").exists());
        assert!(home.join("bin/tamaup").exists());
        assert!(version_bin.join("tamaup").exists());
        assert!(!home.join("active").exists());
    }

    #[test]
    fn install_archive_replaces_existing_version_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let version_bin = home.join("versions/0.1.0/bin");
        fs::create_dir_all(&version_bin).unwrap();
        fs::write(version_bin.join("tama"), b"old").unwrap();
        fs::write(version_bin.join("tamaup"), b"old").unwrap();
        let tarball = test_archive(&[
            ("bin/tama", b"new-tama".as_slice(), 0o755),
            ("bin/tamaup", b"new-tamaup".as_slice(), 0o755),
        ]);

        install_archive_at(&home, "0.1.0", &tarball).unwrap();

        assert_eq!(
            fs::read(home.join("versions/0.1.0/bin/tama")).unwrap(),
            b"new-tama"
        );
        assert_eq!(
            fs::read(home.join("versions/0.1.0/bin/tamaup")).unwrap(),
            b"new-tamaup"
        );
        assert!(fs::read_dir(home.join("versions"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')));
    }

    #[test]
    fn install_archive_failure_keeps_existing_version() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let version_bin = home.join("versions/0.1.0/bin");
        fs::create_dir_all(&version_bin).unwrap();
        fs::write(version_bin.join("tama"), b"old-tama").unwrap();
        fs::write(version_bin.join("tamaup"), b"old-tamaup").unwrap();
        let tarball = test_archive(&[("bin/tama", b"partial".as_slice(), 0o755)]);

        let err = install_archive_at(&home, "0.1.0", &tarball).unwrap_err();

        assert!(err.contains("missing expected"));
        assert_eq!(
            fs::read(home.join("versions/0.1.0/bin/tama")).unwrap(),
            b"old-tama"
        );
        assert_eq!(
            fs::read(home.join("versions/0.1.0/bin/tamaup")).unwrap(),
            b"old-tamaup"
        );
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
