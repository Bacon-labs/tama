use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, ExitCode, Stdio};

use camino::{Utf8Path, Utf8PathBuf};
#[cfg(test)]
use clap::CommandFactory;
use clap::{ArgAction, Args, Parser, Subcommand};

mod style;

use style::{paint, ColorChoice, Palette, Stream};

const LAKE_PACKAGE_CACHE_ENV: &str = "TAMA_LAKE_PACKAGE_CACHE";
const FORGE_STD_DEPENDENCY: &str = "foundry-rs/forge-std@v1.16.1";
const DEFAULT_VERITY_GIT: &str = "https://github.com/lfglabs-dev/verity.git";
const INIT_AFTER_HELP: &str = "\
Creates a Foundry-compatible Verity starter. The starter includes commented
ERC20Lite source/spec/proof files, mirror Foundry tests, generated bridge
placeholders, and a deploy script that works after `tama build`.

Online init also runs `lake update`, initializes Git when needed, installs
forge-std as a pinned submodule, and refreshes tama.lock.
Use --offline to write files only and do dependency setup later.";
const NEW_AFTER_HELP: &str = "\
Adds one Verity contract scaffold under the configured source, spec, proof, and
mirror-test paths, then updates the configured aggregate modules and
tama.lock.

The scaffold contains TODO proofs marked with `sorry`; `tama check` may pass,
but `tama audit trust-boundary` will reject them until they are discharged.";
const CHECK_AFTER_HELP: &str = "\
Fast Lean check for the configured implementation and spec aggregate modules.

Proof modules, Verity codegen, solc, bridge generation, Forge, and audit checks
do not run here. Use `tama build` when proofs and generated artifacts should be
validated.";
const AUDIT_AFTER_HELP: &str = "\
Checks:
  structure        Required files, aggregate imports, generated paths, and bytecode hashes
  selectors        ABI selectors/topics, Solidity declarations, and Yul dispatch
  storage-layout   Storage slots, overlap, encodings, and compiler layout drift
  coverage         Every spec has a discharger and either a Foundry mirror or a `[coverage.proof_only]` entry
  trust-boundary   Lean axioms, sorryAx, unresolved declarations, and trust reports

Run `tama audit` for the full suite, or `tama audit <check>` for one check.
Aliases: `storage` for `storage-layout`, `trust` for `trust-boundary`.
Use `--deny-warnings` to make warning-severity findings fail CI.";
const BUILD_AFTER_HELP: &str = "\
What gets built:
  proof-check      Lake builds the configured proof aggregate so Lean elaborates implementation, spec, and proof modules
  verity-codegen   Verity emits Yul, ABI, storage, assumption, and trust reports
  manifest         Tama adapts and validates generated contract manifests
  trust-probe      Lean validates dischargers and records proof dependencies
  solc             solc compiles generated Yul through standard JSON and records bytecode hashes
  bridge           Tama generates Solidity interfaces and deployers from the validated manifest
  forge            Foundry compiles generated Solidity and project tests
  lock             tama.lock records the exact inputs used for the successful build

Proofs are first-class build inputs: a proof module that does not elaborate stops
the build before bytecode or bridges are produced. Use `tama check` when you only
want the faster implementation/spec Lean check.

Notes:
  --no-solc stops after fresh Yul/manifests and therefore also skips bridges and Forge.
  --locked checks tama.lock before the build and does not rewrite it after success.";
const TEST_AFTER_HELP: &str = "\
Tama prefixes the command with `forge test` and passes the remaining arguments
through unchanged. With global --offline, Tama adds Forge's --offline flag unless
you already supplied it.

Output and exit status are Forge's by design.";
const INSPECT_AFTER_HELP: &str = "\
Fields:
  manifest
  selectors
  abi
  storage-layout
  yul
  bytecode
  runtime-bytecode
  specs
  obligations
  mirrors
  trust

Use --json when you want machine-readable output for supported fields.";
const CLEAN_AFTER_HELP: &str = "\
Removes generated Tama and Foundry build output, including artifacts/yul,
artifacts/abi, artifacts/bytecode, artifacts/solc-json, artifacts/manifest,
artifacts/lean, artifacts/trust-probe, generated Solidity, Foundry out, and
Foundry cache.

Generated Solidity is removed only when it still has Tama's generated-file
header. Use --deep to also remove Lake package/build state under .lake.";
const DOCTOR_AFTER_HELP: &str = "\
Checks required tools, configured tool versions, project paths, Lake buildDir,
Verity dependency resolution, generated artifact directories, and tama.lock
freshness.

With --fix, Tama creates missing generated directories, repairs safe Verity
dependency drift when possible, and refreshes tama.lock. It does not rewrite
user-owned source, spec, proof, or test files.";
const TOOLCHAIN_AFTER_HELP: &str = "\
Commands:
  solc    Resolve or install the solc binary configured by [yul].solc in tama.toml

By default, network access is allowed for missing managed tool versions. With
global --offline, Tama only resolves already-installed tools and fails if the
configured version is unavailable.";
const INSTALL_AFTER_HELP: &str = "\
Adds a Tama package to lakefile.toml, runs `lake update`, and refreshes
tama.lock. The package must contain tama.toml; pure Lake packages should be added
to lakefile.toml manually.

Packages may be GitHub shorthand, a full Git URL, or a local path. Add @rev to a
Git package to pin an explicit revision.";
const REMOVE_AFTER_HELP: &str = "\
Removes one Tama/Lake dependency from lakefile.toml, runs `lake update`, and
refreshes tama.lock. The dependency name must already be present in the Lakefile.";
const UPDATE_AFTER_HELP: &str = "\
Refreshes dependency manifests and lock state. Without flags, Tama syncs the
configured Verity dependency, runs `lake update`, runs `forge update`, and writes
tama.lock.

Use --package <name> to update one Lake package. Use --no-lake or --no-forge for
local lock/config refreshes when you intentionally do not want those tools run.";

#[derive(Debug, Parser)]
#[command(name = "tama", version, about = "Verity developer toolchain")]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Run as if invoked from another project root"
    )]
    root: Option<Utf8PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Fail if tama.lock or tracked inputs are stale"
    )]
    locked: bool,
    #[arg(long, global = true, help = "Avoid network access where supported")]
    offline: bool,
    #[arg(
        long,
        global = true,
        help = "Emit machine-readable Tama-owned JSON where supported"
    )]
    json: bool,
    #[arg(
        short,
        long,
        global = true,
        action = ArgAction::Count,
        help = "Increase Tama-owned logging verbosity"
    )]
    verbose: u8,
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
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(
        about = "Create a new ERC20Lite starter project",
        long_about = "Create a new Tama/Foundry project with the ERC20Lite starter.",
        after_help = INIT_AFTER_HELP
    )]
    Init {
        #[arg(
            value_name = "PATH",
            help = "Directory to create; defaults to the current directory"
        )]
        path: Option<Utf8PathBuf>,
    },
    #[command(
        about = "Scaffold a new Verity contract",
        long_about = "Scaffold a new Verity contract, spec, proof, mirror test, aggregate imports, and lock state.",
        after_help = NEW_AFTER_HELP
    )]
    New {
        #[arg(value_name = "Name", help = "UpperCamelCase contract name")]
        name: String,
    },
    #[command(
        about = "Lean-check implementation and spec modules",
        long_about = "Run the fast Lean check for implementation and spec aggregate modules.",
        after_help = CHECK_AFTER_HELP
    )]
    Check,
    #[command(
        about = "Build proofs, Verity artifacts, bytecode, Solidity bridges, and Forge targets",
        long_about = "Build a Tama project from Verity source through proof validation, Verity code generation, manifest adaptation, solc bytecode generation, generated Solidity bridges, and Foundry compilation.",
        after_help = BUILD_AFTER_HELP
    )]
    Build(BuildArgs),
    #[command(
        about = "Pass through directly to forge test",
        long_about = "Run Foundry tests through `forge test`, preserving Forge arguments and exit status.",
        after_help = TEST_AFTER_HELP
    )]
    Test(TestArgs),
    #[command(
        about = "Audit generated Verity artifacts after a build",
        long_about = "Audit generated Verity artifacts after `tama build`.\n\nTama validates that generated manifests, Yul, bytecode, Solidity bridges, Foundry mirror coverage, and Lean/Verity trust-boundary reports still agree with the project source. The command reports every finding it can collect and exits nonzero for errors.",
        after_help = AUDIT_AFTER_HELP
    )]
    Audit(AuditArgs),
    #[command(
        about = "Inspect generated Verity artifacts",
        long_about = "Print one generated artifact or manifest-derived view for a built Verity contract.",
        after_help = INSPECT_AFTER_HELP
    )]
    Inspect(InspectArgs),
    #[command(
        about = "Remove generated build artifacts",
        long_about = "Remove generated Tama, Lake, and Foundry build artifacts from a project.",
        after_help = CLEAN_AFTER_HELP
    )]
    Clean {
        #[arg(long, help = "Also remove Lake package/build state under .lake")]
        deep: bool,
    },
    #[command(
        about = "Diagnose toolchain and project drift",
        long_about = "Check the local toolchain, project layout, dependency resolution, generated directories, and lock freshness.",
        after_help = DOCTOR_AFTER_HELP
    )]
    Doctor {
        #[arg(long, help = "Apply safe repairs before reporting status")]
        fix: bool,
    },
    #[command(
        about = "Resolve configured external tool binaries",
        long_about = "Resolve configured external tool binaries for CI and shell setup.",
        after_help = TOOLCHAIN_AFTER_HELP
    )]
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommand,
    },
    #[command(
        about = "Add a Tama/Lake dependency",
        long_about = "Add a Tama package dependency, run Lake update, and refresh tama.lock.",
        after_help = INSTALL_AFTER_HELP
    )]
    Install {
        #[arg(
            value_name = "PACKAGE",
            help = "Package path, Git URL, or GitHub shorthand"
        )]
        package: String,
    },
    #[command(
        about = "Remove a Tama/Lake dependency",
        long_about = "Remove a Tama/Lake dependency, run Lake update, and refresh tama.lock.",
        after_help = REMOVE_AFTER_HELP
    )]
    Remove {
        #[arg(value_name = "PACKAGE", help = "Lake package name to remove")]
        package: String,
    },
    #[command(
        about = "Refresh dependencies and lock state",
        long_about = "Refresh Lake/Forge dependencies and Tama lock state.",
        after_help = UPDATE_AFTER_HELP
    )]
    Update {
        #[arg(long, help = "Skip `forge update`")]
        no_forge: bool,
        #[arg(long, help = "Skip `lake update`")]
        no_lake: bool,
        #[arg(long, value_name = "name", help = "Update one Lake package")]
        package: Option<String>,
    },
}

#[derive(Debug, Args)]
struct BuildArgs {
    #[arg(
        long,
        help = "Stop after fresh Yul/manifests; skip solc bytecode, generated bridges, and Forge"
    )]
    no_solc: bool,
    #[arg(long, help = "Skip the final `forge build` step")]
    no_forge: bool,
    #[arg(
        long = "contract",
        value_name = "Name",
        help = "Build one Verity contract instead of every discovered contract"
    )]
    contract_: Option<String>,
}

#[derive(Debug, Args)]
struct TestArgs {
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Arguments passed after `forge test`"
    )]
    forge_args: Vec<String>,
}

#[derive(Debug, Args)]
struct AuditArgs {
    #[arg(
        value_name = "CHECK",
        help = "Run one audit check instead of the full suite"
    )]
    check: Option<String>,
    #[arg(long, help = "Treat warning-severity findings as failures")]
    deny_warnings: bool,
}

#[derive(Debug, Args)]
struct InspectArgs {
    #[arg(
        value_name = "CONTRACT",
        help = "Contract name from a generated manifest"
    )]
    contract: String,
    #[arg(value_name = "FIELD", help = "Artifact field to print")]
    field: String,
}

#[derive(Debug, Subcommand)]
enum ToolchainCommand {
    #[command(
        about = "Resolve or install the configured solc binary",
        long_about = "Resolve or install the solc binary configured by [yul].solc in tama.toml, then print the resolved binary path."
    )]
    Solc,
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();
    if cli.no_color {
        cli.color = ColorChoice::Never;
    }
    style::apply_env(cli.color, cli.json);
    tama_common::init_logging(
        cli.json,
        matches!(cli.command, Command::Test(_)),
        cli.verbose,
    );
    let stderr_palette = Palette::new(style::resolve(cli.color, cli.json, Stream::Stderr));
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            anstream::eprintln!("{} {err}", paint(stderr_palette.error_prefix, "error:"));
            ExitCode::from(1)
        }
    }
}

#[derive(Clone, Copy)]
struct CommandProgress {
    enabled: bool,
    palette: Palette,
}

impl CommandProgress {
    fn new(enabled: bool, palette: Palette) -> Self {
        Self { enabled, palette }
    }

    fn scope(&self, title: &str, rows: &[(&str, String)]) {
        if !self.enabled {
            return;
        }
        anstream::println!("{}:", paint(self.palette.header, title));
        for (label, value) in rows {
            anstream::println!("  {}: {value}", paint(self.palette.dim, label));
        }
        anstream::println!();
        anstream::println!("{}", paint(self.palette.header, "Steps:"));
    }

    fn run<T, F>(&self, name: &str, running: &str, success: &str, f: F) -> Result<T, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        self.start(name, running);
        match f() {
            Ok(value) => {
                self.ok(name, success);
                Ok(value)
            }
            Err(err) => {
                self.fail(name, "failed");
                Err(err)
            }
        }
    }

    fn start(&self, name: &str, detail: &str) {
        self.line("run", self.palette.run, name, detail);
    }

    fn ok(&self, name: &str, detail: &str) {
        self.line("ok", self.palette.ok, name, detail);
    }

    fn skip(&self, name: &str, detail: &str) {
        self.line("skip", self.palette.skip, name, detail);
    }

    fn fail(&self, name: &str, detail: &str) {
        self.line("fail", self.palette.fail, name, detail);
    }

    fn line(&self, status: &str, style: anstyle::Style, name: &str, detail: &str) {
        if self.enabled {
            anstream::println!("{}", command_progress_line(status, style, name, detail));
        }
    }
}

