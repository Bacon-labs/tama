# IMPLEMENTATION_PLAN — Tama v0.1 Production-Ready Minimal Release

## Purpose

Build Tama into a production-ready, feature-minimal developer tool for Verity smart-contract projects. The end result is not a Rust workspace skeleton and not a mocked Counter demo. The end result is a real CLI that a Verity developer can install, use to create a fresh project, compile real Verity code, generate Yul and bytecode, run Foundry tests, audit proof/test coverage and trust boundaries, inspect artifacts, and diagnose toolchain drift.

This plan supersedes the earlier implementation plan. The earlier plan was useful as a crate-by-crate scaffold, but it allowed too many stubbed success paths and assumed Verity compiler/audit features that may not exist. This version makes those assumptions explicit release blockers.

## Definition of “production-ready but feature-minimal”

Tama v0.1 is production-ready when it is reliable inside its declared scope, not when it supports every possible Verity or Solidity feature.

### In scope for v0.1

- Linux x86_64, Linux aarch64, macOS x86_64, macOS aarch64.
- No Windows support.
- Flat Verity project layout:
  - `verity/src/Foo.lean`
  - `verity/spec/FooSpec.lean`
  - `verity/proof/FooProof.lean`
  - `test/verity/Foo.t.sol`
- Generated aggregate Lean modules:
  - `TamaSrc.lean`
  - `TamaSpec.lean`
  - `TamaProof.lean`
- User-owned `lakefile.toml` after init.
- Explicit Tama Yul/solc config in `tama.toml`.
- `tama.lock` for resolved Verity, Lake, Foundry, solc, and input hashes.
- Real `tama init`, `tama new`, `tama check`, `tama build`, `tama test`, `tama audit`, `tama inspect`, `tama clean`, `tama doctor`, `tama update`, `tama install`, and `tama remove`.
- Real Verity integration, or an explicitly implemented and tested Tama-side compatibility adapter if upstream Verity lacks the required manifest emission.
- Real Foundry integration.
- Real solc standard-JSON Yul compilation.
- Generated Solidity bridge files for Foundry tests.
- A complete starter ERC20Lite project with no `sorry` in the default proof path.
- A smaller Counter compatibility fixture that keeps the simplest generated-contract path covered without defining the starter experience.
- Release artifacts and installer verification suitable for public distribution.

### Out of scope for v0.1

- Windows support.
- Full Solidity feature parity.
- Automatic support for every Verity monorepo script.
- Full semantic storage proof of arbitrary Yul.
- Background daemons, LSP integration, watch mode.
- A public package registry.

Unsupported Verity/Solidity features must be documented in `docs/LIMITATIONS.md`. Unsupported does not mean half-working. If a feature is unsupported, Tama must fail with a clear diagnostic before emitting misleading artifacts.

## Non-negotiable release gates

A release candidate cannot be tagged until every gate below is green.

| Gate                  | Pass criterion                                               | Fail criterion                                               |
| --------------------- | ------------------------------------------------------------ | ------------------------------------------------------------ |
| Fresh starter         | `tama init tmp && cd tmp && tama check && tama build && tama test && tama audit` passes with the real toolchain | Any step is skipped, stubbed, gated on missing tools in release CI, or fails because of `sorry` |
| Real Verity           | `tama build` invokes real Verity codegen or a committed real adapter backed by Verity artifacts | Build succeeds only because a stub emitted canned Yul/manifest |
| Real solc             | Generated Yul compiles through `solc --standard-json`, and Tama fails on JSON `errors[].severity == "error"` | Tama only checks solc process exit status                    |
| Real Foundry          | `tama test` runs `forge test` unchanged except global `--offline`, and mirror tests pass | Tama mutates user Forge filters or replaces Foundry with a fake runner |
| Starter proof         | Starter ERC20Lite has complete proofs and passes trust audit | Starter contains `sorry`, unallowlisted axioms, or starter-only audit bypasses |
| Counter fixture       | Counter fixture also passes check/build/test/audit/inspect end-to-end | Only the ERC20Lite starter is tested end-to-end               |
| Audit negative tests  | Injected selector/storage/coverage/structure/trust failures are caught | Audits only prove happy paths                                |
| Locking               | `tama build --locked` and CI fail on stale lock/input drift  | Drift is logged but allowed under `--locked`                 |
| Generated-file safety | Tama overwrites only generated files with intact headers and refuses hand-edited generated files | User edits are silently clobbered                            |
| Installer integrity   | Release install verifies signed manifest and artifact SHA-256 before extraction | Unsigned downloads or unchecked tarball extraction           |
| Docs                  | Quickstart, command reference, limitations, compatibility, and release docs exist and match behavior | User must infer behavior from source                         |

## Updated decisions

