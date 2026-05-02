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
#[serde(deny_unknown_fields)]
pub struct ContractManifest {
    pub schema: String,
    pub contract: String,
    pub source: SourcePaths,
    pub lean: LeanModules,
    pub abi: Abi,
    pub storage: Vec<StorageEntry>,
    pub obligations: Vec<Obligation>,
    pub artifacts: ArtifactPaths,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcePaths {
    pub implementation: Utf8PathBuf,
    pub spec: Utf8PathBuf,
    pub proof: Utf8PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeanModules {
    pub implementation_module: String,
    pub spec_module: String,
    pub proof_module: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Abi {
    pub constructor: Option<Constructor>,
    pub functions: Vec<Function>,
    pub events: Vec<Event>,
    pub errors: Vec<ErrorEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constructor {
    pub inputs: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct Event {
    pub name: String,
    pub signature: String,
    pub topic0: String,
    pub fields: Vec<EventField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorEntry {
    pub name: String,
    pub signature: String,
    pub selector: String,
    pub inputs: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub indexed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
        self.validate_lean_modules()?;
        self.validate_artifacts()?;
        if let Some(constructor) = &self.abi.constructor {
            validate_params(&constructor.inputs, "constructor input", "constructor").map_err(
                |message| Error::Invalid {
                    contract: self.contract.clone(),
                    message,
                },
            )?;
        }
        for function in &self.abi.functions {
            if function.name.trim().is_empty() {
                return self.invalid("function name cannot be empty");
            }
            if !is_identifier(&function.name) {
                return self.invalid(format!(
                    "function name `{}` must be a Solidity identifier",
                    function.name
                ));
            }
            validate_params(&function.inputs, "function input", &function.name).map_err(
                |message| Error::Invalid {
                    contract: self.contract.clone(),
                    message,
                },
            )?;
            validate_params(&function.outputs, "function output", &function.name).map_err(
                |message| Error::Invalid {
                    contract: self.contract.clone(),
                    message,
                },
            )?;
            let expected_signature = function_signature(function);
            if function.signature != expected_signature {
                return self.invalid(format!(
                    "function `{}` signature must be `{}`",
                    function.name, expected_signature
                ));
            }
            if function.visibility.trim().is_empty() {
                return self.invalid(format!(
                    "function `{}` visibility cannot be empty",
                    function.name
                ));
            }
            if !matches!(function.visibility.as_str(), "external" | "public") {
                return self.invalid(format!(
                    "function `{}` has unsupported visibility `{}`",
                    function.name, function.visibility
                ));
            }
            if !matches!(
                function.mutability.as_str(),
                "nonpayable" | "payable" | "view" | "pure"
            ) {
                return self.invalid(format!(
                    "function `{}` has unsupported mutability `{}`",
                    function.name, function.mutability
                ));
            }
            let expected = tama_common::function_selector(&function.signature);
            if function.selector != expected {
                return self.invalid(format!(
                    "function `{}` selector `{}` does not match `{}`",
                    function.signature, function.selector, expected
                ));
            }
        }
        for error in &self.abi.errors {
            if error.name.trim().is_empty() {
                return self.invalid("error name cannot be empty");
            }
            if !is_identifier(&error.name) {
                return self.invalid(format!(
                    "error name `{}` must be a Solidity identifier",
                    error.name
                ));
            }
            validate_params(&error.inputs, "error input", &error.name).map_err(|message| {
                Error::Invalid {
                    contract: self.contract.clone(),
                    message,
                }
            })?;
            let expected_signature = error_signature(error);
            if error.signature != expected_signature {
                return self.invalid(format!(
                    "error `{}` signature must be `{}`",
                    error.name, expected_signature
                ));
            }
            let expected = tama_common::error_selector(&error.signature);
            if error.selector != expected {
                return self.invalid(format!(
                    "error `{}` selector `{}` does not match `{}`",
                    error.signature, error.selector, expected
                ));
            }
        }
        for event in &self.abi.events {
            if event.name.trim().is_empty() {
                return self.invalid("event name cannot be empty");
            }
            if !is_identifier(&event.name) {
                return self.invalid(format!(
                    "event name `{}` must be a Solidity identifier",
                    event.name
                ));
            }
            validate_event_fields(&event.fields, &event.name).map_err(|message| {
                Error::Invalid {
                    contract: self.contract.clone(),
                    message,
                }
            })?;
            let expected_signature = event_signature(event);
            if event.signature != expected_signature {
                return self.invalid(format!(
                    "event `{}` signature must be `{}`",
                    event.name, expected_signature
                ));
            }
            let expected = tama_common::event_topic(&event.signature);
            if event.topic0 != expected {
                return self.invalid(format!(
                    "event `{}` topic0 `{}` does not match `{}`",
                    event.signature, event.topic0, expected
                ));
            }
        }
        for storage in &self.storage {
            if storage.name.trim().is_empty() {
                return self.invalid("storage name cannot be empty");
            }
            if storage.ty.trim().is_empty() {
                return self.invalid(format!("storage `{}` type cannot be empty", storage.name));
            }
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
            if storage.offset > 31 {
                return self.invalid(format!(
                    "storage `{}` has invalid offset {}",
                    storage.name, storage.offset
                ));
            }
            if !matches!(
                storage.encoding.as_str(),
                "value" | "mapping" | "dynamic_array" | "bytes" | "struct"
            ) {
                return self.invalid(format!(
                    "storage `{}` has unsupported encoding `{}`",
                    storage.name, storage.encoding
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
            if path.as_str().trim().is_empty() {
                return self.invalid("manifest paths cannot be empty");
            }
            if path.is_absolute() || path.components().any(|part| part.as_str() == "..") {
                return self.invalid(format!("path `{path}` escapes project root"));
            }
        }
        Ok(())
    }

    fn validate_lean_modules(&self) -> Result<()> {
        for (label, name) in [
            (
                "implementation_module",
                self.lean.implementation_module.as_str(),
            ),
            ("spec_module", self.lean.spec_module.as_str()),
            ("proof_module", self.lean.proof_module.as_str()),
        ] {
            if !is_qualified_lean_name(name) {
                return self.invalid(format!(
                    "lean {label} `{name}` must be a fully qualified Lean name"
                ));
            }
        }
        Ok(())
    }

    fn validate_artifacts(&self) -> Result<()> {
        if let Some(hash) = &self.artifacts.bytecode_hash {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return self.invalid("artifact bytecode_hash must be 64 lowercase hex characters");
            }
        }
        Ok(())
    }

    fn validate_obligation(&self, obligation: &Obligation) -> Result<()> {
        if obligation.id.trim().is_empty() {
            return self.invalid("obligation id cannot be empty");
        }
        if obligation.name.trim().is_empty() {
            return self.invalid(format!(
                "obligation `{}` name cannot be empty",
                obligation.id
            ));
        }
        if !is_identifier(&obligation.name) {
            return self.invalid(format!(
                "obligation `{}` name `{}` must be a Lean identifier",
                obligation.id, obligation.name
            ));
        }
        if obligation.contract != self.contract {
            return self.invalid(format!(
                "obligation `{}` contract `{}` must match manifest contract `{}`",
                obligation.id, obligation.contract, self.contract
            ));
        }
        if let Some(function) = &obligation.function {
            if function.trim().is_empty() {
                return self.invalid(format!(
                    "obligation `{}` function cannot be empty",
                    obligation.id
                ));
            }
            let known_function = self
                .abi
                .functions
                .iter()
                .any(|abi| abi.name == function.as_str())
                || (function == "constructor" && self.abi.constructor.is_some());
            if !known_function {
                return self.invalid(format!(
                    "obligation `{}` references unknown function `{}`",
                    obligation.id, function
                ));
            }
        }
        if !is_qualified_lean_name(&obligation.lean_decl) {
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
                        let Some((file, symbol)) = path.split_once(':') else {
                            return self.invalid(format!(
                                "obligation `{}` mirror coverage path `{}` must include a Solidity symbol",
                                obligation.id, path
                            ));
                        };
                        if file.trim().is_empty() || symbol.trim().is_empty() {
                            return self.invalid(format!(
                                "obligation `{}` mirror coverage path `{}` must include a non-empty file and symbol",
                                obligation.id, path
                            ));
                        }
                        if !mirror_symbol_is_property(symbol) {
                            return self.invalid(format!(
                                "obligation `{}` mirror symbol `{}` must be a fuzz test or invariant",
                                obligation.id, symbol
                            ));
                        }
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

fn mirror_symbol_is_property(symbol: &str) -> bool {
    let name = symbol.rsplit('.').next().unwrap_or(symbol).trim();
    name.starts_with("testFuzz") || name.starts_with("invariant_")
}

fn is_qualified_lean_name(value: &str) -> bool {
    let mut segment_count = 0;
    for segment in value.split('.') {
        segment_count += 1;
        if !is_identifier(segment) {
            return false;
        }
    }
    segment_count >= 2
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn function_signature(function: &Function) -> String {
    signature_from_types(
        &function.name,
        function.inputs.iter().map(|param| param.ty.as_str()),
    )
}

fn error_signature(error: &ErrorEntry) -> String {
    signature_from_types(
        &error.name,
        error.inputs.iter().map(|param| param.ty.as_str()),
    )
}

fn event_signature(event: &Event) -> String {
    signature_from_types(
        &event.name,
        event.fields.iter().map(|field| field.ty.as_str()),
    )
}

fn signature_from_types<'a>(name: &str, types: impl Iterator<Item = &'a str>) -> String {
    format!("{}({})", name, types.collect::<Vec<_>>().join(","))
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

fn validate_params(params: &[Param], label: &str, owner: &str) -> std::result::Result<(), String> {
    for (index, param) in params.iter().enumerate() {
        if param.ty.trim().is_empty() {
            return Err(format!(
                "{label} {index} for `{owner}` type cannot be empty"
            ));
        }
    }
    Ok(())
}

fn validate_event_fields(fields: &[EventField], event: &str) -> std::result::Result<(), String> {
    for (index, field) in fields.iter().enumerate() {
        if field.ty.trim().is_empty() {
            return Err(format!(
                "event field {index} for `{event}` type cannot be empty"
            ));
        }
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
                implementation_module: "src.ERC20Lite".to_string(),
                spec_module: "spec.ERC20LiteSpec".to_string(),
                proof_module: "proof.ERC20LiteProof".to_string(),
            },
            abi: Abi {
                constructor: None,
                functions: vec![Function {
                    name: "transfer".to_string(),
                    signature: "transfer(address,uint256)".to_string(),
                    selector: "0xa9059cbb".to_string(),
                    visibility: "external".to_string(),
                    mutability: "nonpayable".to_string(),
                    inputs: vec![
                        Param {
                            name: "to".to_string(),
                            ty: "address".to_string(),
                        },
                        Param {
                            name: "amount".to_string(),
                            ty: "uint256".to_string(),
                        },
                    ],
                    outputs: vec![],
                }],
                events: vec![Event {
                    name: "Transfer".to_string(),
                    signature: "Transfer(address,address,uint256)".to_string(),
                    topic0: "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
                        .to_string(),
                    fields: vec![
                        EventField {
                            name: "from".to_string(),
                            ty: "address".to_string(),
                            indexed: true,
                        },
                        EventField {
                            name: "to".to_string(),
                            ty: "address".to_string(),
                            indexed: true,
                        },
                        EventField {
                            name: "amount".to_string(),
                            ty: "uint256".to_string(),
                            indexed: false,
                        },
                    ],
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
                lean_decl: "proof.ERC20LiteProof.transfer_post".to_string(),
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
    fn serde_rejects_unknown_and_missing_schema_fields() {
        let mut unknown = serde_json::to_value(manifest()).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_string(), serde_json::json!(true));
        assert!(serde_json::from_value::<ContractManifest>(unknown).is_err());

        let mut missing = serde_json::to_value(manifest()).unwrap();
        missing.as_object_mut().unwrap().remove("obligations");
        assert!(serde_json::from_value::<ContractManifest>(missing).is_err());
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
    fn abi_entries_require_canonical_names_and_signatures() {
        let mut empty_name = manifest();
        empty_name.abi.functions[0].name.clear();
        assert!(matches!(
            empty_name.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("function name cannot be empty")
        ));

        let mut bad_signature = manifest();
        bad_signature.abi.functions[0].signature = "transfer(uint256,address)".to_string();
        bad_signature.abi.functions[0].selector =
            tama_common::function_selector(&bad_signature.abi.functions[0].signature);
        assert!(matches!(
            bad_signature.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("signature must be")
        ));

        let mut bad_event_signature = manifest();
        bad_event_signature.abi.events[0].signature =
            "Transfer(address,uint256,address)".to_string();
        bad_event_signature.abi.events[0].topic0 =
            tama_common::event_topic(&bad_event_signature.abi.events[0].signature);
        assert!(matches!(
            bad_event_signature.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("signature must be")
        ));
    }

    #[test]
    fn function_mutability_must_be_supported() {
        let mut bad_mutability = manifest();
        bad_mutability.abi.functions[0].mutability = "delegatecall".to_string();
        assert!(matches!(
            bad_mutability.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("unsupported mutability")
        ));

        let mut bad_visibility = manifest();
        bad_visibility.abi.functions[0].visibility = "internal".to_string();
        assert!(matches!(
            bad_visibility.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("unsupported visibility")
        ));
    }

    #[test]
    fn abi_and_storage_types_cannot_be_empty() {
        let mut empty_function_param = manifest();
        empty_function_param.abi.functions[0].inputs[0].ty = " ".to_string();
        assert!(matches!(
            empty_function_param.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("function input")
                && message.contains("type cannot be empty")
        ));

        let mut empty_event_field = manifest();
        empty_event_field.abi.events[0].fields[0].ty = String::new();
        assert!(matches!(
            empty_event_field.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("event field")
                && message.contains("type cannot be empty")
        ));

        let mut empty_storage_type = manifest();
        empty_storage_type.storage[0].ty = String::new();
        assert!(matches!(
            empty_storage_type.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("storage `balances` type cannot be empty")
        ));
    }

    #[test]
    fn constructor_params_are_validated() {
        let mut manifest = manifest();
        manifest.abi.constructor = Some(Constructor {
            inputs: vec![Param {
                name: "owner".to_string(),
                ty: " ".to_string(),
            }],
        });

        assert!(matches!(
            manifest.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("constructor input")
                && message.contains("type cannot be empty")
        ));
    }

    #[test]
    fn path_traversal_fails() {
        let mut escaping_path = manifest();
        escaping_path.artifacts.yul = "../escape.yul".into();
        assert!(escaping_path.validate().is_err());

        let mut empty_path = manifest();
        empty_path.artifacts.yul = "".into();
        assert!(matches!(
            empty_path.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("paths cannot be empty")
        ));
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
    fn mirror_coverage_requires_property_symbol() {
        let mut manifest = manifest();
        manifest.obligations[0].coverage.path = Some("test/verity/ERC20Lite.t.sol".to_string());
        assert!(manifest.validate().is_err());

        manifest.obligations[0].coverage.path =
            Some("test/verity/ERC20Lite.t.sol:ERC20LiteTest.testTransfer".to_string());
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn lean_names_must_be_qualified_identifiers() {
        let mut bad_module = manifest();
        bad_module.lean.proof_module = "proof".to_string();
        assert!(matches!(
            bad_module.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("proof_module")
                && message.contains("fully qualified")
        ));

        let mut bad_decl = manifest();
        bad_decl.obligations[0].lean_decl = "proof.ERC20LiteProof.transfer-post".to_string();
        assert!(matches!(
            bad_decl.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("lean_decl")
                && message.contains("fully qualified")
        ));
    }

    #[test]
    fn artifact_and_storage_shapes_must_match_schema() {
        let mut bad_hash = manifest();
        bad_hash.artifacts.bytecode_hash = Some("ABCD".to_string());
        assert!(matches!(
            bad_hash.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("bytecode_hash")
        ));

        let mut bad_offset = manifest();
        bad_offset.storage[0].offset = 32;
        assert!(matches!(
            bad_offset.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("invalid offset")
        ));

        let mut bad_encoding = manifest();
        bad_encoding.storage[0].encoding = "packed".to_string();
        assert!(matches!(
            bad_encoding.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("unsupported encoding")
        ));
    }

    #[test]
    fn obligations_must_reference_manifest_contract_and_known_functions() {
        let mut empty_name = manifest();
        empty_name.obligations[0].name.clear();
        assert!(matches!(
            empty_name.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("name cannot be empty")
        ));

        let mut wrong_contract = manifest();
        wrong_contract.obligations[0].contract = "Other".to_string();
        assert!(matches!(
            wrong_contract.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("must match manifest contract")
        ));

        let mut unknown_function = manifest();
        unknown_function.obligations[0].function = Some("mint".to_string());
        assert!(matches!(
            unknown_function.validate(),
            Err(Error::Invalid { message, .. }) if message.contains("unknown function")
        ));
    }

    #[test]
    fn missing_public_coverage_fails() {
        let mut manifest = manifest();
        manifest.obligations[0].coverage.disposition = CoverageDisposition::None;
        assert!(manifest.validate().is_err());
    }
}
