use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tama_manifest::{ContractManifest, CoverageDisposition, ObligationKind, SCHEMA};

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

struct TrustContext<'a> {
    root: &'a Utf8Path,
    path: &'a Utf8Path,
    deny: &'a BTreeSet<String>,
    allow: &'a BTreeMap<String, String>,
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
            manifests.push(ContractManifest::load_unvalidated(&path)?);
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
        if manifest.schema != SCHEMA {
            issues.push(issue(
                "structure",
                Some(&manifest.contract),
                "TAMA_STRUCTURE_MANIFEST_SCHEMA",
                format!("unsupported manifest schema `{}`", manifest.schema),
                None,
            ));
        }
        for path in [
            &manifest.source.implementation,
            &manifest.source.spec,
            &manifest.source.proof,
            &manifest.artifacts.yul,
            &manifest.artifacts.creation_bytecode,
            &manifest.artifacts.runtime_bytecode,
            &manifest.artifacts.solc_input,
            &manifest.artifacts.solc_output,
            &manifest.artifacts.interface,
            &manifest.artifacts.deployer,
        ] {
            if path_escapes_project(path) {
                issues.push(issue(
                    "structure",
                    Some(&manifest.contract),
                    "TAMA_STRUCTURE_MANIFEST_PATH",
                    format!("manifest path escapes project root: {path}"),
                    Some(path.clone()),
                ));
                continue;
            }
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
        check_bytecode_hash(root, manifest, issues);
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
            if path_escapes_project(path) {
                continue;
            }
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

fn check_bytecode_hash(root: &Utf8Path, manifest: &ContractManifest, issues: &mut Vec<Issue>) {
    if path_escapes_project(&manifest.artifacts.creation_bytecode) {
        return;
    }
    let path = root.join(&manifest.artifacts.creation_bytecode);
    if !path.is_file() {
        return;
    }
    let Some(expected) = &manifest.artifacts.bytecode_hash else {
        issues.push(issue(
            "structure",
            Some(&manifest.contract),
            "TAMA_STRUCTURE_BYTECODE_HASH",
            format!(
                "manifest is missing bytecode_hash for {}",
                manifest.artifacts.creation_bytecode
            ),
            Some(manifest.artifacts.creation_bytecode.clone()),
        ));
        return;
    };
    match tama_common::sha256_file(&path) {
        Ok(actual) if actual == *expected => {}
        Ok(actual) => issues.push(issue(
            "structure",
            Some(&manifest.contract),
            "TAMA_STRUCTURE_BYTECODE_HASH",
            format!(
                "bytecode_hash mismatch for {}: manifest has {expected}, file has {actual}",
                manifest.artifacts.creation_bytecode
            ),
            Some(manifest.artifacts.creation_bytecode.clone()),
        )),
        Err(err) => issues.push(issue(
            "structure",
            Some(&manifest.contract),
            "TAMA_STRUCTURE_BYTECODE_HASH",
            format!(
                "could not hash bytecode file {}: {err}",
                manifest.artifacts.creation_bytecode
            ),
            Some(manifest.artifacts.creation_bytecode.clone()),
        )),
    }
}

fn path_escapes_project(path: &Utf8Path) -> bool {
    path.is_absolute() || path.components().any(|part| part.as_str() == "..")
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
    let text = strip_solidity_non_code(text);
    let re = Regex::new(&format!(
        r"\b{}\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)",
        regex::escape(kind)
    ))
    .expect("valid regex");
    re.captures_iter(&text)
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

fn solidity_function_declared(text: &str, name: &str) -> bool {
    let text = strip_solidity_non_code(text);
    let re =
        Regex::new(&format!(r"\bfunction\s+{}\s*\(", regex::escape(name))).expect("valid regex");
    re.is_match(&text)
}

fn mirror_symbol_is_property(name: &str) -> bool {
    name.starts_with("testFuzz") || name.starts_with("invariant_")
}

fn strip_solidity_non_code(text: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Code,
        LineComment,
        BlockComment,
        String { quote: char, escaped: bool },
    }

    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut state = State::Code;
    while let Some(ch) = chars.next() {
        match state {
            State::Code => match ch {
                '/' if chars.peek() == Some(&'/') => {
                    chars.next();
                    out.push(' ');
                    out.push(' ');
                    state = State::LineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    out.push(' ');
                    out.push(' ');
                    state = State::BlockComment;
                }
                '"' | '\'' => {
                    out.push(' ');
                    state = State::String {
                        quote: ch,
                        escaped: false,
                    };
                }
                _ => out.push(ch),
            },
            State::LineComment => {
                if ch == '\n' {
                    out.push('\n');
                    state = State::Code;
                } else {
                    out.push(' ');
                }
            }
            State::BlockComment => {
                if ch == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    out.push(' ');
                    out.push(' ');
                    state = State::Code;
                } else if ch == '\n' {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
            }
            State::String { quote, escaped } => {
                if ch == '\n' {
                    out.push('\n');
                    state = State::Code;
                } else {
                    out.push(' ');
                    if escaped {
                        state = State::String {
                            quote,
                            escaped: false,
                        };
                    } else if ch == '\\' {
                        state = State::String {
                            quote,
                            escaped: true,
                        };
                    } else if ch == quote {
                        state = State::Code;
                    }
                }
            }
        }
    }
    out
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
        let mut fixed = Vec::<(&tama_manifest::StorageEntry, u32)>::new();
        for entry in &manifest.storage {
            if !valid_storage_slot(&entry.slot) {
                issues.push(issue(
                    "storage-layout",
                    Some(&manifest.contract),
                    "TAMA_STORAGE_SLOT",
                    format!(
                        "storage entry `{}` has invalid slot `{}`",
                        entry.name, entry.slot
                    ),
                    None,
                ));
            }
            if !valid_storage_encoding(&entry.encoding) {
                issues.push(issue(
                    "storage-layout",
                    Some(&manifest.contract),
                    "TAMA_STORAGE_ENCODING",
                    format!(
                        "storage entry `{}` has unsupported encoding `{}`",
                        entry.name, entry.encoding
                    ),
                    None,
                ));
            }
            if entry.encoding != "mapping" {
                let end = entry.offset.saturating_add(entry.width_bytes);
                for (prev, prev_end) in &fixed {
                    if prev.slot == entry.slot && entry.offset < *prev_end && prev.offset < end {
                        issues.push(issue(
                            "storage-layout",
                            Some(&manifest.contract),
                            "TAMA_STORAGE_DUPLICATE",
                            format!(
                                "storage entries `{}` and `{}` overlap",
                                prev.name, entry.name
                            ),
                            None,
                        ));
                    }
                }
                fixed.push((entry, end));
            }
            if entry.offset >= 32
                || entry.width_bytes == 0
                || entry.width_bytes > 32
                || entry.offset.saturating_add(entry.width_bytes) > 32
            {
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

fn valid_storage_slot(slot: &str) -> bool {
    let Some(rest) = slot.strip_prefix("0x") else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 64
        && rest.chars().all(|ch| ch.is_ascii_hexdigit())
        && !rest.chars().any(|ch| ch.is_ascii_uppercase())
}

fn valid_storage_encoding(encoding: &str) -> bool {
    matches!(
        encoding,
        "value" | "mapping" | "dynamic_array" | "bytes" | "struct"
    )
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
                    if path_escapes_project(Utf8Path::new(file)) {
                        issues.push(issue(
                            "coverage",
                            Some(&manifest.contract),
                            "TAMA_COVERAGE_PATH",
                            format!("mirror file path escapes project root: {file}"),
                            Some(file.into()),
                        ));
                        continue;
                    }
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
                        if !mirror_symbol_is_property(name) {
                            issues.push(issue(
                                "coverage",
                                Some(&manifest.contract),
                                "TAMA_COVERAGE_SHAPE",
                                format!(
                                    "mirror symbol `{symbol}` must be a fuzz test or invariant"
                                ),
                                Some(file.into()),
                            ));
                        }
                        if !solidity_function_declared(&text, name) {
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
    let trust_report = root.join(config.paths.out.join("trust-report.json"));
    let assumption_report = root.join(config.paths.out.join("assumption-report.json"));
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
        if let Some(value) = read_trust_json(
            root,
            &probe,
            "TAMA_TRUST_PROBE_INVALID",
            "trust probe",
            issues,
        ) {
            audit_axiom_probe(root, &probe, &value, &deny, allow, &mut seen_decls, issues);
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
    let assumption_report_is_valid = if assumption_report.is_file() {
        read_trust_json(
            root,
            &assumption_report,
            "TAMA_TRUST_ASSUMPTION_REPORT_INVALID",
            "assumption report",
            issues,
        )
        .map(|value| {
            audit_assumption_report(root, &assumption_report, &value, &deny, allow, issues);
        })
        .is_some()
    } else {
        false
    };
    if trust_report.is_file() {
        if let Some(value) = read_trust_json(
            root,
            &trust_report,
            "TAMA_TRUST_REPORT_INVALID",
            "trust report",
            issues,
        ) {
            audit_verity_trust_report(
                root,
                &trust_report,
                &value,
                !assumption_report_is_valid,
                &deny,
                allow,
                issues,
            );
        }
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

fn audit_axiom_probe(
    root: &Utf8Path,
    path: &Utf8Path,
    value: &serde_json::Value,
    deny: &BTreeSet<String>,
    allow: &BTreeMap<String, String>,
    seen_decls: &mut BTreeSet<String>,
    issues: &mut Vec<Issue>,
) {
    let Some(obligations) = value.get("obligations").and_then(|value| value.as_array()) else {
        issues.push(issue(
            "trust-boundary",
            None,
            "TAMA_TRUST_PROBE_INVALID",
            "trust probe is missing `obligations[]`",
            Some(relative_path(root, path)),
        ));
        return;
    };
    for obligation in obligations {
        let decl = obligation
            .get("lean_decl")
            .and_then(|value| value.as_str())
            .unwrap_or("<unknown>");
        seen_decls.insert(decl.to_string());
        let Some(axioms) = obligation.get("axioms").and_then(|value| value.as_array()) else {
            issues.push(issue(
                "trust-boundary",
                None,
                "TAMA_TRUST_PROBE_INVALID",
                format!("{decl} trust probe entry is missing `axioms[]`"),
                Some(relative_path(root, path)),
            ));
            continue;
        };
        for axiom in axioms {
            let Some(axiom) = axiom.as_str() else {
                issues.push(issue(
                    "trust-boundary",
                    None,
                    "TAMA_TRUST_PROBE_INVALID",
                    format!("{decl} trust probe entry contains a non-string axiom"),
                    Some(relative_path(root, path)),
                ));
                continue;
            };
            if deny.contains(axiom) || !allow.contains_key(axiom) {
                issues.push(issue(
                    "trust-boundary",
                    None,
                    "TAMA_TRUST_AXIOM",
                    format!("{decl} depends on unallowlisted axiom `{axiom}`"),
                    Some(relative_path(root, path)),
                ));
            }
        }
    }
}

fn audit_verity_trust_report(
    root: &Utf8Path,
    path: &Utf8Path,
    value: &serde_json::Value,
    audit_assumptions: bool,
    deny: &BTreeSet<String>,
    allow: &BTreeMap<String, String>,
    issues: &mut Vec<Issue>,
) {
    let Some(contracts) = value.get("contracts").and_then(|value| value.as_array()) else {
        issues.push(issue(
            "trust-boundary",
            None,
            "TAMA_TRUST_REPORT_INVALID",
            "trust report is missing `contracts[]`",
            Some(relative_path(root, path)),
        ));
        return;
    };
    for contract in contracts {
        let Some(name) = contract.get("contract").and_then(|value| value.as_str()) else {
            issues.push(issue(
                "trust-boundary",
                None,
                "TAMA_TRUST_REPORT_INVALID",
                "trust report contract entry is missing `contract`",
                Some(relative_path(root, path)),
            ));
            continue;
        };
        audit_contract_trust_surfaces(root, path, contract, name, issues);
        audit_contract_unchecked_dependencies(root, path, contract, name, issues);
        if audit_assumptions {
            audit_contract_external_assumptions(root, path, contract, name, deny, allow, issues);
        }
    }
}

fn audit_contract_trust_surfaces(
    root: &Utf8Path,
    path: &Utf8Path,
    contract: &serde_json::Value,
    contract_name: &str,
    issues: &mut Vec<Issue>,
) {
    let surface_fields = [
        ("modeledLowLevelMechanics", "low-level mechanics"),
        ("notModeledEventEmission", "not-modeled event emission"),
        (
            "notModeledProxyUpgradeability",
            "proxy/delegatecall upgradeability",
        ),
        (
            "partiallyModeledLinearMemoryMechanics",
            "partially modeled linear-memory mechanics",
        ),
        (
            "partiallyModeledRuntimeIntrospection",
            "partially modeled runtime introspection",
        ),
        ("unsafeBlocks", "unsafe blocks"),
    ];
    let mut contract_surfaces = Vec::new();
    for (field, label) in surface_fields {
        let Some(values) = contract.get(field).and_then(|value| value.as_array()) else {
            issues.push(issue(
                "trust-boundary",
                Some(contract_name),
                "TAMA_TRUST_REPORT_INVALID",
                format!("{contract_name} trust report is missing `{field}[]`"),
                Some(relative_path(root, path)),
            ));
            continue;
        };
        if values.iter().any(|value| !value.is_string()) {
            issues.push(issue(
                "trust-boundary",
                Some(contract_name),
                "TAMA_TRUST_REPORT_INVALID",
                format!("{contract_name} trust report `{field}[]` contains a non-string value"),
                Some(relative_path(root, path)),
            ));
            continue;
        }
        let values = values
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        if !values.is_empty() {
            contract_surfaces.push((label, values.join(", ")));
        }
    }
    let Some(sites) = contract
        .get("usageSites")
        .and_then(|value| value.as_array())
    else {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_REPORT_INVALID",
            format!("{contract_name} trust report is missing `usageSites[]`"),
            Some(relative_path(root, path)),
        ));
        return;
    };
    if sites.is_empty() {
        for (label, values) in contract_surfaces {
            issues.push(issue(
                "trust-boundary",
                Some(contract_name),
                "TAMA_TRUST_SURFACE",
                format!("{contract_name} uses {label}: {values}"),
                Some(relative_path(root, path)),
            ));
        }
    }
    for site in sites {
        let kind = site
            .get("kind")
            .and_then(|value| value.as_str())
            .unwrap_or("site");
        let name = site
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("<unknown>");
        for (field, label) in surface_fields {
            let Some(values) = site.get(field).and_then(|value| value.as_array()) else {
                issues.push(issue(
                    "trust-boundary",
                    Some(contract_name),
                    "TAMA_TRUST_REPORT_INVALID",
                    format!("{contract_name} [{kind}:{name}] trust site is missing `{field}[]`"),
                    Some(relative_path(root, path)),
                ));
                continue;
            };
            if values.iter().any(|value| !value.is_string()) {
                issues.push(issue(
                    "trust-boundary",
                    Some(contract_name),
                    "TAMA_TRUST_REPORT_INVALID",
                    format!(
                        "{contract_name} [{kind}:{name}] trust site `{field}[]` contains a non-string value"
                    ),
                    Some(relative_path(root, path)),
                ));
                continue;
            }
            let values = values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>();
            if !values.is_empty() {
                issues.push(issue(
                    "trust-boundary",
                    Some(contract_name),
                    "TAMA_TRUST_SURFACE",
                    format!(
                        "{contract_name} [{kind}:{name}] uses {label}: {}",
                        values.join(", ")
                    ),
                    Some(relative_path(root, path)),
                ));
            }
        }
    }
}

fn audit_contract_unchecked_dependencies(
    root: &Utf8Path,
    path: &Utf8Path,
    contract: &serde_json::Value,
    contract_name: &str,
    issues: &mut Vec<Issue>,
) {
    let Some(has_unchecked) = contract
        .get("hasUncheckedDependencies")
        .and_then(|value| value.as_bool())
    else {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_REPORT_INVALID",
            format!("{contract_name} trust report is missing `hasUncheckedDependencies`"),
            Some(relative_path(root, path)),
        ));
        return;
    };
    if !has_unchecked {
        return;
    }
    let unchecked = contract
        .get("proofStatus")
        .and_then(|value| value.get("unchecked"))
        .map(describe_status_bucket)
        .unwrap_or_else(|| "unchecked dependencies".to_string());
    issues.push(issue(
        "trust-boundary",
        Some(contract_name),
        "TAMA_TRUST_UNCHECKED",
        format!("{contract_name} has unchecked trust dependencies: {unchecked}"),
        Some(relative_path(root, path)),
    ));
}

fn audit_contract_external_assumptions(
    root: &Utf8Path,
    path: &Utf8Path,
    contract: &serde_json::Value,
    contract_name: &str,
    deny: &BTreeSet<String>,
    allow: &BTreeMap<String, String>,
    issues: &mut Vec<Issue>,
) {
    let ctx = TrustContext {
        root,
        path,
        deny,
        allow,
    };
    let Some(external) = contract.get("externalAssumptions") else {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_REPORT_INVALID",
            format!("{contract_name} trust report is missing `externalAssumptions`"),
            Some(relative_path(root, path)),
        ));
        return;
    };
    let Some(local_obligations) = contract
        .get("localObligations")
        .and_then(|value| value.as_array())
    else {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_REPORT_INVALID",
            format!("{contract_name} trust report is missing `localObligations[]`"),
            Some(relative_path(root, path)),
        ));
        return;
    };
    for obligation in local_obligations {
        let name = obligation
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("<unknown>");
        let status = obligation
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("<unknown>");
        if status != "proved" {
            issues.push(issue(
                "trust-boundary",
                Some(contract_name),
                "TAMA_TRUST_LOCAL_OBLIGATION",
                format!("{contract_name} has undischarged local obligation `{name}` ({status})"),
                Some(relative_path(root, path)),
            ));
        }
    }
    let Some(primitives) = external
        .get("axiomatizedPrimitives")
        .and_then(|value| value.as_array())
    else {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_REPORT_INVALID",
            format!("{contract_name} trust report is missing `externalAssumptions.axiomatizedPrimitives[]`"),
            Some(relative_path(root, path)),
        ));
        return;
    };
    for primitive in primitives {
        let assumption = primitive
            .get("assumption")
            .and_then(|value| value.as_str())
            .or_else(|| primitive.get("primitive").and_then(|value| value.as_str()))
            .unwrap_or("");
        check_allowed_assumption(
            &ctx,
            contract_name,
            assumption,
            "axiomatized primitive",
            issues,
        );
    }
    let Some(externals) = external
        .get("linkedExternals")
        .and_then(|value| value.as_array())
    else {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_REPORT_INVALID",
            format!(
                "{contract_name} trust report is missing `externalAssumptions.linkedExternals[]`"
            ),
            Some(relative_path(root, path)),
        ));
        return;
    };
    for entry in externals {
        let name = entry
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("<unknown>");
        check_axiom_array(&ctx, contract_name, entry, name, "linked external", issues);
    }
    let Some(ecm_axioms) = external.get("ecmAxioms").and_then(|value| value.as_array()) else {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_REPORT_INVALID",
            format!("{contract_name} trust report is missing `externalAssumptions.ecmAxioms[]`"),
            Some(relative_path(root, path)),
        ));
        return;
    };
    for entry in ecm_axioms {
        let assumption = entry
            .get("assumption")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let module = entry
            .get("module")
            .and_then(|value| value.as_str())
            .unwrap_or("<unknown>");
        check_allowed_assumption(
            &ctx,
            contract_name,
            assumption,
            &format!("ECM module `{module}`"),
            issues,
        );
    }
    let Some(modules) = external
        .get("ecmModules")
        .and_then(|value| value.as_array())
    else {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_REPORT_INVALID",
            format!("{contract_name} trust report is missing `externalAssumptions.ecmModules[]`"),
            Some(relative_path(root, path)),
        ));
        return;
    };
    for entry in modules {
        let module = entry
            .get("module")
            .and_then(|value| value.as_str())
            .unwrap_or("<unknown>");
        if entry
            .get("axioms")
            .and_then(|value| value.as_array())
            .is_some_and(|axioms| axioms.is_empty())
            && entry.get("status").and_then(|value| value.as_str()) != Some("proved")
        {
            issues.push(issue(
                "trust-boundary",
                Some(contract_name),
                "TAMA_TRUST_ASSUMPTION",
                format!(
                    "{contract_name} ECM module `{module}` has an undischarged assumption without a declared allowlist identifier"
                ),
                Some(relative_path(root, path)),
            ));
        }
    }
}

