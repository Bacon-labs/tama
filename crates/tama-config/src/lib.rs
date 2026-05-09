use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use tama_common::{read_to_string, sha256_file};
use toml_edit::{value, ArrayOfTables, Item, Table};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Common(#[from] tama_common::Error),
    #[error("failed to parse {path}: {source}")]
    Toml {
        path: Utf8PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Json {
        path: Utf8PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported lockfile version {0}")]
    UnsupportedLockVersion(u32),
    #[error("lockfile is stale: {0}")]
    StaleLock(String),
    #[error("unsupported lakefile: {0}")]
    UnsupportedLakefile(String),
    #[error("invalid dependency spec `{0}`")]
    InvalidDependencySpec(String),
    #[error("dependency `{0}` not found in lakefile.toml")]
    DependencyNotFound(String),
    #[error("configured path `{field}` must stay inside the project and contain a directory name: {path}")]
    InvalidPath {
        field: &'static str,
        path: Utf8PathBuf,
    },
    #[error("[coverage.proof_only] entry `{key}` requires a non-empty reason")]
    EmptyProofOnlyReason { key: String },
    #[error("configured module `{field}` must be a Lean module name: {name}")]
    InvalidModuleName { field: &'static str, name: String },
    #[error("Lean file `{path}` is not under configured module directory `{base_dir}`")]
    ModulePathOutsideRoot {
        path: Utf8PathBuf,
        base_dir: Utf8PathBuf,
    },
    #[error("Lean file `{path}` is outside configured module root `{module_root}`")]
    ModuleOutsideRoot {
        path: Utf8PathBuf,
        module_root: String,
    },
    #[error("Lean file `{path}` contains unsupported module path component `{component}`")]
    InvalidModulePathComponent {
        path: Utf8PathBuf,
        component: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TamaConfig {
    pub project: ProjectConfig,
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub modules: Option<ModulesConfig>,
    pub yul: YulConfig,
    #[serde(default)]
    pub trust: TrustConfig,
    #[serde(default)]
    pub coverage: CoverageConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub name: String,
    pub verity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathsConfig {
    #[serde(default = "default_src")]
    pub src: Utf8PathBuf,
    #[serde(default = "default_spec")]
    pub spec: Utf8PathBuf,
    #[serde(default = "default_proof")]
    pub proof: Utf8PathBuf,
    #[serde(default = "default_test", alias = "mirror_test")]
    pub test: Utf8PathBuf,
    #[serde(default = "default_out")]
    pub out: Utf8PathBuf,
    #[serde(default = "default_generated", alias = "generated_solidity")]
    pub generated: Utf8PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModulesConfig {
    pub src: String,
    pub spec: String,
    pub proof: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Src,
    Spec,
    Proof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateModule {
    pub module: String,
    pub path: Utf8PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            src: default_src(),
            spec: default_spec(),
            proof: default_proof(),
            test: default_test(),
            out: default_out(),
            generated: default_generated(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YulConfig {
    pub solc: String,
    #[serde(default = "default_true")]
    pub optimizer: bool,
    #[serde(default = "default_optimizer_runs")]
    pub optimizer_runs: u32,
    #[serde(default = "default_true")]
    pub yul_optimizer: bool,
    #[serde(default = "default_evm")]
    pub evm_version: String,
    #[serde(default = "default_metadata_hash", alias = "metadata_bytecode_hash")]
    pub metadata_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustConfig {
    #[serde(default)]
    pub allow_axioms: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageConfig {
    #[serde(default)]
    pub proof_only: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TamaLock {
    pub version: u32,
    #[serde(default)]
    pub resolved: BTreeMap<String, String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    #[serde(default)]
    pub yul: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundryConfig {
    #[serde(default = "default_foundry_src")]
    pub src: Utf8PathBuf,
    #[serde(default = "default_foundry_test")]
    pub test: Utf8PathBuf,
    #[serde(default = "default_foundry_out")]
    pub out: Utf8PathBuf,
    #[serde(default = "default_foundry_cache")]
    pub cache: Utf8PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakeDependency {
    pub name: String,
    pub source: LakeDependencySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LakeDependencySource {
    Git { url: String, rev: String },
    Path { path: Utf8PathBuf },
}

impl Default for FoundryConfig {
    fn default() -> Self {
        Self {
            src: default_foundry_src(),
            test: default_foundry_test(),
            out: default_foundry_out(),
            cache: default_foundry_cache(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct FoundryToml {
    #[serde(default)]
    src: Option<Utf8PathBuf>,
    #[serde(default)]
    test: Option<Utf8PathBuf>,
    #[serde(default)]
    out: Option<Utf8PathBuf>,
    #[serde(default)]
    cache_path: Option<Utf8PathBuf>,
    #[serde(default)]
    profile: FoundryProfiles,
}

#[derive(Debug, Default, Deserialize)]
struct FoundryProfiles {
    #[serde(default)]
    default: FoundryProfileConfig,
}

#[derive(Debug, Default, Deserialize)]
struct FoundryProfileConfig {
    #[serde(default)]
    src: Option<Utf8PathBuf>,
    #[serde(default)]
    test: Option<Utf8PathBuf>,
    #[serde(default)]
    out: Option<Utf8PathBuf>,
    #[serde(default)]
    cache_path: Option<Utf8PathBuf>,
}

impl FoundryToml {
    fn into_config(self) -> FoundryConfig {
        let defaults = FoundryConfig::default();
        FoundryConfig {
            src: self
                .profile
                .default
                .src
                .or(self.src)
                .unwrap_or(defaults.src),
            test: self
                .profile
                .default
                .test
                .or(self.test)
                .unwrap_or(defaults.test),
            out: self
                .profile
                .default
                .out
                .or(self.out)
                .unwrap_or(defaults.out),
            cache: self
                .profile
                .default
                .cache_path
                .or(self.cache_path)
                .unwrap_or(defaults.cache),
        }
    }
}

pub fn load_config(root: &Utf8Path) -> Result<TamaConfig> {
    let path = root.join("tama.toml");
    parse_tama_config(&path)
}

pub fn parse_tama_config(path: &Utf8Path) -> Result<TamaConfig> {
    let text = read_to_string(path)?;
    let config = toml::from_str(&text).map_err(|source| Error::Toml {
        path: path.to_owned(),
        source,
    })?;
    validate_tama_config(&config)?;
    Ok(config)
}

fn validate_tama_config(config: &TamaConfig) -> Result<()> {
    for (field, path) in [
        ("paths.src", &config.paths.src),
        ("paths.spec", &config.paths.spec),
        ("paths.proof", &config.paths.proof),
        ("paths.mirror_test", &config.paths.test),
        ("paths.out", &config.paths.out),
        ("paths.generated_solidity", &config.paths.generated),
    ] {
        validate_project_relative_path(field, path)?;
    }
    for (key, reason) in &config.coverage.proof_only {
        if reason.trim().is_empty() {
            return Err(Error::EmptyProofOnlyReason {
                key: key.to_owned(),
            });
        }
    }
    if let Some(modules) = &config.modules {
        for (field, name) in [
            ("modules.src", modules.src.as_str()),
            ("modules.spec", modules.spec.as_str()),
            ("modules.proof", modules.proof.as_str()),
        ] {
            validate_lean_module_name(field, name)?;
        }
    }
    Ok(())
}

impl TamaConfig {
    pub fn module_root(&self, kind: ModuleKind) -> &str {
        match (&self.modules, kind) {
            (Some(modules), ModuleKind::Src) => &modules.src,
            (Some(modules), ModuleKind::Spec) => &modules.spec,
            (Some(modules), ModuleKind::Proof) => &modules.proof,
            (None, ModuleKind::Src) => "src",
            (None, ModuleKind::Spec) => "spec",
            (None, ModuleKind::Proof) => "proof",
        }
    }

    pub fn module_path(&self, kind: ModuleKind) -> &Utf8Path {
        match kind {
            ModuleKind::Src => &self.paths.src,
            ModuleKind::Spec => &self.paths.spec,
            ModuleKind::Proof => &self.paths.proof,
        }
    }

    pub fn aggregate_module(&self, kind: ModuleKind) -> AggregateModule {
        if self.modules.is_some() {
            let module = self.module_root(kind).to_string();
            AggregateModule {
                path: self.module_path(kind).join(lean_module_file(&module)),
                module,
            }
        } else {
            match kind {
                ModuleKind::Src => AggregateModule {
                    module: "TamaSrc".to_string(),
                    path: "TamaSrc.lean".into(),
                },
                ModuleKind::Spec => AggregateModule {
                    module: "TamaSpec".to_string(),
                    path: "TamaSpec.lean".into(),
                },
                ModuleKind::Proof => AggregateModule {
                    module: "TamaProof".to_string(),
                    path: "TamaProof.lean".into(),
                },
            }
        }
    }

    pub fn check_targets(&self) -> [String; 2] {
        [
            self.aggregate_module(ModuleKind::Src).module,
            self.aggregate_module(ModuleKind::Spec).module,
        ]
    }

    pub fn proof_target(&self) -> String {
        self.aggregate_module(ModuleKind::Proof).module
    }

    pub fn module_child_dir(&self, kind: ModuleKind) -> Utf8PathBuf {
        if self.modules.is_some() {
            self.module_path(kind)
                .join(lean_module_dir(self.module_root(kind)))
        } else {
            self.module_path(kind).to_path_buf()
        }
    }

    pub fn lean_module_for_path(
        &self,
        root: &Utf8Path,
        kind: ModuleKind,
        path: &Utf8Path,
    ) -> Result<String> {
        let base_dir = root.join(self.module_path(kind));
        let relative = path
            .strip_prefix(&base_dir)
            .map_err(|_| Error::ModulePathOutsideRoot {
                path: path.to_owned(),
                base_dir: base_dir.clone(),
            })?;
        let relative = relative.with_extension("");
        let mut parts = if self.modules.is_some() {
            Vec::new()
        } else {
            vec![self.module_root(kind).to_string()]
        };
        for component in relative.components() {
            match component {
                Utf8Component::Normal(part) => parts.push(part.to_string()),
                Utf8Component::CurDir => {}
                other => {
                    return Err(Error::InvalidModulePathComponent {
                        path: path.to_owned(),
                        component: other.as_str().to_string(),
                    });
                }
            }
        }
        let module = parts.join(".");
        if self.modules.is_some() {
            let root_module = self.module_root(kind);
            if module != root_module
                && !module
                    .strip_prefix(root_module)
                    .is_some_and(|suffix| suffix.starts_with('.'))
            {
                return Err(Error::ModuleOutsideRoot {
                    path: path.to_owned(),
                    module_root: root_module.to_string(),
                });
            }
        }
        Ok(module)
    }
}

pub fn validate_lean_module_name(field: &'static str, name: &str) -> Result<()> {
    if is_lean_module_name(name) {
        Ok(())
    } else {
        Err(Error::InvalidModuleName {
            field,
            name: name.to_string(),
        })
    }
}

pub fn is_lean_module_name(value: &str) -> bool {
    let mut segment_count = 0;
    for segment in value.split('.') {
        segment_count += 1;
        if !is_lean_identifier(segment) {
            return false;
        }
    }
    segment_count > 0
}

fn is_lean_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '\'')
}

fn lean_module_dir(module: &str) -> Utf8PathBuf {
    let mut path = Utf8PathBuf::new();
    for component in module.split('.') {
        path.push(component);
    }
    path
}

fn lean_module_file(module: &str) -> Utf8PathBuf {
    let mut path = lean_module_dir(module);
    path.set_extension("lean");
    path
}

pub fn validate_project_relative_path(field: &'static str, path: &Utf8Path) -> Result<()> {
    let has_directory_name = path
        .components()
        .any(|component| matches!(component, Utf8Component::Normal(_)));
    let unsafe_component = path.components().any(|component| {
        matches!(
            component,
            Utf8Component::ParentDir | Utf8Component::RootDir | Utf8Component::Prefix(_)
        )
    });
    if path.as_str().is_empty() || path.is_absolute() || !has_directory_name || unsafe_component {
        Err(Error::InvalidPath {
            field,
            path: path.to_owned(),
        })
    } else {
        Ok(())
    }
}

pub fn load_lock(root: &Utf8Path) -> Result<TamaLock> {
    let path = root.join("tama.lock");
    let text = read_to_string(&path)?;
    let lock: TamaLock = toml::from_str(&text).map_err(|source| Error::Toml { path, source })?;
    if lock.version != 1 {
        return Err(Error::UnsupportedLockVersion(lock.version));
    }
    Ok(lock)
}

pub fn write_lock(root: &Utf8Path, lock: &TamaLock) -> Result<()> {
    let path = root.join("tama.lock");
    let text = toml::to_string_pretty(lock)
        .map_err(|source| Error::StaleLock(format!("failed to serialize lockfile: {source}")))?;
    tama_common::write_string(&path, &text)?;
    Ok(())
}

pub fn parse_foundry_config(root: &Utf8Path) -> Result<FoundryConfig> {
    let path = root.join("foundry.toml");
    if !path.exists() {
        return Ok(FoundryConfig::default());
    }
    let text = read_to_string(&path)?;
    let foundry: FoundryToml =
        toml::from_str(&text).map_err(|source| Error::Toml { path, source })?;
    let config = foundry.into_config();
    validate_foundry_config(&config)?;
    Ok(config)
}

fn validate_foundry_config(config: &FoundryConfig) -> Result<()> {
    for (field, path) in [
        ("foundry.src", &config.src),
        ("foundry.test", &config.test),
        ("foundry.out", &config.out),
        ("foundry.cache_path", &config.cache),
    ] {
        validate_project_relative_path(field, path)?;
    }
    Ok(())
}

pub fn parse_lake_build_dir(root: &Utf8Path) -> Result<Option<Utf8PathBuf>> {
    let path = lakefile_toml_path(root)?;
    let text = read_to_string(&path)?;
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|source| Error::StaleLock(format!("failed to parse {path}: {source}")))?;
    Ok(doc
        .get("buildDir")
        .and_then(Item::as_str)
        .map(Utf8PathBuf::from))
}

pub fn lake_package_name(root: &Utf8Path) -> Result<String> {
    let path = lakefile_toml_path(root)?;
    let text = read_to_string(&path)?;
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|source| Error::StaleLock(format!("failed to parse {path}: {source}")))?;
    let name = doc
        .get("name")
        .and_then(Item::as_str)
        .filter(|name| is_safe_dependency_name(name))
        .ok_or_else(|| {
            Error::UnsupportedLakefile("missing or invalid root package name".to_string())
        })?;
    Ok(name.to_string())
}

pub fn read_lean_toolchain(root: &Utf8Path) -> Result<String> {
    Ok(read_to_string(&root.join("lean-toolchain"))?
        .trim()
        .to_string())
}

pub fn tracked_input_hashes(root: &Utf8Path) -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    let mut tracked = vec![
        "tama.toml",
        "lakefile.toml",
        "lake-manifest.json",
        "foundry.toml",
        "lean-toolchain",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    if root.join("tama.toml").is_file() {
        match load_config(root) {
            Ok(config) if config.modules.is_some() => {
                tracked.extend(
                    [ModuleKind::Src, ModuleKind::Spec, ModuleKind::Proof]
                        .into_iter()
                        .map(|kind| config.aggregate_module(kind).path.to_string()),
                );
            }
            Ok(_) | Err(_) => {
                tracked
                    .extend(["TamaSrc.lean", "TamaSpec.lean", "TamaProof.lean"].map(String::from));
            }
        }
    } else {
        tracked.extend(["TamaSrc.lean", "TamaSpec.lean", "TamaProof.lean"].map(String::from));
    }
    for rel in tracked {
        let path = root.join(&rel);
        if path.is_file() {
            hashes.insert(rel, sha256_file(&path)?);
        }
    }
    Ok(hashes)
}

pub fn lock_drift(root: &Utf8Path, lock: &TamaLock) -> Result<Vec<String>> {
    let actual = tracked_input_hashes(root)?;
    let mut drift = Vec::new();
    for (path, hash) in &actual {
        if lock.inputs.get(path) != Some(hash) {
            drift.push(path.clone());
        }
    }
    for path in lock.inputs.keys() {
        if !actual.contains_key(path) {
            drift.push(path.clone());
        }
    }
    let lake_manifest_current = actual
        .get("lake-manifest.json")
        .is_some_and(|hash| lock.inputs.get("lake-manifest.json") == Some(hash));
    if lake_manifest_current {
        let lake_resolved = lake_manifest_resolutions(root)?;
        for (key, value) in &lake_resolved {
            if lock.resolved.get(key) != Some(value) {
                drift.push(format!("resolved.{key}"));
            }
        }
        for key in lock.resolved.keys().filter(|key| key.starts_with("lake.")) {
            if !lake_resolved.contains_key(key) {
                drift.push(format!("resolved.{key}"));
            }
        }
    }
    let tama_toml_current = actual
        .get("tama.toml")
        .is_some_and(|hash| lock.inputs.get("tama.toml") == Some(hash));
    if tama_toml_current {
        let config = load_config(root)?;
        let verity_rev = verity_rev_from_config(&config.project.verity);
        if lock.resolved.get("verity_rev") != Some(&verity_rev) {
            drift.push("resolved.verity_rev".to_string());
        }
        if let Some(lake_git) = lock.resolved.get("lake.verity.url") {
            if lock.resolved.get("verity_git") != Some(lake_git) {
                drift.push("resolved.verity_git".to_string());
            }
        }
        let yul = yul_lock_entries(&config.yul);
        if lock.yul != yul {
            drift.push("yul".to_string());
        }
        if lock.resolved.get("solc") != Some(&config.yul.solc) {
            drift.push("resolved.solc".to_string());
        }
    }
    let lean_toolchain_current = actual
        .get("lean-toolchain")
        .is_some_and(|hash| lock.inputs.get("lean-toolchain") == Some(hash));
    if lean_toolchain_current {
        let toolchain = read_lean_toolchain(root)?;
        if lock.resolved.get("lean_toolchain") != Some(&toolchain) {
            drift.push("resolved.lean_toolchain".to_string());
        }
    }
    Ok(drift)
}

pub fn enforce_locked(root: &Utf8Path, lock: &TamaLock) -> Result<()> {
    let drift = lock_drift(root, lock)?;
    if drift.is_empty() {
        Ok(())
    } else {
        Err(Error::StaleLock(drift.join(", ")))
    }
}

pub fn update_lock_inputs(root: &Utf8Path, lock: &mut TamaLock) -> Result<()> {
    record_lake_manifest_resolutions(root, lock)?;
    record_project_resolution(root, lock)?;
    record_lean_toolchain(root, lock)?;
    record_yul_config(root, lock)?;
    lock.inputs = tracked_input_hashes(root)?;
    Ok(())
}

pub fn record_project_resolution(root: &Utf8Path, lock: &mut TamaLock) -> Result<()> {
    if !root.join("tama.toml").is_file() {
        lock.resolved.remove("verity_rev");
        lock.resolved.remove("verity_git");
        return Ok(());
    }
    let config = load_config(root)?;
    lock.resolved.insert(
        "verity_rev".to_string(),
        verity_rev_from_config(&config.project.verity),
    );
    if let Some(url) = lock.resolved.get("lake.verity.url").cloned() {
        lock.resolved.insert("verity_git".to_string(), url);
    }
    Ok(())
}

pub fn record_lean_toolchain(root: &Utf8Path, lock: &mut TamaLock) -> Result<()> {
    if !root.join("lean-toolchain").is_file() {
        lock.resolved.remove("lean_toolchain");
        return Ok(());
    }
    lock.resolved
        .insert("lean_toolchain".to_string(), read_lean_toolchain(root)?);
    Ok(())
}

pub fn record_yul_config(root: &Utf8Path, lock: &mut TamaLock) -> Result<()> {
    if !root.join("tama.toml").is_file() {
        lock.yul.clear();
        return Ok(());
    }
    let config = load_config(root)?;
    lock.resolved
        .insert("solc".to_string(), config.yul.solc.clone());
    lock.yul = yul_lock_entries(&config.yul);
    Ok(())
}

pub fn record_lake_manifest_resolutions(root: &Utf8Path, lock: &mut TamaLock) -> Result<()> {
    let resolved = lake_manifest_resolutions(root)?;
    lock.resolved.retain(|key, _| !key.starts_with("lake."));
    lock.resolved.extend(resolved);
    Ok(())
}

fn lake_manifest_resolutions(root: &Utf8Path) -> Result<BTreeMap<String, String>> {
    let path = root.join("lake-manifest.json");
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let text = read_to_string(&path)?;
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|source| Error::Json { path, source })?;
    let mut resolved = BTreeMap::new();
    let Some(packages) = manifest
        .get("packages")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(resolved);
    };
    for package in packages {
        if package
            .get("inherited")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            continue;
        }
        if package.get("type").and_then(serde_json::Value::as_str) != Some("git") {
            continue;
        }
        let Some(name) = package.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !valid_lock_component(name) {
            continue;
        }
        if let Some(url) = package.get("url").and_then(serde_json::Value::as_str) {
            resolved.insert(format!("lake.{name}.url"), url.to_string());
        }
        if let Some(rev) = package.get("rev").and_then(serde_json::Value::as_str) {
            resolved.insert(format!("lake.{name}.rev"), rev.to_string());
        }
        if let Some(input_rev) = package.get("inputRev").and_then(serde_json::Value::as_str) {
            resolved.insert(format!("lake.{name}.input_rev"), input_rev.to_string());
        }
    }
    Ok(resolved)
}

fn valid_lock_component(name: &str) -> bool {
    is_safe_dependency_name(name)
}

fn yul_lock_entries(yul: &YulConfig) -> BTreeMap<String, toml::Value> {
    BTreeMap::from([
        (
            "evm_version".to_string(),
            toml::Value::String(yul.evm_version.clone()),
        ),
        (
            "metadata_bytecode_hash".to_string(),
            toml::Value::String(yul.metadata_hash.clone()),
        ),
        ("optimizer".to_string(), toml::Value::Boolean(yul.optimizer)),
        (
            "optimizer_runs".to_string(),
            toml::Value::Integer(i64::from(yul.optimizer_runs)),
        ),
        (
            "yul_optimizer".to_string(),
            toml::Value::Boolean(yul.yul_optimizer),
        ),
    ])
}

fn is_safe_dependency_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn verity_rev_from_config(version: &str) -> String {
    if version.starts_with('v') || looks_like_git_rev(version) {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

fn looks_like_git_rev(value: &str) -> bool {
    value.len() >= 7 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub fn parse_lake_dependency(root: &Utf8Path, raw: &str) -> Result<LakeDependency> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::InvalidDependencySpec(raw.to_string()));
    }
    if looks_like_local_path(raw) {
        let path = Utf8PathBuf::from(raw);
        let resolved = if path.is_absolute() {
            path.clone()
        } else {
            root.join(&path)
        };
        if !resolved.join("tama.toml").is_file() {
            return Err(Error::InvalidDependencySpec(format!(
                "local dependency `{raw}` does not contain tama.toml; pure Lake packages are outside `tama install`'s scope, so add this dependency manually to lakefile.toml"
            )));
        }
        let name = dependency_name_from_path(&path)?;
        return Ok(LakeDependency {
            name,
            source: LakeDependencySource::Path { path },
        });
    }

    let (repo, rev) = split_repo_rev(raw);
    let url = if repo.starts_with("https://")
        || repo.starts_with("http://")
        || repo.starts_with("git@")
        || repo.ends_with(".git")
    {
        repo.to_string()
    } else if repo.split('/').count() == 2 {
        format!("https://github.com/{repo}.git")
    } else {
        return Err(Error::InvalidDependencySpec(raw.to_string()));
    };
    let name = dependency_name_from_repo(repo)?;
    Ok(LakeDependency {
        name,
        source: LakeDependencySource::Git {
            url,
            rev: rev.unwrap_or("main").to_string(),
        },
    })
}

pub fn lake_dependency(root: &Utf8Path, name: &str) -> Result<LakeDependency> {
    let path = lakefile_toml_path(root)?;
    let text = read_to_string(&path)?;
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|source| Error::StaleLock(format!("failed to parse {path}: {source}")))?;
    let Some(requires) = doc["require"].as_array_of_tables() else {
        return Err(Error::DependencyNotFound(name.to_string()));
    };
    for table in requires {
        if table_string(table, "name") == Some(name) {
            return dependency_from_table(table, name);
        }
    }
    Err(Error::DependencyNotFound(name.to_string()))
}

pub fn upsert_lake_dependency(root: &Utf8Path, dependency: &LakeDependency) -> Result<()> {
    let path = lakefile_toml_path(root)?;
    preserve_toml_edit(&path, |doc| {
        let requires = require_array_mut(doc)?;
        for table in requires.iter_mut() {
            if table_string(table, "name") == Some(dependency.name.as_str()) {
                set_dependency_table(table, dependency);
                return Ok(());
            }
        }
        let mut table = Table::new();
        set_dependency_table(&mut table, dependency);
        requires.push(table);
        Ok(())
    })
}

pub fn remove_lake_dependency(root: &Utf8Path, name: &str) -> Result<()> {
    let path = lakefile_toml_path(root)?;
    let mut removed = false;
    preserve_toml_edit(&path, |doc| {
        if let Some(requires) = existing_require_array_mut(doc)? {
            requires.retain(|table| {
                let keep = table_string(table, "name") != Some(name);
                if !keep {
                    removed = true;
                }
                keep
            });
        }
        Ok(())
    })?;
    if removed {
        Ok(())
    } else {
        Err(Error::DependencyNotFound(name.to_string()))
    }
}

pub fn preserve_toml_edit(
    path: &Utf8Path,
    edit: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>,
) -> Result<()> {
    let text = read_to_string(path)?;
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|source| Error::StaleLock(format!("failed to parse {path}: {source}")))?;
    edit(&mut doc)?;
    fs::write(path, doc.to_string())
        .map_err(|source| tama_common::io_error(path.to_owned(), source))?;
    Ok(())
}

fn lakefile_toml_path(root: &Utf8Path) -> Result<Utf8PathBuf> {
    let path = root.join("lakefile.toml");
    if path.is_file() {
        Ok(path)
    } else if root.join("lakefile.lean").is_file() {
        Err(Error::UnsupportedLakefile(
            "lakefile.lean projects must edit Lake dependencies manually".to_string(),
        ))
    } else {
        Err(Error::UnsupportedLakefile(
            "missing lakefile.toml".to_string(),
        ))
    }
}

fn require_array_mut(doc: &mut toml_edit::DocumentMut) -> Result<&mut ArrayOfTables> {
    if has_no_require_item(doc) {
        doc["require"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    existing_require_array_mut(doc)?.ok_or_else(|| {
        Error::UnsupportedLakefile("failed to initialize `require` array of tables".to_string())
    })
}

fn existing_require_array_mut(
    doc: &mut toml_edit::DocumentMut,
) -> Result<Option<&mut ArrayOfTables>> {
    match doc.get_mut("require") {
        None | Some(Item::None) => Ok(None),
        Some(Item::ArrayOfTables(requires)) => Ok(Some(requires)),
        Some(_) => Err(Error::UnsupportedLakefile(
            "`require` must be an array of tables (`[[require]]`)".to_string(),
        )),
    }
}

fn has_no_require_item(doc: &toml_edit::DocumentMut) -> bool {
    matches!(doc.get("require"), None | Some(Item::None))
}

fn set_dependency_table(table: &mut Table, dependency: &LakeDependency) {
    table["name"] = value(dependency.name.clone());
    match &dependency.source {
        LakeDependencySource::Git { url, rev } => {
            table.remove("path");
            table["git"] = value(url.clone());
            table["rev"] = value(rev.clone());
        }
        LakeDependencySource::Path { path } => {
            table.remove("git");
            table.remove("rev");
            table["path"] = value(path.to_string());
        }
    }
}

fn table_string<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(Item::as_str)
}

fn dependency_from_table(table: &Table, fallback_name: &str) -> Result<LakeDependency> {
    let name = table_string(table, "name").unwrap_or(fallback_name);
    if let Some(url) = table_string(table, "git") {
        return Ok(LakeDependency {
            name: name.to_string(),
            source: LakeDependencySource::Git {
                url: url.to_string(),
                rev: table_string(table, "rev").unwrap_or("main").to_string(),
            },
        });
    }
    if let Some(path) = table_string(table, "path") {
        return Ok(LakeDependency {
            name: name.to_string(),
            source: LakeDependencySource::Path {
                path: Utf8PathBuf::from(path),
            },
        });
    }
    Err(Error::InvalidDependencySpec(name.to_string()))
}

fn looks_like_local_path(raw: &str) -> bool {
    raw.starts_with('/')
        || raw.starts_with("./")
        || raw.starts_with("../")
        || raw == "."
        || raw == ".."
}

fn split_repo_rev(raw: &str) -> (&str, Option<&str>) {
    match raw.rsplit_once('@') {
        Some((repo, rev)) if split_at_is_explicit_rev(raw, repo, rev) => (repo, Some(rev)),
        _ => (raw, None),
    }
}

fn split_at_is_explicit_rev(raw: &str, repo: &str, rev: &str) -> bool {
    if repo.is_empty() || rev.is_empty() {
        return false;
    }
    if raw.starts_with("git@") {
        return repo.starts_with("git@") && repo.contains(':');
    }
    true
}

fn dependency_name_from_path(path: &Utf8Path) -> Result<String> {
    path.file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.trim_end_matches(".git").to_string())
        .ok_or_else(|| Error::InvalidDependencySpec(path.to_string()))
}

fn dependency_name_from_repo(repo: &str) -> Result<String> {
    let trimmed = repo.trim_end_matches('/').trim_end_matches(".git");
    trimmed
        .rsplit(['/', ':'])
        .next()
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| Error::InvalidDependencySpec(repo.to_string()))
}

fn default_src() -> Utf8PathBuf {
    "verity/src".into()
}
fn default_spec() -> Utf8PathBuf {
    "verity/spec".into()
}
fn default_proof() -> Utf8PathBuf {
    "verity/proof".into()
}
fn default_test() -> Utf8PathBuf {
    "test/verity".into()
}
fn default_out() -> Utf8PathBuf {
    "artifacts".into()
}
fn default_generated() -> Utf8PathBuf {
    "src/generated/verity".into()
}
fn default_foundry_src() -> Utf8PathBuf {
    "src".into()
}
fn default_foundry_test() -> Utf8PathBuf {
    "test".into()
}
fn default_foundry_out() -> Utf8PathBuf {
    "out".into()
}
fn default_foundry_cache() -> Utf8PathBuf {
    "cache".into()
}
fn default_true() -> bool {
    true
}
fn default_optimizer_runs() -> u32 {
    200
}
fn default_evm() -> String {
    "cancun".to_string()
}
fn default_metadata_hash() -> String {
    "none".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_config_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("tama.toml")).unwrap();
        tama_common::write_string(
            &path,
            r#"
[project]
name = "my-protocol"
verity = "0.1.0"

[yul]
solc = "0.8.33"

[trust.allow_axioms]
"Classical.choice" = "Lean classical reasoning"

[coverage.proof_only]
"Foo.symbolic_only" = "quantifies over all key pairs"
"#,
        )
        .unwrap();
        let cfg = parse_tama_config(&path).unwrap();
        assert_eq!(cfg.paths.src, Utf8PathBuf::from("verity/src"));
        assert_eq!(cfg.yul.optimizer_runs, 200);
        assert!(cfg.yul.yul_optimizer);
        assert!(cfg.trust.allow_axioms.contains_key("Classical.choice"));
        assert_eq!(
            cfg.coverage
                .proof_only
                .get("Foo.symbolic_only")
                .map(String::as_str),
            Some("quantifies over all key pairs")
        );
    }

    #[test]
    fn empty_proof_only_reason_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("tama.toml")).unwrap();
        tama_common::write_string(
            &path,
            r#"
[project]
name = "p"
verity = "0.1.0"

[yul]
solc = "0.8.33"

[coverage.proof_only]
"Foo.bar" = "   "
"#,
        )
        .unwrap();
        match parse_tama_config(&path) {
            Err(Error::EmptyProofOnlyReason { key }) => assert_eq!(key, "Foo.bar"),
            other => panic!("expected EmptyProofOnlyReason, got {other:?}"),
        }
    }

    #[test]
    fn spec_config_keys_and_legacy_aliases_parse() {
        for (paths, metadata_key) in [
            (
                r#"
[paths]
mirror_test = "tests/verity"
generated_solidity = "contracts/generated"
"#,
                "metadata_bytecode_hash",
            ),
            (
                r#"
[paths]
test = "tests/verity"
generated = "contracts/generated"
"#,
                "metadata_hash",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = Utf8PathBuf::from_path_buf(dir.path().join("tama.toml")).unwrap();
            tama_common::write_string(
                &path,
                &format!(
                    r#"[project]
name = "my-protocol"
verity = "0.1.0"
{paths}
[yul]
solc = "0.8.33"
{metadata_key} = "none"
"#
                ),
            )
            .unwrap();

            let cfg = parse_tama_config(&path).unwrap();

            assert_eq!(cfg.paths.test, Utf8PathBuf::from("tests/verity"));
            assert_eq!(
                cfg.paths.generated,
                Utf8PathBuf::from("contracts/generated")
            );
            assert_eq!(cfg.yul.metadata_hash, "none");
        }
    }

    #[test]
    fn tama_config_rejects_unknown_keys() {
        for body in [
            r#"[project]
name = "my-protocol"
verity = "0.1.0"

[yul]
solc = "0.8.33"

[unexpected]
value = true
"#,
            r#"[project]
name = "my-protocol"
verity = "0.1.0"

[paths]
generated_solidity_typo = "src/generated/verity"

[yul]
solc = "0.8.33"
"#,
            r#"[project]
name = "my-protocol"
verity = "0.1.0"

[yul]
solc = "0.8.33"
optimizer_runz = 200
"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = Utf8PathBuf::from_path_buf(dir.path().join("tama.toml")).unwrap();
            tama_common::write_string(&path, body).unwrap();

            let err = parse_tama_config(&path).unwrap_err();

            assert!(matches!(err, Error::Toml { .. }));
            assert!(err.to_string().contains("unknown field"));
        }
    }

    #[test]
    fn tama_config_rejects_unsafe_project_paths() {
        for (path_key, path_value) in [
            ("src", ""),
            ("spec", "."),
            ("proof", "../proof"),
            ("mirror_test", "/tmp/tests"),
            ("out", "artifacts/../escape"),
            ("generated_solidity", "../generated"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = Utf8PathBuf::from_path_buf(dir.path().join("tama.toml")).unwrap();
            tama_common::write_string(
                &path,
                &format!(
                    r#"[project]
name = "my-protocol"
verity = "0.1.0"

[paths]
{path_key} = "{path_value}"

[yul]
solc = "0.8.33"
"#
                ),
            )
            .unwrap();

            let err = parse_tama_config(&path).unwrap_err();

            assert!(matches!(
                err,
                Error::InvalidPath { field, .. } if field.starts_with("paths.")
            ));
        }
    }

    #[test]
    fn foundry_profile_default_paths_are_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("foundry.toml"),
            r#"[profile.default]
src = "contracts"
test = "tests"
out = "build/out"
cache_path = "build/cache"
"#,
        )
        .unwrap();

        let foundry = parse_foundry_config(&root).unwrap();

        assert_eq!(foundry.src, Utf8PathBuf::from("contracts"));
        assert_eq!(foundry.test, Utf8PathBuf::from("tests"));
        assert_eq!(foundry.out, Utf8PathBuf::from("build/out"));
        assert_eq!(foundry.cache, Utf8PathBuf::from("build/cache"));
    }

    #[test]
    fn foundry_profile_paths_override_root_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("foundry.toml"),
            r#"src = "root-src"
test = "root-test"
out = "root-out"
cache_path = "root-cache"

[profile.default]
test = "profile-test"
cache_path = "profile-cache"
"#,
        )
        .unwrap();

        let foundry = parse_foundry_config(&root).unwrap();

        assert_eq!(foundry.src, Utf8PathBuf::from("root-src"));
        assert_eq!(foundry.test, Utf8PathBuf::from("profile-test"));
        assert_eq!(foundry.out, Utf8PathBuf::from("root-out"));
        assert_eq!(foundry.cache, Utf8PathBuf::from("profile-cache"));
    }

    #[test]
    fn foundry_config_rejects_unsafe_project_paths() {
        for (path_key, path_value) in [
            ("src", ""),
            ("test", "."),
            ("out", "../out"),
            ("cache_path", "/tmp/cache"),
            ("profile.default.out", "build/../out"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
            let config = if let Some(profile_key) = path_key.strip_prefix("profile.default.") {
                format!(
                    r#"[profile.default]
{profile_key} = "{path_value}"
"#
                )
            } else {
                format!("{path_key} = \"{path_value}\"\n")
            };
            tama_common::write_string(&root.join("foundry.toml"), &config).unwrap();

            let err = parse_foundry_config(&root).unwrap_err();

            assert!(matches!(
                err,
                Error::InvalidPath { field, .. } if field.starts_with("foundry.")
            ));
        }
    }

    #[test]
    fn config_parses_package_root_modules_and_aggregate_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let path = root.join("tama.toml");
        tama_common::write_string(
            &path,
            r#"[project]
name = "tamago"
verity = "v"

[paths]
src = "verity/src"
spec = "verity/spec"
proof = "verity/proof"

[modules]
src = "Tamago"
spec = "Tamago.Spec"
proof = "Tamago.Proof"

[yul]
solc = "0.8.33"
"#,
        )
        .unwrap();

        let config = parse_tama_config(&path).unwrap();

        assert_eq!(config.check_targets(), ["Tamago", "Tamago.Spec"]);
        assert_eq!(config.proof_target(), "Tamago.Proof");
        assert_eq!(
            config.aggregate_module(ModuleKind::Src).path,
            Utf8PathBuf::from("verity/src/Tamago.lean")
        );
        assert_eq!(
            config.aggregate_module(ModuleKind::Spec).path,
            Utf8PathBuf::from("verity/spec/Tamago/Spec.lean")
        );
        assert_eq!(
            config.module_child_dir(ModuleKind::Proof),
            Utf8PathBuf::from("verity/proof/Tamago/Proof")
        );
    }

    #[test]
    fn config_rejects_invalid_module_roots() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("tama.toml")).unwrap();
        tama_common::write_string(
            &path,
            r#"[project]
name = "x"
verity = "v"

[modules]
src = "Tamago"
spec = "Tamago.bad-name"
proof = "Tamago.Proof"

[yul]
solc = "0.8.33"
"#,
        )
        .unwrap();

        let err = parse_tama_config(&path).unwrap_err();

        assert!(matches!(
            err,
            Error::InvalidModuleName {
                field: "modules.spec",
                ..
            }
        ));
    }

    #[test]
    fn configured_lock_inputs_track_configured_aggregates() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            r#"[project]
name = "tamago"
verity = "v"

[modules]
src = "Tamago"
spec = "Tamago.Spec"
proof = "Tamago.Proof"

[yul]
solc = "0.8.33"
"#,
        )
        .unwrap();
        for path in [
            "verity/src/Tamago.lean",
            "verity/spec/Tamago/Spec.lean",
            "verity/proof/Tamago/Proof.lean",
            "TamaSrc.lean",
        ] {
            tama_common::write_string(&root.join(path), "tracked input\n").unwrap();
        }

        let hashes = tracked_input_hashes(&root).unwrap();

        assert!(hashes.contains_key("verity/src/Tamago.lean"));
        assert!(hashes.contains_key("verity/spec/Tamago/Spec.lean"));
        assert!(hashes.contains_key("verity/proof/Tamago/Proof.lean"));
        assert!(!hashes.contains_key("TamaSrc.lean"));
    }

    #[test]
    fn locked_detects_input_drift() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname='x'\nverity='v'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        let mut lock = TamaLock {
            version: 1,
            resolved: BTreeMap::new(),
            inputs: BTreeMap::new(),
            yul: BTreeMap::new(),
        };
        update_lock_inputs(&root, &mut lock).unwrap();
        assert!(enforce_locked(&root, &lock).is_ok());
        tama_common::write_string(&root.join("tama.toml"), "changed").unwrap();
        assert!(matches!(
            enforce_locked(&root, &lock),
            Err(Error::StaleLock(_))
        ));
    }

    #[test]
    fn locked_detects_every_tracked_input_change_and_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let tracked = [
            "tama.toml",
            "lakefile.toml",
            "lake-manifest.json",
            "foundry.toml",
            "lean-toolchain",
            "TamaSrc.lean",
            "TamaSpec.lean",
            "TamaProof.lean",
        ];
        for path in tracked {
            tama_common::write_string(&root.join(path), tracked_input_fixture(path)).unwrap();
        }
        let mut lock = TamaLock {
            version: 1,
            resolved: BTreeMap::new(),
            inputs: BTreeMap::new(),
            yul: BTreeMap::new(),
        };
        update_lock_inputs(&root, &mut lock).unwrap();
        assert!(enforce_locked(&root, &lock).is_ok());

        for path in tracked {
            tama_common::write_string(&root.join(path), &format!("{path}\nchanged\n")).unwrap();
            let drift = lock_drift(&root, &lock).unwrap();
            assert!(
                drift.contains(&path.to_string()),
                "expected drift for {path}"
            );
            tama_common::write_string(&root.join(path), tracked_input_fixture(path)).unwrap();
        }

        std::fs::remove_file(root.join("TamaProof.lean")).unwrap();
        let drift = lock_drift(&root, &lock).unwrap();
        assert!(drift.contains(&"TamaProof.lean".to_string()));
        assert!(matches!(
            enforce_locked(&root, &lock),
            Err(Error::StaleLock(_))
        ));
    }

    fn tracked_input_fixture(path: &str) -> &'static str {
        match path {
            "tama.toml" => {
                "[project]\nname='x'\nverity='v'\n[yul]\nsolc='0.8.33'\nmetadata_bytecode_hash='none'\n"
            }
            "lake-manifest.json" => r#"{"version":"1.1.0","packages":[]}"#,
            _ => "tracked input\n",
        }
    }

    #[test]
    fn update_lock_inputs_records_yul_config_and_solc_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            r#"[project]
