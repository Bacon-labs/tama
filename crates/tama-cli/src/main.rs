use std::path::PathBuf;
use std::process::{Command as ProcessCommand, ExitCode, Stdio};

use camino::{Utf8Path, Utf8PathBuf};
use clap::{ArgAction, Args, Parser, Subcommand};

const LAKE_PACKAGE_CACHE_ENV: &str = "TAMA_LAKE_PACKAGE_CACHE";

#[derive(Debug, Parser)]
#[command(name = "tama", version, about = "Verity developer toolchain")]
struct Cli {
    #[arg(long, global = true)]
    root: Option<Utf8PathBuf>,
    #[arg(long, global = true)]
    locked: bool,
    #[arg(long, global = true)]
    offline: bool,
    #[arg(long, global = true)]
    json: bool,
    #[arg(short, long, global = true, action = ArgAction::Count)]
    verbose: u8,
    #[arg(long, global = true)]
    no_color: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init {
        path: Option<Utf8PathBuf>,
    },
    New {
        name: String,
    },
    Check,
    Build(BuildArgs),
    Test(TestArgs),
    Audit(AuditArgs),
    Inspect(InspectArgs),
    Clean {
        #[arg(long)]
        deep: bool,
    },
    Doctor {
        #[arg(long)]
        fix: bool,
    },
    Install {
        package: String,
    },
    Remove {
        package: String,
    },
    Update {
        #[arg(long)]
        no_forge: bool,
        #[arg(long)]
        no_lake: bool,
    },
}

#[derive(Debug, Args)]
struct BuildArgs {
    #[arg(long)]
    no_solc: bool,
    #[arg(long)]
    no_forge: bool,
    #[arg(long = "contract")]
    contract_: Option<String>,
}

#[derive(Debug, Args)]
struct TestArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    forge_args: Vec<String>,
}

#[derive(Debug, Args)]
struct AuditArgs {
    check: Option<String>,
    #[arg(long)]
    deny_warnings: bool,
}

