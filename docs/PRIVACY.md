# Privacy

Tama does not collect telemetry and does not phone home during normal project commands.

Network access occurs only when the user runs commands that explicitly need external tools or dependencies:

- `tamaup install` and `tamaup self update` download the signed release manifest and selected release archive.
- `installer/install.sh` downloads the signed release manifest, selected archive, and bootstrap assets unless `--offline --manifest-file` is used.
- `tama init` runs `lake update` and a pinned `forge install foundry-rs/forge-std@v1.16.1 --shallow` unless `--offline` is used.
- `tama install` validates remote Tama packages and runs `lake update`.
- `tama remove` runs `lake update` after editing dependencies.
- `tama update` runs `lake update` and `forge update` unless `--no-lake` and `--no-forge` are used. Local refreshes that skip Lake refuse Verity dependency drift before editing `lakefile.toml`.

`TAMA_LAKE_PACKAGE_CACHE` is a local cache only. Tama copies package checkouts between that cache and `.lake/packages` to avoid repeated downloads, but the cache is not uploaded by Tama. For `tama check` and `tama build`, cached packages are seeded only when their Git HEAD matches the revision pinned in `lake-manifest.json`.

`tama check` and `tama build` do not run `lake update`; with global `--offline`, they use the project's existing local Lake state and manifest-matching cached Lake packages. `tama build --offline` also passes `--offline` to `forge build`.

`tama build`, `tama check`, `tama test`, `tama audit`, `tama inspect`, `tama clean`, and `tama doctor` do not intentionally contact Tama-operated services. They may execute external tools such as Lake, Forge, or solc, so their own network behavior depends on how those tools are configured and whether required dependencies are already installed.