fn command_progress_line(status: &str, style: anstyle::Style, name: &str, detail: &str) -> String {
    let padded = format!("{status:<4}");
    format!("  {} {name:<12} {detail}", paint(style, padded))
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    let palette = Palette::new(style::resolve(cli.color, cli.json, Stream::Stdout));
    match cli.command {
        Command::Init { path } => {
            let path = path.unwrap_or_else(|| Utf8PathBuf::from("."));
            let name = path
                .file_name()
                .filter(|name| !name.is_empty())
                .unwrap_or("my-protocol")
                .to_string();
            let progress = CommandProgress::new(!cli.json, palette);
            progress.scope(
                "Init scope",
                &[
                    ("project", path.to_string()),
                    ("starter", "ERC20Lite".to_string()),
                    (
                        "mode",
                        if cli.offline { "offline" } else { "online" }.to_string(),
                    ),
                ],
            );
            progress.run(
                "files",
                "write starter project files",
                "starter project files written",
                || {
                    tama_project::init(
                        &path,
                        tama_project::InitOptions {
                            name,
                            ..Default::default()
                        },
                    )
                    .map_err(|err| err.to_string())
                },
            )?;
            finalize_init(&path, cli.offline, progress)?;
            anstream::println!(
                "{} {}",
                paint(palette.ok, "Initialized Tama ERC20Lite starter at"),
                paint(palette.path, &path),
            );
            if cli.offline {
                for line in offline_init_instructions() {
                    anstream::println!("{line}");
                }
            }
            for line in init_next_steps(&path) {
                anstream::println!("{line}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::New { name } => {
            let root = project_root(cli.root)?;
            enforce_locked_if_requested(&root, cli.locked)?;
            let progress = CommandProgress::new(!cli.json, palette);
            progress.scope(
                "New scope",
                &[
                    ("project", root.to_string()),
                    ("contract", name.clone()),
                    (
                        "writes",
                        "source, spec, proof, mirror test, aggregates, lock".to_string(),
                    ),
                ],
            );
            progress.run(
                "scaffold",
                "write contract files and aggregate imports",
                "contract scaffold written",
                || tama_project::scaffold_contract(&root, &name).map_err(|err| err.to_string()),
            )?;
            anstream::println!(
                "{} {}",
                paint(palette.ok, "Created Verity contract scaffold"),
                paint(palette.path, &name),
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Check => {
            let root = project_root(cli.root)?;
            enforce_locked_if_requested(&root, cli.locked)?;
            let config = tama_config::load_config(&root).map_err(|err| err.to_string())?;
            let targets = config.check_targets();
            let progress = CommandProgress::new(!cli.json, palette);
            progress.scope(
                "Check scope",
                &[
                    ("project", root.to_string()),
                    ("targets", targets.join(", ")),
                    ("proofs", "not run; use `tama build`".to_string()),
                ],
            );
            progress.run(
                "cache",
                "prepare Lake package checkouts",
                "Lake packages ready",
                || prepare_lake_packages_for_build(&root, cli.offline),
            )?;
            progress.run(
                "lake",
                &format!("build {} and {}", targets[0], targets[1]),
                "implementation/spec targets accepted",
                || {
                    tama_build::Lake::new_json(root.clone(), cli.json)
                        .check_src_and_spec(&config)
                        .map_err(|err| err.to_string())
                },
            )?;
            refresh_lake_package_cache_after_success(&root, &palette);
            if cli.json {
                anstream::println!("{}", check_status_json(&targets)?);
            } else {
                anstream::println!(
                    "{} {} and {} accepted",
                    paint(palette.ok, "Check completed:"),
                    targets[0],
                    targets[1],
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Build(args) => {
            let root = project_root(cli.root)?;
            enforce_locked_if_requested(&root, cli.locked)?;
            prepare_lake_packages_for_build(&root, cli.offline)?;
            let status = tama_build::Pipeline::new(root.clone())
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
            refresh_lake_package_cache_after_success(&root, &palette);
            if cli.json {
                anstream::println!("{}", build_status_json(&status)?);
            } else {
                anstream::println!(
                    "{} {}",
                    paint(palette.ok, "Build completed:"),
                    paint(
                        palette.count,
                        format_count(status.manifests.len(), "manifest", "manifests"),
                    ),
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Test(args) => {
            let root = project_root(cli.root)?;
            enforce_locked_if_requested(&root, cli.locked)?;
            let status = tama_toolchain::run_passthrough(
                "forge",
                &prefixed_test_args(args.forge_args, cli.offline),
                &root,
            )
            .map_err(|err| err.to_string())?;
            Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
        }
        Command::Audit(args) => {
            let root = project_root(cli.root)?;
            enforce_locked_if_requested(&root, cli.locked)?;
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
                anstream::println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?
                );
            } else {
                anstream::println!(
                    "{}",
                    format_audit_report(&root, &report, args.deny_warnings, &palette)
                );
            }
            Ok(if report.has_failures(args.deny_warnings) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            })
        }
        Command::Inspect(args) => {
            let root = project_root(cli.root)?;
            enforce_locked_if_requested(&root, cli.locked)?;
            let field = tama_inspect::parse_field(&args.field)
                .ok_or_else(|| format!("unknown inspect field `{}`", args.field))?;
            anstream::print!(
                "{}",
                tama_inspect::inspect(&root, &args.contract, field, cli.json)
                    .map_err(|err| err.to_string())?
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Clean { deep } => {
            let root = project_root(cli.root)?;
            enforce_locked_if_requested(&root, cli.locked)?;
            let progress = CommandProgress::new(!cli.json, palette);
            progress.scope(
                "Clean scope",
                &[
                    ("project", root.to_string()),
                    (
                        "mode",
                        if deep {
                            "generated artifacts and Lake state"
                        } else {
                            "generated artifacts"
                        }
                        .to_string(),
                    ),
                ],
            );
            let report = progress.run(
                "remove",
                "remove generated Tama, Lake, and Foundry outputs",
                "clean targets processed",
                || clean(&root, deep),
            )?;
            if !cli.json {
                anstream::println!("{}", format_clean_report(&report, &palette));
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor { fix } => {
            let root = cli.root.unwrap_or_else(|| Utf8PathBuf::from("."));
            let project = tama_common::find_project_root(&root).ok();
            if cli.locked {
                if let Some(project_root) = project.as_ref() {
                    enforce_locked_if_requested(project_root, true)?;
                }
            }
            let progress = CommandProgress::new(!cli.json, palette);
            progress.scope(
                "Doctor scope",
                &[
                    (
                        "project",
                        project
                            .as_ref()
                            .map(|root| root.to_string())
                            .unwrap_or_else(|| "none found".to_string()),
                    ),
                    (
                        "repair",
                        if fix { "enabled" } else { "disabled" }.to_string(),
                    ),
                ],
            );
            if fix {
                progress.run(
                    "repair",
                    "apply safe repairs",
                    "safe repairs applied",
                    || apply_doctor_fix(&root, project.as_ref(), cli.offline),
                )?;
            }
            let report = doctor_report(project.as_ref())?;
            if cli.json {
                anstream::println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|err| err.to_string())?
                );
            } else {
                progress.ok("report", "diagnostics collected");
                anstream::println!("{}", format_doctor_report(&report, fix, &palette));
            }
            Ok(if doctor_report_has_failures(&report) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            })
        }
        Command::Toolchain { command } => match command {
            ToolchainCommand::Solc => {
                let root = project_root(cli.root)?;
                let tool = resolve_configured_solc(&root, cli.offline)?;
                if cli.json {
                    anstream::println!(
                        "{}",
                        serde_json::to_string_pretty(&tool).map_err(|err| err.to_string())?
                    );
                } else {
                    anstream::println!("{}", tool.path);
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Install { package } => {
            let root = project_root(cli.root)?;
            let progress = CommandProgress::new(!cli.json, palette);
            progress.scope(
                "Install scope",
                &[
                    ("project", root.to_string()),
                    ("package", package.clone()),
                    (
                        "updates",
                        "lakefile.toml, lake-manifest.json, tama.lock".to_string(),
                    ),
                ],
            );
            progress.run(
                "install",
                "resolve package, run Lake update, refresh lock",
                "dependency installed",
                || install_package(&root, &package, cli.offline, cli.locked),
            )?;
            anstream::println!(
                "{} {}",
                paint(palette.ok, "Installed Tama dependency"),
                paint(palette.path, &package),
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Remove { package } => {
            let root = project_root(cli.root)?;
            let progress = CommandProgress::new(!cli.json, palette);
            progress.scope(
                "Remove scope",
                &[
                    ("project", root.to_string()),
                    ("package", package.clone()),
                    (
                        "updates",
                        "lakefile.toml, lake-manifest.json, tama.lock".to_string(),
                    ),
                ],
            );
            progress.run(
                "remove",
                "remove dependency, run Lake update, refresh lock",
                "dependency removed",
                || {
                    mutate_dependencies(&root, cli.locked, cli.offline, |root| {
                        tama_config::remove_lake_dependency(root, &package)
                            .map_err(|err| err.to_string())
                    })
                },
            )?;
            anstream::println!(
                "{} {}",
                paint(palette.ok, "Removed Tama dependency"),
                paint(palette.path, &package),
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Update {
            no_forge,
            no_lake,
            package,
        } => {
            let root = project_root(cli.root)?;
            let progress = CommandProgress::new(!cli.json, palette);
            progress.scope(
                "Update scope",
                &[
                    ("project", root.to_string()),
                    (
                        "package",
                        package.clone().unwrap_or_else(|| "all".to_string()),
                    ),
                    (
                        "lake",
                        if no_lake { "skipped" } else { "enabled" }.to_string(),
                    ),
                    (
                        "forge",
                        if no_forge { "skipped" } else { "enabled" }.to_string(),
                    ),
                ],
            );
            progress.run(
                "update",
                "refresh dependencies and lock state",
                "project lock state current",
                || {
                    update_project(
                        &root,
                        cli.locked,
                        cli.offline,
                        no_lake,
                        no_forge,
                        package.as_deref(),
                    )
                },
            )?;
            anstream::println!("{}", paint(palette.ok, "Updated Tama project lock state"),);
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn doctor_report(project: Option<&Utf8PathBuf>) -> Result<tama_toolchain::DoctorReport, String> {
    let mut report = tama_toolchain::detect_required_tools_at(project.map(|root| root.as_path()));
    report.tools.insert(0, tama_status());
    if let Some(project_root) = project {
        let config = match tama_config::load_config(project_root) {
            Ok(config) => Some(config),
            Err(err) => {
                report
                    .notes
                    .push(format!("tama.toml could not be read: {err}"));
                report.tools.push(tama_toolchain::ToolStatus::Incompatible {
                    name: "tama.toml".to_string(),
                    found: "invalid or missing".to_string(),
                    expected: "valid Tama project config".to_string(),
                });
                None
            }
        };
        if let Ok(toolchain) = tama_config::read_lean_toolchain(project_root) {
            if let Some(expected) = lean_version_from_toolchain(&toolchain) {
                mark_version_mismatch(
                    &mut report,
                    "lean",
                    &expected,
                    tama_toolchain::parse_lean_version,
                );
                mark_version_mismatch(
                    &mut report,
                    "lake",
                    &expected,
                    tama_toolchain::parse_lake_lean_version,
                );
            }
        }
        if let Some(config) = &config {
            mark_version_mismatch(
                &mut report,
                "solc",
                &config.yul.solc,
                tama_toolchain::parse_solc_version,
            );
            report_legacy_module_layout(&mut report, project_root, config);
            report_lake_build_dir(&mut report, project_root);
            report_generated_dirs(&mut report, project_root, &config.paths);
        }
        match tama_config::load_lock(project_root) {
            Ok(lock) => {
                let drift =
                    tama_config::lock_drift(project_root, &lock).map_err(|err| err.to_string())?;
                report.lock_current = Some(drift.is_empty());
                if let Some(config) = &config {
                    check_verity_resolution(&mut report, config, &lock);
                }
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

fn resolve_configured_solc(root: &Utf8Path, offline: bool) -> Result<tama_toolchain::Tool, String> {
    let config = tama_config::load_config(root).map_err(|err| err.to_string())?;
    let result = if offline {
        tama_toolchain::resolve_solc(&config.yul.solc, root)
    } else {
        tama_toolchain::resolve_or_install_solc(&config.yul.solc, root)
    };
    result.map_err(|err| err.to_string())
}

fn tama_status() -> tama_toolchain::ToolStatus {
    let path = std::env::current_exe()
        .ok()
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| Utf8PathBuf::from("tama"));
    tama_toolchain::ToolStatus::Ok(tama_toolchain::Tool {
        name: "tama".to_string(),
        path,
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
    })
}

fn doctor_report_has_failures(report: &tama_toolchain::DoctorReport) -> bool {
    report.tools.iter().any(|tool| {
        matches!(
            tool,
            tama_toolchain::ToolStatus::Missing { .. }
                | tama_toolchain::ToolStatus::Incompatible { .. }
        )
    }) || report.lock_current == Some(false)
}

fn format_doctor_report(
    report: &tama_toolchain::DoctorReport,
    fixed: bool,
    palette: &Palette,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}\n", paint(palette.header, "Checks:")));
    let mut checks = 0usize;
    let mut failures = 0usize;
    for tool in &report.tools {
        checks += 1;
        match tool {
            tama_toolchain::ToolStatus::Ok(tool) => {
                let name = format!("{:<15}", tool.name);
                out.push_str(&format!(
                    "  {} {} {}\n",
                    paint(palette.ok, "ok  "),
                    paint(palette.header, name),
                    concise_tool_detail(tool),
                ));
            }
            tama_toolchain::ToolStatus::Missing { name, remediation } => {
                failures += 1;
                let name = format!("{name:<15}");
                out.push_str(&format!(
                    "  {} {} {remediation}\n",
                    paint(palette.fail, "fail"),
                    paint(palette.header, name),
                ));
            }
            tama_toolchain::ToolStatus::Incompatible {
                name,
                found,
                expected,
            } => {
                failures += 1;
                let name = format!("{name:<15}");
                out.push_str(&format!(
                    "  {} {} found {found}, expected {expected}\n",
                    paint(palette.fail, "fail"),
                    paint(palette.header, name),
                ));
            }
        }
    }
    if let Some(lock_current) = report.lock_current {
        checks += 1;
        if lock_current {
            out.push_str(&format!(
                "  {} {} current\n",
                paint(palette.ok, "ok  "),
                paint(palette.header, "lock           "),
            ));
        } else {
            failures += 1;
            out.push_str(&format!(
                "  {} {} stale or unreadable\n",
                paint(palette.fail, "fail"),
                paint(palette.header, "lock           "),
            ));
        }
    }
    if !report.notes.is_empty() {
        out.push_str(&format!("\n{}\n", paint(palette.header, "Notes:")));
        for note in &report.notes {
            out.push_str(&format!("  {note}\n"));
        }
    }
    out.push('\n');
    let summary_style = if failures == 0 {
        palette.ok
    } else {
        palette.fail
    };
    let prefix = if failures == 0 {
        if fixed {
            "Doctor passed after repair:"
        } else {
            "Doctor passed:"
        }
    } else {
        "Doctor found issues:"
    };
    out.push_str(&format!(
        "{} {}, {}",
        paint(summary_style, prefix),
        paint(palette.count, format_count(checks, "check", "checks")),
        paint(palette.count, format_count(failures, "issue", "issues")),
    ));
    out
}

fn concise_tool_detail(tool: &tama_toolchain::Tool) -> String {
    tool.version
        .as_deref()
        .map(concise_tool_output)
        .unwrap_or_else(|| tool.path.to_string())
}

fn concise_tool_output(raw: &str) -> String {
    let mut lines = raw.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or(raw.trim());
    if let Some(version) = first.strip_prefix("Lean (version ") {
        return version.split(',').next().unwrap_or(version).to_string();
    }
    if let Some(version) = first.strip_prefix("Lake version ") {
        return version.to_string();
    }
    if let Some(version) = first.strip_prefix("forge Version: ") {
        return version.to_string();
    }
    if let Some(version) = first.strip_prefix("git version ") {
        return version.to_string();
    }
    if first.starts_with("solc,") {
        if let Some(version) = lines.find_map(|line| line.strip_prefix("Version:")) {
            return version.trim().to_string();
        }
    }
    first.to_string()
}

fn report_legacy_module_layout(
    report: &mut tama_toolchain::DoctorReport,
    root: &Utf8Path,
    config: &tama_config::TamaConfig,
) {
    if config.modules.is_some() {
        return;
    }
    if ["TamaSrc.lean", "TamaSpec.lean", "TamaProof.lean"]
        .iter()
        .all(|path| root.join(path).is_file())
    {
        report.notes.push(
            "warning: legacy TamaSrc/TamaSpec/TamaProof module layout detected; add [modules] to use package-root modules"
                .to_string(),
        );
    }
}

fn report_lake_build_dir(report: &mut tama_toolchain::DoctorReport, root: &Utf8Path) {
    match tama_config::parse_lake_build_dir(root) {
        Ok(Some(dir)) => report
            .tools
            .push(tama_toolchain::ToolStatus::Ok(tama_toolchain::Tool {
                name: "lake buildDir".to_string(),
                path: "lakefile.toml".into(),
                version: Some(dir.to_string()),
            })),
        Ok(None) => report.tools.push(tama_toolchain::ToolStatus::Incompatible {
            name: "lake buildDir".to_string(),
            found: "<default>".to_string(),
            expected: "configured under artifacts/lean".to_string(),
        }),
        Err(tama_config::Error::UnsupportedLakefile(message))
            if message.contains("lakefile.lean") =>
        {
            report
                .notes
                .push(format!("lake buildDir could not be checked: {message}"));
        }
        Err(err) => report.tools.push(tama_toolchain::ToolStatus::Incompatible {
            name: "lake buildDir".to_string(),
            found: err.to_string(),
            expected: "configured in lakefile.toml".to_string(),
        }),
    }
}

fn report_generated_dirs(
    report: &mut tama_toolchain::DoctorReport,
    root: &Utf8Path,
    paths: &tama_config::PathsConfig,
) {
    let dirs = match generated_dirs(root, paths, "inspect") {
        Ok(dirs) => dirs,
        Err(err) => {
            report.tools.push(tama_toolchain::ToolStatus::Incompatible {
                name: "project directories".to_string(),
                found: err,
                expected: "project-relative generated directories".to_string(),
            });
            return;
        }
    };
    let missing = dirs
        .into_iter()
        .filter(|dir| !root.join(dir).is_dir())
        .map(|dir| dir.to_string())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        report.tools.push(tama_toolchain::ToolStatus::Incompatible {
            name: "project directories".to_string(),
            found: format!("missing {}", missing.join(", ")),
            expected: "generated directories present; run `tama doctor --fix`".to_string(),
        });
    }
}

fn enforce_locked_if_requested(root: &Utf8Path, locked: bool) -> Result<(), String> {
    if !locked {
        return Ok(());
    }
    let lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
    tama_config::enforce_locked(root, &lock).map_err(|err| err.to_string())
}

fn check_verity_resolution(
    report: &mut tama_toolchain::DoctorReport,
    config: &tama_config::TamaConfig,
    lock: &tama_config::TamaLock,
) {
    let requested = verity_rev_from_config(&config.project.verity);
    let mut mismatches = Vec::new();
    match lock.resolved.get("verity_rev") {
        Some(found) if found == &requested => {}
        Some(found) => mismatches.push(format!("lock verity_rev={found}")),
        None => mismatches.push("lock verity_rev=<missing>".to_string()),
    }
    if let Some(input_rev) = lock.resolved.get("lake.verity.input_rev") {
        if input_rev != &requested {
            mismatches.push(format!("lake verity inputRev={input_rev}"));
        }
    }
    if is_full_git_sha(&requested) {
        match lock.resolved.get("lake.verity.rev") {
            Some(found) if found == &requested => {}
            Some(found) => mismatches.push(format!("lake verity rev={found}")),
            None => mismatches.push("lake verity rev=<missing>".to_string()),
        }
    }
    if let (Some(locked_git), Some(lake_git)) = (
        lock.resolved.get("verity_git"),
        lock.resolved.get("lake.verity.url"),
    ) {
        if locked_git != lake_git {
            mismatches.push(format!("lake verity url={lake_git}"));
        }
    }

    if mismatches.is_empty() {
        let resolved = lock
            .resolved
            .get("lake.verity.rev")
            .or_else(|| lock.resolved.get("verity_rev"))
            .cloned()
            .unwrap_or_else(|| requested.clone());
        report
            .tools
            .push(tama_toolchain::ToolStatus::Ok(tama_toolchain::Tool {
                name: "verity".to_string(),
                path: lock
                    .resolved
                    .get("verity_git")
                    .cloned()
                    .unwrap_or_else(|| "lake-manifest.json".to_string())
                    .into(),
                version: Some(format!("requested {requested}, resolved {resolved}")),
            }));
    } else {
        report.tools.push(tama_toolchain::ToolStatus::Incompatible {
            name: "verity".to_string(),
            found: mismatches.join("; "),
            expected: format!("project.verity resolves to {requested}"),
        });
        report
            .notes
            .push("run `tama update` to sync the Verity dependency state".to_string());
    }
}

fn is_full_git_sha(raw: &str) -> bool {
    raw.len() == 40 && raw.chars().all(|ch| ch.is_ascii_hexdigit())
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

fn apply_doctor_fix(
    _root: &Utf8PathBuf,
    project: Option<&Utf8PathBuf>,
    offline: bool,
) -> Result<(), String> {
    let Some(project_root) = project else {
        return Err("`tama doctor --fix` requires a Tama project with tama.toml".to_string());
    };
    let config = tama_config::load_config(project_root).map_err(|err| err.to_string())?;
    let mut lock = tama_config::load_lock(project_root).map_err(|err| err.to_string())?;
    for dir in generated_dirs(project_root, &config.paths, "create")? {
        std::fs::create_dir_all(project_root.join(dir)).map_err(|err| err.to_string())?;
    }
    let (_, needs_lake_update) = planned_verity_lake_dependency(project_root, &config, &lock)?;
    if needs_lake_update && offline {
        return Err("`tama doctor --fix --offline` cannot repair Verity dependency drift because it must run `lake update`".to_string());
    }
    let snapshot = snapshot_dependency_files(project_root);
    let changed = sync_verity_lake_dependency(project_root, &config, &mut lock)?;
    if changed {
        if let Err(err) = run_lake_update(project_root, None) {
            return Err(restore_dependency_files_after_failure(
                project_root,
                snapshot,
                err,
            ));
        }
    }
    tama_config::update_lock_inputs(project_root, &mut lock).map_err(|err| err.to_string())?;
    tama_config::write_lock(project_root, &lock).map_err(|err| err.to_string())?;
    Ok(())
}

fn generated_dirs(
    root: &Utf8Path,
    paths: &tama_config::PathsConfig,
    action: &str,
) -> Result<Vec<Utf8PathBuf>, String> {
    let mut dirs = vec![
        paths.out.clone(),
        paths.out.join("yul"),
        paths.out.join("abi"),
        paths.out.join("bytecode"),
        paths.out.join("solc-json"),
        paths.out.join("manifest"),
        paths.out.join("lean"),
        paths.out.join("trust-probe"),
        paths.generated.clone(),
    ];
    match tama_config::parse_lake_build_dir(root) {
        Ok(Some(dir)) => push_unique_dir(&mut dirs, dir),
        Ok(None) | Err(tama_config::Error::UnsupportedLakefile(_)) => {}
        Err(err) => return Err(err.to_string()),
    }
    for dir in &dirs {
        ensure_project_relative(dir, action)?;
    }
    Ok(dirs)
}

fn push_unique_dir(dirs: &mut Vec<Utf8PathBuf>, dir: Utf8PathBuf) {
    if !dirs.iter().any(|existing| existing == &dir) {
        dirs.push(dir);
    }
}

fn finalize_init(
    root: &Utf8PathBuf,
    offline: bool,
    progress: CommandProgress,
) -> Result<(), String> {
    if offline {
        progress.skip("lake", "offline; lake update not run");
        progress.skip("git", "offline; Git worktree unchanged");
        progress.skip("forge", "offline; forge-std not installed");
        return Ok(());
    }
    progress.run(
        "lake",
        "resolve Lake dependencies",
        "Lake manifest updated",
        || run_lake_update(root, None),
    )?;
    progress.run(
        "git",
        "ensure project is a Git worktree",
        "Git ready",
        || ensure_git_worktree(root),
    )?;
    let mut forge_args = vec!["install", FORGE_STD_DEPENDENCY];
    forge_args.extend(forge_install_optional_flags()?);
    progress.run(
        "forge",
        "install pinned forge-std submodule",
        "forge-std installed",
        || run_tool(root, "forge", &forge_args),
    )?;
    progress.run("lock", "refresh tama.lock", "tama.lock current", || {
        refresh_lock(root)
    })
}

fn install_package(
    root: &Utf8PathBuf,
    package: &str,
    offline: bool,
    locked: bool,
) -> Result<(), String> {
    let mut dependency =
        tama_config::parse_lake_dependency(root, package).map_err(|err| err.to_string())?;
    if offline {
        return Err("`tama install` cannot run with --offline because it must validate the dependency and run `lake update`".to_string());
    }
    if locked {
        let lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
        tama_config::enforce_locked(root, &lock).map_err(|err| err.to_string())?;
    }
    let explicit_rev = package_has_explicit_git_rev(package);
    match &mut dependency.source {
        tama_config::LakeDependencySource::Git { url, rev } => {
            let package = validate_remote_tama_package(url, rev, explicit_rev)?;
            dependency.name = package.name;
            if !explicit_rev {
                *rev = package.rev;
            }
        }
        tama_config::LakeDependencySource::Path { path } => {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                root.join(&path)
            };
            dependency.name = tama_config::lake_package_name(&resolved).map_err(|err| {
                format!("local dependency `{path}` must contain Lake package metadata: {err}")
            })?;
        }
    }
    mutate_dependencies(root, false, offline, |root| {
        tama_config::upsert_lake_dependency(root, &dependency).map_err(|err| err.to_string())
    })
}

fn update_project(
    root: &Utf8PathBuf,
    locked: bool,
    offline: bool,
    no_lake: bool,
    no_forge: bool,
    package: Option<&str>,
) -> Result<(), String> {
    if offline && (!no_lake || !no_forge) {
        return Err("`tama update --offline` requires both --no-lake and --no-forge".to_string());
    }
    if package.is_some() && no_lake {
        return Err("`tama update --package` requires Lake update; remove --no-lake".to_string());
    }
    if locked {
        let lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
        tama_config::enforce_locked(root, &lock).map_err(|err| err.to_string())?;
    }
    let config = tama_config::load_config(root).map_err(|err| err.to_string())?;
    let mut lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
    if matches!(package, Some(name) if name != "verity") {
        let (_, needs_lake_update) = planned_verity_lake_dependency(root, &config, &lock)?;
        if needs_lake_update {
            return Err(
                "`tama update --package` cannot refresh another package while the Verity dependency is drifting; run `tama update` or `tama update --package verity` first"
                    .to_string(),
            );
        }
        let snapshot = snapshot_dependency_files(root);
        if let Err(err) = run_lake_update(root, package) {
            return Err(restore_dependency_files_after_failure(root, snapshot, err));
        }
    } else {
        let (_, needs_lake_update) = planned_verity_lake_dependency(root, &config, &lock)?;
        if needs_lake_update && no_lake {
            return Err(
                "`tama update --no-lake` cannot repair Verity dependency drift because it must run `lake update`"
                    .to_string(),
            );
        }
        let snapshot = snapshot_dependency_files(root);
        sync_verity_lake_dependency(root, &config, &mut lock)?;
        if !no_lake {
            if let Err(err) = run_lake_update(root, package) {
                return Err(restore_dependency_files_after_failure(root, snapshot, err));
            }
        }
    }
    if package.is_none() && !no_forge {
        run_tool(root, "forge", &["update"])?;
    }
    tama_config::update_lock_inputs(root, &mut lock).map_err(|err| err.to_string())?;
    tama_config::write_lock(root, &lock).map_err(|err| err.to_string())
}

fn planned_verity_lake_dependency(
    root: &Utf8Path,
    config: &tama_config::TamaConfig,
    lock: &tama_config::TamaLock,
) -> Result<(tama_config::LakeDependency, bool), String> {
    let current = match tama_config::lake_dependency(root, "verity") {
        Ok(dependency) => Some(dependency),
        Err(tama_config::Error::DependencyNotFound(_)) => None,
        Err(err) => return Err(err.to_string()),
    };
    let url = match current.as_ref().map(|dependency| &dependency.source) {
        Some(tama_config::LakeDependencySource::Git { url, .. }) => url.clone(),
        Some(tama_config::LakeDependencySource::Path { .. }) => {
            return Err(
                "cannot automatically sync path-based Verity dependency; edit lakefile.toml manually"
                    .to_string(),
            );
        }
        None => lock
            .resolved
            .get("verity_git")
            .or_else(|| lock.resolved.get("lake.verity.url"))
            .cloned()
            .unwrap_or_else(|| DEFAULT_VERITY_GIT.to_string()),
    };
    let dependency = tama_config::LakeDependency {
        name: "verity".to_string(),
        source: tama_config::LakeDependencySource::Git {
            url,
            rev: verity_rev_from_config(&config.project.verity),
        },
    };
    let changed = current.as_ref() != Some(&dependency);
    Ok((dependency, changed))
}

fn sync_verity_lake_dependency(
    root: &Utf8Path,
    config: &tama_config::TamaConfig,
    lock: &mut tama_config::TamaLock,
) -> Result<bool, String> {
    let (dependency, changed) = planned_verity_lake_dependency(root, config, lock)?;
    if changed {
        tama_config::upsert_lake_dependency(root, &dependency).map_err(|err| err.to_string())?;
    }
    if let tama_config::LakeDependencySource::Git { url, rev } = &dependency.source {
        lock.resolved.insert("verity_git".to_string(), url.clone());
        lock.resolved.insert("verity_rev".to_string(), rev.clone());
    }
    Ok(changed)
}

fn mutate_dependencies(
    root: &Utf8PathBuf,
    locked: bool,
    offline: bool,
    edit: impl FnOnce(&Utf8PathBuf) -> Result<(), String>,
) -> Result<(), String> {
    if offline {
        return Err(
            "dependency changes cannot run with --offline because they must run `lake update`"
                .to_string(),
        );
    }
    if locked {
        let lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
        tama_config::enforce_locked(root, &lock).map_err(|err| err.to_string())?;
    }
    let snapshot = snapshot_dependency_files(root);
    edit(root)?;
    if let Err(err) = run_lake_update(root, None) {
        return Err(restore_dependency_files_after_failure(root, snapshot, err));
    }
    refresh_lock(root)
}

struct DependencyFileSnapshots {
    lakefile: FileSnapshot,
    lake_manifest: FileSnapshot,
}

enum FileSnapshot {
    Present(String),
    Missing,
}

fn snapshot_dependency_files(root: &Utf8Path) -> DependencyFileSnapshots {
    DependencyFileSnapshots {
        lakefile: snapshot_file(root, "lakefile.toml"),
        lake_manifest: snapshot_file(root, "lake-manifest.json"),
    }
}

fn snapshot_file(root: &Utf8Path, path: &str) -> FileSnapshot {
    match tama_common::read_to_string(&root.join(path)) {
        Ok(contents) => FileSnapshot::Present(contents),
        Err(_) => FileSnapshot::Missing,
    }
}

fn restore_dependency_files_after_failure(
    root: &Utf8Path,
    snapshot: DependencyFileSnapshots,
    err: String,
) -> String {
    let mut restore_errors = Vec::new();
    for (path, snapshot) in [
        ("lakefile.toml", snapshot.lakefile),
        ("lake-manifest.json", snapshot.lake_manifest),
    ] {
        if let Err(restore_err) = restore_file(root, path, snapshot) {
            restore_errors.push(restore_err);
        }
    }
    if restore_errors.is_empty() {
        err
    } else {
        format!(
            "{err}; additionally failed to restore dependency files: {}",
            restore_errors.join("; ")
        )
    }
}

fn restore_file(root: &Utf8Path, path: &str, snapshot: FileSnapshot) -> Result<(), String> {
    let path = root.join(path);
    match snapshot {
        FileSnapshot::Present(contents) => {
            tama_common::write_string(&path, &contents).map_err(|err| err.to_string())
        }
        FileSnapshot::Missing => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("failed to remove `{path}`: {err}")),
        },
    }
}

fn refresh_lock(root: &Utf8PathBuf) -> Result<(), String> {
    let mut lock = tama_config::load_lock(root).map_err(|err| err.to_string())?;
    tama_config::update_lock_inputs(root, &mut lock).map_err(|err| err.to_string())?;
    tama_config::write_lock(root, &lock).map_err(|err| err.to_string())
}

#[derive(Debug)]
struct RemoteTamaPackage {
    name: String,
    rev: String,
}

fn validate_remote_tama_package(
    url: &str,
    rev: &str,
    explicit_rev: bool,
) -> Result<RemoteTamaPackage, String> {
    let temp = tempfile::tempdir().map_err(|err| err.to_string())?;
    let checkout = Utf8PathBuf::from_path_buf(temp.path().join("package"))
        .map_err(|path| path.display().to_string())?;
    run_process(
        "git",
        &["clone", "--depth", "1", url, checkout.as_str()],
        None,
    )?;
    if explicit_rev {
        run_process(
            "git",
            &["fetch", "--depth", "1", "origin", rev],
            Some(&checkout),
        )?;
        run_process("git", &["checkout", "FETCH_HEAD"], Some(&checkout))?;
    }
    if !checkout.join("tama.toml").is_file() {
        return Err(format!(
            "remote dependency `{url}` at `{rev}` does not contain tama.toml; pure Lake packages are outside `tama install`'s scope, so add this dependency manually to lakefile.toml"
        ));
    }
    let name = tama_config::lake_package_name(&checkout).map_err(|err| {
        format!("remote dependency `{url}` at `{rev}` must contain Lake package metadata: {err}")
    })?;
    let rev = git_head_rev(&checkout)?;
    Ok(RemoteTamaPackage { name, rev })
}

fn package_has_explicit_git_rev(raw: &str) -> bool {
    let raw = raw.trim();
    matches!(
        raw.rsplit_once('@'),
        Some((repo, rev)) if package_split_is_explicit_rev(raw, repo, rev)
    )
}

fn package_split_is_explicit_rev(raw: &str, repo: &str, rev: &str) -> bool {
    if repo.is_empty() || rev.is_empty() {
        return false;
    }
    if raw.starts_with("git@") {
        return repo.starts_with("git@") && repo.contains(':');
    }
    true
}

fn git_head_rev(checkout: &Utf8Path) -> Result<String, String> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(checkout)
        .output()
        .map_err(|err| format!("failed to inspect cloned dependency revision: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "`git rev-parse HEAD` failed with status {}",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_worktree_clean(checkout: &Utf8Path) -> Result<bool, String> {
    let output = ProcessCommand::new("git")
        .args(["status", "--porcelain"])
        .current_dir(checkout)
        .output()
        .map_err(|err| format!("failed to inspect cached package status: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "`git status --porcelain` failed with status {}",
            output.status
        ));
    }
    Ok(output.stdout.is_empty())
}

fn run_lake_update(root: &Utf8PathBuf, package: Option<&str>) -> Result<(), String> {
    let args = lake_update_args(package);
    let Some(cache) = lake_package_cache()? else {
        return run_process("lake", &args, Some(root));
    };
    seed_lake_package_cache(root, &cache)?;
    run_process("lake", &args, Some(root))?;
    sync_lake_package_cache(root, &cache)
}

fn lake_update_args(package: Option<&str>) -> Vec<&str> {
    let mut args = vec!["update"];
    if let Some(package) = package {
        args.push(package);
    }
    args
}

fn lake_package_cache() -> Result<Option<Utf8PathBuf>, String> {
    if let Some(value) = std::env::var_os(LAKE_PACKAGE_CACHE_ENV) {
        return lake_package_cache_from_override(value);
    }
    Ok(default_lake_package_cache())
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

fn default_lake_package_cache() -> Option<Utf8PathBuf> {
    default_lake_package_cache_from_parts(
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
        cfg!(target_os = "macos"),
    )
}

fn default_lake_package_cache_from_parts(
    home: Option<PathBuf>,
    xdg_cache_home: Option<PathBuf>,
    macos: bool,
) -> Option<Utf8PathBuf> {
    let base = if macos {
        home?.join("Library/Caches")
    } else if let Some(cache_home) = xdg_cache_home {
        cache_home
    } else {
        home?.join(".cache")
    };
    Utf8PathBuf::from_path_buf(base.join("tama/lake-packages")).ok()
}

fn prepare_lake_packages_for_build(root: &Utf8Path, offline: bool) -> Result<(), String> {
    seed_lake_package_cache_for_build(root)?;
    if offline {
        ensure_lake_packages_available_offline(root)?;
    }
    Ok(())
}

fn refresh_lake_package_cache_after_success(root: &Utf8Path, palette: &Palette) {
    if let Err(err) = try_refresh_lake_package_cache_after_success(root) {
        anstream::eprintln!(
            "{} failed to refresh Lake package cache: {err}",
            paint(palette.warning_prefix, "warning:"),
        );
    }
}

fn try_refresh_lake_package_cache_after_success(root: &Utf8Path) -> Result<(), String> {
    let Some(cache) = lake_package_cache()? else {
        return Ok(());
    };
    sync_lake_package_cache(root, &cache)
}

fn seed_lake_package_cache_for_build(root: &Utf8Path) -> Result<(), String> {
    let Some(cache) = lake_package_cache()? else {
        return Ok(());
    };
    seed_lake_package_cache_from_manifest(root, &cache)
}

fn ensure_lake_packages_available_offline(root: &Utf8Path) -> Result<(), String> {
    let package_revs = lake_manifest_git_revs(root)?;
    let packages = root.join(".lake/packages");
    for (name, expected_rev) in package_revs {
        let checkout = packages.join(&name);
        if !checkout.is_dir() {
            return Err(format!(
                "--offline requires Lake package `{name}` at `{checkout}`; restore it from `{LAKE_PACKAGE_CACHE_ENV}` or run `lake update` while online"
            ));
        }
        if !checkout.join(".git").is_dir() {
            return Err(format!(
                "--offline requires Lake package `{name}` at `{checkout}` to be a Git checkout"
            ));
        }
        let actual_rev = git_head_rev(&checkout)?;
        if actual_rev != expected_rev {
            return Err(format!(
                "--offline requires Lake package `{name}` at `{checkout}` to match lake-manifest.json revision `{expected_rev}`, found `{actual_rev}`"
            ));
        }
        if !git_worktree_clean(&checkout)? {
            return Err(format!(
                "--offline requires Lake package `{name}` at `{checkout}` to have a clean Git worktree"
            ));
        }
    }
    Ok(())
}

fn seed_lake_package_cache_from_manifest(root: &Utf8Path, cache: &Utf8Path) -> Result<(), String> {
    if !cache.exists() {
        return Ok(());
    }
    let package_revs = lake_manifest_git_revs(root)?;
    if package_revs.is_empty() {
        return Ok(());
    }
    let packages = root.join(".lake/packages");
    copy_matching_package_dirs(cache, &packages, &package_revs)
}

fn lake_manifest_git_revs(root: &Utf8Path) -> Result<BTreeMap<String, String>, String> {
    let path = root.join("lake-manifest.json");
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let text =
        std::fs::read_to_string(&path).map_err(|err| format!("failed to read `{path}`: {err}"))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| format!("failed to parse `{path}`: {err}"))?;
    let mut package_revs = BTreeMap::new();
    let Some(packages) = manifest
        .get("packages")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(package_revs);
    };
    for package in packages {
        if package.get("type").and_then(serde_json::Value::as_str) != Some("git") {
            continue;
        }
        let Some(name) = package.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(rev) = package.get("rev").and_then(serde_json::Value::as_str) else {
            continue;
        };
        package_revs.insert(name.to_string(), rev.to_string());
    }
    Ok(package_revs)
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
    refresh_package_dirs(&packages, cache)
}

fn copy_missing_package_dirs(source: &Utf8Path, destination: &Utf8Path) -> Result<(), String> {
    copy_package_dirs(source, destination, false)
}

fn refresh_package_dirs(source: &Utf8Path, destination: &Utf8Path) -> Result<(), String> {
    copy_package_dirs(source, destination, true)
}

fn copy_matching_package_dirs(
    source: &Utf8Path,
    destination: &Utf8Path,
    package_revs: &BTreeMap<String, String>,
) -> Result<(), String> {
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
        let Some(expected_rev) = package_revs.get(&name) else {
            continue;
        };
        if git_head_rev(&source).ok().as_deref() != Some(expected_rev.as_str()) {
            continue;
        }
        if !git_worktree_clean(&source).unwrap_or(false) {
            continue;
        }
        let target = destination.join(&name);
        if !should_seed_manifest_package(&target, expected_rev)? {
            continue;
        }
        if path_exists(&target)? {
            replace_dir_recursively(&source, &target)?;
        } else {
            copy_dir_recursively_into_new(&source, &target)?;
        }
    }
    Ok(())
}

fn copy_package_dirs(
    source: &Utf8Path,
    destination: &Utf8Path,
    replace_existing: bool,
) -> Result<(), String> {
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
        if !git_worktree_clean(&source).unwrap_or(false) {
            continue;
        }
        let target = destination.join(&name);
        let target_exists = path_exists(&target)?;
        if target_exists && !replace_existing {
            continue;
        }
        if replace_existing {
            if target_exists && !should_replace_cached_package(&source, &target)? {
                continue;
            }
            replace_dir_recursively(&source, &target)?;
        } else {
            copy_dir_recursively_into_new(&source, &target)?;
        }
    }
    Ok(())
}

fn should_replace_cached_package(source: &Utf8Path, target: &Utf8Path) -> Result<bool, String> {
    if !path_exists(target)? {
        return Ok(true);
    }
    if !target.join(".git").is_dir() {
        return Ok(true);
    }
    if !git_worktree_clean(target).unwrap_or(false) {
        return Ok(true);
    }
    let source_rev = git_head_rev(source).ok();
    let target_rev = git_head_rev(target).ok();
    if source_rev.is_none() || source_rev != target_rev {
        return Ok(true);
    }
    Ok(lake_build_artifacts_present(source) && !lake_build_artifacts_present(target))
}

fn lake_build_artifacts_present(package: &Utf8Path) -> bool {
    let build = package.join(".lake/build");
    std::fs::read_dir(build)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

fn should_seed_manifest_package(target: &Utf8Path, expected_rev: &str) -> Result<bool, String> {
    if !path_exists(target)? {
        return Ok(true);
    }
    if !target.join(".git").is_dir() {
        return Ok(false);
    }
    if !git_worktree_clean(target).unwrap_or(false) {
        return Ok(false);
    }
    let Ok(target_rev) = git_head_rev(target) else {
        return Ok(false);
    };
    Ok(target_rev != expected_rev)
}

fn copy_dir_recursively_into_new(source: &Utf8Path, target: &Utf8Path) -> Result<(), String> {
    let temp = copy_dir_recursively_to_temp(source, target)?;
    match path_exists(target) {
        Ok(true) => {
            remove_path_if_exists(&temp)?;
            Ok(())
        }
        Ok(false) => std::fs::rename(&temp, target).map_err(|err| {
            let _ = remove_path_if_exists(&temp);
            format!("failed to copy cached package `{target}`: {err}")
        }),
        Err(err) => {
            let _ = remove_path_if_exists(&temp);
            Err(err)
        }
    }
}

fn replace_dir_recursively(source: &Utf8Path, target: &Utf8Path) -> Result<(), String> {
    let temp = copy_dir_recursively_to_temp(source, target)?;
    if let Err(err) = remove_path_if_exists(target) {
        let _ = remove_path_if_exists(&temp);
        return Err(err);
    }
    std::fs::rename(&temp, target).map_err(|err| {
        let _ = remove_path_if_exists(&temp);
        format!("failed to replace cached package `{target}`: {err}")
    })
}

fn copy_dir_recursively_to_temp(
    source: &Utf8Path,
    target: &Utf8Path,
) -> Result<Utf8PathBuf, String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("cannot copy cache entry `{target}` without a parent"))?;
    let name = target
        .file_name()
        .ok_or_else(|| format!("cannot copy cache entry `{target}` without a file name"))?;
    let temp = parent.join(format!(".tama-cache-tmp-{name}-{}", std::process::id()));
    remove_path_if_exists(&temp)?;
    if let Err(err) = copy_dir_recursively(source, &temp) {
        let _ = remove_path_if_exists(&temp);
        return Err(err);
    }
    Ok(temp)
}

fn path_exists(path: &Utf8Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("failed to inspect `{path}`: {err}")),
    }
}

fn remove_path_if_exists(path: &Utf8Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to inspect `{path}`: {err}")),
    };
    let result = if metadata.file_type().is_symlink() || metadata.is_file() {
        std::fs::remove_file(path)
    } else if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|err| format!("failed to remove `{path}`: {err}"))
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
    let rendered = command_display(program, args);
    tracing::debug!(command = %rendered, "running external command");
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| format!("failed to run `{rendered}`: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{rendered}` failed with status {status}"))
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

fn forge_install_optional_flags() -> Result<Vec<&'static str>, String> {
    let output = ProcessCommand::new("forge")
        .args(["install", "--help"])
        .output()
        .map_err(|err| format!("failed to inspect `forge install --help`: {err}"))?;
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(select_forge_install_optional_flags(&help))
}

fn select_forge_install_optional_flags(help: &str) -> Vec<&'static str> {
    let mut flags = Vec::new();
    // `forge install <repo>@<tag> --shallow` can clone the default branch
    // without the requested tag, then fail tag checkout even when the tag
    // exists upstream. Keep pinned installs reproducible and non-shallow.
    if help.contains("--no-commit") {
        flags.push("--no-commit");
    }
    flags
}

fn offline_init_instructions() -> [&'static str; 6] {
    [
        "Offline follow-up:",
        "  for offline check/build, `.lake/packages` or `TAMA_LAKE_PACKAGE_CACHE` must contain clean Git checkouts at the revisions in `lake-manifest.json`.",
        "  when network access is available, run:",
        "  lake update",
        "  git init  # if this project is not already inside a Git worktree",
        "  forge install foundry-rs/forge-std@v1.16.1",
    ]
}

fn init_next_steps(path: &Utf8Path) -> Vec<String> {
    let mut steps = vec!["Next steps:".to_string()];
    if path.as_str() != "." {
        steps.push(format!("  cd {path}"));
    }
    steps.extend(
        [
            "tama doctor",
            "tama check",
            "tama build",
            "tama test",
            "tama audit",
        ]
        .map(|command| format!("  {command}")),
    );
    steps
}

fn check_status_json(targets: &[String; 2]) -> Result<String, String> {
    serde_json::to_string_pretty(&serde_json::json!({
        "status": "ok",
        "targets": targets
    }))
    .map_err(|err| err.to_string())
}

fn build_status_json(status: &tama_build::BuildStatus) -> Result<String, String> {
    serde_json::to_string_pretty(&serde_json::json!({
        "manifests": status.manifests,
    }))
    .map_err(|err| err.to_string())
}

fn format_audit_report(
    root: &Utf8Path,
    report: &tama_audit::AuditReport,
    deny_warnings: bool,
    palette: &Palette,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}\n", paint(palette.header, "Audit scope:")));
    out.push_str(&format!("  {}: {root}\n", paint(palette.dim, "project")));
    out.push_str(&format!(
        "  {}: {}\n",
        paint(palette.dim, "manifests"),
        report.summary.manifest_dir,
    ));
    out.push_str(&format!(
        "  {}: {}\n",
        paint(palette.dim, "contracts"),
        audit_contracts_summary(&report.summary.contracts),
    ));
    out.push_str(&format!(
        "  {}: {}\n",
        paint(palette.dim, "warnings"),
        if deny_warnings { "fail" } else { "report" },
    ));
    out.push_str(&format!("\n{}\n", paint(palette.header, "Checks:")));
    for check in &report.summary.checks {
        let (errors, warnings, infos) = audit_issue_counts(report, *check);
        let status = audit_check_status(errors, warnings, infos, deny_warnings);
        let style = audit_status_style(status, palette);
        let padded_status = format!("{status:<4}");
        out.push_str(&format!(
            "  {} {:<15} {}{}\n",
            paint(style, padded_status),
            check.as_str(),
            check.description(),
            audit_count_suffix(errors, warnings, infos),
        ));
    }

    if !report.issues.is_empty() {
        out.push_str(&format!(
            "\n{} ({}):\n",
            paint(palette.header, "Findings"),
            paint(
                palette.count,
                format_count(report.issues.len(), "issue", "issues")
            ),
        ));
        for issue in &report.issues {
            let severity_style = severity_style(issue.severity, palette);
            out.push_str(&format!(
                "  {} {} {}",
                paint(severity_style, severity_label(issue.severity)),
                issue.check,
                issue.code,
            ));
            if let Some(contract) = &issue.contract {
                out.push_str(&format!(" ({contract})"));
            }
            out.push_str(&format!(": {}\n", issue.message));
            if let Some(path) = &issue.path {
                out.push_str(&format!("    {}: {path}\n", paint(palette.dim, "path"),));
            }
        }
    }

    let (errors, warnings, infos) = total_audit_issue_counts(report);
    out.push('\n');
    let (summary_prefix, summary_style) = if report.has_failures(deny_warnings) {
        ("Audit failed:", palette.fail)
    } else if warnings > 0 {
        ("Audit passed with warnings:", palette.warn)
    } else {
        ("Audit passed:", palette.ok)
    };
    out.push_str(&format!("{} ", paint(summary_style, summary_prefix)));
    out.push_str(&format!(
        "{}, {}, {}",
        paint(
            palette.count,
            format_count(report.summary.checks.len(), "check", "checks")
        ),
        paint(
            palette.count,
            format_count(report.summary.contracts.len(), "contract", "contracts")
        ),
        paint(
            palette.count,
            format_count(report.issues.len(), "issue", "issues")
        ),
    ));
    if !report.issues.is_empty() {
        out.push_str(&format!(
            " ({}, {}, {})",
            paint(
                palette.severity_error,
                format_count(errors, "error", "errors")
            ),
            paint(
                palette.severity_warning,
                format_count(warnings, "warning", "warnings")
            ),
            paint(palette.severity_info, format_count(infos, "info", "info")),
        ));
    }
    out
}

fn audit_status_style(status: &str, palette: &Palette) -> anstyle::Style {
    match status {
        "fail" => palette.fail,
        "warn" => palette.warn,
        "info" => palette.info,
        _ => palette.ok,
    }
}

fn severity_style(severity: tama_audit::Severity, palette: &Palette) -> anstyle::Style {
    match severity {
        tama_audit::Severity::Error => palette.severity_error,
        tama_audit::Severity::Warning => palette.severity_warning,
        tama_audit::Severity::Info => palette.severity_info,
    }
}

fn audit_contracts_summary(contracts: &[String]) -> String {
    match contracts {
        [] => "none (no valid manifests loaded)".to_string(),
        [one] => one.clone(),
        many if many.len() <= 4 => many.join(", "),
        many => format!("{}, ... ({} total)", many[..4].join(", "), many.len()),
    }
}

fn audit_issue_counts(
    report: &tama_audit::AuditReport,
    check: tama_audit::Check,
) -> (usize, usize, usize) {
    let check = check.as_str();
    report
        .issues
        .iter()
        .filter(|issue| issue.check == check)
        .fold((0, 0, 0), |(errors, warnings, infos), issue| {
            match issue.severity {
                tama_audit::Severity::Error => (errors + 1, warnings, infos),
                tama_audit::Severity::Warning => (errors, warnings + 1, infos),
                tama_audit::Severity::Info => (errors, warnings, infos + 1),
            }
        })
}

fn total_audit_issue_counts(report: &tama_audit::AuditReport) -> (usize, usize, usize) {
    report
        .issues
        .iter()
        .fold((0, 0, 0), |(errors, warnings, infos), issue| {
            match issue.severity {
                tama_audit::Severity::Error => (errors + 1, warnings, infos),
                tama_audit::Severity::Warning => (errors, warnings + 1, infos),
                tama_audit::Severity::Info => (errors, warnings, infos + 1),
            }
        })
}

fn audit_check_status(
    errors: usize,
    warnings: usize,
    infos: usize,
    deny_warnings: bool,
) -> &'static str {
    if errors > 0 || (deny_warnings && warnings > 0) {
        "fail"
    } else if warnings > 0 {
        "warn"
    } else if infos > 0 {
        "info"
    } else {
        "ok"
    }
}

fn audit_count_suffix(errors: usize, warnings: usize, infos: usize) -> String {
    let counts = [
        (errors, "error", "errors"),
        (warnings, "warning", "warnings"),
        (infos, "info", "info"),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .map(|(count, singular, plural)| format_count(count, singular, plural))
    .collect::<Vec<_>>();
    if counts.is_empty() {
        String::new()
    } else {
        format!(" ({})", counts.join(", "))
    }
}

fn format_count(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn severity_label(severity: tama_audit::Severity) -> &'static str {
    match severity {
        tama_audit::Severity::Error => "error",
        tama_audit::Severity::Warning => "warning",
        tama_audit::Severity::Info => "info",
    }
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

fn prefixed_test_args(args: Vec<String>, offline: bool) -> Vec<String> {
    let add_offline = offline && !args.iter().any(|arg| arg == "--offline");
    let mut out = Vec::with_capacity(args.len() + 1 + usize::from(add_offline));
    out.push("test".to_string());
    if add_offline {
        out.push("--offline".to_string());
    }
    out.extend(args);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CleanReport {
    entries: Vec<CleanEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CleanEntry {
    path: Utf8PathBuf,
    action: CleanAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanAction {
    Removed,
    AlreadyClean,
}

fn clean(root: &Utf8PathBuf, deep: bool) -> Result<CleanReport, String> {
    let (paths, foundry) = clean_paths(root)?;
    let configured_lake_build_dir = match tama_config::parse_lake_build_dir(root) {
        Ok(path) => path,
        Err(tama_config::Error::UnsupportedLakefile(_)) => None,
        Err(err) => return Err(err.to_string()),
    };
    let mut report = CleanReport {
        entries: Vec::new(),
    };
    for rel in [
        paths.out.join("yul"),
        paths.out.join("abi"),
        paths.out.join("bytecode"),
        paths.out.join("solc-json"),
        paths.out.join("manifest"),
        paths.out.join("lean"),
        paths.out.join("trust-probe"),
        foundry.out,
        foundry.cache,
    ] {
        let removed = remove_project_dir(root, &rel)?;
        record_clean_entry(&mut report, rel, removed);
    }
    if let Some(rel) = configured_lake_build_dir {
        let removed = remove_project_dir(root, &rel)?;
        record_clean_entry(&mut report, rel, removed);
    }
    let removed = remove_generated_dir(root, &paths.generated)?;
    record_clean_entry(&mut report, paths.generated, removed);
    for rel in [
        paths.out.join("verity-modules.txt"),
        paths.out.join("trust-report.json"),
        paths.out.join("layout-report.json"),
        paths.out.join("assumption-report.json"),
    ] {
        let removed = remove_project_file(root, &rel)?;
        record_clean_entry(&mut report, rel, removed);
    }
    if deep {
        let rel = Utf8PathBuf::from(".lake");
        let removed = remove_project_dir(root, &rel)?;
        record_clean_entry(&mut report, rel, removed);
    }
    Ok(report)
}

fn record_clean_entry(report: &mut CleanReport, path: Utf8PathBuf, removed: bool) {
    if report.entries.iter().any(|entry| entry.path == path) {
        return;
    }
    report.entries.push(CleanEntry {
        path,
        action: if removed {
            CleanAction::Removed
        } else {
            CleanAction::AlreadyClean
        },
    });
}

fn format_clean_report(report: &CleanReport, palette: &Palette) -> String {
    let removed = report
        .entries
        .iter()
        .filter(|entry| entry.action == CleanAction::Removed)
        .count();
    let clean = report.entries.len().saturating_sub(removed);
    let mut out = String::new();
    out.push_str(&format!("{}\n", paint(palette.header, "Cleaned:")));
    for entry in &report.entries {
        let (status, style) = match entry.action {
            CleanAction::Removed => ("ok  ", palette.ok),
            CleanAction::AlreadyClean => ("skip", palette.skip),
        };
        let detail = match entry.action {
            CleanAction::Removed => "removed",
            CleanAction::AlreadyClean => "already clean",
        };
        let path = format!("{:<32}", entry.path);
        out.push_str(&format!(
            "  {} {} {detail}\n",
            paint(style, status),
            paint(palette.path, path),
        ));
    }
    out.push_str(&format!(
        "{} {}, {}",
        paint(palette.header, "Clean completed:"),
        paint(
            palette.count,
            format_count(removed, "target removed", "targets removed")
        ),
        paint(
            palette.count,
            format_count(clean, "already clean", "already clean")
        ),
    ));
    out
}

fn clean_paths(
    root: &Utf8Path,
) -> Result<(tama_config::PathsConfig, tama_config::FoundryConfig), String> {
    let paths = match tama_config::load_config(root) {
        Ok(config) => config.paths,
        Err(err) if is_missing_config_error(&err) => tama_config::PathsConfig::default(),
        Err(err) => return Err(err.to_string()),
    };
    let foundry = tama_config::parse_foundry_config(root).map_err(|err| err.to_string())?;
    Ok((paths, foundry))
}

fn is_missing_config_error(err: &tama_config::Error) -> bool {
    matches!(
        err,
        tama_config::Error::Common(tama_common::Error::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound
    )
}

fn remove_generated_dir(root: &Utf8Path, rel: &Utf8Path) -> Result<bool, String> {
    ensure_project_relative(rel, "clean")?;
    let path = root.join(rel);
    if !path.exists() {
        return Ok(false);
    }
    ensure_generated_tree_cleanable(&path)?;
    std::fs::remove_dir_all(&path).map_err(|err| format!("failed to remove `{path}`: {err}"))?;
    Ok(true)
}

fn ensure_generated_tree_cleanable(path: &Utf8Path) -> Result<(), String> {
    for entry in std::fs::read_dir(path).map_err(|err| format!("failed to read `{path}`: {err}"))? {
        let entry = entry.map_err(|err| format!("failed to read `{path}` entry: {err}"))?;
        let child = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|path| format!("generated path `{}` is not UTF-8", path.display()))?;
        let metadata = std::fs::symlink_metadata(&child)
            .map_err(|err| format!("failed to inspect `{child}`: {err}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("refusing to clean generated symlink `{child}`"));
        }
        if metadata.is_dir() {
            ensure_generated_tree_cleanable(&child)?;
        } else if metadata.is_file()
            && !tama_common::has_generated_header(&child).map_err(|err| err.to_string())?
        {
            return Err(tama_common::Error::GeneratedFileModified(child).to_string());
        }
    }
    Ok(())
}

fn remove_project_dir(root: &Utf8Path, rel: &Utf8Path) -> Result<bool, String> {
    ensure_project_relative(rel, "clean")?;
    let path = root.join(rel);
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&path).map_err(|err| format!("failed to remove `{path}`: {err}"))?;
    Ok(true)
}

fn remove_project_file(root: &Utf8Path, rel: &Utf8Path) -> Result<bool, String> {
    ensure_project_relative(rel, "clean")?;
    let path = root.join(rel);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("failed to remove `{path}`: {err}")),
    }
}

fn ensure_project_relative(path: &Utf8Path, action: &str) -> Result<(), String> {
    if path.as_str().is_empty()
        || path == Utf8Path::new(".")
        || path.is_absolute()
        || path.components().any(|part| part.as_str() == "..")
    {
        Err(format!(
            "refusing to {action} path outside project: `{path}`"
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct EnvVarGuard {
        name: &'static str,
        old: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: impl Into<OsString>) -> Self {
            let old = std::env::var_os(name);
            std::env::set_var(name, value.into());
            Self { name, old }
        }

        fn unset(name: &'static str) -> Self {
            let old = std::env::var_os(name);
            std::env::remove_var(name);
            Self { name, old }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

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
            prefixed_test_args(
                vec![
                    "--match-test".to_string(),
                    "foo".to_string(),
                    "-vvv".to_string(),
                ],
                false
            ),
            vec!["test", "--match-test", "foo", "-vvv"]
        );
    }

    #[test]
    fn offline_test_args_gate_forge_network_without_rewriting_filters() {
        assert_eq!(
            prefixed_test_args(
                vec![
                    "--match-test".to_string(),
                    "foo".to_string(),
                    "-vvv".to_string(),
                ],
                true
            ),
            vec!["test", "--offline", "--match-test", "foo", "-vvv"]
        );
        assert_eq!(
            prefixed_test_args(vec!["--offline".to_string(), "-vvv".to_string()], true),
            vec!["test", "--offline", "-vvv"]
        );
    }

    #[test]
    fn test_args_parse_after_double_dash() {
        let cli =
            Cli::try_parse_from(["tama", "test", "--", "--match-test", "foo", "-vvv"]).unwrap();
        match cli.command {
            Command::Test(args) => assert_eq!(
                prefixed_test_args(args.forge_args, cli.offline),
                vec!["test", "--match-test", "foo", "-vvv"]
            ),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn test_args_parse_direct_hyphenated_forge_args() {
        let cli = Cli::try_parse_from(["tama", "test", "--match-test", "foo", "-vvv"]).unwrap();
        match cli.command {
            Command::Test(args) => assert_eq!(
                prefixed_test_args(args.forge_args, cli.offline),
                vec!["test", "--match-test", "foo", "-vvv"]
            ),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn global_offline_is_translated_for_test_passthrough() {
        let cli =
            Cli::try_parse_from(["tama", "--offline", "test", "--match-test", "foo"]).unwrap();
        assert!(cli.offline);
        match cli.command {
            Command::Test(args) => assert_eq!(
                prefixed_test_args(args.forge_args, cli.offline),
                vec!["test", "--offline", "--match-test", "foo"]
            ),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn offline_init_instructions_are_actionable() {
        let instructions = offline_init_instructions().join("\n");
        assert!(instructions.contains("lake update"));
        assert!(instructions.contains("git init"));
        assert!(instructions.contains("TAMA_LAKE_PACKAGE_CACHE"));
        assert!(instructions.contains("forge install foundry-rs/forge-std@v1.16.1"));
        assert!(!instructions.contains("--shallow"));
        assert!(!instructions.contains("--no-git"));
    }

    #[test]
    fn init_next_steps_are_actionable() {
        assert_eq!(
            init_next_steps(Utf8Path::new("demo")),
            vec![
                "Next steps:",
                "  cd demo",
                "  tama doctor",
                "  tama check",
                "  tama build",
                "  tama test",
                "  tama audit",
            ]
        );
        assert!(!init_next_steps(Utf8Path::new("."))
            .iter()
            .any(|line| line == "  cd ."));
    }

    #[test]
    fn help_lists_command_descriptions() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("Create a new ERC20Lite starter project"));
        assert!(help.contains(
            "Build proofs, Verity artifacts, bytecode, Solidity bridges, and Forge targets"
        ));
        assert!(help.contains("Pass through directly to forge test"));
        assert!(help.contains("Diagnose toolchain and project drift"));
        assert!(help.contains("Resolve configured external tool binaries"));
    }

    #[test]
    fn build_help_explains_proof_validation_and_options() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("build")
            .unwrap()
            .render_help()
            .to_string();

        assert!(help.contains(
            "Build proofs, Verity artifacts, bytecode, Solidity bridges, and Forge targets"
        ));
        assert!(help.contains("What gets built:"));
        assert!(help.contains("proof-check"));
        assert!(help.contains("Verity emits Yul, ABI, storage, assumption, and trust reports"));
        assert!(help.contains("Proofs are first-class build inputs"));
        assert!(help.contains("--contract <Name>"));
        assert!(help.contains("--no-solc"));
        assert!(help.contains("--no-forge"));
        assert!(help.contains("--locked"));
    }

    #[test]
    fn audit_help_explains_checks_and_failure_policy() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("audit")
            .unwrap()
            .render_help()
            .to_string();

        assert!(help.contains("Audit generated Verity artifacts after a build"));
        assert!(help.contains("Run `tama audit` for the full suite"));
        assert!(help.contains("structure"));
        assert!(help.contains("selectors"));
        assert!(help.contains("storage-layout"));
        assert!(help.contains("coverage"));
        assert!(help.contains("trust-boundary"));
        assert!(help.contains("--deny-warnings"));
        assert!(help.contains("Aliases: `storage`"));
    }

    #[test]
    fn other_command_help_explains_purpose_and_shape() {
        let cases = [
            (
                "init",
                [
                    "commented",
                    "deploy script",
                    "Online init also runs `lake update`",
                ],
            ),
            (
                "new",
                [
                    "source, spec, proof",
                    "TODO proofs marked with `sorry`",
                    "aggregate modules",
                ],
            ),
            (
                "check",
                [
                    "configured implementation and spec aggregate modules",
                    "Proof modules",
                    "Use `tama build`",
                ],
            ),
            (
                "test",
                [
                    "forge test",
                    "passes the remaining arguments",
                    "Forge's by design",
                ],
            ),
            (
                "inspect",
                ["Fields:", "runtime-bytecode", "machine-readable output"],
            ),
            (
                "clean",
                [
                    "generated Tama and Foundry build output",
                    "Tama's generated-file",
                    "--deep",
                ],
            ),
            (
                "doctor",
                ["required tools", "With --fix", "user-owned source, spec"],
            ),
            ("toolchain", ["Commands:", "solc", "global --offline"]),
            (
                "install",
                ["lakefile.toml", "pure Lake packages", "Add @rev"],
            ),
            (
                "remove",
                ["lakefile.toml", "runs `lake update`", "already be present"],
            ),
            (
                "update",
                ["runs `lake update`", "--package <name>", "--no-lake"],
            ),
        ];

        for (subcommand, snippets) in cases {
            let mut command = Cli::command();
            let help = command
                .find_subcommand_mut(subcommand)
                .unwrap()
                .render_help()
                .to_string();
            for snippet in snippets {
                assert!(
                    help.contains(snippet),
                    "{subcommand} help did not contain {snippet:?}\n{help}"
                );
            }
        }
    }

    #[test]
    fn audit_human_output_lists_scope_checks_and_findings() {
        let report = tama_audit::AuditReport {
            issues: vec![tama_audit::Issue {
                check: "coverage".to_string(),
                contract: Some("Counter".to_string()),
                severity: tama_audit::Severity::Error,
                code: "TAMA_COVERAGE_MISSING".to_string(),
                message: "postcondition has no mirror test".to_string(),
                path: Some("test/verity/Counter.t.sol".into()),
            }],
            summary: tama_audit::AuditSummary {
                checks: vec![tama_audit::Check::Structure, tama_audit::Check::Coverage],
                contracts: vec!["Counter".to_string()],
                manifest_dir: "artifacts/manifest".into(),
            },
        };

        let output = format_audit_report(
            Utf8Path::new("/tmp/project"),
            &report,
            false,
            &Palette::plain(),
        );

        assert!(output.contains("Audit scope:"));
        assert!(output.contains("project: /tmp/project"));
        assert!(output.contains("manifests: artifacts/manifest"));
        assert!(output.contains("contracts: Counter"));
        assert!(output.contains("warnings: report"));
        assert!(output.contains("ok   structure"));
        assert!(output.contains("fail coverage"));
        assert!(output.contains("Findings (1 issue):"));
        assert!(output.contains("error coverage TAMA_COVERAGE_MISSING (Counter)"));
        assert!(output.contains("path: test/verity/Counter.t.sol"));
        assert!(output.contains("Audit failed: 2 checks, 1 contract, 1 issue"));
    }

    #[test]
    fn audit_human_output_pass_summary_is_informative() {
        let report = tama_audit::AuditReport {
            issues: Vec::new(),
            summary: tama_audit::AuditSummary {
                checks: tama_audit::Check::all().to_vec(),
                contracts: vec!["ERC20Lite".to_string()],
                manifest_dir: "artifacts/manifest".into(),
            },
        };

        let output = format_audit_report(
            Utf8Path::new("/tmp/project"),
            &report,
            false,
            &Palette::plain(),
        );

        assert!(output.contains("ok   structure"));
        assert!(output.contains("ok   trust-boundary"));
        assert!(output.contains("Audit passed: 5 checks, 1 contract, 0 issues"));
        assert!(!output.trim().eq("Audit passed"));
    }

    #[test]
    fn command_progress_line_is_scan_friendly() {
        let plain = anstyle::Style::new();
        assert_eq!(
            command_progress_line("run", plain, "cache", "prepare Lake package checkouts"),
            "  run  cache        prepare Lake package checkouts"
        );
        assert_eq!(
            command_progress_line("skip", plain, "forge", "offline; forge-std not installed"),
            "  skip forge        offline; forge-std not installed"
        );
    }

    #[test]
    fn doctor_human_output_keeps_tool_versions_on_one_line() {
        let report = tama_toolchain::DoctorReport {
            tools: vec![
                tama_toolchain::ToolStatus::Ok(tama_toolchain::Tool {
                    name: "forge".to_string(),
                    path: "forge".into(),
                    version: Some(
                        "forge Version: 1.6.0\nCommit SHA: abc\nBuild Profile: dist".to_string(),
                    ),
                }),
                tama_toolchain::ToolStatus::Ok(tama_toolchain::Tool {
                    name: "solc".to_string(),
                    path: "solc".into(),
                    version: Some(
                        "solc, the solidity compiler commandline interface\nVersion: 0.8.33"
                            .to_string(),
                    ),
                }),
            ],
            lock_current: Some(true),
            notes: Vec::new(),
        };

        let output = format_doctor_report(&report, false, &Palette::plain());

        assert!(output.contains("ok   forge           1.6.0"));
        assert!(output.contains("ok   solc            0.8.33"));
        assert!(!output.contains("Commit SHA"));
        assert!(output.contains("Doctor passed: 3 checks, 0 issues"));
    }

    #[test]
    fn doctor_notes_legacy_module_layout() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        for path in ["TamaSrc.lean", "TamaSpec.lean", "TamaProof.lean"] {
            tama_common::write_string(&root.join(path), "import src.Counter\n").unwrap();
        }
        let config = tama_config::TamaConfig {
            project: tama_config::ProjectConfig {
                name: "x".to_string(),
                verity: "v".to_string(),
            },
            paths: Default::default(),
            modules: None,
            yul: tama_config::YulConfig {
                solc: "0.8.33".to_string(),
                optimizer: true,
                optimizer_runs: 200,
                yul_optimizer: true,
                evm_version: "cancun".to_string(),
                metadata_hash: "none".to_string(),
            },
            trust: Default::default(),
            coverage: Default::default(),
        };
        let mut report = tama_toolchain::DoctorReport::default();

        report_legacy_module_layout(&mut report, &root, &config);

        assert!(report.notes.iter().any(|note| note
            .contains("legacy TamaSrc/TamaSpec/TamaProof module layout detected; add [modules]")));
    }

    #[test]
    fn clean_report_summarizes_removed_and_clean_targets() {
        let report = CleanReport {
            entries: vec![
                CleanEntry {
                    path: "artifacts/yul".into(),
                    action: CleanAction::Removed,
                },
                CleanEntry {
                    path: "cache".into(),
                    action: CleanAction::AlreadyClean,
                },
            ],
        };

        let output = format_clean_report(&report, &Palette::plain());

        assert!(output.contains("ok   artifacts/yul"));
        assert!(output.contains("skip cache"));
        assert!(output.contains("Clean completed: 1 target removed, 1 already clean"));
    }

    #[test]
    fn check_json_status_is_machine_readable() {
        let targets = ["Tamago".to_string(), "Tamago.Spec".to_string()];
        let value: serde_json::Value =
            serde_json::from_str(&check_status_json(&targets).unwrap()).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(
            value["targets"],
            serde_json::json!(["Tamago", "Tamago.Spec"])
        );

        let cli = Cli::try_parse_from(["tama", "--json", "check"]).unwrap();
        assert!(cli.json);
        assert!(matches!(cli.command, Command::Check));
    }

    #[test]
    fn build_json_status_shape_is_stable() {
        let status = tama_build::BuildStatus {
            manifests: vec![Utf8PathBuf::from("artifacts/manifest/Counter.json")],
        };

        assert_eq!(
            build_status_json(&status).unwrap(),
            r#"{
  "manifests": [
    "artifacts/manifest/Counter.json"
  ]
}"#
        );
    }

    #[test]
    fn forge_install_flag_preserves_submodule_installs() {
        assert_eq!(
            select_forge_install_optional_flags("Options:\n      --no-git\n      --commit\n"),
            Vec::<&'static str>::new()
        );
        assert_eq!(
            select_forge_install_optional_flags("Options:\n      --shallow\n      --no-commit\n"),
            vec!["--no-commit"]
        );
        assert_eq!(
            select_forge_install_optional_flags("Options:\n      --shallow\n"),
            Vec::<&'static str>::new()
        );
    }

    #[test]
    fn package_revision_detection_ignores_scp_urls() {
        assert!(!package_has_explicit_git_rev("lfglabs-dev/verity-erc20"));
        assert!(package_has_explicit_git_rev(
            "lfglabs-dev/verity-erc20@v0.2.0"
        ));
        assert!(!package_has_explicit_git_rev(
            "git@github.com:lfglabs-dev/verity.git"
        ));
        assert!(package_has_explicit_git_rev(
            "git@github.com:lfglabs-dev/verity.git@v0.2.0"
        ));
    }

    #[test]
    fn update_package_args_target_one_lake_package() {
        assert_eq!(lake_update_args(None), vec!["update"]);
        assert_eq!(lake_update_args(Some("mathlib")), vec!["update", "mathlib"]);

        let cli = Cli::try_parse_from(["tama", "update", "--package", "mathlib"]).unwrap();
        match cli.command {
            Command::Update {
                package,
                no_lake,
                no_forge,
            } => {
                assert_eq!(package.as_deref(), Some("mathlib"));
                assert!(!no_lake);
                assert!(!no_forge);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn default_branch_validation_returns_head_commit() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().join("package.git")).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        run_process("git", &["init"], Some(&repo)).unwrap();
        run_process("git", &["branch", "-M", "trunk"], Some(&repo)).unwrap();
        tama_common::write_string(
            &repo.join("tama.toml"),
            "[project]\nname='dep'\nverity='v'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        tama_common::write_string(
            &repo.join("lakefile.toml"),
            "name = \"metadata_dep\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        run_process("git", &["add", "tama.toml", "lakefile.toml"], Some(&repo)).unwrap();
        run_process(
            "git",
            &[
                "-c",
                "user.name=Tama Test",
                "-c",
                "user.email=tama@example.test",
                "commit",
                "-m",
                "init",
            ],
            Some(&repo),
        )
        .unwrap();

        let expected = git_head_rev(&repo).unwrap();
        let resolved = validate_remote_tama_package(repo.as_str(), "main", false).unwrap();

        assert_eq!(resolved.rev, expected);
        assert_eq!(resolved.name, "metadata_dep");
    }

    #[test]
    fn pure_lake_remote_dependency_points_to_manual_install() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(dir.path().join("package.git")).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        run_process("git", &["init"], Some(&repo)).unwrap();
        tama_common::write_string(
            &repo.join("lakefile.toml"),
            "name = \"pure_lake_dep\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        run_process("git", &["add", "lakefile.toml"], Some(&repo)).unwrap();
        run_process(
            "git",
            &[
                "-c",
                "user.name=Tama Test",
                "-c",
                "user.email=tama@example.test",
                "commit",
                "-m",
                "init",
            ],
            Some(&repo),
        )
        .unwrap();

        let err = validate_remote_tama_package(repo.as_str(), "main", false).unwrap_err();

        assert!(err.contains("does not contain tama.toml"));
        assert!(err.contains("pure Lake packages are outside `tama install`'s scope"));
        assert!(err.contains("add this dependency manually to lakefile.toml"));
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
    fn project_commands_report_missing_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("outside")).unwrap();
        std::fs::create_dir_all(&root).unwrap();

        let err = project_root(Some(root.clone())).unwrap_err();

        assert!(err.contains("could not find Tama project root"));
        assert!(err.contains(root.as_str()));
    }

    #[test]
    fn ensure_git_worktree_initializes_fresh_project_root() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        assert!(!is_git_worktree(&root).unwrap());
        ensure_git_worktree(&root).unwrap();
        assert!(is_git_worktree(&root).unwrap());
        assert!(root.join(".git").is_dir());
    }

    #[test]
    fn lake_package_cache_seeds_and_records_missing_packages() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();

        init_git_package(&cache.join("mathlib"), "cached").unwrap();
        seed_lake_package_cache(&root, &cache).unwrap();
        assert!(root.join(".lake/packages/mathlib/package.txt").is_file());

        init_git_package(&root.join(".lake/packages/verity"), "resolved").unwrap();
        sync_lake_package_cache(&root, &cache).unwrap();
        assert!(cache.join("verity/package.txt").is_file());
    }

    #[test]
    fn lake_package_cache_refreshes_existing_entries_after_update() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();

        init_git_package(&cache.join("verity"), "old").unwrap();
        init_git_package(&root.join(".lake/packages/verity"), "new").unwrap();

        sync_lake_package_cache(&root, &cache).unwrap();

        let package = std::fs::read_to_string(cache.join("verity/package.txt")).unwrap();
        assert_eq!(package, "new");
    }

    #[test]
    fn lake_package_cache_refresh_skips_clean_matching_revisions() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        let package = root.join(".lake/packages/verity");

        init_git_package(&package, "same").unwrap();
        sync_lake_package_cache(&root, &cache).unwrap();

        assert!(!should_replace_cached_package(&package, &cache.join("verity")).unwrap());

        tama_common::write_string(&cache.join("verity/untracked.lean"), "local change\n").unwrap();
        assert!(should_replace_cached_package(&package, &cache.join("verity")).unwrap());
    }

    #[test]
    fn lake_package_cache_refreshes_matching_revisions_missing_build_artifacts() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        let package = root.join(".lake/packages/verity");

        init_git_package(&package, "same").unwrap();
        tama_common::write_string(&package.join(".gitignore"), ".lake/\n").unwrap();
        run_process("git", &["add", ".gitignore"], Some(&package)).unwrap();
        run_process(
            "git",
            &[
                "-c",
                "user.name=Tama Test",
                "-c",
                "user.email=tama@example.test",
                "commit",
                "-m",
                "ignore lake builds",
            ],
            Some(&package),
        )
        .unwrap();

        sync_lake_package_cache(&root, &cache).unwrap();
        assert!(!should_replace_cached_package(&package, &cache.join("verity")).unwrap());

        let build_artifact = package.join(".lake/build/lib/Compiler.olean");
        tama_common::write_string(&build_artifact, "compiled\n").unwrap();

        assert!(git_worktree_clean(&package).unwrap());
        assert!(should_replace_cached_package(&package, &cache.join("verity")).unwrap());

        sync_lake_package_cache(&root, &cache).unwrap();

        assert_eq!(
            std::fs::read_to_string(cache.join("verity/.lake/build/lib/Compiler.olean")).unwrap(),
            "compiled\n"
        );
    }

    #[test]
    fn lake_package_cache_sync_skips_dirty_packages() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();

        init_git_package(&root.join(".lake/packages/verity"), "resolved").unwrap();
        tama_common::write_string(
            &root.join(".lake/packages/verity/untracked.lean"),
            "local change\n",
        )
        .unwrap();

        sync_lake_package_cache(&root, &cache).unwrap();

        assert!(!cache.join("verity").exists());
    }

    #[test]
    fn successful_build_refreshes_cache_from_existing_lake_packages() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, cache.as_str());

        init_git_package(&root.join(".lake/packages/verity"), "resolved").unwrap();

        try_refresh_lake_package_cache_after_success(&root).unwrap();

        assert_eq!(
            std::fs::read_to_string(cache.join("verity/package.txt")).unwrap(),
            "resolved"
        );
    }

    #[test]
    fn lake_package_cache_for_build_seeds_only_manifest_matching_revisions() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        let verity_rev = init_git_package(&cache.join("verity"), "matching").unwrap();
        init_git_package(&cache.join("mathlib"), "stale").unwrap();

        tama_common::write_string(
            &root.join("lake-manifest.json"),
            &format!(
                r#"{{
  "version": "1.1.0",
  "packages": [
    {{"type": "git", "name": "verity", "rev": "{verity_rev}"}},
    {{"type": "git", "name": "mathlib", "rev": "0000000000000000000000000000000000000000"}}
  ]
}}
"#
            ),
        )
        .unwrap();

        seed_lake_package_cache_from_manifest(&root, &cache).unwrap();

        assert!(root.join(".lake/packages/verity/package.txt").is_file());
        assert!(!root.join(".lake/packages/mathlib").exists());
    }

    #[test]
    fn lake_package_cache_for_build_skips_dirty_matching_revisions() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        let verity = cache.join("verity");
        let verity_rev = init_git_package(&verity, "matching").unwrap();
        tama_common::write_string(&verity.join("untracked.lean"), "local change\n").unwrap();

        tama_common::write_string(
            &root.join("lake-manifest.json"),
            &format!(
                r#"{{
  "version": "1.1.0",
  "packages": [
    {{"type": "git", "name": "verity", "rev": "{verity_rev}"}}
  ]
}}
"#
            ),
        )
        .unwrap();

        seed_lake_package_cache_from_manifest(&root, &cache).unwrap();

        assert!(!root.join(".lake/packages/verity").exists());
    }

    #[test]
    fn offline_build_preflight_seeds_manifest_matching_cache_before_lake() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        let verity_rev = init_git_package(&cache.join("verity"), "cached").unwrap();
        write_lake_manifest(&root, &[("verity", &verity_rev)]);
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, cache.as_str());

        prepare_lake_packages_for_build(&root, true).unwrap();

        assert!(root.join(".lake/packages/verity/package.txt").is_file());
    }

    #[test]
    fn offline_build_preflight_replaces_clean_stale_package_from_cache() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        let expected_rev = init_git_package(&cache.join("verity"), "cached").unwrap();
        let target = root.join(".lake/packages/verity");
        let stale_rev = init_git_package(&target, "stale").unwrap();
        assert_ne!(stale_rev, expected_rev);
        write_lake_manifest(&root, &[("verity", &expected_rev)]);
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, cache.as_str());

        prepare_lake_packages_for_build(&root, true).unwrap();

        assert_eq!(
            std::fs::read_to_string(target.join("package.txt")).unwrap(),
            "cached"
        );
        assert_eq!(git_head_rev(&target).unwrap(), expected_rev);
    }

    #[test]
    fn offline_build_preflight_preserves_dirty_stale_package_even_when_cache_matches() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        let expected_rev = init_git_package(&cache.join("verity"), "cached").unwrap();
        let target = root.join(".lake/packages/verity");
        init_git_package(&target, "stale").unwrap();
        tama_common::write_string(&target.join("untracked.lean"), "local change\n").unwrap();
        write_lake_manifest(&root, &[("verity", &expected_rev)]);
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, cache.as_str());

        let err = prepare_lake_packages_for_build(&root, true).unwrap_err();

        assert!(err.contains("match lake-manifest.json revision"));
        assert_eq!(
            std::fs::read_to_string(target.join("package.txt")).unwrap(),
            "stale"
        );
        assert!(target.join("untracked.lean").is_file());
    }

    #[test]
    fn offline_build_preflight_rejects_missing_lake_package() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        write_lake_manifest(
            &root,
            &[("verity", "0000000000000000000000000000000000000000")],
        );
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, "off");

        let err = prepare_lake_packages_for_build(&root, true).unwrap_err();

        assert!(err.contains("--offline requires Lake package `verity`"));
        assert!(err.contains(".lake/packages/verity"));
        assert!(err.contains("lake update"));
    }

    #[test]
    fn offline_build_preflight_rejects_stale_lake_package_revision() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        init_git_package(&root.join(".lake/packages/verity"), "local").unwrap();
        write_lake_manifest(
            &root,
            &[("verity", "0000000000000000000000000000000000000000")],
        );
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, "off");

        let err = prepare_lake_packages_for_build(&root, true).unwrap_err();

        assert!(err.contains("match lake-manifest.json revision"));
        assert!(err.contains(".lake/packages/verity"));
    }

    #[test]
    fn offline_build_preflight_rejects_dirty_lake_package() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let verity = root.join(".lake/packages/verity");
        let verity_rev = init_git_package(&verity, "local").unwrap();
        tama_common::write_string(&verity.join("untracked.lean"), "local change\n").unwrap();
        write_lake_manifest(&root, &[("verity", &verity_rev)]);
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, "off");

        let err = prepare_lake_packages_for_build(&root, true).unwrap_err();

        assert!(err.contains("clean Git worktree"));
        assert!(err.contains(".lake/packages/verity"));
    }

    #[cfg(unix)]
    #[test]
    fn lake_package_cache_copy_failures_do_not_leave_partial_entries() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let source = root.join("source");
        let packages = root.join(".lake/packages");
        let target = packages.join("verity");
        let temp = packages.join(format!(".tama-cache-tmp-verity-{}", std::process::id()));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&packages).unwrap();
        tama_common::write_string(&source.join("package.txt"), "new").unwrap();
        let unreadable = source.join("unreadable.txt");
        tama_common::write_string(&unreadable, "blocked").unwrap();
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();

        let new_result = copy_dir_recursively_into_new(&source, &target);

        assert!(new_result.is_err());
        assert!(!target.exists());
        assert!(!temp.exists());

        std::fs::create_dir_all(&target).unwrap();
        tama_common::write_string(&target.join("package.txt"), "old").unwrap();

        let replace_result = replace_dir_recursively(&source, &target);

        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(replace_result.is_err());
        assert_eq!(
            std::fs::read_to_string(target.join("package.txt")).unwrap(),
            "old"
        );
        assert!(!temp.exists());
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
    fn default_lake_package_cache_is_optional_when_home_is_missing() {
        assert_eq!(
            default_lake_package_cache_from_parts(None, None, false),
            None
        );
        assert_eq!(
            default_lake_package_cache_from_parts(None, None, true),
            None
        );
        assert_eq!(
            default_lake_package_cache_from_parts(Some(PathBuf::from("/home/alice")), None, false)
                .as_deref(),
            Some(Utf8Path::new("/home/alice/.cache/tama/lake-packages"))
        );
        assert_eq!(
            default_lake_package_cache_from_parts(
                None,
                Some(PathBuf::from("/tmp/cache-home")),
                false
            )
            .as_deref(),
            Some(Utf8Path::new("/tmp/cache-home/tama/lake-packages"))
        );
    }

    fn init_git_package(path: &Utf8Path, text: &str) -> Result<String, String> {
        std::fs::create_dir_all(path).map_err(|err| err.to_string())?;
        let cwd = path.to_owned();
        run_process("git", &["init"], Some(&cwd))?;
        tama_common::write_string(&path.join("package.txt"), text)
            .map_err(|err| err.to_string())?;
        run_process("git", &["add", "package.txt"], Some(&cwd))?;
        run_process(
            "git",
            &[
                "-c",
                "user.name=Tama Test",
                "-c",
                "user.email=tama@example.test",
                "commit",
                "-m",
                "init",
            ],
            Some(&cwd),
        )?;
        git_head_rev(path)
    }

    fn write_lake_manifest(root: &Utf8Path, package_revs: &[(&str, &str)]) {
        let packages = package_revs
            .iter()
            .map(|(name, rev)| {
                serde_json::json!({
                    "type": "git",
                    "name": name,
                    "rev": rev,
                })
            })
            .collect::<Vec<_>>();
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "version": "1.1.0",
            "packages": packages,
        }))
        .unwrap()
            + "\n";
        tama_common::write_string(&root.join("lake-manifest.json"), &text).unwrap();
    }

    #[test]
    fn local_fake_tama_dependency_installs_and_removes_with_lake_metadata() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let dependency_root = Utf8PathBuf::from_path_buf(dir.path().join("dep-worktree")).unwrap();
        tama_common::write_string(
            &dependency_root.join("tama.toml"),
            "[project]\nname='dep'\nverity='v'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        tama_common::write_string(
            &dependency_root.join("lakefile.toml"),
            "name = \"utility_dep\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let bin = Utf8PathBuf::from_path_buf(dir.path().join("bin")).unwrap();
        let lake = bin.join("lake");
        let log = Utf8PathBuf::from_path_buf(dir.path().join("lake.log")).unwrap();
        tama_common::write_string(
            &lake,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
             echo 'Lake version 5.0.0-src+test (Lean version 4.22.0)'\n\
             exit 0\n\
             fi\n\
             printf '%s\\n' \"$@\" >> \"$TAMA_TEST_LAKE_LOG\"\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&lake).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&lake, permissions).unwrap();
        }
        let mut path_entries = vec![bin.as_std_path().to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            path_entries.extend(std::env::split_paths(&path));
        }
        let _path_guard = EnvVarGuard::set("PATH", std::env::join_paths(path_entries).unwrap());
        let _lake_log_guard = EnvVarGuard::set("TAMA_TEST_LAKE_LOG", log.as_str());
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, "off");

        install_package(&root, "../dep-worktree", false, false).unwrap();

        let installed = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        assert!(installed.contains("name = \"utility_dep\""));
        assert!(installed.contains("path = \"../dep-worktree\""));
        assert!(
            tama_config::lock_drift(&root, &tama_config::load_lock(&root).unwrap())
                .unwrap()
                .is_empty()
        );

        mutate_dependencies(&root, false, false, |root| {
            tama_config::remove_lake_dependency(root, "utility_dep").map_err(|err| err.to_string())
        })
        .unwrap();

        let removed = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        assert!(!removed.contains("name = \"utility_dep\""));
        let lake_log = tama_common::read_to_string(&log).unwrap();
        assert_eq!(
            lake_log.lines().collect::<Vec<_>>(),
            vec!["update", "update"]
        );
    }

    #[test]
    fn local_pure_lake_dependency_points_to_manual_install() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let dependency_root = Utf8PathBuf::from_path_buf(dir.path().join("pure-lake")).unwrap();
        tama_common::write_string(
            &dependency_root.join("lakefile.toml"),
            "name = \"pure_lake_dep\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let lakefile_before = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();

        let err = install_package(&root, "../pure-lake", false, false).unwrap_err();

        assert!(err.contains("local dependency `../pure-lake` does not contain tama.toml"));
        assert!(err.contains("pure Lake packages are outside `tama install`'s scope"));
        assert!(err.contains("add this dependency manually to lakefile.toml"));
        assert_eq!(
            tama_common::read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile_before
        );
    }

    #[test]
    fn offline_dependency_mutations_fail_before_editing() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let mut called = false;

        let err = mutate_dependencies(&root, false, true, |_| {
            called = true;
            Ok(())
        })
        .unwrap_err();

        assert!(err.contains("--offline"));
        assert!(!called);
    }

    #[cfg(unix)]
    #[test]
    fn install_locked_rejects_stale_inputs_before_remote_validation() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let lakefile_before = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname = \"starter\"\nverity = \"0.1.0\"\n\n[yul]\nsolc = \"0.8.34\"\n",
        )
        .unwrap();

        let bin = Utf8PathBuf::from_path_buf(dir.path().join("bin")).unwrap();
        let git = bin.join("git");
        let log = Utf8PathBuf::from_path_buf(dir.path().join("git.log")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        tama_common::write_string(
            &git,
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" >> \"$TAMA_TEST_GIT_LOG\"\n\
             exit 42\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&git).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&git, permissions).unwrap();
        }
        let mut path_entries = vec![bin.as_std_path().to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            path_entries.extend(std::env::split_paths(&path));
        }
        let _path_guard = EnvVarGuard::set("PATH", std::env::join_paths(path_entries).unwrap());
        let _git_log_guard = EnvVarGuard::set("TAMA_TEST_GIT_LOG", log.as_str());
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, "off");

        let err =
            install_package(&root, "https://example.test/org/dep.git", false, true).unwrap_err();

        assert!(err.contains("lockfile is stale"));
        assert!(err.contains("tama.toml"));
        assert!(!log.exists());
        assert_eq!(
            tama_common::read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile_before
        );
    }

    #[cfg(unix)]
    #[test]
    fn dependency_mutation_restores_lakefile_when_lake_update_fails() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let lakefile_before = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        let manifest_before =
            tama_common::read_to_string(&root.join("lake-manifest.json")).unwrap();

        let bin = Utf8PathBuf::from_path_buf(dir.path().join("bin")).unwrap();
        let lake = bin.join("lake");
        tama_common::write_string(
            &lake,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
             echo 'Lake version 5.0.0-src+test (Lean version 4.22.0)'\n\
             exit 0\n\
             fi\n\
             printf '%s\\n' '{\"partial\":true}' > lake-manifest.json\n\
             exit 42\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&lake).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&lake, permissions).unwrap();
        }
        let mut path_entries = vec![bin.as_std_path().to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            path_entries.extend(std::env::split_paths(&path));
        }
        let _path_guard = EnvVarGuard::set("PATH", std::env::join_paths(path_entries).unwrap());
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, "off");

        let err = mutate_dependencies(&root, false, false, |root| {
            tama_config::upsert_lake_dependency(
                root,
                &tama_config::LakeDependency {
                    name: "mathlib".to_string(),
                    source: tama_config::LakeDependencySource::Git {
                        url: "https://github.com/leanprover-community/mathlib4.git".to_string(),
                        rev: "v4.22.0".to_string(),
                    },
                },
            )
            .map_err(|err| err.to_string())
        })
        .unwrap_err();

        assert!(err.contains("lake update"));
        assert_eq!(
            tama_common::read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile_before
        );
        assert_eq!(
            tama_common::read_to_string(&root.join("lake-manifest.json")).unwrap(),
            manifest_before
        );
    }

    #[test]
    fn offline_update_requires_external_tools_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();

        let err = update_project(&root, false, true, false, true, None).unwrap_err();
        assert!(err.contains("--no-lake"));

        let before = tama_config::load_lock(&root)
            .unwrap()
            .resolved
            .get("verity_rev")
            .cloned()
            .unwrap();
        update_project(&root, false, true, true, true, None).unwrap();
        let after = tama_config::load_lock(&root)
            .unwrap()
            .resolved
            .get("verity_rev")
            .cloned()
            .unwrap();
        assert_eq!(after, before);
        assert!(tama_common::read_to_string(&root.join("lakefile.toml"))
            .unwrap()
            .contains(&format!("rev = \"{before}\"")));
    }

    #[test]
    fn no_lake_update_refuses_verity_dependency_drift_before_editing() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let old_rev = tama_config::load_lock(&root)
            .unwrap()
            .resolved
            .get("verity_rev")
            .cloned()
            .unwrap();
        let config = tama_common::read_to_string(&root.join("tama.toml"))
            .unwrap()
            .replace(&format!("verity = \"{old_rev}\""), "verity = \"0.2.0\"");
        tama_common::write_string(&root.join("tama.toml"), &config).unwrap();
        let mut lakefile = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        lakefile.push_str("\n# user lake option\n[leanOptions]\npp.unicode.fun = true\n");
        tama_common::write_string(&root.join("lakefile.toml"), &lakefile).unwrap();

        let err = update_project(&root, false, true, true, true, None).unwrap_err();

        assert!(err.contains("--no-lake"));
        assert!(err.contains("lake update"));
        assert_eq!(
            tama_common::read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile
        );
    }

    #[cfg(unix)]
    #[test]
    fn update_restores_lakefile_when_verity_sync_fails_lake_update() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let old_rev = tama_config::load_lock(&root)
            .unwrap()
            .resolved
            .get("verity_rev")
            .cloned()
            .unwrap();
        let lakefile_before = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        let manifest_before =
            tama_common::read_to_string(&root.join("lake-manifest.json")).unwrap();
        let config = tama_common::read_to_string(&root.join("tama.toml"))
            .unwrap()
            .replace(&format!("verity = \"{old_rev}\""), "verity = \"0.2.0\"");
        tama_common::write_string(&root.join("tama.toml"), &config).unwrap();

        let bin = Utf8PathBuf::from_path_buf(dir.path().join("bin")).unwrap();
        let lake = bin.join("lake");
        tama_common::write_string(
            &lake,
            "#!/bin/sh\nprintf '%s\\n' '{\"partial\":true}' > lake-manifest.json\nexit 42\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&lake).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&lake, permissions).unwrap();
        }
        let mut path_entries = vec![bin.as_std_path().to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            path_entries.extend(std::env::split_paths(&path));
        }
        let _path_guard = EnvVarGuard::set("PATH", std::env::join_paths(path_entries).unwrap());
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, "off");

        let err = update_project(&root, false, false, false, true, None).unwrap_err();

        assert!(err.contains("lake update"));
        assert_eq!(
            tama_common::read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile_before
        );
        assert_eq!(
            tama_common::read_to_string(&root.join("lake-manifest.json")).unwrap(),
            manifest_before
        );
    }

    #[test]
    fn targeted_package_update_refuses_unrelated_verity_drift_before_tools() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let old_rev = tama_config::load_lock(&root)
            .unwrap()
            .resolved
            .get("verity_rev")
            .cloned()
            .unwrap();
        let config = tama_common::read_to_string(&root.join("tama.toml"))
            .unwrap()
            .replace(&format!("verity = \"{old_rev}\""), "verity = \"0.2.0\"");
        tama_common::write_string(&root.join("tama.toml"), &config).unwrap();

        let err = update_project(&root, false, false, false, false, Some("mathlib")).unwrap_err();

        assert!(err.contains("Verity dependency is drifting"));
        assert_eq!(
            tama_config::load_lock(&root)
                .unwrap()
                .resolved
                .get("verity_rev")
                .map(String::as_str),
            Some(old_rev.as_str())
        );
    }

    #[cfg(unix)]
    #[test]
    fn targeted_package_update_restores_manifest_when_lake_update_fails() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let lakefile_before = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        let manifest_before =
            tama_common::read_to_string(&root.join("lake-manifest.json")).unwrap();

        let bin = Utf8PathBuf::from_path_buf(dir.path().join("bin")).unwrap();
        let lake = bin.join("lake");
        tama_common::write_string(
            &lake,
            "#!/bin/sh\nprintf '%s\\n' '{\"partial\":true}' > lake-manifest.json\nexit 42\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&lake).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&lake, permissions).unwrap();
        }
        let mut path_entries = vec![bin.as_std_path().to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            path_entries.extend(std::env::split_paths(&path));
        }
        let _path_guard = EnvVarGuard::set("PATH", std::env::join_paths(path_entries).unwrap());
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, "off");

        let err = update_project(&root, false, false, false, true, Some("mathlib")).unwrap_err();

        assert!(err.contains("lake update mathlib"));
        assert_eq!(
            tama_common::read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile_before
        );
        assert_eq!(
            tama_common::read_to_string(&root.join("lake-manifest.json")).unwrap(),
            manifest_before
        );
    }

    #[test]
    fn global_locked_guard_rejects_stale_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();

        enforce_locked_if_requested(&root, true).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname = \"starter\"\nverity = \"0.1.0\"\n\n[yul]\nsolc = \"0.8.34\"\n",
        )
        .unwrap();

        let err = enforce_locked_if_requested(&root, true).unwrap_err();
        assert!(err.contains("lockfile is stale"));
        assert!(err.contains("tama.toml"));
        enforce_locked_if_requested(&root, false).unwrap();
    }

    #[test]
    fn build_locked_rejects_stale_inputs_before_seeding_lake_cache() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        let cache = Utf8PathBuf::from_path_buf(dir.path().join("cache")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let rev = init_git_package(&cache.join("verity"), "cached").unwrap();
        tama_common::write_string(
            &root.join("lake-manifest.json"),
            &format!(
                r#"{{
  "version": "1.1.0",
  "packages": [
    {{"type": "git", "name": "verity", "rev": "{rev}"}}
  ]
}}
"#
            ),
        )
        .unwrap();
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, cache.as_str());

        let err = run(Cli {
            root: Some(root.clone()),
            locked: true,
            offline: false,
            json: false,
            verbose: 0,
            color: ColorChoice::Never,
            no_color: false,
            command: Command::Build(BuildArgs {
                no_solc: true,
                no_forge: true,
                contract_: None,
            }),
        })
        .unwrap_err();

        assert!(err.contains("lockfile is stale"));
        assert!(!root.join(".lake/packages/verity").exists());
    }

    #[test]
    fn doctor_fix_refreshes_lock_inputs() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        tama_common::write_string(
            &root.join("verity/src/MyProtocol.lean"),
            "import MyProtocol.ERC20Lite\n-- changed\n",
        )
        .unwrap();
        let stale = doctor_report(Some(&root)).unwrap();
        assert_eq!(stale.lock_current, Some(false));
        apply_doctor_fix(&root, Some(&root), false).unwrap();
        let current = doctor_report(Some(&root)).unwrap();
        assert_eq!(current.lock_current, Some(true));
    }

    #[test]
    fn doctor_locked_refuses_stale_inputs_before_fixing() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let lock_before = tama_common::read_to_string(&root.join("tama.lock")).unwrap();
        tama_common::write_string(
            &root.join("verity/src/MyProtocol.lean"),
            "import MyProtocol.ERC20Lite\n-- changed\n",
        )
        .unwrap();

        let err = run(Cli {
            root: Some(root.clone()),
            locked: true,
            offline: false,
            json: false,
            verbose: 0,
            color: ColorChoice::Never,
            no_color: false,
            command: Command::Doctor { fix: true },
        })
        .unwrap_err();

        assert!(err.contains("lockfile is stale"));
        assert!(err.contains("verity/src/MyProtocol.lean"));
        assert_eq!(
            tama_common::read_to_string(&root.join("tama.lock")).unwrap(),
            lock_before
        );
        let lock = tama_config::load_lock(&root).unwrap();
        assert!(!tama_config::lock_drift(&root, &lock).unwrap().is_empty());
    }

    #[test]
    fn doctor_reports_and_repairs_missing_generated_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        std::fs::remove_dir_all(root.join("artifacts/yul")).unwrap();
        std::fs::remove_dir_all(root.join("artifacts/lean")).unwrap();

        let stale = doctor_report(Some(&root)).unwrap();
        assert!(stale.tools.iter().any(|status| {
            matches!(
                status,
                tama_toolchain::ToolStatus::Incompatible { name, found, .. }
                    if name == "project directories"
                        && found.contains("artifacts/yul")
                        && found.contains("artifacts/lean")
            )
        }));

        apply_doctor_fix(&root, Some(&root), false).unwrap();

        assert!(root.join("artifacts/yul").is_dir());
        assert!(root.join("artifacts/lean").is_dir());
        let repaired = doctor_report(Some(&root)).unwrap();
        assert!(!repaired.tools.iter().any(|status| {
            matches!(
                status,
                tama_toolchain::ToolStatus::Incompatible { name, .. }
                    if name == "project directories"
            )
        }));
    }

    #[test]
    fn doctor_fix_requires_project_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let err = apply_doctor_fix(&root, None, false).unwrap_err();

        assert!(err.contains("requires a Tama project"));
        assert!(!root.join("artifacts").exists());
        assert!(!root.join("src/generated/verity").exists());
    }

    #[test]
    fn doctor_fix_uses_configured_generated_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let config = tama_common::read_to_string(&root.join("tama.toml"))
            .unwrap()
            .replace("out = \"artifacts\"", "out = \"build/tama\"")
            .replace(
                "generated_solidity = \"src/generated/verity\"",
                "generated_solidity = \"gen/verity\"",
            );
        tama_common::write_string(&root.join("tama.toml"), &config).unwrap();
        let lakefile = tama_common::read_to_string(&root.join("lakefile.toml"))
            .unwrap()
            .replace("buildDir = \"artifacts/lean\"", "buildDir = \"build/lake\"");
        tama_common::write_string(&root.join("lakefile.toml"), &lakefile).unwrap();

        apply_doctor_fix(&root, Some(&root), false).unwrap();

        assert!(root.join("build/tama").is_dir());
        assert!(root.join("build/tama/yul").is_dir());
        assert!(root.join("build/tama/bytecode").is_dir());
        assert!(root.join("build/tama/trust-probe").is_dir());
        assert!(root.join("build/lake").is_dir());
        assert!(root.join("gen/verity").is_dir());
    }

    #[test]
    fn doctor_fix_refuses_offline_verity_dependency_sync_before_editing() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let old_rev = tama_config::load_lock(&root)
            .unwrap()
            .resolved
            .get("verity_rev")
            .cloned()
            .unwrap();
        let config = tama_common::read_to_string(&root.join("tama.toml"))
            .unwrap()
            .replace(&format!("verity = \"{old_rev}\""), "verity = \"0.2.0\"");
        tama_common::write_string(&root.join("tama.toml"), &config).unwrap();
        let lakefile_before = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();

        let err = apply_doctor_fix(&root, Some(&root), true).unwrap_err();

        assert!(err.contains("--offline"));
        assert!(err.contains("lake update"));
        assert_eq!(
            tama_common::read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile_before
        );
    }

    #[cfg(unix)]
    #[test]
    fn doctor_fix_restores_dependency_files_when_lake_update_fails() {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        tama_project::init(&root, tama_project::InitOptions::default()).unwrap();
        let old_rev = tama_config::load_lock(&root)
            .unwrap()
            .resolved
            .get("verity_rev")
            .cloned()
            .unwrap();
        let config = tama_common::read_to_string(&root.join("tama.toml"))
            .unwrap()
            .replace(&format!("verity = \"{old_rev}\""), "verity = \"0.2.0\"");
        tama_common::write_string(&root.join("tama.toml"), &config).unwrap();
        let lakefile_before = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        let manifest_before =
            tama_common::read_to_string(&root.join("lake-manifest.json")).unwrap();

        let bin = Utf8PathBuf::from_path_buf(dir.path().join("bin")).unwrap();
        let lake = bin.join("lake");
        tama_common::write_string(
            &lake,
            "#!/bin/sh\nprintf '%s\\n' '{\"partial\":true}' > lake-manifest.json\nexit 42\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&lake).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&lake, permissions).unwrap();
        }
        let mut path_entries = vec![bin.as_std_path().to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            path_entries.extend(std::env::split_paths(&path));
        }
        let _path_guard = EnvVarGuard::set("PATH", std::env::join_paths(path_entries).unwrap());
        let _cache_guard = EnvVarGuard::set(LAKE_PACKAGE_CACHE_ENV, "off");

        let err = apply_doctor_fix(&root, Some(&root), false).unwrap_err();

        assert!(err.contains("lake update"));
        assert_eq!(
            tama_common::read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile_before
        );
        assert_eq!(
            tama_common::read_to_string(&root.join("lake-manifest.json")).unwrap(),
            manifest_before
        );
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
    fn clean_uses_configured_artifact_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            r#"[project]
name = "x"
verity = "v"

[paths]
out = "build/tama"
generated = "gen/verity"

[yul]
solc = "0.8.33"
"#,
        )
        .unwrap();
        tama_common::write_string(&root.join("build/tama/yul/Token.yul"), "").unwrap();
        tama_common::write_string(&root.join("build/tama/abi/Token.abi.json"), "").unwrap();
        tama_common::write_string(&root.join("build/tama/trust-report.json"), "{}").unwrap();
        tama_common::write_generated(&root.join("gen/verity/TokenIface.sol"), "").unwrap();
        tama_common::write_string(&root.join("artifacts/yul/Token.yul"), "").unwrap();

        clean(&root, false).unwrap();

        assert!(!root.join("build/tama/yul").exists());
        assert!(!root.join("build/tama/abi").exists());
        assert!(!root.join("build/tama/trust-report.json").exists());
        assert!(!root.join("gen/verity").exists());
        assert!(root.join("artifacts/yul/Token.yul").exists());
    }

    #[test]
    fn clean_uses_foundry_profile_default_out_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname='x'\nverity='v'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join("foundry.toml"),
            "[profile.default]\nout = 'forge-out'\n",
        )
        .unwrap();
        tama_common::write_string(&root.join("forge-out/Counter.json"), "{}\n").unwrap();

        clean(&root, false).unwrap();

        assert!(!root.join("forge-out").exists());
    }

    #[test]
    fn clean_uses_foundry_profile_default_cache_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname='x'\nverity='v'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join("foundry.toml"),
            "[profile.default]\ncache_path = 'forge-cache'\n",
        )
        .unwrap();
        tama_common::write_string(&root.join("forge-cache/solidity-files-cache.json"), "{}\n")
            .unwrap();
        tama_common::write_string(&root.join("cache/keep.txt"), "user cache\n").unwrap();

        clean(&root, false).unwrap();

        assert!(!root.join("forge-cache").exists());
        assert!(root.join("cache/keep.txt").is_file());
    }

    #[test]
    fn clean_removes_configured_lake_build_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("lakefile.toml"),
            r#"name = "demo"