#[derive(Debug, Args)]
struct InspectArgs {
    contract: String,
    field: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.no_color {
        std::env::set_var("NO_COLOR", "1");
    }
    tama_common::init_logging(cli.json, matches!(cli.command, Command::Test(_)));
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.command {
        Command::Init { path } => {
            let path = path.unwrap_or_else(|| Utf8PathBuf::from("."));
            let name = path
                .file_name()
                .filter(|name| !name.is_empty())
                .unwrap_or("my-protocol")
                .to_string();
            tama_project::init(
                &path,
                tama_project::InitOptions {
                    name,
                    ..Default::default()
                },
            )
            .map_err(|err| err.to_string())?;
            finalize_init(&path, cli.offline)?;
            println!("Initialized Tama ERC20Lite starter at {path}");
            Ok(ExitCode::SUCCESS)
        }
        Command::New { name } => {
            let root = project_root(cli.root)?;
            tama_project::scaffold_contract(&root, &name).map_err(|err| err.to_string())?;
            println!("Created Verity contract scaffold {name}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Check => {
            let root = project_root(cli.root)?;
            tama_build::Lake::new(root)
                .check_src_and_spec()
                .map_err(|err| err.to_string())?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Build(args) => {
            let root = project_root(cli.root)?;
            let status = tama_build::Pipeline::new(root)
                .run(tama_build::BuildOptions {
                    locked: cli.locked,
                    offline: cli.offline,
                    no_solc: args.no_solc,
                    no_forge: args.no_forge,
                    contract: args.contract_,
                    json: cli.json,
                    verbose: cli.verbose,
                })
                .map_err(|err| err.to_string())?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({ "manifests": status.manifests })
                    )
                    .map_err(|err| err.to_string())?
                );
            } else {
                println!("Build completed for {} manifest(s)", status.manifests.len());
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Test(args) => {
            let root = project_root(cli.root)?;
            let status = tama_toolchain::run_passthrough(
                "forge",
                &prefixed_test_args(args.forge_args),
                &root,
            )
            .map_err(|err| err.to_string())?;
            Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
        }
        Command::Audit(args) => {
            let root = project_root(cli.root)?;
            let check = args
                .check
                .as_deref()
                .map(|raw| {
                    tama_audit::parse_check(raw)
                        .ok_or_else(|| format!("unknown audit check `{raw}`"))
                })
                .transpose()?;
            let report = tama_audit::run(
                &root,
                tama_audit::AuditOptions {
                    check,
                    deny_warnings: args.deny_warnings,
                },
            )
            .map_err(|err| err.to_string())?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?
                );
            } else if report.issues.is_empty() {
                println!("Audit passed");
            } else {
                for issue in &report.issues {
                    println!("{} {}: {}", issue.check, issue.code, issue.message);
                }
            }
            Ok(if report.has_failures(args.deny_warnings) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            })
        }
        Command::Inspect(args) => {
            let root = project_root(cli.root)?;
            let field = tama_inspect::parse_field(&args.field)
                .ok_or_else(|| format!("unknown inspect field `{}`", args.field))?;
            print!(
                "{}",
                tama_inspect::inspect(&root, &args.contract, field, cli.json)
                    .map_err(|err| err.to_string())?
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Clean { deep } => {
            let root = project_root(cli.root)?;
            clean(&root, deep).map_err(|err| err.to_string())?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor { fix } => {
            let root = cli.root.unwrap_or_else(|| Utf8PathBuf::from("."));
            let project = tama_common::find_project_root(&root).ok();
            if fix {
                apply_doctor_fix(&root, project.as_ref())?;
            }
            let report = doctor_report(project.as_ref())?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?
                );
            } else {
                for tool in report.tools {
                    match tool {
                        tama_toolchain::ToolStatus::Ok(tool) => {
                            println!(
                                "ok  {:<8} {}",
                                tool.name,
                                tool.version.unwrap_or_else(|| tool.path.to_string())
                            );
                        }
                        tama_toolchain::ToolStatus::Missing { name, remediation } => {
                            println!("err {:<8} {remediation}", name);
                        }
                        tama_toolchain::ToolStatus::Incompatible {
                            name,
                            found,
                            expected,
                        } => {
                            println!("err {name:<8} found {found}, expected {expected}");
                        }
                    }
                }
                if let Some(lock_current) = report.lock_current {
                    if lock_current {
                        println!("ok  lock     current");
                    } else {
                        println!("err lock     stale or unreadable");
                    }
                }
                for note in &report.notes {
                    println!("note {note}");
                }
                if fix {
                    println!("Applied safe directory repairs");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Install { package } => {
            let root = project_root(cli.root)?;
            install_package(&root, &package, cli.offline, cli.locked)?;
            println!("Installed Tama dependency {package}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Remove { package } => {
            let root = project_root(cli.root)?;
            mutate_dependencies(&root, cli.locked, |root| {
                tama_config::remove_lake_dependency(root, &package).map_err(|err| err.to_string())
            })?;
            println!("Removed Tama dependency {package}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Update { no_forge, no_lake } => {
            let root = project_root(cli.root)?;
            update_project(&root, cli.locked, no_lake, no_forge)?;
            println!("Updated Tama project lock state");
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn doctor_report(project: Option<&Utf8PathBuf>) -> Result<tama_toolchain::DoctorReport, String> {
    let mut report = tama_toolchain::detect_required_tools();
    if let Some(project_root) = project {
        if let Ok(toolchain) = tama_config::read_lean_toolchain(project_root) {
            if let Some(expected) = lean_version_from_toolchain(&toolchain) {
                mark_version_mismatch(
                    &mut report,
                    "lean",
                    &expected,
                    tama_toolchain::parse_lean_version,
                );
            }
        }
        if let Ok(config) = tama_config::load_config(project_root) {
            mark_version_mismatch(
                &mut report,
                "solc",
                &config.yul.solc,
                tama_toolchain::parse_solc_version,
            );
        }
        match tama_config::load_lock(project_root) {
            Ok(lock) => {
                let drift =
                    tama_config::lock_drift(project_root, &lock).map_err(|err| err.to_string())?;
                report.lock_current = Some(drift.is_empty());
                if !drift.is_empty() {
                    report
                        .notes
                        .push(format!("stale lock inputs: {}", drift.join(", ")));
                }
            }
            Err(err) => {
                report.lock_current = Some(false);
                report
                    .notes
                    .push(format!("lockfile could not be read: {err}"));
            }
        }
    }
    Ok(report)
}

fn mark_version_mismatch(
    report: &mut tama_toolchain::DoctorReport,
    name: &str,
    expected: &str,
    parse: fn(&str) -> tama_toolchain::Result<semver::Version>,
) {
    let Ok(expected_version) = tama_toolchain::parse_expected_version(name, expected) else {
        return;
    };
    for status in &mut report.tools {
        let tama_toolchain::ToolStatus::Ok(tool) = status else {
            continue;
        };
        if tool.name != name {
            continue;
        }
        let Some(raw) = &tool.version else {
            continue;
        };
        match parse(raw) {
            Ok(found) if found == expected_version => {}
            Ok(found) => {
                *status = tama_toolchain::ToolStatus::Incompatible {
                    name: name.to_string(),
                    found: found.to_string(),
                    expected: expected_version.to_string(),
                };
            }
            Err(_) => {
                *status = tama_toolchain::ToolStatus::Incompatible {
                    name: name.to_string(),
                    found: raw.clone(),
                    expected: expected_version.to_string(),
                };
            }
        }
    }
}

fn lean_version_from_toolchain(toolchain: &str) -> Option<String> {
    toolchain
        .rsplit_once(":v")
        .map(|(_, version)| version.to_string())
        .or_else(|| {
            toolchain
                .rsplit_once(':')
                .map(|(_, version)| version.to_string())
        })
}

fn apply_doctor_fix(root: &Utf8PathBuf, project: Option<&Utf8PathBuf>) -> Result<(), String> {
    let repair_root = project.unwrap_or(root);
    for dir in ["artifacts", "src/generated/verity"] {
        std::fs::create_dir_all(repair_root.join(dir)).map_err(|err| err.to_string())?;
    }
    if let Some(project_root) = project {
        refresh_lock(project_root)?;
    }
    Ok(())
}

fn finalize_init(root: &Utf8PathBuf, offline: bool) -> Result<(), String> {
    if offline {
        for line in offline_init_instructions() {
            eprintln!("{line}");
        }
        return Ok(());
    }
    run_lake_update(root)?;
    ensure_git_worktree(root)?;
    let mut forge_args = vec!["install", "foundry-rs/forge-std"];
    if let Some(flag) = forge_install_no_commit_flag()? {
        forge_args.push(flag);
    }
    run_tool(root, "forge", &forge_args)?;
    refresh_lock(root)
}

fn install_package(
    root: &Utf8PathBuf,
    package: &str,
    offline: bool,
    locked: bool,
) -> Result<(), String> {
    let dependency =
        tama_config::parse_lake_dependency(root, package).map_err(|err| err.to_string())?;
    if let tama_config::LakeDependencySource::Git { url, rev } = &dependency.source {
        if offline {
            return Err(format!(
                "`tama install {package}` needs network access; use a local path or remove --offline"
            ));
        }
        validate_remote_tama_package(url, rev)?;
    }
    mutate_dependencies(root, locked, |root| {
        tama_config::upsert_lake_dependency(root, &dependency).map_err(|err| err.to_string())
    })
}

fn update_project(
    root: &Utf8PathBuf,
    locked: bool,
    no_lake: bool,
    no_forge: bool,
) -> Result<(), String> {
    if locked {
        let lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
        tama_config::enforce_locked(root, &lock).map_err(|err| err.to_string())?;
    }
    let config = tama_config::load_config(root).map_err(|err| err.to_string())?;
    let mut lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
    let verity_git = lock
        .resolved
        .get("verity_git")
        .cloned()
        .unwrap_or_else(|| "https://github.com/lfglabs-dev/verity.git".to_string());
    let verity_rev = verity_rev_from_config(&config.project.verity);
    let dependency = tama_config::LakeDependency {
        name: "verity".to_string(),
        source: tama_config::LakeDependencySource::Git {
            url: verity_git.clone(),
            rev: verity_rev.clone(),
        },
    };
    tama_config::upsert_lake_dependency(root, &dependency).map_err(|err| err.to_string())?;
    lock.resolved.insert("verity_git".to_string(), verity_git);
    lock.resolved.insert("verity_rev".to_string(), verity_rev);
    if !no_lake {
        run_lake_update(root)?;
    }
    if !no_forge {
        run_tool(root, "forge", &["update"])?;
    }
    tama_config::update_lock_inputs(root, &mut lock).map_err(|err| err.to_string())?;
    tama_config::write_lock(root, &lock).map_err(|err| err.to_string())
}

fn mutate_dependencies(
    root: &Utf8PathBuf,
    locked: bool,
    edit: impl FnOnce(&Utf8PathBuf) -> Result<(), String>,
) -> Result<(), String> {
    if locked {
        let lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
        tama_config::enforce_locked(root, &lock).map_err(|err| err.to_string())?;
    }
    edit(root)?;
    run_lake_update(root)?;
    refresh_lock(root)
}

fn refresh_lock(root: &Utf8PathBuf) -> Result<(), String> {
    let mut lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
    tama_config::update_lock_inputs(root, &mut lock).map_err(|err| err.to_string())?;
    tama_config::write_lock(root, &lock).map_err(|err| err.to_string())
}

fn validate_remote_tama_package(url: &str, rev: &str) -> Result<(), String> {
    let temp = tempfile::tempdir().map_err(|err| err.to_string())?;
    let checkout = Utf8PathBuf::from_path_buf(temp.path().join("package"))
        .map_err(|path| path.display().to_string())?;
    run_process(
        "git",
        &["clone", "--depth", "1", url, checkout.as_str()],
        None,
    )?;
    if rev != "main" {
        run_process(
            "git",
            &["fetch", "--depth", "1", "origin", rev],
            Some(&checkout),
        )?;
        run_process("git", &["checkout", "FETCH_HEAD"], Some(&checkout))?;
    }
    if checkout.join("tama.toml").is_file() {
        Ok(())
    } else {
        Err(format!(
            "remote dependency `{url}` at `{rev}` does not contain tama.toml"
        ))
    }
}

fn run_lake_update(root: &Utf8PathBuf) -> Result<(), String> {
    let Some(cache) = lake_package_cache()? else {
        return run_tool(root, "lake", &["update"]);
    };
    seed_lake_package_cache(root, &cache)?;
    run_tool(root, "lake", &["update"])?;
    sync_lake_package_cache(root, &cache)
}

fn lake_package_cache() -> Result<Option<Utf8PathBuf>, String> {
    if let Some(value) = std::env::var_os(LAKE_PACKAGE_CACHE_ENV) {
        return lake_package_cache_from_override(value);
    }
    default_lake_package_cache().map(Some)
}

fn lake_package_cache_from_override(
    value: impl Into<std::ffi::OsString>,
) -> Result<Option<Utf8PathBuf>, String> {
    let value = value.into();
    if value.as_os_str().is_empty() {
        return Ok(None);
    }
    let raw = value.to_string_lossy();
    if matches!(raw.as_ref(), "0" | "false" | "none" | "off") {
        return Ok(None);
    }
    utf8_path_from_path_buf(PathBuf::from(value), LAKE_PACKAGE_CACHE_ENV).map(Some)
}

fn default_lake_package_cache() -> Result<Utf8PathBuf, String> {
    let base = if cfg!(target_os = "macos") {
        home_dir("HOME")?.join("Library/Caches")
    } else if let Some(cache_home) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(cache_home)
    } else {
        home_dir("HOME")?.join(".cache")
    };
    utf8_path_from_path_buf(base.join("tama/lake-packages"), "default cache directory")
}

fn home_dir(name: &str) -> Result<PathBuf, String> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("cannot determine Tama cache directory because ${name} is unset"))
}

fn utf8_path_from_path_buf(path: PathBuf, label: &str) -> Result<Utf8PathBuf, String> {
    Utf8PathBuf::from_path_buf(path)
        .map_err(|path| format!("{label} must be a UTF-8 path, got `{}`", path.display()))
}

fn seed_lake_package_cache(root: &Utf8Path, cache: &Utf8Path) -> Result<(), String> {
    if !cache.exists() {
        return Ok(());
    }
    let packages = root.join(".lake/packages");
    copy_missing_package_dirs(cache, &packages)
}

fn sync_lake_package_cache(root: &Utf8Path, cache: &Utf8Path) -> Result<(), String> {
    let packages = root.join(".lake/packages");
    if !packages.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(cache).map_err(|err| format!("failed to create `{cache}`: {err}"))?;
    copy_missing_package_dirs(&packages, cache)
}

fn copy_missing_package_dirs(source: &Utf8Path, destination: &Utf8Path) -> Result<(), String> {
    if same_existing_path(source, destination) {
        return Ok(());
    }
    std::fs::create_dir_all(destination)
        .map_err(|err| format!("failed to create `{destination}`: {err}"))?;
    for entry in
        std::fs::read_dir(source).map_err(|err| format!("failed to read `{source}`: {err}"))?
    {
        let entry = entry.map_err(|err| format!("failed to read `{source}` entry: {err}"))?;
        let file_type = entry
            .file_type()
            .map_err(|err| format!("failed to inspect `{}`: {err}", entry.path().display()))?;
        if !file_type.is_dir() {
            continue;
        }
        let source = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|path| format!("package path `{}` is not UTF-8", path.display()))?;
        if !source.join(".git").exists() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| format!("package cache entry `{name:?}` is not UTF-8"))?;
        let target = destination.join(&name);
        if target.exists() {
            continue;
        }
        copy_dir_recursively(&source, &target)?;
    }
    Ok(())
}

fn copy_dir_recursively(source: &Utf8Path, destination: &Utf8Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|err| format!("failed to inspect `{source}`: {err}"))?;
    if !metadata.is_dir() {
        return Err(format!("`{source}` is not a directory"));
    }
    std::fs::create_dir_all(destination)
        .map_err(|err| format!("failed to create `{destination}`: {err}"))?;
    std::fs::set_permissions(destination, metadata.permissions())
        .map_err(|err| format!("failed to set permissions on `{destination}`: {err}"))?;
    for entry in
        std::fs::read_dir(source).map_err(|err| format!("failed to read `{source}`: {err}"))?
    {
        let entry = entry.map_err(|err| format!("failed to read `{source}` entry: {err}"))?;
        let source_path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|path| format!("package path `{}` is not UTF-8", path.display()))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| format!("package cache entry `{name:?}` is not UTF-8"))?;
        let destination_path = destination.join(name);
        let metadata = std::fs::symlink_metadata(&source_path)
            .map_err(|err| format!("failed to inspect `{source_path}`: {err}"))?;
        if metadata.file_type().is_symlink() {
            copy_symlink(&source_path, &destination_path)?;
        } else if metadata.is_dir() {
            copy_dir_recursively(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            std::fs::copy(&source_path, &destination_path)
                .map_err(|err| format!("failed to copy `{source_path}`: {err}"))?;
            std::fs::set_permissions(&destination_path, metadata.permissions()).map_err(|err| {
                format!("failed to set permissions on `{destination_path}`: {err}")
            })?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(source: &Utf8Path, destination: &Utf8Path) -> Result<(), String> {
    let target = std::fs::read_link(source)
        .map_err(|err| format!("failed to read symlink `{source}`: {err}"))?;
    std::os::unix::fs::symlink(&target, destination).map_err(|err| {
        format!(
            "failed to create symlink `{destination}` to `{}`: {err}",
            target.display()
        )
    })
}

#[cfg(not(unix))]
fn copy_symlink(source: &Utf8Path, _destination: &Utf8Path) -> Result<(), String> {
    Err(format!("cannot copy symlink `{source}` on this platform"))
}

fn same_existing_path(left: &Utf8Path, right: &Utf8Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn run_tool(root: &Utf8PathBuf, program: &str, args: &[&str]) -> Result<(), String> {
    run_process(program, args, Some(root))
}

fn run_process(program: &str, args: &[&str], cwd: Option<&Utf8PathBuf>) -> Result<(), String> {
    let mut command = ProcessCommand::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let display = command_display(program, args);
    eprintln!("running `{display}`");
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| format!("failed to run `{display}`: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{display}` failed with status {status}"))
    }
}

fn command_display(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

fn verity_rev_from_config(version: &str) -> String {
    if version.starts_with('v') || looks_like_git_rev(version) {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

fn looks_like_git_rev(value: &str) -> bool {
    value.len() >= 7 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn ensure_git_worktree(root: &Utf8PathBuf) -> Result<(), String> {
    if is_git_worktree(root)? {
        Ok(())
    } else {
        run_tool(root, "git", &["init"])
    }
}

fn is_git_worktree(root: &Utf8PathBuf) -> Result<bool, String> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(root)
        .output()
        .map_err(|err| format!("failed to inspect git worktree: {err}"))?;
    Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true")
}

fn forge_install_no_commit_flag() -> Result<Option<&'static str>, String> {
    let output = ProcessCommand::new("forge")
        .args(["install", "--help"])
        .output()
        .map_err(|err| format!("failed to inspect `forge install --help`: {err}"))?;
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(select_forge_install_no_commit_flag(&help))
}

fn select_forge_install_no_commit_flag(help: &str) -> Option<&'static str> {
    if help.contains("--no-commit") {
        Some("--no-commit")
    } else {
        None
    }
}

fn offline_init_instructions() -> [&'static str; 5] {
    [
        "offline init: skipped `lake update`, `git init` if needed, and `forge install foundry-rs/forge-std`.",
        "when network access is available, run:",
        "  lake update",
        "  git init  # if this project is not already inside a Git worktree",
        "  forge install foundry-rs/forge-std",
    ]
}

fn project_root(root: Option<Utf8PathBuf>) -> Result<Utf8PathBuf, String> {
    let start = match root {
        Some(root) => root,
        None => {
            let cwd = std::env::current_dir().map_err(|err| err.to_string())?;
            Utf8PathBuf::from_path_buf(cwd).map_err(|path| path.display().to_string())?
        }
    };
    let root = tama_common::find_project_root(&start).map_err(|err| err.to_string())?;
    canonicalize_utf8(&root)
}

fn canonicalize_utf8(path: &Utf8Path) -> Result<Utf8PathBuf, String> {
    let path = std::fs::canonicalize(path)
        .map_err(|err| format!("failed to canonicalize `{path}`: {err}"))?;
    Utf8PathBuf::from_path_buf(path).map_err(|path| path.display().to_string())
}

fn prefixed_test_args(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len() + 1);
    out.push("test".to_string());
    out.extend(args);
    out
}

fn clean(root: &Utf8PathBuf, deep: bool) -> std::io::Result<()> {
    for rel in [
        "artifacts/yul",
        "artifacts/bytecode",
        "artifacts/solc-json",
        "artifacts/manifest",
        "artifacts/lean",
        "artifacts/trust-probe",
        "out",
        "cache",
        "src/generated/verity",
    ] {
        let path = root.join(rel);
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
    }
    if deep {
        let path = root.join(".lake");
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verity_versions_default_to_tags() {
        assert_eq!(verity_rev_from_config("0.5.0"), "v0.5.0");
        assert_eq!(verity_rev_from_config("v0.5.0"), "v0.5.0");
        assert_eq!(
            verity_rev_from_config("9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e"),
            "9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e"
        );
    }

    #[test]
    fn test_args_are_prefixed_without_rewriting() {
        assert_eq!(
            prefixed_test_args(vec![
                "--match-test".to_string(),
                "foo".to_string(),
                "-vvv".to_string(),
            ]),
            vec!["test", "--match-test", "foo", "-vvv"]
        );
    }

    #[test]
    fn offline_init_instructions_are_actionable() {
        let instructions = offline_init_instructions().join("\n");
        assert!(instructions.contains("lake update"));
        assert!(instructions.contains("git init"));
        assert!(instructions.contains("forge install foundry-rs/forge-std"));
        assert!(!instructions.contains("--no-git"));
    }

    #[test]
    fn forge_install_flag_preserves_submodule_installs() {
        assert_eq!(
            select_forge_install_no_commit_flag("Options:\n      --no-git\n      --commit\n"),
            None
        );
        assert_eq!(
            select_forge_install_no_commit_flag("Options:\n      --no-commit\n"),
            Some("--no-commit")
        );
        assert_eq!(select_forge_install_no_commit_flag("Options:\n"), None);
    }

    #[test]
    fn provided_project_root_is_canonicalized() {
        let cwd = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir_in(&cwd).unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname='x'\nverity='v'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        let relative = root.as_std_path().strip_prefix(&cwd).unwrap().to_path_buf();
        let relative = Utf8PathBuf::from_path_buf(relative).unwrap();
        assert!(project_root(Some(relative)).unwrap().is_absolute());
    }

    #[test]
    fn ensure_git_worktree_initializes_fresh_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        assert!(!is_git_worktree(&root).unwrap());
        ensure_git_worktree(&root).unwrap();
        assert!(is_git_worktree(&root).unwrap());
        assert!(root.join(".git").is_dir());
    }

    #[test]
    fn lake_package_cache_seeds_and_records_missing_packages() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();

        tama_common::write_string(&cache.join("mathlib/.git/config"), "[remote]\n").unwrap();
        seed_lake_package_cache(&root, &cache).unwrap();
        assert!(root.join(".lake/packages/mathlib/.git/config").is_file());

        tama_common::write_string(
            &root.join(".lake/packages/verity/.git/config"),
            "[remote]\n",
        )
        .unwrap();
        sync_lake_package_cache(&root, &cache).unwrap();
        assert!(cache.join("verity/.git/config").is_file());
    }

    #[test]
    fn lake_package_cache_override_can_disable_default_cache() {
        assert_eq!(lake_package_cache_from_override("").unwrap(), None);
        assert_eq!(lake_package_cache_from_override("off").unwrap(), None);
        assert_eq!(lake_package_cache_from_override("false").unwrap(), None);
        assert_eq!(
            lake_package_cache_from_override("/tmp/tama-cache")
                .unwrap()
                .as_deref(),
            Some(Utf8Path::new("/tmp/tama-cache"))
        );
    }

    #[test]
    fn doctor_fix_refreshes_lock_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        tama_common::write_string(
            &root.join("TamaSrc.lean"),
            "import src.ERC20Lite\n-- changed\n",
        )
        .unwrap();
        let stale = doctor_report(Some(&root)).unwrap();
        assert_eq!(stale.lock_current, Some(false));
        apply_doctor_fix(&root, Some(&root)).unwrap();
        let current = doctor_report(Some(&root)).unwrap();
        assert_eq!(current.lock_current, Some(true));
    }

    #[test]
    fn clean_removes_lake_build_and_deep_removes_lake_cache() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("artifacts/lean")).unwrap();
        std::fs::create_dir_all(root.join(".lake/packages")).unwrap();
        clean(&root, false).unwrap();
        assert!(!root.join("artifacts/lean").exists());
        assert!(root.join(".lake/packages").exists());

        std::fs::create_dir_all(root.join("artifacts/lean")).unwrap();
        clean(&root, true).unwrap();
        assert!(!root.join(".lake").exists());
    }

    #[test]
    fn lean_toolchain_version_is_extracted() {
        assert_eq!(
            lean_version_from_toolchain("leanprover/lean4:v4.22.0").as_deref(),
            Some("4.22.0")
        );
    }

    #[test]
    fn doctor_marks_tool_version_mismatch() {
        let mut report = tama_toolchain::DoctorReport {
            tools: vec![tama_toolchain::ToolStatus::Ok(tama_toolchain::Tool {
                name: "solc".to_string(),
                path: "solc".into(),
                version: Some("Version: 0.8.32+commit.test".to_string()),
            })],
            lock_current: None,
            notes: vec![],
        };
        mark_version_mismatch(
            &mut report,
            "solc",
            "0.8.33",
            tama_toolchain::parse_solc_version,
        );
        assert!(matches!(
            &report.tools[0],
            tama_toolchain::ToolStatus::Incompatible { found, expected, .. }
                if found == "0.8.32" && expected == "0.8.33"
        ));
    }
}