name = "x"
verity = "v"

[yul]
solc = "0.8.34"
optimizer = false
optimizer_runs = 1
yul_optimizer = false
evm_version = "paris"
metadata_bytecode_hash = "ipfs"
"#,
        )
        .unwrap();
        let mut lock = TamaLock {
            version: 1,
            resolved: BTreeMap::new(),
            inputs: BTreeMap::new(),
            yul: BTreeMap::new(),
        };

        update_lock_inputs(&root, &mut lock).unwrap();

        assert_eq!(
            lock.resolved.get("solc").map(String::as_str),
            Some("0.8.34")
        );
        assert_eq!(
            lock.yul.get("evm_version"),
            Some(&toml::Value::String("paris".to_string()))
        );
        assert_eq!(
            lock.yul.get("metadata_bytecode_hash"),
            Some(&toml::Value::String("ipfs".to_string()))
        );
        assert_eq!(
            lock.yul.get("optimizer"),
            Some(&toml::Value::Boolean(false))
        );
        assert_eq!(
            lock.yul.get("optimizer_runs"),
            Some(&toml::Value::Integer(1))
        );
        assert_eq!(
            lock.yul.get("yul_optimizer"),
            Some(&toml::Value::Boolean(false))
        );
        assert!(lock_drift(&root, &lock).unwrap().is_empty());

        lock.yul.clear();
        let drift = lock_drift(&root, &lock).unwrap();

        assert!(drift.contains(&"yul".to_string()));
    }

    #[test]
    fn update_lock_inputs_records_lean_toolchain_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(&root.join("lean-toolchain"), "leanprover/lean4:v4.22.0\n")
            .unwrap();
        let mut lock = TamaLock {
            version: 1,
            resolved: BTreeMap::new(),
            inputs: BTreeMap::new(),
            yul: BTreeMap::new(),
        };

        update_lock_inputs(&root, &mut lock).unwrap();

        assert_eq!(
            lock.resolved.get("lean_toolchain").map(String::as_str),
            Some("leanprover/lean4:v4.22.0")
        );
        assert!(lock_drift(&root, &lock).unwrap().is_empty());

        lock.resolved.insert(
            "lean_toolchain".to_string(),
            "leanprover/lean4:v4.21.0".to_string(),
        );
        let drift = lock_drift(&root, &lock).unwrap();

        assert!(drift.contains(&"resolved.lean_toolchain".to_string()));
    }

    #[test]
    fn update_lock_inputs_records_project_verity_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("tama.toml"),
            "[project]\nname='x'\nverity='0.5.0'\n[yul]\nsolc='0.8.33'\n",
        )
        .unwrap();
        tama_common::write_string(
            &root.join("lake-manifest.json"),
            r#"{
  "packages": [{
    "name": "verity",
    "type": "git",
    "url": "https://github.com/lfglabs-dev/verity.git",
    "rev": "abc123",
    "inputRev": "v0.5.0",
    "inherited": false
  }]
}"#,
        )
        .unwrap();
        let mut lock = TamaLock {
            version: 1,
            resolved: BTreeMap::new(),
            inputs: BTreeMap::new(),
            yul: BTreeMap::new(),
        };

        update_lock_inputs(&root, &mut lock).unwrap();

        assert_eq!(verity_rev_from_config("9b0114e"), "9b0114e");
        assert_eq!(verity_rev_from_config("v0.5.0"), "v0.5.0");
        assert_eq!(
            lock.resolved.get("verity_rev").map(String::as_str),
            Some("v0.5.0")
        );
        assert_eq!(
            lock.resolved.get("verity_git").map(String::as_str),
            Some("https://github.com/lfglabs-dev/verity.git")
        );
        assert!(lock_drift(&root, &lock).unwrap().is_empty());

        lock.resolved
            .insert("verity_rev".to_string(), "v0.4.0".to_string());
        let drift = lock_drift(&root, &lock).unwrap();

        assert!(drift.contains(&"resolved.verity_rev".to_string()));
    }

    #[test]
    fn update_lock_inputs_records_direct_lake_manifest_resolutions() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("lake-manifest.json"),
            r#"{
  "packages": [
    {
      "name": "directpkg",
      "type": "git",
      "url": "https://example.test/direct.git",
      "rev": "abc123",
      "inputRev": "main",
      "inherited": false
    },
    {
      "name": "indirectpkg",
      "type": "git",
      "url": "https://example.test/indirect.git",
      "rev": "def456",
      "inputRev": "v1",
      "inherited": true
    }
  ]
}
"#,
        )
        .unwrap();
        let mut lock = TamaLock {
            version: 1,
            resolved: BTreeMap::from([("lake.stale.rev".to_string(), "old".to_string())]),
            inputs: BTreeMap::new(),
            yul: BTreeMap::new(),
        };

        update_lock_inputs(&root, &mut lock).unwrap();

        assert_eq!(
            lock.resolved.get("lake.directpkg.url").map(String::as_str),
            Some("https://example.test/direct.git")
        );
        assert_eq!(
            lock.resolved.get("lake.directpkg.rev").map(String::as_str),
            Some("abc123")
        );
        assert_eq!(
            lock.resolved
                .get("lake.directpkg.input_rev")
                .map(String::as_str),
            Some("main")
        );
        assert!(!lock.resolved.contains_key("lake.indirectpkg.rev"));
        assert!(!lock.resolved.contains_key("lake.stale.rev"));
        assert!(lock.inputs.contains_key("lake-manifest.json"));
        assert!(lock_drift(&root, &lock).unwrap().is_empty());

        lock.resolved
            .insert("lake.directpkg.rev".to_string(), "stale".to_string());
        let drift = lock_drift(&root, &lock).unwrap();

        assert!(drift.contains(&"resolved.lake.directpkg.rev".to_string()));
    }

    #[test]
    fn lake_dependency_edits_preserve_unrelated_toml() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("lakefile.toml"),
            r#"# project comment
