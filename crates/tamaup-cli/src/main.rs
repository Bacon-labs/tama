use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Read};
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::ExitCode;

use camino::{Utf8Path, Utf8PathBuf};
#[cfg(test)]
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod style;

use style::{paint, ColorChoice, Palette, Stream};

const DEFAULT_BASE_URL: &str = "https://github.com/bacon-labs/tama/releases/latest/download";
const RELEASE_MANIFEST_SCHEMA: &str = "tama.release-manifest.v1";

#[derive(Debug, Parser)]
#[command(name = "tamaup", version, about = "Install and update Tama")]
struct Cli {
    #[arg(long, global = true)]
    offline: bool,
    #[arg(
        long,
        global = true,
        value_name = "WHEN",
        value_enum,
        default_value_t = ColorChoice::Auto,
        help = "Control colored output: auto, always, never"
    )]
    color: ColorChoice,
    #[arg(long, global = true, hide = true, help = "Alias for --color=never")]
    no_color: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Install a signed Tama release")]
    Install {
        version: Option<String>,
        #[arg(long)]
        manifest_file: Option<Utf8PathBuf>,
    },
    #[command(about = "Switch the active Tama version")]
    Use { version: String },
    #[command(about = "List installed Tama versions")]
    List,
    #[command(name = "self", about = "Manage tamaup itself")]
    Self_ {
        #[command(subcommand)]
        command: SelfCommand,
    },
    #[command(about = "Remove active tama while keeping tamaup")]
    Uninstall,
}

#[derive(Debug, Subcommand)]
enum SelfCommand {
    #[command(about = "Update tamaup to the latest stable release")]
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseManifest {
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    stable: Option<String>,
    #[serde(default)]
    nightly: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifacts: Option<Vec<Artifact>>,
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    version: String,
    artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    let mut cli = Cli::parse();
    if cli.no_color {
        cli.color = ColorChoice::Never;
    }
    style::apply_env(cli.color);
    let stderr_palette = Palette::new(style::resolve(cli.color, Stream::Stderr));
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            anstream::eprintln!("{} {err}", paint(stderr_palette.error_prefix, "error:"));
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let palette = Palette::new(style::resolve(cli.color, Stream::Stdout));
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
            &palette,
        ),
        Command::Use { version } => use_version(&version, &palette),
        Command::List => list_versions(&palette),
        Command::Self_ {
            command: SelfCommand::Update,
        } => install("stable", None, cli.offline, &palette),
        Command::Uninstall => uninstall(),
    }
}

fn install(
    version: &str,
    manifest_file: Option<Utf8PathBuf>,
    offline: bool,
    palette: &Palette,
) -> Result<(), String> {
    let local_manifest = manifest_file.is_some();
    let manifest_bytes = if let Some(path) = manifest_file {
        fs::read(&path).map_err(|err| err.to_string())?
    } else {
        if offline {
            return Err("offline install requires --manifest-file".to_string());
        }
        download(&format!("{DEFAULT_BASE_URL}/manifest.json"))?
    };
    let manifest: ReleaseManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|err| err.to_string())?;
    validate_release_manifest(&manifest)?;
    if !local_manifest {
        validate_published_release_manifest(&manifest)?;
    }
    let platform = platform()?;
    let (selected_version, artifact) = select_artifact(&manifest, &platform, version)?;
    let archive = if artifact.url.starts_with("file://") {
        fs::read(artifact.url.trim_start_matches("file://")).map_err(|err| err.to_string())?
    } else {
        if offline {
            return Err("offline install cannot download artifact".to_string());
        }
        download(&artifact.url)?
    };
    verify_sha256(&archive, &artifact.sha256)?;
    let home = tama_home();
    install_archive_at(&home, &selected_version, &archive)?;
    use_version_at(&home, &selected_version)?;
    anstream::println!(
        "{} {}",
        paint(palette.ok, "Installed Tama"),
        paint(palette.count, &selected_version),
    );
    Ok(())
}

