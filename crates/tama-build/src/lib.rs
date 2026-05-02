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

pub struct Lake {
    root: Utf8PathBuf,
}

impl Lake {
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn check_src_and_spec(&self) -> Result<()> {
        run("lake", &["build", "TamaSrc", "TamaSpec"], &self.root)
    }

    pub fn build_proofs(&self) -> Result<()> {
        run("lake", &["build", "TamaProof"], &self.root)
    }

    pub fn verity_codegen(&self, config: &TamaConfig, opts: &BuildOptions) -> Result<()> {
        let out = config.paths.out.join("yul");
        let abi = config.paths.out.join("abi");
        fs::create_dir_all(self.root.join(&out))
            .map_err(|source| tama_common::io_error(self.root.join(&out), source))?;
        fs::create_dir_all(self.root.join(&abi))
            .map_err(|source| tama_common::io_error(self.root.join(&abi), source))?;
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
        run_owned("lake", &args, &self.root)
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
        let mut lock = tama_config::load_lock(&self.root).unwrap_or(TamaLock {
            version: 1,
            resolved: Default::default(),
            inputs: Default::default(),
            yul: Default::default(),
        });
        if opts.locked {
            tama_config::enforce_locked(&self.root, &lock)?;
        }

        let lake = Lake::new(self.root.clone());
        lake.build_proofs()?;
        lake.verity_codegen(&config, &opts)?;
        let mut manifests = adapt_verity_outputs(&self.root, &config, opts.contract.as_deref())?;
        for manifest in &mut manifests {
            manifest.validate()?;
            if opts.no_solc {
                continue;
            }
            compile_yul_standard_json(&self.root, &config, manifest)?;
            generate_bridge(&self.root, manifest)?;
        }
        if should_run_forge(&opts) {
            run("forge", &["build"], &self.root)?;
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

fn should_run_forge(opts: &BuildOptions) -> bool {
    !opts.no_forge && !opts.no_solc
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
        read_json_optional(&root.join(config.paths.out.join("layout-report.json")))?;
    let mut manifests = Vec::new();
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
        let contract = path
            .file_stem()
            .ok_or_else(|| Error::Adapter(path.to_string()))?
            .to_string();
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
        let proof_module = format!("proof.{contract}Proof");
        let manifest = ContractManifest {
            schema: SCHEMA.to_string(),
            contract: contract.clone(),
            source: SourcePaths {
                implementation: config.paths.src.join(format!("{contract}.lean")),
                spec: config.paths.spec.join(format!("{contract}Spec.lean")),
                proof: config.paths.proof.join(format!("{contract}Proof.lean")),
            },
            lean: LeanModules {
                implementation_module: format!("src.{contract}"),
                spec_module: format!("spec.{contract}Spec"),
                proof_module: proof_module.clone(),
            },
            abi: parse_abi(&path)?,
            storage: parse_storage(&storage_report, &contract),
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

pub fn compile_yul_standard_json(
    root: &Utf8Path,
    config: &TamaConfig,
    manifest: &mut ContractManifest,
) -> Result<()> {
    let contract = manifest.contract.clone();
    let yul_path = root.join(&manifest.artifacts.yul);
    let yul = tama_common::read_to_string(&yul_path)?;
    let input = json!({
        "language": "Yul",
        "sources": {
            format!("{contract}.yul"): { "content": yul }
        },
        "settings": {
            "optimizer": {
                "enabled": config.yul.optimizer,
                "runs": config.yul.optimizer_runs
            },
            "evmVersion": config.yul.evm_version,
            "metadata": {
                "bytecodeHash": config.yul.metadata_hash
            },
            "outputSelection": {
                "*": { "*": ["evm.bytecode.object", "evm.deployedBytecode.object"] }
            }
        }
    });
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
    let errors = value
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
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(Error::SolcErrors {
            contract,
            errors: errors.join("\n"),
        });
    }
    let (creation, runtime) = extract_solc_bytecode(&value)
        .ok_or_else(|| Error::Adapter("solc output did not contain bytecode".to_string()))?;
    let creation_path = root.join(&manifest.artifacts.creation_bytecode);
    let runtime_path = root.join(&manifest.artifacts.runtime_bytecode);
    tama_common::write_string(&creation_path, &(creation.clone() + "\n"))?;
    tama_common::write_string(&runtime_path, &(runtime + "\n"))?;
    manifest.artifacts.bytecode_hash = Some(tama_common::sha256_bytes(creation.as_bytes()));
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

pub fn generate_bridge(root: &Utf8Path, manifest: &ContractManifest) -> Result<()> {
    let bytecode = if root.join(&manifest.artifacts.creation_bytecode).is_file() {
        tama_common::read_to_string(&root.join(&manifest.artifacts.creation_bytecode))?
            .trim()
            .trim_start_matches("0x")
            .to_string()
    } else {
        String::new()
    };
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
            "view" | "pure" => format!(" {}", function.mutability),
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

fn run(program: &str, args: &[&str], cwd: &Utf8Path) -> Result<()> {
    let owned = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_owned(program, &owned, cwd)
}

fn run_owned(program: &str, args: &[String], cwd: &Utf8Path) -> Result<()> {
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
            return Ok(Self {
                path,
                marker,
                created: false,
            });
        }
        fs::create_dir_all(&path).map_err(|source| tama_common::io_error(path.clone(), source))?;
        tama_common::write_string(
            &marker,
            "Temporary Tama marker used while building Verity's evmyul FFI target.\n",
        )?;
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

fn read_json_optional(path: &Utf8Path) -> Result<Option<Value>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = tama_common::read_to_string(path)?;
    let value = serde_json::from_str(&text).map_err(|source| tama_manifest::Error::Json {
        path: path.to_owned(),
        source,
    })?;
    Ok(Some(value))
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
                abi.constructor = Some(Constructor {
                    inputs: entry.inputs,
                });
            }
            "function" => {
                let signature = format!(
                    "{}({})",
                    entry.name.clone().unwrap_or_default(),
                    entry
                        .inputs
                        .iter()
                        .map(|param| param.ty.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                );
                abi.functions.push(Function {
                    name: entry.name.unwrap_or_default(),
                    selector: tama_common::function_selector(&signature),
                    signature,
                    visibility: "external".to_string(),
                    mutability: entry
                        .state_mutability
                        .unwrap_or_else(|| "nonpayable".to_string()),
                    inputs: entry.inputs,
                    outputs: entry.outputs,
                });
            }
            "event" => {
                let name = entry.name.unwrap_or_default();
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
                            indexed: false,
                        })
                        .collect(),
                });
            }
            "error" => {
                let name = entry.name.unwrap_or_default();
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
                    inputs: entry.inputs,
                });
            }
            _ => {}
        }
    }
    Ok(abi)
}

