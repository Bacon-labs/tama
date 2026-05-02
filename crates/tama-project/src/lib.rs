use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use tama_common::{read_to_string, write_string};
use tama_config::TamaLock;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Common(#[from] tama_common::Error),
    #[error(transparent)]
    Config(#[from] tama_config::Error),
    #[error("invalid contract name `{0}`")]
    InvalidContractName(String),
    #[error("project already contains {0}")]
    AlreadyExists(Utf8PathBuf),
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
            verity_version: "0.1.0".to_string(),
            verity_git: "https://github.com/lfglabs-dev/verity.git".to_string(),
            verity_rev: "9b0114efcc0af589af63dd3f2eafcdf1a24dbf1e".to_string(),
            lean_toolchain: "leanprover/lean4:v4.22.0".to_string(),
            solc: "0.8.33".to_string(),
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
        "artifacts/bytecode",
        "artifacts/solc-json",
        "artifacts/manifest",
        "artifacts/lean",
        "artifacts/trust-probe",
        "docs",
    ] {
        fs::create_dir_all(path.join(dir))
            .map_err(|source| tama_common::io_error(path.join(dir), source))?;
    }

    write_string(&path.join("tama.toml"), &tama_toml(&opts))?;
    write_string(&path.join("foundry.toml"), FOUNDRY_TOML)?;
    write_string(&path.join("lakefile.toml"), &lakefile_toml(&opts))?;
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
    tama_common::write_generated(
        &path.join("src/generated/verity/ERC20LiteIface.sol"),
        ERC20LITE_IFACE_SOL,
    )?;
    tama_common::write_generated(
        &path.join("src/generated/verity/ERC20LiteDeployer.sol"),
        ERC20LITE_DEPLOYER_SOL,
    )?;
    write_string(&path.join("docs/README.md"), STARTER_README)?;

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
    let src = root.join(format!("verity/src/{name}.lean"));
    if src.exists() {
        return Err(Error::AlreadyExists(src));
    }
    write_string(&src, &contract_template(name))?;
    write_string(
        &root.join(format!("verity/spec/{name}Spec.lean")),
        &spec_template(name),
    )?;
    write_string(
        &root.join(format!("verity/proof/{name}Proof.lean")),
        &proof_template(name),
    )?;
    write_string(
        &root.join(format!("test/verity/{name}.t.sol")),
        &test_template(name),
    )?;
    update_aggregate(root, "TamaSrc.lean", &format!("import src.{name}"))?;
    update_aggregate(root, "TamaSpec.lean", &format!("import spec.{name}Spec"))?;
    update_aggregate(root, "TamaProof.lean", &format!("import proof.{name}Proof"))?;
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
test = "test/verity"
out = "artifacts"
generated = "src/generated/verity"

[yul]
solc = "{solc}"
optimizer = true
optimizer_runs = 200
evm_version = "cancun"
metadata_hash = "none"

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

  function getValue () : Uint256 := do
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

theorem scaffold_spec_marker : True := by
  trivial

end spec.{name}Spec
"#
    )
}

fn proof_template(name: &str) -> String {
    format!(
        r#"import spec.{name}Spec

namespace proof.{name}Proof

theorem scaffold_proof_marker : True := by
  trivial

end proof.{name}Proof
"#
    )
}

