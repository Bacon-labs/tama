use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};
use tabled::{Table, Tabled};
use tama_config::PathsConfig;
use tama_manifest::ContractManifest;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Common(#[from] tama_common::Error),
    #[error(transparent)]
    Config(#[from] tama_config::Error),
    #[error(transparent)]
    Manifest(#[from] tama_manifest::Error),
    #[error("unknown inspect field `{0}`")]
    UnknownField(String),
    #[error("missing artifact for {contract}: {path}. Run `tama build`.")]
    MissingArtifact { contract: String, path: Utf8PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Manifest,
    Selectors,
    Abi,
    StorageLayout,
    Yul,
    Bytecode,
    RuntimeBytecode,
    Theorems,
    Obligations,
    Mirrors,
    Trust,
}

pub fn parse_field(raw: &str) -> Option<Field> {
    match raw {
        "manifest" => Some(Field::Manifest),
        "selectors" => Some(Field::Selectors),
        "abi" => Some(Field::Abi),
        "storage-layout" => Some(Field::StorageLayout),
        "yul" => Some(Field::Yul),
        "bytecode" => Some(Field::Bytecode),
        "runtime-bytecode" => Some(Field::RuntimeBytecode),
        "theorems" => Some(Field::Theorems),
        "obligations" => Some(Field::Obligations),
        "mirrors" => Some(Field::Mirrors),
        "trust" => Some(Field::Trust),
        _ => None,
    }
}

pub fn inspect(root: &Utf8Path, contract: &str, field: Field, json_mode: bool) -> Result<String> {
    let paths = tama_config::load_config(root)?.paths;
    let manifest_path = root
        .join(paths.out.join("manifest"))
        .join(format!("{contract}.json"));
    let manifest = ContractManifest::load(&manifest_path)?;
    if json_mode {
        Ok(serde_json::to_string_pretty(&inspect_json(root, &paths, &manifest, field)?)? + "\n")
    } else {
        inspect_human(root, &paths, &manifest, field)
    }
}

fn inspect_json(
    root: &Utf8Path,
    paths: &PathsConfig,
    manifest: &ContractManifest,
    field: Field,
) -> Result<Value> {
    Ok(match field {
        Field::Manifest => serde_json::to_value(manifest)?,
        Field::Selectors => selectors_json(manifest),
        Field::Abi => serde_json::to_value(&manifest.abi)?,
        Field::StorageLayout => serde_json::to_value(&manifest.storage)?,
        Field::Yul => json!({ "yul": artifact(root, manifest, &manifest.artifacts.yul)? }),
        Field::Bytecode => {
            json!({ "bytecode": artifact(root, manifest, &manifest.artifacts.creation_bytecode)? })
        }
        Field::RuntimeBytecode => {
            json!({ "runtime_bytecode": artifact(root, manifest, &manifest.artifacts.runtime_bytecode)? })
        }
        Field::Theorems | Field::Obligations => serde_json::to_value(&manifest.obligations)?,
        Field::Mirrors => json!(manifest
            .obligations
            .iter()
            .filter_map(|obligation| obligation.coverage.path.as_ref())
            .collect::<Vec<_>>()),
        Field::Trust => trust_artifacts(root, paths),
    })
}

fn inspect_human(
    root: &Utf8Path,
    paths: &PathsConfig,
    manifest: &ContractManifest,
    field: Field,
) -> Result<String> {
    match field {
        Field::Manifest => Ok(serde_json::to_string_pretty(manifest)? + "\n"),
        Field::Selectors => {
            #[derive(Tabled)]
            struct Row {
                kind: String,
                name: String,
                signature: String,
                value: String,
            }
            let rows = manifest
                .abi
                .functions
                .iter()
                .map(|function| Row {
                    kind: "function".to_string(),
                    name: function.name.clone(),
                    signature: function.signature.clone(),
                    value: function.selector.clone(),
                })
                .chain(manifest.abi.errors.iter().map(|error| Row {
                    kind: "error".to_string(),
                    name: error.name.clone(),
                    signature: error.signature.clone(),
                    value: error.selector.clone(),
                }))
                .chain(manifest.abi.events.iter().map(|event| Row {
                    kind: "event".to_string(),
                    name: event.name.clone(),
                    signature: event.signature.clone(),
                    value: event.topic0.clone(),
                }));
            Ok(Table::new(rows).to_string() + "\n")
        }
        Field::Abi => Ok(serde_json::to_string_pretty(&manifest.abi)? + "\n"),
        Field::StorageLayout => Ok(serde_json::to_string_pretty(&manifest.storage)? + "\n"),
        Field::Yul => artifact(root, manifest, &manifest.artifacts.yul),
        Field::Bytecode => artifact(root, manifest, &manifest.artifacts.creation_bytecode),
        Field::RuntimeBytecode => artifact(root, manifest, &manifest.artifacts.runtime_bytecode),
        Field::Theorems | Field::Obligations => {
            Ok(serde_json::to_string_pretty(&manifest.obligations)? + "\n")
        }
        Field::Mirrors => Ok(manifest
            .obligations
            .iter()
            .filter_map(|obligation| obligation.coverage.path.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"),
        Field::Trust => Ok(serde_json::to_string_pretty(&trust_artifacts(root, paths))
            .unwrap_or_default()
            + "\n"),
    }
}

fn selectors_json(manifest: &ContractManifest) -> Value {
    json!({
        "functions": manifest
            .abi
            .functions
            .iter()
            .map(|function| json!({
                "name": function.name,
                "signature": function.signature,
                "selector": function.selector
            }))
            .collect::<Vec<_>>(),
        "errors": manifest
            .abi
            .errors
            .iter()
            .map(|error| json!({
                "name": error.name,
                "signature": error.signature,
                "selector": error.selector
            }))
            .collect::<Vec<_>>(),
        "events": manifest
            .abi
            .events
            .iter()
            .map(|event| json!({
                "name": event.name,
                "signature": event.signature,
                "topic0": event.topic0
            }))
            .collect::<Vec<_>>()
    })
}

fn trust_artifacts(root: &Utf8Path, paths: &PathsConfig) -> Value {
    let axiom_probe = read_json(root.join(paths.out.join("trust-probe/axioms.json")));
    let trust_report = read_json(root.join(paths.out.join("trust-report.json")));
    let assumption_report = read_json(root.join(paths.out.join("assumption-report.json")));
    if axiom_probe.is_none() && trust_report.is_none() && assumption_report.is_none() {
        json!({})
    } else {
        json!({
            "axiom_probe": axiom_probe,
            "trust_report": trust_report,
            "assumption_report": assumption_report,
        })
    }
}

fn artifact(root: &Utf8Path, manifest: &ContractManifest, path: &Utf8Path) -> Result<String> {
    let abs = root.join(path);
    if !abs.is_file() {
        return Err(Error::MissingArtifact {
            contract: manifest.contract.clone(),
            path: path.to_owned(),
        });
    }
    Ok(tama_common::read_to_string(&abs)?)
}

fn read_json(path: Utf8PathBuf) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Error::Common(tama_common::Error::Message(source.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_field() {
        assert_eq!(
            parse_field("runtime-bytecode"),
            Some(Field::RuntimeBytecode)
        );
        assert_eq!(parse_field("unknown"), None);
    }

    #[test]
    fn inspect_uses_configured_artifact_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            r#"[project]
name = "custom"
verity = "0.1.0"

[paths]
out = "build/tama"

[yul]
solc = "0.8.33"
"#,
        )
        .unwrap();
        let manifest = counter_manifest("build/tama");
        manifest
            .write_pretty(&root.join("build/tama/manifest/Counter.json"))
            .unwrap();
        tama_common::write_string(
            &root.join("build/tama/trust-probe/axioms.json"),
            r#"{"obligations":[]}"#,
        )
        .unwrap();

        let manifest_out = inspect(&root, "Counter", Field::Manifest, false).unwrap();
        let trust_out = inspect(&root, "Counter", Field::Trust, true).unwrap();

        assert!(manifest_out.contains(r#""contract": "Counter""#));
        assert!(trust_out.contains("axiom_probe"));
    }

    #[test]
    fn selectors_include_functions_errors_and_events() {
        let mut manifest = counter_manifest("artifacts");
        manifest.abi.functions.push(tama_manifest::Function {
            name: "increment".to_string(),
            signature: "increment()".to_string(),
            selector: "0xd09de08a".to_string(),
            visibility: "external".to_string(),
            mutability: "nonpayable".to_string(),
            inputs: vec![],
            outputs: vec![],
        });
        manifest.abi.errors.push(tama_manifest::ErrorEntry {
            name: "Bad".to_string(),
            signature: "Bad(address)".to_string(),
            selector: tama_common::error_selector("Bad(address)"),
            inputs: vec![tama_manifest::Param {
                name: "account".to_string(),
                ty: "address".to_string(),
            }],
        });
        manifest.abi.events.push(tama_manifest::Event {
            name: "Transfer".to_string(),
            signature: "Transfer(address,address,uint256)".to_string(),
            topic0: tama_common::event_topic("Transfer(address,address,uint256)"),
            fields: vec![],
        });

        let value = selectors_json(&manifest);

        assert_eq!(value["functions"][0]["selector"], "0xd09de08a");
        assert_eq!(value["errors"][0]["signature"], "Bad(address)");
        assert_eq!(
            value["events"][0]["topic0"],
            "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
        );
    }

    fn counter_manifest(out: &str) -> ContractManifest {
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
                yul: format!("{out}/yul/Counter.yul").into(),
                creation_bytecode: format!("{out}/bytecode/Counter.bin").into(),
                runtime_bytecode: format!("{out}/bytecode/Counter.runtime.bin").into(),
                bytecode_hash: None,
                solc_input: format!("{out}/solc-json/Counter.input.json").into(),
                solc_output: format!("{out}/solc-json/Counter.output.json").into(),
                interface: "src/generated/verity/CounterIface.sol".into(),
                deployer: "src/generated/verity/CounterDeployer.sol".into(),
            },
        }
    }
}
