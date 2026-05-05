use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use tama_config::{TamaConfig, TamaLock};
use tama_manifest::{
    Abi, ArtifactPaths, Constructor, ContractManifest, ErrorEntry, Event, Function, LeanModules,
    Obligation, Param, SourcePaths, StorageEntry, SCHEMA,
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Common(#[from] tama_common::Error),
    #[error(transparent)]
    Config(#[from] tama_config::Error),
    #[error(transparent)]
    Manifest(#[from] tama_manifest::Error),
    #[error(transparent)]
    Toolchain(#[from] tama_toolchain::Error),
    #[error("process `{program}` failed: {message}")]
    Process { program: String, message: String },
    #[error("solc reported errors for {contract}: {errors}")]
    SolcErrors { contract: String, errors: String },
    #[error("missing build artifact for {contract}: {path}")]
    MissingArtifact { contract: String, path: Utf8PathBuf },
    #[error("missing required project file for {contract}: {path}")]
    MissingProjectFile { contract: String, path: Utf8PathBuf },
    #[error("could not adapt Verity outputs for {0}")]
    Adapter(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildOptions {
    pub locked: bool,
    pub offline: bool,
    pub no_solc: bool,
    pub no_forge: bool,
    pub contract: Option<String>,
    pub json: bool,
    pub verbose: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStatus {
    pub manifests: Vec<Utf8PathBuf>,
}

#[derive(Debug)]
struct CommandOutput {
    stdout: String,
}

pub struct Lake {
    root: Utf8PathBuf,
    json_output: bool,
}

impl Lake {
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        Self {
            root: root.into(),
            json_output: false,
        }
    }

    pub fn new_json(root: impl Into<Utf8PathBuf>, json_output: bool) -> Self {
        Self {
            root: root.into(),
            json_output,
        }
    }

    pub fn check_src_and_spec(&self) -> Result<()> {
        run_owned(
            "lake",
            &lake_build_args(&["TamaSrc", "TamaSpec"]),
            &self.root,
            self.json_output,
        )
    }

    pub fn build_proofs(&self) -> Result<()> {
        run_owned(
            "lake",
            &lake_build_args(&["TamaProof"]),
            &self.root,
            self.json_output,
        )
    }

    pub fn verity_codegen(&self, config: &TamaConfig, opts: &BuildOptions) -> Result<()> {
        let out = config.paths.out.join("yul");
        let abi = config.paths.out.join("abi");
        fs::create_dir_all(self.root.join(&out))
            .map_err(|source| tama_common::io_error(self.root.join(&out), source))?;
        fs::create_dir_all(self.root.join(&abi))
            .map_err(|source| tama_common::io_error(self.root.join(&abi), source))?;
        clear_verity_codegen_outputs(&self.root, config, opts.contract.as_deref())?;
        let module_manifest = self.root.join(config.paths.out.join("verity-modules.txt"));
        let modules = if let Some(contract) = &opts.contract {
            format!("src.{contract}\n")
        } else {
            let mut modules = discover_modules(&self.root.join(&config.paths.src))?.join("\n");
            if !modules.is_empty() {
                modules.push('\n');
            }
            modules
        };
        tama_common::write_string(&module_manifest, &modules)?;

        let args = vec![
            "exe".to_string(),
            "verity-compiler".to_string(),
            "--manifest".to_string(),
            module_manifest.to_string(),
            "-o".to_string(),
            out.to_string(),
            "--abi-output".to_string(),
            abi.to_string(),
            "--trust-report".to_string(),
            config.paths.out.join("trust-report.json").to_string(),
            "--layout-report".to_string(),
            config.paths.out.join("layout-report.json").to_string(),
            "--assumption-report".to_string(),
            config.paths.out.join("assumption-report.json").to_string(),
        ];
        let _evmyul_guard = EvmyulConformanceGuard::prepare(&self.root)?;
        run_owned("lake", &args, &self.root, self.json_output)
    }
}

pub struct Pipeline {
    root: Utf8PathBuf,
}

impl Pipeline {
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn run(&self, opts: BuildOptions) -> Result<BuildStatus> {
        let config = tama_config::load_config(&self.root)?;
        if let Some(contract) = &opts.contract {
            validate_contract_filter(contract)?;
        }
        let mut lock = load_or_initialize_lock(&self.root)?;
        if opts.locked {
            tama_config::enforce_locked(&self.root, &lock)?;
        }

        let progress = BuildProgress::new(!opts.json);
        progress.scope(&self.root, &config, &opts);

        let lake = Lake::new_json(self.root.clone(), opts.json);
        progress.run(
            "proof-check",
            "Lean elaborates implementations, specs, and proofs",
            "proof modules accepted by Lake",
            || lake.build_proofs(),
        )?;
        progress.run(
            "verity-codegen",
            "Verity emits Yul, ABI, storage, and trust reports",
            "compiler artifacts generated",
            || lake.verity_codegen(&config, &opts),
        )?;
        progress.start(
            "manifest",
            "Tama adapts Verity outputs into contract manifests",
        );
        let mut manifests =
            match adapt_verity_outputs(&self.root, &config, opts.contract.as_deref()) {
                Ok(manifests) => {
                    progress.ok(
                        "manifest",
                        &format!(
                            "{}: {}",
                            format_count(manifests.len(), "manifest", "manifests"),
                            contract_names(&manifests)
                        ),
                    );
                    manifests
                }
                Err(err) => {
                    progress.fail("manifest", "could not adapt Verity outputs");
                    return Err(err);
                }
            };
        progress.run(
            "trust-probe",
            "Lean records proof dependencies for audit",
            "trust-boundary inputs written",
            || generate_trust_probe(&self.root, &config, &manifests),
        )?;
        for manifest in &mut manifests {
            progress.run(
                "validate",
                &format!("{} manifest schema and artifact paths", manifest.contract),
                &format!("{} manifest validated", manifest.contract),
                || {
                    manifest.validate()?;
                    Ok(())
                },
            )?;
            clear_downstream_artifacts(&self.root, manifest)?;
            if opts.no_solc {
                manifest.write_pretty(
                    &self.root.join(
                        config
                            .paths
                            .out
                            .join("manifest")
                            .join(format!("{}.json", manifest.contract)),
                    ),
                )?;
                progress.skip(
                    "solc",
                    &format!(
                        "{} skipped by --no-solc; bytecode remains absent",
                        manifest.contract
                    ),
                );
                progress.skip(
                    "bridge",
                    &format!(
                        "{} skipped because generated bridges require solc bytecode",
                        manifest.contract
                    ),
                );
                continue;
            }
            progress.run(
                "solc",
                &format!(
                    "{} Yul through solc {} standard JSON",
                    manifest.contract, config.yul.solc
                ),
                &format!("{} bytecode and hash written", manifest.contract),
                || compile_yul_standard_json(&self.root, &config, manifest),
            )?;
            progress.run(
                "bridge",
                &format!("{} Solidity interface and deployer", manifest.contract),
                &format!("{} bridge files generated", manifest.contract),
                || generate_bridge(&self.root, manifest),
            )?;
        }
        if should_run_forge(&opts) {
            progress.run(
                "forge",
                "Foundry compiles generated Solidity bridges and project tests",
                "forge build completed",
                || {
                    run_owned(
                        "forge",
                        &forge_build_args(opts.offline),
                        &self.root,
                        opts.json,
                    )
                },
            )?;
        } else if opts.no_solc {
            progress.skip("forge", "skipped because --no-solc omits generated bridges");
        } else {
            progress.skip("forge", "skipped by --no-forge");
        }
        if !opts.locked {
            progress.run(
                "lock",
                "Refresh tama.lock inputs after a successful build",
                "tama.lock current",
                || {
                    tama_config::update_lock_inputs(&self.root, &mut lock)?;
                    tama_config::write_lock(&self.root, &lock)?;
                    Ok(())
                },
            )?;
        } else {
            progress.skip("lock", "checked by --locked; tama.lock was not rewritten");
        }
        Ok(BuildStatus {
            manifests: manifests
                .iter()
                .map(|manifest| {
                    config
                        .paths
                        .out
                        .join("manifest")
                        .join(format!("{}.json", manifest.contract))
                })
                .collect(),
        })
    }
}

struct BuildProgress {
    enabled: bool,
}

impl BuildProgress {
    fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    fn scope(&self, root: &Utf8Path, config: &TamaConfig, opts: &BuildOptions) {
        if !self.enabled {
            return;
        }
        println!("Build scope:");
        println!("  project: {root}");
        println!(
            "  contracts: {}",
            opts.contract.as_deref().unwrap_or("all Verity contracts")
        );
        println!("  proofs: Lean proof modules are built and must elaborate before codegen");
        println!(
            "  solc: {}{}",
            config.yul.solc,
            if opts.no_solc {
                " (skipped by --no-solc)"
            } else {
                ""
            }
        );
        println!(
            "  forge: {}",
            if should_run_forge(opts) {
                "enabled"
            } else if opts.no_solc {
                "skipped because --no-solc was set"
            } else {
                "skipped by --no-forge"
            }
        );
        println!(
            "  lock: {}",
            if opts.locked {
                "checked only (--locked)"
            } else {
                "refreshed after success"
            }
        );
        println!();
        println!("Build steps:");
    }

    fn run<T, F>(&self, name: &str, running: &str, success: &str, f: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
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
        self.line("run", name, detail);
    }

    fn ok(&self, name: &str, detail: &str) {
        self.line("ok", name, detail);
    }

    fn skip(&self, name: &str, detail: &str) {
        self.line("skip", name, detail);
    }

    fn fail(&self, name: &str, detail: &str) {
        self.line("fail", name, detail);
    }

    fn line(&self, status: &str, name: &str, detail: &str) {
        if self.enabled {
            println!("{}", build_progress_line(status, name, detail));
        }
    }
}

fn build_progress_line(status: &str, name: &str, detail: &str) -> String {
    format!("  {status:<4} {name:<15} {detail}")
}

fn contract_names(manifests: &[ContractManifest]) -> String {
    manifests
        .iter()
        .map(|manifest| manifest.contract.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_count(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn load_or_initialize_lock(root: &Utf8Path) -> Result<TamaLock> {
    match tama_config::load_lock(root) {
        Ok(lock) => Ok(lock),
        Err(err) if is_missing_lock_error(&err) => Ok(empty_lock()),
        Err(err) => Err(err.into()),
    }
}

fn empty_lock() -> TamaLock {
    TamaLock {
        version: 1,
        resolved: Default::default(),
        inputs: Default::default(),
        yul: Default::default(),
    }
}

fn is_missing_lock_error(err: &tama_config::Error) -> bool {
    matches!(
        err,
        tama_config::Error::Common(tama_common::Error::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound
    )
}

fn should_run_forge(opts: &BuildOptions) -> bool {
    !opts.no_forge && !opts.no_solc
}

fn lake_build_args(targets: &[&str]) -> Vec<String> {
    let mut args = vec!["build".to_string()];
    args.extend(targets.iter().map(|target| (*target).to_string()));
    args
}

fn forge_build_args(offline: bool) -> Vec<String> {
    let mut args = vec!["build".to_string()];
    if offline {
        args.push("--offline".to_string());
    }
    args
}

fn clear_verity_codegen_outputs(
    root: &Utf8Path,
    config: &TamaConfig,
    contract_filter: Option<&str>,
) -> Result<()> {
    let out = &config.paths.out;
    for path in [
        out.join("layout-report.json"),
        out.join("trust-report.json"),
        out.join("assumption-report.json"),
    ] {
        remove_file_if_exists(&root.join(path))?;
    }

    let abi_dir = root.join(out.join("abi"));
    let yul_dir = root.join(out.join("yul"));
    let manifest_dir = root.join(out.join("manifest"));
    if let Some(contract) = contract_filter {
        for path in [
            abi_dir.join(format!("{contract}.json")),
            abi_dir.join(format!("{contract}.abi.json")),
            abi_dir.join(format!("{contract}.storage.json")),
            yul_dir.join(format!("{contract}.yul")),
            manifest_dir.join(format!("{contract}.json")),
        ] {
            remove_file_if_exists(&path)?;
        }
    } else {
        remove_files_with_extension(&abi_dir, "json")?;
        remove_files_with_extension(&yul_dir, "yul")?;
        remove_files_with_extension(&manifest_dir, "json")?;
    }
    Ok(())
}

fn remove_files_with_extension(dir: &Utf8Path, extension: &str) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in
        fs::read_dir(dir).map_err(|source| tama_common::io_error(dir.to_owned(), source))?
    {
        let entry = entry.map_err(|source| tama_common::io_error(dir.to_owned(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|path| tama_common::Error::NonUtf8Path(path.display().to_string()))?;
        if path.is_file() && path.extension() == Some(extension) {
            remove_file_if_exists(&path)?;
        }
    }
    Ok(())
}

pub fn adapt_verity_outputs(
    root: &Utf8Path,
    config: &TamaConfig,
    contract_filter: Option<&str>,
) -> Result<Vec<ContractManifest>> {
    let abi_dir = root.join(config.paths.out.join("abi"));
    let yul_dir = root.join(config.paths.out.join("yul"));
    let manifest_dir = root.join(config.paths.out.join("manifest"));
    fs::create_dir_all(&manifest_dir)
        .map_err(|source| tama_common::io_error(manifest_dir.clone(), source))?;
    let storage_report =
        read_json_required(&root.join(config.paths.out.join("layout-report.json")))?;
    let mut abi_paths = Vec::new();
    for entry in
        fs::read_dir(&abi_dir).map_err(|source| tama_common::io_error(abi_dir.clone(), source))?
    {
        let entry = entry.map_err(|source| tama_common::io_error(abi_dir.clone(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|path| tama_common::Error::NonUtf8Path(path.display().to_string()))?;
        if path.extension() != Some("json")
            || path.file_name().unwrap_or("").ends_with(".storage.json")
        {
            continue;
        }
        abi_paths.push(path);
    }
    abi_paths.sort();

    let mut all_contracts: Vec<(String, Utf8PathBuf, SourcePaths)> = Vec::new();
    for path in &abi_paths {
        let contract =
            contract_name_from_abi_path(path).ok_or_else(|| Error::Adapter(path.to_string()))?;
        let yul = yul_dir.join(format!("{contract}.yul"));
        if !yul.is_file() {
            return Err(Error::MissingArtifact {
                contract,
                path: yul,
            });
        }
        let source = SourcePaths {
            implementation: config.paths.src.join(format!("{contract}.lean")),
            spec: config.paths.spec.join(format!("{contract}Spec.lean")),
            proof: config.paths.proof.join(format!("{contract}Proof.lean")),
        };
        require_contract_files(root, &contract, &source)?;
        all_contracts.push((contract, path.clone(), source));
    }

    let mut specs_by_contract: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut spec_owner: BTreeMap<String, String> = BTreeMap::new();
    for (contract, _, source) in &all_contracts {
        let names = extract_specs(&root.join(&source.spec))?;
        for name in &names {
            if let Some(prev) = spec_owner.insert(name.clone(), contract.clone()) {
                return Err(Error::Adapter(format!(
                    "spec name `{name}` is defined in both `{prev}` and `{contract}`"
                )));
            }
        }
        specs_by_contract.insert(contract.clone(), names);
    }

    let mirrors_index = match parse_foundry_test_dir(root)? {
        Some(test_dir) => extract_mirrors(root, &test_dir, &spec_owner)?,
        None => BTreeMap::new(),
    };

    let known_proof_only_keys: BTreeSet<String> = specs_by_contract
        .iter()
        .flat_map(|(contract, specs)| specs.iter().map(move |name| format!("{contract}.{name}")))
        .collect();
    for key in config.coverage.proof_only.keys() {
        if !known_proof_only_keys.contains(key.as_str()) {
            return Err(Error::Adapter(format!(
                "[coverage.proof_only] entry `{key}` does not match any known obligation"
            )));
        }
    }

    let mut manifests = Vec::new();
    for (contract, abi_path, source) in all_contracts {
        if contract_filter.is_some_and(|filter| filter != contract) {
            continue;
        }
        let proof_module = format!("proof.{contract}Proof");
        let spec_module = format!("spec.{contract}Spec");
        let specs = specs_by_contract
            .get(&contract)
            .cloned()
            .unwrap_or_default();
        let dischargers = extract_dischargers(&root.join(&source.proof), &proof_module, &specs)?;
        let obligations = merge_obligations(
            &contract,
            &spec_module,
            &specs,
            &dischargers,
            &mirrors_index,
            &config.coverage.proof_only,
        );
        let manifest = ContractManifest {
            schema: SCHEMA.to_string(),
            contract: contract.clone(),
            source,
            lean: LeanModules {
                implementation_module: format!("src.{contract}"),
                spec_module,
                proof_module,
            },
            abi: parse_abi(&abi_path)?,
            storage: parse_storage(&storage_report, &contract)?,
            obligations,
            artifacts: ArtifactPaths {
                yul: config.paths.out.join("yul").join(format!("{contract}.yul")),
                creation_bytecode: config
                    .paths
                    .out
                    .join("bytecode")
                    .join(format!("{contract}.bin")),
                runtime_bytecode: config
                    .paths
                    .out
                    .join("bytecode")
                    .join(format!("{contract}.runtime.bin")),
                bytecode_hash: None,
                solc_input: config
                    .paths
                    .out
                    .join("solc-json")
                    .join(format!("{contract}.input.json")),
                solc_output: config
                    .paths
                    .out
                    .join("solc-json")
                    .join(format!("{contract}.output.json")),
                interface: config.paths.generated.join(format!("{contract}Iface.sol")),
                deployer: config
                    .paths
                    .generated
                    .join(format!("{contract}Deployer.sol")),
            },
        };
        let out = manifest_dir.join(format!("{contract}.json"));
        manifest.write_pretty(&out)?;
        manifests.push(manifest);
    }
    if manifests.is_empty() {
        return Err(Error::Adapter("no ABI/Yul outputs found".to_string()));
    }
    Ok(manifests)
}

fn parse_foundry_test_dir(root: &Utf8Path) -> Result<Option<Utf8PathBuf>> {
    let foundry = tama_config::parse_foundry_config(root)?;
    let test_dir = root.join(&foundry.test);
    if test_dir.is_dir() {
        Ok(Some(test_dir))
    } else {
        Ok(None)
    }
}

fn require_contract_files(root: &Utf8Path, contract: &str, source: &SourcePaths) -> Result<()> {
    for path in [&source.implementation, &source.spec, &source.proof] {
        if !root.join(path).is_file() {
            return Err(Error::MissingProjectFile {
                contract: contract.to_string(),
                path: path.clone(),
            });
        }
    }
    Ok(())
}

fn contract_name_from_abi_path(path: &Utf8Path) -> Option<String> {
    let file_name = path.file_name()?;
    file_name
        .strip_suffix(".abi.json")
        .or_else(|| file_name.strip_suffix(".json"))
        .filter(|contract| !contract.is_empty())
        .map(str::to_string)
}

fn validate_contract_filter(contract: &str) -> Result<()> {
    let mut chars = contract.chars();
    let valid = matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(Error::Adapter(format!(
            "invalid contract name `{contract}` for --contract"
        )))
    }
}

pub fn compile_yul_standard_json(
    root: &Utf8Path,
    config: &TamaConfig,
    manifest: &mut ContractManifest,
) -> Result<()> {
    let contract = manifest.contract.clone();
    let yul_path = root.join(&manifest.artifacts.yul);
    let yul = tama_common::read_to_string(&yul_path)?;
    let input = solc_standard_json_input(&contract, &yul, config);
    let input_text = serde_json::to_string(&input).map_err(|err| {
        Error::Adapter(format!(
            "failed to serialize solc standard JSON input: {err}"
        ))
    })?;
    let input_pretty = serde_json::to_string_pretty(&input).map_err(|err| {
        Error::Adapter(format!(
            "failed to serialize solc standard JSON input: {err}"
        ))
    })?;
    let input_path = root.join(&manifest.artifacts.solc_input);
    tama_common::write_string(&input_path, &(input_pretty + "\n"))?;

    let solc = tama_toolchain::resolve_solc(&config.yul.solc, root)?;
    let solc_program = solc.path.to_string();
    let mut child = Command::new(solc.path.as_std_path())
        .arg("--standard-json")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Process {
            program: solc_program.clone(),
            message: source.to_string(),
        })?;
    let stdin = child.stdin.as_mut().ok_or_else(|| Error::Process {
        program: solc_program.clone(),
        message: "failed to open solc stdin".to_string(),
    })?;
    stdin
        .write_all(input_text.as_bytes())
        .map_err(|source| Error::Process {
            program: solc_program.clone(),
            message: source.to_string(),
        })?;
    let output = child.wait_with_output().map_err(|source| Error::Process {
        program: solc_program.clone(),
        message: source.to_string(),
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.trim().is_empty() && !output.status.success() {
        return Err(Error::Process {
            program: solc_program,
            message: stderr.to_string(),
        });
    }
    let output_path = root.join(&manifest.artifacts.solc_output);
    tama_common::write_string(&output_path, &stdout)?;
    let value: Value =
        serde_json::from_str(&stdout).map_err(|source| tama_manifest::Error::Json {
            path: output_path.clone(),
            source,
        })?;
    ensure_solc_success(
        output.status.success(),
        &stderr,
        &value,
        &contract,
        &solc_program,
    )?;
    let (creation, runtime) = extract_solc_bytecode(&value, &contract).ok_or_else(|| {
        Error::Adapter(format!(
            "solc output did not contain bytecode for {contract}"
        ))
    })?;
    let creation_path = root.join(&manifest.artifacts.creation_bytecode);
    let runtime_path = root.join(&manifest.artifacts.runtime_bytecode);
    tama_common::write_string(&creation_path, &(creation.clone() + "\n"))?;
    tama_common::write_string(&runtime_path, &(runtime + "\n"))?;
    manifest.artifacts.bytecode_hash = Some(tama_common::sha256_file(&creation_path)?);
    manifest.write_pretty(
        &root.join(
            config
                .paths
                .out
                .join("manifest")
                .join(format!("{}.json", manifest.contract)),
        ),
    )?;
    Ok(())
}

fn solc_standard_json_input(contract: &str, yul: &str, config: &TamaConfig) -> Value {
    json!({
        "language": "Yul",
        "sources": {
            format!("{contract}.yul"): { "content": yul }
        },
        "settings": {
            "optimizer": {
                "enabled": config.yul.optimizer,
                "runs": config.yul.optimizer_runs,
                "details": {
                    "yul": config.yul.yul_optimizer
                }
            },
            "evmVersion": config.yul.evm_version,
            "metadata": {
                "bytecodeHash": config.yul.metadata_hash
            },
            "outputSelection": {
                "*": { "*": ["evm.bytecode.object", "evm.deployedBytecode.object"] }
            }
        }
    })
}

fn ensure_solc_success(
    status_success: bool,
    stderr: &str,
    value: &Value,
    contract: &str,
    program: &str,
) -> Result<()> {
    let errors = solc_error_messages(value);
    if !errors.is_empty() {
        return Err(Error::SolcErrors {
            contract: contract.to_string(),
            errors: errors.join("\n"),
        });
    }
    if !status_success {
        return Err(Error::Process {
            program: program.to_string(),
            message: stderr.to_string(),
        });
    }
    Ok(())
}

fn solc_error_messages(value: &Value) -> Vec<String> {
    value
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("severity").and_then(Value::as_str) == Some("error"))
        .map(|entry| {
            entry
                .get("formattedMessage")
                .or_else(|| entry.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown solc error")
                .to_string()
        })
        .collect()
}

pub fn generate_bridge(root: &Utf8Path, manifest: &ContractManifest) -> Result<()> {
    let bytecode_path = root.join(&manifest.artifacts.creation_bytecode);
    if !bytecode_path.is_file() {
        return Err(Error::MissingArtifact {
            contract: manifest.contract.clone(),
            path: bytecode_path,
        });
    }
    let bytecode = tama_common::read_to_string(&bytecode_path)?
        .trim()
        .trim_start_matches("0x")
        .to_string();
    if !valid_bytecode_hex(&bytecode) {
        return Err(Error::Adapter(format!(
            "creation bytecode for {} is empty or not valid hex",
            manifest.contract
        )));
    }
    tama_common::write_generated(
        &root.join(&manifest.artifacts.interface),
        &interface_sol(manifest),
    )?;
    tama_common::write_generated(
        &root.join(&manifest.artifacts.deployer),
        &deployer_sol(manifest, &bytecode),
    )?;
    Ok(())
}

fn clear_downstream_artifacts(root: &Utf8Path, manifest: &mut ContractManifest) -> Result<()> {
    for path in [
        &manifest.artifacts.creation_bytecode,
        &manifest.artifacts.runtime_bytecode,
        &manifest.artifacts.solc_input,
        &manifest.artifacts.solc_output,
    ] {
        remove_file_if_exists(&root.join(path))?;
    }
    for path in [&manifest.artifacts.interface, &manifest.artifacts.deployer] {
        remove_generated_file_if_exists(&root.join(path))?;
    }
    manifest.artifacts.bytecode_hash = None;
    Ok(())
}

fn remove_file_if_exists(path: &Utf8Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(tama_common::io_error(path.to_owned(), source).into()),
    }
}

fn remove_generated_file_if_exists(path: &Utf8Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if tama_common::has_generated_header(path)? {
        remove_file_if_exists(path)
    } else {
        Err(tama_common::Error::GeneratedFileModified(path.to_owned()).into())
    }
}

fn interface_sol(manifest: &ContractManifest) -> String {
    let mut out = String::from("// SPDX-License-Identifier: MIT\npragma solidity ^0.8.20;\n\n");
    out.push_str(&format!("interface {}Iface {{\n", manifest.contract));
    for event in &manifest.abi.events {
        out.push_str("    event ");
        out.push_str(&event.name);
        out.push('(');
        out.push_str(
            &event
                .fields
                .iter()
                .map(|field| {
                    format!(
                        "{}{} {}",
                        field.ty,
                        if field.indexed { " indexed" } else { "" },
                        field.name
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str(");\n");
    }
    for error in &manifest.abi.errors {
        out.push_str(&format!(
            "    error {}({});\n",
            error.name,
            solidity_params(&error.inputs)
        ));
    }
    for function in &manifest.abi.functions {
        let returns = if function.outputs.is_empty() {
            String::new()
        } else {
            format!(" returns ({})", solidity_params(&function.outputs))
        };
        let mutability = match function.mutability.as_str() {
            "view" | "pure" | "payable" => format!(" {}", function.mutability),
            _ => String::new(),
        };
        out.push_str(&format!(
            "    function {}({}) external{}{};\n",
            function.name,
            solidity_params(&function.inputs),
            mutability,
            returns
        ));
    }
    out.push_str("}\n");
    out
}

fn deployer_sol(manifest: &ContractManifest, bytecode: &str) -> String {
    let (deploy_params, constructor_args) = constructor_params(manifest);
    let code_expr = if constructor_args.is_empty() {
        format!(r#"hex"{bytecode}""#)
    } else {
        format!(r#"abi.encodePacked(hex"{bytecode}", abi.encode({constructor_args}))"#)
    };
    format!(
        r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {{{contract}Iface}} from "./{contract}Iface.sol";

library {contract}Deployer {{
    function deploy({deploy_params}) internal returns ({contract}Iface deployed) {{
        bytes memory code = {code_expr};
        address addr;
        assembly {{
            addr := create(0, add(code, 0x20), mload(code))
        }}
        require(addr != address(0), "TAMA_DEPLOY_FAILED");
        deployed = {contract}Iface(addr);
    }}
}}
"#,
        contract = manifest.contract
    )
}

fn constructor_params(manifest: &ContractManifest) -> (String, String) {
    let Some(constructor) = &manifest.abi.constructor else {
        return (String::new(), String::new());
    };
    let params = constructor
        .inputs
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let name = constructor_param_name(param, index);
            format!("{} {}", param.ty, name)
        })
        .collect::<Vec<_>>();
    let args = constructor
        .inputs
        .iter()
        .enumerate()
        .map(|(index, param)| constructor_param_name(param, index))
        .collect::<Vec<_>>();
    (params.join(", "), args.join(", "))
}

fn constructor_param_name(param: &Param, index: usize) -> String {
    if param.name.is_empty() {
        format!("arg{index}")
    } else {
        param.name.clone()
    }
}

fn solidity_params(params: &[Param]) -> String {
    params
        .iter()
        .map(|param| {
            if param.name.is_empty() {
                param.ty.clone()
            } else {
                format!("{} {}", param.ty, param.name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn run_owned(program: &str, args: &[String], cwd: &Utf8Path, json_output: bool) -> Result<()> {
    tracing::debug!(
        command = %owned_command_display(program, args),
        cwd = %cwd,
        "running external command"
    );
    if json_output {
        return run_owned_json_safe(program, args, cwd);
    }
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|source| Error::Process {
            program: program.to_string(),
            message: source.to_string(),
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Process {
            program: program.to_string(),
            message: format!("exited with status {status}"),
        })
    }
}

fn run_owned_json_safe(program: &str, args: &[String], cwd: &Utf8Path) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|source| Error::Process {
            program: program.to_string(),
            message: source.to_string(),
        })?;
    let mut stderr = std::io::stderr().lock();
    stderr
        .write_all(&output.stdout)
        .and_then(|()| stderr.write_all(&output.stderr))
        .map_err(|source| Error::Process {
            program: program.to_string(),
            message: source.to_string(),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Process {
            program: program.to_string(),
            message: format!("exited with status {}", output.status),
        })
    }
}

fn run_capture(program: &str, args: &[&str], cwd: &Utf8Path) -> Result<CommandOutput> {
    tracing::debug!(
        command = %borrowed_command_display(program, args),
        cwd = %cwd,
        "capturing external command output"
    );
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|source| Error::Process {
            program: program.to_string(),
            message: source.to_string(),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        Ok(CommandOutput { stdout })
    } else {
        Err(Error::Process {
            program: program.to_string(),
            message: format!(
                "exited with status {}{}",
                output.status,
                if stderr.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", stderr.trim())
                }
            ),
        })
    }
}

fn owned_command_display(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn borrowed_command_display(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

struct EvmyulConformanceGuard {
    path: Utf8PathBuf,
    marker: Utf8PathBuf,
    created: bool,
}

impl EvmyulConformanceGuard {
    fn prepare(root: &Utf8Path) -> Result<Self> {
        let path = root.join("EthereumTests");
        let marker = path.join(".tama-evmyul-placeholder");
        if path.exists() {
            if !path.is_dir() {
                return Err(Error::Adapter(format!(
                    "`{path}` exists but is not a directory; Verity's evmyul FFI target requires an EthereumTests directory"
                )));
            }
            return Ok(Self {
                path,
                marker,
                created: false,
            });
        }
        fs::create_dir_all(&path).map_err(|source| tama_common::io_error(path.clone(), source))?;
        if let Err(err) = tama_common::write_string(
            &marker,
            "Temporary Tama marker used while building Verity's evmyul FFI target.\n",
        ) {
            let _ = fs::remove_dir(&path);
            return Err(err.into());
        }
        Ok(Self {
            path,
            marker,
            created: true,
        })
    }
}

impl Drop for EvmyulConformanceGuard {
    fn drop(&mut self) {
        if self.created {
            let _ = fs::remove_file(&self.marker);
            let _ = fs::remove_dir(&self.path);
        }
    }
}

fn discover_modules(src: &Utf8Path) -> Result<Vec<String>> {
    let mut modules = Vec::new();
    for entry in
        fs::read_dir(src).map_err(|source| tama_common::io_error(src.to_owned(), source))?
    {
        let entry = entry.map_err(|source| tama_common::io_error(src.to_owned(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|path| tama_common::Error::NonUtf8Path(path.display().to_string()))?;
        if path.extension() == Some("lean") {
            if let Some(stem) = path.file_stem() {
                modules.push(format!("src.{stem}"));
            }
        }
    }
    modules.sort();
    Ok(modules)
}

fn read_json_required(path: &Utf8Path) -> Result<Value> {
    if !path.is_file() {
        return Err(Error::Adapter(format!(
            "Verity layout report is missing: {path}"
        )));
    }
    let text = tama_common::read_to_string(path)?;
    let value = serde_json::from_str(&text).map_err(|source| tama_manifest::Error::Json {
        path: path.to_owned(),
        source,
    })?;
    Ok(value)
}

fn parse_abi(path: &Utf8Path) -> Result<Abi> {
    let text = tama_common::read_to_string(path)?;
    let entries: Vec<AbiEntry> =
        serde_json::from_str(&text).map_err(|source| tama_manifest::Error::Json {
            path: path.to_owned(),
            source,
        })?;
    let mut abi = Abi::default();
    for entry in entries {
        match entry.kind.as_str() {
            "constructor" => {
                validate_abi_params(path, "constructor input", &entry.inputs)?;
                abi.constructor = Some(Constructor {
                    inputs: entry.inputs.into_iter().map(Param::from).collect(),
                });
            }
            "function" => {
                let name = abi_entry_name(path, "function", entry.name)?;
                validate_abi_params(path, &format!("function `{name}` input"), &entry.inputs)?;
                validate_abi_params(path, &format!("function `{name}` output"), &entry.outputs)?;
                let signature = format!(
                    "{}({})",
                    name,
                    entry
                        .inputs
                        .iter()
                        .map(|param| param.ty.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                );
                let mutability = entry
                    .state_mutability
                    .unwrap_or_else(|| "nonpayable".to_string());
                abi.functions.push(Function {
                    name,
                    selector: tama_common::function_selector(&signature),
                    signature,
                    visibility: "external".to_string(),
                    mutability,
                    inputs: entry.inputs.into_iter().map(Param::from).collect(),
                    outputs: entry.outputs.into_iter().map(Param::from).collect(),
                });
            }
            "event" => {
                if entry.anonymous {
                    return Err(Error::Adapter(format!(
                        "unsupported anonymous event ABI entry in {path}"
                    )));
                }
                let name = abi_entry_name(path, "event", entry.name)?;
                validate_abi_params(path, &format!("event `{name}` field"), &entry.inputs)?;
                let signature = format!(
                    "{}({})",
                    name,
                    entry
                        .inputs
                        .iter()
                        .map(|param| param.ty.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                );
                abi.events.push(Event {
                    name,
                    topic0: tama_common::event_topic(&signature),
                    signature,
                    fields: entry
                        .inputs
                        .into_iter()
                        .map(|param| tama_manifest::EventField {
                            name: param.name,
                            ty: param.ty,
                            indexed: param.indexed,
                        })
                        .collect(),
                });
            }
            "error" => {
                let name = abi_entry_name(path, "error", entry.name)?;
                validate_abi_params(path, &format!("error `{name}` input"), &entry.inputs)?;
                let signature = format!(
                    "{}({})",
                    name,
                    entry
                        .inputs
                        .iter()
                        .map(|param| param.ty.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                );
                abi.errors.push(ErrorEntry {
                    name,
                    selector: tama_common::error_selector(&signature),
                    signature,
                    inputs: entry.inputs.into_iter().map(Param::from).collect(),
                });
            }
            other => {
                return Err(Error::Adapter(format!(
                    "unsupported ABI entry type `{other}` in {path}"
                )));
            }
        }
    }
    Ok(abi)
}

fn validate_abi_params(path: &Utf8Path, label: &str, params: &[AbiParam]) -> Result<()> {
    for (index, param) in params.iter().enumerate() {
        if param.ty.trim().is_empty() {
            return Err(Error::Adapter(format!(
                "{label} {index} type cannot be empty in {path}"
            )));
        }
        if !tama_manifest::is_supported_abi_type(&param.ty) {
            return Err(Error::Adapter(format!(
                "{label} {index} has unsupported ABI type `{}` in {path}",
                param.ty
            )));
        }
    }
    Ok(())
}

fn abi_entry_name(path: &Utf8Path, kind: &str, name: Option<String>) -> Result<String> {
    let Some(name) = name else {
        return Err(Error::Adapter(format!(
            "{kind} ABI entry in {path} is missing `name`"
        )));
    };
    if name.trim().is_empty() {
        return Err(Error::Adapter(format!(
            "{kind} ABI entry in {path} has an empty `name`"
        )));
    }
    Ok(name)
}

fn parse_storage(report: &Value, contract: &str) -> Result<Vec<StorageEntry>> {
    let Some(contracts) = report.get("contracts").and_then(Value::as_array) else {
        return Err(Error::Adapter(
            "layout report is missing `contracts[]`".to_string(),
        ));
    };
    let Some(fields) = contracts
        .iter()
        .find(|entry| entry.get("contract").and_then(Value::as_str) == Some(contract))
    else {
        return Err(Error::Adapter(format!(
            "{contract} layout report entry is missing"
        )));
    };
    let Some(fields) = fields.get("fields").and_then(Value::as_array) else {
        return Err(Error::Adapter(format!(
            "{contract} layout report entry is missing `fields[]`"
        )));
    };
    fields
        .iter()
        .map(|field| {
            let name = field
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| {
                    Error::Adapter(format!(
                        "{contract} layout report storage field is missing `name`"
                    ))
                })?
                .to_string();
            let slot = field
                .get("canonicalSlot")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    Error::Adapter(format!(
                        "{contract}.{name} layout report field is missing `canonicalSlot`"
                    ))
                })?;
            let ty_value = field.get("type").ok_or_else(|| {
                Error::Adapter(format!(
                    "{contract}.{name} layout report field is missing `type`"
                ))
            })?;
            let (ty, encoding) = storage_type(ty_value, contract, &name)?;
            Ok(StorageEntry {
                name,
                ty,
                slot: format!("0x{slot:02x}"),
                offset: 0,
                width_bytes: 32,
                encoding,
            })
        })
        .collect()
}

fn storage_type(value: &Value, contract: &str, field: &str) -> Result<(String, String)> {
    match value.get("kind").and_then(Value::as_str) {
        Some("address") => Ok(("address".to_string(), "value".to_string())),
        Some("bool") => Ok(("bool".to_string(), "value".to_string())),
        Some("uint256") => Ok(("uint256".to_string(), "value".to_string())),
        Some("mapping") => Ok((
            "mapping(address => uint256)".to_string(),
            "mapping".to_string(),
        )),
        Some(kind) => Err(Error::Adapter(format!(
            "{contract}.{field} layout report has unsupported storage type `{kind}`"
        ))),
        None => Err(Error::Adapter(format!(
            "{contract}.{field} layout report field type is missing `kind`"
        ))),
    }
}

fn extract_specs(spec_path: &Utf8Path) -> Result<Vec<String>> {
    if !spec_path.is_file() {
        return Ok(Vec::new());
    }
    let text = tama_common::read_to_string(spec_path)?;
    let stripped = strip_lean_block_comments(&text);
    let def_re = Regex::new(r"^def\s+([A-Za-z_][A-Za-z0-9_']*)").expect("valid def regex");
    let gen_spec_re =
        Regex::new(r"^#gen_spec\s+([A-Za-z_][A-Za-z0-9_']*)").expect("valid gen_spec regex");
    let mut names = Vec::new();
    for raw in stripped.lines() {
        if raw.is_empty() || raw.starts_with(|ch: char| ch.is_whitespace()) {
            continue;
        }
        if raw.starts_with("--") {
            if raw.trim_start().starts_with("-- tama:") {
                return Err(Error::Adapter(format!(
                    "{spec_path}: spec files must not carry `-- tama:` comments — coverage and discharge tags live on tests and proofs"
                )));
            }
            continue;
        }
        if let Some(captures) = def_re.captures(raw) {
            names.push(captures.get(1).unwrap().as_str().to_string());
            continue;
        }
        if let Some(captures) = gen_spec_re.captures(raw) {
            names.push(captures.get(1).unwrap().as_str().to_string());
            continue;
        }
        let allowed = ["import", "namespace", "open", "end"]
            .iter()
            .any(|kw| top_level_keyword(raw, kw));
        if !allowed {
            return Err(Error::Adapter(format!(
                "spec module {spec_path} contains forbidden top-level form: `{}`",
                raw.trim_end()
            )));
        }
    }
    Ok(names)
}

fn top_level_keyword(line: &str, keyword: &str) -> bool {
    if !line.starts_with(keyword) {
        return false;
    }
    match line.as_bytes().get(keyword.len()) {
        None => true,
        Some(byte) => !(byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'\''),
    }
}

fn extract_dischargers(
    proof_path: &Utf8Path,
    proof_module: &str,
    known_specs: &[String],
) -> Result<HashMap<String, Vec<String>>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    if !proof_path.is_file() {
        return Ok(map);
    }
    let text = tama_common::read_to_string(proof_path)?;
    let theorem_re = Regex::new(
        r"^\s*(?:@\[[^\]]*\]\s*)*(?:(?:private|protected)\s+)*(?:theorem|lemma)\s+([A-Za-z_][A-Za-z0-9_.']*)",
    )
    .expect("valid theorem regex");
    let stripped = strip_lean_block_comments(&text);
    let known: BTreeSet<&str> = known_specs.iter().map(String::as_str).collect();
    let mut pending: Vec<String> = Vec::new();
    for line in stripped.lines() {
        let trimmed = line.trim();
        if let Some(raw) = trimmed.strip_prefix("-- tama:") {
            let values = parse_key_values(raw);
            for key in values.keys() {
                if key.as_str() != "discharges" {
                    return Err(Error::Adapter(format!(
                        "{proof_path}: unsupported Tama metadata key `{key}` (proof tags accept only `discharges=`)"
                    )));
                }
            }
            if let Some(value) = values.get("discharges") {
                for spec in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    if !known.contains(spec) {
                        return Err(Error::Adapter(format!(
                            "{proof_path}: discharges=`{spec}` does not match any spec in this contract"
                        )));
                    }
                    pending.push(spec.to_string());
                }
            }
            continue;
        }
        if let Some(captures) = theorem_re.captures(trimmed) {
            if !pending.is_empty() {
                let name = captures.get(1).unwrap().as_str();
                let decl = format!("{proof_module}.{name}");
                for spec in pending.drain(..) {
                    map.entry(spec).or_default().push(decl.clone());
                }
            }
            continue;
        }
        if !trimmed.is_empty() && !trimmed.starts_with("--") && !trimmed.starts_with("@[") {
            pending.clear();
        }
    }
    Ok(map)
}

fn extract_mirrors(
    root: &Utf8Path,
    test_dir: &Utf8Path,
    spec_owner: &BTreeMap<String, String>,
) -> Result<BTreeMap<(String, String), Vec<String>>> {
    let mut out: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let function_re =
        Regex::new(r"^\s*function\s+(testFuzz[A-Za-z0-9_]*|invariant_[A-Za-z0-9_]*)\s*\(")
            .expect("valid function regex");
    let any_function_re = Regex::new(r"^\s*function\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(")
        .expect("valid any function regex");
    let contract_re = Regex::new(r"^\s*(?:abstract\s+)?contract\s+([A-Za-z_][A-Za-z0-9_]*)\b")
        .expect("valid contract regex");
    walk_test_files(test_dir, &mut |path| {
        let text = tama_common::read_to_string(path)?;
        let rel = path
            .strip_prefix(root)
            .map(Utf8Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf());
        let mut current_contract: Option<String> = None;
        let mut pending: Vec<String> = Vec::new();
        let mut brace_depth: i32 = 0;
        let mut in_block_comment = false;
        for raw in text.lines() {
            if in_block_comment {
                if let Some(idx) = raw.find("*/") {
                    let rest = &raw[idx + 2..];
                    in_block_comment = false;
                    if rest.trim().is_empty() {
                        continue;
                    }
                    // Treat the post-`*/` remainder as a fresh logical line; fall through.
                    let trimmed = rest.trim();
                    if trimmed.starts_with("/*") {
                        in_block_comment = true;
                        continue;
                    }
                    brace_depth += count_brace_delta(rest);
                    continue;
                } else {
                    continue;
                }
            }
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(tag) = trimmed.strip_prefix("// tama:") {
                if brace_depth > 1 {
                    continue;
                }
                let values = parse_key_values(tag);
                for key in values.keys() {
                    if key.as_str() != "mirrors" {
                        return Err(Error::Adapter(format!(
                            "{rel}: unsupported Tama metadata key `{key}` (mirror tags accept only `mirrors=`)"
                        )));
                    }
                }
                if let Some(value) = values.get("mirrors") {
                    for spec in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        pending.push(spec.to_string());
                    }
                }
                continue;
            }
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            if let Some(after) = trimmed.strip_prefix("/*") {
                if !after.contains("*/") {
                    in_block_comment = true;
                }
                continue;
            }
            let mut consumed = false;
            if brace_depth <= 1 {
                if let Some(captures) = contract_re.captures(raw) {
                    current_contract = Some(captures.get(1).unwrap().as_str().to_string());
                    pending.clear();
                    consumed = true;
                } else if let Some(captures) = function_re.captures(raw) {
                    if !pending.is_empty() {
                        let func = captures.get(1).unwrap().as_str();
                        let Some(sol_contract) = current_contract.as_deref() else {
                            return Err(Error::Adapter(format!(
                                "{rel}: mirror tag above `{func}` is outside any `contract` block"
                            )));
                        };
                        let mirror_path = format!("{rel}:{sol_contract}.{func}");
                        for spec in pending.drain(..) {
                            let Some(owner) = spec_owner.get(&spec) else {
                                return Err(Error::Adapter(format!(
                                    "{rel}: mirrors=`{spec}` does not match any known spec"
                                )));
                            };
                            out.entry((owner.clone(), spec))
                                .or_default()
                                .push(mirror_path.clone());
                        }
                    }
                    consumed = true;
                } else if let Some(captures) = any_function_re.captures(raw) {
                    if !pending.is_empty() {
                        let func = captures.get(1).unwrap().as_str();
                        return Err(Error::Adapter(format!(
                            "{rel}: mirror tag on non-property-shaped test `{func}` (must be testFuzz* or invariant_*)"
                        )));
                    }
                    consumed = true;
                }
            }
            if !consumed {
                pending.clear();
            }
            brace_depth += count_brace_delta(raw);
        }
        Ok(())
    })?;
    Ok(out)
}

fn count_brace_delta(line: &str) -> i32 {
    let mut delta = 0i32;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch == b'/' && bytes.get(i + 1) == Some(&b'/') {
            return delta;
        }
        if ch == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = i.saturating_add(2).min(bytes.len());
            continue;
        }
        if ch == b'"' || ch == b'\'' {
            let quote = ch;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        if ch == b'{' {
            delta += 1;
        } else if ch == b'}' {
            delta -= 1;
        }
        i += 1;
    }
    delta
}

fn walk_test_files<F>(dir: &Utf8Path, visit: &mut F) -> Result<()>
where
    F: FnMut(&Utf8Path) -> Result<()>,
{
    let entries =
        fs::read_dir(dir).map_err(|source| tama_common::io_error(dir.to_owned(), source))?;
    let mut paths: Vec<Utf8PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| tama_common::io_error(dir.to_owned(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|p| tama_common::Error::NonUtf8Path(p.display().to_string()))?;
        paths.push(path);
    }
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk_test_files(&path, visit)?;
            continue;
        }
        if path
            .file_name()
            .map(|name| name.ends_with(".t.sol"))
            .unwrap_or(false)
        {
            visit(&path)?;
        }
    }
    Ok(())
}

fn merge_obligations(
    contract: &str,
    spec_module: &str,
    specs: &[String],
    dischargers: &HashMap<String, Vec<String>>,
    mirrors: &BTreeMap<(String, String), Vec<String>>,
    proof_only: &BTreeMap<String, String>,
) -> Vec<Obligation> {
    let mut out = Vec::with_capacity(specs.len());
    for name in specs {
        let id = format!("{contract}.{name}");
        let dischargers_for_spec = dischargers.get(name).cloned().unwrap_or_default();
        let mirrors_for_spec = mirrors
            .get(&(contract.to_string(), name.clone()))
            .cloned()
            .unwrap_or_default();
        let proof_only_reason = proof_only.get(&id).cloned();
        out.push(Obligation {
            id,
            name: name.clone(),
            lean_decl: format!("{spec_module}.{name}"),
            contract: contract.to_string(),
            dischargers: dischargers_for_spec,
            mirrors: mirrors_for_spec,
            proof_only_reason,
        });
    }
    out
}

fn parse_key_values(raw: &str) -> std::collections::BTreeMap<String, String> {
    let mut values = std::collections::BTreeMap::new();
    let mut chars = raw.trim().chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
            chars.next();
        }
        let mut key = String::new();
        while chars
            .peek()
            .is_some_and(|ch| !ch.is_whitespace() && *ch != '=')
        {
            let Some(ch) = chars.next() else {
                break;
            };
            key.push(ch);
        }
        if key.is_empty() {
            break;
        }
        while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
            chars.next();
        }
        if chars.peek() != Some(&'=') {
            values.insert(key, "true".to_string());
            continue;
        }
        chars.next();
        while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
            chars.next();
        }
        let value = if chars.peek() == Some(&'"') {
            chars.next();
            let mut value = String::new();
            for ch in chars.by_ref() {
                if ch == '"' {
                    break;
                }
                value.push(ch);
            }
            value
        } else {
            let mut value = String::new();
            while chars.peek().is_some_and(|ch| !ch.is_whitespace()) {
                let Some(ch) = chars.next() else {
                    break;
                };
                value.push(ch);
            }
            value
        };
        values.insert(key, value);
    }
    values
}

fn strip_lean_block_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut depth = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '/' && chars.peek() == Some(&'-') {
            chars.next();
            depth += 1;
            continue;
        }
        if depth > 0 {
            if ch == '-' && chars.peek() == Some(&'/') {
                chars.next();
                depth -= 1;
            } else if ch == '\n' {
                out.push('\n');
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn generate_trust_probe(
    root: &Utf8Path,
    config: &TamaConfig,
    manifests: &[ContractManifest],
) -> Result<()> {
    let obligations = manifests
        .iter()
        .flat_map(|manifest| manifest.obligations.iter())
        .collect::<Vec<_>>();
    let probe_dir = root.join(config.paths.out.join("trust-probe"));
    fs::create_dir_all(&probe_dir)
        .map_err(|source| tama_common::io_error(probe_dir.clone(), source))?;
    let legacy_source = probe_dir.join("PrintAxioms.lean");
    match fs::remove_file(&legacy_source) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(tama_common::io_error(legacy_source, source).into()),
    }
    let output_path = probe_dir.join("axioms.json");
    if obligations.is_empty() {
        let report = json!({
            "schema": "tama.trust-probe.v1",
            "method": "lean.collectAxioms",
            "obligations": []
        });
        let report_text = serde_json::to_string_pretty(&report)
            .map_err(|err| Error::Adapter(format!("failed to serialize trust report: {err}")))?;
        tama_common::write_string(&output_path, &(report_text + "\n"))?;
        return Ok(());
    }

    let source_path = probe_dir.join("CollectAxioms.lean");
    let source = collect_axioms_probe_source(manifests, &obligations)?;
    tama_common::write_string(&source_path, &source)?;
    let output = run_capture("lake", &["env", "lean", source_path.as_str()], root)?;
    let report = parse_collect_axioms_output(&output.stdout, &obligations)?;
    let report_text = serde_json::to_string_pretty(&report)
        .map_err(|err| Error::Adapter(format!("failed to serialize trust report: {err}")))?;
    tama_common::write_string(&output_path, &(report_text + "\n"))?;
    Ok(())
}

fn collect_axioms_probe_source(
    manifests: &[ContractManifest],
    obligations: &[&Obligation],
) -> Result<String> {
    let mut modules = manifests
        .iter()
        .map(|manifest| manifest.lean.proof_module.as_str())
        .collect::<Vec<_>>();
    modules.sort_unstable();
    modules.dedup();
    let mut out =
        String::from("-- Generated by Tama. Trust probe using Lean.collectAxioms.\nimport Lean\n");
    for module in modules {
        validate_lean_name(module)?;
        out.push_str(&format!("import {module}\n"));
    }
    out.push_str(
        r#"
open Lean

def tamaAxiomJson (decl : String) (constName : Name) : CoreM Json := do
  let env ← getEnv
  if (env.find? constName).isNone then
    throwError m!"missing trust probe declaration {constName}"
  let axioms ← collectAxioms constName
  let sorted := axioms.map toString |>.qsort (fun a b => a < b)
  pure <| Json.mkObj [
    ("lean_decl", Json.str decl),
    ("axioms", Json.arr (sorted.map Json.str))
  ]

#eval show CoreM Unit from do
"#,
    );
    for (oi, obligation) in obligations.iter().enumerate() {
        for (di, discharger) in obligation.dischargers.iter().enumerate() {
            let name = lean_name_literal(discharger)?;
            out.push_str(&format!(
                "  let d_{oi}_{di} ← tamaAxiomJson \"{discharger}\" {name}\n"
            ));
        }
        out.push_str(&format!("  let dischargers_{oi} := #["));
        for di in 0..obligation.dischargers.len() {
            if di > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("d_{oi}_{di}"));
        }
        out.push_str("]\n");
        out.push_str(&format!(
            "  let obligation_{oi} := Json.mkObj [\n    (\"lean_decl\", Json.str \"{}\"),\n    (\"dischargers\", Json.arr dischargers_{oi})\n  ]\n",
            obligation.lean_decl
        ));
    }
    out.push_str("  let obligations := #[");
    for oi in 0..obligations.len() {
        if oi > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("obligation_{oi}"));
    }
    out.push_str(
        r#"]
  let report := Json.mkObj [
    ("schema", Json.str "tama.trust-probe.v1"),
    ("method", Json.str "lean.collectAxioms"),
    ("obligations", Json.arr obligations)
  ]
  IO.println report.compress
"#,
    );
    Ok(out)
}

fn parse_collect_axioms_output(output: &str, obligations: &[&Obligation]) -> Result<Value> {
    let json_line = output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| Error::Adapter("trust probe emitted no JSON".to_string()))?;
    let value: Value =
        serde_json::from_str(json_line).map_err(|err| Error::Adapter(err.to_string()))?;
    if value.get("schema").and_then(Value::as_str) != Some("tama.trust-probe.v1") {
        return Err(Error::Adapter(
            "trust probe emitted unsupported schema".to_string(),
        ));
    }
    if value.get("method").and_then(Value::as_str) != Some("lean.collectAxioms") {
        return Err(Error::Adapter(
            "trust probe did not use Lean.collectAxioms".to_string(),
        ));
    }
    let Some(reported_obligations) = value.get("obligations").and_then(Value::as_array) else {
        return Err(Error::Adapter(
            "trust probe emitted no obligations array".to_string(),
        ));
    };
    let mut reported_dischargers = BTreeSet::<&str>::new();
    for entry in reported_obligations {
        let Some(dischargers) = entry.get("dischargers").and_then(Value::as_array) else {
            return Err(Error::Adapter(
                "trust probe obligation entry missing `dischargers`".to_string(),
            ));
        };
        for d in dischargers {
            if let Some(decl) = d.get("lean_decl").and_then(Value::as_str) {
                reported_dischargers.insert(decl);
            }
        }
    }
    for obligation in obligations {
        for discharger in &obligation.dischargers {
            if !reported_dischargers.contains(discharger.as_str()) {
                return Err(Error::Adapter(format!(
                    "trust probe did not report axioms for {discharger}"
                )));
            }
        }
    }
    Ok(value)
}

fn lean_name_literal(name: &str) -> Result<String> {
    validate_lean_name(name)?;
    Ok(format!("`{name}"))
}

fn validate_lean_name(name: &str) -> Result<()> {
    if tama_manifest::is_qualified_lean_name(name) {
        return Ok(());
    }
    Err(Error::Adapter(format!(
        "trust probe Lean name `{name}` is not a supported identifier"
    )))
}

#[cfg(test)]
fn parse_print_axioms_output(output: &str, obligations: &[&Obligation]) -> Result<Value> {
    let expected: BTreeSet<&str> = obligations
        .iter()
        .flat_map(|o| o.dischargers.iter().map(String::as_str))
        .collect();
    let mut current = None::<String>;
    let mut reported = BTreeMap::<String, Vec<String>>::new();
    for line in output.lines() {
        if let Some(decl) = line.split("TAMA_AXIOMS_BEGIN ").nth(1) {
            current = Some(decl.trim().to_string());
            continue;
        }
        if line.contains("TAMA_AXIOMS_END ") {
            current = None;
            continue;
        }
        let Some(decl) = current.as_ref() else {
            continue;
        };
        if line.contains("does not depend on any axioms") {
            reported.entry(decl.clone()).or_default();
        } else if let Some((_, tail)) = line.split_once("depends on axioms:") {
            reported.insert(decl.clone(), parse_axiom_list(tail));
        }
    }
    for decl in expected {
        if !reported.contains_key(decl) {
            return Err(Error::Adapter(format!(
                "trust probe did not report axioms for {decl}"
            )));
        }
    }
    Ok(json!({
        "schema": "tama.trust-probe.v1.fallback",
        "method": "lean.printAxioms",
        "obligations": obligations
            .iter()
            .map(|obligation| {
                json!({
                    "lean_decl": obligation.lean_decl,
                    "dischargers": obligation
                        .dischargers
                        .iter()
                        .map(|decl| json!({
                            "lean_decl": decl,
                            "axioms": reported.get(decl).cloned().unwrap_or_default()
                        }))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>()
    }))
}

#[cfg(test)]
fn parse_axiom_list(raw: &str) -> Vec<String> {
    let list = raw
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']').map(|(items, _)| items))
        .unwrap_or(raw);
    list.split(',')
        .map(|item| item.trim().trim_matches('`').trim_matches('\'').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn extract_solc_bytecode(value: &Value, contract: &str) -> Option<(String, String)> {
    let contracts = value.get("contracts")?.as_object()?;
    for by_file in contracts.values() {
        let Some(contract_value) = by_file
            .as_object()
            .and_then(|contracts| contracts.get(contract))
        else {
            continue;
        };
        let evm = contract_value.get("evm")?;
        let creation = evm.get("bytecode")?.get("object")?.as_str()?.to_string();
        let runtime = evm
            .get("deployedBytecode")
            .and_then(|bytecode| bytecode.get("object"))
            .and_then(Value::as_str)?
            .to_string();
        if !valid_bytecode_hex(&creation) || !valid_bytecode_hex(&runtime) {
            return None;
        }
        return Some((creation, runtime));
    }
    None
}

fn valid_bytecode_hex(value: &str) -> bool {
    !value.is_empty() && value.len() % 2 == 0 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

#[derive(Debug, Deserialize)]
struct AbiEntry {
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
    #[serde(default)]
    inputs: Vec<AbiParam>,
    #[serde(default)]
    outputs: Vec<AbiParam>,
    #[serde(rename = "stateMutability")]
    state_mutability: Option<String>,
    #[serde(default)]
    anonymous: bool,
}

#[derive(Debug, Deserialize)]
struct AbiParam {
    #[serde(default)]
    name: String,
    #[serde(rename = "type")]
    ty: String,
    #[serde(default)]
    indexed: bool,
}

impl From<AbiParam> for Param {
    fn from(value: AbiParam) -> Self {
        Self {
            name: value.name,
            ty: value.ty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

    struct EnvVarGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old }
        }

        fn unset(key: &'static str) -> Self {
            let old = std::env::var_os(key);
            std::env::remove_var(key);
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
    fn canned_solc_error_is_detected() {
        let value = json!({
            "errors": [{"severity": "error", "message": "bad yul"}]
        });
        assert_eq!(solc_error_messages(&value), vec!["bad yul"]);
    }

    #[test]
    fn solc_nonzero_status_fails_even_without_json_errors() {
        let value = json!({
            "errors": [{"severity": "warning", "message": "warning only"}]
        });

        let err =
            ensure_solc_success(false, "solc crashed", &value, "Counter", "solc").unwrap_err();

        assert!(matches!(
            err,
            Error::Process { program, message } if program == "solc" && message == "solc crashed"
        ));
    }

    #[test]
    fn solc_bytecode_requires_creation_and_runtime_objects() {
        let valid = json!({
            "contracts": {
                "Counter.yul": {
                    "Counter": {
                        "evm": {
                            "bytecode": {"object": "6000"},
                            "deployedBytecode": {"object": "6001"}
                        }
                    }
                }
            }
        });
        assert_eq!(
            extract_solc_bytecode(&valid, "Counter"),
            Some(("6000".to_string(), "6001".to_string()))
        );

        let missing_runtime = json!({
            "contracts": {
                "Counter.yul": {
                    "Counter": {
                        "evm": {
                            "bytecode": {"object": "6000"}
                        }
                    }
                }
            }
        });
        assert_eq!(extract_solc_bytecode(&missing_runtime, "Counter"), None);

        let malformed_creation = json!({
            "contracts": {
                "Counter.yul": {
                    "Counter": {
                        "evm": {
                            "bytecode": {"object": "0"},
                            "deployedBytecode": {"object": "6001"}
                        }
                    }
                }
            }
        });
        assert_eq!(extract_solc_bytecode(&malformed_creation, "Counter"), None);
    }

    #[test]
    fn build_progress_line_is_scan_friendly() {
        assert_eq!(
            build_progress_line(
                "run",
                "proof-check",
                "Lean elaborates implementations, specs, and proofs",
            ),
            "  run  proof-check     Lean elaborates implementations, specs, and proofs"
        );
        assert_eq!(
            build_progress_line("skip", "forge", "skipped by --no-forge"),
            "  skip forge           skipped by --no-forge"
        );
    }

    #[test]
    fn solc_bytecode_selects_expected_contract() {
        let value = json!({
            "contracts": {
                "Other.yul": {
                    "Other": {
                        "evm": {
                            "bytecode": {"object": "6000"},
                            "deployedBytecode": {"object": "6001"}
                        }
                    }
                },
                "Counter.yul": {
                    "Counter": {
                        "evm": {
                            "bytecode": {"object": "6002"},
                            "deployedBytecode": {"object": "6003"}
                        }
                    }
                }
            }
        });

        assert_eq!(
            extract_solc_bytecode(&value, "Counter"),
            Some(("6002".to_string(), "6003".to_string()))
        );
        assert_eq!(extract_solc_bytecode(&value, "Missing"), None);
    }

    #[test]
    fn bridge_generation_contains_header_sensitive_body() {
        let manifest = ContractManifest {
            schema: SCHEMA.to_string(),
            contract: "Counter".to_string(),
            source: SourcePaths {
                implementation: "verity/src/Counter.lean".into(),
                spec: "verity/spec/CounterSpec.lean".into(),
                proof: "verity/proof/CounterProof.lean".into(),
            },
            lean: LeanModules {
                implementation_module: "src.Counter".to_string(),
                spec_module: "spec.CounterSpec".to_string(),
                proof_module: "proof.CounterProof".to_string(),
            },
            abi: Abi {
                constructor: None,
                functions: vec![Function {
                    name: "getCount".to_string(),
                    signature: "getCount()".to_string(),
                    selector: tama_common::function_selector("getCount()"),
                    visibility: "external".to_string(),
                    mutability: "view".to_string(),
                    inputs: vec![],
                    outputs: vec![Param {
                        name: "".to_string(),
                        ty: "uint256".to_string(),
                    }],
                }],
                events: vec![],
                errors: vec![],
            },
            storage: vec![],
            obligations: vec![],
            artifacts: ArtifactPaths {
                yul: "artifacts/yul/Counter.yul".into(),
                creation_bytecode: "artifacts/bytecode/Counter.bin".into(),
                runtime_bytecode: "artifacts/bytecode/Counter.runtime.bin".into(),
                bytecode_hash: None,
                solc_input: "artifacts/solc-json/Counter.input.json".into(),
                solc_output: "artifacts/solc-json/Counter.output.json".into(),
                interface: "src/generated/verity/CounterIface.sol".into(),
                deployer: "src/generated/verity/CounterDeployer.sol".into(),
            },
        };
        assert!(interface_sol(&manifest)
            .contains("function getCount() external view returns (uint256);"));
    }

    #[test]
    fn bridge_generation_requires_valid_creation_bytecode() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let manifest = test_manifest("Counter");

        let err = generate_bridge(&root, &manifest).unwrap_err();
        assert!(matches!(err, Error::MissingArtifact { .. }));

        tama_common::write_string(&root.join(&manifest.artifacts.creation_bytecode), "0x0\n")
            .unwrap();
        let err = generate_bridge(&root, &manifest).unwrap_err();
        assert!(matches!(err, Error::Adapter(message) if message.contains("valid hex")));

        tama_common::write_string(
            &root.join(&manifest.artifacts.creation_bytecode),
            "0x6000\n",
        )
        .unwrap();
        generate_bridge(&root, &manifest).unwrap();
        assert!(
            tama_common::read_to_string(&root.join(&manifest.artifacts.deployer))
                .unwrap()
                .contains(r#"hex"6000""#)
        );
    }

    #[test]
    fn no_solc_implies_no_forge() {
        assert!(!should_run_forge(&BuildOptions {
            no_solc: true,
            no_forge: false,
            ..Default::default()
        }));
        assert!(should_run_forge(&BuildOptions::default()));
    }

    #[test]
    fn missing_lock_initializes_empty_build_lock() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let lock = load_or_initialize_lock(&root).unwrap();

        assert_eq!(lock, empty_lock());
    }

    #[test]
    fn corrupt_lock_fails_instead_of_resetting_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(&root.join("tama.lock"), "not = [valid").unwrap();

        let err = load_or_initialize_lock(&root).unwrap_err();

        assert!(matches!(
            err,
            Error::Config(tama_config::Error::Toml { .. })
        ));
    }

    #[test]
    fn offline_build_args_preserve_lake_cache_and_gate_forge_network() {
        assert_eq!(
            lake_build_args(&["TamaSrc", "TamaSpec"]),
            vec!["build", "TamaSrc", "TamaSpec"]
        );
        assert_eq!(lake_build_args(&["TamaProof"]), vec!["build", "TamaProof"]);
        assert_eq!(forge_build_args(true), vec!["build", "--offline"]);
        assert_eq!(forge_build_args(false), vec!["build"]);
    }

    #[test]
    fn json_safe_process_mode_keeps_stdout_available_for_tama_json() {
        let _guard = ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let args = vec![
            "-c".to_string(),
            "printf subprocess-stdout; printf subprocess-stderr >&2".to_string(),
        ];

        run_owned("sh", &args, &root, true).unwrap();
    }

    #[test]
    fn evmyul_guard_creates_and_removes_placeholder_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let path = root.join("EthereumTests");
        let marker = path.join(".tama-evmyul-placeholder");

        {
            let _guard = EvmyulConformanceGuard::prepare(&root).unwrap();
            assert!(path.is_dir());
            assert!(marker.is_file());
        }

        assert!(!path.exists());
    }

    #[test]
    fn evmyul_guard_rejects_file_placeholder_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let path = root.join("EthereumTests");
        tama_common::write_string(&path, "not a directory\n").unwrap();

        let err = match EvmyulConformanceGuard::prepare(&root) {
            Ok(_) => panic!("expected EthereumTests file to be rejected"),
            Err(err) => err,
        };

        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("not a directory")
        ));
        assert!(path.is_file());
    }

    #[test]
    fn contract_filter_rejects_invalid_names() {
        assert!(validate_contract_filter("ERC20Lite").is_ok());
        let err = validate_contract_filter("../ERC20Lite").unwrap_err();
        assert!(
            matches!(err, Error::Adapter(message) if message.contains("invalid contract name"))
        );
        let err = validate_contract_filter("erc20Lite").unwrap_err();
        assert!(
            matches!(err, Error::Adapter(message) if message.contains("invalid contract name"))
        );
    }

    #[test]
    fn downstream_artifact_cleanup_removes_stale_generated_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let mut manifest = test_manifest("Counter");
        manifest.artifacts.bytecode_hash = Some("old".to_string());

        tama_common::write_string(&root.join(&manifest.artifacts.yul), "{ }\n").unwrap();
        tama_common::write_string(&root.join(&manifest.artifacts.creation_bytecode), "old\n")
            .unwrap();
        tama_common::write_string(&root.join(&manifest.artifacts.runtime_bytecode), "old\n")
            .unwrap();
        tama_common::write_string(&root.join(&manifest.artifacts.solc_input), "{}\n").unwrap();
        tama_common::write_string(&root.join(&manifest.artifacts.solc_output), "{}\n").unwrap();
        tama_common::write_generated(
            &root.join(&manifest.artifacts.interface),
            "interface Old {}\n",
        )
        .unwrap();
        tama_common::write_generated(&root.join(&manifest.artifacts.deployer), "library Old {}\n")
            .unwrap();

        clear_downstream_artifacts(&root, &mut manifest).unwrap();
        assert!(root.join(&manifest.artifacts.yul).is_file());
        assert!(!root.join(&manifest.artifacts.creation_bytecode).exists());
        assert!(!root.join(&manifest.artifacts.runtime_bytecode).exists());
        assert!(!root.join(&manifest.artifacts.solc_input).exists());
        assert!(!root.join(&manifest.artifacts.solc_output).exists());
        assert!(!root.join(&manifest.artifacts.interface).exists());
        assert!(!root.join(&manifest.artifacts.deployer).exists());
        assert_eq!(manifest.artifacts.bytecode_hash, None);
    }

    #[test]
    fn downstream_artifact_cleanup_refuses_hand_edited_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let mut manifest = test_manifest("Counter");
        tama_common::write_string(
            &root.join(&manifest.artifacts.interface),
            "interface HandEdited {}\n",
        )
        .unwrap();

        let err = clear_downstream_artifacts(&root, &mut manifest).unwrap_err();
        assert!(matches!(
            err,
            Error::Common(tama_common::Error::GeneratedFileModified(_))
        ));
        assert!(root.join(&manifest.artifacts.interface).is_file());
    }

    #[test]
    fn verity_codegen_cleanup_removes_stale_generated_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        for path in [
            "artifacts/abi/Counter.abi.json",
            "artifacts/abi/Counter.storage.json",
            "artifacts/abi/Other.abi.json",
            "artifacts/yul/Counter.yul",
            "artifacts/yul/Other.yul",
            "artifacts/manifest/Counter.json",
            "artifacts/manifest/Other.json",
            "artifacts/layout-report.json",
            "artifacts/trust-report.json",
            "artifacts/assumption-report.json",
            "artifacts/abi/README.txt",
        ] {
            tama_common::write_string(&root.join(path), "stale\n").unwrap();
        }

        clear_verity_codegen_outputs(&root, &config, Some("Counter")).unwrap();

        assert!(!root.join("artifacts/abi/Counter.abi.json").exists());
        assert!(!root.join("artifacts/abi/Counter.storage.json").exists());
        assert!(!root.join("artifacts/yul/Counter.yul").exists());
        assert!(!root.join("artifacts/manifest/Counter.json").exists());
        assert!(!root.join("artifacts/layout-report.json").exists());
        assert!(!root.join("artifacts/trust-report.json").exists());
        assert!(!root.join("artifacts/assumption-report.json").exists());
        assert!(root.join("artifacts/abi/Other.abi.json").is_file());
        assert!(root.join("artifacts/yul/Other.yul").is_file());
        assert!(root.join("artifacts/manifest/Other.json").is_file());
        assert!(root.join("artifacts/abi/README.txt").is_file());

        clear_verity_codegen_outputs(&root, &config, None).unwrap();

        assert!(!root.join("artifacts/abi/Other.abi.json").exists());
        assert!(!root.join("artifacts/yul/Other.yul").exists());
        assert!(!root.join("artifacts/manifest/Other.json").exists());
        assert!(root.join("artifacts/abi/README.txt").is_file());
    }

    #[test]
    fn extract_specs_enumerates_top_level_decls() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let spec = root.join("verity/spec/CounterSpec.lean");
        tama_common::write_string(
            &spec,
            r#"import src.Counter

namespace spec.CounterSpec

open Verity

#gen_spec increment_spec (0, foo)

def getCount_spec (s : ContractState) : Prop :=
  s.storage 0 = 0

def getCount_preserves_state_spec (s s' : ContractState) : Prop :=
  s' = s

end spec.CounterSpec
"#,
        )
        .unwrap();
        let names = extract_specs(&spec).unwrap();
        assert_eq!(
            names,
            vec![
                "increment_spec".to_string(),
                "getCount_spec".to_string(),
                "getCount_preserves_state_spec".to_string()
            ]
        );
    }

    #[test]
    fn extract_specs_preserves_lean_prime_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let spec = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .unwrap()
            .join("Spec.lean");
        tama_common::write_string(
            &spec,
            r#"namespace spec.Foo

def transfer' (s : Nat) : Prop := s = 0

end spec.Foo
"#,
        )
        .unwrap();
        let names = extract_specs(&spec).unwrap();
        assert_eq!(names, vec!["transfer'".to_string()]);
    }

    #[test]
    fn extract_dischargers_preserves_lean_prime_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let proof = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .unwrap()
            .join("Proof.lean");
        tama_common::write_string(
            &proof,
            r#"namespace proof.Foo

-- tama: discharges=transfer'
theorem transfer'_meets : True := by trivial

end proof.Foo
"#,
        )
        .unwrap();
        let map = extract_dischargers(&proof, "proof.Foo", &["transfer'".to_string()]).unwrap();
        assert_eq!(
            map.get("transfer'").map(|v| v.as_slice()),
            Some(&["proof.Foo.transfer'_meets".to_string()][..])
        );
    }

    #[test]
    fn extract_specs_rejects_forbidden_top_level_form() {
        let dir = tempfile::tempdir().unwrap();
        let spec = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .unwrap()
            .join("Spec.lean");
        tama_common::write_string(
            &spec,
            r#"namespace spec.Foo

def foo_spec (s : Nat) : Prop := s = 0

theorem helper : True := by trivial

end spec.Foo
"#,
        )
        .unwrap();
        let err = extract_specs(&spec).unwrap_err();
        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("forbidden top-level form")
        ));
    }

    #[test]
    fn extract_specs_rejects_tama_comments() {
        let dir = tempfile::tempdir().unwrap();
        let spec = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .unwrap()
            .join("Spec.lean");
        tama_common::write_string(
            &spec,
            r#"namespace spec.Foo

-- tama: coverage=mirror
def foo_spec (s : Nat) : Prop := s = 0

end spec.Foo
"#,
        )
        .unwrap();
        let err = extract_specs(&spec).unwrap_err();
        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("must not carry `-- tama:` comments")
        ));
    }

    #[test]
    fn extract_dischargers_parses_discharges_tag() {
        let dir = tempfile::tempdir().unwrap();
        let proof = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .unwrap()
            .join("Proof.lean");
        tama_common::write_string(
            &proof,
            r#"namespace proof.CounterProof

-- tama: discharges=increment_spec
theorem increment_meets_spec : True := by trivial

-- tama: discharges=getCount_spec,getCount_preserves_state_spec
theorem getCount_combo : True := by trivial

lemma helper : True := by trivial

end proof.CounterProof
"#,
        )
        .unwrap();
        let specs = vec![
            "increment_spec".to_string(),
            "getCount_spec".to_string(),
            "getCount_preserves_state_spec".to_string(),
        ];
        let map = extract_dischargers(&proof, "proof.CounterProof", &specs).unwrap();
        assert_eq!(
            map.get("increment_spec").unwrap(),
            &vec!["proof.CounterProof.increment_meets_spec".to_string()]
        );
        assert_eq!(
            map.get("getCount_spec").unwrap(),
            &vec!["proof.CounterProof.getCount_combo".to_string()]
        );
        assert_eq!(
            map.get("getCount_preserves_state_spec").unwrap(),
            &vec!["proof.CounterProof.getCount_combo".to_string()]
        );
    }

    #[test]
    fn extract_dischargers_rejects_unknown_spec() {
        let dir = tempfile::tempdir().unwrap();
        let proof = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .unwrap()
            .join("Proof.lean");
        tama_common::write_string(
            &proof,
            r#"namespace proof.Foo

-- tama: discharges=does_not_exist
theorem t : True := by trivial

end proof.Foo
"#,
        )
        .unwrap();
        let err = extract_dischargers(&proof, "proof.Foo", &["foo_spec".to_string()]).unwrap_err();
        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("does not match any spec")
        ));
    }

    #[test]
    fn extract_dischargers_rejects_unknown_key() {
        let dir = tempfile::tempdir().unwrap();
        let proof = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .unwrap()
            .join("Proof.lean");
        tama_common::write_string(
            &proof,
            r#"namespace proof.Foo

-- tama: discharges=foo_spec coverage=mirror
theorem t : True := by trivial

end proof.Foo
"#,
        )
        .unwrap();
        let err = extract_dischargers(&proof, "proof.Foo", &["foo_spec".to_string()]).unwrap_err();
        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("unsupported Tama metadata key `coverage`")
        ));
    }

    #[test]
    fn extract_mirrors_reads_solidity_tags() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let test_dir = root.join("test/verity");
        let test_file = test_dir.join("Counter.t.sol");
        tama_common::write_string(
            &test_file,
            r#"// SPDX-License-Identifier: MIT
contract CounterTest {
    // tama: mirrors=increment_spec
    function testFuzzIncrement(uint256 x) public {}

    // tama: mirrors=getCount_spec,getCount_preserves_state_spec
    function testFuzzGetterMirror(uint256 x) public {}

    function testNotTracked() public {}
}
"#,
        )
        .unwrap();
        let mut spec_owner = BTreeMap::new();
        spec_owner.insert("increment_spec".to_string(), "Counter".to_string());
        spec_owner.insert("getCount_spec".to_string(), "Counter".to_string());
        spec_owner.insert(
            "getCount_preserves_state_spec".to_string(),
            "Counter".to_string(),
        );
        let mirrors = extract_mirrors(&root, &test_dir, &spec_owner).unwrap();
        assert_eq!(
            mirrors
                .get(&("Counter".to_string(), "increment_spec".to_string()))
                .unwrap(),
            &vec!["test/verity/Counter.t.sol:CounterTest.testFuzzIncrement".to_string()]
        );
        assert_eq!(
            mirrors
                .get(&("Counter".to_string(), "getCount_spec".to_string()))
                .unwrap(),
            &vec!["test/verity/Counter.t.sol:CounterTest.testFuzzGetterMirror".to_string()]
        );
    }

    #[test]
    fn extract_mirrors_rejects_unknown_spec() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let test_dir = root.join("test/verity");
        let test_file = test_dir.join("Foo.t.sol");
        tama_common::write_string(
            &test_file,
            r#"contract FooTest {
    // tama: mirrors=mystery_spec
    function testFuzzFoo() public {}
}
"#,
        )
        .unwrap();
        let err = extract_mirrors(&root, &test_dir, &BTreeMap::new()).unwrap_err();
        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("does not match any known spec")
        ));
    }

    #[test]
    fn count_brace_delta_skips_comments_and_strings() {
        assert_eq!(count_brace_delta("contract Foo {"), 1);
        assert_eq!(count_brace_delta("function f() public { return; }"), 0);
        assert_eq!(count_brace_delta("    // close } brace"), 0);
        assert_eq!(count_brace_delta("bytes memory s = \"}\";"), 0);
        assert_eq!(count_brace_delta("/* } */ {"), 1);
        assert_eq!(count_brace_delta("if (a) { /* inner } */ }"), 0);
    }

    #[test]
    fn extract_mirrors_skips_multi_line_block_comments() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let test_dir = root.join("test/verity");
        let test_file = test_dir.join("Foo.t.sol");
        tama_common::write_string(
            &test_file,
            r#"/*
 contract FakeOuter {
   function testFuzzFake() public {}
 }
*/

contract FooTest {
    // tama: mirrors=foo_spec
    function testFuzzReal(uint256 x) public {}
}
"#,
        )
        .unwrap();
        let mut spec_owner = BTreeMap::new();
        spec_owner.insert("foo_spec".to_string(), "Foo".to_string());
        let mirrors = extract_mirrors(&root, &test_dir, &spec_owner).unwrap();
        assert_eq!(
            mirrors
                .get(&("Foo".to_string(), "foo_spec".to_string()))
                .map(|v| v.as_slice()),
            Some(&["test/verity/Foo.t.sol:FooTest.testFuzzReal".to_string()][..]),
            "block-commented contract must not affect parsing, got {mirrors:?}"
        );
    }

    #[test]
    fn extract_mirrors_recognizes_abstract_contract_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let test_dir = root.join("test/verity");
        let test_file = test_dir.join("Foo.t.sol");
        tama_common::write_string(
            &test_file,
            r#"abstract contract FooTestBase {
    // tama: mirrors=foo_spec
    function testFuzzShared(uint256 x) public {}
}

