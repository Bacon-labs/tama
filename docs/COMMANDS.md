# Commands

Global flags:

```text
--root <path>       Run as if invoked from another project root
--locked            Fail if the lockfile or tracked inputs are stale
--offline           Do not access the network
--json              Emit JSON for Tama-owned output
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

                    Tama copies missing packages into `.lake/packages` before
                    `lake update` and records newly fetched packages after a
                    successful update. This is a performance optimization only;
                    locked builds must also work from an empty cache.
```

## `tama init [path]`

Creates a new Tama/Foundry project with the ERC20Lite starter. It writes `tama.toml`, `tama.lock`, `foundry.toml`, `lakefile.toml`, `lake-manifest.json`, `lean-toolchain`, aggregate Lean modules, Verity source/spec/proof files, generated bridge paths, and Foundry mirror tests.

## `tama new <Name>`

Adds `verity/src/<Name>.lean`, `verity/spec/<Name>Spec.lean`, `verity/proof/<Name>Proof.lean`, and `test/verity/<Name>.t.sol`, then updates the aggregate modules. It does not rewrite the Lakefile.

## `tama check`

Runs the fast Lean check for implementation and spec aggregate modules only:

```sh
lake build TamaSrc TamaSpec
```

Proof modules, Verity codegen, solc, and Foundry are not run.

## `tama build`

Runs proof elaboration, real Verity codegen, Tama manifest adaptation, `solc --standard-json`, Solidity bridge generation, `forge build`, and lockfile update.

Useful flags:

```text
--locked
--offline
--no-solc
--no-forge
--contract <Name>
```

`--no-solc` and `--no-forge` are local development escape hatches and are not release gates. `--no-solc` removes stale downstream solc JSON, bytecode, and generated bridge files for the selected contracts while leaving fresh Yul and manifests.

## `tama test [forge-args...]`

Executes `forge test` with arguments unchanged. Forge stdout, stderr, and exit code are preserved. Tama-owned JSON output does not wrap or corrupt Foundry passthrough output.

## `tama audit [check]`

Runs structure, selector/topic, storage-layout, coverage, and trust-boundary audits. `--json` emits the canonical audit issue format.

## `tama inspect <Contract> <field>`

Reads manifests and artifacts. Fields:

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

## `tama clean [--deep]`

Removes generated Tama artifacts and generated Solidity. `--deep` also removes the configured Lake build directory.

## `tama install`, `tama remove`, `tama update`, `tama doctor`

These commands manage Verity/Tama dependencies, refresh lock state, and diagnose toolchain drift. Lakefile edits must preserve unrelated user content.

`tama install` and `tama remove` refuse `--offline` because they must run `lake update` after editing dependencies. `tama update --offline` is allowed only with both `--no-lake` and `--no-forge`, which limits it to local lock/config refreshes.

`tama doctor` exits nonzero when required tools are missing or incompatible, or when project lock inputs are stale. `tama doctor --fix` applies safe directory and lock repairs first, then reports the post-fix status.