fn parse_storage(report: &Option<Value>, contract: &str) -> Vec<StorageEntry> {
    let Some(report) = report else {
        return Vec::new();
    };
    let Some(contracts) = report.get("contracts").and_then(Value::as_array) else {
        return Vec::new();
    };
    contracts
        .iter()
        .find(|entry| entry.get("contract").and_then(Value::as_str) == Some(contract))
        .and_then(|entry| entry.get("fields").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .map(|field| {
            let name = field
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("field")
                .to_string();
            let slot = field
                .get("canonicalSlot")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let ty_value = field.get("type").cloned().unwrap_or(Value::Null);
            let (ty, encoding) = storage_type(&ty_value);
            StorageEntry {
                name,
                ty,
                slot: format!("0x{slot:02x}"),
                offset: 0,
                width_bytes: 32,
                encoding,
            }
        })
        .collect()
}

fn storage_type(value: &Value) -> (String, String) {
    match value.get("kind").and_then(Value::as_str) {
        Some("address") => ("address".to_string(), "value".to_string()),
        Some("uint256") => ("uint256".to_string(), "value".to_string()),
        Some("mapping") => (
            "mapping(address => uint256)".to_string(),
            "mapping".to_string(),
        ),
        Some(kind) => (kind.to_string(), kind.to_string()),
        None => ("uint256".to_string(), "value".to_string()),
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
        let metadata_line = parse_obligation_metadata(trimmed, &mut pending);
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

fn parse_obligation_metadata(line: &str, meta: &mut ObligationMeta) -> bool {
    let mut parsed = false;
    if let Some(raw) = line.strip_prefix("-- tama:") {
        parsed = true;
        apply_tama_metadata(raw, meta);
    }
    if line.contains("tama.") {
        parsed = true;
        apply_tama_attribute_metadata(line, meta);
    }
    parsed
}

fn apply_tama_metadata(raw: &str, meta: &mut ObligationMeta) {
    let values = parse_key_values(raw);
    if raw.contains("obligation") {
        meta.tagged = true;
    }
    if raw.contains("helper") || values.get("kind").is_some_and(|kind| kind == "helper") {
        meta.tagged = true;
        meta.kind = Some(ObligationKind::Helper);
    } else if raw.contains("invariant")
        || values.get("kind").is_some_and(|kind| kind == "invariant")
    {
        meta.tagged = true;
        meta.kind = Some(ObligationKind::Invariant);
    } else if raw.contains("postcondition")
        || values
            .get("kind")
            .is_some_and(|kind| kind == "postcondition")
    {
        meta.tagged = true;
        meta.kind = Some(ObligationKind::Postcondition);
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
        _ => {}
    }
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

fn extract_solc_bytecode(value: &Value) -> Option<(String, String)> {
    let contracts = value.get("contracts")?.as_object()?;
    for by_file in contracts.values() {
        if let Some(contract) = by_file.as_object()?.values().next() {
            let evm = contract.get("evm")?;
            let creation = evm.get("bytecode")?.get("object")?.as_str()?.to_string();
            let runtime = evm
                .get("deployedBytecode")
                .and_then(|bytecode| bytecode.get("object"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            return Some((creation, runtime));
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct AbiEntry {
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
    #[serde(default)]
    inputs: Vec<Param>,
    #[serde(default)]
    outputs: Vec<Param>,
    #[serde(rename = "stateMutability")]
    state_mutability: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canned_solc_error_is_detected() {
        let value = json!({
            "errors": [{"severity": "error", "message": "bad yul"}]
        });
        let errors = value
            .get("errors")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter(|entry| entry.get("severity").and_then(Value::as_str) == Some("error"))
            .count();
        assert_eq!(errors, 1);
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
    fn no_solc_implies_no_forge() {
        assert!(!should_run_forge(&BuildOptions {
            no_solc: true,
            no_forge: false,
            ..Default::default()
        }));
        assert!(should_run_forge(&BuildOptions::default()));
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

-- tama: obligation kind=postcondition function=increment coverage=mirror path=test/verity/Counter.t.sol:CounterTest.testIncrementPost
theorem increment_post : True := by
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
        assert_eq!(obligations.len(), 3);
        assert_eq!(obligations[0].id, "Counter.increment_post");
        assert_eq!(obligations[0].kind, ObligationKind::Postcondition);
        assert_eq!(obligations[0].function.as_deref(), Some("increment"));
        assert_eq!(
            obligations[0].coverage.disposition,
            CoverageDisposition::Mirror
        );
        assert_eq!(
            obligations[0].coverage.path.as_deref(),
            Some("test/verity/Counter.t.sol:CounterTest.testIncrementPost")
        );
        assert_eq!(obligations[1].kind, ObligationKind::Helper);
        assert_eq!(
            obligations[1].lean_decl,
            "proof.CounterProof.arithmetic_helper"
        );
        assert_eq!(obligations[2].kind, ObligationKind::Invariant);
        assert_eq!(
            obligations[2].coverage.disposition,
            CoverageDisposition::ProofOnly
        );
        assert_eq!(
            obligations[2].coverage.reason.as_deref(),
            Some("Symbolic state only.")
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
                evm_version: "cancun".to_string(),
                metadata_hash: "none".to_string(),
            },
            trust: tama_config::TrustConfig::default(),
        }
    }
}