1. Rust edition is `2021`. MSRV is `1.81`. Set `[workspace.package] rust-version = "1.81"`; `rust-toolchain.toml` may pin `stable`, but CI must run at least one job on Rust `1.81`.
2. Virtual workspace at repo root. Member crates live under `crates/`.
3. Public path APIs use `camino::Utf8PathBuf` / `&Utf8Path`. `std::path::Path` is allowed at OS/process boundaries only.
4. Read-only TOML ingest uses `toml` + serde. Any Tama rewrite uses `toml_edit` and must preserve unrelated comments/order.
5. `docs/SPEC.md` is updated to the revised spec before implementation. Stale spec text is not allowed to drive codegen.
6. Flat contract files are retained, but Lake builds through generated aggregate modules: `TamaSrc.lean`, `TamaSpec.lean`, `TamaProof.lean`.
7. `lakefile.toml` is generated at init and user-owned afterward. Tama may edit it only for explicit dependency commands (`install`, `remove`, `update`, `doctor --fix`) through formatting-preserving narrow edits.
8. Lake output is configured with `buildDir = "artifacts/lean"`. `tama clean --deep` removes the configured Lake build dir, not hardcoded `.lake/build` unless that is the configured dir.
9. `tama check` builds only implementation + spec aggregate targets. It must not import proof modules.
10. `tama build` builds proofs before compiler codegen. Local builds may allow Lean warnings, but `tama audit trust-boundary` is the CI gate for `sorry` and unallowlisted axioms.
11. Contract manifest schema string is `tama.contract-manifest.v1`. Any other schema is a hard typed error.
12. Manifest obligations use fully qualified Lean declaration names. Never infer theorem names from filenames.
13. Proof-only is a coverage disposition, not an obligation kind.
14. Function/error selectors are Keccak-256(signature) first four bytes. Event topic0 is full Keccak-256(signature). All are lowercase hex with `0x` prefix in manifests.
15. SHA-256 artifact hashes are lowercase hex without `0x`.
16. Tama invokes solc only via `--standard-json`; `--bin` is forbidden in production build path.
17. Solc standard-JSON output is parsed. Any `errors[]` entry with severity `error` fails the build regardless of subprocess exit code.
18. Tama’s Yul compiler configuration is explicit in `[yul]` inside `tama.toml`; it does not inherit Foundry compiler settings.
19. `tama test` is exact passthrough to `forge test` except that Tama's global `--offline` maps to Forge's `--offline`. Wrapper logs go to stderr. Forge stdout/stderr and exit code are preserved.
20. CLI uses `clap` derive. `tama test` captures trailing hyphenated args correctly; `tama test -- --match-test foo -vvv` must work, and direct `tama test --match-test foo -vvv` should work if Clap configuration supports it safely.
21. `--json` applies to Tama-owned output only. It must not corrupt Forge passthrough output.
22. Generated Solidity bridge files start with `// GENERATED by tama; do not edit.`. Tama overwrites only files with this exact first line.
23. The starter ERC20Lite must have complete proofs. No `sorry`, no starter-only trust/audit bypass. Counter is a secondary compatibility fixture, not the starter template.
24. Existing Verity Python scripts, if present in the pinned upstream commit, are semantic references. Rust parity compares canonical issue sets, not byte-for-byte stdout.
25. If required Python scripts are not present in upstream Verity, Tama implements the audit from the manifest/spec directly and documents that there is no upstream parity baseline for that check.
26. Trust-boundary audit uses a generated Lean probe that calls Lean’s axiom-collection API and emits JSON. Text parsing of `#print axioms` is fallback-only and must be documented as such.
27. Trust-boundary audit hard-denies `sorryAx` and any axiom not listed in `[trust.allow_axioms]` in `tama.toml`. The allowlist value is a required human-readable reason.
28. Verity compatibility bugs are never patched silently in a user project. Required fixes must be upstreamed, pinned to a Tama-maintained Verity fork/commit, or represented as deterministic patches recorded in `tama.lock`.
29. Unit tests may use tool stubs. Release CI must include non-stubbed e2e jobs with real Lean, Lake, Verity, solc, and Forge.
30. `tamaup` uses in-process signature verification for release manifests and SHA-256 verification for artifacts. The shell installer may require an external verifier, but `tamaup` itself must not depend on `minisign` being installed.
31. No Windows release artifacts in v0.1.
32. `Cargo.lock` is committed.
33. Every phase must compile and pass tests before commit. Any deferral becomes a new phase with pass/fail criteria.

## Project layout generated by `tama init`

```text
my-protocol/
├── tama.toml
├── tama.lock
├── foundry.toml
├── lakefile.toml
├── lake-manifest.json
├── lean-toolchain
├── TamaSrc.lean
├── TamaSpec.lean
├── TamaProof.lean
├── verity/
│   ├── src/
│   │   └── ERC20Lite.lean
│   ├── spec/
│   │   └── ERC20LiteSpec.lean
│   └── proof/
│       └── ERC20LiteProof.lean
├── src/
│   └── generated/
│       └── verity/
│           ├── ERC20LiteIface.sol
│           └── ERC20LiteDeployer.sol
├── test/
│   └── verity/
│       └── ERC20Lite.t.sol
├── lib/
├── script/
│   └── ERC20Lite.s.sol
├── artifacts/
│   ├── yul/
│   ├── bytecode/
│   ├── solc-json/
│   ├── manifest/
│   ├── lean/
│   └── trust-probe/
└── docs/
    └── README.md
```

Aggregate modules import the configured Lake module roots. The generated files live under `verity/`, but Lake maps them through `srcDir = "verity"` so local modules do not conflict with the upstream dependency package directory named `verity`.

```lean
-- TamaSrc.lean
import src.ERC20Lite
```

```lean
-- TamaSpec.lean
import TamaSrc
import spec.ERC20LiteSpec
```

```lean
-- TamaProof.lean
import TamaSpec
import proof.ERC20LiteProof
```

`tama new Foo` writes the four per-contract files and updates the aggregate modules, not the Lakefile.
If the configured source/spec/proof paths are no longer covered by the user-owned Lakefile module roots, `tama new` refuses before writing rather than creating files Lake cannot import.

## Manifest schema requirements

The manifest is the center of the toolchain. Audit, inspect, bridge generation, and test wiring read the manifest. The manifest is either emitted upstream by Verity or produced by a committed Tama adapter from Verity’s real artifacts.

Minimum `tama.contract-manifest.v1` fields:

```json
{
  "schema": "tama.contract-manifest.v1",
  "contract": "ERC20Lite",
  "source": {
    "implementation": "verity/src/ERC20Lite.lean",
    "spec": "verity/spec/ERC20LiteSpec.lean",
    "proof": "verity/proof/ERC20LiteProof.lean"
  },
  "lean": {
    "implementation_module": "src.ERC20Lite",
    "spec_module": "spec.ERC20LiteSpec",
    "proof_module": "proof.ERC20LiteProof"
  },
  "abi": {
    "constructor": null,
    "functions": [],
    "events": [],
    "errors": []
  },
  "storage": [],
  "obligations": [],
  "artifacts": {
    "yul": "artifacts/yul/ERC20Lite.yul",
    "creation_bytecode": "artifacts/bytecode/ERC20Lite.bin",
    "runtime_bytecode": "artifacts/bytecode/ERC20Lite.runtime.bin",
    "bytecode_hash": null,
    "solc_input": "artifacts/solc-json/ERC20Lite.input.json",
    "solc_output": "artifacts/solc-json/ERC20Lite.output.json",
    "interface": "src/generated/verity/ERC20LiteIface.sol",
    "deployer": "src/generated/verity/ERC20LiteDeployer.sol"
  }
}
```

Function entry:

