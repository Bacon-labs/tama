use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use tama_common::{read_to_string, write_string};
use tama_config::{PathsConfig, TamaLock};
use toml_edit::Item;

pub type Result<T> = std::result::Result<T, Error>;

const DEFAULT_VERITY_GIT: &str = "https://github.com/lfglabs-dev/verity.git";
const DEFAULT_VERITY_REV: &str = "9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e";
const DEFAULT_LEAN_TOOLCHAIN: &str = "leanprover/lean4:v4.22.0";
const DEFAULT_SOLC: &str = "0.8.33";
const STARTER_LAKE_MANIFEST: &str = include_str!("templates/starter-lake-manifest.json");
const STARTER_CI_WORKFLOW: &str = include_str!("templates/starter-ci.yml");

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Common(#[from] tama_common::Error),
    #[error(transparent)]
    Config(#[from] tama_config::Error),
    #[error("invalid contract name `{0}`")]
    InvalidContractName(String),
    #[error("configured path must stay inside the project: {0}")]
    InvalidPath(Utf8PathBuf),
    #[error("project already contains {0}")]
    AlreadyExists(Utf8PathBuf),
    #[error("unsupported lakefile: {0}")]
    UnsupportedLakefile(String),
    #[error("failed to serialize starter lake manifest: {0}")]
    StarterManifest(#[source] serde_json::Error),
    #[error(
        "configured path `{path}` is not covered by Lake library `{library}` in lakefile.toml; expected `{expected}`"
    )]
    LakePathMismatch {
        library: &'static str,
        path: Utf8PathBuf,
        expected: Utf8PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    pub name: String,
    pub verity_version: String,
    pub verity_git: String,
    pub verity_rev: String,
    pub lean_toolchain: String,
    pub solc: String,
}

impl Default for InitOptions {
    fn default() -> Self {
        Self {
            name: "my-protocol".to_string(),
            verity_version: DEFAULT_VERITY_REV.to_string(),
            verity_git: DEFAULT_VERITY_GIT.to_string(),
            verity_rev: DEFAULT_VERITY_REV.to_string(),
            lean_toolchain: DEFAULT_LEAN_TOOLCHAIN.to_string(),
            solc: DEFAULT_SOLC.to_string(),
        }
    }
}

pub fn init(path: &Utf8Path, opts: InitOptions) -> Result<()> {
    if path.exists()
        && path
            .read_dir()
            .map_err(|source| tama_common::io_error(path.to_owned(), source))?
            .next()
            .is_some()
    {
        return Err(Error::AlreadyExists(path.to_owned()));
    }
    fs::create_dir_all(path).map_err(|source| tama_common::io_error(path.to_owned(), source))?;
    for dir in [
        "verity/src",
        "verity/spec",
        "verity/proof",
        "test/verity",
        "src/generated/verity",
        "lib",
        "script",
        "artifacts/yul",
        "artifacts/abi",
        "artifacts/bytecode",
        "artifacts/solc-json",
        "artifacts/manifest",
        "artifacts/lean",
        "artifacts/trust-probe",
        "docs",
        ".github/workflows",
    ] {
        fs::create_dir_all(path.join(dir))
            .map_err(|source| tama_common::io_error(path.join(dir), source))?;
    }

    write_string(&path.join("tama.toml"), &tama_toml(&opts))?;
    write_string(&path.join("foundry.toml"), FOUNDRY_TOML)?;
    write_string(&path.join("lakefile.toml"), &lakefile_toml(&opts))?;
    write_string(
        &path.join("lake-manifest.json"),
        &lake_manifest_json(&opts)?,
    )?;
    write_string(
        &path.join("lean-toolchain"),
        &(opts.lean_toolchain.clone() + "\n"),
    )?;
    write_string(&path.join("TamaSrc.lean"), "import src.ERC20Lite\n")?;
    write_string(
        &path.join("TamaSpec.lean"),
        "import TamaSrc\nimport spec.ERC20LiteSpec\n",
    )?;
    write_string(
        &path.join("TamaProof.lean"),
        "import TamaSpec\nimport proof.ERC20LiteProof\n",
    )?;
    write_string(&path.join("verity/src/ERC20Lite.lean"), ERC20LITE_LEAN)?;
    write_string(
        &path.join("verity/spec/ERC20LiteSpec.lean"),
        ERC20LITE_SPEC_LEAN,
    )?;
    write_string(
        &path.join("verity/proof/ERC20LiteProof.lean"),
        ERC20LITE_PROOF_LEAN,
    )?;
    write_string(
        &path.join("test/verity/ERC20Lite.t.sol"),
        ERC20LITE_TEST_SOL,
    )?;
    write_string(&path.join("script/ERC20Lite.s.sol"), ERC20LITE_SCRIPT_SOL)?;
    tama_common::write_generated(
        &path.join("src/generated/verity/ERC20LiteIface.sol"),
        ERC20LITE_IFACE_SOL,
    )?;
    tama_common::write_generated(
        &path.join("src/generated/verity/ERC20LiteDeployer.sol"),
        ERC20LITE_DEPLOYER_SOL,
    )?;
    write_string(&path.join("docs/README.md"), STARTER_README)?;
    write_string(&path.join(".github/workflows/ci.yml"), STARTER_CI_WORKFLOW)?;
    write_string(&path.join(".gitignore"), STARTER_GITIGNORE)?;

    let mut lock = TamaLock {
        version: 1,
        resolved: BTreeMap::from([
            ("verity_git".to_string(), opts.verity_git),
            ("verity_rev".to_string(), opts.verity_rev),
            ("lean_toolchain".to_string(), opts.lean_toolchain),
            ("solc".to_string(), opts.solc),
        ]),
        inputs: BTreeMap::new(),
        yul: BTreeMap::new(),
    };
    tama_config::update_lock_inputs(path, &mut lock)?;
    tama_config::write_lock(path, &lock)?;
    Ok(())
}

pub fn scaffold_contract(root: &Utf8Path, name: &str) -> Result<()> {
    validate_contract_name(name)?;
    let paths = tama_config::load_config(root)?.paths;
    ensure_project_relative(&paths.src)?;
    ensure_project_relative(&paths.spec)?;
    ensure_project_relative(&paths.proof)?;
    ensure_project_relative(&paths.test)?;
    ensure_project_relative(&paths.generated)?;
    validate_lakefile_covers_paths(root, &paths)?;
    let src = root.join(paths.src.join(format!("{name}.lean")));
    let spec = root.join(paths.spec.join(format!("{name}Spec.lean")));
    let proof = root.join(paths.proof.join(format!("{name}Proof.lean")));
    let test = root.join(paths.test.join(format!("{name}.t.sol")));
    for path in [&src, &spec, &proof, &test] {
        if path.exists() {
            return Err(Error::AlreadyExists(path.clone()));
        }
    }
    let mut lock = tama_config::load_lock(root)?;
    write_string(&src, &contract_template(name))?;
    write_string(&spec, &spec_template(name))?;
    write_string(&proof, &proof_template(name, paths.test.as_str()))?;
    let generated_import_root = relative_project_path(&paths.test, &paths.generated);
    write_string(&test, &test_template(name, generated_import_root.as_str()))?;
    update_aggregate(root, "TamaSrc.lean", &format!("import src.{name}"))?;
    update_aggregate(root, "TamaSpec.lean", &format!("import spec.{name}Spec"))?;
    update_aggregate(root, "TamaProof.lean", &format!("import proof.{name}Proof"))?;
    tama_config::update_lock_inputs(root, &mut lock)?;
    tama_config::write_lock(root, &lock)?;
    Ok(())
}

pub fn validate_contract_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidContractName(name.to_string()))
    }
}

