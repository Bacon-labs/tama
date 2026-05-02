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

## Artifacts

Build four platform archives:

- `linux-x86_64`
- `linux-aarch64`
- `macos-x86_64`
- `macos-aarch64`

Each archive contains `tama` and `tamaup`. No Windows archive is published for v0.1.

## Signing

The release workflow writes a cumulative `manifest.json`, computes SHA-256 for every archive, signs the manifest, and publishes the manifest, signature, and archives.

`tamaup` verifies the manifest signature in process and verifies archive SHA-256 before extraction. `installer/install.sh` may require an external signature verifier, but it must hard fail if verification is unavailable.

## Website

The release workflow publishes the static GitHub Pages site at `https://tama.tools/`. The site lives under `site/` and includes:

- installation instructions matching `docs/SPEC.md` section 12:

```sh
curl -L https://tama.tools/install.sh | sh
```

- links to quickstart, command reference, audit guide, limitations, release artifacts, `install.sh`, `manifest.json`, and `manifest.json.minisig`;
- no Windows installation path for v0.1.

## Installer Safety

Extraction must reject absolute paths, `..` traversal, unexpected file names, bad signatures, and bad hashes before any binary is installed.