fn audit_assumption_report(
    root: &Utf8Path,
    path: &Utf8Path,
    value: &serde_json::Value,
    deny: &BTreeSet<String>,
    allow: &BTreeMap<String, String>,
    issues: &mut Vec<Issue>,
) {
    let ctx = TrustContext {
        root,
        path,
        deny,
        allow,
    };
    let Some(contracts) = value.get("contracts").and_then(|value| value.as_array()) else {
        issues.push(issue(
            "trust-boundary",
            None,
            "TAMA_TRUST_ASSUMPTION_REPORT_INVALID",
            "assumption report is missing `contracts[]`",
            Some(relative_path(root, path)),
        ));
        return;
    };
    for contract in contracts {
        let Some(contract_name) = contract.get("contract").and_then(|value| value.as_str()) else {
            issues.push(issue(
                "trust-boundary",
                None,
                "TAMA_TRUST_ASSUMPTION_REPORT_INVALID",
                "assumption report contract entry is missing `contract`",
                Some(relative_path(root, path)),
            ));
            continue;
        };
        let Some(undischarged) = contract
            .get("undischarged")
            .and_then(|value| value.as_array())
        else {
            issues.push(issue(
                "trust-boundary",
                Some(contract_name),
                "TAMA_TRUST_ASSUMPTION_REPORT_INVALID",
                format!("{contract_name} assumption report is missing `undischarged[]`"),
                Some(relative_path(root, path)),
            ));
            continue;
        };
        for entry in undischarged {
            audit_assumption_entry(&ctx, contract_name, entry, issues);
        }
    }
}

