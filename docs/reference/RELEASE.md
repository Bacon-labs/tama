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

## Signing

The release workflow writes a cumulative `manifest.json` with `schema = "tama.release-manifest.v1"`, a `stable` version, an optional `nightly` version preserved from the previous signed manifest, and a `releases[]` list. It computes SHA-256 for every new archive, preserves older release entries only after verifying the previously published manifest signature and rejecting unknown fields, empty release artifact lists, duplicate release versions, duplicate platform artifacts, dangling channel references, or mixed manifest shapes, signs the new manifest, and publishes the manifest, signature, and archives.

The schema-less legacy `version` plus `artifacts[]` manifest shape is accepted only for local/offline tests. Installers reject cumulative manifests that omit `releases[]`, include release entries without artifacts, or mix the legacy and cumulative shapes.

Published artifact URLs must use `https://` with a host. Absolute-path `file://`
artifact URLs are accepted only for signed local/offline manifests used by
installer tests or manual recovery.

`tamaup` verifies the manifest signature in process and verifies archive SHA-256 before extraction. `installer/install.sh` is a thin TLS-only bootstrap (matching the Foundry installer model): it fetches the cumulative manifest from `https://github.com/bacon-labs/tama/releases/latest/download/manifest.json`, validates manifest field safety, and verifies archive SHA-256 against the manifest before installing. It does not require `minisign`.

`tamaup install` also checks Lean/Lake, Foundry, and solc. Missing or incompatible tools fail closed unless the user passes the matching `--no-install-*` opt-out, or passes `--yes` to allow bootstrap installation. Bootstrap installation is disabled in `--offline` mode.

The release workflow lives in `.github/workflows/release.yml`. Configure these repository secrets before tagging a release:

- `TAMA_MINISIGN_SECRET_KEY`: unencrypted minisign secret key used only by CI signing.
- `TAMA_MINISIGN_PUBLIC_KEY`: public key matching the embedded `tamaup` verifier key.

Rotate keys by updating the repository secrets, the `tamaup` embedded public key, and the published release docs in one release.

## Website

The release workflow publishes the static GitHub Pages site at `https://tama.tools/`. The site lives under `docs/` and includes:

- installation instructions matching `docs/reference/SPEC.md` section 12:

```sh
curl -L https://tama.tools/install.sh | sh
```

- links to quickstart, command reference, audit guide, generated-artifact rules, troubleshooting, Verity compatibility, limitations, release artifacts, `install.sh`, and the GitHub Releases-hosted `manifest.json` and `manifest.json.minisig`;
- links to the telemetry-free privacy statement;
- no Windows installation path for v0.1.

## Installer Safety

Installation must reject unknown or unsafe manifest fields, bad signatures, bad hashes, absolute archive paths, `..` traversal, duplicate binary entries, and unexpected archive file names before any binary is installed.

`tamaup uninstall` removes the active `tama` binary and active marker, but keeps `tamaup` available so users can reinstall or switch versions later.
