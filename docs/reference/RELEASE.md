# Release

Release candidates must pass all non-negotiable gates in `docs/reference/IMPLEMENTATION_PLAN.md`.

## Required Checks

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

Release CI also runs real end-to-end jobs with Lean/Lake, pinned Verity, solc, and Foundry. The ERC20Lite starter and Counter compatibility fixture must both pass check, build, test, audit, and inspect.

The CI workflow lives in `.github/workflows/ci.yml` and includes:

- Rust MSRV `1.81.0`, stable Linux, and stable macOS arm64 jobs.
- Real Linux and macOS e2e jobs that install Lean/Lake, Foundry, and solc `0.8.33`.
- Negative audit mutations for structure, selector, storage, coverage, generated-file, and trust-boundary failures.
- Shell installer checks, ShellCheck, and `tamaup` archive-safety tests.

## Artifacts

Build four platform archives:

- `linux-x86_64`
- `linux-aarch64`
- `macos-x86_64`
- `macos-aarch64`

Each archive contains `tama` and `tamaup`. No Windows archive is published for v0.1.

## Manifest

The release workflow writes a cumulative `manifest.json` with `schema = "tama.release-manifest.v1"`, a `stable` version, an optional `nightly` version preserved from the previous published manifest, and a `releases[]` list. It computes SHA-256 for every new archive, preserves older release entries only after rejecting unknown fields, empty release artifact lists, duplicate release versions, duplicate platform artifacts, dangling channel references, or mixed manifest shapes, and publishes the manifest plus archives to the GitHub Release.

The schema-less legacy `version` plus `artifacts[]` manifest shape is accepted only by `tamaup --manifest-file` for local/offline tests. Installers reject cumulative manifests that omit `releases[]`, include release entries without artifacts, or mix the legacy and cumulative shapes.

Published artifact URLs must use `https://` with a host. Absolute-path `file://` artifact URLs are accepted only for local/offline manifests passed via `tamaup --manifest-file`.

Both `installer/install.sh` and `tamaup` use the Foundry installer trust model: TLS to GitHub for the manifest, then SHA-256 verification of every downloaded archive against the SHA-256 carried in the manifest. There is no separate manifest signature; trust is anchored at GitHub Releases. No external signing key, public key, or signature-verifier binary is required.

The release workflow lives in `.github/workflows/release.yml` and requires no repository secrets beyond the default `GITHUB_TOKEN`.

## Website

The release workflow publishes the static GitHub Pages site at `https://tama.tools/`. The site lives under `docs/` and includes:

- installation instructions matching `docs/reference/SPEC.md` section 12:

```sh
curl -L https://tama.tools/install.sh | sh
```

- links to quickstart, command reference, audit guide, generated-artifact rules, troubleshooting, Verity compatibility, limitations, release artifacts, `install.sh`, and the GitHub Releases-hosted `manifest.json`;
- links to the telemetry-free privacy statement;
- no Windows installation path for v0.1.

## Installer Safety

Installation must reject unknown or unsafe manifest fields, bad hashes, absolute archive paths, `..` traversal, duplicate binary entries, and unexpected archive file names before any binary is installed.

`tamaup uninstall` removes the active `tama` binary and active marker, but keeps `tamaup` available so users can reinstall or switch versions later.