fn audit_assumption_entry(
    ctx: &TrustContext<'_>,
    contract_name: &str,
    entry: &serde_json::Value,
    issues: &mut Vec<Issue>,
) {
    let category = entry
        .get("category")
        .and_then(|value| value.as_str())
        .unwrap_or("<unknown>");
    let name = entry
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("<unknown>");
    let status = entry
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("<unknown>");
    if status == "unchecked" {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_UNCHECKED",
            format!("{contract_name} has unchecked {category} `{name}`"),
            Some(relative_path(ctx.root, ctx.path)),
        ));
    }
    match category {
        "axiomatizedPrimitive" => {
            let assumption = entry
                .get("assumption")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .unwrap_or(name);
            check_allowed_assumption(
                ctx,
                contract_name,
                assumption,
                "axiomatized primitive",
                issues,
            );
        }
        "linkedExternal" => {
            check_axiom_array(ctx, contract_name, entry, name, "linked external", issues)
        }
        "ecmAxiom" => check_allowed_assumption(ctx, contract_name, name, "ECM axiom", issues),
        "localObligation" => issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_LOCAL_OBLIGATION",
            format!("{contract_name} has undischarged local obligation `{name}` ({status})"),
            Some(relative_path(ctx.root, ctx.path)),
        )),
        "ecmModule" => {
            if entry
                .get("axioms")
                .and_then(|value| value.as_array())
                .is_some_and(|axioms| axioms.is_empty())
            {
                issues.push(issue(
                    "trust-boundary",
                    Some(contract_name),
                    "TAMA_TRUST_ASSUMPTION",
                    format!(
                        "{contract_name} ECM module `{name}` has an undischarged assumption without a declared allowlist identifier"
                    ),
                    Some(relative_path(ctx.root, ctx.path)),
                ));
            }
        }
        _ => issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_ASSUMPTION_REPORT_INVALID",
            format!("{contract_name} assumption report has unknown category `{category}`"),
            Some(relative_path(ctx.root, ctx.path)),
        )),
    }
}

