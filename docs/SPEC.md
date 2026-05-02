# Tama — Verity Developer Toolchain

**Authors:** Zefram (Foglight)  
**Modeled after:** Foundry (`forge`, `foundryup`), Cargo, Rustup

---

## 1. Motivation

Verity's current developer workflow is fragmented:

- `elan` / `lake build` for Lean elaboration and proof checking
- `lake exe verity-compiler` for Verity → Yul codegen
- `solc` for Yul → bytecode
- `forge test` for Solidity-side mirror and property tests
- A swarm of Python scripts (`generate_contract.py`, `check_selectors.py`, `check_yul.py`, `extract_property_manifest.py`, `report_property_coverage.py`, `check_contract_structure.py`, `check_storage_layout.py`, …) for project scaffolding, audits, and reports

This works for the framework's own monorepo, but it is not viable as a public developer experience. Tama is a single Rust binary that subsumes orchestration and provides a CLI shaped like `forge`, so Solidity developers can pick it up quickly.

`tama` is to Verity what `forge` is to Solidity. `tamaup` is to `tama` what `foundryup` is to `forge` and what `rustup` is to `cargo`.

---

## 2. Design principles

1. **A Tama project is also a Foundry project.** Users can run `forge` directly whenever they want.
2. **Lean, Verity, Yul, bytecode, and Foundry tests are one pipeline.** Tama coordinates the pipeline, but it does not hide the underlying tools.
3. **Config files are user-owned after init.** `tama init` creates sane defaults. After that, Tama only mutates files when the command explicitly says it will, such as `tama install`, `tama remove`, `tama update`, or `tama doctor --fix`.
4. **The generated manifest is the source of truth for Verity artifacts.** Selector maps, ABI fragments, storage layout, generated Yul, bytecode hashes, theorem obligations, and mirror-test links are recorded in `artifacts/manifest/`.
5. **Yul compilation is configured by Tama, not Foundry.** Foundry does not compile Verity's generated Yul. Tama invokes `solc` explicitly using Tama's own config.
6. **Audits should initially preserve Verity-script semantics.** Rust audit checks are semantic ports of the upstream Python scripts, with parity fixtures, before they are simplified or redesigned.

---

## 3. Project layout

A new project starts as a Foundry-compatible project plus the Verity layers:

```text
my-protocol/
├── tama.toml                    # Tama project marker, Verity pin, Yul-solc config, trust allowlist
├── tama.lock                    # Tama lockfile: resolved Verity rev, solc resolution, config hashes
├── foundry.toml                 # Standard Foundry config for Solidity tests/scripts
├── lakefile.toml                # Created by init; user-owned after init
├── lake-manifest.json           # Lake dependency manifest; checked into source control
├── lean-toolchain               # Pinned Lean toolchain
├── verity/
│   ├── src/                     # Flat EDSL implementations: ERC20Lite.lean, TipJar.lean, …
│   ├── spec/                    # Flat specs/invariants: ERC20LiteSpec.lean, TipJarSpec.lean, …
│   └── proof/                   # Flat proofs: ERC20LiteProof.lean, TipJarProof.lean, …
├── src/
│   └── generated/verity/        # Generated Solidity interfaces/deployers for Verity contracts
├── script/                      # Foundry deployment scripts
├── test/
│   ├── verity/                  # Foundry tests mirroring Verity obligations
│   │   └── ERC20Lite.t.sol
│   └── ...                      # User-authored Foundry tests
├── lib/                         # Foundry deps, e.g. forge-std
├── out/                         # Foundry build output
├── cache/                       # Foundry cache
└── artifacts/
    ├── lean/                    # Lake build directory for this package
    ├── yul/                     # Verity → Yul output
    ├── bytecode/                # solc-compiled creation/runtime bytecode
    ├── solc-json/               # solc standard-json inputs/outputs
    └── manifest/                # Contract manifests, selector maps, coverage reports, audit reports
```

The layout is flat per layer. There are no per-contract directories and no forced `Basic`/`Correctness` split. Large contracts may be split using normal Lean imports, but Tama does not impose that structure.

### 3.1 Per-contract file convention

For a contract `Foo`:

| Path                                   | Role                                                  |
| -------------------------------------- | ----------------------------------------------------- |
| `verity/src/Foo.lean`                  | EDSL implementation using `verity_contract`           |
| `verity/spec/FooSpec.lean`             | Specs, postconditions, invariants, frame helpers      |
| `verity/proof/FooProof.lean`           | Theorems proving implementation satisfies the spec    |
| `test/verity/Foo.t.sol`                | Foundry mirror/property tests                         |
| `src/generated/verity/FooIface.sol`    | Generated Solidity interface                          |
| `src/generated/verity/FooDeployer.sol` | Generated deployer embedding compiled Verity bytecode |
| `artifacts/manifest/Foo.json`          | Generated contract manifest                           |

The Lean filenames intentionally avoid dotted suffixes like `Foo.spec.lean`. Dots in Lean module names map to path separators, so the flat convention uses `FooSpec.lean` and `FooProof.lean` instead.

Inside `FooSpec.lean`, invariants are marked with an attribute such as `@[tama.invariant]`. Per-function obligations are marked with attributes that let Tama distinguish public contract obligations from helper lemmas.

Example shape:

```lean
import Foo

namespace FooSpec

section Invariants

@[tama.invariant]
def totalSupply_nonnegative := ...

end Invariants

section Functions

@[tama.postcondition Foo.transfer]
def transfer_preserves_totalSupply := ...

end Functions

end FooSpec
```

`FooProof.lean` imports `FooSpec.lean` and contains proof obligations. Helper lemmas may exist freely, but only obligations marked as public contract obligations participate in mirror-test coverage.

---

## 4. `tama.toml`

`tama.toml` has four roles:

1. mark the project as a Tama project;
2. pin the intended Verity framework version;
3. define Verity/Tama-side paths;
4. define Yul compilation and trust-boundary audit policy.

```toml
[project]
name = "my_protocol"
verity = "0.5.0"         # desired Verity framework version/tag

[paths]
src = "verity/src"                       # Verity EDSL contracts
spec = "verity/spec"                     # Specs and invariants
proof = "verity/proof"                   # Proof modules
mirror_test = "test/verity"              # Foundry mirror tests; normally under foundry.toml's test dir
generated_solidity = "src/generated/verity"
out = "artifacts"                        # Tama artifacts root

[yul]
solc = "0.8.33"                          # solc version Tama uses for generated Yul only
evm_version = "cancun"
optimizer = true
optimizer_runs = 200
yul_optimizer = true
metadata_bytecode_hash = "none"          # reproducible bytecode by default

[trust.allow_axioms]
"Classical.choice" = "accepted Lean classical reasoning"
"Quot.sound" = "accepted Lean quotient axiom"
```

`[paths]` is optional; omitted fields fall back to the defaults above.

`tama.toml` and `foundry.toml` are intentionally non-overlapping:

- Solidity contracts, Solidity test configuration, Foundry profiles, remappings, fuzz settings, RPC endpoints, and Solidity compiler settings stay in `foundry.toml`.
- Verity source/spec/proof paths, generated Verity bridge files, Verity artifact paths, Yul compilation settings, and trust-boundary policy live in `tama.toml`.

Tama does **not** read `foundry.toml` to configure Yul compilation. Tama invokes `solc` for generated Yul using `[yul]`; Foundry continues to handle Solidity compilation using `foundry.toml`.

---

## 5. `tama.lock`

`tama.lock` records the resolved state Tama needs for reproducible builds. It does not replace `lake-manifest.json`; Lake still owns Lean dependency resolution. Tama records the pieces of that resolution it cares about and validates that the lockfile, `tama.toml`, `lakefile.toml`, and `lake-manifest.json` agree.

Example shape:

```toml
version = 1

[resolved]
verity_version = "0.5.0"
verity_git = "https://github.com/lfglabs-dev/verity"
verity_rev = "3a6f..."
lean_toolchain = "leanprover/lean4:v4.22.0"
solc = "0.8.33"
solc_sha256 = "..."

[inputs]
tama_toml_sha256 = "..."
lakefile_toml_sha256 = "..."
lake_manifest_sha256 = "..."
foundry_toml_sha256 = "..."

[yul]
evm_version = "cancun"
optimizer = true
optimizer_runs = 200
yul_optimizer = true
metadata_bytecode_hash = "none"
```

Rules:

- `tama init` writes the initial lockfile.
- `tama update` refreshes `lake-manifest.json` and `tama.lock`.
- `tama build --locked` and CI fail if the lockfile is stale.
- `tama doctor` reports drift.
- `tama doctor --fix` may apply narrow, formatting-preserving repairs to `tama.lock` and dependency entries in `lakefile.toml`.

---

## 6. Lakefile ownership

`lakefile.toml` is created by `tama init`, then treated as user-owned.

Tama does not regenerate it during `tama build`. This is important: users may add pure Lake dependencies, custom Lean libraries, options, scripts, or comments. Build commands should not clobber those edits.

Tama may edit `lakefile.toml` only for commands whose job is explicitly to edit dependencies or repair drift:

- `tama install`
- `tama remove`
- `tama update`
- `tama doctor --fix`

Those edits must use formatting-preserving TOML edits.

The initial generated Lake config should:

- set Lake's build directory to `artifacts/lean`;
- define separate build targets for implementation/spec/proof modules;
- make `tama check` build only implementation + spec targets;
- make `tama build` build proof targets too;
- include the Verity framework dependency pinned to the version from `tama.toml`.

Illustrative shape:

```toml
name = "my_protocol"
version = "0.1.0"
defaultTargets = ["TamaProof"]
buildDir = "artifacts/lean"

[[require]]
name = "verity"
git = "https://github.com/lfglabs-dev/verity"
rev = "v0.5.0"

# Tama generates concrete Lake library/target entries using Lake's supported
# srcDir/root/glob configuration. The goal is:
# - TamaSrc:   verity/src/**/*.lean
# - TamaSpec:  verity/src/**/*.lean + verity/spec/**/*.lean
# - TamaProof: verity/src/**/*.lean + verity/spec/**/*.lean + verity/proof/**/*.lean
```

If a project replaces `lakefile.toml` with `lakefile.lean`, Tama still runs `lake` normally, but automatic `tama install` / `remove` / `doctor --fix` edits are not guaranteed. Tama should error with explicit manual instructions rather than silently rewriting a Lean lakefile.

---

## 7. Generated manifest

The Verity compiler must emit one manifest per contract, and Tama should treat that manifest as the shared interface between build, bridge generation, audit, inspect, and coverage.

Example:

```json
{
  "schema": "tama.contract-manifest.v1",
  "contract": "ERC20Lite",
  "lean": {
    "implementation_module": "src.ERC20Lite",
    "spec_module": "spec.ERC20LiteSpec",
    "proof_module": "proof.ERC20LiteProof"
  },
  "abi": [
    {
      "name": "balanceOf",
      "signature": "balanceOf(address)",
      "selector": "0x70a08231",
      "mutability": "view",
      "returns": ["uint256"]
    },
    {
      "name": "transfer",
      "signature": "transfer(address,uint256)",
      "selector": "0xa9059cbb",
      "mutability": "nonpayable",
      "returns": ["bool"]
    }
  ],
  "storage": [
    {
      "name": "balances",
      "type": "mapping(address => uint256)",
      "slot": "1",
      "offset": 0
    }
  ],
  "artifacts": {
    "yul": "artifacts/yul/ERC20Lite.yul",
    "solc_input": "artifacts/solc-json/ERC20Lite.input.json",
    "solc_output": "artifacts/solc-json/ERC20Lite.output.json",
    "creation_bytecode": "artifacts/bytecode/ERC20Lite.bin",
    "runtime_bytecode": "artifacts/bytecode/ERC20Lite.runtime.bin",
    "bytecode_hash": "..."
  },
  "obligations": [
    {
      "name": "ERC20LiteProof.transfer_preserves_total_supply",
      "kind": "invariant",
      "mirror": "test/verity/ERC20Lite.t.sol:ERC20LiteTest.testTransferPreservesTotalSupply"
    }
  ]
}
```

The manifest is not a report-only artifact. It is the canonical source used by:

- generated Solidity interfaces;
- generated deployers;
- `tama inspect`;
- selector audits;
- storage-layout audits;
- property-coverage audits;
- CI reports.

---

## 8. Generated Solidity bridge

Tama generates Solidity bridge files from the manifest after Yul bytecode has been compiled and before `forge build` runs.

For `ERC20Lite`, Tama emits:

```text
src/generated/verity/ERC20LiteIface.sol
src/generated/verity/ERC20LiteDeployer.sol
```