fn ensure_project_relative(path: &Utf8Path) -> Result<()> {
    if path.is_absolute() || path.components().any(|part| part.as_str() == "..") {
        Err(Error::InvalidPath(path.to_owned()))
    } else {
        Ok(())
    }
}

fn relative_project_path(from_dir: &Utf8Path, to: &Utf8Path) -> Utf8PathBuf {
    let from_components = normal_components(from_dir);
    let to_components = normal_components(to);
    let common_len = from_components
        .iter()
        .zip(&to_components)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = Utf8PathBuf::new();
    for _ in common_len..from_components.len() {
        relative.push("..");
    }
    for component in &to_components[common_len..] {
        relative.push(component);
    }
    if relative.as_str().is_empty() {
        ".".into()
    } else {
        relative
    }
}

fn normal_components(path: &Utf8Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Utf8Component::Normal(part) => Some(part.to_string()),
            Utf8Component::CurDir => None,
            Utf8Component::ParentDir | Utf8Component::RootDir | Utf8Component::Prefix(_) => None,
        })
        .collect()
}

fn validate_lakefile_covers_paths(root: &Utf8Path, paths: &PathsConfig) -> Result<()> {
    let lakefile = root.join("lakefile.toml");
    if !lakefile.is_file() {
        if root.join("lakefile.lean").is_file() {
            return Ok(());
        }
        return Err(Error::UnsupportedLakefile(
            "missing lakefile.toml".to_string(),
        ));
    }
    let text = read_to_string(&lakefile)?;
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|err| Error::UnsupportedLakefile(format!("failed to parse {lakefile}: {err}")))?;
    for (library, path) in [
        ("src", &paths.src),
        ("spec", &paths.spec),
        ("proof", &paths.proof),
    ] {
        let src_dir = lean_lib_src_dir(&doc, library)?;
        let expected = expected_module_path(&src_dir, library);
        if path != &expected {
            return Err(Error::LakePathMismatch {
                library,
                path: path.clone(),
                expected,
            });
        }
    }
    Ok(())
}

fn lean_lib_src_dir(doc: &toml_edit::DocumentMut, library: &'static str) -> Result<Utf8PathBuf> {
    let Some(libs) = doc.get("lean_lib").and_then(Item::as_array_of_tables) else {
        return Err(Error::UnsupportedLakefile(format!(
            "missing [[lean_lib]] entry for `{library}`"
        )));
    };
    for table in libs {
        if table.get("name").and_then(Item::as_str) == Some(library) {
            let src_dir = table.get("srcDir").and_then(Item::as_str).unwrap_or(".");
            let src_dir = Utf8PathBuf::from(src_dir);
            ensure_project_relative(&src_dir)?;
            return Ok(src_dir);
        }
    }
    Err(Error::UnsupportedLakefile(format!(
        "missing [[lean_lib]] entry for `{library}`"
    )))
}

fn expected_module_path(src_dir: &Utf8Path, library: &str) -> Utf8PathBuf {
    if src_dir.as_str() == "." {
        Utf8PathBuf::from(library)
    } else {
        src_dir.join(library)
    }
}

fn update_aggregate(root: &Utf8Path, file: &str, import: &str) -> Result<()> {
    let path = root.join(file);
    let mut text = if path.exists() {
        read_to_string(&path)?
    } else {
        String::new()
    };
    if !text.lines().any(|line| line.trim() == import) {
        if !text.ends_with('\n') && !text.is_empty() {
            text.push('\n');
        }
        text.push_str(import);
        text.push('\n');
        write_string(&path, &text)?;
    }
    Ok(())
}

fn tama_toml(opts: &InitOptions) -> String {
    format!(
        r#"[project]
name = "{name}"
verity = "{verity}"

[paths]
src = "verity/src"
spec = "verity/spec"
proof = "verity/proof"
mirror_test = "test/verity"
out = "artifacts"
generated_solidity = "src/generated/verity"

[yul]
solc = "{solc}"
optimizer = true
optimizer_runs = 200
yul_optimizer = true
evm_version = "cancun"
metadata_bytecode_hash = "none"

[trust.allow_axioms]
"Classical.choice" = "Lean standard classical reasoning accepted for this project"
"propext" = "Lean standard propositional extensionality accepted for this project"
"Quot.sound" = "Lean quotient soundness accepted for this project"
"#,
        name = opts.name,
        verity = opts.verity_version,
        solc = opts.solc
    )
}

