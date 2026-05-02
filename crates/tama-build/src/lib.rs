use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use tama_config::{TamaConfig, TamaLock};
use tama_manifest::{
    Abi, ArtifactPaths, Constructor, ContractManifest, Coverage, CoverageDisposition, ErrorEntry,
    Event, Function, LeanModules, Obligation, ObligationKind, Param, SourcePaths, StorageEntry,
    SCHEMA,
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
            discover_modules(&self.root.join(&config.paths.src))?
                .into_iter()
                .map(|module| format!("{module}\n"))
                .collect()
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

        let lake = Lake::new_json(self.root.clone(), opts.json);
        lake.build_proofs()?;
        lake.verity_codegen(&config, &opts)?;
        let mut manifests = adapt_verity_outputs(&self.root, &config, opts.contract.as_deref())?;
        generate_trust_probe(&self.root, &config, &manifests)?;
        for manifest in &mut manifests {
            manifest.validate()?;
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
                continue;
            }
            compile_yul_standard_json(&self.root, &config, manifest)?;
            generate_bridge(&self.root, manifest)?;
        }
        if should_run_forge(&opts) {
            run_owned(
                "forge",
                &forge_build_args(opts.offline),
                &self.root,
                opts.json,
            )?;
        }
        if !opts.locked {
            tama_config::update_lock_inputs(&self.root, &mut lock)?;
            tama_config::write_lock(&self.root, &lock)?;
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
    let mut manifests = Vec::new();
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
    for path in abi_paths {
        let contract =
            contract_name_from_abi_path(&path).ok_or_else(|| Error::Adapter(path.to_string()))?;
        if contract_filter.is_some_and(|filter| filter != contract) {
            continue;
        }
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
        let proof_module = format!("proof.{contract}Proof");
        let manifest = ContractManifest {
            schema: SCHEMA.to_string(),
            contract: contract.clone(),
            source,
            lean: LeanModules {
                implementation_module: format!("src.{contract}"),
                spec_module: format!("spec.{contract}Spec"),
                proof_module: proof_module.clone(),
            },
            abi: parse_abi(&path)?,
            storage: parse_storage(&storage_report, &contract)?,
            obligations: extract_obligations(root, config, &contract, &proof_module)?,
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
    let input_path = root.join(&manifest.artifacts.solc_input);
    tama_common::write_string(
        &input_path,
        &(serde_json::to_string_pretty(&input).unwrap() + "\n"),
    )?;

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
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(serde_json::to_string(&input).unwrap().as_bytes())
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

fn extract_obligations(
    root: &Utf8Path,
    config: &TamaConfig,
    contract: &str,
    proof_module: &str,
) -> Result<Vec<Obligation>> {
    let proof_path = root.join(config.paths.proof.join(format!("{contract}Proof.lean")));
    if !proof_path.is_file() {
        return Ok(Vec::new());
    }
    let text = tama_common::read_to_string(&proof_path)?;
    let theorem_re = Regex::new(
        r"^\s*(?:@\[[^\]]*\]\s*)*(?:(?:private|protected)\s+)*(?:theorem|lemma)\s+([A-Za-z_][A-Za-z0-9_.']*)\b",
    )
    .expect("valid theorem regex");
    let mut obligations = Vec::new();
    let mut pending = ObligationMeta::default();
    for line in strip_lean_block_comments(&text).lines() {
        let trimmed = line.trim();
        let metadata_line = parse_obligation_metadata(trimmed, &proof_path, &mut pending)?;
        if let Some(captures) = theorem_re.captures(trimmed) {
            if let Some(meta) = pending.take_if_obligation() {
                let name = captures.get(1).expect("theorem name").as_str();
                obligations.push(Obligation {
                    id: format!("{contract}.{name}"),
                    name: name.to_string(),
                    kind: meta.kind.unwrap_or(ObligationKind::Postcondition),
                    lean_decl: format!("{proof_module}.{name}"),
                    contract: contract.to_string(),
                    function: meta.function,
                    coverage: meta.coverage,
                });
            }
            pending = ObligationMeta::default();
        } else if !metadata_line && !trimmed.is_empty() && !trimmed.starts_with("@[") {
            pending = ObligationMeta::default();
        }
    }
    Ok(obligations)
}

#[derive(Debug, Clone)]
struct ObligationMeta {
    tagged: bool,
    kind: Option<ObligationKind>,
    function: Option<String>,
    coverage: Coverage,
}

impl Default for ObligationMeta {
    fn default() -> Self {
        Self {
            tagged: false,
            kind: None,
            function: None,
            coverage: Coverage {
                disposition: CoverageDisposition::None,
                path: None,
                reason: None,
            },
        }
    }
}

impl ObligationMeta {
    fn take_if_obligation(&self) -> Option<Self> {
        if self.tagged || self.kind.is_some() {
            Some(self.clone())
        } else {
            None
        }
    }
}

fn parse_obligation_metadata(
    line: &str,
    proof_path: &Utf8Path,
    meta: &mut ObligationMeta,
) -> Result<bool> {
    let mut parsed = false;
    if let Some(raw) = line.trim_start().strip_prefix("-- tama:") {
        parsed = true;
        apply_tama_metadata(raw, proof_path, meta)?;
    }
    if line.contains("tama.") {
        parsed = true;
        apply_tama_attribute_metadata(line, meta);
    }
    Ok(parsed)
}

fn apply_tama_metadata(raw: &str, proof_path: &Utf8Path, meta: &mut ObligationMeta) -> Result<()> {
    let values = parse_key_values(raw);
    for key in values.keys() {
        if !tama_metadata_key_supported(key) {
            return Err(Error::Adapter(format!(
                "unsupported Tama metadata key `{key}` in {proof_path}"
            )));
        }
    }
    if values.contains_key("obligation") {
        meta.tagged = true;
    }
    match values.get("kind").map(String::as_str) {
        Some("helper") => {
            meta.tagged = true;
            meta.kind = Some(ObligationKind::Helper);
        }
        Some("invariant") => {
            meta.tagged = true;
            meta.kind = Some(ObligationKind::Invariant);
        }
        Some("postcondition") => {
            meta.tagged = true;
            meta.kind = Some(ObligationKind::Postcondition);
        }
        _ if values.contains_key("helper") => {
            meta.tagged = true;
            meta.kind = Some(ObligationKind::Helper);
        }
        _ if values.contains_key("invariant") => {
            meta.tagged = true;
            meta.kind = Some(ObligationKind::Invariant);
        }
        _ if values.contains_key("postcondition") => {
            meta.tagged = true;
            meta.kind = Some(ObligationKind::Postcondition);
        }
        Some(kind) => {
            return Err(Error::Adapter(format!(
                "unsupported Tama obligation kind `{kind}` in {proof_path}"
            )));
        }
        _ => {}
    }
    if let Some(function) = values.get("function").filter(|value| !value.is_empty()) {
        meta.function = Some(function.clone());
    }
    match values.get("coverage").map(String::as_str) {
        Some("mirror") => {
            meta.coverage.disposition = CoverageDisposition::Mirror;
            meta.coverage.path = values.get("path").cloned();
            meta.coverage.reason = None;
        }
        Some("proof_only") => {
            meta.coverage.disposition = CoverageDisposition::ProofOnly;
            meta.coverage.path = None;
            meta.coverage.reason = values.get("reason").cloned();
        }
        Some("none") => {
            meta.coverage.disposition = CoverageDisposition::None;
            meta.coverage.path = None;
            meta.coverage.reason = None;
        }
        Some(coverage) => {
            return Err(Error::Adapter(format!(
                "unsupported Tama coverage disposition `{coverage}` in {proof_path}"
            )));
        }
        _ => {}
    }
    Ok(())
}

fn tama_metadata_key_supported(key: &str) -> bool {
    matches!(
        key,
        "obligation"
            | "kind"
            | "function"
            | "coverage"
            | "path"
            | "reason"
            | "helper"
            | "invariant"
            | "postcondition"
    )
}

fn apply_tama_attribute_metadata(line: &str, meta: &mut ObligationMeta) {
    if line.contains("tama.obligation") {
        meta.tagged = true;
    }
    if line.contains("tama.helper") {
        meta.tagged = true;
        meta.kind = Some(ObligationKind::Helper);
    }
    if line.contains("tama.invariant") {
        meta.tagged = true;
        meta.kind = Some(ObligationKind::Invariant);
    }
    if let Some(rest) = line.split("tama.postcondition").nth(1) {
        meta.tagged = true;
        meta.kind = Some(ObligationKind::Postcondition);
        if let Some(function) = rest
            .split([']', ','])
            .next()
            .and_then(|value| value.split_whitespace().next())
            .filter(|value| !value.is_empty())
        {
            meta.function = Some(function.rsplit('.').next().unwrap_or(function).to_string());
        }
    }
    if line.contains("tama.mirror") {
        if let Some(path) = first_quoted_value(line) {
            meta.coverage.disposition = CoverageDisposition::Mirror;
            meta.coverage.path = Some(path);
            meta.coverage.reason = None;
        }
    }
    if line.contains("tama.proof_only") {
        if let Some(reason) = first_quoted_value(line) {
            meta.coverage.disposition = CoverageDisposition::ProofOnly;
            meta.coverage.path = None;
            meta.coverage.reason = Some(reason);
        }
    }
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
            key.push(chars.next().expect("peeked char"));
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
                value.push(chars.next().expect("peeked char"));
            }
            value
        };
        values.insert(key, value);
    }
    values
}

