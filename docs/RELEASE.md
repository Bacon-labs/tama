# Release

Release candidates must pass all non-negotiable gates in `docs/IMPLEMENTATION_PLAN.md`.

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

The release workflow writes a cumulative `manifest.json` with `schema = "tama.release-manifest.v1"`, a `stable` version, and a `releases[]` list. It computes SHA-256 for every new archive, preserves older release entries only after verifying the previously published manifest signature and rejecting unknown fields, empty release artifact lists, or mixed manifest shapes, signs the new manifest, and publishes the manifest, signature, and archives.

The schema-less legacy `version` plus `artifacts[]` manifest shape is accepted only for local/offline tests. Installers reject cumulative manifests that omit `releases[]`, include release entries without artifacts, or mix the legacy and cumulative shapes.

`tamaup` verifies the manifest signature in process and verifies archive SHA-256 before extraction. `installer/install.sh` may require an external signature verifier, but it must hard fail if verification is unavailable.

`tamaup install` also checks Lean/Lake, Foundry, and solc. Missing or incompatible tools fail closed unless the user passes the matching `--no-install-*` opt-out, or passes `--yes` to allow bootstrap installation. Bootstrap installation is disabled in `--offline` mode.

The release workflow lives in `.github/workflows/release.yml`. Configure these repository secrets before tagging a release:

- `TAMA_MINISIGN_SECRET_KEY`: unencrypted minisign secret key used only by CI signing.
- `TAMA_MINISIGN_PUBLIC_KEY`: public key matching the embedded `tamaup` verifier key and installer default.

Rotate keys by updating the repository secrets, the `tamaup` embedded public key, `installer/install.sh`, and the published release docs in one release.

## Website

The release workflow publishes the static GitHub Pages site at `https://tama.tools/`. The site lives under `site/` and includes:

- installation instructions matching `docs/SPEC.md` section 12:

```sh
curl -L https://tama.tools/install.sh | sh
```

- links to quickstart, command reference, audit guide, generated-artifact rules, Verity compatibility, limitations, release artifacts, `install.sh`, `manifest.json`, and `manifest.json.minisig`;
- links to the telemetry-free privacy statement;
- no Windows installation path for v0.1.

## Installer Safety

Installation must reject unknown or unsafe manifest fields, bad signatures, bad hashes, absolute archive paths, `..` traversal, and unexpected archive file names before any binary is installed.

`tamaup uninstall` removes the active `tama` binary and active marker, but keeps `tamaup` available so users can reinstall or switch versions later.