```json
{
  "name": "transfer",
  "signature": "transfer(address,uint256)",
  "selector": "0xa9059cbb",
  "visibility": "external",
  "mutability": "nonpayable",
  "inputs": [
    { "name": "to", "type": "address" },
    { "name": "amount", "type": "uint256" }
  ],
  "outputs": [
    { "name": "ok", "type": "bool" }
  ]
}
```

Event entry:

```json
{
  "name": "Transfer",
  "signature": "Transfer(address,address,uint256)",
  "topic0": "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
  "fields": [
    { "name": "from", "type": "address", "indexed": true },
    { "name": "to", "type": "address", "indexed": true },
    { "name": "amount", "type": "uint256", "indexed": false }
  ]
}
```

Error entry:

```json
{
  "name": "InsufficientBalance",
  "signature": "InsufficientBalance(address,uint256,uint256)",
  "selector": "0x...",
  "inputs": []
}
```

Storage entry:

```json
{
  "name": "balances",
  "type": "mapping(address => uint256)",
  "slot": "0x01",
  "offset": 0,
  "width_bytes": 32,
  "encoding": "mapping"
}
```

Obligation entry:

```json
{
  "id": "ERC20Lite.transfer_post",
  "name": "transfer_post",
  "kind": "postcondition",
  "lean_decl": "proof.ERC20LiteProof.transfer_post",
  "contract": "ERC20Lite",
  "function": "transfer",
  "coverage": {
    "disposition": "mirror",
    "path": "test/verity/ERC20Lite.t.sol:ERC20LiteTest.testFuzzTransferPreservesTotalSupply"
  }
}
```

Proof-only coverage disposition:

```json
{
  "disposition": "proof_only",
  "reason": "Quantifies over symbolic EVM states and is not meaningfully executable as a Foundry mirror."
}
```

Valid obligation kinds:

```text
invariant
postcondition
helper
```

Valid coverage dispositions:

```text
mirror
proof_only
none
```

For `invariant` and `postcondition`, `coverage.disposition` must be `mirror` or `proof_only`. `helper` obligations are excluded from coverage requirements but still visible in `tama inspect`.

## Phases

### Phase -1 — Verity prerequisite audit and compatibility decision

This phase happens before Rust implementation. It is a release blocker.

#### Work

1. Clone the exact upstream Verity repository/commit intended for v0.1.
2. Record it in `docs/VERITY_COMPAT.md` with:
   - repository URL
   - commit SHA
   - Lean version from `lean-toolchain`
   - Lake manifest hash
   - known required patches
   - supported Verity feature subset
3. Verify whether upstream `verity-compiler` can compile an external project outside the Verity monorepo.
4. Verify the actual compiler command and flags. Do not assume `--manifest-out` exists.
5. Verify whether upstream emits enough metadata for `tama.contract-manifest.v1`:
   - ABI functions
   - constructor data, if supported
   - events
   - errors, if supported
   - storage layout
   - generated Yul path
   - proof obligations
   - fully qualified Lean declarations
6. Verify whether upstream scripts exist in the pinned commit:
   - `check_selectors.py`
   - `check_contract_structure.py`
   - `check_storage_layout.py`
   - `report_property_coverage.py`
   - any manifest extraction scripts
7. Check the known Verity footguns explicitly:
   - eventless contracts must compile, or templates/compiler pin must handle the empty-events case.
   - ECM/Oracle `mload(0x40)` memory pointer behavior must be fixed upstream or pinned to a compatible fork/commit.
8. Build two handwritten fixture projects outside the Verity monorepo:
   - `fixtures/verity/Counter`
   - `fixtures/verity/ERC20Lite`
9. Decide one path:
   - **Path A — upstream manifest:** Verity emits `tama.contract-manifest.v1` directly.
   - **Path B — Tama adapter:** Tama derives the manifest from real Verity outputs and Lean reflection/probe results.
   - **Path C — block:** Required data cannot be obtained reliably; implementation stops until upstream Verity changes land.
10. Write `docs/VERITY_COMPAT.md` with the decision and exact commands that passed.

#### Pass/fail

Pass only if:

- ERC20Lite compiles outside the Verity monorepo.
- Counter compiles outside the Verity monorepo or the exact blocking Verity feature is documented and accepted as a v0.1 limitation.
- A real path exists to produce complete v1 manifests.
- Known compiler footguns are either fixed in the pinned Verity commit or accounted for by a deterministic compatibility decision.
- All assumptions are in `docs/VERITY_COMPAT.md`.

Fail if:

- Manifest emission is assumed but not proven.
- Required Python scripts are assumed but not found.
- Required Verity patches would be applied silently to user projects.
- Only monorepo-local Verity examples work.

### Phase 0a — Spec sync and stale-plan cleanup

#### Work

1. Replace `docs/SPEC.md` with the revised spec:
   - flat Lean-valid file layout
   - user-owned Lakefile after init
   - explicit `[yul]` config
   - `tama.lock`
   - generated manifest/interface/deployer artifacts
   - exact `tama test` passthrough
   - trust allowlist in `tama.toml`
   - no Foundry default template contracts
   - Lake build output under `artifacts/lean`
2. Add `docs/LIMITATIONS.md` from Phase -1.
3. Add `docs/VERITY_COMPAT.md` from Phase -1.
4. Add `docs/QUICKSTART.md` with the exact v0.1 happy path.
5. Add `docs/COMMANDS.md` with command behavior and global flags.
6. Add `docs/RELEASE.md` with signing, release, and installer instructions.

#### Pass/fail

Pass only if:

- `docs/SPEC.md` no longer contains dotted file names like `Foo.spec.lean` / `Foo.proof.lean`.
- `docs/SPEC.md` no longer says `tama check` maps to `lake build --no-build`.
- `docs/SPEC.md` no longer says Lakefile is regenerated on build.
- `docs/SPEC.md` no longer says Yul solc config is inherited from Foundry.
- `docs/SPEC.md` no longer says commands are flagless.
- `docs/SPEC.md` no longer starts from Foundry’s default template contracts.

### Phase 0 — Workspace scaffold

#### Work

1. Create root `Cargo.toml` virtual workspace with members:
   - `crates/tama-cli`
   - `crates/tamaup-cli`
   - `crates/tama-config`
   - `crates/tama-project`
   - `crates/tama-build`
   - `crates/tama-manifest`
   - `crates/tama-audit`
   - `crates/tama-inspect`
   - `crates/tama-toolchain`
   - `crates/tama-common`