`ERC20LiteIface.sol` exposes the ABI-level interface implied by the Verity contract:

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

interface ERC20LiteIface {
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
}
```

`ERC20LiteDeployer.sol` embeds the Verity-compiled creation bytecode:

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {ERC20LiteIface} from "./ERC20LiteIface.sol";

library ERC20LiteDeployer {
    function deploy() internal returns (ERC20LiteIface token) {
        bytes memory code = hex"...";
        address addr;
        assembly {
            addr := create(0, add(code, 0x20), mload(code))
        }
        require(addr != address(0), "TAMA_DEPLOY_FAILED");
        token = ERC20LiteIface(addr);
    }
}
```

Mirror tests import the generated deployer/interface and test the actual Verity-generated bytecode, not a Solidity reimplementation.

---

## 9. Commands

Commands should keep the happy path terse, but flags are allowed where they are genuinely useful.

Global flags:

```text
--root <path>       Run as if invoked from another project root
--locked            Fail if tama.lock / lake-manifest.json are stale
--offline           Do not access the network
--json              Emit machine-readable output where supported
--verbose, -v       Increase logging verbosity
--no-color          Disable colored output
```

Environment:

```text
TAMA_LAKE_PACKAGE_CACHE
                    Directory of trusted reusable Lake package checkouts. By
                    default Tama uses the platform user cache directory:
                    `~/Library/Caches/tama/lake-packages` on macOS and
                    `$XDG_CACHE_HOME/tama/lake-packages` or
                    `~/.cache/tama/lake-packages` elsewhere. Set this variable
                    to a different directory, or to `off` to disable caching.

                    Tama may seed `.lake/packages` from this directory before
                    `lake update` and copy newly fetched packages back
                    afterward. The cache is a performance optimization only;
                    release and locked builds must remain reproducible from an
                    empty cache.
```

### `tama init [path]`

Scaffolds a new Tama project.

It does **not** keep Foundry's default example contracts. The result should be a Tama-native starter project: a real Verity `ERC20Lite`, its spec, complete starter proofs, generated bridge files, and mirror Foundry tests.

Steps:

1. Create a Foundry-compatible directory layout.
2. Write a minimal `foundry.toml` for Solidity tests/scripts.
3. Write `tama.toml`.
4. Write `tama.lock`.
5. Write `lakefile.toml` with Lake build output configured under `artifacts/lean`.
6. Write `lean-toolchain` pinned to the compatible Lean version.
7. Create `verity/src`, `verity/spec`, `verity/proof`, `test/verity`, `src/generated/verity`.
8. Generate the `ERC20Lite` starter across implementation, spec, proof, bridge, manifest, and mirror test layers.
9. Install `forge-std` using Foundry.
10. Pin the Verity Lean dependency in `lakefile.toml` and `lake-manifest.json`.
11. Print next steps.

### `tama new <Name>`

Scaffolds a new Verity contract.

```text
tama new TipJar
```

Generates:

- `verity/src/TipJar.lean`
- `verity/spec/TipJarSpec.lean`
- `verity/proof/TipJarProof.lean`
- `test/verity/TipJar.t.sol`

The generated files are skeletons. Bridge files and manifests are generated by `tama build`, not by `tama new`, because they depend on successful codegen and bytecode compilation.

`tama new` should not rewrite `lakefile.toml`; the initial Lake targets/globs must already cover new flat files under the configured source/spec/proof paths.

### `tama check`

Fast Lean check for implementation and spec modules only.

Equivalent behavior:

1. Build the implementation target.
2. Build the spec target.
3. Do not import proof modules.
4. Do not run Verity Yul codegen.
5. Do not run `solc`.
6. Do not run Foundry.

This is not `lake build --no-build`. It is a real Lean elaboration of implementation + spec targets, excluding proof targets.

`tama check` is fast by convention: spec modules should contain declarations and obligations, while expensive proof scripts should live in proof modules.

### `tama build`

The full pipeline:

1. `lake build <proof-target>` — elaborate implementation, spec, and proof modules.
2. `lake exe verity-compiler` — emit Yul and per-contract manifests.
3. `solc` via standard JSON — compile generated Yul according to `[yul]` in `tama.toml`.
4. Generate Solidity bridge files from manifests and bytecode.
5. `forge build` — compile user Solidity, generated bridge files, and tests.