name = "demo"

[[require]]
name = "verity"
git = "https://github.com/lfglabs-dev/verity.git"
rev = "old"

[leanOptions]
pp.unicode.fun = true
"#,
        )
        .unwrap();
        let dependency = LakeDependency {
            name: "mathlib".to_string(),
            source: LakeDependencySource::Git {
                url: "https://github.com/leanprover-community/mathlib4.git".to_string(),
                rev: "v4.22.0".to_string(),
            },
        };

        upsert_lake_dependency(&root, &dependency).unwrap();
        let edited = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        assert!(edited.contains("# project comment"));
        assert!(edited.contains("[leanOptions]"));
        assert!(edited.contains("pp.unicode.fun = true"));
        assert!(edited.contains("name = \"mathlib\""));
        assert!(edited.contains("rev = \"v4.22.0\""));

        remove_lake_dependency(&root, "mathlib").unwrap();
        let removed = tama_common::read_to_string(&root.join("lakefile.toml")).unwrap();
        assert!(removed.contains("# project comment"));
        assert!(removed.contains("[leanOptions]"));
        assert!(!removed.contains("name = \"mathlib\""));
    }

    #[test]
    fn lake_dependency_edits_reject_malformed_require_without_rewriting() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let lakefile = root.join("lakefile.toml");
        tama_common::write_string(
            &lakefile,
            r#"name = "demo"
require = "not a dependency list"
"#,
        )
        .unwrap();
        let original = tama_common::read_to_string(&lakefile).unwrap();
        let dependency = LakeDependency {
            name: "mathlib".to_string(),
            source: LakeDependencySource::Git {
                url: "https://github.com/leanprover-community/mathlib4.git".to_string(),
                rev: "v4.22.0".to_string(),
            },
        };

        let err = upsert_lake_dependency(&root, &dependency).unwrap_err();
        assert!(err
            .to_string()
            .contains("`require` must be an array of tables"));
        assert_eq!(tama_common::read_to_string(&lakefile).unwrap(), original);

        let err = remove_lake_dependency(&root, "mathlib").unwrap_err();
        assert!(err
            .to_string()
            .contains("`require` must be an array of tables"));
        assert_eq!(tama_common::read_to_string(&lakefile).unwrap(), original);
    }

    #[test]
    fn lake_dependency_reads_existing_require_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("lakefile.toml"),
            r#"[[require]]