2. Set workspace metadata:
   - `edition = "2021"`
   - `version = "0.1.0"`
   - `rust-version = "1.81"`
   - `license = "MIT OR Apache-2.0"`
3. Create `rust-toolchain.toml` with `channel = "stable"`.
4. Create `.gitignore`, `rustfmt.toml`, `clippy.toml`.
5. Commit `Cargo.lock`.
6. Add dependencies:
   - `clap` with derive
   - `serde`, `serde_json`
   - `toml`, `toml_edit`
   - `tracing`, `tracing-subscriber`
   - `which`
   - `xshell`
   - `sha2`
   - `tiny-keccak`
   - `camino`
   - `thiserror`
   - `miette`
   - `tabled`
   - `insta`
   - `assert_cmd`
   - `tempfile`
   - `semver`
   - `regex`
   - `ureq` or equivalent HTTP client with TLS
   - `minisign-verify` or equivalent in-process minisign verification crate
   - `tar`, `flate2` or equivalent archive support
7. Each crate has an error enum and a smoke test.

#### Pass/fail

Pass only if:

```sh
cargo +1.81 build --workspace
cargo +1.81 test --workspace
cargo +1.81 clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build --workspace
cargo test --workspace
```

are green.

### Phase 1 — `tama-common`

#### Work

1. Implement shared result/error helpers.
2. Implement path helpers:
   - project root discovery by `tama.toml`
   - artifact path helpers
   - generated-file safety helpers
3. Implement selector/topic/hash helpers:
   - function/error selector
   - event topic
   - SHA-256 bytes/file
4. Implement logging initialization:
   - human mode
   - JSON mode for Tama-owned output
   - `RUST_LOG` override
   - no wrapper logs to stdout during passthrough commands
5. Implement diagnostic helpers for external tool failures.

#### Pass/fail

- Known Keccak vectors pass:
  - `transfer(address,uint256)` → `0xa9059cbb`
  - `Transfer(address,address,uint256)` → `0xddf252ad...3b3ef`
- Empty SHA-256 vector passes.
- Generated-file overwrite policy is unit-tested.
- Logging does not write stdout in passthrough mode.

### Phase 2 — `tama-config`

#### Work

1. Parse `tama.toml`:

```toml
[project]
name = "my-protocol"
verity = "0.5.0"

[paths]
src = "verity/src"
spec = "verity/spec"
proof = "verity/proof"
mirror_test = "test/verity"
out = "artifacts"
generated_solidity = "src/generated/verity"

[yul]
solc = "0.8.33"
optimizer = true
optimizer_runs = 200
yul_optimizer = true
evm_version = "cancun"
metadata_bytecode_hash = "none"

[trust.allow_axioms]
"Classical.choice" = "Lean standard classical reasoning accepted for this project"
"propext" = "Lean standard propositional extensionality accepted for this project"
"Quot.sound" = "Lean quotient soundness accepted for this project"
```

2. Parse `tama.lock` version 1:
   - resolved Verity repo/rev/tag
   - resolved Lake manifest hash
   - resolved Foundry dependency hash if applicable
   - resolved solc version/path/hash if managed by Tama
   - tool versions used for last build
   - input hashes for `tama.toml`, `lakefile.toml`, `lake-manifest.json`, `foundry.toml`, `lean-toolchain`, aggregate modules
   - compatibility patch/fork metadata if applicable
3. Parse the subset of `foundry.toml` Tama needs:
   - Foundry test dir
   - Solidity source dir
   - output dir
4. Parse `lean-toolchain`.
5. Implement lock staleness checks.
6. Implement lock write preserving comments where possible.
7. Implement exact `--locked` behavior.

#### Pass/fail

- Golden `tama.toml` and `tama.lock` parse and round-trip.
- Stale lock is detected when any tracked input changes.
- `--locked` returns a typed error and does not rewrite the lock.
- Non-locked mode emits warning and can rewrite lock only when command semantics call for it.

### Phase 3 — `tama-manifest`

#### Work

1. Define v1 manifest structs matching the schema section above.
2. Reject schema mismatch.
3. Validate selectors, event topics, and error selectors.
4. Validate required paths are project-relative and do not escape repo root.
5. Validate obligations:
   - non-empty `id`
   - fully qualified `lean_decl`
   - valid `kind`
   - valid coverage disposition
   - `invariant` and `postcondition` require `mirror` or `proof_only`
6. Validate ABI compatibility for bridge generation.
7. Provide pretty JSON write with deterministic key order.
8. Add JSON schema file under `schemas/tama.contract-manifest.v1.schema.json`.

#### Pass/fail

- ERC20Lite starter manifest round-trips.
- Counter fixture manifest round-trips.
- Corrupt selector fails.
- Corrupt event topic fails.
- Missing obligation coverage fails for public obligations.
- Path traversal fails.

### Phase 4 — `tama-toolchain`

#### Work

1. Detect tools:
   - `tama`
   - `lean`
   - `lake`
   - `forge`
   - `solc`
   - `git`
   - `tar`
   - `shasum` / `sha256sum` where shell installer needs them
2. Parse versions robustly.
3. Provide thin process wrappers.
4. Provide passthrough runner for Forge that preserves stdout/stderr/exit code.
5. Implement solc resolution:
   - `TAMA_SOLC`
   - project-local managed solc
   - user home managed solc
   - PATH solc matching `[yul].solc`
6. Implement managed solc install support if selected in Phase -1 / compatibility docs.
7. Implement `doctor` report model.
8. Add no-stub integration tests for version parsing on CI.

#### Pass/fail

- Stub tests cover parsing failures.
- Real CI detects actual installed tools.
- `tama doctor` clearly distinguishes missing, incompatible, stale, and OK.
- Missing solc gives exact remediation.
- `tama build` never silently uses wrong solc version.

### Phase 5a — `tama-project` templates and init/new

This moves earlier than CLI wiring so CLI commands call real project APIs.

#### Work