contract ConcreteFooTest is FooTestBase {
    function setUp() public {}
}
"#,
        )
        .unwrap();
        let mut spec_owner = BTreeMap::new();
        spec_owner.insert("foo_spec".to_string(), "Foo".to_string());
        let mirrors = extract_mirrors(&root, &test_dir, &spec_owner).unwrap();
        assert_eq!(
            mirrors
                .get(&("Foo".to_string(), "foo_spec".to_string()))
                .map(|v| v.as_slice()),
            Some(&["test/verity/Foo.t.sol:FooTestBase.testFuzzShared".to_string()][..])
        );
    }

    #[test]
    fn extract_mirrors_ignores_tags_inside_function_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let test_dir = root.join("test/verity");
        let test_file = test_dir.join("Foo.t.sol");
        tama_common::write_string(
            &test_file,
            r#"contract FooTest {
    function helper() internal {
        // tama: mirrors=should_not_attach
        bytes memory x = "";
    }

    function testFuzzReal() public {}
}
"#,
        )
        .unwrap();
        let mut spec_owner = BTreeMap::new();
        spec_owner.insert("should_not_attach".to_string(), "Foo".to_string());
        let mirrors = extract_mirrors(&root, &test_dir, &spec_owner).unwrap();
        assert!(
            mirrors.is_empty(),
            "tag inside function body must not bind to subsequent test, got {mirrors:?}"
        );
    }

    #[test]
    fn extract_mirrors_rejects_non_property_test() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let test_dir = root.join("test/verity");
        let test_file = test_dir.join("Foo.t.sol");
        tama_common::write_string(
            &test_file,
            r#"contract FooTest {
    // tama: mirrors=foo_spec
    function testSomething() public {}
}
"#,
        )
        .unwrap();
        let mut spec_owner = BTreeMap::new();
        spec_owner.insert("foo_spec".to_string(), "Foo".to_string());
        let err = extract_mirrors(&root, &test_dir, &spec_owner).unwrap_err();
        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("non-property-shaped test")
        ));
    }

    #[test]
    fn counter_fixture_obligations_cover_behavior_with_properties() {
        let root =
            Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/counter");
        let specs = extract_specs(&root.join("verity/spec/CounterSpec.lean")).unwrap();
        let dischargers = extract_dischargers(
            &root.join("verity/proof/CounterProof.lean"),
            "proof.CounterProof",
            &specs,
        )
        .unwrap();
        let mut spec_owner = BTreeMap::new();
        for name in &specs {
            spec_owner.insert(name.clone(), "Counter".to_string());
        }
        let mirrors = extract_mirrors(&root, &root.join("test/verity"), &spec_owner).unwrap();
        let test = tama_common::read_to_string(&root.join("test/verity/Counter.t.sol")).unwrap();
        assert!(test.contains("function invariant_countTracksModel"));
        assert!(test.contains("handlerIncrement"));
        assert!(test.contains("handlerDecrement"));
        assert!(test.contains("contract CounterTest is MinimalStdInvariant"));
        assert!(test.contains("function targetSelectors()"));
        for spec in &specs {
            assert!(
                dischargers.contains_key(spec),
                "spec `{spec}` has no discharger"
            );
            let key = ("Counter".to_string(), spec.clone());
            assert!(mirrors.contains_key(&key), "spec `{spec}` has no mirror");
        }
    }

    #[test]
    fn adapter_accepts_upstream_abi_file_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        write_contract_files(&root, "Counter");
        write_empty_layout_report(&root, &["Counter"]);
        tama_common::write_string(&root.join("artifacts/abi/Counter.abi.json"), "[]\n").unwrap();
        tama_common::write_string(&root.join("artifacts/yul/Counter.yul"), "{ }\n").unwrap();
        let manifests = adapt_verity_outputs(&root, &test_config(), None).unwrap();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].contract, "Counter");
        assert_eq!(
            manifests[0].artifacts.yul,
            Utf8PathBuf::from("artifacts/yul/Counter.yul")
        );
        assert!(root.join("artifacts/manifest/Counter.json").is_file());
    }

    #[test]
    fn adapter_orders_manifests_by_abi_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        for contract in ["Zed", "Alpha"] {
            write_contract_files(&root, contract);
            tama_common::write_string(
                &root.join(format!("artifacts/abi/{contract}.abi.json")),
                "[]\n",
            )
            .unwrap();
            tama_common::write_string(&root.join(format!("artifacts/yul/{contract}.yul")), "{ }\n")
                .unwrap();
        }
        write_empty_layout_report(&root, &["Zed", "Alpha"]);

        let manifests = adapt_verity_outputs(&root, &test_config(), None).unwrap();

        assert_eq!(
            manifests
                .iter()
                .map(|manifest| manifest.contract.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "Zed"]
        );
    }

    #[test]
    fn adapter_filter_still_discovers_specs_from_other_contracts() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        for contract in ["Foo", "Bar"] {
            write_contract_files(&root, contract);
            tama_common::write_string(
                &root.join(format!("artifacts/abi/{contract}.abi.json")),
                "[]\n",
            )
            .unwrap();
            tama_common::write_string(&root.join(format!("artifacts/yul/{contract}.yul")), "{ }\n")
                .unwrap();
        }
        tama_common::write_string(
            &root.join("verity/spec/FooSpec.lean"),
            "namespace spec.FooSpec\ndef foo_spec (n : Nat) : Prop := n = 0\nend spec.FooSpec\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join("verity/spec/BarSpec.lean"),
            "namespace spec.BarSpec\ndef bar_spec (n : Nat) : Prop := n = 0\nend spec.BarSpec\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join("verity/proof/FooProof.lean"),
            "namespace proof.FooProof\n-- tama: discharges=foo_spec\ntheorem t : True := by trivial\nend proof.FooProof\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join("verity/proof/BarProof.lean"),
            "namespace proof.BarProof\n-- tama: discharges=bar_spec\ntheorem t : True := by trivial\nend proof.BarProof\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join("test/verity/Foo.t.sol"),
            "contract FooTest {\n    // tama: mirrors=foo_spec\n    function testFuzzFoo() public {}\n}\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join("test/verity/Bar.t.sol"),
            "contract BarTest {\n    // tama: mirrors=bar_spec\n    function testFuzzBar() public {}\n}\n",
        )
        .unwrap();
        write_empty_layout_report(&root, &["Foo", "Bar"]);

        let manifests = adapt_verity_outputs(&root, &test_config(), Some("Foo")).unwrap();

        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].contract, "Foo");
        assert_eq!(manifests[0].obligations[0].mirrors.len(), 1);
    }

    #[test]
    fn adapter_rejects_generated_contracts_without_project_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(&root.join("artifacts/abi/Counter.abi.json"), "[]\n").unwrap();
        tama_common::write_string(&root.join("artifacts/yul/Counter.yul"), "{ }\n").unwrap();
        write_empty_layout_report(&root, &["Counter"]);

        let err = adapt_verity_outputs(&root, &test_config(), None).unwrap_err();

        assert!(matches!(
            err,
            Error::MissingProjectFile { path, .. } if path.as_str() == "verity/src/Counter.lean"
        ));
    }

    #[test]
    fn adapter_rejects_missing_layout_report() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        write_contract_files(&root, "Counter");
        tama_common::write_string(&root.join("artifacts/abi/Counter.abi.json"), "[]\n").unwrap();
        tama_common::write_string(&root.join("artifacts/yul/Counter.yul"), "{ }\n").unwrap();

        let err = adapt_verity_outputs(&root, &test_config(), None).unwrap_err();

        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("layout report is missing")
        ));
        assert!(!root.join("artifacts/manifest/Counter.json").exists());
    }

    #[test]
    fn storage_adapter_parses_supported_layout_fields() {
        let report = json!({
            "contracts": [{
                "contract": "Counter",
                "fields": [
                    {"name": "owner", "canonicalSlot": 0, "type": {"kind": "address"}},
                    {"name": "enabled", "canonicalSlot": 1, "type": {"kind": "bool"}},
                    {"name": "count", "canonicalSlot": 2, "type": {"kind": "uint256"}},
                    {"name": "balances", "canonicalSlot": 3, "type": {"kind": "mapping"}}
                ]
            }]
        });

        let storage = parse_storage(&report, "Counter").unwrap();

        assert_eq!(
            storage
                .iter()
                .map(|entry| (entry.name.as_str(), entry.ty.as_str(), entry.slot.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("owner", "address", "0x00"),
                ("enabled", "bool", "0x01"),
                ("count", "uint256", "0x02"),
                ("balances", "mapping(address => uint256)", "0x03"),
            ]
        );
    }

    #[test]
    fn storage_adapter_rejects_malformed_layout_fields() {
        let missing_contracts = json!({});
        assert!(matches!(
            parse_storage(&missing_contracts, "Counter"),
            Err(Error::Adapter(message)) if message.contains("contracts")
        ));

        let missing_contract = json!({
            "contracts": [{
                "contract": "Other",
                "fields": []
            }]
        });
        assert!(matches!(
            parse_storage(&missing_contract, "Counter"),
            Err(Error::Adapter(message)) if message.contains("Counter layout report entry is missing")
        ));

        let missing_slot = json!({
            "contracts": [{
                "contract": "Counter",
                "fields": [{"name": "count", "type": {"kind": "uint256"}}]
            }]
        });
        assert!(matches!(
            parse_storage(&missing_slot, "Counter"),
            Err(Error::Adapter(message)) if message.contains("canonicalSlot")
        ));

        let unsupported_type = json!({
            "contracts": [{
                "contract": "Counter",
                "fields": [{"name": "nested", "canonicalSlot": 0, "type": {"kind": "nestedMapping"}}]
            }]
        });
        assert!(matches!(
            parse_storage(&unsupported_type, "Counter"),
            Err(Error::Adapter(message)) if message.contains("unsupported storage type")
        ));
    }

    #[test]
    fn abi_parser_preserves_event_indexed_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("Counter.abi.json")).unwrap();
        tama_common::write_string(
            &path,
            r#"[
  {
    "type": "event",
    "name": "Transfer",
    "inputs": [
      {"name": "from", "type": "address", "indexed": true},
      {"name": "to", "type": "address", "indexed": true},
      {"name": "amount", "type": "uint256", "indexed": false}
    ],
    "anonymous": false
  }
]
"#,
        )
        .unwrap();
        let abi = parse_abi(&path).unwrap();
        assert_eq!(abi.events.len(), 1);
        assert_eq!(
            abi.events[0]
                .fields
                .iter()
                .map(|field| field.indexed)
                .collect::<Vec<_>>(),
            vec![true, true, false]
        );
    }

    #[test]
    fn abi_parser_rejects_missing_names_and_unsupported_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("Counter.abi.json")).unwrap();

        tama_common::write_string(&path, r#"[{"type":"function","inputs":[],"outputs":[]}]"#)
            .unwrap();
        let err = parse_abi(&path).unwrap_err();
        assert!(matches!(err, Error::Adapter(message) if message.contains("missing `name`")));

        tama_common::write_string(&path, r#"[{"type":"fallback"}]"#).unwrap();
        let err = parse_abi(&path).unwrap_err();
        assert!(
            matches!(err, Error::Adapter(message) if message.contains("unsupported ABI entry type"))
        );

        tama_common::write_string(
            &path,
            r#"[{"type":"event","name":"Hidden","anonymous":true,"inputs":[]}]"#,
        )
        .unwrap();
        let err = parse_abi(&path).unwrap_err();
        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("anonymous event")
        ));

        tama_common::write_string(
            &path,
            r#"[{"type":"function","name":"setHash","inputs":[{"name":"hash","type":"bytes32"}],"outputs":[]}]"#,
        )
        .unwrap();
        let err = parse_abi(&path).unwrap_err();
        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("unsupported ABI type `bytes32`")
        ));
    }

    fn fixture_obligation() -> Obligation {
        Obligation {
            id: "Counter.increment_spec".to_string(),
            name: "increment_spec".to_string(),
            lean_decl: "spec.CounterSpec.increment_spec".to_string(),
            contract: "Counter".to_string(),
            dischargers: vec!["proof.CounterProof.increment_meets_spec".to_string()],
            mirrors: vec![
                "test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount".to_string(),
            ],
            proof_only_reason: None,
        }
    }

    #[test]
    fn print_axioms_output_is_parsed_into_probe_json() {
        let obligation = fixture_obligation();
        let output = r#"
TAMA_AXIOMS_BEGIN proof.CounterProof.increment_meets_spec
proof.CounterProof.increment_meets_spec depends on axioms: [propext, Classical.choice]
TAMA_AXIOMS_END proof.CounterProof.increment_meets_spec
"#;
        let report = parse_print_axioms_output(output, &[&obligation]).unwrap();
        assert_eq!(
            report,
            json!({
                "schema": "tama.trust-probe.v1.fallback",
                "method": "lean.printAxioms",
                "obligations": [{
                    "lean_decl": "spec.CounterSpec.increment_spec",
                    "dischargers": [{
                        "lean_decl": "proof.CounterProof.increment_meets_spec",
                        "axioms": ["propext", "Classical.choice"]
                    }]
                }]
            })
        );
    }

    #[test]
    fn collect_axioms_probe_uses_lean_api() {
        let obligation = fixture_obligation();
        let mut manifest = test_manifest("Counter");
        manifest.obligations.push(obligation.clone());

        let source = collect_axioms_probe_source(&[manifest], &[&obligation]).unwrap();

        assert!(source.contains("collectAxioms constName"));
        assert!(source.contains(r#""method", Json.str "lean.collectAxioms""#));
        assert!(source.contains(r#""dischargers", Json.arr dischargers_0"#));
        assert!(!source.contains("#print axioms"));
    }

    #[test]
    fn collect_axioms_output_is_parsed_into_probe_json() {
        let obligation = fixture_obligation();
        let output = r#"{"schema":"tama.trust-probe.v1","obligations":[{"lean_decl":"spec.CounterSpec.increment_spec","dischargers":[{"lean_decl":"proof.CounterProof.increment_meets_spec","axioms":["Quot.sound","propext"]}]}],"method":"lean.collectAxioms"}"#;

        let report = parse_collect_axioms_output(output, &[&obligation]).unwrap();

        assert_eq!(
            report.get("schema").and_then(Value::as_str),
            Some("tama.trust-probe.v1")
        );
    }

    #[test]
    fn print_axioms_output_fails_when_obligation_is_missing() {
        let obligation = fixture_obligation();
        let err = parse_print_axioms_output("", &[&obligation]).unwrap_err();
        assert!(
            matches!(err, Error::Adapter(message) if message.contains("trust probe did not report"))
        );
    }

    #[test]
    fn deployer_encodes_constructor_args() {
        let mut manifest = ContractManifest {
            schema: SCHEMA.to_string(),
            contract: "WithConstructor".to_string(),
            source: SourcePaths {
                implementation: "verity/src/WithConstructor.lean".into(),
                spec: "verity/spec/WithConstructorSpec.lean".into(),
                proof: "verity/proof/WithConstructorProof.lean".into(),
            },
            lean: LeanModules {
                implementation_module: "src.WithConstructor".to_string(),
                spec_module: "spec.WithConstructorSpec".to_string(),
                proof_module: "proof.WithConstructorProof".to_string(),
            },
            abi: Abi::default(),
            storage: vec![],
            obligations: vec![],
            artifacts: ArtifactPaths {
                yul: "artifacts/yul/WithConstructor.yul".into(),
                creation_bytecode: "artifacts/bytecode/WithConstructor.bin".into(),
                runtime_bytecode: "artifacts/bytecode/WithConstructor.runtime.bin".into(),
                bytecode_hash: None,
                solc_input: "artifacts/solc-json/WithConstructor.input.json".into(),
                solc_output: "artifacts/solc-json/WithConstructor.output.json".into(),
                interface: "src/generated/verity/WithConstructorIface.sol".into(),
                deployer: "src/generated/verity/WithConstructorDeployer.sol".into(),
            },
        };
        manifest.abi.constructor = Some(Constructor {
            inputs: vec![
                Param {
                    name: "owner".to_string(),
                    ty: "address".to_string(),
                },
                Param {
                    name: "".to_string(),
                    ty: "uint256".to_string(),
                },
            ],
        });
        let sol = deployer_sol(&manifest, "6000");
        assert!(sol.contains("function deploy(address owner, uint256 arg1)"));
        assert!(sol.contains(r#"abi.encodePacked(hex"6000", abi.encode(owner, arg1))"#));
    }

    #[test]
    fn solc_standard_json_includes_yul_optimizer_setting() {
        let mut config = test_config();
        config.yul.optimizer = false;
        config.yul.optimizer_runs = 1;
        config.yul.yul_optimizer = false;

        let input = solc_standard_json_input("Counter", "object \"Counter\" {}", &config);

        assert_eq!(input["settings"]["optimizer"]["enabled"], false);
        assert_eq!(input["settings"]["optimizer"]["runs"], 1);
        assert_eq!(input["settings"]["optimizer"]["details"]["yul"], false);
    }

    #[cfg(unix)]
    #[test]
    fn compile_yul_rejects_wrong_solc_version_before_bytecode() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let solc = root.join("solc");
        tama_common::write_string(
            &solc,
            "#!/bin/sh\nprintf '%s\\n' 'Version: 0.8.32+commit.test'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&solc).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&solc, permissions).unwrap();
        let _solc_guard = EnvVarGuard::set("TAMA_SOLC", solc.as_os_str());
        let config = test_config();
        let mut manifest = test_manifest("Counter");
        tama_common::write_string(
            &root.join(&manifest.artifacts.yul),
            "object \"Counter\" { code { stop() } }\n",
        )
        .unwrap();

        let err = compile_yul_standard_json(&root, &config, &mut manifest).unwrap_err();

        assert!(matches!(
            err,
            Error::Toolchain(tama_toolchain::Error::ToolVersionMismatch(message))
                if message.contains("0.8.32") && message.contains("0.8.33")
        ));
        assert!(!root.join(&manifest.artifacts.creation_bytecode).exists());
        assert!(!root.join(&manifest.artifacts.runtime_bytecode).exists());
        assert_eq!(manifest.artifacts.bytecode_hash, None);
    }

    #[test]
    fn compile_yul_reports_missing_solc_before_bytecode() {
        let _guard = ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let bin = root.join("bin");
        let home = root.join("home");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let _solc_guard = EnvVarGuard::unset("TAMA_SOLC");
        let _path_guard = EnvVarGuard::set("PATH", bin.as_os_str());
        let _home_guard = EnvVarGuard::set("HOME", home.as_os_str());
        let config = test_config();
        let mut manifest = test_manifest("Counter");
        tama_common::write_string(
            &root.join(&manifest.artifacts.yul),
            "object \"Counter\" { code { stop() } }\n",
        )
        .unwrap();

        let err = compile_yul_standard_json(&root, &config, &mut manifest).unwrap_err();

        assert!(matches!(
            err,
            Error::Toolchain(tama_toolchain::Error::MissingTool(name)) if name == "solc"
        ));
        assert!(!root.join(&manifest.artifacts.creation_bytecode).exists());
        assert!(!root.join(&manifest.artifacts.runtime_bytecode).exists());
        assert_eq!(manifest.artifacts.bytecode_hash, None);
    }

    fn test_config() -> TamaConfig {
        TamaConfig {
            project: tama_config::ProjectConfig {
                name: "test".to_string(),
                verity: "0.1.0".to_string(),
            },
            paths: tama_config::PathsConfig::default(),
            yul: tama_config::YulConfig {
                solc: "0.8.33".to_string(),
                optimizer: true,
                optimizer_runs: 200,
                yul_optimizer: true,
                evm_version: "cancun".to_string(),
                metadata_hash: "none".to_string(),
            },
            trust: tama_config::TrustConfig::default(),
            coverage: tama_config::CoverageConfig::default(),
        }
    }

    fn write_contract_files(root: &Utf8Path, contract: &str) {
        tama_common::write_string(
            &root.join(format!("verity/src/{contract}.lean")),
            "import Contracts.Common\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join(format!("verity/spec/{contract}Spec.lean")),
            &format!("import src.{contract}\n"),
        )
        .unwrap();
        tama_common::write_string(
            &root.join(format!("verity/proof/{contract}Proof.lean")),
            &format!("import spec.{contract}Spec\n"),
        )
        .unwrap();
    }

    fn write_empty_layout_report(root: &Utf8Path, contracts: &[&str]) {
        let contracts = contracts
            .iter()
            .map(|contract| json!({ "contract": contract, "fields": [] }))
            .collect::<Vec<_>>();
        tama_common::write_string(
            &root.join("artifacts/layout-report.json"),
            &(serde_json::to_string_pretty(&json!({ "contracts": contracts })).unwrap() + "\n"),
        )
        .unwrap();
    }

    fn test_manifest(contract: &str) -> ContractManifest {
        ContractManifest {
            schema: SCHEMA.to_string(),
            contract: contract.to_string(),
            source: SourcePaths {
                implementation: format!("verity/src/{contract}.lean").into(),
                spec: format!("verity/spec/{contract}Spec.lean").into(),
                proof: format!("verity/proof/{contract}Proof.lean").into(),
            },
            lean: LeanModules {
                implementation_module: format!("src.{contract}"),
                spec_module: format!("spec.{contract}Spec"),
                proof_module: format!("proof.{contract}Proof"),
            },
            abi: Abi::default(),
            storage: vec![],
            obligations: vec![],
            artifacts: ArtifactPaths {
                yul: format!("artifacts/yul/{contract}.yul").into(),
                creation_bytecode: format!("artifacts/bytecode/{contract}.bin").into(),
                runtime_bytecode: format!("artifacts/bytecode/{contract}.runtime.bin").into(),
                bytecode_hash: None,
                solc_input: format!("artifacts/solc-json/{contract}.input.json").into(),
                solc_output: format!("artifacts/solc-json/{contract}.output.json").into(),
                interface: format!("src/generated/verity/{contract}Iface.sol").into(),
                deployer: format!("src/generated/verity/{contract}Deployer.sol").into(),
            },
        }
    }
}