name = "verity"
git = "https://github.com/lfglabs-dev/verity.git"
rev = "v1"

[[require]]
name = "localpkg"
path = "../localpkg"
"#,
        )
        .unwrap();

        assert_eq!(
            lake_dependency(&root, "verity").unwrap(),
            LakeDependency {
                name: "verity".to_string(),
                source: LakeDependencySource::Git {
                    url: "https://github.com/lfglabs-dev/verity.git".to_string(),
                    rev: "v1".to_string(),
                },
            }
        );
        assert_eq!(
            lake_dependency(&root, "localpkg").unwrap().source,
            LakeDependencySource::Path {
                path: "../localpkg".into(),
            }
        );
        assert!(matches!(
            lake_dependency(&root, "missing").unwrap_err(),
            Error::DependencyNotFound(name) if name == "missing"
        ));
    }

    #[test]
    fn lake_build_dir_reads_optional_root_setting() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("lakefile.toml"),
            r#"name = "demo"
buildDir = "build/lean"

[[require]]
name = "verity"
git = "https://github.com/lfglabs-dev/verity.git"
rev = "v1"
"#,
        )
        .unwrap();

        assert_eq!(
            parse_lake_build_dir(&root).unwrap(),
            Some(Utf8PathBuf::from("build/lean"))
        );

        tama_common::write_string(&root.join("lakefile.toml"), "name = \"demo\"\n").unwrap();
        assert_eq!(parse_lake_build_dir(&root).unwrap(), None);
    }

    #[test]
    fn lake_package_name_reads_safe_root_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(
            &root.join("lakefile.toml"),
            "name = \"utility_dep\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        assert_eq!(lake_package_name(&root).unwrap(), "utility_dep");

        tama_common::write_string(&root.join("lakefile.toml"), "name = \"bad/name\"\n").unwrap();
        assert!(matches!(
            lake_package_name(&root).unwrap_err(),
            Error::UnsupportedLakefile(message) if message.contains("package name")
        ));
    }

    #[test]
    fn path_dependency_without_tama_config_points_to_manual_install() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        let dependency = Utf8PathBuf::from_path_buf(dir.path().join("pure-lake")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        tama_common::write_string(
            &dependency.join("lakefile.toml"),
            "name = \"pure_lake_dep\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let err = parse_lake_dependency(&root, "../pure-lake").unwrap_err();

        assert!(matches!(
            err,
            Error::InvalidDependencySpec(message)
                if message.contains("local dependency `../pure-lake` does not contain tama.toml")
                    && message.contains("add this dependency manually to lakefile.toml")
        ));
    }

    #[test]
    fn git_scp_urls_are_not_split_at_user_host_marker() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let dependency =
            parse_lake_dependency(&root, "git@github.com:lfglabs-dev/verity.git").unwrap();
        assert_eq!(dependency.name, "verity");
        assert_eq!(
            dependency.source,
            LakeDependencySource::Git {
                url: "git@github.com:lfglabs-dev/verity.git".to_string(),
                rev: "main".to_string(),
            }
        );

        let dependency =
            parse_lake_dependency(&root, "git@github.com:lfglabs-dev/verity.git@v1").unwrap();
        assert_eq!(dependency.name, "verity");
        assert_eq!(
            dependency.source,
            LakeDependencySource::Git {
                url: "git@github.com:lfglabs-dev/verity.git".to_string(),
                rev: "v1".to_string(),
            }
        );
    }

    #[test]
    fn lakefile_lean_dependency_edits_fail_with_manual_instruction() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        tama_common::write_string(&root.join("lakefile.lean"), "import Lake\n").unwrap();
        let dependency = LakeDependency {
            name: "mathlib".to_string(),
            source: LakeDependencySource::Git {
                url: "https://github.com/leanprover-community/mathlib4.git".to_string(),
                rev: "v4.22.0".to_string(),
            },
        };

        let err = upsert_lake_dependency(&root, &dependency).unwrap_err();

        assert!(matches!(err, Error::UnsupportedLakefile(message) if message.contains("manually")));
    }
}