1. Implement `tama_project::init(path, opts)`.
2. Implement `tama_project::scaffold_contract(root, name)`.
3. Templates:
   - `tama.toml`
   - `tama.lock` or initial lock-generation input
   - `foundry.toml`
   - `lakefile.toml`
   - `lake-manifest.json` generated by `lake update`, not handwritten unless Phase -1 proves a stable fixture
   - `lean-toolchain`
   - `TamaSrc.lean`
   - `TamaSpec.lean`
   - `TamaProof.lean`
   - `ERC20Lite.lean`
   - `ERC20LiteSpec.lean`
   - `ERC20LiteProof.lean`
   - `ERC20Lite.t.sol`
   - `ERC20Lite.s.sol`
4. Starter ERC20Lite has a real complete proof accepted by pinned Verity.
5. Starter Foundry tests and deploy script use generated bytecode through the generated deployer.
6. `tama init` does not create Foundry default sample contracts.
7. `tama init` installs `forge-std` or gives exact offline instructions.
8. `tama new Foo` updates aggregate modules safely.
9. `tama new Foo` refuses invalid Lean/Solidity identifiers.

#### Pass/fail

- Fresh init tree exactly matches expected layout.
- No `sorry` appears in starter proof files.
- No Foundry default `Counter.sol` or default `Counter.t.sol` exists.
- Generated aggregate modules import the starter files.
- `tama new TipJar` creates files and updates aggregate modules.
- Running `lake build TamaSrc TamaSpec` on the initialized project succeeds in real CI.

### Phase 5b — `tama-build` Lake and Verity integration

#### Work

1. Implement `Lake::check_src_and_spec`:

```sh
lake build TamaSrc TamaSpec
```

2. Implement `Lake::build_proofs`:

```sh
lake build TamaProof
```

3. Implement Verity codegen using the Phase -1 chosen path.

Path A command shape, if upstream supports it:

```sh
lake exe verity-compiler --out artifacts/yul --manifest-out artifacts/manifest
```

Path B adapter shape, if upstream lacks direct manifest:

```sh
lake exe verity-compiler --out artifacts/yul
# then Tama adapter generates artifacts/manifest/*.json from real Verity artifacts and Lean probes
```

4. Discover manifests by reading `artifacts/manifest/*.json`, not by guessing contracts from filenames.
5. Fail if a manifest references missing Yul.
6. Fail if emitted manifest schema is not v1.
7. Implement `--contract Name` as downstream filtering unless Phase -1 proves Verity supports per-contract codegen.

#### Pass/fail

- Unit tests may stub Lake/Verity.
- Real CI must run actual Lake/Verity against starter ERC20Lite.
- Real CI must run actual Lake/Verity against the Counter compatibility fixture.
- Missing manifest fails.
- Schema mismatch fails.
- Missing Yul fails.

### Phase 5c — `tama-build` solc standard JSON

#### Work

1. Construct standard JSON input with `language = "Yul"`.
2. Include settings from `[yul]` only.
3. Write input to `artifacts/solc-json/<Contract>.input.json`.
4. Run:

```sh
solc --standard-json
```

5. Write output to `artifacts/solc-json/<Contract>.output.json`.
6. Parse JSON output.
7. Fail on `errors[].severity == "error"`.
8. Preserve warnings in diagnostics.
9. Extract creation bytecode and runtime bytecode.
10. Write:

   - `artifacts/bytecode/<Contract>.bin`
   - `artifacts/bytecode/<Contract>.runtime.bin`

11. Compute creation bytecode SHA-256.
12. Update manifest artifact paths and hash.

#### Pass/fail

- Canned solc success output produces bytecode files.
- Canned solc JSON error output fails even if process exit code is zero.
- Real CI compiles starter ERC20Lite Yul.
- Real CI compiles Counter fixture Yul.
- Manifest bytecode hash matches file contents.

### Phase 5d — Solidity bridge generation

#### Work

1. Generate `<Contract>Iface.sol`:
   - functions
   - events
   - errors if supported
   - ABI-compatible types
2. Generate `<Contract>Deployer.sol`:
   - imports interface
   - embeds creation bytecode
   - supports constructor args if manifest has constructor
   - deploys with `create`
   - reverts clearly on failed deployment
3. Optionally generate `<Contract>Abi.json` for tooling convenience.
4. Enforce generated-file header safety.
5. Generated Solidity compiles under `forge build`.
6. Mirror tests import generated deployer/interface.

#### Pass/fail

- Snapshot generated ERC20Lite starter bridge.
- Snapshot generated Counter fixture bridge.
- Hand-edited generated bridge is not overwritten.
- `forge build` compiles generated bridge in real CI.

### Phase 5e — Build pipeline

#### Work

Implement `Pipeline::run(ctx, opts)`:

1. Load config and lock.
2. If `--locked`, enforce no drift before running.
3. `lake build TamaProof`.
4. Verity codegen / manifest adapter.
5. Manifest validation.
6. solc per contract.
7. Bridge generation.
8. `forge build` unless `--no-forge`.
9. Recompute and write `tama.lock` unless `--locked`.

Options:

```text
--locked
--offline
--no-solc
--no-forge
--contract <Name>
--json
--verbose / -v / -vv
```

`--no-solc` and `--no-forge` are developer escape hatches, not release CI paths.

#### Pass/fail

- Full starter ERC20Lite build produces all artifacts.
- Full Counter fixture build produces all artifacts.
- `--locked` catches stale lock.
- `--no-solc` stops before bytecode and marks downstream artifacts stale.
- `--contract ERC20Lite` filters downstream processing.
- No release gate uses `--no-solc` or `--no-forge`.

### Phase 6a — `tama-audit` structure and selectors

#### Work

1. Define canonical audit model:

```rust
Issue {
  check: String,
  contract: Option<String>,
  severity: Error | Warning | Info,
  code: String,
  message: String,
  path: Option<Utf8PathBuf>,
}
```

2. Define canonical JSON output for audits.
3. Structure check:
   - required files exist
   - aggregate modules import required files
   - generated bridge files exist and have generated header
   - manifests reference existing artifacts
4. Selector/topic check:
   - manifest function selectors recompute correctly
   - manifest error selectors recompute correctly
   - manifest event topic0 recomputes correctly
   - generated interface signatures match manifest ABI
5. If upstream scripts exist, run Python reference against fixtures and compare canonical issue sets.

#### Pass/fail

- Missing proof file fails.
- Missing mirror test fails.
- Missing generated deployer fails after build.
- Corrupt selector fails.
- Corrupt event topic fails.
- Generated interface drift fails.

