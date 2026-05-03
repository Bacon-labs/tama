# Tama 玉魂

> *Tama* — jade and soul. The developer toolchain for Verity smart contracts.

A Verity contract is written three times: as code that runs on the EVM, as a
specification of what the code must do, and as a machine-checked proof that the
two agree. **Tama** is the single CLI that builds, proves, compiles, audits,
and inspects Verity projects — and a Tama project is a Foundry project, so
`forge` keeps working as you expect.

**Read the docs at https://tama.tools.**

## Install

```sh
curl -L https://tama.tools/install.sh | sh
```

Then `tamaup install <version>` to switch to a specific signed release.

## A few of the commands

```sh
tama init my-protocol     # scaffold a project
tama check                # fast Lean check (impl + spec only)
tama build                # full pipeline: proofs → Yul → bytecode → bridge → forge
tama test                 # passthrough to forge test
tama audit                # release-time audits over manifests and artifacts
tama inspect ERC20Lite manifest
```

The full CLI reference lives at https://tama.tools/reference/cli.

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

The website source is in [`site/`](site/) and is deployed to GitHub Pages by
[`.github/workflows/deploy-docs.yml`](.github/workflows/deploy-docs.yml). The
internal product specification and contributor docs live in
[`docs/reference/`](docs/reference/).
