use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};
use tabled::{Table, Tabled};
use tama_manifest::ContractManifest;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Common(#[from] tama_common::Error),
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
    let manifest_path = root
        .join("artifacts/manifest")
        .join(format!("{contract}.json"));
    let manifest = ContractManifest::load(&manifest_path)?;
    if json_mode {
        Ok(serde_json::to_string_pretty(&inspect_json(root, &manifest, field)?)? + "\n")
    } else {
        inspect_human(root, &manifest, field)
    }
}

fn inspect_json(root: &Utf8Path, manifest: &ContractManifest, field: Field) -> Result<Value> {
    Ok(match field {
        Field::Manifest => serde_json::to_value(manifest)?,
        Field::Selectors => json!(manifest
            .abi
            .functions
            .iter()
            .map(|function| json!({
                "name": function.name,
                "signature": function.signature,
                "selector": function.selector
            }))
            .collect::<Vec<_>>()),
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
        Field::Trust => {
            read_json(root.join("artifacts/trust-probe/axioms.json")).unwrap_or_else(|| json!({}))
        }
    })
}

fn inspect_human(root: &Utf8Path, manifest: &ContractManifest, field: Field) -> Result<String> {
    match field {
        Field::Manifest => Ok(serde_json::to_string_pretty(manifest)? + "\n"),
        Field::Selectors => {
            #[derive(Tabled)]
            struct Row {
                name: String,
                signature: String,
                selector: String,
            }
            Ok(
                Table::new(manifest.abi.functions.iter().map(|function| Row {
                    name: function.name.clone(),
                    signature: function.signature.clone(),
                    selector: function.selector.clone(),
                }))
                .to_string()
                    + "\n",
            )
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
        Field::Trust => Ok(read_json(root.join("artifacts/trust-probe/axioms.json"))
            .map(|value| serde_json::to_string_pretty(&value).unwrap_or_default() + "\n")
            .unwrap_or_else(|| "{}\n".to_string())),
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
}