Lean proof checking is part of elaboration. A real proof failure fails the build. `sorry` and `axiom` may pass Lean elaboration but are handled by `tama audit trust-boundary`.

Useful flags:

```text
--locked            Require lockfiles to be current
--no-forge          Stop after bridge generation; do not run forge build
--no-solc           Stop after Yul/manifest generation
--contract <Name>   Build one contract where supported by the compiler
```

When `--no-solc` is used, Tama must remove downstream generated artifacts for
the selected contracts: solc JSON, bytecode files, and generated Solidity
bridges. The manifest remains, but `bytecode_hash` is `null`, so `inspect
bytecode`, bridge-dependent audits, and release gates cannot accidentally pass
using stale outputs from an earlier full build.

### `tama test [forge-args...]`

Pure syntactic sugar for:

```text
forge test [forge-args...]
```

Tama does not inject test paths, does not override filters, and does not force mirror tests to run. Mirror tests are included by default because they live under Foundry's normal test tree. If the user passes Foundry filters that exclude mirror tests, Tama respects that. Coverage enforcement belongs to `tama audit coverage`, not `tama test`.

Examples:

```text
tama test
tama test -vvv
tama test --match-contract ERC20LiteTest
tama test --match-path test/verity/ERC20Lite.t.sol
```

### `tama audit [check-name]`

Runs release/CI sanity checks after `tama build` succeeds.

```text
tama audit
tama audit selectors
tama audit trust-boundary
```

Checks:

#### `selectors`

Verifies selector consistency across:

1. Verity declarations;
2. generated manifest entries;
3. generated Solidity interfaces;
4. expected keccak-derived function selectors.

The initial implementation should port the upstream Verity selector-check script semantics into Rust and maintain golden parity tests against the Python script behavior.

#### `storage-layout`

Verifies storage-layout consistency across:

1. Verity storage declarations;
2. generated manifest storage entries;
3. generated Yul assumptions;
4. generated Solidity bridge/harness expectations where applicable.

This should also be a Rust semantic port of the upstream Verity storage-layout script, backed by fixtures.

#### `coverage`

Checks that every public contract obligation has a mirror classification.

Coverage is not over every theorem in `verity/proof/`. Helper lemmas are not obligations. Proof-only facts may be intentionally non-executable. Public contract obligations must be explicitly marked with Tama attributes or `-- tama:` metadata comments and either:

- linked to a Foundry mirror test; or
- marked proof-only with a reason.

Example Lean shape:

```lean
@[tama.obligation]
@[tama.mirror "test/verity/ERC20Lite.t.sol:ERC20LiteTest.testTransferPreservesTotalSupply"]
theorem transfer_preserves_total_supply : ... := by
  ...

@[tama.helper]
theorem arithmetic_helper : ... := by
  ...

@[tama.proof_only "quantifies over symbolic state; no executable mirror"]
theorem symbolic_refinement : ... := by
  ...
```

The comment form exists for projects that have not loaded no-op Tama Lean attributes yet:

```lean
-- tama: obligation kind=postcondition function=transfer coverage=mirror path=test/verity/ERC20Lite.t.sol:ERC20LiteTest.testTransferPost
theorem transfer_post : ... := by
  ...
```

#### `structure`

Checks project layout sanity:

- all contracts have expected implementation/spec/proof/test file names;
- generated bridge files correspond to manifests;
- configured paths exist;
- mirror test path is under Foundry's test tree;
- manifests are present and schema-compatible.

#### `trust-boundary`

Checks the Lean trust boundary for public contract obligations.

For each public obligation, Tama queries or extracts the Lean environment dependency set and fails if the theorem depends on:

- `sorryAx`;
- an unallowlisted axiom;
- an unsafe declaration;
- a new trusted declaration outside the approved framework boundary.

The allowlist lives in `tama.toml` under `[trust.allow_axioms]`. `sorryAx` is hard-denied and is not allowlistable for `tama audit`.

Tama also consumes Verity compiler trust-surface artifacts when present. `artifacts/trust-report.json` localizes unsupported or partially modeled mechanics, unsafe blocks, and unchecked dependency buckets; `artifacts/assumption-report.json` flattens undischarged compiler assumptions. Undischarged assumptions must be explicitly allowlisted by their stable assumption or axiom identifier.

### `tama inspect <Contract> <field>`

