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

Without `RUST_LOG`, Tama defaults to info-level diagnostics. `-v` raises
Tama-owned diagnostics to debug, and `-vv` raises them to trace. External tool
output still follows the invoked tool's own behavior.

`tama.toml` is strict: unknown Tama-owned table names or keys are rejected
instead of ignored. This keeps misspelled compiler or path settings from
silently changing build behavior. Foundry-specific settings remain in
`foundry.toml` and are parsed by Foundry.

Tama-owned paths in `tama.toml` must be non-empty relative paths inside the
project. Absolute paths, `..` components, and `.` are rejected before writes.
The Foundry `src`, `test`, `out`, and `cache_path` paths that Tama consumes for
audit and clean behavior must follow the same project-relative rule.

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
                    `lake update` and refreshes cached packages after a
                    successful update. Before `tama check` or `tama build`, Tama
                    may also seed missing packages or replace clean stale package
                    checkouts when the cached checkout's Git HEAD exactly
                    matches `lake-manifest.json` and the checkout has a clean Git
                    worktree. Tama does not overwrite dirty or non-Git package
                    directories during this seed step. This is a performance
                    optimization only; locked builds must also work from an
                    empty cache.
```

## `tama init [path]`

Creates a new Tama/Foundry project with the ERC20Lite starter. It writes `tama.toml`, `tama.lock`, `foundry.toml`, `lakefile.toml`, `lake-manifest.json`, `lean-toolchain`, aggregate Lean modules, Verity source/spec/proof files, generated bridge paths, and Foundry mirror tests.

## `tama new <Name>`

Adds `verity/src/<Name>.lean`, `verity/spec/<Name>Spec.lean`, `verity/proof/<Name>Proof.lean`, and `test/verity/<Name>.t.sol`, then updates the aggregate modules. It does not rewrite the Lakefile.

When `[paths]` has been customized, `lakefile.toml` must already map the `src`, `spec`, and `proof` Lake libraries to those roots. Tama refuses before writing files if the configured paths are not covered by the Lakefile.

The scaffold includes concrete spec stubs, public proof obligations marked with
`sorry`, and fuzz-shaped Foundry mirrors that import the generated deployer and
interface. `tama check` still passes immediately because proof modules are not
part of the fast check; `tama audit trust-boundary` rejects the TODO proofs until
they are discharged.

## `tama check`

Runs the fast Lean check for implementation and spec aggregate modules only:

```sh
lake build TamaSrc TamaSpec
```

Proof modules, Verity codegen, solc, and Foundry are not run.

`tama check` does not run `lake update`. It seeds manifest-matching package checkouts already present in `TAMA_LAKE_PACKAGE_CACHE` before invoking Lake, including replacement of clean stale package checkouts, and after a successful check it refreshes that cache from clean Git checkouts under `.lake/packages`. With global `--offline`, it fails before invoking Lake if any git package pinned in `lake-manifest.json` is missing, dirty, or at another revision under `.lake/packages`.

With `--json`, Lake output is forwarded to stderr and stdout contains only:

```json
{
  "status": "ok",
  "targets": ["TamaSrc", "TamaSpec"]
}
```

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

`tama build` does not run `lake update`. It seeds manifest-matching cached Lake package checkouts before invoking Lake, including replacement of clean stale package checkouts, and after a successful build it refreshes `TAMA_LAKE_PACKAGE_CACHE` from clean Git checkouts under `.lake/packages`. With global `--offline`, it fails before invoking Lake if any pinned git package is missing, dirty, or at another revision under `.lake/packages`, and passes `--offline` to `forge build`.

With `--json`, external tool output is forwarded to stderr and stdout contains only the generated manifest paths:

```json
{
  "manifests": [
    "artifacts/manifest/ERC20Lite.json"
  ]
}
```

## `tama test [forge-args...]`

Executes `forge test` with arguments unchanged. Forge stdout, stderr, and exit code are preserved. Tama-owned JSON output does not wrap or corrupt Foundry passthrough output.

The Tama global `--offline` flag is translated to Forge's `--offline` flag for this command. Other Forge arguments, filters, and verbosity flags are preserved as provided.

## `tama audit [check]`

Runs audits over the artifacts produced by `tama build`. Human output lists the project root, manifest directory, loaded contracts, each check that ran, and any findings. `--json` emits the canonical audit issue format for CI consumers.

Without `[check]`, Tama runs the full suite:

```text
structure        Required files, aggregate imports, generated bridge headers, artifact paths, and bytecode hashes
selectors        ABI selectors/topics, generated Solidity declarations, and Yul dispatch cases
storage-layout   Storage declarations, slot overlap, encodings, and compiler layout drift
coverage         Public obligations have property-shaped Foundry mirrors or proof-only reasons
trust-boundary   Lean axioms, sorryAx, unresolved declarations, and Verity trust/assumption reports
```

Use `tama audit <check>` to run one check. `storage` is accepted as an alias for `storage-layout`, and `trust` is accepted as an alias for `trust-boundary`. `--deny-warnings` treats warning-severity findings as failures.

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

`selectors` includes function selectors, custom-error selectors, and event topic0 values.

## `tama clean [--deep]`

Removes generated Tama artifacts, generated Solidity, the configured Lake build directory, and Foundry's configured `out` and `cache_path` directories. `--deep` also removes Lake dependency/cache state.

## `tama install`, `tama remove`, `tama update`, `tama doctor`

These commands manage Verity/Tama dependencies, refresh lock state, and diagnose toolchain drift. Lakefile edits must preserve unrelated user content.

`tama install` and `tama remove` refuse `--offline` because they must run `lake update` after editing dependencies. `tama install` only manages Tama packages with `tama.toml`; add pure Lake packages manually to `lakefile.toml`. Direct git packages resolved by Lake are recorded in `tama.lock` under `resolved.lake.<package>.*`. `tama update --offline` is allowed only with both `--no-lake` and `--no-forge`, which limits it to local lock/config refreshes. If the Verity dependency would need to change, `tama update --no-lake` refuses before editing `lakefile.toml` because `lake-manifest.json` must be refreshed by Lake at the same time.

`tama update --package <name>` runs `lake update <name>` for one Lake package and refreshes `tama.lock`; it does not run `forge update`. Use Forge directly for Solidity-side dependency updates.

`tama doctor` exits nonzero when required tools are missing or incompatible, Verity resolution disagrees with `tama.toml`, generated artifact directories are missing, or project lock inputs are stale. `tama doctor --fix` requires a Tama project and applies safe directory, Verity dependency, and lock repairs first, then reports the post-fix status.

With `--json`, each tool entry has an explicit `status` tag and `details` object:

```json
{
  "tools": [
    {
      "status": "ok",
      "details": {
        "name": "tama",
        "path": "/usr/local/bin/tama",
        "version": "0.1.0"
      }
    }
  ],
  "lock_current": true,
  "notes": []
}
```