### Phase 6b — `tama-audit` storage and coverage

#### Work

1. Storage audit reads the manifest storage model. It does not pretend Yul literal scanning is semantic storage verification.
2. Storage audit checks:
   - duplicate storage entries within a contract
   - overlapping fixed slots/offsets within a contract
   - invalid slot hex
   - invalid offset/width
   - unsupported encoding values
   - declared storage paths are compatible with generated manifest format
3. Optional smoke check: if adapter/compiler emits Yul storage annotations, verify they agree with manifest. Without annotations, do not scan arbitrary Yul for slot literals and call it proof.
4. Cross-contract storage collision is checked only when contracts are annotated as sharing storage, proxy implementation, or delegatecall storage participants.
5. Coverage audit checks public obligations:
   - `invariant` and `postcondition` require `mirror` or `proof_only` disposition
   - mirror path must exist
   - mirror test selector/name must be findable in test file by conservative regex
   - proof-only must include non-empty reason
   - helper obligations excluded from required coverage but reported

#### Pass/fail

- Duplicate storage slot in one contract fails.
- Mapping storage entry is accepted.
- Dynamic/unsupported storage encoding fails with clear error unless declared unsupported by schema.
- Missing mirror for postcondition fails.
- Proof-only without reason fails.
- Helper without mirror does not fail.

### Phase 6c — `tama-audit` trust boundary

#### Work

1. Generate a Lean probe under `artifacts/trust-probe/` that imports all proof modules needed for selected obligations.
2. The probe uses Lean’s axiom collection API to emit JSON:

```json
{
  "obligations": [
    {
      "lean_decl": "proof.ERC20LiteProof.transfer_post",
      "axioms": ["Classical.choice"]
    }
  ]
}
```

3. If Lean API constraints force fallback to `#print axioms`, fallback must be behind a clearly named implementation path and tested separately.
4. Compare axiom set to `[trust.allow_axioms]`.
5. Hard-deny `sorryAx` even if allowlisted.
6. Fail if any public obligation decl cannot be resolved.
7. Report all violations, not just first.
8. Store probe source and JSON output under artifacts for debugging.

#### Pass/fail

- Complete ERC20Lite starter proof passes.
- Injected `sorry` fails because of `sorryAx`.
- Injected custom `axiom` fails unless allowlisted.
- Allowlisted axiom passes and prints allowlist reason.
- Missing Lean declaration fails.
- Real CI runs this with actual Lean, not a stub.

### Phase 6d — Audit command orchestration

#### Work

1. `tama audit` runs:
   - `structure`
   - `selectors`
   - `storage-layout`
   - `coverage`
   - `trust-boundary`
2. `tama audit <check>` runs one check.
3. Human output is concise and actionable.
4. `--json` emits canonical audit JSON.
5. Exit non-zero if any error issue exists.
6. Warnings do not fail unless `--deny-warnings` is passed.

#### Pass/fail

- All checks pass on fresh ERC20Lite starter.
- All checks pass on the Counter compatibility fixture.
- Each negative fixture fails exactly the intended check.
- JSON output is stable and snapshotted.

### Phase 7 — `tama-inspect`

#### Work

Supported fields:

```text
manifest
selectors
abi
storage-layout
yul
bytecode
runtime-bytecode
theorems
obligations
mirrors
trust
```

1. Inspect reads manifests and artifacts.
2. Human output uses tables where useful.
3. `--json` uses structured JSON.
4. Missing artifacts produce typed errors with build/audit hints.

#### Pass/fail

- All fields work for ERC20Lite.
- All fields work for the Counter compatibility fixture.
- JSON snapshots are stable.
- Missing bytecode gives “run tama build” diagnostic.

### Phase 8a — `tama-cli` global surface and project commands

#### Work

1. Implement global flags:

```text
--root <path>
--locked
--offline
--json
--verbose / -v / -vv
--no-color
```

2. Implement commands:
   - `init [path]`
   - `new <Name>`
   - `check`
   - `clean [--deep]`
3. `--root` changes project root resolution for project commands.
4. `--no-color` sets `NO_COLOR=1` for Tama diagnostics.
5. `check` calls only `lake build TamaSrc TamaSpec`.
6. `clean` removes configured artifact dirs.

#### Pass/fail

- `tama init` fresh project passes tree assertions.
- `tama check` does not import proof modules; a deliberate broken proof does not fail check.
- `tama check` does fail broken implementation/spec.
- `tama clean` removes artifacts but not source.
- `tama clean --deep` removes configured Lake build dir and dependency build artifacts according to docs.

### Phase 8b — `tama-cli` build/test/inspect/audit

#### Work

1. `tama build` calls pipeline.
2. `tama test [forge-args...]` execs `forge test` with args unchanged except for Tama global `--offline`.
3. Clap captures Forge args safely, including hyphenated args.
4. `tama inspect <Contract> <field>` calls inspect.
5. `tama audit [check]` calls audit orchestration.

#### Pass/fail

- `tama test -- --match-test testIncrement -vvv` passes exact args to Forge.
- Direct `tama test --match-test testIncrement -vvv` works or errors with a clear instruction to use `--`; choose one behavior and document it.
- Forge exit code is forwarded.
- Tama logs do not pollute Forge stdout.
- `--json` works for inspect/audit/build status but not Forge passthrough.

### Phase 8c — `tama-cli` install/remove/update/doctor

#### Work

1. `tama install <repo>[@<version>]`:
   - clones into temp dir
   - validates `tama.toml`
   - reads dependency Lake package info
   - adds/updates `[[require]]` in user-owned `lakefile.toml` via `toml_edit`
   - runs `lake update`
   - updates `tama.lock`
2. `tama remove <package>`:
   - removes matching `[[require]]`
   - runs `lake update`
   - updates lock
3. `tama update`:
   - syncs Verity dependency with `tama.toml`
   - runs `lake update`
   - runs `forge update` unless `--no-forge`
   - updates lock
4. `tama doctor`:
   - prints tool versions
   - validates lock drift
   - validates Verity compatibility from `docs/VERITY_COMPAT.md` / embedded compatibility table
   - validates solc version/path
   - validates generated dirs
