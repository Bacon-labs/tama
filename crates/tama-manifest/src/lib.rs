use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "tama.contract-manifest.v1";

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Common(#[from] tama_common::Error),
    #[error("manifest JSON error in {path}: {source}")]
    Json {
        path: Utf8PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported manifest schema `{0}`")]
    Schema(String),
    #[error("invalid manifest for {contract}: {message}")]
    Invalid { contract: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractManifest {
    pub schema: String,
    pub contract: String,
    pub source: SourcePaths,
    pub lean: LeanModules,
    pub abi: Abi,
    #[serde(default)]
    pub storage: Vec<StorageEntry>,
    #[serde(default)]
    pub obligations: Vec<Obligation>,
    pub artifacts: ArtifactPaths,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePaths {
    pub implementation: Utf8PathBuf,
    pub spec: Utf8PathBuf,
    pub proof: Utf8PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanModules {
    pub implementation_module: String,
    pub spec_module: String,
    pub proof_module: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Abi {
    pub constructor: Option<Constructor>,
    #[serde(default)]
    pub functions: Vec<Function>,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub errors: Vec<ErrorEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constructor {
    #[serde(default)]
    pub inputs: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    pub signature: String,
    pub selector: String,
    pub visibility: String,
    pub mutability: String,
    #[serde(default)]
    pub inputs: Vec<Param>,
    #[serde(default)]
    pub outputs: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    pub signature: String,
    pub topic0: String,
    #[serde(default)]
    pub fields: Vec<EventField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub name: String,
    pub signature: String,
    pub selector: String,
    #[serde(default)]
    pub inputs: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub indexed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub slot: String,
    pub offset: u32,
    pub width_bytes: u32,
    pub encoding: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Obligation {
    pub id: String,
    pub name: String,
    pub kind: ObligationKind,
    pub lean_decl: String,
    pub contract: String,
    pub function: Option<String>,
    pub coverage: Coverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationKind {
    Invariant,
    Postcondition,
    Helper,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub disposition: CoverageDisposition,
    pub path: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageDisposition {
    Mirror,
    ProofOnly,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPaths {
    pub yul: Utf8PathBuf,
    pub creation_bytecode: Utf8PathBuf,
    pub runtime_bytecode: Utf8PathBuf,
    pub bytecode_hash: Option<String>,
    pub solc_input: Utf8PathBuf,
    pub solc_output: Utf8PathBuf,
    pub interface: Utf8PathBuf,
    pub deployer: Utf8PathBuf,
}

impl ContractManifest {
    pub fn load(path: &Utf8Path) -> Result<Self> {
        let manifest = Self::load_unvalidated(path)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load_unvalidated(path: &Utf8Path) -> Result<Self> {
        let text = tama_common::read_to_string(path)?;
        let manifest: Self = serde_json::from_str(&text).map_err(|source| Error::Json {
            path: path.to_owned(),
            source,
        })?;
        Ok(manifest)
    }

    pub fn write_pretty(&self, path: &Utf8Path) -> Result<()> {
        self.validate()?;
        let text = serde_json::to_string_pretty(self).map_err(|source| Error::Json {
            path: path.to_owned(),
            source,
        })?;
        tama_common::write_string(path, &(text + "\n"))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA {
            return Err(Error::Schema(self.schema.clone()));
        }
        self.validate_contract_name()?;
        self.validate_paths()?;
        for function in &self.abi.functions {
            let expected = tama_common::function_selector(&function.signature);
            if function.selector != expected {
                return self.invalid(format!(
                    "function `{}` selector `{}` does not match `{}`",
                    function.signature, function.selector, expected
                ));
            }
        }
        for error in &self.abi.errors {
            let expected = tama_common::error_selector(&error.signature);
            if error.selector != expected {
                return self.invalid(format!(
                    "error `{}` selector `{}` does not match `{}`",
                    error.signature, error.selector, expected
                ));
            }
        }
        for event in &self.abi.events {
            let expected = tama_common::event_topic(&event.signature);
            if event.topic0 != expected {
                return self.invalid(format!(
                    "event `{}` topic0 `{}` does not match `{}`",
                    event.signature, event.topic0, expected
                ));
            }
        }
        for storage in &self.storage {
            validate_hex_slot(&storage.slot).map_err(|message| Error::Invalid {
                contract: self.contract.clone(),
                message: format!("storage `{}` {message}", storage.name),
            })?;
            if storage.width_bytes == 0 || storage.width_bytes > 32 {
                return self.invalid(format!(
                    "storage `{}` has invalid width {}",
                    storage.name, storage.width_bytes
                ));
            }
        }
        for obligation in &self.obligations {
            self.validate_obligation(obligation)?;
        }
        Ok(())
    }

    fn validate_contract_name(&self) -> Result<()> {
        let re = Regex::new(r"^[A-Z][A-Za-z0-9_]*$").expect("valid regex");
        if re.is_match(&self.contract) {
            Ok(())
        } else {
            self.invalid(format!("invalid contract name `{}`", self.contract))
        }
    }

    fn validate_paths(&self) -> Result<()> {
        for path in [
            &self.source.implementation,
            &self.source.spec,
            &self.source.proof,
            &self.artifacts.yul,
            &self.artifacts.creation_bytecode,
            &self.artifacts.runtime_bytecode,
            &self.artifacts.solc_input,
            &self.artifacts.solc_output,
            &self.artifacts.interface,
            &self.artifacts.deployer,
        ] {
            if path.is_absolute() || path.components().any(|part| part.as_str() == "..") {
                return self.invalid(format!("path `{path}` escapes project root"));
            }
        }
        Ok(())
    }

    fn validate_obligation(&self, obligation: &Obligation) -> Result<()> {
        if obligation.id.trim().is_empty() {
            return self.invalid("obligation id cannot be empty");
        }
        if !obligation.lean_decl.contains('.') {
            return self.invalid(format!(
                "obligation `{}` lean_decl must be fully qualified",
                obligation.id
            ));
        }
        match obligation.kind {
            ObligationKind::Invariant | ObligationKind::Postcondition => {
                match obligation.coverage.disposition {
                    CoverageDisposition::Mirror => {
                        let path = obligation.coverage.path.as_deref().unwrap_or("").trim();
                        if path.is_empty() {
                            return self.invalid(format!(
                                "obligation `{}` mirror coverage requires a path",
                                obligation.id
                            ));
                        }
                        let file = path.split_once(':').map(|(file, _)| file).unwrap_or(path);
                        let file = Utf8Path::new(file);
                        if file.is_absolute() || file.components().any(|part| part.as_str() == "..")
                        {
                            return self.invalid(format!(
                                "obligation `{}` mirror path `{}` escapes project root",
                                obligation.id, path
                            ));
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
                            return self.invalid(format!(
                                "obligation `{}` proof-only coverage requires a reason",
                                obligation.id
                            ));
                        }
                    }
                    CoverageDisposition::None => {
                        return self.invalid(format!(
                            "public obligation `{}` requires mirror or proof_only coverage",
                            obligation.id
                        ));
                    }
                }
            }
            ObligationKind::Helper => {}
        }
        Ok(())
    }

    fn invalid<T>(&self, message: impl Into<String>) -> Result<T> {
        Err(Error::Invalid {
            contract: self.contract.clone(),
            message: message.into(),
        })
    }
}

fn validate_hex_slot(slot: &str) -> std::result::Result<(), &'static str> {
    let rest = slot.strip_prefix("0x").ok_or("slot must have 0x prefix")?;
    if rest.is_empty() || rest.len() > 64 || !rest.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("slot must be lowercase hex");
    }
    if rest.chars().any(|ch| ch.is_ascii_uppercase()) {
        return Err("slot must be lowercase hex");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ContractManifest {
        ContractManifest {
            schema: SCHEMA.to_string(),
            contract: "ERC20Lite".to_string(),
            source: SourcePaths {
                implementation: "verity/src/ERC20Lite.lean".into(),
                spec: "verity/spec/ERC20LiteSpec.lean".into(),
                proof: "verity/proof/ERC20LiteProof.lean".into(),
            },
            lean: LeanModules {
                implementation_module: "verity.src.ERC20Lite".to_string(),
                spec_module: "verity.spec.ERC20LiteSpec".to_string(),
                proof_module: "verity.proof.ERC20LiteProof".to_string(),
            },
            abi: Abi {
                constructor: None,
                functions: vec![Function {
                    name: "transfer".to_string(),
                    signature: "transfer(address,uint256)".to_string(),
                    selector: "0xa9059cbb".to_string(),
                    visibility: "external".to_string(),
                    mutability: "nonpayable".to_string(),
                    inputs: vec![],
                    outputs: vec![],
                }],
                events: vec![Event {
                    name: "Transfer".to_string(),
                    signature: "Transfer(address,address,uint256)".to_string(),
                    topic0: "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
                        .to_string(),
                    fields: vec![],
                }],
                errors: vec![],
            },
            storage: vec![StorageEntry {
                name: "balances".to_string(),
                ty: "mapping(address => uint256)".to_string(),
                slot: "0x01".to_string(),
                offset: 0,
                width_bytes: 32,
                encoding: "mapping".to_string(),
            }],
            obligations: vec![Obligation {
                id: "ERC20Lite.transfer_post".to_string(),
                name: "transfer_post".to_string(),
                kind: ObligationKind::Postcondition,
                lean_decl: "verity.proof.ERC20LiteProof.transfer_post".to_string(),
                contract: "ERC20Lite".to_string(),
                function: Some("transfer".to_string()),
                coverage: Coverage {
                    disposition: CoverageDisposition::Mirror,
                    path: Some("test/verity/ERC20Lite.t.sol:ERC20LiteTest.testFuzzTransferPreservesTotalSupply".to_string()),
                    reason: None,
                },
            }],
            artifacts: ArtifactPaths {
                yul: "artifacts/yul/ERC20Lite.yul".into(),
                creation_bytecode: "artifacts/bytecode/ERC20Lite.bin".into(),
                runtime_bytecode: "artifacts/bytecode/ERC20Lite.runtime.bin".into(),
                bytecode_hash: None,
                solc_input: "artifacts/solc-json/ERC20Lite.input.json".into(),
                solc_output: "artifacts/solc-json/ERC20Lite.output.json".into(),
                interface: "src/generated/verity/ERC20LiteIface.sol".into(),
                deployer: "src/generated/verity/ERC20LiteDeployer.sol".into(),
            },
        }
    }

    #[test]
    fn valid_manifest_passes() {
        manifest().validate().unwrap();
    }

    #[test]
    fn corrupt_selector_fails() {
        let mut manifest = manifest();
        manifest.abi.functions[0].selector = "0x00000000".to_string();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn corrupt_event_topic_fails() {
        let mut manifest = manifest();
        manifest.abi.events[0].topic0 = "0x00".to_string();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn path_traversal_fails() {
        let mut manifest = manifest();
        manifest.artifacts.yul = "../escape.yul".into();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn coverage_path_traversal_fails() {
        let mut manifest = manifest();
        manifest.obligations[0].coverage.path = Some(
            "../ERC20Lite.t.sol:ERC20LiteTest.testFuzzTransferPreservesTotalSupply".to_string(),
        );
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn missing_public_coverage_fails() {
        let mut manifest = manifest();
        manifest.obligations[0].coverage.disposition = CoverageDisposition::None;
        assert!(manifest.validate().is_err());
    }
}