fn check_axiom_array(
    ctx: &TrustContext<'_>,
    contract_name: &str,
    entry: &serde_json::Value,
    name: &str,
    label: &str,
    issues: &mut Vec<Issue>,
) {
    let Some(axioms) = entry.get("axioms").and_then(|value| value.as_array()) else {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_ASSUMPTION",
            format!("{contract_name} {label} `{name}` is missing declared axioms"),
            Some(relative_path(ctx.root, ctx.path)),
        ));
        return;
    };
    if axioms.is_empty() && entry.get("status").and_then(|value| value.as_str()) != Some("proved") {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_ASSUMPTION",
            format!("{contract_name} {label} `{name}` has an undischarged assumption without a declared allowlist identifier"),
            Some(relative_path(ctx.root, ctx.path)),
        ));
    }
    for axiom in axioms {
        let Some(axiom) = axiom.as_str() else {
            issues.push(issue(
                "trust-boundary",
                Some(contract_name),
                "TAMA_TRUST_ASSUMPTION",
                format!("{contract_name} {label} `{name}` contains a non-string axiom"),
                Some(relative_path(ctx.root, ctx.path)),
            ));
            continue;
        };
        check_allowed_assumption(ctx, contract_name, axiom, label, issues);
    }
}

fn check_allowed_assumption(
    ctx: &TrustContext<'_>,
    contract_name: &str,
    identifier: &str,
    label: &str,
    issues: &mut Vec<Issue>,
) {
    if identifier.is_empty() {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_ASSUMPTION",
            format!("{contract_name} {label} has an empty allowlist identifier"),
            Some(relative_path(ctx.root, ctx.path)),
        ));
    } else if ctx.deny.contains(identifier) || !ctx.allow.contains_key(identifier) {
        issues.push(issue(
            "trust-boundary",
            Some(contract_name),
            "TAMA_TRUST_ASSUMPTION",
            format!("{contract_name} {label} depends on unallowlisted assumption `{identifier}`"),
            Some(relative_path(ctx.root, ctx.path)),
        ));
    }
}