fn select_artifact(
    manifest: &ReleaseManifest,
    platform: &str,
    requested: &str,
) -> Result<(String, Artifact), String> {
    if !manifest.releases.is_empty() {
        let version = channel_or_version(manifest, requested)?;
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
    let artifacts = manifest
        .artifacts
        .as_deref()
        .ok_or_else(|| "legacy release manifest is missing artifacts".to_string())?;
    let artifact = artifacts
        .iter()
        .find(|artifact| artifact.platform == platform)
        .ok_or_else(|| format!("no artifact for {platform} version {requested}"))?;
    Ok((manifest_version.to_string(), artifact.clone()))
}

fn channel_or_version<'a>(
    manifest: &'a ReleaseManifest,
    requested: &'a str,
) -> Result<&'a str, String> {
    match requested {
        "stable" => manifest
            .stable
            .as_deref()
            .ok_or_else(|| "release manifest is missing stable version".to_string()),
        "nightly" => manifest
            .nightly
            .as_deref()
            .ok_or_else(|| "release manifest is missing nightly version".to_string()),
        _ => Ok(requested),
    }
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
    if let Some(nightly) = &manifest.nightly {
        validate_release_version(nightly)?;
    }
    if let Some(version) = &manifest.version {
        validate_release_version(version)?;
    }
    validate_artifacts(manifest.artifacts.as_deref().unwrap_or(&[]))?;
    let mut release_versions = BTreeSet::new();
    for release in &manifest.releases {
        validate_release_version(&release.version)?;
        if !release_versions.insert(release.version.clone()) {
            return Err(format!(
                "duplicate release version `{}` in release manifest",
                release.version
            ));
        }
        if release.artifacts.is_empty() {
            return Err(format!(
                "release `{}` is missing artifacts",
                release.version
            ));
        }
        validate_artifacts(&release.artifacts)?;
    }
    let cumulative_shape = manifest.schema.is_some()
        || manifest.stable.is_some()
        || manifest.nightly.is_some()
        || !manifest.releases.is_empty();
    if cumulative_shape {
        if manifest.schema.as_deref() != Some(RELEASE_MANIFEST_SCHEMA) {
            return Err(
                "cumulative release manifest must declare schema `tama.release-manifest.v1`"
                    .to_string(),
            );
        }
        if manifest.stable.is_none() {
            return Err("cumulative release manifest is missing stable version".to_string());
        }
        if manifest.releases.is_empty() {
            return Err("cumulative release manifest must contain releases[]".to_string());
        }
        for (channel, version) in [
            ("stable", manifest.stable.as_deref()),
            ("nightly", manifest.nightly.as_deref()),
        ] {
            if let Some(version) = version {
                if !release_versions.contains(version) {
                    return Err(format!(
                        "release manifest {channel} channel points to missing release `{version}`"
                    ));
                }
            }
        }
        if manifest.version.is_some() || manifest.artifacts.is_some() {
            return Err(
                "cumulative release manifest must not mix legacy version/artifacts fields"
                    .to_string(),
            );
        }
    } else {
        if manifest.version.is_none() {
            return Err("legacy release manifest is missing version".to_string());
        }
        if manifest
            .artifacts
            .as_deref()
            .map_or(true, |artifacts| artifacts.is_empty())
        {
            return Err("legacy release manifest is missing artifacts".to_string());
        }
    }
    Ok(())
}

fn validate_published_release_manifest(manifest: &ReleaseManifest) -> Result<(), String> {
    if manifest.schema.as_deref() != Some(RELEASE_MANIFEST_SCHEMA)
        || manifest.version.is_some()
        || manifest.artifacts.is_some()
        || manifest.releases.is_empty()
    {
        return Err(
            "published release manifest must be cumulative `tama.release-manifest.v1`".to_string(),
        );
    }
    for release in &manifest.releases {
        for artifact in &release.artifacts {
            if !artifact.url.starts_with("https://") {
                return Err(format!(
                    "published artifact URL must use https:// with a host for {} {}",
                    release.version, artifact.platform
                ));
            }
        }
    }
    Ok(())
}

fn use_version(version: &str, palette: &Palette) -> Result<(), String> {
    use_version_at(&tama_home(), version)?;
    anstream::println!(
        "{} {}",
        paint(palette.header, "Active Tama version:"),
        paint(palette.count, version),
    );
    Ok(())
}