fn lakefile_toml(opts: &InitOptions) -> String {
    format!(
        r#"name = "{name}"
version = "0.1.0"
defaultTargets = ["TamaProof"]
buildDir = "artifacts/lean"

[[require]]
name = "verity"
git = "{git}"
rev = "{rev}"

[[lean_lib]]
name = "TamaSrc"

[[lean_lib]]
name = "TamaSpec"

[[lean_lib]]
name = "TamaProof"

[[lean_lib]]
name = "src"
srcDir = "verity"

[[lean_lib]]
name = "spec"
srcDir = "verity"

[[lean_lib]]
name = "proof"
srcDir = "verity"
"#,
        name = opts.name.replace('-', "_"),
        git = opts.verity_git,
        rev = opts.verity_rev
    )
}

fn lake_manifest_json(opts: &InitOptions) -> Result<String> {
    let name = opts.name.replace('-', "_");
    if opts.verity_git == DEFAULT_VERITY_GIT && opts.verity_rev == DEFAULT_VERITY_REV {
        return Ok(STARTER_LAKE_MANIFEST.replace("__TAMA_PROJECT_NAME__", &name));
    }
    let manifest = serde_json::to_string_pretty(&serde_json::json!({
        "version": "1.1.0",
        "packagesDir": ".lake/packages",
        "packages": [{
            "url": opts.verity_git,
            "type": "git",
            "subDir": serde_json::Value::Null,
            "scope": "",
            "rev": opts.verity_rev,
            "name": "verity",
            "manifestFile": "lake-manifest.json",
            "inputRev": opts.verity_rev,
            "inherited": false,
            "configFile": "lakefile.lean"
        }],
        "name": name,
        "lakeDir": ".lake"
    }))
    .map_err(Error::StarterManifest)?;
    Ok(manifest + "\n")
}

fn contract_template(name: &str) -> String {
    format!(
        r#"import Contracts.Common

namespace src

open Verity hiding pure bind
open Contracts
open Verity.EVM.Uint256
open Verity.Stdlib.Math

verity_contract {name} where
  storage
    value : Uint256 := slot 0

  function setValue (newValue : Uint256) : Unit := do
    setStorage value newValue

  function view getValue () : Uint256 := do
    let currentValue ← getStorage value
    return currentValue

end src
"#
    )
}