fn describe_status_bucket(value: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    for key in [
        "axiomatizedPrimitives",
        "linkedExternals",
        "ecmModules",
        "localObligations",
    ] {
        let entries = value
            .get(key)
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        if !entries.is_empty() {
            parts.push(format!("{key}={}", entries.join(",")));
        }
    }
    if parts.is_empty() {
        "unchecked dependencies".to_string()
    } else {
        parts.join("; ")
    }
}

fn read_trust_json(
    root: &Utf8Path,
    path: &Utf8Path,
    code: &str,
    label: &str,
    issues: &mut Vec<Issue>,
) -> Option<serde_json::Value> {
    let text = match tama_common::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            issues.push(issue(
                "trust-boundary",
                None,
                code,
                format!("could not read {label}: {err}"),
                Some(relative_path(root, path)),
            ));
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(err) => {
            issues.push(issue(
                "trust-boundary",
                None,
                code,
                format!("could not parse {label}: {err}"),
                Some(relative_path(root, path)),
            ));
            None
        }
    }
}

fn relative_path(root: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_owned()
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
    fn structure_reports_manifest_schema_and_path_issues() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        let mut manifest = counter_manifest();
        manifest.schema = "wrong.schema".to_string();
        manifest.source.implementation = "../Counter.lean".into();

        let mut issues = Vec::new();
        structure(&root, &config, &[manifest], &mut issues);
        let codes = issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<BTreeSet<_>>();
        assert!(codes.contains("TAMA_STRUCTURE_MANIFEST_SCHEMA"));
        assert!(codes.contains("TAMA_STRUCTURE_MANIFEST_PATH"));
    }

    #[test]
    fn structure_reports_bytecode_hash_drift() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        let mut manifest = counter_manifest();
        write_complete_structure_fixture(&root, &config, &mut manifest);

        let mut clean_issues = Vec::new();
        structure(&root, &config, &[manifest.clone()], &mut clean_issues);
        assert!(!clean_issues
            .iter()
            .any(|issue| issue.code == "TAMA_STRUCTURE_BYTECODE_HASH"));

        manifest.artifacts.bytecode_hash = Some("deadbeef".to_string());
        let mut drift_issues = Vec::new();
        structure(&root, &config, &[manifest.clone()], &mut drift_issues);
        assert!(drift_issues
            .iter()
            .any(|issue| issue.code == "TAMA_STRUCTURE_BYTECODE_HASH"));

        manifest.artifacts.bytecode_hash = None;
        let mut missing_issues = Vec::new();
        structure(&root, &config, &[manifest], &mut missing_issues);
        assert!(missing_issues
            .iter()
            .any(|issue| issue.code == "TAMA_STRUCTURE_BYTECODE_HASH"));
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
    fn trust_report_fails_on_unallowlisted_compiler_assumption() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        write_trust_report(&root, verity_trust_report_json());
        let mut issues = Vec::new();
        trust(&root, &config, &[counter_manifest()], &mut issues);
        assert!(issues.iter().any(|issue| {
            issue.code == "TAMA_TRUST_ASSUMPTION"
                && issue.message.contains("keccak256_memory_slice_matches_evm")
        }));
    }

    #[test]
    fn trust_report_accepts_allowlisted_compiler_assumption() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let mut config = test_config();
        config.trust.allow_axioms.insert(
            "keccak256_memory_slice_matches_evm".to_string(),
            "Accepted pinned Verity keccak primitive assumption".to_string(),
        );
        write_trust_report(&root, verity_trust_report_json());
        let mut issues = Vec::new();
        trust(&root, &config, &[counter_manifest()], &mut issues);
        assert!(!issues
            .iter()
            .any(|issue| issue.code == "TAMA_TRUST_ASSUMPTION"));
    }

    #[test]
    fn assumption_report_fails_on_unallowlisted_undischarged_entry() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        tama_common::write_string(
            &root.join("artifacts/assumption-report.json"),
            r#"{"contracts":[{"contract":"Counter","entries":[],"undischarged":[{"category":"axiomatizedPrimitive","siteKind":"function","siteName":"increment","name":"keccak256","status":"assumed","detail":"","assumption":"keccak256_memory_slice_matches_evm","module":"","axioms":[]}]}]}"#,
        )
        .unwrap();
        let mut issues = Vec::new();
        trust(&root, &config, &[counter_manifest()], &mut issues);
        assert!(issues.iter().any(|issue| {
            issue.code == "TAMA_TRUST_ASSUMPTION"
                && issue.message.contains("keccak256_memory_slice_matches_evm")
        }));
    }

    #[test]
    fn trust_report_fails_on_undischarged_local_obligation_without_assumption_report() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let mut config = test_config();
        config.trust.allow_axioms.insert(
            "keccak256_memory_slice_matches_evm".to_string(),
            "Accepted pinned Verity keccak primitive assumption".to_string(),
        );
        let report = verity_trust_report_json().replacen(
            r#""localObligations": []"#,
            r#""localObligations": [{"name":"manual_refinement","status":"assumed","obligation":"Manual proof required"}]"#,
            1,
        );
        write_trust_report(&root, &report);
        let mut issues = Vec::new();
        trust(&root, &config, &[counter_manifest()], &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.code == "TAMA_TRUST_LOCAL_OBLIGATION"));
    }

    #[test]
    fn trust_report_fails_on_low_level_surface() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let mut config = test_config();
        config.trust.allow_axioms.insert(
            "keccak256_memory_slice_matches_evm".to_string(),
            "Accepted pinned Verity keccak primitive assumption".to_string(),
        );
        write_trust_report(
            &root,
            &verity_trust_report_json().replace(
                r#""modeledLowLevelMechanics": []"#,
                r#""modeledLowLevelMechanics": ["staticcall"]"#,
            ),
        );
        let mut issues = Vec::new();
        trust(&root, &config, &[counter_manifest()], &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.code == "TAMA_TRUST_SURFACE"));
    }

    #[test]
    fn trust_report_fails_closed_on_unknown_shape() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let config = test_config();
        write_trust_report(&root, r#"{"contracts":{}}"#);
        let mut issues = Vec::new();
        trust(&root, &config, &[counter_manifest()], &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.code == "TAMA_TRUST_REPORT_INVALID"));
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
    // function ignored(uint256 amount) external;
    /* event Ignored(address indexed from); */
    string constant S = "function alsoIgnored(bytes32) external";
    function set(uint256 amount, bytes32[] calldata proof) external;
    event Transfer(address indexed from, address indexed to, uint256 amount);
    error Bad(address account);
}
"#;
        assert!(solidity_declarations(text, "function").contains("set(uint256,bytes32[])"));
        assert!(!solidity_declarations(text, "function").contains("ignored(uint256)"));
        assert!(!solidity_declarations(text, "function").contains("alsoIgnored(bytes32)"));
        assert!(solidity_declarations(text, "event").contains("Transfer(address,address,uint256)"));
        assert!(!solidity_declarations(text, "event").contains("Ignored(address)"));
        assert!(solidity_declarations(text, "error").contains("Bad(address)"));
    }

    #[test]
    fn coverage_requires_solidity_function_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let mut manifest = counter_manifest();
        let mut obligation = public_obligation();
        obligation.coverage = tama_manifest::Coverage {
            disposition: CoverageDisposition::Mirror,
            path: Some(
                "test/verity/Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount".to_string(),
            ),
            reason: None,
        };
        manifest.obligations.push(obligation);
        tama_common::write_string(
            &root.join("test/verity/Counter.t.sol"),
            r#"// function testFuzzIncrementUpdatesCount(uint8 initialSteps, uint8 extraSteps) public {}
contract CounterTest {
    string constant S = "function testFuzzIncrementUpdatesCount(uint8,uint8) public";
}
"#,
        )
        .unwrap();

        let mut issues = Vec::new();
        coverage(&root, &[manifest.clone()], &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.code == "TAMA_COVERAGE_MISSING_SYMBOL"));

        tama_common::write_string(
            &root.join("test/verity/Counter.t.sol"),
            r#"contract CounterTest {
    function testFuzzIncrementUpdatesCount(uint8 initialSteps, uint8 extraSteps) public {}
}
"#,
        )
        .unwrap();

        let mut clean_issues = Vec::new();
        coverage(&root, &[manifest], &mut clean_issues);
        assert!(!clean_issues
            .iter()
            .any(|issue| issue.code == "TAMA_COVERAGE_MISSING_SYMBOL"));
    }

    #[test]
    fn coverage_requires_property_shaped_mirror_tests() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let mut manifest = counter_manifest();
        let mut obligation = public_obligation();
        obligation.coverage = tama_manifest::Coverage {
            disposition: CoverageDisposition::Mirror,
            path: Some("test/verity/Counter.t.sol:CounterTest.testIncrementPost".to_string()),
            reason: None,
        };
        manifest.obligations.push(obligation);
        tama_common::write_string(
            &root.join("test/verity/Counter.t.sol"),
            r#"contract CounterTest {
    function testIncrementPost() public {}
}
"#,
        )
        .unwrap();

        let mut issues = Vec::new();
        coverage(&root, &[manifest], &mut issues);
        assert!(issues
            .iter()
            .any(|issue| issue.code == "TAMA_COVERAGE_SHAPE"));
    }

    #[test]
    fn coverage_rejects_escaping_mirror_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let mut manifest = counter_manifest();
        let mut obligation = public_obligation();
        obligation.coverage = tama_manifest::Coverage {
            disposition: CoverageDisposition::Mirror,
            path: Some("../Counter.t.sol:CounterTest.testFuzzIncrementUpdatesCount".to_string()),
            reason: None,
        };
        manifest.obligations.push(obligation);

        let mut issues = Vec::new();
        coverage(&root, &[manifest], &mut issues);

        assert!(issues
            .iter()
            .any(|issue| issue.code == "TAMA_COVERAGE_PATH"));
    }

    #[test]
    fn audit_reports_corrupt_selector_as_issue_instead_of_load_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        write_project_config(&root);
        std::fs::create_dir_all(root.join("artifacts/manifest")).unwrap();
        std::fs::create_dir_all(root.join("src/generated/verity")).unwrap();
        tama_common::write_generated(
            &root.join("src/generated/verity/CounterIface.sol"),
            r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

interface CounterIface {
    function getCount() external view returns (uint256);
}
"#,
        )
        .unwrap();
        let mut manifest = counter_manifest();
        manifest.abi.functions.push(tama_manifest::Function {
            name: "getCount".to_string(),
            signature: "getCount()".to_string(),
            selector: "0x00000000".to_string(),
            visibility: "external".to_string(),
            mutability: "view".to_string(),
            inputs: vec![],
            outputs: vec![tama_manifest::Param {
                name: "".to_string(),
                ty: "uint256".to_string(),
            }],
        });
        let text = serde_json::to_string_pretty(&manifest).unwrap();
        tama_common::write_string(&root.join("artifacts/manifest/Counter.json"), &text).unwrap();

        let report = run(
            &root,
            AuditOptions {
                check: Some(Check::Selectors),
                deny_warnings: false,
            },
        )
        .unwrap();
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "TAMA_SELECTOR_INVALID"));
    }

    #[test]
    fn storage_reports_slot_encoding_and_overlap_issues() {
        let mut manifest = counter_manifest();
        manifest.storage = vec![
            tama_manifest::StorageEntry {
                name: "a".to_string(),
                ty: "uint128".to_string(),
                slot: "0xZZ".to_string(),
                offset: 0,
                width_bytes: 16,
                encoding: "value".to_string(),
            },
            tama_manifest::StorageEntry {
                name: "b".to_string(),
                ty: "uint128".to_string(),
                slot: "0x00".to_string(),
                offset: 8,
                width_bytes: 16,
                encoding: "unsupported".to_string(),
            },
            tama_manifest::StorageEntry {
                name: "c".to_string(),
                ty: "uint128".to_string(),
                slot: "0x00".to_string(),
                offset: 20,
                width_bytes: 16,
                encoding: "value".to_string(),
            },
        ];
        let mut issues = Vec::new();
        storage(&[manifest], &mut issues);
        let codes = issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<BTreeSet<_>>();
        assert!(codes.contains("TAMA_STORAGE_SLOT"));
        assert!(codes.contains("TAMA_STORAGE_ENCODING"));
        assert!(codes.contains("TAMA_STORAGE_DUPLICATE"));
        assert!(codes.contains("TAMA_STORAGE_WIDTH"));
    }

    fn write_project_config(root: &Utf8Path) {
        tama_common::write_string(
            &root.join("tama.toml"),
            r#"[project]
name = "test"
verity = "0.1.0"

[yul]
solc = "0.8.33"
"#,
        )
        .unwrap();
    }

    fn write_trust_report(root: &Utf8Path, contents: &str) {
        tama_common::write_string(&root.join("artifacts/trust-report.json"), contents).unwrap();
    }

    fn verity_trust_report_json() -> &'static str {
        r#"{
  "contracts": [
    {
      "contract": "Counter",
      "modeledLowLevelMechanics": [],
      "notModeledEventEmission": [],
      "notModeledProxyUpgradeability": [],
      "partiallyModeledLinearMemoryMechanics": [],
      "partiallyModeledRuntimeIntrospection": [],
      "axiomatizedPrimitives": ["keccak256"],
      "localObligations": [],
      "unsafeBlocks": [],
      "proofStatus": {
        "proved": {
          "axiomatizedPrimitives": [],
          "linkedExternals": [],
          "ecmModules": [],
          "localObligations": []
        },
        "assumed": {
          "axiomatizedPrimitives": ["keccak256"],
          "linkedExternals": [],
          "ecmModules": [],
          "localObligations": []
        },
        "unchecked": {
          "axiomatizedPrimitives": [],
          "linkedExternals": [],
          "ecmModules": [],
          "localObligations": []
        }
      },
      "hasUncheckedDependencies": false,
      "proofBoundary": {
        "compilerModelsMechanics": true,
        "proofInterpretersModelMechanics": false,
        "calleeBehaviorRequiresAssumptions": true
      },
      "usageSites": [
        {
          "kind": "function",
          "name": "increment",
          "modeledLowLevelMechanics": [],
          "notModeledEventEmission": [],
          "notModeledProxyUpgradeability": [],
          "partiallyModeledLinearMemoryMechanics": [],
          "partiallyModeledRuntimeIntrospection": [],
          "axiomatizedPrimitives": ["keccak256"],
          "proofStatus": {
            "proved": {
              "axiomatizedPrimitives": [],
              "linkedExternals": [],
              "ecmModules": [],
              "localObligations": []
            },
            "assumed": {
              "axiomatizedPrimitives": ["keccak256"],
              "linkedExternals": [],
              "ecmModules": [],
              "localObligations": []
            },
            "unchecked": {
              "axiomatizedPrimitives": [],
              "linkedExternals": [],
              "ecmModules": [],
              "localObligations": []
            }
          },
          "localObligations": [],
          "unsafeBlocks": [],
          "hasUncheckedDependencies": false,
          "externalAssumptions": {
            "axiomatizedPrimitives": [
              {
                "primitive": "keccak256",
                "status": "assumed",
                "assumption": "keccak256_memory_slice_matches_evm"
              }
            ],
            "linkedExternals": [],
            "ecmAxioms": [],
            "ecmModules": []
          }
        }
      ],
      "externalAssumptions": {
        "axiomatizedPrimitives": [
          {
            "primitive": "keccak256",
            "status": "assumed",
            "assumption": "keccak256_memory_slice_matches_evm"
          }
        ],
        "linkedExternals": [],
        "ecmAxioms": [],
        "ecmModules": []
      }
    }
  ]
}"#
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

    fn write_complete_structure_fixture(
        root: &Utf8Path,
        config: &tama_config::TamaConfig,
        manifest: &mut ContractManifest,
    ) {
        for dir in [
            &config.paths.src,
            &config.paths.spec,
            &config.paths.proof,
            &config.paths.test,
            &config.paths.generated,
            &config.paths.out,
            &config.paths.out.join("yul"),
            &config.paths.out.join("bytecode"),
            &config.paths.out.join("solc-json"),
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        tama_common::write_string(&root.join(&manifest.source.implementation), "").unwrap();
        tama_common::write_string(&root.join(&manifest.source.spec), "").unwrap();
        tama_common::write_string(&root.join(&manifest.source.proof), "").unwrap();
        tama_common::write_string(&root.join("test/verity/Counter.t.sol"), "").unwrap();
        tama_common::write_string(&root.join(&manifest.artifacts.yul), "{ }\n").unwrap();
        tama_common::write_string(&root.join(&manifest.artifacts.creation_bytecode), "6000\n")
            .unwrap();
        tama_common::write_string(&root.join(&manifest.artifacts.runtime_bytecode), "6000\n")
            .unwrap();
        tama_common::write_string(&root.join(&manifest.artifacts.solc_input), "{}\n").unwrap();
        tama_common::write_string(&root.join(&manifest.artifacts.solc_output), "{}\n").unwrap();
        tama_common::write_generated(&root.join(&manifest.artifacts.interface), "").unwrap();
        tama_common::write_generated(&root.join(&manifest.artifacts.deployer), "").unwrap();
        tama_common::write_string(&root.join("TamaSrc.lean"), "import src.Counter\n").unwrap();
        tama_common::write_string(
            &root.join("TamaSpec.lean"),
            "import TamaSrc\nimport spec.CounterSpec\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join("TamaProof.lean"),
            "import TamaSpec\nimport proof.CounterProof\n",
        )
        .unwrap();
        manifest.artifacts.bytecode_hash = Some(
            tama_common::sha256_file(&root.join(&manifest.artifacts.creation_bytecode)).unwrap(),
        );
    }
}
