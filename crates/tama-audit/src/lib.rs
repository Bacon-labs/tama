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
            Check::Structure => structure(root, &manifests, &mut issues),
            Check::Selectors => selectors(&manifests, &mut issues),
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

fn structure(root: &Utf8Path, manifests: &[ContractManifest], issues: &mut Vec<Issue>) {
    if manifests.is_empty() {
        issues.push(issue(
            "structure",
            None,
            "TAMA_STRUCTURE_NO_MANIFEST",
            "no contract manifests found",
            None,
        ));
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

fn selectors(manifests: &[ContractManifest], issues: &mut Vec<Issue>) {
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
    }
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
    if probe.is_file() {
        let text = tama_common::read_to_string(&probe).unwrap_or_default();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        if let Some(obligations) = value.get("obligations").and_then(|value| value.as_array()) {
            for obligation in obligations {
                let decl = obligation
                    .get("lean_decl")
                    .and_then(|value| value.as_str())
                    .unwrap_or("<unknown>");
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
    }
    for manifest in manifests {
        for obligation in &manifest.obligations {
            if !obligation.lean_decl.contains('.') {
                issues.push(issue(
                    "trust-boundary",
                    Some(&manifest.contract),
                    "TAMA_TRUST_DECL",
                    format!("{} is not fully qualified", obligation.lean_decl),
                    None,
                ));
            }
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
}