fn test_template(name: &str) -> String {
    format!(
        r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

contract {name}Test {{
    function testScaffoldCompiles() public pure {{
        require(true);
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

const ERC20LITE_LEAN: &str = r#"import Contracts.Common

namespace src

open Verity hiding pure bind
open Contracts
open Verity.EVM.Uint256
open Verity.Stdlib.Math

verity_contract ERC20Lite where
  storage
    ownerSlot : Address := slot 0
    balancesSlot : Address → Uint256 := slot 1
    totalSupplySlot : Uint256 := slot 2

  constructor (initialOwner : Address) := do
    setStorageAddr ownerSlot initialOwner
    setStorage totalSupplySlot 0

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

set_option linter.unusedSimpArgs false

namespace proof.ERC20LiteProof

open Verity
open Verity.EVM.Uint256
open spec.ERC20LiteSpec
open src.ERC20Lite

-- tama: obligation kind=postcondition function=mint coverage=mirror path=test/verity/ERC20Lite.t.sol:ERC20LiteTest.testFuzzMintUpdatesBalanceAndSupply
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
    · simp [h_balance_overflow, Verity.require, Verity.bind, Bind.bind, Verity.pure,
        Pure.pure, Contract.run, ContractResult.snd]
    · by_cases h_supply_overflow : Verity.Stdlib.Math.MAX_UINT256 <
          (s.storage 2).val + amount.val
      · simp [h_balance_overflow, h_supply_overflow, Verity.require, Verity.bind,
          Bind.bind, Verity.pure, Pure.pure, Contract.run, ContractResult.snd]
      · simp [h_balance_overflow, h_supply_overflow, setMapping, setStorage,
          Verity.bind, Bind.bind, Verity.pure, Pure.pure, Contract.run,
          ContractResult.snd]
  · simp [mint, ownerSlot, msgSender, getStorageAddr, Contract.run, ContractResult.snd,
      Verity.bind, Bind.bind, Verity.require, h_owner]

-- tama: obligation kind=postcondition function=transfer coverage=mirror path=test/verity/ERC20Lite.t.sol:ERC20LiteTest.testFuzzTransferPreservesTotalSupply
theorem transfer_total_supply_preserved_after_run (toAddr : Address) (amount : Uint256) (s : ContractState) :
  let s' := ((transfer toAddr amount).run s).snd
  transfer_total_supply_preserved s s' := by
  unfold transfer_total_supply_preserved
  by_cases h_balance : amount.val ≤ (s.storageMap 1 s.sender).val
  · simp [transfer, balancesSlot, msgSender, getMapping, setMapping, Contract.run,
      ContractResult.snd, Verity.bind, Bind.bind, Verity.pure, Pure.pure,
      Verity.require, Verity.Stdlib.Math.requireSomeUint, Verity.Stdlib.Math.safeAdd,
      h_balance]
    by_cases h_same : s.sender = toAddr
    · simp [h_same, Verity.pure, Pure.pure, Contract.run, ContractResult.snd]
    · by_cases h_overflow : Verity.Stdlib.Math.MAX_UINT256 <
          (s.storageMap 1 toAddr).val + amount.val
      · simp [h_same, h_overflow, getMapping, setMapping, Verity.require,
          Verity.Stdlib.Math.requireSomeUint, Verity.Stdlib.Math.safeAdd,
          Verity.bind, Bind.bind, Verity.pure, Pure.pure, Contract.run,
          ContractResult.snd]
      · simp [h_same, h_overflow, getMapping, setMapping,
          Verity.Stdlib.Math.requireSomeUint, Verity.Stdlib.Math.safeAdd,
          Verity.bind, Bind.bind, Verity.pure, Pure.pure, Contract.run,
          ContractResult.snd]
  · simp [transfer, balancesSlot, msgSender, getMapping, Contract.run, ContractResult.snd,
      Verity.bind, Bind.bind, Verity.require, h_balance]

-- tama: obligation kind=postcondition function=balanceOf coverage=mirror path=test/verity/ERC20Lite.t.sol:ERC20LiteTest.testFuzzBalanceOfMirrorsGeneratedBytecode
theorem balanceOf_returns_storage_balance (account : Address) (s : ContractState) :
  let result := ((balanceOf account).run s).fst
  balanceOf_spec account result s := by
  simp [balanceOf_spec, balanceOf, balancesSlot, getMapping, Contract.run,
    ContractResult.fst, Verity.bind, Bind.bind, Verity.pure, Pure.pure]

-- tama: obligation kind=postcondition function=totalSupply coverage=mirror path=test/verity/ERC20Lite.t.sol:ERC20LiteTest.testFuzzMintUpdatesBalanceAndSupply
theorem totalSupply_returns_storage_supply (s : ContractState) :
  let result := ((totalSupply).run s).fst
  totalSupply_spec result s := by
  simp [totalSupply_spec, totalSupply, totalSupplySlot, getStorage, Contract.run,
    ContractResult.fst, Verity.bind, Bind.bind, Verity.pure, Pure.pure]

-- tama: obligation kind=postcondition function=owner coverage=mirror path=test/verity/ERC20Lite.t.sol:ERC20LiteTest.testDeploymentSetsOwner
theorem owner_returns_storage_owner (s : ContractState) :
  let result := ((owner).run s).fst
  owner_spec result s := by
  simp [owner_spec, owner, ownerSlot, getStorageAddr, Contract.run, ContractResult.fst,
    Verity.bind, Bind.bind, Verity.pure, Pure.pure]

end proof.ERC20LiteProof
"#;

const ERC20LITE_TEST_SOL: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {ERC20LiteDeployer} from "../../src/generated/verity/ERC20LiteDeployer.sol";
import {ERC20LiteIface} from "../../src/generated/verity/ERC20LiteIface.sol";
import {StdInvariant} from "forge-std/StdInvariant.sol";

contract ERC20LiteTest is StdInvariant {
    ERC20LiteIface internal invariantToken;
    uint256 internal invariantMinted;

    function setUp() public {
        invariantToken = deployToken();
        bytes4[] memory selectors = new bytes4[](2);
        selectors[0] = this.handlerMint.selector;
        selectors[1] = this.handlerTransferFromOwner.selector;
        targetSelector(FuzzSelector({addr: address(this), selectors: selectors}));
    }

    function deployToken() internal returns (ERC20LiteIface token) {
        token = ERC20LiteDeployer.deploy(address(this));
    }

    function testDeploymentSetsOwner() public {
        ERC20LiteIface token = deployToken();
        require(token.owner() == address(this), "owner");
        require(token.totalSupply() == 0, "initial supply");
    }

    function testFuzzMintUpdatesBalanceAndSupply(address account, uint256 rawAmount) public {
        ERC20LiteIface token = deployToken();
        uint256 amount = rawAmount % 1e36;
        require(token.mint(account, amount), "mint");
        require(token.balanceOf(account) == amount, "minted balance");
        require(token.totalSupply() == amount, "minted supply");
        require(token.owner() == address(this), "owner preserved");
    }

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

const ERC20LITE_IFACE_SOL: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

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
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_erc20lite_starter_without_foundry_counter() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().join("starter")).unwrap();
        init(&root, InitOptions::default()).unwrap();
        assert!(root.join("verity/src/ERC20Lite.lean").is_file());
        assert!(root.join("test/verity/ERC20Lite.t.sol").is_file());
        assert!(!root.join("src/Counter.sol").exists());
        assert!(!root.join("test/Counter.t.sol").exists());
        let proof = read_to_string(&root.join("verity/proof/ERC20LiteProof.lean")).unwrap();
        assert!(!proof.contains("sorry"));
        assert!(!proof.contains("Placeholder"));
        assert!(!proof.contains("coverage=proof_only"));
        assert!(proof.contains("tama: obligation kind=postcondition function=transfer"));
        assert!(proof.contains("((transfer toAddr amount).run s).snd"));
        assert!(proof.contains("((mint toAddr amount).run s).snd"));
        assert!(proof.contains("((balanceOf account).run s).fst"));
        let test = read_to_string(&root.join("test/verity/ERC20Lite.t.sol")).unwrap();
        assert!(!test.contains("testTransferPostPlaceholder"));
        assert!(test.contains("ERC20LiteDeployer.deploy(address(this))"));
        assert!(test.contains("testFuzzTransferPreservesTotalSupply"));
        assert!(test.contains("StdInvariant"));
        assert!(test.contains("invariant_totalSupplyTracksMinted"));
        assert!(test.contains("token.transfer(recipient, amount)"));
        let source = read_to_string(&root.join("verity/src/ERC20Lite.lean")).unwrap();
        assert!(source.contains("function view balanceOf"));
        assert!(!source.contains(r#"emit "Transfer""#));
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
            evm_version: "cancun".to_string(),
            metadata_hash: "none".to_string(),
        };
    }
}