buildDir = "build/lean"
"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("artifacts/lean")).unwrap();
        std::fs::create_dir_all(root.join("build/lean")).unwrap();

        clean(&root, false).unwrap();

        assert!(!root.join("artifacts/lean").exists());
        assert!(!root.join("build/lean").exists());
    }

    #[test]
    fn clean_refuses_empty_configured_lake_build_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("lakefile.toml"),
            r#"name = "demo"
buildDir = ""
"#,
        )
        .unwrap();

        let err = clean(&root, false).unwrap_err();

        assert!(err.contains("refusing to clean path outside project"));
    }

    #[test]
    fn clean_refuses_foundry_out_at_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname='x'\nverity='v'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        tama_common::write_string(&root.join("foundry.toml"), "[profile.default]\nout = '.'\n")
            .unwrap();
        tama_common::write_string(&root.join("keep.txt"), "keep\n").unwrap();

        let err = clean(&root, false).unwrap_err();

        assert!(err.contains("configured path `foundry.out`"));
        assert!(root.join("keep.txt").is_file());
    }

    #[test]
    fn clean_rejects_invalid_project_config_before_removing_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(&root.join("tama.toml"), "not = [valid").unwrap();
        tama_common::write_string(&root.join("artifacts/yul/Counter.yul"), "").unwrap();

        let err = clean(&root, false).unwrap_err();

        assert!(err.contains("failed to parse"));
        assert!(root.join("artifacts/yul/Counter.yul").is_file());
    }

    #[test]
    fn clean_rejects_invalid_foundry_config_before_removing_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname='x'\nverity='v'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        tama_common::write_string(&root.join("foundry.toml"), "not = [valid").unwrap();
        tama_common::write_string(&root.join("artifacts/yul/Counter.yul"), "").unwrap();

        let err = clean(&root, false).unwrap_err();

        assert!(err.contains("failed to parse"));
        assert!(root.join("artifacts/yul/Counter.yul").is_file());
    }

    #[test]
    fn clean_refuses_hand_edited_generated_solidity() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname='x'\nverity='v'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        tama_common::write_generated(&root.join("src/generated/verity/TokenIface.sol"), "")
            .unwrap();
        tama_common::write_string(&root.join("src/generated/verity/Manual.sol"), "// user\n")
            .unwrap();

        let err = clean(&root, false).unwrap_err();

        assert!(err.contains("hand-edited generated file"));
        assert!(root.join("src/generated/verity").exists());
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
            tools: vec![
                tama_toolchain::ToolStatus::Ok(tama_toolchain::Tool {
                    name: "solc".to_string(),
                    path: "solc".into(),
                    version: Some("Version: 0.8.32+commit.test".to_string()),
                }),
                tama_toolchain::ToolStatus::Ok(tama_toolchain::Tool {
                    name: "lake".to_string(),
                    path: "lake".into(),
                    version: Some(
                        "Lake version 5.0.0-src+abc123 (Lean version 4.29.1)".to_string(),
                    ),
                }),
            ],
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
        mark_version_mismatch(
            &mut report,
            "lake",
            "4.22.0",
            tama_toolchain::parse_lake_lean_version,
        );
        assert!(matches!(
            &report.tools[1],
            tama_toolchain::ToolStatus::Incompatible { found, expected, .. }
                if found == "4.29.1" && expected == "4.22.0"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn toolchain_solc_resolves_version_from_tama_config() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("project")).unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().join("home")).unwrap();
        let solc = home.join(".tama/solc/0.8.34/solc");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(solc.parent().unwrap()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname='x'\nverity='v'\n[yul]\nsolc='0.8.34'\n",
        )
        .unwrap();
        tama_common::write_string(
            &solc,
            "#!/bin/sh\nprintf '%s\\n' 'Version: 0.8.34+commit.test'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&solc).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&solc, permissions).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", home.as_str());
        let _path_guard = EnvVarGuard::set("PATH", "");
        let _solc_guard = EnvVarGuard::unset("TAMA_SOLC");

        let tool = resolve_configured_solc(&root, true).unwrap();

        assert_eq!(tool.path, solc);
        assert!(tool
            .version
            .as_deref()
            .is_some_and(|version| version.contains("0.8.34")));
    }

    #[test]
    fn doctor_reports_tama_version_and_lake_build_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("lakefile.toml"),
            "name = \"x\"\nbuildDir = \"artifacts/lean\"\n",
        )
        .unwrap();
        let mut report = tama_toolchain::DoctorReport {
            tools: vec![tama_status()],
            lock_current: None,
            notes: vec![],
        };

        report_lake_build_dir(&mut report, &root);

        assert!(report.tools.iter().any(|status| matches!(
            status,
            tama_toolchain::ToolStatus::Ok(tool)
                if tool.name == "tama"
                    && tool.version.as_deref() == Some(env!("CARGO_PKG_VERSION"))
        )));
        assert!(report.tools.iter().any(|status| matches!(
            status,
            tama_toolchain::ToolStatus::Ok(tool)
                if tool.name == "lake buildDir"
                    && tool.version.as_deref() == Some("artifacts/lean")
        )));
        assert!(!doctor_report_has_failures(&report));
    }

    #[test]
    fn doctor_reports_invalid_tama_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(&root.join("tama.toml"), "not = [valid").unwrap();

        let report = doctor_report(Some(&root)).unwrap();

        assert!(doctor_report_has_failures(&report));
        assert!(report
            .notes
            .iter()
            .any(|note| note.contains("tama.toml could not be read")));
        assert!(report.tools.iter().any(|status| matches!(
            status,
            tama_toolchain::ToolStatus::Incompatible { name, expected, .. }
                if name == "tama.toml" && expected == "valid Tama project config"
        )));
    }

    #[test]
    fn doctor_marks_verity_resolution_mismatch() {
        let config = tama_config::TamaConfig {
            project: tama_config::ProjectConfig {
                name: "x".to_string(),
                verity: "9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e".to_string(),
            },
            paths: Default::default(),
            modules: None,
            yul: tama_config::YulConfig {
                solc: "0.8.33".to_string(),
                optimizer: true,
                optimizer_runs: 200,
                yul_optimizer: true,
                evm_version: "cancun".to_string(),
                metadata_hash: "none".to_string(),
            },
            trust: Default::default(),
            coverage: Default::default(),
        };
        let mut report = tama_toolchain::DoctorReport::default();
        let lock = tama_config::TamaLock {
            version: 1,
            resolved: std::collections::BTreeMap::from([
                (
                    "verity_git".to_string(),
                    "https://github.com/lfglabs-dev/verity.git".to_string(),
                ),
                ("verity_rev".to_string(), "v0.1.0".to_string()),
                ("lake.verity.input_rev".to_string(), "v0.1.0".to_string()),
                (
                    "lake.verity.rev".to_string(),
                    "1111111111111111111111111111111111111111".to_string(),
                ),
            ]),
            inputs: Default::default(),
            yul: Default::default(),
        };

        check_verity_resolution(&mut report, &config, &lock);

        assert!(doctor_report_has_failures(&report));
        assert!(matches!(
            &report.tools[0],
            tama_toolchain::ToolStatus::Incompatible { name, found, expected }
                if name == "verity"
                    && found.contains("verity_rev=v0.1.0")
                    && expected.contains("9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e")
        ));
    }

    #[test]
    fn doctor_accepts_matching_verity_resolution() {
        let config = tama_config::TamaConfig {
            project: tama_config::ProjectConfig {
                name: "x".to_string(),
                verity: "9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e".to_string(),
            },
            paths: Default::default(),
            modules: None,
            yul: tama_config::YulConfig {
                solc: "0.8.33".to_string(),
                optimizer: true,
                optimizer_runs: 200,
                yul_optimizer: true,
                evm_version: "cancun".to_string(),
                metadata_hash: "none".to_string(),
            },
            trust: Default::default(),
            coverage: Default::default(),
        };
        let mut report = tama_toolchain::DoctorReport::default();
        let lock = tama_config::TamaLock {
            version: 1,
            resolved: std::collections::BTreeMap::from([
                (
                    "verity_git".to_string(),
                    "https://github.com/lfglabs-dev/verity.git".to_string(),
                ),
                (
                    "verity_rev".to_string(),
                    "9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e".to_string(),
                ),
                (
                    "lake.verity.input_rev".to_string(),
                    "9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e".to_string(),
                ),
                (
                    "lake.verity.rev".to_string(),
                    "9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e".to_string(),
                ),
            ]),
            inputs: Default::default(),
            yul: Default::default(),
        };

        check_verity_resolution(&mut report, &config, &lock);

        assert!(!doctor_report_has_failures(&report));
        assert!(matches!(
            &report.tools[0],
            tama_toolchain::ToolStatus::Ok(tool)
                if tool.name == "verity"
                    && tool
                        .version
                        .as_deref()
                        .is_some_and(|version| version.contains("resolved 9b0114"))
        ));
    }

    #[test]
    fn doctor_report_failures_drive_exit_status() {
        let ok = tama_toolchain::DoctorReport {
            tools: vec![tama_toolchain::ToolStatus::Ok(tama_toolchain::Tool {
                name: "git".to_string(),
                path: "git".into(),
                version: None,
            })],
            lock_current: Some(true),
            notes: vec![],
        };
        assert!(!doctor_report_has_failures(&ok));

        let missing_tool = tama_toolchain::DoctorReport {
            tools: vec![tama_toolchain::ToolStatus::Missing {
                name: "solc".to_string(),
                remediation: "install solc".to_string(),
            }],
            lock_current: Some(true),
            notes: vec![],
        };
        assert!(doctor_report_has_failures(&missing_tool));

        let stale_lock = tama_toolchain::DoctorReport {
            tools: vec![],
            lock_current: Some(false),
            notes: vec![],
        };
        assert!(doctor_report_has_failures(&stale_lock));
    }
}