fn spec_template(name: &str) -> String {
    format!(
        r#"import src.{name}

namespace spec.{name}Spec

open Verity
open Verity.EVM.Uint256

def setValue_spec (newValue : Uint256) (_s s' : ContractState) : Prop :=
  s'.storage 0 = newValue

def getValue_spec (result : Uint256) (s : ContractState) : Prop :=
  result = s.storage 0

end spec.{name}Spec
"#
    )
}

fn proof_template(name: &str, _mirror_test_dir: &str) -> String {
    format!(
        r#"import spec.{name}Spec

namespace proof.{name}Proof

open Verity
open Verity.EVM.Uint256
open spec.{name}Spec
open src.{name}

-- tama: discharges=setValue_spec
theorem setValue_meets_spec (newValue : Uint256) (s : ContractState) :
  let s' := ((setValue newValue).run s).snd
  setValue_spec newValue s s' := by
  sorry

-- tama: discharges=getValue_spec
theorem getValue_returns_value (s : ContractState) :
  let result := ((getValue).run s).fst
  getValue_spec result s := by
  sorry

end proof.{name}Proof
"#
    )
}

fn test_template(name: &str, generated_import_root: &str) -> String {
    format!(
        r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {{{name}Deployer}} from "{generated_import_root}/{name}Deployer.sol";
import {{{name}Iface}} from "{generated_import_root}/{name}Iface.sol";

contract {name}Test {{
    // tama: mirrors=setValue_spec
    function testFuzzSetValueUpdatesValue(uint256 newValue) public {{
        {name}Iface target = {name}Deployer.deploy();
        target.setValue(newValue);
        require(target.getValue() == newValue, "value");
    }}

    // tama: mirrors=getValue_spec
    function testFuzzGetValueMirrorsGeneratedBytecode(uint256 newValue) public {{
        {name}Iface target = {name}Deployer.deploy();
        target.setValue(newValue);
        require(target.getValue() == newValue, "getValue");
    }}
}}
"#
    )
}

const FOUNDRY_TOML: &str = r#"[profile.default]
src = "src"
test = "test"
out = "out"
libs = ["lib"]
solc_version = "0.8.33"
fs_permissions = [{ access = "read", path = "./artifacts" }]
"#;

const STARTER_GITIGNORE: &str = "/.lake/\n/artifacts/\n/cache/\n/out/\nfoundry.lock\n";

const ERC20LITE_LEAN: &str = r#"import Contracts.Common

namespace src

open Verity hiding pure bind
open Contracts
open Verity.EVM.Uint256
open Verity.Stdlib.Math

-- This starter is intentionally small: it shows the full Tama path from a
-- Verity contract, to Lean specs/proofs, to generated bytecode and Foundry
-- tests. The contract is ERC20-shaped, but omits allowances and events so the
-- proof surface stays readable.
verity_contract ERC20Lite where
  storage
    -- Storage slots are explicit. The generated manifest records these slots
    -- so `tama audit storage-layout` can compare Lean and compiler artifacts.
    ownerSlot : Address := slot 0
    balancesSlot : Address → Uint256 := slot 1
    totalSupplySlot : Uint256 := slot 2

  constructor (initialOwner : Address) := do
    setStorageAddr ownerSlot initialOwner
    setStorage totalSupplySlot 0

  -- Only the deployment owner can mint. `safeAdd` returns `none` on overflow,
  -- and `requireSomeUint` turns that into a revert.
  function mint (toAddr : Address, amount : Uint256) : Bool := do
    let sender ← msgSender
    let currentOwner ← getStorageAddr ownerSlot
    require (sender == currentOwner) "Caller is not the owner"
    let currentBalance ← getMapping balancesSlot toAddr
    let newBalance ← requireSomeUint (safeAdd currentBalance amount) "Balance overflow"
    let currentSupply ← getStorage totalSupplySlot
    let newSupply ← requireSomeUint (safeAdd currentSupply amount) "Supply overflow"
    setMapping balancesSlot toAddr newBalance
    setStorage totalSupplySlot newSupply
    return true

  -- Transfers preserve total supply. The self-transfer branch avoids a
  -- read/modify/write round trip that would otherwise double-count.
  function transfer (toAddr : Address, amount : Uint256) : Bool := do
    let sender ← msgSender
    let senderBalance ← getMapping balancesSlot sender
    require (senderBalance >= amount) "Insufficient balance"
    if sender == toAddr then
      pure ()
    else
      let recipientBalance ← getMapping balancesSlot toAddr
      let newRecipientBalance ← requireSomeUint (safeAdd recipientBalance amount) "Recipient balance overflow"
      setMapping balancesSlot sender (sub senderBalance amount)
      setMapping balancesSlot toAddr newRecipientBalance
    return true

  function view balanceOf (addr : Address) : Uint256 := do
    let currentBalance ← getMapping balancesSlot addr
    return currentBalance

  function view totalSupply () : Uint256 := do
    let currentSupply ← getStorage totalSupplySlot
    return currentSupply

  function view owner () : Address := do
    let currentOwner ← getStorageAddr ownerSlot
    return currentOwner

end src
"#;

const ERC20LITE_SPEC_LEAN: &str = r#"import src.ERC20Lite

namespace spec.ERC20LiteSpec

open Verity
open Verity.EVM.Uint256

def transfer_total_supply_preserved (s s' : ContractState) : Prop :=
  s'.storage 2 = s.storage 2

def mint_owner_preserved (s s' : ContractState) : Prop :=
  s'.storageAddr 0 = s.storageAddr 0

def balanceOf_spec (account : Address) (result : Uint256) (s : ContractState) : Prop :=
  result = s.storageMap 1 account

def totalSupply_spec (result : Uint256) (s : ContractState) : Prop :=
  result = s.storage 2

def owner_spec (result : Address) (s : ContractState) : Prop :=
  result = s.storageAddr 0

end spec.ERC20LiteSpec
"#;

const ERC20LITE_PROOF_LEAN: &str = r#"import spec.ERC20LiteSpec
import Verity.Proofs.Stdlib.Automation

namespace proof.ERC20LiteProof

open Verity
open Verity.EVM.Uint256
open spec.ERC20LiteSpec
open src.ERC20Lite

-- Each `tama: discharges=` comment binds a proof theorem to a spec; the spec
-- in turn is mirrored by a Foundry test (or listed under
-- `[coverage.proof_only]` in `tama.toml`).
-- tama: discharges=mint_owner_preserved
theorem mint_preserves_owner_after_run (toAddr : Address) (amount : Uint256) (s : ContractState) :
  let s' := ((mint toAddr amount).run s).snd
  mint_owner_preserved s s' := by
  unfold mint_owner_preserved
  by_cases h_owner : s.sender = s.storageAddr 0
  · simp [mint, ownerSlot, balancesSlot, totalSupplySlot, msgSender, getStorageAddr,
      getMapping, getStorage, setMapping, setStorage, Contract.run, ContractResult.snd,
      Verity.bind, Bind.bind, Verity.pure, Pure.pure, Verity.require,
      Verity.Stdlib.Math.requireSomeUint, Verity.Stdlib.Math.safeAdd, h_owner]
    by_cases h_balance_overflow : Verity.Stdlib.Math.MAX_UINT256 <
        (s.storageMap 1 toAddr).val + amount.val
    · simp [h_balance_overflow, Verity.require, Verity.bind]
    · by_cases h_supply_overflow : Verity.Stdlib.Math.MAX_UINT256 <
          (s.storage 2).val + amount.val <;>
        simp [h_balance_overflow, h_supply_overflow, Verity.require, Verity.bind,
          Verity.pure]
  · simp [mint, ownerSlot, msgSender, getStorageAddr, Contract.run, ContractResult.snd,
      Verity.bind, Bind.bind, Verity.require, h_owner]

-- tama: discharges=transfer_total_supply_preserved
theorem transfer_total_supply_preserved_after_run (toAddr : Address) (amount : Uint256) (s : ContractState) :
  let s' := ((transfer toAddr amount).run s).snd
  transfer_total_supply_preserved s s' := by
  unfold transfer_total_supply_preserved
  by_cases h_balance : amount.val ≤ (s.storageMap 1 s.sender).val
  · simp [transfer, balancesSlot, msgSender, getMapping, Contract.run,
      ContractResult.snd, Verity.bind, Bind.bind, Pure.pure,
      Verity.require, Verity.Stdlib.Math.requireSomeUint, Verity.Stdlib.Math.safeAdd,
      h_balance]
    by_cases h_same : s.sender = toAddr
    · simp [h_same, Verity.pure]
    · by_cases h_overflow : Verity.Stdlib.Math.MAX_UINT256 <
          (s.storageMap 1 toAddr).val + amount.val <;>
        simp [h_same, h_overflow, getMapping, setMapping, Verity.require,
          Verity.bind, Verity.pure]
  · simp [transfer, balancesSlot, msgSender, getMapping, Contract.run, ContractResult.snd,
      Verity.bind, Bind.bind, Verity.require, h_balance]

-- tama: discharges=balanceOf_spec
theorem balanceOf_returns_storage_balance (account : Address) (s : ContractState) :
  let result := ((balanceOf account).run s).fst
  balanceOf_spec account result s := by
  simp [balanceOf_spec, balanceOf, balancesSlot, Bind.bind, Pure.pure]

-- tama: discharges=totalSupply_spec
theorem totalSupply_returns_storage_supply (s : ContractState) :
  let result := ((totalSupply).run s).fst
  totalSupply_spec result s := by
  simp [totalSupply_spec, totalSupply, totalSupplySlot, Bind.bind, Pure.pure]

-- tama: discharges=owner_spec
theorem owner_returns_storage_owner (s : ContractState) :
  let result := ((owner).run s).fst
  owner_spec result s := by
  simp [owner_spec, owner, ownerSlot, Bind.bind, Pure.pure]

end proof.ERC20LiteProof
"#;

const ERC20LITE_TEST_SOL: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {ERC20LiteDeployer} from "../../src/generated/verity/ERC20LiteDeployer.sol";
import {ERC20LiteIface} from "../../src/generated/verity/ERC20LiteIface.sol";
import {StdInvariant} from "forge-std/StdInvariant.sol";

// These Foundry tests mirror the proof obligations in
// verity/proof/ERC20LiteProof.lean. Run `tama build` first so the generated
// deployer contains bytecode compiled from the Verity/Yul pipeline.
contract ERC20LiteTest is StdInvariant {
    ERC20LiteIface internal invariantToken;
    uint256 internal invariantMinted;

    function setUp() public {
        // Invariant handlers exercise mint and transfer across many calls while
        // `invariant_totalSupplyTracksMinted` checks the global property.
        invariantToken = deployToken();
        bytes4[] memory selectors = new bytes4[](2);
        selectors[0] = this.handlerMint.selector;
        selectors[1] = this.handlerTransferFromOwner.selector;
        targetSelector(FuzzSelector({addr: address(this), selectors: selectors}));
    }

    function deployToken() internal returns (ERC20LiteIface token) {
        token = deployToken(address(this));
    }

    function deployToken(address initialOwner) internal returns (ERC20LiteIface token) {
        token = ERC20LiteDeployer.deploy(initialOwner);
    }

    // Deployment mirror: the constructor writes owner and starts supply at zero.
    // tama: mirrors=owner_spec
    function testFuzzDeploymentSetsOwner(address initialOwner) public {
        ERC20LiteIface token = deployToken(initialOwner);
        require(token.owner() == initialOwner, "owner");
        require(token.totalSupply() == 0, "initial supply");
    }

    // Mint mirror: owner minting updates one balance and total supply together.
    // tama: mirrors=mint_owner_preserved,totalSupply_spec
    function testFuzzMintUpdatesBalanceAndSupply(address account, uint256 rawAmount) public {
        ERC20LiteIface token = deployToken();
        uint256 amount = rawAmount % 1e36;
        require(token.mint(account, amount), "mint");
        require(token.balanceOf(account) == amount, "minted balance");
        require(token.totalSupply() == amount, "minted supply");
        require(token.owner() == address(this), "owner preserved");
    }

    // Transfer mirror: moving tokens changes balances but preserves supply.
    // tama: mirrors=transfer_total_supply_preserved
    function testFuzzTransferPreservesTotalSupply(address recipient, uint256 rawMint, uint256 rawTransfer) public {
        ERC20LiteIface token = deployToken();
        uint256 minted = rawMint % 1e36;
        uint256 amount = minted == 0 ? 0 : rawTransfer % (minted + 1);
        require(token.mint(address(this), minted), "mint");
        require(token.transfer(recipient, amount), "transfer");
        if (recipient == address(this)) {
            require(token.balanceOf(address(this)) == minted, "self transfer balance");
        } else {
            require(token.balanceOf(address(this)) == minted - amount, "sender balance");
            require(token.balanceOf(recipient) == amount, "recipient balance");
        }
        require(token.totalSupply() == minted, "supply preserved");
    }

    // tama: mirrors=balanceOf_spec
    function testFuzzBalanceOfMirrorsGeneratedBytecode(address account, uint256 rawAmount) public {
        ERC20LiteIface token = deployToken();
        uint256 amount = rawAmount % 1e36;
        require(token.mint(account, amount), "mint");
        require(token.balanceOf(account) == amount, "balanceOf");
    }

    function handlerMint(uint8 accountIndex, uint256 rawAmount) public {
        uint256 amount = rawAmount % 1e24;
        require(invariantToken.mint(invariantAccount(accountIndex), amount), "invariant mint");
        invariantMinted += amount;
    }

    function handlerTransferFromOwner(uint8 accountIndex, uint256 rawAmount) public {
        uint256 balance = invariantToken.balanceOf(address(this));
        uint256 amount = balance == 0 ? 0 : rawAmount % (balance + 1);
        require(invariantToken.transfer(invariantAccount(accountIndex), amount), "invariant transfer");
    }

    // tama: mirrors=totalSupply_spec
    function invariant_totalSupplyTracksMinted() public view {
        require(invariantToken.totalSupply() == invariantMinted, "invariant supply");
    }

    function invariantAccount(uint8 index) internal view returns (address) {
        uint8 account = index % 3;
        if (account == 0) {
            return address(this);
        }
        if (account == 1) {
            return address(0xA11CE);
        }
        return address(0xB0B);
    }
}
"#;

const ERC20LITE_SCRIPT_SOL: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script} from "forge-std/Script.sol";
import {ERC20LiteDeployer} from "../src/generated/verity/ERC20LiteDeployer.sol";
import {ERC20LiteIface} from "../src/generated/verity/ERC20LiteIface.sol";

// Run after `tama build`:
//   forge script script/ERC20Lite.s.sol:DeployERC20Lite --broadcast --rpc-url <url>
//
// The generated deployer embeds bytecode produced from the Verity/Yul build.
// Set ERC20LITE_OWNER to override the initial owner.
contract DeployERC20Lite is Script {
    function run() external returns (ERC20LiteIface token) {
        address initialOwner = vm.envOr("ERC20LITE_OWNER", msg.sender);

        vm.startBroadcast();
        token = ERC20LiteDeployer.deploy(initialOwner);
        vm.stopBroadcast();
    }
}
"#;

const ERC20LITE_IFACE_SOL: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

// This file is generated by `tama build`. The starter version is a small
// placeholder so tests and scripts can show the intended imports immediately.
interface ERC20LiteIface {
    function mint(address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
    function totalSupply() external view returns (uint256);
    function owner() external view returns (address);
}
"#;

const ERC20LITE_DEPLOYER_SOL: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {ERC20LiteIface} from "./ERC20LiteIface.sol";

// This placeholder is replaced by `tama build` with a deployer that embeds the
// solc-compiled creation bytecode for the Verity contract.
library ERC20LiteDeployer {
    function deploy(address initialOwner) internal pure returns (ERC20LiteIface token) {
        initialOwner;
        token;
        revert("TAMA_BUILD_REQUIRED");
    }
}
"#;

const STARTER_README: &str = r#"# Tama ERC20Lite Starter

This project was generated by `tama init`.

Run:

```sh
tama doctor
tama check
tama build
tama test
tama audit
```

After `tama build`, the generated deployer contains real bytecode and the starter
deploy script can be run with Foundry:

```sh
forge script script/ERC20Lite.s.sol:DeployERC20Lite --broadcast --rpc-url <url>
```

Set `ERC20LITE_OWNER=<address>` to choose an owner other than Foundry's sender.

## Continuous integration

`.github/workflows/ci.yml` runs `tama doctor`, `tama check`, `tama build
--locked`, `tama test`, and `tama audit` on every push and pull request. The
first run installs Lean (elan), Foundry, solc 0.8.33, and Tama; later runs
reuse caches keyed on `lake-manifest.json` and `tama.lock`.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn write_custom_paths_config(root: &Utf8Path) {
        tama_common::write_string(
            &root.join("tama.toml"),
            &format!(
                r#"[project]
name = "starter"
verity = "{DEFAULT_VERITY_REV}"

[paths]
src = "contracts/src"
spec = "contracts/spec"
proof = "contracts/proof"
mirror_test = "tests/verity"
out = "artifacts"
generated_solidity = "src/generated/verity"

[yul]
solc = "0.8.33"
optimizer = true
optimizer_runs = 200
yul_optimizer = true
evm_version = "cancun"
metadata_bytecode_hash = "none"
"#
            ),
        )
        .unwrap();
    }

    fn write_custom_lakefile_roots(root: &Utf8Path) -> String {
        let lakefile = read_to_string(&root.join("lakefile.toml"))
            .unwrap()
            .replace(r#"srcDir = "verity""#, r#"srcDir = "contracts""#);
        write_string(&root.join("lakefile.toml"), &lakefile).unwrap();
        lakefile
    }

    fn move_starter_to_custom_paths(root: &Utf8Path) {
        for (from, to) in [
            ("verity/src/ERC20Lite.lean", "contracts/src/ERC20Lite.lean"),
            (
                "verity/spec/ERC20LiteSpec.lean",
                "contracts/spec/ERC20LiteSpec.lean",
            ),
            (
                "verity/proof/ERC20LiteProof.lean",
                "contracts/proof/ERC20LiteProof.lean",
            ),
        ] {
            let to = root.join(to);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::rename(root.join(from), to).unwrap();
        }
    }

    #[test]
    fn init_creates_erc20lite_starter_without_foundry_counter() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        init(&root, InitOptions::default()).unwrap();
        assert!(root.join("verity/src/ERC20Lite.lean").is_file());
        assert!(root.join("test/verity/ERC20Lite.t.sol").is_file());
        assert!(root.join("script/ERC20Lite.s.sol").is_file());
        assert!(root.join("artifacts/abi").is_dir());
        assert!(!root.join("src/Counter.sol").exists());
        assert!(!root.join("test/Counter.t.sol").exists());
        let proof = read_to_string(&root.join("verity/proof/ERC20LiteProof.lean")).unwrap();
        assert!(!proof.contains("sorry"));
        assert!(!proof.contains("Placeholder"));
        assert!(!proof.contains("kind="));
        assert!(!proof.contains("coverage="));
        assert!(proof.contains("tama: discharges=transfer_total_supply_preserved"));
        assert!(proof.contains("((transfer toAddr amount).run s).snd"));
        assert!(proof.contains("((mint toAddr amount).run s).snd"));
        assert!(proof.contains("((balanceOf account).run s).fst"));
        let test = read_to_string(&root.join("test/verity/ERC20Lite.t.sol")).unwrap();
        assert!(!test.contains("testTransferPostPlaceholder"));
        assert!(!test.contains("function testDeploymentSetsOwner"));
        assert!(test.contains("token = deployToken(address(this));"));
        assert!(test.contains("ERC20LiteDeployer.deploy(initialOwner)"));
        assert!(test.contains("// tama: mirrors=owner_spec"));
        assert!(test.contains("// tama: mirrors=transfer_total_supply_preserved"));
        assert!(test.contains("function testFuzzDeploymentSetsOwner(address initialOwner)"));
        assert!(test.contains("testFuzzTransferPreservesTotalSupply"));
        assert!(test.contains("StdInvariant"));
        assert!(test.contains("invariant_totalSupplyTracksMinted"));
        assert!(test.contains("token.transfer(recipient, amount)"));
        assert!(test.contains("These Foundry tests mirror the proof obligations"));
        let script = read_to_string(&root.join("script/ERC20Lite.s.sol")).unwrap();
        assert!(script.contains("contract DeployERC20Lite is Script"));
        assert!(script.contains("ERC20LiteDeployer.deploy(initialOwner)"));
        assert!(script.contains("ERC20LITE_OWNER"));
        assert!(script.contains("Run after `tama build`"));
        let source = read_to_string(&root.join("verity/src/ERC20Lite.lean")).unwrap();
        assert!(source.contains("This starter is intentionally small"));
        assert!(source.contains("Storage slots are explicit"));
        assert!(source.contains("function view balanceOf"));
        assert!(!source.contains(r#"emit "Transfer""#));
        let spec = read_to_string(&root.join("verity/spec/ERC20LiteSpec.lean")).unwrap();
        assert!(spec.contains("def transfer_total_supply_preserved"));
        assert!(!spec.contains("-- tama:"));
        let starter_readme = read_to_string(&root.join("docs/README.md")).unwrap();
        assert!(starter_readme.contains("script/ERC20Lite.s.sol:DeployERC20Lite"));
        assert!(starter_readme.contains("ERC20LITE_OWNER"));
        let config = read_to_string(&root.join("tama.toml")).unwrap();
        assert!(config.contains(&format!("verity = \"{DEFAULT_VERITY_REV}\"")));
        assert!(config.contains("mirror_test = \"test/verity\""));
        assert!(config.contains("generated_solidity = \"src/generated/verity\""));
        assert!(config.contains("metadata_bytecode_hash = \"none\""));
        assert!(config.contains("yul_optimizer = true"));
        let lake_manifest = read_to_string(&root.join("lake-manifest.json")).unwrap();
        assert!(lake_manifest.contains(&format!(r#""rev": "{DEFAULT_VERITY_REV}""#)));
        assert!(lake_manifest.contains(r#""name": "my_protocol""#));
        let lock = tama_config::load_lock(&root).unwrap();
        assert_eq!(
            lock.resolved.get("verity_rev").map(String::as_str),
            Some(DEFAULT_VERITY_REV)
        );
        assert_eq!(
            lock.resolved.get("lake.verity.rev").map(String::as_str),
            Some(DEFAULT_VERITY_REV)
        );
        assert_eq!(
            lock.resolved.get("lean_toolchain").map(String::as_str),
            Some(DEFAULT_LEAN_TOOLCHAIN)
        );
        assert_eq!(
            lock.yul
                .get("metadata_bytecode_hash")
                .and_then(|value| value.as_str()),
            Some("none")
        );
        assert_eq!(
            lock.yul.get("optimizer").and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            lock.yul
                .get("yul_optimizer")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert!(lock.inputs.contains_key("lake-manifest.json"));
        assert!(tama_config::lock_drift(&root, &lock).unwrap().is_empty());
    }

    #[test]
    fn init_creates_github_actions_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        init(&root, InitOptions::default()).unwrap();
        let workflow_path = root.join(".github/workflows/ci.yml");
        assert!(workflow_path.is_file());
        let workflow = read_to_string(&workflow_path).unwrap();
        for needle in [
            "name: CI",
            "tama doctor",
            "tama check",
            "tama build --locked",
            "tama test",
            "tama audit",
            "foundry-rs/foundry-toolchain",
            "solc-select",
            "https://tama.tools/install.sh",
        ] {
            assert!(
                workflow.contains(needle),
                "starter workflow missing `{needle}`"
            );
        }
        let starter_readme = read_to_string(&root.join("docs/README.md")).unwrap();
        assert!(starter_readme.contains("Continuous integration"));
        assert!(starter_readme.contains(".github/workflows/ci.yml"));
    }

    #[test]
    fn init_creates_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        init(&root, InitOptions::default()).unwrap();
        let gitignore = read_to_string(&root.join(".gitignore")).unwrap();
        for entry in ["/.lake/", "/artifacts/", "/cache/", "/out/", "foundry.lock"] {
            assert!(
                gitignore.contains(entry),
                "starter .gitignore missing `{entry}`"
            );
        }
        assert!(
            !gitignore.lines().any(|line| line.trim() == "/lib/"
                || line.trim() == "lib/"
                || line.trim() == "/lib"
                || line.trim() == "lib"),
            "starter .gitignore must not exclude lib/ — `forge install` adds submodules under it"
        );
    }

    #[test]
    fn starter_ci_workflow_is_valid_yaml() {
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(STARTER_CI_WORKFLOW).expect("starter-ci.yml must be valid YAML");
        let steps = parsed
            .get("jobs")
            .and_then(|jobs| jobs.get("verify"))
            .and_then(|verify| verify.get("steps"))
            .and_then(|steps| steps.as_sequence())
            .expect("starter-ci.yml must declare jobs.verify.steps as a sequence");
        assert!(
            !steps.is_empty(),
            "starter-ci.yml jobs.verify.steps must not be empty"
        );
    }

    #[test]
    fn new_updates_aggregate_modules() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        init(&root, InitOptions::default()).unwrap();
        scaffold_contract(&root, "TipJar").unwrap();
        assert!(read_to_string(&root.join("TamaSrc.lean"))
            .unwrap()
            .contains("import src.TipJar"));
        let source = read_to_string(&root.join("verity/src/TipJar.lean")).unwrap();
        assert!(source.contains("function view getValue"));
        let spec = read_to_string(&root.join("verity/spec/TipJarSpec.lean")).unwrap();
        assert!(spec.contains("def setValue_spec"));
        assert!(spec.contains("def getValue_spec"));
        let proof = read_to_string(&root.join("verity/proof/TipJarProof.lean")).unwrap();
        assert!(proof.contains("((setValue newValue).run s).snd"));
        assert!(proof.contains("((getValue).run s).fst"));
        assert!(proof.contains("tama: discharges=setValue_spec"));
        assert!(proof.contains("tama: discharges=getValue_spec"));
        assert!(proof.contains("sorry"));
        let test = read_to_string(&root.join("test/verity/TipJar.t.sol")).unwrap();
        assert!(test.contains(
            "import {TipJarDeployer} from \"../../src/generated/verity/TipJarDeployer.sol\";"
        ));
        assert!(test.contains("// tama: mirrors=setValue_spec"));
        assert!(test.contains("// tama: mirrors=getValue_spec"));
        assert!(test.contains("function testFuzzSetValueUpdatesValue(uint256 newValue)"));
        assert!(
            test.contains("function testFuzzGetValueMirrorsGeneratedBytecode(uint256 newValue)")
        );
        assert!(!test.contains("testScaffoldCompiles"));
        let lock = tama_config::load_lock(&root).unwrap();
        assert!(tama_config::lock_drift(&root, &lock).unwrap().is_empty());
    }

    #[test]
    fn new_refuses_to_overwrite_any_existing_scaffold_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        init(&root, InitOptions::default()).unwrap();
        let existing = root.join("verity/spec/TipJarSpec.lean");
        write_string(&existing, "user spec\n").unwrap();

        let err = scaffold_contract(&root, "TipJar").unwrap_err();

        assert!(matches!(err, Error::AlreadyExists(path) if path == existing));
        assert_eq!(read_to_string(&existing).unwrap(), "user spec\n");
        assert!(!root.join("verity/src/TipJar.lean").exists());
        assert!(!read_to_string(&root.join("TamaSrc.lean"))
            .unwrap()
            .contains("import src.TipJar"));
    }

    #[test]
    fn new_rejects_corrupt_lock_before_writing_scaffold_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        init(&root, InitOptions::default()).unwrap();
        let src_before = read_to_string(&root.join("TamaSrc.lean")).unwrap();
        let spec_before = read_to_string(&root.join("TamaSpec.lean")).unwrap();
        let proof_before = read_to_string(&root.join("TamaProof.lean")).unwrap();
        write_string(&root.join("tama.lock"), "not = [valid").unwrap();

        let err = scaffold_contract(&root, "TipJar").unwrap_err();

        assert!(matches!(
            err,
            Error::Config(tama_config::Error::Toml { .. })
        ));
        assert!(!root.join("verity/src/TipJar.lean").exists());
        assert!(!root.join("verity/spec/TipJarSpec.lean").exists());
        assert!(!root.join("verity/proof/TipJarProof.lean").exists());
        assert!(!root.join("test/verity/TipJar.t.sol").exists());
        assert_eq!(
            read_to_string(&root.join("TamaSrc.lean")).unwrap(),
            src_before
        );
        assert_eq!(
            read_to_string(&root.join("TamaSpec.lean")).unwrap(),
            spec_before
        );
        assert_eq!(
            read_to_string(&root.join("TamaProof.lean")).unwrap(),
            proof_before
        );
    }

    #[test]
    fn new_rejects_configured_paths_not_covered_by_lakefile() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        init(&root, InitOptions::default()).unwrap();
        let lakefile_before = read_to_string(&root.join("lakefile.toml")).unwrap();
        write_custom_paths_config(&root);

        let err = scaffold_contract(&root, "TipJar").unwrap_err();

        assert!(matches!(
            err,
            Error::LakePathMismatch {
                library: "src",
                path,
                expected,
            } if path == "contracts/src" && expected == "verity/src"
        ));
        assert!(!root.join("contracts/src/TipJar.lean").exists());
        assert!(!root.join("contracts/spec/TipJarSpec.lean").exists());
        assert!(!root.join("contracts/proof/TipJarProof.lean").exists());
        assert!(!root.join("tests/verity/TipJar.t.sol").exists());
        assert!(!read_to_string(&root.join("TamaSrc.lean"))
            .unwrap()
            .contains("import src.TipJar"));
        assert_eq!(
            read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile_before
        );
    }

    #[test]
    fn new_uses_configured_project_paths_when_lakefile_matches_without_rewriting() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        init(&root, InitOptions::default()).unwrap();
        write_custom_paths_config(&root);
        move_starter_to_custom_paths(&root);
        let lakefile_before = write_custom_lakefile_roots(&root);

        scaffold_contract(&root, "TipJar").unwrap();

        assert!(root.join("contracts/src/TipJar.lean").is_file());
        assert!(root.join("contracts/spec/TipJarSpec.lean").is_file());
        assert!(root.join("contracts/proof/TipJarProof.lean").is_file());
        assert!(root.join("tests/verity/TipJar.t.sol").is_file());
        let proof = read_to_string(&root.join("contracts/proof/TipJarProof.lean")).unwrap();
        assert!(proof.contains("tama: discharges=setValue_spec"));
        let test = read_to_string(&root.join("tests/verity/TipJar.t.sol")).unwrap();
        assert!(test.contains("// tama: mirrors=setValue_spec"));
        assert!(!root.join("verity/src/TipJar.lean").exists());
        assert!(read_to_string(&root.join("TamaSrc.lean"))
            .unwrap()
            .contains("import src.TipJar"));
        assert_eq!(
            read_to_string(&root.join("lakefile.toml")).unwrap(),
            lakefile_before
        );
        let lock = tama_config::load_lock(&root).unwrap();
        assert!(tama_config::lock_drift(&root, &lock).unwrap().is_empty());
    }

    #[test]
    fn scaffold_imports_generated_solidity_relative_to_mirror_dir() {
        assert_eq!(
            relative_project_path(
                Utf8Path::new("test/verity"),
                Utf8Path::new("src/generated/verity")
            ),
            Utf8PathBuf::from("../../src/generated/verity")
        );
        assert_eq!(
            relative_project_path(
                Utf8Path::new("tests/integration/verity"),
                Utf8Path::new("contracts/generated")
            ),
            Utf8PathBuf::from("../../../contracts/generated")
        );
    }

    #[test]
    fn invalid_contract_name_fails() {
        assert!(validate_contract_name("tipJar").is_err());
    }

    #[test]
    fn yul_config_type_stays_constructible() {
        let _ = tama_config::YulConfig {
            solc: "0.8.33".to_string(),
            optimizer: true,
            optimizer_runs: 200,
            yul_optimizer: true,
            evm_version: "cancun".to_string(),
            metadata_hash: "none".to_string(),
        };
    }
}
