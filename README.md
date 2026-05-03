# Tama

Tama is a developer toolchain for Verity smart-contract projects. It gives Verity developers one CLI for project scaffolding, Lean/Lake checks, Verity-to-Yul codegen, `solc` bytecode generation, Foundry mirror tests, artifact inspection, and audit checks.

Tama projects are also Foundry projects, so users can run `forge` directly whenever that is useful.

## Install

```sh
curl -L https://tama.tools/install.sh | sh
```

Use `tamaup install <version>` to install a specific signed release.

## Quickstart

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

Add another Verity contract scaffold:

```sh
tama new TipJar
tama check
```

New contract scaffolds include TODO proofs, so `tama audit` will reject them until the public obligations are discharged.

## Commands

- `tama init`: create an ERC20Lite starter project.
- `tama new <Name>`: add Verity source, spec, proof, and Foundry mirror-test files.
- `tama check`: run the fast Lean check for implementation and spec modules.
- `tama build`: run proofs, Verity codegen, manifest adaptation, `solc`, bridge generation, and `forge build`.
- `tama test`: pass through to `forge test`.
- `tama audit`: check structure, selectors, storage layout, coverage, and trust boundaries.
- `tama inspect`: print generated manifests, selectors, ABI, bytecode, obligations, mirrors, and trust data.
- `tamaup`: install, switch, list, update, and uninstall Tama versions.

## Docs

- [Quickstart](docs/reference/QUICKSTART.md)
- [Command reference](docs/reference/COMMANDS.md)
- [Generated artifacts](docs/reference/GENERATED_ARTIFACTS.md)
- [Audit guide](docs/reference/AUDIT.md)
- [Limitations](docs/reference/LIMITATIONS.md)
- [Verity compatibility](docs/reference/VERITY_COMPAT.md)
- [Release and installer notes](docs/reference/RELEASE.md)
- [Privacy](docs/reference/PRIVACY.md)

The full product specification lives in [docs/reference/SPEC.md](docs/reference/SPEC.md).

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

The workspace MSRV is Rust 1.81:

```sh
cargo +1.81.0 build --workspace
cargo +1.81.0 test --workspace
```