fn first_quoted_value(line: &str) -> Option<String> {
    let (_, rest) = line.split_once('"')?;
    let (value, _) = rest.split_once('"')?;
    Some(value.to_string())
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
    let public_obligations = manifests
        .iter()
        .flat_map(|manifest| {
            manifest
                .obligations
                .iter()
                .filter(|obligation| obligation.kind != ObligationKind::Helper)
        })
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
    if public_obligations.is_empty() {
        let report = json!({
            "schema": "tama.trust-probe.v1",
            "method": "lean.collectAxioms",
            "obligations": []
        });
        tama_common::write_string(
            &output_path,
            &(serde_json::to_string_pretty(&report).expect("trust report JSON") + "\n"),
        )?;
        return Ok(());
    }

    let source_path = probe_dir.join("CollectAxioms.lean");
    let source = collect_axioms_probe_source(manifests, &public_obligations)?;
    tama_common::write_string(&source_path, &source)?;
    let output = run_capture("lake", &["env", "lean", source_path.as_str()], root)?;
    let report = parse_collect_axioms_output(&output.stdout, &public_obligations)?;
    tama_common::write_string(
        &output_path,
        &(serde_json::to_string_pretty(&report).expect("trust report JSON") + "\n"),
    )?;
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
    for (index, obligation) in obligations.iter().enumerate() {
        let name = lean_name_literal(&obligation.lean_decl)?;
        out.push_str(&format!(
            "  let obligation{index} ← tamaAxiomJson \"{}\" {name}\n",
            obligation.lean_decl
        ));
    }
    out.push_str("  let obligations := #[");
    for index in 0..obligations.len() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("obligation{index}"));
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
    let expected = obligations
        .iter()
        .map(|obligation| obligation.lean_decl.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let reported = reported_obligations
        .iter()
        .filter_map(|entry| entry.get("lean_decl").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(decl) = expected.difference(&reported).next() {
        return Err(Error::Adapter(format!(
            "trust probe did not report axioms for {decl}"
        )));
    }
    Ok(value)
}

fn lean_name_literal(name: &str) -> Result<String> {
    validate_lean_name(name)?;
    Ok(format!("`{name}"))
}

fn validate_lean_name(name: &str) -> Result<()> {
    if name.split('.').all(valid_lean_name_segment) {
        return Ok(());
    }
    Err(Error::Adapter(format!(
        "trust probe Lean name `{name}` is not a supported identifier"
    )))
}

fn valid_lean_name_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '\'')
}

