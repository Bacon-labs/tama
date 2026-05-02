use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TamaConfig {
    pub project: ProjectConfig,
    #[serde(default)]
    pub paths: PathsConfig,
    pub yul: YulConfig,
    #[serde(default)]
    pub trust: TrustConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub verity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct YulConfig {
    pub solc: String,
    #[serde(default = "default_true")]
    pub optimizer: bool,
    #[serde(default = "default_optimizer_runs")]
    pub optimizer_runs: u32,
    #[serde(default = "default_evm")]
    pub evm_version: String,
    #[serde(default = "default_metadata_hash", alias = "metadata_bytecode_hash")]
    pub metadata_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustConfig {
    #[serde(default)]
    pub allow_axioms: BTreeMap<String, String>,
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
        }
    }
}

pub fn load_config(root: &Utf8Path) -> Result<TamaConfig> {
    let path = root.join("tama.toml");
    parse_tama_config(&path)
}

pub fn parse_tama_config(path: &Utf8Path) -> Result<TamaConfig> {
    let text = read_to_string(path)?;
    toml::from_str(&text).map_err(|source| Error::Toml {
        path: path.to_owned(),
        source,
    })
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
    toml::from_str(&text).map_err(|source| Error::Toml { path, source })
}

pub fn read_lean_toolchain(root: &Utf8Path) -> Result<String> {
    Ok(read_to_string(&root.join("lean-toolchain"))?
        .trim()
        .to_string())
}

pub fn tracked_input_hashes(root: &Utf8Path) -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    for rel in [
        "tama.toml",
        "lakefile.toml",
        "lake-manifest.json",
        "foundry.toml",
        "lean-toolchain",
        "TamaSrc.lean",
        "TamaSpec.lean",
        "TamaProof.lean",
    ] {
        let path = root.join(rel);
        if path.is_file() {
            hashes.insert(rel.to_string(), sha256_file(&path)?);
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
    lock.inputs = tracked_input_hashes(root)?;
    Ok(())
}

pub fn record_lake_manifest_resolutions(root: &Utf8Path, lock: &mut TamaLock) -> Result<()> {
    let path = root.join("lake-manifest.json");
    if !path.is_file() {
        return Ok(());
    }
    let text = read_to_string(&path)?;
    let manifest: serde_json::Value =
        serde_json::from_str(&text).map_err(|source| Error::Json { path, source })?;
    lock.resolved.retain(|key, _| !key.starts_with("lake."));
    let Some(packages) = manifest
        .get("packages")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
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
            lock.resolved
                .insert(format!("lake.{name}.url"), url.to_string());
        }
        if let Some(rev) = package.get("rev").and_then(serde_json::Value::as_str) {
            lock.resolved
                .insert(format!("lake.{name}.rev"), rev.to_string());
        }
        if let Some(input_rev) = package.get("inputRev").and_then(serde_json::Value::as_str) {
            lock.resolved
                .insert(format!("lake.{name}.input_rev"), input_rev.to_string());
        }
    }
    Ok(())
}

fn valid_lock_component(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
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
                "{raw} does not point to a Tama package with tama.toml"
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

pub fn upsert_lake_dependency(root: &Utf8Path, dependency: &LakeDependency) -> Result<()> {
    let path = lakefile_toml_path(root)?;
    preserve_toml_edit(&path, |doc| {
        let requires = require_array_mut(doc);
        for table in requires.iter_mut() {
            if table_string(table, "name") == Some(dependency.name.as_str()) {
                set_dependency_table(table, dependency);
                return;
            }
        }
        let mut table = Table::new();
        set_dependency_table(&mut table, dependency);
        requires.push(table);
    })
}

pub fn remove_lake_dependency(root: &Utf8Path, name: &str) -> Result<()> {
    let path = lakefile_toml_path(root)?;
    let mut removed = false;
    preserve_toml_edit(&path, |doc| {
        if let Some(requires) = doc["require"].as_array_of_tables_mut() {
            requires.retain(|table| {
                let keep = table_string(table, "name") != Some(name);
                if !keep {
                    removed = true;
                }
                keep
            });
        }
    })?;
    if removed {
        Ok(())
    } else {
        Err(Error::DependencyNotFound(name.to_string()))
    }
}

pub fn preserve_toml_edit(
    path: &Utf8Path,
    edit: impl FnOnce(&mut toml_edit::DocumentMut),
) -> Result<()> {
    let text = read_to_string(path)?;
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|source| Error::StaleLock(format!("failed to parse {path}: {source}")))?;
    edit(&mut doc);
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

fn require_array_mut(doc: &mut toml_edit::DocumentMut) -> &mut ArrayOfTables {
    if doc["require"].as_array_of_tables_mut().is_none() {
        doc["require"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    doc["require"]
        .as_array_of_tables_mut()
        .expect("require was initialized as array of tables")
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

fn looks_like_local_path(raw: &str) -> bool {
    raw.starts_with('/')
        || raw.starts_with("./")
        || raw.starts_with("../")
        || raw == "."
        || raw == ".."
}

fn split_repo_rev(raw: &str) -> (&str, Option<&str>) {
    match raw.rsplit_once('@') {
        Some((repo, rev)) if !repo.is_empty() && !rev.is_empty() && !repo.starts_with("git@") => {
            (repo, Some(rev))
        }
        _ => (raw, None),
    }
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
"#,
        )
        .unwrap();
        let cfg = parse_tama_config(&path).unwrap();
        assert_eq!(cfg.paths.src, Utf8PathBuf::from("verity/src"));
        assert_eq!(cfg.yul.optimizer_runs, 200);
        assert!(cfg.trust.allow_axioms.contains_key("Classical.choice"));
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
        if path == "lake-manifest.json" {
            r#"{"version":"1.1.0","packages":[]}"#
        } else {
            "tracked input\n"
        }
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
