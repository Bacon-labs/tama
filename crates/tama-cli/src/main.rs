use std::process::{Command as ProcessCommand, ExitCode};

use camino::Utf8PathBuf;
use clap::{ArgAction, Args, Parser, Subcommand};

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
            let mut report = tama_toolchain::detect_required_tools();
            let project = tama_common::find_project_root(&root).ok();
            if let Some(project_root) = &project {
                match tama_config::load_lock(project_root) {
                    Ok(lock) => {
                        let drift = tama_config::lock_drift(project_root, &lock)
                            .map_err(|err| err.to_string())?;
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
                    let repair_root = project.as_ref().unwrap_or(&root);
                    for dir in ["artifacts", "src/generated/verity"] {
                        std::fs::create_dir_all(repair_root.join(dir))
                            .map_err(|err| err.to_string())?;
                    }
                    if let Some(project_root) = &project {
                        let mut lock =
                            tama_config::load_lock(project_root).map_err(|err| err.to_string())?;
                        tama_config::update_lock_inputs(project_root, &mut lock)
                            .map_err(|err| err.to_string())?;
                        tama_config::write_lock(project_root, &lock)
                            .map_err(|err| err.to_string())?;
                    }
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

fn finalize_init(root: &Utf8PathBuf, offline: bool) -> Result<(), String> {
    if offline {
        for line in offline_init_instructions() {
            eprintln!("{line}");
        }
        return Ok(());
    }
    run_tool(root, "lake", &["update"])?;
    run_tool(
        root,
        "forge",
        &["install", "foundry-rs/forge-std", "--no-commit"],
    )?;
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
        run_tool(root, "lake", &["update"])?;
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
    run_tool(root, "lake", &["update"])?;
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

fn run_tool(root: &Utf8PathBuf, program: &str, args: &[&str]) -> Result<(), String> {
    run_process(program, args, Some(root))
}

fn run_process(program: &str, args: &[&str], cwd: Option<&Utf8PathBuf>) -> Result<(), String> {
    let mut command = ProcessCommand::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().map_err(|err| {
        format!(
            "failed to run `{}`: {err}",
            std::iter::once(program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ")
        )
    })?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "`{}` failed with status {}{}",
            std::iter::once(program)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" "),
            output.status,
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim())
            }
        ))
    }
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

fn offline_init_instructions() -> [&'static str; 4] {
    [
        "offline init: skipped `lake update` and `forge install foundry-rs/forge-std --no-commit`.",
        "when network access is available, run:",
        "  lake update",
        "  forge install foundry-rs/forge-std --no-commit",
    ]
}

fn project_root(root: Option<Utf8PathBuf>) -> Result<Utf8PathBuf, String> {
    match root {
        Some(root) => Ok(root),
        None => {
            let cwd = std::env::current_dir().map_err(|err| err.to_string())?;
            let cwd = Utf8PathBuf::from_path_buf(cwd).map_err(|path| path.display().to_string())?;
            tama_common::find_project_root(&cwd).map_err(|err| err.to_string())
        }
    }
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
        let path = root.join("artifacts/lean");
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
        assert!(instructions.contains("forge install foundry-rs/forge-std --no-commit"));
    }
}