5. `doctor --fix` performs only safe repairs:
   - create missing generated artifact dirs, generated Solidity dir, and configured Lake build dir
   - recompute lock when inputs are otherwise valid
   - repair generated aggregate module imports if generated header/marker is intact
   - sync explicit Verity require block if it is Tama-managed

#### Pass/fail

- Local fake git Tama dependency can be installed/removed.
- Hand-edited unrelated Lakefile content survives install/remove/update.
- `doctor` reports exact missing/incompatible tool.
- `doctor --fix` does not overwrite hand-edited Lakefile sections.

### Phase 9 — Counter compatibility fixture

This is a release blocker and should land before final CLI e2e.

#### Work

ERC20Lite is the default starter template and is already exercised by every fresh-project gate. Add `fixtures/projects/counter/` as a compact compatibility fixture with:

- storage:
  - `count : uint256`
- functions:
  - `increment()`
  - `decrement()` if supported by the pinned Verity feature subset
  - `getCount() view returns (uint256)`
- obligations:
  - increment postcondition
  - read-only getter preservation
  - proof-only classification for any non-executable symbolic fact
- Foundry mirrors:
  - fuzzed deployment/start-state
  - fuzzed increment/decrement behavior
  - fuzzed getter mirrors generated bytecode state
  - invariant count tracks model
- generated bridge:
  - interface
  - deployer
- audit:
  - structure
  - selectors
  - storage
  - coverage
  - trust boundary

#### Pass/fail

- `tama check` passes for the Counter fixture.
- `tama build` passes with real Verity/solc/Forge for the Counter fixture.
- `tama test` passes with real Forge for the Counter fixture.
- `tama audit` passes for the Counter fixture.
- `tama inspect Counter selectors/abi/storage-layout/yul/bytecode/obligations/mirrors/trust` passes.
- At least one negative mutation per audit check fails across the starter and fixture suites.

If Verity cannot support one of these features, the phase must not fake it. The plan must either reduce the declared v0.1 supported feature set and update docs, or block release pending Verity work.

### Phase 10a — `tamaup-cli`

#### Work

1. Commands:
   - no-arg install latest stable
   - `install [version|nightly]`
   - `use <version>`
   - `list`
   - `self update`
   - `uninstall`
2. State:
   - `${TAMAUP_HOME:-$HOME/.tama}`
   - `versions/<version>/bin/tama`
   - `versions/<version>/bin/tamaup`
   - `bin/tama` symlink to active version
   - `active` file
3. Fetch release manifest over HTTPS with typed HTTP client.
4. Verify manifest signature in process using embedded public key.
5. Locate platform artifact.
6. Download tarball.
7. Verify SHA-256 before extraction.
8. Extract safely:
   - no absolute paths
   - no `..` path traversal
   - expected binary names only
9. Atomically update symlink.
10. Toolchain bootstrap:

   - detect Lean/Lake/Forge/solc
   - install only with explicit user consent or `--yes`
   - respect `--no-install-lean`, `--no-install-foundry`, `--no-install-solc`, `--no-modify-path`, `--offline`

11. No Windows logic.

#### Pass/fail

- Fake signed manifest installs local fake artifact.
- Bad signature fails before artifact download/extraction.
- Bad SHA fails before extraction.
- Tar path traversal fails.
- `tamaup use` symlink update is atomic.
- No external `minisign` binary is required by `tamaup`.

### Phase 10b — `installer/install.sh`

#### Work

1. POSIX shell; ShellCheck clean.
2. Supports Linux/macOS x86_64/aarch64 only.
3. Downloads:
   - `manifest.json`
   - `manifest.json.minisig`
   - selected tarball
4. Requires external signature verifier unless a safe bootstrap alternative is implemented. If absent, hard fail with install instructions.
5. Verifies manifest signature against embedded public key.
6. Verifies tarball SHA-256.
7. Extracts into `${TAMAUP_HOME:-$HOME/.tama}`.
8. Honors:
   - `--yes`
   - `--no-modify-path`
   - `--no-install-lean`
   - `--no-install-foundry`
   - `--no-install-solc`
   - `--offline`
   - `--manifest-file <path>`
   - `--version <version>`
9. Does not support Windows.

#### Pass/fail

- ShellCheck passes.
- Local HTTP/file-server test passes.
- Bad signature fails.
- Bad SHA fails.
- Unsupported platform fails cleanly.
- No extraction happens before verification.

### Phase 11 — CI and release automation

#### Work

1. CI jobs:
   - Rust MSRV `1.81`
   - Rust stable Linux x86_64
   - Rust stable macOS arm64
   - real e2e Linux
   - real e2e macOS
   - installer tests
2. Rust jobs run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

3. Real e2e job installs real:
   - Lean/Lake matching pinned `lean-toolchain`
   - Verity dependency matching compatibility report
   - Forge
   - solc matching `[yul].solc`
4. Real e2e job runs:

```sh
tama init /tmp/tama-counter
cd /tmp/tama-counter
tama doctor
tama check
tama build --locked
tama test
tama audit
tama inspect ERC20Lite selectors
tama inspect ERC20Lite abi
tama inspect ERC20Lite storage-layout
tama inspect ERC20Lite obligations
```

5. Real Counter fixture e2e job runs the same check/build/test/audit/inspect sequence.
6. Negative audit job mutates fixtures and verifies failure.
7. Release workflow on `v*` tags:
   - builds four platform artifacts
   - computes SHA-256
   - writes cumulative `manifest.json`
   - signs manifest
   - publishes the website, `install.sh`, signed manifest, and release metadata to GitHub Pages / `tama.tools`
   - attaches artifacts and signed manifest to GitHub Release
8. GitHub Pages site:
   - lives under `site/`
   - is deployable as static files with no server-side code
   - documents the install command:

```sh
curl -L https://tama.tools/install.sh | sh
```

   - links to quickstart, command reference, limitations, audit guide, release artifacts, and signed manifest
   - does not advertise Windows support
   - is included in release CI link checks
9. `docs/RELEASE.md` documents one-time setup:
   - GitHub Pages branch
   - custom domain
   - signing secret
   - public key rotation procedure

#### Pass/fail

- No release e2e step is gated on `which`.
- Missing tool setup fails CI.
- Stubs are allowed only in unit tests.
- Release workflow produces artifacts for all four supported platforms.
- Manifest signature verifies locally after release workflow.
- `site/` builds as a static GitHub Pages website for `tama.tools`, and its install command matches `docs/SPEC.md` section 12.