fn use_version_at(home: &Utf8Path, version: &str) -> Result<(), String> {
    validate_release_version(version)?;
    let active = home.join("active");
    let bin = home.join("bin");
    let version_dir = home.join("versions").join(version);
    if !version_dir.is_dir() {
        return Err(format!("Tama version `{version}` is not installed"));
    }
    let targets = [
        ("tama", binary_path(&version_dir, "tama")?),
        ("tamaup", binary_path(&version_dir, "tamaup")?),
    ];
    fs::create_dir_all(&bin).map_err(|err| err.to_string())?;
    for (binary, target) in targets {
        atomic_symlink(&target, &bin.join(binary))?;
    }
    atomic_write(&active, version.as_bytes())?;
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

fn list_versions(palette: &Palette) -> Result<(), String> {
    let home = tama_home();
    for (name, active) in installed_versions_at(&home)? {
        if active {
            anstream::println!(
                "{} {}",
                paint(palette.ok, "*"),
                paint(palette.header, &name),
            );
        } else {
            anstream::println!("  {name}");
        }
    }
    Ok(())
}

fn installed_versions_at(home: &Utf8Path) -> Result<Vec<(String, bool)>, String> {
    let active = read_active_version(home)?;
    let versions = home.join("versions");
    if !versions.is_dir() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in fs::read_dir(&versions).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        if !entry.file_type().map_err(|err| err.to_string())?.is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| format!("installed version entry `{name:?}` is not UTF-8"))?;
        if validate_release_version(&name).is_err() {
            continue;
        }
        let version_dir = versions.join(&name);
        if binary_path(&version_dir, "tama").is_err()
            || binary_path(&version_dir, "tamaup").is_err()
        {
            continue;
        }
        rows.push((name.clone(), active.as_deref() == Some(name.as_str())));
    }
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(rows)
}

fn uninstall() -> Result<(), String> {
    let home = tama_home();
    uninstall_at(&home)
}

fn uninstall_at(home: &Utf8Path) -> Result<(), String> {
    let active = read_active_version(home)?;
    remove_file_if_exists(&home.join("bin/tama"))?;
    if let Some(active) = active {
        remove_file_if_exists(&home.join("versions").join(active).join("bin/tama"))?;
    }
    remove_file_if_exists(&home.join("active"))
}

fn read_active_version(home: &Utf8Path) -> Result<Option<String>, String> {
    let path = home.join("active");
    let active = match fs::read_to_string(&path) {
        Ok(active) => active,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("failed to read `{path}`: {err}")),
    };
    let active = active.trim();
    if active.is_empty() {
        return Ok(None);
    }
    validate_release_version(active)?;
    Ok(Some(active.to_string()))
}

fn remove_file_if_exists(path: &Utf8Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to remove `{path}`: {err}")),
    }
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), String> {
    validate_sha256(expected)?;
    let actual = hex::encode(Sha256::digest(bytes));
    if actual == expected {
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
    let mut platforms = BTreeSet::new();
    for artifact in artifacts {
        validate_artifact_platform(&artifact.platform)?;
        if !platforms.insert(artifact.platform.clone()) {
            return Err(format!(
                "duplicate artifact platform `{}` in release manifest",
                artifact.platform
            ));
        }
        validate_artifact_url(&artifact.url)?;
        validate_sha256(&artifact.sha256)?;
    }
    Ok(())
}

fn validate_artifact_platform(platform: &str) -> Result<(), String> {
    validate_manifest_string(platform, "artifact platform")?;
    if !platform
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '+' | '-'))
    {
        return Err(format!("unsafe artifact platform `{platform}`"));
    }
    Ok(())
}

fn validate_artifact_url(url: &str) -> Result<(), String> {
    validate_manifest_string(url, "artifact URL")?;
    if let Some(path) = url.strip_prefix("file://") {
        if path.is_empty() || !path.starts_with('/') {
            return Err(format!(
                "file artifact URL must use an absolute path `{url}`"
            ));
        }
    } else if let Some(rest) = url.strip_prefix("https://") {
        if !rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        {
            return Err(format!("https artifact URL must include a host `{url}`"));
        }
    } else {
        return Err(format!("unsupported artifact URL `{url}`"));
    }
    Ok(())
}