`forge inspect` analog for Verity artifacts.

Fields:

- `manifest`
- `selectors`
- `storage-layout`
- `abi`
- `yul`
- `bytecode`
- `runtime-bytecode`
- `theorems`
- `obligations`
- `mirrors`

Examples:

```text
tama inspect TipJar manifest
tama inspect TipJar selectors
tama inspect TipJar obligations
```

For Solidity contracts that are not generated by Verity, use `forge inspect` directly.

### `tama clean`

Removes generated outputs:

```text
artifacts/yul/
artifacts/bytecode/
artifacts/solc-json/
artifacts/manifest/
artifacts/lean/
out/
cache/
src/generated/verity/
```

It does not remove `.lake/packages` by default. Use `tama clean --deep` to remove Lake dependency/build cache state as well.

### `tama install <repo>[@<version>]`

Adds a Tama package as a Lean dependency.

```text
tama install lfglabs-dev/verity-erc20
tama install lfglabs-dev/verity-erc20@v0.2.0
tama install lfglabs-dev/verity-erc20@some-branch
```

Rules:

1. The target repo must contain `tama.toml`.
2. Tama reads the dependency's package metadata.
3. Tama appends or updates a `require` block in the consumer's `lakefile.toml` using formatting-preserving edits.
4. Tama runs `lake update`.
5. Tama updates `tama.lock`.

Without `@version`, Tama resolves and pins the repo's default branch head in `tama.lock`.

If the target repo lacks `tama.toml`, Tama errors and tells the user to add the dependency manually to `lakefile.toml`. Pure Lake libraries are intentionally outside `tama install`'s scope.

For Solidity-side dependencies, use `forge install` directly.

### `tama remove <package>`

Removes a Tama/Lake dependency from `lakefile.toml`, runs `lake update`, and updates `tama.lock`.

```text
tama remove verity-erc20
```

For Solidity-side dependencies, use `forge remove` directly.

### `tama update`

Updates resolved dependency state.

Default behavior:

1. Ensure the `verity` dependency in `lakefile.toml` matches `tama.toml`.
2. Run `lake update`.
3. Run `forge update` for Foundry dependencies.
4. Recompute `tama.lock`.

Useful flags:

```text
--locked            Fail instead of updating if anything is stale
--no-forge          Skip forge update
--no-lake           Skip lake update
--package <name>    Update one named dependency where supported
```

To bump Verity, edit `tama.toml` and run `tama update`.

### `tama doctor`

Prints versions, config resolution, and drift checks.

```text
$ tama doctor
✓ tama          0.1.0
✓ verity        0.5.0    resolved 3a6f...; matches tama.toml and lakefile.toml
✓ lean          4.22.0   matches lean-toolchain
✓ lake          5.0.0
✓ forge         1.5.2
✓ solc          0.8.33   matches tama.toml [yul]
✓ lake buildDir artifacts/lean
✓ lockfile      current
```

`doctor --fix` may:

- update stale `tama.lock` hashes;
- update the Verity `require` block in `lakefile.toml` to match `tama.toml`;
- repair missing generated directories;
- warn about, but not overwrite, user-authored config it cannot safely edit.

---

## 10. Yul compilation

Tama compiles generated Yul using `solc` standard JSON input/output.

For each contract:

1. Verity emits `artifacts/yul/Foo.yul` and `artifacts/manifest/Foo.json`.
2. Tama writes `artifacts/solc-json/Foo.input.json` using `[yul]` settings.
3. Tama invokes the resolved `solc` binary with standard JSON.
4. Tama writes `artifacts/solc-json/Foo.output.json`.
5. Tama extracts creation bytecode and runtime bytecode into `artifacts/bytecode/`.
6. Tama updates the manifest with bytecode paths and hashes.

Tama should not use ad hoc parsing of `solc --bin` output for production builds. Standard JSON gives deterministic inputs, machine-readable outputs, explicit optimizer settings, and better auditability.

---

## 11. Audit implementation strategy

The first implementation of `tama-audit` should clone the behavior of upstream Verity scripts as closely as possible.

Concretely:

- port the Python scripts to Rust check-by-check;
- preserve edge-case behavior unless intentionally changed;
- build fixture tests where Python output and Rust output are compared on the same sample contracts;
- only then refactor around the manifest-first architecture.

