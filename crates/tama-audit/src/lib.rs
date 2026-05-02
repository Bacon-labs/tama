use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tama_manifest::{ContractManifest, CoverageDisposition, ObligationKind};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Common(#[from] tama_common::Error),
    #[error(transparent)]
    Config(#[from] tama_config::Error),
    #[error(transparent)]
    Manifest(#[from] tama_manifest::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub check: String,
    pub contract: Option<String>,
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub path: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Structure,
    Selectors,
    StorageLayout,
    Coverage,
    TrustBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditOptions {
    pub check: Option<Check>,
    pub deny_warnings: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReport {
    pub issues: Vec<Issue>,
}

impl AuditReport {
    pub fn has_failures(&self, deny_warnings: bool) -> bool {
        self.issues.iter().any(|issue| {
            issue.severity == Severity::Error
                || (deny_warnings && issue.severity == Severity::Warning)
        })
    }
}

pub fn run(root: &Utf8Path, opts: AuditOptions) -> Result<AuditReport> {
    let config = tama_config::load_config(root)?;
    let manifests = load_manifests(root, &config.paths.out.join("manifest"))?;
    let checks = match opts.check {
        Some(check) => vec![check],
        None => vec![
            Check::Structure,
            Check::Selectors,
            Check::StorageLayout,
            Check::Coverage,
            Check::TrustBoundary,
        ],
    };
    let mut issues = Vec::new();
    for check in checks {
        match check {
            Check::Structure => structure(root, &config, &manifests, &mut issues),
            Check::Selectors => selectors(root, &manifests, &mut issues),
            Check::StorageLayout => storage(&manifests, &mut issues),
            Check::Coverage => coverage(root, &manifests, &mut issues),
            Check::TrustBoundary => trust(root, &config, &manifests, &mut issues),
        }
    }
    Ok(AuditReport { issues })
}

pub fn parse_check(raw: &str) -> Option<Check> {
    match raw {
        "structure" => Some(Check::Structure),
        "selectors" => Some(Check::Selectors),
        "storage-layout" | "storage" => Some(Check::StorageLayout),
        "coverage" => Some(Check::Coverage),
        "trust-boundary" | "trust" => Some(Check::TrustBoundary),
        _ => None,
    }
}

fn load_manifests(root: &Utf8Path, manifest_dir: &Utf8Path) -> Result<Vec<ContractManifest>> {
    let dir = root.join(manifest_dir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut manifests = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|source| tama_common::io_error(dir.clone(), source))? {
        let entry = entry.map_err(|source| tama_common::io_error(dir.clone(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|path| tama_common::Error::NonUtf8Path(path.display().to_string()))?;
        if path.extension() == Some("json") {
            manifests.push(ContractManifest::load(&path)?);
        }
    }
    Ok(manifests)
}

fn structure(
    root: &Utf8Path,
    config: &tama_config::TamaConfig,
    manifests: &[ContractManifest],
    issues: &mut Vec<Issue>,
) {
    if manifests.is_empty() {
        issues.push(issue(
            "structure",
            None,
            "TAMA_STRUCTURE_NO_MANIFEST",
            "no contract manifests found",
            None,
        ));
    }
    for path in [
        &config.paths.src,
        &config.paths.spec,
        &config.paths.proof,
        &config.paths.test,
        &config.paths.generated,
        &config.paths.out,
    ] {
        if !root.join(path).is_dir() {
            issues.push(issue(
                "structure",
                None,
                "TAMA_STRUCTURE_MISSING_DIR",
                format!("configured directory is missing: {path}"),
                Some(path.clone()),
            ));
        }
    }
    match tama_config::parse_foundry_config(root) {
        Ok(foundry) if !config.paths.test.starts_with(&foundry.test) => {
            issues.push(issue(
                "structure",
                None,
                "TAMA_STRUCTURE_TEST_ROOT",
                format!(
                    "mirror test path `{}` is outside Foundry test directory `{}`",
                    config.paths.test, foundry.test
                ),
                Some(config.paths.test.clone()),
            ));
        }
        Ok(_) => {}
        Err(err) => issues.push(issue(
            "structure",
            None,
            "TAMA_STRUCTURE_FOUNDRY_CONFIG",
            format!("could not read foundry.toml: {err}"),
            Some("foundry.toml".into()),
        )),
    }
    for manifest in manifests {
        for path in [
            &manifest.source.implementation,
            &manifest.source.spec,
            &manifest.source.proof,
            &manifest.artifacts.yul,
            &manifest.artifacts.interface,
            &manifest.artifacts.deployer,
        ] {
            if !root.join(path).exists() {
                issues.push(issue(
                    "structure",
                    Some(&manifest.contract),
                    "TAMA_STRUCTURE_MISSING_FILE",
                    format!("required file is missing: {path}"),
                    Some(path.clone()),
                ));
            }
        }
        let test_path = config
            .paths
            .test
            .join(format!("{}.t.sol", manifest.contract));
        if !root.join(&test_path).is_file() {
            issues.push(issue(
                "structure",
                Some(&manifest.contract),
                "TAMA_STRUCTURE_MISSING_TEST",
                format!("mirror test file is missing: {test_path}"),
                Some(test_path),
            ));
        }
        for (aggregate, import) in [
            ("TamaSrc.lean", manifest.lean.implementation_module.as_str()),
            ("TamaSpec.lean", manifest.lean.spec_module.as_str()),
            ("TamaProof.lean", manifest.lean.proof_module.as_str()),
        ] {
            check_aggregate_import(root, manifest, aggregate, import, issues);
        }
        for path in [&manifest.artifacts.interface, &manifest.artifacts.deployer] {
            let abs = root.join(path);
            if abs.is_file() && !tama_common::has_generated_header(&abs).unwrap_or(false) {
                issues.push(issue(
                    "structure",
                    Some(&manifest.contract),
                    "TAMA_GENERATED_HEADER",
                    format!("generated bridge is missing Tama header: {path}"),
                    Some(path.clone()),
                ));
            }
        }
    }
}

fn check_aggregate_import(
    root: &Utf8Path,
    manifest: &ContractManifest,
    aggregate: &str,
    import: &str,
    issues: &mut Vec<Issue>,
) {
    let path = root.join(aggregate);
    let Ok(text) = tama_common::read_to_string(&path) else {
        issues.push(issue(
            "structure",
            Some(&manifest.contract),
            "TAMA_STRUCTURE_MISSING_AGGREGATE",
            format!("aggregate module is missing: {aggregate}"),
            Some(aggregate.into()),
        ));
        return;
    };
    let expected = format!("import {import}");
    if !text.lines().any(|line| line.trim() == expected) {
        issues.push(issue(
            "structure",
            Some(&manifest.contract),
            "TAMA_STRUCTURE_AGGREGATE_IMPORT",
            format!("{aggregate} does not import {import}"),
            Some(aggregate.into()),
        ));
    }
}

fn selectors(root: &Utf8Path, manifests: &[ContractManifest], issues: &mut Vec<Issue>) {
    for manifest in manifests {
        if let Err(err) = manifest.validate() {
            issues.push(issue(
                "selectors",
                Some(&manifest.contract),
                "TAMA_SELECTOR_INVALID",
                err.to_string(),
                None,
            ));
        }
        check_interface_abi(root, manifest, issues);
    }
}

fn check_interface_abi(root: &Utf8Path, manifest: &ContractManifest, issues: &mut Vec<Issue>) {
    let interface = root.join(&manifest.artifacts.interface);
    let Ok(text) = tama_common::read_to_string(&interface) else {
        issues.push(issue(
            "selectors",
            Some(&manifest.contract),
            "TAMA_SELECTOR_INTERFACE_MISSING",
            format!(
                "generated interface is missing: {}",
                manifest.artifacts.interface
            ),
            Some(manifest.artifacts.interface.clone()),
        ));
        return;
    };
    let functions = solidity_declarations(&text, "function");
    let events = solidity_declarations(&text, "event");
    let errors = solidity_declarations(&text, "error");
    for function in &manifest.abi.functions {
        if !functions.contains(&function.signature) {
            issues.push(issue(
                "selectors",
                Some(&manifest.contract),
                "TAMA_SELECTOR_INTERFACE_DRIFT",
                format!(
                    "generated interface does not declare function `{}`",
                    function.signature
                ),
                Some(manifest.artifacts.interface.clone()),
            ));
        }
    }
    for event in &manifest.abi.events {
        if !events.contains(&event.signature) {
            issues.push(issue(
                "selectors",
                Some(&manifest.contract),
                "TAMA_SELECTOR_INTERFACE_DRIFT",
                format!(
                    "generated interface does not declare event `{}`",
                    event.signature
                ),
                Some(manifest.artifacts.interface.clone()),
            ));
        }
    }
    for error in &manifest.abi.errors {
        if !errors.contains(&error.signature) {
            issues.push(issue(
                "selectors",
                Some(&manifest.contract),
                "TAMA_SELECTOR_INTERFACE_DRIFT",
                format!(
                    "generated interface does not declare error `{}`",
                    error.signature
                ),
                Some(manifest.artifacts.interface.clone()),
            ));
        }
    }
}

fn solidity_declarations(text: &str, kind: &str) -> BTreeSet<String> {
    let re = Regex::new(&format!(
        r"\b{}\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)",
        regex::escape(kind)
    ))
    .expect("valid regex");
    re.captures_iter(text)
        .map(|captures| {
            let name = captures.get(1).map(|m| m.as_str()).unwrap_or("");
            let params = captures
                .get(2)
                .map(|m| canonical_solidity_param_types(m.as_str()))
                .unwrap_or_default();
            format!("{name}({params})")
        })
        .collect()
}

fn canonical_solidity_param_types(params: &str) -> String {
    params
        .split(',')
        .map(str::trim)
        .filter(|param| !param.is_empty())
        .filter_map(|param| {
            let parts = param
                .split_whitespace()
                .filter(|part| !matches!(*part, "indexed" | "memory" | "calldata" | "storage"))
                .collect::<Vec<_>>();
            parts.first().copied()
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn storage(manifests: &[ContractManifest], issues: &mut Vec<Issue>) {
    for manifest in manifests {
        let mut fixed = BTreeMap::<String, String>::new();
        for entry in &manifest.storage {
            if entry.encoding != "mapping" {
                let key = format!("{}:{}", entry.slot, entry.offset);
                if let Some(prev) = fixed.insert(key, entry.name.clone()) {
                    issues.push(issue(
                        "storage-layout",
                        Some(&manifest.contract),
                        "TAMA_STORAGE_DUPLICATE",
                        format!("storage entries `{prev}` and `{}` overlap", entry.name),
                        None,
                    ));
                }
            }
            if entry.offset >= 32 || entry.width_bytes == 0 || entry.width_bytes > 32 {
                issues.push(issue(
                    "storage-layout",
                    Some(&manifest.contract),
                    "TAMA_STORAGE_WIDTH",
                    format!("storage entry `{}` has invalid offset/width", entry.name),
                    None,
                ));
            }
        }
    }
}

fn coverage(root: &Utf8Path, manifests: &[ContractManifest], issues: &mut Vec<Issue>) {
    for manifest in manifests {
        for obligation in &manifest.obligations {
            match obligation.kind {
                ObligationKind::Helper => continue,
                ObligationKind::Invariant | ObligationKind::Postcondition => {}
            }
            match obligation.coverage.disposition {
                CoverageDisposition::Mirror => {
                    let Some(path_ref) = &obligation.coverage.path else {
                        issues.push(issue(
                            "coverage",
                            Some(&manifest.contract),
                            "TAMA_COVERAGE_MISSING",
                            format!("{} has no mirror path", obligation.id),
                            None,
                        ));
                        continue;
                    };
                    let file = path_ref
                        .split_once(':')
                        .map(|(file, _)| file)
                        .unwrap_or(path_ref);
                    let abs = root.join(file);
                    if !abs.is_file() {
                        issues.push(issue(
                            "coverage",
                            Some(&manifest.contract),
                            "TAMA_COVERAGE_MISSING_FILE",
                            format!("mirror file does not exist: {file}"),
                            Some(file.into()),
                        ));
                        continue;
                    }
                    if let Some((_, symbol)) = path_ref.split_once(':') {
                        let text = tama_common::read_to_string(&abs).unwrap_or_default();
                        let name = symbol.rsplit('.').next().unwrap_or(symbol);
                        let re = Regex::new(&format!(r"\b{}\b", regex::escape(name)))
                            .expect("valid regex");
                        if !re.is_match(&text) {
                            issues.push(issue(
                                "coverage",
                                Some(&manifest.contract),
                                "TAMA_COVERAGE_MISSING_SYMBOL",
                                format!("mirror symbol `{symbol}` not found"),
                                Some(file.into()),
                            ));
                        }
                    }
                }
                CoverageDisposition::ProofOnly => {
                    if obligation
                        .coverage
                        .reason
                        .as_deref()
                        .unwrap_or("")
                        .trim()
                        .is_empty()
                    {
                        issues.push(issue(
                            "coverage",
                            Some(&manifest.contract),
                            "TAMA_COVERAGE_REASON",
                            format!("{} proof_only coverage requires a reason", obligation.id),
                            None,
                        ));
                    }
                }
                CoverageDisposition::None => {
                    issues.push(issue(
                        "coverage",
                        Some(&manifest.contract),
                        "TAMA_COVERAGE_NONE",
                        format!("{} has no coverage disposition", obligation.id),
                        None,
                    ));
                }
            }
        }
    }
}

fn trust(
    root: &Utf8Path,
    config: &tama_config::TamaConfig,
    manifests: &[ContractManifest],
    issues: &mut Vec<Issue>,
) {
    let deny = BTreeSet::from(["sorryAx".to_string()]);
    let allow = &config.trust.allow_axioms;
    let probe = root.join(config.paths.out.join("trust-probe").join("axioms.json"));
    let public_obligations = manifests
        .iter()
        .flat_map(|manifest| {
            manifest
                .obligations
                .iter()
                .filter(|obligation| obligation.kind != ObligationKind::Helper)
                .map(move |obligation| (manifest, obligation))
        })
        .collect::<Vec<_>>();
    let mut seen_decls = BTreeSet::new();
    if probe.is_file() {
        let text = tama_common::read_to_string(&probe).unwrap_or_default();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        if let Some(obligations) = value.get("obligations").and_then(|value| value.as_array()) {
            for obligation in obligations {
                let decl = obligation
                    .get("lean_decl")
                    .and_then(|value| value.as_str())
                    .unwrap_or("<unknown>");
                seen_decls.insert(decl.to_string());
                for axiom in obligation
                    .get("axioms")
                    .and_then(|value| value.as_array())
                    .into_iter()
                    .flatten()
                {
                    let axiom = axiom.as_str().unwrap_or("");
                    if deny.contains(axiom) || !allow.contains_key(axiom) {
                        issues.push(issue(
                            "trust-boundary",
                            None,
                            "TAMA_TRUST_AXIOM",
                            format!("{decl} depends on unallowlisted axiom `{axiom}`"),
                            Some(probe.clone()),
                        ));
                    }
                }
            }
        }
    } else if !public_obligations.is_empty() {
        issues.push(issue(
            "trust-boundary",
            None,
            "TAMA_TRUST_PROBE_MISSING",
            format!(
                "trust probe output is missing: {}",
                probe.strip_prefix(root).unwrap_or(&probe)
            ),
            Some(probe.strip_prefix(root).unwrap_or(&probe).to_owned()),
        ));
    }
    for (manifest, obligation) in public_obligations {
        if !obligation.lean_decl.contains('.') {
            issues.push(issue(
                "trust-boundary",
                Some(&manifest.contract),
                "TAMA_TRUST_DECL",
                format!("{} is not fully qualified", obligation.lean_decl),
                None,
            ));
        }
        if probe.is_file() && !seen_decls.contains(&obligation.lean_decl) {
            issues.push(issue(
                "trust-boundary",
                Some(&manifest.contract),
                "TAMA_TRUST_DECL_MISSING",
                format!(
                    "{} was not reported by the trust probe",
                    obligation.lean_decl
                ),
                Some(probe.strip_prefix(root).unwrap_or(&probe).to_owned()),
            ));
        }
    }
}

fn issue(
    check: &str,
    contract: Option<&str>,
    code: &str,
    message: impl Into<String>,
    path: Option<Utf8PathBuf>,
) -> Issue {
    Issue {
        check: check.to_string(),
        contract: contract.map(str::to_string),
        severity: Severity::Error,
        code: code.to_string(),
        message: message.into(),
        path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_check_name() {
        assert_eq!(parse_check("selectors"), Some(Check::Selectors));
        assert_eq!(parse_check("nope"), None);
    }

    #[test]
    fn report_failure_policy_respects_warnings() {
        let report = AuditReport {
            issues: vec![Issue {
                check: "x".to_string(),
                contract: None,
                severity: Severity::Warning,
                code: "W".to_string(),
                message: "warn".to_string(),
                path: None,
            }],
        };
        assert!(!report.has_failures(false));
        assert!(report.has_failures(true));
    }

    #[test]
    fn structure_reports_missing_test_and_aggregate_import() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        for dir in [
            &config.paths.src,
            &config.paths.spec,
            &config.paths.proof,
            &config.paths.test,
            &config.paths.generated,
            &config.paths.out,
            &config.paths.out.join("yul"),
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        tama_common::write_string(&root.join("verity/src/Counter.lean"), "").unwrap();
        tama_common::write_string(&root.join("verity/spec/CounterSpec.lean"), "").unwrap();
        tama_common::write_string(&root.join("verity/proof/CounterProof.lean"), "").unwrap();
        tama_common::write_string(&root.join("artifacts/yul/Counter.yul"), "").unwrap();
        tama_common::write_generated(&root.join("src/generated/verity/CounterIface.sol"), "")
            .unwrap();
        tama_common::write_generated(&root.join("src/generated/verity/CounterDeployer.sol"), "")
            .unwrap();
        tama_common::write_string(&root.join("TamaSrc.lean"), "import src.Other\n").unwrap();
        tama_common::write_string(&root.join("TamaSpec.lean"), "import spec.CounterSpec\n")
            .unwrap();
        tama_common::write_string(&root.join("TamaProof.lean"), "import proof.CounterProof\n")
            .unwrap();

        let mut issues = Vec::new();
        structure(&root, &config, &[counter_manifest()], &mut issues);
        let codes = issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<BTreeSet<_>>();
        assert!(codes.contains("TAMA_STRUCTURE_MISSING_TEST"));
        assert!(codes.contains("TAMA_STRUCTURE_AGGREGATE_IMPORT"));
    }

    #[test]
    fn trust_fails_when_probe_is_missing_for_public_obligation() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        let mut manifest = counter_manifest();
        manifest.obligations.push(public_obligation());
        let mut issues = Vec::new();
        trust(&root, &config, &[manifest], &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.code == "TAMA_TRUST_PROBE_MISSING"));
    }

    #[test]
    fn trust_fails_when_probe_omits_public_obligation() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        std::fs::create_dir_all(root.join("artifacts/trust-probe")).unwrap();
        tama_common::write_string(
            &root.join("artifacts/trust-probe/axioms.json"),
            r#"{"obligations":[]}"#,
        )
        .unwrap();
        let mut manifest = counter_manifest();
        manifest.obligations.push(public_obligation());
        let mut issues = Vec::new();
        trust(&root, &config, &[manifest], &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.code == "TAMA_TRUST_DECL_MISSING"));
    }

    #[test]
    fn selectors_report_generated_interface_drift() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("src/generated/verity")).unwrap();
        tama_common::write_generated(
            &root.join("src/generated/verity/CounterIface.sol"),
            r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

interface CounterIface {
    function wrong() external;
}
"#,
        )
        .unwrap();
        let mut manifest = counter_manifest();
        manifest.abi.functions.push(tama_manifest::Function {
            name: "getCount".to_string(),
            signature: "getCount()".to_string(),
            selector: tama_common::function_selector("getCount()"),
            visibility: "external".to_string(),
            mutability: "view".to_string(),
            inputs: vec![],
            outputs: vec![tama_manifest::Param {
                name: "".to_string(),
                ty: "uint256".to_string(),
            }],
        });
        let mut issues = Vec::new();
        selectors(&root, &[manifest], &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.code == "TAMA_SELECTOR_INTERFACE_DRIFT"));
    }

    #[test]
    fn solidity_declaration_parser_ignores_names_and_location_modifiers() {
        let text = r#"
interface Example {
    function set(uint256 amount, bytes32[] calldata proof) external;
    event Transfer(address indexed from, address indexed to, uint256 amount);
    error Bad(address account);
}
"#;
        assert!(solidity_declarations(text, "function").contains("set(uint256,bytes32[])"));
        assert!(solidity_declarations(text, "event").contains("Transfer(address,address,uint256)"));
        assert!(solidity_declarations(text, "error").contains("Bad(address)"));
    }

    fn test_config() -> tama_config::TamaConfig {
        tama_config::TamaConfig {
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

    fn public_obligation() -> tama_manifest::Obligation {
        tama_manifest::Obligation {
            id: "Counter.increment_post".to_string(),
            name: "increment_post".to_string(),
            kind: ObligationKind::Postcondition,
            lean_decl: "proof.CounterProof.increment_post".to_string(),
            contract: "Counter".to_string(),
            function: Some("increment".to_string()),
            coverage: tama_manifest::Coverage {
                disposition: CoverageDisposition::ProofOnly,
                path: None,
                reason: Some("symbolic only".to_string()),
            },
        }
    }

    fn counter_manifest() -> ContractManifest {
        ContractManifest {
            schema: tama_manifest::SCHEMA.to_string(),
            contract: "Counter".to_string(),
            source: tama_manifest::SourcePaths {
                implementation: "verity/src/Counter.lean".into(),
                spec: "verity/spec/CounterSpec.lean".into(),
                proof: "verity/proof/CounterProof.lean".into(),
            },
            lean: tama_manifest::LeanModules {
                implementation_module: "src.Counter".to_string(),
                spec_module: "spec.CounterSpec".to_string(),
                proof_module: "proof.CounterProof".to_string(),
            },
            abi: tama_manifest::Abi::default(),
            storage: vec![],
            obligations: vec![],
            artifacts: tama_manifest::ArtifactPaths {
                yul: "artifacts/yul/Counter.yul".into(),
                creation_bytecode: "artifacts/bytecode/Counter.bin".into(),
                runtime_bytecode: "artifacts/bytecode/Counter.runtime.bin".into(),
                bytecode_hash: None,
                solc_input: "artifacts/solc-json/Counter.input.json".into(),
                solc_output: "artifacts/solc-json/Counter.output.json".into(),
                interface: "src/generated/verity/CounterIface.sol".into(),
                deployer: "src/generated/verity/CounterDeployer.sol".into(),
            },
        }
    }
}