fn validate_release_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || !version
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        || !version
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        || version.contains("..")
        || !version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '+' | '-'))
    {
        return Err(format!("unsafe release version `{version}`"));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
    {
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
            "bin/tama" | "tama" => {
                if has_tama {
                    return Err("archive contains duplicate tama binary entries".to_string());
                }
                has_tama = true;
            }
            "bin/tamaup" | "tamaup" => {
                if has_tamaup {
                    return Err("archive contains duplicate tamaup binary entries".to_string());
                }
                has_tamaup = true;
            }
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
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x86_64".to_string()),
        ("linux", "aarch64") => Ok("linux-aarch64".to_string()),
        ("macos", "aarch64") => Ok("macos-aarch64".to_string()),
        (os, arch) => Err(format!("unsupported platform for Tama v0.1: {os}-{arch}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct EnvVarGuard {
        key: &'static str,
        old: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.old {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn bad_sha_fails() {
        assert!(verify_sha256(b"abc", "deadbeef").is_err());
        assert!(validate_sha256(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        )
        .is_err());
    }

    #[test]
    fn platform_is_not_windows() {
        let platform = platform().unwrap();
        assert!(!platform.contains("windows"));
    }


    #[test]
    fn website_install_command_matches_spec() {
        let root = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let spec = fs::read_to_string(root.join("docs/reference/SPEC.md").as_std_path()).unwrap();
        let install_page =
            fs::read_to_string(root.join("site/src/pages/install.astro").as_std_path()).unwrap();
        let home_page =
            fs::read_to_string(root.join("site/src/pages/index.astro").as_std_path()).unwrap();
        let pages_installer =
            fs::read_to_string(root.join("site/public/install.sh").as_std_path()).unwrap();
        let installer =
            fs::read_to_string(root.join("installer/install.sh").as_std_path()).unwrap();
        let install_command = "curl -L https://tama.tools/install.sh | sh";

        assert!(spec.contains(install_command));
        assert!(install_page.contains(install_command));
        assert!(home_page.contains(install_command));

        // site/public/install.sh is the published copy served at
        // https://tama.tools/install.sh. The deploy workflow re-syncs it from
        // installer/install.sh during the build, but the tracked copy must
        // also match so a stale checkout can't drift unnoticed.
        assert_eq!(
            pages_installer, installer,
            "site/public/install.sh must match installer/install.sh; \
             run `cp installer/install.sh site/public/install.sh`",
        );
    }

    #[test]
    fn release_base_url_matches_across_installer_tamaup_and_workflow() {
        let root = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let installer =
            fs::read_to_string(root.join("installer/install.sh").as_std_path()).unwrap();
        let installer_base_url = installer
            .lines()
            .find_map(|line| {
                line.strip_prefix("BASE_URL=\"")
                    .and_then(|s| s.strip_suffix('"'))
            })
            .expect("installer/install.sh must declare BASE_URL=\"...\" on a single line");
        assert_eq!(
            installer_base_url, DEFAULT_BASE_URL,
            "tamaup DEFAULT_BASE_URL must match installer/install.sh BASE_URL",
        );

        let workflow =
            fs::read_to_string(root.join(".github/workflows/release.yml").as_std_path()).unwrap();
        let manifest_url = format!("{installer_base_url}/manifest.json");
        assert!(
            workflow.contains(&manifest_url),
            "release.yml must fetch the previous manifest from {manifest_url}",
        );
        let archive_url_prefix = installer_base_url
            .strip_suffix("/latest/download")
            .expect("BASE_URL must end with /latest/download");
        let archive_url_template = format!("{archive_url_prefix}/download/v");
        assert!(
            workflow.contains(&archive_url_template),
            "release.yml must publish artifact URLs under {archive_url_template}<version>/...",
        );
    }

    #[test]
    fn help_lists_command_descriptions() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("Install a signed Tama release"));
        assert!(help.contains("Switch the active Tama version"));
        assert!(help.contains("List installed Tama versions"));
        assert!(help.contains("Remove active tama while keeping tamaup"));
    }

    fn valid_artifact(platform: &str) -> Artifact {
        Artifact {
            platform: platform.to_string(),
            url: format!("file:///tmp/tama-{platform}.tar.gz"),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        }
    }

    fn valid_release(version: &str) -> Release {
        Release {
            version: version.to_string(),
            artifacts: vec![valid_artifact("linux-x86_64")],
        }
    }

    #[test]
    fn release_manifest_selects_stable_and_specific_versions() {
        let manifest = ReleaseManifest {
            schema: Some(RELEASE_MANIFEST_SCHEMA.to_string()),
            stable: Some("0.2.0".to_string()),
            nightly: Some("0.3.0-nightly".to_string()),
            version: None,
            artifacts: None,
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
                Release {
                    version: "0.3.0-nightly".to_string(),
                    artifacts: vec![Artifact {
                        platform: "linux-x86_64".to_string(),
                        url: "file:///tmp/tama-nightly.tar.gz".to_string(),
                        sha256: "nightly".to_string(),
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

        let (version, artifact) = select_artifact(&manifest, "linux-x86_64", "nightly").unwrap();
        assert_eq!(version, "0.3.0-nightly");
        assert_eq!(artifact.sha256, "nightly");
    }

    #[test]
    fn release_manifest_rejects_unknown_schema() {
        let manifest = ReleaseManifest {
            schema: Some("tama.release-manifest.v2".to_string()),
            stable: Some("0.1.0".to_string()),
            nightly: None,
            version: None,
            artifacts: None,
            releases: vec![],
        };

        let err = validate_release_manifest(&manifest).unwrap_err();
        assert!(err.contains("unsupported release manifest schema"));
    }

    #[test]
    fn release_manifest_rejects_unknown_fields() {
        let err = serde_json::from_str::<ReleaseManifest>(
            r#"{
  "schema": "tama.release-manifest.v1",
  "stable": "0.1.0",
  "extra": true,
  "releases": []
}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown field"));

        let err = serde_json::from_str::<ReleaseManifest>(
            r#"{
  "schema": "tama.release-manifest.v1",
  "stable": "0.1.0",
  "releases": [{
    "version": "0.1.0",
    "extra": true,
    "artifacts": []
  }]
}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown field"));

        let err = serde_json::from_str::<ReleaseManifest>(
            r#"{
  "schema": "tama.release-manifest.v1",
  "stable": "0.1.0",
  "releases": [{
    "version": "0.1.0",
    "artifacts": [{
      "platform": "linux-x86_64",
      "url": "file:///tmp/tama.tar.gz",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "extra": true
    }]
  }]
}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn release_manifest_rejects_ambiguous_shapes() {
        let mut manifest = ReleaseManifest {
            schema: Some(RELEASE_MANIFEST_SCHEMA.to_string()),
            stable: Some("0.1.0".to_string()),
            nightly: None,
            version: None,
            artifacts: None,
            releases: vec![],
        };
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("releases"));

        manifest.releases = vec![Release {
            version: "0.1.0".to_string(),
            artifacts: vec![Artifact {
                platform: "linux-x86_64".to_string(),
                url: "file:///tmp/tama.tar.gz".to_string(),
                sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            }],
        }];
        manifest.version = Some("0.1.0".to_string());
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("must not mix"));

        manifest.schema = None;
        manifest.stable = None;
        manifest.version = None;
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("must declare schema"));

        manifest.version = Some("0.1.0".to_string());
        manifest.artifacts = Some(vec![Artifact {
            platform: "linux-x86_64".to_string(),
            url: "file:///tmp/tama.tar.gz".to_string(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        }]);
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("must declare schema"));

        let manifest = serde_json::from_str::<ReleaseManifest>(
            r#"{
  "schema": "tama.release-manifest.v1",
  "stable": "0.1.0",
  "artifacts": [],
  "releases": [{
    "version": "0.1.0",
    "artifacts": [{
      "platform": "linux-x86_64",
      "url": "file:///tmp/tama.tar.gz",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
    }]
  }]
}"#,
        )
        .unwrap();
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("must not mix"));
    }

    #[test]
    fn release_manifest_rejects_empty_release_artifacts() {
        let manifest = ReleaseManifest {
            schema: Some(RELEASE_MANIFEST_SCHEMA.to_string()),
            stable: Some("0.1.0".to_string()),
            nightly: None,
            version: None,
            artifacts: None,
            releases: vec![Release {
                version: "0.1.0".to_string(),
                artifacts: vec![],
            }],
        };

        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("missing artifacts"));
    }

    #[test]
    fn release_manifest_rejects_ambiguous_release_entries() {
        let mut manifest = ReleaseManifest {
            schema: Some(RELEASE_MANIFEST_SCHEMA.to_string()),
            stable: Some("0.1.0".to_string()),
            nightly: None,
            version: None,
            artifacts: None,
            releases: vec![valid_release("0.1.0"), valid_release("0.1.0")],
        };
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("duplicate release version"));

        manifest.releases = vec![Release {
            version: "0.1.0".to_string(),
            artifacts: vec![
                valid_artifact("linux-x86_64"),
                valid_artifact("linux-x86_64"),
            ],
        }];
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("duplicate artifact platform"));

        manifest.releases = vec![valid_release("0.1.0")];
        manifest.stable = Some("0.2.0".to_string());
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("stable channel points to missing release"));

        manifest.stable = Some("0.1.0".to_string());
        manifest.nightly = Some("0.2.0-nightly".to_string());
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("nightly channel points to missing release"));
    }

    #[test]
    fn release_manifest_rejects_unsafe_fields() {
        let mut manifest = ReleaseManifest {
            schema: Some(RELEASE_MANIFEST_SCHEMA.to_string()),
            stable: Some("../evil".to_string()),
            nightly: None,
            version: None,
            artifacts: None,
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

        manifest.releases[0].artifacts[0].sha256 =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        manifest.releases[0].artifacts[0].url = "http://example.invalid/tama.tar.gz".to_string();
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("unsupported artifact URL"));

        manifest.releases[0].artifacts[0].url = "https:///tama.tar.gz".to_string();
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("include a host"));

        manifest.releases[0].artifacts[0].url = "https://example.invalid/tama.tar.gz".to_string();
        manifest.releases[0].artifacts[0].platform = "linux/x86_64".to_string();
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("unsafe artifact platform"));

        manifest.releases[0].artifacts[0].platform = "linux-x86_64".to_string();
        manifest.releases[0].artifacts[0].url = "file://relative/tama.tar.gz".to_string();
        assert!(validate_release_manifest(&manifest)
            .unwrap_err()
            .contains("absolute path"));
    }

    #[test]
    fn published_release_manifest_rejects_local_artifact_urls() {
        let manifest = ReleaseManifest {
            schema: Some(RELEASE_MANIFEST_SCHEMA.to_string()),
            stable: Some("0.1.0".to_string()),
            nightly: None,
            version: None,
            artifacts: None,
            releases: vec![valid_release("0.1.0")],
        };

        validate_release_manifest(&manifest).unwrap();
        assert!(validate_published_release_manifest(&manifest)
            .unwrap_err()
            .contains("published artifact URL must use https://"));
    }

    #[test]
    fn published_release_manifest_rejects_legacy_shape() {
        let manifest = ReleaseManifest {
            schema: None,
            stable: None,
            nightly: None,
            version: Some("0.1.0".to_string()),
            artifacts: Some(vec![Artifact {
                platform: "linux-x86_64".to_string(),
                url: "https://tama.tools/releases/tama-0.1.0-linux-x86_64.tar.gz".to_string(),
                sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            }]),
            releases: vec![],
        };

        validate_release_manifest(&manifest).unwrap();
        assert!(validate_published_release_manifest(&manifest)
            .unwrap_err()
            .contains("must be cumulative"));
    }

    #[test]
    fn release_manifest_keeps_legacy_single_version_shape() {
        let manifest = ReleaseManifest {
            schema: None,
            stable: None,
            nightly: None,
            version: Some("0.1.0".to_string()),
            artifacts: Some(vec![Artifact {
                platform: "linux-x86_64".to_string(),
                url: "file:///tmp/tama-0.1.0.tar.gz".to_string(),
                sha256: "legacy".to_string(),
            }]),
            releases: vec![],
        };

        let (version, artifact) = select_artifact(&manifest, "linux-x86_64", "stable").unwrap();
        assert_eq!(version, "0.1.0");
        assert_eq!(artifact.sha256, "legacy");
    }

    #[test]
    fn install_accepts_global_flags_after_subcommand() {
        let cli = Cli::try_parse_from([
            "tamaup",
            "install",
            "--offline",
            "--manifest-file",
            "manifest.json",
        ])
        .unwrap();
        assert!(cli.offline);
        match cli.command.unwrap() {
            Command::Install { manifest_file, .. } => {
                assert_eq!(manifest_file.unwrap(), Utf8PathBuf::from("manifest.json"));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::try_parse_from(["tamaup", "install", "0.1.0"]).unwrap();
        match cli.command.unwrap() {
            Command::Install { version, .. } => assert_eq!(version.as_deref(), Some("0.1.0")),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn self_update_command_matches_documented_surface() {
        let cli = Cli::try_parse_from(["tamaup", "self", "update"]).unwrap();

        assert!(matches!(
            cli.command,
            Some(Command::Self_ {
                command: SelfCommand::Update
            })
        ));
    }

    #[test]
    fn install_verifies_manifest_before_extract() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Utf8PathBuf::from_path_buf(dir.path().join("manifest.json")).unwrap();
        fs::write(&manifest, br#"{"version":"0.1.0","artifacts":[]}"#).unwrap();

        let err = install("stable", Some(manifest), true, &Palette::plain()).unwrap_err();

        assert!(!err.is_empty());
    }

    #[test]
    fn install_rejects_bad_sha_before_extract() {
        let _env_lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let home = root.join("home");
        let _home_guard = EnvVarGuard::set("TAMAUP_HOME", home.as_std_path().as_os_str());
        let archive = test_archive(&[
            ("bin/tama", b"fake tama".as_slice(), 0o755),
            ("bin/tamaup", b"fake tamaup".as_slice(), 0o755),
        ]);
        let archive_path = root.join("tama.tar.gz");
        fs::write(&archive_path, &archive).unwrap();
        let manifest_path = root.join("manifest.json");
        let manifest = serde_json::json!({
            "schema": RELEASE_MANIFEST_SCHEMA,
            "stable": "0.1.0",
            "releases": [{
                "version": "0.1.0",
                "artifacts": [{
                    "platform": platform().unwrap(),
                    "url": format!("file://{archive_path}"),
                    "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
                }]
            }]
        });
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        let err = install("stable", Some(manifest_path), true, &Palette::plain()).unwrap_err();

        assert!(err.contains("bad artifact SHA-256"));
        assert!(!home.exists());
    }

    #[test]
    fn install_offline_refuses_remote_artifact() {
        let _env_lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let home = root.join("home");
        let _home_guard = EnvVarGuard::set("TAMAUP_HOME", home.as_std_path().as_os_str());
        let manifest_path = root.join("manifest.json");
        let manifest = serde_json::json!({
            "schema": RELEASE_MANIFEST_SCHEMA,
            "stable": "0.1.0",
            "releases": [{
                "version": "0.1.0",
                "artifacts": [{
                    "platform": platform().unwrap(),
                    "url": "https://example.invalid/tama.tar.gz",
                    "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
                }]
            }]
        });
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        let err = install("stable", Some(manifest_path), true, &Palette::plain()).unwrap_err();

        assert_eq!(err, "offline install cannot download artifact");
        assert!(!home.exists());
    }

    #[test]
    fn manifest_file_installs_local_fake_artifact() {
        let _env_lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let home = root.join("home");
        let _home_guard = EnvVarGuard::set("TAMAUP_HOME", home.as_std_path().as_os_str());
        let archive = test_archive(&[
            ("bin/tama", b"fake tama".as_slice(), 0o755),
            ("bin/tamaup", b"fake tamaup".as_slice(), 0o755),
        ]);
        let archive_path = root.join("tama.tar.gz");
        fs::write(&archive_path, &archive).unwrap();
        let sha256 = hex::encode(Sha256::digest(&archive));
        let manifest_path = root.join("manifest.json");
        let manifest = serde_json::json!({
            "schema": RELEASE_MANIFEST_SCHEMA,
            "stable": "0.1.0",
            "releases": [{
                "version": "0.1.0",
                "artifacts": [{
                    "platform": platform().unwrap(),
                    "url": format!("file://{archive_path}"),
                    "sha256": sha256
                }]
            }]
        });
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        install("stable", Some(manifest_path), true, &Palette::plain()).unwrap();

        assert!(home.join("versions/0.1.0/bin/tama").is_file());
        assert!(home.join("versions/0.1.0/bin/tamaup").is_file());
        assert!(home.join("bin/tama").exists());
        assert!(home.join("bin/tamaup").exists());
        assert_eq!(fs::read_to_string(home.join("active")).unwrap(), "0.1.0");
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
    fn list_versions_is_sorted_and_filters_non_installs() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        fs::create_dir_all(home.join("versions/0.2.0/bin")).unwrap();
        fs::write(home.join("versions/0.2.0/bin/tama"), b"tama").unwrap();
        fs::write(home.join("versions/0.2.0/bin/tamaup"), b"tamaup").unwrap();
        fs::create_dir_all(home.join("versions/0.1.0")).unwrap();
        fs::write(home.join("versions/0.1.0/tama"), b"tama").unwrap();
        fs::write(home.join("versions/0.1.0/tamaup"), b"tamaup").unwrap();
        fs::create_dir_all(home.join("versions/0.3.0/bin")).unwrap();
        fs::write(home.join("versions/0.3.0/bin/tama"), b"missing tamaup").unwrap();
        fs::create_dir_all(home.join("versions/bad..name/bin")).unwrap();
        fs::write(home.join("versions/bad..name/bin/tama"), b"tama").unwrap();
        fs::write(home.join("versions/bad..name/bin/tamaup"), b"tamaup").unwrap();
        fs::write(home.join("versions/README"), b"not a version").unwrap();
        fs::write(home.join("active"), b"0.2.0").unwrap();

        assert_eq!(
            installed_versions_at(&home).unwrap(),
            vec![("0.1.0".to_string(), false), ("0.2.0".to_string(), true)]
        );
    }

    #[test]
    #[cfg(unix)]
    fn use_version_preflights_binaries_before_switching() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let old_bin = home.join("versions/0.1.0/bin");
        fs::create_dir_all(&old_bin).unwrap();
        fs::write(old_bin.join("tama"), b"old tama").unwrap();
        fs::write(old_bin.join("tamaup"), b"old tamaup").unwrap();
        use_version_at(&home, "0.1.0").unwrap();

        let new_bin = home.join("versions/0.2.0/bin");
        fs::create_dir_all(&new_bin).unwrap();
        fs::write(new_bin.join("tama"), b"new tama").unwrap();

        let err = use_version_at(&home, "0.2.0").unwrap_err();

        assert!(err.contains("tamaup"));
        assert_eq!(fs::read_to_string(home.join("active")).unwrap(), "0.1.0");
        assert_eq!(
            fs::read_link(home.join("bin/tama")).unwrap(),
            old_bin.join("tama")
        );
        assert_eq!(
            fs::read_link(home.join("bin/tamaup")).unwrap(),
            old_bin.join("tamaup")
        );
    }

    #[test]
    fn use_version_rejects_unsafe_version_paths() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let err = use_version_at(&home, "../evil").unwrap_err();

        assert!(err.contains("unsafe release version"));
        assert!(use_version_at(&home, "..")
            .unwrap_err()
            .contains("unsafe release version"));
        assert!(use_version_at(&home, ".")
            .unwrap_err()
            .contains("unsafe release version"));
        assert!(use_version_at(&home, ".hidden")
            .unwrap_err()
            .contains("unsafe release version"));
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
    fn uninstall_rejects_unsafe_active_version_before_removing_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        fs::create_dir_all(home.join("bin")).unwrap();
        fs::write(home.join("bin/tama"), b"tama").unwrap();
        fs::write(home.join("active"), b"../evil").unwrap();

        let err = uninstall_at(&home).unwrap_err();

        assert!(err.contains("unsafe release version"));
        assert!(home.join("bin/tama").exists());
        assert!(home.join("active").exists());
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
    fn extract_archive_rejects_duplicate_binary_roles_before_writing() {
        let tarball = test_archive(&[
            ("bin/tama", b"first".as_slice(), 0o755),
            ("tama", b"second".as_slice(), 0o755),
            ("bin/tamaup", b"tamaup".as_slice(), 0o755),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let err = extract_archive(&tarball, &dest).unwrap_err();

        assert!(err.contains("duplicate tama binary"));
        assert!(!dest.join("bin/tama").exists());
        assert!(!dest.join("tama").exists());
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

    #[test]
    fn extract_archive_rejects_absolute_path_before_writing() {
        let tarball = test_archive_with_raw_name(b"/evil", b"evil");
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