This avoids replacing known-but-ugly behavior with clean-looking behavior that accidentally changes semantics.

The desired endpoint is not "Rust scripts that happen to look like Python scripts." The desired endpoint is:

```text
Verity declarations → manifest → generated bridge → bytecode → audit reports
```

But parity comes first.

---

## 12. `tamaup`

`tamaup` installs and updates the Tama toolchain on Linux and macOS.

Supported platforms:

```text
linux-x86_64
linux-aarch64
macos-x86_64
macos-aarch64
```

No native Windows support is required for v1.

Entry point:

```text
curl -L https://tama.tools/install.sh | sh
```

Commands:

```text
tamaup                       # install/update latest stable
tamaup install nightly       # install nightly channel
tamaup install 0.1.3         # install specific version
tamaup use 0.1.3             # switch active version
tamaup list                  # installed versions
tamaup self update           # update tamaup itself
tamaup uninstall             # remove tama; keep tamaup
```

Security requirements:

1. Releases publish a signed manifest containing artifact URLs, versions, platform triples, and SHA-256 hashes.
2. Installer verifies the signed manifest before installing binaries.
3. Installer verifies artifact SHA-256 hashes before executing or placing binaries on `$PATH`.
4. `tamaup` repeats signature and checksum verification for every install/update.
5. Installer supports noninteractive and opt-out flags:

```text
--yes
--no-modify-path
--no-install-lean
--no-install-foundry
--offline
--version <version>
```

6. If the local machine lacks the verifier needed for signature checks, the installer fails with manual installation instructions rather than silently falling back to unchecked downloads.

Toolchain behavior:

1. `tamaup` installs `tama` into `~/.tama/versions/<version>/bin/tama`.
2. `~/.tama/bin/tama` is a shim or symlink to the active version.
3. `tamaup` checks for compatible `lean`, `lake`, and `forge` on `$PATH`.
4. If Lean is absent and `--no-install-lean` was not passed, `tamaup` installs/uses `elan` and installs the compatible Lean version.
5. If Foundry is absent and `--no-install-foundry` was not passed, `tamaup` installs/uses `foundryup`.
6. Project-local `lean-toolchain` always wins when running inside a project. `tamaup` provides compatible tools; `lake`/`elan` obey the project pin.

---

## 13. Rust crate layout

```text
tama/
├── Cargo.toml
├── crates/
│   ├── tama-cli/               # bin: tama
│   ├── tamaup-cli/             # bin: tamaup
│   ├── tama-config/            # tama.toml, tama.lock, foundry.toml, lean-toolchain reading
│   ├── tama-project/           # init, scaffolding, tama new
│   ├── tama-build/             # lake / verity-compiler / solc / bridge generation / forge
│   ├── tama-manifest/          # manifest schema, reading, writing, validation
│   ├── tama-audit/             # selectors, storage, coverage, structure, trust-boundary
│   ├── tama-inspect/           # inspect commands over manifests/artifacts
│   ├── tama-toolchain/         # external tool discovery/version checks
│   └── tama-common/            # shared errors, paths, logging
└── installer/
    └── install.sh
```

Key dependencies:

- `clap` — CLI
- `serde`, `serde_json`, `toml_edit` — config, manifests, formatting-preserving edits
- `tracing` — structured logs and Foundry-style terminal output
- `which` — tool discovery
- `xshell` or `duct` — process orchestration
- `sha2` — artifact/config hashing
- `keccak-hash` or equivalent — selector computation
- `camino` — UTF-8 paths
- `thiserror`, `miette` — error reporting

Internal interfaces:

```rust
Lake::check_src_and_spec()
Lake::build_proofs()
VerityCompiler::emit_yul_and_manifests()
Solc::compile_yul_standard_json()
BridgeGenerator::generate_solidity()
Forge::build()
Forge::test_passthrough(args)
Audit::run(checks)
```

External tools are wrapped behind thin structs with structured errors and mockable process execution.

---

## 14. CI defaults

Recommended CI:

```text
tama build --locked
tama test
tama audit
```

A stricter release CI can add:

```text
tama doctor
tama audit trust-boundary
tama audit coverage
```

Local development commonly uses:

```text
tama check
tama build
tama test --match-contract ERC20LiteTest
```

`sorry` is allowed during local `tama build` because Lean allows it. It is rejected by `tama audit trust-boundary`.