### Phase 12 — Final product hardening

#### Work

1. Add integration tests for user mistakes:
   - run outside project
   - stale lock
   - missing solc
   - wrong solc
   - corrupted manifest
   - hand-edited generated file
   - missing mirror test
   - unallowlisted axiom
   - `sorry`
   - invalid contract name
   - invalid `tama.toml`
2. Add docs:
   - quickstart
   - command reference
   - generated artifacts guide
   - audit guide
   - troubleshooting
   - limitations
   - release guide
3. Add human-readable diagnostics for every hard failure.
4. Add `--json` output snapshots for machine consumers.
5. Add basic telemetry-free privacy statement: Tama does not phone home except installer/update network requests explicitly invoked by user.

#### Pass/fail

- Each failure mode has a test and readable diagnostic.
- Docs commands are copy-paste tested in CI where feasible.
- `cargo deny` or equivalent dependency/license/security check is green if added.
- Final holistic review finds no mocked release path.

## Updated end-state expectations

The following expectations define “done.” Every item is pass/fail.

### Build artifacts

- `cargo build --workspace --release` produces `target/release/tama` and `target/release/tamaup` on Linux and macOS.
- `cargo +1.81 build --workspace` passes.
- `cargo +1.81 test --workspace` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- `cargo fmt --check` passes.
- `Cargo.lock` is committed.

### Fresh project usability

On a clean supported machine after installing via `tamaup`:

```sh
tama init my-protocol
cd my-protocol
tama doctor
tama check
tama build
tama test
tama audit
tama inspect ERC20Lite selectors
```

must pass without hand-editing files.

Fresh project must contain:

- no Foundry default template contracts
- no `sorry` in starter proof path
- no unallowlisted axioms in public starter obligations
- generated bridge files
- generated manifests
- generated bytecode
- working Foundry mirror test
- valid `tama.lock`

### Practical Verity development

A user can then run:

```sh
tama new TipJar
tama check
```

and receive a valid scaffold with:

- implementation file
- spec file
- proof file
- mirror test file
- aggregate module imports updated
- no Lakefile regeneration needed

The generated `TipJar` skeleton may require the user to fill in proofs before audit passes, but it must not break the existing ERC20Lite starter build/check/test/audit.

### Real build pipeline

`tama build` on starter ERC20Lite and the Counter compatibility fixture must run real:

1. `lake build TamaProof`
2. Verity compiler or real compatibility adapter
3. manifest validation
4. `solc --standard-json`
5. bridge generation
6. `forge build`
7. lock update

No step may be satisfied by canned fixture output in release CI.

### Test passthrough

`tama test` must behave like syntactic sugar for `forge test`:

- forwards args unchanged except for Tama global `--offline`
- forwards exit code
- preserves Forge stdout/stderr
- does not force mirror paths
- does not apply hidden filters

### Audit correctness

`tama audit` must pass on starter ERC20Lite and the Counter compatibility fixture.

It must fail on these injected mutations:

- deleted required source/spec/proof/test/generated file
- corrupted function selector
- corrupted event topic
- duplicate fixed storage slot
- missing mirror coverage for postcondition
- proof-only disposition with empty reason
- unresolved `lean_decl`
- inserted `sorry`
- inserted custom axiom not in allowlist
- hand-edited generated bridge file that Tama tries to overwrite

### Inspect usability

For ERC20Lite and the Counter compatibility fixture, every field must work in human and JSON mode:

```text
manifest
selectors
abi
storage-layout
yul
bytecode
runtime-bytecode
theorems
obligations
mirrors
trust
```

### Locking and reproducibility

- `tama build --locked` fails on stale `tama.toml`, `lakefile.toml`, `lake-manifest.json`, `foundry.toml`, `lean-toolchain`, or aggregate modules.
- `tama update` refreshes lock deterministically.
- `tama doctor` explains drift and safe fixes.
- CI uses `--locked` after init/update has produced the lock.

### Installer and updater

- `tamaup install 0.1.0` verifies signed manifest and tarball hash before extraction.
- `tamaup use 0.1.0` switches active version atomically.
- `tamaup list` shows installed versions and active version.
- `https://tama.tools/` is a GitHub Pages site with installation instructions, command documentation, release links, and the signed release manifest/install script entry points.
- Bad signature fails.
- Bad SHA fails.
- Tar path traversal fails.
- `installer/install.sh` is ShellCheck clean.
- No Windows path is advertised for v0.1.

### Documentation

Docs must include:

- `docs/QUICKSTART.md`
- `docs/COMMANDS.md`
- `docs/GENERATED_ARTIFACTS.md`
- `docs/AUDIT.md`
- `docs/LIMITATIONS.md`
- `docs/VERITY_COMPAT.md`
- `docs/RELEASE.md`
- `site/` static GitHub Pages website for `tama.tools`

Every command in quickstart must be tested in CI or explicitly marked as documentation-only setup.

### Human-facing quality

- Error messages identify the file, command, or declaration that failed.
- No panic backtraces for normal user mistakes.
- JSON mode is stable and suitable for CI bots.
- Human output is readable without `RUST_LOG`.
- `tama doctor` is the first recommended diagnostic command and gives actionable next steps.

## Orchestration rules for coding agents

1. Each phase gets its own prompt and commit.
2. A phase is not complete until its pass/fail criteria are green.
3. Stubs are allowed only where the phase explicitly says unit tests may stub external tools.
4. Any “Known deferred” item creates a new phase before moving on, unless explicitly accepted by the user.
5. Mid-stream review checkpoints happen after:
   - Phase -1
   - Phase 5e
   - Phase 8c
   - Phase 10b
   - Phase 11
6. Reviews are read-only and focus on:
   - stale spec drift
   - manifest/schema drift
   - accidental mocked release path
   - error UX
   - security regressions
   - generated-file overwrite safety
7. The final post-lift review runs the full end-state checklist. No tag is cut until every item is green.

## One-sentence release bar

Tama v0.1 ships only when a new user can install it, initialize a Verity project, build real Verity code to bytecode, run real Foundry mirror tests, audit proof/test/trust boundaries, inspect generated artifacts, and diagnose drift — all without touching the Verity monorepo or relying on mocked pipeline output.