#[cfg(test)]
fn parse_print_axioms_output(output: &str, obligations: &[&Obligation]) -> Result<Value> {
    let expected = obligations
        .iter()
        .map(|obligation| obligation.lean_decl.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut current = None::<String>;
    let mut reported = std::collections::BTreeMap::<String, Vec<String>>::new();
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
                    "axioms": reported
                        .get(&obligation.lean_decl)
                        .cloned()
                        .unwrap_or_default()
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
    fn obligation_metadata_is_extracted_from_proof_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        tama_common::write_string(
            &root.join("verity/proof/CounterProof.lean"),
            r#"import spec.CounterSpec

namespace proof.CounterProof

-- tama: obligation kind=postcondition function=increment coverage=mirror path=test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount
theorem increment_post : True := by
  trivial

  -- tama: obligation kind=postcondition function=transfer coverage=mirror path=test/verity/Counter.t.sol:CounterTest.invariant_transferModel
theorem postcondition_with_invariant_mirror : True := by
  trivial

@[tama.helper]
lemma arithmetic_helper : True := by
  trivial

-- tama: obligation kind=invariant coverage=proof_only reason="Symbolic state only."
theorem supply_invariant : True := by
  trivial

end proof.CounterProof
"#,
        )
        .unwrap();
        let obligations = extract_obligations(&root, &config, "Counter", "proof.CounterProof")
            .expect("obligations");
        assert_eq!(obligations.len(), 4);
        assert_eq!(obligations[0].id, "Counter.increment_post");
        assert_eq!(obligations[0].kind, ObligationKind::Postcondition);
        assert_eq!(obligations[0].function.as_deref(), Some("increment"));
        assert_eq!(
            obligations[0].coverage.disposition,
            CoverageDisposition::Mirror
        );
        assert_eq!(
            obligations[0].coverage.path.as_deref(),
            Some("test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount")
        );
        assert_eq!(obligations[1].kind, ObligationKind::Postcondition);
        assert_eq!(obligations[1].function.as_deref(), Some("transfer"));
        assert_eq!(
            obligations[1].coverage.path.as_deref(),
            Some("test/verity/Counter.t.sol:CounterTest.invariant_transferModel")
        );
        assert_eq!(obligations[2].kind, ObligationKind::Helper);
        assert_eq!(
            obligations[2].lean_decl,
            "proof.CounterProof.arithmetic_helper"
        );
        assert_eq!(obligations[3].kind, ObligationKind::Invariant);
        assert_eq!(
            obligations[3].coverage.disposition,
            CoverageDisposition::ProofOnly
        );
        assert_eq!(
            obligations[3].coverage.reason.as_deref(),
            Some("Symbolic state only.")
        );
    }

    #[test]
    fn obligation_metadata_rejects_unknown_comment_values() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        let proof = root.join("verity/proof/CounterProof.lean");
        tama_common::write_string(
            &proof,
            r#"
namespace proof.CounterProof

-- tama: obligation kind=safety coverage=mirror path=test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount
theorem bad_kind : True := by
  trivial

end proof.CounterProof
"#,
        )
        .unwrap();

        let err = extract_obligations(&root, &config, "Counter", "proof.CounterProof").unwrap_err();

        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("unsupported Tama obligation kind `safety`")
        ));

        tama_common::write_string(
            &proof,
            r#"
namespace proof.CounterProof

-- tama: obligation kind=postcondition coverage=example path=test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount
theorem bad_coverage : True := by
  trivial

end proof.CounterProof
"#,
        )
        .unwrap();

        let err = extract_obligations(&root, &config, "Counter", "proof.CounterProof").unwrap_err();

        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("unsupported Tama coverage disposition `example`")
        ));

        tama_common::write_string(
            &proof,
            r#"
namespace proof.CounterProof

-- tama: obligation kind=postcondition functon=increment coverage=mirror path=test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount
theorem typoed_key : True := by
  trivial

end proof.CounterProof
"#,
        )
        .unwrap();

        let err = extract_obligations(&root, &config, "Counter", "proof.CounterProof").unwrap_err();

        assert!(matches!(
            err,
            Error::Adapter(message) if message.contains("unsupported Tama metadata key `functon`")
        ));
    }

    #[test]
    fn counter_fixture_obligations_cover_behavior_with_properties() {
        let root =
            Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/counter");
        let config = tama_config::load_config(&root).unwrap();
        let obligations =
            extract_obligations(&root, &config, "Counter", "proof.CounterProof").unwrap();
        let test = tama_common::read_to_string(&root.join("test/verity/Counter.t.sol")).unwrap();

        let functions = obligations
            .iter()
            .filter_map(|obligation| obligation.function.as_deref())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(functions.contains("increment"));
        assert!(functions.contains("decrement"));
        assert!(functions.contains("getCount"));
        assert!(test.contains("function invariant_countTracksModel"));
        assert!(test.contains("handlerIncrement"));
        assert!(test.contains("handlerDecrement"));
        assert!(test.contains("contract CounterTest is MinimalStdInvariant"));
        assert!(test.contains("function targetSelectors()"));
        assert!(!test.contains("abstract contract InvariantTargets"));

        for obligation in &obligations {
            assert_ne!(obligation.coverage.disposition, CoverageDisposition::None);
            let path = obligation
                .coverage
                .path
                .as_deref()
                .expect("fixture obligations use mirror coverage paths");
            let symbol = path
                .rsplit_once(':')
                .map(|(_, symbol)| symbol)
                .expect("mirror path is symbol-qualified");
            let name = symbol
                .rsplit_once('.')
                .map(|(_, name)| name)
                .expect("mirror path includes contract and function");
            assert!(
                name.starts_with("testFuzz") || name.starts_with("invariant_"),
                "{name} should be a Foundry property"
            );
            assert!(
                test.contains(&format!("function {name}")),
                "{name} should be declared in Counter.t.sol"
            );
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

    #[test]
    fn print_axioms_output_is_parsed_into_probe_json() {
        let obligation = Obligation {
            id: "Counter.increment_post".to_string(),
            name: "increment_post".to_string(),
            kind: ObligationKind::Postcondition,
            lean_decl: "proof.CounterProof.increment_post".to_string(),
            contract: "Counter".to_string(),
            function: Some("increment".to_string()),
            coverage: Coverage {
                disposition: CoverageDisposition::Mirror,
                path: Some(
                    "test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount"
                        .to_string(),
                ),
                reason: None,
            },
        };
        let output = r#"
TAMA_AXIOMS_BEGIN proof.CounterProof.increment_post
proof.CounterProof.increment_post depends on axioms: [propext, Classical.choice]
TAMA_AXIOMS_END proof.CounterProof.increment_post
"#;
        let report = parse_print_axioms_output(output, &[&obligation]).unwrap();
        assert_eq!(
            report,
            json!({
                "schema": "tama.trust-probe.v1.fallback",
                "method": "lean.printAxioms",
                "obligations": [{
                    "lean_decl": "proof.CounterProof.increment_post",
                    "axioms": ["propext", "Classical.choice"]
                }]
            })
        );
    }

    #[test]
    fn collect_axioms_probe_uses_lean_api() {
        let obligation = Obligation {
            id: "Counter.increment_post".to_string(),
            name: "increment_post".to_string(),
            kind: ObligationKind::Postcondition,
            lean_decl: "proof.CounterProof.increment_post".to_string(),
            contract: "Counter".to_string(),
            function: Some("increment".to_string()),
            coverage: Coverage {
                disposition: CoverageDisposition::Mirror,
                path: Some(
                    "test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount"
                        .to_string(),
                ),
                reason: None,
            },
        };
        let mut manifest = test_manifest("Counter");
        manifest.obligations.push(obligation.clone());

        let source = collect_axioms_probe_source(&[manifest], &[&obligation]).unwrap();

        assert!(source.contains("collectAxioms constName"));
        assert!(source.contains(r#""method", Json.str "lean.collectAxioms""#));
        assert!(!source.contains("#print axioms"));
    }

    #[test]
    fn collect_axioms_output_is_parsed_into_probe_json() {
        let obligation = Obligation {
            id: "Counter.increment_post".to_string(),
            name: "increment_post".to_string(),
            kind: ObligationKind::Postcondition,
            lean_decl: "proof.CounterProof.increment_post".to_string(),
            contract: "Counter".to_string(),
            function: Some("increment".to_string()),
            coverage: Coverage {
                disposition: CoverageDisposition::Mirror,
                path: Some(
                    "test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount"
                        .to_string(),
                ),
                reason: None,
            },
        };
        let output = r#"{"schema":"tama.trust-probe.v1","obligations":[{"lean_decl":"proof.CounterProof.increment_post","axioms":["Quot.sound","propext"]}],"method":"lean.collectAxioms"}"#;

        let report = parse_collect_axioms_output(output, &[&obligation]).unwrap();

        assert_eq!(
            report,
            json!({
                "schema": "tama.trust-probe.v1",
                "method": "lean.collectAxioms",
                "obligations": [{
                    "lean_decl": "proof.CounterProof.increment_post",
                    "axioms": ["Quot.sound", "propext"]
                }]
            })
        );
    }

    #[test]
    fn print_axioms_output_fails_when_obligation_is_missing() {
        let obligation = Obligation {
            id: "Counter.increment_post".to_string(),
            name: "increment_post".to_string(),
            kind: ObligationKind::Postcondition,
            lean_decl: "proof.CounterProof.increment_post".to_string(),
            contract: "Counter".to_string(),
            function: Some("increment".to_string()),
            coverage: Coverage {
                disposition: CoverageDisposition::Mirror,
                path: Some(
                    "test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount"
                        .to_string(),
                ),
                reason: None,
            },
        };
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
